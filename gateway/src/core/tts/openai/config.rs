//! Configuration types for OpenAI TTS API.
//!
//! This module contains configuration types for OpenAI's text-to-speech API:
//! - Model selection (tts-1, tts-1-hd, gpt-4o-mini-tts)
//! - Voice selection (11 available voices)
//! - Audio format and speed options

use serde::{Deserialize, Serialize};

// =============================================================================
// OpenAI TTS Models
// =============================================================================

/// Supported OpenAI TTS models.
///
/// OpenAI offers several TTS models:
/// - `tts-1`: Standard quality, lower latency
/// - `tts-1-hd`: High definition quality, higher latency
/// - `gpt-4o-mini-tts`: Latest model with improved quality
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum OpenAITTSModel {
    /// Standard quality TTS model - good balance of quality and latency
    #[default]
    #[serde(rename = "tts-1")]
    Tts1,
    /// High definition TTS model - best quality, higher latency
    #[serde(rename = "tts-1-hd")]
    Tts1Hd,
    /// GPT-4o mini TTS model - latest improvements
    #[serde(rename = "gpt-4o-mini-tts")]
    Gpt4oMiniTts,
}

impl OpenAITTSModel {
    /// Convert to the API parameter value.
    #[inline]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Tts1 => "tts-1",
            Self::Tts1Hd => "tts-1-hd",
            Self::Gpt4oMiniTts => "gpt-4o-mini-tts",
        }
    }

    /// Parse from string, with fallback to default.
    pub fn from_str_or_default(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "tts-1" | "tts1" => Self::Tts1,
            "tts-1-hd" | "tts1-hd" | "tts1hd" => Self::Tts1Hd,
            "gpt-4o-mini-tts" | "gpt4o-mini-tts" => Self::Gpt4oMiniTts,
            _ => Self::default(),
        }
    }
}

impl std::fmt::Display for OpenAITTSModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =============================================================================
// OpenAI TTS Voices
// =============================================================================

/// Available voices for OpenAI TTS.
///
/// OpenAI provides 11 distinct voices with different characteristics:
/// - Alloy, Echo, Fable, Onyx, Nova, Shimmer: Original voices
/// - Ash, Ballad, Coral, Sage, Verse: Additional voices
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OpenAIVoice {
    /// Alloy voice
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
    /// Fable voice
    Fable,
    /// Onyx voice
    Onyx,
    /// Nova voice
    Nova,
    /// Sage voice
    Sage,
    /// Shimmer voice
    Shimmer,
    /// Verse voice
    Verse,
}

impl OpenAIVoice {
    /// Convert to the API parameter value.
    #[inline]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Alloy => "alloy",
            Self::Ash => "ash",
            Self::Ballad => "ballad",
            Self::Coral => "coral",
            Self::Echo => "echo",
            Self::Fable => "fable",
            Self::Onyx => "onyx",
            Self::Nova => "nova",
            Self::Sage => "sage",
            Self::Shimmer => "shimmer",
            Self::Verse => "verse",
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
            "fable" => Self::Fable,
            "onyx" => Self::Onyx,
            "nova" => Self::Nova,
            "sage" => Self::Sage,
            "shimmer" => Self::Shimmer,
            "verse" => Self::Verse,
            _ => Self::default(),
        }
    }

    /// Get all available voices.
    pub fn all() -> &'static [OpenAIVoice] {
        &[
            Self::Alloy,
            Self::Ash,
            Self::Ballad,
            Self::Coral,
            Self::Echo,
            Self::Fable,
            Self::Onyx,
            Self::Nova,
            Self::Sage,
            Self::Shimmer,
            Self::Verse,
        ]
    }
}

impl std::fmt::Display for OpenAIVoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =============================================================================
// Audio Output Format
// =============================================================================

/// Supported audio output formats for OpenAI TTS.
///
/// The default response format is mp3. PCM output is 24kHz mono.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioOutputFormat {
    /// MP3 format (default)
    #[default]
    Mp3,
    /// Opus format
    Opus,
    /// AAC format
    Aac,
    /// FLAC format
    Flac,
    /// WAV format
    Wav,
    /// Raw PCM format (24kHz 16-bit mono little-endian)
    Pcm,
}

impl AudioOutputFormat {
    /// Convert to the API parameter value.
    #[inline]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Mp3 => "mp3",
            Self::Opus => "opus",
            Self::Aac => "aac",
            Self::Flac => "flac",
            Self::Wav => "wav",
            Self::Pcm => "pcm",
        }
    }

    /// Get the MIME type for this format.
    #[inline]
    pub fn mime_type(&self) -> &'static str {
        match self {
            Self::Mp3 => "audio/mpeg",
            Self::Opus => "audio/opus",
            Self::Aac => "audio/aac",
            Self::Flac => "audio/flac",
            Self::Wav => "audio/wav",
            Self::Pcm => "audio/pcm",
        }
    }

    /// Get the sample rate for this format.
    /// Note: PCM is always 24kHz from OpenAI.
    #[inline]
    pub fn sample_rate(&self) -> u32 {
        match self {
            Self::Pcm => 24000,
            _ => 24000, // OpenAI TTS outputs at 24kHz
        }
    }

    /// Parse from string, with fallback to default.
    pub fn from_str_or_default(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "mp3" | "mpeg" => Self::Mp3,
            "opus" => Self::Opus,
            "aac" => Self::Aac,
            "flac" => Self::Flac,
            "wav" => Self::Wav,
            "pcm" | "linear16" | "raw" => Self::Pcm,
            _ => Self::default(),
        }
    }
}

impl std::fmt::Display for AudioOutputFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_as_str() {
        assert_eq!(OpenAITTSModel::Tts1.as_str(), "tts-1");
        assert_eq!(OpenAITTSModel::Tts1Hd.as_str(), "tts-1-hd");
        assert_eq!(OpenAITTSModel::Gpt4oMiniTts.as_str(), "gpt-4o-mini-tts");
    }

    #[test]
    fn test_model_from_str() {
        assert_eq!(
            OpenAITTSModel::from_str_or_default("tts-1"),
            OpenAITTSModel::Tts1
        );
        assert_eq!(
            OpenAITTSModel::from_str_or_default("tts-1-hd"),
            OpenAITTSModel::Tts1Hd
        );
        assert_eq!(
            OpenAITTSModel::from_str_or_default("unknown"),
            OpenAITTSModel::Tts1
        );
    }

    #[test]
    fn test_voice_as_str() {
        assert_eq!(OpenAIVoice::Alloy.as_str(), "alloy");
        assert_eq!(OpenAIVoice::Nova.as_str(), "nova");
        assert_eq!(OpenAIVoice::Shimmer.as_str(), "shimmer");
    }

    #[test]
    fn test_voice_from_str() {
        assert_eq!(OpenAIVoice::from_str_or_default("nova"), OpenAIVoice::Nova);
        assert_eq!(
            OpenAIVoice::from_str_or_default("ALLOY"),
            OpenAIVoice::Alloy
        );
        assert_eq!(
            OpenAIVoice::from_str_or_default("unknown"),
            OpenAIVoice::Alloy
        );
    }

    #[test]
    fn test_voice_all() {
        let voices = OpenAIVoice::all();
        assert_eq!(voices.len(), 11);
        assert!(voices.contains(&OpenAIVoice::Alloy));
        assert!(voices.contains(&OpenAIVoice::Verse));
    }

    #[test]
    fn test_audio_format_as_str() {
        assert_eq!(AudioOutputFormat::Mp3.as_str(), "mp3");
        assert_eq!(AudioOutputFormat::Pcm.as_str(), "pcm");
        assert_eq!(AudioOutputFormat::Opus.as_str(), "opus");
    }

    #[test]
    fn test_audio_format_from_str() {
        assert_eq!(
            AudioOutputFormat::from_str_or_default("pcm"),
            AudioOutputFormat::Pcm
        );
        assert_eq!(
            AudioOutputFormat::from_str_or_default("linear16"),
            AudioOutputFormat::Pcm
        );
        assert_eq!(
            AudioOutputFormat::from_str_or_default("unknown"),
            AudioOutputFormat::Mp3
        );
    }

    #[test]
    fn test_audio_format_mime_type() {
        assert_eq!(AudioOutputFormat::Mp3.mime_type(), "audio/mpeg");
        assert_eq!(AudioOutputFormat::Pcm.mime_type(), "audio/pcm");
        assert_eq!(AudioOutputFormat::Wav.mime_type(), "audio/wav");
    }

    #[test]
    fn test_audio_format_sample_rate() {
        assert_eq!(AudioOutputFormat::Pcm.sample_rate(), 24000);
        assert_eq!(AudioOutputFormat::Mp3.sample_rate(), 24000);
    }
}
