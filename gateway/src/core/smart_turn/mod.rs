//! Smart Turn detection module.
//!
//! This module provides audio-based semantic turn detection using a Whisper encoder
//! fine-tuned for detecting when a user has finished speaking (vs. a pause).
//!
//! ## Architecture
//!
//! The Smart Turn system consists of:
//! - `MelExtractor`: Extracts Whisper-compatible mel spectrograms from audio
//! - `SmartTurnDetector`: ONNX model inference for turn probability
//! - Integration with VoiceManager for real-time turn detection
//!
//! ## Features
//!
//! - **Whisper-compatible**: Uses the same mel spectrogram format as OpenAI Whisper
//! - **Low latency**: <15ms inference time target
//! - **Streaming**: Processes audio in real-time using ring buffers
//!
//! ## Usage
//!
//! ```rust,ignore
//! use waav_gateway::core::smart_turn::{
//!     MelExtractor, MelExtractorConfig,
//!     SmartTurnDetector, SmartTurnDetectorConfig,
//! };
//!
//! // Create mel extractor for audio preprocessing
//! let mel_config = MelExtractorConfig::default();
//! let mut mel_extractor = MelExtractor::new(mel_config)?;
//!
//! // Create detector for turn prediction
//! let detector_config = SmartTurnDetectorConfig::default();
//! let mut detector = SmartTurnDetector::new(detector_config).await?;
//!
//! // Process audio and detect turns
//! mel_extractor.process(&audio_samples)?;
//! let result = detector.predict(mel_extractor.get_mel_frames()).await?;
//!
//! if result.is_turn_complete {
//!     println!("Turn complete with probability: {}", result.probability);
//! }
//! ```

#[cfg(feature = "smart-turn")]
mod whisper_mel;

#[cfg(feature = "smart-turn")]
mod mel_extractor;

#[cfg(feature = "smart-turn")]
mod detector;

// Processor requires either silero-vad or smart-turn
#[cfg(any(feature = "silero-vad", feature = "smart-turn"))]
mod processor;

#[cfg(feature = "smart-turn")]
pub use mel_extractor::{
    MelExtractor, MelExtractorConfig, WHISPER_HOP_LENGTH, WHISPER_MAX_DURATION_SECS,
    WHISPER_MAX_FRAMES, WHISPER_N_FFT, WHISPER_N_MELS, WHISPER_SAMPLE_RATE,
};

#[cfg(feature = "smart-turn")]
pub use detector::{
    SMART_TURN_MAX_FRAMES, SmartTurnDetector, SmartTurnDetectorBuilder, SmartTurnDetectorConfig,
    SmartTurnResult,
};

#[cfg(any(feature = "silero-vad", feature = "smart-turn"))]
pub use processor::{
    SileroVADConfigWrapper, SmartTurnProcessResult, SmartTurnProcessor, SmartTurnProcessorConfig,
};

// Stub when smart-turn feature is disabled
#[cfg(not(feature = "smart-turn"))]
mod stub;
#[cfg(not(feature = "smart-turn"))]
pub use stub::*;
