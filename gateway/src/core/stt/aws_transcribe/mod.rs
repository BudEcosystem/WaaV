//! Amazon Transcribe Streaming STT provider module.
//!
//! This module provides real-time speech-to-text transcription using
//! Amazon Transcribe Streaming API. It supports:
//!
//! - 100+ languages with automatic language detection
//! - Real-time streaming with low latency
//! - Partial results stabilization for live captions
//! - Speaker diarization (speaker identification)
//! - Custom vocabularies and language models
//! - Content redaction (PII masking)
//!
//! # Architecture
//!
//! The provider uses the AWS SDK for Rust to establish a bidirectional
//! streaming connection with Amazon Transcribe. Audio is sent as chunks
//! (50-200ms recommended) and transcription results are received as
//! they become available.
//!
//! # Authentication
//!
//! AWS credentials can be provided via:
//! 1. Environment variables: `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`
//! 2. AWS credentials file (`~/.aws/credentials`)
//! 3. IAM instance profiles (for EC2/ECS/Lambda)
//!
//! # Example
//!
//! ```rust,ignore
//! use waav_gateway::core::stt::{BaseSTT, STTConfig, create_stt_provider};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Create provider using factory
//!     let config = STTConfig {
//!         provider: "aws-transcribe".to_string(),
//!         api_key: String::new(), // Use AWS credentials from environment
//!         language: "en-US".to_string(),
//!         sample_rate: 16000,
//!         channels: 1,
//!         punctuation: true,
//!         encoding: "pcm".to_string(),
//!         model: String::new(),
//!     };
//!
//!     let mut stt = create_stt_provider("aws-transcribe", config)?;
//!     stt.connect().await?;
//!
//!     // Register result callback
//!     let callback = std::sync::Arc::new(|result: waav_gateway::core::stt::STTResult| {
//!         Box::pin(async move {
//!             println!("Transcript: {} (final: {})", result.transcript, result.is_final);
//!         }) as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
//!     });
//!     stt.on_result(callback).await?;
//!
//!     // Send audio chunks...
//!     // let audio = bytes::Bytes::from(audio_buffer);
//!     // stt.send_audio(audio).await?;
//!
//!     stt.disconnect().await?;
//!     Ok(())
//! }
//! ```
//!
//! # Best Practices
//!
//! 1. **Sample Rate**: Use 16kHz for best accuracy vs bandwidth trade-off
//! 2. **Chunk Size**: Send 50-200ms chunks for optimal latency
//! 3. **Encoding**: Use PCM for lowest latency, FLAC for compression
//! 4. **Partial Results**: Enable stabilization for live captions
//! 5. **Error Handling**: Register error callbacks to handle stream errors
//!
//! # Limitations
//!
//! - Maximum session duration: 4 hours
//! - One stream per HTTP/2 session
//! - PCM audio must be 16-bit signed little-endian

mod client;
mod config;
mod messages;

#[cfg(test)]
mod tests;

pub use client::AwsTranscribeSTT;
pub use config::{
    AwsRegion, AwsTranscribeSTTConfig, ContentRedactionType, DEFAULT_CHUNK_DURATION_MS,
    MAX_SAMPLE_RATE, MIN_SAMPLE_RATE, MediaEncoding, PartialResultsStability,
    RECOMMENDED_SAMPLE_RATE, VocabularyFilterMethod,
};
pub use messages::{
    Alternative, Entity, Item, LanguageWithScore, Result as TranscribeResult, TranscribeError,
    Transcript, TranscriptEvent,
};
