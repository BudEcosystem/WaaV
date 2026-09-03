//! Configuration types for OpenAI STT (Whisper) API.
//!
//! This module contains all configuration-related types including:
//! - Model selection (whisper-1, gpt-4o-transcribe, gpt-4o-mini-transcribe)
//! - Response format options
//! - Audio format specifications
//! - Provider-specific configuration options

use super::super::base::STTConfig;
use serde::{Deserialize, Serialize};

const OPENAI_BASE_URL_ENV: &str = "OPENAI_BASE_URL";
const OPENAI_API_BASE_URL: &str = "https://api.openai.com";

fn openai_audio_url_from_base(
    source: &str,
    base: &str,
    path: &str,
) -> Result<Option<String>, String> {
    let base = base.trim().trim_end_matches('/');
    if base.is_empty() {
        return Ok(None);
    }
    crate::core::net::validate_url_for_ssrf(base, &["http", "https"])
        .map_err(|msg| format!("{source} rejected (SSRF protection): {msg}"))?;
    Ok(Some(format!("{base}{path}")))
}

fn resolve_openai_audio_url(endpoint_override: Option<&str>, path: &str) -> Result<String, String> {
    if let Some(override_url) = endpoint_override
        && let Some(url) = openai_audio_url_from_base("endpoint_override", override_url, path)?
    {
        return Ok(url);
    }

    match std::env::var(OPENAI_BASE_URL_ENV) {
        Ok(base) => {
            if let Some(url) = openai_audio_url_from_base(OPENAI_BASE_URL_ENV, &base, path)? {
                return Ok(url);
            }
        }
        Err(std::env::VarError::NotPresent) => {}
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err(format!("{OPENAI_BASE_URL_ENV} must be valid UTF-8"));
        }
    }

    Ok(format!("{OPENAI_API_BASE_URL}{path}"))
}

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
        if let Some(ref strategy) = self.chunking_strategy
            && let Some(threshold) = strategy.threshold
            && !(0.0..=1.0).contains(&threshold)
        {
            return Err(format!(
                "Chunking threshold must be between 0.0 and 1.0, got {}",
                threshold
            ));
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

    /// Request streaming partial transcripts (server-sent events). Maps the standardized
    /// `interim_results` feature onto OpenAI's `stream` form parameter. Only supported by the
    /// `gpt-4o-transcribe`/`gpt-4o-mini-transcribe` models (`whisper-1` ignores it). Ref: OpenAI
    /// `POST /v1/audio/transcriptions` `stream` field.
    pub stream: bool,

    /// Base endpoint override (scheme://host) from the standardized `endpoint_override` — points the
    /// transcription POST at a mock/proxy host (credential-free e2e), taking precedence over the
    /// `OPENAI_BASE_URL` env var. `None` falls back to the env var, then the production host.
    /// Non-empty override values must pass the canonical SSRF gate before the client can dial them.
    pub endpoint_override: Option<String>,

    /// P5 translation (Class B): when `true`, POST to `/v1/audio/translations` (English-only)
    /// instead of `/v1/audio/transcriptions`. Whisper's translations endpoint always outputs
    /// English regardless of source language and takes no target/`language` param — so the
    /// canonical translation block selects it via `translate_to_english` (arbitrary `target_languages`
    /// are degraded to English with a `config_warning`). Mirrors the Groq reference impl.
    pub translate_to_english: bool,
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
            stream: false,
            endpoint_override: None,
            translate_to_english: false,
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

        let mut cfg = Self {
            base,
            model,
            ..Default::default()
        };
        cfg.coerce_response_format_for_model();
        cfg
    }

    /// gpt-4o-transcribe / gpt-4o-mini-transcribe support ONLY `json`/`text`
    /// response formats — `verbose_json` (the whisper-1 default, needed for word
    /// timestamps) returns a 400 on them. Coerce `VerboseJson` → `Json` for those
    /// models so the default (and any word-timestamp request) degrades safely
    /// instead of erroring. Streaming (`stream=true`) is `json`-only too, and
    /// only those models stream, so this covers it. `DiarizedJson` is untouched
    /// (its own model path).
    fn coerce_response_format_for_model(&mut self) {
        if self.model != OpenAISTTModel::Whisper1
            && self.response_format == ResponseFormat::VerboseJson
        {
            self.response_format = ResponseFormat::Json;
            // Word-timestamp granularities require verbose_json — drop them too.
            self.timestamp_granularities.clear();
        }
    }

    /// Build from the standardized config (W1 keystone). OpenAI models its advanced features on
    /// the `response_format` (diarization needs `DiarizedJson`, word timestamps need a JSON format
    /// with word granularity), the free-form `prompt` (the documented spelling/vocabulary hint),
    /// the `stream` flag (partial transcripts) and a set of diarization-side knobs. This maps:
    /// - `diarization` → `DiarizedJson` response format,
    /// - `word_timestamps` → word timestamp granularity (+ JSON format),
    /// - `keyterms` → `prompt`,
    /// - `interim_results` → `stream` (partial transcripts, gpt-4o models),
    /// - extras `logprobs` (bool) → `include[]=logprobs` (token log probabilities),
    /// - extras `chunking_strategy` (string `"auto"` or object) → `chunking_strategy` (server VAD),
    /// - extras `known_speaker_names` (string array) → `known_speaker_names[]`,
    /// - extras `known_speaker_references` (string array) → `known_speaker_references[]`.
    ///
    /// Features OpenAI cannot express (smart_format, profanity_filter, redaction, vad_events,
    /// entity/language detection) are capability gaps and stay at default.
    pub fn from_standard(std: &crate::core::stt::standard::StandardSTTConfig) -> Self {
        let f = &std.features;
        let mut cfg = Self::from_base(std.base.clone());
        cfg.endpoint_override = std.endpoint_override().map(|s| s.to_string());
        // P5 translation (Class B — English-only fast path): select `/v1/audio/translations`.
        if let Some(t) = &std.translation
            && !t.is_noop()
        {
            cfg.translate_to_english = true;
        }
        if let Some(true) = f.diarization {
            cfg.response_format = ResponseFormat::DiarizedJson;
        }
        if let Some(w) = f.word_timestamps {
            cfg.timestamp_granularities = if w {
                // Granularities are only sent for VerboseJson (client.rs guards on it). Ensure a
                // word-timestamp request that isn't also a diarization request lands on VerboseJson
                // so the granularities actually reach the wire. (Review S6.)
                if cfg.response_format != ResponseFormat::DiarizedJson {
                    cfg.response_format = ResponseFormat::VerboseJson;
                }
                vec![TimestampGranularity::Word]
            } else {
                Vec::new()
            };
        }
        if let Some(k) = &f.keyterms
            && !k.is_empty()
        {
            cfg.prompt = Some(k.join(", "));
        }
        // interim_results → stream (typed).
        if let Some(s) = f.interim_results {
            cfg.stream = s;
        }
        // Provider extras → diarization-side knobs (string keys not modeled by the typed vocab).
        let e = &std.extras.0;
        if let Some(b) = e.get("logprobs").and_then(|v| v.as_bool()) {
            cfg.diarization.include_logprobs = b;
        }
        if let Some(arr) = e.get("known_speaker_names").and_then(|v| v.as_array()) {
            cfg.diarization.known_speaker_names = arr
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
        }
        if let Some(arr) = e.get("known_speaker_references").and_then(|v| v.as_array()) {
            // References are paired positionally with names: name[i] ↔ reference[i].
            let names = &cfg.diarization.known_speaker_names;
            cfg.diarization.speaker_references = arr
                .iter()
                .filter_map(|v| v.as_str())
                .enumerate()
                .map(|(i, audio)| {
                    let name = names
                        .get(i)
                        .cloned()
                        .unwrap_or_else(|| format!("speaker_{i}"));
                    SpeakerReference::new(name, audio.to_string())
                })
                .collect();
        }
        if let Some(cs) = e.get("chunking_strategy") {
            cfg.diarization.chunking_strategy =
                serde_json::from_value::<ChunkingStrategy>(cs.clone())
                    .ok()
                    .or_else(|| {
                        // A bare `"auto"`/`"server_vad"` string selects the default server-VAD strategy.
                        cs.as_str()
                            .filter(|s| !s.is_empty())
                            .map(|_| ChunkingStrategy::default())
                    });
        }
        // The word-timestamp branch above may have set VerboseJson; coerce back to
        // Json for gpt-4o-transcribe / -mini (they reject verbose_json).
        cfg.coerce_response_format_for_model();
        cfg
    }

    /// Build the multipart text form fields (key/value pairs) that accompany the audio file part
    /// on `POST /v1/audio/transcriptions`. This is the exact wire surface the client serializes
    /// into the multipart body — extracted as a pure function so a test can assert each advanced
    /// api_param actually reaches the request (the recurring "lives on the struct but never hits
    /// the wire" bug class). The `file` part is added separately by the client.
    ///
    /// Repeated keys (`timestamp_granularities[]`, `include[]`, `known_speaker_names[]`,
    /// `known_speaker_references[]`) appear once per value, matching OpenAI's array-form encoding.
    pub fn transcription_text_fields(&self) -> Vec<(String, String)> {
        let mut fields: Vec<(String, String)> = Vec::new();
        fields.push(("model".into(), self.model.as_str().to_string()));
        fields.push((
            "response_format".into(),
            self.response_format.as_str().to_string(),
        ));

        if !self.base.language.is_empty() {
            fields.push(("language".into(), self.base.language.clone()));
        }
        if let Some(temp) = self.temperature {
            fields.push(("temperature".into(), temp.to_string()));
        }
        if let Some(ref prompt) = self.prompt {
            fields.push(("prompt".into(), prompt.clone()));
        }

        // Timestamp granularities only apply to verbose_json.
        if self.response_format == ResponseFormat::VerboseJson {
            for g in &self.timestamp_granularities {
                fields.push(("timestamp_granularities[]".into(), g.as_str().to_string()));
            }
        }

        // Partial transcripts (interim_results → stream).
        if self.stream {
            fields.push(("stream".into(), "true".into()));
        }

        // Token log probabilities (`include[]=logprobs`). OpenAI only honors `include[]` for the
        // JSON response formats, so guard on a JSON format to avoid a 400.
        if self.diarization.include_logprobs
            && matches!(
                self.response_format,
                ResponseFormat::Json | ResponseFormat::VerboseJson | ResponseFormat::DiarizedJson
            )
        {
            fields.push(("include[]".into(), "logprobs".into()));
        }

        // Server-side VAD chunking strategy. Sent as a JSON-encoded value (object), matching the
        // OpenAI schema for `chunking_strategy`.
        if let Some(ref cs) = self.diarization.chunking_strategy
            && let Ok(json) = serde_json::to_string(cs)
        {
            fields.push(("chunking_strategy".into(), json));
        }

        // Known speaker names + reference audio (diarization speaker matching).
        for name in &self.diarization.known_speaker_names {
            fields.push(("known_speaker_names[]".into(), name.clone()));
        }
        for r in &self.diarization.speaker_references {
            fields.push(("known_speaker_references[]".into(), r.audio.clone()));
        }

        fields
    }

    /// Get the API endpoint URL.
    ///
    /// Honors the standard `OPENAI_BASE_URL` override (as the OpenAI SDKs do), so the provider
    /// also works against OpenAI-compatible endpoints (Azure OpenAI, Groq, vLLM, a local
    /// transcription server, or a proxy) — and enables credential-free contract/e2e testing.
    #[inline]
    pub fn try_api_url(&self) -> Result<String, String> {
        // P5 translation (Class B): the English-only fast path POSTs to `/audio/translations`.
        let path = if self.translate_to_english {
            "/v1/audio/translations"
        } else {
            "/v1/audio/transcriptions"
        };
        resolve_openai_audio_url(self.endpoint_override.as_deref(), path)
    }

    /// Get the API endpoint URL, falling back to the production host if an invalid process env
    /// override appears after construction. Runtime send paths use [`Self::try_api_url`] so invalid
    /// explicit config still fails closed with a typed configuration error.
    #[inline]
    pub fn api_url(&self) -> String {
        self.try_api_url().unwrap_or_else(|e| {
            tracing::error!(error = %e, "invalid OpenAI STT endpoint override; using production endpoint for display-only URL");
            let path = if self.translate_to_english {
                "/v1/audio/translations"
            } else {
                "/v1/audio/transcriptions"
            };
            format!("{OPENAI_API_BASE_URL}{path}")
        })
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

        self.try_api_url()?;

        // Validate that diarized_json format is only used with compatible models
        if self.response_format == ResponseFormat::DiarizedJson && !self.supports_diarization() {
            return Err(format!(
                "Diarized JSON format requires gpt-4o-transcribe or gpt-4o-mini-transcribe model, got {}",
                self.model
            ));
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

    // WIRE-LEVEL (the recurring bug class): the advanced features mapped in `from_standard` must
    // reach the multipart text fields the client serializes into the request body — not merely
    // live on `DiarizationConfig`. This drives the standardized config through `from_standard`
    // then through the SAME `transcription_text_fields()` the client posts, asserting each
    // api_param (`stream`, `include[]=logprobs`, `chunking_strategy`, `known_speaker_names[]`,
    // `known_speaker_references[]`) is present in the form body.
    #[test]
    fn gpt4o_transcribe_coerces_verbose_json_to_json() {
        // Live-caught: gpt-4o-transcribe / -mini 400 on the verbose_json default.
        for m in ["gpt-4o-transcribe", "gpt-4o-mini-transcribe"] {
            let base = STTConfig {
                model: m.to_string(),
                ..Default::default()
            };
            let cfg = OpenAISTTConfig::from_base(base);
            assert_eq!(
                cfg.response_format,
                ResponseFormat::Json,
                "{m}: verbose_json must coerce to json"
            );
        }
        // whisper-1 keeps the verbose_json default (it supports it).
        let wcfg = OpenAISTTConfig::from_base(STTConfig {
            model: "whisper-1".to_string(),
            ..Default::default()
        });
        assert_eq!(wcfg.response_format, ResponseFormat::VerboseJson);
    }

    #[test]
    fn advanced_features_reach_multipart_form_fields() {
        use crate::core::stt::standard::{ProviderExtras, StandardSTTConfig, SttFeatures};

        let mut extras = serde_json::Map::new();
        extras.insert("logprobs".into(), serde_json::json!(true));
        extras.insert(
            "chunking_strategy".into(),
            serde_json::json!({"type": "server_vad", "threshold": 0.6}),
        );
        extras.insert(
            "known_speaker_names".into(),
            serde_json::json!(["Alice", "Bob"]),
        );
        extras.insert(
            "known_speaker_references".into(),
            serde_json::json!(["data:audio/wav;base64,AAA=", "data:audio/wav;base64,BBB="]),
        );

        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "openai".into(),
                api_key: "test-key".into(),
                model: "gpt-4o-transcribe".into(),
                ..Default::default()
            },
            features: SttFeatures {
                diarization: Some(true),
                interim_results: Some(true),
                ..Default::default()
            },
            extras: ProviderExtras(extras),
            translation: None,
        };

        let cfg = OpenAISTTConfig::from_standard(&std);
        // The mapped config must validate (diarization needs a gpt-4o model + DiarizedJson).
        assert!(cfg.validate().is_ok(), "mapped config should validate");

        let fields = cfg.transcription_text_fields();
        let has = |k: &str, v: &str| fields.iter().any(|(fk, fv)| fk == k && fv == v);

        // interim_results → stream=true (typed)
        assert!(has("stream", "true"), "stream not in form: {fields:?}");
        // logprobs → include[]=logprobs (extras)
        assert!(
            has("include[]", "logprobs"),
            "include[]=logprobs not in form: {fields:?}"
        );
        // chunking_strategy (extras): JSON object carrying the server_vad type
        let cs = fields
            .iter()
            .find(|(k, _)| k == "chunking_strategy")
            .map(|(_, v)| v.clone())
            .expect("chunking_strategy not in form");
        assert!(cs.contains("server_vad"), "chunking_strategy wrong: {cs}");
        // known_speaker_names[] (extras)
        assert!(
            has("known_speaker_names[]", "Alice"),
            "name Alice missing: {fields:?}"
        );
        assert!(
            has("known_speaker_names[]", "Bob"),
            "name Bob missing: {fields:?}"
        );
        // known_speaker_references[] (extras) — paired positionally with names
        assert!(
            has("known_speaker_references[]", "data:audio/wav;base64,AAA="),
            "reference AAA missing: {fields:?}"
        );
        assert!(
            has("known_speaker_references[]", "data:audio/wav;base64,BBB="),
            "reference BBB missing: {fields:?}"
        );
    }

    // A config with no advanced features omits every new key (additive: unchanged wire shape).
    #[test]
    fn default_form_fields_omit_advanced_keys() {
        let cfg = OpenAISTTConfig {
            base: STTConfig {
                api_key: "k".into(),
                ..Default::default()
            },
            ..Default::default()
        };
        let fields = cfg.transcription_text_fields();
        for k in [
            "stream",
            "include[]",
            "chunking_strategy",
            "known_speaker_names[]",
            "known_speaker_references[]",
        ] {
            assert!(
                !fields.iter().any(|(fk, _)| fk == k),
                "{k} should be omitted by default"
            );
        }
        // The base fields are still present.
        assert!(fields.iter().any(|(k, _)| k == "model"));
        assert!(fields.iter().any(|(k, _)| k == "response_format"));
    }

    // W1 keystone: the standardized features unlock OpenAI's response-format-driven feature
    // surface (diarization + word timestamps) — previously unreachable via the flat factory.
    #[test]
    fn from_standard_maps_features() {
        use crate::core::stt::standard::{StandardSTTConfig, SttFeatures};
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "openai".into(),
                api_key: "test-key".into(),
                model: "gpt-4o-transcribe".into(),
                ..Default::default()
            },
            features: SttFeatures {
                diarization: Some(true),
                keyterms: Some(vec!["WaaV".into(), "OpenAI".into()]),
                ..Default::default()
            },
            ..StandardSTTConfig::from_base(STTConfig::default())
        };
        let cfg = OpenAISTTConfig::from_standard(&std);
        assert_eq!(cfg.response_format, ResponseFormat::DiarizedJson);
        assert_eq!(cfg.prompt.as_deref(), Some("WaaV, OpenAI"));
    }

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
