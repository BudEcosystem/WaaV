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

use super::config::{FPT_STT_ENDPOINT, FptSttConfig, FptSttResponse};
use crate::core::stt::base::{
    BaseSTT, STTConfig, STTConnectionState, STTError, STTErrorCallback, STTResult,
    STTResultCallback,
};
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
        let wav_data = self.wrap_in_wav(&audio_data);

        // Send to FPT.AI
        let response = self
            .http_client
            .post(FPT_STT_ENDPOINT)
            .header("api_key", &self.config.api_key)
            .header("Content-Type", "audio/wav")
            .body(wav_data)
            .send()
            .await
            .map_err(|e| STTError::NetworkError(format!("Failed to send STT request: {e}")))?;

        let status = response.status();

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
        let http_client = Client::builder()
            .timeout(std::time::Duration::from_secs(timeout_secs))
            .build()
            .map_err(|e| {
                STTError::ConfigurationError(format!("Failed to create HTTP client: {e}"))
            })?;

        Ok(Self {
            config: fpt_config,
            base_config: Some(std.base.clone()),
            http_client,
            ..Default::default()
        })
    }

    /// Get the FPT-specific configuration.
    pub fn get_fpt_config(&self) -> &FptSttConfig {
        &self.config
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
impl BaseSTT for FptStt {
    fn new(config: STTConfig) -> Result<Self, STTError> {
        let fpt_config = FptSttConfig::from_base(&config)?;

        let timeout_secs = fpt_config.request_timeout_secs;
        let http_client = Client::builder()
            .timeout(std::time::Duration::from_secs(timeout_secs))
            .build()
            .map_err(|e| {
                STTError::ConfigurationError(format!("Failed to create HTTP client: {e}"))
            })?;

        info!(
            "FPT STT: Initialized with sample_rate={}, channels={}",
            fpt_config.sample_rate, fpt_config.channels
        );

        Ok(Self {
            config: fpt_config,
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
}
