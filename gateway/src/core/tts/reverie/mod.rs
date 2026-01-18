//! Reverie Language Technologies TTS Provider Implementation
//!
//! This module provides integration with Reverie's Text-to-Speech API,
//! optimized for 22+ Indian languages with 36+ voices.
//!
//! # Features
//!
//! - **REST API**: HTTP POST for audio synthesis
//! - **36+ Voices**: Male and female voices across multiple Indian languages
//! - **SSML Support**: Custom voice control via SSML markup
//! - **Multiple Formats**: WAV and MP3 output
//! - **Adjustable Parameters**: Speed (0.5-1.5) and pitch (-3 to +3)
//!
//! # Example
//!
//! ```rust,ignore
//! use waav_gateway::core::tts::reverie::ReverieTts;
//! use waav_gateway::core::tts::{TTSConfig, BaseTTS};
//!
//! let config = TTSConfig {
//!     api_key: "your-reverie-api-key".to_string(),
//!     voice_id: Some("hi_female".to_string()),
//!     model: "your-app-id".to_string(), // App ID stored in model field
//!     audio_format: Some("wav".to_string()),
//!     sample_rate: Some(22050),
//!     ..Default::default()
//! };
//!
//! let mut tts = ReverieTts::new(config)?;
//! tts.connect().await?;
//! tts.speak("नमस्ते, दुनिया!", true).await?;
//! ```
//!
//! # API Reference
//!
//! - TTS Endpoint: `POST https://revapi.reverieinc.com/`
//!
//! # Authentication
//!
//! Reverie uses header-based authentication:
//! - `REV-API-KEY`: Your API key
//! - `REV-APP-ID`: Your application ID
//! - `REV-APPNAME`: "tts"
//! - `speaker`: Speaker code (e.g., "hi_female")

pub mod config;
pub mod messages;
pub mod provider;

pub use config::{ReverieGender, ReverieSpeaker, ReverieTtsAudioFormat, ReverieTtsConfig};
pub use messages::{ReverieTtsRequest, ReverieTtsResponse};
pub use provider::ReverieTts;

// =============================================================================
// API Constants
// =============================================================================

/// Reverie TTS REST endpoint.
pub const REVERIE_TTS_URL: &str = "https://revapi.reverieinc.com/";

/// Application name for TTS requests.
pub const TTS_APP_NAME: &str = "tts";

// =============================================================================
// Limits and Defaults
// =============================================================================

/// Maximum characters per TTS request.
pub const MAX_TEXT_LENGTH: usize = 5000;

/// Default speaker voice.
pub const DEFAULT_SPEAKER: &str = "hi_female";

/// Default audio format.
pub const DEFAULT_FORMAT: ReverieTtsAudioFormat = ReverieTtsAudioFormat::Wav;

/// Default sample rate in Hz.
pub const DEFAULT_SAMPLE_RATE: u32 = 22050;

/// Minimum speed value.
pub const MIN_SPEED: f32 = 0.5;

/// Maximum speed value.
pub const MAX_SPEED: f32 = 1.5;

/// Default speed value.
pub const DEFAULT_SPEED: f32 = 1.0;

/// Minimum pitch value (semitones).
pub const MIN_PITCH: f32 = -3.0;

/// Maximum pitch value (semitones).
pub const MAX_PITCH: f32 = 3.0;

/// Default pitch value.
pub const DEFAULT_PITCH: f32 = 0.0;

// =============================================================================
// Supported Sample Rates
// =============================================================================

/// Supported sample rates for Reverie TTS.
pub const SUPPORTED_SAMPLE_RATES: [u32; 7] = [8000, 11025, 16000, 22050, 24000, 44100, 48000];

/// Check if a sample rate is supported.
#[inline]
pub fn is_valid_sample_rate(rate: u32) -> bool {
    SUPPORTED_SAMPLE_RATES.contains(&rate)
}

/// Get the closest supported sample rate.
pub fn nearest_sample_rate(rate: u32) -> u32 {
    SUPPORTED_SAMPLE_RATES
        .iter()
        .min_by_key(|&&r| (r as i32 - rate as i32).abs())
        .copied()
        .unwrap_or(DEFAULT_SAMPLE_RATE)
}
