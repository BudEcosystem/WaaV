//! Tencent Cloud TTS Configuration
//!
//! Configuration types and constants for Tencent Cloud Text-to-Speech service.
//!
//! # Overview
//!
//! Tencent Cloud TTS provides high-quality text-to-speech synthesis via REST API.
//! Authentication uses TC3-HMAC-SHA256 signature algorithm.
//!
//! # API Endpoint
//!
//! - REST: `https://tts.tencentcloudapi.com` (International)
//! - REST: `https://tts.intl.tencentcloudapi.com` (International API)
//!
//! # Authentication
//!
//! The API key format is `secret_id|secret_key` (pipe-separated).
//! Requests are signed using TC3-HMAC-SHA256 algorithm.

use crate::core::tts::base::{TTSConfig, TTSError};
use serde::{Deserialize, Serialize};

// =============================================================================
// Constants
// =============================================================================

/// Tencent Cloud TTS REST endpoint (International).
pub const TENCENT_TTS_URL: &str = "https://tts.tencentcloudapi.com";

/// Tencent Cloud TTS REST endpoint (International API).
pub const TENCENT_TTS_INTL_URL: &str = "https://tts.intl.tencentcloudapi.com";

/// API Action name for TextToVoice.
pub const TTS_ACTION: &str = "TextToVoice";

/// API Version.
pub const TTS_VERSION: &str = "2019-08-23";

/// Default voice type (亲和女声 - Friendly Female).
pub const DEFAULT_VOICE_TYPE: i64 = 0;

/// Default speed (0.5-2.0, 1.0 = normal).
pub const DEFAULT_SPEED: f32 = 1.0;

/// Default volume (0-10, 5 = normal).
pub const DEFAULT_VOLUME: f32 = 5.0;

/// Default project ID.
pub const DEFAULT_PROJECT_ID: i64 = 0;

/// Minimum speed.
pub const MIN_SPEED: f32 = 0.5;

/// Maximum speed.
pub const MAX_SPEED: f32 = 2.0;

/// Minimum volume.
pub const MIN_VOLUME: f32 = 0.0;

/// Maximum volume.
pub const MAX_VOLUME: f32 = 10.0;

/// Maximum text length (characters).
pub const MAX_TEXT_LENGTH: usize = 300;

// =============================================================================
// Voice Categories
// =============================================================================

/// Tencent TTS voice categories.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TencentVoiceCategory {
    /// Basic standard voices.
    #[default]
    Standard,
    /// Premium high-quality voices.
    Premium,
    /// Emotional expressive voices.
    Emotional,
    /// Dialect voices (Cantonese, Sichuan, etc.).
    Dialect,
    /// Child voices.
    Child,
    /// English voices.
    English,
    /// Multilingual voices.
    Multilingual,
}

impl TencentVoiceCategory {
    /// Get all voice type IDs for this category.
    pub fn voice_ids(&self) -> &'static [i64] {
        match self {
            TencentVoiceCategory::Standard => &[0, 1, 2, 4, 5, 6],
            TencentVoiceCategory::Premium => &[
                101001, 101002, 101003, 101004, 101005, 101006, 101007, 101008, 101009, 101010,
                101011, 101012, 101013, 101014, 101015, 101016, 101017, 101018, 101019, 101020,
                101021, 101022, 101023, 101024, 101025, 101026, 101027, 101028, 101029, 101030,
            ],
            TencentVoiceCategory::Emotional => &[
                101031, 101032, 101033, 101034, 101035, 101036, 101037, 101038, 101039, 101040,
            ],
            TencentVoiceCategory::Dialect => &[
                301001, 301002, 301003, 301004, 301005, 301006, 301007, 301008, 301009, 301010,
                301011, 301012, 301013, 301014, 301015, 301016, 301017, 301018, 301019, 301020,
                301021, 301022, 301023, 301024, 301025, 301026, 301027, 301028, 301029, 301030,
                301031, 301032,
            ],
            TencentVoiceCategory::Child => &[501001, 501002, 501003, 501004, 501005],
            TencentVoiceCategory::English => &[601001, 601002, 601003, 601004],
            TencentVoiceCategory::Multilingual => &[701001, 701002, 701003, 701004],
        }
    }

    /// Get human-readable name.
    pub fn name(&self) -> &'static str {
        match self {
            TencentVoiceCategory::Standard => "Standard",
            TencentVoiceCategory::Premium => "Premium",
            TencentVoiceCategory::Emotional => "Emotional",
            TencentVoiceCategory::Dialect => "Dialect",
            TencentVoiceCategory::Child => "Child",
            TencentVoiceCategory::English => "English",
            TencentVoiceCategory::Multilingual => "Multilingual",
        }
    }
}

// =============================================================================
// Voice Enum
// =============================================================================

/// Tencent TTS voice options.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TencentTtsVoice {
    // Standard voices
    /// 亲和女声 - Friendly Female (ID: 0).
    #[default]
    FriendlyFemale,
    /// 亲和男声 - Friendly Male (ID: 1).
    FriendlyMale,
    /// 成熟男声 - Mature Male (ID: 2).
    MatureMale,
    /// 温暖女声 - Warm Female (ID: 4).
    WarmFemale,
    /// 情感女声 - Emotional Female (ID: 5).
    EmotionalFemale,
    /// 情感男声 - Emotional Male (ID: 6).
    EmotionalMale,

    // Custom voice by ID
    /// Custom voice by numeric ID.
    Custom(i64),
}

impl TencentTtsVoice {
    /// Get the voice type ID for API request.
    pub fn as_id(&self) -> i64 {
        match self {
            TencentTtsVoice::FriendlyFemale => 0,
            TencentTtsVoice::FriendlyMale => 1,
            TencentTtsVoice::MatureMale => 2,
            TencentTtsVoice::WarmFemale => 4,
            TencentTtsVoice::EmotionalFemale => 5,
            TencentTtsVoice::EmotionalMale => 6,
            TencentTtsVoice::Custom(id) => *id,
        }
    }

    /// Get the voice name.
    pub fn name(&self) -> &str {
        match self {
            TencentTtsVoice::FriendlyFemale => "亲和女声 (Friendly Female)",
            TencentTtsVoice::FriendlyMale => "亲和男声 (Friendly Male)",
            TencentTtsVoice::MatureMale => "成熟男声 (Mature Male)",
            TencentTtsVoice::WarmFemale => "温暖女声 (Warm Female)",
            TencentTtsVoice::EmotionalFemale => "情感女声 (Emotional Female)",
            TencentTtsVoice::EmotionalMale => "情感男声 (Emotional Male)",
            TencentTtsVoice::Custom(_) => "Custom Voice",
        }
    }

    /// Parse from string or numeric ID.
    pub fn from_str_or_id(s: &str) -> Self {
        // Try to parse as numeric ID first
        if let Ok(id) = s.parse::<i64>() {
            return match id {
                0 => TencentTtsVoice::FriendlyFemale,
                1 => TencentTtsVoice::FriendlyMale,
                2 => TencentTtsVoice::MatureMale,
                4 => TencentTtsVoice::WarmFemale,
                5 => TencentTtsVoice::EmotionalFemale,
                6 => TencentTtsVoice::EmotionalMale,
                _ => TencentTtsVoice::Custom(id),
            };
        }

        // Try to parse as voice name
        match s.to_lowercase().as_str() {
            "friendly_female" | "friendlyfemale" | "female" | "0" => {
                TencentTtsVoice::FriendlyFemale
            }
            "friendly_male" | "friendlymale" | "male" | "1" => TencentTtsVoice::FriendlyMale,
            "mature_male" | "maturemale" | "2" => TencentTtsVoice::MatureMale,
            "warm_female" | "warmfemale" | "4" => TencentTtsVoice::WarmFemale,
            "emotional_female" | "emotionalfemale" | "5" => TencentTtsVoice::EmotionalFemale,
            "emotional_male" | "emotionalmale" | "6" => TencentTtsVoice::EmotionalMale,
            _ => TencentTtsVoice::FriendlyFemale,
        }
    }

    /// Get the category of this voice.
    pub fn category(&self) -> TencentVoiceCategory {
        let id = self.as_id();
        if id <= 10 {
            TencentVoiceCategory::Standard
        } else if (101001..=101030).contains(&id) {
            TencentVoiceCategory::Premium
        } else if (101031..=101040).contains(&id) {
            TencentVoiceCategory::Emotional
        } else if (301001..=301032).contains(&id) {
            TencentVoiceCategory::Dialect
        } else if (501001..=501005).contains(&id) {
            TencentVoiceCategory::Child
        } else if (601001..=601004).contains(&id) {
            TencentVoiceCategory::English
        } else {
            TencentVoiceCategory::Multilingual
        }
    }
}

// =============================================================================
// Audio Format Enum
// =============================================================================

/// Tencent TTS audio output format (codec).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TencentTtsAudioFormat {
    /// WAV format.
    #[default]
    Wav,
    /// MP3 format.
    Mp3,
    /// PCM format (16-bit).
    Pcm,
}

impl TencentTtsAudioFormat {
    /// Get the codec string for API request.
    pub fn as_codec(&self) -> &'static str {
        match self {
            TencentTtsAudioFormat::Wav => "wav",
            TencentTtsAudioFormat::Mp3 => "mp3",
            TencentTtsAudioFormat::Pcm => "pcm",
        }
    }

    /// Get the format string representation.
    pub fn as_format_str(&self) -> &'static str {
        match self {
            TencentTtsAudioFormat::Wav => "wav",
            TencentTtsAudioFormat::Mp3 => "mp3",
            TencentTtsAudioFormat::Pcm => "pcm",
        }
    }

    /// Get the MIME type.
    pub fn mime_type(&self) -> &'static str {
        match self {
            TencentTtsAudioFormat::Wav => "audio/wav",
            TencentTtsAudioFormat::Mp3 => "audio/mpeg",
            TencentTtsAudioFormat::Pcm => "audio/pcm",
        }
    }

    /// Get the sample rate.
    pub fn sample_rate(&self) -> u32 {
        // Tencent TTS outputs at 16kHz
        16000
    }

    /// Parse from string.
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "wav" => TencentTtsAudioFormat::Wav,
            "mp3" | "mpeg" => TencentTtsAudioFormat::Mp3,
            "pcm" | "raw" => TencentTtsAudioFormat::Pcm,
            _ => TencentTtsAudioFormat::Wav,
        }
    }
}

// =============================================================================
// Sample Rate Enum
// =============================================================================

/// Tencent TTS sample rate options.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TencentTtsSampleRate {
    /// 8000 Hz.
    Rate8000,
    /// 16000 Hz (default).
    #[default]
    Rate16000,
}

impl TencentTtsSampleRate {
    /// Get the sample rate value.
    pub fn value(&self) -> i64 {
        match self {
            TencentTtsSampleRate::Rate8000 => 8000,
            TencentTtsSampleRate::Rate16000 => 16000,
        }
    }

    /// Parse from integer value.
    pub fn from_value(value: u32) -> Self {
        if value <= 8000 {
            TencentTtsSampleRate::Rate8000
        } else {
            TencentTtsSampleRate::Rate16000
        }
    }
}

// =============================================================================
// API Response Types
// =============================================================================

/// Tencent TTS API response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TencentTtsResponse {
    /// Response wrapper.
    #[serde(rename = "Response")]
    pub response: TencentTtsResponseInner,
}

/// Inner response data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TencentTtsResponseInner {
    /// Base64-encoded audio data.
    #[serde(rename = "Audio")]
    pub audio: Option<String>,

    /// Session ID.
    #[serde(rename = "SessionId")]
    pub session_id: Option<String>,

    /// Error information.
    #[serde(rename = "Error")]
    pub error: Option<TencentTtsErrorInfo>,

    /// Request ID.
    #[serde(rename = "RequestId")]
    pub request_id: Option<String>,

    /// Subtitles (word-level timestamps).
    #[serde(rename = "Subtitles")]
    pub subtitles: Option<Vec<TencentTtsSubtitle>>,
}

/// Error information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TencentTtsErrorInfo {
    /// Error code.
    #[serde(rename = "Code")]
    pub code: String,

    /// Error message.
    #[serde(rename = "Message")]
    pub message: String,
}

/// Subtitle (word-level timestamp).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TencentTtsSubtitle {
    /// Text content.
    #[serde(rename = "Text")]
    pub text: String,

    /// Start time in milliseconds.
    #[serde(rename = "BeginTime")]
    pub begin_time: i64,

    /// End time in milliseconds.
    #[serde(rename = "EndTime")]
    pub end_time: i64,

    /// Start index in original text.
    #[serde(rename = "BeginIndex")]
    pub begin_index: Option<i64>,

    /// End index in original text.
    #[serde(rename = "EndIndex")]
    pub end_index: Option<i64>,

    /// Phoneme information.
    #[serde(rename = "Phoneme")]
    pub phoneme: Option<String>,
}

// =============================================================================
// Configuration
// =============================================================================

/// Tencent Cloud TTS configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TencentTtsConfig {
    /// Tencent Cloud Secret ID.
    pub secret_id: String,

    /// Tencent Cloud Secret Key.
    pub secret_key: String,

    /// Project ID (default: 0).
    pub project_id: i64,

    /// Voice type ID.
    pub voice_type: TencentTtsVoice,

    /// Audio output format.
    pub audio_format: TencentTtsAudioFormat,

    /// Sample rate.
    pub sample_rate: TencentTtsSampleRate,

    /// Speech speed (0.5-2.0, 1.0 = normal).
    pub speed: f32,

    /// Volume level (0-10, 5 = normal).
    pub volume: f32,

    /// Use international endpoint.
    pub use_intl_endpoint: bool,

    /// Enable word-level timestamps.
    pub enable_subtitles: bool,

    /// Language code.
    pub primary_language: Option<i64>,

    /// Emotion category (for emotional voices).
    pub emotion_category: Option<String>,

    /// Emotion intensity (0-200, 100 = normal).
    pub emotion_intensity: Option<i64>,

    /// Region (for domestic API).
    pub region: Option<String>,
}

impl Default for TencentTtsConfig {
    fn default() -> Self {
        Self {
            secret_id: String::new(),
            secret_key: String::new(),
            project_id: DEFAULT_PROJECT_ID,
            voice_type: TencentTtsVoice::default(),
            audio_format: TencentTtsAudioFormat::default(),
            sample_rate: TencentTtsSampleRate::default(),
            speed: DEFAULT_SPEED,
            volume: DEFAULT_VOLUME,
            use_intl_endpoint: true,
            enable_subtitles: false,
            primary_language: None,
            emotion_category: None,
            emotion_intensity: None,
            region: None,
        }
    }
}

impl TencentTtsConfig {
    /// Create a new configuration with credentials.
    pub fn new(secret_id: impl Into<String>, secret_key: impl Into<String>) -> Self {
        Self {
            secret_id: secret_id.into(),
            secret_key: secret_key.into(),
            ..Default::default()
        }
    }

    /// Set the voice type.
    pub fn with_voice_type(mut self, voice: TencentTtsVoice) -> Self {
        self.voice_type = voice;
        self
    }

    /// Set the audio format.
    pub fn with_audio_format(mut self, format: TencentTtsAudioFormat) -> Self {
        self.audio_format = format;
        self
    }

    /// Set the sample rate.
    pub fn with_sample_rate(mut self, rate: TencentTtsSampleRate) -> Self {
        self.sample_rate = rate;
        self
    }

    /// Set the speech speed.
    pub fn with_speed(mut self, speed: f32) -> Self {
        self.speed = speed.clamp(MIN_SPEED, MAX_SPEED);
        self
    }

    /// Set the volume level.
    pub fn with_volume(mut self, volume: f32) -> Self {
        self.volume = volume.clamp(MIN_VOLUME, MAX_VOLUME);
        self
    }

    /// Set the project ID.
    pub fn with_project_id(mut self, project_id: i64) -> Self {
        self.project_id = project_id;
        self
    }

    /// Enable or disable international endpoint.
    pub fn with_intl_endpoint(mut self, use_intl: bool) -> Self {
        self.use_intl_endpoint = use_intl;
        self
    }

    /// Enable word-level timestamps.
    pub fn with_subtitles(mut self, enable: bool) -> Self {
        self.enable_subtitles = enable;
        self
    }

    /// Set the primary language.
    pub fn with_primary_language(mut self, language: i64) -> Self {
        self.primary_language = Some(language);
        self
    }

    /// Set the emotion category.
    pub fn with_emotion(mut self, category: impl Into<String>, intensity: i64) -> Self {
        self.emotion_category = Some(category.into());
        self.emotion_intensity = Some(intensity.clamp(0, 200));
        self
    }

    /// Set the region.
    pub fn with_region(mut self, region: impl Into<String>) -> Self {
        self.region = Some(region.into());
        self
    }

    /// Validate the configuration.
    pub fn validate(&self) -> Result<(), TTSError> {
        if self.secret_id.is_empty() {
            return Err(TTSError::InvalidConfiguration(
                "Tencent Secret ID is required".to_string(),
            ));
        }

        if self.secret_key.is_empty() {
            return Err(TTSError::InvalidConfiguration(
                "Tencent Secret Key is required".to_string(),
            ));
        }

        if self.speed < MIN_SPEED || self.speed > MAX_SPEED {
            return Err(TTSError::InvalidConfiguration(format!(
                "Speed must be between {} and {}",
                MIN_SPEED, MAX_SPEED
            )));
        }

        if self.volume < MIN_VOLUME || self.volume > MAX_VOLUME {
            return Err(TTSError::InvalidConfiguration(format!(
                "Volume must be between {} and {}",
                MIN_VOLUME, MAX_VOLUME
            )));
        }

        Ok(())
    }

    /// Get the endpoint URL.
    pub fn get_endpoint_url(&self) -> &'static str {
        if self.use_intl_endpoint {
            TENCENT_TTS_INTL_URL
        } else {
            TENCENT_TTS_URL
        }
    }

    /// Get the sample rate as u32.
    pub fn get_sample_rate(&self) -> u32 {
        self.sample_rate.value() as u32
    }

    /// Get list of supported voices.
    pub fn supported_voices() -> Vec<(&'static str, i64, &'static str)> {
        vec![
            ("亲和女声 (Friendly Female)", 0, "zh"),
            ("亲和男声 (Friendly Male)", 1, "zh"),
            ("成熟男声 (Mature Male)", 2, "zh"),
            ("温暖女声 (Warm Female)", 4, "zh"),
            ("情感女声 (Emotional Female)", 5, "zh"),
            ("情感男声 (Emotional Male)", 6, "zh"),
        ]
    }

    /// Get list of supported formats.
    pub fn supported_formats() -> Vec<&'static str> {
        vec!["wav", "mp3", "pcm"]
    }

    /// Create from base TTSConfig.
    ///
    /// Expected API key format: `secret_id|secret_key`
    pub fn from_base(config: TTSConfig) -> Result<Self, TTSError> {
        let parts: Vec<&str> = config.api_key.splitn(2, '|').collect();
        if parts.len() != 2 {
            return Err(TTSError::InvalidConfiguration(
                "API key must be in format: secret_id|secret_key".to_string(),
            ));
        }

        let secret_id = parts[0].to_string();
        let secret_key = parts[1].to_string();

        if secret_id.is_empty() || secret_key.is_empty() {
            return Err(TTSError::InvalidConfiguration(
                "Both secret_id and secret_key are required".to_string(),
            ));
        }

        // Parse voice
        let voice = config
            .voice_id
            .as_ref()
            .map(|v| TencentTtsVoice::from_str_or_id(v))
            .unwrap_or_default();

        // Parse audio format
        let audio_format = config
            .audio_format
            .as_ref()
            .map(|f| TencentTtsAudioFormat::from_str(f))
            .unwrap_or_default();

        // Parse sample rate
        let sample_rate = config
            .sample_rate
            .map(TencentTtsSampleRate::from_value)
            .unwrap_or_default();

        // Parse speed (speaking_rate maps to speed)
        let speed = config.speaking_rate.unwrap_or(DEFAULT_SPEED);

        let tencent_config = Self {
            secret_id,
            secret_key,
            voice_type: voice,
            audio_format,
            sample_rate,
            speed,
            ..Default::default()
        };

        tencent_config.validate()?;
        Ok(tencent_config)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // Voice Tests
    // =========================================================================

    #[test]
    fn test_voice_default() {
        let voice = TencentTtsVoice::default();
        assert_eq!(voice.as_id(), 0);
    }

    #[test]
    fn test_voice_parsing_id() {
        assert_eq!(
            TencentTtsVoice::from_str_or_id("0"),
            TencentTtsVoice::FriendlyFemale
        );
        assert_eq!(
            TencentTtsVoice::from_str_or_id("1"),
            TencentTtsVoice::FriendlyMale
        );
        assert_eq!(
            TencentTtsVoice::from_str_or_id("101001"),
            TencentTtsVoice::Custom(101001)
        );
    }

    #[test]
    fn test_voice_parsing_name() {
        assert_eq!(
            TencentTtsVoice::from_str_or_id("friendly_female"),
            TencentTtsVoice::FriendlyFemale
        );
        assert_eq!(
            TencentTtsVoice::from_str_or_id("male"),
            TencentTtsVoice::FriendlyMale
        );
    }

    #[test]
    fn test_voice_category() {
        assert_eq!(
            TencentTtsVoice::FriendlyFemale.category(),
            TencentVoiceCategory::Standard
        );
        assert_eq!(
            TencentTtsVoice::Custom(101001).category(),
            TencentVoiceCategory::Premium
        );
    }

    // =========================================================================
    // Audio Format Tests
    // =========================================================================

    #[test]
    fn test_audio_format_default() {
        let format = TencentTtsAudioFormat::default();
        assert_eq!(format.as_codec(), "wav");
    }

    #[test]
    fn test_audio_format_parsing() {
        assert_eq!(
            TencentTtsAudioFormat::from_str("mp3"),
            TencentTtsAudioFormat::Mp3
        );
        assert_eq!(
            TencentTtsAudioFormat::from_str("pcm"),
            TencentTtsAudioFormat::Pcm
        );
    }

    #[test]
    fn test_audio_format_sample_rate() {
        assert_eq!(TencentTtsAudioFormat::Wav.sample_rate(), 16000);
        assert_eq!(TencentTtsAudioFormat::Mp3.sample_rate(), 16000);
        assert_eq!(TencentTtsAudioFormat::Pcm.sample_rate(), 16000);
    }

    // =========================================================================
    // Config Tests
    // =========================================================================

    #[test]
    fn test_config_new() {
        let config = TencentTtsConfig::new("secret_id", "secret_key");
        assert_eq!(config.secret_id, "secret_id");
        assert_eq!(config.secret_key, "secret_key");
    }

    #[test]
    fn test_config_validation_empty_secret_id() {
        let config = TencentTtsConfig::new("", "secret");
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_empty_secret_key() {
        let config = TencentTtsConfig::new("id", "");
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_success() {
        let config = TencentTtsConfig::new("id", "key");
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_speed_clamping() {
        let config = TencentTtsConfig::new("id", "key")
            .with_speed(0.1); // Below minimum
        assert_eq!(config.speed, MIN_SPEED);

        let config = TencentTtsConfig::new("id", "key")
            .with_speed(5.0); // Above maximum
        assert_eq!(config.speed, MAX_SPEED);
    }

    #[test]
    fn test_config_volume_clamping() {
        let config = TencentTtsConfig::new("id", "key")
            .with_volume(-1.0); // Below minimum
        assert_eq!(config.volume, MIN_VOLUME);

        let config = TencentTtsConfig::new("id", "key")
            .with_volume(15.0); // Above maximum
        assert_eq!(config.volume, MAX_VOLUME);
    }

    #[test]
    fn test_config_from_base() {
        let base = TTSConfig {
            api_key: "secret_id|secret_key".to_string(),
            voice_id: Some("1".to_string()),
            audio_format: Some("mp3".to_string()),
            speaking_rate: Some(1.5),
            ..Default::default()
        };

        let result = TencentTtsConfig::from_base(base);
        assert!(result.is_ok());

        let config = result.unwrap();
        assert_eq!(config.secret_id, "secret_id");
        assert_eq!(config.secret_key, "secret_key");
        assert_eq!(config.voice_type, TencentTtsVoice::FriendlyMale);
        assert_eq!(config.audio_format, TencentTtsAudioFormat::Mp3);
        assert_eq!(config.speed, 1.5);
    }

    #[test]
    fn test_config_from_base_invalid_format() {
        let base = TTSConfig {
            api_key: "no_pipe".to_string(),
            ..Default::default()
        };

        let result = TencentTtsConfig::from_base(base);
        assert!(result.is_err());
    }

    // =========================================================================
    // Endpoint Tests
    // =========================================================================

    #[test]
    fn test_intl_endpoint() {
        let config = TencentTtsConfig::new("id", "key").with_intl_endpoint(true);
        assert_eq!(config.get_endpoint_url(), TENCENT_TTS_INTL_URL);
    }

    #[test]
    fn test_domestic_endpoint() {
        let config = TencentTtsConfig::new("id", "key").with_intl_endpoint(false);
        assert_eq!(config.get_endpoint_url(), TENCENT_TTS_URL);
    }

    // =========================================================================
    // Constants Tests
    // =========================================================================

    #[test]
    fn test_constants() {
        assert!(!TENCENT_TTS_URL.is_empty());
        assert!(!TENCENT_TTS_INTL_URL.is_empty());
        assert_eq!(TTS_ACTION, "TextToVoice");
        assert!(MAX_TEXT_LENGTH > 0);
    }

    // =========================================================================
    // Supported Items Tests
    // =========================================================================

    #[test]
    fn test_supported_voices() {
        let voices = TencentTtsConfig::supported_voices();
        assert!(!voices.is_empty());
        assert!(voices.iter().any(|(_, id, _)| *id == 0));
    }

    #[test]
    fn test_supported_formats() {
        let formats = TencentTtsConfig::supported_formats();
        assert!(formats.contains(&"wav"));
        assert!(formats.contains(&"mp3"));
        assert!(formats.contains(&"pcm"));
    }
}
