//! Gladia STT Configuration
//!
//! Configuration types for Gladia speech-to-text provider.
//! Gladia provides real-time STT with 100+ languages and <300ms latency.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

use crate::core::stt::base::{STTConfig, STTError};

// =============================================================================
// Region
// =============================================================================

/// Gladia API region
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GladiaRegion {
    /// European Union West region (default)
    #[default]
    EuWest,
    /// United States West region
    UsWest,
}

impl GladiaRegion {
    /// Get the region value for API query parameter
    pub fn as_str(&self) -> &'static str {
        match self {
            GladiaRegion::EuWest => "eu-west",
            GladiaRegion::UsWest => "us-west",
        }
    }
}

impl fmt::Display for GladiaRegion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for GladiaRegion {
    type Err = STTError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().replace('_', "-").as_str() {
            "eu-west" | "eu" | "europe" | "european" => Ok(GladiaRegion::EuWest),
            "us-west" | "us" | "usa" | "united-states" => Ok(GladiaRegion::UsWest),
            _ => Err(STTError::ConfigurationError(format!(
                "Invalid Gladia region: '{}'. Valid options: eu-west, us-west",
                s
            ))),
        }
    }
}

// =============================================================================
// Audio Encoding
// =============================================================================

/// Gladia audio encoding format
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum GladiaEncoding {
    /// PCM WAV encoding (default)
    #[default]
    #[serde(rename = "wav/pcm")]
    WavPcm,
    /// A-law encoding
    #[serde(rename = "wav/alaw")]
    WavAlaw,
    /// Mu-law encoding
    #[serde(rename = "wav/ulaw")]
    WavUlaw,
}

impl GladiaEncoding {
    /// Get the encoding string for API requests
    pub fn as_str(&self) -> &'static str {
        match self {
            GladiaEncoding::WavPcm => "wav/pcm",
            GladiaEncoding::WavAlaw => "wav/alaw",
            GladiaEncoding::WavUlaw => "wav/ulaw",
        }
    }
}

impl fmt::Display for GladiaEncoding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for GladiaEncoding {
    type Err = STTError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "wav/pcm" | "pcm" | "linear16" | "pcm_s16le" | "s16le" => Ok(GladiaEncoding::WavPcm),
            "wav/alaw" | "alaw" | "a-law" => Ok(GladiaEncoding::WavAlaw),
            "wav/ulaw" | "ulaw" | "u-law" | "mulaw" | "mu-law" => Ok(GladiaEncoding::WavUlaw),
            _ => Err(STTError::ConfigurationError(format!(
                "Invalid encoding: '{}'. Valid options: wav/pcm, wav/alaw, wav/ulaw",
                s
            ))),
        }
    }
}

// =============================================================================
// Bit Depth
// =============================================================================

/// Gladia audio bit depth
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum GladiaBitDepth {
    /// 8-bit
    #[serde(rename = "8")]
    Bit8,
    /// 16-bit (default)
    #[default]
    #[serde(rename = "16")]
    Bit16,
    /// 24-bit
    #[serde(rename = "24")]
    Bit24,
    /// 32-bit
    #[serde(rename = "32")]
    Bit32,
}

impl GladiaBitDepth {
    /// Get the bit depth value
    pub fn value(&self) -> u8 {
        match self {
            GladiaBitDepth::Bit8 => 8,
            GladiaBitDepth::Bit16 => 16,
            GladiaBitDepth::Bit24 => 24,
            GladiaBitDepth::Bit32 => 32,
        }
    }
}

impl fmt::Display for GladiaBitDepth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.value())
    }
}

impl FromStr for GladiaBitDepth {
    type Err = STTError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "8" => Ok(GladiaBitDepth::Bit8),
            "16" => Ok(GladiaBitDepth::Bit16),
            "24" => Ok(GladiaBitDepth::Bit24),
            "32" => Ok(GladiaBitDepth::Bit32),
            _ => Err(STTError::ConfigurationError(format!(
                "Invalid bit depth: '{}'. Valid options: 8, 16, 24, 32",
                s
            ))),
        }
    }
}

impl From<u8> for GladiaBitDepth {
    fn from(value: u8) -> Self {
        match value {
            8 => GladiaBitDepth::Bit8,
            24 => GladiaBitDepth::Bit24,
            32 => GladiaBitDepth::Bit32,
            _ => GladiaBitDepth::Bit16, // Default to 16-bit
        }
    }
}

// =============================================================================
// Language Configuration
// =============================================================================

/// Language configuration for Gladia STT
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GladiaLanguageConfig {
    /// List of expected languages (ISO 639-1 codes)
    /// Empty array enables automatic language detection
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub languages: Vec<String>,
    /// Enable code-switching (automatic language switching)
    #[serde(default)]
    pub code_switching: bool,
}

impl GladiaLanguageConfig {
    /// Create a new language config with a single language
    pub fn new(language: impl Into<String>) -> Self {
        Self {
            languages: vec![language.into()],
            code_switching: false,
        }
    }

    /// Create a config with automatic language detection
    pub fn auto() -> Self {
        Self {
            languages: Vec::new(),
            code_switching: false,
        }
    }

    /// Enable code-switching for multilingual conversations
    pub fn with_code_switching(mut self) -> Self {
        self.code_switching = true;
        self
    }

    /// Add a language to the expected languages list
    pub fn add_language(mut self, language: impl Into<String>) -> Self {
        self.languages.push(language.into());
        self
    }
}

// =============================================================================
// Messages Configuration
// =============================================================================

/// Configuration for which WebSocket messages to receive
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GladiaMessagesConfig {
    /// Receive partial (interim) transcripts
    #[serde(default = "default_true")]
    pub receive_partial_transcripts: bool,
    /// Receive final transcripts
    #[serde(default = "default_true")]
    pub receive_final_transcripts: bool,
    /// Receive speech start/end events
    #[serde(default)]
    pub receive_speech_events: bool,
    /// Receive pre-processing events
    #[serde(default)]
    pub receive_pre_processing_events: bool,
    /// Receive realtime processing events
    #[serde(default)]
    pub receive_realtime_processing_events: bool,
    /// Receive post-processing events
    #[serde(default)]
    pub receive_post_processing_events: bool,
}

fn default_true() -> bool {
    true
}

impl Default for GladiaMessagesConfig {
    fn default() -> Self {
        Self {
            receive_partial_transcripts: true,
            receive_final_transcripts: true,
            receive_speech_events: false,
            receive_pre_processing_events: false,
            receive_realtime_processing_events: false,
            receive_post_processing_events: false,
        }
    }
}

// =============================================================================
// Pre-processing Configuration
// =============================================================================

/// Pre-processing configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GladiaPreProcessing {
    /// Enable audio enhancement
    #[serde(default)]
    pub audio_enhancer: bool,
    /// Speech detection threshold (0.0-1.0)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speech_threshold: Option<f32>,
}

// =============================================================================
// Realtime Processing Configuration
// =============================================================================

/// Realtime processing configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GladiaRealtimeProcessing {
    /// Enable word-level accurate timestamps
    #[serde(default)]
    pub words_accurate_timestamps: bool,
    /// Custom vocabulary words
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_vocabulary: Vec<String>,
    /// Enable real-time translation
    #[serde(default)]
    pub translation: bool,
    /// Target languages for translation
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub translation_target_languages: Vec<String>,
    /// Enable named entity recognition
    #[serde(default)]
    pub named_entity_recognition: bool,
    /// Enable sentiment analysis
    #[serde(default)]
    pub sentiment_analysis: bool,
}

// =============================================================================
// STT Configuration
// =============================================================================

/// Configuration for Gladia STT provider
#[derive(Debug, Clone)]
pub struct GladiaSTTConfig {
    /// API key for authentication
    pub api_key: String,
    /// API region (eu-west or us-west)
    pub region: GladiaRegion,
    /// Audio encoding format
    pub encoding: GladiaEncoding,
    /// Audio bit depth
    pub bit_depth: GladiaBitDepth,
    /// Sample rate in Hz
    pub sample_rate: u32,
    /// Number of audio channels (1-8)
    pub channels: u8,
    /// Speech recognition model (currently only "solaria-1")
    pub model: String,
    /// Endpointing duration in seconds (0.01-10)
    pub endpointing: f32,
    /// Maximum duration without endpointing (5-60 seconds)
    pub maximum_duration_without_endpointing: f32,
    /// Language configuration
    pub language_config: GladiaLanguageConfig,
    /// Messages configuration
    pub messages_config: GladiaMessagesConfig,
    /// Pre-processing configuration
    pub pre_processing: GladiaPreProcessing,
    /// Realtime processing configuration
    pub realtime_processing: GladiaRealtimeProcessing,
    /// Custom metadata for session tracking
    pub custom_metadata: Option<serde_json::Value>,
}

impl Default for GladiaSTTConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            region: GladiaRegion::EuWest,
            encoding: GladiaEncoding::WavPcm,
            bit_depth: GladiaBitDepth::Bit16,
            sample_rate: 16000,
            channels: 1,
            model: "solaria-1".to_string(),
            endpointing: super::DEFAULT_ENDPOINTING,
            maximum_duration_without_endpointing: super::DEFAULT_MAX_DURATION_WITHOUT_ENDPOINTING,
            language_config: GladiaLanguageConfig::new("en"),
            messages_config: GladiaMessagesConfig::default(),
            pre_processing: GladiaPreProcessing::default(),
            realtime_processing: GladiaRealtimeProcessing::default(),
            custom_metadata: None,
        }
    }
}

impl GladiaSTTConfig {
    /// Create a new configuration with the given API key
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            ..Default::default()
        }
    }

    /// Create configuration from base STTConfig
    pub fn from_base(config: &STTConfig) -> Result<Self, STTError> {
        // API key is required
        let api_key = if !config.api_key.is_empty() {
            config.api_key.clone()
        } else {
            std::env::var("GLADIA_API_KEY").map_err(|_| {
                STTError::ConfigurationError(
                    "Gladia API key required. Set api_key or GLADIA_API_KEY env var".to_string(),
                )
            })?
        };

        // Parse encoding
        let encoding = config.encoding.parse().unwrap_or(GladiaEncoding::WavPcm);

        // Parse language
        let language = if config.language.is_empty() {
            "en".to_string()
        } else {
            // Extract base language code (e.g., "en-US" -> "en")
            config
                .language
                .split('-')
                .next()
                .unwrap_or("en")
                .to_string()
        };

        Ok(Self {
            api_key,
            encoding,
            sample_rate: config.sample_rate,
            language_config: GladiaLanguageConfig::new(language),
            ..Default::default()
        })
    }

    /// Validate the configuration
    pub fn validate(&self) -> Result<(), STTError> {
        if self.api_key.is_empty() {
            return Err(STTError::ConfigurationError(
                "API key is required".to_string(),
            ));
        }

        // Validate sample rate
        if ![8000, 16000, 32000, 44100, 48000].contains(&self.sample_rate) {
            return Err(STTError::ConfigurationError(format!(
                "Invalid sample rate: {}. Valid options: 8000, 16000, 32000, 44100, 48000",
                self.sample_rate
            )));
        }

        // Validate channels
        if self.channels < 1 || self.channels > 8 {
            return Err(STTError::ConfigurationError(format!(
                "Channels must be between 1 and 8, got {}",
                self.channels
            )));
        }

        // Validate endpointing
        if self.endpointing < 0.01 || self.endpointing > 10.0 {
            return Err(STTError::ConfigurationError(format!(
                "Endpointing must be between 0.01 and 10.0 seconds, got {}",
                self.endpointing
            )));
        }

        // Validate maximum duration without endpointing
        if self.maximum_duration_without_endpointing < 5.0
            || self.maximum_duration_without_endpointing > 60.0
        {
            return Err(STTError::ConfigurationError(format!(
                "Maximum duration without endpointing must be between 5 and 60 seconds, got {}",
                self.maximum_duration_without_endpointing
            )));
        }

        // Validate pre-processing speech threshold
        if let Some(threshold) = self.pre_processing.speech_threshold {
            if !(0.0..=1.0).contains(&threshold) {
                return Err(STTError::ConfigurationError(format!(
                    "Speech threshold must be between 0.0 and 1.0, got {}",
                    threshold
                )));
            }
        }

        Ok(())
    }

    /// Get the API URL for session initialization
    pub fn api_url(&self) -> String {
        format!("{}?region={}", super::GLADIA_LIVE_URL, self.region)
    }

    /// Set the region
    pub fn with_region(mut self, region: GladiaRegion) -> Self {
        self.region = region;
        self
    }

    /// Set the encoding
    pub fn with_encoding(mut self, encoding: GladiaEncoding) -> Self {
        self.encoding = encoding;
        self
    }

    /// Set the bit depth
    pub fn with_bit_depth(mut self, bit_depth: GladiaBitDepth) -> Self {
        self.bit_depth = bit_depth;
        self
    }

    /// Set the sample rate
    pub fn with_sample_rate(mut self, sample_rate: u32) -> Self {
        self.sample_rate = sample_rate;
        self
    }

    /// Set the number of channels
    pub fn with_channels(mut self, channels: u8) -> Self {
        self.channels = channels.clamp(1, 8);
        self
    }

    /// Set the language
    pub fn with_language(mut self, language: impl Into<String>) -> Self {
        self.language_config = GladiaLanguageConfig::new(language);
        self
    }

    /// Enable code-switching for multilingual conversations
    pub fn with_code_switching(mut self, enable: bool) -> Self {
        self.language_config.code_switching = enable;
        self
    }

    /// Set endpointing duration
    pub fn with_endpointing(mut self, seconds: f32) -> Self {
        self.endpointing = seconds.clamp(0.01, 10.0);
        self
    }

    /// Enable audio enhancement
    pub fn with_audio_enhancer(mut self, enable: bool) -> Self {
        self.pre_processing.audio_enhancer = enable;
        self
    }

    /// Enable word-level timestamps
    pub fn with_word_timestamps(mut self, enable: bool) -> Self {
        self.realtime_processing.words_accurate_timestamps = enable;
        self
    }

    /// Set custom vocabulary
    pub fn with_custom_vocabulary(mut self, words: Vec<String>) -> Self {
        self.realtime_processing.custom_vocabulary = words;
        self
    }

    /// Enable translation to target languages
    pub fn with_translation(mut self, target_languages: Vec<String>) -> Self {
        self.realtime_processing.translation = !target_languages.is_empty();
        self.realtime_processing.translation_target_languages = target_languages;
        self
    }

    /// Configure which messages to receive
    pub fn with_partials(mut self, enable: bool) -> Self {
        self.messages_config.receive_partial_transcripts = enable;
        self
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // Region tests
    #[test]
    fn test_region_default() {
        assert_eq!(GladiaRegion::default(), GladiaRegion::EuWest);
    }

    #[test]
    fn test_region_as_str() {
        assert_eq!(GladiaRegion::EuWest.as_str(), "eu-west");
        assert_eq!(GladiaRegion::UsWest.as_str(), "us-west");
    }

    #[test]
    fn test_region_from_str() {
        assert_eq!(
            "eu-west".parse::<GladiaRegion>().unwrap(),
            GladiaRegion::EuWest
        );
        assert_eq!(
            "us-west".parse::<GladiaRegion>().unwrap(),
            GladiaRegion::UsWest
        );
        assert_eq!("eu".parse::<GladiaRegion>().unwrap(), GladiaRegion::EuWest);
        assert_eq!("us".parse::<GladiaRegion>().unwrap(), GladiaRegion::UsWest);
        assert!("invalid".parse::<GladiaRegion>().is_err());
    }

    // Encoding tests
    #[test]
    fn test_encoding_default() {
        assert_eq!(GladiaEncoding::default(), GladiaEncoding::WavPcm);
    }

    #[test]
    fn test_encoding_as_str() {
        assert_eq!(GladiaEncoding::WavPcm.as_str(), "wav/pcm");
        assert_eq!(GladiaEncoding::WavAlaw.as_str(), "wav/alaw");
        assert_eq!(GladiaEncoding::WavUlaw.as_str(), "wav/ulaw");
    }

    #[test]
    fn test_encoding_from_str() {
        assert_eq!(
            "wav/pcm".parse::<GladiaEncoding>().unwrap(),
            GladiaEncoding::WavPcm
        );
        assert_eq!(
            "pcm".parse::<GladiaEncoding>().unwrap(),
            GladiaEncoding::WavPcm
        );
        assert_eq!(
            "linear16".parse::<GladiaEncoding>().unwrap(),
            GladiaEncoding::WavPcm
        );
        assert_eq!(
            "wav/alaw".parse::<GladiaEncoding>().unwrap(),
            GladiaEncoding::WavAlaw
        );
        assert_eq!(
            "wav/ulaw".parse::<GladiaEncoding>().unwrap(),
            GladiaEncoding::WavUlaw
        );
        assert_eq!(
            "mulaw".parse::<GladiaEncoding>().unwrap(),
            GladiaEncoding::WavUlaw
        );
        assert!("invalid".parse::<GladiaEncoding>().is_err());
    }

    // Bit depth tests
    #[test]
    fn test_bit_depth_default() {
        assert_eq!(GladiaBitDepth::default(), GladiaBitDepth::Bit16);
    }

    #[test]
    fn test_bit_depth_value() {
        assert_eq!(GladiaBitDepth::Bit8.value(), 8);
        assert_eq!(GladiaBitDepth::Bit16.value(), 16);
        assert_eq!(GladiaBitDepth::Bit24.value(), 24);
        assert_eq!(GladiaBitDepth::Bit32.value(), 32);
    }

    #[test]
    fn test_bit_depth_from_str() {
        assert_eq!("8".parse::<GladiaBitDepth>().unwrap(), GladiaBitDepth::Bit8);
        assert_eq!(
            "16".parse::<GladiaBitDepth>().unwrap(),
            GladiaBitDepth::Bit16
        );
        assert_eq!(
            "24".parse::<GladiaBitDepth>().unwrap(),
            GladiaBitDepth::Bit24
        );
        assert_eq!(
            "32".parse::<GladiaBitDepth>().unwrap(),
            GladiaBitDepth::Bit32
        );
        assert!("invalid".parse::<GladiaBitDepth>().is_err());
    }

    #[test]
    fn test_bit_depth_from_u8() {
        assert_eq!(GladiaBitDepth::from(8), GladiaBitDepth::Bit8);
        assert_eq!(GladiaBitDepth::from(16), GladiaBitDepth::Bit16);
        assert_eq!(GladiaBitDepth::from(24), GladiaBitDepth::Bit24);
        assert_eq!(GladiaBitDepth::from(32), GladiaBitDepth::Bit32);
        assert_eq!(GladiaBitDepth::from(99), GladiaBitDepth::Bit16); // Default
    }

    // Language config tests
    #[test]
    fn test_language_config_new() {
        let config = GladiaLanguageConfig::new("en");
        assert_eq!(config.languages, vec!["en"]);
        assert!(!config.code_switching);
    }

    #[test]
    fn test_language_config_auto() {
        let config = GladiaLanguageConfig::auto();
        assert!(config.languages.is_empty());
        assert!(!config.code_switching);
    }

    #[test]
    fn test_language_config_with_code_switching() {
        let config = GladiaLanguageConfig::new("en").with_code_switching();
        assert!(config.code_switching);
    }

    #[test]
    fn test_language_config_add_language() {
        let config = GladiaLanguageConfig::new("en")
            .add_language("es")
            .add_language("fr");
        assert_eq!(config.languages, vec!["en", "es", "fr"]);
    }

    // Messages config tests
    #[test]
    fn test_messages_config_default() {
        let config = GladiaMessagesConfig::default();
        assert!(config.receive_partial_transcripts);
        assert!(config.receive_final_transcripts);
        assert!(!config.receive_speech_events);
        assert!(!config.receive_pre_processing_events);
        assert!(!config.receive_realtime_processing_events);
        assert!(!config.receive_post_processing_events);
    }

    // STT Config tests
    #[test]
    fn test_config_default() {
        let config = GladiaSTTConfig::default();
        assert!(config.api_key.is_empty());
        assert_eq!(config.region, GladiaRegion::EuWest);
        assert_eq!(config.encoding, GladiaEncoding::WavPcm);
        assert_eq!(config.bit_depth, GladiaBitDepth::Bit16);
        assert_eq!(config.sample_rate, 16000);
        assert_eq!(config.channels, 1);
        assert_eq!(config.model, "solaria-1");
        assert_eq!(config.endpointing, 0.05);
        assert_eq!(config.maximum_duration_without_endpointing, 5.0);
    }

    #[test]
    fn test_config_new() {
        let config = GladiaSTTConfig::new("test-api-key");
        assert_eq!(config.api_key, "test-api-key");
        assert_eq!(config.region, GladiaRegion::EuWest);
    }

    #[test]
    fn test_config_validate_valid() {
        let config = GladiaSTTConfig::new("test-api-key");
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validate_empty_api_key() {
        let config = GladiaSTTConfig::default();
        let err = config.validate().unwrap_err();
        assert!(matches!(err, STTError::ConfigurationError(_)));
    }

    #[test]
    fn test_config_validate_invalid_sample_rate() {
        let mut config = GladiaSTTConfig::new("test-api-key");
        config.sample_rate = 12000;
        let err = config.validate().unwrap_err();
        assert!(matches!(err, STTError::ConfigurationError(_)));
    }

    #[test]
    fn test_config_validate_invalid_channels() {
        let mut config = GladiaSTTConfig::new("test-api-key");
        config.channels = 0;
        let err = config.validate().unwrap_err();
        assert!(matches!(err, STTError::ConfigurationError(_)));

        config.channels = 10;
        let err = config.validate().unwrap_err();
        assert!(matches!(err, STTError::ConfigurationError(_)));
    }

    #[test]
    fn test_config_validate_invalid_endpointing() {
        let mut config = GladiaSTTConfig::new("test-api-key");
        config.endpointing = 0.001;
        let err = config.validate().unwrap_err();
        assert!(matches!(err, STTError::ConfigurationError(_)));

        config.endpointing = 15.0;
        let err = config.validate().unwrap_err();
        assert!(matches!(err, STTError::ConfigurationError(_)));
    }

    #[test]
    fn test_config_validate_invalid_max_duration() {
        let mut config = GladiaSTTConfig::new("test-api-key");
        config.maximum_duration_without_endpointing = 2.0;
        let err = config.validate().unwrap_err();
        assert!(matches!(err, STTError::ConfigurationError(_)));

        config.maximum_duration_without_endpointing = 100.0;
        let err = config.validate().unwrap_err();
        assert!(matches!(err, STTError::ConfigurationError(_)));
    }

    #[test]
    fn test_config_validate_invalid_speech_threshold() {
        let mut config = GladiaSTTConfig::new("test-api-key");
        config.pre_processing.speech_threshold = Some(-0.5);
        let err = config.validate().unwrap_err();
        assert!(matches!(err, STTError::ConfigurationError(_)));

        config.pre_processing.speech_threshold = Some(1.5);
        let err = config.validate().unwrap_err();
        assert!(matches!(err, STTError::ConfigurationError(_)));
    }

    #[test]
    fn test_config_builder_methods() {
        let config = GladiaSTTConfig::new("test-api-key")
            .with_region(GladiaRegion::UsWest)
            .with_encoding(GladiaEncoding::WavUlaw)
            .with_bit_depth(GladiaBitDepth::Bit8)
            .with_sample_rate(8000)
            .with_channels(2)
            .with_language("es")
            .with_code_switching(true)
            .with_endpointing(0.1)
            .with_audio_enhancer(true)
            .with_word_timestamps(true)
            .with_custom_vocabulary(vec!["custom".to_string()])
            .with_translation(vec!["fr".to_string()])
            .with_partials(false);

        assert_eq!(config.region, GladiaRegion::UsWest);
        assert_eq!(config.encoding, GladiaEncoding::WavUlaw);
        assert_eq!(config.bit_depth, GladiaBitDepth::Bit8);
        assert_eq!(config.sample_rate, 8000);
        assert_eq!(config.channels, 2);
        assert_eq!(config.language_config.languages, vec!["es"]);
        assert!(config.language_config.code_switching);
        assert_eq!(config.endpointing, 0.1);
        assert!(config.pre_processing.audio_enhancer);
        assert!(config.realtime_processing.words_accurate_timestamps);
        assert_eq!(config.realtime_processing.custom_vocabulary, vec!["custom"]);
        assert!(config.realtime_processing.translation);
        assert_eq!(
            config.realtime_processing.translation_target_languages,
            vec!["fr"]
        );
        assert!(!config.messages_config.receive_partial_transcripts);
    }

    #[test]
    fn test_config_api_url() {
        let config = GladiaSTTConfig::new("test-api-key");
        assert_eq!(
            config.api_url(),
            "https://api.gladia.io/v2/live?region=eu-west"
        );

        let config = config.with_region(GladiaRegion::UsWest);
        assert_eq!(
            config.api_url(),
            "https://api.gladia.io/v2/live?region=us-west"
        );
    }

    #[test]
    fn test_config_from_base() {
        let base_config = STTConfig {
            api_key: "test-key".to_string(),
            language: "fr-FR".to_string(),
            sample_rate: 48000,
            encoding: "pcm".to_string(),
            ..Default::default()
        };

        let config = GladiaSTTConfig::from_base(&base_config).unwrap();
        assert_eq!(config.api_key, "test-key");
        assert_eq!(config.language_config.languages, vec!["fr"]);
        assert_eq!(config.sample_rate, 48000);
        assert_eq!(config.encoding, GladiaEncoding::WavPcm);
    }

    #[test]
    fn test_config_from_base_empty_api_key() {
        let base_config = STTConfig {
            api_key: String::new(),
            ..Default::default()
        };

        // Should fail without env var
        let result = GladiaSTTConfig::from_base(&base_config);
        assert!(result.is_err());
    }

    #[test]
    fn test_config_channels_clamped() {
        let config = GladiaSTTConfig::new("test-api-key").with_channels(20);
        assert_eq!(config.channels, 8); // Clamped to max

        let config = GladiaSTTConfig::new("test-api-key").with_channels(0);
        assert_eq!(config.channels, 1); // Clamped to min
    }

    #[test]
    fn test_config_endpointing_clamped() {
        let config = GladiaSTTConfig::new("test-api-key").with_endpointing(100.0);
        assert_eq!(config.endpointing, 10.0); // Clamped to max

        let config = GladiaSTTConfig::new("test-api-key").with_endpointing(0.001);
        assert_eq!(config.endpointing, 0.01); // Clamped to min
    }
}
