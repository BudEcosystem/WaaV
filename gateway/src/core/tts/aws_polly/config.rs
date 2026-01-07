//! Configuration types for Amazon Polly TTS API.
//!
//! This module defines configuration options for Amazon Polly's
//! text-to-speech service. The configuration supports:
//! - AWS authentication (access key/secret or IAM roles)
//! - Voice selection (60+ voices across 30+ languages)
//! - Engine selection (standard, neural, long-form, generative)
//! - Audio output formats (mp3, ogg_vorbis, pcm)
//!
//! # Example
//!
//! ```rust,ignore
//! use waav_gateway::core::tts::aws_polly::{AwsPollyTTSConfig, PollyVoice, PollyEngine};
//!
//! let config = AwsPollyTTSConfig {
//!     voice_id: PollyVoice::Joanna,
//!     engine: PollyEngine::Neural,
//!     output_format: PollyOutputFormat::Pcm,
//!     sample_rate: Some(16000),
//!     ..Default::default()
//! };
//! ```

use serde::{Deserialize, Serialize};

use crate::core::stt::aws_transcribe::AwsRegion;
use crate::core::tts::base::TTSConfig;

// =============================================================================
// Polly Engine
// =============================================================================

/// Amazon Polly synthesis engine options.
///
/// Different engines provide different quality/latency trade-offs:
/// - **Standard**: Basic TTS, lowest latency, good for simple use cases
/// - **Neural**: High-quality neural voices, recommended for most applications
/// - **LongForm**: Optimized for longer content like audiobooks
/// - **Generative**: Latest generative AI voices with best quality
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PollyEngine {
    /// Standard TTS engine
    #[serde(rename = "standard")]
    Standard,
    /// Neural TTS engine (recommended)
    #[default]
    #[serde(rename = "neural")]
    Neural,
    /// Long-form TTS engine (for audiobooks, articles)
    #[serde(rename = "long-form")]
    LongForm,
    /// Generative AI TTS engine (highest quality)
    #[serde(rename = "generative")]
    Generative,
}

impl PollyEngine {
    /// Convert to AWS API string.
    #[inline]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Neural => "neural",
            Self::LongForm => "long-form",
            Self::Generative => "generative",
        }
    }

    /// Parse from string, with fallback to Neural.
    pub fn from_str_or_default(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "standard" => Self::Standard,
            "neural" => Self::Neural,
            "long-form" | "longform" | "long_form" => Self::LongForm,
            "generative" => Self::Generative,
            _ => Self::default(),
        }
    }
}

impl std::fmt::Display for PollyEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =============================================================================
// Polly Output Format
// =============================================================================

/// Audio output formats supported by Amazon Polly.
///
/// # Format Details
///
/// - **Mp3**: Compressed audio, good for streaming (default)
/// - **OggVorbis**: Open-source compression, good quality/size ratio
/// - **Pcm**: Raw uncompressed audio, lowest latency, 16-bit little-endian
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PollyOutputFormat {
    /// MP3 format (default, compressed)
    #[default]
    #[serde(rename = "mp3")]
    Mp3,
    /// OGG Vorbis format (compressed)
    #[serde(rename = "ogg_vorbis")]
    OggVorbis,
    /// PCM format (uncompressed, 16-bit signed little-endian)
    #[serde(rename = "pcm")]
    Pcm,
}

impl PollyOutputFormat {
    /// Convert to AWS API string.
    #[inline]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Mp3 => "mp3",
            Self::OggVorbis => "ogg_vorbis",
            Self::Pcm => "pcm",
        }
    }

    /// Get the MIME type for this format.
    #[inline]
    pub fn mime_type(&self) -> &'static str {
        match self {
            Self::Mp3 => "audio/mpeg",
            Self::OggVorbis => "audio/ogg",
            Self::Pcm => "audio/pcm",
        }
    }

    /// Get default sample rates for this format.
    #[inline]
    pub fn default_sample_rate(&self) -> u32 {
        match self {
            Self::Mp3 | Self::OggVorbis => 22050,
            Self::Pcm => 16000,
        }
    }

    /// Get supported sample rates for this format.
    pub fn supported_sample_rates(&self) -> &'static [u32] {
        match self {
            Self::Mp3 | Self::OggVorbis => &[8000, 16000, 22050, 24000],
            Self::Pcm => &[8000, 16000],
        }
    }

    /// Parse from string, with fallback to Mp3.
    pub fn from_str_or_default(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "mp3" | "mpeg" => Self::Mp3,
            "ogg_vorbis" | "ogg" | "vorbis" => Self::OggVorbis,
            "pcm" | "linear16" | "raw" => Self::Pcm,
            _ => Self::default(),
        }
    }
}

impl std::fmt::Display for PollyOutputFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =============================================================================
// Polly Voice
// =============================================================================

/// Popular Amazon Polly voices.
///
/// This is a subset of the 60+ voices available. For the complete list,
/// see: https://docs.aws.amazon.com/polly/latest/dg/voicelist.html
///
/// Neural voices are recommended for most applications.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PollyVoice {
    // US English Neural Voices
    /// Joanna - US English, Female, Neural (most popular)
    #[default]
    Joanna,
    /// Matthew - US English, Male, Neural
    Matthew,
    /// Salli - US English, Female, Neural
    Salli,
    /// Kendra - US English, Female, Neural
    Kendra,
    /// Kimberly - US English, Female, Neural
    Kimberly,
    /// Joey - US English, Male, Neural
    Joey,
    /// Ruth - US English, Female, Neural/Generative
    Ruth,
    /// Stephen - US English, Male, Neural
    Stephen,
    /// Kevin - US English, Male, Child, Neural
    Kevin,
    /// Ivy - US English, Female, Child, Neural
    Ivy,
    /// Justin - US English, Male, Child, Neural
    Justin,

    // UK English Neural Voices
    /// Amy - British English, Female, Neural
    Amy,
    /// Emma - British English, Female, Neural
    Emma,
    /// Brian - British English, Male, Neural
    Brian,
    /// Arthur - British English, Male, Neural
    Arthur,

    // Australian English Neural Voices
    /// Olivia - Australian English, Female, Neural
    Olivia,

    // Other Languages - Popular Neural Voices
    /// Léa - French, Female, Neural
    Lea,
    /// Hans - German, Male, Neural
    Hans,
    /// Vicki - German, Female, Neural
    Vicki,
    /// Mizuki - Japanese, Female, Neural
    Mizuki,
    /// Takumi - Japanese, Male, Neural
    Takumi,
    /// Seoyeon - Korean, Female, Neural
    Seoyeon,
    /// Zhiyu - Mandarin Chinese, Female, Neural
    Zhiyu,
    /// Camila - Portuguese (Brazilian), Female, Neural
    Camila,
    /// Vitoria - Portuguese (Brazilian), Female, Neural
    Vitoria,
    /// Lupe - Spanish (US), Female, Neural
    Lupe,
    /// Pedro - Spanish (US), Male, Neural
    Pedro,
    /// Lucia - Spanish (Castilian), Female, Neural
    Lucia,
    /// Enrique - Spanish (Castilian), Male, Neural
    Enrique,
    /// Mia - Spanish (Mexican), Female, Neural
    Mia,
    /// Bianca - Italian, Female, Neural
    Bianca,
    /// Adriano - Italian, Male, Neural
    Adriano,
    /// Ola - Polish, Female, Neural
    Ola,
    /// Kajal - Hindi, Female, Neural
    Kajal,
    /// Aria - New Zealand English, Female, Neural
    Aria,

    /// Custom voice ID (for voices not in this enum)
    #[serde(rename = "custom")]
    Custom(String),
}

impl PollyVoice {
    /// Convert to AWS voice ID string.
    pub fn as_str(&self) -> &str {
        match self {
            // US English
            Self::Joanna => "Joanna",
            Self::Matthew => "Matthew",
            Self::Salli => "Salli",
            Self::Kendra => "Kendra",
            Self::Kimberly => "Kimberly",
            Self::Joey => "Joey",
            Self::Ruth => "Ruth",
            Self::Stephen => "Stephen",
            Self::Kevin => "Kevin",
            Self::Ivy => "Ivy",
            Self::Justin => "Justin",
            // UK English
            Self::Amy => "Amy",
            Self::Emma => "Emma",
            Self::Brian => "Brian",
            Self::Arthur => "Arthur",
            // Australian
            Self::Olivia => "Olivia",
            // Other languages
            Self::Lea => "Léa",
            Self::Hans => "Hans",
            Self::Vicki => "Vicki",
            Self::Mizuki => "Mizuki",
            Self::Takumi => "Takumi",
            Self::Seoyeon => "Seoyeon",
            Self::Zhiyu => "Zhiyu",
            Self::Camila => "Camila",
            Self::Vitoria => "Vitoria",
            Self::Lupe => "Lupe",
            Self::Pedro => "Pedro",
            Self::Lucia => "Lucia",
            Self::Enrique => "Enrique",
            Self::Mia => "Mia",
            Self::Bianca => "Bianca",
            Self::Adriano => "Adriano",
            Self::Ola => "Ola",
            Self::Kajal => "Kajal",
            Self::Aria => "Aria",
            // Custom
            Self::Custom(id) => id,
        }
    }

    /// Get the default language code for this voice.
    pub fn language_code(&self) -> &'static str {
        match self {
            // US English
            Self::Joanna
            | Self::Matthew
            | Self::Salli
            | Self::Kendra
            | Self::Kimberly
            | Self::Joey
            | Self::Ruth
            | Self::Stephen
            | Self::Kevin
            | Self::Ivy
            | Self::Justin => "en-US",
            // UK English
            Self::Amy | Self::Emma | Self::Brian | Self::Arthur => "en-GB",
            // Australian
            Self::Olivia => "en-AU",
            // French
            Self::Lea => "fr-FR",
            // German
            Self::Hans | Self::Vicki => "de-DE",
            // Japanese
            Self::Mizuki | Self::Takumi => "ja-JP",
            // Korean
            Self::Seoyeon => "ko-KR",
            // Chinese
            Self::Zhiyu => "cmn-CN",
            // Portuguese
            Self::Camila | Self::Vitoria => "pt-BR",
            // Spanish
            Self::Lupe | Self::Pedro => "es-US",
            Self::Lucia | Self::Enrique => "es-ES",
            Self::Mia => "es-MX",
            // Italian
            Self::Bianca | Self::Adriano => "it-IT",
            // Polish
            Self::Ola => "pl-PL",
            // Hindi
            Self::Kajal => "hi-IN",
            // New Zealand
            Self::Aria => "en-NZ",
            // Custom - default to US English
            Self::Custom(_) => "en-US",
        }
    }

    /// Check if this voice supports the neural engine.
    pub fn supports_neural(&self) -> bool {
        // Most modern Polly voices support neural
        !matches!(self, Self::Custom(_))
    }

    /// Parse from string, with fallback to Custom voice if not recognized.
    ///
    /// Note: For unrecognized voices, preserves the original case to support
    /// custom voice IDs that may be case-sensitive.
    pub fn from_str_or_default(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "joanna" => Self::Joanna,
            "matthew" => Self::Matthew,
            "salli" => Self::Salli,
            "kendra" => Self::Kendra,
            "kimberly" => Self::Kimberly,
            "joey" => Self::Joey,
            "ruth" => Self::Ruth,
            "stephen" => Self::Stephen,
            "kevin" => Self::Kevin,
            "ivy" => Self::Ivy,
            "justin" => Self::Justin,
            "amy" => Self::Amy,
            "emma" => Self::Emma,
            "brian" => Self::Brian,
            "arthur" => Self::Arthur,
            "olivia" => Self::Olivia,
            "lea" | "léa" => Self::Lea,
            "hans" => Self::Hans,
            "vicki" => Self::Vicki,
            "mizuki" => Self::Mizuki,
            "takumi" => Self::Takumi,
            "seoyeon" => Self::Seoyeon,
            "zhiyu" => Self::Zhiyu,
            "camila" => Self::Camila,
            "vitoria" => Self::Vitoria,
            "lupe" => Self::Lupe,
            "pedro" => Self::Pedro,
            "lucia" => Self::Lucia,
            "enrique" => Self::Enrique,
            "mia" => Self::Mia,
            "bianca" => Self::Bianca,
            "adriano" => Self::Adriano,
            "ola" => Self::Ola,
            "kajal" => Self::Kajal,
            "aria" => Self::Aria,
            // Preserve original case for custom voices
            _ => Self::Custom(s.to_string()),
        }
    }

    /// Get all common voices for a language.
    pub fn voices_for_language(language: &str) -> Vec<PollyVoice> {
        match language.to_lowercase().as_str() {
            "en-us" | "en_us" => vec![
                Self::Joanna,
                Self::Matthew,
                Self::Salli,
                Self::Kendra,
                Self::Joey,
                Self::Ruth,
            ],
            "en-gb" | "en_gb" => vec![Self::Amy, Self::Emma, Self::Brian, Self::Arthur],
            "en-au" | "en_au" => vec![Self::Olivia],
            "de-de" | "de_de" => vec![Self::Hans, Self::Vicki],
            "fr-fr" | "fr_fr" => vec![Self::Lea],
            "ja-jp" | "ja_jp" => vec![Self::Mizuki, Self::Takumi],
            "es-us" | "es_us" => vec![Self::Lupe, Self::Pedro],
            _ => vec![Self::Joanna],
        }
    }
}

impl std::fmt::Display for PollyVoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =============================================================================
// Text Type
// =============================================================================

/// Input text type for Amazon Polly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TextType {
    /// Plain text input
    #[default]
    #[serde(rename = "text")]
    Text,
    /// SSML (Speech Synthesis Markup Language) input
    #[serde(rename = "ssml")]
    Ssml,
}

impl TextType {
    /// Convert to AWS API string.
    #[inline]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Ssml => "ssml",
        }
    }

    /// Parse from string.
    pub fn from_str_or_default(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "ssml" => Self::Ssml,
            _ => Self::Text,
        }
    }
}

impl std::fmt::Display for TextType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =============================================================================
// Main Configuration
// =============================================================================

/// Maximum text length for SynthesizeSpeech API (characters).
pub const MAX_TEXT_LENGTH: usize = 3000;

/// Maximum total input length including SSML tags (characters).
pub const MAX_TOTAL_LENGTH: usize = 6000;

/// Configuration for Amazon Polly TTS.
///
/// This configuration extends the base TTS configuration with
/// Amazon Polly-specific options.
///
/// # Authentication
///
/// AWS credentials can be provided via:
/// 1. `aws_access_key_id` and `aws_secret_access_key` fields
/// 2. Environment variables: `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`
/// 3. IAM instance profile (for EC2/ECS/Lambda)
/// 4. AWS credentials file (`~/.aws/credentials`)
///
/// # Best Practices
///
/// - Use neural engine for best quality
/// - Use PCM format for lowest latency in real-time applications
/// - Keep text under 3000 characters per request
/// - Use SSML for fine-grained control over pronunciation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AwsPollyTTSConfig {
    /// Base TTS configuration
    #[serde(flatten)]
    pub base: TTSConfig,

    /// AWS region for the Polly service
    #[serde(default)]
    pub region: AwsRegion,

    /// AWS access key ID (optional if using IAM roles)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aws_access_key_id: Option<String>,

    /// AWS secret access key (optional if using IAM roles)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aws_secret_access_key: Option<String>,

    /// AWS session token for temporary credentials (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aws_session_token: Option<String>,

    /// Voice to use for synthesis
    #[serde(default)]
    pub voice: PollyVoice,

    /// TTS engine to use
    #[serde(default)]
    pub engine: PollyEngine,

    /// Audio output format
    #[serde(default)]
    pub output_format: PollyOutputFormat,

    /// Input text type (plain text or SSML)
    #[serde(default)]
    pub text_type: TextType,

    /// Language code override (optional, defaults to voice's language)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language_code: Option<String>,

    /// Custom lexicon names to apply
    #[serde(default)]
    pub lexicon_names: Vec<String>,
}

impl Default for AwsPollyTTSConfig {
    fn default() -> Self {
        Self {
            base: TTSConfig {
                provider: "aws-polly".to_string(),
                api_key: String::new(), // Not used, AWS uses access keys
                voice_id: Some("Joanna".to_string()),
                model: "neural".to_string(),
                speaking_rate: Some(1.0),
                audio_format: Some("pcm".to_string()),
                sample_rate: Some(16000),
                connection_timeout: Some(30),
                request_timeout: Some(60),
                pronunciations: Vec::new(),
                request_pool_size: Some(4),
                emotion_config: None,
            },
            region: AwsRegion::default(),
            aws_access_key_id: None,
            aws_secret_access_key: None,
            aws_session_token: None,
            voice: PollyVoice::default(),
            engine: PollyEngine::default(),
            output_format: PollyOutputFormat::Pcm,
            text_type: TextType::default(),
            language_code: None,
            lexicon_names: Vec::new(),
        }
    }
}

impl AwsPollyTTSConfig {
    /// Create a new configuration with the given voice.
    pub fn with_voice(voice: PollyVoice) -> Self {
        let mut config = Self::default();
        config.base.voice_id = Some(voice.as_str().to_string());
        config.voice = voice;
        config
    }

    /// Validate the configuration.
    pub fn validate(&self) -> Result<(), String> {
        // Validate sample rate for the output format
        if let Some(rate) = self.base.sample_rate {
            let supported = self.output_format.supported_sample_rates();
            if !supported.contains(&rate) {
                return Err(format!(
                    "Sample rate {} is not supported for {} format. Supported rates: {:?}",
                    rate,
                    self.output_format.as_str(),
                    supported
                ));
            }
        }

        // Validate lexicon count (max 5)
        if self.lexicon_names.len() > 5 {
            return Err("Maximum 5 lexicons can be applied per request".to_string());
        }

        Ok(())
    }

    /// Check if explicit AWS credentials are provided.
    pub fn has_explicit_credentials(&self) -> bool {
        self.aws_access_key_id.is_some() && self.aws_secret_access_key.is_some()
    }

    /// Get the effective language code (voice default or override).
    pub fn effective_language_code(&self) -> &str {
        self.language_code
            .as_deref()
            .unwrap_or_else(|| self.voice.language_code())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_polly_engine() {
        assert_eq!(PollyEngine::Neural.as_str(), "neural");
        assert_eq!(PollyEngine::Standard.as_str(), "standard");
        assert_eq!(
            PollyEngine::from_str_or_default("long-form"),
            PollyEngine::LongForm
        );
        assert_eq!(
            PollyEngine::from_str_or_default("unknown"),
            PollyEngine::Neural
        );
    }

    #[test]
    fn test_polly_output_format() {
        assert_eq!(PollyOutputFormat::Mp3.as_str(), "mp3");
        assert_eq!(PollyOutputFormat::Pcm.as_str(), "pcm");
        assert_eq!(PollyOutputFormat::Pcm.mime_type(), "audio/pcm");
        assert_eq!(PollyOutputFormat::Pcm.default_sample_rate(), 16000);
        assert!(
            PollyOutputFormat::Pcm
                .supported_sample_rates()
                .contains(&16000)
        );
    }

    #[test]
    fn test_polly_voice() {
        assert_eq!(PollyVoice::Joanna.as_str(), "Joanna");
        assert_eq!(PollyVoice::Joanna.language_code(), "en-US");
        assert!(PollyVoice::Joanna.supports_neural());
        assert_eq!(
            PollyVoice::from_str_or_default("matthew"),
            PollyVoice::Matthew
        );
    }

    #[test]
    fn test_polly_voice_custom() {
        let custom = PollyVoice::from_str_or_default("CustomVoice123");
        assert!(matches!(custom, PollyVoice::Custom(_)));
        assert_eq!(custom.as_str(), "CustomVoice123");
    }

    #[test]
    fn test_voices_for_language() {
        let us_voices = PollyVoice::voices_for_language("en-US");
        assert!(us_voices.contains(&PollyVoice::Joanna));
        assert!(us_voices.contains(&PollyVoice::Matthew));

        let de_voices = PollyVoice::voices_for_language("de-DE");
        assert!(de_voices.contains(&PollyVoice::Hans));
    }

    #[test]
    fn test_config_default() {
        let config = AwsPollyTTSConfig::default();
        assert_eq!(config.voice, PollyVoice::Joanna);
        assert_eq!(config.engine, PollyEngine::Neural);
        assert_eq!(config.output_format, PollyOutputFormat::Pcm);
        assert_eq!(config.base.sample_rate, Some(16000));
    }

    #[test]
    fn test_config_validation_valid() {
        let config = AwsPollyTTSConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validation_invalid_sample_rate() {
        let mut config = AwsPollyTTSConfig::default();
        config.output_format = PollyOutputFormat::Pcm;
        config.base.sample_rate = Some(44100); // Not supported for PCM

        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Sample rate"));
    }

    #[test]
    fn test_config_validation_too_many_lexicons() {
        let mut config = AwsPollyTTSConfig::default();
        config.lexicon_names = vec![
            "lex1".to_string(),
            "lex2".to_string(),
            "lex3".to_string(),
            "lex4".to_string(),
            "lex5".to_string(),
            "lex6".to_string(), // 6th lexicon - should fail
        ];

        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("lexicons"));
    }

    #[test]
    fn test_config_with_voice() {
        let config = AwsPollyTTSConfig::with_voice(PollyVoice::Amy);
        assert_eq!(config.voice, PollyVoice::Amy);
        assert_eq!(config.base.voice_id, Some("Amy".to_string()));
    }

    #[test]
    fn test_effective_language_code() {
        let config = AwsPollyTTSConfig::default();
        assert_eq!(config.effective_language_code(), "en-US");

        let mut config_override = AwsPollyTTSConfig::default();
        config_override.language_code = Some("en-GB".to_string());
        assert_eq!(config_override.effective_language_code(), "en-GB");
    }

    #[test]
    fn test_has_explicit_credentials() {
        let mut config = AwsPollyTTSConfig::default();
        assert!(!config.has_explicit_credentials());

        config.aws_access_key_id = Some("AKIAIOSFODNN7EXAMPLE".to_string());
        assert!(!config.has_explicit_credentials());

        config.aws_secret_access_key = Some("wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".to_string());
        assert!(config.has_explicit_credentials());
    }

    #[test]
    fn test_text_type() {
        assert_eq!(TextType::Text.as_str(), "text");
        assert_eq!(TextType::Ssml.as_str(), "ssml");
        assert_eq!(TextType::from_str_or_default("ssml"), TextType::Ssml);
        assert_eq!(TextType::from_str_or_default("unknown"), TextType::Text);
    }
}
