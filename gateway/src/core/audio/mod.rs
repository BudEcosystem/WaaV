//! Audio processing utilities for voice activity detection and analysis.
//!
//! This module provides:
//! - [`VADAnalyzer`] - Voice Activity Detection state machine with configurable debouncing
//! - [`VADParams`] - Configuration for VAD behavior
//! - [`VADState`] - Current state of the VAD analyzer
//! - [`AudioRingBuffer`] - Pre-allocated ring buffer for audio samples

pub mod ring_buffer;
pub mod vad;

pub use ring_buffer::AudioRingBuffer;
pub use vad::{VADAnalyzer, VADParams, VADState, VADTransition};
