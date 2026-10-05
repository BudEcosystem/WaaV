//! Scenario tests: a level detector, a scripted vendor that honours deadlines, real-time audio on
//! a paused clock.

use std::sync::atomic::AtomicUsize;

use super::*;
use crate::detector::DetectorError;
use crate::endpointer::EndOfTurnError;
use crate::transcriber::SegmentTranscript;
use crate::transcriber::attempts::Ledger;

/// Probability from loudness: tests write the speech they want into the audio itself.
struct LevelDetector;

impl SpeechDetector for LevelDetector {
    fn probability(&mut self, frame: &[f32]) -> Result<f32, DetectorError> {
        let rms = (frame.iter().map(|s| s * s).sum::<f32>() / frame.len() as f32).sqrt();
        Ok(if rms > 0.02 { 0.9 } else { 0.02 })
    }
    fn reset(&mut self) {}
    fn kind(&self) -> DetectorKind {
        DetectorKind::Scripted
    }
}

type Responder = Arc<
    dyn Fn(usize, &SegmentAudio) -> (Duration, Result<SegmentTranscript, UnitFailure>)
        + Send
        + Sync,
>;

#[derive(Debug, Clone)]
struct Call {
    at_ms: u64,
    deadline_in_ms: u64,
    audio_ms: u32,
    turn_final: bool,
    language: Option<String>,
}

struct ScriptedUpload {
    responder: Responder,
    calls: Mutex<Vec<Call>>,
    speculative: AtomicBool,
    in_flight: AtomicUsize,
    max_in_flight: AtomicUsize,
    clock: Arc<TokioClock>,
}

impl ScriptedUpload {
    fn new(clock: Arc<TokioClock>, responder: Responder) -> Arc<Self> {
        Arc::new(Self {
            responder,
            calls: Mutex::new(Vec::new()),
            speculative: AtomicBool::new(true),
            in_flight: AtomicUsize::new(0),
            max_in_flight: AtomicUsize::new(0),
            clock,
        })
    }
}

fn text(t: &str) -> Result<SegmentTranscript, UnitFailure> {
    Ok(SegmentTranscript {
        text: t.into(),
        ..Default::default()
    })
}

fn ms(v: u64) -> Duration {
    Duration::from_millis(v)
}

#[async_trait::async_trait]
impl SegmentUpload for ScriptedUpload {
    async fn run(&self, audio: SegmentAudio, req: UnitUpload) -> UploadResolution {
        let n = {
            let mut calls = self.calls.lock();
            calls.push(Call {
                at_ms: self.clock.now_ms(),
                deadline_in_ms: req
                    .deadline_at
                    .saturating_duration_since(Instant::now())
                    .as_millis() as u64,
                audio_ms: audio.audio_ms(),
                turn_final: req.turn_final,
                language: req.ctx.language.clone(),
            });
            calls.len() - 1
        };
        let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_in_flight.fetch_max(now, Ordering::SeqCst);
        struct Leave<'a>(&'a AtomicUsize);
        impl Drop for Leave<'_> {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::SeqCst);
            }
        }
        let _leave = Leave(&self.in_flight);
        let (delay, result) = (self.responder)(n, &audio);
        let answer_at = Instant::now() + delay;
        let result = if answer_at >= req.deadline_at {
            tokio::time::sleep_until(req.deadline_at).await;
            Err(UnitFailure::TimedOut)
        } else {
            tokio::time::sleep_until(answer_at).await;
            result
        };
        let fatal = match &result {
            Err(UnitFailure::Vendor(e)) if e.class == ErrorClass::Auth => Some(ErrorClass::Auth),
            _ => None,
        };
        UploadResolution {
            result,
            ledger: Ledger {
                requests: 1,
                uploaded_ms: audio.audio_ms(),
            },
            queue_wait: Duration::ZERO,
            round_trip: Some(delay),
            fatal,
            warnings: Vec::new(),
        }
    }
    fn deadline_ms(&self) -> u32 {
        6000
    }
    fn try_speculative(&self) -> bool {
        self.speculative.load(Ordering::SeqCst)
    }
    async fn prewarm(&self, _connections: usize) {}
    fn min_audio_ms(&self) -> u32 {
        0
    }
}

struct FixedAudioModel(Mutex<VecDeque<f32>>, f32);

#[async_trait::async_trait]
impl EndOfTurnModel for FixedAudioModel {
    async fn completion_probability(&self, _audio: Vec<f32>) -> Result<f32, EndOfTurnError> {
        Ok(self.0.lock().pop_front().unwrap_or(self.1))
    }
}

fn audio_model(verdicts: &[f32], after: f32) -> Option<Arc<dyn EndOfTurnModel>> {
    Some(Arc::new(FixedAudioModel(
        Mutex::new(verdicts.iter().copied().collect()),
        after,
    )))
}

struct FixedTextModel(bool);

#[async_trait::async_trait]
impl EndOfTurnTextModel for FixedTextModel {
    async fn is_complete(&self, _t: &str) -> Result<bool, EndOfTurnError> {
        Ok(self.0)
    }
}

#[derive(Default)]
struct Sink {
    outcomes: Mutex<Vec<SegmentOutcome>>,
}

impl SegmentOutcomeSink for Sink {
    fn record(&self, o: &SegmentOutcome) {
        self.outcomes.lock().push(o.clone());
    }
}

struct Harness {
    handle: EngineHandle,
    clock: Arc<TokioClock>,
    upload: Arc<ScriptedUpload>,
    results: Arc<Mutex<Vec<(u64, EngineResult)>>>,
    activities: Arc<Mutex<Vec<(u64, SpeechActivity)>>>,
    notices: Arc<Mutex<Vec<SttNotice>>>,
    fatals: Arc<Mutex<Vec<EngineFatal>>>,
    sink: Arc<Sink>,
}

const CHUNK_MS: u64 = 20;

impl Harness {
    fn new(
        profile: SegmentProfile,
        responder: Responder,
        audio: Option<Arc<dyn EndOfTurnModel>>,
        text: Option<Arc<dyn EndOfTurnTextModel>>,
    ) -> Self {
        Self::with_config(EngineConfig::new(profile), responder, audio, text)
    }

    fn with_config(
        cfg: EngineConfig,
        responder: Responder,
        audio: Option<Arc<dyn EndOfTurnModel>>,
        text: Option<Arc<dyn EndOfTurnTextModel>>,
    ) -> Self {
        let clock = Arc::new(TokioClock::default());
        let upload = ScriptedUpload::new(Arc::clone(&clock), responder);
        let callbacks = Arc::new(Callbacks::default());
        let results = Arc::new(Mutex::new(Vec::new()));
        let activities = Arc::new(Mutex::new(Vec::new()));
        let notices = Arc::new(Mutex::new(Vec::new()));
        let fatals = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::new(Sink::default());
        {
            let (r, c) = (Arc::clone(&results), Arc::clone(&clock));
            *callbacks.result.write() = Some(Arc::new(move |res| {
                r.lock().push((c.now_ms(), res));
                Box::pin(async {})
            }));
            let (a, c) = (Arc::clone(&activities), Arc::clone(&clock));
            *callbacks.activity.write() =
                Some(Arc::new(move |act| a.lock().push((c.now_ms(), act))));
            let n = Arc::clone(&notices);
            *callbacks.notice.write() = Some(Arc::new(move |x| n.lock().push(x)));
            let f = Arc::clone(&fatals);
            *callbacks.fatal.write() = Some(Arc::new(move |x| {
                f.lock().push(x);
                Box::pin(async {})
            }));
            *callbacks.outcome.write() = Some(Arc::clone(&sink) as Arc<dyn SegmentOutcomeSink>);
        }
        let handle = EngineHandle::spawn(
            cfg,
            EngineParts {
                detector: Box::new(LevelDetector),
                upload: Arc::clone(&upload) as Arc<dyn SegmentUpload>,
                audio_model: audio,
                text_model: text,
                clock: Arc::clone(&clock) as Arc<dyn Clock>,
            },
            callbacks,
        )
        .expect("engine starts");
        Self {
            handle,
            clock,
            upload,
            results,
            activities,
            notices,
            fatals,
            sink,
        }
    }

    async fn feed(&self, duration_ms: u64, loud: bool) {
        let samples = (16 * CHUNK_MS) as usize;
        for _ in 0..duration_ms / CHUNK_MS {
            let level: i16 = if loud { 4000 } else { 0 };
            let bytes: Vec<u8> = (0..samples)
                .flat_map(|i| (if i % 2 == 0 { level } else { -level }).to_le_bytes())
                .collect();
            self.handle.send_audio(Bytes::from(bytes)).unwrap();
            tokio::time::sleep(ms(CHUNK_MS)).await;
        }
    }

    async fn speech(&self, ms: u64) {
        self.feed(ms, true).await
    }

    async fn silence(&self, ms: u64) {
        self.feed(ms, false).await
    }

    fn results(&self) -> Vec<(u64, EngineResult)> {
        self.results.lock().clone()
    }

    fn finals(&self) -> Vec<(u64, EngineResult)> {
        self.results()
            .into_iter()
            .filter(|(_, r)| r.is_final)
            .collect()
    }

    fn activities(&self) -> Vec<(u64, SpeechActivity)> {
        self.activities.lock().clone()
    }

    fn closed(&self) -> Vec<(u64, SpeechActivity)> {
        self.activities()
            .into_iter()
            .filter(|(_, a)| matches!(a, SpeechActivity::TurnClosed { .. }))
            .collect()
    }

    fn assert_two_shapes_only(&self) {
        for (_, r) in self.results() {
            assert!(
                !r.is_final || r.is_speech_final,
                "final without end of turn: {r:?}"
            );
            assert_eq!(
                r.is_final, r.is_speech_final,
                "an interim is never end of turn"
            );
        }
    }
}

fn constant(delay: u64, t: &'static str) -> Responder {
    Arc::new(move |_, _| (ms(delay), text(t)))
}

fn by_call(steps: Vec<(u64, &'static str)>) -> Responder {
    Arc::new(move |n, _| {
        let (d, t) = steps.get(n).copied().unwrap_or((10, ""));
        (ms(d), text(t))
    })
}

#[tokio::test(start_paused = true)]
async fn one_utterance_yields_one_final_that_is_also_end_of_turn() {
    let h = Harness::new(
        SegmentProfile::for_tests(),
        constant(800, "I'd like to change my booking."),
        None,
        None,
    );
    h.silence(500).await;
    h.speech(3000).await;
    h.silence(2500).await;
    let finals = h.finals();
    assert_eq!(finals.len(), 1);
    assert_eq!(finals[0].1.transcript, "I'd like to change my booking.");
    assert!(
        h.results().iter().all(|(_, r)| r.is_final),
        "one segment: no interim"
    );
    h.assert_two_shapes_only();
    let calls = h.upload.calls.lock().clone();
    assert_eq!(calls.len(), 1, "one upload per unit");
    // 400 pre-roll + 3,000 speech + 224 hangover + 500 zeros, to whole frames.
    assert!(
        (4100..=4200).contains(&calls[0].audio_ms),
        "{}",
        calls[0].audio_ms
    );
    let closed = h.closed();
    assert!(matches!(
        closed[0].1,
        SpeechActivity::TurnClosed {
            had_text: true,
            result_follows: true,
            ..
        }
    ));
}

#[tokio::test(start_paused = true)]
async fn speech_is_reported_before_any_transcript() {
    let h = Harness::new(
        SegmentProfile::for_tests(),
        constant(800, "hello"),
        None,
        None,
    );
    h.silence(200).await;
    h.speech(1000).await;
    let acts = h.activities();
    let started = acts
        .iter()
        .find(|(_, a)| matches!(a, SpeechActivity::Started { .. }))
        .expect("started");
    assert!(
        started.0 <= 200 + 224 + 40,
        "confirmed 224 ms after onset: {}",
        started.0
    );
    assert!(h.results().is_empty());
    let sustained: Vec<u32> = acts
        .iter()
        .filter_map(|(_, a)| match a {
            SpeechActivity::Started { sustained_ms, .. } => Some(*sustained_ms),
            _ => None,
        })
        .collect();
    assert!(sustained.starts_with(&[224, 384, 512]), "{sustained:?}");
}

#[tokio::test(start_paused = true)]
async fn two_segments_of_one_turn_give_an_interim_then_one_final_in_order() {
    // The first pause is "not finished"; the second is.
    let h = Harness::new(
        SegmentProfile::for_tests(),
        by_call(vec![(300, "I'd like to"), (300, "change my booking.")]),
        audio_model(&[0.2, 0.95], 0.95),
        None,
    );
    h.silence(200).await;
    h.speech(1500).await;
    h.silence(400).await; // cut at 224, "not finished"
    h.speech(1500).await;
    h.silence(2500).await;
    let results = h.results();
    let interims: Vec<&EngineResult> = results
        .iter()
        .map(|(_, r)| r)
        .filter(|r| !r.is_final)
        .collect();
    assert_eq!(interims.len(), 1, "{results:?}");
    assert_eq!(interims[0].transcript, "I'd like to");
    let finals = h.finals();
    assert_eq!(finals.len(), 1);
    assert_eq!(finals[0].1.transcript, "I'd like to change my booking.");
    assert_eq!(finals[0].1.turn_id, interims[0].turn_id);
    assert_eq!(h.closed().len(), 1, "one turn");
    h.assert_two_shapes_only();
}

#[tokio::test(start_paused = true)]
async fn results_are_released_in_sequence_when_responses_arrive_out_of_order() {
    let h = Harness::new(
        SegmentProfile::for_tests(),
        by_call(vec![(2500, "first part"), (100, "second part")]),
        audio_model(&[0.2, 0.95], 0.95),
        None,
    );
    h.silence(200).await;
    h.speech(1000).await;
    h.silence(400).await;
    h.speech(1000).await;
    h.silence(4000).await;
    let finals = h.finals();
    assert_eq!(finals.len(), 1);
    assert_eq!(finals[0].1.transcript, "first part second part");
    let seqs: Vec<u32> = h.sink.outcomes.lock().iter().map(|o| o.seq).collect();
    assert_eq!(seqs, vec![1, 2], "outcomes in sequence order");
}

#[tokio::test(start_paused = true)]
async fn a_turn_does_not_end_while_a_unit_is_in_flight() {
    let h = Harness::new(
        SegmentProfile::for_tests(),
        constant(3000, "slow vendor"),
        audio_model(&[], 0.95),
        None,
    );
    h.silence(200).await;
    h.speech(1000).await;
    let cut_at = h.clock.now_ms() + 224;
    h.silence(2000).await;
    assert!(h.closed().is_empty(), "the upload is still in flight");
    assert!(
        h.activities()
            .iter()
            .any(|(_, a)| matches!(a, SpeechActivity::EndpointDecided { .. }))
    );
    h.silence(2000).await;
    let closed = h.closed();
    assert_eq!(closed.len(), 1);
    assert!(closed[0].0 + 64 >= cut_at + 3000, "closed at the response");
}

#[tokio::test(start_paused = true)]
async fn end_of_speech_to_final_is_the_cut_delay_plus_the_upload_delay_when_the_verdict_is_finished()
 {
    let h = Harness::new(
        SegmentProfile::for_tests(),
        constant(800, "done"),
        audio_model(&[], 0.95),
        None,
    );
    h.silence(200).await;
    h.speech(3000).await;
    let end_of_speech = h.clock.now_ms();
    h.silence(2000).await;
    let finals = h.finals();
    let lag = finals[0].0 - end_of_speech;
    // 224 ms cut + 800 ms vendor, within one frame and one chunk of alignment.
    assert!((1024 - 32..=1024 + 64).contains(&lag), "{lag}");
}

#[tokio::test(start_paused = true)]
async fn upload_starts_at_the_cut_without_waiting_for_the_turn_decision() {
    // The audio model is slow to say anything; the upload must not wait for it.
    let h = Harness::new(
        SegmentProfile::for_tests(),
        constant(500, "x"),
        audio_model(&[0.1], 0.1),
        None,
    );
    h.silence(200).await;
    h.speech(1000).await;
    let end = h.clock.now_ms();
    h.silence(1000).await;
    let calls = h.upload.calls.lock().clone();
    assert!(
        calls[0].at_ms <= end + 224 + 40,
        "{} vs {}",
        calls[0].at_ms,
        end
    );
    assert!(!calls[0].turn_final);
}

#[tokio::test(start_paused = true)]
async fn the_deadline_instant_is_the_newest_cut_plus_the_deadline() {
    let h = Harness::new(SegmentProfile::for_tests(), constant(100, "x"), None, None);
    h.silence(200).await;
    h.speech(1000).await;
    h.silence(1000).await;
    let calls = h.upload.calls.lock().clone();
    assert!(
        (5960..=6000).contains(&calls[0].deadline_in_ms),
        "{}",
        calls[0].deadline_in_ms
    );
}

#[tokio::test(start_paused = true)]
async fn a_held_unit_starts_at_the_endpoint_with_the_newest_cut_plus_the_deadline() {
    let h = Harness::new(
        SegmentProfile::for_tests(),
        constant(100, "held"),
        None,
        None,
    );
    h.upload.speculative.store(false, Ordering::SeqCst);
    h.silence(200).await;
    h.speech(1000).await;
    let end = h.clock.now_ms();
    h.silence(2000).await;
    let calls = h.upload.calls.lock().clone();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].turn_final);
    // Handed over at the silence rule (512 ms of silence), with 6,000 ms from the cut (224 ms).
    assert!(
        calls[0].at_ms >= end + 512 && calls[0].at_ms <= end + 600,
        "{}",
        calls[0].at_ms - end
    );
    let expected = 6000 - (calls[0].at_ms - (end + 224));
    assert!((expected.saturating_sub(40)..=expected + 40).contains(&calls[0].deadline_in_ms));
}

#[tokio::test(start_paused = true)]
async fn under_the_per_turn_policy_nothing_is_uploaded_before_the_endpoint() {
    let mut p = SegmentProfile::for_tests();
    p.upload_policy = UploadPolicy::PerTurn;
    let h = Harness::new(
        p,
        constant(200, "one upload for the turn"),
        audio_model(&[0.1, 0.95], 0.95),
        None,
    );
    h.silence(200).await;
    h.speech(1000).await;
    h.silence(400).await;
    assert!(h.upload.calls.lock().is_empty());
    h.speech(1000).await;
    h.silence(2000).await;
    let calls = h.upload.calls.lock().clone();
    assert_eq!(calls.len(), 1, "the turn's segments joined into one upload");
    assert!(calls[0].turn_final);
    assert!(calls[0].audio_ms > 2400, "{}", calls[0].audio_ms);
    assert_eq!(h.finals().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn at_most_two_uploads_run_while_the_turn_is_open() {
    // 4.5 s per upload: the first two are still running at the endpoint, and the held unit,
    // handed over then with its deadline counted from the newest cut, still answers in time.
    let h = Harness::new(
        SegmentProfile::for_tests(),
        constant(4500, "x"),
        audio_model(&[0.1, 0.1, 0.1, 0.1], 0.1),
        None,
    );
    h.silence(200).await;
    for _ in 0..4 {
        h.speech(600).await;
        h.silence(300).await;
    }
    h.silence(8000).await;
    let calls = h.upload.calls.lock().clone();
    assert_eq!(
        calls.len(),
        3,
        "the third and fourth segments were held and joined"
    );
    assert!(
        calls[2].turn_final,
        "the held unit starts at the endpoint as the turn's last"
    );
    assert_eq!(
        h.upload.max_in_flight.load(Ordering::SeqCst),
        3,
        "two speculative plus the turn's last"
    );
    let finals = h.finals();
    assert_eq!(finals.len(), 1);
    assert_eq!(finals[0].1.transcript, "x x x");
}

#[tokio::test(start_paused = true)]
async fn the_text_model_rescues_a_one_word_answer_the_audio_model_called_unfinished() {
    let h = Harness::new(
        SegmentProfile::for_tests(),
        constant(300, "Yes."),
        audio_model(&[0.66], 0.66),
        Some(Arc::new(FixedTextModel(true))),
    );
    h.silence(200).await;
    h.speech(400).await;
    let end = h.clock.now_ms();
    h.silence(2000).await;
    let closed = h.closed();
    assert!(matches!(
        closed[0].1,
        SpeechActivity::TurnClosed {
            reason: TurnCloseReason::TextModel,
            ..
        }
    ));
    assert!(closed[0].0 - end < 1000, "{}", closed[0].0 - end);
}

#[tokio::test(start_paused = true)]
async fn with_no_model_saying_finished_the_turn_ends_at_the_ceiling() {
    let h = Harness::new(
        SegmentProfile::for_tests(),
        constant(300, "Yes."),
        audio_model(&[0.3], 0.3),
        None,
    );
    h.silence(200).await;
    h.speech(400).await;
    let end = h.clock.now_ms();
    h.silence(2500).await;
    let closed = h.closed();
    assert!(matches!(
        closed[0].1,
        SpeechActivity::TurnClosed {
            reason: TurnCloseReason::MaxEndpointing,
            ..
        }
    ));
    let lag = closed[0].0 - end;
    assert!((1504 - 32..=1504 + 64).contains(&lag), "{lag}");
}

#[tokio::test(start_paused = true)]
async fn without_an_audio_model_the_silence_rule_ends_the_turn() {
    let h = Harness::new(SegmentProfile::for_tests(), constant(100, "ok"), None, None);
    h.silence(200).await;
    h.speech(600).await;
    let end = h.clock.now_ms();
    h.silence(1500).await;
    let closed = h.closed();
    assert!(matches!(
        closed[0].1,
        SpeechActivity::TurnClosed {
            reason: TurnCloseReason::SilenceThreshold,
            ..
        }
    ));
    let lag = closed[0].0 - end;
    assert!((512 - 32..=600).contains(&lag), "{lag}");
}

#[tokio::test(start_paused = true)]
async fn a_cough_after_a_finished_sentence_does_not_reopen_the_turn() {
    let h = Harness::new(
        SegmentProfile::for_tests(),
        constant(1500, "That's all."),
        audio_model(&[0.95, 0.1], 0.1),
        None,
    );
    h.silence(200).await;
    h.speech(1200).await;
    h.silence(300).await;
    h.speech(200).await; // a cough: confirmed, under the minimum span
    h.silence(2500).await;
    let finals = h.finals();
    assert_eq!(finals.len(), 1);
    assert_eq!(finals[0].1.transcript, "That's all.");
    assert_eq!(h.closed().len(), 1);
    assert!(matches!(
        h.closed()[0].1,
        SpeechActivity::TurnClosed {
            reason: TurnCloseReason::EndOfTurnModel,
            ..
        }
    ));
    assert!(
        h.sink
            .outcomes
            .lock()
            .iter()
            .any(|o| o.kind == SegmentResultKind::Filtered(FilteredBy::TooShort))
    );
}

#[tokio::test(start_paused = true)]
async fn a_cough_alone_closes_a_turn_with_no_text_and_no_result() {
    let h = Harness::new(
        SegmentProfile::for_tests(),
        constant(100, "never"),
        audio_model(&[], 0.1),
        None,
    );
    h.silence(200).await;
    h.speech(200).await;
    h.silence(1000).await;
    assert!(h.results().is_empty());
    assert!(h.upload.calls.lock().is_empty(), "never uploaded");
    let closed = h.closed();
    assert!(matches!(
        closed[0].1,
        SpeechActivity::TurnClosed {
            had_text: false,
            result_follows: false,
            reason: TurnCloseReason::NoText,
            gaps: 0,
            ..
        }
    ));
}

#[tokio::test(start_paused = true)]
async fn a_lost_unit_closes_the_turn_with_a_gap_and_no_final() {
    let failing: Responder = Arc::new(|_, _| {
        (
            ms(100),
            Err(UnitFailure::Vendor(
                crate::transcriber::SegmentError::from_status(500, "down"),
            )),
        )
    });
    let h = Harness::new(
        SegmentProfile::for_tests(),
        failing,
        audio_model(&[], 0.95),
        None,
    );
    h.silence(200).await;
    h.speech(1000).await;
    h.silence(1500).await;
    assert!(h.results().is_empty());
    let closed = h.closed();
    let SpeechActivity::TurnClosed {
        had_text,
        gaps,
        lost_voiced_ms,
        ..
    } = closed[0].1
    else {
        panic!()
    };
    assert!(!had_text);
    assert_eq!(gaps, 1);
    assert!(lost_voiced_ms >= 900);
    let o = h.sink.outcomes.lock()[0].clone();
    assert_eq!(
        o.kind,
        SegmentResultKind::Failed(FailureClass::Vendor(ErrorClass::Vendor))
    );
}

#[tokio::test(start_paused = true)]
async fn a_stuck_vendor_still_closes_the_turn_within_the_resolution_deadline() {
    let never: Responder = Arc::new(|_, _| (ms(60_000), text("never")));
    let h = Harness::new(
        SegmentProfile::for_tests(),
        never,
        audio_model(&[], 0.95),
        None,
    );
    h.silence(200).await;
    h.speech(1000).await;
    let end = h.clock.now_ms();
    h.silence(8000).await;
    let closed = h.closed();
    assert_eq!(closed.len(), 1);
    assert!(closed[0].0 - end <= 6922, "{}", closed[0].0 - end);
    assert_eq!(h.sink.outcomes.lock()[0].kind, SegmentResultKind::TimedOut);
    assert_eq!(h.handle.facts().resolution_deadline_ms, 6922);
    assert_eq!(h.handle.facts().final_deadline_ms, 6000);
}

#[tokio::test(start_paused = true)]
async fn speech_that_is_not_input_is_neither_uploaded_nor_shown() {
    let h = Harness::new(
        SegmentProfile::for_tests(),
        constant(100, "over the greeting"),
        audio_model(&[], 0.95),
        None,
    );
    *h.handle.callbacks().admission.write() = Some(Arc::new(|_| SegmentAdmission {
        is_input: false,
        overlapped_agent_speech: true,
        agent_spoke_since_previous_segment: true,
    }));
    h.silence(200).await;
    h.speech(1000).await;
    h.silence(1500).await;
    assert!(h.upload.calls.lock().is_empty());
    assert!(h.results().is_empty());
    assert!(h.activities().iter().any(|(_, a)| matches!(
        a,
        SpeechActivity::Stopped {
            will_upload: false,
            ..
        }
    )));
    assert_eq!(
        h.sink.outcomes.lock()[0].kind,
        SegmentResultKind::Filtered(FilteredBy::NotInput)
    );
}

#[tokio::test(start_paused = true)]
async fn a_commit_after_only_silence_uploads_nothing_and_closes_one_turn_with_an_empty_final() {
    let h = Harness::new(SegmentProfile::for_tests(), constant(100, "x"), None, None);
    h.silence(600).await;
    let out = h.handle.flush().await.unwrap();
    assert!(out.result_follows);
    assert!(!out.will_upload);
    tokio::time::sleep(ms(50)).await;
    let finals = h.finals();
    assert_eq!(finals.len(), 1);
    assert_eq!(finals[0].1.transcript, "");
    assert!(h.upload.calls.lock().is_empty());
    // A commit with no new audio does nothing.
    let again = h.handle.flush().await.unwrap();
    assert_eq!(
        again,
        FlushOutcome {
            turn_id: None,
            will_upload: false,
            result_follows: false
        }
    );
}

#[tokio::test(start_paused = true)]
async fn a_commit_during_speech_cuts_at_once_and_seals_the_turn() {
    let h = Harness::new(
        SegmentProfile::for_tests(),
        constant(300, "No."),
        audio_model(&[], 0.1),
        None,
    );
    h.silence(200).await;
    h.speech(400).await;
    let out = h.handle.flush().await.unwrap();
    assert!(out.will_upload);
    assert!(out.result_follows);
    tokio::time::sleep(ms(500)).await;
    let finals = h.finals();
    assert_eq!(finals.len(), 1);
    assert_eq!(finals[0].1.transcript, "No.");
    assert!(matches!(
        h.closed()[0].1,
        SpeechActivity::TurnClosed {
            reason: TurnCloseReason::Commit,
            ..
        }
    ));
}

#[tokio::test(start_paused = true)]
async fn speech_events_are_not_delayed_by_a_blocked_result_callback() {
    let h = Harness::new(
        SegmentProfile::for_tests(),
        constant(100, "first"),
        None,
        None,
    );
    let blocked = Arc::new(AtomicBool::new(false));
    let b = Arc::clone(&blocked);
    *h.handle.callbacks().result.write() = Some(Arc::new(move |_r| {
        b.store(true, Ordering::SeqCst);
        Box::pin(async { tokio::time::sleep(Duration::from_secs(30)).await })
    }));
    h.silence(200).await;
    h.speech(600).await;
    h.silence(1200).await;
    assert!(
        blocked.load(Ordering::SeqCst),
        "the result callback is now stuck"
    );
    let before = h.activities().len();
    h.speech(600).await;
    let starts_after: usize = h.activities()[before..]
        .iter()
        .filter(|(_, a)| matches!(a, SpeechActivity::Started { .. }))
        .count();
    assert!(starts_after >= 1, "speech events still arrive");
}

#[tokio::test(start_paused = true)]
async fn stopping_ends_the_engine_and_cancels_uploads_in_flight() {
    let h = Harness::new(
        SegmentProfile::for_tests(),
        constant(5000, "late"),
        None,
        None,
    );
    h.silence(200).await;
    h.speech(800).await;
    h.silence(400).await;
    assert_eq!(h.upload.in_flight.load(Ordering::SeqCst), 1);
    h.handle.stop().await;
    tokio::time::sleep(ms(10)).await;
    assert!(!h.handle.is_alive());
    assert_eq!(
        h.upload.in_flight.load(Ordering::SeqCst),
        0,
        "the upload was dropped"
    );
    assert!(h.handle.send_audio(Bytes::from_static(&[0, 0])).is_err());
    assert!(
        h.sink
            .outcomes
            .lock()
            .iter()
            .any(|o| o.kind == SegmentResultKind::Failed(FailureClass::Cancelled))
    );
    tokio::time::sleep(ms(6000)).await;
    assert!(h.results().is_empty());
}

#[tokio::test(start_paused = true)]
async fn the_session_language_travels_with_every_upload() {
    let mut cfg = EngineConfig::new(SegmentProfile::for_tests());
    cfg.language = Some("en".into());
    let h = Harness::with_config(cfg, constant(100, "x"), None, None);
    h.silence(200).await;
    h.speech(600).await;
    h.silence(800).await;
    h.handle.set_language(Some("de".into()));
    h.speech(600).await;
    h.silence(800).await;
    let langs: Vec<Option<String>> = h
        .upload
        .calls
        .lock()
        .iter()
        .map(|c| c.language.clone())
        .collect();
    assert_eq!(langs, vec![Some("en".into()), Some("de".into())]);
}

fn detected(lang: &'static str) -> Responder {
    Arc::new(move |_, _| {
        (
            ms(100),
            Ok(SegmentTranscript {
                text: "words".into(),
                detected_language: Some(lang.into()),
                ..Default::default()
            }),
        )
    })
}

async fn utterances(h: &Harness, n: usize) {
    h.silence(200).await;
    for _ in 0..n {
        h.speech(600).await;
        h.silence(800).await;
    }
}

fn languages_sent(h: &Harness) -> Vec<Option<String>> {
    h.upload
        .calls
        .lock()
        .iter()
        .map(|c| c.language.clone())
        .collect()
}

/// Addendum B7: with no session language, two segments the vendor detected alike pin it for every
/// later upload.
#[tokio::test(start_paused = true)]
async fn two_agreeing_segments_pin_the_session_language_for_later_uploads() {
    let h = Harness::with_config(
        EngineConfig::new(SegmentProfile::for_tests()),
        detected("english"),
        None,
        None,
    );
    utterances(&h, 4).await;
    assert_eq!(
        languages_sent(&h),
        vec![None, None, Some("en".into()), Some("en".into())]
    );
}

#[tokio::test(start_paused = true)]
async fn a_session_that_named_its_language_never_votes() {
    let mut cfg = EngineConfig::new(SegmentProfile::for_tests());
    cfg.language = Some("hi".into());
    let h = Harness::with_config(cfg, detected("english"), None, None);
    utterances(&h, 3).await;
    assert_eq!(languages_sent(&h), vec![Some("hi".into()); 3]);
}

#[tokio::test(start_paused = true)]
async fn a_language_the_client_sets_mid_call_ends_the_vote() {
    let h = Harness::with_config(
        EngineConfig::new(SegmentProfile::for_tests()),
        detected("english"),
        None,
        None,
    );
    h.handle.set_language(Some("fr".into()));
    utterances(&h, 3).await;
    assert_eq!(languages_sent(&h), vec![Some("fr".into()); 3]);
}

#[tokio::test(start_paused = true)]
async fn a_refused_credential_reaches_the_fatal_callback_once() {
    let refused: Responder = Arc::new(|_, _| {
        (
            ms(100),
            Err(UnitFailure::Vendor(
                crate::transcriber::SegmentError::from_status(401, "bad key"),
            )),
        )
    });
    let h = Harness::new(SegmentProfile::for_tests(), refused, None, None);
    h.silence(200).await;
    h.speech(600).await;
    h.silence(1000).await;
    h.speech(600).await;
    h.silence(1000).await;
    let f = h.fatals.lock().clone();
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].reason, "credential_rejected");
}

#[tokio::test(start_paused = true)]
async fn mulaw_telephone_audio_at_eight_khz_is_segmented() {
    let mut cfg = EngineConfig::new(SegmentProfile::for_tests());
    cfg.encoding = "mulaw".into();
    cfg.sample_rate = 8000;
    let h = Harness::with_config(cfg, constant(200, "telephone"), None, None);
    // 20 ms of 8 kHz μ-law is 160 bytes: loud = alternating extremes, quiet = 0xFF (zero).
    for (ms_len, loud) in [(300u64, false), (1000, true), (1500, false)] {
        for _ in 0..ms_len / 20 {
            let bytes: Vec<u8> = (0..160)
                .map(|i| {
                    if loud {
                        if i % 8 < 4 { 0x1F } else { 0x9F }
                    } else {
                        0xFF
                    }
                })
                .collect();
            h.handle.send_audio(Bytes::from(bytes)).unwrap();
            tokio::time::sleep(ms(20)).await;
        }
    }
    assert_eq!(h.finals().len(), 1);
    assert_eq!(h.handle.facts().resampled_from_hz, Some(8000));
}

#[tokio::test(start_paused = true)]
async fn the_ingest_queue_drops_over_budget_and_raises_one_notice() {
    let mut p = SegmentProfile::for_tests();
    p.ingest_budget_bytes = 1000;
    let h = Harness::new(p, constant(100, "x"), None, None);
    for _ in 0..5 {
        h.handle.send_audio(Bytes::from(vec![0u8; 640])).unwrap();
    }
    let drops: Vec<SttNotice> = h
        .notices
        .lock()
        .iter()
        .filter(|n| matches!(n.kind, NoticeKind::AudioDropped { .. }))
        .cloned()
        .collect();
    assert_eq!(drops.len(), 1);
}

#[tokio::test(start_paused = true)]
async fn an_input_stall_cuts_the_segment_and_the_turn_closes_on_idle_time() {
    let mut p = SegmentProfile::for_tests();
    p.input_stall_ms = Some(1000);
    let h = Harness::new(
        p,
        constant(200, "then nothing"),
        audio_model(&[], 0.1),
        None,
    );
    h.silence(200).await;
    h.speech(800).await;
    // The client stops sending audio entirely.
    tokio::time::sleep(ms(4000)).await;
    assert_eq!(h.upload.calls.lock().len(), 1, "cut at the stall");
    let closed = h.closed();
    assert_eq!(closed.len(), 1);
    assert!(
        matches!(
            closed[0].1,
            SpeechActivity::TurnClosed {
                reason: TurnCloseReason::InputIdle,
                ..
            }
        ),
        "{:?}",
        closed[0].1
    );
    assert_eq!(h.finals()[0].1.transcript, "then nothing");
}

#[tokio::test(start_paused = true)]
async fn a_long_turn_is_sealed_at_its_limit() {
    let mut p = SegmentProfile::for_tests();
    p.max_turn_ms = 3000;
    let h = Harness::new(p, constant(100, "monologue"), audio_model(&[], 0.1), None);
    h.silence(200).await;
    h.speech(5000).await;
    h.silence(500).await;
    assert!(h.closed().iter().any(|(_, a)| matches!(
        a,
        SpeechActivity::TurnClosed {
            reason: TurnCloseReason::MaxTurnDuration,
            ..
        }
    )));
}

#[tokio::test(start_paused = true)]
async fn a_split_segment_that_comes_back_empty_raises_the_thresholds() {
    let mut p = SegmentProfile::for_tests();
    p.max_segment_ms = 6000; // soft split at 1,000 ms … at least 1 s: max(6000-5000, 1000)
    let empty_first: Responder = Arc::new(|n, _| (ms(50), text(if n == 0 { "" } else { "x" })));
    let h = Harness::new(p, empty_first, audio_model(&[], 0.1), None);
    h.silence(200).await;
    // Noise with tiny dips: splits at the first 96 ms dip after the soft limit.
    for _ in 0..6 {
        h.speech(900).await;
        h.silence(100).await;
    }
    h.silence(2000).await;
    assert!(
        h.notices
            .lock()
            .iter()
            .any(|n| matches!(n.kind, NoticeKind::NoiseThresholdRaised { .. }))
    );
    assert!(
        h.sink
            .outcomes
            .lock()
            .iter()
            .any(|o| o.kind == SegmentResultKind::Filtered(FilteredBy::NoiseSuspected))
    );
}

#[tokio::test(start_paused = true)]
async fn interims_can_be_switched_off() {
    let mut p = SegmentProfile::for_tests();
    p.interims = InterimMode::Off;
    let h = Harness::new(
        p,
        by_call(vec![(100, "one"), (100, "two")]),
        audio_model(&[0.1, 0.95], 0.95),
        None,
    );
    h.silence(200).await;
    h.speech(800).await;
    h.silence(400).await;
    h.speech(800).await;
    h.silence(2000).await;
    assert!(h.results().iter().all(|(_, r)| r.is_final));
    assert_eq!(h.finals()[0].1.transcript, "one two");
}

#[tokio::test(start_paused = true)]
async fn a_second_turn_gets_its_own_id_and_final() {
    let h = Harness::new(
        SegmentProfile::for_tests(),
        by_call(vec![(100, "first"), (100, "second")]),
        audio_model(&[], 0.95),
        None,
    );
    h.silence(200).await;
    h.speech(800).await;
    h.silence(1500).await;
    h.speech(800).await;
    h.silence(1500).await;
    let finals = h.finals();
    assert_eq!(finals.len(), 2);
    assert_ne!(finals[0].1.turn_id, finals[1].1.turn_id);
    assert_eq!(finals[1].1.transcript, "second");
}
