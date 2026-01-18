//! Tinkoff VoiceKit STT Message Types
//!
//! Message types for Tinkoff's Speech-to-Text gRPC streaming API.
//! These types match Tinkoff's proto definitions for the SpeechToText service.
//!
//! ## gRPC Service Definition
//!
//! ```protobuf
//! service SpeechToText {
//!     rpc StreamingRecognize(stream StreamingRecognizeRequest)
//!         returns (stream StreamingRecognizeResponse);
//! }
//! ```

use bytes::Bytes;
use serde::{Deserialize, Serialize};

use super::config::{TinkoffAudioEncoding, VadConfig};

/// Streaming recognition request sent to Tinkoff.
///
/// The first request must contain streaming_config, subsequent requests contain audio.
#[derive(Debug, Clone)]
pub struct StreamingRecognizeRequest {
    /// Streaming configuration (first request only)
    pub streaming_config: Option<StreamingRecognitionConfig>,
    /// Audio content (subsequent requests)
    pub audio_content: Option<Bytes>,
}

impl StreamingRecognizeRequest {
    /// Create a config request (first message)
    pub fn config(config: StreamingRecognitionConfig) -> Self {
        Self {
            streaming_config: Some(config),
            audio_content: None,
        }
    }

    /// Create an audio request (subsequent messages)
    pub fn audio(content: Bytes) -> Self {
        Self {
            streaming_config: None,
            audio_content: Some(content),
        }
    }

    /// Encode to protobuf wire format
    ///
    /// Message structure:
    /// ```protobuf
    /// message StreamingRecognizeRequest {
    ///     oneof streaming_request {
    ///         StreamingRecognitionConfig streaming_config = 1;
    ///         bytes audio_content = 2;
    ///     }
    /// }
    /// ```
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(1024);

        if let Some(ref config) = self.streaming_config {
            // Field 1: streaming_config (message)
            let config_bytes = config.encode();
            buf.push(0x0a); // field 1, wire type 2
            encode_varint(&mut buf, config_bytes.len() as u64);
            buf.extend_from_slice(&config_bytes);
        } else if let Some(ref audio) = self.audio_content {
            // Field 2: audio_content (bytes)
            buf.push(0x12); // field 2, wire type 2
            encode_varint(&mut buf, audio.len() as u64);
            buf.extend_from_slice(audio);
        }

        buf
    }
}

/// Configuration for streaming recognition
#[derive(Debug, Clone)]
pub struct StreamingRecognitionConfig {
    /// Recognition configuration
    pub config: RecognitionConfig,
    /// Return interim results
    pub interim_results: bool,
    /// Single utterance mode
    pub single_utterance: bool,
}

impl StreamingRecognitionConfig {
    /// Encode to protobuf wire format
    ///
    /// ```protobuf
    /// message StreamingRecognitionConfig {
    ///     RecognitionConfig config = 1;
    ///     bool single_utterance = 2;
    ///     bool interim_results = 3;
    /// }
    /// ```
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(256);

        // Field 1: config (message)
        let config_bytes = self.config.encode();
        buf.push(0x0a); // field 1, wire type 2
        encode_varint(&mut buf, config_bytes.len() as u64);
        buf.extend_from_slice(&config_bytes);

        // Field 2: single_utterance (bool)
        if self.single_utterance {
            buf.push(0x10); // field 2, wire type 0
            buf.push(0x01);
        }

        // Field 3: interim_results (bool)
        if self.interim_results {
            buf.push(0x18); // field 3, wire type 0
            buf.push(0x01);
        }

        buf
    }
}

/// Recognition configuration
#[derive(Debug, Clone)]
pub struct RecognitionConfig {
    /// Audio encoding
    pub encoding: TinkoffAudioEncoding,
    /// Sample rate in Hz
    pub sample_rate_hertz: u32,
    /// Language code (e.g., "ru-RU")
    pub language_code: String,
    /// Maximum alternatives to return
    pub max_alternatives: u32,
    /// Enable automatic punctuation
    pub enable_automatic_punctuation: bool,
    /// Number of audio channels
    pub num_channels: u32,
    /// VAD configuration
    pub vad: Option<VadConfig>,
}

impl RecognitionConfig {
    /// Encode to protobuf wire format
    ///
    /// ```protobuf
    /// message RecognitionConfig {
    ///     AudioEncoding encoding = 1;
    ///     uint32 sample_rate_hertz = 2;
    ///     string language_code = 3;
    ///     uint32 max_alternatives = 4;
    ///     bool enable_automatic_punctuation = 7;
    ///     int32 num_channels = 11;
    ///     VADConfig vad = 12;
    /// }
    /// ```
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(128);

        // Field 1: encoding (enum/int32)
        buf.push(0x08); // field 1, wire type 0
        encode_varint(&mut buf, self.encoding.as_i32() as u64);

        // Field 2: sample_rate_hertz (uint32)
        buf.push(0x10); // field 2, wire type 0
        encode_varint(&mut buf, self.sample_rate_hertz as u64);

        // Field 3: language_code (string)
        if !self.language_code.is_empty() {
            buf.push(0x1a); // field 3, wire type 2
            encode_varint(&mut buf, self.language_code.len() as u64);
            buf.extend_from_slice(self.language_code.as_bytes());
        }

        // Field 4: max_alternatives (uint32)
        if self.max_alternatives > 0 {
            buf.push(0x20); // field 4, wire type 0
            encode_varint(&mut buf, self.max_alternatives as u64);
        }

        // Field 7: enable_automatic_punctuation (bool)
        if self.enable_automatic_punctuation {
            buf.push(0x38); // field 7, wire type 0
            buf.push(0x01);
        }

        // Field 11: num_channels (int32)
        if self.num_channels > 1 {
            buf.push(0x58); // field 11, wire type 0
            encode_varint(&mut buf, self.num_channels as u64);
        }

        // Field 12: vad (message)
        if let Some(ref vad) = self.vad {
            let vad_bytes = vad.encode();
            buf.push(0x62); // field 12, wire type 2
            encode_varint(&mut buf, vad_bytes.len() as u64);
            buf.extend_from_slice(&vad_bytes);
        }

        buf
    }
}

impl VadConfig {
    /// Encode to protobuf wire format
    ///
    /// ```protobuf
    /// message VADConfig {
    ///     float min_speech_duration = 1;
    ///     float max_speech_duration = 2;
    ///     float silence_duration_threshold = 3;
    ///     float silence_prob_threshold = 4;
    /// }
    /// ```
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(32);

        // Field 1: min_speech_duration (float)
        if self.min_speech_duration > 0.0 {
            buf.push(0x0d); // field 1, wire type 5 (32-bit)
            buf.extend_from_slice(&self.min_speech_duration.to_le_bytes());
        }

        // Field 2: max_speech_duration (float)
        if self.max_speech_duration > 0.0 {
            buf.push(0x15); // field 2, wire type 5 (32-bit)
            buf.extend_from_slice(&self.max_speech_duration.to_le_bytes());
        }

        // Field 3: silence_duration_threshold (float)
        if self.silence_duration_threshold > 0.0 {
            buf.push(0x1d); // field 3, wire type 5 (32-bit)
            buf.extend_from_slice(&self.silence_duration_threshold.to_le_bytes());
        }

        // Field 4: silence_prob_threshold (float)
        if self.silence_prob_threshold > 0.0 {
            buf.push(0x25); // field 4, wire type 5 (32-bit)
            buf.extend_from_slice(&self.silence_prob_threshold.to_le_bytes());
        }

        buf
    }
}

/// Streaming recognition response from Tinkoff
///
/// ```protobuf
/// message StreamingRecognizeResponse {
///     repeated StreamingRecognitionResult results = 1;
///     EndpointDetectionType endpoint_detection_type = 2;
/// }
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StreamingRecognizeResponse {
    /// Recognition results
    pub results: Vec<SpeechRecognitionResult>,
    /// Endpoint detection type (if any)
    pub endpoint_detection_type: i32,
}

impl StreamingRecognizeResponse {
    /// Decode from protobuf wire format
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut response = Self::default();
        let mut pos = 0;

        while pos < buf.len() {
            let (field_tag, new_pos) = decode_varint(&buf[pos..])?;
            pos += new_pos;

            let field_number = field_tag >> 3;
            let wire_type = field_tag & 0x07;

            match (field_number, wire_type) {
                // Field 1: results (repeated message)
                (1, 2) => {
                    let (len, len_size) = decode_varint(&buf[pos..])?;
                    pos += len_size;
                    let end = pos + len as usize;
                    if end > buf.len() {
                        return Err(DecodeError::BufferTooShort);
                    }
                    let result = SpeechRecognitionResult::decode(&buf[pos..end])?;
                    response.results.push(result);
                    pos = end;
                }
                // Field 2: endpoint_detection_type (enum/int32)
                (2, 0) => {
                    let (value, size) = decode_varint(&buf[pos..])?;
                    pos += size;
                    response.endpoint_detection_type = value as i32;
                }
                // Skip unknown fields
                (_, 0) => {
                    let (_, size) = decode_varint(&buf[pos..])?;
                    pos += size;
                }
                (_, 2) => {
                    let (len, len_size) = decode_varint(&buf[pos..])?;
                    pos += len_size + len as usize;
                }
                (_, 5) => {
                    pos += 4;
                }
                (_, 1) => {
                    pos += 8;
                }
                _ => {
                    return Err(DecodeError::UnknownWireType(wire_type as u8));
                }
            }
        }

        Ok(response)
    }

    /// Get the best transcript from the first result
    pub fn best_transcript(&self) -> Option<&str> {
        self.results.first()?.alternatives.first().map(|a| a.transcript.as_str())
    }

    /// Check if any result is final
    pub fn has_final_result(&self) -> bool {
        self.results.iter().any(|r| r.is_final)
    }

    /// Get the first final result's transcript
    pub fn final_transcript(&self) -> Option<&str> {
        self.results
            .iter()
            .find(|r| r.is_final)?
            .alternatives
            .first()
            .map(|a| a.transcript.as_str())
    }
}

/// Speech recognition result
///
/// ```protobuf
/// message StreamingRecognitionResult {
///     repeated SpeechRecognitionAlternative alternatives = 1;
///     bool is_final = 2;
///     float stability = 3;
/// }
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SpeechRecognitionResult {
    /// Alternative transcriptions
    pub alternatives: Vec<SpeechRecognitionAlternative>,
    /// Whether this is a final result
    pub is_final: bool,
    /// Stability of interim result (0.0 to 1.0)
    pub stability: f32,
}

impl SpeechRecognitionResult {
    /// Decode from protobuf wire format
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut result = Self::default();
        let mut pos = 0;

        while pos < buf.len() {
            let (field_tag, new_pos) = decode_varint(&buf[pos..])?;
            pos += new_pos;

            let field_number = field_tag >> 3;
            let wire_type = field_tag & 0x07;

            match (field_number, wire_type) {
                // Field 1: alternatives (repeated message)
                (1, 2) => {
                    let (len, len_size) = decode_varint(&buf[pos..])?;
                    pos += len_size;
                    let end = pos + len as usize;
                    if end > buf.len() {
                        return Err(DecodeError::BufferTooShort);
                    }
                    let alt = SpeechRecognitionAlternative::decode(&buf[pos..end])?;
                    result.alternatives.push(alt);
                    pos = end;
                }
                // Field 2: is_final (bool)
                (2, 0) => {
                    let (value, size) = decode_varint(&buf[pos..])?;
                    pos += size;
                    result.is_final = value != 0;
                }
                // Field 3: stability (float)
                (3, 5) => {
                    if pos + 4 > buf.len() {
                        return Err(DecodeError::BufferTooShort);
                    }
                    let bytes: [u8; 4] = buf[pos..pos + 4].try_into().unwrap();
                    result.stability = f32::from_le_bytes(bytes);
                    pos += 4;
                }
                // Skip unknown fields
                (_, 0) => {
                    let (_, size) = decode_varint(&buf[pos..])?;
                    pos += size;
                }
                (_, 2) => {
                    let (len, len_size) = decode_varint(&buf[pos..])?;
                    pos += len_size + len as usize;
                }
                (_, 5) => {
                    pos += 4;
                }
                (_, 1) => {
                    pos += 8;
                }
                _ => {
                    return Err(DecodeError::UnknownWireType(wire_type as u8));
                }
            }
        }

        Ok(result)
    }
}

/// Speech recognition alternative
///
/// ```protobuf
/// message SpeechRecognitionAlternative {
///     string transcript = 1;
///     float confidence = 2;
/// }
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SpeechRecognitionAlternative {
    /// Transcribed text
    pub transcript: String,
    /// Confidence score (0.0 to 1.0)
    pub confidence: f32,
}

impl SpeechRecognitionAlternative {
    /// Decode from protobuf wire format
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut alt = Self::default();
        let mut pos = 0;

        while pos < buf.len() {
            let (field_tag, new_pos) = decode_varint(&buf[pos..])?;
            pos += new_pos;

            let field_number = field_tag >> 3;
            let wire_type = field_tag & 0x07;

            match (field_number, wire_type) {
                // Field 1: transcript (string)
                (1, 2) => {
                    let (len, len_size) = decode_varint(&buf[pos..])?;
                    pos += len_size;
                    let end = pos + len as usize;
                    if end > buf.len() {
                        return Err(DecodeError::BufferTooShort);
                    }
                    alt.transcript = String::from_utf8_lossy(&buf[pos..end]).to_string();
                    pos = end;
                }
                // Field 2: confidence (float)
                (2, 5) => {
                    if pos + 4 > buf.len() {
                        return Err(DecodeError::BufferTooShort);
                    }
                    let bytes: [u8; 4] = buf[pos..pos + 4].try_into().unwrap();
                    alt.confidence = f32::from_le_bytes(bytes);
                    pos += 4;
                }
                // Skip unknown fields
                (_, 0) => {
                    let (_, size) = decode_varint(&buf[pos..])?;
                    pos += size;
                }
                (_, 2) => {
                    let (len, len_size) = decode_varint(&buf[pos..])?;
                    pos += len_size + len as usize;
                }
                (_, 5) => {
                    pos += 4;
                }
                (_, 1) => {
                    pos += 8;
                }
                _ => {
                    return Err(DecodeError::UnknownWireType(wire_type as u8));
                }
            }
        }

        Ok(alt)
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
pub fn encode_varint(buf: &mut Vec<u8>, mut value: u64) {
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
pub fn decode_varint(buf: &[u8]) -> Result<(u64, usize), DecodeError> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_streaming_request_config_encode() {
        let config = StreamingRecognitionConfig {
            config: RecognitionConfig {
                encoding: TinkoffAudioEncoding::Linear16,
                sample_rate_hertz: 16000,
                language_code: "ru-RU".to_string(),
                max_alternatives: 1,
                enable_automatic_punctuation: true,
                num_channels: 1,
                vad: None,
            },
            interim_results: true,
            single_utterance: false,
        };

        let request = StreamingRecognizeRequest::config(config);
        let encoded = request.encode();

        // Should have field 1 tag (0x0a)
        assert!(encoded.contains(&0x0a));
        assert!(!encoded.is_empty());
    }

    #[test]
    fn test_streaming_request_audio_encode() {
        let audio = Bytes::from_static(&[0x01, 0x02, 0x03, 0x04]);
        let request = StreamingRecognizeRequest::audio(audio);
        let encoded = request.encode();

        // Should have field 2 tag (0x12)
        assert!(encoded.contains(&0x12));
        assert!(encoded.len() > 4);
    }

    #[test]
    fn test_recognition_config_encode() {
        let config = RecognitionConfig {
            encoding: TinkoffAudioEncoding::Linear16,
            sample_rate_hertz: 16000,
            language_code: "ru-RU".to_string(),
            max_alternatives: 1,
            enable_automatic_punctuation: true,
            num_channels: 1,
            vad: None,
        };

        let encoded = config.encode();

        // Should contain encoding field (0x08)
        assert!(encoded.contains(&0x08));
        // Should contain sample rate field (0x10)
        assert!(encoded.contains(&0x10));
        // Should contain language field (0x1a)
        assert!(encoded.contains(&0x1a));
    }

    #[test]
    fn test_speech_recognition_alternative_decode() {
        // Encode: transcript = "test", confidence = 0.95
        let mut buf = Vec::new();

        // Field 1: transcript = "test"
        buf.push(0x0a); // field 1, wire type 2
        buf.push(0x04); // length 4
        buf.extend_from_slice(b"test");

        // Field 2: confidence = 0.95
        buf.push(0x15); // field 2, wire type 5
        buf.extend_from_slice(&0.95f32.to_le_bytes());

        let alt = SpeechRecognitionAlternative::decode(&buf).unwrap();
        assert_eq!(alt.transcript, "test");
        assert!((alt.confidence - 0.95).abs() < 0.001);
    }

    #[test]
    fn test_speech_recognition_result_decode() {
        // Encode a result with one alternative and is_final = true
        let mut buf = Vec::new();

        // First encode an alternative
        let mut alt_buf = Vec::new();
        alt_buf.push(0x0a); // transcript field
        alt_buf.push(0x05); // length 5
        alt_buf.extend_from_slice(b"hello");

        // Field 1: alternatives (embedded message)
        buf.push(0x0a); // field 1, wire type 2
        encode_varint(&mut buf, alt_buf.len() as u64);
        buf.extend_from_slice(&alt_buf);

        // Field 2: is_final = true
        buf.push(0x10); // field 2, wire type 0
        buf.push(0x01);

        let result = SpeechRecognitionResult::decode(&buf).unwrap();
        assert_eq!(result.alternatives.len(), 1);
        assert_eq!(result.alternatives[0].transcript, "hello");
        assert!(result.is_final);
    }

    #[test]
    fn test_streaming_response_decode() {
        // Build a response with one result
        let mut buf = Vec::new();

        // Build inner result
        let mut result_buf = Vec::new();

        // Alternative with transcript "world"
        let mut alt_buf = Vec::new();
        alt_buf.push(0x0a);
        alt_buf.push(0x05);
        alt_buf.extend_from_slice(b"world");

        result_buf.push(0x0a); // alternatives field
        encode_varint(&mut result_buf, alt_buf.len() as u64);
        result_buf.extend_from_slice(&alt_buf);

        result_buf.push(0x10); // is_final
        result_buf.push(0x01);

        // Add result to response
        buf.push(0x0a); // results field
        encode_varint(&mut buf, result_buf.len() as u64);
        buf.extend_from_slice(&result_buf);

        let response = StreamingRecognizeResponse::decode(&buf).unwrap();
        assert_eq!(response.results.len(), 1);
        assert_eq!(response.best_transcript(), Some("world"));
        assert!(response.has_final_result());
    }

    #[test]
    fn test_varint_encoding_roundtrip() {
        let test_values = [0u64, 1, 127, 128, 300, 16383, 16384, 10000000];

        for value in test_values {
            let mut buf = Vec::new();
            encode_varint(&mut buf, value);
            let (decoded, _) = decode_varint(&buf).unwrap();
            assert_eq!(decoded, value, "Roundtrip failed for value {}", value);
        }
    }

    #[test]
    fn test_vad_config_encode() {
        let vad = VadConfig {
            min_speech_duration: 0.5,
            max_speech_duration: 30.0,
            silence_duration_threshold: 1.0,
            silence_prob_threshold: 0.8,
        };

        let encoded = vad.encode();
        // Should have all 4 float fields (each 5 bytes: 1 tag + 4 data)
        assert!(encoded.len() >= 20);
    }
}
