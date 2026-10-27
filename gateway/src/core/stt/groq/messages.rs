//! Message types for Groq STT (Whisper) API responses.
//!
//! This module contains serde types for parsing API responses,
//! including simple JSON, verbose JSON with timestamps, and error responses.

/// Default confidence value when actual confidence is unavailable.
/// Using 0.5 (neutral) to avoid overconfidence in systems that rely on this value.
/// This indicates "unknown confidence" rather than "high confidence".
pub const DEFAULT_UNKNOWN_CONFIDENCE: f64 = 0.5;

// =============================================================================
// Response types: the shared OpenAI-compatible set
// =============================================================================

pub use waav_segmented_stt::vendor::openai::{
    ErrorBody as GroqError, ErrorResponse as GroqErrorResponse, GroqMetadata, Segment,
    TranscriptionResponse, VerboseTranscriptionResponse, Word,
};

/// Groq's reading of a segment: a confidence from its `avg_logprob`, and whether it holds speech.
pub trait SegmentScore {
    /// Calculate confidence score (0.0 to 1.0) from avg_logprob.
    fn confidence(&self) -> f64;
    /// Whether this segment likely contains speech: false when `no_speech_prob` is above 0.5.
    fn is_speech(&self) -> bool;
}

impl SegmentScore for Segment {
    /// Calculate confidence score (0.0 to 1.0) from avg_logprob.
    ///
    /// The log probability is typically negative, with values closer to 0
    /// indicating higher confidence. Based on empirical observation:
    /// - avg_logprob around -0.2 to -0.5 = very high confidence
    /// - avg_logprob around -0.5 to -1.0 = high confidence
    /// - avg_logprob around -1.0 to -2.0 = medium confidence
    /// - avg_logprob below -2.0 = low confidence
    ///
    /// This implementation uses exponential mapping to preserve precision
    /// across the full range of log probabilities.
    fn confidence(&self) -> f64 {
        self.avg_logprob
            .map(|lp| {
                // Use exponential transformation: e^(logprob) gives probability
                // Then scale to 0-1 range with reasonable bounds
                // avg_logprob of 0 -> confidence 1.0
                // avg_logprob of -1 -> confidence ~0.37
                // avg_logprob of -2 -> confidence ~0.14
                // avg_logprob of -5 -> confidence ~0.007
                //
                // We use a slightly modified formula to keep confidence
                // in a more useful range (0.1 to 1.0):
                // confidence = max(0.1, e^(logprob * 0.5))
                //
                // This gives:
                // avg_logprob of 0 -> confidence 1.0
                // avg_logprob of -0.5 -> confidence ~0.78
                // avg_logprob of -1.0 -> confidence ~0.61
                // avg_logprob of -2.0 -> confidence ~0.37
                // avg_logprob of -5.0 -> confidence ~0.1 (clamped)
                let raw_confidence = (lp * 0.5).exp();
                raw_confidence.clamp(0.1, 1.0)
            })
            .unwrap_or(DEFAULT_UNKNOWN_CONFIDENCE)
    }

    /// Check if this segment likely contains actual speech.
    ///
    /// Returns false if no_speech_prob is high (> 0.5).
    fn is_speech(&self) -> bool {
        self.no_speech_prob.map(|p| p < 0.5).unwrap_or(true)
    }
}

// =============================================================================
// Unified Result Type
// =============================================================================

/// Unified transcription result that can hold any response format.
#[derive(Debug, Clone)]
pub enum TranscriptionResult {
    /// Simple JSON response with just text.
    Simple(TranscriptionResponse),
    /// Verbose JSON response with timestamps.
    Verbose(VerboseTranscriptionResponse),
    /// Plain text response.
    PlainText(String),
}

impl TranscriptionResult {
    /// Get the transcribed text from any response format.
    pub fn text(&self) -> &str {
        match self {
            Self::Simple(r) => &r.text,
            Self::Verbose(r) => &r.text,
            Self::PlainText(t) => t,
        }
    }

    /// Get the overall confidence score (0.0 to 1.0).
    ///
    /// For verbose responses, this is the average confidence across segments.
    /// For simple/text responses, returns `DEFAULT_UNKNOWN_CONFIDENCE` since
    /// these formats don't include confidence information.
    pub fn confidence(&self) -> f64 {
        match self {
            Self::Simple(_) => DEFAULT_UNKNOWN_CONFIDENCE, // No confidence info available
            Self::Verbose(r) => {
                if r.segments.is_empty() {
                    DEFAULT_UNKNOWN_CONFIDENCE // No segments to calculate from
                } else {
                    // Calculate weighted average based on segment duration
                    let total_duration: f64 = r.segments.iter().map(|s| s.end - s.start).sum();
                    if total_duration > 0.0 {
                        // Duration-weighted average confidence
                        let weighted_sum: f64 = r
                            .segments
                            .iter()
                            .map(|s| s.confidence() * (s.end - s.start))
                            .sum();
                        weighted_sum / total_duration
                    } else {
                        // Fallback to simple average if durations are zero
                        let total: f64 = r.segments.iter().map(|s| s.confidence()).sum();
                        total / r.segments.len() as f64
                    }
                }
            }
            Self::PlainText(_) => DEFAULT_UNKNOWN_CONFIDENCE, // No confidence info available
        }
    }

    /// Get the detected language (if available).
    pub fn language(&self) -> Option<&str> {
        match self {
            Self::Simple(_) => None,
            Self::Verbose(r) => r.language.as_deref(),
            Self::PlainText(_) => None,
        }
    }

    /// Get the audio duration in seconds (if available).
    pub fn duration(&self) -> Option<f64> {
        match self {
            Self::Simple(_) => None,
            Self::Verbose(r) => r.duration,
            Self::PlainText(_) => None,
        }
    }

    /// Get word-level timestamps (if available).
    pub fn words(&self) -> Option<&[Word]> {
        match self {
            Self::Simple(_) => None,
            Self::Verbose(r) if !r.words.is_empty() => Some(&r.words),
            Self::Verbose(_) => None,
            Self::PlainText(_) => None,
        }
    }

    /// Get segment-level timestamps (if available).
    pub fn segments(&self) -> Option<&[Segment]> {
        match self {
            Self::Simple(_) => None,
            Self::Verbose(r) if !r.segments.is_empty() => Some(&r.segments),
            Self::Verbose(_) => None,
            Self::PlainText(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_response_parsing() {
        let json = r#"{
            "text": "Hello world",
            "x_groq": {"id": "req_123"}
        }"#;

        let response: TranscriptionResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.text, "Hello world");
        assert_eq!(response.x_groq.as_ref().unwrap().id, "req_123");
    }

    #[test]
    fn test_verbose_response_parsing() {
        let json = r#"{
            "text": "Hello world",
            "language": "en",
            "duration": 2.5,
            "segments": [{
                "id": 0,
                "seek": 0,
                "start": 0.0,
                "end": 2.5,
                "text": "Hello world",
                "avg_logprob": -0.5,
                "no_speech_prob": 0.01,
                "compression_ratio": 1.2,
                "tokens": [1, 2, 3],
                "temperature": 0.0
            }],
            "words": [{
                "word": "Hello",
                "start": 0.0,
                "end": 1.0
            }, {
                "word": "world",
                "start": 1.0,
                "end": 2.5
            }]
        }"#;

        let response: VerboseTranscriptionResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.text, "Hello world");
        assert_eq!(response.language.as_deref(), Some("en"));
        assert_eq!(response.duration, Some(2.5));
        assert_eq!(response.segments.len(), 1);
        assert_eq!(response.words.len(), 2);
    }

    #[test]
    fn test_segment_confidence() {
        let segment = Segment {
            id: 0,
            seek: Some(0),
            start: 0.0,
            end: 1.0,
            text: "Test".to_string(),
            avg_logprob: Some(-0.5),
            no_speech_prob: Some(0.01),
            compression_ratio: None,
            tokens: vec![],
            temperature: None,
        };

        let confidence = segment.confidence();
        assert!(confidence > 0.0 && confidence <= 1.0);
        assert!(segment.is_speech());
    }

    #[test]
    fn test_segment_no_speech() {
        let segment = Segment {
            id: 0,
            seek: Some(0),
            start: 0.0,
            end: 1.0,
            text: "".to_string(),
            avg_logprob: Some(-5.0),
            no_speech_prob: Some(0.8), // High no-speech probability
            compression_ratio: None,
            tokens: vec![],
            temperature: None,
        };

        assert!(!segment.is_speech());
    }

    #[test]
    fn test_error_response_parsing() {
        let json = r#"{
            "error": {
                "message": "Rate limit exceeded",
                "type": "rate_limit_error",
                "code": "rate_limit_exceeded"
            }
        }"#;

        let error: GroqErrorResponse = serde_json::from_str(json).unwrap();
        assert_eq!(error.error.message, "Rate limit exceeded");
        assert_eq!(error.error.error_type, "rate_limit_error");
        assert_eq!(error.error.code, Some("rate_limit_exceeded".to_string()));
    }

    #[test]
    fn test_transcription_result_text() {
        let simple = TranscriptionResult::Simple(TranscriptionResponse {
            text: "Hello".to_string(),
            x_groq: None,
        });
        assert_eq!(simple.text(), "Hello");

        let verbose = TranscriptionResult::Verbose(VerboseTranscriptionResponse {
            text: "World".to_string(),
            language: Some("en".to_string()),
            duration: Some(1.0),
            segments: vec![],
            words: vec![],
            x_groq: None,
            ..Default::default()
        });
        assert_eq!(verbose.text(), "World");

        let plain = TranscriptionResult::PlainText("Plain".to_string());
        assert_eq!(plain.text(), "Plain");
    }

    #[test]
    fn test_transcription_result_confidence() {
        let verbose = TranscriptionResult::Verbose(VerboseTranscriptionResponse {
            text: "Test".to_string(),
            language: None,
            duration: None,
            segments: vec![Segment {
                id: 0,
                seek: Some(0),
                start: 0.0,
                end: 1.0,
                text: "Test".to_string(),
                avg_logprob: Some(-0.3),
                no_speech_prob: None,
                compression_ratio: None,
                tokens: vec![],
                temperature: None,
            }],
            words: vec![],
            x_groq: None,
            ..Default::default()
        });

        let confidence = verbose.confidence();
        assert!(confidence > 0.0 && confidence <= 1.0);
    }

    #[test]
    fn test_word_timing() {
        let word = Word {
            word: "Hello".to_string(),
            start: 0.5,
            end: 1.2,
        };

        assert_eq!(word.word, "Hello");
        assert_eq!(word.start, 0.5);
        assert_eq!(word.end, 1.2);
    }
}
