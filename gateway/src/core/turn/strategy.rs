//! The three strategy traits — sync, infallible, zero-alloc verdict calls.
//!
//! Strategies are pure state machines: `on_signal` must not await, must not
//! take locks, and must not panic (the controller calls it on the STT/turn
//! hot path). All I/O happens in the controller's CALLER after the verdict.

use super::signal::ControllerSignal;

/// Verdict from a start strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartVerdict {
    /// Not my signal / not enough evidence — let the next strategy look.
    Ignore,
    /// A user turn begins. `interrupt` = this counts as barge-in (the caller
    /// cancels the bot's turn and clears TTS).
    Start { interrupt: bool },
    /// Sub-threshold input (a cough, a stray word below the min-words gate):
    /// discard the partial aggregation so it never reaches the LLM.
    ResetAggregation,
}

/// Verdict from a stop strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopVerdict {
    /// Not my signal — let the next strategy look.
    Ignore,
    /// Enough signal to START LLM inference speculatively while the turn
    /// stays open (WaaV's eager-EoT `Speculate` semantics — fix-plan A8: this
    /// fires BEFORE the turn is semantically over, unlike Pipecat's
    /// inference-triggered which requires an active stopped-ish turn).
    Speculate,
    /// The user turn is semantically over: commit + run/confirm the LLM turn.
    Stopped,
}

/// Read-only context snapshot handed to strategies with each signal.
#[derive(Debug, Clone, Copy)]
pub struct TurnCtx {
    /// The bot is audibly speaking (first egress chunk seen, playback not yet
    /// drained). Drives min-words barge-in gating (A-G3).
    pub bot_speaking: bool,
    /// A user turn is currently active.
    pub turn_active: bool,
    /// The provider's measured speech-end→final p99, if known (D-G8).
    pub stt_ttfs_p99_ms: Option<u64>,
}

/// When does a user turn begin / when does barge-in count?
pub trait UserTurnStartStrategy: Send {
    fn on_signal(&mut self, sig: &ControllerSignal, ctx: &TurnCtx) -> StartVerdict;
    /// Reset per-turn state (called on every turn boundary).
    fn reset(&mut self) {}
}

/// When is the user turn over (or ready for speculative inference)?
pub trait UserTurnStopStrategy: Send {
    fn on_signal(&mut self, sig: &ControllerSignal, ctx: &TurnCtx) -> StopVerdict;
    fn reset(&mut self) {}
}

/// Should user input be suppressed right now? OR-combined across strategies.
pub trait UserMuteStrategy: Send {
    fn on_signal(&mut self, sig: &ControllerSignal, ctx: &TurnCtx) -> bool;
    fn reset(&mut self) {}
}
