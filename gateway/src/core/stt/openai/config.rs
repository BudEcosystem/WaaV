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
/// - `diarized_json`: JSON with speaker diarization (GPT-4o-transcribe only)
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
    /// Diarized JSON with speaker labels (requires gpt-4o-transcribe)
    ///
    /// Returns segments with speaker assignments and word-level timing.
    /// Each segment includes a `speaker` field identifying the speaker.
    #[serde(rename = "diarized_json")]
    DiarizedJson,
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
            Self::DiarizedJson => "diarized_json",
            Self::Srt => "srt",
            Self::Vtt => "vtt",
        }
    }

    /// Check if this format supports diarization.
    #[inline]
    pub fn supports_diarization(&self) -> bool {
        matches!(self, Self::DiarizedJson)
    }

    /// Check if this format returns word-level timestamps.
    #[inline]
    pub fn has_word_timestamps(&self) -> bool {
        matches!(self, Self::VerboseJson | Self::DiarizedJson)
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
// Chunking Strategy (Server-side VAD)
// =============================================================================

/// Chunking strategy type for server-side processing.
///
/// Controls how the audio is chunked for processing. Currently only
/// server_vad is supported for automatic voice activity detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChunkingStrategyType {
    /// Server-side Voice Activity Detection
    #[default]
    #[serde(rename = "server_vad")]
    ServerVad,
}

impl ChunkingStrategyType {
    /// Convert to the API parameter value.
    #[inline]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ServerVad => "server_vad",
        }
    }
}

/// Configuration for server-side voice activity detection chunking.
///
/// When using `server_vad` chunking, the server automatically detects
/// speech boundaries and segments the audio accordingly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkingStrategy {
    /// The type of chunking strategy (currently only server_vad).
    #[serde(rename = "type")]
    pub strategy_type: ChunkingStrategyType,

    /// Activation threshold for voice detection (0.0 to 1.0).
    ///
    /// Lower values make the VAD more sensitive to speech.
    /// Default is 0.5.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threshold: Option<f32>,

    /// Amount of audio to include before speech starts (in milliseconds).
    ///
    /// Helps capture the beginning of utterances.
    /// Default is typically around 200-500ms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix_padding_ms: Option<u32>,

    /// Duration of silence to wait before considering speech ended (in milliseconds).
    ///
    /// Higher values wait longer before segmenting, useful for speakers
    /// with longer pauses.
    /// Default is typically around 500-1000ms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub silence_duration_ms: Option<u32>,
}

impl Default for ChunkingStrategy {
    fn default() -> Self {
        Self {
            strategy_type: ChunkingStrategyType::default(),
            threshold: Some(0.5),
            prefix_padding_ms: Some(300),
            silence_duration_ms: Some(500),
        }
    }
}

// =============================================================================
// Speaker Reference (for Known Speaker Recognition)
// =============================================================================

/// Reference audio for a known speaker.
///
/// Used to provide sample audio of known speakers to improve
/// speaker identification accuracy in diarization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeakerReference {
    /// Name/identifier for this speaker.
    ///
    /// This name will be used in the diarization output instead of
    /// generic speaker IDs like "speaker_0".
    pub name: String,

    /// Base64-encoded audio sample of this speaker.
    ///
    /// The audio should be a clear sample of the speaker's voice,
    /// typically 3-10 seconds of speech.
    pub audio: String,
}

impl SpeakerReference {
    /// Create a new speaker reference.
    pub fn new(name: impl Into<String>, audio_base64: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            audio: audio_base64.into(),
        }
    }
}

// =============================================================================
// Diarization Configuration
// =============================================================================

/// Configuration for speaker diarization.
///
/// Controls how speakers are identified and labeled in the transcription.
/// Only applies when using `DiarizedJson` response format.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DiarizationConfig {
    /// Whether to include log probabilities in the output.
    ///
    /// When enabled, each token includes its log probability,
    /// useful for confidence scoring.
    #[serde(default)]
    pub include_logprobs: bool,

    /// List of known speaker names.
    ///
    /// If provided, the API will try to match detected speakers
    /// to these names when possible.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub known_speaker_names: Vec<String>,

    /// Reference audio samples for known speakers.
    ///
    /// Providing audio samples helps the model identify specific
    /// speakers more accurately.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub speaker_references: Vec<SpeakerReference>,

    /// Server-side chunking/VAD configuration.
    ///
    /// Controls how the audio is segmented for processing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chunking_strategy: Option<ChunkingStrategy>,
}

impl DiarizationConfig {
    /// Create a new diarization config with known speaker names.
    pub fn with_speakers(names: Vec<String>) -> Self {
        Self {
            known_speaker_names: names,
            ..Default::default()
        }
    }

    /// Check if any speaker references are provided.
    #[inline]
    pub fn has_speaker_references(&self) -> bool {
        !self.speaker_references.is_empty()
    }

    /// Validate the diarization configuration.
    pub fn validate(&self) -> Result<(), String> {
        // Validate chunking strategy threshold
        if let Some(ref strategy) = self.chunking_strategy {
            if let Some(threshold) = strategy.threshold {
                if !(0.0..=1.0).contains(&threshold) {
                    return Err(format!(
                        "Chunking threshold must be between 0.0 and 1.0, got {}",
                        threshold
                    ));
                }
            }
        }

        // Validate speaker references have both name and audio
        for (i, speaker) in self.speaker_references.iter().enumerate() {
            if speaker.name.is_empty() {
                return Err(format!("Speaker reference {} has empty name", i));
            }
            if speaker.audio.is_empty() {
                return Err(format!(
                    "Speaker reference '{}' has empty audio",
                    speaker.name
                ));
            }
        }

        Ok(())
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

    /// Diarization configuration for speaker identification.
    ///
    /// Only used when `response_format` is `DiarizedJson`.
    /// Requires `gpt-4o-transcribe` or `gpt-4o-mini-transcribe` model.
    pub diarization: DiarizationConfig,
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
            diarization: DiarizationConfig::default(),
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

        // Validate diarization configuration
        self.diarization.validate()?;

        // Validate that diarized_json format is only used with compatible models
        if self.response_format == ResponseFormat::DiarizedJson {
            if !self.supports_diarization() {
                return Err(format!(
                    "Diarized JSON format requires gpt-4o-transcribe or gpt-4o-mini-transcribe model, got {}",
                    self.model
                ));
            }
        }

        Ok(())
    }

    /// Check if the current model supports diarization.
    #[inline]
    pub fn supports_diarization(&self) -> bool {
        matches!(
            self.model,
            OpenAISTTModel::Gpt4oTranscribe | OpenAISTTModel::Gpt4oMiniTranscribe
        )
    }

    /// Check if diarization is enabled.
    #[inline]
    pub fn is_diarization_enabled(&self) -> bool {
        self.response_format == ResponseFormat::DiarizedJson && self.supports_diarization()
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

    // =========================================================================
    // Diarization Tests
    // =========================================================================

    #[test]
    fn test_response_format_diarized_json() {
        assert_eq!(ResponseFormat::DiarizedJson.as_str(), "diarized_json");
        assert!(ResponseFormat::DiarizedJson.supports_diarization());
        assert!(ResponseFormat::DiarizedJson.has_word_timestamps());
        assert!(!ResponseFormat::Json.supports_diarization());
    }

    #[test]
    fn test_chunking_strategy_default() {
        let strategy = ChunkingStrategy::default();
        assert_eq!(strategy.strategy_type, ChunkingStrategyType::ServerVad);
        assert_eq!(strategy.threshold, Some(0.5));
        assert_eq!(strategy.prefix_padding_ms, Some(300));
        assert_eq!(strategy.silence_duration_ms, Some(500));
    }

    #[test]
    fn test_speaker_reference_new() {
        let reference = SpeakerReference::new("John", "base64audio==");
        assert_eq!(reference.name, "John");
        assert_eq!(reference.audio, "base64audio==");
    }

    #[test]
    fn test_diarization_config_default() {
        let config = DiarizationConfig::default();
        assert!(!config.include_logprobs);
        assert!(config.known_speaker_names.is_empty());
        assert!(config.speaker_references.is_empty());
        assert!(config.chunking_strategy.is_none());
    }

    #[test]
    fn test_diarization_config_with_speakers() {
        let config = DiarizationConfig::with_speakers(vec!["Alice".to_string(), "Bob".to_string()]);
        assert_eq!(config.known_speaker_names.len(), 2);
        assert_eq!(config.known_speaker_names[0], "Alice");
        assert_eq!(config.known_speaker_names[1], "Bob");
    }

    #[test]
    fn test_diarization_config_validate_valid() {
        let config = DiarizationConfig {
            include_logprobs: true,
            known_speaker_names: vec!["Alice".to_string()],
            speaker_references: vec![SpeakerReference::new("Bob", "audio==")],
            chunking_strategy: Some(ChunkingStrategy::default()),
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_diarization_config_validate_invalid_threshold() {
        let config = DiarizationConfig {
            chunking_strategy: Some(ChunkingStrategy {
                strategy_type: ChunkingStrategyType::ServerVad,
                threshold: Some(1.5), // Invalid: > 1.0
                prefix_padding_ms: None,
                silence_duration_ms: None,
            }),
            ..Default::default()
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("threshold"));
    }

    #[test]
    fn test_diarization_config_validate_empty_speaker_name() {
        let config = DiarizationConfig {
            speaker_references: vec![SpeakerReference::new("", "audio==")],
            ..Default::default()
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("empty name"));
    }

    #[test]
    fn test_diarization_config_validate_empty_speaker_audio() {
        let config = DiarizationConfig {
            speaker_references: vec![SpeakerReference::new("Alice", "")],
            ..Default::default()
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("empty audio"));
    }

    #[test]
    fn test_config_supports_diarization() {
        let whisper_config = OpenAISTTConfig {
            model: OpenAISTTModel::Whisper1,
            ..Default::default()
        };
        assert!(!whisper_config.supports_diarization());

        let gpt4o_config = OpenAISTTConfig {
            model: OpenAISTTModel::Gpt4oTranscribe,
            ..Default::default()
        };
        assert!(gpt4o_config.supports_diarization());

        let gpt4o_mini_config = OpenAISTTConfig {
            model: OpenAISTTModel::Gpt4oMiniTranscribe,
            ..Default::default()
        };
        assert!(gpt4o_mini_config.supports_diarization());
    }

    #[test]
    fn test_config_is_diarization_enabled() {
        let config = OpenAISTTConfig {
            base: STTConfig {
                api_key: "test".to_string(),
                ..Default::default()
            },
            model: OpenAISTTModel::Gpt4oTranscribe,
            response_format: ResponseFormat::DiarizedJson,
            ..Default::default()
        };
        assert!(config.is_diarization_enabled());

        let config_no_diarize = OpenAISTTConfig {
            model: OpenAISTTModel::Gpt4oTranscribe,
            response_format: ResponseFormat::VerboseJson,
            ..Default::default()
        };
        assert!(!config_no_diarize.is_diarization_enabled());
    }

    #[test]
    fn test_config_validate_diarization_wrong_model() {
        let config = OpenAISTTConfig {
            base: STTConfig {
                api_key: "test".to_string(),
                ..Default::default()
            },
            model: OpenAISTTModel::Whisper1,
            response_format: ResponseFormat::DiarizedJson,
            ..Default::default()
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("gpt-4o-transcribe"));
    }

    #[test]
    fn test_chunking_strategy_type_as_str() {
        assert_eq!(ChunkingStrategyType::ServerVad.as_str(), "server_vad");
    }
}
