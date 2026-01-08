//! Gnani.ai TTS Configuration
//!
//! Configuration types for Gnani's Text-to-Speech API with support for
//! 7 Indian and English languages, multi-speaker voices, and SSML gender.

use crate::core::tts::base::TTSConfig;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Gnani TTS provider-specific configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GnaniTTSConfig {
    /// Base TTS configuration
    #[serde(flatten)]
    pub base: TTSConfig,

    /// Gnani access token (received via email after registration)
    #[serde(default, skip_serializing)]
    pub token: String,

    /// Gnani access key (received via email after registration)
    #[serde(default, skip_serializing)]
    pub access_key: String,

    /// Path to SSL certificate file (cert.pem) - optional for TTS
    #[serde(default)]
    pub certificate_path: Option<PathBuf>,

    /// Language code for TTS
    #[serde(default)]
    pub language_code: GnaniTTSLanguage,

    /// Voice name (for multi-speaker)
    #[serde(default)]
    pub voice_name: Option<String>,

    /// SSML gender: MALE or FEMALE
    #[serde(default)]
    pub ssml_gender: GnaniGender,

    /// Output sample rate in Hz (default: 8000)
    #[serde(default = "default_sample_rate")]
    pub output_sample_rate: u32,

    /// Request timeout in seconds
    #[serde(default = "default_request_timeout")]
    pub request_timeout_secs: u64,
}

fn default_sample_rate() -> u32 {
    8000
}

fn default_request_timeout() -> u64 {
    30
}

impl Default for GnaniTTSConfig {
    fn default() -> Self {
        Self {
            base: TTSConfig {
                provider: "gnani".to_string(),
                api_key: String::new(),
                voice_id: None,
                model: "default".to_string(),
                speaking_rate: Some(1.0),
                audio_format: Some("pcm16".to_string()),
                sample_rate: Some(8000),
                connection_timeout: Some(10),
                request_timeout: Some(30),
                pronunciations: Vec::new(),
                request_pool_size: None,
                emotion_config: None,
            },
            token: String::new(),
            access_key: String::new(),
            certificate_path: None,
            language_code: GnaniTTSLanguage::default(),
            voice_name: None,
            ssml_gender: GnaniGender::default(),
            output_sample_rate: default_sample_rate(),
            request_timeout_secs: default_request_timeout(),
        }
    }
}

impl GnaniTTSConfig {
    /// Create GnaniTTSConfig from base TTSConfig
    pub fn from_base(base: TTSConfig) -> Result<Self, String> {
        // Get credentials from environment
        let token = std::env::var("GNANI_TOKEN").unwrap_or_default();
        let access_key = std::env::var("GNANI_ACCESS_KEY").unwrap_or_default();
        let certificate_path = std::env::var("GNANI_CERTIFICATE_PATH")
            .ok()
            .map(PathBuf::from);

        // Parse language from voice_id or use default
        let language_code = base
            .voice_id
            .as_ref()
            .and_then(|v| GnaniTTSLanguage::from_str(v).ok())
            .unwrap_or_default();

        // Parse gender from model field if present
        let ssml_gender = if base.model.to_uppercase().contains("MALE") {
            GnaniGender::Male
        } else {
            GnaniGender::Female
        };

        let output_sample_rate = base.sample_rate.unwrap_or(8000);

        Ok(Self {
            base,
            token,
            access_key,
            certificate_path,
            language_code,
            voice_name: None,
            ssml_gender,
            output_sample_rate,
            request_timeout_secs: default_request_timeout(),
        })
    }

    /// Validate configuration
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

        Ok(())
    }

    /// Get the TTS API endpoint
    pub fn endpoint(&self) -> &'static str {
        "https://asr.gnani.ai/synthesize"
    }
}

/// SSML Gender for voice selection
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum GnaniGender {
    /// Female voice
    #[default]
    #[serde(rename = "FEMALE")]
    Female,
    /// Male voice
    #[serde(rename = "MALE")]
    Male,
}

impl GnaniGender {
    /// Get the gender string for API requests
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Female => "FEMALE",
            Self::Male => "MALE",
        }
    }

    /// Parse from string
    pub fn from_str(s: &str) -> Result<Self, String> {
        match s.to_uppercase().as_str() {
            "FEMALE" | "F" => Ok(Self::Female),
            "MALE" | "M" => Ok(Self::Male),
            _ => Err(format!("Invalid gender: {}. Use MALE or FEMALE", s)),
        }
    }
}

/// Supported languages for Gnani TTS (7 languages with single/multi-speaker)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum GnaniTTSLanguage {
    /// English - India (En-IN) - supports single and multi-speaker
    #[default]
    #[serde(rename = "En-IN")]
    EnglishIndia,
    /// Hindi (Hi-IN) - supports single and multi-speaker
    #[serde(rename = "Hi-IN")]
    Hindi,
    /// Hindi Alternate (Hi-IN-al)
    #[serde(rename = "Hi-IN-al")]
    HindiAlt,
    /// Kannada (Kn-IN) - supports single and multi-speaker
    #[serde(rename = "Kn-IN")]
    Kannada,
    /// Tamil (Ta-IN) - supports single and multi-speaker
    #[serde(rename = "Ta-IN")]
    Tamil,
    /// Telugu (Te-IN) - supports single and multi-speaker
    #[serde(rename = "Te-IN")]
    Telugu,
    /// Marathi (Mr-IN) - Female only for single-speaker
    #[serde(rename = "Mr-IN")]
    Marathi,
    /// Malayalam (Ml-IN) - multi-speaker only
    #[serde(rename = "Ml-IN")]
    Malayalam,
    /// Gujarati (Gu-IN) - multi-speaker only
    #[serde(rename = "Gu-IN")]
    Gujarati,
    /// Bengali (Bn-IN) - multi-speaker only
    #[serde(rename = "Bn-IN")]
    Bengali,
    /// Punjabi (Pa-IN) - multi-speaker only
    #[serde(rename = "Pa-IN")]
    Punjabi,
    /// Nepali (Ne-NP) - multi-speaker only
    #[serde(rename = "Ne-NP")]
    Nepali,
}

impl GnaniTTSLanguage {
    /// Get the language code string for API requests
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::EnglishIndia => "En-IN",
            Self::Hindi => "Hi-IN",
            Self::HindiAlt => "Hi-IN-al",
            Self::Kannada => "Kn-IN",
            Self::Tamil => "Ta-IN",
            Self::Telugu => "Te-IN",
            Self::Marathi => "Mr-IN",
            Self::Malayalam => "Ml-IN",
            Self::Gujarati => "Gu-IN",
            Self::Bengali => "Bn-IN",
            Self::Punjabi => "Pa-IN",
            Self::Nepali => "Ne-NP",
        }
    }

    /// Parse language code string to enum
    pub fn from_str(s: &str) -> Result<Self, String> {
        match s.to_lowercase().replace('_', "-").as_str() {
            "en-in" | "english" | "english-india" => Ok(Self::EnglishIndia),
            "hi-in" | "hindi" => Ok(Self::Hindi),
            "hi-in-al" | "hindi-alt" => Ok(Self::HindiAlt),
            "kn-in" | "kannada" => Ok(Self::Kannada),
            "ta-in" | "tamil" => Ok(Self::Tamil),
            "te-in" | "telugu" => Ok(Self::Telugu),
            "mr-in" | "marathi" => Ok(Self::Marathi),
            "ml-in" | "malayalam" => Ok(Self::Malayalam),
            "gu-in" | "gujarati" => Ok(Self::Gujarati),
            "bn-in" | "bengali" => Ok(Self::Bengali),
            "pa-in" | "punjabi" => Ok(Self::Punjabi),
            "ne-np" | "nepali" => Ok(Self::Nepali),
            _ => Err(format!(
                "Unsupported Gnani TTS language: {}. Supported: En-IN, Hi-IN, Kn-IN, Ta-IN, Te-IN, Mr-IN, Ml-IN, Gu-IN, Bn-IN, Pa-IN, Ne-NP",
                s
            )),
        }
    }

    /// Get all supported language codes
    pub fn all_codes() -> &'static [&'static str] {
        &[
            "En-IN", "Hi-IN", "Hi-IN-al", "Kn-IN", "Ta-IN", "Te-IN", "Mr-IN", "Ml-IN", "Gu-IN",
            "Bn-IN", "Pa-IN", "Ne-NP",
        ]
    }

    /// Check if this language supports single-speaker voices
    pub fn supports_single_speaker(&self) -> bool {
        matches!(
            self,
            Self::EnglishIndia
                | Self::Hindi
                | Self::HindiAlt
                | Self::Kannada
                | Self::Tamil
                | Self::Telugu
                | Self::Marathi
        )
    }

    /// Check if this language supports multi-speaker voices
    pub fn supports_multi_speaker(&self) -> bool {
        // All languages support multi-speaker
        true
    }

    /// Get supported genders for single-speaker mode
    pub fn supported_genders(&self) -> &'static [GnaniGender] {
        match self {
            Self::Marathi => &[GnaniGender::Female],
            Self::EnglishIndia
            | Self::Hindi
            | Self::HindiAlt
            | Self::Kannada
            | Self::Tamil
            | Self::Telugu => &[GnaniGender::Male, GnaniGender::Female],
            // Multi-speaker only languages
            _ => &[],
        }
    }

    /// Get display name for the language
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::EnglishIndia => "English (India)",
            Self::Hindi => "Hindi",
            Self::HindiAlt => "Hindi (Alternate)",
            Self::Kannada => "Kannada",
            Self::Tamil => "Tamil",
            Self::Telugu => "Telugu",
            Self::Marathi => "Marathi",
            Self::Malayalam => "Malayalam",
            Self::Gujarati => "Gujarati",
            Self::Bengali => "Bengali",
            Self::Punjabi => "Punjabi",
            Self::Nepali => "Nepali",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gnani_tts_language_from_str() {
        assert_eq!(
            GnaniTTSLanguage::from_str("Hi-IN").unwrap(),
            GnaniTTSLanguage::Hindi
        );
        assert_eq!(
            GnaniTTSLanguage::from_str("hindi").unwrap(),
            GnaniTTSLanguage::Hindi
        );
        assert_eq!(
            GnaniTTSLanguage::from_str("En-IN").unwrap(),
            GnaniTTSLanguage::EnglishIndia
        );
        assert!(GnaniTTSLanguage::from_str("invalid").is_err());
    }

    #[test]
    fn test_gnani_tts_language_as_str() {
        assert_eq!(GnaniTTSLanguage::Hindi.as_str(), "Hi-IN");
        assert_eq!(GnaniTTSLanguage::EnglishIndia.as_str(), "En-IN");
        assert_eq!(GnaniTTSLanguage::Kannada.as_str(), "Kn-IN");
    }

    #[test]
    fn test_gnani_gender() {
        assert_eq!(GnaniGender::Female.as_str(), "FEMALE");
        assert_eq!(GnaniGender::Male.as_str(), "MALE");
        assert_eq!(GnaniGender::default(), GnaniGender::Female);
    }

    #[test]
    fn test_gnani_gender_from_str() {
        assert_eq!(GnaniGender::from_str("MALE").unwrap(), GnaniGender::Male);
        assert_eq!(
            GnaniGender::from_str("female").unwrap(),
            GnaniGender::Female
        );
        assert_eq!(GnaniGender::from_str("M").unwrap(), GnaniGender::Male);
        assert_eq!(GnaniGender::from_str("F").unwrap(), GnaniGender::Female);
    }

    #[test]
    fn test_gnani_tts_language_single_speaker_support() {
        assert!(GnaniTTSLanguage::Hindi.supports_single_speaker());
        assert!(GnaniTTSLanguage::EnglishIndia.supports_single_speaker());
        assert!(!GnaniTTSLanguage::Malayalam.supports_single_speaker());
        assert!(!GnaniTTSLanguage::Bengali.supports_single_speaker());
    }

    #[test]
    fn test_gnani_tts_marathi_female_only() {
        let genders = GnaniTTSLanguage::Marathi.supported_genders();
        assert_eq!(genders.len(), 1);
        assert_eq!(genders[0], GnaniGender::Female);
    }
}
