//! Stub implementations when silero-vad feature is disabled.
//!
//! This provides compile-time stubs so code can reference these types
//! without conditional compilation everywhere.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Stub configuration for Silero VAD.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SileroVADConfig {
    pub model_path: Option<PathBuf>,
    pub threshold: f32,
    pub sample_rate: u32,
    pub chunk_size: usize,
    pub min_speech_duration_ms: u32,
    pub min_silence_duration_ms: u32,
    pub num_threads: usize,
    pub debug_logging: bool,
}

/// Stub result for VAD processing.
#[derive(Debug, Clone, Copy)]
pub struct SileroVADResult {
    pub probability: f32,
    pub is_speech: bool,
    pub inference_time_us: u64,
}

/// Stub Silero VAD detector.
///
/// This is a placeholder when the silero-vad feature is disabled.
/// All methods will return errors or default values.
pub struct SileroVAD;

impl SileroVAD {
    /// Stub constructor that always fails.
    pub async fn new(_config: SileroVADConfig) -> anyhow::Result<Self> {
        anyhow::bail!("Silero VAD feature is not enabled. Rebuild with --features silero-vad")
    }

    /// Stub process method.
    pub fn process(&mut self, _audio: &[f32]) -> anyhow::Result<SileroVADResult> {
        anyhow::bail!("Silero VAD feature is not enabled")
    }

    /// Returns false.
    pub fn is_in_speech(&self) -> bool {
        false
    }

    /// Returns 0.0.
    pub fn silence_duration_ms(&self) -> f32 {
        0.0
    }

    /// Returns 0.0.
    pub fn speech_duration_ms(&self) -> f32 {
        0.0
    }

    /// No-op reset.
    pub fn reset(&mut self) {}

    /// Returns 0.
    pub fn total_frames(&self) -> u64 {
        0
    }

    /// Returns default config.
    pub fn config(&self) -> &SileroVADConfig {
        static DEFAULT_CONFIG: std::sync::OnceLock<SileroVADConfig> = std::sync::OnceLock::new();
        DEFAULT_CONFIG.get_or_init(SileroVADConfig::default)
    }
}
