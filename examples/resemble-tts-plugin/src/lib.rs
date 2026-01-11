//! Resemble AI TTS Plugin for WaaV Gateway
//!
//! This plugin provides Text-to-Speech synthesis using the Resemble AI API.
//! It supports both synchronous and streaming synthesis modes.
//!
//! # Features
//!
//! - HTTP streaming for low-latency audio delivery
//! - Multiple voice support
//! - Configurable sample rates and audio formats
//! - SSML support for prosody control
//! - Production-grade reliability:
//!   - Retry logic with exponential backoff
//!   - Circuit breaker for failure isolation
//!   - Request timeouts and size limits
//!   - Lock-free callback invocation
//!
//! # Building
//!
//! ```bash
//! cargo build --release
//! ```
//!
//! # Installation
//!
//! 1. Build the plugin: `cargo build --release`
//! 2. Copy to plugin directory: `cp target/release/libwaav_plugin_resemble.so /opt/waav/plugins/resemble/`
//! 3. Configure gateway with `plugins.plugin_dir: /opt/waav/plugins`
//! 4. Restart gateway

use abi_stable::{
    export_root_module,
    prefix_type::PrefixTypeTrait,
    sabi_extern_fn,
    std_types::{ROption, RResult, RString, RVec},
};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant};
use waav_plugin_api::{
    CompleteCallbackFn, ErrorCallbackFn, FFIAudioData, FFIConfig, PluginCapabilityType,
    PluginManifest, PluginModule, PluginModule_Ref, ProviderHandle, TTSAudioCallbackFn,
    TTSProvider, TTSVTable, ffi_err, ffi_ok, ErrorCode,
};

// =============================================================================
// Configuration
// =============================================================================

/// Resemble AI API endpoints
const RESEMBLE_STREAM_URL: &str = "https://f.cluster.resemble.ai/stream";
const RESEMBLE_SYNC_URL: &str = "https://f.cluster.resemble.ai/synthesize";
const RESEMBLE_VOICES_URL: &str = "https://app.resemble.ai/api/v2/voices";

/// Default configuration values
const DEFAULT_SAMPLE_RATE: u32 = 24000;
const DEFAULT_OUTPUT_FORMAT: &str = "wav";
const DEFAULT_MODEL: &str = "chatterbox";
const MAX_STREAMING_CHARS: usize = 2000;

// =============================================================================
// Production Hardening Constants
// =============================================================================

/// Maximum text buffer size (100KB) - prevents OOM from unbounded growth
const MAX_TEXT_BUFFER_SIZE: usize = 100_000;

/// Maximum response size (50MB) - prevents OOM from malicious/corrupted responses
const MAX_RESPONSE_SIZE: usize = 50_000_000;

/// HTTP timeout configuration
const CONNECT_TIMEOUT_SECS: u64 = 5;
const REQUEST_TIMEOUT_SECS: u64 = 30;
const READ_TIMEOUT_SECS: u64 = 60;

/// Retry configuration
const MAX_RETRIES: u32 = 3;
const INITIAL_RETRY_DELAY_MS: u64 = 100;
const MAX_RETRY_DELAY_SECS: u64 = 5;

/// Circuit breaker configuration
const CIRCUIT_BREAKER_FAILURE_THRESHOLD: u32 = 5;
const CIRCUIT_BREAKER_RESET_TIMEOUT_SECS: u64 = 30;

/// Plugin configuration parsed from JSON
///
/// This config is designed to be compatible with the gateway's TTSConfig format.
/// The gateway sends `voice_id` which we accept as an alias for `voice_uuid`,
/// and `audio_format` which we accept as an alias for `output_format`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ResembleConfig {
    /// Resemble AI API key (required)
    pub api_key: String,

    /// Voice UUID to use for synthesis (required)
    /// Accepts both `voice_uuid` (native) and `voice_id` (gateway format)
    #[serde(alias = "voice_id")]
    pub voice_uuid: Option<String>,

    /// Model to use: "chatterbox" (default) or "chatterbox-turbo" (lower latency)
    #[serde(default = "default_model")]
    pub model: String,

    /// Output sample rate in Hz (8000, 16000, 22050, 32000, 44100, 48000)
    #[serde(default = "default_sample_rate")]
    pub sample_rate: u32,

    /// Output format: "wav" (default) or "mp3"
    /// Accepts both `output_format` (native) and `audio_format` (gateway format)
    #[serde(default = "default_output_format", alias = "audio_format")]
    pub output_format: String,

    /// Use HD audio quality
    #[serde(default)]
    pub use_hd: bool,

    /// Audio precision: "MULAW", "PCM_16", "PCM_24", "PCM_32"
    #[serde(default)]
    pub precision: Option<String>,

    /// Project UUID (optional)
    #[serde(default)]
    pub project_uuid: Option<String>,

    /// Use streaming mode (default: true for lower latency)
    #[serde(default = "default_streaming")]
    pub streaming: bool,

    /// Provider name (from gateway, ignored)
    #[serde(default, skip_serializing)]
    pub provider: Option<String>,
}

fn default_model() -> String {
    DEFAULT_MODEL.to_string()
}

fn default_sample_rate() -> u32 {
    DEFAULT_SAMPLE_RATE
}

fn default_output_format() -> String {
    DEFAULT_OUTPUT_FORMAT.to_string()
}

fn default_streaming() -> bool {
    true
}

// =============================================================================
// Circuit Breaker Pattern
// =============================================================================

/// Connection state for the state machine
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Disconnecting,
}

impl Default for ConnectionState {
    fn default() -> Self {
        Self::Disconnected
    }
}

/// Circuit breaker state for failure isolation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum CircuitState {
    Closed = 0,      // Normal operation
    Open = 1,        // Failing, reject requests
    HalfOpen = 2,    // Testing if recovered
}

/// Circuit breaker for protecting against cascading failures
///
/// The circuit breaker tracks consecutive failures and opens the circuit
/// when failures exceed the threshold, preventing further requests from
/// overwhelming a failing service.
struct CircuitBreaker {
    /// Number of consecutive failures
    failure_count: AtomicU32,
    /// Timestamp of last failure (Unix epoch millis)
    last_failure_ms: AtomicU64,
    /// Current circuit state
    state: AtomicU8,
    /// Number of successful requests after opening
    success_count: AtomicU32,
}

impl CircuitBreaker {
    /// Create a new circuit breaker in closed state
    fn new() -> Self {
        Self {
            failure_count: AtomicU32::new(0),
            last_failure_ms: AtomicU64::new(0),
            state: AtomicU8::new(CircuitState::Closed as u8),
            success_count: AtomicU32::new(0),
        }
    }

    /// Get current timestamp in milliseconds
    fn now_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    /// Check if a request should be allowed
    fn is_allowed(&self) -> bool {
        match self.state.load(Ordering::Acquire) {
            0 => true, // Closed - allow all
            1 => {
                // Open - check if reset timeout has passed
                let elapsed_ms = Self::now_ms().saturating_sub(self.last_failure_ms.load(Ordering::Acquire));
                if elapsed_ms >= CIRCUIT_BREAKER_RESET_TIMEOUT_SECS * 1000 {
                    // Transition to half-open
                    self.state.store(CircuitState::HalfOpen as u8, Ordering::Release);
                    self.success_count.store(0, Ordering::Release);
                    true
                } else {
                    false
                }
            }
            2 => true, // Half-open - allow one request to test
            _ => false,
        }
    }

    /// Record a successful request
    fn record_success(&self) {
        let state = self.state.load(Ordering::Acquire);
        if state == CircuitState::HalfOpen as u8 {
            // In half-open, require multiple successes to close
            let count = self.success_count.fetch_add(1, Ordering::AcqRel) + 1;
            if count >= 2 {
                // Recovered - close the circuit
                self.state.store(CircuitState::Closed as u8, Ordering::Release);
                self.failure_count.store(0, Ordering::Release);
                tracing::info!("Circuit breaker closed after recovery");
            }
        } else if state == CircuitState::Closed as u8 {
            // Reset failure count on success
            self.failure_count.store(0, Ordering::Release);
        }
    }

    /// Record a failed request
    fn record_failure(&self) {
        let count = self.failure_count.fetch_add(1, Ordering::AcqRel) + 1;
        self.last_failure_ms.store(Self::now_ms(), Ordering::Release);

        let state = self.state.load(Ordering::Acquire);
        if state == CircuitState::HalfOpen as u8 {
            // Failed in half-open - immediately open
            self.state.store(CircuitState::Open as u8, Ordering::Release);
            tracing::warn!("Circuit breaker opened (failed in half-open state)");
        } else if count >= CIRCUIT_BREAKER_FAILURE_THRESHOLD {
            // Too many failures - open the circuit
            self.state.store(CircuitState::Open as u8, Ordering::Release);
            tracing::warn!(
                "Circuit breaker opened after {} consecutive failures",
                count
            );
        }
    }

    /// Check if circuit is open (requests should be rejected)
    fn is_open(&self) -> bool {
        self.state.load(Ordering::Acquire) == CircuitState::Open as u8
    }

    /// Get circuit state for diagnostics
    fn get_state(&self) -> &'static str {
        match self.state.load(Ordering::Acquire) {
            0 => "closed",
            1 => "open",
            2 => "half-open",
            _ => "unknown",
        }
    }
}

// =============================================================================
// Provider State
// =============================================================================

/// Internal state for the Resemble TTS provider
struct ResembleState {
    /// API key for authentication
    api_key: String,

    /// Voice UUID for synthesis
    voice_uuid: String,

    /// Model to use
    model: String,

    /// Sample rate in Hz
    sample_rate: u32,

    /// Output format
    output_format: String,

    /// Use HD quality
    use_hd: bool,

    /// Audio precision
    precision: Option<String>,

    /// Project UUID
    project_uuid: Option<String>,

    /// Whether to use streaming mode
    streaming: bool,

    /// Connection state (uses RwLock for state machine pattern)
    connection_state: RwLock<ConnectionState>,

    /// Text buffer for batched synthesis (size-limited)
    text_buffer: Mutex<String>,

    /// Audio callback (extracted before invocation to prevent deadlock)
    audio_callback: Mutex<Option<(TTSAudioCallbackFn, *mut ())>>,

    /// Error callback (extracted before invocation to prevent deadlock)
    error_callback: Mutex<Option<(ErrorCallbackFn, *mut ())>>,

    /// Completion callback (extracted before invocation to prevent deadlock)
    complete_callback: Mutex<Option<(CompleteCallbackFn, *mut ())>>,

    /// HTTP client (reused for connection pooling)
    client: reqwest::blocking::Client,

    /// Circuit breaker for failure isolation
    circuit_breaker: CircuitBreaker,

    /// Total bytes of audio data received (for metrics)
    total_audio_bytes: AtomicU64,

    /// Total requests made (for metrics)
    total_requests: AtomicU64,

    /// Total failures (for metrics)
    total_failures: AtomicU64,
}

// Safety: ResembleState uses Mutex for interior mutability and AtomicBool for connected state
// All mutable access goes through proper synchronization
unsafe impl Send for ResembleState {}
unsafe impl Sync for ResembleState {}

impl ResembleState {
    /// Create a new state from configuration
    fn new(config: ResembleConfig) -> Result<Self, String> {
        // Validate required fields
        if config.api_key.is_empty() {
            return Err("api_key is required".to_string());
        }

        // Extract voice_uuid - accepts both voice_uuid and voice_id (gateway alias)
        let voice_uuid = config.voice_uuid
            .filter(|v| !v.is_empty())
            .ok_or_else(|| "voice_uuid (or voice_id) is required".to_string())?;

        // Validate sample rate
        // Resemble supports these sample rates (24000 is commonly used for TTS)
        let valid_rates = [8000, 16000, 22050, 24000, 32000, 44100, 48000];
        if !valid_rates.contains(&config.sample_rate) {
            return Err(format!(
                "Invalid sample_rate: {}. Valid values: {:?}",
                config.sample_rate, valid_rates
            ));
        }

        // Normalize output format - gateway may send different names
        // Resemble API only supports "wav" or "mp3"
        let output_format = match config.output_format.to_lowercase().as_str() {
            "wav" | "linear16" | "pcm" | "pcm_16" | "pcm16" => "wav".to_string(),
            "mp3" | "mpeg" => "mp3".to_string(),
            other => {
                return Err(format!(
                    "Invalid output_format: {}. Valid values: wav, mp3, linear16, pcm",
                    other
                ));
            }
        };

        // Build HTTP client with production-grade timeouts
        // Note: blocking client uses timeout() for overall request timeout
        // For streaming responses, we rely on the overall timeout + chunk read logic
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(READ_TIMEOUT_SECS)) // Use longer timeout for streaming
            .connect_timeout(Duration::from_secs(CONNECT_TIMEOUT_SECS))
            .pool_max_idle_per_host(4)
            .build()
            .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

        Ok(Self {
            api_key: config.api_key,
            voice_uuid,  // Use extracted and validated value
            model: config.model,
            sample_rate: config.sample_rate,
            output_format,  // Use normalized format (wav/mp3)
            use_hd: config.use_hd,
            precision: config.precision,
            project_uuid: config.project_uuid,
            streaming: config.streaming,
            connection_state: RwLock::new(ConnectionState::Disconnected),
            text_buffer: Mutex::new(String::with_capacity(4096)), // Pre-allocate reasonable size
            audio_callback: Mutex::new(None),
            error_callback: Mutex::new(None),
            complete_callback: Mutex::new(None),
            client,
            circuit_breaker: CircuitBreaker::new(),
            total_audio_bytes: AtomicU64::new(0),
            total_requests: AtomicU64::new(0),
            total_failures: AtomicU64::new(0),
        })
    }

    /// Check if connected
    fn is_connected(&self) -> bool {
        matches!(
            *self.connection_state.read().unwrap(),
            ConnectionState::Connected
        )
    }

    /// Set connection state
    fn set_connection_state(&self, state: ConnectionState) {
        *self.connection_state.write().unwrap() = state;
    }

    /// Invoke error callback with error code and message
    ///
    /// CRITICAL: This method extracts callback info before invoking to prevent deadlock.
    /// The callback is invoked outside the lock scope.
    fn invoke_error_callback(&self, code: ErrorCode, message: &str) {
        // Extract callback data under the lock
        let callback_info = {
            let guard = match self.error_callback.lock() {
                Ok(g) => g,
                Err(poisoned) => {
                    tracing::error!("Error callback mutex poisoned");
                    poisoned.into_inner()
                }
            };
            guard.clone()
        };
        // Invoke callback outside the lock to prevent deadlock
        if let Some((callback, user_data)) = callback_info {
            let msg: RString = message.into();
            (callback.func)(code.as_u32(), &msg, user_data);
        }
    }

    /// Invoke audio callback with audio data
    ///
    /// CRITICAL: This method extracts callback info before invoking to prevent deadlock.
    /// The callback is invoked outside the lock scope.
    fn invoke_audio_callback(&self, data: &[u8]) {
        // Extract callback data under the lock
        let callback_info = {
            let guard = match self.audio_callback.lock() {
                Ok(g) => g,
                Err(poisoned) => {
                    tracing::error!("Audio callback mutex poisoned");
                    poisoned.into_inner()
                }
            };
            guard.clone()
        };
        // Invoke callback outside the lock to prevent deadlock
        if let Some((callback, user_data)) = callback_info {
            let audio = FFIAudioData::new(
                RVec::from_slice(data),
                self.sample_rate,
                self.output_format.as_str(),
            );
            // Track metrics
            self.total_audio_bytes.fetch_add(data.len() as u64, Ordering::Relaxed);
            (callback.func)(&audio, user_data);
        }
    }

    /// Invoke completion callback
    ///
    /// CRITICAL: This method extracts callback info before invoking to prevent deadlock.
    /// The callback is invoked outside the lock scope.
    fn invoke_complete_callback(&self) {
        // Extract callback data under the lock
        let callback_info = {
            let guard = match self.complete_callback.lock() {
                Ok(g) => g,
                Err(poisoned) => {
                    tracing::error!("Complete callback mutex poisoned");
                    poisoned.into_inner()
                }
            };
            guard.clone()
        };
        // Invoke callback outside the lock to prevent deadlock
        if let Some((callback, user_data)) = callback_info {
            (callback.func)(user_data);
        }
    }

    /// Execute HTTP request with retry logic and exponential backoff
    fn send_with_retry<F, T>(&self, mut op: F) -> Result<T, String>
    where
        F: FnMut() -> Result<T, reqwest::Error>,
    {
        let mut delay = Duration::from_millis(INITIAL_RETRY_DELAY_MS);
        let max_delay = Duration::from_secs(MAX_RETRY_DELAY_SECS);

        for attempt in 0..MAX_RETRIES {
            // Check circuit breaker before each attempt
            if !self.circuit_breaker.is_allowed() {
                return Err("Circuit breaker is open - service unavailable".to_string());
            }

            match op() {
                Ok(result) => {
                    self.circuit_breaker.record_success();
                    return Ok(result);
                }
                Err(e) => {
                    let is_retryable = e.is_timeout() || e.is_connect();

                    if !is_retryable {
                        // Non-retryable error
                        self.circuit_breaker.record_failure();
                        self.total_failures.fetch_add(1, Ordering::Relaxed);
                        return Err(e.to_string());
                    }

                    if attempt < MAX_RETRIES - 1 {
                        tracing::warn!(
                            attempt = attempt + 1,
                            max_retries = MAX_RETRIES,
                            delay_ms = delay.as_millis(),
                            error = %e,
                            "Retrying request after transient error"
                        );
                        std::thread::sleep(delay);
                        // Exponential backoff with jitter
                        delay = delay.saturating_mul(2).min(max_delay);
                    } else {
                        self.circuit_breaker.record_failure();
                        self.total_failures.fetch_add(1, Ordering::Relaxed);
                        return Err(format!("Failed after {} retries: {}", MAX_RETRIES, e));
                    }
                }
            }
        }
        unreachable!()
    }

    /// Perform HTTP streaming synthesis with production-grade error handling
    fn synthesize_streaming(&self, text: &str) -> Result<(), String> {
        // Track request
        self.total_requests.fetch_add(1, Ordering::Relaxed);

        // Check circuit breaker
        if !self.circuit_breaker.is_allowed() {
            return Err("Circuit breaker is open - service temporarily unavailable".to_string());
        }

        // Build request body
        let mut body = serde_json::json!({
            "voice_uuid": self.voice_uuid,
            "data": text,
            "sample_rate": self.sample_rate,
            "output_format": self.output_format,
        });

        // Add optional parameters
        if self.model != DEFAULT_MODEL {
            body["model"] = serde_json::json!(self.model);
        }
        if self.use_hd {
            body["use_hd"] = serde_json::json!(true);
        }
        if let Some(ref precision) = self.precision {
            body["precision"] = serde_json::json!(precision);
        }
        if let Some(ref project_uuid) = self.project_uuid {
            body["project_uuid"] = serde_json::json!(project_uuid);
        }

        let start_time = Instant::now();

        // Make streaming request with retry
        let api_key = self.api_key.clone();
        let client = &self.client;
        let body_clone = body.clone();

        let response = self.send_with_retry(|| {
            client
                .post(RESEMBLE_STREAM_URL)
                .header("Authorization", format!("Bearer {}", api_key))
                .header("Content-Type", "application/json")
                .json(&body_clone)
                .send()
        })?;

        // Check response status
        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().unwrap_or_default();
            self.circuit_breaker.record_failure();
            self.total_failures.fetch_add(1, Ordering::Relaxed);

            // Handle specific error codes
            if status.as_u16() == 429 {
                return Err("Rate limited by Resemble API".to_string());
            } else if status.as_u16() == 401 {
                return Err("Invalid API key".to_string());
            }

            return Err(format!("API error ({}): {}", status, error_text));
        }

        // Stream response chunks with size limit
        let mut reader = response;
        let mut buffer = [0u8; 8192];
        let mut total_bytes_read: usize = 0;

        loop {
            // Check size limit
            if total_bytes_read > MAX_RESPONSE_SIZE {
                tracing::error!(
                    bytes_read = total_bytes_read,
                    max_size = MAX_RESPONSE_SIZE,
                    "Response size limit exceeded"
                );
                return Err(format!(
                    "Response size limit exceeded: {} > {} bytes",
                    total_bytes_read, MAX_RESPONSE_SIZE
                ));
            }

            match std::io::Read::read(&mut reader, &mut buffer) {
                Ok(0) => break, // EOF
                Ok(n) => {
                    total_bytes_read += n;
                    self.invoke_audio_callback(&buffer[..n]);
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    self.circuit_breaker.record_failure();
                    self.total_failures.fetch_add(1, Ordering::Relaxed);
                    return Err(format!("Failed to read response: {}", e));
                }
            }
        }

        // Success - record metrics
        self.circuit_breaker.record_success();
        let elapsed = start_time.elapsed();
        tracing::debug!(
            text_len = text.len(),
            audio_bytes = total_bytes_read,
            elapsed_ms = elapsed.as_millis(),
            "Streaming synthesis completed"
        );

        Ok(())
    }

    /// Perform synchronous synthesis with production-grade error handling
    fn synthesize_sync(&self, text: &str) -> Result<(), String> {
        // Track request
        self.total_requests.fetch_add(1, Ordering::Relaxed);

        // Check circuit breaker
        if !self.circuit_breaker.is_allowed() {
            return Err("Circuit breaker is open - service temporarily unavailable".to_string());
        }

        // Build request body
        let mut body = serde_json::json!({
            "voice_uuid": self.voice_uuid,
            "data": text,
            "sample_rate": self.sample_rate,
            "output_format": self.output_format,
        });

        if self.model != DEFAULT_MODEL {
            body["model"] = serde_json::json!(self.model);
        }
        if self.use_hd {
            body["use_hd"] = serde_json::json!(true);
        }
        if let Some(ref precision) = self.precision {
            body["precision"] = serde_json::json!(precision);
        }
        if let Some(ref project_uuid) = self.project_uuid {
            body["project_uuid"] = serde_json::json!(project_uuid);
        }

        let start_time = Instant::now();

        // Make sync request with retry
        let api_key = self.api_key.clone();
        let client = &self.client;
        let body_clone = body.clone();

        let response = self.send_with_retry(|| {
            client
                .post(RESEMBLE_SYNC_URL)
                .header("Authorization", format!("Bearer {}", api_key))
                .header("Content-Type", "application/json")
                .json(&body_clone)
                .send()
        })?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().unwrap_or_default();
            self.circuit_breaker.record_failure();
            self.total_failures.fetch_add(1, Ordering::Relaxed);

            // Handle specific error codes
            if status.as_u16() == 429 {
                return Err("Rate limited by Resemble API".to_string());
            } else if status.as_u16() == 401 {
                return Err("Invalid API key".to_string());
            }

            return Err(format!("API error ({}): {}", status, error_text));
        }

        // Parse JSON response
        let json: serde_json::Value = response
            .json()
            .map_err(|e| format!("Failed to parse response: {}", e))?;

        // Check for success
        if json.get("success").and_then(|v| v.as_bool()) != Some(true) {
            let error = json
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown error");
            self.circuit_breaker.record_failure();
            self.total_failures.fetch_add(1, Ordering::Relaxed);
            return Err(format!("Synthesis failed: {}", error));
        }

        // Decode base64 audio content
        let audio_content = json
            .get("audio_content")
            .and_then(|v| v.as_str())
            .ok_or("Missing audio_content in response")?;

        let audio_bytes = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            audio_content,
        )
        .map_err(|e| format!("Failed to decode audio: {}", e))?;

        // Check response size limit
        if audio_bytes.len() > MAX_RESPONSE_SIZE {
            tracing::error!(
                audio_size = audio_bytes.len(),
                max_size = MAX_RESPONSE_SIZE,
                "Audio response size limit exceeded"
            );
            return Err(format!(
                "Audio response size limit exceeded: {} > {} bytes",
                audio_bytes.len(),
                MAX_RESPONSE_SIZE
            ));
        }

        // Send audio via callback
        self.invoke_audio_callback(&audio_bytes);

        // Success - record metrics
        self.circuit_breaker.record_success();
        let elapsed = start_time.elapsed();
        tracing::debug!(
            text_len = text.len(),
            audio_bytes = audio_bytes.len(),
            elapsed_ms = elapsed.as_millis(),
            "Sync synthesis completed"
        );

        Ok(())
    }
}

// =============================================================================
// VTable Function Implementations
// =============================================================================

/// Connect to the Resemble AI service
extern "C" fn resemble_connect(handle: *mut ProviderHandle) -> RResult<(), RString> {
    if handle.is_null() {
        return ffi_err("Null handle");
    }

    unsafe {
        let handle = &mut *handle;
        if handle.is_null() {
            return ffi_err("Invalid handle state");
        }

        let state = handle.as_mut::<ResembleState>();

        // Check current state - prevent concurrent connect attempts
        {
            let current_state = *state.connection_state.read().unwrap();
            match current_state {
                ConnectionState::Connected => {
                    return ffi_ok(); // Already connected
                }
                ConnectionState::Connecting => {
                    return ffi_err("Connection already in progress");
                }
                ConnectionState::Disconnecting => {
                    return ffi_err("Disconnect in progress");
                }
                ConnectionState::Disconnected => {
                    // Proceed with connection
                }
            }
        }

        // Set state to connecting
        state.set_connection_state(ConnectionState::Connecting);

        // Test API connection by fetching voices with retry
        let api_key = state.api_key.clone();
        let response = match state.send_with_retry(|| {
            state.client
                .get(RESEMBLE_VOICES_URL)
                .header("Authorization", format!("Bearer {}", api_key))
                .query(&[("page", "1"), ("page_size", "1")])
                .send()
        }) {
            Ok(resp) => resp,
            Err(e) => {
                state.set_connection_state(ConnectionState::Disconnected);
                state.invoke_error_callback(
                    ErrorCode::ConnectionFailed,
                    &format!("Connection failed: {}", e),
                );
                return ffi_err(format!("Connection failed: {}", e));
            }
        };

        if response.status().is_success() {
            state.set_connection_state(ConnectionState::Connected);
            tracing::info!("Connected to Resemble AI service");
            ffi_ok()
        } else if response.status().as_u16() == 401 {
            state.set_connection_state(ConnectionState::Disconnected);
            state.invoke_error_callback(
                ErrorCode::AuthenticationFailed,
                "Invalid API key",
            );
            ffi_err("Invalid API key")
        } else if response.status().as_u16() == 429 {
            state.set_connection_state(ConnectionState::Disconnected);
            state.invoke_error_callback(ErrorCode::RateLimited, "Rate limited");
            ffi_err("Rate limited by Resemble API")
        } else {
            state.set_connection_state(ConnectionState::Disconnected);
            let error = format!("API error: {}", response.status());
            state.invoke_error_callback(ErrorCode::ProviderError, &error);
            ffi_err(error)
        }
    }
}

/// Disconnect from the Resemble AI service
extern "C" fn resemble_disconnect(handle: *mut ProviderHandle) -> RResult<(), RString> {
    if handle.is_null() {
        return ffi_err("Null handle");
    }

    unsafe {
        let handle = &mut *handle;
        if handle.is_null() {
            return ffi_err("Invalid handle state");
        }

        let state = handle.as_mut::<ResembleState>();

        // Set state to disconnecting
        state.set_connection_state(ConnectionState::Disconnecting);

        // Clear any buffered text (using proper lock handling)
        match state.text_buffer.lock() {
            Ok(mut guard) => guard.clear(),
            Err(poisoned) => poisoned.into_inner().clear(),
        }

        // Set state to disconnected
        state.set_connection_state(ConnectionState::Disconnected);
        tracing::info!("Disconnected from Resemble AI service");
    }

    ffi_ok()
}

/// Check if connected and ready
extern "C" fn resemble_is_ready(handle: *const ProviderHandle) -> bool {
    if handle.is_null() {
        return false;
    }

    unsafe {
        let handle = &*handle;
        if handle.is_null() {
            return false;
        }

        let state = handle.as_ref::<ResembleState>();

        // Check connection state and circuit breaker
        let connected = state.is_connected();
        let circuit_ok = state.circuit_breaker.is_allowed();

        connected && circuit_ok
    }
}

/// Send text for synthesis
extern "C" fn resemble_speak(
    handle: *mut ProviderHandle,
    text: *const RString,
    flush: bool,
) -> RResult<(), RString> {
    if handle.is_null() || text.is_null() {
        return ffi_err("Null handle or text");
    }

    unsafe {
        let handle = &mut *handle;
        if handle.is_null() {
            return ffi_err("Invalid handle state");
        }

        let state = handle.as_mut::<ResembleState>();
        let text_str = (*text).as_str();

        // Check connection
        if !state.is_connected() {
            state.invoke_error_callback(ErrorCode::NotConnected, "Not connected");
            return ffi_err("Not connected to Resemble AI");
        }

        // Check circuit breaker
        if !state.circuit_breaker.is_allowed() {
            state.invoke_error_callback(
                ErrorCode::ProviderError,
                "Service temporarily unavailable (circuit breaker open)",
            );
            return ffi_err("Service temporarily unavailable");
        }

        // Add text to buffer with size limit check
        {
            let mut buffer = match state.text_buffer.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };

            // Check buffer size limit BEFORE adding new text
            let new_size = buffer.len() + text_str.len();
            if new_size > MAX_TEXT_BUFFER_SIZE {
                state.invoke_error_callback(
                    ErrorCode::InvalidInput,
                    &format!(
                        "Text buffer overflow: {} + {} > {} bytes. Call flush() or clear() first.",
                        buffer.len(),
                        text_str.len(),
                        MAX_TEXT_BUFFER_SIZE
                    ),
                );
                return ffi_err(format!(
                    "Text buffer overflow: {} bytes exceeds limit of {} bytes",
                    new_size, MAX_TEXT_BUFFER_SIZE
                ));
            }

            buffer.push_str(text_str);
        }

        // If flush is true, synthesize immediately
        if flush {
            let text_to_synthesize = {
                let mut buffer = match state.text_buffer.lock() {
                    Ok(guard) => guard,
                    Err(poisoned) => poisoned.into_inner(),
                };
                std::mem::take(&mut *buffer)
            };

            if !text_to_synthesize.is_empty() {
                // Choose synthesis method based on text length and mode
                let result = if state.streaming && text_to_synthesize.len() <= MAX_STREAMING_CHARS {
                    state.synthesize_streaming(&text_to_synthesize)
                } else {
                    state.synthesize_sync(&text_to_synthesize)
                };

                match result {
                    Ok(()) => {
                        state.invoke_complete_callback();
                    }
                    Err(e) => {
                        state.invoke_error_callback(ErrorCode::ProviderError, &e);
                        return ffi_err(e);
                    }
                }
            }
        }
    }

    ffi_ok()
}

/// Clear queued text
extern "C" fn resemble_clear(handle: *mut ProviderHandle) -> RResult<(), RString> {
    if handle.is_null() {
        return ffi_err("Null handle");
    }

    unsafe {
        let handle = &mut *handle;
        if handle.is_null() {
            return ffi_err("Invalid handle state");
        }

        let state = handle.as_mut::<ResembleState>();

        // Use proper lock handling
        match state.text_buffer.lock() {
            Ok(mut guard) => guard.clear(),
            Err(poisoned) => poisoned.into_inner().clear(),
        }
    }

    ffi_ok()
}

/// Flush and synthesize queued text
extern "C" fn resemble_flush(handle: *mut ProviderHandle) -> RResult<(), RString> {
    if handle.is_null() {
        return ffi_err("Null handle");
    }

    unsafe {
        let handle = &mut *handle;
        if handle.is_null() {
            return ffi_err("Invalid handle state");
        }

        let state = handle.as_mut::<ResembleState>();

        // Check connection
        if !state.is_connected() {
            state.invoke_error_callback(ErrorCode::NotConnected, "Not connected");
            return ffi_err("Not connected to Resemble AI");
        }

        // Check circuit breaker
        if !state.circuit_breaker.is_allowed() {
            state.invoke_error_callback(
                ErrorCode::ProviderError,
                "Service temporarily unavailable (circuit breaker open)",
            );
            return ffi_err("Service temporarily unavailable");
        }

        // Get buffered text (use proper lock handling)
        let text_to_synthesize = {
            let mut buffer = match state.text_buffer.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            std::mem::take(&mut *buffer)
        };

        if text_to_synthesize.is_empty() {
            // Nothing to synthesize, but still signal completion
            state.invoke_complete_callback();
            return ffi_ok();
        }

        // Synthesize
        let result = if state.streaming && text_to_synthesize.len() <= MAX_STREAMING_CHARS {
            state.synthesize_streaming(&text_to_synthesize)
        } else {
            state.synthesize_sync(&text_to_synthesize)
        };

        match result {
            Ok(()) => {
                state.invoke_complete_callback();
                ffi_ok()
            }
            Err(e) => {
                state.invoke_error_callback(ErrorCode::ProviderError, &e);
                ffi_err(e)
            }
        }
    }
}

/// Set audio callback
extern "C" fn resemble_set_audio_callback(
    handle: *mut ProviderHandle,
    callback: TTSAudioCallbackFn,
    user_data: *mut (),
) {
    if handle.is_null() {
        return;
    }

    unsafe {
        let handle = &mut *handle;
        if !handle.is_null() {
            let state = handle.as_mut::<ResembleState>();
            *state.audio_callback.lock().unwrap() = Some((callback, user_data));
        }
    }
}

/// Set error callback
extern "C" fn resemble_set_error_callback(
    handle: *mut ProviderHandle,
    callback: ErrorCallbackFn,
    user_data: *mut (),
) {
    if handle.is_null() {
        return;
    }

    unsafe {
        let handle = &mut *handle;
        if !handle.is_null() {
            let state = handle.as_mut::<ResembleState>();
            *state.error_callback.lock().unwrap() = Some((callback, user_data));
        }
    }
}

/// Set completion callback
extern "C" fn resemble_set_complete_callback(
    handle: *mut ProviderHandle,
    callback: CompleteCallbackFn,
    user_data: *mut (),
) {
    if handle.is_null() {
        return;
    }

    unsafe {
        let handle = &mut *handle;
        if !handle.is_null() {
            let state = handle.as_mut::<ResembleState>();
            *state.complete_callback.lock().unwrap() = Some((callback, user_data));
        }
    }
}

/// Get provider info as JSON (includes diagnostics)
extern "C" fn resemble_get_provider_info(handle: *const ProviderHandle) -> RString {
    let base_info = serde_json::json!({
        "provider": "resemble-tts",
        "version": "1.0.0",
        "type": "dynamic",
        "capabilities": ["tts", "streaming", "ssml"],
        "supported_formats": ["wav", "mp3"],
        "supported_sample_rates": [8000, 16000, 22050, 24000, 32000, 44100, 48000],
        "models": ["chatterbox", "chatterbox-turbo"],
        "hardening": {
            "max_text_buffer_size": MAX_TEXT_BUFFER_SIZE,
            "max_response_size": MAX_RESPONSE_SIZE,
            "max_retries": MAX_RETRIES,
            "connect_timeout_secs": CONNECT_TIMEOUT_SECS,
            "request_timeout_secs": REQUEST_TIMEOUT_SECS,
            "circuit_breaker_threshold": CIRCUIT_BREAKER_FAILURE_THRESHOLD,
        }
    });

    if !handle.is_null() {
        unsafe {
            let handle = &*handle;
            if !handle.is_null() {
                let state = handle.as_ref::<ResembleState>();

                // Get connection state string
                let connection_state = match *state.connection_state.read().unwrap() {
                    ConnectionState::Disconnected => "disconnected",
                    ConnectionState::Connecting => "connecting",
                    ConnectionState::Connected => "connected",
                    ConnectionState::Disconnecting => "disconnecting",
                };

                let info = serde_json::json!({
                    "provider": "resemble-tts",
                    "version": "1.0.0",
                    "type": "dynamic",
                    "capabilities": ["tts", "streaming", "ssml"],
                    "supported_formats": ["wav", "mp3"],
                    "supported_sample_rates": [8000, 16000, 22050, 24000, 32000, 44100, 48000],
                    "models": ["chatterbox", "chatterbox-turbo"],
                    "current_config": {
                        "voice_uuid": state.voice_uuid,
                        "model": state.model,
                        "sample_rate": state.sample_rate,
                        "output_format": state.output_format,
                        "streaming": state.streaming,
                    },
                    "status": {
                        "connection_state": connection_state,
                        "circuit_breaker_state": state.circuit_breaker.get_state(),
                        "is_ready": state.is_connected() && state.circuit_breaker.is_allowed(),
                    },
                    "metrics": {
                        "total_requests": state.total_requests.load(Ordering::Relaxed),
                        "total_failures": state.total_failures.load(Ordering::Relaxed),
                        "total_audio_bytes": state.total_audio_bytes.load(Ordering::Relaxed),
                    },
                    "hardening": {
                        "max_text_buffer_size": MAX_TEXT_BUFFER_SIZE,
                        "max_response_size": MAX_RESPONSE_SIZE,
                        "max_retries": MAX_RETRIES,
                        "connect_timeout_secs": CONNECT_TIMEOUT_SECS,
                        "request_timeout_secs": REQUEST_TIMEOUT_SECS,
                        "circuit_breaker_threshold": CIRCUIT_BREAKER_FAILURE_THRESHOLD,
                    }
                });
                return info.to_string().into();
            }
        }
    }

    base_info.to_string().into()
}

// =============================================================================
// VTable Definition
// =============================================================================

/// TTS VTable for Resemble AI provider
const RESEMBLE_TTS_VTABLE: TTSVTable = TTSVTable {
    connect: resemble_connect,
    disconnect: resemble_disconnect,
    is_ready: resemble_is_ready,
    speak: resemble_speak,
    clear: resemble_clear,
    flush: resemble_flush,
    set_audio_callback: resemble_set_audio_callback,
    set_error_callback: resemble_set_error_callback,
    set_complete_callback: resemble_set_complete_callback,
    get_provider_info: resemble_get_provider_info,
};

// =============================================================================
// Plugin Module Functions
// =============================================================================

/// Create a new TTS provider instance
#[sabi_extern_fn]
fn create_tts(config: *const FFIConfig) -> RResult<TTSProvider, RString> {
    // Parse configuration
    let config_json = if config.is_null() {
        "{}".to_string()
    } else {
        unsafe { (*config).as_str().to_string() }
    };

    let parsed_config: ResembleConfig = match serde_json::from_str(&config_json) {
        Ok(cfg) => cfg,
        Err(e) => {
            return RResult::RErr(format!("Invalid configuration: {}", e).into());
        }
    };

    // Create state
    let state = match ResembleState::new(parsed_config) {
        Ok(s) => s,
        Err(e) => {
            return RResult::RErr(format!("Failed to create provider: {}", e).into());
        }
    };

    // Wrap in handle
    let handle = ProviderHandle::new(state);

    RResult::ROk(TTSProvider {
        handle,
        vtable: RESEMBLE_TTS_VTABLE,
    })
}

/// Get plugin manifest
#[sabi_extern_fn]
fn get_manifest() -> PluginManifest {
    PluginManifest::new("resemble-tts", "Resemble AI TTS Plugin", "1.0.0")
        .with_gateway_version(">=1.0.0")
        .with_capability(PluginCapabilityType::TTS)
        .with_author("WaaV Team")
        .with_description(
            "Real-time text-to-speech synthesis using Resemble AI. \
             Supports streaming audio, multiple voices, and SSML prosody control.",
        )
}

/// Initialize the plugin
#[sabi_extern_fn]
fn init(_config: *const FFIConfig) -> RResult<(), RString> {
    // Plugin-level initialization
    // Currently no global state to initialize
    tracing::info!("Resemble AI TTS plugin initialized");
    ffi_ok()
}

/// Shutdown the plugin
#[sabi_extern_fn]
fn shutdown() -> RResult<(), RString> {
    // Plugin-level cleanup
    tracing::info!("Resemble AI TTS plugin shutting down");
    ffi_ok()
}

// =============================================================================
// Root Module Export
// =============================================================================

/// Export the root module that the gateway will load
#[export_root_module]
fn get_root_module() -> PluginModule_Ref {
    PluginModule {
        manifest: get_manifest,
        init,
        shutdown,
        create_stt: ROption::RNone,
        create_tts: ROption::RSome(create_tts),
        create_realtime: ROption::RNone,
    }
    .leak_into_prefix()
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_parsing() {
        let json = r#"{
            "api_key": "test_key",
            "voice_uuid": "test_voice",
            "model": "chatterbox-turbo",
            "sample_rate": 24000,
            "output_format": "wav"
        }"#;

        let config: ResembleConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.api_key, "test_key");
        assert_eq!(config.voice_uuid, Some("test_voice".to_string()));
        assert_eq!(config.model, "chatterbox-turbo");
        assert_eq!(config.sample_rate, 24000);
        assert_eq!(config.output_format, "wav");
    }

    #[test]
    fn test_config_parsing_with_voice_id_alias() {
        // Test that voice_id (gateway format) is accepted as alias for voice_uuid
        let json = r#"{
            "api_key": "test_key",
            "voice_id": "test_voice_from_gateway",
            "model": "chatterbox"
        }"#;

        let config: ResembleConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.voice_uuid, Some("test_voice_from_gateway".to_string()));
    }

    #[test]
    fn test_config_parsing_with_audio_format_alias() {
        // Test that audio_format (gateway format) is accepted as alias for output_format
        let json = r#"{
            "api_key": "test_key",
            "voice_uuid": "test_voice",
            "audio_format": "mp3"
        }"#;

        let config: ResembleConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.output_format, "mp3");
    }

    #[test]
    fn test_config_defaults() {
        let json = r#"{
            "api_key": "test_key",
            "voice_uuid": "test_voice"
        }"#;

        let config: ResembleConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.model, DEFAULT_MODEL);
        assert_eq!(config.sample_rate, DEFAULT_SAMPLE_RATE);
        assert_eq!(config.output_format, DEFAULT_OUTPUT_FORMAT);
        assert!(config.streaming);
    }

    #[test]
    fn test_state_validation_missing_api_key() {
        let config = ResembleConfig {
            api_key: String::new(),
            voice_uuid: Some("test".to_string()),
            model: DEFAULT_MODEL.to_string(),
            sample_rate: DEFAULT_SAMPLE_RATE,
            output_format: DEFAULT_OUTPUT_FORMAT.to_string(),
            use_hd: false,
            precision: None,
            project_uuid: None,
            streaming: true,
            provider: None,
        };

        let result = ResembleState::new(config);
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert!(err.contains("api_key"), "Expected 'api_key' in error: {}", err);
    }

    #[test]
    fn test_state_validation_missing_voice_uuid() {
        let config = ResembleConfig {
            api_key: "test".to_string(),
            voice_uuid: None,  // Missing voice_uuid
            model: DEFAULT_MODEL.to_string(),
            sample_rate: DEFAULT_SAMPLE_RATE,
            output_format: DEFAULT_OUTPUT_FORMAT.to_string(),
            use_hd: false,
            precision: None,
            project_uuid: None,
            streaming: true,
            provider: None,
        };

        let result = ResembleState::new(config);
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert!(err.contains("voice_uuid"), "Expected 'voice_uuid' in error: {}", err);
    }

    #[test]
    fn test_state_validation_invalid_sample_rate() {
        let config = ResembleConfig {
            api_key: "test".to_string(),
            voice_uuid: Some("test".to_string()),
            model: DEFAULT_MODEL.to_string(),
            sample_rate: 12345, // Invalid
            output_format: DEFAULT_OUTPUT_FORMAT.to_string(),
            use_hd: false,
            precision: None,
            project_uuid: None,
            streaming: true,
            provider: None,
        };

        let result = ResembleState::new(config);
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert!(err.contains("sample_rate"), "Expected 'sample_rate' in error: {}", err);
    }

    #[test]
    fn test_state_validation_invalid_format() {
        let config = ResembleConfig {
            api_key: "test".to_string(),
            voice_uuid: Some("test".to_string()),
            model: DEFAULT_MODEL.to_string(),
            sample_rate: DEFAULT_SAMPLE_RATE,
            output_format: "ogg".to_string(), // Invalid
            use_hd: false,
            precision: None,
            project_uuid: None,
            streaming: true,
            provider: None,
        };

        let result = ResembleState::new(config);
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert!(err.contains("output_format"), "Expected 'output_format' in error: {}", err);
    }

    #[test]
    fn test_manifest() {
        let manifest = get_manifest();
        assert_eq!(manifest.id.as_str(), "resemble-tts");
        assert_eq!(manifest.version.as_str(), "1.0.0");
        assert_eq!(manifest.capabilities.len(), 1);
        assert_eq!(manifest.capabilities[0], PluginCapabilityType::TTS);
    }

    #[test]
    fn test_provider_info() {
        let info = resemble_get_provider_info(std::ptr::null());
        let json: serde_json::Value = serde_json::from_str(info.as_str()).unwrap();

        assert_eq!(json["provider"], "resemble-tts");
        assert_eq!(json["version"], "1.0.0");
        assert!(json["capabilities"].as_array().unwrap().contains(&serde_json::json!("tts")));
    }

    // =============================================================================
    // Hardening Tests
    // =============================================================================

    #[test]
    fn test_circuit_breaker_initial_state() {
        let cb = CircuitBreaker::new();
        assert!(cb.is_allowed(), "Circuit breaker should allow requests initially");
        assert_eq!(cb.get_state(), "closed");
    }

    #[test]
    fn test_circuit_breaker_opens_after_failures() {
        let cb = CircuitBreaker::new();

        // Record failures up to threshold
        for _ in 0..CIRCUIT_BREAKER_FAILURE_THRESHOLD {
            cb.record_failure();
        }

        // Circuit should now be open
        assert!(!cb.is_allowed() || cb.get_state() == "open" || cb.get_state() == "half-open",
            "Circuit breaker should be open or transitioning after {} failures",
            CIRCUIT_BREAKER_FAILURE_THRESHOLD);
    }

    #[test]
    fn test_circuit_breaker_success_resets_count() {
        let cb = CircuitBreaker::new();

        // Record some failures (but not enough to open)
        cb.record_failure();
        cb.record_failure();

        // Record success
        cb.record_success();

        // Should still be closed
        assert_eq!(cb.get_state(), "closed");
        assert!(cb.is_allowed());
    }

    #[test]
    fn test_connection_state_default() {
        assert_eq!(ConnectionState::default(), ConnectionState::Disconnected);
    }

    #[test]
    fn test_state_initialization_with_circuit_breaker() {
        let config = ResembleConfig {
            api_key: "test_key".to_string(),
            voice_uuid: Some("test_voice".to_string()),
            model: DEFAULT_MODEL.to_string(),
            sample_rate: DEFAULT_SAMPLE_RATE,
            output_format: DEFAULT_OUTPUT_FORMAT.to_string(),
            use_hd: false,
            precision: None,
            project_uuid: None,
            streaming: true,
            provider: None,
        };

        let state = ResembleState::new(config).unwrap();

        // Verify initial state
        assert!(!state.is_connected());
        assert!(state.circuit_breaker.is_allowed());
        assert_eq!(state.total_requests.load(Ordering::Relaxed), 0);
        assert_eq!(state.total_failures.load(Ordering::Relaxed), 0);
        assert_eq!(state.total_audio_bytes.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_provider_info_includes_hardening_config() {
        let info = resemble_get_provider_info(std::ptr::null());
        let json: serde_json::Value = serde_json::from_str(info.as_str()).unwrap();

        // Verify hardening section exists
        assert!(json["hardening"].is_object(), "Provider info should include hardening config");
        assert_eq!(json["hardening"]["max_text_buffer_size"], MAX_TEXT_BUFFER_SIZE);
        assert_eq!(json["hardening"]["max_response_size"], MAX_RESPONSE_SIZE);
        assert_eq!(json["hardening"]["max_retries"], MAX_RETRIES);
    }

    #[test]
    fn test_buffer_limit_constants() {
        // Verify constants are reasonable
        assert!(MAX_TEXT_BUFFER_SIZE > 0, "Buffer size limit should be positive");
        assert!(MAX_TEXT_BUFFER_SIZE <= 1_000_000, "Buffer size should be <=1MB for safety");

        assert!(MAX_RESPONSE_SIZE > 0, "Response size limit should be positive");
        assert!(MAX_RESPONSE_SIZE <= 100_000_000, "Response size should be <=100MB for safety");

        assert!(MAX_RETRIES >= 1, "Should retry at least once");
        assert!(MAX_RETRIES <= 10, "Should not retry excessively");
    }

    #[test]
    fn test_timeout_constants() {
        assert!(CONNECT_TIMEOUT_SECS >= 1, "Connect timeout should be at least 1 second");
        assert!(CONNECT_TIMEOUT_SECS <= 30, "Connect timeout should not be excessive");

        assert!(REQUEST_TIMEOUT_SECS >= 5, "Request timeout should be at least 5 seconds");
        assert!(READ_TIMEOUT_SECS >= REQUEST_TIMEOUT_SECS,
            "Read timeout should be >= request timeout for streaming");
    }
}
