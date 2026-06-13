//! User-mute strategies (PIPECAT_FIX_PLAN A-G5).
//!
//! Pipecat `turns/user_mute/`: evaluated per signal, OR-combined by the
//! controller; while any returns `true`, USER INPUT signals are dropped —
//! bot/lifecycle/metadata signals always flow (the controller enforces the
//! exemption via [`ControllerSignal::is_user_input`]).
//!
//! - [`AlwaysMuteWhileBotSpeaks`]: drop user input whenever the bot is
//!   audibly speaking (no barge-in at all — kiosk/announcement mode).
//! - [`MuteUntilFirstBotComplete`]: drop user input from session start until
//!   the bot finishes its FIRST utterance (greeting/disclaimer guard).
//! - [`FirstSpeechMute`]: only the user's first speech segment passes; after
//!   it completes, input is muted (single-shot intake prompts).
//! - [`FunctionCallMute`]: drop user input while ≥1 tracked function call is
//!   in flight (B-G4). The handle is shared with the orchestration layer,
//!   which MUST mark calls ended on success AND cancel/error — the drain on
//!   every exit path is what prevents a stuck mute.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::super::signal::ControllerSignal;
use super::super::strategy::{TurnCtx, UserMuteStrategy};

/// No barge-in: user input is suppressed whenever the bot is speaking.
#[derive(Debug, Default)]
pub struct AlwaysMuteWhileBotSpeaks;

impl UserMuteStrategy for AlwaysMuteWhileBotSpeaks {
    fn on_signal(&mut self, _sig: &ControllerSignal, ctx: &TurnCtx) -> bool {
        ctx.bot_speaking
    }
}

/// Greeting/disclaimer guard: mute from session start until the bot's FIRST
/// utterance completes.
///
/// Driven by a SHARED latch (`first_complete`) the orchestration layer flips
/// on the first TTS-complete — NOT by sampling signals (review wf_d43814c3
/// #4: a user who listens silently through the greeting sends zero signals
/// while the bot speaks, so a probe-edge version that only samples at signal
/// time never observes the completion and mutes forever). The external latch
/// removes that dependency: the greeting's completion flips it regardless of
/// whether any user input arrived.
///
/// NOTE: this strategy is only meaningful WITH a bot utterance that plays
/// without requiring user input first (a greeting/disclaimer-on-join, or any
/// prior bot turn). With no such utterance, input stays guarded — the
/// correct disclaimer semantics, not a bug.
#[derive(Debug, Clone)]
pub struct MuteUntilFirstBotComplete {
    first_complete: Arc<std::sync::atomic::AtomicBool>,
}

impl MuteUntilFirstBotComplete {
    /// Build the strategy plus the shared latch the orchestration layer flips
    /// on the first bot-utterance completion.
    pub fn new() -> (Self, Arc<std::sync::atomic::AtomicBool>) {
        let latch = Arc::new(std::sync::atomic::AtomicBool::new(false));
        (Self { first_complete: Arc::clone(&latch) }, latch)
    }
}

impl UserMuteStrategy for MuteUntilFirstBotComplete {
    fn on_signal(&mut self, _sig: &ControllerSignal, _ctx: &TurnCtx) -> bool {
        !self.first_complete.load(Ordering::Acquire)
    }
    // No reset(): the latch is SESSION-lifecycle external state, never
    // per-turn — and the controller no longer resets mute strategies anyway
    // (review wf_d43814c3 #3).
}

/// Only the FIRST user speech segment passes; everything after its
/// completing `speech_final` is muted.
#[derive(Debug, Default)]
pub struct FirstSpeechMute {
    first_segment_done: bool,
}

impl UserMuteStrategy for FirstSpeechMute {
    fn on_signal(&mut self, sig: &ControllerSignal, _ctx: &TurnCtx) -> bool {
        if self.first_segment_done {
            return true;
        }
        if let ControllerSignal::SttFinal { is_speech_final: true, .. } = sig {
            // This signal still passes (it completes the first segment);
            // muting begins with the NEXT signal.
            self.first_segment_done = true;
        }
        false
    }

    fn reset(&mut self) {
        self.first_segment_done = false;
    }
}

/// Shared in-flight function-call counter (B-G4 integration). The
/// orchestration layer increments at call start and MUST decrement on
/// success, error, timeout, AND cancellation — every exit path — or the
/// mute sticks (the failure mode Pipecat wires `FunctionCallEnded` for).
#[derive(Debug, Default)]
pub struct FunctionCallActivity {
    in_flight: AtomicUsize,
}

impl FunctionCallActivity {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// A call started. Returns a guard that decrements on EVERY exit path
    /// (drop = RAII — success, error, panic, cancel all drain).
    pub fn begin(self: &Arc<Self>) -> FunctionCallGuard {
        self.in_flight.fetch_add(1, Ordering::AcqRel);
        FunctionCallGuard { activity: Arc::clone(self) }
    }

    pub fn in_flight(&self) -> usize {
        self.in_flight.load(Ordering::Acquire)
    }
}

/// RAII drain for [`FunctionCallActivity`].
pub struct FunctionCallGuard {
    activity: Arc<FunctionCallActivity>,
}

impl Drop for FunctionCallGuard {
    fn drop(&mut self) {
        self.activity.in_flight.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Mute user input while ≥1 function call is in flight.
#[derive(Debug)]
pub struct FunctionCallMute {
    activity: Arc<FunctionCallActivity>,
}

impl FunctionCallMute {
    pub fn new(activity: Arc<FunctionCallActivity>) -> Self {
        Self { activity }
    }
}

impl UserMuteStrategy for FunctionCallMute {
    fn on_signal(&mut self, _sig: &ControllerSignal, _ctx: &TurnCtx) -> bool {
        self.activity.in_flight() > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(bot_speaking: bool) -> TurnCtx {
        TurnCtx { bot_speaking, turn_active: false, stt_ttfs_p99_ms: None }
    }
    fn interim(text: &str) -> ControllerSignal {
        ControllerSignal::SttInterim { text: text.into(), confidence: 0.9 }
    }
    fn speech_final(text: &str) -> ControllerSignal {
        ControllerSignal::SttFinal {
            text: text.into(),
            is_speech_final: true,
            is_finalized: false,
        }
    }

    #[test]
    fn always_mute_blocks_during_bot_speech() {
        let mut s = AlwaysMuteWhileBotSpeaks;
        assert!(s.on_signal(&interim("stop"), &ctx(true)), "muted while bot speaks");
        assert!(!s.on_signal(&interim("hello"), &ctx(false)), "open while silent");
    }

    #[test]
    fn mute_until_first_bot_complete_blocks_from_start() {
        use std::sync::atomic::Ordering;
        let (mut s, latch) = MuteUntilFirstBotComplete::new();
        // Muted from session start (before the bot even starts).
        assert!(s.on_signal(&interim("hi"), &ctx(false)));
        // Bot speaks its greeting; the user barges in — still muted.
        assert!(s.on_signal(&interim("interrupting!"), &ctx(true)), "greeting protected");
        // The greeting's first TTS-complete flips the shared latch...
        latch.store(true, Ordering::Release);
        assert!(!s.on_signal(&interim("now I can talk"), &ctx(false)));
    }

    #[test]
    fn mute_until_first_bot_complete_opens_for_silent_listener() {
        // review wf_d43814c3 #4: a user who listens SILENTLY through the
        // greeting sends NO signals while the bot speaks. The external latch
        // (flipped on TTS-complete) means their first post-greeting signal
        // sees first_complete=true — no signal-sampling dependency, no
        // forever-mute.
        use std::sync::atomic::Ordering;
        let (mut s, latch) = MuteUntilFirstBotComplete::new();
        // (zero signals during the greeting) ... greeting completes:
        latch.store(true, Ordering::Release);
        // The very first signal the user sends is NOT muted.
        assert!(!s.on_signal(&interim("hello after greeting"), &ctx(false)));
    }

    #[test]
    fn first_speech_only_first() {
        let mut s = FirstSpeechMute::default();
        assert!(!s.on_signal(&interim("my name"), &ctx(false)));
        // The completing final itself still passes...
        assert!(!s.on_signal(&speech_final("my name is Ada"), &ctx(false)));
        // ...everything after is muted.
        assert!(s.on_signal(&interim("and another thing"), &ctx(false)));
        assert!(s.on_signal(&speech_final("and another thing"), &ctx(false)));
        s.reset();
        assert!(!s.on_signal(&interim("fresh session"), &ctx(false)));
    }

    #[test]
    fn function_call_mute_drains_on_cancel() {
        let activity = FunctionCallActivity::new();
        let mut s = FunctionCallMute::new(Arc::clone(&activity));
        assert!(!s.on_signal(&interim("hello"), &ctx(false)));

        let g1 = activity.begin();
        let g2 = activity.begin();
        assert!(s.on_signal(&interim("muted"), &ctx(false)), "in-flight calls mute");
        drop(g1); // success path
        assert!(s.on_signal(&interim("still one left"), &ctx(false)));
        drop(g2); // cancel/error path — SAME guard, RAII drains every exit
        assert!(
            !s.on_signal(&interim("open again"), &ctx(false)),
            "mute must drain on cancellation, never stick"
        );
    }

    /// The lifecycle exemption lives in the CONTROLLER: a muted session
    /// still processes Bot/metadata signals (only user input drops).
    #[test]
    fn lifecycle_never_muted() {
        use crate::core::turn::strategies::legacy::{AnySpeechStart, LegacySpeechFinalStop};
        use crate::core::turn::{ControllerSignal as CS, TurnController, TurnEvent};

        let c = TurnController::new(
            vec![Box::new(AnySpeechStart)],
            vec![Box::new(LegacySpeechFinalStop)],
            vec![Box::new(AlwaysMuteWhileBotSpeaks)],
        );
        // Bot starts: user input is now muted...
        let e = c.feed(&CS::BotStarted);
        assert!(e.contains(&TurnEvent::MuteChanged { muted: true }));
        assert!(c.feed(&interim("uh")).is_empty(), "user input dropped");
        // ...but the lifecycle BotStopped flows and unmutes.
        let e = c.feed(&CS::BotStopped);
        assert!(
            e.contains(&TurnEvent::MuteChanged { muted: false }),
            "lifecycle signals are never muted: {e:?}"
        );
        // Input works again.
        assert!(!c.feed(&interim("hello there")).is_empty());
    }
}
