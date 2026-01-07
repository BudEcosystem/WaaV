//! Play.ht TTS configuration types.
//!
//! This module defines the configuration structures for the Play.ht TTS provider,
//! including audio format mappings, model selection, and provider-specific settings.

use serde::{Deserialize, Serialize};

use crate::core::tts::base::TTSConfig;

use super::{
    DEFAULT_MODEL, DEFAULT_SAMPLE_RATE, DEFAULT_SPEED, MAX_SPEED, MAX_TEMPERATURE, MIN_SPEED,
    MIN_TEMPERATURE,
};

// =============================================================================
// Voice Engine / Model
// =============================================================================

/// Play.ht voice engine (model).
///
/// Maps to Play.ht's `voice_engine` parameter in the API request.
///
/// # Available Models
///
/// - `Play30Mini` - Fast, multilingual, streaming support (~190ms latency)
/// - `PlayDialog` - Expressive, dialogue support, two-speaker generation (~350ms latency)
/// - `PlayDialogMultilingual` - Multilingual dialogue support
/// - `PlayDialogArabic` - Arabic dialogue support
/// - `PlayHt20Turbo` - English only, legacy model (~230ms latency)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum PlayHtModel {
    /// Play 3.0 mini - fast, multilingual (default)
    #[default]
    #[serde(rename = "Play3.0-mini")]
    Play30Mini,
    /// PlayDialog - expressive, dialogue support
    #[serde(rename = "PlayDialog")]
    PlayDialog,
    /// PlayDialog Multilingual - multilingual dialogue
    #[serde(rename = "PlayDialogMultilingual")]
    PlayDialogMultilingual,
    /// PlayDialog Arabic - Arabic dialogue support
    #[serde(rename = "PlayDialogArabic")]
    PlayDialogArabic,
    /// PlayHT 2.0 Turbo - legacy English-only model
    #[serde(rename = "PlayHT2.0-turbo")]
    PlayHt20Turbo,
}

impl PlayHtModel {
    /// Returns the Play.ht API string for this model.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use waav_gateway::core::tts::playht::PlayHtModel;
    ///
    /// assert_eq!(PlayHtModel::Play30Mini.as_str(), "Play3.0-mini");
    /// assert_eq!(PlayHtModel::PlayDialog.as_str(), "PlayDialog");
    /// ```
    #[inline]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Play30Mini => "Play3.0-mini",
            Self::PlayDialog => "PlayDialog",
            Self::PlayDialogMultilingual => "PlayDialogMultilingual",
            Self::PlayDialogArabic => "PlayDialogArabic",
            Self::PlayHt20Turbo => "PlayHT2.0-turbo",
        }
    }

    /// Returns whether this model supports dialogue/multi-turn conversations.
    #[inline]
    pub const fn supports_dialogue(&self) -> bool {
        matches!(
            self,
            Self::PlayDialog | Self::PlayDialogMultilingual | Self::PlayDialogArabic
        )
    }

    /// Returns whether this model supports the `language` parameter.
    #[inline]
    pub const fn supports_language_param(&self) -> bool {
        matches!(self, Self::Play30Mini)
    }

    /// Returns whether this model supports guidance parameters.
    #[inline]
    pub const fn supports_guidance(&self) -> bool {
        matches!(self, Self::Play30Mini | Self::PlayHt20Turbo)
    }

    /// Returns the typical latency in milliseconds.
    #[inline]
    pub const fn typical_latency_ms(&self) -> u32 {
        match self {
            Self::Play30Mini => 190,
            Self::PlayDialog | Self::PlayDialogMultilingual | Self::PlayDialogArabic => 350,
            Self::PlayHt20Turbo => 230,
        }
    }

    /// Returns all available models.
    pub const fn all() -> &'static [Self] {
        &[
            Self::Play30Mini,
            Self::PlayDialog,
            Self::PlayDialogMultilingual,
            Self::PlayDialogArabic,
            Self::PlayHt20Turbo,
        ]
    }

    /// Creates a model from a string.
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "play3.0-mini" | "play30mini" | "play3" | "play30" | "mini" => Some(Self::Play30Mini),
            "playdialog" | "dialog" | "dialogue" => Some(Self::PlayDialog),
            "playdialogmultilingual" | "dialogmultilingual" => Some(Self::PlayDialogMultilingual),
            "playdialogarabic" | "dialogarabic" => Some(Self::PlayDialogArabic),
            "playht2.0-turbo" | "playht20turbo" | "turbo" | "2.0" => Some(Self::PlayHt20Turbo),
            _ => None,
        }
    }
}

impl std::fmt::Display for PlayHtModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =============================================================================
// Audio Format
// =============================================================================

/// Play.ht audio output format.
///
/// Maps to Play.ht's `output_format` parameter in the API request.
///
/// # Supported Formats
///
/// - `Mp3` - MP3 format (default)
/// - `Wav` - WAV container
/// - `Mulaw` - G.711 mu-law
/// - `Flac` - FLAC lossless
/// - `Ogg` - OGG container (Vorbis)
/// - `Raw` - Raw PCM data
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PlayHtAudioFormat {
    /// MP3 format (default)
    #[default]
    Mp3,
    /// WAV container
    Wav,
    /// G.711 mu-law
    Mulaw,
    /// FLAC lossless
    Flac,
    /// OGG container (Vorbis)
    Ogg,
    /// Raw PCM data
    Raw,
}

impl PlayHtAudioFormat {
    /// Returns the Play.ht API format string.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use waav_gateway::core::tts::playht::PlayHtAudioFormat;
    ///
    /// assert_eq!(PlayHtAudioFormat::Mp3.as_str(), "mp3");
    /// assert_eq!(PlayHtAudioFormat::Raw.as_str(), "raw");
    /// ```
    #[inline]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Mp3 => "mp3",
            Self::Wav => "wav",
            Self::Mulaw => "mulaw",
            Self::Flac => "flac",
            Self::Ogg => "ogg",
            Self::Raw => "raw",
        }
    }

    /// Returns the MIME content type for the Accept header.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use waav_gateway::core::tts::playht::PlayHtAudioFormat;
    ///
    /// assert_eq!(PlayHtAudioFormat::Mp3.content_type(), "audio/mpeg");
    /// assert_eq!(PlayHtAudioFormat::Raw.content_type(), "audio/pcm");
    /// ```
    #[inline]
    pub const fn content_type(&self) -> &'static str {
        match self {
            Self::Mp3 => "audio/mpeg",
            Self::Wav => "audio/wav",
            Self::Mulaw => "audio/basic",
            Self::Flac => "audio/flac",
            Self::Ogg => "audio/ogg",
            Self::Raw => "audio/pcm",
        }
    }

    /// Returns the bytes per sample for raw PCM format.
    ///
    /// Returns `None` for compressed formats.
    #[inline]
    pub const fn bytes_per_sample(&self) -> Option<usize> {
        match self {
            Self::Raw => Some(2), // 16-bit PCM
            Self::Mulaw => Some(1),
            _ => None,
        }
    }

    /// Creates a `PlayHtAudioFormat` from a WaaV base format string.
    ///
    /// # Supported Mappings
    ///
    /// | WaaV Format | Play.ht Format |
    /// |-------------|----------------|
    /// | `linear16`, `pcm` | `Raw` |
    /// | `mp3`, `mpeg` | `Mp3` |
    /// | `mulaw`, `ulaw` | `Mulaw` |
    /// | `wav`, `wave` | `Wav` |
    /// | `flac` | `Flac` |
    /// | `ogg`, `vorbis` | `Ogg` |
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use waav_gateway::core::tts::playht::PlayHtAudioFormat;
    ///
    /// assert_eq!(PlayHtAudioFormat::from_base_format("linear16"), PlayHtAudioFormat::Raw);
    /// assert_eq!(PlayHtAudioFormat::from_base_format("mp3"), PlayHtAudioFormat::Mp3);
    /// assert_eq!(PlayHtAudioFormat::from_base_format("unknown"), PlayHtAudioFormat::Mp3);
    /// ```
    pub fn from_base_format(format: &str) -> Self {
        match format.to_lowercase().as_str() {
            "linear16" | "pcm" | "pcm16" | "raw" => Self::Raw,
            "mp3" | "mpeg" => Self::Mp3,
            "mulaw" | "ulaw" | "g711" => Self::Mulaw,
            "wav" | "wave" => Self::Wav,
            "flac" => Self::Flac,
            "ogg" | "vorbis" => Self::Ogg,
            // Default to MP3 for good balance of quality and size
            _ => Self::Mp3,
        }
    }

    /// Returns all available formats.
    pub const fn all() -> &'static [Self] {
        &[
            Self::Mp3,
            Self::Wav,
            Self::Mulaw,
            Self::Flac,
            Self::Ogg,
            Self::Raw,
        ]
    }
}

impl std::fmt::Display for PlayHtAudioFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =============================================================================
// Play.ht TTS Configuration
// =============================================================================

/// Play.ht-specific TTS configuration.
///
/// This structure extends the base `TTSConfig` with Play.ht-specific parameters
/// for controlling voice synthesis.
///
/// # Parameters
///
/// - **user_id**: Play.ht user ID for authentication (required)
/// - **voice_engine**: The model to use (default: Play3.0-mini)
/// - **output_format**: Audio format for the response
/// - **sample_rate**: Output sample rate (8000, 16000, 24000, 44100, 48000 Hz)
/// - **speed**: Playback speed (0.5-2.0, default 1.0)
/// - **quality**: Audio quality tier (draft/standard/premium)
/// - **temperature**: Randomness control (0.0-1.0)
/// - **seed**: Random seed for deterministic output
/// - **language**: ISO 639-1 language code (Play3.0-mini only)
///
/// # PlayDialog Multi-Turn Parameters
///
/// - **voice_2**: Second speaker voice URL
/// - **turn_prefix**: First speaker identifier (e.g., "S1:")
/// - **turn_prefix_2**: Second speaker identifier (e.g., "S2:")
///
/// # Example
///
/// ```rust,ignore
/// use waav_gateway::core::tts::playht::{PlayHtTtsConfig, PlayHtAudioFormat, PlayHtModel};
/// use waav_gateway::core::tts::TTSConfig;
///
/// let base = TTSConfig {
///     api_key: "your-api-key".to_string(),
///     voice_id: Some("s3://voice-cloning-zero-shot/.../manifest.json".to_string()),
///     ..Default::default()
/// };
///
/// let config = PlayHtTtsConfig::from_base(base, "your-user-id".to_string())
///     .with_model(PlayHtModel::PlayDialog)
///     .with_speed(1.2);
/// ```
#[derive(Debug, Clone)]
pub struct PlayHtTtsConfig {
    /// Base TTS configuration (contains api_key, voice_id, etc.)
    pub base: TTSConfig,

    /// Play.ht user ID for authentication
    pub user_id: String,

    /// Voice engine / model to use
    pub voice_engine: PlayHtModel,

    /// Output audio format
    pub output_format: PlayHtAudioFormat,

    /// Sample rate in Hz (8000, 16000, 24000, 44100, 48000)
    pub sample_rate: u32,

    /// Playback speed (0.5-2.0, default 1.0)
    pub speed: f32,

    /// Audio quality tier (draft/standard/premium)
    pub quality: Option<String>,

    /// Randomness control (0.0-1.0)
    pub temperature: Option<f32>,

    /// Random seed for deterministic output
    pub seed: Option<i64>,

    /// ISO 639-1 language code (Play3.0-mini only)
    pub language: Option<String>,

    /// Text adherence control (Play3.0, PlayHT2.0)
    pub text_guidance: Option<f32>,

    /// Voice adherence control (Play3.0, PlayHT2.0)
    pub voice_guidance: Option<f32>,

    /// Style adherence control (Play3.0 only)
    pub style_guidance: Option<f32>,

    /// Repetition penalty (Play3.0, PlayHT2.0)
    pub repetition_penalty: Option<f32>,

    /// Second speaker voice URL (PlayDialog)
    pub voice_2: Option<String>,

    /// First speaker identifier (PlayDialog)
    pub turn_prefix: Option<String>,

    /// Second speaker identifier (PlayDialog)
    pub turn_prefix_2: Option<String>,

    /// Voice conditioning seconds (PlayDialog)
    pub voice_conditioning_seconds: Option<f32>,

    /// Number of candidates for ranking (PlayDialog)
    pub num_candidates: Option<u32>,
}

impl PlayHtTtsConfig {
    /// Creates a new Play.ht TTS config from base TTS config.
    ///
    /// This constructor extracts Play.ht-specific settings from the base config
    /// and applies defaults for unspecified values.
    ///
    /// # Arguments
    ///
    /// * `base` - Base TTS configuration
    /// * `user_id` - Play.ht user ID for authentication
    pub fn from_base(base: TTSConfig, user_id: String) -> Self {
        let output_format = base
            .audio_format
            .as_ref()
            .map(|f| PlayHtAudioFormat::from_base_format(f))
            .unwrap_or_default();

        let sample_rate = base.sample_rate.unwrap_or(DEFAULT_SAMPLE_RATE);

        // Extract speed from speaking_rate if provided
        let speed = base.speaking_rate.unwrap_or(DEFAULT_SPEED);

        // Extract model from base.model if provided
        let voice_engine = if base.model.is_empty() {
            DEFAULT_MODEL
        } else {
            PlayHtModel::parse(&base.model).unwrap_or(DEFAULT_MODEL)
        };

        Self {
            base,
            user_id,
            voice_engine,
            output_format,
            sample_rate,
            speed,
            quality: None,
            temperature: None,
            seed: None,
            language: None,
            text_guidance: None,
            voice_guidance: None,
            style_guidance: None,
            repetition_penalty: None,
            voice_2: None,
            turn_prefix: None,
            turn_prefix_2: None,
            voice_conditioning_seconds: None,
            num_candidates: None,
        }
    }

    /// Returns the voice ID to use for synthesis.
    ///
    /// # Panics
    ///
    /// This method should only be called after validation. If voice_id is not set,
    /// validation will fail. Use `try_voice_id()` for safe access.
    #[inline]
    pub fn voice_id(&self) -> &str {
        self.base
            .voice_id
            .as_deref()
            .expect("voice_id should be validated before access")
    }

    /// Returns the voice ID if set, or None.
    #[inline]
    pub fn try_voice_id(&self) -> Option<&str> {
        self.base.voice_id.as_deref()
    }

    /// Returns whether a voice ID is configured.
    #[inline]
    pub fn has_voice_id(&self) -> bool {
        self.base.voice_id.is_some()
    }

    /// Sets the voice engine / model.
    #[inline]
    pub fn with_model(mut self, model: PlayHtModel) -> Self {
        self.voice_engine = model;
        self
    }

    /// Sets the output format.
    #[inline]
    pub fn with_format(mut self, format: PlayHtAudioFormat) -> Self {
        self.output_format = format;
        self
    }

    /// Sets the sample rate.
    #[inline]
    pub fn with_sample_rate(mut self, sample_rate: u32) -> Self {
        self.sample_rate = sample_rate;
        self
    }

    /// Sets the speed multiplier.
    ///
    /// Value is clamped to [0.5, 2.0].
    #[inline]
    pub fn with_speed(mut self, speed: f32) -> Self {
        self.speed = speed.clamp(MIN_SPEED, MAX_SPEED);
        self
    }

    /// Sets the quality tier.
    #[inline]
    pub fn with_quality(mut self, quality: impl Into<String>) -> Self {
        self.quality = Some(quality.into());
        self
    }

    /// Sets the temperature value.
    ///
    /// Value is clamped to [0.0, 1.0].
    #[inline]
    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature.clamp(MIN_TEMPERATURE, MAX_TEMPERATURE));
        self
    }

    /// Sets the random seed for deterministic output.
    #[inline]
    pub fn with_seed(mut self, seed: i64) -> Self {
        self.seed = Some(seed);
        self
    }

    /// Clears the random seed.
    #[inline]
    pub fn without_seed(mut self) -> Self {
        self.seed = None;
        self
    }

    /// Sets the language code.
    #[inline]
    pub fn with_language(mut self, language: impl Into<String>) -> Self {
        self.language = Some(language.into());
        self
    }

    /// Sets the text guidance value.
    #[inline]
    pub fn with_text_guidance(mut self, value: f32) -> Self {
        self.text_guidance = Some(value);
        self
    }

    /// Sets the voice guidance value.
    #[inline]
    pub fn with_voice_guidance(mut self, value: f32) -> Self {
        self.voice_guidance = Some(value);
        self
    }

    /// Sets the style guidance value.
    #[inline]
    pub fn with_style_guidance(mut self, value: f32) -> Self {
        self.style_guidance = Some(value);
        self
    }

    /// Sets the repetition penalty.
    #[inline]
    pub fn with_repetition_penalty(mut self, value: f32) -> Self {
        self.repetition_penalty = Some(value);
        self
    }

    /// Sets the second speaker voice for PlayDialog.
    #[inline]
    pub fn with_voice_2(mut self, voice: impl Into<String>) -> Self {
        self.voice_2 = Some(voice.into());
        self
    }

    /// Sets the turn prefixes for PlayDialog multi-turn.
    #[inline]
    pub fn with_turn_prefixes(
        mut self,
        prefix1: impl Into<String>,
        prefix2: impl Into<String>,
    ) -> Self {
        self.turn_prefix = Some(prefix1.into());
        self.turn_prefix_2 = Some(prefix2.into());
        self
    }

    /// Validates the configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - `user_id` is empty
    /// - `voice_id` is not set (required for Play.ht)
    /// - `speed` is outside [0.5, 2.0]
    /// - `temperature` is outside [0.0, 1.0]
    /// - `sample_rate` is not 8000, 16000, 24000, 44100, or 48000
    /// - `language` is set for a non-Play3.0-mini model
    /// - PlayDialog parameters are set for non-PlayDialog model
    pub fn validate(&self) -> Result<(), String> {
        // Validate user_id
        if self.user_id.is_empty() {
            return Err("user_id is required for Play.ht authentication".to_string());
        }

        // Validate voice_id - required for Play.ht
        if self.base.voice_id.is_none() || self.base.voice_id.as_ref().is_some_and(|v| v.is_empty())
        {
            return Err(
                "voice_id is required for Play.ht. Provide a voice manifest URL (e.g., 's3://voice-cloning-zero-shot/.../manifest.json') or use the voice list API to discover available voices."
                    .to_string(),
            );
        }

        // Validate speed
        if self.speed < MIN_SPEED || self.speed > MAX_SPEED {
            return Err(format!(
                "speed must be between {} and {}, got {}",
                MIN_SPEED, MAX_SPEED, self.speed
            ));
        }

        // Validate temperature if set
        if let Some(temp) = self.temperature
            && !(MIN_TEMPERATURE..=MAX_TEMPERATURE).contains(&temp)
        {
            return Err(format!(
                "temperature must be between {} and {}, got {}",
                MIN_TEMPERATURE, MAX_TEMPERATURE, temp
            ));
        }

        // Validate sample rate
        if !matches!(self.sample_rate, 8000 | 16000 | 24000 | 44100 | 48000) {
            return Err(format!(
                "sample_rate must be 8000, 16000, 24000, 44100, or 48000, got {}",
                self.sample_rate
            ));
        }

        // Warn about language param on non-Play3.0-mini models
        if self.language.is_some() && !self.voice_engine.supports_language_param() {
            return Err(format!(
                "language parameter is only supported for Play3.0-mini, not {}",
                self.voice_engine
            ));
        }

        // Validate PlayDialog parameters
        if (self.voice_2.is_some() || self.turn_prefix.is_some() || self.turn_prefix_2.is_some())
            && !self.voice_engine.supports_dialogue()
        {
            return Err(format!(
                "PlayDialog parameters (voice_2, turn_prefix) are only supported for PlayDialog models, not {}",
                self.voice_engine
            ));
        }

        Ok(())
    }

    /// Returns whether this config uses default values for all optional parameters.
    pub fn is_default(&self) -> bool {
        self.voice_engine == DEFAULT_MODEL
            && (self.speed - DEFAULT_SPEED).abs() < 0.001
            && self.quality.is_none()
            && self.temperature.is_none()
            && self.seed.is_none()
            && self.language.is_none()
            && self.text_guidance.is_none()
            && self.voice_guidance.is_none()
            && self.style_guidance.is_none()
            && self.repetition_penalty.is_none()
            && self.voice_2.is_none()
            && self.turn_prefix.is_none()
            && self.turn_prefix_2.is_none()
    }
}

impl Default for PlayHtTtsConfig {
    fn default() -> Self {
        Self::from_base(TTSConfig::default(), String::new())
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // PlayHtModel Tests
    // =========================================================================

    #[test]
    fn test_model_as_str() {
        assert_eq!(PlayHtModel::Play30Mini.as_str(), "Play3.0-mini");
        assert_eq!(PlayHtModel::PlayDialog.as_str(), "PlayDialog");
        assert_eq!(
            PlayHtModel::PlayDialogMultilingual.as_str(),
            "PlayDialogMultilingual"
        );
        assert_eq!(PlayHtModel::PlayDialogArabic.as_str(), "PlayDialogArabic");
        assert_eq!(PlayHtModel::PlayHt20Turbo.as_str(), "PlayHT2.0-turbo");
    }

    #[test]
    fn test_model_supports_dialogue() {
        assert!(!PlayHtModel::Play30Mini.supports_dialogue());
        assert!(PlayHtModel::PlayDialog.supports_dialogue());
        assert!(PlayHtModel::PlayDialogMultilingual.supports_dialogue());
        assert!(PlayHtModel::PlayDialogArabic.supports_dialogue());
        assert!(!PlayHtModel::PlayHt20Turbo.supports_dialogue());
    }

    #[test]
    fn test_model_supports_language_param() {
        assert!(PlayHtModel::Play30Mini.supports_language_param());
        assert!(!PlayHtModel::PlayDialog.supports_language_param());
        assert!(!PlayHtModel::PlayDialogMultilingual.supports_language_param());
        assert!(!PlayHtModel::PlayDialogArabic.supports_language_param());
        assert!(!PlayHtModel::PlayHt20Turbo.supports_language_param());
    }

    #[test]
    fn test_model_supports_guidance() {
        assert!(PlayHtModel::Play30Mini.supports_guidance());
        assert!(!PlayHtModel::PlayDialog.supports_guidance());
        assert!(!PlayHtModel::PlayDialogMultilingual.supports_guidance());
        assert!(!PlayHtModel::PlayDialogArabic.supports_guidance());
        assert!(PlayHtModel::PlayHt20Turbo.supports_guidance());
    }

    #[test]
    fn test_model_typical_latency() {
        assert_eq!(PlayHtModel::Play30Mini.typical_latency_ms(), 190);
        assert_eq!(PlayHtModel::PlayDialog.typical_latency_ms(), 350);
        assert_eq!(PlayHtModel::PlayHt20Turbo.typical_latency_ms(), 230);
    }

    #[test]
    fn test_model_from_str() {
        assert_eq!(
            PlayHtModel::parse("Play3.0-mini"),
            Some(PlayHtModel::Play30Mini)
        );
        assert_eq!(
            PlayHtModel::parse("play30mini"),
            Some(PlayHtModel::Play30Mini)
        );
        assert_eq!(
            PlayHtModel::parse("PlayDialog"),
            Some(PlayHtModel::PlayDialog)
        );
        assert_eq!(
            PlayHtModel::parse("turbo"),
            Some(PlayHtModel::PlayHt20Turbo)
        );
        assert_eq!(PlayHtModel::parse("invalid"), None);
    }

    #[test]
    fn test_model_default() {
        assert_eq!(PlayHtModel::default(), PlayHtModel::Play30Mini);
    }

    #[test]
    fn test_model_display() {
        assert_eq!(format!("{}", PlayHtModel::Play30Mini), "Play3.0-mini");
        assert_eq!(format!("{}", PlayHtModel::PlayDialog), "PlayDialog");
    }

    #[test]
    fn test_model_all() {
        let all = PlayHtModel::all();
        assert_eq!(all.len(), 5);
        assert!(all.contains(&PlayHtModel::Play30Mini));
        assert!(all.contains(&PlayHtModel::PlayDialog));
    }

    // =========================================================================
    // PlayHtAudioFormat Tests
    // =========================================================================

    #[test]
    fn test_audio_format_as_str() {
        assert_eq!(PlayHtAudioFormat::Mp3.as_str(), "mp3");
        assert_eq!(PlayHtAudioFormat::Wav.as_str(), "wav");
        assert_eq!(PlayHtAudioFormat::Mulaw.as_str(), "mulaw");
        assert_eq!(PlayHtAudioFormat::Flac.as_str(), "flac");
        assert_eq!(PlayHtAudioFormat::Ogg.as_str(), "ogg");
        assert_eq!(PlayHtAudioFormat::Raw.as_str(), "raw");
    }

    #[test]
    fn test_audio_format_content_type() {
        assert_eq!(PlayHtAudioFormat::Mp3.content_type(), "audio/mpeg");
        assert_eq!(PlayHtAudioFormat::Wav.content_type(), "audio/wav");
        assert_eq!(PlayHtAudioFormat::Mulaw.content_type(), "audio/basic");
        assert_eq!(PlayHtAudioFormat::Flac.content_type(), "audio/flac");
        assert_eq!(PlayHtAudioFormat::Ogg.content_type(), "audio/ogg");
        assert_eq!(PlayHtAudioFormat::Raw.content_type(), "audio/pcm");
    }

    #[test]
    fn test_audio_format_bytes_per_sample() {
        assert_eq!(PlayHtAudioFormat::Raw.bytes_per_sample(), Some(2));
        assert_eq!(PlayHtAudioFormat::Mulaw.bytes_per_sample(), Some(1));
        assert_eq!(PlayHtAudioFormat::Mp3.bytes_per_sample(), None);
        assert_eq!(PlayHtAudioFormat::Wav.bytes_per_sample(), None);
    }

    #[test]
    fn test_audio_format_from_base_format_pcm() {
        assert_eq!(
            PlayHtAudioFormat::from_base_format("linear16"),
            PlayHtAudioFormat::Raw
        );
        assert_eq!(
            PlayHtAudioFormat::from_base_format("pcm"),
            PlayHtAudioFormat::Raw
        );
        assert_eq!(
            PlayHtAudioFormat::from_base_format("raw"),
            PlayHtAudioFormat::Raw
        );
    }

    #[test]
    fn test_audio_format_from_base_format_mp3() {
        assert_eq!(
            PlayHtAudioFormat::from_base_format("mp3"),
            PlayHtAudioFormat::Mp3
        );
        assert_eq!(
            PlayHtAudioFormat::from_base_format("MP3"),
            PlayHtAudioFormat::Mp3
        );
        assert_eq!(
            PlayHtAudioFormat::from_base_format("mpeg"),
            PlayHtAudioFormat::Mp3
        );
    }

    #[test]
    fn test_audio_format_from_base_format_mulaw() {
        assert_eq!(
            PlayHtAudioFormat::from_base_format("mulaw"),
            PlayHtAudioFormat::Mulaw
        );
        assert_eq!(
            PlayHtAudioFormat::from_base_format("ulaw"),
            PlayHtAudioFormat::Mulaw
        );
        assert_eq!(
            PlayHtAudioFormat::from_base_format("g711"),
            PlayHtAudioFormat::Mulaw
        );
    }

    #[test]
    fn test_audio_format_from_base_format_other() {
        assert_eq!(
            PlayHtAudioFormat::from_base_format("wav"),
            PlayHtAudioFormat::Wav
        );
        assert_eq!(
            PlayHtAudioFormat::from_base_format("flac"),
            PlayHtAudioFormat::Flac
        );
        assert_eq!(
            PlayHtAudioFormat::from_base_format("ogg"),
            PlayHtAudioFormat::Ogg
        );
    }

    #[test]
    fn test_audio_format_from_base_format_unknown_defaults_to_mp3() {
        assert_eq!(
            PlayHtAudioFormat::from_base_format("unknown"),
            PlayHtAudioFormat::Mp3
        );
        assert_eq!(
            PlayHtAudioFormat::from_base_format(""),
            PlayHtAudioFormat::Mp3
        );
    }

    #[test]
    fn test_audio_format_default() {
        assert_eq!(PlayHtAudioFormat::default(), PlayHtAudioFormat::Mp3);
    }

    #[test]
    fn test_audio_format_display() {
        assert_eq!(format!("{}", PlayHtAudioFormat::Mp3), "mp3");
        assert_eq!(format!("{}", PlayHtAudioFormat::Raw), "raw");
    }

    #[test]
    fn test_audio_format_all() {
        let all = PlayHtAudioFormat::all();
        assert_eq!(all.len(), 6);
        assert!(all.contains(&PlayHtAudioFormat::Mp3));
        assert!(all.contains(&PlayHtAudioFormat::Raw));
    }

    // =========================================================================
    // PlayHtTtsConfig Tests
    // =========================================================================

    fn create_test_base_config() -> TTSConfig {
        TTSConfig {
            provider: "playht".to_string(),
            api_key: "test-api-key".to_string(),
            voice_id: Some("s3://test-voice/manifest.json".to_string()),
            model: String::new(),
            speaking_rate: None,
            audio_format: Some("mp3".to_string()),
            sample_rate: Some(48000),
            connection_timeout: Some(30),
            request_timeout: Some(60),
            pronunciations: Vec::new(),
            request_pool_size: Some(4),
            emotion_config: None,
        }
    }

    #[test]
    fn test_config_from_base() {
        let base = create_test_base_config();
        let config = PlayHtTtsConfig::from_base(base, "test-user-id".to_string());

        assert_eq!(config.user_id, "test-user-id");
        assert_eq!(config.voice_id(), "s3://test-voice/manifest.json");
        assert_eq!(config.voice_engine, PlayHtModel::Play30Mini);
        assert_eq!(config.output_format, PlayHtAudioFormat::Mp3);
        assert_eq!(config.sample_rate, 48000);
        assert!((config.speed - DEFAULT_SPEED).abs() < 0.001);
        assert!(config.quality.is_none());
        assert!(config.temperature.is_none());
        assert!(config.seed.is_none());
    }

    #[test]
    fn test_config_missing_voice_id_validation_fails() {
        let mut base = create_test_base_config();
        base.voice_id = None;
        let config = PlayHtTtsConfig::from_base(base, "test-user-id".to_string());

        // Config without voice_id should fail validation
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("voice_id"));
    }

    #[test]
    fn test_config_empty_voice_id_validation_fails() {
        let mut base = create_test_base_config();
        base.voice_id = Some(String::new());
        let config = PlayHtTtsConfig::from_base(base, "test-user-id".to_string());

        // Config with empty voice_id should fail validation
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("voice_id"));
    }

    #[test]
    fn test_config_try_voice_id() {
        let base = create_test_base_config();
        let config = PlayHtTtsConfig::from_base(base, "test-user-id".to_string());

        assert_eq!(config.try_voice_id(), Some("s3://test-voice/manifest.json"));
    }

    #[test]
    fn test_config_has_voice_id() {
        let base = create_test_base_config();
        let config = PlayHtTtsConfig::from_base(base, "test-user-id".to_string());
        assert!(config.has_voice_id());

        let mut base_no_voice = create_test_base_config();
        base_no_voice.voice_id = None;
        let config_no_voice = PlayHtTtsConfig::from_base(base_no_voice, "test-user-id".to_string());
        assert!(!config_no_voice.has_voice_id());
    }

    #[test]
    fn test_config_with_model() {
        let base = create_test_base_config();
        let config = PlayHtTtsConfig::from_base(base, "test-user".to_string())
            .with_model(PlayHtModel::PlayDialog);

        assert_eq!(config.voice_engine, PlayHtModel::PlayDialog);
    }

    #[test]
    fn test_config_with_format() {
        let base = create_test_base_config();
        let config = PlayHtTtsConfig::from_base(base, "test-user".to_string())
            .with_format(PlayHtAudioFormat::Raw);

        assert_eq!(config.output_format, PlayHtAudioFormat::Raw);
    }

    #[test]
    fn test_config_with_speed() {
        let base = create_test_base_config();
        let config = PlayHtTtsConfig::from_base(base, "test-user".to_string()).with_speed(1.5);

        assert!((config.speed - 1.5).abs() < 0.001);
    }

    #[test]
    fn test_config_with_speed_clamped() {
        let base = create_test_base_config();

        // Test clamping to max
        let config =
            PlayHtTtsConfig::from_base(base.clone(), "test-user".to_string()).with_speed(3.0);
        assert!((config.speed - MAX_SPEED).abs() < 0.001);

        // Test clamping to min
        let config = PlayHtTtsConfig::from_base(base, "test-user".to_string()).with_speed(0.1);
        assert!((config.speed - MIN_SPEED).abs() < 0.001);
    }

    #[test]
    fn test_config_with_temperature() {
        let base = create_test_base_config();
        let config =
            PlayHtTtsConfig::from_base(base, "test-user".to_string()).with_temperature(0.8);

        assert_eq!(config.temperature, Some(0.8));
    }

    #[test]
    fn test_config_with_temperature_clamped() {
        let base = create_test_base_config();

        // Test clamping to max
        let config =
            PlayHtTtsConfig::from_base(base.clone(), "test-user".to_string()).with_temperature(1.5);
        assert_eq!(config.temperature, Some(MAX_TEMPERATURE));

        // Test clamping to min
        let config =
            PlayHtTtsConfig::from_base(base, "test-user".to_string()).with_temperature(-0.5);
        assert_eq!(config.temperature, Some(MIN_TEMPERATURE));
    }

    #[test]
    fn test_config_with_seed() {
        let base = create_test_base_config();
        let config = PlayHtTtsConfig::from_base(base, "test-user".to_string()).with_seed(12345);

        assert_eq!(config.seed, Some(12345));
    }

    #[test]
    fn test_config_without_seed() {
        let base = create_test_base_config();
        let config = PlayHtTtsConfig::from_base(base, "test-user".to_string())
            .with_seed(12345)
            .without_seed();

        assert!(config.seed.is_none());
    }

    #[test]
    fn test_config_with_language() {
        let base = create_test_base_config();
        let config = PlayHtTtsConfig::from_base(base, "test-user".to_string()).with_language("en");

        assert_eq!(config.language, Some("en".to_string()));
    }

    #[test]
    fn test_config_with_turn_prefixes() {
        let base = create_test_base_config();
        let config = PlayHtTtsConfig::from_base(base, "test-user".to_string())
            .with_model(PlayHtModel::PlayDialog)
            .with_turn_prefixes("S1:", "S2:");

        assert_eq!(config.turn_prefix, Some("S1:".to_string()));
        assert_eq!(config.turn_prefix_2, Some("S2:".to_string()));
    }

    #[test]
    fn test_config_validate_success() {
        let base = create_test_base_config();
        let config = PlayHtTtsConfig::from_base(base, "test-user".to_string());

        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validate_empty_user_id() {
        let base = create_test_base_config();
        let config = PlayHtTtsConfig::from_base(base, String::new());

        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("user_id"));
    }

    #[test]
    fn test_config_validate_invalid_sample_rate() {
        let mut base = create_test_base_config();
        base.sample_rate = Some(22050);
        let config = PlayHtTtsConfig::from_base(base, "test-user".to_string());

        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("sample_rate"));
    }

    #[test]
    fn test_config_validate_language_on_wrong_model() {
        let base = create_test_base_config();
        let config = PlayHtTtsConfig::from_base(base, "test-user".to_string())
            .with_model(PlayHtModel::PlayDialog)
            .with_language("en");

        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("language"));
    }

    #[test]
    fn test_config_validate_dialogue_params_on_wrong_model() {
        let base = create_test_base_config();
        let config = PlayHtTtsConfig::from_base(base, "test-user".to_string())
            .with_model(PlayHtModel::Play30Mini)
            .with_turn_prefixes("S1:", "S2:");

        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("PlayDialog"));
    }

    #[test]
    fn test_config_is_default() {
        let base = create_test_base_config();
        let config = PlayHtTtsConfig::from_base(base, "test-user".to_string());

        assert!(config.is_default());
    }

    #[test]
    fn test_config_is_not_default_with_speed() {
        let base = create_test_base_config();
        let config = PlayHtTtsConfig::from_base(base, "test-user".to_string()).with_speed(1.5);

        assert!(!config.is_default());
    }

    #[test]
    fn test_config_is_not_default_with_model() {
        let base = create_test_base_config();
        let config = PlayHtTtsConfig::from_base(base, "test-user".to_string())
            .with_model(PlayHtModel::PlayDialog);

        assert!(!config.is_default());
    }

    #[test]
    fn test_config_extracts_speed_from_speaking_rate() {
        let mut base = create_test_base_config();
        base.speaking_rate = Some(1.5);
        let config = PlayHtTtsConfig::from_base(base, "test-user".to_string());

        assert!((config.speed - 1.5).abs() < 0.001);
    }

    #[test]
    fn test_config_extracts_model_from_base() {
        let mut base = create_test_base_config();
        base.model = "PlayDialog".to_string();
        let config = PlayHtTtsConfig::from_base(base, "test-user".to_string());

        assert_eq!(config.voice_engine, PlayHtModel::PlayDialog);
    }
}
