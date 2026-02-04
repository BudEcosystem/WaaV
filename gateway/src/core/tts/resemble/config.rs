//! Resemble AI TTS Configuration
//!
//! This module provides configuration types for the Resemble AI TTS API.

use serde::{Deserialize, Serialize};
use std::fmt;

use crate::core::tts::{TTSConfig, TTSError, TTSResult};

use super::{DEFAULT_SAMPLE_RATE, MAX_TEXT_LENGTH_STREAM};

// =============================================================================
// Model Selection
// =============================================================================

/// Resemble AI TTS model selection
///
/// Three models are available:
/// - **Chatterbox**: Standard high-quality synthesis
/// - **ChatterboxTurbo**: Low-latency synthesis with paralinguistic tag support
/// - **ChatterboxMultilingual**: 24+ language support
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ResembleModel {
    /// Standard high-quality synthesis model
    #[default]
    Chatterbox,
    /// Low-latency synthesis (~350M params) with paralinguistic tags
    ChatterboxTurbo,
    /// Multilingual support (24+ languages)
    ChatterboxMultilingual,
}

impl ResembleModel {
    /// Returns the API string representation of the model
    pub fn as_str(&self) -> &'static str {
        match self {
            ResembleModel::Chatterbox => "chatterbox",
            ResembleModel::ChatterboxTurbo => "chatterbox-turbo",
            ResembleModel::ChatterboxMultilingual => "chatterbox-multilingual",
        }
    }

    /// Parse model from string
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "chatterbox" | "" => Some(ResembleModel::Chatterbox),
            "chatterbox-turbo" | "chatterbox_turbo" | "turbo" => {
                Some(ResembleModel::ChatterboxTurbo)
            }
            "chatterbox-multilingual" | "chatterbox_multilingual" | "multilingual" => {
                Some(ResembleModel::ChatterboxMultilingual)
            }
            _ => None,
        }
    }

    /// Whether this model supports paralinguistic tags
    pub fn supports_paralinguistic_tags(&self) -> bool {
        matches!(self, ResembleModel::ChatterboxTurbo)
    }

    /// Whether this model supports multiple languages
    pub fn is_multilingual(&self) -> bool {
        matches!(self, ResembleModel::ChatterboxMultilingual)
    }
}

impl fmt::Display for ResembleModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =============================================================================
// Output Format
// =============================================================================

/// Resemble AI output audio format
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ResembleOutputFormat {
    /// WAV format (default)
    #[default]
    Wav,
    /// MP3 format
    Mp3,
}

impl ResembleOutputFormat {
    /// Returns the API string representation of the format
    pub fn as_str(&self) -> &'static str {
        match self {
            ResembleOutputFormat::Wav => "wav",
            ResembleOutputFormat::Mp3 => "mp3",
        }
    }

    /// Parse format from string
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "wav" | "" => Some(ResembleOutputFormat::Wav),
            "mp3" => Some(ResembleOutputFormat::Mp3),
            _ => None,
        }
    }
}

impl fmt::Display for ResembleOutputFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =============================================================================
// Audio Precision
// =============================================================================

/// Resemble AI audio bit depth/precision
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ResemblePrecision {
    /// 32-bit PCM (default, highest quality)
    #[default]
    Pcm32,
    /// 24-bit PCM
    Pcm24,
    /// 16-bit PCM
    Pcm16,
    /// 8-bit MULAW (telephony)
    Mulaw,
}

impl ResemblePrecision {
    /// Returns the API string representation of the precision
    pub fn as_str(&self) -> &'static str {
        match self {
            ResemblePrecision::Pcm32 => "PCM_32",
            ResemblePrecision::Pcm24 => "PCM_24",
            ResemblePrecision::Pcm16 => "PCM_16",
            ResemblePrecision::Mulaw => "MULAW",
        }
    }

    /// Parse precision from string
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_uppercase().as_str() {
            "PCM_32" | "PCM32" | "" => Some(ResemblePrecision::Pcm32),
            "PCM_24" | "PCM24" => Some(ResemblePrecision::Pcm24),
            "PCM_16" | "PCM16" => Some(ResemblePrecision::Pcm16),
            "MULAW" | "ULAW" => Some(ResemblePrecision::Mulaw),
            _ => None,
        }
    }
}

impl fmt::Display for ResemblePrecision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =============================================================================
// Voice Information
// =============================================================================

/// Resemble AI voice information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResembleVoice {
    /// Unique voice identifier (UUID)
    pub uuid: String,
    /// Display name
    pub name: String,
    /// Voice status (ready, processing, etc.)
    pub status: String,
    /// Default language code
    #[serde(default)]
    pub default_language: Option<String>,
    /// Voice type (custom, pre-built)
    #[serde(default)]
    pub voice_type: Option<String>,
    /// Supported languages
    #[serde(default)]
    pub supported_languages: Vec<String>,
    /// API support flags
    #[serde(default)]
    pub api_support: Option<ResembleApiSupport>,
    /// Creation timestamp
    #[serde(default)]
    pub created_at: Option<String>,
    /// Last update timestamp
    #[serde(default)]
    pub updated_at: Option<String>,
}

/// API support flags for a voice
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResembleApiSupport {
    /// Supports sync API
    #[serde(default)]
    pub sync: bool,
    /// Supports async API
    #[serde(default, rename = "async")]
    pub async_api: bool,
    /// Supports direct synthesis
    #[serde(default)]
    pub direct_synthesis: bool,
    /// Supports streaming
    #[serde(default)]
    pub streaming: bool,
}

/// Response from voices list endpoint
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResembleVoicesResponse {
    /// Success flag
    pub success: bool,
    /// Current page
    pub page: u32,
    /// Total pages
    pub num_pages: u32,
    /// Page size
    pub page_size: u32,
    /// Voice items
    pub items: Vec<ResembleVoice>,
}

// =============================================================================
// TTS Request
// =============================================================================

/// Resemble AI TTS streaming request body
#[derive(Debug, Clone, Serialize)]
pub struct ResembleStreamRequest {
    /// Voice UUID
    pub voice_uuid: String,
    /// Text or SSML to synthesize
    pub data: String,
    /// Project UUID (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_uuid: Option<String>,
    /// Model selection
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Audio precision
    #[serde(skip_serializing_if = "Option::is_none")]
    pub precision: Option<String>,
    /// Sample rate in Hz
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,
    /// Enable HD synthesis
    #[serde(skip_serializing_if = "Option::is_none")]
    pub use_hd: Option<bool>,
}

impl ResembleStreamRequest {
    /// Create a new stream request
    pub fn new(voice_uuid: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            voice_uuid: voice_uuid.into(),
            data: text.into(),
            project_uuid: None,
            model: None,
            precision: None,
            sample_rate: None,
            use_hd: None,
        }
    }

    /// Create request from config
    pub fn from_config(config: &ResembleTtsConfig, text: &str) -> Self {
        Self {
            voice_uuid: config.voice_uuid.clone(),
            data: text.to_string(),
            project_uuid: config.project_uuid.clone(),
            model: if config.model == ResembleModel::Chatterbox {
                None // Default model, no need to specify
            } else {
                Some(config.model.as_str().to_string())
            },
            precision: if config.precision == ResemblePrecision::Pcm32 {
                None // Default precision
            } else {
                Some(config.precision.as_str().to_string())
            },
            sample_rate: if config.sample_rate == DEFAULT_SAMPLE_RATE {
                None
            } else {
                Some(config.sample_rate)
            },
            use_hd: if config.use_hd { Some(true) } else { None },
        }
    }

    /// Set the model
    pub fn with_model(mut self, model: ResembleModel) -> Self {
        self.model = if model == ResembleModel::Chatterbox {
            None
        } else {
            Some(model.as_str().to_string())
        };
        self
    }

    /// Set the precision
    pub fn with_precision(mut self, precision: ResemblePrecision) -> Self {
        self.precision = if precision == ResemblePrecision::Pcm32 {
            None
        } else {
            Some(precision.as_str().to_string())
        };
        self
    }

    /// Set the sample rate
    pub fn with_sample_rate(mut self, sample_rate: u32) -> Self {
        self.sample_rate = Some(sample_rate);
        self
    }

    /// Enable HD synthesis
    pub fn with_hd(mut self) -> Self {
        self.use_hd = Some(true);
        self
    }
}

// =============================================================================
// TTS Configuration
// =============================================================================

/// Resemble AI TTS configuration
///
/// # Example
///
/// ```rust,ignore
/// use waav_gateway::core::tts::resemble::{ResembleTtsConfig, ResembleModel};
///
/// let config = ResembleTtsConfig::new("your-api-key", "voice-uuid")
///     .with_model(ResembleModel::ChatterboxTurbo)
///     .with_hd(true);
/// ```
#[derive(Debug, Clone)]
pub struct ResembleTtsConfig {
    /// Resemble AI API key
    pub api_key: String,
    /// Voice UUID
    pub voice_uuid: String,
    /// Project UUID (optional)
    pub project_uuid: Option<String>,
    /// Model selection
    pub model: ResembleModel,
    /// Output format
    pub output_format: ResembleOutputFormat,
    /// Audio precision
    pub precision: ResemblePrecision,
    /// Sample rate in Hz
    pub sample_rate: u32,
    /// Enable HD synthesis
    pub use_hd: bool,
}

impl ResembleTtsConfig {
    /// Create a new configuration with API key and voice UUID
    pub fn new(api_key: impl Into<String>, voice_uuid: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            voice_uuid: voice_uuid.into(),
            project_uuid: None,
            model: ResembleModel::default(),
            output_format: ResembleOutputFormat::default(),
            precision: ResemblePrecision::default(),
            sample_rate: DEFAULT_SAMPLE_RATE,
            use_hd: false,
        }
    }

    /// Set the project UUID
    pub fn with_project(mut self, project_uuid: impl Into<String>) -> Self {
        self.project_uuid = Some(project_uuid.into());
        self
    }

    /// Set the model
    pub fn with_model(mut self, model: ResembleModel) -> Self {
        self.model = model;
        self
    }

    /// Set the output format
    pub fn with_output_format(mut self, format: ResembleOutputFormat) -> Self {
        self.output_format = format;
        self
    }

    /// Set the audio precision
    pub fn with_precision(mut self, precision: ResemblePrecision) -> Self {
        self.precision = precision;
        self
    }

    /// Set the sample rate
    pub fn with_sample_rate(mut self, sample_rate: u32) -> Self {
        self.sample_rate = sample_rate;
        self
    }

    /// Enable HD synthesis
    pub fn with_hd(mut self, use_hd: bool) -> Self {
        self.use_hd = use_hd;
        self
    }

    /// Create configuration from base TTSConfig
    ///
    /// Extracts Resemble-specific settings from the base TTS config.
    /// The voice_id field is used as voice_uuid.
    pub fn from_base(config: &TTSConfig) -> TTSResult<Self> {
        // Get API key from config or environment
        let api_key = if !config.api_key.is_empty() {
            config.api_key.clone()
        } else {
            std::env::var("RESEMBLE_API_KEY").map_err(|_| {
                TTSError::InvalidConfiguration(
                    "RESEMBLE_API_KEY environment variable not set and no api_key provided"
                        .to_string(),
                )
            })?
        };

        // Get voice UUID from voice_id
        let voice_uuid = config
            .voice_id
            .clone()
            .filter(|v| !v.is_empty())
            .ok_or_else(|| {
                TTSError::InvalidConfiguration(
                    "voice_id (voice_uuid) is required for Resemble AI".to_string(),
                )
            })?;

        // Parse model from config.model field
        let model = if !config.model.is_empty() {
            ResembleModel::from_str(&config.model).unwrap_or_default()
        } else {
            ResembleModel::default()
        };

        // Parse output format from audio_format
        let output_format = config
            .audio_format
            .as_ref()
            .and_then(|f| ResembleOutputFormat::from_str(f))
            .unwrap_or_default();

        // Parse sample rate
        let sample_rate = config.sample_rate.unwrap_or(DEFAULT_SAMPLE_RATE);

        let resemble_config = Self {
            api_key,
            voice_uuid,
            project_uuid: None,
            model,
            output_format,
            precision: ResemblePrecision::default(),
            sample_rate,
            use_hd: false,
        };

        // Validate
        resemble_config.validate()?;

        Ok(resemble_config)
    }

    /// Validate the configuration
    pub fn validate(&self) -> TTSResult<()> {
        if self.api_key.is_empty() {
            return Err(TTSError::InvalidConfiguration(
                "Resemble AI API key is required".to_string(),
            ));
        }

        if self.voice_uuid.is_empty() {
            return Err(TTSError::InvalidConfiguration(
                "Resemble AI voice_uuid is required".to_string(),
            ));
        }

        // Validate sample rate - Resemble supports these rates
        let valid_sample_rates = [8000, 16000, 22050, 24000, 32000, 44100, 48000];
        if !valid_sample_rates.contains(&self.sample_rate) {
            return Err(TTSError::InvalidConfiguration(format!(
                "Invalid sample rate {}. Must be one of: {:?}",
                self.sample_rate, valid_sample_rates
            )));
        }

        Ok(())
    }

    /// Validate text length for streaming
    pub fn validate_text(text: &str) -> TTSResult<()> {
        if text.len() > MAX_TEXT_LENGTH_STREAM {
            return Err(TTSError::InvalidConfiguration(format!(
                "Text exceeds maximum length of {} characters (got {})",
                MAX_TEXT_LENGTH_STREAM,
                text.len()
            )));
        }
        Ok(())
    }
}

impl Default for ResembleTtsConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            voice_uuid: String::new(),
            project_uuid: None,
            model: ResembleModel::default(),
            output_format: ResembleOutputFormat::default(),
            precision: ResemblePrecision::default(),
            sample_rate: DEFAULT_SAMPLE_RATE,
            use_hd: false,
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_enum() {
        assert_eq!(ResembleModel::Chatterbox.as_str(), "chatterbox");
        assert_eq!(ResembleModel::ChatterboxTurbo.as_str(), "chatterbox-turbo");
        assert_eq!(
            ResembleModel::ChatterboxMultilingual.as_str(),
            "chatterbox-multilingual"
        );
        assert_eq!(ResembleModel::default(), ResembleModel::Chatterbox);
    }

    #[test]
    fn test_model_from_str() {
        assert_eq!(
            ResembleModel::from_str("chatterbox"),
            Some(ResembleModel::Chatterbox)
        );
        assert_eq!(
            ResembleModel::from_str("chatterbox-turbo"),
            Some(ResembleModel::ChatterboxTurbo)
        );
        assert_eq!(
            ResembleModel::from_str("turbo"),
            Some(ResembleModel::ChatterboxTurbo)
        );
        assert_eq!(
            ResembleModel::from_str("multilingual"),
            Some(ResembleModel::ChatterboxMultilingual)
        );
        assert_eq!(ResembleModel::from_str(""), Some(ResembleModel::Chatterbox));
        assert_eq!(ResembleModel::from_str("invalid"), None);
    }

    #[test]
    fn test_model_features() {
        assert!(!ResembleModel::Chatterbox.supports_paralinguistic_tags());
        assert!(ResembleModel::ChatterboxTurbo.supports_paralinguistic_tags());
        assert!(!ResembleModel::Chatterbox.is_multilingual());
        assert!(ResembleModel::ChatterboxMultilingual.is_multilingual());
    }

    #[test]
    fn test_output_format_enum() {
        assert_eq!(ResembleOutputFormat::Wav.as_str(), "wav");
        assert_eq!(ResembleOutputFormat::Mp3.as_str(), "mp3");
        assert_eq!(ResembleOutputFormat::default(), ResembleOutputFormat::Wav);
    }

    #[test]
    fn test_output_format_from_str() {
        assert_eq!(
            ResembleOutputFormat::from_str("wav"),
            Some(ResembleOutputFormat::Wav)
        );
        assert_eq!(
            ResembleOutputFormat::from_str("mp3"),
            Some(ResembleOutputFormat::Mp3)
        );
        assert_eq!(
            ResembleOutputFormat::from_str(""),
            Some(ResembleOutputFormat::Wav)
        );
        assert_eq!(ResembleOutputFormat::from_str("invalid"), None);
    }

    #[test]
    fn test_precision_enum() {
        assert_eq!(ResemblePrecision::Pcm32.as_str(), "PCM_32");
        assert_eq!(ResemblePrecision::Pcm24.as_str(), "PCM_24");
        assert_eq!(ResemblePrecision::Pcm16.as_str(), "PCM_16");
        assert_eq!(ResemblePrecision::Mulaw.as_str(), "MULAW");
        assert_eq!(ResemblePrecision::default(), ResemblePrecision::Pcm32);
    }

    #[test]
    fn test_precision_from_str() {
        assert_eq!(
            ResemblePrecision::from_str("PCM_32"),
            Some(ResemblePrecision::Pcm32)
        );
        assert_eq!(
            ResemblePrecision::from_str("PCM_16"),
            Some(ResemblePrecision::Pcm16)
        );
        assert_eq!(
            ResemblePrecision::from_str("MULAW"),
            Some(ResemblePrecision::Mulaw)
        );
        assert_eq!(
            ResemblePrecision::from_str(""),
            Some(ResemblePrecision::Pcm32)
        );
    }

    #[test]
    fn test_config_defaults() {
        let config = ResembleTtsConfig::new("test-key", "voice-uuid");
        assert_eq!(config.api_key, "test-key");
        assert_eq!(config.voice_uuid, "voice-uuid");
        assert_eq!(config.model, ResembleModel::Chatterbox);
        assert_eq!(config.output_format, ResembleOutputFormat::Wav);
        assert_eq!(config.precision, ResemblePrecision::Pcm32);
        assert_eq!(config.sample_rate, DEFAULT_SAMPLE_RATE);
        assert!(!config.use_hd);
    }

    #[test]
    fn test_config_builder() {
        let config = ResembleTtsConfig::new("test-key", "voice-uuid")
            .with_model(ResembleModel::ChatterboxTurbo)
            .with_output_format(ResembleOutputFormat::Mp3)
            .with_precision(ResemblePrecision::Pcm16)
            .with_sample_rate(44100)
            .with_hd(true)
            .with_project("project-uuid");

        assert_eq!(config.model, ResembleModel::ChatterboxTurbo);
        assert_eq!(config.output_format, ResembleOutputFormat::Mp3);
        assert_eq!(config.precision, ResemblePrecision::Pcm16);
        assert_eq!(config.sample_rate, 44100);
        assert!(config.use_hd);
        assert_eq!(config.project_uuid, Some("project-uuid".to_string()));
    }

    #[test]
    fn test_config_validation() {
        // Valid config
        let config = ResembleTtsConfig::new("test-key", "voice-uuid");
        assert!(config.validate().is_ok());

        // Empty API key
        let config = ResembleTtsConfig::new("", "voice-uuid");
        assert!(config.validate().is_err());

        // Empty voice UUID
        let config = ResembleTtsConfig::new("test-key", "");
        assert!(config.validate().is_err());

        // Invalid sample rate
        let config = ResembleTtsConfig::new("test-key", "voice-uuid").with_sample_rate(12345);
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_from_base() {
        let base = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("voice-uuid".to_string()),
            model: "chatterbox-turbo".to_string(),
            audio_format: Some("mp3".to_string()),
            sample_rate: Some(44100),
            ..Default::default()
        };

        let config = ResembleTtsConfig::from_base(&base).unwrap();
        assert_eq!(config.api_key, "test-key");
        assert_eq!(config.voice_uuid, "voice-uuid");
        assert_eq!(config.model, ResembleModel::ChatterboxTurbo);
        assert_eq!(config.output_format, ResembleOutputFormat::Mp3);
        assert_eq!(config.sample_rate, 44100);
    }

    #[test]
    fn test_config_from_base_requires_voice_id() {
        let base = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: None,
            ..Default::default()
        };

        let result = ResembleTtsConfig::from_base(&base);
        assert!(result.is_err());
        if let Err(TTSError::InvalidConfiguration(msg)) = result {
            assert!(msg.contains("voice_uuid"));
        }
    }

    #[test]
    fn test_text_validation() {
        // Valid text
        assert!(ResembleTtsConfig::validate_text("Hello, world!").is_ok());

        // Too long
        let long_text = "a".repeat(MAX_TEXT_LENGTH_STREAM + 1);
        assert!(ResembleTtsConfig::validate_text(&long_text).is_err());

        // At limit
        let limit_text = "a".repeat(MAX_TEXT_LENGTH_STREAM);
        assert!(ResembleTtsConfig::validate_text(&limit_text).is_ok());
    }

    #[test]
    fn test_stream_request_serialization() {
        let request = ResembleStreamRequest::new("voice-uuid", "Hello world");
        let json = serde_json::to_string(&request).unwrap();

        assert!(json.contains("\"voice_uuid\":\"voice-uuid\""));
        assert!(json.contains("\"data\":\"Hello world\""));
        // Optional fields should not be present when None
        assert!(!json.contains("\"model\""));
        assert!(!json.contains("\"precision\""));
    }

    #[test]
    fn test_stream_request_with_options() {
        let request = ResembleStreamRequest::new("voice-uuid", "Hello")
            .with_model(ResembleModel::ChatterboxTurbo)
            .with_precision(ResemblePrecision::Pcm16)
            .with_sample_rate(44100)
            .with_hd();

        let json = serde_json::to_string(&request).unwrap();

        assert!(json.contains("\"model\":\"chatterbox-turbo\""));
        assert!(json.contains("\"precision\":\"PCM_16\""));
        assert!(json.contains("\"sample_rate\":44100"));
        assert!(json.contains("\"use_hd\":true"));
    }

    #[test]
    fn test_stream_request_from_config() {
        let config = ResembleTtsConfig::new("test-key", "voice-uuid")
            .with_model(ResembleModel::ChatterboxTurbo)
            .with_precision(ResemblePrecision::Pcm16)
            .with_hd(true);

        let request = ResembleStreamRequest::from_config(&config, "Test text");

        assert_eq!(request.voice_uuid, "voice-uuid");
        assert_eq!(request.data, "Test text");
        assert_eq!(request.model, Some("chatterbox-turbo".to_string()));
        assert_eq!(request.precision, Some("PCM_16".to_string()));
        assert_eq!(request.use_hd, Some(true));
    }

    #[test]
    fn test_voice_deserialization() {
        let json = r#"{
            "uuid": "abc123",
            "name": "Test Voice",
            "status": "ready",
            "default_language": "en-US",
            "voice_type": "custom",
            "supported_languages": ["en-US", "es-ES"],
            "api_support": {
                "sync": true,
                "async": true,
                "direct_synthesis": true,
                "streaming": true
            },
            "created_at": "2025-01-01T00:00:00Z",
            "updated_at": "2025-01-01T00:00:00Z"
        }"#;

        let voice: ResembleVoice = serde_json::from_str(json).unwrap();
        assert_eq!(voice.uuid, "abc123");
        assert_eq!(voice.name, "Test Voice");
        assert_eq!(voice.status, "ready");
        assert_eq!(voice.default_language, Some("en-US".to_string()));
        assert_eq!(voice.supported_languages.len(), 2);
        assert!(voice.api_support.as_ref().unwrap().sync);
        assert!(voice.api_support.as_ref().unwrap().streaming);
    }

    #[test]
    fn test_voices_response_deserialization() {
        let json = r#"{
            "success": true,
            "page": 1,
            "num_pages": 5,
            "page_size": 10,
            "items": []
        }"#;

        let response: ResembleVoicesResponse = serde_json::from_str(json).unwrap();
        assert!(response.success);
        assert_eq!(response.page, 1);
        assert_eq!(response.num_pages, 5);
        assert_eq!(response.page_size, 10);
        assert!(response.items.is_empty());
    }
}
