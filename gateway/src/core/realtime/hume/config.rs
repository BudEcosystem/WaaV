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
use tracing::warn;

use super::messages::{
    AudioEncoding, HUME_EVI_DEFAULT_CHANNELS, HUME_EVI_DEFAULT_SAMPLE_RATE,
    HUME_EVI_WEBSOCKET_URL,
};
use crate::core::realtime::base::ReconnectionConfig;

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

    /// Logs a deprecation warning if this version is deprecated.
    ///
    /// Call this during configuration validation to warn users.
    pub fn warn_if_deprecated(&self) {
        if self.is_deprecated() {
            warn!(
                version = self.as_str(),
                "EVI version {} is deprecated and reached end of support on August 30, 2025. \
                 Please migrate to EVI V3 or V4-mini. See: https://dev.hume.ai/docs/evi-version",
                self.as_str()
            );
        }
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
            params.push(format!(
                "resumed_chat_group_id={}",
                encode(chat_group_id)
            ));
        }

        // Add voice ID if present
        if let Some(ref voice_id) = self.voice_id {
            params.push(format!("voice_id={}", encode(voice_id)));
        }

        // Add verbose transcription flag
        if self.verbose_transcription {
            params.push("verbose_transcription=true".to_string());
        }

        // Add EVI version (for v3+)
        match self.evi_version {
            EVIVersion::V1 | EVIVersion::V2 => {
                // Use version query param for v1/v2
                params.push(format!("version={}", self.evi_version.as_str()));
            }
            EVIVersion::V3 | EVIVersion::V4Mini => {
                // V3+ uses evi_version
                params.push(format!("evi_version={}", self.evi_version.as_str()));
            }
        }

        if !params.is_empty() {
            url.push('?');
            url.push_str(&params.join("&"));
        }

        url
    }

    /// Validate the configuration.
    pub fn validate(&self) -> Result<(), String> {
        // Warn if using deprecated EVI version
        self.evi_version.warn_if_deprecated();

        if self.api_key.is_empty() {
            return Err("API key is required".to_string());
        }

        if self.sample_rate == 0 {
            return Err("Sample rate must be greater than 0".to_string());
        }

        if self.channels == 0 {
            return Err("Channels must be greater than 0".to_string());
        }

        Ok(())
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_build_websocket_url_v2() {
        let config = HumeEVIConfig::new("test-key").with_version(EVIVersion::V2);

        let url = config.build_websocket_url();
        assert!(url.contains("version=2"));
    }

    #[test]
    fn test_build_websocket_url_v4_mini() {
        let config = HumeEVIConfig::new("test-key").with_version(EVIVersion::V4Mini);

        let url = config.build_websocket_url();
        assert!(url.contains("evi_version=4-mini"));
    }

    #[test]
    fn test_validate_empty_api_key() {
        let config = HumeEVIConfig::default();
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("API key"));
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
        assert!(result.unwrap_err().contains("Sample rate"));
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
        assert!(result.unwrap_err().contains("Channels"));
    }

    #[test]
    fn test_validate_success() {
        let config = HumeEVIConfig::new("test-key");
        assert!(config.validate().is_ok());
    }

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
    fn test_config_deserialization() {
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
}
