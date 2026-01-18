//! Tinkoff VoiceKit TTS Message Types
//!
//! Message types for Tinkoff's Text-to-Speech gRPC API.
//! These types match Tinkoff's proto definitions for the TextToSpeech service.

use bytes::Bytes;
use serde::{Deserialize, Serialize};

use super::config::TinkoffAudioEncoding;

/// Synthesis request for Tinkoff TTS
#[derive(Debug, Clone)]
pub struct SynthesizeSpeechRequest {
    /// Input text or SSML
    pub input: SynthesisInput,
    /// Voice selection
    pub voice: VoiceSelectionParams,
    /// Audio configuration
    pub audio_config: AudioConfig,
}

impl SynthesizeSpeechRequest {
    /// Create a new synthesis request
    pub fn new(
        text: impl Into<String>,
        voice_name: impl Into<String>,
        encoding: TinkoffAudioEncoding,
        sample_rate: u32,
    ) -> Self {
        Self {
            input: SynthesisInput::text(text),
            voice: VoiceSelectionParams::new(voice_name),
            audio_config: AudioConfig::new(encoding, sample_rate),
        }
    }

    /// Create with SSML input
    pub fn with_ssml(
        ssml: impl Into<String>,
        voice_name: impl Into<String>,
        encoding: TinkoffAudioEncoding,
        sample_rate: u32,
    ) -> Self {
        Self {
            input: SynthesisInput::ssml(ssml),
            voice: VoiceSelectionParams::new(voice_name),
            audio_config: AudioConfig::new(encoding, sample_rate),
        }
    }

    /// Encode to protobuf wire format
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(256);

        // Field 1: input (SynthesisInput)
        let input_bytes = self.input.encode();
        if !input_bytes.is_empty() {
            buf.push(0x0a); // field 1, wire type 2
            encode_varint(&mut buf, input_bytes.len() as u64);
            buf.extend_from_slice(&input_bytes);
        }

        // Field 2: voice (VoiceSelectionParams)
        let voice_bytes = self.voice.encode();
        if !voice_bytes.is_empty() {
            buf.push(0x12); // field 2, wire type 2
            encode_varint(&mut buf, voice_bytes.len() as u64);
            buf.extend_from_slice(&voice_bytes);
        }

        // Field 3: audio_config (AudioConfig)
        let config_bytes = self.audio_config.encode();
        if !config_bytes.is_empty() {
            buf.push(0x1a); // field 3, wire type 2
            encode_varint(&mut buf, config_bytes.len() as u64);
            buf.extend_from_slice(&config_bytes);
        }

        buf
    }
}

/// Input for synthesis (text or SSML)
#[derive(Debug, Clone)]
pub struct SynthesisInput {
    /// Plain text input
    pub text: Option<String>,
    /// SSML input
    pub ssml: Option<String>,
}

impl SynthesisInput {
    /// Create text input
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: Some(text.into()),
            ssml: None,
        }
    }

    /// Create SSML input
    pub fn ssml(ssml: impl Into<String>) -> Self {
        Self {
            text: None,
            ssml: Some(ssml.into()),
        }
    }

    /// Encode to protobuf wire format
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();

        // Field 1: text (oneof input_source)
        if let Some(ref text) = self.text {
            buf.push(0x0a); // field 1, wire type 2
            encode_varint(&mut buf, text.len() as u64);
            buf.extend_from_slice(text.as_bytes());
        }

        // Field 2: ssml (oneof input_source)
        if let Some(ref ssml) = self.ssml {
            buf.push(0x12); // field 2, wire type 2
            encode_varint(&mut buf, ssml.len() as u64);
            buf.extend_from_slice(ssml.as_bytes());
        }

        buf
    }
}

/// Voice selection parameters
#[derive(Debug, Clone)]
pub struct VoiceSelectionParams {
    /// Language code (e.g., "ru-RU")
    pub language_code: String,
    /// Voice name (e.g., "alyona", "dorofeev")
    pub name: String,
}

impl VoiceSelectionParams {
    /// Create new voice selection
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            language_code: "ru-RU".to_string(),
            name: name.into(),
        }
    }

    /// Encode to protobuf wire format
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();

        // Field 1: language_code (string)
        if !self.language_code.is_empty() {
            buf.push(0x0a); // field 1, wire type 2
            encode_varint(&mut buf, self.language_code.len() as u64);
            buf.extend_from_slice(self.language_code.as_bytes());
        }

        // Field 2: name (string)
        if !self.name.is_empty() {
            buf.push(0x12); // field 2, wire type 2
            encode_varint(&mut buf, self.name.len() as u64);
            buf.extend_from_slice(self.name.as_bytes());
        }

        buf
    }
}

/// Audio output configuration
#[derive(Debug, Clone)]
pub struct AudioConfig {
    /// Audio encoding format
    pub audio_encoding: TinkoffAudioEncoding,
    /// Speaking rate (0.25 to 4.0, default 1.0)
    pub speaking_rate: f32,
    /// Pitch adjustment (-20.0 to 20.0 semitones)
    pub pitch: f32,
    /// Volume gain in dB (-96.0 to 16.0)
    pub volume_gain_db: f32,
    /// Sample rate in Hz (1000-48000)
    pub sample_rate_hertz: u32,
}

impl AudioConfig {
    /// Create new audio config with defaults
    pub fn new(encoding: TinkoffAudioEncoding, sample_rate: u32) -> Self {
        Self {
            audio_encoding: encoding,
            speaking_rate: 1.0,
            pitch: 0.0,
            volume_gain_db: 0.0,
            sample_rate_hertz: sample_rate,
        }
    }

    /// Set speaking rate
    pub fn with_speaking_rate(mut self, rate: f32) -> Self {
        self.speaking_rate = rate;
        self
    }

    /// Set pitch
    pub fn with_pitch(mut self, pitch: f32) -> Self {
        self.pitch = pitch;
        self
    }

    /// Set volume gain
    pub fn with_volume_gain(mut self, gain: f32) -> Self {
        self.volume_gain_db = gain;
        self
    }

    /// Encode to protobuf wire format
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();

        // Field 1: audio_encoding (enum/int32)
        buf.push(0x08); // field 1, wire type 0
        encode_varint(&mut buf, self.audio_encoding.as_i32() as u64);

        // Field 2: speaking_rate (float)
        if (self.speaking_rate - 1.0).abs() > 0.001 {
            buf.push(0x15); // field 2, wire type 5 (32-bit)
            buf.extend_from_slice(&self.speaking_rate.to_le_bytes());
        }

        // Field 3: pitch (float)
        if self.pitch.abs() > 0.001 {
            buf.push(0x1d); // field 3, wire type 5 (32-bit)
            buf.extend_from_slice(&self.pitch.to_le_bytes());
        }

        // Field 4: volume_gain_db (float)
        if self.volume_gain_db.abs() > 0.001 {
            buf.push(0x25); // field 4, wire type 5 (32-bit)
            buf.extend_from_slice(&self.volume_gain_db.to_le_bytes());
        }

        // Field 5: sample_rate_hertz (uint32)
        buf.push(0x28); // field 5, wire type 0
        encode_varint(&mut buf, self.sample_rate_hertz as u64);

        buf
    }
}

/// Synthesis response from Tinkoff TTS
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SynthesizeSpeechResponse {
    /// Synthesized audio content
    pub audio_content: Bytes,
}

impl SynthesizeSpeechResponse {
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
                // Field 1: audio_content (bytes)
                (1, 2) => {
                    let (len, len_size) = decode_varint(&buf[pos..])?;
                    pos += len_size;
                    let end = pos + len as usize;
                    if end > buf.len() {
                        return Err(DecodeError::BufferTooShort);
                    }
                    response.audio_content = Bytes::copy_from_slice(&buf[pos..end]);
                    pos = end;
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
}

/// Streaming synthesis response (single chunk)
#[derive(Debug, Clone, Default)]
pub struct StreamingSynthesizeSpeechResponse {
    /// Audio chunk data
    pub audio_chunk: Bytes,
}

impl StreamingSynthesizeSpeechResponse {
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
                // Field 1: audio_chunk (bytes)
                (1, 2) => {
                    let (len, len_size) = decode_varint(&buf[pos..])?;
                    pos += len_size;
                    let end = pos + len as usize;
                    if end > buf.len() {
                        return Err(DecodeError::BufferTooShort);
                    }
                    response.audio_chunk = Bytes::copy_from_slice(&buf[pos..end]);
                    pos = end;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_synthesis_input_text_encode() {
        let input = SynthesisInput::text("Hello");
        let encoded = input.encode();
        assert!(!encoded.is_empty());
        assert!(encoded.contains(&0x0a)); // field 1
    }

    #[test]
    fn test_synthesis_input_ssml_encode() {
        let input = SynthesisInput::ssml("<speak>Hello</speak>");
        let encoded = input.encode();
        assert!(!encoded.is_empty());
        assert!(encoded.contains(&0x12)); // field 2
    }

    #[test]
    fn test_voice_selection_params_encode() {
        let voice = VoiceSelectionParams::new("alyona");
        let encoded = voice.encode();
        assert!(!encoded.is_empty());
        assert!(encoded.contains(&0x0a)); // language_code
        assert!(encoded.contains(&0x12)); // name
    }

    #[test]
    fn test_audio_config_encode() {
        let config = AudioConfig::new(TinkoffAudioEncoding::Linear16, 24000);
        let encoded = config.encode();
        assert!(!encoded.is_empty());
        assert!(encoded.contains(&0x08)); // audio_encoding
        assert!(encoded.contains(&0x28)); // sample_rate_hertz
    }

    #[test]
    fn test_audio_config_with_modifiers() {
        let config = AudioConfig::new(TinkoffAudioEncoding::Linear16, 24000)
            .with_speaking_rate(1.5)
            .with_pitch(2.0)
            .with_volume_gain(6.0);

        assert!((config.speaking_rate - 1.5).abs() < 0.001);
        assert!((config.pitch - 2.0).abs() < 0.001);
        assert!((config.volume_gain_db - 6.0).abs() < 0.001);
    }

    #[test]
    fn test_synthesize_request_encode() {
        let request = SynthesizeSpeechRequest::new(
            "Привет",
            "alyona",
            TinkoffAudioEncoding::Linear16,
            24000,
        );
        let encoded = request.encode();
        assert!(!encoded.is_empty());
        assert!(encoded.contains(&0x0a)); // input
        assert!(encoded.contains(&0x12)); // voice
        assert!(encoded.contains(&0x1a)); // audio_config
    }

    #[test]
    fn test_synthesize_response_decode() {
        // Manually encode: audio_content = [0x01, 0x02, 0x03]
        let mut buf = Vec::new();
        buf.push(0x0a); // field 1, wire type 2
        buf.push(0x03); // length 3
        buf.extend_from_slice(&[0x01, 0x02, 0x03]);

        let response = SynthesizeSpeechResponse::decode(&buf).unwrap();
        assert_eq!(response.audio_content.as_ref(), &[0x01, 0x02, 0x03]);
    }

    #[test]
    fn test_streaming_response_decode() {
        let mut buf = Vec::new();
        buf.push(0x0a); // field 1, wire type 2
        buf.push(0x02); // length 2
        buf.extend_from_slice(&[0xAB, 0xCD]);

        let response = StreamingSynthesizeSpeechResponse::decode(&buf).unwrap();
        assert_eq!(response.audio_chunk.as_ref(), &[0xAB, 0xCD]);
    }

    #[test]
    fn test_varint_encoding_roundtrip() {
        let values = [0u64, 1, 127, 128, 300, 16384, 1_000_000];

        for &value in &values {
            let mut buf = Vec::new();
            encode_varint(&mut buf, value);
            let (decoded, _) = decode_varint(&buf).unwrap();
            assert_eq!(decoded, value);
        }
    }
}
