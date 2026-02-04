//! Alibaba Cloud DashScope STT (Speech-to-Text) Provider
//!
//! This module provides integration with Alibaba Cloud's DashScope Model Studio
//! Speech-to-Text WebSocket API.
//!
//! # Overview
//!
//! Alibaba Cloud (阿里云) provides comprehensive speech AI services through
//! DashScope Model Studio. The STT service supports:
//!
//! - 25+ languages including Chinese dialects
//! - Multiple models: Qwen3-ASR, Paraformer, FunASR
//! - Real-time streaming recognition
//! - Emotion recognition
//! - Word-level timestamps
//!
//! # Models
//!
//! | Model | Description | Sample Rates |
//! |-------|-------------|--------------|
//! | qwen3-asr-flash-realtime | Latest Qwen3 ASR | 8kHz, 16kHz |
//! | paraformer-realtime-v2 | Multilingual | 8-48kHz |
//! | paraformer-realtime-8k-v2 | Telephony | 8kHz |
//! | fun-asr-realtime | FunASR model | Various |
//!
//! # Authentication
//!
//! DashScope uses Bearer token authentication:
//! ```text
//! Authorization: Bearer <DASHSCOPE_API_KEY>
//! ```
//!
//! API keys are obtained from the DashScope console:
//! - China: https://dashscope.console.aliyun.com/
//! - International: https://dashscope-intl.console.aliyun.com/
//!
//! # Usage
//!
//! ```rust,ignore
//! use waav_gateway::core::stt::{BaseSTT, STTConfig};
//! use waav_gateway::core::stt::alibaba_cloud::DashScopeStt;
//!
//! let config = STTConfig {
//!     api_key: std::env::var("DASHSCOPE_API_KEY").unwrap(),
//!     language: "zh".to_string(),
//!     sample_rate: 16000,
//!     model: Some("qwen3-asr-flash-realtime".to_string()),
//!     ..Default::default()
//! };
//!
//! let mut stt = DashScopeStt::new(config)?;
//!
//! // Register callbacks
//! stt.on_result(Arc::new(|result| {
//!     println!("Transcript: {}", result.text);
//! }))?;
//!
//! // Connect and stream audio
//! stt.connect().await?;
//! stt.send_audio(audio_bytes).await?;
//! stt.disconnect().await?;
//! ```
//!
//! # Supported Languages
//!
//! - Chinese: Mandarin, Cantonese, Sichuanese, Wu, Minnan
//! - European: English, German, French, Spanish, Portuguese, Italian, Russian
//! - Asian: Japanese, Korean, Vietnamese, Thai, Indonesian, Hindi
//! - Others: Arabic, Turkish, Ukrainian, Malay, and more
//!
//! # Regions
//!
//! - Beijing (China): `dashscope.aliyuncs.com`
//! - Singapore (International): `dashscope-intl.aliyuncs.com`

pub mod client;
pub mod config;
pub mod messages;

// Re-export main types
pub use client::DashScopeStt;
pub use config::{
    DASHSCOPE_BEIJING_INFERENCE_URL, DASHSCOPE_BEIJING_REALTIME_URL,
    DASHSCOPE_SINGAPORE_INFERENCE_URL, DASHSCOPE_SINGAPORE_REALTIME_URL, DEFAULT_AUDIO_FORMAT,
    DEFAULT_SAMPLE_RATE, DEFAULT_SILENCE_DURATION_MS, DEFAULT_STT_MODEL, DashScopeAudioFormat,
    DashScopeLanguage, DashScopeRegion, DashScopeSttConfig, DashScopeSttModel, TurnDetectionMode,
};
pub use messages::{
    DashScopeErrorCode, ParaformerFinishTask, ParaformerHeader, ParaformerOutput,
    ParaformerParameters, ParaformerPayload, ParaformerResponse, ParaformerResponseHeader,
    ParaformerResponsePayload, ParaformerRunTask, ParaformerSentence, ParaformerUsage,
    ParaformerWord, QwenAudioBufferAppend, QwenAudioBufferCommit, QwenAudioTranscriptionConfig,
    QwenEmotion, QwenError, QwenServerMessage, QwenSessionConfig, QwenSessionFinish,
    QwenSessionInfo, QwenSessionUpdate, QwenTurnDetection,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::stt::base::{BaseSTT, STTConfig, STTError};

    fn create_test_api_key() -> String {
        "sk-test-api-key-xxxxxxxxxxxxxxxx".to_string()
    }

    fn create_test_config() -> STTConfig {
        STTConfig {
            api_key: create_test_api_key(),
            language: "zh".to_string(),
            sample_rate: 16000,
            encoding: "pcm".to_string(),
            model: "qwen3-asr-flash-realtime".to_string(),
            ..Default::default()
        }
    }

    // Integration tests for the module
    #[test]
    fn test_module_exports() {
        // Test config exports
        let _endpoint_beijing = DASHSCOPE_BEIJING_REALTIME_URL;
        let _endpoint_singapore = DASHSCOPE_SINGAPORE_REALTIME_URL;
        let _inference_beijing = DASHSCOPE_BEIJING_INFERENCE_URL;
        let _inference_singapore = DASHSCOPE_SINGAPORE_INFERENCE_URL;
        let _model = DEFAULT_STT_MODEL;
        let _sample_rate = DEFAULT_SAMPLE_RATE;
        let _format = DEFAULT_AUDIO_FORMAT;
        let _silence = DEFAULT_SILENCE_DURATION_MS;

        // Test enum exports
        let _region = DashScopeRegion::Beijing;
        let _model = DashScopeSttModel::Qwen3AsrFlashRealtime;
        let _format = DashScopeAudioFormat::Pcm16;
        let _language = DashScopeLanguage::Chinese;
        let _turn = TurnDetectionMode::ServerVad;

        // Test config exports
        let _config = DashScopeSttConfig::default();
    }

    #[test]
    fn test_provider_creation() {
        let config = create_test_config();
        let result = DashScopeStt::new(config);
        assert!(result.is_ok());

        let stt = result.unwrap();
        assert!(!stt.is_ready());
    }

    #[test]
    fn test_provider_info() {
        let config = create_test_config();
        let stt = DashScopeStt::new(config).unwrap();

        let info = stt.get_provider_info();
        assert!(info.contains("DashScope") || info.contains("阿里云"));
    }

    #[test]
    fn test_supported_languages() {
        let languages = DashScopeSttConfig::supported_languages();
        assert!(languages.len() >= 20);
        assert!(languages.contains(&"zh"));
        assert!(languages.contains(&"en"));
        assert!(languages.contains(&"ja"));
        assert!(languages.contains(&"ko"));
    }

    #[test]
    fn test_supported_models() {
        let models = DashScopeSttConfig::supported_models();
        assert!(models.len() >= 4);
        assert!(models.contains(&"qwen3-asr-flash-realtime"));
        assert!(models.contains(&"paraformer-realtime-v2"));
    }

    #[test]
    fn test_region_endpoints() {
        assert!(
            DashScopeRegion::Beijing
                .realtime_url()
                .contains("dashscope.aliyuncs.com")
        );
        assert!(
            DashScopeRegion::Singapore
                .realtime_url()
                .contains("dashscope-intl")
        );
        assert!(
            DashScopeRegion::Beijing
                .inference_url()
                .contains("inference")
        );
        assert!(
            DashScopeRegion::Singapore
                .inference_url()
                .contains("inference")
        );
    }

    #[test]
    fn test_qwen_session_update_creation() {
        let msg = QwenSessionUpdate::new("zh", 16000, "pcm16", 400, "server_vad");
        let json = msg.to_json().unwrap();

        assert!(json.contains("session.update"));
        assert!(json.contains("\"zh\""));
        assert!(json.contains("16000"));
        assert!(json.contains("server_vad"));
    }

    #[test]
    fn test_qwen_audio_buffer_append() {
        let audio = vec![0u8; 1024];
        let msg = QwenAudioBufferAppend::from_bytes(&audio);
        let json = msg.to_json().unwrap();

        assert!(json.contains("input_audio_buffer.append"));
        assert!(json.contains("audio"));
    }

    #[test]
    fn test_paraformer_run_task() {
        let msg = ParaformerRunTask::new("paraformer-realtime-v2", "pcm", 16000, "zh", true, true);
        let json = msg.to_json().unwrap();

        assert!(json.contains("run-task"));
        assert!(json.contains("paraformer-realtime-v2"));
        assert!(!msg.task_id().is_empty());
    }

    #[test]
    fn test_qwen_server_message_parsing() {
        let json = r#"{
            "type": "conversation.item.input_audio_transcription.completed",
            "transcript": "测试文本"
        }"#;

        let msg = QwenServerMessage::from_json(json).unwrap();
        assert!(msg.is_transcription_completed());
        assert_eq!(msg.get_transcript(), Some("测试文本"));
    }

    #[test]
    fn test_paraformer_response_parsing() {
        let json = r#"{
            "header": {
                "task_id": "test-id",
                "event": "result-generated"
            },
            "payload": {
                "output": {
                    "sentence": {
                        "begin_time": 0,
                        "end_time": 1000,
                        "text": "你好",
                        "words": [],
                        "sentence_end": true
                    }
                }
            }
        }"#;

        let msg = ParaformerResponse::from_json(json).unwrap();
        assert!(msg.is_result_generated());
        assert!(msg.is_final());
        assert_eq!(msg.get_transcript(), Some("你好"));
    }

    #[test]
    fn test_error_code_parsing() {
        assert_eq!(
            DashScopeErrorCode::from_str("200"),
            DashScopeErrorCode::Success
        );
        assert_eq!(
            DashScopeErrorCode::from_str("401"),
            DashScopeErrorCode::Unauthorized
        );
        assert_eq!(
            DashScopeErrorCode::from_str("429"),
            DashScopeErrorCode::RateLimitExceeded
        );
    }

    #[tokio::test]
    async fn test_send_audio_not_connected() {
        let config = create_test_config();
        let mut stt = DashScopeStt::new(config).unwrap();

        let result = stt
            .send_audio(bytes::Bytes::from_static(&[0u8; 1024]))
            .await;
        assert!(matches!(result, Err(STTError::ConnectionFailed(_))));
    }

    #[tokio::test]
    async fn test_disconnect_when_not_connected() {
        let config = create_test_config();
        let mut stt = DashScopeStt::new(config).unwrap();

        let result = stt.disconnect().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_callback_registration() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        struct TestCallback {
            result_received: AtomicBool,
            error_received: AtomicBool,
        }

        let config = create_test_config();
        let mut stt = DashScopeStt::new(config).unwrap();

        let callback = Arc::new(TestCallback {
            result_received: AtomicBool::new(false),
            error_received: AtomicBool::new(false),
        });

        let result_callback = callback.clone();
        let result = stt
            .on_result(Arc::new(move |_result| {
                let cb = result_callback.clone();
                Box::pin(async move {
                    cb.result_received.store(true, Ordering::Relaxed);
                })
            }))
            .await;
        assert!(result.is_ok());

        let error_callback = callback.clone();
        let result = stt
            .on_error(Arc::new(move |_error| {
                let cb = error_callback.clone();
                Box::pin(async move {
                    cb.error_received.store(true, Ordering::Relaxed);
                })
            }))
            .await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_model_parsing() {
        // Test Qwen model
        assert_eq!(
            DashScopeSttModel::from_str("qwen3-asr-flash-realtime"),
            DashScopeSttModel::Qwen3AsrFlashRealtime
        );
        assert_eq!(
            DashScopeSttModel::from_str("qwen-asr"),
            DashScopeSttModel::Qwen3AsrFlashRealtime
        );

        // Test Paraformer models
        assert_eq!(
            DashScopeSttModel::from_str("paraformer-realtime-v2"),
            DashScopeSttModel::ParaformerRealtimeV2
        );
        assert_eq!(
            DashScopeSttModel::from_str("paraformer"),
            DashScopeSttModel::ParaformerRealtimeV2
        );
        assert_eq!(
            DashScopeSttModel::from_str("paraformer-8k"),
            DashScopeSttModel::Paraformer8kRealtimeV2
        );

        // Test FunASR
        assert_eq!(
            DashScopeSttModel::from_str("fun-asr-realtime"),
            DashScopeSttModel::FunAsrRealtime
        );
    }

    #[test]
    fn test_audio_format_parsing() {
        assert_eq!(
            DashScopeAudioFormat::from_str("pcm"),
            Some(DashScopeAudioFormat::Pcm16)
        );
        assert_eq!(
            DashScopeAudioFormat::from_str("linear16"),
            Some(DashScopeAudioFormat::Pcm16)
        );
        assert_eq!(
            DashScopeAudioFormat::from_str("opus"),
            Some(DashScopeAudioFormat::Opus)
        );
        assert_eq!(
            DashScopeAudioFormat::from_str("wav"),
            Some(DashScopeAudioFormat::Wav)
        );
        assert_eq!(
            DashScopeAudioFormat::from_str("mp3"),
            Some(DashScopeAudioFormat::Mp3)
        );
    }

    #[test]
    fn test_language_parsing() {
        assert_eq!(
            DashScopeLanguage::from_str("zh"),
            DashScopeLanguage::Chinese
        );
        assert_eq!(
            DashScopeLanguage::from_str("zh_cn"),
            DashScopeLanguage::Chinese
        );
        assert_eq!(
            DashScopeLanguage::from_str("en"),
            DashScopeLanguage::English
        );
        assert_eq!(
            DashScopeLanguage::from_str("yue"),
            DashScopeLanguage::Cantonese
        );
        assert_eq!(
            DashScopeLanguage::from_str("cantonese"),
            DashScopeLanguage::Cantonese
        );
        assert_eq!(DashScopeLanguage::from_str("auto"), DashScopeLanguage::Auto);
    }

    #[test]
    fn test_config_validation() {
        // Valid config
        let mut config = DashScopeSttConfig::default();
        config.api_key = "test_key".to_string();
        assert!(config.validate().is_ok());

        // Invalid sample rate
        config.sample_rate = 44100;
        assert!(config.validate().is_err());

        // Invalid audio format for Qwen
        config.sample_rate = 16000;
        config.model = DashScopeSttModel::Qwen3AsrFlashRealtime;
        config.audio_format = DashScopeAudioFormat::Mp3;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_websocket_url_construction() {
        // Qwen model
        let mut config = DashScopeSttConfig::default();
        config.model = DashScopeSttModel::Qwen3AsrFlashRealtime;
        config.region = DashScopeRegion::Singapore;

        let url = config.get_websocket_url();
        assert!(url.contains("dashscope-intl"));
        assert!(url.contains("realtime"));
        assert!(url.contains("qwen3-asr-flash-realtime"));

        // Paraformer model
        config.model = DashScopeSttModel::ParaformerRealtimeV2;
        let url = config.get_websocket_url();
        assert!(url.contains("inference"));
    }

    #[test]
    fn test_from_base_config() {
        let base = STTConfig {
            api_key: "test_key".to_string(),
            language: "zh".to_string(),
            sample_rate: 16000,
            encoding: "pcm".to_string(),
            model: "paraformer-realtime-v2".to_string(),
            ..Default::default()
        };

        let config = DashScopeSttConfig::from_base(base).unwrap();
        assert_eq!(config.model, DashScopeSttModel::ParaformerRealtimeV2);
        assert_eq!(config.language, DashScopeLanguage::Chinese);
    }
}
