//! Alibaba Cloud DashScope TTS Configuration
//!
//! Configuration types and constants for DashScope TTS providers.

use crate::core::tts::base::{TTSConfig, TTSError};
use serde::{Deserialize, Serialize};

// =============================================================================
// Constants
// =============================================================================

/// DashScope Beijing realtime endpoint.
pub const DASHSCOPE_BEIJING_REALTIME_URL: &str = "wss://dashscope.aliyuncs.com/api-ws/v1/realtime";

/// DashScope Singapore realtime endpoint.
pub const DASHSCOPE_SINGAPORE_REALTIME_URL: &str =
    "wss://dashscope-intl.aliyuncs.com/api-ws/v1/realtime";

/// DashScope Beijing inference endpoint.
pub const DASHSCOPE_BEIJING_INFERENCE_URL: &str =
    "wss://dashscope.aliyuncs.com/api-ws/v1/inference";

/// DashScope Singapore inference endpoint.
pub const DASHSCOPE_SINGAPORE_INFERENCE_URL: &str =
    "wss://dashscope-intl.aliyuncs.com/api-ws/v1/inference";

/// Default TTS model.
pub const DEFAULT_TTS_MODEL: &str = "cosyvoice-v3-flash";

/// Default sample rate.
pub const DEFAULT_SAMPLE_RATE: u32 = 22050;

/// Default audio format.
pub const DEFAULT_AUDIO_FORMAT: &str = "mp3";

/// Default voice.
pub const DEFAULT_VOICE: &str = "longxiaochun";

// =============================================================================
// Region Enum
// =============================================================================

/// DashScope API region.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DashScopeRegion {
    /// Beijing (China mainland).
    #[default]
    Beijing,
    /// Singapore (International).
    Singapore,
}

impl DashScopeRegion {
    /// Get the realtime endpoint URL.
    pub fn realtime_url(&self) -> &'static str {
        match self {
            DashScopeRegion::Beijing => DASHSCOPE_BEIJING_REALTIME_URL,
            DashScopeRegion::Singapore => DASHSCOPE_SINGAPORE_REALTIME_URL,
        }
    }

    /// Get the inference endpoint URL.
    pub fn inference_url(&self) -> &'static str {
        match self {
            DashScopeRegion::Beijing => DASHSCOPE_BEIJING_INFERENCE_URL,
            DashScopeRegion::Singapore => DASHSCOPE_SINGAPORE_INFERENCE_URL,
        }
    }

    /// Parse from string.
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "singapore" | "intl" | "international" => DashScopeRegion::Singapore,
            _ => DashScopeRegion::Beijing,
        }
    }
}

// =============================================================================
// TTS Model Enum
// =============================================================================

/// DashScope TTS model types.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DashScopeTtsModel {
    /// CosyVoice v3 Flash (fast).
    #[default]
    CosyVoiceV3Flash,
    /// CosyVoice v3 Plus (premium).
    CosyVoiceV3Plus,
    /// CosyVoice v2 (legacy).
    CosyVoiceV2,
    /// Qwen3 TTS Flash Realtime.
    Qwen3TtsFlashRealtime,
}

impl DashScopeTtsModel {
    /// Get the model ID string.
    pub fn as_model_id(&self) -> &'static str {
        match self {
            DashScopeTtsModel::CosyVoiceV3Flash => "cosyvoice-v3-flash",
            DashScopeTtsModel::CosyVoiceV3Plus => "cosyvoice-v3-plus",
            DashScopeTtsModel::CosyVoiceV2 => "cosyvoice-v2",
            DashScopeTtsModel::Qwen3TtsFlashRealtime => "qwen3-tts-flash-realtime",
        }
    }

    /// Check if this is a Qwen model (uses realtime protocol).
    pub fn is_qwen_model(&self) -> bool {
        matches!(self, DashScopeTtsModel::Qwen3TtsFlashRealtime)
    }

    /// Check if this is a CosyVoice model (uses inference protocol).
    pub fn is_cosyvoice_model(&self) -> bool {
        matches!(
            self,
            DashScopeTtsModel::CosyVoiceV3Flash
                | DashScopeTtsModel::CosyVoiceV3Plus
                | DashScopeTtsModel::CosyVoiceV2
        )
    }

    /// Parse from string.
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "qwen3-tts-flash-realtime" | "qwen3-tts" | "qwen-tts" => {
                DashScopeTtsModel::Qwen3TtsFlashRealtime
            }
            "cosyvoice-v3-plus" | "cosyvoice-plus" => DashScopeTtsModel::CosyVoiceV3Plus,
            "cosyvoice-v2" => DashScopeTtsModel::CosyVoiceV2,
            _ => DashScopeTtsModel::CosyVoiceV3Flash,
        }
    }

    /// Get default sample rate for this model.
    pub fn default_sample_rate(&self) -> u32 {
        match self {
            DashScopeTtsModel::Qwen3TtsFlashRealtime => 24000,
            _ => 22050,
        }
    }
}

// =============================================================================
// Audio Format Enum
// =============================================================================

/// DashScope TTS audio output format.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DashScopeAudioFormat {
    /// MP3 format.
    #[default]
    Mp3,
    /// PCM16 format.
    Pcm16,
    /// WAV format.
    Wav,
    /// Opus format.
    Opus,
}

impl DashScopeAudioFormat {
    /// Get the format string for API.
    pub fn as_format_str(&self) -> &'static str {
        match self {
            DashScopeAudioFormat::Mp3 => "mp3",
            DashScopeAudioFormat::Pcm16 => "pcm",
            DashScopeAudioFormat::Wav => "wav",
            DashScopeAudioFormat::Opus => "opus",
        }
    }

    /// Parse from string.
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "mp3" => Some(DashScopeAudioFormat::Mp3),
            "pcm" | "pcm16" | "linear16" => Some(DashScopeAudioFormat::Pcm16),
            "wav" => Some(DashScopeAudioFormat::Wav),
            "opus" => Some(DashScopeAudioFormat::Opus),
            _ => None,
        }
    }
}

// =============================================================================
// TTS Configuration
// =============================================================================

/// Configuration for Alibaba Cloud DashScope TTS.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashScopeTtsConfig {
    /// API key (from DashScope console).
    pub api_key: String,

    /// Region for API endpoint.
    #[serde(default)]
    pub region: DashScopeRegion,

    /// TTS model to use.
    #[serde(default)]
    pub model: DashScopeTtsModel,

    /// Voice to use.
    #[serde(default = "default_voice")]
    pub voice: String,

    /// Audio sample rate in Hz.
    #[serde(default = "default_sample_rate")]
    pub sample_rate: u32,

    /// Audio output format.
    #[serde(default)]
    pub audio_format: DashScopeAudioFormat,

    /// Speech rate (0.5-2.0).
    #[serde(default = "default_rate")]
    pub rate: f32,

    /// Pitch adjustment (0.5-2.0).
    #[serde(default = "default_pitch")]
    pub pitch: f32,

    /// Volume level (0-100).
    #[serde(default = "default_volume")]
    pub volume: u8,
}

fn default_voice() -> String {
    DEFAULT_VOICE.to_string()
}

fn default_sample_rate() -> u32 {
    DEFAULT_SAMPLE_RATE
}

fn default_rate() -> f32 {
    1.0
}

fn default_pitch() -> f32 {
    1.0
}

fn default_volume() -> u8 {
    50
}

impl Default for DashScopeTtsConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            region: DashScopeRegion::default(),
            model: DashScopeTtsModel::default(),
            voice: DEFAULT_VOICE.to_string(),
            sample_rate: DEFAULT_SAMPLE_RATE,
            audio_format: DashScopeAudioFormat::default(),
            rate: 1.0,
            pitch: 1.0,
            volume: 50,
        }
    }
}

impl DashScopeTtsConfig {
    /// Validate the configuration.
    pub fn validate(&self) -> Result<(), TTSError> {
        if self.api_key.is_empty() {
            return Err(TTSError::InvalidConfiguration(
                "DashScope API key is required".to_string(),
            ));
        }

        if self.voice.is_empty() {
            return Err(TTSError::InvalidConfiguration(
                "Voice is required".to_string(),
            ));
        }

        if !(0.5..=2.0).contains(&self.rate) {
            return Err(TTSError::InvalidConfiguration(
                "Rate must be between 0.5 and 2.0".to_string(),
            ));
        }

        if !(0.5..=2.0).contains(&self.pitch) {
            return Err(TTSError::InvalidConfiguration(
                "Pitch must be between 0.5 and 2.0".to_string(),
            ));
        }

        if self.volume > 100 {
            return Err(TTSError::InvalidConfiguration(
                "Volume must be between 0 and 100".to_string(),
            ));
        }

        Ok(())
    }

    /// Get the WebSocket URL for this configuration.
    pub fn get_websocket_url(&self) -> String {
        if self.model.is_qwen_model() {
            format!(
                "{}?model={}",
                self.region.realtime_url(),
                self.model.as_model_id()
            )
        } else {
            self.region.inference_url().to_string()
        }
    }

    /// Convert from base TTSConfig.
    pub fn from_base(config: TTSConfig) -> Result<Self, TTSError> {
        if config.api_key.is_empty() {
            return Err(TTSError::InvalidConfiguration(
                "DashScope API key is required".to_string(),
            ));
        }

        let model = if config.model.is_empty() {
            DashScopeTtsModel::default()
        } else {
            DashScopeTtsModel::from_str(&config.model)
        };

        let voice = config.voice_id.unwrap_or_else(|| DEFAULT_VOICE.to_string());
        let sample_rate = config
            .sample_rate
            .unwrap_or_else(|| model.default_sample_rate());
        let audio_format = config
            .audio_format
            .as_ref()
            .and_then(|f| DashScopeAudioFormat::from_str(f))
            .unwrap_or_default();

        Ok(Self {
            api_key: config.api_key,
            region: DashScopeRegion::default(),
            model,
            voice,
            sample_rate,
            audio_format,
            rate: config.speaking_rate.unwrap_or(1.0),
            pitch: 1.0,
            volume: 50,
        })
    }

    /// Get list of supported voices.
    pub fn supported_voices() -> Vec<&'static str> {
        vec![
            // CosyVoice voices
            "longxiaochun",
            "longxiaoxia",
            "longlaotie",
            "longshu",
            "longyue",
            "longwan",
            "longfei",
            "longbella",
            "longjielidou",
            "longshuo",
            "longjing",
            "longmiao",
            "longchen",
            "longhua",
            "longtong",
            // Qwen TTS voices
            "Cherry",
            "Serena",
            "Ethan",
            "Jennifer",
            "Ryan",
            "Neil",
            "Elias",
            "Shanghai-Jada",
            "Beijing-Dylan",
            "Cantonese-Kiki",
        ]
    }

    /// Get list of supported models.
    pub fn supported_models() -> Vec<&'static str> {
        vec![
            "cosyvoice-v3-flash",
            "cosyvoice-v3-plus",
            "cosyvoice-v2",
            "qwen3-tts-flash-realtime",
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
    fn test_config_default() {
        let config = DashScopeTtsConfig::default();
        assert!(config.api_key.is_empty());
        assert_eq!(config.voice, DEFAULT_VOICE);
        assert_eq!(config.sample_rate, DEFAULT_SAMPLE_RATE);
    }

    #[test]
    fn test_config_validation_valid() {
        let mut config = DashScopeTtsConfig::default();
        config.api_key = "test_key".to_string();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validation_empty_api_key() {
        let config = DashScopeTtsConfig::default();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_invalid_rate() {
        let mut config = DashScopeTtsConfig::default();
        config.api_key = "test_key".to_string();
        config.rate = 3.0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_invalid_volume() {
        let mut config = DashScopeTtsConfig::default();
        config.api_key = "test_key".to_string();
        config.volume = 150;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_region_parsing() {
        assert_eq!(
            DashScopeRegion::from_str("singapore"),
            DashScopeRegion::Singapore
        );
        assert_eq!(
            DashScopeRegion::from_str("intl"),
            DashScopeRegion::Singapore
        );
        assert_eq!(
            DashScopeRegion::from_str("beijing"),
            DashScopeRegion::Beijing
        );
        assert_eq!(DashScopeRegion::from_str("china"), DashScopeRegion::Beijing);
    }

    #[test]
    fn test_model_parsing() {
        assert_eq!(
            DashScopeTtsModel::from_str("cosyvoice-v3-flash"),
            DashScopeTtsModel::CosyVoiceV3Flash
        );
        assert_eq!(
            DashScopeTtsModel::from_str("cosyvoice-v3-plus"),
            DashScopeTtsModel::CosyVoiceV3Plus
        );
        assert_eq!(
            DashScopeTtsModel::from_str("qwen3-tts-flash-realtime"),
            DashScopeTtsModel::Qwen3TtsFlashRealtime
        );
    }

    #[test]
    fn test_model_type_detection() {
        assert!(DashScopeTtsModel::Qwen3TtsFlashRealtime.is_qwen_model());
        assert!(!DashScopeTtsModel::CosyVoiceV3Flash.is_qwen_model());
        assert!(DashScopeTtsModel::CosyVoiceV3Flash.is_cosyvoice_model());
    }

    #[test]
    fn test_audio_format_parsing() {
        assert_eq!(
            DashScopeAudioFormat::from_str("mp3"),
            Some(DashScopeAudioFormat::Mp3)
        );
        assert_eq!(
            DashScopeAudioFormat::from_str("pcm"),
            Some(DashScopeAudioFormat::Pcm16)
        );
        assert_eq!(
            DashScopeAudioFormat::from_str("wav"),
            Some(DashScopeAudioFormat::Wav)
        );
        assert_eq!(
            DashScopeAudioFormat::from_str("opus"),
            Some(DashScopeAudioFormat::Opus)
        );
    }

    #[test]
    fn test_websocket_url_cosyvoice() {
        let mut config = DashScopeTtsConfig::default();
        config.model = DashScopeTtsModel::CosyVoiceV3Flash;
        let url = config.get_websocket_url();
        assert!(url.contains("inference"));
    }

    #[test]
    fn test_websocket_url_qwen() {
        let mut config = DashScopeTtsConfig::default();
        config.model = DashScopeTtsModel::Qwen3TtsFlashRealtime;
        let url = config.get_websocket_url();
        assert!(url.contains("realtime"));
        assert!(url.contains("qwen3-tts-flash-realtime"));
    }

    #[test]
    fn test_from_base_config() {
        let base = TTSConfig {
            api_key: "test_key".to_string(),
            voice_id: Some("Cherry".to_string()),
            sample_rate: Some(24000),
            model: "qwen3-tts-flash-realtime".to_string(),
            ..Default::default()
        };

        let config = DashScopeTtsConfig::from_base(base).unwrap();
        assert_eq!(config.api_key, "test_key");
        assert_eq!(config.voice, "Cherry");
        assert_eq!(config.model, DashScopeTtsModel::Qwen3TtsFlashRealtime);
    }

    #[test]
    fn test_supported_voices() {
        let voices = DashScopeTtsConfig::supported_voices();
        assert!(voices.contains(&"longxiaochun"));
        assert!(voices.contains(&"Cherry"));
    }

    #[test]
    fn test_supported_models() {
        let models = DashScopeTtsConfig::supported_models();
        assert!(models.contains(&"cosyvoice-v3-flash"));
        assert!(models.contains(&"qwen3-tts-flash-realtime"));
    }
}
