//! The voice manager's side of a segmented session.
//!
//! The engine reports speech events, upload outcomes and notices on its own task. This dispatcher
//! fans them out to every consumer that registered (turn-taking, the wire, metering), accepting
//! consumers that register after the session started. It also decides, once per caller turn and at
//! the moment the caller starts speaking, whether that speech is input: speech over a greeting that
//! may not be cut is neither announced, uploaded nor shown. Deciding when the transcript arrives
//! would answer the wrong question, because on this path it arrives a second or more later.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::{Mutex, RwLock};

use crate::core::stt::speech_activity::{
    NoticeCallback, SegmentAdmission, SegmentMeta, SegmentOutcome, SegmentOutcomeSink,
    SpeechActivity, SpeechActivityCallback, SttNotice,
};

/// Receives each upload outcome.
pub type OutcomeListener = Arc<dyn Fn(&SegmentOutcome) + Send + Sync>;
/// Whether the caller's speech is input right now.
pub type SpeechGate = Arc<dyn Fn() -> bool + Send + Sync>;

#[derive(Default)]
pub struct SegmentedDispatch {
    speech: RwLock<Vec<SpeechActivityCallback>>,
    outcomes: RwLock<Vec<OutcomeListener>>,
    notices: RwLock<Vec<NoticeCallback>>,
    gate: RwLock<Option<SpeechGate>>,
    agent_audible: RwLock<Option<SpeechGate>>,
    admitted: Mutex<HashMap<u64, bool>>,
}

impl std::fmt::Debug for SegmentedDispatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SegmentedDispatch")
            .field("speech_listeners", &self.speech.read().len())
            .field("outcome_listeners", &self.outcomes.read().len())
            .finish()
    }
}

impl SegmentedDispatch {
    pub fn add_speech_listener(&self, cb: SpeechActivityCallback) {
        self.speech.write().push(cb);
    }

    pub fn add_outcome_listener(&self, cb: OutcomeListener) {
        self.outcomes.write().push(cb);
    }

    pub fn add_notice_listener(&self, cb: NoticeCallback) {
        self.notices.write().push(cb);
    }

    /// Replace the gate (a voice agent's: it accepts speech while idle, and while it speaks only
    /// if what it says may be cut).
    pub fn set_gate(&self, gate: SpeechGate) {
        *self.gate.write() = Some(gate);
    }

    /// Whether the agent is audible, the overlap evidence for the quality check.
    pub fn set_agent_audible(&self, probe: SpeechGate) {
        *self.agent_audible.write() = Some(probe);
    }

    fn gate_now(&self) -> bool {
        self.gate.read().as_ref().is_none_or(|g| g())
    }

    /// The admission of a caller turn, decided the first time the turn is seen.
    fn admitted(&self, turn_id: u64) -> bool {
        if let Some(a) = self.admitted.lock().get(&turn_id) {
            return *a;
        }
        let a = self.gate_now();
        *self.admitted.lock().entry(turn_id).or_insert(a)
    }

    /// One speech event from the engine: decide admission, forward admitted turns' events.
    pub fn on_activity(&self, a: SpeechActivity) {
        let turn = a.turn_id();
        let admitted = self.admitted(turn);
        if let SpeechActivity::TurnClosed {
            result_follows: false,
            ..
        } = a
        {
            self.admitted.lock().remove(&turn);
        }
        if !admitted {
            tracing::debug!(
                turn,
                "caller speech while the agent may not be cut: not input"
            );
            return;
        }
        for cb in self.speech.read().iter() {
            cb(a.clone());
        }
    }

    /// The engine's question at each cut.
    pub fn admission(&self, meta: &SegmentMeta) -> SegmentAdmission {
        let audible = self.agent_audible.read().as_ref().is_some_and(|p| p());
        SegmentAdmission {
            is_input: self.admitted(meta.turn_id),
            overlapped_agent_speech: audible,
            agent_spoke_since_previous_segment: audible,
        }
    }

    /// Whether a result of this turn may pass. The record is kept until the turn's final passed.
    pub fn result_admitted(&self, turn_id: u64, is_final: bool) -> bool {
        let a = self.admitted(turn_id);
        if is_final {
            self.admitted.lock().remove(&turn_id);
        }
        a
    }

    pub fn on_outcome(&self, o: &SegmentOutcome) {
        for cb in self.outcomes.read().iter() {
            cb(o);
        }
    }

    pub fn on_notice(&self, n: SttNotice) {
        for cb in self.notices.read().iter() {
            cb(n.clone());
        }
    }
}

/// The dispatcher as the engine's one outcome sink.
pub struct DispatchSink(pub Arc<SegmentedDispatch>);

impl SegmentOutcomeSink for DispatchSink {
    fn record(&self, outcome: &SegmentOutcome) {
        self.0.on_outcome(outcome);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::stt::speech_activity::TurnCloseReason;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn started(turn: u64) -> SpeechActivity {
        SpeechActivity::Started {
            turn_id: turn,
            at_sample: 0,
            sustained_ms: 224,
            at_mono_ms: 0,
        }
    }

    #[test]
    fn admission_is_decided_once_per_turn_when_the_caller_starts_speaking() {
        let d = SegmentedDispatch::default();
        let open = Arc::new(AtomicBool::new(false));
        let g = Arc::clone(&open);
        d.set_gate(Arc::new(move || g.load(Ordering::SeqCst)));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s = Arc::clone(&seen);
        d.add_speech_listener(Arc::new(move |a| s.lock().push(a)));
        d.on_activity(started(1)); // over a protected greeting: not input
        open.store(true, Ordering::SeqCst); // the greeting ends mid-utterance
        d.on_activity(started(1));
        assert!(
            seen.lock().is_empty(),
            "the decision holds for the whole turn"
        );
        assert!(!d.result_admitted(1, true));
        d.on_activity(started(2));
        assert_eq!(seen.lock().len(), 1);
        assert!(d.result_admitted(2, false));
    }

    #[test]
    fn segment_outcomes_reach_every_listener_and_survive_late_registration() {
        let d = Arc::new(SegmentedDispatch::default());
        let sink = DispatchSink(Arc::clone(&d));
        let first = Arc::new(Mutex::new(0));
        let f = Arc::clone(&first);
        d.add_outcome_listener(Arc::new(move |_| *f.lock() += 1));
        let o = SegmentOutcome {
            turn_id: 1,
            seq: 1,
            index_in_turn: 0,
            speech_segments: 1,
            turn_final: false,
            voiced_ms: 0,
            audio_ms: 0,
            joined_text_offset: 0,
            uploaded_seconds: 0.0,
            billed_seconds: 0.0,
            timings: Default::default(),
            kind: crate::core::stt::speech_activity::SegmentResultKind::Text,
            retries: 0,
            overlapped_agent_speech: false,
            suspect: false,
            cut: crate::core::stt::speech_activity::CutReason::Pause,
            short: false,
            detector: crate::core::stt::speech_activity::DetectorKind::Scripted,
            vendor_request_id: None,
        };
        sink.record(&o);
        let late = Arc::new(Mutex::new(0));
        let l = Arc::clone(&late);
        d.add_outcome_listener(Arc::new(move |_| *l.lock() += 1));
        sink.record(&o);
        assert_eq!(*first.lock(), 2);
        assert_eq!(*late.lock(), 1);
    }

    #[test]
    fn a_turn_closed_without_a_result_frees_its_record() {
        let d = SegmentedDispatch::default();
        d.on_activity(started(5));
        d.on_activity(SpeechActivity::TurnClosed {
            turn_id: 5,
            had_text: false,
            result_follows: false,
            segments: 1,
            gaps: 0,
            lost_voiced_ms: 0,
            reason: TurnCloseReason::NoText,
            speech_end_mono_ms: 0,
        });
        assert!(d.admitted.lock().is_empty());
    }
}
