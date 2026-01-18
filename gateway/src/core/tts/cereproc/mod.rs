//! CereProc CereVoice Cloud TTS Module
//!
//! This module provides integration with CereProc's CereVoice Cloud text-to-speech API.
//! CereProc specializes in characterful, emotional TTS voices with support for
//! Celtic languages (Welsh, Scottish Gaelic, Irish).
//!
//! # Features
//!
//! - Bearer token authentication (email/password)
//! - 20+ voices with emotional variations
//! - Multiple audio formats (WAV, MP3, OGG)
//! - SSML support with custom emotion tags
//! - Custom lexicons and abbreviations
//! - Celtic language support
//!
//! # Example
//!
//! ```rust,ignore
//! use waav_gateway::core::tts::cereproc::CereprocTts;
//! use waav_gateway::core::tts::base::{BaseTTS, TTSConfig};
//!
//! let config = TTSConfig {
//!     api_key: "email:password".to_string(),
//!     voice_id: Some("Stuart".to_string()),
//!     audio_format: Some("mp3".to_string()),
//!     ..Default::default()
//! };
//!
//! let mut tts = CereprocTts::new(config)?;
//! tts.connect().await?;
//! tts.speak("Hello world!", true).await?;
//! ```

pub mod config;
pub mod messages;
pub mod provider;

// Re-export main types
pub use config::{CereprocAudioFormat, CereprocCredentials, CereprocTtsConfig};
pub use messages::{AuthResponse, CereprocApiError, SpeakResponse};
pub use provider::CereprocTts;

// =============================================================================
// API Constants
// =============================================================================

/// CereVoice Cloud API v2 base URL
pub const CEREVOICE_BASE_URL: &str = "https://api.cerevoice.com/v2";

/// Authentication endpoint
pub const CEREVOICE_AUTH_URL: &str = "https://api.cerevoice.com/v2/auth";

/// TTS synthesis endpoint
pub const CEREVOICE_SPEAK_URL: &str = "https://api.cerevoice.com/v2/speak";

/// Credit check endpoint
pub const CEREVOICE_CREDIT_URL: &str = "https://api.cerevoice.com/v2/credit";

/// Voices list endpoint
pub const CEREVOICE_VOICES_URL: &str = "https://api.cerevoice.com/v2/voices";

/// Audio formats list endpoint
pub const CEREVOICE_FORMATS_URL: &str = "https://api.cerevoice.com/v2/formats";

/// Lexicons endpoint
pub const CEREVOICE_LEXICONS_URL: &str = "https://api.cerevoice.com/v2/lexicons";

// =============================================================================
// Default Configuration
// =============================================================================

/// Default voice (Stuart - Scottish English male)
pub const DEFAULT_VOICE_ID: &str = "Stuart";

/// Default audio format
pub const DEFAULT_AUDIO_FORMAT: &str = "mp3";

/// Default sample rate (Hz)
pub const DEFAULT_SAMPLE_RATE: u32 = 22050;

/// Minimum sample rate (Hz)
pub const MIN_SAMPLE_RATE: u32 = 8000;

/// Maximum sample rate (Hz)
pub const MAX_SAMPLE_RATE: u32 = 48000;

/// Token cache TTL (30 minutes, same as Acapela)
pub const TOKEN_CACHE_TTL_SECS: u64 = 30 * 60;

/// Maximum text length per request (characters)
pub const MAX_TEXT_LENGTH: usize = 5000;
