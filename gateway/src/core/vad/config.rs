//! VAD configuration types

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// VAD backend selection
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum VADBackend {
    /// Silero VAD - ML-based, high accuracy (~2MB model)
    #[default]
    Silero,
    /// WebRTC VAD - lightweight, good for real-time
    WebRTC,
    /// Energy-based - simple RMS threshold detection
    Energy,
}

impl std::fmt::Display for VADBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VADBackend::Silero => write!(f, "silero"),
            VADBackend::WebRTC => write!(f, "webrtc"),
            VADBackend::Energy => write!(f, "energy"),
        }
    }
}

/// Configuration for Voice Activity Detection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VADConfig {
    /// Enable/disable VAD processing
    pub enabled: bool,

    /// VAD backend to use
    pub backend: VADBackend,

    /// Speech probability threshold (0.0 - 1.0)
    /// Higher values = stricter detection, fewer false positives
    pub threshold: f32,

    /// Minimum speech duration before triggering speech_start (ms)
    /// Helps filter out brief noise spikes
    pub min_speech_duration_ms: u32,

    /// Minimum silence duration before triggering speech_end (ms)
    /// Prevents premature end detection during pauses
    pub min_silence_duration_ms: u32,

    /// Pre-speech audio padding (ms)
    /// Amount of audio to include before speech_start for context
    pub pre_speech_padding_ms: u32,

    /// Post-speech audio padding (ms)
    /// Amount of audio to include after speech_end
    pub post_speech_padding_ms: u32,

    /// Sample rate for audio processing (Hz)
    /// Silero VAD expects 16000 Hz
    pub sample_rate: u32,

    /// Frame size in samples
    /// Silero VAD works with 512 samples (32ms at 16kHz)
    pub frame_size: usize,

    /// Path to the ONNX model file (optional, will download if not specified)
    pub model_path: Option<PathBuf>,

    /// URL to download the model from
    pub model_url: Option<String>,

    /// Cache directory for downloaded models
    pub cache_path: Option<PathBuf>,

    /// Number of threads for ONNX inference
    pub num_threads: Option<usize>,

    /// ONNX graph optimization level
    pub graph_optimization_level: GraphOptimizationLevel,

    /// Emit speech probability events (can be verbose)
    pub emit_probability_events: bool,
}

impl Default for VADConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            backend: VADBackend::Silero,
            threshold: 0.5,
            min_speech_duration_ms: 250,
            min_silence_duration_ms: 300,
            pre_speech_padding_ms: 100,
            post_speech_padding_ms: 100,
            sample_rate: 16000,
            frame_size: 512, // 32ms at 16kHz
            model_path: None,
            model_url: Some(
                "https://github.com/snakers4/silero-vad/raw/master/src/silero_vad/data/silero_vad.onnx"
                    .to_string(),
            ),
            cache_path: None,
            num_threads: Some(1), // VAD is lightweight, 1 thread is enough
            graph_optimization_level: GraphOptimizationLevel::Level3,
            emit_probability_events: false,
        }
    }
}

impl VADConfig {
    /// Create a new VADConfig with default values
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a VADConfig optimized for low latency
    pub fn low_latency() -> Self {
        Self {
            min_speech_duration_ms: 100,
            min_silence_duration_ms: 200,
            pre_speech_padding_ms: 50,
            post_speech_padding_ms: 50,
            ..Default::default()
        }
    }

    /// Create a VADConfig optimized for accuracy (fewer false positives)
    pub fn high_accuracy() -> Self {
        Self {
            threshold: 0.7,
            min_speech_duration_ms: 300,
            min_silence_duration_ms: 500,
            ..Default::default()
        }
    }

    /// Get the cache directory for VAD models
    pub fn get_cache_dir(&self) -> Result<PathBuf> {
        let cache_dir = if let Some(cache_path) = &self.cache_path {
            cache_path.join("vad")
        } else {
            anyhow::bail!("No cache directory specified for VAD");
        };
        Ok(cache_dir)
    }

    /// Calculate frame duration in milliseconds
    pub fn frame_duration_ms(&self) -> f32 {
        (self.frame_size as f32 / self.sample_rate as f32) * 1000.0
    }

    /// Calculate number of frames for a given duration in milliseconds
    pub fn frames_for_duration(&self, duration_ms: u32) -> usize {
        let frame_duration = self.frame_duration_ms();
        (duration_ms as f32 / frame_duration).ceil() as usize
    }

    /// Validate the configuration
    pub fn validate(&self) -> Result<()> {
        if self.threshold < 0.0 || self.threshold > 1.0 {
            anyhow::bail!("VAD threshold must be between 0.0 and 1.0");
        }
        if self.sample_rate == 0 {
            anyhow::bail!("VAD sample_rate must be greater than 0");
        }
        if self.frame_size == 0 {
            anyhow::bail!("VAD frame_size must be greater than 0");
        }
        // Silero VAD only supports specific sample rates
        if self.backend == VADBackend::Silero && self.sample_rate != 16000 && self.sample_rate != 8000 {
            anyhow::bail!("Silero VAD only supports 16000 Hz or 8000 Hz sample rates");
        }
        Ok(())
    }
}

/// ONNX graph optimization level
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
pub enum GraphOptimizationLevel {
    Disabled,
    Basic,
    Extended,
    #[default]
    Level3,
}

impl GraphOptimizationLevel {
    #[cfg(feature = "vad")]
    pub fn to_ort_level(&self) -> ort::session::builder::GraphOptimizationLevel {
        use ort::session::builder::GraphOptimizationLevel as OrtLevel;
        match self {
            Self::Disabled => OrtLevel::Disable,
            Self::Basic => OrtLevel::Level1,
            Self::Extended => OrtLevel::Level2,
            Self::Level3 => OrtLevel::Level3,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = VADConfig::default();
        assert!(config.enabled);
        assert_eq!(config.backend, VADBackend::Silero);
        assert_eq!(config.threshold, 0.5);
        assert_eq!(config.sample_rate, 16000);
        assert_eq!(config.frame_size, 512);
    }

    #[test]
    fn test_frame_duration() {
        let config = VADConfig::default();
        assert_eq!(config.frame_duration_ms(), 32.0);
    }

    #[test]
    fn test_frames_for_duration() {
        let config = VADConfig::default();
        // 300ms at 32ms per frame = ~10 frames
        assert_eq!(config.frames_for_duration(300), 10);
    }

    #[test]
    fn test_validate_threshold() {
        let mut config = VADConfig::default();

        config.threshold = 0.5;
        assert!(config.validate().is_ok());

        config.threshold = -0.1;
        assert!(config.validate().is_err());

        config.threshold = 1.1;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_sample_rate() {
        let mut config = VADConfig::default();

        config.sample_rate = 16000;
        assert!(config.validate().is_ok());

        config.sample_rate = 8000;
        assert!(config.validate().is_ok());

        config.sample_rate = 44100; // Not supported by Silero
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_backend_display() {
        assert_eq!(format!("{}", VADBackend::Silero), "silero");
        assert_eq!(format!("{}", VADBackend::WebRTC), "webrtc");
        assert_eq!(format!("{}", VADBackend::Energy), "energy");
    }
}
