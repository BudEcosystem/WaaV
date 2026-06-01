//! WellSaid Labs TTS Configuration
//!
//! This module provides configuration types for the WellSaid Labs TTS API.

use serde::{Deserialize, Serialize};
use std::fmt;

use crate::core::tts::{TTSConfig, TTSError, TTSResult};

use super::{DEFAULT_SPEAKER_ID, MAX_TEXT_LENGTH};

// =============================================================================
// Model Selection
// =============================================================================

/// WellSaid TTS model selection
///
/// Two models are available:
/// - **Legacy**: Default model, supports all languages
/// - **Caruso**: English only, with AI Director capabilities
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum WellSaidModel {
    /// Default model with multi-language support
    #[default]
    Legacy,
    /// English-only model with AI Director (pitch, tempo, loudness control)
    Caruso,
}

impl WellSaidModel {
    /// Returns the API string representation of the model
    pub fn as_str(&self) -> &'static str {
        match self {
            WellSaidModel::Legacy => "legacy",
            WellSaidModel::Caruso => "caruso",
        }
    }

    /// Parse model from string
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "legacy" | "" => Some(WellSaidModel::Legacy),
            "caruso" => Some(WellSaidModel::Caruso),
            _ => None,
        }
    }

    /// Whether this model supports AI Director features
    pub fn supports_ai_director(&self) -> bool {
        matches!(self, WellSaidModel::Caruso)
    }

    /// Whether this model supports multiple languages
    pub fn is_multilingual(&self) -> bool {
        matches!(self, WellSaidModel::Legacy)
    }
}

impl fmt::Display for WellSaidModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =============================================================================
// Voice Avatar
// =============================================================================

/// WellSaid voice avatar information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WellSaidAvatar {
    /// Unique avatar identifier (e.g., "alana-b")
    pub avatar_id: String,
    /// Numeric speaker ID for TTS requests
    pub speaker_id: u32,
    /// Display name (e.g., "Alana B.")
    pub name: String,
    /// Gender (Male, Female)
    #[serde(default)]
    pub gender: Option<String>,
    /// Accent/language (e.g., "US English", "British English")
    #[serde(default)]
    pub accent: Option<String>,
    /// Speaking style (Narration, Promo, Conversational)
    #[serde(default)]
    pub style: Option<String>,
    /// Supported models for this avatar
    #[serde(default)]
    pub models: Vec<String>,
}

impl WellSaidAvatar {
    /// Check if this avatar supports the Caruso model
    pub fn supports_caruso(&self) -> bool {
        self.models.iter().any(|m| m.to_lowercase() == "caruso")
    }

    /// Check if this avatar supports the Legacy model
    pub fn supports_legacy(&self) -> bool {
        self.models.iter().any(|m| m.to_lowercase() == "legacy")
    }
}

// =============================================================================
// TTS Request
// =============================================================================

/// WellSaid TTS streaming request body
#[derive(Debug, Clone, Serialize)]
pub struct WellSaidStreamRequest {
    /// Numeric speaker ID for voice selection
    pub speaker_id: u32,
    /// Text to synthesize (max 1000 characters)
    pub text: String,
    /// Model selection (optional, defaults to "legacy")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl WellSaidStreamRequest {
    /// Create a new stream request
    pub fn new(speaker_id: u32, text: impl Into<String>) -> Self {
        Self {
            speaker_id,
            text: text.into(),
            model: None,
        }
    }

    /// Create request from config
    pub fn from_config(config: &WellSaidTtsConfig, text: &str) -> Self {
        Self {
            speaker_id: config.speaker_id,
            text: text.to_string(),
            model: if config.model == WellSaidModel::Caruso {
                Some("caruso".to_string())
            } else {
                None // Legacy is default, no need to specify
            },
        }
    }

    /// Set the model
    pub fn with_model(mut self, model: WellSaidModel) -> Self {
        self.model = if model == WellSaidModel::Caruso {
            Some("caruso".to_string())
        } else {
            None
        };
        self
    }
}

// =============================================================================
// TTS Configuration
// =============================================================================

/// WellSaid Labs TTS configuration
///
/// # Example
///
/// ```rust,ignore
/// use waav_gateway::core::tts::wellsaid::{WellSaidTtsConfig, WellSaidModel};
///
/// let config = WellSaidTtsConfig::new("your-api-key")
///     .with_speaker_id(3) // Alana B.
///     .with_model(WellSaidModel::Caruso);
/// ```
#[derive(Debug, Clone)]
pub struct WellSaidTtsConfig {
    /// WellSaid API key
    pub api_key: String,
    /// Speaker ID (voice avatar)
    pub speaker_id: u32,
    /// Model selection (Legacy or Caruso)
    pub model: WellSaidModel,
}

impl WellSaidTtsConfig {
    /// Create a new configuration with API key
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            speaker_id: DEFAULT_SPEAKER_ID,
            model: WellSaidModel::default(),
        }
    }

    /// Set the speaker ID
    pub fn with_speaker_id(mut self, speaker_id: u32) -> Self {
        self.speaker_id = speaker_id;
        self
    }

    /// Set the model
    pub fn with_model(mut self, model: WellSaidModel) -> Self {
        self.model = model;
        self
    }

    /// Create configuration from base TTSConfig
    ///
    /// Extracts WellSaid-specific settings from the base TTS config.
    /// The voice_id field is parsed as speaker_id (numeric).
    pub fn from_base(config: &TTSConfig) -> TTSResult<Self> {
        // Get API key from config or environment
        let api_key = if !config.api_key.is_empty() {
            config.api_key.clone()
        } else {
            std::env::var("WELLSAID_API_KEY").map_err(|_| {
                TTSError::InvalidConfiguration(
                    "WELLSAID_API_KEY environment variable not set and no api_key provided"
                        .to_string(),
                )
            })?
        };

        // Parse speaker_id from voice_id (numeric string)
        let speaker_id = config
            .voice_id
            .as_ref()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(DEFAULT_SPEAKER_ID);

        // Parse model from config.model field
        let model = if !config.model.is_empty() {
            WellSaidModel::from_str(&config.model).unwrap_or_default()
        } else {
            WellSaidModel::default()
        };

        let wellsaid_config = Self {
            api_key,
            speaker_id,
            model,
        };

        // Validate
        wellsaid_config.validate()?;

        Ok(wellsaid_config)
    }

    /// Build from the standardized TTS config (W1 keystone).
    ///
    /// WellSaid's config only carries voice (`speaker_id`) and model selection; it has no struct
    /// field for any standardized prosody/voice feature. The Caruso model's AI Director (pitch,
    /// tempo, loudness) is not represented as config state here, so there is nothing to map. Every
    /// [`TtsFeatures`](crate::core::tts::standard::TtsFeatures) field (speed, pitch, volume,
    /// stability, similarity_boost, style, use_speaker_boost, emotion, instructions, ssml, language,
    /// word_timestamps, streaming, seed, sample_rate) is a capability gap and is skipped — this is a
    /// pure `from_base` passthrough (speaker_id/model already flow through the flat base config).
    pub fn from_standard(std: &crate::core::tts::standard::StandardTTSConfig) -> TTSResult<Self> {
        Self::from_base(&std.base)
    }

    /// Validate the configuration
    pub fn validate(&self) -> TTSResult<()> {
        if self.api_key.is_empty() {
            return Err(TTSError::InvalidConfiguration(
                "WellSaid API key is required".to_string(),
            ));
        }

        if self.speaker_id == 0 {
            return Err(TTSError::InvalidConfiguration(
                "Speaker ID must be greater than 0".to_string(),
            ));
        }

        Ok(())
    }

    /// Validate text length
    pub fn validate_text(text: &str) -> TTSResult<()> {
        if text.len() > MAX_TEXT_LENGTH {
            return Err(TTSError::InvalidConfiguration(format!(
                "Text exceeds maximum length of {} characters (got {})",
                MAX_TEXT_LENGTH,
                text.len()
            )));
        }
        Ok(())
    }
}

impl Default for WellSaidTtsConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            speaker_id: DEFAULT_SPEAKER_ID,
            model: WellSaidModel::default(),
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // W1 keystone (TTS): WellSaid's config only holds voice/model selection (no prosody/voice
    // feature fields), so `from_standard` is a pure `from_base` passthrough. Standardized features
    // (here speed) are capability gaps and must be ignored, while speaker_id/model still flow
    // through from the flat base config.
    #[test]
    fn from_standard_passthrough_ignores_capability_gaps() {
        use crate::core::tts::standard::{ProviderExtras, StandardTTSConfig, TtsFeatures};
        let std = StandardTTSConfig {
            base: TTSConfig {
                provider: "wellsaid".into(),
                api_key: "test-key".into(),
                voice_id: Some("26".into()),
                model: "caruso".into(),
                ..Default::default()
            },
            features: TtsFeatures {
                speed: Some(1.5),   // capability gap: no prosody field, must be ignored
                pitch: Some(70.0),  // capability gap: AI Director not stored, must be ignored
                ssml: Some(true),   // capability gap: must be ignored
                ..Default::default()
            },
            extras: ProviderExtras::default(),
        };
        let cfg = WellSaidTtsConfig::from_standard(&std).unwrap();
        // Identical to a pure from_base of the same flat config (voice + model only).
        let base_cfg = WellSaidTtsConfig::from_base(&std.base).unwrap();
        assert_eq!(cfg.speaker_id, base_cfg.speaker_id);
        assert_eq!(cfg.speaker_id, 26); // from voice_id
        assert_eq!(cfg.model, WellSaidModel::Caruso); // from base.model
        assert_eq!(cfg.api_key, "test-key");
    }

    #[test]
    fn test_model_enum() {
        assert_eq!(WellSaidModel::Legacy.as_str(), "legacy");
        assert_eq!(WellSaidModel::Caruso.as_str(), "caruso");
        assert_eq!(WellSaidModel::default(), WellSaidModel::Legacy);
    }

    #[test]
    fn test_model_from_str() {
        assert_eq!(
            WellSaidModel::from_str("legacy"),
            Some(WellSaidModel::Legacy)
        );
        assert_eq!(
            WellSaidModel::from_str("caruso"),
            Some(WellSaidModel::Caruso)
        );
        assert_eq!(
            WellSaidModel::from_str("CARUSO"),
            Some(WellSaidModel::Caruso)
        );
        assert_eq!(WellSaidModel::from_str(""), Some(WellSaidModel::Legacy));
        assert_eq!(WellSaidModel::from_str("invalid"), None);
    }

    #[test]
    fn test_model_features() {
        assert!(!WellSaidModel::Legacy.supports_ai_director());
        assert!(WellSaidModel::Caruso.supports_ai_director());
        assert!(WellSaidModel::Legacy.is_multilingual());
        assert!(!WellSaidModel::Caruso.is_multilingual());
    }

    #[test]
    fn test_config_defaults() {
        let config = WellSaidTtsConfig::new("test-key");
        assert_eq!(config.api_key, "test-key");
        assert_eq!(config.speaker_id, DEFAULT_SPEAKER_ID);
        assert_eq!(config.model, WellSaidModel::Legacy);
    }

    #[test]
    fn test_config_builder() {
        let config = WellSaidTtsConfig::new("test-key")
            .with_speaker_id(26)
            .with_model(WellSaidModel::Caruso);

        assert_eq!(config.speaker_id, 26);
        assert_eq!(config.model, WellSaidModel::Caruso);
    }

    #[test]
    fn test_config_validation() {
        // Valid config
        let config = WellSaidTtsConfig::new("test-key");
        assert!(config.validate().is_ok());

        // Empty API key
        let config = WellSaidTtsConfig::default();
        assert!(config.validate().is_err());

        // Zero speaker ID
        let config = WellSaidTtsConfig::new("test-key").with_speaker_id(0);
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_from_base() {
        let base = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("26".to_string()),
            model: "caruso".to_string(),
            ..Default::default()
        };

        let config = WellSaidTtsConfig::from_base(&base).unwrap();
        assert_eq!(config.api_key, "test-key");
        assert_eq!(config.speaker_id, 26);
        assert_eq!(config.model, WellSaidModel::Caruso);
    }

    #[test]
    fn test_config_from_base_defaults() {
        let base = TTSConfig {
            api_key: "test-key".to_string(),
            ..Default::default()
        };

        let config = WellSaidTtsConfig::from_base(&base).unwrap();
        assert_eq!(config.speaker_id, DEFAULT_SPEAKER_ID);
        assert_eq!(config.model, WellSaidModel::Legacy);
    }

    #[test]
    fn test_text_validation() {
        // Valid text
        assert!(WellSaidTtsConfig::validate_text("Hello, world!").is_ok());

        // Too long
        let long_text = "a".repeat(MAX_TEXT_LENGTH + 1);
        assert!(WellSaidTtsConfig::validate_text(&long_text).is_err());

        // At limit
        let limit_text = "a".repeat(MAX_TEXT_LENGTH);
        assert!(WellSaidTtsConfig::validate_text(&limit_text).is_ok());
    }

    #[test]
    fn test_stream_request_serialization() {
        let request = WellSaidStreamRequest::new(3, "Hello world");
        let json = serde_json::to_string(&request).unwrap();

        assert!(json.contains("\"speaker_id\":3"));
        assert!(json.contains("\"text\":\"Hello world\""));
        assert!(!json.contains("\"model\"")); // model should be omitted for legacy
    }

    #[test]
    fn test_stream_request_with_caruso() {
        let request = WellSaidStreamRequest::new(26, "Hello").with_model(WellSaidModel::Caruso);
        let json = serde_json::to_string(&request).unwrap();

        assert!(json.contains("\"model\":\"caruso\""));
    }

    #[test]
    fn test_stream_request_from_config() {
        let config = WellSaidTtsConfig::new("test-key")
            .with_speaker_id(26)
            .with_model(WellSaidModel::Caruso);

        let request = WellSaidStreamRequest::from_config(&config, "Test text");

        assert_eq!(request.speaker_id, 26);
        assert_eq!(request.text, "Test text");
        assert_eq!(request.model, Some("caruso".to_string()));
    }

    #[test]
    fn test_avatar_caruso_support() {
        let avatar = WellSaidAvatar {
            avatar_id: "alana-b".to_string(),
            speaker_id: 3,
            name: "Alana B.".to_string(),
            gender: Some("Female".to_string()),
            accent: Some("US English".to_string()),
            style: Some("Narration".to_string()),
            models: vec!["caruso".to_string(), "legacy".to_string()],
        };

        assert!(avatar.supports_caruso());
        assert!(avatar.supports_legacy());

        let legacy_only = WellSaidAvatar {
            avatar_id: "test".to_string(),
            speaker_id: 100,
            name: "Test".to_string(),
            gender: None,
            accent: None,
            style: None,
            models: vec!["legacy".to_string()],
        };

        assert!(!legacy_only.supports_caruso());
        assert!(legacy_only.supports_legacy());
    }
}
