//! Viettel AI STT client implementation.
//!
//! This module provides a REST API-based STT client for Viettel Group's
//! AI Speech-to-Text service.
//!
//! # Protocol
//!
//! Viettel AI STT uses HTTP multipart file upload:
//! 1. Collect audio data
//! 2. POST audio file to decode endpoint
//! 3. Receive JSON response with transcription
//!
//! # Audio Format
//!
//! Supports WAV, PCM, MP3 formats:
//! - Sample Rate: 8kHz or 16kHz recommended
//! - Channels: Mono recommended
//! - PCM: Linear16 (S16LE)

use super::config::{
    MAX_AUDIO_BUFFER_SIZE, MIN_AUDIO_BUFFER_SIZE, VIETTEL_STT_ENDPOINT, ViettelSttConfig,
    ViettelSttResponse,
};
use crate::core::stt::base::{
    BaseSTT, STTConfig, STTConnectionState, STTError, STTErrorCallback, STTResult,
    STTResultCallback,
};
use bytes::Bytes;
use reqwest::{
    Client,
    multipart::{Form, Part},
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

/// Viettel AI STT client.
///
/// This client buffers audio data and sends it via HTTP multipart upload when flushed.
/// Viettel AI supports various audio formats but recommends WAV or PCM.
pub struct ViettelStt {
    /// Provider configuration.
    config: ViettelSttConfig,
    /// Base STT configuration.
    base_config: Option<STTConfig>,
    /// HTTP client for REST API calls.
    http_client: Client,
    /// Whether the client is ready to receive audio.
    is_ready: AtomicBool,
    /// Current connection state.
    connection_state: Arc<RwLock<STTConnectionState>>,
    /// Audio buffer for collecting audio data.
    audio_buffer: Arc<RwLock<Vec<u8>>>,
    /// Result callback.
    result_callback: Arc<RwLock<Option<STTResultCallback>>>,
    /// Error callback.
    error_callback: Arc<RwLock<Option<STTErrorCallback>>>,
}

impl ViettelStt {
    /// W1 keystone — construct directly from the standardized config. Viettel AI is a simple
    /// batch decode endpoint that exposes no advanced-feature surface, so `from_standard` is a
    /// pure `from_base` passthrough: this gives a uniform standardized entry point that carries
    /// the base config through unchanged. Mirrors `DeepgramSTT::new_standard`.
    pub fn new_standard(
        std: &crate::core::stt::standard::StandardSTTConfig,
    ) -> Result<Self, STTError> {
        if std.base.api_key.is_empty() {
            return Err(STTError::AuthenticationFailed(
                "Viettel AI API token is required".to_string(),
            ));
        }
        let viettel_config = ViettelSttConfig::from_standard(std)?;

        let timeout_secs = viettel_config.request_timeout_secs;
        let http_client = Client::builder()
            .timeout(std::time::Duration::from_secs(timeout_secs))
            .build()
            .map_err(|e| {
                STTError::ConfigurationError(format!("Failed to create HTTP client: {e}"))
            })?;

        Ok(Self {
            config: viettel_config,
            base_config: Some(std.base.clone()),
            http_client,
            is_ready: AtomicBool::new(false),
            connection_state: Arc::new(RwLock::new(STTConnectionState::Disconnected)),
            audio_buffer: Arc::new(RwLock::new(Vec::with_capacity(MAX_AUDIO_BUFFER_SIZE))),
            result_callback: Arc::new(RwLock::new(None)),
            error_callback: Arc::new(RwLock::new(None)),
        })
    }

    /// Process buffered audio and get transcription.
    async fn process_audio(&self, audio_data: Vec<u8>) -> Result<String, STTError> {
        if audio_data.len() < MIN_AUDIO_BUFFER_SIZE {
            warn!(
                "Viettel STT: Audio buffer too small ({} bytes), minimum is {} bytes",
                audio_data.len(),
                MIN_AUDIO_BUFFER_SIZE
            );
            return Ok(String::new());
        }

        debug!(
            "Viettel STT: Processing {} bytes of audio",
            audio_data.len()
        );

        // Build WAV header for raw PCM data
        let wav_data = self.wrap_in_wav(&audio_data);

        // Create multipart form with audio file
        let part = Part::bytes(wav_data)
            .file_name("audio.wav")
            .mime_str("audio/wav")
            .map_err(|e| {
                STTError::ConfigurationError(format!("Failed to create form part: {e}"))
            })?;

        let form = Form::new().part("file", part);

        // Build request with headers
        let mut request_builder = self
            .http_client
            .post(VIETTEL_STT_ENDPOINT)
            .header("token", &self.config.api_key);

        // Add PCM format headers if needed
        request_builder = request_builder
            .header("sample_rate", self.config.sample_rate.to_string())
            .header("format", &self.config.format)
            .header("num_of_channels", self.config.channels.to_string());

        // Add ASR model if specified
        if let Some(model) = &self.config.asr_model {
            request_builder = request_builder.header("asr_model", model);
        }

        // Send request
        let response = request_builder
            .multipart(form)
            .send()
            .await
            .map_err(|e| STTError::NetworkError(format!("Failed to send STT request: {e}")))?;

        let status = response.status();

        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unable to read error body".to_string());

            error!(
                "Viettel STT: API error (status {}): {}",
                status.as_u16(),
                body
            );

            return match status.as_u16() {
                401 => Err(STTError::AuthenticationFailed(format!(
                    "Invalid or expired token: {body}"
                ))),
                429 => Err(STTError::ProviderError(format!("Rate limited: {body}"))),
                400 => Err(STTError::ConfigurationError(format!("Bad request: {body}"))),
                _ => Err(STTError::ProviderError(format!(
                    "API error (status {}): {}",
                    status.as_u16(),
                    body
                ))),
            };
        }

        // Parse JSON response
        let api_response: ViettelSttResponse = response
            .json()
            .await
            .map_err(|e| STTError::ProviderError(format!("Failed to parse API response: {e}")))?;

        // Check for API-level errors
        if !api_response.is_success() {
            let error_msg = api_response.status_message();

            // Status 1 (no voice) is not really an error
            if api_response.status == 1 {
                debug!("Viettel STT: No voice detected in audio");
                return Ok(String::new());
            }

            error!(
                "Viettel STT: API error status {}: {}",
                api_response.status, error_msg
            );

            return Err(STTError::ProviderError(error_msg.to_string()));
        }

        // Get transcription
        let transcription = api_response.transcription().unwrap_or_default();

        info!(
            "Viettel STT: Transcription complete, text='{}'",
            transcription
        );

        Ok(transcription.to_string())
    }

    /// Wrap raw PCM data in a WAV container.
    fn wrap_in_wav(&self, pcm_data: &[u8]) -> Vec<u8> {
        let sample_rate = self.config.sample_rate;
        let channels = self.config.channels;
        let bits_per_sample: u16 = 16;
        let byte_rate = sample_rate * u32::from(channels) * u32::from(bits_per_sample) / 8;
        let block_align = channels * bits_per_sample / 8;
        let data_size = pcm_data.len() as u32;
        let file_size = 36 + data_size;

        let mut wav = Vec::with_capacity(44 + pcm_data.len());

        // RIFF header
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&file_size.to_le_bytes());
        wav.extend_from_slice(b"WAVE");

        // fmt chunk
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes()); // chunk size
        wav.extend_from_slice(&1u16.to_le_bytes()); // audio format (PCM)
        wav.extend_from_slice(&channels.to_le_bytes());
        wav.extend_from_slice(&sample_rate.to_le_bytes());
        wav.extend_from_slice(&byte_rate.to_le_bytes());
        wav.extend_from_slice(&block_align.to_le_bytes());
        wav.extend_from_slice(&bits_per_sample.to_le_bytes());

        // data chunk
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_size.to_le_bytes());
        wav.extend_from_slice(pcm_data);

        wav
    }

    /// Flush the audio buffer and get transcription.
    pub async fn flush(&mut self) -> Result<String, STTError> {
        let audio_data = {
            let mut buffer = self.audio_buffer.write().await;
            std::mem::take(&mut *buffer)
        };

        if audio_data.is_empty() {
            return Ok(String::new());
        }

        let transcription = self.process_audio(audio_data).await?;

        // Send result via callback if registered
        if !transcription.is_empty() {
            let callback_guard = self.result_callback.read().await;
            if let Some(callback) = callback_guard.as_ref() {
                let result = STTResult::new(
                    transcription.clone(),
                    true, // is_final
                    true, // is_speech_final
                    0.96, // confidence (Viettel claims 96% accuracy)
                );
                callback(result).await;
            }
        }

        Ok(transcription)
    }

    /// Get the Viettel-specific configuration.
    pub fn get_viettel_config(&self) -> &ViettelSttConfig {
        &self.config
    }

    /// Get the current buffer size.
    pub async fn buffer_size(&self) -> usize {
        self.audio_buffer.read().await.len()
    }
}

impl Default for ViettelStt {
    fn default() -> Self {
        Self {
            config: ViettelSttConfig::default(),
            base_config: None,
            http_client: Client::new(),
            is_ready: AtomicBool::new(false),
            connection_state: Arc::new(RwLock::new(STTConnectionState::Disconnected)),
            audio_buffer: Arc::new(RwLock::new(Vec::with_capacity(MAX_AUDIO_BUFFER_SIZE))),
            result_callback: Arc::new(RwLock::new(None)),
            error_callback: Arc::new(RwLock::new(None)),
        }
    }
}

#[async_trait::async_trait]
impl BaseSTT for ViettelStt {
    fn new(config: STTConfig) -> Result<Self, STTError> {
        let viettel_config = ViettelSttConfig::from_base(&config)?;

        let timeout_secs = viettel_config.request_timeout_secs;
        let http_client = Client::builder()
            .timeout(std::time::Duration::from_secs(timeout_secs))
            .build()
            .map_err(|e| {
                STTError::ConfigurationError(format!("Failed to create HTTP client: {e}"))
            })?;

        info!(
            "Viettel STT: Initialized with sample_rate={}, channels={}",
            viettel_config.sample_rate, viettel_config.channels
        );

        Ok(Self {
            config: viettel_config,
            base_config: Some(config),
            http_client,
            is_ready: AtomicBool::new(false),
            connection_state: Arc::new(RwLock::new(STTConnectionState::Disconnected)),
            audio_buffer: Arc::new(RwLock::new(Vec::with_capacity(MAX_AUDIO_BUFFER_SIZE))),
            result_callback: Arc::new(RwLock::new(None)),
            error_callback: Arc::new(RwLock::new(None)),
        })
    }

    async fn connect(&mut self) -> Result<(), STTError> {
        // Validate credentials
        if self.config.api_key.is_empty() {
            return Err(STTError::AuthenticationFailed(
                "Viettel AI API token is required".to_string(),
            ));
        }

        {
            let mut state = self.connection_state.write().await;
            *state = STTConnectionState::Connecting;
        }

        // REST API doesn't need a persistent connection
        // Just mark as ready
        self.is_ready.store(true, Ordering::SeqCst);

        {
            let mut state = self.connection_state.write().await;
            *state = STTConnectionState::Connected;
        }

        // Clear any previous audio buffer
        {
            let mut buffer = self.audio_buffer.write().await;
            buffer.clear();
        }

        info!("Viettel STT: Ready to receive audio");
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<(), STTError> {
        // Flush any remaining audio
        if self.is_ready() {
            let _ = self.flush().await;
        }

        self.is_ready.store(false, Ordering::SeqCst);

        {
            let mut state = self.connection_state.write().await;
            *state = STTConnectionState::Disconnected;
        }

        // Clear audio buffer
        {
            let mut buffer = self.audio_buffer.write().await;
            buffer.clear();
        }

        info!("Viettel STT: Disconnected");
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.is_ready.load(Ordering::SeqCst)
    }

    async fn send_audio(&mut self, audio_data: Bytes) -> Result<(), STTError> {
        if !self.is_ready() {
            // Auto-connect if not ready
            self.connect().await?;
        }

        if audio_data.is_empty() {
            return Ok(());
        }

        // Buffer the audio
        {
            let mut buffer = self.audio_buffer.write().await;

            // Check if buffer would exceed maximum
            if buffer.len() + audio_data.len() > MAX_AUDIO_BUFFER_SIZE {
                warn!(
                    "Viettel STT: Audio buffer full ({} bytes), flushing before adding more",
                    buffer.len()
                );
                // We need to drop the lock before flushing
                drop(buffer);
                self.flush().await?;
                buffer = self.audio_buffer.write().await;
            }

            buffer.extend_from_slice(&audio_data);
            debug!(
                "Viettel STT: Buffered {} bytes, total buffer size: {} bytes",
                audio_data.len(),
                buffer.len()
            );
        }

        Ok(())
    }

    async fn on_result(&mut self, callback: STTResultCallback) -> Result<(), STTError> {
        let mut guard = self.result_callback.write().await;
        *guard = Some(callback);
        Ok(())
    }

    async fn on_error(&mut self, callback: STTErrorCallback) -> Result<(), STTError> {
        let mut guard = self.error_callback.write().await;
        *guard = Some(callback);
        Ok(())
    }

    fn get_config(&self) -> Option<&STTConfig> {
        self.base_config.as_ref()
    }

    async fn update_config(&mut self, config: STTConfig) -> Result<(), STTError> {
        let viettel_config = ViettelSttConfig::from_base(&config)?;
        self.config = viettel_config;
        self.base_config = Some(config);
        Ok(())
    }

    fn get_provider_info(&self) -> &'static str {
        "Viettel AI Speech-to-Text (Viettel Group) - Vietnamese language recognition with 96% accuracy"
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    
    
    use std::sync::atomic::AtomicUsize;

    fn make_test_config() -> STTConfig {
        STTConfig {
            provider: "viettel-ai".to_string(),
            api_key: "test_token".to_string(),
            language: "vi".to_string(),
            sample_rate: 16000,
            channels: 1,
            ..Default::default()
        }
    }

    struct MockResultCallback {
        result_count: AtomicUsize,
        last_transcript: Arc<tokio::sync::RwLock<String>>,
    }

    impl MockResultCallback {
        fn new() -> Self {
            Self {
                result_count: AtomicUsize::new(0),
                last_transcript: Arc::new(tokio::sync::RwLock::new(String::new())),
            }
        }

        #[allow(dead_code)]
        fn get_result_count(&self) -> usize {
            self.result_count.load(Ordering::SeqCst)
        }
    }

    // W1 keystone: Viettel AI exposes no advanced-feature surface, so `new_standard` is a pure
    // passthrough — even with features set, the standardized path must build the provider and
    // carry the base config (api_key/sample_rate/channels) through into the provider-specific
    // config unchanged. (Advanced features are documented capability gaps left at default.)
    #[test]
    fn test_viettel_new_standard_passes_base_through() {
        use crate::core::stt::standard::{ProviderExtras, SttFeatures, StandardSTTConfig};
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "viettel-ai".into(),
                api_key: "test_token".into(),
                sample_rate: 8000,
                channels: 1,
                ..Default::default()
            },
            features: SttFeatures {
                diarization: Some(true),
                word_timestamps: Some(true),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
        };
        let stt = ViettelStt::new_standard(&std).unwrap();
        let cfg = stt.get_viettel_config();
        assert_eq!(cfg.api_key, "test_token");
        assert_eq!(cfg.sample_rate, 8000);
        assert_eq!(cfg.channels, 1);
    }

    #[test]
    fn test_viettel_new_standard_requires_api_key() {
        use crate::core::stt::standard::StandardSTTConfig;
        let std = StandardSTTConfig::from_base(STTConfig {
            provider: "viettel-ai".into(),
            api_key: String::new(),
            ..Default::default()
        });
        assert!(ViettelStt::new_standard(&std).is_err());
    }

    #[test]
    fn test_new_valid_config() {
        let config = make_test_config();
        let result = ViettelStt::new(config);
        assert!(result.is_ok());

        let stt = result.unwrap();
        assert!(!stt.is_ready());
    }

    #[test]
    fn test_new_empty_api_key() {
        let config = STTConfig {
            provider: "viettel-ai".to_string(),
            api_key: String::new(),
            ..Default::default()
        };

        let result = ViettelStt::new(config);
        assert!(result.is_err());

        match result {
            Err(STTError::AuthenticationFailed(msg)) => {
                assert!(msg.contains("token"));
            }
            _ => panic!("Expected AuthenticationFailed error"),
        }
    }

    #[test]
    fn test_provider_info() {
        let stt = ViettelStt::new(make_test_config()).unwrap();
        let info = stt.get_provider_info();

        assert!(info.contains("Viettel"));
        assert!(info.contains("Vietnamese"));
        assert!(info.contains("96%"));
    }

    #[test]
    fn test_default_state() {
        let stt = ViettelStt::default();
        assert!(!stt.is_ready());
    }

    #[test]
    fn test_get_config() {
        let config = make_test_config();
        let stt = ViettelStt::new(config.clone()).unwrap();

        let retrieved_config = stt.get_config();
        assert!(retrieved_config.is_some());
        assert_eq!(retrieved_config.unwrap().api_key, config.api_key);
    }

    #[tokio::test]
    async fn test_connect_success() {
        let mut stt = ViettelStt::new(make_test_config()).unwrap();
        let result = stt.connect().await;

        assert!(result.is_ok());
        assert!(stt.is_ready());
    }

    #[tokio::test]
    async fn test_connect_empty_api_key() {
        let mut stt = ViettelStt::default();
        let result = stt.connect().await;

        assert!(result.is_err());
        match result {
            Err(STTError::AuthenticationFailed(_)) => {}
            _ => panic!("Expected AuthenticationFailed error"),
        }
    }

    #[tokio::test]
    async fn test_disconnect() {
        let mut stt = ViettelStt::new(make_test_config()).unwrap();
        stt.connect().await.unwrap();

        let result = stt.disconnect().await;
        assert!(result.is_ok());
        assert!(!stt.is_ready());
    }

    #[tokio::test]
    async fn test_send_audio_empty() {
        let mut stt = ViettelStt::new(make_test_config()).unwrap();
        stt.connect().await.unwrap();

        let result = stt.send_audio(Bytes::new()).await;
        assert!(result.is_ok());
        assert_eq!(stt.buffer_size().await, 0);
    }

    #[tokio::test]
    async fn test_send_audio_buffers() {
        let mut stt = ViettelStt::new(make_test_config()).unwrap();
        stt.connect().await.unwrap();

        let audio_data = Bytes::from(vec![0u8; 1000]);
        let result = stt.send_audio(audio_data).await;

        assert!(result.is_ok());
        assert_eq!(stt.buffer_size().await, 1000);
    }

    #[tokio::test]
    async fn test_send_audio_multiple() {
        let mut stt = ViettelStt::new(make_test_config()).unwrap();
        stt.connect().await.unwrap();

        stt.send_audio(Bytes::from(vec![0u8; 500])).await.unwrap();
        stt.send_audio(Bytes::from(vec![0u8; 500])).await.unwrap();
        stt.send_audio(Bytes::from(vec![0u8; 500])).await.unwrap();

        assert_eq!(stt.buffer_size().await, 1500);
    }

    #[tokio::test]
    async fn test_flush_empty_buffer() {
        let mut stt = ViettelStt::new(make_test_config()).unwrap();
        stt.connect().await.unwrap();

        let result = stt.flush().await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_disconnect_clears_buffer() {
        let mut stt = ViettelStt::new(make_test_config()).unwrap();
        stt.connect().await.unwrap();

        stt.send_audio(Bytes::from(vec![0u8; 1000])).await.unwrap();
        assert!(stt.buffer_size().await > 0);

        stt.disconnect().await.unwrap();
        assert_eq!(stt.buffer_size().await, 0);
    }

    #[tokio::test]
    async fn test_on_result_callback() {
        let mut stt = ViettelStt::new(make_test_config()).unwrap();
        let callback_count = Arc::new(AtomicUsize::new(0));
        let callback_count_clone = callback_count.clone();

        let callback: STTResultCallback = Arc::new(move |_result: STTResult| {
            callback_count_clone.fetch_add(1, Ordering::SeqCst);
            Box::pin(async {})
        });

        let result = stt.on_result(callback).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_on_error_callback() {
        let mut stt = ViettelStt::new(make_test_config()).unwrap();
        let callback_count = Arc::new(AtomicUsize::new(0));
        let callback_count_clone = callback_count.clone();

        let callback: STTErrorCallback = Arc::new(move |_error: STTError| {
            callback_count_clone.fetch_add(1, Ordering::SeqCst);
            Box::pin(async {})
        });

        let result = stt.on_error(callback).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_update_config() {
        let mut stt = ViettelStt::new(make_test_config()).unwrap();

        let new_config = STTConfig {
            provider: "viettel-ai".to_string(),
            api_key: "new_token".to_string(),
            sample_rate: 8000,
            channels: 1,
            ..Default::default()
        };

        let result = stt.update_config(new_config).await;
        assert!(result.is_ok());

        // Verify config was updated
        assert_eq!(stt.get_viettel_config().sample_rate, 8000);
    }

    #[tokio::test]
    async fn test_auto_connect_on_send_audio() {
        let mut stt = ViettelStt::new(make_test_config()).unwrap();
        assert!(!stt.is_ready());

        // Should auto-connect
        let result = stt.send_audio(Bytes::from(vec![0u8; 100])).await;
        assert!(result.is_ok());
        assert!(stt.is_ready());
    }

    #[test]
    fn test_get_viettel_config() {
        let stt = ViettelStt::new(make_test_config()).unwrap();
        let config = stt.get_viettel_config();

        assert_eq!(config.api_key, "test_token");
        assert_eq!(config.sample_rate, 16000);
        assert_eq!(config.channels, 1);
    }
}
