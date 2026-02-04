//! Tencent Cloud Text-to-Speech (腾讯云语音合成)
//!
//! This module provides integration with Tencent Cloud Speech services
//! for text-to-speech synthesis.
//!
//! # Overview
//!
//! Tencent Cloud TTS (语音合成) provides high-quality speech synthesis
//! supporting Chinese, English, and multiple Chinese dialects.
//!
//! # Features
//!
//! - **REST API**: HTTP-based TTS synthesis via TextToVoice API
//! - **Multiple Voices**: 70+ voice options including standard, premium, and emotional voices
//! - **Chinese Optimization**: Native support for Chinese text with dialect support
//! - **Speed/Volume Control**: Full control over speech characteristics
//! - **Multiple Audio Formats**: WAV, MP3, PCM output
//! - **Word-level Timestamps**: Subtitle generation for synchronization
//! - **Emotion Control**: Emotional expression for supported voices
//!
//! # Voice Categories
//!
//! | Category | Description | Voice IDs |
//! |----------|-------------|-----------|
//! | Standard | Basic voices | 0-6 |
//! | Premium | High-quality voices | 101001-101030 |
//! | Emotional | Expressive voices | 101031-101040 |
//! | Dialect | Regional voices (Cantonese, Sichuan) | 301001-301032 |
//! | Child | Children's voices | 501001-501005 |
//! | English | English voices | 601001-601004 |
//!
//! # Usage Example
//!
//! ```rust,ignore
//! use waav_gateway::core::tts::{BaseTTS, TTSConfig, AudioCallback, AudioData, TTSError};
//! use waav_gateway::core::tts::tencent::TencentTts;
//! use std::sync::Arc;
//! use std::pin::Pin;
//! use std::future::Future;
//!
//! // Create a callback to receive audio
//! struct MyCallback;
//!
//! impl AudioCallback for MyCallback {
//!     fn on_audio(&self, audio: AudioData) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
//!         Box::pin(async move {
//!             println!("Received {} bytes of audio", audio.data.len());
//!         })
//!     }
//!
//!     fn on_error(&self, error: TTSError) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
//!         Box::pin(async move {
//!             eprintln!("Error: {}", error);
//!         })
//!     }
//!
//!     fn on_complete(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
//!         Box::pin(async move {
//!             println!("Synthesis complete");
//!         })
//!     }
//! }
//!
//! async fn example() -> Result<(), TTSError> {
//!     let config = TTSConfig {
//!         // API key format: secret_id|secret_key
//!         api_key: "your_secret_id|your_secret_key".to_string(),
//!         voice_id: Some("0".to_string()),  // 亲和女声 friendly female voice
//!         audio_format: Some("wav".to_string()),
//!         sample_rate: Some(16000),
//!         speaking_rate: Some(1.0),  // Maps to speed 0.5-2.0
//!         ..Default::default()
//!     };
//!
//!     let mut tts = TencentTts::new(config)?;
//!     tts.connect().await?;
//!
//!     let callback = Arc::new(MyCallback);
//!     tts.on_audio(callback)?;
//!
//!     tts.speak("你好世界", true).await?;
//!     tts.disconnect().await?;
//!
//!     Ok(())
//! }
//! ```
//!
//! # API Key Format
//!
//! The API key must be provided in the format `secret_id|secret_key` (pipe-separated).
//! Both values can be obtained from the Tencent Cloud Console.
//!
//! # Text Limitations
//!
//! - Maximum text length: 300 characters per request
//! - Longer texts are automatically split into chunks
//! - SSML is supported for advanced pronunciation control
//!
//! # Audio Specifications
//!
//! | Format | Sample Rate | Channels |
//! |--------|-------------|----------|
//! | WAV | 16kHz/8kHz | Mono |
//! | MP3 | 16kHz/8kHz | Mono |
//! | PCM | 16kHz/8kHz | Mono |
//!
//! # Latency Considerations
//!
//! The current implementation uses HTTP REST API which may have higher latency
//! (~200-500ms per request). For lower latency requirements, consider:
//! - Pre-caching frequently used phrases
//! - Chunking text for progressive playback
//! - Using longer segments to amortize request overhead
//!
//! Note: Tencent Cloud does not provide a WebSocket streaming TTS API for
//! real-time synthesis. For ultra-low latency requirements, consider providers
//! with streaming support (e.g., ElevenLabs, Cartesia).
//!
//! # References
//!
//! - [Tencent Cloud TTS](https://cloud.tencent.com/product/tts)
//! - [TextToVoice API Documentation](https://www.tencentcloud.com/document/product/1154/48916)
//! - [Voice List](https://cloud.tencent.com/document/product/1073/92668)
//! - [Pricing](https://cloud.tencent.com/document/product/1073/34112)

pub mod config;
pub mod provider;

// Re-export primary types
pub use config::{
    // Constants
    DEFAULT_PROJECT_ID,
    DEFAULT_SPEED,
    DEFAULT_VOICE_TYPE,
    DEFAULT_VOLUME,
    MAX_SPEED,
    MAX_TEXT_LENGTH,
    MAX_VOLUME,
    MIN_SPEED,
    MIN_VOLUME,
    TENCENT_TTS_INTL_URL,
    TENCENT_TTS_URL,
    TTS_ACTION,
    TTS_VERSION,
    TencentTtsAudioFormat,
    TencentTtsConfig,
    TencentTtsErrorInfo,
    TencentTtsResponse,
    TencentTtsResponseInner,
    TencentTtsSampleRate,
    TencentTtsSubtitle,
    TencentTtsVoice,
    TencentVoiceCategory,
};
pub use provider::TencentTts;

// =============================================================================
// Module-level Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tts::base::{BaseTTS, ConnectionState, TTSConfig};

    fn create_test_config() -> TTSConfig {
        TTSConfig {
            api_key: "test_secret_id|test_secret_key".to_string(),
            voice_id: Some("0".to_string()),
            sample_rate: Some(16000),
            audio_format: Some("wav".to_string()),
            ..Default::default()
        }
    }

    // =========================================================================
    // Config Module Tests
    // =========================================================================

    #[test]
    fn test_config_module_exports() {
        // Verify all expected types are exported
        let _voice: TencentTtsVoice = TencentTtsVoice::default();
        let _format: TencentTtsAudioFormat = TencentTtsAudioFormat::default();
        let _category: TencentVoiceCategory = TencentVoiceCategory::default();
    }

    #[test]
    fn test_constants_exported() {
        assert!(!TENCENT_TTS_URL.is_empty());
        assert!(!TENCENT_TTS_INTL_URL.is_empty());
        assert_eq!(TTS_ACTION, "TextToVoice");
        assert!(MAX_TEXT_LENGTH > 0);
        assert!(MIN_SPEED > 0.0);
        assert!(MAX_SPEED > MIN_SPEED);
        assert!(MAX_VOLUME > MIN_VOLUME);
    }

    // =========================================================================
    // Provider Module Tests
    // =========================================================================

    #[test]
    fn test_provider_module_exports() {
        // Verify TencentTts is exported
        let config = create_test_config();
        let result = TencentTts::new(config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_provider_creation_with_defaults() {
        let config = TTSConfig {
            api_key: "id|secret".to_string(),
            ..Default::default()
        };

        let tts = TencentTts::new(config);
        assert!(tts.is_ok());
    }

    #[test]
    fn test_provider_creation_with_custom_voice() {
        let config = TTSConfig {
            api_key: "id|secret".to_string(),
            voice_id: Some("101001".to_string()), // Premium voice
            ..Default::default()
        };

        let tts = TencentTts::new(config);
        assert!(tts.is_ok());
    }

    #[test]
    fn test_provider_creation_with_all_formats() {
        for format in ["wav", "mp3", "pcm"] {
            let config = TTSConfig {
                api_key: "id|secret".to_string(),
                audio_format: Some(format.to_string()),
                ..Default::default()
            };

            let tts = TencentTts::new(config);
            assert!(
                tts.is_ok(),
                "Failed to create provider with format: {}",
                format
            );
        }
    }

    // =========================================================================
    // Integration Tests
    // =========================================================================

    #[test]
    fn test_config_to_provider_integration() {
        let mut config = TencentTtsConfig::default();
        config.secret_id = "test_id".to_string();
        config.secret_key = "test_secret".to_string();
        config.voice_type = TencentTtsVoice::FriendlyMale;
        config.audio_format = TencentTtsAudioFormat::Mp3;
        config.speed = 1.5;

        assert!(config.validate().is_ok());
        assert_eq!(config.voice_type.as_id(), 1);
        assert_eq!(config.audio_format.as_codec(), "mp3");
        assert_eq!(config.get_sample_rate(), 16000);
    }

    #[test]
    fn test_base_tts_config_conversion() {
        let base = TTSConfig {
            api_key: "secret_id|secret_key".to_string(),
            voice_id: Some("2".to_string()), // Mature male
            audio_format: Some("mp3".to_string()),
            speaking_rate: Some(1.5),
            ..Default::default()
        };

        let tencent_config = TencentTtsConfig::from_base(base);
        assert!(tencent_config.is_ok());

        let config = tencent_config.unwrap();
        assert_eq!(config.voice_type, TencentTtsVoice::MatureMale);
        assert_eq!(config.audio_format, TencentTtsAudioFormat::Mp3);
    }

    #[tokio::test]
    async fn test_connection_state_lifecycle() {
        let config = create_test_config();
        let tts = TencentTts::new(config).unwrap();

        // Initially disconnected
        assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);
        assert!(!tts.is_ready());
    }

    #[tokio::test]
    async fn test_disconnect_without_connect() {
        let config = create_test_config();
        let mut tts = TencentTts::new(config).unwrap();

        // Should not error when disconnecting without connecting
        let result = tts.disconnect().await;
        assert!(result.is_ok());
    }

    // =========================================================================
    // Voice Category Tests
    // =========================================================================

    #[test]
    fn test_all_voice_categories() {
        let categories = [
            TencentVoiceCategory::Standard,
            TencentVoiceCategory::Premium,
            TencentVoiceCategory::Emotional,
            TencentVoiceCategory::Dialect,
            TencentVoiceCategory::Child,
            TencentVoiceCategory::English,
            TencentVoiceCategory::Multilingual,
        ];

        for category in categories {
            assert!(!category.voice_ids().is_empty());
            assert!(!category.name().is_empty());
        }
    }

    #[test]
    fn test_voice_category_classification() {
        // Standard voices
        assert_eq!(
            TencentTtsVoice::FriendlyFemale.category(),
            TencentVoiceCategory::Standard
        );

        // Premium voices
        assert_eq!(
            TencentTtsVoice::Custom(101001).category(),
            TencentVoiceCategory::Premium
        );

        // Dialect voices
        assert_eq!(
            TencentTtsVoice::Custom(301001).category(),
            TencentVoiceCategory::Dialect
        );
    }

    // =========================================================================
    // Audio Format Tests
    // =========================================================================

    #[test]
    fn test_all_audio_formats() {
        let formats = [
            TencentTtsAudioFormat::Wav,
            TencentTtsAudioFormat::Mp3,
            TencentTtsAudioFormat::Pcm,
        ];

        for format in formats {
            assert!(!format.as_codec().is_empty());
            assert!(!format.as_format_str().is_empty());
            assert!(!format.mime_type().is_empty());
            assert!(format.sample_rate() >= 8000);
        }
    }

    // =========================================================================
    // Error Handling Tests
    // =========================================================================

    #[test]
    fn test_invalid_api_key_format() {
        let config = TTSConfig {
            api_key: "no_pipe_separator".to_string(),
            ..Default::default()
        };

        let result = TencentTts::new(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_empty_api_key() {
        let config = TTSConfig {
            api_key: "".to_string(),
            ..Default::default()
        };

        let result = TencentTts::new(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_empty_secret_key() {
        let config = TTSConfig {
            api_key: "secret_id|".to_string(), // Empty secret key
            ..Default::default()
        };

        let result = TencentTts::new(config);
        assert!(result.is_err());
    }

    // =========================================================================
    // Provider Info Tests
    // =========================================================================

    #[test]
    fn test_provider_info_complete() {
        let config = create_test_config();
        let tts = TencentTts::new(config).unwrap();
        let info = tts.get_provider_info();

        // Check required fields
        assert_eq!(info["provider"], "tencent");
        assert!(!info["name"].as_str().unwrap().is_empty());
        assert!(info["voice_id"].is_number());
        assert!(info["sample_rate"].is_number());
        assert!(info["features"].is_array());
        assert!(info["languages"].is_array());
    }

    // =========================================================================
    // Endpoint Tests
    // =========================================================================

    #[test]
    fn test_intl_endpoint_default() {
        let config = TencentTtsConfig {
            secret_id: "test".to_string(),
            secret_key: "test".to_string(),
            use_intl_endpoint: true,
            ..Default::default()
        };

        assert!(config.get_endpoint_url().contains("intl"));
    }

    #[test]
    fn test_domestic_endpoint() {
        let config = TencentTtsConfig {
            secret_id: "test".to_string(),
            secret_key: "test".to_string(),
            use_intl_endpoint: false,
            ..Default::default()
        };

        assert!(!config.get_endpoint_url().contains("intl"));
    }

    // =========================================================================
    // Supported Items Tests
    // =========================================================================

    #[test]
    fn test_supported_voices_list() {
        let voices = TencentTtsConfig::supported_voices();
        assert!(!voices.is_empty());

        // Check basic voices are present
        assert!(voices.iter().any(|(_, id, _)| *id == 0));
        assert!(voices.iter().any(|(_, id, _)| *id == 1));
    }

    #[test]
    fn test_supported_formats_list() {
        let formats = TencentTtsConfig::supported_formats();
        assert!(formats.contains(&"wav"));
        assert!(formats.contains(&"mp3"));
        assert!(formats.contains(&"pcm"));
    }
}
