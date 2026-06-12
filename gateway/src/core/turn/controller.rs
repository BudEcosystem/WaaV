//! The turn controller: feeds normalized signals through the mute → start →
//! stop strategy pipeline and returns turn-decision events to the CALLER.
//!
//! Concurrency contract (the load-bearing part — PIPECAT_FIX_PLAN §6.2):
//! - [`TurnController::feed`] is **synchronous**: it takes the strategy lock
//!   only across pure verdict calls (microseconds, no awaits possible), then
//!   returns events. The caller — the same task that received the STT
//!   result — performs the async actions, preserving the synchronous
//!   caller-fires-callback contract bit-for-bit.
//! - **Exactly-once**: `Started` fires only on an inactive→active transition,
//!   `Stopped` only on active→inactive (atomic swaps); a `turn_id` is
//!   allocated per turn and is identical across that turn's events.
//! - Strategies short-circuit: the first non-`Ignore` verdict in each family
//!   wins (Pipecat's `ProcessFrameResult.STOP`).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use parking_lot::Mutex;

use super::signal::ControllerSignal;
use super::strategy::{
    StartVerdict, StopVerdict, TurnCtx, UserMuteStrategy, UserTurnStartStrategy,
    UserTurnStopStrategy,
};

/// A turn-decision event for the caller to act on.
#[derive(Debug, Clone, PartialEq)]
pub enum TurnEvent {
    /// A user turn began. `interrupt` ⇒ treat as barge-in (cancel bot turn +
    /// clear TTS).
    Started { turn_id: u64, interrupt: bool },
    /// Start LLM inference speculatively (eager-EoT) while the turn stays
    /// open; superseded or confirmed later.
    Speculate { turn_id: u64, transcript: String },
    /// The user turn is over; run/confirm the LLM turn with `transcript`.
    Stopped { turn_id: u64, transcript: String },
    /// Discard the partial aggregation (sub-threshold input).
    ResetAggregation,
    /// User speech continued while the bot is STILL audibly speaking after
    /// the turn already started: re-run the (idempotent) barge-in mop-up so
    /// audio that landed AFTER the first clear (an in-flight speak resolving
    /// post-cancel) is cleared too — the legacy continuous-clear behavior,
    /// emitted only when actually needed (review wf_5772cd64 #5).
    BargeInMopUp,
    /// The mute state flipped.
    MuteChanged { muted: bool },
}

/// Pluggable turn-policy controller. One per session.
pub struct TurnController {
    start: Mutex<Vec<Box<dyn UserTurnStartStrategy>>>,
    stop: Mutex<Vec<Box<dyn UserTurnStopStrategy>>>,
    mute: Mutex<Vec<Box<dyn UserMuteStrategy>>>,
    turn_active: AtomicBool,
    next_turn_id: AtomicU64,
    current_turn_id: AtomicU64,
    bot_speaking: AtomicBool,
    user_muted: AtomicBool,
    /// 0 = unknown (no SttMetadata seen).
    stt_ttfs_p99_ms: AtomicU64,
    /// Optional live probe for the bot-speaking truth (the VoiceManager's
    /// playout estimate, A-G3/A7). When set it OVERRIDES the signal-driven
    /// flag — it is exactly as fresh as the moment a signal is evaluated,
    /// with no BotStopped timer task needed.
    bot_speaking_probe: Option<Box<dyn Fn() -> bool + Send + Sync>>,
}

impl std::fmt::Debug for TurnController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TurnController")
            .field("turn_active", &self.turn_active.load(Ordering::Relaxed))
            .field("bot_speaking", &self.bot_speaking.load(Ordering::Relaxed))
            .field("user_muted", &self.user_muted.load(Ordering::Relaxed))
            .finish()
    }
}

impl TurnController {
    pub fn new(
        start: Vec<Box<dyn UserTurnStartStrategy>>,
        stop: Vec<Box<dyn UserTurnStopStrategy>>,
        mute: Vec<Box<dyn UserMuteStrategy>>,
    ) -> Self {
        Self {
            start: Mutex::new(start),
            stop: Mutex::new(stop),
            mute: Mutex::new(mute),
            turn_active: AtomicBool::new(false),
            next_turn_id: AtomicU64::new(1),
            current_turn_id: AtomicU64::new(0),
            bot_speaking: AtomicBool::new(false),
            user_muted: AtomicBool::new(false),
            stt_ttfs_p99_ms: AtomicU64::new(0),
            bot_speaking_probe: None,
        }
    }

    /// Install a live bot-speaking probe (overrides the signal-driven flag).
    pub fn with_bot_speaking_probe(
        mut self,
        probe: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Self {
        self.bot_speaking_probe = Some(Box::new(probe));
        self
    }

    /// Whether a user turn is currently active.
    pub fn turn_active(&self) -> bool {
        self.turn_active.load(Ordering::Acquire)
    }

    /// The bot-speaking flag (from BotStarted/BotStopped signals).
    pub fn bot_speaking(&self) -> bool {
        self.bot_speaking.load(Ordering::Acquire)
    }

    /// Feed one signal; returns the events the caller must act on, in order.
    ///
    /// Synchronous by design (see module docs). Cheap when nothing happens:
    /// a few atomic loads + the strategy verdict calls.
    pub fn feed(&self, sig: &ControllerSignal) -> Vec<TurnEvent> {
        let mut out = Vec::new();

        // 1) Bookkeeping signals first (never muted, never strategy input).
        match sig {
            ControllerSignal::BotStarted => {
                self.bot_speaking.store(true, Ordering::Release);
            }
            ControllerSignal::BotStopped => {
                self.bot_speaking.store(false, Ordering::Release);
            }
            ControllerSignal::SttMetadata { ttfs_p99_ms } => {
                self.stt_ttfs_p99_ms.store(*ttfs_p99_ms, Ordering::Release);
            }
            _ => {}
        }

        let ctx = TurnCtx {
            bot_speaking: match &self.bot_speaking_probe {
                Some(probe) => probe(),
                None => self.bot_speaking.load(Ordering::Acquire),
            },
            turn_active: self.turn_active.load(Ordering::Acquire),
            stt_ttfs_p99_ms: match self.stt_ttfs_p99_ms.load(Ordering::Acquire) {
                0 => None,
                v => Some(v),
            },
        };

        // 2) Mute evaluation (OR across strategies); muted ⇒ drop USER INPUT
        //    signals only — bot/lifecycle/metadata always flow.
        let muted = {
            let mut mute = self.mute.lock();
            let mut any = false;
            for s in mute.iter_mut() {
                any |= s.on_signal(sig, &ctx);
            }
            any
        };
        if muted != self.user_muted.swap(muted, Ordering::AcqRel) {
            out.push(TurnEvent::MuteChanged { muted });
        }
        if muted && sig.is_user_input() {
            return out;
        }

        // 3) Start strategies (short-circuit on the first non-Ignore).
        let start_verdict = {
            let mut start = self.start.lock();
            let mut verdict = StartVerdict::Ignore;
            for s in start.iter_mut() {
                match s.on_signal(sig, &ctx) {
                    StartVerdict::Ignore => continue,
                    v => {
                        verdict = v;
                        break;
                    }
                }
            }
            verdict
        };
        match start_verdict {
            StartVerdict::Ignore => {}
            StartVerdict::ResetAggregation => out.push(TurnEvent::ResetAggregation),
            StartVerdict::Start { interrupt } => {
                // Exactly-once: only the inactive→active transition starts.
                if !self.turn_active.swap(true, Ordering::AcqRel) {
                    let id = self.next_turn_id.fetch_add(1, Ordering::Relaxed);
                    self.current_turn_id.store(id, Ordering::Release);
                    self.reset_strategies();
                    out.push(TurnEvent::Started { turn_id: id, interrupt });
                } else if interrupt && ctx.bot_speaking {
                    // Turn already active but the bot is STILL audible
                    // (e.g. an in-flight speak resolved after the first
                    // clear): mop up.
                    out.push(TurnEvent::BargeInMopUp);
                }
            }
        }

        // 4) Stop strategies (short-circuit). Re-read turn_active — the start
        //    phase above may have just opened the turn on this same signal
        //    (e.g. a first-ever speech_final with no prior interim).
        let ctx = TurnCtx { turn_active: self.turn_active.load(Ordering::Acquire), ..ctx };
        let stop_verdict = {
            let mut stop = self.stop.lock();
            let mut verdict = StopVerdict::Ignore;
            for s in stop.iter_mut() {
                match s.on_signal(sig, &ctx) {
                    StopVerdict::Ignore => continue,
                    v => {
                        verdict = v;
                        break;
                    }
                }
            }
            verdict
        };
        match stop_verdict {
            StopVerdict::Ignore => {}
            StopVerdict::Speculate => {
                // Speculation is only meaningful while the turn is open.
                if self.turn_active.load(Ordering::Acquire) {
                    let transcript = signal_text(sig).unwrap_or_default();
                    out.push(TurnEvent::Speculate {
                        turn_id: self.current_turn_id.load(Ordering::Acquire),
                        transcript,
                    });
                }
            }
            StopVerdict::Stopped => {
                // Exactly-once: only the active→inactive transition stops.
                if self.turn_active.swap(false, Ordering::AcqRel) {
                    let transcript = signal_text(sig).unwrap_or_default();
                    out.push(TurnEvent::Stopped {
                        turn_id: self.current_turn_id.load(Ordering::Acquire),
                        transcript,
                    });
                }
            }
        }

        out
    }

    fn reset_strategies(&self) {
        for s in self.start.lock().iter_mut() {
            s.reset();
        }
        for s in self.stop.lock().iter_mut() {
            s.reset();
        }
        for s in self.mute.lock().iter_mut() {
            s.reset();
        }
    }
}

fn signal_text(sig: &ControllerSignal) -> Option<String> {
    match sig {
        ControllerSignal::SttInterim { text, .. } | ControllerSignal::SttFinal { text, .. } => {
            Some(text.clone())
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::super::strategies::legacy::{AnySpeechStart, LegacySpeechFinalStop};
    use super::*;

    fn legacy_controller() -> TurnController {
        TurnController::new(
            vec![Box::new(AnySpeechStart)],
            vec![Box::new(LegacySpeechFinalStop)],
            vec![],
        )
    }

    fn interim(text: &str) -> ControllerSignal {
        ControllerSignal::SttInterim { text: text.into(), confidence: 0.9 }
    }
    fn final_sig(text: &str, speech_final: bool) -> ControllerSignal {
        ControllerSignal::SttFinal {
            text: text.into(),
            is_speech_final: speech_final,
            is_finalized: false,
        }
    }

    #[test]
    fn no_double_start() {
        let c = legacy_controller();
        let e1 = c.feed(&interim("hello"));
        assert!(matches!(e1.as_slice(), [TurnEvent::Started { interrupt: true, .. }]));
        // More speech while the turn is open: no second Started.
        assert!(c.feed(&interim("hello there")).is_empty());
        assert!(c.feed(&final_sig("hello there", false)).is_empty());
    }

    #[test]
    fn mop_up_while_bot_still_audible_silent_otherwise() {
        // review wf_5772cd64 #5: after Started{interrupt} clears the bot, an
        // in-flight speak can resolve and make it audible AGAIN. Continued
        // user speech must re-run the (idempotent) mop-up — but only while
        // the bot is actually audible.
        let audible = std::sync::Arc::new(AtomicBool::new(true));
        let probe = audible.clone();
        let c = TurnController::new(
            vec![Box::new(AnySpeechStart)],
            vec![Box::new(LegacySpeechFinalStop)],
            vec![],
        )
        .with_bot_speaking_probe(move || probe.load(Ordering::Relaxed));

        let e1 = c.feed(&interim("stop"));
        assert!(matches!(e1.as_slice(), [TurnEvent::Started { interrupt: true, .. }]));
        // Bot still audible mid-turn → mop-up, NOT a second Started.
        assert_eq!(c.feed(&interim("stop talking")), vec![TurnEvent::BargeInMopUp]);
        // Bot went silent → continued speech is just aggregation, no events.
        audible.store(false, Ordering::Relaxed);
        assert!(c.feed(&interim("stop talking please")).is_empty());
    }

    #[test]
    fn no_double_stop() {
        let c = legacy_controller();
        c.feed(&interim("hi"));
        let e = c.feed(&final_sig("hi", true));
        assert!(matches!(e.as_slice(), [TurnEvent::Stopped { .. }]));
        // A duplicate speech_final after the stop: Started fires for the NEW
        // turn (legacy parity: any text barge-ins), then Stopped again — both
        // for the NEW turn id, never a re-stop of the old one.
        let e2 = c.feed(&final_sig("hi", true));
        match e2.as_slice() {
            [TurnEvent::Started { turn_id: s, .. }, TurnEvent::Stopped { turn_id: t, .. }] => {
                assert_eq!(s, t);
            }
            other => panic!("unexpected events: {other:?}"),
        }
    }

    #[test]
    fn turn_id_stable_within_turn_and_monotonic_across() {
        let c = legacy_controller();
        let e1 = c.feed(&interim("one"));
        let id1 = match e1.as_slice() {
            [TurnEvent::Started { turn_id, .. }] => *turn_id,
            other => panic!("{other:?}"),
        };
        let e2 = c.feed(&final_sig("one", true));
        let id1_stop = match e2.as_slice() {
            [TurnEvent::Stopped { turn_id, .. }] => *turn_id,
            other => panic!("{other:?}"),
        };
        assert_eq!(id1, id1_stop, "same id across one turn's events");

        let e3 = c.feed(&interim("two"));
        let id2 = match e3.as_slice() {
            [TurnEvent::Started { turn_id, .. }] => *turn_id,
            other => panic!("{other:?}"),
        };
        assert!(id2 > id1, "ids strictly increase across turns");
    }

    #[test]
    fn first_event_speech_final_starts_and_stops_in_one_feed() {
        // No prior interim (e.g. a provider that only emits finals): the same
        // signal opens AND closes the turn, in order, with one id.
        let c = legacy_controller();
        let e = c.feed(&final_sig("quick question", true));
        match e.as_slice() {
            [
                TurnEvent::Started { turn_id: a, interrupt: true },
                TurnEvent::Stopped { turn_id: b, transcript },
            ] => {
                assert_eq!(a, b);
                assert_eq!(transcript, "quick question");
            }
            other => panic!("unexpected events: {other:?}"),
        }
    }

    #[test]
    fn empty_text_produces_no_events() {
        // Legacy parity: empty interims/finals neither barge-in nor fire.
        let c = legacy_controller();
        assert!(c.feed(&interim("")).is_empty());
        assert!(c.feed(&final_sig("", true)).is_empty());
        assert!(c.feed(&final_sig("   ", true)).is_empty());
    }

    #[test]
    fn stopped_carries_the_signal_transcript() {
        let c = legacy_controller();
        c.feed(&interim("the answer"));
        let e = c.feed(&final_sig("the answer is 42", true));
        assert!(matches!(
            e.as_slice(),
            [TurnEvent::Stopped { transcript, .. }] if transcript == "the answer is 42"
        ));
    }

    #[test]
    fn start_strategy_short_circuits() {
        struct CountingStart(std::sync::Arc<AtomicU64>);
        impl UserTurnStartStrategy for CountingStart {
            fn on_signal(&mut self, _: &ControllerSignal, _: &TurnCtx) -> StartVerdict {
                self.0.fetch_add(1, Ordering::Relaxed);
                StartVerdict::Ignore
            }
        }
        let counter = std::sync::Arc::new(AtomicU64::new(0));
        let c = TurnController::new(
            vec![Box::new(AnySpeechStart), Box::new(CountingStart(counter.clone()))],
            vec![Box::new(LegacySpeechFinalStop)],
            vec![],
        );
        c.feed(&interim("hello"));
        assert_eq!(
            counter.load(Ordering::Relaxed),
            0,
            "the second strategy must not run once the first returned a verdict"
        );
    }

    #[test]
    fn bot_speaking_tracked_and_bot_signals_never_muted() {
        struct MuteAll;
        impl UserMuteStrategy for MuteAll {
            fn on_signal(&mut self, _: &ControllerSignal, _: &TurnCtx) -> bool {
                true
            }
        }
        let c = TurnController::new(
            vec![Box::new(AnySpeechStart)],
            vec![Box::new(LegacySpeechFinalStop)],
            vec![Box::new(MuteAll)],
        );
        // Muted: user input produces only the MuteChanged edge, no Started.
        let e = c.feed(&interim("hello"));
        assert_eq!(e, vec![TurnEvent::MuteChanged { muted: true }]);
        assert!(!c.turn_active());
        // Bot signals still flow and update state while muted.
        c.feed(&ControllerSignal::BotStarted);
        assert!(c.bot_speaking());
        c.feed(&ControllerSignal::BotStopped);
        assert!(!c.bot_speaking());
    }

    #[test]
    fn ttfs_metadata_reaches_ctx() {
        struct CaptureCtx(std::sync::Arc<AtomicU64>);
        impl UserTurnStopStrategy for CaptureCtx {
            fn on_signal(&mut self, _: &ControllerSignal, ctx: &TurnCtx) -> StopVerdict {
                self.0.store(ctx.stt_ttfs_p99_ms.unwrap_or(0), Ordering::Relaxed);
                StopVerdict::Ignore
            }
        }
        let seen = std::sync::Arc::new(AtomicU64::new(0));
        let c = TurnController::new(
            vec![Box::new(AnySpeechStart)],
            vec![Box::new(CaptureCtx(seen.clone()))],
            vec![],
        );
        c.feed(&ControllerSignal::SttMetadata { ttfs_p99_ms: 350 });
        c.feed(&interim("x"));
        assert_eq!(seen.load(Ordering::Relaxed), 350);
    }
}
