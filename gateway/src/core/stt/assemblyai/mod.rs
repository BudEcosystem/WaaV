//! AssemblyAI Speech-to-Text Streaming API v3 integration.
//!
//! This module provides a streaming STT client for the AssemblyAI Streaming
//! API v3 with support for:
//!
//! - Real-time streaming transcription
//! - Immutable transcripts (transcripts are never modified after delivery)
//! - End-of-turn detection for speech boundaries
//! - Multiple regional endpoints (US, EU)
//! - Binary audio streaming (no base64 encoding overhead)
//! - Word-level timestamps
//! - Multilingual support with automatic language detection
//!
//! # Architecture
//!
//! The module is organized into focused submodules:
//!
//! - [`config`]: Configuration types (`AssemblyAISTTConfig`, `AssemblyAIEncoding`, etc.)
//! - [`messages`]: WebSocket message types for API communication
//! - [`client`]: The main `AssemblyAISTT` client implementation
//!
//! # Key Differentiators
//!
//! AssemblyAI's v3 API is unique in providing **immutable transcripts**:
//!
//! - When `format_turns=true`, transcripts are delivered in "turns"
//! - Once a turn is delivered, it will never be modified
//! - This contrasts with other providers where interim results may change
//!
//! # Example
//!
//! ```rust,no_run
//! use waav_gateway::core::stt::{BaseSTT, STTConfig, AssemblyAISTT};
//! use std::sync::Arc;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let config = STTConfig {
//!         api_key: "your-assemblyai-api-key".to_string(),
//!         language: "en".to_string(),
//!         sample_rate: 16000,
//!         ..Default::default()
//!     };
//!
//!     let mut stt = AssemblyAISTT::new(config)?;
//!     stt.connect().await?;
//!
//!     // Register callback for results
//!     stt.on_result(Arc::new(|result| {
//!         Box::pin(async move {
//!             println!("Transcription: {}", result.transcript);
//!             if result.is_final {
//!                 println!("(End of turn)");
//!             }
//!         })
//!     })).await?;
//!
//!     // Send audio data (raw PCM, no base64 encoding needed)
//!     let audio_data = vec![0u8; 1024];
//!     stt.send_audio(audio_data.into()).await?;
//!
//!     Ok(())
//! }
//! ```
//!
//! # Supported Audio Formats
//!
//! AssemblyAI Streaming API v3 supports:
//!
//! | Encoding | Sample Rates | Use Case |
//! |----------|--------------|----------|
//! | `pcm_s16le` | Any | Standard PCM audio (default) |
//! | `pcm_mulaw` | 8000 | Telephony, SIP |
//!
//! # Speech Models
//!
//! | Model | Languages | Use Case |
//! |-------|-----------|----------|
//! | `universal-streaming-english` | English only | Best for English-only apps |
//! | `universal-streaming-multilingual` | 99+ languages | Multilingual support with auto-detection |

mod client;
mod config;
mod messages;

#[cfg(test)]
mod tests;

// Re-export public types
pub use client::AssemblyAISTT;
pub use config::{
    AssemblyAIEncoding, AssemblyAIRegion, AssemblyAISTTConfig, AssemblyAISpeechModel,
};
pub use messages::{
    AssemblyAIMessage, BeginMessage, ErrorMessage, ForceEndpointMessage, TerminateMessage,
    TerminationMessage, TurnMessage, UpdateConfigurationMessage, Word,
};
