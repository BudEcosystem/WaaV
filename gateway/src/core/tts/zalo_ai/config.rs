//! Configuration types for Zalo AI TTS API.
//!
//! This module provides configuration types for VNG Corporation's
//! Zalo AI Text-to-Speech service.
//!
//! # Overview
//!
//! Zalo TTS (ZTTS) delivers fast and premium quality audio from Vietnamese text.
//! It is optimized for real-time and high-volume traffic applications such as:
//! - News websites
//! - Voice streaming services
//! - Chatbots
//! - Virtual assistants
//!
//! # Features
//!
//! - 4 Vietnamese voices (Northern and Southern accents)
//! - Adjustable speech speed (0.8 - 1.2)
//! - WAV audio output (16kHz, mono, 16-bit)
//! - Two-step synthesis (get URL, then download audio)
//!
//! # Authentication
//!
//! The API uses a single API key header:
//! - `apikey`: Your Zalo AI API key

use crate::core::tts::base::{TTSConfig, TTSError};
use serde::{Deserialize, Serialize};

// =============================================================================
// Constants
// =============================================================================

/// Zalo TTS API endpoint.
pub const ZALO_TTS_ENDPOINT: &str = "https://api.zalo.ai/v1/tts/synthesize";

/// Minimum speech speed.
pub const MIN_SPEED: f32 = 0.8;

/// Maximum speech speed.
pub const MAX_SPEED: f32 = 1.2;

/// Default speech speed.
pub const DEFAULT_SPEED: f32 = 1.0;

/// Default request timeout in seconds.
pub const DEFAULT_REQUEST_TIMEOUT: u64 = 30;

/// Audio sample rate (Hz).
pub const AUDIO_SAMPLE_RATE: u32 = 16000;

/// Audio channels (mono).
pub const AUDIO_CHANNELS: u8 = 1;

/// Audio sample width in bytes (16-bit).
pub const AUDIO_SAMPLE_WIDTH: u8 = 2;

// =============================================================================
// Voice Types
// =============================================================================

/// Zalo AI TTS voice options.
///
/// Zalo TTS supports 4 Vietnamese voices with regional accents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ZaloVoice {
    /// Female Southern Vietnamese voice.
    FemaleSouth,
    /// Female Northern Vietnamese voice.
    #[default]
    FemaleNorth,
    /// Male Southern Vietnamese voice.
    MaleSouth,
    /// Male Northern Vietnamese voice.
    MaleNorth,
}

impl ZaloVoice {
    /// Get the speaker ID for the API.
    ///
    /// The API uses integer IDs 1-4 for speaker selection.
    pub fn speaker_id(&self) -> u8 {
        match self {
            Self::FemaleSouth => 1,
            Self::FemaleNorth => 2,
            Self::MaleSouth => 3,
            Self::MaleNorth => 4,
        }
    }

    /// Get the display name for the voice.
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::FemaleSouth => "Nữ miền Nam (Female Southern)",
            Self::FemaleNorth => "Nữ miền Bắc (Female Northern)",
            Self::MaleSouth => "Nam miền Nam (Male Southern)",
            Self::MaleNorth => "Nam miền Bắc (Male Northern)",
        }
    }

    /// Get the primary language for the voice.
    pub fn language(&self) -> &'static str {
        "vi-VN"
    }

    /// Get the accent/region for the voice.
    pub fn region(&self) -> &'static str {
        match self {
            Self::FemaleSouth | Self::MaleSouth => "South",
            Self::FemaleNorth | Self::MaleNorth => "North",
        }
    }

    /// Get the gender for the voice.
    pub fn gender(&self) -> &'static str {
        match self {
            Self::FemaleSouth | Self::FemaleNorth => "female",
            Self::MaleSouth | Self::MaleNorth => "male",
        }
    }

    /// Parse voice from string.
    ///
    /// Accepts various formats:
    /// - Numeric IDs: "1", "2", "3", "4"
    /// - Names: "female_south", "female_north", "male_south", "male_north"
    /// - Short names: "fs", "fn", "ms", "mn"
    pub fn from_str_relaxed(s: &str) -> Option<Self> {
        let s = s.trim().to_lowercase();

        match s.as_str() {
            // Numeric IDs
            "1" => Some(Self::FemaleSouth),
            "2" => Some(Self::FemaleNorth),
            "3" => Some(Self::MaleSouth),
            "4" => Some(Self::MaleNorth),

            // Full names
            "female_south" | "femalesouth" | "south_female" | "southfemale" => {
                Some(Self::FemaleSouth)
            }
            "female_north" | "femalenorth" | "north_female" | "northfemale" => {
                Some(Self::FemaleNorth)
            }
            "male_south" | "malesouth" | "south_male" | "southmale" => Some(Self::MaleSouth),
            "male_north" | "malenorth" | "north_male" | "northmale" => Some(Self::MaleNorth),

            // Short names
            "fs" | "sf" => Some(Self::FemaleSouth),
            "fn" | "nf" => Some(Self::FemaleNorth),
            "ms" | "sm" => Some(Self::MaleSouth),
            "mn" | "nm" => Some(Self::MaleNorth),

            // Vietnamese names
            "nu_mien_nam" | "giong_nu_mien_nam" => Some(Self::FemaleSouth),
            "nu_mien_bac" | "giong_nu_mien_bac" => Some(Self::FemaleNorth),
            "nam_mien_nam" | "giong_nam_mien_nam" => Some(Self::MaleSouth),
            "nam_mien_bac" | "giong_nam_mien_bac" => Some(Self::MaleNorth),

            _ => None,
        }
    }

    /// Get all available voices.
    pub fn all() -> Vec<Self> {
        vec![
            Self::FemaleSouth,
            Self::FemaleNorth,
            Self::MaleSouth,
            Self::MaleNorth,
        ]
    }
}

// =============================================================================
// Configuration
// =============================================================================

/// Zalo AI TTS configuration.
#[derive(Debug, Clone)]
pub struct ZaloTtsConfig {
    /// API key for authentication.
    pub api_key: String,

    /// Voice/speaker selection.
    pub voice: ZaloVoice,

    /// Speech speed (0.8 - 1.2).
    pub speed: f32,

    /// Request timeout in seconds.
    pub request_timeout_secs: u64,
}

impl Default for ZaloTtsConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            voice: ZaloVoice::default(),
            speed: DEFAULT_SPEED,
            request_timeout_secs: DEFAULT_REQUEST_TIMEOUT,
        }
    }
}

impl ZaloTtsConfig {
    /// Create configuration from base TTSConfig.
    pub fn from_base(config: TTSConfig) -> Result<Self, TTSError> {
        let api_key = config.api_key.clone();

        if api_key.is_empty() {
            return Err(TTSError::AuthenticationFailed(
                "Zalo API key is required".to_string(),
            ));
        }

        // Parse voice from voice_id field
        let voice = match &config.voice_id {
            Some(id) if !id.is_empty() => ZaloVoice::from_str_relaxed(id).unwrap_or_else(|| {
                tracing::warn!(
                    "Unknown Zalo voice '{}', using default (Female Northern)",
                    id
                );
                ZaloVoice::default()
            }),
            _ => ZaloVoice::default(),
        };

        // Parse speed from speaking_rate config
        let speed = config
            .speaking_rate
            .unwrap_or(DEFAULT_SPEED)
            .clamp(MIN_SPEED, MAX_SPEED);

        // Get request timeout
        let request_timeout_secs = config.request_timeout.unwrap_or(DEFAULT_REQUEST_TIMEOUT);

        Ok(Self {
            api_key,
            voice,
            speed,
            request_timeout_secs,
        })
    }

    /// Build from the standardized TTS config. Zalo's only prosody knob is `speed`, a
    /// 1.0-is-normal multiplier matching the standardized feature, so `speed` -> `speed`
    /// (reusing `from_base`'s `MIN_SPEED..=MAX_SPEED` clamp for consistency). The non-standard
    /// `request_timeout_secs` knob is read from the `extras` passthrough. Zalo has no field for
    /// pitch, volume, stability, similarity_boost, style, use_speaker_boost, emotion, instructions,
    /// SSML, language, word_timestamps, streaming, seed or sample_rate, so those features are
    /// skipped.
    pub fn from_standard(std: &crate::core::tts::standard::StandardTTSConfig) -> Result<Self, TTSError> {
        let f = &std.features;
        let mut cfg = Self::from_base(std.base.clone())?;

        if let Some(speed) = f.speed {
            // Standardized speed is a 1.0-is-normal multiplier; Zalo uses the same scale.
            cfg.speed = speed.clamp(MIN_SPEED, MAX_SPEED);
        }

        // Provider-specific passthrough.
        if let Some(timeout) = std
            .extras
            .0
            .get("request_timeout_secs")
            .and_then(|v| v.as_u64())
        {
            cfg.request_timeout_secs = timeout;
        }

        Ok(cfg)
    }

    /// Validate the configuration.
    pub fn validate(&self) -> Result<(), String> {
        if self.api_key.is_empty() {
            return Err("Zalo API key is required".to_string());
        }

        if self.speed < MIN_SPEED || self.speed > MAX_SPEED {
            return Err(format!(
                "Speed must be between {} and {}, got {}",
                MIN_SPEED, MAX_SPEED, self.speed
            ));
        }

        Ok(())
    }

    /// Build URL-encoded request body.
    pub fn build_request_body(&self, text: &str) -> String {
        // URL-encode text for form submission
        let encoded_text: String = url::form_urlencoded::byte_serialize(text.as_bytes()).collect();

        format!(
            "input={}&speed={}&speaker_id={}",
            encoded_text,
            self.speed,
            self.voice.speaker_id()
        )
    }
}

// =============================================================================
// Response Types
// =============================================================================

/// Zalo TTS API response.
#[derive(Debug, Clone, Deserialize)]
pub struct ZaloTtsResponse {
    /// Error code (0 = success).
    pub error_code: i32,

    /// Error message (optional).
    #[serde(default)]
    pub error_message: String,

    /// Response data.
    #[serde(default)]
    pub data: Option<ZaloTtsData>,
}

/// Zalo TTS response data.
#[derive(Debug, Clone, Deserialize)]
pub struct ZaloTtsData {
    /// URL to download the synthesized audio.
    pub url: String,
}

impl ZaloTtsResponse {
    /// Check if the response indicates success.
    pub fn is_success(&self) -> bool {
        self.error_code == 0
    }

    /// Get the audio URL if successful.
    pub fn audio_url(&self) -> Option<&str> {
        if self.is_success() {
            self.data.as_ref().map(|d| d.url.as_str())
        } else {
            None
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // W1 keystone: the standardized `speed` feature reaches Zalo's only prosody field (reusing
    // `from_base`'s 0.8-1.2 clamp), and the non-standard `request_timeout_secs` knob flows through
    // the extras passthrough. Features Zalo cannot express (pitch, volume, ssml, ...) are ignored.
    #[test]
    fn from_standard_maps_speed_and_extras() {
        use crate::core::tts::standard::{ProviderExtras, StandardTTSConfig, TtsFeatures};
        let mut extras = serde_json::Map::new();
        extras.insert("request_timeout_secs".into(), serde_json::json!(45));
        let std = StandardTTSConfig {
            base: TTSConfig {
                provider: "zalo_ai".into(),
                api_key: "test_key".into(),
                voice_id: Some("male_north".into()),
                ..Default::default()
            },
            features: TtsFeatures {
                speed: Some(1.1),
                pitch: Some(2.0),  // capability gap: Zalo has no pitch field, must be ignored
                ssml: Some(true),  // capability gap: Zalo has no SSML field, must be ignored
                ..Default::default()
            },
            extras: ProviderExtras(extras),
        };
        let cfg = ZaloTtsConfig::from_standard(&std).unwrap();
        assert_eq!(cfg.speed, 1.1); // 1.1 is within 0.8..=1.2, unchanged by the clamp
        assert_eq!(cfg.voice, ZaloVoice::MaleNorth); // from_base passthrough
        assert_eq!(cfg.request_timeout_secs, 45); // from extras passthrough
    }

    #[test]
    fn test_voice_speaker_ids() {
        assert_eq!(ZaloVoice::FemaleSouth.speaker_id(), 1);
        assert_eq!(ZaloVoice::FemaleNorth.speaker_id(), 2);
        assert_eq!(ZaloVoice::MaleSouth.speaker_id(), 3);
        assert_eq!(ZaloVoice::MaleNorth.speaker_id(), 4);
    }

    #[test]
    fn test_voice_display_names() {
        assert!(ZaloVoice::FemaleSouth.display_name().contains("Southern"));
        assert!(ZaloVoice::FemaleNorth.display_name().contains("Northern"));
        assert!(ZaloVoice::MaleSouth.display_name().contains("Southern"));
        assert!(ZaloVoice::MaleNorth.display_name().contains("Northern"));
    }

    #[test]
    fn test_voice_from_str_numeric() {
        assert_eq!(
            ZaloVoice::from_str_relaxed("1"),
            Some(ZaloVoice::FemaleSouth)
        );
        assert_eq!(
            ZaloVoice::from_str_relaxed("2"),
            Some(ZaloVoice::FemaleNorth)
        );
        assert_eq!(ZaloVoice::from_str_relaxed("3"), Some(ZaloVoice::MaleSouth));
        assert_eq!(ZaloVoice::from_str_relaxed("4"), Some(ZaloVoice::MaleNorth));
    }

    #[test]
    fn test_voice_from_str_names() {
        assert_eq!(
            ZaloVoice::from_str_relaxed("female_south"),
            Some(ZaloVoice::FemaleSouth)
        );
        assert_eq!(
            ZaloVoice::from_str_relaxed("female_north"),
            Some(ZaloVoice::FemaleNorth)
        );
        assert_eq!(
            ZaloVoice::from_str_relaxed("male_south"),
            Some(ZaloVoice::MaleSouth)
        );
        assert_eq!(
            ZaloVoice::from_str_relaxed("male_north"),
            Some(ZaloVoice::MaleNorth)
        );
    }

    #[test]
    fn test_voice_from_str_short() {
        assert_eq!(
            ZaloVoice::from_str_relaxed("fs"),
            Some(ZaloVoice::FemaleSouth)
        );
        assert_eq!(
            ZaloVoice::from_str_relaxed("fn"),
            Some(ZaloVoice::FemaleNorth)
        );
        assert_eq!(
            ZaloVoice::from_str_relaxed("ms"),
            Some(ZaloVoice::MaleSouth)
        );
        assert_eq!(
            ZaloVoice::from_str_relaxed("mn"),
            Some(ZaloVoice::MaleNorth)
        );
    }

    #[test]
    fn test_voice_from_str_invalid() {
        assert_eq!(ZaloVoice::from_str_relaxed("invalid"), None);
        assert_eq!(ZaloVoice::from_str_relaxed("5"), None);
        assert_eq!(ZaloVoice::from_str_relaxed(""), None);
    }

    #[test]
    fn test_all_voices() {
        let voices = ZaloVoice::all();
        assert_eq!(voices.len(), 4);
        assert!(voices.contains(&ZaloVoice::FemaleSouth));
        assert!(voices.contains(&ZaloVoice::FemaleNorth));
        assert!(voices.contains(&ZaloVoice::MaleSouth));
        assert!(voices.contains(&ZaloVoice::MaleNorth));
    }

    #[test]
    fn test_config_default() {
        let config = ZaloTtsConfig::default();
        assert!(config.api_key.is_empty());
        assert_eq!(config.voice, ZaloVoice::FemaleNorth);
        assert_eq!(config.speed, DEFAULT_SPEED);
    }

    #[test]
    fn test_config_validation_empty_key() {
        let config = ZaloTtsConfig::default();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_invalid_speed() {
        let mut config = ZaloTtsConfig::default();
        config.api_key = "test_key".to_string();
        config.speed = 0.5; // Below MIN_SPEED

        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_success() {
        let mut config = ZaloTtsConfig::default();
        config.api_key = "test_key".to_string();

        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_build_request_body() {
        let mut config = ZaloTtsConfig::default();
        config.api_key = "test_key".to_string();
        config.voice = ZaloVoice::MaleNorth;
        config.speed = 1.1;

        let body = config.build_request_body("Xin chào");

        assert!(body.contains("input=Xin"));
        assert!(body.contains("speaker_id=4"));
        assert!(body.contains("speed=1.1"));
    }

    #[test]
    fn test_response_success() {
        let response = ZaloTtsResponse {
            error_code: 0,
            error_message: String::new(),
            data: Some(ZaloTtsData {
                url: "https://example.com/audio.wav".to_string(),
            }),
        };

        assert!(response.is_success());
        assert_eq!(response.audio_url(), Some("https://example.com/audio.wav"));
    }

    #[test]
    fn test_response_error() {
        let response = ZaloTtsResponse {
            error_code: 401,
            error_message: "Unauthorized".to_string(),
            data: None,
        };

        assert!(!response.is_success());
        assert_eq!(response.audio_url(), None);
    }

    #[test]
    fn test_voice_language() {
        for voice in ZaloVoice::all() {
            assert_eq!(voice.language(), "vi-VN");
        }
    }

    #[test]
    fn test_voice_gender() {
        assert_eq!(ZaloVoice::FemaleSouth.gender(), "female");
        assert_eq!(ZaloVoice::FemaleNorth.gender(), "female");
        assert_eq!(ZaloVoice::MaleSouth.gender(), "male");
        assert_eq!(ZaloVoice::MaleNorth.gender(), "male");
    }

    #[test]
    fn test_voice_region() {
        assert_eq!(ZaloVoice::FemaleSouth.region(), "South");
        assert_eq!(ZaloVoice::FemaleNorth.region(), "North");
        assert_eq!(ZaloVoice::MaleSouth.region(), "South");
        assert_eq!(ZaloVoice::MaleNorth.region(), "North");
    }

    #[test]
    fn test_speed_clamping() {
        // Test clamping to minimum (0.5 is below MIN_SPEED of 0.8)
        let config = TTSConfig {
            api_key: "test".to_string(),
            speaking_rate: Some(0.5),
            ..Default::default()
        };

        let zalo_config = ZaloTtsConfig::from_base(config).unwrap();
        assert_eq!(zalo_config.speed, MIN_SPEED); // Clamped to minimum

        // Test clamping to maximum (2.0 is above MAX_SPEED of 1.2)
        let config = TTSConfig {
            api_key: "test".to_string(),
            speaking_rate: Some(2.0),
            ..Default::default()
        };

        let zalo_config = ZaloTtsConfig::from_base(config).unwrap();
        assert_eq!(zalo_config.speed, MAX_SPEED); // Clamped to maximum
    }

    #[test]
    fn test_speed_within_range() {
        // Test speed within valid range
        let config = TTSConfig {
            api_key: "test".to_string(),
            speaking_rate: Some(1.0),
            ..Default::default()
        };

        let zalo_config = ZaloTtsConfig::from_base(config).unwrap();
        assert_eq!(zalo_config.speed, 1.0);
    }

    #[test]
    fn test_default_speed() {
        // Test default speed when not specified
        let config = TTSConfig {
            api_key: "test".to_string(),
            speaking_rate: None,
            ..Default::default()
        };

        let zalo_config = ZaloTtsConfig::from_base(config).unwrap();
        assert_eq!(zalo_config.speed, DEFAULT_SPEED);
    }
}
