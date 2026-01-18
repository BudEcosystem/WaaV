//! AmiVoice Speech-to-Text provider implementation.
//!
//! This module provides a real-time speech-to-text integration with the
//! AmiVoice Cloud Platform API from Advanced Media Inc. (Japan).
//!
//! # Features
//!
//! - Real-time streaming transcription via WebSocket
//! - Simple API key (APPKEY) authentication
//! - Support for 4 languages: Japanese, English, Chinese, Korean
//! - 19+ speech recognition engines (E2E and Hybrid)
//! - Domain-specific engines: medical, finance, insurance
//! - Interim (partial) and final transcription results
//! - Word-level timestamps and confidence scores
//! - Speaker diarization support
//! - Custom vocabulary registration (hybrid engines)
//! - Sentiment/emotion analysis
//! - No-logging mode for privacy
//!
//! # Speech Recognition Engines
//!
//! ## End-to-End (E2E) Engines
//!
//! Next-generation neural models with generally higher accuracy.
//! Do NOT support word registration.
//!
//! | Engine | Language | Type |
//! |--------|----------|------|
//! | `-a2-ja-general` | Japanese | Real-time |
//! | `-a2-zh-general` | Chinese | Real-time |
//! | `-a2-multi-general` | Multilingual | Real-time |
//! | `-a2b-ja-general` | Japanese | Batch |
//! | `-a2b-zh-general` | Chinese | Batch |
//! | `-a2b-multi-general` | Multilingual | Batch |
//!
//! ## Hybrid Engines (Conversation)
//!
//! Domain-optimized models with word registration support.
//!
//! | Engine | Language | Domain |
//! |--------|----------|--------|
//! | `-a-general` | Japanese | General (default) |
//! | `-a-medical` | Japanese | Medical |
//! | `-a-finance` | Japanese | Finance |
//! | `-a-insurance` | Japanese | Insurance |
//! | `-a-general-zh` | Chinese | General |
//! | `-a-general-en` | English | General |
//! | `-a-general-ko` | Korean | General |
//!
//! ## Hybrid Engines (Voice Input)
//!
//! | Engine | Language | Domain |
//! |--------|----------|--------|
//! | `-a-general-input` | Japanese | General dictation |
//! | `-a-medical-input` | Japanese | Medical dictation |
//! | `-a-finance-input` | Japanese | Finance dictation |
//! | `-a-name-input` | Japanese | Name recognition |
//! | `-a-address-input` | Japanese | Address recognition |
//!
//! # Configuration
//!
//! ## Environment Variables
//!
//! ```bash
//! export AMIVOICE_APPKEY="your-appkey"
//! ```
//!
//! ## WebSocket Configuration Message
//!
//! ```json
//! {
//!   "type": "config",
//!   "config": {
//!     "stt_provider": "amivoice",
//!     "model": "-a-general",
//!     "sample_rate": 16000
//!   }
//! }
//! ```
//!
//! # Example Usage
//!
//! ```rust,no_run
//! use waav_gateway::core::stt::{BaseSTT, STTConfig};
//! use waav_gateway::core::stt::amivoice::{AmiVoiceSTT, AmiVoiceEngine};
//! use std::sync::Arc;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let config = STTConfig {
//!         api_key: std::env::var("AMIVOICE_APPKEY")?,
//!         language: "ja".to_string(),
//!         sample_rate: 16000,
//!         model: "-a-general".to_string(),
//!         ..Default::default()
//!     };
//!
//!     let mut stt = AmiVoiceSTT::new(config)?;
//!
//!     // Use medical domain engine
//!     stt.set_engine(AmiVoiceEngine::HybridJapaneseMedical);
//!
//!     // Enable speaker diarization
//!     stt.set_diarization(true);
//!
//!     // Register transcription callback
//!     stt.on_result(Arc::new(|result| {
//!         Box::pin(async move {
//!             if result.is_final {
//!                 println!("Final: {} (confidence: {:.2})", result.transcript, result.confidence);
//!             } else {
//!                 println!("Interim: {}", result.transcript);
//!             }
//!         })
//!     })).await?;
//!
//!     // Connect to AmiVoice
//!     stt.connect().await?;
//!
//!     // Send audio data (PCM 16-bit mono at configured sample rate)
//!     let audio_data: Vec<u8> = vec![0u8; 3200]; // Example: 100ms of audio at 16kHz
//!     stt.send_audio(audio_data.into()).await?;
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
//! - **Encoding**: PCM 16-bit little-endian (Linear16)
//! - **Sample Rate**: 8kHz (telephony) or 16kHz (multimedia)
//! - **Channels**: Mono (1 channel)
//!
//! # WebSocket Protocol
//!
//! AmiVoice uses a proprietary text-based protocol:
//!
//! ## Commands (Client → Server)
//! - `s <format> <engine> [params]` - Start recognition session
//! - `p<binary_data>` - Send audio data
//! - `e` - End session
//!
//! ## Events (Server → Client)
//! - `s` / `s <error>` - Session start response
//! - `S <timestamp>` - Speech detected start
//! - `E <timestamp>` - Speech detected end
//! - `C` - Recognition processing started
//! - `U <json>` - Interim result
//! - `A <json>` - Final result
//! - `e` / `e <error>` - Session end response
//!
//! # References
//!
//! - [AmiVoice Cloud Platform](https://acp.amivoice.com/)
//! - [API Documentation](https://docs.amivoice.com/en/amivoice-api/manual/)
//! - [WebSocket Interface](https://docs.amivoice.com/en/amivoice-api/manual/websocket-interface/)
//! - [Speech Recognition Engines](https://docs.amivoice.com/en/amivoice-api/manual/engines/)
//! - [Client Libraries](https://github.com/advanced-media-inc/amivoice-api-client-library)

mod client;
pub mod config;
pub mod messages;

#[cfg(test)]
mod tests;

pub use client::AmiVoiceSTT;
pub use config::{
    AMIVOICE_HTTP_NOLOG_URL, AMIVOICE_HTTP_URL, AMIVOICE_WS_NOLOG_URL, AMIVOICE_WS_URL,
    AmiVoiceAudioFormat, AmiVoiceEngine, AmiVoiceSTTConfig, DEFAULT_ENGINE,
    DEFAULT_INACTIVITY_TIMEOUT, DEFAULT_RESULT_UPDATED_INTERVAL,
};
pub use messages::{
    AmiVoiceMessage, AmiVoiceRecognitionResult, AmiVoiceResult, AmiVoiceSentiment, AmiVoiceToken,
};
