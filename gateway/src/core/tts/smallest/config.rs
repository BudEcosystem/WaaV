//! Smallest.ai TTS Configuration Types
//!
//! This module defines configuration structures for the Smallest.ai Waves TTS API.

use serde::{Deserialize, Serialize};
use std::fmt;

use super::{
    DEFAULT_CONSISTENCY, DEFAULT_ENHANCEMENT, DEFAULT_SAMPLE_RATE, DEFAULT_SIMILARITY,
    DEFAULT_SPEED, DEFAULT_VOICE_ID, MAX_CONSISTENCY, MAX_ENHANCEMENT, MAX_SIMILARITY, MAX_SPEED,
    MIN_CONSISTENCY, MIN_ENHANCEMENT, MIN_SIMILARITY, MIN_SPEED,
};
use crate::core::tts::base::{TTSConfig, TTSError};

fn validate_smallest_tts_endpoint(source: &str, endpoint: &str) -> Result<(), String> {
    let endpoint = endpoint.trim();
    if endpoint.is_empty() {
        return Ok(());
    }
    crate::core::net::validate_url_for_ssrf(endpoint, crate::core::net::HTTP_URL_SCHEMES)
        .map_err(|msg| format!("{source} rejected (SSRF protection): {msg}"))
}

// =============================================================================
// SmallestModel
// =============================================================================

/// Smallest.ai TTS models: the two Lightning v3.1 voice pools.
///
/// Smallest retired every earlier model (`lightning`, `lightning-large`, `lightning-v2`) — their
/// endpoints answer `410 MODEL_DEPRECATED`, "Please migrate to lightning-v3.1" — and `thunder`
/// is gone. Those names still parse, to [`Self::LightningV31`], so a config written for them keeps
/// working instead of failing every request; [`Self::parse_with_migration`] says when it happens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum SmallestModel {
    /// Lightning v3.1, the standard pool: the larger voice catalog, and voice cloning.
    #[serde(
        rename = "lightning_v3.1",
        alias = "lightning-v3.1",
        alias = "lightning",
        alias = "lightning-large",
        alias = "lightning_large",
        alias = "lightning-v2",
        alias = "lightning_v2",
        alias = "thunder"
    )]
    #[default]
    LightningV31,
    /// Lightning v3.1 Pro: the premium pool, with its own voices (a standard-pool voice id is not
    /// valid here, nor the reverse).
    #[serde(rename = "lightning_v3.1_pro", alias = "lightning-v3.1-pro")]
    LightningV31Pro,
}

/// Model names Smallest has retired, all served by Lightning v3.1 now.
const RETIRED_MODELS: &[&str] = &[
    "lightning",
    "lightning-large",
    "lightning_large",
    "lightninglarge",
    "lightning-v2",
    "lightning_v2",
    "lightningv2",
    "thunder",
];

impl SmallestModel {
    /// The `model` value the synthesis request body carries.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::LightningV31 => "lightning_v3.1",
            Self::LightningV31Pro => "lightning_v3.1_pro",
        }
    }

    /// The model's segment in the voice-list path (`/waves/v1/<segment>/get_voices`).
    pub fn path_segment(&self) -> &'static str {
        match self {
            Self::LightningV31 => "lightning-v3.1",
            Self::LightningV31Pro => "lightning-v3.1-pro",
        }
    }

    /// Parse a model name, reporting whether it was a retired one mapped onto Lightning v3.1.
    pub fn parse_with_migration(s: &str) -> Result<(Self, bool), TTSError> {
        let lowered = s.trim().to_lowercase();
        if RETIRED_MODELS.contains(&lowered.as_str()) {
            return Ok((Self::LightningV31, true));
        }
        match lowered.as_str() {
            "lightning_v3.1" | "lightning-v3.1" | "lightning-v3_1" | "lightning_v3_1" => {
                Ok((Self::LightningV31, false))
            }
            "lightning_v3.1_pro" | "lightning-v3.1-pro" | "lightning_v3_1_pro" => {
                Ok((Self::LightningV31Pro, false))
            }
            _ => Err(TTSError::InvalidConfiguration(format!(
                "Unknown Smallest.ai model: {s}. Valid models: lightning_v3.1, lightning_v3.1_pro"
            ))),
        }
    }

    /// Returns whether this model supports voice cloning (the standard pool only).
    pub fn supports_voice_cloning(&self) -> bool {
        matches!(self, Self::LightningV31)
    }

    /// Returns whether this model supports WebSocket streaming (`/waves/v1/tts/live`).
    pub fn supports_websocket(&self) -> bool {
        true
    }

    /// Returns the maximum speed multiplier for this model.
    pub fn max_speed(&self) -> f32 {
        MAX_SPEED
    }

    /// Returns the approximate time to first byte for this model, in ms.
    pub fn latency_ms(&self) -> u32 {
        200
    }
}

impl fmt::Display for SmallestModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl std::str::FromStr for SmallestModel {
    type Err = TTSError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse_with_migration(s).map(|(model, _)| model)
    }
}

// =============================================================================
// SmallestOutputFormat
// =============================================================================

/// Output audio formats supported by Smallest.ai.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SmallestOutputFormat {
    /// Raw PCM audio (default).
    #[default]
    Pcm,

    /// WAV format (PCM with header).
    Wav,

    /// MP3 compressed format.
    Mp3,

    /// µ-law encoded audio (the API's `ulaw`).
    #[serde(rename = "ulaw", alias = "mulaw")]
    Mulaw,
}

impl SmallestOutputFormat {
    /// Returns the API string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pcm => "pcm",
            Self::Wav => "wav",
            Self::Mp3 => "mp3",
            Self::Mulaw => "ulaw",
        }
    }

    /// Returns the MIME content type.
    pub fn content_type(&self) -> &'static str {
        match self {
            Self::Pcm => "audio/pcm",
            Self::Wav => "audio/wav",
            Self::Mp3 => "audio/mpeg",
            Self::Mulaw => "audio/basic",
        }
    }

    /// Returns file extension for this format.
    pub fn extension(&self) -> &'static str {
        match self {
            Self::Pcm => "pcm",
            Self::Wav => "wav",
            Self::Mp3 => "mp3",
            Self::Mulaw => "mulaw",
        }
    }
}

impl fmt::Display for SmallestOutputFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl std::str::FromStr for SmallestOutputFormat {
    type Err = TTSError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "pcm" | "linear16" | "raw" => Ok(Self::Pcm),
            "wav" | "wave" => Ok(Self::Wav),
            "mp3" | "mpeg" => Ok(Self::Mp3),
            "mulaw" | "ulaw" | "mu-law" => Ok(Self::Mulaw),
            _ => Err(TTSError::InvalidConfiguration(format!(
                "Unknown audio format: {}. Valid formats: pcm, wav, mp3, ulaw",
                s
            ))),
        }
    }
}

// =============================================================================
// SmallestLanguage
// =============================================================================

/// Languages supported by Smallest.ai TTS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SmallestLanguage {
    /// English (default).
    #[default]
    #[serde(alias = "en")]
    English,

    /// Hindi.
    #[serde(alias = "hi")]
    Hindi,

    /// Marathi.
    #[serde(alias = "mr")]
    Marathi,

    /// Kannada.
    #[serde(alias = "kn")]
    Kannada,

    /// Tamil.
    #[serde(alias = "ta")]
    Tamil,

    /// Bengali.
    #[serde(alias = "bn")]
    Bengali,

    /// Gujarati.
    #[serde(alias = "gu")]
    Gujarati,

    /// German.
    #[serde(alias = "de")]
    German,

    /// French.
    #[serde(alias = "fr")]
    French,

    /// Spanish.
    #[serde(alias = "es")]
    Spanish,

    /// Italian.
    #[serde(alias = "it")]
    Italian,

    /// Polish.
    #[serde(alias = "pl")]
    Polish,

    /// Dutch.
    #[serde(alias = "nl")]
    Dutch,

    /// Russian.
    #[serde(alias = "ru")]
    Russian,

    /// Arabic.
    #[serde(alias = "ar")]
    Arabic,

    /// Hebrew.
    #[serde(alias = "he")]
    Hebrew,
}

impl SmallestLanguage {
    /// Returns the ISO 639-1 language code.
    pub fn as_code(&self) -> &'static str {
        match self {
            Self::English => "en",
            Self::Hindi => "hi",
            Self::Marathi => "mr",
            Self::Kannada => "kn",
            Self::Tamil => "ta",
            Self::Bengali => "bn",
            Self::Gujarati => "gu",
            Self::German => "de",
            Self::French => "fr",
            Self::Spanish => "es",
            Self::Italian => "it",
            Self::Polish => "pl",
            Self::Dutch => "nl",
            Self::Russian => "ru",
            Self::Arabic => "ar",
            Self::Hebrew => "he",
        }
    }

    /// Returns the display name.
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::English => "English",
            Self::Hindi => "Hindi",
            Self::Marathi => "Marathi",
            Self::Kannada => "Kannada",
            Self::Tamil => "Tamil",
            Self::Bengali => "Bengali",
            Self::Gujarati => "Gujarati",
            Self::German => "German",
            Self::French => "French",
            Self::Spanish => "Spanish",
            Self::Italian => "Italian",
            Self::Polish => "Polish",
            Self::Dutch => "Dutch",
            Self::Russian => "Russian",
            Self::Arabic => "Arabic",
            Self::Hebrew => "Hebrew",
        }
    }
}

impl fmt::Display for SmallestLanguage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_code())
    }
}

impl std::str::FromStr for SmallestLanguage {
    type Err = TTSError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "en" | "english" => Ok(Self::English),
            "hi" | "hindi" => Ok(Self::Hindi),
            "mr" | "marathi" => Ok(Self::Marathi),
            "kn" | "kannada" => Ok(Self::Kannada),
            "ta" | "tamil" => Ok(Self::Tamil),
            "bn" | "bengali" => Ok(Self::Bengali),
            "gu" | "gujarati" => Ok(Self::Gujarati),
            "de" | "german" => Ok(Self::German),
            "fr" | "french" => Ok(Self::French),
            "es" | "spanish" => Ok(Self::Spanish),
            "it" | "italian" => Ok(Self::Italian),
            "pl" | "polish" => Ok(Self::Polish),
            "nl" | "dutch" => Ok(Self::Dutch),
            "ru" | "russian" => Ok(Self::Russian),
            "ar" | "arabic" => Ok(Self::Arabic),
            "he" | "hebrew" => Ok(Self::Hebrew),
            _ => Err(TTSError::InvalidConfiguration(format!(
                "Unknown language: {}",
                s
            ))),
        }
    }
}

// =============================================================================
// SmallestTtsConfig
// =============================================================================

/// Configuration for Smallest.ai TTS provider.
#[derive(Debug, Clone)]
pub struct SmallestTtsConfig {
    /// API key for authentication.
    pub api_key: String,

    /// Voice ID to use.
    pub voice_id: String,

    /// Model to use.
    pub model: SmallestModel,

    /// Language for number pronunciation.
    pub language: SmallestLanguage,

    /// Output audio format.
    pub format: SmallestOutputFormat,

    /// Sample rate in Hz.
    pub sample_rate: u32,

    /// Speech speed multiplier.
    pub speed: f32,

    /// Voice consistency (0-1; streaming body only, the retired lightning-large knob).
    pub consistency: f32,

    /// Voice similarity to reference (0-1; streaming body only, the retired lightning-large knob).
    pub similarity: f32,

    /// Audio enhancement level (0-2; streaming body only, the retired lightning-large knob).
    pub enhancement: u8,

    /// Max wait (ms) before the streaming WebSocket flushes its output buffer
    /// (`max_buffer_flush_ms` in the `get_speech/stream` request; 0-1000ms). A latency/chunking
    /// knob, not an audio-content knob, so it stays out of the synthesis cache key. `None` lets
    /// the server default apply.
    pub max_buffer_flush_ms: Option<u32>,

    /// Connection timeout in seconds.
    pub connection_timeout: u64,

    /// Request timeout in seconds.
    pub request_timeout: u64,

    /// Optional base URL override for the synth REST endpoint (test/mock redirection).
    pub endpoint_override: Option<String>,
}

impl SmallestTtsConfig {
    /// Creates a new configuration from a base TTSConfig.
    pub fn from_base(config: TTSConfig) -> Result<Self, TTSError> {
        // Validate API key
        if config.api_key.is_empty() {
            return Err(TTSError::InvalidConfiguration(
                "Smallest.ai API key is required".to_string(),
            ));
        }

        // Parse model; a retired one is served by Lightning v3.1 (see `SmallestModel`).
        let model = if !config.model.is_empty() {
            let (model, migrated) = SmallestModel::parse_with_migration(&config.model)?;
            if migrated {
                tracing::warn!(
                    requested = %config.model,
                    using = model.as_str(),
                    "Smallest retired this model (its endpoint answers 410); using lightning_v3.1"
                );
            }
            model
        } else {
            SmallestModel::default()
        };

        // Get voice ID
        let voice_id = config
            .voice_id
            .unwrap_or_else(|| DEFAULT_VOICE_ID.to_string());

        // Parse output format
        let format = config
            .audio_format
            .as_ref()
            .map(|f| f.parse())
            .transpose()?
            .unwrap_or_default();

        // Get sample rate
        let sample_rate = config.sample_rate.unwrap_or(DEFAULT_SAMPLE_RATE);

        // Get speed from speaking_rate
        let speed = config
            .speaking_rate
            .map(|r| r.clamp(MIN_SPEED, model.max_speed()))
            .unwrap_or(DEFAULT_SPEED);

        // Timeouts
        let connection_timeout = config.connection_timeout.unwrap_or(30);
        let request_timeout = config.request_timeout.unwrap_or(60);

        Ok(Self {
            api_key: config.api_key,
            voice_id,
            model,
            language: SmallestLanguage::default(),
            format,
            sample_rate,
            speed,
            consistency: DEFAULT_CONSISTENCY,
            similarity: DEFAULT_SIMILARITY,
            enhancement: DEFAULT_ENHANCEMENT,
            max_buffer_flush_ms: None,
            connection_timeout,
            request_timeout,
            endpoint_override: None,
        })
    }

    /// Build from the standardized TTS config (W1 keystone).
    ///
    /// Maps `speed` -> `speed` (reusing `from_base`'s `MIN_SPEED..=model.max_speed()` clamp so 1.0
    /// stays normal), `language` -> the
    /// `SmallestLanguage` enum (rejecting unknown non-empty values), and `sample_rate` ->
    /// `sample_rate` (snapped to the nearest supported rate). Smallest's non-standard
    /// `enhancement` level and the
    /// streaming `max_buffer_flush_ms` knob are read from the `extras` passthrough.
    /// Features without a Smallest field (stability, similarity_boost — the retired
    /// lightning-large's knobs, which Lightning v3.1 has no parameter for — pitch, volume, style,
    /// use_speaker_boost, emotion, instructions, ssml, word_timestamps, streaming, seed) are
    /// skipped, so the capability matrix does not claim them.
    pub fn from_standard(
        std: &crate::core::tts::standard::StandardTTSConfig,
    ) -> Result<Self, TTSError> {
        let f = &std.features;
        let mut cfg = Self::from_base(std.base.clone())?;

        if let Some(speed) = f.speed {
            // Reuse from_base's clamp: a 1.0-is-normal multiplier bounded by the model's max speed.
            cfg.speed = speed.clamp(MIN_SPEED, cfg.model.max_speed());
        }
        if let Some(language) = f
            .language
            .as_deref()
            .map(str::trim)
            .filter(|l| !l.is_empty())
        {
            cfg.language = language.parse()?;
        }
        if let Some(rate) = f.sample_rate {
            cfg.sample_rate = super::nearest_sample_rate(rate);
        }

        // Provider-specific passthrough.
        if let Some(enhancement) = std.extras.0.get("enhancement").and_then(|v| v.as_u64()) {
            cfg.enhancement = (enhancement as u8).clamp(MIN_ENHANCEMENT, MAX_ENHANCEMENT);
        }
        // streaming buffer-flush latency knob (0-1000ms), reaches the WS request body.
        if let Some(ms) = std
            .extras
            .0
            .get("max_buffer_flush_ms")
            .and_then(|v| v.as_u64())
        {
            cfg.max_buffer_flush_ms = Some((ms.min(1000)) as u32);
        }

        cfg.endpoint_override = std.endpoint_override().map(String::from);
        cfg.validate()?;

        Ok(cfg)
    }

    /// Validates the configuration.
    pub fn validate(&self) -> Result<(), TTSError> {
        if self.api_key.is_empty() {
            return Err(TTSError::InvalidConfiguration(
                "API key is required".to_string(),
            ));
        }

        if self.voice_id.is_empty() {
            return Err(TTSError::InvalidConfiguration(
                "Voice ID is required".to_string(),
            ));
        }

        let max_speed = self.model.max_speed();
        if self.speed < MIN_SPEED || self.speed > max_speed {
            return Err(TTSError::InvalidConfiguration(format!(
                "Speed must be between {} and {} for {} model",
                MIN_SPEED, max_speed, self.model
            )));
        }

        if self.consistency < MIN_CONSISTENCY || self.consistency > MAX_CONSISTENCY {
            return Err(TTSError::InvalidConfiguration(format!(
                "Consistency must be between {} and {}",
                MIN_CONSISTENCY, MAX_CONSISTENCY
            )));
        }

        if self.similarity < MIN_SIMILARITY || self.similarity > MAX_SIMILARITY {
            return Err(TTSError::InvalidConfiguration(format!(
                "Similarity must be between {} and {}",
                MIN_SIMILARITY, MAX_SIMILARITY
            )));
        }

        // `enhancement` is a `u8` and `MIN_ENHANCEMENT == 0`, so the lower bound is
        // guaranteed by the type; only the upper bound needs validating.
        if self.enhancement > MAX_ENHANCEMENT {
            return Err(TTSError::InvalidConfiguration(format!(
                "Enhancement must be between {} and {}",
                MIN_ENHANCEMENT, MAX_ENHANCEMENT
            )));
        }

        if let Some(endpoint) = self.endpoint_override.as_deref() {
            validate_smallest_tts_endpoint("endpoint_override", endpoint)
                .map_err(TTSError::InvalidConfiguration)?;
        }

        Ok(())
    }

    /// Sets the voice ID.
    pub fn with_voice_id(mut self, voice_id: impl Into<String>) -> Self {
        self.voice_id = voice_id.into();
        self
    }

    /// Sets the model.
    pub fn with_model(mut self, model: SmallestModel) -> Self {
        self.model = model;
        self
    }

    /// Sets the language.
    pub fn with_language(mut self, language: SmallestLanguage) -> Self {
        self.language = language;
        self
    }

    /// Sets the output format.
    pub fn with_format(mut self, format: SmallestOutputFormat) -> Self {
        self.format = format;
        self
    }

    /// Sets the sample rate.
    pub fn with_sample_rate(mut self, sample_rate: u32) -> Self {
        self.sample_rate = super::nearest_sample_rate(sample_rate);
        self
    }

    /// Sets the speech speed.
    pub fn with_speed(mut self, speed: f32) -> Self {
        let max_speed = self.model.max_speed();
        self.speed = speed.clamp(MIN_SPEED, max_speed);
        self
    }

    /// Sets the consistency parameter (the retired lightning-large's knob; streaming body only).
    pub fn with_consistency(mut self, consistency: f32) -> Self {
        self.consistency = consistency.clamp(MIN_CONSISTENCY, MAX_CONSISTENCY);
        self
    }

    /// Sets the similarity parameter (the retired lightning-large's knob; streaming body only).
    pub fn with_similarity(mut self, similarity: f32) -> Self {
        self.similarity = similarity.clamp(MIN_SIMILARITY, MAX_SIMILARITY);
        self
    }

    /// Sets the enhancement level (the retired lightning-large's knob; streaming body only).
    pub fn with_enhancement(mut self, enhancement: u8) -> Self {
        self.enhancement = enhancement.clamp(MIN_ENHANCEMENT, MAX_ENHANCEMENT);
        self
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_config() -> TTSConfig {
        TTSConfig {
            provider: "smallest".to_string(),
            api_key: "test-api-key".to_string(),
            voice_id: Some("olivia".to_string()),
            model: "lightning_v3.1".to_string(),
            speaking_rate: Some(1.0),
            audio_format: Some("wav".to_string()),
            sample_rate: Some(24000),
            connection_timeout: Some(30),
            request_timeout: Some(60),
            pronunciations: Vec::new(),
            request_pool_size: Some(4),
            emotion_config: None,
            api_base: None,
        }
    }

    // W1 keystone (TTS): the standardized features Smallest can express — the speed multiplier,
    // the lightning-large voice knobs (stability -> consistency, similarity_boost -> similarity),
    // the synthesis language, and the output sample rate — reach the config fields, and the open
    // extras passthrough carries the provider-specific enhancement level.
    #[test]
    fn from_standard_maps_speed_voice_knobs_language_and_extras() {
        use crate::core::tts::standard::{ProviderExtras, StandardTTSConfig, TtsFeatures};
        let mut extras = serde_json::Map::new();
        extras.insert("enhancement".into(), serde_json::json!(2));
        let std = StandardTTSConfig {
            base: create_test_config(),
            features: TtsFeatures {
                speed: Some(1.5),
                stability: Some(0.8),
                similarity_boost: Some(0.7),
                language: Some("hi".into()),
                sample_rate: Some(16000),
                ssml: Some(true), // capability gap: Smallest has no SSML, must be ignored
                ..Default::default()
            },
            extras: ProviderExtras(extras),
        };
        let cfg = SmallestTtsConfig::from_standard(&std).unwrap();
        // Lightning v3.1's max speed is 2.0, so 1.5 passes through the clamp unchanged.
        assert!((cfg.speed - 1.5).abs() < 0.001);
        // stability / similarity_boost have no Lightning v3.1 parameter: left at the defaults.
        assert!((cfg.consistency - DEFAULT_CONSISTENCY).abs() < 0.001);
        assert!((cfg.similarity - DEFAULT_SIMILARITY).abs() < 0.001);
        assert_eq!(cfg.language, SmallestLanguage::Hindi);
        assert_eq!(cfg.sample_rate, 16000);
        assert_eq!(cfg.enhancement, 2); // from extras passthrough
    }

    // The streaming `max_buffer_flush_ms` knob arrives via the extras passthrough and
    // reaches the config field (clamped to the 0-1000ms documented range).
    #[test]
    fn from_standard_maps_max_buffer_flush_ms_from_extras() {
        use crate::core::tts::standard::{ProviderExtras, StandardTTSConfig, TtsFeatures};
        let mut extras = serde_json::Map::new();
        extras.insert("max_buffer_flush_ms".into(), serde_json::json!(250));
        let std = StandardTTSConfig {
            base: create_test_config(),
            features: TtsFeatures::default(),
            extras: ProviderExtras(extras),
        };
        let cfg = SmallestTtsConfig::from_standard(&std).unwrap();
        assert_eq!(cfg.max_buffer_flush_ms, Some(250));

        // Over-range values are clamped to the documented 1000ms ceiling.
        let mut extras = serde_json::Map::new();
        extras.insert("max_buffer_flush_ms".into(), serde_json::json!(5000));
        let std = StandardTTSConfig {
            base: create_test_config(),
            features: TtsFeatures::default(),
            extras: ProviderExtras(extras),
        };
        let cfg = SmallestTtsConfig::from_standard(&std).unwrap();
        assert_eq!(cfg.max_buffer_flush_ms, Some(1000));
    }

    #[test]
    fn from_standard_rejects_unknown_language() {
        use crate::core::tts::standard::{StandardTTSConfig, TtsFeatures};

        let std = StandardTTSConfig {
            base: create_test_config(),
            features: TtsFeatures {
                language: Some("klingon".into()),
                ..Default::default()
            },
            extras: Default::default(),
        };

        let err = SmallestTtsConfig::from_standard(&std)
            .expect_err("unknown Smallest.ai language must be rejected");
        assert!(err.to_string().contains("Unknown language: klingon"));
    }

    #[test]
    fn test_config_validation_rejects_ssrf_endpoint_override() {
        let _env = crate::core::net::ssrf_env_lock();
        let mut config = SmallestTtsConfig::from_base(create_test_config()).unwrap();

        config.endpoint_override = Some("https://smallest-proxy.example.com".to_string());
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

        config.endpoint_override = Some("ws://smallest-proxy.example.com".to_string());
        let err = config
            .validate()
            .expect_err("WebSocket endpoint_override must be rejected");
        assert!(format!("{err:?}").contains("URL scheme"));

        let std = crate::core::tts::standard::StandardTTSConfig::from_base(create_test_config())
            .with_endpoint_override("file:///tmp/socket");
        assert!(SmallestTtsConfig::from_standard(&std).is_err());
    }

    // =========================================================================
    // Model Tests
    // =========================================================================

    #[test]
    fn test_model_as_str() {
        assert_eq!(SmallestModel::LightningV31.as_str(), "lightning_v3.1");
        assert_eq!(
            SmallestModel::LightningV31Pro.as_str(),
            "lightning_v3.1_pro"
        );
        assert_eq!(SmallestModel::LightningV31.path_segment(), "lightning-v3.1");
        assert_eq!(
            SmallestModel::LightningV31Pro.path_segment(),
            "lightning-v3.1-pro"
        );
    }

    #[test]
    fn test_model_from_str() {
        for name in ["lightning_v3.1", "lightning-v3.1", "LIGHTNING_V3.1"] {
            assert_eq!(
                name.parse::<SmallestModel>().unwrap(),
                SmallestModel::LightningV31
            );
        }
        for name in ["lightning_v3.1_pro", "lightning-v3.1-pro"] {
            assert_eq!(
                name.parse::<SmallestModel>().unwrap(),
                SmallestModel::LightningV31Pro
            );
        }
    }

    #[test]
    fn retired_models_parse_to_lightning_v3_1_and_say_so() {
        for name in [
            "lightning",
            "lightning-large",
            "lightning_large",
            "lightning-v2",
            "thunder",
        ] {
            assert_eq!(
                SmallestModel::parse_with_migration(name).unwrap(),
                (SmallestModel::LightningV31, true),
                "{name}"
            );
        }
        assert_eq!(
            SmallestModel::parse_with_migration("lightning_v3.1_pro").unwrap(),
            (SmallestModel::LightningV31Pro, false)
        );
        // serde (a config file) takes the same names.
        let m: SmallestModel = serde_json::from_str("\"lightning-v2\"").unwrap();
        assert_eq!(m, SmallestModel::LightningV31);
        assert_eq!(serde_json::to_string(&m).unwrap(), "\"lightning_v3.1\"");
    }

    #[test]
    fn test_model_invalid() {
        assert!("invalid".parse::<SmallestModel>().is_err());
    }

    #[test]
    fn test_model_supports_voice_cloning() {
        assert!(SmallestModel::LightningV31.supports_voice_cloning());
        assert!(!SmallestModel::LightningV31Pro.supports_voice_cloning());
    }

    // =========================================================================
    // Output Format Tests
    // =========================================================================

    #[test]
    fn test_output_format_as_str() {
        assert_eq!(SmallestOutputFormat::Pcm.as_str(), "pcm");
        assert_eq!(SmallestOutputFormat::Wav.as_str(), "wav");
        assert_eq!(SmallestOutputFormat::Mp3.as_str(), "mp3");
        assert_eq!(SmallestOutputFormat::Mulaw.as_str(), "ulaw");
    }

    #[test]
    fn test_output_format_content_type() {
        assert_eq!(SmallestOutputFormat::Pcm.content_type(), "audio/pcm");
        assert_eq!(SmallestOutputFormat::Wav.content_type(), "audio/wav");
        assert_eq!(SmallestOutputFormat::Mp3.content_type(), "audio/mpeg");
        assert_eq!(SmallestOutputFormat::Mulaw.content_type(), "audio/basic");
    }

    #[test]
    fn test_output_format_from_str() {
        assert_eq!(
            "pcm".parse::<SmallestOutputFormat>().unwrap(),
            SmallestOutputFormat::Pcm
        );
        assert_eq!(
            "wav".parse::<SmallestOutputFormat>().unwrap(),
            SmallestOutputFormat::Wav
        );
        assert_eq!(
            "mp3".parse::<SmallestOutputFormat>().unwrap(),
            SmallestOutputFormat::Mp3
        );
        assert_eq!(
            "mulaw".parse::<SmallestOutputFormat>().unwrap(),
            SmallestOutputFormat::Mulaw
        );
    }

    // =========================================================================
    // Language Tests
    // =========================================================================

    #[test]
    fn test_language_as_code() {
        assert_eq!(SmallestLanguage::English.as_code(), "en");
        assert_eq!(SmallestLanguage::Hindi.as_code(), "hi");
        assert_eq!(SmallestLanguage::German.as_code(), "de");
    }

    #[test]
    fn test_language_from_str() {
        assert_eq!(
            "en".parse::<SmallestLanguage>().unwrap(),
            SmallestLanguage::English
        );
        assert_eq!(
            "hi".parse::<SmallestLanguage>().unwrap(),
            SmallestLanguage::Hindi
        );
        assert_eq!(
            "english".parse::<SmallestLanguage>().unwrap(),
            SmallestLanguage::English
        );
    }

    // =========================================================================
    // Config Tests
    // =========================================================================

    #[test]
    fn test_config_from_base() {
        let base = create_test_config();
        let config = SmallestTtsConfig::from_base(base).unwrap();

        assert_eq!(config.api_key, "test-api-key");
        assert_eq!(config.voice_id, "olivia");
        assert_eq!(config.model, SmallestModel::LightningV31);
        assert_eq!(config.format, SmallestOutputFormat::Wav);
        assert_eq!(config.sample_rate, 24000);
        assert!((config.speed - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_config_from_base_missing_api_key() {
        let mut base = create_test_config();
        base.api_key = String::new();

        let result = SmallestTtsConfig::from_base(base);
        assert!(result.is_err());
    }

    #[test]
    fn test_config_from_base_default_voice() {
        let mut base = create_test_config();
        base.voice_id = None;

        let config = SmallestTtsConfig::from_base(base).unwrap();
        assert_eq!(config.voice_id, DEFAULT_VOICE_ID);
    }

    #[test]
    fn test_config_validate_success() {
        let base = create_test_config();
        let config = SmallestTtsConfig::from_base(base).unwrap();

        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validate_invalid_speed() {
        let base = create_test_config();
        let mut config = SmallestTtsConfig::from_base(base).unwrap();
        config.speed = 10.0; // Way over max

        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_with_speed() {
        let base = create_test_config();
        let config = SmallestTtsConfig::from_base(base).unwrap().with_speed(1.5);

        assert!((config.speed - 1.5).abs() < 0.001);
    }

    #[test]
    fn test_config_with_speed_clamped() {
        let base = create_test_config();
        let config = SmallestTtsConfig::from_base(base).unwrap().with_speed(10.0);

        // Lightning v3.1's max is 2.0
        assert!((config.speed - MAX_SPEED).abs() < 0.001);
    }

    #[test]
    fn test_config_with_consistency() {
        let base = create_test_config();
        let config = SmallestTtsConfig::from_base(base)
            .unwrap()
            .with_consistency(0.8);

        assert!((config.consistency - 0.8).abs() < 0.001);
    }

    #[test]
    fn test_config_with_similarity() {
        let base = create_test_config();
        let config = SmallestTtsConfig::from_base(base)
            .unwrap()
            .with_similarity(0.7);

        assert!((config.similarity - 0.7).abs() < 0.001);
    }

    #[test]
    fn test_config_with_enhancement() {
        let base = create_test_config();
        let config = SmallestTtsConfig::from_base(base)
            .unwrap()
            .with_enhancement(2);

        assert_eq!(config.enhancement, 2);
    }

    #[test]
    fn test_config_with_model() {
        let base = create_test_config();
        let config = SmallestTtsConfig::from_base(base)
            .unwrap()
            .with_model(SmallestModel::LightningV31Pro);

        assert_eq!(config.model, SmallestModel::LightningV31Pro);
    }

    #[test]
    fn test_config_with_language() {
        let base = create_test_config();
        let config = SmallestTtsConfig::from_base(base)
            .unwrap()
            .with_language(SmallestLanguage::Hindi);

        assert_eq!(config.language, SmallestLanguage::Hindi);
    }

    #[test]
    fn test_config_with_format() {
        let base = create_test_config();
        let config = SmallestTtsConfig::from_base(base)
            .unwrap()
            .with_format(SmallestOutputFormat::Mp3);

        assert_eq!(config.format, SmallestOutputFormat::Mp3);
    }

    #[test]
    fn test_config_with_sample_rate() {
        let base = create_test_config();
        let config = SmallestTtsConfig::from_base(base)
            .unwrap()
            .with_sample_rate(16000);

        assert_eq!(config.sample_rate, 16000);
    }

    #[test]
    fn test_config_with_sample_rate_nearest() {
        let base = create_test_config();
        // 22050 is not a Lightning v3.1 rate; it snaps to the nearest (24000)
        let config = SmallestTtsConfig::from_base(base)
            .unwrap()
            .with_sample_rate(22050);

        assert_eq!(config.sample_rate, 24000);
    }
}
