//! The Bud agent as the background: one budprompt run per task revision, reached through the
//! agent engine's own client ([`TurnBackend`]) with the caller's credential.
//!
//! The gateway cannot prompt a Bud agent and does not run its tools, so three things differ
//! from the LLM tier:
//!
//! * **The answer.** Text streams to the caller only after the line the agent is declared to
//!   write before its final answer (`background.answer_marker`). Without a declared marker
//!   the text after the last tool item is released when the run completes: until then a draft
//!   and the answer look the same.
//! * **Tools.** A tool item is budprompt's. It is reported as an operation in flight while it
//!   runs and as an effect of the task when it has run; nothing of it is gated here.
//! * **One run at a time.** budprompt serialises a conversation, so a run waits for the one
//!   before it to end, and what the caller did not hear of an answer is sent with the next
//!   run as `bud_truncate`.

use std::collections::HashMap;
use std::sync::Arc;

use futures::StreamExt;
use parking_lot::Mutex;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, debug};

use crate::auth::SessionCredential;
use crate::core::agent::engine::{agent_turn_span, open_turn, record_failure};
use crate::core::agent::text::SpeakFieldExtractor;
use crate::core::agent::{
    AgentEvent, AgentSessionConfig, AgentTurnRequest, ClientExtras, Truncate, TurnBackend,
    TurnFailure,
};
use crate::observability::voice_attrs::realtime;

use super::actor::Msg;
use super::background::{AnswerGate, Backend, Job, Relay, caller_words, request_text};
use super::turn_record::TurnTrace;
use super::types::{RunEvent, RunId};

/// The agent of one session and what its runs share.
pub struct AgentBackground {
    backend: Arc<dyn TurnBackend>,
    session: AgentSessionConfig,
    credential: SessionCredential,
    /// `background.answer_marker`, when the agent's entry or the operator declares one.
    marker: Option<String>,
    /// Held by the run that is on the conversation.
    turn: tokio::sync::Mutex<()>,
    /// budprompt's id of each run that got one.
    responses: Mutex<HashMap<RunId, String>>,
    /// The answer the caller did not hear in full, for the next run to say so.
    truncate: Mutex<Option<Truncate>>,
}

impl AgentBackground {
    pub fn new(
        backend: Arc<dyn TurnBackend>,
        session: AgentSessionConfig,
        credential: SessionCredential,
        marker: Option<String>,
    ) -> Arc<Self> {
        Arc::new(Self {
            backend,
            session,
            credential,
            marker,
            turn: tokio::sync::Mutex::default(),
            responses: Mutex::default(),
            truncate: Mutex::default(),
        })
    }

    /// The agent's name, as the caller addressed it.
    pub fn name(&self) -> &str {
        &self.session.prompt_name
    }

    /// Whether a task may change something: only an agent without tools cannot.
    pub fn side_effects(&self) -> bool {
        !self.session.entry.allow_preemptive
    }

    /// What the caller has heard of the answer of `run`. `cut`: the rest of it will not be
    /// heard, so the next run tells budprompt to keep only this much in its history.
    pub fn heard(&self, run: RunId, heard: &str, cut: bool) {
        let Some(response_id) = self.responses.lock().get(&run).cloned() else {
            // budprompt never knew the run: there is nothing to shorten.
            return;
        };
        let mut owed = self.truncate.lock();
        if cut || owed.as_ref().is_some_and(|t| t.response_id == response_id) {
            *owed = Some(Truncate {
                response_id,
                output_text: heard.to_owned(),
            });
        }
    }
}

/// One run: the caller's request with its corrections.
pub struct AgentRun {
    pub run: RunId,
    pub agent: Arc<AgentBackground>,
    pub words: Vec<String>,
    /// Everything the caller said, for the outbound speech policy.
    pub caller: String,
    /// The caller turn the run belongs to (`Job::turn`).
    pub turn: Option<TurnTrace>,
}

/// Start a run. It reports through `Msg::Run(spec.run, ..)` like an LLM-tier run and stops
/// when `cancel` fires; its last report is `Finished`, `Failed` or `Cancelled`.
pub fn spawn_agent_run(
    spec: AgentRun,
    tx: mpsc::Sender<Msg>,
    cancel: CancellationToken,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let (run, agent) = (spec.run, Arc::clone(&spec.agent));
        // The run before this one may still be ending: its cancel is delivered first.
        let turn = tokio::select! {
            biased;
            _ = cancel.cancelled() => None,
            turn = agent.turn.lock() => Some(turn),
        };
        let last = match &turn {
            None => RunEvent::Cancelled,
            Some(_) => {
                let input = request_text(&spec.words);
                // A child of its caller turn's span, in the turn's trace (spec 029 D-9).
                let parent = spec.turn.as_ref().map(|turn| &turn.span);
                let index = spec.turn.as_ref().map_or(run.0, |turn| turn.index);
                let span = agent_turn_span(&agent.session, index, &input, parent);
                let driven = drive(spec, input, &span, &tx, &cancel);
                driven.instrument(span.clone()).await
            }
        };
        let cancelled = matches!(last, RunEvent::Cancelled);
        let _ = tx.send(Msg::Run(run, last)).await;
        // Dropping the stream stopped the run; the cancel records it as cancelled.
        let response = agent.responses.lock().get(&run).cloned();
        if let (true, Some(response_id)) = (cancelled, response) {
            let bearer = agent.credential.current();
            let name = agent.name();
            agent.backend.cancel(&response_id, name, &bearer).await;
        }
        drop(turn);
    })
}

impl Backend for AgentBackground {
    fn label(&self) -> String {
        format!("agent:{}", self.name())
    }

    fn own_tools(&self) -> Option<bool> {
        Some(self.side_effects())
    }

    /// What a run of the agent records: the agent, its project and the caller's words.
    fn turn_span(&self, _session_id: &str, turn: u64, input: &str) -> tracing::Span {
        agent_turn_span(&self.session, turn, input, None)
    }

    /// budprompt runs one response of a conversation at a time, and every task of the session
    /// continues its one conversation.
    fn max_concurrent(&self) -> Option<usize> {
        Some(1)
    }

    fn start(
        self: Arc<Self>,
        job: Job,
        tx: mpsc::Sender<Msg>,
        cancel: CancellationToken,
    ) -> JoinHandle<()> {
        let spec = AgentRun {
            run: job.run,
            agent: self,
            caller: caller_words(&job.history, &job.words),
            words: job.words,
            turn: job.turn,
        };
        spawn_agent_run(spec, tx, cancel)
    }

    fn heard(&self, run: RunId, text: &str, cut: bool) {
        AgentBackground::heard(self, run, text, cut);
    }
}

/// The label of a tool item in a task's effects, after "it had already".
fn label(verb: &str, tool: &str) -> String {
    format!("{verb} {}", tool.replace('_', " "))
}

fn failed(span: &tracing::Span, failure: &TurnFailure) -> RunEvent {
    record_failure(span, failure);
    RunEvent::Failed {
        error: format!("{}: {}", failure.code(), failure.message()),
    }
}

async fn drive(
    spec: AgentRun,
    input: String,
    span: &tracing::Span,
    tx: &mpsc::Sender<Msg>,
    cancel: &CancellationToken,
) -> RunEvent {
    let (run, agent) = (spec.run, &spec.agent);
    let (session, entry) = (&agent.session, &agent.session.entry);
    let truncate = agent.truncate.lock().take();
    let request = AgentTurnRequest {
        prompt_name: session.prompt_name.clone(),
        version: session.version,
        variables: session.variables.clone(),
        input,
        conversation_id: session.conversation_id.clone(),
        truncate: truncate.clone(),
        session_id: session.session_id.clone(),
        turn_index: run.0,
        extras: ClientExtras::default(),
    };
    let bearer = agent.credential.current();
    let opened = open_turn(&agent.backend, &request, &bearer, span, cancel).await;
    let mut stream = match opened {
        Some(Ok(stream)) => stream,
        unreached => {
            // budprompt never saw the request: the truncate it carried still applies.
            let mut owed = agent.truncate.lock();
            if owed.is_none() {
                *owed = truncate;
            }
            return match unreached {
                Some(Err(failure)) => failed(span, &failure),
                _ => RunEvent::Cancelled,
            };
        }
    };

    let mut relay = Relay::new(run, tx, AnswerGate::held(agent.marker.clone()), spec.caller);
    relay.transforms = Some(entry.text_transforms.clone());
    // A structured-output agent speaks one field of its JSON.
    let speak_field = entry.structured_output.speak_field.clone();
    let mut spoken_field = speak_field
        .filter(|field| !field.is_empty())
        .map(SpeakFieldExtractor::new);
    // Tool items that started and have not reported back, by item id.
    let mut tools: HashMap<String, String> = HashMap::new();
    let report = async |event: RunEvent| {
        let _ = tx.send(Msg::Run(run, event)).await;
    };

    loop {
        let event = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                // What a tool that was still running did is not seen here.
                for tool in tools.values() {
                    report(RunEvent::Committed { label: label("started", tool) }).await;
                }
                span.record(realtime::RESPONSE_STATUS, "cancelled");
                return RunEvent::Cancelled;
            }
            event = stream.next() => event,
        };
        match event {
            Some(Ok(AgentEvent::Created { response_id, .. })) => {
                span.record(realtime::RESPONSE_ID, response_id.as_str());
                agent.responses.lock().insert(run, response_id);
                // budprompt works on it: the health watcher's first-token deadline is met, as a
                // remote backend's is when it accepts the request (spec 029 W-9). The run may list
                // its tools and reason for seconds before it writes; that is no stall.
                relay.alive().await;
            }
            Some(Ok(AgentEvent::TextDelta { delta, .. })) => {
                let text = match spoken_field.as_mut() {
                    Some(field) => field.push(&delta),
                    None => delta,
                };
                relay.push(&text).await;
            }
            Some(Ok(AgentEvent::ToolStarted { item_id, name, .. })) => {
                // What the agent wrote before a tool item was not its answer.
                relay.end(true).await;
                let phrase = entry
                    .fillers
                    .use_tool_status_messages
                    .then(|| entry.tool_phrase(&name).map(str::to_owned))
                    .flatten();
                let started = RunEvent::ToolStarted {
                    name: name.clone(),
                    phrase,
                    remote: true,
                };
                report(started).await;
                tools.insert(item_id, name);
            }
            Some(Ok(
                AgentEvent::ToolFinished { item_id, name, ok }
                | AgentEvent::FunctionCall {
                    item_id, name, ok, ..
                },
            )) => {
                tools.remove(&item_id);
                // A tool that failed may still have done a part of its work.
                let verb = if ok { "used" } else { "tried" };
                let label = label(verb, &name);
                report(RunEvent::Committed { label }).await;
            }
            Some(Ok(AgentEvent::Incomplete { status, .. })) if status == "cancelled" => {
                span.record(realtime::RESPONSE_STATUS, "cancelled");
                return RunEvent::Cancelled;
            }
            // An incomplete run (a token limit, a withheld tail) said what it could.
            Some(Ok(AgentEvent::Completed { .. } | AgentEvent::Incomplete { .. })) | None => {
                relay.end(false).await;
                break;
            }
            Some(Ok(AgentEvent::Failed { code, message })) => {
                let failure = TurnFailure::Upstream(format!("{code}: {message}"));
                return failed(span, &failure);
            }
            Some(Err(failure)) => return failed(span, &failure),
        }
    }
    if relay.answer.is_empty() {
        debug!(
            run = run.0,
            "interaction agent run produced no speakable text"
        );
        return RunEvent::Failed {
            error: "the agent returned no answer".to_owned(),
        };
    }
    span.record(realtime::RESPONSE_STATUS, "completed");
    RunEvent::Finished {
        text: relay.answer.join(" "),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::time::Duration;

    use async_trait::async_trait;
    use futures::stream::{self, BoxStream};
    use serde_json::json;

    use super::super::types::APPROVAL_REQUIRED;
    use super::*;

    type Reply = Result<Vec<AgentEvent>, TurnFailure>;

    /// A Bud gateway that answers each request with the next scripted run. A run's stream
    /// stays open after its events, as a run that is still working does.
    #[derive(Default)]
    struct Scripted {
        replies: Mutex<VecDeque<Reply>>,
        requests: Mutex<Vec<AgentTurnRequest>>,
        cancels: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl TurnBackend for Scripted {
        async fn open(
            &self,
            request: &AgentTurnRequest,
            _bearer: &str,
            _traceparent: Option<&str>,
        ) -> Result<BoxStream<'static, Result<AgentEvent, TurnFailure>>, TurnFailure> {
            self.requests.lock().push(request.clone());
            let events = self.replies.lock().pop_front().expect("a scripted run")?;
            Ok(stream::iter(events.into_iter().map(Ok))
                .chain(stream::pending())
                .boxed())
        }

        async fn cancel(&self, response_id: &str, _prompt_name: &str, _bearer: &str) {
            self.cancels.lock().push(response_id.to_owned());
        }
    }

    fn agent(backend: &Arc<Scripted>, marker: Option<&str>) -> Arc<AgentBackground> {
        let entry = json!({
            "prompt_id": "c0de", "version": 2,
            "stt": {"endpoint_id": "s"}, "tts": {"endpoint_id": "t"},
            "fillers": {"use_tool_status_messages": true},
            "tool_status_messages": {"orders__lookup_order": ["Let me look that order up."]},
        });
        let entry = bud_auth::parse_voice_agent_blob(&entry.to_string()).unwrap();
        let session = AgentSessionConfig {
            session_id: "s".to_owned(),
            prompt_name: "support".to_owned(),
            version: 2,
            entry: Arc::new(entry),
            conversation_id: "conv_voice_s".to_owned(),
            variables: None,
            text_only: false,
        };
        let backend = Arc::clone(backend) as Arc<dyn TurnBackend>;
        let credential = SessionCredential::new("bud-caller-key");
        AgentBackground::new(backend, session, credential, marker.map(str::to_owned))
    }

    fn created(id: &str) -> AgentEvent {
        AgentEvent::Created {
            response_id: id.to_owned(),
            conversation_id: None,
        }
    }

    fn text(delta: &str) -> AgentEvent {
        AgentEvent::TextDelta {
            item_id: None,
            delta: delta.to_owned(),
        }
    }

    fn tool_started(name: &str) -> AgentEvent {
        AgentEvent::ToolStarted {
            item_id: format!("item_{name}"),
            name: name.to_owned(),
            kind: "mcp_call".to_owned(),
        }
    }

    fn tool_finished(name: &str, ok: bool) -> AgentEvent {
        AgentEvent::ToolFinished {
            item_id: format!("item_{name}"),
            name: name.to_owned(),
            ok,
        }
    }

    fn completed() -> AgentEvent {
        AgentEvent::Completed {
            usage: None,
            output: Vec::new(),
        }
    }

    /// One run of `request`, and everything it reports.
    fn start(
        agent: &Arc<AgentBackground>,
        run: u64,
        request: &str,
    ) -> (mpsc::Receiver<Msg>, CancellationToken) {
        let (tx, rx) = mpsc::channel(64);
        let cancel = CancellationToken::new();
        let spec = AgentRun {
            run: RunId(run),
            agent: Arc::clone(agent),
            words: vec![request.to_owned()],
            caller: request.to_owned(),
            turn: None,
        };
        spawn_agent_run(spec, tx, cancel.clone());
        (rx, cancel)
    }

    /// The run's next report, past the health watcher's sign of life (`Msg::RunAlive`).
    async fn next_report(rx: &mut mpsc::Receiver<Msg>) -> Option<Msg> {
        loop {
            match rx.recv().await {
                Some(Msg::RunAlive(_)) => continue,
                other => return other,
            }
        }
    }

    /// The run's reports up to its last one, as text.
    async fn reports(rx: &mut mpsc::Receiver<Msg>) -> Vec<String> {
        let mut seen = Vec::new();
        loop {
            let waited = tokio::time::timeout(Duration::from_secs(30), rx.recv()).await;
            let event = match waited.expect("the run reports") {
                Some(Msg::Run(_, event)) => event,
                // The run's first token (the health watcher's deadline, chapter 10).
                Some(Msg::RunAlive(_)) => continue,
                _ => panic!("the run ended without a last report: {seen:?}"),
            };
            let last = matches!(
                event,
                RunEvent::Finished { .. } | RunEvent::Failed { .. } | RunEvent::Cancelled
            );
            seen.push(format!("{event:?}"));
            if last {
                return seen;
            }
        }
    }

    #[tokio::test]
    async fn the_answer_is_what_follows_the_last_tool_and_comes_when_the_run_completes() {
        let backend = Arc::new(Scripted::default());
        backend.replies.lock().push_back(Ok(vec![
            created("resp_1"),
            text("Let me check that. "),
            tool_started("lookup_order"),
            tool_finished("lookup_order", true),
            text("It shipped on **Monday**. "),
            text("It arrives Friday."),
            completed(),
        ]));
        let agent = agent(&backend, None);
        let (mut rx, _cancel) = start(&agent, 7, "where is my order");
        assert_eq!(
            reports(&mut rx).await,
            [
                // budprompt's tool: in flight until it reports, then an effect of the task.
                r#"ToolStarted { name: "lookup_order", phrase: Some("Let me look that order up."), remote: true }"#,
                r#"Committed { label: "used lookup order" }"#,
                r#"Sentence { attempt: 0, seq: 0, text: "It shipped on Monday." }"#,
                r#"Sentence { attempt: 0, seq: 1, text: "It arrives Friday." }"#,
                r#"Finished { text: "It shipped on Monday. It arrives Friday." }"#,
            ]
        );
        let request = backend.requests.lock()[0].clone();
        assert_eq!(request.prompt_name, "support");
        assert_eq!((request.version, request.turn_index), (2, 7));
        assert_eq!(request.input, "where is my order");
        assert_eq!(request.conversation_id, "conv_voice_s");
        assert_eq!(request.truncate, None);
        assert!(backend.cancels.lock().is_empty(), "a finished run");
    }

    #[tokio::test]
    async fn a_declared_marker_streams_the_answer_while_the_run_goes_on() {
        let backend = Arc::new(Scripted::default());
        // No `completed`: the run is still writing.
        backend.replies.lock().push_back(Ok(vec![
            created("resp_1"),
            text("Checking the hours. ANSWER: The store opens at nine. It closes"),
        ]));
        let agent = agent(&backend, Some("ANSWER:"));
        let (mut rx, cancel) = start(&agent, 1, "when do you open");
        // The run's first token comes first (the health watcher's deadline, chapter 10).
        let alive = tokio::time::timeout(Duration::from_secs(30), rx.recv()).await;
        assert!(matches!(alive, Ok(Some(Msg::RunAlive(_)))), "{alive:?}");
        let first = tokio::time::timeout(Duration::from_secs(30), rx.recv()).await;
        let Ok(Some(Msg::Run(_, RunEvent::Sentence { text, .. }))) = first else {
            panic!("a sentence before the run ends: {first:?}");
        };
        assert_eq!(text, "The store opens at nine.");

        // A cancel stops the run at budprompt; no tool ran, so nothing is named.
        cancel.cancel();
        assert_eq!(reports(&mut rx).await, ["Cancelled"]);
        for _ in 0..50 {
            tokio::task::yield_now().await;
        }
        assert_eq!(*backend.cancels.lock(), ["resp_1"]);
    }

    #[tokio::test]
    async fn a_cancel_names_the_tool_that_was_running() {
        let backend = Arc::new(Scripted::default());
        backend.replies.lock().push_back(Ok(vec![
            created("resp_1"),
            tool_started("lookup_order"),
            tool_finished("lookup_order", false),
            tool_started("refund_order"),
        ]));
        let agent = agent(&backend, None);
        let (mut rx, cancel) = start(&agent, 1, "refund my order");
        let mut seen = Vec::new();
        while seen.len() < 3 {
            let Some(Msg::Run(_, event)) = next_report(&mut rx).await else {
                panic!("the run ended early: {seen:?}");
            };
            seen.push(format!("{event:?}"));
        }
        cancel.cancel();
        seen.extend(reports(&mut rx).await);
        assert_eq!(
            seen[1..],
            [
                // It failed, which is not to say it did nothing.
                r#"Committed { label: "tried lookup order" }"#,
                r#"ToolStarted { name: "refund_order", phrase: None, remote: true }"#,
                r#"Committed { label: "started refund order" }"#,
                "Cancelled",
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn what_the_caller_did_not_hear_goes_with_the_next_run_that_arrives() {
        let unreachable = || Err(TurnFailure::Transport("connection refused".to_owned()));
        let backend = Arc::new(Scripted::default());
        let answered = vec![
            created("resp_1"),
            text("It shipped on Monday."),
            completed(),
        ];
        backend.replies.lock().extend([
            Ok(answered),
            // The second run never reaches the agent: one try and two more.
            unreachable(),
            unreachable(),
            unreachable(),
            Ok(vec![created("resp_3"), text("Okay."), completed()]),
            Ok(vec![created("resp_4"), text("Sure."), completed()]),
        ]);
        let agent = agent(&backend, None);
        let (mut rx, _cancel) = start(&agent, 1, "where is my order");
        reports(&mut rx).await;

        // A run budprompt never knew has nothing to shorten.
        agent.heard(RunId(9), "It", true);
        // What was heard so far is not news; the cut is, and what is heard after it adds.
        agent.heard(RunId(1), "It", false);
        assert_eq!(*agent.truncate.lock(), None);
        agent.heard(RunId(1), "It", true);
        agent.heard(RunId(1), "It shipped", false);
        let heard = Some(Truncate {
            response_id: "resp_1".to_owned(),
            output_text: "It shipped".to_owned(),
        });

        let (mut rx, _cancel) = start(&agent, 2, "never mind");
        let failed = reports(&mut rx).await;
        assert!(
            failed[0].starts_with(r#"Failed { error: "upstream_unavailable: "#),
            "{failed:?}"
        );
        let (mut rx, _cancel) = start(&agent, 3, "never mind");
        reports(&mut rx).await;
        let (mut rx, _cancel) = start(&agent, 4, "thanks");
        reports(&mut rx).await;
        let truncates: Vec<Option<Truncate>> = backend
            .requests
            .lock()
            .iter()
            .map(|request| request.truncate.clone())
            .collect();
        assert_eq!(
            truncates,
            [
                None,
                heard.clone(),
                heard.clone(),
                heard.clone(),
                heard,
                None
            ]
        );
    }

    /// Spec 029 W-9: a run budprompt has started is alive, as a remote backend that accepted its
    /// request is (`remote.rs`): budprompt lists its tools and reasons before it writes (its first
    /// text came 3 s or more after `response.created` on 2026-10-29), and a working run is no
    /// stall.
    #[tokio::test]
    async fn a_started_run_is_alive_before_it_writes() {
        let backend = Arc::new(Scripted::default());
        // Only the start: the run goes on reasoning, and nothing is written yet.
        backend
            .replies
            .lock()
            .push_back(Ok(vec![created("resp_1")]));
        let agent = agent(&backend, None);
        let (mut rx, _cancel) = start(&agent, 1, "what are your weekend hours");
        let first = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await;
        assert!(
            matches!(first, Ok(Some(Msg::RunAlive(RunId(1))))),
            "a started run reports itself alive at once: {first:?}"
        );
    }

    /// Spec 029 W-9's other half: a backend that answers nothing gives no sign of life, so the
    /// health watcher's first-token deadline still catches it.
    #[tokio::test]
    async fn a_run_that_never_starts_gives_no_sign_of_life() {
        let backend = Arc::new(Scripted::default());
        backend.replies.lock().push_back(Ok(Vec::new()));
        let agent = agent(&backend, None);
        let (mut rx, _cancel) = start(&agent, 1, "what are your weekend hours");
        let first = tokio::time::timeout(Duration::from_millis(300), rx.recv()).await;
        assert!(
            first.is_err(),
            "nothing from a run that never started: {first:?}"
        );
    }

    #[tokio::test]
    async fn a_run_held_for_approval_fails_under_the_approval_code() {
        let backend = Arc::new(Scripted::default());
        let held = TurnFailure::ApprovalRequired("a manager must approve refunds".to_owned());
        backend.replies.lock().push_back(Err(held));
        let agent = agent(&backend, None);
        let (mut rx, _cancel) = start(&agent, 1, "refund my order");
        let failed = reports(&mut rx).await;
        let error =
            format!(r#"Failed {{ error: "{APPROVAL_REQUIRED}: a manager must approve refunds" }}"#);
        assert_eq!(failed, [error]);
    }

    #[tokio::test]
    async fn a_run_waits_for_the_one_before_it_and_for_its_cancel() {
        let backend = Arc::new(Scripted::default());
        backend.replies.lock().extend([
            Ok(vec![created("resp_1"), tool_started("refund_order")]),
            Ok(vec![created("resp_2"), text("Done."), completed()]),
        ]);
        let agent = agent(&backend, None);
        let (mut first, cancel) = start(&agent, 1, "refund my order");
        assert!(matches!(
            next_report(&mut first).await,
            Some(Msg::Run(_, RunEvent::ToolStarted { .. }))
        ));
        // The revised request: it is not sent while the first run holds the conversation.
        let (mut second, _cancel) = start(&agent, 2, "refund my order\nCorrection: the other one");
        for _ in 0..50 {
            tokio::task::yield_now().await;
        }
        assert_eq!(backend.requests.lock().len(), 1);

        cancel.cancel();
        reports(&mut first).await;
        let done = reports(&mut second).await;
        assert_eq!(done.last().unwrap(), r#"Finished { text: "Done." }"#);
        assert_eq!(
            *backend.cancels.lock(),
            ["resp_1"],
            "cancelled before the next"
        );
        assert_eq!(backend.requests.lock().len(), 2);

        // A run cancelled while it waited never reaches the agent.
        let working = Ok(vec![created("resp_3")]);
        backend.replies.lock().push_back(working);
        let (_third, _hold) = start(&agent, 3, "one more thing");
        for _ in 0..50 {
            tokio::task::yield_now().await;
        }
        assert_eq!(backend.requests.lock().len(), 3);
        let (mut fourth, cancel) = start(&agent, 4, "and another");
        cancel.cancel();
        assert_eq!(reports(&mut fourth).await, ["Cancelled"]);
        assert_eq!(backend.requests.lock().len(), 3);
    }
}
