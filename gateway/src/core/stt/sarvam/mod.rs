//! Sarvam.ai STT Provider
//!
//! Speech-to-text provider for Sarvam.ai's Saarika model,
//! specialized in Indian language recognition.
//!
//! # Features
//!
//! - 11 Indian languages supported (Hindi, Tamil, Telugu, etc.)
//! - Real-time streaming via WebSocket
//! - Voice Activity Detection (VAD) signals
//! - Code-mixing support (Hindi-English, etc.)
//!
//! # Authentication
//!
//! **IMPORTANT**: Sarvam uses `api-subscription-key` header, NOT Bearer token.
//!
//! # Usage
//!
//! ```rust,no_run
//! use waav_gateway::core::stt::{create_stt_provider, STTConfig};
//!
//! let config = STTConfig {
//!     provider: "sarvam".to_string(),
//!     api_key: std::env::var("SARVAM_API_KEY").unwrap(),
//!     language: "hi-IN".to_string(),
//!     sample_rate: 16000,
//!     channels: 1,
//!     punctuation: true,
//!     encoding: "pcm_s16le".to_string(),
//!     model: "saarika:v2.5".to_string(),
//! };
//!
//! let stt = create_stt_provider("sarvam", config).unwrap();
//! ```
//!
//! # Supported Languages
//!
//! | Code | Language |
//! |------|----------|
//! | hi-IN | Hindi |
//! | bn-IN | Bengali |
//! | ta-IN | Tamil |
//! | te-IN | Telugu |
//! | gu-IN | Gujarati |
//! | kn-IN | Kannada |
//! | ml-IN | Malayalam |
//! | mr-IN | Marathi |
//! | od-IN | Odia |
//! | pa-IN | Punjabi |
//! | en-IN | English (Indian) |

mod config;
mod provider;

pub use config::{
    DEFAULT_MODEL, DEFAULT_SAMPLE_RATE, KEEPALIVE_INTERVAL_SECS, SARVAM_STT_WS_URL,
    SUPPORTED_LANGUAGES, SarvamSTTConfig,
};
pub use provider::SarvamSTT;
