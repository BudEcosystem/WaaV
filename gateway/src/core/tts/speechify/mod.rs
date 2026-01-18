//! Speechify TTS Provider
//!
//! This module provides Text-to-Speech integration with Speechify API,
//! supporting voice cloning, SSML, and multiple models:
//! - **Simba English**: Standard English model, clear and natural
//! - **Simba Turbo**: Faster processing with emotion control
//! - **Simba Multilingual**: 50+ languages support
//!
//! # Example
//!
//! ```rust,ignore
//! use waav_gateway::core::tts::speechify::SpeechifyTts;
//! use waav_gateway::core::tts::{TTSConfig, BaseTTS};
//!
//! let config = TTSConfig {
//!     api_key: "your-api-key".to_string(),
//!     voice_id: Some("george".to_string()),
//!     ..Default::default()
//! };
//!
//! let mut tts = SpeechifyTts::new(config)?;
//! tts.connect().await?;
//! tts.speak("Hello, world!", true).await?;
//! ```

mod config;
mod provider;

pub use config::*;
pub use provider::*;

// =============================================================================
// API Constants
// =============================================================================

/// Speechify synchronous TTS endpoint
pub const SPEECHIFY_TTS_SYNC_URL: &str = "https://api.sws.speechify.com/v1/audio/speech";

/// Speechify streaming TTS endpoint
pub const SPEECHIFY_TTS_STREAM_URL: &str = "https://api.sws.speechify.com/v1/audio/stream";

/// Speechify voices list endpoint
pub const SPEECHIFY_VOICES_URL: &str = "https://api.sws.speechify.com/v1/voices";

/// Speechify auth token endpoint
pub const SPEECHIFY_AUTH_URL: &str = "https://api.sws.speechify.com/v1/auth/token";

/// Maximum text length per request (characters)
pub const MAX_TEXT_LENGTH: usize = 20000;

/// Default voice ID
pub const DEFAULT_VOICE_ID: &str = "george";
