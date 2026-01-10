//! Voice Activity Detection (VAD) module
//!
//! This module provides acoustic-level speech detection that complements
//! the text-based turn detection. VAD analyzes raw audio to determine
//! if there is human speech, enabling:
//!
//! - Reduced STT costs by only sending speech audio
//! - Lower latency by detecting speech start immediately
//! - Better noise handling with ML-based filtering
//! - Accurate end-of-speech detection
//!
//! # Feature Flag
//!
//! This module requires the `vad` feature to be enabled for full functionality.
//! When disabled, a no-op stub implementation is used.
//!
//! # Example
//!
//! ```rust,ignore
//! use waav_gateway::core::vad::{SileroVAD, VADConfig, VoiceActivityDetector};
//!
//! let config = VADConfig::default();
//! let mut vad = SileroVAD::new(config).await?;
//!
//! // Process audio frames (512 samples at 16kHz = 32ms)
//! let result = vad.process_frame(&audio_samples).await?;
//!
//! if result.speech_start {
//!     println!("Speech started!");
//! }
//! if result.speech_end {
//!     println!("Speech ended!");
//! }
//! ```

#[cfg(feature = "vad")]
pub mod assets;
pub mod config;
#[cfg(feature = "vad")]
pub mod detector;
#[cfg(feature = "vad")]
pub mod model_manager;

#[cfg(not(feature = "vad"))]
mod stub;

pub use config::{VADConfig, VADBackend};

#[cfg(feature = "vad")]
pub use detector::{SileroVAD, VADResult, VoiceActivityDetector};

#[cfg(not(feature = "vad"))]
pub use stub::{SileroVAD, VADResult, VoiceActivityDetector, create_vad};

#[cfg(feature = "vad")]
use anyhow::Result;

/// Create a VAD instance with the given configuration
#[cfg(feature = "vad")]
pub async fn create_vad(config: VADConfig) -> Result<SileroVAD> {
    SileroVAD::new(config).await
}
