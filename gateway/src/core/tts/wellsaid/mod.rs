//! WellSaid Labs TTS Provider
//!
//! This module provides Text-to-Speech integration with WellSaid Labs API,
//! supporting 200+ voice avatars across 20+ languages with two models:
//! - **Legacy**: All languages, natural-sounding speech
//! - **Caruso**: English only, AI Director capabilities (pitch, tempo, loudness)
//!
//! # Example
//!
//! ```rust,ignore
//! use waav_gateway::core::tts::wellsaid::WellSaidTts;
//! use waav_gateway::core::tts::{TTSConfig, BaseTTS};
//!
//! let config = TTSConfig {
//!     api_key: "your-api-key".to_string(),
//!     voice_id: Some("3".to_string()), // Alana B.
//!     ..Default::default()
//! };
//!
//! let mut tts = WellSaidTts::new(config)?;
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

/// WellSaid TTS streaming endpoint
pub const WELLSAID_TTS_STREAM_URL: &str = "https://api.wellsaidlabs.com/v1/tts/stream";

/// WellSaid avatars list endpoint
pub const WELLSAID_AVATARS_URL: &str = "https://api.wellsaidlabs.com/v1/tts/avatars";

/// Default speaker ID (Alana B. - US English, Narration)
pub const DEFAULT_SPEAKER_ID: u32 = 3;

/// Maximum text length per request (characters)
pub const MAX_TEXT_LENGTH: usize = 1000;
