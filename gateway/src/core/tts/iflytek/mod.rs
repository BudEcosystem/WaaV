//! iFlytek TTS (Text-to-Speech) Provider
//!
//! This module provides integration with iFlytek's WebSocket-based TTS API.
//!
//! # Overview
//!
//! iFlytek (科大讯飞) is a leading Chinese AI company specializing in voice technologies.
//! Their TTS service provides high-quality speech synthesis with support for:
//!
//! - 15+ languages including Chinese, English, Japanese, Hindi
//! - Multiple voices with different genders and speaking styles
//! - Adjustable speed, volume, and pitch
//! - Multiple audio encoding formats (PCM, MP3, Speex)
//!
//! # Authentication
//!
//! iFlytek uses HMAC-SHA256 signature-based authentication. The API key format is:
//! ```text
//! app_id|api_key|api_secret
//! ```
//!
//! # Usage
//!
//! ```rust,ignore
//! use waav_gateway::core::tts::{BaseTTS, TTSConfig};
//! use waav_gateway::core::tts::iflytek::IFlytekTts;
//!
//! let config = TTSConfig {
//!     api_key: "app_id|api_key|api_secret".to_string(),
//!     voice_id: Some("xiaoyan".to_string()),
//!     sample_rate: Some(16000),
//!     audio_format: Some("raw".to_string()),
//!     ..Default::default()
//! };
//!
//! let mut tts = IFlytekTts::new(config)?;
//! tts.connect().await?;
//!
//! // Register audio callback
//! tts.on_audio(callback)?;
//!
//! // Synthesize text
//! tts.speak("你好世界", true).await?;
//!
//! tts.disconnect().await?;
//! ```
//!
//! # Supported Voices
//!
//! | Voice Code | Language | Description |
//! |------------|----------|-------------|
//! | xiaoyan | Chinese | Standard female |
//! | aisjiuxu | Chinese | Young male |
//! | aisxping | Chinese | Female broadcaster |
//! | aisjinger | Chinese | Sweet female |
//! | aisbabyxu | Chinese | Child voice |
//! | john_ce | English | American male |
//! | catherine | English | American female |
//! | luna | Japanese | Japanese female |
//! | anjali | Hindi | Hindi female |
//!
//! # Audio Formats
//!
//! - `raw`: PCM 16-bit little-endian
//! - `lame`: MP3 encoding
//! - `speex`: Speex narrowband (8kHz)
//! - `speex-wb`: Speex wideband (16kHz)

pub mod config;
pub mod messages;
pub mod provider;

// Re-export main types
pub use config::{
    DEFAULT_PITCH, DEFAULT_SPEED, DEFAULT_TTS_SAMPLE_RATE, DEFAULT_VOLUME, IFLYTEK_TTS_ENDPOINT,
    IFLYTEK_TTS_HOST, IFLYTEK_TTS_PATH, IFlytekTextEncoding, IFlytekTtsConfig, IFlytekTtsEncoding,
    IFlytekVoice,
};
pub use messages::{
    IFlytekErrorCode, TtsRequest, TtsRequestBusiness, TtsRequestCommon, TtsRequestData,
    TtsResponse, TtsResponseData,
};
pub use provider::IFlytekTts;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tts::base::{
        AudioCallback, AudioData, BaseTTS, ConnectionState, TTSConfig, TTSError,
    };
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn create_test_api_key() -> String {
        "test_app_id|test_api_key_xxxxx|test_api_secret_xx".to_string()
    }

    fn create_test_config() -> TTSConfig {
        TTSConfig {
            api_key: create_test_api_key(),
            voice_id: Some("xiaoyan".to_string()),
            sample_rate: Some(16000),
            audio_format: Some("raw".to_string()),
            ..Default::default()
        }
    }

    // Integration tests for the module
    #[test]
    fn test_module_exports() {
        // Test config exports
        let _endpoint = IFLYTEK_TTS_ENDPOINT;
        let _host = IFLYTEK_TTS_HOST;
        let _path = IFLYTEK_TTS_PATH;
        let _sample_rate = DEFAULT_TTS_SAMPLE_RATE;
        let _speed = DEFAULT_SPEED;
        let _volume = DEFAULT_VOLUME;
        let _pitch = DEFAULT_PITCH;

        // Test enum exports
        let _voice = IFlytekVoice::Xiaoyan;
        let _encoding = IFlytekTtsEncoding::Raw;
        let _text_encoding = IFlytekTextEncoding::Utf8;

        // Test struct exports
        let _config = IFlytekTtsConfig::default();
    }

    #[test]
    fn test_tts_request_creation() {
        let request = TtsRequest::simple("test_app", "xiaoyan", "你好");
        assert!(request.to_json().is_ok());
    }

    #[test]
    fn test_tts_response_parsing() {
        let json = r#"{
            "code": 0,
            "message": "success",
            "sid": "test_sid",
            "data": {
                "audio": "SGVsbG8=",
                "status": 1
            }
        }"#;

        let response = TtsResponse::from_json(json);
        assert!(response.is_ok());

        let response = response.unwrap();
        assert!(response.is_success());
        assert!(!response.is_last());
    }

    #[test]
    fn test_provider_creation() {
        let config = create_test_config();
        let result = IFlytekTts::new(config);
        assert!(result.is_ok());

        let tts = result.unwrap();
        assert!(!tts.is_ready());
        assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);
    }

    #[test]
    fn test_provider_info() {
        let config = create_test_config();
        let tts = IFlytekTts::new(config).unwrap();

        let info = tts.get_provider_info();
        assert!(info["provider"].as_str().unwrap().contains("iFlytek"));
        assert!(info["available_voices"].is_array());
    }

    #[test]
    fn test_available_voices() {
        let voices = IFlytekTtsConfig::available_voices();
        assert!(voices.len() >= 9);
        assert!(voices.contains(&"xiaoyan"));
        assert!(voices.contains(&"john_ce"));
        assert!(voices.contains(&"luna"));
    }

    #[tokio::test]
    async fn test_speak_empty_text() {
        let config = create_test_config();
        let mut tts = IFlytekTts::new(config).unwrap();

        // Empty text should succeed without doing anything
        let result = tts.speak("", false).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_flush_no_op() {
        let config = create_test_config();
        let tts = IFlytekTts::new(config).unwrap();

        // Flush is a no-op
        let result = tts.flush().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_disconnect_not_connected() {
        let config = create_test_config();
        let mut tts = IFlytekTts::new(config).unwrap();

        // Disconnect when not connected should succeed
        let result = tts.disconnect().await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_callback_registration() {
        struct TestCallback {
            audio_received: AtomicBool,
            complete_called: AtomicBool,
        }

        impl AudioCallback for TestCallback {
            fn on_audio(
                &self,
                _audio_data: AudioData,
            ) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
                self.audio_received.store(true, Ordering::Relaxed);
                Box::pin(async {})
            }

            fn on_error(&self, _error: TTSError) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
                Box::pin(async {})
            }

            fn on_complete(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
                self.complete_called.store(true, Ordering::Relaxed);
                Box::pin(async {})
            }
        }

        let config = create_test_config();
        let mut tts = IFlytekTts::new(config).unwrap();

        let callback = Arc::new(TestCallback {
            audio_received: AtomicBool::new(false),
            complete_called: AtomicBool::new(false),
        });

        let result = tts.on_audio(callback.clone());
        assert!(result.is_ok());

        let result = tts.remove_audio_callback();
        assert!(result.is_ok());
    }

    #[test]
    fn test_voice_parsing() {
        // Test Chinese voices
        assert_eq!(IFlytekVoice::from_str("xiaoyan"), IFlytekVoice::Xiaoyan);
        assert_eq!(IFlytekVoice::from_str("aisjiuxu"), IFlytekVoice::Aisjiuxu);

        // Test English voices
        assert_eq!(IFlytekVoice::from_str("john_ce"), IFlytekVoice::JohnCe);
        assert_eq!(IFlytekVoice::from_str("john"), IFlytekVoice::JohnCe); // Alias
        assert_eq!(IFlytekVoice::from_str("catherine"), IFlytekVoice::Catherine);

        // Test other languages
        assert_eq!(IFlytekVoice::from_str("luna"), IFlytekVoice::Luna);
        assert_eq!(IFlytekVoice::from_str("anjali"), IFlytekVoice::Anjali);

        // Test custom voice
        let custom = IFlytekVoice::from_str("custom_voice");
        assert!(matches!(custom, IFlytekVoice::Custom(_)));
    }

    #[test]
    fn test_encoding_parsing() {
        // Raw/PCM
        assert_eq!(
            IFlytekTtsEncoding::from_str("raw"),
            Some(IFlytekTtsEncoding::Raw)
        );
        assert_eq!(
            IFlytekTtsEncoding::from_str("pcm"),
            Some(IFlytekTtsEncoding::Raw)
        );
        assert_eq!(
            IFlytekTtsEncoding::from_str("linear16"),
            Some(IFlytekTtsEncoding::Raw)
        );

        // MP3
        assert_eq!(
            IFlytekTtsEncoding::from_str("lame"),
            Some(IFlytekTtsEncoding::Lame)
        );
        assert_eq!(
            IFlytekTtsEncoding::from_str("mp3"),
            Some(IFlytekTtsEncoding::Lame)
        );

        // Speex
        assert_eq!(
            IFlytekTtsEncoding::from_str("speex"),
            Some(IFlytekTtsEncoding::Speex)
        );
        assert_eq!(
            IFlytekTtsEncoding::from_str("speex-wb"),
            Some(IFlytekTtsEncoding::SpeexWb)
        );

        // Unknown
        assert_eq!(IFlytekTtsEncoding::from_str("unknown"), None);
    }

    #[test]
    fn test_config_validation() {
        // Valid config
        let mut config = IFlytekTtsConfig::from_base(create_test_config()).unwrap();
        assert!(config.validate().is_ok());

        // Invalid sample rate
        config.sample_rate = 44100;
        assert!(config.validate().is_err());

        // Reset and test invalid speed
        config.sample_rate = 16000;
        config.speed = 150;
        assert!(config.validate().is_err());

        // Reset and test invalid volume
        config.speed = 50;
        config.volume = 200;
        assert!(config.validate().is_err());

        // Reset and test invalid pitch
        config.volume = 50;
        config.pitch = 101;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_error_code_parsing() {
        assert_eq!(IFlytekErrorCode::from_code(0), IFlytekErrorCode::Success);
        assert_eq!(
            IFlytekErrorCode::from_code(10005),
            IFlytekErrorCode::AuthorizationFailure
        );
        // Unknown codes should return Unknown variant
        let unknown = IFlytekErrorCode::from_code(99999);
        assert!(matches!(unknown, IFlytekErrorCode::Unknown(_)));
    }
}
