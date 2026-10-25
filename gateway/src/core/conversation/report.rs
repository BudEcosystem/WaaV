//! What a conversation turn tells its client while it runs.
//!
//! The same lifecycle a voice-agent turn reports ([`AgentSignal`]): the reply started, its words as
//! they are handed to speech, how it ended, and what was heard when it was cut off. `/ws` sends
//! them as `agent_response_started`, `assistant_transcript`, `agent_response_done` and
//! `agent_truncated`; the `/v1/realtime` cascade turns them into GA response events. Without a
//! receiver every call is a no-op.

use std::sync::Arc;

use parking_lot::Mutex as SyncMutex;
use tokio::sync::mpsc::UnboundedSender;

use crate::core::agent::text::chunk_separator;
use crate::core::agent::{AgentSignal, TurnKind, TurnStatus};

/// One turn's reporter. Cheap to clone (the streaming pump holds one).
#[derive(Clone)]
pub(super) struct TurnReport {
    tx: Option<UnboundedSender<AgentSignal>>,
    turn: u64,
    /// Everything reported as spoken so far, joined as the client sees it.
    spoken: Arc<SyncMutex<String>>,
    /// The session's delivered-audio total when the turn started.
    audio_mark: u64,
}

impl TurnReport {
    /// Start reporting turn `turn`; announces it when there is a receiver.
    pub(super) fn start(
        tx: Option<UnboundedSender<AgentSignal>>,
        turn: u64,
        input: Option<&str>,
        audio_mark: u64,
    ) -> Self {
        let report = Self {
            tx,
            turn,
            spoken: Arc::new(SyncMutex::new(String::new())),
            audio_mark,
        };
        report.send(AgentSignal::ResponseStarted {
            turn,
            kind: TurnKind::Agent,
            input: input.map(str::to_string),
        });
        report
    }

    fn send(&self, signal: AgentSignal) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(signal);
        }
    }

    /// `text` was handed to speech: report it as the next transcript delta.
    pub(super) fn spoke(&self, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let delta = {
            let mut spoken = self.spoken.lock();
            let delta = format!("{}{text}", chunk_separator(&spoken, text));
            spoken.push_str(&delta);
            delta
        };
        self.send(AgentSignal::Transcript {
            turn: self.turn,
            delta,
        });
    }

    /// The turn ended. A cancelled turn that had spoken also reports what was cut:
    /// `audio_end_ms` is the reply's audio delivered by then (`audio_now - mark`).
    pub(super) fn done(&self, status: TurnStatus, audio_now: u64) {
        let transcript = self.spoken.lock().clone();
        if status == TurnStatus::Cancelled && !transcript.is_empty() {
            self.send(AgentSignal::Truncated {
                turn: self.turn,
                response_id: None,
                spoken: transcript.clone(),
                audio_end_ms: audio_now.saturating_sub(self.audio_mark),
            });
        }
        self.send(AgentSignal::ResponseDone {
            turn: self.turn,
            kind: TurnKind::Agent,
            response_id: None,
            status,
            usage: None,
            transcript,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_turn_reports_start_spoken_words_and_its_end() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let report = TurnReport::start(Some(tx), 3, Some("what is the capital"), 1_000);
        report.spoke("Paris is the capital.");
        report.spoke("  ");
        report.spoke("It is in France.");
        report.done(TurnStatus::Completed, 4_000);
        let got: Vec<AgentSignal> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert_eq!(
            got,
            vec![
                AgentSignal::ResponseStarted {
                    turn: 3,
                    kind: TurnKind::Agent,
                    input: Some("what is the capital".into()),
                },
                AgentSignal::Transcript {
                    turn: 3,
                    delta: "Paris is the capital.".into(),
                },
                AgentSignal::Transcript {
                    turn: 3,
                    delta: " It is in France.".into(),
                },
                AgentSignal::ResponseDone {
                    turn: 3,
                    kind: TurnKind::Agent,
                    response_id: None,
                    status: TurnStatus::Completed,
                    usage: None,
                    transcript: "Paris is the capital. It is in France.".into(),
                },
            ]
        );
    }

    #[test]
    fn a_cancelled_turn_reports_what_was_cut() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let report = TurnReport::start(Some(tx), 0, None, 500);
        report.spoke("First sentence.");
        report.done(TurnStatus::Cancelled, 1_700);
        let got: Vec<AgentSignal> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert!(matches!(
            &got[2],
            AgentSignal::Truncated { spoken, audio_end_ms: 1_200, .. } if spoken == "First sentence."
        ));
        assert!(matches!(
            &got[3],
            AgentSignal::ResponseDone {
                status: TurnStatus::Cancelled,
                ..
            }
        ));
        // Nothing spoken: no truncation to report.
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        TurnReport::start(Some(tx), 1, None, 0).done(TurnStatus::Cancelled, 0);
        let got: Vec<AgentSignal> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn without_a_receiver_nothing_is_sent() {
        let report = TurnReport::start(None, 0, Some("hi"), 0);
        report.spoke("Hello.");
        report.done(TurnStatus::Completed, 0);
    }
}
