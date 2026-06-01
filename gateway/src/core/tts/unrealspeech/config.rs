//! Unreal Speech TTS Configuration
//!
//! This module defines configuration structures and types for the Unreal Speech TTS API.

use serde::{Deserialize, Serialize};
use std::fmt;

use crate::core::tts::base::{TTSConfig, TTSError, TTSResult};

// =============================================================================
// Voice Types
// =============================================================================

/// Unreal Speech voice identifiers
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum UnrealSpeechVoice {
    // Standard voices
    /// Young female voice (default)
    #[default]
    Scarlett,
    /// Young female voice
    Liv,
    /// Mature female voice
    Amy,
    /// Young male voice
    Dan,
    /// Mature male voice
    Will,

    // Kokoro V8 voices - American
    /// American female
    Af,
    /// American female - Bella
    AfBella,
    /// American female - Sarah
    AfSarah,
    /// American female - Nicole
    AfNicole,
    /// American female - Sky
    AfSky,
    /// American male - Adam
    AmAdam,
    /// American male - Michael
    AmMichael,

    // Kokoro V8 voices - British
    /// British female - Emma
    BfEmma,
    /// British female - Isabella
    BfIsabella,
    /// British male - George
    BmGeorge,
    /// British male - Lewis
    BmLewis,

    /// Custom voice ID
    Custom(String),
}

impl UnrealSpeechVoice {
    /// Get the API voice identifier
    pub fn as_str(&self) -> &str {
        match self {
            Self::Scarlett => "Scarlett",
            Self::Liv => "Liv",
            Self::Amy => "Amy",
            Self::Dan => "Dan",
            Self::Will => "Will",
            Self::Af => "af",
            Self::AfBella => "af_bella",
            Self::AfSarah => "af_sarah",
            Self::AfNicole => "af_nicole",
            Self::AfSky => "af_sky",
            Self::AmAdam => "am_adam",
            Self::AmMichael => "am_michael",
            Self::BfEmma => "bf_emma",
            Self::BfIsabella => "bf_isabella",
            Self::BmGeorge => "bm_george",
            Self::BmLewis => "bm_lewis",
            Self::Custom(id) => id.as_str(),
        }
    }

    /// Check if voice is a standard voice
    pub fn is_standard(&self) -> bool {
        matches!(
            self,
            Self::Scarlett | Self::Liv | Self::Amy | Self::Dan | Self::Will
        )
    }

    /// Check if voice is a Kokoro V8 voice
    pub fn is_kokoro(&self) -> bool {
        matches!(
            self,
            Self::Af
                | Self::AfBella
                | Self::AfSarah
                | Self::AfNicole
                | Self::AfSky
                | Self::AmAdam
                | Self::AmMichael
                | Self::BfEmma
                | Self::BfIsabella
                | Self::BmGeorge
                | Self::BmLewis
        )
    }
}

impl fmt::Display for UnrealSpeechVoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl std::str::FromStr for UnrealSpeechVoice {
    type Err = TTSError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "scarlett" => Ok(Self::Scarlett),
            "liv" => Ok(Self::Liv),
            "amy" => Ok(Self::Amy),
            "dan" => Ok(Self::Dan),
            "will" => Ok(Self::Will),
            "af" => Ok(Self::Af),
            "af_bella" | "afbella" | "bella" => Ok(Self::AfBella),
            "af_sarah" | "afsarah" | "sarah" => Ok(Self::AfSarah),
            "af_nicole" | "afnicole" | "nicole" => Ok(Self::AfNicole),
            "af_sky" | "afsky" | "sky" => Ok(Self::AfSky),
            "am_adam" | "amadam" | "adam" => Ok(Self::AmAdam),
            "am_michael" | "ammichael" | "michael" => Ok(Self::AmMichael),
            "bf_emma" | "bfemma" | "emma" => Ok(Self::BfEmma),
            "bf_isabella" | "bfisabella" | "isabella" => Ok(Self::BfIsabella),
            "bm_george" | "bmgeorge" | "george" => Ok(Self::BmGeorge),
            "bm_lewis" | "bmlewis" | "lewis" => Ok(Self::BmLewis),
            // Allow any other voice ID as custom
            other => Ok(Self::Custom(other.to_string())),
        }
    }
}

impl Serialize for UnrealSpeechVoice {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for UnrealSpeechVoice {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

// =============================================================================
// Codec Types
// =============================================================================

/// Unreal Speech audio codec options
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UnrealSpeechCodec {
    /// MP3 codec (libmp3lame) - default
    #[default]
    Mp3,
    /// PCM mu-law codec
    PcmMulaw,
}

impl UnrealSpeechCodec {
    /// Get the API codec identifier
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Mp3 => "libmp3lame",
            Self::PcmMulaw => "pcm_mulaw",
        }
    }

    /// Get the MIME type for this codec
    pub fn mime_type(&self) -> &'static str {
        match self {
            Self::Mp3 => "audio/mpeg",
            Self::PcmMulaw => "audio/basic",
        }
    }
}

impl fmt::Display for UnrealSpeechCodec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl std::str::FromStr for UnrealSpeechCodec {
    type Err = TTSError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "libmp3lame" | "mp3" | "mpeg" => Ok(Self::Mp3),
            "pcm_mulaw" | "pcmmulaw" | "mulaw" | "ulaw" => Ok(Self::PcmMulaw),
            _ => Err(TTSError::InvalidConfiguration(format!(
                "Unknown Unreal Speech codec: '{}'. Valid codecs: libmp3lame (mp3), pcm_mulaw",
                s
            ))),
        }
    }
}

impl Serialize for UnrealSpeechCodec {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for UnrealSpeechCodec {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

// =============================================================================
// Bitrate Type
// =============================================================================

/// Unreal Speech bitrate options
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UnrealSpeechBitrate {
    /// 16 kbps - lowest quality, smallest file
    Bitrate16k,
    /// 32 kbps
    Bitrate32k,
    /// 64 kbps
    Bitrate64k,
    /// 128 kbps
    Bitrate128k,
    /// 192 kbps - default, good balance
    #[default]
    Bitrate192k,
    /// 256 kbps
    Bitrate256k,
    /// 320 kbps - highest quality
    Bitrate320k,
}

impl UnrealSpeechBitrate {
    /// Get the API bitrate string
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Bitrate16k => "16k",
            Self::Bitrate32k => "32k",
            Self::Bitrate64k => "64k",
            Self::Bitrate128k => "128k",
            Self::Bitrate192k => "192k",
            Self::Bitrate256k => "256k",
            Self::Bitrate320k => "320k",
        }
    }

    /// Get the numeric bitrate value in kbps
    pub fn kbps(&self) -> u32 {
        match self {
            Self::Bitrate16k => 16,
            Self::Bitrate32k => 32,
            Self::Bitrate64k => 64,
            Self::Bitrate128k => 128,
            Self::Bitrate192k => 192,
            Self::Bitrate256k => 256,
            Self::Bitrate320k => 320,
        }
    }
}

impl fmt::Display for UnrealSpeechBitrate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl std::str::FromStr for UnrealSpeechBitrate {
    type Err = TTSError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "16k" | "16" => Ok(Self::Bitrate16k),
            "32k" | "32" => Ok(Self::Bitrate32k),
            "64k" | "64" => Ok(Self::Bitrate64k),
            "128k" | "128" => Ok(Self::Bitrate128k),
            "192k" | "192" => Ok(Self::Bitrate192k),
            "256k" | "256" => Ok(Self::Bitrate256k),
            "320k" | "320" => Ok(Self::Bitrate320k),
            _ => Err(TTSError::InvalidConfiguration(format!(
                "Unknown Unreal Speech bitrate: '{}'. Valid bitrates: 16k, 32k, 64k, 128k, 192k, 256k, 320k",
                s
            ))),
        }
    }
}

impl Serialize for UnrealSpeechBitrate {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for UnrealSpeechBitrate {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

// =============================================================================
// Request Types
// =============================================================================

/// Request body for Unreal Speech /stream endpoint
#[derive(Debug, Clone, Serialize)]
pub struct UnrealSpeechStreamRequest {
    /// Text to synthesize
    #[serde(rename = "Text")]
    pub text: String,

    /// Voice identifier
    #[serde(rename = "VoiceId")]
    pub voice_id: String,

    /// Audio bitrate
    #[serde(rename = "Bitrate")]
    pub bitrate: String,

    /// Speech speed (-1.0 to 1.0)
    #[serde(rename = "Speed")]
    pub speed: f32,

    /// Voice pitch (0.5 to 1.5)
    #[serde(rename = "Pitch")]
    pub pitch: f32,

    /// Audio codec
    #[serde(rename = "Codec")]
    pub codec: String,
}

impl UnrealSpeechStreamRequest {
    /// Create a new stream request from config
    pub fn from_config(config: &UnrealSpeechTtsConfig, text: &str) -> Self {
        Self {
            text: text.to_string(),
            voice_id: config.voice.as_str().to_string(),
            bitrate: config.bitrate.as_str().to_string(),
            speed: config.speed,
            pitch: config.pitch,
            codec: config.codec.as_str().to_string(),
        }
    }
}

// =============================================================================
// Configuration
// =============================================================================

/// Unreal Speech TTS provider configuration
#[derive(Debug, Clone)]
pub struct UnrealSpeechTtsConfig {
    /// API key for authentication
    pub api_key: String,

    /// Voice to use
    pub voice: UnrealSpeechVoice,

    /// Audio bitrate
    pub bitrate: UnrealSpeechBitrate,

    /// Speech speed (-1.0 to 1.0, 0 = normal)
    pub speed: f32,

    /// Voice pitch (0.5 to 1.5, 1.0 = normal)
    pub pitch: f32,

    /// Audio codec
    pub codec: UnrealSpeechCodec,
}

impl Default for UnrealSpeechTtsConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            voice: UnrealSpeechVoice::default(),
            bitrate: UnrealSpeechBitrate::default(),
            speed: super::DEFAULT_SPEED,
            pitch: super::DEFAULT_PITCH,
            codec: UnrealSpeechCodec::default(),
        }
    }
}

impl UnrealSpeechTtsConfig {
    /// Create a new config builder
    pub fn builder() -> UnrealSpeechTtsConfigBuilder {
        UnrealSpeechTtsConfigBuilder::default()
    }

    /// Create configuration from base TTSConfig
    pub fn from_base(config: &TTSConfig) -> TTSResult<Self> {
        // Get API key from config or environment
        let api_key = if !config.api_key.is_empty() {
            config.api_key.clone()
        } else {
            std::env::var("UNREALSPEECH_API_KEY").unwrap_or_default()
        };

        if api_key.is_empty() {
            return Err(TTSError::InvalidConfiguration(
                "Unreal Speech API key is required. Set api_key in config or UNREALSPEECH_API_KEY environment variable.".to_string()
            ));
        }

        // Get voice from config
        let voice = config
            .voice_id
            .as_ref()
            .map(|v| v.parse().unwrap_or_default())
            .unwrap_or_default();

        // Parse codec from audio_format - fall back to default for unsupported formats
        let codec = if let Some(ref fmt) = config.audio_format {
            if !fmt.is_empty() {
                fmt.parse().unwrap_or_default()
            } else {
                UnrealSpeechCodec::default()
            }
        } else {
            UnrealSpeechCodec::default()
        };

        Ok(Self {
            api_key,
            voice,
            bitrate: UnrealSpeechBitrate::default(),
            speed: super::DEFAULT_SPEED,
            pitch: super::DEFAULT_PITCH,
            codec,
        })
    }

    /// Build from the standardized config (W1 keystone). Maps the TTS features Unreal Speech can
    /// express to its real request fields: [`TtsFeatures::speed`] becomes the `speed` offset
    /// (`0.0` is normal, clamped to Unreal Speech's `-1.0..=1.0` range — same range
    /// [`validate_speed`] enforces) and [`TtsFeatures::pitch`] becomes the `pitch` multiplier
    /// (`1.0` is normal, clamped to `0.5..=1.5` per [`validate_pitch`]). The non-standard
    /// `bitrate` knob is read from the `extras` passthrough (a numeric kbps value or a string like
    /// `"320k"`). Unreal Speech has no field for volume, ElevenLabs-style voice settings, emotion,
    /// instructions, SSML, language, word timestamps, streaming, seed or sample rate, so those
    /// features are skipped.
    ///
    /// [`TtsFeatures::speed`]: crate::core::tts::standard::TtsFeatures::speed
    /// [`TtsFeatures::pitch`]: crate::core::tts::standard::TtsFeatures::pitch
    /// [`validate_speed`]: Self::validate_speed
    /// [`validate_pitch`]: Self::validate_pitch
    pub fn from_standard(std: &crate::core::tts::standard::StandardTTSConfig) -> TTSResult<Self> {
        let f = &std.features;
        let mut cfg = Self::from_base(&std.base)?;

        if let Some(speed) = f.speed {
            // Unreal Speech's speed is a 0.0-is-normal offset in -1.0..=1.0.
            cfg.speed = speed.clamp(-1.0, 1.0);
        }
        if let Some(pitch) = f.pitch {
            // Unreal Speech's pitch is a 1.0-is-normal multiplier in 0.5..=1.5.
            cfg.pitch = pitch.clamp(0.5, 1.5);
        }

        // Provider-specific passthrough.
        if let Some(bitrate) = std.extras.0.get("bitrate") {
            if let Some(kbps) = bitrate.as_u64() {
                cfg.bitrate = format!("{}k", kbps).parse().unwrap_or_default();
            } else if let Some(s) = bitrate.as_str() {
                cfg.bitrate = s.parse().unwrap_or_default();
            }
        }

        Ok(cfg)
    }

    /// Validate text length for stream endpoint
    pub fn validate_stream_text(text: &str) -> TTSResult<()> {
        if text.len() > super::MAX_STREAM_TEXT_LENGTH {
            return Err(TTSError::InvalidConfiguration(format!(
                "Text exceeds maximum length of {} characters for /stream endpoint (got {}). Use /speech for longer text.",
                super::MAX_STREAM_TEXT_LENGTH,
                text.len()
            )));
        }
        Ok(())
    }

    /// Validate text length for speech endpoint
    pub fn validate_speech_text(text: &str) -> TTSResult<()> {
        if text.len() > super::MAX_SPEECH_TEXT_LENGTH {
            return Err(TTSError::InvalidConfiguration(format!(
                "Text exceeds maximum length of {} characters for /speech endpoint (got {}). Use /synthesisTasks for longer text.",
                super::MAX_SPEECH_TEXT_LENGTH,
                text.len()
            )));
        }
        Ok(())
    }

    /// Validate speed value
    pub fn validate_speed(speed: f32) -> TTSResult<()> {
        if !(-1.0..=1.0).contains(&speed) {
            return Err(TTSError::InvalidConfiguration(format!(
                "Speed must be between -1.0 and 1.0 (got {})",
                speed
            )));
        }
        Ok(())
    }

    /// Validate pitch value
    pub fn validate_pitch(pitch: f32) -> TTSResult<()> {
        if !(0.5..=1.5).contains(&pitch) {
            return Err(TTSError::InvalidConfiguration(format!(
                "Pitch must be between 0.5 and 1.5 (got {})",
                pitch
            )));
        }
        Ok(())
    }

    /// Validate configuration
    pub fn validate(&self) -> TTSResult<()> {
        if self.api_key.is_empty() {
            return Err(TTSError::InvalidConfiguration(
                "API key is required".to_string(),
            ));
        }

        Self::validate_speed(self.speed)?;
        Self::validate_pitch(self.pitch)?;

        Ok(())
    }
}

// =============================================================================
// Config Builder
// =============================================================================

/// Builder for UnrealSpeechTtsConfig
#[derive(Debug, Default)]
pub struct UnrealSpeechTtsConfigBuilder {
    api_key: Option<String>,
    voice: Option<UnrealSpeechVoice>,
    bitrate: Option<UnrealSpeechBitrate>,
    speed: Option<f32>,
    pitch: Option<f32>,
    codec: Option<UnrealSpeechCodec>,
}

impl UnrealSpeechTtsConfigBuilder {
    /// Set the API key
    pub fn api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    /// Set the voice
    pub fn voice(mut self, voice: UnrealSpeechVoice) -> Self {
        self.voice = Some(voice);
        self
    }

    /// Set the bitrate
    pub fn bitrate(mut self, bitrate: UnrealSpeechBitrate) -> Self {
        self.bitrate = Some(bitrate);
        self
    }

    /// Set the speed
    pub fn speed(mut self, speed: f32) -> Self {
        self.speed = Some(speed);
        self
    }

    /// Set the pitch
    pub fn pitch(mut self, pitch: f32) -> Self {
        self.pitch = Some(pitch);
        self
    }

    /// Set the codec
    pub fn codec(mut self, codec: UnrealSpeechCodec) -> Self {
        self.codec = Some(codec);
        self
    }

    /// Build the configuration
    pub fn build(self) -> TTSResult<UnrealSpeechTtsConfig> {
        let config = UnrealSpeechTtsConfig {
            api_key: self.api_key.unwrap_or_default(),
            voice: self.voice.unwrap_or_default(),
            bitrate: self.bitrate.unwrap_or_default(),
            speed: self.speed.unwrap_or(super::DEFAULT_SPEED),
            pitch: self.pitch.unwrap_or(super::DEFAULT_PITCH),
            codec: self.codec.unwrap_or_default(),
        };

        config.validate()?;
        Ok(config)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // W1 keystone: the standardized features unlock Unreal Speech's speed offset and pitch
    // multiplier — plus the bitrate knob via the open extras passthrough — previously unreachable
    // through the flat factory (which always defaulted speed/pitch/bitrate).
    #[test]
    fn from_standard_maps_speed_pitch_and_bitrate_extras() {
        use crate::core::tts::standard::{ProviderExtras, StandardTTSConfig, TtsFeatures};
        let mut extras = serde_json::Map::new();
        extras.insert("bitrate".into(), serde_json::json!(320));
        let std = StandardTTSConfig {
            base: TTSConfig {
                provider: "unrealspeech".into(),
                api_key: "test-key".into(),
                voice_id: Some("Dan".into()),
                ..Default::default()
            },
            features: TtsFeatures {
                speed: Some(0.5),
                pitch: Some(1.2),
                ssml: Some(true), // capability gap: Unreal Speech has no SSML, must be ignored
                ..Default::default()
            },
            extras: ProviderExtras(extras),
        };
        let cfg = UnrealSpeechTtsConfig::from_standard(&std).unwrap();
        assert_eq!(cfg.speed, 0.5); // -1.0..=1.0 offset, passed through
        assert_eq!(cfg.pitch, 1.2); // 0.5..=1.5 multiplier, passed through
        assert_eq!(cfg.bitrate, UnrealSpeechBitrate::Bitrate320k); // extras passthrough
        assert_eq!(cfg.voice, UnrealSpeechVoice::Dan); // base passthrough preserved
    }

    #[test]
    fn test_voice_enum_standard() {
        assert_eq!(UnrealSpeechVoice::Scarlett.as_str(), "Scarlett");
        assert_eq!(UnrealSpeechVoice::Liv.as_str(), "Liv");
        assert_eq!(UnrealSpeechVoice::Amy.as_str(), "Amy");
        assert_eq!(UnrealSpeechVoice::Dan.as_str(), "Dan");
        assert_eq!(UnrealSpeechVoice::Will.as_str(), "Will");

        assert!(UnrealSpeechVoice::Scarlett.is_standard());
        assert!(!UnrealSpeechVoice::Scarlett.is_kokoro());
    }

    #[test]
    fn test_voice_enum_kokoro() {
        assert_eq!(UnrealSpeechVoice::Af.as_str(), "af");
        assert_eq!(UnrealSpeechVoice::AfBella.as_str(), "af_bella");
        assert_eq!(UnrealSpeechVoice::AmAdam.as_str(), "am_adam");
        assert_eq!(UnrealSpeechVoice::BfEmma.as_str(), "bf_emma");
        assert_eq!(UnrealSpeechVoice::BmGeorge.as_str(), "bm_george");

        assert!(!UnrealSpeechVoice::AfBella.is_standard());
        assert!(UnrealSpeechVoice::AfBella.is_kokoro());
    }

    #[test]
    fn test_voice_from_str() {
        assert_eq!(
            "scarlett".parse::<UnrealSpeechVoice>().unwrap(),
            UnrealSpeechVoice::Scarlett
        );
        assert_eq!(
            "dan".parse::<UnrealSpeechVoice>().unwrap(),
            UnrealSpeechVoice::Dan
        );
        assert_eq!(
            "af_bella".parse::<UnrealSpeechVoice>().unwrap(),
            UnrealSpeechVoice::AfBella
        );
        assert_eq!(
            "bella".parse::<UnrealSpeechVoice>().unwrap(),
            UnrealSpeechVoice::AfBella
        );
        assert_eq!(
            "am_adam".parse::<UnrealSpeechVoice>().unwrap(),
            UnrealSpeechVoice::AmAdam
        );

        // Custom voice ID
        let custom = "custom_voice".parse::<UnrealSpeechVoice>().unwrap();
        assert!(matches!(custom, UnrealSpeechVoice::Custom(_)));
    }

    #[test]
    fn test_codec_enum() {
        assert_eq!(UnrealSpeechCodec::Mp3.as_str(), "libmp3lame");
        assert_eq!(UnrealSpeechCodec::PcmMulaw.as_str(), "pcm_mulaw");

        assert_eq!(UnrealSpeechCodec::Mp3.mime_type(), "audio/mpeg");
        assert_eq!(UnrealSpeechCodec::PcmMulaw.mime_type(), "audio/basic");
    }

    #[test]
    fn test_codec_from_str() {
        assert_eq!(
            "libmp3lame".parse::<UnrealSpeechCodec>().unwrap(),
            UnrealSpeechCodec::Mp3
        );
        assert_eq!(
            "mp3".parse::<UnrealSpeechCodec>().unwrap(),
            UnrealSpeechCodec::Mp3
        );
        assert_eq!(
            "pcm_mulaw".parse::<UnrealSpeechCodec>().unwrap(),
            UnrealSpeechCodec::PcmMulaw
        );
        assert_eq!(
            "mulaw".parse::<UnrealSpeechCodec>().unwrap(),
            UnrealSpeechCodec::PcmMulaw
        );

        // Invalid codec
        assert!("invalid".parse::<UnrealSpeechCodec>().is_err());
    }

    #[test]
    fn test_bitrate_enum() {
        assert_eq!(UnrealSpeechBitrate::Bitrate16k.as_str(), "16k");
        assert_eq!(UnrealSpeechBitrate::Bitrate192k.as_str(), "192k");
        assert_eq!(UnrealSpeechBitrate::Bitrate320k.as_str(), "320k");

        assert_eq!(UnrealSpeechBitrate::Bitrate16k.kbps(), 16);
        assert_eq!(UnrealSpeechBitrate::Bitrate192k.kbps(), 192);
        assert_eq!(UnrealSpeechBitrate::Bitrate320k.kbps(), 320);
    }

    #[test]
    fn test_bitrate_from_str() {
        assert_eq!(
            "16k".parse::<UnrealSpeechBitrate>().unwrap(),
            UnrealSpeechBitrate::Bitrate16k
        );
        assert_eq!(
            "192k".parse::<UnrealSpeechBitrate>().unwrap(),
            UnrealSpeechBitrate::Bitrate192k
        );
        assert_eq!(
            "320k".parse::<UnrealSpeechBitrate>().unwrap(),
            UnrealSpeechBitrate::Bitrate320k
        );
        assert_eq!(
            "192".parse::<UnrealSpeechBitrate>().unwrap(),
            UnrealSpeechBitrate::Bitrate192k
        );

        // Invalid bitrate
        assert!("500k".parse::<UnrealSpeechBitrate>().is_err());
    }

    #[test]
    fn test_config_defaults() {
        let config = UnrealSpeechTtsConfig::default();
        assert_eq!(config.voice, UnrealSpeechVoice::Scarlett);
        assert_eq!(config.bitrate, UnrealSpeechBitrate::Bitrate192k);
        assert_eq!(config.speed, 0.0);
        assert_eq!(config.pitch, 1.0);
        assert_eq!(config.codec, UnrealSpeechCodec::Mp3);
    }

    #[test]
    fn test_config_from_base() {
        let base_config = TTSConfig {
            api_key: "test-api-key".to_string(),
            voice_id: Some("Will".to_string()),
            audio_format: Some("mp3".to_string()),
            ..Default::default()
        };

        let config = UnrealSpeechTtsConfig::from_base(&base_config).unwrap();
        assert_eq!(config.api_key, "test-api-key");
        assert_eq!(config.voice, UnrealSpeechVoice::Will);
        assert_eq!(config.codec, UnrealSpeechCodec::Mp3);
    }

    #[test]
    fn test_config_from_base_requires_api_key() {
        let base_config = TTSConfig {
            voice_id: Some("Scarlett".to_string()),
            ..Default::default()
        };

        let result = UnrealSpeechTtsConfig::from_base(&base_config);
        assert!(result.is_err());

        if let Err(TTSError::InvalidConfiguration(msg)) = result {
            assert!(msg.contains("API key") || msg.contains("UNREALSPEECH_API_KEY"));
        } else {
            panic!("Expected InvalidConfiguration error");
        }
    }

    #[test]
    fn test_config_validation() {
        let config = UnrealSpeechTtsConfig {
            api_key: "test-key".to_string(),
            voice: UnrealSpeechVoice::Scarlett,
            speed: 0.0,
            pitch: 1.0,
            ..Default::default()
        };
        assert!(config.validate().is_ok());

        // Empty API key
        let config = UnrealSpeechTtsConfig {
            api_key: "".to_string(),
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_speed_validation() {
        // Valid speed
        assert!(UnrealSpeechTtsConfig::validate_speed(0.0).is_ok());
        assert!(UnrealSpeechTtsConfig::validate_speed(-1.0).is_ok());
        assert!(UnrealSpeechTtsConfig::validate_speed(1.0).is_ok());
        assert!(UnrealSpeechTtsConfig::validate_speed(0.5).is_ok());

        // Invalid speed
        assert!(UnrealSpeechTtsConfig::validate_speed(-1.5).is_err());
        assert!(UnrealSpeechTtsConfig::validate_speed(1.5).is_err());
    }

    #[test]
    fn test_config_pitch_validation() {
        // Valid pitch
        assert!(UnrealSpeechTtsConfig::validate_pitch(1.0).is_ok());
        assert!(UnrealSpeechTtsConfig::validate_pitch(0.5).is_ok());
        assert!(UnrealSpeechTtsConfig::validate_pitch(1.5).is_ok());

        // Invalid pitch
        assert!(UnrealSpeechTtsConfig::validate_pitch(0.4).is_err());
        assert!(UnrealSpeechTtsConfig::validate_pitch(1.6).is_err());
    }

    #[test]
    fn test_text_validation() {
        // Valid text for stream
        let short_text = "Hello world".to_string();
        assert!(UnrealSpeechTtsConfig::validate_stream_text(&short_text).is_ok());

        // Text at stream limit
        let limit_text = "a".repeat(super::super::MAX_STREAM_TEXT_LENGTH);
        assert!(UnrealSpeechTtsConfig::validate_stream_text(&limit_text).is_ok());

        // Text over stream limit
        let over_limit_text = "a".repeat(super::super::MAX_STREAM_TEXT_LENGTH + 1);
        let result = UnrealSpeechTtsConfig::validate_stream_text(&over_limit_text);
        assert!(result.is_err());
        if let Err(TTSError::InvalidConfiguration(msg)) = result {
            assert!(msg.contains("1000"));
        }
    }

    #[test]
    fn test_config_builder() {
        let config = UnrealSpeechTtsConfig::builder()
            .api_key("test-key")
            .voice(UnrealSpeechVoice::Dan)
            .bitrate(UnrealSpeechBitrate::Bitrate320k)
            .speed(0.5)
            .pitch(0.9)
            .codec(UnrealSpeechCodec::Mp3)
            .build()
            .unwrap();

        assert_eq!(config.api_key, "test-key");
        assert_eq!(config.voice, UnrealSpeechVoice::Dan);
        assert_eq!(config.bitrate, UnrealSpeechBitrate::Bitrate320k);
        assert_eq!(config.speed, 0.5);
        assert_eq!(config.pitch, 0.9);
        assert_eq!(config.codec, UnrealSpeechCodec::Mp3);
    }

    #[test]
    fn test_stream_request_from_config() {
        let config = UnrealSpeechTtsConfig {
            api_key: "test-key".to_string(),
            voice: UnrealSpeechVoice::Scarlett,
            bitrate: UnrealSpeechBitrate::Bitrate192k,
            speed: 0.0,
            pitch: 1.0,
            codec: UnrealSpeechCodec::Mp3,
        };

        let request = UnrealSpeechStreamRequest::from_config(&config, "Hello world");

        assert_eq!(request.text, "Hello world");
        assert_eq!(request.voice_id, "Scarlett");
        assert_eq!(request.bitrate, "192k");
        assert_eq!(request.speed, 0.0);
        assert_eq!(request.pitch, 1.0);
        assert_eq!(request.codec, "libmp3lame");
    }

    #[test]
    fn test_stream_request_serialization() {
        let config = UnrealSpeechTtsConfig {
            api_key: "test-key".to_string(),
            voice: UnrealSpeechVoice::Dan,
            bitrate: UnrealSpeechBitrate::Bitrate320k,
            speed: 0.5,
            pitch: 0.92,
            codec: UnrealSpeechCodec::Mp3,
        };

        let request = UnrealSpeechStreamRequest::from_config(&config, "Test text");
        let json = serde_json::to_string(&request).unwrap();

        assert!(json.contains("\"Text\":\"Test text\""));
        assert!(json.contains("\"VoiceId\":\"Dan\""));
        assert!(json.contains("\"Bitrate\":\"320k\""));
        assert!(json.contains("\"Speed\":0.5"));
        assert!(json.contains("\"Pitch\":0.92"));
        assert!(json.contains("\"Codec\":\"libmp3lame\""));
    }

    #[test]
    fn test_voice_serialization() {
        let voice = UnrealSpeechVoice::AfBella;
        let json = serde_json::to_string(&voice).unwrap();
        assert_eq!(json, "\"af_bella\"");

        let deserialized: UnrealSpeechVoice = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, UnrealSpeechVoice::AfBella);
    }

    #[test]
    fn test_codec_serialization() {
        let codec = UnrealSpeechCodec::PcmMulaw;
        let json = serde_json::to_string(&codec).unwrap();
        assert_eq!(json, "\"pcm_mulaw\"");

        let deserialized: UnrealSpeechCodec = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, UnrealSpeechCodec::PcmMulaw);
    }

    #[test]
    fn test_bitrate_serialization() {
        let bitrate = UnrealSpeechBitrate::Bitrate256k;
        let json = serde_json::to_string(&bitrate).unwrap();
        assert_eq!(json, "\"256k\"");

        let deserialized: UnrealSpeechBitrate = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, UnrealSpeechBitrate::Bitrate256k);
    }
}
