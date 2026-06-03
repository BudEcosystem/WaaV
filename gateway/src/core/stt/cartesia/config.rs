//! Configuration types for Cartesia STT WebSocket API.
//!
//! This module contains all configuration-related types including:
//! - Audio encoding specifications
//! - Provider-specific configuration options
//! - WebSocket URL construction
//! - Configuration validation

use super::super::base::{STTConfig, STTError};
use url::form_urlencoded;

// =============================================================================
// Audio Encoding
// =============================================================================

/// Supported audio encodings for Cartesia STT WebSocket API.
///
/// The Cartesia STT WebSocket accepts several raw-PCM `encoding` values in addition to the
/// default signed-16-bit LE; the format must match the bytes sent on the socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CartesiaAudioEncoding {
    /// PCM signed 16-bit little-endian (`pcm_s16le`, the default).
    #[default]
    PcmS16le,
    /// PCM 32-bit float little-endian (`pcm_f32le`).
    PcmF32le,
    /// PCM 8-bit μ-law (`pcm_mulaw`, telephony).
    PcmMulaw,
    /// PCM 8-bit A-law (`pcm_alaw`, telephony).
    PcmAlaw,
}

impl CartesiaAudioEncoding {
    /// Convert to the API query parameter value.
    #[inline]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::PcmS16le => "pcm_s16le",
            Self::PcmF32le => "pcm_f32le",
            Self::PcmMulaw => "pcm_mulaw",
            Self::PcmAlaw => "pcm_alaw",
        }
    }

    /// Parse a base `STTConfig.encoding` string onto a Cartesia encoding.
    ///
    /// Accepts the Cartesia wire spellings plus the shared-vocabulary aliases the rest of the
    /// gateway uses (`linear16` is the `STTConfig` default and means signed-16-bit LE). Unknown
    /// values fall back to the default so a stray value cannot get the connection rejected.
    pub fn from_base_str(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "pcm_f32le" | "f32le" | "pcm_float32" => Self::PcmF32le,
            "pcm_mulaw" | "mulaw" | "ulaw" | "pcmu" | "g711u" => Self::PcmMulaw,
            "pcm_alaw" | "alaw" | "pcma" | "g711a" => Self::PcmAlaw,
            // pcm_s16le / linear16 / pcm / "" -> default signed-16-bit LE
            _ => Self::PcmS16le,
        }
    }
}

// =============================================================================
// Supported Sample Rates
// =============================================================================

/// Supported sample rates for Cartesia STT API (in Hz).
pub const SUPPORTED_SAMPLE_RATES: &[u32] = &[8000, 16000, 22050, 24000, 44100, 48000];

/// Check if a sample rate is supported by Cartesia STT.
#[inline]
pub fn is_sample_rate_supported(sample_rate: u32) -> bool {
    SUPPORTED_SAMPLE_RATES.contains(&sample_rate)
}

// =============================================================================
// Main Configuration
// =============================================================================

/// Configuration specific to Cartesia STT WebSocket API.
///
/// This configuration extends the base `STTConfig` with Cartesia-specific
/// parameters for the WebSocket streaming API.
#[derive(Debug, Clone)]
pub struct CartesiaSTTConfig {
    /// Base STT configuration (shared across all providers).
    pub base: STTConfig,

    /// Cartesia model identifier.
    ///
    /// Currently the only supported model is "ink-whisper".
    pub model: String,

    /// Audio encoding format.
    ///
    /// Must match the format of audio data sent to the API.
    pub encoding: CartesiaAudioEncoding,

    /// Minimum volume threshold for Voice Activity Detection (0.0 to 1.0).
    ///
    /// Audio below this volume level will be treated as silence.
    pub min_volume: Option<f32>,

    /// Maximum silence duration in seconds before endpointing.
    ///
    /// When speech is followed by silence for this duration,
    /// the transcript will be finalized.
    pub max_silence_duration_secs: Option<f32>,

    /// Cartesia API version date (YYYY-MM-DD format).
    ///
    /// If not specified, the latest API version is used.
    pub cartesia_version: Option<String>,

    /// Short-lived client access token (Cartesia `access_token` query param).
    ///
    /// When set, the WebSocket authenticates with this ephemeral token instead of the
    /// long-lived `api_key` — the recommended pattern for browser/edge clients so the
    /// secret API key is never shipped to untrusted environments. Carried via the
    /// standardized extras passthrough.
    pub access_token: Option<String>,

    /// Override the WebSocket base endpoint (`scheme://host[:port]`) — e.g. `ws://127.0.0.1:PORT`.
    /// Carried from the standardized `endpoint_override`. Used by the in-repo chaos/integration
    /// mock WebSocket server; `None` → the production `WEBSOCKET_BASE_URL`.
    pub endpoint_override: Option<String>,
}

/// Default Cartesia API version date. REQUIRED on the STT WebSocket — without it the
/// connection is rejected. Previously defaulted to `None`, so every factory-built Cartesia
/// STT session failed to connect (BROKEN). Keep in sync with the TTS DEFAULT_API_VERSION.
pub const DEFAULT_API_VERSION: &str = "2025-04-16";

impl Default for CartesiaSTTConfig {
    fn default() -> Self {
        Self {
            base: STTConfig::default(),
            model: "ink-whisper".to_string(),
            encoding: CartesiaAudioEncoding::default(),
            min_volume: None,
            max_silence_duration_secs: None,
            cartesia_version: Some(DEFAULT_API_VERSION.to_string()),
            access_token: None,
            endpoint_override: None,
        }
    }
}

impl CartesiaSTTConfig {
    /// WebSocket base URL for Cartesia STT API.
    pub const WEBSOCKET_BASE_URL: &'static str = "wss://api.cartesia.ai/stt/websocket";

    /// Validate the configuration.
    ///
    /// Checks that:
    /// - API key is not empty
    /// - Sample rate is supported (8000, 16000, 22050, 24000, 44100, 48000)
    /// - Language is not empty
    /// - min_volume is in range 0.0-1.0 if set
    /// - max_silence_duration_secs is positive if set
    ///
    /// # Returns
    ///
    /// `Ok(())` if configuration is valid, otherwise `Err(STTError::ConfigurationError)`.
    pub fn validate(&self) -> Result<(), STTError> {
        // Check API key
        if self.base.api_key.is_empty() {
            return Err(STTError::ConfigurationError(
                "Cartesia API key is required".to_string(),
            ));
        }

        // Check sample rate
        if !is_sample_rate_supported(self.base.sample_rate) {
            return Err(STTError::ConfigurationError(format!(
                "Unsupported sample rate: {}. Supported rates: {:?}",
                self.base.sample_rate, SUPPORTED_SAMPLE_RATES
            )));
        }

        // Check language
        if self.base.language.is_empty() {
            return Err(STTError::ConfigurationError(
                "Language code is required".to_string(),
            ));
        }

        // Validate min_volume if set
        if let Some(min_vol) = self.min_volume
            && !(0.0..=1.0).contains(&min_vol)
        {
            return Err(STTError::ConfigurationError(format!(
                "min_volume must be between 0.0 and 1.0, got: {min_vol}"
            )));
        }

        // Validate max_silence_duration_secs if set
        if let Some(max_silence) = self.max_silence_duration_secs
            && max_silence <= 0.0
        {
            return Err(STTError::ConfigurationError(format!(
                "max_silence_duration_secs must be positive, got: {max_silence}"
            )));
        }

        Ok(())
    }

    /// Build the WebSocket URL with query parameters.
    ///
    /// Constructs the full WebSocket URL including:
    /// - Base URL
    /// - API key (authentication)
    /// - Model parameter
    /// - Language code
    /// - Audio encoding
    /// - Sample rate
    /// - Optional VAD parameters
    ///
    /// # Performance Note
    ///
    /// Uses pre-allocated String with estimated capacity (256 bytes)
    /// to minimize allocations during URL construction.
    ///
    /// # Example URL
    ///
    /// ```text
    /// wss://api.cartesia.ai/stt/websocket?api_key=xxx&model=ink-whisper&language=en&encoding=pcm_s16le&sample_rate=16000
    /// ```
    pub fn build_websocket_url(&self, api_key: &str) -> String {
        // URL-encode parameters that could contain special characters
        let encode =
            |s: &str| -> String { form_urlencoded::byte_serialize(s.as_bytes()).collect() };

        // Pre-allocate URL string capacity for performance
        let mut url = String::with_capacity(256);

        // Base URL: honor an `endpoint_override` (scheme://host[:port]) for the in-repo mock/proxy
        // (the chaos test points this at a local ws:// server); otherwise the production endpoint.
        match self.endpoint_override.as_deref().filter(|o| !o.is_empty()) {
            Some(o) => {
                url.push_str(o.trim_end_matches('/'));
                url.push_str("/stt/websocket");
            }
            None => url.push_str(Self::WEBSOCKET_BASE_URL),
        }

        // Authentication: a short-lived `access_token` (extras) takes precedence over the
        // long-lived `api_key` — the recommended pattern for untrusted clients. Exactly one
        // auth param is emitted so the two are never sent together.
        if let Some(token) = self.access_token.as_deref().filter(|t| !t.is_empty()) {
            url.push_str("?access_token=");
            url.push_str(&encode(token));
        } else {
            url.push_str("?api_key=");
            url.push_str(&encode(api_key));
        }

        // Required parameters - URL encode model and language
        url.push_str("&model=");
        url.push_str(&encode(&self.model));
        url.push_str("&language=");
        url.push_str(&encode(&self.base.language));
        url.push_str("&encoding=");
        url.push_str(self.encoding.as_str()); // Safe: enum value
        url.push_str("&sample_rate=");
        url.push_str(&self.base.sample_rate.to_string()); // Safe: numeric

        // Optional VAD parameters (numeric values, safe)
        if let Some(min_vol) = self.min_volume {
            url.push_str("&min_volume=");
            url.push_str(&min_vol.to_string());
        }

        if let Some(max_silence) = self.max_silence_duration_secs {
            url.push_str("&max_silence_duration_secs=");
            url.push_str(&max_silence.to_string());
        }

        if let Some(ref version) = self.cartesia_version {
            url.push_str("&cartesia_version=");
            url.push_str(&encode(version));
        }

        url
    }

    /// Create a new configuration from base STTConfig.
    ///
    /// Applies Cartesia-specific defaults while preserving base configuration values.
    pub fn from_base(base: STTConfig) -> Self {
        // Cartesia currently exposes a single streaming model ("ink-whisper"), so `base.model` is
        // intentionally NOT mapped here: the shared `STTConfig` default model is Deepgram-specific
        // ("nova-3") and forwarding an arbitrary value would only risk an invalid-model rejection.
        // When Cartesia adds selectable models, map a non-empty `base.model` onto `self.model`.
        Self {
            base,
            ..Default::default()
        }
    }

    /// Build from the standardized config (W1 keystone).
    ///
    /// Mapped onto Cartesia wire params:
    /// - `endpointing_ms` (typed) → `max_silence_duration_secs` (ms → seconds), the endpointing
    ///   window after speech before a transcript is finalized.
    /// - `base.model` (typed) → `model` query param — Cartesia now ships selectable streaming
    ///   models (e.g. `ink-2`); a non-empty, non-Deepgram-default model is honored.
    /// - `base.encoding` (typed) → `encoding` query param — formats beyond `pcm_s16le`
    ///   (`pcm_f32le`, `pcm_mulaw`, `pcm_alaw`).
    /// - `access_token` (extras) → `access_token` query param — a short-lived client token that
    ///   replaces `api_key` on the connect URL.
    ///
    /// Remaining standardized features (interim_results, diarization, word_timestamps,
    /// smart_format, profanity_filter, filler_words, vad_events, utterance_end, keyterms,
    /// redaction, entity/language detection) have no Cartesia field and stay at their defaults.
    pub fn from_standard(std: &crate::core::stt::standard::StandardSTTConfig) -> Self {
        let f = &std.features;
        let mut cfg = Self::from_base(std.base.clone());
        if let Some(ms) = f.endpointing_ms {
            cfg.max_silence_duration_secs = Some(ms as f32 / 1000.0);
        }
        // Streaming model selection (typed). `from_base` deliberately drops `base.model` because
        // the `STTConfig` default is the Deepgram-specific "nova-3"; honor any other explicit
        // value so callers can select e.g. `ink-2` without it being silently overridden.
        let model = std.base.model.trim();
        if !model.is_empty() && model != "nova-3" {
            cfg.model = model.to_string();
        }
        // Audio encoding selection (typed): map the shared `STTConfig.encoding` onto Cartesia's
        // `encoding` enum (formats beyond pcm_s16le). The `STTConfig` default ("linear16") maps
        // back to the Cartesia default, so the wire param is unchanged for default callers.
        cfg.encoding = CartesiaAudioEncoding::from_base_str(&std.base.encoding);
        // Short-lived client access token (extras) → `access_token` on the connect URL.
        if let Some(token) = std
            .extras
            .0
            .get("access_token")
            .and_then(|v| v.as_str())
            .filter(|t| !t.is_empty())
        {
            cfg.access_token = Some(token.to_string());
        }
        // Endpoint override (scheme://host[:port]) for the in-repo mock/proxy (chaos test).
        cfg.endpoint_override = std.endpoint_override().map(|s| s.to_string());
        cfg
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // W1 keystone: Cartesia's only mappable standardized feature is endpointing
    // (`endpointing_ms` -> `max_silence_duration_secs`, ms converted to seconds); the base
    // (api_key) carries through unchanged.
    #[test]
    fn from_standard_maps_endpointing() {
        use crate::core::stt::standard::{ProviderExtras, SttFeatures, StandardSTTConfig};
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "cartesia".into(),
                api_key: "test-key".into(),
                ..Default::default()
            },
            features: SttFeatures {
                endpointing_ms: Some(500),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
        };
        let cfg = CartesiaSTTConfig::from_standard(&std);
        assert_eq!(cfg.max_silence_duration_secs, Some(0.5)); // endpointing_ms -> seconds
        assert_eq!(cfg.base.api_key, "test-key"); // base carried through
    }

    // WIRE-LEVEL: a selected streaming model (e.g. `ink-2`) must reach the connect URL's
    // `model=` query param, not just sit on the config struct.
    #[test]
    fn model_selection_reaches_ws_url() {
        use crate::core::stt::standard::{ProviderExtras, SttFeatures, StandardSTTConfig};
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "cartesia".into(),
                api_key: "k".into(),
                language: "en".into(),
                model: "ink-2".into(),
                ..Default::default()
            },
            features: SttFeatures::default(),
            extras: ProviderExtras::default(),
        };
        let cfg = CartesiaSTTConfig::from_standard(&std);
        assert_eq!(cfg.model, "ink-2");
        let url = cfg.build_websocket_url("k");
        assert!(url.contains("&model=ink-2"), "model not on wire: {url}");
    }

    // WIRE-LEVEL: an audio encoding beyond pcm_s16le must reach the connect URL's `encoding=`
    // query param.
    #[test]
    fn encoding_selection_reaches_ws_url() {
        use crate::core::stt::standard::{ProviderExtras, SttFeatures, StandardSTTConfig};
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "cartesia".into(),
                api_key: "k".into(),
                language: "en".into(),
                encoding: "pcm_mulaw".into(),
                ..Default::default()
            },
            features: SttFeatures::default(),
            extras: ProviderExtras::default(),
        };
        let cfg = CartesiaSTTConfig::from_standard(&std);
        assert_eq!(cfg.encoding, CartesiaAudioEncoding::PcmMulaw);
        let url = cfg.build_websocket_url("k");
        assert!(url.contains("&encoding=pcm_mulaw"), "encoding not on wire: {url}");
    }

    // The shared default encoding ("linear16") must map back to Cartesia's default so default
    // callers' wire body is unchanged.
    #[test]
    fn default_linear16_encoding_maps_to_pcm_s16le() {
        use crate::core::stt::standard::StandardSTTConfig;
        let std = StandardSTTConfig::from_base(STTConfig {
            api_key: "k".into(),
            language: "en".into(),
            ..Default::default() // encoding == "linear16"
        });
        let cfg = CartesiaSTTConfig::from_standard(&std);
        assert_eq!(cfg.encoding, CartesiaAudioEncoding::PcmS16le);
        let url = cfg.build_websocket_url("k");
        assert!(url.contains("&encoding=pcm_s16le"), "default encoding wrong: {url}");
    }

    // WIRE-LEVEL: a short-lived `access_token` (extras) must reach the connect URL's
    // `access_token=` query param AND replace `api_key=` (exactly one auth param).
    #[test]
    fn access_token_extra_reaches_ws_url_and_replaces_api_key() {
        use crate::core::stt::standard::{ProviderExtras, SttFeatures, StandardSTTConfig};
        let mut extras = serde_json::Map::new();
        extras.insert("access_token".into(), serde_json::json!("ephemeral-xyz"));
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "cartesia".into(),
                api_key: "long-lived-key".into(),
                language: "en".into(),
                ..Default::default()
            },
            features: SttFeatures::default(),
            extras: ProviderExtras(extras),
        };
        let cfg = CartesiaSTTConfig::from_standard(&std);
        assert_eq!(cfg.access_token.as_deref(), Some("ephemeral-xyz"));
        let url = cfg.build_websocket_url(&cfg.base.api_key);
        assert!(
            url.contains("access_token=ephemeral-xyz"),
            "access_token not on wire: {url}"
        );
        // The long-lived api_key must NOT also be present once a token is supplied.
        assert!(
            !url.contains("api_key="),
            "api_key must be replaced by access_token: {url}"
        );
    }
}
