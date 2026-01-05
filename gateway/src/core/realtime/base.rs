//! Base traits and types for Realtime audio-to-audio providers.
//!
//! This module defines the foundational abstractions for providers that support
//! bidirectional audio streaming with real-time transcription and TTS.
//!
//! # Supported Providers
//!
//! - OpenAI Realtime API (gpt-4o-realtime-preview)
//!
//! # Audio Format
//!
//! All providers use PCM 16-bit signed little-endian at 24kHz sample rate.

use async_trait::async_trait;
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use thiserror::Error;

// =============================================================================
// Error Types
// =============================================================================

/// Errors that can occur during realtime operations.
#[derive(Debug, Error)]
pub enum RealtimeError {
    /// Connection to the provider failed
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),

    /// Authentication failed
    #[error("Authentication failed: {0}")]
    AuthenticationFailed(String),

    /// Invalid configuration
    #[error("Invalid configuration: {0}")]
    InvalidConfiguration(String),

    /// WebSocket error
    #[error("WebSocket error: {0}")]
    WebSocketError(String),

    /// Provider-specific error
    #[error("Provider error: {0}")]
    ProviderError(String),

    /// Serialization error
    #[error("Serialization error: {0}")]
    SerializationError(String),

    /// Operation timeout
    #[error("Operation timed out: {0}")]
    Timeout(String),

    /// Not connected
    #[error("Not connected")]
    NotConnected,

    /// Session error
    #[error("Session error: {0}")]
    SessionError(String),

    /// Rate limit exceeded
    #[error("Rate limit exceeded: {0}")]
    RateLimitExceeded(String),

    /// Internal error
    #[error("Internal error: {0}")]
    InternalError(String),
}

/// Result type for realtime operations.
pub type RealtimeResult<T> = Result<T, RealtimeError>;

// =============================================================================
// Configuration Types
// =============================================================================

/// Configuration for automatic reconnection behavior.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReconnectionConfig {
    /// Enable automatic reconnection on connection loss.
    /// Default: true
    pub enabled: bool,

    /// Maximum number of reconnection attempts before giving up.
    /// Set to 0 for unlimited attempts.
    /// Default: 5
    pub max_attempts: u32,

    /// Initial delay between reconnection attempts (milliseconds).
    /// Default: 1000ms
    pub initial_delay_ms: u64,

    /// Maximum delay between reconnection attempts (milliseconds).
    /// Default: 30000ms (30 seconds)
    pub max_delay_ms: u64,

    /// Multiplier for exponential backoff.
    /// Default: 2.0
    pub backoff_multiplier: f32,

    /// Whether to add jitter to the delay to prevent thundering herd.
    /// Default: true
    pub jitter: bool,
}

impl Default for ReconnectionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_attempts: 5,
            initial_delay_ms: 1000,
            max_delay_ms: 30000,
            backoff_multiplier: 2.0,
            jitter: true,
        }
    }
}

impl ReconnectionConfig {
    /// Create a config with reconnection disabled.
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Default::default()
        }
    }

    /// Calculate the delay for a given attempt number using exponential backoff.
    /// Returns the delay in milliseconds.
    pub fn calculate_delay(&self, attempt: u32) -> u64 {
        let base_delay = self.initial_delay_ms as f64;
        let multiplier = self.backoff_multiplier as f64;

        // Exponential backoff: base_delay * multiplier^(attempt-1)
        let delay = base_delay * multiplier.powi(attempt.saturating_sub(1) as i32);
        let delay = delay.min(self.max_delay_ms as f64);

        if self.jitter {
            // Add up to 25% jitter
            let jitter_range = delay * 0.25;
            let jitter = rand_jitter(jitter_range);
            (delay + jitter) as u64
        } else {
            delay as u64
        }
    }

    /// Check if more reconnection attempts are allowed.
    pub fn should_retry(&self, attempt: u32) -> bool {
        self.enabled && (self.max_attempts == 0 || attempt < self.max_attempts)
    }
}

/// Generate a pseudo-random jitter value using a simple LCG.
/// This avoids pulling in the rand crate for a simple use case.
fn rand_jitter(range: f64) -> f64 {
    use std::time::SystemTime;
    let seed = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    // Simple LCG: (a * seed + c) mod m
    let random = ((seed.wrapping_mul(1103515245).wrapping_add(12345)) % (1 << 31)) as f64;
    let normalized = random / (1u64 << 31) as f64; // 0.0 to 1.0
    (normalized - 0.5) * 2.0 * range // -range to +range
}

/// Base configuration for realtime providers.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RealtimeConfig {
    /// API key for authentication
    pub api_key: String,

    /// Provider name (e.g., "openai")
    #[serde(default)]
    pub provider: String,

    /// Model to use (e.g., "gpt-4o-realtime-preview")
    #[serde(default)]
    pub model: String,

    /// Voice ID for TTS output
    #[serde(default)]
    pub voice: Option<String>,

    /// System instructions for the assistant
    #[serde(default)]
    pub instructions: Option<String>,

    /// Temperature for response generation (0.0 to 2.0)
    #[serde(default)]
    pub temperature: Option<f32>,

    /// Maximum response tokens (-1 for infinite)
    #[serde(default)]
    pub max_response_output_tokens: Option<i32>,

    /// Input audio format
    #[serde(default)]
    pub input_audio_format: Option<String>,

    /// Output audio format
    #[serde(default)]
    pub output_audio_format: Option<String>,

    /// Enable input audio transcription
    #[serde(default)]
    pub input_audio_transcription: Option<InputTranscriptionConfig>,

    /// Turn detection configuration
    #[serde(default)]
    pub turn_detection: Option<TurnDetectionConfig>,

    /// Tool definitions for function calling
    #[serde(default)]
    pub tools: Option<Vec<ToolDefinition>>,

    /// Tool choice strategy
    #[serde(default)]
    pub tool_choice: Option<String>,

    /// Response modalities (text, audio, or both)
    #[serde(default)]
    pub modalities: Option<Vec<String>>,

    /// Reconnection configuration for automatic reconnection on connection loss.
    #[serde(default)]
    pub reconnection: Option<ReconnectionConfig>,
}

/// Configuration for input audio transcription.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InputTranscriptionConfig {
    /// Model to use for transcription (e.g., "whisper-1")
    pub model: String,
}

/// Configuration for turn detection (VAD).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum TurnDetectionConfig {
    /// Server-side VAD using Silero
    #[serde(rename = "server_vad")]
    ServerVad {
        /// Activation threshold (0.0 to 1.0)
        #[serde(default)]
        threshold: Option<f32>,
        /// Amount of audio to include before voice detection (ms)
        #[serde(default)]
        prefix_padding_ms: Option<u32>,
        /// Silence duration before end of turn (ms)
        #[serde(default)]
        silence_duration_ms: Option<u32>,
        /// Eagerness for detecting speech (0.0 = never, 1.0 = always)
        #[serde(default)]
        create_response: Option<bool>,
        /// Interrupt model output on speech detection
        #[serde(default)]
        interrupt_response: Option<bool>,
    },
    /// Semantic-aware turn detection
    #[serde(rename = "semantic_vad")]
    SemanticVad {
        /// Eagerness level (low, medium, high, auto)
        #[serde(default)]
        eagerness: Option<String>,
        /// Whether to create response on turn end
        #[serde(default)]
        create_response: Option<bool>,
        /// Interrupt model output on speech detection
        #[serde(default)]
        interrupt_response: Option<bool>,
    },
    /// No automatic turn detection
    #[serde(rename = "none")]
    None,
}

impl Default for TurnDetectionConfig {
    fn default() -> Self {
        TurnDetectionConfig::ServerVad {
            threshold: Some(0.5),
            prefix_padding_ms: Some(300),
            silence_duration_ms: Some(500),
            create_response: Some(true),
            interrupt_response: Some(true),
        }
    }
}

/// Tool definition for function calling.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Tool type (always "function")
    #[serde(rename = "type")]
    pub tool_type: String,
    /// Function definition
    pub function: FunctionDefinition,
}

/// Function definition for tool calling.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDefinition {
    /// Function name
    pub name: String,
    /// Function description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON schema for parameters
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<serde_json::Value>,
}

// =============================================================================
// Connection State
// =============================================================================

/// Connection state for realtime providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConnectionState {
    /// Not connected to the provider
    #[default]
    Disconnected,
    /// Currently connecting
    Connecting,
    /// Connected and ready
    Connected,
    /// Reconnecting after connection loss
    Reconnecting,
    /// Connection failed
    Failed,
}

impl fmt::Display for ConnectionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConnectionState::Disconnected => write!(f, "Disconnected"),
            ConnectionState::Connecting => write!(f, "Connecting"),
            ConnectionState::Connected => write!(f, "Connected"),
            ConnectionState::Reconnecting => write!(f, "Reconnecting"),
            ConnectionState::Failed => write!(f, "Failed"),
        }
    }
}

// =============================================================================
// Callback Types
// =============================================================================

/// Transcript result from realtime transcription.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptResult {
    /// The transcribed text
    pub text: String,
    /// Role of the speaker (user or assistant)
    pub role: TranscriptRole,
    /// Whether this is a final transcript
    pub is_final: bool,
    /// Item ID from the provider
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
}

/// Role of the speaker in a transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TranscriptRole {
    /// User speech transcript
    User,
    /// Assistant speech transcript
    Assistant,
}

impl fmt::Display for TranscriptRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TranscriptRole::User => write!(f, "user"),
            TranscriptRole::Assistant => write!(f, "assistant"),
        }
    }
}

/// Audio data from realtime TTS.
#[derive(Debug, Clone)]
pub struct RealtimeAudioData {
    /// Raw audio bytes (PCM 16-bit, 24kHz, mono, little-endian)
    pub data: Bytes,
    /// Sample rate in Hz
    pub sample_rate: u32,
    /// Item ID from the provider
    pub item_id: Option<String>,
    /// Response ID from the provider
    pub response_id: Option<String>,
}

/// Function call request from the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCallRequest {
    /// Call ID for the function call
    pub call_id: String,
    /// Function name
    pub name: String,
    /// JSON arguments
    pub arguments: String,
    /// Item ID
    pub item_id: Option<String>,
}

/// Speech events (VAD events).
#[derive(Debug, Clone)]
pub enum SpeechEvent {
    /// Speech started detection
    Started {
        /// Audio timestamp in milliseconds
        audio_start_ms: u64,
        /// Item ID
        item_id: Option<String>,
    },
    /// Speech stopped detection
    Stopped {
        /// Audio timestamp in milliseconds
        audio_end_ms: u64,
        /// Item ID
        item_id: Option<String>,
    },
}

// =============================================================================
// Callback Traits
// =============================================================================

/// Callback type for transcript events.
pub type TranscriptCallback =
    Arc<dyn Fn(TranscriptResult) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// Callback type for audio output events.
pub type AudioOutputCallback =
    Arc<dyn Fn(RealtimeAudioData) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// Callback type for error events.
pub type RealtimeErrorCallback =
    Arc<dyn Fn(RealtimeError) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// Callback type for function call events.
pub type FunctionCallCallback =
    Arc<dyn Fn(FunctionCallRequest) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// Callback type for speech events (VAD).
pub type SpeechEventCallback =
    Arc<dyn Fn(SpeechEvent) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// Callback type for response completion.
pub type ResponseDoneCallback =
    Arc<dyn Fn(String) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// Reconnection event details.
#[derive(Debug, Clone)]
pub struct ReconnectionEvent {
    /// Number of reconnection attempts made
    pub attempt: u32,
    /// Whether reconnection was successful
    pub success: bool,
    /// Error message if reconnection failed
    pub error: Option<String>,
}

/// Callback type for reconnection events.
/// Called when the client reconnects after connection loss.
pub type ReconnectionCallback =
    Arc<dyn Fn(ReconnectionEvent) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

// =============================================================================
// Base Trait
// =============================================================================

/// Base trait for realtime audio-to-audio providers.
///
/// This trait defines the interface for providers that support bidirectional
/// audio streaming with real-time transcription and TTS.
///
/// # Audio Format
///
/// Input and output audio should be PCM 16-bit signed little-endian at 24kHz.
///
/// # Example
///
/// ```rust,ignore
/// use waav_gateway::core::realtime::{BaseRealtime, RealtimeConfig};
///
/// #[tokio::main]
/// async fn main() {
///     let config = RealtimeConfig {
///         api_key: "sk-...".to_string(),
///         model: "gpt-4o-realtime-preview".to_string(),
///         voice: Some("alloy".to_string()),
///         ..Default::default()
///     };
///
///     let mut realtime = OpenAIRealtime::new(config)?;
///     realtime.connect().await?;
///
///     // Register callbacks
///     realtime.on_transcript(Arc::new(|t| Box::pin(async move {
///         println!("Transcript: {}", t.text);
///     })))?;
///
///     realtime.on_audio(Arc::new(|audio| Box::pin(async move {
///         // Play audio
///     })))?;
///
///     // Send audio
///     realtime.send_audio(audio_bytes).await?;
/// }
/// ```
#[async_trait]
pub trait BaseRealtime: Send + Sync {
    /// Create a new realtime provider instance.
    fn new(config: RealtimeConfig) -> RealtimeResult<Self>
    where
        Self: Sized;

    /// Connect to the realtime provider.
    async fn connect(&mut self) -> RealtimeResult<()>;

    /// Disconnect from the realtime provider.
    async fn disconnect(&mut self) -> RealtimeResult<()>;

    /// Check if the provider is connected and ready.
    fn is_ready(&self) -> bool;

    /// Get the current connection state.
    fn get_connection_state(&self) -> ConnectionState;

    // -------------------------------------------------------------------------
    // Audio I/O
    // -------------------------------------------------------------------------

    /// Send audio data to the provider.
    ///
    /// Audio should be PCM 16-bit, 24kHz, mono, little-endian.
    async fn send_audio(&mut self, audio_data: Bytes) -> RealtimeResult<()>;

    /// Send a text message to the conversation.
    async fn send_text(&mut self, text: &str) -> RealtimeResult<()>;

    // -------------------------------------------------------------------------
    // Session Control
    // -------------------------------------------------------------------------

    /// Request the model to generate a response.
    async fn create_response(&mut self) -> RealtimeResult<()>;

    /// Cancel the current response generation.
    async fn cancel_response(&mut self) -> RealtimeResult<()>;

    /// Commit the audio buffer (for manual turn detection).
    async fn commit_audio_buffer(&mut self) -> RealtimeResult<()>;

    /// Clear the audio buffer.
    async fn clear_audio_buffer(&mut self) -> RealtimeResult<()>;

    // -------------------------------------------------------------------------
    // Callbacks
    // -------------------------------------------------------------------------

    /// Register a callback for transcript events.
    fn on_transcript(&mut self, callback: TranscriptCallback) -> RealtimeResult<()>;

    /// Register a callback for audio output events.
    fn on_audio(&mut self, callback: AudioOutputCallback) -> RealtimeResult<()>;

    /// Register a callback for error events.
    fn on_error(&mut self, callback: RealtimeErrorCallback) -> RealtimeResult<()>;

    /// Register a callback for function call events.
    fn on_function_call(&mut self, callback: FunctionCallCallback) -> RealtimeResult<()>;

    /// Register a callback for speech events (VAD).
    fn on_speech_event(&mut self, callback: SpeechEventCallback) -> RealtimeResult<()>;

    /// Register a callback for response completion.
    fn on_response_done(&mut self, callback: ResponseDoneCallback) -> RealtimeResult<()>;

    /// Register a callback for reconnection events.
    ///
    /// This callback is invoked when the client automatically reconnects after
    /// connection loss. It provides visibility into reconnection attempts and
    /// allows clients to take action (e.g., re-send state) after reconnection.
    fn on_reconnection(&mut self, callback: ReconnectionCallback) -> RealtimeResult<()>;

    // -------------------------------------------------------------------------
    // Configuration
    // -------------------------------------------------------------------------

    /// Update the session configuration.
    async fn update_session(&mut self, config: RealtimeConfig) -> RealtimeResult<()>;

    /// Submit a function call result.
    async fn submit_function_result(&mut self, call_id: &str, result: &str) -> RealtimeResult<()>;

    /// Get provider information.
    fn get_provider_info(&self) -> serde_json::Value;
}

// =============================================================================
// Factory
// =============================================================================

/// Boxed trait object for realtime providers.
pub type BoxedRealtime = Box<dyn BaseRealtime>;

/// Factory trait for creating realtime providers.
pub trait RealtimeFactory {
    /// Create a new realtime provider from configuration.
    fn create(config: RealtimeConfig) -> RealtimeResult<BoxedRealtime>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connection_state_display() {
        assert_eq!(ConnectionState::Connected.to_string(), "Connected");
        assert_eq!(ConnectionState::Disconnected.to_string(), "Disconnected");
        assert_eq!(ConnectionState::Connecting.to_string(), "Connecting");
    }

    #[test]
    fn test_transcript_role_display() {
        assert_eq!(TranscriptRole::User.to_string(), "user");
        assert_eq!(TranscriptRole::Assistant.to_string(), "assistant");
    }

    #[test]
    fn test_default_config() {
        let config = RealtimeConfig::default();
        assert!(config.api_key.is_empty());
        assert!(config.voice.is_none());
    }

    #[test]
    fn test_default_turn_detection() {
        let td = TurnDetectionConfig::default();
        match td {
            TurnDetectionConfig::ServerVad { threshold, .. } => {
                assert_eq!(threshold, Some(0.5));
            }
            _ => panic!("Expected ServerVad default"),
        }
    }

    #[test]
    fn test_error_display() {
        let err = RealtimeError::ConnectionFailed("test".to_string());
        assert!(err.to_string().contains("Connection failed"));

        let err = RealtimeError::NotConnected;
        assert_eq!(err.to_string(), "Not connected");
    }

    #[test]
    fn test_reconnection_config_default() {
        let config = ReconnectionConfig::default();
        assert!(config.enabled);
        assert_eq!(config.max_attempts, 5);
        assert_eq!(config.initial_delay_ms, 1000);
        assert_eq!(config.max_delay_ms, 30000);
        assert_eq!(config.backoff_multiplier, 2.0);
        assert!(config.jitter);
    }

    #[test]
    fn test_reconnection_config_disabled() {
        let config = ReconnectionConfig::disabled();
        assert!(!config.enabled);
    }

    #[test]
    fn test_reconnection_should_retry() {
        let config = ReconnectionConfig::default();

        // Should retry for attempts 0-4 when max_attempts is 5
        assert!(config.should_retry(0));
        assert!(config.should_retry(1));
        assert!(config.should_retry(4));
        assert!(!config.should_retry(5));
        assert!(!config.should_retry(10));

        // Disabled config should never retry
        let disabled = ReconnectionConfig::disabled();
        assert!(!disabled.should_retry(0));
    }

    #[test]
    fn test_reconnection_unlimited_attempts() {
        let config = ReconnectionConfig {
            max_attempts: 0, // Unlimited
            ..Default::default()
        };

        assert!(config.should_retry(0));
        assert!(config.should_retry(100));
        assert!(config.should_retry(u32::MAX));
    }

    #[test]
    fn test_reconnection_calculate_delay_no_jitter() {
        let config = ReconnectionConfig {
            initial_delay_ms: 1000,
            max_delay_ms: 30000,
            backoff_multiplier: 2.0,
            jitter: false,
            ..Default::default()
        };

        // First attempt: 1000ms
        assert_eq!(config.calculate_delay(1), 1000);

        // Second attempt: 2000ms
        assert_eq!(config.calculate_delay(2), 2000);

        // Third attempt: 4000ms
        assert_eq!(config.calculate_delay(3), 4000);

        // Fourth attempt: 8000ms
        assert_eq!(config.calculate_delay(4), 8000);

        // Fifth attempt: 16000ms
        assert_eq!(config.calculate_delay(5), 16000);

        // Sixth attempt: capped at 30000ms
        assert_eq!(config.calculate_delay(6), 30000);
    }

    #[test]
    fn test_reconnection_calculate_delay_with_jitter() {
        let config = ReconnectionConfig {
            initial_delay_ms: 1000,
            jitter: true,
            ..Default::default()
        };

        // With jitter, the delay should be within 25% of the base delay
        let delay = config.calculate_delay(1);
        assert!(
            delay >= 750 && delay <= 1250,
            "Delay {} should be within 750-1250",
            delay
        );
    }
}
