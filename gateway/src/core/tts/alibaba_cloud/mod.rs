//! Alibaba Cloud DashScope Text-to-Speech provider.
//!
//! This module provides integration with Alibaba Cloud's DashScope Model Studio
//! Text-to-Speech WebSocket API.
//!
//! # Architecture
//!
//! The DashScope TTS provider supports two message protocols:
//!
//! 1. **Qwen3-TTS**: OpenAI-like realtime protocol for Qwen models
//! 2. **CosyVoice**: DashScope inference protocol for CosyVoice models
//!
//! ```text
//! DashScopeTts
//!     │
//!     ├── Qwen3-TTS (realtime protocol)
//!     │   └── wss://dashscope.aliyuncs.com/api-ws/v1/realtime?model=qwen3-tts-flash-realtime
//!     │
//!     └── CosyVoice (inference protocol)
//!         └── wss://dashscope.aliyuncs.com/api-ws/v1/inference
//! ```
//!
//! # Key Features
//!
//! - **Multiple Models**: CosyVoice v3 Flash (default), CosyVoice v3 Plus, Qwen3-TTS
//! - **Chinese Optimized**: Native Chinese voices with dialect support
//! - **Multilingual**: 25+ languages including Chinese, English, Japanese, Korean
//! - **Low Latency**: WebSocket streaming for real-time synthesis
//! - **Prosody Control**: Speed, pitch, and volume adjustment
//! - **Regional Endpoints**: Beijing (China) and Singapore (International)
//!
//! # Quick Start
//!
//! ## Via Factory Function (Recommended)
//!
//! ```rust,ignore
//! use waav_gateway::core::tts::{create_tts_provider, TTSConfig, BaseTTS};
//!
//! let config = TTSConfig {
//!     provider: "alibaba-cloud".to_string(),
//!     api_key: std::env::var("DASHSCOPE_API_KEY").unwrap(),
//!     voice_id: Some("longxiaochun".to_string()),
//!     audio_format: Some("mp3".to_string()),
//!     sample_rate: Some(22050),
//!     model: "cosyvoice-v3-flash".to_string(),
//!     ..Default::default()
//! };
//!
//! let mut tts = create_tts_provider("alibaba-cloud", config)?;
//! tts.connect().await?;
//! tts.speak("你好世界", true).await?;
//! ```
//!
//! ## Direct Instantiation
//!
//! ```rust,ignore
//! use waav_gateway::core::tts::alibaba_cloud::{DashScopeTts, DashScopeTtsConfig};
//! use waav_gateway::core::tts::{TTSConfig, BaseTTS};
//!
//! let config = TTSConfig {
//!     api_key: std::env::var("DASHSCOPE_API_KEY").unwrap(),
//!     voice_id: Some("Cherry".to_string()),
//!     model: "qwen3-tts-flash-realtime".to_string(),
//!     sample_rate: Some(24000),
//!     ..Default::default()
//! };
//!
//! let mut tts = DashScopeTts::new(config)?;
//! tts.connect().await?;
//! tts.speak("Hello, world!", true).await?;
//! tts.disconnect().await?;
//! ```
//!
//! # TTS Models
//!
//! | Model | Description | Use Case |
//! |-------|-------------|----------|
//! | `cosyvoice-v3-flash` | Fast CosyVoice (default) | General purpose |
//! | `cosyvoice-v3-plus` | Premium CosyVoice | Higher quality |
//! | `cosyvoice-v2` | Legacy CosyVoice | Compatibility |
//! | `qwen3-tts-flash-realtime` | Qwen3 TTS | Real-time applications |
//!
//! # Voices
//!
//! ## CosyVoice Voices
//!
//! | Voice | Language | Description |
//! |-------|----------|-------------|
//! | `longxiaochun` | Chinese | Default female voice |
//! | `longxiaoxia` | Chinese | Female voice |
//! | `longlaotie` | Chinese | Male voice |
//! | `longshu` | Chinese | Male voice |
//! | `longwan` | Chinese | Female voice |
//!
//! ## Qwen3 TTS Voices
//!
//! | Voice | Language | Description |
//! |-------|----------|-------------|
//! | `Cherry` | English | Female voice |
//! | `Serena` | English | Female voice |
//! | `Ethan` | English | Male voice |
//! | `Jennifer` | English | Female voice |
//! | `Shanghai-Jada` | Chinese | Shanghai dialect |
//! | `Beijing-Dylan` | Chinese | Beijing dialect |
//!
//! # Audio Formats
//!
//! | Format | Description | Use Case |
//! |--------|-------------|----------|
//! | `mp3` | MP3 compressed (default) | Bandwidth optimization |
//! | `pcm` | 16-bit PCM | Real-time streaming |
//! | `wav` | WAV with headers | File storage |
//! | `opus` | Opus compressed | Low-bandwidth streaming |
//!
//! # Authentication
//!
//! DashScope uses Bearer token authentication:
//!
//! ```http
//! Authorization: Bearer <DASHSCOPE_API_KEY>
//! ```
//!
//! API keys are obtained from the DashScope console:
//! - China: <https://dashscope.console.aliyun.com/>
//! - International: <https://dashscope-intl.console.aliyun.com/>
//!
//! # Regional Endpoints
//!
//! | Region | Endpoint | Use Case |
//! |--------|----------|----------|
//! | Beijing | `dashscope.aliyuncs.com` | China mainland |
//! | Singapore | `dashscope-intl.aliyuncs.com` | International |
//!
//! # See Also
//!
//! - [`crate::core::tts::BaseTTS`] - Base trait for TTS providers
//! - [`crate::core::tts::create_tts_provider`] - Factory function
//! - [`crate::core::stt::alibaba_cloud`] - DashScope STT provider

mod config;
mod messages;
mod provider;

// =============================================================================
// Public Re-exports
// =============================================================================

// Configuration types
pub use config::{
    DashScopeAudioFormat, DashScopeRegion, DashScopeTtsConfig, DashScopeTtsModel,
    DASHSCOPE_BEIJING_INFERENCE_URL, DASHSCOPE_BEIJING_REALTIME_URL,
    DASHSCOPE_SINGAPORE_INFERENCE_URL, DASHSCOPE_SINGAPORE_REALTIME_URL,
    DEFAULT_AUDIO_FORMAT, DEFAULT_SAMPLE_RATE, DEFAULT_TTS_MODEL, DEFAULT_VOICE,
};

// Message types
pub use messages::{
    CosyVoiceContinuePayload, CosyVoiceContinueTask, CosyVoiceEmptyPayload, CosyVoiceFinishTask,
    CosyVoiceHeader, CosyVoiceOutput, CosyVoiceParameters, CosyVoiceResponse,
    CosyVoiceResponsePayload, CosyVoiceRunPayload, CosyVoiceRunTask, CosyVoiceTextInput,
    CosyVoiceUsage, QwenTtsError, QwenTtsServerMessage, QwenTtsSession, QwenTtsSessionUpdate,
    QwenTtsTextAppend, QwenTtsTextCommit,
};

// Provider types
pub use provider::DashScopeTts;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tts::base::{BaseTTS, ConnectionState, TTSConfig};

    fn create_test_config() -> TTSConfig {
        TTSConfig {
            api_key: "test_api_key".to_string(),
            voice_id: Some("longxiaochun".to_string()),
            sample_rate: Some(22050),
            model: "cosyvoice-v3-flash".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn test_module_exports() {
        // Test config exports
        let _endpoint_beijing_realtime = DASHSCOPE_BEIJING_REALTIME_URL;
        let _endpoint_singapore_realtime = DASHSCOPE_SINGAPORE_REALTIME_URL;
        let _endpoint_beijing_inference = DASHSCOPE_BEIJING_INFERENCE_URL;
        let _endpoint_singapore_inference = DASHSCOPE_SINGAPORE_INFERENCE_URL;
        let _model = DEFAULT_TTS_MODEL;
        let _sample_rate = DEFAULT_SAMPLE_RATE;
        let _format = DEFAULT_AUDIO_FORMAT;
        let _voice = DEFAULT_VOICE;

        // Test enum exports
        let _region = DashScopeRegion::Beijing;
        let _model = DashScopeTtsModel::CosyVoiceV3Flash;
        let _format = DashScopeAudioFormat::Mp3;

        // Test config exports
        let _config = DashScopeTtsConfig::default();
    }

    #[test]
    fn test_provider_creation() {
        let config = create_test_config();
        let result = DashScopeTts::new(config);
        assert!(result.is_ok());

        let tts = result.unwrap();
        assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);
    }

    #[test]
    fn test_provider_info() {
        let config = create_test_config();
        let tts = DashScopeTts::new(config).unwrap();

        let info = tts.get_provider_info();
        let info_str = info.to_string();
        assert!(info_str.contains("alibaba-cloud") || info_str.contains("DashScope"));
        assert!(info["provider"] == "alibaba-cloud");
    }

    #[test]
    fn test_supported_voices() {
        let voices = DashScopeTtsConfig::supported_voices();
        assert!(voices.len() >= 10);
        assert!(voices.contains(&"longxiaochun"));
        assert!(voices.contains(&"Cherry"));
    }

    #[test]
    fn test_supported_models() {
        let models = DashScopeTtsConfig::supported_models();
        assert!(models.len() >= 4);
        assert!(models.contains(&"cosyvoice-v3-flash"));
        assert!(models.contains(&"qwen3-tts-flash-realtime"));
    }

    #[test]
    fn test_region_endpoints() {
        assert!(DashScopeRegion::Beijing.realtime_url().contains("dashscope.aliyuncs.com"));
        assert!(DashScopeRegion::Singapore.realtime_url().contains("dashscope-intl"));
        assert!(DashScopeRegion::Beijing.inference_url().contains("inference"));
        assert!(DashScopeRegion::Singapore.inference_url().contains("inference"));
    }

    #[test]
    fn test_qwen_session_update_creation() {
        let msg = QwenTtsSessionUpdate::new("Cherry", "pcm", 24000, Some(1.0));
        let json = msg.to_json().unwrap();

        assert!(json.contains("session.update"));
        assert!(json.contains("Cherry"));
        assert!(json.contains("24000"));
    }

    #[test]
    fn test_cosyvoice_run_task_creation() {
        let msg = CosyVoiceRunTask::new(
            "cosyvoice-v3-flash",
            "longxiaochun",
            "mp3",
            22050,
            50,
            1.0,
            1.0,
        );
        let json = msg.to_json().unwrap();

        assert!(json.contains("run-task"));
        assert!(json.contains("cosyvoice-v3-flash"));
        assert!(json.contains("longxiaochun"));
        assert!(!msg.task_id().is_empty());
    }

    #[test]
    fn test_qwen_server_message_parsing() {
        let json = r#"{
            "type": "response.audio.delta",
            "delta": "SGVsbG8gV29ybGQ="
        }"#;

        let msg = QwenTtsServerMessage::from_json(json).unwrap();
        assert!(msg.is_audio_delta());
        assert!(msg.get_audio_data().is_some());
    }

    #[test]
    fn test_cosyvoice_response_parsing() {
        let json = r#"{
            "header": {
                "task_id": "test-id",
                "action": "run-task",
                "streaming": "out",
                "event": "result-generated"
            },
            "payload": {
                "output": {
                    "audio": "SGVsbG8gV29ybGQ="
                }
            }
        }"#;

        let msg = CosyVoiceResponse::from_json(json).unwrap();
        assert!(msg.is_result_generated());
        assert!(msg.get_audio_data().is_some());
    }

    #[test]
    fn test_model_parsing() {
        assert_eq!(
            DashScopeTtsModel::from_str("cosyvoice-v3-flash"),
            DashScopeTtsModel::CosyVoiceV3Flash
        );
        assert_eq!(
            DashScopeTtsModel::from_str("qwen3-tts-flash-realtime"),
            DashScopeTtsModel::Qwen3TtsFlashRealtime
        );
        assert!(DashScopeTtsModel::Qwen3TtsFlashRealtime.is_qwen_model());
        assert!(DashScopeTtsModel::CosyVoiceV3Flash.is_cosyvoice_model());
    }

    #[test]
    fn test_audio_format_parsing() {
        assert_eq!(DashScopeAudioFormat::from_str("mp3"), Some(DashScopeAudioFormat::Mp3));
        assert_eq!(DashScopeAudioFormat::from_str("pcm"), Some(DashScopeAudioFormat::Pcm16));
        assert_eq!(DashScopeAudioFormat::from_str("wav"), Some(DashScopeAudioFormat::Wav));
        assert_eq!(DashScopeAudioFormat::from_str("opus"), Some(DashScopeAudioFormat::Opus));
    }

    #[test]
    fn test_config_validation() {
        // Valid config
        let mut config = DashScopeTtsConfig::default();
        config.api_key = "test_key".to_string();
        assert!(config.validate().is_ok());

        // Invalid rate
        config.rate = 3.0;
        assert!(config.validate().is_err());

        // Invalid volume
        config.rate = 1.0;
        config.volume = 150;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_websocket_url_construction() {
        // CosyVoice model
        let mut config = DashScopeTtsConfig::default();
        config.model = DashScopeTtsModel::CosyVoiceV3Flash;
        config.region = DashScopeRegion::Singapore;

        let url = config.get_websocket_url();
        assert!(url.contains("dashscope-intl"));
        assert!(url.contains("inference"));

        // Qwen model
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

    #[tokio::test]
    async fn test_disconnect_when_not_connected() {
        let config = create_test_config();
        let mut tts = DashScopeTts::new(config).unwrap();

        let result = tts.disconnect().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_flush_ok() {
        let config = create_test_config();
        let tts = DashScopeTts::new(config).unwrap();

        let result = tts.flush().await;
        assert!(result.is_ok());
    }
}
