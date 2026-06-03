//! Murf.ai TTS Configuration Types
//!
//! This module defines configuration types for the Murf.ai TTS provider,
//! including model selection, audio formats, regional endpoints, and
//! speech customization parameters.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

use crate::core::tts::{TTSConfig, TTSError, TTSResult};

use super::{
    DEFAULT_SAMPLE_RATE, DEFAULT_VOICE_ID, MAX_PITCH, MAX_RATE, MAX_VARIATION, MIN_PITCH, MIN_RATE,
    MIN_VARIATION,
};

// =============================================================================
// Model Selection
// =============================================================================

/// Murf.ai TTS model selection
///
/// # Models
///
/// - **Falcon**: Ultra-low latency (~130ms TTFA), optimized for real-time conversational AI
/// - **Gen2**: Studio-quality output with full customization options
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum MurfModel {
    /// Falcon model - ultra-low latency (~55ms model latency, sub-130ms TTFA)
    /// Optimized for real-time applications, voice agents, conversational AI.
    /// Supports: voiceId, text, multiNativeLocale, format, sampleRate, channelType
    #[default]
    Falcon,

    /// Gen2 model - studio-quality output with full customization
    /// Supports: rate, pitch, style, pronunciationDictionary, audioDuration, variation
    Gen2,
}

impl MurfModel {
    /// Returns the API string representation
    #[inline]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Falcon => "FALCON",
            Self::Gen2 => "GEN2",
        }
    }

    /// Parse from string (case-insensitive)
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_uppercase().as_str() {
            "FALCON" => Some(Self::Falcon),
            "GEN2" => Some(Self::Gen2),
            _ => None,
        }
    }

    /// Check if this model supports customization parameters
    #[inline]
    pub const fn supports_customization(&self) -> bool {
        matches!(self, Self::Gen2)
    }
}

impl fmt::Display for MurfModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =============================================================================
// Audio Format
// =============================================================================

/// Murf.ai supported audio formats
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum MurfAudioFormat {
    /// Uncompressed WAV - highest quality, larger file size
    Wav,

    /// Compressed MP3 - widely supported, good compression
    Mp3,

    /// Lossless FLAC - high quality with compression
    Flac,

    /// Raw PCM - lowest latency for streaming, no processing overhead
    #[default]
    Pcm,

    /// OGG Vorbis - efficient streaming format
    Ogg,

    /// A-law - telephony format (8kHz mono, G.711)
    Alaw,

    /// μ-law - telephony format (8kHz mono, G.711)
    Ulaw,
}

impl MurfAudioFormat {
    /// Returns the API string representation
    #[inline]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Wav => "WAV",
            Self::Mp3 => "MP3",
            Self::Flac => "FLAC",
            Self::Pcm => "PCM",
            Self::Ogg => "OGG",
            Self::Alaw => "ALAW",
            Self::Ulaw => "ULAW",
        }
    }

    /// Parse from string (case-insensitive)
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_uppercase().as_str() {
            "WAV" => Some(Self::Wav),
            "MP3" => Some(Self::Mp3),
            "FLAC" => Some(Self::Flac),
            "PCM" | "RAW" => Some(Self::Pcm),
            "OGG" | "VORBIS" => Some(Self::Ogg),
            "ALAW" | "A-LAW" => Some(Self::Alaw),
            "ULAW" | "U-LAW" | "MULAW" | "MU-LAW" => Some(Self::Ulaw),
            _ => None,
        }
    }

    /// Get the MIME type for this format
    #[inline]
    pub const fn mime_type(&self) -> &'static str {
        match self {
            Self::Wav => "audio/wav",
            Self::Mp3 => "audio/mpeg",
            Self::Flac => "audio/flac",
            Self::Pcm => "audio/pcm",
            Self::Ogg => "audio/ogg",
            Self::Alaw => "audio/pcm-alaw",
            Self::Ulaw => "audio/pcm-ulaw",
        }
    }

    /// Check if this format requires 8kHz mono (telephony formats)
    #[inline]
    pub const fn requires_telephony_settings(&self) -> bool {
        matches!(self, Self::Alaw | Self::Ulaw)
    }

    /// Get the default sample rate for this format
    #[inline]
    pub const fn default_sample_rate(&self) -> u32 {
        if self.requires_telephony_settings() {
            8000
        } else {
            DEFAULT_SAMPLE_RATE
        }
    }
}

impl fmt::Display for MurfAudioFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =============================================================================
// Regional Endpoints
// =============================================================================

/// Murf.ai regional streaming endpoints for data residency and latency optimization
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MurfRegion {
    /// Global router - auto-routes to nearest region
    #[default]
    Global,

    /// US East (Virginia) - highest concurrency (15)
    UsEast,

    /// US West (Oregon)
    UsWest,

    /// India (Mumbai)
    India,

    /// Canada (Montreal)
    Canada,

    /// EU Central (Frankfurt)
    EuCentral,

    /// United Kingdom (London)
    Uk,

    /// Japan (Tokyo)
    Japan,

    /// Australia (Sydney)
    Australia,

    /// South Korea (Seoul)
    SouthKorea,

    /// UAE (Dubai)
    Uae,

    /// Brazil (São Paulo)
    Brazil,
}

impl MurfRegion {
    /// Get the streaming endpoint URL for this region
    #[inline]
    pub const fn streaming_url(&self) -> &'static str {
        match self {
            Self::Global => "https://global.api.murf.ai/v1/speech/stream",
            Self::UsEast => "https://us-east.api.murf.ai/v1/speech/stream",
            Self::UsWest => "https://us-west.api.murf.ai/v1/speech/stream",
            Self::India => "https://in.api.murf.ai/v1/speech/stream",
            Self::Canada => "https://ca.api.murf.ai/v1/speech/stream",
            Self::EuCentral => "https://eu.api.murf.ai/v1/speech/stream",
            Self::Uk => "https://uk.api.murf.ai/v1/speech/stream",
            Self::Japan => "https://jp.api.murf.ai/v1/speech/stream",
            Self::Australia => "https://au.api.murf.ai/v1/speech/stream",
            Self::SouthKorea => "https://kr.api.murf.ai/v1/speech/stream",
            Self::Uae => "https://ae.api.murf.ai/v1/speech/stream",
            Self::Brazil => "https://br.api.murf.ai/v1/speech/stream",
        }
    }

    /// Get the region subdomain prefix
    #[inline]
    pub const fn subdomain(&self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::UsEast => "us-east",
            Self::UsWest => "us-west",
            Self::India => "in",
            Self::Canada => "ca",
            Self::EuCentral => "eu",
            Self::Uk => "uk",
            Self::Japan => "jp",
            Self::Australia => "au",
            Self::SouthKorea => "kr",
            Self::Uae => "ae",
            Self::Brazil => "br",
        }
    }

    /// Get the default concurrency limit for this region
    #[inline]
    pub const fn default_concurrency(&self) -> u32 {
        match self {
            Self::UsEast => 15,
            _ => 2,
        }
    }

    /// Parse from string (case-insensitive)
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "global" | "" => Some(Self::Global),
            "us-east" | "us_east" | "useast" => Some(Self::UsEast),
            "us-west" | "us_west" | "uswest" => Some(Self::UsWest),
            "india" | "in" | "mumbai" => Some(Self::India),
            "canada" | "ca" | "montreal" => Some(Self::Canada),
            "eu" | "eu-central" | "eu_central" | "frankfurt" => Some(Self::EuCentral),
            "uk" | "london" => Some(Self::Uk),
            "japan" | "jp" | "tokyo" => Some(Self::Japan),
            "australia" | "au" | "sydney" => Some(Self::Australia),
            "south-korea" | "south_korea" | "korea" | "kr" | "seoul" => Some(Self::SouthKorea),
            "uae" | "ae" | "dubai" => Some(Self::Uae),
            "brazil" | "br" | "sao-paulo" => Some(Self::Brazil),
            _ => None,
        }
    }
}

impl fmt::Display for MurfRegion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.subdomain())
    }
}

// =============================================================================
// Channel Type
// =============================================================================

/// Audio channel type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum MurfChannelType {
    /// Single channel audio
    #[default]
    Mono,

    /// Dual channel audio
    Stereo,
}

impl MurfChannelType {
    /// Returns the API string representation
    #[inline]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Mono => "MONO",
            Self::Stereo => "STEREO",
        }
    }

    /// Parse from string (case-insensitive)
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_uppercase().as_str() {
            "MONO" | "1" => Some(Self::Mono),
            "STEREO" | "2" => Some(Self::Stereo),
            _ => None,
        }
    }
}

impl fmt::Display for MurfChannelType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =============================================================================
// Murf TTS Configuration
// =============================================================================

/// Murf.ai TTS configuration with all supported parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MurfTtsConfig {
    /// API key for authentication
    #[serde(skip_serializing)]
    pub api_key: String,

    /// Voice ID (e.g., "en-US-natalie" or "natalie")
    pub voice_id: String,

    /// TTS model (Falcon for real-time, Gen2 for studio quality)
    pub model: MurfModel,

    /// Audio output format
    pub format: MurfAudioFormat,

    /// Sample rate in Hz (8000, 24000, 44100, 48000)
    pub sample_rate: u32,

    /// Channel type (mono/stereo)
    pub channel_type: MurfChannelType,

    /// Regional endpoint for data residency and latency
    pub region: MurfRegion,

    /// Multi-native locale for natural cross-language pronunciation (Falcon only)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub multi_native_locale: Option<String>,

    // === Gen2 Model Only Parameters ===
    /// Speech rate adjustment (-50 to 50, 0 = normal)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate: Option<i32>,

    /// Pitch adjustment (-50 to 50, 0 = normal)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pitch: Option<i32>,

    /// Speaking style (e.g., "Conversational", "Newscast")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,

    /// Variation level (0-5) for dynamic delivery
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variation: Option<u8>,

    /// Custom pronunciation dictionary
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pronunciation_dictionary: Option<HashMap<String, String>>,

    /// Target audio duration in seconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_duration: Option<f32>,

    /// Encode response as Base64 (zero data retention)
    #[serde(default)]
    pub encode_as_base64: bool,

    /// Custom streaming endpoint override
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_endpoint: Option<String>,
}

impl Default for MurfTtsConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            voice_id: DEFAULT_VOICE_ID.to_string(),
            model: MurfModel::default(),
            format: MurfAudioFormat::default(),
            sample_rate: DEFAULT_SAMPLE_RATE,
            channel_type: MurfChannelType::default(),
            region: MurfRegion::default(),
            multi_native_locale: None,
            rate: None,
            pitch: None,
            style: None,
            variation: None,
            pronunciation_dictionary: None,
            audio_duration: None,
            encode_as_base64: false,
            custom_endpoint: None,
        }
    }
}

impl MurfTtsConfig {
    /// Create a new configuration with the given API key
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            ..Default::default()
        }
    }

    /// Create configuration from base TTSConfig
    ///
    /// This extracts Murf-specific settings from the base TTS config.
    /// The model name can contain model type (e.g., "FALCON" or "GEN2").
    /// Additional Murf-specific options can be set using the builder methods
    /// after construction.
    pub fn from_base(config: &TTSConfig) -> TTSResult<Self> {
        // Validate API key - check config first, then environment
        let api_key = if !config.api_key.is_empty() {
            config.api_key.clone()
        } else {
            std::env::var("MURF_API_KEY").map_err(|_| {
                TTSError::InvalidConfiguration(
                    "MURF_API_KEY environment variable not set and no api_key provided".to_string(),
                )
            })?
        };

        // Parse model from config.model field (e.g., "FALCON", "GEN2")
        let model = if !config.model.is_empty() {
            config
                .model
                .as_str()
                .split_whitespace()
                .next()
                .and_then(MurfModel::from_str)
                .unwrap_or_default()
        } else {
            MurfModel::default() // Default to Falcon
        };

        // Parse audio format from config
        let format = config
            .audio_format
            .as_ref()
            .and_then(|f| MurfAudioFormat::from_str(f))
            .unwrap_or_default();

        // Get sample rate with format-specific default
        let sample_rate = config.sample_rate.unwrap_or(format.default_sample_rate());

        // Use defaults for extended options - these can be set via builder methods
        // Region defaults to Global (auto-routing)
        let region = MurfRegion::default();

        // Channel type defaults to Mono
        let channel_type = MurfChannelType::default();

        let murf_config = Self {
            api_key,
            voice_id: config
                .voice_id
                .clone()
                .unwrap_or_else(|| DEFAULT_VOICE_ID.to_string()),
            model,
            format,
            sample_rate,
            channel_type,
            region,
            // Gen2 specific parameters - can be set via builder methods
            multi_native_locale: None,
            rate: None,
            pitch: None,
            style: None,
            variation: None,
            pronunciation_dictionary: None,
            audio_duration: None,
            encode_as_base64: false,
            custom_endpoint: None,
        };

        // Validate the configuration
        murf_config.validate()?;

        Ok(murf_config)
    }

    /// Build from the standardized config (W1 keystone for TTS — first TTS provider migrated).
    /// Wraps `from_base` (the uniform entry point) and maps the [`TtsFeatures`] whose meaning
    /// matches a real Murf field: `speed`→`rate` (-50..50), `pitch`→`pitch` (-50..50),
    /// `emotion`→`style` (Murf's named speaking style, e.g. "Conversational"),
    /// `language`→`multi_native_locale` (cross-language pronunciation), `sample_rate`→`sample_rate`.
    /// The non-standard `region` (data-residency endpoint) and `variation` (Gen2-only 0..=5
    /// dynamic-delivery level) are read from `extras`. Features Murf cannot express (volume,
    /// stability/similarity_boost/style-exaggeration/use_speaker_boost, instructions, ssml,
    /// word_timestamps, streaming, seed) are skipped.
    ///
    /// [`TtsFeatures`]: crate::core::tts::standard::TtsFeatures
    pub fn from_standard(
        std: &crate::core::tts::standard::StandardTTSConfig,
    ) -> TTSResult<Self> {
        let f = &std.features;
        let mut cfg = Self::from_base(&std.base)?;
        if let Some(speed) = f.speed {
            cfg = cfg.with_rate(speed as i32);
        }
        if let Some(pitch) = f.pitch {
            cfg = cfg.with_pitch(pitch as i32);
        }
        if let Some(emotion) = &f.emotion {
            cfg = cfg.with_style(emotion.clone());
        }
        if let Some(language) = &f.language {
            cfg.multi_native_locale = Some(language.clone());
        }
        if let Some(sample_rate) = f.sample_rate {
            cfg.sample_rate = sample_rate;
        }
        if let Some(region) = std
            .extras
            .0
            .get("region")
            .and_then(|v| v.as_str())
            .and_then(MurfRegion::from_str)
        {
            cfg.region = region;
        }
        // `variation` (0..=5 dynamic-delivery level) is a Gen2-only, non-standard Murf knob with no
        // canonical TtsFeatures field, so it rides the open `extras` passthrough. `with_variation`
        // clamps to MAX_VARIATION; `MurfStreamRequest::from_config` emits it on the wire for Gen2.
        if let Some(variation) = std
            .extras
            .0
            .get("variation")
            .and_then(|v| v.as_u64())
        {
            cfg = cfg.with_variation(variation as u8);
        }
        cfg.validate()?;
        Ok(cfg)
    }

    /// Validate the configuration
    pub fn validate(&self) -> TTSResult<()> {
        // API key is required
        if self.api_key.is_empty() {
            return Err(TTSError::InvalidConfiguration(
                "Murf API key is required".to_string(),
            ));
        }

        // Voice ID is required
        if self.voice_id.is_empty() {
            return Err(TTSError::InvalidConfiguration(
                "Voice ID is required".to_string(),
            ));
        }

        // Validate rate if provided
        if let Some(rate) = self.rate
            && (!(MIN_RATE..=MAX_RATE).contains(&rate)) {
                return Err(TTSError::InvalidConfiguration(format!(
                    "Rate must be between {} and {}, got {}",
                    MIN_RATE, MAX_RATE, rate
                )));
            }

        // Validate pitch if provided
        if let Some(pitch) = self.pitch
            && (!(MIN_PITCH..=MAX_PITCH).contains(&pitch)) {
                return Err(TTSError::InvalidConfiguration(format!(
                    "Pitch must be between {} and {}, got {}",
                    MIN_PITCH, MAX_PITCH, pitch
                )));
            }

        // Validate variation if provided
        if let Some(variation) = self.variation
            && variation > MAX_VARIATION {
                return Err(TTSError::InvalidConfiguration(format!(
                    "Variation must be between {} and {}, got {}",
                    MIN_VARIATION, MAX_VARIATION, variation
                )));
            }

        // Validate sample rate for telephony formats
        if self.format.requires_telephony_settings() && self.sample_rate != 8000 {
            return Err(TTSError::InvalidConfiguration(format!(
                "ALAW/ULAW formats require 8000 Hz sample rate, got {}",
                self.sample_rate
            )));
        }

        // Validate sample rate is a supported value
        if !matches!(self.sample_rate, 8000 | 24000 | 44100 | 48000) {
            return Err(TTSError::InvalidConfiguration(format!(
                "Unsupported sample rate: {}. Use 8000, 24000, 44100, or 48000",
                self.sample_rate
            )));
        }

        Ok(())
    }

    /// Get the streaming endpoint URL
    pub fn streaming_url(&self) -> &str {
        self.custom_endpoint
            .as_deref()
            .unwrap_or_else(|| self.region.streaming_url())
    }

    /// Set the model
    pub fn with_model(mut self, model: MurfModel) -> Self {
        self.model = model;
        self
    }

    /// Set the voice ID
    pub fn with_voice(mut self, voice_id: impl Into<String>) -> Self {
        self.voice_id = voice_id.into();
        self
    }

    /// Set the audio format
    pub fn with_format(mut self, format: MurfAudioFormat) -> Self {
        self.format = format;
        // Update sample rate if telephony format
        if format.requires_telephony_settings() {
            self.sample_rate = 8000;
            self.channel_type = MurfChannelType::Mono;
        }
        self
    }

    /// Set the sample rate
    pub fn with_sample_rate(mut self, sample_rate: u32) -> Self {
        self.sample_rate = sample_rate;
        self
    }

    /// Set the region
    pub fn with_region(mut self, region: MurfRegion) -> Self {
        self.region = region;
        self
    }

    /// Set speech rate (Gen2 only)
    pub fn with_rate(mut self, rate: i32) -> Self {
        self.rate = Some(rate.clamp(MIN_RATE, MAX_RATE));
        self
    }

    /// Set pitch (Gen2 only)
    pub fn with_pitch(mut self, pitch: i32) -> Self {
        self.pitch = Some(pitch.clamp(MIN_PITCH, MAX_PITCH));
        self
    }

    /// Set style (Gen2 only)
    pub fn with_style(mut self, style: impl Into<String>) -> Self {
        self.style = Some(style.into());
        self
    }

    /// Set variation (Gen2 only)
    pub fn with_variation(mut self, variation: u8) -> Self {
        self.variation = Some(variation.min(MAX_VARIATION));
        self
    }

    /// Validate voice ID format
    pub fn validate_voice_id(voice_id: &str) -> TTSResult<()> {
        if voice_id.is_empty() {
            return Err(TTSError::InvalidConfiguration(
                "Voice ID cannot be empty".to_string(),
            ));
        }
        // Both formats are valid: "en-US-natalie" or "natalie"
        Ok(())
    }
}

// =============================================================================
// API Request Types
// =============================================================================

/// Murf.ai streaming request body
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MurfStreamRequest {
    /// Text to synthesize
    pub text: String,

    /// Voice identifier
    pub voice_id: String,

    /// TTS model
    pub model: String,

    /// Audio format
    pub format: String,

    /// Sample rate in Hz
    pub sample_rate: u32,

    /// Channel type
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_type: Option<String>,

    /// Multi-native locale (Falcon only)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub multi_native_locale: Option<String>,

    // === Gen2 Only Parameters ===
    /// Rate adjustment (-50 to 50)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate: Option<i32>,

    /// Pitch adjustment (-50 to 50)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pitch: Option<i32>,

    /// Speaking style
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,

    /// Variation level (0-5)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variation: Option<u8>,

    /// Pronunciation dictionary
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pronunciation_dictionary: Option<HashMap<String, String>>,

    /// Target audio duration
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_duration: Option<f32>,

    /// Encode as Base64
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub encode_as_base64: bool,
}

impl MurfStreamRequest {
    /// Create a new streaming request from config and text
    pub fn from_config(config: &MurfTtsConfig, text: impl Into<String>) -> Self {
        let text = text.into();

        // Base request
        let mut request = Self {
            text,
            voice_id: config.voice_id.clone(),
            model: config.model.as_str().to_string(),
            format: config.format.as_str().to_string(),
            sample_rate: config.sample_rate,
            channel_type: Some(config.channel_type.as_str().to_string()),
            multi_native_locale: config.multi_native_locale.clone(),
            rate: None,
            pitch: None,
            style: None,
            variation: None,
            pronunciation_dictionary: None,
            audio_duration: None,
            encode_as_base64: config.encode_as_base64,
        };

        // Add Gen2-only parameters if using Gen2 model
        if config.model == MurfModel::Gen2 {
            request.rate = config.rate;
            request.pitch = config.pitch;
            request.style = config.style.clone();
            request.variation = config.variation;
            request.pronunciation_dictionary = config.pronunciation_dictionary.clone();
            request.audio_duration = config.audio_duration;
        } else if config.rate.is_some() || config.pitch.is_some() || config.style.is_some() {
            // Review S2: rate/pitch/style (standardized speed/pitch/emotion) are Gen2-only on Murf.
            // The default model is Falcon, so these were silently dropped. Surface the capability
            // gap loudly instead of failing silently.
            tracing::warn!(
                model = %config.model.as_str(),
                "Murf rate/pitch/style (speed/pitch/emotion) require the GEN2 model; ignored on the current model"
            );
        }

        request
    }
}

// =============================================================================
// Voice Types
// =============================================================================

/// Murf.ai voice metadata
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MurfVoice {
    /// Voice identifier
    pub voice_id: String,

    /// Display name
    #[serde(default)]
    pub name: String,

    /// Language code (e.g., "en-US")
    #[serde(default)]
    pub language: String,

    /// Voice gender
    #[serde(default)]
    pub gender: Option<String>,

    /// Available speaking styles
    #[serde(default)]
    pub styles: Vec<String>,

    /// Sample audio URL
    #[serde(default)]
    pub sample_url: Option<String>,
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // W1 keystone (first TTS provider): the standardized features map onto Murf's real fields
    // (rate, pitch, style, multi_native_locale, sample_rate) and the `region` extra — previously
    // unreachable via the flat factory.
    #[test]
    fn from_standard_maps_features_and_region_extra() {
        use crate::core::tts::standard::{StandardTTSConfig, TtsFeatures};

        let mut extras = serde_json::Map::new();
        extras.insert("region".into(), serde_json::json!("us-east"));

        let std = StandardTTSConfig {
            base: TTSConfig {
                provider: "murf".into(),
                api_key: "test-key".into(),
                voice_id: Some("en-US-natalie".into()),
                model: "GEN2".into(),
                ..Default::default()
            },
            features: TtsFeatures {
                speed: Some(10.0),
                pitch: Some(-5.0),
                emotion: Some("Conversational".into()),
                language: Some("en-US".into()),
                sample_rate: Some(44100),
                ..Default::default()
            },
            extras: crate::core::stt::standard::ProviderExtras(extras),
        };

        let cfg = MurfTtsConfig::from_standard(&std).unwrap();
        assert_eq!(cfg.rate, Some(10)); // speed -> rate
        assert_eq!(cfg.pitch, Some(-5)); // pitch -> pitch
        assert_eq!(cfg.style, Some("Conversational".to_string())); // emotion -> style
        assert_eq!(cfg.multi_native_locale, Some("en-US".to_string())); // language -> locale
        assert_eq!(cfg.sample_rate, 44100); // sample_rate passthrough
        assert_eq!(cfg.region, MurfRegion::UsEast); // region from extras
        // base carried through from_base
        assert_eq!(cfg.api_key, "test-key");
        assert_eq!(cfg.voice_id, "en-US-natalie");
        assert_eq!(cfg.model, MurfModel::Gen2);
    }

    #[test]
    fn test_config_defaults() {
        let config = MurfTtsConfig::default();
        assert_eq!(config.model, MurfModel::Falcon);
        assert_eq!(config.sample_rate, DEFAULT_SAMPLE_RATE);
        assert_eq!(config.format, MurfAudioFormat::Pcm);
        assert_eq!(config.region, MurfRegion::Global);
        assert_eq!(config.voice_id, DEFAULT_VOICE_ID);
    }

    #[test]
    fn test_model_enum() {
        assert_eq!(MurfModel::Falcon.as_str(), "FALCON");
        assert_eq!(MurfModel::Gen2.as_str(), "GEN2");
        assert_eq!(MurfModel::from_str("falcon"), Some(MurfModel::Falcon));
        assert_eq!(MurfModel::from_str("GEN2"), Some(MurfModel::Gen2));
        assert_eq!(MurfModel::from_str("invalid"), None);
    }

    #[test]
    fn test_format_enum() {
        assert_eq!(MurfAudioFormat::Pcm.as_str(), "PCM");
        assert_eq!(MurfAudioFormat::Mp3.as_str(), "MP3");
        assert!(MurfAudioFormat::Alaw.requires_telephony_settings());
        assert!(!MurfAudioFormat::Pcm.requires_telephony_settings());
        assert_eq!(MurfAudioFormat::Alaw.default_sample_rate(), 8000);
        assert_eq!(MurfAudioFormat::Pcm.default_sample_rate(), 24000);
    }

    #[test]
    fn test_region_endpoints() {
        assert_eq!(
            MurfRegion::Global.streaming_url(),
            "https://global.api.murf.ai/v1/speech/stream"
        );
        assert_eq!(
            MurfRegion::UsEast.streaming_url(),
            "https://us-east.api.murf.ai/v1/speech/stream"
        );
        assert_eq!(MurfRegion::UsEast.default_concurrency(), 15);
        assert_eq!(MurfRegion::India.default_concurrency(), 2);
    }

    #[test]
    fn test_config_validation() {
        // Valid config
        let config = MurfTtsConfig::new("test-api-key");
        assert!(config.validate().is_ok());

        // Missing API key
        let config = MurfTtsConfig::default();
        assert!(config.validate().is_err());

        // Invalid rate
        let mut config = MurfTtsConfig::new("test-api-key");
        config.rate = Some(100);
        assert!(config.validate().is_err());

        // Invalid pitch
        let mut config = MurfTtsConfig::new("test-api-key");
        config.pitch = Some(-100);
        assert!(config.validate().is_err());

        // Invalid sample rate for telephony
        let mut config = MurfTtsConfig::new("test-api-key");
        config.format = MurfAudioFormat::Alaw;
        config.sample_rate = 24000;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_stream_request_serialization() {
        let config = MurfTtsConfig::new("test-key")
            .with_voice("en-US-natalie")
            .with_model(MurfModel::Falcon);

        let request = MurfStreamRequest::from_config(&config, "Hello world");

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"voiceId\":\"en-US-natalie\""));
        assert!(json.contains("\"model\":\"FALCON\""));
        assert!(json.contains("\"text\":\"Hello world\""));
    }

    #[test]
    fn test_gen2_parameters() {
        let config = MurfTtsConfig::new("test-key")
            .with_model(MurfModel::Gen2)
            .with_rate(10)
            .with_pitch(-5)
            .with_style("Conversational")
            .with_variation(3);

        let request = MurfStreamRequest::from_config(&config, "Test");

        assert_eq!(request.rate, Some(10));
        assert_eq!(request.pitch, Some(-5));
        assert_eq!(request.style, Some("Conversational".to_string()));
        assert_eq!(request.variation, Some(3));
    }

    #[test]
    fn test_falcon_excludes_gen2_params() {
        let mut config = MurfTtsConfig::new("test-key").with_model(MurfModel::Falcon);
        config.rate = Some(10);
        config.pitch = Some(-5);

        let request = MurfStreamRequest::from_config(&config, "Test");

        // Falcon should not include Gen2 parameters
        assert!(request.rate.is_none());
        assert!(request.pitch.is_none());
    }

    #[test]
    fn test_region_from_str() {
        assert_eq!(MurfRegion::from_str("global"), Some(MurfRegion::Global));
        assert_eq!(MurfRegion::from_str("us-east"), Some(MurfRegion::UsEast));
        assert_eq!(MurfRegion::from_str("india"), Some(MurfRegion::India));
        assert_eq!(MurfRegion::from_str("mumbai"), Some(MurfRegion::India));
        assert_eq!(MurfRegion::from_str("invalid"), None);
    }

    #[test]
    fn test_channel_type() {
        assert_eq!(MurfChannelType::Mono.as_str(), "MONO");
        assert_eq!(MurfChannelType::Stereo.as_str(), "STEREO");
        assert_eq!(
            MurfChannelType::from_str("mono"),
            Some(MurfChannelType::Mono)
        );
    }

    #[test]
    fn test_voice_id_validation() {
        // Valid formats
        assert!(MurfTtsConfig::validate_voice_id("en-US-natalie").is_ok());
        assert!(MurfTtsConfig::validate_voice_id("natalie").is_ok());

        // Invalid
        assert!(MurfTtsConfig::validate_voice_id("").is_err());
    }

    #[test]
    fn test_config_from_base() {
        // Set env var for test (SAFETY: Test-only, no concurrent access)
        unsafe {
            std::env::set_var("MURF_API_KEY", "test-env-key");
        }

        let base = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("en-US-natalie".to_string()),
            audio_format: Some("mp3".to_string()),
            sample_rate: Some(44100),
            model: "GEN2".to_string(),
            ..Default::default()
        };

        let config = MurfTtsConfig::from_base(&base).unwrap();
        assert_eq!(config.api_key, "test-key");
        assert_eq!(config.voice_id, "en-US-natalie");
        assert_eq!(config.format, MurfAudioFormat::Mp3);
        assert_eq!(config.sample_rate, 44100);
        assert_eq!(config.model, MurfModel::Gen2);
        // Default region is Global (can be set via builder methods)
        assert_eq!(config.region, MurfRegion::Global);

        // Clean up (SAFETY: Test-only, no concurrent access)
        unsafe {
            std::env::remove_var("MURF_API_KEY");
        }
    }

    #[test]
    fn test_config_with_builder_methods() {
        let config = MurfTtsConfig::new("test-key")
            .with_voice("en-US-matthew")
            .with_model(MurfModel::Gen2)
            .with_region(MurfRegion::UsEast)
            .with_rate(10)
            .with_pitch(-5)
            .with_style("Conversational");

        assert_eq!(config.voice_id, "en-US-matthew");
        assert_eq!(config.model, MurfModel::Gen2);
        assert_eq!(config.region, MurfRegion::UsEast);
        assert_eq!(config.rate, Some(10));
        assert_eq!(config.pitch, Some(-5));
        assert_eq!(config.style, Some("Conversational".to_string()));
    }
}
