//! Gladia STT Provider
//!
//! Real-time speech-to-text using Gladia API with WebSocket streaming.
//! Supports 100+ languages, code-switching, translation, and <300ms latency.
//!
//! # Architecture
//!
//! Gladia uses a two-step connection process:
//! 1. POST to /v2/live to initialize session and get WebSocket URL
//! 2. Connect to the returned WebSocket URL for streaming
//!
//! # Example
//!
//! ```rust,no_run
//! use waav_gateway::core::stt::gladia::{GladiaSTT, GladiaSTTConfig};
//! use waav_gateway::core::stt::BaseSTT;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let config = GladiaSTTConfig::new("your-api-key")
//!         .with_language("en")
//!         .with_word_timestamps(true);
//!
//!     let mut stt = GladiaSTT::new(config)?;
//!     stt.connect().await?;
//!
//!     // Send audio data
//!     let audio = vec![0u8; 1024];
//!     stt.send_audio(audio.into()).await?;
//!
//!     stt.disconnect().await?;
//!     Ok(())
//! }
//! ```

pub mod client;
pub mod config;
pub mod messages;

// Re-export public types
pub use client::GladiaSTT;
pub use config::{
    GladiaBitDepth, GladiaEncoding, GladiaLanguageConfig, GladiaMessagesConfig,
    GladiaPreProcessing, GladiaRealtimeProcessing, GladiaRegion, GladiaSTTConfig,
};
pub use messages::{
    AudioChunkMessage, GladiaError, InitSessionRequest, InitSessionResponse, StopRecordingMessage,
    TranscriptData, TranscriptMessage, UtteranceData, WordData,
};

// =============================================================================
// Constants
// =============================================================================

/// Gladia API base URL
pub const GLADIA_API_BASE_URL: &str = "https://api.gladia.io";

/// Gladia Live Transcription initialization URL
pub const GLADIA_LIVE_URL: &str = "https://api.gladia.io/v2/live";

/// Default model (currently the only supported model)
pub const DEFAULT_MODEL: &str = "solaria-1";

/// Default sample rate in Hz
pub const DEFAULT_SAMPLE_RATE: u32 = 16000;

/// Default bit depth
pub const DEFAULT_BIT_DEPTH: u8 = 16;

/// Default number of channels
pub const DEFAULT_CHANNELS: u8 = 1;

/// Default endpointing duration in seconds
pub const DEFAULT_ENDPOINTING: f32 = 0.05;

/// Default maximum duration without endpointing in seconds
pub const DEFAULT_MAX_DURATION_WITHOUT_ENDPOINTING: f32 = 5.0;

/// Minimum endpointing duration in seconds
pub const MIN_ENDPOINTING: f32 = 0.01;

/// Maximum endpointing duration in seconds
pub const MAX_ENDPOINTING: f32 = 10.0;

/// Minimum maximum duration without endpointing in seconds
pub const MIN_MAX_DURATION: f32 = 5.0;

/// Maximum maximum duration without endpointing in seconds
pub const MAX_MAX_DURATION: f32 = 60.0;

/// Gladia's reported latency for partial transcripts
pub const PARTIAL_LATENCY_MS: u32 = 300;

/// Gladia's reported latency for final transcripts (typical 3s utterance)
pub const FINAL_LATENCY_MS: u32 = 700;
