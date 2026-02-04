//! Stub implementations when smart-turn feature is disabled.
//!
//! This provides compile-time stubs so code can reference these types
//! without conditional compilation everywhere.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// =============================================================================
// Mel Extractor Constants and Types
// =============================================================================

/// Default Whisper sample rate.
pub const WHISPER_SAMPLE_RATE: u32 = 16000;
/// Default Whisper FFT size.
pub const WHISPER_N_FFT: usize = 400;
/// Default Whisper hop length.
pub const WHISPER_HOP_LENGTH: usize = 160;
/// Default Whisper number of mel bins.
pub const WHISPER_N_MELS: usize = 80;
/// Default Whisper max duration in seconds.
pub const WHISPER_MAX_DURATION_SECS: f32 = 8.0;
/// Default Whisper max frames.
pub const WHISPER_MAX_FRAMES: usize = 800;

/// Stub configuration for MelExtractor.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MelExtractorConfig {
    pub input_sample_rate: u32,
    pub n_fft: usize,
    pub hop_length: usize,
    pub n_mels: usize,
    pub max_duration_secs: f32,
}

/// Stub MelExtractor.
///
/// This is a placeholder when the smart-turn feature is disabled.
pub struct MelExtractor;

impl MelExtractor {
    /// Stub constructor that always fails.
    pub fn new(_config: MelExtractorConfig) -> anyhow::Result<Self> {
        anyhow::bail!("Smart Turn feature is not enabled. Rebuild with --features smart-turn")
    }

    /// Stub process method.
    pub fn process(&mut self, _audio: &[f32]) -> anyhow::Result<usize> {
        anyhow::bail!("Smart Turn feature is not enabled")
    }

    /// Stub get_mel_frames method.
    pub fn get_mel_frames(&self) -> &[Vec<f32>] {
        &[]
    }

    /// Stub reset method.
    pub fn reset(&mut self) {}
}

// =============================================================================
// Smart Turn Detector Constants and Types
// =============================================================================

/// Maximum frames for Smart Turn model.
pub const SMART_TURN_MAX_FRAMES: usize = 800;

/// Stub configuration for SmartTurnDetector.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SmartTurnDetectorConfig {
    pub model_path: Option<PathBuf>,
    pub model_url: Option<String>,
    pub threshold: f32,
    pub max_frames: usize,
    pub n_mels: usize,
    pub num_threads: usize,
    pub debug_logging: bool,
    pub min_speech_ms: u32,
    pub hysteresis_frames: u32,
}

/// Stub result for SmartTurnDetector.
#[derive(Debug, Clone, Copy, Default)]
pub struct SmartTurnResult {
    pub probability: f32,
    pub is_turn_complete: bool,
    pub inference_time_us: u64,
    pub frames_processed: usize,
}

/// Stub SmartTurnDetector.
///
/// This is a placeholder when the smart-turn feature is disabled.
pub struct SmartTurnDetector;

impl SmartTurnDetector {
    /// Stub constructor that always fails.
    pub async fn new(_config: SmartTurnDetectorConfig) -> anyhow::Result<Self> {
        anyhow::bail!("Smart Turn feature is not enabled. Rebuild with --features smart-turn")
    }

    /// Stub predict method.
    pub async fn predict(&mut self, _mel_frames: &[Vec<f32>]) -> anyhow::Result<SmartTurnResult> {
        anyhow::bail!("Smart Turn feature is not enabled")
    }

    /// Stub reset method.
    pub fn reset(&mut self) {}
}

/// Stub builder for SmartTurnDetector.
pub struct SmartTurnDetectorBuilder {
    config: SmartTurnDetectorConfig,
}

impl Default for SmartTurnDetectorBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl SmartTurnDetectorBuilder {
    /// Creates a new builder.
    pub fn new() -> Self {
        Self {
            config: SmartTurnDetectorConfig::default(),
        }
    }

    /// Sets the model path.
    pub fn model_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.config.model_path = Some(path.into());
        self
    }

    /// Sets the threshold.
    pub fn threshold(mut self, threshold: f32) -> Self {
        self.config.threshold = threshold;
        self
    }

    /// Builds the detector (always fails when feature is disabled).
    pub async fn build(self) -> anyhow::Result<SmartTurnDetector> {
        anyhow::bail!("Smart Turn feature is not enabled. Rebuild with --features smart-turn")
    }
}
