//! Tinkoff VoiceKit Text-to-Speech Provider
//!
//! This module provides integration with Tinkoff VoiceKit's gRPC-based TTS service,
//! offering high-quality Russian speech synthesis with multiple voices.
//!
//! ## Features
//!
//! - Russian language synthesis
//! - The public FastPitch voices (anna, dima; the retired alyona and dorofeev map to them)
//! - SSML support
//! - Speaking rate and pitch control
//! - Streaming synthesis
//!
//! ## Authentication
//!
//! Tinkoff requires API key and secret key credentials:
//! - API Key (`TINKOFF_API_KEY`)
//! - Secret Key (`TINKOFF_SECRET_KEY`)
//!
//! ## Usage
//!
//! ```ignore
//! use waav_gateway::core::tts::{create_tts_provider, TTSConfig};
//!
//! let config = TTSConfig {
//!     provider: "tinkoff".to_string(),
//!     voice_id: Some("anna".to_string()),
//!     sample_rate: Some(24000),
//!     ..Default::default()
//! };
//!
//! let mut tts = create_tts_provider("tinkoff", config)?;
//! tts.connect().await?;
//! tts.speak("Привет, мир!", true).await?;
//! ```
//!
//! ## Available Voices
//!
//! | Voice ID | Gender | Language | Description |
//! |----------|--------|----------|-------------|
//! | anna | Female | ru-RU | Default Russian female voice |
//! | dima | Male | ru-RU | Russian male voice |
//!
//! `api.tinkoff.ai` presents a certificate chained to "Russian Trusted Root CA", which the common
//! OS trust stores do not carry; the gRPC channel uses the OS store, so a host that calls Tinkoff
//! needs that root installed.
//!
//! ## gRPC Endpoint
//!
//! - Endpoint: `api.tinkoff.ai:443`
//! - Service: `tinkoff.cloud.tts.v1.TextToSpeech`

mod config;
mod grpc;
mod messages;
mod provider;

pub use config::{TinkoffAudioEncoding, TinkoffTtsConfig, TinkoffVoice};
pub use grpc::{TinkoffTtsGrpcError, create_tinkoff_tts_channel, generate_jwt_token};
pub use messages::{
    AudioConfig, DecodeError, StreamingSynthesizeSpeechResponse, SynthesisInput,
    SynthesizeSpeechRequest, SynthesizeSpeechResponse, VoiceSelectionParams,
};
pub use provider::TinkoffTts;

/// Tinkoff VoiceKit gRPC endpoint
pub const TINKOFF_GRPC_ENDPOINT: &str = "https://api.tinkoff.ai:443";

/// gRPC service path for Synthesize
pub const GRPC_SYNTHESIZE_PATH: &str = "/tinkoff.cloud.tts.v1.TextToSpeech/Synthesize";

/// gRPC service path for StreamingSynthesize
pub const GRPC_STREAMING_SYNTHESIZE_PATH: &str =
    "/tinkoff.cloud.tts.v1.TextToSpeech/StreamingSynthesize";

/// TTS scope for JWT token
pub const TTS_SCOPE: &str = "tinkoff.cloud.tts";

/// Default JWT expiration in seconds
pub const JWT_EXPIRATION_SECS: u64 = 600;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tts::base::{BaseTTS, TTSConfig};

    fn create_test_config() -> TTSConfig {
        TTSConfig {
            api_key: String::new(),
            voice_id: Some("anna".to_string()),
            model: "default".to_string(),
            audio_format: Some("linear16".to_string()),
            sample_rate: Some(24000),
            ..Default::default()
        }
    }

    #[test]
    fn test_tinkoff_tts_module_exports() {
        // Verify all public types are accessible
        let _: TinkoffVoice = TinkoffVoice::default();
        let _: TinkoffAudioEncoding = TinkoffAudioEncoding::default();
    }

    #[test]
    fn test_tinkoff_voice_codes() {
        assert_eq!(TinkoffVoice::Anna.as_str(), "anna");
        assert_eq!(TinkoffVoice::Dima.as_str(), "dima");
        // Replaced on 19.03.2025: each parses to the new voice of its gender, and says so.
        assert_eq!(
            TinkoffVoice::parse_with_migration("alyona"),
            Ok((TinkoffVoice::Anna, Some("alyona")))
        );
        assert_eq!(
            TinkoffVoice::parse_with_migration("Dorofeev"),
            Ok((TinkoffVoice::Dima, Some("dorofeev")))
        );
        assert_eq!(
            TinkoffVoice::parse_with_migration("dima"),
            Ok((TinkoffVoice::Dima, None))
        );
    }

    #[test]
    fn test_tinkoff_audio_encodings() {
        assert_eq!(TinkoffAudioEncoding::Linear16.as_i32(), 1);
        assert_eq!(TinkoffAudioEncoding::RawOpus.as_i32(), 2);
        assert_eq!(TinkoffAudioEncoding::Alaw.as_i32(), 6);
    }

    #[test]
    fn test_tinkoff_tts_creation() {
        let config = create_test_config();
        let result = TinkoffTts::new(config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_tinkoff_config_from_base() {
        let config = create_test_config();
        let tinkoff_config = TinkoffTtsConfig::from_base(config).unwrap();
        assert_eq!(tinkoff_config.voice, TinkoffVoice::Anna);
        assert_eq!(tinkoff_config.encoding, TinkoffAudioEncoding::Linear16);
    }

    #[test]
    fn test_tinkoff_config_validation_missing_api_key() {
        let config = TinkoffTtsConfig::default();
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("API key"));
    }
}
