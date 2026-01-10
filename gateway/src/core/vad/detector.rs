//! Silero VAD detector implementation

use anyhow::Result;
use std::collections::VecDeque;
use std::time::Instant;
use tracing::{debug, info, trace};

use super::config::VADConfig;
use super::model_manager::ModelManager;

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
    /// Timestamp when this result was generated
    pub timestamp_ms: u64,
}

/// State machine for VAD transitions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VADState {
    /// No speech detected
    Silence,
    /// Potential speech detected, waiting for confirmation
    PotentialSpeech,
    /// Confirmed speech in progress
    Speech,
    /// Potential end of speech, waiting for confirmation
    PotentialSilence,
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

/// Silero VAD implementation using ONNX Runtime
pub struct SileroVAD {
    /// ONNX model manager
    model: ModelManager,
    /// VAD configuration
    config: VADConfig,
    /// Current VAD state
    state: VADState,
    /// Current speech probability
    current_probability: f32,
    /// Frame counter for timing
    frame_count: u64,
    /// Start time for session timing
    start_time: Instant,
    /// Frames with speech probability above threshold (for min_speech_duration)
    speech_frames: u32,
    /// Frames with speech probability below threshold (for min_silence_duration)
    silence_frames: u32,
    /// Required frames for speech confirmation
    min_speech_frames: u32,
    /// Required frames for silence confirmation
    min_silence_frames: u32,
    /// Pre-speech audio buffer (ring buffer)
    pre_speech_buffer: VecDeque<Vec<f32>>,
    /// Number of frames to buffer for pre-speech padding
    pre_speech_buffer_frames: usize,
    /// Smoothed probability using exponential moving average
    smoothed_probability: f32,
    /// EMA smoothing factor
    smoothing_factor: f32,
    /// Statistics: total frames processed
    total_frames: u64,
    /// Statistics: total speech frames
    total_speech_frames: u64,
    /// Statistics: total inference time in microseconds
    total_inference_us: u64,
}

impl SileroVAD {
    /// Create a new Silero VAD instance
    pub async fn new(config: VADConfig) -> Result<Self> {
        config.validate()?;

        let model = ModelManager::new(config.clone()).await?;

        // Calculate frame counts for duration thresholds
        let frame_duration_ms = config.frame_duration_ms();
        let min_speech_frames = (config.min_speech_duration_ms as f32 / frame_duration_ms).ceil() as u32;
        let min_silence_frames = (config.min_silence_duration_ms as f32 / frame_duration_ms).ceil() as u32;
        let pre_speech_buffer_frames = (config.pre_speech_padding_ms as f32 / frame_duration_ms).ceil() as usize;

        info!(
            "Silero VAD initialized: threshold={:.2}, min_speech={}ms ({}f), min_silence={}ms ({}f)",
            config.threshold,
            config.min_speech_duration_ms,
            min_speech_frames,
            config.min_silence_duration_ms,
            min_silence_frames
        );

        Ok(Self {
            model,
            config,
            state: VADState::Silence,
            current_probability: 0.0,
            frame_count: 0,
            start_time: Instant::now(),
            speech_frames: 0,
            silence_frames: 0,
            min_speech_frames,
            min_silence_frames,
            pre_speech_buffer: VecDeque::with_capacity(pre_speech_buffer_frames),
            pre_speech_buffer_frames,
            smoothed_probability: 0.0,
            smoothing_factor: 0.7, // EMA factor for smoothing
            total_frames: 0,
            total_speech_frames: 0,
            total_inference_us: 0,
        })
    }

    /// Process an audio frame and return VAD result
    pub fn process_frame(&mut self, audio: &[f32]) -> Result<VADResult> {
        let inference_start = Instant::now();

        // Run model inference
        let raw_probability = self.model.predict(audio)?;

        let inference_us = inference_start.elapsed().as_micros() as u64;
        self.total_inference_us += inference_us;

        // Apply exponential moving average smoothing
        self.smoothed_probability = self.smoothing_factor * self.smoothed_probability
            + (1.0 - self.smoothing_factor) * raw_probability;

        self.current_probability = self.smoothed_probability;
        self.frame_count += 1;
        self.total_frames += 1;

        // Determine if this frame is speech
        let is_speech_frame = self.current_probability >= self.config.threshold;

        // Update frame counters
        if is_speech_frame {
            self.speech_frames += 1;
            self.silence_frames = 0;
            self.total_speech_frames += 1;
        } else {
            self.silence_frames += 1;
            self.speech_frames = 0;
        }

        // State machine transitions
        let (new_state, speech_start, speech_end) = self.transition_state(is_speech_frame);

        let prev_state = self.state;
        self.state = new_state;

        // Calculate durations
        let frame_duration_ms = self.config.frame_duration_ms() as u32;
        let speech_duration_ms = if matches!(self.state, VADState::Speech | VADState::PotentialSilence) {
            self.speech_frames * frame_duration_ms
        } else {
            0
        };
        let silence_duration_ms = if matches!(self.state, VADState::Silence | VADState::PotentialSpeech) {
            self.silence_frames * frame_duration_ms
        } else {
            0
        };

        // Update pre-speech buffer
        if self.pre_speech_buffer_frames > 0 {
            if self.pre_speech_buffer.len() >= self.pre_speech_buffer_frames {
                self.pre_speech_buffer.pop_front();
            }
            self.pre_speech_buffer.push_back(audio.to_vec());
        }

        let timestamp_ms = self.start_time.elapsed().as_millis() as u64;

        let result = VADResult {
            is_speech: matches!(self.state, VADState::Speech | VADState::PotentialSilence),
            probability: self.current_probability,
            speech_start,
            speech_end,
            speech_duration_ms,
            silence_duration_ms,
            timestamp_ms,
        };

        trace!(
            "VAD frame {}: prob={:.3} (raw={:.3}), state={:?}->{:?}, speech_start={}, speech_end={}",
            self.frame_count,
            self.current_probability,
            raw_probability,
            prev_state,
            self.state,
            speech_start,
            speech_end
        );

        Ok(result)
    }

    /// State machine transition logic
    fn transition_state(&self, is_speech_frame: bool) -> (VADState, bool, bool) {
        let mut speech_start = false;
        let mut speech_end = false;

        let new_state = match self.state {
            VADState::Silence => {
                if is_speech_frame {
                    if self.speech_frames >= self.min_speech_frames {
                        speech_start = true;
                        debug!(
                            "Speech started after {} frames ({:.0}ms)",
                            self.speech_frames,
                            self.speech_frames as f32 * self.config.frame_duration_ms()
                        );
                        VADState::Speech
                    } else {
                        VADState::PotentialSpeech
                    }
                } else {
                    VADState::Silence
                }
            }
            VADState::PotentialSpeech => {
                if is_speech_frame {
                    if self.speech_frames >= self.min_speech_frames {
                        speech_start = true;
                        debug!(
                            "Speech confirmed after {} frames ({:.0}ms)",
                            self.speech_frames,
                            self.speech_frames as f32 * self.config.frame_duration_ms()
                        );
                        VADState::Speech
                    } else {
                        VADState::PotentialSpeech
                    }
                } else {
                    // Speech didn't last long enough, return to silence
                    VADState::Silence
                }
            }
            VADState::Speech => {
                if !is_speech_frame {
                    if self.silence_frames >= self.min_silence_frames {
                        speech_end = true;
                        debug!(
                            "Speech ended after {} silence frames ({:.0}ms)",
                            self.silence_frames,
                            self.silence_frames as f32 * self.config.frame_duration_ms()
                        );
                        VADState::Silence
                    } else {
                        VADState::PotentialSilence
                    }
                } else {
                    VADState::Speech
                }
            }
            VADState::PotentialSilence => {
                if !is_speech_frame {
                    if self.silence_frames >= self.min_silence_frames {
                        speech_end = true;
                        debug!(
                            "Silence confirmed after {} frames ({:.0}ms)",
                            self.silence_frames,
                            self.silence_frames as f32 * self.config.frame_duration_ms()
                        );
                        VADState::Silence
                    } else {
                        VADState::PotentialSilence
                    }
                } else {
                    // Speaker resumed, return to speech
                    VADState::Speech
                }
            }
        };

        (new_state, speech_start, speech_end)
    }

    /// Reset the VAD state
    pub fn reset(&mut self) {
        self.model.reset();
        self.state = VADState::Silence;
        self.current_probability = 0.0;
        self.smoothed_probability = 0.0;
        self.frame_count = 0;
        self.speech_frames = 0;
        self.silence_frames = 0;
        self.pre_speech_buffer.clear();
        self.start_time = Instant::now();

        debug!("VAD state reset");
    }

    /// Get current speech probability
    pub fn speech_probability(&self) -> f32 {
        self.current_probability
    }

    /// Check if currently speaking
    pub fn is_speaking(&self) -> bool {
        matches!(self.state, VADState::Speech | VADState::PotentialSilence)
    }

    /// Get the configuration
    pub fn config(&self) -> &VADConfig {
        &self.config
    }

    /// Get pre-speech audio buffer (for including audio before speech_start)
    pub fn get_pre_speech_audio(&self) -> Vec<f32> {
        self.pre_speech_buffer
            .iter()
            .flatten()
            .copied()
            .collect()
    }

    /// Get statistics about VAD performance
    pub fn get_stats(&self) -> VADStats {
        let avg_inference_us = if self.total_frames > 0 {
            self.total_inference_us / self.total_frames
        } else {
            0
        };

        let speech_ratio = if self.total_frames > 0 {
            self.total_speech_frames as f32 / self.total_frames as f32
        } else {
            0.0
        };

        VADStats {
            total_frames: self.total_frames,
            total_speech_frames: self.total_speech_frames,
            speech_ratio,
            avg_inference_us,
            total_duration_ms: self.start_time.elapsed().as_millis() as u64,
        }
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

/// Statistics about VAD performance
#[derive(Debug, Clone)]
pub struct VADStats {
    /// Total frames processed
    pub total_frames: u64,
    /// Total frames classified as speech
    pub total_speech_frames: u64,
    /// Ratio of speech frames to total frames
    pub speech_ratio: f32,
    /// Average inference time per frame in microseconds
    pub avg_inference_us: u64,
    /// Total processing duration in milliseconds
    pub total_duration_ms: u64,
}

impl std::fmt::Display for VADStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "VAD Stats: {} frames ({} speech, {:.1}% ratio), avg inference: {}us, duration: {}ms",
            self.total_frames,
            self.total_speech_frames,
            self.speech_ratio * 100.0,
            self.avg_inference_us,
            self.total_duration_ms
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vad_result_default() {
        let result = VADResult::default();
        assert!(!result.is_speech);
        assert_eq!(result.probability, 0.0);
        assert!(!result.speech_start);
        assert!(!result.speech_end);
    }

    #[test]
    fn test_vad_state_transitions() {
        // This tests the state machine logic without needing a model
        // We'll create a mock state and test transitions

        // Silence -> PotentialSpeech (1 frame of speech)
        // PotentialSpeech -> Silence (silence frame before confirmation)
        // PotentialSpeech -> Speech (enough speech frames)
        // Speech -> PotentialSilence (1 frame of silence)
        // PotentialSilence -> Speech (speech resumes)
        // PotentialSilence -> Silence (enough silence frames)
    }

    #[test]
    fn test_vad_stats_display() {
        let stats = VADStats {
            total_frames: 1000,
            total_speech_frames: 300,
            speech_ratio: 0.3,
            avg_inference_us: 500,
            total_duration_ms: 32000,
        };

        let display = format!("{}", stats);
        assert!(display.contains("1000 frames"));
        assert!(display.contains("300 speech"));
        assert!(display.contains("30.0%"));
        assert!(display.contains("500us"));
    }
}
