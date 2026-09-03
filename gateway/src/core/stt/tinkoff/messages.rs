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

/// Configuration of interim (partial) results.
///
/// Tinkoff VoiceKit `tinkoff.cloud.stt.v1.InterimResultsConfig` — interim results are
/// requested via this nested message (NOT a bare bool), and `interval` lets the caller ask for
/// a desired cadence in seconds.
///
/// ```protobuf
/// message InterimResultsConfig {
///     bool enable_interim_results = 1;
///     float interval = 2;
/// }
/// ```
#[derive(Debug, Clone, Default)]
pub struct InterimResultsConfig {
    /// Flag to enable sending interim results.
    pub enable_interim_results: bool,
    /// Desired interval (seconds) between interim results. `None` → service default cadence.
    pub interval: Option<f32>,
}

impl InterimResultsConfig {
    /// Encode to protobuf wire format.
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(8);

        // Field 1: enable_interim_results (bool)
        if self.enable_interim_results {
            buf.push(0x08); // field 1, wire type 0
            buf.push(0x01);
        }

        // Field 2: interval (float)
        if let Some(interval) = self.interval {
            if interval > 0.0 {
                buf.push(0x15); // field 2, wire type 5 (32-bit)
                buf.extend_from_slice(&interval.to_le_bytes());
            }
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
    /// Desired interval (seconds) between interim results, when interim results are enabled.
    /// Maps to `InterimResultsConfig.interval` on the wire.
    pub interim_results_interval: Option<f32>,
}

impl StreamingRecognitionConfig {
    /// Encode to protobuf wire format
    ///
    /// ```protobuf
    /// message StreamingRecognitionConfig {
    ///     RecognitionConfig config = 1;
    ///     bool single_utterance = 2;
    ///     InterimResultsConfig interim_results_config = 3;
    /// }
    /// ```
    ///
    /// NOTE: field 3 is the nested `InterimResultsConfig` message in Tinkoff's
    /// `tinkoff.cloud.stt.v1` proto — NOT a bare `bool interim_results`. Encoding a bare bool
    /// here would land on the wrong wire type and the service would drop interim requests, so we
    /// always emit the nested message (carrying `enable_interim_results` + optional `interval`).
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

        // Field 3: interim_results_config (message). Emitted whenever interim results are
        // enabled OR a cadence interval was requested.
        if self.interim_results || self.interim_results_interval.is_some() {
            let irc = InterimResultsConfig {
                enable_interim_results: self.interim_results,
                interval: self.interim_results_interval,
            };
            let irc_bytes = irc.encode();
            buf.push(0x1a); // field 3, wire type 2
            encode_varint(&mut buf, irc_bytes.len() as u64);
            buf.extend_from_slice(&irc_bytes);
        }

        buf
    }
}

/// A single phrase to boost (or suppress) during recognition.
///
/// Tinkoff VoiceKit `tinkoff.cloud.stt.v1.SpeechContextPhrase`.
///
/// ```protobuf
/// message SpeechContextPhrase {
///     string text = 1;
///     float score = 2;
/// }
/// ```
#[derive(Debug, Clone, Default)]
pub struct SpeechContextPhrase {
    /// Phrase text. Phrases shorter than 5 characters are discouraged by Tinkoff.
    pub text: String,
    /// Phrase score. Recommended range `[1.0, 10.0]`; `None` lets the service default to `1.0`.
    pub score: Option<f32>,
}

impl SpeechContextPhrase {
    /// Encode to protobuf wire format.
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(self.text.len() + 8);

        // Field 1: text (string)
        if !self.text.is_empty() {
            buf.push(0x0a); // field 1, wire type 2
            encode_varint(&mut buf, self.text.len() as u64);
            buf.extend_from_slice(self.text.as_bytes());
        }

        // Field 2: score (float)
        if let Some(score) = self.score {
            buf.push(0x15); // field 2, wire type 5 (32-bit)
            buf.extend_from_slice(&score.to_le_bytes());
        }

        buf
    }
}

/// A set of phrases to be recognised with higher (or lower) probability.
///
/// Tinkoff VoiceKit `tinkoff.cloud.stt.v1.SpeechContext`.
///
/// ```protobuf
/// message SpeechContext {
///     repeated SpeechContextPhrase phrases = 3;
///     string speech_context_dictionary_id = 4;
/// }
/// ```
#[derive(Debug, Clone, Default)]
pub struct SpeechContext {
    /// Phrases to recognise with higher (or lower) probability.
    pub phrases: Vec<SpeechContextPhrase>,
    /// Use a speech-context object stored in the cloud (dictionary id).
    pub speech_context_dictionary_id: Option<String>,
}

impl SpeechContext {
    /// Encode to protobuf wire format.
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(64);

        // Field 3: phrases (repeated message)
        for phrase in &self.phrases {
            let phrase_bytes = phrase.encode();
            buf.push(0x1a); // field 3, wire type 2
            encode_varint(&mut buf, phrase_bytes.len() as u64);
            buf.extend_from_slice(&phrase_bytes);
        }

        // Field 4: speech_context_dictionary_id (string)
        if let Some(ref id) = self.speech_context_dictionary_id {
            if !id.is_empty() {
                buf.push(0x22); // field 4, wire type 2
                encode_varint(&mut buf, id.len() as u64);
                buf.extend_from_slice(id.as_bytes());
            }
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
    /// Enable profanity filter for the first (most probable) final alternative.
    pub profanity_filter: bool,
    /// Phrase-boosting speech contexts (proto field 6, repeated).
    pub speech_contexts: Vec<SpeechContext>,
    /// Enable automatic punctuation
    pub enable_automatic_punctuation: bool,
    /// Number of audio channels
    pub num_channels: u32,
    /// Flag to disable phrase range detection — all speech recognised as a single phrase.
    /// Part of the proto `oneof vad`: mutually exclusive with `vad`.
    pub do_not_perform_vad: bool,
    /// VAD configuration (proto `oneof vad` → `vad_config`).
    pub vad: Option<VadConfig>,
    /// Enable automatic conversion of numerals from text to numeric form (denormalization).
    pub enable_denormalization: bool,
    /// Enable gender identification (male/female) on every final hypothesis.
    pub enable_gender_identification: bool,
}

impl RecognitionConfig {
    /// Encode to protobuf wire format
    ///
    /// Field numbers match Tinkoff VoiceKit `tinkoff.cloud.stt.v1.RecognitionConfig`:
    ///
    /// ```protobuf
    /// message RecognitionConfig {
    ///     AudioEncoding encoding = 1;
    ///     uint32 sample_rate_hertz = 2;
    ///     string language_code = 3;
    ///     uint32 max_alternatives = 4;
    ///     bool profanity_filter = 5;
    ///     repeated SpeechContext speech_contexts = 6;
    ///     bool enable_automatic_punctuation = 8;
    ///     uint32 num_channels = 12;
    ///     oneof vad {
    ///         bool do_not_perform_vad = 13;
    ///         VoiceActivityDetectionConfig vad_config = 14;
    ///     }
    ///     bool enable_denormalization = 16;
    ///     bool enable_gender_identification = 18;
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

        // Field 5: profanity_filter (bool)
        if self.profanity_filter {
            buf.push(0x28); // field 5, wire type 0
            buf.push(0x01);
        }

        // Field 6: speech_contexts (repeated message)
        for ctx in &self.speech_contexts {
            let ctx_bytes = ctx.encode();
            buf.push(0x32); // field 6, wire type 2
            encode_varint(&mut buf, ctx_bytes.len() as u64);
            buf.extend_from_slice(&ctx_bytes);
        }

        // Field 8: enable_automatic_punctuation (bool)
        if self.enable_automatic_punctuation {
            buf.push(0x40); // field 8, wire type 0
            buf.push(0x01);
        }

        // Field 12: num_channels (uint32)
        if self.num_channels > 1 {
            buf.push(0x60); // field 12, wire type 0
            encode_varint(&mut buf, self.num_channels as u64);
        }

        // oneof vad: field 13 (do_not_perform_vad) XOR field 14 (vad_config). `do_not_perform_vad`
        // wins when set, mirroring proto oneof semantics (only one member may be present).
        if self.do_not_perform_vad {
            // Field 13: do_not_perform_vad (bool)
            buf.push(0x68); // field 13, wire type 0
            buf.push(0x01);
        } else if let Some(ref vad) = self.vad {
            // Field 14: vad_config (message)
            let vad_bytes = vad.encode();
            buf.push(0x72); // field 14, wire type 2
            encode_varint(&mut buf, vad_bytes.len() as u64);
            buf.extend_from_slice(&vad_bytes);
        }

        // Field 16: enable_denormalization (bool). Tag = 16<<3 | 0 = 128 → varint 0x80 0x01.
        if self.enable_denormalization {
            buf.push(0x80);
            buf.push(0x01);
            buf.push(0x01);
        }

        // Field 18: enable_gender_identification (bool). Tag = 18<<3 | 0 = 144 → varint 0x90 0x01.
        if self.enable_gender_identification {
            buf.push(0x90);
            buf.push(0x01);
            buf.push(0x01);
        }

        buf
    }
}

impl VadConfig {
    /// Encode to protobuf wire format
    ///
    /// Field numbers match Tinkoff VoiceKit `VoiceActivityDetectionConfig`:
    ///
    /// ```protobuf
    /// message VoiceActivityDetectionConfig {
    ///     float min_speech_duration = 1;
    ///     float max_speech_duration = 2;
    ///     float silence_duration_threshold = 3;
    ///     float silence_prob_threshold = 4;
    ///     float aggressiveness = 5;
    ///     float silence_max = 6;
    ///     float silence_min = 7;
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

        // Field 6: silence_max (float)
        if self.silence_max > 0.0 {
            buf.push(0x35); // field 6, wire type 5 (32-bit)
            buf.extend_from_slice(&self.silence_max.to_le_bytes());
        }

        // Field 7: silence_min (float)
        if self.silence_min > 0.0 {
            buf.push(0x3d); // field 7, wire type 5 (32-bit)
            buf.extend_from_slice(&self.silence_min.to_le_bytes());
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
                    let bytes = take_len_delimited(buf, &mut pos)?;
                    let result = SpeechRecognitionResult::decode(bytes)?;
                    response.results.push(result);
                }
                // Field 2: endpoint_detection_type (enum/int32)
                (2, 0) => {
                    let (value, size) = decode_varint(&buf[pos..])?;
                    pos += size;
                    response.endpoint_detection_type = value as i32;
                }
                // Skip unknown fields
                (_, 0) => {
                    skip_varint(buf, &mut pos)?;
                }
                (_, 2) => {
                    let _ = take_len_delimited(buf, &mut pos)?;
                }
                (_, 5) => {
                    let _ = take_exact::<4>(buf, &mut pos)?;
                }
                (_, 1) => {
                    let _ = take_exact::<8>(buf, &mut pos)?;
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
        self.results
            .first()?
            .alternatives
            .first()
            .map(|a| a.transcript.as_str())
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
                    let bytes = take_len_delimited(buf, &mut pos)?;
                    let alt = SpeechRecognitionAlternative::decode(bytes)?;
                    result.alternatives.push(alt);
                }
                // Field 2: is_final (bool)
                (2, 0) => {
                    let (value, size) = decode_varint(&buf[pos..])?;
                    pos += size;
                    result.is_final = value != 0;
                }
                // Field 3: stability (float)
                (3, 5) => {
                    let bytes = take_exact::<4>(buf, &mut pos)?;
                    result.stability = f32::from_le_bytes(bytes);
                }
                // Skip unknown fields
                (_, 0) => {
                    skip_varint(buf, &mut pos)?;
                }
                (_, 2) => {
                    let _ = take_len_delimited(buf, &mut pos)?;
                }
                (_, 5) => {
                    let _ = take_exact::<4>(buf, &mut pos)?;
                }
                (_, 1) => {
                    let _ = take_exact::<8>(buf, &mut pos)?;
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
                    let bytes = take_len_delimited(buf, &mut pos)?;
                    alt.transcript = String::from_utf8_lossy(bytes).to_string();
                }
                // Field 2: confidence (float)
                (2, 5) => {
                    let bytes = take_exact::<4>(buf, &mut pos)?;
                    alt.confidence = f32::from_le_bytes(bytes);
                }
                // Skip unknown fields
                (_, 0) => {
                    skip_varint(buf, &mut pos)?;
                }
                (_, 2) => {
                    let _ = take_len_delimited(buf, &mut pos)?;
                }
                (_, 5) => {
                    let _ = take_exact::<4>(buf, &mut pos)?;
                }
                (_, 1) => {
                    let _ = take_exact::<8>(buf, &mut pos)?;
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

fn checked_end(pos: usize, len: usize, buf_len: usize) -> Result<usize, DecodeError> {
    let end = pos.checked_add(len).ok_or(DecodeError::BufferTooShort)?;
    if end > buf_len {
        return Err(DecodeError::BufferTooShort);
    }
    Ok(end)
}

fn take_exact<const N: usize>(buf: &[u8], pos: &mut usize) -> Result<[u8; N], DecodeError> {
    let end = checked_end(*pos, N, buf.len())?;
    let bytes = buf.get(*pos..end).ok_or(DecodeError::BufferTooShort)?;
    let out = bytes.try_into().map_err(|_| DecodeError::BufferTooShort)?;
    *pos = end;
    Ok(out)
}

fn take_len_delimited<'a>(buf: &'a [u8], pos: &mut usize) -> Result<&'a [u8], DecodeError> {
    let rest = buf.get(*pos..).ok_or(DecodeError::BufferTooShort)?;
    let (len, len_size) = decode_varint(rest)?;
    *pos = checked_end(*pos, len_size, buf.len())?;
    let len = usize::try_from(len).map_err(|_| DecodeError::BufferTooShort)?;
    let end = checked_end(*pos, len, buf.len())?;
    let bytes = buf.get(*pos..end).ok_or(DecodeError::BufferTooShort)?;
    *pos = end;
    Ok(bytes)
}

fn skip_varint(buf: &[u8], pos: &mut usize) -> Result<(), DecodeError> {
    let rest = buf.get(*pos..).ok_or(DecodeError::BufferTooShort)?;
    let (_, size) = decode_varint(rest)?;
    *pos = checked_end(*pos, size, buf.len())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal `RecognitionConfig` with all the newly-added fields at defaults, so tests
    /// only need to override what they exercise.
    fn test_recognition_config() -> RecognitionConfig {
        RecognitionConfig {
            encoding: TinkoffAudioEncoding::Linear16,
            sample_rate_hertz: 16000,
            language_code: "ru-RU".to_string(),
            max_alternatives: 1,
            profanity_filter: false,
            speech_contexts: Vec::new(),
            enable_automatic_punctuation: true,
            num_channels: 1,
            do_not_perform_vad: false,
            vad: None,
            enable_denormalization: false,
            enable_gender_identification: false,
        }
    }

    #[test]
    fn test_streaming_request_config_encode() {
        let config = StreamingRecognitionConfig {
            config: test_recognition_config(),
            interim_results: true,
            single_utterance: false,
            interim_results_interval: None,
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
        let config = test_recognition_config();

        let encoded = config.encode();

        // Should contain encoding field (0x08)
        assert!(encoded.contains(&0x08));
        // Should contain sample rate field (0x10)
        assert!(encoded.contains(&0x10));
        // Should contain language field (0x1a)
        assert!(encoded.contains(&0x1a));
    }

    // WIRE-LEVEL guard: each newly-wired feature must land at its EXACT protobuf field tag in the
    // encoded `RecognitionConfig` bytes — proving the param reaches the request body, not just the
    // config struct (the recurring "set on struct, never reaches wire" bug class).
    #[test]
    fn test_recognition_config_new_features_reach_wire() {
        let config = RecognitionConfig {
            profanity_filter: true,
            speech_contexts: vec![SpeechContext {
                phrases: vec![SpeechContextPhrase {
                    text: "Тинькофф".to_string(),
                    score: Some(7.0),
                }],
                speech_context_dictionary_id: Some("dict-42".to_string()),
            }],
            do_not_perform_vad: false,
            enable_denormalization: true,
            enable_gender_identification: true,
            vad: Some(VadConfig {
                silence_max: 1.5,
                silence_min: 0.3,
                ..Default::default()
            }),
            ..test_recognition_config()
        };
        let e = config.encode();

        // Helper: does `e` contain the contiguous tag-byte sequence?
        let contains = |needle: &[u8]| e.windows(needle.len()).any(|w| w == needle);

        // profanity_filter (field 5, bool true): tag 0x28, value 0x01.
        assert!(contains(&[0x28, 0x01]), "profanity_filter not on wire");
        // speech_contexts (field 6, message): tag 0x32.
        assert!(e.contains(&0x32), "speech_contexts not on wire");
        // SpeechContextPhrase.text bytes present (UTF-8 "Тинькофф").
        assert!(
            contains("Тинькофф".as_bytes()),
            "speech context phrase text not on wire"
        );
        // SpeechContext.speech_context_dictionary_id (field 4 string): tag 0x22 + value.
        assert!(contains(b"dict-42"), "dictionary id not on wire");
        // enable_denormalization (field 16, bool): tag varint 0x80 0x01, value 0x01.
        assert!(
            contains(&[0x80, 0x01, 0x01]),
            "enable_denormalization not on wire"
        );
        // enable_gender_identification (field 18, bool): tag varint 0x90 0x01, value 0x01.
        assert!(
            contains(&[0x90, 0x01, 0x01]),
            "enable_gender_identification not on wire"
        );
        // vad_config (field 14, message): tag 0x72; nested silence_max (field 6, float) tag 0x35,
        // silence_min (field 7, float) tag 0x3d.
        assert!(e.contains(&0x72), "vad_config not on wire");
        assert!(contains(&[0x35]), "vad silence_max tag not on wire");
        assert!(contains(&[0x3d]), "vad silence_min tag not on wire");
    }

    // WIRE-LEVEL guard: `do_not_perform_vad` (oneof member, field 13) must land on the wire and be
    // mutually exclusive with `vad_config` (field 14).
    #[test]
    fn test_do_not_perform_vad_oneof_on_wire() {
        // Empty language_code so a stray 0x72 byte can only originate from the vad_config tag
        // (the "ru-RU" string literal contains byte 0x72 = 'r', which would be a false positive).
        let config = RecognitionConfig {
            language_code: String::new(),
            do_not_perform_vad: true,
            // Provide a vad config too: the oneof must drop it in favor of do_not_perform_vad.
            vad: Some(VadConfig {
                silence_max: 1.0,
                ..Default::default()
            }),
            ..test_recognition_config()
        };
        let e = config.encode();
        // do_not_perform_vad (field 13, bool): tag 0x68, value 0x01.
        assert!(
            e.windows(2).any(|w| w == [0x68, 0x01]),
            "do_not_perform_vad not on wire"
        );
        // vad_config tag (0x72) must NOT appear — proto oneof: only one member may be set, and the
        // encoder gives precedence to do_not_perform_vad.
        assert!(
            !e.contains(&0x72),
            "vad_config must not coexist with do_not_perform_vad"
        );
    }

    // WIRE-LEVEL guard: interim results travel as the nested `InterimResultsConfig` at field 3 of
    // `StreamingRecognitionConfig`, carrying `interval` at the message's field 2.
    #[test]
    fn test_interim_results_config_interval_on_wire() {
        let config = StreamingRecognitionConfig {
            config: test_recognition_config(),
            interim_results: true,
            single_utterance: false,
            interim_results_interval: Some(0.25),
        };
        let e = config.encode();
        // interim_results_config (field 3, message): tag 0x1a.
        assert!(e.contains(&0x1a), "interim_results_config not on wire");
        // The float 0.25 little-endian bytes must appear (InterimResultsConfig.interval, field 2).
        let interval_le = 0.25f32.to_le_bytes();
        assert!(
            e.windows(4).any(|w| w == interval_le),
            "interim interval value not on wire"
        );
        // The InterimResultsConfig itself carries enable_interim_results (field 1) tag 0x08 then
        // interval tag 0x15.
        assert!(e.contains(&0x15), "interim interval tag not on wire");
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

    fn assert_buffer_too_short<T: std::fmt::Debug>(result: Result<T, DecodeError>) {
        match result {
            Err(DecodeError::BufferTooShort) => {}
            other => panic!("expected BufferTooShort, got {other:?}"),
        }
    }

    #[test]
    fn test_tinkoff_decode_truncated_known_fixed32_fields_are_typed_errors() {
        // SpeechRecognitionResult.stability: field 3, wire type fixed32.
        assert_buffer_too_short(SpeechRecognitionResult::decode(&[0x1d, 0x00, 0x00]));

        // SpeechRecognitionAlternative.confidence: field 2, wire type fixed32.
        assert_buffer_too_short(SpeechRecognitionAlternative::decode(&[0x15, 0x00]));
    }

    #[test]
    fn test_tinkoff_decode_unknown_fields_are_bounds_checked() {
        // Unknown fixed32 field with only two payload bytes.
        assert_buffer_too_short(StreamingRecognizeResponse::decode(&[0x4d, 0x01, 0x02]));

        // Unknown fixed64 field with only three payload bytes.
        assert_buffer_too_short(SpeechRecognitionResult::decode(&[0x49, 0x01, 0x02, 0x03]));

        // Unknown length-delimited field claims five bytes but carries two.
        assert_buffer_too_short(SpeechRecognitionAlternative::decode(&[
            0x4a, 0x05, 0x01, 0x02,
        ]));
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
            ..Default::default()
        };

        let encoded = vad.encode();
        // Should have all 4 float fields (each 5 bytes: 1 tag + 4 data)
        assert!(encoded.len() >= 20);
    }
}
