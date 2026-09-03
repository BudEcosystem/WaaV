//! Gnani.ai STT Configuration
//!
//! Configuration types for Gnani's Speech-to-Text API with support for
//! 14 Indian and English language variants, gRPC streaming, and mTLS authentication.

use crate::core::stt::base::STTConfig;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

fn validate_gnani_stt_endpoint(source: &str, endpoint: &str) -> Result<(), String> {
    let endpoint = endpoint.trim();
    if endpoint.is_empty() {
        return Ok(());
    }
    crate::core::net::validate_url_for_ssrf(endpoint, crate::core::net::HTTP_URL_SCHEMES)
        .map_err(|msg| format!("{source} rejected (SSRF protection): {msg}"))
}

/// Gnani STT provider-specific configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GnaniSTTConfig {
    /// Base STT configuration (api_key, language, sample_rate, etc.)
    #[serde(flatten)]
    pub base: STTConfig,

    /// Gnani access token (received via email after registration)
    #[serde(default, skip_serializing)]
    pub token: String,

    /// Gnani access key (received via email after registration)
    #[serde(default, skip_serializing)]
    pub access_key: String,

    /// Override gRPC endpoint (plaintext, no TLS/cert) — used by mock e2e tests.
    #[serde(default, skip_serializing)]
    pub endpoint_override: Option<String>,

    /// Path to SSL certificate file (cert.pem)
    /// Required for gRPC connection authentication
    #[serde(default)]
    pub certificate_path: Option<PathBuf>,

    /// SSL certificate content (alternative to certificate_path)
    /// Useful for embedded/containerized deployments
    #[serde(default, skip_serializing)]
    pub certificate_content: Option<String>,

    /// Audio format for the input stream
    #[serde(default)]
    pub audio_format: GnaniAudioFormat,

    /// Connection timeout in seconds
    #[serde(default = "default_connection_timeout")]
    pub connection_timeout_secs: u64,

    /// Request timeout in seconds
    #[serde(default = "default_request_timeout")]
    pub request_timeout_secs: u64,

    /// Enable interim (partial) results
    #[serde(default = "default_interim_results")]
    pub interim_results: bool,

    /// Sensitive-data flag: sent to Gnani as the `sensitive` gRPC request header so the
    /// provider treats the audio as sensitive (e.g. suppresses logging/retention). `None`
    /// omits the header entirely. Maps from the `sensitive` ProviderExtras key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sensitive: Option<bool>,

    /// Optional audio filename forwarded as the `filename` gRPC request header for
    /// server-side debugging/correlation. `None` omits the header. Maps from the
    /// `filename` ProviderExtras key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
}

fn default_connection_timeout() -> u64 {
    10
}

fn default_request_timeout() -> u64 {
    30
}

fn default_interim_results() -> bool {
    true
}

impl Default for GnaniSTTConfig {
    fn default() -> Self {
        Self {
            base: STTConfig {
                provider: "gnani".to_string(),
                api_key: String::new(),
                language: "en-IN".to_string(),
                sample_rate: 16000,
                channels: 1,
                punctuation: true,
                encoding: "pcm16".to_string(),
                model: "default".to_string(),
            },
            token: String::new(),
            access_key: String::new(),
            endpoint_override: None,
            certificate_path: None,
            certificate_content: None,
            audio_format: GnaniAudioFormat::default(),
            connection_timeout_secs: default_connection_timeout(),
            request_timeout_secs: default_request_timeout(),
            interim_results: default_interim_results(),
            sensitive: None,
            filename: None,
        }
    }
}

impl GnaniSTTConfig {
    /// Build from the standardized config (W1 keystone). Gnani is a thin gRPC streaming provider
    /// whose only standardized-feature surface is interim (partial) results, so this maps that one
    /// boolean; the remaining advanced features (diarization, word_timestamps, redaction, keyterms,
    /// …) are capability gaps Gnani cannot express and are left at default. Gnani's non-standard
    /// credentials (`token`, `access_key`) and request headers (`sensitive`, `filename`) are read
    /// from `provider_extras` when present, overriding the env-var defaults that `from_base`
    /// supplies. `sensitive`/`filename` are forwarded as gRPC metadata by `create_gnani_metadata`.
    pub fn from_standard(
        std: &crate::core::stt::standard::StandardSTTConfig,
    ) -> Result<Self, String> {
        let f = &std.features;
        let mut cfg = Self::from_base(std.base.clone())?;
        if let Some(i) = f.interim_results {
            cfg.interim_results = i;
        }
        if let Some(t) = std.extras.0.get("token").and_then(|v| v.as_str()) {
            cfg.token = t.to_string();
        }
        if let Some(k) = std.extras.0.get("access_key").and_then(|v| v.as_str()) {
            cfg.access_key = k.to_string();
        }
        // Newly-wired Gnani request headers (forwarded as gRPC metadata by create_gnani_metadata).
        if let Some(s) = std.extras.0.get("sensitive").and_then(|v| v.as_bool()) {
            cfg.sensitive = Some(s);
        }
        if let Some(fname) = std.extras.0.get("filename").and_then(|v| v.as_str()) {
            cfg.filename = Some(fname.to_string());
        }
        cfg.endpoint_override = std.endpoint_override().map(|s| s.to_string());
        if let Some(endpoint) = &cfg.endpoint_override {
            validate_gnani_stt_endpoint("endpoint_override", endpoint)?;
        }
        Ok(cfg)
    }

    /// Create GnaniSTTConfig from base STTConfig
    ///
    /// Extracts Gnani-specific credentials from environment variables if not
    /// provided in the base config.
    pub fn from_base(base: STTConfig) -> Result<Self, String> {
        // Try to get credentials from environment if not in config
        let token = std::env::var("GNANI_TOKEN").unwrap_or_default();
        let access_key = std::env::var("GNANI_ACCESS_KEY").unwrap_or_default();
        let certificate_path = std::env::var("GNANI_CERTIFICATE_PATH")
            .ok()
            .map(PathBuf::from);
        let certificate_content = std::env::var("GNANI_CERTIFICATE_CONTENT").ok();

        // Parse language code to validate it's supported
        let language = GnaniLanguage::from_str(&base.language)
            .map(|l| l.as_str().to_string())
            .unwrap_or_else(|_| base.language.clone());

        // Parse audio format from encoding
        let audio_format = match base.encoding.to_lowercase().as_str() {
            "amr-wb" | "amrwb" => GnaniAudioFormat::AmrWb,
            _ => GnaniAudioFormat::Wav,
        };

        Ok(Self {
            base: STTConfig { language, ..base },
            token,
            access_key,
            endpoint_override: None,
            certificate_path,
            certificate_content,
            audio_format,
            connection_timeout_secs: default_connection_timeout(),
            request_timeout_secs: default_request_timeout(),
            interim_results: default_interim_results(),
            sensitive: None,
            filename: None,
        })
    }

    /// Validate that all required credentials are present
    pub fn validate(&self) -> Result<(), String> {
        if self.token.is_empty() {
            return Err(
                "Gnani token is required. Set GNANI_TOKEN environment variable.".to_string(),
            );
        }

        if self.access_key.is_empty() {
            return Err(
                "Gnani access key is required. Set GNANI_ACCESS_KEY environment variable."
                    .to_string(),
            );
        }

        if let Some(endpoint) = &self.endpoint_override {
            validate_gnani_stt_endpoint("endpoint_override", endpoint)?;
        }

        // The TLS CA certificate is only needed for the production endpoint; a plaintext
        // `endpoint_override` (mock/proxy) skips TLS entirely, so don't require a cert there.
        if self.endpoint_override.is_none()
            && self.certificate_path.is_none()
            && self.certificate_content.is_none()
        {
            return Err(
                "Gnani certificate is required. Set GNANI_CERTIFICATE_PATH or GNANI_CERTIFICATE_CONTENT environment variable.".to_string()
            );
        }

        // Validate certificate path exists if provided
        if let Some(ref path) = self.certificate_path
            && !path.exists()
        {
            return Err(format!(
                "Gnani certificate file not found: {}",
                path.display()
            ));
        }

        // Validate language is supported
        GnaniLanguage::from_str(&self.base.language)?;

        Ok(())
    }

    /// Load the SSL certificate content
    pub fn load_certificate(&self) -> Result<String, String> {
        if let Some(ref content) = self.certificate_content {
            return Ok(content.clone());
        }

        if let Some(ref path) = self.certificate_path {
            return std::fs::read_to_string(path)
                .map_err(|e| format!("Failed to read Gnani certificate: {}", e));
        }

        Err("No certificate path or content provided".to_string())
    }

    /// Get the gRPC endpoint URL
    pub fn endpoint(&self) -> &'static str {
        "https://asr.gnani.ai:443"
    }
}

/// Supported audio formats for Gnani STT
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GnaniAudioFormat {
    /// WAV format (PCM16, 16kHz, mono)
    #[default]
    Wav,
    /// AMR-WB format (16kHz)
    #[serde(rename = "amr-wb")]
    AmrWb,
}

impl GnaniAudioFormat {
    /// Get the format string for API requests
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Wav => "wav",
            Self::AmrWb => "amr-wb",
        }
    }

    /// Get the MIME type for this format
    pub fn mime_type(&self) -> &'static str {
        match self {
            Self::Wav => "audio/wav",
            Self::AmrWb => "audio/amr-wb",
        }
    }
}

/// Supported languages for Gnani STT (14 languages)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum GnaniLanguage {
    /// Kannada (kn-IN)
    #[serde(rename = "kn-IN")]
    Kannada,
    /// Hindi (hi-IN)
    #[serde(rename = "hi-IN")]
    Hindi,
    /// Tamil (ta-IN)
    #[serde(rename = "ta-IN")]
    Tamil,
    /// Telugu (te-IN)
    #[serde(rename = "te-IN")]
    Telugu,
    /// Gujarati (gu-IN)
    #[serde(rename = "gu-IN")]
    Gujarati,
    /// Marathi (mr-IN)
    #[serde(rename = "mr-IN")]
    Marathi,
    /// Bengali (bn-IN)
    #[serde(rename = "bn-IN")]
    Bengali,
    /// Malayalam (ml-IN)
    #[serde(rename = "ml-IN")]
    Malayalam,
    /// Punjabi (pa-guru-IN)
    #[serde(rename = "pa-guru-IN")]
    Punjabi,
    /// Urdu (ur-IN)
    #[serde(rename = "ur-IN")]
    Urdu,
    /// English - India (en-IN)
    #[serde(rename = "en-IN")]
    #[default]
    EnglishIndia,
    /// English - UK (en-GB)
    #[serde(rename = "en-GB")]
    EnglishUK,
    /// English - US (en-US)
    #[serde(rename = "en-US")]
    EnglishUS,
    /// English - Singapore (en-SG)
    #[serde(rename = "en-SG")]
    EnglishSG,
}

impl GnaniLanguage {
    /// Get the language code string for API requests
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Kannada => "kn-IN",
            Self::Hindi => "hi-IN",
            Self::Tamil => "ta-IN",
            Self::Telugu => "te-IN",
            Self::Gujarati => "gu-IN",
            Self::Marathi => "mr-IN",
            Self::Bengali => "bn-IN",
            Self::Malayalam => "ml-IN",
            Self::Punjabi => "pa-guru-IN",
            Self::Urdu => "ur-IN",
            Self::EnglishIndia => "en-IN",
            Self::EnglishUK => "en-GB",
            Self::EnglishUS => "en-US",
            Self::EnglishSG => "en-SG",
        }
    }

    /// Parse language code string to enum
    pub fn from_str(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "kn-in" | "kannada" => Ok(Self::Kannada),
            "hi-in" | "hindi" => Ok(Self::Hindi),
            "ta-in" | "tamil" => Ok(Self::Tamil),
            "te-in" | "telugu" => Ok(Self::Telugu),
            "gu-in" | "gujarati" => Ok(Self::Gujarati),
            "mr-in" | "marathi" => Ok(Self::Marathi),
            "bn-in" | "bengali" => Ok(Self::Bengali),
            "ml-in" | "malayalam" => Ok(Self::Malayalam),
            "pa-guru-in" | "punjabi" => Ok(Self::Punjabi),
            "ur-in" | "urdu" => Ok(Self::Urdu),
            "en-in" | "english-india" => Ok(Self::EnglishIndia),
            "en-gb" | "english-uk" => Ok(Self::EnglishUK),
            "en-us" | "english-us" => Ok(Self::EnglishUS),
            "en-sg" | "english-sg" => Ok(Self::EnglishSG),
            _ => Err(format!(
                "Unsupported Gnani language: {}. Supported: kn-IN, hi-IN, ta-IN, te-IN, \
                 gu-IN, mr-IN, bn-IN, ml-IN, pa-guru-IN, ur-IN, en-IN, en-GB, en-US, en-SG",
                s
            )),
        }
    }

    /// Get all supported language codes
    pub fn all_codes() -> &'static [&'static str] {
        &[
            "kn-IN",
            "hi-IN",
            "ta-IN",
            "te-IN",
            "gu-IN",
            "mr-IN",
            "bn-IN",
            "ml-IN",
            "pa-guru-IN",
            "ur-IN",
            "en-IN",
            "en-GB",
            "en-US",
            "en-SG",
        ]
    }

    /// Get the display name for this language
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Kannada => "Kannada",
            Self::Hindi => "Hindi",
            Self::Tamil => "Tamil",
            Self::Telugu => "Telugu",
            Self::Gujarati => "Gujarati",
            Self::Marathi => "Marathi",
            Self::Bengali => "Bengali",
            Self::Malayalam => "Malayalam",
            Self::Punjabi => "Punjabi",
            Self::Urdu => "Urdu",
            Self::EnglishIndia => "English (India)",
            Self::EnglishUK => "English (UK)",
            Self::EnglishUS => "English (US)",
            Self::EnglishSG => "English (Singapore)",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // W1 keystone: the standardized features unlock Gnani's only mappable feature surface
    // (interim/partial results) plus its non-standard credentials via extras — previously
    // unreachable through the flat factory.
    #[test]
    fn from_standard_maps_features() {
        use crate::core::stt::standard::{ProviderExtras, StandardSTTConfig, SttFeatures};
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "gnani".into(),
                api_key: String::new(),
                language: "hi-IN".into(),
                ..Default::default()
            },
            features: SttFeatures {
                interim_results: Some(false),
                ..Default::default()
            },
            extras: ProviderExtras(
                serde_json::json!({ "access_key": "ak-123" })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
            translation: None,
        };
        let cfg = GnaniSTTConfig::from_standard(&std).unwrap();
        assert!(!cfg.interim_results);
        assert_eq!(cfg.access_key, "ak-123");
    }

    #[test]
    fn test_config_validation_rejects_ssrf_endpoint_override() {
        let _env = crate::core::net::ssrf_env_lock();
        let mut config = GnaniSTTConfig {
            token: "test-token".to_string(),
            access_key: "test-access-key".to_string(),
            endpoint_override: Some("https://gnani-proxy.example.com".to_string()),
            ..Default::default()
        };

        assert!(config.validate().is_ok());

        config.endpoint_override = Some("http://127.0.0.1:9000".to_string());
        let err = config
            .validate()
            .expect_err("loopback endpoint_override must be rejected");
        assert!(err.contains("SSRF protection"), "{err}");

        config.endpoint_override = Some("grpc://gnani-proxy.example.com".to_string());
        let err = config
            .validate()
            .expect_err("non-HTTP endpoint_override must be rejected");
        assert!(err.contains("SSRF protection"), "{err}");
    }

    #[test]
    fn test_gnani_language_from_str() {
        assert_eq!(
            GnaniLanguage::from_str("hi-IN").unwrap(),
            GnaniLanguage::Hindi
        );
        assert_eq!(
            GnaniLanguage::from_str("hindi").unwrap(),
            GnaniLanguage::Hindi
        );
        assert_eq!(
            GnaniLanguage::from_str("en-IN").unwrap(),
            GnaniLanguage::EnglishIndia
        );
        assert_eq!(
            GnaniLanguage::from_str("kn-IN").unwrap(),
            GnaniLanguage::Kannada
        );
        assert!(GnaniLanguage::from_str("invalid").is_err());
    }

    #[test]
    fn test_gnani_language_as_str() {
        assert_eq!(GnaniLanguage::Hindi.as_str(), "hi-IN");
        assert_eq!(GnaniLanguage::EnglishIndia.as_str(), "en-IN");
        assert_eq!(GnaniLanguage::Kannada.as_str(), "kn-IN");
        assert_eq!(GnaniLanguage::Punjabi.as_str(), "pa-guru-IN");
    }

    #[test]
    fn test_gnani_audio_format() {
        assert_eq!(GnaniAudioFormat::Wav.as_str(), "wav");
        assert_eq!(GnaniAudioFormat::AmrWb.as_str(), "amr-wb");
        assert_eq!(GnaniAudioFormat::default(), GnaniAudioFormat::Wav);
    }

    #[test]
    fn test_gnani_config_from_base() {
        let base = STTConfig {
            provider: "gnani".to_string(),
            api_key: String::new(),
            language: "hi-IN".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "pcm16".to_string(),
            model: "default".to_string(),
        };

        let config = GnaniSTTConfig::from_base(base).unwrap();
        assert_eq!(config.base.language, "hi-IN");
        assert_eq!(config.audio_format, GnaniAudioFormat::Wav);
    }

    #[test]
    fn test_gnani_config_validation_missing_token() {
        let config = GnaniSTTConfig::default();
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("token"));
    }
}
