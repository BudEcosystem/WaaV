//! Phonexia STT Messages
//!
//! Message types for Phonexia WebSocket and REST API communication.

use serde::{Deserialize, Serialize};

// =============================================================================
// Server Messages
// =============================================================================

/// Server message from Phonexia
///
/// Uses custom deserialization to handle different message types based on field presence.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ServerMessage {
    /// Error message (has 'error' field or 'code' with 'message')
    Error(PhonexiaError),
    /// Status message (has 'status' or 'stream_id' without segments)
    Status(StatusMessage),
    /// Transcription result (has 'segments' or 'is_last' or transcription data)
    Result(PhonexiaResult),
}

impl ServerMessage {
    /// Parse a JSON string into a ServerMessage with smart type detection
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        // Use a helper to determine message type based on field presence
        let value: serde_json::Value = serde_json::from_str(json)?;

        // Check for error indicators
        if value.get("code").is_some() && value.get("message").is_some() {
            let error: PhonexiaError = serde_json::from_value(value)?;
            return Ok(Self::Error(error));
        }

        // Check for status indicators (has status/stream_id but no segments)
        let has_status = value.get("status").is_some()
            || value.get("type").is_some()
            || value.get("stream_id").is_some();
        let has_segments = value.get("segments").is_some()
            || value.get("one_best").is_some()
            || value.get("n_best").is_some()
            || value.get("confusion_network").is_some();

        if has_status && !has_segments {
            let status: StatusMessage = serde_json::from_value(value)?;
            return Ok(Self::Status(status));
        }

        // Default to transcription result
        let result: PhonexiaResult = serde_json::from_value(value)?;
        Ok(Self::Result(result))
    }

    /// Check if this is the last result
    pub fn is_last(&self) -> bool {
        match self {
            Self::Result(result) => result.is_last,
            Self::Error(_) => true,
            Self::Status(status) => status.is_last.unwrap_or(false),
        }
    }
}

// =============================================================================
// Result Messages
// =============================================================================

/// Transcription result from Phonexia
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhonexiaResult {
    /// Whether this is the final result
    #[serde(default)]
    pub is_last: bool,

    /// Transcription segments
    #[serde(default)]
    pub segments: Vec<TranscriptSegment>,

    /// One-best transcription
    pub one_best: Option<OneBestResult>,

    /// N-best alternatives
    pub n_best: Option<Vec<NBestAlternative>>,

    /// Confusion network
    pub confusion_network: Option<ConfusionNetworkResult>,

    /// Detected language
    pub language: Option<String>,

    /// Processing time in seconds
    pub processing_time: Option<f64>,

    /// Audio duration in seconds
    pub audio_duration: Option<f64>,
}

impl PhonexiaResult {
    /// Get the primary transcript text
    pub fn text(&self) -> String {
        // Try one_best first
        if let Some(ref one_best) = self.one_best {
            return one_best.text();
        }

        // Try n_best (get first alternative)
        if let Some(ref n_best) = self.n_best {
            if let Some(first) = n_best.first() {
                return first.text.clone();
            }
        }

        // Fall back to segments
        self.segments
            .iter()
            .flat_map(|s| s.words.iter())
            .map(|w| w.text.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Get average confidence score
    pub fn average_confidence(&self) -> f64 {
        if let Some(ref one_best) = self.one_best {
            return one_best.average_confidence();
        }

        if let Some(ref n_best) = self.n_best {
            if let Some(first) = n_best.first() {
                return first.confidence.unwrap_or(0.0);
            }
        }

        let words: Vec<&TranscriptWord> =
            self.segments.iter().flat_map(|s| s.words.iter()).collect();

        if words.is_empty() {
            return 0.0;
        }

        let sum: f64 = words.iter().filter_map(|w| w.confidence).sum();
        sum / words.len() as f64
    }

    /// Get word count
    pub fn word_count(&self) -> usize {
        if let Some(ref one_best) = self.one_best {
            return one_best.word_count();
        }

        self.segments.iter().map(|s| s.words.len()).sum()
    }
}

/// One-best transcription result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OneBestResult {
    /// Transcript segments
    #[serde(default)]
    pub segments: Vec<OneBestSegment>,
}

impl OneBestResult {
    /// Get full text
    pub fn text(&self) -> String {
        self.segments
            .iter()
            .flat_map(|s| s.words.iter())
            .map(|w| w.text.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Get average confidence
    pub fn average_confidence(&self) -> f64 {
        let words: Vec<&TranscriptWord> =
            self.segments.iter().flat_map(|s| s.words.iter()).collect();

        if words.is_empty() {
            return 0.0;
        }

        let sum: f64 = words.iter().filter_map(|w| w.confidence).sum();
        sum / words.len() as f64
    }

    /// Get word count
    pub fn word_count(&self) -> usize {
        self.segments.iter().map(|s| s.words.len()).sum()
    }
}

/// One-best segment
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OneBestSegment {
    /// Words in this segment
    #[serde(default)]
    pub words: Vec<TranscriptWord>,
}

/// N-best alternative
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NBestAlternative {
    /// Rank (1 = best)
    pub rank: Option<u32>,

    /// Transcript text
    pub text: String,

    /// Confidence score
    pub confidence: Option<f64>,

    /// Words with timestamps
    pub words: Option<Vec<TranscriptWord>>,
}

/// Confusion network result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfusionNetworkResult {
    /// Network nodes (sets of alternatives at each position)
    pub nodes: Vec<ConfusionNetworkNode>,
}

/// Confusion network node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfusionNetworkNode {
    /// Alternative words at this position
    pub alternatives: Vec<ConfusionNetworkAlternative>,

    /// Start time in seconds
    pub start_time: Option<f64>,

    /// End time in seconds
    pub end_time: Option<f64>,
}

/// Confusion network alternative
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfusionNetworkAlternative {
    /// Word text (empty for null/silence)
    pub text: String,

    /// Probability (0.0-1.0)
    pub probability: f64,

    /// Item type
    pub item_type: Option<ItemType>,
}

/// Item type in confusion network
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemType {
    /// Regular word
    Word,
    /// Silence
    Silence,
    /// Segment marker
    SegmentMarker,
    /// Null alternative (deletion)
    Null,
}

// =============================================================================
// Transcript Types
// =============================================================================

/// Transcript segment (a continuous speech segment)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptSegment {
    /// Words in this segment
    #[serde(default)]
    pub words: Vec<TranscriptWord>,

    /// Start time in seconds
    pub start_time: Option<f64>,

    /// End time in seconds
    pub end_time: Option<f64>,

    /// Speaker ID (if diarization enabled)
    pub speaker: Option<String>,
}

/// Individual word in transcript
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptWord {
    /// Word text
    pub text: String,

    /// Start time in seconds
    #[serde(alias = "start")]
    pub start_time: Option<f64>,

    /// End time in seconds
    #[serde(alias = "end")]
    pub end_time: Option<f64>,

    /// Confidence score (0.0-1.0)
    pub confidence: Option<f64>,
}

// =============================================================================
// Error Messages
// =============================================================================

/// Error message from Phonexia
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhonexiaError {
    /// Error code
    pub code: Option<u32>,

    /// Error message
    pub message: String,

    /// Error details
    pub details: Option<String>,

    /// API version
    pub version: Option<String>,
}

impl std::fmt::Display for PhonexiaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(code) = self.code {
            write!(f, "[{}] ", code)?;
        }
        write!(f, "{}", self.message)?;
        if let Some(ref details) = self.details {
            write!(f, ": {}", details)?;
        }
        Ok(())
    }
}

impl std::error::Error for PhonexiaError {}

/// Common Phonexia error codes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhonexiaErrorCode {
    /// Missing required parameters
    MissingParameters = 1003,
    /// Invalid parameter values
    InvalidValues = 1004,
    /// Authentication required
    AuthRequired = 1005,
    /// Permission denied
    PermissionDenied = 1006,
    /// Resource not found
    NotFound = 1007,
    /// Server error
    ServerError = 1500,
    /// Unknown error
    Unknown = 0,
}

impl PhonexiaErrorCode {
    /// Create from numeric code
    pub fn from_code(code: u32) -> Self {
        match code {
            1003 => Self::MissingParameters,
            1004 => Self::InvalidValues,
            1005 => Self::AuthRequired,
            1006 => Self::PermissionDenied,
            1007 => Self::NotFound,
            1500 => Self::ServerError,
            _ => Self::Unknown,
        }
    }

    /// Get human-readable description
    pub fn description(&self) -> &'static str {
        match self {
            Self::MissingParameters => "Missing required parameters",
            Self::InvalidValues => "Invalid parameter values",
            Self::AuthRequired => "Authentication required",
            Self::PermissionDenied => "Permission denied",
            Self::NotFound => "Resource not found",
            Self::ServerError => "Internal server error",
            Self::Unknown => "Unknown error",
        }
    }

    /// Check if error is retryable
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::ServerError | Self::Unknown)
    }
}

// =============================================================================
// Status Messages
// =============================================================================

/// Status message from server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusMessage {
    /// Status type
    #[serde(rename = "type", alias = "status")]
    pub status_type: Option<String>,

    /// Message text
    pub message: Option<String>,

    /// Whether this is the last message
    pub is_last: Option<bool>,

    /// Stream ID
    pub stream_id: Option<String>,
}

// =============================================================================
// WebSocket Close Codes
// =============================================================================

/// WebSocket close codes for Phonexia
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhonexiaCloseCode {
    /// Normal closure
    Normal,
    /// Authentication failed
    Unauthorized,
    /// Bad request (invalid parameters)
    BadRequest,
    /// Stream timeout (no data sent)
    Timeout,
    /// Server error
    ServerError,
    /// Unknown error
    Unknown(u16),
}

impl PhonexiaCloseCode {
    /// Create from numeric code
    pub fn from_code(code: u16) -> Self {
        match code {
            1000 => Self::Normal,
            1008 | 4001 => Self::Unauthorized,
            4000 | 4002 => Self::BadRequest,
            4003 => Self::Timeout,
            1011 | 4500 => Self::ServerError,
            _ => Self::Unknown(code),
        }
    }

    /// Get human-readable description
    pub fn description(&self) -> &'static str {
        match self {
            Self::Normal => "Connection closed normally",
            Self::Unauthorized => "Authentication failed or unauthorized",
            Self::BadRequest => "Invalid request parameters",
            Self::Timeout => "Stream timeout - no data received",
            Self::ServerError => "Internal server error",
            Self::Unknown(_) => "Unknown error",
        }
    }

    /// Check if connection should be retried
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Timeout | Self::ServerError | Self::Unknown(_))
    }
}

// =============================================================================
// Login Response
// =============================================================================

/// Login response from Phonexia REST API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginResponse {
    /// Session token
    #[serde(alias = "sessionId", alias = "session_id")]
    pub token: Option<String>,

    /// User information
    pub user: Option<UserInfo>,

    /// Success status
    pub success: Option<bool>,

    /// Error message if login failed
    pub error: Option<String>,
}

/// User information from login
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInfo {
    /// Username
    pub username: Option<String>,

    /// User role
    pub role: Option<String>,

    /// Permissions
    pub permissions: Option<Vec<String>>,
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_phonexia_result_text_one_best() {
        let result = PhonexiaResult {
            is_last: true,
            segments: vec![],
            one_best: Some(OneBestResult {
                segments: vec![OneBestSegment {
                    words: vec![
                        TranscriptWord {
                            text: "hello".to_string(),
                            start_time: Some(0.0),
                            end_time: Some(0.5),
                            confidence: Some(0.95),
                        },
                        TranscriptWord {
                            text: "world".to_string(),
                            start_time: Some(0.6),
                            end_time: Some(1.0),
                            confidence: Some(0.90),
                        },
                    ],
                }],
            }),
            n_best: None,
            confusion_network: None,
            language: Some("en-US".to_string()),
            processing_time: Some(0.1),
            audio_duration: Some(1.0),
        };

        assert_eq!(result.text(), "hello world");
        assert!(result.average_confidence() > 0.9);
        assert_eq!(result.word_count(), 2);
    }

    #[test]
    fn test_phonexia_result_text_segments() {
        let result = PhonexiaResult {
            is_last: false,
            segments: vec![TranscriptSegment {
                words: vec![
                    TranscriptWord {
                        text: "test".to_string(),
                        start_time: Some(0.0),
                        end_time: Some(0.3),
                        confidence: Some(0.85),
                    },
                    TranscriptWord {
                        text: "message".to_string(),
                        start_time: Some(0.4),
                        end_time: Some(0.8),
                        confidence: Some(0.88),
                    },
                ],
                start_time: Some(0.0),
                end_time: Some(0.8),
                speaker: None,
            }],
            one_best: None,
            n_best: None,
            confusion_network: None,
            language: None,
            processing_time: None,
            audio_duration: None,
        };

        assert_eq!(result.text(), "test message");
    }

    #[test]
    fn test_phonexia_result_text_n_best() {
        let result = PhonexiaResult {
            is_last: true,
            segments: vec![],
            one_best: None,
            n_best: Some(vec![
                NBestAlternative {
                    rank: Some(1),
                    text: "first alternative".to_string(),
                    confidence: Some(0.92),
                    words: None,
                },
                NBestAlternative {
                    rank: Some(2),
                    text: "second alternative".to_string(),
                    confidence: Some(0.85),
                    words: None,
                },
            ]),
            confusion_network: None,
            language: None,
            processing_time: None,
            audio_duration: None,
        };

        assert_eq!(result.text(), "first alternative");
        assert!((result.average_confidence() - 0.92).abs() < 0.001);
    }

    #[test]
    fn test_server_message_from_json_result() {
        let json = r#"{
            "is_last": true,
            "segments": [{
                "words": [{"text": "hello", "confidence": 0.95}]
            }]
        }"#;

        let msg = ServerMessage::from_json(json).unwrap();
        match msg {
            ServerMessage::Result(result) => {
                assert!(result.is_last);
                assert_eq!(result.segments.len(), 1);
            }
            _ => panic!("Expected Result message"),
        }
    }

    #[test]
    fn test_server_message_from_json_error() {
        let json = r#"{
            "code": 1005,
            "message": "Authentication required"
        }"#;

        let msg = ServerMessage::from_json(json).unwrap();
        match msg {
            ServerMessage::Error(error) => {
                assert_eq!(error.code, Some(1005));
                assert!(error.message.contains("Authentication"));
            }
            _ => panic!("Expected Error message"),
        }
    }

    #[test]
    fn test_phonexia_error_display() {
        let error = PhonexiaError {
            code: Some(1005),
            message: "Authentication required".to_string(),
            details: Some("Invalid credentials".to_string()),
            version: Some("3.0.0".to_string()),
        };

        let display = format!("{}", error);
        assert!(display.contains("[1005]"));
        assert!(display.contains("Authentication required"));
        assert!(display.contains("Invalid credentials"));
    }

    #[test]
    fn test_phonexia_error_code_from_code() {
        assert_eq!(
            PhonexiaErrorCode::from_code(1003),
            PhonexiaErrorCode::MissingParameters
        );
        assert_eq!(
            PhonexiaErrorCode::from_code(1004),
            PhonexiaErrorCode::InvalidValues
        );
        assert_eq!(
            PhonexiaErrorCode::from_code(1005),
            PhonexiaErrorCode::AuthRequired
        );
        assert_eq!(
            PhonexiaErrorCode::from_code(9999),
            PhonexiaErrorCode::Unknown
        );
    }

    #[test]
    fn test_phonexia_error_code_retryable() {
        assert!(!PhonexiaErrorCode::MissingParameters.is_retryable());
        assert!(!PhonexiaErrorCode::AuthRequired.is_retryable());
        assert!(PhonexiaErrorCode::ServerError.is_retryable());
        assert!(PhonexiaErrorCode::Unknown.is_retryable());
    }

    #[test]
    fn test_phonexia_close_code() {
        assert_eq!(PhonexiaCloseCode::from_code(1000), PhonexiaCloseCode::Normal);
        assert_eq!(
            PhonexiaCloseCode::from_code(4001),
            PhonexiaCloseCode::Unauthorized
        );
        assert_eq!(
            PhonexiaCloseCode::from_code(4003),
            PhonexiaCloseCode::Timeout
        );
        assert!(PhonexiaCloseCode::Normal.description().contains("normally"));
    }

    #[test]
    fn test_phonexia_close_code_retryable() {
        assert!(!PhonexiaCloseCode::Normal.is_retryable());
        assert!(!PhonexiaCloseCode::Unauthorized.is_retryable());
        assert!(PhonexiaCloseCode::Timeout.is_retryable());
        assert!(PhonexiaCloseCode::ServerError.is_retryable());
    }

    #[test]
    fn test_server_message_is_last() {
        let result = ServerMessage::Result(PhonexiaResult {
            is_last: true,
            segments: vec![],
            one_best: None,
            n_best: None,
            confusion_network: None,
            language: None,
            processing_time: None,
            audio_duration: None,
        });
        assert!(result.is_last());

        let error = ServerMessage::Error(PhonexiaError {
            code: Some(1005),
            message: "Error".to_string(),
            details: None,
            version: None,
        });
        assert!(error.is_last());

        let status = ServerMessage::Status(StatusMessage {
            status_type: Some("connected".to_string()),
            message: None,
            is_last: Some(false),
            stream_id: Some("stream-123".to_string()),
        });
        assert!(!status.is_last());
    }

    #[test]
    fn test_login_response() {
        let json = r#"{
            "token": "session-token-123",
            "user": {
                "username": "admin",
                "role": "administrator"
            },
            "success": true
        }"#;

        let response: LoginResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.token, Some("session-token-123".to_string()));
        assert!(response.success.unwrap());
        assert_eq!(
            response.user.as_ref().unwrap().username,
            Some("admin".to_string())
        );
    }

    #[test]
    fn test_confusion_network() {
        let json = r#"{
            "is_last": true,
            "confusion_network": {
                "nodes": [{
                    "alternatives": [
                        {"text": "hello", "probability": 0.9, "item_type": "word"},
                        {"text": "halo", "probability": 0.1, "item_type": "word"}
                    ],
                    "start_time": 0.0,
                    "end_time": 0.5
                }]
            }
        }"#;

        let result: PhonexiaResult = serde_json::from_str(json).unwrap();
        assert!(result.confusion_network.is_some());

        let cn = result.confusion_network.unwrap();
        assert_eq!(cn.nodes.len(), 1);
        assert_eq!(cn.nodes[0].alternatives.len(), 2);
        assert_eq!(cn.nodes[0].alternatives[0].text, "hello");
        assert!((cn.nodes[0].alternatives[0].probability - 0.9).abs() < 0.001);
    }
}
