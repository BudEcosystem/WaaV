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
    /// Plain text (for text, srt, vtt formats).
    PlainText(String),
}

impl TranscriptionResult {
    /// Get the full transcript text regardless of format.
    pub fn text(&self) -> &str {
        match self {
            Self::Simple(r) => &r.text,
            Self::Verbose(r) => &r.text,
            Self::PlainText(s) => s,
        }
    }

    /// Get word-level timestamps if available.
    pub fn words(&self) -> Option<&[TranscriptionWord]> {
        match self {
            Self::Verbose(r) if !r.words.is_empty() => Some(&r.words),
            _ => None,
        }
    }

    /// Get segment-level timestamps if available.
    pub fn segments(&self) -> Option<&[TranscriptionSegment]> {
        match self {
            Self::Verbose(r) if !r.segments.is_empty() => Some(&r.segments),
            _ => None,
        }
    }

    /// Get the detected language if available.
    pub fn language(&self) -> Option<&str> {
        match self {
            Self::Verbose(r) => r.language.as_deref(),
            _ => None,
        }
    }

    /// Get the duration if available.
    pub fn duration(&self) -> Option<f64> {
        match self {
            Self::Verbose(r) => r.duration,
            _ => None,
        }
    }

    /// Calculate confidence from average log probability of segments.
    ///
    /// Returns a value between 0.0 and 1.0.
    /// If no log probabilities are available, returns 1.0 (full confidence).
    pub fn confidence(&self) -> f32 {
        match self {
            Self::Verbose(r) if !r.segments.is_empty() => {
                // Calculate average log probability across all segments
                let (sum, count) = r.segments.iter().fold((0.0, 0), |(sum, count), seg| {
                    if let Some(avg_logprob) = seg.avg_logprob {
                        (sum + avg_logprob, count + 1)
                    } else {
                        (sum, count)
                    }
                });

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
            _ => 1.0, // Default to high confidence if no log probs available
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
}
