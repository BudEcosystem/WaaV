//! Stub implementation for when the `vad` feature is disabled

use anyhow::Result;
use super::config::VADConfig;

/// Result of VAD processing for a single audio frame
#[derive(Debug, Clone, Default)]
pub struct VADResult {
    /// Whether the current frame contains speech
    pub is_speech: bool,
    /// Speech probability (0.0 - 1.0)
    pub probability: f32,
    /// Whether speech just started (transition from silence to speech)
    pub speech_start: bool,
    /// Whether speech just ended (transition from speech to silence)
    pub speech_end: bool,
    /// Duration of current speech segment in milliseconds (if speaking)
    pub speech_duration_ms: u32,
    /// Duration of current silence segment in milliseconds (if silent)
    pub silence_duration_ms: u32,
}

/// Trait for Voice Activity Detection implementations
pub trait VoiceActivityDetector: Send + Sync {
    /// Process a single audio frame and return VAD result
    fn process_frame(&mut self, audio: &[f32]) -> Result<VADResult>;

    /// Reset internal state (call when starting a new audio stream)
    fn reset(&mut self);

    /// Get the current speech probability
    fn speech_probability(&self) -> f32;

    /// Check if currently in speech state
    fn is_speaking(&self) -> bool;

    /// Get the configuration
    fn config(&self) -> &VADConfig;
}

/// No-op Silero VAD placeholder when the `vad` feature is disabled
pub struct SileroVAD {
    config: VADConfig,
}

impl SileroVAD {
    /// Create a disabled VAD instance
    pub async fn new(config: VADConfig) -> Result<Self> {
        Ok(Self { config })
    }

    /// Process a frame - always returns no speech detected
    pub fn process_frame(&mut self, _audio: &[f32]) -> Result<VADResult> {
        Ok(VADResult::default())
    }

    /// Reset state - no-op
    pub fn reset(&mut self) {}

    /// Get speech probability - always 0.0
    pub fn speech_probability(&self) -> f32 {
        0.0
    }

    /// Check if speaking - always false
    pub fn is_speaking(&self) -> bool {
        false
    }

    /// Get configuration
    pub fn config(&self) -> &VADConfig {
        &self.config
    }
}

impl VoiceActivityDetector for SileroVAD {
    fn process_frame(&mut self, audio: &[f32]) -> Result<VADResult> {
        SileroVAD::process_frame(self, audio)
    }

    fn reset(&mut self) {
        SileroVAD::reset(self)
    }

    fn speech_probability(&self) -> f32 {
        SileroVAD::speech_probability(self)
    }

    fn is_speaking(&self) -> bool {
        SileroVAD::is_speaking(self)
    }

    fn config(&self) -> &VADConfig {
        SileroVAD::config(self)
    }
}

/// Create a disabled VAD instance
pub async fn create_vad(config: VADConfig) -> Result<SileroVAD> {
    SileroVAD::new(config).await
}
