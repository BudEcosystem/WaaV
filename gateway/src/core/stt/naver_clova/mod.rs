//! NAVER CLOVA Speech Recognition (CSR) STT provider.
//!
//! This module implements STT using NAVER Cloud Platform's CLOVA Speech Recognition
//! REST API, optimized for Korean, Japanese, English, and Chinese.
//!
//! # Overview
//!
//! CLOVA Speech Recognition (CSR) is NAVER's speech recognition service with:
//! - Highest accuracy for Korean language
//! - Support for Korean, English, Japanese, and Chinese
//! - REST API for audio up to 60 seconds
//! - Simple two-key authentication
//!
//! # API Endpoint
//!
//! ```text
//! POST https://naveropenapi.apigw.ntruss.com/recog/v1/stt?lang={language_code}
//! ```
//!
//! # Authentication
//!
//! Uses two headers for authentication:
//! - `X-NCP-APIGW-API-KEY-ID`: Client ID
//! - `X-NCP-APIGW-API-KEY`: Client Secret
//!
//! Get credentials from NAVER Cloud Platform Console:
//! Console -> AI·Application Service -> AI·NAVER API -> Application
//!
//! # Language Support
//!
//! | Code | Language | Notes |
//! |------|----------|-------|
//! | Kor | Korean | Highest accuracy |
//! | Eng | English | |
//! | Jpn | Japanese | |
//! | Chn | Chinese (Simplified) | |
//!
//! # Usage
//!
//! ```rust,ignore
//! use waav_gateway::core::stt::naver_clova::NaverClovaStt;
//! use waav_gateway::core::stt::{BaseSTT, STTConfig};
//!
//! // Create config with client_id|client_secret format
//! let config = STTConfig {
//!     api_key: "your_client_id|your_client_secret".to_string(),
//!     language: "ko-KR".to_string(),  // Korean
//!     sample_rate: 16000,
//!     ..Default::default()
//! };
//!
//! // Create and use the STT client
//! let mut stt = NaverClovaStt::new(config)?;
//! stt.connect().await?;
//! stt.send_audio(audio_bytes).await?;
//! stt.disconnect().await?;  // Transcribes buffered audio
//! ```
//!
//! # Notes
//!
//! - This is a batch/REST-based STT, not real-time streaming
//! - Audio is buffered until `disconnect()` is called
//! - Maximum audio duration: 60 seconds
//! - Minimum sample rate: 16 kHz
//! - Supports PCM, WAV, and MP3 formats

mod client;
mod config;

pub use client::NaverClovaStt;
pub use config::{
    NaverClovaAudioFormat, NaverClovaErrorResponse, NaverClovaLanguage, NaverClovaSttConfig,
    NaverClovaSttResponse, DEFAULT_SAMPLE_RATE, MAX_AUDIO_DURATION_SECONDS, MIN_SAMPLE_RATE,
    NAVER_CSR_ENDPOINT,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::stt::{BaseSTT, STTConfig};

    #[test]
    fn test_module_exports() {
        // Verify all exports are accessible
        assert_eq!(
            NAVER_CSR_ENDPOINT,
            "https://naveropenapi.apigw.ntruss.com/recog/v1/stt"
        );
        assert_eq!(MAX_AUDIO_DURATION_SECONDS, 60);
        assert_eq!(MIN_SAMPLE_RATE, 16000);
        assert_eq!(DEFAULT_SAMPLE_RATE, 16000);
    }

    #[test]
    fn test_language_enum() {
        assert_eq!(NaverClovaLanguage::Korean.code(), "Kor");
        assert_eq!(NaverClovaLanguage::English.code(), "Eng");
        assert_eq!(NaverClovaLanguage::Japanese.code(), "Jpn");
        assert_eq!(NaverClovaLanguage::Chinese.code(), "Chn");
    }

    #[test]
    fn test_audio_format_enum() {
        assert_eq!(
            NaverClovaAudioFormat::Pcm.content_type(),
            "application/octet-stream"
        );
        assert_eq!(NaverClovaAudioFormat::Wav.content_type(), "audio/wav");
        assert_eq!(NaverClovaAudioFormat::Mp3.content_type(), "audio/mpeg");
    }

    #[test]
    fn test_stt_creation() {
        let config = STTConfig {
            api_key: "client_id|client_secret".to_string(),
            language: "ko-KR".to_string(),
            sample_rate: 16000,
            ..Default::default()
        };

        let stt = NaverClovaStt::new(config);
        assert!(stt.is_ok());
    }

    #[test]
    fn test_stt_invalid_api_key() {
        let config = STTConfig {
            api_key: "invalid_format".to_string(),
            ..Default::default()
        };

        let stt = NaverClovaStt::new(config);
        assert!(stt.is_err());
    }

    #[tokio::test]
    async fn test_stt_lifecycle() {
        let config = STTConfig {
            api_key: "test_id|test_secret".to_string(),
            language: "ko-KR".to_string(),
            sample_rate: 16000,
            ..Default::default()
        };

        let mut stt = NaverClovaStt::new(config).unwrap();

        // Test connect
        assert!(stt.connect().await.is_ok());
        assert!(stt.is_ready());

        // Test send audio
        let audio = bytes::Bytes::from(vec![0u8; 1000]);
        assert!(stt.send_audio(audio).await.is_ok());

        // Test disconnect
        assert!(stt.disconnect().await.is_ok());
        assert!(!stt.is_ready());
    }

    #[test]
    fn test_config_from_base() {
        let base = STTConfig {
            api_key: "client_id|client_secret".to_string(),
            language: "en-US".to_string(),
            sample_rate: 16000,
            encoding: "wav".to_string(),
            ..Default::default()
        };

        let config = NaverClovaSttConfig::from_base(base).unwrap();
        assert_eq!(config.client_id, "client_id");
        assert_eq!(config.client_secret, "client_secret");
        assert_eq!(config.language, NaverClovaLanguage::English);
        assert_eq!(config.audio_format, NaverClovaAudioFormat::Wav);
    }

    #[test]
    fn test_response_parsing() {
        let json = r#"{"text": "안녕하세요"}"#;
        let response: NaverClovaSttResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.text, "안녕하세요");
    }

    #[test]
    fn test_error_response_parsing() {
        let json = r#"{"errorCode": "401", "errorMessage": "Authentication failed"}"#;
        let error: NaverClovaErrorResponse = serde_json::from_str(json).unwrap();
        assert_eq!(error.error_code, Some("401".to_string()));
        assert_eq!(
            error.error_message,
            Some("Authentication failed".to_string())
        );
    }
}
