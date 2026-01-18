//! Configuration types for NAVER CLOVA Speech Recognition (CSR) API.
//!
//! This module provides configuration types for NAVER Cloud Platform's
//! CLOVA Speech Recognition service, which converts speech to text.
//!
//! # Overview
//!
//! CLOVA Speech Recognition (CSR) is optimized for short utterances (< 60 seconds)
//! and provides the highest recognition accuracy for Korean language.
//!
//! # Authentication
//!
//! The API uses a two-key authentication system:
//! - `X-NCP-APIGW-API-KEY-ID`: Client ID
//! - `X-NCP-APIGW-API-KEY`: Client Secret
//!
//! # Supported Languages
//!
//! | Code | Language | Notes |
//! |------|----------|-------|
//! | Kor | Korean | Highest accuracy |
//! | Eng | English | |
//! | Jpn | Japanese | |
//! | Chn | Chinese (Simplified) | |

use serde::{Deserialize, Serialize};

use crate::core::stt::base::{STTConfig, STTError};

// =============================================================================
// Constants
// =============================================================================

/// CLOVA Speech Recognition (CSR) API endpoint.
pub const NAVER_CSR_ENDPOINT: &str = "https://naveropenapi.apigw.ntruss.com/recog/v1/stt";

/// Maximum audio duration in seconds.
pub const MAX_AUDIO_DURATION_SECONDS: usize = 60;

/// Minimum sample rate in Hz.
pub const MIN_SAMPLE_RATE: u32 = 16000;

/// Default sample rate in Hz.
pub const DEFAULT_SAMPLE_RATE: u32 = 16000;

// =============================================================================
// Language
// =============================================================================

/// Supported languages for CLOVA Speech Recognition.
///
/// Korean has the highest recognition accuracy due to NAVER's extensive
/// research and accumulated data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum NaverClovaLanguage {
    /// Korean - highest recognition accuracy
    #[default]
    Korean,
    /// English
    English,
    /// Japanese
    Japanese,
    /// Chinese (Simplified)
    Chinese,
}

impl NaverClovaLanguage {
    /// Get the API language code.
    #[inline]
    pub fn code(&self) -> &'static str {
        match self {
            Self::Korean => "Kor",
            Self::English => "Eng",
            Self::Japanese => "Jpn",
            Self::Chinese => "Chn",
        }
    }

    /// Get the display name.
    #[inline]
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Korean => "Korean",
            Self::English => "English",
            Self::Japanese => "Japanese",
            Self::Chinese => "Chinese",
        }
    }

    /// Parse from string.
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "kor" | "korean" | "ko" | "ko-kr" => Some(Self::Korean),
            "eng" | "english" | "en" | "en-us" => Some(Self::English),
            "jpn" | "japanese" | "ja" | "jp" | "ja-jp" => Some(Self::Japanese),
            "chn" | "chinese" | "zh" | "zh-cn" => Some(Self::Chinese),
            _ => None,
        }
    }

    /// Get all supported languages.
    pub fn all() -> Vec<Self> {
        vec![Self::Korean, Self::English, Self::Japanese, Self::Chinese]
    }
}

impl std::fmt::Display for NaverClovaLanguage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.code())
    }
}

// =============================================================================
// Audio Format
// =============================================================================

/// Supported audio input formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum NaverClovaAudioFormat {
    /// Raw PCM audio (recommended)
    #[default]
    Pcm,
    /// WAV format
    Wav,
    /// MP3 format
    Mp3,
}

impl NaverClovaAudioFormat {
    /// Get the content type for HTTP request.
    #[inline]
    pub fn content_type(&self) -> &'static str {
        match self {
            Self::Pcm => "application/octet-stream",
            Self::Wav => "audio/wav",
            Self::Mp3 => "audio/mpeg",
        }
    }

    /// Get the file extension.
    #[inline]
    pub fn extension(&self) -> &'static str {
        match self {
            Self::Pcm => "pcm",
            Self::Wav => "wav",
            Self::Mp3 => "mp3",
        }
    }

    /// Parse from string.
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "pcm" | "raw" | "linear16" => Some(Self::Pcm),
            "wav" | "wave" => Some(Self::Wav),
            "mp3" | "mpeg" => Some(Self::Mp3),
            _ => None,
        }
    }
}

impl std::fmt::Display for NaverClovaAudioFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.extension())
    }
}

// =============================================================================
// Response
// =============================================================================

/// Response from CLOVA Speech Recognition API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NaverClovaSttResponse {
    /// Recognized text.
    pub text: String,
}

/// Error response from the API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NaverClovaErrorResponse {
    /// Error code.
    #[serde(rename = "errorCode")]
    pub error_code: Option<String>,
    /// Error message.
    #[serde(rename = "errorMessage")]
    pub error_message: Option<String>,
    /// Error description.
    pub error: Option<String>,
}

// =============================================================================
// Configuration
// =============================================================================

/// Configuration for NAVER CLOVA Speech Recognition.
///
/// # Example
///
/// ```rust,ignore
/// use waav_gateway::core::stt::naver_clova::{NaverClovaSttConfig, NaverClovaLanguage};
///
/// let config = NaverClovaSttConfig {
///     client_id: "your_client_id".to_string(),
///     client_secret: "your_client_secret".to_string(),
///     language: NaverClovaLanguage::Korean,
///     sample_rate: 16000,
///     ..Default::default()
/// };
/// ```
#[derive(Debug, Clone)]
pub struct NaverClovaSttConfig {
    /// Client ID from NAVER Cloud Platform.
    pub client_id: String,

    /// Client Secret from NAVER Cloud Platform.
    pub client_secret: String,

    /// Recognition language.
    pub language: NaverClovaLanguage,

    /// Audio sample rate in Hz.
    pub sample_rate: u32,

    /// Audio format.
    pub audio_format: NaverClovaAudioFormat,

    /// Connection timeout in seconds.
    pub connection_timeout_secs: u64,

    /// Request timeout in seconds.
    pub request_timeout_secs: u64,

    /// Custom endpoint URL (for enterprise deployments).
    pub custom_endpoint: Option<String>,
}

impl Default for NaverClovaSttConfig {
    fn default() -> Self {
        Self {
            client_id: String::new(),
            client_secret: String::new(),
            language: NaverClovaLanguage::default(),
            sample_rate: DEFAULT_SAMPLE_RATE,
            audio_format: NaverClovaAudioFormat::default(),
            connection_timeout_secs: 10,
            request_timeout_secs: 60,
            custom_endpoint: None,
        }
    }
}

impl NaverClovaSttConfig {
    /// Create configuration from base STTConfig.
    ///
    /// API key format: `client_id|client_secret`
    pub fn from_base(config: STTConfig) -> Result<Self, STTError> {
        if config.api_key.is_empty() {
            return Err(STTError::ConfigurationError(
                "NAVER CLOVA API key is required (format: client_id|client_secret)".to_string(),
            ));
        }

        // Parse client_id|client_secret format
        let parts: Vec<&str> = config.api_key.split('|').collect();
        if parts.len() != 2 {
            return Err(STTError::ConfigurationError(
                "NAVER CLOVA API key must be in format: client_id|client_secret".to_string(),
            ));
        }

        let client_id = parts[0].to_string();
        let client_secret = parts[1].to_string();

        if client_id.is_empty() || client_secret.is_empty() {
            return Err(STTError::ConfigurationError(
                "Both client_id and client_secret must be non-empty".to_string(),
            ));
        }

        // Parse language
        let language = if !config.language.is_empty() {
            NaverClovaLanguage::from_str(&config.language).unwrap_or_default()
        } else {
            NaverClovaLanguage::default()
        };

        // Parse audio format
        let audio_format = if !config.encoding.is_empty() {
            NaverClovaAudioFormat::from_str(&config.encoding).unwrap_or_default()
        } else {
            NaverClovaAudioFormat::default()
        };

        // Sample rate
        let sample_rate = if config.sample_rate > 0 {
            config.sample_rate
        } else {
            DEFAULT_SAMPLE_RATE
        };

        Ok(Self {
            client_id,
            client_secret,
            language,
            sample_rate,
            audio_format,
            connection_timeout_secs: 10,
            request_timeout_secs: 60,
            custom_endpoint: None,
        })
    }

    /// Validate the configuration.
    pub fn validate(&self) -> Result<(), STTError> {
        if self.client_id.is_empty() {
            return Err(STTError::ConfigurationError(
                "Client ID is required".to_string(),
            ));
        }

        if self.client_secret.is_empty() {
            return Err(STTError::ConfigurationError(
                "Client Secret is required".to_string(),
            ));
        }

        if self.sample_rate < MIN_SAMPLE_RATE {
            return Err(STTError::ConfigurationError(format!(
                "Sample rate must be at least {} Hz, got {}",
                MIN_SAMPLE_RATE, self.sample_rate
            )));
        }

        Ok(())
    }

    /// Get the API endpoint URL.
    #[inline]
    pub fn get_endpoint_url(&self) -> String {
        if let Some(ref custom) = self.custom_endpoint {
            format!("{}?lang={}", custom, self.language.code())
        } else {
            format!("{}?lang={}", NAVER_CSR_ENDPOINT, self.language.code())
        }
    }

    /// Get list of supported languages.
    pub fn supported_languages() -> Vec<(&'static str, &'static str)> {
        NaverClovaLanguage::all()
            .iter()
            .map(|lang| (lang.code(), lang.display_name()))
            .collect()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_language_code() {
        assert_eq!(NaverClovaLanguage::Korean.code(), "Kor");
        assert_eq!(NaverClovaLanguage::English.code(), "Eng");
        assert_eq!(NaverClovaLanguage::Japanese.code(), "Jpn");
        assert_eq!(NaverClovaLanguage::Chinese.code(), "Chn");
    }

    #[test]
    fn test_language_from_str() {
        assert_eq!(
            NaverClovaLanguage::from_str("Kor"),
            Some(NaverClovaLanguage::Korean)
        );
        assert_eq!(
            NaverClovaLanguage::from_str("korean"),
            Some(NaverClovaLanguage::Korean)
        );
        assert_eq!(
            NaverClovaLanguage::from_str("ko-KR"),
            Some(NaverClovaLanguage::Korean)
        );
        assert_eq!(
            NaverClovaLanguage::from_str("en-us"),
            Some(NaverClovaLanguage::English)
        );
        assert_eq!(
            NaverClovaLanguage::from_str("ja"),
            Some(NaverClovaLanguage::Japanese)
        );
        assert_eq!(
            NaverClovaLanguage::from_str("zh-cn"),
            Some(NaverClovaLanguage::Chinese)
        );
        assert_eq!(NaverClovaLanguage::from_str("invalid"), None);
    }

    #[test]
    fn test_audio_format_content_type() {
        assert_eq!(
            NaverClovaAudioFormat::Pcm.content_type(),
            "application/octet-stream"
        );
        assert_eq!(NaverClovaAudioFormat::Wav.content_type(), "audio/wav");
        assert_eq!(NaverClovaAudioFormat::Mp3.content_type(), "audio/mpeg");
    }

    #[test]
    fn test_audio_format_from_str() {
        assert_eq!(
            NaverClovaAudioFormat::from_str("pcm"),
            Some(NaverClovaAudioFormat::Pcm)
        );
        assert_eq!(
            NaverClovaAudioFormat::from_str("wav"),
            Some(NaverClovaAudioFormat::Wav)
        );
        assert_eq!(
            NaverClovaAudioFormat::from_str("mp3"),
            Some(NaverClovaAudioFormat::Mp3)
        );
        assert_eq!(NaverClovaAudioFormat::from_str("invalid"), None);
    }

    #[test]
    fn test_config_from_base() {
        let base = STTConfig {
            api_key: "client_id|client_secret".to_string(),
            language: "ko-KR".to_string(),
            sample_rate: 16000,
            ..Default::default()
        };

        let config = NaverClovaSttConfig::from_base(base).unwrap();
        assert_eq!(config.client_id, "client_id");
        assert_eq!(config.client_secret, "client_secret");
        assert_eq!(config.language, NaverClovaLanguage::Korean);
        assert_eq!(config.sample_rate, 16000);
    }

    #[test]
    fn test_config_from_base_invalid_format() {
        let base = STTConfig {
            api_key: "no_separator".to_string(),
            ..Default::default()
        };

        let result = NaverClovaSttConfig::from_base(base);
        assert!(result.is_err());
    }

    #[test]
    fn test_config_from_base_empty_api_key() {
        let base = STTConfig {
            api_key: String::new(),
            ..Default::default()
        };

        let result = NaverClovaSttConfig::from_base(base);
        assert!(result.is_err());
    }

    #[test]
    fn test_config_validation_valid() {
        let config = NaverClovaSttConfig {
            client_id: "test_id".to_string(),
            client_secret: "test_secret".to_string(),
            ..Default::default()
        };

        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validation_empty_client_id() {
        let config = NaverClovaSttConfig {
            client_id: String::new(),
            client_secret: "test_secret".to_string(),
            ..Default::default()
        };

        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_empty_client_secret() {
        let config = NaverClovaSttConfig {
            client_id: "test_id".to_string(),
            client_secret: String::new(),
            ..Default::default()
        };

        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_low_sample_rate() {
        let config = NaverClovaSttConfig {
            client_id: "test_id".to_string(),
            client_secret: "test_secret".to_string(),
            sample_rate: 8000,
            ..Default::default()
        };

        assert!(config.validate().is_err());
    }

    #[test]
    fn test_get_endpoint_url() {
        let config = NaverClovaSttConfig {
            client_id: "test_id".to_string(),
            client_secret: "test_secret".to_string(),
            language: NaverClovaLanguage::Korean,
            ..Default::default()
        };

        let url = config.get_endpoint_url();
        assert!(url.contains("naveropenapi.apigw.ntruss.com"));
        assert!(url.contains("lang=Kor"));
    }

    #[test]
    fn test_get_endpoint_url_custom() {
        let config = NaverClovaSttConfig {
            client_id: "test_id".to_string(),
            client_secret: "test_secret".to_string(),
            language: NaverClovaLanguage::English,
            custom_endpoint: Some("https://custom.endpoint.com/stt".to_string()),
            ..Default::default()
        };

        let url = config.get_endpoint_url();
        assert!(url.starts_with("https://custom.endpoint.com"));
        assert!(url.contains("lang=Eng"));
    }

    #[test]
    fn test_supported_languages() {
        let languages = NaverClovaSttConfig::supported_languages();
        assert_eq!(languages.len(), 4);
        assert!(languages.iter().any(|(code, _)| *code == "Kor"));
        assert!(languages.iter().any(|(code, _)| *code == "Eng"));
        assert!(languages.iter().any(|(code, _)| *code == "Jpn"));
        assert!(languages.iter().any(|(code, _)| *code == "Chn"));
    }

    #[test]
    fn test_response_deserialize() {
        let json = r#"{"text": "Hello world"}"#;
        let response: NaverClovaSttResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.text, "Hello world");
    }

    #[test]
    fn test_error_response_deserialize() {
        let json = r#"{"errorCode": "401", "errorMessage": "Unauthorized"}"#;
        let error: NaverClovaErrorResponse = serde_json::from_str(json).unwrap();
        assert_eq!(error.error_code, Some("401".to_string()));
        assert_eq!(error.error_message, Some("Unauthorized".to_string()));
    }

    #[test]
    fn test_default_config() {
        let config = NaverClovaSttConfig::default();
        assert!(config.client_id.is_empty());
        assert!(config.client_secret.is_empty());
        assert_eq!(config.language, NaverClovaLanguage::Korean);
        assert_eq!(config.sample_rate, DEFAULT_SAMPLE_RATE);
        assert_eq!(config.audio_format, NaverClovaAudioFormat::Pcm);
    }

    #[test]
    fn test_constants() {
        assert_eq!(
            NAVER_CSR_ENDPOINT,
            "https://naveropenapi.apigw.ntruss.com/recog/v1/stt"
        );
        assert_eq!(MAX_AUDIO_DURATION_SECONDS, 60);
        assert_eq!(MIN_SAMPLE_RATE, 16000);
    }
}
