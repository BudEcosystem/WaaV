//! Reverie STT Provider
//!
//! Real-time speech-to-text using Reverie API with WebSocket streaming.
//! Optimized for 22+ Indian languages with dialect-agnostic recognition.
//!
//! # Architecture
//!
//! Reverie uses a single-step WebSocket connection with all parameters in query string:
//! 1. Connect to wss://revapi.reverieinc.com/stream?params...
//! 2. Stream binary audio chunks
//! 3. Send --EOF-- to signal end of stream
//!
//! # Example
//!
//! ```rust,no_run
//! use waav_gateway::core::stt::reverie::{ReverieSTT, ReverieSTTConfig};
//! use waav_gateway::core::stt::BaseSTT;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let config = ReverieSTTConfig::new("your-api-key", "your-app-id")
//!         .with_language(ReverieLanguage::Hindi)
//!         .with_punctuate(true);
//!
//!     let mut stt = ReverieSTT::new(config)?;
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

pub mod config;
pub mod messages;
pub mod client;

// Re-export public types
pub use config::{
    ReverieAudioFormat, ReverieLanguage, ReverieLogging, ReverieSTTConfig,
};
pub use messages::{
    ReverieAsrResult, ReverieCloseReason, ReverieServerMessage,
};
pub use client::ReverieSTT;

// =============================================================================
// Constants
// =============================================================================

/// Reverie WebSocket streaming endpoint
pub const REVERIE_STREAM_URL: &str = "wss://revapi.reverieinc.com/stream";

/// Reverie REST API base URL
pub const REVERIE_API_BASE_URL: &str = "https://revapi.reverieinc.com";

/// App name for streaming STT
pub const STT_STREAM_APPNAME: &str = "stt_stream";

/// App name for file STT
pub const STT_FILE_APPNAME: &str = "stt_file";

/// App name for batch STT
pub const STT_BATCH_APPNAME: &str = "stt_batch";

/// End-of-stream marker
pub const EOF_MARKER: &[u8] = b"--EOF--";

/// Default sample rate in Hz
pub const DEFAULT_SAMPLE_RATE: u32 = 16000;

/// Default audio format
pub const DEFAULT_AUDIO_FORMAT: &str = "16k_int16";

/// Default timeout in seconds
pub const DEFAULT_TIMEOUT: u32 = 15;

/// Maximum timeout in seconds
pub const MAX_TIMEOUT: u32 = 180;

/// Default silence detection in seconds
pub const DEFAULT_SILENCE: u32 = 1;

/// Maximum silence detection in seconds
pub const MAX_SILENCE: u32 = 30;

/// Default domain
pub const DEFAULT_DOMAIN: &str = "generic";

/// Maximum audio duration in seconds
pub const MAX_AUDIO_DURATION: u32 = 180;
