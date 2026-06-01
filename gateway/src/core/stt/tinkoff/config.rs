//! Tinkoff VoiceKit STT Configuration
//!
//! Configuration types for Tinkoff's Speech-to-Text gRPC API with support for
//! Russian language, multiple audio encodings, and configurable VAD.

use crate::core::stt::base::STTConfig;
use serde::{Deserialize, Serialize};

/// Tinkoff STT provider-specific configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TinkoffSttConfig {
    /// Base STT configuration (api_key, language, sample_rate, etc.)
    #[serde(flatten)]
    pub base: STTConfig,

    /// Tinkoff API key (from console)
    #[serde(default, skip_serializing)]
    pub api_key: String,

    /// Tinkoff secret key (from console)
    #[serde(default, skip_serializing)]
    pub secret_key: String,

    /// Audio encoding format
    #[serde(default)]
    pub encoding: TinkoffAudioEncoding,

    /// Maximum alternatives to return
    #[serde(default = "default_max_alternatives")]
    pub max_alternatives: u32,

    /// Enable automatic punctuation
    #[serde(default = "default_punctuation")]
    pub enable_punctuation: bool,

    /// Enable interim (partial) results
    #[serde(default = "default_interim_results")]
    pub interim_results: bool,

    /// Single utterance mode (stop after first pause)
    #[serde(default)]
    pub single_utterance: bool,

    /// VAD (Voice Activity Detection) configuration
    #[serde(default)]
    pub vad_config: Option<VadConfig>,

    /// Connection timeout in seconds
    #[serde(default = "default_connection_timeout")]
    pub connection_timeout_secs: u64,

    /// Request timeout in seconds
    #[serde(default = "default_request_timeout")]
    pub request_timeout_secs: u64,
}

fn default_max_alternatives() -> u32 {
    1
}

fn default_punctuation() -> bool {
    true
}

fn default_interim_results() -> bool {
    true
}

fn default_connection_timeout() -> u64 {
    10
}

fn default_request_timeout() -> u64 {
    60
}

impl Default for TinkoffSttConfig {
    fn default() -> Self {
        Self {
            base: STTConfig {
                provider: "tinkoff".to_string(),
                api_key: String::new(),
                language: "ru-RU".to_string(),
                sample_rate: 16000,
                channels: 1,
                punctuation: true,
                encoding: "linear16".to_string(),
                model: "default".to_string(),
            },
            api_key: String::new(),
            secret_key: String::new(),
            encoding: TinkoffAudioEncoding::default(),
            max_alternatives: default_max_alternatives(),
            enable_punctuation: default_punctuation(),
            interim_results: default_interim_results(),
            single_utterance: false,
            vad_config: None,
            connection_timeout_secs: default_connection_timeout(),
            request_timeout_secs: default_request_timeout(),
        }
    }
}

impl TinkoffSttConfig {
    /// Build from the standardized config (W1 keystone). Tinkoff's VoiceKit gRPC surface is
    /// narrow: of the canonical features it can only express interim/partial results, so this
    /// maps `interim_results` and leaves the rest at provider defaults. Diarization, redaction,
    /// keyterms, word timestamps, smart formatting, profanity filtering, entity/language
    /// detection and explicit VAD-event toggles have no corresponding field on this provider and
    /// are capability gaps (intentionally skipped).
    pub fn from_standard(
        std: &crate::core::stt::standard::StandardSTTConfig,
    ) -> Result<Self, String> {
        let f = &std.features;
        let mut cfg = Self::from_base(std.base.clone())?;
        if let Some(i) = f.interim_results {
            cfg.interim_results = i;
        }
        Ok(cfg)
    }

    /// Create TinkoffSttConfig from base STTConfig
    ///
    /// Extracts Tinkoff-specific credentials from environment variables if not
    /// provided in the base config.
    pub fn from_base(base: STTConfig) -> Result<Self, String> {
        // Try to get credentials from environment if not in config
        let api_key = if base.api_key.is_empty() {
            std::env::var("TINKOFF_API_KEY").unwrap_or_default()
        } else {
            base.api_key.clone()
        };

        let secret_key = std::env::var("TINKOFF_SECRET_KEY").unwrap_or_default();

        // Parse audio encoding
        let encoding = TinkoffAudioEncoding::from_str(&base.encoding)?;

        // Normalize language code
        let language = if base.language.is_empty() || base.language.to_lowercase() == "ru" {
            "ru-RU".to_string()
        } else {
            base.language.clone()
        };

        Ok(Self {
            base: STTConfig { language, ..base },
            api_key,
            secret_key,
            encoding,
            max_alternatives: default_max_alternatives(),
            enable_punctuation: default_punctuation(),
            interim_results: default_interim_results(),
            single_utterance: false,
            vad_config: None,
            connection_timeout_secs: default_connection_timeout(),
            request_timeout_secs: default_request_timeout(),
        })
    }

    /// Validate that all required credentials are present
    pub fn validate(&self) -> Result<(), String> {
        if self.api_key.is_empty() {
            return Err(
                "Tinkoff API key is required. Set TINKOFF_API_KEY environment variable or provide in config.".to_string(),
            );
        }

        if self.secret_key.is_empty() {
            return Err(
                "Tinkoff secret key is required. Set TINKOFF_SECRET_KEY environment variable."
                    .to_string(),
            );
        }

        // Validate sample rate
        if !SUPPORTED_SAMPLE_RATES.contains(&self.base.sample_rate) {
            return Err(format!(
                "Unsupported sample rate: {}. Supported: {:?}",
                self.base.sample_rate, SUPPORTED_SAMPLE_RATES
            ));
        }

        Ok(())
    }

    /// Get the gRPC endpoint URL
    pub fn endpoint(&self) -> &'static str {
        "https://api.tinkoff.ai:443"
    }
}

/// Supported sample rates for Tinkoff STT
pub const SUPPORTED_SAMPLE_RATES: &[u32] = &[8000, 16000, 22050, 24000, 44100, 48000];

/// Audio encoding formats for Tinkoff STT
///
/// Maps to protobuf enum AudioEncoding
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TinkoffAudioEncoding {
    /// Uncompressed 16-bit signed little-endian PCM
    #[default]
    #[serde(rename = "linear16")]
    Linear16,
    /// Opus encoded audio without container
    #[serde(rename = "raw_opus")]
    RawOpus,
    /// 8-bit G.711 mu-law
    #[serde(rename = "mulaw")]
    Mulaw,
    /// 8-bit G.711 A-law
    #[serde(rename = "alaw")]
    Alaw,
    /// FLAC lossless audio
    #[serde(rename = "flac")]
    Flac,
    /// MP3 audio
    #[serde(rename = "mp3")]
    MpegAudio,
}

impl TinkoffAudioEncoding {
    /// Get the protobuf enum value
    pub fn as_i32(&self) -> i32 {
        match self {
            Self::Linear16 => 1,
            Self::RawOpus => 2,
            Self::Mulaw => 5,
            Self::Alaw => 6,
            Self::Flac => 16,
            Self::MpegAudio => 8,
        }
    }

    /// Get the string representation for API
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Linear16 => "LINEAR16",
            Self::RawOpus => "RAW_OPUS",
            Self::Mulaw => "MULAW",
            Self::Alaw => "ALAW",
            Self::Flac => "FLAC",
            Self::MpegAudio => "MPEG_AUDIO",
        }
    }

    /// Parse from string
    pub fn from_str(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "linear16" | "pcm16" | "pcm" => Ok(Self::Linear16),
            "raw_opus" | "opus" => Ok(Self::RawOpus),
            "mulaw" | "ulaw" => Ok(Self::Mulaw),
            "alaw" => Ok(Self::Alaw),
            "flac" => Ok(Self::Flac),
            "mp3" | "mpeg" | "mpeg_audio" => Ok(Self::MpegAudio),
            _ => Err(format!(
                "Unsupported Tinkoff encoding: {}. Supported: linear16, raw_opus, mulaw, alaw, flac, mp3",
                s
            )),
        }
    }
}

/// Voice Activity Detection configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VadConfig {
    /// Minimum speech duration in seconds
    #[serde(default)]
    pub min_speech_duration: f32,
    /// Maximum speech duration in seconds
    #[serde(default)]
    pub max_speech_duration: f32,
    /// Silence duration threshold in seconds
    #[serde(default)]
    pub silence_duration_threshold: f32,
    /// Silence probability threshold (0.0 to 1.0)
    #[serde(default)]
    pub silence_prob_threshold: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    // W1 keystone: Tinkoff has a narrow surface, so the standardized config can only unlock
    // interim results here; diarization (set below) is a documented capability gap and must stay
    // at the provider default rather than silently mapping to an unrelated field.
    #[test]
    fn from_standard_maps_features() {
        use crate::core::stt::standard::{SttFeatures, StandardSTTConfig};
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "tinkoff".into(),
                api_key: "test-api-key".into(),
                ..Default::default()
            },
            features: SttFeatures {
                interim_results: Some(false),
                diarization: Some(true), // capability gap: no field to map to
                ..Default::default()
            },
            ..StandardSTTConfig::from_base(STTConfig::default())
        };
        let cfg = TinkoffSttConfig::from_standard(&std).unwrap();
        assert!(!cfg.interim_results); // mapped from the standardized feature
        assert_eq!(cfg.api_key, "test-api-key"); // base carried through from_base
    }

    #[test]
    fn test_tinkoff_encoding_from_str() {
        assert_eq!(
            TinkoffAudioEncoding::from_str("linear16").unwrap(),
            TinkoffAudioEncoding::Linear16
        );
        assert_eq!(
            TinkoffAudioEncoding::from_str("pcm16").unwrap(),
            TinkoffAudioEncoding::Linear16
        );
        assert_eq!(
            TinkoffAudioEncoding::from_str("opus").unwrap(),
            TinkoffAudioEncoding::RawOpus
        );
        assert_eq!(
            TinkoffAudioEncoding::from_str("mulaw").unwrap(),
            TinkoffAudioEncoding::Mulaw
        );
        assert!(TinkoffAudioEncoding::from_str("invalid").is_err());
    }

    #[test]
    fn test_tinkoff_encoding_as_i32() {
        assert_eq!(TinkoffAudioEncoding::Linear16.as_i32(), 1);
        assert_eq!(TinkoffAudioEncoding::RawOpus.as_i32(), 2);
        assert_eq!(TinkoffAudioEncoding::Mulaw.as_i32(), 5);
        assert_eq!(TinkoffAudioEncoding::Alaw.as_i32(), 6);
        assert_eq!(TinkoffAudioEncoding::Flac.as_i32(), 16);
        assert_eq!(TinkoffAudioEncoding::MpegAudio.as_i32(), 8);
    }

    #[test]
    fn test_tinkoff_config_from_base() {
        let base = STTConfig {
            provider: "tinkoff".to_string(),
            api_key: "test-api-key".to_string(),
            language: "ru-RU".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
            model: "default".to_string(),
        };

        let config = TinkoffSttConfig::from_base(base).unwrap();
        assert_eq!(config.api_key, "test-api-key");
        assert_eq!(config.base.language, "ru-RU");
        assert_eq!(config.encoding, TinkoffAudioEncoding::Linear16);
    }

    #[test]
    fn test_tinkoff_config_validation_missing_api_key() {
        let config = TinkoffSttConfig::default();
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("API key"));
    }

    #[test]
    fn test_tinkoff_config_validation_missing_secret_key() {
        let mut config = TinkoffSttConfig::default();
        config.api_key = "test-key".to_string();
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("secret key"));
    }

    #[test]
    fn test_tinkoff_config_validation_invalid_sample_rate() {
        let mut config = TinkoffSttConfig::default();
        config.api_key = "test-key".to_string();
        config.secret_key = "test-secret".to_string();
        config.base.sample_rate = 12345;
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("sample rate"));
    }

    #[test]
    fn test_vad_config_default() {
        let vad = VadConfig::default();
        assert_eq!(vad.min_speech_duration, 0.0);
        assert_eq!(vad.max_speech_duration, 0.0);
        assert_eq!(vad.silence_duration_threshold, 0.0);
        assert_eq!(vad.silence_prob_threshold, 0.0);
    }
}
