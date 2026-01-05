//! OpenAI Realtime API WebSocket message types.
//!
//! This module defines the client and server event types for the OpenAI Realtime API.
//! All events are JSON-encoded and sent over WebSocket.
//!
//! # Protocol Overview
//!
//! Client events (sent to server):
//! - session.update - Update session configuration
//! - input_audio_buffer.append - Append audio to buffer
//! - input_audio_buffer.commit - Commit audio buffer
//! - input_audio_buffer.clear - Clear audio buffer
//! - conversation.item.create - Add item to conversation
//! - conversation.item.delete - Delete conversation item
//! - response.create - Generate a response
//! - response.cancel - Cancel current response
//!
//! Server events (received from server):
//! - session.created - Session created
//! - session.updated - Session configuration updated
//! - input_audio_buffer.speech_started - Speech detection started
//! - input_audio_buffer.speech_stopped - Speech detection stopped
//! - input_audio_buffer.committed - Audio buffer committed
//! - conversation.item.created - Item added to conversation
//! - response.created - Response generation started
//! - response.output_item.added - Output item added
//! - response.audio.delta - Audio data chunk
//! - response.audio.done - Audio generation complete
//! - response.audio_transcript.delta - Transcript chunk
//! - response.audio_transcript.done - Transcript complete
//! - response.text.delta - Text chunk
//! - response.text.done - Text complete
//! - response.done - Response complete
//! - error - Error occurred

use base64::prelude::*;
use serde::{Deserialize, Serialize};

// =============================================================================
// Session Configuration
// =============================================================================

/// Session configuration for OpenAI Realtime API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfig {
    /// Response modalities (text, audio)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modalities: Option<Vec<String>>,

    /// System instructions for the assistant
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,

    /// Voice for audio output
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,

    /// Input audio format
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_audio_format: Option<String>,

    /// Output audio format
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_audio_format: Option<String>,

    /// Input audio transcription configuration
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_audio_transcription: Option<InputAudioTranscription>,

    /// Turn detection configuration
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_detection: Option<TurnDetection>,

    /// Tool definitions
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDef>>,

    /// Tool choice strategy
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<String>,

    /// Temperature for response generation
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,

    /// Maximum response output tokens
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_response_output_tokens: Option<MaxTokens>,
}

/// Maximum tokens configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MaxTokens {
    /// Specific number of tokens
    Number(i32),
    /// Infinite tokens
    Infinite(String), // "inf"
}

/// Input audio transcription configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputAudioTranscription {
    /// Transcription model (e.g., "whisper-1")
    pub model: String,
}

/// Turn detection configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum TurnDetection {
    /// Server-side VAD
    #[serde(rename = "server_vad")]
    ServerVad {
        /// Activation threshold
        #[serde(skip_serializing_if = "Option::is_none")]
        threshold: Option<f32>,
        /// Audio prefix padding in ms
        #[serde(skip_serializing_if = "Option::is_none")]
        prefix_padding_ms: Option<u32>,
        /// Silence duration in ms
        #[serde(skip_serializing_if = "Option::is_none")]
        silence_duration_ms: Option<u32>,
        /// Whether to create response on turn end
        #[serde(skip_serializing_if = "Option::is_none")]
        create_response: Option<bool>,
        /// Whether to interrupt on speech
        #[serde(skip_serializing_if = "Option::is_none")]
        interrupt_response: Option<bool>,
    },
    /// Semantic VAD
    #[serde(rename = "semantic_vad")]
    SemanticVad {
        /// Eagerness level
        #[serde(skip_serializing_if = "Option::is_none")]
        eagerness: Option<String>,
        /// Whether to create response on turn end
        #[serde(skip_serializing_if = "Option::is_none")]
        create_response: Option<bool>,
        /// Whether to interrupt on speech
        #[serde(skip_serializing_if = "Option::is_none")]
        interrupt_response: Option<bool>,
    },
    /// No turn detection
    #[serde(rename = "none")]
    None {},
}

/// Tool definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDef {
    /// Tool type (always "function")
    #[serde(rename = "type")]
    pub tool_type: String,
    /// Function name
    pub name: String,
    /// Function description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Function parameters JSON schema
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<serde_json::Value>,
}

// =============================================================================
// Conversation Items
// =============================================================================

/// Conversation item.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationItem {
    /// Item ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Item type
    #[serde(rename = "type")]
    pub item_type: String,
    /// Item status
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Item role (user, assistant, system)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Content parts
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<Vec<ContentPart>>,
    /// Call ID for function call
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
    /// Function name for function call
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Function arguments for function call
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<String>,
    /// Function output for function call result
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
}

/// Content part within a conversation item.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentPart {
    /// Content type (input_text, input_audio, text, audio)
    #[serde(rename = "type")]
    pub content_type: String,
    /// Text content
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Audio content (base64 encoded)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio: Option<String>,
    /// Transcript of audio content
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcript: Option<String>,
}

// =============================================================================
// Response Configuration
// =============================================================================

/// Response configuration for creating responses.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResponseConfig {
    /// Response modalities
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modalities: Option<Vec<String>>,
    /// System instructions
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    /// Voice for audio
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
    /// Output audio format
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_audio_format: Option<String>,
    /// Tools
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDef>>,
    /// Tool choice
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<String>,
    /// Temperature
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Max output tokens
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_response_output_tokens: Option<MaxTokens>,
    /// Conversation to use
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation: Option<String>,
    /// Metadata
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    /// Input items to add
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<Vec<ConversationItem>>,
}

// =============================================================================
// Client Events (sent to server)
// =============================================================================

/// Client events sent to the OpenAI Realtime API.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum ClientEvent {
    /// Update session configuration
    #[serde(rename = "session.update")]
    SessionUpdate {
        /// Session configuration
        session: SessionConfig,
    },

    /// Append audio to input buffer
    #[serde(rename = "input_audio_buffer.append")]
    InputAudioBufferAppend {
        /// Base64-encoded audio data
        audio: String,
    },

    /// Commit the input audio buffer
    #[serde(rename = "input_audio_buffer.commit")]
    InputAudioBufferCommit,

    /// Clear the input audio buffer
    #[serde(rename = "input_audio_buffer.clear")]
    InputAudioBufferClear,

    /// Create a conversation item
    #[serde(rename = "conversation.item.create")]
    ConversationItemCreate {
        /// Item to create
        item: ConversationItem,
        /// Previous item ID to insert after
        #[serde(skip_serializing_if = "Option::is_none")]
        previous_item_id: Option<String>,
    },

    /// Truncate a conversation item
    #[serde(rename = "conversation.item.truncate")]
    ConversationItemTruncate {
        /// Item ID
        item_id: String,
        /// Content index
        content_index: u32,
        /// Audio end in ms
        audio_end_ms: u32,
    },

    /// Delete a conversation item
    #[serde(rename = "conversation.item.delete")]
    ConversationItemDelete {
        /// Item ID
        item_id: String,
    },

    /// Create a response
    #[serde(rename = "response.create")]
    ResponseCreate {
        /// Response configuration
        #[serde(skip_serializing_if = "Option::is_none")]
        response: Option<ResponseConfig>,
    },

    /// Cancel the current response
    #[serde(rename = "response.cancel")]
    ResponseCancel,
}

impl ClientEvent {
    /// Create an audio append event from raw bytes.
    pub fn audio_append(data: &[u8]) -> Self {
        ClientEvent::InputAudioBufferAppend {
            audio: BASE64_STANDARD.encode(data),
        }
    }
}

// =============================================================================
// Server Events (received from server)
// =============================================================================

/// Server events received from the OpenAI Realtime API.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum ServerEvent {
    /// Error occurred
    #[serde(rename = "error")]
    Error {
        /// Error details
        error: ApiError,
    },

    /// Session created
    #[serde(rename = "session.created")]
    SessionCreated {
        /// Session information
        session: Session,
    },

    /// Session updated
    #[serde(rename = "session.updated")]
    SessionUpdated {
        /// Session information
        session: Session,
    },

    /// Speech started (VAD detected speech)
    #[serde(rename = "input_audio_buffer.speech_started")]
    SpeechStarted {
        /// Audio start timestamp in ms
        audio_start_ms: u64,
        /// Item ID
        item_id: String,
    },

    /// Speech stopped (VAD detected silence)
    #[serde(rename = "input_audio_buffer.speech_stopped")]
    SpeechStopped {
        /// Audio end timestamp in ms
        audio_end_ms: u64,
        /// Item ID
        item_id: String,
    },

    /// Audio buffer committed
    #[serde(rename = "input_audio_buffer.committed")]
    InputAudioBufferCommitted {
        /// Previous item ID
        previous_item_id: Option<String>,
        /// New item ID
        item_id: String,
    },

    /// Audio buffer cleared
    #[serde(rename = "input_audio_buffer.cleared")]
    InputAudioBufferCleared,

    /// Conversation item created
    #[serde(rename = "conversation.item.created")]
    ConversationItemCreated {
        /// Previous item ID
        previous_item_id: Option<String>,
        /// Created item
        item: ConversationItem,
    },

    /// Input audio transcription completed
    #[serde(rename = "conversation.item.input_audio_transcription.completed")]
    TranscriptionCompleted {
        /// Item ID
        item_id: String,
        /// Content index
        content_index: u32,
        /// Transcript text
        transcript: String,
    },

    /// Input audio transcription failed
    #[serde(rename = "conversation.item.input_audio_transcription.failed")]
    TranscriptionFailed {
        /// Item ID
        item_id: String,
        /// Content index
        content_index: u32,
        /// Error details
        error: ApiError,
    },

    /// Conversation item truncated
    #[serde(rename = "conversation.item.truncated")]
    ConversationItemTruncated {
        /// Item ID
        item_id: String,
        /// Content index
        content_index: u32,
        /// Audio end in ms
        audio_end_ms: u32,
    },

    /// Conversation item deleted
    #[serde(rename = "conversation.item.deleted")]
    ConversationItemDeleted {
        /// Item ID
        item_id: String,
    },

    /// Response created
    #[serde(rename = "response.created")]
    ResponseCreated {
        /// Response information
        response: Response,
    },

    /// Response done
    #[serde(rename = "response.done")]
    ResponseDone {
        /// Response information
        response: Response,
    },

    /// Output item added to response
    #[serde(rename = "response.output_item.added")]
    OutputItemAdded {
        /// Response ID
        response_id: String,
        /// Output index
        output_index: u32,
        /// Item
        item: ConversationItem,
    },

    /// Output item done
    #[serde(rename = "response.output_item.done")]
    OutputItemDone {
        /// Response ID
        response_id: String,
        /// Output index
        output_index: u32,
        /// Item
        item: ConversationItem,
    },

    /// Content part added
    #[serde(rename = "response.content_part.added")]
    ContentPartAdded {
        /// Response ID
        response_id: String,
        /// Item ID
        item_id: String,
        /// Output index
        output_index: u32,
        /// Content index
        content_index: u32,
        /// Content part
        part: ContentPart,
    },

    /// Content part done
    #[serde(rename = "response.content_part.done")]
    ContentPartDone {
        /// Response ID
        response_id: String,
        /// Item ID
        item_id: String,
        /// Output index
        output_index: u32,
        /// Content index
        content_index: u32,
        /// Content part
        part: ContentPart,
    },

    /// Text delta
    #[serde(rename = "response.text.delta")]
    TextDelta {
        /// Response ID
        response_id: String,
        /// Item ID
        item_id: String,
        /// Output index
        output_index: u32,
        /// Content index
        content_index: u32,
        /// Text delta
        delta: String,
    },

    /// Text done
    #[serde(rename = "response.text.done")]
    TextDone {
        /// Response ID
        response_id: String,
        /// Item ID
        item_id: String,
        /// Output index
        output_index: u32,
        /// Content index
        content_index: u32,
        /// Full text
        text: String,
    },

    /// Audio transcript delta
    #[serde(rename = "response.audio_transcript.delta")]
    AudioTranscriptDelta {
        /// Response ID
        response_id: String,
        /// Item ID
        item_id: String,
        /// Output index
        output_index: u32,
        /// Content index
        content_index: u32,
        /// Transcript delta
        delta: String,
    },

    /// Audio transcript done
    #[serde(rename = "response.audio_transcript.done")]
    AudioTranscriptDone {
        /// Response ID
        response_id: String,
        /// Item ID
        item_id: String,
        /// Output index
        output_index: u32,
        /// Content index
        content_index: u32,
        /// Full transcript
        transcript: String,
    },

    /// Audio delta (audio data chunk)
    #[serde(rename = "response.audio.delta")]
    AudioDelta {
        /// Response ID
        response_id: String,
        /// Item ID
        item_id: String,
        /// Output index
        output_index: u32,
        /// Content index
        content_index: u32,
        /// Base64-encoded audio delta
        delta: String,
    },

    /// Audio done
    #[serde(rename = "response.audio.done")]
    AudioDone {
        /// Response ID
        response_id: String,
        /// Item ID
        item_id: String,
        /// Output index
        output_index: u32,
        /// Content index
        content_index: u32,
    },

    /// Function call arguments delta
    #[serde(rename = "response.function_call_arguments.delta")]
    FunctionCallArgumentsDelta {
        /// Response ID
        response_id: String,
        /// Item ID
        item_id: String,
        /// Output index
        output_index: u32,
        /// Call ID
        call_id: String,
        /// Arguments delta
        delta: String,
    },

    /// Function call arguments done
    #[serde(rename = "response.function_call_arguments.done")]
    FunctionCallArgumentsDone {
        /// Response ID
        response_id: String,
        /// Item ID
        item_id: String,
        /// Output index
        output_index: u32,
        /// Call ID
        call_id: String,
        /// Full arguments
        arguments: String,
    },

    /// Rate limits updated
    #[serde(rename = "rate_limits.updated")]
    RateLimitsUpdated {
        /// Rate limit information
        rate_limits: Vec<RateLimit>,
    },
}

impl ServerEvent {
    /// Decode base64 audio from an AudioDelta event.
    pub fn decode_audio_delta(delta: &str) -> Result<Vec<u8>, base64::DecodeError> {
        BASE64_STANDARD.decode(delta)
    }
}

// =============================================================================
// Supporting Types
// =============================================================================

/// API error information.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ApiError {
    /// Error type
    #[serde(rename = "type")]
    pub error_type: String,
    /// Error code
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// Error message
    pub message: String,
    /// Parameter that caused the error
    #[serde(skip_serializing_if = "Option::is_none")]
    pub param: Option<String>,
    /// Event ID that caused the error
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,
}

/// Session information.
#[derive(Debug, Clone, Deserialize)]
pub struct Session {
    /// Session ID
    pub id: String,
    /// Object type
    pub object: String,
    /// Model used
    pub model: String,
    /// Expires at timestamp
    pub expires_at: u64,
    /// Response modalities
    #[serde(default)]
    pub modalities: Vec<String>,
    /// System instructions
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    /// Voice
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
    /// Input audio format
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_audio_format: Option<String>,
    /// Output audio format
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_audio_format: Option<String>,
    /// Input audio transcription config
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_audio_transcription: Option<InputAudioTranscription>,
    /// Turn detection config
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_detection: Option<TurnDetection>,
    /// Tools
    #[serde(default)]
    pub tools: Vec<ToolDef>,
    /// Tool choice
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<String>,
    /// Temperature
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Max response output tokens
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_response_output_tokens: Option<serde_json::Value>,
}

/// Response information.
#[derive(Debug, Clone, Deserialize)]
pub struct Response {
    /// Response ID
    pub id: String,
    /// Object type
    pub object: String,
    /// Response status
    pub status: String,
    /// Status details
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_details: Option<serde_json::Value>,
    /// Output items
    #[serde(default)]
    pub output: Vec<ConversationItem>,
    /// Usage information
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// Metadata
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

/// Usage information.
#[derive(Debug, Clone, Deserialize)]
pub struct Usage {
    /// Total tokens
    pub total_tokens: u32,
    /// Input tokens
    pub input_tokens: u32,
    /// Output tokens
    pub output_tokens: u32,
    /// Input token details
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_token_details: Option<TokenDetails>,
    /// Output token details
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_token_details: Option<TokenDetails>,
}

/// Token usage details.
#[derive(Debug, Clone, Deserialize)]
pub struct TokenDetails {
    /// Cached tokens
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_tokens: Option<u32>,
    /// Text tokens
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_tokens: Option<u32>,
    /// Audio tokens
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_tokens: Option<u32>,
}

/// Rate limit information.
#[derive(Debug, Clone, Deserialize)]
pub struct RateLimit {
    /// Rate limit name
    pub name: String,
    /// Limit value
    pub limit: u32,
    /// Remaining value
    pub remaining: u32,
    /// Reset timestamp
    pub reset_seconds: f64,
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_event_serialization() {
        let event = ClientEvent::InputAudioBufferCommit;
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("input_audio_buffer.commit"));
    }

    #[test]
    fn test_audio_append() {
        let data = vec![0u8, 1, 2, 3];
        let event = ClientEvent::audio_append(&data);
        match event {
            ClientEvent::InputAudioBufferAppend { audio } => {
                let decoded = BASE64_STANDARD.decode(&audio).unwrap();
                assert_eq!(decoded, data);
            }
            _ => panic!("Wrong event type"),
        }
    }

    #[test]
    fn test_session_update_serialization() {
        let event = ClientEvent::SessionUpdate {
            session: SessionConfig {
                modalities: Some(vec!["text".to_string(), "audio".to_string()]),
                voice: Some("alloy".to_string()),
                instructions: None,
                input_audio_format: None,
                output_audio_format: None,
                input_audio_transcription: None,
                turn_detection: None,
                tools: None,
                tool_choice: None,
                temperature: None,
                max_response_output_tokens: None,
            },
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("session.update"));
        assert!(json.contains("alloy"));
    }

    #[test]
    fn test_server_event_deserialization() {
        let json = r#"{
            "type": "error",
            "error": {
                "type": "invalid_request_error",
                "message": "Test error"
            }
        }"#;
        let event: ServerEvent = serde_json::from_str(json).unwrap();
        match event {
            ServerEvent::Error { error } => {
                assert_eq!(error.message, "Test error");
            }
            _ => panic!("Wrong event type"),
        }
    }

    #[test]
    fn test_audio_delta_decode() {
        let original = vec![0u8, 1, 2, 3, 4, 5];
        let encoded = BASE64_STANDARD.encode(&original);
        let decoded = ServerEvent::decode_audio_delta(&encoded).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn test_response_create_serialization() {
        let event = ClientEvent::ResponseCreate { response: None };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("response.create"));
    }
}
