//! Reverie STT Message Types
//!
//! Message types for WebSocket communication with the Reverie STT API.

use serde::{Deserialize, Serialize};
use std::fmt;

// =============================================================================
// Close Reasons
// =============================================================================

/// Reasons why a Reverie STT session may close
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReverieCloseReason {
    /// Connection timeout reached
    Timeout,
    /// Silence detected after audio input
    SilenceDetected,
    /// End-of-file marker received from client
    EofReceived,
    /// Normal completion
    Complete,
    /// Unknown reason
    Unknown(String),
}

impl ReverieCloseReason {
    /// Parse close reason from string
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "timeout" => Self::Timeout,
            "silence detected" | "silence_detected" => Self::SilenceDetected,
            "eof received" | "eof_received" => Self::EofReceived,
            "complete" => Self::Complete,
            _ => Self::Unknown(s.to_string()),
        }
    }

    /// Check if this is a normal close reason
    pub fn is_normal(&self) -> bool {
        matches!(
            self,
            Self::SilenceDetected | Self::EofReceived | Self::Complete
        )
    }
}

impl fmt::Display for ReverieCloseReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout => write!(f, "timeout"),
            Self::SilenceDetected => write!(f, "silence detected"),
            Self::EofReceived => write!(f, "EOF received"),
            Self::Complete => write!(f, "complete"),
            Self::Unknown(s) => write!(f, "{}", s),
        }
    }
}

// =============================================================================
// ASR Result
// =============================================================================

/// Result from Reverie ASR (Speech-to-Text)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ReverieAsrResult {
    /// Session ID
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,

    /// Transcribed text
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,

    /// Whether this is a final result
    #[serde(default)]
    pub r#final: bool,

    /// Cause of session end (if final)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause: Option<String>,

    /// Whether the transcription was successful
    #[serde(default)]
    pub success: bool,

    /// Confidence score (as string, e.g., "0.95")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<String>,

    /// Display-formatted text (with capitalization)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_text: Option<String>,
}

impl ReverieAsrResult {
    /// Get the close reason if this is a final result
    pub fn close_reason(&self) -> Option<ReverieCloseReason> {
        self.cause.as_ref().map(|c| ReverieCloseReason::from_str(c))
    }

    /// Get confidence as float
    pub fn confidence_f64(&self) -> Option<f64> {
        self.confidence.as_ref().and_then(|c| c.parse().ok())
    }

    /// Get the best available text (display_text or text)
    pub fn best_text(&self) -> Option<&str> {
        self.display_text.as_deref().or(self.text.as_deref())
    }

    /// Check if this is a final result that should close the connection
    pub fn should_close(&self, continuous: bool) -> bool {
        if !self.r#final {
            return false;
        }

        // In continuous mode, only close on EOF or timeout
        if continuous {
            matches!(
                self.close_reason(),
                Some(ReverieCloseReason::EofReceived) | Some(ReverieCloseReason::Timeout)
            )
        } else {
            // Non-continuous: close on any final result
            true
        }
    }
}

// =============================================================================
// Server Messages
// =============================================================================

/// Messages received from the Reverie server
#[derive(Debug, Clone)]
pub enum ReverieServerMessage {
    /// Partial transcription result
    Partial(ReverieAsrResult),
    /// Final transcription result
    Final(ReverieAsrResult),
    /// Error message
    Error(String),
    /// Unknown message type
    Unknown(String),
}

impl ReverieServerMessage {
    /// Parse a JSON message from the server
    pub fn from_json(json: &str) -> Self {
        // First check if this is an error message (has "message" or "error" field but no "text"/"id")
        if let Ok(obj) = serde_json::from_str::<serde_json::Value>(json) {
            // Error messages have "message" or "error" field without standard ASR fields
            let has_message = obj.get("message").is_some();
            let has_error = obj.get("error").is_some();
            let has_text = obj.get("text").is_some();
            let has_id = obj.get("id").is_some();
            let has_final = obj.get("final").is_some();

            if (has_message || has_error) && !has_text && !has_id && !has_final {
                if let Some(msg) = obj.get("message").and_then(|m| m.as_str()) {
                    return Self::Error(msg.to_string());
                }
                if let Some(err) = obj.get("error").and_then(|e| e.as_str()) {
                    return Self::Error(err.to_string());
                }
            }
        }

        // Now try to parse as ASR result
        match serde_json::from_str::<ReverieAsrResult>(json) {
            Ok(result) => {
                if result.r#final {
                    Self::Final(result)
                } else {
                    Self::Partial(result)
                }
            }
            Err(e) => Self::Unknown(format!("Parse error: {} - Raw: {}", e, json)),
        }
    }

    /// Check if this is a final message
    pub fn is_final(&self) -> bool {
        matches!(self, Self::Final(_))
    }

    /// Check if this is an error
    pub fn is_error(&self) -> bool {
        matches!(self, Self::Error(_))
    }

    /// Get the ASR result if available
    pub fn as_result(&self) -> Option<&ReverieAsrResult> {
        match self {
            Self::Partial(r) | Self::Final(r) => Some(r),
            _ => None,
        }
    }

    /// Get the text content
    pub fn text(&self) -> Option<&str> {
        self.as_result().and_then(|r| r.best_text())
    }

    /// Get the confidence score
    pub fn confidence(&self) -> Option<f64> {
        self.as_result().and_then(|r| r.confidence_f64())
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_close_reason_parsing() {
        assert_eq!(
            ReverieCloseReason::from_str("timeout"),
            ReverieCloseReason::Timeout
        );
        assert_eq!(
            ReverieCloseReason::from_str("silence detected"),
            ReverieCloseReason::SilenceDetected
        );
        assert_eq!(
            ReverieCloseReason::from_str("EOF received"),
            ReverieCloseReason::EofReceived
        );
    }

    #[test]
    fn test_close_reason_is_normal() {
        assert!(!ReverieCloseReason::Timeout.is_normal());
        assert!(ReverieCloseReason::SilenceDetected.is_normal());
        assert!(ReverieCloseReason::EofReceived.is_normal());
    }

    #[test]
    fn test_parse_partial_result() {
        let json = r#"{
            "id": "session-123",
            "text": "hello world",
            "final": false,
            "success": true,
            "confidence": "0.85"
        }"#;

        let msg = ReverieServerMessage::from_json(json);
        assert!(matches!(msg, ReverieServerMessage::Partial(_)));
        assert!(!msg.is_final());
        assert_eq!(msg.text(), Some("hello world"));
        assert_eq!(msg.confidence(), Some(0.85));
    }

    #[test]
    fn test_parse_final_result() {
        let json = r#"{
            "id": "session-123",
            "text": "hello world",
            "final": true,
            "cause": "EOF received",
            "success": true,
            "confidence": "0.92",
            "display_text": "Hello World"
        }"#;

        let msg = ReverieServerMessage::from_json(json);
        assert!(matches!(msg, ReverieServerMessage::Final(_)));
        assert!(msg.is_final());

        if let ReverieServerMessage::Final(result) = msg {
            assert_eq!(result.best_text(), Some("Hello World"));
            assert!(matches!(
                result.close_reason(),
                Some(ReverieCloseReason::EofReceived)
            ));
        }
    }

    #[test]
    fn test_parse_error_message() {
        let json = r#"{"message": "Invalid API key"}"#;
        let msg = ReverieServerMessage::from_json(json);
        assert!(matches!(msg, ReverieServerMessage::Error(_)));
        assert!(msg.is_error());
    }

    #[test]
    fn test_should_close_continuous() {
        let mut result = ReverieAsrResult::default();
        result.r#final = true;
        result.cause = Some("silence detected".to_string());

        // In continuous mode, silence doesn't close
        assert!(!result.should_close(true));

        result.cause = Some("EOF received".to_string());
        // EOF closes even in continuous mode
        assert!(result.should_close(true));
    }

    #[test]
    fn test_should_close_non_continuous() {
        let mut result = ReverieAsrResult::default();
        result.r#final = true;
        result.cause = Some("silence detected".to_string());

        // Non-continuous: any final closes
        assert!(result.should_close(false));
    }

    #[test]
    fn test_confidence_parsing() {
        let result = ReverieAsrResult {
            confidence: Some("0.95".to_string()),
            ..Default::default()
        };
        assert_eq!(result.confidence_f64(), Some(0.95));

        let result = ReverieAsrResult {
            confidence: Some("invalid".to_string()),
            ..Default::default()
        };
        assert_eq!(result.confidence_f64(), None);
    }
}
