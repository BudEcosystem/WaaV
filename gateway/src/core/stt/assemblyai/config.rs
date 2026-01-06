//! Configuration types for AssemblyAI Streaming STT API v3.
//!
//! This module contains all configuration-related types including:
//! - Audio encoding specifications
//! - Speech model selection
//! - Regional endpoint selection
//! - Provider-specific configuration options

use std::str::FromStr;

use super::super::base::STTConfig;

// =============================================================================
// Audio Encoding
// =============================================================================

/// Supported audio encodings for AssemblyAI Streaming API.
///
/// AssemblyAI supports PCM and mu-law encodings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AssemblyAIEncoding {
    /// PCM signed 16-bit little-endian (default, most common)
    #[default]
    PcmS16le,
    /// PCM mu-law (telephony, 8kHz)
    PcmMulaw,
}

impl AssemblyAIEncoding {
    /// Convert to the API query parameter value.
    #[inline]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::PcmS16le => "pcm_s16le",
            Self::PcmMulaw => "pcm_mulaw",
        }
    }
}

impl FromStr for AssemblyAIEncoding {
    type Err = ();

    /// Parse from encoding string (case-insensitive).
    /// Returns Ok(Self::PcmS16le) as default for unknown values.
    #[inline]
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.to_lowercase().as_str() {
            "pcm_mulaw" | "mulaw" | "ulaw" => Self::PcmMulaw,
            _ => Self::PcmS16le, // Default to PCM S16LE
        })
    }
}

// =============================================================================
// Speech Model
// =============================================================================

/// AssemblyAI streaming speech recognition models.
///
/// Available models for real-time transcription.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AssemblyAISpeechModel {
    /// Universal streaming model optimized for English
    /// Best performance for English-only use cases
    #[default]
    UniversalStreamingEnglish,
    /// Universal streaming model supporting multiple languages
    /// Supports automatic language detection
    UniversalStreamingMultilingual,
}

impl AssemblyAISpeechModel {
    /// Convert to the API query parameter value.
    #[inline]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::UniversalStreamingEnglish => "universal-streaming-english",
            Self::UniversalStreamingMultilingual => "universal-streaming-multilingual",
        }
    }

    /// Check if model supports automatic language detection.
    #[inline]
    pub fn supports_language_detection(&self) -> bool {
        matches!(self, Self::UniversalStreamingMultilingual)
    }
}

impl FromStr for AssemblyAISpeechModel {
    type Err = ();

    /// Parse from model string (case-insensitive).
    /// Returns Ok(Self::UniversalStreamingEnglish) as default for unknown values.
    #[inline]
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.to_lowercase().as_str() {
            "universal-streaming-multilingual" | "multilingual" => {
                Self::UniversalStreamingMultilingual
            }
            _ => Self::UniversalStreamingEnglish, // Default to English
        })
    }
}

// =============================================================================
// Regional Endpoints
// =============================================================================

/// AssemblyAI regional endpoints for Streaming API.
///
/// Choose the region closest to your users for optimal latency,
/// or use EU endpoint for data residency requirements.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AssemblyAIRegion {
    /// Default global endpoint (US-based)
    #[default]
    Default,
    /// EU endpoint for European data residency
    Eu,
}

impl AssemblyAIRegion {
    /// Get the WebSocket base URL for this region.
    #[inline]
    pub fn websocket_base_url(&self) -> &'static str {
        match self {
            Self::Default => "wss://streaming.assemblyai.com",
            Self::Eu => "wss://streaming.eu.assemblyai.com",
        }
    }

    /// Get the host name for HTTP headers.
    #[inline]
    pub fn host(&self) -> &'static str {
        match self {
            Self::Default => "streaming.assemblyai.com",
            Self::Eu => "streaming.eu.assemblyai.com",
        }
    }
}

// =============================================================================
// Main Configuration
// =============================================================================

/// Configuration specific to AssemblyAI Streaming STT API v3.
///
/// This configuration extends the base `STTConfig` with AssemblyAI-specific
/// parameters for the WebSocket streaming API.
#[derive(Debug, Clone)]
pub struct AssemblyAISTTConfig {
    /// Base STT configuration (shared across all providers).
    pub base: STTConfig,

    /// Speech recognition model to use.
    ///
    /// - `UniversalStreamingEnglish`: Optimized for English (default)
    /// - `UniversalStreamingMultilingual`: Supports multiple languages
    pub speech_model: AssemblyAISpeechModel,

    /// Audio encoding format.
    ///
    /// Must match the format of audio data sent to the API.
    pub encoding: AssemblyAIEncoding,

    /// Enable turn formatting for immutable transcripts.
    ///
    /// When true, transcripts are returned in "turns" (complete utterances)
    /// that won't be modified by subsequent audio. This is AssemblyAI's
    /// key differentiator - transcripts are never overwritten.
    pub format_turns: bool,

    /// End-of-turn detection confidence threshold (0.0 to 1.0).
    ///
    /// Controls when a turn is considered complete:
    /// - Lower values: More aggressive end-of-turn detection
    /// - Higher values: Wait longer before finalizing turns
    ///
    /// Only applies when `format_turns` is true.
    pub end_of_turn_confidence_threshold: Option<f32>,

    /// Regional endpoint selection.
    ///
    /// Choose based on latency requirements or data residency needs.
    pub region: AssemblyAIRegion,

    /// Enable word-level timestamps in transcription results.
    ///
    /// When enabled, each word includes start/end timing information.
    /// Default is true for AssemblyAI (always provided in API v3).
    pub include_word_timestamps: bool,
}

impl Default for AssemblyAISTTConfig {
    fn default() -> Self {
        Self {
            base: STTConfig::default(),
            speech_model: AssemblyAISpeechModel::default(),
            encoding: AssemblyAIEncoding::default(),
            format_turns: true, // AssemblyAI's key feature
            end_of_turn_confidence_threshold: Some(0.5), // Balanced threshold
            region: AssemblyAIRegion::default(),
            include_word_timestamps: true, // Always available in v3
        }
    }
}

impl AssemblyAISTTConfig {
    /// Build the WebSocket URL with query parameters.
    ///
    /// Constructs the full WebSocket URL including:
    /// - Regional endpoint base URL
    /// - API path (/v3/ws)
    /// - All configuration query parameters
    ///
    /// # Performance Note
    ///
    /// Uses pre-allocated String with estimated capacity (256 bytes)
    /// to minimize allocations during URL construction.
    pub fn build_websocket_url(&self) -> String {
        let base_url = self.region.websocket_base_url();

        // Pre-allocate with estimated capacity
        let mut url = String::with_capacity(256);

        // Base URL and path
        url.push_str(base_url);
        url.push_str("/v3/ws");

        // Required: sample_rate
        url.push_str("?sample_rate=");
        url.push_str(&self.base.sample_rate.to_string());

        // Required: encoding
        url.push_str("&encoding=");
        url.push_str(self.encoding.as_str());

        // Speech model
        url.push_str("&speech_model=");
        url.push_str(self.speech_model.as_str());

        // Format turns (immutable transcripts)
        url.push_str("&format_turns=");
        url.push_str(if self.format_turns { "true" } else { "false" });

        // End-of-turn confidence threshold
        if let Some(threshold) = self.end_of_turn_confidence_threshold {
            url.push_str("&end_of_turn_confidence_threshold=");
            url.push_str(&format!("{:.2}", threshold.clamp(0.0, 1.0)));
        }

        url
    }

    /// Create a new configuration from base STTConfig.
    ///
    /// Automatically determines the encoding and speech model from config.
    pub fn from_base(base: STTConfig) -> Self {
        // Determine encoding from base config (unwrap is safe - FromStr impl never fails)
        let encoding = base.encoding.parse().unwrap_or_default();

        // Determine speech model based on language
        let speech_model = if base.language.starts_with("en") || base.language.is_empty() {
            AssemblyAISpeechModel::UniversalStreamingEnglish
        } else {
            AssemblyAISpeechModel::UniversalStreamingMultilingual
        };

        Self {
            base,
            speech_model,
            encoding,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encoding_as_str() {
        assert_eq!(AssemblyAIEncoding::PcmS16le.as_str(), "pcm_s16le");
        assert_eq!(AssemblyAIEncoding::PcmMulaw.as_str(), "pcm_mulaw");
    }

    #[test]
    fn test_encoding_from_str() {
        assert_eq!(
            "pcm_s16le".parse::<AssemblyAIEncoding>().unwrap(),
            AssemblyAIEncoding::PcmS16le
        );
        assert_eq!(
            "pcm_mulaw".parse::<AssemblyAIEncoding>().unwrap(),
            AssemblyAIEncoding::PcmMulaw
        );
        assert_eq!(
            "mulaw".parse::<AssemblyAIEncoding>().unwrap(),
            AssemblyAIEncoding::PcmMulaw
        );
        assert_eq!(
            "unknown".parse::<AssemblyAIEncoding>().unwrap(),
            AssemblyAIEncoding::PcmS16le
        );
    }

    #[test]
    fn test_speech_model_as_str() {
        assert_eq!(
            AssemblyAISpeechModel::UniversalStreamingEnglish.as_str(),
            "universal-streaming-english"
        );
        assert_eq!(
            AssemblyAISpeechModel::UniversalStreamingMultilingual.as_str(),
            "universal-streaming-multilingual"
        );
    }

    #[test]
    fn test_speech_model_from_str() {
        assert_eq!(
            "universal-streaming-english"
                .parse::<AssemblyAISpeechModel>()
                .unwrap(),
            AssemblyAISpeechModel::UniversalStreamingEnglish
        );
        assert_eq!(
            "universal-streaming-multilingual"
                .parse::<AssemblyAISpeechModel>()
                .unwrap(),
            AssemblyAISpeechModel::UniversalStreamingMultilingual
        );
        assert_eq!(
            "multilingual".parse::<AssemblyAISpeechModel>().unwrap(),
            AssemblyAISpeechModel::UniversalStreamingMultilingual
        );
    }

    #[test]
    fn test_speech_model_language_detection() {
        assert!(!AssemblyAISpeechModel::UniversalStreamingEnglish.supports_language_detection());
        assert!(
            AssemblyAISpeechModel::UniversalStreamingMultilingual.supports_language_detection()
        );
    }

    #[test]
    fn test_region_websocket_url() {
        assert_eq!(
            AssemblyAIRegion::Default.websocket_base_url(),
            "wss://streaming.assemblyai.com"
        );
        assert_eq!(
            AssemblyAIRegion::Eu.websocket_base_url(),
            "wss://streaming.eu.assemblyai.com"
        );
    }

    #[test]
    fn test_region_host() {
        assert_eq!(AssemblyAIRegion::Default.host(), "streaming.assemblyai.com");
        assert_eq!(AssemblyAIRegion::Eu.host(), "streaming.eu.assemblyai.com");
    }

    #[test]
    fn test_build_websocket_url() {
        let config = AssemblyAISTTConfig {
            base: STTConfig {
                sample_rate: 16000,
                ..Default::default()
            },
            speech_model: AssemblyAISpeechModel::UniversalStreamingEnglish,
            encoding: AssemblyAIEncoding::PcmS16le,
            format_turns: true,
            end_of_turn_confidence_threshold: Some(0.5),
            region: AssemblyAIRegion::Default,
            include_word_timestamps: true,
        };

        let url = config.build_websocket_url();

        assert!(url.starts_with("wss://streaming.assemblyai.com/v3/ws?"));
        assert!(url.contains("sample_rate=16000"));
        assert!(url.contains("encoding=pcm_s16le"));
        assert!(url.contains("speech_model=universal-streaming-english"));
        assert!(url.contains("format_turns=true"));
        assert!(url.contains("end_of_turn_confidence_threshold=0.50"));
    }

    #[test]
    fn test_build_websocket_url_eu_region() {
        let config = AssemblyAISTTConfig {
            base: STTConfig {
                sample_rate: 8000,
                ..Default::default()
            },
            speech_model: AssemblyAISpeechModel::UniversalStreamingMultilingual,
            encoding: AssemblyAIEncoding::PcmMulaw,
            format_turns: false,
            end_of_turn_confidence_threshold: None,
            region: AssemblyAIRegion::Eu,
            include_word_timestamps: true,
        };

        let url = config.build_websocket_url();

        assert!(url.starts_with("wss://streaming.eu.assemblyai.com/v3/ws?"));
        assert!(url.contains("sample_rate=8000"));
        assert!(url.contains("encoding=pcm_mulaw"));
        assert!(url.contains("speech_model=universal-streaming-multilingual"));
        assert!(url.contains("format_turns=false"));
        assert!(!url.contains("end_of_turn_confidence_threshold"));
    }

    #[test]
    fn test_from_base_english() {
        let base = STTConfig {
            api_key: "test_key".to_string(),
            language: "en-US".to_string(),
            sample_rate: 16000,
            encoding: "linear16".to_string(),
            ..Default::default()
        };

        let config = AssemblyAISTTConfig::from_base(base);

        assert_eq!(
            config.speech_model,
            AssemblyAISpeechModel::UniversalStreamingEnglish
        );
        assert_eq!(config.encoding, AssemblyAIEncoding::PcmS16le);
    }

    #[test]
    fn test_from_base_multilingual() {
        let base = STTConfig {
            api_key: "test_key".to_string(),
            language: "fr-FR".to_string(),
            sample_rate: 16000,
            encoding: "linear16".to_string(),
            ..Default::default()
        };

        let config = AssemblyAISTTConfig::from_base(base);

        assert_eq!(
            config.speech_model,
            AssemblyAISpeechModel::UniversalStreamingMultilingual
        );
    }

    #[test]
    fn test_default_config() {
        let config = AssemblyAISTTConfig::default();

        assert_eq!(
            config.speech_model,
            AssemblyAISpeechModel::UniversalStreamingEnglish
        );
        assert_eq!(config.encoding, AssemblyAIEncoding::PcmS16le);
        assert!(config.format_turns);
        assert_eq!(config.end_of_turn_confidence_threshold, Some(0.5));
        assert_eq!(config.region, AssemblyAIRegion::Default);
        assert!(config.include_word_timestamps);
    }
}
