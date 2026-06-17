//! Reverie STT Configuration
//!
//! Configuration types for the Reverie Speech-to-Text streaming API.

use serde::{Deserialize, Serialize};
use std::fmt;

use crate::core::stt::STTConfig;

use super::{
    DEFAULT_DOMAIN, DEFAULT_SILENCE, DEFAULT_TIMEOUT, MAX_SILENCE, MAX_TIMEOUT, REVERIE_STREAM_URL,
    STT_STREAM_APPNAME,
};

// =============================================================================
// Language Enum
// =============================================================================

/// Supported languages for Reverie STT
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ReverieLanguage {
    /// Hindi
    #[default]
    Hindi,
    /// English
    English,
    /// Tamil
    Tamil,
    /// Telugu
    Telugu,
    /// Bengali
    Bengali,
    /// Marathi
    Marathi,
    /// Gujarati
    Gujarati,
    /// Kannada
    Kannada,
    /// Malayalam
    Malayalam,
    /// Punjabi
    Punjabi,
    /// Odia (Oriya)
    Odia,
    /// Assamese
    Assamese,
    /// Urdu
    Urdu,
    /// Kashmiri
    Kashmiri,
    /// Sindhi
    Sindhi,
    /// Nepali
    Nepali,
    /// Sanskrit
    Sanskrit,
    /// Konkani
    Konkani,
    /// Manipuri
    Manipuri,
    /// Bodo
    Bodo,
    /// Santhali
    Santhali,
    /// Maithili
    Maithili,
    /// Dogri
    Dogri,
}

impl ReverieLanguage {
    /// Get the language code for API requests
    pub fn as_code(&self) -> &'static str {
        match self {
            Self::Hindi => "hi",
            Self::English => "en",
            Self::Tamil => "ta",
            Self::Telugu => "te",
            Self::Bengali => "bn",
            Self::Marathi => "mr",
            Self::Gujarati => "gu",
            Self::Kannada => "kn",
            Self::Malayalam => "ml",
            Self::Punjabi => "pa",
            Self::Odia => "or",
            Self::Assamese => "as",
            Self::Urdu => "ur",
            Self::Kashmiri => "ks",
            Self::Sindhi => "sd",
            Self::Nepali => "ne",
            Self::Sanskrit => "sa",
            Self::Konkani => "kok",
            Self::Manipuri => "mni",
            Self::Bodo => "brx",
            Self::Santhali => "sat",
            Self::Maithili => "mai",
            Self::Dogri => "doi",
        }
    }

    /// Create language from code string
    pub fn from_code(code: &str) -> Option<Self> {
        match code.to_lowercase().as_str() {
            "hi" | "hindi" | "hi-in" => Some(Self::Hindi),
            "en" | "english" | "en-in" | "en-us" | "en-gb" => Some(Self::English),
            "ta" | "tamil" | "ta-in" => Some(Self::Tamil),
            "te" | "telugu" | "te-in" => Some(Self::Telugu),
            "bn" | "bengali" | "bn-in" => Some(Self::Bengali),
            "mr" | "marathi" | "mr-in" => Some(Self::Marathi),
            "gu" | "gujarati" | "gu-in" => Some(Self::Gujarati),
            "kn" | "kannada" | "kn-in" => Some(Self::Kannada),
            "ml" | "malayalam" | "ml-in" => Some(Self::Malayalam),
            "pa" | "punjabi" | "pa-in" => Some(Self::Punjabi),
            "or" | "odia" | "oriya" | "or-in" => Some(Self::Odia),
            "as" | "assamese" | "as-in" => Some(Self::Assamese),
            "ur" | "urdu" | "ur-in" => Some(Self::Urdu),
            "ks" | "kashmiri" | "ks-in" => Some(Self::Kashmiri),
            "sd" | "sindhi" | "sd-in" => Some(Self::Sindhi),
            "ne" | "nepali" | "ne-np" => Some(Self::Nepali),
            "sa" | "sanskrit" | "sa-in" => Some(Self::Sanskrit),
            "kok" | "konkani" | "kok-in" => Some(Self::Konkani),
            "mni" | "manipuri" | "mni-in" => Some(Self::Manipuri),
            "brx" | "bodo" | "brx-in" => Some(Self::Bodo),
            "sat" | "santhali" | "sat-in" => Some(Self::Santhali),
            "mai" | "maithili" | "mai-in" => Some(Self::Maithili),
            "doi" | "dogri" | "doi-in" => Some(Self::Dogri),
            _ => None,
        }
    }

    /// Check if punctuation is supported for this language
    pub fn supports_punctuation(&self) -> bool {
        matches!(self, Self::Hindi | Self::English)
    }
}

impl fmt::Display for ReverieLanguage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_code())
    }
}

// =============================================================================
// Audio Format Enum
// =============================================================================

/// Supported audio formats for Reverie STT
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum ReverieAudioFormat {
    /// Signed 16-bit PCM, 16kHz (default)
    #[default]
    Pcm16kInt16,
    /// Unsigned 8-bit PCM, 16kHz
    Pcm16kUint8,
    /// Signed 16-bit PCM, 8kHz
    Pcm8kInt16,
    /// Unsigned 8-bit PCM, 8kHz
    Pcm8kUint8,
    /// Opus encoded, 16kHz
    Opus16k,
    /// Opus encoded, 8kHz
    Opus8k,
    /// Opus in Ogg container
    OggOpus,
    /// u-Law encoded, 16kHz
    Ulaw16k,
    /// u-Law encoded, 8kHz
    Ulaw8k,
}

impl ReverieAudioFormat {
    /// Get the format string for API requests
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pcm16kInt16 => "16k_int16",
            Self::Pcm16kUint8 => "16k_uint8",
            Self::Pcm8kInt16 => "8k_int16",
            Self::Pcm8kUint8 => "8k_uint8",
            Self::Opus16k => "opus_16k",
            Self::Opus8k => "opus_8k",
            Self::OggOpus => "ogg_opus",
            Self::Ulaw16k => "16k_ulaw",
            Self::Ulaw8k => "8k_ulaw",
        }
    }

    /// Get the sample rate for this format
    pub fn sample_rate(&self) -> u32 {
        match self {
            Self::Pcm16kInt16
            | Self::Pcm16kUint8
            | Self::Opus16k
            | Self::Ulaw16k
            | Self::OggOpus => 16000,
            Self::Pcm8kInt16 | Self::Pcm8kUint8 | Self::Opus8k | Self::Ulaw8k => 8000,
        }
    }

    /// Create format from string
    pub fn from_str_value(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "16k_int16" | "linear16" | "pcm" | "pcm_s16le" => Some(Self::Pcm16kInt16),
            "16k_uint8" => Some(Self::Pcm16kUint8),
            "8k_int16" => Some(Self::Pcm8kInt16),
            "8k_uint8" => Some(Self::Pcm8kUint8),
            "opus_16k" | "opus" => Some(Self::Opus16k),
            "opus_8k" => Some(Self::Opus8k),
            "ogg_opus" | "ogg" => Some(Self::OggOpus),
            "16k_ulaw" | "mulaw" | "ulaw" => Some(Self::Ulaw16k),
            "8k_ulaw" => Some(Self::Ulaw8k),
            _ => None,
        }
    }
}

impl fmt::Display for ReverieAudioFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =============================================================================
// Logging Mode Enum
// =============================================================================

/// Logging mode for Reverie STT
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReverieLogging {
    /// Store audio and keep transcripts in logs (default)
    #[default]
    True,
    /// Don't store audio but keep transcripts in logs
    NoAudio,
    /// Don't keep transcripts in logs but store audio
    NoTranscript,
    /// Don't store audio or transcripts
    False,
}

impl ReverieLogging {
    /// Get the string value for API requests
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::True => "true",
            Self::NoAudio => "no_audio",
            Self::NoTranscript => "no_transcript",
            Self::False => "false",
        }
    }
}

impl fmt::Display for ReverieLogging {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =============================================================================
// Configuration
// =============================================================================

/// Configuration for Reverie STT
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReverieSTTConfig {
    /// API key for authentication
    pub api_key: String,

    /// Application ID for authentication
    pub app_id: String,

    /// Source language
    #[serde(default)]
    pub language: ReverieLanguage,

    /// Domain for vocabulary optimization (e.g., "generic", "banking", "insurance")
    #[serde(default = "default_domain")]
    pub domain: String,

    /// Audio format
    #[serde(default)]
    pub format: ReverieAudioFormat,

    /// Connection timeout in seconds (1-180)
    #[serde(default = "default_timeout")]
    pub timeout: u32,

    /// Silence detection timeout in seconds (1-30)
    #[serde(default = "default_silence")]
    pub silence: u32,

    /// Logging mode
    #[serde(default)]
    pub logging: ReverieLogging,

    /// Enable punctuation and capitalization (en, hi only)
    #[serde(default = "default_true")]
    pub punctuate: bool,

    /// Continue decoding after silence detection
    #[serde(default)]
    pub continuous: bool,

    /// Sample rate override (derived from format if not set)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,

    /// Extra query parameters
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra_params: Vec<(String, String)>,

    /// Carried from the standardized `endpoint_override` — points the dial at the in-repo mock/proxy
    /// (a local `ws://` server) for credential-free end-to-end integration tests; `None` uses the
    /// production Reverie endpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint_override: Option<String>,
}

fn default_domain() -> String {
    DEFAULT_DOMAIN.to_string()
}

fn default_timeout() -> u32 {
    DEFAULT_TIMEOUT
}

fn default_silence() -> u32 {
    DEFAULT_SILENCE
}

fn default_true() -> bool {
    true
}

/// URL encode a string for query parameters
fn url_encode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

impl ReverieSTTConfig {
    /// Create a new configuration with required credentials
    pub fn new(api_key: impl Into<String>, app_id: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            app_id: app_id.into(),
            language: ReverieLanguage::default(),
            domain: DEFAULT_DOMAIN.to_string(),
            format: ReverieAudioFormat::default(),
            timeout: DEFAULT_TIMEOUT,
            silence: DEFAULT_SILENCE,
            logging: ReverieLogging::default(),
            punctuate: true,
            continuous: false,
            sample_rate: None,
            extra_params: Vec::new(),
            endpoint_override: None,
        }
    }

    /// Set the language
    pub fn with_language(mut self, language: ReverieLanguage) -> Self {
        self.language = language;
        // Disable punctuation for unsupported languages
        if !language.supports_punctuation() {
            self.punctuate = false;
        }
        self
    }

    /// Set the domain
    pub fn with_domain(mut self, domain: impl Into<String>) -> Self {
        self.domain = domain.into();
        self
    }

    /// Set the audio format
    pub fn with_format(mut self, format: ReverieAudioFormat) -> Self {
        self.format = format;
        self
    }

    /// Set the connection timeout
    pub fn with_timeout(mut self, timeout: u32) -> Self {
        self.timeout = timeout.clamp(1, MAX_TIMEOUT);
        self
    }

    /// Set the silence detection timeout
    pub fn with_silence(mut self, silence: u32) -> Self {
        self.silence = silence.clamp(1, MAX_SILENCE);
        self
    }

    /// Set the logging mode
    pub fn with_logging(mut self, logging: ReverieLogging) -> Self {
        self.logging = logging;
        self
    }

    /// Enable/disable punctuation
    pub fn with_punctuate(mut self, punctuate: bool) -> Self {
        // Only enable if language supports it
        self.punctuate = punctuate && self.language.supports_punctuation();
        self
    }

    /// Enable/disable continuous mode
    pub fn with_continuous(mut self, continuous: bool) -> Self {
        self.continuous = continuous;
        self
    }

    /// Add extra query parameter
    pub fn with_extra_param(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra_params.push((key.into(), value.into()));
        self
    }

    /// Get the effective sample rate
    pub fn effective_sample_rate(&self) -> u32 {
        self.sample_rate
            .unwrap_or_else(|| self.format.sample_rate())
    }

    /// Build the WebSocket URL with query parameters
    pub fn build_websocket_url(&self) -> String {
        let mut params = vec![
            ("appname", STT_STREAM_APPNAME.to_string()),
            ("apikey", self.api_key.clone()),
            ("appid", self.app_id.clone()),
            ("src_lang", self.language.as_code().to_string()),
            ("domain", self.domain.clone()),
            ("format", self.format.as_str().to_string()),
            ("timeout", self.timeout.to_string()),
            ("silence", self.silence.to_string()),
            ("logging", self.logging.as_str().to_string()),
            ("punctuate", self.punctuate.to_string()),
            (
                "continuous",
                if self.continuous { "1" } else { "0" }.to_string(),
            ),
        ];

        // Add extra params
        for (key, value) in &self.extra_params {
            params.push((key.as_str(), value.clone()));
        }

        let query = params
            .iter()
            .map(|(k, v)| format!("{}={}", k, url_encode(v)))
            .collect::<Vec<_>>()
            .join("&");

        // Base endpoint: honor an `endpoint_override` (scheme://host[:port]) for the in-repo
        // mock/proxy; the Reverie stream path is re-appended (a path-less URL fails the WS handshake).
        let base = match self.endpoint_override.as_deref().filter(|o| !o.is_empty()) {
            Some(o) => format!("{}/stream", o.trim_end_matches('/')),
            None => REVERIE_STREAM_URL.to_string(),
        };
        format!("{}?{}", base, query)
    }

    /// Validate the configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.api_key.is_empty() {
            return Err("API key cannot be empty".to_string());
        }
        if self.app_id.is_empty() {
            return Err("App ID cannot be empty".to_string());
        }
        if self.timeout > MAX_TIMEOUT {
            return Err(format!("Timeout cannot exceed {} seconds", MAX_TIMEOUT));
        }
        if self.silence > MAX_SILENCE {
            return Err(format!("Silence cannot exceed {} seconds", MAX_SILENCE));
        }
        if self.punctuate && !self.language.supports_punctuation() {
            return Err(format!(
                "Punctuation not supported for language: {}",
                self.language
            ));
        }
        Ok(())
    }

    /// Create configuration from base STTConfig
    ///
    /// The `model` field is used to pass the app_id (required for Reverie).
    /// Alternatively, set REVERIE_APP_ID environment variable.
    pub fn from_base(config: &STTConfig) -> Result<Self, String> {
        if config.api_key.is_empty() {
            return Err("API key is required".to_string());
        }

        // App ID comes from model field or environment variable
        let app_id = if !config.model.is_empty() {
            config.model.clone()
        } else {
            std::env::var("REVERIE_APP_ID")
                .map_err(|_| "App ID is required: set via model field or REVERIE_APP_ID env var")?
        };

        // Parse language from config
        let language = if !config.language.is_empty() {
            ReverieLanguage::from_code(&config.language).unwrap_or_default()
        } else {
            ReverieLanguage::default()
        };

        // Parse audio format from encoding
        let format = if !config.encoding.is_empty() {
            ReverieAudioFormat::from_str_value(&config.encoding).unwrap_or_default()
        } else {
            ReverieAudioFormat::default()
        };

        // Use punctuation setting from config
        let punctuate = config.punctuation && language.supports_punctuation();

        let mut cfg = Self::new(config.api_key.clone(), app_id)
            .with_language(language)
            .with_format(format)
            .with_punctuate(punctuate);

        // Set sample rate if provided
        if config.sample_rate > 0 {
            cfg.sample_rate = Some(config.sample_rate);
        }

        cfg.validate()?;
        Ok(cfg)
    }

    /// Build from the standardized config (W1 keystone). Reverie is a streaming Indian-language
    /// ASR provider whose query-parameter surface exposes no advanced-feature knobs — none of the
    /// standardized features (interim_results, diarization, word_timestamps, smart_format,
    /// profanity_filter, filler_words, vad_events, endpointing, utterance_end, keyterms, redaction,
    /// entity_detection, language_detection) maps to a real field on this config. So this is a pure
    /// passthrough: a uniform standardized entry point that simply delegates to `from_base`.
    pub fn from_standard(
        std: &crate::core::stt::standard::StandardSTTConfig,
    ) -> Result<Self, String> {
        let mut cfg = Self::from_base(&std.base)?;
        // Standardized endpoint override (mock/proxy host) for credential-free integration tests.
        cfg.endpoint_override = std.endpoint_override().map(|s| s.to_string());
        Ok(cfg)
    }
}

impl Default for ReverieSTTConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            app_id: String::new(),
            language: ReverieLanguage::default(),
            domain: DEFAULT_DOMAIN.to_string(),
            format: ReverieAudioFormat::default(),
            timeout: DEFAULT_TIMEOUT,
            silence: DEFAULT_SILENCE,
            logging: ReverieLogging::default(),
            punctuate: true,
            continuous: false,
            sample_rate: None,
            extra_params: Vec::new(),
            endpoint_override: None,
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // W1 keystone: Reverie maps zero standardized features (streaming ASR provider with no advanced
    // knobs), so from_standard is a pure from_base passthrough — assert it succeeds and the base
    // (api_key + app_id parsed from the model field) carries through unchanged.
    #[test]
    fn from_standard_passthrough_carries_base() {
        use crate::core::stt::standard::{ProviderExtras, SttFeatures, StandardSTTConfig};
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "reverie".into(),
                api_key: "test-api-key".into(),
                language: "hi".into(),
                model: "test-app-id".into(),
                ..Default::default()
            },
            features: SttFeatures {
                // None of these can map to a real Reverie field; they must be ignored.
                diarization: Some(true),
                word_timestamps: Some(true),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
            translation: None,
        };
        let cfg = ReverieSTTConfig::from_standard(&std).unwrap();
        assert_eq!(cfg.api_key, "test-api-key");
        assert_eq!(cfg.app_id, "test-app-id");
        assert_eq!(cfg.language, ReverieLanguage::Hindi);
    }

    #[test]
    fn test_language_codes() {
        assert_eq!(ReverieLanguage::Hindi.as_code(), "hi");
        assert_eq!(ReverieLanguage::English.as_code(), "en");
        assert_eq!(ReverieLanguage::Tamil.as_code(), "ta");
        assert_eq!(ReverieLanguage::Odia.as_code(), "or");
    }

    #[test]
    fn test_language_from_code() {
        assert_eq!(
            ReverieLanguage::from_code("hi"),
            Some(ReverieLanguage::Hindi)
        );
        assert_eq!(
            ReverieLanguage::from_code("hindi"),
            Some(ReverieLanguage::Hindi)
        );
        assert_eq!(
            ReverieLanguage::from_code("hi-in"),
            Some(ReverieLanguage::Hindi)
        );
        assert_eq!(ReverieLanguage::from_code("xx"), None);
    }

    #[test]
    fn test_punctuation_support() {
        assert!(ReverieLanguage::Hindi.supports_punctuation());
        assert!(ReverieLanguage::English.supports_punctuation());
        assert!(!ReverieLanguage::Tamil.supports_punctuation());
    }

    #[test]
    fn test_audio_format_codes() {
        assert_eq!(ReverieAudioFormat::Pcm16kInt16.as_str(), "16k_int16");
        assert_eq!(ReverieAudioFormat::Opus8k.as_str(), "opus_8k");
        assert_eq!(ReverieAudioFormat::OggOpus.as_str(), "ogg_opus");
    }

    #[test]
    fn test_audio_format_sample_rate() {
        assert_eq!(ReverieAudioFormat::Pcm16kInt16.sample_rate(), 16000);
        assert_eq!(ReverieAudioFormat::Pcm8kInt16.sample_rate(), 8000);
        assert_eq!(ReverieAudioFormat::Opus16k.sample_rate(), 16000);
    }

    #[test]
    fn test_audio_format_from_str() {
        assert_eq!(
            ReverieAudioFormat::from_str_value("linear16"),
            Some(ReverieAudioFormat::Pcm16kInt16)
        );
        assert_eq!(
            ReverieAudioFormat::from_str_value("pcm"),
            Some(ReverieAudioFormat::Pcm16kInt16)
        );
        assert_eq!(
            ReverieAudioFormat::from_str_value("opus"),
            Some(ReverieAudioFormat::Opus16k)
        );
    }

    #[test]
    fn test_config_builder() {
        let config = ReverieSTTConfig::new("test-key", "test-app")
            .with_language(ReverieLanguage::Hindi)
            .with_timeout(60)
            .with_continuous(true);

        assert_eq!(config.api_key, "test-key");
        assert_eq!(config.app_id, "test-app");
        assert_eq!(config.language, ReverieLanguage::Hindi);
        assert_eq!(config.timeout, 60);
        assert!(config.continuous);
    }

    #[test]
    fn test_config_validation() {
        let config = ReverieSTTConfig::new("", "app");
        assert!(config.validate().is_err());

        let config = ReverieSTTConfig::new("key", "");
        assert!(config.validate().is_err());

        let config = ReverieSTTConfig::new("key", "app");
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_websocket_url() {
        let config = ReverieSTTConfig::new("test-key", "test-app")
            .with_language(ReverieLanguage::Hindi)
            .with_timeout(30);

        let url = config.build_websocket_url();
        assert!(url.starts_with("wss://revapi.reverieinc.com/stream?"));
        assert!(url.contains("apikey=test-key"));
        assert!(url.contains("appid=test-app"));
        assert!(url.contains("src_lang=hi"));
        assert!(url.contains("timeout=30"));
    }

    #[test]
    fn test_punctuate_auto_disable() {
        let config = ReverieSTTConfig::new("key", "app")
            .with_language(ReverieLanguage::Tamil)
            .with_punctuate(true);

        // Tamil doesn't support punctuation, should be disabled
        assert!(!config.punctuate);
    }

    #[test]
    fn test_logging_modes() {
        assert_eq!(ReverieLogging::True.as_str(), "true");
        assert_eq!(ReverieLogging::NoAudio.as_str(), "no_audio");
        assert_eq!(ReverieLogging::NoTranscript.as_str(), "no_transcript");
        assert_eq!(ReverieLogging::False.as_str(), "false");
    }

    #[test]
    fn test_from_base_config() {
        let base_config = STTConfig {
            provider: "reverie".to_string(),
            api_key: "test-api-key".to_string(),
            language: "hi".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
            model: "test-app-id".to_string(),
        };

        let reverie_config = ReverieSTTConfig::from_base(&base_config).unwrap();
        assert_eq!(reverie_config.api_key, "test-api-key");
        assert_eq!(reverie_config.app_id, "test-app-id");
        assert_eq!(reverie_config.language, ReverieLanguage::Hindi);
        assert_eq!(reverie_config.format, ReverieAudioFormat::Pcm16kInt16);
        assert!(reverie_config.punctuate);
    }

    #[test]
    fn test_from_base_config_missing_api_key() {
        let base_config = STTConfig {
            provider: "reverie".to_string(),
            api_key: String::new(),
            language: "hi".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
            model: "test-app-id".to_string(),
        };

        let result = ReverieSTTConfig::from_base(&base_config);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("API key"));
    }
}
