//! Bhashini TTS configuration.
//!
//! This module provides configuration types for the Bhashini TTS provider.

use crate::core::tts::base::{TTSConfig, TTSError};

// Re-use language and pipeline types from STT module
pub use crate::core::stt::bhashini::{
    BhashiniLanguage, BhashiniPipelineProvider, LanguageFamily, BHASHINI_CONFIG_URL,
};

/// Default sample rate for TTS output (22.05 kHz).
pub const DEFAULT_TTS_SAMPLE_RATE: u32 = 22050;

/// Bhashini TTS audio format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BhashiniTtsAudioFormat {
    /// WAV format (default).
    #[default]
    Wav,
    /// MP3 format.
    Mp3,
}

impl BhashiniTtsAudioFormat {
    /// Get the format as a string.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Wav => "wav",
            Self::Mp3 => "mp3",
        }
    }

    /// Get the content type for the format.
    pub fn content_type(&self) -> &'static str {
        match self {
            Self::Wav => "audio/wav",
            Self::Mp3 => "audio/mpeg",
        }
    }

    /// Parse from string.
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "wav" | "wave" => Some(Self::Wav),
            "mp3" | "mpeg" => Some(Self::Mp3),
            _ => None,
        }
    }
}

/// Gender for TTS voice selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BhashiniTtsGender {
    /// Male voice.
    Male,
    /// Female voice (default).
    #[default]
    Female,
}

impl BhashiniTtsGender {
    /// Get the gender as a string for the API.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Male => "male",
            Self::Female => "female",
        }
    }

    /// Parse from string.
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "male" | "m" => Some(Self::Male),
            "female" | "f" => Some(Self::Female),
            _ => None,
        }
    }
}

/// Bhashini TTS configuration.
#[derive(Debug, Clone)]
pub struct BhashiniTtsConfig {
    /// User ID for authentication.
    pub user_id: String,
    /// ULCA API key for authentication.
    pub ulca_api_key: String,
    /// Optional inference API key (if pre-obtained).
    pub inference_api_key: Option<String>,
    /// Target language for TTS.
    pub language: BhashiniLanguage,
    /// Audio format for output.
    pub audio_format: BhashiniTtsAudioFormat,
    /// Sample rate for output.
    pub sample_rate: u32,
    /// Voice gender preference.
    pub gender: BhashiniTtsGender,
    /// Pipeline provider to use.
    pub pipeline_provider: BhashiniPipelineProvider,
    /// Custom callback URL (if not using pipeline config).
    pub custom_callback_url: Option<String>,
    /// Custom service ID (if not using pipeline config).
    pub custom_service_id: Option<String>,
}

impl Default for BhashiniTtsConfig {
    fn default() -> Self {
        Self {
            user_id: String::new(),
            ulca_api_key: String::new(),
            inference_api_key: None,
            language: BhashiniLanguage::default(),
            audio_format: BhashiniTtsAudioFormat::default(),
            sample_rate: DEFAULT_TTS_SAMPLE_RATE,
            gender: BhashiniTtsGender::default(),
            pipeline_provider: BhashiniPipelineProvider::default(),
            custom_callback_url: None,
            custom_service_id: None,
        }
    }
}

impl BhashiniTtsConfig {
    /// Create from base TTSConfig.
    ///
    /// API key format: `userId|ulcaApiKey` or `userId|ulcaApiKey|inferenceApiKey`
    pub fn from_base(config: TTSConfig) -> Result<Self, TTSError> {
        // Parse API key
        let parts: Vec<&str> = config.api_key.split('|').collect();
        if parts.len() < 2 {
            return Err(TTSError::InvalidConfiguration(
                "Bhashini API key must be in format 'userId|ulcaApiKey' or 'userId|ulcaApiKey|inferenceApiKey'".to_string(),
            ));
        }

        let user_id = parts[0].trim().to_string();
        let ulca_api_key = parts[1].trim().to_string();
        let inference_api_key = parts.get(2).map(|s| s.trim().to_string());

        if user_id.is_empty() {
            return Err(TTSError::InvalidConfiguration(
                "User ID cannot be empty".to_string(),
            ));
        }
        if ulca_api_key.is_empty() {
            return Err(TTSError::InvalidConfiguration(
                "ULCA API key cannot be empty".to_string(),
            ));
        }

        // Parse language from voice_id or model
        let language_str = config
            .voice_id
            .as_deref()
            .or_else(|| {
                if config.model.is_empty() {
                    None
                } else {
                    Some(config.model.as_str())
                }
            })
            .unwrap_or("hi");

        // Try to parse language, extract gender if present (e.g., "hi-female")
        let (lang_code, gender) = if language_str.contains('-') {
            let parts: Vec<&str> = language_str.split('-').collect();
            let gender = parts.get(1).and_then(|g| BhashiniTtsGender::from_str(g));
            (parts[0], gender)
        } else {
            (language_str, None)
        };

        let language = BhashiniLanguage::from_code(lang_code)
            .ok_or_else(|| {
                TTSError::InvalidConfiguration(format!(
                    "Unsupported language code: {}. Supported: hi, ta, te, kn, ml, bn, mr, gu, pa, or, ur, as, sa, en, etc.",
                    lang_code
                ))
            })?;

        // Parse audio format
        let audio_format = config
            .audio_format
            .as_deref()
            .and_then(BhashiniTtsAudioFormat::from_str)
            .unwrap_or_default();

        // Parse sample rate
        let sample_rate = config.sample_rate.unwrap_or(DEFAULT_TTS_SAMPLE_RATE);

        Ok(Self {
            user_id,
            ulca_api_key,
            inference_api_key,
            language,
            audio_format,
            sample_rate,
            gender: gender.unwrap_or_default(),
            pipeline_provider: BhashiniPipelineProvider::default(),
            custom_callback_url: None,
            custom_service_id: None,
        })
    }

    /// Validate the configuration.
    pub fn validate(&self) -> Result<(), TTSError> {
        if self.user_id.is_empty() {
            return Err(TTSError::InvalidConfiguration(
                "User ID is required".to_string(),
            ));
        }
        if self.ulca_api_key.is_empty() {
            return Err(TTSError::InvalidConfiguration(
                "ULCA API key is required".to_string(),
            ));
        }
        Ok(())
    }

    /// Get the pipeline ID to use.
    pub fn pipeline_id(&self) -> &'static str {
        self.pipeline_provider.pipeline_id()
    }

    /// Get the TTS service ID for the selected language.
    pub fn tts_service_id(&self) -> &'static str {
        self.language.tts_service_id()
    }
}

impl BhashiniLanguage {
    /// Get the TTS service ID for this language.
    pub fn tts_service_id(&self) -> &'static str {
        // Bhashini TTS uses AI4Bharat models for most languages
        match self.family() {
            LanguageFamily::Dravidian => "ai4bharat/indic-tts-coqui-dravidian-gpu--t4",
            LanguageFamily::IndoAryan => {
                if matches!(self, Self::Hindi) {
                    "ai4bharat/indic-tts-coqui-hi-gpu--t4"
                } else {
                    "ai4bharat/indic-tts-coqui-indo_aryan-gpu--t4"
                }
            }
            LanguageFamily::Misc => {
                if matches!(self, Self::English) {
                    "ai4bharat/indic-tts-coqui-misc-gpu--t4"
                } else {
                    "ai4bharat/indic-tts-coqui-misc-gpu--t4"
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_from_base_valid() {
        let base_config = TTSConfig {
            api_key: "test_user|test_key".to_string(),
            voice_id: Some("hi".to_string()),
            sample_rate: Some(22050),
            ..Default::default()
        };

        let config = BhashiniTtsConfig::from_base(base_config).unwrap();
        assert_eq!(config.user_id, "test_user");
        assert_eq!(config.ulca_api_key, "test_key");
        assert_eq!(config.language, BhashiniLanguage::Hindi);
    }

    #[test]
    fn test_config_from_base_with_inference_key() {
        let base_config = TTSConfig {
            api_key: "user|key|inference_key".to_string(),
            voice_id: Some("ta".to_string()),
            ..Default::default()
        };

        let config = BhashiniTtsConfig::from_base(base_config).unwrap();
        assert_eq!(config.inference_api_key, Some("inference_key".to_string()));
        assert_eq!(config.language, BhashiniLanguage::Tamil);
    }

    #[test]
    fn test_config_from_base_with_gender() {
        let base_config = TTSConfig {
            api_key: "user|key".to_string(),
            voice_id: Some("hi-male".to_string()),
            ..Default::default()
        };

        let config = BhashiniTtsConfig::from_base(base_config).unwrap();
        assert_eq!(config.language, BhashiniLanguage::Hindi);
        assert_eq!(config.gender, BhashiniTtsGender::Male);
    }

    #[test]
    fn test_config_from_base_invalid() {
        let base_config = TTSConfig {
            api_key: "invalid".to_string(),
            ..Default::default()
        };

        let result = BhashiniTtsConfig::from_base(base_config);
        assert!(result.is_err());
    }

    #[test]
    fn test_audio_format() {
        assert_eq!(BhashiniTtsAudioFormat::Wav.as_str(), "wav");
        assert_eq!(BhashiniTtsAudioFormat::Mp3.as_str(), "mp3");
        assert_eq!(BhashiniTtsAudioFormat::Wav.content_type(), "audio/wav");
        assert_eq!(BhashiniTtsAudioFormat::Mp3.content_type(), "audio/mpeg");
    }

    #[test]
    fn test_gender() {
        assert_eq!(BhashiniTtsGender::Male.as_str(), "male");
        assert_eq!(BhashiniTtsGender::Female.as_str(), "female");
        assert_eq!(BhashiniTtsGender::from_str("male"), Some(BhashiniTtsGender::Male));
        assert_eq!(BhashiniTtsGender::from_str("f"), Some(BhashiniTtsGender::Female));
    }

    #[test]
    fn test_tts_service_id() {
        assert!(BhashiniLanguage::Hindi.tts_service_id().contains("hi"));
        assert!(BhashiniLanguage::Tamil.tts_service_id().contains("dravidian"));
    }

    #[test]
    fn test_config_validation() {
        let mut config = BhashiniTtsConfig::default();
        assert!(config.validate().is_err());

        config.user_id = "user".to_string();
        config.ulca_api_key = "key".to_string();
        assert!(config.validate().is_ok());
    }
}
