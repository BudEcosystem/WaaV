//! Unreal Speech TTS Provider
//!
//! This module provides integration with the Unreal Speech Text-to-Speech API,
//! offering cost-effective TTS with ultra-low latency (~300ms).
//!
//! # Features
//!
//! - **Ultra-low latency**: ~300ms time-to-first-audio
//! - **Multiple voices**: Standard (5) and Kokoro V8 (11) voices
//! - **Speed/Pitch control**: Adjustable speed (-1.0 to 1.0) and pitch (0.5 to 1.5)
//! - **Flexible bitrate**: 16k to 320k bitrates
//! - **Cost-effective**: Up to 90% cheaper than competitors
//!
//! # Example
//!
//! ```rust,ignore
//! use waav_gateway::core::tts::unrealspeech::UnrealSpeechTts;
//! use waav_gateway::core::tts::{TTSConfig, BaseTTS};
//!
//! let config = TTSConfig {
//!     api_key: "your-api-key".to_string(),
//!     voice_id: Some("Scarlett".to_string()),
//!     ..Default::default()
//! };
//!
//! let mut tts = UnrealSpeechTts::new(config)?;
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

/// Unreal Speech API base URL (V8)
pub const UNREALSPEECH_API_BASE_URL: &str = "https://api.v8.unrealspeech.com";

/// Stream endpoint - for short text up to 1,000 characters, ~300ms latency
pub const UNREALSPEECH_STREAM_URL: &str = "https://api.v8.unrealspeech.com/stream";

/// Speech endpoint - for medium text up to 3,000 characters
pub const UNREALSPEECH_SPEECH_URL: &str = "https://api.v8.unrealspeech.com/speech";

/// Synthesis tasks endpoint - for long text up to 500,000 characters
pub const UNREALSPEECH_SYNTHESIS_URL: &str = "https://api.v8.unrealspeech.com/synthesisTasks";

/// Stream with timestamps endpoint
pub const UNREALSPEECH_STREAM_TIMESTAMPS_URL: &str =
    "https://api.v8.unrealspeech.com/streamWithTimestamps";

/// Maximum text length for /stream endpoint
pub const MAX_STREAM_TEXT_LENGTH: usize = 1000;

/// Maximum text length for /speech endpoint
pub const MAX_SPEECH_TEXT_LENGTH: usize = 3000;

/// Maximum text length for /synthesisTasks endpoint
pub const MAX_SYNTHESIS_TEXT_LENGTH: usize = 500000;

/// Default voice ID
pub const DEFAULT_VOICE_ID: &str = "Scarlett";

/// Default bitrate
pub const DEFAULT_BITRATE: &str = "192k";

/// Default speed (0 = normal)
pub const DEFAULT_SPEED: f32 = 0.0;

/// Default pitch (1.0 = normal)
pub const DEFAULT_PITCH: f32 = 1.0;
