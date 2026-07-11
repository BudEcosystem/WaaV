//! Configuration types for ElevenLabs STT Real-Time API.
//!
//! This module contains all configuration-related types including:
//! - Audio format specifications
//! - Commit strategies for transcription finalization
//! - Regional endpoint selection
//! - Provider-specific configuration options

use super::super::base::STTConfig;
use url::form_urlencoded;

fn validate_elevenlabs_stt_endpoint(source: &str, endpoint: &str) -> Result<(), String> {
    let endpoint = endpoint.trim();
    if endpoint.is_empty() {
        return Ok(());
    }

    crate::core::net::validate_url_for_ssrf(endpoint, &["ws", "wss"])
        .map_err(|e| format!("{source} rejected (SSRF protection): {e}"))
}

// =============================================================================
// Audio Format
// =============================================================================

/// Supported audio formats for ElevenLabs STT Real-Time API.
///
/// All PCM formats are 16-bit signed little-endian.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ElevenLabsAudioFormat {
    /// 8kHz PCM 16-bit signed little-endian
    Pcm8000,
    /// 16kHz PCM 16-bit signed little-endian (common for telephony)
    #[default]
    Pcm16000,
    /// 22.05kHz PCM 16-bit signed little-endian
    Pcm22050,
    /// 24kHz PCM 16-bit signed little-endian (recommended by ElevenLabs)
    Pcm24000,
    /// 44.1kHz PCM 16-bit signed little-endian (CD quality)
    Pcm44100,
    /// 48kHz PCM 16-bit signed little-endian (professional audio)
    Pcm48000,
    /// 8kHz μ-law (telephony, SIP)
    Ulaw8000,
}

impl ElevenLabsAudioFormat {
    /// Convert to the API query parameter value.
    #[inline]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pcm8000 => "pcm_8000",
            Self::Pcm16000 => "pcm_16000",
            Self::Pcm22050 => "pcm_22050",
            Self::Pcm24000 => "pcm_24000",
            Self::Pcm44100 => "pcm_44100",
            Self::Pcm48000 => "pcm_48000",
            Self::Ulaw8000 => "ulaw_8000",
        }
    }

    /// Get the sample rate for this format in Hz.
    #[inline]
    pub fn sample_rate(&self) -> u32 {
        match self {
            Self::Pcm8000 | Self::Ulaw8000 => 8000,
            Self::Pcm16000 => 16000,
            Self::Pcm22050 => 22050,
            Self::Pcm24000 => 24000,
            Self::Pcm44100 => 44100,
            Self::Pcm48000 => 48000,
        }
    }

    /// Create from sample rate (defaults to PCM encoding).
    ///
    /// Unknown sample rates default to 16kHz PCM.
    #[inline]
    pub fn from_sample_rate(sample_rate: u32) -> Self {
        match sample_rate {
            8000 => Self::Pcm8000,
            16000 => Self::Pcm16000,
            22050 => Self::Pcm22050,
            24000 => Self::Pcm24000,
            44100 => Self::Pcm44100,
            48000 => Self::Pcm48000,
            _ => Self::Pcm16000, // Default to 16kHz for unknown rates
        }
    }
}

// =============================================================================
// Commit Strategy
// =============================================================================

/// Commit strategy for transcription finalization.
///
/// Controls how and when transcription results are finalized.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CommitStrategy {
    /// Manual commit - client controls when to finalize transcription.
    ///
    /// Use this when you want explicit control over speech boundaries.
    /// Call `commit: true` in the audio chunk message to finalize.
    Manual,

    /// VAD-based automatic commit (default).
    ///
    /// Transcription is automatically finalized when Voice Activity Detection
    /// detects end of speech. This is the recommended mode for most use cases.
    #[default]
    Vad,
}

impl CommitStrategy {
    /// Convert to the API query parameter value.
    #[inline]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Vad => "vad",
        }
    }
}

// =============================================================================
// Regional Endpoints
// =============================================================================

/// ElevenLabs regional endpoints for STT Real-Time API.
///
/// Choose the region closest to your users for optimal latency,
/// or use regional endpoints for data residency requirements.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ElevenLabsRegion {
    /// Default production endpoint (global)
    #[default]
    Default,
    /// US production endpoint
    Us,
    /// EU production endpoint (for EU data residency)
    Eu,
    /// India production endpoint (for India data residency)
    India,
}

impl ElevenLabsRegion {
    /// Get the WebSocket base URL for this region.
    #[inline]
    pub fn websocket_base_url(&self) -> &'static str {
        match self {
            Self::Default => "wss://api.elevenlabs.io",
            Self::Us => "wss://api.us.elevenlabs.io",
            Self::Eu => "wss://api.eu.residency.elevenlabs.io",
            Self::India => "wss://api.in.residency.elevenlabs.io",
        }
    }

    /// Get the host name for HTTP headers.
    #[inline]
    pub fn host(&self) -> &'static str {
        match self {
            Self::Default => "api.elevenlabs.io",
            Self::Us => "api.us.elevenlabs.io",
            Self::Eu => "api.eu.residency.elevenlabs.io",
            Self::India => "api.in.residency.elevenlabs.io",
        }
    }
}

// =============================================================================
// Main Configuration
// =============================================================================

/// Configuration specific to ElevenLabs STT Real-Time API.
///
/// This configuration extends the base `STTConfig` with ElevenLabs-specific
/// parameters for the WebSocket streaming API.
#[derive(Debug, Clone)]
pub struct ElevenLabsSTTConfig {
    /// Base STT configuration (shared across all providers).
    pub base: STTConfig,

    /// ElevenLabs model identifier.
    ///
    /// Available models for realtime WebSocket API:
    /// - `scribe_v2_realtime`: Real-time streaming model with ~150ms latency
    ///
    /// Note: `scribe_v1` is only for the batch transcription API, not realtime.
    pub model_id: String,

    /// Audio format for the WebSocket connection.
    ///
    /// Must match the format of audio data sent to the API.
    pub audio_format: ElevenLabsAudioFormat,

    /// Commit strategy for transcription finalization.
    ///
    /// - `Vad`: Automatic finalization based on voice activity detection
    /// - `Manual`: Client-controlled finalization via commit flag
    pub commit_strategy: CommitStrategy,

    /// Enable word-level timestamps in transcription results.
    ///
    /// When enabled, committed transcripts include timing information
    /// for each word.
    pub include_timestamps: bool,

    /// VAD silence threshold in seconds.
    ///
    /// Time of silence before considering speech has ended.
    /// Only applies when `commit_strategy` is `Vad`.
    pub vad_silence_threshold_secs: Option<f32>,

    /// VAD sensitivity threshold (0.0 to 1.0).
    ///
    /// Lower values = more sensitive to speech (may pick up more noise).
    /// Higher values = less sensitive (may miss quiet speech).
    pub vad_threshold: Option<f32>,

    /// Minimum speech duration in milliseconds.
    ///
    /// Speech segments shorter than this duration will be ignored.
    /// Helps filter out brief noise spikes.
    pub min_speech_duration_ms: Option<u32>,

    /// Minimum silence duration in milliseconds for end-of-speech detection.
    ///
    /// Controls how long silence must persist before speech is considered ended.
    pub min_silence_duration_ms: Option<u32>,

    /// Enable request logging for debugging.
    ///
    /// When enabled, ElevenLabs logs request data for debugging purposes.
    /// Disable in production for privacy.
    pub enable_logging: bool,

    /// Regional endpoint selection.
    ///
    /// Choose based on latency requirements or data residency needs.
    pub region: ElevenLabsRegion,

    // =========================================================================
    // Advanced Features (Entity Detection, Diarization, PII/PHI)
    // =========================================================================
    /// Key terms for improved transcription accuracy.
    ///
    /// A list of domain-specific terms, product names, or specialized vocabulary
    /// that may not be in the standard vocabulary. Maximum 100 terms.
    ///
    /// Example: `["ElevenLabs", "WebSocket", "API"]`
    pub keyterms: Option<Vec<String>>,

    /// Enable entity detection in transcription.
    ///
    /// When enabled, the API will detect and return named entities such as
    /// person names, organizations, locations, dates, numbers, etc.
    /// Supports 56 entity categories.
    pub enable_entity_detection: Option<bool>,

    /// Enable speaker diarization.
    ///
    /// When enabled, the API will identify and label different speakers
    /// in the audio. Requires `include_timestamps` to be true for full
    /// speaker information per word.
    pub enable_diarization: Option<bool>,

    /// Maximum number of speakers for diarization.
    ///
    /// Specifies the expected maximum number of speakers in the audio.
    /// Helps the model optimize speaker separation. Maximum 48 speakers.
    /// Only applies when `enable_diarization` is true.
    pub max_speakers: Option<u8>,

    /// Enable PII (Personally Identifiable Information) detection.
    ///
    /// When enabled, the API will detect sensitive personal information
    /// such as names, addresses, phone numbers, email addresses, SSNs, etc.
    /// Results are returned in the sensitive_data field.
    pub enable_pii_detection: Option<bool>,

    /// Enable PHI (Protected Health Information) detection.
    ///
    /// When enabled, the API will detect health-related sensitive information
    /// such as medical conditions, medications, patient IDs, etc.
    /// Results are returned in the sensitive_data field.
    /// Useful for HIPAA compliance in healthcare applications.
    pub enable_phi_detection: Option<bool>,

    /// Detect and report the spoken language (ElevenLabs `include_language_detection`).
    ///
    /// When `Some(true)`, the realtime API runs automatic language identification and
    /// returns the detected language alongside the transcript. Maps from the standardized
    /// `SttFeatures::language_detection`.
    pub include_language_detection: Option<bool>,

    /// Suppress filler words / disfluencies (ElevenLabs `no_verbatim`).
    ///
    /// This is the INVERSE of the standardized `SttFeatures::filler_words`: a caller asking
    /// to *keep* filler words (`filler_words = true`) sets `no_verbatim = false`, and a caller
    /// asking to drop them (`filler_words = false`) sets `no_verbatim = true`.
    pub no_verbatim: Option<bool>,

    /// Test/diagnostic WebSocket endpoint override.
    ///
    /// When set (and non-empty), this replaces the region-derived base URL in
    /// [`ElevenLabsSTT::build_websocket_url`], so the connection can be redirected at a
    /// localhost mock WebSocket server (e.g. `ws://127.0.0.1:PORT`). `None` uses the
    /// region's production endpoint. Plumbed from `StandardSTTConfig::endpoint_override`.
    pub endpoint_override: Option<String>,
}

impl Default for ElevenLabsSTTConfig {
    fn default() -> Self {
        Self {
            base: STTConfig::default(),
            model_id: "scribe_v2_realtime".to_string(),
            audio_format: ElevenLabsAudioFormat::default(),
            commit_strategy: CommitStrategy::default(),
            include_timestamps: false,
            // Set sensible VAD defaults for responsive end-of-speech detection
            // Similar to Deepgram's endpointing=200ms and utterance_end_ms=500ms
            vad_silence_threshold_secs: Some(0.5), // 500ms silence triggers commit
            vad_threshold: None,                   // Use API default sensitivity
            min_speech_duration_ms: Some(50),      // Minimum 50ms of speech to count
            min_silence_duration_ms: Some(300),    // 300ms silence for end-of-speech
            enable_logging: false,
            region: ElevenLabsRegion::default(),
            // Advanced features disabled by default
            keyterms: None,
            enable_entity_detection: None,
            enable_diarization: None,
            max_speakers: None,
            enable_pii_detection: None,
            enable_phi_detection: None,
            include_language_detection: None,
            no_verbatim: None,
            endpoint_override: None,
        }
    }
}

/// Maximum number of keyterms allowed.
pub const MAX_KEYTERMS: usize = 100;

/// Maximum number of speakers for diarization.
pub const MAX_SPEAKERS: u8 = 48;

impl ElevenLabsSTTConfig {
    /// Validate the configuration.
    ///
    /// Checks:
    /// - API key is not empty
    /// - Keyterms count doesn't exceed maximum (100)
    /// - Max speakers doesn't exceed maximum (48)
    ///
    /// # Returns
    /// * `Ok(())` if configuration is valid
    /// * `Err(String)` with error description if invalid
    pub fn validate(&self) -> Result<(), String> {
        // Validate API key
        if self.base.api_key.is_empty() {
            return Err("API key is required".to_string());
        }

        // Validate keyterms count
        if let Some(ref terms) = self.keyterms {
            if terms.len() > MAX_KEYTERMS {
                return Err(format!(
                    "Too many keyterms: {} provided, maximum is {}",
                    terms.len(),
                    MAX_KEYTERMS
                ));
            }
            // Validate each keyterm is not empty
            for (i, term) in terms.iter().enumerate() {
                if term.trim().is_empty() {
                    return Err(format!("Keyterm at index {} is empty", i));
                }
            }
        }

        // Validate max speakers
        if let Some(max) = self.max_speakers {
            if max > MAX_SPEAKERS {
                return Err(format!(
                    "max_speakers {} exceeds maximum of {}",
                    max, MAX_SPEAKERS
                ));
            }
            if max == 0 {
                return Err("max_speakers must be at least 1".to_string());
            }
        }

        // Warn if diarization is enabled but timestamps are not
        // (diarization requires timestamps for per-word speaker info)
        if self.enable_diarization == Some(true) && !self.include_timestamps {
            // Not an error, but diarization results will be limited
            tracing::warn!(
                "Diarization enabled without timestamps - \
                 speaker info will only be at segment level, not per-word"
            );
        }

        if let Some(endpoint) = self.endpoint_override.as_deref() {
            validate_elevenlabs_stt_endpoint("endpoint_override", endpoint)?;
        }

        Ok(())
    }

    /// Build the WebSocket URL with query parameters.
    ///
    /// Constructs the full WebSocket URL including:
    /// - Regional endpoint base URL
    /// - API path
    /// - All configuration query parameters
    ///
    /// # Performance Note
    ///
    /// Uses pre-allocated String with estimated capacity (512 bytes)
    /// to minimize allocations during URL construction.
    pub fn build_websocket_url(&self) -> String {
        let base_url = self.region.websocket_base_url();
        let mut url = format!(
            "{}/v1/speech-to-text/realtime?model_id={}&audio_format={}&commit_strategy={}",
            base_url,
            self.model_id,
            self.audio_format.as_str(),
            self.commit_strategy.as_str()
        );

        // Add language if specified and not empty
        if !self.base.language.is_empty() {
            url.push_str("&language_code=");
            url.push_str(&self.base.language);
        }

        // Add optional parameters
        if self.include_timestamps {
            url.push_str("&include_timestamps=true");
        }

        if let Some(threshold) = self.vad_silence_threshold_secs {
            url.push_str(&format!("&vad_silence_threshold_secs={threshold}"));
        }

        if let Some(threshold) = self.vad_threshold {
            url.push_str(&format!("&vad_threshold={threshold}"));
        }

        if let Some(duration) = self.min_speech_duration_ms {
            url.push_str(&format!("&min_speech_duration_ms={duration}"));
        }

        if let Some(duration) = self.min_silence_duration_ms {
            url.push_str(&format!("&min_silence_duration_ms={duration}"));
        }

        if self.enable_logging {
            url.push_str("&enable_logging=true");
        }

        // Add advanced feature parameters
        if let Some(ref terms) = self.keyterms
            && !terms.is_empty()
        {
            // URL-encode each keyterm and join with comma
            // Use form_urlencoded to properly escape special characters
            let encoded_terms: Vec<String> = terms
                .iter()
                .map(|t| form_urlencoded::byte_serialize(t.as_bytes()).collect::<String>())
                .collect();
            url.push_str("&keyterms=");
            url.push_str(&encoded_terms.join(","));
        }

        if self.enable_entity_detection == Some(true) {
            url.push_str("&entity_detection=true");
        }

        if self.enable_diarization == Some(true) {
            url.push_str("&diarization=true");
            if let Some(max) = self.max_speakers {
                url.push_str(&format!("&max_speakers={max}"));
            }
        }

        if self.enable_pii_detection == Some(true) {
            url.push_str("&pii_detection=true");
        }

        if self.enable_phi_detection == Some(true) {
            url.push_str("&phi_detection=true");
        }

        // Automatic spoken-language detection (only emitted when explicitly set, so the default
        // connect URL is unchanged).
        if let Some(detect) = self.include_language_detection {
            url.push_str("&include_language_detection=");
            url.push_str(if detect { "true" } else { "false" });
        }

        // Filler-word suppression. ElevenLabs' `no_verbatim` is the inverse of "keep filler
        // words"; only emitted when explicitly configured.
        if let Some(no_verbatim) = self.no_verbatim {
            url.push_str("&no_verbatim=");
            url.push_str(if no_verbatim { "true" } else { "false" });
        }

        url
    }

    /// Create a new configuration from base STTConfig.
    ///
    /// Automatically determines the audio format from the sample rate.
    pub fn from_base(base: STTConfig) -> Self {
        let audio_format = ElevenLabsAudioFormat::from_sample_rate(base.sample_rate);
        // Start from defaults (model_id = the realtime default), then honor an explicitly
        // configured model. Previously `base.model` was dropped on the floor, so a caller who
        // selected a specific realtime model was silently overridden to the default.
        let mut cfg = Self {
            audio_format,
            ..Default::default()
        };
        if !base.model.is_empty() {
            cfg.model_id = base.model.clone();
        }
        cfg.base = base;
        cfg
    }

    /// Build from the standardized config (W1 keystone). ElevenLabs exposes a rich advanced
    /// surface (word timestamps, diarization, entity detection, key terms, PII/PHI redaction,
    /// automatic language detection, filler-word suppression), so this maps those features
    /// through the standardized API — previously unreachable via the flat factory. Features
    /// ElevenLabs cannot express (smart_format, profanity_filter, interim_results, vad_events,
    /// endpointing) stay at provider defaults.
    pub fn from_standard(std: &crate::core::stt::standard::StandardSTTConfig) -> Self {
        let f = &std.features;
        let mut cfg = Self::from_base(std.base.clone());
        if let Some(w) = f.word_timestamps {
            cfg.include_timestamps = w;
        }
        if let Some(d) = f.diarization {
            cfg.enable_diarization = Some(d);
        }
        if let Some(e) = f.entity_detection {
            cfg.enable_entity_detection = Some(e);
        }
        if let Some(k) = &f.keyterms {
            cfg.keyterms = Some(k.clone());
        }
        if let Some(r) = &f.redaction {
            // Redaction categories map to PII detection; health-related categories also enable PHI.
            cfg.enable_pii_detection = Some(!r.is_empty());
            let phi = r
                .iter()
                .any(|c| c.eq_ignore_ascii_case("phi") || c.to_lowercase().contains("health"));
            if phi {
                cfg.enable_phi_detection = Some(true);
            }
        }
        // Automatic spoken-language detection (typed) -> `include_language_detection`.
        if let Some(detect) = f.language_detection {
            cfg.include_language_detection = Some(detect);
        }
        // Filler words (typed) -> `no_verbatim` (INVERTED): keep filler words => no_verbatim=false.
        if let Some(keep_fillers) = f.filler_words {
            cfg.no_verbatim = Some(!keep_fillers);
        }
        // Plumb the test/diagnostic WS endpoint override (e.g. localhost mock) through from the
        // standardized config so `build_websocket_url` can redirect the connection.
        cfg.endpoint_override = std.endpoint_override().map(|s| s.to_string());
        cfg
    }

    /// Check if entity detection is enabled.
    #[inline]
    pub fn has_entity_detection(&self) -> bool {
        self.enable_entity_detection == Some(true)
    }

    /// Check if diarization is enabled.
    #[inline]
    pub fn has_diarization(&self) -> bool {
        self.enable_diarization == Some(true)
    }

    /// Check if PII detection is enabled.
    #[inline]
    pub fn has_pii_detection(&self) -> bool {
        self.enable_pii_detection == Some(true)
    }

    /// Check if PHI detection is enabled.
    #[inline]
    pub fn has_phi_detection(&self) -> bool {
        self.enable_phi_detection == Some(true)
    }

    /// Check if any sensitive data detection is enabled (PII or PHI).
    #[inline]
    pub fn has_sensitive_data_detection(&self) -> bool {
        self.has_pii_detection() || self.has_phi_detection()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // W1 keystone: the standardized features unlock ElevenLabs' advanced feature surface
    // (diarization + key terms) — previously unreachable via the flat factory.
    #[test]
    fn from_standard_maps_features() {
        use crate::core::stt::standard::{StandardSTTConfig, SttFeatures};
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "elevenlabs".into(),
                api_key: "test-key".into(),
                ..Default::default()
            },
            features: SttFeatures {
                diarization: Some(true),
                keyterms: Some(vec!["WaaV".into(), "ElevenLabs".into()]),
                ..Default::default()
            },
            ..StandardSTTConfig::from_base(STTConfig::default())
        };
        let cfg = ElevenLabsSTTConfig::from_standard(&std);
        assert_eq!(cfg.enable_diarization, Some(true));
        assert_eq!(
            cfg.keyterms,
            Some(vec!["WaaV".to_string(), "ElevenLabs".to_string()])
        );
    }

    // WIRE-LEVEL: `language_detection` (typed) must map onto `include_language_detection` AND
    // reach the connect URL query string.
    #[test]
    fn language_detection_reaches_ws_url() {
        use crate::core::stt::standard::{ProviderExtras, StandardSTTConfig, SttFeatures};
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "elevenlabs".into(),
                api_key: "k".into(),
                ..Default::default()
            },
            features: SttFeatures {
                language_detection: Some(true),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
            translation: None,
        };
        let cfg = ElevenLabsSTTConfig::from_standard(&std);
        assert_eq!(cfg.include_language_detection, Some(true));
        let url = cfg.build_websocket_url();
        assert!(
            url.contains("&include_language_detection=true"),
            "language detection not on wire: {url}"
        );
    }

    // WIRE-LEVEL: `filler_words` (typed) must map onto `no_verbatim` (INVERTED) AND reach the
    // connect URL. Keeping filler words => no_verbatim=false.
    #[test]
    fn filler_words_map_to_no_verbatim_inverted_on_ws_url() {
        use crate::core::stt::standard::{ProviderExtras, StandardSTTConfig, SttFeatures};

        // filler_words = false (drop them) => no_verbatim = true
        let drop = StandardSTTConfig {
            base: STTConfig {
                api_key: "k".into(),
                ..Default::default()
            },
            features: SttFeatures {
                filler_words: Some(false),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
            translation: None,
        };
        let cfg = ElevenLabsSTTConfig::from_standard(&drop);
        assert_eq!(cfg.no_verbatim, Some(true));
        assert!(
            cfg.build_websocket_url().contains("&no_verbatim=true"),
            "no_verbatim=true expected when dropping fillers"
        );

        // filler_words = true (keep them) => no_verbatim = false
        let keep = StandardSTTConfig {
            base: STTConfig {
                api_key: "k".into(),
                ..Default::default()
            },
            features: SttFeatures {
                filler_words: Some(true),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
            translation: None,
        };
        let cfg = ElevenLabsSTTConfig::from_standard(&keep);
        assert_eq!(cfg.no_verbatim, Some(false));
        assert!(
            cfg.build_websocket_url().contains("&no_verbatim=false"),
            "no_verbatim=false expected when keeping fillers"
        );
    }

    // Neither param is emitted when the feature is unset (default connect URL unchanged).
    #[test]
    fn absent_features_omit_both_params() {
        let cfg = ElevenLabsSTTConfig {
            base: STTConfig {
                api_key: "k".into(),
                ..Default::default()
            },
            ..Default::default()
        };
        let url = cfg.build_websocket_url();
        assert!(!url.contains("include_language_detection"), "url: {url}");
        assert!(!url.contains("no_verbatim"), "url: {url}");
    }

    #[test]
    fn test_config_validation_rejects_ssrf_endpoint_override() {
        let _guard = crate::core::net::test_env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let previous = std::env::var_os("WAAV_ALLOW_LOOPBACK_ENDPOINTS");
        // SAFETY: test-only env mutation, serialized by core::net::test_env_lock.
        unsafe { std::env::remove_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS") };

        let mut config = ElevenLabsSTTConfig {
            base: STTConfig {
                api_key: "test-key".to_string(),
                ..Default::default()
            },
            endpoint_override: Some("wss://elevenlabs-proxy.example.com".to_string()),
            ..Default::default()
        };
        assert!(config.validate().is_ok());

        config.endpoint_override = Some("ws://elevenlabs-proxy.example.com".to_string());
        assert!(config.validate().is_ok());

        config.endpoint_override = Some("ws://127.0.0.1:9000".to_string());
        let err = config
            .validate()
            .expect_err("loopback endpoint_override must be rejected");
        assert!(err.contains("SSRF protection"), "{err}");

        config.endpoint_override = Some("file:///tmp/socket".to_string());
        let err = config
            .validate()
            .expect_err("non-WebSocket endpoint_override must be rejected");
        assert!(err.contains("not allowed"), "{err}");

        config.endpoint_override = Some("https://elevenlabs-proxy.example.com".to_string());
        let err = config
            .validate()
            .expect_err("HTTP endpoint_override must be rejected for ElevenLabs WebSocket dial");
        assert!(err.contains("not allowed"), "{err}");

        // SAFETY: restore the process env before releasing the test env lock.
        unsafe {
            if let Some(previous) = previous {
                std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", previous);
            } else {
                std::env::remove_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS");
            }
        }
    }
}
