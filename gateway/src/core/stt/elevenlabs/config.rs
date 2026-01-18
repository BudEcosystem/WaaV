//! Configuration types for ElevenLabs STT Real-Time API.
//!
//! This module contains all configuration-related types including:
//! - Audio format specifications
//! - Commit strategies for transcription finalization
//! - Regional endpoint selection
//! - Provider-specific configuration options

use super::super::base::STTConfig;
use url::form_urlencoded;

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
        if let Some(ref terms) = self.keyterms {
            if !terms.is_empty() {
                // URL-encode each keyterm and join with comma
                // Use form_urlencoded to properly escape special characters
                let encoded_terms: Vec<String> = terms
                    .iter()
                    .map(|t| {
                        form_urlencoded::byte_serialize(t.as_bytes()).collect::<String>()
                    })
                    .collect();
                url.push_str("&keyterms=");
                url.push_str(&encoded_terms.join(","));
            }
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

        url
    }

    /// Create a new configuration from base STTConfig.
    ///
    /// Automatically determines the audio format from the sample rate.
    pub fn from_base(base: STTConfig) -> Self {
        let audio_format = ElevenLabsAudioFormat::from_sample_rate(base.sample_rate);
        Self {
            base,
            audio_format,
            ..Default::default()
        }
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
