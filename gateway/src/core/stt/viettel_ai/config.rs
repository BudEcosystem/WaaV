//! Configuration types for Viettel AI STT API.
//!
//! This module provides configuration types for Viettel Group's
//! AI Speech-to-Text service.
//!
//! # Overview
//!
//! Viettel AI STT converts Vietnamese audio to text with 96% accuracy
//! using advanced deep neural network technology.
//!
//! # Features
//!
//! - High Vietnamese accuracy (96%)
//! - Regional accent detection
//! - Multiple input types (direct recording, phone, operator)
//! - Enterprise-grade security
//!
//! # Authentication
//!
//! The API uses token-based authentication:
//! - `token`: Your Viettel AI token from dashboard

use crate::core::stt::base::{STTConfig, STTError};
use serde::{Deserialize, Serialize};

fn validate_viettel_http_endpoint(source: &str, endpoint: &str) -> Result<(), String> {
    let endpoint = endpoint.trim();
    if endpoint.is_empty() {
        return Ok(());
    }
    crate::core::net::validate_url_for_ssrf(endpoint, crate::core::net::HTTP_URL_SCHEMES)
        .map_err(|msg| format!("{source} rejected (SSRF protection): {msg}"))
}

// =============================================================================
// Constants
// =============================================================================

/// Viettel AI STT decode endpoint.
pub const VIETTEL_STT_ENDPOINT: &str = "https://viettelgroup.ai/voice/api/asr/v1/rest/decode_file";

/// Default request timeout in seconds.
pub const DEFAULT_REQUEST_TIMEOUT: u64 = 60;

/// Default sample rate.
pub const DEFAULT_SAMPLE_RATE: u32 = 16000;

/// Default audio channels.
pub const DEFAULT_CHANNELS: u16 = 1;

/// PCM format for signed 16-bit little endian.
pub const PCM_FORMAT_S16LE: &str = "S16LE";

/// Minimum audio buffer size in bytes before transcription.
pub const MIN_AUDIO_BUFFER_SIZE: usize = 1600; // ~50ms at 16kHz mono 16-bit

/// Maximum audio buffer size in bytes.
pub const MAX_AUDIO_BUFFER_SIZE: usize = 9_600_000; // ~5 minutes at 16kHz mono 16-bit

// =============================================================================
// Configuration
// =============================================================================

/// Viettel AI STT configuration.
#[derive(Debug, Clone)]
pub struct ViettelSttConfig {
    /// API token for authentication.
    pub api_key: String,

    /// Audio sample rate in Hz.
    pub sample_rate: u32,

    /// Audio format (e.g., "S16LE" for PCM).
    pub format: String,

    /// Number of audio channels.
    pub channels: u16,

    /// Optional ASR model code.
    pub asr_model: Option<String>,

    /// Request timeout in seconds.
    pub request_timeout_secs: u64,

    /// Base endpoint override (scheme://host) from the standardized `endpoint_override` — points the
    /// batch multipart POST at an in-repo mock/proxy for credential-free e2e; `None` uses production.
    pub endpoint_override: Option<String>,
}

impl Default for ViettelSttConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            sample_rate: DEFAULT_SAMPLE_RATE,
            format: PCM_FORMAT_S16LE.to_string(),
            channels: DEFAULT_CHANNELS,
            asr_model: None,
            request_timeout_secs: DEFAULT_REQUEST_TIMEOUT,
            endpoint_override: None,
        }
    }
}

impl ViettelSttConfig {
    /// Create configuration from base STTConfig.
    pub fn from_base(config: &STTConfig) -> Result<Self, STTError> {
        let api_key = config.api_key.clone();

        if api_key.is_empty() {
            return Err(STTError::AuthenticationFailed(
                "Viettel AI API token is required".to_string(),
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
            format: PCM_FORMAT_S16LE.to_string(),
            channels,
            // Honor an explicitly configured ASR model; previously `config.model` was dropped.
            asr_model: (!config.model.is_empty()).then(|| config.model.clone()),
            request_timeout_secs: DEFAULT_REQUEST_TIMEOUT,
            endpoint_override: None,
        })
    }

    /// Build from the standardized config (W1 keystone). Viettel AI is a simple batch decode
    /// endpoint whose config only carries transport knobs (api_key, sample_rate, format, channels,
    /// asr_model, timeout) — it exposes no advanced-feature surface. None of the standardized
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
        cfg.validate().map_err(STTError::ConfigurationError)?;
        Ok(cfg)
    }

    /// Validate the configuration.
    pub fn validate(&self) -> Result<(), String> {
        if self.api_key.is_empty() {
            return Err("Viettel AI API token is required".to_string());
        }

        if self.sample_rate == 0 {
            return Err("Sample rate must be greater than 0".to_string());
        }

        if self.channels == 0 {
            return Err("Channels must be greater than 0".to_string());
        }

        if let Some(endpoint) = &self.endpoint_override {
            validate_viettel_http_endpoint("endpoint_override", endpoint)?;
        }

        Ok(())
    }
}

// =============================================================================
// Response Types
// =============================================================================

/// Viettel AI STT API response.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ViettelSttResponse {
    /// Status code (0 = success).
    #[serde(default)]
    pub status: i32,

    /// Transcription result.
    #[serde(default)]
    pub result: String,

    /// Error message.
    #[serde(default)]
    pub message: String,
}

impl ViettelSttResponse {
    /// Check if the response indicates success.
    pub fn is_success(&self) -> bool {
        self.status == 0
    }

    /// Get the transcription text if successful.
    pub fn transcription(&self) -> Option<&str> {
        if self.is_success() && !self.result.is_empty() {
            Some(self.result.as_str())
        } else {
            None
        }
    }

    /// Get status message based on status code.
    pub fn status_message(&self) -> &str {
        if !self.message.is_empty() {
            return &self.message;
        }

        match self.status {
            0 => "Success",
            1 => "No voice detected",
            401 => "Unauthorized - Invalid or expired token",
            400 => "Bad request",
            500 => "Server error",
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

    // W1 keystone: Viettel AI exposes no advanced-feature surface, so `from_standard` is a pure
    // `from_base` passthrough. Even with features set, it must succeed and carry the base
    // (api_key/sample_rate/channels) through unchanged.
    #[test]
    fn from_standard_passes_base_through() {
        use crate::core::stt::standard::{ProviderExtras, StandardSTTConfig, SttFeatures};
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "viettel_ai".into(),
                api_key: "test_token".into(),
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
        let cfg = ViettelSttConfig::from_standard(&std).unwrap();
        assert_eq!(cfg.api_key, "test_token");
        assert_eq!(cfg.sample_rate, 8000);
        assert_eq!(cfg.channels, 1);
    }

    #[test]
    fn test_config_default() {
        let config = ViettelSttConfig::default();
        assert!(config.api_key.is_empty());
        assert_eq!(config.sample_rate, DEFAULT_SAMPLE_RATE);
        assert_eq!(config.channels, DEFAULT_CHANNELS);
        assert_eq!(config.format, PCM_FORMAT_S16LE);
        assert!(config.asr_model.is_none());
    }

    #[test]
    fn test_config_validation_empty_key() {
        let config = ViettelSttConfig::default();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_zero_sample_rate() {
        let mut config = ViettelSttConfig::default();
        config.api_key = "test_token".to_string();
        config.sample_rate = 0;

        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_zero_channels() {
        let mut config = ViettelSttConfig::default();
        config.api_key = "test_token".to_string();
        config.channels = 0;

        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_success() {
        let mut config = ViettelSttConfig::default();
        config.api_key = "test_token".to_string();

        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validation_rejects_ssrf_endpoint_override() {
        let _guard = crate::core::net::test_env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let previous = std::env::var_os("WAAV_ALLOW_LOOPBACK_ENDPOINTS");
        // SAFETY: test-only env mutation, serialized by core::net::test_env_lock.
        unsafe { std::env::remove_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS") };

        let mut config = ViettelSttConfig {
            api_key: "test_token".to_string(),
            ..Default::default()
        };

        config.endpoint_override = Some("https://viettel-proxy.example.com".to_string());
        assert!(config.validate().is_ok());

        config.endpoint_override = Some("http://127.0.0.1:9000".to_string());
        let err = config
            .validate()
            .expect_err("loopback endpoint_override must be rejected");
        assert!(err.contains("SSRF protection"), "{err}");

        config.endpoint_override = Some("file:///tmp/socket".to_string());
        let err = config
            .validate()
            .expect_err("non-HTTP endpoint_override must be rejected");
        assert!(err.contains("not allowed"), "{err}");

        // SAFETY: restore the process env before releasing the shared test env lock.
        unsafe {
            if let Some(previous) = previous {
                std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", previous);
            } else {
                std::env::remove_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS");
            }
        }
    }

    #[test]
    fn test_config_from_base_empty_key() {
        let config = STTConfig {
            api_key: String::new(),
            ..Default::default()
        };

        let result = ViettelSttConfig::from_base(&config);
        assert!(result.is_err());
    }

    #[test]
    fn test_config_from_base_with_params() {
        let config = STTConfig {
            api_key: "test_token".to_string(),
            sample_rate: 8000,
            channels: 1,
            ..Default::default()
        };

        let viettel_config = ViettelSttConfig::from_base(&config).unwrap();
        assert_eq!(viettel_config.sample_rate, 8000);
        assert_eq!(viettel_config.channels, 1);
    }

    #[test]
    fn test_config_from_base_default_sample_rate() {
        let config = STTConfig {
            api_key: "test_token".to_string(),
            sample_rate: 0,
            ..Default::default()
        };

        let viettel_config = ViettelSttConfig::from_base(&config).unwrap();
        assert_eq!(viettel_config.sample_rate, DEFAULT_SAMPLE_RATE);
    }

    #[test]
    fn test_response_success() {
        let response = ViettelSttResponse {
            status: 0,
            result: "Xin chào".to_string(),
            message: String::new(),
        };

        assert!(response.is_success());
        assert_eq!(response.transcription(), Some("Xin chào"));
        assert_eq!(response.status_message(), "Success");
    }

    #[test]
    fn test_response_no_voice() {
        let response = ViettelSttResponse {
            status: 1,
            result: String::new(),
            message: String::new(),
        };

        assert!(!response.is_success());
        assert_eq!(response.transcription(), None);
        assert_eq!(response.status_message(), "No voice detected");
    }

    #[test]
    fn test_response_unauthorized() {
        let response = ViettelSttResponse {
            status: 401,
            result: String::new(),
            message: String::new(),
        };

        assert!(!response.is_success());
        assert!(response.status_message().contains("Unauthorized"));
    }

    #[test]
    fn test_response_custom_message() {
        let response = ViettelSttResponse {
            status: 500,
            result: String::new(),
            message: "Custom error".to_string(),
        };

        assert!(!response.is_success());
        assert_eq!(response.status_message(), "Custom error");
    }

    #[test]
    fn test_response_empty_result_not_success() {
        let response = ViettelSttResponse {
            status: 0,
            result: String::new(),
            message: String::new(),
        };

        // Status is 0 but result is empty, so is_success is true
        // but transcription returns None
        assert!(response.is_success());
        assert_eq!(response.transcription(), None);
    }
}
