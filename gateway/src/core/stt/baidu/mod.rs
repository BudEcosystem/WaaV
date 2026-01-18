//! Baidu AI Cloud Speech-to-Text provider.
//!
//! This module provides integration with Baidu AI Cloud's Speech-to-Text
//! (百度语音识别) WebSocket and REST APIs.
//!
//! # Architecture
//!
//! The Baidu STT provider supports two operation modes:
//!
//! 1. **Real-time WebSocket**: For streaming audio recognition (up to 1 hour)
//! 2. **REST API**: For short audio files (up to 60 seconds)
//!
//! ```text
//! BaiduStt
//!     │
//!     ├── Real-time Mode (WebSocket)
//!     │   └── wss://vop.baidu.com/realtime_asr?sn=UUID
//!     │
//!     └── Short Audio Mode (REST)
//!         └── https://vop.baidu.com/server_api
//! ```
//!
//! # Key Features
//!
//! - **Multiple Languages**: Mandarin, English, Cantonese, Sichuan dialect
//! - **Real-time Streaming**: WebSocket with interim and final results
//! - **Custom Vocabulary**: Supported for Mandarin models
//! - **OAuth 2.0 Auth**: Automatic token refresh (30-day validity)
//!
//! # Quick Start
//!
//! ## Via Factory Function (Recommended)
//!
//! ```rust,ignore
//! use waav_gateway::core::stt::{create_stt_provider, STTConfig, BaseSTT};
//!
//! let config = STTConfig {
//!     provider: "baidu".to_string(),
//!     // API key format: api_key|secret_key
//!     api_key: "your_api_key|your_secret_key".to_string(),
//!     language: "zh".to_string(),
//!     sample_rate: 16000,
//!     encoding: "pcm".to_string(),
//!     model: "mandarin".to_string(),
//!     ..Default::default()
//! };
//!
//! let mut stt = create_stt_provider("baidu", config)?;
//! stt.connect().await?;
//! stt.send_audio(audio_data).await?;
//! ```
//!
//! ## Direct Instantiation
//!
//! ```rust,ignore
//! use waav_gateway::core::stt::baidu::{BaiduStt, BaiduSttConfig};
//! use waav_gateway::core::stt::{STTConfig, BaseSTT};
//!
//! let config = STTConfig {
//!     api_key: "your_api_key|your_secret_key".to_string(),
//!     language: "zh".to_string(),
//!     sample_rate: 16000,
//!     model: "mandarin".to_string(),
//!     ..Default::default()
//! };
//!
//! let mut stt = BaiduStt::new(config)?;
//! stt.connect().await?;
//! stt.send_audio(audio_data).await?;
//! stt.disconnect().await?;
//! ```
//!
//! # Recognition Models (dev_pid)
//!
//! | Model | ID | Language | Custom Vocab |
//! |-------|-----|----------|--------------|
//! | Mandarin | 1537 | Chinese (with punctuation) | Yes |
//! | MandarinNoPunct | 1536 | Chinese (no punctuation) | Yes |
//! | English | 1737 | English | No |
//! | Cantonese | 1637 | Cantonese | No |
//! | Sichuan | 1837 | Sichuan dialect | No |
//! | MandarinFarField | 1936 | Chinese (far-field) | No |
//!
//! # Audio Requirements
//!
//! | Property | Value |
//! |----------|-------|
//! | Sample Rates | 16000 Hz (standard), 8000 Hz (Mandarin only) |
//! | Formats | PCM, WAV, AMR, M4A |
//! | Channels | Mono (1 channel) |
//! | Bit Depth | 16-bit |
//! | Chunk Size | 5120 bytes (160ms at 16kHz) |
//!
//! # Authentication
//!
//! Baidu uses OAuth 2.0 client credentials:
//!
//! 1. API Key and Secret Key from Baidu AI Console
//! 2. Token endpoint: `https://aip.baidubce.com/oauth/2.0/token`
//! 3. Token validity: 30 days (auto-refreshed)
//!
//! **API Key Format**: `api_key|secret_key` (pipe-separated)
//!
//! # WebSocket Protocol
//!
//! 1. Connect to `wss://vop.baidu.com/realtime_asr?sn=UUID`
//! 2. Send START frame (JSON) with auth and config
//! 3. Send audio frames (binary PCM, 160ms chunks)
//! 4. Receive MID_TEXT (interim) and FIN_TEXT (final) results
//! 5. Send FINISH frame (JSON) when done
//!
//! # Error Codes
//!
//! | Code | Description |
//! |------|-------------|
//! | 3300 | Input parameter error |
//! | 3301 | Authentication error |
//! | 3302 | Token invalid/expired |
//! | 3303 | Audio too long |
//! | 3304 | Audio too large |
//! | 3305 | Audio quality issue |
//! | 3306 | Bad audio format |
//! | 3308 | Server busy (retryable) |
//!
//! # See Also
//!
//! - [`crate::core::stt::BaseSTT`] - Base trait for STT providers
//! - [`crate::core::stt::create_stt_provider`] - Factory function
//! - [`crate::core::tts::baidu`] - Baidu TTS provider

mod client;
mod config;
mod messages;

// =============================================================================
// Public Re-exports
// =============================================================================

// Configuration types
pub use config::{
    BaiduAudioFormat, BaiduOAuthError, BaiduOAuthResponse, BaiduSampleRate, BaiduSttConfig,
    BaiduSttModel, BAIDU_OAUTH_URL, BAIDU_REALTIME_ASR_URL, BAIDU_SHORT_ASR_URL,
    BAIDU_SHORT_ASR_URL_HTTPS, DEFAULT_AUDIO_FORMAT, DEFAULT_MODEL, DEFAULT_SAMPLE_RATE,
    MAX_FRAME_INTERVAL_SECS, MAX_REALTIME_AUDIO_DURATION_SECS, MAX_SHORT_AUDIO_DURATION_SECS,
    RECOMMENDED_CHUNK_DURATION_MS, TOKEN_VALIDITY_SECS,
};

// Message types
pub use messages::{
    BaiduCancelFrame, BaiduErrorCode, BaiduFinishFrame, BaiduRealtimeResponse,
    BaiduShortAsrRequest, BaiduShortAsrResponse, BaiduStartData, BaiduStartFrame,
};

// Client
pub use client::BaiduStt;

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::stt::base::{BaseSTT, STTConfig};

    fn create_test_config() -> STTConfig {
        STTConfig {
            api_key: "test_api_key|test_secret_key".to_string(),
            language: "zh".to_string(),
            sample_rate: 16000,
            encoding: "pcm".to_string(),
            model: "mandarin".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn test_module_exports() {
        // Test constant exports
        let _oauth_url = BAIDU_OAUTH_URL;
        let _realtime_url = BAIDU_REALTIME_ASR_URL;
        let _short_url = BAIDU_SHORT_ASR_URL;
        let _short_url_https = BAIDU_SHORT_ASR_URL_HTTPS;
        let _model = DEFAULT_MODEL;
        let _sample_rate = DEFAULT_SAMPLE_RATE;
        let _format = DEFAULT_AUDIO_FORMAT;
        let _token_validity = TOKEN_VALIDITY_SECS;
        let _max_short = MAX_SHORT_AUDIO_DURATION_SECS;
        let _max_realtime = MAX_REALTIME_AUDIO_DURATION_SECS;
        let _chunk_duration = RECOMMENDED_CHUNK_DURATION_MS;
        let _frame_interval = MAX_FRAME_INTERVAL_SECS;

        // Test enum exports
        let _model = BaiduSttModel::Mandarin;
        let _format = BaiduAudioFormat::Pcm;
        let _rate = BaiduSampleRate::Rate16000;
        let _error_code = BaiduErrorCode::Success;

        // Test struct exports
        let _config = BaiduSttConfig::default();
    }

    #[test]
    fn test_provider_creation() {
        let config = create_test_config();
        let result = BaiduStt::new(config);
        assert!(result.is_ok());

        let stt = result.unwrap();
        assert!(!stt.is_ready());
    }

    #[test]
    fn test_provider_info() {
        let config = create_test_config();
        let stt = BaiduStt::new(config).unwrap();

        let info = stt.get_provider_info();
        assert!(info.contains("Baidu") || info.contains("百度"));
    }

    #[test]
    fn test_supported_models() {
        let models = BaiduSttConfig::supported_models();
        assert!(models.len() >= 6);
        assert!(models.contains(&"mandarin"));
        assert!(models.contains(&"english"));
        assert!(models.contains(&"cantonese"));
    }

    #[test]
    fn test_supported_languages() {
        let languages = BaiduSttConfig::supported_languages();
        assert!(languages.contains(&"zh"));
        assert!(languages.contains(&"en"));
        assert!(languages.contains(&"yue"));
    }

    #[test]
    fn test_model_parsing() {
        assert_eq!(BaiduSttModel::from_str("mandarin"), BaiduSttModel::Mandarin);
        assert_eq!(BaiduSttModel::from_str("1537"), BaiduSttModel::Mandarin);
        assert_eq!(BaiduSttModel::from_str("english"), BaiduSttModel::English);
        assert_eq!(BaiduSttModel::from_str("cantonese"), BaiduSttModel::Cantonese);
    }

    #[test]
    fn test_model_dev_pid() {
        assert_eq!(BaiduSttModel::Mandarin.dev_pid(), 1537);
        assert_eq!(BaiduSttModel::English.dev_pid(), 1737);
        assert_eq!(BaiduSttModel::Cantonese.dev_pid(), 1637);
        assert_eq!(BaiduSttModel::Sichuan.dev_pid(), 1837);
    }

    #[test]
    fn test_audio_format_parsing() {
        assert_eq!(BaiduAudioFormat::from_str("pcm"), Some(BaiduAudioFormat::Pcm));
        assert_eq!(BaiduAudioFormat::from_str("wav"), Some(BaiduAudioFormat::Wav));
        assert_eq!(BaiduAudioFormat::from_str("amr"), Some(BaiduAudioFormat::Amr));
        assert_eq!(BaiduAudioFormat::from_str("m4a"), Some(BaiduAudioFormat::M4a));
    }

    #[test]
    fn test_sample_rate_chunk_size() {
        let rate = BaiduSampleRate::Rate16000;
        // 160ms at 16kHz, 16-bit: 16000 * 2 * 160 / 1000 = 5120 bytes
        assert_eq!(rate.chunk_size_for_duration(160), 5120);
    }

    #[test]
    fn test_start_frame_serialization() {
        let frame = BaiduStartFrame::new(
            "app123",
            "key456",
            1537,
            "user789",
            16000,
            "pcm",
        );

        let json = frame.to_json().unwrap();
        assert!(json.contains("\"type\":\"START\""));
        assert!(json.contains("\"appid\":\"app123\""));
        assert!(json.contains("\"dev_pid\":1537"));
    }

    #[test]
    fn test_finish_frame_serialization() {
        let frame = BaiduFinishFrame::new();
        let json = frame.to_json().unwrap();
        assert!(json.contains("\"type\":\"FINISH\""));
    }

    #[test]
    fn test_cancel_frame_serialization() {
        let frame = BaiduCancelFrame::new();
        let json = frame.to_json().unwrap();
        assert!(json.contains("\"type\":\"CANCEL\""));
    }

    #[test]
    fn test_realtime_response_parsing_success() {
        let json = r#"{
            "err_no": 0,
            "err_msg": "success",
            "type": "FIN_TEXT",
            "result": "你好世界",
            "sn": "123456"
        }"#;

        let response = BaiduRealtimeResponse::from_json(json).unwrap();
        assert!(response.is_success());
        assert!(response.is_final());
        assert!(!response.is_interim());
        assert!(!response.is_error());
        assert_eq!(response.get_transcript(), Some("你好世界"));
    }

    #[test]
    fn test_realtime_response_parsing_interim() {
        let json = r#"{
            "err_no": 0,
            "err_msg": "success",
            "type": "MID_TEXT",
            "result": "你好"
        }"#;

        let response = BaiduRealtimeResponse::from_json(json).unwrap();
        assert!(response.is_success());
        assert!(response.is_interim());
        assert!(!response.is_final());
    }

    #[test]
    fn test_realtime_response_parsing_error() {
        let json = r#"{
            "err_no": 3301,
            "err_msg": "Authentication error",
            "type": "ERROR"
        }"#;

        let response = BaiduRealtimeResponse::from_json(json).unwrap();
        assert!(!response.is_success());
        assert!(response.is_error());
        assert!(response.get_error().is_some());
    }

    #[test]
    fn test_short_asr_request() {
        let audio_data = vec![0u8; 100];
        let request = BaiduShortAsrRequest::new(
            "pcm",
            16000,
            "user123",
            "token456",
            1537,
            &audio_data,
        );

        assert_eq!(request.format, "pcm");
        assert_eq!(request.rate, 16000);
        assert_eq!(request.channel, 1);
        assert_eq!(request.len, 100);
        assert!(!request.speech.is_empty());
    }

    #[test]
    fn test_short_asr_response_parsing() {
        let json = r#"{
            "err_no": 0,
            "err_msg": "success.",
            "sn": "481D633F-73BA-726F-49EF-8659ACCC2F3D",
            "result": ["北京天气"],
            "corpus_no": "6890859905390146256"
        }"#;

        let response = BaiduShortAsrResponse::from_json(json).unwrap();
        assert!(response.is_success());
        assert_eq!(response.get_transcript(), Some("北京天气"));
    }

    #[test]
    fn test_error_code_parsing() {
        assert_eq!(BaiduErrorCode::from_code(0), BaiduErrorCode::Success);
        assert_eq!(BaiduErrorCode::from_code(3301), BaiduErrorCode::AuthenticationError);
        assert_eq!(BaiduErrorCode::from_code(3302), BaiduErrorCode::TokenInvalid);
        assert_eq!(BaiduErrorCode::from_code(9999), BaiduErrorCode::Unknown);
    }

    #[test]
    fn test_error_code_retryable() {
        assert!(BaiduErrorCode::ServerBusy.is_retryable());
        assert!(BaiduErrorCode::RecognitionTimeout.is_retryable());
        assert!(BaiduErrorCode::QpsExceeded.is_retryable());
        assert!(!BaiduErrorCode::AuthenticationError.is_retryable());
        assert!(!BaiduErrorCode::TokenInvalid.is_retryable());
    }

    #[test]
    fn test_config_validation_valid() {
        let config = BaiduSttConfig::new("api_key", "secret_key");
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validation_empty_api_key() {
        let config = BaiduSttConfig::new("", "secret_key");
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_8k_english() {
        let config = BaiduSttConfig::new("api_key", "secret_key")
            .with_model(BaiduSttModel::English)
            .with_sample_rate(BaiduSampleRate::Rate8000);
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_oauth_url() {
        let config = BaiduSttConfig::new("my_api_key", "my_secret_key");
        let url = config.get_oauth_url();
        assert!(url.contains("grant_type=client_credentials"));
        assert!(url.contains("client_id=my_api_key"));
        assert!(url.contains("client_secret=my_secret_key"));
    }

    #[test]
    fn test_realtime_url() {
        let config = BaiduSttConfig::default();
        let url = config.get_realtime_url("test-session-123");
        assert!(url.contains("wss://vop.baidu.com/realtime_asr"));
        assert!(url.contains("sn=test-session-123"));
    }

    #[test]
    fn test_from_base_config() {
        let base = STTConfig {
            api_key: "my_api_key|my_secret_key".to_string(),
            language: "zh".to_string(),
            sample_rate: 16000,
            encoding: "pcm".to_string(),
            model: "mandarin".to_string(),
            ..Default::default()
        };

        let config = BaiduSttConfig::from_base(base).unwrap();
        assert_eq!(config.api_key, "my_api_key");
        assert_eq!(config.secret_key, "my_secret_key");
        assert_eq!(config.model, BaiduSttModel::Mandarin);
    }

    #[tokio::test]
    async fn test_disconnect_when_not_connected() {
        let config = create_test_config();
        let mut stt = BaiduStt::new(config).unwrap();

        let result = stt.disconnect().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_send_audio_not_connected() {
        let config = create_test_config();
        let mut stt = BaiduStt::new(config).unwrap();

        let result = stt.send_audio(bytes::Bytes::from_static(&[0u8; 1024])).await;
        assert!(result.is_err());
    }
}
