//! Audio processing utilities for voice activity detection and analysis.
//!
//! This module provides:
//! - [`VADAnalyzer`] - Voice Activity Detection state machine with configurable debouncing
//! - [`VADParams`] - Configuration for VAD behavior
//! - [`VADState`] - Current state of the VAD analyzer
//! - [`AudioRingBuffer`] - Pre-allocated ring buffer for audio samples

pub mod ring_buffer;
// The single streaming resampler (C-G5) — available exactly when `rubato`
// is pulled in (same gating pattern as `core::onnx`).
#[cfg(any(feature = "silero-vad", feature = "smart-turn", feature = "noise-filter"))]
pub mod resampler;
pub mod vad;

pub use ring_buffer::AudioRingBuffer;
#[cfg(any(feature = "silero-vad", feature = "smart-turn", feature = "noise-filter"))]
pub use resampler::{StreamResampler, egress_to_client_rate, resample_pcm16};
pub use vad::{VADAnalyzer, VADParams, VADState, VADTransition};
