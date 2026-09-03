//! Rev AI STT Configuration
//!
//! Configuration structures for Rev AI streaming speech-to-text.

use crate::core::stt::{STTConfig, STTError};
use serde::{Deserialize, Serialize};
use url::form_urlencoded;

use super::{
    DEFAULT_CHANNELS, DEFAULT_LANGUAGE, DEFAULT_SAMPLE_RATE, MAX_CHANNELS, MAX_SAMPLE_RATE,
    MIN_CHANNELS, MIN_SAMPLE_RATE, REVAI_STREAM_URL,
};

fn validate_revai_stt_endpoint(source: &str, endpoint: &str) -> Result<(), STTError> {
    let endpoint = endpoint.trim();
    if endpoint.is_empty() {
        return Ok(());
    }

    crate::core::net::validate_url_for_ssrf(endpoint, &["ws", "wss"]).map_err(|e| {
        STTError::ConfigurationError(format!("{source} rejected (SSRF protection): {e}"))
    })
}

// =============================================================================
// Sample Format
// =============================================================================

/// Audio sample format for Rev AI
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum RevAISampleFormat {
    /// Signed 16-bit little-endian (most common)
    #[default]
    #[serde(rename = "S16LE")]
    S16LE,
    /// Signed 32-bit little-endian
    #[serde(rename = "S32LE")]
    S32LE,
    /// 32-bit float little-endian
    #[serde(rename = "F32LE")]
    F32LE,
    /// Signed 16-bit big-endian
    #[serde(rename = "S16BE")]
    S16BE,
    /// Signed 32-bit big-endian
    #[serde(rename = "S32BE")]
    S32BE,
    /// 32-bit float big-endian
    #[serde(rename = "F32BE")]
    F32BE,
    /// Unsigned 8-bit
    #[serde(rename = "U8")]
    U8,
}

impl RevAISampleFormat {
    /// Convert to string for content_type parameter
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::S16LE => "S16LE",
            Self::S32LE => "S32LE",
            Self::F32LE => "F32LE",
            Self::S16BE => "S16BE",
            Self::S32BE => "S32BE",
            Self::F32BE => "F32BE",
            Self::U8 => "U8",
        }
    }

    /// Parse from string
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_uppercase().as_str() {
            "S16LE" | "LINEAR16" | "PCM_S16LE" => Some(Self::S16LE),
            "S32LE" | "LINEAR32" | "PCM_S32LE" => Some(Self::S32LE),
            "F32LE" | "FLOAT32LE" => Some(Self::F32LE),
            "S16BE" | "PCM_S16BE" => Some(Self::S16BE),
            "S32BE" | "PCM_S32BE" => Some(Self::S32BE),
            "F32BE" | "FLOAT32BE" => Some(Self::F32BE),
            "U8" | "PCM_U8" => Some(Self::U8),
            _ => None,
        }
    }

    /// Get bit depth for this format
    pub fn bit_depth(&self) -> u8 {
        match self {
            Self::U8 => 8,
            Self::S16LE | Self::S16BE => 16,
            Self::S32LE | Self::S32BE | Self::F32LE | Self::F32BE => 32,
        }
    }
}

// =============================================================================
// Audio Layout
// =============================================================================

/// Audio channel layout
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RevAIAudioLayout {
    /// Interleaved audio channels (default)
    #[default]
    Interleaved,
    /// Non-interleaved (planar) audio channels
    NonInterleaved,
}

impl RevAIAudioLayout {
    /// Convert to string for content_type parameter
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Interleaved => "interleaved",
            Self::NonInterleaved => "non-interleaved",
        }
    }
}

// =============================================================================
// Transcriber
// =============================================================================

/// Transcriber type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RevAITranscriber {
    /// Default machine transcription
    #[default]
    Machine,
    /// Machine V2 with speaker detection support
    MachineV2,
    /// Human transcription (higher accuracy, higher latency)
    Human,
}

impl RevAITranscriber {
    /// Convert to string for query parameter
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Machine => "machine",
            Self::MachineV2 => "machine_v2",
            Self::Human => "human",
        }
    }
}

// =============================================================================
// Main Configuration
// =============================================================================

/// Rev AI STT Configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevAISTTConfig {
    /// Rev AI API access token
    pub api_key: String,

    /// Language code for transcription
    #[serde(default = "default_language")]
    pub language: String,

    /// Sample rate in Hz (8000-48000)
    #[serde(default = "default_sample_rate")]
    pub sample_rate: u32,

    /// Number of audio channels (1-10)
    #[serde(default = "default_channels")]
    pub channels: u8,

    /// Audio sample format
    #[serde(default)]
    pub sample_format: RevAISampleFormat,

    /// Audio channel layout
    #[serde(default)]
    pub layout: RevAIAudioLayout,

    /// Transcriber type
    #[serde(default)]
    pub transcriber: RevAITranscriber,

    /// Custom metadata string
    #[serde(default)]
    pub metadata: Option<String>,

    /// Custom vocabulary ID
    #[serde(default)]
    pub custom_vocabulary_id: Option<String>,

    /// Filter profanity in output
    #[serde(default)]
    pub filter_profanity: bool,

    /// Remove disfluencies (ums, uhs, etc.)
    #[serde(default)]
    pub remove_disfluencies: bool,

    /// Include detailed timestamps in partial results
    #[serde(default)]
    pub detailed_partials: bool,

    /// Auto-delete transcript after N seconds
    #[serde(default)]
    pub delete_after_seconds: Option<u32>,

    /// Starting timestamp offset in seconds
    #[serde(default)]
    pub start_ts: Option<f64>,

    /// Maximum segment duration in seconds
    #[serde(default)]
    pub max_segment_duration_seconds: Option<u32>,

    /// Enable speaker detection (machine_v2 only)
    #[serde(default)]
    pub enable_speaker_switch: bool,

    /// Skip text post-processing/normalization
    #[serde(default)]
    pub skip_postprocessing: bool,

    /// Optimization mode for the streaming session: `speed` (default) or `accuracy`
    /// (Rev AI streaming query param `priority`; see docs/providers/rev_ai.md). Carried via the
    /// standardized `extras` passthrough since it has no canonical typed feature.
    #[serde(default)]
    pub priority: Option<String>,

    /// Maximum time (seconds, 60-600) Rev AI will hold the connection open waiting for the next
    /// audio chunk before closing (Rev AI streaming query param `max_connection_wait_seconds`;
    /// see docs/providers/rev_ai.md). Carried via the standardized `extras` passthrough.
    #[serde(default)]
    pub max_connection_wait_seconds: Option<u32>,

    /// Carried from the standardized `endpoint_override` — points the dial at the in-repo mock/proxy
    /// (a local `ws://` server) for credential-free end-to-end integration tests; `None` uses the
    /// production Rev AI endpoint.
    #[serde(default)]
    pub endpoint_override: Option<String>,
}

fn default_language() -> String {
    DEFAULT_LANGUAGE.to_string()
}

fn default_sample_rate() -> u32 {
    DEFAULT_SAMPLE_RATE
}

fn default_channels() -> u8 {
    DEFAULT_CHANNELS
}

impl Default for RevAISTTConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            language: DEFAULT_LANGUAGE.to_string(),
            sample_rate: DEFAULT_SAMPLE_RATE,
            channels: DEFAULT_CHANNELS,
            sample_format: RevAISampleFormat::default(),
            layout: RevAIAudioLayout::default(),
            transcriber: RevAITranscriber::default(),
            metadata: None,
            custom_vocabulary_id: None,
            filter_profanity: false,
            remove_disfluencies: false,
            detailed_partials: false,
            delete_after_seconds: None,
            start_ts: None,
            max_segment_duration_seconds: None,
            enable_speaker_switch: false,
            skip_postprocessing: false,
            priority: None,
            max_connection_wait_seconds: None,
            endpoint_override: None,
        }
    }
}

impl RevAISTTConfig {
    /// Create a new configuration with the given API key
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            ..Default::default()
        }
    }

    /// Set the language
    pub fn with_language(mut self, language: impl Into<String>) -> Self {
        self.language = language.into();
        self
    }

    /// Set the sample rate
    pub fn with_sample_rate(mut self, sample_rate: u32) -> Self {
        self.sample_rate = sample_rate;
        self
    }

    /// Set the number of channels
    pub fn with_channels(mut self, channels: u8) -> Self {
        self.channels = channels;
        self
    }

    /// Set the sample format
    pub fn with_sample_format(mut self, format: RevAISampleFormat) -> Self {
        self.sample_format = format;
        self
    }

    /// Set the audio layout
    pub fn with_layout(mut self, layout: RevAIAudioLayout) -> Self {
        self.layout = layout;
        self
    }

    /// Set the transcriber type
    pub fn with_transcriber(mut self, transcriber: RevAITranscriber) -> Self {
        self.transcriber = transcriber;
        self
    }

    /// Set custom metadata
    pub fn with_metadata(mut self, metadata: impl Into<String>) -> Self {
        self.metadata = Some(metadata.into());
        self
    }

    /// Set custom vocabulary ID
    pub fn with_custom_vocabulary(mut self, vocabulary_id: impl Into<String>) -> Self {
        self.custom_vocabulary_id = Some(vocabulary_id.into());
        self
    }

    /// Enable profanity filtering
    pub fn with_filter_profanity(mut self, filter: bool) -> Self {
        self.filter_profanity = filter;
        self
    }

    /// Enable disfluency removal
    pub fn with_remove_disfluencies(mut self, remove: bool) -> Self {
        self.remove_disfluencies = remove;
        self
    }

    /// Enable detailed partials
    pub fn with_detailed_partials(mut self, detailed: bool) -> Self {
        self.detailed_partials = detailed;
        self
    }

    /// Enable speaker detection (requires machine_v2 transcriber)
    pub fn with_speaker_switch(mut self, enabled: bool) -> Self {
        self.enable_speaker_switch = enabled;
        if enabled {
            self.transcriber = RevAITranscriber::MachineV2;
        }
        self
    }

    /// Skip post-processing
    pub fn with_skip_postprocessing(mut self, skip: bool) -> Self {
        self.skip_postprocessing = skip;
        self
    }

    /// Validate the configuration
    pub fn validate(&self) -> Result<(), STTError> {
        if self.api_key.is_empty() {
            return Err(STTError::ConfigurationError(
                "Rev AI API key is required".to_string(),
            ));
        }

        if self.sample_rate < MIN_SAMPLE_RATE || self.sample_rate > MAX_SAMPLE_RATE {
            return Err(STTError::ConfigurationError(format!(
                "Sample rate must be between {} and {} Hz, got {}",
                MIN_SAMPLE_RATE, MAX_SAMPLE_RATE, self.sample_rate
            )));
        }

        if self.channels < MIN_CHANNELS || self.channels > MAX_CHANNELS {
            return Err(STTError::ConfigurationError(format!(
                "Channels must be between {} and {}, got {}",
                MIN_CHANNELS, MAX_CHANNELS, self.channels
            )));
        }

        if self.enable_speaker_switch && self.transcriber != RevAITranscriber::MachineV2 {
            return Err(STTError::ConfigurationError(
                "Speaker detection requires machine_v2 transcriber".to_string(),
            ));
        }

        if let Some(endpoint) = self.endpoint_override.as_deref() {
            validate_revai_stt_endpoint("endpoint_override", endpoint)?;
        }

        Ok(())
    }

    /// Build the content_type parameter value
    pub fn build_content_type(&self) -> String {
        format!(
            "audio/x-raw;layout={};rate={};format={};channels={}",
            self.layout.as_str(),
            self.sample_rate,
            self.sample_format.as_str(),
            self.channels
        )
    }

    /// Build the full WebSocket URL with all query parameters
    pub fn build_websocket_url(&self) -> String {
        // URL encode helper
        let encode =
            |s: &str| -> String { form_urlencoded::byte_serialize(s.as_bytes()).collect() };

        // Base endpoint: honor an `endpoint_override` (the in-repo mock/proxy points this at a local
        // ws:// server) for credential-free integration; otherwise the production Rev AI endpoint.
        // The override carries only `scheme://host[:port]`, so the Rev AI stream path is re-appended
        // (a path-less URL would fail the WS handshake), mirroring the Cartesia override handling.
        let base = match self
            .endpoint_override
            .as_deref()
            .map(str::trim)
            .filter(|o| !o.is_empty())
        {
            Some(o) => format!("{}/speechtotext/v1/stream", o.trim_end_matches('/')),
            None => REVAI_STREAM_URL.to_string(),
        };
        let mut url = format!(
            "{}?access_token={}&content_type={}",
            base,
            encode(&self.api_key),
            encode(&self.build_content_type())
        );

        // Add language
        url.push_str(&format!("&language={}", encode(&self.language)));

        // Add optional parameters
        if let Some(ref metadata) = self.metadata {
            url.push_str(&format!("&metadata={}", encode(metadata)));
        }

        if let Some(ref vocab_id) = self.custom_vocabulary_id {
            url.push_str(&format!("&custom_vocabulary_id={}", encode(vocab_id)));
        }

        if self.filter_profanity {
            url.push_str("&filter_profanity=true");
        }

        if self.remove_disfluencies {
            url.push_str("&remove_disfluencies=true");
        }

        if self.detailed_partials {
            url.push_str("&detailed_partials=true");
        }

        if let Some(seconds) = self.delete_after_seconds {
            url.push_str(&format!("&delete_after_seconds={}", seconds));
        }

        if let Some(start) = self.start_ts {
            url.push_str(&format!("&start_ts={}", start));
        }

        if let Some(max_duration) = self.max_segment_duration_seconds {
            url.push_str(&format!("&max_segment_duration_seconds={}", max_duration));
        }

        if self.transcriber != RevAITranscriber::Machine {
            url.push_str(&format!("&transcriber={}", self.transcriber.as_str()));
        }

        if self.enable_speaker_switch {
            url.push_str("&enable_speaker_switch=true");
        }

        if self.skip_postprocessing {
            url.push_str("&skip_postprocessing=true");
        }

        // Provider-extras passthrough params (Rev AI streaming query params; docs/providers/rev_ai.md).
        if let Some(ref priority) = self.priority {
            url.push_str(&format!("&priority={}", encode(priority)));
        }

        if let Some(wait) = self.max_connection_wait_seconds {
            url.push_str(&format!("&max_connection_wait_seconds={}", wait));
        }

        url
    }

    /// Create from base STTConfig
    pub fn from_base(config: &STTConfig) -> Result<Self, STTError> {
        let sample_format =
            RevAISampleFormat::from_str(&config.encoding).unwrap_or(RevAISampleFormat::S16LE);

        // Honor an explicitly configured model (Rev AI calls it the "transcriber":
        // machine / machine_v2 / human); otherwise use the default. Previously `config.model` was
        // dropped, so the configured transcriber was silently ignored.
        let transcriber = match config.model.as_str() {
            "machine" => RevAITranscriber::Machine,
            "machine_v2" => RevAITranscriber::MachineV2,
            "human" => RevAITranscriber::Human,
            _ => RevAITranscriber::default(),
        };

        let revai_config = Self {
            api_key: config.api_key.clone(),
            language: config.language.clone(),
            sample_rate: config.sample_rate,
            channels: config.channels as u8,
            sample_format,
            transcriber,
            ..Default::default()
        };

        revai_config.validate()?;
        Ok(revai_config)
    }

    /// Build from the standardized config (W1 keystone). Rev AI exposes a narrow boolean surface,
    /// so this maps the features it can actually express: diarization (`enable_speaker_switch`,
    /// which requires the `machine_v2` transcriber), profanity filtering (`filter_profanity`), and
    /// filler words (`remove_disfluencies`, whose sense is inverted — keeping fillers means *not*
    /// removing disfluencies). Features Rev AI can't express (word_timestamps, smart_format,
    /// interim_results, vad/endpointing, keyterms, redaction, entity/language detection) are
    /// capability gaps and stay at their defaults.
    pub fn from_standard(
        std: &crate::core::stt::standard::StandardSTTConfig,
    ) -> Result<Self, STTError> {
        let f = &std.features;
        let mut cfg = Self::from_base(&std.base)?;
        if let Some(d) = f.diarization {
            cfg.enable_speaker_switch = d;
            if d {
                cfg.transcriber = RevAITranscriber::MachineV2;
            }
        }
        if let Some(p) = f.profanity_filter {
            cfg.filter_profanity = p;
        }
        if let Some(filler) = f.filler_words {
            cfg.remove_disfluencies = !filler;
        }
        // Provider extras → Rev AI streaming query params not modeled by the typed vocabulary
        // (docs/providers/rev_ai.md): `priority` (speed/accuracy) and `max_connection_wait_seconds`
        // (60-600). String keys are forwarded verbatim onto the WebSocket URL.
        let e = &std.extras.0;
        if let Some(p) = e.get("priority").and_then(|v| v.as_str()) {
            cfg.priority = Some(p.to_string());
        }
        if let Some(w) = e
            .get("max_connection_wait_seconds")
            .and_then(|v| v.as_u64())
        {
            cfg.max_connection_wait_seconds = Some(w as u32);
        }
        // Standardized endpoint override (mock/proxy host) for credential-free integration tests.
        cfg.endpoint_override = std.endpoint_override().map(|s| s.to_string());
        Ok(cfg)
    }
}

// =============================================================================
// Conversion to base STTConfig
// =============================================================================

impl From<RevAISTTConfig> for STTConfig {
    fn from(config: RevAISTTConfig) -> Self {
        STTConfig {
            provider: "revai".to_string(),
            api_key: config.api_key,
            language: config.language,
            sample_rate: config.sample_rate,
            channels: config.channels as u16,
            punctuation: true,
            encoding: config.sample_format.as_str().to_string(),
            model: config.transcriber.as_str().to_string(),
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // W1 keystone: maps the standardized features Rev AI can express (diarization +
    // profanity filter) onto its own config fields.
    #[test]
    fn from_standard_maps_features() {
        use crate::core::stt::standard::{ProviderExtras, StandardSTTConfig, SttFeatures};
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "revai".into(),
                api_key: "test-key".into(),
                ..Default::default()
            },
            features: SttFeatures {
                diarization: Some(true),
                profanity_filter: Some(true),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
            translation: None,
        };
        let cfg = RevAISTTConfig::from_standard(&std).unwrap();
        assert!(cfg.enable_speaker_switch); // diarization
        assert_eq!(cfg.transcriber, RevAITranscriber::MachineV2); // required by speaker switch
        assert!(cfg.filter_profanity); // profanity_filter
    }

    // WIRE-LEVEL: the `priority` and `max_connection_wait_seconds` extras must travel from the
    // standardized `extras` passthrough all the way onto the streaming WebSocket URL — not merely
    // land on the config struct. Asserts the actual query string `build_websocket_url` produces
    // (the bytes that hit the wire), guarding the recurring "set on the struct but never emitted"
    // gap class.
    #[test]
    fn priority_and_max_connection_wait_reach_websocket_url() {
        use crate::core::stt::standard::{ProviderExtras, StandardSTTConfig};
        let mut extras = serde_json::Map::new();
        extras.insert("priority".into(), serde_json::json!("accuracy"));
        extras.insert("max_connection_wait_seconds".into(), serde_json::json!(120));
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "revai".into(),
                api_key: "test-key".into(),
                ..Default::default()
            },
            features: Default::default(),
            extras: ProviderExtras(extras),
            translation: None,
        };
        let cfg = RevAISTTConfig::from_standard(&std).unwrap();
        // mapped onto the config struct...
        assert_eq!(cfg.priority.as_deref(), Some("accuracy"));
        assert_eq!(cfg.max_connection_wait_seconds, Some(120));
        // ...AND emitted on the wire (the URL the WebSocket actually connects to).
        let url = cfg.build_websocket_url();
        assert!(
            url.contains("priority=accuracy"),
            "priority must reach the WS URL; got: {url}"
        );
        assert!(
            url.contains("max_connection_wait_seconds=120"),
            "max_connection_wait_seconds must reach the WS URL; got: {url}"
        );
    }

    #[test]
    fn test_sample_format_as_str() {
        assert_eq!(RevAISampleFormat::S16LE.as_str(), "S16LE");
        assert_eq!(RevAISampleFormat::S32LE.as_str(), "S32LE");
        assert_eq!(RevAISampleFormat::F32LE.as_str(), "F32LE");
        assert_eq!(RevAISampleFormat::S16BE.as_str(), "S16BE");
        assert_eq!(RevAISampleFormat::U8.as_str(), "U8");
    }

    #[test]
    fn test_sample_format_from_str() {
        assert_eq!(
            RevAISampleFormat::from_str("S16LE"),
            Some(RevAISampleFormat::S16LE)
        );
        assert_eq!(
            RevAISampleFormat::from_str("linear16"),
            Some(RevAISampleFormat::S16LE)
        );
        assert_eq!(
            RevAISampleFormat::from_str("F32LE"),
            Some(RevAISampleFormat::F32LE)
        );
        assert_eq!(RevAISampleFormat::from_str("invalid"), None);
    }

    #[test]
    fn test_sample_format_bit_depth() {
        assert_eq!(RevAISampleFormat::U8.bit_depth(), 8);
        assert_eq!(RevAISampleFormat::S16LE.bit_depth(), 16);
        assert_eq!(RevAISampleFormat::S32LE.bit_depth(), 32);
        assert_eq!(RevAISampleFormat::F32LE.bit_depth(), 32);
    }

    #[test]
    fn test_audio_layout_as_str() {
        assert_eq!(RevAIAudioLayout::Interleaved.as_str(), "interleaved");
        assert_eq!(RevAIAudioLayout::NonInterleaved.as_str(), "non-interleaved");
    }

    #[test]
    fn test_transcriber_as_str() {
        assert_eq!(RevAITranscriber::Machine.as_str(), "machine");
        assert_eq!(RevAITranscriber::MachineV2.as_str(), "machine_v2");
        assert_eq!(RevAITranscriber::Human.as_str(), "human");
    }

    #[test]
    fn test_from_base_honors_transcriber_model() {
        // The configured model selects Rev AI's transcriber; previously `config.model` was dropped.
        let base = STTConfig {
            api_key: "k".to_string(),
            model: "machine_v2".to_string(),
            ..Default::default()
        };
        let cfg = RevAISTTConfig::from_base(&base).unwrap();
        assert_eq!(cfg.transcriber, RevAITranscriber::MachineV2);

        // An unrecognized model falls back to the default transcriber rather than erroring.
        let base2 = STTConfig {
            api_key: "k".to_string(),
            model: "nova-3".to_string(),
            ..Default::default()
        };
        let cfg2 = RevAISTTConfig::from_base(&base2).unwrap();
        assert_eq!(cfg2.transcriber, RevAITranscriber::default());
    }

    #[test]
    fn test_config_default() {
        let config = RevAISTTConfig::default();
        assert!(config.api_key.is_empty());
        assert_eq!(config.language, "en");
        assert_eq!(config.sample_rate, 16000);
        assert_eq!(config.channels, 1);
        assert_eq!(config.sample_format, RevAISampleFormat::S16LE);
        assert_eq!(config.layout, RevAIAudioLayout::Interleaved);
        assert_eq!(config.transcriber, RevAITranscriber::Machine);
        assert!(!config.filter_profanity);
        assert!(!config.remove_disfluencies);
    }

    #[test]
    fn test_config_builder() {
        let config = RevAISTTConfig::new("test-key")
            .with_language("es")
            .with_sample_rate(44100)
            .with_channels(2)
            .with_filter_profanity(true)
            .with_remove_disfluencies(true)
            .with_detailed_partials(true);

        assert_eq!(config.api_key, "test-key");
        assert_eq!(config.language, "es");
        assert_eq!(config.sample_rate, 44100);
        assert_eq!(config.channels, 2);
        assert!(config.filter_profanity);
        assert!(config.remove_disfluencies);
        assert!(config.detailed_partials);
    }

    #[test]
    fn test_config_speaker_switch_sets_transcriber() {
        let config = RevAISTTConfig::new("test-key").with_speaker_switch(true);

        assert!(config.enable_speaker_switch);
        assert_eq!(config.transcriber, RevAITranscriber::MachineV2);
    }

    #[test]
    fn test_config_validation_empty_api_key() {
        let config = RevAISTTConfig::default();
        let result = config.validate();
        assert!(result.is_err());
        if let Err(STTError::ConfigurationError(msg)) = result {
            assert!(msg.contains("API key"));
        }
    }

    #[test]
    fn test_config_validation_invalid_sample_rate() {
        let config = RevAISTTConfig::new("test-key").with_sample_rate(1000);
        let result = config.validate();
        assert!(result.is_err());
        if let Err(STTError::ConfigurationError(msg)) = result {
            assert!(msg.contains("Sample rate"));
        }
    }

    #[test]
    fn test_config_validation_invalid_channels() {
        let config = RevAISTTConfig::new("test-key").with_channels(20);
        let result = config.validate();
        assert!(result.is_err());
        if let Err(STTError::ConfigurationError(msg)) = result {
            assert!(msg.contains("Channels"));
        }
    }

    #[test]
    fn test_config_validation_speaker_switch_wrong_transcriber() {
        let mut config = RevAISTTConfig::new("test-key");
        config.enable_speaker_switch = true;
        config.transcriber = RevAITranscriber::Machine;

        let result = config.validate();
        assert!(result.is_err());
        if let Err(STTError::ConfigurationError(msg)) = result {
            assert!(msg.contains("machine_v2"));
        }
    }

    #[test]
    fn test_config_validation_success() {
        let config = RevAISTTConfig::new("test-key");
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validation_rejects_ssrf_endpoint_override() {
        let _guard = crate::core::net::test_env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let previous = std::env::var_os("WAAV_ALLOW_LOOPBACK_ENDPOINTS");
        // SAFETY: test-only env mutation, serialized by core::net::test_env_lock.
        unsafe { std::env::remove_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS") };

        let mut config = RevAISTTConfig {
            endpoint_override: Some("wss://revai-proxy.example.com".to_string()),
            ..RevAISTTConfig::new("test-key")
        };
        assert!(config.validate().is_ok());

        config.endpoint_override = Some("ws://revai-proxy.example.com".to_string());
        assert!(config.validate().is_ok());

        config.endpoint_override = Some("ws://127.0.0.1:9000".to_string());
        let err = config
            .validate()
            .expect_err("loopback endpoint_override must be rejected");
        assert!(err.to_string().contains("SSRF protection"), "{err}");

        config.endpoint_override = Some("file:///tmp/socket".to_string());
        let err = config
            .validate()
            .expect_err("non-WebSocket endpoint_override must be rejected");
        assert!(err.to_string().contains("not allowed"), "{err}");

        config.endpoint_override = Some("https://revai-proxy.example.com".to_string());
        let err = config
            .validate()
            .expect_err("HTTP endpoint_override must be rejected for Rev AI WebSocket dial");
        assert!(err.to_string().contains("not allowed"), "{err}");

        // SAFETY: restore the process env before releasing the test env lock.
        unsafe {
            if let Some(previous) = previous {
                std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", previous);
            } else {
                std::env::remove_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS");
            }
        }
    }

    #[test]
    fn test_build_content_type() {
        let config = RevAISTTConfig::new("test-key")
            .with_sample_rate(16000)
            .with_channels(1)
            .with_sample_format(RevAISampleFormat::S16LE);

        let content_type = config.build_content_type();
        assert_eq!(
            content_type,
            "audio/x-raw;layout=interleaved;rate=16000;format=S16LE;channels=1"
        );
    }

    #[test]
    fn test_build_websocket_url_basic() {
        let config = RevAISTTConfig::new("test-api-key").with_language("en");

        let url = config.build_websocket_url();
        assert!(url.starts_with("wss://api.rev.ai/speechtotext/v1/stream"));
        assert!(url.contains("access_token=test-api-key"));
        assert!(url.contains("content_type="));
        assert!(url.contains("language=en"));
    }

    #[test]
    fn test_build_websocket_url_with_options() {
        let config = RevAISTTConfig::new("test-key")
            .with_language("es")
            .with_filter_profanity(true)
            .with_remove_disfluencies(true)
            .with_detailed_partials(true)
            .with_speaker_switch(true);

        let url = config.build_websocket_url();
        assert!(url.contains("language=es"));
        assert!(url.contains("filter_profanity=true"));
        assert!(url.contains("remove_disfluencies=true"));
        assert!(url.contains("detailed_partials=true"));
        assert!(url.contains("enable_speaker_switch=true"));
        assert!(url.contains("transcriber=machine_v2"));
    }

    #[test]
    fn test_build_websocket_url_with_custom_vocabulary() {
        let config = RevAISTTConfig::new("test-key").with_custom_vocabulary("vocab-123");

        let url = config.build_websocket_url();
        assert!(url.contains("custom_vocabulary_id=vocab-123"));
    }

    #[test]
    fn test_build_websocket_url_trims_endpoint_override() {
        let config = RevAISTTConfig {
            endpoint_override: Some(" wss://revai-proxy.example.com/ ".to_string()),
            ..RevAISTTConfig::new("test-key")
        };

        let url = config.build_websocket_url();
        assert!(url.starts_with("wss://revai-proxy.example.com/speechtotext/v1/stream?"));
        assert!(
            !url.contains("example.com//speechtotext"),
            "endpoint_override slash should be normalized: {url}"
        );
    }

    #[test]
    fn test_from_base_config() {
        let base_config = STTConfig {
            provider: "revai".to_string(),
            api_key: "test-key".to_string(),
            language: "es".to_string(),
            sample_rate: 44100,
            channels: 2,
            punctuation: true,
            encoding: "S16LE".to_string(),
            model: "machine".to_string(),
        };

        let revai_config = RevAISTTConfig::from_base(&base_config).unwrap();
        assert_eq!(revai_config.api_key, "test-key");
        assert_eq!(revai_config.language, "es");
        assert_eq!(revai_config.sample_rate, 44100);
        assert_eq!(revai_config.channels, 2);
        assert_eq!(revai_config.sample_format, RevAISampleFormat::S16LE);
    }

    #[test]
    fn test_config_to_stt_config() {
        let revai_config = RevAISTTConfig::new("test-key")
            .with_language("fr")
            .with_sample_rate(48000)
            .with_channels(2)
            .with_transcriber(RevAITranscriber::MachineV2);

        let stt_config: STTConfig = revai_config.into();
        assert_eq!(stt_config.provider, "revai");
        assert_eq!(stt_config.api_key, "test-key");
        assert_eq!(stt_config.language, "fr");
        assert_eq!(stt_config.sample_rate, 48000);
        assert_eq!(stt_config.channels, 2);
        assert_eq!(stt_config.encoding, "S16LE");
        assert_eq!(stt_config.model, "machine_v2");
    }
}
