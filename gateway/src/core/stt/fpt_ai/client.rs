//! FPT.AI STT client implementation.
//!
//! This module provides an HTTP-based STT client for FPT Corporation's
//! FPT.AI Speech-to-Text service.
//!
//! # Protocol
//!
//! FPT.AI STT uses HTTP file upload:
//! 1. Collect audio data
//! 2. POST audio file to `/hmi/asr/general`
//! 3. Receive JSON with transcription
//!
//! # Audio Format
//!
//! Input is WAV/PCM format:
//! - Sample Rate: 8kHz or 16kHz
//! - Channels: Mono
//! - Encoding: Linear16 (PCM 16-bit)

use super::super::http_resilience::HttpBreaker;
use super::config::{FPT_STT_ENDPOINT, FptSttConfig, FptSttResponse};
use crate::core::stt::base::{
    BaseSTT, STTConfig, STTConnectionState, STTError, STTErrorCallback, STTResult,
    STTResultCallback,
};
use crate::core::stt::wav as stt_wav;
use bytes::Bytes;
use reqwest::Client;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

/// Minimum audio buffer size in bytes before attempting transcription.
const MIN_AUDIO_BUFFER_SIZE: usize = 3200; // ~100ms at 16kHz mono 16-bit

/// Maximum audio buffer size in bytes.
const MAX_AUDIO_BUFFER_SIZE: usize = 9_600_000; // ~5 minutes at 16kHz mono 16-bit

/// FPT.AI STT client.
///
/// This client buffers audio data and sends it via HTTP when flushed.
/// FPT.AI does not support real-time streaming, so audio must be
/// collected and sent in batches.
pub struct FptStt {
    /// Provider configuration.
    config: FptSttConfig,
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
    /// Shared per-provider circuit breaker for the REST transport (uniform with the WS fleet):
    /// consulted before each upstream call, fed by the unified HTTP status classification.
    /// Inert until `set_resilience` injects the process-global handles (W-D2).
    resilience: HttpBreaker,
}

fn fpt_stt_http_client(timeout_secs: u64) -> Result<Client, reqwest::Error> {
    crate::core::net::ssrf_protected_client_builder(crate::core::net::HTTP_URL_SCHEMES)
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .build()
}

fn default_fpt_stt_http_client() -> Option<Client> {
    match fpt_stt_http_client(super::config::DEFAULT_REQUEST_TIMEOUT) {
        Ok(client) => Some(client),
        Err(err) => {
            warn!(
                error = %err,
                "failed to create default FPT STT HTTP client; default instance is inert until rebuilt with a valid config"
            );
            None
        }
    }
}

impl FptStt {
    /// Process buffered audio and get transcription.
    async fn process_audio(&self, audio_data: Vec<u8>) -> Result<String, STTError> {
        if audio_data.len() < MIN_AUDIO_BUFFER_SIZE {
            warn!(
                "FPT STT: Audio buffer too small ({} bytes), minimum is {} bytes",
                audio_data.len(),
                MIN_AUDIO_BUFFER_SIZE
            );
            return Ok(String::new());
        }

        debug!("FPT STT: Processing {} bytes of audio", audio_data.len());

        // Build WAV header for raw PCM data
        let wav_data = self.wrap_in_wav(&audio_data)?;

        // Send to FPT.AI. When `endpoint_override` is set (credential-free mock harness), its
        // scheme://host replaces the production host while the `/hmi/asr/general` path is preserved.
        let url = match &self.config.endpoint_override {
            Some(ov) => format!("{}/hmi/asr/general", ov.trim_end_matches('/')),
            None => FPT_STT_ENDPOINT.to_string(),
        };
        let http_client = self.http_client.as_ref().ok_or_else(|| {
            STTError::ConfigurationError(
                "FPT STT default HTTP client is unavailable; construct with FptStt::new or new_standard".to_string(),
            )
        })?;

        // Consult the shared per-provider breaker before paying the upstream round-trip: an
        // open breaker fails fast with a typed classified refusal (uniform with the WS fleet).
        self.resilience.check()?;

        let response = http_client
            .post(url)
            .header("api_key", &self.config.api_key)
            .header("Content-Type", "audio/wav")
            .body(wav_data)
            .send()
            .await
            .map_err(|e| {
                self.resilience.record_send_error();
                STTError::NetworkError(format!("Failed to send STT request: {e}"))
            })?;

        let status = response.status();
        self.resilience.record_status(status);

        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unable to read error body".to_string());

            error!("FPT STT: API error (status {}): {}", status.as_u16(), body);

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
        let api_response: FptSttResponse = response
            .json()
            .await
            .map_err(|e| STTError::ProviderError(format!("Failed to parse API response: {e}")))?;

        // Check for API-level errors
        if !api_response.is_success() {
            let error_msg = api_response.status_message();

            // Status 1 (no voice) is not really an error
            if api_response.status == 1 {
                debug!("FPT STT: No voice detected in audio");
                return Ok(String::new());
            }

            error!(
                "FPT STT: API error status {}: {}",
                api_response.status, error_msg
            );

            return Err(STTError::ProviderError(error_msg.to_string()));
        }

        // Get transcription
        let transcription = api_response.transcription().unwrap_or_default();

        info!(
            "FPT STT: Transcription complete, request_id={}, text='{}'",
            api_response.id, transcription
        );

        Ok(transcription.to_string())
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
                    0.9,  // confidence (FPT doesn't provide this)
                );
                callback(result).await;
            }
        }

        Ok(transcription)
    }

    /// W1 keystone — construct directly from the standardized config so the standardized entry
    /// point is uniform across providers. FPT.AI exposes no advanced-feature surface (it is a
    /// simple batch decode endpoint), so `from_standard` is a pure `from_base` passthrough and no
    /// [`SttFeatures`](crate::core::stt::standard::SttFeatures) are mapped; only the base transport
    /// knobs (api_key, sample_rate, channels) survive. Mirrors `DeepgramSTT::new_standard`.
    pub fn new_standard(
        std: &crate::core::stt::standard::StandardSTTConfig,
    ) -> Result<Self, STTError> {
        let fpt_config = FptSttConfig::from_standard(std)?;

        let timeout_secs = fpt_config.request_timeout_secs;
        let http_client = fpt_stt_http_client(timeout_secs).map_err(|e| {
            STTError::ConfigurationError(format!("Failed to create HTTP client: {e}"))
        })?;

        Ok(Self {
            config: fpt_config,
            base_config: Some(std.base.clone()),
            http_client: Some(http_client),
            ..Default::default()
        })
    }

    /// Get the FPT-specific configuration.
    pub fn get_fpt_config(&self) -> &FptSttConfig {
        &self.config
    }

    /// The shared circuit breaker this client feeds, if the process-global resilience handles
    /// have been injected (W-D2). Two `FptStt` built from the same
    /// [`crate::core::resilience::ResilienceRegistry`] return the *same* `Arc`.
    pub fn resilience_breaker(&self) -> Option<&Arc<crate::core::resilience::CircuitBreaker>> {
        self.resilience.breaker()
    }

    /// Get the current buffer size.
    pub async fn buffer_size(&self) -> usize {
        self.audio_buffer.read().await.len()
    }
}

impl Default for FptStt {
    fn default() -> Self {
        Self {
            config: FptSttConfig::default(),
            base_config: None,
            http_client: default_fpt_stt_http_client(),
            is_ready: AtomicBool::new(false),
            connection_state: Arc::new(RwLock::new(STTConnectionState::Disconnected)),
            audio_buffer: Arc::new(RwLock::new(Vec::with_capacity(MAX_AUDIO_BUFFER_SIZE))),
            result_callback: Arc::new(RwLock::new(None)),
            error_callback: Arc::new(RwLock::new(None)),
            resilience: HttpBreaker::new("fpt_ai"),
        }
    }
}

#[async_trait::async_trait]
impl BaseSTT for FptStt {
    fn new(config: STTConfig) -> Result<Self, STTError> {
        let fpt_config = FptSttConfig::from_base(&config)?;

        let timeout_secs = fpt_config.request_timeout_secs;
        let http_client = fpt_stt_http_client(timeout_secs).map_err(|e| {
            STTError::ConfigurationError(format!("Failed to create HTTP client: {e}"))
        })?;

        info!(
            "FPT STT: Initialized with sample_rate={}, channels={}",
            fpt_config.sample_rate, fpt_config.channels
        );

        Ok(Self {
            config: fpt_config,
            base_config: Some(config),
            http_client: Some(http_client),
            is_ready: AtomicBool::new(false),
            connection_state: Arc::new(RwLock::new(STTConnectionState::Disconnected)),
            audio_buffer: Arc::new(RwLock::new(Vec::with_capacity(MAX_AUDIO_BUFFER_SIZE))),
            result_callback: Arc::new(RwLock::new(None)),
            error_callback: Arc::new(RwLock::new(None)),
            resilience: HttpBreaker::new("fpt_ai"),
        })
    }

    async fn connect(&mut self) -> Result<(), STTError> {
        // Validate credentials
        if self.config.api_key.is_empty() {
            return Err(STTError::AuthenticationFailed(
                "FPT.AI API key is required".to_string(),
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

        info!("FPT STT: Ready to receive audio");
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

        info!("FPT STT: Disconnected");
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
                    "FPT STT: Audio buffer full ({} bytes), flushing before adding more",
                    buffer.len()
                );
                // We need to drop the lock before flushing
                drop(buffer);
                self.flush().await?;
                buffer = self.audio_buffer.write().await;
            }

            buffer.extend_from_slice(&audio_data);
            debug!(
                "FPT STT: Buffered {} bytes, total buffer size: {} bytes",
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
        let fpt_config = FptSttConfig::from_base(&config)?;
        self.config = fpt_config;
        self.base_config = Some(config);
        Ok(())
    }

    fn get_provider_info(&self) -> &'static str {
        "FPT.AI Speech-to-Text (FPT Corporation) - Vietnamese language recognition"
    }

    /// W-D2: attach the shared per-provider circuit breaker so every FPT.AI session trips
    /// (and observes) the SAME breaker, uniform with the WS fleet. The REST transport consults
    /// it before each upstream call and feeds it the unified HTTP status classification.
    fn set_resilience(&mut self, resilience: crate::core::resilience::ResilienceHandles) {
        self.resilience.set_handles(resilience);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_config() -> STTConfig {
        STTConfig {
            provider: "fpt-ai".to_string(),
            api_key: "test_api_key".to_string(),
            language: "vi".to_string(),
            sample_rate: 16000,
            channels: 1,
            ..Default::default()
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fpt_stt_redirect_policy_rejects_private_hop() {
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

        let stt = FptStt::new(make_test_config()).expect("construct FPT STT");
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
        let mut stt = FptStt::default();
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
}
