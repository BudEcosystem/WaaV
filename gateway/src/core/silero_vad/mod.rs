//! Silero VAD: Local neural network voice activity detection.
//!
//! This module provides a Rust wrapper around the Silero VAD v5 ONNX model
//! for real-time voice activity detection. It is optimized for low-latency
//! inference in the audio processing pipeline.
//!
//! # Architecture
//!
//! The Silero VAD model is a lightweight LSTM-based neural network that:
//! - Takes 512 audio samples (32ms at 16kHz) as input
//! - Outputs a speech probability [0.0, 1.0]
//! - Maintains internal state (h, c) across frames
//!
//! # Usage
//!
//! ```ignore
//! use waav_gateway::core::silero_vad::{SileroVAD, SileroVADConfig};
//!
//! let config = SileroVADConfig::default();
//! let mut vad = SileroVAD::new(config).await?;
//!
//! // Process 512-sample chunks
//! let audio_chunk: Vec<f32> = vec![0.0; 512];
//! let probability = vad.process(&audio_chunk)?;
//!
//! if probability > 0.5 {
//!     println!("Speech detected!");
//! }
//! ```
//!
//! # Performance
//!
//! - Model size: ~2MB
//! - Inference time: < 1ms per chunk on CPU
//! - Memory: ~5MB total with model loaded

#[cfg(feature = "silero-vad")]
mod config;
#[cfg(feature = "silero-vad")]
mod detector;

#[cfg(feature = "silero-vad")]
pub use config::SileroVADConfig;
#[cfg(feature = "silero-vad")]
pub use detector::{SileroVAD, SileroVADResult};

// Stub implementations when feature is disabled
#[cfg(not(feature = "silero-vad"))]
mod stub;
#[cfg(not(feature = "silero-vad"))]
pub use stub::*;
