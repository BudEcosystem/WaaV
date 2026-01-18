//! Yandex SpeechKit STT Module
//!
//! This module provides integration with Yandex Cloud's SpeechKit speech-to-text API.
//! SpeechKit offers neural network-based speech recognition with support for multiple
//! languages and dialects, with focus on Russia and CIS region.
//!
//! # Features
//!
//! - REST API v1 for synchronous recognition (up to 30 seconds)
//! - Multiple languages support (Russian, English, German, etc.)
//! - Automatic language detection
//! - Profanity filtering
//! - Custom vocabulary hints
//! - IAM token or API key authentication
//!
//! # Example
//!
//! ```rust,ignore
//! use waav_gateway::core::stt::yandex::YandexSTT;
//! use waav_gateway::core::stt::base::{BaseSTT, STTConfig};
//!
//! let config = STTConfig {
//!     api_key: "your-api-key".to_string(),
//!     language: "ru-RU".to_string(),
//!     sample_rate: 16000,
//!     encoding: "lpcm".to_string(),
//!     ..Default::default()
//! };
//!
//! let mut stt = YandexSTT::new(config)?;
//! stt.connect().await?;
//! stt.send_audio(audio_bytes).await?;
//! ```
//!
//! # Supported Languages
//!
//! | Language | Code | Description |
//! |----------|------|-------------|
//! | Russian | ru-RU | Default language |
//! | English | en-US | American English |
//! | German | de-DE | German |
//! | French | fr-FR | French |
//! | Finnish | fi-FI | Finnish |
//! | Swedish | sv-SE | Swedish |
//! | Dutch | nl-NL | Dutch |
//! | Polish | pl-PL | Polish |
//! | Portuguese | pt-PT | Portuguese |
//! | Turkish | tr-TR | Turkish |
//! | Ukrainian | uk-UA | Ukrainian |
//! | Kazakh | kk-KZ | Kazakh |
//! | Uzbek | uz-UZ | Uzbek |
//! | Hebrew | he-IL | Hebrew |
//! | Auto | auto | Automatic detection |
//!
//! # Audio Formats
//!
//! - LPCM (16-bit signed, little-endian)
//! - OGG with Opus codec
//! - MP3
//!
//! # Limitations
//!
//! - Synchronous API: max 30 seconds, max 1 MB audio
//! - Rate limits apply (contact Yandex for commercial access)

pub mod client;
pub mod config;
pub mod messages;

// Re-export main types
pub use client::{YandexSTT, YANDEX_STT_RECOGNIZE_URL};
pub use config::{YandexSTTAudioFormat, YandexSTTConfig, YandexSTTLanguage, YandexSTTModel};
pub use messages::{
    RecognitionAlternative, StreamingRecognitionResult, YandexSTTApiError, YandexSTTStatusCode,
    YandexSyncResponse,
};

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::stt::base::{BaseSTT, STTConfig};

    fn create_test_config() -> STTConfig {
        STTConfig {
            provider: "yandex".to_string(),
            api_key: "test-api-key".to_string(),
            language: "ru-RU".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "lpcm".to_string(),
            model: "general".to_string(),
        }
    }

    #[test]
    fn test_yandex_stt_module_exports() {
        // Verify all public types are accessible
        let _: YandexSTTAudioFormat = YandexSTTAudioFormat::default();
        let _: YandexSTTLanguage = YandexSTTLanguage::default();
        let _: YandexSTTModel = YandexSTTModel::default();
    }

    #[test]
    fn test_yandex_stt_language_codes() {
        assert_eq!(YandexSTTLanguage::Russian.as_code(), "ru-RU");
        assert_eq!(YandexSTTLanguage::English.as_code(), "en-US");
        assert_eq!(YandexSTTLanguage::German.as_code(), "de-DE");
        assert_eq!(YandexSTTLanguage::Kazakh.as_code(), "kk-KZ");
        assert_eq!(YandexSTTLanguage::Auto.as_code(), "auto");
    }

    #[test]
    fn test_yandex_stt_audio_formats() {
        assert_eq!(YandexSTTAudioFormat::Lpcm.as_api_str(), "lpcm");
        assert_eq!(YandexSTTAudioFormat::OggOpus.as_api_str(), "oggopus");
        assert_eq!(YandexSTTAudioFormat::Mp3.as_api_str(), "mp3");
    }

    #[test]
    fn test_yandex_stt_creation_via_base_trait() {
        let config = create_test_config();
        let result = YandexSTT::new(config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_yandex_stt_recognize_url() {
        assert!(YANDEX_STT_RECOGNIZE_URL.contains("stt.api.cloud.yandex.net"));
        assert!(YANDEX_STT_RECOGNIZE_URL.contains("recognize"));
    }
}
