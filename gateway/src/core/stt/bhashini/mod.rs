//! Bhashini (ULCA) Speech-to-Text Provider
//!
//! This module provides integration with Bhashini's ULCA (Universal Language
//! Contribution APIs) for speech recognition supporting 22+ Indian languages.
//!
//! ## Features
//!
//! - Support for all 22 scheduled Indian languages plus English
//! - Government of India (MeitY) backed infrastructure
//! - Multiple ASR models from AI4Bharat, IITM, IISC, CDAC
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
//! use waav_gateway::core::stt::{BaseSTT, STTConfig};
//! use waav_gateway::core::stt::bhashini::BhashiniStt;
//!
//! let config = STTConfig {
//!     api_key: "userId|ulcaApiKey".to_string(),
//!     language: "hi".to_string(), // Hindi
//!     sample_rate: 16000,
//!     ..Default::default()
//! };
//!
//! let mut stt = BhashiniStt::new(config)?;
//! stt.connect().await?;
//! stt.send_audio(&audio_data).await?;
//! stt.disconnect().await?; // Transcription happens here
//! ```
//!
//! ## Available Languages
//!
//! | Language | Code | Family |
//! |----------|------|--------|
//! | Hindi | hi | Indo-Aryan |
//! | Tamil | ta | Dravidian |
//! | Telugu | te | Dravidian |
//! | Kannada | kn | Dravidian |
//! | Malayalam | ml | Dravidian |
//! | Bengali | bn | Indo-Aryan |
//! | Marathi | mr | Indo-Aryan |
//! | Gujarati | gu | Indo-Aryan |
//! | Punjabi | pa | Indo-Aryan |
//! | Odia | or | Indo-Aryan |
//! | Urdu | ur | Indo-Aryan |
//! | Assamese | as | Misc |
//! | Sanskrit | sa | Misc |
//! | English | en | Misc |
//! | + 10 more languages |
//!
//! ## Audio Formats
//!
//! | Format | Notes |
//! |--------|-------|
//! | WAV | Recommended for Android/Web |
//! | FLAC | Recommended for iOS |
//! | MP3 | Supported |
//!
//! ## Sample Rates
//!
//! - Minimum: 8000 Hz
//! - Recommended: 16000 Hz
//!
//! ## Pipeline Providers
//!
//! | Provider | Pipeline ID | Notes |
//! |----------|-------------|-------|
//! | MeitY | 64392f96daac500b55c543cd | Official government pipeline |
//! | AI4Bharat | 643930aa521a4b1ba0f4c41d | Research partner pipeline |
//!
//! ## API Endpoints
//!
//! - Config: `https://meity-auth.ulcacontrib.org/ulca/apis/v0/model/getModelsPipeline`
//! - Compute: `https://dhruva-api.bhashini.gov.in/services/inference/pipeline`

mod client;
mod config;
mod messages;

pub use client::BhashiniStt;
pub use config::{
    BhashiniAudioFormat, BhashiniLanguage, BhashiniPipelineProvider, BhashiniSttConfig,
    LanguageFamily,
    // Constants
    AI4BHARAT_PIPELINE_ID, BHASHINI_COMPUTE_URL, BHASHINI_CONFIG_URL, DEFAULT_SAMPLE_RATE,
    MAX_AUDIO_DURATION_SECS, MAX_AUDIO_SIZE_BYTES, MEITY_PIPELINE_ID, MIN_SAMPLE_RATE,
};
pub use messages::{
    PipelineComputeRequest, PipelineComputeResponse, PipelineConfigRequest, PipelineConfigResponse,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::stt::base::STTConfig;

    fn create_test_config() -> STTConfig {
        STTConfig {
            api_key: "test_user|test_ulca_key".to_string(),
            language: "hi".to_string(),
            sample_rate: 16000,
            ..Default::default()
        }
    }

    #[test]
    fn test_bhashini_module_exports() {
        // Verify all public types are accessible
        let _: BhashiniLanguage = BhashiniLanguage::default();
        let _: BhashiniAudioFormat = BhashiniAudioFormat::default();
        let _: BhashiniPipelineProvider = BhashiniPipelineProvider::default();
    }

    #[test]
    fn test_bhashini_language_codes() {
        assert_eq!(BhashiniLanguage::Hindi.as_code(), "hi");
        assert_eq!(BhashiniLanguage::Tamil.as_code(), "ta");
        assert_eq!(BhashiniLanguage::Telugu.as_code(), "te");
        assert_eq!(BhashiniLanguage::Bengali.as_code(), "bn");
        assert_eq!(BhashiniLanguage::English.as_code(), "en");
    }

    #[test]
    fn test_bhashini_audio_formats() {
        assert_eq!(BhashiniAudioFormat::Wav.as_str(), "wav");
        assert_eq!(BhashiniAudioFormat::Flac.as_str(), "flac");
        assert_eq!(BhashiniAudioFormat::Mp3.as_str(), "mp3");
    }

    #[test]
    fn test_bhashini_stt_creation() {
        let config = create_test_config();
        let result = BhashiniStt::new(config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_bhashini_config_from_base() {
        let config = create_test_config();
        let bhashini_config = BhashiniSttConfig::from_base(config).unwrap();
        assert_eq!(bhashini_config.language, BhashiniLanguage::Hindi);
        assert_eq!(bhashini_config.audio_format, BhashiniAudioFormat::Wav);
        assert_eq!(bhashini_config.sample_rate, 16000);
    }

    #[test]
    fn test_bhashini_config_validation_missing_api_key() {
        let config = BhashiniSttConfig::default();
        let result = config.validate();
        assert!(result.is_err());
    }

    #[test]
    fn test_bhashini_language_all() {
        let languages = BhashiniLanguage::all();
        assert_eq!(languages.len(), 24);
        assert!(languages.contains(&BhashiniLanguage::Hindi));
        assert!(languages.contains(&BhashiniLanguage::Tamil));
        assert!(languages.contains(&BhashiniLanguage::English));
    }

    #[test]
    fn test_bhashini_language_display_names() {
        assert_eq!(BhashiniLanguage::Hindi.display_name(), "Hindi");
        assert_eq!(BhashiniLanguage::Tamil.display_name(), "Tamil");
        assert_eq!(BhashiniLanguage::English.display_name(), "English");
    }

    #[test]
    fn test_bhashini_language_families() {
        assert_eq!(BhashiniLanguage::Tamil.family(), LanguageFamily::Dravidian);
        assert_eq!(BhashiniLanguage::Hindi.family(), LanguageFamily::IndoAryan);
        assert_eq!(BhashiniLanguage::English.family(), LanguageFamily::Misc);
    }

    #[test]
    fn test_bhashini_pipeline_ids() {
        assert_eq!(
            BhashiniPipelineProvider::MeitY.pipeline_id(),
            MEITY_PIPELINE_ID
        );
        assert_eq!(
            BhashiniPipelineProvider::AI4Bharat.pipeline_id(),
            AI4BHARAT_PIPELINE_ID
        );
    }

    #[test]
    fn test_bhashini_endpoint_urls() {
        assert_eq!(
            BHASHINI_CONFIG_URL,
            "https://meity-auth.ulcacontrib.org/ulca/apis/v0/model/getModelsPipeline"
        );
        assert_eq!(
            BHASHINI_COMPUTE_URL,
            "https://dhruva-api.bhashini.gov.in/services/inference/pipeline"
        );
    }

    #[test]
    fn test_bhashini_constants() {
        assert_eq!(DEFAULT_SAMPLE_RATE, 16000);
        assert_eq!(MIN_SAMPLE_RATE, 8000);
        assert_eq!(MAX_AUDIO_DURATION_SECS, 300);
        assert_eq!(MAX_AUDIO_SIZE_BYTES, 10 * 1024 * 1024);
    }
}
