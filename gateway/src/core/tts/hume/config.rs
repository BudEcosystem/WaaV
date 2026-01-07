//! Configuration types for Hume AI Octave Text-to-Speech API.
//!
//! This module contains all configuration-related types including:
//! - Audio format specifications (`HumeAudioFormat`)
//! - Output format configuration (`HumeOutputFormat`)
//! - Provider-specific configuration options (`HumeTTSConfig`)
//!
//! # API Overview
//!
//! Hume AI Octave TTS uses HTTP streaming at `POST https://api.hume.ai/v0/tts/stream/file`
//! with JSON request bodies. The key differentiator is natural language emotion instructions
//! via the `description` field (no SSML required).
//!
//! # Emotion Instructions
//!
//! Hume uses natural language "acting instructions" to control emotion:
//! - Max 100 characters
//! - Examples: "happy, energetic", "sad, melancholic", "whispered fearfully"
//! - Combined with speaking style and emotion together
//!
//! # Example
//!
//! ```rust,ignore
//! use waav_gateway::core::tts::hume::{HumeTTSConfig, HumeOutputFormat};
//! use waav_gateway::core::tts::TTSConfig;
//!
//! let base = TTSConfig {
//!     api_key: "your-api-key".to_string(),
//!     voice_id: Some("Kora".to_string()),
//!     audio_format: Some("linear16".to_string()),
//!     sample_rate: Some(24000),
//!     ..Default::default()
//! };
//!
//! let mut config = HumeTTSConfig::from_base(base);
//! config.description = Some("warm, friendly, inviting".to_string());
//! config.speed = Some(1.0);
//! ```

use crate::core::tts::base::{TTSConfig, TTSError};
use serde::{Deserialize, Serialize};
use tracing::warn;

// =============================================================================
// Constants
// =============================================================================

/// Hume AI TTS streaming REST API endpoint.
pub const HUME_TTS_STREAM_URL: &str = "https://api.hume.ai/v0/tts/stream/file";

/// Hume AI TTS synchronous REST API endpoint.
pub const HUME_TTS_SYNC_URL: &str = "https://api.hume.ai/v0/tts/file";

/// Default Hume voice name.
pub const DEFAULT_VOICE: &str = "Kora";

/// Default speaking speed (1.0 = normal).
pub const DEFAULT_SPEED: f32 = 1.0;

/// Maximum length for acting instructions (description).
pub const MAX_DESCRIPTION_LENGTH: usize = 100;

/// Supported sample rates for Hume TTS (in Hz).
pub const SUPPORTED_SAMPLE_RATES: &[u32] = &[8000, 16000, 22050, 24000, 44100, 48000];

/// Minimum speaking speed (API limit).
pub const MIN_SPEED: f32 = 0.5;

/// Maximum speaking speed (API limit).
pub const MAX_SPEED: f32 = 2.0;

/// Recommended minimum speed (per Hume documentation).
/// Values below this may produce poor quality audio.
pub const RECOMMENDED_MIN_SPEED: f32 = 0.75;

/// Recommended maximum speed (per Hume documentation).
/// Values above this may produce poor quality audio.
pub const RECOMMENDED_MAX_SPEED: f32 = 1.5;

// =============================================================================
// Audio Format
// =============================================================================

/// Audio format for Hume TTS output.
///
/// Hume supports multiple audio formats via the `format` field in the request.
///
/// # Formats
///
/// | Format | Description | Use Case |
/// |--------|-------------|----------|
/// | `Pcm16` | 16-bit signed PCM | Real-time streaming (default) |
/// | `Mp3` | MP3 compressed | Bandwidth optimization |
/// | `Wav` | WAV with headers | File storage |
/// | `Mulaw` | μ-law companding | US telephony |
/// | `Alaw` | A-law companding | European telephony |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HumeAudioFormat {
    /// 16-bit signed PCM - lowest latency for streaming (default).
    #[default]
    #[serde(rename = "pcm16")]
    Pcm16,
    /// MP3 compressed format.
    #[serde(rename = "mp3")]
    Mp3,
    /// WAV with headers.
    #[serde(rename = "wav")]
    Wav,
    /// μ-law companding for US telephony.
    #[serde(rename = "mulaw")]
    Mulaw,
    /// A-law companding for European telephony.
    #[serde(rename = "alaw")]
    Alaw,
}

impl HumeAudioFormat {
    /// Returns the API string representation.
    #[inline]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Pcm16 => "pcm16",
            Self::Mp3 => "mp3",
            Self::Wav => "wav",
            Self::Mulaw => "mulaw",
            Self::Alaw => "alaw",
        }
    }

    /// Returns bytes per sample for PCM formats.
    #[inline]
    pub const fn bytes_per_sample(&self) -> usize {
        match self {
            Self::Pcm16 => 2,
            Self::Mulaw | Self::Alaw => 1,
            Self::Mp3 | Self::Wav => 0, // Variable/container
        }
    }

    /// Returns true if this is a PCM format suitable for real-time streaming.
    #[inline]
    pub const fn is_pcm(&self) -> bool {
        matches!(self, Self::Pcm16)
    }

    /// Returns true if this is a telephony format.
    #[inline]
    pub const fn is_telephony(&self) -> bool {
        matches!(self, Self::Mulaw | Self::Alaw)
    }

    /// Returns the MIME content type for Accept header.
    #[inline]
    pub const fn content_type(&self) -> &'static str {
        match self {
            Self::Pcm16 => "audio/pcm",
            Self::Mp3 => "audio/mpeg",
            Self::Wav => "audio/wav",
            Self::Mulaw => "audio/basic",
            Self::Alaw => "audio/basic",
        }
    }

    /// Maps WaaV Gateway format strings to Hume format.
    pub fn from_format_string(format: &str) -> Self {
        match format.to_lowercase().as_str() {
            "linear16" | "pcm" | "pcm16" => Self::Pcm16,
            "mp3" => Self::Mp3,
            "wav" => Self::Wav,
            "mulaw" | "ulaw" => Self::Mulaw,
            "alaw" => Self::Alaw,
            _ => Self::default(),
        }
    }
}

// =============================================================================
// Output Format
// =============================================================================

/// Complete output format specification for Hume TTS requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HumeOutputFormat {
    /// Audio format type.
    pub format: HumeAudioFormat,

    /// Sample rate in Hz.
    pub sample_rate: u32,
}

impl HumeOutputFormat {
    /// Create a new output format.
    pub fn new(format: HumeAudioFormat, sample_rate: u32) -> Self {
        Self {
            format,
            sample_rate,
        }
    }

    /// Create a PCM16 format (default for streaming).
    pub fn pcm16(sample_rate: u32) -> Self {
        Self::new(HumeAudioFormat::Pcm16, sample_rate)
    }

    /// Create an MP3 format.
    pub fn mp3(sample_rate: u32) -> Self {
        Self::new(HumeAudioFormat::Mp3, sample_rate)
    }

    /// Create from WaaV Gateway format string.
    pub fn from_format_string(format: &str, sample_rate: u32) -> Self {
        Self::new(HumeAudioFormat::from_format_string(format), sample_rate)
    }

    /// Validates the output format configuration.
    pub fn validate(&self) -> Result<(), TTSError> {
        if !SUPPORTED_SAMPLE_RATES.contains(&self.sample_rate) {
            return Err(TTSError::InvalidConfiguration(format!(
                "Unsupported sample rate: {}. Supported rates: {:?}",
                self.sample_rate, SUPPORTED_SAMPLE_RATES
            )));
        }
        Ok(())
    }
}

impl Default for HumeOutputFormat {
    fn default() -> Self {
        Self::pcm16(24000)
    }
}

// =============================================================================
// Predefined Voices
// =============================================================================

/// Predefined Hume Octave voices.
///
/// Hume provides several high-quality predefined voices.
/// Users can also create custom voices via voice cloning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HumeVoice {
    /// Kora - Default female voice, warm and natural.
    Kora,
    /// Custom voice by name.
    Custom(String),
}

impl HumeVoice {
    /// Returns the voice name string.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Kora => "Kora",
            Self::Custom(name) => name,
        }
    }

    /// Parse a voice from string.
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "kora" => Self::Kora,
            _ => Self::Custom(s.to_string()),
        }
    }
}

impl Default for HumeVoice {
    fn default() -> Self {
        Self::Kora
    }
}

// =============================================================================
// Main Configuration
// =============================================================================

/// Configuration specific to Hume AI Octave Text-to-Speech API.
///
/// This configuration extends the base `TTSConfig` with Hume-specific
/// parameters for emotion control via natural language instructions.
///
/// # Key Features
///
/// - **Acting Instructions**: Natural language emotion/style control via `description`
/// - **Speed Control**: 0.5 to 2.0 range
/// - **Instant Mode**: Low-latency streaming (default: true)
/// - **Context Continuity**: `generation_id` for consistent voice across utterances
///
/// # Example
///
/// ```rust,ignore
/// use waav_gateway::core::tts::hume::HumeTTSConfig;
/// use waav_gateway::core::tts::TTSConfig;
///
/// let base = TTSConfig {
///     api_key: "your-api-key".to_string(),
///     voice_id: Some("Kora".to_string()),
///     ..Default::default()
/// };
///
/// let mut config = HumeTTSConfig::from_base(base);
/// config.description = Some("excited, energetic".to_string());
/// config.speed = Some(1.2);
/// config.instant_mode = true;
/// ```
#[derive(Debug, Clone)]
pub struct HumeTTSConfig {
    /// Base TTS configuration (shared across all providers).
    pub base: TTSConfig,

    /// Predefined or custom voice to use.
    pub voice: HumeVoice,

    /// Natural language voice description for voice design.
    /// Used when creating custom voice characteristics.
    /// Example: "A warm, friendly voice with a slight British accent"
    pub voice_description: Option<String>,

    /// Acting instructions for emotion and style (max 100 chars).
    /// Examples: "happy, energetic", "sad, melancholic", "whispered fearfully"
    pub description: Option<String>,

    /// Speaking speed (0.5 to 2.0, default 1.0).
    pub speed: Option<f32>,

    /// Trailing silence duration in seconds after audio.
    pub trailing_silence: Option<f32>,

    /// Enable instant mode for low-latency streaming (default: true).
    pub instant_mode: bool,

    /// Generation ID for context continuity across utterances.
    /// Ensures consistent voice characteristics in multi-turn conversations.
    pub generation_id: Option<String>,

    /// Output format specification.
    pub output_format: HumeOutputFormat,

    /// Number of audio variations to generate (1-3).
    /// Higher values increase latency but provide options.
    pub num_generations: Option<u8>,
}

impl HumeTTSConfig {
    /// Creates a HumeTTSConfig from a base TTSConfig with default Hume settings.
    pub fn from_base(base: TTSConfig) -> Self {
        let sample_rate = base.sample_rate.unwrap_or(24000);
        let output_format = base
            .audio_format
            .as_deref()
            .map(|f| HumeOutputFormat::from_format_string(f, sample_rate))
            .unwrap_or_default();

        // Parse voice from voice_id
        let voice = base
            .voice_id
            .as_deref()
            .map(HumeVoice::from_str)
            .unwrap_or_default();

        // Map speaking_rate to Hume's speed range
        let speed = base.speaking_rate.map(|rate| {
            // Clamp to Hume's valid range
            rate.clamp(MIN_SPEED, MAX_SPEED)
        });

        Self {
            base,
            voice,
            voice_description: None,
            description: None,
            speed,
            trailing_silence: None,
            instant_mode: true,
            generation_id: None,
            output_format,
            num_generations: None,
        }
    }

    /// Validates the configuration for API compatibility.
    pub fn validate(&self) -> Result<(), TTSError> {
        // Check API key
        if self.base.api_key.is_empty() {
            return Err(TTSError::InvalidConfiguration(
                "Hume API key is required".to_string(),
            ));
        }

        // Validate description length
        if let Some(desc) = &self.description {
            if desc.len() > MAX_DESCRIPTION_LENGTH {
                return Err(TTSError::InvalidConfiguration(format!(
                    "Acting instructions (description) must be {} characters or less, got {}",
                    MAX_DESCRIPTION_LENGTH,
                    desc.len()
                )));
            }
        }

        // Validate speed range
        if let Some(speed) = self.speed {
            if !(MIN_SPEED..=MAX_SPEED).contains(&speed) {
                return Err(TTSError::InvalidConfiguration(format!(
                    "Speed must be between {} and {}, got {}",
                    MIN_SPEED, MAX_SPEED, speed
                )));
            }
            // Warn about values outside recommended range
            if !(RECOMMENDED_MIN_SPEED..=RECOMMENDED_MAX_SPEED).contains(&speed) {
                warn!(
                    speed = speed,
                    recommended_min = RECOMMENDED_MIN_SPEED,
                    recommended_max = RECOMMENDED_MAX_SPEED,
                    "Hume TTS speed {} is outside the recommended range ({}-{}). \
                     Audio quality may be degraded.",
                    speed,
                    RECOMMENDED_MIN_SPEED,
                    RECOMMENDED_MAX_SPEED
                );
            }
        }

        // Validate num_generations
        if let Some(num) = self.num_generations {
            if !(1..=3).contains(&num) {
                return Err(TTSError::InvalidConfiguration(format!(
                    "num_generations must be between 1 and 3, got {}",
                    num
                )));
            }
        }

        // Validate output format
        self.output_format.validate()?;

        Ok(())
    }

    /// Returns the voice name for API requests.
    #[inline]
    pub fn voice_name(&self) -> &str {
        self.voice.as_str()
    }

    /// Sets the acting instructions (emotion/style description).
    ///
    /// # Arguments
    /// * `description` - Natural language description (max 100 chars)
    ///
    /// # Examples
    /// - "happy, energetic"
    /// - "sad, melancholic"
    /// - "whispered fearfully"
    /// - "warm, inviting, professional"
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        let desc = description.into();
        // Truncate if too long
        self.description = Some(if desc.len() > MAX_DESCRIPTION_LENGTH {
            desc[..MAX_DESCRIPTION_LENGTH].to_string()
        } else {
            desc
        });
        self
    }

    /// Sets the speaking speed.
    pub fn with_speed(mut self, speed: f32) -> Self {
        self.speed = Some(speed.clamp(MIN_SPEED, MAX_SPEED));
        self
    }

    /// Sets instant mode for low-latency streaming.
    pub fn with_instant_mode(mut self, enabled: bool) -> Self {
        self.instant_mode = enabled;
        self
    }

    /// Sets trailing silence duration.
    pub fn with_trailing_silence(mut self, seconds: f32) -> Self {
        self.trailing_silence = Some(seconds.max(0.0));
        self
    }

    /// Sets generation ID for context continuity.
    pub fn with_generation_id(mut self, id: impl Into<String>) -> Self {
        self.generation_id = Some(id.into());
        self
    }
}

impl Default for HumeTTSConfig {
    fn default() -> Self {
        Self {
            base: TTSConfig::default(),
            voice: HumeVoice::default(),
            voice_description: None,
            description: None,
            speed: Some(DEFAULT_SPEED),
            trailing_silence: None,
            instant_mode: true,
            generation_id: None,
            output_format: HumeOutputFormat::default(),
            num_generations: None,
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // HumeAudioFormat Tests
    // =========================================================================

    #[test]
    fn test_audio_format_as_str() {
        assert_eq!(HumeAudioFormat::Pcm16.as_str(), "pcm16");
        assert_eq!(HumeAudioFormat::Mp3.as_str(), "mp3");
        assert_eq!(HumeAudioFormat::Wav.as_str(), "wav");
        assert_eq!(HumeAudioFormat::Mulaw.as_str(), "mulaw");
        assert_eq!(HumeAudioFormat::Alaw.as_str(), "alaw");
    }

    #[test]
    fn test_audio_format_default_is_pcm16() {
        assert_eq!(HumeAudioFormat::default(), HumeAudioFormat::Pcm16);
    }

    #[test]
    fn test_audio_format_bytes_per_sample() {
        assert_eq!(HumeAudioFormat::Pcm16.bytes_per_sample(), 2);
        assert_eq!(HumeAudioFormat::Mulaw.bytes_per_sample(), 1);
        assert_eq!(HumeAudioFormat::Alaw.bytes_per_sample(), 1);
        assert_eq!(HumeAudioFormat::Mp3.bytes_per_sample(), 0);
        assert_eq!(HumeAudioFormat::Wav.bytes_per_sample(), 0);
    }

    #[test]
    fn test_audio_format_is_pcm() {
        assert!(HumeAudioFormat::Pcm16.is_pcm());
        assert!(!HumeAudioFormat::Mp3.is_pcm());
        assert!(!HumeAudioFormat::Wav.is_pcm());
        assert!(!HumeAudioFormat::Mulaw.is_pcm());
        assert!(!HumeAudioFormat::Alaw.is_pcm());
    }

    #[test]
    fn test_audio_format_is_telephony() {
        assert!(!HumeAudioFormat::Pcm16.is_telephony());
        assert!(!HumeAudioFormat::Mp3.is_telephony());
        assert!(!HumeAudioFormat::Wav.is_telephony());
        assert!(HumeAudioFormat::Mulaw.is_telephony());
        assert!(HumeAudioFormat::Alaw.is_telephony());
    }

    #[test]
    fn test_audio_format_content_type() {
        assert_eq!(HumeAudioFormat::Pcm16.content_type(), "audio/pcm");
        assert_eq!(HumeAudioFormat::Mp3.content_type(), "audio/mpeg");
        assert_eq!(HumeAudioFormat::Wav.content_type(), "audio/wav");
        assert_eq!(HumeAudioFormat::Mulaw.content_type(), "audio/basic");
        assert_eq!(HumeAudioFormat::Alaw.content_type(), "audio/basic");
    }

    #[test]
    fn test_audio_format_from_format_string() {
        assert_eq!(
            HumeAudioFormat::from_format_string("linear16"),
            HumeAudioFormat::Pcm16
        );
        assert_eq!(
            HumeAudioFormat::from_format_string("pcm"),
            HumeAudioFormat::Pcm16
        );
        assert_eq!(
            HumeAudioFormat::from_format_string("pcm16"),
            HumeAudioFormat::Pcm16
        );
        assert_eq!(
            HumeAudioFormat::from_format_string("mp3"),
            HumeAudioFormat::Mp3
        );
        assert_eq!(
            HumeAudioFormat::from_format_string("wav"),
            HumeAudioFormat::Wav
        );
        assert_eq!(
            HumeAudioFormat::from_format_string("mulaw"),
            HumeAudioFormat::Mulaw
        );
        assert_eq!(
            HumeAudioFormat::from_format_string("ulaw"),
            HumeAudioFormat::Mulaw
        );
        assert_eq!(
            HumeAudioFormat::from_format_string("alaw"),
            HumeAudioFormat::Alaw
        );
        // Unknown defaults to Pcm16
        assert_eq!(
            HumeAudioFormat::from_format_string("unknown"),
            HumeAudioFormat::Pcm16
        );
    }

    #[test]
    fn test_audio_format_from_format_string_case_insensitive() {
        assert_eq!(
            HumeAudioFormat::from_format_string("LINEAR16"),
            HumeAudioFormat::Pcm16
        );
        assert_eq!(
            HumeAudioFormat::from_format_string("MP3"),
            HumeAudioFormat::Mp3
        );
    }

    #[test]
    fn test_audio_format_serialization() {
        assert_eq!(
            serde_json::to_string(&HumeAudioFormat::Pcm16).unwrap(),
            "\"pcm16\""
        );
        assert_eq!(
            serde_json::to_string(&HumeAudioFormat::Mp3).unwrap(),
            "\"mp3\""
        );
    }

    #[test]
    fn test_audio_format_deserialization() {
        assert_eq!(
            serde_json::from_str::<HumeAudioFormat>("\"pcm16\"").unwrap(),
            HumeAudioFormat::Pcm16
        );
        assert_eq!(
            serde_json::from_str::<HumeAudioFormat>("\"mp3\"").unwrap(),
            HumeAudioFormat::Mp3
        );
    }

    // =========================================================================
    // HumeOutputFormat Tests
    // =========================================================================

    #[test]
    fn test_output_format_new() {
        let format = HumeOutputFormat::new(HumeAudioFormat::Pcm16, 24000);
        assert_eq!(format.format, HumeAudioFormat::Pcm16);
        assert_eq!(format.sample_rate, 24000);
    }

    #[test]
    fn test_output_format_pcm16() {
        let format = HumeOutputFormat::pcm16(16000);
        assert_eq!(format.format, HumeAudioFormat::Pcm16);
        assert_eq!(format.sample_rate, 16000);
    }

    #[test]
    fn test_output_format_mp3() {
        let format = HumeOutputFormat::mp3(24000);
        assert_eq!(format.format, HumeAudioFormat::Mp3);
        assert_eq!(format.sample_rate, 24000);
    }

    #[test]
    fn test_output_format_default() {
        let format = HumeOutputFormat::default();
        assert_eq!(format.format, HumeAudioFormat::Pcm16);
        assert_eq!(format.sample_rate, 24000);
    }

    #[test]
    fn test_output_format_validate_success() {
        let format = HumeOutputFormat::pcm16(24000);
        assert!(format.validate().is_ok());
    }

    #[test]
    fn test_output_format_validate_invalid_sample_rate() {
        let format = HumeOutputFormat::new(HumeAudioFormat::Pcm16, 12345);
        let result = format.validate();
        assert!(result.is_err());
        assert!(matches!(result, Err(TTSError::InvalidConfiguration(_))));
    }

    // =========================================================================
    // HumeVoice Tests
    // =========================================================================

    #[test]
    fn test_voice_default() {
        assert_eq!(HumeVoice::default(), HumeVoice::Kora);
    }

    #[test]
    fn test_voice_as_str() {
        assert_eq!(HumeVoice::Kora.as_str(), "Kora");
        assert_eq!(HumeVoice::Custom("MyVoice".to_string()).as_str(), "MyVoice");
    }

    #[test]
    fn test_voice_from_str() {
        assert_eq!(HumeVoice::from_str("kora"), HumeVoice::Kora);
        assert_eq!(HumeVoice::from_str("KORA"), HumeVoice::Kora);
        assert_eq!(
            HumeVoice::from_str("CustomVoice"),
            HumeVoice::Custom("CustomVoice".to_string())
        );
    }

    // =========================================================================
    // HumeTTSConfig Tests
    // =========================================================================

    #[test]
    fn test_config_default() {
        let config = HumeTTSConfig::default();

        assert_eq!(config.voice, HumeVoice::Kora);
        assert!(config.description.is_none());
        assert_eq!(config.speed, Some(DEFAULT_SPEED));
        assert!(config.instant_mode);
        assert_eq!(config.output_format.format, HumeAudioFormat::Pcm16);
        assert_eq!(config.output_format.sample_rate, 24000);
    }

    #[test]
    fn test_config_from_base() {
        let base = TTSConfig {
            api_key: "test-api-key".to_string(),
            voice_id: Some("Kora".to_string()),
            audio_format: Some("mp3".to_string()),
            sample_rate: Some(24000),
            speaking_rate: Some(1.5),
            ..Default::default()
        };

        let config = HumeTTSConfig::from_base(base);

        assert_eq!(config.base.api_key, "test-api-key");
        assert_eq!(config.voice, HumeVoice::Kora);
        assert_eq!(config.output_format.format, HumeAudioFormat::Mp3);
        assert_eq!(config.speed, Some(1.5));
    }

    #[test]
    fn test_config_from_base_clamps_speed() {
        let mut base = TTSConfig::default();
        base.speaking_rate = Some(3.0); // Over max

        let config = HumeTTSConfig::from_base(base);

        assert_eq!(config.speed, Some(MAX_SPEED));
    }

    #[test]
    fn test_config_validate_success() {
        let base = TTSConfig {
            api_key: "test-api-key".to_string(),
            voice_id: Some("Kora".to_string()),
            sample_rate: Some(24000),
            ..Default::default()
        };

        let config = HumeTTSConfig::from_base(base);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validate_empty_api_key() {
        let base = TTSConfig {
            api_key: String::new(),
            ..Default::default()
        };

        let config = HumeTTSConfig::from_base(base);
        let result = config.validate();

        assert!(result.is_err());
        assert!(matches!(result, Err(TTSError::InvalidConfiguration(_))));
    }

    #[test]
    fn test_config_validate_description_too_long() {
        let base = TTSConfig {
            api_key: "test-key".to_string(),
            ..Default::default()
        };

        let mut config = HumeTTSConfig::from_base(base);
        config.description = Some("a".repeat(MAX_DESCRIPTION_LENGTH + 1));

        let result = config.validate();
        assert!(result.is_err());
    }

    #[test]
    fn test_config_validate_invalid_speed() {
        let base = TTSConfig {
            api_key: "test-key".to_string(),
            ..Default::default()
        };

        let mut config = HumeTTSConfig::from_base(base);
        config.speed = Some(0.1); // Below min

        let result = config.validate();
        assert!(result.is_err());
    }

    #[test]
    fn test_config_validate_invalid_num_generations() {
        let base = TTSConfig {
            api_key: "test-key".to_string(),
            ..Default::default()
        };

        let mut config = HumeTTSConfig::from_base(base);
        config.num_generations = Some(5); // Over max

        let result = config.validate();
        assert!(result.is_err());
    }

    #[test]
    fn test_config_voice_name() {
        let base = TTSConfig {
            voice_id: Some("CustomVoice".to_string()),
            ..Default::default()
        };

        let config = HumeTTSConfig::from_base(base);
        assert_eq!(config.voice_name(), "CustomVoice");
    }

    #[test]
    fn test_config_with_description() {
        let config = HumeTTSConfig::default().with_description("happy, energetic");
        assert_eq!(config.description, Some("happy, energetic".to_string()));
    }

    #[test]
    fn test_config_with_description_truncates() {
        let long_desc = "a".repeat(150);
        let config = HumeTTSConfig::default().with_description(long_desc);
        assert_eq!(
            config.description.as_ref().unwrap().len(),
            MAX_DESCRIPTION_LENGTH
        );
    }

    #[test]
    fn test_config_with_speed() {
        let config = HumeTTSConfig::default().with_speed(1.5);
        assert_eq!(config.speed, Some(1.5));
    }

    #[test]
    fn test_config_with_speed_clamps() {
        let config = HumeTTSConfig::default().with_speed(5.0);
        assert_eq!(config.speed, Some(MAX_SPEED));

        let config = HumeTTSConfig::default().with_speed(0.1);
        assert_eq!(config.speed, Some(MIN_SPEED));
    }

    #[test]
    fn test_config_with_instant_mode() {
        let config = HumeTTSConfig::default().with_instant_mode(false);
        assert!(!config.instant_mode);
    }

    #[test]
    fn test_config_with_trailing_silence() {
        let config = HumeTTSConfig::default().with_trailing_silence(0.5);
        assert_eq!(config.trailing_silence, Some(0.5));
    }

    #[test]
    fn test_config_with_trailing_silence_clamps_negative() {
        let config = HumeTTSConfig::default().with_trailing_silence(-1.0);
        assert_eq!(config.trailing_silence, Some(0.0));
    }

    #[test]
    fn test_config_with_generation_id() {
        let config = HumeTTSConfig::default().with_generation_id("gen-123");
        assert_eq!(config.generation_id, Some("gen-123".to_string()));
    }

    // =========================================================================
    // Constants Tests
    // =========================================================================

    #[test]
    fn test_constants() {
        assert_eq!(
            HUME_TTS_STREAM_URL,
            "https://api.hume.ai/v0/tts/stream/file"
        );
        assert_eq!(HUME_TTS_SYNC_URL, "https://api.hume.ai/v0/tts/file");
        assert_eq!(DEFAULT_VOICE, "Kora");
        assert_eq!(DEFAULT_SPEED, 1.0);
        assert_eq!(MAX_DESCRIPTION_LENGTH, 100);
        assert_eq!(MIN_SPEED, 0.5);
        assert_eq!(MAX_SPEED, 2.0);
        assert_eq!(
            SUPPORTED_SAMPLE_RATES,
            &[8000, 16000, 22050, 24000, 44100, 48000]
        );
    }
}
