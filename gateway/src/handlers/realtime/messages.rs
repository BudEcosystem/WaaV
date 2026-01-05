//! Realtime WebSocket message types
//!
//! This module defines all message types for the Realtime audio-to-audio API.
//! The protocol is designed to be provider-agnostic, abstracting away
//! provider-specific details (OpenAI, etc.) behind a unified interface.

use bytes::Bytes;
use serde::{Deserialize, Serialize};

/// Maximum allowed size for instructions (100 KB)
pub const MAX_INSTRUCTIONS_SIZE: usize = 100 * 1024;

/// Maximum allowed size for text messages (50 KB)
pub const MAX_TEXT_SIZE: usize = 50 * 1024;

/// Maximum allowed size for function result (100 KB)
pub const MAX_FUNCTION_RESULT_SIZE: usize = 100 * 1024;

// =============================================================================
// Incoming Messages (Client -> Server)
// =============================================================================

/// Incoming WebSocket messages from client
#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type")]
#[allow(clippy::large_enum_variant)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum RealtimeIncomingMessage {
    /// Session configuration
    #[serde(rename = "config")]
    Config(RealtimeSessionConfig),

    /// Send text message to the conversation
    #[serde(rename = "text")]
    Text {
        /// Text content
        text: String,
    },

    /// Request model to generate a response
    #[serde(rename = "create_response")]
    CreateResponse,

    /// Cancel current response generation
    #[serde(rename = "cancel_response")]
    CancelResponse,

    /// Commit audio buffer (for manual turn detection)
    #[serde(rename = "commit_audio")]
    CommitAudio,

    /// Clear audio buffer
    #[serde(rename = "clear_audio")]
    ClearAudio,

    /// Submit function call result
    #[serde(rename = "function_result")]
    FunctionResult {
        /// Function call ID
        call_id: String,
        /// Function result as JSON string
        result: String,
    },

    /// Update session configuration mid-stream
    #[serde(rename = "update_session")]
    UpdateSession(RealtimeSessionConfig),
}

/// Realtime session configuration
///
/// This configuration is provider-agnostic. Provider-specific options
/// are abstracted into common patterns.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RealtimeSessionConfig {
    /// Provider to use (e.g., "openai")
    #[serde(default)]
    pub provider: Option<String>,

    /// Model to use (provider-specific)
    /// For OpenAI: "gpt-4o-realtime-preview", "gpt-4o-mini-realtime-preview"
    #[serde(default)]
    pub model: Option<String>,

    /// Voice for TTS output (provider-specific)
    /// For OpenAI: "alloy", "ash", "ballad", "coral", "echo", "sage", "shimmer", "verse"
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
    pub max_response_tokens: Option<i32>,

    /// Turn detection configuration
    #[serde(default)]
    pub turn_detection: Option<TurnDetectionConfig>,

    /// Tool definitions for function calling
    #[serde(default)]
    pub tools: Option<Vec<ToolConfig>>,

    /// Response modalities
    #[serde(default)]
    pub modalities: Option<Vec<String>>,

    /// Enable input audio transcription
    #[serde(default)]
    pub transcribe_input: Option<bool>,

    /// Transcription model for input audio
    /// For OpenAI: "whisper-1", "gpt-4o-transcribe", "gpt-4o-mini-transcribe"
    #[serde(default)]
    pub transcription_model: Option<String>,

    /// Input audio format override
    #[serde(default)]
    pub input_audio_format: Option<String>,

    /// Output audio format override
    #[serde(default)]
    pub output_audio_format: Option<String>,
}

/// Turn detection configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "mode")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum TurnDetectionConfig {
    /// Server-side Voice Activity Detection
    #[serde(rename = "server_vad")]
    ServerVad {
        /// Activation threshold (0.0 to 1.0)
        #[serde(default)]
        threshold: Option<f32>,
        /// Silence duration before end of turn (ms)
        #[serde(default)]
        silence_duration_ms: Option<u32>,
        /// Amount of audio to include before voice detection (ms)
        #[serde(default)]
        prefix_padding_ms: Option<u32>,
    },
    /// Semantic turn detection (provider-specific)
    #[serde(rename = "semantic")]
    Semantic {
        /// Eagerness level (low, medium, high, auto)
        #[serde(default)]
        eagerness: Option<String>,
    },
    /// Manual turn detection (no auto-detection)
    #[serde(rename = "manual")]
    Manual,
}

/// Tool configuration for function calling
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ToolConfig {
    /// Tool type (e.g., "function")
    #[serde(rename = "type")]
    pub tool_type: String,
    /// Function definition
    pub function: FunctionConfig,
}

/// Function definition for tool calling
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FunctionConfig {
    /// Function name
    pub name: String,
    /// Function description
    #[serde(default)]
    pub description: Option<String>,
    /// JSON schema for parameters
    #[serde(default)]
    pub parameters: Option<serde_json::Value>,
}

// =============================================================================
// Outgoing Messages (Server -> Client)
// =============================================================================

/// Outgoing WebSocket messages to client
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum RealtimeOutgoingMessage {
    /// Session created/ready
    #[serde(rename = "session_created")]
    SessionCreated {
        /// Session ID
        session_id: String,
        /// Provider used
        provider: String,
        /// Model in use
        model: String,
    },

    /// Session updated
    #[serde(rename = "session_updated")]
    SessionUpdated,

    /// Transcript from user speech
    #[serde(rename = "transcript")]
    Transcript {
        /// Transcribed text
        text: String,
        /// Role (user or assistant)
        role: String,
        /// Whether this is a final transcript
        is_final: bool,
    },

    /// Speech detection event
    #[serde(rename = "speech_event")]
    SpeechEvent {
        /// Event type (started, stopped)
        event: String,
        /// Audio timestamp in milliseconds
        audio_ms: u64,
    },

    /// Function call request from model
    #[serde(rename = "function_call")]
    FunctionCall {
        /// Call ID
        call_id: String,
        /// Function name
        name: String,
        /// JSON arguments
        arguments: String,
    },

    /// Response generation started
    #[serde(rename = "response_started")]
    ResponseStarted {
        /// Response ID
        response_id: String,
    },

    /// Response generation completed
    #[serde(rename = "response_done")]
    ResponseDone {
        /// Response ID
        response_id: String,
    },

    /// Error message
    #[serde(rename = "error")]
    Error {
        /// Error code (optional)
        #[serde(skip_serializing_if = "Option::is_none")]
        code: Option<String>,
        /// Error message
        message: String,
    },

    /// Connection closing
    #[serde(rename = "closing")]
    Closing {
        /// Reason for closing
        reason: String,
    },
}

// =============================================================================
// Message Routing
// =============================================================================

/// Message routing for optimized throughput
pub enum RealtimeMessageRoute {
    /// JSON text message
    Outgoing(RealtimeOutgoingMessage),
    /// Binary audio data
    Audio(Bytes),
    /// Close connection
    Close,
}

// =============================================================================
// Validation
// =============================================================================

/// Error type for message validation failures
#[derive(Debug, Clone)]
pub enum RealtimeValidationError {
    /// Instructions exceed maximum allowed size
    InstructionsTooLarge { size: usize, max: usize },
    /// Text content exceeds maximum allowed size
    TextTooLarge { size: usize, max: usize },
    /// Function result exceeds maximum allowed size
    FunctionResultTooLarge { size: usize, max: usize },
    /// Invalid provider
    InvalidProvider { provider: String },
}

impl std::fmt::Display for RealtimeValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InstructionsTooLarge { size, max } => {
                write!(
                    f,
                    "Instructions too large: {} bytes (max: {} bytes)",
                    size, max
                )
            }
            Self::TextTooLarge { size, max } => {
                write!(f, "Text too large: {} bytes (max: {} bytes)", size, max)
            }
            Self::FunctionResultTooLarge { size, max } => {
                write!(
                    f,
                    "Function result too large: {} bytes (max: {} bytes)",
                    size, max
                )
            }
            Self::InvalidProvider { provider } => {
                write!(f, "Invalid provider: {}", provider)
            }
        }
    }
}

impl std::error::Error for RealtimeValidationError {}

impl RealtimeIncomingMessage {
    /// Validates message field sizes to prevent resource exhaustion attacks.
    pub fn validate_size(&self) -> Result<(), RealtimeValidationError> {
        match self {
            RealtimeIncomingMessage::Config(config)
            | RealtimeIncomingMessage::UpdateSession(config) => {
                if let Some(instructions) = &config.instructions {
                    let size = instructions.len();
                    if size > MAX_INSTRUCTIONS_SIZE {
                        return Err(RealtimeValidationError::InstructionsTooLarge {
                            size,
                            max: MAX_INSTRUCTIONS_SIZE,
                        });
                    }
                }
            }
            RealtimeIncomingMessage::Text { text } => {
                let size = text.len();
                if size > MAX_TEXT_SIZE {
                    return Err(RealtimeValidationError::TextTooLarge {
                        size,
                        max: MAX_TEXT_SIZE,
                    });
                }
            }
            RealtimeIncomingMessage::FunctionResult { result, .. } => {
                let size = result.len();
                if size > MAX_FUNCTION_RESULT_SIZE {
                    return Err(RealtimeValidationError::FunctionResultTooLarge {
                        size,
                        max: MAX_FUNCTION_RESULT_SIZE,
                    });
                }
            }
            // Other messages don't have user-provided content that needs size limits
            RealtimeIncomingMessage::CreateResponse
            | RealtimeIncomingMessage::CancelResponse
            | RealtimeIncomingMessage::CommitAudio
            | RealtimeIncomingMessage::ClearAudio => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_message_deserialization() {
        let json = r#"{
            "type": "config",
            "provider": "openai",
            "model": "gpt-4o-realtime-preview",
            "voice": "alloy",
            "instructions": "You are a helpful assistant."
        }"#;

        let msg: RealtimeIncomingMessage = serde_json::from_str(json).expect("Should deserialize");
        match msg {
            RealtimeIncomingMessage::Config(config) => {
                assert_eq!(config.provider.as_deref(), Some("openai"));
                assert_eq!(config.model.as_deref(), Some("gpt-4o-realtime-preview"));
                assert_eq!(config.voice.as_deref(), Some("alloy"));
            }
            _ => panic!("Expected Config variant"),
        }
    }

    #[test]
    fn test_text_message_deserialization() {
        let json = r#"{"type": "text", "text": "Hello, world!"}"#;
        let msg: RealtimeIncomingMessage = serde_json::from_str(json).expect("Should deserialize");
        match msg {
            RealtimeIncomingMessage::Text { text } => {
                assert_eq!(text, "Hello, world!");
            }
            _ => panic!("Expected Text variant"),
        }
    }

    #[test]
    fn test_function_result_deserialization() {
        let json = r#"{"type": "function_result", "call_id": "call_123", "result": "{\"weather\": \"sunny\"}"}"#;
        let msg: RealtimeIncomingMessage = serde_json::from_str(json).expect("Should deserialize");
        match msg {
            RealtimeIncomingMessage::FunctionResult { call_id, result } => {
                assert_eq!(call_id, "call_123");
                assert!(result.contains("sunny"));
            }
            _ => panic!("Expected FunctionResult variant"),
        }
    }

    #[test]
    fn test_session_created_serialization() {
        let msg = RealtimeOutgoingMessage::SessionCreated {
            session_id: "sess_123".to_string(),
            provider: "openai".to_string(),
            model: "gpt-4o-realtime-preview".to_string(),
        };

        let json = serde_json::to_string(&msg).expect("Should serialize");
        assert!(json.contains(r#""type":"session_created""#));
        assert!(json.contains(r#""session_id":"sess_123""#));
    }

    #[test]
    fn test_transcript_serialization() {
        let msg = RealtimeOutgoingMessage::Transcript {
            text: "Hello".to_string(),
            role: "user".to_string(),
            is_final: true,
        };

        let json = serde_json::to_string(&msg).expect("Should serialize");
        assert!(json.contains(r#""type":"transcript""#));
        assert!(json.contains(r#""text":"Hello""#));
        assert!(json.contains(r#""is_final":true"#));
    }

    #[test]
    fn test_error_serialization() {
        let msg = RealtimeOutgoingMessage::Error {
            code: Some("invalid_config".to_string()),
            message: "Provider not supported".to_string(),
        };

        let json = serde_json::to_string(&msg).expect("Should serialize");
        assert!(json.contains(r#""type":"error""#));
        assert!(json.contains(r#""code":"invalid_config""#));
    }

    #[test]
    fn test_validation_instructions_within_limit() {
        let config = RealtimeSessionConfig {
            instructions: Some("a".repeat(MAX_INSTRUCTIONS_SIZE)),
            ..Default::default()
        };
        let msg = RealtimeIncomingMessage::Config(config);
        assert!(msg.validate_size().is_ok());
    }

    #[test]
    fn test_validation_instructions_exceeds_limit() {
        let config = RealtimeSessionConfig {
            instructions: Some("a".repeat(MAX_INSTRUCTIONS_SIZE + 1)),
            ..Default::default()
        };
        let msg = RealtimeIncomingMessage::Config(config);
        let err = msg.validate_size().unwrap_err();
        match err {
            RealtimeValidationError::InstructionsTooLarge { .. } => {}
            _ => panic!("Expected InstructionsTooLarge error"),
        }
    }

    #[test]
    fn test_validation_text_exceeds_limit() {
        let msg = RealtimeIncomingMessage::Text {
            text: "a".repeat(MAX_TEXT_SIZE + 1),
        };
        let err = msg.validate_size().unwrap_err();
        match err {
            RealtimeValidationError::TextTooLarge { .. } => {}
            _ => panic!("Expected TextTooLarge error"),
        }
    }

    #[test]
    fn test_turn_detection_config_deserialization() {
        let json = r#"{
            "type": "config",
            "turn_detection": {
                "mode": "server_vad",
                "threshold": 0.5,
                "silence_duration_ms": 500
            }
        }"#;

        let msg: RealtimeIncomingMessage = serde_json::from_str(json).expect("Should deserialize");
        match msg {
            RealtimeIncomingMessage::Config(config) => {
                let td = config.turn_detection.expect("Should have turn_detection");
                match td {
                    TurnDetectionConfig::ServerVad {
                        threshold,
                        silence_duration_ms,
                        ..
                    } => {
                        assert_eq!(threshold, Some(0.5));
                        assert_eq!(silence_duration_ms, Some(500));
                    }
                    _ => panic!("Expected ServerVad variant"),
                }
            }
            _ => panic!("Expected Config variant"),
        }
    }
}
