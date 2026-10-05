//! Spec 025 — the voice-agent turn engine, against a recording speech output and a scripted
//! budgateway. No vendor, no network: every case drives the engine's own decisions.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use bud_auth::voice_agent::{VoiceAgentEntry, parse_voice_agent_blob};
use futures::stream::BoxStream;
use parking_lot::Mutex;
use serde_json::{Map, Value, json};
use tokio::sync::mpsc;

use super::*;

// ---------------------------------------------------------------------------------------------
// Fakes
// ---------------------------------------------------------------------------------------------

/// Records what was spoken. Each speak "delivers" its audio at once, at 50 chars/s.
#[derive(Default)]
struct FakeSpeech {
    epoch: AtomicUsize,
    spoken: Mutex<Vec<(String, bool)>>,
    audio_out: AtomicU64,
    /// Audio still queued at the client (drives `is_audible` / `playout_remaining_ms`).
    remaining: AtomicU64,
    clears: AtomicUsize,
    /// How long each speak takes to return (synthesis); 0 = at once.
    synth_ms: AtomicU64,
    /// Queued audio plays out in (paused-clock) time: `playout_remaining_ms` drains.
    paced: std::sync::atomic::AtomicBool,
    queued_until: Mutex<Option<tokio::time::Instant>>,
    /// The output cannot carry a sound (a compressed TTS format).
    no_sound: std::sync::atomic::AtomicBool,
    /// What reached the caller, in order: `say:<text>`, or `tone` for a run of pulses.
    timeline: Mutex<Vec<String>>,
    /// When each pulse of the tone was played.
    pulses: Mutex<Vec<tokio::time::Instant>>,
}

impl FakeSpeech {
    fn spoken(&self) -> Vec<String> {
        self.spoken.lock().iter().map(|(t, _)| t.clone()).collect()
    }

    fn deliver(&self, ms: u64) {
        self.audio_out.fetch_add(ms, Ordering::AcqRel);
        self.remaining.fetch_add(ms, Ordering::AcqRel);
        let now = tokio::time::Instant::now();
        let mut q = self.queued_until.lock();
        let from = q.filter(|t| *t > now).unwrap_or(now);
        *q = Some(from + Duration::from_millis(ms));
    }

    fn timeline(&self) -> Vec<String> {
        self.timeline.lock().clone()
    }
}

#[async_trait]
impl SpeechOut for FakeSpeech {
    fn clear_epoch(&self) -> usize {
        self.epoch.load(Ordering::Acquire)
    }
    async fn speak(&self, text: &str, epoch: usize, interruptible: bool) -> bool {
        if self.epoch.load(Ordering::Acquire) != epoch {
            return false;
        }
        let synth = self.synth_ms.load(Ordering::Acquire);
        if synth > 0 {
            tokio::time::sleep(Duration::from_millis(synth)).await;
        }
        self.spoken.lock().push((text.to_string(), interruptible));
        self.timeline.lock().push(format!("say:{text}"));
        let ms = (text.chars().count() as u64) * 20;
        self.deliver(ms);
        true
    }
    async fn clear(&self) {
        self.epoch.fetch_add(1, Ordering::AcqRel);
        self.remaining.store(0, Ordering::Release);
        *self.queued_until.lock() = None;
        self.clears.fetch_add(1, Ordering::AcqRel);
    }
    fn is_audible(&self) -> bool {
        self.remaining.load(Ordering::Acquire) > 0
    }
    fn audio_out_ms(&self) -> u64 {
        self.audio_out.load(Ordering::Acquire)
    }
    fn playout_remaining_ms(&self) -> u64 {
        if self.paced.load(Ordering::Acquire) {
            let now = tokio::time::Instant::now();
            return self
                .queued_until
                .lock()
                .map_or(0, |t| t.saturating_duration_since(now).as_millis() as u64);
        }
        self.remaining.load(Ordering::Acquire)
    }
    fn sound_rate(&self) -> Option<u32> {
        (!self.no_sound.load(Ordering::Acquire)).then_some(16_000)
    }
    async fn play_sound(&self, pcm: &[i16], epoch: usize) -> bool {
        if self.epoch.load(Ordering::Acquire) != epoch {
            return false;
        }
        let ms = pcm.len() as u64 * 1000 / 16_000;
        self.pulses.lock().push(tokio::time::Instant::now());
        {
            let mut t = self.timeline.lock();
            if t.last().map(String::as_str) != Some("tone") {
                t.push("tone".into());
            }
        }
        self.deliver(ms);
        true
    }
}

/// A scripted Bud gateway: each `open` takes the next script; events are released one by one.
#[derive(Default)]
struct FakeBackend {
    scripts: Mutex<std::collections::VecDeque<Result<Vec<Step>, TurnFailure>>>,
    requests: Mutex<Vec<AgentTurnRequest>>,
    cancels: Mutex<Vec<String>>,
    /// How long `open` takes to answer at all (budprompt listing an agent's tools first).
    open_delay: Mutex<Duration>,
}

#[derive(Clone)]
enum Step {
    Ev(AgentEvent),
    /// Hold the stream for this long (a slow model, a running tool).
    Wait(Duration),
    /// A bug: the turn task panics while polling the stream.
    Panic,
}

impl FakeBackend {
    fn push(&self, script: Result<Vec<Step>, TurnFailure>) {
        self.scripts.lock().push_back(script);
    }
}

#[async_trait]
impl TurnBackend for FakeBackend {
    async fn open(
        &self,
        request: &AgentTurnRequest,
        _bearer: &str,
        _traceparent: Option<&str>,
    ) -> Result<BoxStream<'static, Result<AgentEvent, TurnFailure>>, TurnFailure> {
        self.requests.lock().push(request.clone());
        let delay = *self.open_delay.lock();
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        let script = self
            .scripts
            .lock()
            .pop_front()
            .unwrap_or_else(|| Ok(vec![]))?;
        let s = futures::stream::unfold(script.into_iter(), |mut it| async move {
            loop {
                match it.next()? {
                    Step::Wait(d) => tokio::time::sleep(d).await,
                    Step::Ev(e) => return Some((Ok(e), it)),
                    Step::Panic => panic!("scripted turn-task panic"),
                }
            }
        });
        Ok(Box::pin(s))
    }
    async fn cancel(&self, response_id: &str, _prompt_name: &str, _bearer: &str) {
        self.cancels.lock().push(response_id.to_string());
    }
}

fn entry(extra: Value) -> Arc<VoiceAgentEntry> {
    let mut base = json!({
        "prompt_id": "c0de", "version": 3,
        "stt": {"endpoint_id": "stt-1"}, "tts": {"endpoint_id": "tts-1"},
        "fillers": {"tool_call_after_ms": 0, "slow_response_after_ms": 0, "messages": ["One moment."]},
        "idle": {"end_after_silence_ms": 600000},
    });
    if let (Value::Object(b), Value::Object(e)) = (&mut base, extra) {
        b.extend(e);
    }
    Arc::new(parse_voice_agent_blob(&base.to_string()).unwrap())
}

struct Harness {
    engine: Arc<AgentEngine>,
    speech: Arc<FakeSpeech>,
    backend: Arc<FakeBackend>,
    rx: mpsc::UnboundedReceiver<AgentSignal>,
}

fn harness(
    entry: Arc<VoiceAgentEntry>,
    variables: Option<Map<String, Value>>,
    text_only: bool,
) -> Harness {
    let speech = Arc::new(FakeSpeech::default());
    let backend = Arc::new(FakeBackend::default());
    let (tx, rx) = mpsc::unbounded_channel();
    let engine = AgentEngine::new(
        AgentSessionConfig {
            session_id: "s1".into(),
            prompt_name: "support".into(),
            version: 3,
            entry,
            conversation_id: "conv_voice_s1".into(),
            variables,
            text_only,
        },
        Arc::clone(&backend) as Arc<dyn TurnBackend>,
        Arc::clone(&speech) as Arc<dyn SpeechOut>,
        crate::auth::SessionCredential::new("bud-key"),
        tx,
    );
    Harness {
        engine,
        speech,
        backend,
        rx,
    }
}

fn created(id: &str) -> Step {
    Step::Ev(AgentEvent::Created {
        response_id: id.into(),
        conversation_id: Some("conv_voice_s1".into()),
    })
}
fn delta(t: &str) -> Step {
    Step::Ev(AgentEvent::TextDelta {
        item_id: Some("msg_1".into()),
        delta: t.into(),
    })
}
fn completed() -> Step {
    Step::Ev(AgentEvent::Completed {
        usage: Some(json!({"input_tokens": 10, "output_tokens": 5})),
        output: vec![],
    })
}

/// Collect signals until `ResponseDone` for `turn` (bounded).
async fn until_done(h: &mut Harness, turn: u64) -> Vec<AgentSignal> {
    let mut out = Vec::new();
    loop {
        let s = tokio::time::timeout(Duration::from_secs(120), h.rx.recv())
            .await
            .expect("signal")
            .expect("channel open");
        let done = matches!(&s, AgentSignal::ResponseDone { turn: t, .. } if *t == turn);
        out.push(s);
        if done {
            return out;
        }
    }
}

fn done_of(signals: &[AgentSignal]) -> (TurnStatus, Option<String>, String, Option<Value>) {
    signals
        .iter()
        .find_map(|s| match s {
            AgentSignal::ResponseDone {
                status,
                response_id,
                transcript,
                usage,
                ..
            } => Some((
                *status,
                response_id.clone(),
                transcript.clone(),
                usage.clone(),
            )),
            _ => None,
        })
        .expect("a ResponseDone")
}

// ---------------------------------------------------------------------------------------------
// Cases
// ---------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn a_turn_streams_into_speech_and_reports_in_order() {
    let mut h = harness(entry(json!({})), None, false);
    h.backend.push(Ok(vec![
        created("resp_1"),
        delta("Your order shipped on Monday"),
        delta(", and it should arrive "),
        delta("**by Friday**. "),
        delta("Anything else?"),
        completed(),
    ]));
    h.engine.start_turn("where is my order".into()).await;
    let signals = until_done(&mut h, 0).await;

    assert!(
        matches!(&signals[0], AgentSignal::ResponseStarted { turn: 0, kind: TurnKind::Agent, input: Some(i) } if i == "where is my order")
    );
    assert!(
        matches!(&signals[1], AgentSignal::ResponseCreated { response_id, .. } if response_id == "resp_1")
    );
    assert_eq!(
        h.speech.spoken(),
        vec![
            "Your order shipped on Monday,",
            "and it should arrive by Friday.",
            "Anything else?"
        ],
        "first chunk at the clause, markdown stripped before TTS"
    );
    let (status, rid, transcript, usage) = done_of(&signals);
    assert_eq!(status, TurnStatus::Completed);
    assert_eq!(rid.as_deref(), Some("resp_1"));
    assert_eq!(
        transcript,
        "Your order shipped on Monday, and it should arrive **by Friday**. Anything else?",
        "the transcript keeps the agent's own text"
    );
    assert_eq!(usage, Some(json!({"input_tokens": 10, "output_tokens": 5})));

    let req = h.backend.requests.lock()[0].clone();
    assert_eq!(req.prompt_name, "support");
    assert_eq!(req.version, 3);
    assert_eq!(req.conversation_id, "conv_voice_s1");
    assert!(req.truncate.is_none());
    assert_eq!(req.body()["bud_channel"], "voice");
}

#[tokio::test(start_paused = true)]
async fn a_barge_in_mid_reply_cancels_and_the_next_turn_truncates_history() {
    let mut h = harness(entry(json!({})), None, false);
    h.backend.push(Ok(vec![
        created("resp_1"),
        delta("Your order shipped on Monday, "),
        Step::Wait(Duration::from_secs(5)),
        delta("and it will arrive Friday."),
        completed(),
    ]));
    h.engine.start_turn("where is my order".into()).await;
    // Wait until the first chunk was spoken.
    loop {
        if !h.speech.spoken().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // The client has played 300 ms of the 580 ms delivered.
    h.speech.remaining.store(280, Ordering::Release);
    h.engine.barge_in().await;
    let signals = until_done(&mut h, 0).await;

    let truncated = signals
        .iter()
        .find_map(|s| match s {
            AgentSignal::Truncated {
                spoken,
                response_id,
                ..
            } => Some((spoken.clone(), response_id.clone())),
            _ => None,
        })
        .expect("a Truncated signal");
    assert_eq!(truncated.1.as_deref(), Some("resp_1"));
    assert!(
        "Your order shipped on Monday,".starts_with(&truncated.0) && !truncated.0.is_empty(),
        "only what was heard: {:?}",
        truncated.0
    );
    assert_eq!(done_of(&signals).0, TurnStatus::Cancelled);
    assert!(
        h.speech.clears.load(Ordering::Acquire) >= 1,
        "speech stops at once"
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        h.backend.cancels.lock().as_slice(),
        ["resp_1"],
        "the run is cancelled at budprompt"
    );

    // The next turn carries the truncate.
    h.backend
        .push(Ok(vec![created("resp_2"), delta("Sure."), completed()]));
    h.engine.start_turn("actually, cancel it".into()).await;
    let _ = until_done(&mut h, 1).await;
    let req = h.backend.requests.lock()[1].clone();
    let t = req.truncate.expect("bud_truncate on the next turn");
    assert_eq!(t.response_id, "resp_1");
    assert_eq!(t.output_text, truncated.0);
}

/// "Caller can interrupt the agent" off: what the caller says while the agent answers is not a
/// turn, so the answer is never cut. Live, the caller's sentence still cancelled the reply: the
/// barge-in was skipped, but the next turn it started superseded the answer anyway.
#[tokio::test(start_paused = true)]
async fn with_interruptions_off_the_caller_cannot_cut_the_answer() {
    use crate::core::turn::TurnEvent;
    let mut h = harness(
        entry(json!({"interruption": {"enabled": false}})),
        None,
        false,
    );
    h.backend.push(Ok(vec![
        created("resp_1"),
        delta("One, two, three, "),
        Step::Wait(Duration::from_secs(3)),
        delta("four, five."),
        completed(),
    ]));
    h.engine.start_turn("count".into()).await;
    while h.speech.spoken().is_empty() {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        !h.engine.accepts_speech(),
        "while it answers, speech is not input"
    );
    h.engine
        .handle_turn_events(&[
            TurnEvent::Started {
                turn_id: 1,
                interrupt: true,
            },
            TurnEvent::Stopped {
                turn_id: 1,
                transcript: "stop counting right now".into(),
            },
        ])
        .await;
    let signals = until_done(&mut h, 0).await;
    assert_eq!(done_of(&signals).0, TurnStatus::Completed);
    assert_eq!(
        h.speech.clears.load(Ordering::Acquire),
        0,
        "nothing was cleared"
    );
    assert_eq!(
        h.backend.requests.lock().len(),
        1,
        "the caller's words started no turn"
    );

    // Once the answer has played out, the caller is heard again.
    h.speech.remaining.store(0, Ordering::Release);
    assert!(h.engine.accepts_speech());
}

/// The greeting has its own switch: it may be talked over while answers may not, and the
/// other way round.
#[tokio::test(start_paused = true)]
async fn the_greeting_follows_its_own_switch_not_the_answers() {
    for (greeting_cut, answers_cut) in [(true, false), (false, true)] {
        let h = harness(
            entry(json!({
                "interruption": {"enabled": answers_cut},
                "greeting": {"mode": "static", "text": "Hi, this is Bud support.", "interruptible": greeting_cut},
            })),
            None,
            false,
        );
        let e = Arc::clone(&h.engine);
        tokio::spawn(async move { e.greet().await });
        while h.speech.spoken().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(h.engine.accepts_speech(), greeting_cut, "while it greets");
        // The fake delivers at once; the greeting turn ends at its drain deadline.
        tokio::time::sleep(Duration::from_secs(10)).await;
        assert!(
            !h.engine.has_active_turn(),
            "the greeting has been delivered"
        );
        assert_eq!(
            h.engine.accepts_speech(),
            greeting_cut,
            "while the greeting is still heard"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn with_interruptions_on_speech_is_input_even_mid_answer() {
    let h = harness(entry(json!({})), None, false);
    h.backend.push(Ok(vec![
        created("resp_1"),
        Step::Wait(Duration::from_secs(3)),
        delta("Hi."),
        completed(),
    ]));
    h.engine.start_turn("hi".into()).await;
    assert!(h.engine.accepts_speech());
}

#[tokio::test(start_paused = true)]
async fn a_barge_in_while_the_finished_reply_still_plays_truncates_it() {
    let mut h = harness(entry(json!({})), None, false);
    h.backend.push(Ok(vec![
        created("resp_1"),
        delta("This answer has two long sentences in it. The second one keeps going for a while."),
        completed(),
    ]));
    h.engine.start_turn("explain".into()).await;
    let _ = until_done(&mut h, 0).await;
    // Done generating, but the client still has 1.2 s of it queued.
    h.speech.remaining.store(1_200, Ordering::Release);
    h.engine.barge_in().await;
    let s = tokio::time::timeout(Duration::from_secs(1), h.rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(s, AgentSignal::Truncated { ref response_id, .. } if response_id.as_deref() == Some("resp_1"))
    );

    h.backend.push(Ok(vec![created("resp_2"), completed()]));
    h.engine.start_turn("stop".into()).await;
    let _ = until_done(&mut h, 1).await;
    assert!(h.backend.requests.lock()[1].truncate.is_some());
}

#[tokio::test(start_paused = true)]
async fn a_reply_heard_to_the_end_is_never_truncated() {
    let mut h = harness(entry(json!({})), None, false);
    h.backend
        .push(Ok(vec![created("resp_1"), delta("Done."), completed()]));
    h.engine.start_turn("go".into()).await;
    let _ = until_done(&mut h, 0).await;
    h.speech.remaining.store(0, Ordering::Release);
    h.engine.barge_in().await;
    h.backend.push(Ok(vec![created("resp_2"), completed()]));
    h.engine.start_turn("next".into()).await;
    let signals = until_done(&mut h, 1).await;
    assert!(
        !signals
            .iter()
            .any(|s| matches!(s, AgentSignal::Truncated { .. }))
    );
    assert!(h.backend.requests.lock()[1].truncate.is_none());
}

#[tokio::test(start_paused = true)]
async fn turns_never_overlap() {
    let mut h = harness(entry(json!({})), None, false);
    h.backend.push(Ok(vec![
        created("resp_1"),
        delta("Long answer starting now, "),
        Step::Wait(Duration::from_secs(30)),
        completed(),
    ]));
    h.engine.start_turn("first".into()).await;
    loop {
        if !h.speech.spoken().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    h.backend
        .push(Ok(vec![created("resp_2"), delta("Second."), completed()]));
    h.engine.start_turn("second".into()).await;
    let first = until_done(&mut h, 0).await;
    assert_eq!(
        done_of(&first).0,
        TurnStatus::Cancelled,
        "the live turn ends before the next starts"
    );
    let second = until_done(&mut h, 1).await;
    assert_eq!(done_of(&second).0, TurnStatus::Completed);
}

#[tokio::test(start_paused = true)]
async fn missing_variables_refuse_the_turn_without_calling_the_agent() {
    let mut h = harness(
        entry(json!({"input": {"required_variables": ["customer_id"], "has_input_schema": true}})),
        None,
        false,
    );
    h.engine.start_turn("hi".into()).await;
    let s = h.rx.recv().await.unwrap();
    assert!(
        matches!(s, AgentSignal::Error { code: "missing_variables", ref message } if message.contains("customer_id"))
    );
    assert!(h.backend.requests.lock().is_empty());

    h.engine
        .set_variables(Map::from_iter([("customer_id".to_string(), json!("c1"))]))
        .unwrap();
    h.backend.push(Ok(vec![created("resp_1"), completed()]));
    h.engine.start_turn("hi".into()).await;
    let _ = until_done(&mut h, 1).await;
    let req = h.backend.requests.lock()[0].clone();
    assert_eq!(req.variables.unwrap()["customer_id"], "c1");
    assert!(
        h.engine.set_variables(Map::new()).is_err(),
        "variables are fixed once the conversation has started"
    );
}

#[tokio::test(start_paused = true)]
async fn a_rate_limit_speaks_the_degradation_message_and_keeps_the_truncate() {
    let mut h = harness(
        entry(json!({"degradation_message": "Busy, try again."})),
        None,
        false,
    );
    // Turn 0: cut off so a truncate is pending.
    h.backend.push(Ok(vec![
        created("resp_1"),
        delta("A fairly long first answer here, "),
        Step::Wait(Duration::from_secs(9)),
        completed(),
    ]));
    h.engine.start_turn("one".into()).await;
    loop {
        if !h.speech.spoken().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    h.speech.remaining.store(500, Ordering::Release);
    h.engine.barge_in().await;
    let _ = until_done(&mut h, 0).await;

    h.backend
        .push(Err(TurnFailure::RateLimited("slow down".into())));
    h.engine.start_turn("two".into()).await;
    let signals = until_done(&mut h, 1).await;
    assert!(signals.iter().any(|s| matches!(
        s,
        AgentSignal::Error {
            code: "rate_limit_exceeded",
            ..
        }
    )));
    assert_eq!(done_of(&signals).0, TurnStatus::Failed);
    assert_eq!(
        h.speech.spoken().last().map(String::as_str),
        Some("Busy, try again.")
    );

    h.backend.push(Ok(vec![created("resp_3"), completed()]));
    h.engine.start_turn("three".into()).await;
    let _ = until_done(&mut h, 2).await;
    let req = h.backend.requests.lock()[2].clone();
    assert_eq!(
        req.truncate.map(|t| t.response_id).as_deref(),
        Some("resp_1"),
        "budprompt never saw the refused turn, so its truncate still applies"
    );
}

#[tokio::test(start_paused = true)]
async fn an_approval_requirement_speaks_the_approval_message() {
    let mut h = harness(
        entry(json!({"approval_message": "I need approval first."})),
        None,
        false,
    );
    h.backend.push(Err(TurnFailure::ApprovalRequired(
        "approval_required_foreground".into(),
    )));
    h.engine.start_turn("delete my account".into()).await;
    let signals = until_done(&mut h, 0).await;
    assert!(signals.iter().any(|s| matches!(
        s,
        AgentSignal::Error {
            code: "approval_required",
            ..
        }
    )));
    assert_eq!(h.speech.spoken(), vec!["I need approval first."]);
}

#[tokio::test(start_paused = true)]
async fn invalid_variables_are_reported_silently() {
    let mut h = harness(entry(json!({})), None, false);
    h.backend.push(Err(TurnFailure::InvalidVariables(
        "age must be an integer".into(),
    )));
    h.engine.start_turn("hi".into()).await;
    let signals = until_done(&mut h, 0).await;
    assert!(signals.iter().any(|s| matches!(
        s,
        AgentSignal::Error {
            code: "invalid_variables",
            ..
        }
    )));
    assert!(h.speech.spoken().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_failed_run_after_partial_speech_does_not_talk_over_itself_with_silence() {
    let mut h = harness(entry(json!({"degradation_message": "Sorry."})), None, false);
    h.backend.push(Ok(vec![
        created("resp_1"),
        delta("Let me check that for you. "),
        Step::Ev(AgentEvent::Failed {
            code: "server_error".into(),
            message: "boom".into(),
        }),
    ]));
    h.engine.start_turn("hi".into()).await;
    let signals = until_done(&mut h, 0).await;
    assert_eq!(done_of(&signals).0, TurnStatus::Failed);
    assert_eq!(
        h.speech.spoken(),
        vec!["Let me check that for you.", "Sorry."]
    );
}

#[tokio::test(start_paused = true)]
async fn a_structured_agent_speaks_only_its_field_and_reports_the_json() {
    let mut h = harness(
        entry(json!({"structured_output": {"speak_field": "answer", "has_output_schema": true}})),
        None,
        false,
    );
    h.backend.push(Ok(vec![
        created("resp_1"),
        delta(r#"{"confidence": 0.9, "answer": "It shipped Monday."#),
        delta(r#" It arrives Friday.", "order_id": "42"}"#),
        completed(),
    ]));
    h.engine.start_turn("status?".into()).await;
    let signals = until_done(&mut h, 0).await;
    assert_eq!(
        h.speech.spoken(),
        vec!["It shipped Monday.", "It arrives Friday."]
    );
    let output = signals
        .iter()
        .find_map(|s| match s {
            AgentSignal::Output { output, .. } => Some(output.clone()),
            _ => None,
        })
        .expect("the JSON reaches the client");
    assert_eq!(output["order_id"], "42");
}

#[tokio::test(start_paused = true)]
async fn text_mode_streams_text_and_speaks_nothing() {
    let mut h = harness(entry(json!({})), None, true);
    h.backend.push(Ok(vec![
        created("resp_1"),
        delta("Hello "),
        delta("**there**."),
        completed(),
    ]));
    h.engine.start_turn("hi".into()).await;
    let signals = until_done(&mut h, 0).await;
    assert!(h.speech.spoken().is_empty());
    let deltas: Vec<_> = signals
        .iter()
        .filter_map(|s| match s {
            AgentSignal::Transcript { delta, .. } => Some(delta.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(deltas, vec!["Hello ", "**there**."]);
}

#[tokio::test(start_paused = true)]
async fn a_slow_tool_gets_its_status_phrase() {
    let mut h = harness(
        entry(json!({
            "fillers": {"tool_call_after_ms": 1000, "slow_response_after_ms": 0, "messages": ["One moment."], "use_tool_status_messages": true},
            "tool_status_messages": {"orders__lookup_order": ["Let me look that up."]},
        })),
        None,
        false,
    );
    h.backend.push(Ok(vec![
        created("resp_1"),
        Step::Ev(AgentEvent::ToolStarted {
            item_id: "mcp_1".into(),
            name: "lookup_order".into(),
            kind: "mcp_call".into(),
        }),
        Step::Wait(Duration::from_secs(3)),
        Step::Ev(AgentEvent::ToolFinished {
            item_id: "mcp_1".into(),
            name: "lookup_order".into(),
            ok: true,
        }),
        delta("Found it."),
        completed(),
    ]));
    h.engine.start_turn("order?".into()).await;
    let signals = until_done(&mut h, 0).await;
    assert_eq!(h.speech.spoken(), vec!["Let me look that up.", "Found it."]);
    let statuses: Vec<_> = signals
        .iter()
        .filter_map(|s| match s {
            AgentSignal::Tool { status, .. } => Some(*status),
            _ => None,
        })
        .collect();
    assert_eq!(statuses, vec!["in_progress", "completed"]);
}

#[tokio::test(start_paused = true)]
async fn a_slow_first_answer_gets_a_filler_once() {
    let mut h = harness(
        entry(
            json!({"fillers": {"tool_call_after_ms": 0, "slow_response_after_ms": 1500, "messages": ["Hmm, one second."]}}),
        ),
        None,
        false,
    );
    h.backend.push(Ok(vec![
        created("resp_1"),
        Step::Wait(Duration::from_secs(4)),
        delta("Here."),
        completed(),
    ]));
    h.engine.start_turn("think".into()).await;
    let _ = until_done(&mut h, 0).await;
    assert_eq!(h.speech.spoken(), vec!["Hmm, one second.", "Here."]);
}

fn ordered_fillers(follow_up_after_ms: u64) -> Arc<VoiceAgentEntry> {
    fillers_with_tone(follow_up_after_ms, false)
}

/// The phrase cases above describe an agent with the tool-call tone switched off.
fn fillers_with_tone(follow_up_after_ms: u64, tone: bool) -> Arc<VoiceAgentEntry> {
    entry(json!({"fillers": {
        "tool_call_after_ms": 1000, "slow_response_after_ms": 1000,
        "follow_up_after_ms": follow_up_after_ms,
        "messages": ["Hmm.", "One moment.", "Still checking."],
        "use_tool_status_messages": true,
        "tool_call_sound": tone,
    }, "tool_status_messages": {"orders__lookup_order": ["Let me look that up."]}}))
}

fn tool_run(run: Duration) -> Vec<Step> {
    vec![
        created("resp_1"),
        Step::Ev(AgentEvent::ToolStarted {
            item_id: "mcp_1".into(),
            name: "lookup_order".into(),
            kind: "mcp_call".into(),
        }),
        Step::Wait(run),
        Step::Ev(AgentEvent::ToolFinished {
            item_id: "mcp_1".into(),
            name: "lookup_order".into(),
            ok: true,
        }),
        delta("Found it."),
        completed(),
    ]
}

// ---------------------------------------------------------------------------------------------
// The tool-call tone
// ---------------------------------------------------------------------------------------------

/// The tool's phrase first, then the tone until the tool finishes: no further phrase in between,
/// and the tone starts only once the phrase has played out.
#[tokio::test(start_paused = true)]
async fn the_tone_follows_the_tool_phrase_until_the_tool_finishes() {
    let mut h = harness(fillers_with_tone(2000, true), None, false);
    h.speech.paced.store(true, Ordering::Release);
    h.backend.push(Ok(tool_run(Duration::from_millis(5500))));
    let start = tokio::time::Instant::now();
    h.engine.start_turn("order?".into()).await;
    let _ = until_done(&mut h, 0).await;
    assert_eq!(h.speech.spoken(), vec!["Let me look that up.", "Found it."]);
    assert_eq!(
        h.speech.timeline(),
        vec!["say:Let me look that up.", "tone", "say:Found it."]
    );
    // The phrase is due at 1 s and plays for 400 ms; the tool returns at 5.5 s.
    let pulses: Vec<Duration> = h.speech.pulses.lock().iter().map(|t| *t - start).collect();
    assert_eq!(
        pulses.len(),
        3,
        "a pulse every period while the tool runs: {pulses:?}"
    );
    assert!(
        pulses[0] >= Duration::from_millis(1_400) + super::TONE_GAP,
        "after the phrase, and a pause: {pulses:?}"
    );
    assert!(
        pulses[2] < Duration::from_millis(5_500),
        "not after the tool: {pulses:?}"
    );
    for w in pulses.windows(2) {
        assert!(
            w[1] - w[0] >= Duration::from_millis(super::tone::PERIOD_MS),
            "{pulses:?}"
        );
    }
}

/// Live, an agent's tools return in under a second while its model thinks for seconds before and
/// after each one: the caller waits on the agent's work, not on the tool. The phrase and the tone
/// cover that work, from the tool call until the agent speaks again.
#[tokio::test(start_paused = true)]
async fn the_tone_covers_the_agent_thinking_after_a_quick_tool() {
    let mut h = harness(fillers_with_tone(2000, true), None, false);
    h.speech.paced.store(true, Ordering::Release);
    h.backend.push(Ok(vec![
        created("resp_1"),
        Step::Ev(AgentEvent::ToolStarted {
            item_id: "mcp_1".into(),
            name: "lookup_order".into(),
            kind: "mcp_call".into(),
        }),
        Step::Wait(Duration::from_millis(300)),
        Step::Ev(AgentEvent::ToolFinished {
            item_id: "mcp_1".into(),
            name: "lookup_order".into(),
            ok: true,
        }),
        Step::Wait(Duration::from_millis(6_000)),
        delta("Found it."),
        completed(),
    ]));
    let start = tokio::time::Instant::now();
    h.engine.start_turn("order?".into()).await;
    let _ = until_done(&mut h, 0).await;
    assert_eq!(h.speech.spoken(), vec!["Let me look that up.", "Found it."]);
    assert_eq!(
        h.speech.timeline(),
        vec!["say:Let me look that up.", "tone", "say:Found it."]
    );
    let pulses: Vec<Duration> = h.speech.pulses.lock().iter().map(|t| *t - start).collect();
    assert!(pulses.len() >= 3, "the tone fills the thinking: {pulses:?}");
    assert!(
        pulses.iter().all(|p| *p < Duration::from_millis(6_300)),
        "{pulses:?}"
    );
}

/// Once the agent speaks again, the tone is over: a pause later in the answer is not tool work.
#[tokio::test(start_paused = true)]
async fn the_tone_ends_when_the_agent_speaks_again() {
    let mut h = harness(fillers_with_tone(2000, true), None, false);
    h.speech.paced.store(true, Ordering::Release);
    h.backend.push(Ok(vec![
        created("resp_1"),
        Step::Ev(AgentEvent::ToolStarted {
            item_id: "mcp_1".into(),
            name: "lookup_order".into(),
            kind: "mcp_call".into(),
        }),
        Step::Wait(Duration::from_millis(300)),
        Step::Ev(AgentEvent::ToolFinished {
            item_id: "mcp_1".into(),
            name: "lookup_order".into(),
            ok: true,
        }),
        Step::Wait(Duration::from_millis(4_000)),
        delta("I found your order. "),
        delta("It shipped on Monday"),
        Step::Wait(Duration::from_millis(6_000)),
        delta(", and it arrives on Friday."),
        completed(),
    ]));
    h.engine.start_turn("order?".into()).await;
    let _ = until_done(&mut h, 0).await;
    let timeline = h.speech.timeline();
    let said = timeline
        .iter()
        .position(|e| e == "say:I found your order.")
        .expect("the answer was spoken");
    assert!(
        !timeline[said..].iter().any(|e| e == "tone"),
        "no tone after the agent spoke again: {timeline:?}"
    );
    assert!(timeline[..said].iter().any(|e| e == "tone"), "{timeline:?}");
}

/// A caller who talks over the tone stops it at once; nothing more of it is played.
#[tokio::test(start_paused = true)]
async fn a_barge_in_stops_the_tone() {
    let h = harness(fillers_with_tone(2000, true), None, false);
    h.speech.paced.store(true, Ordering::Release);
    h.backend.push(Ok(tool_run(Duration::from_millis(20_000))));
    h.engine.start_turn("order?".into()).await;
    tokio::time::sleep(Duration::from_millis(3_000)).await;
    let before = h.speech.pulses.lock().len();
    assert!(before > 0, "the tone was playing");
    h.engine.barge_in().await;
    tokio::time::sleep(Duration::from_millis(5_000)).await;
    assert_eq!(h.speech.pulses.lock().len(), before);
}

/// With the tone off, the phrases keep coming while the tool runs, as before.
#[tokio::test(start_paused = true)]
async fn with_the_tone_off_the_phrases_keep_coming() {
    let mut h = harness(fillers_with_tone(2000, false), None, false);
    h.speech.paced.store(true, Ordering::Release);
    h.backend.push(Ok(tool_run(Duration::from_millis(5500))));
    h.engine.start_turn("order?".into()).await;
    let _ = until_done(&mut h, 0).await;
    assert_eq!(
        h.speech.spoken(),
        vec![
            "Let me look that up.",
            "One moment.",
            "Still checking.",
            "Found it."
        ]
    );
    assert!(h.speech.pulses.lock().is_empty());
}

/// A session whose audio output cannot carry the tone (a compressed TTS format) keeps the phrases,
/// so the caller is never left in silence.
#[tokio::test(start_paused = true)]
async fn an_output_that_cannot_carry_the_tone_keeps_the_phrases() {
    let mut h = harness(fillers_with_tone(2000, true), None, false);
    h.speech.paced.store(true, Ordering::Release);
    h.speech.no_sound.store(true, Ordering::Release);
    h.backend.push(Ok(tool_run(Duration::from_millis(5500))));
    h.engine.start_turn("order?".into()).await;
    let _ = until_done(&mut h, 0).await;
    assert_eq!(
        h.speech.spoken(),
        vec![
            "Let me look that up.",
            "One moment.",
            "Still checking.",
            "Found it."
        ]
    );
    assert!(h.speech.pulses.lock().is_empty());
}

/// The tone is for tools: a slow answer with no tool keeps its phrases and plays no tone.
#[tokio::test(start_paused = true)]
async fn a_slow_answer_without_a_tool_plays_no_tone() {
    let mut h = harness(fillers_with_tone(2000, true), None, false);
    h.speech.paced.store(true, Ordering::Release);
    h.backend.push(Ok(vec![
        created("resp_1"),
        Step::Wait(Duration::from_millis(3500)),
        delta("Here it is."),
        completed(),
    ]));
    h.engine.start_turn("order?".into()).await;
    let _ = until_done(&mut h, 0).await;
    assert_eq!(
        h.speech.spoken(),
        vec!["Hmm.", "One moment.", "Here it is."]
    );
    assert!(h.speech.pulses.lock().is_empty());
}

/// A text-only session is never spoken to, tone included.
#[tokio::test(start_paused = true)]
async fn a_text_only_session_plays_no_tone() {
    let mut h = harness(fillers_with_tone(2000, true), None, true);
    h.speech.paced.store(true, Ordering::Release);
    h.backend.push(Ok(tool_run(Duration::from_millis(5500))));
    h.engine.start_turn("order?".into()).await;
    let _ = until_done(&mut h, 0).await;
    assert!(h.speech.pulses.lock().is_empty());
}

/// The list is a script for one wait: the phrase heard says how long the caller has waited.
#[tokio::test(start_paused = true)]
async fn fillers_follow_the_list_in_order_while_the_wait_lasts() {
    let mut h = harness(ordered_fillers(2000), None, false);
    h.backend.push(Ok(vec![
        created("resp_1"),
        // 1 s "Hmm.", 3 s "One moment.", 5 s "Still checking.", then the list is used up.
        Step::Wait(Duration::from_secs(12)),
        delta("Here."),
        completed(),
    ]));
    h.engine.start_turn("think".into()).await;
    let _ = until_done(&mut h, 0).await;
    assert_eq!(
        h.speech.spoken(),
        vec!["Hmm.", "One moment.", "Still checking.", "Here."]
    );
}

/// budprompt can take seconds to answer at all: the wait for the stream to open is the caller's
/// wait too, and is filled on the same schedule (live: the first filler came at 5.3 s, not 0.5 s).
#[tokio::test(start_paused = true)]
async fn fillers_cover_the_wait_for_the_stream_to_open() {
    let mut h = harness(ordered_fillers(2000), None, false);
    *h.backend.open_delay.lock() = Duration::from_millis(3500);
    h.backend
        .push(Ok(vec![created("resp_1"), delta("Here."), completed()]));
    h.engine.start_turn("think".into()).await;
    let _ = until_done(&mut h, 0).await;
    assert_eq!(h.speech.spoken(), vec!["Hmm.", "One moment.", "Here."]);
}

#[tokio::test(start_paused = true)]
async fn every_turn_starts_again_from_the_first_filler() {
    let mut h = harness(ordered_fillers(2000), None, false);
    h.backend.push(Ok(vec![
        created("resp_1"),
        Step::Wait(Duration::from_millis(3500)),
        delta("First."),
        completed(),
    ]));
    h.backend.push(Ok(vec![
        created("resp_2"),
        Step::Wait(Duration::from_millis(1500)),
        delta("Second."),
        completed(),
    ]));
    h.engine.start_turn("one".into()).await;
    let _ = until_done(&mut h, 0).await;
    h.engine.start_turn("two".into()).await;
    let _ = until_done(&mut h, 1).await;
    assert_eq!(
        h.speech.spoken(),
        vec!["Hmm.", "One moment.", "First.", "Hmm.", "Second."]
    );
}

#[tokio::test(start_paused = true)]
async fn no_follow_up_means_one_filler_per_wait() {
    let mut h = harness(ordered_fillers(0), None, false);
    h.backend.push(Ok(vec![
        created("resp_1"),
        Step::Wait(Duration::from_secs(8)),
        delta("Here."),
        completed(),
    ]));
    h.engine.start_turn("think".into()).await;
    let _ = until_done(&mut h, 0).await;
    assert_eq!(h.speech.spoken(), vec!["Hmm.", "Here."]);
}

#[tokio::test(start_paused = true)]
async fn fillers_stop_once_the_reply_is_speaking() {
    let mut h = harness(ordered_fillers(2000), None, false);
    h.backend.push(Ok(vec![
        created("resp_1"),
        Step::Wait(Duration::from_millis(1500)),
        // The first sentence is spoken once the next one begins; the model then stalls mid-reply.
        delta("Let me think about that. Right,"),
        Step::Wait(Duration::from_secs(8)),
        delta(" done."),
        completed(),
    ]));
    h.engine.start_turn("think".into()).await;
    let _ = until_done(&mut h, 0).await;
    assert_eq!(
        h.speech.spoken(),
        vec!["Hmm.", "Let me think about that.", "Right, done."]
    );
}

/// The gap to the next phrase runs from when a filler was due, not from when its synthesis
/// returned, whether it was the tool's own phrase or the list's.
#[tokio::test(start_paused = true)]
async fn the_follow_up_gap_runs_from_when_the_tool_phrase_was_due() {
    let mut h = harness(ordered_fillers(2000), None, false);
    h.speech.synth_ms.store(1500, Ordering::Release);
    h.backend.push(Ok(vec![
        created("resp_1"),
        Step::Ev(AgentEvent::ToolStarted {
            item_id: "mcp_1".into(),
            name: "lookup_order".into(),
            kind: "mcp_call".into(),
        }),
        // The tool's phrase is due at 1 s and synthesized by 2.5 s; the next is due at 3 s.
        Step::Wait(Duration::from_millis(3500)),
        Step::Ev(AgentEvent::ToolFinished {
            item_id: "mcp_1".into(),
            name: "lookup_order".into(),
            ok: true,
        }),
        delta("Found it."),
        completed(),
    ]));
    h.engine.start_turn("order?".into()).await;
    let _ = until_done(&mut h, 0).await;
    assert_eq!(
        h.speech.spoken(),
        vec!["Let me look that up.", "One moment.", "Found it."]
    );
}

/// What the model says before calling a tool ("Let me check.") is a finished thought: it is spoken
/// as the tool starts, not held until the tool returns and heard after the fillers.
#[tokio::test(start_paused = true)]
async fn a_lead_in_before_a_tool_is_spoken_before_the_tool_runs() {
    let mut h = harness(ordered_fillers(2000), None, false);
    h.backend.push(Ok(vec![
        created("resp_1"),
        delta("Let me check that."),
        Step::Ev(AgentEvent::ToolStarted {
            item_id: "mcp_1".into(),
            name: "lookup_order".into(),
            kind: "mcp_call".into(),
        }),
        Step::Wait(Duration::from_millis(5500)),
        Step::Ev(AgentEvent::ToolFinished {
            item_id: "mcp_1".into(),
            name: "lookup_order".into(),
            ok: true,
        }),
        delta("Found it."),
        completed(),
    ]));
    h.engine.start_turn("order?".into()).await;
    let _ = until_done(&mut h, 0).await;
    assert_eq!(
        h.speech.spoken(),
        vec![
            "Let me check that.",
            "Let me look that up.",
            "One moment.",
            "Still checking.",
            "Found it."
        ]
    );
}

/// A tool's own phrase takes the slot the list's next phrase would have had, so what follows
/// still matches how long the caller has waited; the follow-ups keep going while the tool runs.
#[tokio::test(start_paused = true)]
async fn a_tool_phrase_takes_its_place_in_the_list() {
    let mut h = harness(ordered_fillers(2000), None, false);
    h.backend.push(Ok(vec![
        created("resp_1"),
        Step::Ev(AgentEvent::ToolStarted {
            item_id: "mcp_1".into(),
            name: "lookup_order".into(),
            kind: "mcp_call".into(),
        }),
        // 1 s the tool's phrase (slot 1), 3 s "One moment.", 5 s "Still checking."
        Step::Wait(Duration::from_millis(5500)),
        Step::Ev(AgentEvent::ToolFinished {
            item_id: "mcp_1".into(),
            name: "lookup_order".into(),
            ok: true,
        }),
        delta("Found it."),
        completed(),
    ]));
    h.engine.start_turn("order?".into()).await;
    let _ = until_done(&mut h, 0).await;
    assert_eq!(
        h.speech.spoken(),
        vec![
            "Let me look that up.",
            "One moment.",
            "Still checking.",
            "Found it."
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn the_greeting_is_spoken_but_never_sent_to_the_agent() {
    let mut h = harness(
        entry(
            json!({"greeting": {"mode": "static", "text": "Hi, I'm the support agent.", "interruptible": false}}),
        ),
        None,
        false,
    );
    h.engine.greet().await;
    let signals = until_done(&mut h, 0).await;
    assert!(matches!(
        signals[0],
        AgentSignal::ResponseStarted {
            kind: TurnKind::Greeting,
            input: None,
            ..
        }
    ));
    assert_eq!(
        h.speech.spoken.lock()[0],
        ("Hi, I'm the support agent.".to_string(), false)
    );
    assert!(h.backend.requests.lock().is_empty());
}

#[tokio::test(start_paused = true)]
async fn an_exact_client_truncate_overrides_the_estimate() {
    let mut h = harness(entry(json!({})), None, false);
    h.backend.push(Ok(vec![
        created("resp_1"),
        delta("First sentence here. Second sentence follows. Third one ends it."),
        completed(),
    ]));
    h.engine.start_turn("talk".into()).await;
    let _ = until_done(&mut h, 0).await;
    // The GA client played exactly the first sentence (20 chars at 50 chars/s = 400 ms).
    h.engine.rate.lock().observe(0, 0); // keep the default rate
    let cpms = h.engine.rate.lock().chars_per_ms();
    h.engine.truncate_at((20.0 / cpms) as u64);
    let s = h.rx.recv().await.unwrap();
    assert!(
        matches!(s, AgentSignal::Truncated { ref spoken, .. } if spoken == "First sentence here."),
        "{s:?}"
    );
}

#[test]
fn backchannels_are_recognised() {
    let phrases = vec!["uh-huh".to_string(), "okay".to_string(), "yeah".to_string()];
    assert!(is_ignored_phrase("Uh-huh.", &phrases));
    assert!(is_ignored_phrase("  okay ", &phrases));
    assert!(!is_ignored_phrase("okay stop", &phrases));
    assert!(!is_ignored_phrase("", &phrases));
}

/// A bug that panics the turn task must not wedge the session: the caller hears the degradation
/// message and gets an error, the turn ends `failed` (so the client sees `response.done`), the run
/// is cancelled at budprompt, and the next turn runs. Live: a UTF-8 slice panic on "Hi, I’m" left
/// a session silent until the caller hung up.
#[tokio::test(start_paused = true)]
async fn a_panicking_turn_fails_cleanly_and_the_session_continues() {
    let mut h = harness(entry(json!({})), None, false);
    h.backend.push(Ok(vec![
        created("resp_1"),
        delta("Hello there, friend"),
        Step::Panic,
    ]));
    h.backend.push(Ok(vec![
        created("resp_2"),
        delta("Still here."),
        completed(),
    ]));
    h.engine.start_turn("first".into()).await;
    let signals = until_done(&mut h, 0).await;
    let (status, rid, ..) = done_of(&signals);
    assert_eq!(status, TurnStatus::Failed);
    assert_eq!(rid.as_deref(), Some("resp_1"));
    assert!(
        signals
            .iter()
            .any(|s| matches!(s, AgentSignal::Error { code, .. } if *code == "internal_error")),
        "the caller is told: {signals:?}"
    );
    assert!(
        h.speech
            .spoken()
            .iter()
            .any(|t| t.contains("having trouble")),
        "the degradation message is spoken: {:?}",
        h.speech.spoken()
    );
    assert!(!h.engine.has_active_turn(), "the session is not wedged");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(h.backend.cancels.lock().clone(), vec!["resp_1".to_string()]);

    h.engine.start_turn("second".into()).await;
    let signals = until_done(&mut h, 1).await;
    assert_eq!(done_of(&signals).0, TurnStatus::Completed);
}

/// budprompt streams `response.created` then `response.in_progress` for the same run; both map to
/// `Created`, and the client must hear about the run once (live: `/ws` sent
/// `agent_response_created` twice).
#[tokio::test(start_paused = true)]
async fn a_run_is_announced_once_even_when_its_lifecycle_repeats() {
    let mut h = harness(entry(json!({})), None, false);
    h.backend.push(Ok(vec![
        created("resp_1"),
        created("resp_1"),
        delta("Four."),
        completed(),
    ]));
    h.engine.start_turn("two plus two".into()).await;
    let signals = until_done(&mut h, 0).await;
    let announced = signals
        .iter()
        .filter(|s| matches!(s, AgentSignal::ResponseCreated { .. }))
        .count();
    assert_eq!(announced, 1, "{signals:?}");
    assert_eq!(done_of(&signals).0, TurnStatus::Completed);
}

/// A turn whose request never reached the agent (budgateway could not send it to budprompt — live,
/// right after a barge-in cancelled the previous turn) is retried before the caller hears the
/// degradation message: no run exists yet, so nothing can be duplicated. LiveKit Agents retries an
/// LLM call that produced no output the same way.
#[tokio::test(start_paused = true)]
async fn a_request_that_never_reached_the_agent_is_retried() {
    let mut h = harness(entry(json!({})), None, false);
    h.backend.push(Err(TurnFailure::Transport(
        "Error sending request: error sending request for url (http://budprompt/v1/responses)"
            .into(),
    )));
    h.backend
        .push(Ok(vec![created("resp_1"), delta("Four."), completed()]));
    h.engine.start_turn("two plus two".into()).await;
    let signals = until_done(&mut h, 0).await;
    assert_eq!(done_of(&signals).0, TurnStatus::Completed);
    assert_eq!(h.backend.requests.lock().len(), 2, "one retry");
    assert!(
        !signals
            .iter()
            .any(|s| matches!(s, AgentSignal::Error { .. })),
        "a recovered turn is not an error: {signals:?}"
    );
    assert_eq!(h.speech.spoken(), vec!["Four."]);
}

#[tokio::test(start_paused = true)]
async fn retries_are_bounded_and_end_in_the_degradation_message() {
    let mut h = harness(entry(json!({})), None, false);
    for _ in 0..4 {
        h.backend
            .push(Err(TurnFailure::Transport("connection refused".into())));
    }
    h.engine.start_turn("hello".into()).await;
    let signals = until_done(&mut h, 0).await;
    assert_eq!(done_of(&signals).0, TurnStatus::Failed);
    assert_eq!(
        h.backend.requests.lock().len(),
        3,
        "one try and two retries"
    );
    assert_eq!(
        h.speech
            .spoken()
            .iter()
            .filter(|t| t.contains("having trouble"))
            .count(),
        1
    );
}

#[tokio::test(start_paused = true)]
async fn a_refusal_is_not_retried() {
    let mut h = harness(entry(json!({})), None, false);
    h.backend
        .push(Err(TurnFailure::RateLimited("slow down".into())));
    h.engine.start_turn("hello".into()).await;
    let signals = until_done(&mut h, 0).await;
    assert_eq!(done_of(&signals).0, TurnStatus::Failed);
    assert_eq!(h.backend.requests.lock().len(), 1);
}
