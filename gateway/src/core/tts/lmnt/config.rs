//! LMNT TTS configuration types.
//!
//! This module defines the configuration structures for the LMNT TTS provider,
//! including audio format mappings and provider-specific settings.

use serde::{Deserialize, Serialize};

use crate::core::tts::base::TTSConfig;

use super::{
    DEFAULT_LANGUAGE, DEFAULT_MODEL, DEFAULT_SAMPLE_RATE, DEFAULT_SPEED, DEFAULT_TEMPERATURE,
    DEFAULT_TOP_P, MAX_SPEED, MAX_TOP_P, MIN_SPEED, MIN_TEMPERATURE, MIN_TOP_P,
};

// =============================================================================
// Audio Format
// =============================================================================

/// LMNT audio output format.
///
/// Maps to LMNT's `format` parameter in the API request.
///
/// # Streamable Formats
///
/// - `Mp3` - 96kbps MP3 (default)
/// - `PcmS16le` - 16-bit signed little-endian PCM
/// - `PcmF32le` - 32-bit float little-endian PCM
/// - `Ulaw` - 8-bit G.711 µ-law
/// - `Webm` - WebM container with Opus codec
///
/// # Non-Streamable Formats
///
/// - `Aac` - AAC audio (not recommended for streaming)
/// - `Wav` - WAV container (not recommended for streaming)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LmntAudioFormat {
    /// MP3 format (96kbps, streamable) - default
    #[default]
    Mp3,
    /// 16-bit signed PCM (little-endian, streamable)
    PcmS16le,
    /// 32-bit float PCM (little-endian, streamable)
    PcmF32le,
    /// 8-bit G.711 µ-law (streamable)
    Ulaw,
    /// WebM container with Opus codec (streamable)
    Webm,
    /// AAC format (non-streamable)
    Aac,
    /// WAV format (non-streamable)
    Wav,
}

impl LmntAudioFormat {
    /// Returns the LMNT API format string.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use waav_gateway::core::tts::lmnt::LmntAudioFormat;
    ///
    /// assert_eq!(LmntAudioFormat::PcmS16le.as_str(), "pcm_s16le");
    /// assert_eq!(LmntAudioFormat::Mp3.as_str(), "mp3");
    /// ```
    #[inline]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Mp3 => "mp3",
            Self::PcmS16le => "pcm_s16le",
            Self::PcmF32le => "pcm_f32le",
            Self::Ulaw => "ulaw",
            Self::Webm => "webm",
            Self::Aac => "aac",
            Self::Wav => "wav",
        }
    }

    /// Returns the MIME content type for the Accept header.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use waav_gateway::core::tts::lmnt::LmntAudioFormat;
    ///
    /// assert_eq!(LmntAudioFormat::PcmS16le.content_type(), "audio/pcm");
    /// assert_eq!(LmntAudioFormat::Mp3.content_type(), "audio/mpeg");
    /// ```
    #[inline]
    pub const fn content_type(&self) -> &'static str {
        match self {
            Self::Mp3 => "audio/mpeg",
            Self::PcmS16le | Self::PcmF32le => "audio/pcm",
            Self::Ulaw => "audio/basic",
            Self::Webm => "audio/webm",
            Self::Aac => "audio/aac",
            Self::Wav => "audio/wav",
        }
    }

    /// Returns whether this format supports streaming.
    ///
    /// Non-streamable formats (AAC, WAV) should not be used for real-time
    /// voice applications as they require the full audio to be generated
    /// before delivery.
    #[inline]
    pub const fn is_streamable(&self) -> bool {
        match self {
            Self::Mp3 | Self::PcmS16le | Self::PcmF32le | Self::Ulaw | Self::Webm => true,
            Self::Aac | Self::Wav => false,
        }
    }

    /// Returns the bytes per sample for PCM formats.
    ///
    /// Returns `None` for compressed formats.
    #[inline]
    pub const fn bytes_per_sample(&self) -> Option<usize> {
        match self {
            Self::PcmS16le => Some(2),
            Self::PcmF32le => Some(4),
            Self::Ulaw => Some(1),
            _ => None,
        }
    }

    /// Creates an `LmntAudioFormat` from a WaaV base format string.
    ///
    /// # Supported Mappings
    ///
    /// | WaaV Format | LMNT Format |
    /// |-------------|-------------|
    /// | `linear16`, `pcm` | `PcmS16le` |
    /// | `mp3` | `Mp3` |
    /// | `mulaw`, `ulaw` | `Ulaw` |
    /// | `wav` | `Wav` |
    /// | `webm` | `Webm` |
    /// | `aac` | `Aac` |
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use waav_gateway::core::tts::lmnt::LmntAudioFormat;
    ///
    /// assert_eq!(LmntAudioFormat::from_base_format("linear16"), LmntAudioFormat::PcmS16le);
    /// assert_eq!(LmntAudioFormat::from_base_format("mp3"), LmntAudioFormat::Mp3);
    /// assert_eq!(LmntAudioFormat::from_base_format("unknown"), LmntAudioFormat::PcmS16le);
    /// ```
    pub fn from_base_format(format: &str) -> Self {
        match format.to_lowercase().as_str() {
            "linear16" | "pcm" | "pcm16" | "pcm_s16le" => Self::PcmS16le,
            "pcm_f32le" | "f32le" | "float32" => Self::PcmF32le,
            "mp3" | "mpeg" => Self::Mp3,
            "mulaw" | "ulaw" | "g711" => Self::Ulaw,
            "wav" | "wave" => Self::Wav,
            "webm" | "opus" => Self::Webm,
            "aac" | "m4a" => Self::Aac,
            // Default to PCM for lowest latency and no decoding overhead
            _ => Self::PcmS16le,
        }
    }

    /// Returns all available formats.
    pub const fn all() -> &'static [Self] {
        &[
            Self::Mp3,
            Self::PcmS16le,
            Self::PcmF32le,
            Self::Ulaw,
            Self::Webm,
            Self::Aac,
            Self::Wav,
        ]
    }

    /// Returns all streamable formats.
    pub const fn streamable() -> &'static [Self] {
        &[
            Self::Mp3,
            Self::PcmS16le,
            Self::PcmF32le,
            Self::Ulaw,
            Self::Webm,
        ]
    }
}

impl std::fmt::Display for LmntAudioFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =============================================================================
// LMNT TTS Configuration
// =============================================================================

/// LMNT-specific TTS configuration.
///
/// This structure extends the base `TTSConfig` with LMNT-specific parameters
/// for controlling voice synthesis.
///
/// # Parameters
///
/// - **model**: The synthesis model to use (default: "blizzard")
/// - **language**: ISO 639-1 language code or "auto" for detection
/// - **output_format**: Audio format for the response
/// - **sample_rate**: Output sample rate (8000, 16000, or 24000 Hz)
/// - **top_p**: Speech stability control (0-1, default 0.8)
/// - **temperature**: Expressiveness control (≥0, default 1.0)
/// - **speed**: Playback speed (0.25-2.0, default 1.0)
/// - **seed**: Random seed for deterministic output
/// - **debug**: Save output to LMNT clip library
///
/// # Example
///
/// ```rust,ignore
/// use waav_gateway::core::tts::lmnt::{LmntTtsConfig, LmntAudioFormat};
/// use waav_gateway::core::tts::TTSConfig;
///
/// let base = TTSConfig {
///     api_key: "your-api-key".to_string(),
///     voice_id: Some("lily".to_string()),
///     ..Default::default()
/// };
///
/// let config = LmntTtsConfig::from_base(base)
///     .with_language("en")
///     .with_top_p(0.9)
///     .with_temperature(1.2);
/// ```
#[derive(Debug, Clone)]
pub struct LmntTtsConfig {
    /// Base TTS configuration (contains api_key, voice_id, etc.)
    pub base: TTSConfig,

    /// Model name (default: "blizzard")
    pub model: String,

    /// Language code (ISO 639-1) or "auto" for detection
    pub language: String,

    /// Output audio format
    pub output_format: LmntAudioFormat,

    /// Sample rate in Hz (8000, 16000, 24000)
    pub sample_rate: u32,

    /// Speech stability control (0-1, default 0.8)
    /// Lower values produce more varied/expressive speech
    /// Higher values produce more consistent speech
    pub top_p: f32,

    /// Expressiveness control (≥0, default 1.0)
    /// Higher values produce more expressive speech
    pub temperature: f32,

    /// Playback speed multiplier (0.25-2.0, default 1.0)
    pub speed: f32,

    /// Random seed for deterministic output
    pub seed: Option<i64>,

    /// Save output to LMNT clip library for debugging
    pub debug: bool,
}

impl LmntTtsConfig {
    /// Creates a new LMNT TTS config from base TTS config.
    ///
    /// This constructor extracts LMNT-specific settings from the base config
    /// and applies defaults for unspecified values.
    pub fn from_base(base: TTSConfig) -> Self {
        let output_format = base
            .audio_format
            .as_ref()
            .map(|f| LmntAudioFormat::from_base_format(f))
            .unwrap_or_default();

        let sample_rate = base.sample_rate.unwrap_or(DEFAULT_SAMPLE_RATE);

        // Extract speed from speaking_rate if provided
        let speed = base.speaking_rate.unwrap_or(DEFAULT_SPEED);

        Self {
            base,
            model: DEFAULT_MODEL.to_string(),
            language: DEFAULT_LANGUAGE.to_string(),
            output_format,
            sample_rate,
            top_p: DEFAULT_TOP_P,
            temperature: DEFAULT_TEMPERATURE,
            speed,
            seed: None,
            debug: false,
        }
    }

    /// Returns the voice ID to use for synthesis.
    ///
    /// Falls back to "lily" if no voice is specified.
    #[inline]
    pub fn voice_id(&self) -> &str {
        self.base.voice_id.as_deref().unwrap_or("lily")
    }

    /// Sets the model name.
    #[inline]
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Sets the language code.
    #[inline]
    pub fn with_language(mut self, language: impl Into<String>) -> Self {
        self.language = language.into();
        self
    }

    /// Sets the output format.
    #[inline]
    pub fn with_format(mut self, format: LmntAudioFormat) -> Self {
        self.output_format = format;
        self
    }

    /// Sets the sample rate.
    #[inline]
    pub fn with_sample_rate(mut self, sample_rate: u32) -> Self {
        self.sample_rate = sample_rate;
        self
    }

    /// Sets the top_p value (speech stability).
    ///
    /// Value is clamped to [0, 1].
    #[inline]
    pub fn with_top_p(mut self, top_p: f32) -> Self {
        self.top_p = top_p.clamp(MIN_TOP_P, MAX_TOP_P);
        self
    }

    /// Sets the temperature value (expressiveness).
    ///
    /// Value is clamped to [0, ∞).
    #[inline]
    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = temperature.max(MIN_TEMPERATURE);
        self
    }

    /// Sets the speed multiplier.
    ///
    /// Value is clamped to [0.25, 2.0].
    #[inline]
    pub fn with_speed(mut self, speed: f32) -> Self {
        self.speed = speed.clamp(MIN_SPEED, MAX_SPEED);
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

    /// Enables debug mode (saves output to LMNT clip library).
    #[inline]
    pub fn with_debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }

    /// Validates the configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - `top_p` is outside [0, 1]
    /// - `temperature` is negative
    /// - `speed` is outside [0.25, 2.0]
    /// - `sample_rate` is not 8000, 16000, or 24000
    pub fn validate(&self) -> Result<(), String> {
        // Validate top_p
        if self.top_p < MIN_TOP_P || self.top_p > MAX_TOP_P {
            return Err(format!(
                "top_p must be between {} and {}, got {}",
                MIN_TOP_P, MAX_TOP_P, self.top_p
            ));
        }

        // Validate temperature
        if self.temperature < MIN_TEMPERATURE {
            return Err(format!(
                "temperature must be >= {}, got {}",
                MIN_TEMPERATURE, self.temperature
            ));
        }

        // Validate speed
        if self.speed < MIN_SPEED || self.speed > MAX_SPEED {
            return Err(format!(
                "speed must be between {} and {}, got {}",
                MIN_SPEED, MAX_SPEED, self.speed
            ));
        }

        // Validate sample rate
        if !matches!(self.sample_rate, 8000 | 16000 | 24000) {
            return Err(format!(
                "sample_rate must be 8000, 16000, or 24000, got {}",
                self.sample_rate
            ));
        }

        Ok(())
    }

    /// Returns whether this config uses default values for all optional parameters.
    pub fn is_default(&self) -> bool {
        self.model == DEFAULT_MODEL
            && self.language == DEFAULT_LANGUAGE
            && (self.top_p - DEFAULT_TOP_P).abs() < 0.001
            && (self.temperature - DEFAULT_TEMPERATURE).abs() < 0.001
            && (self.speed - DEFAULT_SPEED).abs() < 0.001
            && self.seed.is_none()
            && !self.debug
    }
}

impl Default for LmntTtsConfig {
    fn default() -> Self {
        Self::from_base(TTSConfig::default())
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // LmntAudioFormat Tests
    // =========================================================================

    #[test]
    fn test_audio_format_as_str() {
        assert_eq!(LmntAudioFormat::Mp3.as_str(), "mp3");
        assert_eq!(LmntAudioFormat::PcmS16le.as_str(), "pcm_s16le");
        assert_eq!(LmntAudioFormat::PcmF32le.as_str(), "pcm_f32le");
        assert_eq!(LmntAudioFormat::Ulaw.as_str(), "ulaw");
        assert_eq!(LmntAudioFormat::Webm.as_str(), "webm");
        assert_eq!(LmntAudioFormat::Aac.as_str(), "aac");
        assert_eq!(LmntAudioFormat::Wav.as_str(), "wav");
    }

    #[test]
    fn test_audio_format_content_type() {
        assert_eq!(LmntAudioFormat::Mp3.content_type(), "audio/mpeg");
        assert_eq!(LmntAudioFormat::PcmS16le.content_type(), "audio/pcm");
        assert_eq!(LmntAudioFormat::PcmF32le.content_type(), "audio/pcm");
        assert_eq!(LmntAudioFormat::Ulaw.content_type(), "audio/basic");
        assert_eq!(LmntAudioFormat::Webm.content_type(), "audio/webm");
        assert_eq!(LmntAudioFormat::Aac.content_type(), "audio/aac");
        assert_eq!(LmntAudioFormat::Wav.content_type(), "audio/wav");
    }

    #[test]
    fn test_audio_format_is_streamable() {
        assert!(LmntAudioFormat::Mp3.is_streamable());
        assert!(LmntAudioFormat::PcmS16le.is_streamable());
        assert!(LmntAudioFormat::PcmF32le.is_streamable());
        assert!(LmntAudioFormat::Ulaw.is_streamable());
        assert!(LmntAudioFormat::Webm.is_streamable());
        assert!(!LmntAudioFormat::Aac.is_streamable());
        assert!(!LmntAudioFormat::Wav.is_streamable());
    }

    #[test]
    fn test_audio_format_bytes_per_sample() {
        assert_eq!(LmntAudioFormat::PcmS16le.bytes_per_sample(), Some(2));
        assert_eq!(LmntAudioFormat::PcmF32le.bytes_per_sample(), Some(4));
        assert_eq!(LmntAudioFormat::Ulaw.bytes_per_sample(), Some(1));
        assert_eq!(LmntAudioFormat::Mp3.bytes_per_sample(), None);
        assert_eq!(LmntAudioFormat::Wav.bytes_per_sample(), None);
    }

    #[test]
    fn test_audio_format_from_base_format_linear16() {
        assert_eq!(
            LmntAudioFormat::from_base_format("linear16"),
            LmntAudioFormat::PcmS16le
        );
        assert_eq!(
            LmntAudioFormat::from_base_format("pcm"),
            LmntAudioFormat::PcmS16le
        );
        assert_eq!(
            LmntAudioFormat::from_base_format("pcm16"),
            LmntAudioFormat::PcmS16le
        );
        assert_eq!(
            LmntAudioFormat::from_base_format("PCM_S16LE"),
            LmntAudioFormat::PcmS16le
        );
    }

    #[test]
    fn test_audio_format_from_base_format_mp3() {
        assert_eq!(
            LmntAudioFormat::from_base_format("mp3"),
            LmntAudioFormat::Mp3
        );
        assert_eq!(
            LmntAudioFormat::from_base_format("MP3"),
            LmntAudioFormat::Mp3
        );
        assert_eq!(
            LmntAudioFormat::from_base_format("mpeg"),
            LmntAudioFormat::Mp3
        );
    }

    #[test]
    fn test_audio_format_from_base_format_ulaw() {
        assert_eq!(
            LmntAudioFormat::from_base_format("mulaw"),
            LmntAudioFormat::Ulaw
        );
        assert_eq!(
            LmntAudioFormat::from_base_format("ulaw"),
            LmntAudioFormat::Ulaw
        );
        assert_eq!(
            LmntAudioFormat::from_base_format("g711"),
            LmntAudioFormat::Ulaw
        );
    }

    #[test]
    fn test_audio_format_from_base_format_other() {
        assert_eq!(
            LmntAudioFormat::from_base_format("wav"),
            LmntAudioFormat::Wav
        );
        assert_eq!(
            LmntAudioFormat::from_base_format("webm"),
            LmntAudioFormat::Webm
        );
        assert_eq!(
            LmntAudioFormat::from_base_format("aac"),
            LmntAudioFormat::Aac
        );
    }

    #[test]
    fn test_audio_format_from_base_format_unknown_defaults_to_pcm() {
        assert_eq!(
            LmntAudioFormat::from_base_format("unknown"),
            LmntAudioFormat::PcmS16le
        );
        assert_eq!(
            LmntAudioFormat::from_base_format(""),
            LmntAudioFormat::PcmS16le
        );
    }

    #[test]
    fn test_audio_format_default() {
        assert_eq!(LmntAudioFormat::default(), LmntAudioFormat::Mp3);
    }

    #[test]
    fn test_audio_format_display() {
        assert_eq!(format!("{}", LmntAudioFormat::Mp3), "mp3");
        assert_eq!(format!("{}", LmntAudioFormat::PcmS16le), "pcm_s16le");
    }

    #[test]
    fn test_audio_format_all() {
        let all = LmntAudioFormat::all();
        assert_eq!(all.len(), 7);
        assert!(all.contains(&LmntAudioFormat::Mp3));
        assert!(all.contains(&LmntAudioFormat::PcmS16le));
    }

    #[test]
    fn test_audio_format_streamable() {
        let streamable = LmntAudioFormat::streamable();
        assert_eq!(streamable.len(), 5);
        assert!(streamable.contains(&LmntAudioFormat::Mp3));
        assert!(streamable.contains(&LmntAudioFormat::PcmS16le));
        assert!(!streamable.contains(&LmntAudioFormat::Wav));
        assert!(!streamable.contains(&LmntAudioFormat::Aac));
    }

    // =========================================================================
    // LmntTtsConfig Tests
    // =========================================================================

    fn create_test_base_config() -> TTSConfig {
        TTSConfig {
            provider: "lmnt".to_string(),
            api_key: "test-api-key".to_string(),
            voice_id: Some("lily".to_string()),
            model: String::new(),
            speaking_rate: None,
            audio_format: Some("linear16".to_string()),
            sample_rate: Some(24000),
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
        let config = LmntTtsConfig::from_base(base);

        assert_eq!(config.voice_id(), "lily");
        assert_eq!(config.model, DEFAULT_MODEL);
        assert_eq!(config.language, DEFAULT_LANGUAGE);
        assert_eq!(config.output_format, LmntAudioFormat::PcmS16le);
        assert_eq!(config.sample_rate, 24000);
        assert!((config.top_p - DEFAULT_TOP_P).abs() < 0.001);
        assert!((config.temperature - DEFAULT_TEMPERATURE).abs() < 0.001);
        assert!((config.speed - DEFAULT_SPEED).abs() < 0.001);
        assert!(config.seed.is_none());
        assert!(!config.debug);
    }

    #[test]
    fn test_config_default_voice_id() {
        let mut base = create_test_base_config();
        base.voice_id = None;
        let config = LmntTtsConfig::from_base(base);

        assert_eq!(config.voice_id(), "lily");
    }

    #[test]
    fn test_config_with_model() {
        let base = create_test_base_config();
        let config = LmntTtsConfig::from_base(base).with_model("custom-model");

        assert_eq!(config.model, "custom-model");
    }

    #[test]
    fn test_config_with_language() {
        let base = create_test_base_config();
        let config = LmntTtsConfig::from_base(base).with_language("en");

        assert_eq!(config.language, "en");
    }

    #[test]
    fn test_config_with_format() {
        let base = create_test_base_config();
        let config = LmntTtsConfig::from_base(base).with_format(LmntAudioFormat::Mp3);

        assert_eq!(config.output_format, LmntAudioFormat::Mp3);
    }

    #[test]
    fn test_config_with_sample_rate() {
        let base = create_test_base_config();
        let config = LmntTtsConfig::from_base(base).with_sample_rate(16000);

        assert_eq!(config.sample_rate, 16000);
    }

    #[test]
    fn test_config_with_top_p() {
        let base = create_test_base_config();
        let config = LmntTtsConfig::from_base(base).with_top_p(0.9);

        assert!((config.top_p - 0.9).abs() < 0.001);
    }

    #[test]
    fn test_config_with_top_p_clamped() {
        let base = create_test_base_config();

        // Test clamping to max
        let config = LmntTtsConfig::from_base(base.clone()).with_top_p(1.5);
        assert!((config.top_p - MAX_TOP_P).abs() < 0.001);

        // Test clamping to min
        let config = LmntTtsConfig::from_base(base).with_top_p(-0.5);
        assert!((config.top_p - MIN_TOP_P).abs() < 0.001);
    }

    #[test]
    fn test_config_with_temperature() {
        let base = create_test_base_config();
        let config = LmntTtsConfig::from_base(base).with_temperature(1.5);

        assert!((config.temperature - 1.5).abs() < 0.001);
    }

    #[test]
    fn test_config_with_temperature_clamped() {
        let base = create_test_base_config();
        let config = LmntTtsConfig::from_base(base).with_temperature(-0.5);

        assert!((config.temperature - MIN_TEMPERATURE).abs() < 0.001);
    }

    #[test]
    fn test_config_with_speed() {
        let base = create_test_base_config();
        let config = LmntTtsConfig::from_base(base).with_speed(1.5);

        assert!((config.speed - 1.5).abs() < 0.001);
    }

    #[test]
    fn test_config_with_speed_clamped() {
        let base = create_test_base_config();

        // Test clamping to max
        let config = LmntTtsConfig::from_base(base.clone()).with_speed(3.0);
        assert!((config.speed - MAX_SPEED).abs() < 0.001);

        // Test clamping to min
        let config = LmntTtsConfig::from_base(base).with_speed(0.1);
        assert!((config.speed - MIN_SPEED).abs() < 0.001);
    }

    #[test]
    fn test_config_with_seed() {
        let base = create_test_base_config();
        let config = LmntTtsConfig::from_base(base).with_seed(12345);

        assert_eq!(config.seed, Some(12345));
    }

    #[test]
    fn test_config_without_seed() {
        let base = create_test_base_config();
        let config = LmntTtsConfig::from_base(base)
            .with_seed(12345)
            .without_seed();

        assert!(config.seed.is_none());
    }

    #[test]
    fn test_config_with_debug() {
        let base = create_test_base_config();
        let config = LmntTtsConfig::from_base(base).with_debug(true);

        assert!(config.debug);
    }

    #[test]
    fn test_config_validate_success() {
        let base = create_test_base_config();
        let config = LmntTtsConfig::from_base(base);

        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validate_invalid_sample_rate() {
        let mut base = create_test_base_config();
        base.sample_rate = Some(44100);
        let config = LmntTtsConfig::from_base(base);

        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("sample_rate"));
    }

    #[test]
    fn test_config_is_default() {
        let base = create_test_base_config();
        let config = LmntTtsConfig::from_base(base);

        assert!(config.is_default());
    }

    #[test]
    fn test_config_is_not_default_with_language() {
        let base = create_test_base_config();
        let config = LmntTtsConfig::from_base(base).with_language("en");

        assert!(!config.is_default());
    }

    #[test]
    fn test_config_is_not_default_with_top_p() {
        let base = create_test_base_config();
        let config = LmntTtsConfig::from_base(base).with_top_p(0.5);

        assert!(!config.is_default());
    }

    #[test]
    fn test_config_is_not_default_with_seed() {
        let base = create_test_base_config();
        let config = LmntTtsConfig::from_base(base).with_seed(123);

        assert!(!config.is_default());
    }

    #[test]
    fn test_config_is_not_default_with_debug() {
        let base = create_test_base_config();
        let config = LmntTtsConfig::from_base(base).with_debug(true);

        assert!(!config.is_default());
    }

    #[test]
    fn test_config_default() {
        let config = LmntTtsConfig::default();

        assert_eq!(config.model, DEFAULT_MODEL);
        assert_eq!(config.language, DEFAULT_LANGUAGE);
        assert!(config.is_default());
    }

    #[test]
    fn test_config_extracts_speed_from_speaking_rate() {
        let mut base = create_test_base_config();
        base.speaking_rate = Some(1.5);
        let config = LmntTtsConfig::from_base(base);

        assert!((config.speed - 1.5).abs() < 0.001);
    }
}
