//! Acapela Cloud TTS Provider
//!
//! This module provides Text-to-Speech integration with Acapela Cloud API,
//! supporting 250+ voices across 30+ languages with advanced features:
//! - **AI Neural Voices**: Natural-sounding synthesis using Deep Neural Networks
//! - **Word Position Events**: Real-time word timing for text highlighting
//! - **Viseme Data**: Mouth shape data for lip-sync animation
//! - **Custom Dictionaries**: Upload pronunciation dictionaries
//!
//! # Authentication
//!
//! Acapela Cloud uses email/password authentication to obtain a session token.
//! The token is cached and reused for subsequent requests.
//!
//! # Example
//!
//! ```rust,ignore
//! use waav_gateway::core::tts::acapela::AcapelaTts;
//! use waav_gateway::core::tts::{TTSConfig, BaseTTS};
//!
//! // Using email/password authentication
//! let config = TTSConfig {
//!     api_key: "user@example.com:password".to_string(),  // email:password format
//!     voice_id: Some("alice".to_string()),
//!     ..Default::default()
//! };
//!
//! let mut tts = AcapelaTts::new(config)?;
//! tts.connect().await?;  // This performs login and caches token
//! tts.speak("Hello, world!", true).await?;
//! ```

mod config;
mod messages;
mod provider;

pub use config::*;
pub use messages::*;
pub use provider::*;

// =============================================================================
// API Constants
// =============================================================================

/// Acapela Cloud base URL
pub const ACAPELA_BASE_URL: &str = "https://www.acapela-cloud.com";

/// Acapela Cloud login endpoint
pub const ACAPELA_LOGIN_URL: &str = "https://www.acapela-cloud.com/api/login/";

/// Acapela Cloud logout endpoint
pub const ACAPELA_LOGOUT_URL: &str = "https://www.acapela-cloud.com/api/logout/";

/// Acapela Cloud TTS command endpoint (synthesis)
pub const ACAPELA_COMMAND_URL: &str = "https://www.acapela-cloud.com/api/command/";

/// Acapela Cloud account info endpoint
pub const ACAPELA_ACCOUNT_URL: &str = "https://www.acapela-cloud.com/api/account/";

/// Acapela Cloud storage endpoint (dictionaries, audio files)
pub const ACAPELA_STORAGE_URL: &str = "https://www.acapela-cloud.com/api/storage/";

/// Maximum text length per request (GET method)
pub const MAX_TEXT_LENGTH_GET: usize = 2048;

/// Maximum text length per request (stream output)
pub const MAX_TEXT_LENGTH_STREAM: usize = 3000;

/// Default voice ID (Alice - French female)
pub const DEFAULT_VOICE_ID: &str = "alice";

/// Default sample rate (Hz)
pub const DEFAULT_SAMPLE_RATE: u32 = 22050;

/// Minimum sample rate (Hz)
pub const MIN_SAMPLE_RATE: u32 = 8000;

/// Maximum sample rate (Hz)
pub const MAX_SAMPLE_RATE: u32 = 48000;

/// Default speech speed (100 = normal)
pub const DEFAULT_SPEED: u32 = 100;

/// Minimum speech speed
pub const MIN_SPEED: u32 = 30;

/// Maximum speech speed
pub const MAX_SPEED: u32 = 300;

/// Default volume level
pub const DEFAULT_VOLUME: u32 = 32768;

/// Minimum volume level
pub const MIN_VOLUME: u32 = 50;

/// Maximum volume level
pub const MAX_VOLUME: u32 = 65535;

/// Default voice shaping
pub const DEFAULT_SHAPING: u32 = 100;

/// Minimum voice shaping
pub const MIN_SHAPING: u32 = 50;

/// Maximum voice shaping
pub const MAX_SHAPING: u32 = 150;
