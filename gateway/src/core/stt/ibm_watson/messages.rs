//! IBM Watson Speech-to-Text message types.
//!
//! This module defines the message structures for parsing WebSocket responses
//! from the IBM Watson Speech-to-Text API.

use crate::core::stt::base::STTResult;
use serde::{Deserialize, Serialize};

// =============================================================================
// Main Response Types
// =============================================================================

/// IBM Watson STT WebSocket response message.
///
/// The API sends different message types during recognition:
/// - `listening`: Service is ready to receive audio
/// - `state`: State change notification
/// - `results`: Recognition results (interim or final)
/// - `error`: Error notification
/// - `speaker_labels`: Speaker diarization results
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum IbmWatsonMessage {
    /// Recognition results message
    Results(ResultsMessage),
    /// Listening state message
    Listening(ListeningMessage),
    /// State change message
    State(StateMessage),
    /// Error message
    Error(ErrorMessage),
    /// Speaker labels message
    SpeakerLabels(SpeakerLabelsMessage),
}

impl IbmWatsonMessage {
    /// Parse a JSON string into an IBM Watson message.
    pub fn parse(text: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(text)
    }

    /// Check if this is a listening message.
    pub fn is_listening(&self) -> bool {
        matches!(self, Self::Listening(_))
    }

    /// Check if this is a results message.
    pub fn is_results(&self) -> bool {
        matches!(self, Self::Results(_))
    }

    /// Check if this is an error message.
    pub fn is_error(&self) -> bool {
        matches!(self, Self::Error(_))
    }
}

// =============================================================================
// Recognition Results
// =============================================================================

/// Recognition results message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResultsMessage {
    /// Array of recognition results.
    pub results: Vec<RecognitionResult>,
    /// Index of the result in the overall session.
    #[serde(default)]
    pub result_index: i32,
}

impl ResultsMessage {
    /// Convert to STTResult for the most recent transcript.
    pub fn to_stt_result(&self) -> Option<STTResult> {
        self.results.last().and_then(|r| r.to_stt_result())
    }

    /// Get all transcripts from this message.
    pub fn all_transcripts(&self) -> Vec<STTResult> {
        self.results.iter().filter_map(|r| r.to_stt_result()).collect()
    }
}

/// Single recognition result within a results message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecognitionResult {
    /// Whether this is a final result (not subject to change).
    #[serde(rename = "final")]
    pub is_final: bool,
    /// Alternative transcription hypotheses.
    pub alternatives: Vec<TranscriptionAlternative>,
    /// Optional word-level timestamps.
    #[serde(default)]
    pub keywords_result: Option<serde_json::Value>,
    /// Word alternatives (confusion networks).
    #[serde(default)]
    pub word_alternatives: Option<Vec<WordAlternatives>>,
    /// End of utterance marker.
    #[serde(default)]
    pub end_of_utterance: Option<String>,
}

impl RecognitionResult {
    /// Convert to STTResult using the best alternative.
    pub fn to_stt_result(&self) -> Option<STTResult> {
        self.alternatives.first().map(|alt| {
            STTResult::new(
                alt.transcript.trim().to_string(),
                self.is_final,
                self.is_final
                    && self
                        .end_of_utterance
                        .as_ref()
                        .is_some_and(|s| s == "end_of_data" || s == "end_of_utterance"),
                alt.confidence.unwrap_or(0.0) as f32,
            )
        })
    }

    /// Get word-level timing information.
    pub fn get_word_timestamps(&self) -> Vec<WordTimestamp> {
        self.alternatives
            .first()
            .and_then(|alt| alt.timestamps.clone())
            .unwrap_or_default()
    }

    /// Get word-level confidence scores.
    pub fn get_word_confidences(&self) -> Vec<WordConfidence> {
        self.alternatives
            .first()
            .and_then(|alt| alt.word_confidence.clone())
            .unwrap_or_default()
    }
}

/// Transcription alternative (hypothesis).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptionAlternative {
    /// Transcribed text.
    pub transcript: String,
    /// Confidence score (0.0 to 1.0).
    pub confidence: Option<f64>,
    /// Word-level timestamps: [[word, start_time, end_time], ...]
    #[serde(default)]
    pub timestamps: Option<Vec<WordTimestamp>>,
    /// Word-level confidence scores: [[word, confidence], ...]
    #[serde(default)]
    pub word_confidence: Option<Vec<WordConfidence>>,
}

/// Word-level timestamp [word, start_time, end_time].
pub type WordTimestamp = (String, f64, f64);

/// Word-level confidence [word, confidence].
pub type WordConfidence = (String, f64);

/// Word alternatives (confusion network entry).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WordAlternatives {
    /// Start time in seconds.
    pub start_time: f64,
    /// End time in seconds.
    pub end_time: f64,
    /// Alternative words at this position.
    pub alternatives: Vec<WordAlternative>,
}

/// Single word alternative.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WordAlternative {
    /// Confidence score for this alternative.
    pub confidence: f64,
    /// The word.
    pub word: String,
}

// =============================================================================
// State Messages
// =============================================================================

/// Listening state message indicating the service is ready.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListeningMessage {
    /// Always "listening" for this message type.
    pub state: String,
}

/// State change message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateMessage {
    /// The new state.
    pub state: String,
}

// =============================================================================
// Speaker Labels
// =============================================================================

/// Speaker labels message for speaker diarization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeakerLabelsMessage {
    /// Speaker label entries.
    pub speaker_labels: Vec<SpeakerLabel>,
}

/// Individual speaker label entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeakerLabel {
    /// Start time of the segment in seconds.
    pub from: f64,
    /// End time of the segment in seconds.
    pub to: f64,
    /// Speaker identifier (0, 1, 2, ...).
    pub speaker: i32,
    /// Confidence score for this label.
    pub confidence: f64,
    /// Whether this is a final label.
    #[serde(rename = "final")]
    pub is_final: bool,
}

// =============================================================================
// Error Messages
// =============================================================================

/// Error message from the service.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorMessage {
    /// Error description.
    pub error: String,
    /// Error code (optional).
    #[serde(default)]
    pub code: Option<i32>,
    /// Warning messages (optional).
    #[serde(default)]
    pub warnings: Option<Vec<String>>,
}

impl ErrorMessage {
    /// Check if this is a critical error that should close the connection.
    pub fn is_critical(&self) -> bool {
        // Session timeout or invalid state errors are critical
        self.error.contains("session timed out")
            || self.error.contains("session closed")
            || self.error.contains("invalid state")
            || self.code.is_some_and(|c| c >= 500)
    }

    /// Check if this is an inactivity timeout error.
    pub fn is_inactivity_timeout(&self) -> bool {
        self.error.contains("inactivity")
            || self.error.contains("no speech")
            || self.code == Some(408)
    }
}

// =============================================================================
// Start/Stop Messages (Client to Server)
// =============================================================================

/// Start recognition message sent to the service.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartMessage {
    /// Action type: "start"
    pub action: String,
    /// Audio content type
    #[serde(rename = "content-type")]
    pub content_type: String,
    /// Enable interim results
    #[serde(default)]
    pub interim_results: bool,
    /// Enable timestamps
    #[serde(default)]
    pub timestamps: bool,
    /// Enable word confidence
    #[serde(default)]
    pub word_confidence: bool,
    /// Enable speaker labels
    #[serde(default)]
    pub speaker_labels: bool,
    /// Smart formatting
    #[serde(default)]
    pub smart_formatting: bool,
    /// Profanity filter
    #[serde(default)]
    pub profanity_filter: bool,
    /// Inactivity timeout in seconds
    #[serde(default)]
    pub inactivity_timeout: i32,
}

/// Stop recognition message sent to the service.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StopMessage {
    /// Action type: "stop"
    pub action: String,
}

impl StopMessage {
    /// Create a new stop message.
    pub fn new() -> Self {
        Self {
            action: "stop".to_string(),
        }
    }
}

impl Default for StopMessage {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_listening_message() {
        let json = r#"{"state": "listening"}"#;
        let msg = IbmWatsonMessage::parse(json).unwrap();
        assert!(msg.is_listening());
    }

    #[test]
    fn test_parse_results_message() {
        let json = r#"{
            "results": [
                {
                    "alternatives": [
                        {
                            "transcript": "hello world",
                            "confidence": 0.95
                        }
                    ],
                    "final": true
                }
            ],
            "result_index": 0
        }"#;

        let msg = IbmWatsonMessage::parse(json).unwrap();
        assert!(msg.is_results());

        if let IbmWatsonMessage::Results(results) = msg {
            assert_eq!(results.results.len(), 1);
            let stt_result = results.to_stt_result().unwrap();
            assert_eq!(stt_result.transcript, "hello world");
            assert!(stt_result.is_final);
            assert!((stt_result.confidence - 0.95).abs() < 0.001);
        }
    }

    #[test]
    fn test_parse_interim_results() {
        let json = r#"{
            "results": [
                {
                    "alternatives": [
                        {
                            "transcript": "hello"
                        }
                    ],
                    "final": false
                }
            ],
            "result_index": 0
        }"#;

        let msg = IbmWatsonMessage::parse(json).unwrap();
        if let IbmWatsonMessage::Results(results) = msg {
            let stt_result = results.to_stt_result().unwrap();
            assert_eq!(stt_result.transcript, "hello");
            assert!(!stt_result.is_final);
            assert_eq!(stt_result.confidence, 0.0); // No confidence for interim
        }
    }

    #[test]
    fn test_parse_error_message() {
        let json = r#"{
            "error": "session timed out after 30 seconds",
            "code": 408
        }"#;

        let msg = IbmWatsonMessage::parse(json).unwrap();
        assert!(msg.is_error());

        if let IbmWatsonMessage::Error(error) = msg {
            assert!(error.is_critical());
            assert!(error.is_inactivity_timeout());
        }
    }

    #[test]
    fn test_parse_results_with_timestamps() {
        let json = r#"{
            "results": [
                {
                    "alternatives": [
                        {
                            "transcript": "hello world",
                            "confidence": 0.95,
                            "timestamps": [
                                ["hello", 0.0, 0.5],
                                ["world", 0.6, 1.0]
                            ]
                        }
                    ],
                    "final": true
                }
            ],
            "result_index": 0
        }"#;

        let msg = IbmWatsonMessage::parse(json).unwrap();
        if let IbmWatsonMessage::Results(results) = msg {
            let timestamps = results.results[0].get_word_timestamps();
            assert_eq!(timestamps.len(), 2);
            assert_eq!(timestamps[0].0, "hello");
            assert!((timestamps[0].1 - 0.0).abs() < 0.001);
            assert!((timestamps[0].2 - 0.5).abs() < 0.001);
        }
    }

    #[test]
    fn test_parse_speaker_labels() {
        let json = r#"{
            "speaker_labels": [
                {
                    "from": 0.0,
                    "to": 1.5,
                    "speaker": 0,
                    "confidence": 0.85,
                    "final": true
                },
                {
                    "from": 1.6,
                    "to": 3.0,
                    "speaker": 1,
                    "confidence": 0.90,
                    "final": true
                }
            ]
        }"#;

        let msg = IbmWatsonMessage::parse(json).unwrap();
        if let IbmWatsonMessage::SpeakerLabels(labels) = msg {
            assert_eq!(labels.speaker_labels.len(), 2);
            assert_eq!(labels.speaker_labels[0].speaker, 0);
            assert_eq!(labels.speaker_labels[1].speaker, 1);
        }
    }

    #[test]
    fn test_stop_message() {
        let msg = StopMessage::new();
        assert_eq!(msg.action, "stop");

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"action\":\"stop\""));
    }

    #[test]
    fn test_error_criticality() {
        let timeout_error = ErrorMessage {
            error: "session timed out".to_string(),
            code: Some(408),
            warnings: None,
        };
        assert!(timeout_error.is_critical());
        assert!(timeout_error.is_inactivity_timeout());

        let minor_error = ErrorMessage {
            error: "unable to find speaker".to_string(),
            code: None,
            warnings: None,
        };
        assert!(!minor_error.is_critical());
        assert!(!minor_error.is_inactivity_timeout());
    }
}
