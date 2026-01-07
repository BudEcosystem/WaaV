//! Play.ht TTS provider implementation.
//!
//! This module provides integration with the Play.ht Text-to-Speech API,
//! offering low-latency speech synthesis with voice cloning support.
//!
//! # Features
//!
//! - **HTTP Streaming**: Low-latency audio generation via HTTP POST (~190ms)
//! - **Voice Cloning**: Create custom voices from 30+ second audio samples
//! - **36+ Languages**: Support for multiple languages with auto-detection
//! - **PlayDialog**: Multi-turn dialogue support with two-speaker generation
//! - **Multiple Models**: Play3.0-mini, PlayDialog, PlayHT2.0-turbo
//!
//! # Example
//!
//! ```rust,ignore
//! use waav_gateway::core::tts::playht::PlayHtTts;
//! use waav_gateway::core::tts::{TTSConfig, BaseTTS};
//!
//! let config = TTSConfig {
//!     api_key: "your-playht-api-key".to_string(),
//!     voice_id: Some("s3://voice-cloning-zero-shot/.../manifest.json".to_string()),
//!     audio_format: Some("mp3".to_string()),
//!     sample_rate: Some(48000),
//!     ..Default::default()
//! };
//!
//! // Option 1: Use with_user_id for explicit user ID
//! let mut tts = PlayHtTts::with_user_id(config.clone(), "your-user-id".to_string())?;
//!
//! // Option 2: Use new() which reads PLAYHT_USER_ID from environment
//! // let mut tts = PlayHtTts::new(config)?;
//!
//! tts.connect().await?;
//! tts.speak("Hello, world!", true).await?;
//! ```
//!
//! # API Reference
//!
//! - HTTP Streaming: `POST https://api.play.ht/api/v2/tts/stream`
//! - Voice List: `GET https://api.play.ht/api/v2/voices`
//! - Voice Clone: `POST https://api.play.ht/api/v2/cloned-voices/instant`
//! - WebSocket Auth: `POST https://api.play.ht/api/v4/websocket-auth`
//!
//! # Authentication
//!
//! Play.ht uses dual-header authentication:
//! - `X-USER-ID`: Your Play.ht user ID
//! - `AUTHORIZATION`: Your Play.ht API key

pub mod config;
pub mod messages;
pub mod provider;

pub use config::{PlayHtAudioFormat, PlayHtModel, PlayHtTtsConfig};
pub use messages::{
    PlayHtApiError, PlayHtVoice, PlayHtVoiceCloneRequest, PlayHtWsAuthResponse, PlayHtWsMessage,
};
pub use provider::PlayHtTts;

// =============================================================================
// API Constants
// =============================================================================

/// Play.ht TTS HTTP streaming endpoint.
///
/// This is the recommended endpoint for most text-to-speech use cases,
/// offering low-latency streaming audio generation.
pub const PLAYHT_TTS_URL: &str = "https://api.play.ht/api/v2/tts/stream";

/// Play.ht voice list endpoint.
///
/// Returns available voices including system voices and user-created clones.
pub const PLAYHT_VOICE_LIST_URL: &str = "https://api.play.ht/api/v2/voices";

/// Play.ht WebSocket authentication endpoint.
///
/// Returns URLs for WebSocket connections with authentication tokens.
pub const PLAYHT_WS_AUTH_URL: &str = "https://api.play.ht/api/v4/websocket-auth";

/// Play.ht voice clone endpoint.
///
/// Create custom voices from audio samples (minimum 30 seconds recommended).
pub const PLAYHT_CLONE_URL: &str = "https://api.play.ht/api/v2/cloned-voices/instant";

// =============================================================================
// Limits and Defaults
// =============================================================================

/// Maximum characters per TTS request.
pub const MAX_TEXT_LENGTH: usize = 20000;

/// Default model (voice engine).
pub const DEFAULT_MODEL: PlayHtModel = PlayHtModel::Play30Mini;

/// Default sample rate in Hz.
pub const DEFAULT_SAMPLE_RATE: u32 = 48000;

/// Minimum speed value.
pub const MIN_SPEED: f32 = 0.5;

/// Maximum speed value.
pub const MAX_SPEED: f32 = 2.0;

/// Default speed value.
pub const DEFAULT_SPEED: f32 = 1.0;

/// Minimum temperature value.
pub const MIN_TEMPERATURE: f32 = 0.0;

/// Maximum temperature value.
pub const MAX_TEMPERATURE: f32 = 1.0;
