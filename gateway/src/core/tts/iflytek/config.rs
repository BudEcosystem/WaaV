//! iFlytek TTS Configuration Module
//!
//! This module provides configuration types for the iFlytek TTS provider,
//! including voice selection, audio encoding, and API parameters.
//!
//! # Supported Languages
//!
//! iFlytek TTS supports 15+ languages including:
//! - Chinese, English, Japanese
//! - Indonesian, Russian, French
//! - German, Arabic, Vietnamese
//! - Thai, Korean, Portuguese
//! - Malay, Hindi, Urdu
//!
//! # Audio Output
//!
//! - Sample Rate: 16000 Hz or 8000 Hz
//! - Encoding: PCM, MP3, Speex, Speex-WB

use crate::core::stt::iflytek::IFlytekAuth;
use crate::core::tts::base::{TTSConfig, TTSError};

// =============================================================================
// Constants
// =============================================================================

/// TTS WebSocket endpoint.
pub const IFLYTEK_TTS_ENDPOINT: &str = "wss://tts-api-sg.xf-yun.com/v2/tts";

/// TTS WebSocket host.
pub const IFLYTEK_TTS_HOST: &str = "tts-api-sg.xf-yun.com";

/// TTS WebSocket path.
pub const IFLYTEK_TTS_PATH: &str = "/v2/tts";

/// Default sample rate for TTS output.
pub const DEFAULT_TTS_SAMPLE_RATE: u32 = 16000;

/// Default speaking speed (0-100, 50 is normal).
pub const DEFAULT_SPEED: u32 = 50;

/// Default volume (0-100, 50 is normal).
pub const DEFAULT_VOLUME: u32 = 50;

/// Default pitch (0-100, 50 is normal).
pub const DEFAULT_PITCH: u32 = 50;

// =============================================================================
// Voice Enum
// =============================================================================

/// Available voices for iFlytek TTS.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum IFlytekVoice {
    // Chinese voices
    /// Xiaoyan - Standard Chinese female (default).
    #[default]
    Xiaoyan,
    /// Aisjiuxu - Young Chinese male.
    Aisjiuxu,
    /// Aisxping - Chinese female broadcaster.
    Aisxping,
    /// Aisjinger - Sweet Chinese female.
    Aisjinger,
    /// Aisbabyxu - Chinese child voice.
    Aisbabyxu,

    // English voices
    /// John - American English male.
    JohnCe,
    /// Catherine - American English female.
    Catherine,

    // Other language voices
    /// Luna - Japanese female.
    Luna,
    /// Anjali - Hindi female.
    Anjali,

    /// Custom voice by name.
    Custom(String),
}

impl IFlytekVoice {
    /// Get the voice code for the API.
    pub fn as_code(&self) -> &str {
        match self {
            Self::Xiaoyan => "xiaoyan",
            Self::Aisjiuxu => "aisjiuxu",
            Self::Aisxping => "aisxping",
            Self::Aisjinger => "aisjinger",
            Self::Aisbabyxu => "aisbabyxu",
            Self::JohnCe => "john_ce",
            Self::Catherine => "catherine",
            Self::Luna => "luna",
            Self::Anjali => "anjali",
            Self::Custom(name) => name,
        }
    }

    /// Get the display name for the voice.
    pub fn display_name(&self) -> &str {
        match self {
            Self::Xiaoyan => "Xiaoyan (Chinese Female)",
            Self::Aisjiuxu => "Aisjiuxu (Chinese Male)",
            Self::Aisxping => "Aisxping (Chinese Female Broadcaster)",
            Self::Aisjinger => "Aisjinger (Chinese Sweet Female)",
            Self::Aisbabyxu => "Aisbabyxu (Chinese Child)",
            Self::JohnCe => "John (English Male)",
            Self::Catherine => "Catherine (English Female)",
            Self::Luna => "Luna (Japanese Female)",
            Self::Anjali => "Anjali (Hindi Female)",
            Self::Custom(name) => name,
        }
    }

    /// Parse voice from string.
    pub fn from_str(s: &str) -> Self {
        let normalized = s.trim().to_lowercase();
        match normalized.as_str() {
            "xiaoyan" => Self::Xiaoyan,
            "aisjiuxu" => Self::Aisjiuxu,
            "aisxping" => Self::Aisxping,
            "aisjinger" => Self::Aisjinger,
            "aisbabyxu" => Self::Aisbabyxu,
            "john_ce" | "john" => Self::JohnCe,
            "catherine" => Self::Catherine,
            "luna" => Self::Luna,
            "anjali" => Self::Anjali,
            _ => Self::Custom(s.to_string()),
        }
    }

    /// Get all built-in voices.
    pub fn all() -> &'static [Self] {
        &[
            Self::Xiaoyan,
            Self::Aisjiuxu,
            Self::Aisxping,
            Self::Aisjinger,
            Self::Aisbabyxu,
            Self::JohnCe,
            Self::Catherine,
            Self::Luna,
            Self::Anjali,
        ]
    }
}

// =============================================================================
// Audio Encoding Enum
// =============================================================================

/// Audio encoding formats for TTS output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IFlytekTtsEncoding {
    /// Raw PCM (16-bit, signed, little-endian).
    #[default]
    Raw,
    /// MP3 encoding.
    Lame,
    /// Speex encoding (8kHz).
    Speex,
    /// Speex wideband encoding (16kHz).
    SpeexWb,
}

impl IFlytekTtsEncoding {
    /// Get the encoding string for the API.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::Lame => "lame",
            Self::Speex => "speex",
            Self::SpeexWb => "speex-wb",
        }
    }

    /// Get the content type for the encoding.
    pub fn content_type(&self) -> &'static str {
        match self {
            Self::Raw => "audio/pcm",
            Self::Lame => "audio/mpeg",
            Self::Speex => "audio/speex",
            Self::SpeexWb => "audio/speex",
        }
    }

    /// Parse from string.
    pub fn from_str(s: &str) -> Option<Self> {
        let normalized = s.trim().to_lowercase();
        match normalized.as_str() {
            "raw" | "pcm" | "linear16" => Some(Self::Raw),
            "lame" | "mp3" | "mpeg" => Some(Self::Lame),
            "speex" => Some(Self::Speex),
            "speex-wb" | "speex_wb" => Some(Self::SpeexWb),
            _ => None,
        }
    }
}

// =============================================================================
// Text Encoding Enum
// =============================================================================

/// Text encoding formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IFlytekTextEncoding {
    /// UTF-8 encoding (default).
    #[default]
    Utf8,
    /// GB2312 encoding.
    Gb2312,
    /// GBK encoding.
    Gbk,
    /// BIG5 encoding.
    Big5,
    /// Unicode encoding.
    Unicode,
}

impl IFlytekTextEncoding {
    /// Get the encoding string for the API.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Utf8 => "UTF8",
            Self::Gb2312 => "GB2312",
            Self::Gbk => "GBK",
            Self::Big5 => "BIG5",
            Self::Unicode => "UNICODE",
        }
    }
}

// =============================================================================
// Configuration Struct
// =============================================================================

/// iFlytek TTS configuration.
#[derive(Debug, Clone)]
pub struct IFlytekTtsConfig {
    /// Authentication credentials.
    pub auth: IFlytekAuth,
    /// Voice to use.
    pub voice: IFlytekVoice,
    /// Audio encoding format.
    pub encoding: IFlytekTtsEncoding,
    /// Sample rate in Hz (16000 or 8000).
    pub sample_rate: u32,
    /// Text encoding format.
    pub text_encoding: IFlytekTextEncoding,
    /// Speaking speed (0-100, 50 is normal).
    pub speed: u32,
    /// Volume (0-100, 50 is normal).
    pub volume: u32,
    /// Pitch (0-100, 50 is normal).
    pub pitch: u32,
    /// Enable background sound.
    pub background_sound: bool,
    /// English pronunciation mode (0=auto, 1=letter, 2=word).
    pub english_pronunciation: u32,
    /// Number pronunciation mode (0=auto, 1=digit, 2=value, 3=auto v2).
    pub number_pronunciation: u32,
}

impl Default for IFlytekTtsConfig {
    fn default() -> Self {
        Self {
            auth: IFlytekAuth::new(String::new(), String::new(), String::new()),
            voice: IFlytekVoice::default(),
            encoding: IFlytekTtsEncoding::default(),
            sample_rate: DEFAULT_TTS_SAMPLE_RATE,
            text_encoding: IFlytekTextEncoding::default(),
            speed: DEFAULT_SPEED,
            volume: DEFAULT_VOLUME,
            pitch: DEFAULT_PITCH,
            background_sound: false,
            english_pronunciation: 0,
            number_pronunciation: 0,
        }
    }
}

impl IFlytekTtsConfig {
    /// Create configuration from base TTSConfig.
    ///
    /// # API Key Format
    /// `app_id|api_key|api_secret`
    pub fn from_base(config: TTSConfig) -> Result<Self, TTSError> {
        // Parse authentication credentials
        let auth = IFlytekAuth::from_combined(&config.api_key)
            .map_err(|e| TTSError::AuthenticationFailed(e.to_string()))?;

        // Parse voice
        let voice = config
            .voice_id
            .as_deref()
            .map(IFlytekVoice::from_str)
            .unwrap_or_default();

        // Parse encoding
        let encoding = config
            .audio_format
            .as_deref()
            .and_then(IFlytekTtsEncoding::from_str)
            .unwrap_or_default();

        // Parse sample rate
        let sample_rate = config.sample_rate.unwrap_or(DEFAULT_TTS_SAMPLE_RATE);

        // Parse speed from speaking_rate (convert 0.25-4.0 to 0-100)
        let speed = config
            .speaking_rate
            .map(|rate| {
                // Map 0.25-4.0 to 0-100
                // 1.0 -> 50
                // 0.25 -> 0
                // 4.0 -> 100
                let normalized = ((rate - 0.25) / 3.75 * 100.0).clamp(0.0, 100.0);
                normalized as u32
            })
            .unwrap_or(DEFAULT_SPEED);

        Ok(Self {
            auth,
            voice,
            encoding,
            sample_rate,
            text_encoding: IFlytekTextEncoding::default(),
            speed,
            volume: DEFAULT_VOLUME,
            pitch: DEFAULT_PITCH,
            background_sound: false,
            english_pronunciation: 0,
            number_pronunciation: 0,
        })
    }

    /// Build from the standardized TTS config (W1 keystone).
    ///
    /// iFlytek exposes prosody as 0-100 levels (50 = normal), so this maps `speed` (a multiplier
    /// where 1.0 = normal, scaled `speed * 50` so 1.0 -> 50 and clamped to 0-100), `pitch` and
    /// `volume` (taken as iFlytek 0-100 levels) onto those fields, plus `sample_rate` onto the
    /// output rate. iFlytek's
    /// `background_sound` (not a standard feature) is read from the `extras` passthrough. Features
    /// without an iFlytek field (stability, similarity_boost, style, use_speaker_boost, emotion,
    /// instructions, ssml, language, word_timestamps, streaming, seed) are skipped.
    pub fn from_standard(
        std: &crate::core::tts::standard::StandardTTSConfig,
    ) -> Result<Self, TTSError> {
        let f = &std.features;
        let mut cfg = Self::from_base(std.base.clone())?;

        if let Some(speed) = f.speed {
            // Map the speed multiplier (1.0 = normal) onto iFlytek's 0-100 level (50 = normal)
            // via `speed * 50`, so 1.0 -> 50, clamped to the valid 0-100 range.
            cfg.speed = (speed * 50.0).clamp(0.0, 100.0) as u32;
        }
        if let Some(pitch) = f.pitch {
            // iFlytek pitch is a 0-100 level (50 = normal).
            cfg.pitch = pitch.clamp(0.0, 100.0) as u32;
        }
        if let Some(volume) = f.volume {
            // iFlytek volume is a 0-100 level (50 = normal).
            cfg.volume = volume.clamp(0.0, 100.0) as u32;
        }
        if let Some(rate) = f.sample_rate {
            cfg.sample_rate = rate;
        }

        // Provider-specific passthrough.
        if let Some(bg) = std
            .extras
            .0
            .get("background_sound")
            .and_then(|v| v.as_bool())
        {
            cfg.background_sound = bg;
        }

        Ok(cfg)
    }

    /// Validate the configuration.
    pub fn validate(&self) -> Result<(), TTSError> {
        self.auth
            .validate()
            .map_err(|e| TTSError::AuthenticationFailed(e.to_string()))?;

        if self.sample_rate != 8000 && self.sample_rate != 16000 {
            return Err(TTSError::InvalidConfiguration(format!(
                "Invalid sample rate: {}. Must be 8000 or 16000 Hz.",
                self.sample_rate
            )));
        }

        if self.speed > 100 {
            return Err(TTSError::InvalidConfiguration(format!(
                "Invalid speed: {}. Must be 0-100.",
                self.speed
            )));
        }

        if self.volume > 100 {
            return Err(TTSError::InvalidConfiguration(format!(
                "Invalid volume: {}. Must be 0-100.",
                self.volume
            )));
        }

        if self.pitch > 100 {
            return Err(TTSError::InvalidConfiguration(format!(
                "Invalid pitch: {}. Must be 0-100.",
                self.pitch
            )));
        }

        Ok(())
    }

    /// Build the audio format string for the API.
    pub fn audio_format_string(&self) -> String {
        format!("audio/L16;rate={}", self.sample_rate)
    }

    /// Get list of available voices.
    pub fn available_voices() -> Vec<&'static str> {
        IFlytekVoice::all().iter().map(|v| v.as_code()).collect()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_api_key() -> String {
        "test_app_id|test_api_key_xxxxx|test_api_secret_xx".to_string()
    }

    fn create_test_config() -> TTSConfig {
        TTSConfig {
            api_key: create_test_api_key(),
            voice_id: Some("xiaoyan".to_string()),
            sample_rate: Some(16000),
            audio_format: Some("raw".to_string()),
            ..Default::default()
        }
    }

    // W1 keystone (TTS): the standardized prosody features iFlytek can express (speed, pitch,
    // volume as 0-100 levels, plus output sample rate) reach the request fields, and the open
    // extras passthrough carries the provider-specific background_sound knob.
    #[test]
    fn from_standard_maps_prosody_and_extras() {
        use crate::core::tts::standard::{ProviderExtras, StandardTTSConfig, TtsFeatures};
        let mut extras = serde_json::Map::new();
        extras.insert("background_sound".into(), serde_json::json!(true));
        let std = StandardTTSConfig {
            base: TTSConfig {
                provider: "iflytek".into(),
                api_key: create_test_api_key(),
                ..Default::default()
            },
            features: TtsFeatures {
                speed: Some(1.0),
                pitch: Some(70.0),
                volume: Some(80.0),
                sample_rate: Some(8000),
                ssml: Some(true), // capability gap: iFlytek has no SSML, must be ignored
                ..Default::default()
            },
            extras: ProviderExtras(extras),
        };
        let cfg = IFlytekTtsConfig::from_standard(&std).unwrap();
        assert_eq!(cfg.speed, 50); // 1.0x multiplier -> 50 (iFlytek normal)
        assert_eq!(cfg.pitch, 70);
        assert_eq!(cfg.volume, 80);
        assert_eq!(cfg.sample_rate, 8000);
        assert!(cfg.background_sound); // from extras passthrough
    }

    // Voice tests
    #[test]
    fn test_voice_codes() {
        assert_eq!(IFlytekVoice::Xiaoyan.as_code(), "xiaoyan");
        assert_eq!(IFlytekVoice::JohnCe.as_code(), "john_ce");
        assert_eq!(IFlytekVoice::Luna.as_code(), "luna");
    }

    #[test]
    fn test_voice_from_str() {
        assert_eq!(IFlytekVoice::from_str("xiaoyan"), IFlytekVoice::Xiaoyan);
        assert_eq!(IFlytekVoice::from_str("john_ce"), IFlytekVoice::JohnCe);
        assert_eq!(IFlytekVoice::from_str("john"), IFlytekVoice::JohnCe);
    }

    #[test]
    fn test_voice_custom() {
        let custom = IFlytekVoice::from_str("custom_voice_123");
        assert!(matches!(custom, IFlytekVoice::Custom(_)));
        assert_eq!(custom.as_code(), "custom_voice_123");
    }

    #[test]
    fn test_voice_all() {
        let all = IFlytekVoice::all();
        assert!(all.len() >= 9);
        assert!(all.contains(&IFlytekVoice::Xiaoyan));
        assert!(all.contains(&IFlytekVoice::JohnCe));
    }

    // Encoding tests
    #[test]
    fn test_encoding_strings() {
        assert_eq!(IFlytekTtsEncoding::Raw.as_str(), "raw");
        assert_eq!(IFlytekTtsEncoding::Lame.as_str(), "lame");
        assert_eq!(IFlytekTtsEncoding::Speex.as_str(), "speex");
    }

    #[test]
    fn test_encoding_from_str() {
        assert_eq!(
            IFlytekTtsEncoding::from_str("raw"),
            Some(IFlytekTtsEncoding::Raw)
        );
        assert_eq!(
            IFlytekTtsEncoding::from_str("mp3"),
            Some(IFlytekTtsEncoding::Lame)
        );
        assert_eq!(IFlytekTtsEncoding::from_str("unknown"), None);
    }

    #[test]
    fn test_encoding_content_type() {
        assert_eq!(IFlytekTtsEncoding::Raw.content_type(), "audio/pcm");
        assert_eq!(IFlytekTtsEncoding::Lame.content_type(), "audio/mpeg");
    }

    // Text encoding tests
    #[test]
    fn test_text_encoding_strings() {
        assert_eq!(IFlytekTextEncoding::Utf8.as_str(), "UTF8");
        assert_eq!(IFlytekTextEncoding::Gb2312.as_str(), "GB2312");
    }

    // Config tests
    #[test]
    fn test_config_from_base_valid() {
        let base = create_test_config();
        let config = IFlytekTtsConfig::from_base(base);
        assert!(config.is_ok());

        let config = config.unwrap();
        assert_eq!(config.voice, IFlytekVoice::Xiaoyan);
        assert_eq!(config.sample_rate, 16000);
        assert_eq!(config.encoding, IFlytekTtsEncoding::Raw);
    }

    #[test]
    fn test_config_from_base_invalid_api_key() {
        let base = TTSConfig {
            api_key: "invalid_format".to_string(),
            ..Default::default()
        };
        let config = IFlytekTtsConfig::from_base(base);
        assert!(config.is_err());
    }

    #[test]
    fn test_config_validation_valid() {
        let base = create_test_config();
        let config = IFlytekTtsConfig::from_base(base).unwrap();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validation_invalid_sample_rate() {
        let mut config = IFlytekTtsConfig::default();
        config.auth = IFlytekAuth::new("app".to_string(), "key".to_string(), "secret".to_string());
        config.sample_rate = 44100;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_invalid_speed() {
        let mut config = IFlytekTtsConfig::default();
        config.auth = IFlytekAuth::new("app".to_string(), "key".to_string(), "secret".to_string());
        config.speed = 150;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_audio_format_string() {
        let mut config = IFlytekTtsConfig::default();
        config.sample_rate = 16000;
        assert_eq!(config.audio_format_string(), "audio/L16;rate=16000");

        config.sample_rate = 8000;
        assert_eq!(config.audio_format_string(), "audio/L16;rate=8000");
    }

    // Constants tests
    #[test]
    fn test_constants() {
        assert_eq!(DEFAULT_TTS_SAMPLE_RATE, 16000);
        assert_eq!(DEFAULT_SPEED, 50);
        assert_eq!(DEFAULT_VOLUME, 50);
        assert_eq!(DEFAULT_PITCH, 50);
    }

    #[test]
    fn test_endpoint_constants() {
        assert!(IFLYTEK_TTS_ENDPOINT.starts_with("wss://"));
        assert!(IFLYTEK_TTS_ENDPOINT.contains("tts"));
        assert_eq!(IFLYTEK_TTS_HOST, "tts-api-sg.xf-yun.com");
        assert_eq!(IFLYTEK_TTS_PATH, "/v2/tts");
    }
}
