//! Gnani.ai Text-to-Speech Provider
//!
//! This module provides integration with Gnani.ai's TTS service,
//! supporting 12 Indian language variants with multi-speaker voices
//! and SSML gender selection.
//!
//! ## Features
//!
//! - 12 language support (Hindi, Kannada, Tamil, Telugu, etc.)
//! - Multi-speaker voices (10+ voices per language)
//! - Single-speaker voices with SSML gender selection
//! - PCM16 audio output at 8000 Hz
//!
//! ## Authentication
//!
//! Gnani requires two credentials (provided via email after registration):
//! - Token (`GNANI_TOKEN`)
//! - Access Key (`GNANI_ACCESS_KEY`)
//!
//! SSL certificate is optional for TTS (unlike STT which requires it).
//!
//! ## Usage
//!
//! ```ignore
//! use waav_gateway::core::tts::{create_tts_provider, TTSConfig};
//!
//! let config = TTSConfig {
//!     provider: "gnani".to_string(),
//!     voice_id: Some("Hi-IN".to_string()),
//!     ..Default::default()
//! };
//!
//! let mut tts = create_tts_provider("gnani", config)?;
//! tts.connect().await?;
//! tts.speak("नमस्ते दुनिया", false).await?;
//! ```
//!
//! ## Supported Languages
//!
//! ### Single-Speaker Mode (ssmlGender: MALE/FEMALE)
//!
//! | Language | Code | Gender Support |
//! |----------|------|----------------|
//! | English (India) | En-IN | Male, Female |
//! | Hindi | Hi-IN | Male, Female |
//! | Hindi (Alternate) | Hi-IN-al | Male, Female |
//! | Kannada | Kn-IN | Male, Female |
//! | Tamil | Ta-IN | Male, Female |
//! | Telugu | Te-IN | Male, Female |
//! | Marathi | Mr-IN | Female only |
//!
//! ### Multi-Speaker Mode (voice name selection)
//!
//! All languages support multi-speaker mode with 10+ voices each:
//! - En-IN, Hi-IN, Hi-IN-al, Kn-IN, Ta-IN, Te-IN, Mr-IN
//! - Ml-IN (Malayalam), Gu-IN (Gujarati), Bn-IN (Bengali)
//! - Pa-IN (Punjabi), Ne-NP (Nepali)
//!
//! ## Rate Limits
//!
//! - Contact hello@gnani.ai for commercial access

mod config;
mod provider;

pub use config::{GnaniGender, GnaniTTSConfig, GnaniTTSLanguage};
pub use provider::GnaniTTS;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tts::base::TTSConfig;

    fn create_test_config() -> TTSConfig {
        TTSConfig {
            provider: "gnani".to_string(),
            api_key: String::new(),
            voice_id: Some("Hi-IN".to_string()),
            model: "default".to_string(),
            speaking_rate: Some(1.0),
            audio_format: Some("pcm16".to_string()),
            sample_rate: Some(8000),
            connection_timeout: Some(10),
            request_timeout: Some(30),
            pronunciations: Vec::new(),
            request_pool_size: None,
            emotion_config: None,
        }
    }

    #[test]
    fn test_gnani_tts_module_exports() {
        // Verify all public types are accessible
        let _: GnaniGender = GnaniGender::default();
        let _: GnaniTTSLanguage = GnaniTTSLanguage::default();
    }

    #[test]
    fn test_gnani_tts_language_codes() {
        // Verify all language codes are valid
        let codes = GnaniTTSLanguage::all_codes();
        assert_eq!(codes.len(), 12);
        assert!(codes.contains(&"Hi-IN"));
        assert!(codes.contains(&"En-IN"));
        assert!(codes.contains(&"Kn-IN"));
    }

    #[test]
    fn test_gnani_tts_creation_via_base_trait() {
        let config = create_test_config();
        let result = GnaniTTS::create(config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_gnani_tts_single_speaker_support() {
        // Languages that support single-speaker mode
        assert!(GnaniTTSLanguage::Hindi.supports_single_speaker());
        assert!(GnaniTTSLanguage::EnglishIndia.supports_single_speaker());
        assert!(GnaniTTSLanguage::Tamil.supports_single_speaker());

        // Languages that only support multi-speaker
        assert!(!GnaniTTSLanguage::Malayalam.supports_single_speaker());
        assert!(!GnaniTTSLanguage::Bengali.supports_single_speaker());
        assert!(!GnaniTTSLanguage::Gujarati.supports_single_speaker());
    }

    #[test]
    fn test_gnani_tts_marathi_female_only() {
        // Marathi only supports female voice in single-speaker mode
        let genders = GnaniTTSLanguage::Marathi.supported_genders();
        assert_eq!(genders.len(), 1);
        assert_eq!(genders[0], GnaniGender::Female);
    }

    #[test]
    fn test_gnani_tts_gender_parsing() {
        assert_eq!(GnaniGender::from_str("MALE").unwrap(), GnaniGender::Male);
        assert_eq!(
            GnaniGender::from_str("female").unwrap(),
            GnaniGender::Female
        );
        assert_eq!(GnaniGender::from_str("M").unwrap(), GnaniGender::Male);
        assert_eq!(GnaniGender::from_str("F").unwrap(), GnaniGender::Female);
    }

    #[test]
    fn test_gnani_tts_language_display_names() {
        assert_eq!(GnaniTTSLanguage::Hindi.display_name(), "Hindi");
        assert_eq!(
            GnaniTTSLanguage::EnglishIndia.display_name(),
            "English (India)"
        );
        assert_eq!(GnaniTTSLanguage::Malayalam.display_name(), "Malayalam");
    }
}
