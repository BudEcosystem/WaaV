//! Configuration types for Groq STT (Whisper) API.
//!
//! This module contains all configuration-related types including:
//! - Model selection (whisper-large-v3, whisper-large-v3-turbo)
//! - Response format options
//! - Audio format specifications
//! - Provider-specific configuration options
//!
//! # Groq Whisper API Overview
//!
//! Groq provides the fastest Whisper inference available, with speeds up to 216x real-time.
//! The API is OpenAI-compatible, using REST endpoints for audio transcription.
//!
//! ## Models
//!
//! | Model | WER | Speed | Cost/Hour |
//! |-------|-----|-------|-----------|
//! | whisper-large-v3 | 10.3% | 189x | $0.111 |
//! | whisper-large-v3-turbo | 12% | 216x | $0.04 |
//!
//! ## File Limits
//!
//! - Free tier: 25MB max
//! - Dev tier: 100MB max
//! - Can use `url` parameter for larger files

use super::super::base::STTConfig;
use serde::{Deserialize, Serialize};

// =============================================================================
// Constants
// =============================================================================

/// Groq API base URL for audio transcriptions.
pub const GROQ_STT_URL: &str = "https://api.groq.com/openai/v1/audio/transcriptions";

/// Groq API base URL for audio translations (to English).
pub const GROQ_TRANSLATION_URL: &str = "https://api.groq.com/openai/v1/audio/translations";

/// Default maximum file size in bytes (25MB for free tier).
pub const DEFAULT_MAX_FILE_SIZE: usize = 25 * 1024 * 1024;

/// Maximum file size for dev tier (100MB).
pub const DEV_TIER_MAX_FILE_SIZE: usize = 100 * 1024 * 1024;

/// Default model to use.
pub const DEFAULT_MODEL: &str = "whisper-large-v3-turbo";

/// Maximum prompt length in tokens.
pub const MAX_PROMPT_TOKENS: usize = 224;

/// Minimum audio length in seconds (below this, 10 second minimum is billed).
pub const MIN_BILLED_DURATION_SECONDS: f32 = 10.0;

// =============================================================================
// Groq STT Models
// =============================================================================

/// Supported Groq STT (Whisper) models.
///
/// Groq offers two Whisper models:
/// - `whisper-large-v3`: Higher accuracy (10.3% WER), slightly slower
/// - `whisper-large-v3-turbo`: Faster, cost-effective (12% WER), 216x real-time
///
/// # Model Selection Guide
///
/// - Use `WhisperLargeV3` for error-sensitive applications requiring best accuracy
/// - Use `WhisperLargeV3Turbo` for best price/performance with multilingual support
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GroqSTTModel {
    /// Whisper Large V3 - highest accuracy (10.3% WER), 189x real-time
    #[serde(rename = "whisper-large-v3")]
    WhisperLargeV3,

    /// Whisper Large V3 Turbo - fastest, cost-effective (12% WER), 216x real-time
    #[default]
    #[serde(rename = "whisper-large-v3-turbo")]
    WhisperLargeV3Turbo,
}

impl GroqSTTModel {
    /// Convert to the API parameter value.
    #[inline]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::WhisperLargeV3 => "whisper-large-v3",
            Self::WhisperLargeV3Turbo => "whisper-large-v3-turbo",
        }
    }

    /// Parse from string, with fallback to default.
    pub fn from_str_or_default(s: &str) -> Self {
        match s.to_lowercase().replace('_', "-").as_str() {
            "whisper-large-v3" | "whisper-v3" | "large-v3" => Self::WhisperLargeV3,
            "whisper-large-v3-turbo" | "whisper-v3-turbo" | "turbo" | "large-v3-turbo" => {
                Self::WhisperLargeV3Turbo
            }
            _ => Self::default(),
        }
    }

    /// Get the word error rate (WER) for this model.
    pub fn word_error_rate(&self) -> f32 {
        match self {
            Self::WhisperLargeV3 => 0.103,      // 10.3%
            Self::WhisperLargeV3Turbo => 0.12,  // 12%
        }
    }

    /// Get the speed factor (how many times faster than real-time).
    pub fn speed_factor(&self) -> u32 {
        match self {
            Self::WhisperLargeV3 => 189,
            Self::WhisperLargeV3Turbo => 216,
        }
    }

    /// Get the cost per hour in USD.
    ///
    /// Uses centralized pricing from `crate::config::pricing`.
    /// Falls back to hardcoded values if not found in pricing database.
    pub fn cost_per_hour(&self) -> f64 {
        crate::config::get_stt_price_per_hour("groq", self.as_str()).unwrap_or_else(|| {
            // Fallback to hardcoded values (should not happen if pricing db is up to date)
            match self {
                Self::WhisperLargeV3 => 0.111,
                Self::WhisperLargeV3Turbo => 0.04,
            }
        })
    }
}

impl std::fmt::Display for GroqSTTModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =============================================================================
// Response Format
// =============================================================================

/// Response format for transcription results.
///
/// Controls the output format returned by the API:
/// - `json`: Simple JSON with transcript text
/// - `text`: Plain text transcript
/// - `verbose_json`: JSON with word-level timestamps and metadata
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroqResponseFormat {
    /// Simple JSON response with transcript text
    #[default]
    Json,
    /// Plain text transcript
    Text,
    /// Verbose JSON with word timestamps and metadata
    VerboseJson,
}

impl GroqResponseFormat {
    /// Convert to the API parameter value.
    #[inline]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Text => "text",
            Self::VerboseJson => "verbose_json",
        }
    }
}

impl std::fmt::Display for GroqResponseFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =============================================================================
// Timestamp Granularities
// =============================================================================

/// Timestamp granularity options for verbose_json format.
///
/// When using `verbose_json` response format, you can request timestamps
/// at different granularities:
/// - `Word`: Timing for each word (adds latency)
/// - `Segment`: Timing for each segment/sentence
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TimestampGranularity {
    /// Word-level timestamps (adds latency)
    Word,
    /// Segment-level timestamps
    Segment,
}

impl TimestampGranularity {
    /// Convert to the API parameter value.
    #[inline]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Word => "word",
            Self::Segment => "segment",
        }
    }
}

// =============================================================================
// Audio Input Format
// =============================================================================

/// Supported audio input formats for Groq Whisper API.
///
/// The Whisper API supports various audio formats. Audio is downsampled
/// to 16kHz mono before processing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioInputFormat {
    /// WAV format (PCM audio) - recommended for WaaV Gateway
    #[default]
    Wav,
    /// FLAC format (lossless compression)
    Flac,
    /// MP3 format
    Mp3,
    /// MP4 format
    Mp4,
    /// MPEG format
    Mpeg,
    /// MPGA format
    Mpga,
    /// M4A format (Apple audio)
    M4a,
    /// OGG format
    Ogg,
    /// WebM format
    Webm,
}

impl AudioInputFormat {
    /// Get the MIME type for this format.
    #[inline]
    pub fn mime_type(&self) -> &'static str {
        match self {
            Self::Wav => "audio/wav",
            Self::Flac => "audio/flac",
            Self::Mp3 => "audio/mpeg",
            Self::Mp4 => "audio/mp4",
            Self::Mpeg => "audio/mpeg",
            Self::Mpga => "audio/mpeg",
            Self::M4a => "audio/m4a",
            Self::Ogg => "audio/ogg",
            Self::Webm => "audio/webm",
        }
    }

    /// Get the file extension for this format.
    #[inline]
    pub fn extension(&self) -> &'static str {
        match self {
            Self::Wav => "wav",
            Self::Flac => "flac",
            Self::Mp3 => "mp3",
            Self::Mp4 => "mp4",
            Self::Mpeg => "mpeg",
            Self::Mpga => "mpga",
            Self::M4a => "m4a",
            Self::Ogg => "ogg",
            Self::Webm => "webm",
        }
    }
}

// =============================================================================
// Flush Strategy
// =============================================================================

/// Strategy for when to send buffered audio to the API.
///
/// Since Groq Whisper is a REST API (not streaming WebSocket),
/// we need to buffer audio and send it in batches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlushStrategy {
    /// Only send on explicit disconnect - accumulate all audio first.
    /// Best for pre-recorded audio or when you want full context.
    #[default]
    OnDisconnect,

    /// Send when buffer reaches a size threshold (in bytes).
    /// Provides faster partial results for long recordings.
    OnThreshold,

    /// Send on silence detection (requires silence detection logic).
    /// Provides natural sentence-level transcription.
    OnSilence,
}

// =============================================================================
// Silence Detection Configuration
// =============================================================================

/// Configuration for silence detection.
///
/// Used to detect when a speaker has stopped talking, triggering
/// a flush of the audio buffer for transcription.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SilenceDetectionConfig {
    /// RMS energy threshold below which audio is considered silent.
    /// Valid range: 0.0 to 1.0 (relative to normalized audio -1.0 to 1.0).
    /// Default is 0.01.
    pub rms_threshold: f32,

    /// Minimum duration of silence (in milliseconds) before triggering flush.
    /// Valid range: 100 to 60000 (0.1 to 60 seconds).
    /// Default is 1000ms (1 second).
    pub silence_duration_ms: u32,

    /// Minimum audio duration (in milliseconds) before silence detection is active.
    /// Prevents premature flush for initial silence.
    /// Valid range: 0 to 30000 (0 to 30 seconds).
    /// Default is 500ms.
    pub min_audio_duration_ms: u32,
}

impl Default for SilenceDetectionConfig {
    fn default() -> Self {
        Self {
            rms_threshold: 0.01,
            silence_duration_ms: 1000,
            min_audio_duration_ms: 500,
        }
    }
}

impl SilenceDetectionConfig {
    /// Validate the silence detection configuration.
    pub fn validate(&self) -> Result<(), String> {
        if !(0.0..=1.0).contains(&self.rms_threshold) {
            return Err(format!(
                "RMS threshold must be between 0.0 and 1.0, got {}",
                self.rms_threshold
            ));
        }

        if !(100..=60000).contains(&self.silence_duration_ms) {
            return Err(format!(
                "Silence duration must be between 100ms and 60000ms, got {}ms",
                self.silence_duration_ms
            ));
        }

        if self.min_audio_duration_ms > 30000 {
            return Err(format!(
                "Minimum audio duration must be at most 30000ms, got {}ms",
                self.min_audio_duration_ms
            ));
        }

        Ok(())
    }
}

// =============================================================================
// Main Configuration
// =============================================================================

/// Configuration specific to Groq STT (Whisper) API.
///
/// This configuration extends the base `STTConfig` with Groq-specific
/// parameters for the REST transcription API.
///
/// # Example
///
/// ```rust,ignore
/// use waav_gateway::core::stt::groq::{GroqSTTConfig, GroqSTTModel, GroqResponseFormat};
/// use waav_gateway::core::stt::STTConfig;
///
/// let config = GroqSTTConfig {
///     base: STTConfig {
///         api_key: "gsk_...".to_string(),
///         language: "en".to_string(),
///         sample_rate: 16000,
///         ..Default::default()
///     },
///     model: GroqSTTModel::WhisperLargeV3Turbo,
///     response_format: GroqResponseFormat::VerboseJson,
///     temperature: Some(0.0),
///     ..Default::default()
/// };
/// ```
#[derive(Debug, Clone)]
pub struct GroqSTTConfig {
    /// Base STT configuration (shared across all providers).
    pub base: STTConfig,

    /// Groq STT model to use.
    pub model: GroqSTTModel,

    /// Response format for transcription results.
    pub response_format: GroqResponseFormat,

    /// Temperature for sampling (0.0 to 1.0).
    ///
    /// Lower values make output more deterministic.
    /// Higher values make output more creative/varied.
    /// Default is 0.0 for most deterministic results.
    pub temperature: Option<f32>,

    /// Timestamp granularities to include in verbose_json output.
    ///
    /// Only applicable when `response_format` is `VerboseJson`.
    /// Note: Word timestamps add latency.
    pub timestamp_granularities: Vec<TimestampGranularity>,

    /// Audio input format for the API request.
    ///
    /// The gateway will package PCM audio into this format before sending.
    pub audio_input_format: AudioInputFormat,

    /// Optional text prompt to guide the transcription.
    ///
    /// Useful for providing context about the audio content,
    /// spelling of specific terms, or desired formatting.
    /// Maximum 224 tokens.
    pub prompt: Option<String>,

    /// Flush strategy for sending buffered audio.
    pub flush_strategy: FlushStrategy,

    /// Buffer threshold in bytes for OnThreshold flush strategy.
    ///
    /// When the buffer reaches this size, audio is sent to the API.
    /// Default is 1MB (approximately 32 seconds at 16kHz 16-bit mono).
    pub flush_threshold_bytes: usize,

    /// Maximum file size in bytes.
    /// Free tier: 25MB, Dev tier: 100MB
    pub max_file_size_bytes: usize,

    /// Silence detection configuration for OnSilence flush strategy.
    pub silence_detection: SilenceDetectionConfig,

    /// Whether to use the translation endpoint instead of transcription.
    /// Translation always outputs English text regardless of input language.
    pub translate_to_english: bool,

    /// Custom API endpoint URL (for enterprise or custom deployments).
    pub custom_endpoint: Option<String>,
}

impl Default for GroqSTTConfig {
    fn default() -> Self {
        Self {
            base: STTConfig::default(),
            model: GroqSTTModel::default(),
            response_format: GroqResponseFormat::VerboseJson, // For word timestamps
            temperature: Some(0.0),                           // Deterministic results
            timestamp_granularities: vec![TimestampGranularity::Segment],
            audio_input_format: AudioInputFormat::Wav,
            prompt: None,
            flush_strategy: FlushStrategy::OnDisconnect,
            flush_threshold_bytes: 1024 * 1024,           // 1MB
            max_file_size_bytes: DEFAULT_MAX_FILE_SIZE,   // 25MB
            silence_detection: SilenceDetectionConfig::default(),
            translate_to_english: false,
            custom_endpoint: None,
        }
    }
}

impl GroqSTTConfig {
    /// Create a new configuration from base STTConfig.
    ///
    /// Automatically determines the model from the config if specified.
    pub fn from_base(base: STTConfig) -> Self {
        let model = if base.model.is_empty() {
            GroqSTTModel::default()
        } else {
            GroqSTTModel::from_str_or_default(&base.model)
        };

        Self {
            base,
            model,
            ..Default::default()
        }
    }

    /// Get the API endpoint URL.
    #[inline]
    pub fn api_url(&self) -> &str {
        if let Some(ref custom) = self.custom_endpoint {
            return custom;
        }

        if self.translate_to_english {
            GROQ_TRANSLATION_URL
        } else {
            GROQ_STT_URL
        }
    }

    /// Validate the configuration.
    pub fn validate(&self) -> Result<(), String> {
        if self.base.api_key.is_empty() {
            return Err("API key is required".to_string());
        }

        if let Some(temp) = self.temperature
            && !(0.0..=1.0).contains(&temp)
        {
            return Err(format!(
                "Temperature must be between 0.0 and 1.0, got {}",
                temp
            ));
        }

        if self.flush_threshold_bytes > self.max_file_size_bytes {
            return Err(format!(
                "Flush threshold ({}) cannot exceed max file size ({})",
                self.flush_threshold_bytes, self.max_file_size_bytes
            ));
        }

        if let Some(ref prompt) = self.prompt {
            // Token estimation is language-dependent:
            // - English: ~4 chars per token (words avg 4-5 chars + spaces)
            // - CJK (Chinese, Japanese, Korean): ~1-2 chars per token
            // - Other languages vary
            //
            // We use a conservative estimate of 2 chars per token to avoid
            // rejecting valid prompts in non-English languages.
            // The API will return a proper error if the prompt is actually too long.
            let estimated_tokens = prompt.len().div_ceil(2); // Conservative estimate
            if estimated_tokens > MAX_PROMPT_TOKENS {
                return Err(format!(
                    "Prompt likely too long: estimated ~{} tokens (at 2 chars/token), max is {}. \
                     Note: Actual token count varies by language.",
                    estimated_tokens, MAX_PROMPT_TOKENS
                ));
            }
        }

        // Validate silence detection config
        self.silence_detection.validate()?;

        Ok(())
    }

    /// Set the model to use for transcription.
    pub fn with_model(mut self, model: GroqSTTModel) -> Self {
        self.model = model;
        self
    }

    /// Set the response format.
    pub fn with_response_format(mut self, format: GroqResponseFormat) -> Self {
        self.response_format = format;
        self
    }

    /// Enable translation to English.
    pub fn with_translation(mut self) -> Self {
        self.translate_to_english = true;
        self
    }

    /// Set dev tier file size limit (100MB).
    pub fn with_dev_tier(mut self) -> Self {
        self.max_file_size_bytes = DEV_TIER_MAX_FILE_SIZE;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_groq_stt_model_as_str() {
        assert_eq!(GroqSTTModel::WhisperLargeV3.as_str(), "whisper-large-v3");
        assert_eq!(
            GroqSTTModel::WhisperLargeV3Turbo.as_str(),
            "whisper-large-v3-turbo"
        );
    }

    #[test]
    fn test_groq_stt_model_from_str() {
        assert_eq!(
            GroqSTTModel::from_str_or_default("whisper-large-v3"),
            GroqSTTModel::WhisperLargeV3
        );
        assert_eq!(
            GroqSTTModel::from_str_or_default("whisper-large-v3-turbo"),
            GroqSTTModel::WhisperLargeV3Turbo
        );
        assert_eq!(
            GroqSTTModel::from_str_or_default("turbo"),
            GroqSTTModel::WhisperLargeV3Turbo
        );
        assert_eq!(
            GroqSTTModel::from_str_or_default("unknown"),
            GroqSTTModel::default()
        );
    }

    #[test]
    fn test_groq_stt_model_metrics() {
        let turbo = GroqSTTModel::WhisperLargeV3Turbo;
        assert_eq!(turbo.speed_factor(), 216);
        assert!((turbo.cost_per_hour() - 0.04).abs() < f64::EPSILON);
        assert!((turbo.word_error_rate() - 0.12).abs() < f32::EPSILON);

        let v3 = GroqSTTModel::WhisperLargeV3;
        assert_eq!(v3.speed_factor(), 189);
        assert!((v3.cost_per_hour() - 0.111).abs() < f64::EPSILON);
        assert!((v3.word_error_rate() - 0.103).abs() < f32::EPSILON);
    }

    #[test]
    fn test_response_format_as_str() {
        assert_eq!(GroqResponseFormat::Json.as_str(), "json");
        assert_eq!(GroqResponseFormat::Text.as_str(), "text");
        assert_eq!(GroqResponseFormat::VerboseJson.as_str(), "verbose_json");
    }

    #[test]
    fn test_audio_format_mime_type() {
        assert_eq!(AudioInputFormat::Wav.mime_type(), "audio/wav");
        assert_eq!(AudioInputFormat::Flac.mime_type(), "audio/flac");
        assert_eq!(AudioInputFormat::Mp3.mime_type(), "audio/mpeg");
        assert_eq!(AudioInputFormat::Ogg.mime_type(), "audio/ogg");
        assert_eq!(AudioInputFormat::Webm.mime_type(), "audio/webm");
    }

    #[test]
    fn test_audio_format_extension() {
        assert_eq!(AudioInputFormat::Wav.extension(), "wav");
        assert_eq!(AudioInputFormat::Flac.extension(), "flac");
        assert_eq!(AudioInputFormat::Ogg.extension(), "ogg");
    }

    #[test]
    fn test_config_validation_empty_api_key() {
        let config = GroqSTTConfig {
            base: STTConfig {
                api_key: String::new(),
                ..Default::default()
            },
            ..Default::default()
        };

        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("API key"));
    }

    #[test]
    fn test_config_validation_invalid_temperature() {
        let config = GroqSTTConfig {
            base: STTConfig {
                api_key: "test_key".to_string(),
                ..Default::default()
            },
            temperature: Some(1.5),
            ..Default::default()
        };

        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Temperature"));
    }

    #[test]
    fn test_config_validation_threshold_exceeds_max() {
        let config = GroqSTTConfig {
            base: STTConfig {
                api_key: "test_key".to_string(),
                ..Default::default()
            },
            flush_threshold_bytes: 30 * 1024 * 1024, // 30MB
            max_file_size_bytes: 25 * 1024 * 1024,   // 25MB
            ..Default::default()
        };

        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("exceed max file size"));
    }

    #[test]
    fn test_config_validation_valid() {
        let config = GroqSTTConfig {
            base: STTConfig {
                api_key: "test_key".to_string(),
                ..Default::default()
            },
            temperature: Some(0.5),
            ..Default::default()
        };

        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_from_base() {
        let base = STTConfig {
            api_key: "test_key".to_string(),
            model: "whisper-large-v3".to_string(),
            language: "en".to_string(),
            sample_rate: 16000,
            ..Default::default()
        };

        let config = GroqSTTConfig::from_base(base);
        assert_eq!(config.model, GroqSTTModel::WhisperLargeV3);
    }

    #[test]
    fn test_default_config() {
        let config = GroqSTTConfig::default();
        assert_eq!(config.model, GroqSTTModel::WhisperLargeV3Turbo);
        assert_eq!(config.response_format, GroqResponseFormat::VerboseJson);
        assert_eq!(config.temperature, Some(0.0));
        assert_eq!(config.flush_threshold_bytes, 1024 * 1024);
        assert_eq!(config.max_file_size_bytes, DEFAULT_MAX_FILE_SIZE);
        assert!(!config.translate_to_english);
    }

    #[test]
    fn test_api_url() {
        let mut config = GroqSTTConfig::default();
        assert_eq!(config.api_url(), GROQ_STT_URL);

        config.translate_to_english = true;
        assert_eq!(config.api_url(), GROQ_TRANSLATION_URL);

        config.custom_endpoint = Some("https://custom.endpoint.com".to_string());
        assert_eq!(config.api_url(), "https://custom.endpoint.com");
    }

    #[test]
    fn test_config_builder_methods() {
        let config = GroqSTTConfig::default()
            .with_model(GroqSTTModel::WhisperLargeV3)
            .with_response_format(GroqResponseFormat::Text)
            .with_translation()
            .with_dev_tier();

        assert_eq!(config.model, GroqSTTModel::WhisperLargeV3);
        assert_eq!(config.response_format, GroqResponseFormat::Text);
        assert!(config.translate_to_english);
        assert_eq!(config.max_file_size_bytes, DEV_TIER_MAX_FILE_SIZE);
    }

    #[test]
    fn test_constants() {
        assert_eq!(GROQ_STT_URL, "https://api.groq.com/openai/v1/audio/transcriptions");
        assert_eq!(GROQ_TRANSLATION_URL, "https://api.groq.com/openai/v1/audio/translations");
        assert_eq!(DEFAULT_MAX_FILE_SIZE, 25 * 1024 * 1024);
        assert_eq!(DEV_TIER_MAX_FILE_SIZE, 100 * 1024 * 1024);
        assert_eq!(MAX_PROMPT_TOKENS, 224);
    }
}
