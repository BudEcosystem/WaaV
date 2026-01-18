//! FPT.AI Speech-to-Text provider implementation.
//!
//! This module provides integration with FPT Corporation's FPT.AI
//! Speech-to-Text service for Vietnamese language recognition.
//!
//! # Features
//!
//! - Vietnamese language speech recognition
//! - HTTP-based file upload (non-streaming)
//! - Support for WAV audio input
//! - Simple API key authentication
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
//!     "stt_provider": "fpt-ai",
//!     "sample_rate": 16000,
//!     "language": "vi"
//!   }
//! }
//! ```
//!
//! # Example Usage
//!
//! ```rust,no_run
//! use waav_gateway::core::stt::{BaseSTT, STTConfig};
//! use waav_gateway::core::stt::fpt_ai::FptStt;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let config = STTConfig {
//!         api_key: std::env::var("FPT_AI_API_KEY")?,
//!         language: "vi".to_string(),
//!         sample_rate: 16000,
//!         ..Default::default()
//!     };
//!
//!     let mut stt = FptStt::new(config)?;
//!
//!     // Connect to service
//!     stt.connect().await?;
//!
//!     // Buffer audio and flush to get transcription
//!     // (FPT.AI uses HTTP file upload, not streaming)
//!
//!     // Disconnect when done
//!     stt.disconnect().await?;
//!
//!     Ok(())
//! }
//! ```
//!
//! # Audio Format Requirements
//!
//! - **Encoding**: WAV (PCM)
//! - **Sample Rate**: 8kHz or 16kHz
//! - **Channels**: Mono (1 channel)
//!
//! # API Details
//!
//! - **Endpoint:** `POST https://api.fpt.ai/hmi/asr/general`
//! - **Authentication:** `api_key` header
//! - **Request:** Raw audio file data
//! - **Response:** JSON with transcription
//!
//! # References
//!
//! - [FPT.AI Website](https://fpt.ai)
//! - [FPT.AI STT](https://fpt.ai/stt)
//! - [API Documentation](https://docs.fpt.ai/docs/en/speech/api/speech-to-text/)
//! - [Console](https://console.fpt.ai)

mod client;
pub mod config;

#[cfg(test)]
mod tests;

pub use client::FptStt;
pub use config::{
    FptSttConfig, FptSttResponse, FptSttHypothesis, FPT_STT_ENDPOINT,
    DEFAULT_REQUEST_TIMEOUT, MIN_AUDIO_DURATION_MS, MAX_AUDIO_DURATION_MS,
};
