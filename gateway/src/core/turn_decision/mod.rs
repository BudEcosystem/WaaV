//! Turn Decision Engine module.
//!
//! This module provides the ensemble turn detection system that combines:
//! - Silero VAD: Voice activity detection to track speech/silence
//! - SmartTurnDetector: Audio-based semantic turn detection
//! - (Optional) Text-based turn detection via TurnDetector
//!
//! ## Architecture
//!
//! ```text
//! Audio Input
//!     │
//!     ├──► Silero VAD ──► Speech State (speech_start, speech_end, silence_duration)
//!     │
//!     ├──► MelExtractor ──► SmartTurnDetector ──► Audio Turn Probability
//!     │
//!     └──► (downstream STT) ──► Transcript ──► TurnDetector ──► Text Turn Probability
//!                                                                        │
//!     ┌──────────────────────────────────────────────────────────────────┘
//!     │
//!     ▼
//! TurnDecisionEngine
//!     │
//!     ▼
//! Final Turn Decision (is_turn_complete, combined_probability)
//! ```
//!
//! ## Decision Logic
//!
//! 1. **Gate on speech state**: Only check for turn when user is speaking or just finished
//! 2. **Audio signal (primary)**: SmartTurnDetector probability from mel spectrogram
//! 3. **Text signal (secondary)**: TurnDetector probability from transcript (if available)
//! 4. **Ensemble**: Weighted combination or max of signals above threshold

mod engine;

pub use engine::{
    TurnDecision, TurnDecisionEngine, TurnDecisionEngineBuilder, TurnDecisionEngineConfig,
    TurnSignal, TurnState,
};
