//! Tinkoff VoiceKit Speech-to-Text Provider
//!
//! This module provides integration with Tinkoff VoiceKit's gRPC-based STT service,
//! optimized for Russian language with high accuracy and low latency streaming.
//!
//! ## Features
//!
//! - Russian language support with high accuracy
//! - Real-time bidirectional streaming
//! - Interim (partial) results
//! - Configurable VAD (Voice Activity Detection)
//! - Multiple audio encodings (LINEAR16, OPUS, MULAW, ALAW)
//!
//! ## Authentication
//!
//! Tinkoff requires API key and secret key credentials:
//! - API Key (`TINKOFF_API_KEY`)
//! - Secret Key (`TINKOFF_SECRET_KEY`)
//!
//! ## Usage
//!
//! ```ignore
//! use waav_gateway::core::stt::{create_stt_provider, STTConfig};
//!
//! let config = STTConfig {
//!     provider: "tinkoff".to_string(),
//!     language: "ru-RU".to_string(),
//!     sample_rate: 16000,
//!     ..Default::default()
//! };
//!
//! let mut stt = create_stt_provider("tinkoff", config)?;
//! stt.connect().await?;
//! stt.send_audio(audio_bytes).await?;
//! ```
//!
//! ## gRPC Endpoint
//!
//! - Endpoint: `api.tinkoff.ai:443`
//! - Service: `tinkoff.cloud.stt.v1.SpeechToText`
//! - Method: `StreamingRecognize`

mod config;
mod grpc;
mod messages;
mod provider;

pub use config::{TinkoffAudioEncoding, TinkoffSttConfig, VadConfig};
pub use grpc::{TinkoffGrpcError, create_tinkoff_channel, create_tinkoff_metadata};
pub use messages::{
    RecognitionConfig, SpeechRecognitionAlternative, SpeechRecognitionResult,
    StreamingRecognitionConfig, StreamingRecognizeRequest, StreamingRecognizeResponse,
};
pub use provider::TinkoffStt;

/// Tinkoff VoiceKit gRPC endpoint
pub const TINKOFF_GRPC_ENDPOINT: &str = "https://api.tinkoff.ai:443";

/// gRPC service path for StreamingRecognize
pub const GRPC_SERVICE_PATH: &str = "/tinkoff.cloud.stt.v1.SpeechToText/StreamingRecognize";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::stt::base::{BaseSTT, STTConfig};

    fn create_test_config() -> STTConfig {
        STTConfig {
            provider: "tinkoff".to_string(),
            api_key: String::new(),
            language: "ru-RU".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
            model: "default".to_string(),
        }
    }

    #[test]
    fn test_tinkoff_stt_module_exports() {
        // Verify all public types are accessible
        let _: TinkoffAudioEncoding = TinkoffAudioEncoding::default();
    }

    #[test]
    fn test_tinkoff_audio_encodings() {
        assert_eq!(TinkoffAudioEncoding::Linear16.as_i32(), 1);
        assert_eq!(TinkoffAudioEncoding::RawOpus.as_i32(), 2);
        assert_eq!(TinkoffAudioEncoding::Mulaw.as_i32(), 5);
        assert_eq!(TinkoffAudioEncoding::Alaw.as_i32(), 6);
    }

    #[test]
    fn test_tinkoff_stt_creation_via_base_trait() {
        let config = create_test_config();
        let result = TinkoffStt::new(config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_tinkoff_config_from_base() {
        let config = create_test_config();
        let tinkoff_config = TinkoffSttConfig::from_base(config).unwrap();
        assert_eq!(tinkoff_config.base.language, "ru-RU");
        assert_eq!(tinkoff_config.encoding, TinkoffAudioEncoding::Linear16);
    }

    #[test]
    fn test_tinkoff_config_validation_missing_api_key() {
        let config = TinkoffSttConfig::default();
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("API key"));
    }
}
