//! Pluggable turn-taking strategy spine (PIPECAT_FIX_PLAN A-G0).
//!
//! WaaV's turn policy (when a user turn starts / barge-in counts, when it
//! stops, when user input is muted) was hardcoded across `stt_result.rs`,
//! `turn_decision/engine.rs` and `conversation/mod.rs`. This module factors
//! the *policy* into composable strategy objects on one [`TurnController`],
//! mirroring Pipecat's `turns/` stack — adapted to WaaV per the converged
//! fix-plan corrections (§6.2 of `PIPECAT_FIX_PLAN.md`):
//!
//! - The signal type is **[`ControllerSignal`]** — the name `TurnSignal` is
//!   already taken by the live `turn_decision::TurnSignal` ensemble input.
//! - The existing [`crate::core::turn_decision::TurnDecisionEngine`] stays
//!   the smart-turn *detector*; its `is_turn_complete` verdict enters here
//!   **opaque** as [`ControllerSignal::SmartTurn`]. There is deliberately no
//!   strategy re-deriving turn-end from raw probability — that would create
//!   two competing turn-end deciders.
//! - **Actor-bug guardrail** (MASTER_PLAN crit#1): transcript egress to the
//!   client stays a synchronous side-effect of the STT callback. The
//!   controller is fed *alongside* egress and returns **turn-decision events
//!   to the caller**, who performs the async actions (`run_turn`,
//!   `handle_barge_in`) from its own task — the caller-fires-callback
//!   contract is preserved by construction ([`TurnController::feed`] is
//!   synchronous and never dispatches callbacks itself).
//! - Strategies are **sync and infallible** (`fn on_signal(..) -> Verdict`):
//!   no awaits, no locks held by strategy code, no panics on the hot path.
//!   All I/O happens in the caller after the verdict.
//!
//! Behavior-preservation note (the legacy set): the legacy orchestrator ran
//! barge-in on **every** non-empty STT result; the controller emits
//! [`TurnEvent::Started`] (→ barge-in) **once per turn**. These are
//! equivalent because `handle_barge_in` is idempotent within a segment (the
//! bot's turn is already cancelled after the first call), except for the
//! exotic interleaving where bot speech is triggered *externally* (a direct
//! `speak` command) mid-user-segment — the wiring gate's live byte-diff
//! covers this.

pub mod controller;
pub mod signal;
pub mod strategies;
pub mod strategy;

pub use controller::{TurnController, TurnEvent};
pub use signal::ControllerSignal;
pub use strategy::{
    StartVerdict, StopVerdict, TurnCtx, UserMuteStrategy, UserTurnStartStrategy,
    UserTurnStopStrategy,
};
