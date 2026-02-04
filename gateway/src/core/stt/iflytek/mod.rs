//! iFlytek (科大讯飞) Speech-to-Text Provider
//!
//! This module provides integration with iFlytek's WebSocket-based
//! Speech-to-Text API, supporting 30+ languages with industry-leading
//! accuracy for Chinese speech recognition.
//!
//! # Features
//!
//! - **Short Form ASR**: Up to 60 seconds, 30+ languages
//! - **Real-time ASR**: Up to 5 hours streaming
//! - **Dynamic Word Correction**: Progressive refinement for Chinese
//! - **Multi-dialect Support**: 23 Chinese dialects
//! - **Low Latency**: WebSocket-based streaming
//!
//! # Authentication
//!
//! iFlytek uses HMAC-SHA256 signature-based authentication with:
//! - **APPID**: Application identifier
//! - **APIKey**: 32-character key for signatures
//! - **APISecret**: 32-character secret for HMAC
//!
//! Credentials format: `app_id|api_key|api_secret`
//!
//! # Usage
//!
//! ```rust,ignore
//! use waav_gateway::core::stt::{BaseSTT, STTConfig};
//! use waav_gateway::core::stt::iflytek::IFlytekStt;
//!
//! let config = STTConfig {
//!     api_key: "your_app_id|your_api_key|your_api_secret".to_string(),
//!     language: "zh_cn".to_string(),
//!     sample_rate: 16000,
//!     ..Default::default()
//! };
//!
//! let mut stt = IFlytekStt::new(config)?;
//! stt.connect().await?;
//!
//! // Register result callback
//! let callback = Arc::new(|result: STTResult| {
//!     Box::pin(async move {
//!         println!("Transcript: {}", result.transcript);
//!     }) as Pin<Box<dyn Future<Output = ()> + Send>>
//! });
//! stt.on_result(callback).await?;
//!
//! // Send audio data
//! stt.send_audio(audio_bytes).await?;
//!
//! // Disconnect when done
//! stt.disconnect().await?;
//! ```
//!
//! # Audio Requirements
//!
//! | Parameter | Value |
//! |-----------|-------|
//! | Sample Rate | 16000 Hz (recommended) or 8000 Hz |
//! | Bit Depth | 16-bit |
//! | Channels | Mono |
//! | Encoding | PCM, MP3 (Mandarin/English), Speex |
//! | Frame Size | 1280 bytes (40ms @ 16kHz) |
//!
//! # Supported Languages
//!
//! - Chinese (Mandarin, Cantonese, 23 dialects)
//! - English, Japanese, Korean
//! - Russian, French, Spanish, German
//! - Thai, Vietnamese, Arabic
//! - Indonesian, Malay, Hindi
//! - Portuguese, Turkish, Italian
//!
//! # References
//!
//! - [iFlytek Global Platform](https://global.xfyun.cn/)
//! - [Short Form ASR API](https://global.xfyun.cn/doc/asr/voicedictation/API.html)
//! - [Real-time ASR API](https://global.xfyun.cn/doc/rtasr/rtasr/API.html)

mod auth;
mod client;
mod config;
mod messages;

// Re-export main types
pub use auth::{AuthError, IFlytekAuth, MAX_CLOCK_SKEW_SECS};
pub use client::IFlytekStt;
pub use config::{
    // Constants
    DEFAULT_FRAME_INTERVAL_MS,
    DEFAULT_FRAME_SIZE,
    DEFAULT_SAMPLE_RATE,
    DEFAULT_VAD_EOS_MS,
    IFLYTEK_IAT_ENDPOINT,
    IFLYTEK_IAT_HOST,
    IFLYTEK_IAT_PATH,
    IFLYTEK_IST_ENDPOINT,
    IFLYTEK_IST_HOST,
    IFLYTEK_IST_PATH,
    IFlytekAsrDomain,
    IFlytekAsrMode,
    IFlytekAudioEncoding,
    IFlytekLanguage,
    IFlytekSttConfig,
    MAX_REALTIME_DURATION_SECS,
    MAX_SHORT_FORM_DURATION_SECS,
};
pub use messages::{
    DataStatus, IFlytekErrorCode, SttRequest, SttRequestBusiness, SttRequestCommon, SttRequestData,
    SttResponse, SttResponseData, SttResultData, SttWord, SttWordSegment,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::stt::base::{BaseSTT, STTConfig};

    fn create_test_api_key() -> String {
        "test_app_id|test_api_key_xxxxx|test_api_secret_xx".to_string()
    }

    fn create_test_config() -> STTConfig {
        STTConfig {
            api_key: create_test_api_key(),
            language: "zh_cn".to_string(),
            sample_rate: 16000,
            encoding: "raw".to_string(),
            punctuation: true,
            ..Default::default()
        }
    }

    // Module export tests
    #[test]
    fn test_iflytek_module_exports() {
        // Verify all public types are accessible
        let _: IFlytekLanguage = IFlytekLanguage::default();
        let _: IFlytekAudioEncoding = IFlytekAudioEncoding::default();
        let _: IFlytekAsrDomain = IFlytekAsrDomain::default();
        let _: IFlytekAsrMode = IFlytekAsrMode::default();
        let _: DataStatus = DataStatus::First;
    }

    #[test]
    fn test_iflytek_constants() {
        assert_eq!(DEFAULT_SAMPLE_RATE, 16000);
        assert_eq!(DEFAULT_FRAME_SIZE, 1280);
        assert_eq!(DEFAULT_FRAME_INTERVAL_MS, 40);
        assert_eq!(DEFAULT_VAD_EOS_MS, 2000);
        assert_eq!(MAX_SHORT_FORM_DURATION_SECS, 60);
        assert_eq!(MAX_REALTIME_DURATION_SECS, 5 * 60 * 60);
        assert_eq!(MAX_CLOCK_SKEW_SECS, 300);
    }

    #[test]
    fn test_iflytek_endpoints() {
        assert!(IFLYTEK_IAT_ENDPOINT.starts_with("wss://"));
        assert!(IFLYTEK_IST_ENDPOINT.starts_with("wss://"));
        assert!(IFLYTEK_IAT_HOST.contains("iat"));
        assert!(IFLYTEK_IST_HOST.contains("ist"));
    }

    #[test]
    fn test_iflytek_stt_creation() {
        let config = create_test_config();
        let result = IFlytekStt::new(config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_iflytek_auth_from_combined() {
        let combined = create_test_api_key();
        let auth = IFlytekAuth::from_combined(&combined);
        assert!(auth.is_ok());
    }

    #[test]
    fn test_iflytek_config_from_base() {
        let config = create_test_config();
        let iflytek_config = IFlytekSttConfig::from_base(config);
        assert!(iflytek_config.is_ok());
    }

    #[test]
    fn test_iflytek_error_codes() {
        assert!(IFlytekErrorCode::Success.is_success());
        assert!(!IFlytekErrorCode::AuthorizationFailure.is_success());
        assert!(IFlytekErrorCode::ReadTimeout.is_retryable());
    }

    #[test]
    fn test_iflytek_language_codes() {
        assert_eq!(IFlytekLanguage::ChineseMandarin.as_code(), "zh_cn");
        assert_eq!(IFlytekLanguage::English.as_code(), "en_us");
        assert_eq!(IFlytekLanguage::Japanese.as_code(), "ja_jp");
    }

    #[test]
    fn test_iflytek_language_from_code() {
        assert_eq!(
            IFlytekLanguage::from_code("zh_cn"),
            Some(IFlytekLanguage::ChineseMandarin)
        );
        assert_eq!(
            IFlytekLanguage::from_code("en"),
            Some(IFlytekLanguage::English)
        );
    }

    #[test]
    fn test_iflytek_encoding() {
        assert_eq!(IFlytekAudioEncoding::Raw.as_str(), "raw");
        assert_eq!(IFlytekAudioEncoding::Mp3.as_str(), "lame");
    }

    #[test]
    fn test_iflytek_domain() {
        assert_eq!(IFlytekAsrDomain::Iat.as_str(), "iat");
        assert_eq!(IFlytekAsrDomain::Medical.as_str(), "medical");
    }

    #[test]
    fn test_iflytek_mode() {
        assert_eq!(IFlytekAsrMode::ShortForm.host(), IFLYTEK_IAT_HOST);
        assert_eq!(IFlytekAsrMode::Realtime.host(), IFLYTEK_IST_HOST);
    }

    #[test]
    fn test_request_creation() {
        let request = SttRequest::first_frame(
            "app_id",
            "zh_cn",
            "iat",
            Some("mandarin"),
            2000,
            true,
            true,
            true,
            "audio/L16;rate=16000",
            "raw",
            &[0u8; 100],
        );
        assert_eq!(request.data.status, 0);
    }

    #[test]
    fn test_response_parsing() {
        let json = r#"{
            "code": 0,
            "message": "success",
            "sid": "test_sid",
            "data": {
                "result": {
                    "ws": [{"bg": 0, "cw": [{"w": "测试"}]}],
                    "sn": 1,
                    "ls": false
                },
                "status": 1
            }
        }"#;

        let response = SttResponse::from_json(json);
        assert!(response.is_ok());
        assert_eq!(response.unwrap().transcript(), Some("测试".to_string()));
    }

    #[test]
    fn test_iflytek_stt_provider_info() {
        let config = create_test_config();
        let stt = IFlytekStt::new(config).unwrap();
        let info = stt.get_provider_info();
        assert!(info.contains("iFlytek"));
    }

    #[test]
    fn test_iflytek_stt_initial_state() {
        let config = create_test_config();
        let stt = IFlytekStt::new(config).unwrap();
        assert!(!stt.is_ready());
    }
}
