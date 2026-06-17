//! Configuration types for FPT.AI STT API.
//!
//! This module provides configuration types for FPT Corporation's
//! FPT.AI Speech-to-Text service.

use crate::core::stt::base::{STTConfig, STTError};
use serde::{Deserialize, Serialize};

// =============================================================================
// Constants
// =============================================================================

/// FPT.AI STT API endpoint.
pub const FPT_STT_ENDPOINT: &str = "https://api.fpt.ai/hmi/asr/general";

/// Default request timeout in seconds.
pub const DEFAULT_REQUEST_TIMEOUT: u64 = 60;

/// Minimum audio duration in milliseconds.
pub const MIN_AUDIO_DURATION_MS: u64 = 100;

/// Maximum audio duration in milliseconds (5 minutes).
pub const MAX_AUDIO_DURATION_MS: u64 = 300_000;

/// Default sample rate.
pub const DEFAULT_SAMPLE_RATE: u32 = 16000;

/// Default audio channels.
pub const DEFAULT_CHANNELS: u16 = 1;

// =============================================================================
// Configuration
// =============================================================================

/// FPT.AI STT configuration.
#[derive(Debug, Clone)]
pub struct FptSttConfig {
    /// API key for authentication.
    pub api_key: String,

    /// Audio sample rate in Hz.
    pub sample_rate: u32,

    /// Number of audio channels.
    pub channels: u16,

    /// Request timeout in seconds.
    pub request_timeout_secs: u64,

    /// Base endpoint override (scheme://host) from the standardized `endpoint_override` — points the
    /// batch POST at an in-repo mock/proxy for credential-free e2e; `None` uses the production host.
    pub endpoint_override: Option<String>,
}

impl Default for FptSttConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            sample_rate: DEFAULT_SAMPLE_RATE,
            channels: DEFAULT_CHANNELS,
            request_timeout_secs: DEFAULT_REQUEST_TIMEOUT,
            endpoint_override: None,
        }
    }
}

impl FptSttConfig {
    /// Create configuration from base STTConfig.
    pub fn from_base(config: &STTConfig) -> Result<Self, STTError> {
        let api_key = config.api_key.clone();

        if api_key.is_empty() {
            return Err(STTError::AuthenticationFailed(
                "FPT.AI API key is required".to_string(),
            ));
        }

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
            sample_rate,
            channels,
            request_timeout_secs: DEFAULT_REQUEST_TIMEOUT,
            endpoint_override: None,
        })
    }

    /// Build from the standardized config (W1 keystone). FPT.AI is a simple batch decode endpoint
    /// whose config only carries transport knobs (api_key, sample_rate, channels, request_timeout)
    /// — it exposes no advanced-feature surface. None of the standardized
    /// [`SttFeatures`](crate::core::stt::standard::SttFeatures) (diarization, word_timestamps,
    /// smart_format, profanity_filter, filler_words, interim_results, vad_events, endpointing,
    /// utterance_end, keyterms, redaction, entity_detection, language_detection) map to a real
    /// field here, so this is a pure `from_base` passthrough: a uniform standardized entry point
    /// with no feature mapping.
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
            return Err("FPT.AI API key is required".to_string());
        }

        if self.sample_rate != 8000 && self.sample_rate != 16000 {
            return Err(format!(
                "Sample rate must be 8000 or 16000 Hz, got {}",
                self.sample_rate
            ));
        }

        if self.channels != 1 {
            return Err(format!(
                "Only mono audio is supported, got {} channels",
                self.channels
            ));
        }

        Ok(())
    }
}

// =============================================================================
// Response Types
// =============================================================================

/// FPT.AI STT API response.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FptSttResponse {
    /// Status code (0 = success).
    pub status: i32,

    /// Recognition hypotheses.
    #[serde(default)]
    pub hypotheses: Vec<FptSttHypothesis>,

    /// Request ID for tracking.
    #[serde(default)]
    pub id: String,
}

/// Recognition hypothesis from FPT.AI STT.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FptSttHypothesis {
    /// Recognized text.
    pub utterance: String,
}

impl FptSttResponse {
    /// Check if the response indicates success.
    pub fn is_success(&self) -> bool {
        self.status == 0
    }

    /// Get the primary transcription if successful.
    pub fn transcription(&self) -> Option<&str> {
        if self.is_success() && !self.hypotheses.is_empty() {
            Some(self.hypotheses[0].utterance.as_str())
        } else {
            None
        }
    }

    /// Get status message based on status code.
    pub fn status_message(&self) -> &'static str {
        match self.status {
            0 => "Success",
            1 => "No voice detected",
            2 => "Canceled",
            9 => "System busy",
            _ => "Unknown error",
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // W1 keystone: FPT.AI exposes no advanced-feature surface, so `from_standard` is a pure
    // `from_base` passthrough. Even with features set, it must succeed and carry the base
    // (api_key/sample_rate/channels) through unchanged.
    #[test]
    fn from_standard_passes_base_through() {
        use crate::core::stt::standard::{ProviderExtras, SttFeatures, StandardSTTConfig};
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "fpt_ai".into(),
                api_key: "test_key".into(),
                sample_rate: 8000,
                channels: 1,
                ..Default::default()
            },
            features: SttFeatures {
                diarization: Some(true),
                word_timestamps: Some(true),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
            translation: None,
        };
        let cfg = FptSttConfig::from_standard(&std).unwrap();
        assert_eq!(cfg.api_key, "test_key");
        assert_eq!(cfg.sample_rate, 8000);
        assert_eq!(cfg.channels, 1);
    }

    #[test]
    fn test_config_default() {
        let config = FptSttConfig::default();
        assert!(config.api_key.is_empty());
        assert_eq!(config.sample_rate, DEFAULT_SAMPLE_RATE);
        assert_eq!(config.channels, DEFAULT_CHANNELS);
    }

    #[test]
    fn test_config_validation_empty_key() {
        let config = FptSttConfig::default();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_invalid_sample_rate() {
        let mut config = FptSttConfig::default();
        config.api_key = "test_key".to_string();
        config.sample_rate = 44100;

        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_invalid_channels() {
        let mut config = FptSttConfig::default();
        config.api_key = "test_key".to_string();
        config.channels = 2;

        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_success() {
        let mut config = FptSttConfig::default();
        config.api_key = "test_key".to_string();

        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validation_8khz() {
        let mut config = FptSttConfig::default();
        config.api_key = "test_key".to_string();
        config.sample_rate = 8000;

        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_from_base_empty_key() {
        let config = STTConfig {
            api_key: String::new(),
            ..Default::default()
        };

        let result = FptSttConfig::from_base(&config);
        assert!(result.is_err());
    }

    #[test]
    fn test_config_from_base_with_params() {
        let config = STTConfig {
            api_key: "test_key".to_string(),
            sample_rate: 8000,
            channels: 1,
            ..Default::default()
        };

        let fpt_config = FptSttConfig::from_base(&config).unwrap();
        assert_eq!(fpt_config.sample_rate, 8000);
        assert_eq!(fpt_config.channels, 1);
    }

    #[test]
    fn test_response_success() {
        let response = FptSttResponse {
            status: 0,
            hypotheses: vec![FptSttHypothesis {
                utterance: "Xin chào".to_string(),
            }],
            id: "test-id".to_string(),
        };

        assert!(response.is_success());
        assert_eq!(response.transcription(), Some("Xin chào"));
        assert_eq!(response.status_message(), "Success");
    }

    #[test]
    fn test_response_no_voice() {
        let response = FptSttResponse {
            status: 1,
            hypotheses: vec![],
            id: "test-id".to_string(),
        };

        assert!(!response.is_success());
        assert_eq!(response.transcription(), None);
        assert_eq!(response.status_message(), "No voice detected");
    }

    #[test]
    fn test_response_canceled() {
        let response = FptSttResponse {
            status: 2,
            hypotheses: vec![],
            id: "test-id".to_string(),
        };

        assert!(!response.is_success());
        assert_eq!(response.status_message(), "Canceled");
    }

    #[test]
    fn test_response_system_busy() {
        let response = FptSttResponse {
            status: 9,
            hypotheses: vec![],
            id: "test-id".to_string(),
        };

        assert!(!response.is_success());
        assert_eq!(response.status_message(), "System busy");
    }

    #[test]
    fn test_response_empty_hypotheses() {
        let response = FptSttResponse {
            status: 0,
            hypotheses: vec![],
            id: "test-id".to_string(),
        };

        assert!(response.is_success());
        assert_eq!(response.transcription(), None);
    }
}
