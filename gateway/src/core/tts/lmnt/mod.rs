//! LMNT TTS provider implementation.
//!
//! This module provides integration with the LMNT Text-to-Speech API,
//! offering ultra-low latency (~150ms) speech synthesis with voice cloning support.
//!
//! # Features
//!
//! - **HTTP Streaming**: Low-latency audio generation via HTTP POST
//! - **Voice Cloning**: Create custom voices from 5+ second audio samples
//! - **22+ Languages**: Support for multiple languages with auto-detection
//! - **Expressiveness Control**: `top_p` and `temperature` parameters for voice variation
//!
//! # Example
//!
//! ```rust,ignore
//! use waav_gateway::core::tts::lmnt::LmntTts;
//! use waav_gateway::core::tts::{TTSConfig, BaseTTS};
//!
//! let config = TTSConfig {
//!     api_key: "your-lmnt-api-key".to_string(),
//!     voice_id: Some("lily".to_string()),
//!     audio_format: Some("linear16".to_string()),
//!     sample_rate: Some(24000),
//!     ..Default::default()
//! };
//!
//! let mut tts = LmntTts::new(config)?;
//! tts.connect().await?;
//! tts.speak("Hello, world!", true).await?;
//! ```
//!
//! # API Reference
//!
//! - HTTP Streaming: `POST https://api.lmnt.com/v1/ai/speech/bytes`
//! - Voice List: `GET https://api.lmnt.com/v1/ai/voice/list`
//! - Voice Clone: `POST https://api.lmnt.com/v1/ai/voice`
//!
//! # Authentication
//!
//! LMNT uses API key authentication via the `X-API-Key` header.

pub mod config;
pub mod messages;
pub mod provider;

pub use config::{LmntAudioFormat, LmntTtsConfig};
pub use messages::{LmntVoice, LmntVoiceCloneRequest, LmntVoiceOwner, LmntVoiceType};
pub use provider::LmntTts;

// =============================================================================
// API Constants
// =============================================================================

/// LMNT TTS HTTP streaming endpoint.
///
/// This is the recommended endpoint for most text-to-speech use cases,
/// offering low-latency streaming audio generation.
pub const LMNT_TTS_URL: &str = "https://api.lmnt.com/v1/ai/speech/bytes";

/// LMNT voice list endpoint.
///
/// Returns available voices including system voices and user-created clones.
pub const LMNT_VOICE_LIST_URL: &str = "https://api.lmnt.com/v1/ai/voice/list";

/// LMNT voice clone endpoint.
///
/// Create custom voices from audio samples (minimum 5 seconds).
pub const LMNT_VOICE_CLONE_URL: &str = "https://api.lmnt.com/v1/ai/voice";

/// LMNT WebSocket streaming endpoint for ultra-low latency.
///
/// Supports bidirectional streaming with flush control.
pub const LMNT_WS_URL: &str = "wss://api.lmnt.com/v1/ai/speech/stream";

// =============================================================================
// Limits and Defaults
// =============================================================================

/// Maximum characters per TTS request.
pub const MAX_TEXT_LENGTH: usize = 5000;

/// Default model name.
pub const DEFAULT_MODEL: &str = "blizzard";

/// Default language (auto-detect).
pub const DEFAULT_LANGUAGE: &str = "auto";

/// Default sample rate in Hz.
pub const DEFAULT_SAMPLE_RATE: u32 = 24000;

/// Minimum top_p value (speech stability).
pub const MIN_TOP_P: f32 = 0.0;

/// Maximum top_p value (speech stability).
pub const MAX_TOP_P: f32 = 1.0;

/// Default top_p value.
pub const DEFAULT_TOP_P: f32 = 0.8;

/// Minimum temperature value (expressiveness).
pub const MIN_TEMPERATURE: f32 = 0.0;

/// Default temperature value.
pub const DEFAULT_TEMPERATURE: f32 = 1.0;

/// Minimum speed value.
pub const MIN_SPEED: f32 = 0.25;

/// Maximum speed value.
pub const MAX_SPEED: f32 = 2.0;

/// Default speed value.
pub const DEFAULT_SPEED: f32 = 1.0;
