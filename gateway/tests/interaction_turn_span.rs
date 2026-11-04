//! Interaction mode: a background run is the turn budmetrics records. Its exported `voice.turn`
//! span carries what the plain paths record for a turn: for the LLM tier the session, the
//! caller's attribution (Bud mode) and the LLM leg, as a conversation turn does; for a Bud
//! agent the agent, its project and budprompt's run, as an agent turn does.
//!
//! A caller turn of interaction mode is one trace (spec 029 D-9): its record is the root, the
//! runs it starts are children in that trace, and budmetrics folds a trace into one row.
//!
//! Its own test binary with one test, for the reason `agent_turn_span.rs` gives: tracing
//! caches callsite interest process-wide, so a parallel test without a subscriber could leave
//! the span disabled for this one.

#[path = "mock_providers/interaction.rs"]
mod ix_mocks;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::{self, BoxStream};
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::trace::{SdkTracerProvider, SpanData, SpanExporter};
use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::layer::SubscriberExt;

use ix_mocks::{LlmDouble, Round};
use waav_gateway::auth::SessionCredential;
use waav_gateway::core::agent::{
    AgentEvent, AgentSessionConfig, AgentTurnRequest, TurnBackend, TurnFailure,
};
use waav_gateway::core::conversation::{ConversationConfig, TurnAttribution, resolve_llm_legs};
use waav_gateway::core::interaction::actor::Msg;
use waav_gateway::core::interaction::background::{Backend, RunSpec, spawn_run};
use waav_gateway::core::interaction::bud_agent::{AgentBackground, AgentRun, spawn_agent_run};
use waav_gateway::core::interaction::turn_record::TurnRecord;
use waav_gateway::core::interaction::types::{Act, RunEvent, RunId, TurnId};

#[derive(Debug, Clone, Default)]
struct Collect(Arc<Mutex<Vec<SpanData>>>);

impl SpanExporter for Collect {
    fn export(
        &self,
        batch: Vec<SpanData>,
    ) -> impl std::future::Future<Output = OTelSdkResult> + Send {
        self.0.lock().unwrap().extend(batch);
        std::future::ready(Ok(()))
    }
}

/// A Bud gateway whose agent answers in one sentence, and the traceparent it was sent.
#[derive(Default)]
struct Answering(Mutex<Option<String>>);

#[async_trait]
impl TurnBackend for Answering {
    async fn open(
        &self,
        _request: &AgentTurnRequest,
        _bearer: &str,
        traceparent: Option<&str>,
    ) -> Result<BoxStream<'static, Result<AgentEvent, TurnFailure>>, TurnFailure> {
        *self.0.lock().unwrap() = traceparent.map(str::to_owned);
        let events = [
            AgentEvent::Created {
                response_id: "resp_1".to_owned(),
                conversation_id: None,
            },
            AgentEvent::TextDelta {
                item_id: None,
                delta: "It shipped on Monday.".to_owned(),
            },
            AgentEvent::Completed {
                usage: None,
                output: Vec::new(),
            },
        ];
        Ok(stream::iter(events.map(Ok)).boxed())
    }

    async fn cancel(&self, _response_id: &str, _prompt_name: &str, _bearer: &str) {}
}

/// Waits for the run's last report; the test fails unless it finished.
async fn finished(rx: &mut mpsc::Receiver<Msg>) {
    loop {
        let report = tokio::time::timeout(Duration::from_secs(20), rx.recv()).await;
        match report.expect("the run reports") {
            Some(Msg::Run(_, RunEvent::Finished { .. })) => return,
            // The first token (the health watcher's deadline, chapter 10) and the sentences.
            Some(Msg::RunAlive(_) | Msg::Run(_, RunEvent::Sentence { .. })) => {}
            other => panic!("the run did not finish: {other:?}"),
        }
    }
}

fn attributes(span: &SpanData) -> HashMap<String, String> {
    let pairs = span.attributes.iter();
    pairs
        .map(|kv| (kv.key.to_string(), kv.value.to_string()))
        .collect()
}

/// The exported `voice.turn` spans of a session.
fn session_turns<'a>(spans: &'a [SpanData], session: &str) -> Vec<&'a SpanData> {
    spans
        .iter()
        .filter(|span| span.name == "voice.turn")
        .filter(|span| {
            attributes(span)
                .get("bud.voice.session_id")
                .map(String::as_str)
                == Some(session)
        })
        .collect()
}

/// The attributes of the one exported `voice.turn` span of this capability in `session`.
fn turn_span(spans: &[SpanData], capability: &str, session: &str) -> HashMap<String, String> {
    let turns: Vec<HashMap<String, String>> = session_turns(spans, session)
        .into_iter()
        .map(attributes)
        .filter(|found| found.get("bud.voice.capability").map(String::as_str) == Some(capability))
        .collect();
    assert_eq!(turns.len(), 1, "one {capability} turn: {turns:?}");
    turns.into_iter().next().unwrap()
}

#[test]
fn a_background_run_is_recorded_as_the_turn_of_its_plain_path() {
    let collect = Collect::default();
    let provider = SdkTracerProvider::builder()
        .with_simple_exporter(collect.clone())
        .build();
    let layer = tracing_opentelemetry::layer().with_tracer(provider.tracer("interaction-turn"));
    tracing::subscriber::set_global_default(tracing_subscriber::registry().with(layer)).unwrap();
    // The LLM double listens on loopback.
    unsafe {
        std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", "1");
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let backend = Arc::new(Answering::default());
    let sent_2 = runtime.block_on(async {
        // 1. An LLM-tier run of a Bud-mode conversation session.
        let llm = LlmDouble::start().await;
        llm.push(Round::text(50, 10, &["The store ", "opens at nine."]));
        let conversation = ConversationConfig {
            base_url: llm.base_url.clone(),
            model: "chat-deployment".to_owned(),
            api_key: Some("test-key".to_owned()),
            streaming: true,
            ..Default::default()
        };
        let fast = resolve_llm_legs(&conversation).expect("llm legs").llm;
        let (tx, mut rx) = mpsc::channel(16);
        let spec = RunSpec {
            run: RunId(3),
            history_key: "session-1#r3".to_owned(),
            llm: fast,
            api_key: None,
            system_prompt: "You answer callers.".to_owned(),
            operator_prompt: "You answer callers.".to_owned(),
            history: Vec::new(),
            words: vec!["when do you open".to_owned()],
            tools: Vec::new(),
            agents: Vec::new(),
            tool_secret: None,
            session_id: "session-1".to_owned(),
            attribution: Some(TurnAttribution {
                project_id: Some("proj-1".to_owned()),
                api_key_id: Some("key-1".to_owned()),
                api_key_project_id: Some("proj-1".to_owned()),
                user_id: Some("user-1".to_owned()),
                endpoint_name: Some("chat-deployment".to_owned()),
            }),
            trace_id: String::new(),
            reply_in: None,
            state: None,
            turn: None,
        };
        spawn_run(spec, tx, CancellationToken::new());
        finished(&mut rx).await;

        // 2. A run of a Bud agent.
        let entry = json!({
            "prompt_id": "c0de", "version": 2, "project_id": "proj-agent",
            "stt": {"endpoint_id": "s"}, "tts": {"endpoint_id": "t"},
        });
        let entry = bud_auth::parse_voice_agent_blob(&entry.to_string()).unwrap();
        let session = AgentSessionConfig {
            session_id: "session-2".to_owned(),
            prompt_name: "support".to_owned(),
            version: 2,
            entry: Arc::new(entry),
            conversation_id: "conv_voice_session-2".to_owned(),
            variables: None,
            text_only: false,
        };
        let credential = SessionCredential::new("bud-caller-key");
        let brain = Arc::clone(&backend) as Arc<dyn TurnBackend>;
        let agent = AgentBackground::new(brain, session, credential, None);
        let (tx, mut rx) = mpsc::channel(16);
        let spec = AgentRun {
            run: RunId(5),
            agent,
            words: vec!["where is my order".to_owned()],
            caller: "where is my order".to_owned(),
            turn: None,
        };
        spawn_agent_run(spec, tx, CancellationToken::new());
        finished(&mut rx).await;
        let sent_2 = backend.0.lock().unwrap().clone();

        // 3. Interaction mode, a Bud agent session: caller turn 7 starts a run; turn 8, a
        // "thanks", is the front's alone.
        let session = AgentSessionConfig {
            session_id: "session-3".to_owned(),
            prompt_name: "support".to_owned(),
            version: 2,
            entry: Arc::new(
                bud_auth::parse_voice_agent_blob(
                    &json!({
                        "prompt_id": "c0de", "version": 2, "project_id": "proj-agent",
                        "stt": {"endpoint_id": "s"}, "tts": {"endpoint_id": "t"},
                    })
                    .to_string(),
                )
                .unwrap(),
            ),
            conversation_id: "conv_voice_session-3".to_owned(),
            variables: None,
            text_only: false,
        };
        let credential = SessionCredential::new("bud-caller-key");
        let brain = Arc::clone(&backend) as Arc<dyn TurnBackend>;
        let agent = AgentBackground::new(brain, session, credential, None);
        let span = agent.turn_span("session-3", 7, "where is my order");
        let mut turn = TurnRecord::open(span, TurnId(7), 40_000);
        turn.settled(Act::Delegate);
        turn.task("t1");
        let (tx, mut rx) = mpsc::channel(16);
        let spec = AgentRun {
            run: RunId(9),
            agent: Arc::clone(&agent),
            words: vec!["where is my order".to_owned()],
            caller: "where is my order".to_owned(),
            turn: Some(turn.trace()),
        };
        spawn_agent_run(spec, tx, CancellationToken::new());
        finished(&mut rx).await;
        turn.audible(40_900, false, true);
        turn.audible(42_400, true, false);
        drop(turn);
        let thanks = TurnRecord::open(agent.turn_span("session-3", 8, "thanks"), TurnId(8), 50_000);
        thanks.settled(Act::Social);
        drop(thanks);
        // Each run drops its span right after its last report.
        tokio::time::sleep(Duration::from_millis(200)).await;
        sent_2
    });

    let spans = collect.0.lock().unwrap().clone();
    let has = |turn: &HashMap<String, String>, key: &str, value: &str| {
        assert_eq!(
            turn.get(key).map(String::as_str),
            Some(value),
            "{key}: {turn:?}"
        );
    };

    // What `ConversationOrchestrator::run_turn` records for a turn of the same session.
    let turn = turn_span(&spans, "conversation", "session-1");
    has(&turn, "bud.voice.transport", "websocket");
    has(&turn, "bud.voice.session_id", "session-1");
    has(&turn, "bud.voice.turn_index", "3");
    has(&turn, "bud.voice.transcript", "when do you open");
    has(&turn, "bud.project_id", "proj-1");
    has(&turn, "bud.api_key_id", "key-1");
    has(&turn, "bud.api_key_project_id", "proj-1");
    has(&turn, "bud.user_id", "user-1");
    has(&turn, "bud.voice.endpoint_name", "chat-deployment");
    has(&turn, "gen_ai.request.model", "chat-deployment");
    assert!(turn.contains_key("bud.voice.llm.duration_ms"), "{turn:?}");

    // What `AgentEngine::run_turn` records for a turn of the same agent. budgateway
    // attributes the run to the caller from the credential; the span is the root of the trace
    // budgateway and budprompt continue.
    let turn = turn_span(&spans, "agent_turn", "session-2");
    has(&turn, "bud.voice.transport", "websocket");
    has(&turn, "bud.voice.session_id", "session-2");
    has(&turn, "bud.voice.turn_index", "5");
    has(&turn, "bud.voice.endpoint_name", "support");
    has(&turn, "bud.project_id", "proj-agent");
    let exported = session_turns(&spans, "session-2").into_iter().next();
    let context = &exported.expect("the agent turn").span_context;
    let sent = sent_2.expect("a traceparent");
    assert!(
        sent.contains(&context.trace_id().to_string())
            && sent.contains(&context.span_id().to_string()),
        "the run continues the turn's trace: {sent}"
    );
    has(&turn, "bud.voice.rt.component", "agent");
    has(&turn, "bud.voice.rt.response_id", "resp_1");
    has(&turn, "bud.voice.rt.response_status", "completed");

    // Interaction mode: turn 7's record is the root of its trace and its run a child in it;
    // turn 8 is a trace of its own with no run. Two traces, two rows.
    let turns = session_turns(&spans, "session-3");
    assert_eq!(turns.len(), 3, "{turns:?}");
    let index = |span: &SpanData| attributes(span).get("bud.voice.turn_index").cloned();
    let seventh: Vec<&SpanData> = turns
        .iter()
        .copied()
        .filter(|span| index(span).as_deref() == Some("7"))
        .collect();
    assert_eq!(seventh.len(), 2, "the record and its run: {seventh:?}");
    let root = seventh
        .iter()
        .find(|span| span.parent_span_id == opentelemetry::trace::SpanId::INVALID);
    let root = root.expect("the turn's record is a root");
    let run = seventh
        .iter()
        .find(|span| span.parent_span_id == root.span_context.span_id());
    let run = run.expect("the run is the record's child");
    assert_eq!(run.span_context.trace_id(), root.span_context.trace_id());
    let record = attributes(root);
    has(&record, "bud.voice.capability", "agent_turn");
    has(&record, "bud.voice.endpoint_name", "support");
    has(&record, "bud.project_id", "proj-agent");
    has(&record, "bud.voice.interaction.act", "delegate");
    has(&record, "bud.voice.interaction.task_ref", "t1");
    has(&record, "bud.voice.response_latency_ms", "900");
    has(&record, "bud.voice.interaction.ack_latency_ms", "900");
    has(&record, "bud.voice.interaction.answer_latency_ms", "2400");
    has(&attributes(run), "bud.voice.rt.response_id", "resp_1");
    // The run continues the turn's trace toward budgateway and budprompt.
    let sent = backend.0.lock().unwrap().clone().expect("a traceparent");
    assert!(
        sent.contains(&root.span_context.trace_id().to_string()),
        "{sent}"
    );
    let eighth = turns
        .iter()
        .find(|span| index(span).as_deref() == Some("8"));
    let eighth = eighth.expect("the front's turn has its record");
    assert_ne!(eighth.span_context.trace_id(), root.span_context.trace_id());
    has(&attributes(eighth), "bud.voice.interaction.act", "social");
    assert!(!attributes(eighth).contains_key("bud.voice.interaction.task_ref"));
}
