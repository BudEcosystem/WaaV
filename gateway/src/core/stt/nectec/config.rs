//! Configuration types for NECTEC AI for Thai STT API (Partii).
//!
//! This module provides configuration types for Thailand's NECTEC
//! Speech-to-Text service (Partii4 and Partii5).
//!
//! # Overview
//!
//! NECTEC (National Electronics and Computer Technology Center) provides
//! AI for Thai, a government-backed platform with free Thai language
//! processing services including:
//!
//! - Partii4: Legacy STT engine
//! - Partii5: Newer STT engine (recommended)
//!
//! # Audio Requirements
//!
//! - Format: WAV only
//! - Sample Rate: 16,000 Hz
//! - Bit Depth: 16-bit Linear PCM
//! - Channels: Mono
//! - Max Duration: 30 seconds
//! - Max File Size: 1 MB

use crate::core::stt::base::{STTConfig, STTError};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

// =============================================================================
// Constants
// =============================================================================

/// NECTEC Partii5 STT API endpoint (recommended).
pub const PARTII5_ENDPOINT: &str = "https://api.aiforthai.in.th/partii5-poc";

/// NECTEC Partii4 STT API endpoint (legacy).
pub const PARTII4_ENDPOINT: &str = "https://api.aiforthai.in.th/partii-webapi";

/// Default request timeout in seconds.
pub const DEFAULT_REQUEST_TIMEOUT: u64 = 60;

/// Minimum audio duration in milliseconds.
pub const MIN_AUDIO_DURATION_MS: u64 = 100;

/// Maximum audio duration in milliseconds (30 seconds).
pub const MAX_AUDIO_DURATION_MS: u64 = 30_000;

/// Maximum audio file size in bytes (1 MB).
pub const MAX_AUDIO_SIZE_BYTES: usize = 1_048_576;

/// Default sample rate (16kHz).
pub const DEFAULT_SAMPLE_RATE: u32 = 16000;

/// Default audio channels (mono).
pub const DEFAULT_CHANNELS: u16 = 1;

/// API header name.
pub const API_KEY_HEADER: &str = "Apikey";

/// Library identifier header.
pub const LIB_HEADER: &str = "X-lib";

/// Library identifier value.
pub const LIB_VALUE: &str = "waav-gateway";

// =============================================================================
// Enums
// =============================================================================

/// NECTEC STT model selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NectecSttModel {
    /// Partii5 - newer engine (recommended)
    #[default]
    Partii5,
    /// Partii4 - legacy engine
    Partii4,
}

impl NectecSttModel {
    /// Get the API endpoint for this model.
    pub fn endpoint(&self) -> &'static str {
        match self {
            NectecSttModel::Partii5 => PARTII5_ENDPOINT,
            NectecSttModel::Partii4 => PARTII4_ENDPOINT,
        }
    }

    /// Get model name for display.
    pub fn as_str(&self) -> &'static str {
        match self {
            NectecSttModel::Partii5 => "partii5",
            NectecSttModel::Partii4 => "partii4",
        }
    }

    /// Parse model from string (relaxed matching).
    pub fn from_str_relaxed(s: &str) -> Option<Self> {
        let s = s.to_lowercase();
        match s.as_str() {
            "partii5" | "partii-5" | "partii_5" | "5" | "v5" => Some(NectecSttModel::Partii5),
            "partii4" | "partii-4" | "partii_4" | "4" | "v4" | "legacy" => {
                Some(NectecSttModel::Partii4)
            }
            _ => None,
        }
    }
}

impl fmt::Display for NectecSttModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for NectecSttModel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        NectecSttModel::from_str_relaxed(s)
            .ok_or_else(|| format!("Invalid NECTEC model: {}. Use 'partii5' or 'partii4'", s))
    }
}

/// Partii4 output level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Partii4OutputLevel {
    /// Utterance level output (default)
    #[default]
    Utterance,
    /// Word level output
    Word,
}

impl Partii4OutputLevel {
    /// Get the API parameter value.
    pub fn as_param(&self) -> &'static str {
        match self {
            Partii4OutputLevel::Utterance => "--uttlevel",
            Partii4OutputLevel::Word => "--wrdlevel",
        }
    }
}

/// Partii4 output format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Partii4OutputFormat {
    /// Text format (default)
    #[default]
    Text,
    /// JSON format
    Json,
}

impl Partii4OutputFormat {
    /// Get the API parameter value.
    pub fn as_param(&self) -> &'static str {
        match self {
            Partii4OutputFormat::Text => "--txt",
            Partii4OutputFormat::Json => "--json",
        }
    }
}

// =============================================================================
// Configuration
// =============================================================================

/// NECTEC STT configuration.
#[derive(Debug, Clone)]
pub struct NectecSttConfig {
    /// API key for authentication.
    pub api_key: String,

    /// STT model to use.
    pub model: NectecSttModel,

    /// Audio sample rate in Hz.
    pub sample_rate: u32,

    /// Number of audio channels.
    pub channels: u16,

    /// Request timeout in seconds.
    pub request_timeout_secs: u64,

    /// Partii4 output level (only for Partii4).
    pub output_level: Partii4OutputLevel,

    /// Partii4 output format (only for Partii4).
    pub output_format: Partii4OutputFormat,

    /// Optional endpoint base override (scheme+host) for test/mock redirection.
    pub endpoint_override: Option<String>,
}

impl Default for NectecSttConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            model: NectecSttModel::default(),
            sample_rate: DEFAULT_SAMPLE_RATE,
            channels: DEFAULT_CHANNELS,
            request_timeout_secs: DEFAULT_REQUEST_TIMEOUT,
            output_level: Partii4OutputLevel::default(),
            output_format: Partii4OutputFormat::default(),
            endpoint_override: None,
        }
    }
}

impl NectecSttConfig {
    /// Create configuration from base STTConfig.
    pub fn from_base(config: &STTConfig) -> Result<Self, STTError> {
        let api_key = config.api_key.clone();

        if api_key.is_empty() {
            return Err(STTError::AuthenticationFailed(
                "NECTEC API key is required".to_string(),
            ));
        }

        // Parse model from config.model field
        let model = if !config.model.is_empty() {
            NectecSttModel::from_str_relaxed(&config.model).unwrap_or_default()
        } else {
            NectecSttModel::default()
        };

        let sample_rate = if config.sample_rate > 0 {
            config.sample_rate
        } else {
            DEFAULT_SAMPLE_RATE
        };

        let channels = if config.channels > 0 {
            config.channels
        } else {
            DEFAULT_CHANNELS
        };

        Ok(Self {
            api_key,
            model,
            sample_rate,
            channels,
            request_timeout_secs: DEFAULT_REQUEST_TIMEOUT,
            output_level: Partii4OutputLevel::default(),
            output_format: Partii4OutputFormat::default(),
            endpoint_override: None,
        })
    }

    /// Build from the standardized config (W1 keystone — FINAL batch, completes the STT migration).
    ///
    /// NECTEC is a simple batch Thai-only engine (Partii4/Partii5): its config exposes only the
    /// model, audio shape, timeout, and the two Partii4 output knobs (`output_level`/`output_format`).
    /// None of the canonical [`SttFeatures`](crate::core::stt::standard::SttFeatures) — interim_results,
    /// diarization, word_timestamps, smart_format, profanity_filter, filler_words, vad_events,
    /// endpointing/utterance timing, keyterms, redaction, language/entity detection — has a real field
    /// to map onto, so this is a pure `from_base` passthrough: a uniform standardized entry point with
    /// no feature mapping (every standardized feature is a capability gap and stays at its default).
    pub fn from_standard(
        std: &crate::core::stt::standard::StandardSTTConfig,
    ) -> Result<Self, STTError> {
        let mut cfg = Self::from_base(&std.base)?;
        cfg.endpoint_override = std.endpoint_override().map(|s| s.to_string());
        Ok(cfg)
    }

    /// Validate the configuration.
    pub fn validate(&self) -> Result<(), String> {
        if self.api_key.is_empty() {
            return Err("NECTEC API key is required".to_string());
        }

        // NECTEC requires 16kHz sample rate
        if self.sample_rate != 16000 {
            return Err(format!(
                "NECTEC requires 16kHz sample rate, got {} Hz",
                self.sample_rate
            ));
        }

        // NECTEC requires mono audio
        if self.channels != 1 {
            return Err(format!(
                "NECTEC requires mono audio (1 channel), got {} channels",
                self.channels
            ));
        }

        Ok(())
    }

    /// Get the API endpoint for the configured model.
    pub fn endpoint(&self) -> &'static str {
        self.model.endpoint()
    }
}

// =============================================================================
// Response Types
// =============================================================================

/// Partii5 API response.
#[derive(Debug, Clone, Deserialize)]
pub struct Partii5Response {
    /// Transcription content.
    pub content: Option<String>,

    /// Error message if any.
    pub error: Option<String>,
}

impl Partii5Response {
    /// Get the transcription text.
    pub fn text(&self) -> Option<&str> {
        self.content.as_deref()
    }

    /// Check if response has error.
    pub fn has_error(&self) -> bool {
        self.error.is_some()
    }
}

/// Partii4 API response.
#[derive(Debug, Clone, Deserialize)]
pub struct Partii4Response {
    /// Transcription message.
    pub message: Option<String>,

    /// Error message if any.
    pub error: Option<String>,

    /// Status code.
    #[serde(default)]
    pub status: Option<i32>,
}

impl Partii4Response {
    /// Get the transcription text.
    pub fn text(&self) -> Option<&str> {
        self.message.as_deref()
    }

    /// Check if response has error.
    pub fn has_error(&self) -> bool {
        self.error.is_some()
    }
}

/// NECTEC STT error types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NectecSttError {
    /// Invalid or missing API key.
    InvalidApiKey,
    /// Rate limit exceeded.
    RateLimitExceeded,
    /// Invalid audio format.
    InvalidAudioFormat(String),
    /// Audio too long.
    AudioTooLong,
    /// Audio too short.
    AudioTooShort,
    /// File too large.
    FileTooLarge,
    /// Server error.
    ServerError(String),
    /// Unknown error.
    Unknown(String),
}

impl fmt::Display for NectecSttError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NectecSttError::InvalidApiKey => write!(f, "Invalid or missing NECTEC API key"),
            NectecSttError::RateLimitExceeded => write!(f, "NECTEC rate limit exceeded"),
            NectecSttError::InvalidAudioFormat(msg) => write!(f, "Invalid audio format: {}", msg),
            NectecSttError::AudioTooLong => {
                write!(f, "Audio exceeds maximum duration of 30 seconds")
            }
            NectecSttError::AudioTooShort => write!(f, "Audio is too short"),
            NectecSttError::FileTooLarge => write!(f, "File exceeds maximum size of 1 MB"),
            NectecSttError::ServerError(msg) => write!(f, "NECTEC server error: {}", msg),
            NectecSttError::Unknown(msg) => write!(f, "NECTEC error: {}", msg),
        }
    }
}

impl std::error::Error for NectecSttError {}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // W1 keystone (FINAL batch): NECTEC is a simple batch engine with no advanced-feature fields,
    // so `from_standard` is a pure `from_base` passthrough — assert it succeeds and the base
    // (api_key/model) carries through even when advanced features are requested (they're capability
    // gaps and are intentionally dropped).
    #[test]
    fn from_standard_passthrough_carries_base() {
        use crate::core::stt::standard::{ProviderExtras, SttFeatures, StandardSTTConfig};
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "nectec".into(),
                api_key: "test-key".into(),
                model: "partii4".into(),
                ..Default::default()
            },
            features: SttFeatures {
                diarization: Some(true),
                profanity_filter: Some(true),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
            translation: None,
        };
        let cfg = NectecSttConfig::from_standard(&std).unwrap();
        assert_eq!(cfg.api_key, "test-key");
        assert_eq!(cfg.model, NectecSttModel::Partii4);
    }

    #[test]
    fn test_default_config() {
        let config = NectecSttConfig::default();
        assert!(config.api_key.is_empty());
        assert_eq!(config.model, NectecSttModel::Partii5);
        assert_eq!(config.sample_rate, 16000);
        assert_eq!(config.channels, 1);
    }

    #[test]
    fn test_model_endpoints() {
        assert_eq!(
            NectecSttModel::Partii5.endpoint(),
            "https://api.aiforthai.in.th/partii5-poc"
        );
        assert_eq!(
            NectecSttModel::Partii4.endpoint(),
            "https://api.aiforthai.in.th/partii-webapi"
        );
    }

    #[test]
    fn test_model_from_str() {
        assert_eq!(
            NectecSttModel::from_str_relaxed("partii5"),
            Some(NectecSttModel::Partii5)
        );
        assert_eq!(
            NectecSttModel::from_str_relaxed("partii4"),
            Some(NectecSttModel::Partii4)
        );
        assert_eq!(
            NectecSttModel::from_str_relaxed("5"),
            Some(NectecSttModel::Partii5)
        );
        assert_eq!(
            NectecSttModel::from_str_relaxed("legacy"),
            Some(NectecSttModel::Partii4)
        );
        assert_eq!(NectecSttModel::from_str_relaxed("invalid"), None);
    }

    #[test]
    fn test_model_display() {
        assert_eq!(NectecSttModel::Partii5.to_string(), "partii5");
        assert_eq!(NectecSttModel::Partii4.to_string(), "partii4");
    }

    #[test]
    fn test_output_level_params() {
        assert_eq!(Partii4OutputLevel::Utterance.as_param(), "--uttlevel");
        assert_eq!(Partii4OutputLevel::Word.as_param(), "--wrdlevel");
    }

    #[test]
    fn test_output_format_params() {
        assert_eq!(Partii4OutputFormat::Text.as_param(), "--txt");
        assert_eq!(Partii4OutputFormat::Json.as_param(), "--json");
    }

    #[test]
    fn test_config_validation_empty_key() {
        let config = NectecSttConfig::default();
        assert!(config.validate().is_err());
        assert!(
            config
                .validate()
                .unwrap_err()
                .contains("API key is required")
        );
    }

    #[test]
    fn test_config_validation_invalid_sample_rate() {
        let config = NectecSttConfig {
            api_key: "test_key".to_string(),
            sample_rate: 8000,
            ..Default::default()
        };
        assert!(config.validate().is_err());
        assert!(config.validate().unwrap_err().contains("16kHz sample rate"));
    }

    #[test]
    fn test_config_validation_invalid_channels() {
        let config = NectecSttConfig {
            api_key: "test_key".to_string(),
            sample_rate: 16000,
            channels: 2,
            ..Default::default()
        };
        assert!(config.validate().is_err());
        assert!(config.validate().unwrap_err().contains("mono audio"));
    }

    #[test]
    fn test_config_validation_valid() {
        let config = NectecSttConfig {
            api_key: "test_key".to_string(),
            sample_rate: 16000,
            channels: 1,
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_from_base_empty_key() {
        let base = STTConfig {
            api_key: String::new(),
            ..Default::default()
        };
        assert!(NectecSttConfig::from_base(&base).is_err());
    }

    #[test]
    fn test_from_base_valid() {
        let base = STTConfig {
            api_key: "test_key".to_string(),
            model: "partii5".to_string(),
            sample_rate: 16000,
            channels: 1,
            ..Default::default()
        };
        let config = NectecSttConfig::from_base(&base).unwrap();
        assert_eq!(config.api_key, "test_key");
        assert_eq!(config.model, NectecSttModel::Partii5);
    }

    #[test]
    fn test_from_base_with_partii4() {
        let base = STTConfig {
            api_key: "test_key".to_string(),
            model: "partii4".to_string(),
            ..Default::default()
        };
        let config = NectecSttConfig::from_base(&base).unwrap();
        assert_eq!(config.model, NectecSttModel::Partii4);
    }

    #[test]
    fn test_partii5_response_text() {
        let response = Partii5Response {
            content: Some("สวัสดีครับ".to_string()),
            error: None,
        };
        assert_eq!(response.text(), Some("สวัสดีครับ"));
        assert!(!response.has_error());
    }

    #[test]
    fn test_partii4_response_text() {
        let response = Partii4Response {
            message: Some("สวัสดีครับ".to_string()),
            error: None,
            status: None,
        };
        assert_eq!(response.text(), Some("สวัสดีครับ"));
        assert!(!response.has_error());
    }

    #[test]
    fn test_error_display() {
        assert_eq!(
            NectecSttError::InvalidApiKey.to_string(),
            "Invalid or missing NECTEC API key"
        );
        assert_eq!(
            NectecSttError::RateLimitExceeded.to_string(),
            "NECTEC rate limit exceeded"
        );
        assert_eq!(
            NectecSttError::AudioTooLong.to_string(),
            "Audio exceeds maximum duration of 30 seconds"
        );
    }
}
