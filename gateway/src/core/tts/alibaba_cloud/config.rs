//! Alibaba Cloud DashScope TTS Configuration
//!
//! Configuration types and constants for DashScope TTS providers.

use crate::core::tts::base::{TTSConfig, TTSError};
use serde::{Deserialize, Serialize};

fn validate_dashscope_tts_endpoint(source: &str, endpoint: &str) -> Result<(), TTSError> {
    let endpoint = endpoint.trim();
    if endpoint.is_empty() {
        return Ok(());
    }

    crate::core::net::validate_url_for_ssrf(endpoint, crate::core::net::HTTP_WS_URL_SCHEMES)
        .map_err(|msg| {
            TTSError::InvalidConfiguration(format!("{source} rejected (SSRF protection): {msg}"))
        })
}

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

    // --- CosyVoice inference-protocol knobs (run-task `payload.parameters`) ---
    /// Treat input text as SSML (`enable_ssml`). CosyVoice parses `<speak>` prosody/break tags.
    #[serde(default)]
    pub enable_ssml: Option<bool>,

    /// Natural-language delivery `instruction` (emotion / dialect / style control on CosyVoice v3).
    #[serde(default)]
    pub instruction: Option<String>,

    /// Emit word-level timestamps (`word_timestamp_enabled`).
    #[serde(default)]
    pub word_timestamp_enabled: Option<bool>,

    /// Determinism `seed` (same text + voice + seed → identical audio).
    #[serde(default)]
    pub seed: Option<u64>,

    /// Language hint(s) (`language_hints`, e.g. `["zh","en"]`) to disambiguate mixed-language text.
    #[serde(default)]
    pub language_hints: Option<Vec<String>>,

    /// Opus output bitrate in bits/sec (`bit_rate`); only meaningful for the opus format.
    #[serde(default)]
    pub bit_rate: Option<u32>,

    /// Text hotpatch / pronunciation override (`hot_fix`) — a free-form JSON object of
    /// word→pronunciation overrides forwarded verbatim.
    #[serde(default)]
    pub hot_fix: Option<serde_json::Value>,

    /// Strip Markdown markup before synthesis (`enable_markdown_filter`).
    #[serde(default)]
    pub enable_markdown_filter: Option<bool>,

    /// Embed an AIGC watermark tag in the audio (`enable_aigc_tag`).
    #[serde(default)]
    pub enable_aigc_tag: Option<bool>,

    /// Override the DashScope WS endpoint host (credential-free mock/proxy harness, W-T0).
    #[serde(default)]
    pub endpoint_override: Option<String>,
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
            enable_ssml: None,
            instruction: None,
            word_timestamp_enabled: None,
            seed: None,
            language_hints: None,
            bit_rate: None,
            hot_fix: None,
            enable_markdown_filter: None,
            enable_aigc_tag: None,
            endpoint_override: None,
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

        if let Some(endpoint) = self.endpoint_override.as_deref() {
            validate_dashscope_tts_endpoint("endpoint_override", endpoint)?;
        }

        Ok(())
    }

    /// Get the WebSocket URL for this configuration.
    ///
    /// Honors `endpoint_override` (credential-free mock/proxy harness, W-T0): the base's
    /// scheme+host replace the DashScope host while the inference/realtime path+query are kept,
    /// so a `ws://127.0.0.1:PORT` override yields `ws://127.0.0.1:PORT/api-ws/v1/inference`.
    pub fn get_websocket_url(&self) -> String {
        let url = if self.model.is_qwen_model() {
            format!(
                "{}?model={}",
                self.region.realtime_url(),
                self.model.as_model_id()
            )
        } else {
            self.region.inference_url().to_string()
        };
        crate::core::tts::standard::override_rest_endpoint(&url, self.endpoint_override.as_deref())
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
            enable_ssml: None,
            instruction: None,
            word_timestamp_enabled: None,
            seed: None,
            language_hints: None,
            bit_rate: None,
            hot_fix: None,
            enable_markdown_filter: None,
            enable_aigc_tag: None,
            endpoint_override: None,
        })
    }

    /// Build from the standardized TTS config (W1 keystone).
    ///
    /// CosyVoice/Qwen expose prosody knobs directly, so `speed` maps to `rate`, `pitch` to
    /// `pitch`, and `volume` (a 0..=100 level here) is taken from the 0-100 `volume` feature;
    /// `sample_rate` overrides the output rate.
    ///
    /// The CosyVoice inference protocol (`run-task` `payload.parameters`, the protocol WaaV uses
    /// for its default `cosyvoice-v3-flash` model) also exposes several knobs that previously had
    /// no home; these are now mapped from the shared typed [`TtsFeatures`] fields:
    /// - `ssml` → `enable_ssml`
    /// - `instructions` → `instruction` (emotion / dialect / style control)
    /// - `word_timestamps` → `word_timestamp_enabled`
    /// - `seed` → `seed` (determinism)
    /// - `language` → `language_hints` (single-element hint list)
    ///
    /// The provider-specific knobs without a shared field flow through `extras`: `region`,
    /// `bit_rate` (Opus), `hot_fix` (text hotpatch / pronunciation override), `enable_markdown_filter`,
    /// and `enable_aigc_tag` (AIGC watermark). The Qwen3 realtime endpoint (session.update) is
    /// instruction-driven and only honors voice/format/sample_rate/speed; CosyVoice-only knobs are
    /// emitted only on the inference path (see `create_cosyvoice_run_task`).
    pub fn from_standard(
        std: &crate::core::tts::standard::StandardTTSConfig,
    ) -> Result<Self, TTSError> {
        let f = &std.features;
        let mut cfg = Self::from_base(std.base.clone())?;

        if let Some(speed) = f.speed {
            cfg.rate = speed;
        }
        if let Some(pitch) = f.pitch {
            cfg.pitch = pitch;
        }
        if let Some(volume) = f.volume {
            cfg.volume = volume.clamp(0.0, 100.0) as u8;
        }
        if let Some(rate) = f.sample_rate {
            cfg.sample_rate = rate;
        }

        // CosyVoice inference-protocol knobs (typed shared fields).
        if let Some(ssml) = f.ssml {
            cfg.enable_ssml = Some(ssml);
        }
        if let Some(instruction) = &f.instructions {
            cfg.instruction = Some(instruction.clone());
        }
        if let Some(wt) = f.word_timestamps {
            cfg.word_timestamp_enabled = Some(wt);
        }
        if let Some(seed) = f.seed {
            cfg.seed = Some(seed);
        }
        if let Some(language) = &f.language {
            cfg.language_hints = Some(vec![language.clone()]);
        }

        // Provider-specific passthrough.
        if let Some(region) = std.extras.0.get("region").and_then(|v| v.as_str()) {
            cfg.region = DashScopeRegion::from_str(region);
        }
        if let Some(bit_rate) = std.extras.0.get("bit_rate").and_then(|v| v.as_u64()) {
            cfg.bit_rate = Some(bit_rate as u32);
        }
        if let Some(hot_fix) = std.extras.0.get("hot_fix") {
            cfg.hot_fix = Some(hot_fix.clone());
        }
        if let Some(flag) = std
            .extras
            .0
            .get("enable_markdown_filter")
            .and_then(|v| v.as_bool())
        {
            cfg.enable_markdown_filter = Some(flag);
        }
        if let Some(flag) = std
            .extras
            .0
            .get("enable_aigc_tag")
            .and_then(|v| v.as_bool())
        {
            cfg.enable_aigc_tag = Some(flag);
        }

        cfg.endpoint_override = std.endpoint_override().map(String::from);
        cfg.validate()?;

        Ok(cfg)
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

    // W1 keystone: the standardized prosody features (speed/pitch/volume) and output sample rate
    // reach DashScope's real fields, and the non-standard region flows through the extras
    // passthrough — previously unreachable through the flat factory.
    #[test]
    fn from_standard_maps_prosody_rate_and_region() {
        use crate::core::tts::standard::{ProviderExtras, StandardTTSConfig, TtsFeatures};
        let mut extras = serde_json::Map::new();
        extras.insert("region".into(), serde_json::json!("singapore"));
        let std = StandardTTSConfig {
            base: TTSConfig {
                provider: "alibaba-cloud".into(),
                api_key: "k".into(),
                ..Default::default()
            },
            features: TtsFeatures {
                speed: Some(1.25),
                pitch: Some(0.9),
                volume: Some(80.0),
                sample_rate: Some(24000),
                ssml: Some(true), // capability gap: DashScope has no SSML, must be ignored
                ..Default::default()
            },
            extras: ProviderExtras(extras),
        };
        let cfg = DashScopeTtsConfig::from_standard(&std).unwrap();
        assert_eq!(cfg.rate, 1.25);
        assert_eq!(cfg.pitch, 0.9);
        assert_eq!(cfg.volume, 80);
        assert_eq!(cfg.sample_rate, 24000);
        assert_eq!(cfg.region, DashScopeRegion::Singapore); // extras passthrough
    }

    #[test]
    fn test_config_validation_rejects_ssrf_endpoint_override() {
        let _env = crate::core::net::ssrf_env_lock();
        let mut config = DashScopeTtsConfig::from_base(TTSConfig {
            provider: "alibaba-cloud".into(),
            api_key: "k".into(),
            voice_id: Some("longxiaochun".into()),
            ..Default::default()
        })
        .unwrap();

        config.endpoint_override = Some("wss://dashscope-proxy.example.com".to_string());
        assert!(config.validate().is_ok());

        config.endpoint_override = Some("https://dashscope-proxy.example.com".to_string());
        assert!(config.validate().is_ok());

        config.endpoint_override = Some("ws://127.0.0.1:9000".to_string());
        let err = config
            .validate()
            .expect_err("loopback endpoint_override must be rejected");
        assert!(err.to_string().contains("SSRF protection"));

        config.endpoint_override = Some("file:///tmp/socket".to_string());
        let err = config
            .validate()
            .expect_err("file endpoint_override must be rejected");
        assert!(err.to_string().contains("URL scheme"));

        let std = crate::core::tts::standard::StandardTTSConfig::from_base(TTSConfig {
            provider: "alibaba-cloud".into(),
            api_key: "k".into(),
            voice_id: Some("longxiaochun".into()),
            ..Default::default()
        })
        .with_endpoint_override("file:///tmp/socket");
        assert!(DashScopeTtsConfig::from_standard(&std).is_err());
    }

    // The CosyVoice inference-protocol knobs map from the shared typed fields + extras onto the
    // resolved config (the value that the run-task builder reads). The byte-level reach is proven
    // separately in provider.rs::cosyvoice_features_reach_run_task_payload_parameters.
    #[test]
    fn from_standard_maps_cosyvoice_inference_knobs() {
        use crate::core::tts::standard::{ProviderExtras, StandardTTSConfig, TtsFeatures};
        let mut extras = serde_json::Map::new();
        extras.insert("bit_rate".into(), serde_json::json!(32000));
        extras.insert("hot_fix".into(), serde_json::json!({"AI": "ay eye"}));
        extras.insert("enable_markdown_filter".into(), serde_json::json!(true));
        extras.insert("enable_aigc_tag".into(), serde_json::json!(false));
        let std = StandardTTSConfig {
            base: TTSConfig {
                provider: "alibaba-cloud".into(),
                api_key: "k".into(),
                model: "cosyvoice-v3-flash".into(),
                ..Default::default()
            },
            features: TtsFeatures {
                ssml: Some(true),
                instructions: Some("happy".into()),
                word_timestamps: Some(true),
                seed: Some(7),
                language: Some("en".into()),
                ..Default::default()
            },
            extras: ProviderExtras(extras),
        };
        let cfg = DashScopeTtsConfig::from_standard(&std).unwrap();
        assert_eq!(cfg.enable_ssml, Some(true));
        assert_eq!(cfg.instruction, Some("happy".to_string()));
        assert_eq!(cfg.word_timestamp_enabled, Some(true));
        assert_eq!(cfg.seed, Some(7));
        assert_eq!(cfg.language_hints, Some(vec!["en".to_string()]));
        assert_eq!(cfg.bit_rate, Some(32000));
        assert_eq!(cfg.hot_fix, Some(serde_json::json!({"AI": "ay eye"})));
        assert_eq!(cfg.enable_markdown_filter, Some(true));
        assert_eq!(cfg.enable_aigc_tag, Some(false));
    }

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
