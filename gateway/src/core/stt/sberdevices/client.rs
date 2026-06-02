//! SberDevices SaluteSpeech STT Client Implementation
//!
//! This module provides the STT implementation for SberDevices SaluteSpeech API,
//! using HTTP POST requests for synchronous speech recognition with OAuth 2.0 auth.

use async_trait::async_trait;
use bytes::Bytes;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use super::config::{
    MAX_SYNC_AUDIO_SIZE, OAUTH_ENDPOINT, STT_RECOGNIZE_ENDPOINT, SberSTTConfig,
    TOKEN_REFRESH_THRESHOLD_SECS,
};
use super::messages::{
    OAuthTokenRequest, OAuthTokenResponse, SberApiError, SberRecognitionResponse, SberStatusCode,
};
use crate::core::stt::base::{
    BaseSTT, STTConfig, STTError, STTErrorCallback, STTResult, STTResultCallback,
};

// =============================================================================
// Constants
// =============================================================================

/// Audio chunk collection interval for pseudo-streaming (ms)
const CHUNK_COLLECTION_INTERVAL_MS: u64 = 100;

/// Minimum audio size to process (100ms at 16kHz 16-bit mono)
const MIN_AUDIO_SIZE: usize = 3200;

// =============================================================================
// Token Manager
// =============================================================================

/// OAuth token manager with automatic refresh
struct TokenManager {
    /// Current access token
    token: Option<OAuthTokenResponse>,
    /// HTTP client for token requests
    client: reqwest::Client,
    /// Client credentials (Base64)
    credentials: String,
    /// OAuth scope
    scope: String,
}

impl TokenManager {
    fn new(credentials: String, scope: String) -> Self {
        Self {
            token: None,
            client: reqwest::Client::new(),
            credentials,
            scope,
        }
    }

    /// Get a valid access token, refreshing if necessary
    async fn get_token(&mut self) -> Result<String, STTError> {
        // Check if current token is valid
        if let Some(ref token) = self.token {
            if !token.is_expired(TOKEN_REFRESH_THRESHOLD_SECS) {
                return Ok(token.access_token.clone());
            }
            debug!("Token expired or about to expire, refreshing...");
        }

        // Fetch new token
        self.refresh_token().await
    }

    /// Refresh the OAuth token
    async fn refresh_token(&mut self) -> Result<String, STTError> {
        debug!("Requesting new OAuth token from SberDevices");

        // Generate unique request ID
        let rq_uid = Uuid::new_v4().to_string();

        // Build request
        let response = self
            .client
            .post(OAUTH_ENDPOINT)
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .header("Accept", "application/json")
            .header("RqUID", &rq_uid)
            .header(AUTHORIZATION, format!("Basic {}", self.credentials))
            .form(&OAuthTokenRequest::new(&self.scope))
            .send()
            .await
            .map_err(|e| STTError::NetworkError(format!("Token request failed: {}", e)))?;

        let status = response.status();
        let status_code = SberStatusCode::from_http_status(status.as_u16());

        if status_code.is_success() {
            let token_response: OAuthTokenResponse = response
                .json()
                .await
                .map_err(|e| STTError::ProviderError(format!("Failed to parse token: {}", e)))?;

            debug!(
                "OAuth token obtained, valid for {} seconds",
                token_response.remaining_secs()
            );

            let access_token = token_response.access_token.clone();
            self.token = Some(token_response);

            Ok(access_token)
        } else {
            let body = response.text().await.unwrap_or_default();
            let error = SberApiError::from_response(status.as_u16(), &body);

            error!("Failed to obtain OAuth token: {}", error.display_message());

            Err(STTError::AuthenticationFailed(error.display_message()))
        }
    }

    /// Invalidate the current token (force refresh on next request)
    fn invalidate(&mut self) {
        self.token = None;
    }
}

// =============================================================================
// SberDevices STT Provider
// =============================================================================

/// SberDevices SaluteSpeech STT provider
///
/// Implements speech-to-text using the SberDevices SaluteSpeech REST API.
/// Uses OAuth 2.0 authentication with automatic token refresh.
pub struct SberDevicesSTT {
    /// Base STT configuration
    config: STTConfig,
    /// SberDevices-specific configuration
    sber_config: SberSTTConfig,
    /// HTTP client
    client: reqwest::Client,
    /// OAuth token manager
    token_manager: Arc<RwLock<TokenManager>>,
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

impl SberDevicesSTT {
    /// Create a new SberDevices STT provider from STTConfig
    pub fn create(config: STTConfig) -> Result<Self, STTError> {
        let sber_config = SberSTTConfig::from_base(&config)?;
        Self::from_sber_config(config, sber_config)
    }

    /// W1 keystone — construct directly from the standardized config so it is honored END-TO-END.
    /// SberDevices SaluteSpeech is a minimal synchronous REST recognizer that exposes none of the
    /// standardized advanced knobs, so `from_standard` is a pure `from_base` passthrough (capability
    /// gaps stay at provider defaults); this keeps the standardized dispatch path reachable.
    pub fn new_standard(
        std: &crate::core::stt::standard::StandardSTTConfig,
    ) -> Result<Self, STTError> {
        let sber_config = SberSTTConfig::from_standard(std)?;
        Self::from_sber_config(std.base.clone(), sber_config)
    }

    /// Internal: construct the provider from an already-mapped SberDevices config.
    fn from_sber_config(config: STTConfig, sber_config: SberSTTConfig) -> Result<Self, STTError> {
        info!(
            "Creating SberDevices STT provider: language={}, format={:?}",
            sber_config.language.as_code(),
            sber_config.audio_format.as_api_str()
        );

        let token_manager = TokenManager::new(
            sber_config.client_credentials.clone(),
            sber_config.scope.as_str().to_string(),
        );

        Ok(Self {
            config,
            sber_config: sber_config.clone(),
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(
                    sber_config.request_timeout_secs,
                ))
                .connect_timeout(std::time::Duration::from_secs(
                    sber_config.connection_timeout_secs,
                ))
                .build()
                .map_err(|e| STTError::ConnectionFailed(e.to_string()))?,
            token_manager: Arc::new(RwLock::new(token_manager)),
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
        // Get access token
        let access_token = self.token_manager.write().await.get_token().await?;

        // Build headers
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", access_token))
                .map_err(|_| STTError::ConfigurationError("Invalid token".to_string()))?,
        );
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_str(&self.sber_config.audio_content_type())
                .map_err(|_| STTError::ConfigurationError("Invalid content type".to_string()))?,
        );

        debug!(
            "SberDevices STT request: lang={}, format={}, audio_len={}",
            self.sber_config.language.as_code(),
            self.sber_config.audio_format.as_api_str(),
            audio_data.len()
        );

        // Send request
        let response = self
            .client
            .post(STT_RECOGNIZE_ENDPOINT)
            .headers(headers)
            .body(audio_data.to_vec())
            .send()
            .await
            .map_err(|e| STTError::NetworkError(format!("Failed to send request: {}", e)))?;

        let status = response.status();
        let status_code = SberStatusCode::from_http_status(status.as_u16());

        if status_code.is_success() {
            let response_text = response
                .text()
                .await
                .map_err(|e| STTError::NetworkError(format!("Failed to read response: {}", e)))?;

            // Parse response
            let recognition_response: SberRecognitionResponse =
                serde_json::from_str(&response_text).map_err(|e| {
                    STTError::ProviderError(format!("Failed to parse response: {}", e))
                })?;

            if recognition_response.is_error() {
                return Err(STTError::ProviderError(
                    recognition_response.error_message(),
                ));
            }

            Ok(recognition_response.get_all_transcripts())
        } else {
            let body = response.text().await.unwrap_or_default();
            let error = SberApiError::from_response(status.as_u16(), &body);

            // Invalidate token if auth error
            if error.is_auth_error() {
                self.token_manager.write().await.invalidate();
                return Err(STTError::AuthenticationFailed(error.display_message()));
            }

            if error.is_rate_limit_error() {
                return Err(STTError::ProviderError(format!(
                    "Rate limited: {}",
                    error.display_message()
                )));
            }

            Err(STTError::ProviderError(error.display_message()))
        }
    }

    /// Start the audio processing task for pseudo-streaming
    async fn start_processing_task(&self) {
        let audio_buffer = Arc::clone(&self.audio_buffer);
        let result_callback = Arc::clone(&self.result_callback);
        let error_callback = Arc::clone(&self.error_callback);
        let stop_flag = Arc::clone(&self.stop_flag);
        let token_manager = Arc::clone(&self.token_manager);
        let client = self.client.clone();
        let sber_config = self.sber_config.clone();

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
                if audio_data.len() < MIN_AUDIO_SIZE {
                    continue;
                }

                debug!("Processing {} bytes of accumulated audio", audio_data.len());
                last_process_time = std::time::Instant::now();

                // Get access token
                let access_token = match token_manager.write().await.get_token().await {
                    Ok(token) => token,
                    Err(e) => {
                        error!("Failed to get access token: {}", e);
                        if let Some(callback) = error_callback.read().await.as_ref() {
                            callback(e).await;
                        }
                        continue;
                    }
                };

                // Build request
                let mut headers = HeaderMap::new();
                if let Ok(auth) = HeaderValue::from_str(&format!("Bearer {}", access_token)) {
                    headers.insert(AUTHORIZATION, auth);
                }
                if let Ok(ct) = HeaderValue::from_str(&sber_config.audio_content_type()) {
                    headers.insert(CONTENT_TYPE, ct);
                }

                // Send request
                let result = client
                    .post(STT_RECOGNIZE_ENDPOINT)
                    .headers(headers)
                    .body(audio_data)
                    .send()
                    .await;

                match result {
                    Ok(response) => {
                        let status = response.status();
                        if status.is_success() {
                            if let Ok(text) = response.text().await
                                && let Ok(recognition_response) =
                                    serde_json::from_str::<SberRecognitionResponse>(&text)
                                {
                                    let transcript = recognition_response.get_all_transcripts();
                                    if !transcript.is_empty() {
                                        let stt_result = STTResult::new(
                                            transcript, true, // is_final
                                            true, // is_speech_final
                                            0.95, // confidence (Sber doesn't return this)
                                        );

                                        if let Some(callback) =
                                            result_callback.read().await.as_ref()
                                        {
                                            callback(stt_result).await;
                                        }
                                    }
                                }
                        } else {
                            let body = response.text().await.unwrap_or_default();
                            let api_error = SberApiError::from_response(status.as_u16(), &body);

                            // Invalidate token if auth error
                            if api_error.is_auth_error() {
                                token_manager.write().await.invalidate();
                            }

                            error!(
                                "SberDevices STT recognition failed: {}",
                                api_error.display_message()
                            );

                            if let Some(callback) = error_callback.read().await.as_ref() {
                                callback(STTError::ProviderError(api_error.display_message()))
                                    .await;
                            }
                        }
                    }
                    Err(e) => {
                        error!("SberDevices STT request failed: {}", e);
                        if let Some(callback) = error_callback.read().await.as_ref() {
                            callback(STTError::NetworkError(e.to_string())).await;
                        }
                    }
                }
            }

            debug!("SberDevices STT processing task stopped");
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
impl BaseSTT for SberDevicesSTT {
    fn new(config: STTConfig) -> Result<Self, STTError>
    where
        Self: Sized,
    {
        Self::create(config)
    }

    async fn connect(&mut self) -> Result<(), STTError> {
        debug!("Connecting to SberDevices SaluteSpeech STT API");

        // Pre-fetch OAuth token to validate credentials
        self.token_manager.write().await.get_token().await?;

        // Reset state
        self.stop_flag.store(false, Ordering::Relaxed);
        self.audio_buffer.write().await.clear();

        // Start processing task
        self.start_processing_task().await;

        self.connected.store(true, Ordering::Relaxed);
        info!("Connected to SberDevices SaluteSpeech STT API");

        Ok(())
    }

    async fn disconnect(&mut self) -> Result<(), STTError> {
        debug!("Disconnecting from SberDevices SaluteSpeech STT API");

        // Stop processing
        self.stop_processing_task().await;
        self.connected.store(false, Ordering::Relaxed);

        info!("Disconnected from SberDevices SaluteSpeech STT API");
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

            warn!(
                "Audio buffer exceeded limit, processing {} bytes immediately",
                audio_to_process.len()
            );

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
                    error!("SberDevices STT recognition error: {}", e);
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
        let new_sber_config = SberSTTConfig::from_base(&config)?;

        // Update token manager if credentials changed
        if new_sber_config.client_credentials != self.sber_config.client_credentials
            || new_sber_config.scope != self.sber_config.scope
        {
            let mut tm = self.token_manager.write().await;
            *tm = TokenManager::new(
                new_sber_config.client_credentials.clone(),
                new_sber_config.scope.as_str().to_string(),
            );
        }

        // Update configs
        self.config = config;
        self.sber_config = new_sber_config;

        debug!("SberDevices STT configuration updated");
        Ok(())
    }

    fn get_provider_info(&self) -> &'static str {
        "SberDevices SaluteSpeech v1"
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
            provider: "sberdevices".to_string(),
            api_key: "test_client:test_secret".to_string(),
            language: "ru-RU".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "pcm".to_string(),
            model: "SALUTE_SPEECH_PERS".to_string(),
        }
    }

    #[test]
    fn test_sberdevices_stt_creation() {
        let config = create_test_config();
        let stt = SberDevicesSTT::new(config);
        assert!(stt.is_ok());
    }

    // W1 keystone: the standardized config must build the provider THROUGH `new_standard`, with
    // the base (credentials/language/scope and the punctuation knob Sber supports) surviving into
    // the provider's `sber_config`. Advanced streaming features are a capability gap (Sber is a
    // minimal sync recognizer) and stay at default — they are set here but must be ignored.
    #[test]
    fn test_new_standard_carries_base_features() {
        use crate::core::stt::standard::{ProviderExtras, SttFeatures, StandardSTTConfig};
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "sberdevices".into(),
                api_key: "test_client:test_secret".into(),
                language: "ru-RU".into(),
                punctuation: true,
                model: "SALUTE_SPEECH_PERS".into(),
                ..Default::default()
            },
            features: SttFeatures {
                diarization: Some(true),
                interim_results: Some(true),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
        };
        let stt = SberDevicesSTT::new_standard(&std).unwrap();
        use base64::{Engine, engine::general_purpose::STANDARD};
        let expected = STANDARD.encode("test_client:test_secret".as_bytes());
        use super::super::config::{SberSTTLanguage, SberScope};
        assert_eq!(stt.sber_config.client_credentials, expected);
        assert_eq!(stt.sber_config.language, SberSTTLanguage::Russian);
        assert_eq!(stt.sber_config.scope, SberScope::Personal);
        assert!(stt.sber_config.enable_punctuation); // base punctuation survived new_standard

        // Empty credentials are rejected through the standardized path too.
        let bad = StandardSTTConfig::from_base(STTConfig {
            provider: "sberdevices".into(),
            api_key: String::new(),
            ..Default::default()
        });
        assert!(SberDevicesSTT::new_standard(&bad).is_err());
    }

    #[test]
    fn test_sberdevices_stt_requires_credentials() {
        let config = STTConfig {
            api_key: String::new(),
            ..Default::default()
        };

        let result = SberDevicesSTT::new(config);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_sberdevices_stt_not_connected() {
        let config = create_test_config();
        let mut stt = SberDevicesSTT::new(config).unwrap();

        // Should not be ready initially
        assert!(!stt.is_ready());

        // Should fail when not connected
        let result = stt.send_audio(Bytes::from(vec![0u8; 100])).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_sberdevices_stt_provider_info() {
        let config = create_test_config();
        let stt = SberDevicesSTT::new(config).unwrap();

        assert_eq!(stt.get_provider_info(), "SberDevices SaluteSpeech v1");
    }

    #[test]
    fn test_sberdevices_stt_get_config() {
        let config = create_test_config();
        let stt = SberDevicesSTT::new(config).unwrap();

        let stored_config = stt.get_config().unwrap();
        assert_eq!(stored_config.language, "ru-RU");
        assert_eq!(stored_config.sample_rate, 16000);
    }

    #[tokio::test]
    async fn test_sberdevices_stt_send_empty_audio() {
        let config = create_test_config();
        let mut stt = SberDevicesSTT::new(config).unwrap();

        // Manually set connected state for testing
        stt.connected.store(true, Ordering::Relaxed);

        // Empty audio should succeed
        let result = stt.send_audio(Bytes::new()).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_sberdevices_stt_callback_registration() {
        use std::future::Future;
        use std::pin::Pin;

        let config = create_test_config();
        let mut stt = SberDevicesSTT::new(config).unwrap();

        let callback: STTResultCallback = Arc::new(|_result: STTResult| {
            Box::pin(async move {}) as Pin<Box<dyn Future<Output = ()> + Send>>
        });

        let result = stt.on_result(callback).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_sberdevices_stt_error_callback_registration() {
        use std::future::Future;
        use std::pin::Pin;

        let config = create_test_config();
        let mut stt = SberDevicesSTT::new(config).unwrap();

        let callback: STTErrorCallback = Arc::new(|_error: STTError| {
            Box::pin(async move {}) as Pin<Box<dyn Future<Output = ()> + Send>>
        });

        let result = stt.on_error(callback).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_sberdevices_stt_update_config() {
        let config = create_test_config();
        let mut stt = SberDevicesSTT::new(config).unwrap();

        // Update with new config
        let new_config = STTConfig {
            provider: "sberdevices".to_string(),
            api_key: "new_client:new_secret".to_string(),
            language: "en-US".to_string(),
            sample_rate: 8000,
            channels: 1,
            punctuation: false,
            encoding: "opus".to_string(),
            model: "SALUTE_SPEECH_CORP".to_string(),
        };

        let result = stt.update_config(new_config).await;
        assert!(result.is_ok());

        let stored_config = stt.get_config().unwrap();
        assert_eq!(stored_config.language, "en-US");
        assert_eq!(stored_config.sample_rate, 8000);
    }

    #[test]
    fn test_sberdevices_stt_credentials_encoding() {
        // Test raw credentials get encoded
        let config = STTConfig {
            api_key: "client_id:client_secret".to_string(),
            ..Default::default()
        };

        let stt = SberDevicesSTT::new(config).unwrap();

        // Verify the credentials are Base64 encoded
        use base64::{Engine, engine::general_purpose::STANDARD};
        let expected = STANDARD.encode("client_id:client_secret".as_bytes());
        assert_eq!(stt.sber_config.client_credentials, expected);
    }
}
