//! Message types for Groq STT (Whisper) API responses.
//!
//! This module contains serde types for parsing API responses,
//! including simple JSON, verbose JSON with timestamps, and error responses.

use serde::{Deserialize, Serialize};

// =============================================================================
// Simple Transcription Response
// =============================================================================

/// Simple JSON transcription response.
///
/// Returned when `response_format` is `json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptionResponse {
    /// The transcribed text.
    pub text: String,

    /// Groq-specific metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x_groq: Option<GroqMetadata>,
}

/// Groq-specific metadata in responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroqMetadata {
    /// Unique request ID for debugging/tracking.
    pub id: String,
}

// =============================================================================
// Verbose Transcription Response
// =============================================================================

/// Verbose JSON transcription response with timestamps and metadata.
///
/// Returned when `response_format` is `verbose_json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerboseTranscriptionResponse {
    /// The full transcribed text.
    pub text: String,

    /// Detected language of the audio.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,

    /// Total duration of the audio in seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,

    /// Transcription segments with timestamps.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub segments: Vec<Segment>,

    /// Word-level timestamps (if requested).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub words: Vec<Word>,

    /// Groq-specific metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x_groq: Option<GroqMetadata>,
}

/// A transcription segment with timing and confidence information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Segment {
    /// Segment index (0-based).
    pub id: u32,

    /// Audio position in milliseconds (seek position).
    #[serde(default)]
    pub seek: u32,

    /// Start time in seconds.
    pub start: f64,

    /// End time in seconds.
    pub end: f64,

    /// Transcribed text for this segment.
    pub text: String,

    /// Average log probability (confidence metric).
    /// Closer to 0 = higher confidence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_logprob: Option<f64>,

    /// Probability that this segment contains no speech.
    /// 0-1 scale; higher = less likely to be speech.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_speech_prob: Option<f64>,

    /// Compression ratio indicator.
    /// Unusual values suggest clarity issues.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compression_ratio: Option<f64>,

    /// Token IDs for this segment.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tokens: Vec<i64>,

    /// Temperature used for this segment.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
}

impl Segment {
    /// Calculate confidence score (0.0 to 1.0) from avg_logprob.
    ///
    /// The log probability is typically negative, with values closer to 0
    /// indicating higher confidence.
    pub fn confidence(&self) -> f64 {
        self.avg_logprob
            .map(|lp| {
                // Convert log probability to a 0-1 confidence score
                // avg_logprob typically ranges from -1.0 (high confidence) to -10.0 (low confidence)
                let normalized = (lp + 1.0).clamp(-1.0, 0.0);
                1.0 + normalized // Maps to 0.0 - 1.0
            })
            .unwrap_or(0.8) // Default to reasonable confidence if not available
    }

    /// Check if this segment likely contains actual speech.
    ///
    /// Returns false if no_speech_prob is high (> 0.5).
    pub fn is_speech(&self) -> bool {
        self.no_speech_prob.map(|p| p < 0.5).unwrap_or(true)
    }
}

/// Word-level timing information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Word {
    /// The word text.
    pub word: String,

    /// Start time in seconds.
    pub start: f64,

    /// End time in seconds.
    pub end: f64,
}

// =============================================================================
// Error Response
// =============================================================================

/// Error response from Groq API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroqErrorResponse {
    /// The error details.
    pub error: GroqError,
}

/// Error details from Groq API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroqError {
    /// Human-readable error message.
    pub message: String,

    /// Error type classification.
    #[serde(rename = "type")]
    pub error_type: String,

    /// Optional error code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,

    /// Optional parameter that caused the error.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub param: Option<String>,
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
    /// For simple responses, returns a default value.
    pub fn confidence(&self) -> f64 {
        match self {
            Self::Simple(_) => 0.9, // High confidence assumed for simple format
            Self::Verbose(r) => {
                if r.segments.is_empty() {
                    0.9
                } else {
                    let total: f64 = r.segments.iter().map(|s| s.confidence()).sum();
                    total / r.segments.len() as f64
                }
            }
            Self::PlainText(_) => 0.9,
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

// =============================================================================
// WAV File Generation
// =============================================================================

/// WAV file generation utilities.
///
/// Since Groq's API expects audio files, we need to package raw PCM
/// data into a WAV container.
pub mod wav {
    /// Create a WAV file from raw PCM data.
    ///
    /// # Arguments
    /// * `pcm_data` - Raw PCM samples (16-bit signed little-endian)
    /// * `sample_rate` - Sample rate in Hz (typically 16000)
    /// * `channels` - Number of audio channels (1 for mono, 2 for stereo)
    ///
    /// # Returns
    /// A Vec<u8> containing the complete WAV file.
    pub fn create_wav(pcm_data: &[u8], sample_rate: u32, channels: u16) -> Vec<u8> {
        let bits_per_sample: u16 = 16;
        let byte_rate = sample_rate * u32::from(channels) * u32::from(bits_per_sample) / 8;
        let block_align = channels * bits_per_sample / 8;
        let data_size = pcm_data.len() as u32;
        let file_size = 36 + data_size;

        let mut wav = Vec::with_capacity(44 + pcm_data.len());

        // RIFF header
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&file_size.to_le_bytes());
        wav.extend_from_slice(b"WAVE");

        // fmt subchunk
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes()); // Subchunk1Size (16 for PCM)
        wav.extend_from_slice(&1u16.to_le_bytes()); // AudioFormat (1 = PCM)
        wav.extend_from_slice(&channels.to_le_bytes());
        wav.extend_from_slice(&sample_rate.to_le_bytes());
        wav.extend_from_slice(&byte_rate.to_le_bytes());
        wav.extend_from_slice(&block_align.to_le_bytes());
        wav.extend_from_slice(&bits_per_sample.to_le_bytes());

        // data subchunk
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_size.to_le_bytes());
        wav.extend_from_slice(pcm_data);

        wav
    }

    /// Get the WAV header size (44 bytes).
    pub const HEADER_SIZE: usize = 44;
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
            seek: 0,
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
            seek: 0,
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
        });
        assert_eq!(verbose.text(), "World");

        let plain = TranscriptionResult::PlainText("Plain".to_string());
        assert_eq!(plain.text(), "Plain");
    }

    #[test]
    fn test_wav_creation() {
        let pcm_data = vec![0u8; 100];
        let wav = wav::create_wav(&pcm_data, 16000, 1);

        // Check WAV header
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(&wav[36..40], b"data");

        // Check total size
        assert_eq!(wav.len(), wav::HEADER_SIZE + pcm_data.len());
    }

    #[test]
    fn test_wav_header_size() {
        assert_eq!(wav::HEADER_SIZE, 44);
    }

    #[test]
    fn test_transcription_result_confidence() {
        let verbose = TranscriptionResult::Verbose(VerboseTranscriptionResponse {
            text: "Test".to_string(),
            language: None,
            duration: None,
            segments: vec![
                Segment {
                    id: 0,
                    seek: 0,
                    start: 0.0,
                    end: 1.0,
                    text: "Test".to_string(),
                    avg_logprob: Some(-0.3),
                    no_speech_prob: None,
                    compression_ratio: None,
                    tokens: vec![],
                    temperature: None,
                }
            ],
            words: vec![],
            x_groq: None,
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
