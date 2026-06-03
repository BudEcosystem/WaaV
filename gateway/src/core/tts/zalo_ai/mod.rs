//! Zalo AI Text-to-Speech provider implementation.
//!
//! This module provides integration with VNG Corporation's Zalo AI
//! Text-to-Speech service for Vietnamese language synthesis.
//!
//! # Features
//!
//! - 4 Vietnamese voices (Northern and Southern accents)
//! - Adjustable speech speed (0.8 - 1.2)
//! - High-quality WAV audio output (16kHz, mono, 16-bit)
//! - REST API with two-step synthesis (URL then download)
//!
//! # Voices
//!
//! | ID | Name | Accent | Gender |
//! |----|------|--------|--------|
//! | 1 | Nữ miền Nam | Southern | Female |
//! | 2 | Nữ miền Bắc | Northern | Female |
//! | 3 | Nam miền Nam | Southern | Male |
//! | 4 | Nam miền Bắc | Northern | Male |
//!
//! # Configuration
//!
//! ## Environment Variables
//!
//! ```bash
//! export ZALO_API_KEY="your-zalo-api-key"
//! ```
//!
//! ## WebSocket Configuration Message
//!
//! ```json
//! {
//!   "type": "config",
//!   "config": {
//!     "tts_provider": "zalo_ai",
//!     "voice_id": "2",
//!     "extra": {
//!       "speed": 1.0
//!     }
//!   }
//! }
//! ```
//!
//! # Example Usage
//!
//! ```rust,no_run
//! use waav_gateway::core::tts::{BaseTTS, TTSConfig};
//! use waav_gateway::core::tts::zalo_ai::{ZaloTts, ZaloVoice};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let config = TTSConfig {
//!         api_key: std::env::var("ZALO_API_KEY")?,
//!         voice_id: "2".to_string(),  // Female Northern
//!         ..Default::default()
//!     };
//!
//!     let mut tts = ZaloTts::new(config)?;
//!
//!     // Connect to service
//!     tts.connect().await?;
//!
//!     // Synthesize speech
//!     tts.speak("Xin chào các bạn").await?;
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
//! - **Endpoint:** `POST https://api.zalo.ai/v1/tts/synthesize`
//! - **Authentication:** `apikey` header
//! - **Request:** URL-encoded form data (input, speaker_id, speed)
//! - **Response:** JSON with audio URL
//!
//! # References
//!
//! - [Zalo AI Website](https://zalo.ai)
//! - [Zalo AI Cloud](https://ai.zalo.cloud)
//! - [Python Library](https://github.com/iconclub/zalo-tts)

mod client;
pub mod config;

pub use client::ZaloTts;
pub use config::{
    AUDIO_CHANNELS, AUDIO_SAMPLE_RATE, AUDIO_SAMPLE_WIDTH, DEFAULT_ENCODE_TYPE, DEFAULT_SPEED,
    MAX_SPEED, MIN_SPEED, ZALO_TTS_ENDPOINT, ZaloTtsConfig, ZaloTtsData, ZaloTtsResponse, ZaloVoice,
};
