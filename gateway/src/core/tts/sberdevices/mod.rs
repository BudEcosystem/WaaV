//! SberDevices SaluteSpeech Text-to-Speech Provider
//!
//! This module provides integration with SberDevices SaluteSpeech's REST-based TTS service,
//! offering high-quality Russian and English speech synthesis with multiple voices.
//!
//! ## Features
//!
//! - Russian and English synthesis
//! - 7 voices (6 Russian, 1 English)
//! - SSML support
//! - OAuth 2.0 authentication with automatic token refresh
//! - Multiple audio formats (WAV, Opus, MP3)
//!
//! ## Authentication
//!
//! SberDevices uses OAuth 2.0 with client credentials:
//! - Client ID and Client Secret from Sber Studio
//! - Base64 encoded credentials for Basic auth
//! - Token validity: 30 minutes with auto-refresh
//!
//! ## Usage
//!
//! ```ignore
//! use waav_gateway::core::tts::{create_tts_provider, TTSConfig};
//!
//! let config = TTSConfig {
//!     provider: "sberdevices".to_string(),
//!     api_key: "client_id:client_secret".to_string(),
//!     voice_id: Some("Nec".to_string()),
//!     audio_format: Some("wav".to_string()),
//!     sample_rate: Some(24000),
//!     ..Default::default()
//! };
//!
//! let mut tts = create_tts_provider("sberdevices", config)?;
//! tts.connect().await?;
//! tts.speak("Привет, мир!", true).await?;
//! ```
//!
//! ## Available Voices
//!
//! | Voice ID | Name | Gender | Language |
//! |----------|------|--------|----------|
//! | Nec | Natalia | Female | ru-RU |
//! | Bys | Boris | Male | ru-RU |
//! | May | Martha | Female | ru-RU |
//! | Tur | Taras | Male | ru-RU |
//! | Ost | Alexandra | Female | ru-RU |
//! | Pon | Sergey | Male | ru-RU |
//! | Kin | Kira | Female | en-US |
//!
//! ## Audio Formats
//!
//! | Format | MIME Type |
//! |--------|-----------|
//! | wav | audio/wav |
//! | opus | audio/opus |
//! | mp3 | audio/mpeg |
//!
//! ## Sample Rates
//!
//! - 8000 Hz (telephony)
//! - 24000 Hz (high quality)
//!
//! ## REST Endpoint
//!
//! - Endpoint: `https://smartspeech.sber.ru/rest/v1/text:synthesize`
//! - Max text length: 4000 characters (including SSML)

mod config;
mod provider;

pub use config::{
    // Constants
    DEFAULT_SAMPLE_RATE,
    MAX_TEXT_LENGTH,
    OAUTH_ENDPOINT,
    SBER_TTS_SYNTHESIZE_URL,
    SberTtsAudioFormat,
    SberTtsConfig,
    SberTtsScope,
    SberTtsVoice,
    TOKEN_REFRESH_THRESHOLD_SECS,
    TOKEN_VALIDITY_SECS,
};
pub use provider::SberDevicesTts;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tts::base::{BaseTTS, TTSConfig};

    fn create_test_config() -> TTSConfig {
        TTSConfig {
            api_key: "test:secret".to_string(),
            voice_id: Some("Nec".to_string()),
            model: "default".to_string(),
            audio_format: Some("wav".to_string()),
            sample_rate: Some(24000),
            ..Default::default()
        }
    }

    #[test]
    fn test_sberdevices_tts_module_exports() {
        // Verify all public types are accessible
        let _: SberTtsVoice = SberTtsVoice::default();
        let _: SberTtsAudioFormat = SberTtsAudioFormat::default();
        let _: SberTtsScope = SberTtsScope::default();
    }

    #[test]
    fn test_sberdevices_voice_codes() {
        assert_eq!(SberTtsVoice::Nec.as_str(), "Nec");
        assert_eq!(SberTtsVoice::Bys.as_str(), "Bys");
        assert_eq!(SberTtsVoice::May.as_str(), "May");
        assert_eq!(SberTtsVoice::Tur.as_str(), "Tur");
        assert_eq!(SberTtsVoice::Ost.as_str(), "Ost");
        assert_eq!(SberTtsVoice::Pon.as_str(), "Pon");
        assert_eq!(SberTtsVoice::Kin.as_str(), "Kin");
    }

    #[test]
    fn test_sberdevices_audio_formats() {
        assert_eq!(SberTtsAudioFormat::Wav.as_str(), "wav");
        assert_eq!(SberTtsAudioFormat::Opus.as_str(), "opus");
        assert_eq!(SberTtsAudioFormat::Mp3.as_str(), "mp3");
    }

    #[test]
    fn test_sberdevices_tts_creation() {
        let config = create_test_config();
        let result = SberDevicesTts::new(config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_sberdevices_config_from_base() {
        let config = create_test_config();
        let sber_config = SberTtsConfig::from_base(config).unwrap();
        assert_eq!(sber_config.voice, SberTtsVoice::Nec);
        assert_eq!(sber_config.audio_format, SberTtsAudioFormat::Wav);
        assert_eq!(sber_config.sample_rate, 24000);
    }

    #[test]
    fn test_sberdevices_config_validation_missing_api_key() {
        let config = SberTtsConfig::default();
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("credentials"));
    }

    #[test]
    fn test_sberdevices_voice_all() {
        let voices = SberTtsVoice::all();
        assert_eq!(voices.len(), 7);
        assert!(voices.contains(&SberTtsVoice::Nec));
        assert!(voices.contains(&SberTtsVoice::Kin));
    }

    #[test]
    fn test_sberdevices_voice_display_names() {
        assert_eq!(SberTtsVoice::Nec.display_name(), "Natalia");
        assert_eq!(SberTtsVoice::Bys.display_name(), "Boris");
        assert_eq!(SberTtsVoice::Kin.display_name(), "Kira");
    }

    #[test]
    fn test_sberdevices_voice_languages() {
        // Russian voices
        assert_eq!(SberTtsVoice::Nec.language(), "ru-RU");
        assert_eq!(SberTtsVoice::Bys.language(), "ru-RU");
        assert_eq!(SberTtsVoice::May.language(), "ru-RU");
        assert_eq!(SberTtsVoice::Tur.language(), "ru-RU");
        assert_eq!(SberTtsVoice::Ost.language(), "ru-RU");
        assert_eq!(SberTtsVoice::Pon.language(), "ru-RU");

        // English voice
        assert_eq!(SberTtsVoice::Kin.language(), "en-US");
    }

    #[test]
    fn test_sberdevices_endpoint_url() {
        assert_eq!(
            SBER_TTS_SYNTHESIZE_URL,
            "https://smartspeech.sber.ru/rest/v1/text:synthesize"
        );
    }

    #[test]
    fn test_sberdevices_constants() {
        assert_eq!(DEFAULT_SAMPLE_RATE, 24000);
        assert_eq!(MAX_TEXT_LENGTH, 4000);
        assert_eq!(TOKEN_VALIDITY_SECS, 1800);
        assert_eq!(TOKEN_REFRESH_THRESHOLD_SECS, 60);
    }
}
