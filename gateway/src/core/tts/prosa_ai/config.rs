//! Configuration types for Prosa.ai TTS API.
//!
//! This module provides configuration types for Prosa.ai's
//! Text-to-Speech service optimized for Indonesian language.
//!
//! # Overview
//!
//! Prosa.ai TTS is an Indonesian AI service providing natural-sounding
//! speech synthesis with multiple voices and formats.
//!
//! # Features
//!
//! - 40+ Indonesian voices (various accents and styles)
//! - English voice support
//! - Adjustable pitch and tempo
//! - Multiple audio formats (opus, mp3, wav)
//! - Synchronous and asynchronous synthesis
//!
//! # Authentication
//!
//! The API uses API key authentication via `x-api-key` header.
//! Obtain keys from https://console.prosa.ai

use crate::core::tts::base::{TTSConfig, TTSError};
use serde::{Deserialize, Serialize};
use std::fmt;

// =============================================================================
// Constants
// =============================================================================

/// Prosa.ai TTS REST API base URL.
pub const PROSA_TTS_BASE_URL: &str = "https://api.prosa.ai/v2/speech/tts";

/// Default request timeout in seconds.
pub const DEFAULT_REQUEST_TIMEOUT: u64 = 60;

/// Default audio sample rate (Hz).
pub const DEFAULT_SAMPLE_RATE: u32 = 48000;

/// Default pitch offset.
pub const DEFAULT_PITCH: i32 = 0;

/// Minimum pitch offset.
pub const MIN_PITCH: i32 = -10;

/// Maximum pitch offset.
pub const MAX_PITCH: i32 = 10;

/// Default tempo/speed.
pub const DEFAULT_TEMPO: f32 = 1.0;

/// Minimum tempo.
pub const MIN_TEMPO: f32 = 0.5;

/// Maximum tempo.
pub const MAX_TEMPO: f32 = 2.0;

/// Maximum text length for synchronous requests.
pub const MAX_SYNC_TEXT_LENGTH: usize = 280;

/// Maximum text length for asynchronous requests.
pub const MAX_ASYNC_TEXT_LENGTH: usize = 5000;

/// Minimum text length.
pub const MIN_TEXT_LENGTH: usize = 1;

// =============================================================================
// Voice Types
// =============================================================================

/// Prosa.ai TTS voice selection.
///
/// Prosa.ai offers 40+ voices including:
/// - Indonesian voices (formal, friendly, expressive)
/// - English voices
/// - Various styles (news, podcast, audiobook)
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ProsaTtsVoice {
    /// Dimas Formal - Male Indonesian formal voice.
    #[default]
    DimasFormal,
    /// Dimas Expressive - Male Indonesian expressive voice.
    DimasExpressive,
    /// Ocha Friendly - Female Indonesian friendly voice.
    OchaFriendly,
    /// Dini - Female Indonesian audiobook voice.
    Dini,
    /// Kinanti - Female Indonesian podcast voice.
    Kinanti,
    /// Darah - Female Indonesian formal voice.
    Darah,
    /// Abimana - Male Indonesian virtual assistant voice.
    Abimana,
    /// Roger - Male English news voice.
    Roger,
    /// Jennifer - Female English news voice.
    Jennifer,
    /// Custom voice ID.
    Custom(String),
}

impl ProsaTtsVoice {
    /// Get the model ID string for the API.
    pub fn model_id(&self) -> &str {
        match self {
            Self::DimasFormal => "tts-dimas-formal",
            Self::DimasExpressive => "tts-dimas-expressive",
            Self::OchaFriendly => "tts-ocha-friendly",
            Self::Dini => "tts-dini",
            Self::Kinanti => "tts-kinanti",
            Self::Darah => "tts-darah",
            Self::Abimana => "tts-abimana",
            Self::Roger => "tts-roger",
            Self::Jennifer => "tts-jennifer",
            Self::Custom(id) => id.as_str(),
        }
    }

    /// Get the display name for the voice.
    pub fn display_name(&self) -> String {
        match self {
            Self::DimasFormal => "Dimas (Formal, Male, Indonesian)".to_string(),
            Self::DimasExpressive => "Dimas (Expressive, Male, Indonesian)".to_string(),
            Self::OchaFriendly => "Ocha (Friendly, Female, Indonesian)".to_string(),
            Self::Dini => "Dini (Audiobook, Female, Indonesian)".to_string(),
            Self::Kinanti => "Kinanti (Podcast, Female, Indonesian)".to_string(),
            Self::Darah => "Darah (Formal, Female, Indonesian)".to_string(),
            Self::Abimana => "Abimana (Virtual Assistant, Male, Indonesian)".to_string(),
            Self::Roger => "Roger (News, Male, English)".to_string(),
            Self::Jennifer => "Jennifer (News, Female, English)".to_string(),
            Self::Custom(id) => format!("Custom ({})", id),
        }
    }

    /// Get the primary language for the voice.
    pub fn language(&self) -> &'static str {
        match self {
            Self::Roger | Self::Jennifer => "en",
            _ => "id",
        }
    }

    /// Get the gender for the voice.
    pub fn gender(&self) -> &'static str {
        match self {
            Self::DimasFormal | Self::DimasExpressive | Self::Abimana | Self::Roger => "male",
            Self::Custom(_) => "unknown",
            _ => "female",
        }
    }

    /// Get the voice domain/style.
    pub fn domain(&self) -> &'static str {
        match self {
            Self::DimasFormal | Self::Darah => "formal",
            Self::DimasExpressive => "expressive",
            Self::OchaFriendly => "friendly",
            Self::Dini => "audiobook",
            Self::Kinanti => "podcast",
            Self::Abimana => "virtual_assistant",
            Self::Roger | Self::Jennifer => "news",
            Self::Custom(_) => "unknown",
        }
    }

    /// Parse voice from string.
    ///
    /// Accepts various formats:
    /// - Model IDs: "tts-dimas-formal"
    /// - Short names: "dimas-formal", "dimas_formal"
    /// - Just names: "dimas", "ocha", "roger"
    pub fn from_str_relaxed(s: &str) -> Self {
        let s_lower = s.trim().to_lowercase().replace([' ', '-'], "_");

        match s_lower.as_str() {
            // Dimas voices
            "tts_dimas_formal" | "dimas_formal" | "dimas" | "default" => Self::DimasFormal,
            "tts_dimas_expressive" | "dimas_expressive" => Self::DimasExpressive,

            // Ocha
            "tts_ocha_friendly" | "ocha_friendly" | "ocha" => Self::OchaFriendly,

            // Other Indonesian voices
            "tts_dini" | "dini" => Self::Dini,
            "tts_kinanti" | "kinanti" => Self::Kinanti,
            "tts_darah" | "darah" => Self::Darah,
            "tts_abimana" | "abimana" => Self::Abimana,

            // English voices
            "tts_roger" | "roger" | "english_male" => Self::Roger,
            "tts_jennifer" | "jennifer" | "english_female" => Self::Jennifer,

            // Custom/unknown - pass through as custom
            _ => Self::Custom(s.to_string()),
        }
    }

    /// Get all known voices.
    pub fn all() -> Vec<Self> {
        vec![
            Self::DimasFormal,
            Self::DimasExpressive,
            Self::OchaFriendly,
            Self::Dini,
            Self::Kinanti,
            Self::Darah,
            Self::Abimana,
            Self::Roger,
            Self::Jennifer,
        ]
    }
}

impl fmt::Display for ProsaTtsVoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.model_id())
    }
}

// =============================================================================
// Audio Format
// =============================================================================

/// Prosa.ai TTS audio output format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ProsaTtsAudioFormat {
    /// Opus format (default, most efficient).
    #[default]
    Opus,
    /// MP3 format.
    Mp3,
    /// WAV format.
    Wav,
}

impl ProsaTtsAudioFormat {
    /// Get the format string for the API.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Opus => "opus",
            Self::Mp3 => "mp3",
            Self::Wav => "wav",
        }
    }

    /// Get the MIME type for the format.
    pub fn mime_type(&self) -> &'static str {
        match self {
            Self::Opus => "audio/opus",
            Self::Mp3 => "audio/mpeg",
            Self::Wav => "audio/wav",
        }
    }

    /// Get the sample rate for the format.
    pub fn sample_rate(&self) -> u32 {
        DEFAULT_SAMPLE_RATE
    }

    /// Parse format from string.
    pub fn from_str_relaxed(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "opus" | "audio/opus" => Self::Opus,
            "mp3" | "audio/mpeg" | "audio/mp3" => Self::Mp3,
            "wav" | "audio/wav" | "audio/wave" => Self::Wav,
            _ => Self::Opus,
        }
    }
}

impl fmt::Display for ProsaTtsAudioFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =============================================================================
// Configuration
// =============================================================================

/// Prosa.ai TTS configuration.
#[derive(Debug, Clone)]
pub struct ProsaTtsConfig {
    /// API key for authentication.
    pub api_key: String,

    /// Voice/model selection.
    pub voice: ProsaTtsVoice,

    /// Audio output format.
    pub audio_format: ProsaTtsAudioFormat,

    /// Pitch offset (-10 to 10).
    pub pitch: i32,

    /// Tempo/speed (0.5 to 2.0).
    pub tempo: f32,

    /// Whether to wait for result synchronously.
    pub wait: bool,

    /// Return audio as signed URL (2hr expiry).
    pub as_signed_url: bool,

    /// Request timeout in seconds.
    pub request_timeout_secs: u64,

    /// Optional label for the request.
    pub label: Option<String>,

    /// Optional base-URL override for the synth endpoint (mock e2e redirect).
    pub endpoint_override: Option<String>,
}

impl Default for ProsaTtsConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            voice: ProsaTtsVoice::default(),
            audio_format: ProsaTtsAudioFormat::default(),
            pitch: DEFAULT_PITCH,
            tempo: DEFAULT_TEMPO,
            wait: true,
            as_signed_url: false,
            request_timeout_secs: DEFAULT_REQUEST_TIMEOUT,
            label: None,
            endpoint_override: None,
        }
    }
}

impl ProsaTtsConfig {
    /// Create configuration from base TTSConfig.
    pub fn from_base(config: TTSConfig) -> Result<Self, TTSError> {
        let api_key = config.api_key.clone();

        if api_key.is_empty() {
            return Err(TTSError::AuthenticationFailed(
                "Prosa.ai API key is required".to_string(),
            ));
        }

        // Parse voice from voice_id or model field
        let voice = if let Some(ref voice_id) = config.voice_id {
            if !voice_id.is_empty() {
                ProsaTtsVoice::from_str_relaxed(voice_id)
            } else if !config.model.is_empty() {
                ProsaTtsVoice::from_str_relaxed(&config.model)
            } else {
                ProsaTtsVoice::default()
            }
        } else if !config.model.is_empty() {
            ProsaTtsVoice::from_str_relaxed(&config.model)
        } else {
            ProsaTtsVoice::default()
        };

        // Parse audio format
        let audio_format = config
            .audio_format
            .as_ref()
            .map(|f| ProsaTtsAudioFormat::from_str_relaxed(f))
            .unwrap_or_default();

        // Parse tempo/speed from speaking_rate
        let tempo = config
            .speaking_rate
            .map(|rate| rate.clamp(MIN_TEMPO, MAX_TEMPO))
            .unwrap_or(DEFAULT_TEMPO);

        // Get request timeout
        let request_timeout_secs = config.request_timeout.unwrap_or(DEFAULT_REQUEST_TIMEOUT);

        Ok(Self {
            api_key,
            voice,
            audio_format,
            pitch: DEFAULT_PITCH,
            tempo,
            wait: true,
            as_signed_url: false,
            request_timeout_secs,
            label: None,
            endpoint_override: None,
        })
    }

    /// Build from the standardized TTS config (W1 keystone).
    ///
    /// Prosa.ai exposes a narrow prosody surface, so only `speed` (→ `tempo`, clamped to
    /// `MIN_TEMPO..=MAX_TEMPO`) and `pitch` (→ Prosa's integer `pitch` offset, rounded and clamped
    /// to `MIN_PITCH..=MAX_PITCH`) map to real fields. Non-standard Prosa knobs (`label`, `wait`,
    /// `as_signed_url`) are read from the open `extras` passthrough. Voice-tone features
    /// (volume, stability, similarity_boost, style, use_speaker_boost, emotion, instructions, SSML,
    /// language, word_timestamps, streaming, seed, sample_rate) have no matching field and are
    /// skipped.
    pub fn from_standard(
        std: &crate::core::tts::standard::StandardTTSConfig,
    ) -> Result<Self, TTSError> {
        let f = &std.features;
        let mut cfg = Self::from_base(std.base.clone())?;

        if let Some(speed) = f.speed {
            cfg.tempo = speed.clamp(MIN_TEMPO, MAX_TEMPO);
        }
        if let Some(pitch) = f.pitch {
            cfg.pitch = (pitch.round() as i32).clamp(MIN_PITCH, MAX_PITCH);
        }

        // Provider-specific passthrough.
        if let Some(label) = std
            .extras
            .0
            .get("label")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            cfg.label = Some(label.to_string());
        }
        if let Some(wait) = std.extras.0.get("wait").and_then(|v| v.as_bool()) {
            cfg.wait = wait;
        }
        if let Some(as_signed_url) = std.extras.0.get("as_signed_url").and_then(|v| v.as_bool()) {
            cfg.as_signed_url = as_signed_url;
        }

        cfg.endpoint_override = std.endpoint_override().map(String::from);

        Ok(cfg)
    }

    /// Validate the configuration.
    pub fn validate(&self) -> Result<(), String> {
        if self.api_key.is_empty() {
            return Err("Prosa.ai API key is required".to_string());
        }

        if self.pitch < MIN_PITCH || self.pitch > MAX_PITCH {
            return Err(format!(
                "Pitch must be between {} and {}, got {}",
                MIN_PITCH, MAX_PITCH, self.pitch
            ));
        }

        if self.tempo < MIN_TEMPO || self.tempo > MAX_TEMPO {
            return Err(format!(
                "Tempo must be between {} and {}, got {}",
                MIN_TEMPO, MAX_TEMPO, self.tempo
            ));
        }

        Ok(())
    }

    /// Validate text for synthesis.
    pub fn validate_text(&self, text: &str) -> Result<(), TTSError> {
        let char_count = text.chars().count();

        if char_count < MIN_TEXT_LENGTH {
            return Err(TTSError::InvalidConfiguration(format!(
                "Text must be at least {} characters, got {}",
                MIN_TEXT_LENGTH, char_count
            )));
        }

        let max_length = if self.wait {
            MAX_SYNC_TEXT_LENGTH
        } else {
            MAX_ASYNC_TEXT_LENGTH
        };

        if char_count > max_length {
            return Err(TTSError::InvalidConfiguration(format!(
                "Text exceeds maximum length of {} characters for {} mode, got {}",
                max_length,
                if self.wait {
                    "synchronous"
                } else {
                    "asynchronous"
                },
                char_count
            )));
        }

        Ok(())
    }
}

// =============================================================================
// Request/Response Types
// =============================================================================

/// Prosa.ai TTS request configuration.
#[derive(Debug, Clone, Serialize)]
pub struct ProsaTtsRequestConfig {
    /// TTS model/voice to use.
    pub model: String,

    /// Wait for result synchronously.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wait: Option<bool>,

    /// Pitch offset (-10 to 10).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pitch: Option<i32>,

    /// Speech speed/tempo (0.5 to 2.0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tempo: Option<f32>,

    /// Audio output format.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_format: Option<String>,

    /// Return audio as signed URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub as_signed_url: Option<bool>,
}

/// Prosa.ai TTS request data.
#[derive(Debug, Clone, Serialize)]
pub struct ProsaTtsRequestData {
    /// Optional request label.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,

    /// Text to synthesize.
    pub text: String,
}

/// Prosa.ai TTS REST API request body.
#[derive(Debug, Clone, Serialize)]
pub struct ProsaTtsRequest {
    /// Configuration options.
    pub config: ProsaTtsRequestConfig,

    /// Request data.
    pub request: ProsaTtsRequestData,
}

/// Prosa.ai TTS API response model info.
#[derive(Debug, Clone, Deserialize)]
pub struct ProsaTtsModelInfo {
    /// Model ID.
    #[serde(default)]
    pub id: String,

    /// Model language.
    #[serde(default)]
    pub language: String,

    /// Model gender.
    #[serde(default)]
    pub gender: String,
}

/// Prosa.ai TTS API response result.
#[derive(Debug, Clone, Deserialize)]
pub struct ProsaTtsResult {
    /// Base64 encoded audio data (when not using signed URL).
    #[serde(default)]
    pub data: String,

    /// Audio duration in seconds.
    #[serde(default)]
    pub duration: f64,

    /// Audio sample rate.
    #[serde(default)]
    pub sample_rate: u32,

    /// Audio channels.
    #[serde(default)]
    pub channels: u32,

    /// Signed URL for audio (when as_signed_url=true).
    #[serde(default)]
    pub url: String,
}

/// Prosa.ai TTS API error response.
#[derive(Debug, Clone, Deserialize)]
pub struct ProsaTtsError {
    /// Error code.
    #[serde(default)]
    pub code: String,

    /// Error message.
    #[serde(default)]
    pub message: String,
}

/// Prosa.ai TTS API response.
#[derive(Debug, Clone, Deserialize)]
pub struct ProsaTtsResponse {
    /// Job ID.
    #[serde(default)]
    pub job_id: String,

    /// Job status (created, processing, complete, error).
    #[serde(default)]
    pub status: String,

    /// Result data.
    #[serde(default)]
    pub result: Option<ProsaTtsResult>,

    /// Model info.
    #[serde(default)]
    pub model: Option<ProsaTtsModelInfo>,

    /// Error info.
    #[serde(default)]
    pub error: Option<ProsaTtsError>,
}

impl ProsaTtsResponse {
    /// Check if the response is complete.
    pub fn is_complete(&self) -> bool {
        self.status.to_lowercase() == "complete"
    }

    /// Check if the response indicates an error.
    pub fn has_error(&self) -> bool {
        self.status.to_lowercase() == "error" || self.error.is_some()
    }

    /// Check if the job is still in progress.
    pub fn is_in_progress(&self) -> bool {
        let status = self.status.to_lowercase();
        status == "created" || status == "processing"
    }

    /// Get audio data if available.
    pub fn audio_data(&self) -> Option<&str> {
        self.result.as_ref().map(|r| r.data.as_str())
    }

    /// Get audio URL if available.
    pub fn audio_url(&self) -> Option<&str> {
        self.result
            .as_ref()
            .filter(|r| !r.url.is_empty())
            .map(|r| r.url.as_str())
    }

    /// Get audio duration if available.
    pub fn duration(&self) -> Option<f64> {
        self.result.as_ref().map(|r| r.duration)
    }

    /// Get status message for errors.
    pub fn status_message(&self) -> &str {
        if let Some(ref err) = self.error {
            if !err.message.is_empty() {
                return &err.message;
            }
            match err.code.as_str() {
                "auth_invalid_api_key" => "Invalid API key",
                "auth_unauthorized" => "Unauthorized",
                "forbidden" => "Access forbidden",
                "quota_insufficient" => "Insufficient quota",
                "quota_empty" => "Out of quota",
                "tts_model_not_found" => "TTS model not found",
                "tts_request_text_too_long" => "Text too long",
                "tts_request_invalid_pitch" => "Invalid pitch value",
                "tts_request_invalid_tempo" => "Invalid tempo value",
                _ => "Unknown error",
            }
        } else if self.status.to_lowercase() == "error" {
            "Processing error"
        } else {
            "Success"
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // W1 keystone: the standardized features Prosa.ai can express (speed → tempo, pitch → the
    // integer pitch offset) reach their real fields, and the non-standard label / wait /
    // as_signed_url knobs flow through the open extras passthrough.
    #[test]
    fn from_standard_maps_speed_pitch_and_extras() {
        use crate::core::tts::standard::{ProviderExtras, StandardTTSConfig, TtsFeatures};
        let mut extras = serde_json::Map::new();
        extras.insert("label".into(), serde_json::json!("greeting"));
        extras.insert("as_signed_url".into(), serde_json::json!(true));
        let std = StandardTTSConfig {
            base: TTSConfig {
                provider: "prosa-ai".into(),
                api_key: "k".into(),
                ..Default::default()
            },
            features: TtsFeatures {
                speed: Some(1.5),
                pitch: Some(3.0),
                ssml: Some(true), // capability gap: Prosa has no SSML, must be ignored
                ..Default::default()
            },
            extras: ProviderExtras(extras),
        };
        let cfg = ProsaTtsConfig::from_standard(&std).unwrap();
        assert!((cfg.tempo - 1.5).abs() < f32::EPSILON);
        assert_eq!(cfg.pitch, 3);
        assert_eq!(cfg.label, Some("greeting".to_string())); // extras passthrough
        assert!(cfg.as_signed_url);
    }

    #[test]
    fn test_voice_model_ids() {
        assert_eq!(ProsaTtsVoice::DimasFormal.model_id(), "tts-dimas-formal");
        assert_eq!(ProsaTtsVoice::OchaFriendly.model_id(), "tts-ocha-friendly");
        assert_eq!(ProsaTtsVoice::Roger.model_id(), "tts-roger");
    }

    #[test]
    fn test_voice_display_names() {
        assert!(ProsaTtsVoice::DimasFormal.display_name().contains("Dimas"));
        assert!(ProsaTtsVoice::OchaFriendly.display_name().contains("Ocha"));
        assert!(ProsaTtsVoice::Roger.display_name().contains("English"));
    }

    #[test]
    fn test_voice_language() {
        assert_eq!(ProsaTtsVoice::DimasFormal.language(), "id");
        assert_eq!(ProsaTtsVoice::Roger.language(), "en");
        assert_eq!(ProsaTtsVoice::Jennifer.language(), "en");
    }

    #[test]
    fn test_voice_gender() {
        assert_eq!(ProsaTtsVoice::DimasFormal.gender(), "male");
        assert_eq!(ProsaTtsVoice::OchaFriendly.gender(), "female");
        assert_eq!(ProsaTtsVoice::Roger.gender(), "male");
        assert_eq!(ProsaTtsVoice::Jennifer.gender(), "female");
    }

    #[test]
    fn test_voice_from_str_default() {
        assert_eq!(
            ProsaTtsVoice::from_str_relaxed("dimas"),
            ProsaTtsVoice::DimasFormal
        );
        assert_eq!(
            ProsaTtsVoice::from_str_relaxed("default"),
            ProsaTtsVoice::DimasFormal
        );
    }

    #[test]
    fn test_voice_from_str_variations() {
        assert_eq!(
            ProsaTtsVoice::from_str_relaxed("tts-dimas-formal"),
            ProsaTtsVoice::DimasFormal
        );
        assert_eq!(
            ProsaTtsVoice::from_str_relaxed("dimas_formal"),
            ProsaTtsVoice::DimasFormal
        );
        assert_eq!(
            ProsaTtsVoice::from_str_relaxed("ocha-friendly"),
            ProsaTtsVoice::OchaFriendly
        );
    }

    #[test]
    fn test_voice_from_str_english() {
        assert_eq!(
            ProsaTtsVoice::from_str_relaxed("roger"),
            ProsaTtsVoice::Roger
        );
        assert_eq!(
            ProsaTtsVoice::from_str_relaxed("english_male"),
            ProsaTtsVoice::Roger
        );
        assert_eq!(
            ProsaTtsVoice::from_str_relaxed("jennifer"),
            ProsaTtsVoice::Jennifer
        );
    }

    #[test]
    fn test_voice_from_str_custom() {
        let custom = ProsaTtsVoice::from_str_relaxed("some_custom_voice");
        match custom {
            ProsaTtsVoice::Custom(id) => assert_eq!(id, "some_custom_voice"),
            _ => panic!("Expected Custom variant"),
        }
    }

    #[test]
    fn test_all_voices() {
        let voices = ProsaTtsVoice::all();
        assert_eq!(voices.len(), 9);
        assert!(voices.contains(&ProsaTtsVoice::DimasFormal));
        assert!(voices.contains(&ProsaTtsVoice::Roger));
    }

    #[test]
    fn test_audio_format_as_str() {
        assert_eq!(ProsaTtsAudioFormat::Opus.as_str(), "opus");
        assert_eq!(ProsaTtsAudioFormat::Mp3.as_str(), "mp3");
        assert_eq!(ProsaTtsAudioFormat::Wav.as_str(), "wav");
    }

    #[test]
    fn test_audio_format_mime_type() {
        assert_eq!(ProsaTtsAudioFormat::Opus.mime_type(), "audio/opus");
        assert_eq!(ProsaTtsAudioFormat::Mp3.mime_type(), "audio/mpeg");
        assert_eq!(ProsaTtsAudioFormat::Wav.mime_type(), "audio/wav");
    }

    #[test]
    fn test_audio_format_from_str() {
        assert_eq!(
            ProsaTtsAudioFormat::from_str_relaxed("opus"),
            ProsaTtsAudioFormat::Opus
        );
        assert_eq!(
            ProsaTtsAudioFormat::from_str_relaxed("mp3"),
            ProsaTtsAudioFormat::Mp3
        );
        assert_eq!(
            ProsaTtsAudioFormat::from_str_relaxed("wav"),
            ProsaTtsAudioFormat::Wav
        );
        assert_eq!(
            ProsaTtsAudioFormat::from_str_relaxed("unknown"),
            ProsaTtsAudioFormat::Opus
        );
    }

    #[test]
    fn test_config_default() {
        let config = ProsaTtsConfig::default();
        assert!(config.api_key.is_empty());
        assert_eq!(config.voice, ProsaTtsVoice::DimasFormal);
        assert_eq!(config.audio_format, ProsaTtsAudioFormat::Opus);
        assert_eq!(config.pitch, DEFAULT_PITCH);
        assert!((config.tempo - DEFAULT_TEMPO).abs() < f32::EPSILON);
    }

    #[test]
    fn test_config_validation_empty_key() {
        let config = ProsaTtsConfig::default();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_invalid_pitch() {
        let mut config = ProsaTtsConfig::default();
        config.api_key = "test_key".to_string();
        config.pitch = 20;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_invalid_tempo() {
        let mut config = ProsaTtsConfig::default();
        config.api_key = "test_key".to_string();
        config.tempo = 5.0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_success() {
        let mut config = ProsaTtsConfig::default();
        config.api_key = "test_key".to_string();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_from_base_empty_key() {
        let config = TTSConfig {
            api_key: String::new(),
            ..Default::default()
        };
        assert!(ProsaTtsConfig::from_base(config).is_err());
    }

    #[test]
    fn test_config_from_base_with_voice() {
        let config = TTSConfig {
            api_key: "test_key".to_string(),
            voice_id: Some("ocha".to_string()),
            ..Default::default()
        };
        let prosa_config = ProsaTtsConfig::from_base(config).unwrap();
        assert_eq!(prosa_config.voice, ProsaTtsVoice::OchaFriendly);
    }

    #[test]
    fn test_config_from_base_with_format() {
        let config = TTSConfig {
            api_key: "test_key".to_string(),
            audio_format: Some("mp3".to_string()),
            ..Default::default()
        };
        let prosa_config = ProsaTtsConfig::from_base(config).unwrap();
        assert_eq!(prosa_config.audio_format, ProsaTtsAudioFormat::Mp3);
    }

    #[test]
    fn test_config_from_base_with_speed() {
        let config = TTSConfig {
            api_key: "test_key".to_string(),
            speaking_rate: Some(1.5),
            ..Default::default()
        };
        let prosa_config = ProsaTtsConfig::from_base(config).unwrap();
        assert!((prosa_config.tempo - 1.5).abs() < f32::EPSILON);
    }

    #[test]
    fn test_validate_text_empty() {
        let mut config = ProsaTtsConfig::default();
        config.api_key = "test_key".to_string();
        assert!(config.validate_text("").is_err());
    }

    #[test]
    fn test_validate_text_too_long_sync() {
        let mut config = ProsaTtsConfig::default();
        config.api_key = "test_key".to_string();
        config.wait = true;
        let long_text = "a".repeat(MAX_SYNC_TEXT_LENGTH + 1);
        assert!(config.validate_text(&long_text).is_err());
    }

    #[test]
    fn test_validate_text_valid() {
        let mut config = ProsaTtsConfig::default();
        config.api_key = "test_key".to_string();
        assert!(config.validate_text("Selamat pagi").is_ok());
    }

    #[test]
    fn test_response_is_complete() {
        let response = ProsaTtsResponse {
            job_id: "test".to_string(),
            status: "complete".to_string(),
            result: None,
            model: None,
            error: None,
        };
        assert!(response.is_complete());
        assert!(!response.has_error());
    }

    #[test]
    fn test_response_has_error() {
        let response = ProsaTtsResponse {
            job_id: "test".to_string(),
            status: "error".to_string(),
            result: None,
            model: None,
            error: Some(ProsaTtsError {
                code: "test_error".to_string(),
                message: "Test error".to_string(),
            }),
        };
        assert!(response.has_error());
        assert!(!response.is_complete());
    }

    #[test]
    fn test_response_status_message() {
        let response = ProsaTtsResponse {
            job_id: "test".to_string(),
            status: "error".to_string(),
            result: None,
            model: None,
            error: Some(ProsaTtsError {
                code: "auth_invalid_api_key".to_string(),
                message: String::new(),
            }),
        };
        assert_eq!(response.status_message(), "Invalid API key");
    }

    #[test]
    fn test_request_serialization() {
        let request = ProsaTtsRequest {
            config: ProsaTtsRequestConfig {
                model: "tts-dimas-formal".to_string(),
                wait: Some(true),
                pitch: Some(0),
                tempo: Some(1.0),
                audio_format: Some("opus".to_string()),
                as_signed_url: None,
            },
            request: ProsaTtsRequestData {
                label: None,
                text: "Selamat pagi".to_string(),
            },
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("tts-dimas-formal"));
        assert!(json.contains("Selamat pagi"));
    }
}
