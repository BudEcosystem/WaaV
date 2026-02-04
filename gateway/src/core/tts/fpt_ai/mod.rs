//! FPT.AI Text-to-Speech provider implementation.
//!
//! This module provides integration with FPT Corporation's FPT.AI
//! Text-to-Speech service for Vietnamese language synthesis.
//!
//! # Features
//!
//! - 7 Vietnamese voices (Northern accents, various speakers)
//! - Adjustable speech speed (-3 to +3)
//! - MP3 or WAV audio output
//! - REST API with two-step async synthesis (URL then download)
//!
//! # Voices
//!
//! | ID | Name | Accent | Gender |
//! |----|------|--------|--------|
//! | banmai | Ban Mai | Northern | Female |
//! | lannhi | Lan Nhi | Northern | Female |
//! | leminh | Le Minh | Northern | Male |
//! | myan | My An | - | Female |
//! | thuminh | Thu Minh | - | Female |
//! | giahuy | Gia Huy | - | Male |
//! | linhsan | Linh San | - | Female |
//!
//! # Configuration
//!
//! ## Environment Variables
//!
//! ```bash
//! export FPT_AI_API_KEY="your-fpt-ai-api-key"
//! ```
//!
//! ## WebSocket Configuration Message
//!
//! ```json
//! {
//!   "type": "config",
//!   "config": {
//!     "tts_provider": "fpt-ai",
//!     "voice_id": "banmai",
//!     "extra": {
//!       "speed": 0
//!     }
//!   }
//! }
//! ```
//!
//! # Example Usage
//!
//! ```rust,no_run
//! use waav_gateway::core::tts::{BaseTTS, TTSConfig};
//! use waav_gateway::core::tts::fpt_ai::{FptTts, FptVoice};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let config = TTSConfig {
//!         api_key: std::env::var("FPT_AI_API_KEY")?,
//!         voice_id: Some("banmai".to_string()),
//!         ..Default::default()
//!     };
//!
//!     let mut tts = FptTts::new(config)?;
//!
//!     // Connect to service
//!     tts.connect().await?;
//!
//!     // Synthesize speech
//!     tts.speak("Xin chào các bạn", false).await?;
//!
//!     // Disconnect when done
//!     tts.disconnect().await?;
//!
//!     Ok(())
//! }
//! ```
//!
//! # API Details
//!
//! - **Endpoint:** `POST https://api.fpt.ai/hmi/tts/v5`
//! - **Authentication:** `api_key` header
//! - **Request:** Plain text body (UTF-8)
//! - **Response:** JSON with async audio URL
//!
//! # References
//!
//! - [FPT.AI Website](https://fpt.ai)
//! - [FPT.AI TTS](https://fpt.ai/tts)
//! - [API Documentation](https://docs.fpt.ai/docs/en/speech/api/text-to-speech/)
//! - [Console](https://console.fpt.ai)

mod client;
pub mod config;

pub use client::FptTts;
pub use config::{
    AUDIO_SAMPLE_RATE, DEFAULT_AUDIO_FORMAT, DEFAULT_REQUEST_TIMEOUT, DEFAULT_SPEED,
    FPT_TTS_ENDPOINT, FptAudioFormat, FptTtsConfig, FptTtsResponse, FptVoice, MAX_SPEED,
    MAX_TEXT_LENGTH, MIN_SPEED, MIN_TEXT_LENGTH,
};
