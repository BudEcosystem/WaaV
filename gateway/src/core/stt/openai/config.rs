//! Configuration types for OpenAI STT (Whisper) API.
//!
//! This module contains all configuration-related types including:
//! - Model selection (whisper-1, gpt-4o-transcribe, gpt-4o-mini-transcribe)
//! - Response format options
//! - Audio format specifications
//! - Provider-specific configuration options

use super::super::base::STTConfig;
use serde::{Deserialize, Serialize};

// =============================================================================
// OpenAI STT Models
// =============================================================================

/// Supported OpenAI STT models.
///
/// OpenAI offers several transcription models with different capabilities:
/// - `whisper-1`: Original Whisper model, good balance of speed and accuracy
/// - `gpt-4o-transcribe`: Enhanced transcription with GPT-4o intelligence
/// - `gpt-4o-mini-transcribe`: Faster, cost-effective transcription
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OpenAISTTModel {
    /// Original Whisper model - good balance of speed and accuracy
    #[default]
    #[serde(rename = "whisper-1")]
    Whisper1,
    /// GPT-4o enhanced transcription - best accuracy
    #[serde(rename = "gpt-4o-transcribe")]
    Gpt4oTranscribe,
    /// GPT-4o mini transcription - faster, cost-effective
    #[serde(rename = "gpt-4o-mini-transcribe")]
    Gpt4oMiniTranscribe,
}

impl OpenAISTTModel {
    /// Convert to the API parameter value.
    #[inline]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Whisper1 => "whisper-1",
            Self::Gpt4oTranscribe => "gpt-4o-transcribe",
            Self::Gpt4oMiniTranscribe => "gpt-4o-mini-transcribe",
        }
    }

    /// Parse from string, with fallback to default.
    pub fn from_str_or_default(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "whisper-1" | "whisper1" | "whisper" => Self::Whisper1,
            "gpt-4o-transcribe" | "gpt4o-transcribe" => Self::Gpt4oTranscribe,
            "gpt-4o-mini-transcribe" | "gpt4o-mini-transcribe" => Self::Gpt4oMiniTranscribe,
            _ => Self::default(),
        }
    }
}

impl std::fmt::Display for OpenAISTTModel {
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
/// - `srt`: SubRip subtitle format
/// - `vtt`: WebVTT subtitle format
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseFormat {
    /// Simple JSON response with transcript text
    #[default]
    Json,
    /// Plain text transcript
    Text,
    /// Verbose JSON with word timestamps and metadata
    VerboseJson,
    /// SubRip subtitle format
    Srt,
    /// WebVTT subtitle format
    Vtt,
}

impl ResponseFormat {
    /// Convert to the API parameter value.
    #[inline]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Text => "text",
            Self::VerboseJson => "verbose_json",
            Self::Srt => "srt",
            Self::Vtt => "vtt",
        }
    }
}

impl std::fmt::Display for ResponseFormat {
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
/// - `Word`: Timing for each word
/// - `Segment`: Timing for each segment/sentence
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TimestampGranularity {
    /// Word-level timestamps
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

/// Supported audio input formats for OpenAI Whisper API.
///
/// The Whisper API supports various audio formats. Files must be < 25MB.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioInputFormat {
    /// WAV format (PCM audio) - recommended for WaaV Gateway
    #[default]
    Wav,
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
    /// WebM format
    Webm,
}

impl AudioInputFormat {
    /// Get the MIME type for this format.
    #[inline]
    pub fn mime_type(&self) -> &'static str {
        match self {
            Self::Wav => "audio/wav",
            Self::Mp3 => "audio/mpeg",
            Self::Mp4 => "audio/mp4",
            Self::Mpeg => "audio/mpeg",
            Self::Mpga => "audio/mpeg",
            Self::M4a => "audio/m4a",
            Self::Webm => "audio/webm",
        }
    }

    /// Get the file extension for this format.
    #[inline]
    pub fn extension(&self) -> &'static str {
        match self {
            Self::Wav => "wav",
            Self::Mp3 => "mp3",
            Self::Mp4 => "mp4",
            Self::Mpeg => "mpeg",
            Self::Mpga => "mpga",
            Self::M4a => "m4a",
            Self::Webm => "webm",
        }
    }
}

// =============================================================================
// Flush Strategy
// =============================================================================

/// Strategy for when to send buffered audio to the API.
///
/// Since OpenAI Whisper is a REST API (not streaming WebSocket),
/// we need to buffer audio and send it in batches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
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
// Main Configuration
// =============================================================================

/// Configuration specific to OpenAI STT (Whisper) API.
///
/// This configuration extends the base `STTConfig` with OpenAI-specific
/// parameters for the REST transcription API.
///
/// # Example
///
/// ```rust
/// use waav_gateway::core::stt::openai::{OpenAISTTConfig, OpenAISTTModel, ResponseFormat};
/// use waav_gateway::core::stt::STTConfig;
///
/// let config = OpenAISTTConfig {
///     base: STTConfig {
///         api_key: "sk-...".to_string(),
///         language: "en".to_string(),
///         sample_rate: 16000,
///         ..Default::default()
///     },
///     model: OpenAISTTModel::Whisper1,
///     response_format: ResponseFormat::VerboseJson,
///     temperature: Some(0.0),
///     ..Default::default()
/// };
/// ```
#[derive(Debug, Clone)]
pub struct OpenAISTTConfig {
    /// Base STT configuration (shared across all providers).
    pub base: STTConfig,

    /// OpenAI STT model to use.
    pub model: OpenAISTTModel,

    /// Response format for transcription results.
    pub response_format: ResponseFormat,

    /// Temperature for sampling (0.0 to 1.0).
    ///
    /// Lower values make output more deterministic.
    /// Higher values make output more creative/varied.
    /// Default is 0.0 for most deterministic results.
    pub temperature: Option<f32>,

    /// Timestamp granularities to include in verbose_json output.
    ///
    /// Only applicable when `response_format` is `VerboseJson`.
    pub timestamp_granularities: Vec<TimestampGranularity>,

    /// Audio input format for the API request.
    ///
    /// The gateway will package PCM audio into this format before sending.
    pub audio_input_format: AudioInputFormat,

    /// Optional text prompt to guide the transcription.
    ///
    /// Useful for providing context about the audio content,
    /// spelling of specific terms, or desired formatting.
    pub prompt: Option<String>,

    /// Flush strategy for sending buffered audio.
    pub flush_strategy: FlushStrategy,

    /// Buffer threshold in bytes for OnThreshold flush strategy.
    ///
    /// When the buffer reaches this size, audio is sent to the API.
    /// Default is 1MB (approximately 32 seconds at 16kHz 16-bit mono).
    pub flush_threshold_bytes: usize,

    /// Maximum file size in bytes (OpenAI limit is 25MB).
    pub max_file_size_bytes: usize,

    /// Silence detection configuration for OnSilence flush strategy.
    pub silence_detection: SilenceDetectionConfig,
}

/// Configuration for silence detection.
///
/// Used to detect when a speaker has stopped talking, triggering
/// a flush of the audio buffer for transcription.
#[derive(Debug, Clone)]
pub struct SilenceDetectionConfig {
    /// RMS energy threshold below which audio is considered silent.
    /// Default is 0.01 (relative to normalized audio -1.0 to 1.0).
    pub rms_threshold: f32,

    /// Minimum duration of silence (in milliseconds) before triggering flush.
    /// Default is 1000ms (1 second).
    pub silence_duration_ms: u32,

    /// Minimum audio duration (in milliseconds) before silence detection is active.
    /// Prevents premature flush for initial silence.
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

impl Default for OpenAISTTConfig {
    fn default() -> Self {
        Self {
            base: STTConfig::default(),
            model: OpenAISTTModel::default(),
            response_format: ResponseFormat::VerboseJson, // For word timestamps
            temperature: Some(0.0),                       // Deterministic results
            timestamp_granularities: vec![TimestampGranularity::Word],
            audio_input_format: AudioInputFormat::Wav,
            prompt: None,
            flush_strategy: FlushStrategy::OnDisconnect,
            flush_threshold_bytes: 1024 * 1024,    // 1MB
            max_file_size_bytes: 25 * 1024 * 1024, // 25MB (OpenAI limit)
            silence_detection: SilenceDetectionConfig::default(),
        }
    }
}

impl OpenAISTTConfig {
    /// Create a new configuration from base STTConfig.
    ///
    /// Automatically determines the model from the config if specified.
    pub fn from_base(base: STTConfig) -> Self {
        let model = if base.model.is_empty() {
            OpenAISTTModel::default()
        } else {
            OpenAISTTModel::from_str_or_default(&base.model)
        };

        Self {
            base,
            model,
            ..Default::default()
        }
    }

    /// Get the API endpoint URL.
    #[inline]
    pub fn api_url(&self) -> &'static str {
        "https://api.openai.com/v1/audio/transcriptions"
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

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_openai_stt_model_as_str() {
        assert_eq!(OpenAISTTModel::Whisper1.as_str(), "whisper-1");
        assert_eq!(
            OpenAISTTModel::Gpt4oTranscribe.as_str(),
            "gpt-4o-transcribe"
        );
        assert_eq!(
            OpenAISTTModel::Gpt4oMiniTranscribe.as_str(),
            "gpt-4o-mini-transcribe"
        );
    }

    #[test]
    fn test_openai_stt_model_from_str() {
        assert_eq!(
            OpenAISTTModel::from_str_or_default("whisper-1"),
            OpenAISTTModel::Whisper1
        );
        assert_eq!(
            OpenAISTTModel::from_str_or_default("gpt-4o-transcribe"),
            OpenAISTTModel::Gpt4oTranscribe
        );
        assert_eq!(
            OpenAISTTModel::from_str_or_default("unknown"),
            OpenAISTTModel::default()
        );
    }

    #[test]
    fn test_response_format_as_str() {
        assert_eq!(ResponseFormat::Json.as_str(), "json");
        assert_eq!(ResponseFormat::Text.as_str(), "text");
        assert_eq!(ResponseFormat::VerboseJson.as_str(), "verbose_json");
        assert_eq!(ResponseFormat::Srt.as_str(), "srt");
        assert_eq!(ResponseFormat::Vtt.as_str(), "vtt");
    }

    #[test]
    fn test_audio_format_mime_type() {
        assert_eq!(AudioInputFormat::Wav.mime_type(), "audio/wav");
        assert_eq!(AudioInputFormat::Mp3.mime_type(), "audio/mpeg");
        assert_eq!(AudioInputFormat::Webm.mime_type(), "audio/webm");
    }

    #[test]
    fn test_config_validation_empty_api_key() {
        let config = OpenAISTTConfig {
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
        let config = OpenAISTTConfig {
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
    fn test_config_validation_valid() {
        let config = OpenAISTTConfig {
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
            model: "gpt-4o-transcribe".to_string(),
            language: "en".to_string(),
            sample_rate: 16000,
            ..Default::default()
        };

        let config = OpenAISTTConfig::from_base(base);
        assert_eq!(config.model, OpenAISTTModel::Gpt4oTranscribe);
    }

    #[test]
    fn test_default_config() {
        let config = OpenAISTTConfig::default();
        assert_eq!(config.model, OpenAISTTModel::Whisper1);
        assert_eq!(config.response_format, ResponseFormat::VerboseJson);
        assert_eq!(config.temperature, Some(0.0));
        assert_eq!(config.flush_threshold_bytes, 1024 * 1024);
        assert_eq!(config.max_file_size_bytes, 25 * 1024 * 1024);
    }
}
