//! Speechmatics STT Provider
//!
//! Real-time speech-to-text using Speechmatics WebSocket streaming API.
//! Supports 55+ languages with standard and enhanced accuracy models.
//!
//! # Features
//!
//! - Real-time WebSocket streaming
//! - 55+ language support with automatic detection
//! - Standard and enhanced operating points
//! - Speaker diarization
//! - Partial and final transcripts
//! - Custom vocabulary support
//! - Entity recognition
//!
//! # Example
//!
//! ```rust,ignore
//! use waav_gateway::core::stt::speechmatics::{SpeechmaticsSTT, SpeechmaticsSTTConfig};
//! use waav_gateway::core::stt::base::{BaseSTT, STTConfig};
//!
//! let config = STTConfig {
//!     api_key: "your-api-key".to_string(),
//!     language: "en".to_string(),
//!     ..Default::default()
//! };
//!
//! let mut stt = SpeechmaticsSTT::new(config)?;
//! stt.connect().await?;
//! ```

pub mod client;
pub mod config;
pub mod messages;

pub use client::SpeechmaticsSTT;
pub use config::{
    SpeechmaticsEncoding, SpeechmaticsLanguage, SpeechmaticsOperatingPoint, SpeechmaticsRegion,
    SpeechmaticsSTTConfig,
};
pub use messages::{
    AddPartialTranscriptMessage, AddTranscriptMessage, EndOfStreamMessage, ErrorMessage,
    RecognitionStartedMessage, StartRecognitionMessage,
};

/// Default Speechmatics WebSocket URL (EU region)
pub const SPEECHMATICS_WS_URL_EU: &str = "wss://eu.rt.speechmatics.com/v2";

/// Speechmatics WebSocket URL (US region)
pub const SPEECHMATICS_WS_URL_US: &str = "wss://us.rt.speechmatics.com/v2";

/// Speechmatics JWT generation endpoint
pub const SPEECHMATICS_JWT_URL: &str = "https://mp.speechmatics.com/v1/api_keys";

/// Default operating point
pub const DEFAULT_OPERATING_POINT: &str = "standard";

/// Default maximum delay in seconds
pub const DEFAULT_MAX_DELAY: f32 = 2.0;

/// Default language
pub const DEFAULT_LANGUAGE: &str = "en";

/// WebSocket message timeout for idle connections
pub const WS_MESSAGE_TIMEOUT_SECS: u64 = 60;

/// Keep-alive interval in seconds
pub const KEEPALIVE_INTERVAL_SECS: u64 = 10;
