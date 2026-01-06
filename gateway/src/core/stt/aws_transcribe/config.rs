//! Configuration types for Amazon Transcribe Streaming STT API.
//!
//! This module defines configuration options for Amazon Transcribe's real-time
//! streaming transcription service. The configuration supports:
//! - AWS authentication (access key/secret or IAM roles)
//! - Audio encoding formats (PCM, FLAC, OPUS)
//! - Language selection (100+ languages)
//! - Partial results stabilization for low-latency applications
//! - Speaker diarization and channel identification
//!
//! # Example
//!
//! ```rust,ignore
//! use waav_gateway::core::stt::aws_transcribe::{AwsTranscribeSTTConfig, AwsRegion, MediaEncoding};
//!
//! let config = AwsTranscribeSTTConfig {
//!     region: AwsRegion::UsEast1,
//!     language_code: "en-US".to_string(),
//!     media_encoding: MediaEncoding::Pcm,
//!     sample_rate: 16000,
//!     enable_partial_results_stabilization: true,
//!     partial_results_stability: PartialResultsStability::High,
//!     ..Default::default()
//! };
//! ```

use serde::{Deserialize, Serialize};

use crate::core::stt::base::STTConfig;

// =============================================================================
// AWS Regions
// =============================================================================

/// AWS regions where Amazon Transcribe Streaming is available.
///
/// Amazon Transcribe Streaming is available in most major AWS regions.
/// Select the region closest to your users for lowest latency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum AwsRegion {
    /// US East (N. Virginia)
    #[default]
    #[serde(rename = "us-east-1")]
    UsEast1,
    /// US East (Ohio)
    #[serde(rename = "us-east-2")]
    UsEast2,
    /// US West (N. California)
    #[serde(rename = "us-west-1")]
    UsWest1,
    /// US West (Oregon)
    #[serde(rename = "us-west-2")]
    UsWest2,
    /// Asia Pacific (Mumbai)
    #[serde(rename = "ap-south-1")]
    ApSouth1,
    /// Asia Pacific (Singapore)
    #[serde(rename = "ap-southeast-1")]
    ApSoutheast1,
    /// Asia Pacific (Sydney)
    #[serde(rename = "ap-southeast-2")]
    ApSoutheast2,
    /// Asia Pacific (Tokyo)
    #[serde(rename = "ap-northeast-1")]
    ApNortheast1,
    /// Asia Pacific (Seoul)
    #[serde(rename = "ap-northeast-2")]
    ApNortheast2,
    /// Canada (Central)
    #[serde(rename = "ca-central-1")]
    CaCentral1,
    /// Europe (Frankfurt)
    #[serde(rename = "eu-central-1")]
    EuCentral1,
    /// Europe (Ireland)
    #[serde(rename = "eu-west-1")]
    EuWest1,
    /// Europe (London)
    #[serde(rename = "eu-west-2")]
    EuWest2,
    /// Europe (Paris)
    #[serde(rename = "eu-west-3")]
    EuWest3,
    /// South America (Sao Paulo)
    #[serde(rename = "sa-east-1")]
    SaEast1,
    /// AWS GovCloud (US-West)
    #[serde(rename = "us-gov-west-1")]
    UsGovWest1,
}

impl AwsRegion {
    /// Convert to AWS region string.
    #[inline]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::UsEast1 => "us-east-1",
            Self::UsEast2 => "us-east-2",
            Self::UsWest1 => "us-west-1",
            Self::UsWest2 => "us-west-2",
            Self::ApSouth1 => "ap-south-1",
            Self::ApSoutheast1 => "ap-southeast-1",
            Self::ApSoutheast2 => "ap-southeast-2",
            Self::ApNortheast1 => "ap-northeast-1",
            Self::ApNortheast2 => "ap-northeast-2",
            Self::CaCentral1 => "ca-central-1",
            Self::EuCentral1 => "eu-central-1",
            Self::EuWest1 => "eu-west-1",
            Self::EuWest2 => "eu-west-2",
            Self::EuWest3 => "eu-west-3",
            Self::SaEast1 => "sa-east-1",
            Self::UsGovWest1 => "us-gov-west-1",
        }
    }

    /// Parse from string, with fallback to default (us-east-1).
    pub fn from_str_or_default(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "us-east-1" => Self::UsEast1,
            "us-east-2" => Self::UsEast2,
            "us-west-1" => Self::UsWest1,
            "us-west-2" => Self::UsWest2,
            "ap-south-1" => Self::ApSouth1,
            "ap-southeast-1" => Self::ApSoutheast1,
            "ap-southeast-2" => Self::ApSoutheast2,
            "ap-northeast-1" => Self::ApNortheast1,
            "ap-northeast-2" => Self::ApNortheast2,
            "ca-central-1" => Self::CaCentral1,
            "eu-central-1" => Self::EuCentral1,
            "eu-west-1" => Self::EuWest1,
            "eu-west-2" => Self::EuWest2,
            "eu-west-3" => Self::EuWest3,
            "sa-east-1" => Self::SaEast1,
            "us-gov-west-1" => Self::UsGovWest1,
            _ => Self::default(),
        }
    }

    /// Get all available regions.
    pub fn all() -> &'static [AwsRegion] {
        &[
            Self::UsEast1,
            Self::UsEast2,
            Self::UsWest1,
            Self::UsWest2,
            Self::ApSouth1,
            Self::ApSoutheast1,
            Self::ApSoutheast2,
            Self::ApNortheast1,
            Self::ApNortheast2,
            Self::CaCentral1,
            Self::EuCentral1,
            Self::EuWest1,
            Self::EuWest2,
            Self::EuWest3,
            Self::SaEast1,
            Self::UsGovWest1,
        ]
    }
}

impl std::fmt::Display for AwsRegion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =============================================================================
// Media Encoding
// =============================================================================

/// Supported audio encoding formats for Amazon Transcribe Streaming.
///
/// # Recommendations
///
/// - **PCM**: Best for real-time streaming, lowest latency
/// - **FLAC**: Lossless compression, slightly higher latency
/// - **OPUS**: Good compression with low latency (in OGG container)
///
/// # Audio Requirements
///
/// - PCM: 16-bit signed little-endian, mono
/// - Sample rate: 8,000 Hz to 48,000 Hz (16,000 Hz recommended)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum MediaEncoding {
    /// PCM 16-bit signed little-endian (recommended for lowest latency)
    #[default]
    #[serde(rename = "pcm")]
    Pcm,
    /// FLAC lossless compression
    #[serde(rename = "flac")]
    Flac,
    /// OPUS encoded audio in OGG container
    #[serde(rename = "ogg-opus")]
    OggOpus,
}

impl MediaEncoding {
    /// Convert to AWS API string.
    #[inline]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pcm => "pcm",
            Self::Flac => "flac",
            Self::OggOpus => "ogg-opus",
        }
    }

    /// Parse from string, with fallback to PCM.
    pub fn from_str_or_default(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "pcm" | "linear16" | "pcm_s16le" => Self::Pcm,
            "flac" => Self::Flac,
            "ogg-opus" | "opus" | "ogg_opus" => Self::OggOpus,
            _ => Self::default(),
        }
    }
}

impl std::fmt::Display for MediaEncoding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =============================================================================
// Partial Results Stability
// =============================================================================

/// Partial results stability level for Amazon Transcribe Streaming.
///
/// Controls the trade-off between latency and accuracy for interim results:
///
/// - **High**: Fastest transcription, lowest accuracy. Best for live subtitles.
/// - **Medium**: Balanced latency and accuracy.
/// - **Low**: Highest accuracy, higher latency. Best for content moderation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PartialResultsStability {
    /// High stability - fastest, slight accuracy reduction
    #[default]
    #[serde(rename = "high")]
    High,
    /// Medium stability - balanced
    #[serde(rename = "medium")]
    Medium,
    /// Low stability - highest accuracy, higher latency
    #[serde(rename = "low")]
    Low,
}

impl PartialResultsStability {
    /// Convert to AWS API string.
    #[inline]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }

    /// Parse from string, with fallback to High.
    pub fn from_str_or_default(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "high" => Self::High,
            "medium" | "med" => Self::Medium,
            "low" => Self::Low,
            _ => Self::default(),
        }
    }
}

impl std::fmt::Display for PartialResultsStability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =============================================================================
// Vocabulary Filter Method
// =============================================================================

/// Method to apply vocabulary filters in Amazon Transcribe.
///
/// Controls how filtered words are handled in the transcript:
/// - **Remove**: Filtered words are removed entirely
/// - **Mask**: Filtered words are replaced with `***`
/// - **Tag**: Filtered words are tagged with `[PII]` or similar
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum VocabularyFilterMethod {
    /// Remove filtered words from transcript
    #[default]
    #[serde(rename = "remove")]
    Remove,
    /// Replace filtered words with asterisks
    #[serde(rename = "mask")]
    Mask,
    /// Tag filtered words
    #[serde(rename = "tag")]
    Tag,
}

impl VocabularyFilterMethod {
    /// Convert to AWS API string.
    #[inline]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Remove => "remove",
            Self::Mask => "mask",
            Self::Tag => "tag",
        }
    }

    /// Parse from string, with fallback to Remove.
    pub fn from_str_or_default(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "remove" => Self::Remove,
            "mask" => Self::Mask,
            "tag" => Self::Tag,
            _ => Self::default(),
        }
    }
}

impl std::fmt::Display for VocabularyFilterMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =============================================================================
// Content Redaction Type
// =============================================================================

/// Types of content that can be redacted by Amazon Transcribe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContentRedactionType {
    /// Redact personally identifiable information (PII)
    #[serde(rename = "PII")]
    Pii,
}

impl ContentRedactionType {
    /// Convert to AWS API string.
    #[inline]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pii => "PII",
        }
    }
}

impl std::fmt::Display for ContentRedactionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =============================================================================
// Main Configuration
// =============================================================================

/// Minimum supported sample rate (Hz)
pub const MIN_SAMPLE_RATE: u32 = 8000;

/// Maximum supported sample rate (Hz)
pub const MAX_SAMPLE_RATE: u32 = 48000;

/// Recommended sample rate for best quality/latency balance (Hz)
pub const RECOMMENDED_SAMPLE_RATE: u32 = 16000;

/// Default audio chunk duration in milliseconds (50-200ms recommended)
pub const DEFAULT_CHUNK_DURATION_MS: u32 = 100;

/// Configuration specific to Amazon Transcribe Streaming STT.
///
/// This configuration extends the base STT configuration with
/// Amazon Transcribe-specific options.
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
/// - Use 16kHz sample rate for best accuracy
/// - Enable partial results stabilization for live applications
/// - Use PCM encoding for lowest latency
/// - Send audio chunks of 50-200ms duration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AwsTranscribeSTTConfig {
    /// Base STT configuration
    #[serde(flatten)]
    pub base: STTConfig,

    /// AWS region for the Transcribe service
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

    /// Audio encoding format
    #[serde(default)]
    pub media_encoding: MediaEncoding,

    /// Enable partial results stabilization for lower latency
    ///
    /// When enabled, only the last few words of interim results may change,
    /// reducing visual churn in live captions.
    #[serde(default = "default_true")]
    pub enable_partial_results_stabilization: bool,

    /// Stability level for partial results
    #[serde(default)]
    pub partial_results_stability: PartialResultsStability,

    /// Enable speaker identification (diarization)
    ///
    /// Requires `max_speaker_labels` to be set.
    #[serde(default)]
    pub show_speaker_label: bool,

    /// Maximum number of speakers for diarization (2-10)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_speaker_labels: Option<u8>,

    /// Enable channel identification for multi-channel audio
    #[serde(default)]
    pub enable_channel_identification: bool,

    /// Number of audio channels (1-2)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number_of_channels: Option<u8>,

    /// Custom vocabulary name for improved recognition
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vocabulary_name: Option<String>,

    /// Custom vocabulary filter name for content filtering
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vocabulary_filter_name: Option<String>,

    /// Method to apply vocabulary filter
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vocabulary_filter_method: Option<VocabularyFilterMethod>,

    /// Custom language model name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language_model_name: Option<String>,

    /// Enable automatic language identification
    ///
    /// Cannot be used with `language_code` set to a specific language.
    #[serde(default)]
    pub identify_language: bool,

    /// Preferred languages for automatic language identification
    ///
    /// Only used when `identify_language` is true.
    #[serde(default)]
    pub preferred_language: Vec<String>,

    /// Enable content redaction (e.g., PII masking)
    #[serde(default)]
    pub enable_content_redaction: bool,

    /// Types of content to redact
    #[serde(default)]
    pub content_redaction_types: Vec<ContentRedactionType>,

    /// PII entity types to redact (e.g., "NAME", "PHONE", "EMAIL")
    #[serde(default)]
    pub pii_entity_types: Vec<String>,

    /// Session ID for tracking transcription sessions
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,

    /// Audio chunk duration in milliseconds (50-200 recommended)
    #[serde(default = "default_chunk_duration")]
    pub chunk_duration_ms: u32,
}

fn default_true() -> bool {
    true
}

fn default_chunk_duration() -> u32 {
    DEFAULT_CHUNK_DURATION_MS
}

impl Default for AwsTranscribeSTTConfig {
    fn default() -> Self {
        Self {
            base: STTConfig {
                provider: "aws-transcribe".to_string(),
                api_key: String::new(), // Not used, AWS uses access keys
                language: "en-US".to_string(),
                sample_rate: RECOMMENDED_SAMPLE_RATE,
                channels: 1,
                punctuation: true,
                encoding: "pcm".to_string(),
                model: String::new(), // Amazon Transcribe uses default model
            },
            region: AwsRegion::default(),
            aws_access_key_id: None,
            aws_secret_access_key: None,
            aws_session_token: None,
            media_encoding: MediaEncoding::default(),
            enable_partial_results_stabilization: true,
            partial_results_stability: PartialResultsStability::default(),
            show_speaker_label: false,
            max_speaker_labels: None,
            enable_channel_identification: false,
            number_of_channels: None,
            vocabulary_name: None,
            vocabulary_filter_name: None,
            vocabulary_filter_method: None,
            language_model_name: None,
            identify_language: false,
            preferred_language: Vec::new(),
            enable_content_redaction: false,
            content_redaction_types: Vec::new(),
            pii_entity_types: Vec::new(),
            session_id: None,
            chunk_duration_ms: DEFAULT_CHUNK_DURATION_MS,
        }
    }
}

impl AwsTranscribeSTTConfig {
    /// Create a new configuration with the given language code.
    pub fn with_language(language_code: &str) -> Self {
        let mut config = Self::default();
        config.base.language = language_code.to_string();
        config
    }

    /// Validate the configuration.
    pub fn validate(&self) -> Result<(), String> {
        // Validate sample rate
        if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&self.base.sample_rate) {
            return Err(format!(
                "Sample rate must be between {} and {} Hz, got {}",
                MIN_SAMPLE_RATE, MAX_SAMPLE_RATE, self.base.sample_rate
            ));
        }

        // Validate speaker labels
        if self.show_speaker_label {
            if let Some(max_speakers) = self.max_speaker_labels {
                if !(2..=10).contains(&max_speakers) {
                    return Err(format!(
                        "max_speaker_labels must be between 2 and 10, got {}",
                        max_speakers
                    ));
                }
            } else {
                return Err(
                    "max_speaker_labels is required when show_speaker_label is true".to_string(),
                );
            }
        }

        // Validate channel identification
        if self.enable_channel_identification
            && let Some(num_channels) = self.number_of_channels
            && !(1..=2).contains(&num_channels)
        {
            return Err(format!(
                "number_of_channels must be 1 or 2, got {}",
                num_channels
            ));
        }

        // Validate chunk duration
        if !(50..=200).contains(&self.chunk_duration_ms) {
            return Err(format!(
                "chunk_duration_ms should be between 50 and 200 ms for optimal performance, got {}",
                self.chunk_duration_ms
            ));
        }

        // Validate language vs identify_language
        if self.identify_language && !self.base.language.is_empty() {
            tracing::warn!(
                "Both identify_language and language_code are set. \
                 identify_language will be used and language_code will be ignored."
            );
        }

        Ok(())
    }

    /// Calculate the optimal chunk size in bytes based on configuration.
    ///
    /// Formula: chunk_size = (duration_ms / 1000) * sample_rate * bytes_per_sample * channels
    ///
    /// For PCM 16-bit mono: bytes_per_sample = 2
    pub fn calculate_chunk_size(&self) -> usize {
        let bytes_per_sample = match self.media_encoding {
            MediaEncoding::Pcm => 2, // 16-bit = 2 bytes
            MediaEncoding::Flac | MediaEncoding::OggOpus => {
                // For compressed formats, estimate based on uncompressed size
                // FLAC typically achieves 50-60% compression, OPUS is variable
                2
            }
        };

        let duration_secs = self.chunk_duration_ms as f64 / 1000.0;
        let channels = self.base.channels as usize;

        (duration_secs * self.base.sample_rate as f64 * bytes_per_sample as f64 * channels as f64)
            as usize
    }

    /// Check if explicit AWS credentials are provided.
    pub fn has_explicit_credentials(&self) -> bool {
        self.aws_access_key_id.is_some() && self.aws_secret_access_key.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_aws_region_as_str() {
        assert_eq!(AwsRegion::UsEast1.as_str(), "us-east-1");
        assert_eq!(AwsRegion::EuWest1.as_str(), "eu-west-1");
        assert_eq!(AwsRegion::ApNortheast1.as_str(), "ap-northeast-1");
    }

    #[test]
    fn test_aws_region_from_str() {
        assert_eq!(
            AwsRegion::from_str_or_default("us-west-2"),
            AwsRegion::UsWest2
        );
        assert_eq!(
            AwsRegion::from_str_or_default("EU-CENTRAL-1"),
            AwsRegion::EuCentral1
        );
        assert_eq!(
            AwsRegion::from_str_or_default("invalid"),
            AwsRegion::UsEast1
        );
    }

    #[test]
    fn test_media_encoding_as_str() {
        assert_eq!(MediaEncoding::Pcm.as_str(), "pcm");
        assert_eq!(MediaEncoding::Flac.as_str(), "flac");
        assert_eq!(MediaEncoding::OggOpus.as_str(), "ogg-opus");
    }

    #[test]
    fn test_media_encoding_from_str() {
        assert_eq!(
            MediaEncoding::from_str_or_default("pcm"),
            MediaEncoding::Pcm
        );
        assert_eq!(
            MediaEncoding::from_str_or_default("linear16"),
            MediaEncoding::Pcm
        );
        assert_eq!(
            MediaEncoding::from_str_or_default("opus"),
            MediaEncoding::OggOpus
        );
        assert_eq!(
            MediaEncoding::from_str_or_default("unknown"),
            MediaEncoding::Pcm
        );
    }

    #[test]
    fn test_partial_results_stability() {
        assert_eq!(PartialResultsStability::High.as_str(), "high");
        assert_eq!(
            PartialResultsStability::from_str_or_default("medium"),
            PartialResultsStability::Medium
        );
        assert_eq!(
            PartialResultsStability::from_str_or_default("invalid"),
            PartialResultsStability::High
        );
    }

    #[test]
    fn test_config_default_values() {
        let config = AwsTranscribeSTTConfig::default();
        assert_eq!(config.base.sample_rate, 16000);
        assert_eq!(config.region, AwsRegion::UsEast1);
        assert_eq!(config.media_encoding, MediaEncoding::Pcm);
        assert!(config.enable_partial_results_stabilization);
        assert_eq!(
            config.partial_results_stability,
            PartialResultsStability::High
        );
        assert!(!config.show_speaker_label);
        assert_eq!(config.chunk_duration_ms, 100);
    }

    #[test]
    fn test_config_validation_valid() {
        let config = AwsTranscribeSTTConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validation_invalid_sample_rate() {
        let mut config = AwsTranscribeSTTConfig::default();
        config.base.sample_rate = 4000; // Too low

        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Sample rate"));
    }

    #[test]
    fn test_config_validation_speaker_labels() {
        let mut config = AwsTranscribeSTTConfig::default();
        config.show_speaker_label = true;

        // Missing max_speaker_labels should fail
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("max_speaker_labels"));

        // Invalid max_speaker_labels should fail
        config.max_speaker_labels = Some(15);
        let result = config.validate();
        assert!(result.is_err());

        // Valid max_speaker_labels should pass
        config.max_speaker_labels = Some(5);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_calculate_chunk_size() {
        let config = AwsTranscribeSTTConfig::default();
        // 100ms at 16kHz mono 16-bit = 3200 bytes
        assert_eq!(config.calculate_chunk_size(), 3200);

        // 50ms at 16kHz mono 16-bit = 1600 bytes
        let mut config = AwsTranscribeSTTConfig::default();
        config.chunk_duration_ms = 50;
        assert_eq!(config.calculate_chunk_size(), 1600);

        // 100ms at 48kHz mono 16-bit = 9600 bytes
        let mut config = AwsTranscribeSTTConfig::default();
        config.base.sample_rate = 48000;
        assert_eq!(config.calculate_chunk_size(), 9600);
    }

    #[test]
    fn test_has_explicit_credentials() {
        let mut config = AwsTranscribeSTTConfig::default();
        assert!(!config.has_explicit_credentials());

        config.aws_access_key_id = Some("AKIAIOSFODNN7EXAMPLE".to_string());
        assert!(!config.has_explicit_credentials());

        config.aws_secret_access_key = Some("wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".to_string());
        assert!(config.has_explicit_credentials());
    }
}
