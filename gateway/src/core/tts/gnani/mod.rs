//! Gnani.ai Text-to-Speech Provider
//!
//! Gnani's REST text-to-speech API (docs.gnani.ai, "Text-to-Speech (REST)"):
//! `POST https://api.vachana.ai/api/v1/tts/inference` with the `X-API-Key-ID` header. The former
//! `asr.gnani.ai/synthesize` (token + access key) is not in Gnani's docs, and the host sends no HTTP
//! response.
//!
//! ## Models and voices
//!
//! - `timbre-v3.0` (default): 13 voices, each speaks all ten languages. A session that names no
//!   voice gets Shivani (female) or Avinash (male).
//! - `timbre-v2.5`: 42 voices, one preferred language each; the default is the language's preferred
//!   voice of the gender.
//!
//! A voice belongs to one model (Gnani answers 400 otherwise), so naming a voice selects its model.
//! `voice_id` may instead be a language code (`Hi-IN`, the former convention); `model` may name the
//! model or, as before, `MALE`/`FEMALE`.
//!
//! ## Languages
//!
//! Hindi, English, Tamil, Telugu, Kannada, Malayalam, Marathi, Punjabi, Bengali and Gujarati
//! (`hi-IN`, `en-IN`, ...). The former `Hi-IN-al` is Hindi; Nepali has no voice and is refused.
//!
//! ## Authentication
//!
//! One API key from Gnani's console, sent as `X-API-Key-ID`: the server's `GNANI_TOKEN` (or the
//! session's `api_key`). `GNANI_ACCESS_KEY` is not used for text-to-speech.
//!
//! ## Audio
//!
//! Raw 16-bit mono PCM (`container: raw`, `encoding: linear_pcm`) at the session's sample rate
//! (8000 by default; 8000, 16000, 22050, 24000, 44100 or 48000).

mod config;
mod provider;

pub use config::{GnaniGender, GnaniModel, GnaniTTSConfig, GnaniTTSLanguage};
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
            api_base: None,
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
