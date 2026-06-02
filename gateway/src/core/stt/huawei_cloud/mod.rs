//! Huawei Cloud Speech Interaction Service (SIS) STT Provider
//!
//! This module provides integration with Huawei Cloud's Speech-to-Text (语音识别)
//! service through both WebSocket real-time streaming and REST API.
//!
//! # Architecture
//!
//! The Huawei Cloud STT provider supports three operation modes:
//!
//! 1. **Short Sentence (REST)**: HTTP-based recognition for audio up to 30 seconds
//! 2. **Streaming (WebSocket)**: Real-time streaming for audio up to 1 minute
//! 3. **Continuous (WebSocket)**: Long-running streaming for audio up to 5 hours
//!
//! ```text
//! HuaweiCloudStt
//!     │
//!     ├── Short Sentence Mode (REST)
//!     │   └── POST /v1/{project_id}/asr/short-audio
//!     │
//!     ├── Streaming Mode (WebSocket)
//!     │   └── wss://.../v1/{project_id}/rasr/short-stream
//!     │
//!     └── Continuous Mode (WebSocket)
//!         └── wss://.../v1/{project_id}/rasr/continue-stream
//! ```
//!
//! # Key Features
//!
//! - **Multiple Languages**: Mandarin, Cantonese, Sichuanese, Hokkienese, Mongolian, Tibetan, Uyghur
//! - **Real-time Streaming**: WebSocket with interim and final results
//! - **Custom Vocabulary**: Hotword tables for Mandarin models
//! - **IAM Authentication**: Token-based auth (24-hour validity)
//! - **Word-level Timing**: Optional word timestamps in results
//!
//! # Quick Start
//!
//! ## Via Factory Function (Recommended)
//!
//! ```rust,ignore
//! use waav_gateway::core::stt::{create_stt_provider, STTConfig, BaseSTT};
//!
//! let config = STTConfig {
//!     provider: "huawei_cloud".to_string(),
//!     // API key format: username|password|domain_name|project_id
//!     api_key: "user|pass|domain|project123".to_string(),
//!     language: "zh".to_string(),
//!     sample_rate: 16000,
//!     encoding: "pcm16k16bit".to_string(),
//!     model: "chinese_16k_general".to_string(),
//!     ..Default::default()
//! };
//!
//! let mut stt = create_stt_provider("huawei_cloud", config)?;
//! stt.connect().await?;
//! stt.send_audio(audio_data).await?;
//! ```
//!
//! ## Direct Instantiation
//!
//! ```rust,ignore
//! use waav_gateway::core::stt::huawei_cloud::{HuaweiCloudStt, HuaweiCloudSttConfig};
//! use waav_gateway::core::stt::{STTConfig, BaseSTT};
//!
//! let config = STTConfig {
//!     api_key: "user|pass|domain|project123".to_string(),
//!     language: "zh".to_string(),
//!     sample_rate: 16000,
//!     encoding: "pcm16k16bit".to_string(),
//!     model: "chinese_16k_general".to_string(),
//!     ..Default::default()
//! };
//!
//! let mut stt = HuaweiCloudStt::new(config)?;
//! stt.connect().await?;
//! stt.send_audio(audio_data).await?;
//! stt.disconnect().await?;
//! ```
//!
//! # Recognition Models (property)
//!
//! | Model | Property | Language | Sample Rate |
//! |-------|----------|----------|-------------|
//! | Chinese16kGeneral | chinese_16k_general | Mandarin | 16kHz |
//! | Chinese8kGeneral | chinese_8k_general | Mandarin | 8kHz |
//! | Chinese16kCommon | chinese_16k_common | Mandarin | 16kHz |
//! | Cantonese16kGeneral | cantonese_16k_general | Cantonese | 16kHz |
//! | Sichuan16kGeneral | sichuan_16k_general | Sichuanese | 16kHz |
//! | Minnan16kGeneral | minnan_16k_general | Hokkienese | 16kHz |
//! | Mongolian16kGeneral | mongolian_16k_general | Mongolian | 16kHz |
//! | Tibetan16kGeneral | tibetan_16k_general | Tibetan | 16kHz |
//! | Uyghur16kGeneral | uyghur_16k_general | Uyghur | 16kHz |
//!
//! # Audio Formats
//!
//! | Format | Description |
//! |--------|-------------|
//! | pcm8k16bit | PCM, 8kHz sampling, 16-bit |
//! | pcm16k16bit | PCM, 16kHz sampling, 16-bit |
//! | wav | WAV format |
//! | mp3 | MP3 format |
//! | amr | AMR format |
//! | amr-wb | AMR-WB format |
//! | aac | AAC format |
//! | ogg-opus | OGG Opus format |
//! | m4a | M4A format |
//!
//! # Regions
//!
//! | Region | Code | Endpoint |
//! |--------|------|----------|
//! | CN North (Beijing) | cn-north-4 | sis.cn-north-4.myhuaweicloud.com |
//! | CN East (Shanghai) | cn-east-3 | sis.cn-east-3.myhuaweicloud.com |
//! | AP Singapore | ap-southeast-3 | sis-ext.ap-southeast-3.myhuaweicloud.com |
//! | AP Hong Kong | ap-southeast-1 | sis-ext.ap-southeast-1.myhuaweicloud.com |
//! | AP Bangkok | ap-southeast-2 | sis-ext.ap-southeast-2.myhuaweicloud.com |
//!
//! # Authentication
//!
//! Huawei Cloud uses IAM token-based authentication:
//!
//! 1. Username, Password, Domain Name, and Project ID required
//! 2. Token endpoint: `https://iam.{region}.myhuaweicloud.com/v3/auth/tokens`
//! 3. Token returned in `X-Subject-Token` response header
//! 4. Token validity: 24 hours (auto-refreshed)
//!
//! **API Key Format**: `username|password|domain_name|project_id` (pipe-separated)
//!
//! # WebSocket Protocol
//!
//! 1. Connect to WebSocket endpoint with `X-Auth-Token` header
//! 2. Send START command (JSON) with config
//! 3. Receive STARTED confirmation
//! 4. Send audio frames (binary PCM, 200ms chunks recommended)
//! 5. Receive RESULT (interim) and END (final) results
//! 6. Send END command (JSON) when done
//! 7. Receive ENDED confirmation
//!
//! # Error Codes
//!
//! | Code | Description |
//! |------|-------------|
//! | SIS.0001 | Authentication failed |
//! | SIS.0002 | Token expired |
//! | SIS.0003 | Invalid parameter |
//! | SIS.0004 | Audio format mismatch |
//! | SIS.0005 | Service unavailable |
//! | SIS.0006 | Rate limit exceeded |
//! | SIS.0007 | Quota exceeded |
//!
//! # See Also
//!
//! - [`crate::core::stt::BaseSTT`] - Base trait for STT providers
//! - [`crate::core::stt::create_stt_provider`] - Factory function
//! - [`crate::core::tts::huawei_cloud`] - Huawei Cloud TTS provider

mod auth;
mod client;
mod config;
mod messages;

// =============================================================================
// Public Re-exports
// =============================================================================

// Configuration types
pub use config::{
    DEFAULT_AUDIO_FORMAT, DEFAULT_MODEL, DEFAULT_SAMPLE_RATE, HuaweiCloudAsrMode,
    HuaweiCloudAudioFormat, HuaweiCloudRegion, HuaweiCloudSttConfig, HuaweiCloudSttModel,
    HuaweiIamDomain, HuaweiIamError, HuaweiIamProject, HuaweiIamToken, HuaweiIamTokenResponse,
    HuaweiIamUser, MAX_CONTINUOUS_DURATION_SECS, MAX_FRAME_INTERVAL_SECS,
    MAX_SHORT_AUDIO_DURATION_SECS, MAX_STREAMING_DURATION_SECS, REALTIME_ASR_CONTINUOUS_PATH,
    REALTIME_ASR_STREAMING_PATH, RECOMMENDED_CHUNK_DURATION_MS, SHORT_ASR_PATH,
    SIS_CHINA_ENDPOINT_FORMAT, SIS_INTL_ENDPOINT_FORMAT, TOKEN_VALIDITY_SECS, WS_TIMEOUT_SECS,
};

// Message types
pub use messages::{
    HuaweiAsrResult, HuaweiCancelFrame, HuaweiEndFrame, HuaweiRealtimeResponse, HuaweiResponseType,
    HuaweiShortAsrConfig, HuaweiShortAsrRequest, HuaweiShortAsrResponse, HuaweiSisErrorCode,
    HuaweiStartConfig, HuaweiStartFrame, HuaweiWordInfo, HuaweiWsCommand,
};

// Auth types
pub use auth::HuaweiTokenManager;

// Client
pub use client::HuaweiCloudStt;

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::stt::base::{BaseSTT, STTConfig};

    fn create_test_config() -> STTConfig {
        STTConfig {
            api_key: "test_user|test_pass|test_domain|test_project".to_string(),
            language: "zh".to_string(),
            sample_rate: 16000,
            encoding: "pcm16k16bit".to_string(),
            model: "chinese_16k_general".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn test_module_exports() {
        // Test constant exports
        let _default_model = DEFAULT_MODEL;
        let _default_sample_rate = DEFAULT_SAMPLE_RATE;
        let _default_format = DEFAULT_AUDIO_FORMAT;
        let _token_validity = TOKEN_VALIDITY_SECS;
        let _max_short = MAX_SHORT_AUDIO_DURATION_SECS;
        let _max_streaming = MAX_STREAMING_DURATION_SECS;
        let _max_continuous = MAX_CONTINUOUS_DURATION_SECS;
        let _chunk_duration = RECOMMENDED_CHUNK_DURATION_MS;
        let _frame_interval = MAX_FRAME_INTERVAL_SECS;
        let _ws_timeout = WS_TIMEOUT_SECS;

        // Test enum exports
        let _model = HuaweiCloudSttModel::Chinese16kGeneral;
        let _format = HuaweiCloudAudioFormat::Pcm16k16bit;
        let _region = HuaweiCloudRegion::CnNorth4;
        let _mode = HuaweiCloudAsrMode::Streaming;
        let _error_code = HuaweiSisErrorCode::Success;
        let _response_type = HuaweiResponseType::Result;
        let _ws_command = HuaweiWsCommand::Start;

        // Test struct exports
        let _config = HuaweiCloudSttConfig::default();
    }

    #[test]
    fn test_provider_creation() {
        let config = create_test_config();
        let result = HuaweiCloudStt::new(config);
        assert!(result.is_ok());

        let stt = result.unwrap();
        assert!(!stt.is_ready());
    }

    #[test]
    fn test_provider_info() {
        let config = create_test_config();
        let stt = HuaweiCloudStt::new(config).unwrap();

        let info = stt.get_provider_info();
        assert!(info.contains("Huawei") || info.contains("华为"));
    }

    // W1 keystone: Huawei's mappable recognition knobs (word-level timing + smart
    // formatting/punctuation) must survive through `new_standard` into the provider-specific
    // config — previously dropped by the flat factory.
    #[test]
    fn new_standard_unlocks_recognition_features() {
        use crate::core::stt::standard::{ProviderExtras, SttFeatures, StandardSTTConfig};
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "huawei_cloud".into(),
                api_key: "user|pass|domain|project123".into(),
                model: "chinese_16k_general".into(),
                encoding: "pcm16k16bit".into(),
                ..Default::default()
            },
            features: SttFeatures {
                word_timestamps: Some(true),
                smart_format: Some(false),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
        };
        // `new_standard` builds the provider from exactly this config, so asserting the mapping
        // here proves the features it carries reach the constructed client (the config field is
        // private to the client module, so we verify via the same `from_standard` it uses).
        let cfg = HuaweiCloudSttConfig::from_standard(&std).unwrap();
        assert!(cfg.need_word_info); // word_timestamps feature
        assert!(!cfg.add_punctuation); // smart_format feature
        assert_eq!(cfg.project_id, "project123");
        assert!(HuaweiCloudStt::new_standard(&std).is_ok());

        // A malformed credential is rejected through the standardized path too.
        let bad = StandardSTTConfig::from_base(STTConfig {
            provider: "huawei_cloud".into(),
            api_key: "incomplete".into(),
            ..Default::default()
        });
        assert!(HuaweiCloudStt::new_standard(&bad).is_err());
    }

    #[test]
    fn test_supported_models() {
        let models = HuaweiCloudSttConfig::supported_models();
        assert!(models.len() >= 10);
        assert!(models.contains(&"chinese_16k_general"));
        assert!(models.contains(&"cantonese_16k_general"));
        assert!(models.contains(&"sichuan_16k_general"));
    }

    #[test]
    fn test_supported_languages() {
        let languages = HuaweiCloudSttConfig::supported_languages();
        assert!(languages.contains(&"zh-CN"));
        assert!(languages.contains(&"zh-HK"));
        assert!(languages.contains(&"mn"));
        assert!(languages.contains(&"bo"));
    }

    #[test]
    fn test_region_enumeration() {
        let regions = HuaweiCloudRegion::all();
        assert_eq!(regions.len(), 5);
        assert!(regions.contains(&HuaweiCloudRegion::CnNorth4));
        assert!(regions.contains(&HuaweiCloudRegion::ApSoutheast3));
    }

    #[test]
    fn test_region_endpoints() {
        let cn_region = HuaweiCloudRegion::CnNorth4;
        assert!(cn_region.sis_endpoint().contains("sis.cn-north-4"));
        assert!(cn_region.is_china_region());

        let intl_region = HuaweiCloudRegion::ApSoutheast3;
        assert!(
            intl_region
                .sis_endpoint()
                .contains("sis-ext.ap-southeast-3")
        );
        assert!(!intl_region.is_china_region());
    }

    #[test]
    fn test_model_parsing() {
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
    fn test_audio_format_parsing() {
        assert_eq!(
            HuaweiCloudAudioFormat::from_str("pcm16k16bit"),
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
    }

    #[test]
    fn test_asr_mode() {
        assert!(!HuaweiCloudAsrMode::ShortSentence.is_websocket());
        assert!(HuaweiCloudAsrMode::Streaming.is_websocket());
        assert!(HuaweiCloudAsrMode::Continuous.is_websocket());

        assert_eq!(HuaweiCloudAsrMode::ShortSentence.max_duration_secs(), 30);
        assert_eq!(HuaweiCloudAsrMode::Streaming.max_duration_secs(), 60);
        assert_eq!(HuaweiCloudAsrMode::Continuous.max_duration_secs(), 18000);
    }

    #[test]
    fn test_start_frame_serialization() {
        let frame = HuaweiStartFrame::new(
            "pcm16k16bit",
            "chinese_16k_general",
            true,
            true,
            None,
            false,
        );

        let json = frame.to_json().unwrap();
        assert!(json.contains("\"command\":\"START\""));
        assert!(json.contains("\"audio_format\":\"pcm16k16bit\""));
        assert!(json.contains("\"property\":\"chinese_16k_general\""));
    }

    #[test]
    fn test_end_frame_serialization() {
        let frame = HuaweiEndFrame::new();
        let json = frame.to_json().unwrap();
        assert!(json.contains("\"command\":\"END\""));
    }

    #[test]
    fn test_cancel_frame_serialization() {
        let frame = HuaweiCancelFrame::new();
        let json = frame.to_json().unwrap();
        assert!(json.contains("\"command\":\"CANCEL\""));
    }

    #[test]
    fn test_realtime_response_parsing() {
        let json = r#"{
            "resp_type": "END",
            "trace_id": "trace123",
            "error_code": 0,
            "result": {
                "text": "你好世界",
                "score": 0.95,
                "is_final": true
            }
        }"#;

        let response = HuaweiRealtimeResponse::from_json(json).unwrap();
        assert!(response.is_success());
        assert!(response.is_final());
        assert_eq!(response.get_transcript(), Some("你好世界"));
    }

    #[test]
    fn test_short_asr_request() {
        let audio_data = vec![0u8; 100];
        let request = HuaweiShortAsrRequest::new(
            &audio_data,
            "pcm16k16bit",
            "chinese_16k_general",
            true,
            true,
            None,
            false,
        );

        let json = request.to_json().unwrap();
        assert!(json.contains("\"audio_format\":\"pcm16k16bit\""));
        assert!(json.contains("\"property\":\"chinese_16k_general\""));
    }

    #[test]
    fn test_config_validation_valid() {
        let config = HuaweiCloudSttConfig::new("user", "pass", "domain", "project_id");
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validation_empty_credentials() {
        let config = HuaweiCloudSttConfig::new("", "pass", "domain", "project_id");
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_urls() {
        let config = HuaweiCloudSttConfig::new("user", "pass", "domain", "project123")
            .with_region(HuaweiCloudRegion::CnNorth4);

        assert!(config.get_https_endpoint().contains("sis.cn-north-4"));
        assert!(config.get_short_asr_url().contains("project123"));
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
    }

    #[test]
    fn test_error_code_handling() {
        assert_eq!(
            HuaweiSisErrorCode::from_code(0),
            HuaweiSisErrorCode::Success
        );
        assert_eq!(
            HuaweiSisErrorCode::from_code(1),
            HuaweiSisErrorCode::AuthenticationFailed
        );
        assert!(HuaweiSisErrorCode::ServiceUnavailable.is_retryable());
        assert!(!HuaweiSisErrorCode::AuthenticationFailed.is_retryable());
    }

    #[tokio::test]
    async fn test_disconnect_when_not_connected() {
        let config = create_test_config();
        let mut stt = HuaweiCloudStt::new(config).unwrap();

        let result = stt.disconnect().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_send_audio_not_connected() {
        let config = create_test_config();
        let mut stt = HuaweiCloudStt::new(config).unwrap();

        let result = stt
            .send_audio(bytes::Bytes::from_static(&[0u8; 1024]))
            .await;
        assert!(result.is_err());
    }

    #[test]
    fn test_token_manager_creation() {
        let _manager = HuaweiTokenManager::new();
    }
}
