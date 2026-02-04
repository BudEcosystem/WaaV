//! Tinkoff VoiceKit TTS Configuration
//!
//! Configuration types for Tinkoff's Text-to-Speech API with support for
//! Russian language synthesis with multiple voices and SSML support.

use crate::core::tts::base::TTSConfig;
use serde::{Deserialize, Serialize};

/// Tinkoff TTS provider-specific configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TinkoffTtsConfig {
    /// Base TTS configuration
    #[serde(flatten)]
    pub base: TTSConfig,

    /// Tinkoff API key
    #[serde(default, skip_serializing)]
    pub api_key: String,

    /// Tinkoff secret key
    #[serde(default, skip_serializing)]
    pub secret_key: String,

    /// Voice selection
    #[serde(default)]
    pub voice: TinkoffVoice,

    /// Audio encoding format
    #[serde(default)]
    pub encoding: TinkoffAudioEncoding,

    /// Sample rate in Hz (1000-48000)
    #[serde(default = "default_sample_rate")]
    pub sample_rate: u32,

    /// Speaking rate multiplier (0.25-4.0, default 1.0)
    #[serde(default = "default_speaking_rate")]
    pub speaking_rate: f32,

    /// Pitch adjustment (-20.0 to 20.0 semitones, default 0.0)
    #[serde(default)]
    pub pitch: f32,

    /// Volume gain in dB (-96.0 to 16.0, default 0.0)
    #[serde(default)]
    pub volume_gain_db: f32,

    /// Connection timeout in seconds
    #[serde(default = "default_connection_timeout")]
    pub connection_timeout_secs: u64,

    /// Request timeout in seconds
    #[serde(default = "default_request_timeout")]
    pub request_timeout_secs: u64,
}

fn default_sample_rate() -> u32 {
    24000
}

fn default_speaking_rate() -> f32 {
    1.0
}

fn default_connection_timeout() -> u64 {
    10
}

fn default_request_timeout() -> u64 {
    30
}

impl Default for TinkoffTtsConfig {
    fn default() -> Self {
        Self {
            base: TTSConfig::default(),
            api_key: String::new(),
            secret_key: String::new(),
            voice: TinkoffVoice::default(),
            encoding: TinkoffAudioEncoding::default(),
            sample_rate: default_sample_rate(),
            speaking_rate: default_speaking_rate(),
            pitch: 0.0,
            volume_gain_db: 0.0,
            connection_timeout_secs: default_connection_timeout(),
            request_timeout_secs: default_request_timeout(),
        }
    }
}

impl TinkoffTtsConfig {
    /// Create TinkoffTtsConfig from base TTSConfig
    pub fn from_base(base: TTSConfig) -> Result<Self, String> {
        // Get credentials from environment
        let api_key = std::env::var("TINKOFF_API_KEY")
            .or_else(|_| std::env::var("TINKOFF_VOICEKIT_API_KEY"))
            .unwrap_or_else(|_| base.api_key.clone());
        let secret_key = std::env::var("TINKOFF_SECRET_KEY")
            .or_else(|_| std::env::var("TINKOFF_VOICEKIT_SECRET_KEY"))
            .unwrap_or_default();

        // Parse voice
        let voice = base
            .voice_id
            .as_ref()
            .map(|v| TinkoffVoice::from_str(v))
            .transpose()?
            .unwrap_or_default();

        // Parse encoding
        let encoding = base
            .audio_format
            .as_ref()
            .map(|f| TinkoffAudioEncoding::from_str(f))
            .transpose()?
            .unwrap_or_default();

        // Parse sample rate
        let sample_rate = base.sample_rate.unwrap_or(default_sample_rate());
        if !(1000..=48000).contains(&sample_rate) {
            return Err(format!(
                "Invalid sample rate: {}. Must be between 1000 and 48000 Hz",
                sample_rate
            ));
        }

        Ok(Self {
            base,
            api_key,
            secret_key,
            voice,
            encoding,
            sample_rate,
            speaking_rate: default_speaking_rate(),
            pitch: 0.0,
            volume_gain_db: 0.0,
            connection_timeout_secs: default_connection_timeout(),
            request_timeout_secs: default_request_timeout(),
        })
    }

    /// Validate the configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.api_key.is_empty() {
            return Err(
                "Tinkoff API key is required. Set TINKOFF_API_KEY environment variable."
                    .to_string(),
            );
        }

        if self.secret_key.is_empty() {
            return Err(
                "Tinkoff secret key is required. Set TINKOFF_SECRET_KEY environment variable."
                    .to_string(),
            );
        }

        if !(1000..=48000).contains(&self.sample_rate) {
            return Err(format!(
                "Invalid sample rate: {}. Must be between 1000 and 48000 Hz",
                self.sample_rate
            ));
        }

        if !(0.25..=4.0).contains(&self.speaking_rate) {
            return Err(format!(
                "Invalid speaking rate: {}. Must be between 0.25 and 4.0",
                self.speaking_rate
            ));
        }

        if !(-20.0..=20.0).contains(&self.pitch) {
            return Err(format!(
                "Invalid pitch: {}. Must be between -20.0 and 20.0 semitones",
                self.pitch
            ));
        }

        if !(-96.0..=16.0).contains(&self.volume_gain_db) {
            return Err(format!(
                "Invalid volume gain: {}. Must be between -96.0 and 16.0 dB",
                self.volume_gain_db
            ));
        }

        Ok(())
    }

    /// Get the gRPC endpoint URL
    pub fn endpoint(&self) -> &'static str {
        super::TINKOFF_GRPC_ENDPOINT
    }
}

/// Available voices for Tinkoff TTS
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TinkoffVoice {
    /// Alyona - Russian female voice (default)
    #[default]
    Alyona,
    /// Dorofeev - Russian male voice
    Dorofeev,
}

impl TinkoffVoice {
    /// Get the voice ID string for API requests
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Alyona => "alyona",
            Self::Dorofeev => "dorofeev",
        }
    }

    /// Parse voice from string
    pub fn from_str(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "alyona" => Ok(Self::Alyona),
            "dorofeev" => Ok(Self::Dorofeev),
            _ => Err(format!(
                "Unknown Tinkoff voice: {}. Available: alyona, dorofeev",
                s
            )),
        }
    }

    /// Get display name
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Alyona => "Alyona (Female)",
            Self::Dorofeev => "Dorofeev (Male)",
        }
    }

    /// Get gender
    pub fn gender(&self) -> &'static str {
        match self {
            Self::Alyona => "female",
            Self::Dorofeev => "male",
        }
    }

    /// Get all available voices
    pub fn all() -> &'static [TinkoffVoice] {
        &[Self::Alyona, Self::Dorofeev]
    }
}

/// Audio encoding formats for Tinkoff TTS
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TinkoffAudioEncoding {
    /// LINEAR16 - PCM signed 16-bit little-endian
    #[default]
    Linear16,
    /// RAW_OPUS - Opus frames in protobuf (streaming only)
    RawOpus,
    /// ALAW - 8-bit A-law (PCMA)
    Alaw,
}

impl TinkoffAudioEncoding {
    /// Get the protobuf enum value
    pub fn as_i32(&self) -> i32 {
        match self {
            Self::Linear16 => 1,
            Self::RawOpus => 2,
            Self::Alaw => 6,
        }
    }

    /// Parse encoding from string
    pub fn from_str(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "linear16" | "pcm" | "pcm16" | "wav" => Ok(Self::Linear16),
            "raw_opus" | "opus" => Ok(Self::RawOpus),
            "alaw" | "pcma" | "g711a" => Ok(Self::Alaw),
            _ => Err(format!(
                "Unknown Tinkoff audio encoding: {}. Available: linear16, opus, alaw",
                s
            )),
        }
    }

    /// Get the encoding string for API requests
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Linear16 => "LINEAR16",
            Self::RawOpus => "RAW_OPUS",
            Self::Alaw => "ALAW",
        }
    }

    /// Get MIME type
    pub fn mime_type(&self) -> &'static str {
        match self {
            Self::Linear16 => "audio/L16",
            Self::RawOpus => "audio/opus",
            Self::Alaw => "audio/alaw",
        }
    }

    /// Check if streaming is supported
    pub fn supports_streaming(&self) -> bool {
        // All formats support streaming for TTS
        true
    }

    /// Check if non-streaming is supported
    pub fn supports_non_streaming(&self) -> bool {
        // RAW_OPUS is only for streaming
        !matches!(self, Self::RawOpus)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tinkoff_voice_from_str() {
        assert_eq!(
            TinkoffVoice::from_str("alyona").unwrap(),
            TinkoffVoice::Alyona
        );
        assert_eq!(
            TinkoffVoice::from_str("ALYONA").unwrap(),
            TinkoffVoice::Alyona
        );
        assert_eq!(
            TinkoffVoice::from_str("dorofeev").unwrap(),
            TinkoffVoice::Dorofeev
        );
        assert!(TinkoffVoice::from_str("invalid").is_err());
    }

    #[test]
    fn test_tinkoff_voice_as_str() {
        assert_eq!(TinkoffVoice::Alyona.as_str(), "alyona");
        assert_eq!(TinkoffVoice::Dorofeev.as_str(), "dorofeev");
    }

    #[test]
    fn test_tinkoff_encoding_from_str() {
        assert_eq!(
            TinkoffAudioEncoding::from_str("linear16").unwrap(),
            TinkoffAudioEncoding::Linear16
        );
        assert_eq!(
            TinkoffAudioEncoding::from_str("pcm").unwrap(),
            TinkoffAudioEncoding::Linear16
        );
        assert_eq!(
            TinkoffAudioEncoding::from_str("opus").unwrap(),
            TinkoffAudioEncoding::RawOpus
        );
        assert_eq!(
            TinkoffAudioEncoding::from_str("alaw").unwrap(),
            TinkoffAudioEncoding::Alaw
        );
        assert!(TinkoffAudioEncoding::from_str("invalid").is_err());
    }

    #[test]
    fn test_tinkoff_encoding_as_i32() {
        assert_eq!(TinkoffAudioEncoding::Linear16.as_i32(), 1);
        assert_eq!(TinkoffAudioEncoding::RawOpus.as_i32(), 2);
        assert_eq!(TinkoffAudioEncoding::Alaw.as_i32(), 6);
    }

    #[test]
    fn test_tinkoff_config_from_base() {
        let base = TTSConfig {
            api_key: "test".to_string(),
            voice_id: Some("alyona".to_string()),
            sample_rate: Some(24000),
            audio_format: Some("linear16".to_string()),
            ..Default::default()
        };

        let config = TinkoffTtsConfig::from_base(base).unwrap();
        assert_eq!(config.voice, TinkoffVoice::Alyona);
        assert_eq!(config.encoding, TinkoffAudioEncoding::Linear16);
        assert_eq!(config.sample_rate, 24000);
    }

    #[test]
    fn test_tinkoff_config_validation_missing_api_key() {
        let config = TinkoffTtsConfig::default();
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("API key"));
    }

    #[test]
    fn test_tinkoff_config_validation_missing_secret_key() {
        let mut config = TinkoffTtsConfig::default();
        config.api_key = "test".to_string();
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("secret key"));
    }

    #[test]
    fn test_tinkoff_config_validation_invalid_sample_rate() {
        let mut config = TinkoffTtsConfig::default();
        config.api_key = "test".to_string();
        config.secret_key = "secret".to_string();
        config.sample_rate = 500; // Below minimum
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("sample rate"));
    }

    #[test]
    fn test_tinkoff_config_validation_invalid_speaking_rate() {
        let mut config = TinkoffTtsConfig::default();
        config.api_key = "test".to_string();
        config.secret_key = "secret".to_string();
        config.speaking_rate = 5.0; // Above maximum
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("speaking rate"));
    }

    #[test]
    fn test_tinkoff_voice_all() {
        let voices = TinkoffVoice::all();
        assert_eq!(voices.len(), 2);
        assert!(voices.contains(&TinkoffVoice::Alyona));
        assert!(voices.contains(&TinkoffVoice::Dorofeev));
    }
}
