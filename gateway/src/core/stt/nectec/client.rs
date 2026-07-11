//! NECTEC (AI for Thai) STT client implementation.
//!
//! This module provides an HTTP-based STT client for Thailand's NECTEC
//! AI for Thai Speech-to-Text service (Partii4 and Partii5).
//!
//! # Protocol
//!
//! NECTEC STT uses HTTP multipart file upload:
//! 1. Collect audio data
//! 2. POST audio file via multipart form
//! 3. Receive JSON with transcription
//!
//! # Audio Format
//!
//! Input is WAV format:
//! - Sample Rate: 16kHz only
//! - Channels: Mono only
//! - Encoding: Linear16 (PCM 16-bit)
//! - Max Duration: 30 seconds
//! - Max File Size: 1 MB

use super::config::{
    API_KEY_HEADER, LIB_HEADER, LIB_VALUE, MAX_AUDIO_SIZE_BYTES, NectecSttConfig, NectecSttModel,
    Partii4Response, Partii5Response,
};
use crate::core::stt::base::{
    BaseSTT, STTConfig, STTConnectionState, STTError, STTErrorCallback, STTResult,
    STTResultCallback,
};
use crate::core::stt::wav as stt_wav;
use bytes::Bytes;
use reqwest::Client;
use reqwest::multipart::{Form, Part};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

/// Minimum audio buffer size in bytes before attempting transcription.
const MIN_AUDIO_BUFFER_SIZE: usize = 3200; // ~100ms at 16kHz mono 16-bit

/// Maximum audio buffer size in bytes (30 seconds at 16kHz mono 16-bit).
const MAX_AUDIO_BUFFER_SIZE: usize = 960_000;

/// NECTEC STT client.
///
/// This client buffers audio data and sends it via HTTP when flushed.
/// NECTEC does not support real-time streaming, so audio must be
/// collected and sent in batches.
pub struct NectecStt {
    /// Provider configuration.
    config: NectecSttConfig,
    /// Base STT configuration.
    base_config: Option<STTConfig>,
    /// HTTP client for REST API calls.
    http_client: Option<Client>,
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

fn nectec_stt_http_client(timeout_secs: u64) -> Result<Client, reqwest::Error> {
    crate::core::net::ssrf_protected_client_builder(crate::core::net::HTTP_URL_SCHEMES)
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .build()
}

fn default_nectec_stt_http_client() -> Option<Client> {
    match nectec_stt_http_client(super::config::DEFAULT_REQUEST_TIMEOUT) {
        Ok(client) => Some(client),
        Err(err) => {
            warn!(
                error = %err,
                "failed to create default NECTEC STT HTTP client; default instance is inert until rebuilt with a valid config"
            );
            None
        }
    }
}

impl NectecStt {
    /// W1 keystone — construct from the standardized config. NECTEC is a simple batch Thai-only
    /// engine (Partii4/Partii5) whose config exposes none of the standardized advanced features, so
    /// this is a uniform standardized entry point that delegates to `from_standard` (a pure
    /// `from_base` passthrough): the base config carries through and every advanced feature is a
    /// capability gap left at default.
    pub fn new_standard(
        std: &crate::core::stt::standard::StandardSTTConfig,
    ) -> Result<Self, STTError> {
        let nectec_config = NectecSttConfig::from_standard(std)?;

        let timeout_secs = nectec_config.request_timeout_secs;
        let http_client = nectec_stt_http_client(timeout_secs).map_err(|e| {
            STTError::ConfigurationError(format!("Failed to create HTTP client: {e}"))
        })?;

        Ok(Self {
            config: nectec_config,
            base_config: Some(std.base.clone()),
            http_client: Some(http_client),
            is_ready: AtomicBool::new(false),
            connection_state: Arc::new(RwLock::new(STTConnectionState::Disconnected)),
            audio_buffer: Arc::new(RwLock::new(Vec::with_capacity(MAX_AUDIO_BUFFER_SIZE))),
            result_callback: Arc::new(RwLock::new(None)),
            error_callback: Arc::new(RwLock::new(None)),
        })
    }

    /// Process buffered audio and get transcription using Partii5.
    async fn process_partii5(&self, wav_data: Vec<u8>) -> Result<String, STTError> {
        debug!("NECTEC STT: Sending {} bytes to Partii5", wav_data.len());

        let part = Part::bytes(wav_data)
            .file_name("audio.wav")
            .mime_str("audio/wav")
            .map_err(|e| STTError::ProviderError(format!("Failed to create form part: {e}")))?;

        let form = Form::new().part("file", part);

        let http_client = self.http_client.as_ref().ok_or_else(|| {
            STTError::ConfigurationError(
                "NECTEC STT default HTTP client is unavailable; construct with NectecStt::new or new_standard".to_string(),
            )
        })?;

        let response = http_client
            .post(crate::core::tts::standard::override_rest_endpoint(
                self.config.endpoint(),
                self.config.endpoint_override.as_deref(),
            ))
            .header(API_KEY_HEADER, &self.config.api_key)
            .header(LIB_HEADER, LIB_VALUE)
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
                "NECTEC STT: API error (status {}): {}",
                status.as_u16(),
                body
            );

            return match status.as_u16() {
                401 => Err(STTError::AuthenticationFailed(format!(
                    "Invalid API key: {body}"
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
        let api_response: Partii5Response = response
            .json()
            .await
            .map_err(|e| STTError::ProviderError(format!("Failed to parse API response: {e}")))?;

        // Check for API-level errors
        if api_response.has_error() {
            let error_msg = api_response.error.as_deref().unwrap_or("Unknown error");
            error!("NECTEC STT: API error: {}", error_msg);
            return Err(STTError::ProviderError(error_msg.to_string()));
        }

        // Get transcription
        let transcription = api_response.text().unwrap_or_default();

        info!(
            "NECTEC STT (Partii5): Transcription complete, text='{}'",
            transcription
        );

        Ok(transcription.to_string())
    }

    /// Process buffered audio and get transcription using Partii4.
    async fn process_partii4(&self, wav_data: Vec<u8>) -> Result<String, STTError> {
        debug!("NECTEC STT: Sending {} bytes to Partii4", wav_data.len());

        let part = Part::bytes(wav_data)
            .file_name("audio.wav")
            .mime_str("audio/wav")
            .map_err(|e| STTError::ProviderError(format!("Failed to create form part: {e}")))?;

        let form = Form::new()
            .part("wavfile", part)
            .text(
                "outputlevel",
                self.config.output_level.as_param().to_string(),
            )
            .text(
                "outputformat",
                self.config.output_format.as_param().to_string(),
            );

        let http_client = self.http_client.as_ref().ok_or_else(|| {
            STTError::ConfigurationError(
                "NECTEC STT default HTTP client is unavailable; construct with NectecStt::new or new_standard".to_string(),
            )
        })?;

        let response = http_client
            .post(crate::core::tts::standard::override_rest_endpoint(
                self.config.endpoint(),
                self.config.endpoint_override.as_deref(),
            ))
            .header(API_KEY_HEADER, &self.config.api_key)
            .header("Cache-Control", "no-cache")
            .header("Connection", "keep-alive")
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
                "NECTEC STT: API error (status {}): {}",
                status.as_u16(),
                body
            );

            return match status.as_u16() {
                401 => Err(STTError::AuthenticationFailed(format!(
                    "Invalid API key: {body}"
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
        let api_response: Partii4Response = response
            .json()
            .await
            .map_err(|e| STTError::ProviderError(format!("Failed to parse API response: {e}")))?;

        // Check for API-level errors
        if api_response.has_error() {
            let error_msg = api_response.error.as_deref().unwrap_or("Unknown error");
            error!("NECTEC STT: API error: {}", error_msg);
            return Err(STTError::ProviderError(error_msg.to_string()));
        }

        // Get transcription
        let transcription = api_response.text().unwrap_or_default();

        info!(
            "NECTEC STT (Partii4): Transcription complete, text='{}'",
            transcription
        );

        Ok(transcription.to_string())
    }

    /// Process buffered audio and get transcription.
    async fn process_audio(&self, audio_data: Vec<u8>) -> Result<String, STTError> {
        if audio_data.len() < MIN_AUDIO_BUFFER_SIZE {
            warn!(
                "NECTEC STT: Audio buffer too small ({} bytes), minimum is {} bytes",
                audio_data.len(),
                MIN_AUDIO_BUFFER_SIZE
            );
            return Ok(String::new());
        }

        if audio_data.len() > MAX_AUDIO_SIZE_BYTES {
            error!(
                "NECTEC STT: Audio too large ({} bytes), maximum is {} bytes",
                audio_data.len(),
                MAX_AUDIO_SIZE_BYTES
            );
            return Err(STTError::ProviderError(format!(
                "Audio file exceeds maximum size of {} bytes",
                MAX_AUDIO_SIZE_BYTES
            )));
        }

        debug!(
            "NECTEC STT: Processing {} bytes of audio with model {}",
            audio_data.len(),
            self.config.model
        );

        // Build WAV header for raw PCM data
        let wav_data = self.wrap_in_wav(&audio_data)?;

        // Route to appropriate model
        match self.config.model {
            NectecSttModel::Partii5 => self.process_partii5(wav_data).await,
            NectecSttModel::Partii4 => self.process_partii4(wav_data).await,
        }
    }

    /// Wrap raw PCM data in a WAV container.
    fn wrap_in_wav(&self, pcm_data: &[u8]) -> Result<Vec<u8>, STTError> {
        stt_wav::encode_pcm16_wav(pcm_data, self.config.sample_rate, self.config.channels)
            .map_err(|e| STTError::AudioProcessingError(format!("Invalid WAV parameters: {e}")))
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
                    0.9,  // confidence (NECTEC doesn't provide this)
                );
                callback(result).await;
            }
        }

        Ok(transcription)
    }

    /// Get the NECTEC-specific configuration.
    pub fn get_nectec_config(&self) -> &NectecSttConfig {
        &self.config
    }

    /// Get the current buffer size.
    pub async fn buffer_size(&self) -> usize {
        self.audio_buffer.read().await.len()
    }
}

impl Default for NectecStt {
    fn default() -> Self {
        Self {
            config: NectecSttConfig::default(),
            base_config: None,
            http_client: default_nectec_stt_http_client(),
            is_ready: AtomicBool::new(false),
            connection_state: Arc::new(RwLock::new(STTConnectionState::Disconnected)),
            audio_buffer: Arc::new(RwLock::new(Vec::with_capacity(MAX_AUDIO_BUFFER_SIZE))),
            result_callback: Arc::new(RwLock::new(None)),
            error_callback: Arc::new(RwLock::new(None)),
        }
    }
}

#[async_trait::async_trait]
impl BaseSTT for NectecStt {
    fn new(config: STTConfig) -> Result<Self, STTError> {
        let nectec_config = NectecSttConfig::from_base(&config)?;

        let timeout_secs = nectec_config.request_timeout_secs;
        let http_client = nectec_stt_http_client(timeout_secs).map_err(|e| {
            STTError::ConfigurationError(format!("Failed to create HTTP client: {e}"))
        })?;

        info!(
            "NECTEC STT: Initialized with model={}, sample_rate={}, channels={}",
            nectec_config.model, nectec_config.sample_rate, nectec_config.channels
        );

        Ok(Self {
            config: nectec_config,
            base_config: Some(config),
            http_client: Some(http_client),
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
                "NECTEC API key is required".to_string(),
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

        info!(
            "NECTEC STT: Ready to receive audio (model: {})",
            self.config.model
        );
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

        info!("NECTEC STT: Disconnected");
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
                    "NECTEC STT: Audio buffer full ({} bytes), flushing before adding more",
                    buffer.len()
                );
                // We need to drop the lock before flushing
                drop(buffer);
                self.flush().await?;
                buffer = self.audio_buffer.write().await;
            }

            buffer.extend_from_slice(&audio_data);
            debug!(
                "NECTEC STT: Buffered {} bytes, total buffer size: {} bytes",
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
        let nectec_config = NectecSttConfig::from_base(&config)?;
        self.config = nectec_config;
        self.base_config = Some(config);
        Ok(())
    }

    fn get_provider_info(&self) -> &'static str {
        "NECTEC AI for Thai (Partii) - Thai language speech recognition"
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default() {
        let stt = NectecStt::default();
        assert!(!stt.is_ready());
    }

    #[test]
    fn test_new_empty_api_key() {
        let config = STTConfig {
            api_key: String::new(),
            ..Default::default()
        };
        let result = NectecStt::new(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_new_valid_config() {
        let config = STTConfig {
            api_key: "test_key".to_string(),
            model: "partii5".to_string(),
            sample_rate: 16000,
            channels: 1,
            ..Default::default()
        };
        let result = NectecStt::new(config);
        assert!(result.is_ok());
        let stt = result.unwrap();
        assert_eq!(stt.config.api_key, "test_key");
        assert_eq!(stt.config.model, NectecSttModel::Partii5);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn nectec_stt_redirect_policy_rejects_private_hop() {
        let _guard = crate::core::net::test_env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        // SAFETY: test-only env mutation, serialized by core::net::test_env_lock.
        unsafe { std::env::remove_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS") };

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind local redirect test server");
        let addr = listener.local_addr().expect("local addr");

        tokio::spawn(async move {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let mut buf = [0u8; 1024];
            let _ = tokio::io::AsyncReadExt::read(&mut stream, &mut buf).await;
            let response = concat!(
                "HTTP/1.1 302 Found\r\n",
                "Location: http://127.0.0.1:9/metadata\r\n",
                "Content-Length: 0\r\n",
                "\r\n"
            );
            let _ = tokio::io::AsyncWriteExt::write_all(&mut stream, response.as_bytes()).await;
        });

        let stt = NectecStt::new(STTConfig {
            api_key: "test_key".to_string(),
            sample_rate: 16000,
            channels: 1,
            ..Default::default()
        })
        .expect("construct NECTEC STT");
        let err = stt
            .http_client
            .as_ref()
            .expect("strict constructor builds an HTTP client")
            .get(format!("http://{addr}/start"))
            .send()
            .await
            .expect_err("private redirect target must be rejected");
        let mut error_chain = err.to_string();
        let mut source = std::error::Error::source(&err);
        while let Some(err) = source {
            error_chain.push_str(": ");
            error_chain.push_str(&err.to_string());
            source = err.source();
        }
        assert!(error_chain.contains("SSRF protection"), "{error_chain}");
    }

    #[tokio::test]
    async fn default_without_http_client_returns_typed_error_without_panic() {
        let mut stt = NectecStt::default();
        stt.http_client = None;

        let err = stt
            .process_audio(vec![0; MIN_AUDIO_BUFFER_SIZE])
            .await
            .expect_err("inert default client must fail with a typed error");

        match err {
            STTError::ConfigurationError(msg) => {
                assert!(msg.contains("default HTTP client"), "{msg}");
            }
            other => panic!("expected ConfigurationError, got {other:?}"),
        }
    }

    #[test]
    fn test_new_with_partii4() {
        let config = STTConfig {
            api_key: "test_key".to_string(),
            model: "partii4".to_string(),
            sample_rate: 16000,
            channels: 1,
            ..Default::default()
        };
        let result = NectecStt::new(config);
        assert!(result.is_ok());
        let stt = result.unwrap();
        assert_eq!(stt.config.model, NectecSttModel::Partii4);
    }

    // W1 keystone: NECTEC exposes no mappable advanced features, so the meaningful assertion is
    // that the base config survives through `new_standard` onto the provider config (api_key,
    // model) even when advanced features are requested (capability gaps, intentionally dropped) —
    // proving the standardized path is wired.
    #[test]
    fn test_nectec_new_standard_carries_base() {
        use crate::core::stt::standard::{ProviderExtras, StandardSTTConfig, SttFeatures};
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "nectec".into(),
                api_key: "test_key".into(),
                model: "partii4".into(),
                sample_rate: 16000,
                channels: 1,
                ..Default::default()
            },
            // Advanced features the provider cannot express; must not break the standardized path.
            features: SttFeatures {
                diarization: Some(true),
                word_timestamps: Some(true),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
            translation: None,
        };
        let stt = NectecStt::new_standard(&std).unwrap();
        assert_eq!(stt.config.api_key, "test_key"); // base api_key survived
        assert_eq!(stt.config.model, NectecSttModel::Partii4); // base model survived
    }

    #[test]
    fn test_provider_info() {
        let stt = NectecStt::default();
        let info = stt.get_provider_info();
        assert!(info.contains("NECTEC"));
        assert!(info.contains("Thai"));
    }

    #[test]
    fn test_wrap_in_wav() {
        let config = STTConfig {
            api_key: "test_key".to_string(),
            sample_rate: 16000,
            channels: 1,
            ..Default::default()
        };
        let stt = NectecStt::new(config).unwrap();

        let pcm_data = vec![0u8; 1000];
        let wav_data = stt.wrap_in_wav(&pcm_data).expect("valid WAV header");

        // Check WAV header
        assert_eq!(&wav_data[0..4], b"RIFF");
        assert_eq!(&wav_data[8..12], b"WAVE");
        assert_eq!(&wav_data[12..16], b"fmt ");
        assert_eq!(&wav_data[36..40], b"data");

        // Check data size
        let data_size =
            u32::from_le_bytes([wav_data[40], wav_data[41], wav_data[42], wav_data[43]]);
        assert_eq!(data_size as usize, pcm_data.len());
    }

    #[tokio::test]
    async fn test_connect_empty_api_key() {
        let mut stt = NectecStt::default();
        let result = stt.connect().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_connect_success() {
        let config = STTConfig {
            api_key: "test_key".to_string(),
            sample_rate: 16000,
            channels: 1,
            ..Default::default()
        };
        let mut stt = NectecStt::new(config).unwrap();
        let result = stt.connect().await;
        assert!(result.is_ok());
        assert!(stt.is_ready());
    }

    #[tokio::test]
    async fn test_disconnect() {
        let config = STTConfig {
            api_key: "test_key".to_string(),
            sample_rate: 16000,
            channels: 1,
            ..Default::default()
        };
        let mut stt = NectecStt::new(config).unwrap();
        stt.connect().await.unwrap();
        assert!(stt.is_ready());

        stt.disconnect().await.unwrap();
        assert!(!stt.is_ready());
    }

    #[tokio::test]
    async fn test_send_audio() {
        let config = STTConfig {
            api_key: "test_key".to_string(),
            sample_rate: 16000,
            channels: 1,
            ..Default::default()
        };
        let mut stt = NectecStt::new(config).unwrap();
        stt.connect().await.unwrap();

        let audio = Bytes::from(vec![0u8; 100]);
        let result = stt.send_audio(audio).await;
        assert!(result.is_ok());

        let buffer_size = stt.buffer_size().await;
        assert_eq!(buffer_size, 100);
    }

    #[tokio::test]
    async fn test_send_audio_auto_connect() {
        let config = STTConfig {
            api_key: "test_key".to_string(),
            sample_rate: 16000,
            channels: 1,
            ..Default::default()
        };
        let mut stt = NectecStt::new(config).unwrap();
        assert!(!stt.is_ready());

        let audio = Bytes::from(vec![0u8; 100]);
        let result = stt.send_audio(audio).await;
        assert!(result.is_ok());
        assert!(stt.is_ready()); // Should have auto-connected
    }

    #[tokio::test]
    async fn test_buffer_accumulation() {
        let config = STTConfig {
            api_key: "test_key".to_string(),
            sample_rate: 16000,
            channels: 1,
            ..Default::default()
        };
        let mut stt = NectecStt::new(config).unwrap();
        stt.connect().await.unwrap();

        // Send multiple chunks
        for _ in 0..5 {
            let audio = Bytes::from(vec![0u8; 100]);
            stt.send_audio(audio).await.unwrap();
        }

        let buffer_size = stt.buffer_size().await;
        assert_eq!(buffer_size, 500);
    }

    #[tokio::test]
    async fn test_on_result_callback() {
        let config = STTConfig {
            api_key: "test_key".to_string(),
            sample_rate: 16000,
            channels: 1,
            ..Default::default()
        };
        let mut stt = NectecStt::new(config).unwrap();

        let callback_called = Arc::new(AtomicBool::new(false));
        let callback_called_clone = callback_called.clone();

        let callback: STTResultCallback = Arc::new(move |_result| {
            callback_called_clone.store(true, Ordering::SeqCst);
            Box::pin(async {})
        });

        stt.on_result(callback).await.unwrap();

        // Verify callback was registered
        let guard = stt.result_callback.read().await;
        assert!(guard.is_some());
    }

    #[tokio::test]
    async fn test_on_error_callback() {
        let config = STTConfig {
            api_key: "test_key".to_string(),
            sample_rate: 16000,
            channels: 1,
            ..Default::default()
        };
        let mut stt = NectecStt::new(config).unwrap();

        let callback: STTErrorCallback = Arc::new(|_error| Box::pin(async {}));

        stt.on_error(callback).await.unwrap();

        // Verify callback was registered
        let guard = stt.error_callback.read().await;
        assert!(guard.is_some());
    }

    #[tokio::test]
    async fn test_update_config() {
        let config = STTConfig {
            api_key: "test_key".to_string(),
            model: "partii5".to_string(),
            ..Default::default()
        };
        let mut stt = NectecStt::new(config).unwrap();
        assert_eq!(stt.config.model, NectecSttModel::Partii5);

        let new_config = STTConfig {
            api_key: "new_key".to_string(),
            model: "partii4".to_string(),
            ..Default::default()
        };
        stt.update_config(new_config).await.unwrap();

        assert_eq!(stt.config.api_key, "new_key");
        assert_eq!(stt.config.model, NectecSttModel::Partii4);
    }

    #[tokio::test]
    async fn test_flush_empty_buffer() {
        let config = STTConfig {
            api_key: "test_key".to_string(),
            sample_rate: 16000,
            channels: 1,
            ..Default::default()
        };
        let mut stt = NectecStt::new(config).unwrap();
        stt.connect().await.unwrap();

        let result = stt.flush().await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_get_nectec_config() {
        let config = STTConfig {
            api_key: "test_key".to_string(),
            model: "partii5".to_string(),
            sample_rate: 16000,
            channels: 1,
            ..Default::default()
        };
        let stt = NectecStt::new(config).unwrap();

        let nectec_config = stt.get_nectec_config();
        assert_eq!(nectec_config.api_key, "test_key");
        assert_eq!(nectec_config.model, NectecSttModel::Partii5);
    }
}
