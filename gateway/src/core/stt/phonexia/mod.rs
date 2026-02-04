//! Phonexia STT Provider
//!
//! On-premises speech-to-text using Phonexia Speech Platform with WebSocket streaming.
//! Supports 57-64 languages with voice biometrics, speaker identification, and language ID.
//!
//! # Architecture
//!
//! Phonexia is an on-premises/self-hosted solution requiring user-configured server URL.
//! Two API interfaces available:
//! - WebSocket API (legacy SPE): `/input_stream/websocket` with RAW s16le audio
//! - gRPC API (Speech Platform 4): High-performance streaming (not implemented here)
//!
//! # Authentication
//!
//! - Token-based: Login via `/login` endpoint, use X-SessionID header
//! - HTTP Basic Auth: Direct credentials in Authorization header
//!
//! # Example
//!
//! ```rust,no_run
//! use waav_gateway::core::stt::phonexia::{PhonexiaSTT, PhonexiaSTTConfig};
//! use waav_gateway::core::stt::BaseSTT;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let config = PhonexiaSTTConfig::new("https://your-phonexia-server.com")
//!         .with_basic_auth("username", "password")
//!         .with_language("en-US")
//!         .with_sample_rate(16000);
//!
//!     let mut stt = PhonexiaSTT::new(config.into())?;
//!     stt.connect().await?;
//!
//!     // Send audio data (RAW s16le format)
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
pub use client::PhonexiaSTT;
pub use config::{PhonexiaAuth, PhonexiaResultType, PhonexiaSTTConfig};
pub use messages::{
    PhonexiaCloseCode, PhonexiaError, PhonexiaResult, ServerMessage, TranscriptSegment,
    TranscriptWord,
};

// =============================================================================
// Constants
// =============================================================================

/// Default WebSocket endpoint path (SPE legacy API)
pub const WEBSOCKET_PATH: &str = "/input_stream/websocket";

/// Default REST login path for token-based auth
pub const LOGIN_PATH: &str = "/login";

/// Default STT technology path
pub const STT_TECHNOLOGY_PATH: &str = "/technologies/stt";

/// Session ID header name
pub const SESSION_ID_HEADER: &str = "X-SessionID";

/// Default sample rate in Hz
pub const DEFAULT_SAMPLE_RATE: u32 = 16000;

/// Default number of channels
pub const DEFAULT_CHANNELS: u8 = 1;

/// Default audio format (RAW PCM signed 16-bit little-endian)
pub const DEFAULT_AUDIO_FORMAT: &str = "s16le";

/// Minimum sample rate in Hz
pub const MIN_SAMPLE_RATE: u32 = 8000;

/// Maximum sample rate in Hz
pub const MAX_SAMPLE_RATE: u32 = 48000;

/// Minimum channels
pub const MIN_CHANNELS: u8 = 1;

/// Maximum channels
pub const MAX_CHANNELS: u8 = 2;

/// Default stream timeout in seconds (no data sent)
pub const DEFAULT_STREAM_TIMEOUT_SECONDS: u64 = 10;

/// Maximum message size for gRPC (4MB)
pub const GRPC_MAX_MESSAGE_SIZE: usize = 4 * 1024 * 1024;

/// Recommended audio chunk size in bytes (for streaming)
pub const RECOMMENDED_CHUNK_SIZE_BYTES: usize = 4096;

/// Default connection timeout in seconds
pub const DEFAULT_CONNECTION_TIMEOUT_SECONDS: u64 = 30;

/// Default request timeout in seconds
pub const DEFAULT_REQUEST_TIMEOUT_SECONDS: u64 = 60;

// =============================================================================
// Supported Languages
// =============================================================================

/// List of commonly supported Phonexia languages
pub const SUPPORTED_LANGUAGES: &[&str] = &[
    // Eastern European
    "cs", // Czech
    "sk", // Slovak
    "pl", // Polish
    "hu", // Hungarian
    "ro", // Romanian
    "bg", // Bulgarian
    "sr", // Serbian
    "hr", // Croatian
    "sl", // Slovenian
    "uk", // Ukrainian
    "ru", // Russian
    "be", // Belarusian
    // Western European
    "en-US", // English (US)
    "en-GB", // English (UK)
    "de",    // German
    "fr",    // French
    "es",    // Spanish
    "it",    // Italian
    "pt",    // Portuguese
    "nl",    // Dutch
    "da",    // Danish
    "sv",    // Swedish
    "no",    // Norwegian
    "fi",    // Finnish
    // Middle Eastern
    "ar", // Arabic
    "tr", // Turkish
    "fa", // Persian
    "he", // Hebrew
    // Asian
    "zh", // Chinese (Mandarin)
    "ja", // Japanese
    "ko", // Korean
    "th", // Thai
    "vi", // Vietnamese
    "id", // Indonesian
];

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_constants_are_valid() {
        assert!(!WEBSOCKET_PATH.is_empty());
        assert!(WEBSOCKET_PATH.starts_with('/'));
        assert!(!LOGIN_PATH.is_empty());
        assert!(LOGIN_PATH.starts_with('/'));
        assert!(DEFAULT_SAMPLE_RATE >= MIN_SAMPLE_RATE);
        assert!(DEFAULT_SAMPLE_RATE <= MAX_SAMPLE_RATE);
        assert!(DEFAULT_CHANNELS >= MIN_CHANNELS);
        assert!(DEFAULT_CHANNELS <= MAX_CHANNELS);
        assert!(!DEFAULT_AUDIO_FORMAT.is_empty());
        assert!(DEFAULT_STREAM_TIMEOUT_SECONDS > 0);
    }

    #[test]
    fn test_sample_rate_bounds() {
        assert!(MIN_SAMPLE_RATE > 0);
        assert!(MAX_SAMPLE_RATE > MIN_SAMPLE_RATE);
        assert_eq!(MIN_SAMPLE_RATE, 8000);
        assert_eq!(MAX_SAMPLE_RATE, 48000);
    }

    #[test]
    fn test_channel_bounds() {
        assert_eq!(MIN_CHANNELS, 1);
        assert_eq!(MAX_CHANNELS, 2);
    }

    #[test]
    fn test_grpc_max_message_size() {
        assert_eq!(GRPC_MAX_MESSAGE_SIZE, 4 * 1024 * 1024);
    }

    #[test]
    fn test_supported_languages_not_empty() {
        assert!(!SUPPORTED_LANGUAGES.is_empty());
        assert!(SUPPORTED_LANGUAGES.contains(&"en-US"));
        assert!(SUPPORTED_LANGUAGES.contains(&"cs")); // Czech - key Phonexia language
    }

    #[test]
    fn test_paths_format() {
        assert!(WEBSOCKET_PATH.starts_with('/'));
        assert!(LOGIN_PATH.starts_with('/'));
        assert!(STT_TECHNOLOGY_PATH.starts_with('/'));
    }
}
