//! Baidu Speech Recognition Configuration
//!
//! Configuration types for Baidu AI Cloud Speech-to-Text service.

use crate::core::stt::base::{STTConfig, STTError};
use serde::{Deserialize, Serialize};

// =============================================================================
// Constants
// =============================================================================

/// OAuth 2.0 token endpoint for Baidu API authentication.
pub const BAIDU_OAUTH_URL: &str = "https://aip.baidubce.com/oauth/2.0/token";

/// Short audio recognition REST API endpoint.
pub const BAIDU_SHORT_ASR_URL: &str = "http://vop.baidu.com/server_api";

/// Short audio recognition REST API endpoint (HTTPS).
pub const BAIDU_SHORT_ASR_URL_HTTPS: &str = "https://vop.baidu.com/server_api";

/// Internal network endpoint for Baidu Cloud servers.
#[allow(dead_code)]
pub const BAIDU_SHORT_ASR_URL_INTERNAL: &str = "http://vop.baidubce.com/server_api";

/// Real-time WebSocket ASR endpoint.
pub const BAIDU_REALTIME_ASR_URL: &str = "wss://vop.baidu.com/realtime_asr";

/// Default recognition model (Mandarin with punctuation).
pub const DEFAULT_MODEL: &str = "1537";

/// Default sample rate.
pub const DEFAULT_SAMPLE_RATE: u32 = 16000;

/// Default audio format.
pub const DEFAULT_AUDIO_FORMAT: &str = "pcm";

/// Token validity period in seconds (30 days).
pub const TOKEN_VALIDITY_SECS: u64 = 2592000;

/// Maximum audio duration for short audio API (60 seconds).
pub const MAX_SHORT_AUDIO_DURATION_SECS: u32 = 60;

/// Maximum audio duration for real-time API (1 hour).
pub const MAX_REALTIME_AUDIO_DURATION_SECS: u32 = 3600;

/// Recommended chunk duration for WebSocket streaming (160ms).
pub const RECOMMENDED_CHUNK_DURATION_MS: u32 = 160;

/// Maximum time between audio frames before timeout (5 seconds).
pub const MAX_FRAME_INTERVAL_SECS: u32 = 5;

// =============================================================================
// Recognition Models
// =============================================================================

/// Baidu Speech Recognition models (dev_pid).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BaiduSttModel {
    /// Mandarin Chinese with punctuation (普通话，有标点)
    /// ID: 1537, supports custom vocabulary
    #[default]
    Mandarin = 1537,

    /// Mandarin Chinese without punctuation (普通话，无标点)
    /// ID: 1536
    MandarinNoPunctuation = 1536,

    /// English without punctuation
    /// ID: 1737, no custom vocabulary support
    English = 1737,

    /// Cantonese with punctuation (粤语)
    /// ID: 1637, no custom vocabulary support
    Cantonese = 1637,

    /// Sichuan dialect with punctuation (四川话)
    /// ID: 1837, no custom vocabulary support
    Sichuan = 1837,

    /// Far-field Mandarin (远场普通话)
    /// ID: 1936
    MandarinFarField = 1936,
}

impl BaiduSttModel {
    /// Get the dev_pid value for this model.
    pub fn dev_pid(&self) -> u32 {
        *self as u32
    }

    /// Parse a model from string.
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "1537" | "mandarin" | "zh" | "zh-cn" | "chinese" => Self::Mandarin,
            "1536" | "mandarin_no_punct" => Self::MandarinNoPunctuation,
            "1737" | "english" | "en" | "en-us" => Self::English,
            "1637" | "cantonese" | "yue" | "zh-yue" => Self::Cantonese,
            "1837" | "sichuan" | "zh-sichuan" => Self::Sichuan,
            "1936" | "mandarin_far" | "far_field" => Self::MandarinFarField,
            _ => Self::Mandarin,
        }
    }

    /// Get the model name as string.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Mandarin => "mandarin",
            Self::MandarinNoPunctuation => "mandarin_no_punct",
            Self::English => "english",
            Self::Cantonese => "cantonese",
            Self::Sichuan => "sichuan",
            Self::MandarinFarField => "mandarin_far_field",
        }
    }

    /// Check if this model supports custom vocabulary.
    pub fn supports_custom_vocabulary(&self) -> bool {
        matches!(self, Self::Mandarin | Self::MandarinNoPunctuation)
    }

    /// Get the language code for this model.
    pub fn language_code(&self) -> &'static str {
        match self {
            Self::Mandarin | Self::MandarinNoPunctuation | Self::MandarinFarField => "zh",
            Self::English => "en",
            Self::Cantonese => "yue",
            Self::Sichuan => "zh-sichuan",
        }
    }
}

// =============================================================================
// Audio Format
// =============================================================================

/// Supported audio formats for Baidu STT.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BaiduAudioFormat {
    /// PCM format (uncompressed, 16-bit, little-endian).
    #[default]
    Pcm,

    /// WAV format (PCM with header).
    Wav,

    /// AMR format (compressed).
    Amr,

    /// M4A format (AAC-LC codec).
    M4a,
}

impl BaiduAudioFormat {
    /// Get the format string for API requests.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pcm => "pcm",
            Self::Wav => "wav",
            Self::Amr => "amr",
            Self::M4a => "m4a",
        }
    }

    /// Parse a format from string.
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "pcm" | "linear16" => Some(Self::Pcm),
            "wav" => Some(Self::Wav),
            "amr" => Some(Self::Amr),
            "m4a" | "aac" => Some(Self::M4a),
            _ => None,
        }
    }
}

// =============================================================================
// Sample Rate
// =============================================================================

/// Supported sample rates for Baidu STT.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BaiduSampleRate {
    /// 16kHz sample rate (standard).
    #[default]
    Rate16000 = 16000,

    /// 8kHz sample rate (telephony, Mandarin only).
    Rate8000 = 8000,
}

impl BaiduSampleRate {
    /// Get the sample rate value.
    pub fn value(&self) -> u32 {
        *self as u32
    }

    /// Parse from integer.
    pub fn from_u32(rate: u32) -> Option<Self> {
        match rate {
            16000 => Some(Self::Rate16000),
            8000 => Some(Self::Rate8000),
            _ => None,
        }
    }

    /// Calculate chunk size for given duration in milliseconds.
    /// Formula: sample_rate * 2 (bytes per sample) * duration_ms / 1000
    pub fn chunk_size_for_duration(&self, duration_ms: u32) -> usize {
        (self.value() as usize * 2 * duration_ms as usize) / 1000
    }
}

// =============================================================================
// Configuration
// =============================================================================

/// Baidu STT configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BaiduSttConfig {
    /// API Key (client_id for OAuth).
    pub api_key: String,

    /// Secret Key (client_secret for OAuth).
    pub secret_key: String,

    /// Cached access token.
    #[serde(skip)]
    pub access_token: Option<String>,

    /// Token expiration timestamp.
    #[serde(skip)]
    pub token_expires_at: Option<u64>,

    /// Recognition model.
    pub model: BaiduSttModel,

    /// Audio format.
    pub audio_format: BaiduAudioFormat,

    /// Sample rate.
    pub sample_rate: BaiduSampleRate,

    /// Client unique identifier (for logging and analytics).
    pub cuid: String,

    /// Custom vocabulary model ID (optional, Mandarin only).
    pub lm_id: Option<u32>,

    /// Use HTTPS for REST API.
    pub use_https: bool,

    /// Use real-time WebSocket API instead of REST.
    pub use_realtime: bool,
}

impl Default for BaiduSttConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            secret_key: String::new(),
            access_token: None,
            token_expires_at: None,
            model: BaiduSttModel::default(),
            audio_format: BaiduAudioFormat::default(),
            sample_rate: BaiduSampleRate::default(),
            cuid: uuid::Uuid::new_v4().to_string(),
            lm_id: None,
            use_https: true,
            use_realtime: true,
        }
    }
}

impl BaiduSttConfig {
    /// Create a new configuration from API credentials.
    pub fn new(api_key: impl Into<String>, secret_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            secret_key: secret_key.into(),
            ..Default::default()
        }
    }

    /// Set the recognition model.
    pub fn with_model(mut self, model: BaiduSttModel) -> Self {
        self.model = model;
        self
    }

    /// Set the audio format.
    pub fn with_audio_format(mut self, format: BaiduAudioFormat) -> Self {
        self.audio_format = format;
        self
    }

    /// Set the sample rate.
    pub fn with_sample_rate(mut self, rate: BaiduSampleRate) -> Self {
        self.sample_rate = rate;
        self
    }

    /// Set the client unique identifier.
    pub fn with_cuid(mut self, cuid: impl Into<String>) -> Self {
        self.cuid = cuid.into();
        self
    }

    /// Set the custom vocabulary model ID.
    pub fn with_lm_id(mut self, lm_id: u32) -> Self {
        self.lm_id = Some(lm_id);
        self
    }

    /// Enable/disable HTTPS.
    pub fn with_https(mut self, use_https: bool) -> Self {
        self.use_https = use_https;
        self
    }

    /// Enable/disable real-time WebSocket mode.
    pub fn with_realtime(mut self, use_realtime: bool) -> Self {
        self.use_realtime = use_realtime;
        self
    }

    /// Validate the configuration.
    pub fn validate(&self) -> Result<(), STTError> {
        if self.api_key.is_empty() {
            return Err(STTError::ConfigurationError(
                "Baidu API Key is required".to_string(),
            ));
        }

        if self.secret_key.is_empty() {
            return Err(STTError::ConfigurationError(
                "Baidu Secret Key is required".to_string(),
            ));
        }

        if self.cuid.is_empty() || self.cuid.len() > 60 {
            return Err(STTError::ConfigurationError(
                "CUID must be 1-60 characters".to_string(),
            ));
        }

        // 8kHz is only supported for Mandarin
        if self.sample_rate == BaiduSampleRate::Rate8000 {
            if !matches!(
                self.model,
                BaiduSttModel::Mandarin | BaiduSttModel::MandarinNoPunctuation
            ) {
                return Err(STTError::ConfigurationError(
                    "8kHz sample rate is only supported for Mandarin models".to_string(),
                ));
            }
        }

        // Custom vocabulary only for Mandarin
        if self.lm_id.is_some() && !self.model.supports_custom_vocabulary() {
            return Err(STTError::ConfigurationError(
                "Custom vocabulary is only supported for Mandarin models".to_string(),
            ));
        }

        Ok(())
    }

    /// Get the OAuth URL for token retrieval.
    pub fn get_oauth_url(&self) -> String {
        format!(
            "{}?grant_type=client_credentials&client_id={}&client_secret={}",
            BAIDU_OAUTH_URL, self.api_key, self.secret_key
        )
    }

    /// Get the REST API URL for short audio recognition.
    pub fn get_short_asr_url(&self) -> &'static str {
        if self.use_https {
            BAIDU_SHORT_ASR_URL_HTTPS
        } else {
            BAIDU_SHORT_ASR_URL
        }
    }

    /// Get the WebSocket URL for real-time recognition.
    pub fn get_realtime_url(&self, session_id: &str) -> String {
        format!("{}?sn={}", BAIDU_REALTIME_ASR_URL, session_id)
    }

    /// Calculate the recommended chunk size for WebSocket streaming.
    pub fn get_chunk_size(&self) -> usize {
        self.sample_rate
            .chunk_size_for_duration(RECOMMENDED_CHUNK_DURATION_MS)
    }

    /// Check if the cached token is valid.
    pub fn is_token_valid(&self) -> bool {
        match (self.access_token.as_ref(), self.token_expires_at) {
            (Some(_), Some(expires_at)) => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                // Consider token invalid if less than 1 hour remaining
                now < expires_at.saturating_sub(3600)
            }
            _ => false,
        }
    }

    /// Get list of supported languages.
    pub fn supported_languages() -> Vec<&'static str> {
        vec!["zh", "en", "yue", "zh-sichuan"]
    }

    /// Get list of supported models.
    pub fn supported_models() -> Vec<&'static str> {
        vec![
            "1537",
            "mandarin",
            "1536",
            "mandarin_no_punct",
            "1737",
            "english",
            "1637",
            "cantonese",
            "1837",
            "sichuan",
            "1936",
            "mandarin_far_field",
        ]
    }

    /// Create from base STT config.
    pub fn from_base(config: STTConfig) -> Result<Self, STTError> {
        // API key format: api_key|secret_key
        let (api_key, secret_key) = if config.api_key.contains('|') {
            let parts: Vec<&str> = config.api_key.splitn(2, '|').collect();
            if parts.len() != 2 {
                return Err(STTError::ConfigurationError(
                    "Invalid API key format. Expected: api_key|secret_key".to_string(),
                ));
            }
            (parts[0].to_string(), parts[1].to_string())
        } else {
            return Err(STTError::ConfigurationError(
                "API key must be in format: api_key|secret_key".to_string(),
            ));
        };

        let model = BaiduSttModel::from_str(&config.model);
        let audio_format =
            BaiduAudioFormat::from_str(&config.encoding).unwrap_or(BaiduAudioFormat::Pcm);
        let sample_rate =
            BaiduSampleRate::from_u32(config.sample_rate).unwrap_or(BaiduSampleRate::Rate16000);

        let baidu_config = Self {
            api_key,
            secret_key,
            model,
            audio_format,
            sample_rate,
            ..Default::default()
        };

        baidu_config.validate()?;
        Ok(baidu_config)
    }
}

// =============================================================================
// OAuth Response
// =============================================================================

/// OAuth 2.0 token response from Baidu.
#[derive(Debug, Clone, Deserialize)]
pub struct BaiduOAuthResponse {
    /// Access token for API calls.
    pub access_token: String,

    /// Token validity period in seconds.
    pub expires_in: u64,

    /// Refresh token (not used for client credentials).
    pub refresh_token: Option<String>,

    /// Session key.
    pub session_key: Option<String>,

    /// Token scope.
    pub scope: Option<String>,
}

/// OAuth error response.
#[derive(Debug, Clone, Deserialize)]
pub struct BaiduOAuthError {
    /// Error type.
    pub error: String,

    /// Error description.
    pub error_description: Option<String>,
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_parsing() {
        assert_eq!(BaiduSttModel::from_str("mandarin"), BaiduSttModel::Mandarin);
        assert_eq!(BaiduSttModel::from_str("1537"), BaiduSttModel::Mandarin);
        assert_eq!(BaiduSttModel::from_str("zh"), BaiduSttModel::Mandarin);
        assert_eq!(BaiduSttModel::from_str("english"), BaiduSttModel::English);
        assert_eq!(BaiduSttModel::from_str("1737"), BaiduSttModel::English);
        assert_eq!(
            BaiduSttModel::from_str("cantonese"),
            BaiduSttModel::Cantonese
        );
        assert_eq!(BaiduSttModel::from_str("sichuan"), BaiduSttModel::Sichuan);
    }

    #[test]
    fn test_model_dev_pid() {
        assert_eq!(BaiduSttModel::Mandarin.dev_pid(), 1537);
        assert_eq!(BaiduSttModel::English.dev_pid(), 1737);
        assert_eq!(BaiduSttModel::Cantonese.dev_pid(), 1637);
        assert_eq!(BaiduSttModel::Sichuan.dev_pid(), 1837);
    }

    #[test]
    fn test_model_custom_vocabulary_support() {
        assert!(BaiduSttModel::Mandarin.supports_custom_vocabulary());
        assert!(BaiduSttModel::MandarinNoPunctuation.supports_custom_vocabulary());
        assert!(!BaiduSttModel::English.supports_custom_vocabulary());
        assert!(!BaiduSttModel::Cantonese.supports_custom_vocabulary());
    }

    #[test]
    fn test_audio_format_parsing() {
        assert_eq!(
            BaiduAudioFormat::from_str("pcm"),
            Some(BaiduAudioFormat::Pcm)
        );
        assert_eq!(
            BaiduAudioFormat::from_str("wav"),
            Some(BaiduAudioFormat::Wav)
        );
        assert_eq!(
            BaiduAudioFormat::from_str("amr"),
            Some(BaiduAudioFormat::Amr)
        );
        assert_eq!(
            BaiduAudioFormat::from_str("m4a"),
            Some(BaiduAudioFormat::M4a)
        );
        assert_eq!(BaiduAudioFormat::from_str("invalid"), None);
    }

    #[test]
    fn test_sample_rate_chunk_size() {
        let rate_16k = BaiduSampleRate::Rate16000;
        // 160ms chunk at 16kHz: 16000 * 2 * 160 / 1000 = 5120 bytes
        assert_eq!(rate_16k.chunk_size_for_duration(160), 5120);

        let rate_8k = BaiduSampleRate::Rate8000;
        // 160ms chunk at 8kHz: 8000 * 2 * 160 / 1000 = 2560 bytes
        assert_eq!(rate_8k.chunk_size_for_duration(160), 2560);
    }

    #[test]
    fn test_config_validation_valid() {
        let config = BaiduSttConfig::new("api_key", "secret_key");
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validation_empty_api_key() {
        let config = BaiduSttConfig::new("", "secret_key");
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_empty_secret_key() {
        let config = BaiduSttConfig::new("api_key", "");
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_8k_english() {
        let config = BaiduSttConfig::new("api_key", "secret_key")
            .with_model(BaiduSttModel::English)
            .with_sample_rate(BaiduSampleRate::Rate8000);
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_lm_id_english() {
        let config = BaiduSttConfig::new("api_key", "secret_key")
            .with_model(BaiduSttModel::English)
            .with_lm_id(12345);
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_oauth_url() {
        let config = BaiduSttConfig::new("my_api_key", "my_secret_key");
        let url = config.get_oauth_url();
        assert!(url.contains("grant_type=client_credentials"));
        assert!(url.contains("client_id=my_api_key"));
        assert!(url.contains("client_secret=my_secret_key"));
    }

    #[test]
    fn test_realtime_url() {
        let config = BaiduSttConfig::default();
        let url = config.get_realtime_url("test-session-123");
        assert!(url.contains("wss://vop.baidu.com/realtime_asr"));
        assert!(url.contains("sn=test-session-123"));
    }

    #[test]
    fn test_from_base_config() {
        let base = STTConfig {
            api_key: "my_api_key|my_secret_key".to_string(),
            language: "zh".to_string(),
            sample_rate: 16000,
            encoding: "pcm".to_string(),
            model: "mandarin".to_string(),
            ..Default::default()
        };

        let config = BaiduSttConfig::from_base(base).unwrap();
        assert_eq!(config.api_key, "my_api_key");
        assert_eq!(config.secret_key, "my_secret_key");
        assert_eq!(config.model, BaiduSttModel::Mandarin);
    }

    #[test]
    fn test_from_base_config_invalid_format() {
        let base = STTConfig {
            api_key: "api_key_without_secret".to_string(),
            ..Default::default()
        };

        assert!(BaiduSttConfig::from_base(base).is_err());
    }

    #[test]
    fn test_supported_languages() {
        let languages = BaiduSttConfig::supported_languages();
        assert!(languages.contains(&"zh"));
        assert!(languages.contains(&"en"));
        assert!(languages.contains(&"yue"));
    }

    #[test]
    fn test_token_validity() {
        let mut config = BaiduSttConfig::new("api_key", "secret_key");

        // No token
        assert!(!config.is_token_valid());

        // Set valid token
        config.access_token = Some("token".to_string());
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        config.token_expires_at = Some(now + 7200); // 2 hours from now
        assert!(config.is_token_valid());

        // Set expired token
        config.token_expires_at = Some(now - 100); // Already expired
        assert!(!config.is_token_valid());
    }
}
