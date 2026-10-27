//! Smallest.ai Waves TTS Provider Implementation
//!
//! This module provides integration with Smallest.ai's Waves Text-to-Speech API,
//! offering low-latency synthesis (~200 ms to first byte) from two Lightning v3.1 voice pools.
//!
//! # Features
//!
//! - **REST API**: one HTTP POST per utterance; the pool is the body's `model`
//!   (`lightning_v3.1`, the default, or `lightning_v3.1_pro`)
//! - **Retired models**: `lightning`, `lightning-large`, `lightning-v2` and `thunder` now answer
//!   `410 MODEL_DEPRECATED`; configs naming them are served by `lightning_v3.1`
//! - **Voices**: each pool has its own catalog (`/waves/v1/<pool>/get_voices`, public)
//!
//! # Example
//!
//! ```rust,ignore
//! use waav_gateway::core::tts::smallest::SmallestTts;
//! use waav_gateway::core::tts::{TTSConfig, BaseTTS};
//!
//! let config = TTSConfig {
//!     api_key: "your-api-key".to_string(),
//!     voice_id: Some("olivia".to_string()),
//!     model: "lightning_v3.1".to_string(),
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
//! - REST Endpoint: `POST https://api.smallest.ai/waves/v1/tts`
//! - WebSocket / SSE: `wss://api.smallest.ai/waves/v1/tts/live` / `POST …/tts/live`
//! - Docs: <https://docs.smallest.ai/models/text-to-speech/sync-async>
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
pub const SMALLEST_API_BASE_URL: &str = "https://api.smallest.ai/waves/v1";

/// REST synthesis endpoint, shared by both Lightning v3.1 pools (the body's `model` picks one).
pub const SMALLEST_TTS_URL: &str = "https://api.smallest.ai/waves/v1/tts";

/// WebSocket streaming synthesis endpoint.
pub const SMALLEST_TTS_WS_URL: &str = "wss://api.smallest.ai/waves/v1/tts/live";

/// Voice list endpoint template; `{model}` is the pool's path segment.
pub const SMALLEST_VOICES_URL_TEMPLATE: &str =
    "https://api.smallest.ai/waves/v1/{model}/get_voices";

/// Instant voice cloning endpoint (standard pool). WaaV does not clone Smallest voices itself.
pub const SMALLEST_ADD_VOICE_URL: &str = "https://api.smallest.ai/waves/v1/voice-cloning";

// =============================================================================
// Limits and Defaults
// =============================================================================

/// Maximum characters per TTS request.
pub const MAX_TEXT_LENGTH: usize = 10000;

/// Default voice ID: a standard-pool (Lightning v3.1) voice. The retired models' default,
/// `emily`, is in neither v3.1 catalog.
pub const DEFAULT_VOICE_ID: &str = "olivia";

/// Default model.
pub const DEFAULT_MODEL: SmallestModel = SmallestModel::LightningV31;

/// Default audio format.
pub const DEFAULT_FORMAT: SmallestOutputFormat = SmallestOutputFormat::Wav;

/// Default sample rate in Hz.
pub const DEFAULT_SAMPLE_RATE: u32 = 24000;

/// Minimum speed value.
pub const MIN_SPEED: f32 = 0.5;

/// Maximum speed value.
pub const MAX_SPEED: f32 = 2.0;

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
pub const SUPPORTED_SAMPLE_RATES: [u32; 4] = [8000, 16000, 24000, 44100];

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
    SMALLEST_VOICES_URL_TEMPLATE.replace("{model}", model.path_segment())
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
        assert!(is_valid_sample_rate(24000));
        assert!(is_valid_sample_rate(44100));
        assert!(!is_valid_sample_rate(22050));
        assert!(!is_valid_sample_rate(48000));
    }

    #[test]
    fn test_nearest_sample_rate() {
        assert_eq!(nearest_sample_rate(8000), 8000);
        assert_eq!(nearest_sample_rate(10000), 8000);
        assert_eq!(nearest_sample_rate(15000), 16000);
        assert_eq!(nearest_sample_rate(22050), 24000);
        assert_eq!(nearest_sample_rate(25000), 24000);
        assert_eq!(nearest_sample_rate(44100), 44100);
        assert_eq!(nearest_sample_rate(48000), 44100);
    }

    #[test]
    fn test_voices_url() {
        assert_eq!(
            voices_url(&SmallestModel::LightningV31),
            "https://api.smallest.ai/waves/v1/lightning-v3.1/get_voices"
        );
        assert_eq!(
            voices_url(&SmallestModel::LightningV31Pro),
            "https://api.smallest.ai/waves/v1/lightning-v3.1-pro/get_voices"
        );
    }
}
