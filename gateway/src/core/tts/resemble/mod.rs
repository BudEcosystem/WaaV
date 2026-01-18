//! Resemble AI TTS Provider
//!
//! This module provides Text-to-Speech integration with Resemble AI API,
//! supporting voice cloning, multiple models, and streaming synthesis.
//!
//! # Models
//!
//! - **Chatterbox**: Standard high-quality synthesis
//! - **Chatterbox Turbo**: Low-latency synthesis (~350M params), paralinguistic tags
//! - **Chatterbox Multilingual**: 24+ language support
//!
//! # Example
//!
//! ```rust,ignore
//! use waav_gateway::core::tts::resemble::ResembleTts;
//! use waav_gateway::core::tts::{TTSConfig, BaseTTS};
//!
//! let config = TTSConfig {
//!     api_key: "your-api-key".to_string(),
//!     voice_id: Some("voice-uuid".to_string()),
//!     ..Default::default()
//! };
//!
//! let mut tts = ResembleTts::new(config)?;
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

/// Resemble AI synchronous TTS endpoint
pub const RESEMBLE_TTS_SYNC_URL: &str = "https://f.cluster.resemble.ai/synthesize";

/// Resemble AI HTTP streaming TTS endpoint
pub const RESEMBLE_TTS_STREAM_URL: &str = "https://f.cluster.resemble.ai/stream";

/// Resemble AI voices list endpoint
pub const RESEMBLE_VOICES_URL: &str = "https://app.resemble.ai/api/v2/voices";

/// Resemble AI WebSocket streaming endpoint (Business plan required)
pub const RESEMBLE_WS_URL: &str = "wss://websocket.cluster.resemble.ai/stream";

/// Maximum text length for sync/websocket synthesis (characters)
pub const MAX_TEXT_LENGTH_SYNC: usize = 3000;

/// Maximum text length for HTTP streaming synthesis (characters)
pub const MAX_TEXT_LENGTH_STREAM: usize = 2000;

/// Default sample rate (Hz)
pub const DEFAULT_SAMPLE_RATE: u32 = 22050;
