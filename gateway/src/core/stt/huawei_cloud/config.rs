//! Huawei Cloud Speech Interaction Service (SIS) STT Configuration
//!
//! Configuration types for Huawei Cloud Speech-to-Text service.

use crate::core::stt::base::{STTConfig, STTError};
use serde::{Deserialize, Serialize};

// =============================================================================
// Constants
// =============================================================================

/// IAM token endpoint base URL (must be combined with region).
pub const HUAWEI_IAM_BASE_URL: &str = "https://iam.{region}.myhuaweicloud.com/v3/auth/tokens";

/// SIS endpoint format for China regions.
pub const SIS_CHINA_ENDPOINT_FORMAT: &str = "sis.{region}.myhuaweicloud.com";

/// SIS endpoint format for international regions.
pub const SIS_INTL_ENDPOINT_FORMAT: &str = "sis-ext.{region}.myhuaweicloud.com";

/// Short audio recognition REST API path.
pub const SHORT_ASR_PATH: &str = "/v1/{project_id}/asr/short-audio";

/// Real-time ASR WebSocket path (streaming mode, < 1 min).
pub const REALTIME_ASR_STREAMING_PATH: &str = "/v1/{project_id}/rasr/short-stream";

/// Real-time ASR WebSocket path (continuous mode, < 5 hours).
pub const REALTIME_ASR_CONTINUOUS_PATH: &str = "/v1/{project_id}/rasr/continue-stream";

/// Default recognition model (Mandarin 16k general).
pub const DEFAULT_MODEL: &str = "chinese_16k_general";

/// Default sample rate.
pub const DEFAULT_SAMPLE_RATE: u32 = 16000;

/// Default audio format.
pub const DEFAULT_AUDIO_FORMAT: &str = "pcm16k16bit";

/// Token validity period in seconds (24 hours).
pub const TOKEN_VALIDITY_SECS: u64 = 86400;

/// Maximum audio duration for short audio API (30 seconds).
pub const MAX_SHORT_AUDIO_DURATION_SECS: u32 = 30;

/// Maximum audio duration for streaming mode (60 seconds).
pub const MAX_STREAMING_DURATION_SECS: u32 = 60;

/// Maximum audio duration for continuous mode (5 hours).
pub const MAX_CONTINUOUS_DURATION_SECS: u32 = 18000;

/// Recommended chunk duration for WebSocket streaming (200ms).
pub const RECOMMENDED_CHUNK_DURATION_MS: u32 = 200;

/// Maximum time between audio frames before timeout (10 seconds).
pub const MAX_FRAME_INTERVAL_SECS: u32 = 10;

/// WebSocket connection timeout (5 hours).
pub const WS_TIMEOUT_SECS: u64 = 18000;

// =============================================================================
// Region Configuration
// =============================================================================

/// Huawei Cloud regions supporting SIS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum HuaweiCloudRegion {
    /// CN North - Beijing 4 (primary China region with premium voices).
    #[default]
    CnNorth4,

    /// CN East - Shanghai 2 (premium voices available).
    CnEast3,

    /// AP Southeast - Singapore.
    ApSoutheast3,

    /// AP Southeast - Hong Kong.
    ApSoutheast1,

    /// AP Southeast - Bangkok.
    ApSoutheast2,
}

impl HuaweiCloudRegion {
    /// Get the region code string.
    pub fn code(&self) -> &'static str {
        match self {
            Self::CnNorth4 => "cn-north-4",
            Self::CnEast3 => "cn-east-3",
            Self::ApSoutheast3 => "ap-southeast-3",
            Self::ApSoutheast1 => "ap-southeast-1",
            Self::ApSoutheast2 => "ap-southeast-2",
        }
    }

    /// Parse a region from string.
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().replace('_', "-").as_str() {
            "cn-north-4" | "beijing" | "beijing4" => Some(Self::CnNorth4),
            "cn-east-3" | "shanghai" | "shanghai2" => Some(Self::CnEast3),
            "ap-southeast-3" | "singapore" => Some(Self::ApSoutheast3),
            "ap-southeast-1" | "hongkong" | "hong-kong" => Some(Self::ApSoutheast1),
            "ap-southeast-2" | "bangkok" => Some(Self::ApSoutheast2),
            _ => None,
        }
    }

    /// Check if this is a China region.
    pub fn is_china_region(&self) -> bool {
        matches!(self, Self::CnNorth4 | Self::CnEast3)
    }

    /// Check if this region supports premium voices.
    pub fn supports_premium_voices(&self) -> bool {
        matches!(self, Self::CnNorth4 | Self::CnEast3)
    }

    /// Get the SIS endpoint for this region.
    pub fn sis_endpoint(&self) -> String {
        if self.is_china_region() {
            SIS_CHINA_ENDPOINT_FORMAT.replace("{region}", self.code())
        } else {
            SIS_INTL_ENDPOINT_FORMAT.replace("{region}", self.code())
        }
    }

    /// Get the IAM endpoint for this region.
    pub fn iam_endpoint(&self) -> String {
        HUAWEI_IAM_BASE_URL.replace("{region}", self.code())
    }

    /// Get list of all regions.
    pub fn all() -> Vec<Self> {
        vec![
            Self::CnNorth4,
            Self::CnEast3,
            Self::ApSoutheast3,
            Self::ApSoutheast1,
            Self::ApSoutheast2,
        ]
    }
}

// =============================================================================
// Recognition Models
// =============================================================================

/// Huawei Cloud ASR recognition models (property).
///
/// Format: `{language}_{sample_rate}_{domain}`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum HuaweiCloudSttModel {
    /// Mandarin Chinese, 16kHz, General domain.
    #[default]
    Chinese16kGeneral,

    /// Mandarin Chinese, 8kHz, General domain.
    Chinese8kGeneral,

    /// Mandarin Chinese, 16kHz, Common domain.
    Chinese16kCommon,

    /// Mandarin Chinese, 8kHz, Common domain.
    Chinese8kCommon,

    /// Cantonese (粤语), 16kHz, General domain.
    Cantonese16kGeneral,

    /// Sichuanese (四川话), 16kHz, General domain.
    Sichuan16kGeneral,

    /// Hokkienese/Minnan (闽南语), 16kHz, General domain.
    Minnan16kGeneral,

    /// Mongolian (蒙古语), 16kHz, General domain.
    Mongolian16kGeneral,

    /// Tibetan (藏语), 16kHz, General domain.
    Tibetan16kGeneral,

    /// Uyghur (维吾尔语), 16kHz, General domain.
    Uyghur16kGeneral,
}

impl HuaweiCloudSttModel {
    /// Get the property string for API requests.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Chinese16kGeneral => "chinese_16k_general",
            Self::Chinese8kGeneral => "chinese_8k_general",
            Self::Chinese16kCommon => "chinese_16k_common",
            Self::Chinese8kCommon => "chinese_8k_common",
            Self::Cantonese16kGeneral => "cantonese_16k_general",
            Self::Sichuan16kGeneral => "sichuan_16k_general",
            Self::Minnan16kGeneral => "minnan_16k_general",
            Self::Mongolian16kGeneral => "mongolian_16k_general",
            Self::Tibetan16kGeneral => "tibetan_16k_general",
            Self::Uyghur16kGeneral => "uyghur_16k_general",
        }
    }

    /// Parse a model from string.
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().replace('-', "_").as_str() {
            "chinese_16k_general" | "mandarin" | "zh" | "zh-cn" | "chinese" => {
                Self::Chinese16kGeneral
            }
            "chinese_8k_general" | "chinese_8k" => Self::Chinese8kGeneral,
            "chinese_16k_common" | "chinese_common" => Self::Chinese16kCommon,
            "chinese_8k_common" => Self::Chinese8kCommon,
            "cantonese_16k_general" | "cantonese" | "yue" | "zh-yue" => Self::Cantonese16kGeneral,
            "sichuan_16k_general" | "sichuan" | "sichuanese" | "zh-sichuan" => {
                Self::Sichuan16kGeneral
            }
            "minnan_16k_general" | "minnan" | "hokkien" | "hokkienese" | "zh-minnan" => {
                Self::Minnan16kGeneral
            }
            "mongolian_16k_general" | "mongolian" | "mn" => Self::Mongolian16kGeneral,
            "tibetan_16k_general" | "tibetan" | "bo" => Self::Tibetan16kGeneral,
            "uyghur_16k_general" | "uyghur" | "ug" => Self::Uyghur16kGeneral,
            _ => Self::Chinese16kGeneral,
        }
    }

    /// Get the sample rate for this model.
    pub fn sample_rate(&self) -> u32 {
        match self {
            Self::Chinese8kGeneral | Self::Chinese8kCommon => 8000,
            _ => 16000,
        }
    }

    /// Get the language code for this model.
    pub fn language_code(&self) -> &'static str {
        match self {
            Self::Chinese16kGeneral
            | Self::Chinese8kGeneral
            | Self::Chinese16kCommon
            | Self::Chinese8kCommon => "zh-CN",
            Self::Cantonese16kGeneral => "zh-HK",
            Self::Sichuan16kGeneral => "zh-SC",
            Self::Minnan16kGeneral => "zh-MN",
            Self::Mongolian16kGeneral => "mn",
            Self::Tibetan16kGeneral => "bo",
            Self::Uyghur16kGeneral => "ug",
        }
    }

    /// Check if this model supports custom vocabulary (hotwords).
    pub fn supports_vocabulary(&self) -> bool {
        matches!(
            self,
            Self::Chinese16kGeneral
                | Self::Chinese8kGeneral
                | Self::Chinese16kCommon
                | Self::Chinese8kCommon
        )
    }

    /// Get all available models.
    pub fn all() -> Vec<Self> {
        vec![
            Self::Chinese16kGeneral,
            Self::Chinese8kGeneral,
            Self::Chinese16kCommon,
            Self::Chinese8kCommon,
            Self::Cantonese16kGeneral,
            Self::Sichuan16kGeneral,
            Self::Minnan16kGeneral,
            Self::Mongolian16kGeneral,
            Self::Tibetan16kGeneral,
            Self::Uyghur16kGeneral,
        ]
    }
}

// =============================================================================
// Audio Format
// =============================================================================

/// Supported audio formats for Huawei Cloud STT.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum HuaweiCloudAudioFormat {
    /// PCM, 8kHz sampling, 16-bit.
    Pcm8k16bit,

    /// PCM, 16kHz sampling, 16-bit.
    #[default]
    Pcm16k16bit,

    /// WAV format.
    Wav,

    /// AMR format.
    Amr,

    /// AMR-WB format.
    AmrWb,

    /// MP3 format.
    Mp3,

    /// AAC format.
    Aac,

    /// OGG Opus format.
    OggOpus,

    /// M4A format.
    M4a,
}

impl HuaweiCloudAudioFormat {
    /// Get the format string for API requests.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pcm8k16bit => "pcm8k16bit",
            Self::Pcm16k16bit => "pcm16k16bit",
            Self::Wav => "wav",
            Self::Amr => "amr",
            Self::AmrWb => "amr-wb",
            Self::Mp3 => "mp3",
            Self::Aac => "aac",
            Self::OggOpus => "ogg-opus",
            Self::M4a => "m4a",
        }
    }

    /// Parse a format from string.
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().replace('_', "-").as_str() {
            "pcm8k16bit" | "pcm8k" | "pcm-8k" => Some(Self::Pcm8k16bit),
            "pcm16k16bit" | "pcm16k" | "pcm-16k" | "pcm" | "linear16" => Some(Self::Pcm16k16bit),
            "wav" => Some(Self::Wav),
            "amr" => Some(Self::Amr),
            "amr-wb" | "amrwb" => Some(Self::AmrWb),
            "mp3" => Some(Self::Mp3),
            "aac" => Some(Self::Aac),
            "ogg-opus" | "opus" | "ogg" => Some(Self::OggOpus),
            "m4a" => Some(Self::M4a),
            _ => None,
        }
    }

    /// Get the sample rate associated with this format.
    pub fn sample_rate(&self) -> u32 {
        match self {
            Self::Pcm8k16bit | Self::Amr => 8000,
            _ => 16000,
        }
    }

    /// Check if this is a PCM format (uncompressed).
    pub fn is_pcm(&self) -> bool {
        matches!(self, Self::Pcm8k16bit | Self::Pcm16k16bit)
    }

    /// Check if this format is supported for streaming.
    pub fn supports_streaming(&self) -> bool {
        matches!(
            self,
            Self::Pcm8k16bit | Self::Pcm16k16bit | Self::Wav | Self::Mp3
        )
    }
}

// =============================================================================
// Operation Mode
// =============================================================================

/// ASR operation mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum HuaweiCloudAsrMode {
    /// Short sentence recognition via HTTP REST API (max 30 seconds).
    ShortSentence,

    /// Real-time streaming via WebSocket (max 1 minute).
    #[default]
    Streaming,

    /// Continuous recognition via WebSocket (max 5 hours).
    Continuous,
}

impl HuaweiCloudAsrMode {
    /// Get the WebSocket path for this mode.
    pub fn ws_path(&self) -> Option<&'static str> {
        match self {
            Self::ShortSentence => None,
            Self::Streaming => Some(REALTIME_ASR_STREAMING_PATH),
            Self::Continuous => Some(REALTIME_ASR_CONTINUOUS_PATH),
        }
    }

    /// Get the maximum duration for this mode in seconds.
    pub fn max_duration_secs(&self) -> u32 {
        match self {
            Self::ShortSentence => MAX_SHORT_AUDIO_DURATION_SECS,
            Self::Streaming => MAX_STREAMING_DURATION_SECS,
            Self::Continuous => MAX_CONTINUOUS_DURATION_SECS,
        }
    }

    /// Check if this mode uses WebSocket.
    pub fn is_websocket(&self) -> bool {
        !matches!(self, Self::ShortSentence)
    }
}

// =============================================================================
// Configuration
// =============================================================================

/// Huawei Cloud SIS STT configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HuaweiCloudSttConfig {
    /// IAM username (for token authentication).
    pub username: String,

    /// IAM password (for token authentication).
    pub password: String,

    /// IAM domain name (tenant).
    pub domain_name: String,

    /// Project ID (obtained from IAM console).
    pub project_id: String,

    /// Region for SIS service.
    pub region: HuaweiCloudRegion,

    /// Recognition model/property.
    pub model: HuaweiCloudSttModel,

    /// Audio format.
    pub audio_format: HuaweiCloudAudioFormat,

    /// Operation mode (short sentence, streaming, or continuous).
    pub mode: HuaweiCloudAsrMode,

    /// Add punctuation to results.
    pub add_punctuation: bool,

    /// Normalize digits (convert to Arabic numerals).
    pub digit_norm: bool,

    /// Custom vocabulary/hotword table ID.
    pub vocabulary_id: Option<String>,

    /// Include word-level timing information.
    pub need_word_info: bool,

    /// Cached IAM token.
    #[serde(skip)]
    pub token: Option<String>,

    /// Token expiration timestamp.
    #[serde(skip)]
    pub token_expires_at: Option<u64>,
}

impl Default for HuaweiCloudSttConfig {
    fn default() -> Self {
        Self {
            username: String::new(),
            password: String::new(),
            domain_name: String::new(),
            project_id: String::new(),
            region: HuaweiCloudRegion::default(),
            model: HuaweiCloudSttModel::default(),
            audio_format: HuaweiCloudAudioFormat::default(),
            mode: HuaweiCloudAsrMode::default(),
            add_punctuation: true,
            digit_norm: true,
            vocabulary_id: None,
            need_word_info: false,
            token: None,
            token_expires_at: None,
        }
    }
}

impl HuaweiCloudSttConfig {
    /// Create a new configuration with credentials.
    pub fn new(
        username: impl Into<String>,
        password: impl Into<String>,
        domain_name: impl Into<String>,
        project_id: impl Into<String>,
    ) -> Self {
        Self {
            username: username.into(),
            password: password.into(),
            domain_name: domain_name.into(),
            project_id: project_id.into(),
            ..Default::default()
        }
    }

    /// Set the region.
    pub fn with_region(mut self, region: HuaweiCloudRegion) -> Self {
        self.region = region;
        self
    }

    /// Set the recognition model.
    pub fn with_model(mut self, model: HuaweiCloudSttModel) -> Self {
        self.model = model;
        // Update audio format to match model sample rate
        if model.sample_rate() == 8000 {
            self.audio_format = HuaweiCloudAudioFormat::Pcm8k16bit;
        } else {
            self.audio_format = HuaweiCloudAudioFormat::Pcm16k16bit;
        }
        self
    }

    /// Set the audio format.
    pub fn with_audio_format(mut self, format: HuaweiCloudAudioFormat) -> Self {
        self.audio_format = format;
        self
    }

    /// Set the operation mode.
    pub fn with_mode(mut self, mode: HuaweiCloudAsrMode) -> Self {
        self.mode = mode;
        self
    }

    /// Enable/disable punctuation.
    pub fn with_punctuation(mut self, enabled: bool) -> Self {
        self.add_punctuation = enabled;
        self
    }

    /// Enable/disable digit normalization.
    pub fn with_digit_norm(mut self, enabled: bool) -> Self {
        self.digit_norm = enabled;
        self
    }

    /// Set custom vocabulary ID.
    pub fn with_vocabulary_id(mut self, id: impl Into<String>) -> Self {
        self.vocabulary_id = Some(id.into());
        self
    }

    /// Enable word-level timing information.
    pub fn with_word_info(mut self, enabled: bool) -> Self {
        self.need_word_info = enabled;
        self
    }

    /// Validate the configuration.
    pub fn validate(&self) -> Result<(), STTError> {
        if self.username.is_empty() {
            return Err(STTError::ConfigurationError(
                "Huawei Cloud username is required".to_string(),
            ));
        }

        if self.password.is_empty() {
            return Err(STTError::ConfigurationError(
                "Huawei Cloud password is required".to_string(),
            ));
        }

        if self.domain_name.is_empty() {
            return Err(STTError::ConfigurationError(
                "Huawei Cloud domain name is required".to_string(),
            ));
        }

        if self.project_id.is_empty() {
            return Err(STTError::ConfigurationError(
                "Huawei Cloud project ID is required".to_string(),
            ));
        }

        // Validate sample rate matches model
        let expected_rate = self.model.sample_rate();
        let format_rate = self.audio_format.sample_rate();
        if expected_rate != format_rate && self.audio_format.is_pcm() {
            return Err(STTError::ConfigurationError(format!(
                "Audio format sample rate ({}) doesn't match model ({} requires {}kHz)",
                format_rate,
                self.model.as_str(),
                expected_rate / 1000
            )));
        }

        // Validate vocabulary support
        if self.vocabulary_id.is_some() && !self.model.supports_vocabulary() {
            return Err(STTError::ConfigurationError(format!(
                "Model {} does not support custom vocabulary",
                self.model.as_str()
            )));
        }

        // Validate streaming format support
        if self.mode.is_websocket() && !self.audio_format.supports_streaming() {
            return Err(STTError::ConfigurationError(format!(
                "Audio format {} does not support {} mode",
                self.audio_format.as_str(),
                if matches!(self.mode, HuaweiCloudAsrMode::Streaming) {
                    "streaming"
                } else {
                    "continuous"
                }
            )));
        }

        Ok(())
    }

    /// Get the SIS HTTPS endpoint for this configuration.
    pub fn get_https_endpoint(&self) -> String {
        format!("https://{}", self.region.sis_endpoint())
    }

    /// Get the SIS WebSocket endpoint for this configuration.
    pub fn get_wss_endpoint(&self) -> String {
        format!("wss://{}", self.region.sis_endpoint())
    }

    /// Get the short sentence recognition URL.
    pub fn get_short_asr_url(&self) -> String {
        format!(
            "{}{}",
            self.get_https_endpoint(),
            SHORT_ASR_PATH.replace("{project_id}", &self.project_id)
        )
    }

    /// Get the WebSocket URL for real-time recognition.
    pub fn get_realtime_url(&self) -> Option<String> {
        self.mode.ws_path().map(|path| {
            format!(
                "{}{}",
                self.get_wss_endpoint(),
                path.replace("{project_id}", &self.project_id)
            )
        })
    }

    /// Get the IAM token URL.
    pub fn get_iam_url(&self) -> String {
        self.region.iam_endpoint()
    }

    /// Calculate the recommended chunk size for WebSocket streaming.
    /// Formula: sample_rate * 2 (bytes per sample) * duration_ms / 1000
    pub fn get_chunk_size(&self) -> usize {
        let sample_rate = self.model.sample_rate() as usize;
        (sample_rate * 2 * RECOMMENDED_CHUNK_DURATION_MS as usize) / 1000
    }

    /// Check if the cached token is valid.
    pub fn is_token_valid(&self) -> bool {
        match (self.token.as_ref(), self.token_expires_at) {
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
        vec!["zh-CN", "zh-HK", "zh-SC", "zh-MN", "mn", "bo", "ug"]
    }

    /// Get list of supported models.
    pub fn supported_models() -> Vec<&'static str> {
        HuaweiCloudSttModel::all()
            .iter()
            .map(|m| m.as_str())
            .collect()
    }

    /// Create from base STT config.
    ///
    /// API key format: `username|password|domain_name|project_id`
    /// or for AK/SK: `ak|sk|project_id` (region in model field suffix)
    pub fn from_base(config: STTConfig) -> Result<Self, STTError> {
        let parts: Vec<&str> = config.api_key.split('|').collect();

        if parts.len() < 4 {
            return Err(STTError::ConfigurationError(
                "API key must be in format: username|password|domain_name|project_id".to_string(),
            ));
        }

        let username = parts[0].to_string();
        let password = parts[1].to_string();
        let domain_name = parts[2].to_string();
        let project_id = parts[3].to_string();

        // Parse region from model string suffix (e.g., "chinese_16k_general@cn-north-4")
        let (model_str, region) = if config.model.contains('@') {
            let model_parts: Vec<&str> = config.model.splitn(2, '@').collect();
            let region =
                HuaweiCloudRegion::from_str(model_parts[1]).unwrap_or(HuaweiCloudRegion::default());
            (model_parts[0], region)
        } else {
            (config.model.as_str(), HuaweiCloudRegion::default())
        };

        let model = HuaweiCloudSttModel::from_str(model_str);
        let audio_format = HuaweiCloudAudioFormat::from_str(&config.encoding)
            .unwrap_or(HuaweiCloudAudioFormat::Pcm16k16bit);

        let huawei_config = Self {
            username,
            password,
            domain_name,
            project_id,
            region,
            model,
            audio_format,
            ..Default::default()
        };

        huawei_config.validate()?;
        Ok(huawei_config)
    }

    /// Build from the standardized config (W1 keystone). Huawei Cloud SIS exposes a narrow set of
    /// recognition knobs, so this maps the standardized features whose meaning matches an existing
    /// Huawei field: word-level timing (`need_word_info`) and smart formatting, which Huawei
    /// expresses as automatic punctuation (`add_punctuation`). Features Huawei can't express
    /// (diarization, profanity_filter, interim_results, vad/endpointing, keyterms — Huawei requires
    /// a pre-registered vocabulary table ID rather than inline terms, redaction, entity/language
    /// detection) are capability gaps and stay at their defaults.
    pub fn from_standard(
        std: &crate::core::stt::standard::StandardSTTConfig,
    ) -> Result<Self, STTError> {
        let f = &std.features;
        let mut cfg = Self::from_base(std.base.clone())?;
        if let Some(w) = f.word_timestamps {
            cfg.need_word_info = w;
        }
        if let Some(s) = f.smart_format {
            cfg.add_punctuation = s;
        }
        Ok(cfg)
    }
}

// =============================================================================
// IAM Token Response
// =============================================================================

/// IAM authentication token response.
#[derive(Debug, Clone, Deserialize)]
pub struct HuaweiIamTokenResponse {
    /// Token information.
    pub token: HuaweiIamToken,
}

/// IAM token details.
#[derive(Debug, Clone, Deserialize)]
pub struct HuaweiIamToken {
    /// Token expiration time (ISO 8601 format).
    pub expires_at: String,

    /// Token issued time.
    pub issued_at: Option<String>,

    /// Project information.
    pub project: Option<HuaweiIamProject>,

    /// User information.
    pub user: Option<HuaweiIamUser>,
}

/// IAM project information.
#[derive(Debug, Clone, Deserialize)]
pub struct HuaweiIamProject {
    /// Project ID.
    pub id: String,

    /// Project name.
    pub name: String,
}

/// IAM user information.
#[derive(Debug, Clone, Deserialize)]
pub struct HuaweiIamUser {
    /// User ID.
    pub id: String,

    /// User name.
    pub name: String,

    /// Domain information.
    pub domain: Option<HuaweiIamDomain>,
}

/// IAM domain information.
#[derive(Debug, Clone, Deserialize)]
pub struct HuaweiIamDomain {
    /// Domain ID.
    pub id: String,

    /// Domain name.
    pub name: String,
}

/// IAM authentication error response.
#[derive(Debug, Clone, Deserialize)]
pub struct HuaweiIamError {
    /// Error code.
    pub error_code: Option<String>,

    /// Error message.
    pub error_msg: Option<String>,
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // W1 keystone: the standardized features unlock Huawei's recognition knobs
    // (word-level timing + smart formatting/punctuation) — previously unreachable via the flat
    // factory.
    #[test]
    fn from_standard_maps_features() {
        use crate::core::stt::standard::{ProviderExtras, SttFeatures, StandardSTTConfig};
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "huawei_cloud".into(),
                api_key: "user|pass|domain|project123".into(),
                model: "chinese_16k_general".into(),
                ..Default::default()
            },
            features: SttFeatures {
                word_timestamps: Some(true),
                smart_format: Some(false),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
        };
        let cfg = HuaweiCloudSttConfig::from_standard(&std).unwrap();
        assert!(cfg.need_word_info); // word_timestamps
        assert!(!cfg.add_punctuation); // smart_format
    }

    #[test]
    fn test_region_code() {
        assert_eq!(HuaweiCloudRegion::CnNorth4.code(), "cn-north-4");
        assert_eq!(HuaweiCloudRegion::CnEast3.code(), "cn-east-3");
        assert_eq!(HuaweiCloudRegion::ApSoutheast3.code(), "ap-southeast-3");
    }

    #[test]
    fn test_region_from_str() {
        assert_eq!(
            HuaweiCloudRegion::from_str("cn-north-4"),
            Some(HuaweiCloudRegion::CnNorth4)
        );
        assert_eq!(
            HuaweiCloudRegion::from_str("beijing"),
            Some(HuaweiCloudRegion::CnNorth4)
        );
        assert_eq!(
            HuaweiCloudRegion::from_str("singapore"),
            Some(HuaweiCloudRegion::ApSoutheast3)
        );
        assert_eq!(HuaweiCloudRegion::from_str("invalid"), None);
    }

    #[test]
    fn test_region_is_china() {
        assert!(HuaweiCloudRegion::CnNorth4.is_china_region());
        assert!(HuaweiCloudRegion::CnEast3.is_china_region());
        assert!(!HuaweiCloudRegion::ApSoutheast3.is_china_region());
    }

    #[test]
    fn test_region_sis_endpoint() {
        let cn_endpoint = HuaweiCloudRegion::CnNorth4.sis_endpoint();
        assert_eq!(cn_endpoint, "sis.cn-north-4.myhuaweicloud.com");

        let intl_endpoint = HuaweiCloudRegion::ApSoutheast3.sis_endpoint();
        assert_eq!(intl_endpoint, "sis-ext.ap-southeast-3.myhuaweicloud.com");
    }

    #[test]
    fn test_model_from_str() {
        assert_eq!(
            HuaweiCloudSttModel::from_str("chinese_16k_general"),
            HuaweiCloudSttModel::Chinese16kGeneral
        );
        assert_eq!(
            HuaweiCloudSttModel::from_str("mandarin"),
            HuaweiCloudSttModel::Chinese16kGeneral
        );
        assert_eq!(
            HuaweiCloudSttModel::from_str("cantonese"),
            HuaweiCloudSttModel::Cantonese16kGeneral
        );
        assert_eq!(
            HuaweiCloudSttModel::from_str("sichuan"),
            HuaweiCloudSttModel::Sichuan16kGeneral
        );
    }

    #[test]
    fn test_model_sample_rate() {
        assert_eq!(HuaweiCloudSttModel::Chinese16kGeneral.sample_rate(), 16000);
        assert_eq!(HuaweiCloudSttModel::Chinese8kGeneral.sample_rate(), 8000);
        assert_eq!(
            HuaweiCloudSttModel::Cantonese16kGeneral.sample_rate(),
            16000
        );
    }

    #[test]
    fn test_model_vocabulary_support() {
        assert!(HuaweiCloudSttModel::Chinese16kGeneral.supports_vocabulary());
        assert!(HuaweiCloudSttModel::Chinese8kGeneral.supports_vocabulary());
        assert!(!HuaweiCloudSttModel::Cantonese16kGeneral.supports_vocabulary());
        assert!(!HuaweiCloudSttModel::Sichuan16kGeneral.supports_vocabulary());
    }

    #[test]
    fn test_audio_format_from_str() {
        assert_eq!(
            HuaweiCloudAudioFormat::from_str("pcm16k16bit"),
            Some(HuaweiCloudAudioFormat::Pcm16k16bit)
        );
        assert_eq!(
            HuaweiCloudAudioFormat::from_str("pcm"),
            Some(HuaweiCloudAudioFormat::Pcm16k16bit)
        );
        assert_eq!(
            HuaweiCloudAudioFormat::from_str("wav"),
            Some(HuaweiCloudAudioFormat::Wav)
        );
        assert_eq!(
            HuaweiCloudAudioFormat::from_str("ogg-opus"),
            Some(HuaweiCloudAudioFormat::OggOpus)
        );
        assert_eq!(HuaweiCloudAudioFormat::from_str("invalid"), None);
    }

    #[test]
    fn test_audio_format_streaming_support() {
        assert!(HuaweiCloudAudioFormat::Pcm16k16bit.supports_streaming());
        assert!(HuaweiCloudAudioFormat::Wav.supports_streaming());
        assert!(!HuaweiCloudAudioFormat::Aac.supports_streaming());
        assert!(!HuaweiCloudAudioFormat::M4a.supports_streaming());
    }

    #[test]
    fn test_asr_mode_max_duration() {
        assert_eq!(HuaweiCloudAsrMode::ShortSentence.max_duration_secs(), 30);
        assert_eq!(HuaweiCloudAsrMode::Streaming.max_duration_secs(), 60);
        assert_eq!(HuaweiCloudAsrMode::Continuous.max_duration_secs(), 18000);
    }

    #[test]
    fn test_asr_mode_websocket() {
        assert!(!HuaweiCloudAsrMode::ShortSentence.is_websocket());
        assert!(HuaweiCloudAsrMode::Streaming.is_websocket());
        assert!(HuaweiCloudAsrMode::Continuous.is_websocket());
    }

    #[test]
    fn test_config_validation_valid() {
        let config = HuaweiCloudSttConfig::new("user", "pass", "domain", "project_id");
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validation_empty_username() {
        let config = HuaweiCloudSttConfig::new("", "pass", "domain", "project_id");
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_empty_password() {
        let config = HuaweiCloudSttConfig::new("user", "", "domain", "project_id");
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_sample_rate_mismatch() {
        let mut config = HuaweiCloudSttConfig::new("user", "pass", "domain", "project_id");
        config.model = HuaweiCloudSttModel::Chinese8kGeneral;
        config.audio_format = HuaweiCloudAudioFormat::Pcm16k16bit;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_vocabulary_unsupported() {
        let mut config = HuaweiCloudSttConfig::new("user", "pass", "domain", "project_id");
        config.model = HuaweiCloudSttModel::Cantonese16kGeneral;
        config.vocabulary_id = Some("vocab123".to_string());
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_get_urls() {
        let config = HuaweiCloudSttConfig::new("user", "pass", "domain", "project123")
            .with_region(HuaweiCloudRegion::CnNorth4);

        assert_eq!(
            config.get_https_endpoint(),
            "https://sis.cn-north-4.myhuaweicloud.com"
        );
        assert_eq!(
            config.get_short_asr_url(),
            "https://sis.cn-north-4.myhuaweicloud.com/v1/project123/asr/short-audio"
        );
    }

    #[test]
    fn test_config_get_realtime_url() {
        let config = HuaweiCloudSttConfig::new("user", "pass", "domain", "project123")
            .with_region(HuaweiCloudRegion::CnNorth4)
            .with_mode(HuaweiCloudAsrMode::Streaming);

        let url = config.get_realtime_url();
        assert!(url.is_some());
        assert!(url.unwrap().contains("rasr/short-stream"));
    }

    #[test]
    fn test_config_get_chunk_size() {
        let config = HuaweiCloudSttConfig::new("user", "pass", "domain", "project_id")
            .with_model(HuaweiCloudSttModel::Chinese16kGeneral);

        // 200ms at 16kHz, 16-bit: 16000 * 2 * 200 / 1000 = 6400 bytes
        assert_eq!(config.get_chunk_size(), 6400);

        let config_8k = HuaweiCloudSttConfig::new("user", "pass", "domain", "project_id")
            .with_model(HuaweiCloudSttModel::Chinese8kGeneral);

        // 200ms at 8kHz, 16-bit: 8000 * 2 * 200 / 1000 = 3200 bytes
        assert_eq!(config_8k.get_chunk_size(), 3200);
    }

    #[test]
    fn test_config_token_validity() {
        let mut config = HuaweiCloudSttConfig::new("user", "pass", "domain", "project_id");

        // No token
        assert!(!config.is_token_valid());

        // Set valid token
        config.token = Some("token123".to_string());
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        config.token_expires_at = Some(now + 7200); // 2 hours from now
        assert!(config.is_token_valid());

        // Set expired token
        config.token_expires_at = Some(now - 100);
        assert!(!config.is_token_valid());
    }

    #[test]
    fn test_from_base_config() {
        let base = STTConfig {
            api_key: "user|pass|domain|project123".to_string(),
            language: "zh".to_string(),
            sample_rate: 16000,
            encoding: "pcm16k16bit".to_string(),
            model: "chinese_16k_general".to_string(),
            ..Default::default()
        };

        let config = HuaweiCloudSttConfig::from_base(base).unwrap();
        assert_eq!(config.username, "user");
        assert_eq!(config.password, "pass");
        assert_eq!(config.domain_name, "domain");
        assert_eq!(config.project_id, "project123");
        assert_eq!(config.model, HuaweiCloudSttModel::Chinese16kGeneral);
    }

    #[test]
    fn test_from_base_config_with_region() {
        let base = STTConfig {
            api_key: "user|pass|domain|project123".to_string(),
            model: "chinese_16k_general@cn-east-3".to_string(),
            ..Default::default()
        };

        let config = HuaweiCloudSttConfig::from_base(base).unwrap();
        assert_eq!(config.region, HuaweiCloudRegion::CnEast3);
    }

    #[test]
    fn test_from_base_config_invalid_format() {
        let base = STTConfig {
            api_key: "invalid_format".to_string(),
            ..Default::default()
        };

        assert!(HuaweiCloudSttConfig::from_base(base).is_err());
    }

    #[test]
    fn test_supported_languages() {
        let languages = HuaweiCloudSttConfig::supported_languages();
        assert!(languages.contains(&"zh-CN"));
        assert!(languages.contains(&"zh-HK"));
        assert!(languages.contains(&"mn"));
    }

    #[test]
    fn test_supported_models() {
        let models = HuaweiCloudSttConfig::supported_models();
        assert!(models.contains(&"chinese_16k_general"));
        assert!(models.contains(&"cantonese_16k_general"));
        assert!(models.len() >= 10);
    }
}
