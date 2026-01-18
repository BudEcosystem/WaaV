//! Smallest.ai Waves TTS Provider Implementation
//!
//! This module provides integration with Smallest.ai's Waves Text-to-Speech API,
//! offering ultra-low latency synthesis with multiple models.
//!
//! # Features
//!
//! - **REST API**: HTTP POST for audio synthesis (Lightning model)
//! - **WebSocket API**: Real-time streaming TTS (Lightning-V2 model)
//! - **Voice Cloning**: Custom voice creation (Lightning-Large model)
//! - **Multiple Models**: Lightning (~100ms), Lightning-Large (~300ms), Lightning-V2 (<200ms)
//! - **Multi-language**: 16+ languages supported
//!
//! # Example
//!
//! ```rust,ignore
//! use waav_gateway::core::tts::smallest::SmallestTts;
//! use waav_gateway::core::tts::{TTSConfig, BaseTTS};
//!
//! let config = TTSConfig {
//!     api_key: "your-api-key".to_string(),
//!     voice_id: Some("emily".to_string()),
//!     model: "lightning".to_string(),
//!     audio_format: Some("wav".to_string()),
//!     sample_rate: Some(24000),
//!     ..Default::default()
//! };
//!
//! let mut tts = SmallestTts::new(config)?;
//! tts.connect().await?;
//! tts.speak("Hello, world!", true).await?;
//! ```
//!
//! # API Reference
//!
//! - REST Endpoint: `POST https://waves-api.smallest.ai/api/v1/lightning/get_speech`
//! - WebSocket: `wss://waves-api.smallest.ai/api/v1/lightning-v2/get_speech/stream`
//!
//! # Authentication
//!
//! Bearer token authentication: `Authorization: Bearer <API_KEY>`

pub mod config;
pub mod messages;
pub mod provider;

pub use config::{SmallestLanguage, SmallestModel, SmallestOutputFormat, SmallestTtsConfig};
pub use messages::{
    SmallestAddVoiceData, SmallestAddVoiceResponse, SmallestTtsRequest, SmallestTtsResponse,
    SmallestVoice, SmallestVoiceTags, SmallestVoicesResponse, SmallestWsAudioData,
    SmallestWsChunkResponse, SmallestWsErrorResponse, SmallestWsRequest,
};
pub use provider::{SmallestRequestBuilder, SmallestTts};

// =============================================================================
// API Constants
// =============================================================================

/// Base URL for Smallest.ai Waves API.
pub const SMALLEST_API_BASE_URL: &str = "https://waves-api.smallest.ai";

/// REST endpoint for Lightning TTS synthesis.
pub const SMALLEST_TTS_URL: &str = "https://waves-api.smallest.ai/api/v1/lightning/get_speech";

/// WebSocket endpoint for Lightning-V2 streaming TTS.
pub const SMALLEST_TTS_WS_URL: &str =
    "wss://waves-api.smallest.ai/api/v1/lightning-v2/get_speech/stream";

/// Get voices endpoint template (model is inserted).
pub const SMALLEST_VOICES_URL_TEMPLATE: &str =
    "https://waves-api.smallest.ai/api/v1/{model}/get_voices";

/// Voice cloning endpoint (Lightning-Large only).
pub const SMALLEST_ADD_VOICE_URL: &str =
    "https://waves-api.smallest.ai/api/v1/lightning-large/add_voice";

// =============================================================================
// Limits and Defaults
// =============================================================================

/// Maximum characters per TTS request.
pub const MAX_TEXT_LENGTH: usize = 10000;

/// Default voice ID.
pub const DEFAULT_VOICE_ID: &str = "emily";

/// Default model.
pub const DEFAULT_MODEL: SmallestModel = SmallestModel::Lightning;

/// Default audio format.
pub const DEFAULT_FORMAT: SmallestOutputFormat = SmallestOutputFormat::Wav;

/// Default sample rate in Hz.
pub const DEFAULT_SAMPLE_RATE: u32 = 24000;

/// Minimum speed value.
pub const MIN_SPEED: f32 = 0.5;

/// Maximum speed value (REST API limit).
pub const MAX_SPEED_REST: f32 = 2.0;

/// Maximum speed value (WebSocket API limit).
pub const MAX_SPEED_WS: f32 = 5.0;

/// Default speed value.
pub const DEFAULT_SPEED: f32 = 1.0;

/// Minimum consistency value.
pub const MIN_CONSISTENCY: f32 = 0.0;

/// Maximum consistency value.
pub const MAX_CONSISTENCY: f32 = 1.0;

/// Default consistency value.
pub const DEFAULT_CONSISTENCY: f32 = 0.5;

/// Minimum similarity value.
pub const MIN_SIMILARITY: f32 = 0.0;

/// Maximum similarity value.
pub const MAX_SIMILARITY: f32 = 1.0;

/// Default similarity value.
pub const DEFAULT_SIMILARITY: f32 = 0.0;

/// Minimum enhancement value.
pub const MIN_ENHANCEMENT: u8 = 0;

/// Maximum enhancement value.
pub const MAX_ENHANCEMENT: u8 = 2;

/// Default enhancement value.
pub const DEFAULT_ENHANCEMENT: u8 = 1;

/// Default WebSocket timeout in seconds.
pub const DEFAULT_WS_TIMEOUT: u32 = 20;

/// Maximum WebSocket timeout in seconds.
pub const MAX_WS_TIMEOUT: u32 = 60;

// =============================================================================
// Supported Sample Rates
// =============================================================================

/// Supported sample rates for Smallest.ai TTS.
pub const SUPPORTED_SAMPLE_RATES: [u32; 4] = [8000, 16000, 22050, 24000];

/// Check if a sample rate is supported.
#[inline]
pub fn is_valid_sample_rate(rate: u32) -> bool {
    SUPPORTED_SAMPLE_RATES.contains(&rate)
}

/// Get the closest supported sample rate.
pub fn nearest_sample_rate(rate: u32) -> u32 {
    SUPPORTED_SAMPLE_RATES
        .iter()
        .min_by_key(|&&r| (r as i32 - rate as i32).abs())
        .copied()
        .unwrap_or(DEFAULT_SAMPLE_RATE)
}

/// Build the voices URL for a specific model.
pub fn voices_url(model: &SmallestModel) -> String {
    SMALLEST_VOICES_URL_TEMPLATE.replace("{model}", model.as_str())
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_valid_sample_rate() {
        assert!(is_valid_sample_rate(8000));
        assert!(is_valid_sample_rate(16000));
        assert!(is_valid_sample_rate(22050));
        assert!(is_valid_sample_rate(24000));
        assert!(!is_valid_sample_rate(44100));
        assert!(!is_valid_sample_rate(48000));
    }

    #[test]
    fn test_nearest_sample_rate() {
        assert_eq!(nearest_sample_rate(8000), 8000);
        assert_eq!(nearest_sample_rate(10000), 8000);
        assert_eq!(nearest_sample_rate(15000), 16000);
        assert_eq!(nearest_sample_rate(20000), 22050);
        assert_eq!(nearest_sample_rate(23000), 22050);
        assert_eq!(nearest_sample_rate(25000), 24000);
        assert_eq!(nearest_sample_rate(44100), 24000);
    }

    #[test]
    fn test_voices_url() {
        assert_eq!(
            voices_url(&SmallestModel::Lightning),
            "https://waves-api.smallest.ai/api/v1/lightning/get_voices"
        );
        assert_eq!(
            voices_url(&SmallestModel::LightningLarge),
            "https://waves-api.smallest.ai/api/v1/lightning-large/get_voices"
        );
        assert_eq!(
            voices_url(&SmallestModel::LightningV2),
            "https://waves-api.smallest.ai/api/v1/lightning-v2/get_voices"
        );
    }
}
