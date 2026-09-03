//! Configuration types for Viettel AI TTS API.
//!
//! This module provides configuration types for Viettel Group's
//! AI Text-to-Speech service.
//!
//! # Overview
//!
//! Viettel AI TTS transforms Vietnamese texts into speech with regional
//! accents using deep learning technology. The service delivers voices
//! that sound 95% like real humans.
//!
//! # Features
//!
//! - 11 Vietnamese voices from 3 regions
//! - Adjustable speech speed
//! - WAV audio output
//! - Automatic pauses and intonation
//!
//! # Authentication
//!
//! The API uses token-based authentication:
//! - `token`: Your Viettel AI token from dashboard

use crate::core::tts::base::{TTSConfig, TTSError};
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

/// Viettel AI TTS synthesis endpoint.
pub const VIETTEL_TTS_ENDPOINT: &str = "https://viettelgroup.ai/voice/api/tts/v1/rest/syn";

/// Viettel AI TTS voices list endpoint.
pub const VIETTEL_VOICES_ENDPOINT: &str = "https://viettelgroup.ai/voice/api/tts/v1/rest/voices";

/// Minimum speech speed.
pub const MIN_SPEED: f32 = 0.5;

/// Maximum speech speed.
pub const MAX_SPEED: f32 = 2.0;

/// Default speech speed.
pub const DEFAULT_SPEED: f32 = 1.0;

/// Default request timeout in seconds.
pub const DEFAULT_REQUEST_TIMEOUT: u64 = 60;

/// Audio sample rate (Hz).
pub const AUDIO_SAMPLE_RATE: u32 = 16000;

/// Minimum text length.
pub const MIN_TEXT_LENGTH: usize = 1;

/// Maximum text length per request.
pub const MAX_TEXT_LENGTH: usize = 10000;

/// Default TTS return option (2 = binary audio).
pub const DEFAULT_TTS_RETURN_OPTION: u8 = 2;

// =============================================================================
// Voice Types
// =============================================================================

/// Viettel AI TTS voice options.
///
/// Viettel AI provides 11 Vietnamese voices categorized by region:
/// - Northern (5 voices): 3 female, 2 male
/// - Southern (4 voices): 3 female, 1 male
/// - Central (2 voices): 1 female, 1 male
///
/// Note: Voice IDs are fetched dynamically from the API.
/// Common known voice: "doanngocle"
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ViettelVoice {
    /// Doan Ngoc Le - Popular Northern female voice.
    #[default]
    DoanNgocLe,
    /// Northern female voice 1.
    NorthernFemale1,
    /// Northern female voice 2.
    NorthernFemale2,
    /// Northern female voice 3.
    NorthernFemale3,
    /// Northern male voice 1.
    NorthernMale1,
    /// Northern male voice 2.
    NorthernMale2,
    /// Southern female voice 1.
    SouthernFemale1,
    /// Southern female voice 2.
    SouthernFemale2,
    /// Southern female voice 3.
    SouthernFemale3,
    /// Southern male voice.
    SouthernMale,
    /// Central female voice.
    CentralFemale,
    /// Central male voice.
    CentralMale,
    /// Custom voice ID.
    Custom(String),
}

impl ViettelVoice {
    /// Get the voice ID string for the API.
    pub fn voice_id(&self) -> &str {
        match self {
            Self::DoanNgocLe => "doanngocle",
            Self::NorthernFemale1 => "hn_female_ngochuyen_news_48k-fhg",
            Self::NorthernFemale2 => "hn_female_thuthao_news_48k-fhg",
            Self::NorthernFemale3 => "hn_female_phuongtrang_news_48k-fhg",
            Self::NorthernMale1 => "hn_male_xuankien_news_48k-fhg",
            Self::NorthernMale2 => "hn_male_quang_news_48k-fhg",
            Self::SouthernFemale1 => "sg_female_thuphuong_news_48k-fhg",
            Self::SouthernFemale2 => "sg_female_minhly_news_48k-fhg",
            Self::SouthernFemale3 => "sg_female_huonggiang_news_48k-fhg",
            Self::SouthernMale => "sg_male_trongphuc_news_48k-fhg",
            Self::CentralFemale => "hn_female_maiphuong_news_48k-fhg",
            Self::CentralMale => "hn_male_thanhtung_news_48k-fhg",
            Self::Custom(id) => id.as_str(),
        }
    }

    /// Get the display name for the voice.
    pub fn display_name(&self) -> String {
        match self {
            Self::DoanNgocLe => "Doan Ngoc Le (Northern Female)".to_string(),
            Self::NorthernFemale1 => "Ngoc Huyen (Northern Female)".to_string(),
            Self::NorthernFemale2 => "Thu Thao (Northern Female)".to_string(),
            Self::NorthernFemale3 => "Phuong Trang (Northern Female)".to_string(),
            Self::NorthernMale1 => "Xuan Kien (Northern Male)".to_string(),
            Self::NorthernMale2 => "Quang (Northern Male)".to_string(),
            Self::SouthernFemale1 => "Thu Phuong (Southern Female)".to_string(),
            Self::SouthernFemale2 => "Minh Ly (Southern Female)".to_string(),
            Self::SouthernFemale3 => "Huong Giang (Southern Female)".to_string(),
            Self::SouthernMale => "Trong Phuc (Southern Male)".to_string(),
            Self::CentralFemale => "Mai Phuong (Central Female)".to_string(),
            Self::CentralMale => "Thanh Tung (Central Male)".to_string(),
            Self::Custom(id) => format!("Custom ({})", id),
        }
    }

    /// Get the primary language for the voice.
    pub fn language(&self) -> &'static str {
        "vi-VN"
    }

    /// Get the accent/region for the voice.
    pub fn region(&self) -> &'static str {
        match self {
            Self::DoanNgocLe
            | Self::NorthernFemale1
            | Self::NorthernFemale2
            | Self::NorthernFemale3
            | Self::NorthernMale1
            | Self::NorthernMale2 => "Northern",
            Self::SouthernFemale1
            | Self::SouthernFemale2
            | Self::SouthernFemale3
            | Self::SouthernMale => "Southern",
            Self::CentralFemale | Self::CentralMale => "Central",
            Self::Custom(_) => "Unknown",
        }
    }

    /// Get the gender for the voice.
    pub fn gender(&self) -> &'static str {
        match self {
            Self::NorthernMale1 | Self::NorthernMale2 | Self::SouthernMale | Self::CentralMale => {
                "male"
            }
            Self::Custom(_) => "unknown",
            _ => "female",
        }
    }

    /// Parse voice from string.
    ///
    /// Accepts various formats:
    /// - Voice IDs: "doanngocle", etc.
    /// - With underscores: "doan_ngoc_le", etc.
    /// - Display names: "Doan Ngoc Le", etc.
    pub fn from_str_relaxed(s: &str) -> Self {
        let s_lower = s.trim().to_lowercase().replace([' ', '-'], "_");

        match s_lower.as_str() {
            // Default/common voice
            "doanngocle" | "doan_ngoc_le" | "default" | "female" | "0" => Self::DoanNgocLe,

            // Northern voices
            "hn_female_ngochuyen_news_48k-fhg" | "ngochuyen" | "ngoc_huyen" => {
                Self::NorthernFemale1
            }
            "hn_female_thuthao_news_48k-fhg" | "thuthao" | "thu_thao" => Self::NorthernFemale2,
            "hn_female_phuongtrang_news_48k-fhg" | "phuongtrang" | "phuong_trang" => {
                Self::NorthernFemale3
            }
            "hn_male_xuankien_news_48k-fhg" | "xuankien" | "xuan_kien" => Self::NorthernMale1,
            "hn_male_quang_news_48k-fhg" | "quang" => Self::NorthernMale2,

            // Southern voices
            "sg_female_thuphuong_news_48k-fhg" | "thuphuong" | "thu_phuong" => {
                Self::SouthernFemale1
            }
            "sg_female_minhly_news_48k-fhg" | "minhly" | "minh_ly" => Self::SouthernFemale2,
            "sg_female_huonggiang_news_48k-fhg" | "huonggiang" | "huong_giang" => {
                Self::SouthernFemale3
            }
            "sg_male_trongphuc_news_48k-fhg" | "trongphuc" | "trong_phuc" => Self::SouthernMale,

            // Central voices
            "hn_female_maiphuong_news_48k-fhg" | "maiphuong" | "mai_phuong" => Self::CentralFemale,
            "hn_male_thanhtung_news_48k-fhg" | "thanhtung" | "thanh_tung" => Self::CentralMale,

            // Custom/unknown - pass through as custom
            _ => Self::Custom(s.to_string()),
        }
    }

    /// Get all known voices.
    pub fn all() -> Vec<Self> {
        vec![
            Self::DoanNgocLe,
            Self::NorthernFemale1,
            Self::NorthernFemale2,
            Self::NorthernFemale3,
            Self::NorthernMale1,
            Self::NorthernMale2,
            Self::SouthernFemale1,
            Self::SouthernFemale2,
            Self::SouthernFemale3,
            Self::SouthernMale,
            Self::CentralFemale,
            Self::CentralMale,
        ]
    }
}

// =============================================================================
// Configuration
// =============================================================================

/// Viettel AI TTS configuration.
#[derive(Debug, Clone)]
pub struct ViettelTtsConfig {
    /// API token for authentication.
    pub api_key: String,

    /// Voice/speaker selection.
    pub voice: ViettelVoice,

    /// Speech speed (0.5 to 2.0).
    pub speed: f32,

    /// Disable text preprocessing filter.
    pub without_filter: bool,

    /// TTS return option (2 = binary audio).
    pub tts_return_option: u8,

    /// Request timeout in seconds.
    pub request_timeout_secs: u64,

    /// Optional base URL override (scheme+host) for the synth endpoint; used by mock e2e tests.
    pub endpoint_override: Option<String>,
}

impl Default for ViettelTtsConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            voice: ViettelVoice::default(),
            speed: DEFAULT_SPEED,
            without_filter: false,
            tts_return_option: DEFAULT_TTS_RETURN_OPTION,
            request_timeout_secs: DEFAULT_REQUEST_TIMEOUT,
            endpoint_override: None,
        }
    }
}

impl ViettelTtsConfig {
    /// Create configuration from base TTSConfig.
    pub fn from_base(config: TTSConfig) -> Result<Self, TTSError> {
        let api_key = config.api_key.clone();

        if api_key.is_empty() {
            return Err(TTSError::AuthenticationFailed(
                "Viettel AI API token is required".to_string(),
            ));
        }

        // Parse voice from voice_id field
        let voice = match &config.voice_id {
            Some(id) if !id.is_empty() => ViettelVoice::from_str_relaxed(id),
            _ => ViettelVoice::default(),
        };

        // Parse speed from speaking_rate config
        let speed = config
            .speaking_rate
            .map(|rate| rate.clamp(MIN_SPEED, MAX_SPEED))
            .unwrap_or(DEFAULT_SPEED);

        // Get request timeout
        let request_timeout_secs = config.request_timeout.unwrap_or(DEFAULT_REQUEST_TIMEOUT);

        Ok(Self {
            api_key,
            voice,
            speed,
            without_filter: false,
            tts_return_option: DEFAULT_TTS_RETURN_OPTION,
            request_timeout_secs,
            endpoint_override: None,
        })
    }

    /// Build from the standardized TTS config (W1 keystone).
    ///
    /// Viettel exposes a single prosody knob (`speed`), so this maps `speed` onto it (reusing
    /// `from_base`'s `clamp(MIN_SPEED, MAX_SPEED)` so 1.0 = normal and the value matches the flat
    /// path). Viettel's non-standard `without_filter` and `tts_return_option` knobs are read from
    /// the `extras` passthrough. Features without a Viettel field (pitch, volume, stability,
    /// similarity_boost, style, use_speaker_boost, emotion, instructions, ssml, language,
    /// word_timestamps, streaming, seed, sample_rate) are skipped.
    pub fn from_standard(
        std: &crate::core::tts::standard::StandardTTSConfig,
    ) -> Result<Self, TTSError> {
        let f = &std.features;
        let mut cfg = Self::from_base(std.base.clone())?;

        if let Some(speed) = f.speed {
            // Reuse from_base's clamp so 1.0 = normal and the value matches the flat path.
            cfg.speed = speed.clamp(MIN_SPEED, MAX_SPEED);
        }

        // Provider-specific passthrough.
        if let Some(without_filter) = std.extras.0.get("without_filter").and_then(|v| v.as_bool()) {
            cfg.without_filter = without_filter;
        }
        if let Some(opt) = std
            .extras
            .0
            .get("tts_return_option")
            .and_then(|v| v.as_u64())
        {
            cfg.tts_return_option = opt as u8;
        }

        cfg.endpoint_override = std.endpoint_override().map(String::from);

        cfg.validate().map_err(TTSError::InvalidConfiguration)?;

        Ok(cfg)
    }

    /// Validate the configuration.
    pub fn validate(&self) -> Result<(), String> {
        if self.api_key.is_empty() {
            return Err("Viettel AI API token is required".to_string());
        }

        if self.speed < MIN_SPEED || self.speed > MAX_SPEED {
            return Err(format!(
                "Speed must be between {} and {}, got {}",
                MIN_SPEED, MAX_SPEED, self.speed
            ));
        }

        if let Some(endpoint) = &self.endpoint_override {
            validate_viettel_http_endpoint("endpoint_override", endpoint)?;
        }

        Ok(())
    }

    /// Validate text for synthesis.
    pub fn validate_text(text: &str) -> Result<(), TTSError> {
        let char_count = text.chars().count();

        if char_count < MIN_TEXT_LENGTH {
            return Err(TTSError::InvalidConfiguration(format!(
                "Text must be at least {} characters, got {}",
                MIN_TEXT_LENGTH, char_count
            )));
        }

        if char_count > MAX_TEXT_LENGTH {
            return Err(TTSError::InvalidConfiguration(format!(
                "Text exceeds maximum length of {} characters, got {}",
                MAX_TEXT_LENGTH, char_count
            )));
        }

        Ok(())
    }
}

// =============================================================================
// Request/Response Types
// =============================================================================

/// Viettel AI TTS request body.
#[derive(Debug, Clone, Serialize)]
pub struct ViettelTtsRequest {
    /// Text to synthesize.
    pub text: String,

    /// Voice name.
    pub voice: String,

    /// Request identifier.
    pub id: String,

    /// Disable text preprocessing.
    pub without_filter: bool,

    /// Speech speed.
    pub speed: f32,

    /// Return format option (2 = binary).
    pub tts_return_option: u8,
}

/// Viettel AI TTS API response (when tts_return_option = 1).
#[derive(Debug, Clone, Deserialize)]
pub struct ViettelTtsResponse {
    /// Status code.
    #[serde(default)]
    pub status: i32,

    /// Audio URL (when using return option 1).
    #[serde(default)]
    pub audio_url: String,

    /// Error message.
    #[serde(default)]
    pub message: String,
}

impl ViettelTtsResponse {
    /// Check if the response indicates success.
    pub fn is_success(&self) -> bool {
        self.status == 0
    }

    /// Get status message based on status code.
    pub fn status_message(&self) -> &str {
        if self.message.is_empty() {
            match self.status {
                0 => "Success",
                401 => "Unauthorized - Invalid or expired token",
                400 => "Bad request",
                429 => "Rate limited",
                500 => "Server error",
                _ => "Unknown error",
            }
        } else {
            &self.message
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // W1 keystone (TTS): the standardized `speed` feature Viettel can express reaches the request
    // `speed` field (clamped to MIN_SPEED..=MAX_SPEED), and the open extras passthrough carries the
    // provider-specific without_filter / tts_return_option knobs.
    #[test]
    fn from_standard_maps_speed_and_extras() {
        use crate::core::tts::standard::{ProviderExtras, StandardTTSConfig, TtsFeatures};
        let mut extras = serde_json::Map::new();
        extras.insert("without_filter".into(), serde_json::json!(true));
        extras.insert("tts_return_option".into(), serde_json::json!(1));
        let std = StandardTTSConfig {
            base: TTSConfig {
                provider: "viettel_ai".into(),
                api_key: "test_token".into(),
                ..Default::default()
            },
            features: TtsFeatures {
                speed: Some(1.5),
                pitch: Some(70.0), // capability gap: Viettel has no pitch, must be ignored
                ssml: Some(true),  // capability gap: Viettel has no SSML, must be ignored
                ..Default::default()
            },
            extras: ProviderExtras(extras),
        };
        let cfg = ViettelTtsConfig::from_standard(&std).unwrap();
        assert!((cfg.speed - 1.5).abs() < f32::EPSILON); // 1.5.clamp(0.5, 2.0) -> 1.5
        assert!(cfg.without_filter); // from extras passthrough
        assert_eq!(cfg.tts_return_option, 1);
    }

    #[test]
    fn test_voice_ids() {
        assert_eq!(ViettelVoice::DoanNgocLe.voice_id(), "doanngocle");
        assert!(
            ViettelVoice::NorthernFemale1
                .voice_id()
                .contains("hn_female")
        );
        assert!(ViettelVoice::SouthernMale.voice_id().contains("sg_male"));
    }

    #[test]
    fn test_voice_display_names() {
        assert!(
            ViettelVoice::DoanNgocLe
                .display_name()
                .contains("Doan Ngoc Le")
        );
        assert!(ViettelVoice::NorthernMale1.display_name().contains("Male"));
        assert!(
            ViettelVoice::SouthernFemale1
                .display_name()
                .contains("Female")
        );
    }

    #[test]
    fn test_voice_from_str_default() {
        assert_eq!(
            ViettelVoice::from_str_relaxed("doanngocle"),
            ViettelVoice::DoanNgocLe
        );
        assert_eq!(
            ViettelVoice::from_str_relaxed("default"),
            ViettelVoice::DoanNgocLe
        );
        assert_eq!(
            ViettelVoice::from_str_relaxed("female"),
            ViettelVoice::DoanNgocLe
        );
    }

    #[test]
    fn test_voice_from_str_underscores() {
        assert_eq!(
            ViettelVoice::from_str_relaxed("doan_ngoc_le"),
            ViettelVoice::DoanNgocLe
        );
    }

    #[test]
    fn test_voice_from_str_custom() {
        let custom = ViettelVoice::from_str_relaxed("some_custom_voice");
        match custom {
            ViettelVoice::Custom(id) => assert_eq!(id, "some_custom_voice"),
            _ => panic!("Expected Custom variant"),
        }
    }

    #[test]
    fn test_all_voices() {
        let voices = ViettelVoice::all();
        assert_eq!(voices.len(), 12);
        assert!(voices.contains(&ViettelVoice::DoanNgocLe));
        assert!(voices.contains(&ViettelVoice::SouthernMale));
        assert!(voices.contains(&ViettelVoice::CentralFemale));
    }

    #[test]
    fn test_voice_language() {
        for voice in ViettelVoice::all() {
            assert_eq!(voice.language(), "vi-VN");
        }
    }

    #[test]
    fn test_voice_gender() {
        assert_eq!(ViettelVoice::DoanNgocLe.gender(), "female");
        assert_eq!(ViettelVoice::NorthernMale1.gender(), "male");
        assert_eq!(ViettelVoice::SouthernMale.gender(), "male");
        assert_eq!(ViettelVoice::CentralFemale.gender(), "female");
    }

    #[test]
    fn test_voice_region() {
        assert_eq!(ViettelVoice::DoanNgocLe.region(), "Northern");
        assert_eq!(ViettelVoice::NorthernFemale1.region(), "Northern");
        assert_eq!(ViettelVoice::SouthernFemale1.region(), "Southern");
        assert_eq!(ViettelVoice::CentralFemale.region(), "Central");
    }

    #[test]
    fn test_config_default() {
        let config = ViettelTtsConfig::default();
        assert!(config.api_key.is_empty());
        assert_eq!(config.voice, ViettelVoice::DoanNgocLe);
        assert!((config.speed - DEFAULT_SPEED).abs() < f32::EPSILON);
        assert!(!config.without_filter);
        assert_eq!(config.tts_return_option, DEFAULT_TTS_RETURN_OPTION);
    }

    #[test]
    fn test_config_validation_empty_key() {
        let config = ViettelTtsConfig::default();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_invalid_speed_low() {
        let mut config = ViettelTtsConfig::default();
        config.api_key = "test_token".to_string();
        config.speed = 0.1;

        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_invalid_speed_high() {
        let mut config = ViettelTtsConfig::default();
        config.api_key = "test_token".to_string();
        config.speed = 3.0;

        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_success() {
        let mut config = ViettelTtsConfig::default();
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

        let mut config = ViettelTtsConfig {
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
        let config = TTSConfig {
            api_key: String::new(),
            ..Default::default()
        };

        let result = ViettelTtsConfig::from_base(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_config_from_base_with_voice() {
        let config = TTSConfig {
            api_key: "test_token".to_string(),
            voice_id: Some("doanngocle".to_string()),
            ..Default::default()
        };

        let viettel_config = ViettelTtsConfig::from_base(config).unwrap();
        assert_eq!(viettel_config.voice, ViettelVoice::DoanNgocLe);
    }

    #[test]
    fn test_config_from_base_with_custom_voice() {
        let config = TTSConfig {
            api_key: "test_token".to_string(),
            voice_id: Some("custom_voice_id".to_string()),
            ..Default::default()
        };

        let viettel_config = ViettelTtsConfig::from_base(config).unwrap();
        match viettel_config.voice {
            ViettelVoice::Custom(id) => assert_eq!(id, "custom_voice_id"),
            _ => panic!("Expected Custom variant"),
        }
    }

    #[test]
    fn test_config_from_base_with_speed() {
        let config = TTSConfig {
            api_key: "test_token".to_string(),
            speaking_rate: Some(1.5),
            ..Default::default()
        };

        let viettel_config = ViettelTtsConfig::from_base(config).unwrap();
        assert!((viettel_config.speed - 1.5).abs() < f32::EPSILON);
    }

    #[test]
    fn test_config_from_base_speed_clamped() {
        // Test too low
        let config = TTSConfig {
            api_key: "test_token".to_string(),
            speaking_rate: Some(0.1),
            ..Default::default()
        };
        let viettel_config = ViettelTtsConfig::from_base(config).unwrap();
        assert!((viettel_config.speed - MIN_SPEED).abs() < f32::EPSILON);

        // Test too high
        let config = TTSConfig {
            api_key: "test_token".to_string(),
            speaking_rate: Some(5.0),
            ..Default::default()
        };
        let viettel_config = ViettelTtsConfig::from_base(config).unwrap();
        assert!((viettel_config.speed - MAX_SPEED).abs() < f32::EPSILON);
    }

    #[test]
    fn test_validate_text_empty() {
        let result = ViettelTtsConfig::validate_text("");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_text_too_long() {
        let long_text = "a".repeat(MAX_TEXT_LENGTH + 1);
        let result = ViettelTtsConfig::validate_text(&long_text);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_text_valid() {
        let result = ViettelTtsConfig::validate_text("Xin chào");
        assert!(result.is_ok());
    }

    #[test]
    fn test_response_success() {
        let response = ViettelTtsResponse {
            status: 0,
            audio_url: String::new(),
            message: String::new(),
        };

        assert!(response.is_success());
        assert_eq!(response.status_message(), "Success");
    }

    #[test]
    fn test_response_error() {
        let response = ViettelTtsResponse {
            status: 401,
            audio_url: String::new(),
            message: String::new(),
        };

        assert!(!response.is_success());
        assert!(response.status_message().contains("Unauthorized"));
    }

    #[test]
    fn test_response_custom_message() {
        let response = ViettelTtsResponse {
            status: 500,
            audio_url: String::new(),
            message: "Custom error message".to_string(),
        };

        assert!(!response.is_success());
        assert_eq!(response.status_message(), "Custom error message");
    }
}
