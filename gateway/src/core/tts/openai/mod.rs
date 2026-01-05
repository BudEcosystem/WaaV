//! OpenAI TTS provider module.
//!
//! This module provides text-to-speech functionality using OpenAI's Audio Speech API.
//!
//! # Supported Models
//!
//! - `tts-1` - Standard quality, lower latency
//! - `tts-1-hd` - High definition quality, higher latency
//! - `gpt-4o-mini-tts` - Latest model with improved quality
//!
//! # Supported Voices
//!
//! alloy, ash, ballad, coral, echo, fable, onyx, nova, sage, shimmer, verse
//!
//! # Audio Formats
//!
//! mp3, opus, aac, flac, wav, pcm (24kHz 16-bit mono little-endian)
//!
//! # Example
//!
//! ```rust,ignore
//! use waav_gateway::core::tts::{BaseTTS, TTSConfig, OpenAITTS};
//! use std::sync::Arc;
//!
//! #[tokio::main]
//! async fn main() {
//!     let config = TTSConfig {
//!         api_key: "sk-...".to_string(),
//!         voice_id: Some("nova".to_string()),
//!         model: "tts-1-hd".to_string(),
//!         ..Default::default()
//!     };
//!
//!     let mut tts = OpenAITTS::new(config).unwrap();
//!     tts.connect().await.unwrap();
//!     tts.speak("Hello, world!", true).await.unwrap();
//! }
//! ```

mod config;
mod provider;

pub use config::{AudioOutputFormat, OpenAITTSModel, OpenAIVoice};
pub use provider::{OPENAI_TTS_URL, OpenAITTS};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tts::base::{BaseTTS, ConnectionState, TTSConfig};

    #[tokio::test]
    async fn test_openai_tts_default_creation() {
        let tts = OpenAITTS::default();
        assert!(!tts.is_ready());
        assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);
        assert_eq!(tts.model(), OpenAITTSModel::Tts1);
        assert_eq!(tts.voice(), OpenAIVoice::Alloy);
    }

    #[tokio::test]
    async fn test_openai_tts_with_config() {
        let config = TTSConfig {
            provider: "openai".to_string(),
            api_key: "test_key".to_string(),
            voice_id: Some("shimmer".to_string()),
            model: "tts-1-hd".to_string(),
            audio_format: Some("opus".to_string()),
            speaking_rate: Some(1.5),
            ..Default::default()
        };

        let tts = OpenAITTS::new(config).unwrap();
        assert_eq!(tts.model(), OpenAITTSModel::Tts1Hd);
        assert_eq!(tts.voice(), OpenAIVoice::Shimmer);
        assert_eq!(tts.output_format(), AudioOutputFormat::Opus);
    }

    #[tokio::test]
    async fn test_provider_info() {
        let tts = OpenAITTS::default();
        let info = tts.get_provider_info();

        assert_eq!(info["provider"], "openai");
        assert!(
            info["supported_models"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == "tts-1-hd")
        );
        assert!(
            info["supported_voices"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == "shimmer")
        );
        assert!(
            info["supported_formats"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == "pcm")
        );
    }

    #[test]
    fn test_model_parsing() {
        assert_eq!(
            OpenAITTSModel::from_str_or_default("tts-1"),
            OpenAITTSModel::Tts1
        );
        assert_eq!(
            OpenAITTSModel::from_str_or_default("tts-1-hd"),
            OpenAITTSModel::Tts1Hd
        );
        assert_eq!(
            OpenAITTSModel::from_str_or_default("gpt-4o-mini-tts"),
            OpenAITTSModel::Gpt4oMiniTts
        );
        // Unknown defaults to tts-1
        assert_eq!(
            OpenAITTSModel::from_str_or_default("unknown"),
            OpenAITTSModel::Tts1
        );
    }

    #[test]
    fn test_voice_parsing() {
        assert_eq!(
            OpenAIVoice::from_str_or_default("alloy"),
            OpenAIVoice::Alloy
        );
        assert_eq!(OpenAIVoice::from_str_or_default("nova"), OpenAIVoice::Nova);
        assert_eq!(
            OpenAIVoice::from_str_or_default("SHIMMER"),
            OpenAIVoice::Shimmer
        );
        // Unknown defaults to alloy
        assert_eq!(
            OpenAIVoice::from_str_or_default("unknown"),
            OpenAIVoice::Alloy
        );
    }

    #[test]
    fn test_audio_format_parsing() {
        assert_eq!(
            AudioOutputFormat::from_str_or_default("mp3"),
            AudioOutputFormat::Mp3
        );
        assert_eq!(
            AudioOutputFormat::from_str_or_default("pcm"),
            AudioOutputFormat::Pcm
        );
        assert_eq!(
            AudioOutputFormat::from_str_or_default("linear16"),
            AudioOutputFormat::Pcm
        );
        // Unknown defaults to mp3
        assert_eq!(
            AudioOutputFormat::from_str_or_default("unknown"),
            AudioOutputFormat::Mp3
        );
    }

    #[test]
    fn test_all_voices() {
        let voices = OpenAIVoice::all();
        assert_eq!(voices.len(), 11);
        assert!(voices.contains(&OpenAIVoice::Alloy));
        assert!(voices.contains(&OpenAIVoice::Nova));
        assert!(voices.contains(&OpenAIVoice::Verse));
    }

    #[test]
    fn test_audio_format_mime_types() {
        assert_eq!(AudioOutputFormat::Mp3.mime_type(), "audio/mpeg");
        assert_eq!(AudioOutputFormat::Pcm.mime_type(), "audio/pcm");
        assert_eq!(AudioOutputFormat::Opus.mime_type(), "audio/opus");
        assert_eq!(AudioOutputFormat::Wav.mime_type(), "audio/wav");
    }

    #[test]
    fn test_audio_format_sample_rate() {
        // All OpenAI TTS outputs are 24kHz
        assert_eq!(AudioOutputFormat::Mp3.sample_rate(), 24000);
        assert_eq!(AudioOutputFormat::Pcm.sample_rate(), 24000);
        assert_eq!(AudioOutputFormat::Opus.sample_rate(), 24000);
    }
}
