//! OpenAI Speech-to-Text (Whisper) API integration.
//!
//! This module provides a STT client for the OpenAI Audio Transcription API
//! (powered by Whisper) with support for:
//!
//! - Multiple transcription models (whisper-1, gpt-4o-transcribe, gpt-4o-mini-transcribe)
//! - Various response formats (json, verbose_json, text, srt, vtt)
//! - Word-level and segment-level timestamps
//! - Temperature control for output variability
//! - Multiple audio input formats (WAV, MP3, etc.)
//!
//! # Architecture
//!
//! Unlike WebSocket-based providers (Deepgram, ElevenLabs), OpenAI Whisper is a
//! REST-based batch API. This module buffers audio data and sends it to the API
//! when the connection is closed or a configured threshold is reached.
//!
//! The module is organized into focused submodules:
//!
//! - [`config`]: Configuration types (`OpenAISTTConfig`, `OpenAISTTModel`, etc.)
//! - [`messages`]: Request/response types for the transcription API
//! - [`client`]: The main `OpenAISTT` client implementation
//!
//! # Example
//!
//! ```rust,no_run
//! use waav_gateway::core::stt::{BaseSTT, STTConfig, OpenAISTT, STTResult};
//! use std::sync::Arc;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let config = STTConfig {
//!         api_key: "sk-your-openai-api-key".to_string(),
//!         language: "en".to_string(),
//!         sample_rate: 16000,
//!         model: "whisper-1".to_string(),
//!         ..Default::default()
//!     };
//!
//!     let mut stt = OpenAISTT::new(config)?;
//!     stt.connect().await?;
//!
//!     // Register callback for results
//!     stt.on_result(Arc::new(|result: STTResult| {
//!         Box::pin(async move {
//!             println!("Transcription: {}", result.transcript);
//!             println!("Confidence: {:.2}", result.confidence);
//!         })
//!     })).await?;
//!
//!     // Send audio data (buffered until disconnect)
//!     let audio_data = vec![0u8; 16000 * 2]; // 1 second of 16kHz 16-bit audio
//!     stt.send_audio(audio_data.into()).await?;
//!
//!     // Disconnect triggers transcription API call
//!     stt.disconnect().await?;
//!
//!     Ok(())
//! }
//! ```
//!
//! # Using Provider-Specific Configuration
//!
//! For more control over OpenAI-specific features:
//!
//! ```rust,no_run
//! use waav_gateway::core::stt::{STTConfig, OpenAISTT};
//! use waav_gateway::core::stt::openai::{
//!     OpenAISTTConfig, OpenAISTTModel, ResponseFormat, TimestampGranularity
//! };
//!
//! let config = OpenAISTTConfig {
//!     base: STTConfig {
//!         api_key: "sk-your-openai-api-key".to_string(),
//!         language: "en".to_string(),
//!         sample_rate: 16000,
//!         ..Default::default()
//!     },
//!     model: OpenAISTTModel::Gpt4oTranscribe,
//!     response_format: ResponseFormat::VerboseJson,
//!     timestamp_granularities: vec![
//!         TimestampGranularity::Word,
//!         TimestampGranularity::Segment,
//!     ],
//!     temperature: Some(0.0),
//!     ..Default::default()
//! };
//!
//! let stt = OpenAISTT::with_config(config).unwrap();
//! ```
//!
//! # API Reference
//!
//! - API Endpoint: `POST https://api.openai.com/v1/audio/transcriptions`
//! - Max file size: 25MB
//! - Supported formats: mp3, mp4, mpeg, mpga, m4a, wav, webm
//! - Documentation: <https://platform.openai.com/docs/api-reference/audio/createTranscription>

mod client;
mod config;
mod messages;

#[cfg(test)]
mod tests;

// Re-export public types
pub use client::OpenAISTT;
pub use config::{
    AudioInputFormat, FlushStrategy, OpenAISTTConfig, OpenAISTTModel, ResponseFormat,
    TimestampGranularity,
};
pub use messages::{
    OpenAIError, OpenAIErrorResponse, TranscriptionResponse, TranscriptionResult,
    TranscriptionSegment, TranscriptionWord, VerboseTranscriptionResponse, wav,
};
