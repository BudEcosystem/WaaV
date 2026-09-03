//! AmiVoice WebSocket message types.
//!
//! This module defines the message types used for communication with the
//! AmiVoice Cloud Platform WebSocket API.
//!
//! # Protocol Overview
//!
//! AmiVoice uses a proprietary text-based protocol:
//!
//! ## Commands (Client → Server)
//! - `s <format> <engine> [params]` - Start session
//! - `p<binary_data>` - Send audio data
//! - `e` - End session
//!
//! ## Events (Server → Client)
//! - `s` / `s <error>` - Session start response
//! - `S <timestamp>` - Speech detected start
//! - `E <timestamp>` - Speech detected end
//! - `C` - Recognition processing started
//! - `U <json>` - Interim result
//! - `A <json>` - Final result
//! - `G <info>` - Server-generated info
//! - `e` / `e <error>` - Session end response

use crate::core::stt::base::STTResult;
use serde::{Deserialize, Serialize};
use tracing::warn;

// =============================================================================
// AmiVoice Message Types
// =============================================================================

/// Parsed AmiVoice WebSocket message.
#[derive(Debug, Clone)]
pub enum AmiVoiceMessage {
    /// Session start response (success)
    SessionStartOk,

    /// Session start response (error)
    SessionStartError(String),

    /// Speech detected start event with timestamp
    SpeechStart { timestamp_ms: u64 },

    /// Speech detected end event with timestamp
    SpeechEnd { timestamp_ms: u64 },

    /// Recognition processing started
    ProcessingStart,

    /// Interim (partial) recognition result
    InterimResult(AmiVoiceResult),

    /// Final recognition result
    FinalResult(AmiVoiceResult),

    /// Server-generated information
    ServerInfo(String),

    /// Session end response (success)
    SessionEndOk,

    /// Session end response (error)
    SessionEndError(String),
}

impl AmiVoiceMessage {
    /// Parse a text message from AmiVoice WebSocket.
    pub fn parse(text: &str) -> Result<Self, String> {
        if text.is_empty() {
            return Err("Empty message".to_string());
        }

        // Get the first character (command/event type)
        let Some(first_char) = text.chars().next() else {
            return Err("Empty message".to_string());
        };
        let rest = text[first_char.len_utf8()..].trim();

        match first_char {
            's' => {
                // Session start response
                if rest.is_empty() {
                    Ok(Self::SessionStartOk)
                } else {
                    Ok(Self::SessionStartError(rest.to_string()))
                }
            }

            'S' => {
                // Speech start event: "S <timestamp>"
                let timestamp_ms = rest.parse::<u64>().unwrap_or(0);
                Ok(Self::SpeechStart { timestamp_ms })
            }

            'E' => {
                // Speech end event: "E <timestamp>"
                let timestamp_ms = rest.parse::<u64>().unwrap_or(0);
                Ok(Self::SpeechEnd { timestamp_ms })
            }

            'C' => {
                // Recognition processing started
                Ok(Self::ProcessingStart)
            }

            'U' => {
                // Interim result: "U <json>"
                let json_str = rest;
                match serde_json::from_str::<AmiVoiceResult>(json_str) {
                    Ok(result) => Ok(Self::InterimResult(result)),
                    Err(e) => {
                        warn!("Failed to parse interim result JSON: {} - {}", e, json_str);
                        Err(format!("Failed to parse interim result: {e}"))
                    }
                }
            }

            'A' => {
                // Final result: "A <json>"
                let json_str = rest;
                match serde_json::from_str::<AmiVoiceResult>(json_str) {
                    Ok(result) => Ok(Self::FinalResult(result)),
                    Err(e) => {
                        warn!("Failed to parse final result JSON: {} - {}", e, json_str);
                        Err(format!("Failed to parse final result: {e}"))
                    }
                }
            }

            'G' => {
                // Server-generated info: "G <info>"
                Ok(Self::ServerInfo(rest.to_string()))
            }

            'e' => {
                // Session end response
                if rest.is_empty() {
                    Ok(Self::SessionEndOk)
                } else {
                    Ok(Self::SessionEndError(rest.to_string()))
                }
            }

            _ => Err(format!("Unknown message type: {}", first_char)),
        }
    }

    /// Check if this is an error message.
    pub fn is_error(&self) -> bool {
        matches!(self, Self::SessionStartError(_) | Self::SessionEndError(_))
    }

    /// Get error message if this is an error.
    pub fn error_message(&self) -> Option<&str> {
        match self {
            Self::SessionStartError(e) | Self::SessionEndError(e) => Some(e),
            _ => None,
        }
    }
}

// =============================================================================
// AmiVoice Result Structure
// =============================================================================

/// AmiVoice recognition result (from U or A events).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AmiVoiceResult {
    /// Recognition results array.
    #[serde(default)]
    pub results: Vec<AmiVoiceRecognitionResult>,

    /// Full recognized text.
    #[serde(default)]
    pub text: String,

    /// Response code ("0" for success).
    #[serde(default)]
    pub code: String,

    /// Response message.
    #[serde(default)]
    pub message: String,
}

impl AmiVoiceResult {
    /// Convert to STTResult.
    ///
    /// Maps AmiVoice result to the gateway's common STTResult structure.
    /// Note: Word-level timing and extended metadata are stored in AmiVoiceResult
    /// but the base STTResult only contains transcript, confidence, and final status.
    pub fn to_stt_result(&self, is_final: bool) -> STTResult {
        // Get the full text
        let transcript = if !self.text.is_empty() {
            self.text.clone()
        } else {
            // Concatenate all result texts
            self.results
                .iter()
                .map(|r| r.text.as_str())
                .collect::<Vec<_>>()
                .join("")
        };

        // Calculate average confidence from tokens
        let confidence = self.calculate_confidence();

        // Use STTResult::new to ensure confidence clamping
        STTResult::new(
            transcript, is_final, is_final, // is_speech_final matches is_final for AmiVoice
            confidence,
        )
    }

    /// Calculate average confidence from all tokens.
    fn calculate_confidence(&self) -> f32 {
        let mut total_confidence = 0.0;
        let mut count = 0;

        for result in &self.results {
            for token in &result.tokens {
                if let Some(conf) = token.confidence {
                    total_confidence += conf;
                    count += 1;
                }
            }
        }

        if count > 0 {
            total_confidence / count as f32
        } else {
            0.0
        }
    }

    /// Get word-level timing information (AmiVoice-specific).
    ///
    /// Returns a vector of (word, start_time_ms, end_time_ms, confidence) tuples.
    /// This preserves the detailed timing info from AmiVoice that isn't in the base STTResult.
    pub fn get_word_timings(&self) -> Vec<(String, Option<u64>, Option<u64>, Option<f32>)> {
        let mut words = Vec::new();

        for result in &self.results {
            for token in &result.tokens {
                words.push((
                    token.written.clone(),
                    token.starttime,
                    token.endtime,
                    token.confidence,
                ));
            }
        }

        words
    }

    /// Get timing information from results.
    ///
    /// Returns (start_time_ms, end_time_ms) from first and last tokens.
    pub fn get_timing(&self) -> (Option<u64>, Option<u64>) {
        if self.results.is_empty() {
            return (None, None);
        }

        // Get start time from first token of first result
        let start_time = self
            .results
            .first()
            .and_then(|r| r.tokens.first())
            .and_then(|t| t.starttime);

        // Get end time from last token of last result
        let end_time = self
            .results
            .last()
            .and_then(|r| r.tokens.last())
            .and_then(|t| t.endtime);

        (start_time, end_time)
    }

    /// Check if this is a successful result.
    pub fn is_success(&self) -> bool {
        self.code == "0" || self.code.is_empty()
    }
}

/// Individual recognition result entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AmiVoiceRecognitionResult {
    /// Recognized text for this segment.
    #[serde(default)]
    pub text: String,

    /// Token-level information.
    #[serde(default)]
    pub tokens: Vec<AmiVoiceToken>,

    /// Recognition tags (for grammar-based recognition).
    #[serde(default)]
    pub tags: Vec<String>,

    /// Rule name (for grammar-based recognition).
    #[serde(default)]
    pub rulename: String,
}

/// Token (word) information from recognition result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AmiVoiceToken {
    /// Written form of the word.
    #[serde(default)]
    pub written: String,

    /// Confidence score (0.0-1.0).
    #[serde(default)]
    pub confidence: Option<f32>,

    /// Start time in milliseconds.
    #[serde(default)]
    pub starttime: Option<u64>,

    /// End time in milliseconds.
    #[serde(default)]
    pub endtime: Option<u64>,

    /// Spoken form (pronunciation).
    #[serde(default)]
    pub spoken: Option<String>,
}

// =============================================================================
// Sentiment Analysis Result
// =============================================================================

/// Sentiment analysis result from AmiVoice.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AmiVoiceSentiment {
    /// Joy level (0.0-1.0).
    #[serde(default)]
    pub joy: f32,

    /// Anger level (0.0-1.0).
    #[serde(default)]
    pub anger: f32,

    /// Stress level (0.0-1.0).
    #[serde(default)]
    pub stress: f32,

    /// Dissatisfaction level (0.0-1.0).
    #[serde(default)]
    pub dissatisfaction: f32,

    /// Expectation level (0.0-1.0).
    #[serde(default)]
    pub expectation: f32,

    /// Timestamp in milliseconds.
    #[serde(default)]
    pub timestamp_ms: u64,
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_session_start_ok() {
        let msg = AmiVoiceMessage::parse("s").unwrap();
        assert!(matches!(msg, AmiVoiceMessage::SessionStartOk));
    }

    #[test]
    fn test_parse_session_start_error() {
        let msg = AmiVoiceMessage::parse("s Authentication failed").unwrap();
        match msg {
            AmiVoiceMessage::SessionStartError(e) => {
                assert_eq!(e, "Authentication failed");
            }
            _ => panic!("Expected SessionStartError"),
        }
    }

    #[test]
    fn test_parse_speech_start() {
        let msg = AmiVoiceMessage::parse("S 1234").unwrap();
        match msg {
            AmiVoiceMessage::SpeechStart { timestamp_ms } => {
                assert_eq!(timestamp_ms, 1234);
            }
            _ => panic!("Expected SpeechStart"),
        }
    }

    #[test]
    fn test_parse_speech_end() {
        let msg = AmiVoiceMessage::parse("E 5678").unwrap();
        match msg {
            AmiVoiceMessage::SpeechEnd { timestamp_ms } => {
                assert_eq!(timestamp_ms, 5678);
            }
            _ => panic!("Expected SpeechEnd"),
        }
    }

    #[test]
    fn test_parse_processing_start() {
        let msg = AmiVoiceMessage::parse("C").unwrap();
        assert!(matches!(msg, AmiVoiceMessage::ProcessingStart));
    }

    #[test]
    fn test_parse_interim_result() {
        let json_result = r#"{"results":[{"text":"こんにちは","tokens":[{"written":"こんにちは","confidence":0.95}]}],"text":"こんにちは","code":"0","message":"success"}"#;
        let msg = AmiVoiceMessage::parse(&format!("U {}", json_result)).unwrap();

        match msg {
            AmiVoiceMessage::InterimResult(result) => {
                assert_eq!(result.text, "こんにちは");
                assert!(result.is_success());
            }
            _ => panic!("Expected InterimResult"),
        }
    }

    #[test]
    fn test_parse_final_result() {
        let json_result = r#"{"results":[{"text":"今日はいい天気です","tokens":[{"written":"今日","starttime":100,"endtime":300},{"written":"は","starttime":300,"endtime":350},{"written":"いい","starttime":350,"endtime":500},{"written":"天気","starttime":500,"endtime":700},{"written":"です","starttime":700,"endtime":900}]}],"text":"今日はいい天気です","code":"0","message":"success"}"#;
        let msg = AmiVoiceMessage::parse(&format!("A {}", json_result)).unwrap();

        match msg {
            AmiVoiceMessage::FinalResult(result) => {
                assert_eq!(result.text, "今日はいい天気です");
                assert_eq!(result.results.len(), 1);
                assert_eq!(result.results[0].tokens.len(), 5);
            }
            _ => panic!("Expected FinalResult"),
        }
    }

    #[test]
    fn test_parse_server_info() {
        let msg = AmiVoiceMessage::parse("G Some server info").unwrap();
        match msg {
            AmiVoiceMessage::ServerInfo(info) => {
                assert_eq!(info, "Some server info");
            }
            _ => panic!("Expected ServerInfo"),
        }
    }

    #[test]
    fn test_parse_session_end_ok() {
        let msg = AmiVoiceMessage::parse("e").unwrap();
        assert!(matches!(msg, AmiVoiceMessage::SessionEndOk));
    }

    #[test]
    fn test_parse_session_end_error() {
        let msg = AmiVoiceMessage::parse("e Session timeout").unwrap();
        match msg {
            AmiVoiceMessage::SessionEndError(e) => {
                assert_eq!(e, "Session timeout");
            }
            _ => panic!("Expected SessionEndError"),
        }
    }

    #[test]
    fn test_parse_unknown_message() {
        let result = AmiVoiceMessage::parse("X unknown");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_unknown_multibyte_message_type() {
        let result = AmiVoiceMessage::parse("あ unknown");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_empty_message() {
        let result = AmiVoiceMessage::parse("");
        assert!(result.is_err());
    }

    #[test]
    fn test_result_to_stt_result() {
        let result = AmiVoiceResult {
            results: vec![AmiVoiceRecognitionResult {
                text: "テスト".to_string(),
                tokens: vec![AmiVoiceToken {
                    written: "テスト".to_string(),
                    confidence: Some(0.95),
                    starttime: Some(100),
                    endtime: Some(500),
                    spoken: Some("てすと".to_string()),
                }],
                tags: vec![],
                rulename: String::new(),
            }],
            text: "テスト".to_string(),
            code: "0".to_string(),
            message: "success".to_string(),
        };

        let stt_result = result.to_stt_result(true);
        assert_eq!(stt_result.transcript, "テスト");
        assert!(stt_result.is_final);
        assert!(stt_result.is_speech_final);
        assert!((stt_result.confidence - 0.95).abs() < 0.001);

        // Word timings are available through AmiVoiceResult directly
        let word_timings = result.get_word_timings();
        assert_eq!(word_timings.len(), 1);
        assert_eq!(word_timings[0].0, "テスト");
        assert_eq!(word_timings[0].1, Some(100));
        assert_eq!(word_timings[0].2, Some(500));
    }

    #[test]
    fn test_result_confidence_calculation() {
        let result = AmiVoiceResult {
            results: vec![AmiVoiceRecognitionResult {
                text: "test".to_string(),
                tokens: vec![
                    AmiVoiceToken {
                        written: "a".to_string(),
                        confidence: Some(0.9),
                        starttime: None,
                        endtime: None,
                        spoken: None,
                    },
                    AmiVoiceToken {
                        written: "b".to_string(),
                        confidence: Some(0.8),
                        starttime: None,
                        endtime: None,
                        spoken: None,
                    },
                ],
                tags: vec![],
                rulename: String::new(),
            }],
            text: "ab".to_string(),
            code: "0".to_string(),
            message: "".to_string(),
        };

        let confidence = result.calculate_confidence();
        assert!((confidence - 0.85).abs() < 0.001);
    }

    #[test]
    fn test_is_error() {
        assert!(!AmiVoiceMessage::SessionStartOk.is_error());
        assert!(AmiVoiceMessage::SessionStartError("error".to_string()).is_error());
        assert!(!AmiVoiceMessage::SessionEndOk.is_error());
        assert!(AmiVoiceMessage::SessionEndError("error".to_string()).is_error());
    }

    #[test]
    fn test_error_message() {
        let msg = AmiVoiceMessage::SessionStartError("auth failed".to_string());
        assert_eq!(msg.error_message(), Some("auth failed"));

        let msg = AmiVoiceMessage::SessionStartOk;
        assert_eq!(msg.error_message(), None);
    }
}
