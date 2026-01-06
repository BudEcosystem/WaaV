//! Groq Speech-to-Text (Whisper) provider implementation.
//!
//! This module provides real-time speech-to-text integration with Groq's
//! Whisper API using HTTP REST calls.
//!
//! # Features
//!
//! - Ultra-fast transcription (216x real-time with whisper-large-v3-turbo)
//! - OpenAI-compatible API format
//! - Multiple Whisper models (large-v3, large-v3-turbo)
//! - Word and segment-level timestamps
//! - Translation to English
//! - Silence detection for automatic flushing
//! - Automatic retry with exponential backoff
//!
//! # Models
//!
//! | Model | WER | Speed | Cost/Hour |
//! |-------|-----|-------|-----------|
//! | `whisper-large-v3` | 10.3% | 189x | $0.111 |
//! | `whisper-large-v3-turbo` | 12% | 216x | $0.04 |
//!
//! # Model Selection Guide
//!
//! - Use `whisper-large-v3` for error-sensitive applications requiring best accuracy
//! - Use `whisper-large-v3-turbo` for best price/performance with multilingual support
//!
//! # File Limits
//!
//! - Free tier: 25MB max
//! - Dev tier: 100MB max
//! - Supported formats: FLAC, MP3, MP4, MPEG, MPGA, M4A, OGG, WAV, WebM
//!
//! # Configuration
//!
//! ## Environment Variables
//!
//! ```bash
//! export GROQ_API_KEY="gsk_..."
//! ```
//!
//! ## WebSocket Configuration Message
//!
//! ```json
//! {
//!   "type": "config",
//!   "config": {
//!     "stt_provider": "groq",
//!     "groq_model": "whisper-large-v3-turbo",
//!     "language": "en"
//!   }
//! }
//! ```
//!
//! # Example Usage
//!
//! ```rust,ignore
//! use waav_gateway::core::stt::{BaseSTT, STTConfig, STTResult};
//! use waav_gateway::core::stt::groq::{GroqSTT, GroqSTTModel};
//! use std::sync::Arc;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Create base configuration
//!     let config = STTConfig {
//!         api_key: std::env::var("GROQ_API_KEY")?,
//!         language: "en".to_string(),
//!         sample_rate: 16000,
//!         channels: 1,
//!         model: "whisper-large-v3-turbo".to_string(),
//!         ..Default::default()
//!     };
//!
//!     // Create Groq STT instance
//!     let mut stt = GroqSTT::new(config)?;
//!     stt.connect().await?;
//!
//!     // Register callback for transcription results
//!     stt.on_result(Arc::new(|result: STTResult| {
//!         Box::pin(async move {
//!             println!("Transcript: {}", result.transcript);
//!             println!("Confidence: {:.2}", result.confidence);
//!         })
//!     })).await?;
//!
//!     // Send audio data (buffered until disconnect or flush)
//!     let audio_data = vec![0u8; 32000]; // 1 second at 16kHz, 16-bit mono
//!     stt.send_audio(audio_data.into()).await?;
//!
//!     // Disconnect triggers transcription
//!     stt.disconnect().await?;
//!
//!     Ok(())
//! }
//! ```
//!
//! # Rate Limits
//!
//! - Rate limits apply at the organization level
//! - 429 errors include retry-after header
//! - Automatic retry with exponential backoff is recommended
//! - Consider using the Batch API for high-volume workloads
//!
//! # Audio Preprocessing
//!
//! Groq's Whisper models downsample audio to 16kHz mono before processing.
//! For optimal results and reduced upload sizes, preprocess audio to:
//!
//! ```bash
//! ffmpeg -i input.wav -ar 16000 -ac 1 -c:a flac output.flac
//! ```
//!
//! # Translation
//!
//! The translation endpoint (`/v1/audio/translations`) transcribes audio
//! and translates to English regardless of the input language.
//!
//! # References
//!
//! - [Groq Speech-to-Text Documentation](https://console.groq.com/docs/speech-to-text)
//! - [Groq API Reference](https://console.groq.com/docs/api-reference)
//! - [Whisper Large V3 Model](https://console.groq.com/docs/model/whisper-large-v3)
//! - [Whisper Large V3 Turbo Model](https://console.groq.com/docs/model/whisper-large-v3-turbo)

mod client;
pub mod config;
pub mod messages;

#[cfg(test)]
mod tests;

pub use client::GroqSTT;
pub use config::{
    AudioInputFormat, FlushStrategy, GroqResponseFormat, GroqSTTConfig, GroqSTTModel,
    SilenceDetectionConfig, TimestampGranularity, DEFAULT_MAX_FILE_SIZE, DEV_TIER_MAX_FILE_SIZE,
    GROQ_STT_URL, GROQ_TRANSLATION_URL, MAX_PROMPT_TOKENS,
};
pub use messages::{
    GroqError, GroqErrorResponse, GroqMetadata, Segment, TranscriptionResponse,
    TranscriptionResult, VerboseTranscriptionResponse, Word,
};
