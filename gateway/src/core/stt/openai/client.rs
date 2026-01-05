//! OpenAI STT (Whisper) client implementation.
//!
//! This module provides the main `OpenAISTT` client that implements the `BaseSTT` trait
//! for OpenAI's Audio Transcription API (Whisper).
//!
//! # Architecture
//!
//! Unlike WebSocket-based STT providers (Deepgram, ElevenLabs), OpenAI Whisper is a
//! REST API. This implementation:
//!
//! 1. Buffers incoming audio data in memory
//! 2. Sends the accumulated audio to the API on `disconnect()` or when threshold is reached
//! 3. Parses the response and invokes callbacks
//!
//! # Performance Considerations
//!
//! - Audio buffer uses pre-allocated capacity to minimize reallocations
//! - HTTP client is reused across requests (connection pooling)
//! - WAV header is generated on-the-fly to avoid extra copies

use bytes::Bytes;
use reqwest::Client;
use reqwest::multipart::{Form, Part};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

use super::super::base::{
    BaseSTT, STTConfig, STTError, STTErrorCallback, STTResult, STTResultCallback,
};
use super::config::{FlushStrategy, OpenAISTTConfig, ResponseFormat};
use super::messages::{
    OpenAIErrorResponse, TranscriptionResponse, TranscriptionResult, VerboseTranscriptionResponse,
    wav,
};

// =============================================================================
// Constants
// =============================================================================

/// Maximum allowed buffer size to prevent unbounded growth (20MB)
/// Slightly below the 25MB OpenAI limit to leave room for WAV headers
const MAX_BUFFER_SIZE_BYTES: usize = 20 * 1024 * 1024;

/// Scale factor for converting PCM 16-bit samples to normalized float (-1.0 to 1.0)
const PCM_TO_FLOAT_SCALE: f32 = 1.0 / 32768.0;

// =============================================================================
// Type Aliases
// =============================================================================

/// Type alias for the async result callback.
type AsyncSTTCallback = Box<
    dyn Fn(STTResult) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

/// Type alias for the async error callback.
type AsyncErrorCallback = Box<
    dyn Fn(STTError) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

// =============================================================================
// OpenAI STT Client
// =============================================================================

/// OpenAI STT (Whisper) client implementing the BaseSTT trait.
///
/// This client uses the OpenAI Audio Transcription API to convert speech to text.
/// Since Whisper is a batch API (not streaming), audio is buffered and sent
/// when the connection is closed or a threshold is reached.
///
/// # Example
///
/// ```rust,no_run
/// use waav_gateway::core::stt::{BaseSTT, STTConfig, OpenAISTT, STTResult};
/// use std::sync::Arc;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let config = STTConfig {
///         api_key: "sk-...".to_string(),
///         language: "en".to_string(),
///         sample_rate: 16000,
///         ..Default::default()
///     };
///
///     let mut stt = OpenAISTT::new(config)?;
///     stt.connect().await?;
///
///     // Register callback for results
///     stt.on_result(Arc::new(|result: STTResult| {
///         Box::pin(async move {
///             println!("Transcription: {}", result.transcript);
///         })
///     })).await?;
///
///     // Send audio data (buffered until disconnect)
///     let audio_data = vec![0u8; 1024];
///     stt.send_audio(audio_data.into()).await?;
///
///     // Disconnect triggers transcription
///     stt.disconnect().await?;
///
///     Ok(())
/// }
/// ```
pub struct OpenAISTT {
    /// Provider-specific configuration.
    pub(crate) config: Option<OpenAISTTConfig>,

    /// HTTP client for API requests (reused for connection pooling).
    http_client: Client,

    /// Audio buffer for accumulating PCM data.
    /// Uses Vec for efficient appending with pre-allocated capacity.
    pub(crate) audio_buffer: Vec<u8>,

    /// Whether the client is "connected" (ready to receive audio).
    connected: AtomicBool,

    /// Result callback for transcription results.
    pub(crate) result_callback: Arc<Mutex<Option<AsyncSTTCallback>>>,

    /// Error callback for API errors.
    pub(crate) error_callback: Arc<Mutex<Option<AsyncErrorCallback>>>,

    /// Total bytes received (for statistics).
    pub(crate) total_bytes_received: u64,

    // ==========================================================================
    // Silence Detection State
    // ==========================================================================
    /// Timestamp when audio was first received (for min duration check)
    first_audio_time: Option<Instant>,

    /// Timestamp when silence was first detected
    silence_start_time: Option<Instant>,

    /// Whether the last audio chunk was detected as silent
    last_was_silent: bool,
}

impl OpenAISTT {
    /// Create a new OpenAI STT client with provider-specific configuration.
    ///
    /// # Arguments
    /// * `config` - OpenAI-specific STT configuration
    ///
    /// # Returns
    /// * `Result<Self, STTError>` - New instance or error
    pub fn with_config(config: OpenAISTTConfig) -> Result<Self, STTError> {
        // Validate configuration
        config.validate().map_err(STTError::ConfigurationError)?;

        // Create HTTP client with sensible defaults
        let http_client = Client::builder()
            .timeout(Duration::from_secs(120)) // Whisper can take time for long audio
            .pool_max_idle_per_host(4) // Connection pooling
            .build()
            .map_err(|e| {
                STTError::ConfigurationError(format!("Failed to create HTTP client: {e}"))
            })?;

        // Pre-allocate audio buffer with expected capacity
        // Typical audio: 16kHz, 16-bit mono = 32KB/sec
        // Pre-allocate for ~30 seconds of audio
        let initial_capacity = 32 * 1024 * 30; // ~1MB

        Ok(Self {
            config: Some(config),
            http_client,
            audio_buffer: Vec::with_capacity(initial_capacity),
            connected: AtomicBool::new(false),
            result_callback: Arc::new(Mutex::new(None)),
            error_callback: Arc::new(Mutex::new(None)),
            total_bytes_received: 0,
            first_audio_time: None,
            silence_start_time: None,
            last_was_silent: false,
        })
    }

    /// Publicly accessible flush method.
    ///
    /// Triggers transcription of any buffered audio data without disconnecting.
    /// This is useful for getting intermediate results in long-running sessions.
    ///
    /// # Returns
    /// * `Result<(), STTError>` - Success or error
    ///
    /// # Example
    /// ```rust,no_run
    /// # use waav_gateway::core::stt::{BaseSTT, STTConfig, OpenAISTT};
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut stt = OpenAISTT::new(STTConfig::default())?;
    /// stt.connect().await?;
    /// // ... send audio ...
    /// stt.flush().await?; // Trigger transcription without disconnecting
    /// # Ok(())
    /// # }
    /// ```
    pub async fn flush(&mut self) -> Result<(), STTError> {
        if !self.is_ready() {
            return Err(STTError::ConnectionFailed(
                "Cannot flush: not connected".to_string(),
            ));
        }
        self.flush_buffer().await
    }

    /// Calculate RMS (Root Mean Square) energy of audio samples.
    ///
    /// This measures the average power of the audio signal, which is a good
    /// indicator of whether there's speech present.
    ///
    /// # Arguments
    /// * `audio_data` - PCM 16-bit little-endian audio bytes
    ///
    /// # Returns
    /// * RMS energy value (0.0 to 1.0 for normalized audio)
    fn calculate_rms_energy(audio_data: &[u8]) -> f32 {
        if audio_data.len() < 2 {
            return 0.0;
        }

        let mut sum_squares = 0.0f32;
        let sample_count = audio_data.len() / 2;

        // Process PCM 16-bit little-endian samples
        for chunk in audio_data.chunks_exact(2) {
            let sample = i16::from_le_bytes([chunk[0], chunk[1]]) as f32 * PCM_TO_FLOAT_SCALE;
            sum_squares += sample * sample;
        }

        (sum_squares / sample_count as f32).sqrt()
    }

    /// Check if audio data is silent based on RMS energy threshold.
    ///
    /// # Arguments
    /// * `audio_data` - PCM 16-bit little-endian audio bytes
    /// * `threshold` - RMS energy threshold (0.0 to 1.0)
    ///
    /// # Returns
    /// * true if audio is below the silence threshold
    fn is_audio_silent(audio_data: &[u8], threshold: f32) -> bool {
        let rms = Self::calculate_rms_energy(audio_data);
        rms < threshold
    }

    /// Update silence detection state with new audio data.
    ///
    /// # Arguments
    /// * `audio_data` - New PCM audio bytes to analyze
    ///
    /// # Returns
    /// * true if silence has been detected long enough to trigger flush
    fn update_silence_state(&mut self, audio_data: &[u8]) -> bool {
        let config = match &self.config {
            Some(c) => c,
            None => return false,
        };

        // Track when we first received audio
        if self.first_audio_time.is_none() {
            self.first_audio_time = Some(Instant::now());
        }

        // Check minimum audio duration before enabling silence detection
        let min_duration_ms = config.silence_detection.min_audio_duration_ms as u64;
        if let Some(first_time) = self.first_audio_time
            && first_time.elapsed().as_millis() < min_duration_ms as u128
        {
            return false; // Not enough audio collected yet
        }

        // Check if current audio chunk is silent
        let is_silent = Self::is_audio_silent(audio_data, config.silence_detection.rms_threshold);

        if is_silent {
            if !self.last_was_silent {
                // Silence just started
                self.silence_start_time = Some(Instant::now());
                debug!("Silence detected, starting timer");
            }

            // Check if silence has lasted long enough
            if let Some(silence_start) = self.silence_start_time {
                let silence_duration_ms = config.silence_detection.silence_duration_ms as u64;
                if silence_start.elapsed().as_millis() >= silence_duration_ms as u128 {
                    debug!(
                        "Silence duration threshold reached ({}ms), triggering flush",
                        silence_duration_ms
                    );
                    // Reset silence state for next utterance
                    self.silence_start_time = None;
                    self.first_audio_time = None;
                    self.last_was_silent = true;
                    return true;
                }
            }
        } else {
            // Not silent - reset silence timer
            if self.last_was_silent {
                debug!("Speech resumed, resetting silence timer");
            }
            self.silence_start_time = None;
        }

        self.last_was_silent = is_silent;
        false
    }

    /// Send buffered audio to OpenAI API and process the response.
    ///
    /// This is called internally when:
    /// - `disconnect()` is called
    /// - Buffer reaches the configured threshold (if using OnThreshold strategy)
    async fn flush_buffer(&mut self) -> Result<(), STTError> {
        if self.audio_buffer.is_empty() {
            debug!("No audio data to transcribe");
            return Ok(());
        }

        let config = self.config.as_ref().ok_or_else(|| {
            STTError::ConfigurationError("No configuration available".to_string())
        })?;

        let buffer_size = self.audio_buffer.len();
        info!(
            "Sending {} bytes of audio to OpenAI Whisper API",
            buffer_size
        );

        // Check file size limit
        if buffer_size > config.max_file_size_bytes {
            return Err(STTError::AudioProcessingError(format!(
                "Audio buffer ({} bytes) exceeds maximum file size ({} bytes)",
                buffer_size, config.max_file_size_bytes
            )));
        }

        // Create WAV file from buffered PCM data
        let wav_data = wav::create_wav(
            &self.audio_buffer,
            config.base.sample_rate,
            config.base.channels,
        );

        // Build multipart form
        let file_part = Part::bytes(wav_data)
            .file_name("audio.wav")
            .mime_str(config.audio_input_format.mime_type())
            .map_err(|e| STTError::ConfigurationError(format!("Invalid MIME type: {e}")))?;

        let mut form = Form::new()
            .part("file", file_part)
            .text("model", config.model.as_str().to_string())
            .text(
                "response_format",
                config.response_format.as_str().to_string(),
            );

        // Add optional parameters
        if !config.base.language.is_empty() {
            form = form.text("language", config.base.language.clone());
        }

        if let Some(temp) = config.temperature {
            form = form.text("temperature", temp.to_string());
        }

        if let Some(ref prompt) = config.prompt {
            form = form.text("prompt", prompt.clone());
        }

        // Add timestamp granularities for verbose_json format
        if config.response_format == ResponseFormat::VerboseJson
            && !config.timestamp_granularities.is_empty()
        {
            for granularity in &config.timestamp_granularities {
                form = form.text(
                    "timestamp_granularities[]",
                    granularity.as_str().to_string(),
                );
            }
        }

        // Send request to OpenAI API
        let response = self
            .http_client
            .post(config.api_url())
            .header("Authorization", format!("Bearer {}", config.base.api_key))
            .multipart(form)
            .send()
            .await
            .map_err(|e| STTError::NetworkError(format!("Request failed: {e}")))?;

        // Check response status
        let status = response.status();
        let response_text = response
            .text()
            .await
            .map_err(|e| STTError::NetworkError(format!("Failed to read response: {e}")))?;

        if !status.is_success() {
            // Try to parse as OpenAI error
            let error_msg = if let Ok(error_response) =
                serde_json::from_str::<OpenAIErrorResponse>(&response_text)
            {
                format!(
                    "OpenAI API error: {} ({})",
                    error_response.error.message, error_response.error.error_type
                )
            } else {
                format!("OpenAI API error ({}): {}", status, response_text)
            };

            // Send error through callback if registered
            if let Some(callback) = self.error_callback.lock().await.as_ref() {
                let stt_error = if status.as_u16() == 401 {
                    STTError::AuthenticationFailed(error_msg.clone())
                } else {
                    STTError::ProviderError(error_msg.clone())
                };
                callback(stt_error).await;
            }

            return Err(STTError::ProviderError(error_msg));
        }

        // Parse response based on format
        let transcription_result = self.parse_response(&response_text, config)?;

        // Create STT result and invoke callback
        let stt_result = STTResult::new(
            transcription_result.text().to_string(),
            true, // Final result (Whisper doesn't do interim)
            true, // Speech final
            transcription_result.confidence(),
        );

        info!(
            "Transcription complete: {} characters, confidence: {:.2}",
            stt_result.transcript.len(),
            stt_result.confidence
        );

        // Invoke callback
        if let Some(callback) = self.result_callback.lock().await.as_ref() {
            callback(stt_result).await;
        }

        // Clear the buffer after successful transcription
        self.audio_buffer.clear();

        Ok(())
    }

    /// Parse API response based on configured format.
    fn parse_response(
        &self,
        response_text: &str,
        config: &OpenAISTTConfig,
    ) -> Result<TranscriptionResult, STTError> {
        match config.response_format {
            ResponseFormat::Json => {
                let response: TranscriptionResponse =
                    serde_json::from_str(response_text).map_err(|e| {
                        STTError::ProviderError(format!("Failed to parse response: {e}"))
                    })?;
                Ok(TranscriptionResult::Simple(response))
            }
            ResponseFormat::VerboseJson => {
                let response: VerboseTranscriptionResponse = serde_json::from_str(response_text)
                    .map_err(|e| {
                        STTError::ProviderError(format!("Failed to parse response: {e}"))
                    })?;
                Ok(TranscriptionResult::Verbose(response))
            }
            ResponseFormat::Text | ResponseFormat::Srt | ResponseFormat::Vtt => {
                Ok(TranscriptionResult::PlainText(response_text.to_string()))
            }
        }
    }

    /// Check if buffer should be flushed based on strategy and threshold.
    ///
    /// This is called after each audio chunk is received to determine
    /// if the buffer should be flushed for transcription.
    ///
    /// # Arguments
    /// * `last_audio_chunk` - The most recent audio chunk (used for silence detection)
    ///
    /// # Returns
    /// * true if the buffer should be flushed
    pub(crate) fn should_flush(&mut self, last_audio_chunk: Option<&[u8]>) -> bool {
        let config = match &self.config {
            Some(c) => c,
            None => return false,
        };

        // Always flush if buffer exceeds maximum size (safety limit)
        if self.audio_buffer.len() >= MAX_BUFFER_SIZE_BYTES {
            warn!(
                "Buffer exceeded maximum size ({} bytes), forcing flush",
                MAX_BUFFER_SIZE_BYTES
            );
            return true;
        }

        match config.flush_strategy {
            FlushStrategy::OnDisconnect => false, // Only flush on disconnect
            FlushStrategy::OnThreshold => self.audio_buffer.len() >= config.flush_threshold_bytes,
            FlushStrategy::OnSilence => {
                // Check silence detection with the latest audio chunk
                if let Some(audio_data) = last_audio_chunk {
                    // Only flush on silence if we have enough audio buffered
                    // (at least 0.5 seconds at 16kHz mono = ~16KB)
                    if self.audio_buffer.len() >= 16 * 1024 {
                        return self.update_silence_state(audio_data);
                    }
                }
                false
            }
        }
    }
}

impl Default for OpenAISTT {
    fn default() -> Self {
        Self {
            config: None,
            http_client: Client::new(),
            audio_buffer: Vec::with_capacity(32 * 1024 * 30),
            connected: AtomicBool::new(false),
            result_callback: Arc::new(Mutex::new(None)),
            error_callback: Arc::new(Mutex::new(None)),
            total_bytes_received: 0,
            first_audio_time: None,
            silence_start_time: None,
            last_was_silent: false,
        }
    }
}

#[async_trait::async_trait]
impl BaseSTT for OpenAISTT {
    /// Create a new OpenAI STT client from base configuration.
    ///
    /// # Arguments
    /// * `config` - Base STT configuration
    ///
    /// # Returns
    /// * `Result<Self, STTError>` - New instance or error
    fn new(config: STTConfig) -> Result<Self, STTError> {
        // Validate API key
        if config.api_key.is_empty() {
            return Err(STTError::AuthenticationFailed(
                "API key is required for OpenAI STT".to_string(),
            ));
        }

        // Create OpenAI-specific configuration from base
        let openai_config = OpenAISTTConfig::from_base(config);

        Self::with_config(openai_config)
    }

    /// Connect to the STT provider (marks client as ready).
    ///
    /// For OpenAI (REST API), this simply sets the connected state.
    /// No actual network connection is established until audio is sent.
    async fn connect(&mut self) -> Result<(), STTError> {
        if self.config.is_none() {
            return Err(STTError::ConfigurationError(
                "No configuration available".to_string(),
            ));
        }

        self.connected.store(true, Ordering::Release);
        info!("OpenAI STT ready to receive audio");
        Ok(())
    }

    /// Disconnect and flush any buffered audio.
    ///
    /// This triggers transcription of any accumulated audio data.
    async fn disconnect(&mut self) -> Result<(), STTError> {
        if !self.connected.load(Ordering::Acquire) {
            return Ok(()); // Already disconnected
        }

        // Flush any remaining audio
        if !self.audio_buffer.is_empty()
            && let Err(e) = self.flush_buffer().await
        {
            error!("Failed to flush audio buffer during disconnect: {}", e);
            // Continue with disconnect even if flush fails
        }

        // Clear state
        self.connected.store(false, Ordering::Release);
        self.audio_buffer.clear();
        *self.result_callback.lock().await = None;
        *self.error_callback.lock().await = None;

        info!(
            "OpenAI STT disconnected. Total bytes processed: {}",
            self.total_bytes_received
        );
        Ok(())
    }

    /// Check if the client is ready to receive audio.
    fn is_ready(&self) -> bool {
        self.connected.load(Ordering::Acquire) && self.config.is_some()
    }

    /// Send audio data for transcription.
    ///
    /// Audio is buffered until `disconnect()` is called, a threshold is reached,
    /// or silence is detected (depending on flush strategy).
    ///
    /// # Arguments
    /// * `audio_data` - PCM audio bytes (16-bit signed little-endian)
    ///
    /// # Buffer Limits
    /// The buffer is capped at 20MB to prevent unbounded memory growth.
    /// If this limit is reached, the buffer is automatically flushed.
    async fn send_audio(&mut self, audio_data: Bytes) -> Result<(), STTError> {
        if !self.is_ready() {
            return Err(STTError::ConnectionFailed(
                "OpenAI STT not connected".to_string(),
            ));
        }

        let data_len = audio_data.len();

        // Check if adding this data would exceed buffer limit
        if self.audio_buffer.len() + data_len > MAX_BUFFER_SIZE_BYTES {
            warn!("Buffer would exceed maximum size, flushing before adding new data");
            self.flush_buffer().await?;
        }

        self.total_bytes_received += data_len as u64;

        // Append to buffer
        self.audio_buffer.extend_from_slice(&audio_data);
        debug!(
            "Buffered {} bytes of audio (total: {} bytes)",
            data_len,
            self.audio_buffer.len()
        );

        // Check if we should flush based on strategy (pass audio data for silence detection)
        if self.should_flush(Some(&audio_data)) {
            info!(
                "Flush triggered ({} bytes buffered)",
                self.audio_buffer.len()
            );
            self.flush_buffer().await?;
        }

        Ok(())
    }

    /// Register a callback for transcription results.
    async fn on_result(&mut self, callback: STTResultCallback) -> Result<(), STTError> {
        *self.result_callback.lock().await = Some(Box::new(move |result| {
            let cb = callback.clone();
            Box::pin(async move {
                cb(result).await;
            })
        }));
        Ok(())
    }

    /// Register a callback for errors.
    async fn on_error(&mut self, callback: STTErrorCallback) -> Result<(), STTError> {
        *self.error_callback.lock().await = Some(Box::new(move |error| {
            let cb = callback.clone();
            Box::pin(async move {
                cb(error).await;
            })
        }));
        Ok(())
    }

    /// Get the current configuration.
    fn get_config(&self) -> Option<&STTConfig> {
        self.config.as_ref().map(|c| &c.base)
    }

    /// Update configuration (reconnect required for changes to take effect).
    async fn update_config(&mut self, config: STTConfig) -> Result<(), STTError> {
        // Disconnect if connected
        if self.is_ready() {
            self.disconnect().await?;
        }

        // Create new configuration
        let openai_config = OpenAISTTConfig::from_base(config);
        openai_config
            .validate()
            .map_err(STTError::ConfigurationError)?;

        self.config = Some(openai_config);
        self.connect().await?;

        Ok(())
    }

    /// Get provider information string.
    fn get_provider_info(&self) -> &'static str {
        "OpenAI Whisper STT"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_openai_stt_creation() {
        let config = STTConfig {
            provider: "openai".to_string(),
            api_key: "test_key".to_string(),
            language: "en".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
            model: "whisper-1".to_string(),
        };

        let stt = <OpenAISTT as BaseSTT>::new(config).unwrap();
        assert!(!stt.is_ready());
        assert_eq!(stt.get_provider_info(), "OpenAI Whisper STT");
    }

    #[tokio::test]
    async fn test_openai_stt_empty_api_key() {
        let config = STTConfig {
            provider: "openai".to_string(),
            api_key: String::new(),
            language: "en".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
            model: "whisper-1".to_string(),
        };

        let result = <OpenAISTT as BaseSTT>::new(config);
        assert!(result.is_err());
        if let Err(STTError::AuthenticationFailed(msg)) = result {
            assert!(msg.contains("API key"));
        } else {
            panic!("Expected AuthenticationFailed error");
        }
    }

    #[tokio::test]
    async fn test_openai_stt_connect_disconnect() {
        let config = STTConfig {
            provider: "openai".to_string(),
            api_key: "test_key".to_string(),
            language: "en".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
            model: "whisper-1".to_string(),
        };

        let mut stt = <OpenAISTT as BaseSTT>::new(config).unwrap();
        assert!(!stt.is_ready());

        // Connect
        stt.connect().await.unwrap();
        assert!(stt.is_ready());

        // Disconnect (no audio buffered, so no API call)
        stt.disconnect().await.unwrap();
        assert!(!stt.is_ready());
    }

    #[tokio::test]
    async fn test_openai_stt_buffer_audio() {
        let config = STTConfig {
            provider: "openai".to_string(),
            api_key: "test_key".to_string(),
            language: "en".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
            model: "whisper-1".to_string(),
        };

        let mut stt = <OpenAISTT as BaseSTT>::new(config).unwrap();
        stt.connect().await.unwrap();

        // Send audio data
        let audio_data: Bytes = vec![0u8; 1024].into();
        stt.send_audio(audio_data).await.unwrap();

        // Check buffer was filled
        assert_eq!(stt.audio_buffer.len(), 1024);
        assert_eq!(stt.total_bytes_received, 1024);

        // Send more audio
        let more_audio: Bytes = vec![0u8; 512].into();
        stt.send_audio(more_audio).await.unwrap();

        assert_eq!(stt.audio_buffer.len(), 1536);
        assert_eq!(stt.total_bytes_received, 1536);
    }

    #[tokio::test]
    async fn test_openai_stt_send_without_connect() {
        let config = STTConfig {
            provider: "openai".to_string(),
            api_key: "test_key".to_string(),
            language: "en".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
            model: "whisper-1".to_string(),
        };

        let mut stt = <OpenAISTT as BaseSTT>::new(config).unwrap();

        // Try to send without connecting
        let audio_data: Bytes = vec![0u8; 1024].into();
        let result = stt.send_audio(audio_data).await;

        assert!(result.is_err());
        if let Err(STTError::ConnectionFailed(msg)) = result {
            assert!(msg.contains("not connected"));
        } else {
            panic!("Expected ConnectionFailed error");
        }
    }

    #[tokio::test]
    async fn test_openai_stt_callback_registration() {
        use std::sync::atomic::AtomicU32;

        let config = STTConfig {
            provider: "openai".to_string(),
            api_key: "test_key".to_string(),
            language: "en".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
            model: "whisper-1".to_string(),
        };

        let mut stt = <OpenAISTT as BaseSTT>::new(config).unwrap();

        // Register result callback
        let callback_count = Arc::new(AtomicU32::new(0));
        let callback_count_clone = callback_count.clone();

        let callback = Arc::new(move |_result: STTResult| {
            let count = callback_count_clone.clone();
            Box::pin(async move {
                count.fetch_add(1, Ordering::Relaxed);
            }) as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        });

        stt.on_result(callback).await.unwrap();

        // Verify callback was registered
        assert!(stt.result_callback.lock().await.is_some());
    }

    #[tokio::test]
    async fn test_openai_stt_with_config() {
        use super::super::config::{OpenAISTTConfig, OpenAISTTModel, ResponseFormat};

        let openai_config = OpenAISTTConfig {
            base: STTConfig {
                provider: "openai".to_string(),
                api_key: "test_key".to_string(),
                language: "en".to_string(),
                sample_rate: 16000,
                channels: 1,
                punctuation: true,
                encoding: "linear16".to_string(),
                model: "gpt-4o-transcribe".to_string(),
            },
            model: OpenAISTTModel::Gpt4oTranscribe,
            response_format: ResponseFormat::VerboseJson,
            temperature: Some(0.2),
            ..Default::default()
        };

        let stt = OpenAISTT::with_config(openai_config).unwrap();
        assert!(!stt.is_ready());

        let stored_config = stt.config.as_ref().unwrap();
        assert_eq!(stored_config.model, OpenAISTTModel::Gpt4oTranscribe);
        assert_eq!(stored_config.response_format, ResponseFormat::VerboseJson);
        assert_eq!(stored_config.temperature, Some(0.2));
    }

    #[test]
    fn test_should_flush_on_threshold() {
        use super::super::config::FlushStrategy;

        let mut stt = OpenAISTT::default();

        // Create config with threshold strategy
        let config = OpenAISTTConfig {
            base: STTConfig {
                api_key: "test_key".to_string(),
                ..Default::default()
            },
            flush_strategy: FlushStrategy::OnThreshold,
            flush_threshold_bytes: 1000,
            ..Default::default()
        };

        stt.config = Some(config);

        // Below threshold - should not flush
        stt.audio_buffer = vec![0u8; 500];
        assert!(!stt.should_flush(None));

        // At threshold - should flush
        stt.audio_buffer = vec![0u8; 1000];
        assert!(stt.should_flush(None));

        // Above threshold - should flush
        stt.audio_buffer = vec![0u8; 1500];
        assert!(stt.should_flush(None));
    }

    #[test]
    fn test_should_not_flush_on_disconnect_strategy() {
        use super::super::config::FlushStrategy;

        let mut stt = OpenAISTT::default();

        let config = OpenAISTTConfig {
            base: STTConfig {
                api_key: "test_key".to_string(),
                ..Default::default()
            },
            flush_strategy: FlushStrategy::OnDisconnect,
            flush_threshold_bytes: 1000,
            ..Default::default()
        };

        stt.config = Some(config);

        // Even above threshold, OnDisconnect strategy should not flush
        stt.audio_buffer = vec![0u8; 5000];
        assert!(!stt.should_flush(None));
    }
}
