//! Speechmatics TTS Configuration
//!
//! This module provides configuration types for the Speechmatics TTS provider.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

use crate::core::tts::base::{TTSConfig, TTSError, TTSResult};

// =============================================================================
// Voice Enum
// =============================================================================

/// Speechmatics TTS Voice options
///
/// Currently supports 4 English voices (preview):
/// - Sarah (Female, UK English)
/// - Theo (Male, UK English)
/// - Megan (Female, US English)
/// - Jack (Male, US English)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SpeechmaticsVoice {
    /// Sarah - Female UK English voice
    #[default]
    Sarah,
    /// Theo - Male UK English voice
    Theo,
    /// Megan - Female US English voice
    Megan,
    /// Jack - Male US English voice
    Jack,
}

impl SpeechmaticsVoice {
    /// Get the voice ID string for API requests
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Sarah => "sarah",
            Self::Theo => "theo",
            Self::Megan => "megan",
            Self::Jack => "jack",
        }
    }

    /// Get all available voices
    pub fn all() -> &'static [Self] {
        &[Self::Sarah, Self::Theo, Self::Megan, Self::Jack]
    }

    /// Get voice description
    pub fn description(&self) -> &'static str {
        match self {
            Self::Sarah => "Female UK English voice",
            Self::Theo => "Male UK English voice",
            Self::Megan => "Female US English voice",
            Self::Jack => "Male US English voice",
        }
    }

    /// Get voice gender
    pub fn gender(&self) -> &'static str {
        match self {
            Self::Sarah | Self::Megan => "female",
            Self::Theo | Self::Jack => "male",
        }
    }

    /// Get voice accent
    pub fn accent(&self) -> &'static str {
        match self {
            Self::Sarah | Self::Theo => "UK",
            Self::Megan | Self::Jack => "US",
        }
    }
}

impl fmt::Display for SpeechmaticsVoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for SpeechmaticsVoice {
    type Err = TTSError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "sarah" => Ok(Self::Sarah),
            "theo" => Ok(Self::Theo),
            "megan" => Ok(Self::Megan),
            "jack" => Ok(Self::Jack),
            _ => Err(TTSError::InvalidConfiguration(format!(
                "Invalid Speechmatics voice '{}'. Valid options: sarah, theo, megan, jack",
                s
            ))),
        }
    }
}

// =============================================================================
// Output Format Enum
// =============================================================================

/// Speechmatics TTS output format options
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum SpeechmaticsOutputFormat {
    /// WAV format at 16kHz
    #[default]
    Wav16000,
    /// Raw PCM format at 16kHz (little-endian, 16-bit signed)
    Pcm16000,
}

impl SpeechmaticsOutputFormat {
    /// Get the format string for API requests
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Wav16000 => "wav_16000",
            Self::Pcm16000 => "pcm_16000",
        }
    }

    /// Get sample rate
    pub fn sample_rate(&self) -> u32 {
        match self {
            Self::Wav16000 | Self::Pcm16000 => 16000,
        }
    }

    /// Get bit depth
    pub fn bit_depth(&self) -> u8 {
        16
    }

    /// Get channels
    pub fn channels(&self) -> u8 {
        1 // Mono
    }

    /// Check if format includes WAV header
    pub fn has_header(&self) -> bool {
        matches!(self, Self::Wav16000)
    }
}

impl fmt::Display for SpeechmaticsOutputFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for SpeechmaticsOutputFormat {
    type Err = TTSError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "wav_16000" | "wav" => Ok(Self::Wav16000),
            "pcm_16000" | "pcm" | "raw" => Ok(Self::Pcm16000),
            _ => Err(TTSError::InvalidConfiguration(format!(
                "Invalid Speechmatics output format '{}'. Valid options: wav_16000, pcm_16000",
                s
            ))),
        }
    }
}

// =============================================================================
// TTS Configuration
// =============================================================================

/// Speechmatics TTS-specific configuration
#[derive(Debug, Clone)]
#[derive(Default)]
pub struct SpeechmaticsTtsConfig {
    /// API key for authentication
    pub api_key: String,
    /// Voice to use for synthesis
    pub voice: SpeechmaticsVoice,
    /// Output audio format
    pub output_format: SpeechmaticsOutputFormat,
    /// Override base (scheme+host) for the synth POST, used to redirect to a mock in tests.
    pub endpoint_override: Option<String>,
}

impl SpeechmaticsTtsConfig {
    /// Create new configuration with default values
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            voice: SpeechmaticsVoice::default(),
            output_format: SpeechmaticsOutputFormat::default(),
            endpoint_override: None,
        }
    }

    /// Set the voice
    pub fn with_voice(mut self, voice: SpeechmaticsVoice) -> Self {
        self.voice = voice;
        self
    }

    /// Set the output format
    pub fn with_output_format(mut self, format: SpeechmaticsOutputFormat) -> Self {
        self.output_format = format;
        self
    }

    /// Get the generate URL for the configured voice
    pub fn generate_url(&self) -> String {
        format!(
            "{}/{}",
            super::SPEECHMATICS_GENERATE_URL,
            self.voice.as_str()
        )
    }

    /// Get the full URL with query parameters
    pub fn full_url(&self) -> String {
        format!(
            "{}?output_format={}",
            self.generate_url(),
            self.output_format.as_str()
        )
    }

    /// Validate the configuration
    pub fn validate(&self) -> TTSResult<()> {
        if self.api_key.is_empty() {
            return Err(TTSError::InvalidConfiguration(
                "Speechmatics API key is required. Set SPEECHMATICS_API_KEY environment variable."
                    .to_string(),
            ));
        }
        Ok(())
    }

    /// Create from base TTSConfig
    pub fn from_base(config: &TTSConfig) -> TTSResult<Self> {
        // Get API key
        let api_key = if !config.api_key.is_empty() {
            config.api_key.clone()
        } else {
            std::env::var("SPEECHMATICS_API_KEY").unwrap_or_default()
        };

        if api_key.is_empty() {
            return Err(TTSError::InvalidConfiguration(
                "Speechmatics API key is required. Set SPEECHMATICS_API_KEY environment variable."
                    .to_string(),
            ));
        }

        // Parse voice from voice_id
        // Note: TTSConfig::Default uses "aura-asteria-en" which is not valid for Speechmatics,
        // so we fall back to the default Speechmatics voice (sarah)
        let voice = if let Some(ref voice_id) = config.voice_id {
            SpeechmaticsVoice::from_str(voice_id).unwrap_or_default()
        } else {
            SpeechmaticsVoice::default()
        };

        // Parse output format from audio_format
        // Note: TTSConfig::Default uses "linear16" which is not valid for Speechmatics,
        // so we fall back to the default Speechmatics format (wav_16000)
        let output_format = if let Some(ref format) = config.audio_format {
            SpeechmaticsOutputFormat::from_str(format).unwrap_or_default()
        } else {
            SpeechmaticsOutputFormat::default()
        };

        let cfg = Self {
            api_key,
            voice,
            output_format,
            endpoint_override: None,
        };

        cfg.validate()?;
        Ok(cfg)
    }

    /// Build from the standardized config (W1 keystone). Speechmatics' generate endpoint only
    /// accepts a voice (path segment) and an `output_format` query parameter — both already
    /// derived from the base config by [`from_base`] — and its request body carries nothing but the
    /// text. The two supported formats are fixed at 16 kHz mono, so there is no prosody, voice
    /// settings, emotion, instructions, SSML, language, timestamp, streaming, seed or even
    /// adjustable sample-rate surface to map. Every standardized [`TtsFeatures`] field is therefore
    /// a capability gap, and this is a pure `from_base` passthrough.
    ///
    /// [`from_base`]: Self::from_base
    /// [`TtsFeatures`]: crate::core::tts::standard::TtsFeatures
    pub fn from_standard(std: &crate::core::tts::standard::StandardTTSConfig) -> TTSResult<Self> {
        let mut cfg = Self::from_base(&std.base)?;
        cfg.endpoint_override = std.endpoint_override().map(String::from);
        Ok(cfg)
    }

    /// Validate text length for synthesis
    pub fn validate_text(text: &str) -> TTSResult<()> {
        if text.len() > super::MAX_TEXT_LENGTH {
            return Err(TTSError::InvalidConfiguration(format!(
                "Text exceeds maximum length of {} characters for Speechmatics TTS",
                super::MAX_TEXT_LENGTH
            )));
        }
        Ok(())
    }
}


// =============================================================================
// Request Types
// =============================================================================

/// Request body for Speechmatics TTS generate endpoint
#[derive(Debug, Clone, Serialize)]
pub struct SpeechmaticsGenerateRequest {
    /// Text to synthesize
    pub text: String,
}

impl SpeechmaticsGenerateRequest {
    /// Create a new request
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }

    /// Create from config and text
    pub fn from_config(_config: &SpeechmaticsTtsConfig, text: &str) -> Self {
        Self::new(text)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // W1 keystone (TTS): Speechmatics exposes only voice + output_format (both from the base) and a
    // text-only body, so `from_standard` is a pure `from_base` passthrough — every standardized
    // feature is a capability gap and must be ignored while the base voice/format still flow through.
    #[test]
    fn from_standard_passes_base_through_ignoring_features() {
        use crate::core::tts::standard::{ProviderExtras, StandardTTSConfig, TtsFeatures};

        let mut extras = serde_json::Map::new();
        extras.insert("unsupported".into(), serde_json::json!("ignored"));
        let std = StandardTTSConfig {
            base: TTSConfig {
                provider: "speechmatics".into(),
                api_key: "test-api-key".into(),
                voice_id: Some("jack".into()),
                audio_format: Some("pcm".into()),
                ..Default::default()
            },
            features: TtsFeatures {
                // None of these have a Speechmatics surface — all must be ignored.
                speed: Some(1.5),
                pitch: Some(2.0),
                volume: Some(80.0),
                emotion: Some("cheerful".into()),
                instructions: Some("speak slowly".into()),
                ssml: Some(true),
                language: Some("en".into()),
                sample_rate: Some(48000),
                ..Default::default()
            },
            extras: ProviderExtras(extras),
        };

        let cfg = SpeechmaticsTtsConfig::from_standard(&std).unwrap();
        // Base-derived fields still flow through unchanged.
        assert_eq!(cfg.api_key, "test-api-key");
        assert_eq!(cfg.voice, SpeechmaticsVoice::Jack);
        assert_eq!(cfg.output_format, SpeechmaticsOutputFormat::Pcm16000);
        // Output stays fixed at 16 kHz: the sample_rate feature has no field to land in.
        assert_eq!(cfg.output_format.sample_rate(), 16000);
    }

    #[test]
    fn test_voice_as_str() {
        assert_eq!(SpeechmaticsVoice::Sarah.as_str(), "sarah");
        assert_eq!(SpeechmaticsVoice::Theo.as_str(), "theo");
        assert_eq!(SpeechmaticsVoice::Megan.as_str(), "megan");
        assert_eq!(SpeechmaticsVoice::Jack.as_str(), "jack");
    }

    #[test]
    fn test_voice_from_str() {
        assert_eq!(
            SpeechmaticsVoice::from_str("sarah").unwrap(),
            SpeechmaticsVoice::Sarah
        );
        assert_eq!(
            SpeechmaticsVoice::from_str("THEO").unwrap(),
            SpeechmaticsVoice::Theo
        );
        assert_eq!(
            SpeechmaticsVoice::from_str("Megan").unwrap(),
            SpeechmaticsVoice::Megan
        );
        assert_eq!(
            SpeechmaticsVoice::from_str("JACK").unwrap(),
            SpeechmaticsVoice::Jack
        );
        assert!(SpeechmaticsVoice::from_str("invalid").is_err());
    }

    #[test]
    fn test_voice_default() {
        assert_eq!(SpeechmaticsVoice::default(), SpeechmaticsVoice::Sarah);
    }

    #[test]
    fn test_voice_all() {
        let voices = SpeechmaticsVoice::all();
        assert_eq!(voices.len(), 4);
        assert!(voices.contains(&SpeechmaticsVoice::Sarah));
        assert!(voices.contains(&SpeechmaticsVoice::Theo));
        assert!(voices.contains(&SpeechmaticsVoice::Megan));
        assert!(voices.contains(&SpeechmaticsVoice::Jack));
    }

    #[test]
    fn test_voice_gender() {
        assert_eq!(SpeechmaticsVoice::Sarah.gender(), "female");
        assert_eq!(SpeechmaticsVoice::Theo.gender(), "male");
        assert_eq!(SpeechmaticsVoice::Megan.gender(), "female");
        assert_eq!(SpeechmaticsVoice::Jack.gender(), "male");
    }

    #[test]
    fn test_voice_accent() {
        assert_eq!(SpeechmaticsVoice::Sarah.accent(), "UK");
        assert_eq!(SpeechmaticsVoice::Theo.accent(), "UK");
        assert_eq!(SpeechmaticsVoice::Megan.accent(), "US");
        assert_eq!(SpeechmaticsVoice::Jack.accent(), "US");
    }

    #[test]
    fn test_output_format_as_str() {
        assert_eq!(SpeechmaticsOutputFormat::Wav16000.as_str(), "wav_16000");
        assert_eq!(SpeechmaticsOutputFormat::Pcm16000.as_str(), "pcm_16000");
    }

    #[test]
    fn test_output_format_from_str() {
        assert_eq!(
            SpeechmaticsOutputFormat::from_str("wav_16000").unwrap(),
            SpeechmaticsOutputFormat::Wav16000
        );
        assert_eq!(
            SpeechmaticsOutputFormat::from_str("wav").unwrap(),
            SpeechmaticsOutputFormat::Wav16000
        );
        assert_eq!(
            SpeechmaticsOutputFormat::from_str("pcm_16000").unwrap(),
            SpeechmaticsOutputFormat::Pcm16000
        );
        assert_eq!(
            SpeechmaticsOutputFormat::from_str("pcm").unwrap(),
            SpeechmaticsOutputFormat::Pcm16000
        );
        assert_eq!(
            SpeechmaticsOutputFormat::from_str("raw").unwrap(),
            SpeechmaticsOutputFormat::Pcm16000
        );
        assert!(SpeechmaticsOutputFormat::from_str("invalid").is_err());
    }

    #[test]
    fn test_output_format_default() {
        assert_eq!(
            SpeechmaticsOutputFormat::default(),
            SpeechmaticsOutputFormat::Wav16000
        );
    }

    #[test]
    fn test_output_format_sample_rate() {
        assert_eq!(SpeechmaticsOutputFormat::Wav16000.sample_rate(), 16000);
        assert_eq!(SpeechmaticsOutputFormat::Pcm16000.sample_rate(), 16000);
    }

    #[test]
    fn test_output_format_has_header() {
        assert!(SpeechmaticsOutputFormat::Wav16000.has_header());
        assert!(!SpeechmaticsOutputFormat::Pcm16000.has_header());
    }

    #[test]
    fn test_config_new() {
        let config = SpeechmaticsTtsConfig::new("test-key");
        assert_eq!(config.api_key, "test-key");
        assert_eq!(config.voice, SpeechmaticsVoice::Sarah);
        assert_eq!(config.output_format, SpeechmaticsOutputFormat::Wav16000);
    }

    #[test]
    fn test_config_builder() {
        let config = SpeechmaticsTtsConfig::new("test-key")
            .with_voice(SpeechmaticsVoice::Jack)
            .with_output_format(SpeechmaticsOutputFormat::Pcm16000);

        assert_eq!(config.voice, SpeechmaticsVoice::Jack);
        assert_eq!(config.output_format, SpeechmaticsOutputFormat::Pcm16000);
    }

    #[test]
    fn test_config_generate_url() {
        let config = SpeechmaticsTtsConfig::new("test-key").with_voice(SpeechmaticsVoice::Theo);

        let url = config.generate_url();
        assert!(url.contains("speechmatics.com"));
        assert!(url.contains("/generate/theo"));
    }

    #[test]
    fn test_config_full_url() {
        let config = SpeechmaticsTtsConfig::new("test-key")
            .with_voice(SpeechmaticsVoice::Megan)
            .with_output_format(SpeechmaticsOutputFormat::Pcm16000);

        let url = config.full_url();
        assert!(url.contains("/generate/megan"));
        assert!(url.contains("output_format=pcm_16000"));
    }

    #[test]
    fn test_config_validate_empty_api_key() {
        let config = SpeechmaticsTtsConfig::default();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validate_valid() {
        let config = SpeechmaticsTtsConfig::new("test-key");
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_from_base() {
        let base_config = TTSConfig {
            api_key: "test-api-key".to_string(),
            voice_id: Some("jack".to_string()),
            audio_format: Some("pcm".to_string()),
            ..Default::default()
        };

        let config = SpeechmaticsTtsConfig::from_base(&base_config).unwrap();
        assert_eq!(config.api_key, "test-api-key");
        assert_eq!(config.voice, SpeechmaticsVoice::Jack);
        assert_eq!(config.output_format, SpeechmaticsOutputFormat::Pcm16000);
    }

    #[test]
    fn test_config_from_base_defaults() {
        let base_config = TTSConfig {
            api_key: "test-api-key".to_string(),
            ..Default::default()
        };

        let config = SpeechmaticsTtsConfig::from_base(&base_config).unwrap();
        assert_eq!(config.voice, SpeechmaticsVoice::Sarah);
        assert_eq!(config.output_format, SpeechmaticsOutputFormat::Wav16000);
    }

    #[test]
    fn test_config_from_base_requires_api_key() {
        let base_config = TTSConfig::default();
        assert!(SpeechmaticsTtsConfig::from_base(&base_config).is_err());
    }

    #[test]
    fn test_validate_text_ok() {
        let text = "Hello, world!";
        assert!(SpeechmaticsTtsConfig::validate_text(text).is_ok());
    }

    #[test]
    fn test_validate_text_too_long() {
        let text = "a".repeat(5001);
        assert!(SpeechmaticsTtsConfig::validate_text(&text).is_err());
    }

    #[test]
    fn test_generate_request_new() {
        let request = SpeechmaticsGenerateRequest::new("Hello, world!");
        assert_eq!(request.text, "Hello, world!");
    }

    #[test]
    fn test_generate_request_serialization() {
        let request = SpeechmaticsGenerateRequest::new("Test message");
        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"text\":\"Test message\""));
    }
}
