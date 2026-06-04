//! Tencent Cloud Speech Recognition Configuration
//!
//! Configuration types for Tencent Cloud Speech-to-Text service (ASR).
//!
//! # Authentication
//!
//! Tencent Cloud uses HMAC-SHA1 signature authentication:
//! - Parameters are sorted alphabetically
//! - Signature is calculated as: Base64(HMAC-SHA1(secret_key, param_string))
//! - Signature is URL-encoded and included in WebSocket URL params
//!
//! # API Key Format
//!
//! For the WaaV Gateway, API key should be provided as:
//! `secret_id|secret_key|app_id`

use crate::core::stt::base::{STTConfig, STTError};
use serde::{Deserialize, Serialize};

// =============================================================================
// Constants
// =============================================================================

/// WebSocket endpoint for real-time ASR.
pub const TENCENT_ASR_WS_URL: &str = "wss://asr.cloud.tencent.com/asr/v2";

/// Default engine model (Mandarin 16kHz).
pub const DEFAULT_ENGINE_MODEL: &str = "16k_zh";

/// Default voice format (Speex, recommended).
pub const DEFAULT_VOICE_FORMAT: u32 = 4;

/// Default sample rate.
pub const DEFAULT_SAMPLE_RATE: u32 = 16000;

/// Recommended chunk duration in milliseconds (40ms).
pub const RECOMMENDED_CHUNK_DURATION_MS: u32 = 40;

/// Signature validity period in seconds (24 hours).
pub const SIGNATURE_VALIDITY_SECS: u64 = 86400;

/// Maximum nonce value (10 digits).
pub const MAX_NONCE: u64 = 9999999999;

/// Voice ID length (16 characters).
pub const VOICE_ID_LENGTH: usize = 16;

/// VAD silence time minimum (240ms).
pub const VAD_SILENCE_TIME_MIN: u32 = 240;

/// VAD silence time maximum (2000ms).
pub const VAD_SILENCE_TIME_MAX: u32 = 2000;

// =============================================================================
// Engine Models
// =============================================================================

/// Tencent Cloud ASR engine models.
///
/// Each model is optimized for specific language and sample rate combinations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TencentEngineModel {
    /// Mandarin Chinese 8kHz telephone (8k_zh)
    Mandarin8k,

    /// Mandarin Chinese 8kHz short audio (8k_zh_s)
    MandarinShort8k,

    /// English 8kHz telephone (8k_en)
    English8k,

    /// Mandarin Chinese 16kHz (16k_zh) - Default
    #[default]
    Mandarin16k,

    /// Large model - Mandarin + dialects + English (16k_zh_large)
    MandarinLarge16k,

    /// Mandarin Chinese 16kHz video (16k_zh_video)
    MandarinVideo16k,

    /// English 16kHz (16k_en)
    English16k,

    /// Cantonese 16kHz (16k_ca) - Legacy alias
    Cantonese16k,

    /// Cantonese 16kHz (16k_yue) - Standard
    CantoneseYue16k,

    /// Traditional Chinese 16kHz (16k_zh-TW)
    TraditionalChinese16k,

    /// Arabic 16kHz (16k_ar)
    Arabic16k,

    /// Japanese 16kHz (16k_ja)
    Japanese16k,

    /// Korean 16kHz (16k_ko)
    Korean16k,

    /// Thai 16kHz (16k_th)
    Thai16k,

    /// Vietnamese 16kHz (16k_vi)
    Vietnamese16k,

    /// Indonesian 16kHz (16k_id)
    Indonesian16k,

    /// Malay 16kHz (16k_ms)
    Malay16k,
}

impl TencentEngineModel {
    /// Get the model string for API requests.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Mandarin8k => "8k_zh",
            Self::MandarinShort8k => "8k_zh_s",
            Self::English8k => "8k_en",
            Self::Mandarin16k => "16k_zh",
            Self::MandarinLarge16k => "16k_zh_large",
            Self::MandarinVideo16k => "16k_zh_video",
            Self::English16k => "16k_en",
            Self::Cantonese16k => "16k_ca",
            Self::CantoneseYue16k => "16k_yue",
            Self::TraditionalChinese16k => "16k_zh-TW",
            Self::Arabic16k => "16k_ar",
            Self::Japanese16k => "16k_ja",
            Self::Korean16k => "16k_ko",
            Self::Thai16k => "16k_th",
            Self::Vietnamese16k => "16k_vi",
            Self::Indonesian16k => "16k_id",
            Self::Malay16k => "16k_ms",
        }
    }

    /// Parse a model from string.
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "8k_zh" | "mandarin_8k" => Self::Mandarin8k,
            "8k_zh_s" | "mandarin_short_8k" => Self::MandarinShort8k,
            "8k_en" | "english_8k" => Self::English8k,
            "16k_zh" | "mandarin" | "zh" | "zh-cn" | "chinese" => Self::Mandarin16k,
            "16k_zh_large" | "mandarin_large" | "zh_large" => Self::MandarinLarge16k,
            "16k_zh_video" | "mandarin_video" => Self::MandarinVideo16k,
            "16k_en" | "english" | "en" | "en-us" => Self::English16k,
            "16k_ca" | "cantonese_legacy" => Self::Cantonese16k,
            "16k_yue" | "cantonese" | "yue" | "zh-yue" => Self::CantoneseYue16k,
            "16k_zh-tw" | "zh-tw" | "traditional" | "traditional_chinese" => {
                Self::TraditionalChinese16k
            }
            "16k_ar" | "arabic" | "ar" => Self::Arabic16k,
            "16k_ja" | "japanese" | "ja" | "ja-jp" => Self::Japanese16k,
            "16k_ko" | "korean" | "ko" | "ko-kr" => Self::Korean16k,
            "16k_th" | "thai" | "th" | "th-th" => Self::Thai16k,
            "16k_vi" | "vietnamese" | "vi" | "vi-vn" => Self::Vietnamese16k,
            "16k_id" | "indonesian" | "id" | "id-id" => Self::Indonesian16k,
            "16k_ms" | "malay" | "ms" | "ms-my" => Self::Malay16k,
            _ => Self::Mandarin16k,
        }
    }

    /// Get the sample rate for this model.
    pub fn sample_rate(&self) -> u32 {
        match self {
            Self::Mandarin8k | Self::MandarinShort8k | Self::English8k => 8000,
            _ => 16000,
        }
    }

    /// Get the language code for this model.
    pub fn language_code(&self) -> &'static str {
        match self {
            Self::Mandarin8k
            | Self::MandarinShort8k
            | Self::Mandarin16k
            | Self::MandarinLarge16k
            | Self::MandarinVideo16k => "zh",
            Self::English8k | Self::English16k => "en",
            Self::Cantonese16k | Self::CantoneseYue16k => "yue",
            Self::TraditionalChinese16k => "zh-TW",
            Self::Arabic16k => "ar",
            Self::Japanese16k => "ja",
            Self::Korean16k => "ko",
            Self::Thai16k => "th",
            Self::Vietnamese16k => "vi",
            Self::Indonesian16k => "id",
            Self::Malay16k => "ms",
        }
    }

    /// Get all supported models.
    pub fn all() -> &'static [Self] {
        &[
            Self::Mandarin8k,
            Self::MandarinShort8k,
            Self::English8k,
            Self::Mandarin16k,
            Self::MandarinLarge16k,
            Self::MandarinVideo16k,
            Self::English16k,
            Self::Cantonese16k,
            Self::CantoneseYue16k,
            Self::TraditionalChinese16k,
            Self::Arabic16k,
            Self::Japanese16k,
            Self::Korean16k,
            Self::Thai16k,
            Self::Vietnamese16k,
            Self::Indonesian16k,
            Self::Malay16k,
        ]
    }
}

// =============================================================================
// Audio Formats
// =============================================================================

/// Supported audio formats for Tencent Cloud ASR.
///
/// The `voice_format` parameter value in API requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[repr(u32)]
pub enum TencentAudioFormat {
    /// PCM raw audio (uncompressed).
    Pcm = 1,

    /// Speex format (default, recommended for streaming).
    #[default]
    Speex = 4,

    /// SILK format (WeChat audio format).
    Silk = 6,

    /// MP3 format (compressed).
    Mp3 = 8,

    /// OPUS format (low latency, recommended for real-time).
    Opus = 10,

    /// WAV format (PCM with header).
    Wav = 12,

    /// M4A format (AAC container).
    M4a = 14,

    /// AAC format (compressed).
    Aac = 16,
}

impl TencentAudioFormat {
    /// Get the voice_format value for API requests.
    pub fn value(&self) -> u32 {
        *self as u32
    }

    /// Get the format name as string.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pcm => "pcm",
            Self::Speex => "speex",
            Self::Silk => "silk",
            Self::Mp3 => "mp3",
            Self::Opus => "opus",
            Self::Wav => "wav",
            Self::M4a => "m4a",
            Self::Aac => "aac",
        }
    }

    /// Parse a format from string.
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "pcm" | "linear16" | "1" => Some(Self::Pcm),
            "speex" | "4" => Some(Self::Speex),
            "silk" | "6" => Some(Self::Silk),
            "mp3" | "8" => Some(Self::Mp3),
            "opus" | "10" => Some(Self::Opus),
            "wav" | "12" => Some(Self::Wav),
            "m4a" | "14" => Some(Self::M4a),
            "aac" | "16" => Some(Self::Aac),
            _ => None,
        }
    }

    /// Parse from voice_format integer value.
    pub fn from_value(value: u32) -> Option<Self> {
        match value {
            1 => Some(Self::Pcm),
            4 => Some(Self::Speex),
            6 => Some(Self::Silk),
            8 => Some(Self::Mp3),
            10 => Some(Self::Opus),
            12 => Some(Self::Wav),
            14 => Some(Self::M4a),
            16 => Some(Self::Aac),
            _ => None,
        }
    }

    /// Get all supported formats.
    pub fn all() -> &'static [Self] {
        &[
            Self::Pcm,
            Self::Speex,
            Self::Silk,
            Self::Mp3,
            Self::Opus,
            Self::Wav,
            Self::M4a,
            Self::Aac,
        ]
    }
}

// =============================================================================
// Word Info Level
// =============================================================================

/// Word-level timestamp detail level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[repr(u8)]
pub enum TencentWordInfo {
    /// No word-level timestamps.
    #[default]
    None = 0,

    /// Basic word timestamps.
    Basic = 1,

    /// Detailed word timestamps with stability info.
    Detailed = 2,
}

impl TencentWordInfo {
    /// Get the word_info value for API requests.
    pub fn value(&self) -> u8 {
        *self as u8
    }

    /// Parse from integer value.
    pub fn from_value(value: u8) -> Self {
        match value {
            0 => Self::None,
            1 => Self::Basic,
            2 => Self::Detailed,
            _ => Self::None,
        }
    }
}

// =============================================================================
// Configuration
// =============================================================================

/// Maximum speak time minimum (5 seconds).
pub const MAX_SPEAK_TIME_MIN: u32 = 5000;

/// Maximum speak time maximum (90 seconds).
pub const MAX_SPEAK_TIME_MAX: u32 = 90000;

/// Maximum hotword list size (128 terms).
pub const MAX_HOTWORD_LIST_SIZE: usize = 128;

/// Numeral conversion modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[repr(u8)]
pub enum TencentNumeralMode {
    /// Convert numbers to Chinese characters (default).
    #[default]
    Chinese = 0,

    /// Convert numbers to Arabic numerals.
    Arabic = 1,

    /// Mathematical expression mode.
    Math = 3,
}

impl TencentNumeralMode {
    /// Get the convert_num_mode value for API requests.
    pub fn value(&self) -> u8 {
        *self as u8
    }

    /// Parse from integer value.
    pub fn from_value(value: u8) -> Self {
        match value {
            0 => Self::Chinese,
            1 => Self::Arabic,
            3 => Self::Math,
            _ => Self::Chinese,
        }
    }
}

/// Profanity filter modes (0=off, 1=filter, 2=replace).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[repr(u8)]
pub enum TencentFilterDirtyMode {
    /// No filtering.
    #[default]
    Off = 0,

    /// Filter out profanity (remove).
    Filter = 1,

    /// Replace profanity with asterisks.
    Replace = 2,
}

impl TencentFilterDirtyMode {
    /// Get the filter_dirty value for API requests.
    pub fn value(&self) -> u8 {
        *self as u8
    }

    /// Parse from integer value.
    pub fn from_value(value: u8) -> Self {
        match value {
            0 => Self::Off,
            1 => Self::Filter,
            2 => Self::Replace,
            _ => Self::Off,
        }
    }
}

/// Modal particle filter modes (0=off, 1=partial, 2=strict).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[repr(u8)]
pub enum TencentFilterModalMode {
    /// No filtering.
    #[default]
    Off = 0,

    /// Partial filtering.
    Partial = 1,

    /// Strict filtering.
    Strict = 2,
}

impl TencentFilterModalMode {
    /// Get the filter_modal value for API requests.
    pub fn value(&self) -> u8 {
        *self as u8
    }

    /// Parse from integer value.
    pub fn from_value(value: u8) -> Self {
        match value {
            0 => Self::Off,
            1 => Self::Partial,
            2 => Self::Strict,
            _ => Self::Off,
        }
    }
}

/// Tencent Cloud ASR configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TencentSttConfig {
    /// Tencent Cloud Secret ID.
    pub secret_id: String,

    /// Tencent Cloud Secret Key (used for HMAC signature).
    pub secret_key: String,

    /// Tencent Cloud Application ID.
    pub app_id: String,

    /// ASR engine model type.
    pub engine_model_type: TencentEngineModel,

    /// Audio format (voice_format).
    pub voice_format: TencentAudioFormat,

    /// Enable Voice Activity Detection.
    pub needvad: bool,

    /// Filter profanity mode (0=off, 1=filter, 2=replace).
    pub filter_dirty: TencentFilterDirtyMode,

    /// Filter modal particles mode (0=off, 1=partial, 2=strict).
    pub filter_modal: TencentFilterModalMode,

    /// Filter punctuation (0=off, 1=on).
    pub filter_punc: bool,

    /// Word-level timestamp detail.
    pub word_info: TencentWordInfo,

    /// VAD silence threshold in milliseconds (240-2000).
    /// Silence duration to consider end of sentence.
    pub vad_silence_time: Option<u32>,

    /// Maximum speak time before forced segmentation (5000-90000ms).
    pub max_speak_time: Option<u32>,

    /// Custom vocabulary (hot word) ID.
    pub hotword_id: Option<String>,

    /// Temporary hotword list (up to 128 terms).
    pub hotword_list: Option<Vec<String>>,

    /// Enable homophonic replacement (8k_zh, 16k_zh only).
    pub reinforce_hotword: bool,

    /// Numeral conversion mode.
    pub convert_num_mode: TencentNumeralMode,

    /// Self-learning model customization ID.
    pub customization_id: Option<String>,

    /// Filter empty result callbacks.
    pub filter_empty_result: bool,

    // -------------------------------------------------------------------------
    // Provider-extras passthrough params (Tencent Real-Time ASR query params not modeled by the
    // typed `SttFeatures` vocabulary). All optional; `None` => omitted from the URL (provider
    // default). Carried verbatim from `StandardSTTConfig::extras`.
    // -------------------------------------------------------------------------
    /// Noise filter threshold (`noise_threshold`, float in -1.0..=1.0; higher filters more noise,
    /// lower keeps more audio). Tencent Real-Time ASR `noise_threshold` query param.
    pub noise_threshold: Option<f64>,

    /// Sample rate of the *input* audio (`input_sample_rate`) for engines that accept a different
    /// input rate than their nominal model rate. Tencent Real-Time ASR `input_sample_rate` param.
    pub input_sample_rate: Option<u32>,

    /// Carried from the standardized `endpoint_override` — points the dial at the in-repo mock/proxy
    /// (a local `ws://` server) for credential-free end-to-end integration tests; `None` uses the
    /// production Tencent endpoint. The HMAC signature is still computed over the real host (the mock
    /// ignores it); only the dialed scheme://host is swapped.
    pub endpoint_override: Option<String>,
}

impl Default for TencentSttConfig {
    fn default() -> Self {
        Self {
            secret_id: String::new(),
            secret_key: String::new(),
            app_id: String::new(),
            engine_model_type: TencentEngineModel::default(),
            voice_format: TencentAudioFormat::default(),
            needvad: true,
            filter_dirty: TencentFilterDirtyMode::default(),
            filter_modal: TencentFilterModalMode::default(),
            filter_punc: false,
            word_info: TencentWordInfo::default(),
            vad_silence_time: None,
            max_speak_time: None,
            hotword_id: None,
            hotword_list: None,
            reinforce_hotword: false,
            convert_num_mode: TencentNumeralMode::default(),
            customization_id: None,
            filter_empty_result: false,
            noise_threshold: None,
            input_sample_rate: None,
            endpoint_override: None,
        }
    }
}

impl TencentSttConfig {
    /// Create a new configuration with credentials.
    pub fn new(
        secret_id: impl Into<String>,
        secret_key: impl Into<String>,
        app_id: impl Into<String>,
    ) -> Self {
        Self {
            secret_id: secret_id.into(),
            secret_key: secret_key.into(),
            app_id: app_id.into(),
            ..Default::default()
        }
    }

    /// Set the engine model.
    pub fn with_engine_model(mut self, model: TencentEngineModel) -> Self {
        self.engine_model_type = model;
        self
    }

    /// Set the voice format.
    pub fn with_voice_format(mut self, format: TencentAudioFormat) -> Self {
        self.voice_format = format;
        self
    }

    /// Enable/disable VAD.
    pub fn with_vad(mut self, enabled: bool) -> Self {
        self.needvad = enabled;
        self
    }

    /// Set VAD silence threshold.
    pub fn with_vad_silence_time(mut self, ms: u32) -> Self {
        self.vad_silence_time = Some(ms.clamp(VAD_SILENCE_TIME_MIN, VAD_SILENCE_TIME_MAX));
        self
    }

    /// Set profanity filter mode.
    pub fn with_filter_dirty(mut self, mode: TencentFilterDirtyMode) -> Self {
        self.filter_dirty = mode;
        self
    }

    /// Set modal particle filter mode.
    pub fn with_filter_modal(mut self, mode: TencentFilterModalMode) -> Self {
        self.filter_modal = mode;
        self
    }

    /// Enable/disable punctuation filter.
    pub fn with_filter_punc(mut self, enabled: bool) -> Self {
        self.filter_punc = enabled;
        self
    }

    /// Set word info level.
    pub fn with_word_info(mut self, level: TencentWordInfo) -> Self {
        self.word_info = level;
        self
    }

    /// Set custom vocabulary ID.
    pub fn with_hotword_id(mut self, id: impl Into<String>) -> Self {
        self.hotword_id = Some(id.into());
        self
    }

    /// Set temporary hotword list (up to 128 terms).
    pub fn with_hotword_list(mut self, list: Vec<String>) -> Self {
        let truncated: Vec<String> = list.into_iter().take(MAX_HOTWORD_LIST_SIZE).collect();
        self.hotword_list = Some(truncated);
        self
    }

    /// Set maximum speak time before forced segmentation.
    pub fn with_max_speak_time(mut self, ms: u32) -> Self {
        self.max_speak_time = Some(ms.clamp(MAX_SPEAK_TIME_MIN, MAX_SPEAK_TIME_MAX));
        self
    }

    /// Enable homophonic replacement (8k_zh, 16k_zh only).
    pub fn with_reinforce_hotword(mut self, enabled: bool) -> Self {
        self.reinforce_hotword = enabled;
        self
    }

    /// Set numeral conversion mode.
    pub fn with_convert_num_mode(mut self, mode: TencentNumeralMode) -> Self {
        self.convert_num_mode = mode;
        self
    }

    /// Set self-learning model customization ID.
    pub fn with_customization_id(mut self, id: impl Into<String>) -> Self {
        self.customization_id = Some(id.into());
        self
    }

    /// Enable filtering of empty result callbacks.
    pub fn with_filter_empty_result(mut self, enabled: bool) -> Self {
        self.filter_empty_result = enabled;
        self
    }

    /// Validate the configuration.
    pub fn validate(&self) -> Result<(), STTError> {
        if self.secret_id.is_empty() {
            return Err(STTError::ConfigurationError(
                "Tencent Secret ID is required".to_string(),
            ));
        }

        if self.secret_key.is_empty() {
            return Err(STTError::ConfigurationError(
                "Tencent Secret Key is required".to_string(),
            ));
        }

        if self.app_id.is_empty() {
            return Err(STTError::ConfigurationError(
                "Tencent App ID is required".to_string(),
            ));
        }

        // Validate VAD silence time
        if let Some(vad_time) = self.vad_silence_time
            && (!(VAD_SILENCE_TIME_MIN..=VAD_SILENCE_TIME_MAX).contains(&vad_time)) {
                return Err(STTError::ConfigurationError(format!(
                    "VAD silence time must be between {} and {} ms",
                    VAD_SILENCE_TIME_MIN, VAD_SILENCE_TIME_MAX
                )));
            }

        // Validate max speak time
        if let Some(max_speak) = self.max_speak_time
            && (!(MAX_SPEAK_TIME_MIN..=MAX_SPEAK_TIME_MAX).contains(&max_speak)) {
                return Err(STTError::ConfigurationError(format!(
                    "Max speak time must be between {} and {} ms",
                    MAX_SPEAK_TIME_MIN, MAX_SPEAK_TIME_MAX
                )));
            }

        // Validate hotword list size
        if let Some(ref list) = self.hotword_list
            && list.len() > MAX_HOTWORD_LIST_SIZE {
                return Err(STTError::ConfigurationError(format!(
                    "Hotword list must not exceed {} terms",
                    MAX_HOTWORD_LIST_SIZE
                )));
            }

        // Validate reinforce_hotword is only used with compatible models
        if self.reinforce_hotword {
            let model = self.engine_model_type.as_str();
            if model != "8k_zh" && model != "16k_zh" {
                return Err(STTError::ConfigurationError(
                    "reinforce_hotword is only supported for 8k_zh and 16k_zh models".to_string(),
                ));
            }
        }

        Ok(())
    }

    /// Get the sample rate for the configured engine model.
    pub fn sample_rate(&self) -> u32 {
        self.engine_model_type.sample_rate()
    }

    /// Calculate chunk size for the recommended duration.
    /// Formula: sample_rate * 2 (bytes per sample) * duration_ms / 1000
    pub fn get_chunk_size(&self) -> usize {
        (self.sample_rate() as usize * 2 * RECOMMENDED_CHUNK_DURATION_MS as usize) / 1000
    }

    /// Build the WebSocket URL base (without signature params).
    pub fn get_ws_url_base(&self) -> String {
        format!("{}/{}", TENCENT_ASR_WS_URL, self.app_id)
    }

    /// Get list of supported languages.
    pub fn supported_languages() -> Vec<&'static str> {
        vec![
            "zh", "zh-TW", "en", "yue", "ar", "ja", "ko", "th", "vi", "id", "ms",
        ]
    }

    /// Get list of supported models.
    pub fn supported_models() -> Vec<&'static str> {
        TencentEngineModel::all()
            .iter()
            .map(|m| m.as_str())
            .collect()
    }

    /// Create from base STT config.
    ///
    /// Expected API key format: `secret_id|secret_key|app_id`
    pub fn from_base(config: STTConfig) -> Result<Self, STTError> {
        let parts: Vec<&str> = config.api_key.splitn(3, '|').collect();
        if parts.len() != 3 {
            return Err(STTError::ConfigurationError(
                "API key must be in format: secret_id|secret_key|app_id".to_string(),
            ));
        }

        let secret_id = parts[0].to_string();
        let secret_key = parts[1].to_string();
        let app_id = parts[2].to_string();

        if secret_id.is_empty() || secret_key.is_empty() || app_id.is_empty() {
            return Err(STTError::ConfigurationError(
                "All credentials (secret_id, secret_key, app_id) are required".to_string(),
            ));
        }

        let engine_model = TencentEngineModel::from_str(&config.model);
        let voice_format =
            TencentAudioFormat::from_str(&config.encoding).unwrap_or(TencentAudioFormat::Speex);

        let tencent_config = Self {
            secret_id,
            secret_key,
            app_id,
            engine_model_type: engine_model,
            voice_format,
            needvad: true,
            ..Default::default()
        };

        tencent_config.validate()?;
        Ok(tencent_config)
    }

    /// Build from the standardized config (W1 keystone). Tencent ASR models its advanced features
    /// as enum/bool knobs, so this maps the standardized features whose meaning matches an existing
    /// Tencent field: word timestamps (`word_info`), profanity filtering (`filter_dirty`), filler
    /// words (`filter_modal` — modal-particle filtering, whose sense is inverted: keeping fillers
    /// means *not* filtering modal particles), VAD events (`needvad`), endpointing
    /// (`vad_silence_time`) and key terms (`hotword_list`). Features Tencent can't express
    /// (diarization, smart_format, interim_results, redaction, entity/language detection) are
    /// capability gaps and stay at their defaults.
    pub fn from_standard(
        std: &crate::core::stt::standard::StandardSTTConfig,
    ) -> Result<Self, STTError> {
        let f = &std.features;
        let mut cfg = Self::from_base(std.base.clone())?;
        if let Some(w) = f.word_timestamps {
            cfg.word_info = if w {
                TencentWordInfo::Basic
            } else {
                TencentWordInfo::None
            };
        }
        if let Some(p) = f.profanity_filter {
            cfg.filter_dirty = if p {
                TencentFilterDirtyMode::Filter
            } else {
                TencentFilterDirtyMode::Off
            };
        }
        if let Some(filler) = f.filler_words {
            cfg.filter_modal = if filler {
                TencentFilterModalMode::Off
            } else {
                TencentFilterModalMode::Strict
            };
        }
        if let Some(v) = f.vad_events {
            cfg.needvad = v;
        }
        if let Some(ms) = f.endpointing_ms {
            cfg.vad_silence_time = Some(ms.clamp(VAD_SILENCE_TIME_MIN, VAD_SILENCE_TIME_MAX));
        }
        if let Some(k) = &f.keyterms {
            cfg.hotword_list = Some(k.iter().take(MAX_HOTWORD_LIST_SIZE).cloned().collect());
        }
        // Provider extras → Tencent Real-Time ASR query params not modeled by the typed vocabulary.
        // Keys not present stay `None` and are omitted from the URL (provider default).
        let e = &std.extras.0;
        if let Some(v) = e.get("noise_threshold").and_then(|v| v.as_f64()) {
            cfg.noise_threshold = Some(v);
        }
        if let Some(v) = e.get("input_sample_rate").and_then(|v| v.as_u64()) {
            cfg.input_sample_rate = Some(v as u32);
        }
        // Standardized endpoint override (mock/proxy host) for credential-free integration tests.
        cfg.endpoint_override = std.endpoint_override().map(|s| s.to_string());
        Ok(cfg)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // W1 keystone: maps the standardized features Tencent can express (profanity filter +
    // key terms) onto its own config fields.
    #[test]
    fn from_standard_maps_features() {
        use crate::core::stt::standard::{ProviderExtras, SttFeatures, StandardSTTConfig};
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "tencent".into(),
                api_key: "id|key|app".into(),
                ..Default::default()
            },
            features: SttFeatures {
                profanity_filter: Some(true),
                keyterms: Some(vec!["WaaV".into(), "Tencent".into()]),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
        };
        let cfg = TencentSttConfig::from_standard(&std).unwrap();
        assert_eq!(cfg.filter_dirty, TencentFilterDirtyMode::Filter); // profanity_filter
        assert_eq!(
            cfg.hotword_list,
            Some(vec!["WaaV".to_string(), "Tencent".to_string()])
        ); // keyterms
    }

    // =========================================================================
    // Engine Model Tests
    // =========================================================================

    #[test]
    fn test_engine_model_parsing_mandarin() {
        assert_eq!(
            TencentEngineModel::from_str("16k_zh"),
            TencentEngineModel::Mandarin16k
        );
        assert_eq!(
            TencentEngineModel::from_str("mandarin"),
            TencentEngineModel::Mandarin16k
        );
        assert_eq!(
            TencentEngineModel::from_str("zh"),
            TencentEngineModel::Mandarin16k
        );
        assert_eq!(
            TencentEngineModel::from_str("chinese"),
            TencentEngineModel::Mandarin16k
        );
    }

    #[test]
    fn test_engine_model_parsing_8k() {
        assert_eq!(
            TencentEngineModel::from_str("8k_zh"),
            TencentEngineModel::Mandarin8k
        );
        assert_eq!(
            TencentEngineModel::from_str("8k_zh_s"),
            TencentEngineModel::MandarinShort8k
        );
    }

    #[test]
    fn test_engine_model_parsing_languages() {
        assert_eq!(
            TencentEngineModel::from_str("16k_en"),
            TencentEngineModel::English16k
        );
        assert_eq!(
            TencentEngineModel::from_str("english"),
            TencentEngineModel::English16k
        );
        assert_eq!(
            TencentEngineModel::from_str("8k_en"),
            TencentEngineModel::English8k
        );
        assert_eq!(
            TencentEngineModel::from_str("16k_ca"),
            TencentEngineModel::Cantonese16k
        );
        assert_eq!(
            TencentEngineModel::from_str("cantonese"),
            TencentEngineModel::CantoneseYue16k
        ); // Maps to standard 16k_yue
        assert_eq!(
            TencentEngineModel::from_str("16k_yue"),
            TencentEngineModel::CantoneseYue16k
        );
        assert_eq!(
            TencentEngineModel::from_str("16k_zh-tw"),
            TencentEngineModel::TraditionalChinese16k
        );
        assert_eq!(
            TencentEngineModel::from_str("16k_ar"),
            TencentEngineModel::Arabic16k
        );
        assert_eq!(
            TencentEngineModel::from_str("16k_ja"),
            TencentEngineModel::Japanese16k
        );
        assert_eq!(
            TencentEngineModel::from_str("16k_ko"),
            TencentEngineModel::Korean16k
        );
        assert_eq!(
            TencentEngineModel::from_str("16k_th"),
            TencentEngineModel::Thai16k
        );
        assert_eq!(
            TencentEngineModel::from_str("16k_vi"),
            TencentEngineModel::Vietnamese16k
        );
        assert_eq!(
            TencentEngineModel::from_str("16k_id"),
            TencentEngineModel::Indonesian16k
        );
        assert_eq!(
            TencentEngineModel::from_str("16k_ms"),
            TencentEngineModel::Malay16k
        );
    }

    #[test]
    fn test_engine_model_sample_rate() {
        assert_eq!(TencentEngineModel::Mandarin8k.sample_rate(), 8000);
        assert_eq!(TencentEngineModel::MandarinShort8k.sample_rate(), 8000);
        assert_eq!(TencentEngineModel::Mandarin16k.sample_rate(), 16000);
        assert_eq!(TencentEngineModel::English16k.sample_rate(), 16000);
    }

    #[test]
    fn test_engine_model_as_str() {
        assert_eq!(TencentEngineModel::Mandarin16k.as_str(), "16k_zh");
        assert_eq!(TencentEngineModel::English16k.as_str(), "16k_en");
        assert_eq!(TencentEngineModel::Cantonese16k.as_str(), "16k_ca");
    }

    #[test]
    fn test_engine_model_language_code() {
        assert_eq!(TencentEngineModel::Mandarin16k.language_code(), "zh");
        assert_eq!(TencentEngineModel::English16k.language_code(), "en");
        assert_eq!(TencentEngineModel::Cantonese16k.language_code(), "yue");
        assert_eq!(TencentEngineModel::Japanese16k.language_code(), "ja");
    }

    #[test]
    fn test_engine_model_all() {
        let all = TencentEngineModel::all();
        assert_eq!(all.len(), 17); // Updated count with all new models
        assert!(all.contains(&TencentEngineModel::Mandarin16k));
        assert!(all.contains(&TencentEngineModel::English16k));
        assert!(all.contains(&TencentEngineModel::MandarinLarge16k));
        assert!(all.contains(&TencentEngineModel::CantoneseYue16k));
        assert!(all.contains(&TencentEngineModel::TraditionalChinese16k));
        assert!(all.contains(&TencentEngineModel::Arabic16k));
        assert!(all.contains(&TencentEngineModel::Malay16k));
    }

    #[test]
    fn test_engine_model_default() {
        assert_eq!(
            TencentEngineModel::default(),
            TencentEngineModel::Mandarin16k
        );
    }

    #[test]
    fn test_engine_model_unknown_defaults_to_mandarin() {
        assert_eq!(
            TencentEngineModel::from_str("unknown_model"),
            TencentEngineModel::Mandarin16k
        );
    }

    // =========================================================================
    // Audio Format Tests
    // =========================================================================

    #[test]
    fn test_audio_format_values() {
        assert_eq!(TencentAudioFormat::Pcm.value(), 1);
        assert_eq!(TencentAudioFormat::Speex.value(), 4);
        assert_eq!(TencentAudioFormat::Silk.value(), 6);
        assert_eq!(TencentAudioFormat::Mp3.value(), 8);
        assert_eq!(TencentAudioFormat::Opus.value(), 10);
        assert_eq!(TencentAudioFormat::Wav.value(), 12);
        assert_eq!(TencentAudioFormat::M4a.value(), 14);
        assert_eq!(TencentAudioFormat::Aac.value(), 16);
    }

    #[test]
    fn test_audio_format_parsing() {
        assert_eq!(
            TencentAudioFormat::from_str("pcm"),
            Some(TencentAudioFormat::Pcm)
        );
        assert_eq!(
            TencentAudioFormat::from_str("linear16"),
            Some(TencentAudioFormat::Pcm)
        );
        assert_eq!(
            TencentAudioFormat::from_str("speex"),
            Some(TencentAudioFormat::Speex)
        );
        assert_eq!(
            TencentAudioFormat::from_str("silk"),
            Some(TencentAudioFormat::Silk)
        );
        assert_eq!(
            TencentAudioFormat::from_str("mp3"),
            Some(TencentAudioFormat::Mp3)
        );
        assert_eq!(
            TencentAudioFormat::from_str("opus"),
            Some(TencentAudioFormat::Opus)
        );
        assert_eq!(
            TencentAudioFormat::from_str("wav"),
            Some(TencentAudioFormat::Wav)
        );
        assert_eq!(
            TencentAudioFormat::from_str("m4a"),
            Some(TencentAudioFormat::M4a)
        );
        assert_eq!(
            TencentAudioFormat::from_str("aac"),
            Some(TencentAudioFormat::Aac)
        );
        assert_eq!(TencentAudioFormat::from_str("invalid"), None);
    }

    #[test]
    fn test_audio_format_from_value() {
        assert_eq!(
            TencentAudioFormat::from_value(1),
            Some(TencentAudioFormat::Pcm)
        );
        assert_eq!(
            TencentAudioFormat::from_value(4),
            Some(TencentAudioFormat::Speex)
        );
        assert_eq!(
            TencentAudioFormat::from_value(10),
            Some(TencentAudioFormat::Opus)
        );
        assert_eq!(TencentAudioFormat::from_value(99), None);
    }

    #[test]
    fn test_audio_format_as_str() {
        assert_eq!(TencentAudioFormat::Pcm.as_str(), "pcm");
        assert_eq!(TencentAudioFormat::Speex.as_str(), "speex");
        assert_eq!(TencentAudioFormat::Opus.as_str(), "opus");
    }

    #[test]
    fn test_audio_format_default() {
        assert_eq!(TencentAudioFormat::default(), TencentAudioFormat::Speex);
    }

    #[test]
    fn test_audio_format_all() {
        let all = TencentAudioFormat::all();
        assert_eq!(all.len(), 8);
    }

    // =========================================================================
    // Word Info Tests
    // =========================================================================

    #[test]
    fn test_word_info_values() {
        assert_eq!(TencentWordInfo::None.value(), 0);
        assert_eq!(TencentWordInfo::Basic.value(), 1);
        assert_eq!(TencentWordInfo::Detailed.value(), 2);
    }

    #[test]
    fn test_word_info_from_value() {
        assert_eq!(TencentWordInfo::from_value(0), TencentWordInfo::None);
        assert_eq!(TencentWordInfo::from_value(1), TencentWordInfo::Basic);
        assert_eq!(TencentWordInfo::from_value(2), TencentWordInfo::Detailed);
        assert_eq!(TencentWordInfo::from_value(99), TencentWordInfo::None);
    }

    #[test]
    fn test_word_info_default() {
        assert_eq!(TencentWordInfo::default(), TencentWordInfo::None);
    }

    // =========================================================================
    // Configuration Tests
    // =========================================================================

    #[test]
    fn test_config_new() {
        let config = TencentSttConfig::new("secret_id", "secret_key", "app_id");
        assert_eq!(config.secret_id, "secret_id");
        assert_eq!(config.secret_key, "secret_key");
        assert_eq!(config.app_id, "app_id");
    }

    #[test]
    fn test_config_builder_methods() {
        let config = TencentSttConfig::new("id", "key", "app")
            .with_engine_model(TencentEngineModel::English16k)
            .with_voice_format(TencentAudioFormat::Opus)
            .with_vad(true)
            .with_vad_silence_time(500)
            .with_filter_dirty(TencentFilterDirtyMode::Filter)
            .with_filter_modal(TencentFilterModalMode::Partial)
            .with_filter_punc(true)
            .with_word_info(TencentWordInfo::Detailed)
            .with_hotword_id("vocab123");

        assert_eq!(config.engine_model_type, TencentEngineModel::English16k);
        assert_eq!(config.voice_format, TencentAudioFormat::Opus);
        assert!(config.needvad);
        assert_eq!(config.vad_silence_time, Some(500));
        assert_eq!(config.filter_dirty, TencentFilterDirtyMode::Filter);
        assert_eq!(config.filter_modal, TencentFilterModalMode::Partial);
        assert!(config.filter_punc);
        assert_eq!(config.word_info, TencentWordInfo::Detailed);
        assert_eq!(config.hotword_id, Some("vocab123".to_string()));
    }

    #[test]
    fn test_config_vad_silence_time_clamping() {
        let config = TencentSttConfig::new("id", "key", "app").with_vad_silence_time(100); // Below minimum
        assert_eq!(config.vad_silence_time, Some(VAD_SILENCE_TIME_MIN));

        let config = TencentSttConfig::new("id", "key", "app").with_vad_silence_time(5000); // Above maximum
        assert_eq!(config.vad_silence_time, Some(VAD_SILENCE_TIME_MAX));
    }

    #[test]
    fn test_config_validation_valid() {
        let config = TencentSttConfig::new("secret_id", "secret_key", "app_id");
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validation_empty_secret_id() {
        let config = TencentSttConfig::new("", "secret_key", "app_id");
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_empty_secret_key() {
        let config = TencentSttConfig::new("secret_id", "", "app_id");
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_empty_app_id() {
        let config = TencentSttConfig::new("secret_id", "secret_key", "");
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_sample_rate() {
        let config = TencentSttConfig::new("id", "key", "app")
            .with_engine_model(TencentEngineModel::Mandarin16k);
        assert_eq!(config.sample_rate(), 16000);

        let config = TencentSttConfig::new("id", "key", "app")
            .with_engine_model(TencentEngineModel::Mandarin8k);
        assert_eq!(config.sample_rate(), 8000);
    }

    #[test]
    fn test_config_chunk_size() {
        // 40ms at 16kHz, 16-bit: 16000 * 2 * 40 / 1000 = 1280 bytes
        let config = TencentSttConfig::new("id", "key", "app")
            .with_engine_model(TencentEngineModel::Mandarin16k);
        assert_eq!(config.get_chunk_size(), 1280);

        // 40ms at 8kHz, 16-bit: 8000 * 2 * 40 / 1000 = 640 bytes
        let config = TencentSttConfig::new("id", "key", "app")
            .with_engine_model(TencentEngineModel::Mandarin8k);
        assert_eq!(config.get_chunk_size(), 640);
    }

    #[test]
    fn test_config_ws_url_base() {
        let config = TencentSttConfig::new("id", "key", "12345678");
        let url = config.get_ws_url_base();
        assert_eq!(url, "wss://asr.cloud.tencent.com/asr/v2/12345678");
    }

    #[test]
    fn test_config_supported_languages() {
        let languages = TencentSttConfig::supported_languages();
        assert!(languages.contains(&"zh"));
        assert!(languages.contains(&"en"));
        assert!(languages.contains(&"ja"));
        assert!(languages.contains(&"ko"));
    }

    #[test]
    fn test_config_supported_models() {
        let models = TencentSttConfig::supported_models();
        assert!(models.contains(&"16k_zh"));
        assert!(models.contains(&"16k_en"));
        assert!(models.contains(&"8k_zh"));
    }

    // =========================================================================
    // From Base Config Tests
    // =========================================================================

    #[test]
    fn test_from_base_config_valid() {
        let base = STTConfig {
            api_key: "my_secret_id|my_secret_key|my_app_id".to_string(),
            language: "zh".to_string(),
            sample_rate: 16000,
            encoding: "pcm".to_string(),
            model: "16k_zh".to_string(),
            ..Default::default()
        };

        let config = TencentSttConfig::from_base(base).unwrap();
        assert_eq!(config.secret_id, "my_secret_id");
        assert_eq!(config.secret_key, "my_secret_key");
        assert_eq!(config.app_id, "my_app_id");
        assert_eq!(config.engine_model_type, TencentEngineModel::Mandarin16k);
        assert_eq!(config.voice_format, TencentAudioFormat::Pcm);
    }

    #[test]
    fn test_from_base_config_invalid_format_no_pipe() {
        let base = STTConfig {
            api_key: "just_one_value".to_string(),
            ..Default::default()
        };

        assert!(TencentSttConfig::from_base(base).is_err());
    }

    #[test]
    fn test_from_base_config_invalid_format_two_parts() {
        let base = STTConfig {
            api_key: "id|key".to_string(),
            ..Default::default()
        };

        assert!(TencentSttConfig::from_base(base).is_err());
    }

    #[test]
    fn test_from_base_config_empty_parts() {
        let base = STTConfig {
            api_key: "||".to_string(),
            ..Default::default()
        };

        assert!(TencentSttConfig::from_base(base).is_err());
    }

    #[test]
    fn test_from_base_config_english_model() {
        let base = STTConfig {
            api_key: "id|key|app".to_string(),
            model: "english".to_string(),
            encoding: "opus".to_string(),
            ..Default::default()
        };

        let config = TencentSttConfig::from_base(base).unwrap();
        assert_eq!(config.engine_model_type, TencentEngineModel::English16k);
        assert_eq!(config.voice_format, TencentAudioFormat::Opus);
    }

    #[test]
    fn test_from_base_config_default_format() {
        let base = STTConfig {
            api_key: "id|key|app".to_string(),
            encoding: "unknown_format".to_string(),
            ..Default::default()
        };

        let config = TencentSttConfig::from_base(base).unwrap();
        assert_eq!(config.voice_format, TencentAudioFormat::Speex); // Default
    }

    // =========================================================================
    // Default Tests
    // =========================================================================

    #[test]
    fn test_config_default() {
        let config = TencentSttConfig::default();
        assert!(config.secret_id.is_empty());
        assert!(config.secret_key.is_empty());
        assert!(config.app_id.is_empty());
        assert_eq!(config.engine_model_type, TencentEngineModel::Mandarin16k);
        assert_eq!(config.voice_format, TencentAudioFormat::Speex);
        assert!(config.needvad);
        assert_eq!(config.filter_dirty, TencentFilterDirtyMode::Off);
        assert_eq!(config.filter_modal, TencentFilterModalMode::Off);
        assert!(!config.filter_punc);
        assert_eq!(config.word_info, TencentWordInfo::None);
        assert!(config.vad_silence_time.is_none());
        assert!(config.hotword_id.is_none());
    }

    // =========================================================================
    // Constants Tests
    // =========================================================================

    #[test]
    fn test_constants() {
        assert_eq!(TENCENT_ASR_WS_URL, "wss://asr.cloud.tencent.com/asr/v2");
        assert_eq!(DEFAULT_ENGINE_MODEL, "16k_zh");
        assert_eq!(DEFAULT_VOICE_FORMAT, 4);
        assert_eq!(DEFAULT_SAMPLE_RATE, 16000);
        assert_eq!(RECOMMENDED_CHUNK_DURATION_MS, 40);
        assert_eq!(SIGNATURE_VALIDITY_SECS, 86400);
        assert_eq!(VAD_SILENCE_TIME_MIN, 240);
        assert_eq!(VAD_SILENCE_TIME_MAX, 2000);
    }
}
