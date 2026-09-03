//! MinWords barge-in start gate (PIPECAT_FIX_PLAN A-G3).
//!
//! While the bot is audibly speaking, require ≥ `min_words` words to start a
//! turn (= interrupt the bot); while silent, a single word starts it. This is
//! the production answer to "a cough / 'uh-huh' must not cut the bot off".
//!
//! Fragment robustness (WaaV adaptation): providers deliver finals as
//! NON-OVERLAPPING fragments, while interims are typically cumulative.
//! Per-fragment word counting would discard a long utterance piece by piece,
//! so the verdicts are:
//! - text words ≥ threshold → `Start{interrupt:true}` (a cumulative interim
//!   or a multi-word fragment crosses it naturally);
//! - sub-threshold **`speech_final`** → `ResetAggregation` (the utterance is
//!   over and never met the gate — that IS the cough: discard it so it
//!   neither fires timers nor leaks into the next turn's buffer);
//! - any other sub-threshold signal → `Ignore` (keep accumulating — never
//!   discard mid-utterance fragments).
//!
//! The safety net for "long utterance, tiny fragments, no interims": the
//! segment-transcript-truth fix makes the `speech_final` signal carry the
//! FULL accumulated segment text, so the gate evaluates the whole utterance
//! at the latest possible moment and still starts (and immediately stops)
//! the turn on that signal.

use super::super::signal::ControllerSignal;
use super::super::strategy::{StartVerdict, TurnCtx, UserTurnStartStrategy};

/// Word-count barge-in gate. `min_words` applies only while the bot speaks.
#[derive(Debug)]
pub struct MinWordsStart {
    min_words: usize,
}

impl MinWordsStart {
    /// `min_words` < 2 is pointless (1 ≡ the legacy any-speech behavior) —
    /// clamped up to 2 to keep the config honest.
    pub fn new(min_words: usize) -> Self {
        Self {
            min_words: min_words.max(2),
        }
    }
}

impl UserTurnStartStrategy for MinWordsStart {
    fn on_signal(&mut self, sig: &ControllerSignal, ctx: &TurnCtx) -> StartVerdict {
        let (text, is_speech_final) = match sig {
            ControllerSignal::SttInterim { text, .. } => (text, false),
            ControllerSignal::SttFinal {
                text,
                is_speech_final,
                ..
            } => (text, *is_speech_final),
            _ => return StartVerdict::Ignore,
        };
        if text.trim().is_empty() {
            return StartVerdict::Ignore;
        }
        let threshold = if ctx.bot_speaking { self.min_words } else { 1 };
        if text.split_whitespace().count() >= threshold {
            return StartVerdict::Start { interrupt: true };
        }
        if is_speech_final {
            // Utterance over, never met the gate: the cough. Discard.
            return StartVerdict::ResetAggregation;
        }
        StartVerdict::Ignore
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(bot_speaking: bool) -> TurnCtx {
        TurnCtx {
            bot_speaking,
            turn_active: false,
            stt_ttfs_p99_ms: None,
        }
    }
    fn interim(text: &str) -> ControllerSignal {
        ControllerSignal::SttInterim {
            text: text.into(),
            confidence: 0.9,
        }
    }
    fn speech_final(text: &str) -> ControllerSignal {
        ControllerSignal::SttFinal {
            text: text.into(),
            is_speech_final: true,
            is_finalized: false,
        }
    }
    fn plain_final(text: &str) -> ControllerSignal {
        ControllerSignal::SttFinal {
            text: text.into(),
            is_speech_final: false,
            is_finalized: false,
        }
    }

    #[test]
    fn single_word_starts_when_bot_silent() {
        let mut s = MinWordsStart::new(3);
        assert_eq!(
            s.on_signal(&interim("hello"), &ctx(false)),
            StartVerdict::Start { interrupt: true }
        );
    }

    #[test]
    fn single_word_ignored_while_bot_speaks() {
        // The core false-barge-in regression: "uh-huh" over the bot.
        let mut s = MinWordsStart::new(3);
        assert_eq!(
            s.on_signal(&interim("uh-huh"), &ctx(true)),
            StartVerdict::Ignore
        );
    }

    #[test]
    fn n_words_interrupt_the_bot() {
        let mut s = MinWordsStart::new(3);
        assert_eq!(
            s.on_signal(&interim("stop wait actually"), &ctx(true)),
            StartVerdict::Start { interrupt: true }
        );
    }

    #[test]
    fn cumulative_interims_cross_the_gate() {
        let mut s = MinWordsStart::new(3);
        assert_eq!(
            s.on_signal(&interim("turn"), &ctx(true)),
            StartVerdict::Ignore
        );
        assert_eq!(
            s.on_signal(&interim("turn that"), &ctx(true)),
            StartVerdict::Ignore
        );
        assert_eq!(
            s.on_signal(&interim("turn that off"), &ctx(true)),
            StartVerdict::Start { interrupt: true }
        );
    }

    #[test]
    fn mid_utterance_fragments_are_never_discarded() {
        // Sub-threshold plain finals (fragments) must be Ignored, not Reset —
        // discarding them piecewise would eat a long utterance.
        let mut s = MinWordsStart::new(3);
        assert_eq!(
            s.on_signal(&plain_final("hello"), &ctx(true)),
            StartVerdict::Ignore
        );
        assert_eq!(
            s.on_signal(&plain_final("there"), &ctx(true)),
            StartVerdict::Ignore
        );
    }

    #[test]
    fn sub_threshold_speech_final_is_the_cough_discard() {
        let mut s = MinWordsStart::new(3);
        assert_eq!(
            s.on_signal(&speech_final("uh-huh"), &ctx(true)),
            StartVerdict::ResetAggregation
        );
    }

    #[test]
    fn full_segment_at_speech_final_starts_despite_tiny_fragments() {
        // Transcript-truth: the speech_final carries the FULL segment text.
        let mut s = MinWordsStart::new(3);
        assert_eq!(
            s.on_signal(&speech_final("please turn the lights off"), &ctx(true)),
            StartVerdict::Start { interrupt: true }
        );
    }

    #[test]
    fn min_words_clamped_to_two() {
        let mut s = MinWordsStart::new(0);
        // With min_words clamped to 2, a single word while the bot speaks is
        // still gated.
        assert_eq!(
            s.on_signal(&interim("hey"), &ctx(true)),
            StartVerdict::Ignore
        );
    }
}
