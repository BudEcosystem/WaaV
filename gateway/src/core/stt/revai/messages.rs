//! Rev AI WebSocket Message Types
//!
//! Message structures for Rev AI streaming speech-to-text WebSocket protocol.

use serde::{Deserialize, Serialize};

// =============================================================================
// Close Codes
// =============================================================================

/// Rev AI WebSocket close codes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevAICloseCode {
    /// Normal closure
    Normal,
    /// Invalid or missing access token
    Unauthorized,
    /// Invalid content_type, metadata too long, or invalid custom vocabulary
    BadRequest,
    /// Not enough credits (requires 10 min hold)
    InsufficientCredits,
    /// Server shutting down, retry connection
    ServerShuttingDown,
    /// No streaming instances available, retry connection
    NoInstanceAvailable,
    /// Exceeded concurrency limit
    TooManyRequests,
    /// Unknown close code
    Unknown(u16),
}

impl RevAICloseCode {
    /// Parse close code from u16
    pub fn from_code(code: u16) -> Self {
        match code {
            1000 => Self::Normal,
            4001 => Self::Unauthorized,
            4002 => Self::BadRequest,
            4003 => Self::InsufficientCredits,
            4010 => Self::ServerShuttingDown,
            4013 => Self::NoInstanceAvailable,
            4029 => Self::TooManyRequests,
            other => Self::Unknown(other),
        }
    }

    /// Get the numeric code
    pub fn code(&self) -> u16 {
        match self {
            Self::Normal => 1000,
            Self::Unauthorized => 4001,
            Self::BadRequest => 4002,
            Self::InsufficientCredits => 4003,
            Self::ServerShuttingDown => 4010,
            Self::NoInstanceAvailable => 4013,
            Self::TooManyRequests => 4029,
            Self::Unknown(code) => *code,
        }
    }

    /// Check if the error is retryable
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::ServerShuttingDown | Self::NoInstanceAvailable)
    }

    /// Get a human-readable description
    pub fn description(&self) -> &'static str {
        match self {
            Self::Normal => "Normal closure",
            Self::Unauthorized => "Invalid or missing access token",
            Self::BadRequest => {
                "Invalid content_type, metadata too long, or invalid custom vocabulary"
            }
            Self::InsufficientCredits => "Insufficient credits (requires 10 minute hold)",
            Self::ServerShuttingDown => "Server shutting down, retry connection",
            Self::NoInstanceAvailable => "No streaming instances available, retry connection",
            Self::TooManyRequests => "Exceeded concurrency limit",
            Self::Unknown(_) => "Unknown error",
        }
    }
}

// =============================================================================
// Connected Message
// =============================================================================

/// Initial connection acknowledgment message
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ConnectedMessage {
    /// Message type, always "connected"
    #[serde(rename = "type")]
    pub message_type: String,

    /// Unique session identifier
    pub id: String,
}

impl ConnectedMessage {
    /// Check if this is a valid connected message
    pub fn is_valid(&self) -> bool {
        self.message_type == "connected" && !self.id.is_empty()
    }
}

// =============================================================================
// Transcript Element
// =============================================================================

/// Element type in transcripts
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ElementType {
    /// Recognized word
    Text,
    /// Punctuation or whitespace
    Punct,
}

/// Individual element in a transcript
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TranscriptElement {
    /// Element type (text or punct)
    #[serde(rename = "type")]
    pub element_type: ElementType,

    /// The word or punctuation value
    pub value: String,

    /// Start timestamp in seconds (text elements only)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ts: Option<f64>,

    /// End timestamp in seconds (text elements only)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_ts: Option<f64>,

    /// Recognition confidence 0-1 (text elements only)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
}

impl TranscriptElement {
    /// Check if this is a text element (word)
    pub fn is_text(&self) -> bool {
        self.element_type == ElementType::Text
    }

    /// Check if this is a punctuation element
    pub fn is_punct(&self) -> bool {
        self.element_type == ElementType::Punct
    }

    /// Get the confidence or 0.0 if not available
    pub fn confidence_or_default(&self) -> f64 {
        self.confidence.unwrap_or(0.0)
    }
}

// =============================================================================
// Partial Transcript
// =============================================================================

/// Interim transcription result
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PartialTranscript {
    /// Message type, always "partial"
    #[serde(rename = "type")]
    pub message_type: String,

    /// Start timestamp in seconds
    pub ts: f64,

    /// End timestamp in seconds
    pub end_ts: f64,

    /// Elements in the partial transcript
    #[serde(default)]
    pub elements: Vec<TranscriptElement>,
}

impl PartialTranscript {
    /// Check if this is a valid partial message
    pub fn is_valid(&self) -> bool {
        self.message_type == "partial"
    }

    /// Extract the full text from elements
    pub fn text(&self) -> String {
        self.elements.iter().map(|e| e.value.as_str()).collect()
    }

    /// Get the average confidence of text elements
    pub fn average_confidence(&self) -> f64 {
        let text_elements: Vec<_> = self
            .elements
            .iter()
            .filter(|e| e.is_text() && e.confidence.is_some())
            .collect();

        if text_elements.is_empty() {
            return 0.0;
        }

        let sum: f64 = text_elements.iter().filter_map(|e| e.confidence).sum();

        sum / text_elements.len() as f64
    }

    /// Get duration in seconds
    pub fn duration(&self) -> f64 {
        self.end_ts - self.ts
    }
}

// =============================================================================
// Final Transcript
// =============================================================================

/// Final transcription result
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FinalTranscript {
    /// Message type, always "final"
    #[serde(rename = "type")]
    pub message_type: String,

    /// Start timestamp in seconds
    pub ts: f64,

    /// End timestamp in seconds
    pub end_ts: f64,

    /// Elements in the final transcript
    pub elements: Vec<TranscriptElement>,
}

impl FinalTranscript {
    /// Check if this is a valid final message
    pub fn is_valid(&self) -> bool {
        self.message_type == "final"
    }

    /// Extract the full text from elements
    pub fn text(&self) -> String {
        self.elements.iter().map(|e| e.value.as_str()).collect()
    }

    /// Get only the text elements (words)
    pub fn words(&self) -> Vec<&TranscriptElement> {
        self.elements.iter().filter(|e| e.is_text()).collect()
    }

    /// Get the average confidence of text elements
    pub fn average_confidence(&self) -> f64 {
        let text_elements: Vec<_> = self
            .elements
            .iter()
            .filter(|e| e.is_text() && e.confidence.is_some())
            .collect();

        if text_elements.is_empty() {
            return 0.0;
        }

        let sum: f64 = text_elements.iter().filter_map(|e| e.confidence).sum();

        sum / text_elements.len() as f64
    }

    /// Get duration in seconds
    pub fn duration(&self) -> f64 {
        self.end_ts - self.ts
    }

    /// Get word count
    pub fn word_count(&self) -> usize {
        self.elements.iter().filter(|e| e.is_text()).count()
    }
}

// =============================================================================
// Server Message Enum
// =============================================================================

/// Unified enum for all server messages
#[derive(Debug, Clone)]
pub enum ServerMessage {
    /// Connection established
    Connected(ConnectedMessage),
    /// Interim transcription
    Partial(PartialTranscript),
    /// Final transcription
    Final(FinalTranscript),
}

impl ServerMessage {
    /// Parse a server message from JSON
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        // First, peek at the type field
        #[derive(Deserialize)]
        struct TypePeek {
            #[serde(rename = "type")]
            message_type: String,
        }

        let peek: TypePeek = serde_json::from_str(json)?;

        match peek.message_type.as_str() {
            "connected" => {
                let msg: ConnectedMessage = serde_json::from_str(json)?;
                Ok(ServerMessage::Connected(msg))
            }
            "partial" => {
                let msg: PartialTranscript = serde_json::from_str(json)?;
                Ok(ServerMessage::Partial(msg))
            }
            "final" => {
                let msg: FinalTranscript = serde_json::from_str(json)?;
                Ok(ServerMessage::Final(msg))
            }
            other => Err(serde::de::Error::custom(format!(
                "Unknown message type: {}",
                other
            ))),
        }
    }

    /// Check if this is a connected message
    pub fn is_connected(&self) -> bool {
        matches!(self, Self::Connected(_))
    }

    /// Check if this is a partial transcript
    pub fn is_partial(&self) -> bool {
        matches!(self, Self::Partial(_))
    }

    /// Check if this is a final transcript
    pub fn is_final(&self) -> bool {
        matches!(self, Self::Final(_))
    }

    /// Get the text if this is a transcript message
    pub fn text(&self) -> Option<String> {
        match self {
            Self::Connected(_) => None,
            Self::Partial(p) => Some(p.text()),
            Self::Final(f) => Some(f.text()),
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_close_code_from_code() {
        assert_eq!(RevAICloseCode::from_code(1000), RevAICloseCode::Normal);
        assert_eq!(
            RevAICloseCode::from_code(4001),
            RevAICloseCode::Unauthorized
        );
        assert_eq!(RevAICloseCode::from_code(4002), RevAICloseCode::BadRequest);
        assert_eq!(
            RevAICloseCode::from_code(4003),
            RevAICloseCode::InsufficientCredits
        );
        assert_eq!(
            RevAICloseCode::from_code(4010),
            RevAICloseCode::ServerShuttingDown
        );
        assert_eq!(
            RevAICloseCode::from_code(4013),
            RevAICloseCode::NoInstanceAvailable
        );
        assert_eq!(
            RevAICloseCode::from_code(4029),
            RevAICloseCode::TooManyRequests
        );
        assert_eq!(
            RevAICloseCode::from_code(9999),
            RevAICloseCode::Unknown(9999)
        );
    }

    #[test]
    fn test_close_code_is_retryable() {
        assert!(!RevAICloseCode::Normal.is_retryable());
        assert!(!RevAICloseCode::Unauthorized.is_retryable());
        assert!(!RevAICloseCode::BadRequest.is_retryable());
        assert!(!RevAICloseCode::InsufficientCredits.is_retryable());
        assert!(RevAICloseCode::ServerShuttingDown.is_retryable());
        assert!(RevAICloseCode::NoInstanceAvailable.is_retryable());
        assert!(!RevAICloseCode::TooManyRequests.is_retryable());
    }

    #[test]
    fn test_connected_message_parsing() {
        let json = r#"{"type": "connected", "id": "s1d24ax2fd21"}"#;
        let msg: ConnectedMessage = serde_json::from_str(json).unwrap();

        assert_eq!(msg.message_type, "connected");
        assert_eq!(msg.id, "s1d24ax2fd21");
        assert!(msg.is_valid());
    }

    #[test]
    fn test_partial_transcript_parsing() {
        let json = r#"{
            "type": "partial",
            "ts": 1.01,
            "end_ts": 1.55,
            "elements": [
                {"type": "text", "value": "one"}
            ]
        }"#;

        let msg: PartialTranscript = serde_json::from_str(json).unwrap();

        assert_eq!(msg.message_type, "partial");
        assert_eq!(msg.ts, 1.01);
        assert_eq!(msg.end_ts, 1.55);
        assert_eq!(msg.elements.len(), 1);
        assert_eq!(msg.text(), "one");
        assert!(msg.is_valid());
    }

    #[test]
    fn test_final_transcript_parsing() {
        let json = r#"{
            "type": "final",
            "ts": 1.01,
            "end_ts": 3.2,
            "elements": [
                {"type": "text", "value": "One", "ts": 1.04, "end_ts": 1.55, "confidence": 1.0},
                {"type": "punct", "value": " "},
                {"type": "text", "value": "two", "ts": 1.84, "end_ts": 2.15, "confidence": 0.95},
                {"type": "punct", "value": "."}
            ]
        }"#;

        let msg: FinalTranscript = serde_json::from_str(json).unwrap();

        assert_eq!(msg.message_type, "final");
        assert_eq!(msg.ts, 1.01);
        assert_eq!(msg.end_ts, 3.2);
        assert_eq!(msg.elements.len(), 4);
        assert_eq!(msg.text(), "One two.");
        assert_eq!(msg.word_count(), 2);
        assert!(msg.is_valid());

        // Check average confidence
        let avg_conf = msg.average_confidence();
        assert!(avg_conf > 0.97 && avg_conf < 0.98); // (1.0 + 0.95) / 2 = 0.975
    }

    #[test]
    fn test_transcript_element_text() {
        let element = TranscriptElement {
            element_type: ElementType::Text,
            value: "hello".to_string(),
            ts: Some(1.0),
            end_ts: Some(1.5),
            confidence: Some(0.95),
        };

        assert!(element.is_text());
        assert!(!element.is_punct());
        assert_eq!(element.confidence_or_default(), 0.95);
    }

    #[test]
    fn test_transcript_element_punct() {
        let element = TranscriptElement {
            element_type: ElementType::Punct,
            value: ".".to_string(),
            ts: None,
            end_ts: None,
            confidence: None,
        };

        assert!(!element.is_text());
        assert!(element.is_punct());
        assert_eq!(element.confidence_or_default(), 0.0);
    }

    #[test]
    fn test_server_message_from_json_connected() {
        let json = r#"{"type": "connected", "id": "test123"}"#;
        let msg = ServerMessage::from_json(json).unwrap();

        assert!(msg.is_connected());
        assert!(!msg.is_partial());
        assert!(!msg.is_final());
        assert!(msg.text().is_none());
    }

    #[test]
    fn test_server_message_from_json_partial() {
        let json = r#"{"type": "partial", "ts": 0.0, "end_ts": 1.0, "elements": [{"type": "text", "value": "test"}]}"#;
        let msg = ServerMessage::from_json(json).unwrap();

        assert!(!msg.is_connected());
        assert!(msg.is_partial());
        assert!(!msg.is_final());
        assert_eq!(msg.text(), Some("test".to_string()));
    }

    #[test]
    fn test_server_message_from_json_final() {
        let json = r#"{"type": "final", "ts": 0.0, "end_ts": 1.0, "elements": [{"type": "text", "value": "test", "confidence": 0.9}]}"#;
        let msg = ServerMessage::from_json(json).unwrap();

        assert!(!msg.is_connected());
        assert!(!msg.is_partial());
        assert!(msg.is_final());
        assert_eq!(msg.text(), Some("test".to_string()));
    }

    #[test]
    fn test_server_message_from_json_unknown_type() {
        let json = r#"{"type": "unknown", "data": "test"}"#;
        let result = ServerMessage::from_json(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_partial_transcript_duration() {
        let partial = PartialTranscript {
            message_type: "partial".to_string(),
            ts: 1.0,
            end_ts: 3.5,
            elements: vec![],
        };

        assert_eq!(partial.duration(), 2.5);
    }

    #[test]
    fn test_final_transcript_words() {
        let final_transcript = FinalTranscript {
            message_type: "final".to_string(),
            ts: 0.0,
            end_ts: 2.0,
            elements: vec![
                TranscriptElement {
                    element_type: ElementType::Text,
                    value: "Hello".to_string(),
                    ts: Some(0.0),
                    end_ts: Some(0.5),
                    confidence: Some(0.9),
                },
                TranscriptElement {
                    element_type: ElementType::Punct,
                    value: " ".to_string(),
                    ts: None,
                    end_ts: None,
                    confidence: None,
                },
                TranscriptElement {
                    element_type: ElementType::Text,
                    value: "world".to_string(),
                    ts: Some(0.6),
                    end_ts: Some(1.0),
                    confidence: Some(0.95),
                },
            ],
        };

        let words = final_transcript.words();
        assert_eq!(words.len(), 2);
        assert_eq!(words[0].value, "Hello");
        assert_eq!(words[1].value, "world");
    }

    #[test]
    fn test_close_code_description() {
        assert!(!RevAICloseCode::Normal.description().is_empty());
        assert!(RevAICloseCode::Unauthorized.description().contains("token"));
        assert!(
            RevAICloseCode::InsufficientCredits
                .description()
                .contains("credit")
        );
    }

    #[test]
    fn test_close_code_roundtrip() {
        let codes = vec![
            RevAICloseCode::Normal,
            RevAICloseCode::Unauthorized,
            RevAICloseCode::BadRequest,
            RevAICloseCode::InsufficientCredits,
            RevAICloseCode::ServerShuttingDown,
            RevAICloseCode::NoInstanceAvailable,
            RevAICloseCode::TooManyRequests,
        ];

        for code in codes {
            let num = code.code();
            let parsed = RevAICloseCode::from_code(num);
            assert_eq!(code, parsed);
        }
    }
}
