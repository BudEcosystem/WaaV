//! Tencent Cloud Speech Recognition Module
//!
//! This module provides integration with Tencent Cloud's Speech-to-Text
//! (ASR) service, supporting real-time streaming recognition.
//!
//! # Features
//!
//! - WebSocket streaming ASR with 97% word recognition accuracy
//! - Support for 11 languages: Chinese (Mandarin), English, Cantonese,
//!   Japanese, Korean, Thai, Vietnamese, Indonesian, and dialects
//! - HMAC-SHA1 signature authentication
//! - VAD (Voice Activity Detection) with configurable silence threshold
//! - Word-level timestamps
//! - Custom vocabulary (hot words)
//! - Profanity and modal particle filtering
//!
//! # Audio Requirements
//!
//! - Sample rates: 8kHz or 16kHz
//! - Formats: PCM, Speex (default), SILK, MP3, OPUS, WAV, M4A, AAC
//! - Channels: Mono
//! - Bit depth: 16-bit
//! - Recommended chunk size: 40ms (1280 bytes at 16kHz)
//!
//! # Usage
//!
//! ```rust,ignore
//! use waav_gateway::core::stt::{BaseSTT, STTConfig};
//! use waav_gateway::core::stt::tencent::TencentStt;
//!
//! // API key format: secret_id|secret_key|app_id
//! let config = STTConfig {
//!     api_key: "your_secret_id|your_secret_key|your_app_id".to_string(),
//!     language: "zh".to_string(),
//!     sample_rate: 16000,
//!     encoding: "pcm".to_string(),
//!     model: "16k_zh".to_string(),
//!     ..Default::default()
//! };
//!
//! let mut stt = TencentStt::new(config)?;
//!
//! // Register callbacks
//! stt.on_result(Arc::new(|result| {
//!     Box::pin(async move {
//!         println!("Transcript: {}", result.transcript);
//!     })
//! })).await?;
//!
//! // Connect and stream audio
//! stt.connect().await?;
//! stt.send_audio(audio_chunk).await?;
//! stt.disconnect().await?;
//! ```
//!
//! # Configuration Options
//!
//! ## Engine Models
//!
//! | Model | Language | Sample Rate |
//! |-------|----------|-------------|
//! | `16k_zh` | Mandarin Chinese | 16kHz |
//! | `16k_en` | English | 16kHz |
//! | `16k_ca` | Cantonese | 16kHz |
//! | `16k_ja` | Japanese | 16kHz |
//! | `16k_ko` | Korean | 16kHz |
//! | `16k_th` | Thai | 16kHz |
//! | `16k_vi` | Vietnamese | 16kHz |
//! | `16k_id` | Indonesian | 16kHz |
//! | `8k_zh` | Mandarin Chinese | 8kHz |
//!
//! ## Audio Formats
//!
//! | Format | Value | Description |
//! |--------|-------|-------------|
//! | PCM | 1 | Raw uncompressed |
//! | Speex | 4 | Default, recommended |
//! | SILK | 6 | WeChat format |
//! | MP3 | 8 | Compressed |
//! | OPUS | 10 | Low latency |
//! | WAV | 12 | With header |
//! | M4A | 14 | AAC container |
//! | AAC | 16 | Compressed |
//!
//! # References
//!
//! - [Tencent Cloud ASR Documentation](https://www.tencentcloud.com/document/product/1118)
//! - [Real-Time ASR WebSocket API](https://www.tencentcloud.com/document/product/1118/53937)

mod client;
mod config;
mod messages;
mod signature;

// Re-export main types
pub use client::TencentStt;
pub use config::{
    TencentSttConfig,
    TencentEngineModel,
    TencentAudioFormat,
    TencentWordInfo,
    TencentNumeralMode,
    TencentFilterDirtyMode,
    TencentFilterModalMode,
    // Constants
    TENCENT_ASR_WS_URL,
    DEFAULT_ENGINE_MODEL,
    DEFAULT_VOICE_FORMAT,
    DEFAULT_SAMPLE_RATE,
    RECOMMENDED_CHUNK_DURATION_MS,
    SIGNATURE_VALIDITY_SECS,
    VAD_SILENCE_TIME_MIN,
    VAD_SILENCE_TIME_MAX,
    MAX_SPEAK_TIME_MIN,
    MAX_SPEAK_TIME_MAX,
    MAX_HOTWORD_LIST_SIZE,
};
pub use messages::{
    TencentAsrResponse,
    TencentAsrResult,
    TencentWord,
    TencentAsrErrorCode,
    TencentSliceType,
};
pub use signature::{
    TencentSignatureBuilder,
    SignatureError,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::stt::base::STTConfig;

    // =========================================================================
    // Module Integration Tests
    // =========================================================================

    #[test]
    fn test_module_exports() {
        // Verify all public types are accessible
        let _ = TencentEngineModel::Mandarin16k;
        let _ = TencentAudioFormat::Speex;
        let _ = TencentWordInfo::None;
        let _ = TencentAsrErrorCode::Success;
        let _ = TencentSliceType::Interim;
    }

    #[test]
    fn test_create_client_via_module() {
        let config = STTConfig {
            api_key: "test_id|test_key|test_app".to_string(),
            language: "zh".to_string(),
            sample_rate: 16000,
            encoding: "pcm".to_string(),
            model: "16k_zh".to_string(),
            ..Default::default()
        };

        let result = TencentStt::new(config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_config_via_module() {
        let config = TencentSttConfig::new("id", "key", "app")
            .with_engine_model(TencentEngineModel::English16k)
            .with_voice_format(TencentAudioFormat::Opus)
            .with_vad(true)
            .with_word_info(TencentWordInfo::Detailed);

        assert!(config.validate().is_ok());
        assert_eq!(config.engine_model_type, TencentEngineModel::English16k);
        assert_eq!(config.voice_format, TencentAudioFormat::Opus);
    }

    #[test]
    fn test_signature_builder_via_module() {
        let builder = TencentSignatureBuilder::new("id", "key")
            .with_engine_model("16k_zh")
            .with_voice_format(4);

        let result = builder.build();
        assert!(result.is_ok());
    }

    #[test]
    fn test_response_parsing_via_module() {
        let json = r#"{
            "code": 0,
            "message": "success",
            "voice_id": "test",
            "result": {
                "slice_type": 1,
                "index": 0,
                "start_time": 0,
                "end_time": 1000,
                "voice_text_str": "hello"
            }
        }"#;

        let response = TencentAsrResponse::from_json(json).unwrap();
        assert!(!response.is_error());
        assert_eq!(response.get_transcript(), Some("hello"));
    }

    #[test]
    fn test_constants_accessible() {
        assert_eq!(TENCENT_ASR_WS_URL, "wss://asr.cloud.tencent.com/asr/v2");
        assert_eq!(DEFAULT_ENGINE_MODEL, "16k_zh");
        assert_eq!(DEFAULT_VOICE_FORMAT, 4);
        assert_eq!(DEFAULT_SAMPLE_RATE, 16000);
        assert_eq!(RECOMMENDED_CHUNK_DURATION_MS, 40);
        assert_eq!(SIGNATURE_VALIDITY_SECS, 86400);
        assert_eq!(VAD_SILENCE_TIME_MIN, 240);
        assert_eq!(VAD_SILENCE_TIME_MAX, 2000);
    }

    // =========================================================================
    // Cross-Module Tests
    // =========================================================================

    #[test]
    fn test_engine_model_matches_config() {
        for model in TencentEngineModel::all() {
            let model_str = model.as_str();
            let parsed = TencentEngineModel::from_str(model_str);
            assert_eq!(*model, parsed);
        }
    }

    #[test]
    fn test_audio_format_matches_config() {
        for format in TencentAudioFormat::all() {
            let value = format.value();
            let parsed = TencentAudioFormat::from_value(value);
            assert_eq!(Some(*format), parsed);
        }
    }

    #[test]
    fn test_error_code_roundtrip() {
        let codes = [0, 4001, 4002, 4003, 4004, 4005, 4006, 4007, 4008, 5000, 5001, 5002];
        for code in codes {
            let error = TencentAsrErrorCode::from_code(code);
            assert_eq!(error.code(), code);
        }
    }

    #[test]
    fn test_slice_type_roundtrip() {
        for value in 0..=2 {
            let slice_type = TencentSliceType::from_value(value);
            assert_eq!(slice_type.value(), value);
        }
    }

    // =========================================================================
    // Supported Languages/Models Tests
    // =========================================================================

    #[test]
    fn test_supported_languages() {
        let languages = TencentSttConfig::supported_languages();
        assert!(languages.contains(&"zh"));
        assert!(languages.contains(&"en"));
        assert!(languages.contains(&"ja"));
        assert!(languages.contains(&"ko"));
        assert!(languages.contains(&"th"));
        assert!(languages.contains(&"vi"));
        assert!(languages.contains(&"id"));
        assert!(languages.contains(&"yue")); // Cantonese
    }

    #[test]
    fn test_supported_models() {
        let models = TencentSttConfig::supported_models();
        assert!(models.contains(&"16k_zh"));
        assert!(models.contains(&"16k_en"));
        assert!(models.contains(&"8k_zh"));
        assert!(models.contains(&"16k_ca"));
        assert!(models.contains(&"16k_ja"));
        assert!(models.contains(&"16k_ko"));
    }
}
