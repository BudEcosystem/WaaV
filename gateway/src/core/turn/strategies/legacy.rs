//! The behavior-preserving LEGACY strategy set (A-G0 wiring gate).
//!
//! Replicates the orchestrator's hardcoded policy exactly:
//! - any non-empty user speech (interim or final) is a potential barge-in
//!   (`conversation/mod.rs::on_stt_result`: `has_text → handle_barge_in`);
//! - a `speech_final` with non-empty text ends the turn
//!   (`is_speech_final && has_text → run_turn`).
//!
//! These ship first so the controller rewire can be live-gated against
//! today's behavior before any policy improvement (MinWords, TTFS floor)
//! lands on top.

use super::super::signal::ControllerSignal;
use super::super::strategy::{
    StartVerdict, StopVerdict, TurnCtx, UserTurnStartStrategy, UserTurnStopStrategy,
};

/// Legacy start: ANY non-empty user speech starts the turn and counts as
/// barge-in (no word-count gate — that is A-G3's `MinWords` improvement).
#[derive(Debug, Default)]
pub struct AnySpeechStart;

impl UserTurnStartStrategy for AnySpeechStart {
    fn on_signal(&mut self, sig: &ControllerSignal, _ctx: &TurnCtx) -> StartVerdict {
        match sig {
            ControllerSignal::SttInterim { text, .. }
            | ControllerSignal::SttFinal { text, .. }
                if !text.trim().is_empty() =>
            {
                StartVerdict::Start { interrupt: true }
            }
            _ => StartVerdict::Ignore,
        }
    }
}

/// Eager end-of-turn speculation (P1.2b): a smart-turn COMPLETE verdict
/// (consumed opaque from the `TurnDecisionEngine`) starts the LLM
/// speculatively while the turn stays open — confirmed or superseded by the
/// eventual `speech_final` (WaaV's `Speculate` semantics, fix-plan §6.2 A8).
/// The orchestrator's `eager_eot` config gate still applies downstream.
#[derive(Debug, Default)]
pub struct EagerSmartTurnSpeculate;

impl UserTurnStopStrategy for EagerSmartTurnSpeculate {
    fn on_signal(&mut self, sig: &ControllerSignal, _ctx: &TurnCtx) -> StopVerdict {
        match sig {
            ControllerSignal::SmartTurn { is_complete: true } => StopVerdict::Speculate,
            _ => StopVerdict::Ignore,
        }
    }
}

/// Legacy stop: the provider's (or forced) `speech_final` with non-empty text
/// ends the turn. The transcript carried by the signal is the FULL segment
/// text (segment-transcript-truth fix in `stt_result.rs`).
#[derive(Debug, Default)]
pub struct LegacySpeechFinalStop;

impl UserTurnStopStrategy for LegacySpeechFinalStop {
    fn on_signal(&mut self, sig: &ControllerSignal, _ctx: &TurnCtx) -> StopVerdict {
        match sig {
            ControllerSignal::SttFinal { text, is_speech_final: true, .. }
                if !text.trim().is_empty() =>
            {
                StopVerdict::Stopped
            }
            _ => StopVerdict::Ignore,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> TurnCtx {
        TurnCtx { bot_speaking: false, turn_active: false, stt_ttfs_p99_ms: None }
    }

    #[test]
    fn legacy_set_preserves_speech_final_contract() {
        let mut start = AnySpeechStart;
        let mut stop = LegacySpeechFinalStop;

        // Non-empty interim: barge-in start, no stop.
        let interim =
            ControllerSignal::SttInterim { text: "hel".into(), confidence: 0.5 };
        assert_eq!(start.on_signal(&interim, &ctx()), StartVerdict::Start { interrupt: true });
        assert_eq!(stop.on_signal(&interim, &ctx()), StopVerdict::Ignore);

        // Empty results: neither.
        let empty = ControllerSignal::SttFinal {
            text: "  ".into(),
            is_speech_final: true,
            is_finalized: false,
        };
        assert_eq!(start.on_signal(&empty, &ctx()), StartVerdict::Ignore);
        assert_eq!(stop.on_signal(&empty, &ctx()), StopVerdict::Ignore);

        // Non-speech-final final: start (barge-in) but no stop.
        let plain_final = ControllerSignal::SttFinal {
            text: "hello".into(),
            is_speech_final: false,
            is_finalized: false,
        };
        assert_eq!(
            start.on_signal(&plain_final, &ctx()),
            StartVerdict::Start { interrupt: true }
        );
        assert_eq!(stop.on_signal(&plain_final, &ctx()), StopVerdict::Ignore);

        // speech_final with text: stop fires.
        let speech_final = ControllerSignal::SttFinal {
            text: "hello there".into(),
            is_speech_final: true,
            is_finalized: false,
        };
        assert_eq!(stop.on_signal(&speech_final, &ctx()), StopVerdict::Stopped);

        // Smart-turn / bot signals: inert for the legacy set.
        let st = ControllerSignal::SmartTurn { is_complete: true };
        assert_eq!(start.on_signal(&st, &ctx()), StartVerdict::Ignore);
        assert_eq!(stop.on_signal(&st, &ctx()), StopVerdict::Ignore);
    }
}
