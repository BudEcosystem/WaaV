//! Hume EVI configuration types.
//!
//! This module contains configuration types specific to Hume's Empathic Voice
//! Interface (EVI), which provides real-time bidirectional audio streaming with
//! emotional intelligence.
//!
//! # Example
//!
//! ```rust,ignore
//! use waav_gateway::core::realtime::hume::{HumeEVIConfig, EVIVersion};
//!
//! let config = HumeEVIConfig {
//!     api_key: "your-api-key".to_string(),
//!     config_id: Some("your-config-id".to_string()),
//!     evi_version: EVIVersion::V3,
//!     ..Default::default()
//! };
//! ```

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::error;

use super::messages::{
    AudioEncoding, HUME_EVI_DEFAULT_CHANNELS, HUME_EVI_DEFAULT_SAMPLE_RATE, HUME_EVI_WEBSOCKET_URL,
};
use crate::core::realtime::base::ReconnectionConfig;

const HUME_WEBSOCKET_URL_SCHEMES: &[&str] = &["ws", "wss"];

// =============================================================================
// Error Types
// =============================================================================

/// Configuration error for Hume EVI.
#[derive(Debug, Error, Clone, PartialEq)]
pub enum EVIConfigError {
    /// EVI version is deprecated and no longer supported.
    #[error("EVI version {version} is deprecated: {message}. Migration guide: {migration_guide}")]
    DeprecatedVersion {
        /// The deprecated version number
        version: String,
        /// Explanation of deprecation
        message: String,
        /// URL to migration guide
        migration_guide: String,
    },
    /// Unknown EVI version.
    #[error("Unknown EVI version: {0}")]
    UnknownVersion(String),
    /// API key is required.
    #[error("API key is required")]
    MissingApiKey,
    /// Invalid sample rate.
    #[error("Sample rate must be greater than 0")]
    InvalidSampleRate,
    /// Invalid channel count.
    #[error("Channels must be greater than 0")]
    InvalidChannels,
    /// WebSocket URL is invalid or blocked by SSRF protection.
    #[error("Hume EVI websocket_url rejected: {0}")]
    InvalidWebsocketUrl(String),
}

// =============================================================================
// EVI Version
// =============================================================================

/// EVI version to use for the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum EVIVersion {
    /// EVI version 1 (deprecated, sunset Aug 30, 2025).
    #[serde(rename = "1")]
    V1,
    /// EVI version 2 (deprecated, sunset Aug 30, 2025).
    #[serde(rename = "2")]
    V2,
    /// EVI version 3 (current, English only).
    #[default]
    #[serde(rename = "3")]
    V3,
    /// EVI version 4-mini (multilingual, lower latency).
    #[serde(rename = "4-mini")]
    V4Mini,
}

impl EVIVersion {
    /// Parse from a version string.
    ///
    /// This method rejects deprecated versions (V1 and V2) with an error.
    ///
    /// # Arguments
    /// * `version` - The version string (e.g., "1", "2", "3", "4-mini")
    ///
    /// # Returns
    /// * `Ok(EVIVersion)` - For supported versions (V3, V4-mini)
    /// * `Err(EVIConfigError::DeprecatedVersion)` - For deprecated versions (V1, V2)
    /// * `Err(EVIConfigError::UnknownVersion)` - For unrecognized versions
    pub fn from_version_str(version: &str) -> Result<Self, EVIConfigError> {
        match version {
            "1" | "2" => Err(EVIConfigError::DeprecatedVersion {
                version: version.to_string(),
                message:
                    "Hume EVI v1/v2 were sunset on August 30, 2025. Please migrate to v3 or v4-mini"
                        .to_string(),
                migration_guide: "https://dev.hume.ai/docs/evi-version".to_string(),
            }),
            "3" => Ok(EVIVersion::V3),
            "4-mini" => Ok(EVIVersion::V4Mini),
            _ => Err(EVIConfigError::UnknownVersion(version.to_string())),
        }
    }

    /// Get the version string for API requests.
    pub fn as_str(&self) -> &'static str {
        match self {
            EVIVersion::V1 => "1",
            EVIVersion::V2 => "2",
            EVIVersion::V3 => "3",
            EVIVersion::V4Mini => "4-mini",
        }
    }

    /// Returns true if this version is deprecated.
    ///
    /// EVI V1 and V2 reached end of support on August 30, 2025.
    pub fn is_deprecated(&self) -> bool {
        matches!(self, EVIVersion::V1 | EVIVersion::V2)
    }

    /// Validates that this version is still supported.
    ///
    /// Returns an error for deprecated versions (V1, V2).
    pub fn validate(&self) -> Result<(), EVIConfigError> {
        if self.is_deprecated() {
            error!(
                version = self.as_str(),
                "EVI version {} is deprecated and no longer supported. Sunset date: August 30, 2025.",
                self.as_str()
            );
            return Err(EVIConfigError::DeprecatedVersion {
                version: self.as_str().to_string(),
                message:
                    "Hume EVI v1/v2 were sunset on August 30, 2025. Please migrate to v3 or v4-mini"
                        .to_string(),
                migration_guide: "https://dev.hume.ai/docs/evi-version".to_string(),
            });
        }
        Ok(())
    }
}

impl std::fmt::Display for EVIVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =============================================================================
// Hume EVI Configuration
// =============================================================================

/// Configuration for Hume EVI (Empathic Voice Interface).
///
/// EVI provides real-time bidirectional audio streaming with emotional
/// intelligence. It analyzes the user's voice for emotional cues and
/// generates empathic responses.
///
/// # Features
///
/// - Full-duplex audio streaming
/// - 48-dimension prosody (emotion) analysis
/// - Empathic response generation
/// - Function calling support
/// - Conversation context preservation
///
/// # Audio Format
///
/// - Input: Linear16 PCM (44.1kHz, mono) or WebM
/// - Output: Base64-encoded WAV
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HumeEVIConfig {
    /// API key for Hume AI.
    pub api_key: String,

    /// EVI configuration ID (created in Hume dashboard).
    ///
    /// The config includes prompt, language model, voice, and tools.
    /// If not provided, default EVI settings are used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_id: Option<String>,

    /// Chat group ID for resuming a previous conversation.
    ///
    /// Use this to maintain context across multiple sessions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resumed_chat_group_id: Option<String>,

    /// EVI version to use.
    #[serde(default)]
    pub evi_version: EVIVersion,

    /// Voice ID or name to use for speech synthesis.
    ///
    /// Overrides the voice in the EVI configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice_id: Option<String>,

    /// Enable verbose transcription for interim user messages.
    #[serde(default)]
    pub verbose_transcription: bool,

    /// Input audio encoding format.
    #[serde(default)]
    pub input_encoding: AudioEncoding,

    /// Input audio sample rate in Hz.
    #[serde(default = "default_sample_rate")]
    pub sample_rate: u32,

    /// Number of input audio channels.
    #[serde(default = "default_channels")]
    pub channels: u8,

    /// System prompt override.
    ///
    /// Overrides the prompt configured in the EVI config.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,

    /// WebSocket URL (defaults to Hume's production endpoint).
    #[serde(default = "default_websocket_url")]
    pub websocket_url: String,

    /// Connection timeout in seconds.
    #[serde(default = "default_connection_timeout")]
    pub connection_timeout_seconds: u64,

    /// Enable automatic reconnection on connection loss.
    #[serde(default)]
    pub reconnection: Option<ReconnectionConfig>,
}

fn default_sample_rate() -> u32 {
    HUME_EVI_DEFAULT_SAMPLE_RATE
}

fn default_channels() -> u8 {
    HUME_EVI_DEFAULT_CHANNELS
}

fn default_websocket_url() -> String {
    HUME_EVI_WEBSOCKET_URL.to_string()
}

fn default_connection_timeout() -> u64 {
    30
}

impl Default for HumeEVIConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            config_id: None,
            resumed_chat_group_id: None,
            evi_version: EVIVersion::default(),
            voice_id: None,
            verbose_transcription: false,
            input_encoding: AudioEncoding::default(),
            sample_rate: HUME_EVI_DEFAULT_SAMPLE_RATE,
            channels: HUME_EVI_DEFAULT_CHANNELS,
            system_prompt: None,
            websocket_url: HUME_EVI_WEBSOCKET_URL.to_string(),
            connection_timeout_seconds: 30,
            reconnection: None,
        }
    }
}

impl HumeEVIConfig {
    /// Create a new configuration with an API key.
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            ..Default::default()
        }
    }

    /// Set the EVI configuration ID.
    pub fn with_config_id(mut self, config_id: impl Into<String>) -> Self {
        self.config_id = Some(config_id.into());
        self
    }

    /// Set the EVI version.
    pub fn with_version(mut self, version: EVIVersion) -> Self {
        self.evi_version = version;
        self
    }

    /// Set the voice ID.
    pub fn with_voice(mut self, voice_id: impl Into<String>) -> Self {
        self.voice_id = Some(voice_id.into());
        self
    }

    /// Enable verbose transcription.
    pub fn with_verbose_transcription(mut self) -> Self {
        self.verbose_transcription = true;
        self
    }

    /// Set the system prompt.
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    /// Set chat group ID for resuming a conversation.
    pub fn with_chat_group(mut self, chat_group_id: impl Into<String>) -> Self {
        self.resumed_chat_group_id = Some(chat_group_id.into());
        self
    }

    /// Set the audio encoding.
    pub fn with_encoding(mut self, encoding: AudioEncoding) -> Self {
        self.input_encoding = encoding;
        self
    }

    /// Set the sample rate.
    pub fn with_sample_rate(mut self, sample_rate: u32) -> Self {
        self.sample_rate = sample_rate;
        self
    }

    /// Build the WebSocket URL with query parameters.
    pub fn build_websocket_url(&self) -> String {
        let mut url = self.websocket_url.clone();
        let mut params = Vec::new();

        // URL encode helper
        fn encode(s: &str) -> String {
            url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
        }

        // Add API key
        params.push(format!("api_key={}", encode(&self.api_key)));

        // Add config ID if present
        if let Some(ref config_id) = self.config_id {
            params.push(format!("config_id={}", encode(config_id)));
        }

        // Add chat group ID for resuming
        if let Some(ref chat_group_id) = self.resumed_chat_group_id {
            params.push(format!("resumed_chat_group_id={}", encode(chat_group_id)));
        }

        // Add voice ID if present
        if let Some(ref voice_id) = self.voice_id {
            params.push(format!("voice_id={}", encode(voice_id)));
        }

        // Add verbose transcription flag
        if self.verbose_transcription {
            params.push("verbose_transcription=true".to_string());
        }

        // Add EVI version parameter
        // Note: V1/V2 are deprecated and will fail validation, but we still
        // use evi_version param as V3+ format since that's the current API.
        // If this code is reached with V1/V2, validate() wasn't called first.
        params.push(format!("evi_version={}", self.evi_version.as_str()));

        if !params.is_empty() {
            url.push('?');
            url.push_str(&params.join("&"));
        }

        url
    }

    /// Validate the configuration.
    ///
    /// Returns an error if:
    /// - EVI version is deprecated (V1, V2)
    /// - API key is missing
    /// - Sample rate is zero
    /// - Channel count is zero
    pub fn validate(&self) -> Result<(), EVIConfigError> {
        // Reject deprecated EVI versions - this is now a hard error, not a warning
        self.evi_version.validate()?;

        if self.api_key.is_empty() {
            return Err(EVIConfigError::MissingApiKey);
        }

        if self.sample_rate == 0 {
            return Err(EVIConfigError::InvalidSampleRate);
        }

        if self.channels == 0 {
            return Err(EVIConfigError::InvalidChannels);
        }

        crate::core::net::validate_url_for_ssrf(&self.websocket_url, HUME_WEBSOCKET_URL_SCHEMES)
            .map_err(EVIConfigError::InvalidWebsocketUrl)?;

        Ok(())
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // EVIVersion Tests
    // =========================================================================

    #[test]
    fn test_evi_version_as_str() {
        assert_eq!(EVIVersion::V1.as_str(), "1");
        assert_eq!(EVIVersion::V2.as_str(), "2");
        assert_eq!(EVIVersion::V3.as_str(), "3");
        assert_eq!(EVIVersion::V4Mini.as_str(), "4-mini");
    }

    #[test]
    fn test_evi_version_display() {
        assert_eq!(EVIVersion::V3.to_string(), "3");
        assert_eq!(EVIVersion::V4Mini.to_string(), "4-mini");
    }

    #[test]
    fn test_evi_version_default() {
        assert_eq!(EVIVersion::default(), EVIVersion::V3);
    }

    #[test]
    fn test_evi_version_from_str_v3() {
        let result = EVIVersion::from_version_str("3");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), EVIVersion::V3);
    }

    #[test]
    fn test_evi_version_from_str_v4_mini() {
        let result = EVIVersion::from_version_str("4-mini");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), EVIVersion::V4Mini);
    }

    #[test]
    fn test_evi_version_from_str_v1_rejected() {
        let result = EVIVersion::from_version_str("1");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, EVIConfigError::DeprecatedVersion { .. }));
        assert!(err.to_string().contains("deprecated"));
        assert!(err.to_string().contains("sunset"));
    }

    #[test]
    fn test_evi_version_from_str_v2_rejected() {
        let result = EVIVersion::from_version_str("2");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, EVIConfigError::DeprecatedVersion { .. }));
    }

    #[test]
    fn test_evi_version_from_str_unknown() {
        let result = EVIVersion::from_version_str("5");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            EVIConfigError::UnknownVersion(_)
        ));
    }

    #[test]
    fn test_evi_version_validate_v1_fails() {
        let result = EVIVersion::V1.validate();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            EVIConfigError::DeprecatedVersion { .. }
        ));
    }

    #[test]
    fn test_evi_version_validate_v2_fails() {
        let result = EVIVersion::V2.validate();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            EVIConfigError::DeprecatedVersion { .. }
        ));
    }

    #[test]
    fn test_evi_version_validate_v3_succeeds() {
        assert!(EVIVersion::V3.validate().is_ok());
    }

    #[test]
    fn test_evi_version_validate_v4_mini_succeeds() {
        assert!(EVIVersion::V4Mini.validate().is_ok());
    }

    // =========================================================================
    // HumeEVIConfig Tests
    // =========================================================================

    #[test]
    fn test_config_default() {
        let config = HumeEVIConfig::default();
        assert!(config.api_key.is_empty());
        assert!(config.config_id.is_none());
        assert_eq!(config.evi_version, EVIVersion::V3);
        assert_eq!(config.sample_rate, 44100);
        assert_eq!(config.channels, 1);
        assert!(!config.verbose_transcription);
    }

    #[test]
    fn test_config_new() {
        let config = HumeEVIConfig::new("test-key");
        assert_eq!(config.api_key, "test-key");
    }

    #[test]
    fn test_config_builder() {
        let config = HumeEVIConfig::new("test-key")
            .with_config_id("cfg_123")
            .with_version(EVIVersion::V4Mini)
            .with_voice("kora")
            .with_verbose_transcription()
            .with_system_prompt("You are a helpful assistant");

        assert_eq!(config.api_key, "test-key");
        assert_eq!(config.config_id, Some("cfg_123".to_string()));
        assert_eq!(config.evi_version, EVIVersion::V4Mini);
        assert_eq!(config.voice_id, Some("kora".to_string()));
        assert!(config.verbose_transcription);
        assert!(config.system_prompt.is_some());
    }

    #[test]
    fn test_build_websocket_url_minimal() {
        let config = HumeEVIConfig::new("test-key");
        let url = config.build_websocket_url();

        assert!(url.starts_with(HUME_EVI_WEBSOCKET_URL));
        assert!(url.contains("api_key=test-key"));
        assert!(url.contains("evi_version=3"));
    }

    #[test]
    fn test_build_websocket_url_with_config() {
        let config = HumeEVIConfig::new("test-key")
            .with_config_id("cfg_abc")
            .with_voice("voice_xyz")
            .with_verbose_transcription();

        let url = config.build_websocket_url();

        assert!(url.contains("config_id=cfg_abc"));
        assert!(url.contains("voice_id=voice_xyz"));
        assert!(url.contains("verbose_transcription=true"));
    }

    #[test]
    fn test_build_websocket_url_with_chat_group() {
        let config = HumeEVIConfig::new("test-key").with_chat_group("group_123");

        let url = config.build_websocket_url();
        assert!(url.contains("resumed_chat_group_id=group_123"));
    }

    #[test]
    fn test_build_websocket_url_v4_mini() {
        let config = HumeEVIConfig::new("test-key").with_version(EVIVersion::V4Mini);

        let url = config.build_websocket_url();
        assert!(url.contains("evi_version=4-mini"));
    }

    // =========================================================================
    // Validation Tests
    // =========================================================================

    #[test]
    fn test_validate_deprecated_v1_fails() {
        let config = HumeEVIConfig::new("test-key").with_version(EVIVersion::V1);
        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            EVIConfigError::DeprecatedVersion { .. }
        ));
    }

    #[test]
    fn test_validate_deprecated_v2_fails() {
        let config = HumeEVIConfig::new("test-key").with_version(EVIVersion::V2);
        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            EVIConfigError::DeprecatedVersion { .. }
        ));
    }

    #[test]
    fn test_validate_empty_api_key() {
        let config = HumeEVIConfig::default();
        let result = config.validate();
        // Default config has V3 (valid), but empty API key
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), EVIConfigError::MissingApiKey));
    }

    #[test]
    fn test_validate_zero_sample_rate() {
        let config = HumeEVIConfig {
            api_key: "test".to_string(),
            sample_rate: 0,
            ..Default::default()
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            EVIConfigError::InvalidSampleRate
        ));
    }

    #[test]
    fn test_validate_zero_channels() {
        let config = HumeEVIConfig {
            api_key: "test".to_string(),
            channels: 0,
            ..Default::default()
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            EVIConfigError::InvalidChannels
        ));
    }

    #[test]
    fn test_validate_websocket_url_ssrf_checked() {
        let _env = crate::core::net::ssrf_env_lock();
        let public = HumeEVIConfig {
            api_key: "test".to_string(),
            websocket_url: "wss://hume-proxy.invalid/v0/evi/chat".to_string(),
            ..Default::default()
        };
        assert!(public.validate().is_ok());

        let loopback = HumeEVIConfig {
            api_key: "test".to_string(),
            websocket_url: "ws://127.0.0.1:9000/evi".to_string(),
            ..Default::default()
        };
        let err = loopback
            .validate()
            .expect_err("loopback websocket_url must be rejected");
        assert!(err.to_string().contains("SSRF protection"), "{err}");

        let non_ws = HumeEVIConfig {
            api_key: "test".to_string(),
            websocket_url: "https://api.hume.ai/v0/evi/chat".to_string(),
            ..Default::default()
        };
        let err = non_ws
            .validate()
            .expect_err("non-WS websocket_url must be rejected");
        assert!(err.to_string().contains("not allowed"), "{err}");
    }

    #[test]
    fn test_validate_success() {
        let config = HumeEVIConfig::new("test-key");
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_v3_success() {
        let config = HumeEVIConfig::new("test-key").with_version(EVIVersion::V3);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_v4_mini_success() {
        let config = HumeEVIConfig::new("test-key").with_version(EVIVersion::V4Mini);
        assert!(config.validate().is_ok());
    }

    // =========================================================================
    // Serialization Tests
    // =========================================================================

    #[test]
    fn test_config_serialization() {
        let config = HumeEVIConfig::new("test-key")
            .with_config_id("cfg_123")
            .with_version(EVIVersion::V4Mini);

        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("test-key"));
        assert!(json.contains("cfg_123"));
        assert!(json.contains("4-mini"));
    }

    #[test]
    fn test_config_deserialization_v3() {
        let json = r#"{
            "api_key": "my-key",
            "config_id": "cfg_456",
            "evi_version": "3",
            "verbose_transcription": true
        }"#;

        let config: HumeEVIConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.api_key, "my-key");
        assert_eq!(config.config_id, Some("cfg_456".to_string()));
        assert_eq!(config.evi_version, EVIVersion::V3);
        assert!(config.verbose_transcription);
        // Validate should succeed for V3
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_deserialization_deprecated_v1_still_parses() {
        // Note: The serde deserializer still parses V1/V2 for backward compatibility,
        // but validation will fail. This allows reading old configs and giving
        // a meaningful error instead of a parse error.
        let json = r#"{
            "api_key": "my-key",
            "evi_version": "1"
        }"#;

        let config: HumeEVIConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.evi_version, EVIVersion::V1);
        // But validation should fail
        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            EVIConfigError::DeprecatedVersion { .. }
        ));
    }

    #[test]
    fn test_with_encoding() {
        let config = HumeEVIConfig::new("key").with_encoding(AudioEncoding::Webm);
        assert_eq!(config.input_encoding, AudioEncoding::Webm);
    }

    #[test]
    fn test_with_sample_rate() {
        let config = HumeEVIConfig::new("key").with_sample_rate(16000);
        assert_eq!(config.sample_rate, 16000);
    }

    // =========================================================================
    // Error Message Tests
    // =========================================================================

    #[test]
    fn test_deprecated_error_includes_migration_guide() {
        let result = EVIVersion::V1.validate();
        let err = result.unwrap_err();
        let err_string = err.to_string();

        assert!(
            err_string.contains("hume.ai"),
            "Error should include migration URL"
        );
        assert!(
            err_string.contains("2025"),
            "Error should include sunset year"
        );
    }

    #[test]
    fn test_error_display() {
        assert!(
            EVIConfigError::MissingApiKey
                .to_string()
                .contains("API key")
        );
        assert!(
            EVIConfigError::InvalidSampleRate
                .to_string()
                .contains("Sample rate")
        );
        assert!(
            EVIConfigError::InvalidChannels
                .to_string()
                .contains("Channels")
        );
        assert!(
            EVIConfigError::UnknownVersion("5".into())
                .to_string()
                .contains("5")
        );
    }
}
