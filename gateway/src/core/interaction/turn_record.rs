//! The `voice.turn` record of one caller turn (spec 029 D-9).
//!
//! budmetrics keeps one row per trace (its turn id is the `TraceId`, and the `voice.turn` spans
//! of one trace fold into that row), so a caller turn is one trace. This record is its root,
//! opened at the end of the turn: what the front decided (`bud.voice.interaction.act`), the
//! task the turn started or named (`…task_ref`), and from the end of the turn the first agent
//! audio (`bud.voice.response_latency_ms`), the first line that replies to the turn
//! (`…ack_latency_ms`) and the first answer sentence (`…answer_latency_ms`). Every task run
//! the turn starts records its own `voice.turn` as a child in the same trace (its agent, its
//! model, budprompt's response), and budgateway and budprompt continue that trace, so the
//! run's cost joins the turn. A turn the front handles alone (an acknowledgement, a control, a
//! wait) is the root alone.
//!
//! Before, each task run was a trace of its own and front-only turns had no record (spec 029 W-7).

use crate::observability::voice_attrs::{interaction, turn as attr};

use super::types::{Act, Ms, TurnId};

/// What a run needs to record itself in its caller turn's trace.
#[derive(Debug, Clone)]
pub struct TurnTrace {
    /// The turn's root `voice.turn`: the run's span is its child.
    pub span: tracing::Span,
    /// The caller turn's index (`bud.voice.turn_index`), which the run records too: the
    /// spans of one trace fold into one row, the later one's value winning.
    pub index: u64,
}

/// One caller turn's record and what is still to be measured of it.
#[derive(Debug)]
pub struct TurnRecord {
    pub turn: TurnId,
    e: Ms,
    span: tracing::Span,
    response_ms: Option<u64>,
    ack_ms: Option<u64>,
    answer_ms: Option<u64>,
    task_ref: Option<String>,
}

impl TurnRecord {
    /// `span` is the turn's root `voice.turn` (`Backend::turn_span`), `e` its end of turn.
    pub fn open(span: tracing::Span, turn: TurnId, e: Ms) -> Self {
        Self {
            turn,
            e,
            span,
            response_ms: None,
            ack_ms: None,
            answer_ms: None,
            task_ref: None,
        }
    }

    /// For a run the turn starts.
    pub fn trace(&self) -> TurnTrace {
        TurnTrace {
            span: self.span.clone(),
            index: self.turn.0,
        }
    }

    /// The front's act for the turn.
    pub fn settled(&self, act: Act) {
        self.span.record(interaction::ACT, act.as_str());
    }

    /// A task the turn started or named; the first one is the turn's.
    pub fn task(&mut self, task_ref: &str) {
        if self.task_ref.is_none() {
            self.span.record(interaction::TASK_REF, task_ref);
            self.task_ref = Some(task_ref.to_owned());
        }
    }

    /// A line of the agent's began to play at `at`: `answer` for an answer sentence, `reply`
    /// for a line that replies to this turn. The first of each is measured.
    pub fn audible(&mut self, at: Ms, answer: bool, reply: bool) {
        let ms = at.saturating_sub(self.e);
        let first = |slot: &mut Option<u64>, key: &str, span: &tracing::Span| {
            if slot.is_none() {
                *slot = Some(ms);
                span.record(key, ms);
            }
        };
        first(&mut self.response_ms, attr::RESPONSE_LATENCY_MS, &self.span);
        if answer {
            first(
                &mut self.answer_ms,
                interaction::ANSWER_LATENCY_MS,
                &self.span,
            );
        } else if reply {
            first(&mut self.ack_ms, interaction::ACK_LATENCY_MS, &self.span);
        }
    }

    pub fn response_ms(&self) -> Option<u64> {
        self.response_ms
    }

    pub fn ack_ms(&self) -> Option<u64> {
        self.ack_ms
    }

    pub fn answer_ms(&self) -> Option<u64> {
        self.answer_ms
    }

    pub fn task_ref(&self) -> Option<&str> {
        self.task_ref.as_deref()
    }

    /// Where the turn ended.
    pub fn e(&self) -> Ms {
        self.e
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> TurnRecord {
        TurnRecord::open(tracing::Span::none(), TurnId(4), 10_000)
    }

    #[test]
    fn the_first_audio_the_reply_and_the_answer_are_measured_from_the_end_of_the_turn() {
        let mut turn = record();
        // A stall line of an earlier task plays first: the caller heard the agent.
        turn.audible(10_400, false, false);
        // The acknowledgement of this turn, then the answer.
        turn.audible(11_100, false, true);
        turn.audible(13_250, true, false);
        // Later lines measure nothing.
        turn.audible(14_000, false, true);
        turn.audible(15_000, true, false);
        assert_eq!(
            (turn.response_ms(), turn.ack_ms(), turn.answer_ms()),
            (Some(400), Some(1_100), Some(3_250))
        );
    }

    #[test]
    fn an_answer_with_no_acknowledgement_is_the_first_audio() {
        let mut turn = record();
        turn.audible(12_000, true, true);
        assert_eq!(
            (turn.response_ms(), turn.ack_ms(), turn.answer_ms()),
            (Some(2_000), None, Some(2_000))
        );
    }

    #[test]
    fn the_first_task_is_the_turns() {
        let mut turn = record();
        turn.task("t2");
        turn.task("t3");
        assert_eq!(turn.task_ref(), Some("t2"));
        assert_eq!(turn.trace().index, 4);
    }
}
