//! Bhashini TTS configuration.
//!
//! This module provides configuration types for the Bhashini TTS provider.

use crate::core::tts::base::{TTSConfig, TTSError};

// Re-use language and pipeline types from STT module
pub use crate::core::stt::bhashini::{
    BHASHINI_CONFIG_URL, BhashiniLanguage, BhashiniPipelineProvider, LanguageFamily,
};

fn validate_bhashini_tts_endpoint(source: &str, endpoint: &str) -> Result<(), TTSError> {
    let endpoint = endpoint.trim();
    if endpoint.is_empty() {
        return Ok(());
    }
    crate::core::net::validate_url_for_ssrf(endpoint, crate::core::net::HTTP_URL_SCHEMES).map_err(
        |msg| TTSError::InvalidConfiguration(format!("{source} rejected (SSRF protection): {msg}")),
    )
}

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
    /// Test-only base-URL override for the pipeline CONFIG POST (scheme+host swap; path/query kept).
    pub endpoint_override: Option<String>,
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
            endpoint_override: None,
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
            .or({
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
            endpoint_override: None,
        })
    }

    /// Build from the standardized TTS config. Bhashini's surface is a fixed AI4Bharat
    /// pipeline, so only the features that match real fields are mapped: `sample_rate`
    /// overrides the output rate and `language` re-selects the Bhashini language (when it
    /// parses to a supported code). Provider-specific knobs (`custom_callback_url`,
    /// `custom_service_id`) are read from the `extras` passthrough. Features with no
    /// Bhashini field (speed, pitch, volume, stability, similarity_boost, style,
    /// use_speaker_boost, emotion, instructions, ssml, word_timestamps, streaming, seed)
    /// are skipped.
    pub fn from_standard(
        std: &crate::core::tts::standard::StandardTTSConfig,
    ) -> Result<Self, TTSError> {
        let f = &std.features;
        let mut cfg = Self::from_base(std.base.clone())?;

        if let Some(rate) = f.sample_rate {
            cfg.sample_rate = rate;
        }
        if let Some(lang) = f.language.as_deref()
            && let Some(language) = BhashiniLanguage::from_code(lang)
        {
            cfg.language = language;
        }

        // Provider-specific passthrough.
        if let Some(url) = std
            .extras
            .0
            .get("custom_callback_url")
            .and_then(|v| v.as_str())
        {
            cfg.custom_callback_url = Some(url.to_string());
        }
        if let Some(id) = std
            .extras
            .0
            .get("custom_service_id")
            .and_then(|v| v.as_str())
        {
            cfg.custom_service_id = Some(id.to_string());
        }

        cfg.endpoint_override = std.endpoint_override().map(String::from);

        cfg.validate()?;

        Ok(cfg)
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
        if let Some(endpoint) = &self.endpoint_override {
            validate_bhashini_tts_endpoint("endpoint_override", endpoint)?;
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
            // The Misc family uses a single coqui model regardless of English vs other languages
            // (both branches were identical — collapsed; clippy if_same_then_else).
            LanguageFamily::Misc => "ai4bharat/indic-tts-coqui-misc-gpu--t4",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Bhashini maps only the features that have real fields: sample_rate -> output rate,
    // language -> Bhashini language, plus the custom_service_id / custom_callback_url
    // extras passthrough. Prosody/style features have no Bhashini field and are skipped.
    #[test]
    fn from_standard_maps_sample_rate_language_and_extras() {
        use crate::core::tts::standard::{ProviderExtras, StandardTTSConfig, TtsFeatures};
        let mut extras = serde_json::Map::new();
        extras.insert("custom_service_id".into(), serde_json::json!("svc-123"));
        extras.insert(
            "custom_callback_url".into(),
            serde_json::json!("https://cb.example"),
        );
        let std = StandardTTSConfig {
            base: TTSConfig {
                provider: "bhashini".into(),
                api_key: "user|key".into(),
                voice_id: Some("hi".into()),
                sample_rate: Some(22050),
                ..Default::default()
            },
            features: TtsFeatures {
                sample_rate: Some(16000),
                language: Some("ta".into()),
                speed: Some(1.5), // capability gap: Bhashini has no speed field, must be ignored
                ..Default::default()
            },
            extras: ProviderExtras(extras),
        };
        let cfg = BhashiniTtsConfig::from_standard(&std).unwrap();
        assert_eq!(cfg.sample_rate, 16000);
        assert_eq!(cfg.language, BhashiniLanguage::Tamil);
        assert_eq!(cfg.custom_service_id, Some("svc-123".to_string())); // extras passthrough
        assert_eq!(
            cfg.custom_callback_url,
            Some("https://cb.example".to_string())
        );
    }

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
        assert_eq!(
            BhashiniTtsGender::from_str("male"),
            Some(BhashiniTtsGender::Male)
        );
        assert_eq!(
            BhashiniTtsGender::from_str("f"),
            Some(BhashiniTtsGender::Female)
        );
    }

    #[test]
    fn test_tts_service_id() {
        assert!(BhashiniLanguage::Hindi.tts_service_id().contains("hi"));
        assert!(
            BhashiniLanguage::Tamil
                .tts_service_id()
                .contains("dravidian")
        );
    }

    #[test]
    fn test_config_validation() {
        let mut config = BhashiniTtsConfig::default();
        assert!(config.validate().is_err());

        config.user_id = "user".to_string();
        config.ulca_api_key = "key".to_string();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validation_rejects_ssrf_endpoint_override() {
        let _env = crate::core::net::ssrf_env_lock();
        let mut config = BhashiniTtsConfig {
            user_id: "user".to_string(),
            ulca_api_key: "key".to_string(),
            ..Default::default()
        };

        config.endpoint_override = Some("https://bhashini-proxy.example.com".to_string());
        assert!(config.validate().is_ok());

        config.endpoint_override = Some("http://127.0.0.1:9000".to_string());
        let err = config
            .validate()
            .expect_err("loopback endpoint_override must be rejected");
        assert!(err.to_string().contains("SSRF protection"), "{err}");

        config.endpoint_override = Some("file:///tmp/socket".to_string());
        let err = config
            .validate()
            .expect_err("non-HTTP endpoint_override must be rejected");
        assert!(err.to_string().contains("SSRF protection"), "{err}");
    }
}
