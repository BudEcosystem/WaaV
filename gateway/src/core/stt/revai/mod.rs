//! Rev AI STT Provider
//!
//! Real-time speech-to-text using Rev AI API with WebSocket streaming.
//! Supports 9+ streaming languages with speaker detection and custom vocabulary.
//!
//! # Architecture
//!
//! Rev AI uses a single-step WebSocket connection with all parameters in the URL:
//! - Connect to `wss://api.rev.ai/speechtotext/v1/stream` with query parameters
//! - Send binary audio data
//! - Receive partial and final transcript messages
//! - Send "EOS" to gracefully close
//!
//! # Example
//!
//! ```rust,no_run
//! use waav_gateway::core::stt::revai::{RevAISTT, RevAISTTConfig};
//! use waav_gateway::core::stt::BaseSTT;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let config = RevAISTTConfig::new("your-api-key")
//!         .with_language("en")
//!         .with_filter_profanity(true);
//!
//!     let mut stt = RevAISTT::new(config.into())?;
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
pub use client::RevAISTT;
pub use config::{RevAISTTConfig, RevAISampleFormat, RevAITranscriber};
pub use messages::{
    ConnectedMessage, FinalTranscript, PartialTranscript, RevAICloseCode, ServerMessage,
    TranscriptElement,
};

// =============================================================================
// Constants
// =============================================================================

/// Rev AI WebSocket streaming endpoint
pub const REVAI_STREAM_URL: &str = "wss://api.rev.ai/speechtotext/v1/stream";

/// Default sample rate in Hz
pub const DEFAULT_SAMPLE_RATE: u32 = 16000;

/// Default number of channels
pub const DEFAULT_CHANNELS: u8 = 1;

/// Default sample format
pub const DEFAULT_SAMPLE_FORMAT: &str = "S16LE";

/// Default audio layout
pub const DEFAULT_LAYOUT: &str = "interleaved";

/// Default language
pub const DEFAULT_LANGUAGE: &str = "en";

/// Minimum sample rate in Hz
pub const MIN_SAMPLE_RATE: u32 = 8000;

/// Maximum sample rate in Hz
pub const MAX_SAMPLE_RATE: u32 = 48000;

/// Minimum channels
pub const MIN_CHANNELS: u8 = 1;

/// Maximum channels
pub const MAX_CHANNELS: u8 = 10;

/// Maximum stream duration in seconds (3 hours)
pub const MAX_STREAM_DURATION_SECONDS: u64 = 3 * 60 * 60;

/// Default concurrency limit
pub const DEFAULT_CONCURRENCY_LIMIT: u32 = 10;

/// Credit hold duration on connection (10 minutes in seconds)
pub const CREDIT_HOLD_SECONDS: u64 = 10 * 60;

/// Minimum billable duration in seconds
pub const MIN_BILLABLE_SECONDS: u64 = 15;

/// End of stream message to send for graceful close
pub const EOS_MESSAGE: &str = "EOS";

/// Recommended minimum audio chunk size in milliseconds
pub const RECOMMENDED_CHUNK_SIZE_MS: u32 = 250;

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_constants_are_valid() {
        assert!(!REVAI_STREAM_URL.is_empty());
        assert!(DEFAULT_SAMPLE_RATE >= MIN_SAMPLE_RATE);
        assert!(DEFAULT_SAMPLE_RATE <= MAX_SAMPLE_RATE);
        assert!(DEFAULT_CHANNELS >= MIN_CHANNELS);
        assert!(DEFAULT_CHANNELS <= MAX_CHANNELS);
        assert!(!DEFAULT_SAMPLE_FORMAT.is_empty());
        assert!(!DEFAULT_LAYOUT.is_empty());
        assert!(!DEFAULT_LANGUAGE.is_empty());
        assert_eq!(EOS_MESSAGE, "EOS");
    }

    #[test]
    fn test_stream_limits() {
        assert_eq!(MAX_STREAM_DURATION_SECONDS, 10800); // 3 hours
        assert_eq!(CREDIT_HOLD_SECONDS, 600); // 10 minutes
        assert_eq!(MIN_BILLABLE_SECONDS, 15);
    }

    #[test]
    fn test_websocket_url_format() {
        assert!(REVAI_STREAM_URL.starts_with("wss://"));
        assert!(REVAI_STREAM_URL.contains("api.rev.ai"));
    }
}
