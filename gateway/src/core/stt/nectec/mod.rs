//! NECTEC (AI for Thai) STT provider module.
//!
//! This module provides Speech-to-Text integration with Thailand's NECTEC
//! AI for Thai platform, supporting both Partii4 (legacy) and Partii5 (recommended) engines.
//!
//! # Overview
//!
//! NECTEC (National Electronics and Computer Technology Center) provides free Thai language
//! AI services through the AI for Thai platform. The STT service uses HTTP multipart file
//! upload rather than real-time streaming.
//!
//! # Audio Requirements
//!
//! - Format: WAV only
//! - Sample Rate: 16,000 Hz
//! - Bit Depth: 16-bit Linear PCM
//! - Channels: Mono (1)
//! - Max Duration: 30 seconds
//! - Max File Size: 1 MB
//!
//! # Authentication
//!
//! Uses API key authentication via `Apikey` header. Get your API key from:
//! https://aiforthai.in.th
//!
//! # Usage
//!
//! ```ignore
//! use waav_gateway::core::stt::{NectecStt, BaseSTT, STTConfig};
//!
//! let config = STTConfig {
//!     api_key: std::env::var("NECTEC_API_KEY").unwrap(),
//!     model: "partii5".to_string(), // or "partii4"
//!     sample_rate: 16000,
//!     channels: 1,
//!     ..Default::default()
//! };
//!
//! let mut stt = NectecStt::new(config)?;
//! stt.connect().await?;
//!
//! // Send audio data (buffered internally)
//! stt.send_audio(audio_bytes).await?;
//!
//! // Flush to get transcription
//! let transcript = stt.flush().await?;
//! ```

mod client;
pub mod config;

pub use client::NectecStt;
pub use config::{
    NectecSttConfig, NectecSttError, NectecSttModel, Partii4OutputFormat, Partii4OutputLevel,
    Partii4Response, Partii5Response, API_KEY_HEADER, DEFAULT_CHANNELS, DEFAULT_REQUEST_TIMEOUT,
    DEFAULT_SAMPLE_RATE, LIB_HEADER, LIB_VALUE, MAX_AUDIO_DURATION_MS, MAX_AUDIO_SIZE_BYTES,
    MIN_AUDIO_DURATION_MS, PARTII4_ENDPOINT, PARTII5_ENDPOINT,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::stt::base::{BaseSTT, STTConfig};
    use bytes::Bytes;

    #[test]
    fn test_module_exports() {
        // Test that all expected items are exported
        let _ = NectecSttModel::Partii5;
        let _ = NectecSttModel::Partii4;
        let _ = Partii4OutputLevel::Utterance;
        let _ = Partii4OutputFormat::Text;
        assert!(!PARTII5_ENDPOINT.is_empty());
        assert!(!PARTII4_ENDPOINT.is_empty());
        assert!(!API_KEY_HEADER.is_empty());
    }

    #[test]
    fn test_nectec_stt_creation() {
        let config = STTConfig {
            api_key: "test_key".to_string(),
            model: "partii5".to_string(),
            sample_rate: 16000,
            channels: 1,
            ..Default::default()
        };
        let result = NectecStt::new(config);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_nectec_stt_lifecycle() {
        let config = STTConfig {
            api_key: "test_key".to_string(),
            model: "partii5".to_string(),
            sample_rate: 16000,
            channels: 1,
            ..Default::default()
        };
        let mut stt = NectecStt::new(config).unwrap();

        // Test connect
        let connect_result = stt.connect().await;
        assert!(connect_result.is_ok());
        assert!(stt.is_ready());

        // Test send audio
        let audio = Bytes::from(vec![0u8; 320]);
        let send_result = stt.send_audio(audio).await;
        assert!(send_result.is_ok());

        // Test disconnect
        let disconnect_result = stt.disconnect().await;
        assert!(disconnect_result.is_ok());
        assert!(!stt.is_ready());
    }

    #[test]
    fn test_constants() {
        assert_eq!(DEFAULT_SAMPLE_RATE, 16000);
        assert_eq!(DEFAULT_CHANNELS, 1);
        assert_eq!(MAX_AUDIO_DURATION_MS, 30_000);
        assert_eq!(MAX_AUDIO_SIZE_BYTES, 1_048_576);
    }

    #[test]
    fn test_config_from_base() {
        let base = STTConfig {
            api_key: "my_api_key".to_string(),
            model: "partii5".to_string(),
            sample_rate: 16000,
            channels: 1,
            ..Default::default()
        };
        let nectec_config = NectecSttConfig::from_base(&base);
        assert!(nectec_config.is_ok());
        let config = nectec_config.unwrap();
        assert_eq!(config.api_key, "my_api_key");
        assert_eq!(config.model, NectecSttModel::Partii5);
    }

    #[test]
    fn test_error_types() {
        let error = NectecSttError::InvalidApiKey;
        assert!(error.to_string().contains("API key"));

        let error = NectecSttError::RateLimitExceeded;
        assert!(error.to_string().contains("rate limit"));

        let error = NectecSttError::AudioTooLong;
        assert!(error.to_string().contains("30 seconds"));
    }
}
