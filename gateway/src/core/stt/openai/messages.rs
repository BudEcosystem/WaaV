//! Message types for OpenAI STT (Whisper) API.
//!
//! This module contains request and response types for the OpenAI
//! Audio Transcription API (Whisper).
//!
//! API Reference: https://platform.openai.com/docs/api-reference/audio/createTranscription

use serde::{Deserialize, Serialize};

// =============================================================================
// Response Types
// =============================================================================

/// Simple transcription response (json format).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TranscriptionResponse {
    /// The transcribed text.
    pub text: String,
}

/// Verbose transcription response (verbose_json format).
///
/// Contains detailed information including word-level timestamps,
/// segment information, and metadata.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VerboseTranscriptionResponse {
    /// The transcribed text (full transcript).
    pub text: String,

    /// The language of the audio (ISO-639-1 code).
    #[serde(default)]
    pub language: Option<String>,

    /// Duration of the audio in seconds.
    #[serde(default)]
    pub duration: Option<f64>,

    /// Transcription segments with timing information.
    #[serde(default)]
    pub segments: Vec<TranscriptionSegment>,

    /// Word-level timing information (if requested).
    #[serde(default)]
    pub words: Vec<TranscriptionWord>,
}

/// A segment of transcribed text with timing.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TranscriptionSegment {
    /// Segment ID (0-indexed).
    pub id: i32,

    /// Start time of the segment in seconds.
    pub start: f64,

    /// End time of the segment in seconds.
    pub end: f64,

    /// Transcribed text for this segment.
    pub text: String,

    /// Token IDs for this segment.
    #[serde(default)]
    pub tokens: Vec<i32>,

    /// Average log probability of tokens.
    #[serde(default)]
    pub avg_logprob: Option<f64>,

    /// Compression ratio of the segment.
    #[serde(default)]
    pub compression_ratio: Option<f64>,

    /// Probability that this segment is not speech.
    #[serde(default)]
    pub no_speech_prob: Option<f64>,

    /// Temperature used for this segment.
    #[serde(default)]
    pub temperature: Option<f64>,

    /// Seek position in the audio.
    #[serde(default)]
    pub seek: Option<i32>,
}

/// A word with timing information.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TranscriptionWord {
    /// The word text.
    pub word: String,

    /// Start time of the word in seconds.
    pub start: f64,

    /// End time of the word in seconds.
    pub end: f64,
}

// =============================================================================
// Diarization Response Types (for diarized_json format)
// =============================================================================

/// Diarized transcription response (diarized_json format).
///
/// Contains transcription with speaker identification, enabling attribution
/// of speech to individual speakers. Available with gpt-4o-transcribe models.
///
/// API Reference: https://platform.openai.com/docs/api-reference/audio/createTranscription
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DiarizedTranscriptionResponse {
    /// The full transcribed text.
    pub text: String,

    /// The language of the audio (ISO-639-1 code).
    #[serde(default)]
    pub language: Option<String>,

    /// Duration of the audio in seconds.
    #[serde(default)]
    pub duration: Option<f64>,

    /// List of speakers identified in the audio.
    /// Each speaker has an ID and optional recognized name.
    #[serde(default)]
    pub speakers: Vec<DiarizedSpeaker>,

    /// Transcription segments with speaker attribution.
    #[serde(default)]
    pub segments: Vec<DiarizedSegment>,

    /// Word-level information with speaker attribution (if include_logprobs is true).
    #[serde(default)]
    pub words: Vec<DiarizedWord>,
}

/// Speaker information from diarization.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DiarizedSpeaker {
    /// Unique speaker identifier (e.g., "speaker_0", "speaker_1").
    pub id: String,

    /// Recognized speaker name (if known speaker references were provided).
    #[serde(default)]
    pub name: Option<String>,

    /// Confidence score for speaker identification (0.0 to 1.0).
    #[serde(default)]
    pub confidence: Option<f64>,
}

/// A segment of transcribed text with speaker attribution.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DiarizedSegment {
    /// Segment ID (0-indexed).
    pub id: i32,

    /// Start time of the segment in seconds.
    pub start: f64,

    /// End time of the segment in seconds.
    pub end: f64,

    /// Transcribed text for this segment.
    pub text: String,

    /// Speaker ID for this segment (e.g., "speaker_0").
    #[serde(default)]
    pub speaker: Option<String>,

    /// Average log probability of tokens in this segment.
    #[serde(default)]
    pub avg_logprob: Option<f64>,

    /// Probability that this segment contains no speech.
    #[serde(default)]
    pub no_speech_prob: Option<f64>,

    /// Token IDs for this segment.
    #[serde(default)]
    pub tokens: Vec<i32>,

    /// Temperature used for this segment.
    #[serde(default)]
    pub temperature: Option<f64>,

    /// Compression ratio of the segment.
    #[serde(default)]
    pub compression_ratio: Option<f64>,

    /// Seek position in the audio.
    #[serde(default)]
    pub seek: Option<i32>,
}

/// A word with timing and speaker information from diarization.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DiarizedWord {
    /// The word text.
    pub word: String,

    /// Start time of the word in seconds.
    pub start: f64,

    /// End time of the word in seconds.
    pub end: f64,

    /// Speaker ID for this word (e.g., "speaker_0").
    #[serde(default)]
    pub speaker: Option<String>,

    /// Log probability of the word (available when include_logprobs is true).
    #[serde(default)]
    pub logprob: Option<f64>,
}

// =============================================================================
// Error Types
// =============================================================================

/// OpenAI API error response.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OpenAIErrorResponse {
    /// Error details.
    pub error: OpenAIError,
}

/// OpenAI API error details.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OpenAIError {
    /// Human-readable error message.
    pub message: String,

    /// Error type identifier.
    #[serde(rename = "type")]
    pub error_type: String,

    /// Parameter that caused the error (if applicable).
    #[serde(default)]
    pub param: Option<String>,

    /// Error code (if applicable).
    #[serde(default)]
    pub code: Option<String>,
}

impl std::fmt::Display for OpenAIError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.message, self.error_type)
    }
}

impl std::error::Error for OpenAIError {}

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

// =============================================================================
// WAV Header Construction
// =============================================================================

/// Utility functions for constructing WAV files from raw PCM data.
///
/// OpenAI Whisper API requires properly formatted audio files.
/// This module helps package raw PCM audio into WAV format.
pub mod wav {
    /// Create a WAV file header for PCM audio.
    ///
    /// # Arguments
    /// * `data_size` - Size of the audio data in bytes
    /// * `sample_rate` - Sample rate in Hz (e.g., 16000)
    /// * `channels` - Number of channels (1 for mono, 2 for stereo)
    /// * `bits_per_sample` - Bits per sample (typically 16)
    ///
    /// # Returns
    /// A 44-byte WAV header
    pub fn create_header(
        data_size: u32,
        sample_rate: u32,
        channels: u16,
        bits_per_sample: u16,
    ) -> [u8; 44] {
        let byte_rate = sample_rate * u32::from(channels) * u32::from(bits_per_sample) / 8;
        let block_align = channels * bits_per_sample / 8;
        let file_size = 36 + data_size; // File size minus 8 bytes for RIFF header

        let mut header = [0u8; 44];

        // RIFF chunk descriptor
        header[0..4].copy_from_slice(b"RIFF");
        header[4..8].copy_from_slice(&file_size.to_le_bytes());
        header[8..12].copy_from_slice(b"WAVE");

        // fmt sub-chunk
        header[12..16].copy_from_slice(b"fmt ");
        header[16..20].copy_from_slice(&16u32.to_le_bytes()); // Subchunk1 size (16 for PCM)
        header[20..22].copy_from_slice(&1u16.to_le_bytes()); // Audio format (1 = PCM)
        header[22..24].copy_from_slice(&channels.to_le_bytes());
        header[24..28].copy_from_slice(&sample_rate.to_le_bytes());
        header[28..32].copy_from_slice(&byte_rate.to_le_bytes());
        header[32..34].copy_from_slice(&block_align.to_le_bytes());
        header[34..36].copy_from_slice(&bits_per_sample.to_le_bytes());

        // data sub-chunk
        header[36..40].copy_from_slice(b"data");
        header[40..44].copy_from_slice(&data_size.to_le_bytes());

        header
    }

    /// Create a complete WAV file from raw PCM data.
    ///
    /// # Arguments
    /// * `pcm_data` - Raw PCM audio data (16-bit signed little-endian)
    /// * `sample_rate` - Sample rate in Hz
    /// * `channels` - Number of channels
    ///
    /// # Returns
    /// Complete WAV file as bytes
    pub fn create_wav(pcm_data: &[u8], sample_rate: u32, channels: u16) -> Vec<u8> {
        let header = create_header(pcm_data.len() as u32, sample_rate, channels, 16);
        let mut wav = Vec::with_capacity(44 + pcm_data.len());
        wav.extend_from_slice(&header);
        wav.extend_from_slice(pcm_data);
        wav
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
        });
        assert_eq!(simple.text(), "Hello");

        let verbose = TranscriptionResult::Verbose(VerboseTranscriptionResponse {
            text: "World".to_string(),
            language: None,
            duration: None,
            segments: vec![],
            words: vec![],
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
        });

        let confidence = verbose.confidence();
        assert!(confidence > 0.7 && confidence < 0.9);

        // Test default confidence when no log probs
        let simple = TranscriptionResult::Simple(TranscriptionResponse {
            text: "Test".to_string(),
        });
        assert_eq!(simple.confidence(), 1.0);
    }

    #[test]
    fn test_wav_header_creation() {
        let header = wav::create_header(1000, 16000, 1, 16);
        assert_eq!(&header[0..4], b"RIFF");
        assert_eq!(&header[8..12], b"WAVE");
        assert_eq!(&header[12..16], b"fmt ");
        assert_eq!(&header[36..40], b"data");

        // Check sample rate (bytes 24-28)
        let sample_rate = u32::from_le_bytes([header[24], header[25], header[26], header[27]]);
        assert_eq!(sample_rate, 16000);
    }

    #[test]
    fn test_wav_creation() {
        let pcm_data = vec![0u8; 100];
        let wav = wav::create_wav(&pcm_data, 16000, 1);
        assert_eq!(wav.len(), 44 + 100); // Header + data
        assert_eq!(&wav[0..4], b"RIFF");
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
        });
        assert!(!simple.has_diarization());

        let verbose = TranscriptionResult::Verbose(VerboseTranscriptionResponse {
            text: "Test".to_string(),
            language: None,
            duration: None,
            segments: vec![],
            words: vec![],
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
