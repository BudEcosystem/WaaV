//! Speechify TTS Configuration
//!
//! This module defines configuration structures and types for the Speechify TTS API.

use serde::{Deserialize, Serialize};
use std::fmt;

use crate::core::tts::base::{TTSConfig, TTSError, TTSResult};

fn validate_speechify_tts_endpoint(source: &str, endpoint: &str) -> Result<(), String> {
    let endpoint = endpoint.trim();
    if endpoint.is_empty() {
        return Ok(());
    }
    crate::core::net::validate_url_for_ssrf(endpoint, crate::core::net::HTTP_URL_SCHEMES)
        .map_err(|msg| format!("{source} rejected (SSRF protection): {msg}"))
}

// =============================================================================
// Model Types
// =============================================================================

/// Speechify TTS models (<https://docs.speechify.ai/build/guides/concepts/models>).
///
/// The default is `simba-3.0`, the model Speechify itself applies when a request names none, and
/// it is never sent: `SpeechifyStreamRequest::from_config` leaves `model` off for the default, so
/// each workspace keeps the model its API version selects. `simba-3.5` (Speechify's
/// recommendation, 48 languages) and `simba-3.2` are sent when a config names them. Speechify
/// retired `simba-english` and `simba-multilingual` (Simba 1.6): a workspace created since answers
/// `400 model_retired`, while one pinned to an older API version may still take them, so a config
/// that names one is sent as written. `simba-turbo` and `simba-base` are absent from the current
/// docs and are likewise only sent when asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SpeechifyModel {
    /// Simba 3.5: 48 languages under one model id, English included (set `language`).
    Simba35,
    /// Simba 3.2: English.
    Simba32,
    /// Simba 3.0: English, German, Spanish, French, Italian and Portuguese; the API's default.
    #[default]
    Simba30,
    /// Simba 1.6 English (retired for new workspaces).
    SimbaEnglish,
    /// Not in Speechify's current docs.
    SimbaTurbo,
    /// Simba 1.6 multilingual (retired for new workspaces).
    SimbaMultilingual,
    /// Not in Speechify's current docs.
    SimbaBase,
}

impl SpeechifyModel {
    /// Get the API model identifier
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Simba35 => "simba-3.5",
            Self::Simba32 => "simba-3.2",
            Self::Simba30 => "simba-3.0",
            Self::SimbaEnglish => "simba-english",
            Self::SimbaTurbo => "simba-turbo",
            Self::SimbaMultilingual => "simba-multilingual",
            Self::SimbaBase => "simba-base",
        }
    }

    /// The `model` a request carries: none for the default, and none for `simba-english`, the
    /// default before Simba 3 — WaaV never sent it, and sending it now would draw
    /// `400 model_retired` from every workspace created since its retirement.
    pub fn wire_value(&self) -> Option<&'static str> {
        match self {
            Self::Simba30 | Self::SimbaEnglish => None,
            other => Some(other.as_str()),
        }
    }

    /// Check if model supports multilingual
    pub fn is_multilingual(&self) -> bool {
        matches!(
            self,
            Self::Simba35 | Self::Simba30 | Self::SimbaMultilingual
        )
    }

    /// Check if model supports emotion control
    pub fn supports_emotion(&self) -> bool {
        matches!(self, Self::SimbaTurbo)
    }
}

impl fmt::Display for SpeechifyModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl std::str::FromStr for SpeechifyModel {
    type Err = TTSError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "simba-3.5" | "simba_3.5" | "simba-3-5" => Ok(Self::Simba35),
            "simba-3.2" | "simba_3.2" | "simba-3-2" => Ok(Self::Simba32),
            "simba-3.0" | "simba_3.0" | "simba-3-0" | "simba-3" => Ok(Self::Simba30),
            "simba-english" | "simba_english" | "simbaenglish" | "english" => {
                Ok(Self::SimbaEnglish)
            }
            "simba-turbo" | "simba_turbo" | "simbaturbo" | "turbo" => Ok(Self::SimbaTurbo),
            "simba-multilingual" | "simba_multilingual" | "simbamultilingual" | "multilingual" => {
                Ok(Self::SimbaMultilingual)
            }
            "simba-base" | "simba_base" | "simbabase" | "base" => Ok(Self::SimbaBase),
            _ => Err(TTSError::InvalidConfiguration(format!(
                "Unknown Speechify model: '{}'. Valid models: simba-3.5, simba-3.2, simba-3.0 (and the retired simba-english, simba-multilingual)",
                s
            ))),
        }
    }
}

impl Serialize for SpeechifyModel {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for SpeechifyModel {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

// =============================================================================
// Audio Format Types
// =============================================================================

/// Speechify audio output formats
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SpeechifyAudioFormat {
    /// WAV format at 48000 Hz (default, uncompressed)
    #[default]
    Wav48000,
    /// MP3 format at 24000 Hz (compressed)
    Mp3_24000,
    /// OGG Vorbis format at 24000 Hz
    Ogg24000,
    /// AAC format at 24000 Hz
    Aac24000,
}

impl SpeechifyAudioFormat {
    /// Get the API format identifier
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Wav48000 => "wav_48000",
            Self::Mp3_24000 => "mp3_24000",
            Self::Ogg24000 => "ogg_24000",
            Self::Aac24000 => "aac_24000",
        }
    }

    /// Get the sample rate for this format
    pub fn sample_rate(&self) -> u32 {
        match self {
            Self::Wav48000 => 48000,
            Self::Mp3_24000 | Self::Ogg24000 | Self::Aac24000 => 24000,
        }
    }

    /// Get the MIME type for this format
    pub fn mime_type(&self) -> &'static str {
        match self {
            Self::Wav48000 => "audio/wav",
            Self::Mp3_24000 => "audio/mpeg",
            Self::Ogg24000 => "audio/ogg",
            Self::Aac24000 => "audio/aac",
        }
    }
}

impl fmt::Display for SpeechifyAudioFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl std::str::FromStr for SpeechifyAudioFormat {
    type Err = TTSError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "wav_48000" | "wav48000" | "wav" => Ok(Self::Wav48000),
            "mp3_24000" | "mp324000" | "mp3" => Ok(Self::Mp3_24000),
            "ogg_24000" | "ogg24000" | "ogg" => Ok(Self::Ogg24000),
            "aac_24000" | "aac24000" | "aac" => Ok(Self::Aac24000),
            _ => Err(TTSError::InvalidConfiguration(format!(
                "Unknown Speechify audio format: '{}'. Valid formats: wav_48000, mp3_24000, ogg_24000, aac_24000",
                s
            ))),
        }
    }
}

impl Serialize for SpeechifyAudioFormat {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for SpeechifyAudioFormat {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

// =============================================================================
// Voice Types
// =============================================================================

/// One voice from `GET /v1/voices` (Speechify's `GetVoice`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeechifyVoice {
    /// Unique voice identifier
    pub id: String,
    /// Display name
    pub display_name: String,
    /// `male`, `female` or `not_specified`
    #[serde(default)]
    pub gender: Option<String>,
    /// The voice's own locale (BCP-47)
    #[serde(default)]
    pub locale: Option<String>,
    /// `shared` (the public catalogue) or `personal` (a workspace clone)
    #[serde(default, rename = "type")]
    pub voice_type: Option<String>,
    /// Models this voice synthesizes with, and the locales each covers
    #[serde(default)]
    pub models: Vec<SpeechifyVoiceModel>,
    /// Sample audio URL
    #[serde(default)]
    pub preview_audio: Option<String>,
    /// Descriptive tags
    #[serde(default)]
    pub tags: Option<Vec<String>>,
}

impl SpeechifyVoice {
    /// Whether this is a workspace clone rather than a catalogue voice.
    pub fn is_cloned(&self) -> bool {
        self.voice_type.as_deref() == Some("personal")
    }
}

/// A model a voice supports (`GetVoicesModel`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeechifyVoiceModel {
    /// Model id (`simba-3.5`, …)
    pub name: String,
    /// Locales the voice speaks with this model
    #[serde(default)]
    pub languages: Vec<SpeechifyVoiceLanguage>,
}

/// A locale a voice speaks under one model (`GetVoiceLanguage`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeechifyVoiceLanguage {
    /// BCP-47 locale
    pub locale: String,
    /// Sample audio URL for this locale
    #[serde(default)]
    pub preview_audio: Option<String>,
}

/// One page of `GET /v1/voices`. A short page is not the end: follow `next_cursor` while
/// `has_more` is true.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeechifyVoicesResponse {
    /// Voices on this page
    pub voices: Vec<SpeechifyVoice>,
    /// Whether more pages follow
    #[serde(default)]
    pub has_more: bool,
    /// Cursor for the next page
    #[serde(default)]
    pub next_cursor: Option<String>,
}

// =============================================================================
// Request Types
// =============================================================================

/// Request body for Speechify TTS stream endpoint
#[derive(Debug, Clone, Serialize)]
pub struct SpeechifyStreamRequest {
    /// Text or SSML to synthesize
    pub input: String,

    /// Voice identifier
    pub voice_id: String,

    /// Model to use
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Audio output format
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_format: Option<String>,

    /// Language code (ISO-639-1 format, e.g., "en-US")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,

    /// Whether to normalize audio loudness to -14 LUFS
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loudness_normalization: Option<bool>,

    /// Whether to normalize text (numbers, dates to words)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_normalization: Option<bool>,
}

impl SpeechifyStreamRequest {
    /// Create a new stream request from config
    pub fn from_config(config: &SpeechifyTtsConfig, text: &str) -> Self {
        Self {
            input: text.to_string(),
            voice_id: config.voice_id.clone(),
            model: config.model.wire_value().map(str::to_string),
            audio_format: if config.audio_format != SpeechifyAudioFormat::default() {
                Some(config.audio_format.as_str().to_string())
            } else {
                None
            },
            language: config.language.clone(),
            loudness_normalization: if config.loudness_normalization {
                Some(true)
            } else {
                None
            },
            text_normalization: if config.text_normalization {
                Some(true)
            } else {
                None
            },
        }
    }
}

// =============================================================================
// Configuration
// =============================================================================

/// Speechify TTS provider configuration
#[derive(Debug, Clone)]
pub struct SpeechifyTtsConfig {
    /// API key for authentication
    pub api_key: String,

    /// Voice identifier (e.g., "george", "geffen_32"); list them with `GET /v1/voices`
    pub voice_id: String,

    /// Model to use for synthesis
    pub model: SpeechifyModel,

    /// Audio output format
    pub audio_format: SpeechifyAudioFormat,

    /// Language code (ISO-639-1 format, e.g., "en-US")
    /// If not set, auto-detection is used
    pub language: Option<String>,

    /// Whether to normalize audio loudness to -14 LUFS
    /// Note: Increases latency
    pub loudness_normalization: bool,

    /// Whether to normalize text (numbers, dates to words)
    pub text_normalization: bool,

    /// Optional base URL override (scheme+host) for the synth HTTP endpoint; used by mock e2e tests.
    pub endpoint_override: Option<String>,
}

impl Default for SpeechifyTtsConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            voice_id: super::DEFAULT_VOICE_ID.to_string(),
            model: SpeechifyModel::default(),
            audio_format: SpeechifyAudioFormat::default(),
            language: None,
            loudness_normalization: false,
            text_normalization: false,
            endpoint_override: None,
        }
    }
}

impl SpeechifyTtsConfig {
    /// Create a new config builder
    pub fn builder() -> SpeechifyTtsConfigBuilder {
        SpeechifyTtsConfigBuilder::default()
    }

    /// Create configuration from base TTSConfig
    pub fn from_base(config: &TTSConfig) -> TTSResult<Self> {
        // Get API key from config or environment
        let api_key = crate::core::credentials::explicit_api_key(&config.api_key)
            .or_else(|| crate::core::credentials::env_api_key("SPEECHIFY_API_KEY"))
            .unwrap_or_default();

        if api_key.is_empty() {
            return Err(TTSError::InvalidConfiguration(
                "Speechify API key is required. Set api_key in config or SPEECHIFY_API_KEY environment variable.".to_string()
            ));
        }

        // Get voice ID from config
        let voice_id = config
            .voice_id
            .clone()
            .unwrap_or_else(|| super::DEFAULT_VOICE_ID.to_string());

        // Parse model from config
        let model = if !config.model.is_empty() {
            config.model.parse()?
        } else {
            SpeechifyModel::default()
        };

        // Parse audio format from config
        // Note: TTSConfig::Default uses "linear16" which is not valid for Speechify,
        // so we fall back to default Speechify format (wav_48000) for unsupported formats
        let audio_format = if let Some(ref fmt) = config.audio_format {
            if !fmt.is_empty() {
                fmt.parse().unwrap_or_default()
            } else {
                SpeechifyAudioFormat::default()
            }
        } else {
            SpeechifyAudioFormat::default()
        };

        // Language is not in base TTSConfig, so we default to None (auto-detect)
        let language = None;

        Ok(Self {
            api_key,
            voice_id,
            model,
            audio_format,
            language,
            loudness_normalization: false,
            text_normalization: false,
            endpoint_override: None,
        })
    }

    /// Build from the standardized config (W1 keystone). Speechify is a deliberately minimal TTS
    /// surface: the only standardized feature it can express is [`TtsFeatures::language`], which
    /// overrides the synthesis `language` (otherwise auto-detected). The non-standard
    /// `loudness_normalization` and `text_normalization` toggles are read from the `extras`
    /// passthrough. Speechify exposes no speed, pitch, volume, ElevenLabs-style voice settings,
    /// emotion, instructions, SSML, word-timestamp, streaming, seed or sample-rate knobs (sample
    /// rate is fixed by the audio format), so all of those features are skipped.
    ///
    /// [`TtsFeatures::language`]: crate::core::tts::standard::TtsFeatures::language
    pub fn from_standard(std: &crate::core::tts::standard::StandardTTSConfig) -> TTSResult<Self> {
        let f = &std.features;
        let mut cfg = Self::from_base(&std.base)?;

        // Language override (from_base leaves this as None / auto-detect).
        if let Some(language) = f.language.as_ref() {
            cfg.language = Some(language.clone());
        }

        // Provider-specific passthrough.
        if let Some(norm) = std
            .extras
            .0
            .get("loudness_normalization")
            .and_then(|v| v.as_bool())
        {
            cfg.loudness_normalization = norm;
        }
        if let Some(norm) = std
            .extras
            .0
            .get("text_normalization")
            .and_then(|v| v.as_bool())
        {
            cfg.text_normalization = norm;
        }

        cfg.endpoint_override = std.endpoint_override().map(String::from);
        cfg.validate()?;

        Ok(cfg)
    }

    /// Validate text length
    pub fn validate_text(text: &str) -> TTSResult<()> {
        if text.len() > super::MAX_TEXT_LENGTH {
            return Err(TTSError::InvalidConfiguration(format!(
                "Text exceeds maximum length of {} characters (got {}). Split into multiple requests.",
                super::MAX_TEXT_LENGTH,
                text.len()
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

        if self.voice_id.is_empty() {
            return Err(TTSError::InvalidConfiguration(
                "voice_id is required".to_string(),
            ));
        }

        if let Some(endpoint) = self.endpoint_override.as_deref() {
            validate_speechify_tts_endpoint("endpoint_override", endpoint)
                .map_err(TTSError::InvalidConfiguration)?;
        }

        Ok(())
    }
}

// =============================================================================
// Config Builder
// =============================================================================

/// Builder for SpeechifyTtsConfig
#[derive(Debug, Default)]
pub struct SpeechifyTtsConfigBuilder {
    api_key: Option<String>,
    voice_id: Option<String>,
    model: Option<SpeechifyModel>,
    audio_format: Option<SpeechifyAudioFormat>,
    language: Option<String>,
    loudness_normalization: bool,
    text_normalization: bool,
}

impl SpeechifyTtsConfigBuilder {
    /// Set the API key
    pub fn api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    /// Set the voice ID
    pub fn voice_id(mut self, voice_id: impl Into<String>) -> Self {
        self.voice_id = Some(voice_id.into());
        self
    }

    /// Set the model
    pub fn model(mut self, model: SpeechifyModel) -> Self {
        self.model = Some(model);
        self
    }

    /// Set the audio format
    pub fn audio_format(mut self, format: SpeechifyAudioFormat) -> Self {
        self.audio_format = Some(format);
        self
    }

    /// Set the language code
    pub fn language(mut self, language: impl Into<String>) -> Self {
        self.language = Some(language.into());
        self
    }

    /// Enable loudness normalization
    pub fn with_loudness_normalization(mut self) -> Self {
        self.loudness_normalization = true;
        self
    }

    /// Enable text normalization
    pub fn with_text_normalization(mut self) -> Self {
        self.text_normalization = true;
        self
    }

    /// Build the configuration
    pub fn build(self) -> TTSResult<SpeechifyTtsConfig> {
        let config = SpeechifyTtsConfig {
            api_key: self.api_key.unwrap_or_default(),
            voice_id: self
                .voice_id
                .unwrap_or_else(|| super::DEFAULT_VOICE_ID.to_string()),
            model: self.model.unwrap_or_default(),
            audio_format: self.audio_format.unwrap_or_default(),
            language: self.language,
            loudness_normalization: self.loudness_normalization,
            text_normalization: self.text_normalization,
            endpoint_override: None,
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

    // W1 keystone: the one standardized feature Speechify can express (language override) reaches
    // its real `language` field, and the non-standard loudness/text normalization toggles flow
    // through the extras passthrough.
    #[test]
    fn from_standard_maps_language_and_normalization_extras() {
        use crate::core::tts::standard::{ProviderExtras, StandardTTSConfig, TtsFeatures};
        let mut extras = serde_json::Map::new();
        extras.insert("loudness_normalization".into(), serde_json::json!(true));
        extras.insert("text_normalization".into(), serde_json::json!(true));
        let std = StandardTTSConfig {
            base: TTSConfig {
                provider: "speechify".into(),
                api_key: "test-key".into(),
                voice_id: Some("henry".into()),
                ..Default::default()
            },
            features: TtsFeatures {
                language: Some("es-ES".into()),
                // capability gaps: Speechify has no speed/ssml fields, these must be ignored.
                speed: Some(1.5),
                ssml: Some(true),
                ..Default::default()
            },
            extras: ProviderExtras(extras),
        };
        let cfg = SpeechifyTtsConfig::from_standard(&std).unwrap();
        assert_eq!(cfg.language, Some("es-ES".to_string()));
        assert!(cfg.loudness_normalization); // extras passthrough
        assert!(cfg.text_normalization);
    }

    #[test]
    fn test_config_validation_rejects_ssrf_endpoint_override() {
        let _env = crate::core::net::ssrf_env_lock();
        let mut config = SpeechifyTtsConfig {
            api_key: "test-key".to_string(),
            voice_id: "george".to_string(),
            endpoint_override: Some("https://speechify-proxy.example.com".to_string()),
            ..Default::default()
        };
        assert!(config.validate().is_ok());

        config.endpoint_override = Some("http://127.0.0.1:9000".to_string());
        let err = config
            .validate()
            .expect_err("loopback endpoint_override must be rejected");
        assert!(format!("{err:?}").contains("SSRF protection"));

        config.endpoint_override = Some("file:///tmp/socket".to_string());
        let err = config
            .validate()
            .expect_err("file endpoint_override must be rejected");
        assert!(format!("{err:?}").contains("URL scheme"));

        config.endpoint_override = Some("ws://speechify-proxy.example.com".to_string());
        let err = config
            .validate()
            .expect_err("WebSocket endpoint_override must be rejected");
        assert!(format!("{err:?}").contains("URL scheme"));

        let std = crate::core::tts::standard::StandardTTSConfig::from_base(TTSConfig {
            provider: "speechify".to_string(),
            api_key: "test-key".to_string(),
            voice_id: Some("george".to_string()),
            ..Default::default()
        })
        .with_endpoint_override("file:///tmp/socket");
        assert!(SpeechifyTtsConfig::from_standard(&std).is_err());
    }

    #[test]
    fn test_model_enum() {
        assert_eq!(SpeechifyModel::SimbaEnglish.as_str(), "simba-english");
        assert_eq!(SpeechifyModel::SimbaTurbo.as_str(), "simba-turbo");
        assert_eq!(
            SpeechifyModel::SimbaMultilingual.as_str(),
            "simba-multilingual"
        );
        assert_eq!(SpeechifyModel::SimbaBase.as_str(), "simba-base");
    }

    #[test]
    fn test_model_from_str() {
        assert_eq!(
            "simba-english".parse::<SpeechifyModel>().unwrap(),
            SpeechifyModel::SimbaEnglish
        );
        assert_eq!(
            "turbo".parse::<SpeechifyModel>().unwrap(),
            SpeechifyModel::SimbaTurbo
        );
        assert_eq!(
            "multilingual".parse::<SpeechifyModel>().unwrap(),
            SpeechifyModel::SimbaMultilingual
        );
        assert_eq!(
            "simba-base".parse::<SpeechifyModel>().unwrap(),
            SpeechifyModel::SimbaBase
        );

        // Test invalid model
        assert!("invalid".parse::<SpeechifyModel>().is_err());
    }

    #[test]
    fn test_model_features() {
        assert!(SpeechifyModel::Simba35.is_multilingual());
        assert!(!SpeechifyModel::Simba32.is_multilingual());
        assert!(SpeechifyModel::Simba30.is_multilingual());
        assert!(!SpeechifyModel::SimbaEnglish.is_multilingual());
        assert!(!SpeechifyModel::SimbaTurbo.is_multilingual());
        assert!(SpeechifyModel::SimbaMultilingual.is_multilingual());

        assert!(!SpeechifyModel::SimbaEnglish.supports_emotion());
        assert!(SpeechifyModel::SimbaTurbo.supports_emotion());
        assert!(!SpeechifyModel::SimbaMultilingual.supports_emotion());
    }

    #[test]
    fn test_audio_format_enum() {
        assert_eq!(SpeechifyAudioFormat::Wav48000.as_str(), "wav_48000");
        assert_eq!(SpeechifyAudioFormat::Mp3_24000.as_str(), "mp3_24000");
        assert_eq!(SpeechifyAudioFormat::Ogg24000.as_str(), "ogg_24000");
        assert_eq!(SpeechifyAudioFormat::Aac24000.as_str(), "aac_24000");
    }

    #[test]
    fn test_audio_format_from_str() {
        assert_eq!(
            "wav_48000".parse::<SpeechifyAudioFormat>().unwrap(),
            SpeechifyAudioFormat::Wav48000
        );
        assert_eq!(
            "mp3".parse::<SpeechifyAudioFormat>().unwrap(),
            SpeechifyAudioFormat::Mp3_24000
        );
        assert_eq!(
            "ogg".parse::<SpeechifyAudioFormat>().unwrap(),
            SpeechifyAudioFormat::Ogg24000
        );
        assert_eq!(
            "aac".parse::<SpeechifyAudioFormat>().unwrap(),
            SpeechifyAudioFormat::Aac24000
        );

        // Test invalid format
        assert!("invalid".parse::<SpeechifyAudioFormat>().is_err());
    }

    #[test]
    fn test_audio_format_sample_rate() {
        assert_eq!(SpeechifyAudioFormat::Wav48000.sample_rate(), 48000);
        assert_eq!(SpeechifyAudioFormat::Mp3_24000.sample_rate(), 24000);
        assert_eq!(SpeechifyAudioFormat::Ogg24000.sample_rate(), 24000);
        assert_eq!(SpeechifyAudioFormat::Aac24000.sample_rate(), 24000);
    }

    #[test]
    fn test_audio_format_mime_type() {
        assert_eq!(SpeechifyAudioFormat::Wav48000.mime_type(), "audio/wav");
        assert_eq!(SpeechifyAudioFormat::Mp3_24000.mime_type(), "audio/mpeg");
        assert_eq!(SpeechifyAudioFormat::Ogg24000.mime_type(), "audio/ogg");
        assert_eq!(SpeechifyAudioFormat::Aac24000.mime_type(), "audio/aac");
    }

    #[test]
    fn test_config_defaults() {
        let config = SpeechifyTtsConfig::default();
        assert_eq!(config.voice_id, "george");
        assert_eq!(config.model, SpeechifyModel::Simba30);
        assert_eq!(config.audio_format, SpeechifyAudioFormat::Wav48000);
        assert!(!config.loudness_normalization);
        assert!(!config.text_normalization);
    }

    #[test]
    fn test_config_from_base() {
        let base_config = TTSConfig {
            api_key: "test-api-key".to_string(),
            voice_id: Some("henry".to_string()),
            model: "simba-turbo".to_string(),
            audio_format: Some("mp3".to_string()),
            ..Default::default()
        };

        let config = SpeechifyTtsConfig::from_base(&base_config).unwrap();
        assert_eq!(config.api_key, "test-api-key");
        assert_eq!(config.voice_id, "henry");
        assert_eq!(config.model, SpeechifyModel::SimbaTurbo);
        assert_eq!(config.audio_format, SpeechifyAudioFormat::Mp3_24000);
        // Language is not in base TTSConfig, so it defaults to None
        assert_eq!(config.language, None);
    }

    #[test]
    fn test_config_from_base_requires_api_key() {
        let base_config = TTSConfig {
            voice_id: Some("george".to_string()),
            ..Default::default()
        };

        let result = SpeechifyTtsConfig::from_base(&base_config);
        assert!(result.is_err());

        if let Err(TTSError::InvalidConfiguration(msg)) = result {
            assert!(msg.contains("API key") || msg.contains("SPEECHIFY_API_KEY"));
        } else {
            panic!("Expected InvalidConfiguration error");
        }
    }

    #[test]
    fn test_config_validation() {
        let config = SpeechifyTtsConfig {
            api_key: "test-key".to_string(),
            voice_id: "george".to_string(),
            ..Default::default()
        };
        assert!(config.validate().is_ok());

        // Empty API key
        let config = SpeechifyTtsConfig {
            api_key: "".to_string(),
            voice_id: "george".to_string(),
            ..Default::default()
        };
        assert!(config.validate().is_err());

        // Empty voice ID
        let config = SpeechifyTtsConfig {
            api_key: "test-key".to_string(),
            voice_id: "".to_string(),
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_builder() {
        let config = SpeechifyTtsConfig::builder()
            .api_key("test-key")
            .voice_id("kristy")
            .model(SpeechifyModel::SimbaMultilingual)
            .audio_format(SpeechifyAudioFormat::Ogg24000)
            .language("es-ES")
            .with_loudness_normalization()
            .with_text_normalization()
            .build()
            .unwrap();

        assert_eq!(config.api_key, "test-key");
        assert_eq!(config.voice_id, "kristy");
        assert_eq!(config.model, SpeechifyModel::SimbaMultilingual);
        assert_eq!(config.audio_format, SpeechifyAudioFormat::Ogg24000);
        assert_eq!(config.language, Some("es-ES".to_string()));
        assert!(config.loudness_normalization);
        assert!(config.text_normalization);
    }

    #[test]
    fn test_text_validation() {
        // Valid text
        assert!(SpeechifyTtsConfig::validate_text("Hello world").is_ok());

        // Text at limit
        let limit_text = "a".repeat(super::super::MAX_TEXT_LENGTH);
        assert!(SpeechifyTtsConfig::validate_text(&limit_text).is_ok());

        // Text over limit
        let over_limit_text = "a".repeat(super::super::MAX_TEXT_LENGTH + 1);
        let result = SpeechifyTtsConfig::validate_text(&over_limit_text);
        assert!(result.is_err());
        if let Err(TTSError::InvalidConfiguration(msg)) = result {
            assert!(msg.contains("20000"));
        }
    }

    #[test]
    fn test_stream_request_from_config() {
        let config = SpeechifyTtsConfig {
            api_key: "test-key".to_string(),
            voice_id: "george".to_string(),
            model: SpeechifyModel::SimbaEnglish,
            audio_format: SpeechifyAudioFormat::Wav48000,
            language: None,
            loudness_normalization: false,
            text_normalization: false,
            endpoint_override: None,
        };

        let request = SpeechifyStreamRequest::from_config(&config, "Hello world");

        assert_eq!(request.input, "Hello world");
        assert_eq!(request.voice_id, "george");
        assert!(request.model.is_none()); // Default model shouldn't be serialized
        assert!(request.audio_format.is_none()); // Default format shouldn't be serialized
        assert!(request.language.is_none());
        assert!(request.loudness_normalization.is_none());
        assert!(request.text_normalization.is_none());
    }

    #[test]
    fn test_stream_request_with_options() {
        let config = SpeechifyTtsConfig {
            api_key: "test-key".to_string(),
            voice_id: "kristy".to_string(),
            model: SpeechifyModel::SimbaMultilingual,
            audio_format: SpeechifyAudioFormat::Mp3_24000,
            language: Some("fr-FR".to_string()),
            loudness_normalization: true,
            text_normalization: true,
            endpoint_override: None,
        };

        let request = SpeechifyStreamRequest::from_config(&config, "Bonjour");

        assert_eq!(request.input, "Bonjour");
        assert_eq!(request.voice_id, "kristy");
        assert_eq!(request.model, Some("simba-multilingual".to_string()));
        assert_eq!(request.audio_format, Some("mp3_24000".to_string()));
        assert_eq!(request.language, Some("fr-FR".to_string()));
        assert_eq!(request.loudness_normalization, Some(true));
        assert_eq!(request.text_normalization, Some(true));
    }

    #[test]
    fn test_stream_request_serialization() {
        let config = SpeechifyTtsConfig {
            api_key: "test-key".to_string(),
            voice_id: "george".to_string(),
            model: SpeechifyModel::SimbaTurbo,
            audio_format: SpeechifyAudioFormat::Mp3_24000,
            language: Some("en-US".to_string()),
            loudness_normalization: true,
            text_normalization: false,
            endpoint_override: None,
        };

        let request = SpeechifyStreamRequest::from_config(&config, "Test text");
        let json = serde_json::to_string(&request).unwrap();

        assert!(json.contains("\"input\":\"Test text\""));
        assert!(json.contains("\"voice_id\":\"george\""));
        assert!(json.contains("\"model\":\"simba-turbo\""));
        assert!(json.contains("\"audio_format\":\"mp3_24000\""));
        assert!(json.contains("\"language\":\"en-US\""));
        assert!(json.contains("\"loudness_normalization\":true"));
        assert!(!json.contains("text_normalization")); // false shouldn't be serialized
    }

    /// Speechify's documented `GetVoice` (the old struct read `name`, so parsing failed).
    #[test]
    fn test_voice_deserialization() {
        let json = r#"{
            "display_name": "Geffen",
            "gender": "male",
            "locale": "en-US",
            "id": "geffen_32",
            "models": [{"languages": [{"locale": "en-US", "preview_audio": null}], "name": "simba-3.5"}],
            "type": "personal",
            "avatar_image": null,
            "preview_audio": "https://example/preview.mp3",
            "tags": ["calm"]
        }"#;

        let voice: SpeechifyVoice = serde_json::from_str(json).unwrap();
        assert_eq!(voice.id, "geffen_32");
        assert_eq!(voice.display_name, "Geffen");
        assert_eq!(voice.gender, Some("male".to_string()));
        assert_eq!(voice.locale, Some("en-US".to_string()));
        assert_eq!(voice.models[0].name, "simba-3.5");
        assert!(voice.is_cloned());
    }

    /// The current model ids parse; the retired ones still do, for workspaces pinned to them.
    #[test]
    fn current_and_retired_model_ids_parse() {
        assert_eq!(
            "simba-3.5".parse::<SpeechifyModel>().unwrap(),
            SpeechifyModel::Simba35
        );
        assert_eq!(
            "simba-3.2".parse::<SpeechifyModel>().unwrap(),
            SpeechifyModel::Simba32
        );
        assert_eq!(
            "simba-3.0".parse::<SpeechifyModel>().unwrap(),
            SpeechifyModel::Simba30
        );
        assert_eq!(
            "simba-english".parse::<SpeechifyModel>().unwrap().as_str(),
            "simba-english"
        );
        // The default is the API's own default, so leaving it off the wire changes nothing.
        assert_eq!(SpeechifyModel::default().as_str(), "simba-3.0");
        // Neither the default nor the old default goes on the wire (unchanged from before 3.x).
        assert_eq!(SpeechifyModel::default().wire_value(), None);
        assert_eq!(SpeechifyModel::SimbaEnglish.wire_value(), None);
        assert_eq!(SpeechifyModel::Simba35.wire_value(), Some("simba-3.5"));
    }

    #[test]
    fn test_model_serialization() {
        let model = SpeechifyModel::SimbaTurbo;
        let json = serde_json::to_string(&model).unwrap();
        assert_eq!(json, "\"simba-turbo\"");

        let deserialized: SpeechifyModel = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, SpeechifyModel::SimbaTurbo);
    }

    #[test]
    fn test_audio_format_serialization() {
        let format = SpeechifyAudioFormat::Ogg24000;
        let json = serde_json::to_string(&format).unwrap();
        assert_eq!(json, "\"ogg_24000\"");

        let deserialized: SpeechifyAudioFormat = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, SpeechifyAudioFormat::Ogg24000);
    }
}
