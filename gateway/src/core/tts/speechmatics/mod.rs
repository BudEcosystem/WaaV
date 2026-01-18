//! Speechmatics TTS Provider
//!
//! This module provides integration with the Speechmatics Text-to-Speech API,
//! offering high-quality voice synthesis with expressive prosody.
//!
//! # Features
//!
//! - **HTTP Streaming**: Real-time audio output with low latency
//! - **4 English voices**: Sarah (UK), Theo (UK), Megan (US), Jack (US)
//! - **Audio formats**: WAV (16kHz) or raw PCM (16kHz)
//! - **Natural prosody**: Expressive speech without SSML
//!
//! # Example
//!
//! ```rust,ignore
//! use waav_gateway::core::tts::speechmatics::SpeechmaticsTts;
//! use waav_gateway::core::tts::{TTSConfig, BaseTTS};
//!
//! let config = TTSConfig {
//!     api_key: "your-api-key".to_string(),
//!     voice_id: Some("sarah".to_string()),
//!     ..Default::default()
//! };
//!
//! let mut tts = SpeechmaticsTts::new(config)?;
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

/// Speechmatics TTS API base URL (preview)
pub const SPEECHMATICS_TTS_BASE_URL: &str = "https://preview.tts.speechmatics.com";

/// Generate endpoint - URL pattern with voice_id placeholder
pub const SPEECHMATICS_GENERATE_URL: &str = "https://preview.tts.speechmatics.com/generate";

/// Default voice ID
pub const DEFAULT_VOICE_ID: &str = "sarah";

/// Default output format
pub const DEFAULT_OUTPUT_FORMAT: &str = "wav_16000";

/// Default sample rate
pub const DEFAULT_SAMPLE_RATE: u32 = 16000;

/// Maximum text length (approximately 5000 chars for a single request)
pub const MAX_TEXT_LENGTH: usize = 5000;
