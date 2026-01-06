//! IBM Watson Speech-to-Text provider implementation.
//!
//! This module provides a real-time speech-to-text integration with IBM Watson
//! Speech-to-Text API using WebSocket streaming.
//!
//! # Features
//!
//! - Real-time streaming transcription via WebSocket
//! - IAM token-based authentication with automatic refresh
//! - Support for 25+ languages across multimedia and telephony models
//! - Interim (partial) and final transcription results
//! - Word-level timestamps and confidence scores
//! - Speaker diarization (speaker labels)
//! - Smart formatting (dates, times, numbers, etc.)
//! - Profanity filtering and PII redaction
//! - Custom language and acoustic models
//! - Background audio suppression
//! - Low latency mode for faster responses
//!
//! # IBM Watson Regions
//!
//! IBM Watson Speech-to-Text is available in the following regions:
//!
//! | Region | Location |
//! |--------|----------|
//! | `us-south` | Dallas, Texas (Default) |
//! | `us-east` | Washington, D.C. |
//! | `eu-de` | Frankfurt, Germany |
//! | `eu-gb` | London, UK |
//! | `au-syd` | Sydney, Australia |
//! | `jp-tok` | Tokyo, Japan |
//! | `kr-seo` | Seoul, South Korea |
//!
//! # Available Models
//!
//! IBM Watson provides two model types optimized for different audio sources:
//!
//! - **Multimedia models** (`*_Multimedia`): Optimized for high-quality audio (16kHz+)
//! - **Telephony models** (`*_Telephony`): Optimized for telephone audio (8kHz)
//!
//! Supported languages include: English (US, GB, AU), Spanish, French, German,
//! Italian, Portuguese, Japanese, Korean, Chinese, Dutch, Arabic, Hindi, and more.
//!
//! # Configuration
//!
//! ## Environment Variables
//!
//! ```bash
//! export IBM_WATSON_API_KEY="your-api-key"
//! export IBM_WATSON_INSTANCE_ID="your-instance-id"
//! export IBM_WATSON_REGION="us-south"  # Optional, defaults to us-south
//! ```
//!
//! ## WebSocket Configuration Message
//!
//! ```json
//! {
//!   "type": "config",
//!   "config": {
//!     "stt_provider": "ibm-watson",
//!     "ibm_watson_instance_id": "your-instance-id",
//!     "ibm_watson_region": "us-south",
//!     "ibm_watson_model": "en-US_Multimedia"
//!   }
//! }
//! ```
//!
//! # Example Usage
//!
//! ```rust,no_run
//! use waav_gateway::core::stt::{BaseSTT, STTConfig};
//! use waav_gateway::core::stt::ibm_watson::{IbmWatsonSTT, IbmRegion, IbmModel};
//! use std::sync::Arc;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Create base configuration
//!     let config = STTConfig {
//!         api_key: std::env::var("IBM_WATSON_API_KEY")?,
//!         language: "en-US".to_string(),
//!         sample_rate: 16000,
//!         ..Default::default()
//!     };
//!
//!     // Create IBM Watson STT instance
//!     let mut stt = IbmWatsonSTT::new(config)?;
//!
//!     // Configure IBM-specific settings
//!     stt.set_instance_id(std::env::var("IBM_WATSON_INSTANCE_ID")?);
//!     stt.set_region(IbmRegion::UsSouth);
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
//!     // Register error callback
//!     stt.on_error(Arc::new(|error| {
//!         Box::pin(async move {
//!             eprintln!("Error: {}", error);
//!         })
//!     })).await?;
//!
//!     // Connect to IBM Watson
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
//! # Advanced Features
//!
//! ## Custom Language Models
//!
//! ```rust,ignore
//! // Use a custom language model trained on your domain vocabulary
//! stt.set_language_model(Some("custom-model-id".to_string())).await?;
//! ```
//!
//! ## Background Audio Suppression
//!
//! ```rust,ignore
//! // Suppress background noise (0.0 = none, 1.0 = maximum)
//! stt.set_background_suppression(0.5).await?;
//! ```
//!
//! ## Low Latency Mode
//!
//! ```rust,ignore
//! // Enable low latency for faster interim results (may reduce accuracy)
//! stt.set_low_latency(true).await?;
//! ```
//!
//! # Audio Format Requirements
//!
//! - **Encoding**: PCM 16-bit little-endian (Linear16), mu-law, A-law, FLAC, Opus, MP3
//! - **Sample Rate**: 8kHz for telephony models, 16kHz for multimedia models
//! - **Channels**: Mono (1 channel) recommended
//!
//! # References
//!
//! - [IBM Watson STT Documentation](https://cloud.ibm.com/docs/speech-to-text)
//! - [API Reference](https://cloud.ibm.com/apidocs/speech-to-text)
//! - [WebSocket Interface](https://cloud.ibm.com/docs/speech-to-text?topic=speech-to-text-websockets)

mod client;
pub mod config;
pub mod messages;

#[cfg(test)]
mod tests;

pub use client::IbmWatsonSTT;
pub use config::{
    IbmAudioEncoding, IbmModel, IbmRegion, IbmWatsonSTTConfig, DEFAULT_INACTIVITY_TIMEOUT,
    DEFAULT_MODEL, IBM_IAM_URL,
};

/// Default IBM Watson STT WebSocket URL template.
/// The actual URL is constructed using region and instance ID.
pub const IBM_WATSON_STT_URL: &str = "wss://api.us-south.speech-to-text.watson.cloud.ibm.com";
