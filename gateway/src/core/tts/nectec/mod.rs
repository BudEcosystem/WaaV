//! NECTEC (AI for Thai) TTS provider module.
//!
//! This module provides Text-to-Speech integration with Thailand's NECTEC
//! AI for Thai platform, using the VAJA9 TTS engine.
//!
//! # Overview
//!
//! NECTEC (National Electronics and Computer Technology Center) provides free Thai language
//! AI services through the AI for Thai platform. The TTS service uses a two-step HTTP process:
//! request synthesis, then download the audio file.
//!
//! # Audio Output
//!
//! - Format: WAV
//! - Sample Rate: Typically 22050 Hz
//! - Encoding: PCM 16-bit
//!
//! # Voices
//!
//! | Speaker ID | Voice | Language |
//! |------------|-------|----------|
//! | 0 | Male | Thai |
//! | 1 | Female | Thai |
//!
//! # Limitations
//!
//! - Max 300 characters per request
//! - Long text is automatically chunked and concatenated
//!
//! # Authentication
//!
//! Uses API key authentication via `Apikey` header. Get your API key from:
//! <https://aiforthai.in.th>
//!
//! # Usage
//!
//! ```ignore
//! use waav_gateway::core::tts::{NectecTts, BaseTTS, TTSConfig};
//!
//! let config = TTSConfig {
//!     provider: "nectec".to_string(),
//!     api_key: std::env::var("NECTEC_API_KEY").unwrap(),
//!     voice_id: Some("female".to_string()),
//!     ..Default::default()
//! };
//!
//! let mut tts = NectecTts::new(config)?;
//! tts.connect().await?;
//!
//! // Synthesize text
//! tts.speak("สวัสดีครับ", true).await?;
//! ```

mod client;
pub mod config;

pub use client::NectecTts;
pub use config::{
    API_KEY_HEADER, DEFAULT_REQUEST_TIMEOUT, DEFAULT_SAMPLE_RATE, LIB_HEADER, LIB_VALUE,
    MAX_TEXT_LENGTH, NectecTtsConfig, NectecTtsError, NectecVoice, VAJA9_ENDPOINT, Vaja9Request,
    Vaja9Response, chunk_text,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tts::base::{BaseTTS, TTSConfig};

    #[test]
    fn test_module_exports() {
        // Test that all expected items are exported
        let _ = NectecVoice::Male;
        let _ = NectecVoice::Female;
        assert!(!VAJA9_ENDPOINT.is_empty());
        assert!(!API_KEY_HEADER.is_empty());
        assert_eq!(MAX_TEXT_LENGTH, 300);
    }

    #[test]
    fn test_nectec_tts_creation() {
        let config = TTSConfig {
            provider: "nectec".to_string(),
            api_key: "test_key".to_string(),
            voice_id: Some("male".to_string()),
            ..Default::default()
        };
        let result = NectecTts::new(config);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_nectec_tts_lifecycle() {
        let config = TTSConfig {
            provider: "nectec".to_string(),
            api_key: "test_key".to_string(),
            voice_id: Some("female".to_string()),
            ..Default::default()
        };
        let mut tts = NectecTts::new(config).unwrap();

        // Test connect
        let connect_result = tts.connect().await;
        assert!(connect_result.is_ok());
        assert!(tts.is_ready());

        // Test disconnect
        let disconnect_result = tts.disconnect().await;
        assert!(disconnect_result.is_ok());
        assert!(!tts.is_ready());
    }

    #[test]
    fn test_constants() {
        assert_eq!(DEFAULT_SAMPLE_RATE, 22050);
        assert_eq!(MAX_TEXT_LENGTH, 300);
        assert_eq!(DEFAULT_REQUEST_TIMEOUT, 60);
    }

    #[test]
    fn test_config_from_base() {
        let base = TTSConfig {
            provider: "nectec".to_string(),
            api_key: "my_api_key".to_string(),
            voice_id: Some("female".to_string()),
            ..Default::default()
        };
        let nectec_config = NectecTtsConfig::from_base(&base);
        assert!(nectec_config.is_ok());
        let config = nectec_config.unwrap();
        assert_eq!(config.api_key, "my_api_key");
        assert_eq!(config.voice, NectecVoice::Female);
    }

    #[test]
    fn test_error_types() {
        let error = NectecTtsError::InvalidApiKey;
        assert!(error.to_string().contains("API key"));

        let error = NectecTtsError::RateLimitExceeded;
        assert!(error.to_string().contains("rate limit"));

        let error = NectecTtsError::TextTooLong(500);
        assert!(error.to_string().contains("500"));
    }

    #[test]
    fn test_chunk_text_function() {
        let short_text = "Hello world";
        let chunks = chunk_text(short_text, 300);
        assert_eq!(chunks.len(), 1);

        let long_text = "A".repeat(600);
        let chunks = chunk_text(&long_text, 300);
        assert!(chunks.len() >= 2);
    }

    #[test]
    fn test_vaja9_request_creation() {
        let request = Vaja9Request::new("สวัสดีครับ", NectecVoice::Male);
        assert_eq!(request.speaker, 0);
        assert_eq!(request.input_text, "สวัสดีครับ");

        let request = Vaja9Request::new("สวัสดีค่ะ", NectecVoice::Female);
        assert_eq!(request.speaker, 1);
    }

    #[test]
    fn test_voice_selection() {
        assert_eq!(
            NectecVoice::from_str_relaxed("male"),
            Some(NectecVoice::Male)
        );
        assert_eq!(
            NectecVoice::from_str_relaxed("female"),
            Some(NectecVoice::Female)
        );
        assert_eq!(NectecVoice::from_str_relaxed("0"), Some(NectecVoice::Male));
        assert_eq!(
            NectecVoice::from_str_relaxed("1"),
            Some(NectecVoice::Female)
        );
    }

    #[test]
    fn test_provider_info() {
        let config = TTSConfig {
            provider: "nectec".to_string(),
            api_key: "test_key".to_string(),
            voice_id: Some("female".to_string()),
            ..Default::default()
        };
        let tts = NectecTts::new(config).unwrap();
        let info = tts.get_provider_info();

        assert_eq!(info["provider"], "nectec");
        assert!(
            info["supported_languages"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("th"))
        );
    }
}
