//! Phonexia STT Configuration
//!
//! Configuration structs and enums for Phonexia on-premises STT provider.

use serde::{Deserialize, Serialize};
use url::form_urlencoded;

use crate::core::stt::{STTConfig, STTError};

use super::{
    DEFAULT_AUDIO_FORMAT, DEFAULT_CHANNELS, DEFAULT_SAMPLE_RATE, LOGIN_PATH, MAX_CHANNELS,
    MAX_SAMPLE_RATE, MIN_CHANNELS, MIN_SAMPLE_RATE, WEBSOCKET_PATH,
};

// =============================================================================
// Enums
// =============================================================================

/// Authentication method for Phonexia server
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[derive(Default)]
pub enum PhonexiaAuth {
    /// Token-based authentication (login first, then use X-SessionID)
    Token {
        /// Session token from login
        token: String,
    },
    /// HTTP Basic authentication
    Basic {
        /// Username
        username: String,
        /// Password
        password: String,
    },
    /// No authentication (for testing or open servers)
    #[default]
    None,
}


/// Result type format for transcription
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PhonexiaResultType {
    /// Single best transcription (default)
    #[default]
    OneBest,
    /// Multiple alternative transcriptions
    NBest,
    /// Confusion network with all alternatives
    ConfusionNetwork,
}

impl std::fmt::Display for PhonexiaResultType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OneBest => write!(f, "one_best"),
            Self::NBest => write!(f, "n_best"),
            Self::ConfusionNetwork => write!(f, "confusion_network"),
        }
    }
}

impl std::str::FromStr for PhonexiaResultType {
    type Err = STTError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "one_best" | "onebest" | "one-best" | "1best" => Ok(Self::OneBest),
            "n_best" | "nbest" | "n-best" => Ok(Self::NBest),
            "confusion_network" | "confusionnetwork" | "confusion-network" | "lattice" => {
                Ok(Self::ConfusionNetwork)
            }
            _ => Err(STTError::ConfigurationError(format!(
                "Unknown result type: {}. Valid: one_best, n_best, confusion_network",
                s
            ))),
        }
    }
}

// =============================================================================
// Configuration
// =============================================================================

/// Phonexia STT provider configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhonexiaSTTConfig {
    /// Phonexia server base URL (e.g., "https://your-phonexia-server.com")
    /// Required - no default endpoint (on-premises solution)
    pub server_url: String,

    /// Authentication method
    pub auth: PhonexiaAuth,

    /// Language code (e.g., "en-US", "cs", "de")
    pub language: String,

    /// Sample rate in Hz (8000-48000)
    pub sample_rate: u32,

    /// Number of audio channels (1 or 2)
    pub channels: u8,

    /// Audio format (only "s16le" supported for WebSocket)
    pub audio_format: String,

    /// Result type format
    pub result_type: PhonexiaResultType,

    /// Multiple result types to request simultaneously (Phonexia `result_types[]`).
    ///
    /// When non-empty, the server returns every listed result type for each segment (e.g.
    /// `one_best` + `n_best` + `confusion_network`) instead of only the single `result_type`.
    /// Emitted as repeated `result_types[]=<type>` query params on the WebSocket URL.
    pub result_types: Vec<PhonexiaResultType>,

    /// Number of N-best alternatives (when result_type is NBest)
    pub n_best_count: Option<u32>,

    /// Preferred phrases for boosting recognition
    pub preferred_phrases: Vec<String>,

    /// Custom words with pronunciations
    pub custom_words: Vec<CustomWord>,

    /// Enable word-level timestamps
    pub enable_timestamps: bool,

    /// Enable confidence scores
    pub enable_confidence: bool,

    /// Connection timeout in seconds
    pub connection_timeout_seconds: u64,

    /// Request timeout in seconds
    pub request_timeout_seconds: u64,

    /// Custom WebSocket path (default: /input_stream/websocket)
    pub websocket_path: Option<String>,

    /// Enable TLS certificate verification
    pub verify_tls: bool,
}

/// Custom word definition with pronunciation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomWord {
    /// Word spelling
    pub spelling: String,
    /// Phonetic pronunciations (min 3 phonemes each)
    pub pronunciations: Vec<String>,
}

impl Default for PhonexiaSTTConfig {
    fn default() -> Self {
        Self {
            server_url: String::new(),
            auth: PhonexiaAuth::None,
            language: "en-US".to_string(),
            sample_rate: DEFAULT_SAMPLE_RATE,
            channels: DEFAULT_CHANNELS,
            audio_format: DEFAULT_AUDIO_FORMAT.to_string(),
            result_type: PhonexiaResultType::OneBest,
            result_types: Vec::new(),
            n_best_count: None,
            preferred_phrases: Vec::new(),
            custom_words: Vec::new(),
            enable_timestamps: true,
            enable_confidence: true,
            connection_timeout_seconds: super::DEFAULT_CONNECTION_TIMEOUT_SECONDS,
            request_timeout_seconds: super::DEFAULT_REQUEST_TIMEOUT_SECONDS,
            websocket_path: None,
            verify_tls: true,
        }
    }
}

impl PhonexiaSTTConfig {
    /// Create a new Phonexia configuration with server URL
    pub fn new(server_url: impl Into<String>) -> Self {
        Self {
            server_url: server_url.into(),
            ..Default::default()
        }
    }

    /// Set token-based authentication
    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.auth = PhonexiaAuth::Token {
            token: token.into(),
        };
        self
    }

    /// Set basic authentication
    pub fn with_basic_auth(
        mut self,
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> Self {
        self.auth = PhonexiaAuth::Basic {
            username: username.into(),
            password: password.into(),
        };
        self
    }

    /// Set language code
    pub fn with_language(mut self, language: impl Into<String>) -> Self {
        self.language = language.into();
        self
    }

    /// Set sample rate
    pub fn with_sample_rate(mut self, sample_rate: u32) -> Self {
        self.sample_rate = sample_rate;
        self
    }

    /// Set number of channels
    pub fn with_channels(mut self, channels: u8) -> Self {
        self.channels = channels;
        self
    }

    /// Set result type
    pub fn with_result_type(mut self, result_type: PhonexiaResultType) -> Self {
        self.result_type = result_type;
        self
    }

    /// Set N-best count (for NBest result type)
    pub fn with_n_best_count(mut self, count: u32) -> Self {
        self.n_best_count = Some(count);
        self
    }

    /// Add preferred phrases
    pub fn with_preferred_phrases(mut self, phrases: Vec<String>) -> Self {
        self.preferred_phrases = phrases;
        self
    }

    /// Add a custom word
    pub fn with_custom_word(mut self, spelling: String, pronunciations: Vec<String>) -> Self {
        self.custom_words.push(CustomWord {
            spelling,
            pronunciations,
        });
        self
    }

    /// Enable or disable timestamps
    pub fn with_timestamps(mut self, enable: bool) -> Self {
        self.enable_timestamps = enable;
        self
    }

    /// Set custom WebSocket path
    pub fn with_websocket_path(mut self, path: impl Into<String>) -> Self {
        self.websocket_path = Some(path.into());
        self
    }

    /// Enable or disable TLS verification
    pub fn with_tls_verification(mut self, verify: bool) -> Self {
        self.verify_tls = verify;
        self
    }

    /// Validate the configuration
    pub fn validate(&self) -> Result<(), STTError> {
        // Validate server URL
        if self.server_url.is_empty() {
            return Err(STTError::ConfigurationError(
                "Phonexia server URL is required. Phonexia is an on-premises solution - \
                 configure the URL of your Phonexia server."
                    .to_string(),
            ));
        }

        // Validate URL format
        let url_lower = self.server_url.to_lowercase();
        if !url_lower.starts_with("http://") && !url_lower.starts_with("https://") {
            return Err(STTError::ConfigurationError(format!(
                "Invalid server URL: {}. Must start with http:// or https://",
                self.server_url
            )));
        }

        // Validate sample rate
        if self.sample_rate < MIN_SAMPLE_RATE || self.sample_rate > MAX_SAMPLE_RATE {
            return Err(STTError::ConfigurationError(format!(
                "Sample rate must be between {} and {} Hz, got {}",
                MIN_SAMPLE_RATE, MAX_SAMPLE_RATE, self.sample_rate
            )));
        }

        // Validate channels
        if self.channels < MIN_CHANNELS || self.channels > MAX_CHANNELS {
            return Err(STTError::ConfigurationError(format!(
                "Channels must be between {} and {}, got {}",
                MIN_CHANNELS, MAX_CHANNELS, self.channels
            )));
        }

        // Validate audio format (WebSocket only supports s16le)
        if self.audio_format != DEFAULT_AUDIO_FORMAT {
            return Err(STTError::ConfigurationError(format!(
                "WebSocket API only supports '{}' audio format, got '{}'",
                DEFAULT_AUDIO_FORMAT, self.audio_format
            )));
        }

        // Validate language
        if self.language.is_empty() {
            return Err(STTError::ConfigurationError(
                "Language code is required".to_string(),
            ));
        }

        // Validate custom words pronunciations
        for word in &self.custom_words {
            for pron in &word.pronunciations {
                // Split by whitespace to count phonemes
                let phoneme_count = pron.split_whitespace().count();
                if phoneme_count < 3 {
                    return Err(STTError::ConfigurationError(format!(
                        "Custom word '{}' pronunciation '{}' must have at least 3 phonemes, has {}",
                        word.spelling, pron, phoneme_count
                    )));
                }
            }
        }

        Ok(())
    }

    /// Build WebSocket URL with query parameters
    pub fn build_websocket_url(&self) -> String {
        // Parse base URL and construct WebSocket URL
        let base_url = self.server_url.trim_end_matches('/');
        let ws_path = self.websocket_path.as_deref().unwrap_or(WEBSOCKET_PATH);

        // Convert http:// to ws://, https:// to wss://
        let ws_base = if base_url.starts_with("https://") {
            base_url.replacen("https://", "wss://", 1)
        } else if base_url.starts_with("http://") {
            base_url.replacen("http://", "ws://", 1)
        } else {
            format!("wss://{}", base_url)
        };

        // Build query parameters
        let mut params = vec![
            ("frequency", self.sample_rate.to_string()),
            ("channels", self.channels.to_string()),
        ];

        // Add language if specified
        if !self.language.is_empty() {
            params.push(("language", self.language.clone()));
        }

        // Multiple result types (Phonexia `result_types[]`): one repeated query param per type.
        for rt in &self.result_types {
            params.push(("result_types[]", rt.to_string()));
        }

        // Build query string. Both key and value are percent-encoded so keys carrying reserved
        // characters (e.g. the `[]` in `result_types[]`) produce a well-formed URL. Plain keys
        // (`frequency`/`channels`/`language`) are unaffected since they have no special chars.
        let query_string: String = params
            .iter()
            .map(|(k, v)| {
                let key: String = form_urlencoded::byte_serialize(k.as_bytes()).collect();
                let encoded: String = form_urlencoded::byte_serialize(v.as_bytes()).collect();
                format!("{}={}", key, encoded)
            })
            .collect::<Vec<_>>()
            .join("&");

        format!("{}{}?{}", ws_base, ws_path, query_string)
    }

    /// Build REST login URL
    pub fn build_login_url(&self) -> String {
        let base_url = self.server_url.trim_end_matches('/');
        format!("{}{}", base_url, LOGIN_PATH)
    }

    /// Get authorization header value
    pub fn get_auth_header(&self) -> Option<String> {
        match &self.auth {
            PhonexiaAuth::Token { token } => Some(token.clone()),
            PhonexiaAuth::Basic { username, password } => {
                let credentials = format!("{}:{}", username, password);
                let encoded = base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    credentials.as_bytes(),
                );
                Some(format!("Basic {}", encoded))
            }
            PhonexiaAuth::None => None,
        }
    }

    /// Create config from base STTConfig
    pub fn from_base(config: &STTConfig) -> Result<Self, STTError> {
        // Server URL is stored in api_key field for Phonexia
        // (since Phonexia doesn't use API keys, we repurpose this field)
        let server_url = config.api_key.clone();

        if server_url.is_empty() {
            return Err(STTError::ConfigurationError(
                "Phonexia server URL is required. Set the server URL in the api_key field \
                 (Phonexia is on-premises and doesn't use API keys)."
                    .to_string(),
            ));
        }

        let mut phonexia_config = Self::new(server_url)
            .with_language(config.language.clone())
            .with_sample_rate(config.sample_rate)
            .with_channels(config.channels as u8);

        // Parse model field for result type
        if !config.model.is_empty()
            && let Ok(result_type) = config.model.parse::<PhonexiaResultType>() {
                phonexia_config = phonexia_config.with_result_type(result_type);
            }

        phonexia_config.validate()?;
        Ok(phonexia_config)
    }

    /// Build from the standardized config (W1 keystone — final batch). Phonexia is an
    /// on-premises, batch-oriented engine with a narrow tunable surface, so this maps the
    /// standardized features it can actually express: per-word timestamps (`enable_timestamps`)
    /// and key terms / phrase hints (`preferred_phrases`), plus the provider extra
    /// `multiple_result_types` (string array) → `result_types[]` (request several result types
    /// at once). Features Phonexia can't express (diarization, smart_format, profanity_filter,
    /// filler_words, interim_results, vad/endpointing, redaction, entity/language detection) are
    /// capability gaps and stay at their defaults.
    pub fn from_standard(
        std: &crate::core::stt::standard::StandardSTTConfig,
    ) -> Result<Self, STTError> {
        let f = &std.features;
        let mut cfg = Self::from_base(&std.base)?;
        if let Some(w) = f.word_timestamps {
            cfg.enable_timestamps = w;
        }
        if let Some(k) = &f.keyterms {
            cfg.preferred_phrases = k.clone();
        }
        // Provider extra `multiple_result_types` → result_types[]. Each entry is parsed against
        // the known result-type vocabulary; an unrecognized entry fails loudly (it would silently
        // neutralize the feature otherwise — the open-passthrough contract in standard.rs).
        if let Some(arr) = std.extras.0.get("multiple_result_types").and_then(|v| v.as_array()) {
            let mut types = Vec::with_capacity(arr.len());
            for v in arr {
                let s = v.as_str().ok_or_else(|| {
                    STTError::ConfigurationError(
                        "multiple_result_types must be an array of strings".to_string(),
                    )
                })?;
                types.push(s.parse::<PhonexiaResultType>()?);
            }
            cfg.result_types = types;
        }
        Ok(cfg)
    }
}

/// Convert PhonexiaSTTConfig to base STTConfig
impl From<PhonexiaSTTConfig> for STTConfig {
    fn from(config: PhonexiaSTTConfig) -> Self {
        STTConfig {
            provider: "phonexia".to_string(),
            api_key: config.server_url, // Store server URL in api_key field
            language: config.language,
            sample_rate: config.sample_rate,
            channels: config.channels as u16,
            punctuation: true, // Phonexia includes punctuation by default
            encoding: config.audio_format,
            model: config.result_type.to_string(),
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // W1 keystone (final batch): the standardized features map onto the two fields Phonexia can
    // express — per-word timestamps (`enable_timestamps`) and phrase hints (`preferred_phrases`).
    #[test]
    fn from_standard_maps_features() {
        use crate::core::stt::standard::{ProviderExtras, SttFeatures, StandardSTTConfig};
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "phonexia".into(),
                api_key: "https://phonexia.example.com".into(),
                ..Default::default()
            },
            features: SttFeatures {
                word_timestamps: Some(false),
                keyterms: Some(vec!["WaaV".into(), "Phonexia".into()]),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
            translation: None,
        };
        let cfg = PhonexiaSTTConfig::from_standard(&std).unwrap();
        assert!(!cfg.enable_timestamps); // word_timestamps
        assert_eq!(cfg.preferred_phrases, vec!["WaaV", "Phonexia"]); // keyterms
        // base (server URL carried through the api_key field) survived from_base.
        assert_eq!(cfg.server_url, "https://phonexia.example.com");
    }

    // WIRE-LEVEL (the recurring bug class): the `multiple_result_types` extra mapped in
    // `from_standard` must reach the WebSocket URL the client connects with (`build_websocket_url`)
    // — not just live on the config struct. This drives the standardized config through
    // `from_standard` then through the SAME URL builder the client uses, asserting each
    // `result_types[]=<type>` api_param is present in the query string.
    #[test]
    fn multiple_result_types_reach_websocket_url() {
        use crate::core::stt::standard::{ProviderExtras, SttFeatures, StandardSTTConfig};

        let mut extras = serde_json::Map::new();
        extras.insert(
            "multiple_result_types".into(),
            serde_json::json!(["one_best", "n_best", "confusion_network"]),
        );

        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "phonexia".into(),
                api_key: "https://phonexia.example.com".into(),
                language: "en-US".into(),
                sample_rate: 16000,
                channels: 1,
                ..Default::default()
            },
            features: SttFeatures::default(),
            extras: ProviderExtras(extras),
            translation: None,
        };

        let cfg = PhonexiaSTTConfig::from_standard(&std).unwrap();
        let url = cfg.build_websocket_url();

        // Each requested result type lands on the wire as a repeated query param.
        assert!(
            url.contains("result_types%5B%5D=one_best"),
            "one_best result_types[] not on the wire: {url}"
        );
        assert!(
            url.contains("result_types%5B%5D=n_best"),
            "n_best result_types[] not on the wire: {url}"
        );
        assert!(
            url.contains("result_types%5B%5D=confusion_network"),
            "confusion_network result_types[] not on the wire: {url}"
        );
    }

    // A config with no `multiple_result_types` extra omits `result_types[]` (additive: unchanged
    // wire shape).
    #[test]
    fn websocket_url_omits_result_types_when_unset() {
        let config = PhonexiaSTTConfig::new("https://phonexia.example.com")
            .with_sample_rate(16000)
            .with_channels(1)
            .with_language("en-US");
        let url = config.build_websocket_url();
        assert!(!url.contains("result_types"), "result_types[] should be omitted: {url}");
    }

    #[test]
    fn test_phonexia_config_new() {
        let config = PhonexiaSTTConfig::new("https://phonexia.example.com");
        assert_eq!(config.server_url, "https://phonexia.example.com");
        assert_eq!(config.sample_rate, DEFAULT_SAMPLE_RATE);
        assert_eq!(config.channels, DEFAULT_CHANNELS);
        assert!(config.language == "en-US");
    }

    #[test]
    fn test_phonexia_config_builder() {
        let config = PhonexiaSTTConfig::new("https://phonexia.example.com")
            .with_language("cs")
            .with_sample_rate(44100)
            .with_channels(2)
            .with_result_type(PhonexiaResultType::NBest)
            .with_n_best_count(5);

        assert_eq!(config.language, "cs");
        assert_eq!(config.sample_rate, 44100);
        assert_eq!(config.channels, 2);
        assert_eq!(config.result_type, PhonexiaResultType::NBest);
        assert_eq!(config.n_best_count, Some(5));
    }

    #[test]
    fn test_phonexia_config_token_auth() {
        let config =
            PhonexiaSTTConfig::new("https://phonexia.example.com").with_token("session-token-123");

        match config.auth {
            PhonexiaAuth::Token { token } => assert_eq!(token, "session-token-123"),
            _ => panic!("Expected Token auth"),
        }
    }

    #[test]
    fn test_phonexia_config_basic_auth() {
        let config = PhonexiaSTTConfig::new("https://phonexia.example.com")
            .with_basic_auth("admin", "secret");

        match &config.auth {
            PhonexiaAuth::Basic { username, password } => {
                assert_eq!(username, "admin");
                assert_eq!(password, "secret");
            }
            _ => panic!("Expected Basic auth"),
        }

        // Test auth header generation
        let auth_header = config.get_auth_header().unwrap();
        assert!(auth_header.starts_with("Basic "));
    }

    #[test]
    fn test_phonexia_config_validate_empty_url() {
        let config = PhonexiaSTTConfig::new("");
        let result = config.validate();
        assert!(result.is_err());
        if let Err(STTError::ConfigurationError(msg)) = result {
            assert!(msg.contains("server URL is required"));
        }
    }

    #[test]
    fn test_phonexia_config_validate_invalid_url() {
        let config = PhonexiaSTTConfig::new("not-a-valid-url");
        let result = config.validate();
        assert!(result.is_err());
        if let Err(STTError::ConfigurationError(msg)) = result {
            assert!(msg.contains("Must start with http://"));
        }
    }

    #[test]
    fn test_phonexia_config_validate_sample_rate() {
        let config = PhonexiaSTTConfig::new("https://phonexia.example.com").with_sample_rate(100);
        let result = config.validate();
        assert!(result.is_err());
        if let Err(STTError::ConfigurationError(msg)) = result {
            assert!(msg.contains("Sample rate"));
        }
    }

    #[test]
    fn test_phonexia_config_validate_channels() {
        let config = PhonexiaSTTConfig::new("https://phonexia.example.com").with_channels(10);
        let result = config.validate();
        assert!(result.is_err());
        if let Err(STTError::ConfigurationError(msg)) = result {
            assert!(msg.contains("Channels"));
        }
    }

    #[test]
    fn test_phonexia_config_validate_custom_word_phonemes() {
        let mut config = PhonexiaSTTConfig::new("https://phonexia.example.com");
        config.custom_words.push(CustomWord {
            spelling: "test".to_string(),
            pronunciations: vec!["T".to_string()], // Only 1 phoneme, need 3
        });

        let result = config.validate();
        assert!(result.is_err());
        if let Err(STTError::ConfigurationError(msg)) = result {
            assert!(msg.contains("phonemes"));
        }
    }

    #[test]
    fn test_phonexia_config_validate_success() {
        let config = PhonexiaSTTConfig::new("https://phonexia.example.com")
            .with_language("en-US")
            .with_sample_rate(16000)
            .with_channels(1);

        let result = config.validate();
        assert!(result.is_ok());
    }

    #[test]
    fn test_build_websocket_url_https() {
        let config = PhonexiaSTTConfig::new("https://phonexia.example.com")
            .with_sample_rate(16000)
            .with_channels(1)
            .with_language("en-US");

        let url = config.build_websocket_url();
        assert!(url.starts_with("wss://"));
        assert!(url.contains("phonexia.example.com"));
        assert!(url.contains("/input_stream/websocket"));
        assert!(url.contains("frequency=16000"));
        assert!(url.contains("channels=1"));
        assert!(url.contains("language=en-US"));
    }

    #[test]
    fn test_build_websocket_url_http() {
        let config = PhonexiaSTTConfig::new("http://localhost:8080")
            .with_sample_rate(44100)
            .with_channels(2);

        let url = config.build_websocket_url();
        assert!(url.starts_with("ws://"));
        assert!(url.contains("localhost:8080"));
        assert!(url.contains("frequency=44100"));
        assert!(url.contains("channels=2"));
    }

    #[test]
    fn test_build_websocket_url_custom_path() {
        let config = PhonexiaSTTConfig::new("https://phonexia.example.com")
            .with_websocket_path("/custom/ws/path");

        let url = config.build_websocket_url();
        assert!(url.contains("/custom/ws/path"));
        assert!(!url.contains("/input_stream/websocket"));
    }

    #[test]
    fn test_build_login_url() {
        let config = PhonexiaSTTConfig::new("https://phonexia.example.com");
        let url = config.build_login_url();
        assert_eq!(url, "https://phonexia.example.com/login");
    }

    #[test]
    fn test_result_type_from_str() {
        assert_eq!(
            "one_best".parse::<PhonexiaResultType>().unwrap(),
            PhonexiaResultType::OneBest
        );
        assert_eq!(
            "n_best".parse::<PhonexiaResultType>().unwrap(),
            PhonexiaResultType::NBest
        );
        assert_eq!(
            "confusion_network".parse::<PhonexiaResultType>().unwrap(),
            PhonexiaResultType::ConfusionNetwork
        );
        assert_eq!(
            "lattice".parse::<PhonexiaResultType>().unwrap(),
            PhonexiaResultType::ConfusionNetwork
        );
    }

    #[test]
    fn test_result_type_display() {
        assert_eq!(PhonexiaResultType::OneBest.to_string(), "one_best");
        assert_eq!(PhonexiaResultType::NBest.to_string(), "n_best");
        assert_eq!(
            PhonexiaResultType::ConfusionNetwork.to_string(),
            "confusion_network"
        );
    }

    #[test]
    fn test_from_base_config() {
        let base_config = STTConfig {
            provider: "phonexia".to_string(),
            api_key: "https://phonexia.example.com".to_string(),
            language: "cs".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "s16le".to_string(),
            model: "n_best".to_string(),
        };

        let phonexia_config = PhonexiaSTTConfig::from_base(&base_config).unwrap();
        assert_eq!(phonexia_config.server_url, "https://phonexia.example.com");
        assert_eq!(phonexia_config.language, "cs");
        assert_eq!(phonexia_config.sample_rate, 16000);
        assert_eq!(phonexia_config.result_type, PhonexiaResultType::NBest);
    }

    #[test]
    fn test_into_base_config() {
        let phonexia_config = PhonexiaSTTConfig::new("https://phonexia.example.com")
            .with_language("de")
            .with_sample_rate(44100)
            .with_result_type(PhonexiaResultType::ConfusionNetwork);

        let base_config: STTConfig = phonexia_config.into();
        assert_eq!(base_config.provider, "phonexia");
        assert_eq!(base_config.api_key, "https://phonexia.example.com");
        assert_eq!(base_config.language, "de");
        assert_eq!(base_config.sample_rate, 44100);
        assert_eq!(base_config.model, "confusion_network");
    }

    #[test]
    fn test_auth_header_none() {
        let config = PhonexiaSTTConfig::new("https://phonexia.example.com");
        assert!(config.get_auth_header().is_none());
    }

    #[test]
    fn test_auth_header_token() {
        let config =
            PhonexiaSTTConfig::new("https://phonexia.example.com").with_token("my-session-token");
        let header = config.get_auth_header();
        assert_eq!(header, Some("my-session-token".to_string()));
    }
}
