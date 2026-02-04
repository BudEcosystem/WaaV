//! NAVER CLOVA Voice (TTS Premium) provider.
//!
//! This module implements TTS using NAVER Cloud Platform's CLOVA Voice
//! REST API, offering 100+ high-quality voices.
//!
//! # Overview
//!
//! CLOVA Voice is NAVER's premium TTS service with:
//! - 100+ voices with NeuVis (Neural Voice Synthesis) technology
//! - Multiple languages (Korean, Japanese, English, Chinese, Spanish)
//! - Adjustable parameters (volume, speed, pitch, emotion)
//! - MP3 and WAV output formats
//!
//! # API Endpoint
//!
//! ```text
//! POST https://naveropenapi.apigw.ntruss.com/tts-premium/v1/tts
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
//! # Request Format
//!
//! Form-encoded body with parameters:
//! - `speaker`: Voice ID
//! - `text`: Text to synthesize (max 2000 chars)
//! - `volume`: Volume (-5 to 5)
//! - `speed`: Speed (-5 to 5)
//! - `pitch`: Pitch (-5 to 5)
//! - `format`: Output format (mp3 or wav)
//! - `emotion`: Emotion intensity (0-2)
//! - `emotion-strength`: Emotion strength (0-2)
//!
//! # Usage
//!
//! ```rust,ignore
//! use waav_gateway::core::tts::naver_clova::NaverClovaTts;
//! use waav_gateway::core::tts::{BaseTTS, TTSConfig};
//!
//! // Create config with client_id|client_secret format
//! let config = TTSConfig {
//!     api_key: "your_client_id|your_client_secret".to_string(),
//!     voice_id: Some("nara".to_string()),  // Korean female
//!     audio_format: Some("mp3".to_string()),
//!     ..Default::default()
//! };
//!
//! // Create and use the TTS client
//! let mut tts = NaverClovaTts::new(config)?;
//! tts.connect().await?;
//! tts.speak("안녕하세요!", true).await?;
//! tts.disconnect().await?;
//! ```

mod client;
mod config;

pub use client::NaverClovaTts;
pub use config::{
    DEFAULT_SAMPLE_RATE, MAX_TEXT_LENGTH, NAVER_TTS_ENDPOINT, NaverClovaTtsConfig,
    NaverClovaTtsFormat, NaverClovaVoice,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tts::{BaseTTS, TTSConfig};

    #[test]
    fn test_module_exports() {
        // Verify all exports are accessible
        assert_eq!(
            NAVER_TTS_ENDPOINT,
            "https://naveropenapi.apigw.ntruss.com/tts-premium/v1/tts"
        );
        assert_eq!(MAX_TEXT_LENGTH, 2000);
        assert_eq!(DEFAULT_SAMPLE_RATE, 24000);
    }

    #[test]
    fn test_voice_enum() {
        assert_eq!(NaverClovaVoice::Nara.speaker_id(), "nara");
        assert_eq!(NaverClovaVoice::Clara.speaker_id(), "clara");
        assert_eq!(NaverClovaVoice::Ntomoko.speaker_id(), "ntomoko");
        assert_eq!(NaverClovaVoice::Meimei.speaker_id(), "meimei");
    }

    #[test]
    fn test_format_enum() {
        assert_eq!(NaverClovaTtsFormat::Mp3.api_format(), "mp3");
        assert_eq!(NaverClovaTtsFormat::Wav.api_format(), "wav");
        assert_eq!(NaverClovaTtsFormat::Mp3.content_type(), "audio/mpeg");
        assert_eq!(NaverClovaTtsFormat::Wav.content_type(), "audio/wav");
    }

    #[test]
    fn test_tts_creation() {
        let config = TTSConfig {
            api_key: "client_id|client_secret".to_string(),
            voice_id: Some("nara".to_string()),
            ..Default::default()
        };

        let tts = NaverClovaTts::new(config);
        assert!(tts.is_ok());
    }

    #[test]
    fn test_tts_invalid_api_key() {
        let config = TTSConfig {
            api_key: "invalid_format".to_string(),
            ..Default::default()
        };

        let tts = NaverClovaTts::new(config);
        assert!(tts.is_err());
    }

    #[tokio::test]
    async fn test_tts_lifecycle() {
        let config = TTSConfig {
            api_key: "test_id|test_secret".to_string(),
            voice_id: Some("nara".to_string()),
            ..Default::default()
        };

        let mut tts = NaverClovaTts::new(config).unwrap();

        // Test connect
        assert!(tts.connect().await.is_ok());
        assert!(tts.is_ready());

        // Test disconnect
        assert!(tts.disconnect().await.is_ok());
        assert!(!tts.is_ready());
    }

    #[test]
    fn test_config_from_base() {
        let base = TTSConfig {
            api_key: "client_id|client_secret".to_string(),
            voice_id: Some("clara".to_string()),
            audio_format: Some("wav".to_string()),
            speaking_rate: Some(1.0),
            ..Default::default()
        };

        let config = NaverClovaTtsConfig::from_base(base).unwrap();
        assert_eq!(config.client_id, "client_id");
        assert_eq!(config.client_secret, "client_secret");
        assert_eq!(config.voice, NaverClovaVoice::Clara);
        assert_eq!(config.format, NaverClovaTtsFormat::Wav);
    }

    #[test]
    fn test_request_body_building() {
        let config = NaverClovaTtsConfig {
            client_id: "id".to_string(),
            client_secret: "secret".to_string(),
            voice: NaverClovaVoice::Nara,
            volume: 1,
            speed: 2,
            pitch: -1,
            format: NaverClovaTtsFormat::Mp3,
            ..Default::default()
        };

        let body = config.build_request_body("Hello");
        assert!(body.contains("speaker=nara"));
        assert!(body.contains("text=Hello"));
        assert!(body.contains("volume=1"));
        assert!(body.contains("speed=2"));
    }
}
