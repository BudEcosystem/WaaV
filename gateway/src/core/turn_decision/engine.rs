//! Turn Decision Engine implementation.
//!
//! This module implements the ensemble turn detection logic that combines
//! multiple signals to make accurate turn completion decisions.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::time::Instant;
use tracing::{debug, trace};

/// The state of the user's speech.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TurnState {
    /// Waiting for user to start speaking.
    #[default]
    Idle,

    /// User is actively speaking.
    Speaking,

    /// User has stopped speaking, waiting to confirm turn completion.
    /// Contains the time when silence started.
    PausedMaybeTurn,

    /// Turn has been confirmed as complete.
    TurnComplete,
}

impl TurnState {
    /// Returns whether the user is currently speaking.
    #[inline]
    pub fn is_speaking(&self) -> bool {
        matches!(self, TurnState::Speaking)
    }

    /// Returns whether we're waiting to confirm a turn.
    #[inline]
    pub fn is_paused(&self) -> bool {
        matches!(self, TurnState::PausedMaybeTurn)
    }

    /// Returns whether a turn was completed.
    #[inline]
    pub fn is_turn_complete(&self) -> bool {
        matches!(self, TurnState::TurnComplete)
    }
}

/// A signal contributing to the turn decision.
#[derive(Debug, Clone, Copy)]
pub enum TurnSignal {
    /// Audio-based turn signal from SmartTurnDetector.
    Audio {
        probability: f32,
        frames_processed: usize,
    },

    /// Text-based turn signal from TurnDetector.
    Text {
        probability: f32,
        transcript_len: usize,
    },

    /// VAD-based silence signal.
    Silence { duration_ms: f32, is_speech: bool },
}

/// The result of turn decision processing.
#[derive(Debug, Clone)]
pub struct TurnDecision {
    /// Current turn state.
    pub state: TurnState,

    /// Whether a turn completion was detected this update.
    pub is_turn_complete: bool,

    /// Combined probability of turn completion [0.0, 1.0].
    pub combined_probability: f32,

    /// Audio signal probability (if available).
    pub audio_probability: Option<f32>,

    /// Text signal probability (if available).
    pub text_probability: Option<f32>,

    /// Current silence duration in milliseconds.
    pub silence_duration_ms: f32,

    /// Processing latency for this decision.
    pub latency_us: u64,

    /// Reason for the decision (for debugging).
    pub reason: String,
}

impl Default for TurnDecision {
    fn default() -> Self {
        Self {
            state: TurnState::Idle,
            is_turn_complete: false,
            combined_probability: 0.0,
            audio_probability: None,
            text_probability: None,
            silence_duration_ms: 0.0,
            latency_us: 0,
            reason: String::new(),
        }
    }
}

/// Configuration for the Turn Decision Engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnDecisionEngineConfig {
    /// Audio turn detection threshold.
    #[serde(default = "default_audio_threshold")]
    pub audio_threshold: f32,

    /// Text turn detection threshold.
    #[serde(default = "default_text_threshold")]
    pub text_threshold: f32,

    /// Minimum silence duration before checking for turn (milliseconds).
    #[serde(default = "default_min_silence_ms")]
    pub min_silence_ms: u32,

    /// Maximum silence duration before forcing turn (milliseconds).
    /// If silence exceeds this, turn is complete regardless of probability.
    #[serde(default = "default_max_silence_ms")]
    pub max_silence_ms: u32,

    /// Minimum speech duration before a turn can be detected (milliseconds).
    #[serde(default = "default_min_speech_ms")]
    pub min_speech_ms: u32,

    /// Weight for audio signal in ensemble (0.0-1.0).
    #[serde(default = "default_audio_weight")]
    pub audio_weight: f32,

    /// Weight for text signal in ensemble (0.0-1.0).
    /// audio_weight + text_weight should sum to 1.0 for weighted average.
    #[serde(default = "default_text_weight")]
    pub text_weight: f32,

    /// Whether to use audio signal.
    #[serde(default = "default_use_audio")]
    pub use_audio: bool,

    /// Whether to use text signal.
    #[serde(default = "default_use_text")]
    pub use_text: bool,

    /// Enable debug logging.
    #[serde(default)]
    pub debug_logging: bool,

    /// Hysteresis frames for turn confirmation.
    /// Consecutive decisions above threshold needed.
    #[serde(default = "default_hysteresis_frames")]
    pub hysteresis_frames: u32,
}

fn default_audio_threshold() -> f32 {
    0.7
}

fn default_text_threshold() -> f32 {
    0.8
}

fn default_min_silence_ms() -> u32 {
    200
}

fn default_max_silence_ms() -> u32 {
    2000
}

fn default_min_speech_ms() -> u32 {
    300
}

fn default_audio_weight() -> f32 {
    0.7
}

fn default_text_weight() -> f32 {
    0.3
}

fn default_use_audio() -> bool {
    true
}

fn default_use_text() -> bool {
    false
}

fn default_hysteresis_frames() -> u32 {
    2
}

impl Default for TurnDecisionEngineConfig {
    fn default() -> Self {
        Self {
            audio_threshold: default_audio_threshold(),
            text_threshold: default_text_threshold(),
            min_silence_ms: default_min_silence_ms(),
            max_silence_ms: default_max_silence_ms(),
            min_speech_ms: default_min_speech_ms(),
            audio_weight: default_audio_weight(),
            text_weight: default_text_weight(),
            use_audio: default_use_audio(),
            use_text: default_use_text(),
            debug_logging: false,
            hysteresis_frames: default_hysteresis_frames(),
        }
    }
}

impl TurnDecisionEngineConfig {
    /// Creates a new configuration with default values.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the audio threshold.
    pub fn with_audio_threshold(mut self, threshold: f32) -> Self {
        self.audio_threshold = threshold.clamp(0.0, 1.0);
        self
    }

    /// Sets the text threshold.
    pub fn with_text_threshold(mut self, threshold: f32) -> Self {
        self.text_threshold = threshold.clamp(0.0, 1.0);
        self
    }

    /// Sets the minimum silence duration.
    pub fn with_min_silence_ms(mut self, ms: u32) -> Self {
        self.min_silence_ms = ms;
        self
    }

    /// Sets the maximum silence duration.
    pub fn with_max_silence_ms(mut self, ms: u32) -> Self {
        self.max_silence_ms = ms;
        self
    }

    /// Sets the minimum speech duration.
    pub fn with_min_speech_ms(mut self, ms: u32) -> Self {
        self.min_speech_ms = ms;
        self
    }

    /// Enables audio signal.
    pub fn with_audio(mut self, enabled: bool) -> Self {
        self.use_audio = enabled;
        self
    }

    /// Enables text signal.
    pub fn with_text(mut self, enabled: bool) -> Self {
        self.use_text = enabled;
        self
    }

    /// Sets the ensemble weights.
    pub fn with_weights(mut self, audio: f32, text: f32) -> Self {
        self.audio_weight = audio.clamp(0.0, 1.0);
        self.text_weight = text.clamp(0.0, 1.0);
        self
    }

    /// Enables debug logging.
    pub fn with_debug_logging(mut self) -> Self {
        self.debug_logging = true;
        self
    }

    /// Validates the configuration.
    pub fn validate(&self) -> Result<(), String> {
        if self.audio_threshold < 0.0 || self.audio_threshold > 1.0 {
            return Err(format!(
                "audio_threshold must be between 0.0 and 1.0, got {}",
                self.audio_threshold
            ));
        }

        if self.text_threshold < 0.0 || self.text_threshold > 1.0 {
            return Err(format!(
                "text_threshold must be between 0.0 and 1.0, got {}",
                self.text_threshold
            ));
        }

        if self.min_silence_ms > self.max_silence_ms {
            return Err(format!(
                "min_silence_ms ({}) cannot exceed max_silence_ms ({})",
                self.min_silence_ms, self.max_silence_ms
            ));
        }

        if !self.use_audio && !self.use_text {
            return Err("At least one signal (audio or text) must be enabled".to_string());
        }

        Ok(())
    }
}

/// Turn Decision Engine for ensemble turn detection.
///
/// This engine combines multiple signals (VAD, audio turn, text turn)
/// to make accurate turn completion decisions.
pub struct TurnDecisionEngine {
    /// Configuration.
    config: TurnDecisionEngineConfig,

    /// Current turn state.
    state: TurnState,

    /// Time when silence started (for tracking duration).
    silence_start: Option<Instant>,

    /// Time when speech started (for tracking duration).
    speech_start: Option<Instant>,

    /// Total speech duration this turn (milliseconds).
    speech_duration_ms: f32,

    /// Last audio probability.
    last_audio_prob: f32,

    /// Last text probability.
    last_text_prob: f32,

    /// Current silence duration.
    silence_duration_ms: f32,

    /// Consecutive high-probability frames (for hysteresis).
    high_prob_frames: u32,

    /// Total turns detected.
    total_turns: u64,

    /// Total decisions made.
    total_decisions: u64,
}

impl TurnDecisionEngine {
    /// Creates a new Turn Decision Engine.
    pub fn new(config: TurnDecisionEngineConfig) -> Result<Self> {
        config
            .validate()
            .map_err(|e| anyhow::anyhow!("Invalid configuration: {}", e))?;

        debug!(
            "Initializing TurnDecisionEngine: audio_threshold={}, text_threshold={}, min_silence={}ms",
            config.audio_threshold, config.text_threshold, config.min_silence_ms
        );

        Ok(Self {
            config,
            state: TurnState::Idle,
            silence_start: None,
            speech_start: None,
            speech_duration_ms: 0.0,
            last_audio_prob: 0.0,
            last_text_prob: 0.0,
            silence_duration_ms: 0.0,
            high_prob_frames: 0,
            total_turns: 0,
            total_decisions: 0,
        })
    }

    /// Processes incoming signals and returns a turn decision.
    ///
    /// # Arguments
    ///
    /// * `signals` - List of turn signals to process.
    ///
    /// # Returns
    ///
    /// Turn decision with state, probability, and completion status.
    pub fn process(&mut self, signals: &[TurnSignal]) -> TurnDecision {
        let start = Instant::now();
        self.total_decisions += 1;

        // Process each signal type
        let mut vad_is_speech = None;
        let mut vad_silence_duration = 0.0f32;
        let mut audio_prob = None;
        let mut text_prob = None;

        for signal in signals {
            match signal {
                TurnSignal::Silence {
                    duration_ms,
                    is_speech,
                } => {
                    vad_is_speech = Some(*is_speech);
                    vad_silence_duration = *duration_ms;
                }
                TurnSignal::Audio {
                    probability,
                    frames_processed: _,
                } => {
                    if self.config.use_audio {
                        audio_prob = Some(*probability);
                        self.last_audio_prob = *probability;
                    }
                }
                TurnSignal::Text {
                    probability,
                    transcript_len: _,
                } => {
                    if self.config.use_text {
                        text_prob = Some(*probability);
                        self.last_text_prob = *probability;
                    }
                }
            }
        }

        // Update state machine based on VAD
        let now = Instant::now();
        self.update_state_from_vad(vad_is_speech, vad_silence_duration, now);

        // Calculate combined probability
        let combined_prob = self.calculate_combined_probability(audio_prob, text_prob);

        // Determine if turn is complete
        let (is_complete, reason) = self.check_turn_completion(combined_prob);

        // Update hysteresis
        if combined_prob >= self.effective_threshold() {
            self.high_prob_frames += 1;
        } else {
            self.high_prob_frames = 0;
        }

        // Transition to TurnComplete if conditions met
        if is_complete {
            self.state = TurnState::TurnComplete;
            self.total_turns += 1;
        }

        let latency_us = start.elapsed().as_micros() as u64;

        if self.config.debug_logging {
            trace!(
                "TurnDecision: state={:?}, combined={:.4}, audio={:?}, text={:?}, silence={}ms, complete={}, reason={}",
                self.state,
                combined_prob,
                audio_prob,
                text_prob,
                self.silence_duration_ms,
                is_complete,
                reason
            );
        }

        TurnDecision {
            state: self.state,
            is_turn_complete: is_complete,
            combined_probability: combined_prob,
            audio_probability: audio_prob,
            text_probability: text_prob,
            silence_duration_ms: self.silence_duration_ms,
            latency_us,
            reason,
        }
    }

    /// Updates state based on VAD signal.
    fn update_state_from_vad(
        &mut self,
        is_speech: Option<bool>,
        vad_silence_ms: f32,
        now: Instant,
    ) {
        let Some(is_speech) = is_speech else {
            return;
        };

        match self.state {
            TurnState::Idle => {
                if is_speech {
                    // Speech started
                    self.state = TurnState::Speaking;
                    self.speech_start = Some(now);
                    self.silence_start = None;
                    self.silence_duration_ms = 0.0;
                    self.speech_duration_ms = 0.0;

                    if self.config.debug_logging {
                        debug!("Speech started");
                    }
                }
            }
            TurnState::Speaking => {
                if is_speech {
                    // Still speaking
                    if let Some(start) = self.speech_start {
                        self.speech_duration_ms = start.elapsed().as_millis() as f32;
                    }
                    self.silence_duration_ms = 0.0;
                } else {
                    // Silence detected - transition to maybe-turn
                    self.state = TurnState::PausedMaybeTurn;
                    self.silence_start = Some(now);
                    self.silence_duration_ms = vad_silence_ms;

                    if self.config.debug_logging {
                        debug!(
                            "Pause detected after {}ms speech, silence={}ms",
                            self.speech_duration_ms, self.silence_duration_ms
                        );
                    }
                }
            }
            TurnState::PausedMaybeTurn => {
                if is_speech {
                    // Speech resumed - false alarm, back to speaking
                    self.state = TurnState::Speaking;
                    self.silence_start = None;
                    self.silence_duration_ms = 0.0;

                    if self.config.debug_logging {
                        debug!("Speech resumed - was a pause, not turn end");
                    }
                } else {
                    // Still silent
                    if let Some(start) = self.silence_start {
                        self.silence_duration_ms = start.elapsed().as_millis() as f32;
                    } else {
                        self.silence_duration_ms = vad_silence_ms;
                    }
                }
            }
            TurnState::TurnComplete => {
                // Stay in complete until reset
                // If speech detected, transition to new turn
                if is_speech {
                    self.state = TurnState::Speaking;
                    self.speech_start = Some(now);
                    self.silence_start = None;
                    self.silence_duration_ms = 0.0;
                    self.speech_duration_ms = 0.0;
                    self.high_prob_frames = 0;

                    if self.config.debug_logging {
                        debug!("New turn started after completion");
                    }
                }
            }
        }
    }

    /// Calculates combined probability from signals.
    fn calculate_combined_probability(&self, audio: Option<f32>, text: Option<f32>) -> f32 {
        match (audio, text) {
            (Some(a), Some(t)) => {
                // Weighted average
                let total_weight = self.config.audio_weight + self.config.text_weight;
                if total_weight > 0.0 {
                    (a * self.config.audio_weight + t * self.config.text_weight) / total_weight
                } else {
                    a.max(t) // Fallback to max
                }
            }
            (Some(a), None) => a,
            (None, Some(t)) => t,
            (None, None) => 0.0,
        }
    }

    /// Returns the effective threshold based on enabled signals.
    fn effective_threshold(&self) -> f32 {
        if self.config.use_audio && self.config.use_text {
            // When using both, use weighted average of thresholds
            let total_weight = self.config.audio_weight + self.config.text_weight;
            if total_weight > 0.0 {
                (self.config.audio_threshold * self.config.audio_weight
                    + self.config.text_threshold * self.config.text_weight)
                    / total_weight
            } else {
                self.config.audio_threshold.min(self.config.text_threshold)
            }
        } else if self.config.use_audio {
            self.config.audio_threshold
        } else {
            self.config.text_threshold
        }
    }

    /// Checks if turn should be considered complete.
    fn check_turn_completion(&self, combined_prob: f32) -> (bool, String) {
        // Not in a state where turn can complete
        if self.state != TurnState::PausedMaybeTurn {
            return (false, "not in pause state".to_string());
        }

        // Check minimum speech duration
        if self.speech_duration_ms < self.config.min_speech_ms as f32 {
            return (
                false,
                format!(
                    "speech too short: {}ms < {}ms",
                    self.speech_duration_ms, self.config.min_speech_ms
                ),
            );
        }

        // Check minimum silence duration
        if self.silence_duration_ms < self.config.min_silence_ms as f32 {
            return (
                false,
                format!(
                    "silence too short: {}ms < {}ms",
                    self.silence_duration_ms, self.config.min_silence_ms
                ),
            );
        }

        // Force turn on maximum silence
        if self.silence_duration_ms >= self.config.max_silence_ms as f32 {
            return (
                true,
                format!(
                    "max silence exceeded: {}ms >= {}ms",
                    self.silence_duration_ms, self.config.max_silence_ms
                ),
            );
        }

        // Check probability with hysteresis
        let threshold = self.effective_threshold();
        if combined_prob >= threshold && self.high_prob_frames + 1 >= self.config.hysteresis_frames
        {
            return (
                true,
                format!(
                    "probability threshold met: {:.4} >= {:.4} with {} consecutive frames",
                    combined_prob,
                    threshold,
                    self.high_prob_frames + 1
                ),
            );
        }

        (
            false,
            format!(
                "waiting: prob={:.4} < {:.4} or frames={} < {}",
                combined_prob, threshold, self.high_prob_frames, self.config.hysteresis_frames
            ),
        )
    }

    /// Returns the current turn state.
    #[inline]
    pub fn state(&self) -> TurnState {
        self.state
    }

    /// Returns whether currently in speech.
    #[inline]
    pub fn is_speaking(&self) -> bool {
        self.state.is_speaking()
    }

    /// Returns whether we're waiting for turn confirmation.
    #[inline]
    pub fn is_paused(&self) -> bool {
        self.state.is_paused()
    }

    /// Returns current silence duration in milliseconds.
    #[inline]
    pub fn silence_duration_ms(&self) -> f32 {
        self.silence_duration_ms
    }

    /// Returns current speech duration in milliseconds.
    #[inline]
    pub fn speech_duration_ms(&self) -> f32 {
        self.speech_duration_ms
    }

    /// Returns total turns detected.
    #[inline]
    pub fn total_turns(&self) -> u64 {
        self.total_turns
    }

    /// Returns total decisions made.
    #[inline]
    pub fn total_decisions(&self) -> u64 {
        self.total_decisions
    }

    /// Returns the configuration.
    #[inline]
    pub fn config(&self) -> &TurnDecisionEngineConfig {
        &self.config
    }

    /// Resets the engine state for a new conversation.
    pub fn reset(&mut self) {
        self.state = TurnState::Idle;
        self.silence_start = None;
        self.speech_start = None;
        self.speech_duration_ms = 0.0;
        self.last_audio_prob = 0.0;
        self.last_text_prob = 0.0;
        self.silence_duration_ms = 0.0;
        self.high_prob_frames = 0;

        if self.config.debug_logging {
            debug!("TurnDecisionEngine state reset");
        }
    }

    /// Resets statistics only.
    pub fn reset_stats(&mut self) {
        self.total_turns = 0;
        self.total_decisions = 0;
    }

    /// Sets the audio threshold dynamically.
    pub fn set_audio_threshold(&mut self, threshold: f32) {
        self.config.audio_threshold = threshold.clamp(0.0, 1.0);
    }

    /// Sets the text threshold dynamically.
    pub fn set_text_threshold(&mut self, threshold: f32) {
        self.config.text_threshold = threshold.clamp(0.0, 1.0);
    }
}

/// Builder for TurnDecisionEngine.
pub struct TurnDecisionEngineBuilder {
    config: TurnDecisionEngineConfig,
}

impl Default for TurnDecisionEngineBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl TurnDecisionEngineBuilder {
    /// Creates a new builder with default configuration.
    pub fn new() -> Self {
        Self {
            config: TurnDecisionEngineConfig::default(),
        }
    }

    /// Sets the audio threshold.
    pub fn audio_threshold(mut self, threshold: f32) -> Self {
        self.config.audio_threshold = threshold.clamp(0.0, 1.0);
        self
    }

    /// Sets the text threshold.
    pub fn text_threshold(mut self, threshold: f32) -> Self {
        self.config.text_threshold = threshold.clamp(0.0, 1.0);
        self
    }

    /// Sets the minimum silence duration.
    pub fn min_silence_ms(mut self, ms: u32) -> Self {
        self.config.min_silence_ms = ms;
        self
    }

    /// Sets the maximum silence duration.
    pub fn max_silence_ms(mut self, ms: u32) -> Self {
        self.config.max_silence_ms = ms;
        self
    }

    /// Sets the minimum speech duration.
    pub fn min_speech_ms(mut self, ms: u32) -> Self {
        self.config.min_speech_ms = ms;
        self
    }

    /// Enables/disables audio signal.
    pub fn use_audio(mut self, enabled: bool) -> Self {
        self.config.use_audio = enabled;
        self
    }

    /// Enables/disables text signal.
    pub fn use_text(mut self, enabled: bool) -> Self {
        self.config.use_text = enabled;
        self
    }

    /// Sets the ensemble weights.
    pub fn weights(mut self, audio: f32, text: f32) -> Self {
        self.config.audio_weight = audio.clamp(0.0, 1.0);
        self.config.text_weight = text.clamp(0.0, 1.0);
        self
    }

    /// Sets hysteresis frames.
    pub fn hysteresis_frames(mut self, frames: u32) -> Self {
        self.config.hysteresis_frames = frames;
        self
    }

    /// Enables debug logging.
    pub fn debug_logging(mut self, enabled: bool) -> Self {
        self.config.debug_logging = enabled;
        self
    }

    /// Builds the engine.
    pub fn build(self) -> Result<TurnDecisionEngine> {
        TurnDecisionEngine::new(self.config)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // Configuration tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_default_config() {
        let config = TurnDecisionEngineConfig::default();

        assert_eq!(config.audio_threshold, 0.7);
        assert_eq!(config.text_threshold, 0.8);
        assert_eq!(config.min_silence_ms, 200);
        assert_eq!(config.max_silence_ms, 2000);
        assert_eq!(config.min_speech_ms, 300);
        assert_eq!(config.audio_weight, 0.7);
        assert_eq!(config.text_weight, 0.3);
        assert!(config.use_audio);
        assert!(!config.use_text);
        assert!(!config.debug_logging);
    }

    #[test]
    fn test_config_validation_valid() {
        let config = TurnDecisionEngineConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validation_invalid_audio_threshold() {
        let config = TurnDecisionEngineConfig {
            audio_threshold: 1.5,
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_invalid_text_threshold() {
        let config = TurnDecisionEngineConfig {
            text_threshold: -0.1,
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_silence_order() {
        let config = TurnDecisionEngineConfig {
            min_silence_ms: 3000,
            max_silence_ms: 1000,
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_no_signals() {
        let config = TurnDecisionEngineConfig {
            use_audio: false,
            use_text: false,
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_builder_pattern() {
        let config = TurnDecisionEngineConfig::new()
            .with_audio_threshold(0.6)
            .with_text_threshold(0.75)
            .with_min_silence_ms(150)
            .with_max_silence_ms(3000)
            .with_min_speech_ms(200)
            .with_audio(true)
            .with_text(true)
            .with_weights(0.6, 0.4)
            .with_debug_logging();

        assert_eq!(config.audio_threshold, 0.6);
        assert_eq!(config.text_threshold, 0.75);
        assert_eq!(config.min_silence_ms, 150);
        assert_eq!(config.max_silence_ms, 3000);
        assert_eq!(config.min_speech_ms, 200);
        assert!(config.use_audio);
        assert!(config.use_text);
        assert_eq!(config.audio_weight, 0.6);
        assert_eq!(config.text_weight, 0.4);
        assert!(config.debug_logging);
    }

    // -------------------------------------------------------------------------
    // Engine construction tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_engine_construction() {
        let config = TurnDecisionEngineConfig::default();
        let engine = TurnDecisionEngine::new(config);
        assert!(engine.is_ok());

        let e = engine.unwrap();
        assert_eq!(e.state(), TurnState::Idle);
        assert_eq!(e.total_turns(), 0);
        assert_eq!(e.total_decisions(), 0);
    }

    #[test]
    fn test_engine_construction_invalid_config() {
        let config = TurnDecisionEngineConfig {
            use_audio: false,
            use_text: false,
            ..Default::default()
        };
        let engine = TurnDecisionEngine::new(config);
        assert!(engine.is_err());
    }

    // -------------------------------------------------------------------------
    // State machine tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_state_idle_to_speaking() {
        let config = TurnDecisionEngineConfig::default();
        let mut engine = TurnDecisionEngine::new(config).unwrap();

        assert_eq!(engine.state(), TurnState::Idle);

        // Speech detected
        let decision = engine.process(&[TurnSignal::Silence {
            duration_ms: 0.0,
            is_speech: true,
        }]);

        assert_eq!(engine.state(), TurnState::Speaking);
        assert!(!decision.is_turn_complete);
    }

    #[test]
    fn test_state_speaking_to_paused() {
        let config = TurnDecisionEngineConfig::default();
        let mut engine = TurnDecisionEngine::new(config).unwrap();

        // Start speaking
        engine.process(&[TurnSignal::Silence {
            duration_ms: 0.0,
            is_speech: true,
        }]);

        // Simulate some speech time
        std::thread::sleep(std::time::Duration::from_millis(10));

        // Silence detected
        let decision = engine.process(&[TurnSignal::Silence {
            duration_ms: 50.0,
            is_speech: false,
        }]);

        assert_eq!(engine.state(), TurnState::PausedMaybeTurn);
        assert!(!decision.is_turn_complete);
    }

    #[test]
    fn test_state_paused_back_to_speaking() {
        let config = TurnDecisionEngineConfig::default();
        let mut engine = TurnDecisionEngine::new(config).unwrap();

        // Start speaking
        engine.process(&[TurnSignal::Silence {
            duration_ms: 0.0,
            is_speech: true,
        }]);

        // Pause
        engine.process(&[TurnSignal::Silence {
            duration_ms: 50.0,
            is_speech: false,
        }]);

        // Speech resumes
        let decision = engine.process(&[TurnSignal::Silence {
            duration_ms: 0.0,
            is_speech: true,
        }]);

        assert_eq!(engine.state(), TurnState::Speaking);
        assert!(!decision.is_turn_complete);
    }

    #[test]
    fn test_turn_complete_max_silence() {
        let config = TurnDecisionEngineConfig {
            min_silence_ms: 100,
            max_silence_ms: 500,
            min_speech_ms: 0, // Disable for test
            ..Default::default()
        };
        let mut engine = TurnDecisionEngine::new(config).unwrap();

        // Start speaking
        engine.process(&[TurnSignal::Silence {
            duration_ms: 0.0,
            is_speech: true,
        }]);

        // Pause
        engine.process(&[TurnSignal::Silence {
            duration_ms: 100.0,
            is_speech: false,
        }]);

        assert_eq!(engine.state(), TurnState::PausedMaybeTurn);

        // Wait for silence to exceed max
        std::thread::sleep(std::time::Duration::from_millis(600));

        let decision = engine.process(&[TurnSignal::Silence {
            duration_ms: 600.0,
            is_speech: false,
        }]);

        assert!(decision.is_turn_complete);
        assert_eq!(engine.state(), TurnState::TurnComplete);
        assert!(decision.reason.contains("max silence"));
    }

    // -------------------------------------------------------------------------
    // Probability calculation tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_combined_probability_audio_only() {
        let config = TurnDecisionEngineConfig {
            use_audio: true,
            use_text: false,
            min_speech_ms: 0,
            min_silence_ms: 0,
            ..Default::default()
        };
        let mut engine = TurnDecisionEngine::new(config).unwrap();

        // Start speaking and pause
        engine.process(&[TurnSignal::Silence {
            duration_ms: 0.0,
            is_speech: true,
        }]);
        engine.process(&[TurnSignal::Silence {
            duration_ms: 100.0,
            is_speech: false,
        }]);

        // Provide audio signal
        let decision = engine.process(&[
            TurnSignal::Silence {
                duration_ms: 200.0,
                is_speech: false,
            },
            TurnSignal::Audio {
                probability: 0.85,
                frames_processed: 100,
            },
        ]);

        assert_eq!(decision.combined_probability, 0.85);
        assert_eq!(decision.audio_probability, Some(0.85));
        assert!(decision.text_probability.is_none());
    }

    #[test]
    fn test_combined_probability_ensemble() {
        let config = TurnDecisionEngineConfig {
            use_audio: true,
            use_text: true,
            audio_weight: 0.7,
            text_weight: 0.3,
            min_speech_ms: 0,
            min_silence_ms: 0,
            ..Default::default()
        };
        let mut engine = TurnDecisionEngine::new(config).unwrap();

        // Setup state
        engine.process(&[TurnSignal::Silence {
            duration_ms: 0.0,
            is_speech: true,
        }]);
        engine.process(&[TurnSignal::Silence {
            duration_ms: 100.0,
            is_speech: false,
        }]);

        // Provide both signals
        let decision = engine.process(&[
            TurnSignal::Silence {
                duration_ms: 200.0,
                is_speech: false,
            },
            TurnSignal::Audio {
                probability: 0.8,
                frames_processed: 100,
            },
            TurnSignal::Text {
                probability: 0.9,
                transcript_len: 50,
            },
        ]);

        // Expected: (0.8 * 0.7 + 0.9 * 0.3) / 1.0 = 0.56 + 0.27 = 0.83
        let expected = (0.8 * 0.7 + 0.9 * 0.3) / (0.7 + 0.3);
        assert!((decision.combined_probability - expected).abs() < 0.001);
    }

    // -------------------------------------------------------------------------
    // Reset tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_reset() {
        let config = TurnDecisionEngineConfig::default();
        let mut engine = TurnDecisionEngine::new(config).unwrap();

        // Do some processing
        engine.process(&[TurnSignal::Silence {
            duration_ms: 0.0,
            is_speech: true,
        }]);
        engine.process(&[TurnSignal::Silence {
            duration_ms: 100.0,
            is_speech: false,
        }]);

        assert_ne!(engine.state(), TurnState::Idle);

        // Reset
        engine.reset();

        assert_eq!(engine.state(), TurnState::Idle);
        assert_eq!(engine.silence_duration_ms(), 0.0);
        assert_eq!(engine.speech_duration_ms(), 0.0);
    }

    // -------------------------------------------------------------------------
    // Builder tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_builder() {
        let engine = TurnDecisionEngineBuilder::new()
            .audio_threshold(0.6)
            .text_threshold(0.75)
            .min_silence_ms(150)
            .max_silence_ms(3000)
            .min_speech_ms(200)
            .use_audio(true)
            .use_text(true)
            .weights(0.6, 0.4)
            .hysteresis_frames(3)
            .debug_logging(true)
            .build();

        assert!(engine.is_ok());
        let e = engine.unwrap();
        assert_eq!(e.config().audio_threshold, 0.6);
        assert_eq!(e.config().text_threshold, 0.75);
    }

    // -------------------------------------------------------------------------
    // TurnState tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_turn_state_methods() {
        assert!(!TurnState::Idle.is_speaking());
        assert!(!TurnState::Idle.is_paused());
        assert!(!TurnState::Idle.is_turn_complete());

        assert!(TurnState::Speaking.is_speaking());
        assert!(!TurnState::Speaking.is_paused());
        assert!(!TurnState::Speaking.is_turn_complete());

        assert!(!TurnState::PausedMaybeTurn.is_speaking());
        assert!(TurnState::PausedMaybeTurn.is_paused());
        assert!(!TurnState::PausedMaybeTurn.is_turn_complete());

        assert!(!TurnState::TurnComplete.is_speaking());
        assert!(!TurnState::TurnComplete.is_paused());
        assert!(TurnState::TurnComplete.is_turn_complete());
    }

    // -------------------------------------------------------------------------
    // TurnDecision tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_turn_decision_default() {
        let decision = TurnDecision::default();

        assert_eq!(decision.state, TurnState::Idle);
        assert!(!decision.is_turn_complete);
        assert_eq!(decision.combined_probability, 0.0);
        assert!(decision.audio_probability.is_none());
        assert!(decision.text_probability.is_none());
    }
}
