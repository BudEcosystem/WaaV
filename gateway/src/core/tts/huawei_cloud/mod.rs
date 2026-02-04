//! Huawei Cloud Speech Interaction Service (SIS) TTS Module
//!
//! This module provides Text-to-Speech functionality using Huawei Cloud's
//! Speech Interaction Service (华为云语音交互服务).
//!
//! # Features
//!
//! - **Standard TTS**: HTTP REST API for batch synthesis (up to 500 characters)
//! - **Real-time TTS**: WebSocket streaming for low-latency synthesis
//! - **10+ Voices**: Standard, premium, and child voices
//! - **Multiple Formats**: WAV, MP3, PCM output
//! - **Voice Control**: Speed, pitch, and volume adjustment
//!
//! # Voices
//!
//! ## Standard Voices (普通发音人)
//! - 小琪, 小雯, 小燕, 小倩, 小婧 (Female)
//! - 小宇, 小宋 (Male)
//! - 小王, 小呆 (Child)
//! - Cameal (English Female)
//!
//! ## Premium Voices (精品发音人)
//! - 华小夏 (Sales style)
//! - 华小唯 (Customer service)
//! - 华晓刚 (News style)
//! - Note: Premium voices only available in cn-north-4 and cn-east-3 regions
//!
//! # Usage
//!
//! ```rust,ignore
//! use waav_gateway::core::tts::{BaseTTS, TTSConfig};
//! use waav_gateway::core::tts::huawei_cloud::HuaweiCloudTts;
//!
//! // API key format: username|password|domain_name|project_id
//! let config = TTSConfig {
//!     api_key: "user|pass|domain|project123".to_string(),
//!     voice_id: Some("xiaoyan".to_string()),
//!     sample_rate: Some(16000),
//!     audio_format: Some("wav".to_string()),
//!     ..Default::default()
//! };
//!
//! let mut tts = HuaweiCloudTts::new(config)?;
//! tts.connect().await?;
//! tts.speak("你好世界", true).await?;
//! tts.disconnect().await?;
//! ```
//!
//! # Regional Endpoints
//!
//! | Region | Code | Endpoint |
//! |--------|------|----------|
//! | CN North-Beijing4 | cn-north-4 | sis.cn-north-4.myhuaweicloud.com |
//! | CN East-Shanghai2 | cn-east-3 | sis.cn-east-3.myhuaweicloud.com |
//! | AP-Singapore | ap-southeast-3 | sis-ext.ap-southeast-3.myhuaweicloud.com |
//! | CN-Hong Kong | ap-southeast-1 | sis-ext.ap-southeast-1.myhuaweicloud.com |
//! | AP-Bangkok | ap-southeast-2 | sis-ext.ap-southeast-2.myhuaweicloud.com |
//!
//! # Limitations
//!
//! - Maximum text length: 500 characters per request
//! - Premium voices only in China regions (cn-north-4, cn-east-3)
//! - Premium voices do not support pitch adjustment

pub mod config;
pub mod provider;

// Re-export main types
pub use config::{
    HuaweiCloudRegion, HuaweiCloudTtsConfig, HuaweiTtsAudioFormat, HuaweiTtsMode, HuaweiTtsVoice,
    MAX_TEXT_LENGTH,
};
pub use provider::HuaweiCloudTts;

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tts::base::{BaseTTS, ConnectionState, TTSConfig};

    fn create_test_config() -> TTSConfig {
        TTSConfig {
            api_key: "test_user|test_pass|test_domain|test_project".to_string(),
            voice_id: Some("xiaoyan".to_string()),
            sample_rate: Some(16000),
            audio_format: Some("wav".to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn test_module_exports() {
        // Verify all expected types are exported
        let _region: HuaweiCloudRegion = HuaweiCloudRegion::CnNorth4;
        let _voice: HuaweiTtsVoice = HuaweiTtsVoice::XiaoYan;
        let _format: HuaweiTtsAudioFormat = HuaweiTtsAudioFormat::Wav;
        let _mode: HuaweiTtsMode = HuaweiTtsMode::Standard;
        assert_eq!(MAX_TEXT_LENGTH, 500);
    }

    #[test]
    fn test_provider_creation() {
        let config = create_test_config();
        let result = HuaweiCloudTts::new(config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_provider_info() {
        let config = create_test_config();
        let tts = HuaweiCloudTts::new(config).unwrap();
        let info = tts.get_provider_info();
        assert_eq!(info["provider"], "huawei-cloud");
    }

    #[test]
    fn test_connection_state() {
        let config = create_test_config();
        let tts = HuaweiCloudTts::new(config).unwrap();
        assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);
        assert!(!tts.is_ready());
    }

    #[tokio::test]
    async fn test_disconnect_when_not_connected() {
        let config = create_test_config();
        let mut tts = HuaweiCloudTts::new(config).unwrap();
        assert!(tts.disconnect().await.is_ok());
    }

    #[test]
    fn test_region_enumeration() {
        assert_eq!(HuaweiCloudRegion::CnNorth4.code(), "cn-north-4");
        assert_eq!(HuaweiCloudRegion::CnEast3.code(), "cn-east-3");
        assert!(HuaweiCloudRegion::CnNorth4.supports_premium_voices());
        assert!(!HuaweiCloudRegion::ApSoutheast3.supports_premium_voices());
    }

    #[test]
    fn test_voice_enumeration() {
        assert_eq!(
            HuaweiTtsVoice::XiaoYan.as_property(),
            "chinese_xiaoyan_common"
        );
        assert_eq!(
            HuaweiTtsVoice::XiaoYu.as_property(),
            "chinese_xiaoyu_common"
        );
        assert!(!HuaweiTtsVoice::XiaoYan.is_premium());
        assert!(HuaweiTtsVoice::HuaXiaoXia.is_premium());
    }

    #[test]
    fn test_audio_format_enumeration() {
        assert_eq!(HuaweiTtsAudioFormat::Wav.as_str(), "wav");
        assert_eq!(HuaweiTtsAudioFormat::Mp3.as_str(), "mp3");
        assert_eq!(HuaweiTtsAudioFormat::Pcm.as_str(), "pcm");
    }

    #[test]
    fn test_voice_parsing() {
        assert_eq!(HuaweiTtsVoice::from_str("xiaoyan"), HuaweiTtsVoice::XiaoYan);
        assert_eq!(HuaweiTtsVoice::from_str("小燕"), HuaweiTtsVoice::XiaoYan);
        assert_eq!(HuaweiTtsVoice::from_str("xiaoyu"), HuaweiTtsVoice::XiaoYu);
    }

    #[test]
    fn test_config_validation_valid() {
        let config = HuaweiCloudTtsConfig {
            username: "user".to_string(),
            password: "pass".to_string(),
            domain_name: "domain".to_string(),
            project_id: "project".to_string(),
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validation_empty_credentials() {
        let config = HuaweiCloudTtsConfig::default();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_urls() {
        let config = HuaweiCloudTtsConfig {
            username: "user".to_string(),
            password: "pass".to_string(),
            domain_name: "domain".to_string(),
            project_id: "abc123".to_string(),
            region: HuaweiCloudRegion::CnNorth4,
            ..Default::default()
        };

        assert!(config.get_tts_url().contains("cn-north-4"));
        assert!(config.get_tts_url().contains("abc123"));
        assert!(config.get_rtts_url().starts_with("wss://"));
    }

    #[test]
    fn test_supported_voices() {
        let voices = HuaweiCloudTtsConfig::supported_voices();
        assert!(!voices.is_empty());
        assert!(
            voices
                .iter()
                .any(|(_, prop, _)| *prop == "chinese_xiaoyan_common")
        );
    }

    #[test]
    fn test_supported_formats() {
        let formats = HuaweiCloudTtsConfig::supported_formats();
        assert!(formats.contains(&"wav"));
        assert!(formats.contains(&"mp3"));
        assert!(formats.contains(&"pcm"));
    }
}
