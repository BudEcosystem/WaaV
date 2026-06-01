//! Configuration types for FPT.AI TTS API.
//!
//! This module provides configuration types for FPT Corporation's
//! FPT.AI Text-to-Speech service.
//!
//! # Overview
//!
//! FPT.AI TTS delivers high-quality Vietnamese speech synthesis with
//! regional accent support. It is optimized for:
//! - News reading applications
//! - Voice assistants
//! - IVR systems
//! - Accessibility applications
//!
//! # Features
//!
//! - 7 Vietnamese voices (various accents)
//! - Adjustable speech speed (-3 to +3)
//! - MP3 or WAV audio output
//! - Async synthesis with audio URL callback
//!
//! # Authentication
//!
//! The API uses a single API key header:
//! - `api_key`: Your FPT.AI API key from console.fpt.ai

use crate::core::tts::base::{TTSConfig, TTSError};
use serde::{Deserialize, Serialize};

// =============================================================================
// Constants
// =============================================================================

/// FPT.AI TTS API endpoint.
pub const FPT_TTS_ENDPOINT: &str = "https://api.fpt.ai/hmi/tts/v5";

/// Minimum speech speed.
pub const MIN_SPEED: i8 = -3;

/// Maximum speech speed.
pub const MAX_SPEED: i8 = 3;

/// Default speech speed.
pub const DEFAULT_SPEED: i8 = 0;

/// Default request timeout in seconds.
pub const DEFAULT_REQUEST_TIMEOUT: u64 = 60;

/// Default audio format.
pub const DEFAULT_AUDIO_FORMAT: &str = "mp3";

/// Audio sample rate (Hz) - typical for FPT TTS output.
pub const AUDIO_SAMPLE_RATE: u32 = 16000;

/// Minimum text length.
pub const MIN_TEXT_LENGTH: usize = 3;

/// Maximum text length per request.
pub const MAX_TEXT_LENGTH: usize = 5000;

// =============================================================================
// Voice Types
// =============================================================================

/// FPT.AI TTS voice options.
///
/// FPT.AI TTS supports 7 Vietnamese voices, primarily with Northern accents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FptVoice {
    /// Ban Mai - Default Northern female voice.
    #[default]
    BanMai,
    /// Lan Nhi - Northern female voice.
    LanNhi,
    /// Le Minh - Warm Northern male voice.
    LeMinh,
    /// My An - Female voice.
    MyAn,
    /// Thu Minh - Female voice.
    ThuMinh,
    /// Gia Huy - Male voice.
    GiaHuy,
    /// Linh San - Female voice.
    LinhSan,
}

impl FptVoice {
    /// Get the voice ID string for the API.
    pub fn voice_id(&self) -> &'static str {
        match self {
            Self::BanMai => "banmai",
            Self::LanNhi => "lannhi",
            Self::LeMinh => "leminh",
            Self::MyAn => "myan",
            Self::ThuMinh => "thuminh",
            Self::GiaHuy => "giahuy",
            Self::LinhSan => "linhsan",
        }
    }

    /// Get the display name for the voice.
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::BanMai => "Ban Mai (Northern Female)",
            Self::LanNhi => "Lan Nhi (Northern Female)",
            Self::LeMinh => "Le Minh (Northern Male)",
            Self::MyAn => "My An (Female)",
            Self::ThuMinh => "Thu Minh (Female)",
            Self::GiaHuy => "Gia Huy (Male)",
            Self::LinhSan => "Linh San (Female)",
        }
    }

    /// Get the primary language for the voice.
    pub fn language(&self) -> &'static str {
        "vi-VN"
    }

    /// Get the accent/region for the voice.
    pub fn region(&self) -> &'static str {
        match self {
            Self::BanMai | Self::LanNhi | Self::LeMinh => "Northern",
            _ => "Standard",
        }
    }

    /// Get the gender for the voice.
    pub fn gender(&self) -> &'static str {
        match self {
            Self::LeMinh | Self::GiaHuy => "male",
            _ => "female",
        }
    }

    /// Parse voice from string.
    ///
    /// Accepts various formats:
    /// - Voice IDs: "banmai", "lannhi", "leminh", etc.
    /// - With underscores: "ban_mai", "lan_nhi", "le_minh", etc.
    /// - Display names: "Ban Mai", "Lan Nhi", etc.
    pub fn from_str_relaxed(s: &str) -> Option<Self> {
        let s = s.trim().to_lowercase().replace([' ', '-'], "_");

        match s.as_str() {
            // Voice IDs
            "banmai" | "ban_mai" => Some(Self::BanMai),
            "lannhi" | "lan_nhi" => Some(Self::LanNhi),
            "leminh" | "le_minh" => Some(Self::LeMinh),
            "myan" | "my_an" => Some(Self::MyAn),
            "thuminh" | "thu_minh" => Some(Self::ThuMinh),
            "giahuy" | "gia_huy" => Some(Self::GiaHuy),
            "linhsan" | "linh_san" => Some(Self::LinhSan),

            // Default/fallback
            "default" | "female" | "0" => Some(Self::BanMai),

            _ => None,
        }
    }

    /// Get all available voices.
    pub fn all() -> Vec<Self> {
        vec![
            Self::BanMai,
            Self::LanNhi,
            Self::LeMinh,
            Self::MyAn,
            Self::ThuMinh,
            Self::GiaHuy,
            Self::LinhSan,
        ]
    }
}

// =============================================================================
// Audio Format
// =============================================================================

/// FPT.AI TTS audio output format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FptAudioFormat {
    /// MP3 audio format (default).
    #[default]
    Mp3,
    /// WAV audio format.
    Wav,
}

impl FptAudioFormat {
    /// Get the format string for the API.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Mp3 => "mp3",
            Self::Wav => "wav",
        }
    }

    /// Get MIME type for the format.
    pub fn mime_type(&self) -> &'static str {
        match self {
            Self::Mp3 => "audio/mpeg",
            Self::Wav => "audio/wav",
        }
    }

    /// Parse format from string.
    pub fn from_str_relaxed(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "mp3" | "audio/mpeg" | "mpeg" => Some(Self::Mp3),
            "wav" | "audio/wav" | "wave" => Some(Self::Wav),
            _ => None,
        }
    }
}

// =============================================================================
// Configuration
// =============================================================================

/// FPT.AI TTS configuration.
#[derive(Debug, Clone)]
pub struct FptTtsConfig {
    /// API key for authentication.
    pub api_key: String,

    /// Voice/speaker selection.
    pub voice: FptVoice,

    /// Speech speed (-3 to +3).
    pub speed: i8,

    /// Audio output format.
    pub format: FptAudioFormat,

    /// Request timeout in seconds.
    pub request_timeout_secs: u64,

    /// Optional callback URL for async notification.
    pub callback_url: Option<String>,
}

impl Default for FptTtsConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            voice: FptVoice::default(),
            speed: DEFAULT_SPEED,
            format: FptAudioFormat::default(),
            request_timeout_secs: DEFAULT_REQUEST_TIMEOUT,
            callback_url: None,
        }
    }
}

impl FptTtsConfig {
    /// Create configuration from base TTSConfig.
    pub fn from_base(config: TTSConfig) -> Result<Self, TTSError> {
        let api_key = config.api_key.clone();

        if api_key.is_empty() {
            return Err(TTSError::AuthenticationFailed(
                "FPT.AI API key is required".to_string(),
            ));
        }

        // Parse voice from voice_id field
        let voice = match &config.voice_id {
            Some(id) if !id.is_empty() => FptVoice::from_str_relaxed(id).unwrap_or_else(|| {
                tracing::warn!("Unknown FPT.AI voice '{}', using default (Ban Mai)", id);
                FptVoice::default()
            }),
            _ => FptVoice::default(),
        };

        // Parse speed from speaking_rate config
        // FPT uses -3 to +3, we convert from any float rate
        let speed = config
            .speaking_rate
            .map(|rate| {
                // Map speaking_rate (typically 0.5-2.0) to FPT's -3 to +3 scale
                // 1.0 = 0, 0.5 = -3, 2.0 = +3
                let mapped = ((rate - 1.0) * 6.0).round() as i8;
                mapped.clamp(MIN_SPEED, MAX_SPEED)
            })
            .unwrap_or(DEFAULT_SPEED);

        // Parse audio format
        let format = config
            .audio_format
            .as_ref()
            .and_then(|f| FptAudioFormat::from_str_relaxed(f))
            .unwrap_or_default();

        // Get request timeout
        let request_timeout_secs = config.request_timeout.unwrap_or(DEFAULT_REQUEST_TIMEOUT);

        Ok(Self {
            api_key,
            voice,
            speed,
            format,
            request_timeout_secs,
            callback_url: None,
        })
    }

    /// Build from the standardized TTS config (W1 keystone).
    ///
    /// FPT.AI's only adjustable delivery knob is integer `speed` (-3..=+3), so the standardized
    /// [`speed`] feature maps there — converted from the float multiplier the same way `from_base`
    /// converts `speaking_rate` (1.0 → 0, 0.5 → -3, 2.0 → +3) and clamped to the API range. The
    /// provider-specific `callback_url` is read from the open `extras` passthrough. FPT.AI exposes
    /// no pitch / volume / stability / emotion / instructions / SSML / language / word-timestamp /
    /// seed / sample-rate surface, so those features are skipped.
    ///
    /// [`speed`]: crate::core::tts::standard::TtsFeatures::speed
    pub fn from_standard(
        std: &crate::core::tts::standard::StandardTTSConfig,
    ) -> Result<Self, TTSError> {
        let f = &std.features;
        let mut cfg = Self::from_base(std.base.clone())?;

        if let Some(rate) = f.speed {
            // Map the float multiplier (typically 0.5-2.0) to FPT's -3..=+3 scale,
            // matching `from_base`'s `speaking_rate` conversion.
            let mapped = ((rate - 1.0) * 6.0).round() as i8;
            cfg.speed = mapped.clamp(MIN_SPEED, MAX_SPEED);
        }

        // Provider-specific passthrough.
        if let Some(url) = std.extras.0.get("callback_url").and_then(|v| v.as_str()) {
            cfg.callback_url = Some(url.to_string());
        }

        Ok(cfg)
    }

    /// Validate the configuration.
    pub fn validate(&self) -> Result<(), String> {
        if self.api_key.is_empty() {
            return Err("FPT.AI API key is required".to_string());
        }

        if self.speed < MIN_SPEED || self.speed > MAX_SPEED {
            return Err(format!(
                "Speed must be between {} and {}, got {}",
                MIN_SPEED, MAX_SPEED, self.speed
            ));
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
// Response Types
// =============================================================================

/// FPT.AI TTS API response.
#[derive(Debug, Clone, Deserialize)]
pub struct FptTtsResponse {
    /// Error code (0 = success).
    #[serde(default)]
    pub error: i32,

    /// Async audio URL.
    #[serde(default, rename = "async")]
    pub async_url: String,

    /// Request ID for tracking.
    #[serde(default)]
    pub request_id: String,

    /// Response message.
    #[serde(default)]
    pub message: String,
}

impl FptTtsResponse {
    /// Check if the response indicates success.
    pub fn is_success(&self) -> bool {
        self.error == 0 && !self.async_url.is_empty()
    }

    /// Get the audio URL if successful.
    pub fn audio_url(&self) -> Option<&str> {
        if self.is_success() {
            Some(self.async_url.as_str())
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

    // W1 keystone (TTS): the standardized `speed` feature reaches FPT's integer `speed` field
    // (via the same `speaking_rate` conversion `from_base` uses), and the provider-specific
    // `callback_url` flows through the open extras passthrough.
    #[test]
    fn from_standard_maps_speed_and_callback_url() {
        use crate::core::tts::standard::{ProviderExtras, StandardTTSConfig, TtsFeatures};

        let mut extras = serde_json::Map::new();
        extras.insert(
            "callback_url".into(),
            serde_json::json!("https://example.com/cb"),
        );
        let std = StandardTTSConfig {
            base: TTSConfig {
                provider: "fpt_ai".into(),
                api_key: "k".into(),
                ..Default::default()
            },
            features: TtsFeatures {
                speed: Some(1.5), // 1.5 -> +3 on FPT's -3..=+3 scale
                ssml: Some(true), // capability gap: FPT has no SSML, must be ignored
                sample_rate: Some(48000), // capability gap: FPT rate is fixed, must be ignored
                ..Default::default()
            },
            extras: ProviderExtras(extras),
        };

        let cfg = FptTtsConfig::from_standard(&std).unwrap();
        assert_eq!(cfg.speed, 3); // mapped feature reached the speed field
        assert_eq!(cfg.callback_url, Some("https://example.com/cb".to_string())); // extras passthrough
    }

    #[test]
    fn test_voice_ids() {
        assert_eq!(FptVoice::BanMai.voice_id(), "banmai");
        assert_eq!(FptVoice::LanNhi.voice_id(), "lannhi");
        assert_eq!(FptVoice::LeMinh.voice_id(), "leminh");
        assert_eq!(FptVoice::MyAn.voice_id(), "myan");
        assert_eq!(FptVoice::ThuMinh.voice_id(), "thuminh");
        assert_eq!(FptVoice::GiaHuy.voice_id(), "giahuy");
        assert_eq!(FptVoice::LinhSan.voice_id(), "linhsan");
    }

    #[test]
    fn test_voice_display_names() {
        assert!(FptVoice::BanMai.display_name().contains("Ban Mai"));
        assert!(FptVoice::LeMinh.display_name().contains("Male"));
        assert!(FptVoice::LanNhi.display_name().contains("Female"));
    }

    #[test]
    fn test_voice_from_str_ids() {
        assert_eq!(FptVoice::from_str_relaxed("banmai"), Some(FptVoice::BanMai));
        assert_eq!(FptVoice::from_str_relaxed("lannhi"), Some(FptVoice::LanNhi));
        assert_eq!(FptVoice::from_str_relaxed("leminh"), Some(FptVoice::LeMinh));
        assert_eq!(FptVoice::from_str_relaxed("myan"), Some(FptVoice::MyAn));
        assert_eq!(
            FptVoice::from_str_relaxed("thuminh"),
            Some(FptVoice::ThuMinh)
        );
        assert_eq!(FptVoice::from_str_relaxed("giahuy"), Some(FptVoice::GiaHuy));
        assert_eq!(
            FptVoice::from_str_relaxed("linhsan"),
            Some(FptVoice::LinhSan)
        );
    }

    #[test]
    fn test_voice_from_str_underscores() {
        assert_eq!(
            FptVoice::from_str_relaxed("ban_mai"),
            Some(FptVoice::BanMai)
        );
        assert_eq!(
            FptVoice::from_str_relaxed("lan_nhi"),
            Some(FptVoice::LanNhi)
        );
        assert_eq!(
            FptVoice::from_str_relaxed("le_minh"),
            Some(FptVoice::LeMinh)
        );
    }

    #[test]
    fn test_voice_from_str_case_insensitive() {
        assert_eq!(FptVoice::from_str_relaxed("BANMAI"), Some(FptVoice::BanMai));
        assert_eq!(FptVoice::from_str_relaxed("BanMai"), Some(FptVoice::BanMai));
        assert_eq!(
            FptVoice::from_str_relaxed("Ban Mai"),
            Some(FptVoice::BanMai)
        );
    }

    #[test]
    fn test_voice_from_str_invalid() {
        assert_eq!(FptVoice::from_str_relaxed("invalid"), None);
        assert_eq!(FptVoice::from_str_relaxed(""), None);
    }

    #[test]
    fn test_voice_from_str_default() {
        assert_eq!(
            FptVoice::from_str_relaxed("default"),
            Some(FptVoice::BanMai)
        );
        assert_eq!(FptVoice::from_str_relaxed("female"), Some(FptVoice::BanMai));
    }

    #[test]
    fn test_all_voices() {
        let voices = FptVoice::all();
        assert_eq!(voices.len(), 7);
        assert!(voices.contains(&FptVoice::BanMai));
        assert!(voices.contains(&FptVoice::LanNhi));
        assert!(voices.contains(&FptVoice::LeMinh));
        assert!(voices.contains(&FptVoice::MyAn));
        assert!(voices.contains(&FptVoice::ThuMinh));
        assert!(voices.contains(&FptVoice::GiaHuy));
        assert!(voices.contains(&FptVoice::LinhSan));
    }

    #[test]
    fn test_voice_language() {
        for voice in FptVoice::all() {
            assert_eq!(voice.language(), "vi-VN");
        }
    }

    #[test]
    fn test_voice_gender() {
        assert_eq!(FptVoice::BanMai.gender(), "female");
        assert_eq!(FptVoice::LanNhi.gender(), "female");
        assert_eq!(FptVoice::LeMinh.gender(), "male");
        assert_eq!(FptVoice::MyAn.gender(), "female");
        assert_eq!(FptVoice::ThuMinh.gender(), "female");
        assert_eq!(FptVoice::GiaHuy.gender(), "male");
        assert_eq!(FptVoice::LinhSan.gender(), "female");
    }

    #[test]
    fn test_voice_region() {
        assert_eq!(FptVoice::BanMai.region(), "Northern");
        assert_eq!(FptVoice::LanNhi.region(), "Northern");
        assert_eq!(FptVoice::LeMinh.region(), "Northern");
        assert_eq!(FptVoice::MyAn.region(), "Standard");
        assert_eq!(FptVoice::ThuMinh.region(), "Standard");
        assert_eq!(FptVoice::GiaHuy.region(), "Standard");
        assert_eq!(FptVoice::LinhSan.region(), "Standard");
    }

    #[test]
    fn test_audio_format_as_str() {
        assert_eq!(FptAudioFormat::Mp3.as_str(), "mp3");
        assert_eq!(FptAudioFormat::Wav.as_str(), "wav");
    }

    #[test]
    fn test_audio_format_mime_type() {
        assert_eq!(FptAudioFormat::Mp3.mime_type(), "audio/mpeg");
        assert_eq!(FptAudioFormat::Wav.mime_type(), "audio/wav");
    }

    #[test]
    fn test_audio_format_from_str() {
        assert_eq!(
            FptAudioFormat::from_str_relaxed("mp3"),
            Some(FptAudioFormat::Mp3)
        );
        assert_eq!(
            FptAudioFormat::from_str_relaxed("wav"),
            Some(FptAudioFormat::Wav)
        );
        assert_eq!(
            FptAudioFormat::from_str_relaxed("audio/mpeg"),
            Some(FptAudioFormat::Mp3)
        );
        assert_eq!(
            FptAudioFormat::from_str_relaxed("audio/wav"),
            Some(FptAudioFormat::Wav)
        );
        assert_eq!(FptAudioFormat::from_str_relaxed("invalid"), None);
    }

    #[test]
    fn test_config_default() {
        let config = FptTtsConfig::default();
        assert!(config.api_key.is_empty());
        assert_eq!(config.voice, FptVoice::BanMai);
        assert_eq!(config.speed, DEFAULT_SPEED);
        assert_eq!(config.format, FptAudioFormat::Mp3);
    }

    #[test]
    fn test_config_validation_empty_key() {
        let config = FptTtsConfig::default();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_invalid_speed() {
        let mut config = FptTtsConfig::default();
        config.api_key = "test_key".to_string();
        config.speed = 5; // Above MAX_SPEED

        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_success() {
        let mut config = FptTtsConfig::default();
        config.api_key = "test_key".to_string();

        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_from_base_empty_key() {
        let config = TTSConfig {
            api_key: String::new(),
            ..Default::default()
        };

        let result = FptTtsConfig::from_base(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_config_from_base_with_voice() {
        let config = TTSConfig {
            api_key: "test_key".to_string(),
            voice_id: Some("leminh".to_string()),
            ..Default::default()
        };

        let fpt_config = FptTtsConfig::from_base(config).unwrap();
        assert_eq!(fpt_config.voice, FptVoice::LeMinh);
    }

    #[test]
    fn test_config_from_base_with_speed() {
        // Test normal speed (1.0 = 0)
        let config = TTSConfig {
            api_key: "test_key".to_string(),
            speaking_rate: Some(1.0),
            ..Default::default()
        };
        let fpt_config = FptTtsConfig::from_base(config).unwrap();
        assert_eq!(fpt_config.speed, 0);

        // Test fast speed (1.5 = +3)
        let config = TTSConfig {
            api_key: "test_key".to_string(),
            speaking_rate: Some(1.5),
            ..Default::default()
        };
        let fpt_config = FptTtsConfig::from_base(config).unwrap();
        assert_eq!(fpt_config.speed, 3);

        // Test slow speed (0.5 = -3)
        let config = TTSConfig {
            api_key: "test_key".to_string(),
            speaking_rate: Some(0.5),
            ..Default::default()
        };
        let fpt_config = FptTtsConfig::from_base(config).unwrap();
        assert_eq!(fpt_config.speed, -3);
    }

    #[test]
    fn test_config_from_base_with_format() {
        let config = TTSConfig {
            api_key: "test_key".to_string(),
            audio_format: Some("wav".to_string()),
            ..Default::default()
        };

        let fpt_config = FptTtsConfig::from_base(config).unwrap();
        assert_eq!(fpt_config.format, FptAudioFormat::Wav);
    }

    #[test]
    fn test_validate_text_too_short() {
        let result = FptTtsConfig::validate_text("ab");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_text_too_long() {
        let long_text = "a".repeat(MAX_TEXT_LENGTH + 1);
        let result = FptTtsConfig::validate_text(&long_text);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_text_valid() {
        let result = FptTtsConfig::validate_text("Hello world");
        assert!(result.is_ok());
    }

    #[test]
    fn test_response_success() {
        let response = FptTtsResponse {
            error: 0,
            async_url: "https://file.fpt.ai/audio.mp3".to_string(),
            request_id: "test-id".to_string(),
            message: "Success".to_string(),
        };

        assert!(response.is_success());
        assert_eq!(response.audio_url(), Some("https://file.fpt.ai/audio.mp3"));
    }

    #[test]
    fn test_response_error() {
        let response = FptTtsResponse {
            error: 1,
            async_url: String::new(),
            request_id: "test-id".to_string(),
            message: "Error".to_string(),
        };

        assert!(!response.is_success());
        assert_eq!(response.audio_url(), None);
    }

    #[test]
    fn test_response_empty_url() {
        let response = FptTtsResponse {
            error: 0,
            async_url: String::new(),
            request_id: "test-id".to_string(),
            message: "Success".to_string(),
        };

        assert!(!response.is_success());
        assert_eq!(response.audio_url(), None);
    }
}
