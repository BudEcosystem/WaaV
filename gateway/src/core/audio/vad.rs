//! Voice Activity Detection (VAD) state machine with configurable debouncing.
//!
//! This module provides a local VAD state machine independent of STT provider,
//! useful for:
//! - Faster interruption detection
//! - Provider-agnostic speech boundary detection
//! - Custom debouncing behavior
//!
//! # State Machine
//!
//! ```text
//! ┌─────────┐  voice   ┌──────────┐  debounce  ┌──────────┐
//! │  QUIET  ├─────────►│ STARTING ├───────────►│ SPEAKING │
//! └────┬────┘          └────┬─────┘            └────┬─────┘
//!      │                    │                       │
//!      │◄───── silence ─────┘                       │
//!      │                                            │
//!      │              ┌──────────┐                  │
//!      │◄── debounce ─┤ STOPPING │◄──── silence ───┘
//!      │              └────┬─────┘
//!      │                   │
//!      └────── voice ──────┘
//! ```
//!
//! # Example
//!
//! ```rust
//! use waav_gateway::core::audio::{VADAnalyzer, VADParams, VADTransition};
//!
//! let mut vad = VADAnalyzer::new(VADParams::default());
//!
//! // Process audio frames
//! let (state, transition) = vad.analyze(0.8, 0.7); // confidence, volume
//!
//! if let Some(VADTransition::SpeechStarted) = transition {
//!     println!("User started speaking!");
//! }
//! ```

// =============================================================================
// VAD State
// =============================================================================

/// VAD state representing the current speech detection status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
#[derive(Default)]
pub enum VADState {
    /// No speech detected
    #[default]
    Quiet = 0,
    /// Speech beginning, within start debounce window
    Starting = 1,
    /// Confirmed speech in progress
    Speaking = 2,
    /// Speech ending, within stop debounce window
    Stopping = 3,
}


impl From<u8> for VADState {
    fn from(v: u8) -> Self {
        match v {
            0 => VADState::Quiet,
            1 => VADState::Starting,
            2 => VADState::Speaking,
            3 => VADState::Stopping,
            _ => VADState::Quiet,
        }
    }
}

impl std::fmt::Display for VADState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VADState::Quiet => write!(f, "QUIET"),
            VADState::Starting => write!(f, "STARTING"),
            VADState::Speaking => write!(f, "SPEAKING"),
            VADState::Stopping => write!(f, "STOPPING"),
        }
    }
}

// =============================================================================
// VAD Transition
// =============================================================================

/// State transition events emitted by the VAD analyzer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VADTransition {
    /// Speech started (after debounce confirmation)
    SpeechStarted,
    /// Speech ended (after debounce confirmation)
    SpeechEnded,
}

// =============================================================================
// VAD Parameters
// =============================================================================

/// Configuration parameters for VAD behavior.
#[derive(Debug, Clone)]
pub struct VADParams {
    /// Voice confidence threshold (0.0 to 1.0)
    /// Frames with confidence >= this are considered voice
    pub confidence_threshold: f32,

    /// Minimum consecutive frames to confirm speech start
    /// Higher values reduce false positives from noise bursts
    pub start_debounce_frames: u32,

    /// Minimum consecutive silent frames to confirm speech end
    /// Higher values allow for natural pauses in speech
    pub stop_debounce_frames: u32,

    /// Minimum audio volume/energy to consider (0.0 to 1.0)
    /// Helps filter out low-level noise
    pub min_volume: f32,
}

impl Default for VADParams {
    fn default() -> Self {
        Self {
            confidence_threshold: 0.7,
            start_debounce_frames: 4, // ~200ms at 20fps (50ms frames)
            stop_debounce_frames: 16, // ~800ms at 20fps
            min_volume: 0.6,
        }
    }
}

impl VADParams {
    /// Create aggressive parameters for fast detection.
    ///
    /// Good for low-latency applications where false positives are acceptable.
    pub fn aggressive() -> Self {
        Self {
            confidence_threshold: 0.5,
            start_debounce_frames: 2,
            stop_debounce_frames: 8,
            min_volume: 0.4,
        }
    }

    /// Create conservative parameters for fewer false positives.
    ///
    /// Good for noisy environments or high-accuracy requirements.
    pub fn conservative() -> Self {
        Self {
            confidence_threshold: 0.8,
            start_debounce_frames: 6,
            stop_debounce_frames: 24,
            min_volume: 0.7,
        }
    }

    /// Create custom parameters with builder pattern.
    pub fn with_confidence_threshold(mut self, threshold: f32) -> Self {
        self.confidence_threshold = threshold.clamp(0.0, 1.0);
        self
    }

    /// Set start debounce frames.
    pub fn with_start_debounce(mut self, frames: u32) -> Self {
        self.start_debounce_frames = frames;
        self
    }

    /// Set stop debounce frames.
    pub fn with_stop_debounce(mut self, frames: u32) -> Self {
        self.stop_debounce_frames = frames;
        self
    }

    /// Set minimum volume threshold.
    pub fn with_min_volume(mut self, volume: f32) -> Self {
        self.min_volume = volume.clamp(0.0, 1.0);
        self
    }
}

// =============================================================================
// VAD Analyzer
// =============================================================================

/// Voice Activity Detection analyzer with state machine and debouncing.
///
/// # Thread Safety
///
/// This type is NOT thread-safe. Use external synchronization (e.g., Mutex)
/// if sharing across threads. Typically used in a single audio processing thread.
///
/// # Usage Pattern
///
/// ```rust
/// let mut vad = VADAnalyzer::new(VADParams::default());
///
/// // For each audio frame (e.g., every 20-50ms):
/// let confidence = compute_voice_confidence(&frame);
/// let volume = compute_rms(&frame);
///
/// let (state, transition) = vad.analyze(confidence, volume);
///
/// match transition {
///     Some(VADTransition::SpeechStarted) => start_recording(),
///     Some(VADTransition::SpeechEnded) => stop_recording(),
///     None => {} // No state change
/// }
/// ```
#[derive(Debug, Clone)]
pub struct VADAnalyzer {
    state: VADState,
    params: VADParams,
    starting_count: u32,
    stopping_count: u32,
}

impl VADAnalyzer {
    /// Create a new VAD analyzer with the specified parameters.
    pub fn new(params: VADParams) -> Self {
        Self {
            state: VADState::Quiet,
            params,
            starting_count: 0,
            stopping_count: 0,
        }
    }

    /// Process an audio frame and return state and optional transition.
    ///
    /// # Arguments
    /// * `confidence` - Voice confidence score from VAD model (0.0 to 1.0)
    /// * `volume` - Audio volume/RMS (0.0 to 1.0)
    ///
    /// # Returns
    /// * Tuple of (current_state, Option<transition_event>)
    pub fn analyze(&mut self, confidence: f32, volume: f32) -> (VADState, Option<VADTransition>) {
        let is_voice =
            confidence >= self.params.confidence_threshold && volume >= self.params.min_volume;

        let _old_state = self.state;

        match (self.state, is_voice) {
            // QUIET + voice → start debouncing (or transition immediately if debounce = 1)
            (VADState::Quiet, true) => {
                self.starting_count = 1;
                if self.starting_count >= self.params.start_debounce_frames {
                    self.state = VADState::Speaking;
                    return (self.state, Some(VADTransition::SpeechStarted));
                }
                self.state = VADState::Starting;
            }

            // STARTING + voice → increment counter, maybe transition to SPEAKING
            (VADState::Starting, true) => {
                self.starting_count += 1;
                if self.starting_count >= self.params.start_debounce_frames {
                    self.state = VADState::Speaking;
                    return (self.state, Some(VADTransition::SpeechStarted));
                }
            }

            // STARTING + no voice → reset to quiet (false start)
            (VADState::Starting, false) => {
                self.state = VADState::Quiet;
                self.starting_count = 0;
            }

            // SPEAKING + no voice → start stop debouncing (or transition immediately if debounce = 1)
            (VADState::Speaking, false) => {
                self.stopping_count = 1;
                if self.stopping_count >= self.params.stop_debounce_frames {
                    self.state = VADState::Quiet;
                    return (self.state, Some(VADTransition::SpeechEnded));
                }
                self.state = VADState::Stopping;
            }

            // STOPPING + no voice → increment counter, maybe transition to QUIET
            (VADState::Stopping, false) => {
                self.stopping_count += 1;
                if self.stopping_count >= self.params.stop_debounce_frames {
                    self.state = VADState::Quiet;
                    return (self.state, Some(VADTransition::SpeechEnded));
                }
            }

            // STOPPING + voice → back to speaking (user continued)
            (VADState::Stopping, true) => {
                self.state = VADState::Speaking;
                self.stopping_count = 0;
            }

            // QUIET + no voice, SPEAKING + voice → no state change
            (VADState::Quiet, false) | (VADState::Speaking, true) => {}
        }

        (self.state, None)
    }

    /// Get the current VAD state.
    #[inline]
    pub fn state(&self) -> VADState {
        self.state
    }

    /// Check if currently in a speaking state (SPEAKING or STOPPING).
    #[inline]
    pub fn is_speaking(&self) -> bool {
        matches!(self.state, VADState::Speaking | VADState::Stopping)
    }

    /// Check if voice is currently detected (STARTING, SPEAKING, or resuming from STOPPING).
    #[inline]
    pub fn is_voice_detected(&self) -> bool {
        matches!(self.state, VADState::Starting | VADState::Speaking)
    }

    /// Reset the analyzer to quiet state.
    pub fn reset(&mut self) {
        self.state = VADState::Quiet;
        self.starting_count = 0;
        self.stopping_count = 0;
    }

    /// Get the current parameters.
    pub fn params(&self) -> &VADParams {
        &self.params
    }

    /// Update parameters (analyzer state is preserved).
    pub fn set_params(&mut self, params: VADParams) {
        self.params = params;
    }

    /// Get number of frames in current debounce window.
    pub fn debounce_progress(&self) -> (u32, u32) {
        match self.state {
            VADState::Starting => (self.starting_count, self.params.start_debounce_frames),
            VADState::Stopping => (self.stopping_count, self.params.stop_debounce_frames),
            _ => (0, 0),
        }
    }
}

impl Default for VADAnalyzer {
    fn default() -> Self {
        Self::new(VADParams::default())
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // VADState Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_vad_state_default() {
        assert_eq!(VADState::default(), VADState::Quiet);
    }

    #[test]
    fn test_vad_state_from_u8() {
        assert_eq!(VADState::from(0), VADState::Quiet);
        assert_eq!(VADState::from(1), VADState::Starting);
        assert_eq!(VADState::from(2), VADState::Speaking);
        assert_eq!(VADState::from(3), VADState::Stopping);
        assert_eq!(VADState::from(255), VADState::Quiet); // Invalid defaults to Quiet
    }

    #[test]
    fn test_vad_state_display() {
        assert_eq!(format!("{}", VADState::Quiet), "QUIET");
        assert_eq!(format!("{}", VADState::Starting), "STARTING");
        assert_eq!(format!("{}", VADState::Speaking), "SPEAKING");
        assert_eq!(format!("{}", VADState::Stopping), "STOPPING");
    }

    // -------------------------------------------------------------------------
    // VADParams Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_vad_params_default() {
        let params = VADParams::default();
        assert_eq!(params.confidence_threshold, 0.7);
        assert_eq!(params.start_debounce_frames, 4);
        assert_eq!(params.stop_debounce_frames, 16);
        assert_eq!(params.min_volume, 0.6);
    }

    #[test]
    fn test_vad_params_aggressive() {
        let params = VADParams::aggressive();
        assert!(params.confidence_threshold < VADParams::default().confidence_threshold);
        assert!(params.start_debounce_frames < VADParams::default().start_debounce_frames);
    }

    #[test]
    fn test_vad_params_conservative() {
        let params = VADParams::conservative();
        assert!(params.confidence_threshold > VADParams::default().confidence_threshold);
        assert!(params.start_debounce_frames > VADParams::default().start_debounce_frames);
    }

    #[test]
    fn test_vad_params_builder() {
        let params = VADParams::default()
            .with_confidence_threshold(0.5)
            .with_start_debounce(2)
            .with_stop_debounce(10)
            .with_min_volume(0.3);

        assert_eq!(params.confidence_threshold, 0.5);
        assert_eq!(params.start_debounce_frames, 2);
        assert_eq!(params.stop_debounce_frames, 10);
        assert_eq!(params.min_volume, 0.3);
    }

    #[test]
    fn test_vad_params_clamping() {
        let params = VADParams::default()
            .with_confidence_threshold(1.5) // Should clamp to 1.0
            .with_min_volume(-0.5); // Should clamp to 0.0

        assert_eq!(params.confidence_threshold, 1.0);
        assert_eq!(params.min_volume, 0.0);
    }

    // -------------------------------------------------------------------------
    // VADAnalyzer Construction Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_analyzer_creation() {
        let vad = VADAnalyzer::new(VADParams::default());
        assert_eq!(vad.state(), VADState::Quiet);
        assert!(!vad.is_speaking());
        assert!(!vad.is_voice_detected());
    }

    #[test]
    fn test_analyzer_default() {
        let vad = VADAnalyzer::default();
        assert_eq!(vad.state(), VADState::Quiet);
    }

    // -------------------------------------------------------------------------
    // State Transition: QUIET → STARTING → SPEAKING
    // -------------------------------------------------------------------------

    #[test]
    fn test_quiet_to_starting() {
        let mut vad = VADAnalyzer::new(
            VADParams::default()
                .with_confidence_threshold(0.5)
                .with_min_volume(0.5),
        );

        // Voice detected → STARTING
        let (state, transition) = vad.analyze(0.8, 0.7);
        assert_eq!(state, VADState::Starting);
        assert!(transition.is_none()); // No transition yet
    }

    #[test]
    fn test_starting_to_speaking() {
        let mut vad = VADAnalyzer::new(
            VADParams::default()
                .with_confidence_threshold(0.5)
                .with_min_volume(0.5)
                .with_start_debounce(3),
        );

        // First voice frame → STARTING
        vad.analyze(0.8, 0.7);
        assert_eq!(vad.state(), VADState::Starting);

        // Second voice frame → still STARTING
        vad.analyze(0.8, 0.7);
        assert_eq!(vad.state(), VADState::Starting);

        // Third voice frame → SPEAKING (transition!)
        let (state, transition) = vad.analyze(0.8, 0.7);
        assert_eq!(state, VADState::Speaking);
        assert_eq!(transition, Some(VADTransition::SpeechStarted));
    }

    #[test]
    fn test_starting_interrupted_by_silence() {
        let mut vad = VADAnalyzer::new(
            VADParams::default()
                .with_confidence_threshold(0.5)
                .with_min_volume(0.5)
                .with_start_debounce(3),
        );

        // Voice detected → STARTING
        vad.analyze(0.8, 0.7);
        assert_eq!(vad.state(), VADState::Starting);

        // Silence → back to QUIET (false start)
        let (state, transition) = vad.analyze(0.2, 0.7);
        assert_eq!(state, VADState::Quiet);
        assert!(transition.is_none());
    }

    // -------------------------------------------------------------------------
    // State Transition: SPEAKING → STOPPING → QUIET
    // -------------------------------------------------------------------------

    #[test]
    fn test_speaking_to_stopping() {
        let mut vad = VADAnalyzer::new(
            VADParams::default()
                .with_confidence_threshold(0.5)
                .with_min_volume(0.5)
                .with_start_debounce(1),
        ); // Quick start for test

        // Get to SPEAKING state
        vad.analyze(0.8, 0.7);
        assert_eq!(vad.state(), VADState::Speaking);

        // Silence → STOPPING
        let (state, transition) = vad.analyze(0.2, 0.7);
        assert_eq!(state, VADState::Stopping);
        assert!(transition.is_none()); // No transition yet
    }

    #[test]
    fn test_stopping_to_quiet() {
        let mut vad = VADAnalyzer::new(
            VADParams::default()
                .with_confidence_threshold(0.5)
                .with_min_volume(0.5)
                .with_start_debounce(1)
                .with_stop_debounce(3),
        );

        // Get to SPEAKING
        vad.analyze(0.8, 0.7);

        // First silence → STOPPING
        vad.analyze(0.2, 0.7);
        assert_eq!(vad.state(), VADState::Stopping);

        // Second silence → still STOPPING
        vad.analyze(0.2, 0.7);

        // Third silence → QUIET (transition!)
        let (state, transition) = vad.analyze(0.2, 0.7);
        assert_eq!(state, VADState::Quiet);
        assert_eq!(transition, Some(VADTransition::SpeechEnded));
    }

    #[test]
    fn test_stopping_interrupted_by_voice() {
        let mut vad = VADAnalyzer::new(
            VADParams::default()
                .with_confidence_threshold(0.5)
                .with_min_volume(0.5)
                .with_start_debounce(1)
                .with_stop_debounce(5),
        );

        // Get to SPEAKING
        vad.analyze(0.8, 0.7);

        // Silence → STOPPING
        vad.analyze(0.2, 0.7);
        vad.analyze(0.2, 0.7);
        assert_eq!(vad.state(), VADState::Stopping);

        // Voice returns → back to SPEAKING
        let (state, transition) = vad.analyze(0.8, 0.7);
        assert_eq!(state, VADState::Speaking);
        assert!(transition.is_none()); // No transition, just resumed
    }

    // -------------------------------------------------------------------------
    // Edge Cases: Below Thresholds
    // -------------------------------------------------------------------------

    #[test]
    fn test_confidence_below_threshold() {
        let mut vad = VADAnalyzer::new(
            VADParams::default()
                .with_confidence_threshold(0.7)
                .with_min_volume(0.5),
        );

        // Confidence below threshold → stays QUIET
        let (state, transition) = vad.analyze(0.6, 0.8);
        assert_eq!(state, VADState::Quiet);
        assert!(transition.is_none());
    }

    #[test]
    fn test_volume_below_threshold() {
        let mut vad = VADAnalyzer::new(
            VADParams::default()
                .with_confidence_threshold(0.5)
                .with_min_volume(0.7),
        );

        // Volume below threshold → stays QUIET
        let (state, transition) = vad.analyze(0.9, 0.5);
        assert_eq!(state, VADState::Quiet);
        assert!(transition.is_none());
    }

    #[test]
    fn test_both_thresholds_must_be_met() {
        let mut vad = VADAnalyzer::new(
            VADParams::default()
                .with_confidence_threshold(0.5)
                .with_min_volume(0.5)
                .with_start_debounce(1),
        );

        // High confidence, low volume → no detection
        vad.analyze(0.9, 0.3);
        assert_eq!(vad.state(), VADState::Quiet);

        // Low confidence, high volume → no detection
        vad.analyze(0.3, 0.9);
        assert_eq!(vad.state(), VADState::Quiet);

        // Both above threshold → detection
        vad.analyze(0.6, 0.6);
        assert_eq!(vad.state(), VADState::Speaking);
    }

    // -------------------------------------------------------------------------
    // Reset Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_reset_from_speaking() {
        let mut vad = VADAnalyzer::new(VADParams::default().with_start_debounce(1));

        // Get to SPEAKING
        vad.analyze(0.8, 0.8);
        assert_eq!(vad.state(), VADState::Speaking);

        // Reset
        vad.reset();
        assert_eq!(vad.state(), VADState::Quiet);
        assert!(!vad.is_speaking());
    }

    #[test]
    fn test_reset_clears_counters() {
        let mut vad = VADAnalyzer::new(VADParams::default().with_start_debounce(5));

        // Partially through start debounce
        vad.analyze(0.8, 0.8);
        vad.analyze(0.8, 0.8);
        vad.analyze(0.8, 0.8);

        let (progress, total) = vad.debounce_progress();
        assert_eq!(progress, 3);
        assert_eq!(total, 5);

        // Reset
        vad.reset();

        let (progress, total) = vad.debounce_progress();
        assert_eq!(progress, 0);
        assert_eq!(total, 0); // Quiet state has no debounce
    }

    // -------------------------------------------------------------------------
    // Helper Method Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_is_speaking() {
        let mut vad = VADAnalyzer::new(VADParams::default().with_start_debounce(1));

        assert!(!vad.is_speaking());

        vad.analyze(0.8, 0.8); // → SPEAKING
        assert!(vad.is_speaking());

        vad.analyze(0.2, 0.2); // → STOPPING
        assert!(vad.is_speaking()); // Still "speaking" during stopping

        // Continue silence until QUIET
        for _ in 0..20 {
            vad.analyze(0.2, 0.2);
        }
        assert!(!vad.is_speaking());
    }

    #[test]
    fn test_is_voice_detected() {
        let mut vad = VADAnalyzer::new(VADParams::default().with_start_debounce(3));

        assert!(!vad.is_voice_detected());

        vad.analyze(0.8, 0.8); // → STARTING
        assert!(vad.is_voice_detected());

        vad.analyze(0.8, 0.8);
        vad.analyze(0.8, 0.8); // → SPEAKING
        assert!(vad.is_voice_detected());
    }

    #[test]
    fn test_debounce_progress() {
        let mut vad = VADAnalyzer::new(
            VADParams::default()
                .with_start_debounce(5)
                .with_stop_debounce(10),
        );

        // QUIET state
        assert_eq!(vad.debounce_progress(), (0, 0));

        // STARTING state
        vad.analyze(0.8, 0.8);
        assert_eq!(vad.debounce_progress(), (1, 5));

        vad.analyze(0.8, 0.8);
        vad.analyze(0.8, 0.8);
        assert_eq!(vad.debounce_progress(), (3, 5));

        // Continue to SPEAKING
        vad.analyze(0.8, 0.8);
        vad.analyze(0.8, 0.8);
        assert_eq!(vad.state(), VADState::Speaking);
        assert_eq!(vad.debounce_progress(), (0, 0)); // SPEAKING has no debounce

        // STOPPING state
        vad.analyze(0.2, 0.2);
        assert_eq!(vad.debounce_progress(), (1, 10));
    }

    #[test]
    fn test_set_params() {
        let mut vad = VADAnalyzer::new(VADParams::default());

        let new_params = VADParams::aggressive();
        vad.set_params(new_params.clone());

        assert_eq!(
            vad.params().confidence_threshold,
            new_params.confidence_threshold
        );
    }

    // -------------------------------------------------------------------------
    // Comprehensive Sequence Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_typical_speech_sequence() {
        let mut vad = VADAnalyzer::new(
            VADParams::default()
                .with_confidence_threshold(0.5)
                .with_min_volume(0.5)
                .with_start_debounce(3)
                .with_stop_debounce(5),
        );

        let mut transitions = vec![];

        // User starts speaking
        for _ in 0..3 {
            let (_, t) = vad.analyze(0.8, 0.7);
            if let Some(tr) = t {
                transitions.push(tr);
            }
        }

        // Confirm speech started
        assert_eq!(transitions.len(), 1);
        assert_eq!(transitions[0], VADTransition::SpeechStarted);

        // User continues speaking
        for _ in 0..10 {
            let (_, t) = vad.analyze(0.8, 0.7);
            assert!(t.is_none()); // No new transitions
        }

        // User pauses briefly (but not long enough to end)
        for _ in 0..3 {
            let (_, t) = vad.analyze(0.3, 0.7);
            assert!(t.is_none());
        }

        // User continues speaking
        for _ in 0..5 {
            let (_, t) = vad.analyze(0.8, 0.7);
            assert!(t.is_none());
        }

        // User stops speaking
        for _ in 0..5 {
            let (_, t) = vad.analyze(0.2, 0.7);
            if let Some(tr) = t {
                transitions.push(tr);
            }
        }

        // Confirm speech ended
        assert_eq!(transitions.len(), 2);
        assert_eq!(transitions[1], VADTransition::SpeechEnded);
    }

    #[test]
    fn test_noisy_input() {
        let mut vad = VADAnalyzer::new(
            VADParams::default()
                .with_confidence_threshold(0.6)
                .with_min_volume(0.5)
                .with_start_debounce(4),
        );

        // Simulate noisy input with intermittent voice detection
        let noise_pattern = [0.7, 0.3, 0.2, 0.5, 0.4, 0.3, 0.6, 0.2];

        for &conf in &noise_pattern {
            vad.analyze(conf, 0.7);
        }

        // Should not trigger speech start due to inconsistent voice
        assert_eq!(vad.state(), VADState::Quiet);
    }

    #[test]
    fn test_rapid_on_off_speech() {
        let mut vad = VADAnalyzer::new(
            VADParams::default()
                .with_start_debounce(2)
                .with_stop_debounce(3),
        );

        let mut started_count = 0;
        let mut ended_count = 0;

        // Multiple short utterances
        for _ in 0..5 {
            // Start speaking
            for _ in 0..3 {
                let (_, t) = vad.analyze(0.9, 0.8);
                if t == Some(VADTransition::SpeechStarted) {
                    started_count += 1;
                }
            }

            // Stop speaking
            for _ in 0..5 {
                let (_, t) = vad.analyze(0.1, 0.2);
                if t == Some(VADTransition::SpeechEnded) {
                    ended_count += 1;
                }
            }
        }

        // Should have 5 start/end cycles
        assert_eq!(started_count, 5);
        assert_eq!(ended_count, 5);
    }

    // -------------------------------------------------------------------------
    // Boundary Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_exact_threshold_values() {
        let mut vad = VADAnalyzer::new(
            VADParams::default()
                .with_confidence_threshold(0.7)
                .with_min_volume(0.6)
                .with_start_debounce(1),
        );

        // Exactly at threshold should trigger
        let (state, _) = vad.analyze(0.7, 0.6);
        assert_eq!(state, VADState::Speaking);

        vad.reset();

        // Just below threshold should not trigger
        vad.analyze(0.69, 0.6);
        assert_eq!(vad.state(), VADState::Quiet);

        vad.analyze(0.7, 0.59);
        assert_eq!(vad.state(), VADState::Quiet);
    }

    #[test]
    fn test_zero_debounce() {
        let _vad = VADAnalyzer::new(
            VADParams::default()
                .with_start_debounce(0)
                .with_stop_debounce(0),
        );

        // With zero debounce, should transition immediately
        // Note: start_debounce of 0 means condition >= 0, which is always true
        // This is an edge case that may need special handling in production
    }

    #[test]
    fn test_single_frame_debounce() {
        let mut vad = VADAnalyzer::new(
            VADParams::default()
                .with_start_debounce(1)
                .with_stop_debounce(1),
        );

        // Single frame should trigger start
        let (_, t) = vad.analyze(0.9, 0.9);
        assert_eq!(t, Some(VADTransition::SpeechStarted));

        // Single frame should trigger end
        let (_, t) = vad.analyze(0.1, 0.1);
        assert_eq!(t, Some(VADTransition::SpeechEnded));
    }
}
