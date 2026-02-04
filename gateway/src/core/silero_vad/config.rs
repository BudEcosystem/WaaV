//! Configuration for Silero VAD.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Configuration for Silero VAD detector.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SileroVADConfig {
    /// Path to the Silero VAD ONNX model file.
    ///
    /// If not specified, will attempt to download from HuggingFace.
    #[serde(default)]
    pub model_path: Option<PathBuf>,

    /// Speech detection threshold (0.0 to 1.0).
    ///
    /// Higher values require more confidence to classify as speech.
    /// Recommended: 0.5 for balanced detection.
    #[serde(default = "default_threshold")]
    pub threshold: f32,

    /// Sample rate in Hz.
    ///
    /// Silero VAD v5 only supports 16000 Hz.
    #[serde(default = "default_sample_rate")]
    pub sample_rate: u32,

    /// Chunk size in samples.
    ///
    /// Silero VAD v5 only supports 512 samples at 16kHz (32ms).
    #[serde(default = "default_chunk_size")]
    pub chunk_size: usize,

    /// Minimum speech duration in milliseconds to trigger speech start.
    ///
    /// Helps filter out brief noise spikes.
    #[serde(default = "default_min_speech_duration_ms")]
    pub min_speech_duration_ms: u32,

    /// Minimum silence duration in milliseconds to trigger speech end.
    ///
    /// Allows for natural pauses within speech.
    #[serde(default = "default_min_silence_duration_ms")]
    pub min_silence_duration_ms: u32,

    /// Number of threads for ONNX inference.
    ///
    /// Default: 1 (single-threaded is usually sufficient for this lightweight model).
    #[serde(default = "default_num_threads")]
    pub num_threads: usize,

    /// Interval in seconds to automatically reset VAD internal states.
    ///
    /// The Silero VAD LSTM maintains internal state that can drift over time,
    /// especially in long conversations. Periodic resets prevent state
    /// accumulation that could affect detection accuracy.
    ///
    /// Default: 5.0 seconds (matching pipecat/smart-turn reference implementation).
    /// Set to 0.0 to disable automatic resets.
    #[serde(default = "default_state_reset_interval_secs")]
    pub state_reset_interval_secs: f32,

    /// Whether to enable debug logging for VAD events.
    #[serde(default)]
    pub debug_logging: bool,
}

fn default_threshold() -> f32 {
    0.5
}

fn default_sample_rate() -> u32 {
    16000
}

fn default_chunk_size() -> usize {
    512
}

fn default_min_speech_duration_ms() -> u32 {
    250
}

fn default_min_silence_duration_ms() -> u32 {
    200
}

fn default_num_threads() -> usize {
    1
}

fn default_state_reset_interval_secs() -> f32 {
    5.0 // Match pipecat/smart-turn reference implementation
}

impl Default for SileroVADConfig {
    fn default() -> Self {
        Self {
            model_path: None,
            threshold: default_threshold(),
            sample_rate: default_sample_rate(),
            chunk_size: default_chunk_size(),
            min_speech_duration_ms: default_min_speech_duration_ms(),
            min_silence_duration_ms: default_min_silence_duration_ms(),
            num_threads: default_num_threads(),
            state_reset_interval_secs: default_state_reset_interval_secs(),
            debug_logging: false,
        }
    }
}

impl SileroVADConfig {
    /// Creates a new configuration with default values.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the model path.
    pub fn with_model_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.model_path = Some(path.into());
        self
    }

    /// Sets the speech detection threshold.
    pub fn with_threshold(mut self, threshold: f32) -> Self {
        self.threshold = threshold.clamp(0.0, 1.0);
        self
    }

    /// Sets the minimum speech duration.
    pub fn with_min_speech_duration_ms(mut self, ms: u32) -> Self {
        self.min_speech_duration_ms = ms;
        self
    }

    /// Sets the minimum silence duration.
    pub fn with_min_silence_duration_ms(mut self, ms: u32) -> Self {
        self.min_silence_duration_ms = ms;
        self
    }

    /// Sets the state reset interval in seconds.
    ///
    /// Set to 0.0 to disable automatic state resets.
    pub fn with_state_reset_interval_secs(mut self, secs: f32) -> Self {
        self.state_reset_interval_secs = secs.max(0.0);
        self
    }

    /// Enables debug logging.
    pub fn with_debug_logging(mut self) -> Self {
        self.debug_logging = true;
        self
    }

    /// Validates the configuration.
    pub fn validate(&self) -> Result<(), String> {
        if self.sample_rate != 16000 {
            return Err(format!(
                "Silero VAD v5 only supports 16000 Hz sample rate, got {}",
                self.sample_rate
            ));
        }

        if self.chunk_size != 512 {
            return Err(format!(
                "Silero VAD v5 only supports 512 sample chunks at 16kHz, got {}",
                self.chunk_size
            ));
        }

        if !(0.0..=1.0).contains(&self.threshold) {
            return Err(format!(
                "Threshold must be between 0.0 and 1.0, got {}",
                self.threshold
            ));
        }

        Ok(())
    }

    /// Returns the chunk duration in milliseconds.
    #[inline]
    pub fn chunk_duration_ms(&self) -> f32 {
        (self.chunk_size as f32 / self.sample_rate as f32) * 1000.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = SileroVADConfig::default();

        assert_eq!(config.threshold, 0.5);
        assert_eq!(config.sample_rate, 16000);
        assert_eq!(config.chunk_size, 512);
        assert_eq!(config.min_speech_duration_ms, 250);
        assert_eq!(config.min_silence_duration_ms, 200);
        assert_eq!(config.num_threads, 1);
        assert_eq!(config.state_reset_interval_secs, 5.0);
        assert!(!config.debug_logging);
    }

    #[test]
    fn test_state_reset_interval_config() {
        // Test setting custom interval
        let config = SileroVADConfig::new().with_state_reset_interval_secs(10.0);
        assert_eq!(config.state_reset_interval_secs, 10.0);

        // Test disabling by setting to 0
        let config = SileroVADConfig::new().with_state_reset_interval_secs(0.0);
        assert_eq!(config.state_reset_interval_secs, 0.0);

        // Test negative values are clamped to 0
        let config = SileroVADConfig::new().with_state_reset_interval_secs(-1.0);
        assert_eq!(config.state_reset_interval_secs, 0.0);
    }

    #[test]
    fn test_config_validation_valid() {
        let config = SileroVADConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validation_invalid_sample_rate() {
        let config = SileroVADConfig {
            sample_rate: 8000,
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_invalid_chunk_size() {
        let config = SileroVADConfig {
            chunk_size: 256,
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_invalid_threshold() {
        let config = SileroVADConfig {
            threshold: 1.5,
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_chunk_duration_ms() {
        let config = SileroVADConfig::default();
        // 512 samples at 16kHz = 32ms
        assert!((config.chunk_duration_ms() - 32.0).abs() < 0.01);
    }

    #[test]
    fn test_builder_pattern() {
        let config = SileroVADConfig::new()
            .with_threshold(0.7)
            .with_min_speech_duration_ms(300)
            .with_min_silence_duration_ms(250)
            .with_debug_logging();

        assert_eq!(config.threshold, 0.7);
        assert_eq!(config.min_speech_duration_ms, 300);
        assert_eq!(config.min_silence_duration_ms, 250);
        assert!(config.debug_logging);
    }

    #[test]
    fn test_threshold_clamping() {
        let config = SileroVADConfig::new().with_threshold(2.0);
        assert_eq!(config.threshold, 1.0);

        let config = SileroVADConfig::new().with_threshold(-0.5);
        assert_eq!(config.threshold, 0.0);
    }
}
