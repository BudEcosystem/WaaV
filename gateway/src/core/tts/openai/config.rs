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

    /// Parse a recognised model id or alias (case-insensitive); `None` for anything else.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "tts-1" | "tts1" => Some(Self::Tts1),
            "tts-1-hd" | "tts1-hd" | "tts1hd" => Some(Self::Tts1Hd),
            "gpt-4o-mini-tts" | "gpt4o-mini-tts" => Some(Self::Gpt4oMiniTts),
            _ => None,
        }
    }

    /// Parse from string, with fallback to default.
    ///
    /// Classification only — NOT the value sent on the wire. An id this enum does not know
    /// (e.g. `gpt-4o-mini-tts-2025-12-15`, `tts-1-hd-1106`) classifies as the default here, but
    /// is sent verbatim; see `openai_tts_model_id`.
    pub fn from_str_or_default(s: &str) -> Self {
        Self::parse(s).unwrap_or_default()
    }
}

/// The `model` value sent to OpenAI for a configured model string.
///
/// OpenAI requires `model`, so an EMPTY configuration falls back to the default (`tts-1`). A
/// recognised id or alias is sent in its canonical spelling (`TTS1` → `tts-1`: the same model).
/// Anything else is sent VERBATIM: OpenAI ships dated snapshots (`gpt-4o-mini-tts-2025-12-15`,
/// `tts-1-1106`, `tts-1-hd-1106`) and new models this enum does not list, and silently rewriting
/// them to `tts-1` replaced the caller's model with a different one (the Deepgram `aura-2` bug
/// class). An id OpenAI does not accept is its error to report, not WaaV's to paper over.
pub fn openai_tts_model_id(configured: &str) -> String {
    let configured = configured.trim();
    if configured.is_empty() {
        return OpenAITTSModel::default().as_str().to_string();
    }
    match OpenAITTSModel::parse(configured) {
        Some(model) => model.as_str().to_string(),
        None => configured.to_string(),
    }
}

/// Whether a model id accepts the `instructions` field.
///
/// OpenAI documents `instructions` as unsupported on `tts-1` / `tts-1-hd` only (and their dated
/// snapshots such as `tts-1-hd-1106`). Every other speech model — `gpt-4o-mini-tts` and its
/// snapshots, and models newer than this code — takes it, so the gate is "not a tts-1 family
/// model", not "is exactly the gpt-4o-mini-tts enum variant".
pub fn openai_tts_model_supports_instructions(model_id: &str) -> bool {
    !model_id.trim().to_ascii_lowercase().starts_with("tts-1")
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
/// OpenAI provides 13 distinct voices with different characteristics:
/// - Alloy, Echo, Fable, Onyx, Nova, Shimmer: Original voices
/// - Ash, Ballad, Coral, Sage, Verse: Additional voices
/// - Marin, Cedar: newest voices, recommended by OpenAI for gpt-4o-mini-tts
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
    /// Marin voice (newest; recommended for gpt-4o-mini-tts)
    Marin,
    /// Cedar voice (newest; recommended for gpt-4o-mini-tts)
    Cedar,
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
            Self::Marin => "marin",
            Self::Cedar => "cedar",
        }
    }

    /// Parse a recognised voice name (case-insensitive); `None` for anything else.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "alloy" => Some(Self::Alloy),
            "ash" => Some(Self::Ash),
            "ballad" => Some(Self::Ballad),
            "coral" => Some(Self::Coral),
            "echo" => Some(Self::Echo),
            "fable" => Some(Self::Fable),
            "onyx" => Some(Self::Onyx),
            "nova" => Some(Self::Nova),
            "sage" => Some(Self::Sage),
            "shimmer" => Some(Self::Shimmer),
            "verse" => Some(Self::Verse),
            "marin" => Some(Self::Marin),
            "cedar" => Some(Self::Cedar),
            _ => None,
        }
    }

    /// Parse from string, with fallback to default.
    ///
    /// Classification only — NOT the value sent on the wire. A voice this enum does not know
    /// classifies as `Alloy` here, but is sent verbatim; see `openai_tts_voice_id`.
    pub fn from_str_or_default(s: &str) -> Self {
        Self::parse(s).unwrap_or_default()
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
            Self::Marin,
            Self::Cedar,
        ]
    }
}

impl std::fmt::Display for OpenAIVoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// The `voice` value sent to OpenAI for a configured voice.
///
/// OpenAI requires `voice`, so an absent or EMPTY voice falls back to the default (`alloy`). A
/// recognised voice is sent in its canonical lowercase spelling (`NOVA` → `nova`). Anything else
/// is sent VERBATIM: OpenAI adds voices this enum does not list, and silently substituting
/// `alloy` synthesised in a voice nobody chose (the Deepgram `aura-2` bug class). A voice OpenAI
/// does not accept is its error to report.
pub fn openai_tts_voice_id(configured: Option<&str>) -> String {
    let configured = configured.map(str::trim).unwrap_or_default();
    if configured.is_empty() {
        return OpenAIVoice::default().as_str().to_string();
    }
    match OpenAIVoice::parse(configured) {
        Some(voice) => voice.as_str().to_string(),
        None => configured.to_string(),
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

    // Real OpenAI ids the enum does not list must be sent VERBATIM, not rewritten to tts-1.
    #[test]
    fn test_model_id_is_sent_verbatim() {
        for id in [
            "gpt-4o-mini-tts-2025-12-15",
            "tts-1-1106",
            "tts-1-hd-1106",
            "some-future-tts-model",
        ] {
            assert_eq!(openai_tts_model_id(id), id);
        }
        // Recognised ids/aliases keep their canonical spelling (same model).
        assert_eq!(openai_tts_model_id("tts-1-hd"), "tts-1-hd");
        assert_eq!(openai_tts_model_id("TTS1"), "tts-1");
        assert_eq!(openai_tts_model_id("gpt-4o-mini-tts"), "gpt-4o-mini-tts");
        // OpenAI requires `model`: only an EMPTY one gets the default.
        assert_eq!(openai_tts_model_id(""), "tts-1");
        assert_eq!(openai_tts_model_id("   "), "tts-1");
    }

    // `instructions` is unsupported on tts-1 / tts-1-hd (and their snapshots) only.
    #[test]
    fn test_model_supports_instructions() {
        assert!(openai_tts_model_supports_instructions("gpt-4o-mini-tts"));
        assert!(openai_tts_model_supports_instructions(
            "gpt-4o-mini-tts-2025-12-15"
        ));
        assert!(openai_tts_model_supports_instructions(
            "some-future-tts-model"
        ));
        assert!(!openai_tts_model_supports_instructions("tts-1"));
        assert!(!openai_tts_model_supports_instructions("tts-1-hd"));
        assert!(!openai_tts_model_supports_instructions("tts-1-hd-1106"));
        assert!(!openai_tts_model_supports_instructions("TTS-1-1106"));
    }

    // A voice the enum does not list must be sent VERBATIM, not replaced with alloy.
    #[test]
    fn test_voice_id_is_sent_verbatim() {
        assert_eq!(
            openai_tts_voice_id(Some("some-new-voice")),
            "some-new-voice"
        );
        assert_eq!(openai_tts_voice_id(Some("NOVA")), "nova");
        assert_eq!(openai_tts_voice_id(Some("cedar")), "cedar");
        // OpenAI requires `voice`: only an absent/EMPTY one gets the default.
        assert_eq!(openai_tts_voice_id(None), "alloy");
        assert_eq!(openai_tts_voice_id(Some("")), "alloy");
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
        assert_eq!(voices.len(), 13);
        assert!(voices.contains(&OpenAIVoice::Alloy));
        assert!(voices.contains(&OpenAIVoice::Verse));
        assert!(voices.contains(&OpenAIVoice::Marin));
        assert!(voices.contains(&OpenAIVoice::Cedar));
        assert_eq!(
            OpenAIVoice::from_str_or_default("cedar"),
            OpenAIVoice::Cedar
        );
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
