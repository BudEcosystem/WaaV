//! The segmented session engine: one task per session that owns all state.
//!
//! ```text
//! send_audio --(ingest queue: Audio | Flush | Language, in arrival order)--> ENGINE TASK
//!   front end -> detector -> segmenter -> ladder -> units -> in-order release -> turn assembly
//!   activity callback <- Started, Stopped, EndpointDecided, TurnClosed   (synchronous, never blocks)
//!   outcome sink      <- one SegmentOutcome per unit, at release          (synchronous)
//!   [result queue] --> result emitter --await--> result callback
//!   [fatal queue]  --> fatal emitter  --await--> fatal callback
//! ```
//!
//! What the rest of the gateway receives: while a turn is open, each returned segment produces an
//! interim carrying the turn's text so far; when the turn closes, exactly one result that is both
//! final and end of turn. A result that is final but not end of turn is never built, so the voice
//! manager's transcript timers can never fire on a segmented session.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use bytes::Bytes;
use futures::future::BoxFuture;
use parking_lot::{Mutex, RwLock};
use tokio::sync::{mpsc, oneshot};
use tokio::task::{AbortHandle, JoinHandle};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::audio::{AudioError, Frame, FrontEnd, frame_to_f32};
use crate::detector::{EnergyDetector, SpeechDetector};
use crate::endpointer::{
    EndOfTurnModel, EndOfTurnTextModel, LadderInput, LadderThresholds, Verdict, decide,
    wants_text_verdict,
};
use crate::limits::resolution_deadline_ms;
use crate::profile::{SegmentProfile, UploadPolicy};
use crate::segmenter::{CutOutcome, SegPhase, Segment, Segmenter, SegmenterEvent};
use crate::sequencer::{SegmentUpload, UnitUpload, join_segments};
use crate::transcriber::attempts::{UnitFailure, UploadResolution};
use crate::transcriber::quality::{QualityPolicy, QualityVerdict, SegmentEvidence, evaluate};
use crate::transcriber::{SegmentAudio, SegmentContext};
use crate::turn::{Part, TurnText};
use crate::types::{
    CutReason, DetectorFallback, DetectorKind, EngineResult, ErrorClass, FRAME_MS, FRAME_SAMPLES,
    FailureClass, FilteredBy, FlushOutcome, InterimMode, NoticeCallback, NoticeKind,
    SegmentAdmission, SegmentAdmissionHook, SegmentMeta, SegmentOutcome, SegmentOutcomeSink,
    SegmentResultKind, SegmentTimings, SpeechActivity, SpeechActivityCallback, SttLiveFacts,
    SttNotice, TurnCloseReason, ms_to_samples, samples_to_ms,
};

/// The result callback: awaited on the result emitter only.
pub type ResultCallback = Arc<dyn Fn(EngineResult) -> BoxFuture<'static, ()> + Send + Sync>;
/// Called once when transcription stops for good.
pub type FatalCallback = Arc<dyn Fn(EngineFatal) -> BoxFuture<'static, ()> + Send + Sync>;

/// Transcription stopped for good.
#[derive(Debug, Clone, PartialEq)]
pub struct EngineFatal {
    /// `credential_rejected`, `model_not_served`, `endpoint_rejected`, `requests_refused`,
    /// `detector_failed`.
    pub reason: &'static str,
    pub message: String,
}

impl EngineFatal {
    fn from_class(class: ErrorClass, message: String) -> Self {
        let reason = match class {
            ErrorClass::Auth => "credential_rejected",
            ErrorClass::ModelNotServed => "model_not_served",
            ErrorClass::EndpointRejected => "endpoint_rejected",
            ErrorClass::BadRequest => "requests_refused",
            _ => "transcription_stopped",
        };
        Self { reason, message }
    }
}

/// Slots read at the moment of use, so they can be set after the engine started.
#[derive(Default)]
pub struct Callbacks {
    pub result: RwLock<Option<ResultCallback>>,
    pub fatal: RwLock<Option<FatalCallback>>,
    pub activity: RwLock<Option<SpeechActivityCallback>>,
    pub notice: RwLock<Option<NoticeCallback>>,
    pub outcome: RwLock<Option<Arc<dyn SegmentOutcomeSink>>>,
    pub admission: RwLock<Option<SegmentAdmissionHook>>,
}

impl std::fmt::Debug for Callbacks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Callbacks").finish_non_exhaustive()
    }
}

/// A monotonic clock in milliseconds for event timestamps.
pub trait Clock: Send + Sync {
    fn now_ms(&self) -> u64;
}

/// Milliseconds since the clock was made, on tokio's clock (so tests can pause it).
#[derive(Debug)]
pub struct TokioClock {
    start: Instant,
}

impl Default for TokioClock {
    fn default() -> Self {
        Self {
            start: Instant::now(),
        }
    }
}

impl Clock for TokioClock {
    fn now_ms(&self) -> u64 {
        Instant::now()
            .saturating_duration_since(self.start)
            .as_millis() as u64
    }
}

/// What a session configures.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub profile: SegmentProfile,
    pub encoding: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub language: Option<String>,
    pub candidate_languages: Vec<String>,
    pub prompt: Option<String>,
    pub keywords: Vec<String>,
    pub quality: QualityPolicy,
    pub detector_fallback: Option<DetectorFallback>,
    /// After ten consecutive detector failures, switch to the energy detector instead of ending
    /// the session (`WAAV_STT_SEGMENT_ALLOW_ENERGY_DETECTOR=1`).
    pub allow_energy_fallback: bool,
    /// The row's billing rule, for each outcome's `billed_seconds`.
    pub billing: crate::types::BillingRule,
}

impl EngineConfig {
    pub fn new(profile: SegmentProfile) -> Self {
        Self {
            profile,
            encoding: "linear16".into(),
            sample_rate: 16_000,
            channels: 1,
            language: None,
            candidate_languages: Vec::new(),
            prompt: None,
            keywords: Vec::new(),
            quality: QualityPolicy::default(),
            detector_fallback: None,
            allow_energy_fallback: false,
            billing: crate::types::BillingRule::default(),
        }
    }
}

/// Everything the engine runs on.
pub struct EngineParts {
    pub detector: Box<dyn SpeechDetector>,
    pub upload: Arc<dyn SegmentUpload>,
    pub audio_model: Option<Arc<dyn EndOfTurnModel>>,
    pub text_model: Option<Arc<dyn EndOfTurnTextModel>>,
    pub clock: Arc<dyn Clock>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum EngineError {
    #[error("the segmented engine has stopped")]
    Closed,
}

enum Ingest {
    Audio { bytes: Bytes, at_ms: u64 },
    Flush(oneshot::Sender<FlushOutcome>),
    Language(Option<String>),
}

/// The engine's handle. `send_audio` never waits.
pub struct EngineHandle {
    tx: mpsc::UnboundedSender<Ingest>,
    queued_bytes: Arc<AtomicUsize>,
    budget: usize,
    cancel: CancellationToken,
    tasks: Mutex<Vec<JoinHandle<()>>>,
    alive: Arc<AtomicBool>,
    facts: SttLiveFacts,
    callbacks: Arc<Callbacks>,
    clock: Arc<dyn Clock>,
    dropped_since_notice: AtomicU64,
    last_drop_notice_ms: AtomicU64,
}

impl std::fmt::Debug for EngineHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineHandle")
            .field("alive", &self.alive.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl EngineHandle {
    /// Start the engine and its two emitters.
    pub fn spawn(
        cfg: EngineConfig,
        parts: EngineParts,
        callbacks: Arc<Callbacks>,
    ) -> Result<Self, AudioError> {
        let frontend = FrontEnd::new(&cfg.encoding, cfg.sample_rate, cfg.channels)?;
        let facts = SttLiveFacts {
            detector: parts.detector.kind(),
            detector_fallback: cfg.detector_fallback,
            resampled_from_hz: frontend.resampled_from(),
            encoding_assumed_pcm: frontend.assumed_pcm().map(str::to_string),
            end_of_turn_audio_model: parts.audio_model.is_some(),
            end_of_turn_text_model: parts.text_model.is_some(),
            silence_ceiling_ms: cfg.profile.max_endpointing_ms,
            interims: cfg.profile.interims,
            final_deadline_ms: parts.upload.deadline_ms(),
            resolution_deadline_ms: resolution_deadline_ms(
                cfg.profile.cut_silence_ms,
                parts.upload.deadline_ms(),
            ),
        };
        let (tx, rx) = mpsc::unbounded_channel();
        let (results_tx, mut results_rx) = mpsc::unbounded_channel::<EngineResult>();
        let (fatal_tx, mut fatal_rx) = mpsc::unbounded_channel::<EngineFatal>();
        let queued_bytes = Arc::new(AtomicUsize::new(0));
        let queued_interims = Arc::new(AtomicUsize::new(0));
        let cancel = CancellationToken::new();
        let alive = Arc::new(AtomicBool::new(true));
        let budget = cfg.profile.ingest_budget_bytes;

        let cb = Arc::clone(&callbacks);
        let qi = Arc::clone(&queued_interims);
        let result_emitter = tokio::spawn(async move {
            while let Some(r) = results_rx.recv().await {
                if !r.is_final {
                    qi.fetch_sub(1, Ordering::AcqRel);
                }
                let f = cb.result.read().clone();
                if let Some(f) = f {
                    f(r).await;
                }
            }
        });
        let cb = Arc::clone(&callbacks);
        let fatal_emitter = tokio::spawn(async move {
            if let Some(e) = fatal_rx.recv().await {
                let f = cb.fatal.read().clone();
                if let Some(f) = f {
                    f(e).await;
                }
            }
        });

        let clock = Arc::clone(&parts.clock);
        let engine = Engine::new(
            cfg,
            parts,
            frontend,
            Arc::clone(&callbacks),
            results_tx,
            queued_interims,
            fatal_tx,
            Arc::clone(&queued_bytes),
        );
        let task_cancel = cancel.clone();
        let task_alive = Arc::clone(&alive);
        let engine_task = tokio::spawn(async move {
            engine.run(rx, task_cancel).await;
            task_alive.store(false, Ordering::Release);
        });
        Ok(Self {
            tx,
            queued_bytes,
            budget,
            cancel,
            tasks: Mutex::new(vec![engine_task, result_emitter, fatal_emitter]),
            alive,
            facts,
            callbacks,
            clock,
            dropped_since_notice: AtomicU64::new(0),
            last_drop_notice_ms: AtomicU64::new(0),
        })
    }

    pub fn facts(&self) -> &SttLiveFacts {
        &self.facts
    }

    pub fn callbacks(&self) -> &Arc<Callbacks> {
        &self.callbacks
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }

    /// Queue one chunk. Over the byte budget the chunk is dropped and counted, and one notice is
    /// raised per second; that is still `Ok`, because an error here becomes a client error per frame.
    pub fn send_audio(&self, bytes: Bytes) -> Result<(), EngineError> {
        if !self.is_alive() {
            return Err(EngineError::Closed);
        }
        let charge = bytes.len().max(64);
        let queued = self.queued_bytes.load(Ordering::Acquire);
        if queued > 0 && queued + charge > self.budget {
            let dropped = self
                .dropped_since_notice
                .fetch_add(bytes.len() as u64, Ordering::AcqRel)
                + bytes.len() as u64;
            let now = self.clock.now_ms();
            let last = self.last_drop_notice_ms.load(Ordering::Acquire);
            if last == 0 || now.saturating_sub(last) >= 1000 {
                self.last_drop_notice_ms
                    .store(now.max(1), Ordering::Release);
                self.dropped_since_notice.store(0, Ordering::Release);
                if let Some(n) = self.callbacks.notice.read().clone() {
                    n(SttNotice {
                        kind: NoticeKind::AudioDropped { bytes: dropped },
                        turn_id: None,
                        seq: None,
                    });
                }
            }
            return Ok(());
        }
        self.queued_bytes.fetch_add(charge, Ordering::AcqRel);
        self.tx
            .send(Ingest::Audio {
                bytes,
                at_ms: self.clock.now_ms(),
            })
            .map_err(|_| EngineError::Closed)
    }

    /// A client commit, behind any audio already queued. The receiver resolves when the engine has
    /// reached it.
    pub fn flush(&self) -> oneshot::Receiver<FlushOutcome> {
        let (tx, rx) = oneshot::channel();
        let _ = self.tx.send(Ingest::Flush(tx));
        rx
    }

    /// Language for later segments.
    pub fn set_language(&self, language: Option<String>) {
        let _ = self.tx.send(Ingest::Language(language));
    }

    /// Stop now: no flush, no upload, no wait for a transcript. Uploads in flight are dropped.
    pub async fn stop(&self) {
        self.cancel.cancel();
        let tasks: Vec<JoinHandle<()>> = std::mem::take(&mut *self.tasks.lock());
        let mut iter = tasks.into_iter();
        if let Some(engine) = iter.next() {
            let _ = tokio::time::timeout(Duration::from_secs(5), engine).await;
        }
        for t in iter {
            t.abort();
            let _ = t.await;
        }
        self.alive.store(false, Ordering::Release);
    }
}

impl Drop for EngineHandle {
    fn drop(&mut self) {
        self.cancel.cancel();
        for t in self.tasks.lock().iter() {
            t.abort();
        }
    }
}

#[derive(Debug)]
enum UnitState {
    Held,
    InFlight { guard_at: Instant },
    Returned,
    Released,
}

struct Unit {
    seq: u32,
    turn_id: u64,
    index_in_turn: u16,
    segments: Vec<Segment>,
    state: UnitState,
    voiced_ms: u32,
    cut: CutReason,
    admission: SegmentAdmission,
    timings: SegmentTimings,
    turn_final: bool,
    /// Resolved without an upload (too short, not input, session fatal, noise).
    pre_resolved: Option<SegmentResultKind>,
    resolution: Option<UploadResolution>,
    abort: Option<AbortHandle>,
    last_speech_end_sample: u64,
    mean_probability: f32,
    short: bool,
}

impl Unit {
    fn audio_samples(&self) -> usize {
        let pcm: usize = self.segments.iter().map(|s| s.pcm.len()).sum();
        pcm + self
            .segments
            .last()
            .map_or(0, |s| s.trailing_zero_samples as usize)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TextVerdict {
    NotAsked,
    Pending,
    Done(bool),
}

struct Pause {
    unit: Option<u32>,
    verdict: Verdict,
}

struct Turn {
    id: u64,
    first_speech_sample: u64,
    last_speech_end_sample: u64,
    pauses: Vec<Pause>,
    endpoint: Option<TurnCloseReason>,
    decided_emitted: bool,
    sealed: Option<TurnCloseReason>,
    text: TurnText,
    text_verdict: TextVerdict,
    units: Vec<u32>,
    newest_cut_ms: u64,
    newest_cut_at: Instant,
    speech_end_ms: u64,
    interim_pending: bool,
    next_index: u16,
}

enum Done {
    Upload {
        seq: u32,
        resolution: UploadResolution,
    },
    /// A re-decode of the open segment answered (Release 6 interims).
    Redecoded {
        turn_id: u64,
        generation: u64,
        text: Option<String>,
    },
    AudioVerdict {
        turn_id: u64,
        pause: usize,
        verdict: Verdict,
    },
    TextVerdict {
        turn_id: u64,
        complete: Option<bool>,
    },
}

struct Engine {
    cfg: EngineConfig,
    p: SegmentProfile,
    thresholds: LadderThresholds,
    frontend: FrontEnd,
    detector: Box<dyn SpeechDetector>,
    detector_errors: u32,
    segmenter: Segmenter,
    upload: Arc<dyn SegmentUpload>,
    audio_model: Option<Arc<dyn EndOfTurnModel>>,
    text_model: Option<Arc<dyn EndOfTurnTextModel>>,
    clock: Arc<dyn Clock>,
    callbacks: Arc<Callbacks>,
    results_tx: mpsc::UnboundedSender<EngineResult>,
    queued_interims: Arc<AtomicUsize>,
    fatal_tx: mpsc::UnboundedSender<EngineFatal>,
    fatal_sent: bool,
    queued_bytes: Arc<AtomicUsize>,
    done_tx: mpsc::UnboundedSender<Done>,
    done_rx: Option<mpsc::UnboundedReceiver<Done>>,
    next_seq: u32,
    next_turn: u64,
    turns: VecDeque<Turn>,
    units: BTreeMap<u32, Unit>,
    next_release: u32,
    frame_arrivals: VecDeque<(u64, u64)>,
    last_audio_at: Instant,
    audio_since_mark: bool,
    idle_frames: u32,
    detector_kind: DetectorKind,
    language: Option<String>,
    /// The session language vote, while the session has no language (addendum B7).
    vote: Option<crate::transcriber::language_vote::LanguageVote>,
    last_run_start: Option<u64>,
    /// Re-decoding (Release 6): one request at a time; any cut makes an answer in flight stale.
    redecode_in_flight: bool,
    redecode_generation: u64,
    redecode_last_sample: u64,
    /// Where the open segment began: the last cut or split (0 before any).
    segment_from_sample: u64,
}

impl Engine {
    #[allow(clippy::too_many_arguments)]
    fn new(
        cfg: EngineConfig,
        parts: EngineParts,
        frontend: FrontEnd,
        callbacks: Arc<Callbacks>,
        results_tx: mpsc::UnboundedSender<EngineResult>,
        queued_interims: Arc<AtomicUsize>,
        fatal_tx: mpsc::UnboundedSender<EngineFatal>,
        queued_bytes: Arc<AtomicUsize>,
    ) -> Self {
        let (done_tx, done_rx) = mpsc::unbounded_channel();
        let mut p = cfg.profile.clone();
        if parts.detector.kind() == DetectorKind::Energy {
            // The energy detector confirms more slowly: a hum must not become a turn.
            p.start_confirm_ms = p.start_confirm_ms.max(300);
        }
        let detector_kind = parts.detector.kind();
        Self {
            thresholds: LadderThresholds::from_profile(&p),
            segmenter: Segmenter::new(&p),
            language: cfg.language.clone(),
            vote: cfg
                .language
                .is_none()
                .then(crate::transcriber::language_vote::LanguageVote::new),
            p,
            cfg,
            frontend,
            detector: parts.detector,
            detector_errors: 0,
            upload: parts.upload,
            audio_model: parts.audio_model,
            text_model: parts.text_model,
            clock: parts.clock,
            callbacks,
            results_tx,
            queued_interims,
            fatal_tx,
            fatal_sent: false,
            queued_bytes,
            done_tx,
            done_rx: Some(done_rx),
            next_seq: 1,
            next_turn: 1,
            turns: VecDeque::new(),
            units: BTreeMap::new(),
            next_release: 1,
            frame_arrivals: VecDeque::new(),
            last_audio_at: Instant::now(),
            audio_since_mark: false,
            idle_frames: 0,
            detector_kind,
            last_run_start: None,
            redecode_in_flight: false,
            redecode_generation: 0,
            redecode_last_sample: 0,
            segment_from_sample: 0,
        }
    }

    async fn run(mut self, mut rx: mpsc::UnboundedReceiver<Ingest>, cancel: CancellationToken) {
        let mut done_rx = self.done_rx.take().expect("done receiver");
        let upload = Arc::clone(&self.upload);
        tokio::spawn(async move { upload.prewarm(1).await });
        let mut tick = tokio::time::interval(Duration::from_millis(50));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => break,
                Some(d) = done_rx.recv() => self.on_done(d),
                item = rx.recv() => match item {
                    Some(Ingest::Audio { bytes, at_ms }) => {
                        self.queued_bytes.fetch_sub(bytes.len().max(64), Ordering::AcqRel);
                        self.on_audio(&bytes, at_ms).await;
                    }
                    Some(Ingest::Flush(reply)) => {
                        let outcome = self.on_flush();
                        self.drive();
                        let _ = reply.send(outcome);
                    }
                    Some(Ingest::Language(l)) => {
                        // A language the client chose is never overridden by the vote.
                        if l.is_some() {
                            self.vote = None;
                        }
                        self.language = l;
                    }
                    None => break,
                },
                _ = tick.tick() => self.on_tick(),
            }
            self.drive();
        }
        // A real stop: drop uploads in flight; their outcomes are cancelled.
        let in_flight: Vec<u32> = self
            .units
            .iter()
            .filter(|(_, u)| matches!(u.state, UnitState::InFlight { .. } | UnitState::Held))
            .map(|(s, _)| *s)
            .collect();
        for seq in in_flight {
            if let Some(u) = self.units.get_mut(&seq) {
                if let Some(a) = u.abort.take() {
                    a.abort();
                }
                u.pre_resolved = Some(SegmentResultKind::Failed(FailureClass::Cancelled));
                let outcome = self.outcome_of(
                    seq,
                    SegmentResultKind::Failed(FailureClass::Cancelled),
                    0,
                    None,
                    false,
                );
                if let Some(sink) = self.callbacks.outcome.read().clone() {
                    sink.record(&outcome);
                }
            }
        }
    }

    fn now_ms(&self) -> u64 {
        self.clock.now_ms()
    }

    fn activity(&self, a: SpeechActivity) {
        if let Some(cb) = self.callbacks.activity.read().clone() {
            cb(a);
        }
    }

    fn notice(&self, kind: NoticeKind, turn_id: Option<u64>, seq: Option<u32>) {
        if let Some(cb) = self.callbacks.notice.read().clone() {
            cb(SttNotice { kind, turn_id, seq });
        }
    }

    fn mono_at(&self, sample: u64) -> u64 {
        let frame = sample / FRAME_SAMPLES as u64;
        self.frame_arrivals
            .iter()
            .rev()
            .find(|(f, _)| *f <= frame)
            .map(|(_, at)| *at)
            .unwrap_or_else(|| self.now_ms())
    }

    async fn on_audio(&mut self, bytes: &[u8], at_ms: u64) {
        self.last_audio_at = Instant::now();
        self.audio_since_mark = true;
        let mut frames: Vec<Frame> = Vec::new();
        self.frontend.push(bytes, &mut frames);
        for (i, frame) in frames.iter().enumerate() {
            self.on_frame(frame, at_ms);
            if i % 32 == 31 {
                tokio::task::yield_now().await;
            }
        }
    }

    fn on_frame(&mut self, frame: &[i16], at_ms: u64) {
        let index = self.segmenter.now_sample() / FRAME_SAMPLES as u64;
        if self.frame_arrivals.len() >= 320 {
            self.frame_arrivals.pop_front();
        }
        self.frame_arrivals.push_back((index, at_ms));
        let prob = match self.detector.probability(&frame_to_f32(frame)) {
            Ok(p) => {
                self.detector_errors = 0;
                p
            }
            Err(e) => {
                self.detector_errors += 1;
                if self.detector_errors >= 10 {
                    self.on_detector_failure(e.0);
                }
                0.0
            }
        };
        let mut events = Vec::new();
        self.segmenter.push_frame(frame, prob, &mut events);
        if self.segmenter.phase() == SegPhase::Idle {
            self.idle_frames += 1;
            if self.idle_frames * FRAME_MS >= self.p.idle_reset_ms {
                self.detector.reset();
                self.idle_frames = 0;
            }
        } else {
            self.idle_frames = 0;
        }
        for e in events {
            self.on_segmenter_event(e);
        }
        self.maybe_redecode();
        // The longest turn.
        if let Some(t) = self.current_turn()
            && samples_to_ms(
                self.segmenter
                    .now_sample()
                    .saturating_sub(t.first_speech_sample),
            ) >= self.p.max_turn_ms as u64
        {
            let mut events = Vec::new();
            self.segmenter
                .flush(CutReason::MaxTurnDuration, &mut events);
            for e in events {
                self.on_segmenter_event(e);
            }
            if let Some(t) = self.current_turn_mut() {
                t.sealed = Some(TurnCloseReason::MaxTurnDuration);
            }
        }
    }

    fn on_detector_failure(&mut self, message: String) {
        if self.cfg.allow_energy_fallback && self.detector_kind != DetectorKind::Energy {
            self.detector = Box::new(EnergyDetector::new());
            self.detector_kind = DetectorKind::Energy;
            self.detector_errors = 0;
            self.notice(
                NoticeKind::DetectorSwitched {
                    to: DetectorKind::Energy,
                    reason: DetectorFallback::RuntimeErrors,
                },
                None,
                None,
            );
        } else {
            self.raise_fatal(EngineFatal {
                reason: "detector_failed",
                message,
            });
        }
    }

    fn raise_fatal(&mut self, f: EngineFatal) {
        if !self.fatal_sent {
            self.fatal_sent = true;
            let _ = self.fatal_tx.send(f);
        }
    }

    fn current_turn(&self) -> Option<&Turn> {
        self.turns.back().filter(|t| t.sealed.is_none())
    }

    fn current_turn_mut(&mut self) -> Option<&mut Turn> {
        self.turns.back_mut().filter(|t| t.sealed.is_none())
    }

    fn open_turn(&mut self, first_speech_sample: u64) -> u64 {
        let id = self.next_turn;
        self.next_turn += 1;
        self.turns.push_back(Turn {
            id,
            first_speech_sample,
            last_speech_end_sample: first_speech_sample,
            pauses: Vec::new(),
            endpoint: None,
            decided_emitted: false,
            sealed: None,
            text: TurnText::new(self.language.as_deref()),
            text_verdict: TextVerdict::NotAsked,
            units: Vec::new(),
            newest_cut_ms: self.now_ms(),
            newest_cut_at: Instant::now(),
            speech_end_ms: self.now_ms(),
            interim_pending: false,
            next_index: 0,
        });
        id
    }

    fn on_segmenter_event(&mut self, e: SegmenterEvent) {
        if matches!(e, SegmenterEvent::Split(_) | SegmenterEvent::Cut { .. }) {
            // The open segment's own upload now carries its text; a re-decode is stale.
            self.redecode_generation += 1;
            self.redecode_last_sample = self.segmenter.now_sample();
            self.segment_from_sample = self.segmenter.now_sample();
        }
        match e {
            SegmenterEvent::Started {
                first_speech_sample,
                sustained_ms,
            } => {
                let turn_id = match self.current_turn_mut() {
                    Some(t) => {
                        // Speech resumed: the decision is withdrawn.
                        t.endpoint = None;
                        t.decided_emitted = false;
                        t.text_verdict = match t.text_verdict {
                            TextVerdict::Done(_) => TextVerdict::NotAsked,
                            other => other,
                        };
                        t.id
                    }
                    None => self.open_turn(first_speech_sample),
                };
                if self.last_run_start != Some(first_speech_sample) {
                    self.last_run_start = Some(first_speech_sample);
                    let upload = Arc::clone(&self.upload);
                    let depth = self.p.max_in_flight;
                    tokio::spawn(async move { upload.prewarm(depth).await });
                }
                let at_mono_ms = self.mono_at(first_speech_sample);
                self.activity(SpeechActivity::Started {
                    turn_id,
                    at_sample: first_speech_sample,
                    sustained_ms,
                    at_mono_ms,
                });
            }
            SegmenterEvent::Split(segment) => {
                let Some(turn_id) = self.current_turn().map(|t| t.id) else {
                    return;
                };
                self.add_segment(turn_id, segment, None);
                let now = self.now_ms();
                if let Some(t) = self.current_turn_mut() {
                    t.newest_cut_ms = now;
                    t.newest_cut_at = Instant::now();
                }
            }
            SegmenterEvent::Cut {
                reason,
                first_speech_sample: _,
                last_speech_end_sample,
                voiced_ms,
                outcome,
            } => {
                let Some(turn_id) = self.current_turn().map(|t| t.id) else {
                    return;
                };
                let at_mono = self.mono_at(last_speech_end_sample.saturating_sub(1));
                let now = self.now_ms();
                let (unit, will_upload) = match outcome {
                    CutOutcome::Ready(segment) => {
                        let seq = self.add_segment(turn_id, segment, Some(reason));
                        let will = self
                            .units
                            .get(&seq)
                            .is_some_and(|u| u.pre_resolved.is_none());
                        (Some(seq), will)
                    }
                    CutOutcome::TooShort => {
                        let seq = self.pre_resolved_unit(
                            turn_id,
                            reason,
                            voiced_ms,
                            SegmentResultKind::Filtered(FilteredBy::TooShort),
                            last_speech_end_sample,
                        );
                        (Some(seq), false)
                    }
                    CutOutcome::Nothing => (
                        self.turns.back().and_then(|t| t.units.last().copied()),
                        false,
                    ),
                };
                if let Some(t) = self.turns.back_mut().filter(|t| t.id == turn_id) {
                    t.newest_cut_ms = now;
                    t.newest_cut_at = Instant::now();
                    t.speech_end_ms = at_mono;
                    t.last_speech_end_sample = t.last_speech_end_sample.max(last_speech_end_sample);
                    t.pauses.push(Pause {
                        unit,
                        verdict: Verdict::Pending,
                    });
                }
                self.activity(SpeechActivity::Stopped {
                    turn_id,
                    at_sample: last_speech_end_sample,
                    voiced_ms,
                    will_upload,
                    at_mono_ms: at_mono,
                });
                if !reason.is_split() && reason != CutReason::Commit {
                    self.ask_audio_verdict(turn_id);
                }
            }
            SegmenterEvent::OnsetRejected => {}
        }
    }

    fn ask_audio_verdict(&mut self, turn_id: u64) {
        let Some(t) = self.turns.back() else { return };
        let pause = t.pauses.len() - 1;
        let Some(model) = self.audio_model.clone() else {
            if let Some(t) = self.turns.back_mut() {
                t.pauses[pause].verdict = Verdict::Failed;
            }
            return;
        };
        if self.p.endpoint_policy != crate::profile::EndpointPolicy::Auto {
            return;
        }
        let from = t
            .first_speech_sample
            .saturating_sub(ms_to_samples(500))
            .max(
                self.segmenter
                    .now_sample()
                    .saturating_sub(ms_to_samples(8000)),
            );
        let audio: Vec<f32> = self
            .segmenter
            .tail(from)
            .iter()
            .map(|s| *s as f32 / 32768.0)
            .collect();
        let tx = self.done_tx.clone();
        let timeout = Duration::from_millis(self.p.verdict_timeout_ms as u64);
        tokio::spawn(async move {
            let verdict =
                match tokio::time::timeout(timeout, model.completion_probability(audio)).await {
                    Ok(Ok(p)) => Verdict::Probability(p),
                    _ => Verdict::Failed,
                };
            let _ = tx.send(Done::AudioVerdict {
                turn_id,
                pause,
                verdict,
            });
        });
    }

    fn add_segment(&mut self, turn_id: u64, segment: Segment, cut: Option<CutReason>) -> u32 {
        // A segment of the same turn joins a held unit when the result stays within the maximum.
        let max_samples = ms_to_samples(self.p.max_segment_ms as u64) as usize;
        let join_into = self
            .units
            .values()
            .rev()
            .find(|u| {
                u.turn_id == turn_id
                    && matches!(u.state, UnitState::Held)
                    && u.pre_resolved.is_none()
                    && u.audio_samples() + segment.pcm.len() <= max_samples
            })
            .map(|u| u.seq);
        let cut_reason = cut.unwrap_or(segment.cut);
        let meta = SegmentMeta {
            turn_id,
            seq: join_into.unwrap_or(self.next_seq),
            voiced_ms: segment.voiced_ms,
            first_speech_sample: segment.first_speech_sample,
            last_speech_end_sample: segment.last_speech_end_sample,
        };
        let admission = self
            .callbacks
            .admission
            .read()
            .clone()
            .map_or(SegmentAdmission::ADMIT, |h| h(&meta));
        let now = self.now_ms();
        if admission.is_input
            && let Some(seq) = join_into
            && let Some(u) = self.units.get_mut(&seq)
        {
            u.voiced_ms += segment.voiced_ms;
            u.last_speech_end_sample = segment.last_speech_end_sample;
            u.cut = cut_reason;
            u.timings.cut_ms = now;
            u.short = false;
            u.segments.push(segment);
            return seq;
        }
        let seq = self.next_seq;
        self.next_seq += 1;
        let index = self
            .turns
            .back_mut()
            .map(|t| {
                t.units.push(seq);
                t.next_index += 1;
                t.next_index - 1
            })
            .unwrap_or(0);
        let speech_end_ms = self.mono_at(segment.last_speech_end_sample.saturating_sub(1));
        let unit = Unit {
            seq,
            turn_id,
            index_in_turn: index,
            voiced_ms: segment.voiced_ms,
            cut: cut_reason,
            admission,
            timings: SegmentTimings {
                speech_end_ms,
                cut_ms: now,
                ..Default::default()
            },
            turn_final: false,
            pre_resolved: (!admission.is_input)
                .then_some(SegmentResultKind::Filtered(FilteredBy::NotInput)),
            resolution: None,
            abort: None,
            last_speech_end_sample: segment.last_speech_end_sample,
            mean_probability: segment.mean_probability,
            short: segment.short,
            state: if admission.is_input {
                UnitState::Held
            } else {
                UnitState::Returned
            },
            segments: vec![segment],
        };
        self.units.insert(seq, unit);
        seq
    }

    fn pre_resolved_unit(
        &mut self,
        turn_id: u64,
        cut: CutReason,
        voiced_ms: u32,
        kind: SegmentResultKind,
        last_end: u64,
    ) -> u32 {
        let seq = self.next_seq;
        self.next_seq += 1;
        let index = self
            .turns
            .back_mut()
            .map(|t| {
                t.units.push(seq);
                t.next_index += 1;
                t.next_index - 1
            })
            .unwrap_or(0);
        let now = self.now_ms();
        self.units.insert(
            seq,
            Unit {
                seq,
                turn_id,
                index_in_turn: index,
                segments: Vec::new(),
                state: UnitState::Returned,
                voiced_ms,
                cut,
                admission: SegmentAdmission::ADMIT,
                timings: SegmentTimings {
                    speech_end_ms: self.mono_at(last_end.saturating_sub(1)),
                    cut_ms: now,
                    ..Default::default()
                },
                turn_final: false,
                pre_resolved: Some(kind),
                resolution: None,
                abort: None,
                last_speech_end_sample: last_end,
                mean_probability: 0.0,
                short: true,
            },
        );
        seq
    }

    fn on_flush(&mut self) -> FlushOutcome {
        let mut frames = Vec::new();
        self.frontend.flush(&mut frames);
        let at = self.now_ms();
        for f in &frames {
            self.on_frame(f, at);
        }
        let before = self.next_seq;
        let mut events = Vec::new();
        self.segmenter.commit(
            self.upload.min_audio_ms().max(self.p.min_commit_audio_ms),
            &mut events,
        );
        for e in events {
            self.on_segmenter_event(e);
        }
        let will_upload = self
            .units
            .range(before..)
            .any(|(_, u)| u.pre_resolved.is_none());
        let sealed = if let Some(t) = self.current_turn_mut() {
            t.sealed = Some(TurnCloseReason::Commit);
            Some(t.id)
        } else if self.audio_since_mark {
            let id = self.open_turn(self.segmenter.now_sample());
            if let Some(t) = self.turns.back_mut() {
                t.sealed = Some(TurnCloseReason::Commit);
            }
            Some(id)
        } else {
            None
        };
        self.audio_since_mark = false;
        FlushOutcome {
            turn_id: sealed,
            will_upload,
            result_follows: sealed.is_some(),
        }
    }

    fn on_tick(&mut self) {
        // The input-stall rule: a client that stopped sending audio cuts the open segment.
        if let Some(stall) = self.p.input_stall_ms
            && Instant::now().saturating_duration_since(self.last_audio_at)
                >= Duration::from_millis(stall as u64)
            && matches!(
                self.segmenter.phase(),
                SegPhase::Speech | SegPhase::Hangover | SegPhase::Onset
            )
        {
            let mut frames = Vec::new();
            self.frontend.flush(&mut frames);
            let mut events = Vec::new();
            self.segmenter.flush(CutReason::InputStall, &mut events);
            for e in events {
                self.on_segmenter_event(e);
            }
        }
        // The guard: an upload call that overran its deadline by 250 ms is given up.
        let now = Instant::now();
        let overdue: Vec<u32> = self
            .units
            .iter()
            .filter(|(_, u)| matches!(u.state, UnitState::InFlight { guard_at } if now >= guard_at))
            .map(|(s, _)| *s)
            .collect();
        for seq in overdue {
            if let Some(u) = self.units.get_mut(&seq) {
                tracing::warn!(
                    seq,
                    "segment upload overran its deadline; released as timed out"
                );
                u.pre_resolved = Some(SegmentResultKind::TimedOut);
                u.state = UnitState::Returned;
                u.timings.response_ms = Some(self.clock.now_ms());
            }
        }
    }

    fn on_done(&mut self, d: Done) {
        match d {
            Done::Redecoded {
                turn_id,
                generation,
                text,
            } => {
                self.redecode_in_flight = false;
                let open = matches!(
                    self.segmenter.phase(),
                    SegPhase::Speech | SegPhase::Hangover
                );
                let Some(text) = text.filter(|t| !t.trim().is_empty()) else {
                    return;
                };
                if generation != self.redecode_generation || !open {
                    return;
                }
                let Some(t) = self.current_turn().filter(|t| t.id == turn_id) else {
                    return;
                };
                if self.queued_interims.load(Ordering::Acquire) >= 64 {
                    return;
                }
                let mut r = t.text.interim(turn_id);
                r.transcript = if r.transcript.trim().is_empty() {
                    text.trim().to_string()
                } else {
                    format!("{} {}", r.transcript.trim_end(), text.trim())
                };
                self.queued_interims.fetch_add(1, Ordering::AcqRel);
                let _ = self.results_tx.send(r);
            }
            Done::Upload { seq, resolution } => {
                let now = self.now_ms();
                if let Some(u) = self.units.get_mut(&seq)
                    && matches!(u.state, UnitState::InFlight { .. })
                {
                    u.timings.response_ms = Some(now);
                    u.timings.queue_ms = resolution.queue_wait.as_millis() as u64;
                    u.resolution = Some(resolution);
                    u.state = UnitState::Returned;
                    u.abort = None;
                }
            }
            Done::AudioVerdict {
                turn_id,
                pause,
                verdict,
            } => {
                if let Some(t) = self.turns.iter_mut().find(|t| t.id == turn_id)
                    && let Some(p) = t.pauses.get_mut(pause)
                {
                    p.verdict = verdict;
                }
            }
            Done::TextVerdict { turn_id, complete } => {
                if let Some(t) = self.turns.iter_mut().find(|t| t.id == turn_id)
                    && t.text_verdict == TextVerdict::Pending
                {
                    t.text_verdict = TextVerdict::Done(complete.unwrap_or(false));
                }
            }
        }
    }

    fn drive(&mut self) {
        for _ in 0..6 {
            let changed = self.start_units()
                | self.release_units()
                | self.evaluate_endpoints()
                | self.close_turns();
            if !changed {
                break;
            }
        }
        self.emit_interims();
    }

    fn turn_of(&self, turn_id: u64) -> Option<&Turn> {
        self.turns.iter().find(|t| t.id == turn_id)
    }

    fn start_units(&mut self) -> bool {
        let mut changed = false;
        let held: Vec<u32> = self
            .units
            .iter()
            .filter(|(_, u)| matches!(u.state, UnitState::Held))
            .map(|(s, _)| *s)
            .collect();
        for seq in held {
            let Some(u) = self.units.get(&seq) else {
                continue;
            };
            let Some(turn) = self.turn_of(u.turn_id) else {
                continue;
            };
            let decided = turn.endpoint.is_some() || turn.sealed.is_some();
            let full = u.audio_samples() >= ms_to_samples(self.p.max_segment_ms as u64) as usize;
            let in_flight_speculative = self
                .units
                .values()
                .filter(|x| matches!(x.state, UnitState::InFlight { .. }) && !x.turn_final)
                .count();
            let speculative_ok = self.p.upload_policy == UploadPolicy::PerPause
                && in_flight_speculative < self.p.max_in_flight
                && self.upload.try_speculative();
            if decided || full {
                self.start_unit(seq, true);
                changed = true;
            } else if speculative_ok {
                self.start_unit(seq, false);
                changed = true;
            }
        }
        changed
    }

    fn start_unit(&mut self, seq: u32, turn_final: bool) {
        let gap_cap = self.p.join_gap_cap_ms;
        let deadline = Duration::from_millis(self.upload.deadline_ms() as u64);
        let guard = Duration::from_millis(self.p.turn_guard_ms as u64);
        let ctx = SegmentContext {
            turn_id: 0,
            seq,
            language: self.language.clone(),
            candidate_languages: self.cfg.candidate_languages.clone(),
            prompt: self.cfg.prompt.clone(),
            keywords: self.cfg.keywords.clone(),
            omit_fields: Vec::new(),
            minimal: false,
        };
        let Some(u) = self.units.get(&seq) else {
            return;
        };
        let Some(turn) = self.turn_of(u.turn_id) else {
            return;
        };
        let deadline_at = turn.newest_cut_at + deadline;
        let now_ms = self.now_ms();
        let audio = SegmentAudio::new(join_segments(&u.segments, gap_cap));
        let ctx = SegmentContext {
            turn_id: u.turn_id,
            ..ctx
        };
        let upload = Arc::clone(&self.upload);
        let tx = self.done_tx.clone();
        let req = UnitUpload {
            ctx,
            handed_over: Instant::now(),
            deadline_at,
            turn_final,
        };
        let task = tokio::spawn(async move {
            let resolution = upload.run(audio, req).await;
            let _ = tx.send(Done::Upload { seq, resolution });
        });
        if let Some(u) = self.units.get_mut(&seq) {
            u.timings.held_ms = now_ms.saturating_sub(u.timings.cut_ms);
            u.timings.handed_over_ms = Some(now_ms);
            u.turn_final = turn_final;
            u.state = UnitState::InFlight {
                guard_at: deadline_at + guard,
            };
            u.abort = Some(task.abort_handle());
        }
    }

    fn outcome_of(
        &self,
        seq: u32,
        kind: SegmentResultKind,
        offset: u32,
        transcript_request_id: Option<String>,
        suspect: bool,
    ) -> SegmentOutcome {
        let u = &self.units[&seq];
        let ledger = u.resolution.as_ref().map(|r| r.ledger).unwrap_or_default();
        let served_by = u.resolution.as_ref().and_then(|r| r.served_by.clone());
        let billing = served_by.as_ref().map_or(self.cfg.billing, |s| s.billing);
        let audio_ms = samples_to_ms(u.audio_samples() as u64) as u32;
        let uploaded_seconds = ledger.uploaded_ms as f64 / 1000.0;
        SegmentOutcome {
            turn_id: u.turn_id,
            seq,
            index_in_turn: u.index_in_turn,
            speech_segments: u.segments.len().max(1) as u8,
            turn_final: u.turn_final,
            voiced_ms: u.voiced_ms,
            audio_ms,
            joined_text_offset: offset,
            uploaded_seconds,
            billed_seconds: billing.billed_ms(ledger.requests, ledger.uploaded_ms) as f64 / 1000.0,
            timings: SegmentTimings {
                released_ms: self.now_ms(),
                ..u.timings
            },
            kind,
            retries: ledger.requests.saturating_sub(1),
            overlapped_agent_speech: u.admission.overlapped_agent_speech,
            suspect,
            cut: u.cut,
            short: u.short,
            detector: self.detector_kind,
            vendor_request_id: transcript_request_id,
            served_by: served_by.map(|s| s.name),
        }
    }

    fn release_units(&mut self) -> bool {
        let mut changed = false;
        while let Some(u) = self.units.get(&self.next_release) {
            if !matches!(u.state, UnitState::Returned) {
                break;
            }
            let seq = self.next_release;
            self.next_release += 1;
            changed = true;
            self.release_one(seq);
        }
        changed
    }

    fn release_one(&mut self, seq: u32) {
        let (turn_id, kind, part, request_id, suspect) = {
            let u = &self.units[&seq];
            let turn_id = u.turn_id;
            if let Some(k) = u.pre_resolved.clone() {
                let part = matches!(
                    k,
                    SegmentResultKind::TimedOut | SegmentResultKind::Failed(_)
                )
                .then(|| Part::Gap {
                    seq,
                    voiced_ms: u.voiced_ms,
                });
                (turn_id, k, part, None, false)
            } else {
                let r = u
                    .resolution
                    .as_ref()
                    .expect("returned unit has a resolution");
                match &r.result {
                    Ok(t) => {
                        let ev = SegmentEvidence {
                            voiced_ms: u.voiced_ms,
                            mean_speech_probability: Some(u.mean_probability),
                            overlapped_agent_speech: u.admission.overlapped_agent_speech,
                            agent_spoke_since_previous_segment: u
                                .admission
                                .agent_spoke_since_previous_segment,
                        };
                        let verdict = evaluate(t, &ev, &self.cfg.quality);
                        if matches!(verdict, QualityVerdict::Keep { .. })
                            && let Some(vote) = self.vote.as_mut()
                            && let Some(lang) = vote.record(t.detected_language.as_deref())
                        {
                            tracing::info!(language = %lang, "segmented speech-to-text: session language agreed and pinned");
                            self.language = Some(lang);
                            self.vote = None;
                        }
                        match verdict {
                            QualityVerdict::Keep { text, suspect, .. } => (
                                turn_id,
                                SegmentResultKind::Text,
                                Some(Part::Text {
                                    seq,
                                    text,
                                    cut: u.cut,
                                    vendor_confidence: t.vendor_confidence,
                                    derived_confidence: t.derived_confidence,
                                    language: t.detected_language.clone(),
                                    request_id: t.vendor_request_id.clone(),
                                }),
                                t.vendor_request_id.clone(),
                                suspect,
                            ),
                            QualityVerdict::Empty => (
                                turn_id,
                                SegmentResultKind::Empty,
                                None,
                                t.vendor_request_id.clone(),
                                false,
                            ),
                            QualityVerdict::Filtered(r) => (
                                turn_id,
                                SegmentResultKind::Filtered(FilteredBy::Quality(
                                    r.as_str().to_string(),
                                )),
                                None,
                                t.vendor_request_id.clone(),
                                false,
                            ),
                        }
                    }
                    Err(f) => {
                        let kind = match f {
                            UnitFailure::TimedOut => SegmentResultKind::TimedOut,
                            UnitFailure::BreakerOpen => {
                                SegmentResultKind::Failed(FailureClass::BreakerOpen)
                            }
                            UnitFailure::LimiterRefused => {
                                SegmentResultKind::Failed(FailureClass::LimiterRefused)
                            }
                            UnitFailure::SessionFatal(_) => {
                                SegmentResultKind::Failed(FailureClass::SessionFatal)
                            }
                            UnitFailure::Local(_) => SegmentResultKind::Failed(
                                FailureClass::Vendor(ErrorClass::BadRequest),
                            ),
                            UnitFailure::Vendor(e) => {
                                SegmentResultKind::Failed(FailureClass::Vendor(e.class))
                            }
                        };
                        (
                            turn_id,
                            kind,
                            Some(Part::Gap {
                                seq,
                                voiced_ms: u.voiced_ms,
                            }),
                            None,
                            false,
                        )
                    }
                }
            }
        };
        let offset = self.turn_of(turn_id).map_or(0, |t| t.text.offset());
        let outcome = self.outcome_of(seq, kind.clone(), offset, request_id, suspect);
        if let Some(sink) = self.callbacks.outcome.read().clone() {
            sink.record(&outcome);
        }
        // Warnings and a fatal resolution.
        let (fatal, warnings, message) = {
            let u = &self.units[&seq];
            match &u.resolution {
                Some(r) => (
                    r.fatal,
                    r.warnings.clone(),
                    match &r.result {
                        Err(UnitFailure::Vendor(e)) => e.message.clone(),
                        _ => String::new(),
                    },
                ),
                None => (None, Vec::new(), String::new()),
            }
        };
        for (code, msg) in warnings {
            self.notice(
                NoticeKind::Transcriber { code, message: msg },
                Some(turn_id),
                Some(seq),
            );
        }
        if let Some(class) = fatal {
            self.raise_fatal(EngineFatal::from_class(class, message));
        }
        if kind == SegmentResultKind::Text {
            let end = self.units[&seq].timings.speech_end_ms;
            self.upload
                .record_end_to_final(self.now_ms().saturating_sub(end) as u32);
        }
        // Noise escalation: a split segment that came back with no text.
        let cut = self.units[&seq].cut;
        if cut.is_split()
            && matches!(
                kind,
                SegmentResultKind::Empty | SegmentResultKind::Filtered(_)
            )
            && matches!(
                self.segmenter.phase(),
                SegPhase::Speech | SegPhase::Hangover
            )
        {
            let dropped = self.segmenter.abandon_open_segment();
            let to = self
                .segmenter
                .raise_thresholds(self.p.noise_step, self.p.noise_ceiling);
            tracing::info!(to, dropped, "noise suspected: segment thresholds raised");
            self.notice(
                NoticeKind::NoiseThresholdRaised { to },
                Some(turn_id),
                Some(seq),
            );
            let noise = self.pre_resolved_unit(
                turn_id,
                CutReason::SoftSplit,
                dropped,
                SegmentResultKind::Filtered(FilteredBy::NoiseSuspected),
                self.segmenter.now_sample(),
            );
            let _ = noise;
        }
        if let Some(t) = self.turns.iter_mut().find(|t| t.id == turn_id) {
            if let Some(p) = part {
                let is_text = matches!(p, Part::Text { .. });
                t.text.push(p);
                if is_text {
                    t.interim_pending = true;
                }
            }
            // A pause whose unit produced no text no longer decides.
            if t.text_verdict == TextVerdict::Done(false) && kind == SegmentResultKind::Text {
                t.text_verdict = TextVerdict::NotAsked;
            }
        }
        if let Some(u) = self.units.get_mut(&seq) {
            u.state = UnitState::Released;
        }
    }

    fn unit_has_text(&self, seq: u32) -> Option<bool> {
        let u = self.units.get(&seq)?;
        if !matches!(u.state, UnitState::Released) {
            return None;
        }
        let t = self.turn_of(u.turn_id)?;
        Some(
            t.text
                .parts
                .iter()
                .any(|p| matches!(p, Part::Text { seq: s, .. } if *s == seq)),
        )
    }

    fn turn_all_released(&self, t: &Turn) -> bool {
        t.units.iter().all(|s| {
            self.units
                .get(s)
                .is_some_and(|u| matches!(u.state, UnitState::Released))
        })
    }

    /// The deciding pause's verdict: the latest pause that follows a unit with text; while the
    /// latest pause's unit is out, that pause's own verdict.
    fn deciding_verdict(&self, t: &Turn) -> Option<Verdict> {
        let last = t.pauses.last()?;
        if let Some(seq) = last.unit
            && self.unit_has_text(seq).is_none()
        {
            return Some(last.verdict);
        }
        t.pauses
            .iter()
            .rev()
            .find(|p| p.unit.is_some_and(|s| self.unit_has_text(s) == Some(true)))
            .map(|p| p.verdict)
    }

    fn effective_silence(&self) -> (u32, bool) {
        let sample = samples_to_ms(self.segmenter.silence_samples()) as u32;
        let idle = Instant::now().saturating_duration_since(self.last_audio_at);
        match self.p.input_stall_ms {
            Some(stall) if idle >= Duration::from_millis(stall as u64) => {
                (sample + idle.as_millis() as u32, true)
            }
            _ => (sample, false),
        }
    }

    fn evaluate_endpoints(&mut self) -> bool {
        let mut changed = false;
        let n = self.turns.len();
        for i in 0..n {
            let t = &self.turns[i];
            if let Some(s) = t.sealed {
                if t.endpoint != Some(s) {
                    let id = t.id;
                    self.turns[i].endpoint = Some(s);
                    if !self.turns[i].decided_emitted {
                        self.turns[i].decided_emitted = true;
                        let at = self.now_ms();
                        self.activity(SpeechActivity::EndpointDecided {
                            turn_id: id,
                            reason: s,
                            at_mono_ms: at,
                        });
                    }
                    changed = true;
                }
                continue;
            }
            // The current turn: only while the caller is quiet.
            if i != n - 1 || self.segmenter.phase() != SegPhase::Idle || t.pauses.is_empty() {
                continue;
            }
            let all_released = self.turn_all_released(t);
            let text_complete = match t.text_verdict {
                TextVerdict::Done(c) => Some(c),
                _ => None,
            };
            let (silence, idle) = self.effective_silence();
            let input = LadderInput {
                policy: self.p.endpoint_policy,
                audio_model: self.audio_model.is_some(),
                verdict: self.deciding_verdict(t),
                all_released: all_released && t.text.has_text(),
                text_complete,
                effective_silence_ms: silence,
                input_idle: idle,
            };
            if t.text_verdict == TextVerdict::NotAsked
                && wants_text_verdict(&input, &self.thresholds, self.text_model.is_some())
                && let Some(model) = self.text_model.clone()
            {
                let text = t.text.joined();
                let turn_id = t.id;
                let tx = self.done_tx.clone();
                let timeout = Duration::from_millis(self.p.text_verdict_timeout_ms as u64);
                self.turns[i].text_verdict = TextVerdict::Pending;
                tokio::spawn(async move {
                    let complete =
                        match tokio::time::timeout(timeout, model.is_complete(&text)).await {
                            Ok(Ok(c)) => Some(c),
                            _ => None,
                        };
                    let _ = tx.send(Done::TextVerdict { turn_id, complete });
                });
                changed = true;
            }
            let decision = decide(&input, &self.thresholds);
            let t = &mut self.turns[i];
            if decision != t.endpoint {
                t.endpoint = decision;
                changed = true;
                if let Some(reason) = decision
                    && !t.decided_emitted
                {
                    t.decided_emitted = true;
                    let id = t.id;
                    let at = self.clock.now_ms();
                    self.activity(SpeechActivity::EndpointDecided {
                        turn_id: id,
                        reason,
                        at_mono_ms: at,
                    });
                }
            }
        }
        changed
    }

    fn close_turns(&mut self) -> bool {
        let mut changed = false;
        while let Some(t) = self.turns.front() {
            let is_current = self.turns.len() == 1 && t.sealed.is_none();
            let all_released = self.turn_all_released(t);
            let has_units = !t.units.is_empty();
            let idle = self.segmenter.phase() == SegPhase::Idle;
            let (silence, _) = self.effective_silence();
            let ordinary = t.endpoint.is_some()
                && all_released
                && (t.sealed.is_some()
                    || (idle && silence >= self.thresholds.min_endpoint_silence_ms)
                    || !is_current);
            let no_text = t.sealed.is_none()
                && all_released
                && has_units
                && !t.text.has_text()
                && (idle || !is_current);
            if !(ordinary || no_text) {
                break;
            }
            let t = self.turns.pop_front().expect("front");
            self.close_turn(t);
            changed = true;
        }
        changed
    }

    fn close_turn(&mut self, t: Turn) {
        let had_text = t.text.has_text();
        let result_follows = had_text || t.sealed == Some(TurnCloseReason::Commit);
        let (gaps, lost) = t.text.gaps();
        let reason = if had_text || t.sealed.is_some() {
            t.endpoint.unwrap_or(TurnCloseReason::NoText)
        } else {
            TurnCloseReason::NoText
        };
        let closed = SpeechActivity::TurnClosed {
            turn_id: t.id,
            had_text,
            result_follows,
            segments: t.units.len() as u16,
            gaps,
            lost_voiced_ms: lost,
            reason,
            speech_end_mono_ms: t.speech_end_ms,
        };
        if let Some(sink) = self.callbacks.outcome.read().clone() {
            sink.turn_closing(t.id, &closed);
        }
        self.activity(closed);
        if result_follows {
            let duration = (t.last_speech_end_sample > t.first_speech_sample)
                .then(|| (t.last_speech_end_sample - t.first_speech_sample) as f64 / 16_000.0);
            let _ = self.results_tx.send(t.text.final_result(t.id, duration));
            self.audio_since_mark = false;
        }
        for seq in &t.units {
            self.units.remove(seq);
        }
    }

    /// Start a re-decode of the open segment once it has grown by the interval (Release 6).
    fn maybe_redecode(&mut self) {
        let Some(interval) = self.p.redecode_interval_ms else {
            return;
        };
        if self.redecode_in_flight
            || self.p.interims == InterimMode::Off
            || !matches!(
                self.segmenter.phase(),
                SegPhase::Speech | SegPhase::Hangover
            )
        {
            return;
        }
        let Some(run_start) = self.last_run_start else {
            return;
        };
        let Some(turn_id) = self.current_turn().map(|t| t.id) else {
            return;
        };
        let now = self.segmenter.now_sample();
        let since = now.saturating_sub(run_start.max(self.redecode_last_sample));
        if samples_to_ms(since) < interval as u64 {
            return;
        }
        let from = run_start.saturating_sub(ms_to_samples(self.p.pre_roll_ms as u64));
        let pcm = self.segmenter.tail(from.max(self.segment_from_sample));
        if pcm.is_empty() {
            return;
        }
        self.redecode_in_flight = true;
        self.redecode_last_sample = now;
        let ctx = SegmentContext {
            turn_id,
            seq: 0,
            language: self.language.clone(),
            candidate_languages: self.cfg.candidate_languages.clone(),
            prompt: self.cfg.prompt.clone(),
            keywords: self.cfg.keywords.clone(),
            omit_fields: Vec::new(),
            minimal: false,
        };
        let upload = Arc::clone(&self.upload);
        let tx = self.done_tx.clone();
        let generation = self.redecode_generation;
        tokio::spawn(async move {
            let text = upload.redecode(SegmentAudio::new(pcm), ctx).await;
            let _ = tx.send(Done::Redecoded {
                turn_id,
                generation,
                text,
            });
        });
    }

    fn emit_interims(&mut self) {
        if self.p.interims == InterimMode::Off {
            return;
        }
        let Some(front) = self.turns.front_mut() else {
            return;
        };
        if !front.interim_pending {
            return;
        }
        front.interim_pending = false;
        if self.queued_interims.load(Ordering::Acquire) >= 64 {
            return;
        }
        let r = front.text.interim(front.id);
        self.queued_interims.fetch_add(1, Ordering::AcqRel);
        let _ = self.results_tx.send(r);
    }
}

#[cfg(test)]
mod tests;
