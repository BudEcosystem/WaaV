//! Audio processing utilities for voice activity detection and analysis.
//!
//! This module provides:
//! - [`VADAnalyzer`] - Voice Activity Detection state machine with configurable debouncing
//! - [`VADParams`] - Configuration for VAD behavior
//! - [`VADState`] - Current state of the VAD analyzer

pub mod vad;

pub use vad::{VADAnalyzer, VADParams, VADState, VADTransition};
