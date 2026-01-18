//! Murf.ai TTS Provider
//!
//! This module provides a Text-to-Speech implementation using the Murf.ai API.
//!
//! # Features
//!
//! - **Falcon Model**: Ultra-low latency (~130ms TTFA), optimized for real-time conversational AI
//! - **Gen2 Model**: Studio-quality output with full customization (pitch, rate, style)
//! - **150+ Voices**: 35+ languages with 20+ speaking styles per voice
//! - **HTTP Streaming**: Real-time audio streaming via chunked transfer encoding
//! - **Regional Endpoints**: 12 global regions for data residency and latency optimization
//!
//! # Authentication
//!
//! Murf.ai uses API key authentication via the `api-key` header (NOT Bearer token).
//! Set `MURF_API_KEY` environment variable or provide in TTSConfig.
//!
//! # Example
//!
//! ```rust,ignore
//! use waav_gateway::core::tts::murf::MurfTts;
//! use waav_gateway::core::tts::{TTSConfig, BaseTTS};
//!
//! let config = TTSConfig {
//!     api_key: "your-murf-api-key".to_string(),
//!     voice_id: Some("en-US-natalie".to_string()),
//!     audio_format: Some("pcm".to_string()),
//!     sample_rate: Some(24000),
//!     ..Default::default()
//! };
//!
//! let mut tts = MurfTts::new(config)?;
//! tts.connect().await?;
//! tts.speak("Hello, world!", true).await?;
//! ```

pub mod config;
pub mod provider;

pub use config::{MurfAudioFormat, MurfModel, MurfRegion, MurfTtsConfig};
pub use provider::{MurfRequestBuilder, MurfTts};

// =============================================================================
// API URLs
// =============================================================================

/// Murf.ai global streaming endpoint (auto-routes to nearest region)
pub const MURF_TTS_STREAM_URL: &str = "https://global.api.murf.ai/v1/speech/stream";

/// Murf.ai non-streaming generate endpoint
pub const MURF_TTS_GENERATE_URL: &str = "https://api.murf.ai/v1/speech/generate";

/// Murf.ai voices list endpoint
pub const MURF_VOICES_URL: &str = "https://api.murf.ai/v1/speech/voices";

// =============================================================================
// Default Configuration
// =============================================================================

/// Default model (Falcon for real-time low latency)
pub const DEFAULT_MODEL: &str = "FALCON";

/// Default voice ID
pub const DEFAULT_VOICE_ID: &str = "en-US-natalie";

/// Default sample rate for streaming (24000 Hz)
pub const DEFAULT_SAMPLE_RATE: u32 = 24000;

/// Default audio format (PCM for streaming)
pub const DEFAULT_FORMAT: &str = "PCM";

/// Maximum text length per request
pub const MAX_TEXT_LENGTH: usize = 5000;

// =============================================================================
// Speech Customization Limits (Gen2 model)
// =============================================================================

/// Minimum rate adjustment (-50)
pub const MIN_RATE: i32 = -50;

/// Maximum rate adjustment (+50)
pub const MAX_RATE: i32 = 50;

/// Minimum pitch adjustment (-50)
pub const MIN_PITCH: i32 = -50;

/// Maximum pitch adjustment (+50)
pub const MAX_PITCH: i32 = 50;

/// Minimum variation (0)
pub const MIN_VARIATION: u8 = 0;

/// Maximum variation (5)
pub const MAX_VARIATION: u8 = 5;

/// Default rate (0 = normal speed)
pub const DEFAULT_RATE: i32 = 0;

/// Default pitch (0 = normal pitch)
pub const DEFAULT_PITCH: i32 = 0;

/// Default variation (1)
pub const DEFAULT_VARIATION: u8 = 1;
