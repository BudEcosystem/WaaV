//! Alibaba Cloud DashScope STT Configuration
//!
//! Configuration types for Alibaba Cloud's DashScope Model Studio STT API.
//!
//! # Supported Models
//!
//! - `qwen3-asr-flash-realtime`: Latest Qwen3 ASR for real-time streaming
//! - `paraformer-realtime-v2`: Multilingual real-time ASR
//! - `paraformer-realtime-8k-v2`: Telephony-optimized (8kHz)
//! - `fun-asr-realtime`: FunASR model
//!
//! # Regions
//!
//! - Beijing (China): `dashscope.aliyuncs.com`
//! - Singapore (International): `dashscope-intl.aliyuncs.com`

use serde::{Deserialize, Serialize};
use std::fmt;

use crate::core::stt::base::{STTConfig, STTError};

// =============================================================================
// Constants
// =============================================================================

/// DashScope Beijing (China) WebSocket endpoint for realtime API.
pub const DASHSCOPE_BEIJING_REALTIME_URL: &str = "wss://dashscope.aliyuncs.com/api-ws/v1/realtime";

/// DashScope Singapore (International) WebSocket endpoint for realtime API.
pub const DASHSCOPE_SINGAPORE_REALTIME_URL: &str = "wss://dashscope-intl.aliyuncs.com/api-ws/v1/realtime";

/// DashScope Beijing (China) WebSocket endpoint for inference API.
pub const DASHSCOPE_BEIJING_INFERENCE_URL: &str = "wss://dashscope.aliyuncs.com/api-ws/v1/inference";

/// DashScope Singapore (International) WebSocket endpoint for inference API.
pub const DASHSCOPE_SINGAPORE_INFERENCE_URL: &str = "wss://dashscope-intl.aliyuncs.com/api-ws/v1/inference";

/// Default STT model.
pub const DEFAULT_STT_MODEL: &str = "qwen3-asr-flash-realtime";

/// Default sample rate.
pub const DEFAULT_SAMPLE_RATE: u32 = 16000;

/// Default audio format.
pub const DEFAULT_AUDIO_FORMAT: &str = "pcm16";

/// Default silence duration for VAD (milliseconds).
pub const DEFAULT_SILENCE_DURATION_MS: u32 = 400;

// =============================================================================
// Region Enum
// =============================================================================

/// DashScope API region.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DashScopeRegion {
    /// Beijing region (China).
    #[default]
    Beijing,
    /// Singapore region (International).
    Singapore,
}

impl DashScopeRegion {
    /// Get the WebSocket URL for realtime API.
    pub fn realtime_url(&self) -> &'static str {
        match self {
            Self::Beijing => DASHSCOPE_BEIJING_REALTIME_URL,
            Self::Singapore => DASHSCOPE_SINGAPORE_REALTIME_URL,
        }
    }

    /// Get the WebSocket URL for inference API.
    pub fn inference_url(&self) -> &'static str {
        match self {
            Self::Beijing => DASHSCOPE_BEIJING_INFERENCE_URL,
            Self::Singapore => DASHSCOPE_SINGAPORE_INFERENCE_URL,
        }
    }

    /// Parse region from string.
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "beijing" | "cn" | "china" | "cn-beijing" => Self::Beijing,
            "singapore" | "sg" | "intl" | "international" | "ap-southeast-1" => Self::Singapore,
            _ => Self::Beijing, // Default to Beijing
        }
    }

    /// Get region code string.
    pub fn as_code(&self) -> &'static str {
        match self {
            Self::Beijing => "cn-beijing",
            Self::Singapore => "ap-southeast-1",
        }
    }
}

impl fmt::Display for DashScopeRegion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Beijing => write!(f, "Beijing"),
            Self::Singapore => write!(f, "Singapore"),
        }
    }
}

// =============================================================================
// STT Model Enum
// =============================================================================

/// Alibaba Cloud DashScope STT model.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DashScopeSttModel {
    /// Qwen3 ASR Flash Realtime - Latest and recommended.
    #[default]
    Qwen3AsrFlashRealtime,
    /// Qwen3 ASR Flash Realtime snapshot (2025-10-27).
    Qwen3AsrFlashRealtime20251027,
    /// Paraformer Realtime V2 - Multilingual support.
    ParaformerRealtimeV2,
    /// Paraformer Realtime V1.
    ParaformerRealtimeV1,
    /// Paraformer 8K V2 - Telephony optimized.
    Paraformer8kRealtimeV2,
    /// Paraformer 8K V1 - Telephony optimized (legacy).
    Paraformer8kRealtimeV1,
    /// FunASR Realtime.
    FunAsrRealtime,
    /// FunASR Realtime snapshot (2025-09-15).
    FunAsrRealtime20250915,
    /// Custom model ID.
    Custom(String),
}

impl DashScopeSttModel {
    /// Get the model ID string.
    pub fn as_model_id(&self) -> &str {
        match self {
            Self::Qwen3AsrFlashRealtime => "qwen3-asr-flash-realtime",
            Self::Qwen3AsrFlashRealtime20251027 => "qwen3-asr-flash-realtime-2025-10-27",
            Self::ParaformerRealtimeV2 => "paraformer-realtime-v2",
            Self::ParaformerRealtimeV1 => "paraformer-realtime-v1",
            Self::Paraformer8kRealtimeV2 => "paraformer-realtime-8k-v2",
            Self::Paraformer8kRealtimeV1 => "paraformer-realtime-8k-v1",
            Self::FunAsrRealtime => "fun-asr-realtime",
            Self::FunAsrRealtime20250915 => "fun-asr-realtime-2025-09-15",
            Self::Custom(id) => id.as_str(),
        }
    }

    /// Parse model from string.
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "qwen3-asr-flash-realtime" | "qwen3-asr" | "qwen-asr" => Self::Qwen3AsrFlashRealtime,
            "qwen3-asr-flash-realtime-2025-10-27" => Self::Qwen3AsrFlashRealtime20251027,
            "paraformer-realtime-v2" | "paraformer-v2" | "paraformer" => Self::ParaformerRealtimeV2,
            "paraformer-realtime-v1" | "paraformer-v1" => Self::ParaformerRealtimeV1,
            "paraformer-realtime-8k-v2" | "paraformer-8k-v2" | "paraformer-8k" => Self::Paraformer8kRealtimeV2,
            "paraformer-realtime-8k-v1" | "paraformer-8k-v1" => Self::Paraformer8kRealtimeV1,
            "fun-asr-realtime" | "fun-asr" | "funasr" => Self::FunAsrRealtime,
            "fun-asr-realtime-2025-09-15" => Self::FunAsrRealtime20250915,
            other => Self::Custom(other.to_string()),
        }
    }

    /// Check if this is a Qwen model (uses realtime API format).
    pub fn is_qwen_model(&self) -> bool {
        matches!(self, Self::Qwen3AsrFlashRealtime | Self::Qwen3AsrFlashRealtime20251027)
    }

    /// Check if this is a Paraformer model (uses inference API format).
    pub fn is_paraformer_model(&self) -> bool {
        matches!(
            self,
            Self::ParaformerRealtimeV2
                | Self::ParaformerRealtimeV1
                | Self::Paraformer8kRealtimeV2
                | Self::Paraformer8kRealtimeV1
        )
    }

    /// Check if this is an 8kHz model (telephony).
    pub fn is_8k_model(&self) -> bool {
        matches!(self, Self::Paraformer8kRealtimeV2 | Self::Paraformer8kRealtimeV1)
    }
}

impl fmt::Display for DashScopeSttModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_model_id())
    }
}

// =============================================================================
// Audio Format Enum
// =============================================================================

/// Supported audio encoding formats for STT.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DashScopeAudioFormat {
    /// PCM 16-bit signed little-endian (default).
    #[default]
    Pcm16,
    /// Opus encoded audio.
    Opus,
    /// WAV format.
    Wav,
    /// MP3 format.
    Mp3,
    /// Speex narrowband.
    Speex,
    /// AAC format.
    Aac,
    /// AMR-NB format.
    AmrNb,
}

impl DashScopeAudioFormat {
    /// Get the format string for API.
    pub fn as_format_str(&self) -> &'static str {
        match self {
            Self::Pcm16 => "pcm",
            Self::Opus => "opus",
            Self::Wav => "wav",
            Self::Mp3 => "mp3",
            Self::Speex => "speex",
            Self::Aac => "aac",
            Self::AmrNb => "amr-nb",
        }
    }

    /// Parse format from string.
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "pcm" | "pcm16" | "linear16" | "raw" => Some(Self::Pcm16),
            "opus" => Some(Self::Opus),
            "wav" => Some(Self::Wav),
            "mp3" => Some(Self::Mp3),
            "speex" => Some(Self::Speex),
            "aac" => Some(Self::Aac),
            "amr-nb" | "amrnb" | "amr" => Some(Self::AmrNb),
            _ => None,
        }
    }

    /// Check if this format is supported by Qwen3-ASR models.
    pub fn supported_by_qwen(&self) -> bool {
        matches!(self, Self::Pcm16 | Self::Opus)
    }

    /// Check if this format is supported by Paraformer models.
    pub fn supported_by_paraformer(&self) -> bool {
        matches!(
            self,
            Self::Pcm16 | Self::Wav | Self::Mp3 | Self::Opus | Self::Speex | Self::Aac | Self::AmrNb
        )
    }
}

impl fmt::Display for DashScopeAudioFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_format_str())
    }
}

// =============================================================================
// Language Enum
// =============================================================================

/// Supported languages for DashScope STT.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DashScopeLanguage {
    /// Chinese Mandarin (default).
    #[default]
    Chinese,
    /// Chinese Cantonese.
    Cantonese,
    /// Chinese Sichuanese.
    Sichuanese,
    /// Chinese Wu dialect.
    Wu,
    /// Chinese Minnan.
    Minnan,
    /// English.
    English,
    /// Japanese.
    Japanese,
    /// Korean.
    Korean,
    /// German.
    German,
    /// French.
    French,
    /// Russian.
    Russian,
    /// Spanish.
    Spanish,
    /// Portuguese.
    Portuguese,
    /// Arabic.
    Arabic,
    /// Italian.
    Italian,
    /// Hindi.
    Hindi,
    /// Indonesian.
    Indonesian,
    /// Thai.
    Thai,
    /// Vietnamese.
    Vietnamese,
    /// Turkish.
    Turkish,
    /// Ukrainian.
    Ukrainian,
    /// Malay.
    Malay,
    /// Auto-detect.
    Auto,
    /// Custom language code.
    Custom(String),
}

impl DashScopeLanguage {
    /// Get the language code for API.
    pub fn as_code(&self) -> &str {
        match self {
            Self::Chinese => "zh",
            Self::Cantonese => "yue",
            Self::Sichuanese => "sichuan",
            Self::Wu => "wu",
            Self::Minnan => "minnan",
            Self::English => "en",
            Self::Japanese => "ja",
            Self::Korean => "ko",
            Self::German => "de",
            Self::French => "fr",
            Self::Russian => "ru",
            Self::Spanish => "es",
            Self::Portuguese => "pt",
            Self::Arabic => "ar",
            Self::Italian => "it",
            Self::Hindi => "hi",
            Self::Indonesian => "id",
            Self::Thai => "th",
            Self::Vietnamese => "vi",
            Self::Turkish => "tr",
            Self::Ukrainian => "uk",
            Self::Malay => "ms",
            Self::Auto => "auto",
            Self::Custom(code) => code.as_str(),
        }
    }

    /// Parse language from string.
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "zh" | "zh_cn" | "zh-cn" | "chinese" | "mandarin" => Self::Chinese,
            "yue" | "cantonese" | "zh-yue" => Self::Cantonese,
            "sichuan" | "sichuanese" => Self::Sichuanese,
            "wu" | "shanghainese" => Self::Wu,
            "minnan" | "hokkien" | "taiwanese" => Self::Minnan,
            "en" | "en_us" | "en-us" | "english" => Self::English,
            "ja" | "japanese" => Self::Japanese,
            "ko" | "korean" => Self::Korean,
            "de" | "german" => Self::German,
            "fr" | "french" => Self::French,
            "ru" | "russian" => Self::Russian,
            "es" | "spanish" => Self::Spanish,
            "pt" | "portuguese" => Self::Portuguese,
            "ar" | "arabic" => Self::Arabic,
            "it" | "italian" => Self::Italian,
            "hi" | "hindi" => Self::Hindi,
            "id" | "indonesian" => Self::Indonesian,
            "th" | "thai" => Self::Thai,
            "vi" | "vietnamese" => Self::Vietnamese,
            "tr" | "turkish" => Self::Turkish,
            "uk" | "ukrainian" => Self::Ukrainian,
            "ms" | "malay" => Self::Malay,
            "auto" | "" => Self::Auto,
            other => Self::Custom(other.to_string()),
        }
    }
}

impl fmt::Display for DashScopeLanguage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_code())
    }
}

// =============================================================================
// Turn Detection Mode
// =============================================================================

/// Voice Activity Detection (VAD) mode for turn detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnDetectionMode {
    /// Server-side VAD (default).
    #[default]
    ServerVad,
    /// Manual turn detection (push-to-talk style).
    Manual,
    /// No turn detection.
    None,
}

impl TurnDetectionMode {
    /// Get the mode string for API.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ServerVad => "server_vad",
            Self::Manual => "manual",
            Self::None => "none",
        }
    }
}

// =============================================================================
// DashScope STT Configuration
// =============================================================================

/// Configuration for Alibaba Cloud DashScope STT.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashScopeSttConfig {
    /// API key (from DashScope console).
    pub api_key: String,

    /// Region for API endpoint.
    #[serde(default)]
    pub region: DashScopeRegion,

    /// STT model to use.
    #[serde(default)]
    pub model: DashScopeSttModel,

    /// Audio sample rate in Hz.
    #[serde(default = "default_sample_rate")]
    pub sample_rate: u32,

    /// Audio encoding format.
    #[serde(default)]
    pub audio_format: DashScopeAudioFormat,

    /// Language for recognition.
    #[serde(default)]
    pub language: DashScopeLanguage,

    /// Turn detection mode.
    #[serde(default)]
    pub turn_detection: TurnDetectionMode,

    /// Silence duration for VAD (ms).
    #[serde(default = "default_silence_duration")]
    pub silence_duration_ms: u32,

    /// Enable emotion recognition.
    #[serde(default)]
    pub emotion_recognition: bool,

    /// Enable disfluency removal (remove fillers like "um", "uh").
    #[serde(default)]
    pub disfluency_removal: bool,

    /// Enable semantic punctuation.
    #[serde(default = "default_true")]
    pub punctuation: bool,

    /// Context biasing text (hotwords).
    #[serde(default)]
    pub context_text: Option<String>,

    /// Custom vocabulary ID.
    #[serde(default)]
    pub vocabulary_id: Option<String>,

    /// Enable word-level timestamps.
    #[serde(default)]
    pub word_timestamps: bool,
}

fn default_sample_rate() -> u32 {
    DEFAULT_SAMPLE_RATE
}

fn default_silence_duration() -> u32 {
    DEFAULT_SILENCE_DURATION_MS
}

fn default_true() -> bool {
    true
}

impl Default for DashScopeSttConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            region: DashScopeRegion::default(),
            model: DashScopeSttModel::default(),
            sample_rate: DEFAULT_SAMPLE_RATE,
            audio_format: DashScopeAudioFormat::default(),
            language: DashScopeLanguage::default(),
            turn_detection: TurnDetectionMode::default(),
            silence_duration_ms: DEFAULT_SILENCE_DURATION_MS,
            emotion_recognition: false,
            disfluency_removal: false,
            punctuation: true,
            context_text: None,
            vocabulary_id: None,
            word_timestamps: false,
        }
    }
}

impl DashScopeSttConfig {
    /// Create configuration from base STTConfig.
    pub fn from_base(config: STTConfig) -> Result<Self, STTError> {
        if config.api_key.is_empty() {
            return Err(STTError::AuthenticationFailed(
                "DashScope API key is required".to_string(),
            ));
        }

        // Parse model from model field or default
        let model = if config.model.is_empty() {
            DashScopeSttModel::default()
        } else {
            DashScopeSttModel::from_str(&config.model)
        };

        // Use default region (Beijing for China, Singapore for international)
        let region = DashScopeRegion::default();

        // Parse language
        let language = DashScopeLanguage::from_str(&config.language);

        // Parse audio format from encoding
        let audio_format = DashScopeAudioFormat::from_str(&config.encoding)
            .unwrap_or_default();

        // Adjust sample rate for 8k models
        let sample_rate = if model.is_8k_model() {
            8000
        } else {
            config.sample_rate
        };

        Ok(Self {
            api_key: config.api_key,
            region,
            model,
            sample_rate,
            audio_format,
            language,
            turn_detection: TurnDetectionMode::default(),
            silence_duration_ms: DEFAULT_SILENCE_DURATION_MS,
            emotion_recognition: false,
            disfluency_removal: false,
            punctuation: config.punctuation,
            context_text: None,
            vocabulary_id: None,
            word_timestamps: false,
        })
    }

    /// Validate configuration.
    pub fn validate(&self) -> Result<(), STTError> {
        // Check API key
        if self.api_key.is_empty() {
            return Err(STTError::ConfigurationError(
                "DashScope API key is required".to_string(),
            ));
        }

        // Check sample rate
        let valid_sample_rates = if self.model.is_8k_model() {
            vec![8000]
        } else {
            vec![8000, 16000]
        };

        if !valid_sample_rates.contains(&self.sample_rate) {
            return Err(STTError::ConfigurationError(format!(
                "Invalid sample rate {} for model {}. Supported: {:?}",
                self.sample_rate, self.model, valid_sample_rates
            )));
        }

        // Check audio format compatibility
        if self.model.is_qwen_model() && !self.audio_format.supported_by_qwen() {
            return Err(STTError::ConfigurationError(format!(
                "Audio format {} not supported by Qwen models. Use pcm16 or opus.",
                self.audio_format
            )));
        }

        Ok(())
    }

    /// Get the WebSocket URL for the configured model and region.
    pub fn get_websocket_url(&self) -> String {
        if self.model.is_qwen_model() {
            format!("{}?model={}", self.region.realtime_url(), self.model.as_model_id())
        } else {
            self.region.inference_url().to_string()
        }
    }

    /// Get list of supported languages.
    pub fn supported_languages() -> Vec<&'static str> {
        vec![
            "zh", "yue", "sichuan", "wu", "minnan", "en", "ja", "ko", "de", "fr",
            "ru", "es", "pt", "ar", "it", "hi", "id", "th", "vi", "tr", "uk", "ms",
        ]
    }

    /// Get list of supported models.
    pub fn supported_models() -> Vec<&'static str> {
        vec![
            "qwen3-asr-flash-realtime",
            "paraformer-realtime-v2",
            "paraformer-realtime-v1",
            "paraformer-realtime-8k-v2",
            "paraformer-realtime-8k-v1",
            "fun-asr-realtime",
        ]
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_region_urls() {
        assert!(DashScopeRegion::Beijing.realtime_url().contains("dashscope.aliyuncs.com"));
        assert!(DashScopeRegion::Singapore.realtime_url().contains("dashscope-intl"));
    }

    #[test]
    fn test_region_parsing() {
        assert_eq!(DashScopeRegion::from_str("beijing"), DashScopeRegion::Beijing);
        assert_eq!(DashScopeRegion::from_str("singapore"), DashScopeRegion::Singapore);
        assert_eq!(DashScopeRegion::from_str("cn"), DashScopeRegion::Beijing);
        assert_eq!(DashScopeRegion::from_str("intl"), DashScopeRegion::Singapore);
    }

    #[test]
    fn test_model_parsing() {
        assert_eq!(
            DashScopeSttModel::from_str("qwen3-asr-flash-realtime"),
            DashScopeSttModel::Qwen3AsrFlashRealtime
        );
        assert_eq!(
            DashScopeSttModel::from_str("paraformer-realtime-v2"),
            DashScopeSttModel::ParaformerRealtimeV2
        );
        assert_eq!(
            DashScopeSttModel::from_str("paraformer-8k"),
            DashScopeSttModel::Paraformer8kRealtimeV2
        );
    }

    #[test]
    fn test_model_id() {
        assert_eq!(
            DashScopeSttModel::Qwen3AsrFlashRealtime.as_model_id(),
            "qwen3-asr-flash-realtime"
        );
        assert_eq!(
            DashScopeSttModel::ParaformerRealtimeV2.as_model_id(),
            "paraformer-realtime-v2"
        );
    }

    #[test]
    fn test_model_type_detection() {
        assert!(DashScopeSttModel::Qwen3AsrFlashRealtime.is_qwen_model());
        assert!(!DashScopeSttModel::Qwen3AsrFlashRealtime.is_paraformer_model());
        assert!(DashScopeSttModel::ParaformerRealtimeV2.is_paraformer_model());
        assert!(DashScopeSttModel::Paraformer8kRealtimeV2.is_8k_model());
    }

    #[test]
    fn test_audio_format_parsing() {
        assert_eq!(DashScopeAudioFormat::from_str("pcm"), Some(DashScopeAudioFormat::Pcm16));
        assert_eq!(DashScopeAudioFormat::from_str("opus"), Some(DashScopeAudioFormat::Opus));
        assert_eq!(DashScopeAudioFormat::from_str("wav"), Some(DashScopeAudioFormat::Wav));
        assert_eq!(DashScopeAudioFormat::from_str("unknown"), None);
    }

    #[test]
    fn test_audio_format_compatibility() {
        assert!(DashScopeAudioFormat::Pcm16.supported_by_qwen());
        assert!(DashScopeAudioFormat::Opus.supported_by_qwen());
        assert!(!DashScopeAudioFormat::Mp3.supported_by_qwen());
        assert!(DashScopeAudioFormat::Mp3.supported_by_paraformer());
    }

    #[test]
    fn test_language_parsing() {
        assert_eq!(DashScopeLanguage::from_str("zh"), DashScopeLanguage::Chinese);
        assert_eq!(DashScopeLanguage::from_str("en"), DashScopeLanguage::English);
        assert_eq!(DashScopeLanguage::from_str("yue"), DashScopeLanguage::Cantonese);
        assert_eq!(DashScopeLanguage::from_str("auto"), DashScopeLanguage::Auto);
    }

    #[test]
    fn test_language_code() {
        assert_eq!(DashScopeLanguage::Chinese.as_code(), "zh");
        assert_eq!(DashScopeLanguage::English.as_code(), "en");
        assert_eq!(DashScopeLanguage::Cantonese.as_code(), "yue");
    }

    #[test]
    fn test_config_default() {
        let config = DashScopeSttConfig::default();
        assert_eq!(config.region, DashScopeRegion::Beijing);
        assert_eq!(config.sample_rate, 16000);
        assert!(config.punctuation);
    }

    #[test]
    fn test_config_validation_empty_api_key() {
        let config = DashScopeSttConfig::default();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_valid() {
        let mut config = DashScopeSttConfig::default();
        config.api_key = "test_api_key".to_string();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validation_invalid_sample_rate() {
        let mut config = DashScopeSttConfig::default();
        config.api_key = "test_api_key".to_string();
        config.sample_rate = 44100;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_8k_model() {
        let mut config = DashScopeSttConfig::default();
        config.api_key = "test_api_key".to_string();
        config.model = DashScopeSttModel::Paraformer8kRealtimeV2;
        config.sample_rate = 8000;
        assert!(config.validate().is_ok());

        config.sample_rate = 16000;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_audio_format() {
        let mut config = DashScopeSttConfig::default();
        config.api_key = "test_api_key".to_string();
        config.model = DashScopeSttModel::Qwen3AsrFlashRealtime;
        config.audio_format = DashScopeAudioFormat::Mp3;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_websocket_url_qwen() {
        let mut config = DashScopeSttConfig::default();
        config.model = DashScopeSttModel::Qwen3AsrFlashRealtime;
        config.region = DashScopeRegion::Singapore;

        let url = config.get_websocket_url();
        assert!(url.contains("dashscope-intl"));
        assert!(url.contains("realtime"));
        assert!(url.contains("qwen3-asr-flash-realtime"));
    }

    #[test]
    fn test_websocket_url_paraformer() {
        let mut config = DashScopeSttConfig::default();
        config.model = DashScopeSttModel::ParaformerRealtimeV2;
        config.region = DashScopeRegion::Beijing;

        let url = config.get_websocket_url();
        assert!(url.contains("dashscope.aliyuncs.com"));
        assert!(url.contains("inference"));
    }

    #[test]
    fn test_from_base_config() {
        let base_config = STTConfig {
            api_key: "test_key".to_string(),
            language: "zh_cn".to_string(),
            sample_rate: 16000,
            encoding: "pcm".to_string(),
            model: "qwen3-asr-flash-realtime".to_string(),
            ..Default::default()
        };

        let config = DashScopeSttConfig::from_base(base_config).unwrap();
        assert_eq!(config.api_key, "test_key");
        assert_eq!(config.language, DashScopeLanguage::Chinese);
        assert_eq!(config.model, DashScopeSttModel::Qwen3AsrFlashRealtime);
    }

    #[test]
    fn test_supported_languages() {
        let languages = DashScopeSttConfig::supported_languages();
        assert!(languages.contains(&"zh"));
        assert!(languages.contains(&"en"));
        assert!(languages.contains(&"ja"));
    }

    #[test]
    fn test_supported_models() {
        let models = DashScopeSttConfig::supported_models();
        assert!(models.contains(&"qwen3-asr-flash-realtime"));
        assert!(models.contains(&"paraformer-realtime-v2"));
    }

    #[test]
    fn test_turn_detection_mode() {
        assert_eq!(TurnDetectionMode::ServerVad.as_str(), "server_vad");
        assert_eq!(TurnDetectionMode::Manual.as_str(), "manual");
        assert_eq!(TurnDetectionMode::None.as_str(), "none");
    }
}
