//! OpenAI Realtime API configuration types.
//!
//! This module contains configuration types for OpenAI's Realtime API:
//! - Model selection
//! - Voice selection
//! - Audio format configuration
//! - Turn detection settings

use serde::{Deserialize, Serialize};

/// OpenAI Realtime API WebSocket endpoint.
pub const OPENAI_REALTIME_URL: &str = "wss://api.openai.com/v1/realtime";

/// Default audio sample rate for OpenAI Realtime API.
pub const OPENAI_REALTIME_SAMPLE_RATE: u32 = 24000;

// =============================================================================
// Models
// =============================================================================

/// Supported OpenAI Realtime models.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum OpenAIRealtimeModel {
    /// gpt-realtime — the current GA realtime model (default).
    #[default]
    #[serde(rename = "gpt-realtime")]
    GptRealtime,
    /// gpt-realtime-2 — reasoning-capable realtime model (honors `reasoning.effort`).
    #[serde(rename = "gpt-realtime-2")]
    GptRealtime2,
    /// gpt-realtime-mini — smaller / lower-latency realtime model.
    #[serde(rename = "gpt-realtime-mini")]
    GptRealtimeMini,
    /// GPT-4o Realtime Preview (DEPRECATED — retained for backward compatibility)
    #[serde(rename = "gpt-4o-realtime-preview")]
    Gpt4oRealtimePreview,
    /// GPT-4o Realtime Preview 2024-10-01
    #[serde(rename = "gpt-4o-realtime-preview-2024-10-01")]
    Gpt4oRealtimePreview20241001,
    /// GPT-4o Realtime Preview 2024-12-17
    #[serde(rename = "gpt-4o-realtime-preview-2024-12-17")]
    Gpt4oRealtimePreview20241217,
    /// GPT-4o Mini Realtime Preview
    #[serde(rename = "gpt-4o-mini-realtime-preview")]
    Gpt4oMiniRealtimePreview,
    /// GPT-4o Mini Realtime Preview 2024-12-17
    #[serde(rename = "gpt-4o-mini-realtime-preview-2024-12-17")]
    Gpt4oMiniRealtimePreview20241217,
}

impl OpenAIRealtimeModel {
    /// Convert to the API parameter value.
    #[inline]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::GptRealtime => "gpt-realtime",
            Self::GptRealtime2 => "gpt-realtime-2",
            Self::GptRealtimeMini => "gpt-realtime-mini",
            Self::Gpt4oRealtimePreview => "gpt-4o-realtime-preview",
            Self::Gpt4oRealtimePreview20241001 => "gpt-4o-realtime-preview-2024-10-01",
            Self::Gpt4oRealtimePreview20241217 => "gpt-4o-realtime-preview-2024-12-17",
            Self::Gpt4oMiniRealtimePreview => "gpt-4o-mini-realtime-preview",
            Self::Gpt4oMiniRealtimePreview20241217 => "gpt-4o-mini-realtime-preview-2024-12-17",
        }
    }

    /// Parse from string, with fallback to default.
    pub fn from_str_or_default(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "gpt-realtime" => Self::GptRealtime,
            "gpt-realtime-2" => Self::GptRealtime2,
            "gpt-realtime-mini" => Self::GptRealtimeMini,
            "gpt-4o-realtime-preview" => Self::Gpt4oRealtimePreview,
            "gpt-4o-realtime-preview-2024-10-01" => Self::Gpt4oRealtimePreview20241001,
            "gpt-4o-realtime-preview-2024-12-17" => Self::Gpt4oRealtimePreview20241217,
            "gpt-4o-mini-realtime-preview" => Self::Gpt4oMiniRealtimePreview,
            "gpt-4o-mini-realtime-preview-2024-12-17" => Self::Gpt4oMiniRealtimePreview20241217,
            _ => Self::default(),
        }
    }
}

impl std::fmt::Display for OpenAIRealtimeModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =============================================================================
// Voices
// =============================================================================

/// Available voices for OpenAI Realtime API.
///
/// The Realtime API supports the same voices as the TTS API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OpenAIRealtimeVoice {
    /// Alloy voice (default)
    #[default]
    Alloy,
    /// Ash voice
    Ash,
    /// Ballad voice
    Ballad,
    /// Coral voice
    Coral,
    /// Echo voice
    Echo,
    /// Sage voice
    Sage,
    /// Shimmer voice
    Shimmer,
    /// Verse voice
    Verse,
    /// Marin voice (newest; recommended for gpt-realtime)
    Marin,
    /// Cedar voice (newest; recommended for gpt-realtime)
    Cedar,
}

impl OpenAIRealtimeVoice {
    /// Convert to the API parameter value.
    #[inline]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Alloy => "alloy",
            Self::Ash => "ash",
            Self::Ballad => "ballad",
            Self::Coral => "coral",
            Self::Echo => "echo",
            Self::Sage => "sage",
            Self::Shimmer => "shimmer",
            Self::Verse => "verse",
            Self::Marin => "marin",
            Self::Cedar => "cedar",
        }
    }

    /// Parse from string, with fallback to default.
    pub fn from_str_or_default(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "alloy" => Self::Alloy,
            "ash" => Self::Ash,
            "ballad" => Self::Ballad,
            "coral" => Self::Coral,
            "echo" => Self::Echo,
            "sage" => Self::Sage,
            "shimmer" => Self::Shimmer,
            "verse" => Self::Verse,
            "marin" => Self::Marin,
            "cedar" => Self::Cedar,
            _ => Self::default(),
        }
    }

    /// Get all available voices.
    pub fn all() -> &'static [OpenAIRealtimeVoice] {
        &[
            Self::Alloy,
            Self::Ash,
            Self::Ballad,
            Self::Coral,
            Self::Echo,
            Self::Sage,
            Self::Shimmer,
            Self::Verse,
            Self::Marin,
            Self::Cedar,
        ]
    }
}

impl std::fmt::Display for OpenAIRealtimeVoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =============================================================================
// Audio Formats
// =============================================================================

/// Supported audio formats for OpenAI Realtime API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OpenAIRealtimeAudioFormat {
    /// PCM 16-bit signed little-endian (default)
    #[default]
    Pcm16,
    /// G.711 u-law (8-bit)
    #[serde(rename = "g711_ulaw")]
    G711Ulaw,
    /// G.711 a-law (8-bit)
    #[serde(rename = "g711_alaw")]
    G711Alaw,
}

impl OpenAIRealtimeAudioFormat {
    /// Convert to the API parameter value.
    #[inline]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pcm16 => "pcm16",
            Self::G711Ulaw => "g711_ulaw",
            Self::G711Alaw => "g711_alaw",
        }
    }

    /// Get the sample rate for this format.
    #[inline]
    pub fn sample_rate(&self) -> u32 {
        match self {
            Self::Pcm16 => 24000,
            Self::G711Ulaw | Self::G711Alaw => 8000,
        }
    }

    /// Bytes per millisecond of audio (B-G2 truncate math, review
    /// wf_d43814c3 #6): PCM16 = 2 bytes/sample @24kHz = 48 B/ms; G.711 =
    /// 1 byte/sample @8kHz = 8 B/ms. Hardcoding 48 over-truncated telephony
    /// sessions 6×.
    #[inline]
    pub fn bytes_per_ms(&self) -> u64 {
        match self {
            Self::Pcm16 => 48,
            Self::G711Ulaw | Self::G711Alaw => 8,
        }
    }

    /// Parse from string, with fallback to default.
    pub fn from_str_or_default(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "pcm16" | "pcm" | "linear16" => Self::Pcm16,
            "g711_ulaw" | "ulaw" | "mulaw" => Self::G711Ulaw,
            "g711_alaw" | "alaw" => Self::G711Alaw,
            _ => Self::default(),
        }
    }
}

impl std::fmt::Display for OpenAIRealtimeAudioFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =============================================================================
// Modalities
// =============================================================================

/// Output modalities for OpenAI Realtime API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Modality {
    /// Text output only
    Text,
    /// Audio output only
    Audio,
}

impl Modality {
    /// Convert to the API parameter value.
    #[inline]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Audio => "audio",
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_as_str() {
        assert_eq!(
            OpenAIRealtimeModel::Gpt4oRealtimePreview.as_str(),
            "gpt-4o-realtime-preview"
        );
        assert_eq!(
            OpenAIRealtimeModel::Gpt4oMiniRealtimePreview.as_str(),
            "gpt-4o-mini-realtime-preview"
        );
    }

    #[test]
    fn test_model_from_str() {
        assert_eq!(
            OpenAIRealtimeModel::from_str_or_default("gpt-realtime"),
            OpenAIRealtimeModel::GptRealtime
        );
        assert_eq!(
            OpenAIRealtimeModel::from_str_or_default("gpt-realtime-2"),
            OpenAIRealtimeModel::GptRealtime2
        );
        assert_eq!(
            OpenAIRealtimeModel::from_str_or_default("gpt-4o-realtime-preview"),
            OpenAIRealtimeModel::Gpt4oRealtimePreview
        );
        // Default is now the GA gpt-realtime (the deprecated preview is retired).
        assert_eq!(
            OpenAIRealtimeModel::from_str_or_default("unknown"),
            OpenAIRealtimeModel::GptRealtime
        );
    }

    #[test]
    fn test_voice_as_str() {
        assert_eq!(OpenAIRealtimeVoice::Alloy.as_str(), "alloy");
        assert_eq!(OpenAIRealtimeVoice::Shimmer.as_str(), "shimmer");
    }

    #[test]
    fn test_voice_from_str() {
        assert_eq!(
            OpenAIRealtimeVoice::from_str_or_default("alloy"),
            OpenAIRealtimeVoice::Alloy
        );
        assert_eq!(
            OpenAIRealtimeVoice::from_str_or_default("SHIMMER"),
            OpenAIRealtimeVoice::Shimmer
        );
        assert_eq!(
            OpenAIRealtimeVoice::from_str_or_default("unknown"),
            OpenAIRealtimeVoice::Alloy
        );
    }

    #[test]
    fn test_voice_all() {
        let voices = OpenAIRealtimeVoice::all();
        assert_eq!(voices.len(), 10);
        assert!(voices.contains(&OpenAIRealtimeVoice::Alloy));
        assert!(voices.contains(&OpenAIRealtimeVoice::Verse));
        assert!(voices.contains(&OpenAIRealtimeVoice::Marin));
        assert!(voices.contains(&OpenAIRealtimeVoice::Cedar));
    }

    #[test]
    fn test_audio_format_sample_rate() {
        assert_eq!(OpenAIRealtimeAudioFormat::Pcm16.sample_rate(), 24000);
        assert_eq!(OpenAIRealtimeAudioFormat::G711Ulaw.sample_rate(), 8000);
        assert_eq!(OpenAIRealtimeAudioFormat::G711Alaw.sample_rate(), 8000);
    }

    #[test]
    fn test_audio_format_from_str() {
        assert_eq!(
            OpenAIRealtimeAudioFormat::from_str_or_default("pcm16"),
            OpenAIRealtimeAudioFormat::Pcm16
        );
        assert_eq!(
            OpenAIRealtimeAudioFormat::from_str_or_default("linear16"),
            OpenAIRealtimeAudioFormat::Pcm16
        );
        assert_eq!(
            OpenAIRealtimeAudioFormat::from_str_or_default("g711_ulaw"),
            OpenAIRealtimeAudioFormat::G711Ulaw
        );
    }
}
