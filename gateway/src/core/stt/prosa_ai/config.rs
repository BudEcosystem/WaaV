//! Configuration types for Prosa.ai STT API.
//!
//! This module provides configuration types for Prosa.ai's
//! Speech-to-Text service optimized for Indonesian language.
//!
//! # Overview
//!
//! Prosa.ai STT provides speech recognition for Indonesian (Bahasa Indonesia),
//! Javanese, Sundanese, and English languages.
//!
//! # Features
//!
//! - Synchronous recognition (up to 60 seconds)
//! - Asynchronous recognition (up to 4 hours)
//! - WebSocket streaming
//! - Multi-channel audio support
//! - Speaker diarization
//!
//! # Authentication
//!
//! The API uses API key authentication:
//! - `x-api-key`: Your Prosa.ai API key from console

use crate::core::stt::base::{STTConfig, STTError};
use serde::{Deserialize, Serialize};

fn validate_prosa_stt_endpoint(source: &str, endpoint: &str) -> Result<(), String> {
    let endpoint = endpoint.trim();
    if endpoint.is_empty() {
        return Ok(());
    }
    crate::core::net::validate_url_for_ssrf(endpoint, crate::core::net::HTTP_WS_URL_SCHEMES)
        .map_err(|msg| format!("{source} rejected (SSRF protection): {msg}"))
}

// =============================================================================
// Constants
// =============================================================================

/// Prosa.ai STT REST API base URL.
pub const PROSA_STT_BASE_URL: &str = "https://api.prosa.ai/v2/speech/stt";

/// Prosa.ai STT WebSocket streaming endpoint.
pub const PROSA_STT_WS_ENDPOINT: &str = "wss://s-api.prosa.ai/v2/speech/stt";

/// Default request timeout in seconds.
pub const DEFAULT_REQUEST_TIMEOUT: u64 = 60;

/// Default sample rate.
pub const DEFAULT_SAMPLE_RATE: u32 = 16000;

/// Default audio channels.
pub const DEFAULT_CHANNELS: u16 = 1;

/// Maximum audio duration for synchronous requests (in seconds).
pub const MAX_SYNC_DURATION_SECS: u64 = 60;

/// Maximum audio size for synchronous requests (in bytes).
pub const MAX_SYNC_SIZE_BYTES: usize = 10 * 1024 * 1024; // 10 MB

/// Maximum audio duration for asynchronous requests (in seconds).
pub const MAX_ASYNC_DURATION_SECS: u64 = 4 * 60 * 60; // 4 hours

/// Minimum audio buffer size before transcription.
pub const MIN_AUDIO_BUFFER_SIZE: usize = 3200; // ~100ms at 16kHz mono 16-bit

/// Default chunk size for WebSocket streaming.
pub const DEFAULT_CHUNK_SIZE: usize = 16000;

// =============================================================================
// STT Models
// =============================================================================

/// Available Prosa.ai STT models.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ProsaSttModel {
    /// General-purpose STT for batch processing.
    #[default]
    #[serde(rename = "stt-general")]
    General,

    /// Real-time streaming STT.
    #[serde(rename = "stt-general-online")]
    GeneralOnline,
}

impl ProsaSttModel {
    /// Get the model ID string.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::General => "stt-general",
            Self::GeneralOnline => "stt-general-online",
        }
    }

    /// Check if this model supports streaming.
    pub fn supports_streaming(&self) -> bool {
        matches!(self, Self::GeneralOnline)
    }

    /// Parse model from string (case-insensitive).
    pub fn from_str_relaxed(s: &str) -> Option<Self> {
        match s.to_lowercase().replace(['-', '_'], "").as_str() {
            "sttgeneral" | "general" => Some(Self::General),
            "sttgeneralonline" | "generalonline" | "online" | "streaming" => {
                Some(Self::GeneralOnline)
            }
            _ => None,
        }
    }
}

impl std::fmt::Display for ProsaSttModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl std::str::FromStr for ProsaSttModel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_str_relaxed(s).ok_or_else(|| format!("Unknown Prosa STT model: {}", s))
    }
}

// =============================================================================
// Audio Formats
// =============================================================================

/// Supported audio formats for Prosa.ai STT.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ProsaAudioFormat {
    /// WAV format (PCM).
    #[default]
    #[serde(rename = "wav")]
    Wav,

    /// MP3 format.
    #[serde(rename = "mp3")]
    Mp3,

    /// OGG format.
    #[serde(rename = "ogg")]
    Ogg,

    /// FLAC format.
    #[serde(rename = "flac")]
    Flac,

    /// WebM format.
    #[serde(rename = "webm")]
    WebM,
}

impl ProsaAudioFormat {
    /// Get the format string.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Wav => "wav",
            Self::Mp3 => "mp3",
            Self::Ogg => "ogg",
            Self::Flac => "flac",
            Self::WebM => "webm",
        }
    }

    /// Get MIME type for the format.
    pub fn mime_type(&self) -> &'static str {
        match self {
            Self::Wav => "audio/wav",
            Self::Mp3 => "audio/mpeg",
            Self::Ogg => "audio/ogg",
            Self::Flac => "audio/flac",
            Self::WebM => "audio/webm",
        }
    }
}

impl std::fmt::Display for ProsaAudioFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =============================================================================
// Configuration
// =============================================================================

/// Prosa.ai STT configuration.
#[derive(Debug, Clone)]
pub struct ProsaSttConfig {
    /// API key for authentication.
    pub api_key: String,

    /// STT model to use.
    pub model: ProsaSttModel,

    /// Audio sample rate in Hz.
    pub sample_rate: u32,

    /// Number of audio channels.
    pub channels: u16,

    /// Audio format.
    pub audio_format: ProsaAudioFormat,

    /// Wait for result (synchronous) or return job_id (asynchronous).
    pub wait: bool,

    /// Include partial results in streaming mode.
    pub include_partial: bool,

    /// Number of speakers for diarization (0 = auto).
    pub speaker_count: u32,

    /// Include filler words (um, uh).
    pub include_filler: bool,

    /// Auto-add punctuation.
    pub auto_punctuation: bool,

    /// Convert spoken numerals to words.
    pub enable_spoken_numerals: bool,

    /// Request timeout in seconds.
    pub request_timeout_secs: u64,

    /// Optional job label.
    pub label: Option<String>,

    /// Carried from the standardized `endpoint_override` — points the dial at the in-repo mock/proxy
    /// (a local `ws://` server) for credential-free end-to-end integration tests; `None` uses the
    /// production Prosa.ai endpoint.
    pub endpoint_override: Option<String>,
}

impl Default for ProsaSttConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            model: ProsaSttModel::default(),
            sample_rate: DEFAULT_SAMPLE_RATE,
            channels: DEFAULT_CHANNELS,
            audio_format: ProsaAudioFormat::default(),
            wait: true,
            include_partial: true,
            speaker_count: 0,
            include_filler: false,
            auto_punctuation: true,
            enable_spoken_numerals: true,
            request_timeout_secs: DEFAULT_REQUEST_TIMEOUT,
            label: None,
            endpoint_override: None,
        }
    }
}

impl ProsaSttConfig {
    /// Create configuration from base STTConfig.
    pub fn from_base(config: &STTConfig) -> Result<Self, STTError> {
        let api_key = config.api_key.clone();

        if api_key.is_empty() {
            return Err(STTError::AuthenticationFailed(
                "Prosa.ai API key is required".to_string(),
            ));
        }

        let sample_rate = if config.sample_rate > 0 {
            config.sample_rate
        } else {
            DEFAULT_SAMPLE_RATE
        };

        let channels = if config.channels > 0 {
            config.channels
        } else {
            DEFAULT_CHANNELS
        };

        // Parse model from config.model field if provided
        let model = if !config.model.is_empty() {
            ProsaSttModel::from_str_relaxed(&config.model).unwrap_or_default()
        } else {
            ProsaSttModel::default()
        };

        Ok(Self {
            api_key,
            model,
            sample_rate,
            channels,
            audio_format: ProsaAudioFormat::Wav,
            wait: true,
            include_partial: true,
            speaker_count: 0,
            include_filler: false,
            auto_punctuation: true,
            enable_spoken_numerals: true,
            request_timeout_secs: DEFAULT_REQUEST_TIMEOUT,
            label: None,
            endpoint_override: None,
        })
    }

    /// Build from the standardized config (W1 keystone). Prosa.ai exposes a small set of request
    /// toggles, so this maps the standardized features whose meaning matches an existing Prosa
    /// field: interim results (`include_partial`), filler words (`include_filler`) and smart
    /// formatting (`auto_punctuation`). Features Prosa cannot express (diarization is only an
    /// integer speaker count with no boolean toggle, plus word_timestamps, profanity_filter,
    /// vad/endpointing, keyterms, redaction, entity/language detection) are capability gaps and
    /// stay at their defaults.
    pub fn from_standard(
        std: &crate::core::stt::standard::StandardSTTConfig,
    ) -> Result<Self, STTError> {
        let f = &std.features;
        let mut cfg = Self::from_base(&std.base)?;
        if let Some(i) = f.interim_results {
            cfg.include_partial = i;
        }
        if let Some(filler) = f.filler_words {
            cfg.include_filler = filler;
        }
        if let Some(s) = f.smart_format {
            cfg.auto_punctuation = s;
        }
        cfg.endpoint_override = std.endpoint_override().map(|s| s.to_string());
        cfg.validate().map_err(STTError::ConfigurationError)?;
        Ok(cfg)
    }

    /// Validate the configuration.
    pub fn validate(&self) -> Result<(), String> {
        if self.api_key.is_empty() {
            return Err("Prosa.ai API key is required".to_string());
        }

        if self.sample_rate == 0 {
            return Err("Sample rate must be greater than 0".to_string());
        }

        if self.channels == 0 {
            return Err("Channels must be greater than 0".to_string());
        }

        if let Some(endpoint) = &self.endpoint_override {
            validate_prosa_stt_endpoint("endpoint_override", endpoint)?;
        }

        Ok(())
    }
}

// =============================================================================
// Request Types
// =============================================================================

/// Prosa.ai STT REST API request configuration.
#[derive(Debug, Clone, Serialize)]
pub struct ProsaSttRequestConfig {
    /// STT model/engine to use.
    pub engine: String,

    /// Wait for result synchronously.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wait: Option<bool>,

    /// Number of speakers for diarization.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker_count: Option<u32>,

    /// Include filler words.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_filler: Option<bool>,

    /// Auto-add punctuation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_punctuation: Option<bool>,

    /// Convert spoken numerals.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_spoken_numerals: Option<bool>,
}

/// Prosa.ai STT REST API request body.
#[derive(Debug, Clone, Serialize)]
pub struct ProsaSttRequest {
    /// Configuration options.
    pub config: ProsaSttRequestConfig,

    /// Request data.
    pub request: ProsaSttRequestData,
}

/// Prosa.ai STT request data.
#[derive(Debug, Clone, Serialize)]
pub struct ProsaSttRequestData {
    /// Optional job label.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,

    /// Base64-encoded audio data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,

    /// URI to audio file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
}

/// Prosa.ai STT WebSocket configuration message.
#[derive(Debug, Clone, Serialize)]
pub struct ProsaSttStreamConfig {
    /// STT model to use.
    pub model: String,

    /// Optional session label.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,

    /// Include partial results.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_partial: Option<bool>,

    /// Include filler words ("um", "uh") in the streaming transcript rather than dropping them.
    /// Mirrors the REST request's `include_filler`; Prosa.ai's streaming `config` message accepts
    /// the same recognition toggles as the REST `config` object (Prosa.ai STT v2 streaming docs:
    /// the WS session `config` is the streaming analogue of the REST `config`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_filler: Option<bool>,

    /// Audio configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio: Option<ProsaSttAudioConfig>,
}

/// Audio configuration for streaming.
#[derive(Debug, Clone, Serialize)]
pub struct ProsaSttAudioConfig {
    /// Audio format.
    pub format: String,

    /// Number of channels.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channels: Option<u16>,

    /// Sample rate in Hz.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,
}

// =============================================================================
// Response Types
// =============================================================================

/// Prosa.ai STT API response.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProsaSttResponse {
    /// Job ID.
    #[serde(default)]
    pub job_id: String,

    /// Job status.
    #[serde(default)]
    pub status: String,

    /// Transcription result.
    #[serde(default)]
    pub result: Option<ProsaSttResult>,

    /// Model information.
    #[serde(default)]
    pub model: Option<ProsaSttModelInfo>,

    /// Error information.
    #[serde(default)]
    pub error: Option<ProsaSttError>,
}

/// Prosa.ai STT result data.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProsaSttResult {
    /// Transcription segments.
    #[serde(default)]
    pub data: Vec<ProsaSttSegment>,
}

/// Prosa.ai STT transcription segment.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProsaSttSegment {
    /// Transcribed text.
    #[serde(default)]
    pub transcript: String,

    /// Start time in seconds.
    #[serde(default)]
    pub time_start: f64,

    /// End time in seconds.
    #[serde(default)]
    pub time_end: f64,

    /// Audio channel (0-indexed).
    #[serde(default)]
    pub channel: u32,

    /// Speaker ID.
    #[serde(default)]
    pub speaker: Option<u32>,
}

/// Prosa.ai STT model information.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProsaSttModelInfo {
    /// Model ID.
    #[serde(default)]
    pub id: String,

    /// Language code.
    #[serde(default)]
    pub language: String,
}

/// Prosa.ai STT error information.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProsaSttError {
    /// Error code.
    #[serde(default)]
    pub code: String,

    /// Error message.
    #[serde(default)]
    pub message: String,
}

impl ProsaSttResponse {
    /// Check if the job is complete.
    pub fn is_complete(&self) -> bool {
        self.status == "complete"
    }

    /// Check if the job is still in progress.
    pub fn is_in_progress(&self) -> bool {
        self.status == "created" || self.status == "processing"
    }

    /// Check if there's an error.
    pub fn has_error(&self) -> bool {
        self.error.is_some() || self.status == "error"
    }

    /// Get the full transcription text.
    pub fn transcription(&self) -> Option<String> {
        self.result.as_ref().map(|r| {
            r.data
                .iter()
                .map(|s| s.transcript.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        })
    }

    /// Get status message for errors.
    pub fn status_message(&self) -> &str {
        if let Some(err) = &self.error {
            if !err.message.is_empty() {
                return &err.message;
            }
            return match err.code.as_str() {
                "auth_invalid_api_key" => "Invalid API key",
                "auth_unauthorized" => "Unauthorized",
                "forbidden" => "Forbidden",
                "quota_insufficient" => "Insufficient quota",
                "quota_empty" => "Out of quota",
                "asr_model_not_found" => "ASR model not found",
                "asr_request_file_too_large" => "Audio file too large",
                "asr_request_invalid_data" => "Invalid audio data",
                "asr_request_no_audio_data" => "No audio provided",
                "asr_request_unsupported_media_type" => "Unsupported audio format",
                "job_not_found" => "Job not found",
                "internal_server_error" => "Internal server error",
                _ => "Unknown error",
            };
        }
        &self.status
    }
}

/// Prosa.ai STT WebSocket message types.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum ProsaSttWsMessage {
    /// Session created.
    #[serde(rename = "created")]
    Created { id: String },

    /// Status update.
    #[serde(rename = "status")]
    Status { status: String },

    /// Partial transcription.
    #[serde(rename = "partial")]
    Partial { transcript: String },

    /// Final transcription.
    #[serde(rename = "result")]
    Result {
        transcript: String,
        time_start: f64,
        time_end: f64,
    },

    /// Metadata (session summary).
    #[serde(rename = "metadata")]
    Metadata {
        duration: Option<f64>,
        quota_used: Option<f64>,
    },

    /// Error.
    #[serde(rename = "error")]
    Error { message: String },
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // W1 keystone: maps the standardized features Prosa.ai can express (interim results +
    // filler words) onto its own config fields.
    #[test]
    fn from_standard_maps_features() {
        use crate::core::stt::standard::{ProviderExtras, StandardSTTConfig, SttFeatures};
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "prosa_ai".into(),
                api_key: "test-key".into(),
                ..Default::default()
            },
            features: SttFeatures {
                interim_results: Some(false),
                filler_words: Some(true),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
            translation: None,
        };
        let cfg = ProsaSttConfig::from_standard(&std).unwrap();
        assert!(!cfg.include_partial); // interim_results
        assert!(cfg.include_filler); // filler_words
    }

    #[test]
    fn test_config_default() {
        let config = ProsaSttConfig::default();
        assert!(config.api_key.is_empty());
        assert_eq!(config.sample_rate, DEFAULT_SAMPLE_RATE);
        assert_eq!(config.channels, DEFAULT_CHANNELS);
        assert_eq!(config.model, ProsaSttModel::General);
        assert!(config.wait);
        assert!(config.include_partial);
    }

    #[test]
    fn test_config_validation_empty_key() {
        let config = ProsaSttConfig::default();
        assert!(config.validate().is_err());
        let err = config.validate().unwrap_err();
        assert!(err.contains("API key"));
    }

    #[test]
    fn test_config_validation_zero_sample_rate() {
        let mut config = ProsaSttConfig::default();
        config.api_key = "test_key".to_string();
        config.sample_rate = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_zero_channels() {
        let mut config = ProsaSttConfig::default();
        config.api_key = "test_key".to_string();
        config.channels = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_success() {
        let mut config = ProsaSttConfig::default();
        config.api_key = "test_key".to_string();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validation_rejects_ssrf_endpoint_override() {
        let _env = crate::core::net::ssrf_env_lock();
        let mut config = ProsaSttConfig {
            api_key: "test_key".to_string(),
            ..Default::default()
        };

        config.endpoint_override = Some("https://prosa-proxy.example.com".to_string());
        assert!(config.validate().is_ok());

        config.endpoint_override = Some("wss://prosa-stream.example.com".to_string());
        assert!(config.validate().is_ok());

        config.endpoint_override = Some("http://127.0.0.1:9000".to_string());
        let err = config
            .validate()
            .expect_err("loopback endpoint_override must be rejected");
        assert!(err.contains("SSRF protection"), "{err}");

        config.endpoint_override = Some("file:///tmp/socket".to_string());
        let err = config
            .validate()
            .expect_err("non-HTTP/WS endpoint_override must be rejected");
        assert!(err.contains("SSRF protection"), "{err}");
    }

    #[test]
    fn test_config_from_base_empty_key() {
        let config = STTConfig {
            api_key: String::new(),
            ..Default::default()
        };
        let result = ProsaSttConfig::from_base(&config);
        assert!(result.is_err());
    }

    #[test]
    fn test_config_from_base_with_params() {
        let config = STTConfig {
            api_key: "test_key".to_string(),
            sample_rate: 8000,
            channels: 2,
            ..Default::default()
        };
        let prosa_config = ProsaSttConfig::from_base(&config).unwrap();
        assert_eq!(prosa_config.sample_rate, 8000);
        assert_eq!(prosa_config.channels, 2);
    }

    #[test]
    fn test_config_from_base_default_sample_rate() {
        let config = STTConfig {
            api_key: "test_key".to_string(),
            sample_rate: 0,
            ..Default::default()
        };
        let prosa_config = ProsaSttConfig::from_base(&config).unwrap();
        assert_eq!(prosa_config.sample_rate, DEFAULT_SAMPLE_RATE);
    }

    #[test]
    fn test_model_as_str() {
        assert_eq!(ProsaSttModel::General.as_str(), "stt-general");
        assert_eq!(ProsaSttModel::GeneralOnline.as_str(), "stt-general-online");
    }

    #[test]
    fn test_model_supports_streaming() {
        assert!(!ProsaSttModel::General.supports_streaming());
        assert!(ProsaSttModel::GeneralOnline.supports_streaming());
    }

    #[test]
    fn test_model_from_str() {
        assert_eq!(
            ProsaSttModel::from_str_relaxed("stt-general"),
            Some(ProsaSttModel::General)
        );
        assert_eq!(
            ProsaSttModel::from_str_relaxed("general"),
            Some(ProsaSttModel::General)
        );
        assert_eq!(
            ProsaSttModel::from_str_relaxed("stt-general-online"),
            Some(ProsaSttModel::GeneralOnline)
        );
        assert_eq!(
            ProsaSttModel::from_str_relaxed("streaming"),
            Some(ProsaSttModel::GeneralOnline)
        );
        assert_eq!(ProsaSttModel::from_str_relaxed("invalid"), None);
    }

    #[test]
    fn test_model_display() {
        assert_eq!(format!("{}", ProsaSttModel::General), "stt-general");
        assert_eq!(
            format!("{}", ProsaSttModel::GeneralOnline),
            "stt-general-online"
        );
    }

    #[test]
    fn test_audio_format_as_str() {
        assert_eq!(ProsaAudioFormat::Wav.as_str(), "wav");
        assert_eq!(ProsaAudioFormat::Mp3.as_str(), "mp3");
        assert_eq!(ProsaAudioFormat::Ogg.as_str(), "ogg");
        assert_eq!(ProsaAudioFormat::Flac.as_str(), "flac");
        assert_eq!(ProsaAudioFormat::WebM.as_str(), "webm");
    }

    #[test]
    fn test_audio_format_mime_type() {
        assert_eq!(ProsaAudioFormat::Wav.mime_type(), "audio/wav");
        assert_eq!(ProsaAudioFormat::Mp3.mime_type(), "audio/mpeg");
        assert_eq!(ProsaAudioFormat::Ogg.mime_type(), "audio/ogg");
        assert_eq!(ProsaAudioFormat::Flac.mime_type(), "audio/flac");
        assert_eq!(ProsaAudioFormat::WebM.mime_type(), "audio/webm");
    }

    #[test]
    fn test_response_is_complete() {
        let response = ProsaSttResponse {
            job_id: "test".to_string(),
            status: "complete".to_string(),
            result: None,
            model: None,
            error: None,
        };
        assert!(response.is_complete());

        let response = ProsaSttResponse {
            status: "processing".to_string(),
            ..response
        };
        assert!(!response.is_complete());
    }

    #[test]
    fn test_response_is_in_progress() {
        let response = ProsaSttResponse {
            job_id: "test".to_string(),
            status: "created".to_string(),
            result: None,
            model: None,
            error: None,
        };
        assert!(response.is_in_progress());

        let response = ProsaSttResponse {
            status: "processing".to_string(),
            ..response
        };
        assert!(response.is_in_progress());

        let response = ProsaSttResponse {
            status: "complete".to_string(),
            ..response
        };
        assert!(!response.is_in_progress());
    }

    #[test]
    fn test_response_has_error() {
        let response = ProsaSttResponse {
            job_id: "test".to_string(),
            status: "error".to_string(),
            result: None,
            model: None,
            error: None,
        };
        assert!(response.has_error());

        let response = ProsaSttResponse {
            status: "complete".to_string(),
            error: Some(ProsaSttError {
                code: "test".to_string(),
                message: "Test error".to_string(),
            }),
            ..response
        };
        assert!(response.has_error());
    }

    #[test]
    fn test_response_transcription() {
        let response = ProsaSttResponse {
            job_id: "test".to_string(),
            status: "complete".to_string(),
            result: Some(ProsaSttResult {
                data: vec![
                    ProsaSttSegment {
                        transcript: "Hello".to_string(),
                        time_start: 0.0,
                        time_end: 1.0,
                        channel: 0,
                        speaker: None,
                    },
                    ProsaSttSegment {
                        transcript: "world".to_string(),
                        time_start: 1.0,
                        time_end: 2.0,
                        channel: 0,
                        speaker: None,
                    },
                ],
            }),
            model: None,
            error: None,
        };
        assert_eq!(response.transcription(), Some("Hello world".to_string()));
    }

    #[test]
    fn test_response_status_message() {
        let response = ProsaSttResponse {
            job_id: "test".to_string(),
            status: "complete".to_string(),
            result: None,
            model: None,
            error: Some(ProsaSttError {
                code: "auth_invalid_api_key".to_string(),
                message: String::new(),
            }),
        };
        assert_eq!(response.status_message(), "Invalid API key");

        let response = ProsaSttResponse {
            error: Some(ProsaSttError {
                code: "".to_string(),
                message: "Custom error".to_string(),
            }),
            ..response
        };
        assert_eq!(response.status_message(), "Custom error");
    }

    #[test]
    fn test_request_serialization() {
        let request = ProsaSttRequest {
            config: ProsaSttRequestConfig {
                engine: "stt-general".to_string(),
                wait: Some(true),
                speaker_count: None,
                include_filler: Some(false),
                auto_punctuation: Some(true),
                enable_spoken_numerals: Some(true),
            },
            request: ProsaSttRequestData {
                label: Some("test".to_string()),
                data: Some("base64data".to_string()),
                uri: None,
            },
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("stt-general"));
        assert!(json.contains("base64data"));
    }

    #[test]
    fn test_stream_config_serialization() {
        let config = ProsaSttStreamConfig {
            model: "stt-general-online".to_string(),
            label: Some("test".to_string()),
            include_partial: Some(true),
            include_filler: Some(true),
            audio: Some(ProsaSttAudioConfig {
                format: "wav".to_string(),
                channels: Some(1),
                sample_rate: Some(16000),
            }),
        };

        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("stt-general-online"));
        assert!(json.contains("wav"));
        assert!(json.contains("16000"));
        assert!(json.contains("include_filler"));
    }
}
