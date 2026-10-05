//! The start strategy of segmented sessions: the gateway's detector, not the transcript.
//!
//! A transcript on a segmented session arrives a second or more after the speech, so interruption
//! cannot wait for it. While nothing plays, confirmed speech opens the caller's turn at once.
//! While the agent speaks (or is about to), sustained speech of `barge_in_ms` interrupts it; the
//! progress events arrive at 224, 384, then every 128 ms, so the default 500 ms threshold takes
//! effect at 512 ms. Shorter overlaps interrupt only when their transcript arrives and has enough
//! words (a *text-confirmed* interruption); the caller filters backchannels and echo before
//! feeding text.

use super::super::signal::ControllerSignal;
use super::super::strategy::{StartVerdict, TurnCtx, UserTurnStartStrategy};

#[derive(Debug)]
pub struct DetectorSpeechStart {
    barge_in_ms: u32,
    min_words: usize,
}

impl DetectorSpeechStart {
    /// `barge_in_ms` is raised to at least 500 ms; `min_words` to at least 1.
    pub fn new(barge_in_ms: u32, min_words: usize) -> Self {
        Self {
            barge_in_ms: barge_in_ms.max(500),
            min_words: min_words.max(1),
        }
    }

    pub fn barge_in_ms(&self) -> u32 {
        self.barge_in_ms
    }
}

impl UserTurnStartStrategy for DetectorSpeechStart {
    fn on_signal(&mut self, sig: &ControllerSignal, ctx: &TurnCtx) -> StartVerdict {
        match sig {
            ControllerSignal::Speech { sustained_ms } => {
                if !ctx.bot_speaking {
                    StartVerdict::Start { interrupt: false }
                } else if *sustained_ms >= self.barge_in_ms {
                    StartVerdict::Start { interrupt: true }
                } else {
                    StartVerdict::Ignore
                }
            }
            ControllerSignal::SttInterim { text, .. } | ControllerSignal::SttFinal { text, .. } => {
                let words = text.split_whitespace().count();
                if words == 0 {
                    return StartVerdict::Ignore;
                }
                if !ctx.bot_speaking {
                    return StartVerdict::Start { interrupt: false };
                }
                if words >= self.min_words {
                    return StartVerdict::Start { interrupt: true };
                }
                if matches!(
                    sig,
                    ControllerSignal::SttFinal {
                        is_speech_final: true,
                        ..
                    }
                ) {
                    StartVerdict::ResetAggregation
                } else {
                    StartVerdict::Ignore
                }
            }
            _ => StartVerdict::Ignore,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::turn::strategies::LegacySpeechFinalStop;
    use crate::core::turn::{TurnController, TurnEvent};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn ctx(bot_speaking: bool) -> TurnCtx {
        TurnCtx {
            bot_speaking,
            turn_active: false,
            stt_ttfs_p99_ms: None,
        }
    }

    #[test]
    fn sustained_speech_interrupts_only_after_the_threshold() {
        let mut s = DetectorSpeechStart::new(500, 1);
        for sustained in [224, 384] {
            assert_eq!(
                s.on_signal(
                    &ControllerSignal::Speech {
                        sustained_ms: sustained
                    },
                    &ctx(true)
                ),
                StartVerdict::Ignore
            );
        }
        assert_eq!(
            s.on_signal(&ControllerSignal::Speech { sustained_ms: 512 }, &ctx(true)),
            StartVerdict::Start { interrupt: true }
        );
    }

    #[test]
    fn with_nothing_playing_confirmed_speech_opens_the_turn_without_interrupting() {
        let mut s = DetectorSpeechStart::new(500, 1);
        assert_eq!(
            s.on_signal(&ControllerSignal::Speech { sustained_ms: 224 }, &ctx(false)),
            StartVerdict::Start { interrupt: false }
        );
    }

    #[test]
    fn the_threshold_is_never_below_500_ms() {
        assert_eq!(DetectorSpeechStart::new(100, 1).barge_in_ms(), 500);
        assert_eq!(DetectorSpeechStart::new(800, 1).barge_in_ms(), 800);
    }

    #[test]
    fn a_short_overlap_interrupts_only_when_its_text_has_enough_words() {
        let mut s = DetectorSpeechStart::new(500, 2);
        let one = ControllerSignal::SttInterim {
            text: "wait".into(),
            confidence: 1.0,
        };
        assert_eq!(s.on_signal(&one, &ctx(true)), StartVerdict::Ignore);
        let two = ControllerSignal::SttInterim {
            text: "wait stop".into(),
            confidence: 1.0,
        };
        assert_eq!(
            s.on_signal(&two, &ctx(true)),
            StartVerdict::Start { interrupt: true }
        );
        let final_one = ControllerSignal::SttFinal {
            text: "hm".into(),
            is_speech_final: true,
            is_finalized: true,
        };
        assert_eq!(
            s.on_signal(&final_one, &ctx(true)),
            StartVerdict::ResetAggregation
        );
    }

    #[test]
    fn a_cough_below_the_threshold_never_interrupts_and_leaves_no_open_turn() {
        let speaking = Arc::new(AtomicBool::new(true));
        let probe = Arc::clone(&speaking);
        let c = TurnController::new(
            vec![Box::new(DetectorSpeechStart::new(500, 1))],
            vec![Box::new(LegacySpeechFinalStop)],
            vec![],
        )
        .with_bot_speaking_probe(move || probe.load(Ordering::SeqCst));
        assert!(
            c.feed(&ControllerSignal::Speech { sustained_ms: 224 })
                .is_empty()
        );
        assert!(
            c.feed(&ControllerSignal::SpeechTurnClosed { had_text: false })
                .is_empty()
        );
        assert!(!c.turn_active());
    }

    #[test]
    fn a_turn_closed_without_text_is_aborted() {
        let c = TurnController::new(
            vec![Box::new(DetectorSpeechStart::new(500, 1))],
            vec![Box::new(LegacySpeechFinalStop)],
            vec![],
        );
        let started = c.feed(&ControllerSignal::Speech { sustained_ms: 224 });
        let id = match started.as_slice() {
            [
                TurnEvent::Started {
                    turn_id,
                    interrupt: false,
                },
            ] => *turn_id,
            other => panic!("{other:?}"),
        };
        assert_eq!(
            c.feed(&ControllerSignal::SpeechTurnClosed { had_text: false }),
            vec![TurnEvent::Aborted { turn_id: id }]
        );
        assert!(!c.turn_active());
    }

    #[test]
    fn two_segments_of_one_caller_turn_start_one_agent_turn() {
        let c = TurnController::new(
            vec![Box::new(DetectorSpeechStart::new(500, 1))],
            vec![Box::new(LegacySpeechFinalStop)],
            vec![],
        );
        let mut events = Vec::new();
        events.extend(c.feed(&ControllerSignal::Speech { sustained_ms: 224 }));
        events.extend(c.feed(&ControllerSignal::SttInterim {
            text: "I'd like to".into(),
            confidence: 1.0,
        }));
        events.extend(c.feed(&ControllerSignal::Speech { sustained_ms: 224 }));
        events.extend(c.feed(&ControllerSignal::SttFinal {
            text: "I'd like to change my booking".into(),
            is_speech_final: true,
            is_finalized: true,
        }));
        let starts = events
            .iter()
            .filter(|e| matches!(e, TurnEvent::Started { .. }))
            .count();
        let stops: Vec<&TurnEvent> = events
            .iter()
            .filter(|e| matches!(e, TurnEvent::Stopped { .. }))
            .collect();
        assert_eq!(starts, 1);
        assert_eq!(stops.len(), 1);
        assert!(
            matches!(stops[0], TurnEvent::Stopped { transcript, .. } if transcript == "I'd like to change my booking")
        );
    }
}
