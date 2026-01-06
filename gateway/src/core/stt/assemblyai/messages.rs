//! WebSocket message types for AssemblyAI Streaming STT API v3.
//!
//! This module contains all message types for communication with the
//! AssemblyAI WebSocket API, including:
//!
//! - **Incoming messages**: Messages received from server
//!   - [`BeginMessage`]: Session initialization with ID and expiration
//!   - [`TurnMessage`]: Transcription results with words and end-of-turn indicator
//!   - [`TerminationMessage`]: Session termination notification
//!   - [`ErrorMessage`]: Error responses
//!
//! - **Outgoing messages**: Messages sent to server
//!   - Binary audio data (sent directly, no JSON wrapper)
//!   - [`TerminateMessage`]: Request to terminate session
//!   - [`ForceEndpointMessage`]: Force current utterance to end
//!   - [`UpdateConfigurationMessage`]: Update session configuration

use serde::{Deserialize, Serialize};

// =============================================================================
// Incoming Messages (Server to Client)
// =============================================================================

/// Begin message received when session is established.
///
/// Contains the session ID and expiration time.
#[derive(Debug, Clone, Deserialize)]
pub struct BeginMessage {
    /// Message type identifier ("Begin")
    #[serde(rename = "type")]
    pub message_type: String,
    /// Unique session identifier
    pub id: String,
    /// Session expiration timestamp (Unix epoch seconds)
    pub expires_at: i64,
}

/// Word timing information for a transcribed word.
#[derive(Debug, Clone, Deserialize)]
pub struct Word {
    /// Start time in milliseconds from the beginning of the audio stream
    pub start: u64,
    /// End time in milliseconds from the beginning of the audio stream
    pub end: u64,
    /// Confidence score for this word (0.0 to 1.0)
    pub confidence: f64,
    /// The transcribed word text
    pub text: String,
}

/// Turn message containing transcription results.
///
/// In AssemblyAI's v3 API, transcripts are delivered in "turns" which
/// represent complete utterances. Unlike other providers, AssemblyAI
/// transcripts are immutable - once delivered, they won't be modified.
#[derive(Debug, Clone, Deserialize)]
pub struct TurnMessage {
    /// Message type identifier ("Turn")
    #[serde(rename = "type")]
    pub message_type: String,
    /// Turn order number (starts at 0, increments for each turn)
    pub turn_order: u32,
    /// Complete transcript text for this turn
    pub transcript: String,
    /// Whether this marks the end of the current turn/utterance
    pub end_of_turn: bool,
    /// Word-level transcription details with timing
    #[serde(default)]
    pub words: Vec<Word>,
    /// Detected language (only for multilingual model)
    #[serde(default)]
    pub language: Option<String>,
    /// Language confidence score (only for multilingual model)
    #[serde(default)]
    pub language_confidence: Option<f64>,
}

/// Termination message received when session ends.
///
/// Contains information about why the session was terminated.
#[derive(Debug, Clone, Deserialize)]
pub struct TerminationMessage {
    /// Message type identifier ("Termination")
    #[serde(rename = "type")]
    pub message_type: String,
    /// The audio duration in milliseconds that was processed
    pub audio_duration_ms: u64,
    /// Whether the session was terminated normally
    #[serde(default)]
    pub terminated_normally: bool,
}

/// Error message from AssemblyAI.
///
/// Contains error details when something goes wrong.
#[derive(Debug, Clone, Deserialize)]
pub struct ErrorMessage {
    /// Message type identifier ("Error")
    #[serde(rename = "type")]
    pub message_type: String,
    /// Error code (e.g., "invalid_api_key", "rate_limit_exceeded")
    #[serde(default)]
    pub error_code: Option<String>,
    /// Human-readable error description
    pub error: String,
}

// =============================================================================
// Outgoing Messages (Client to Server)
// =============================================================================

/// Terminate message to gracefully end the session.
///
/// Send this when you want to stop transcription and receive
/// any remaining results.
#[derive(Debug, Clone, Serialize)]
pub struct TerminateMessage {
    /// Message type identifier (always "Terminate")
    #[serde(rename = "type")]
    pub message_type: &'static str,
}

impl Default for TerminateMessage {
    fn default() -> Self {
        Self {
            message_type: "Terminate",
        }
    }
}

/// Force endpoint message to manually end the current utterance.
///
/// Use this to force the current speech segment to be finalized
/// and returned as a turn, even if end-of-turn hasn't been
/// automatically detected.
#[derive(Debug, Clone, Serialize)]
pub struct ForceEndpointMessage {
    /// Message type identifier (always "ForceEndpoint")
    #[serde(rename = "type")]
    pub message_type: &'static str,
}

impl Default for ForceEndpointMessage {
    fn default() -> Self {
        Self {
            message_type: "ForceEndpoint",
        }
    }
}

/// Update configuration message to change session settings.
///
/// Allows changing certain parameters during an active session.
#[derive(Debug, Clone, Serialize)]
pub struct UpdateConfigurationMessage {
    /// Message type identifier (always "UpdateConfiguration")
    #[serde(rename = "type")]
    pub message_type: &'static str,
    /// New end-of-turn confidence threshold (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_of_turn_confidence_threshold: Option<f32>,
}

impl UpdateConfigurationMessage {
    /// Create a new update configuration message.
    pub fn new(end_of_turn_confidence_threshold: Option<f32>) -> Self {
        Self {
            message_type: "UpdateConfiguration",
            end_of_turn_confidence_threshold,
        }
    }
}

// =============================================================================
// Message Enum and Parsing
// =============================================================================

/// Enum for all possible WebSocket messages from AssemblyAI.
///
/// Use `AssemblyAIMessage::parse()` to deserialize incoming WebSocket messages.
#[derive(Debug)]
pub enum AssemblyAIMessage {
    /// Session initialization complete
    Begin(BeginMessage),
    /// Transcription result (turn)
    Turn(TurnMessage),
    /// Session terminated
    Termination(TerminationMessage),
    /// Error from the server
    Error(ErrorMessage),
    /// Unknown message type (for forward compatibility)
    Unknown(String),
}

impl AssemblyAIMessage {
    /// Parse a WebSocket text message into the appropriate type.
    ///
    /// # Arguments
    /// * `text` - Raw JSON text from WebSocket message
    ///
    /// # Returns
    /// * `Result<Self, serde_json::Error>` - Parsed message or parse error
    pub fn parse(text: &str) -> Result<Self, serde_json::Error> {
        // First, peek at the type field
        #[derive(Deserialize)]
        struct TypePeek {
            #[serde(rename = "type")]
            message_type: String,
        }

        let peek: TypePeek = serde_json::from_str(text)?;

        match peek.message_type.as_str() {
            "Begin" => {
                let msg: BeginMessage = serde_json::from_str(text)?;
                Ok(AssemblyAIMessage::Begin(msg))
            }
            "Turn" => {
                let msg: TurnMessage = serde_json::from_str(text)?;
                Ok(AssemblyAIMessage::Turn(msg))
            }
            "Termination" => {
                let msg: TerminationMessage = serde_json::from_str(text)?;
                Ok(AssemblyAIMessage::Termination(msg))
            }
            "Error" => {
                let msg: ErrorMessage = serde_json::from_str(text)?;
                Ok(AssemblyAIMessage::Error(msg))
            }
            _ => Ok(AssemblyAIMessage::Unknown(text.to_string())),
        }
    }

    /// Check if this message represents an error.
    #[inline]
    pub fn is_error(&self) -> bool {
        matches!(self, AssemblyAIMessage::Error(_))
    }

    /// Check if this message contains a final transcript (end_of_turn = true).
    #[inline]
    pub fn is_final_transcript(&self) -> bool {
        matches!(self, AssemblyAIMessage::Turn(turn) if turn.end_of_turn)
    }

    /// Check if this is a termination message.
    #[inline]
    pub fn is_termination(&self) -> bool {
        matches!(self, AssemblyAIMessage::Termination(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_begin_message() {
        let json = r#"{"type":"Begin","id":"session-123","expires_at":1704067200}"#;
        let msg = AssemblyAIMessage::parse(json).unwrap();

        match msg {
            AssemblyAIMessage::Begin(begin) => {
                assert_eq!(begin.id, "session-123");
                assert_eq!(begin.expires_at, 1704067200);
            }
            _ => panic!("Expected Begin message"),
        }
    }

    #[test]
    fn test_parse_turn_message() {
        let json = r#"{
            "type": "Turn",
            "turn_order": 0,
            "transcript": "Hello world",
            "end_of_turn": true,
            "words": [
                {"start": 0, "end": 500, "confidence": 0.95, "text": "Hello"},
                {"start": 500, "end": 1000, "confidence": 0.98, "text": "world"}
            ]
        }"#;

        let msg = AssemblyAIMessage::parse(json).unwrap();

        match msg {
            AssemblyAIMessage::Turn(turn) => {
                assert_eq!(turn.turn_order, 0);
                assert_eq!(turn.transcript, "Hello world");
                assert!(turn.end_of_turn);
                assert_eq!(turn.words.len(), 2);
                assert_eq!(turn.words[0].text, "Hello");
                assert_eq!(turn.words[1].text, "world");
            }
            _ => panic!("Expected Turn message"),
        }
    }

    #[test]
    fn test_parse_turn_message_with_language() {
        let json = r#"{
            "type": "Turn",
            "turn_order": 1,
            "transcript": "Bonjour monde",
            "end_of_turn": false,
            "words": [],
            "language": "fr",
            "language_confidence": 0.92
        }"#;

        let msg = AssemblyAIMessage::parse(json).unwrap();

        match msg {
            AssemblyAIMessage::Turn(turn) => {
                assert_eq!(turn.turn_order, 1);
                assert_eq!(turn.language, Some("fr".to_string()));
                assert!((turn.language_confidence.unwrap() - 0.92).abs() < f64::EPSILON);
                assert!(!turn.end_of_turn);
            }
            _ => panic!("Expected Turn message"),
        }
    }

    #[test]
    fn test_parse_termination_message() {
        let json = r#"{"type":"Termination","audio_duration_ms":5000,"terminated_normally":true}"#;
        let msg = AssemblyAIMessage::parse(json).unwrap();

        match msg {
            AssemblyAIMessage::Termination(term) => {
                assert_eq!(term.audio_duration_ms, 5000);
                assert!(term.terminated_normally);
            }
            _ => panic!("Expected Termination message"),
        }
    }

    #[test]
    fn test_parse_error_message() {
        let json =
            r#"{"type":"Error","error_code":"invalid_api_key","error":"API key is invalid"}"#;
        let msg = AssemblyAIMessage::parse(json).unwrap();

        match msg {
            AssemblyAIMessage::Error(err) => {
                assert_eq!(err.error_code, Some("invalid_api_key".to_string()));
                assert_eq!(err.error, "API key is invalid");
            }
            _ => panic!("Expected Error message"),
        }
    }

    #[test]
    fn test_parse_unknown_message() {
        let json = r#"{"type":"FutureMessageType","data":"something"}"#;
        let msg = AssemblyAIMessage::parse(json).unwrap();

        assert!(matches!(msg, AssemblyAIMessage::Unknown(_)));
    }

    #[test]
    fn test_is_error() {
        let error_json =
            r#"{"type":"Error","error_code":"rate_limit","error":"Rate limit exceeded"}"#;
        let msg = AssemblyAIMessage::parse(error_json).unwrap();
        assert!(msg.is_error());

        let begin_json = r#"{"type":"Begin","id":"test","expires_at":0}"#;
        let msg = AssemblyAIMessage::parse(begin_json).unwrap();
        assert!(!msg.is_error());
    }

    #[test]
    fn test_is_final_transcript() {
        let final_json =
            r#"{"type":"Turn","turn_order":0,"transcript":"test","end_of_turn":true,"words":[]}"#;
        let msg = AssemblyAIMessage::parse(final_json).unwrap();
        assert!(msg.is_final_transcript());

        let partial_json =
            r#"{"type":"Turn","turn_order":0,"transcript":"test","end_of_turn":false,"words":[]}"#;
        let msg = AssemblyAIMessage::parse(partial_json).unwrap();
        assert!(!msg.is_final_transcript());
    }

    #[test]
    fn test_terminate_message_serialization() {
        let msg = TerminateMessage::default();
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"Terminate"}"#);
    }

    #[test]
    fn test_force_endpoint_message_serialization() {
        let msg = ForceEndpointMessage::default();
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"ForceEndpoint"}"#);
    }

    #[test]
    fn test_update_configuration_message_serialization() {
        let msg = UpdateConfigurationMessage::new(Some(0.7));
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"type\":\"UpdateConfiguration\""));
        assert!(json.contains("\"end_of_turn_confidence_threshold\":0.7"));

        let msg_none = UpdateConfigurationMessage::new(None);
        let json_none = serde_json::to_string(&msg_none).unwrap();
        assert!(!json_none.contains("end_of_turn_confidence_threshold"));
    }

    #[test]
    fn test_word_timing() {
        let json = r#"{"start":100,"end":500,"confidence":0.95,"text":"hello"}"#;
        let word: Word = serde_json::from_str(json).unwrap();

        assert_eq!(word.start, 100);
        assert_eq!(word.end, 500);
        assert!((word.confidence - 0.95).abs() < f64::EPSILON);
        assert_eq!(word.text, "hello");
    }
}
