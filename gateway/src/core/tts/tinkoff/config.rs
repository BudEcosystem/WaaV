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

    /// Treat synthesis input as SSML. When set, the gRPC `SynthesisInput.ssml` oneof field is
    /// populated instead of `SynthesisInput.text`, unlocking `<speak>`/`<prosody>` markup on the
    /// `tinkoff.cloud.tts.v1.TextToSpeech` Synthesize / StreamingSynthesize RPCs.
    #[serde(default)]
    pub ssml: bool,

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
            ssml: false,
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
            ssml: false,
            connection_timeout_secs: default_connection_timeout(),
            request_timeout_secs: default_request_timeout(),
        })
    }

    /// Build from the standardized TTS config. Tinkoff exposes three direct prosody knobs, so this
    /// maps `speed` -> `speaking_rate` (both are 1.0-is-normal multipliers, clamped to the API's
    /// 0.25-4.0 range), `pitch` -> `pitch` (direct semitone offset, clamped to -20.0..=20.0), and
    /// `volume` -> `volume_gain_db` (direct dB gain, clamped to -96.0..=16.0). `sample_rate`
    /// overrides the output sample rate (clamped to the API's 1000-48000 Hz range). `ssml` flips
    /// the input oneof so synthesis input is sent as `SynthesisInput.ssml` instead of `.text`
    /// (the VoiceKit `TextToSpeech` API accepts SSML on both Synthesize and StreamingSynthesize).
    /// Tinkoff's non-standard `connection_timeout_secs` / `request_timeout_secs` knobs are read
    /// from the `extras` passthrough. Features without a Tinkoff field (stability, similarity_boost,
    /// style, use_speaker_boost, emotion, instructions, language, word_timestamps, streaming, seed)
    /// are skipped.
    pub fn from_standard(
        std: &crate::core::tts::standard::StandardTTSConfig,
    ) -> Result<Self, String> {
        let f = &std.features;
        let mut cfg = Self::from_base(std.base.clone())?;

        if let Some(speed) = f.speed {
            // Both are 1.0-is-normal multipliers; clamp to Tinkoff's 0.25-4.0 range.
            cfg.speaking_rate = speed.clamp(0.25, 4.0);
        }
        if let Some(pitch) = f.pitch {
            // Tinkoff pitch is a direct semitone offset.
            cfg.pitch = pitch.clamp(-20.0, 20.0);
        }
        if let Some(volume) = f.volume {
            // Tinkoff volume is a direct dB gain.
            cfg.volume_gain_db = volume.clamp(-96.0, 16.0);
        }
        if let Some(rate) = f.sample_rate {
            cfg.sample_rate = rate.clamp(1000, 48000);
        }
        if let Some(ssml) = f.ssml {
            // Flip the SynthesisInput oneof from `.text` to `.ssml` (audio-changing).
            cfg.ssml = ssml;
        }

        // Provider-specific passthrough.
        if let Some(secs) = std
            .extras
            .0
            .get("connection_timeout_secs")
            .and_then(|v| v.as_u64())
        {
            cfg.connection_timeout_secs = secs;
        }
        if let Some(secs) = std
            .extras
            .0
            .get("request_timeout_secs")
            .and_then(|v| v.as_u64())
        {
            cfg.request_timeout_secs = secs;
        }

        Ok(cfg)
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

    // Maps speed -> speaking_rate (1.0-is-normal multiplier), pitch -> pitch (direct semitones),
    // volume -> volume_gain_db (direct dB), sample_rate -> sample_rate, and demonstrates the extras
    // passthrough (connection_timeout_secs / request_timeout_secs).
    #[test]
    fn from_standard_maps_prosody_and_extras() {
        use crate::core::tts::standard::{StandardTTSConfig, TtsFeatures};
        let mut extras = serde_json::Map::new();
        extras.insert("connection_timeout_secs".into(), serde_json::json!(20));
        extras.insert("request_timeout_secs".into(), serde_json::json!(45));
        let std = StandardTTSConfig {
            base: TTSConfig {
                provider: "tinkoff".into(),
                api_key: "test".into(),
                // The shared TTSConfig default voice_id ("aura-asteria-en") is a Deepgram voice
                // that Tinkoff rejects; pin a valid Tinkoff voice so from_base/from_standard resolve.
                voice_id: Some("alyona".into()),
                ..Default::default()
            },
            features: TtsFeatures {
                speed: Some(2.0),
                pitch: Some(5.0),
                volume: Some(-6.0),
                sample_rate: Some(48000),
                ssml: Some(true), // now wired: flips the SynthesisInput oneof to `.ssml`
                ..Default::default()
            },
            extras: crate::core::stt::standard::ProviderExtras(extras),
        };
        let cfg = TinkoffTtsConfig::from_standard(&std).unwrap();
        assert_eq!(cfg.speaking_rate, 2.0); // 1.0-is-normal multiplier, passed through
        assert_eq!(cfg.pitch, 5.0); // direct semitone offset
        assert_eq!(cfg.volume_gain_db, -6.0); // direct dB gain
        assert_eq!(cfg.sample_rate, 48000);
        assert!(cfg.ssml); // features.ssml -> config.ssml
        assert_eq!(cfg.connection_timeout_secs, 20); // from extras passthrough
        assert_eq!(cfg.request_timeout_secs, 45);
    }

    // `features.ssml` toggles the typed config flag that selects the SynthesisInput.ssml oneof;
    // when unset it stays false so the plain-text path is preserved.
    #[test]
    fn from_standard_maps_ssml_flag() {
        use crate::core::tts::standard::{StandardTTSConfig, TtsFeatures};
        let base = TTSConfig {
            provider: "tinkoff".into(),
            api_key: "test".into(),
            voice_id: Some("alyona".into()),
            ..Default::default()
        };
        let with_ssml = StandardTTSConfig {
            base: base.clone(),
            features: TtsFeatures {
                ssml: Some(true),
                ..Default::default()
            },
            extras: Default::default(),
        };
        assert!(TinkoffTtsConfig::from_standard(&with_ssml).unwrap().ssml);

        let without = StandardTTSConfig {
            base,
            features: TtsFeatures::default(),
            extras: Default::default(),
        };
        assert!(!TinkoffTtsConfig::from_standard(&without).unwrap().ssml);
    }

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
