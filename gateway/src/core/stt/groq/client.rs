//! Groq STT (Whisper) client implementation.
//!
//! This module provides the main `GroqSTT` client that implements the `BaseSTT` trait
//! for Groq's Audio Transcription API (Whisper).
//!
//! # Architecture
//!
//! Groq's Whisper API is a REST API (not streaming WebSocket). This implementation:
//!
//! 1. Buffers incoming audio data in memory
//! 2. Sends the accumulated audio to the API on `disconnect()` or when threshold is reached
//! 3. Parses the response and invokes callbacks
//!
//! # Performance Characteristics
//!
//! Groq provides the fastest Whisper inference available:
//! - `whisper-large-v3-turbo`: 216x real-time, $0.04/hour
//! - `whisper-large-v3`: 189x real-time, $0.111/hour
//!
//! # Rate Limits
//!
//! - Rate limits apply at the organization level
//! - 429 errors include retry-after header
//! - Automatic retry with exponential backoff recommended

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
use super::config::{FlushStrategy, GroqResponseFormat, GroqSTTConfig};
use super::messages::{
    GroqErrorResponse, TranscriptionResponse, TranscriptionResult, VerboseTranscriptionResponse,
    wav,
};

// =============================================================================
// Constants
// =============================================================================

/// Maximum allowed buffer size to prevent unbounded growth (20MB).
/// Below the 25MB Groq limit to leave room for WAV headers.
const MAX_BUFFER_SIZE_BYTES: usize = 20 * 1024 * 1024;

/// Scale factor for converting PCM 16-bit samples to normalized float (-1.0 to 1.0).
const PCM_TO_FLOAT_SCALE: f32 = 1.0 / 32768.0;

/// Default request timeout in seconds.
const DEFAULT_TIMEOUT_SECS: u64 = 120;

/// Maximum retries for transient errors.
const MAX_RETRIES: u32 = 3;

/// Base delay for exponential backoff (milliseconds).
const BASE_RETRY_DELAY_MS: u64 = 500;

/// Default connect timeout in seconds.
const DEFAULT_CONNECT_TIMEOUT_SECS: u64 = 30;

/// Minimum buffer size before silence detection is active (bytes).
/// ~0.5 seconds at 16kHz 16-bit mono = 16KB
const MIN_BUFFER_FOR_SILENCE_DETECTION: usize = 16 * 1024;

/// User-Agent header value for API requests.
const USER_AGENT: &str = concat!("WaaV-Gateway/", env!("CARGO_PKG_VERSION"));

/// Default confidence value when actual confidence is unavailable (f32 version).
/// This is derived from the canonical f64 constant in messages.rs.
/// Using 0.5 (neutral) instead of 0.9 to avoid overconfidence.
pub const DEFAULT_UNKNOWN_CONFIDENCE: f32 = super::messages::DEFAULT_UNKNOWN_CONFIDENCE as f32;

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
// Groq STT Client
// =============================================================================

/// Rate limit information from API response headers.
#[derive(Debug, Clone, Default)]
pub struct RateLimitInfo {
    /// Remaining requests in current window.
    pub remaining_requests: Option<u32>,
    /// Remaining tokens in current window.
    pub remaining_tokens: Option<u32>,
    /// Unix timestamp when rate limit resets.
    pub reset_requests_at: Option<u64>,
    /// Unix timestamp when token limit resets.
    pub reset_tokens_at: Option<u64>,
    /// Retry-After value from 429 response (milliseconds).
    pub retry_after_ms: Option<u64>,
}

impl RateLimitInfo {
    /// Parse rate limit headers from HTTP response.
    pub fn from_headers(headers: &reqwest::header::HeaderMap) -> Self {
        Self {
            remaining_requests: headers
                .get("x-ratelimit-remaining-requests")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse().ok()),
            remaining_tokens: headers
                .get("x-ratelimit-remaining-tokens")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse().ok()),
            reset_requests_at: headers
                .get("x-ratelimit-reset-requests")
                .and_then(|v| v.to_str().ok())
                .and_then(Self::parse_reset_time),
            reset_tokens_at: headers
                .get("x-ratelimit-reset-tokens")
                .and_then(|v| v.to_str().ok())
                .and_then(Self::parse_reset_time),
            retry_after_ms: headers
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(Self::parse_retry_after),
        }
    }

    /// Parse reset time from header value.
    /// Can be either a Unix timestamp or duration string like "1s", "500ms".
    fn parse_reset_time(s: &str) -> Option<u64> {
        // Try parsing as Unix timestamp first
        if let Ok(ts) = s.parse::<u64>() {
            return Some(ts);
        }
        // Try parsing duration string (e.g., "1s", "500ms")
        Self::parse_duration_string(s).map(|ms| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() + (ms / 1000))
                .unwrap_or(0)
        })
    }

    /// Parse Retry-After header value.
    /// Can be seconds (integer) or duration string.
    pub fn parse_retry_after(s: &str) -> Option<u64> {
        // Try parsing as seconds (integer)
        if let Ok(secs) = s.parse::<u64>() {
            return Some(secs * 1000); // Convert to ms
        }
        // Try parsing as duration string
        Self::parse_duration_string(s)
    }

    /// Parse duration strings like "1s", "500ms", "1m".
    pub fn parse_duration_string(s: &str) -> Option<u64> {
        let s = s.trim();
        if s.ends_with("ms") {
            s.trim_end_matches("ms").parse().ok()
        } else if s.ends_with('s') {
            s.trim_end_matches('s')
                .parse::<u64>()
                .ok()
                .map(|v| v * 1000)
        } else if s.ends_with('m') {
            s.trim_end_matches('m')
                .parse::<u64>()
                .ok()
                .map(|v| v * 60 * 1000)
        } else {
            None
        }
    }
}

/// Groq STT (Whisper) client implementing the BaseSTT trait.
///
/// This client uses the Groq Audio Transcription API to convert speech to text.
/// Since Whisper is a batch API (not streaming), audio is buffered and sent
/// when the connection is closed or a threshold is reached.
///
/// # Example
///
/// ```rust,ignore
/// use waav_gateway::core::stt::{BaseSTT, STTConfig, GroqSTT, STTResult};
/// use std::sync::Arc;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let config = STTConfig {
///         api_key: "gsk_...".to_string(),
///         language: "en".to_string(),
///         sample_rate: 16000,
///         ..Default::default()
///     };
///
///     let mut stt = GroqSTT::new(config)?;
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
pub struct GroqSTT {
    /// Provider-specific configuration.
    pub(crate) config: Option<GroqSTTConfig>,

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
    // Rate Limit Tracking
    // ==========================================================================
    /// Last known rate limit information from API response headers.
    pub rate_limit_info: RateLimitInfo,

    /// Last request ID from Groq API (for debugging/support).
    pub last_request_id: Option<String>,

    // ==========================================================================
    // Silence Detection State
    // ==========================================================================
    /// Timestamp when audio was first received (for min duration check).
    first_audio_time: Option<Instant>,

    /// Timestamp when silence was first detected.
    silence_start_time: Option<Instant>,

    /// Whether the last audio chunk was detected as silent.
    last_was_silent: bool,
}

impl GroqSTT {
    /// Create a new Groq STT client with provider-specific configuration.
    ///
    /// # Arguments
    /// * `config` - Groq-specific STT configuration
    ///
    /// # Returns
    /// * `Result<Self, STTError>` - New instance or error
    pub fn with_config(config: GroqSTTConfig) -> Result<Self, STTError> {
        // Validate configuration
        config.validate().map_err(STTError::ConfigurationError)?;

        // Create HTTP client with sensible defaults
        let http_client = Client::builder()
            .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
            .connect_timeout(Duration::from_secs(DEFAULT_CONNECT_TIMEOUT_SECS))
            .pool_max_idle_per_host(4) // Connection pooling
            .pool_idle_timeout(Duration::from_secs(90)) // Close idle connections after 90s
            .user_agent(USER_AGENT)
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
            rate_limit_info: RateLimitInfo::default(),
            last_request_id: None,
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
    pub(crate) fn calculate_rms_energy(audio_data: &[u8]) -> f32 {
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
    pub(crate) fn is_audio_silent(audio_data: &[u8], threshold: f32) -> bool {
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
                    // Set to false so next audio chunk starts fresh (not "resuming from silence")
                    self.last_was_silent = false;
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

    /// Send buffered audio to Groq API and process the response.
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
            "Sending {} bytes of audio to Groq Whisper API (model: {})",
            buffer_size, config.model
        );

        // Check file size limit
        if buffer_size > config.max_file_size_bytes {
            return Err(STTError::AudioProcessingError(format!(
                "Audio buffer ({} bytes) exceeds maximum file size ({} bytes)",
                buffer_size, config.max_file_size_bytes
            )));
        }

        // Create WAV file from buffered PCM data
        // Clone config values we need since we can't borrow config across await
        let sample_rate = config.base.sample_rate;
        let channels = config.base.channels;
        let wav_data = wav::try_create_wav(&self.audio_buffer, sample_rate, channels)
            .map_err(|e| STTError::AudioProcessingError(format!("Failed to create WAV: {e}")))?;

        // Clone config for use in retry loop (needed because send_request takes &mut self)
        let config_clone = config.clone();

        // Try with retries for transient errors
        // Use Option to allow moving ownership on final attempt (avoids unnecessary clone)
        let mut wav_data = Some(wav_data);
        let mut last_error = None;
        for attempt in 0..MAX_RETRIES {
            if attempt > 0 {
                // Use Retry-After header if available, otherwise exponential backoff
                let delay = self
                    .rate_limit_info
                    .retry_after_ms
                    .unwrap_or_else(|| BASE_RETRY_DELAY_MS * 2u64.pow(attempt - 1));

                debug!(
                    "Retry attempt {} after {}ms delay{}",
                    attempt + 1,
                    delay,
                    if self.rate_limit_info.retry_after_ms.is_some() {
                        " (from Retry-After header)"
                    } else {
                        " (exponential backoff)"
                    }
                );
                tokio::time::sleep(Duration::from_millis(delay)).await;

                // Clear retry_after for next iteration
                self.rate_limit_info.retry_after_ms = None;
            }

            // Get wav_data: clone if more retries possible, move ownership on final attempt
            let is_last_attempt = attempt == MAX_RETRIES - 1;
            let wav_data_for_request = if is_last_attempt {
                // Final attempt: move ownership (no clone needed)
                wav_data.take().ok_or_else(|| {
                    STTError::AudioProcessingError("WAV data unexpectedly empty on final retry".into())
                })?
            } else {
                // More attempts possible: clone (preserves original for retry)
                wav_data
                    .as_ref()
                    .ok_or_else(|| {
                        STTError::AudioProcessingError("WAV data unexpectedly empty during retry".into())
                    })?
                    .clone()
            };

            match self.send_request(wav_data_for_request, &config_clone).await {
                Ok(result) => {
                    // Update last_request_id from response body if available
                    if let TranscriptionResult::Simple(ref resp) = result
                        && let Some(ref x_groq) = resp.x_groq
                    {
                        self.last_request_id = Some(x_groq.id.clone());
                    } else if let TranscriptionResult::Verbose(ref resp) = result
                        && let Some(ref x_groq) = resp.x_groq
                    {
                        self.last_request_id = Some(x_groq.id.clone());
                    }

                    // Create STT result and invoke callback
                    let stt_result = STTResult::new(
                        result.text().to_string(),
                        true, // Final result (Whisper doesn't do interim)
                        true, // Speech final
                        result.confidence() as f32,
                    );

                    info!(
                        "Transcription complete: {} characters, confidence: {:.2}{}",
                        stt_result.transcript.len(),
                        stt_result.confidence,
                        self.last_request_id
                            .as_ref()
                            .map(|id| format!(" [request_id: {}]", id))
                            .unwrap_or_default()
                    );

                    // Invoke callback
                    if let Some(callback) = self.result_callback.lock().await.as_ref() {
                        callback(stt_result).await;
                    }

                    // Clear the buffer after successful transcription
                    self.audio_buffer.clear();
                    return Ok(());
                }
                Err(e) => {
                    // Check if error is retryable
                    if Self::is_retryable_error(&e) && attempt < MAX_RETRIES - 1 {
                        warn!("Retryable error on attempt {}: {}", attempt + 1, e);
                        last_error = Some(e);
                        continue;
                    }

                    // Non-retryable error or max retries reached
                    if let Some(callback) = self.error_callback.lock().await.as_ref() {
                        callback(e.clone()).await;
                    }
                    return Err(e);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            STTError::ProviderError("Unknown error during transcription".to_string())
        }))
    }

    /// Check if an error is retryable (transient).
    pub(crate) fn is_retryable_error(error: &STTError) -> bool {
        match error {
            STTError::NetworkError(_) => true,
            STTError::ProviderError(msg) => {
                msg.contains("429")
                    || msg.contains("rate limit")
                    || msg.contains("500")
                    || msg.contains("502")
                    || msg.contains("503")
                    || msg.contains("Service Unavailable")
            }
            _ => false,
        }
    }

    /// Send a single request to the Groq API.
    ///
    /// Takes ownership of wav_data to avoid unnecessary copies.
    /// Updates self.rate_limit_info and self.last_request_id from response headers.
    async fn send_request(
        &mut self,
        wav_data: Vec<u8>,
        config: &GroqSTTConfig,
    ) -> Result<TranscriptionResult, STTError> {
        // Build multipart form - wav_data ownership is transferred here (no copy)
        let file_part = Part::bytes(wav_data)
            .file_name(format!("audio.{}", config.audio_input_format.extension()))
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
        if config.response_format == GroqResponseFormat::VerboseJson
            && !config.timestamp_granularities.is_empty()
        {
            for granularity in &config.timestamp_granularities {
                form = form.text(
                    "timestamp_granularities[]",
                    granularity.as_str().to_string(),
                );
            }
        }

        // Send request to Groq API
        let response = self
            .http_client
            .post(config.api_url())
            .header("Authorization", format!("Bearer {}", config.base.api_key))
            .multipart(form)
            .send()
            .await
            .map_err(|e| STTError::NetworkError(format!("Request failed: {e}")))?;

        // Extract rate limit headers before consuming response
        let rate_limit_info = RateLimitInfo::from_headers(response.headers());
        self.rate_limit_info = rate_limit_info;

        // Extract request ID for debugging
        self.last_request_id = response
            .headers()
            .get("x-request-id")
            .and_then(|v| v.to_str().ok())
            .map(String::from);

        if let Some(ref request_id) = self.last_request_id {
            debug!("Groq request ID: {}", request_id);
        }

        // Log rate limit info
        if let Some(remaining) = self.rate_limit_info.remaining_requests {
            debug!("Rate limit remaining requests: {}", remaining);
        }

        // Check response status
        let status = response.status();
        let response_text = response
            .text()
            .await
            .map_err(|e| STTError::NetworkError(format!("Failed to read response: {e}")))?;

        if !status.is_success() {
            // Try to parse as Groq error
            let error_msg = if let Ok(error_response) =
                serde_json::from_str::<GroqErrorResponse>(&response_text)
            {
                format!(
                    "Groq API error: {} ({})",
                    error_response.error.message, error_response.error.error_type
                )
            } else {
                format!("Groq API error ({}): {}", status, response_text)
            };

            // Classify error type - include request ID for debugging
            let request_id_suffix = self
                .last_request_id
                .as_ref()
                .map(|id| format!(" [request_id: {}]", id))
                .unwrap_or_default();

            let stt_error = match status.as_u16() {
                400 => STTError::ConfigurationError(format!("{}{}", error_msg, request_id_suffix)),
                401 => {
                    STTError::AuthenticationFailed(format!("{}{}", error_msg, request_id_suffix))
                }
                413 => STTError::AudioProcessingError(format!(
                    "File too large: {}{}",
                    error_msg, request_id_suffix
                )),
                429 => {
                    // Include retry-after info in error message
                    let retry_info = self
                        .rate_limit_info
                        .retry_after_ms
                        .map(|ms| format!(" (retry after {}ms)", ms))
                        .unwrap_or_default();
                    STTError::ProviderError(format!(
                        "Rate limit exceeded: {}{}{}",
                        error_msg, retry_info, request_id_suffix
                    ))
                }
                498 => STTError::ProviderError(format!(
                    "Flex tier capacity exceeded: {}{}",
                    error_msg, request_id_suffix
                )),
                500..=599 => STTError::ProviderError(format!(
                    "Server error: {}{}",
                    error_msg, request_id_suffix
                )),
                _ => STTError::ProviderError(format!("{}{}", error_msg, request_id_suffix)),
            };

            return Err(stt_error);
        }

        // Parse response and extract request ID from response body if available
        self.parse_response(&response_text, config)
    }

    /// Parse API response based on configured format.
    fn parse_response(
        &self,
        response_text: &str,
        config: &GroqSTTConfig,
    ) -> Result<TranscriptionResult, STTError> {
        match config.response_format {
            GroqResponseFormat::Json => {
                let response: TranscriptionResponse =
                    serde_json::from_str(response_text).map_err(|e| {
                        STTError::ProviderError(format!("Failed to parse response: {e}"))
                    })?;
                Ok(TranscriptionResult::Simple(response))
            }
            GroqResponseFormat::VerboseJson => {
                let response: VerboseTranscriptionResponse = serde_json::from_str(response_text)
                    .map_err(|e| {
                        STTError::ProviderError(format!("Failed to parse response: {e}"))
                    })?;
                Ok(TranscriptionResult::Verbose(response))
            }
            GroqResponseFormat::Text => {
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
                    if self.audio_buffer.len() >= MIN_BUFFER_FOR_SILENCE_DETECTION {
                        return self.update_silence_state(audio_data);
                    }
                }
                false
            }
        }
    }

    /// Get the current model being used.
    pub fn model(&self) -> Option<&super::config::GroqSTTModel> {
        self.config.as_ref().map(|c| &c.model)
    }

    /// Get estimated cost for buffered audio in USD.
    pub fn estimated_cost(&self) -> f64 {
        let config = match &self.config {
            Some(c) => c,
            None => return 0.0,
        };

        // Calculate audio duration from buffer size
        // 16kHz, 16-bit mono = 32000 bytes/second
        let bytes_per_second = config.base.sample_rate as f64 * 2.0 * config.base.channels as f64;
        let duration_seconds = self.audio_buffer.len() as f64 / bytes_per_second;
        let duration_hours = duration_seconds / 3600.0;

        // Apply minimum billing duration (10 seconds)
        let billed_duration_hours =
            if duration_seconds < super::config::MIN_BILLED_DURATION_SECONDS as f64 {
                super::config::MIN_BILLED_DURATION_SECONDS as f64 / 3600.0
            } else {
                duration_hours
            };

        billed_duration_hours * config.model.cost_per_hour()
    }

    // =========================================================================
    // Buffer Management Methods
    // =========================================================================

    /// Get the current buffer size in bytes.
    ///
    /// Useful for monitoring buffer growth and deciding when to flush.
    #[inline]
    pub fn buffer_len(&self) -> usize {
        self.audio_buffer.len()
    }

    /// Check if the audio buffer is empty.
    #[inline]
    pub fn is_buffer_empty(&self) -> bool {
        self.audio_buffer.is_empty()
    }

    /// Clear the audio buffer without triggering transcription.
    ///
    /// This discards all buffered audio data. Use with caution.
    /// Useful for error recovery or when you want to start fresh.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // Clear buffer after a failed operation
    /// stt.clear_buffer();
    /// // Start fresh
    /// stt.send_audio(new_audio).await?;
    /// ```
    pub fn clear_buffer(&mut self) {
        self.audio_buffer.clear();
        self.first_audio_time = None;
        self.silence_start_time = None;
        self.last_was_silent = false;
        debug!("Audio buffer cleared");
    }

    /// Take ownership of the audio buffer, replacing it with an empty buffer.
    ///
    /// This is useful for error recovery when you want to:
    /// - Save the audio data to disk for later retry
    /// - Send the audio to a different provider
    /// - Debug/inspect the audio data
    ///
    /// The internal buffer is replaced with a new pre-allocated buffer.
    ///
    /// # Returns
    /// The raw PCM audio data that was buffered.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // Save failed audio for later retry
    /// let audio_data = stt.take_buffer();
    /// std::fs::write("failed_audio.pcm", &audio_data)?;
    /// ```
    pub fn take_buffer(&mut self) -> Vec<u8> {
        // Pre-allocate new buffer with same capacity as before
        let capacity = self.audio_buffer.capacity().max(32 * 1024 * 30);
        let mut new_buffer = Vec::with_capacity(capacity);
        std::mem::swap(&mut self.audio_buffer, &mut new_buffer);

        // Reset silence detection state
        self.first_audio_time = None;
        self.silence_start_time = None;
        self.last_was_silent = false;

        debug!("Audio buffer taken ({} bytes)", new_buffer.len());
        new_buffer
    }

    /// Get a reference to the audio buffer for inspection.
    ///
    /// This is useful for debugging or when you need to inspect
    /// the buffered audio without taking ownership.
    #[inline]
    pub fn buffer(&self) -> &[u8] {
        &self.audio_buffer
    }
}

impl Default for GroqSTT {
    fn default() -> Self {
        // Create HTTP client with sensible defaults matching with_config()
        let http_client = Client::builder()
            .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
            .connect_timeout(Duration::from_secs(DEFAULT_CONNECT_TIMEOUT_SECS))
            .pool_max_idle_per_host(4)
            .pool_idle_timeout(Duration::from_secs(90))
            .user_agent(USER_AGENT)
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            config: None,
            http_client,
            audio_buffer: Vec::with_capacity(32 * 1024 * 30),
            connected: AtomicBool::new(false),
            result_callback: Arc::new(Mutex::new(None)),
            error_callback: Arc::new(Mutex::new(None)),
            total_bytes_received: 0,
            rate_limit_info: RateLimitInfo::default(),
            last_request_id: None,
            first_audio_time: None,
            silence_start_time: None,
            last_was_silent: false,
        }
    }
}

#[async_trait::async_trait]
impl BaseSTT for GroqSTT {
    /// Create a new Groq STT client from base configuration.
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
                "API key is required for Groq STT".to_string(),
            ));
        }

        // Create Groq-specific configuration from base
        let groq_config = GroqSTTConfig::from_base(config);

        Self::with_config(groq_config)
    }

    /// Connect to the STT provider (marks client as ready).
    ///
    /// For Groq (REST API), this simply sets the connected state.
    /// No actual network connection is established until audio is sent.
    async fn connect(&mut self) -> Result<(), STTError> {
        if self.config.is_none() {
            return Err(STTError::ConfigurationError(
                "No configuration available".to_string(),
            ));
        }

        self.connected.store(true, Ordering::Release);
        info!("Groq STT ready to receive audio");
        Ok(())
    }

    /// Disconnect and flush any buffered audio.
    ///
    /// This triggers transcription of any accumulated audio data.
    ///
    /// # Error Handling
    ///
    /// If the flush fails, the error is returned to the caller so they can
    /// decide how to handle it (e.g., retry, save audio to disk, etc.).
    /// The connection state is still updated to disconnected.
    async fn disconnect(&mut self) -> Result<(), STTError> {
        if !self.connected.load(Ordering::Acquire) {
            return Ok(()); // Already disconnected
        }

        // Flush any remaining audio - capture the result
        let flush_result = if !self.audio_buffer.is_empty() {
            let buffer_len = self.audio_buffer.len();
            match self.flush_buffer().await {
                Ok(()) => Ok(()),
                Err(e) => {
                    // Log error with buffer size for debugging
                    error!(
                        "Failed to flush {} bytes of audio during disconnect: {}",
                        buffer_len, e
                    );
                    // Audio buffer is NOT cleared here - caller can retrieve it
                    // via audio_buffer field if needed for recovery
                    Err(e)
                }
            }
        } else {
            Ok(())
        };

        // Update connection state regardless of flush result
        self.connected.store(false, Ordering::Release);

        // Only clear callbacks, not the audio buffer (preserve for error recovery)
        *self.result_callback.lock().await = None;
        *self.error_callback.lock().await = None;

        info!(
            "Groq STT disconnected. Total bytes processed: {}",
            self.total_bytes_received
        );

        // Return flush result - propagate error to caller
        flush_result
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
                "Groq STT not connected".to_string(),
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
        let groq_config = GroqSTTConfig::from_base(config);
        groq_config
            .validate()
            .map_err(STTError::ConfigurationError)?;

        self.config = Some(groq_config);
        self.connect().await?;

        Ok(())
    }

    /// Get provider information string.
    fn get_provider_info(&self) -> &'static str {
        "Groq Whisper STT"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_groq_stt_creation() {
        let config = STTConfig {
            provider: "groq".to_string(),
            api_key: "test_key".to_string(),
            language: "en".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
            model: "whisper-large-v3-turbo".to_string(),
        };

        let stt = <GroqSTT as BaseSTT>::new(config).unwrap();
        assert!(!stt.is_ready());
        assert_eq!(stt.get_provider_info(), "Groq Whisper STT");
    }

    #[tokio::test]
    async fn test_groq_stt_empty_api_key() {
        let config = STTConfig {
            provider: "groq".to_string(),
            api_key: String::new(),
            language: "en".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
            model: "whisper-large-v3-turbo".to_string(),
        };

        let result = <GroqSTT as BaseSTT>::new(config);
        assert!(result.is_err());
        if let Err(STTError::AuthenticationFailed(msg)) = result {
            assert!(msg.contains("API key"));
        } else {
            panic!("Expected AuthenticationFailed error");
        }
    }

    #[tokio::test]
    async fn test_groq_stt_connect_disconnect() {
        let config = STTConfig {
            provider: "groq".to_string(),
            api_key: "test_key".to_string(),
            language: "en".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
            model: "whisper-large-v3-turbo".to_string(),
        };

        let mut stt = <GroqSTT as BaseSTT>::new(config).unwrap();
        assert!(!stt.is_ready());

        // Connect
        stt.connect().await.unwrap();
        assert!(stt.is_ready());

        // Disconnect (no audio buffered, so no API call)
        stt.disconnect().await.unwrap();
        assert!(!stt.is_ready());
    }

    #[tokio::test]
    async fn test_groq_stt_buffer_audio() {
        let config = STTConfig {
            provider: "groq".to_string(),
            api_key: "test_key".to_string(),
            language: "en".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
            model: "whisper-large-v3-turbo".to_string(),
        };

        let mut stt = <GroqSTT as BaseSTT>::new(config).unwrap();
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
    async fn test_groq_stt_send_without_connect() {
        let config = STTConfig {
            provider: "groq".to_string(),
            api_key: "test_key".to_string(),
            language: "en".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
            model: "whisper-large-v3-turbo".to_string(),
        };

        let mut stt = <GroqSTT as BaseSTT>::new(config).unwrap();

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
    async fn test_groq_stt_callback_registration() {
        use std::sync::atomic::AtomicU32;

        let config = STTConfig {
            provider: "groq".to_string(),
            api_key: "test_key".to_string(),
            language: "en".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
            model: "whisper-large-v3-turbo".to_string(),
        };

        let mut stt = <GroqSTT as BaseSTT>::new(config).unwrap();

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
    async fn test_groq_stt_with_config() {
        use super::super::config::{GroqResponseFormat, GroqSTTConfig, GroqSTTModel};

        let groq_config = GroqSTTConfig {
            base: STTConfig {
                provider: "groq".to_string(),
                api_key: "test_key".to_string(),
                language: "en".to_string(),
                sample_rate: 16000,
                channels: 1,
                punctuation: true,
                encoding: "linear16".to_string(),
                model: "whisper-large-v3".to_string(),
            },
            model: GroqSTTModel::WhisperLargeV3,
            response_format: GroqResponseFormat::VerboseJson,
            temperature: Some(0.2),
            ..Default::default()
        };

        let stt = GroqSTT::with_config(groq_config).unwrap();
        assert!(!stt.is_ready());

        let stored_config = stt.config.as_ref().unwrap();
        assert_eq!(stored_config.model, GroqSTTModel::WhisperLargeV3);
        assert_eq!(
            stored_config.response_format,
            GroqResponseFormat::VerboseJson
        );
        assert_eq!(stored_config.temperature, Some(0.2));
    }

    #[test]
    fn test_should_flush_on_threshold() {
        use super::super::config::FlushStrategy;

        let mut stt = GroqSTT::default();

        // Create config with threshold strategy
        let config = GroqSTTConfig {
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

        let mut stt = GroqSTT::default();

        let config = GroqSTTConfig {
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

    #[test]
    fn test_rms_energy_calculation() {
        // Silent audio (all zeros)
        let silent_audio = vec![0u8; 1000];
        let rms = GroqSTT::calculate_rms_energy(&silent_audio);
        assert!(rms < 0.001);

        // Loud audio (max amplitude)
        let mut loud_audio = Vec::new();
        for _ in 0..500 {
            loud_audio.extend_from_slice(&i16::MAX.to_le_bytes());
        }
        let rms = GroqSTT::calculate_rms_energy(&loud_audio);
        assert!(rms > 0.9);
    }

    #[test]
    fn test_is_audio_silent() {
        let silent_audio = vec![0u8; 1000];
        assert!(GroqSTT::is_audio_silent(&silent_audio, 0.01));

        let mut loud_audio = Vec::new();
        for _ in 0..500 {
            loud_audio.extend_from_slice(&10000i16.to_le_bytes());
        }
        assert!(!GroqSTT::is_audio_silent(&loud_audio, 0.01));
    }

    #[test]
    fn test_is_retryable_error() {
        assert!(GroqSTT::is_retryable_error(&STTError::NetworkError(
            "Connection reset".to_string()
        )));
        assert!(GroqSTT::is_retryable_error(&STTError::ProviderError(
            "429 rate limit exceeded".to_string()
        )));
        assert!(GroqSTT::is_retryable_error(&STTError::ProviderError(
            "503 Service Unavailable".to_string()
        )));
        assert!(!GroqSTT::is_retryable_error(
            &STTError::AuthenticationFailed("Invalid API key".to_string())
        ));
        assert!(!GroqSTT::is_retryable_error(&STTError::ConfigurationError(
            "Invalid config".to_string()
        )));
    }

    #[test]
    fn test_estimated_cost() {
        let mut stt = GroqSTT::default();

        let config = GroqSTTConfig {
            base: STTConfig {
                api_key: "test_key".to_string(),
                sample_rate: 16000,
                channels: 1,
                ..Default::default()
            },
            ..Default::default()
        };

        stt.config = Some(config);

        // Empty buffer - minimum billing applies
        assert!(stt.estimated_cost() > 0.0);

        // Add 1 minute of audio (16kHz, 16-bit mono = 1,920,000 bytes)
        stt.audio_buffer = vec![0u8; 1_920_000];
        let cost = stt.estimated_cost();

        // 1 minute at $0.04/hour for turbo = ~$0.00067
        assert!(cost > 0.0005 && cost < 0.001);
    }
}
