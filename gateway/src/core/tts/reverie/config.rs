//! Reverie TTS Configuration Types
//!
//! This module defines configuration structures for the Reverie TTS provider,
//! including speaker voices, audio formats, and synthesis parameters.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

use crate::core::tts::base::TTSConfig;

// =============================================================================
// Audio Format Enum
// =============================================================================

/// Supported audio output formats for Reverie TTS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum ReverieTtsAudioFormat {
    /// WAV format (uncompressed).
    #[default]
    Wav,
    /// MP3 format (compressed).
    Mp3,
}

impl ReverieTtsAudioFormat {
    /// Get the format string for API requests.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Wav => "WAV",
            Self::Mp3 => "MP3",
        }
    }

    /// Get the content type for this format.
    pub fn content_type(&self) -> &'static str {
        match self {
            Self::Wav => "audio/wav",
            Self::Mp3 => "audio/mpeg",
        }
    }

    /// Get the file extension.
    pub fn extension(&self) -> &'static str {
        match self {
            Self::Wav => ".wav",
            Self::Mp3 => ".mp3",
        }
    }
}

impl fmt::Display for ReverieTtsAudioFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for ReverieTtsAudioFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "wav" | "wave" | "linear16" | "pcm" => Ok(Self::Wav),
            "mp3" | "mpeg" => Ok(Self::Mp3),
            _ => Err(format!("Unknown Reverie TTS audio format: {}", s)),
        }
    }
}

// =============================================================================
// Gender Enum
// =============================================================================

/// Speaker gender for Reverie TTS voices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReverieGender {
    /// Male voice.
    Male,
    /// Female voice.
    #[default]
    Female,
}

impl ReverieGender {
    /// Get the gender suffix for speaker codes.
    pub fn as_suffix(&self) -> &'static str {
        match self {
            Self::Male => "male",
            Self::Female => "female",
        }
    }
}

impl fmt::Display for ReverieGender {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_suffix())
    }
}

impl FromStr for ReverieGender {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "male" | "m" => Ok(Self::Male),
            "female" | "f" => Ok(Self::Female),
            _ => Err(format!("Unknown gender: {}", s)),
        }
    }
}

// =============================================================================
// Speaker Configuration
// =============================================================================

/// Speaker configuration for Reverie TTS.
///
/// Speaker codes follow the pattern: `{language_code}_{gender}` or `{language_code}_{gender}_{variant}`
///
/// # Examples
///
/// - `hi_female` - Hindi female voice
/// - `hi_male_2` - Hindi male voice variant 2
/// - `en_female` - English female voice
/// - `ta_male` - Tamil male voice
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReverieSpeaker {
    /// Language code (e.g., "hi", "en", "ta").
    pub language: String,
    /// Voice gender.
    pub gender: ReverieGender,
    /// Voice variant number (1 = default, 2+ = alternate voices).
    pub variant: u8,
}

impl ReverieSpeaker {
    /// Create a new speaker configuration.
    ///
    /// # Arguments
    /// * `language` - ISO language code (e.g., "hi", "en", "ta")
    /// * `gender` - Voice gender
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let speaker = ReverieSpeaker::new("hi", ReverieGender::Female);
    /// assert_eq!(speaker.as_code(), "hi_female");
    /// ```
    pub fn new(language: impl Into<String>, gender: ReverieGender) -> Self {
        Self {
            language: language.into(),
            gender,
            variant: 1,
        }
    }

    /// Create a speaker with a specific variant.
    pub fn with_variant(language: impl Into<String>, gender: ReverieGender, variant: u8) -> Self {
        Self {
            language: language.into(),
            gender,
            variant: variant.max(1),
        }
    }

    /// Get the speaker code for API requests.
    pub fn as_code(&self) -> String {
        if self.variant <= 1 {
            format!("{}_{}", self.language, self.gender.as_suffix())
        } else {
            format!("{}_{}_{}", self.language, self.gender.as_suffix(), self.variant)
        }
    }

    /// Parse a speaker code string.
    ///
    /// Supports formats:
    /// - `{lang}_{gender}` (e.g., "hi_female")
    /// - `{lang}_{gender}_{variant}` (e.g., "hi_male_2")
    pub fn from_code(code: &str) -> Result<Self, String> {
        let parts: Vec<&str> = code.split('_').collect();

        match parts.len() {
            2 => {
                let language = parts[0].to_string();
                let gender = parts[1].parse()?;
                Ok(Self {
                    language,
                    gender,
                    variant: 1,
                })
            }
            3 => {
                let language = parts[0].to_string();
                let gender = parts[1].parse()?;
                let variant = parts[2]
                    .parse()
                    .map_err(|_| format!("Invalid variant: {}", parts[2]))?;
                Ok(Self {
                    language,
                    gender,
                    variant,
                })
            }
            _ => Err(format!("Invalid speaker code format: {}", code)),
        }
    }

    /// Get Hindi female voice (default).
    pub fn hindi_female() -> Self {
        Self::new("hi", ReverieGender::Female)
    }

    /// Get Hindi male voice.
    pub fn hindi_male() -> Self {
        Self::new("hi", ReverieGender::Male)
    }

    /// Get English female voice.
    pub fn english_female() -> Self {
        Self::new("en", ReverieGender::Female)
    }

    /// Get English male voice.
    pub fn english_male() -> Self {
        Self::new("en", ReverieGender::Male)
    }

    /// Get Tamil female voice.
    pub fn tamil_female() -> Self {
        Self::new("ta", ReverieGender::Female)
    }

    /// Get Telugu female voice.
    pub fn telugu_female() -> Self {
        Self::new("te", ReverieGender::Female)
    }

    /// Get Bengali female voice.
    pub fn bengali_female() -> Self {
        Self::new("bn", ReverieGender::Female)
    }

    /// Get Kannada female voice.
    pub fn kannada_female() -> Self {
        Self::new("kn", ReverieGender::Female)
    }

    /// Get Malayalam female voice.
    pub fn malayalam_female() -> Self {
        Self::new("ml", ReverieGender::Female)
    }
}

impl Default for ReverieSpeaker {
    fn default() -> Self {
        Self::hindi_female()
    }
}

impl fmt::Display for ReverieSpeaker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_code())
    }
}

// =============================================================================
// TTS Configuration
// =============================================================================

/// Reverie TTS configuration.
///
/// This struct holds all configuration parameters for the Reverie TTS API.
#[derive(Debug, Clone)]
pub struct ReverieTtsConfig {
    /// Base TTS configuration (API key, timeouts, etc.).
    pub base: TTSConfig,
    /// Application ID for authentication.
    pub app_id: String,
    /// Speaker voice.
    pub speaker: ReverieSpeaker,
    /// Speech speed (0.5 to 1.5, default 1.0).
    pub speed: f32,
    /// Pitch adjustment in semitones (-3 to +3, default 0).
    pub pitch: f32,
    /// Output sample rate in Hz.
    pub sample_rate: u32,
    /// Output audio format.
    pub format: ReverieTtsAudioFormat,
}

impl ReverieTtsConfig {
    /// Create configuration from a base TTSConfig.
    ///
    /// # Arguments
    /// * `config` - Base TTS configuration
    ///
    /// The `api_key` field should contain the Reverie API key.
    /// The `model` field should contain the App ID (or use REVERIE_APP_ID env var).
    /// The `voice_id` field should contain the speaker code (e.g., "hi_female").
    pub fn from_base(config: TTSConfig) -> Result<Self, String> {
        // Validate API key
        if config.api_key.is_empty() {
            return Err("API key is required for Reverie TTS".to_string());
        }

        // Get App ID from model field or environment variable
        let app_id = if !config.model.is_empty() {
            config.model.clone()
        } else {
            std::env::var("REVERIE_APP_ID")
                .map_err(|_| "App ID is required: set via model field or REVERIE_APP_ID env var")?
        };

        // Parse speaker from voice_id
        let speaker = if let Some(ref voice_id) = config.voice_id {
            if !voice_id.is_empty() {
                ReverieSpeaker::from_code(voice_id)?
            } else {
                ReverieSpeaker::default()
            }
        } else {
            ReverieSpeaker::default()
        };

        // Parse audio format
        let format = if let Some(ref fmt) = config.audio_format {
            if !fmt.is_empty() {
                fmt.parse().unwrap_or_default()
            } else {
                ReverieTtsAudioFormat::default()
            }
        } else {
            ReverieTtsAudioFormat::default()
        };

        // Parse sample rate
        let sample_rate = config
            .sample_rate
            .filter(|&r| super::is_valid_sample_rate(r))
            .unwrap_or(super::DEFAULT_SAMPLE_RATE);

        // Parse speed from speaking_rate
        let speed = config
            .speaking_rate
            .map(|r| r.clamp(super::MIN_SPEED, super::MAX_SPEED))
            .unwrap_or(super::DEFAULT_SPEED);

        Ok(Self {
            base: config,
            app_id,
            speaker,
            speed,
            pitch: super::DEFAULT_PITCH,
            sample_rate,
            format,
        })
    }

    /// Set the speaker.
    pub fn with_speaker(mut self, speaker: ReverieSpeaker) -> Self {
        self.speaker = speaker;
        self
    }

    /// Set the speaker from a code string.
    pub fn with_speaker_code(mut self, code: &str) -> Result<Self, String> {
        self.speaker = ReverieSpeaker::from_code(code)?;
        Ok(self)
    }

    /// Set the speed.
    pub fn with_speed(mut self, speed: f32) -> Self {
        self.speed = speed.clamp(super::MIN_SPEED, super::MAX_SPEED);
        self
    }

    /// Set the pitch.
    pub fn with_pitch(mut self, pitch: f32) -> Self {
        self.pitch = pitch.clamp(super::MIN_PITCH, super::MAX_PITCH);
        self
    }

    /// Set the sample rate.
    pub fn with_sample_rate(mut self, rate: u32) -> Self {
        self.sample_rate = if super::is_valid_sample_rate(rate) {
            rate
        } else {
            super::nearest_sample_rate(rate)
        };
        self
    }

    /// Set the audio format.
    pub fn with_format(mut self, format: ReverieTtsAudioFormat) -> Self {
        self.format = format;
        self
    }

    /// Validate the configuration.
    pub fn validate(&self) -> Result<(), String> {
        if self.base.api_key.is_empty() {
            return Err("API key is required".to_string());
        }

        if self.app_id.is_empty() {
            return Err("App ID is required".to_string());
        }

        if self.speaker.language.is_empty() {
            return Err("Speaker language is required".to_string());
        }

        if self.speed < super::MIN_SPEED || self.speed > super::MAX_SPEED {
            return Err(format!(
                "Speed must be between {} and {}, got {}",
                super::MIN_SPEED,
                super::MAX_SPEED,
                self.speed
            ));
        }

        if self.pitch < super::MIN_PITCH || self.pitch > super::MAX_PITCH {
            return Err(format!(
                "Pitch must be between {} and {}, got {}",
                super::MIN_PITCH,
                super::MAX_PITCH,
                self.pitch
            ));
        }

        if !super::is_valid_sample_rate(self.sample_rate) {
            return Err(format!(
                "Invalid sample rate: {}. Supported: {:?}",
                self.sample_rate,
                super::SUPPORTED_SAMPLE_RATES
            ));
        }

        Ok(())
    }

    /// Get the speaker code.
    #[inline]
    pub fn speaker_code(&self) -> String {
        self.speaker.as_code()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tts::reverie::{MAX_PITCH, MAX_SPEED};

    fn create_test_config() -> TTSConfig {
        TTSConfig {
            provider: "reverie".to_string(),
            api_key: "test-api-key".to_string(),
            voice_id: Some("hi_female".to_string()),
            model: "test-app-id".to_string(),
            speaking_rate: Some(1.0),
            audio_format: Some("wav".to_string()),
            sample_rate: Some(22050),
            connection_timeout: Some(30),
            request_timeout: Some(60),
            pronunciations: Vec::new(),
            request_pool_size: Some(4),
            emotion_config: None,
        }
    }

    // =========================================================================
    // Audio Format Tests
    // =========================================================================

    #[test]
    fn test_audio_format_as_str() {
        assert_eq!(ReverieTtsAudioFormat::Wav.as_str(), "WAV");
        assert_eq!(ReverieTtsAudioFormat::Mp3.as_str(), "MP3");
    }

    #[test]
    fn test_audio_format_content_type() {
        assert_eq!(ReverieTtsAudioFormat::Wav.content_type(), "audio/wav");
        assert_eq!(ReverieTtsAudioFormat::Mp3.content_type(), "audio/mpeg");
    }

    #[test]
    fn test_audio_format_from_str() {
        assert_eq!(
            "wav".parse::<ReverieTtsAudioFormat>().unwrap(),
            ReverieTtsAudioFormat::Wav
        );
        assert_eq!(
            "mp3".parse::<ReverieTtsAudioFormat>().unwrap(),
            ReverieTtsAudioFormat::Mp3
        );
        assert_eq!(
            "linear16".parse::<ReverieTtsAudioFormat>().unwrap(),
            ReverieTtsAudioFormat::Wav
        );
        assert!("invalid".parse::<ReverieTtsAudioFormat>().is_err());
    }

    // =========================================================================
    // Gender Tests
    // =========================================================================

    #[test]
    fn test_gender_as_suffix() {
        assert_eq!(ReverieGender::Male.as_suffix(), "male");
        assert_eq!(ReverieGender::Female.as_suffix(), "female");
    }

    #[test]
    fn test_gender_from_str() {
        assert_eq!("male".parse::<ReverieGender>().unwrap(), ReverieGender::Male);
        assert_eq!("m".parse::<ReverieGender>().unwrap(), ReverieGender::Male);
        assert_eq!(
            "female".parse::<ReverieGender>().unwrap(),
            ReverieGender::Female
        );
        assert_eq!("f".parse::<ReverieGender>().unwrap(), ReverieGender::Female);
    }

    // =========================================================================
    // Speaker Tests
    // =========================================================================

    #[test]
    fn test_speaker_new() {
        let speaker = ReverieSpeaker::new("hi", ReverieGender::Female);
        assert_eq!(speaker.language, "hi");
        assert_eq!(speaker.gender, ReverieGender::Female);
        assert_eq!(speaker.variant, 1);
    }

    #[test]
    fn test_speaker_with_variant() {
        let speaker = ReverieSpeaker::with_variant("hi", ReverieGender::Male, 2);
        assert_eq!(speaker.language, "hi");
        assert_eq!(speaker.gender, ReverieGender::Male);
        assert_eq!(speaker.variant, 2);
    }

    #[test]
    fn test_speaker_as_code() {
        assert_eq!(
            ReverieSpeaker::new("hi", ReverieGender::Female).as_code(),
            "hi_female"
        );
        assert_eq!(
            ReverieSpeaker::new("en", ReverieGender::Male).as_code(),
            "en_male"
        );
        assert_eq!(
            ReverieSpeaker::with_variant("hi", ReverieGender::Male, 2).as_code(),
            "hi_male_2"
        );
    }

    #[test]
    fn test_speaker_from_code() {
        let speaker = ReverieSpeaker::from_code("hi_female").unwrap();
        assert_eq!(speaker.language, "hi");
        assert_eq!(speaker.gender, ReverieGender::Female);
        assert_eq!(speaker.variant, 1);

        let speaker = ReverieSpeaker::from_code("en_male_2").unwrap();
        assert_eq!(speaker.language, "en");
        assert_eq!(speaker.gender, ReverieGender::Male);
        assert_eq!(speaker.variant, 2);
    }

    #[test]
    fn test_speaker_from_code_invalid() {
        assert!(ReverieSpeaker::from_code("invalid").is_err());
        assert!(ReverieSpeaker::from_code("hi_invalid").is_err());
    }

    #[test]
    fn test_speaker_presets() {
        assert_eq!(ReverieSpeaker::hindi_female().as_code(), "hi_female");
        assert_eq!(ReverieSpeaker::hindi_male().as_code(), "hi_male");
        assert_eq!(ReverieSpeaker::english_female().as_code(), "en_female");
        assert_eq!(ReverieSpeaker::english_male().as_code(), "en_male");
        assert_eq!(ReverieSpeaker::tamil_female().as_code(), "ta_female");
    }

    // =========================================================================
    // Config Tests
    // =========================================================================

    #[test]
    fn test_config_from_base() {
        let base = create_test_config();
        let config = ReverieTtsConfig::from_base(base).unwrap();

        assert_eq!(config.app_id, "test-app-id");
        assert_eq!(config.speaker.language, "hi");
        assert_eq!(config.speaker.gender, ReverieGender::Female);
        assert!((config.speed - 1.0).abs() < 0.001);
        assert!((config.pitch - 0.0).abs() < 0.001);
        assert_eq!(config.sample_rate, 22050);
        assert_eq!(config.format, ReverieTtsAudioFormat::Wav);
    }

    #[test]
    fn test_config_from_base_missing_api_key() {
        let mut base = create_test_config();
        base.api_key = String::new();

        let result = ReverieTtsConfig::from_base(base);
        assert!(result.is_err());
    }

    #[test]
    fn test_config_with_speaker() {
        let base = create_test_config();
        let config = ReverieTtsConfig::from_base(base)
            .unwrap()
            .with_speaker(ReverieSpeaker::english_male());

        assert_eq!(config.speaker_code(), "en_male");
    }

    #[test]
    fn test_config_with_speed() {
        let base = create_test_config();
        let config = ReverieTtsConfig::from_base(base).unwrap().with_speed(1.3);

        assert!((config.speed - 1.3).abs() < 0.001);
    }

    #[test]
    fn test_config_with_speed_clamped() {
        let base = create_test_config();
        let config = ReverieTtsConfig::from_base(base).unwrap().with_speed(2.0);

        assert!((config.speed - MAX_SPEED).abs() < 0.001);
    }

    #[test]
    fn test_config_with_pitch() {
        let base = create_test_config();
        let config = ReverieTtsConfig::from_base(base).unwrap().with_pitch(1.5);

        assert!((config.pitch - 1.5).abs() < 0.001);
    }

    #[test]
    fn test_config_with_pitch_clamped() {
        let base = create_test_config();
        let config = ReverieTtsConfig::from_base(base).unwrap().with_pitch(5.0);

        assert!((config.pitch - MAX_PITCH).abs() < 0.001);
    }

    #[test]
    fn test_config_with_sample_rate() {
        let base = create_test_config();
        let config = ReverieTtsConfig::from_base(base)
            .unwrap()
            .with_sample_rate(16000);

        assert_eq!(config.sample_rate, 16000);
    }

    #[test]
    fn test_config_with_sample_rate_nearest() {
        let base = create_test_config();
        // 17000 is not valid, should snap to nearest (16000)
        let config = ReverieTtsConfig::from_base(base)
            .unwrap()
            .with_sample_rate(17000);

        assert_eq!(config.sample_rate, 16000);
    }

    #[test]
    fn test_config_validate_success() {
        let base = create_test_config();
        let config = ReverieTtsConfig::from_base(base).unwrap();

        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validate_invalid_speed() {
        let base = create_test_config();
        let mut config = ReverieTtsConfig::from_base(base).unwrap();
        config.speed = 0.1; // Below minimum

        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validate_invalid_pitch() {
        let base = create_test_config();
        let mut config = ReverieTtsConfig::from_base(base).unwrap();
        config.pitch = 5.0; // Above maximum

        assert!(config.validate().is_err());
    }
}
