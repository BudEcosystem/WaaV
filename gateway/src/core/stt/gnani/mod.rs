//! Gnani.ai Speech-to-Text Provider
//!
//! Gnani's realtime speech-to-text (docs.gnani.ai, "Realtime streaming"): a WebSocket to
//! `wss://api.vachana.ai/stt/v3/stream` that takes raw PCM and returns one transcript per
//! speech segment. The former gRPC service (`asr.gnani.ai`, `/Listener/DoSpeechToText`, token +
//! access key + client certificate) is not in Gnani's docs, and a gRPC call to it gets an HTML
//! 403 from Cloudflare.
//!
//! ## Connection
//!
//! Every setting is a WebSocket upgrade header, so a reconnect restores the session by dialing
//! again:
//!
//! | Header | Value |
//! |--------|-------|
//! | `x-api-key-id` | the API key |
//! | `language_code` | one of the ten languages below, or `indic-auto` |
//! | `model` | `prisma-v2.5` (default) or `prisma-v3.0`, from the session's `model` |
//! | `x-sample-rate` | 8000, 16000 (default), 44100 or 48000 |
//! | `format` | `verbatim` (default) or `transcribe`; on Prisma v3.0 also `codemix` or `transliterate` (the `format` extra) |
//!
//! The VAD headers are left at Gnani's defaults (a segment closes after 500 ms of silence).
//!
//! ## Audio and results
//!
//! - Up: raw PCM s16le mono in binary frames of exactly 1,024 bytes. The gateway's chunks are cut
//!   into such frames; a remainder waits for the next chunk and is never padded.
//! - `end_turn` (and `disconnect`, when audio was sent since the last transcript) sends
//!   `{"type":"commit"}`, which closes the open utterance at once.
//! - Down: JSON frames typed `connected` (the session is ready; `connect` waits for it),
//!   `processing`, `speech_start`, `transcript`, `settings_updated` and `error`. Each `transcript`
//!   is a closed segment and reaches the result callback as a final, speech-final result (Gnani
//!   sends no interim results and no confidence).
//! - An `error` is reported on the error callback; a close with code 1008 (a refused handshake
//!   header) ends the session, any other close reconnects.
//!
//! ## Authentication
//!
//! One API key from Gnani's console: the session's `api_key` (the server's `GNANI_TOKEN`), else
//! `GNANI_TOKEN`. A `token|access_key` pair from the former API gives its token.
//! `GNANI_ACCESS_KEY` and the certificate are not used.
//!
//! ## Languages
//!
//! Hindi, English, Tamil, Telugu, Kannada, Malayalam, Marathi, Punjabi, Bengali and Gujarati
//! (`hi-IN`, `en-IN`, `ta-IN`, `te-IN`, `kn-IN`, `ml-IN`, `mr-IN`, `pa-IN`, `bn-IN`, `gu-IN`), or
//! `indic-auto`. The former codes still parse: `pa-guru-IN` is Punjabi, and `en-US`, `en-GB` and
//! `en-SG` are sent as `en-IN`. Urdu has no model and is refused at connect.

mod client;
mod config;
mod messages;

pub use client::GnaniSTT;
pub use config::{
    GNANI_STT_SAMPLE_RATES, GNANI_STT_URL, GnaniLanguage, GnaniSTTConfig, GnaniSTTFormat,
    GnaniSTTModel,
};
pub use messages::{
    COMMIT_MESSAGE, FrameChunker, GNANI_STT_FRAME_BYTES, GnaniConnectedConfig, GnaniServerMessage,
    GnaniTranscript,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::stt::base::{BaseSTT, STTConfig};

    #[test]
    fn test_gnani_language_codes() {
        let codes = GnaniLanguage::all_codes();
        assert_eq!(codes.len(), 10);
        for code in ["hi-IN", "en-IN", "kn-IN", "pa-IN"] {
            assert!(codes.contains(&code), "{code}");
        }
    }

    #[test]
    fn test_gnani_stt_creation_via_base_trait() {
        let config = STTConfig {
            provider: "gnani".to_string(),
            api_key: String::new(),
            language: "hi-IN".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "pcm16".to_string(),
            model: "default".to_string(),
        };
        assert!(GnaniSTT::new(config).is_ok());
    }
}
