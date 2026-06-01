//! Acapela Cloud TTS Configuration
//!
//! This module defines configuration structures and types for the Acapela Cloud TTS API.

use serde::{Deserialize, Serialize};
use std::fmt;

use crate::core::tts::base::{TTSConfig, TTSError, TTSResult};

// =============================================================================
// Audio Format Types
// =============================================================================

/// Acapela Cloud audio output formats
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AcapelaAudioFormat {
    /// MP3 format (default, compressed)
    #[default]
    Mp3,
    /// OGG Vorbis format
    Ogg,
    /// WAV format (uncompressed)
    Wav,
    /// FLAC format (lossless)
    Flac,
    /// AC3 Dolby Digital
    Ac3,
    /// ASF Advanced Systems Format
    Asf,
    /// WMA Windows Media Audio
    Wma,
    /// Opus Interactive Audio
    Opus,
    /// AAC Advanced Audio Coding
    Aac,
    /// AIFF Audio Interchange File Format
    Aiff,
    /// WebM Audio
    Webm,
    /// Matroska Audio
    Mka,
    /// Raw PCM 16-bit signed little-endian
    S16le,
    /// A-law companding
    Alaw,
    /// μ-law companding
    Mulaw,
    /// WAV with μ-law encoding
    WavMulaw,
    /// WAV with A-law encoding
    WavAlaw,
}

impl AcapelaAudioFormat {
    /// Get the API format identifier
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Mp3 => "mp3",
            Self::Ogg => "ogg",
            Self::Wav => "wav",
            Self::Flac => "flac",
            Self::Ac3 => "ac3",
            Self::Asf => "asf",
            Self::Wma => "wma",
            Self::Opus => "opus",
            Self::Aac => "aac",
            Self::Aiff => "aiff",
            Self::Webm => "webm",
            Self::Mka => "mka",
            Self::S16le => "s16le",
            Self::Alaw => "alaw",
            Self::Mulaw => "mulaw",
            Self::WavMulaw => "wav_mulaw",
            Self::WavAlaw => "wav_alaw",
        }
    }

    /// Get the MIME type for this format
    pub fn mime_type(&self) -> &'static str {
        match self {
            Self::Mp3 => "audio/mpeg",
            Self::Ogg => "audio/ogg",
            Self::Wav | Self::WavMulaw | Self::WavAlaw => "audio/wav",
            Self::Flac => "audio/flac",
            Self::Ac3 => "audio/ac3",
            Self::Asf => "audio/asf",
            Self::Wma => "audio/x-ms-wma",
            Self::Opus => "audio/opus",
            Self::Aac => "audio/aac",
            Self::Aiff => "audio/aiff",
            Self::Webm => "audio/webm",
            Self::Mka => "audio/x-matroska",
            Self::S16le | Self::Alaw | Self::Mulaw => "audio/raw",
        }
    }

    /// Check if format is raw (no container)
    pub fn is_raw(&self) -> bool {
        matches!(self, Self::S16le | Self::Alaw | Self::Mulaw)
    }

    /// Check if format is lossless
    pub fn is_lossless(&self) -> bool {
        matches!(self, Self::Wav | Self::Flac | Self::Aiff | Self::S16le)
    }
}

impl fmt::Display for AcapelaAudioFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl std::str::FromStr for AcapelaAudioFormat {
    type Err = TTSError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "mp3" | "mpeg" => Ok(Self::Mp3),
            "ogg" | "vorbis" => Ok(Self::Ogg),
            "wav" | "wave" => Ok(Self::Wav),
            "flac" => Ok(Self::Flac),
            "ac3" | "dolby" => Ok(Self::Ac3),
            "asf" => Ok(Self::Asf),
            "wma" => Ok(Self::Wma),
            "opus" => Ok(Self::Opus),
            "aac" => Ok(Self::Aac),
            "aiff" | "aif" => Ok(Self::Aiff),
            "webm" => Ok(Self::Webm),
            "mka" | "matroska" => Ok(Self::Mka),
            "s16le" | "pcm" | "pcm16" | "raw" | "linear16" => Ok(Self::S16le),
            "alaw" | "a-law" => Ok(Self::Alaw),
            "mulaw" | "mu-law" | "ulaw" | "u-law" => Ok(Self::Mulaw),
            "wav_mulaw" | "wavmulaw" => Ok(Self::WavMulaw),
            "wav_alaw" | "wavalaw" => Ok(Self::WavAlaw),
            _ => Err(TTSError::InvalidConfiguration(format!(
                "Unknown Acapela audio format: '{}'. Valid formats: mp3, ogg, wav, flac, ac3, asf, wma, opus, aac, aiff, webm, mka, s16le, alaw, mulaw, wav_mulaw, wav_alaw",
                s
            ))),
        }
    }
}

impl Serialize for AcapelaAudioFormat {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for AcapelaAudioFormat {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

// =============================================================================
// Output Mode Types
// =============================================================================

/// Acapela Cloud output modes
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AcapelaOutputMode {
    /// Real-time audio chunks (recommended for streaming)
    #[default]
    Stream,
    /// Complete audio file
    File,
    /// JSON event data only (no audio)
    Events,
}

impl AcapelaOutputMode {
    /// Get the API mode identifier
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Stream => "stream",
            Self::File => "file",
            Self::Events => "events",
        }
    }
}

impl fmt::Display for AcapelaOutputMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl std::str::FromStr for AcapelaOutputMode {
    type Err = TTSError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "stream" | "streaming" => Ok(Self::Stream),
            "file" | "complete" => Ok(Self::File),
            "events" | "event" => Ok(Self::Events),
            _ => Err(TTSError::InvalidConfiguration(format!(
                "Unknown Acapela output mode: '{}'. Valid modes: stream, file, events",
                s
            ))),
        }
    }
}

// =============================================================================
// Voice Types
// =============================================================================

/// Acapela voice information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcapelaVoice {
    /// Voice identifier (e.g., "alice", "graham")
    pub id: String,
    /// Display name
    #[serde(default)]
    pub name: Option<String>,
    /// Language code (e.g., "en-US", "fr-FR")
    #[serde(default)]
    pub language: Option<String>,
    /// Gender (male, female, child)
    #[serde(default)]
    pub gender: Option<String>,
    /// Voice type (standard, premium, neural)
    #[serde(default)]
    pub voice_type: Option<String>,
}

/// Account information from Acapela Cloud
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcapelaAccountInfo {
    /// User email
    #[serde(default)]
    pub email: Option<String>,
    /// Available credits
    #[serde(default)]
    pub credits: Option<i64>,
    /// List of available voice IDs
    #[serde(default)]
    pub voices: Vec<String>,
    /// First name
    #[serde(default)]
    pub first_name: Option<String>,
    /// Last name
    #[serde(default)]
    pub last_name: Option<String>,
    /// Company name
    #[serde(default)]
    pub company: Option<String>,
}

// =============================================================================
// Authentication
// =============================================================================

/// Acapela Cloud authentication credentials
#[derive(Debug, Clone)]
pub struct AcapelaCredentials {
    /// User email
    pub email: String,
    /// User password
    pub password: String,
}

impl AcapelaCredentials {
    /// Create credentials from email and password
    pub fn new(email: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            email: email.into(),
            password: password.into(),
        }
    }

    /// Parse credentials from "email:password" format
    pub fn from_api_key(api_key: &str) -> TTSResult<Self> {
        // Split on first colon (passwords might contain colons)
        if let Some(idx) = api_key.find(':') {
            let email = &api_key[..idx];
            let password = &api_key[idx + 1..];

            if email.is_empty() || password.is_empty() {
                return Err(TTSError::InvalidConfiguration(
                    "Acapela credentials must be in 'email:password' format".to_string(),
                ));
            }

            Ok(Self {
                email: email.to_string(),
                password: password.to_string(),
            })
        } else {
            Err(TTSError::InvalidConfiguration(
                "Acapela credentials must be in 'email:password' format. Set api_key to 'your-email@example.com:your-password'.".to_string(),
            ))
        }
    }
}

// =============================================================================
// Configuration
// =============================================================================

/// Acapela Cloud TTS provider configuration
#[derive(Debug, Clone)]
pub struct AcapelaTtsConfig {
    /// Authentication credentials
    pub credentials: AcapelaCredentials,

    /// Voice identifier (e.g., "alice", "graham", "lily")
    pub voice_id: String,

    /// Audio output format
    pub audio_format: AcapelaAudioFormat,

    /// Output mode (stream, file, events)
    pub output_mode: AcapelaOutputMode,

    /// Sample rate in Hz (8000-48000)
    pub sample_rate: u32,

    /// Audio bitrate in kbps (24-320, codec-dependent)
    pub bitrate: Option<u32>,

    /// Speech speed (30-300, 100 = normal)
    pub speed: u32,

    /// Volume level (50-65535)
    pub volume: u32,

    /// Voice shaping (50-150, 100 = normal)
    pub shaping: u32,

    /// Enable word position events for text highlighting
    pub word_positions: bool,

    /// Enable viseme/mouth position events for lip-sync
    pub mouth_positions: bool,

    /// Enable text marker position events
    pub mark_positions: bool,

    /// Custom dictionary file names (comma-separated)
    pub dictionaries: Option<String>,

    /// Application identifier for statistics
    pub application: Option<String>,
}

impl Default for AcapelaTtsConfig {
    fn default() -> Self {
        Self {
            credentials: AcapelaCredentials {
                email: String::new(),
                password: String::new(),
            },
            voice_id: super::DEFAULT_VOICE_ID.to_string(),
            audio_format: AcapelaAudioFormat::default(),
            output_mode: AcapelaOutputMode::default(),
            sample_rate: super::DEFAULT_SAMPLE_RATE,
            bitrate: None,
            speed: super::DEFAULT_SPEED,
            volume: super::DEFAULT_VOLUME,
            shaping: super::DEFAULT_SHAPING,
            word_positions: false,
            mouth_positions: false,
            mark_positions: false,
            dictionaries: None,
            application: None,
        }
    }
}

impl AcapelaTtsConfig {
    /// Create a new config builder
    pub fn builder() -> AcapelaTtsConfigBuilder {
        AcapelaTtsConfigBuilder::default()
    }

    /// Create configuration from base TTSConfig
    pub fn from_base(config: &TTSConfig) -> TTSResult<Self> {
        // Get credentials from config or environment
        let api_key = if !config.api_key.is_empty() {
            config.api_key.clone()
        } else {
            std::env::var("ACAPELA_API_KEY").unwrap_or_default()
        };

        if api_key.is_empty() {
            return Err(TTSError::InvalidConfiguration(
                "Acapela credentials are required. Set api_key to 'email:password' or ACAPELA_API_KEY environment variable.".to_string()
            ));
        }

        let credentials = AcapelaCredentials::from_api_key(&api_key)?;

        // Get voice ID from config
        let voice_id = config
            .voice_id
            .clone()
            .unwrap_or_else(|| super::DEFAULT_VOICE_ID.to_string());

        // Parse audio format from config
        let audio_format = if let Some(ref fmt) = config.audio_format {
            if !fmt.is_empty() {
                fmt.parse().unwrap_or_default()
            } else {
                AcapelaAudioFormat::default()
            }
        } else {
            AcapelaAudioFormat::default()
        };

        // Parse sample rate from config (use provider's sample_rate if set)
        let sample_rate = match config.sample_rate {
            Some(sr) if sr > 0 => sr.clamp(super::MIN_SAMPLE_RATE, super::MAX_SAMPLE_RATE),
            _ => super::DEFAULT_SAMPLE_RATE,
        };

        Ok(Self {
            credentials,
            voice_id,
            audio_format,
            output_mode: AcapelaOutputMode::Stream,
            sample_rate,
            bitrate: None,
            speed: super::DEFAULT_SPEED,
            volume: super::DEFAULT_VOLUME,
            shaping: super::DEFAULT_SHAPING,
            word_positions: false,
            mouth_positions: false,
            mark_positions: false,
            dictionaries: None,
            application: None,
        })
    }

    /// Build from the standardized config (W1 keystone). Maps the TTS features Acapela can express
    /// to real query parameters: [`TtsFeatures::speed`] (a `1.0`-normal multiplier) becomes the
    /// integer `speed` (where `100` is normal, clamped to `MIN_SPEED..=MAX_SPEED`),
    /// [`TtsFeatures::volume`] becomes the integer `volume` level (clamped to
    /// `MIN_VOLUME..=MAX_VOLUME`), [`TtsFeatures::sample_rate`] overrides the output `sample_rate`
    /// (clamped to `MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE`), and [`TtsFeatures::word_timestamps`]
    /// enables word position events (`word_positions`). The non-standard `bitrate`, `dictionaries`
    /// and `application` knobs are read from the `extras` passthrough. Acapela has no field for
    /// pitch, ElevenLabs-style voice settings, emotion, instructions, SSML, language, streaming or
    /// seed, so those features are skipped.
    ///
    /// [`TtsFeatures::speed`]: crate::core::tts::standard::TtsFeatures::speed
    /// [`TtsFeatures::volume`]: crate::core::tts::standard::TtsFeatures::volume
    /// [`TtsFeatures::sample_rate`]: crate::core::tts::standard::TtsFeatures::sample_rate
    /// [`TtsFeatures::word_timestamps`]: crate::core::tts::standard::TtsFeatures::word_timestamps
    pub fn from_standard(std: &crate::core::tts::standard::StandardTTSConfig) -> TTSResult<Self> {
        let f = &std.features;
        let mut cfg = Self::from_base(&std.base)?;

        if let Some(speed) = f.speed {
            // Standardized speed is a 1.0-is-normal multiplier; Acapela uses 100-is-normal.
            cfg.speed =
                ((speed * 100.0).round() as u32).clamp(super::MIN_SPEED, super::MAX_SPEED);
        }
        if let Some(volume) = f.volume {
            cfg.volume = (volume.round() as u32).clamp(super::MIN_VOLUME, super::MAX_VOLUME);
        }
        if let Some(rate) = f.sample_rate {
            cfg.sample_rate = rate.clamp(super::MIN_SAMPLE_RATE, super::MAX_SAMPLE_RATE);
        }
        if let Some(true) = f.word_timestamps {
            cfg.word_positions = true;
        }

        // Provider-specific passthrough.
        if let Some(bitrate) = std.extras.0.get("bitrate").and_then(|v| v.as_u64()) {
            cfg.bitrate = Some(bitrate as u32);
        }
        if let Some(dico) = std.extras.0.get("dictionaries").and_then(|v| v.as_str()) {
            cfg.dictionaries = Some(dico.to_string());
        }
        if let Some(app) = std.extras.0.get("application").and_then(|v| v.as_str()) {
            cfg.application = Some(app.to_string());
        }

        Ok(cfg)
    }

    /// Validate text length
    pub fn validate_text(&self, text: &str) -> TTSResult<()> {
        let max_len = match self.output_mode {
            AcapelaOutputMode::Stream => super::MAX_TEXT_LENGTH_STREAM,
            _ => super::MAX_TEXT_LENGTH_GET,
        };

        if text.len() > max_len {
            return Err(TTSError::InvalidConfiguration(format!(
                "Text exceeds maximum length of {} characters for {} output (got {}). Split into multiple requests.",
                max_len,
                self.output_mode,
                text.len()
            )));
        }
        Ok(())
    }

    /// Validate configuration
    pub fn validate(&self) -> TTSResult<()> {
        if self.credentials.email.is_empty() || self.credentials.password.is_empty() {
            return Err(TTSError::InvalidConfiguration(
                "Email and password are required".to_string(),
            ));
        }

        if self.voice_id.is_empty() {
            return Err(TTSError::InvalidConfiguration(
                "voice_id is required".to_string(),
            ));
        }

        if self.sample_rate < super::MIN_SAMPLE_RATE || self.sample_rate > super::MAX_SAMPLE_RATE {
            return Err(TTSError::InvalidConfiguration(format!(
                "Sample rate must be between {} and {} Hz (got {})",
                super::MIN_SAMPLE_RATE,
                super::MAX_SAMPLE_RATE,
                self.sample_rate
            )));
        }

        if self.speed < super::MIN_SPEED || self.speed > super::MAX_SPEED {
            return Err(TTSError::InvalidConfiguration(format!(
                "Speed must be between {} and {} (got {})",
                super::MIN_SPEED,
                super::MAX_SPEED,
                self.speed
            )));
        }

        if self.volume < super::MIN_VOLUME || self.volume > super::MAX_VOLUME {
            return Err(TTSError::InvalidConfiguration(format!(
                "Volume must be between {} and {} (got {})",
                super::MIN_VOLUME,
                super::MAX_VOLUME,
                self.volume
            )));
        }

        if self.shaping < super::MIN_SHAPING || self.shaping > super::MAX_SHAPING {
            return Err(TTSError::InvalidConfiguration(format!(
                "Shaping must be between {} and {} (got {})",
                super::MIN_SHAPING,
                super::MAX_SHAPING,
                self.shaping
            )));
        }

        Ok(())
    }

    /// Build query parameters for the command endpoint
    pub fn build_query_params(&self, text: &str) -> Vec<(&'static str, String)> {
        let mut params = vec![
            ("voice", self.voice_id.clone()),
            ("text", text.to_string()),
            ("output", self.output_mode.as_str().to_string()),
            ("type", self.audio_format.as_str().to_string()),
            ("samplerate", self.sample_rate.to_string()),
            ("speed", self.speed.to_string()),
            ("volume", self.volume.to_string()),
            ("shaping", self.shaping.to_string()),
        ];

        if let Some(bitrate) = self.bitrate {
            params.push(("bitrate", bitrate.to_string()));
        }

        if self.word_positions {
            params.push(("wordpos", "on".to_string()));
        }

        if self.mouth_positions {
            params.push(("mouthpos", "on".to_string()));
        }

        if self.mark_positions {
            params.push(("markpos", "on".to_string()));
        }

        if let Some(ref dico) = self.dictionaries {
            params.push(("dico", dico.clone()));
        }

        if let Some(ref app) = self.application {
            params.push(("application", app.clone()));
        }

        params
    }
}

// =============================================================================
// Config Builder
// =============================================================================

/// Builder for AcapelaTtsConfig
#[derive(Debug, Default)]
pub struct AcapelaTtsConfigBuilder {
    email: Option<String>,
    password: Option<String>,
    voice_id: Option<String>,
    audio_format: Option<AcapelaAudioFormat>,
    output_mode: Option<AcapelaOutputMode>,
    sample_rate: Option<u32>,
    bitrate: Option<u32>,
    speed: Option<u32>,
    volume: Option<u32>,
    shaping: Option<u32>,
    word_positions: bool,
    mouth_positions: bool,
    mark_positions: bool,
    dictionaries: Option<String>,
    application: Option<String>,
}

impl AcapelaTtsConfigBuilder {
    /// Set the email
    pub fn email(mut self, email: impl Into<String>) -> Self {
        self.email = Some(email.into());
        self
    }

    /// Set the password
    pub fn password(mut self, password: impl Into<String>) -> Self {
        self.password = Some(password.into());
        self
    }

    /// Set the voice ID
    pub fn voice_id(mut self, voice_id: impl Into<String>) -> Self {
        self.voice_id = Some(voice_id.into());
        self
    }

    /// Set the audio format
    pub fn audio_format(mut self, format: AcapelaAudioFormat) -> Self {
        self.audio_format = Some(format);
        self
    }

    /// Set the output mode
    pub fn output_mode(mut self, mode: AcapelaOutputMode) -> Self {
        self.output_mode = Some(mode);
        self
    }

    /// Set the sample rate
    pub fn sample_rate(mut self, rate: u32) -> Self {
        self.sample_rate = Some(rate);
        self
    }

    /// Set the bitrate
    pub fn bitrate(mut self, bitrate: u32) -> Self {
        self.bitrate = Some(bitrate);
        self
    }

    /// Set the speech speed
    pub fn speed(mut self, speed: u32) -> Self {
        self.speed = Some(speed);
        self
    }

    /// Set the volume level
    pub fn volume(mut self, volume: u32) -> Self {
        self.volume = Some(volume);
        self
    }

    /// Set the voice shaping
    pub fn shaping(mut self, shaping: u32) -> Self {
        self.shaping = Some(shaping);
        self
    }

    /// Enable word position events
    pub fn with_word_positions(mut self) -> Self {
        self.word_positions = true;
        self
    }

    /// Enable mouth position events (visemes)
    pub fn with_mouth_positions(mut self) -> Self {
        self.mouth_positions = true;
        self
    }

    /// Enable mark position events
    pub fn with_mark_positions(mut self) -> Self {
        self.mark_positions = true;
        self
    }

    /// Set custom dictionary files
    pub fn dictionaries(mut self, dico: impl Into<String>) -> Self {
        self.dictionaries = Some(dico.into());
        self
    }

    /// Set application identifier
    pub fn application(mut self, app: impl Into<String>) -> Self {
        self.application = Some(app.into());
        self
    }

    /// Build the configuration
    pub fn build(self) -> TTSResult<AcapelaTtsConfig> {
        let config = AcapelaTtsConfig {
            credentials: AcapelaCredentials {
                email: self.email.unwrap_or_default(),
                password: self.password.unwrap_or_default(),
            },
            voice_id: self
                .voice_id
                .unwrap_or_else(|| super::DEFAULT_VOICE_ID.to_string()),
            audio_format: self.audio_format.unwrap_or_default(),
            output_mode: self.output_mode.unwrap_or_default(),
            sample_rate: self.sample_rate.unwrap_or(super::DEFAULT_SAMPLE_RATE),
            bitrate: self.bitrate,
            speed: self.speed.unwrap_or(super::DEFAULT_SPEED),
            volume: self.volume.unwrap_or(super::DEFAULT_VOLUME),
            shaping: self.shaping.unwrap_or(super::DEFAULT_SHAPING),
            word_positions: self.word_positions,
            mouth_positions: self.mouth_positions,
            mark_positions: self.mark_positions,
            dictionaries: self.dictionaries,
            application: self.application,
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

    // W1 keystone: the standardized features Acapela can express (speed, volume, output sample
    // rate, word-timing events) reach their real query-parameter fields, and the non-standard
    // bitrate / dictionaries / application knobs flow through the extras passthrough.
    #[test]
    fn from_standard_maps_speed_volume_rate_and_word_positions() {
        use crate::core::tts::standard::{ProviderExtras, StandardTTSConfig, TtsFeatures};
        let mut extras = serde_json::Map::new();
        extras.insert("bitrate".into(), serde_json::json!(128));
        extras.insert("dictionaries".into(), serde_json::json!("custom.dic"));
        extras.insert("application".into(), serde_json::json!("my-app"));
        let std = StandardTTSConfig {
            base: TTSConfig {
                provider: "acapela".into(),
                api_key: "user@example.com:password123".into(),
                sample_rate: Some(22050),
                ..Default::default()
            },
            features: TtsFeatures {
                speed: Some(1.2),
                volume: Some(40000.0),
                sample_rate: Some(16000),
                word_timestamps: Some(true),
                ssml: Some(true), // capability gap: Acapela has no SSML field, must be ignored
                ..Default::default()
            },
            extras: ProviderExtras(extras),
        };
        let cfg = AcapelaTtsConfig::from_standard(&std).unwrap();
        assert_eq!(cfg.speed, 120); // 1.2 multiplier -> 120 (100 = normal)
        assert_eq!(cfg.volume, 40000);
        assert_eq!(cfg.sample_rate, 16000);
        assert!(cfg.word_positions);
        assert_eq!(cfg.bitrate, Some(128)); // extras passthrough
        assert_eq!(cfg.dictionaries, Some("custom.dic".to_string()));
        assert_eq!(cfg.application, Some("my-app".to_string()));
    }

    #[test]
    fn test_audio_format_enum() {
        assert_eq!(AcapelaAudioFormat::Mp3.as_str(), "mp3");
        assert_eq!(AcapelaAudioFormat::Wav.as_str(), "wav");
        assert_eq!(AcapelaAudioFormat::S16le.as_str(), "s16le");
        assert_eq!(AcapelaAudioFormat::WavMulaw.as_str(), "wav_mulaw");
    }

    #[test]
    fn test_audio_format_from_str() {
        assert_eq!(
            "mp3".parse::<AcapelaAudioFormat>().unwrap(),
            AcapelaAudioFormat::Mp3
        );
        assert_eq!(
            "wav".parse::<AcapelaAudioFormat>().unwrap(),
            AcapelaAudioFormat::Wav
        );
        assert_eq!(
            "linear16".parse::<AcapelaAudioFormat>().unwrap(),
            AcapelaAudioFormat::S16le
        );
        assert_eq!(
            "pcm".parse::<AcapelaAudioFormat>().unwrap(),
            AcapelaAudioFormat::S16le
        );
        assert_eq!(
            "mulaw".parse::<AcapelaAudioFormat>().unwrap(),
            AcapelaAudioFormat::Mulaw
        );

        // Test invalid format
        assert!("invalid".parse::<AcapelaAudioFormat>().is_err());
    }

    #[test]
    fn test_audio_format_mime_type() {
        assert_eq!(AcapelaAudioFormat::Mp3.mime_type(), "audio/mpeg");
        assert_eq!(AcapelaAudioFormat::Wav.mime_type(), "audio/wav");
        assert_eq!(AcapelaAudioFormat::S16le.mime_type(), "audio/raw");
        assert_eq!(AcapelaAudioFormat::Ogg.mime_type(), "audio/ogg");
    }

    #[test]
    fn test_audio_format_properties() {
        assert!(AcapelaAudioFormat::S16le.is_raw());
        assert!(AcapelaAudioFormat::Alaw.is_raw());
        assert!(!AcapelaAudioFormat::Mp3.is_raw());

        assert!(AcapelaAudioFormat::Wav.is_lossless());
        assert!(AcapelaAudioFormat::Flac.is_lossless());
        assert!(!AcapelaAudioFormat::Mp3.is_lossless());
    }

    #[test]
    fn test_output_mode_enum() {
        assert_eq!(AcapelaOutputMode::Stream.as_str(), "stream");
        assert_eq!(AcapelaOutputMode::File.as_str(), "file");
        assert_eq!(AcapelaOutputMode::Events.as_str(), "events");
    }

    #[test]
    fn test_output_mode_from_str() {
        assert_eq!(
            "stream".parse::<AcapelaOutputMode>().unwrap(),
            AcapelaOutputMode::Stream
        );
        assert_eq!(
            "file".parse::<AcapelaOutputMode>().unwrap(),
            AcapelaOutputMode::File
        );
        assert_eq!(
            "events".parse::<AcapelaOutputMode>().unwrap(),
            AcapelaOutputMode::Events
        );
    }

    #[test]
    fn test_credentials_from_api_key() {
        // Valid format
        let creds = AcapelaCredentials::from_api_key("user@example.com:password123").unwrap();
        assert_eq!(creds.email, "user@example.com");
        assert_eq!(creds.password, "password123");

        // Password with colon
        let creds = AcapelaCredentials::from_api_key("user@example.com:pass:word:123").unwrap();
        assert_eq!(creds.email, "user@example.com");
        assert_eq!(creds.password, "pass:word:123");

        // Invalid formats
        assert!(AcapelaCredentials::from_api_key("no-colon").is_err());
        assert!(AcapelaCredentials::from_api_key(":password").is_err());
        assert!(AcapelaCredentials::from_api_key("email:").is_err());
    }

    #[test]
    fn test_config_defaults() {
        let config = AcapelaTtsConfig::default();
        assert_eq!(config.voice_id, "alice");
        assert_eq!(config.audio_format, AcapelaAudioFormat::Mp3);
        assert_eq!(config.output_mode, AcapelaOutputMode::Stream);
        assert_eq!(config.sample_rate, 22050);
        assert_eq!(config.speed, 100);
        assert_eq!(config.volume, 32768);
        assert_eq!(config.shaping, 100);
        assert!(!config.word_positions);
        assert!(!config.mouth_positions);
    }

    #[test]
    fn test_config_from_base() {
        let base_config = TTSConfig {
            api_key: "user@example.com:password123".to_string(),
            voice_id: Some("graham".to_string()),
            audio_format: Some("wav".to_string()),
            sample_rate: Some(16000),
            ..Default::default()
        };

        let config = AcapelaTtsConfig::from_base(&base_config).unwrap();
        assert_eq!(config.credentials.email, "user@example.com");
        assert_eq!(config.credentials.password, "password123");
        assert_eq!(config.voice_id, "graham");
        assert_eq!(config.audio_format, AcapelaAudioFormat::Wav);
        assert_eq!(config.sample_rate, 16000);
    }

    #[test]
    fn test_config_from_base_requires_credentials() {
        let base_config = TTSConfig {
            voice_id: Some("alice".to_string()),
            ..Default::default()
        };

        let result = AcapelaTtsConfig::from_base(&base_config);
        assert!(result.is_err());

        if let Err(TTSError::InvalidConfiguration(msg)) = result {
            assert!(msg.contains("credentials") || msg.contains("ACAPELA"));
        } else {
            panic!("Expected InvalidConfiguration error");
        }
    }

    #[test]
    fn test_config_validation() {
        // Valid config
        let config = AcapelaTtsConfig::builder()
            .email("user@example.com")
            .password("password")
            .voice_id("alice")
            .build()
            .unwrap();
        assert!(config.validate().is_ok());

        // Empty email
        let config = AcapelaTtsConfig {
            credentials: AcapelaCredentials::new("", "password"),
            ..Default::default()
        };
        assert!(config.validate().is_err());

        // Speed out of range
        let config = AcapelaTtsConfig {
            credentials: AcapelaCredentials::new("email", "password"),
            speed: 10, // Too low
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_text_validation() {
        let config = AcapelaTtsConfig {
            output_mode: AcapelaOutputMode::Stream,
            ..Default::default()
        };

        // Valid text
        assert!(config.validate_text("Hello world").is_ok());

        // Text at stream limit
        let limit_text = "a".repeat(super::super::MAX_TEXT_LENGTH_STREAM);
        assert!(config.validate_text(&limit_text).is_ok());

        // Text over stream limit
        let over_limit_text = "a".repeat(super::super::MAX_TEXT_LENGTH_STREAM + 1);
        assert!(config.validate_text(&over_limit_text).is_err());

        // File mode has lower limit
        let config = AcapelaTtsConfig {
            output_mode: AcapelaOutputMode::File,
            ..Default::default()
        };
        let file_limit_text = "a".repeat(super::super::MAX_TEXT_LENGTH_GET + 1);
        assert!(config.validate_text(&file_limit_text).is_err());
    }

    #[test]
    fn test_build_query_params() {
        let config = AcapelaTtsConfig::builder()
            .email("user@example.com")
            .password("password")
            .voice_id("graham")
            .audio_format(AcapelaAudioFormat::Wav)
            .sample_rate(16000)
            .speed(120)
            .with_word_positions()
            .application("test-app")
            .build()
            .unwrap();

        let params = config.build_query_params("Hello world");

        // Convert to map for easier testing
        let params_map: std::collections::HashMap<_, _> = params.into_iter().collect();

        assert_eq!(params_map.get("voice"), Some(&"graham".to_string()));
        assert_eq!(params_map.get("text"), Some(&"Hello world".to_string()));
        assert_eq!(params_map.get("output"), Some(&"stream".to_string()));
        assert_eq!(params_map.get("type"), Some(&"wav".to_string()));
        assert_eq!(params_map.get("samplerate"), Some(&"16000".to_string()));
        assert_eq!(params_map.get("speed"), Some(&"120".to_string()));
        assert_eq!(params_map.get("wordpos"), Some(&"on".to_string()));
        assert_eq!(params_map.get("application"), Some(&"test-app".to_string()));
        assert!(params_map.get("mouthpos").is_none()); // Not enabled
    }

    #[test]
    fn test_config_builder() {
        let config = AcapelaTtsConfig::builder()
            .email("user@example.com")
            .password("password")
            .voice_id("lily")
            .audio_format(AcapelaAudioFormat::Ogg)
            .output_mode(AcapelaOutputMode::File)
            .sample_rate(24000)
            .bitrate(128)
            .speed(110)
            .volume(40000)
            .shaping(90)
            .with_word_positions()
            .with_mouth_positions()
            .dictionaries("custom.dic,extra.dic")
            .application("my-app")
            .build()
            .unwrap();

        assert_eq!(config.credentials.email, "user@example.com");
        assert_eq!(config.voice_id, "lily");
        assert_eq!(config.audio_format, AcapelaAudioFormat::Ogg);
        assert_eq!(config.output_mode, AcapelaOutputMode::File);
        assert_eq!(config.sample_rate, 24000);
        assert_eq!(config.bitrate, Some(128));
        assert_eq!(config.speed, 110);
        assert_eq!(config.volume, 40000);
        assert_eq!(config.shaping, 90);
        assert!(config.word_positions);
        assert!(config.mouth_positions);
        assert!(!config.mark_positions);
        assert_eq!(
            config.dictionaries,
            Some("custom.dic,extra.dic".to_string())
        );
        assert_eq!(config.application, Some("my-app".to_string()));
    }

    #[test]
    fn test_audio_format_serialization() {
        let format = AcapelaAudioFormat::Flac;
        let json = serde_json::to_string(&format).unwrap();
        assert_eq!(json, "\"flac\"");

        let deserialized: AcapelaAudioFormat = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, AcapelaAudioFormat::Flac);
    }

    #[test]
    fn test_voice_deserialization() {
        let json = r#"{
            "id": "alice",
            "name": "Alice",
            "language": "fr-FR",
            "gender": "female",
            "voice_type": "neural"
        }"#;

        let voice: AcapelaVoice = serde_json::from_str(json).unwrap();
        assert_eq!(voice.id, "alice");
        assert_eq!(voice.name, Some("Alice".to_string()));
        assert_eq!(voice.language, Some("fr-FR".to_string()));
        assert_eq!(voice.gender, Some("female".to_string()));
        assert_eq!(voice.voice_type, Some("neural".to_string()));
    }

    #[test]
    fn test_account_info_deserialization() {
        let json = r#"{
            "email": "user@example.com",
            "credits": 10000,
            "voices": ["alice", "graham", "lily"],
            "first_name": "John",
            "last_name": "Doe"
        }"#;

        let account: AcapelaAccountInfo = serde_json::from_str(json).unwrap();
        assert_eq!(account.email, Some("user@example.com".to_string()));
        assert_eq!(account.credits, Some(10000));
        assert_eq!(account.voices, vec!["alice", "graham", "lily"]);
        assert_eq!(account.first_name, Some("John".to_string()));
    }
}
