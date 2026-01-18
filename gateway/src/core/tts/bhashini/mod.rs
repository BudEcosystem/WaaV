//! Bhashini (ULCA) Text-to-Speech Provider
//!
//! This module provides integration with Bhashini's ULCA (Universal Language
//! Contribution APIs) for speech synthesis supporting 22+ Indian languages.
//!
//! ## Features
//!
//! - Support for all 22 scheduled Indian languages plus English
//! - Government of India (MeitY) backed infrastructure
//! - Multiple TTS models from AI4Bharat, IITM, IISC, CDAC
//! - Pipeline-based architecture for combined ASR+Translation+TTS
//!
//! ## Authentication
//!
//! Bhashini uses a 2-step authentication:
//! 1. Pipeline Config call: `userID` + `ulcaApiKey` headers
//! 2. Pipeline Compute call: `inferenceApiKey` from config response
//!
//! Credentials format: `userId|ulcaApiKey` or `userId|ulcaApiKey|inferenceApiKey`
//!
//! ## Usage
//!
//! ```ignore
//! use waav_gateway::core::tts::{BaseTTS, TTSConfig};
//! use waav_gateway::core::tts::bhashini::BhashiniTts;
//!
//! let config = TTSConfig {
//!     api_key: "userId|ulcaApiKey".to_string(),
//!     voice_id: Some("hi-female".to_string()), // Hindi, female voice
//!     sample_rate: Some(22050),
//!     ..Default::default()
//! };
//!
//! let mut tts = BhashiniTts::new(config)?;
//! tts.connect().await?;
//! tts.speak("नमस्ते, आप कैसे हैं?", true).await?;
//! tts.disconnect().await?;
//! ```
//!
//! ## Available Languages
//!
//! All languages supported by Bhashini STT are also supported for TTS.
//! See `BhashiniLanguage` enum for the full list.
//!
//! ## Voice Selection
//!
//! Voice ID format: `{language_code}` or `{language_code}-{gender}`
//!
//! Examples:
//! - `hi` - Hindi, default gender (female)
//! - `hi-male` - Hindi, male voice
//! - `ta-female` - Tamil, female voice

mod config;
mod provider;

pub use config::{
    BhashiniTtsAudioFormat, BhashiniTtsConfig, BhashiniTtsGender, DEFAULT_TTS_SAMPLE_RATE,
};
pub use provider::BhashiniTts;

// Re-export language types from STT module
pub use crate::core::stt::bhashini::{
    BhashiniLanguage, BhashiniPipelineProvider, LanguageFamily,
    BHASHINI_COMPUTE_URL, BHASHINI_CONFIG_URL, MEITY_PIPELINE_ID, AI4BHARAT_PIPELINE_ID,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tts::base::{BaseTTS, TTSConfig};

    fn create_test_config() -> TTSConfig {
        TTSConfig {
            api_key: "test_user|test_ulca_key".to_string(),
            voice_id: Some("hi".to_string()),
            sample_rate: Some(22050),
            ..Default::default()
        }
    }

    #[test]
    fn test_bhashini_tts_module_exports() {
        // Verify all public types are accessible
        let _: BhashiniTtsAudioFormat = BhashiniTtsAudioFormat::default();
        let _: BhashiniTtsGender = BhashiniTtsGender::default();
    }

    #[test]
    fn test_bhashini_tts_audio_formats() {
        assert_eq!(BhashiniTtsAudioFormat::Wav.as_str(), "wav");
        assert_eq!(BhashiniTtsAudioFormat::Mp3.as_str(), "mp3");
    }

    #[test]
    fn test_bhashini_tts_gender() {
        assert_eq!(BhashiniTtsGender::Male.as_str(), "male");
        assert_eq!(BhashiniTtsGender::Female.as_str(), "female");
    }

    #[test]
    fn test_bhashini_tts_creation() {
        let config = create_test_config();
        let result = BhashiniTts::new(config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_bhashini_tts_config_from_base() {
        let config = create_test_config();
        let bhashini_config = BhashiniTtsConfig::from_base(config).unwrap();
        assert_eq!(bhashini_config.language, BhashiniLanguage::Hindi);
        assert_eq!(bhashini_config.audio_format, BhashiniTtsAudioFormat::default());
        assert_eq!(bhashini_config.sample_rate, 22050);
    }

    #[test]
    fn test_bhashini_tts_config_validation() {
        let config = BhashiniTtsConfig::default();
        let result = config.validate();
        assert!(result.is_err());
    }

    #[test]
    fn test_bhashini_tts_constants() {
        assert_eq!(DEFAULT_TTS_SAMPLE_RATE, 22050);
    }
}
