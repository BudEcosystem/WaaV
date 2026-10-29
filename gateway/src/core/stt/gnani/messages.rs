//! Gnani.ai realtime STT wire messages.
//!
//! Up: raw PCM s16le mono binary frames of exactly [`GNANI_STT_FRAME_BYTES`] bytes, and the
//! [`COMMIT_MESSAGE`] text frame that closes the open utterance. Down: JSON text frames with a
//! `type` discriminator (docs.gnani.ai, "Realtime streaming" → "Server messages").

use bytes::{Bytes, BytesMut};
use serde::Deserialize;

use crate::core::stt::base::STTResult;

/// Every binary audio frame is exactly this long: 512 samples of 16-bit PCM.
pub const GNANI_STT_FRAME_BYTES: usize = 1024;

/// Closes the open utterance now instead of waiting for silence.
pub const COMMIT_MESSAGE: &str = r#"{"type":"commit"}"#;

/// A server message.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GnaniServerMessage {
    /// The session is ready; echoes the active configuration.
    Connected {
        #[serde(default)]
        message: Option<String>,
        /// Prisma v3.0: the `x-api-request-id`, or one Gnani generated.
        #[serde(default)]
        request_id: Option<String>,
        #[serde(default)]
        config: Option<GnaniConnectedConfig>,
    },
    /// VAD closed a segment; its transcript follows.
    Processing {
        #[serde(default)]
        timestamp: Option<String>,
    },
    /// Voice was detected.
    SpeechStart {
        #[serde(default)]
        utterance_id: Option<String>,
    },
    /// The transcript of a closed segment (always final).
    Transcript(GnaniTranscript),
    /// The reply to an `update_settings` frame.
    SettingsUpdated {
        #[serde(default)]
        config: serde_json::Value,
    },
    /// A recoverable or fatal error; a fatal one is followed by a close.
    Error {
        #[serde(default)]
        message: String,
    },
    /// A message type this client does not know.
    #[serde(other)]
    Unknown,
}

impl GnaniServerMessage {
    /// Parse a text frame.
    pub fn parse(text: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(text)
    }
}

/// The `config` of a `connected` message.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct GnaniConnectedConfig {
    /// The active sample rate (echo of `x-sample-rate`).
    #[serde(default)]
    pub sample_rate: Option<u32>,
    /// Samples per frame (always 512).
    #[serde(default)]
    pub chunk_size: Option<u32>,
    #[serde(default)]
    pub language_code: Option<String>,
    #[serde(default)]
    pub format: Option<String>,
}

/// A `transcript` message.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct GnaniTranscript {
    #[serde(default)]
    pub text: String,
    /// The segment's speech, in ms (an integer on v2.5, a float on v3.0).
    #[serde(default)]
    pub audio_duration_ms: Option<f64>,
    /// `<request_id>-seg-<N>`.
    #[serde(default)]
    pub segment_id: Option<String>,
    /// 1-based position of the segment within the session.
    #[serde(default)]
    pub segment_index: Option<u64>,
    /// Time Gnani took to transcribe the segment, in ms.
    #[serde(default)]
    pub latency: Option<f64>,
    /// Prisma v3.0: the language heard (`te-IN`).
    #[serde(default)]
    pub detected_language_code: Option<String>,
    #[serde(default)]
    pub utterance_id: Option<String>,
    #[serde(default)]
    pub timestamp: Option<String>,
}

impl GnaniTranscript {
    /// The gateway result: each transcript is a VAD-closed segment, so it is final and ends the
    /// speech. Gnani sends no confidence (0.0 here stands for "not sent").
    pub fn to_stt_result(&self) -> STTResult {
        let mut result = STTResult::new(self.text.trim().to_string(), true, true, 0.0);
        result.detected_language = self
            .detected_language_code
            .as_deref()
            .map(|code| code.split('-').next().unwrap_or(code).to_ascii_lowercase());
        result.audio_duration = self.audio_duration_ms.map(|ms| ms / 1000.0);
        result.vendor_request_id = self.segment_id.clone();
        result
    }
}

/// Cuts the gateway's arbitrary audio chunks into exact [`GNANI_STT_FRAME_BYTES`] frames. A
/// remainder shorter than a frame waits for the next chunk; nothing is ever padded.
#[derive(Debug, Default)]
pub struct FrameChunker {
    pending: BytesMut,
}

impl FrameChunker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append `data` and take every whole frame now available.
    pub fn push(&mut self, data: &[u8]) -> Vec<Bytes> {
        self.pending.extend_from_slice(data);
        let mut frames = Vec::with_capacity(self.pending.len() / GNANI_STT_FRAME_BYTES);
        while self.pending.len() >= GNANI_STT_FRAME_BYTES {
            frames.push(self.pending.split_to(GNANI_STT_FRAME_BYTES).freeze());
        }
        frames
    }

    /// Bytes held back for the next frame.
    pub fn remainder(&self) -> usize {
        self.pending.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_is_cut_into_exact_frames_and_the_remainder_kept() {
        let mut chunker = FrameChunker::new();
        assert!(chunker.push(&[1u8; 1000]).is_empty());
        assert_eq!(chunker.remainder(), 1000);

        let frames = chunker.push(&[2u8; 100]);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].len(), GNANI_STT_FRAME_BYTES);
        assert_eq!(&frames[0][..1000], &[1u8; 1000][..]);
        assert_eq!(&frames[0][1000..], &[2u8; 24][..]);
        assert_eq!(chunker.remainder(), 76);

        let frames = chunker.push(&[3u8; 3000]);
        assert_eq!(frames.len(), 3);
        assert!(frames.iter().all(|f| f.len() == GNANI_STT_FRAME_BYTES));
        assert_eq!(chunker.remainder(), 76 + 3000 - 3 * GNANI_STT_FRAME_BYTES);

        let mut exact = FrameChunker::new();
        assert_eq!(exact.push(&[0u8; 2048]).len(), 2);
        assert_eq!(exact.remainder(), 0);
    }

    #[test]
    fn connected_is_parsed() {
        let msg = GnaniServerMessage::parse(
            r#"{"type":"connected","message":"STT service ready","timestamp":"2024-01-15T10:30:00.000Z",
                "config":{"sample_rate":16000,"chunk_size":512}}"#,
        )
        .unwrap();
        match msg {
            GnaniServerMessage::Connected { config, .. } => {
                let config = config.unwrap();
                assert_eq!(config.sample_rate, Some(16000));
                assert_eq!(config.chunk_size, Some(512));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn processing_speech_start_and_settings_updated_are_parsed() {
        assert!(matches!(
            GnaniServerMessage::parse(r#"{"type":"processing","timestamp":"t"}"#).unwrap(),
            GnaniServerMessage::Processing { .. }
        ));
        assert!(matches!(
            GnaniServerMessage::parse(r#"{"type":"speech_start","utterance_id":"u1"}"#).unwrap(),
            GnaniServerMessage::SpeechStart {
                utterance_id: Some(_)
            }
        ));
        assert!(matches!(
            GnaniServerMessage::parse(
                r#"{"type":"settings_updated","config":{"language_code":"en-IN"}}"#
            )
            .unwrap(),
            GnaniServerMessage::SettingsUpdated { .. }
        ));
        assert_eq!(
            GnaniServerMessage::parse(r#"{"type":"something_new","x":1}"#).unwrap(),
            GnaniServerMessage::Unknown
        );
    }

    #[test]
    fn transcript_is_parsed_into_a_final_result() {
        let msg = GnaniServerMessage::parse(
            r#"{"type":"transcript","timestamp":"2024-01-15T10:30:05.987Z","text":"Hello, how are you today?",
                "audio_duration_ms":2340,"segment_id":"req-seg-1","segment_index":1,"latency":320}"#,
        )
        .unwrap();
        let GnaniServerMessage::Transcript(t) = msg else {
            panic!("not a transcript");
        };
        assert_eq!(t.segment_index, Some(1));
        assert_eq!(t.latency, Some(320.0));
        let result = t.to_stt_result();
        assert_eq!(result.transcript, "Hello, how are you today?");
        assert!(result.is_final && result.is_speech_final);
        assert_eq!(result.audio_duration, Some(2.34));
        assert_eq!(result.vendor_request_id.as_deref(), Some("req-seg-1"));
        assert_eq!(result.detected_language, None);

        // Prisma v3.0: a float duration and the detected language.
        let msg = GnaniServerMessage::parse(
            r#"{"type":"transcript","text":"ఈ రోజు","detected_language_code":"te-IN",
                "audio_duration_ms":2340.0,"segment_id":"s","segment_index":1,"latency":380}"#,
        )
        .unwrap();
        let GnaniServerMessage::Transcript(t) = msg else {
            panic!("not a transcript");
        };
        assert_eq!(t.to_stt_result().detected_language.as_deref(), Some("te"));
    }

    #[test]
    fn error_is_parsed_with_its_message() {
        assert_eq!(
            GnaniServerMessage::parse(
                r#"{"type":"error","timestamp":"t","message":"STT engine failed to initialize"}"#
            )
            .unwrap(),
            GnaniServerMessage::Error {
                message: "STT engine failed to initialize".into()
            }
        );
    }

    #[test]
    fn the_commit_frame_is_gnanis() {
        let v: serde_json::Value = serde_json::from_str(COMMIT_MESSAGE).unwrap();
        assert_eq!(v, serde_json::json!({"type": "commit"}));
    }
}
