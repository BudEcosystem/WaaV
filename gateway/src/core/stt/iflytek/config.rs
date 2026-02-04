//! iFlytek STT Configuration Module
//!
//! This module provides configuration types for the iFlytek STT provider,
//! including language selection, audio encoding, and API parameters.
//!
//! # Supported Languages
//!
//! iFlytek supports 30+ languages including:
//! - Chinese (Mandarin, Cantonese, 23+ dialects)
//! - English, Japanese, Korean
//! - Russian, French, Spanish, German
//! - Thai, Vietnamese, Arabic, and more
//!
//! # Audio Requirements
//!
//! - Sample Rate: 16000 Hz (recommended) or 8000 Hz
//! - Bit Depth: 16-bit
//! - Channels: Mono
//! - Encoding: PCM, MP3 (Mandarin/English only), Speex

use super::auth::IFlytekAuth;
use crate::core::stt::base::{STTConfig, STTError};

// =============================================================================
// Constants
// =============================================================================

/// Short Form ASR WebSocket endpoint (Voice Dictation).
pub const IFLYTEK_IAT_ENDPOINT: &str = "wss://iat-api-sg.xf-yun.com/v2/iat";

/// Real-time ASR WebSocket endpoint (Streaming).
pub const IFLYTEK_IST_ENDPOINT: &str = "wss://ist-api-sg.xf-yun.com/v2/ist";

/// Short Form ASR host.
pub const IFLYTEK_IAT_HOST: &str = "iat-api-sg.xf-yun.com";

/// Real-time ASR host.
pub const IFLYTEK_IST_HOST: &str = "ist-api-sg.xf-yun.com";

/// Short Form ASR path.
pub const IFLYTEK_IAT_PATH: &str = "/v2/iat";

/// Real-time ASR path.
pub const IFLYTEK_IST_PATH: &str = "/v2/ist";

/// Default sample rate for iFlytek.
pub const DEFAULT_SAMPLE_RATE: u32 = 16000;

/// Default frame size in bytes (PCM @ 16kHz, 40ms).
pub const DEFAULT_FRAME_SIZE: usize = 1280;

/// Default frame interval in milliseconds.
pub const DEFAULT_FRAME_INTERVAL_MS: u64 = 40;

/// Maximum audio duration for Short Form ASR (60 seconds).
pub const MAX_SHORT_FORM_DURATION_SECS: u64 = 60;

/// Maximum audio duration for Real-time ASR (5 hours).
pub const MAX_REALTIME_DURATION_SECS: u64 = 5 * 60 * 60;

/// Default VAD end-of-speech timeout in milliseconds.
pub const DEFAULT_VAD_EOS_MS: u32 = 2000;

// =============================================================================
// Language Enum
// =============================================================================

/// Supported languages for iFlytek STT.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IFlytekLanguage {
    /// Chinese Mandarin (default).
    #[default]
    ChineseMandarin,
    /// Chinese Cantonese.
    ChineseCantonese,
    /// English.
    English,
    /// Japanese.
    Japanese,
    /// Korean.
    Korean,
    /// Russian.
    Russian,
    /// French.
    French,
    /// Spanish.
    Spanish,
    /// German.
    German,
    /// Thai.
    Thai,
    /// Vietnamese.
    Vietnamese,
    /// Arabic.
    Arabic,
    /// Indonesian.
    Indonesian,
    /// Malay.
    Malay,
    /// Hindi.
    Hindi,
    /// Portuguese.
    Portuguese,
    /// Turkish.
    Turkish,
    /// Italian.
    Italian,
}

impl IFlytekLanguage {
    /// Get the language code for the API.
    pub fn as_code(&self) -> &'static str {
        match self {
            Self::ChineseMandarin => "zh_cn",
            Self::ChineseCantonese => "zh_hk",
            Self::English => "en_us",
            Self::Japanese => "ja_jp",
            Self::Korean => "ko_kr",
            Self::Russian => "ru_ru",
            Self::French => "fr_fr",
            Self::Spanish => "es_es",
            Self::German => "de_de",
            Self::Thai => "th_th",
            Self::Vietnamese => "vi_vn",
            Self::Arabic => "ar_ae",
            Self::Indonesian => "id_id",
            Self::Malay => "ms_my",
            Self::Hindi => "hi_in",
            Self::Portuguese => "pt_pt",
            Self::Turkish => "tr_tr",
            Self::Italian => "it_it",
        }
    }

    /// Get the display name for the language.
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::ChineseMandarin => "Chinese (Mandarin)",
            Self::ChineseCantonese => "Chinese (Cantonese)",
            Self::English => "English (US)",
            Self::Japanese => "Japanese",
            Self::Korean => "Korean",
            Self::Russian => "Russian",
            Self::French => "French",
            Self::Spanish => "Spanish",
            Self::German => "German",
            Self::Thai => "Thai",
            Self::Vietnamese => "Vietnamese",
            Self::Arabic => "Arabic",
            Self::Indonesian => "Indonesian",
            Self::Malay => "Malay",
            Self::Hindi => "Hindi",
            Self::Portuguese => "Portuguese",
            Self::Turkish => "Turkish",
            Self::Italian => "Italian",
        }
    }

    /// Get the default accent for the language.
    pub fn default_accent(&self) -> &'static str {
        match self {
            Self::ChineseMandarin => "mandarin",
            Self::ChineseCantonese => "cantonese",
            _ => "mandarin", // Non-Chinese languages don't use accent
        }
    }

    /// Parse language from code.
    pub fn from_code(code: &str) -> Option<Self> {
        let normalized = code.trim().to_lowercase();
        match normalized.as_str() {
            "zh_cn" | "zh-cn" | "zh" | "chinese" | "mandarin" => Some(Self::ChineseMandarin),
            "zh_hk" | "zh-hk" | "cantonese" | "yue" => Some(Self::ChineseCantonese),
            "en_us" | "en-us" | "en" | "english" => Some(Self::English),
            "ja_jp" | "ja-jp" | "ja" | "japanese" => Some(Self::Japanese),
            "ko_kr" | "ko-kr" | "ko" | "korean" => Some(Self::Korean),
            "ru_ru" | "ru-ru" | "ru" | "russian" => Some(Self::Russian),
            "fr_fr" | "fr-fr" | "fr" | "french" => Some(Self::French),
            "es_es" | "es-es" | "es" | "spanish" => Some(Self::Spanish),
            "de_de" | "de-de" | "de" | "german" => Some(Self::German),
            "th_th" | "th-th" | "th" | "thai" => Some(Self::Thai),
            "vi_vn" | "vi-vn" | "vi" | "vietnamese" => Some(Self::Vietnamese),
            "ar_ae" | "ar-ae" | "ar" | "arabic" => Some(Self::Arabic),
            "id_id" | "id-id" | "id" | "indonesian" => Some(Self::Indonesian),
            "ms_my" | "ms-my" | "ms" | "malay" => Some(Self::Malay),
            "hi_in" | "hi-in" | "hi" | "hindi" => Some(Self::Hindi),
            "pt_pt" | "pt-pt" | "pt" | "portuguese" => Some(Self::Portuguese),
            "tr_tr" | "tr-tr" | "tr" | "turkish" => Some(Self::Turkish),
            "it_it" | "it-it" | "it" | "italian" => Some(Self::Italian),
            _ => None,
        }
    }

    /// Get all supported languages.
    pub fn all() -> &'static [Self] {
        &[
            Self::ChineseMandarin,
            Self::ChineseCantonese,
            Self::English,
            Self::Japanese,
            Self::Korean,
            Self::Russian,
            Self::French,
            Self::Spanish,
            Self::German,
            Self::Thai,
            Self::Vietnamese,
            Self::Arabic,
            Self::Indonesian,
            Self::Malay,
            Self::Hindi,
            Self::Portuguese,
            Self::Turkish,
            Self::Italian,
        ]
    }
}

// =============================================================================
// Audio Encoding Enum
// =============================================================================

/// Audio encoding formats supported by iFlytek.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IFlytekAudioEncoding {
    /// Raw PCM (16-bit, signed, little-endian).
    #[default]
    Raw,
    /// Speex encoding.
    Speex,
    /// Speex wideband encoding.
    SpeexWb,
    /// MP3 encoding (Mandarin/English only).
    Mp3,
}

impl IFlytekAudioEncoding {
    /// Get the encoding string for the API.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::Speex => "speex",
            Self::SpeexWb => "speex-wb",
            Self::Mp3 => "lame",
        }
    }

    /// Get the MIME type for the encoding.
    pub fn mime_type(&self) -> &'static str {
        match self {
            Self::Raw => "audio/L16;rate=16000",
            Self::Speex => "audio/speex;rate=8000",
            Self::SpeexWb => "audio/speex;rate=16000",
            Self::Mp3 => "audio/mpeg",
        }
    }

    /// Parse from string.
    pub fn from_str(s: &str) -> Option<Self> {
        let normalized = s.trim().to_lowercase();
        match normalized.as_str() {
            "raw" | "pcm" | "linear16" => Some(Self::Raw),
            "speex" => Some(Self::Speex),
            "speex-wb" | "speex_wb" => Some(Self::SpeexWb),
            "mp3" | "lame" | "mpeg" => Some(Self::Mp3),
            _ => None,
        }
    }
}

// =============================================================================
// ASR Domain Enum
// =============================================================================

/// ASR recognition domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IFlytekAsrDomain {
    /// General daily conversation (default).
    #[default]
    Iat,
    /// Medical domain.
    Medical,
}

impl IFlytekAsrDomain {
    /// Get the domain string for the API.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Iat => "iat",
            Self::Medical => "medical",
        }
    }
}

// =============================================================================
// ASR Mode Enum
// =============================================================================

/// ASR operation mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IFlytekAsrMode {
    /// Short Form ASR (Voice Dictation) - max 60 seconds.
    #[default]
    ShortForm,
    /// Real-time ASR (Streaming) - up to 5 hours.
    Realtime,
}

impl IFlytekAsrMode {
    /// Get the WebSocket host for this mode.
    pub fn host(&self) -> &'static str {
        match self {
            Self::ShortForm => IFLYTEK_IAT_HOST,
            Self::Realtime => IFLYTEK_IST_HOST,
        }
    }

    /// Get the WebSocket path for this mode.
    pub fn path(&self) -> &'static str {
        match self {
            Self::ShortForm => IFLYTEK_IAT_PATH,
            Self::Realtime => IFLYTEK_IST_PATH,
        }
    }

    /// Get the maximum duration in seconds.
    pub fn max_duration_secs(&self) -> u64 {
        match self {
            Self::ShortForm => MAX_SHORT_FORM_DURATION_SECS,
            Self::Realtime => MAX_REALTIME_DURATION_SECS,
        }
    }
}

// =============================================================================
// Configuration Struct
// =============================================================================

/// iFlytek STT configuration.
#[derive(Debug, Clone)]
pub struct IFlytekSttConfig {
    /// Authentication credentials.
    pub auth: IFlytekAuth,
    /// Recognition language.
    pub language: IFlytekLanguage,
    /// Audio encoding format.
    pub encoding: IFlytekAudioEncoding,
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// ASR domain.
    pub domain: IFlytekAsrDomain,
    /// ASR mode (short form or real-time).
    pub mode: IFlytekAsrMode,
    /// Accent for Chinese languages.
    pub accent: String,
    /// VAD end-of-speech timeout in milliseconds.
    pub vad_eos_ms: u32,
    /// Enable dynamic word correction (Chinese only).
    pub dynamic_correction: bool,
    /// Enable punctuation.
    pub punctuation: bool,
    /// Convert numbers to digits.
    pub convert_numbers: bool,
}

impl Default for IFlytekSttConfig {
    fn default() -> Self {
        Self {
            auth: IFlytekAuth::new(String::new(), String::new(), String::new()),
            language: IFlytekLanguage::default(),
            encoding: IFlytekAudioEncoding::default(),
            sample_rate: DEFAULT_SAMPLE_RATE,
            domain: IFlytekAsrDomain::default(),
            mode: IFlytekAsrMode::default(),
            accent: "mandarin".to_string(),
            vad_eos_ms: DEFAULT_VAD_EOS_MS,
            dynamic_correction: true,
            punctuation: true,
            convert_numbers: true,
        }
    }
}

impl IFlytekSttConfig {
    /// Create configuration from base STTConfig.
    ///
    /// # API Key Format
    /// `app_id|api_key|api_secret`
    pub fn from_base(config: STTConfig) -> Result<Self, STTError> {
        // Parse authentication credentials
        let auth = IFlytekAuth::from_combined(&config.api_key)
            .map_err(|e| STTError::AuthenticationFailed(e.to_string()))?;

        // Parse language
        let language =
            IFlytekLanguage::from_code(&config.language).unwrap_or(IFlytekLanguage::default());

        // Parse encoding
        let encoding = IFlytekAudioEncoding::from_str(&config.encoding)
            .unwrap_or(IFlytekAudioEncoding::default());

        // Determine ASR mode from model field
        let mode = if config.model.to_lowercase().contains("realtime")
            || config.model.to_lowercase().contains("stream")
            || config.model.to_lowercase().contains("ist")
        {
            IFlytekAsrMode::Realtime
        } else {
            IFlytekAsrMode::ShortForm
        };

        Ok(Self {
            auth,
            language,
            encoding,
            sample_rate: config.sample_rate,
            domain: IFlytekAsrDomain::default(),
            mode,
            accent: language.default_accent().to_string(),
            vad_eos_ms: DEFAULT_VAD_EOS_MS,
            dynamic_correction: true,
            punctuation: config.punctuation,
            convert_numbers: true,
        })
    }

    /// Validate the configuration.
    pub fn validate(&self) -> Result<(), STTError> {
        self.auth
            .validate()
            .map_err(|e| STTError::AuthenticationFailed(e.to_string()))?;

        if self.sample_rate != 8000 && self.sample_rate != 16000 {
            return Err(STTError::ConfigurationError(format!(
                "Invalid sample rate: {}. Must be 8000 or 16000 Hz.",
                self.sample_rate
            )));
        }

        // MP3 is only supported for Mandarin and English
        if matches!(self.encoding, IFlytekAudioEncoding::Mp3)
            && !matches!(
                self.language,
                IFlytekLanguage::ChineseMandarin | IFlytekLanguage::English
            )
        {
            return Err(STTError::ConfigurationError(
                "MP3 encoding is only supported for Mandarin Chinese and English".to_string(),
            ));
        }

        Ok(())
    }

    /// Get the WebSocket host for the configured mode.
    pub fn host(&self) -> &'static str {
        self.mode.host()
    }

    /// Get the WebSocket path for the configured mode.
    pub fn path(&self) -> &'static str {
        self.mode.path()
    }

    /// Build the audio format string for the API.
    pub fn audio_format_string(&self) -> String {
        format!("audio/L16;rate={}", self.sample_rate)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_api_key() -> String {
        "test_app_id|test_api_key_xxxxx|test_api_secret_xx".to_string()
    }

    fn create_test_base_config() -> STTConfig {
        STTConfig {
            api_key: create_test_api_key(),
            language: "zh_cn".to_string(),
            sample_rate: 16000,
            encoding: "raw".to_string(),
            punctuation: true,
            ..Default::default()
        }
    }

    // Language tests
    #[test]
    fn test_language_codes() {
        assert_eq!(IFlytekLanguage::ChineseMandarin.as_code(), "zh_cn");
        assert_eq!(IFlytekLanguage::ChineseCantonese.as_code(), "zh_hk");
        assert_eq!(IFlytekLanguage::English.as_code(), "en_us");
        assert_eq!(IFlytekLanguage::Japanese.as_code(), "ja_jp");
    }

    #[test]
    fn test_language_from_code() {
        assert_eq!(
            IFlytekLanguage::from_code("zh_cn"),
            Some(IFlytekLanguage::ChineseMandarin)
        );
        assert_eq!(
            IFlytekLanguage::from_code("zh-cn"),
            Some(IFlytekLanguage::ChineseMandarin)
        );
        assert_eq!(
            IFlytekLanguage::from_code("en"),
            Some(IFlytekLanguage::English)
        );
        assert_eq!(
            IFlytekLanguage::from_code("JAPANESE"),
            Some(IFlytekLanguage::Japanese)
        );
        assert_eq!(IFlytekLanguage::from_code("unknown"), None);
    }

    #[test]
    fn test_language_display_names() {
        assert_eq!(
            IFlytekLanguage::ChineseMandarin.display_name(),
            "Chinese (Mandarin)"
        );
        assert_eq!(IFlytekLanguage::English.display_name(), "English (US)");
    }

    #[test]
    fn test_language_default_accent() {
        assert_eq!(
            IFlytekLanguage::ChineseMandarin.default_accent(),
            "mandarin"
        );
        assert_eq!(
            IFlytekLanguage::ChineseCantonese.default_accent(),
            "cantonese"
        );
        assert_eq!(IFlytekLanguage::English.default_accent(), "mandarin");
    }

    #[test]
    fn test_language_all() {
        let all = IFlytekLanguage::all();
        assert!(all.len() >= 18);
        assert!(all.contains(&IFlytekLanguage::ChineseMandarin));
        assert!(all.contains(&IFlytekLanguage::English));
    }

    // Encoding tests
    #[test]
    fn test_encoding_strings() {
        assert_eq!(IFlytekAudioEncoding::Raw.as_str(), "raw");
        assert_eq!(IFlytekAudioEncoding::Speex.as_str(), "speex");
        assert_eq!(IFlytekAudioEncoding::Mp3.as_str(), "lame");
    }

    #[test]
    fn test_encoding_from_str() {
        assert_eq!(
            IFlytekAudioEncoding::from_str("raw"),
            Some(IFlytekAudioEncoding::Raw)
        );
        assert_eq!(
            IFlytekAudioEncoding::from_str("pcm"),
            Some(IFlytekAudioEncoding::Raw)
        );
        assert_eq!(
            IFlytekAudioEncoding::from_str("mp3"),
            Some(IFlytekAudioEncoding::Mp3)
        );
        assert_eq!(IFlytekAudioEncoding::from_str("unknown"), None);
    }

    #[test]
    fn test_encoding_mime_types() {
        assert_eq!(
            IFlytekAudioEncoding::Raw.mime_type(),
            "audio/L16;rate=16000"
        );
        assert_eq!(IFlytekAudioEncoding::Mp3.mime_type(), "audio/mpeg");
    }

    // ASR Domain tests
    #[test]
    fn test_domain_strings() {
        assert_eq!(IFlytekAsrDomain::Iat.as_str(), "iat");
        assert_eq!(IFlytekAsrDomain::Medical.as_str(), "medical");
    }

    // ASR Mode tests
    #[test]
    fn test_mode_hosts() {
        assert_eq!(IFlytekAsrMode::ShortForm.host(), IFLYTEK_IAT_HOST);
        assert_eq!(IFlytekAsrMode::Realtime.host(), IFLYTEK_IST_HOST);
    }

    #[test]
    fn test_mode_paths() {
        assert_eq!(IFlytekAsrMode::ShortForm.path(), IFLYTEK_IAT_PATH);
        assert_eq!(IFlytekAsrMode::Realtime.path(), IFLYTEK_IST_PATH);
    }

    #[test]
    fn test_mode_max_duration() {
        assert_eq!(IFlytekAsrMode::ShortForm.max_duration_secs(), 60);
        assert_eq!(IFlytekAsrMode::Realtime.max_duration_secs(), 5 * 60 * 60);
    }

    // Config tests
    #[test]
    fn test_config_from_base_valid() {
        let base = create_test_base_config();
        let config = IFlytekSttConfig::from_base(base);
        assert!(config.is_ok());

        let config = config.unwrap();
        assert_eq!(config.language, IFlytekLanguage::ChineseMandarin);
        assert_eq!(config.sample_rate, 16000);
        assert_eq!(config.encoding, IFlytekAudioEncoding::Raw);
    }

    #[test]
    fn test_config_from_base_invalid_api_key() {
        let base = STTConfig {
            api_key: "invalid_key_format".to_string(),
            ..Default::default()
        };
        let config = IFlytekSttConfig::from_base(base);
        assert!(config.is_err());
    }

    #[test]
    fn test_config_from_base_realtime_mode() {
        let base = STTConfig {
            api_key: create_test_api_key(),
            model: "realtime".to_string(),
            ..Default::default()
        };
        let config = IFlytekSttConfig::from_base(base).unwrap();
        assert_eq!(config.mode, IFlytekAsrMode::Realtime);
    }

    #[test]
    fn test_config_validation_valid() {
        let base = create_test_base_config();
        let config = IFlytekSttConfig::from_base(base).unwrap();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validation_invalid_sample_rate() {
        let mut config = IFlytekSttConfig::default();
        config.auth = IFlytekAuth::new("app".to_string(), "key".to_string(), "secret".to_string());
        config.sample_rate = 44100;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_mp3_non_chinese() {
        let mut config = IFlytekSttConfig::default();
        config.auth = IFlytekAuth::new("app".to_string(), "key".to_string(), "secret".to_string());
        config.language = IFlytekLanguage::Japanese;
        config.encoding = IFlytekAudioEncoding::Mp3;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_host_and_path() {
        let config = IFlytekSttConfig::default();
        assert_eq!(config.host(), IFLYTEK_IAT_HOST);
        assert_eq!(config.path(), IFLYTEK_IAT_PATH);
    }

    #[test]
    fn test_config_audio_format_string() {
        let mut config = IFlytekSttConfig::default();
        config.sample_rate = 16000;
        assert_eq!(config.audio_format_string(), "audio/L16;rate=16000");

        config.sample_rate = 8000;
        assert_eq!(config.audio_format_string(), "audio/L16;rate=8000");
    }

    // Constants tests
    #[test]
    fn test_constants() {
        assert_eq!(DEFAULT_SAMPLE_RATE, 16000);
        assert_eq!(DEFAULT_FRAME_SIZE, 1280);
        assert_eq!(DEFAULT_FRAME_INTERVAL_MS, 40);
        assert_eq!(DEFAULT_VAD_EOS_MS, 2000);
        assert_eq!(MAX_SHORT_FORM_DURATION_SECS, 60);
        assert_eq!(MAX_REALTIME_DURATION_SECS, 5 * 60 * 60);
    }

    #[test]
    fn test_endpoint_constants() {
        assert!(IFLYTEK_IAT_ENDPOINT.starts_with("wss://"));
        assert!(IFLYTEK_IST_ENDPOINT.starts_with("wss://"));
        assert!(IFLYTEK_IAT_ENDPOINT.contains("iat"));
        assert!(IFLYTEK_IST_ENDPOINT.contains("ist"));
    }
}
