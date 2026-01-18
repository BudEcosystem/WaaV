//! Yandex SpeechKit TTS Module
//!
//! This module provides integration with Yandex Cloud's SpeechKit text-to-speech API.
//! SpeechKit offers neural network-based voice synthesis with support for multiple
//! languages and emotional voices.
//!
//! # Features
//!
//! - REST API v1 for synchronous synthesis
//! - Multiple voices with emotional variations (Russian)
//! - SSML support
//! - Multiple audio formats (LPCM, OggOpus, MP3)
//! - IAM token or API key authentication
//!
//! # Example
//!
//! ```rust,ignore
//! use waav_gateway::core::tts::yandex::YandexTts;
//! use waav_gateway::core::tts::base::{BaseTTS, TTSConfig};
//!
//! let config = TTSConfig {
//!     api_key: "your-api-key".to_string(),
//!     voice_id: Some("alena".to_string()),
//!     audio_format: Some("mp3".to_string()),
//!     ..Default::default()
//! };
//!
//! let mut tts = YandexTts::new(config)?;
//! tts.connect().await?;
//! tts.speak("Привет мир!", true).await?;
//! ```

pub mod config;
pub mod messages;
pub mod provider;

// Re-export main types
pub use config::{YandexAudioFormat, YandexEmotion, YandexTtsConfig, YandexVoice};
pub use messages::{YandexApiError, YandexSynthesisResponse};
pub use provider::YandexTts;

// =============================================================================
// API Constants
// =============================================================================

/// Yandex SpeechKit TTS API base URL
pub const YANDEX_TTS_BASE_URL: &str = "https://tts.api.cloud.yandex.net";

/// TTS synthesis endpoint (v1)
pub const YANDEX_TTS_SYNTHESIZE_URL: &str =
    "https://tts.api.cloud.yandex.net/speech/v1/tts:synthesize";

/// IAM token service URL
pub const YANDEX_IAM_TOKEN_URL: &str = "https://iam.api.cloud.yandex.net/iam/v1/tokens";

// =============================================================================
// Default Configuration
// =============================================================================

/// Default voice (Alena - Russian female)
pub const DEFAULT_VOICE_ID: &str = "alena";

/// Default language
pub const DEFAULT_LANGUAGE: &str = "ru-RU";

/// Default audio format
pub const DEFAULT_AUDIO_FORMAT: &str = "mp3";

/// Default sample rate (Hz)
pub const DEFAULT_SAMPLE_RATE: u32 = 48000;

/// Minimum sample rate (Hz)
pub const MIN_SAMPLE_RATE: u32 = 8000;

/// Maximum sample rate (Hz)
pub const MAX_SAMPLE_RATE: u32 = 48000;

/// Default speech speed
pub const DEFAULT_SPEED: f32 = 1.0;

/// Minimum speech speed
pub const MIN_SPEED: f32 = 0.1;

/// Maximum speech speed
pub const MAX_SPEED: f32 = 3.0;

/// Maximum text length per request (characters)
pub const MAX_TEXT_LENGTH: usize = 5000;

/// Maximum request body size (bytes)
pub const MAX_REQUEST_BODY_SIZE: usize = 15 * 1024;
