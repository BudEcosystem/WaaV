//! Yandex SpeechKit STT Client Implementation
//!
//! This module provides the STT implementation for Yandex SpeechKit API v1,
//! using HTTP POST requests for synchronous speech recognition.

use async_trait::async_trait;
use bytes::Bytes;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::RwLock;
use tracing::{debug, error, info};

use super::config::YandexSTTConfig;
use super::messages::{YandexSTTApiError, YandexSTTStatusCode, YandexSyncResponse};
use crate::core::stt::base::{
    BaseSTT, STTConfig, STTError, STTErrorCallback, STTResult, STTResultCallback,
};

// =============================================================================
// Constants
// =============================================================================

/// Yandex STT synchronous recognition endpoint
pub const YANDEX_STT_RECOGNIZE_URL: &str =
    "https://stt.api.cloud.yandex.net/speech/v1/stt:recognize";

/// Maximum audio size for synchronous recognition (1 MB)
pub const MAX_SYNC_AUDIO_SIZE: usize = 1_024_000;

/// Maximum audio duration for synchronous recognition (30 seconds)
pub const MAX_SYNC_AUDIO_DURATION_SECS: u32 = 30;

/// Audio chunk collection interval for pseudo-streaming (ms)
const CHUNK_COLLECTION_INTERVAL_MS: u64 = 100;

// =============================================================================
// Yandex STT Provider
// =============================================================================

/// Yandex SpeechKit STT provider
///
/// Implements speech-to-text using the Yandex SpeechKit API v1.
/// Uses HTTP POST requests for synchronous recognition.
///
/// For streaming recognition, this provider collects audio chunks
/// and periodically sends them to the API for recognition.
pub struct YandexSTT {
    /// Base STT configuration
    config: STTConfig,
    /// Yandex-specific configuration
    yandex_config: YandexSTTConfig,
    /// HTTP client
    client: reqwest::Client,
    /// Connection state
    connected: AtomicBool,
    /// Result callback
    result_callback: Arc<RwLock<Option<STTResultCallback>>>,
    /// Error callback
    error_callback: Arc<RwLock<Option<STTErrorCallback>>>,
    /// Audio buffer for pseudo-streaming
    audio_buffer: Arc<RwLock<Vec<u8>>>,
    /// Pending audio processing task
    processing_task: Arc<RwLock<Option<tokio::task::JoinHandle<()>>>>,
    /// Flag to stop processing
    stop_flag: Arc<AtomicBool>,
}

impl YandexSTT {
    /// Create a new Yandex STT provider from STTConfig
    pub fn create(config: STTConfig) -> Result<Self, STTError> {
        let yandex_config = YandexSTTConfig::from_base(&config)?;

        info!(
            "Creating Yandex STT provider: language={}, format={}, model={}",
            yandex_config.language.as_code(),
            yandex_config.audio_format.as_api_str(),
            yandex_config.model.as_topic()
        );

        Ok(Self {
            config,
            yandex_config,
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .map_err(|e| STTError::ConnectionFailed(e.to_string()))?,
            connected: AtomicBool::new(false),
            result_callback: Arc::new(RwLock::new(None)),
            error_callback: Arc::new(RwLock::new(None)),
            audio_buffer: Arc::new(RwLock::new(Vec::with_capacity(MAX_SYNC_AUDIO_SIZE))),
            processing_task: Arc::new(RwLock::new(None)),
            stop_flag: Arc::new(AtomicBool::new(false)),
        })
    }

    /// W1 keystone — construct directly from the standardized config so the advanced features
    /// Yandex can express (diarization, partials, profanity filtering, custom-vocabulary hints)
    /// are honored END-TO-END. The flat `BaseSTT::new` path resets those to provider defaults;
    /// this is the reachable standardized path. Mirrors `DeepgramSTT::new_standard`.
    pub fn new_standard(
        std: &crate::core::stt::standard::StandardSTTConfig,
    ) -> Result<Self, STTError> {
        if std.base.api_key.is_empty() {
            return Err(STTError::AuthenticationFailed(
                "Yandex API key is required".to_string(),
            ));
        }
        let yandex_config = YandexSTTConfig::from_standard(std)?;

        Ok(Self {
            config: std.base.clone(),
            yandex_config,
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .map_err(|e| STTError::ConnectionFailed(e.to_string()))?,
            connected: AtomicBool::new(false),
            result_callback: Arc::new(RwLock::new(None)),
            error_callback: Arc::new(RwLock::new(None)),
            audio_buffer: Arc::new(RwLock::new(Vec::with_capacity(MAX_SYNC_AUDIO_SIZE))),
            processing_task: Arc::new(RwLock::new(None)),
            stop_flag: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Recognize audio using synchronous API
    async fn recognize_sync(&self, audio_data: &[u8]) -> Result<String, STTError> {
        // Build headers
        let mut headers = HeaderMap::new();

        // Authorization header
        if let Ok(auth) = HeaderValue::from_str(&self.yandex_config.auth_header_value()) {
            headers.insert(AUTHORIZATION, auth);
        }

        // Content-Type for audio
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static(self.yandex_config.audio_format.content_type()),
        );

        // Build query parameters
        let params = self.yandex_config.build_query_params();

        debug!(
            "Yandex STT request: lang={}, format={}, audio_len={}",
            self.yandex_config.language.as_code(),
            self.yandex_config.audio_format.as_api_str(),
            audio_data.len()
        );

        // Send request
        let response = self
            .client
            .post(self.yandex_config.recognize_url())
            .headers(headers)
            .query(&params)
            .body(audio_data.to_vec())
            .send()
            .await
            .map_err(|e| STTError::NetworkError(format!("Failed to send request: {}", e)))?;

        let status = response.status();
        let status_code = YandexSTTStatusCode::from_http_status(status.as_u16());

        if status_code.is_success() {
            let response_text = response
                .text()
                .await
                .map_err(|e| STTError::NetworkError(format!("Failed to read response: {}", e)))?;

            // Parse response
            let sync_response: YandexSyncResponse = serde_json::from_str(&response_text)
                .map_err(|e| STTError::ProviderError(format!("Failed to parse response: {}", e)))?;

            if let Some(error) = sync_response.error {
                return Err(STTError::ProviderError(error.display_message()));
            }

            Ok(sync_response.result.unwrap_or_default())
        } else {
            let body = response.text().await.unwrap_or_default();
            let error = YandexSTTApiError::from_response(status.as_u16(), &body);

            if error.is_auth_error() {
                Err(STTError::AuthenticationFailed(error.display_message()))
            } else if error.is_rate_limit_error() {
                Err(STTError::ProviderError(format!(
                    "Rate limited: {}",
                    error.display_message()
                )))
            } else {
                Err(STTError::ProviderError(error.display_message()))
            }
        }
    }

    /// Start the audio processing task for pseudo-streaming
    async fn start_processing_task(&self) {
        let audio_buffer = Arc::clone(&self.audio_buffer);
        let result_callback = Arc::clone(&self.result_callback);
        let error_callback = Arc::clone(&self.error_callback);
        let stop_flag = Arc::clone(&self.stop_flag);
        let client = self.client.clone();
        let yandex_config = self.yandex_config.clone();

        let task = tokio::spawn(async move {
            let mut last_process_time = std::time::Instant::now();

            loop {
                if stop_flag.load(Ordering::Relaxed) {
                    break;
                }

                // Wait for chunk interval
                tokio::time::sleep(tokio::time::Duration::from_millis(
                    CHUNK_COLLECTION_INTERVAL_MS,
                ))
                .await;

                // Check if we should process
                let elapsed = last_process_time.elapsed();
                if elapsed < std::time::Duration::from_millis(500) {
                    continue;
                }

                // Get accumulated audio
                let audio_data = {
                    let mut buffer = audio_buffer.write().await;
                    if buffer.is_empty() {
                        continue;
                    }
                    std::mem::take(&mut *buffer)
                };

                // Skip if audio is too small (likely noise)
                if audio_data.len() < 1600 {
                    // Less than 100ms at 16kHz 16-bit
                    continue;
                }

                debug!("Processing {} bytes of accumulated audio", audio_data.len());
                last_process_time = std::time::Instant::now();

                // Build request
                let mut headers = HeaderMap::new();
                if let Ok(auth) = HeaderValue::from_str(&yandex_config.auth_header_value()) {
                    headers.insert(AUTHORIZATION, auth);
                }
                headers.insert(
                    CONTENT_TYPE,
                    HeaderValue::from_static(yandex_config.audio_format.content_type()),
                );

                let params = yandex_config.build_query_params();

                // Send request
                let result = client
                    .post(yandex_config.recognize_url())
                    .headers(headers)
                    .query(&params)
                    .body(audio_data)
                    .send()
                    .await;

                match result {
                    Ok(response) => {
                        let status = response.status();
                        if status.is_success() {
                            if let Ok(text) = response.text().await
                                && let Ok(sync_response) =
                                    serde_json::from_str::<YandexSyncResponse>(&text)
                                    && let Some(transcript) = sync_response.result
                                        && !transcript.is_empty() {
                                            let stt_result = STTResult::new(
                                                transcript, true, // is_final
                                                true, // is_speech_final
                                                0.95, // confidence (Yandex doesn't return this for sync)
                                            );

                                            if let Some(callback) =
                                                result_callback.read().await.as_ref()
                                            {
                                                callback(stt_result).await;
                                            }
                                        }
                        } else {
                            let body = response.text().await.unwrap_or_default();
                            let api_error =
                                YandexSTTApiError::from_response(status.as_u16(), &body);
                            error!(
                                "Yandex STT recognition failed: {}",
                                api_error.display_message()
                            );

                            if let Some(callback) = error_callback.read().await.as_ref() {
                                callback(STTError::ProviderError(api_error.display_message()))
                                    .await;
                            }
                        }
                    }
                    Err(e) => {
                        error!("Yandex STT request failed: {}", e);
                        if let Some(callback) = error_callback.read().await.as_ref() {
                            callback(STTError::NetworkError(e.to_string())).await;
                        }
                    }
                }
            }

            debug!("Yandex STT processing task stopped");
        });

        *self.processing_task.write().await = Some(task);
    }

    /// Stop the audio processing task
    async fn stop_processing_task(&self) {
        self.stop_flag.store(true, Ordering::Relaxed);

        if let Some(task) = self.processing_task.write().await.take() {
            task.abort();
        }
    }
}

// =============================================================================
// BaseSTT Implementation
// =============================================================================

#[async_trait]
impl BaseSTT for YandexSTT {
    fn new(config: STTConfig) -> Result<Self, STTError>
    where
        Self: Sized,
    {
        Self::create(config)
    }

    async fn connect(&mut self) -> Result<(), STTError> {
        debug!("Connecting to Yandex SpeechKit STT API");

        // Reset state
        self.stop_flag.store(false, Ordering::Relaxed);
        self.audio_buffer.write().await.clear();

        // Start processing task
        self.start_processing_task().await;

        self.connected.store(true, Ordering::Relaxed);
        info!("Connected to Yandex SpeechKit STT API");

        Ok(())
    }

    async fn disconnect(&mut self) -> Result<(), STTError> {
        debug!("Disconnecting from Yandex SpeechKit STT API");

        // Stop processing
        self.stop_processing_task().await;
        self.connected.store(false, Ordering::Relaxed);

        info!("Disconnected from Yandex SpeechKit STT API");
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    async fn send_audio(&mut self, audio_data: Bytes) -> Result<(), STTError> {
        if !self.is_ready() {
            return Err(STTError::ConnectionFailed(
                "Not connected. Call connect() first.".to_string(),
            ));
        }

        if audio_data.is_empty() {
            return Ok(());
        }

        // Add to buffer
        let mut buffer = self.audio_buffer.write().await;
        buffer.extend_from_slice(&audio_data);

        // Check buffer size limit
        if buffer.len() > MAX_SYNC_AUDIO_SIZE {
            // Process immediately if buffer is too large
            let audio_to_process = std::mem::take(&mut *buffer);
            drop(buffer);

            // Recognize synchronously
            match self.recognize_sync(&audio_to_process).await {
                Ok(transcript) => {
                    if !transcript.is_empty() {
                        let result = STTResult::new(transcript, true, true, 0.95);
                        if let Some(callback) = self.result_callback.read().await.as_ref() {
                            callback(result).await;
                        }
                    }
                }
                Err(e) => {
                    error!("Yandex STT recognition error: {}", e);
                    if let Some(callback) = self.error_callback.read().await.as_ref() {
                        callback(e).await;
                    }
                }
            }
        }

        Ok(())
    }

    async fn on_result(&mut self, callback: STTResultCallback) -> Result<(), STTError> {
        *self.result_callback.write().await = Some(callback);
        Ok(())
    }

    async fn on_error(&mut self, callback: STTErrorCallback) -> Result<(), STTError> {
        *self.error_callback.write().await = Some(callback);
        Ok(())
    }

    fn get_config(&self) -> Option<&STTConfig> {
        Some(&self.config)
    }

    async fn update_config(&mut self, config: STTConfig) -> Result<(), STTError> {
        // Validate new config
        let new_yandex_config = YandexSTTConfig::from_base(&config)?;

        // Update configs
        self.config = config;
        self.yandex_config = new_yandex_config;

        debug!("Yandex STT configuration updated");
        Ok(())
    }

    fn get_provider_info(&self) -> &'static str {
        "Yandex SpeechKit v1"
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_config() -> STTConfig {
        STTConfig {
            provider: "yandex".to_string(),
            api_key: "test-api-key".to_string(),
            language: "ru-RU".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "lpcm".to_string(),
            model: "general".to_string(),
        }
    }

    // W1 keystone: advanced features Yandex supports (diarization -> speaker_identification,
    // keyterms -> hints) must survive through `new_standard` into the provider-specific config,
    // instead of being reset to the provider default by the flat path. RED until `new_standard`
    // maps them.
    #[test]
    fn test_yandex_new_standard_unlocks_advanced_features() {
        use crate::core::stt::standard::{ProviderExtras, SttFeatures, StandardSTTConfig};
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "yandex".into(),
                api_key: "test-api-key".into(),
                ..Default::default()
            },
            features: SttFeatures {
                diarization: Some(true),
                keyterms: Some(vec!["WaaV".into(), "Yandex".into()]),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
        };
        let stt = YandexSTT::new_standard(&std).unwrap();
        assert!(stt.yandex_config.speaker_identification); // diarization
        assert_eq!(stt.yandex_config.hints, vec!["WaaV", "Yandex"]); // keyterms
    }

    #[test]
    fn test_yandex_new_standard_requires_api_key() {
        use crate::core::stt::standard::StandardSTTConfig;
        let std = StandardSTTConfig::from_base(STTConfig {
            api_key: String::new(),
            ..Default::default()
        });
        assert!(YandexSTT::new_standard(&std).is_err());
    }

    #[test]
    fn test_yandex_stt_creation() {
        let config = create_test_config();
        let stt = YandexSTT::new(config);
        assert!(stt.is_ok());
    }

    #[test]
    fn test_yandex_stt_requires_api_key() {
        let config = STTConfig {
            api_key: String::new(),
            ..Default::default()
        };

        let result = YandexSTT::new(config);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_yandex_stt_not_connected() {
        let config = create_test_config();
        let mut stt = YandexSTT::new(config).unwrap();

        // Should not be ready initially
        assert!(!stt.is_ready());

        // Should fail when not connected
        let result = stt.send_audio(Bytes::from(vec![0u8; 100])).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_yandex_stt_connect_disconnect() {
        let config = create_test_config();
        let mut stt = YandexSTT::new(config).unwrap();

        // Connect
        let result = stt.connect().await;
        assert!(result.is_ok());
        assert!(stt.is_ready());

        // Disconnect
        let result = stt.disconnect().await;
        assert!(result.is_ok());
        assert!(!stt.is_ready());
    }

    #[tokio::test]
    async fn test_yandex_stt_send_empty_audio() {
        let config = create_test_config();
        let mut stt = YandexSTT::new(config).unwrap();

        stt.connect().await.unwrap();

        // Empty audio should succeed
        let result = stt.send_audio(Bytes::new()).await;
        assert!(result.is_ok());

        stt.disconnect().await.unwrap();
    }

    #[test]
    fn test_yandex_stt_provider_info() {
        let config = create_test_config();
        let stt = YandexSTT::new(config).unwrap();

        assert_eq!(stt.get_provider_info(), "Yandex SpeechKit v1");
    }

    #[test]
    fn test_yandex_stt_get_config() {
        let config = create_test_config();
        let stt = YandexSTT::new(config).unwrap();

        let stored_config = stt.get_config().unwrap();
        assert_eq!(stored_config.api_key, "test-api-key");
        assert_eq!(stored_config.language, "ru-RU");
    }

    #[tokio::test]
    async fn test_yandex_stt_update_config() {
        let config = create_test_config();
        let mut stt = YandexSTT::new(config).unwrap();

        // Update with new config
        let new_config = STTConfig {
            provider: "yandex".to_string(),
            api_key: "new-api-key".to_string(),
            language: "en-US".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: false,
            encoding: "mp3".to_string(),
            model: "general".to_string(),
        };

        let result = stt.update_config(new_config).await;
        assert!(result.is_ok());

        let stored_config = stt.get_config().unwrap();
        assert_eq!(stored_config.api_key, "new-api-key");
        assert_eq!(stored_config.language, "en-US");
    }

    #[tokio::test]
    async fn test_yandex_stt_callback_registration() {
        use std::future::Future;
        use std::pin::Pin;

        let config = create_test_config();
        let mut stt = YandexSTT::new(config).unwrap();

        let callback: STTResultCallback = Arc::new(|_result: STTResult| {
            Box::pin(async move {}) as Pin<Box<dyn Future<Output = ()> + Send>>
        });

        let result = stt.on_result(callback).await;
        assert!(result.is_ok());
    }
}
