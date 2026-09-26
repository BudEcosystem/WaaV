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
//!     region: AwsRegion::parse("eu-north-1")?,
//!     language_code: "en-US".to_string(),
//!     media_encoding: MediaEncoding::Pcm,
//!     sample_rate: 16000,
//!     enable_partial_results_stabilization: true,
//!     partial_results_stability: PartialResultsStability::High,
//!     ..Default::default()
//! };
//! ```

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use crate::core::stt::base::STTConfig;

fn validate_aws_transcribe_endpoint(source: &str, endpoint: &str) -> Result<(), String> {
    let endpoint = endpoint.trim();
    if endpoint.is_empty() {
        return Ok(());
    }
    crate::core::net::validate_url_for_ssrf(endpoint, crate::core::net::HTTP_URL_SCHEMES)
        .map_err(|msg| format!("{source} rejected (SSRF protection): {msg}"))
}

// =============================================================================
// AWS Regions
// =============================================================================

/// An AWS region name — validated for shape, NOT limited to a fixed list.
///
/// Shared by Amazon Transcribe Streaming and Amazon Polly. The region is where the audio is
/// processed and stored in transit, so it is a data-residency choice and must reach the SDK
/// exactly as named. This used to be a closed enum of 16 regions whose parser mapped every other
/// name — `eu-north-1`, `ap-northeast-3`, `af-south-1`, `me-south-1`, `eu-central-2`, … — to
/// `us-east-1`, silently sending, say, a Stockholm tenant's audio to Virginia. AWS opens regions
/// every year and the SDK needs no table to reach one (`Region::new(name)` resolves the regional
/// endpoint), so any name of the AWS shape `^[a-z]{2}(-gov)?-[a-z]+-\d+$` is accepted and passed
/// through verbatim; anything else is a configuration error — never a fallback region. Whether
/// the service actually runs in that region is AWS's to say, and it says so on the request.
///
/// The field is private so every value went through [`AwsRegion::parse`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct AwsRegion(Cow<'static, str>);

impl AwsRegion {
    /// US East (N. Virginia): the region used when none is configured anywhere.
    pub const US_EAST_1: Self = Self(Cow::Borrowed("us-east-1"));

    /// Parse a region name. Case-insensitive and whitespace-trimmed (`EU-NORTH-1` → `eu-north-1`);
    /// a name that is not AWS-shaped is an error naming it.
    pub fn parse(name: &str) -> Result<Self, String> {
        let normalized = name.trim().to_ascii_lowercase();
        if is_aws_region_name(&normalized) {
            Ok(Self(Cow::Owned(normalized)))
        } else {
            Err(format!(
                "invalid AWS region {:?}: expected a region name such as us-east-1, eu-north-1 \
                 or us-gov-west-1",
                name.trim()
            ))
        }
    }

    /// Read the `region` provider extra. Absent, `null` or blank means "not chosen" (`Ok(None)`)
    /// so the caller's fallback applies; a string is parsed; any other JSON type is an error.
    pub fn from_extra(value: Option<&serde_json::Value>) -> Result<Option<Self>, String> {
        match value {
            None | Some(serde_json::Value::Null) => Ok(None),
            Some(serde_json::Value::String(s)) if s.trim().is_empty() => Ok(None),
            Some(serde_json::Value::String(s)) => Self::parse(s).map(Some),
            Some(other) => Err(format!(
                "invalid AWS region: `region` must be a string, got {other}"
            )),
        }
    }

    /// The region name as sent to AWS.
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The SDK region — the name passed through unchanged, which is all the SDK needs to resolve
    /// the regional endpoint.
    pub fn to_sdk(&self) -> aws_config::Region {
        aws_config::Region::new(self.0.clone())
    }
}

/// `^[a-z]{2}(-gov)?-[a-z]+-\d+$`, without pulling a regex into a config type: two-letter area,
/// an optional `gov` partition marker, a lowercase direction/locality word, a number.
fn is_aws_region_name(name: &str) -> bool {
    fn word(p: &str) -> bool {
        !p.is_empty() && p.bytes().all(|b| b.is_ascii_lowercase())
    }
    fn number(p: &str) -> bool {
        !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())
    }
    let parts: Vec<&str> = name.split('-').collect();
    match parts.as_slice() {
        [area, locality, n] | [area, "gov", locality, n] => {
            area.len() == 2 && word(area) && word(locality) && number(n)
        }
        _ => false,
    }
}

impl Default for AwsRegion {
    fn default() -> Self {
        Self::US_EAST_1
    }
}

impl TryFrom<String> for AwsRegion {
    type Error = String;

    fn try_from(name: String) -> Result<Self, Self::Error> {
        Self::parse(&name)
    }
}

impl From<AwsRegion> for String {
    fn from(region: AwsRegion) -> Self {
        region.0.into_owned()
    }
}

impl std::str::FromStr for AwsRegion {
    type Err = String;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        Self::parse(name)
    }
}

impl std::fmt::Display for AwsRegion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
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

/// Refuse a PARTIAL set of explicit AWS credentials.
///
/// Explicit credentials are used only when both the access key id and the secret are present;
/// otherwise the client falls back to the SDK default chain — the gateway's OWN identity
/// (environment, profile, instance role). A request that carried a key id without its secret, or a
/// session token with no keys, meant to authenticate as someone else; quietly running it as the
/// gateway is the wrong-identity outcome, so it is a configuration error instead. The message
/// names the fields, never their values. Shared with Amazon Polly.
pub(crate) fn validate_explicit_credentials(
    access_key_id: &Option<String>,
    secret_access_key: &Option<String>,
    session_token: &Option<String>,
) -> Result<(), String> {
    let (key, secret) = (access_key_id.is_some(), secret_access_key.is_some());
    if key && !secret {
        return Err(
            "aws_access_key_id was given without aws_secret_access_key; explicit AWS \
                    credentials need both"
                .to_string(),
        );
    }
    if secret && !key {
        return Err(
            "aws_secret_access_key was given without aws_access_key_id; explicit AWS \
                    credentials need both"
                .to_string(),
        );
    }
    if session_token.is_some() && !key {
        return Err(
            "aws_session_token was given without aws_access_key_id and aws_secret_access_key"
                .to_string(),
        );
    }
    Ok(())
}

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

    /// Candidate language list for language identification (`LanguageOptions`,
    /// wire header `x-amzn-transcribe-language-options`). A comma-separated list of
    /// language codes the identifier should choose between. Carried via `ProviderExtras`
    /// (`language_options`). Only meaningful when `identify_language` /
    /// `identify_multiple_languages` is on.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub language_options: Vec<String>,

    /// Multi-language (code-switching) identification (`IdentifyMultipleLanguages`,
    /// wire header `x-amzn-transcribe-identify-multiple-languages`). When true, Transcribe
    /// detects multiple languages within a single stream. Carried via `ProviderExtras`
    /// (`identify_multiple_languages`).
    #[serde(default)]
    pub identify_multiple_languages: bool,

    /// Custom vocabulary names for language-ID mode (`VocabularyNames`, wire header
    /// `x-amzn-transcribe-vocabulary-names`). A comma-separated list of custom vocabularies,
    /// one per candidate language. Distinct from the single-language `vocabulary_name`.
    /// Carried via `ProviderExtras` (`vocabulary_names`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vocabulary_names: Option<String>,

    /// Custom vocabulary filter names for language-ID mode (`VocabularyFilterNames`, wire header
    /// `x-amzn-transcribe-vocabulary-filter-names`). Comma-separated, one per candidate language.
    /// Distinct from the single-language `vocabulary_filter_name`. Carried via `ProviderExtras`
    /// (`vocabulary_filter_names`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vocabulary_filter_names: Option<String>,

    /// Session resume window in minutes (`SessionResumeWindow`, wire header
    /// `x-amzn-transcribe-session-resume-window`). How long a dropped session may be resumed.
    /// Carried via `ProviderExtras` (`session_resume_window`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_resume_window: Option<i32>,

    /// PII content identification — FLAG (not redact) mode (`ContentIdentificationType`,
    /// wire header `x-amzn-transcribe-content-identification-type`). Tags PII in the
    /// transcript instead of masking it (distinct from `enable_content_redaction`). The only
    /// accepted value is `PII`. Carried via `ProviderExtras` (`content_identification_type`).
    #[serde(default)]
    pub enable_content_identification: bool,

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

    /// Override the AWS Transcribe Streaming endpoint base URL (e.g. a localhost mock for e2e
    /// tests). When set, the AWS SDK config loader is pointed at this URL via `.endpoint_url(..)`
    /// instead of the resolved regional endpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint_override: Option<String>,
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
            language_options: Vec::new(),
            identify_multiple_languages: false,
            vocabulary_names: None,
            vocabulary_filter_names: None,
            session_resume_window: None,
            enable_content_identification: false,
            enable_content_redaction: false,
            content_redaction_types: Vec::new(),
            pii_entity_types: Vec::new(),
            session_id: None,
            chunk_duration_ms: DEFAULT_CHUNK_DURATION_MS,
            endpoint_override: None,
        }
    }
}

impl AwsTranscribeSTTConfig {
    /// Build from the standardized config (W1 keystone — 4th provider). Unlocks the diarization
    /// AND content-redaction that the flat factory hardcoded off (BRUTAL_REVIEW.md flagged both),
    /// plus partial-results stabilization, through the standardized API.
    ///
    /// Fails on a malformed `region` extra (never a fallback region — see [`AwsRegion`]) and on
    /// a partial set of credential extras (see `validate_explicit_credentials`).
    pub fn from_standard(
        std: &crate::core::stt::standard::StandardSTTConfig,
    ) -> Result<Self, String> {
        let f = &std.features;
        let ex = &std.extras.0;
        let mut cfg = Self {
            base: std.base.clone(),
            ..Default::default()
        };
        if let Some(d) = f.diarization {
            cfg.show_speaker_label = d;
            if d && cfg.max_speaker_labels.is_none() {
                cfg.max_speaker_labels = Some(10); // AWS max
            }
        }
        if let Some(i) = f.interim_results {
            cfg.enable_partial_results_stabilization = i;
        }
        if let Some(r) = &f.redaction {
            cfg.enable_content_redaction = true;
            cfg.pii_entity_types = r.clone();
        }

        // --- Newly-wired StartStreamTranscription features (all provider-specific → extras) ----
        // These have no canonical typed `SttFeatures` field, so they ride the open
        // `ProviderExtras` passthrough. Each maps to a documented `x-amzn-transcribe-*` request
        // header serialized by the aws-sdk-transcribestreaming `StartStreamTranscriptionInput`
        // (confirmed against the SDK protocol_serde header serializer, June 2026).

        // `language_options` (x-amzn-transcribe-language-options): comma-separated string OR JSON
        // array of language codes. Accept either form so callers can pass a list or a string.
        cfg.language_options = match ex.get("language_options") {
            Some(serde_json::Value::String(s)) => s
                .split(',')
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty())
                .collect(),
            Some(serde_json::Value::Array(a)) => a
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect(),
            _ => Vec::new(),
        };

        // `identify_multiple_languages` (x-amzn-transcribe-identify-multiple-languages).
        cfg.identify_multiple_languages = ex
            .get("identify_multiple_languages")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // `vocabulary_names` (x-amzn-transcribe-vocabulary-names): comma-separated list for
        // language-ID mode (distinct from the single-language `vocabulary_name`).
        cfg.vocabulary_names = ex
            .get("vocabulary_names")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        // `vocabulary_filter_names` (x-amzn-transcribe-vocabulary-filter-names): language-ID mode.
        cfg.vocabulary_filter_names = ex
            .get("vocabulary_filter_names")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        // `session_resume_window` (x-amzn-transcribe-session-resume-window): minutes.
        cfg.session_resume_window = ex
            .get("session_resume_window")
            .and_then(|v| v.as_i64())
            .map(|n| n as i32);

        // `content_identification_type` (x-amzn-transcribe-content-identification-type): PII FLAG
        // mode (not redaction). Accept a bool toggle or the literal "PII" string.
        cfg.enable_content_identification = match ex.get("content_identification_type") {
            Some(serde_json::Value::Bool(b)) => *b,
            Some(serde_json::Value::String(s)) => s.eq_ignore_ascii_case("pii"),
            _ => false,
        };

        // Endpoint override (mock harness): point the AWS SDK Transcribe client at a localhost mock.
        cfg.endpoint_override = std.endpoint_override().map(|s| s.to_string());
        // AWS credentials + region flow through the standardized path via the `extras` passthrough;
        // without this the standard path could never authenticate against an explicit-credential
        // endpoint (the explicit-credential branch in `start_connection` is otherwise unreachable
        // from `from_standard`). Mirrors aws_polly TTS.
        if let Some(k) = ex.get("aws_access_key_id").and_then(|v| v.as_str()) {
            cfg.aws_access_key_id = Some(k.to_string());
        }
        if let Some(k) = ex.get("aws_secret_access_key").and_then(|v| v.as_str()) {
            cfg.aws_secret_access_key = Some(k.to_string());
        }
        if let Some(k) = ex.get("aws_session_token").and_then(|v| v.as_str()) {
            cfg.aws_session_token = Some(k.to_string());
        }
        validate_explicit_credentials(
            &cfg.aws_access_key_id,
            &cfg.aws_secret_access_key,
            &cfg.aws_session_token,
        )?;
        // The region the caller chose, passed through; a malformed one is refused rather than
        // replaced with us-east-1 (see `AwsRegion`). Unset keeps the default for `new_standard`
        // to fill from the gateway's own AWS_REGION.
        if let Some(r) = AwsRegion::from_extra(ex.get("region"))? {
            cfg.region = r;
        }

        Ok(cfg)
    }

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

        if let Some(endpoint) = self.endpoint_override.as_deref() {
            validate_aws_transcribe_endpoint("endpoint_override", endpoint)?;
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

    // W1 keystone (4th provider): unlocks AWS diarization + content redaction — both hardcoded
    // off by the flat factory per BRUTAL_REVIEW.md.
    #[test]
    fn from_standard_unlocks_aws_diarization_and_redaction() {
        use crate::core::stt::standard::{StandardSTTConfig, SttFeatures};
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "aws-transcribe".into(),
                api_key: "k".into(),
                ..Default::default()
            },
            features: SttFeatures {
                diarization: Some(true),
                redaction: Some(vec!["NAME".into(), "PHONE".into()]),
                ..Default::default()
            },
            extras: Default::default(),
            translation: None,
        };
        let cfg = AwsTranscribeSTTConfig::from_standard(&std).unwrap();
        assert!(cfg.show_speaker_label);
        assert_eq!(cfg.max_speaker_labels, Some(10));
        assert!(cfg.enable_content_redaction);
        assert_eq!(cfg.pii_entity_types, vec!["NAME", "PHONE"]);
    }

    #[test]
    fn test_aws_region_as_str() {
        assert_eq!(AwsRegion::US_EAST_1.as_str(), "us-east-1");
        assert_eq!(AwsRegion::default(), AwsRegion::US_EAST_1);
        assert_eq!(
            AwsRegion::parse("ap-northeast-1").unwrap().as_str(),
            "ap-northeast-1"
        );
        assert_eq!(
            AwsRegion::parse("eu-west-1").unwrap().to_string(),
            "eu-west-1"
        );
    }

    #[test]
    fn test_aws_region_from_str() {
        assert_eq!(AwsRegion::parse("us-west-2").unwrap().as_str(), "us-west-2");
        // Case and surrounding whitespace are normalised to AWS's spelling.
        assert_eq!(
            AwsRegion::parse(" EU-CENTRAL-1 ").unwrap().as_str(),
            "eu-central-1"
        );
        // A malformed name is an error — it used to become us-east-1.
        assert!(AwsRegion::parse("invalid").is_err());
    }

    /// Regions outside the old 16-entry enum pass through verbatim. Each of these used to parse to
    /// us-east-1, sending the audio to a region the caller never chose (data residency).
    #[test]
    fn any_aws_shaped_region_is_accepted_and_passed_through() {
        for name in [
            "eu-north-1",
            "ap-northeast-3",
            "af-south-1",
            "me-south-1",
            "me-central-1",
            "eu-central-2",
            "eu-south-2",
            "ap-southeast-5",
            "ca-west-1",
            "il-central-1",
            "mx-central-1",
            "us-gov-west-1",
            "us-gov-east-1",
            "cn-north-1",
        ] {
            let region = AwsRegion::parse(name).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(region.as_str(), name);
            assert_eq!(region.to_sdk().as_ref(), name, "SDK region for {name}");
            assert_ne!(region, AwsRegion::US_EAST_1, "{name}");
        }
    }

    /// Anything not of the shape `^[a-z]{2}(-gov)?-[a-z]+-\d+$` is refused, naming the value.
    #[test]
    fn malformed_region_is_an_error_never_us_east_1() {
        for bad in [
            "",
            "   ",
            "invalid",
            "us-east",
            "useast1",
            "us_east_1",
            "u-east-1",
            "usa-east-1",
            "us-east-1a",
            "us-east-",
            "us--east-1",
            "us-iso-east-1",
            "eu-north-1/../x",
            "Europe (Stockholm)",
        ] {
            let err = AwsRegion::parse(bad).expect_err(bad);
            assert!(err.contains("invalid AWS region"), "{bad}: {err}");
        }
    }

    #[test]
    fn region_serde_round_trips_and_rejects_malformed_names() {
        let region: AwsRegion = serde_json::from_str(r#""eu-north-1""#).unwrap();
        assert_eq!(region.as_str(), "eu-north-1");
        assert_eq!(serde_json::to_string(&region).unwrap(), r#""eu-north-1""#);
        assert!(serde_json::from_str::<AwsRegion>(r#""narnia""#).is_err());
    }

    #[test]
    fn region_extra_blank_is_unset_and_non_string_is_an_error() {
        assert_eq!(AwsRegion::from_extra(None), Ok(None));
        assert_eq!(
            AwsRegion::from_extra(Some(&serde_json::json!(null))),
            Ok(None)
        );
        assert_eq!(
            AwsRegion::from_extra(Some(&serde_json::json!("  "))),
            Ok(None)
        );
        assert_eq!(
            AwsRegion::from_extra(Some(&serde_json::json!("eu-north-1"))),
            Ok(Some(AwsRegion::parse("eu-north-1").unwrap()))
        );
        assert!(AwsRegion::from_extra(Some(&serde_json::json!(1))).is_err());
        assert!(AwsRegion::from_extra(Some(&serde_json::json!("narnia"))).is_err());
    }

    /// The `region` extra reaches the config verbatim; a malformed one fails construction instead
    /// of being replaced with us-east-1.
    #[test]
    fn from_standard_passes_the_region_extra_through_or_refuses_it() {
        use crate::core::stt::standard::{ProviderExtras, StandardSTTConfig};
        let with_region = |region: serde_json::Value| {
            let mut extras = serde_json::Map::new();
            extras.insert("region".into(), region);
            StandardSTTConfig {
                extras: ProviderExtras(extras),
                ..StandardSTTConfig::from_base(STTConfig {
                    provider: "aws-transcribe".into(),
                    ..Default::default()
                })
            }
        };
        let cfg =
            AwsTranscribeSTTConfig::from_standard(&with_region(serde_json::json!("eu-north-1")))
                .unwrap();
        assert_eq!(cfg.region.as_str(), "eu-north-1");

        let err = AwsTranscribeSTTConfig::from_standard(&with_region(serde_json::json!("narnia")))
            .expect_err("a malformed region must not become us-east-1");
        assert!(err.contains("narnia"), "{err}");
    }

    /// A partial credential set must not fall through to the gateway's own identity.
    #[test]
    fn partial_explicit_credentials_are_refused_without_echoing_them() {
        const KEY: &str = "AKIAKEYVALUE";
        const SECRET: &str = "SECRETVALUE";
        const TOKEN: &str = "TOKENVALUE";
        let s = |v: &str| Some(v.to_string());
        assert!(validate_explicit_credentials(&None, &None, &None).is_ok());
        assert!(validate_explicit_credentials(&s(KEY), &s(SECRET), &None).is_ok());
        assert!(validate_explicit_credentials(&s(KEY), &s(SECRET), &s(TOKEN)).is_ok());
        for (key, secret, token) in [
            (s(KEY), None, None),
            (None, s(SECRET), None),
            (None, None, s(TOKEN)),
            (s(KEY), None, s(TOKEN)),
        ] {
            let err = validate_explicit_credentials(&key, &secret, &token).unwrap_err();
            assert!(
                !err.contains(KEY) && !err.contains(SECRET) && !err.contains(TOKEN),
                "credential values must not be echoed: {err}"
            );
        }
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
        assert_eq!(config.region, AwsRegion::US_EAST_1);
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
    fn test_config_validation_rejects_ssrf_endpoint_override() {
        let _guard = crate::core::net::test_env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let previous = std::env::var_os("WAAV_ALLOW_LOOPBACK_ENDPOINTS");
        // SAFETY: test-only env mutation, serialized by core::net::test_env_lock.
        unsafe { std::env::remove_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS") };

        let mut config = AwsTranscribeSTTConfig {
            endpoint_override: Some("https://transcribe-proxy.example.com".to_string()),
            ..Default::default()
        };
        assert!(config.validate().is_ok());

        config.endpoint_override = Some("http://127.0.0.1:9000".to_string());
        let err = config
            .validate()
            .expect_err("loopback endpoint_override must be rejected");
        assert!(err.contains("SSRF protection"), "{err}");

        config.endpoint_override = Some("file:///tmp/socket".to_string());
        let err = config
            .validate()
            .expect_err("non-HTTP endpoint_override must be rejected");
        assert!(err.contains("not allowed"), "{err}");

        config.endpoint_override = Some("ws://transcribe-proxy.example.com".to_string());
        let err = config
            .validate()
            .expect_err("WebSocket endpoint_override must be rejected for AWS SDK HTTP endpoint");
        assert!(err.contains("not allowed"), "{err}");

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
