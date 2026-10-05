//! Message types for OpenAI STT (Whisper) API.
//!
//! This module contains request and response types for the OpenAI
//! Audio Transcription API (Whisper).
//!
//! API Reference: https://platform.openai.com/docs/api-reference/audio/createTranscription

// =============================================================================
// Response types: the shared OpenAI-compatible set, under this client's names
// =============================================================================

pub use waav_segmented_stt::vendor::openai::{
    DiarizedSegment, DiarizedSpeaker, DiarizedTranscriptionResponse, DiarizedWord,
    ErrorBody as OpenAIError, ErrorResponse as OpenAIErrorResponse,
    Segment as TranscriptionSegment, TranscriptionResponse, VerboseTranscriptionResponse,
    Word as TranscriptionWord,
};

// =============================================================================
// Parsed Response (unified type)
// =============================================================================

/// Unified transcription result that can represent any response format.
///
/// This enum provides a consistent interface regardless of the response
/// format requested from the API.
#[derive(Debug, Clone)]
pub enum TranscriptionResult {
    /// Simple text response.
    Simple(TranscriptionResponse),
    /// Verbose response with metadata and timestamps.
    Verbose(VerboseTranscriptionResponse),
    /// Diarized response with speaker identification.
    Diarized(DiarizedTranscriptionResponse),
    /// Plain text (for text, srt, vtt formats).
    PlainText(String),
}

impl TranscriptionResult {
    /// Get the full transcript text regardless of format.
    pub fn text(&self) -> &str {
        match self {
            Self::Simple(r) => &r.text,
            Self::Verbose(r) => &r.text,
            Self::Diarized(r) => &r.text,
            Self::PlainText(s) => s,
        }
    }

    /// Get word-level timestamps if available (non-diarized).
    pub fn words(&self) -> Option<&[TranscriptionWord]> {
        match self {
            Self::Verbose(r) if !r.words.is_empty() => Some(&r.words),
            _ => None,
        }
    }

    /// Get diarized word-level information with speaker attribution.
    pub fn diarized_words(&self) -> Option<&[DiarizedWord]> {
        match self {
            Self::Diarized(r) if !r.words.is_empty() => Some(&r.words),
            _ => None,
        }
    }

    /// Get segment-level timestamps if available (non-diarized).
    pub fn segments(&self) -> Option<&[TranscriptionSegment]> {
        match self {
            Self::Verbose(r) if !r.segments.is_empty() => Some(&r.segments),
            _ => None,
        }
    }

    /// Get diarized segment-level information with speaker attribution.
    pub fn diarized_segments(&self) -> Option<&[DiarizedSegment]> {
        match self {
            Self::Diarized(r) if !r.segments.is_empty() => Some(&r.segments),
            _ => None,
        }
    }

    /// Get speaker information from diarization.
    pub fn speakers(&self) -> Option<&[DiarizedSpeaker]> {
        match self {
            Self::Diarized(r) if !r.speakers.is_empty() => Some(&r.speakers),
            _ => None,
        }
    }

    /// Get the detected language if available.
    pub fn language(&self) -> Option<&str> {
        match self {
            Self::Verbose(r) => r.language.as_deref(),
            Self::Diarized(r) => r.language.as_deref(),
            _ => None,
        }
    }

    /// Get the duration if available.
    pub fn duration(&self) -> Option<f64> {
        match self {
            Self::Verbose(r) => r.duration,
            Self::Diarized(r) => r.duration,
            _ => None,
        }
    }

    /// Check if this result contains diarization data.
    pub fn has_diarization(&self) -> bool {
        matches!(self, Self::Diarized(_))
    }

    /// Calculate confidence from average log probability of segments.
    ///
    /// Returns a value between 0.0 and 1.0.
    /// If no log probabilities are available, returns 1.0 (full confidence).
    pub fn confidence(&self) -> f32 {
        match self {
            Self::Verbose(r) if !r.segments.is_empty() => Self::calculate_confidence_from_logprobs(
                r.segments.iter().filter_map(|seg| seg.avg_logprob),
            ),
            Self::Diarized(r) if !r.segments.is_empty() => {
                Self::calculate_confidence_from_logprobs(
                    r.segments.iter().filter_map(|seg| seg.avg_logprob),
                )
            }
            _ => 1.0, // Default to high confidence if no log probs available
        }
    }

    /// Helper function to calculate confidence from log probabilities.
    fn calculate_confidence_from_logprobs(logprobs: impl Iterator<Item = f64>) -> f32 {
        let (sum, count) =
            logprobs.fold((0.0, 0), |(sum, count), logprob| (sum + logprob, count + 1));

        if count > 0 {
            // Convert log probability to linear probability
            // avg_logprob is typically in range [-1, 0] for good transcriptions
            // We map this to [0, 1] confidence score
            let avg = sum / count as f64;
            // Clamp to reasonable range and convert
            let confidence = (avg + 1.0).clamp(0.0, 1.0);
            confidence as f32
        } else {
            1.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_response_parsing() {
        let json = r#"{"text": "Hello world"}"#;
        let response: TranscriptionResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.text, "Hello world");
    }

    #[test]
    fn test_verbose_response_parsing() {
        let json = r#"{
            "text": "Hello world",
            "language": "en",
            "duration": 2.5,
            "segments": [
                {
                    "id": 0,
                    "start": 0.0,
                    "end": 2.5,
                    "text": "Hello world",
                    "tokens": [1, 2, 3],
                    "avg_logprob": -0.25
                }
            ],
            "words": [
                {"word": "Hello", "start": 0.0, "end": 1.0},
                {"word": "world", "start": 1.1, "end": 2.5}
            ]
        }"#;

        let response: VerboseTranscriptionResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.text, "Hello world");
        assert_eq!(response.language, Some("en".to_string()));
        assert_eq!(response.duration, Some(2.5));
        assert_eq!(response.segments.len(), 1);
        assert_eq!(response.words.len(), 2);
        assert_eq!(response.words[0].word, "Hello");
        assert_eq!(response.words[1].word, "world");
    }

    #[test]
    fn test_error_response_parsing() {
        let json = r#"{
            "error": {
                "message": "Invalid API key",
                "type": "invalid_request_error",
                "param": null,
                "code": "invalid_api_key"
            }
        }"#;

        let response: OpenAIErrorResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.error.message, "Invalid API key");
        assert_eq!(response.error.error_type, "invalid_request_error");
        assert_eq!(response.error.code, Some("invalid_api_key".to_string()));
    }

    #[test]
    fn test_transcription_result_text() {
        let simple = TranscriptionResult::Simple(TranscriptionResponse {
            text: "Hello".to_string(),
            ..Default::default()
        });
        assert_eq!(simple.text(), "Hello");

        let verbose = TranscriptionResult::Verbose(VerboseTranscriptionResponse {
            text: "World".to_string(),
            language: None,
            duration: None,
            segments: vec![],
            words: vec![],
            ..Default::default()
        });
        assert_eq!(verbose.text(), "World");

        let plain = TranscriptionResult::PlainText("Plain text".to_string());
        assert_eq!(plain.text(), "Plain text");
    }

    #[test]
    fn test_transcription_result_confidence() {
        // Test with segments that have avg_logprob
        let verbose = TranscriptionResult::Verbose(VerboseTranscriptionResponse {
            text: "Test".to_string(),
            language: None,
            duration: None,
            segments: vec![TranscriptionSegment {
                id: 0,
                start: 0.0,
                end: 1.0,
                text: "Test".to_string(),
                tokens: vec![],
                avg_logprob: Some(-0.2), // Should map to ~0.8 confidence
                compression_ratio: None,
                no_speech_prob: None,
                temperature: None,
                seek: None,
            }],
            words: vec![],
            ..Default::default()
        });

        let confidence = verbose.confidence();
        assert!(confidence > 0.7 && confidence < 0.9);

        // Test default confidence when no log probs
        let simple = TranscriptionResult::Simple(TranscriptionResponse {
            text: "Test".to_string(),
            ..Default::default()
        });
        assert_eq!(simple.confidence(), 1.0);
    }

    #[test]
    fn test_openai_error_display() {
        let error = OpenAIError {
            message: "Rate limit exceeded".to_string(),
            error_type: "rate_limit_error".to_string(),
            param: None,
            code: None,
        };
        assert_eq!(
            format!("{}", error),
            "Rate limit exceeded (rate_limit_error)"
        );
    }

    // =========================================================================
    // Diarization Tests
    // =========================================================================

    #[test]
    fn test_diarized_response_parsing() {
        let json = r#"{
            "text": "Hello from speaker one. Hi from speaker two.",
            "language": "en",
            "duration": 5.0,
            "speakers": [
                {"id": "speaker_0", "name": "Alice", "confidence": 0.95},
                {"id": "speaker_1", "confidence": 0.87}
            ],
            "segments": [
                {
                    "id": 0,
                    "start": 0.0,
                    "end": 2.5,
                    "text": "Hello from speaker one.",
                    "speaker": "speaker_0",
                    "avg_logprob": -0.15
                },
                {
                    "id": 1,
                    "start": 2.6,
                    "end": 5.0,
                    "text": "Hi from speaker two.",
                    "speaker": "speaker_1",
                    "avg_logprob": -0.20
                }
            ],
            "words": [
                {"word": "Hello", "start": 0.0, "end": 0.5, "speaker": "speaker_0"},
                {"word": "from", "start": 0.6, "end": 0.9, "speaker": "speaker_0"},
                {"word": "speaker", "start": 1.0, "end": 1.5, "speaker": "speaker_0"},
                {"word": "one", "start": 1.6, "end": 2.0, "speaker": "speaker_0"},
                {"word": "Hi", "start": 2.6, "end": 2.9, "speaker": "speaker_1"},
                {"word": "from", "start": 3.0, "end": 3.3, "speaker": "speaker_1"},
                {"word": "speaker", "start": 3.4, "end": 3.9, "speaker": "speaker_1"},
                {"word": "two", "start": 4.0, "end": 4.5, "speaker": "speaker_1"}
            ]
        }"#;

        let response: DiarizedTranscriptionResponse = serde_json::from_str(json).unwrap();
        assert_eq!(
            response.text,
            "Hello from speaker one. Hi from speaker two."
        );
        assert_eq!(response.language, Some("en".to_string()));
        assert_eq!(response.duration, Some(5.0));
        assert_eq!(response.speakers.len(), 2);
        assert_eq!(response.segments.len(), 2);
        assert_eq!(response.words.len(), 8);

        // Check speaker info
        assert_eq!(response.speakers[0].id, "speaker_0");
        assert_eq!(response.speakers[0].name, Some("Alice".to_string()));
        assert_eq!(response.speakers[0].confidence, Some(0.95));
        assert_eq!(response.speakers[1].id, "speaker_1");
        assert!(response.speakers[1].name.is_none());

        // Check segment speaker attribution
        assert_eq!(response.segments[0].speaker, Some("speaker_0".to_string()));
        assert_eq!(response.segments[1].speaker, Some("speaker_1".to_string()));

        // Check word speaker attribution
        assert_eq!(response.words[0].speaker, Some("speaker_0".to_string()));
        assert_eq!(response.words[4].speaker, Some("speaker_1".to_string()));
    }

    #[test]
    fn test_diarized_response_minimal() {
        // Test parsing with minimal required fields
        let json = r#"{
            "text": "Hello world"
        }"#;

        let response: DiarizedTranscriptionResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.text, "Hello world");
        assert!(response.language.is_none());
        assert!(response.duration.is_none());
        assert!(response.speakers.is_empty());
        assert!(response.segments.is_empty());
        assert!(response.words.is_empty());
    }

    #[test]
    fn test_transcription_result_diarized_text() {
        let diarized = TranscriptionResult::Diarized(DiarizedTranscriptionResponse {
            text: "Diarized text".to_string(),
            language: Some("en".to_string()),
            duration: Some(3.0),
            speakers: vec![],
            segments: vec![],
            words: vec![],
        });
        assert_eq!(diarized.text(), "Diarized text");
    }

    #[test]
    fn test_transcription_result_diarized_language() {
        let diarized = TranscriptionResult::Diarized(DiarizedTranscriptionResponse {
            text: "Test".to_string(),
            language: Some("es".to_string()),
            duration: None,
            speakers: vec![],
            segments: vec![],
            words: vec![],
        });
        assert_eq!(diarized.language(), Some("es"));
    }

    #[test]
    fn test_transcription_result_diarized_duration() {
        let diarized = TranscriptionResult::Diarized(DiarizedTranscriptionResponse {
            text: "Test".to_string(),
            language: None,
            duration: Some(10.5),
            speakers: vec![],
            segments: vec![],
            words: vec![],
        });
        assert_eq!(diarized.duration(), Some(10.5));
    }

    #[test]
    fn test_transcription_result_has_diarization() {
        let simple = TranscriptionResult::Simple(TranscriptionResponse {
            text: "Test".to_string(),
            ..Default::default()
        });
        assert!(!simple.has_diarization());

        let verbose = TranscriptionResult::Verbose(VerboseTranscriptionResponse {
            text: "Test".to_string(),
            language: None,
            duration: None,
            segments: vec![],
            words: vec![],
            ..Default::default()
        });
        assert!(!verbose.has_diarization());

        let diarized = TranscriptionResult::Diarized(DiarizedTranscriptionResponse {
            text: "Test".to_string(),
            language: None,
            duration: None,
            speakers: vec![],
            segments: vec![],
            words: vec![],
        });
        assert!(diarized.has_diarization());
    }

    #[test]
    fn test_transcription_result_speakers() {
        let diarized = TranscriptionResult::Diarized(DiarizedTranscriptionResponse {
            text: "Test".to_string(),
            language: None,
            duration: None,
            speakers: vec![
                DiarizedSpeaker {
                    id: "speaker_0".to_string(),
                    name: Some("Alice".to_string()),
                    confidence: Some(0.95),
                },
                DiarizedSpeaker {
                    id: "speaker_1".to_string(),
                    name: None,
                    confidence: None,
                },
            ],
            segments: vec![],
            words: vec![],
        });

        let speakers = diarized.speakers().unwrap();
        assert_eq!(speakers.len(), 2);
        assert_eq!(speakers[0].id, "speaker_0");
        assert_eq!(speakers[0].name, Some("Alice".to_string()));
        assert_eq!(speakers[1].id, "speaker_1");
    }

    #[test]
    fn test_transcription_result_diarized_words() {
        let diarized = TranscriptionResult::Diarized(DiarizedTranscriptionResponse {
            text: "Hello world".to_string(),
            language: None,
            duration: None,
            speakers: vec![],
            segments: vec![],
            words: vec![
                DiarizedWord {
                    word: "Hello".to_string(),
                    start: 0.0,
                    end: 0.5,
                    speaker: Some("speaker_0".to_string()),
                    logprob: Some(-0.1),
                },
                DiarizedWord {
                    word: "world".to_string(),
                    start: 0.6,
                    end: 1.0,
                    speaker: Some("speaker_0".to_string()),
                    logprob: Some(-0.2),
                },
            ],
        });

        let words = diarized.diarized_words().unwrap();
        assert_eq!(words.len(), 2);
        assert_eq!(words[0].word, "Hello");
        assert_eq!(words[0].speaker, Some("speaker_0".to_string()));
        assert_eq!(words[0].logprob, Some(-0.1));
    }

    #[test]
    fn test_transcription_result_diarized_segments() {
        let diarized = TranscriptionResult::Diarized(DiarizedTranscriptionResponse {
            text: "Hello. World.".to_string(),
            language: None,
            duration: None,
            speakers: vec![],
            segments: vec![
                DiarizedSegment {
                    id: 0,
                    start: 0.0,
                    end: 1.0,
                    text: "Hello.".to_string(),
                    speaker: Some("speaker_0".to_string()),
                    avg_logprob: Some(-0.15),
                    no_speech_prob: None,
                    tokens: vec![],
                    temperature: None,
                    compression_ratio: None,
                    seek: None,
                },
                DiarizedSegment {
                    id: 1,
                    start: 1.1,
                    end: 2.0,
                    text: "World.".to_string(),
                    speaker: Some("speaker_1".to_string()),
                    avg_logprob: Some(-0.25),
                    no_speech_prob: None,
                    tokens: vec![],
                    temperature: None,
                    compression_ratio: None,
                    seek: None,
                },
            ],
            words: vec![],
        });

        let segments = diarized.diarized_segments().unwrap();
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].text, "Hello.");
        assert_eq!(segments[0].speaker, Some("speaker_0".to_string()));
        assert_eq!(segments[1].text, "World.");
        assert_eq!(segments[1].speaker, Some("speaker_1".to_string()));
    }

    #[test]
    fn test_diarized_confidence_calculation() {
        let diarized = TranscriptionResult::Diarized(DiarizedTranscriptionResponse {
            text: "Test".to_string(),
            language: None,
            duration: None,
            speakers: vec![],
            segments: vec![DiarizedSegment {
                id: 0,
                start: 0.0,
                end: 1.0,
                text: "Test".to_string(),
                speaker: None,
                avg_logprob: Some(-0.2), // Should map to ~0.8 confidence
                no_speech_prob: None,
                tokens: vec![],
                temperature: None,
                compression_ratio: None,
                seek: None,
            }],
            words: vec![],
        });

        let confidence = diarized.confidence();
        assert!(confidence > 0.7 && confidence < 0.9);
    }
}
