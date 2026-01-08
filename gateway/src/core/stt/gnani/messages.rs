//! Gnani.ai STT Message Types
//!
//! Message types for Gnani's Speech-to-Text gRPC streaming API.
//! These types match Gnani's proto definitions for the Listener service.
//!
//! ## gRPC Service Definition
//!
//! ```protobuf
//! service Listener {
//!     rpc DoSpeechToText(stream SpeechChunk) returns (stream TranscriptChunk);
//! }
//! ```

use bytes::Bytes;
use serde::{Deserialize, Serialize};

/// Audio chunk sent to Gnani for streaming recognition.
///
/// This maps to the gRPC `SpeechChunk` message:
/// ```protobuf
/// message SpeechChunk {
///     bytes content = 1;
///     string token = 2;
///     string lang = 3;
///     string demo = 4;
/// }
/// ```
#[derive(Debug, Clone)]
pub struct SpeechChunk {
    /// Raw audio content (PCM16, 16kHz, mono)
    pub content: Bytes,
    /// Authentication token (passed in first chunk only for gRPC)
    pub token: String,
    /// Language code (e.g., "hi-IN", "en-IN")
    pub lang: String,
    /// Demo mode flag (empty string for production)
    pub demo: String,
}

impl SpeechChunk {
    /// Create a new speech chunk with audio data
    pub fn new(content: Bytes, token: &str, lang: &str) -> Self {
        Self {
            content,
            token: token.to_string(),
            lang: lang.to_string(),
            demo: String::new(),
        }
    }

    /// Create a speech chunk with only audio (for subsequent chunks)
    pub fn audio_only(content: Bytes) -> Self {
        Self {
            content,
            token: String::new(),
            lang: String::new(),
            demo: String::new(),
        }
    }

    /// Encode to protobuf wire format
    ///
    /// Field numbers:
    /// - content: 1 (bytes)
    /// - token: 2 (string)
    /// - lang: 3 (string)
    /// - demo: 4 (string)
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(self.content.len() + 100);

        // Field 1: content (bytes) - wire type 2 (length-delimited)
        if !self.content.is_empty() {
            buf.push(0x0a); // field 1, wire type 2
            encode_varint(&mut buf, self.content.len() as u64);
            buf.extend_from_slice(&self.content);
        }

        // Field 2: token (string) - wire type 2 (length-delimited)
        if !self.token.is_empty() {
            buf.push(0x12); // field 2, wire type 2
            encode_varint(&mut buf, self.token.len() as u64);
            buf.extend_from_slice(self.token.as_bytes());
        }

        // Field 3: lang (string) - wire type 2 (length-delimited)
        if !self.lang.is_empty() {
            buf.push(0x1a); // field 3, wire type 2
            encode_varint(&mut buf, self.lang.len() as u64);
            buf.extend_from_slice(self.lang.as_bytes());
        }

        // Field 4: demo (string) - wire type 2 (length-delimited)
        if !self.demo.is_empty() {
            buf.push(0x22); // field 4, wire type 2
            encode_varint(&mut buf, self.demo.len() as u64);
            buf.extend_from_slice(self.demo.as_bytes());
        }

        buf
    }
}

/// Transcription result from Gnani's streaming recognition.
///
/// This maps to the gRPC `TranscriptChunk` message:
/// ```protobuf
/// message TranscriptChunk {
///     string asr = 1;
///     string transcript = 2;
///     bool is_final = 3;
///     float confidence = 4;
///     string answer = 5;
///     string image_url = 6;
///     string image_yes_no = 7;
/// }
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TranscriptChunk {
    /// Raw ASR output
    #[serde(default)]
    pub asr: String,

    /// Processed transcript text
    #[serde(default)]
    pub transcript: String,

    /// Whether this is a final result
    #[serde(default)]
    pub is_final: bool,

    /// Confidence score (0.0 to 1.0)
    #[serde(default)]
    pub confidence: f32,

    /// Answer field (used for Q&A applications)
    #[serde(default)]
    pub answer: String,

    /// Image URL (if applicable)
    #[serde(default)]
    pub image_url: String,

    /// Yes/No classification result
    #[serde(default)]
    pub image_yes_no: String,
}

impl TranscriptChunk {
    /// Decode from protobuf wire format
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut chunk = TranscriptChunk::default();
        let mut pos = 0;

        while pos < buf.len() {
            let (field_tag, new_pos) = decode_varint(&buf[pos..])?;
            pos += new_pos;

            let field_number = field_tag >> 3;
            let wire_type = field_tag & 0x07;

            match (field_number, wire_type) {
                // Field 1: asr (string)
                (1, 2) => {
                    let (len, len_size) = decode_varint(&buf[pos..])?;
                    pos += len_size;
                    let end = pos + len as usize;
                    if end > buf.len() {
                        return Err(DecodeError::BufferTooShort);
                    }
                    chunk.asr = String::from_utf8_lossy(&buf[pos..end]).to_string();
                    pos = end;
                }
                // Field 2: transcript (string)
                (2, 2) => {
                    let (len, len_size) = decode_varint(&buf[pos..])?;
                    pos += len_size;
                    let end = pos + len as usize;
                    if end > buf.len() {
                        return Err(DecodeError::BufferTooShort);
                    }
                    chunk.transcript = String::from_utf8_lossy(&buf[pos..end]).to_string();
                    pos = end;
                }
                // Field 3: is_final (bool/varint)
                (3, 0) => {
                    let (value, size) = decode_varint(&buf[pos..])?;
                    pos += size;
                    chunk.is_final = value != 0;
                }
                // Field 4: confidence (float)
                (4, 5) => {
                    if pos + 4 > buf.len() {
                        return Err(DecodeError::BufferTooShort);
                    }
                    let bytes: [u8; 4] = buf[pos..pos + 4].try_into().unwrap();
                    chunk.confidence = f32::from_le_bytes(bytes);
                    pos += 4;
                }
                // Field 5: answer (string)
                (5, 2) => {
                    let (len, len_size) = decode_varint(&buf[pos..])?;
                    pos += len_size;
                    let end = pos + len as usize;
                    if end > buf.len() {
                        return Err(DecodeError::BufferTooShort);
                    }
                    chunk.answer = String::from_utf8_lossy(&buf[pos..end]).to_string();
                    pos = end;
                }
                // Field 6: image_url (string)
                (6, 2) => {
                    let (len, len_size) = decode_varint(&buf[pos..])?;
                    pos += len_size;
                    let end = pos + len as usize;
                    if end > buf.len() {
                        return Err(DecodeError::BufferTooShort);
                    }
                    chunk.image_url = String::from_utf8_lossy(&buf[pos..end]).to_string();
                    pos = end;
                }
                // Field 7: image_yes_no (string)
                (7, 2) => {
                    let (len, len_size) = decode_varint(&buf[pos..])?;
                    pos += len_size;
                    let end = pos + len as usize;
                    if end > buf.len() {
                        return Err(DecodeError::BufferTooShort);
                    }
                    chunk.image_yes_no = String::from_utf8_lossy(&buf[pos..end]).to_string();
                    pos = end;
                }
                // Skip unknown fields
                (_, 0) => {
                    // Varint
                    let (_, size) = decode_varint(&buf[pos..])?;
                    pos += size;
                }
                (_, 2) => {
                    // Length-delimited
                    let (len, len_size) = decode_varint(&buf[pos..])?;
                    pos += len_size + len as usize;
                }
                (_, 5) => {
                    // 32-bit
                    pos += 4;
                }
                (_, 1) => {
                    // 64-bit
                    pos += 8;
                }
                _ => {
                    return Err(DecodeError::UnknownWireType(wire_type as u8));
                }
            }
        }

        Ok(chunk)
    }

    /// Get the best available transcript
    pub fn best_transcript(&self) -> &str {
        if !self.transcript.is_empty() {
            &self.transcript
        } else {
            &self.asr
        }
    }

    /// Check if this result has meaningful content
    pub fn has_content(&self) -> bool {
        !self.transcript.is_empty() || !self.asr.is_empty()
    }
}

/// Protobuf decoding error
#[derive(Debug, Clone, thiserror::Error)]
pub enum DecodeError {
    #[error("Buffer too short")]
    BufferTooShort,
    #[error("Invalid varint")]
    InvalidVarint,
    #[error("Unknown wire type: {0}")]
    UnknownWireType(u8),
}

/// Encode a varint to the buffer
fn encode_varint(buf: &mut Vec<u8>, mut value: u64) {
    loop {
        let mut byte = (value & 0x7F) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        buf.push(byte);
        if value == 0 {
            break;
        }
    }
}

/// Decode a varint from the buffer, returning (value, bytes_consumed)
fn decode_varint(buf: &[u8]) -> Result<(u64, usize), DecodeError> {
    let mut value: u64 = 0;
    let mut shift = 0;

    for (i, &byte) in buf.iter().enumerate() {
        value |= ((byte & 0x7F) as u64) << shift;
        if byte & 0x80 == 0 {
            return Ok((value, i + 1));
        }
        shift += 7;
        if shift >= 64 {
            return Err(DecodeError::InvalidVarint);
        }
    }

    Err(DecodeError::BufferTooShort)
}

/// Streaming recognition response wrapper for compatibility
#[derive(Debug, Clone, Default)]
pub struct StreamingRecognitionResponse {
    /// List of transcript chunks
    pub results: Vec<TranscriptChunk>,
    /// Error information if any
    pub error: Option<StreamingError>,
}

/// Error from streaming recognition
#[derive(Debug, Clone)]
pub struct StreamingError {
    /// gRPC status code
    pub code: i32,
    /// Error message
    pub message: String,
}

impl StreamingRecognitionResponse {
    /// Create from a single transcript chunk
    pub fn from_chunk(chunk: TranscriptChunk) -> Self {
        Self {
            results: vec![chunk],
            error: None,
        }
    }

    /// Create an error response
    pub fn error(code: i32, message: String) -> Self {
        Self {
            results: vec![],
            error: Some(StreamingError { code, message }),
        }
    }

    /// Get the best transcript from results
    pub fn best_transcript(&self) -> Option<&str> {
        self.results.first().map(|r| r.best_transcript())
    }

    /// Check if any result is final
    pub fn has_final_result(&self) -> bool {
        self.results.iter().any(|r| r.is_final)
    }

    /// Check if there's an error
    pub fn has_error(&self) -> bool {
        self.error.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_speech_chunk_encode() {
        let chunk = SpeechChunk::new(
            Bytes::from_static(&[0x01, 0x02, 0x03]),
            "test-token",
            "hi-IN",
        );

        let encoded = chunk.encode();
        assert!(!encoded.is_empty());

        // Verify field tags are present
        assert!(encoded.contains(&0x0a)); // content field
        assert!(encoded.contains(&0x12)); // token field
        assert!(encoded.contains(&0x1a)); // lang field
    }

    #[test]
    fn test_speech_chunk_audio_only() {
        let chunk = SpeechChunk::audio_only(Bytes::from_static(&[0x01, 0x02]));
        let encoded = chunk.encode();

        // Should only contain content field
        assert!(encoded.contains(&0x0a));
        assert!(!encoded.contains(&0x12)); // no token
    }

    #[test]
    fn test_transcript_chunk_decode() {
        // Manually encode a simple message: transcript = "hello", is_final = true
        let mut buf = Vec::new();

        // Field 2: transcript = "hello"
        buf.push(0x12); // field 2, wire type 2
        buf.push(0x05); // length 5
        buf.extend_from_slice(b"hello");

        // Field 3: is_final = true
        buf.push(0x18); // field 3, wire type 0
        buf.push(0x01); // value 1 (true)

        let chunk = TranscriptChunk::decode(&buf).unwrap();
        assert_eq!(chunk.transcript, "hello");
        assert!(chunk.is_final);
    }

    #[test]
    fn test_transcript_chunk_with_confidence() {
        let mut buf = Vec::new();

        // Field 2: transcript = "test"
        buf.push(0x12);
        buf.push(0x04);
        buf.extend_from_slice(b"test");

        // Field 4: confidence = 0.95 (as float32)
        buf.push(0x25); // field 4, wire type 5 (32-bit)
        buf.extend_from_slice(&0.95f32.to_le_bytes());

        let chunk = TranscriptChunk::decode(&buf).unwrap();
        assert_eq!(chunk.transcript, "test");
        assert!((chunk.confidence - 0.95).abs() < 0.001);
    }

    #[test]
    fn test_transcript_chunk_best_transcript() {
        let chunk = TranscriptChunk {
            asr: "raw asr".to_string(),
            transcript: "processed transcript".to_string(),
            ..Default::default()
        };
        assert_eq!(chunk.best_transcript(), "processed transcript");

        let chunk2 = TranscriptChunk {
            asr: "only asr".to_string(),
            ..Default::default()
        };
        assert_eq!(chunk2.best_transcript(), "only asr");
    }

    #[test]
    fn test_varint_encoding() {
        let mut buf = Vec::new();

        // Test small value
        encode_varint(&mut buf, 1);
        assert_eq!(buf, vec![0x01]);

        buf.clear();
        encode_varint(&mut buf, 127);
        assert_eq!(buf, vec![0x7f]);

        buf.clear();
        encode_varint(&mut buf, 128);
        assert_eq!(buf, vec![0x80, 0x01]);

        buf.clear();
        encode_varint(&mut buf, 300);
        assert_eq!(buf, vec![0xac, 0x02]);
    }

    #[test]
    fn test_varint_decoding() {
        let (value, size) = decode_varint(&[0x01]).unwrap();
        assert_eq!(value, 1);
        assert_eq!(size, 1);

        let (value, size) = decode_varint(&[0x7f]).unwrap();
        assert_eq!(value, 127);
        assert_eq!(size, 1);

        let (value, size) = decode_varint(&[0x80, 0x01]).unwrap();
        assert_eq!(value, 128);
        assert_eq!(size, 2);

        let (value, size) = decode_varint(&[0xac, 0x02]).unwrap();
        assert_eq!(value, 300);
        assert_eq!(size, 2);
    }
}
