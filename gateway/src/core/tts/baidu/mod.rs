//! Baidu AI Cloud Text-to-Speech (百度语音合成)
//!
//! This module provides integration with Baidu AI Cloud Speech services
//! for text-to-speech synthesis.
//!
//! # Overview
//!
//! Baidu AI Cloud Speech (百度语音) is China's leading speech technology platform,
//! offering multiple voice options including basic, premium, and AI-powered voices.
//!
//! # Features
//!
//! - **REST API**: Simple HTTP-based TTS synthesis
//! - **Multiple Voices**: 40+ voice options across different categories
//! - **Chinese Optimization**: Native support for Chinese text with polyphonic handling
//! - **Speed/Pitch/Volume Control**: Full control over speech characteristics
//! - **Multiple Audio Formats**: MP3, PCM (16k/8k), WAV output
//!
//! # Voice Categories
//!
//! | Category | Description | Voice IDs |
//! |----------|-------------|-----------|
//! | Basic | Standard voices | 0, 1, 3, 4 |
//! | Premium | Higher quality | 5003, 5118, 106, etc. |
//! | Premium+ | Professional grade | 4003, 4106, 4115, etc. |
//! | Large Model | AI-powered voices | 4189, 4194, 20100, etc. |
//!
//! # Usage Example
//!
//! ```rust,ignore
//! use waav_gateway::core::tts::{BaseTTS, TTSConfig, AudioCallback, AudioData, TTSError};
//! use waav_gateway::core::tts::baidu::BaiduTts;
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
//!         // API key format: api_key|secret_key
//!         api_key: "your_api_key|your_secret_key".to_string(),
//!         voice_id: Some("0".to_string()),  // 度小美 female voice
//!         audio_format: Some("mp3".to_string()),
//!         sample_rate: Some(16000),
//!         speaking_rate: Some(1.0),  // Maps to speed 0-15
//!         ..Default::default()
//!     };
//!
//!     let mut tts = BaiduTts::new(config)?;
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
//! The API key must be provided in the format `api_key|secret_key` (pipe-separated).
//! Both values can be obtained from the Baidu AI Console.
//!
//! # Text Limitations
//!
//! - Maximum text length: 1024 GBK bytes (~512 Chinese characters)
//! - Longer texts are automatically split into chunks
//! - Special characters should be URL-encoded
//! - Polyphonic characters: Use format like "重(chong2)报集团"
//!
//! # Audio Specifications
//!
//! | Format | aue Value | Sample Rate |
//! |--------|-----------|-------------|
//! | MP3 | 3 | 16kHz |
//! | PCM | 4 | 16kHz |
//! | PCM | 5 | 8kHz |
//! | WAV | 6 | 16kHz |
//!
//! # References
//!
//! - [Baidu AI Platform](https://ai.baidu.com/)
//! - [TTS API Documentation](https://ai.baidu.com/ai-doc/SPEECH/Gk38y8lzk)
//! - [Pricing](https://cloud.baidu.com/product-price/speech.html)

pub mod config;
pub mod provider;

// Re-export primary types
pub use config::{
    BaiduOAuthError, BaiduOAuthResponse, BaiduTtsAudioFormat, BaiduTtsConfig,
    BaiduTtsErrorResponse, BaiduTtsVoice, BaiduVoiceCategory, BAIDU_OAUTH_URL, BAIDU_TTS_URL,
    BAIDU_TTS_URL_HTTPS, DEFAULT_AUDIO_FORMAT, DEFAULT_PITCH, DEFAULT_SPEED, DEFAULT_VOICE,
    DEFAULT_VOLUME, MAX_TEXT_LENGTH_GBK,
};
pub use provider::BaiduTts;

// =============================================================================
// Module-level Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tts::base::{BaseTTS, ConnectionState, TTSConfig};

    fn create_test_config() -> TTSConfig {
        TTSConfig {
            api_key: "test_api_key|test_secret_key".to_string(),
            voice_id: Some("0".to_string()),
            sample_rate: Some(16000),
            audio_format: Some("mp3".to_string()),
            ..Default::default()
        }
    }

    // =========================================================================
    // Config Module Tests
    // =========================================================================

    #[test]
    fn test_config_module_exports() {
        // Verify all expected types are exported
        let _voice: BaiduTtsVoice = BaiduTtsVoice::default();
        let _format: BaiduTtsAudioFormat = BaiduTtsAudioFormat::default();
        let _category: BaiduVoiceCategory = BaiduVoiceCategory::default();
    }

    #[test]
    fn test_constants_exported() {
        assert!(!BAIDU_TTS_URL.is_empty());
        assert!(!BAIDU_TTS_URL_HTTPS.is_empty());
        assert!(!BAIDU_OAUTH_URL.is_empty());
        assert!(MAX_TEXT_LENGTH_GBK > 0);
        assert!(DEFAULT_SPEED <= 15);
        assert!(DEFAULT_PITCH <= 15);
        assert!(DEFAULT_VOLUME <= 15);
    }

    // =========================================================================
    // Provider Module Tests
    // =========================================================================

    #[test]
    fn test_provider_module_exports() {
        // Verify BaiduTts is exported
        let config = create_test_config();
        let result = BaiduTts::new(config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_provider_creation_with_defaults() {
        let config = TTSConfig {
            api_key: "api|secret".to_string(),
            ..Default::default()
        };

        let tts = BaiduTts::new(config);
        assert!(tts.is_ok());
    }

    #[test]
    fn test_provider_creation_with_custom_voice() {
        let config = TTSConfig {
            api_key: "api|secret".to_string(),
            voice_id: Some("5003".to_string()), // Premium voice
            ..Default::default()
        };

        let tts = BaiduTts::new(config);
        assert!(tts.is_ok());
    }

    #[test]
    fn test_provider_creation_with_all_formats() {
        for format in ["mp3", "pcm", "pcm8k", "wav"] {
            let config = TTSConfig {
                api_key: "api|secret".to_string(),
                audio_format: Some(format.to_string()),
                ..Default::default()
            };

            let tts = BaiduTts::new(config);
            assert!(tts.is_ok(), "Failed to create provider with format: {}", format);
        }
    }

    // =========================================================================
    // Integration Tests
    // =========================================================================

    #[test]
    fn test_config_to_provider_integration() {
        let mut config = BaiduTtsConfig::default();
        config.api_key = "test_key".to_string();
        config.secret_key = "test_secret".to_string();
        config.voice = BaiduTtsVoice::DuXiaoyu;
        config.audio_format = BaiduTtsAudioFormat::Wav;
        config.speed = 10;

        assert!(config.validate().is_ok());
        assert_eq!(config.voice.as_id(), 1);
        assert_eq!(config.audio_format.as_aue(), 6);
        assert_eq!(config.get_sample_rate(), 16000);
    }

    #[test]
    fn test_base_tts_config_conversion() {
        let base = TTSConfig {
            api_key: "api_key|secret_key".to_string(),
            voice_id: Some("4".to_string()), // Child voice
            audio_format: Some("wav".to_string()),
            speaking_rate: Some(2.0), // Fast
            ..Default::default()
        };

        let baidu_config = BaiduTtsConfig::from_base(base);
        assert!(baidu_config.is_ok());

        let config = baidu_config.unwrap();
        assert_eq!(config.voice, BaiduTtsVoice::DuYaya);
        assert_eq!(config.audio_format, BaiduTtsAudioFormat::Wav);
    }

    #[tokio::test]
    async fn test_connection_state_lifecycle() {
        let config = create_test_config();
        let tts = BaiduTts::new(config).unwrap();

        // Initially disconnected
        assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);
        assert!(!tts.is_ready());
    }

    #[tokio::test]
    async fn test_disconnect_without_connect() {
        let config = create_test_config();
        let mut tts = BaiduTts::new(config).unwrap();

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
            BaiduVoiceCategory::Basic,
            BaiduVoiceCategory::Premium,
            BaiduVoiceCategory::PremiumPlus,
            BaiduVoiceCategory::LargeModel,
        ];

        for category in categories {
            assert!(!category.voice_ids().is_empty());
            assert!(!category.name().is_empty());
        }
    }

    #[test]
    fn test_voice_category_classification() {
        // Basic voices
        assert_eq!(
            BaiduTtsVoice::Custom(0).category(),
            BaiduVoiceCategory::Basic
        );
        assert_eq!(
            BaiduTtsVoice::Custom(1).category(),
            BaiduVoiceCategory::Basic
        );

        // Premium voices
        assert_eq!(
            BaiduTtsVoice::Custom(5003).category(),
            BaiduVoiceCategory::Premium
        );

        // Premium+ voices
        assert_eq!(
            BaiduTtsVoice::Custom(4003).category(),
            BaiduVoiceCategory::PremiumPlus
        );

        // Large model voices
        assert_eq!(
            BaiduTtsVoice::Custom(4189).category(),
            BaiduVoiceCategory::LargeModel
        );
    }

    // =========================================================================
    // Audio Format Tests
    // =========================================================================

    #[test]
    fn test_all_audio_formats() {
        let formats = [
            BaiduTtsAudioFormat::Mp3,
            BaiduTtsAudioFormat::Pcm16k,
            BaiduTtsAudioFormat::Pcm8k,
            BaiduTtsAudioFormat::Wav,
        ];

        for format in formats {
            assert!(format.as_aue() >= 3 && format.as_aue() <= 6);
            assert!(!format.as_format_str().is_empty());
            assert!(!format.mime_type().is_empty());
            assert!(format.sample_rate() >= 8000);
        }
    }

    #[test]
    fn test_audio_format_sample_rates() {
        assert_eq!(BaiduTtsAudioFormat::Mp3.sample_rate(), 16000);
        assert_eq!(BaiduTtsAudioFormat::Pcm16k.sample_rate(), 16000);
        assert_eq!(BaiduTtsAudioFormat::Pcm8k.sample_rate(), 8000);
        assert_eq!(BaiduTtsAudioFormat::Wav.sample_rate(), 16000);
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

        let result = BaiduTts::new(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_empty_api_key() {
        let config = TTSConfig {
            api_key: "".to_string(),
            ..Default::default()
        };

        let result = BaiduTts::new(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_empty_secret_key() {
        let config = TTSConfig {
            api_key: "api_key|".to_string(), // Empty secret key
            ..Default::default()
        };

        let result = BaiduTts::new(config);
        assert!(result.is_err());
    }

    // =========================================================================
    // Provider Info Tests
    // =========================================================================

    #[test]
    fn test_provider_info_complete() {
        let config = create_test_config();
        let tts = BaiduTts::new(config).unwrap();
        let info = tts.get_provider_info();

        // Check required fields
        assert_eq!(info["provider"], "baidu");
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
    fn test_https_endpoint_default() {
        let config = BaiduTtsConfig {
            api_key: "test".to_string(),
            secret_key: "test".to_string(),
            use_https: true,
            ..Default::default()
        };

        assert!(config.get_endpoint_url().starts_with("https://"));
    }

    #[test]
    fn test_http_endpoint() {
        let config = BaiduTtsConfig {
            api_key: "test".to_string(),
            secret_key: "test".to_string(),
            use_https: false,
            ..Default::default()
        };

        assert!(config.get_endpoint_url().starts_with("http://"));
        assert!(!config.get_endpoint_url().starts_with("https://"));
    }

    // =========================================================================
    // Supported Items Tests
    // =========================================================================

    #[test]
    fn test_supported_voices_list() {
        let voices = BaiduTtsConfig::supported_voices();
        assert!(!voices.is_empty());

        // Check basic voices are present
        assert!(voices.iter().any(|(_, id, _)| *id == 0));
        assert!(voices.iter().any(|(_, id, _)| *id == 1));
        assert!(voices.iter().any(|(_, id, _)| *id == 3));
        assert!(voices.iter().any(|(_, id, _)| *id == 4));
    }

    #[test]
    fn test_supported_formats_list() {
        let formats = BaiduTtsConfig::supported_formats();
        assert!(formats.contains(&"mp3"));
        assert!(formats.contains(&"wav"));
        assert!(formats.contains(&"pcm16k"));
        assert!(formats.contains(&"pcm8k"));
    }
}
