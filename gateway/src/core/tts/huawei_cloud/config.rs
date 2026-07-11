//! Huawei Cloud Speech Interaction Service (SIS) TTS Configuration
//!
//! Configuration types and constants for Huawei Cloud TTS provider.
//!
//! # Overview
//!
//! Huawei Cloud SIS (华为云语音) provides Text-to-Speech services via:
//! - HTTP REST API for standard TTS
//! - WebSocket for real-time streaming TTS (RTTS)
//!
//! # Authentication
//!
//! Uses IAM token-based authentication with X-Auth-Token header.
//! The API key format is `username|password|domain_name|project_id` (pipe-separated).

use crate::core::tts::base::{TTSConfig, TTSError};
use serde::{Deserialize, Serialize};

fn validate_huawei_tts_endpoint(source: &str, endpoint: &str) -> Result<(), String> {
    let endpoint = endpoint.trim();
    if endpoint.is_empty() {
        return Ok(());
    }
    crate::core::net::validate_url_for_ssrf(endpoint, crate::core::net::HTTP_URL_SCHEMES)
        .map_err(|msg| format!("{source} rejected (SSRF protection): {msg}"))
}

// =============================================================================
// Constants
// =============================================================================

/// Maximum text length per TTS request (Chinese characters).
pub const MAX_TEXT_LENGTH: usize = 500;

/// Default sample rate.
pub const DEFAULT_SAMPLE_RATE: u32 = 16000;

/// Default volume (0-100).
pub const DEFAULT_VOLUME: u8 = 50;

/// Default speed (-500 to 500, 0 = normal).
pub const DEFAULT_SPEED: i16 = 0;

/// Default pitch (-500 to 500, 0 = normal).
pub const DEFAULT_PITCH: i16 = 0;

/// Token validity in seconds (24 hours).
pub const TOKEN_VALIDITY_SECS: u64 = 86400;

// =============================================================================
// Region Configuration
// =============================================================================

/// Huawei Cloud regions supporting SIS.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum HuaweiCloudRegion {
    /// CN North-Beijing4 (primary region).
    #[default]
    CnNorth4,
    /// CN East-Shanghai2.
    CnEast3,
    /// AP-Singapore.
    ApSoutheast3,
    /// CN-Hong Kong.
    ApSoutheast1,
    /// AP-Bangkok.
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

    /// Get the SIS API endpoint for this region.
    pub fn sis_endpoint(&self) -> &'static str {
        match self {
            Self::CnNorth4 => "sis.cn-north-4.myhuaweicloud.com",
            Self::CnEast3 => "sis.cn-east-3.myhuaweicloud.com",
            Self::ApSoutheast3 => "sis-ext.ap-southeast-3.myhuaweicloud.com",
            Self::ApSoutheast1 => "sis-ext.ap-southeast-1.myhuaweicloud.com",
            Self::ApSoutheast2 => "sis-ext.ap-southeast-2.myhuaweicloud.com",
        }
    }

    /// Get the IAM endpoint for this region.
    pub fn iam_endpoint(&self) -> String {
        format!(
            "https://iam.{}.myhuaweicloud.com/v3/auth/tokens",
            self.code()
        )
    }

    /// Parse region from string.
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().replace('_', "-").as_str() {
            "cn-north-4" | "beijing" | "beijing4" => Some(Self::CnNorth4),
            "cn-east-3" | "shanghai" | "shanghai2" => Some(Self::CnEast3),
            "ap-southeast-3" | "singapore" => Some(Self::ApSoutheast3),
            "ap-southeast-1" | "hong-kong" | "hongkong" => Some(Self::ApSoutheast1),
            "ap-southeast-2" | "bangkok" => Some(Self::ApSoutheast2),
            _ => None,
        }
    }

    /// Check if this region is in China.
    pub fn is_china(&self) -> bool {
        matches!(self, Self::CnNorth4 | Self::CnEast3)
    }

    /// Check if this region supports premium voices.
    pub fn supports_premium_voices(&self) -> bool {
        // Premium voices only available in cn-north-4 and cn-east-3
        matches!(self, Self::CnNorth4 | Self::CnEast3)
    }
}

// =============================================================================
// Voice Configuration
// =============================================================================

/// Huawei Cloud TTS voice identifiers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum HuaweiTtsVoice {
    // Standard Voices (普通发音人)
    /// 小琪 - Female, Standard
    XiaoQi,
    /// 小雯 - Female, Soft
    XiaoWen,
    /// 小燕 - Female, Gentle (default)
    #[default]
    XiaoYan,
    /// 小倩 - Female, Mature
    XiaoQian,
    /// 小婧 - Female, Lively
    XiaoJing,
    /// 小宇 - Male, Standard
    XiaoYu,
    /// 小宋 - Male, Passionate
    XiaoSong,
    /// 小王 - Child, Standard
    XiaoWang,
    /// 小呆 - Child, Cute
    XiaoDai,
    /// Cameal - Female, English
    Cameal,

    // Premium Voices (精品发音人) - cn-north-4 and cn-east-3 only
    /// 华小夏 - Female, Sales style
    HuaXiaoXia,
    /// 华小唯 - Female, Customer service
    HuaXiaoWei,
    /// 华晓刚 - Male, News style
    HuaXiaoGang,

    /// Custom voice by property string
    Custom(String),
}

impl HuaweiTtsVoice {
    /// Get the property string for API requests.
    pub fn as_property(&self) -> &str {
        match self {
            // Standard voices
            Self::XiaoQi => "chinese_xiaoqi_common",
            Self::XiaoWen => "chinese_xiaowen_common",
            Self::XiaoYan => "chinese_xiaoyan_common",
            Self::XiaoQian => "chinese_xiaoqian_common",
            Self::XiaoJing => "chinese_xiaojing_common",
            Self::XiaoYu => "chinese_xiaoyu_common",
            Self::XiaoSong => "chinese_xiaosong_common",
            Self::XiaoWang => "chinese_xiaowang_common",
            Self::XiaoDai => "chinese_xiaodai_common",
            Self::Cameal => "chinese_cameal_common",
            // Premium voices
            Self::HuaXiaoXia => "chinese_huaxiaoxia_common",
            Self::HuaXiaoWei => "chinese_huaxiaowei_common",
            Self::HuaXiaoGang => "chinese_huaxiaogang_common",
            Self::Custom(name) => name.as_str(),
        }
    }

    /// Get the display name for this voice.
    pub fn name(&self) -> &str {
        match self {
            Self::XiaoQi => "小琪 (Female, Standard)",
            Self::XiaoWen => "小雯 (Female, Soft)",
            Self::XiaoYan => "小燕 (Female, Gentle)",
            Self::XiaoQian => "小倩 (Female, Mature)",
            Self::XiaoJing => "小婧 (Female, Lively)",
            Self::XiaoYu => "小宇 (Male, Standard)",
            Self::XiaoSong => "小宋 (Male, Passionate)",
            Self::XiaoWang => "小王 (Child, Standard)",
            Self::XiaoDai => "小呆 (Child, Cute)",
            Self::Cameal => "Cameal (Female, English)",
            Self::HuaXiaoXia => "华小夏 (Female, Sales)",
            Self::HuaXiaoWei => "华小唯 (Female, Customer Service)",
            Self::HuaXiaoGang => "华晓刚 (Male, News)",
            Self::Custom(_) => "Custom Voice",
        }
    }

    /// Check if this is a premium voice.
    pub fn is_premium(&self) -> bool {
        matches!(
            self,
            Self::HuaXiaoXia | Self::HuaXiaoWei | Self::HuaXiaoGang | Self::Custom(_)
        )
    }

    /// Check if pitch adjustment is supported for this voice.
    /// Premium voices do not support pitch adjustment.
    pub fn supports_pitch(&self) -> bool {
        !self.is_premium()
    }

    /// Parse voice from string.
    pub fn from_str(s: &str) -> Self {
        let normalized = s.to_lowercase().replace(['-', '_'], "");
        match normalized.as_str() {
            "xiaoqi" | "小琪" => Self::XiaoQi,
            "xiaowen" | "小雯" => Self::XiaoWen,
            "xiaoyan" | "小燕" => Self::XiaoYan,
            "xiaoqian" | "小倩" => Self::XiaoQian,
            "xiaojing" | "小婧" => Self::XiaoJing,
            "xiaoyu" | "小宇" => Self::XiaoYu,
            "xiaosong" | "小宋" => Self::XiaoSong,
            "xiaowang" | "小王" => Self::XiaoWang,
            "xiaodai" | "小呆" => Self::XiaoDai,
            "cameal" => Self::Cameal,
            "huaxiaoxia" | "华小夏" => Self::HuaXiaoXia,
            "huaxiaowei" | "华小唯" => Self::HuaXiaoWei,
            "huaxiaogang" | "华晓刚" => Self::HuaXiaoGang,
            _ => {
                // If it looks like a property string, use it directly
                if s.contains("_") {
                    Self::Custom(s.to_string())
                } else {
                    Self::XiaoYan // Default voice
                }
            }
        }
    }
}

// =============================================================================
// Audio Format Configuration
// =============================================================================

/// Huawei Cloud TTS audio output format.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum HuaweiTtsAudioFormat {
    /// WAV format (default).
    #[default]
    Wav,
    /// MP3 format.
    Mp3,
    /// Raw PCM format.
    Pcm,
}

impl HuaweiTtsAudioFormat {
    /// Get the format string for API requests.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Wav => "wav",
            Self::Mp3 => "mp3",
            Self::Pcm => "pcm",
        }
    }

    /// Get the MIME type for this format.
    pub fn mime_type(&self) -> &'static str {
        match self {
            Self::Wav => "audio/wav",
            Self::Mp3 => "audio/mp3",
            Self::Pcm => "audio/pcm",
        }
    }

    /// Parse format from string.
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "wav" => Some(Self::Wav),
            "mp3" => Some(Self::Mp3),
            "pcm" | "linear16" | "raw" => Some(Self::Pcm),
            _ => None,
        }
    }
}

// =============================================================================
// TTS Mode Configuration
// =============================================================================

/// TTS operation mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum HuaweiTtsMode {
    /// Standard HTTP REST API.
    #[default]
    Standard,
    /// Real-time WebSocket streaming.
    Realtime,
}

impl HuaweiTtsMode {
    /// Check if this mode uses WebSocket.
    pub fn is_websocket(&self) -> bool {
        matches!(self, Self::Realtime)
    }
}

// =============================================================================
// IAM Token Response
// =============================================================================

/// IAM token response structure.
#[derive(Debug, Clone, Deserialize)]
pub struct HuaweiIamTokenResponse {
    /// Token details.
    pub token: TokenDetails,
}

/// Token details from IAM response.
#[derive(Debug, Clone, Deserialize)]
pub struct TokenDetails {
    /// Token expiration time (ISO 8601).
    pub expires_at: String,
}

// =============================================================================
// TTS Configuration
// =============================================================================

/// Configuration for Huawei Cloud TTS.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HuaweiCloudTtsConfig {
    /// IAM username.
    pub username: String,

    /// IAM password.
    pub password: String,

    /// IAM domain name.
    pub domain_name: String,

    /// Project ID.
    pub project_id: String,

    /// Region.
    #[serde(default)]
    pub region: HuaweiCloudRegion,

    /// Voice to use.
    #[serde(default)]
    pub voice: HuaweiTtsVoice,

    /// Audio output format.
    #[serde(default)]
    pub audio_format: HuaweiTtsAudioFormat,

    /// Sample rate (8000 or 16000).
    #[serde(default = "default_sample_rate")]
    pub sample_rate: u32,

    /// Speech speed (-500 to 500, 0 = normal).
    #[serde(default)]
    pub speed: i16,

    /// Speech pitch (-500 to 500, 0 = normal).
    #[serde(default)]
    pub pitch: i16,

    /// Volume (0 to 100, 50 = normal).
    #[serde(default = "default_volume")]
    pub volume: u8,

    /// Word/phoneme timestamps ("subtitle"). When enabled, Huawei SIS returns word-level timing
    /// metadata alongside the audio. Maps to the SIS `subtitle` config field (integer 0=off, 1=on)
    /// on BOTH the REST `/tts` config and the RTTS `/rtts` START config.
    /// Doc: Huawei SIS Real-Time/Standard TTS — `config.subtitle`.
    #[serde(default)]
    pub subtitle: bool,

    /// TTS operation mode.
    #[serde(default)]
    pub mode: HuaweiTtsMode,

    /// Optional endpoint base (scheme+host) redirecting the IAM token + synth POSTs to a mock/proxy.
    #[serde(default)]
    pub endpoint_override: Option<String>,
}

fn default_sample_rate() -> u32 {
    DEFAULT_SAMPLE_RATE
}

fn default_volume() -> u8 {
    DEFAULT_VOLUME
}

impl Default for HuaweiCloudTtsConfig {
    fn default() -> Self {
        Self {
            username: String::new(),
            password: String::new(),
            domain_name: String::new(),
            project_id: String::new(),
            region: HuaweiCloudRegion::default(),
            voice: HuaweiTtsVoice::default(),
            audio_format: HuaweiTtsAudioFormat::default(),
            sample_rate: DEFAULT_SAMPLE_RATE,
            speed: DEFAULT_SPEED,
            pitch: DEFAULT_PITCH,
            volume: DEFAULT_VOLUME,
            subtitle: false,
            mode: HuaweiTtsMode::default(),
            endpoint_override: None,
        }
    }
}

impl HuaweiCloudTtsConfig {
    /// Validate the configuration.
    pub fn validate(&self) -> Result<(), TTSError> {
        if self.username.is_empty() {
            return Err(TTSError::InvalidConfiguration(
                "Huawei Cloud username is required".to_string(),
            ));
        }

        if self.password.is_empty() {
            return Err(TTSError::InvalidConfiguration(
                "Huawei Cloud password is required".to_string(),
            ));
        }

        if self.domain_name.is_empty() {
            return Err(TTSError::InvalidConfiguration(
                "Huawei Cloud domain name is required".to_string(),
            ));
        }

        if self.project_id.is_empty() {
            return Err(TTSError::InvalidConfiguration(
                "Huawei Cloud project ID is required".to_string(),
            ));
        }

        // Validate sample rate
        if self.sample_rate != 8000 && self.sample_rate != 16000 {
            return Err(TTSError::InvalidConfiguration(
                "Sample rate must be 8000 or 16000".to_string(),
            ));
        }

        // Validate speed
        if self.speed < -500 || self.speed > 500 {
            return Err(TTSError::InvalidConfiguration(
                "Speed must be between -500 and 500".to_string(),
            ));
        }

        // Validate pitch
        if self.pitch < -500 || self.pitch > 500 {
            return Err(TTSError::InvalidConfiguration(
                "Pitch must be between -500 and 500".to_string(),
            ));
        }

        // Validate volume
        if self.volume > 100 {
            return Err(TTSError::InvalidConfiguration(
                "Volume must be between 0 and 100".to_string(),
            ));
        }

        // Check premium voice region support
        if self.voice.is_premium() && !self.region.supports_premium_voices() {
            return Err(TTSError::InvalidConfiguration(format!(
                "Premium voices are only available in cn-north-4 and cn-east-3 regions, not {}",
                self.region.code()
            )));
        }

        // Check pitch support for premium voices
        if self.pitch != 0 && !self.voice.supports_pitch() {
            return Err(TTSError::InvalidConfiguration(
                "Pitch adjustment is not supported for premium voices".to_string(),
            ));
        }

        if let Some(endpoint) = self.endpoint_override.as_deref() {
            validate_huawei_tts_endpoint("endpoint_override", endpoint)
                .map_err(TTSError::InvalidConfiguration)?;
        }

        Ok(())
    }

    /// Convert from base TTSConfig.
    ///
    /// The API key should be in the format `username|password|domain_name|project_id`.
    pub fn from_base(config: TTSConfig) -> Result<Self, TTSError> {
        if config.api_key.is_empty() {
            return Err(TTSError::InvalidConfiguration(
                "Huawei Cloud API key is required (format: username|password|domain_name|project_id)"
                    .to_string(),
            ));
        }

        // Parse api key parts
        let parts: Vec<&str> = config.api_key.split('|').collect();
        if parts.len() != 4 {
            return Err(TTSError::InvalidConfiguration(
                "Huawei Cloud API key must be in format: username|password|domain_name|project_id"
                    .to_string(),
            ));
        }

        let username = parts[0].to_string();
        let password = parts[1].to_string();
        let domain_name = parts[2].to_string();
        let project_id = parts[3].to_string();

        // Parse region from model field (e.g., "cn-north-4", "cn-east-3")
        // If not specified, default to cn-north-4 which has the most features
        let region = if !config.model.is_empty() {
            HuaweiCloudRegion::from_str(&config.model).unwrap_or_default()
        } else {
            HuaweiCloudRegion::default()
        };

        // Parse voice
        let voice = config
            .voice_id
            .as_ref()
            .map(|v| HuaweiTtsVoice::from_str(v))
            .unwrap_or_default();

        // Parse audio format
        let audio_format = config
            .audio_format
            .as_ref()
            .and_then(|f| HuaweiTtsAudioFormat::from_str(f))
            .unwrap_or_default();

        // Parse sample rate
        let sample_rate = config.sample_rate.unwrap_or(DEFAULT_SAMPLE_RATE);

        // Map speaking_rate to Huawei speed
        // speaking_rate: 0.25-4.0 (1.0 = normal) -> speed: -500 to 500 (0 = normal)
        let speed = config
            .speaking_rate
            .map(|rate| {
                // Map: 0.25 -> -500, 1.0 -> 0, 4.0 -> 500
                let normalized = (rate - 1.0) / (4.0 - 0.25) * 1000.0;
                (normalized.round() as i16).clamp(-500, 500)
            })
            .unwrap_or(DEFAULT_SPEED);

        Ok(Self {
            username,
            password,
            domain_name,
            project_id,
            region,
            voice,
            audio_format,
            sample_rate,
            speed,
            pitch: DEFAULT_PITCH,
            volume: DEFAULT_VOLUME,
            subtitle: false,
            mode: HuaweiTtsMode::default(),
            endpoint_override: None,
        })
    }

    /// Build from the standardized TTS config. Huawei SIS exposes prosody knobs directly, so this
    /// maps `speed` -> `speed` (the same `speaking_rate`-style normalization into the -500..=500
    /// range used by `from_base`), `pitch` -> `pitch` and `volume` -> `volume` (both clamped to the
    /// SIS ranges), plus `sample_rate` -> `sample_rate`. The typed `word_timestamps` feature maps to
    /// SIS `subtitle` (word/phoneme timing metadata), emitted on both the REST `/tts` and RTTS START
    /// configs. Huawei's `region` (not a standard field) is read from the `extras` passthrough,
    /// overriding the `model`-derived region. Features without a Huawei field (stability,
    /// similarity_boost, style, use_speaker_boost, emotion, instructions, ssml, language, streaming,
    /// seed) are skipped.
    pub fn from_standard(
        std: &crate::core::tts::standard::StandardTTSConfig,
    ) -> Result<Self, TTSError> {
        let f = &std.features;
        let mut cfg = Self::from_base(std.base.clone())?;

        if let Some(speed) = f.speed {
            // Map: 0.25 -> -500, 1.0 -> 0, 4.0 -> 500 (same normalization as from_base).
            let normalized = (speed - 1.0) / (4.0 - 0.25) * 1000.0;
            cfg.speed = (normalized.round() as i16).clamp(-500, 500);
        }
        if let Some(pitch) = f.pitch {
            cfg.pitch = (pitch.round() as i16).clamp(-500, 500);
        }
        if let Some(volume) = f.volume {
            cfg.volume = (volume.round() as i64).clamp(0, 100) as u8;
        }
        if let Some(rate) = f.sample_rate {
            cfg.sample_rate = rate;
        }
        // Word/phoneme timestamps → SIS `subtitle` (typed `word_timestamps`). Reaches the REST `/tts`
        // and RTTS START configs as the integer `subtitle` field (0/1).
        if let Some(ts) = f.word_timestamps {
            cfg.subtitle = ts;
        }

        // Provider-specific passthrough: region override.
        if let Some(region) = std.extras.0.get("region").and_then(|v| v.as_str()) {
            cfg.region = HuaweiCloudRegion::from_str(region).unwrap_or(cfg.region);
        }

        // Endpoint override (W-T0 mock harness): redirects IAM token + synth POSTs to a mock/proxy.
        cfg.endpoint_override = std.endpoint_override().map(String::from);

        cfg.validate()?;
        Ok(cfg)
    }

    /// Get the standard TTS endpoint URL.
    ///
    /// Honors `endpoint_override` (scheme+host) when set, keeping the SIS path+query so the synth
    /// POST can be redirected to a mock/proxy.
    pub fn get_tts_url(&self) -> String {
        let url = format!(
            "https://{}/v1/{}/tts",
            self.region.sis_endpoint(),
            self.project_id
        );
        crate::core::tts::standard::override_rest_endpoint(&url, self.endpoint_override.as_deref())
    }

    /// Get the real-time TTS WebSocket URL.
    pub fn get_rtts_url(&self) -> String {
        format!(
            "wss://{}/v1/{}/rtts",
            self.region.sis_endpoint(),
            self.project_id
        )
    }

    /// Get list of supported voices.
    pub fn supported_voices() -> Vec<(&'static str, &'static str, &'static str)> {
        vec![
            // Standard voices (name, property, type)
            (
                "小琪 (Female, Standard)",
                "chinese_xiaoqi_common",
                "standard",
            ),
            ("小雯 (Female, Soft)", "chinese_xiaowen_common", "standard"),
            (
                "小燕 (Female, Gentle)",
                "chinese_xiaoyan_common",
                "standard",
            ),
            (
                "小倩 (Female, Mature)",
                "chinese_xiaoqian_common",
                "standard",
            ),
            (
                "小婧 (Female, Lively)",
                "chinese_xiaojing_common",
                "standard",
            ),
            ("小宇 (Male, Standard)", "chinese_xiaoyu_common", "standard"),
            (
                "小宋 (Male, Passionate)",
                "chinese_xiaosong_common",
                "standard",
            ),
            (
                "小王 (Child, Standard)",
                "chinese_xiaowang_common",
                "standard",
            ),
            ("小呆 (Child, Cute)", "chinese_xiaodai_common", "standard"),
            (
                "Cameal (Female, English)",
                "chinese_cameal_common",
                "standard",
            ),
            // Premium voices
            (
                "华小夏 (Female, Sales)",
                "chinese_huaxiaoxia_common",
                "premium",
            ),
            (
                "华小唯 (Female, Customer Service)",
                "chinese_huaxiaowei_common",
                "premium",
            ),
            (
                "华晓刚 (Male, News)",
                "chinese_huaxiaogang_common",
                "premium",
            ),
        ]
    }

    /// Get list of supported audio formats.
    pub fn supported_formats() -> Vec<&'static str> {
        vec!["wav", "mp3", "pcm"]
    }

    /// Get list of supported sample rates.
    pub fn supported_sample_rates() -> Vec<u32> {
        vec![8000, 16000]
    }
}

// =============================================================================
// TTS Request/Response
// =============================================================================

/// Standard TTS request body.
#[derive(Debug, Clone, Serialize)]
pub struct HuaweiTtsRequest {
    /// Text to synthesize.
    pub text: String,
    /// Configuration.
    pub config: HuaweiTtsRequestConfig,
}

/// TTS request configuration.
#[derive(Debug, Clone, Serialize)]
pub struct HuaweiTtsRequestConfig {
    /// Audio format.
    pub audio_format: String,
    /// Sample rate.
    pub sample_rate: String,
    /// Voice property.
    pub property: String,
    /// Speed (-500 to 500).
    pub speed: i16,
    /// Pitch (-500 to 500).
    pub pitch: i16,
    /// Volume (0 to 100).
    pub volume: u8,
    /// Word/phoneme timestamps ("subtitle"): SIS integer flag (0=off, 1=on). Omitted when off so
    /// the default request shape is unchanged. Doc: Huawei SIS Standard TTS — `config.subtitle`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<u8>,
}

impl HuaweiTtsRequest {
    /// Create a new TTS request.
    pub fn new(text: &str, config: &HuaweiCloudTtsConfig) -> Self {
        Self {
            text: text.to_string(),
            config: HuaweiTtsRequestConfig {
                audio_format: config.audio_format.as_str().to_string(),
                sample_rate: config.sample_rate.to_string(),
                property: config.voice.as_property().to_string(),
                speed: config.speed,
                pitch: if config.voice.supports_pitch() {
                    config.pitch
                } else {
                    0
                },
                volume: config.volume,
                subtitle: if config.subtitle { Some(1) } else { None },
            },
        }
    }

    /// Serialize to JSON.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

/// Standard TTS response body.
#[derive(Debug, Clone, Deserialize)]
pub struct HuaweiTtsResponse {
    /// Trace ID.
    pub trace_id: Option<String>,
    /// Result with audio data.
    pub result: Option<HuaweiTtsResult>,
    /// Error code.
    #[serde(default)]
    pub error_code: Option<String>,
    /// Error message.
    pub error_msg: Option<String>,
}

/// TTS result containing audio data.
#[derive(Debug, Clone, Deserialize)]
pub struct HuaweiTtsResult {
    /// Base64-encoded audio data.
    pub data: String,
}

impl HuaweiTtsResponse {
    /// Parse from JSON string.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Check if the response indicates success.
    pub fn is_success(&self) -> bool {
        self.error_code.is_none() && self.result.is_some()
    }

    /// Get error message if present.
    pub fn get_error(&self) -> Option<String> {
        self.error_code.as_ref().map(|code| {
            format!(
                "{}: {}",
                code,
                self.error_msg.as_deref().unwrap_or("Unknown error")
            )
        })
    }

    /// Get audio data if present (base64-decoded).
    pub fn get_audio_data(&self) -> Option<Vec<u8>> {
        use base64::{Engine, engine::general_purpose::STANDARD};

        self.result
            .as_ref()
            .and_then(|r| STANDARD.decode(&r.data).ok())
    }
}

// =============================================================================
// Real-time TTS WebSocket Messages
// =============================================================================

/// Real-time TTS START command.
#[derive(Debug, Clone, Serialize)]
pub struct HuaweiRttsStartCommand {
    /// Command type.
    pub command: String,
    /// Text to synthesize.
    pub text: String,
    /// Configuration.
    pub config: HuaweiRttsConfig,
}

/// Real-time TTS configuration.
#[derive(Debug, Clone, Serialize)]
pub struct HuaweiRttsConfig {
    /// Audio format.
    pub audio_format: String,
    /// Voice property.
    pub property: String,
    /// Sample rate.
    pub sample_rate: String,
    /// Speed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed: Option<i16>,
    /// Pitch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pitch: Option<i16>,
    /// Volume.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume: Option<u8>,
    /// Word/phoneme timestamps ("subtitle"): SIS integer flag (0=off, 1=on). Omitted when off so the
    /// default START shape is unchanged. Doc: Huawei SIS Real-Time TTS — `config.subtitle`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<u8>,
}

impl HuaweiRttsStartCommand {
    /// Create a new START command.
    pub fn new(text: &str, config: &HuaweiCloudTtsConfig) -> Self {
        Self {
            command: "START".to_string(),
            text: text.to_string(),
            config: HuaweiRttsConfig {
                audio_format: config.audio_format.as_str().to_string(),
                property: config.voice.as_property().to_string(),
                sample_rate: config.sample_rate.to_string(),
                speed: if config.speed != 0 {
                    Some(config.speed)
                } else {
                    None
                },
                pitch: if config.pitch != 0 && config.voice.supports_pitch() {
                    Some(config.pitch)
                } else {
                    None
                },
                volume: if config.volume != 50 {
                    Some(config.volume)
                } else {
                    None
                },
                subtitle: if config.subtitle { Some(1) } else { None },
            },
        }
    }

    /// Serialize to JSON.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

/// Real-time TTS response types.
#[derive(Debug, Clone, Deserialize)]
pub struct HuaweiRttsResponse {
    /// Response type.
    pub resp_type: Option<String>,
    /// Trace ID.
    pub trace_id: Option<String>,
    /// Error code.
    pub error_code: Option<i32>,
    /// Error message.
    pub error_msg: Option<String>,
}

impl HuaweiRttsResponse {
    /// Parse from JSON string.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Check if this is an END response.
    pub fn is_end(&self) -> bool {
        self.resp_type.as_deref() == Some("END")
    }

    /// Check if this is an error.
    pub fn is_error(&self) -> bool {
        self.error_code.map(|c| c != 0).unwrap_or(false)
    }

    /// Get error message.
    pub fn get_error(&self) -> Option<String> {
        if self.is_error() {
            Some(format!(
                "Error {}: {}",
                self.error_code.unwrap_or(0),
                self.error_msg.as_deref().unwrap_or("Unknown error")
            ))
        } else {
            None
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // Maps speed -> speed (normalized into SIS -500..=500), pitch -> pitch, volume -> volume,
    // sample_rate -> sample_rate, and demonstrates the extras passthrough (region override).
    #[test]
    fn from_standard_maps_prosody_and_region() {
        use crate::core::tts::standard::{ProviderExtras, StandardTTSConfig, TtsFeatures};
        let mut extras = serde_json::Map::new();
        extras.insert("region".into(), serde_json::json!("cn-east-3"));
        let std = StandardTTSConfig {
            base: TTSConfig {
                provider: "huawei-cloud".into(),
                api_key: "user|pass|domain|project123".into(),
                sample_rate: Some(16000),
                ..Default::default()
            },
            features: TtsFeatures {
                speed: Some(1.0),
                pitch: Some(100.0),
                volume: Some(80.0),
                sample_rate: Some(8000),
                ssml: Some(true), // capability gap: Huawei has no SSML, must be ignored
                ..Default::default()
            },
            extras: ProviderExtras(extras),
        };
        let cfg = HuaweiCloudTtsConfig::from_standard(&std).unwrap();
        assert_eq!(cfg.speed, 0); // 1.0x -> 0 (normal)
        assert_eq!(cfg.pitch, 100);
        assert_eq!(cfg.volume, 80);
        assert_eq!(cfg.sample_rate, 8000);
        assert_eq!(cfg.region, HuaweiCloudRegion::CnEast3); // from extras passthrough
    }

    // The typed `word_timestamps` feature maps to the SIS `subtitle` config flag.
    #[test]
    fn from_standard_maps_word_timestamps_to_subtitle() {
        use crate::core::tts::standard::{StandardTTSConfig, TtsFeatures};
        let std = StandardTTSConfig {
            base: TTSConfig {
                provider: "huawei-cloud".into(),
                api_key: "user|pass|domain|project123".into(),
                sample_rate: Some(16000),
                ..Default::default()
            },
            features: TtsFeatures {
                word_timestamps: Some(true),
                ..Default::default()
            },
            extras: Default::default(),
        };
        let cfg = HuaweiCloudTtsConfig::from_standard(&std).unwrap();
        assert!(cfg.subtitle);
    }

    // WIRE-LEVEL: `subtitle` reaches the SERIALIZED REST `/tts` request config (not just the config
    // struct). Doc: Huawei SIS Standard TTS — `config.subtitle` (integer 0/1).
    #[test]
    fn word_timestamps_reach_serialized_rest_tts_body() {
        use crate::core::tts::standard::{StandardTTSConfig, TtsFeatures};
        let std = StandardTTSConfig {
            base: TTSConfig {
                provider: "huawei-cloud".into(),
                api_key: "user|pass|domain|project123".into(),
                sample_rate: Some(16000),
                ..Default::default()
            },
            features: TtsFeatures {
                word_timestamps: Some(true),
                ..Default::default()
            },
            extras: Default::default(),
        };
        let cfg = HuaweiCloudTtsConfig::from_standard(&std).unwrap();
        let json = HuaweiTtsRequest::new("hello", &cfg).to_json().unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            v["config"]["subtitle"], 1,
            "subtitle must be on the REST wire: {json}"
        );
    }

    // WIRE-LEVEL: `subtitle` reaches the SERIALIZED RTTS START command config.
    // Doc: Huawei SIS Real-Time TTS — `config.subtitle` (integer 0/1).
    #[test]
    fn word_timestamps_reach_serialized_rtts_start_command() {
        use crate::core::tts::standard::{StandardTTSConfig, TtsFeatures};
        let std = StandardTTSConfig {
            base: TTSConfig {
                provider: "huawei-cloud".into(),
                api_key: "user|pass|domain|project123".into(),
                sample_rate: Some(16000),
                ..Default::default()
            },
            features: TtsFeatures {
                word_timestamps: Some(true),
                ..Default::default()
            },
            extras: Default::default(),
        };
        let cfg = HuaweiCloudTtsConfig::from_standard(&std).unwrap();
        let json = HuaweiRttsStartCommand::new("hello", &cfg)
            .to_json()
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["command"], "START");
        assert_eq!(
            v["config"]["subtitle"], 1,
            "subtitle must be on the RTTS START wire: {json}"
        );
    }

    // When word_timestamps is unset/false, `subtitle` is OMITTED from both wire shapes (no spurious
    // 0 that would change the default request and could be rejected by stricter SIS validation).
    #[test]
    fn subtitle_omitted_from_wire_when_unset() {
        let cfg = HuaweiCloudTtsConfig::from_base(TTSConfig {
            provider: "huawei-cloud".into(),
            api_key: "user|pass|domain|project123".into(),
            ..Default::default()
        })
        .unwrap();
        assert!(!cfg.subtitle);
        let rest: serde_json::Value =
            serde_json::from_str(&HuaweiTtsRequest::new("hi", &cfg).to_json().unwrap()).unwrap();
        assert!(
            rest["config"].get("subtitle").is_none(),
            "subtitle must be absent on REST when off"
        );
        let rtts: serde_json::Value =
            serde_json::from_str(&HuaweiRttsStartCommand::new("hi", &cfg).to_json().unwrap())
                .unwrap();
        assert!(
            rtts["config"].get("subtitle").is_none(),
            "subtitle must be absent on RTTS when off"
        );
    }

    #[test]
    fn test_region_code() {
        assert_eq!(HuaweiCloudRegion::CnNorth4.code(), "cn-north-4");
        assert_eq!(HuaweiCloudRegion::CnEast3.code(), "cn-east-3");
        assert_eq!(HuaweiCloudRegion::ApSoutheast3.code(), "ap-southeast-3");
    }

    #[test]
    fn test_region_endpoint() {
        assert_eq!(
            HuaweiCloudRegion::CnNorth4.sis_endpoint(),
            "sis.cn-north-4.myhuaweicloud.com"
        );
        assert_eq!(
            HuaweiCloudRegion::ApSoutheast3.sis_endpoint(),
            "sis-ext.ap-southeast-3.myhuaweicloud.com"
        );
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
    fn test_region_premium_support() {
        assert!(HuaweiCloudRegion::CnNorth4.supports_premium_voices());
        assert!(HuaweiCloudRegion::CnEast3.supports_premium_voices());
        assert!(!HuaweiCloudRegion::ApSoutheast3.supports_premium_voices());
    }

    #[test]
    fn test_voice_property() {
        assert_eq!(
            HuaweiTtsVoice::XiaoYan.as_property(),
            "chinese_xiaoyan_common"
        );
        assert_eq!(
            HuaweiTtsVoice::XiaoYu.as_property(),
            "chinese_xiaoyu_common"
        );
        assert_eq!(
            HuaweiTtsVoice::HuaXiaoXia.as_property(),
            "chinese_huaxiaoxia_common"
        );
    }

    #[test]
    fn test_voice_from_str() {
        assert_eq!(HuaweiTtsVoice::from_str("xiaoyan"), HuaweiTtsVoice::XiaoYan);
        assert_eq!(HuaweiTtsVoice::from_str("小燕"), HuaweiTtsVoice::XiaoYan);
        assert_eq!(HuaweiTtsVoice::from_str("xiaoyu"), HuaweiTtsVoice::XiaoYu);
        assert_eq!(
            HuaweiTtsVoice::from_str("huaxiaoxia"),
            HuaweiTtsVoice::HuaXiaoXia
        );
    }

    #[test]
    fn test_voice_premium() {
        assert!(!HuaweiTtsVoice::XiaoYan.is_premium());
        assert!(HuaweiTtsVoice::HuaXiaoXia.is_premium());
    }

    #[test]
    fn test_voice_pitch_support() {
        assert!(HuaweiTtsVoice::XiaoYan.supports_pitch());
        assert!(!HuaweiTtsVoice::HuaXiaoXia.supports_pitch());
    }

    #[test]
    fn test_audio_format() {
        assert_eq!(HuaweiTtsAudioFormat::Wav.as_str(), "wav");
        assert_eq!(HuaweiTtsAudioFormat::Mp3.as_str(), "mp3");
        assert_eq!(HuaweiTtsAudioFormat::Pcm.as_str(), "pcm");
    }

    #[test]
    fn test_audio_format_from_str() {
        assert_eq!(
            HuaweiTtsAudioFormat::from_str("wav"),
            Some(HuaweiTtsAudioFormat::Wav)
        );
        assert_eq!(
            HuaweiTtsAudioFormat::from_str("mp3"),
            Some(HuaweiTtsAudioFormat::Mp3)
        );
        assert_eq!(
            HuaweiTtsAudioFormat::from_str("pcm"),
            Some(HuaweiTtsAudioFormat::Pcm)
        );
        assert_eq!(HuaweiTtsAudioFormat::from_str("invalid"), None);
    }

    #[test]
    fn test_config_validation_valid() {
        let config = HuaweiCloudTtsConfig {
            username: "user".to_string(),
            password: "pass".to_string(),
            domain_name: "domain".to_string(),
            project_id: "project123".to_string(),
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validation_rejects_ssrf_endpoint_override() {
        let _env = crate::core::net::ssrf_env_lock();
        let mut config = HuaweiCloudTtsConfig {
            username: "user".to_string(),
            password: "pass".to_string(),
            domain_name: "domain".to_string(),
            project_id: "project123".to_string(),
            endpoint_override: Some("https://gateway.example.com".to_string()),
            ..Default::default()
        };
        assert!(config.validate().is_ok());

        config.endpoint_override = Some("http://127.0.0.1:8080".to_string());
        let err = config
            .validate()
            .expect_err("loopback endpoint_override must be rejected");
        assert!(err.to_string().contains("SSRF protection"));

        config.endpoint_override = Some("file:///tmp/socket".to_string());
        let err = config
            .validate()
            .expect_err("file endpoint_override must be rejected");
        assert!(err.to_string().contains("URL scheme"));

        config.endpoint_override = Some("wss://gateway.example.com".to_string());
        let err = config
            .validate()
            .expect_err("WebSocket endpoint_override must be rejected");
        assert!(err.to_string().contains("URL scheme"));

        let std = crate::core::tts::standard::StandardTTSConfig::from_base(TTSConfig {
            provider: "huawei-cloud".to_string(),
            api_key: "user|pass|domain|project123".to_string(),
            ..Default::default()
        })
        .with_endpoint_override("file:///tmp/socket");
        assert!(HuaweiCloudTtsConfig::from_standard(&std).is_err());
    }

    #[test]
    fn test_config_validation_empty_username() {
        let config = HuaweiCloudTtsConfig {
            password: "pass".to_string(),
            domain_name: "domain".to_string(),
            project_id: "project123".to_string(),
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_invalid_sample_rate() {
        let config = HuaweiCloudTtsConfig {
            username: "user".to_string(),
            password: "pass".to_string(),
            domain_name: "domain".to_string(),
            project_id: "project123".to_string(),
            sample_rate: 44100,
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_invalid_speed() {
        let config = HuaweiCloudTtsConfig {
            username: "user".to_string(),
            password: "pass".to_string(),
            domain_name: "domain".to_string(),
            project_id: "project123".to_string(),
            speed: 600,
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_premium_voice_region() {
        let config = HuaweiCloudTtsConfig {
            username: "user".to_string(),
            password: "pass".to_string(),
            domain_name: "domain".to_string(),
            project_id: "project123".to_string(),
            voice: HuaweiTtsVoice::HuaXiaoXia,
            region: HuaweiCloudRegion::ApSoutheast3,
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_premium_voice_pitch() {
        let config = HuaweiCloudTtsConfig {
            username: "user".to_string(),
            password: "pass".to_string(),
            domain_name: "domain".to_string(),
            project_id: "project123".to_string(),
            voice: HuaweiTtsVoice::HuaXiaoXia,
            region: HuaweiCloudRegion::CnNorth4,
            pitch: 100,
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_from_base_config() {
        let base = TTSConfig {
            api_key: "user|pass|domain|project123".to_string(),
            voice_id: Some("xiaoyu".to_string()),
            audio_format: Some("mp3".to_string()),
            sample_rate: Some(8000),
            ..Default::default()
        };

        let config = HuaweiCloudTtsConfig::from_base(base).unwrap();
        assert_eq!(config.username, "user");
        assert_eq!(config.password, "pass");
        assert_eq!(config.domain_name, "domain");
        assert_eq!(config.project_id, "project123");
        assert_eq!(config.voice, HuaweiTtsVoice::XiaoYu);
        assert_eq!(config.audio_format, HuaweiTtsAudioFormat::Mp3);
        assert_eq!(config.sample_rate, 8000);
    }

    #[test]
    fn test_from_base_config_invalid() {
        let base = TTSConfig {
            api_key: "invalid_format".to_string(),
            ..Default::default()
        };
        assert!(HuaweiCloudTtsConfig::from_base(base).is_err());
    }

    #[test]
    fn test_config_urls() {
        let config = HuaweiCloudTtsConfig {
            username: "user".to_string(),
            password: "pass".to_string(),
            domain_name: "domain".to_string(),
            project_id: "abc123".to_string(),
            region: HuaweiCloudRegion::CnNorth4,
            ..Default::default()
        };

        assert_eq!(
            config.get_tts_url(),
            "https://sis.cn-north-4.myhuaweicloud.com/v1/abc123/tts"
        );
        assert_eq!(
            config.get_rtts_url(),
            "wss://sis.cn-north-4.myhuaweicloud.com/v1/abc123/rtts"
        );
    }

    #[test]
    fn test_tts_request_serialization() {
        let config = HuaweiCloudTtsConfig {
            username: "user".to_string(),
            password: "pass".to_string(),
            domain_name: "domain".to_string(),
            project_id: "project123".to_string(),
            voice: HuaweiTtsVoice::XiaoYan,
            audio_format: HuaweiTtsAudioFormat::Wav,
            sample_rate: 16000,
            speed: 100,
            pitch: 50,
            volume: 80,
            ..Default::default()
        };

        let request = HuaweiTtsRequest::new("你好世界", &config);
        let json = request.to_json().unwrap();

        assert!(json.contains("\"text\":\"你好世界\""));
        assert!(json.contains("\"property\":\"chinese_xiaoyan_common\""));
        assert!(json.contains("\"audio_format\":\"wav\""));
        assert!(json.contains("\"sample_rate\":\"16000\""));
        assert!(json.contains("\"speed\":100"));
        assert!(json.contains("\"pitch\":50"));
        assert!(json.contains("\"volume\":80"));
    }

    #[test]
    fn test_tts_response_success() {
        let json = r#"{
            "trace_id": "abc123",
            "result": {
                "data": "SGVsbG8gV29ybGQ="
            }
        }"#;

        let response = HuaweiTtsResponse::from_json(json).unwrap();
        assert!(response.is_success());
        assert!(response.get_error().is_none());

        let audio = response.get_audio_data().unwrap();
        assert_eq!(audio, b"Hello World");
    }

    #[test]
    fn test_tts_response_error() {
        let json = r#"{
            "trace_id": "abc123",
            "error_code": "SIS.0001",
            "error_msg": "Authentication failed"
        }"#;

        let response = HuaweiTtsResponse::from_json(json).unwrap();
        assert!(!response.is_success());
        let error = response.get_error().unwrap();
        assert!(error.contains("SIS.0001"));
        assert!(error.contains("Authentication failed"));
    }

    #[test]
    fn test_rtts_start_command() {
        let config = HuaweiCloudTtsConfig {
            username: "user".to_string(),
            password: "pass".to_string(),
            domain_name: "domain".to_string(),
            project_id: "project123".to_string(),
            voice: HuaweiTtsVoice::XiaoYu,
            audio_format: HuaweiTtsAudioFormat::Pcm,
            sample_rate: 8000,
            speed: 100,
            volume: 70,
            ..Default::default()
        };

        let cmd = HuaweiRttsStartCommand::new("测试文本", &config);
        let json = cmd.to_json().unwrap();

        assert!(json.contains("\"command\":\"START\""));
        assert!(json.contains("\"text\":\"测试文本\""));
        assert!(json.contains("\"property\":\"chinese_xiaoyu_common\""));
        assert!(json.contains("\"audio_format\":\"pcm\""));
        assert!(json.contains("\"sample_rate\":\"8000\""));
        assert!(json.contains("\"speed\":100"));
        assert!(json.contains("\"volume\":70"));
    }

    #[test]
    fn test_rtts_response() {
        let json = r#"{"resp_type": "END", "trace_id": "abc123", "error_code": 0}"#;
        let response = HuaweiRttsResponse::from_json(json).unwrap();

        assert!(response.is_end());
        assert!(!response.is_error());
    }

    #[test]
    fn test_rtts_response_error() {
        let json = r#"{"resp_type": "ERROR", "error_code": 1, "error_msg": "Invalid text"}"#;
        let response = HuaweiRttsResponse::from_json(json).unwrap();

        assert!(response.is_error());
        let error = response.get_error().unwrap();
        assert!(error.contains("Invalid text"));
    }

    #[test]
    fn test_supported_voices() {
        let voices = HuaweiCloudTtsConfig::supported_voices();
        assert!(!voices.is_empty());
        assert!(
            voices
                .iter()
                .any(|(_, prop, _)| *prop == "chinese_xiaoyan_common")
        );
    }

    #[test]
    fn test_supported_formats() {
        let formats = HuaweiCloudTtsConfig::supported_formats();
        assert!(formats.contains(&"wav"));
        assert!(formats.contains(&"mp3"));
        assert!(formats.contains(&"pcm"));
    }
}
