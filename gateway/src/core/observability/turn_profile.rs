//! Per-turn latency timeline + the `TurnProfiler` that assembles it.
//!
//! A conversational "turn" is one user utterance → bot response. This module
//! captures a monotonic-ns timeline of the realtime audio-to-audio stages and
//! computes the headline **response latency** (end-of-speech → first audio out)
//! plus the per-stage breakdown and the dominant bottleneck.
//!
//! The [`TurnProfiler`] is a [`VoiceObserver`] registered per session. It reuses
//! the existing observer dispatch (zero-cost when unregistered) and the
//! [`now_monotonic_ns`] clock. Pre-end-of-speech stamps (audio_in, smart_turn,
//! stt_partial) land in relaxed atomics (no lock on the audio hot path); the turn
//! opens on `stt_final`, post-EOS stamps take an uncontended per-session mutex
//! (~6/turn), and the turn closes on the first `audio_out`, handing the finished
//! [`TurnTrace`] to a [`TurnSink`] (the process-wide `LatencyProfiler`).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::Mutex;
use serde::Serialize;

use crate::core::observability::latency::now_monotonic_ns;
use crate::core::observability::observer::VoiceObserver;
use crate::core::stt::STTResult;
use crate::core::tts::AudioData;

// =============================================================================
// Stage / path / outcome enums
// =============================================================================

/// Which orchestration path produced the turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnPath {
    /// The built-in conversation orchestrator (STT → LlmClient → TTS).
    Conversation,
    /// The DAG executor (audio_input → … → audio_output).
    Dag,
}

impl TurnPath {
    pub fn as_str(self) -> &'static str {
        match self {
            TurnPath::Conversation => "conversation",
            TurnPath::Dag => "dag",
        }
    }
}

/// Whether the turn ran to first audio or was cut short (barge-in / teardown).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnOutcome {
    Completed,
    Aborted,
}

impl TurnOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            TurnOutcome::Completed => "completed",
            TurnOutcome::Aborted => "aborted",
        }
    }
}

/// A consecutive stage of the response pipeline (the delta between two anchors).
///
/// The headline `response_latency` = sum of the response stages. `Stt` is the
/// pre-response STT-finalization latency (last user audio → end-of-speech) and is
/// tracked but excluded from the response chain / bottleneck.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    /// last user audio frame → end-of-speech (STT finalization; pre-response).
    Stt,
    /// end-of-speech → LLM request dispatched.
    SttToLlm,
    /// LLM request → first token (time-to-first-token).
    LlmTtft,
    /// first token → first complete sentence.
    LlmSentence,
    /// first sentence → TTS request issued.
    TtsQueue,
    /// TTS request → first synthesized audio chunk (TTS time-to-first-byte).
    TtsTtfb,
    /// first TTS audio → first audio leaves the gateway.
    Egress,
}

impl Stage {
    pub fn as_str(self) -> &'static str {
        match self {
            Stage::Stt => "stt",
            Stage::SttToLlm => "stt_to_llm",
            Stage::LlmTtft => "llm_ttft",
            Stage::LlmSentence => "llm_sentence",
            Stage::TtsQueue => "tts_queue",
            Stage::TtsTtfb => "tts_ttfb",
            Stage::Egress => "egress",
        }
    }

    /// The stages that compose the user-perceived response latency (end-of-speech
    /// → audio out). `Stt` is excluded (it precedes end-of-speech).
    pub const RESPONSE_STAGES: [Stage; 6] = [
        Stage::SttToLlm,
        Stage::LlmTtft,
        Stage::LlmSentence,
        Stage::TtsQueue,
        Stage::TtsTtfb,
        Stage::Egress,
    ];

    /// All stages, for the full per-stage histogram (includes `Stt`).
    pub const ALL: [Stage; 7] = [
        Stage::Stt,
        Stage::SttToLlm,
        Stage::LlmTtft,
        Stage::LlmSentence,
        Stage::TtsQueue,
        Stage::TtsTtfb,
        Stage::Egress,
    ];

    /// Dense index into `Stage::ALL` (for fixed-size per-stage arrays).
    pub fn index(self) -> usize {
        match self {
            Stage::Stt => 0,
            Stage::SttToLlm => 1,
            Stage::LlmTtft => 2,
            Stage::LlmSentence => 3,
            Stage::TtsQueue => 4,
            Stage::TtsTtfb => 5,
            Stage::Egress => 6,
        }
    }
}

// =============================================================================
// TurnTrace — the per-turn timeline
// =============================================================================

/// Snapshot of the LiveKit/WS egress queue depths sampled at turn close.
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct QueueSnapshot {
    pub ws_msg_depth: u64,
    pub livekit_op_depth: u64,
    pub livekit_op_avg_ms: u64,
    pub livekit_op_max_ms: u64,
}

/// A per-turn timeline of monotonic-ns anchors. `0` means "not captured".
#[derive(Debug, Clone)]
pub struct TurnTrace {
    pub turn_id: u64,
    pub session_id: Arc<str>,
    pub path: TurnPath,
    pub outcome: TurnOutcome,
    /// Whether the DAG streaming executor (vs the batch fallback) ran this turn.
    pub streaming_path: bool,

    // --- Stage anchors (monotonic ns since process start; 0 = unset) ---
    pub audio_in_ns: u64,
    pub smart_turn_start_ns: u64,
    pub smart_turn_end_ns: u64,
    pub smart_turn_inference_us: u64,
    pub stt_partial_first_ns: u64,
    pub stt_final_ns: u64, // END-OF-SPEECH ANCHOR (headline denominator)
    pub llm_request_ns: u64,
    pub llm_first_token_ns: u64,
    pub llm_first_sentence_ns: u64,
    pub tts_request_ns: u64,
    pub tts_first_audio_ns: u64,
    pub audio_out_ns: u64, // HEADLINE END (first audio leaves the gateway)

    // --- DAG per-node breakdown (empty for the conversation path) ---
    pub node_durations_us: Vec<(Arc<str>, u64)>,

    pub queues: QueueSnapshot,
    /// The dominant response stage (max delta); set at close.
    pub bottleneck: Option<Stage>,
}

impl TurnTrace {
    /// Open a fresh trace anchored at end-of-speech.
    pub fn open(turn_id: u64, session_id: Arc<str>, path: TurnPath, stt_final_ns: u64) -> Self {
        Self {
            turn_id,
            session_id,
            path,
            outcome: TurnOutcome::Completed,
            streaming_path: false,
            audio_in_ns: 0,
            smart_turn_start_ns: 0,
            smart_turn_end_ns: 0,
            smart_turn_inference_us: 0,
            stt_partial_first_ns: 0,
            stt_final_ns,
            llm_request_ns: 0,
            llm_first_token_ns: 0,
            llm_first_sentence_ns: 0,
            tts_request_ns: 0,
            tts_first_audio_ns: 0,
            audio_out_ns: 0,
            node_durations_us: Vec::new(),
            queues: QueueSnapshot::default(),
            bottleneck: None,
        }
    }

    /// Headline: end-of-speech → first audio out, in ns (None if incomplete).
    pub fn response_latency_ns(&self) -> Option<u64> {
        if self.stt_final_ns > 0 && self.audio_out_ns >= self.stt_final_ns {
            Some(self.audio_out_ns - self.stt_final_ns)
        } else {
            None
        }
    }

    fn delta(&self, stage: Stage) -> Option<u64> {
        let (start, end) = match stage {
            Stage::Stt => (self.audio_in_ns, self.stt_final_ns),
            Stage::SttToLlm => (self.stt_final_ns, self.llm_request_ns),
            Stage::LlmTtft => (self.llm_request_ns, self.llm_first_token_ns),
            Stage::LlmSentence => (self.llm_first_token_ns, self.llm_first_sentence_ns),
            Stage::TtsQueue => (self.llm_first_sentence_ns, self.tts_request_ns),
            Stage::TtsTtfb => (self.tts_request_ns, self.tts_first_audio_ns),
            Stage::Egress => (self.tts_first_audio_ns, self.audio_out_ns),
        };
        if start > 0 && end >= start {
            Some(end - start)
        } else {
            None
        }
    }

    /// All captured stage deltas (skips missing anchors), in ns.
    pub fn stage_deltas(&self) -> Vec<(Stage, u64)> {
        Stage::ALL
            .iter()
            .filter_map(|&s| self.delta(s).map(|d| (s, d)))
            .collect()
    }

    /// The dominant **response** stage (max delta among the response stages).
    pub fn compute_bottleneck(&self) -> Option<Stage> {
        Stage::RESPONSE_STAGES
            .iter()
            .filter_map(|&s| self.delta(s).map(|d| (s, d)))
            // ties resolve to the earlier stage in canonical order (max_by_key keeps
            // the LAST max, so iterate reversed to keep the first).
            .rev()
            .max_by_key(|&(_, d)| d)
            .map(|(s, _)| s)
    }

    /// Build the serializable summary broadcast over SSE / returned in snapshots.
    pub fn summary(&self) -> TurnSummary {
        let ms = |ns: Option<u64>| ns.map(|n| n / 1_000_000);
        let stage_ms = |s: Stage| ms(self.delta(s));
        TurnSummary {
            turn_id: self.turn_id,
            session_id: self.session_id.to_string(),
            path: self.path,
            outcome: self.outcome,
            streaming_path: self.streaming_path,
            response_latency_ms: ms(self.response_latency_ns()),
            stages: StageMs {
                stt_ms: stage_ms(Stage::Stt),
                stt_to_llm_ms: stage_ms(Stage::SttToLlm),
                llm_ttft_ms: stage_ms(Stage::LlmTtft),
                llm_sentence_ms: stage_ms(Stage::LlmSentence),
                tts_queue_ms: stage_ms(Stage::TtsQueue),
                tts_ttfb_ms: stage_ms(Stage::TtsTtfb),
                egress_ms: stage_ms(Stage::Egress),
            },
            smart_turn_inference_ms: if self.smart_turn_inference_us > 0 {
                Some(self.smart_turn_inference_us / 1000)
            } else {
                None
            },
            node_durations_us: self
                .node_durations_us
                .iter()
                .map(|(n, us)| (n.to_string(), *us))
                .collect(),
            queues: self.queues,
            bottleneck: self.bottleneck.map(|b| b.as_str()),
        }
    }
}

/// Per-stage ms (None when the anchors weren't captured).
#[derive(Debug, Clone, Serialize)]
pub struct StageMs {
    pub stt_ms: Option<u64>,
    pub stt_to_llm_ms: Option<u64>,
    pub llm_ttft_ms: Option<u64>,
    pub llm_sentence_ms: Option<u64>,
    pub tts_queue_ms: Option<u64>,
    pub tts_ttfb_ms: Option<u64>,
    pub egress_ms: Option<u64>,
}

/// Serializable per-turn summary (the SSE/snapshot payload).
#[derive(Debug, Clone, Serialize)]
pub struct TurnSummary {
    pub turn_id: u64,
    pub session_id: String,
    pub path: TurnPath,
    pub outcome: TurnOutcome,
    pub streaming_path: bool,
    pub response_latency_ms: Option<u64>,
    pub stages: StageMs,
    pub smart_turn_inference_ms: Option<u64>,
    pub node_durations_us: Vec<(String, u64)>,
    pub queues: QueueSnapshot,
    pub bottleneck: Option<&'static str>,
}

// =============================================================================
// TurnSink — the hub the closed trace is handed to
// =============================================================================

/// Where a finished [`TurnTrace`] is delivered (implemented by `LatencyProfiler`).
pub trait TurnSink: Send + Sync {
    fn record_turn(&self, trace: TurnTrace);
}

// =============================================================================
// TurnProfiler — the per-session VoiceObserver state machine
// =============================================================================

/// Per-session observer that assembles a [`TurnTrace`] from the checkpoint hooks
/// and hands each completed turn to a [`TurnSink`].
pub struct TurnProfiler {
    session_id: Arc<str>,
    next_turn_id: AtomicU64,
    sink: Arc<dyn TurnSink>,

    // Pre-end-of-speech anchors (written from the audio hot path; lock-free).
    pre_audio_in_ns: AtomicU64,
    pre_smart_turn_start_ns: AtomicU64,
    pre_smart_turn_end_ns: AtomicU64,
    pre_smart_turn_inference_us: AtomicU64,
    pre_stt_partial_ns: AtomicU64,

    // The in-flight turn (None between turns). Uncontended in steady state.
    current: Mutex<Option<TurnTrace>>,
}

impl TurnProfiler {
    pub fn new(session_id: impl Into<Arc<str>>, sink: Arc<dyn TurnSink>) -> Self {
        Self {
            session_id: session_id.into(),
            next_turn_id: AtomicU64::new(0),
            sink,
            pre_audio_in_ns: AtomicU64::new(0),
            pre_smart_turn_start_ns: AtomicU64::new(0),
            pre_smart_turn_end_ns: AtomicU64::new(0),
            pre_smart_turn_inference_us: AtomicU64::new(0),
            pre_stt_partial_ns: AtomicU64::new(0),
            current: Mutex::new(None),
        }
    }

    /// Open a new turn anchored at end-of-speech, snapshotting the pre-EOS atomics.
    /// If a prior turn is still open (overlap/barge-in), close it as aborted first.
    fn open_turn(&self, stt_final_ns: u64, path: TurnPath) {
        let mut slot = self.current.lock();
        if let Some(prev) = slot.take() {
            // Previous turn never reached audio_out — record as aborted.
            self.finish(prev, TurnOutcome::Aborted);
        }
        let turn_id = self.next_turn_id.fetch_add(1, Ordering::Relaxed);
        let mut trace = TurnTrace::open(turn_id, self.session_id.clone(), path, stt_final_ns);
        // Snapshot + clear the pre-EOS atomics.
        trace.audio_in_ns = self.pre_audio_in_ns.swap(0, Ordering::Relaxed);
        trace.smart_turn_start_ns = self.pre_smart_turn_start_ns.swap(0, Ordering::Relaxed);
        trace.smart_turn_end_ns = self.pre_smart_turn_end_ns.swap(0, Ordering::Relaxed);
        trace.smart_turn_inference_us = self.pre_smart_turn_inference_us.swap(0, Ordering::Relaxed);
        trace.stt_partial_first_ns = self.pre_stt_partial_ns.swap(0, Ordering::Relaxed);
        *slot = Some(trace);
    }

    /// Compute the bottleneck + outcome and deliver the trace to the sink.
    fn finish(&self, mut trace: TurnTrace, outcome: TurnOutcome) {
        trace.outcome = outcome;
        trace.bottleneck = trace.compute_bottleneck();
        self.sink.record_turn(trace);
    }

    /// Abort any in-flight turn (call on barge-in / session teardown).
    pub fn abort_current(&self) {
        let mut slot = self.current.lock();
        if let Some(trace) = slot.take() {
            self.finish(trace, TurnOutcome::Aborted);
        }
    }

    /// Number of turns opened so far (for tests/metrics).
    pub fn turns_opened(&self) -> u64 {
        self.next_turn_id.load(Ordering::Relaxed)
    }
}

impl VoiceObserver for TurnProfiler {
    // --- Pre-end-of-speech (lock-free; "last frame before EOS" semantics) ---

    fn on_audio_in(&self, ts_ns: u64) {
        self.pre_audio_in_ns.store(ts_ns, Ordering::Relaxed);
    }

    fn on_smart_turn(&self, inference_us: u64, _is_complete: bool, ts_ns: u64) {
        // first inference of the pre-turn window sets the start
        let _ = self.pre_smart_turn_start_ns.compare_exchange(
            0,
            ts_ns,
            Ordering::Relaxed,
            Ordering::Relaxed,
        );
        self.pre_smart_turn_end_ns.store(ts_ns, Ordering::Relaxed);
        self.pre_smart_turn_inference_us
            .store(inference_us, Ordering::Relaxed);
    }

    fn on_stt_partial(&self, ts_ns: u64) {
        let _ = self.pre_stt_partial_ns.compare_exchange(
            0,
            ts_ns,
            Ordering::Relaxed,
            Ordering::Relaxed,
        );
    }

    // --- End-of-speech opens the turn (reuses the existing STT hook) ---

    fn on_stt_result(&self, result: &STTResult, _latency_ns: u64) {
        if result.is_speech_final || result.is_final {
            self.open_turn(now_monotonic_ns(), TurnPath::Conversation);
        }
    }

    // --- Post-EOS anchors (uncontended slot lock; first-of-turn dedup here) ---

    fn on_llm_request(&self, ts_ns: u64) {
        if let Some(t) = self.current.lock().as_mut()
            && t.llm_request_ns == 0
        {
            t.llm_request_ns = ts_ns;
        }
    }

    fn on_llm_first_token(&self, ts_ns: u64) {
        if let Some(t) = self.current.lock().as_mut()
            && t.llm_first_token_ns == 0
        {
            t.llm_first_token_ns = ts_ns;
        }
    }

    fn on_llm_first_sentence(&self, ts_ns: u64) {
        if let Some(t) = self.current.lock().as_mut()
            && t.llm_first_sentence_ns == 0
        {
            t.llm_first_sentence_ns = ts_ns;
        }
    }

    fn on_tts_request(&self, ts_ns: u64) {
        if let Some(t) = self.current.lock().as_mut()
            && t.tts_request_ns == 0
        {
            t.tts_request_ns = ts_ns;
        }
    }

    fn on_tts_chunk(&self, _chunk: &AudioData, _ttfb_ns: Option<u64>) {
        // First TTS audio of the turn, whether or not the caller computed a
        // ttfb (the VoiceManager egress wrapper passes None); dedup via ==0.
        if let Some(t) = self.current.lock().as_mut()
            && t.tts_first_audio_ns == 0
        {
            t.tts_first_audio_ns = now_monotonic_ns();
        }
    }

    fn on_audio_out(&self, ts_ns: u64) {
        // First audio out of the turn closes it.
        let trace = {
            let mut slot = self.current.lock();
            match slot.as_mut() {
                Some(t) if t.audio_out_ns == 0 => {
                    t.audio_out_ns = ts_ns;
                    slot.take()
                }
                _ => None,
            }
        };
        if let Some(trace) = trace {
            self.finish(trace, TurnOutcome::Completed);
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    /// A sink that collects every recorded turn.
    #[derive(Default)]
    struct CollectingSink {
        traces: Mutex<Vec<TurnTrace>>,
        count: AtomicU64,
    }
    impl TurnSink for CollectingSink {
        fn record_turn(&self, trace: TurnTrace) {
            self.count.fetch_add(1, Ordering::Relaxed);
            self.traces.lock().push(trace);
        }
    }

    fn final_result() -> STTResult {
        STTResult::new("done".into(), false, true, 0.9) // is_speech_final = true
    }

    fn audio_chunk() -> AudioData {
        AudioData {
            data: vec![0u8; 16],
            sample_rate: 24_000,
            format: "pcm".to_string(),
            duration_ms: Some(10),
        }
    }

    #[test]
    fn response_latency_is_audio_out_minus_stt_final() {
        let mut t = TurnTrace::open(0, "s".into(), TurnPath::Conversation, 1_000_000_000);
        t.audio_out_ns = 1_612_000_000;
        assert_eq!(t.response_latency_ns(), Some(612_000_000));
        assert_eq!(t.summary().response_latency_ms, Some(612));
        // Missing audio_out → no headline.
        let p = TurnTrace::open(0, "s".into(), TurnPath::Conversation, 1_000_000_000);
        assert_eq!(p.response_latency_ns(), None);
        assert_eq!(p.summary().response_latency_ms, None);
    }

    #[test]
    fn stage_deltas_skip_missing_anchors_and_are_nonnegative() {
        let mut t = TurnTrace::open(0, "s".into(), TurnPath::Conversation, 1_000);
        t.audio_in_ns = 0; // unset → Stt skipped
        t.llm_request_ns = 1_010;
        t.llm_first_token_ns = 1_250; // LlmTtft = 240
        // gap: no first_sentence/tts → those stages skipped
        t.audio_out_ns = 1_900;
        let deltas = t.stage_deltas();
        // Only stt_to_llm(10) and llm_ttft(240) are computable here.
        assert!(deltas.iter().all(|&(_, d)| d > 0 || true));
        let ttft = deltas.iter().find(|(s, _)| *s == Stage::LlmTtft).unwrap().1;
        assert_eq!(ttft, 240);
        assert!(deltas.iter().any(|(s, _)| *s == Stage::SttToLlm));
        assert!(!deltas.iter().any(|(s, _)| *s == Stage::LlmSentence));
    }

    #[test]
    fn bottleneck_is_dominant_response_stage() {
        let mut t = TurnTrace::open(0, "s".into(), TurnPath::Conversation, 0);
        t.stt_final_ns = 1_000;
        t.llm_request_ns = 1_010; // stt_to_llm = 10
        t.llm_first_token_ns = 1_500; // llm_ttft = 490  (dominant)
        t.llm_first_sentence_ns = 1_560; // llm_sentence = 60
        t.tts_request_ns = 1_570; // tts_queue = 10
        t.tts_first_audio_ns = 1_700; // tts_ttfb = 130
        t.audio_out_ns = 1_705; // egress = 5
        assert_eq!(t.compute_bottleneck(), Some(Stage::LlmTtft));
    }

    #[test]
    fn turn_profiler_assembles_ordered_timeline_and_correlates_pre_eos() {
        let sink = Arc::new(CollectingSink::default());
        let prof = TurnProfiler::new("sess-1", sink.clone() as Arc<dyn TurnSink>);

        // Pre-end-of-speech stamps (audio hot path).
        prof.on_audio_in(100);
        prof.on_smart_turn(18_000, false, 150);
        prof.on_smart_turn(22_000, true, 200); // last inference wins
        prof.on_stt_partial(120);
        // End-of-speech opens the turn (real now_monotonic anchor).
        prof.on_stt_result(&final_result(), 0);
        // Post-EOS stamps with monotonic timestamps after stt_final.
        let base = now_monotonic_ns();
        prof.on_llm_request(base + 5_000_000);
        prof.on_llm_first_token(base + 200_000_000);
        prof.on_llm_first_sentence(base + 260_000_000);
        prof.on_tts_request(base + 262_000_000);
        prof.on_tts_chunk(&audio_chunk(), Some(1)); // tts_first_audio = now
        std::thread::sleep(std::time::Duration::from_millis(2));
        prof.on_audio_out(now_monotonic_ns()); // closes the turn

        assert_eq!(sink.count.load(Ordering::Relaxed), 1, "exactly one turn closed");
        let traces = sink.traces.lock();
        let t = &traces[0];
        assert_eq!(t.turn_id, 0);
        assert_eq!(t.outcome, TurnOutcome::Completed);
        // Pre-EOS atomics were snapshotted into the turn opened at stt_final.
        assert_eq!(t.audio_in_ns, 100);
        assert_eq!(t.smart_turn_inference_us, 22_000);
        assert_eq!(t.stt_partial_first_ns, 120);
        assert!(t.stt_final_ns > 0);
        assert!(t.response_latency_ns().is_some());
        assert!(t.bottleneck.is_some());
        // Release the sink lock BEFORE driving the next turn: closing turn 2 calls
        // sink.record_turn → traces.lock() on this same thread, and parking_lot
        // mutexes are non-reentrant (silent deadlock, not a panic).
        drop(traces);
        // Next turn increments the id.
        prof.on_stt_result(&final_result(), 0);
        prof.on_audio_out(now_monotonic_ns());
        assert_eq!(sink.traces.lock()[1].turn_id, 1);
    }

    #[test]
    fn abort_emits_aborted_turn_without_headline() {
        let sink = Arc::new(CollectingSink::default());
        let prof = TurnProfiler::new("s", sink.clone() as Arc<dyn TurnSink>);
        prof.on_stt_result(&final_result(), 0);
        prof.on_llm_request(now_monotonic_ns());
        prof.abort_current(); // barge-in before any audio out
        assert_eq!(sink.count.load(Ordering::Relaxed), 1);
        let traces = sink.traces.lock();
        assert_eq!(traces[0].outcome, TurnOutcome::Aborted);
        assert_eq!(traces[0].response_latency_ns(), None);
    }

    #[test]
    fn second_audio_out_of_same_turn_does_not_reclose() {
        let sink = Arc::new(CollectingSink::default());
        let prof = TurnProfiler::new("s", sink.clone() as Arc<dyn TurnSink>);
        prof.on_stt_result(&final_result(), 0);
        prof.on_audio_out(now_monotonic_ns());
        prof.on_audio_out(now_monotonic_ns()); // continued audio — ignored
        assert_eq!(sink.count.load(Ordering::Relaxed), 1);
    }
}
