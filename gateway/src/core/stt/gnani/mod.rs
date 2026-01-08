//! Gnani.ai Speech-to-Text Provider
//!
//! This module provides integration with Gnani.ai's Vachana STT service,
//! supporting 14 Indian and English language variants with high accuracy
//! for Indic languages.
//!
//! ## Features
//!
//! - 14 language support (Hindi, Kannada, Tamil, Telugu, etc.)
//! - Real-time streaming transcription
//! - Interim (partial) results
//! - mTLS authentication with certificate
//!
//! ## Authentication
//!
//! Gnani requires three credentials (provided via email after registration):
//! - Token (`GNANI_TOKEN`)
//! - Access Key (`GNANI_ACCESS_KEY`)
//! - SSL Certificate (`GNANI_CERTIFICATE_PATH` or `GNANI_CERTIFICATE_CONTENT`)
//!
//! ## Usage
//!
//! ```ignore
//! use waav_gateway::core::stt::{create_stt_provider, STTConfig};
//!
//! let config = STTConfig {
//!     provider: "gnani".to_string(),
//!     language: "hi-IN".to_string(),
//!     sample_rate: 16000,
//!     ..Default::default()
//! };
//!
//! let mut stt = create_stt_provider("gnani", config)?;
//! stt.connect().await?;
//! stt.send_audio(audio_bytes).await?;
//! ```
//!
//! ## Supported Languages
//!
//! | Language | Code | Display Name |
//! |----------|------|--------------|
//! | Kannada | kn-IN | Kannada |
//! | Hindi | hi-IN | Hindi |
//! | Tamil | ta-IN | Tamil |
//! | Telugu | te-IN | Telugu |
//! | Gujarati | gu-IN | Gujarati |
//! | Marathi | mr-IN | Marathi |
//! | Bengali | bn-IN | Bengali |
//! | Malayalam | ml-IN | Malayalam |
//! | Punjabi | pa-guru-IN | Punjabi |
//! | Urdu | ur-IN | Urdu |
//! | English (India) | en-IN | English (India) |
//! | English (UK) | en-GB | English (UK) |
//! | English (US) | en-US | English (US) |
//! | English (Singapore) | en-SG | English (Singapore) |
//!
//! ## Rate Limits
//!
//! - Free tier: 250 requests of 15 seconds audio each
//! - Maximum 5 concurrent requests per second
//! - Contact hello@gnani.ai for commercial access

mod client;
mod config;
mod grpc;
mod messages;

pub use client::GnaniSTT;
pub use config::{GnaniAudioFormat, GnaniLanguage, GnaniSTTConfig};
pub use grpc::{GnaniGrpcError, create_gnani_channel, create_gnani_metadata};
pub use messages::{
    DecodeError, SpeechChunk, StreamingError, StreamingRecognitionResponse, TranscriptChunk,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::stt::base::{BaseSTT, STTConfig};

    fn create_test_config() -> STTConfig {
        STTConfig {
            provider: "gnani".to_string(),
            api_key: String::new(),
            language: "hi-IN".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "pcm16".to_string(),
            model: "default".to_string(),
        }
    }

    #[test]
    fn test_gnani_stt_module_exports() {
        // Verify all public types are accessible
        let _: GnaniAudioFormat = GnaniAudioFormat::default();
        let _: GnaniLanguage = GnaniLanguage::default();
    }

    #[test]
    fn test_gnani_language_codes() {
        // Verify all language codes are valid
        let codes = GnaniLanguage::all_codes();
        assert_eq!(codes.len(), 14);
        assert!(codes.contains(&"hi-IN"));
        assert!(codes.contains(&"en-IN"));
        assert!(codes.contains(&"kn-IN"));
    }

    #[test]
    fn test_gnani_stt_creation_via_base_trait() {
        let config = create_test_config();
        let result = GnaniSTT::new(config);
        assert!(result.is_ok());
    }
}
