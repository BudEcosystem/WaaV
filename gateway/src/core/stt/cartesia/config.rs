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
/// Currently only PCM signed 16-bit little-endian is supported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CartesiaAudioEncoding {
    /// PCM signed 16-bit little-endian
    #[default]
    PcmS16le,
}

impl CartesiaAudioEncoding {
    /// Convert to the API query parameter value.
    #[inline]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::PcmS16le => "pcm_s16le",
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

        // Base URL
        url.push_str(Self::WEBSOCKET_BASE_URL);

        // Required parameters - URL encode api_key, model, and language
        url.push_str("?api_key=");
        url.push_str(&encode(api_key));
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

    /// Build from the standardized config (W1 keystone). Cartesia's `ink-whisper` streaming model
    /// exposes a narrow VAD-oriented surface, so the only standardized feature it can actually
    /// express is endpointing: `endpointing_ms` (silence after speech before finalizing) maps onto
    /// `max_silence_duration_secs` (Cartesia's endpointing window, expressed in seconds — converted
    /// from milliseconds). The other standardized features (interim_results, diarization,
    /// word_timestamps, smart_format, profanity_filter, filler_words, vad_events, utterance_end,
    /// keyterms, redaction, entity/language detection) have no corresponding Cartesia field and are
    /// capability gaps that stay at their defaults.
    pub fn from_standard(std: &crate::core::stt::standard::StandardSTTConfig) -> Self {
        let f = &std.features;
        let mut cfg = Self::from_base(std.base.clone());
        if let Some(ms) = f.endpointing_ms {
            cfg.max_silence_duration_secs = Some(ms as f32 / 1000.0);
        }
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
}
