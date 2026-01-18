//! Baidu AI Cloud TTS Configuration
//!
//! Configuration types and constants for Baidu AI Cloud TTS providers.
//!
//! # Overview
//!
//! Baidu AI Cloud Speech (百度语音) provides Text-to-Speech services via REST API.
//! Authentication uses OAuth 2.0 client credentials flow with access token caching.
//!
//! # API Endpoint
//!
//! - REST: `http://tsn.baidu.com/text2audio` or `https://tsn.baidu.com/text2audio`
//!
//! # Authentication
//!
//! The API key format is `api_key|secret_key` (pipe-separated).
//! An OAuth 2.0 access token is obtained from the token endpoint and cached.

use crate::core::tts::base::{TTSConfig, TTSError};
use serde::{Deserialize, Serialize};

// =============================================================================
// Constants
// =============================================================================

/// Baidu TTS REST endpoint (HTTP).
pub const BAIDU_TTS_URL: &str = "http://tsn.baidu.com/text2audio";

/// Baidu TTS REST endpoint (HTTPS).
pub const BAIDU_TTS_URL_HTTPS: &str = "https://tsn.baidu.com/text2audio";

/// OAuth 2.0 token endpoint.
pub const BAIDU_OAUTH_URL: &str = "https://aip.baidubce.com/oauth/2.0/token";

/// Default voice ID (度小美 - female voice).
pub const DEFAULT_VOICE: u32 = 0;

/// Default speed (0-15 scale).
pub const DEFAULT_SPEED: u8 = 5;

/// Default pitch (0-15 scale).
pub const DEFAULT_PITCH: u8 = 5;

/// Default volume (0-15 scale).
pub const DEFAULT_VOLUME: u8 = 5;

/// Default audio format (MP3).
pub const DEFAULT_AUDIO_FORMAT: BaiduTtsAudioFormat = BaiduTtsAudioFormat::Mp3;

/// Maximum text length in GBK bytes.
pub const MAX_TEXT_LENGTH_GBK: usize = 1024;

// =============================================================================
// Voice Categories
// =============================================================================

/// Baidu TTS voice categories.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BaiduVoiceCategory {
    /// Basic library voices.
    #[default]
    Basic,
    /// Premium library voices.
    Premium,
    /// Premium+ library voices.
    PremiumPlus,
    /// Large model library voices.
    LargeModel,
}

impl BaiduVoiceCategory {
    /// Get all voice IDs for this category.
    pub fn voice_ids(&self) -> &'static [u32] {
        match self {
            BaiduVoiceCategory::Basic => &[0, 1, 3, 4],
            BaiduVoiceCategory::Premium => &[5003, 5118, 106, 110, 111, 103, 5],
            BaiduVoiceCategory::PremiumPlus => &[
                4003, 4106, 4115, 4119, 4105, 4117, 4100, 4103, 4144, 4278, 4143, 4140, 4129, 4149,
                4254, 4206, 4226,
            ],
            BaiduVoiceCategory::LargeModel => &[
                4189, 4194, 4193, 4195, 4196, 4197, 20100, 20101, 4257, 4132, 4139, 5977, 4007,
                4150, 4134, 4172,
            ],
        }
    }

    /// Get human-readable name.
    pub fn name(&self) -> &'static str {
        match self {
            BaiduVoiceCategory::Basic => "Basic",
            BaiduVoiceCategory::Premium => "Premium",
            BaiduVoiceCategory::PremiumPlus => "Premium+",
            BaiduVoiceCategory::LargeModel => "Large Model",
        }
    }
}

// =============================================================================
// Voice Enum
// =============================================================================

/// Baidu TTS voice options.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BaiduTtsVoice {
    // Basic Library
    /// 度小美 - Female voice (ID: 0).
    #[default]
    DuXiaomei,
    /// 度小宇 - Male voice (ID: 1).
    DuXiaoyu,
    /// 度逍遥 - Male voice basic (ID: 3).
    DuXiaoyao,
    /// 度丫丫 - Child voice (ID: 4).
    DuYaya,

    // Custom voice by ID
    /// Custom voice by numeric ID.
    Custom(u32),
}

impl BaiduTtsVoice {
    /// Get the voice ID for API request.
    pub fn as_id(&self) -> u32 {
        match self {
            BaiduTtsVoice::DuXiaomei => 0,
            BaiduTtsVoice::DuXiaoyu => 1,
            BaiduTtsVoice::DuXiaoyao => 3,
            BaiduTtsVoice::DuYaya => 4,
            BaiduTtsVoice::Custom(id) => *id,
        }
    }

    /// Get the voice name.
    pub fn name(&self) -> &str {
        match self {
            BaiduTtsVoice::DuXiaomei => "度小美 (Female)",
            BaiduTtsVoice::DuXiaoyu => "度小宇 (Male)",
            BaiduTtsVoice::DuXiaoyao => "度逍遥 (Male Basic)",
            BaiduTtsVoice::DuYaya => "度丫丫 (Child)",
            BaiduTtsVoice::Custom(_) => "Custom Voice",
        }
    }

    /// Parse from string or numeric ID.
    pub fn from_str_or_id(s: &str) -> Self {
        // Try to parse as numeric ID first
        if let Ok(id) = s.parse::<u32>() {
            return match id {
                0 => BaiduTtsVoice::DuXiaomei,
                1 => BaiduTtsVoice::DuXiaoyu,
                3 => BaiduTtsVoice::DuXiaoyao,
                4 => BaiduTtsVoice::DuYaya,
                _ => BaiduTtsVoice::Custom(id),
            };
        }

        // Try to parse as voice name
        match s.to_lowercase().as_str() {
            "duxiaomei" | "du_xiaomei" | "du-xiaomei" | "xiaomei" | "female" => {
                BaiduTtsVoice::DuXiaomei
            }
            "duxiaoyu" | "du_xiaoyu" | "du-xiaoyu" | "xiaoyu" | "male" => BaiduTtsVoice::DuXiaoyu,
            "duxiaoyao" | "du_xiaoyao" | "du-xiaoyao" | "xiaoyao" => BaiduTtsVoice::DuXiaoyao,
            "duyaya" | "du_yaya" | "du-yaya" | "yaya" | "child" => BaiduTtsVoice::DuYaya,
            _ => BaiduTtsVoice::DuXiaomei,
        }
    }

    /// Get the category of this voice.
    pub fn category(&self) -> BaiduVoiceCategory {
        let id = self.as_id();
        if [0, 1, 3, 4].contains(&id) {
            BaiduVoiceCategory::Basic
        } else if [5003, 5118, 106, 110, 111, 103, 5].contains(&id) {
            BaiduVoiceCategory::Premium
        } else if [
            4003, 4106, 4115, 4119, 4105, 4117, 4100, 4103, 4144, 4278, 4143, 4140, 4129, 4149,
            4254, 4206, 4226,
        ]
        .contains(&id)
        {
            BaiduVoiceCategory::PremiumPlus
        } else {
            BaiduVoiceCategory::LargeModel
        }
    }
}

// =============================================================================
// Audio Format Enum
// =============================================================================

/// Baidu TTS audio output format.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BaiduTtsAudioFormat {
    /// MP3 format (aue=3).
    #[default]
    Mp3,
    /// PCM 16kHz format (aue=4).
    Pcm16k,
    /// PCM 8kHz format (aue=5).
    Pcm8k,
    /// WAV format (aue=6).
    Wav,
}

impl BaiduTtsAudioFormat {
    /// Get the aue parameter value for API request.
    pub fn as_aue(&self) -> u8 {
        match self {
            BaiduTtsAudioFormat::Mp3 => 3,
            BaiduTtsAudioFormat::Pcm16k => 4,
            BaiduTtsAudioFormat::Pcm8k => 5,
            BaiduTtsAudioFormat::Wav => 6,
        }
    }

    /// Get the format string for AudioData.
    pub fn as_format_str(&self) -> &'static str {
        match self {
            BaiduTtsAudioFormat::Mp3 => "mp3",
            BaiduTtsAudioFormat::Pcm16k | BaiduTtsAudioFormat::Pcm8k => "pcm",
            BaiduTtsAudioFormat::Wav => "wav",
        }
    }

    /// Get the sample rate for this format.
    pub fn sample_rate(&self) -> u32 {
        match self {
            BaiduTtsAudioFormat::Mp3 | BaiduTtsAudioFormat::Pcm16k | BaiduTtsAudioFormat::Wav => {
                16000
            }
            BaiduTtsAudioFormat::Pcm8k => 8000,
        }
    }

    /// Get the MIME type for this format.
    pub fn mime_type(&self) -> &'static str {
        match self {
            BaiduTtsAudioFormat::Mp3 => "audio/mp3",
            BaiduTtsAudioFormat::Pcm16k | BaiduTtsAudioFormat::Pcm8k => "audio/pcm",
            BaiduTtsAudioFormat::Wav => "audio/wav",
        }
    }

    /// Parse from string.
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "mp3" => Some(BaiduTtsAudioFormat::Mp3),
            "pcm" | "pcm16" | "pcm16k" | "linear16" => Some(BaiduTtsAudioFormat::Pcm16k),
            "pcm8" | "pcm8k" => Some(BaiduTtsAudioFormat::Pcm8k),
            "wav" => Some(BaiduTtsAudioFormat::Wav),
            _ => None,
        }
    }
}

// =============================================================================
// OAuth Response
// =============================================================================

/// OAuth 2.0 token response from Baidu.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BaiduOAuthResponse {
    /// Access token.
    pub access_token: String,
    /// Token expiration time in seconds.
    pub expires_in: u64,
    /// Refresh token (if provided).
    pub refresh_token: Option<String>,
    /// Session key (if provided).
    pub session_key: Option<String>,
    /// Token scope.
    pub scope: Option<String>,
}

/// OAuth error response from Baidu.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BaiduOAuthError {
    /// Error code.
    pub error: String,
    /// Error description.
    pub error_description: Option<String>,
}

// =============================================================================
// TTS Error Response
// =============================================================================

/// TTS error response from Baidu API.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BaiduTtsErrorResponse {
    /// Error code.
    pub err_no: i32,
    /// Error message.
    pub err_msg: String,
    /// Serial number.
    pub sn: Option<String>,
    /// Index.
    pub idx: Option<i32>,
}

impl BaiduTtsErrorResponse {
    /// Get a human-readable error description.
    pub fn description(&self) -> &'static str {
        match self.err_no {
            500 => "Unknown error",
            501 => "Parameter error",
            502 => "Token validation failed",
            503 => "Quota exceeded",
            _ => "Unknown error",
        }
    }
}

// =============================================================================
// TTS Configuration
// =============================================================================

/// Configuration for Baidu AI Cloud TTS.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BaiduTtsConfig {
    /// API Key from Baidu console.
    pub api_key: String,

    /// Secret Key from Baidu console.
    pub secret_key: String,

    /// Voice to use.
    #[serde(default)]
    pub voice: BaiduTtsVoice,

    /// Audio output format.
    #[serde(default)]
    pub audio_format: BaiduTtsAudioFormat,

    /// Speech speed (0-15, default 5).
    #[serde(default = "default_speed")]
    pub speed: u8,

    /// Speech pitch (0-15, default 5).
    #[serde(default = "default_pitch")]
    pub pitch: u8,

    /// Speech volume (0-15, default 5).
    #[serde(default = "default_volume")]
    pub volume: u8,

    /// Client unique identifier (cuid).
    #[serde(default = "default_cuid")]
    pub cuid: String,

    /// Use HTTPS endpoint.
    #[serde(default = "default_use_https")]
    pub use_https: bool,
}

fn default_speed() -> u8 {
    DEFAULT_SPEED
}

fn default_pitch() -> u8 {
    DEFAULT_PITCH
}

fn default_volume() -> u8 {
    DEFAULT_VOLUME
}

fn default_cuid() -> String {
    format!("waav_gateway_{}", uuid::Uuid::new_v4().to_string().replace('-', "")[..16].to_string())
}

fn default_use_https() -> bool {
    true
}

impl Default for BaiduTtsConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            secret_key: String::new(),
            voice: BaiduTtsVoice::default(),
            audio_format: BaiduTtsAudioFormat::default(),
            speed: DEFAULT_SPEED,
            pitch: DEFAULT_PITCH,
            volume: DEFAULT_VOLUME,
            cuid: default_cuid(),
            use_https: true,
        }
    }
}

impl BaiduTtsConfig {
    /// Validate the configuration.
    pub fn validate(&self) -> Result<(), TTSError> {
        if self.api_key.is_empty() {
            return Err(TTSError::InvalidConfiguration(
                "Baidu API Key is required".to_string(),
            ));
        }

        if self.secret_key.is_empty() {
            return Err(TTSError::InvalidConfiguration(
                "Baidu Secret Key is required".to_string(),
            ));
        }

        if self.speed > 15 {
            return Err(TTSError::InvalidConfiguration(
                "Speed must be between 0 and 15".to_string(),
            ));
        }

        if self.pitch > 15 {
            return Err(TTSError::InvalidConfiguration(
                "Pitch must be between 0 and 15".to_string(),
            ));
        }

        if self.volume > 15 {
            return Err(TTSError::InvalidConfiguration(
                "Volume must be between 0 and 15".to_string(),
            ));
        }

        if self.cuid.is_empty() || self.cuid.len() > 60 {
            return Err(TTSError::InvalidConfiguration(
                "CUID must be between 1 and 60 characters".to_string(),
            ));
        }

        Ok(())
    }

    /// Get the TTS endpoint URL.
    pub fn get_endpoint_url(&self) -> &'static str {
        if self.use_https {
            BAIDU_TTS_URL_HTTPS
        } else {
            BAIDU_TTS_URL
        }
    }

    /// Get the sample rate based on audio format.
    pub fn get_sample_rate(&self) -> u32 {
        self.audio_format.sample_rate()
    }

    /// Convert from base TTSConfig.
    ///
    /// The API key should be in the format `api_key|secret_key` (pipe-separated).
    pub fn from_base(config: TTSConfig) -> Result<Self, TTSError> {
        if config.api_key.is_empty() {
            return Err(TTSError::InvalidConfiguration(
                "Baidu API Key is required (format: api_key|secret_key)".to_string(),
            ));
        }

        // Parse api_key|secret_key format
        let parts: Vec<&str> = config.api_key.split('|').collect();
        let (api_key, secret_key) = if parts.len() == 2 {
            (parts[0].to_string(), parts[1].to_string())
        } else {
            return Err(TTSError::InvalidConfiguration(
                "Baidu API Key must be in format: api_key|secret_key".to_string(),
            ));
        };

        // Parse voice from voice_id
        let voice = config
            .voice_id
            .as_ref()
            .map(|v| BaiduTtsVoice::from_str_or_id(v))
            .unwrap_or_default();

        // Parse audio format
        let audio_format = config
            .audio_format
            .as_ref()
            .and_then(|f| BaiduTtsAudioFormat::from_str(f))
            .unwrap_or_default();

        // Map speaking_rate from 0.25-4.0 to 0-15
        let speed = config
            .speaking_rate
            .map(|rate| {
                // Map rate: 0.25 -> 0, 1.0 -> 5, 4.0 -> 15
                let mapped = ((rate - 0.25) / (4.0 - 0.25) * 15.0).round() as u8;
                mapped.min(15)
            })
            .unwrap_or(DEFAULT_SPEED);

        Ok(Self {
            api_key,
            secret_key,
            voice,
            audio_format,
            speed,
            pitch: DEFAULT_PITCH,
            volume: DEFAULT_VOLUME,
            cuid: default_cuid(),
            use_https: true,
        })
    }

    /// Get list of supported voices.
    pub fn supported_voices() -> Vec<(&'static str, u32, &'static str)> {
        vec![
            // Basic Library
            ("度小美 (Female)", 0, "basic"),
            ("度小宇 (Male)", 1, "basic"),
            ("度逍遥 (Male Basic)", 3, "basic"),
            ("度丫丫 (Child)", 4, "basic"),
            // Premium Library (sample)
            ("Premium Voice 5003", 5003, "premium"),
            ("Premium Voice 5118", 5118, "premium"),
            // Premium+ Library (sample)
            ("Premium+ Voice 4003", 4003, "premium_plus"),
            ("Premium+ Voice 4106", 4106, "premium_plus"),
            // Large Model Library (sample)
            ("AI Voice 4189", 4189, "large_model"),
            ("AI Voice 4194", 4194, "large_model"),
        ]
    }

    /// Get list of supported audio formats.
    pub fn supported_formats() -> Vec<&'static str> {
        vec!["mp3", "pcm16k", "pcm8k", "wav"]
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = BaiduTtsConfig::default();
        assert!(config.api_key.is_empty());
        assert!(config.secret_key.is_empty());
        assert_eq!(config.speed, DEFAULT_SPEED);
        assert_eq!(config.pitch, DEFAULT_PITCH);
        assert_eq!(config.volume, DEFAULT_VOLUME);
        assert!(config.use_https);
    }

    #[test]
    fn test_config_validation_valid() {
        let mut config = BaiduTtsConfig::default();
        config.api_key = "test_api_key".to_string();
        config.secret_key = "test_secret_key".to_string();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validation_empty_api_key() {
        let config = BaiduTtsConfig::default();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_empty_secret_key() {
        let mut config = BaiduTtsConfig::default();
        config.api_key = "test_api_key".to_string();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_invalid_speed() {
        let mut config = BaiduTtsConfig::default();
        config.api_key = "test_api_key".to_string();
        config.secret_key = "test_secret_key".to_string();
        config.speed = 20;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_invalid_pitch() {
        let mut config = BaiduTtsConfig::default();
        config.api_key = "test_api_key".to_string();
        config.secret_key = "test_secret_key".to_string();
        config.pitch = 20;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_invalid_volume() {
        let mut config = BaiduTtsConfig::default();
        config.api_key = "test_api_key".to_string();
        config.secret_key = "test_secret_key".to_string();
        config.volume = 20;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_voice_from_str_numeric() {
        assert_eq!(BaiduTtsVoice::from_str_or_id("0"), BaiduTtsVoice::DuXiaomei);
        assert_eq!(BaiduTtsVoice::from_str_or_id("1"), BaiduTtsVoice::DuXiaoyu);
        assert_eq!(BaiduTtsVoice::from_str_or_id("3"), BaiduTtsVoice::DuXiaoyao);
        assert_eq!(BaiduTtsVoice::from_str_or_id("4"), BaiduTtsVoice::DuYaya);
        assert_eq!(
            BaiduTtsVoice::from_str_or_id("5003"),
            BaiduTtsVoice::Custom(5003)
        );
    }

    #[test]
    fn test_voice_from_str_name() {
        assert_eq!(
            BaiduTtsVoice::from_str_or_id("duxiaomei"),
            BaiduTtsVoice::DuXiaomei
        );
        assert_eq!(
            BaiduTtsVoice::from_str_or_id("female"),
            BaiduTtsVoice::DuXiaomei
        );
        assert_eq!(
            BaiduTtsVoice::from_str_or_id("male"),
            BaiduTtsVoice::DuXiaoyu
        );
        assert_eq!(
            BaiduTtsVoice::from_str_or_id("child"),
            BaiduTtsVoice::DuYaya
        );
    }

    #[test]
    fn test_voice_as_id() {
        assert_eq!(BaiduTtsVoice::DuXiaomei.as_id(), 0);
        assert_eq!(BaiduTtsVoice::DuXiaoyu.as_id(), 1);
        assert_eq!(BaiduTtsVoice::DuXiaoyao.as_id(), 3);
        assert_eq!(BaiduTtsVoice::DuYaya.as_id(), 4);
        assert_eq!(BaiduTtsVoice::Custom(5003).as_id(), 5003);
    }

    #[test]
    fn test_voice_category() {
        assert_eq!(BaiduTtsVoice::DuXiaomei.category(), BaiduVoiceCategory::Basic);
        assert_eq!(
            BaiduTtsVoice::Custom(5003).category(),
            BaiduVoiceCategory::Premium
        );
        assert_eq!(
            BaiduTtsVoice::Custom(4003).category(),
            BaiduVoiceCategory::PremiumPlus
        );
        assert_eq!(
            BaiduTtsVoice::Custom(4189).category(),
            BaiduVoiceCategory::LargeModel
        );
    }

    #[test]
    fn test_audio_format_aue() {
        assert_eq!(BaiduTtsAudioFormat::Mp3.as_aue(), 3);
        assert_eq!(BaiduTtsAudioFormat::Pcm16k.as_aue(), 4);
        assert_eq!(BaiduTtsAudioFormat::Pcm8k.as_aue(), 5);
        assert_eq!(BaiduTtsAudioFormat::Wav.as_aue(), 6);
    }

    #[test]
    fn test_audio_format_sample_rate() {
        assert_eq!(BaiduTtsAudioFormat::Mp3.sample_rate(), 16000);
        assert_eq!(BaiduTtsAudioFormat::Pcm16k.sample_rate(), 16000);
        assert_eq!(BaiduTtsAudioFormat::Pcm8k.sample_rate(), 8000);
        assert_eq!(BaiduTtsAudioFormat::Wav.sample_rate(), 16000);
    }

    #[test]
    fn test_audio_format_from_str() {
        assert_eq!(
            BaiduTtsAudioFormat::from_str("mp3"),
            Some(BaiduTtsAudioFormat::Mp3)
        );
        assert_eq!(
            BaiduTtsAudioFormat::from_str("pcm"),
            Some(BaiduTtsAudioFormat::Pcm16k)
        );
        assert_eq!(
            BaiduTtsAudioFormat::from_str("pcm8k"),
            Some(BaiduTtsAudioFormat::Pcm8k)
        );
        assert_eq!(
            BaiduTtsAudioFormat::from_str("wav"),
            Some(BaiduTtsAudioFormat::Wav)
        );
        assert_eq!(BaiduTtsAudioFormat::from_str("invalid"), None);
    }

    #[test]
    fn test_endpoint_url() {
        let mut config = BaiduTtsConfig::default();
        config.use_https = true;
        assert_eq!(config.get_endpoint_url(), BAIDU_TTS_URL_HTTPS);

        config.use_https = false;
        assert_eq!(config.get_endpoint_url(), BAIDU_TTS_URL);
    }

    #[test]
    fn test_from_base_config() {
        let base = TTSConfig {
            api_key: "test_api_key|test_secret_key".to_string(),
            voice_id: Some("1".to_string()),
            audio_format: Some("wav".to_string()),
            speaking_rate: Some(1.0),
            ..Default::default()
        };

        let config = BaiduTtsConfig::from_base(base).unwrap();
        assert_eq!(config.api_key, "test_api_key");
        assert_eq!(config.secret_key, "test_secret_key");
        assert_eq!(config.voice, BaiduTtsVoice::DuXiaoyu);
        assert_eq!(config.audio_format, BaiduTtsAudioFormat::Wav);
    }

    #[test]
    fn test_from_base_config_invalid_format() {
        let base = TTSConfig {
            api_key: "missing_pipe_separator".to_string(),
            ..Default::default()
        };

        let result = BaiduTtsConfig::from_base(base);
        assert!(result.is_err());
    }

    #[test]
    fn test_from_base_config_empty_api_key() {
        let base = TTSConfig {
            api_key: "".to_string(),
            ..Default::default()
        };

        let result = BaiduTtsConfig::from_base(base);
        assert!(result.is_err());
    }

    #[test]
    fn test_supported_voices() {
        let voices = BaiduTtsConfig::supported_voices();
        assert!(!voices.is_empty());
        assert!(voices.iter().any(|(_, id, _)| *id == 0)); // DuXiaomei
        assert!(voices.iter().any(|(_, id, _)| *id == 1)); // DuXiaoyu
    }

    #[test]
    fn test_supported_formats() {
        let formats = BaiduTtsConfig::supported_formats();
        assert!(formats.contains(&"mp3"));
        assert!(formats.contains(&"wav"));
        assert!(formats.contains(&"pcm16k"));
        assert!(formats.contains(&"pcm8k"));
    }

    #[test]
    fn test_oauth_response_deserialization() {
        let json = r#"{
            "access_token": "test_token",
            "expires_in": 2592000,
            "refresh_token": "refresh",
            "scope": "public"
        }"#;

        let response: BaiduOAuthResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.access_token, "test_token");
        assert_eq!(response.expires_in, 2592000);
    }

    #[test]
    fn test_error_response_deserialization() {
        let json = r#"{
            "err_no": 502,
            "err_msg": "Token validation failed",
            "sn": "12345"
        }"#;

        let response: BaiduTtsErrorResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.err_no, 502);
        assert_eq!(response.description(), "Token validation failed");
    }

    #[test]
    fn test_voice_category_ids() {
        let basic = BaiduVoiceCategory::Basic.voice_ids();
        assert!(basic.contains(&0));
        assert!(basic.contains(&1));
        assert!(basic.contains(&3));
        assert!(basic.contains(&4));

        let premium = BaiduVoiceCategory::Premium.voice_ids();
        assert!(premium.contains(&5003));
    }

    #[test]
    fn test_speed_mapping_from_base() {
        // Test edge cases of speaking_rate mapping
        let base = TTSConfig {
            api_key: "api|secret".to_string(),
            speaking_rate: Some(0.25), // Minimum
            ..Default::default()
        };
        let config = BaiduTtsConfig::from_base(base).unwrap();
        assert_eq!(config.speed, 0);

        let base = TTSConfig {
            api_key: "api|secret".to_string(),
            speaking_rate: Some(1.0), // Normal
            ..Default::default()
        };
        let config = BaiduTtsConfig::from_base(base).unwrap();
        assert_eq!(config.speed, 3); // Approximately maps to middle range

        let base = TTSConfig {
            api_key: "api|secret".to_string(),
            speaking_rate: Some(4.0), // Maximum
            ..Default::default()
        };
        let config = BaiduTtsConfig::from_base(base).unwrap();
        assert_eq!(config.speed, 15);
    }
}
