//! Message types for Amazon Transcribe Streaming responses.
//!
//! This module defines the data structures used to parse and handle
//! responses from the Amazon Transcribe Streaming API.
//!
//! The API returns transcription results as `TranscriptEvent` messages
//! containing `Transcript` objects with one or more `Result` items.

use serde::{Deserialize, Serialize};

// =============================================================================
// Transcription Results
// =============================================================================

/// A single word or token in the transcription.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Item {
    /// Start time of the word in seconds from the beginning of the stream.
    #[serde(rename = "StartTime")]
    pub start_time: Option<f64>,

    /// End time of the word in seconds from the beginning of the stream.
    #[serde(rename = "EndTime")]
    pub end_time: Option<f64>,

    /// The type of item (pronunciation, punctuation).
    #[serde(rename = "Type")]
    pub item_type: Option<String>,

    /// The transcribed word or punctuation.
    #[serde(rename = "Content")]
    pub content: Option<String>,

    /// Whether this word is vocabulary filtered.
    #[serde(rename = "VocabularyFilterMatch")]
    pub vocabulary_filter_match: Option<bool>,

    /// Confidence score for this word (0.0 to 1.0).
    #[serde(rename = "Confidence")]
    pub confidence: Option<f64>,

    /// Speaker label for this item (if speaker diarization is enabled).
    #[serde(rename = "Speaker")]
    pub speaker: Option<String>,

    /// Whether this item is stable (won't change in future results).
    #[serde(rename = "Stable")]
    pub stable: Option<bool>,
}

/// An alternative transcription with confidence score.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alternative {
    /// The transcribed text.
    #[serde(rename = "Transcript")]
    pub transcript: Option<String>,

    /// Individual items (words) in this alternative.
    #[serde(rename = "Items")]
    pub items: Option<Vec<Item>>,

    /// Entities detected in the transcription.
    #[serde(rename = "Entities")]
    pub entities: Option<Vec<Entity>>,
}

/// An entity detected in the transcription (e.g., PII).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entity {
    /// Start time of the entity in seconds.
    #[serde(rename = "StartTime")]
    pub start_time: Option<f64>,

    /// End time of the entity in seconds.
    #[serde(rename = "EndTime")]
    pub end_time: Option<f64>,

    /// Category of the entity (e.g., "PII", "PHI").
    #[serde(rename = "Category")]
    pub category: Option<String>,

    /// Type of the entity (e.g., "NAME", "PHONE", "EMAIL").
    #[serde(rename = "Type")]
    pub entity_type: Option<String>,

    /// The actual text of the entity.
    #[serde(rename = "Content")]
    pub content: Option<String>,

    /// Confidence score for entity detection.
    #[serde(rename = "Confidence")]
    pub confidence: Option<f64>,
}

/// A transcription result segment.
///
/// Results can be partial (interim) or final. Partial results may change
/// as more audio is processed, while final results are stable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Result {
    /// Unique identifier for this result.
    #[serde(rename = "ResultId")]
    pub result_id: Option<String>,

    /// Start time of this segment in seconds.
    #[serde(rename = "StartTime")]
    pub start_time: Option<f64>,

    /// End time of this segment in seconds.
    #[serde(rename = "EndTime")]
    pub end_time: Option<f64>,

    /// Whether this is a partial (interim) result.
    ///
    /// - `true`: This result may change in subsequent responses
    /// - `false`: This result is final and won't change
    #[serde(rename = "IsPartial")]
    pub is_partial: Option<bool>,

    /// Alternative transcriptions, ordered by confidence.
    #[serde(rename = "Alternatives")]
    pub alternatives: Option<Vec<Alternative>>,

    /// Channel ID for multi-channel audio.
    #[serde(rename = "ChannelId")]
    pub channel_id: Option<String>,

    /// Detected language code (if automatic language detection is enabled).
    #[serde(rename = "LanguageCode")]
    pub language_code: Option<String>,

    /// Confidence score for the detected language.
    #[serde(rename = "LanguageIdentification")]
    pub language_identification: Option<Vec<LanguageWithScore>>,
}

/// Language identification result with confidence score.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanguageWithScore {
    /// The language code (e.g., "en-US").
    #[serde(rename = "LanguageCode")]
    pub language_code: Option<String>,

    /// Confidence score for this language (0.0 to 1.0).
    #[serde(rename = "Score")]
    pub score: Option<f64>,
}

/// The main transcript object containing all results.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transcript {
    /// List of transcription results.
    #[serde(rename = "Results")]
    pub results: Option<Vec<Result>>,
}

/// A transcript event from the Amazon Transcribe Streaming API.
///
/// This is the top-level response structure for streaming transcription.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptEvent {
    /// The transcript data.
    #[serde(rename = "Transcript")]
    pub transcript: Option<Transcript>,
}

// =============================================================================
// Helper Methods
// =============================================================================

impl Result {
    /// Get the best transcription from this result.
    ///
    /// Returns the transcript from the first alternative (highest confidence).
    pub fn best_transcript(&self) -> Option<&str> {
        self.alternatives
            .as_ref()
            .and_then(|alts| alts.first())
            .and_then(|alt| alt.transcript.as_deref())
    }

    /// Check if this result is final (not partial).
    pub fn is_final(&self) -> bool {
        self.is_partial.map(|p| !p).unwrap_or(false)
    }

    /// Get the confidence score for the best alternative.
    ///
    /// Calculates average confidence from word-level confidences.
    pub fn confidence(&self) -> f32 {
        if let Some(alts) = &self.alternatives
            && let Some(alt) = alts.first()
            && let Some(items) = &alt.items
        {
            let confidences: Vec<f64> = items.iter().filter_map(|item| item.confidence).collect();

            if !confidences.is_empty() {
                let sum: f64 = confidences.iter().sum();
                return (sum / confidences.len() as f64) as f32;
            }
        }

        // Default confidence if not available
        0.0
    }

    /// Get the duration of this segment in seconds.
    pub fn duration(&self) -> Option<f64> {
        match (self.start_time, self.end_time) {
            (Some(start), Some(end)) => Some(end - start),
            _ => None,
        }
    }

    /// Get all stable words from this result.
    ///
    /// Useful when partial results stabilization is enabled.
    pub fn stable_words(&self) -> Vec<&str> {
        self.alternatives
            .as_ref()
            .and_then(|alts| alts.first())
            .and_then(|alt| alt.items.as_ref())
            .map(|items| {
                items
                    .iter()
                    .filter(|item| item.stable.unwrap_or(false))
                    .filter_map(|item| item.content.as_deref())
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl TranscriptEvent {
    /// Get all results from this event.
    pub fn results(&self) -> Option<&[Result]> {
        self.transcript.as_ref().and_then(|t| t.results.as_deref())
    }

    /// Check if this event contains any final results.
    pub fn has_final_results(&self) -> bool {
        self.results()
            .map(|results| results.iter().any(|r| r.is_final()))
            .unwrap_or(false)
    }

    /// Get the best transcript from the first result.
    pub fn best_transcript(&self) -> Option<&str> {
        self.results()
            .and_then(|results| results.first())
            .and_then(|r| r.best_transcript())
    }
}

// =============================================================================
// Error Types
// =============================================================================

/// Error response from Amazon Transcribe Streaming.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscribeError {
    /// Error message.
    #[serde(rename = "Message")]
    pub message: Option<String>,

    /// Error code.
    #[serde(rename = "Code")]
    pub code: Option<String>,
}

impl std::fmt::Display for TranscribeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (&self.code, &self.message) {
            (Some(code), Some(msg)) => write!(f, "{}: {}", code, msg),
            (Some(code), None) => write!(f, "{}", code),
            (None, Some(msg)) => write!(f, "{}", msg),
            (None, None) => write!(f, "Unknown error"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_result_best_transcript() {
        let result = Result {
            result_id: Some("123".to_string()),
            start_time: Some(0.0),
            end_time: Some(1.0),
            is_partial: Some(false),
            alternatives: Some(vec![Alternative {
                transcript: Some("Hello world".to_string()),
                items: None,
                entities: None,
            }]),
            channel_id: None,
            language_code: None,
            language_identification: None,
        };

        assert_eq!(result.best_transcript(), Some("Hello world"));
        assert!(result.is_final());
    }

    #[test]
    fn test_result_is_partial() {
        let result = Result {
            result_id: Some("123".to_string()),
            start_time: None,
            end_time: None,
            is_partial: Some(true),
            alternatives: None,
            channel_id: None,
            language_code: None,
            language_identification: None,
        };

        assert!(!result.is_final());
    }

    #[test]
    fn test_result_confidence() {
        let result = Result {
            result_id: None,
            start_time: None,
            end_time: None,
            is_partial: Some(false),
            alternatives: Some(vec![Alternative {
                transcript: Some("Hello world".to_string()),
                items: Some(vec![
                    Item {
                        start_time: Some(0.0),
                        end_time: Some(0.5),
                        item_type: Some("pronunciation".to_string()),
                        content: Some("Hello".to_string()),
                        vocabulary_filter_match: None,
                        confidence: Some(0.95),
                        speaker: None,
                        stable: Some(true),
                    },
                    Item {
                        start_time: Some(0.5),
                        end_time: Some(1.0),
                        item_type: Some("pronunciation".to_string()),
                        content: Some("world".to_string()),
                        vocabulary_filter_match: None,
                        confidence: Some(0.85),
                        speaker: None,
                        stable: Some(true),
                    },
                ]),
                entities: None,
            }]),
            channel_id: None,
            language_code: None,
            language_identification: None,
        };

        let confidence = result.confidence();
        assert!((confidence - 0.9).abs() < 0.01); // Average of 0.95 and 0.85
    }

    #[test]
    fn test_result_duration() {
        let result = Result {
            result_id: None,
            start_time: Some(1.5),
            end_time: Some(3.5),
            is_partial: None,
            alternatives: None,
            channel_id: None,
            language_code: None,
            language_identification: None,
        };

        assert_eq!(result.duration(), Some(2.0));
    }

    #[test]
    fn test_stable_words() {
        let result = Result {
            result_id: None,
            start_time: None,
            end_time: None,
            is_partial: Some(true),
            alternatives: Some(vec![Alternative {
                transcript: Some("Hello world test".to_string()),
                items: Some(vec![
                    Item {
                        start_time: None,
                        end_time: None,
                        item_type: None,
                        content: Some("Hello".to_string()),
                        vocabulary_filter_match: None,
                        confidence: None,
                        speaker: None,
                        stable: Some(true),
                    },
                    Item {
                        start_time: None,
                        end_time: None,
                        item_type: None,
                        content: Some("world".to_string()),
                        vocabulary_filter_match: None,
                        confidence: None,
                        speaker: None,
                        stable: Some(true),
                    },
                    Item {
                        start_time: None,
                        end_time: None,
                        item_type: None,
                        content: Some("test".to_string()),
                        vocabulary_filter_match: None,
                        confidence: None,
                        speaker: None,
                        stable: Some(false), // Not stable
                    },
                ]),
                entities: None,
            }]),
            channel_id: None,
            language_code: None,
            language_identification: None,
        };

        let stable = result.stable_words();
        assert_eq!(stable, vec!["Hello", "world"]);
    }

    #[test]
    fn test_transcript_event_has_final_results() {
        let event = TranscriptEvent {
            transcript: Some(Transcript {
                results: Some(vec![Result {
                    result_id: None,
                    start_time: None,
                    end_time: None,
                    is_partial: Some(false), // Final
                    alternatives: None,
                    channel_id: None,
                    language_code: None,
                    language_identification: None,
                }]),
            }),
        };

        assert!(event.has_final_results());
    }

    #[test]
    fn test_transcribe_error_display() {
        let error = TranscribeError {
            message: Some("Invalid audio format".to_string()),
            code: Some("BadRequestException".to_string()),
        };

        assert_eq!(
            error.to_string(),
            "BadRequestException: Invalid audio format"
        );
    }
}
