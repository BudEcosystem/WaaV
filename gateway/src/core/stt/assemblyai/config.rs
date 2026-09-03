//! Configuration types for AssemblyAI Streaming STT API v3.
//!
//! This module contains all configuration-related types including:
//! - Audio encoding specifications
//! - Speech model selection
//! - Regional endpoint selection
//! - Provider-specific configuration options

use std::str::FromStr;

use super::super::base::{STTConfig, STTError};

/// AssemblyAI streaming keyterms limits (per the keyterms-prompting docs):
/// at most 100 terms per session, each at most 50 characters.
const MAX_KEYTERMS: usize = 100;
const MAX_KEYTERM_CHARS: usize = 50;

/// Percent-encode a query-string value so phrases with spaces ("John Smith") or
/// JSON punctuation survive intact in the WebSocket connection URL. An unencoded
/// space (or `[`/`"`) produces a malformed URL and the param is silently dropped
/// by the server — exactly the wire-level bug class the review flagged.
#[inline]
fn encode_query_value(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

fn validate_assemblyai_ws_endpoint(source: &str, endpoint: &str) -> Result<(), String> {
    let endpoint = endpoint.trim();
    if endpoint.is_empty() {
        return Ok(());
    }
    crate::core::net::validate_url_for_ssrf(endpoint, &["ws", "wss"])
        .map_err(|msg| format!("{source} rejected (SSRF protection): {msg}"))
}

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

    /// Keyterm prompts to boost recognition of domain-specific words/phrases.
    ///
    /// Maps the standardized `keyterms` feature onto AssemblyAI v3 streaming's
    /// `keyterms_prompt` connection query parameter. AssemblyAI does NOT use the
    /// batch-only `word_boost` field on the streaming endpoint — `keyterms_prompt`
    /// is the streaming equivalent (see `build_websocket_url`).
    ///
    /// Limits enforced at URL-build time: max 100 terms, each ≤ 50 chars
    /// (longer terms are dropped, the list is truncated to 100).
    pub keyterms_prompt: Vec<String>,

    /// Enable automatic spoken-language detection.
    ///
    /// Maps the standardized `language_detection` feature onto AssemblyAI v3
    /// streaming's `language_detection` connection query parameter. Only effective
    /// with the multilingual model; AssemblyAI ignores it for the English-only model.
    pub language_detection: bool,

    /// Streaming speaker diarization (who-spoke-when).
    ///
    /// Maps the standardized typed `diarization` feature onto AssemblyAI v3 streaming's
    /// `speaker_labels` connection query parameter. (Confirmed June 2026 against the v3
    /// streaming AsyncAPI query-parameter schema: `speaker_labels` true/false.) Off by
    /// default so the URL matches the provider default when unset.
    pub speaker_labels: bool,

    /// Hint for the maximum number of speakers to detect (1–10). Only meaningful when
    /// `speaker_labels` is on. Maps to the `max_speakers` connection query parameter.
    /// Carried via `ProviderExtras` (`max_speakers`) — no typed field on `SttFeatures`.
    pub max_speakers: Option<u8>,

    /// Maximum silence (ms) within a turn before AssemblyAI forces a turn boundary
    /// (endpointing). Maps the standardized typed `endpointing_ms` feature onto the
    /// `max_turn_silence` connection query parameter.
    pub max_turn_silence: Option<u32>,

    /// Minimum silence (ms) that must elapse before a turn can end. Maps to the
    /// `min_turn_silence` connection query parameter. Carried via `ProviderExtras`
    /// (`min_turn_silence`) — no typed field on `SttFeatures`.
    pub min_turn_silence: Option<u32>,

    /// Voice-activity-detection silence threshold (0.0–1.0). Maps to the `vad_threshold`
    /// connection query parameter. Carried via `ProviderExtras` (`vad_threshold`).
    pub vad_threshold: Option<f32>,

    /// Inactivity timeout (seconds, 5–3600) after which AssemblyAI ends an idle session.
    /// Maps to the `inactivity_timeout` connection query parameter. Carried via
    /// `ProviderExtras` (`inactivity_timeout`).
    pub inactivity_timeout: Option<u32>,

    /// Domain-specific model selection (e.g. `medical-v1`). Maps to the `domain`
    /// connection query parameter. Carried via `ProviderExtras` (`domain`).
    pub domain: Option<String>,

    /// Override the WebSocket base endpoint (scheme://host[:port]) — e.g. `ws://127.0.0.1:PORT`
    /// for a local mock (W-T0 harness) or a proxy. When `None`, the regional production
    /// endpoint is used. Generalizes the OpenAI `OPENAI_BASE_URL` pattern; required so the
    /// reconnection chaos tests can drive a real mock through this provider.
    pub endpoint_override: Option<String>,
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
            keyterms_prompt: Vec::new(),   // No boost terms by default
            language_detection: false,     // Off by default (multilingual model only)
            speaker_labels: false,         // Off by default (provider default)
            max_speakers: None,            // Provider default (no hint)
            max_turn_silence: None,        // Provider default endpointing
            min_turn_silence: None,        // Provider default endpointing
            vad_threshold: None,           // Provider default VAD threshold
            inactivity_timeout: None,      // Provider default session timeout
            domain: None,                  // Provider default (no domain model)
            endpoint_override: None,
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
        // An explicit override (mock/proxy) wins over the regional production endpoint.
        let base_url: &str = match self
            .endpoint_override
            .as_deref()
            .map(str::trim)
            .filter(|o| !o.is_empty())
        {
            Some(o) => o.trim_end_matches('/'),
            None => self.region.websocket_base_url(),
        };

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

        // Automatic language detection (only effective on the multilingual model;
        // AssemblyAI ignores it for the English-only model). Only emit when enabled
        // so the URL matches the provider default (`language_detection=false`) when off.
        if self.language_detection {
            url.push_str("&language_detection=true");
        }

        // Streaming speaker diarization (`speaker_labels`). Only emit when enabled so the URL
        // matches the provider default (`speaker_labels=false`) when off — same omit-when-default
        // discipline as `language_detection`.
        if self.speaker_labels {
            url.push_str("&speaker_labels=true");

            // `max_speakers` is only meaningful alongside diarization; the server ignores it
            // otherwise, so gate it behind `speaker_labels` to keep the URL faithful to intent.
            if let Some(n) = self.max_speakers {
                let n = n.clamp(1, 10);
                url.push_str("&max_speakers=");
                url.push_str(&n.to_string());
            }
        }

        // Endpointing knobs (milliseconds). Emit only when explicitly set; otherwise the server
        // uses its own defaults and the URL stays minimal.
        if let Some(ms) = self.max_turn_silence {
            url.push_str("&max_turn_silence=");
            url.push_str(&ms.to_string());
        }
        if let Some(ms) = self.min_turn_silence {
            url.push_str("&min_turn_silence=");
            url.push_str(&ms.to_string());
        }

        // VAD silence threshold (0.0–1.0).
        if let Some(t) = self.vad_threshold {
            url.push_str("&vad_threshold=");
            url.push_str(&format!("{:.2}", t.clamp(0.0, 1.0)));
        }

        // Inactivity timeout (seconds, clamped to the documented 5–3600 range).
        if let Some(secs) = self.inactivity_timeout {
            let secs = secs.clamp(5, 3600);
            url.push_str("&inactivity_timeout=");
            url.push_str(&secs.to_string());
        }

        // Domain-specific model (e.g. `medical-v1`). URL-encode in case of punctuation.
        if let Some(domain) = &self.domain {
            if !domain.is_empty() {
                url.push_str("&domain=");
                url.push_str(&encode_query_value(domain));
            }
        }

        // Keyterm prompting: AssemblyAI v3 streaming's `keyterms_prompt`. The canonical
        // wire format (per the keyterms-prompting docs / SDK) is a single JSON-array
        // string, URL-encoded into one query param. Terms longer than 50 chars are
        // dropped and the list is capped at 100 (server rejects more than 100).
        if !self.keyterms_prompt.is_empty() {
            let terms: Vec<&str> = self
                .keyterms_prompt
                .iter()
                .map(|s| s.as_str())
                .filter(|s| !s.is_empty() && s.chars().count() <= MAX_KEYTERM_CHARS)
                .take(MAX_KEYTERMS)
                .collect();
            if !terms.is_empty() {
                // serde_json never fails serializing a `Vec<&str>`.
                let json = serde_json::to_string(&terms).unwrap_or_default();
                url.push_str("&keyterms_prompt=");
                url.push_str(&encode_query_value(&json));
            }
        }

        url
    }

    pub fn validate_endpoint_override(&self) -> Result<(), STTError> {
        if let Some(endpoint) = self.endpoint_override.as_deref() {
            validate_assemblyai_ws_endpoint("endpoint_override", endpoint)
                .map_err(STTError::ConfigurationError)?;
        }
        Ok(())
    }

    /// Create a new configuration from base STTConfig.
    ///
    /// Automatically determines the encoding and speech model from config.
    pub fn from_base(base: STTConfig) -> Self {
        // Determine encoding from base config (unwrap is safe - FromStr impl never fails)
        let encoding = base.encoding.parse().unwrap_or_default();

        // Determine speech model based on language
        // Honor an explicitly configured model; otherwise pick a sensible default by language.
        // Previously `base.model` was ignored entirely and the model was chosen from language
        // alone, so a caller selecting e.g. the multilingual model for English audio was overridden.
        let speech_model = if base.model.is_empty() {
            if base.language.starts_with("en") || base.language.is_empty() {
                AssemblyAISpeechModel::UniversalStreamingEnglish
            } else {
                AssemblyAISpeechModel::UniversalStreamingMultilingual
            }
        } else {
            base.model
                .parse()
                .unwrap_or(AssemblyAISpeechModel::UniversalStreamingEnglish)
        };

        Self {
            base,
            speech_model,
            encoding,
            ..Default::default()
        }
    }

    /// Build from the standardized config (W1 keystone — the 2nd provider migrated after
    /// Deepgram). Maps the features AssemblyAI v3 supports; features it cannot express
    /// (diarization, keyterms) are simply left at provider defaults (graceful capability
    /// degradation rather than a silent lie).
    pub fn from_standard(std: &crate::core::stt::standard::StandardSTTConfig) -> Self {
        let mut cfg = Self::from_base(std.base.clone());
        let f = &std.features;
        let ex = &std.extras.0;

        if let Some(w) = f.word_timestamps {
            cfg.include_word_timestamps = w;
        }

        // Streaming speaker diarization — typed `diarization` maps to the v3 streaming
        // `speaker_labels` connection query parameter (confirmed June 2026 against the v3
        // streaming AsyncAPI query-parameter schema). This is the STREAMING diarization knob,
        // distinct from the batch `speaker_labels` request field.
        if let Some(d) = f.diarization {
            cfg.speaker_labels = d;
        }
        // Max-speakers hint (`max_speakers`, 1–10) — provider-specific, via extras passthrough.
        cfg.max_speakers = ex
            .get("max_speakers")
            .and_then(|v| v.as_u64())
            .map(|n| n.min(u8::MAX as u64) as u8);

        // Endpointing: typed `endpointing_ms` → `max_turn_silence` (ms). `min_turn_silence` has
        // no typed field, so it rides the extras passthrough.
        cfg.max_turn_silence = f.endpointing_ms;
        cfg.min_turn_silence = ex
            .get("min_turn_silence")
            .and_then(|v| v.as_u64())
            .map(|n| n.min(u32::MAX as u64) as u32);

        // VAD silence threshold (`vad_threshold`, 0.0–1.0) — provider-specific, via extras.
        cfg.vad_threshold = ex
            .get("vad_threshold")
            .and_then(|v| v.as_f64())
            .map(|t| t as f32);

        // Inactivity timeout (`inactivity_timeout`, seconds) — provider-specific, via extras.
        cfg.inactivity_timeout = ex
            .get("inactivity_timeout")
            .and_then(|v| v.as_u64())
            .map(|n| n.min(u32::MAX as u64) as u32);

        // Domain-specific model (`domain`, e.g. "medical-v1") — provider-specific, via extras.
        cfg.domain = ex
            .get("domain")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        // Keyterm prompting — AssemblyAI v3 streaming supports `keyterms_prompt` as a connection
        // query param (canonical: a URL-encoded JSON array; max 100 terms, ≤50 chars each). This
        // is the STREAMING equivalent of the batch-only `word_boost`, so the standardized
        // `keyterms` feature maps here. Confirmed:
        // https://www.assemblyai.com/docs/speech-to-text/universal-streaming/keyterms-prompting
        // and the streaming API reference (keyterms_prompt connection parameter).
        if let Some(terms) = &f.keyterms {
            cfg.keyterms_prompt = terms.clone();
        }

        // Automatic language detection — supported on streaming as the `language_detection`
        // connection query param (multilingual model only). Confirmed in the streaming API
        // reference (language_detection: 'true'|'false', "Only available for the multilingual
        // model"). https://assemblyai.com/docs/api-reference/streaming-api/streaming-api
        if let Some(ld) = f.language_detection {
            cfg.language_detection = ld;
        }

        // Endpoint override (W-T0): carried via the standardized extras passthrough so the
        // restored, *featured* session can be pointed at a mock/proxy.
        cfg.endpoint_override = std.endpoint_override().map(|s| s.to_string());

        // CAPABILITY GAPS — intentionally NOT mapped to the wire (left at provider defaults,
        // not silently faked). The following standardized features exist ONLY on AssemblyAI's
        // BATCH transcription API and are absent from the v3 STREAMING WebSocket spec
        // (https://assemblyai.com/docs/api-reference/streaming-api/streaming-api):
        //   - `word_boost`            → not a streaming param; streaming uses `keyterms_prompt`
        //                               (mapped above from `keyterms`), so we never emit word_boost.
        //   - `f.sentiment`           → `sentiment_analysis` is batch-only; absent from streaming.
        //   - `f.entity_detection`    → `entity_detection` is batch-only; absent from streaming.
        // (diarization and endpointing ARE now wired above: streaming exposes them as
        // `speaker_labels` and `max_turn_silence` respectively — confirmed against the v3
        // streaming AsyncAPI query-parameter schema, June 2026.)
        let _ = (f.sentiment, f.entity_detection); // referenced so intent is explicit, not wired.

        cfg
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

    // W1 keystone (2nd provider): the standardized `word_timestamps` feature is honored.
    #[test]
    fn from_standard_maps_word_timestamps() {
        use crate::core::stt::standard::{StandardSTTConfig, SttFeatures};
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "assemblyai".into(),
                api_key: "k".into(),
                ..Default::default()
            },
            features: SttFeatures {
                word_timestamps: Some(false),
                ..Default::default()
            },
            extras: Default::default(),
            translation: None,
        };
        let cfg = AssemblyAISTTConfig::from_standard(&std);
        assert!(
            !cfg.include_word_timestamps,
            "word_timestamps=false must reach the AssemblyAI config"
        );
        // And the default (unset) keeps the provider default (true).
        let cfg2 =
            AssemblyAISTTConfig::from_standard(&StandardSTTConfig::from_base(std.base.clone()));
        assert!(cfg2.include_word_timestamps);
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
            ..Default::default()
        };

        let url = config.build_websocket_url();

        assert!(url.starts_with("wss://streaming.assemblyai.com/v3/ws?"));
        assert!(url.contains("sample_rate=16000"));
        assert!(url.contains("encoding=pcm_s16le"));
        assert!(url.contains("speech_model=universal-streaming-english"));
        assert!(url.contains("format_turns=true"));
        assert!(url.contains("end_of_turn_confidence_threshold=0.50"));
        // Optional advanced params are omitted when unset (provider defaults).
        assert!(!url.contains("keyterms_prompt"));
        assert!(!url.contains("language_detection"));
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
            ..Default::default()
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
    fn test_endpoint_override_validation_rejects_ssrf_targets() {
        let _env = crate::core::net::ssrf_env_lock();
        let mut config = AssemblyAISTTConfig {
            endpoint_override: Some("wss://assemblyai-proxy.example.com".to_string()),
            ..Default::default()
        };
        assert!(config.validate_endpoint_override().is_ok());

        config.endpoint_override = Some("ws://127.0.0.1:9000".to_string());
        let err = config
            .validate_endpoint_override()
            .expect_err("loopback endpoint_override must be rejected");
        assert!(err.to_string().contains("SSRF protection"), "{err}");

        config.endpoint_override = Some("file:///tmp/socket".to_string());
        let err = config
            .validate_endpoint_override()
            .expect_err("non-WebSocket endpoint_override must be rejected");
        assert!(err.to_string().contains("not allowed"), "{err}");

        config.endpoint_override = Some("https://assemblyai-proxy.example.com".to_string());
        let err = config
            .validate_endpoint_override()
            .expect_err("HTTP endpoint_override must be rejected for WebSocket dial");
        assert!(err.to_string().contains("not allowed"), "{err}");
    }

    #[test]
    fn test_build_websocket_url_trims_endpoint_override() {
        let config = AssemblyAISTTConfig {
            base: STTConfig {
                sample_rate: 16000,
                ..Default::default()
            },
            endpoint_override: Some(" wss://assemblyai-proxy.example.com/ ".to_string()),
            ..Default::default()
        };

        let url = config.build_websocket_url();
        assert!(url.starts_with("wss://assemblyai-proxy.example.com/v3/ws?"));
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
            // No explicit model → model is selected by language (the default STTConfig model is
            // Deepgram's "nova-3", which is now honored, so clear it to test the fallback).
            model: String::new(),
            ..Default::default()
        };

        let config = AssemblyAISTTConfig::from_base(base);

        assert_eq!(
            config.speech_model,
            AssemblyAISpeechModel::UniversalStreamingMultilingual
        );
    }

    #[test]
    fn test_from_base_honors_explicit_model() {
        // An explicitly configured model must win over the language-based default.
        let base = STTConfig {
            api_key: "test_key".to_string(),
            language: "en-US".to_string(), // English would otherwise force the English model
            model: "universal-streaming-multilingual".to_string(),
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
        assert!(config.keyterms_prompt.is_empty());
        assert!(!config.language_detection);
    }

    // =========================================================================
    // Wire-level feature tests (the bug class the review caught: a feature can be
    // present on the config struct yet never reach the request URL). These assert
    // the param appears in the SERIALIZED WebSocket connection URL, not merely on
    // the struct.
    // =========================================================================

    use crate::core::stt::standard::{ProviderExtras, StandardSTTConfig, SttFeatures};

    /// keyterms (standardized) → `keyterms_prompt` connection query param. AssemblyAI v3
    /// streaming uses `keyterms_prompt` (a URL-encoded JSON array), NOT the batch-only
    /// `word_boost`. WIRE assert: the encoded JSON array is in the URL and `word_boost` is not.
    #[test]
    fn keyterms_reach_the_wire_as_keyterms_prompt() {
        let mut config = AssemblyAISTTConfig {
            base: STTConfig {
                sample_rate: 16000,
                ..Default::default()
            },
            ..Default::default()
        };
        // A multi-word phrase must be percent-encoded; a JSON array must be the wire format.
        config.keyterms_prompt = vec!["AssemblyAI".into(), "John Smith".into()];

        let url = config.build_websocket_url();

        // `keyterms_prompt=` must be present, carrying a URL-encoded JSON array.
        assert!(
            url.contains("keyterms_prompt="),
            "keyterms_prompt missing from URL: {url}"
        );
        // The canonical wire form is a JSON array, URL-encoded: `["AssemblyAI","John Smith"]`
        // → `%5B%22AssemblyAI%22%2C%22John+Smith%22%5D` (space → `+`, brackets/quotes/comma escaped).
        let expected = encode_query_value(r#"["AssemblyAI","John Smith"]"#);
        assert!(
            url.contains(&format!("keyterms_prompt={expected}")),
            "keyterms_prompt not encoded as a JSON array: {url}"
        );
        // A raw space (unencoded) would silently break the param — must NOT appear.
        assert!(
            !url.contains("John Smith"),
            "raw space leaked into URL: {url}"
        );
        // CAPABILITY GAP guard: the batch-only `word_boost` must never reach the streaming URL.
        assert!(
            !url.contains("word_boost"),
            "word_boost (batch-only) must not appear on streaming URL: {url}"
        );
    }

    /// keyterms limits: terms >50 chars are dropped, list capped at 100.
    #[test]
    fn keyterms_enforce_length_and_count_limits() {
        let mut config = AssemblyAISTTConfig {
            base: STTConfig {
                sample_rate: 16000,
                ..Default::default()
            },
            ..Default::default()
        };
        let too_long: String = "x".repeat(MAX_KEYTERM_CHARS + 1);
        config.keyterms_prompt = vec!["ok".into(), too_long.clone()];
        let url = config.build_websocket_url();
        let decoded =
            url::form_urlencoded::parse(url.split("keyterms_prompt=").nth(1).unwrap().as_bytes())
                .next();
        // Easier: assert the over-length term's content is absent and "ok" present.
        let _ = decoded;
        assert!(
            url.contains("keyterms_prompt="),
            "expected keyterms on wire"
        );
        let encoded_ok = encode_query_value(r#"["ok"]"#);
        assert!(
            url.contains(&format!("keyterms_prompt={encoded_ok}")),
            "over-length keyterm not dropped (expected just [\"ok\"]): {url}"
        );

        // Over 100 terms → capped at 100 (server rejects >100).
        let many: Vec<String> = (0..150).map(|i| format!("t{i}")).collect();
        config.keyterms_prompt = many;
        let url2 = config.build_websocket_url();
        let raw = url2.split("keyterms_prompt=").nth(1).unwrap();
        let json = url::form_urlencoded::parse(format!("k={raw}").as_bytes())
            .next()
            .map(|(_, v)| v.into_owned())
            .unwrap();
        let parsed: Vec<String> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.len(), MAX_KEYTERMS, "keyterms not capped at 100");
    }

    /// language_detection (standardized) → `language_detection=true` connection query param.
    #[test]
    fn language_detection_reaches_the_wire() {
        let mut config = AssemblyAISTTConfig {
            base: STTConfig {
                sample_rate: 16000,
                ..Default::default()
            },
            ..Default::default()
        };
        // Off → param omitted (matches provider default `language_detection=false`).
        assert!(!config.build_websocket_url().contains("language_detection"));
        // On → exact wire param present.
        config.language_detection = true;
        assert!(
            config
                .build_websocket_url()
                .contains("language_detection=true"),
            "language_detection=true missing from URL"
        );
    }

    /// KEYSTONE wire test: standardized SttFeatures → from_standard → URL. This is the
    /// reachable end-to-end path, and it also pins the CAPABILITY GAPS: sentiment /
    /// entity_detection are batch-only on AssemblyAI and must NOT appear on the streaming URL
    /// even when requested in the standardized features.
    #[test]
    fn from_standard_features_reach_the_wire_and_gaps_stay_off() {
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "assemblyai".into(),
                api_key: "k".into(),
                sample_rate: 16000,
                ..Default::default()
            },
            features: SttFeatures {
                keyterms: Some(vec!["WaaV".into(), "Universal-3".into()]),
                language_detection: Some(true),
                // Capability gaps — requested but unsupported on streaming:
                sentiment: Some(true),
                entity_detection: Some(true),
                ..Default::default()
            },
            extras: Default::default(),
            translation: None,
        };
        let cfg = AssemblyAISTTConfig::from_standard(&std);
        // Struct mapping happened.
        assert_eq!(cfg.keyterms_prompt, vec!["WaaV", "Universal-3"]);
        assert!(cfg.language_detection);

        let url = cfg.build_websocket_url();
        // Supported features reach the wire.
        assert!(
            url.contains("keyterms_prompt="),
            "keyterms not on wire: {url}"
        );
        let expected = encode_query_value(r#"["WaaV","Universal-3"]"#);
        assert!(
            url.contains(&format!("keyterms_prompt={expected}")),
            "keyterms JSON array not on wire: {url}"
        );
        assert!(
            url.contains("language_detection=true"),
            "language_detection not on wire: {url}"
        );
        // CAPABILITY GAPS: streaming v3 has no sentiment/entity_detection — they must be absent.
        assert!(
            !url.contains("sentiment"),
            "sentiment_analysis is batch-only and must not appear on the streaming URL: {url}"
        );
        assert!(
            !url.contains("entity_detection"),
            "entity_detection is batch-only and must not appear on the streaming URL: {url}"
        );
        // And `word_boost` is never emitted (streaming uses keyterms_prompt).
        assert!(
            !url.contains("word_boost"),
            "word_boost must not appear: {url}"
        );
    }

    // =========================================================================
    // Newly-wired streaming features (diarization + endpointing/VAD knobs).
    // Each test asserts the api_param reaches the SERIALIZED WebSocket URL — the
    // recurring "present on struct, never on wire" bug class.
    // =========================================================================

    fn base_cfg() -> AssemblyAISTTConfig {
        AssemblyAISTTConfig {
            base: STTConfig {
                sample_rate: 16000,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// diarization (typed) → `speaker_labels=true`; off → param omitted (provider default).
    #[test]
    fn speaker_labels_reaches_the_wire() {
        let mut config = base_cfg();
        assert!(
            !config.build_websocket_url().contains("speaker_labels"),
            "speaker_labels must be omitted when off (provider default)"
        );
        config.speaker_labels = true;
        assert!(
            config.build_websocket_url().contains("speaker_labels=true"),
            "speaker_labels=true missing from URL"
        );
    }

    /// max_speakers (extras) → `max_speakers=N`, only alongside diarization, clamped 1..=10.
    #[test]
    fn max_speakers_reaches_the_wire_only_with_diarization() {
        let mut config = base_cfg();
        // Set without diarization → omitted (server ignores it without speaker_labels).
        config.max_speakers = Some(4);
        assert!(
            !config.build_websocket_url().contains("max_speakers"),
            "max_speakers must not appear without speaker_labels"
        );
        // With diarization → present.
        config.speaker_labels = true;
        assert!(
            config.build_websocket_url().contains("max_speakers=4"),
            "max_speakers=4 missing from URL"
        );
        // Clamp out-of-range.
        config.max_speakers = Some(99);
        assert!(config.build_websocket_url().contains("max_speakers=10"));
    }

    /// max_turn_silence (typed/endpointing_ms) → `max_turn_silence=MS`.
    #[test]
    fn max_turn_silence_reaches_the_wire() {
        let mut config = base_cfg();
        assert!(!config.build_websocket_url().contains("max_turn_silence"));
        config.max_turn_silence = Some(700);
        assert!(
            config
                .build_websocket_url()
                .contains("max_turn_silence=700"),
            "max_turn_silence=700 missing from URL"
        );
    }

    /// min_turn_silence (extras) → `min_turn_silence=MS`.
    #[test]
    fn min_turn_silence_reaches_the_wire() {
        let mut config = base_cfg();
        assert!(!config.build_websocket_url().contains("min_turn_silence"));
        config.min_turn_silence = Some(160);
        assert!(
            config
                .build_websocket_url()
                .contains("min_turn_silence=160"),
            "min_turn_silence=160 missing from URL"
        );
    }

    /// vad_threshold (extras) → `vad_threshold=0.NN`.
    #[test]
    fn vad_threshold_reaches_the_wire() {
        let mut config = base_cfg();
        assert!(!config.build_websocket_url().contains("vad_threshold"));
        config.vad_threshold = Some(0.4);
        assert!(
            config.build_websocket_url().contains("vad_threshold=0.40"),
            "vad_threshold=0.40 missing from URL"
        );
    }

    /// inactivity_timeout (extras) → `inactivity_timeout=SECS`, clamped 5..=3600.
    #[test]
    fn inactivity_timeout_reaches_the_wire() {
        let mut config = base_cfg();
        assert!(!config.build_websocket_url().contains("inactivity_timeout"));
        config.inactivity_timeout = Some(30);
        assert!(
            config
                .build_websocket_url()
                .contains("inactivity_timeout=30"),
            "inactivity_timeout=30 missing from URL"
        );
        config.inactivity_timeout = Some(1); // below min → clamp to 5
        assert!(
            config
                .build_websocket_url()
                .contains("inactivity_timeout=5")
        );
    }

    /// domain (extras) → `domain=...` (URL-encoded).
    #[test]
    fn domain_reaches_the_wire() {
        let mut config = base_cfg();
        assert!(!config.build_websocket_url().contains("domain="));
        config.domain = Some("medical-v1".into());
        assert!(
            config.build_websocket_url().contains("domain=medical-v1"),
            "domain=medical-v1 missing from URL"
        );
    }

    /// KEYSTONE: standardized SttFeatures + extras → from_standard → URL, end-to-end.
    /// This is the reachable production path and pins that typed fields AND extras both land.
    #[test]
    fn from_standard_streaming_features_reach_the_wire() {
        let mut extras = serde_json::Map::new();
        extras.insert("max_speakers".into(), serde_json::json!(6));
        extras.insert("min_turn_silence".into(), serde_json::json!(120));
        extras.insert("vad_threshold".into(), serde_json::json!(0.55));
        extras.insert("inactivity_timeout".into(), serde_json::json!(45));
        extras.insert("domain".into(), serde_json::json!("medical-v1"));

        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "assemblyai".into(),
                api_key: "k".into(),
                sample_rate: 16000,
                ..Default::default()
            },
            features: SttFeatures {
                diarization: Some(true),   // → speaker_labels=true
                endpointing_ms: Some(640), // → max_turn_silence=640
                ..Default::default()
            },
            extras: ProviderExtras(extras),
            translation: None,
        };
        let cfg = AssemblyAISTTConfig::from_standard(&std);
        let url = cfg.build_websocket_url();

        assert!(
            url.contains("speaker_labels=true"),
            "diarization not on wire: {url}"
        );
        assert!(
            url.contains("max_speakers=6"),
            "max_speakers not on wire: {url}"
        );
        assert!(
            url.contains("max_turn_silence=640"),
            "endpointing_ms not on wire: {url}"
        );
        assert!(
            url.contains("min_turn_silence=120"),
            "min_turn_silence not on wire: {url}"
        );
        assert!(
            url.contains("vad_threshold=0.55"),
            "vad_threshold not on wire: {url}"
        );
        assert!(
            url.contains("inactivity_timeout=45"),
            "inactivity_timeout not on wire: {url}"
        );
        assert!(
            url.contains("domain=medical-v1"),
            "domain not on wire: {url}"
        );
    }
}
