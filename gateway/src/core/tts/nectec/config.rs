//! Configuration types for NECTEC AI for Thai TTS API (VAJA).
//!
//! This module provides configuration types for Thailand's NECTEC
//! Text-to-Speech service (VAJA9).
//!
//! # Overview
//!
//! NECTEC (National Electronics and Computer Technology Center) provides
//! AI for Thai, a government-backed platform with free Thai language
//! processing services including VAJA TTS.
//!
//! # Audio Output
//!
//! - Format: WAV
//! - Sample Rate: Varies (typically 22050 Hz or 24000 Hz)
//! - Encoding: PCM 16-bit
//!
//! # Limitations
//!
//! - Max 300 characters per request
//! - Long text must be chunked and concatenated

use crate::core::tts::base::TTSConfig;
use serde::{Deserialize, Serialize};
use std::fmt;

// =============================================================================
// Constants
// =============================================================================

/// NECTEC VAJA9 TTS API endpoint.
pub const VAJA9_ENDPOINT: &str = "https://api.aiforthai.in.th/vaja9/synth_audiovisual";

/// Default request timeout in seconds.
pub const DEFAULT_REQUEST_TIMEOUT: u64 = 60;

/// Maximum text length per request.
pub const MAX_TEXT_LENGTH: usize = 300;

/// Default sample rate for output audio.
pub const DEFAULT_SAMPLE_RATE: u32 = 22050;

/// API header name.
pub const API_KEY_HEADER: &str = "Apikey";

/// Library identifier header.
pub const LIB_HEADER: &str = "X-lib";

/// Library identifier value.
pub const LIB_VALUE: &str = "waav-gateway";

// =============================================================================
// Enums
// =============================================================================

/// NECTEC TTS voice selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NectecVoice {
    /// Male voice (speaker ID 0)
    #[default]
    Male,
    /// Female voice (speaker ID 1)
    Female,
}

impl NectecVoice {
    /// Get the speaker ID for this voice.
    pub fn speaker_id(&self) -> i32 {
        match self {
            NectecVoice::Male => 0,
            NectecVoice::Female => 1,
        }
    }

    /// Get voice name for display.
    pub fn as_str(&self) -> &'static str {
        match self {
            NectecVoice::Male => "male",
            NectecVoice::Female => "female",
        }
    }

    /// Parse voice from string (relaxed matching).
    pub fn from_str_relaxed(s: &str) -> Option<Self> {
        let s = s.to_lowercase();
        match s.as_str() {
            "male" | "0" | "m" => Some(NectecVoice::Male),
            "female" | "1" | "f" => Some(NectecVoice::Female),
            _ => None,
        }
    }
}

impl fmt::Display for NectecVoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl std::str::FromStr for NectecVoice {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        NectecVoice::from_str_relaxed(s)
            .ok_or_else(|| format!("Invalid NECTEC voice: {}. Use 'male' or 'female'", s))
    }
}

// =============================================================================
// Configuration
// =============================================================================

/// NECTEC TTS configuration.
#[derive(Debug, Clone)]
pub struct NectecTtsConfig {
    /// API key for authentication.
    pub api_key: String,

    /// Voice to use.
    pub voice: NectecVoice,

    /// Phrase break control (0 = default).
    pub phrase_break: i32,

    /// Audiovisual mode (0 = audio only).
    pub audiovisual: i32,

    /// Request timeout in seconds.
    pub request_timeout_secs: u64,

    /// Whether to automatically chunk long text.
    pub auto_chunk: bool,
}

impl Default for NectecTtsConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            voice: NectecVoice::default(),
            phrase_break: 0,
            audiovisual: 0,
            request_timeout_secs: DEFAULT_REQUEST_TIMEOUT,
            auto_chunk: true,
        }
    }
}

impl NectecTtsConfig {
    /// Create configuration from base TTSConfig.
    pub fn from_base(config: &TTSConfig) -> Result<Self, String> {
        let api_key = config.api_key.clone();

        if api_key.is_empty() {
            return Err("NECTEC API key is required".to_string());
        }

        // Parse voice from config.voice_id field
        let voice = config
            .voice_id
            .as_ref()
            .and_then(|v| NectecVoice::from_str_relaxed(v))
            .unwrap_or_default();

        Ok(Self {
            api_key,
            voice,
            phrase_break: 0,
            audiovisual: 0,
            request_timeout_secs: DEFAULT_REQUEST_TIMEOUT,
            auto_chunk: true,
        })
    }

    /// Validate the configuration.
    pub fn validate(&self) -> Result<(), String> {
        if self.api_key.is_empty() {
            return Err("NECTEC API key is required".to_string());
        }

        Ok(())
    }

    /// Get the API endpoint.
    pub fn endpoint(&self) -> &'static str {
        VAJA9_ENDPOINT
    }
}

// =============================================================================
// Request/Response Types
// =============================================================================

/// VAJA9 TTS API request body.
#[derive(Debug, Clone, Serialize)]
pub struct Vaja9Request {
    /// Text to synthesize.
    pub input_text: String,

    /// Speaker ID (0 = male, 1 = female).
    pub speaker: i32,

    /// Phrase break control.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phrase_break: Option<i32>,

    /// Audiovisual mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audiovisual: Option<i32>,
}

impl Vaja9Request {
    /// Create a new request with the given text and voice.
    pub fn new(text: impl Into<String>, voice: NectecVoice) -> Self {
        Self {
            input_text: text.into(),
            speaker: voice.speaker_id(),
            phrase_break: Some(0),
            audiovisual: Some(0),
        }
    }
}

/// VAJA9 TTS API response.
#[derive(Debug, Clone, Deserialize)]
pub struct Vaja9Response {
    /// URL to download the generated audio.
    pub wav_url: Option<String>,

    /// Error message if any.
    pub error: Option<String>,
}

impl Vaja9Response {
    /// Get the audio URL.
    pub fn audio_url(&self) -> Option<&str> {
        self.wav_url.as_deref()
    }

    /// Check if response has error.
    pub fn has_error(&self) -> bool {
        self.error.is_some()
    }
}

/// NECTEC TTS error types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NectecTtsError {
    /// Invalid or missing API key.
    InvalidApiKey,
    /// Rate limit exceeded.
    RateLimitExceeded,
    /// Text too long.
    TextTooLong(usize),
    /// Empty text.
    EmptyText,
    /// Audio download failed.
    AudioDownloadFailed(String),
    /// Server error.
    ServerError(String),
    /// Unknown error.
    Unknown(String),
}

impl fmt::Display for NectecTtsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NectecTtsError::InvalidApiKey => write!(f, "Invalid or missing NECTEC API key"),
            NectecTtsError::RateLimitExceeded => write!(f, "NECTEC rate limit exceeded"),
            NectecTtsError::TextTooLong(len) => {
                write!(f, "Text too long ({} chars), max is {} chars", len, MAX_TEXT_LENGTH)
            }
            NectecTtsError::EmptyText => write!(f, "Text is empty"),
            NectecTtsError::AudioDownloadFailed(msg) => write!(f, "Failed to download audio: {}", msg),
            NectecTtsError::ServerError(msg) => write!(f, "NECTEC server error: {}", msg),
            NectecTtsError::Unknown(msg) => write!(f, "NECTEC error: {}", msg),
        }
    }
}

impl std::error::Error for NectecTtsError {}

// =============================================================================
// Utility Functions
// =============================================================================

/// Split text into chunks of maximum size.
///
/// This function attempts to split at sentence boundaries when possible,
/// falling back to word boundaries, then character boundaries.
pub fn chunk_text(text: &str, max_len: usize) -> Vec<String> {
    if text.len() <= max_len {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut remaining = text;

    while !remaining.is_empty() {
        if remaining.len() <= max_len {
            chunks.push(remaining.to_string());
            break;
        }

        // Find a good break point within max_len
        let chunk_text = &remaining[..max_len];

        // Try to find sentence break
        let break_point = chunk_text
            .rfind(|c: char| c == '.' || c == '!' || c == '?' || c == '\n')
            .map(|p| p + 1)
            // Try word break
            .or_else(|| chunk_text.rfind(|c: char| c.is_whitespace()))
            // Fall back to max_len
            .unwrap_or(max_len);

        let (chunk, rest) = remaining.split_at(break_point);
        chunks.push(chunk.trim().to_string());
        remaining = rest.trim_start();
    }

    // Filter out empty chunks
    chunks.into_iter().filter(|c| !c.is_empty()).collect()
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = NectecTtsConfig::default();
        assert!(config.api_key.is_empty());
        assert_eq!(config.voice, NectecVoice::Male);
        assert_eq!(config.phrase_break, 0);
        assert_eq!(config.audiovisual, 0);
        assert!(config.auto_chunk);
    }

    #[test]
    fn test_voice_speaker_ids() {
        assert_eq!(NectecVoice::Male.speaker_id(), 0);
        assert_eq!(NectecVoice::Female.speaker_id(), 1);
    }

    #[test]
    fn test_voice_from_str() {
        assert_eq!(NectecVoice::from_str_relaxed("male"), Some(NectecVoice::Male));
        assert_eq!(NectecVoice::from_str_relaxed("female"), Some(NectecVoice::Female));
        assert_eq!(NectecVoice::from_str_relaxed("0"), Some(NectecVoice::Male));
        assert_eq!(NectecVoice::from_str_relaxed("1"), Some(NectecVoice::Female));
        assert_eq!(NectecVoice::from_str_relaxed("m"), Some(NectecVoice::Male));
        assert_eq!(NectecVoice::from_str_relaxed("f"), Some(NectecVoice::Female));
        assert_eq!(NectecVoice::from_str_relaxed("invalid"), None);
    }

    #[test]
    fn test_voice_display() {
        assert_eq!(NectecVoice::Male.to_string(), "male");
        assert_eq!(NectecVoice::Female.to_string(), "female");
    }

    #[test]
    fn test_config_validation_empty_key() {
        let config = NectecTtsConfig::default();
        assert!(config.validate().is_err());
        assert!(config.validate().unwrap_err().contains("API key is required"));
    }

    #[test]
    fn test_config_validation_valid() {
        let config = NectecTtsConfig {
            api_key: "test_key".to_string(),
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_from_base_empty_key() {
        let base = TTSConfig {
            api_key: String::new(),
            ..Default::default()
        };
        assert!(NectecTtsConfig::from_base(&base).is_err());
    }

    #[test]
    fn test_from_base_valid() {
        let base = TTSConfig {
            api_key: "test_key".to_string(),
            voice_id: Some("female".to_string()),
            ..Default::default()
        };
        let config = NectecTtsConfig::from_base(&base).unwrap();
        assert_eq!(config.api_key, "test_key");
        assert_eq!(config.voice, NectecVoice::Female);
    }

    #[test]
    fn test_vaja9_request() {
        let request = Vaja9Request::new("สวัสดีครับ", NectecVoice::Male);
        assert_eq!(request.input_text, "สวัสดีครับ");
        assert_eq!(request.speaker, 0);

        let request = Vaja9Request::new("สวัสดีค่ะ", NectecVoice::Female);
        assert_eq!(request.speaker, 1);
    }

    #[test]
    fn test_vaja9_response() {
        let response = Vaja9Response {
            wav_url: Some("https://api.aiforthai.in.th/temp/audio.wav".to_string()),
            error: None,
        };
        assert_eq!(response.audio_url(), Some("https://api.aiforthai.in.th/temp/audio.wav"));
        assert!(!response.has_error());

        let error_response = Vaja9Response {
            wav_url: None,
            error: Some("Error occurred".to_string()),
        };
        assert!(error_response.has_error());
    }

    #[test]
    fn test_error_display() {
        assert_eq!(
            NectecTtsError::InvalidApiKey.to_string(),
            "Invalid or missing NECTEC API key"
        );
        assert_eq!(
            NectecTtsError::RateLimitExceeded.to_string(),
            "NECTEC rate limit exceeded"
        );
        assert!(NectecTtsError::TextTooLong(500).to_string().contains("500"));
    }

    #[test]
    fn test_chunk_text_short() {
        let text = "Hello";
        let chunks = chunk_text(text, 300);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "Hello");
    }

    #[test]
    fn test_chunk_text_long() {
        let text = "This is a long sentence. Another sentence here. And one more.";
        let chunks = chunk_text(text, 30);
        assert!(chunks.len() > 1);
        for chunk in &chunks {
            assert!(chunk.len() <= 30);
        }
    }

    #[test]
    fn test_chunk_text_exact() {
        let text = "a".repeat(300);
        let chunks = chunk_text(&text, 300);
        assert_eq!(chunks.len(), 1);
    }

    #[test]
    fn test_constants() {
        assert_eq!(MAX_TEXT_LENGTH, 300);
        assert!(!VAJA9_ENDPOINT.is_empty());
        assert!(!API_KEY_HEADER.is_empty());
    }
}
