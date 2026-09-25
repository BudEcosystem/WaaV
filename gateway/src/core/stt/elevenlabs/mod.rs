//! ElevenLabs Speech-to-Text integration — BOTH of its transports.
//!
//! ElevenLabs serves speech-to-text two ways, over disjoint model vocabularies, and each rejects
//! the other's model ids:
//!
//! | model id | transport | wire |
//! |---|---|---|
//! | `scribe_v2`, `scribe_v2_medical` | batch | `POST /v1/speech-to-text` ([`batch`]) |
//! | `scribe_v2_realtime` | realtime | WebSocket ([`client`]) |
//!
//! Which one runs is decided by the CALLER, not by the config: a prerecorded upload goes through
//! `create_stt_standard_prerecorded`, a live session through `create_stt_standard`. See
//! [`batch::model_is_realtime`].
//!
//! The realtime client below supports:
//!
//! - Real-time streaming transcription
//! - Multiple regional endpoints (US, EU, India)
//! - VAD-based automatic or manual commit strategies
//! - Word-level timestamps
//! - Multiple audio format support (PCM, μ-law)
//!
//! # Architecture
//!
//! The module is organized into focused submodules:
//!
//! - [`config`]: Configuration types (`ElevenLabsSTTConfig`, `ElevenLabsAudioFormat`, etc.)
//! - [`messages`]: WebSocket message types for API communication
//! - [`client`]: The main `ElevenLabsSTT` client implementation
//! - [`batch`]: The prerecorded `POST /v1/speech-to-text` client and its config
//!
//! # Example
//!
//! ```rust,no_run
//! use waav_gateway::core::stt::{BaseSTT, STTConfig, ElevenLabsSTT};
//! use std::sync::Arc;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let config = STTConfig {
//!         api_key: "your-elevenlabs-api-key".to_string(),
//!         language: "en".to_string(),
//!         sample_rate: 16000,
//!         ..Default::default()
//!     };
//!
//!     let mut stt = ElevenLabsSTT::new(config)?;
//!     stt.connect().await?;
//!
//!     // Register callback for results
//!     stt.on_result(Arc::new(|result| {
//!         Box::pin(async move {
//!             println!("Transcription: {}", result.transcript);
//!         })
//!     })).await?;
//!
//!     // Send audio data
//!     let audio_data = vec![0u8; 1024];
//!     stt.send_audio(audio_data.into()).await?;
//!
//!     Ok(())
//! }
//! ```

pub mod batch;
mod client;
mod config;
mod messages;

#[cfg(test)]
mod tests;

// Re-export public types
pub use batch::{
    DEFAULT_BATCH_MODEL, ElevenLabsBatchConfig, EntityRedactionMode, MAX_BATCH_KEYTERMS,
    TimestampsGranularity, model_is_realtime,
};
pub use client::ElevenLabsSTT;
pub use config::{
    CommitStrategy, ElevenLabsAudioFormat, ElevenLabsRegion, ElevenLabsSTTConfig, REALTIME_MODELS,
};
pub use messages::{
    CommittedTranscript, CommittedTranscriptWithTimestamps, ElevenLabsMessage, ElevenLabsSTTError,
    EndOfStream, InputAudioChunk, PartialTranscript, SessionStarted, WordTiming,
};
