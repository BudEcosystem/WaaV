//! The operator's remote backends: a signed webhook, an Agent2Agent (A2A) agent, an MCP
//! server's task tool and an OpenAI Responses model. A session names one with
//! `tasks.default_backend`; its address and secret come from the server's catalog
//! (`interaction.task_backends`).
//!
//! The first three run the task themselves. Each is a [`Wire`]: create the task, hear what it
//! reports (progress, a question for the caller, an effect, the result), answer a question,
//! cancel. One driver ([`Remote::drive`]) turns that into the reports of a run. The fourth is
//! a model the gateway prompts, with the session's tools, over the Responses API
//! ([`Remote::respond`]).
//!
//! # The webhook wire
//!
//! Every message of the gateway is a `POST` to the entry's `url`, signed like a tool call
//! (`X-WaaV-Timestamp`, `X-WaaV-Signature`, `Idempotency-Key`):
//!
//! ```text
//! {"type": "task", "task_id", "revision", "request", "history": [{"role", "content"}],
//!  "callback": "/v1/tasks/<task_id>/callback"}
//! {"type": "answer", "task_id", "revision", "request_id", "outcome", "value", "operation_id"}
//! {"type": "cancel", "task_id", "revision"}
//! ```
//!
//! The reply's body, and the body of a later signed `POST` to the callback path, is one event
//! or a list of events:
//!
//! ```text
//! {"type": "progress", "text"}       {"type": "effect", "label"}
//! {"type": "question", "request_id", "kind": "confirm", "action", "arguments"}
//! {"type": "question", "request_id", "kind": "choose" | "value", "prompt", "options"}
//! {"type": "result", "text"}         {"type": "failed", "error"}       {"type": "cancelled"}
//! ```

use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use serde_json::{Value, json};
use subtle::ConstantTimeEq;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info};

use crate::core::agent::brain::SseDecoder;
use crate::core::llm::{ChatMessage, MessageRole};
use crate::core::resilience::{CircuitBreaker, CircuitBreakerConfig};

use super::actor::Msg;
use super::background::{
    ANSWER_RULE, AgentTool, AnswerGate, Backend, ERROR_BODY_CHARS, Job, NotPosted, Relay,
    TOOL_CALL_BACKSTOP, ToolCalls, ask_caller_tool, caller_words, post_json, post_signed,
    read_capped, refused, request_text, send_json, signature, traced, with_context,
};
use super::config::{
    ASK_CALLER, BackendKind, BackendMode, BackendSpec, Confirm, OAuthClient, SideEffects, ToolSpec,
};
use super::controls::{Control, ControlSource};
use super::guard;
use super::types::{GateDecision, HitlKind, RunEvent, TaskOutput};

/// How often a task or a response without a stream is read.
const POLL: Duration = Duration::from_millis(500);
/// The bounds of the interval an MCP server asks for.
const POLL_BOUNDS: (u64, u64) = (250, 5_000);
/// How long a backend has to confirm a cancel. The registry waits ten seconds for the run.
const CANCEL_CONFIRM: Duration = Duration::from_secs(8);
/// Polls that may fail in a row before the task is given up.
const MAX_MISSES: u32 = 3;
/// How much of a backend's reply is read.
const REPLY_MAX_BYTES: usize = 1 << 20;
/// What a webhook is shown of the conversation before the request.
const HISTORY_MESSAGES: usize = 12;
/// A callback's timestamp may be this far from the gateway's clock.
const CALLBACK_SKEW_S: u64 = 300;
/// Rounds of tool calls of one Responses run before it gives up.
const MAX_ROUNDS: usize = 8;
const MCP_VERSION: &str = "2026-07-28";

/// One breaker per catalog entry, shared by every session of this process: an entry that
/// keeps failing stops getting tasks for a while, and no other entry is touched.
static BREAKERS: LazyLock<Mutex<HashMap<String, Arc<CircuitBreaker>>>> =
    LazyLock::new(Mutex::default);

/// The breaker of the catalog entry `name`: open after 3 of the last tasks failed to reach
/// it or to end in time (at least half of them), for 30 seconds, then one task tries it.
pub fn breaker(name: &str) -> Arc<CircuitBreaker> {
    let config = CircuitBreakerConfig {
        min_request_volume: 3,
        window_size: 10,
        cooldown: Duration::from_secs(30),
        ..CircuitBreakerConfig::default()
    };
    let label = format!("task_backend:{name}");
    let mut breakers = BREAKERS.lock();
    let breaker = breakers
        .entry(name.to_owned())
        .or_insert_with(|| Arc::new(CircuitBreaker::with_label(config, label)));
    Arc::clone(breaker)
}

/// The entry's breaker is open and not yet ready for a trial task: its new work goes to its
/// fallback.
pub fn is_open(name: &str) -> bool {
    breaker(name).is_open()
}

/// One entry of the catalog as a session's backend.
pub struct Remote {
    spec: BackendSpec,
    /// Signs a webhook's messages when its entry names no secret of its own.
    tool_secret: Option<String>,
    /// For a Responses model: the persona preamble and the operator's prompt, their role
    /// lines ([`guard::role_lines`]), and the session's tools.
    system_prompt: String,
    roles: String,
    tools: Vec<ToolSpec>,
    /// The context an A2A agent gave the session's first task: later tasks continue it.
    context: Mutex<Option<String>>,
}

impl Remote {
    /// `prompts`: the persona preamble and the operator's prompt (empty for none).
    pub fn new(
        spec: BackendSpec,
        tool_secret: Option<String>,
        prompts: [&str; 2],
        tools: Vec<ToolSpec>,
    ) -> Arc<Self> {
        Arc::new(Self {
            spec,
            tool_secret,
            system_prompt: prompts.join("\n\n").trim().to_owned(),
            roles: guard::role_lines(&prompts),
            tools,
            context: Mutex::default(),
        })
    }

    fn bearer(&self) -> Option<(&'static str, String)> {
        bearer(&self.spec)
    }
}

fn bearer(spec: &BackendSpec) -> Option<(&'static str, String)> {
    let secret = spec.secret.as_ref()?;
    Some(("Authorization", format!("Bearer {secret}")))
}

impl Backend for Remote {
    fn label(&self) -> String {
        self.spec.label()
    }

    fn own_tools(&self) -> Option<bool> {
        // A Responses model is prompted by the gateway and calls the session's tools.
        (self.spec.kind != BackendKind::OpenaiResponses)
            .then_some(self.spec.side_effects == SideEffects::Possible)
    }

    fn max_concurrent(&self) -> Option<usize> {
        self.spec.max_concurrent.map(|n| n as usize)
    }

    fn start(
        self: Arc<Self>,
        job: Job,
        tx: mpsc::Sender<Msg>,
        cancel: CancellationToken,
    ) -> JoinHandle<()> {
        let trace_id = job.trace_id.clone();
        tokio::spawn(traced(trace_id, async move {
            let run = job.run;
            let last = self.run(job, &tx, &cancel).await;
            let _ = tx.send(Msg::Run(run, last)).await;
        }))
    }
}

impl Remote {
    /// One run under the entry's breaker: refused at once while it is open, and a task that
    /// changes nothing tried once more when the backend did not answer.
    async fn run(&self, job: Job, tx: &mpsc::Sender<Msg>, cancel: &CancellationToken) -> RunEvent {
        let breaker = breaker(&self.spec.name);
        if !breaker.allow_request() {
            let error = format!("the {} keeps failing, so it is not tried now", self.label());
            return RunEvent::Failed { error };
        }
        // A Responses run may have spoken part of its answer before it failed: never twice.
        let read_only = self.spec.side_effects == SideEffects::None;
        let mut tries = if read_only && self.spec.kind != BackendKind::OpenaiResponses {
            2
        } else {
            1
        };
        loop {
            let reached = match self.spec.kind {
                BackendKind::OpenaiResponses => match self.respond(&job, tx, cancel).await {
                    RunEvent::Failed { error } => Err(error),
                    last => Ok(last),
                },
                _ => self.drive(&job, tx, cancel).await,
            };
            match reached {
                Ok(last) => {
                    if !matches!(last, RunEvent::Cancelled) {
                        breaker.record_success();
                    }
                    return last;
                }
                Err(error) => {
                    breaker.record_failure();
                    tries -= 1;
                    if tries == 0 || cancel.is_cancelled() {
                        return RunEvent::Failed { error };
                    }
                    debug!(backend = %self.spec.name, %error, "interaction backend tried again");
                }
            }
        }
    }

    /// How long a task may take on a backend that runs it itself.
    fn timeout(&self) -> Duration {
        self.spec
            .timeout_s
            .map_or(TOOL_CALL_BACKSTOP, Duration::from_secs)
    }
}

/// What a remote task reported.
#[derive(Debug, PartialEq)]
enum Step {
    /// It is working; the text says on what.
    Working(Option<String>),
    /// It changed something: the label completes "it had already ...".
    Effect(String),
    Ask(Ask),
    /// What the result carries besides its text; just before its `Done`.
    Output(TaskOutput),
    Done(String),
    Failed(String),
    Cancelled,
    /// A conversation control the agent asks for (chapter 9), with the agent's id for it.
    Control {
        control: Control,
        id: Option<String>,
    },
}

/// A result's structured `data` and `artifacts` as a step; `None` when it has neither.
fn output(data: Option<&Value>, artifacts: Vec<Value>) -> Option<Step> {
    let data = data.filter(|data| !data.is_null()).cloned();
    (data.is_some() || !artifacts.is_empty())
        .then_some(Step::Output(TaskOutput { data, artifacts }))
}

/// A question of the backend's for the caller, as the registry's gate takes it.
#[derive(Debug, PartialEq)]
struct Ask {
    /// The backend's name for the question.
    id: String,
    kind: HitlKind,
    op_name: String,
    args: Value,
    /// MCP: the field of the form the answer fills, when it is a yes or no.
    field: Option<(String, bool)>,
}

impl Ask {
    /// A question in words, with the options of a choice.
    fn asking(id: &str, question: &str, options: &[String]) -> Self {
        let kind = if options.len() > 1 {
            HitlKind::Choose
        } else {
            HitlKind::Value
        };
        let name = if kind == HitlKind::Choose {
            "choose"
        } else {
            "value"
        };
        let mut args = json!({ "kind": name, "question": question });
        if kind == HitlKind::Choose {
            args["options"] = json!(options);
        }
        Self {
            id: id.to_owned(),
            kind,
            op_name: ASK_CALLER.to_owned(),
            args,
            field: None,
        }
    }

    /// A form or a sign-in link for the client's screen: `source` holds its `schema` or its
    /// `url`, and `sensitive`.
    fn on_screen(id: &str, kind: HitlKind, question: &str, source: &Value) -> Self {
        let mut args = json!({ "question": question, "sensitive": source["sensitive"] == true });
        for key in ["schema", "url"] {
            if !source[key].is_null() {
                args[key] = source[key].clone();
            }
        }
        Self {
            id: id.to_owned(),
            kind,
            op_name: ASK_CALLER.to_owned(),
            args,
            field: None,
        }
    }
}

/// One protocol's side of a remote task. A request that never reached the backend is
/// [`NotPosted::Unsent`]; one whose fate is not known is [`NotPosted::Unknown`].
#[async_trait::async_trait]
trait Wire: Send {
    async fn create(
        &mut self,
        request: &str,
        history: &[ChatMessage],
    ) -> Result<Vec<Step>, NotPosted>;
    /// What it reported since the last call: waits for the next poll, or for a callback.
    async fn next(&mut self) -> Result<Vec<Step>, NotPosted>;
    async fn answer(&mut self, ask: &Ask, decision: &GateDecision) -> Result<Vec<Step>, NotPosted>;
    /// Ask it to stop, and wait for its last word.
    async fn cancel(&mut self) -> Result<Vec<Step>, NotPosted>;
    /// Whether it says what it changed. When it does not, the effects of a task that may
    /// change something are never known.
    fn reports_effects(&self) -> bool;
    /// What continues this task later (a follow-up), once the backend has named it.
    fn handle(&self) -> Option<String>;
    /// The `control_applied` of a control the backend asked for, back on its own wire.
    async fn applied(
        &mut self,
        _id: Option<&str>,
        _applied: &Value,
    ) -> Result<Vec<Step>, NotPosted> {
        Ok(Vec::new())
    }
}

impl Remote {
    /// One run on a backend that does the task itself. `Err`: the backend did not answer
    /// (it could not be reached, stopped answering or gave no result in time).
    async fn drive(
        &self,
        job: &Job,
        tx: &mpsc::Sender<Msg>,
        cancel: &CancellationToken,
    ) -> Result<RunEvent, String> {
        let run = job.run;
        // The run's reports, and the gate its questions go through.
        let gates = ToolCalls::new(run, tx, cancel, None);
        let effects = self.spec.side_effects == SideEffects::Possible;
        let mut wire: Box<dyn Wire + '_> = match self.spec.kind {
            BackendKind::Webhook => Box::new(Webhook::open(self, job)),
            BackendKind::A2a => Box::new(
                A2a {
                    state: job.state.clone(),
                    ..A2a::new(self, job.handle.clone())
                }
                .with_pushes(job),
            ),
            _ => Box::new(Mcp {
                backend: self,
                id: None,
                poll: POLL,
                said: None,
                answered: HashSet::new(),
                follows: job.handle.clone(),
                events: None,
                state: job.state.clone(),
                heard: Heard::new(&self.spec),
            }),
        };
        let caller = caller_words(&job.history, &job.words);
        let mut relay = Relay::new(run, tx, AnswerGate::new(false), caller);
        // A child's answer is its orchestrator's source, not speech.
        relay.guarded = !job.child;
        // The request is not abandoned half way: a task it created must be stopped by name.
        let request = request_text(&job.words);
        let mut steps: VecDeque<Step> = match wire.create(&request, &job.history).await {
            Ok(steps) => {
                // The backend works on it: the health watcher's first-token deadline is met.
                relay.alive().await;
                steps.into()
            }
            Err(NotPosted::Unsent(error)) => return Err(error),
            Err(NotPosted::Unknown(error)) => {
                if effects {
                    gates.report(RunEvent::EffectsUnseen).await;
                }
                return Err(error);
            }
        };
        if effects && !wire.reports_effects() {
            gates.report(RunEvent::EffectsUnseen).await;
        }
        // What a follow-up of this task continues.
        if let Some(handle) = wire.handle() {
            gates.report(RunEvent::Handle(handle)).await;
        }
        // While the backend works, a task that may change something has an operation in
        // flight: a cancel is confirmed by the backend, not assumed.
        let mut phrase: Option<String> = None;
        let name = self.spec.name.clone();
        let working = move |phrase: Option<String>| RunEvent::ToolStarted {
            name: name.clone(),
            phrase,
            remote: effects,
        };
        if effects {
            gates.report(working(phrase.clone())).await;
        }

        let give_up = tokio::time::Instant::now() + self.timeout();
        let mut misses = 0;
        loop {
            if cancel.is_cancelled() {
                return Ok(stop(wire, effects, &gates).await);
            }
            let Some(step) = steps.pop_front() else {
                let more = tokio::select! {
                    biased;
                    _ = cancel.cancelled() => continue,
                    _ = tokio::time::sleep_until(give_up) => None,
                    more = wire.next() => Some(more),
                };
                let Some(more) = more else {
                    stop(wire, effects, &gates).await;
                    return Err(format!("the {} gave no result in time", self.label()));
                };
                match more {
                    Ok(more) => {
                        misses = 0;
                        steps.extend(more);
                    }
                    Err(NotPosted::Unsent(error) | NotPosted::Unknown(error)) => {
                        misses += 1;
                        debug!(backend = %self.spec.name, %error, "interaction backend poll failed");
                        if misses == MAX_MISSES {
                            return Err(error);
                        }
                    }
                }
                continue;
            };
            match step {
                Step::Working(text) => {
                    phrase = text;
                    gates.report(working(phrase.clone())).await;
                }
                Step::Effect(label) => {
                    gates.report(RunEvent::Committed { label }).await;
                    if effects {
                        gates.report(working(phrase.clone())).await;
                    }
                }
                Step::Ask(ask) => {
                    let decision = gates.gate(&ask.op_name, ask.args.clone(), ask.kind).await;
                    if cancel.is_cancelled() {
                        continue;
                    }
                    match wire.answer(&ask, &decision).await {
                        Ok(more) => steps.extend(more),
                        Err(NotPosted::Unsent(error) | NotPosted::Unknown(error)) => {
                            return Err(error);
                        }
                    }
                    if effects {
                        gates.report(working(phrase.clone())).await;
                    }
                }
                Step::Output(output) => {
                    gates.report(RunEvent::Output(Box::new(output))).await;
                }
                Step::Done(text) => {
                    relay.push(&text).await;
                    relay.end(false).await;
                    if relay.answer.is_empty() {
                        let error = format!("the {} returned no answer", self.label());
                        return Ok(RunEvent::Failed { error });
                    }
                    let text = relay.answer.join(" ");
                    return Ok(RunEvent::Finished { text });
                }
                Step::Failed(error) => return Ok(RunEvent::Failed { error }),
                Step::Cancelled => return Ok(RunEvent::Cancelled),
                // Applied by the session's one control executor, as the orchestrator's
                // `conversation_control`; its `control_applied` goes back on the wire.
                Step::Control { control, id } => {
                    let (reply, applied) = oneshot::channel();
                    gates.report(RunEvent::Control { control, reply }).await;
                    let applied = applied.await.unwrap_or_default();
                    if applied.is_null() {
                        continue;
                    }
                    match wire.applied(id.as_deref(), &applied).await {
                        Ok(more) => steps.extend(more),
                        Err(NotPosted::Unsent(error) | NotPosted::Unknown(error)) => {
                            debug!(backend = %self.spec.name, %error, "interaction control_applied not delivered");
                        }
                    }
                }
            }
        }
    }
}

/// Stop a remote task. What it says it had done by then is reported; a backend that may
/// change things and does not confirm leaves the task's effects unknown.
async fn stop(mut wire: Box<dyn Wire + '_>, effects: bool, gates: &ToolCalls) -> RunEvent {
    let last = tokio::time::timeout(CANCEL_CONFIRM, wire.cancel()).await;
    let steps = last.ok().and_then(Result::ok).unwrap_or_default();
    let over = |step: &Step| matches!(step, Step::Cancelled | Step::Done(_) | Step::Failed(_));
    if !steps.iter().any(over) {
        if effects {
            gates.report(RunEvent::EffectsUnseen).await;
        }
        return RunEvent::Cancelled;
    }
    let done = steps.into_iter().filter_map(|step| match step {
        Step::Effect(label) => Some(label),
        _ => None,
    });
    let done: Vec<String> = done.collect();
    if !done.is_empty() {
        // One report: the first one after a cancel settles the task.
        let label = done.join(" and ");
        gates.report(RunEvent::Committed { label }).await;
    }
    RunEvent::Cancelled
}

/// The text of a reply that succeeded. A 4xx is a refusal (nothing happened); anything else
/// that is not a 2xx leaves the request's fate unknown.
async fn body_of(mut response: reqwest::Response) -> Result<String, NotPosted> {
    let status = response.status();
    let text = read_capped(&mut response, REPLY_MAX_BYTES).await;
    if status.is_success() {
        return Ok(text);
    }
    let shown: String = text.chars().take(ERROR_BODY_CHARS).collect();
    let error = format!("HTTP {status}: {shown}");
    if status.is_client_error() {
        Err(NotPosted::Unsent(error))
    } else {
        Err(NotPosted::Unknown(error))
    }
}

/// One JSON-RPC call: its `result`. A reply written as server-sent events carries the
/// response as its last event.
async fn rpc(
    url: &str,
    method: &str,
    params: Value,
    headers: &[(&str, String)],
) -> Result<Value, NotPosted> {
    let text = body_of(post_json(url, rpc_body(method, params), headers).await?).await?;
    rpc_reply(method, &text)
}

fn rpc_body(method: &str, params: Value) -> String {
    let id = uuid::Uuid::new_v4().simple().to_string();
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }).to_string()
}

/// The `result` of a JSON-RPC reply, or of the last response among its server-sent events.
fn rpc_reply(method: &str, text: &str) -> Result<Value, NotPosted> {
    let parsed = serde_json::from_str::<Value>(text).ok().or_else(|| {
        let events = text.lines().filter_map(|line| line.strip_prefix("data:"));
        let mut replies = events.filter_map(|data| serde_json::from_str::<Value>(data).ok());
        replies.rfind(|reply| reply.get("result").is_some() || reply.get("error").is_some())
    });
    let Some(reply) = parsed else {
        return Err(NotPosted::Unknown(format!(
            "{method}: the reply is not JSON-RPC"
        )));
    };
    rpc_result(method, reply)
}

fn rpc_result(method: &str, mut reply: Value) -> Result<Value, NotPosted> {
    if let Some(error) = reply.get("error").filter(|error| !error.is_null()) {
        // The backend heard the call and refused it.
        let message = error["message"].as_str().unwrap_or("refused");
        return Err(NotPosted::Unsent(format!("{method}: {message}")));
    }
    Ok(reply["result"].take())
}

/// A call whose reply may be written as server-sent events, read as they come: its events
/// when it is, else its one reply's `result`.
async fn rpc_stream(
    url: &str,
    method: &str,
    params: Value,
    headers: &[(&str, String)],
) -> Result<Result<Events, Value>, NotPosted> {
    let body = rpc_body(method, params);
    let reply = send_json(reqwest::Method::POST, url, Some(body), headers, true).await?;
    if reply.status().is_success() && is_stream(&reply) {
        return Ok(Ok(Events {
            reply,
            decoder: SseDecoder::default(),
            ready: VecDeque::new(),
        }));
    }
    let text = body_of(reply).await?;
    rpc_reply(method, &text).map(Err)
}

/// An HTTP+JSON `POST /message:stream` (A2A 1.0): its events when the reply streams, else the
/// one reply.
async fn rest_stream(
    endpoint: &str,
    params: &Value,
    headers: &[(&str, String)],
) -> Result<Result<Events, Value>, NotPosted> {
    let (verb, url, body) = rest_route(endpoint, "SendStreamingMessage", params)?;
    let reply = send_json(verb, &url, body, headers, true).await?;
    if reply.status().is_success() && is_stream(&reply) {
        return Ok(Ok(Events {
            reply,
            decoder: SseDecoder::default(),
            ready: VecDeque::new(),
        }));
    }
    let text = body_of(reply).await?;
    let reply = serde_json::from_str(&text).map_err(|_| {
        NotPosted::Unknown("SendStreamingMessage: the reply is not JSON".to_owned())
    })?;
    Ok(Err(reply))
}

/// The events of a reply written as server-sent events, each as it arrives.
struct Events {
    reply: reqwest::Response,
    decoder: SseDecoder,
    ready: VecDeque<Value>,
}

impl Events {
    /// The next event's `data`; `None` once the reply has ended.
    async fn next(&mut self) -> Result<Option<Value>, NotPosted> {
        loop {
            if let Some(event) = self.ready.pop_front() {
                return Ok(Some(event));
            }
            let chunk = self.reply.chunk().await;
            let chunk = chunk.map_err(|error| NotPosted::Unknown(error.without_url().to_string()));
            let Some(bytes) = chunk? else {
                return Ok(None);
            };
            let events = take_events(&mut self.decoder, &bytes).map_err(NotPosted::Unknown)?;
            self.ready.extend(events);
        }
    }
}

/// An A2A agent's open stream: server-sent events (JSON-RPC, HTTP+JSON) or a gRPC stream.
enum A2aEvents {
    Sse(Events),
    Grpc(super::a2a_grpc::Stream),
}

impl A2aEvents {
    async fn next(&mut self) -> Result<Option<Value>, NotPosted> {
        match self {
            A2aEvents::Sse(events) => events.next().await,
            A2aEvents::Grpc(events) => events.next().await,
        }
    }
}

fn text_of(value: &Value) -> Option<String> {
    let text = value.as_str()?.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// How a question ended, as a backend is told it.
fn outcome(decision: &GateDecision) -> &'static str {
    match decision {
        GateDecision::Allow(_) | GateDecision::Answer(_) => "accept",
        GateDecision::Refuse("declined" | "not_by_voice") => "decline",
        GateDecision::Refuse("timeout") => "timeout",
        GateDecision::Refuse(_) => "cancel",
    }
}

// ---- the webhook

/// A run that waits for a backend's callbacks, by task id.
struct Callback {
    /// The run's place ([`Listening`]): only that run gives it up.
    place: u64,
    revision: u32,
    proof: Proof,
    events: mpsc::Sender<Value>,
}

/// How a callback shows it is the backend's.
#[derive(Clone)]
enum Proof {
    /// A webhook signs it like the gateway's own messages, with this secret.
    Signed(Option<String>),
    /// An A2A agent's push carries the token the gateway gave it with the push
    /// configuration, in `X-A2A-Notification-Token`.
    Token(String),
}

/// A run's place in the callback table, given up when the run ends (unless a later run of the
/// task has taken it). A place is the run's own, not its revision's: words that go on with a
/// request (`TaskCmd::Continue`) start a run of the same revision, and the replaced run's end
/// took the new run's place (live PR2, r17: its pushes 404, `GetTask` polled without a pause).
struct Listening {
    task: String,
    place: u64,
}

impl Listening {
    fn open(task: &str, revision: u32, proof: Proof) -> (Self, mpsc::Receiver<Value>) {
        static PLACES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let place = PLACES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (tx, events) = mpsc::channel(64);
        let waiting = Callback {
            place,
            revision,
            proof,
            events: tx,
        };
        CALLBACKS.lock().insert(task.to_owned(), waiting);
        let listening = Self {
            task: task.to_owned(),
            place,
        };
        (listening, events)
    }
}

impl Drop for Listening {
    fn drop(&mut self) {
        let mut callbacks = CALLBACKS.lock();
        if callbacks
            .get(&self.task)
            .is_some_and(|c| c.place == self.place)
        {
            callbacks.remove(&self.task);
        }
    }
}

/// The task id and revision of a job, or a made-up one for a run of no task.
fn job_task(job: &Job) -> (String, u32) {
    match &job.task {
        Some((task, revision)) => (task.0.clone(), *revision),
        None => (format!("run_{}", uuid::Uuid::new_v4().simple()), 1),
    }
}

static CALLBACKS: LazyLock<Mutex<HashMap<String, Callback>>> = LazyLock::new(Mutex::default);

/// Why a callback was not taken, as an HTTP status.
#[derive(Debug, PartialEq)]
pub enum CallbackRefused {
    /// 404: no run of this process waits for that task.
    UnknownTask,
    /// 401: no signature, a wrong one, or an old timestamp.
    Unsigned,
    /// 409: the task was revised since; the callback is of a replaced run.
    Stale,
    /// 400: the body is not an event or a list of events.
    Invalid,
}

/// How a callback says who sent it: the webhook's `X-WaaV-Timestamp` and
/// `X-WaaV-Signature`, or an A2A push's `X-A2A-Notification-Token`. A push handed on by
/// another gateway process carries the token's SHA-256 ([`token_digest`]) in its place.
#[derive(Default, Clone, Copy)]
pub struct CallbackFrom<'a> {
    pub timestamp: Option<&'a str>,
    pub signature: Option<&'a str>,
    pub token: Option<&'a str>,
    pub token_sha256: Option<&'a str>,
}

// The token stays out of logs.
impl std::fmt::Debug for CallbackFrom<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CallbackFrom")
            .field("timestamp", &self.timestamp)
            .field("signature", &self.signature.is_some())
            .field("token", &self.token.is_some())
            .field("token_sha256", &self.token_sha256.is_some())
            .finish()
    }
}

impl CallbackFrom<'_> {
    /// Whether it carries a proof at all: a push's token, or a timestamp and a signature. One
    /// without is refused before anything else is asked (another process, the task Redis).
    pub fn has_proof(&self) -> bool {
        let given = |value: Option<&str>| value.is_some_and(|value| !value.trim().is_empty());
        given(self.token)
            || given(self.token_sha256)
            || (given(self.timestamp) && given(self.signature))
    }
}

/// The hex SHA-256 of a push's token: what goes on the task Redis's channel when another
/// process hands the push on, so the token itself is never there.
pub fn token_digest(token: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(token.trim().as_bytes()))
}

/// A backend's `POST /v1/tasks/{task_id}/callback`: `body` holds one event or a list of
/// events. A webhook's is signed like the gateway's own messages with the backend's secret;
/// an A2A agent's push (a task, or a status or artifact update) carries the task's token.
pub fn callback(
    task_id: &str,
    sender: CallbackFrom<'_>,
    body: &str,
) -> Result<(), CallbackRefused> {
    // Checked and parsed outside the process-wide table: one slow callback holds up no other.
    let waiting = CALLBACKS.lock().get(task_id).map(|waiting| Callback {
        place: waiting.place,
        revision: waiting.revision,
        proof: waiting.proof.clone(),
        events: waiting.events.clone(),
    });
    let waiting = waiting.ok_or(CallbackRefused::UnknownTask)?;
    let same = |given: Option<&str>, expected: &str| {
        given.is_some_and(|given| bool::from(given.trim().as_bytes().ct_eq(expected.as_bytes())))
    };
    match &waiting.proof {
        Proof::Token(token) if same(sender.token, token) => {}
        // Handed on by another process: the token's digest.
        Proof::Token(token) if same(sender.token_sha256, &token_digest(token)) => {}
        Proof::Token(_) | Proof::Signed(None) => return Err(CallbackRefused::Unsigned),
        Proof::Signed(Some(secret)) => {
            let sent = sender
                .timestamp
                .and_then(|text| text.trim().parse::<u64>().ok());
            let sent = sent.ok_or(CallbackRefused::Unsigned)?;
            let now = SystemTime::now().duration_since(UNIX_EPOCH);
            let now = now.map_or(0, |since_epoch| since_epoch.as_secs());
            let expected = signature(secret, sent, body);
            if !same(sender.signature, &expected) || now.abs_diff(sent) > CALLBACK_SKEW_S {
                return Err(CallbackRefused::Unsigned);
            }
        }
    }
    let events: Value = serde_json::from_str(body).map_err(|_| CallbackRefused::Invalid)?;
    let events = match events {
        Value::Array(events) => events,
        event @ Value::Object(_) => vec![event],
        _ => return Err(CallbackRefused::Invalid),
    };
    let of_run = |event: &Value| {
        let revision = event.get("revision").and_then(Value::as_u64);
        revision.is_none_or(|revision| revision == u64::from(waiting.revision))
    };
    if !events.iter().all(of_run) {
        return Err(CallbackRefused::Stale);
    }
    for event in events {
        // A run that is ending no longer reads them.
        let _ = waiting.events.try_send(event);
    }
    Ok(())
}

/// What the operator said of a webhook hand-off to a person (`controls.human.url`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersonAnswer {
    /// `human_connected`: a person takes the call.
    Connected,
    /// `human_unavailable`, or the request was refused or not delivered.
    Unavailable,
    /// Neither came within the wait.
    NoAnswer,
}

impl PersonAnswer {
    pub fn as_str(self) -> &'static str {
        match self {
            PersonAnswer::Connected => "connected",
            PersonAnswer::Unavailable => "unavailable",
            PersonAnswer::NoAnswer => "no_answer",
        }
    }

    /// The operator's event, in its reply or on the callback route.
    fn of(event: &Value) -> Option<Self> {
        match event["type"].as_str()? {
            "human_connected" => Some(PersonAnswer::Connected),
            "human_unavailable" => Some(PersonAnswer::Unavailable),
            _ => None,
        }
    }
}

/// Asks the operator for a person: `body` (the `human_requested` event, with the session's
/// state) is signed to `url` with a `task_id` and the `callback` route of its own, and the
/// operator answers `human_connected` or `human_unavailable`, in its reply or later on that
/// route (signed with the same secret), within `wait`.
pub(super) async fn ask_for_person(
    url: &str,
    secret: Option<String>,
    mut body: Value,
    key: &str,
    wait: Duration,
) -> PersonAnswer {
    let task = format!("human_{}", uuid::Uuid::new_v4().simple());
    let (_listening, mut events) = Listening::open(&task, 1, Proof::Signed(secret.clone()));
    // A callback that reaches another gateway process is handed here (task Redis).
    let _held = super::store::store().hold_person(&task, wait).await;
    body["task_id"] = json!(task);
    body["callback"] = json!(format!("/v1/tasks/{task}/callback"));
    let posted = post_signed(url, body.to_string(), secret.as_deref(), Some(key)).await;
    let reply = match posted {
        Ok(response) => body_of(response).await,
        Err(not_posted) => Err(not_posted),
    };
    let Ok(reply) = reply else {
        info!(%task, "interaction request for a person not taken");
        return PersonAnswer::Unavailable;
    };
    let replied: Value = serde_json::from_str(&reply).unwrap_or_default();
    let listed = match replied {
        Value::Array(events) => events,
        event => vec![event],
    };
    if let Some(answer) = listed.iter().find_map(PersonAnswer::of) {
        return answer;
    }
    let called_back = async {
        while let Some(event) = events.recv().await {
            if let Some(answer) = PersonAnswer::of(&event) {
                return answer;
            }
        }
        PersonAnswer::NoAnswer
    };
    tokio::time::timeout(wait, called_back)
        .await
        .unwrap_or(PersonAnswer::NoAnswer)
}

struct Webhook<'a> {
    backend: &'a Remote,
    task: String,
    revision: u32,
    events: mpsc::Receiver<Value>,
    _listening: Listening,
    /// The task id of the finished task this one follows up.
    follows: Option<String>,
    /// The conversation state (chapter 9), sent as `metadata.waav_state`.
    state: Option<Value>,
}

impl<'a> Webhook<'a> {
    /// The run's side of the webhook, listening for callbacks from now on.
    fn open(backend: &'a Remote, job: &Job) -> Self {
        let (task, revision) = job_task(job);
        let proof = Proof::Signed(backend.secret().map(str::to_owned));
        let (listening, events) = Listening::open(&task, revision, proof);
        Self {
            backend,
            task,
            revision,
            events,
            _listening: listening,
            follows: job.handle.clone(),
            state: job.state.clone(),
        }
    }

    async fn post(&self, kind: &str, mut message: Value) -> Result<Vec<Step>, NotPosted> {
        message["type"] = json!(kind);
        message["task_id"] = json!(self.task);
        message["revision"] = json!(self.revision);
        // A repeated delivery of one message is the same message.
        let key = format!("{}:{}:{kind}", self.task, self.revision);
        let key = match message["request_id"].as_str() {
            Some(request) => format!("{key}:{request}"),
            None => key,
        };
        let (url, secret) = (&self.backend.spec.url, self.backend.secret());
        let posted = post_signed(url, message.to_string(), secret, Some(&key)).await?;
        let body = body_of(posted).await?;
        Ok(webhook_steps(
            serde_json::from_str(&body).unwrap_or_default(),
        ))
    }
}

impl Remote {
    /// What signs a webhook's messages and its callbacks.
    fn secret(&self) -> Option<&str> {
        self.spec.secret.as_deref().or(self.tool_secret.as_deref())
    }
}

/// The events of a webhook's reply or callback.
fn webhook_steps(body: Value) -> Vec<Step> {
    let events = match body {
        Value::Array(events) => events,
        event => vec![event],
    };
    let with_output = |event: &Value| {
        let artifacts = event["artifacts"].as_array().cloned().unwrap_or_default();
        let output = (event["type"] == "result")
            .then(|| output(event.get("data"), artifacts))
            .flatten();
        output.into_iter().chain(webhook_step(event))
    };
    events.iter().flat_map(with_output).collect()
}

fn webhook_step(event: &Value) -> Option<Step> {
    let step = webhook_event(event)?;
    Some(with_deadline(step, event["expires_in_ms"].as_u64()))
}

/// A question that must be answered within `ms` (`expires_in_ms` of its backend): the gate
/// reads it from the question's arguments.
fn with_deadline(step: Step, ms: Option<u64>) -> Step {
    match (step, ms) {
        (Step::Ask(mut ask), Some(ms)) => {
            ask.args["expires_in_ms"] = json!(ms);
            Step::Ask(ask)
        }
        (step, _) => step,
    }
}

fn webhook_event(event: &Value) -> Option<Step> {
    let text = |key: &str| text_of(&event[key]);
    Some(match event["type"].as_str()? {
        "progress" => Step::Working(text("text")),
        "effect" => Step::Effect(text("label")?),
        "result" => Step::Done(text("text").unwrap_or_default()),
        "failed" => Step::Failed(text("error").unwrap_or_else(|| "the webhook failed".to_owned())),
        "cancelled" => Step::Cancelled,
        "control" => return control_step(event),
        "question" => {
            let id = text("request_id").unwrap_or_default();
            if event["kind"] == "confirm" {
                let Some(action) = text("action") else {
                    return Some(Step::Failed(
                        "the webhook's confirmation names no action".into(),
                    ));
                };
                let args = match &event["arguments"] {
                    args @ Value::Object(_) => args.clone(),
                    _ => json!({}),
                };
                Step::Ask(Ask {
                    id,
                    kind: HitlKind::Confirm,
                    op_name: action,
                    args,
                    field: None,
                })
            } else {
                let Some(prompt) = text("prompt") else {
                    return Some(Step::Failed("the webhook's question has no prompt".into()));
                };
                // Answered on the client's screen: fields to fill in, or a link to open.
                let screen = match event["kind"].as_str() {
                    Some("form") if event["schema"].is_object() => Some(HitlKind::Form),
                    Some("auth_url") if text("url").is_some() => Some(HitlKind::AuthUrl),
                    Some("form" | "auth_url") => {
                        return Some(Step::Failed(
                            "the webhook's form has no schema, or its link no url".into(),
                        ));
                    }
                    _ => None,
                };
                if let Some(kind) = screen {
                    return Some(Step::Ask(Ask::on_screen(&id, kind, &prompt, event)));
                }
                let options = event["options"].as_array();
                let options: Vec<String> = options
                    .map(|all| all.iter().filter_map(text_of).collect())
                    .unwrap_or_default();
                Step::Ask(Ask::asking(&id, &prompt, &options))
            }
        }
        _ => return None,
    })
}

#[async_trait::async_trait]
impl Wire for Webhook<'_> {
    async fn create(
        &mut self,
        request: &str,
        history: &[ChatMessage],
    ) -> Result<Vec<Step>, NotPosted> {
        let recent = &history[history.len().saturating_sub(HISTORY_MESSAGES)..];
        let mut message = json!({
            "request": request,
            "history": spoken_turns(recent),
            "callback": format!("/v1/tasks/{}/callback", self.task),
        });
        if let Some(follows) = &self.follows {
            message["follows"] = json!(follows);
        }
        if let Some(state) = &self.state {
            message["metadata"] = json!({ "waav_state": state });
        }
        self.post("task", message).await
    }

    async fn next(&mut self) -> Result<Vec<Step>, NotPosted> {
        match self.events.recv().await {
            Some(event) => Ok(webhook_steps(event)),
            // The sender lives in the callback table as long as this run does.
            None => Err(NotPosted::Unknown("the callback channel closed".to_owned())),
        }
    }

    async fn answer(&mut self, ask: &Ask, decision: &GateDecision) -> Result<Vec<Step>, NotPosted> {
        let mut message = json!({ "request_id": ask.id, "outcome": outcome(decision) });
        match decision {
            GateDecision::Allow(op) => message["operation_id"] = json!(op.0),
            GateDecision::Answer(value) => message["value"] = value.clone(),
            GateDecision::Refuse(_) => {}
        }
        self.post("answer", message).await
    }

    async fn cancel(&mut self) -> Result<Vec<Step>, NotPosted> {
        let mut steps = self.post("cancel", json!({})).await?;
        let over = |step: &Step| matches!(step, Step::Cancelled | Step::Done(_) | Step::Failed(_));
        while !steps.iter().any(over) {
            steps.extend(self.next().await?);
        }
        Ok(steps)
    }

    fn reports_effects(&self) -> bool {
        true
    }

    fn handle(&self) -> Option<String> {
        Some(self.task.clone())
    }

    async fn applied(&mut self, id: Option<&str>, applied: &Value) -> Result<Vec<Step>, NotPosted> {
        let mut message = applied.clone();
        let id = id.map_or_else(|| uuid::Uuid::new_v4().simple().to_string(), str::to_owned);
        message["request_id"] = json!(id);
        self.post("control_applied", message).await
    }
}

/// A control event of an agent's (`{"type": "control", "kind", "value", "request_id"}`):
/// the same control a client's `control` message makes. One the gateway cannot read is
/// dropped.
fn control_step(event: &Value) -> Option<Step> {
    match Control::from_json(event, ControlSource::Agent) {
        Ok(control) => Some(Step::Control {
            control,
            id: text_of(&event["request_id"]),
        }),
        Err(why) => {
            debug!(%why, "interaction agent control not read");
            None
        }
    }
}

/// The caller's and the agent's turns, as `{"role", "content"}`.
fn spoken_turns(history: &[ChatMessage]) -> Vec<Value> {
    let turn = |message: &ChatMessage| {
        let role = match message.role {
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            _ => return None,
        };
        let content = message.content.as_deref().filter(|text| !text.is_empty())?;
        Some(json!({ "role": role, "content": content }))
    };
    history.iter().filter_map(turn).collect()
}

// ---- Agent2Agent 1.0: the JSON-RPC, HTTP+JSON and gRPC bindings

/// How an A2A agent is reached: the protocol binding of the interface its card lists (A2A 1.0
/// `supportedInterfaces[].protocolBinding`). JSON-RPC without a card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Binding {
    JsonRpc,
    /// HTTP+JSON (`POST /message:send`, `GET /tasks/{id}` ...).
    Rest,
    /// `lf.a2a.v1.A2AService` over gRPC ([`super::a2a_grpc`]).
    Grpc,
}

impl Binding {
    /// A card's name for a binding (1.0 `protocolBinding`, 0.3 `transport`); none is JSON-RPC.
    fn named(name: &Value) -> Option<Self> {
        let Some(name) = name.as_str() else {
            return name.is_null().then_some(Self::JsonRpc);
        };
        match name.to_ascii_uppercase().as_str() {
            "JSONRPC" => Some(Self::JsonRpc),
            "HTTP+JSON" | "REST" => Some(Self::Rest),
            "GRPC" => Some(Self::Grpc),
            _ => None,
        }
    }
}

/// One A2A method on the agent's binding: the JSON-RPC `result`, the HTTP+JSON reply, or the
/// gRPC reply written as that JSON. The three carry the same JSON (ProtoJSON of A2A 1.0), so
/// one reader serves them.
async fn a2a_call(
    binding: Binding,
    endpoint: &str,
    method: &str,
    params: Value,
    headers: &[(&str, String)],
) -> Result<Value, NotPosted> {
    match binding {
        Binding::JsonRpc => rpc(endpoint, method, params, headers).await,
        Binding::Grpc => super::a2a_grpc::call(endpoint, method, &params, headers).await,
        Binding::Rest => {
            let (verb, url, body) = rest_route(endpoint, method, &params)?;
            let text = body_of(send_json(verb, &url, body, headers, false).await?).await?;
            serde_json::from_str(&text)
                .map_err(|_| NotPosted::Unknown(format!("{method}: the reply is not JSON")))
        }
    }
}

/// The HTTP+JSON route of an A2A method (the HTTP annotations of A2A 1.0's `a2a.proto`): its
/// verb, address and body.
fn rest_route(
    endpoint: &str,
    method: &str,
    params: &Value,
) -> Result<(reqwest::Method, String, Option<String>), NotPosted> {
    let base = endpoint.trim_end_matches('/');
    // A task id is a path segment: anything but an unreserved character is escaped.
    let id = || {
        let id = text_of(&params["id"])
            .ok_or_else(|| NotPosted::Unsent(format!("{method}: no task id")))?;
        let escape = |c: char| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '.' | '_' | '~' => c.to_string(),
            _ => c
                .to_string()
                .bytes()
                .map(|byte| format!("%{byte:02X}"))
                .collect(),
        };
        Ok::<String, NotPosted>(id.chars().map(escape).collect())
    };
    let (post, get) = (reqwest::Method::POST, reqwest::Method::GET);
    let body = Some(params.to_string());
    Ok(match method {
        "SendMessage" => (post, format!("{base}/message:send"), body),
        "SendStreamingMessage" => (post, format!("{base}/message:stream"), body),
        "GetTask" => (get, format!("{base}/tasks/{}", id()?), None),
        "CancelTask" => (post, format!("{base}/tasks/{}:cancel", id()?), body),
        "GetExtendedAgentCard" => (get, format!("{base}/extendedAgentCard"), None),
        _ => {
            let error = format!("{method} has no HTTP+JSON route");
            return Err(NotPosted::Unsent(error));
        }
    })
}

struct A2a<'a> {
    backend: &'a Remote,
    id: Option<String>,
    /// The status text last passed on as progress.
    said: Option<String>,
    /// The task's context: a follow-up's is the one of the task it follows.
    context: Option<String>,
    /// What the agent's card says, when a session's start read it: its binding and where it
    /// is, the credential it takes (the entry's secret in the card's scheme, or the OAuth
    /// client whose token goes as a bearer), whether it streams.
    binding: Binding,
    endpoint: String,
    credential: Option<(String, String)>,
    oauth: Option<OAuthClient>,
    streams: bool,
    /// The open stream of the last message, and the task as it has streamed so far.
    events: Option<A2aEvents>,
    task: Value,
    /// The conversation state (chapter 9), sent as the message's `metadata.waav_state`.
    state: Option<Value>,
    /// The card says the agent pushes a task's updates to a webhook of the client's.
    pushes: bool,
    /// The push configuration sent with the task's first message, and the pushes that came
    /// to the callback route since.
    push: Option<Value>,
    pushed: Option<(mpsc::Receiver<Value>, Listening)>,
    /// The status messages whose controls were taken: a task read again shows them again.
    controls_taken: HashSet<String>,
}

/// How often a task the agent pushes the updates of is read all the same, in case a push
/// was lost.
const PUSH_CHECK: Duration = Duration::from_secs(5);

/// The text parts of an A2A message or artifact.
fn parts_text(holder: &Value) -> String {
    let parts = holder["parts"].as_array();
    let texts = parts.map(|parts| parts.iter().filter_map(|part| text_of(&part["text"])));
    texts
        .map(|texts| texts.collect::<Vec<_>>().join(" "))
        .unwrap_or_default()
}

/// The sign-in link of an A2A message: a part's `url`, a data part's `url`, or the first
/// `http(s)://` address in its text.
fn sign_in_link(message: &Value) -> Option<String> {
    let parts = message["parts"].as_array()?;
    let web = |url: &str| url.starts_with("https://") || url.starts_with("http://");
    let given = parts
        .iter()
        .flat_map(|part| [&part["url"], &part["data"]["url"]])
        .filter_map(Value::as_str)
        .find(|url| web(url));
    let written = || {
        let words = parts.iter().filter_map(|part| part["text"].as_str());
        let mut words = words.flat_map(str::split_whitespace);
        let trimmed = |word: &str| word.trim_end_matches(['.', ',', ')', ';']).to_owned();
        words.find(|word| web(word)).map(trimmed)
    };
    given.map(str::to_owned).or_else(written)
}

impl<'a> A2a<'a> {
    fn new(backend: &'a Remote, context: Option<String>) -> Self {
        let spec = &backend.spec;
        let card = discovered(spec);
        let card = card.as_deref().unwrap_or(&Value::Null);
        let (binding, endpoint) = card_binding(card, &spec.url);
        Self {
            backend,
            id: None,
            said: None,
            context,
            binding,
            endpoint,
            credential: spec
                .secret
                .as_deref()
                .map(|secret| card_credential(card, secret)),
            oauth: card_oauth(spec, card),
            streams: card["capabilities"]["streaming"] == true,
            events: None,
            task: json!({}),
            state: None,
            pushes: card["capabilities"]["pushNotifications"] == true,
            push: None,
            pushed: None,
            controls_taken: HashSet::new(),
        }
    }

    /// For an agent that does not stream and pushes its tasks' updates, with an entry that
    /// says where the gateway is reached (`push_base_url`): the task's first message carries
    /// a push configuration (the task's callback route and a token of its own), and the
    /// pushes that come there are read as the stream's events would be.
    fn with_pushes(mut self, job: &Job) -> Self {
        let Some(base) = self.backend.spec.push_base_url.as_deref() else {
            return self;
        };
        if !self.pushes || self.streams {
            return self;
        }
        let (task, revision) = job_task(job);
        let token = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let url = format!("{}/v1/tasks/{task}/callback", base.trim_end_matches('/'));
        self.push = Some(json!({ "url": url, "token": token }));
        let (listening, events) = Listening::open(&task, revision, Proof::Token(token));
        self.pushed = Some((events, listening));
        self
    }

    async fn headers(&self) -> Result<Vec<(&str, String)>, NotPosted> {
        a2a_headers(self.oauth.as_ref(), self.credential.as_ref()).await
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value, NotPosted> {
        let headers = self.headers().await?;
        let called = a2a_call(self.binding, &self.endpoint, method, params, &headers).await;
        if let (Err(error), Some(oauth)) = (&called, &self.oauth) {
            token_refused(oauth, error);
        }
        called
    }

    async fn send(&mut self, text: &str) -> Result<Vec<Step>, NotPosted> {
        let id = uuid::Uuid::new_v4().to_string();
        let mut message =
            json!({ "messageId": id, "role": "ROLE_USER", "parts": [{ "text": text }] });
        if let Some(task) = &self.id {
            message["taskId"] = json!(task);
        }
        let session = || self.backend.context.lock().clone();
        if let Some(context) = self.context.clone().or_else(session) {
            message["contextId"] = json!(context);
        }
        if let Some(state) = &self.state {
            message["metadata"] = json!({ "waav_state": state });
        }
        if self.streams {
            // The task's progress and its questions come on the reply as they happen.
            let configuration = json!({ "acceptedOutputModes": ["text/plain"] });
            let params = json!({ "message": message, "configuration": configuration });
            let method = "SendStreamingMessage";
            let headers = self.headers().await?;
            let streamed = match self.binding {
                Binding::Rest => rest_stream(&self.endpoint, &params, &headers)
                    .await
                    .map(|events| events.map(A2aEvents::Sse)),
                Binding::Grpc => super::a2a_grpc::stream(&self.endpoint, &params, &headers)
                    .await
                    .map(|events| Ok(A2aEvents::Grpc(events))),
                Binding::JsonRpc => rpc_stream(&self.endpoint, method, params, &headers)
                    .await
                    .map(|events| events.map(A2aEvents::Sse)),
            };
            if let (Err(error), Some(oauth)) = (&streamed, &self.oauth) {
                token_refused(oauth, error);
            }
            return match streamed? {
                Ok(events) => {
                    self.events = Some(events);
                    self.streamed().await
                }
                Err(result) => Ok(self.update(&result)),
            };
        }
        let mut configuration =
            json!({ "returnImmediately": true, "acceptedOutputModes": ["text/plain"] });
        if let Some(push) = self.push.as_ref().filter(|_| self.id.is_none()) {
            configuration["pushNotificationConfig"] = push.clone();
        }
        let params = json!({ "message": message, "configuration": configuration });
        let result = self.call("SendMessage", params).await?;
        Ok(self.read(&result))
    }

    /// The steps of the stream's next event. When the stream has ended (the task waits for
    /// input, or the agent closed it early), the task is read by polling from then on.
    async fn streamed(&mut self) -> Result<Vec<Step>, NotPosted> {
        let Some(events) = self.events.as_mut() else {
            return Ok(Vec::new());
        };
        let event = events.next().await;
        let Ok(Some(event)) = event else {
            self.events = None;
            return event.map(|_| Vec::new());
        };
        // An HTTP+JSON or gRPC stream's events are the results themselves.
        let result = match self.binding {
            Binding::Rest | Binding::Grpc => event,
            Binding::JsonRpc => rpc_result("SendStreamingMessage", event)?,
        };
        Ok(self.update(&result))
    }

    /// The steps of one streamed result: a task, a message, or an update of the task's
    /// status or of its artifacts (1.0 names the kind by its field, 0.3 by `kind`).
    fn update(&mut self, result: &Value) -> Vec<Step> {
        let kind = result["kind"].as_str().unwrap_or_default();
        let of = |field: &str, named: &str| result.get(field).or((kind == named).then_some(result));
        if let Some(update) = of("statusUpdate", "status-update") {
            for (from, to) in [("taskId", "id"), ("contextId", "contextId")] {
                if let Some(value) = text_of(&update[from]) {
                    self.task[to] = json!(value);
                }
            }
            self.task["status"] = update["status"].clone();
            let task = self.task.clone();
            return self.read(&task);
        }
        if let Some(update) = of("artifactUpdate", "artifact-update") {
            let artifact = &update["artifact"];
            if !self.task["artifacts"].is_array() {
                self.task["artifacts"] = json!([]);
            }
            let artifacts = self.task["artifacts"].as_array_mut();
            let artifacts = artifacts.unwrap_or_else(|| unreachable!("made an array above"));
            let same = |earlier: &&mut Value| earlier["artifactId"] == artifact["artifactId"];
            match artifacts.iter_mut().find(same) {
                Some(earlier) if update["append"] == true => {
                    let parts = artifact["parts"].as_array().cloned().unwrap_or_default();
                    if let Some(all) = earlier["parts"].as_array_mut() {
                        all.extend(parts);
                    }
                }
                _ => artifacts.push(artifact.clone()),
            }
            return Vec::new();
        }
        if kind == "message" {
            return self.read(&json!({ "message": result }));
        }
        let task = result.get("task").unwrap_or(result);
        if task.is_object() && task.get("status").is_some() {
            self.task = task.clone();
        }
        self.read(result)
    }

    /// The steps of a `SendMessage`, `GetTask` or `CancelTask` result: the controls its
    /// message asks for first.
    fn read(&mut self, result: &Value) -> Vec<Step> {
        let task = result.get("task").unwrap_or(result);
        let message = result.get("message").filter(|message| message.is_object());
        let message = message.unwrap_or(&task["status"]["message"]);
        let mut steps = self.controls(message);
        steps.extend(self.read_state(result));
        steps
    }

    /// The controls an agent's message asks for: each data part `{"type": "control", "kind",
    /// "value", "request_id"}`, once per message.
    fn controls(&mut self, message: &Value) -> Vec<Step> {
        let parts = message["parts"].as_array().into_iter().flatten();
        let asked = parts.filter(|part| part["data"]["type"] == "control");
        let mut steps = Vec::new();
        for (n, part) in asked.enumerate() {
            let id = text_of(&message["messageId"]).unwrap_or_else(|| part["data"].to_string());
            if self.controls_taken.insert(format!("{id}#{n}")) {
                steps.extend(control_step(&part["data"]));
            }
        }
        steps
    }

    fn read_state(&mut self, result: &Value) -> Vec<Step> {
        // An agent may answer a message outright, with no task.
        if let Some(message) = result.get("message").filter(|message| message.is_object()) {
            return vec![Step::Done(parts_text(message))];
        }
        let task = result.get("task").unwrap_or(result);
        if let Some(id) = text_of(&task["id"]) {
            self.id = Some(id);
        }
        if let Some(context) = text_of(&task["contextId"]) {
            *self.backend.context.lock() = Some(context.clone());
            self.context = Some(context);
        }
        // 1.0 writes `TASK_STATE_INPUT_REQUIRED`; earlier versions wrote `input-required`.
        let state = task["status"]["state"].as_str().unwrap_or_default();
        let state = state.to_ascii_lowercase().replace('-', "_");
        let said = parts_text(&task["status"]["message"]);
        let step = match state.trim_start_matches("task_state_") {
            "completed" => {
                let artifacts = task["artifacts"].as_array();
                let texts = artifacts.map(|all| all.iter().map(parts_text).collect::<Vec<_>>());
                let answer = texts.unwrap_or_default().join(" ");
                let done = Step::Done(if answer.trim().is_empty() {
                    said
                } else {
                    answer
                });
                // Data parts are the result's data; file and link parts its artifacts.
                let parts = artifacts.into_iter().flatten();
                let parts = parts.flat_map(|artifact| artifact["parts"].as_array().cloned());
                let (data, files): (Vec<Value>, Vec<Value>) = parts
                    .flatten()
                    .filter(|part| part.get("text").is_none())
                    .partition(|part| part.get("data").is_some());
                let mut data: Vec<Value> =
                    data.into_iter().map(|part| part["data"].clone()).collect();
                let data = match data.len() {
                    0 => None,
                    1 => data.pop(),
                    _ => Some(Value::Array(data)),
                };
                return output(data.as_ref(), files)
                    .into_iter()
                    .chain([done])
                    .collect();
            }
            state @ ("failed" | "rejected") => Step::Failed(if said.is_empty() {
                format!("the agent's task {state}")
            } else {
                said
            }),
            "canceled" | "cancelled" => Step::Cancelled,
            "input_required" if !said.is_empty() => {
                // A deadline has no standard field in A2A: the status message's metadata.
                let ms = task["status"]["message"]["metadata"]["expires_in_ms"].as_u64();
                with_deadline(Step::Ask(Ask::asking("", &said, &[])), ms)
            }
            "input_required" => Step::Failed("the agent asked for input without a question".into()),
            // A sign-in is given on the client's screen, through the link the agent sent.
            "auth_required" => match sign_in_link(&task["status"]["message"]) {
                Some(url) => {
                    let question = if said.is_empty() {
                        "Please sign in."
                    } else {
                        &said
                    };
                    let link = json!({ "url": url });
                    Step::Ask(Ask::on_screen("", HitlKind::AuthUrl, question, &link))
                }
                None => Step::Failed("the agent asks for a sign-in and sends no link".into()),
            },
            _ if !said.is_empty() && self.said.as_deref() != Some(&said) => {
                self.said = Some(said.clone());
                Step::Working(Some(said))
            }
            _ => return Vec::new(),
        };
        vec![step]
    }
}

#[async_trait::async_trait]
impl Wire for A2a<'_> {
    async fn create(
        &mut self,
        request: &str,
        _history: &[ChatMessage],
    ) -> Result<Vec<Step>, NotPosted> {
        self.send(request).await
    }

    async fn next(&mut self) -> Result<Vec<Step>, NotPosted> {
        if self.events.is_some() {
            return self.streamed().await;
        }
        let pushed = match self.pushed.as_mut() {
            Some((pushed, _)) => {
                let push = tokio::time::timeout(PUSH_CHECK, pushed.recv()).await;
                push.ok().flatten()
            }
            None => {
                tokio::time::sleep(POLL).await;
                None
            }
        };
        // A push is a task, a message, or a status or artifact update, as a stream's event.
        if let Some(push) = pushed {
            return Ok(self.update(&push));
        }
        let result = self.call("GetTask", json!({ "id": self.id })).await?;
        Ok(self.read(&result))
    }

    async fn answer(&mut self, ask: &Ask, decision: &GateDecision) -> Result<Vec<Step>, NotPosted> {
        self.said = None;
        let text = match decision {
            // The caller signed in through the link.
            GateDecision::Answer(_) if ask.kind == HitlKind::AuthUrl => "Signed in.".to_owned(),
            GateDecision::Answer(Value::String(text)) => text.clone(),
            GateDecision::Answer(value) => value.to_string(),
            GateDecision::Refuse(reason) => refused(reason),
            GateDecision::Allow(_) => "Yes.".to_owned(),
        };
        self.send(&text).await
    }

    async fn cancel(&mut self) -> Result<Vec<Step>, NotPosted> {
        self.events = None;
        let result = self.call("CancelTask", json!({ "id": self.id })).await?;
        Ok(self.read(&result))
    }

    fn reports_effects(&self) -> bool {
        false
    }

    fn handle(&self) -> Option<String> {
        self.context.clone()
    }

    /// A message to the task with one data part, the `control_applied`; the open stream, if
    /// any, stays the task's.
    async fn applied(&mut self, id: Option<&str>, applied: &Value) -> Result<Vec<Step>, NotPosted> {
        let mut data = applied.clone();
        data["type"] = json!("control_applied");
        if let Some(id) = id {
            data["request_id"] = json!(id);
        }
        let message_id = uuid::Uuid::new_v4().to_string();
        let mut message =
            json!({ "messageId": message_id, "role": "ROLE_USER", "parts": [{ "data": data }] });
        if let Some(task) = &self.id {
            message["taskId"] = json!(task);
        }
        if let Some(context) = &self.context {
            message["contextId"] = json!(context);
        }
        let configuration =
            json!({ "returnImmediately": true, "acceptedOutputModes": ["text/plain"] });
        let params = json!({ "message": message, "configuration": configuration });
        let result = self.call("SendMessage", params).await?;
        Ok(self.read(&result))
    }
}

// ---- MCP 2026-07-28 with the Tasks extension (io.modelcontextprotocol/tasks)

struct Mcp<'a> {
    backend: &'a Remote,
    id: Option<String>,
    poll: Duration,
    said: Option<String>,
    /// The input requests already answered.
    answered: HashSet<String>,
    /// The task id of the finished task this call follows up.
    follows: Option<String>,
    /// The open reply of a `tools/call` written as server-sent events: its progress
    /// notifications, then its result.
    events: Option<Events>,
    /// The conversation state (chapter 9), sent as `_meta["io.waav/state"]`.
    state: Option<Value>,
    /// What the server's GET stream says of this call (its progress token) and of its task.
    heard: Heard,
}

/// The method of an agent's notification that asks for a conversation control (chapter 9):
/// its `params` are `{"kind", "value", "request_id"}`, as a webhook's `control` event. A
/// tool's or a task's result may carry the same in `_meta["io.waav/control"]` (one or a list).
const CONTROL_NOTE: &str = "notifications/io.waav/control";
/// The gateway's answer to it: the `control_applied` event's fields and the `request_id`.
const CONTROL_APPLIED_NOTE: &str = "notifications/io.waav/control_applied";
/// Where a server's message names the task it is about (the Tasks extension).
const RELATED_TASK: &str = "io.modelcontextprotocol/related-task";

/// The calls that wait for what an entry's GET stream says, by the entry and the call's
/// progress token (`progress:<token>`) or its task (`task:<id>`).
static MCP_LISTENERS: LazyLock<Mutex<HashMap<String, mpsc::Sender<Value>>>> =
    LazyLock::new(Mutex::default);

/// A call's place among [`MCP_LISTENERS`], given up when the call ends.
struct Heard {
    entry: String,
    tx: mpsc::Sender<Value>,
    rx: mpsc::Receiver<Value>,
    keys: Vec<String>,
}

impl Heard {
    fn new(spec: &BackendSpec) -> Self {
        let (tx, rx) = mpsc::channel(16);
        Self {
            entry: discovery_key(spec),
            tx,
            rx,
            keys: Vec::new(),
        }
    }

    fn listen(&mut self, what: String) {
        let key = format!("{} {what}", self.entry);
        if !self.keys.contains(&key) {
            MCP_LISTENERS.lock().insert(key.clone(), self.tx.clone());
            self.keys.push(key);
        }
    }
}

impl Drop for Heard {
    fn drop(&mut self) {
        let mut listeners = MCP_LISTENERS.lock();
        for key in &self.keys {
            // A later call's entry under the same key is not this one's to remove.
            if listeners
                .get(key)
                .is_some_and(|tx| tx.same_channel(&self.tx))
            {
                listeners.remove(key);
            }
        }
    }
}

/// The call a message of the server's GET stream is about: the task its `_meta` names, a
/// task status's `taskId`, or a progress notification's token.
fn listener_of(entry: &str, event: &Value) -> Option<String> {
    let params = &event["params"];
    let task = text_of(&params["_meta"][RELATED_TASK]["taskId"]).or_else(|| {
        (event["method"] == "notifications/tasks/status")
            .then(|| text_of(&params["taskId"]))
            .flatten()
    });
    let what = match task {
        Some(task) => format!("task:{task}"),
        None => format!("progress:{}", params["progressToken"].as_str()?),
    };
    Some(format!("{entry} {what}"))
}

/// Reads the server's GET stream (MCP Streamable HTTP: what the server sends outside any
/// call's reply) while the handshake `session` is the entry's: a message about a call of a
/// session goes to that call; a request nobody waits for is refused (`-32601`), a `ping`
/// answered, a changed tool list makes the next session read it again. A server that offers
/// no GET stream (405, or any other refusal) is not asked again for this handshake.
async fn listen_get(spec: BackendSpec, session: McpSession) {
    let entry = discovery_key(&spec);
    let current = || {
        let sessions = MCP_SESSIONS.lock();
        sessions
            .get(&entry)
            .is_some_and(|(_, held)| *held == session)
    };
    let mut failures = 0;
    while current() && failures < 3 {
        let Ok(headers) = session_headers(&spec, &session).await else {
            return;
        };
        let reply = send_json(reqwest::Method::GET, &spec.url, None, &headers, true).await;
        let reply = match reply {
            Ok(reply) if reply.status().is_success() && is_stream(&reply) => reply,
            // 405 (the server has none), or any other answer: not asked again for this
            // handshake. A session the server forgot is found by the next call (404).
            Ok(reply) => {
                let status = reply.status();
                debug!(backend = %spec.name, %status, "interaction MCP server opens no GET stream");
                return;
            }
            Err(NotPosted::Unsent(error) | NotPosted::Unknown(error)) => {
                debug!(backend = %spec.name, %error, "interaction MCP GET stream not opened");
                failures += 1;
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }
        };
        failures = 0;
        debug!(backend = %spec.name, "interaction MCP GET stream open");
        let mut events = Events {
            reply,
            decoder: SseDecoder::default(),
            ready: VecDeque::new(),
        };
        while let Ok(Some(event)) = events.next().await {
            heard_on_get(&spec, &entry, &headers, event).await;
        }
        // The server closed it (or it was quiet for the read timeout): opened again shortly.
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn heard_on_get(
    spec: &BackendSpec,
    entry: &str,
    headers: &[(&'static str, String)],
    event: Value,
) {
    let method = event["method"].as_str().unwrap_or_default();
    if method == "notifications/tools/list_changed" {
        debug!(backend = %spec.name, "interaction MCP tool list changed; read again by the next session");
        DISCOVERED.lock().remove(entry);
        return;
    }
    let waiting =
        listener_of(entry, &event).and_then(|key| MCP_LISTENERS.lock().get(&key).cloned());
    if let Some(waiting) = waiting {
        let _ = waiting.try_send(event);
        return;
    }
    if !is_request(&event) {
        return;
    }
    let answer = if method == "ping" {
        json!({ "jsonrpc": "2.0", "id": event["id"], "result": {} })
    } else {
        debug!(backend = %spec.name, %method, "interaction MCP request refused: no call of a session waits for it");
        let message = "No caller can answer this: no call of a session waits for it.";
        let error = json!({ "code": NOT_SERVED, "message": message });
        json!({ "jsonrpc": "2.0", "id": event["id"], "error": error })
    };
    if let Err(NotPosted::Unsent(error) | NotPosted::Unknown(error)) =
        answer_request(&spec.url, headers, answer).await
    {
        debug!(backend = %spec.name, %error, "interaction MCP answer on the GET stream not delivered");
    }
}

/// The controls a tool's or a task's result asks for (`_meta["io.waav/control"]`).
fn result_controls(result: &Value) -> Vec<Step> {
    let asked = &result["_meta"]["io.waav/control"];
    let asked = match asked {
        Value::Array(all) => all.iter().collect(),
        Value::Object(_) => vec![asked],
        _ => Vec::new(),
    };
    asked.into_iter().filter_map(control_step).collect()
}

/// What the gateway says of itself to an MCP server.
fn mcp_client_info() -> Value {
    json!({ "name": "waav-gateway", "version": env!("CARGO_PKG_VERSION") })
}

fn mcp_client_capabilities() -> Value {
    json!({ "elicitation": {}, "extensions": { "io.modelcontextprotocol/tasks": {} } })
}

/// An MCP request's `_meta` and its method's headers, on the `base` headers of every request
/// to the server ([`mcp_base`]). Each request also says which version and capabilities it
/// comes with, as 2026-07-28 asks.
fn with_method(
    mut headers: Vec<(&'static str, String)>,
    method: &str,
    params: &mut Value,
) -> Vec<(&'static str, String)> {
    params["_meta"] = json!({
        "io.modelcontextprotocol/protocolVersion": MCP_VERSION,
        "io.modelcontextprotocol/clientInfo": mcp_client_info(),
        "io.modelcontextprotocol/clientCapabilities": mcp_client_capabilities(),
    });
    headers.push(("Mcp-Method", method.to_owned()));
    if let Some(name) = params["name"].as_str() {
        headers.push(("Mcp-Name", name.to_owned()));
    }
    headers
}

/// The headers of a request to the entry's server: after `initialize`, with the agreed
/// version and the server's session.
async fn mcp_headers(
    spec: &BackendSpec,
    method: &str,
    params: &mut Value,
) -> Result<Vec<(&'static str, String)>, NotPosted> {
    Ok(with_method(mcp_base(spec).await?, method, params))
}

/// The headers every request to the entry's server carries: the protocol version and the
/// session `initialize` agreed, and the credential (the entry's secret, or its OAuth token).
async fn mcp_base(spec: &BackendSpec) -> Result<Vec<(&'static str, String)>, NotPosted> {
    let session = mcp_session(spec).await;
    session_headers(spec, &session).await
}

async fn session_headers(
    spec: &BackendSpec,
    session: &McpSession,
) -> Result<Vec<(&'static str, String)>, NotPosted> {
    let mut headers = vec![
        ("MCP-Protocol-Version", session.version.clone()),
        ("Accept", "application/json, text/event-stream".to_owned()),
    ];
    if let Some(id) = &session.id {
        headers.push(("Mcp-Session-Id", id.clone()));
    }
    match &spec.oauth {
        Some(oauth) => {
            let token = access_token(oauth).await?;
            headers.push(("Authorization", format!("Bearer {token}")));
        }
        None => headers.extend(bearer(spec)),
    }
    Ok(headers)
}

// ---- the MCP handshake

/// What `initialize` agreed with a server.
#[derive(Debug, Clone, PartialEq)]
struct McpSession {
    /// The protocol version the server answered with.
    version: String,
    /// `Mcp-Session-Id`, when the server keeps a session.
    id: Option<String>,
    /// The server's capabilities.
    capabilities: Value,
}

impl McpSession {
    /// A server that does not take `initialize`: every request stands alone.
    fn stateless() -> Self {
        Self {
            version: MCP_VERSION.to_owned(),
            id: None,
            capabilities: Value::Null,
        }
    }
}

/// The handshake of each entry's server ([`discovery_key`]), and when it was made: shared by
/// the sessions of this process for the entry's `catalog_ttl_s`.
static MCP_SESSIONS: LazyLock<Mutex<HashMap<String, (Instant, McpSession)>>> =
    LazyLock::new(Mutex::default);

/// The server's session: the one this process made within `catalog_ttl_s`, else a new
/// `initialize`. A server that refuses it is used without a session; one that cannot be
/// reached is asked again by the next request.
async fn mcp_session(spec: &BackendSpec) -> McpSession {
    let ttl = spec.catalog_ttl_s.map_or(CATALOG_TTL, Duration::from_secs);
    let known = MCP_SESSIONS.lock().get(&discovery_key(spec)).cloned();
    if let Some((_, session)) = known.filter(|(at, _)| at.elapsed() < ttl) {
        return session;
    }
    let mut initialized = false;
    let session = match initialize(spec).await {
        Ok(session) => {
            initialized = true;
            session
        }
        Err(NotPosted::Unsent(error)) => {
            debug!(backend = %spec.name, %error, "interaction MCP server refused initialize; requests stand alone");
            McpSession::stateless()
        }
        Err(NotPosted::Unknown(error)) => {
            debug!(backend = %spec.name, %error, "interaction MCP initialize failed");
            return McpSession::stateless();
        }
    };
    let made = (Instant::now(), session.clone());
    MCP_SESSIONS.lock().insert(discovery_key(spec), made);
    // What the server sends outside the calls' replies, while this handshake is the entry's.
    if initialized {
        tokio::spawn(listen_get(spec.clone(), session.clone()));
    }
    session
}

/// `initialize` (the version and capabilities of both sides), then
/// `notifications/initialized`.
async fn initialize(spec: &BackendSpec) -> Result<McpSession, NotPosted> {
    let mut params = json!({
        "protocolVersion": MCP_VERSION,
        "capabilities": mcp_client_capabilities(),
        "clientInfo": mcp_client_info(),
    });
    let first = session_headers(spec, &McpSession::stateless()).await?;
    let headers = with_method(first, "initialize", &mut params);
    let body = rpc_body("initialize", params);
    let reply = send_json(reqwest::Method::POST, &spec.url, Some(body), &headers, true).await?;
    let id = reply.headers().get("mcp-session-id");
    let id = id.and_then(|id| id.to_str().ok()).map(str::to_owned);
    let result = unattended(reply, "initialize", &spec.url, &headers).await?;
    let session = McpSession {
        version: text_of(&result["protocolVersion"]).unwrap_or_else(|| MCP_VERSION.to_owned()),
        id,
        capabilities: result["capabilities"].clone(),
    };
    info!(backend = %spec.name, version = %session.version, session = session.id.is_some(), "interaction MCP server initialized");
    let mut headers = session_headers(spec, &session).await?;
    headers.push(("Mcp-Method", "notifications/initialized".to_owned()));
    let note = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
    // A notification has no answer worth reading; a server that refuses it still serves.
    if let Ok(reply) = post_json(&spec.url, note.to_string(), &headers).await {
        let _ = body_of(reply).await;
    }
    Ok(session)
}

/// A request that failed: a session the server no longer knows (404) is made again, and a
/// token it refused (401) is fetched again, by the next request.
fn mcp_failed(spec: &BackendSpec, error: &NotPosted) {
    let NotPosted::Unsent(text) = error else {
        return;
    };
    if text.starts_with("HTTP 404") {
        MCP_SESSIONS.lock().remove(&discovery_key(spec));
    } else if let Some(oauth) = &spec.oauth {
        token_refused(oauth, error);
    }
}

/// A request the server refused for its credential (HTTP 401, gRPC `Unauthenticated`): the
/// OAuth client's token is fetched again by the next request.
fn token_refused(oauth: &OAuthClient, error: &NotPosted) {
    let NotPosted::Unsent(text) = error else {
        return;
    };
    if text.starts_with("HTTP 401") || text.starts_with("gRPC Unauthenticated") {
        TOKENS.lock().remove(&token_key(oauth));
    }
}

/// JSON-RPC's code for a request the client does not serve.
const NOT_SERVED: i64 = -32601;

/// The result of a reply no session waits on (`initialize`, `tools/list`). A request of the
/// server's written on it (an elicitation) has no caller to ask: it is refused with a
/// JSON-RPC error, so the server goes on.
async fn unattended(
    reply: reqwest::Response,
    method: &str,
    url: &str,
    headers: &[(&'static str, String)],
) -> Result<Value, NotPosted> {
    if !(reply.status().is_success() && is_stream(&reply)) {
        let text = body_of(reply).await?;
        return rpc_reply(method, &text);
    }
    let mut events = Events {
        reply,
        decoder: SseDecoder::default(),
        ready: VecDeque::new(),
    };
    while let Some(event) = events.next().await? {
        if is_request(&event) {
            debug!(method = %event["method"], "interaction MCP request refused: no call of a session waits for it");
            let message = "No caller can answer this: it did not come with a call of a session.";
            let error = json!({ "code": NOT_SERVED, "message": message });
            let refusal = json!({ "jsonrpc": "2.0", "id": event["id"], "error": error });
            answer_request(url, headers, refusal).await?;
        } else if event.get("result").is_some() || event.get("error").is_some() {
            return rpc_result(method, event);
        }
    }
    Err(NotPosted::Unknown(format!(
        "{method}: the reply ended without a result"
    )))
}

/// A request of the server's to the gateway (it has an `id` and a `method`).
fn is_request(event: &Value) -> bool {
    event.get("method").is_some() && event.get("id").is_some_and(|id| !id.is_null())
}

/// The gateway's answer to a request of the server's, with the headers of a request of the
/// gateway's minus those that name its method.
async fn answer_request(
    url: &str,
    headers: &[(&'static str, String)],
    answer: Value,
) -> Result<(), NotPosted> {
    let headers: Vec<(&str, String)> = headers
        .iter()
        .filter(|(name, _)| !matches!(*name, "Mcp-Method" | "Mcp-Name"))
        .cloned()
        .collect();
    body_of(post_json(url, answer.to_string(), &headers).await?).await?;
    Ok(())
}

// ---- OAuth 2.0 client credentials (an MCP entry's `oauth`)

/// A token is renewed this long before it expires.
const TOKEN_MARGIN: Duration = Duration::from_secs(60);
/// How long a token whose lifetime the server does not give is used.
const TOKEN_LIFE: Duration = Duration::from_secs(300);

/// The access token of each OAuth client, and when it is renewed. The token is never
/// logged.
static TOKENS: LazyLock<Mutex<HashMap<String, (Instant, String)>>> = LazyLock::new(Mutex::default);

/// A token is for its client, scope and resource: another entry of the same client that asks
/// for other scopes gets its own.
fn token_key(oauth: &OAuthClient) -> String {
    let (scope, resource) = (oauth.scope.as_deref(), oauth.resource.as_deref());
    format!(
        "{} {} {} {}",
        oauth.token_url,
        oauth.client_id,
        scope.unwrap_or_default(),
        resource.unwrap_or_default()
    )
}

/// The client's access token: the one this process holds until shortly before it expires,
/// else a new one from the token endpoint (client credentials grant, HTTP Basic client
/// authentication).
async fn access_token(oauth: &OAuthClient) -> Result<String, NotPosted> {
    let key = token_key(oauth);
    let held = TOKENS.lock().get(&key).cloned();
    if let Some((_, token)) = held.filter(|(renew, _)| Instant::now() < *renew) {
        return Ok(token);
    }
    let Some(secret) = oauth.client_secret.as_deref() else {
        let error = format!("{} is not set", oauth.client_secret_env);
        return Err(NotPosted::Unsent(error));
    };
    let form = {
        let mut form = url::form_urlencoded::Serializer::new(String::new());
        form.append_pair("grant_type", "client_credentials");
        for (name, value) in [("scope", &oauth.scope), ("resource", &oauth.resource)] {
            if let Some(value) = value {
                form.append_pair(name, value);
            }
        }
        form.finish()
    };
    let encode =
        |text: &str| url::form_urlencoded::byte_serialize(text.as_bytes()).collect::<String>();
    let basic = format!("{}:{}", encode(&oauth.client_id), encode(secret));
    let basic = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, basic);
    let headers = [
        ("Authorization", format!("Basic {basic}")),
        (
            "Content-Type",
            "application/x-www-form-urlencoded".to_owned(),
        ),
        ("Accept", "application/json".to_owned()),
    ];
    let mut reply = send_json(
        reqwest::Method::POST,
        &oauth.token_url,
        Some(form),
        &headers,
        false,
    )
    .await?;
    let status = reply.status();
    let body = read_capped(&mut reply, REPLY_MAX_BYTES).await;
    let body: Value = serde_json::from_str(&body).unwrap_or_default();
    // Only the error's code is shown: the rest of a reply may carry what is secret.
    let token = text_of(&body["access_token"]).filter(|_| status.is_success());
    let Some(token) = token else {
        let code = text_of(&body["error"]).unwrap_or_else(|| "no access_token".to_owned());
        let error = format!("the token endpoint answered HTTP {status}: {code}");
        return Err(if status.is_server_error() {
            NotPosted::Unknown(error)
        } else {
            NotPosted::Unsent(error)
        });
    };
    let life = body["expires_in"].as_u64();
    let renew = Instant::now() + renew_in(life);
    debug!(client = %oauth.client_id, life_s = ?life, "interaction OAuth token fetched");
    TOKENS.lock().insert(key, (renew, token.clone()));
    Ok(token)
}

/// When a token that lives `expires_in` seconds (the server's word, else [`TOKEN_LIFE`]) is
/// renewed: [`TOKEN_MARGIN`] before it expires, a short-lived one half way through its life.
fn renew_in(expires_in: Option<u64>) -> Duration {
    let life = expires_in.map_or(TOKEN_LIFE, Duration::from_secs);
    life.saturating_sub(TOKEN_MARGIN).max(life / 2)
}

/// What a progress notification says: its message, else how far it is.
fn progress_text(params: &Value) -> Option<String> {
    let given = text_of(&params["message"]);
    let of = |total: f64| {
        let done = params["progress"].as_f64()? / total * 100.0;
        (total > 0.0).then(|| format!("{:.0}% done.", done.clamp(0.0, 100.0)))
    };
    given.or_else(|| of(params["total"].as_f64()?))
}

/// The arguments an orchestrator gave a server's tool: the JSON object after the tool's name
/// in the child task's request (`ToolCalls::dispatch`).
fn tool_arguments(request: &str) -> Option<Value> {
    let (_, given) = request.split_once(' ')?;
    serde_json::from_str::<Value>(given)
        .ok()
        .filter(Value::is_object)
}

impl Mcp<'_> {
    async fn call(&self, method: &str, mut params: Value) -> Result<Value, NotPosted> {
        let spec = &self.backend.spec;
        let headers = mcp_headers(spec, method, &mut params).await?;
        let result = rpc(&spec.url, method, params, &headers).await;
        if let Err(error) = &result {
            mcp_failed(spec, error);
        }
        result
    }

    /// The next of what the open reply of the call says: a progress notification, or its
    /// result.
    async fn streamed(&mut self) -> Result<Vec<Step>, NotPosted> {
        let Some(events) = self.events.as_mut() else {
            return Ok(Vec::new());
        };
        loop {
            let event = match events.next().await {
                Ok(Some(event)) => event,
                ended => {
                    self.events = None;
                    let lost = "tools/call: the reply ended without a result".to_owned();
                    return Err(ended.err().unwrap_or(NotPosted::Unknown(lost)));
                }
            };
            if event["method"] == "notifications/progress" {
                if let Some(text) = progress_text(&event["params"]) {
                    return Ok(vec![Step::Working(Some(text))]);
                }
            } else if event["method"] == CONTROL_NOTE {
                let control = control_step(&event["params"]);
                if control.is_some() {
                    return Ok(control.into_iter().collect());
                }
            } else if is_request(&event) {
                // A request of the server's on the call's own reply, outside any MCP task:
                // the call is this session's, so its caller answers an elicitation.
                let key = format!("rpc:{}", event["id"]);
                if let Some(ask) = elicitation(&key, &event) {
                    return Ok(vec![Step::Ask(ask)]);
                }
                let answer = if event["method"] == "elicitation/create" {
                    json!({ "jsonrpc": "2.0", "id": event["id"], "result": { "action": "decline" } })
                } else {
                    let error =
                        json!({ "code": NOT_SERVED, "message": "Not served by this client." });
                    json!({ "jsonrpc": "2.0", "id": event["id"], "error": error })
                };
                let spec = &self.backend.spec;
                answer_request(&spec.url, &mcp_base(spec).await?, answer).await?;
            } else if event.get("result").is_some() || event.get("error").is_some() {
                self.events = None;
                let result = rpc_result("tools/call", event)?;
                return self.read(&result).await;
            }
        }
    }

    /// A call of one of the server's tools (an orchestrator's agent tool) changes something
    /// the caller should know of: once it has ended, it did.
    fn ran(&self, steps: Vec<Step>) -> Vec<Step> {
        let spec = &self.backend.spec;
        let done = steps.iter().any(|step| matches!(step, Step::Done(_)));
        if !self.direct() || spec.side_effects == SideEffects::None || !done {
            return steps;
        }
        let tool = spec.tool.as_deref().unwrap_or("the tool");
        let ran = Step::Effect(format!("ran {}", tool.replace('_', " ")));
        std::iter::once(ran).chain(steps).collect()
    }

    /// One of the server's tools, called with the arguments its orchestrator gave.
    fn direct(&self) -> bool {
        !self.backend.spec.tools.is_empty()
    }

    /// The steps of a task as `tools/call` or `tasks/get` returned it. An input request the
    /// caller cannot answer by voice is declined on the spot.
    async fn read(&mut self, task: &Value) -> Result<Vec<Step>, NotPosted> {
        if let Some(id) = text_of(&task["taskId"]) {
            // What the server's GET stream says of the task comes to this call.
            self.heard.listen(format!("task:{id}"));
            self.id = Some(id);
            if let Some(ms) = task["pollIntervalMs"].as_u64() {
                self.poll = Duration::from_millis(ms.clamp(POLL_BOUNDS.0, POLL_BOUNDS.1));
            }
        } else if task.get("status").is_none() {
            // The server answered the call outright; the controls it asks for come first.
            let steps = tool_output(task).into_iter().chain([tool_result(task)]);
            let steps = result_controls(task)
                .into_iter()
                .chain(self.ran(steps.collect()));
            return Ok(steps.collect());
        }
        let said = text_of(&task["statusMessage"]);
        let step = match task["status"].as_str().unwrap_or_default() {
            "completed" => {
                let result = &task["result"];
                let steps = tool_output(result).into_iter().chain([tool_result(result)]);
                let steps = result_controls(result)
                    .into_iter()
                    .chain(self.ran(steps.collect()));
                return Ok(steps.collect());
            }
            "failed" => {
                let error = text_of(&task["error"]["message"]).or(said);
                Step::Failed(error.unwrap_or_else(|| "the tool's task failed".to_owned()))
            }
            "cancelled" => Step::Cancelled,
            "input_required" => {
                let requests = task["inputRequests"].as_object();
                let open = requests.and_then(|requests| {
                    requests
                        .iter()
                        .find(|(key, _)| !self.answered.contains(*key))
                });
                let Some((key, request)) = open else {
                    return Ok(Vec::new());
                };
                match elicitation(key, request) {
                    Some(ask) => Step::Ask(ask),
                    None => {
                        self.respond(key, json!({ "action": "decline" })).await?;
                        return Ok(Vec::new());
                    }
                }
            }
            _ if said.is_some() && self.said != said => {
                self.said = said.clone();
                Step::Working(said)
            }
            _ => return Ok(Vec::new()),
        };
        Ok(vec![step])
    }

    /// The answer to an input request: of the server's task with `tasks/update`, or of a
    /// request on the call's reply (`rpc:<id>`) as its JSON-RPC response.
    async fn respond(&mut self, key: &str, response: Value) -> Result<(), NotPosted> {
        if let Some(id) = key.strip_prefix("rpc:") {
            let id: Value = serde_json::from_str(id).unwrap_or_default();
            let answer = json!({ "jsonrpc": "2.0", "id": id, "result": response });
            let spec = &self.backend.spec;
            answer_request(&spec.url, &mcp_base(spec).await?, answer).await?;
        } else {
            let params = json!({ "taskId": self.id, "inputResponses": { key: response } });
            self.call("tasks/update", params).await?;
        }
        self.answered.insert(key.to_owned());
        Ok(())
    }

    /// The steps of a message of the server's GET stream about this call: a progress
    /// notification, a control, an elicitation for the caller (answered as the JSON-RPC
    /// response of its `id`). `None`: the task is read now (a status notification, or a
    /// message that says nothing the gateway reads).
    async fn heard_steps(&mut self, event: &Value) -> Result<Option<Vec<Step>>, NotPosted> {
        let method = event["method"].as_str().unwrap_or_default();
        if method == "notifications/progress" {
            let text = progress_text(&event["params"]);
            return Ok(text.map(|text| vec![Step::Working(Some(text))]));
        }
        if method == CONTROL_NOTE {
            return Ok(control_step(&event["params"]).map(|step| vec![step]));
        }
        if !is_request(event) {
            return Ok(None);
        }
        let key = format!("rpc:{}", event["id"]);
        if let Some(ask) = elicitation(&key, event) {
            return Ok(Some(vec![Step::Ask(ask)]));
        }
        let answer = if method == "elicitation/create" {
            json!({ "action": "decline" })
        } else {
            let error = json!({ "code": NOT_SERVED, "message": "Not served by this client." });
            let refusal = json!({ "jsonrpc": "2.0", "id": event["id"], "error": error });
            let spec = &self.backend.spec;
            answer_request(&spec.url, &mcp_base(spec).await?, refusal).await?;
            return Ok(Some(Vec::new()));
        };
        self.respond(&key, answer).await?;
        Ok(Some(Vec::new()))
    }
}

/// A tool's result besides its text: its `structuredContent`, and the content that is no
/// text (resource links, embedded resources, images). Not for a failed call.
fn tool_output(result: &Value) -> Option<Step> {
    if result["isError"] == true {
        return None;
    }
    let content = result["content"].as_array().into_iter().flatten();
    let artifacts = content
        .filter(|item| item["type"] != "text")
        .cloned()
        .collect();
    output(result.get("structuredContent"), artifacts)
}

/// A tool's result as the task's answer: its text content.
fn tool_result(result: &Value) -> Step {
    let content = result["content"].as_array();
    let texts = content.map(|all| all.iter().filter_map(|item| text_of(&item["text"])));
    let text = texts
        .map(|texts| texts.collect::<Vec<_>>().join(" "))
        .unwrap_or_default();
    if result["isError"] == true {
        Step::Failed(if text.is_empty() {
            "the tool failed".to_owned()
        } else {
            text
        })
    } else {
        Step::Done(text)
    }
}

/// A form elicitation as a question for the caller: one field is asked by voice, with its
/// message, and answered into that field; several are a form for the client's screen. A URL
/// elicitation is a sign-in link for the client's screen. `None` for any other request.
fn elicitation(key: &str, request: &Value) -> Option<Ask> {
    let mut ask = elicited(key, request)?;
    // A deadline the server gave the question, in the request's `_meta`.
    if let Some(ms) = request["params"]["_meta"]["expires_in_ms"].as_u64() {
        ask.args["expires_in_ms"] = json!(ms);
    }
    Some(ask)
}

fn elicited(key: &str, request: &Value) -> Option<Ask> {
    let params = &request["params"];
    if request["method"] != "elicitation/create" {
        return None;
    }
    if params["mode"] == "url" {
        let question = text_of(&params["message"])?;
        text_of(&params["url"])?;
        return Some(Ask::on_screen(key, HitlKind::AuthUrl, &question, params));
    }
    let question = text_of(&params["message"])?;
    let fields = params["requestedSchema"]["properties"].as_object()?;
    if fields.len() > 1 {
        let form = json!({ "schema": params["requestedSchema"] });
        return Some(Ask::on_screen(key, HitlKind::Form, &question, &form));
    }
    let (name, field) = fields.iter().next()?;
    let yes_no = field["type"] == "boolean";
    let options: Vec<String> = if yes_no {
        vec!["yes".to_owned(), "no".to_owned()]
    } else {
        let listed = field["enum"].as_array();
        listed
            .map(|all| all.iter().filter_map(text_of).collect())
            .unwrap_or_default()
    };
    let mut ask = Ask::asking(key, &question, &options);
    ask.field = Some((name.clone(), yes_no));
    Some(ask)
}

#[async_trait::async_trait]
impl Wire for Mcp<'_> {
    async fn create(
        &mut self,
        request: &str,
        _history: &[ChatMessage],
    ) -> Result<Vec<Step>, NotPosted> {
        let spec = &self.backend.spec;
        let Some(tool) = &spec.tool else {
            let error = format!("{} lists tools for an orchestrator only", spec.label());
            return Ok(vec![Step::Failed(error)]);
        };
        let argument = spec.argument.as_deref().unwrap_or("request");
        let given = self.direct().then(|| tool_arguments(request)).flatten();
        let arguments = given.unwrap_or_else(|| json!({ argument: request }));
        let mut params = json!({ "name": tool, "arguments": arguments });
        let method = "tools/call";
        let headers = mcp_headers(spec, method, &mut params).await?;
        if let Some(follows) = &self.follows {
            params["_meta"]["io.waav/follows"] = json!(follows);
        }
        if let Some(state) = &self.state {
            params["_meta"]["io.waav/state"] = state.clone();
        }
        // Progress of a call the server runs on the reply comes as notifications on it (or on
        // the server's GET stream, by the token).
        let token = uuid::Uuid::new_v4().simple().to_string();
        self.heard.listen(format!("progress:{token}"));
        params["_meta"]["progressToken"] = json!(token);
        let called = rpc_stream(&spec.url, method, params, &headers).await;
        if let Err(error) = &called {
            mcp_failed(spec, error);
        }
        match called? {
            Ok(events) => {
                self.events = Some(events);
                self.streamed().await
            }
            Err(result) => self.read(&result).await,
        }
    }

    async fn next(&mut self) -> Result<Vec<Step>, NotPosted> {
        if self.events.is_some() {
            return self.streamed().await;
        }
        if self.id.is_none() {
            return Err(NotPosted::Unknown(
                "the call has no task to read".to_owned(),
            ));
        }
        // The next poll, unless the server's GET stream says something of the task first.
        let heard = tokio::time::timeout(self.poll, self.heard.rx.recv()).await;
        if let Ok(Some(event)) = heard
            && let Some(steps) = self.heard_steps(&event).await?
        {
            return Ok(steps);
        }
        let task = self.call("tasks/get", json!({ "taskId": self.id })).await?;
        self.read(&task).await
    }

    async fn answer(&mut self, ask: &Ask, decision: &GateDecision) -> Result<Vec<Step>, NotPosted> {
        self.said = None;
        let response = match (decision, &ask.field) {
            (GateDecision::Answer(value), Some((field, yes_no))) => {
                let value = if *yes_no {
                    json!(*value == "yes")
                } else {
                    value.clone()
                };
                json!({ "action": "accept", "content": { field: value } })
            }
            // The caller opened the link and says so.
            (GateDecision::Answer(_), None) if ask.kind == HitlKind::AuthUrl => {
                json!({ "action": "accept" })
            }
            // The form's fields, as the client filled them in.
            (GateDecision::Answer(fields @ Value::Object(_)), None)
                if ask.kind == HitlKind::Form =>
            {
                json!({ "action": "accept", "content": fields })
            }
            (GateDecision::Answer(_), None) if ask.kind == HitlKind::Form => {
                json!({ "action": "decline" })
            }
            _ if outcome(decision) == "decline" => json!({ "action": "decline" }),
            _ => json!({ "action": "cancel" }),
        };
        self.respond(&ask.id, response).await?;
        Ok(Vec::new())
    }

    /// Cancellation is cooperative: the server takes the request, and the task says later
    /// how it ended.
    async fn cancel(&mut self) -> Result<Vec<Step>, NotPosted> {
        // A call with no task cannot be stopped by name: how it ended is not known.
        self.events = None;
        if self.id.is_none() {
            return Ok(Vec::new());
        }
        self.call("tasks/cancel", json!({ "taskId": self.id }))
            .await?;
        loop {
            let steps = self.next().await?;
            let over =
                |step: &Step| matches!(step, Step::Cancelled | Step::Done(_) | Step::Failed(_));
            if steps.iter().any(over) {
                return Ok(steps);
            }
        }
    }

    /// A call of one of the server's tools says what it changed: it ran.
    fn reports_effects(&self) -> bool {
        self.direct()
    }

    fn handle(&self) -> Option<String> {
        self.id.clone()
    }

    /// The `control_applied` of a control the server asked for, as a notification of the
    /// gateway's ([`CONTROL_APPLIED_NOTE`]).
    async fn applied(&mut self, id: Option<&str>, applied: &Value) -> Result<Vec<Step>, NotPosted> {
        let mut params = applied.clone();
        if let Some(id) = id {
            params["request_id"] = json!(id);
        }
        if let Some(task) = &self.id {
            params["_meta"] = json!({ RELATED_TASK: { "taskId": task } });
        }
        let note = json!({ "jsonrpc": "2.0", "method": CONTROL_APPLIED_NOTE, "params": params });
        let spec = &self.backend.spec;
        let mut headers = mcp_base(spec).await?;
        headers.push(("Mcp-Method", CONTROL_APPLIED_NOTE.to_owned()));
        body_of(post_json(&spec.url, note.to_string(), &headers).await?).await?;
        Ok(Vec::new())
    }
}

// ---- what an agent says of itself: an A2A agent's card, an MCP server's tools

/// How long a card or a tool list serves later sessions, unless the entry says
/// (`catalog_ttl_s`).
const CATALOG_TTL: Duration = Duration::from_secs(300);
/// How long a session's start waits for the cards and tool lists it reads.
const DISCOVER_TIMEOUT: Duration = Duration::from_secs(3);
/// The most of a card's words an orchestrator's tool description takes.
const DESCRIPTION_MAX_CHARS: usize = 1_000;
/// Pages of `tools/list` read at most.
const TOOL_PAGES: usize = 5;

/// What the entries' servers said of themselves, by entry: when, and what.
static DISCOVERED: LazyLock<Mutex<HashMap<String, (Instant, Arc<Value>)>>> =
    LazyLock::new(Mutex::default);

/// An entry's place in what is kept per server: its name too, as two entries of one server
/// may hold different credentials (an extended card, an MCP session or tool list read with
/// one is not the other's).
fn discovery_key(spec: &BackendSpec) -> String {
    format!("{} {} {}", spec.kind.as_str(), spec.name, spec.url)
}

/// An entry an orchestrator reads at a session's start: an A2A agent (its card), an MCP
/// server whose tools it calls (its tool list).
fn discovers(spec: &BackendSpec) -> bool {
    spec.kind == BackendKind::A2a || (spec.kind == BackendKind::Mcp && !spec.tools.is_empty())
}

/// What the entry's server said of itself, while it is fresh (`catalog_ttl_s`).
fn discovered(spec: &BackendSpec) -> Option<Arc<Value>> {
    let ttl = spec.catalog_ttl_s.map_or(CATALOG_TTL, Duration::from_secs);
    let found = DISCOVERED.lock().get(&discovery_key(spec)).cloned()?;
    (found.0.elapsed() < ttl).then_some(found.1)
}

/// Reads the card of each A2A agent and the tool list of each MCP server of `specs` that no
/// session of this process read within its `catalog_ttl_s`, all at once and for at most
/// [`DISCOVER_TIMEOUT`]. What could not be read is not kept: the entry works as configured,
/// and the next session tries again.
pub async fn discover(specs: &[&BackendSpec]) {
    let unread = specs
        .iter()
        .filter(|spec| discovers(spec) && discovered(spec).is_none());
    let reads: Vec<_> = unread
        .map(|spec| async move {
            let read = match spec.kind {
                BackendKind::A2a => read_card(spec).await,
                _ => list_tools(spec).await,
            };
            match read {
                Ok(found) => {
                    let found = (Instant::now(), Arc::new(found));
                    DISCOVERED.lock().insert(discovery_key(spec), found);
                }
                Err(NotPosted::Unsent(error) | NotPosted::Unknown(error)) => {
                    debug!(backend = %spec.name, %error, "interaction agent not discovered");
                }
            }
        })
        .collect();
    if reads.is_empty() {
        return;
    }
    let all = futures::future::join_all(reads);
    if tokio::time::timeout(DISCOVER_TIMEOUT, all).await.is_err() {
        debug!("interaction agents: some were not discovered in time");
    }
}

/// An A2A agent's card, at `/.well-known/agent-card.json` under the entry's url.
async fn read_card(spec: &BackendSpec) -> Result<Value, NotPosted> {
    let url = format!(
        "{}/.well-known/agent-card.json",
        spec.url.trim_end_matches('/')
    );
    let reply = send_json(reqwest::Method::GET, &url, None, &[], false).await?;
    let text = body_of(reply).await?;
    let card = serde_json::from_str::<Value>(&text).ok();
    let card = card
        .filter(Value::is_object)
        .ok_or_else(|| NotPosted::Unknown("the agent card is not a JSON object".to_owned()))?;
    Ok(extended_card(spec, &card).await.unwrap_or(card))
}

/// The agent's authenticated extended card, when its public card says it has one and the
/// entry has a credential: asked for with it (1.0 `GetExtendedAgentCard`, 0.3
/// `agent/getAuthenticatedExtendedCard`) at the card's JSON-RPC address. `None` keeps the
/// public card.
async fn extended_card(spec: &BackendSpec, card: &Value) -> Option<Value> {
    let credential = spec
        .secret
        .as_deref()
        .map(|secret| card_credential(card, secret));
    let oauth = card_oauth(spec, card);
    if credential.is_none() && oauth.is_none() {
        return None;
    }
    let method = if card["supportsAuthenticatedExtendedCard"] == true {
        "agent/getAuthenticatedExtendedCard"
    } else if card["capabilities"]["extendedAgentCard"] == true
        || card["supportsExtendedAgentCard"] == true
    {
        "GetExtendedAgentCard"
    } else {
        return None;
    };
    let (binding, endpoint) = card_binding(card, &spec.url);
    let headers = match a2a_headers(oauth.as_ref(), credential.as_ref()).await {
        Ok(headers) => headers,
        Err(NotPosted::Unsent(error) | NotPosted::Unknown(error)) => {
            debug!(backend = %spec.name, %error, "interaction agent's extended card not asked for");
            return None;
        }
    };
    match a2a_call(binding, &endpoint, method, json!({}), &headers).await {
        Ok(extended) if extended.is_object() && extended.get("name").is_some() => {
            debug!(backend = %spec.name, "interaction agent's extended card read");
            Some(extended)
        }
        Ok(_) => None,
        Err(NotPosted::Unsent(error) | NotPosted::Unknown(error)) => {
            debug!(backend = %spec.name, %error, "interaction agent's extended card not read");
            None
        }
    }
}

/// The tools an MCP server lists, as `{"tools": [...]}`.
async fn list_tools(spec: &BackendSpec) -> Result<Value, NotPosted> {
    let mut tools = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..TOOL_PAGES {
        let mut params = json!({});
        if let Some(cursor) = &cursor {
            params["cursor"] = json!(cursor);
        }
        let headers = mcp_headers(spec, "tools/list", &mut params).await?;
        let body = rpc_body("tools/list", params);
        let reply = send_json(reqwest::Method::POST, &spec.url, Some(body), &headers, true);
        let page = match reply.await {
            Ok(reply) => unattended(reply, "tools/list", &spec.url, &headers).await,
            Err(error) => Err(error),
        };
        if let Err(error) = &page {
            mcp_failed(spec, error);
        }
        let page = page?;
        tools.extend(page["tools"].as_array().cloned().unwrap_or_default());
        cursor = text_of(&page["nextCursor"]);
        if cursor.is_none() {
            break;
        }
    }
    Ok(json!({ "tools": tools }))
}

fn clipped(text: &str) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match text.char_indices().nth(DESCRIPTION_MAX_CHARS) {
        Some((at, _)) => format!("{}...", &text[..at]),
        None => text,
    }
}

/// What a card says the agent does: its description, then its skills.
fn card_description(card: &Value) -> Option<String> {
    let skills = card["skills"].as_array().into_iter().flatten();
    let skill = |skill: &Value| {
        let name = text_of(&skill["name"]).or_else(|| text_of(&skill["id"]))?;
        Some(match text_of(&skill["description"]) {
            Some(what) => format!("{name}: {}", what.trim_end_matches('.')),
            None => name,
        })
    };
    let skills: Vec<String> = skills.filter_map(skill).collect();
    let mut text = text_of(&card["description"]).unwrap_or_default();
    if !skills.is_empty() {
        let end = if text.is_empty() || text.ends_with(['.', '!', '?']) {
            ""
        } else {
            "."
        };
        text = format!("{text}{end} Skills: {}.", skills.join("; "));
    }
    let text = clipped(text.trim());
    (!text.is_empty()).then_some(text)
}

/// The agent's binding and where it is: the card's interfaces (1.0 `supportedInterfaces`, 0.3
/// `url` and `additionalInterfaces`) on the entry's own origin, JSON-RPC first, then HTTP+JSON,
/// then gRPC (on the entry's host, any port: a gRPC server is seldom on the card's port); else
/// JSON-RPC at the entry's url. A card never sends the entry's credential elsewhere.
fn card_binding(card: &Value, entry: &str) -> (Binding, String) {
    let parsed = |url: &str| reqwest::Url::parse(url).ok();
    let own = parsed(entry);
    let listed = card["supportedInterfaces"].as_array().into_iter().flatten();
    let listed = listed.chain(
        card["additionalInterfaces"]
            .as_array()
            .into_iter()
            .flatten(),
    );
    let listed = listed.filter_map(|item| {
        let named = if item["protocolBinding"].is_null() {
            &item["transport"]
        } else {
            &item["protocolBinding"]
        };
        Some((Binding::named(named)?, text_of(&item["url"])?))
    });
    let main = Binding::named(&card["preferredTransport"])
        .zip(text_of(&card["url"]))
        .into_iter();
    let reachable = |(binding, url): &(Binding, String)| {
        let (Some(own), Some(url)) = (own.as_ref(), parsed(url)) else {
            return false;
        };
        match binding {
            Binding::Grpc => url.host_str() == own.host_str(),
            _ => url.origin() == own.origin(),
        }
    };
    let found: Vec<(Binding, String)> = listed.chain(main).filter(reachable).collect();
    [Binding::JsonRpc, Binding::Rest, Binding::Grpc]
        .into_iter()
        .find_map(|want| found.iter().find(|(binding, _)| *binding == want).cloned())
        .unwrap_or_else(|| (Binding::JsonRpc, entry.to_owned()))
}

/// The entry's OAuth client as the card completes it: an A2A entry that names no token
/// endpoint takes the one of the card's OAuth 2.0 scheme with a client-credentials flow (1.0
/// `oauth2SecurityScheme.flows.clientCredentials.tokenUrl`, 0.3 `type: oauth2`), when it is on
/// the entry's own origin or on https: the client secret goes nowhere else.
fn card_oauth(spec: &BackendSpec, card: &Value) -> Option<OAuthClient> {
    let mut oauth = spec.oauth.clone()?;
    if !oauth.token_url.is_empty() {
        return Some(oauth);
    }
    let schemes = card["securitySchemes"].as_object().into_iter().flatten();
    let token_url = schemes.map(|(_, scheme)| scheme).find_map(|scheme| {
        let oauth2 = scheme
            .get("oauth2SecurityScheme")
            .or((scheme["type"] == "oauth2").then_some(scheme))?;
        let flow = &oauth2["flows"]["clientCredentials"];
        text_of(&flow["tokenUrl"]).or_else(|| text_of(&flow["token_url"]))
    });
    let origin = |url: &str| reqwest::Url::parse(url).ok().map(|url| url.origin());
    oauth.token_url = token_url
        .filter(|url| url.starts_with("https://") || origin(url) == origin(&spec.url))
        .unwrap_or_default();
    Some(oauth)
}

/// The headers of a request to an A2A agent: the version, and the credential (the OAuth
/// client's token as a bearer, else the entry's secret in the card's scheme).
async fn a2a_headers<'a>(
    oauth: Option<&OAuthClient>,
    credential: Option<&'a (String, String)>,
) -> Result<Vec<(&'a str, String)>, NotPosted> {
    let mut headers = vec![("A2A-Version", "1.0".to_owned())];
    match oauth {
        Some(oauth) if oauth.token_url.is_empty() => {
            let error = "the agent's card names no OAuth token endpoint the gateway may use";
            return Err(NotPosted::Unsent(error.to_owned()));
        }
        Some(oauth) => {
            let token = access_token(oauth).await?;
            headers.push(("Authorization", format!("Bearer {token}")));
        }
        None => headers.extend(credential.map(|(name, value)| (name.as_str(), value.clone()))),
    }
    Ok(headers)
}

/// The header that carries the entry's secret, as the card's security scheme asks: an API
/// key in a named header (0.3 `apiKey`, 1.0 `apiKeySecurityScheme`), else a bearer token.
fn card_credential(card: &Value, secret: &str) -> (String, String) {
    let schemes = card["securitySchemes"].as_object().into_iter().flatten();
    let header = schemes.map(|(_, scheme)| scheme).find_map(|scheme| {
        let key = scheme.get("apiKeySecurityScheme").unwrap_or(scheme);
        let api_key = scheme.get("apiKeySecurityScheme").is_some() || scheme["type"] == "apiKey";
        let place = key["location"].as_str().or(key["in"].as_str());
        let place = place.unwrap_or_default();
        (api_key && place.eq_ignore_ascii_case("header")).then(|| text_of(&key["name"]))?
    });
    match header {
        Some(name) => (name, secret.to_owned()),
        None => ("Authorization".to_owned(), format!("Bearer {secret}")),
    }
}

/// Whether an orchestrator's call of `spec` (of its MCP tool `tool`) is confirmed with the
/// caller first: the operator's `confirm`, else `by_default`.
fn confirmed(spec: &BackendSpec, tool: Option<&str>, by_default: bool) -> bool {
    match &spec.confirm {
        Some(Confirm::Every(every)) => *every,
        Some(Confirm::Tools(named)) => tool
            .and_then(|tool| named.get(tool))
            .copied()
            .unwrap_or(by_default),
        None => by_default,
    }
}

/// A name a model may call: letters, digits, `_` and `-`, at most 64 of them.
fn tool_name(name: &str) -> String {
    let fit = |c: char| {
        if c.is_ascii_alphanumeric() || c == '-' {
            c
        } else {
            '_'
        }
    };
    name.chars().map(fit).take(64).collect()
}

/// The orchestrator's tools of the catalog entries `listed`, each with the entry its calls
/// run on. An A2A agent or a webhook is one tool, described by the entry, else by its card;
/// an MCP server with `tools` is one tool per tool it lists that the entry allows, with the
/// server's name, description and input schema, on a copy of the entry that calls that tool.
/// A call is confirmed with the caller when the entry may change things and, for an MCP
/// tool, its annotations do not say it only reads or does not destroy, unless the entry's
/// `confirm` says otherwise. A name already taken gets its entry's name in front.
pub fn agent_tools(listed: &[&BackendSpec]) -> Vec<(AgentTool, BackendSpec)> {
    let mut taken = HashSet::new();
    let mut tools = Vec::new();
    for spec in listed {
        let changes = spec.side_effects == SideEffects::Possible;
        if spec.kind != BackendKind::Mcp || spec.tools.is_empty() {
            let card = discovered(spec);
            let described = card.as_deref().and_then(card_description);
            let description = spec.description.clone().or(described);
            let tool = AgentTool {
                name: spec.name.clone(),
                description: description.unwrap_or_else(|| format!("The {} agent.", spec.name)),
                fallback: spec.fallback.clone(),
                parameters: None,
                confirm: confirmed(spec, None, changes),
            };
            taken.insert(tool.name.clone());
            tools.push((tool, (*spec).clone()));
            continue;
        }
        let listing = discovered(spec);
        let listing = listing.as_deref().unwrap_or(&Value::Null);
        let every = spec.tools.iter().any(|name| name == "*");
        let offered = listing["tools"].as_array().into_iter().flatten();
        for tool in offered {
            let Some(name) = text_of(&tool["name"]) else {
                continue;
            };
            if !every && !spec.tools.contains(&name) {
                continue;
            }
            let mut called = tool_name(&name);
            if taken.contains(&called) {
                called = tool_name(&format!("{}_{name}", spec.name));
            }
            if !taken.insert(called.clone()) {
                continue;
            }
            let hints = &tool["annotations"];
            let reads = hints["readOnlyHint"] == true;
            // MCP's defaults: a tool that does not only read may destroy.
            let destroys = !reads && hints["destructiveHint"] != false;
            let mut schema = tool["inputSchema"].clone();
            if !schema.is_object() {
                schema = json!({ "type": "object", "properties": {} });
            }
            let description = text_of(&tool["description"]).or_else(|| text_of(&tool["title"]));
            let agent = AgentTool {
                name: called,
                description: clipped(&description.unwrap_or_else(|| format!("The {name} tool."))),
                fallback: None,
                parameters: Some(schema),
                confirm: confirmed(spec, Some(&name), changes && destroys),
            };
            let mut runs = (*spec).clone();
            runs.tool = Some(name);
            if reads {
                runs.side_effects = SideEffects::None;
            }
            tools.push((agent, runs));
        }
    }
    tools
}

// ---- OpenAI Responses

/// Why a response has no final object.
enum Ended {
    Cancelled,
    Failed(String),
}

impl From<NotPosted> for Ended {
    fn from(lost: NotPosted) -> Self {
        let (NotPosted::Unsent(error) | NotPosted::Unknown(error)) = lost;
        Self::Failed(error)
    }
}

/// The server-sent events `bytes` completed, their `data` as JSON. The decoder works on
/// bytes, so a character or a line end split between chunks stays whole.
fn take_events(decoder: &mut SseDecoder, bytes: &[u8]) -> Result<Vec<Value>, String> {
    let messages = decoder.push(bytes)?;
    Ok(messages
        .into_iter()
        .filter_map(|(_, data)| serde_json::from_str(&data).ok())
        .collect())
}

/// A future behind a pointer. The run's task is spawned, and the compiler cannot show that
/// a future which borrows across several nested calls is `Send` for every lifetime; one
/// that is boxed where its borrows are known is.
fn boxed<'a, T>(
    future: impl Future<Output = T> + Send + 'a,
) -> Pin<Box<dyn Future<Output = T> + Send + 'a>> {
    Box::pin(future)
}

/// Whether a reply is written as server-sent events.
fn is_stream(reply: &reqwest::Response) -> bool {
    let kind = reply.headers().get(reqwest::header::CONTENT_TYPE);
    let kind = kind.and_then(|kind| kind.to_str().ok());
    kind.is_some_and(|kind| kind.contains("event-stream"))
}

/// The text of a response's message items.
fn output_text(response: &Value) -> String {
    let items = response["output"].as_array();
    let messages = items
        .into_iter()
        .flatten()
        .filter(|item| item["type"] == "message");
    let parts = messages
        .filter_map(|item| item["content"].as_array())
        .flatten();
    let texts = parts.filter(|part| part["type"] == "output_text");
    texts.filter_map(|part| part["text"].as_str()).collect()
}

impl Remote {
    fn responses_url(&self, rest: &str) -> String {
        format!("{}/responses{rest}", self.spec.url.trim_end_matches('/'))
    }

    async fn fetch(&self, rest: &str, stream: bool) -> Result<reqwest::Response, NotPosted> {
        let headers: Vec<_> = self.bearer().into_iter().collect();
        let url = self.responses_url(rest);
        send_json(reqwest::Method::GET, &url, None, &headers, stream).await
    }

    /// One run on a Responses model: the LLM tier's loop (the session's tools, their gates,
    /// `ask_caller`, the answer marker) over background responses.
    async fn respond(
        &self,
        job: &Job,
        tx: &mpsc::Sender<Msg>,
        cancel: &CancellationToken,
    ) -> RunEvent {
        let tools: &[ToolSpec] = if job.plain { &[] } else { &self.tools };
        let gated = !tools.is_empty();
        let mut instructions = with_context(&self.system_prompt, &job.context);
        let mut declared = Vec::new();
        if gated {
            instructions.push_str(ANSWER_RULE);
            let function = |name: &str, description: &str, parameters: &Value| {
                json!({ "type": "function", "name": name, "description": description,
                        "parameters": parameters })
            };
            declared = tools
                .iter()
                .map(|tool| function(&tool.name, &tool.description, &tool.parameters))
                .collect();
            let (description, parameters) = ask_caller_tool();
            declared.push(function(ASK_CALLER, description, &parameters));
        }
        let calls = ToolCalls::new(job.run, tx, cancel, self.tool_secret.clone());
        let caller = caller_words(&job.history, &job.words);
        let mut relay = Relay::new(job.run, tx, AnswerGate::new(gated), caller);
        relay.guarded = !job.child;
        relay.roles.clone_from(&self.roles);
        // A follow-up continues the response it follows: the server has what came before.
        let mut input = match &job.handle {
            Some(_) => Vec::new(),
            None => spoken_turns(&job.history),
        };
        input.push(json!({ "role": "user", "content": request_text(&job.words) }));
        let mut last_id = None;

        for round in 0.. {
            let mut body = json!({
                "model": self.spec.model, "input": input, "background": true, "store": true,
                "stream": self.spec.mode != Some(BackendMode::Poll),
            });
            if !instructions.is_empty() {
                body["instructions"] = json!(instructions);
            }
            if gated {
                body["tools"] = json!(declared);
            }
            if let Some(previous) = &job.handle {
                body["previous_response_id"] = json!(previous);
            }
            let response = match boxed(self.response(body, &mut relay, cancel)).await {
                Ok(response) => response,
                Err(Ended::Cancelled) => return RunEvent::Cancelled,
                Err(Ended::Failed(error)) => return RunEvent::Failed { error },
            };
            last_id = text_of(&response["id"]).or(last_id);
            let output = response["output"].as_array().cloned().unwrap_or_default();
            let asked: Vec<&Value> = output
                .iter()
                .filter(|item| item["type"] == "function_call")
                .collect();
            relay.end(!asked.is_empty()).await;
            if asked.is_empty() {
                break;
            }
            if round == MAX_ROUNDS {
                let error = format!("no answer after {MAX_ROUNDS} rounds of tool calls");
                return RunEvent::Failed { error };
            }
            // The model's own items go back with the results of its calls.
            input.extend(output.iter().cloned());
            for call in asked {
                let name = call["name"].as_str().unwrap_or_default();
                let arguments = call["arguments"].as_str().unwrap_or("{}");
                let arguments = serde_json::from_str(arguments).unwrap_or_else(|_| json!({}));
                let tool = tools.iter().find(|tool| tool.name == name);
                let result = match tool {
                    _ if name == ASK_CALLER => calls.ask_caller(arguments).await,
                    Some(tool) => boxed(calls.call(tool, arguments)).await,
                    None => Err(format!("{name} is not a tool of this call")),
                };
                let shown = result.unwrap_or_else(|error| json!({ "error": error }));
                input.push(
                    json!({ "type": "function_call_output", "call_id": call["call_id"],
                                   "output": shown.to_string() }),
                );
            }
            if calls.unconfirmed.load(std::sync::atomic::Ordering::Relaxed) {
                // As on the LLM tier: the model is not left to word an unknown outcome.
                let error = "a confirmed tool call did not report its outcome".to_owned();
                return RunEvent::Failed { error };
            }
            if cancel.is_cancelled() {
                return RunEvent::Cancelled;
            }
        }
        if relay.answer.is_empty() {
            let error = "the model returned no answer".to_owned();
            return RunEvent::Failed { error };
        }
        // What a follow-up of this task continues: its last response.
        if let Some(id) = last_id {
            calls.report(RunEvent::Handle(id)).await;
        }
        let text = relay.answer.join(" ");
        RunEvent::Finished { text }
    }

    /// One response, from its request to its final object. Its text is pushed to `relay`
    /// while it streams; a stream that breaks is picked up after its last event, and a
    /// response that does not stream is read until it is done.
    async fn response(
        &self,
        body: Value,
        relay: &mut Relay<'_>,
        cancel: &CancellationToken,
    ) -> Result<Value, Ended> {
        let stream = body["stream"] == true;
        let mut headers: Vec<_> = self.bearer().into_iter().collect();
        if stream {
            headers.push(("Accept", "text/event-stream".to_owned()));
        }
        let url = self.responses_url("");
        let posted = send_json(
            reqwest::Method::POST,
            &url,
            Some(body.to_string()),
            &headers,
            stream,
        );
        let mut reply = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(Ended::Cancelled),
            reply = posted => reply?,
        };
        // A polled response works on its own once it is accepted; a streamed one is alive at
        // its first token.
        if !stream && reply.status().is_success() {
            relay.alive().await;
        }
        // What the stream said of this response's text, its id and its last event.
        let (mut said, mut id, mut last) = (String::new(), None::<String>, 0u64);
        let mut resumes = 0;
        let mut response = 'read: {
            if !reply.status().is_success() || !is_stream(&reply) {
                break 'read serde_json::from_str::<Value>(&body_of(reply).await?).ok();
            }
            let mut decoder = SseDecoder::default();
            loop {
                let chunk = tokio::select! {
                    biased;
                    _ = cancel.cancelled() => {
                        self.cancel_response(id.as_deref()).await;
                        return Err(Ended::Cancelled);
                    }
                    chunk = reply.chunk() => chunk,
                };
                if let Ok(Some(bytes)) = chunk {
                    for event in take_events(&mut decoder, &bytes).map_err(Ended::Failed)? {
                        last = event["sequence_number"].as_u64().unwrap_or(last);
                        if let Some(of) = text_of(&event["response"]["id"]) {
                            id = Some(of);
                        }
                        match event["type"].as_str().unwrap_or_default() {
                            "response.output_text.delta" => {
                                let delta = event["delta"].as_str().unwrap_or_default();
                                said.push_str(delta);
                                boxed(relay.push(delta)).await;
                            }
                            "response.completed"
                            | "response.incomplete"
                            | "response.failed"
                            | "response.cancelled" => break 'read Some(event["response"].clone()),
                            "error" => {
                                let message = text_of(&event["message"]);
                                let message =
                                    message.or_else(|| text_of(&event["error"]["message"]));
                                return Err(Ended::Failed(
                                    message.unwrap_or_else(|| "the stream failed".to_owned()),
                                ));
                            }
                            _ => {}
                        }
                    }
                    continue;
                }
                // The stream ended before the response did.
                let Some(of) = id.as_deref() else {
                    return Err(Ended::Failed(
                        "the stream ended before the response began".into(),
                    ));
                };
                resumes += 1;
                let rest = format!("/{of}?stream=true&starting_after={last}");
                match self.fetch(&rest, true).await {
                    Ok(resumed)
                        if resumes <= MAX_MISSES
                            && resumed.status().is_success()
                            && is_stream(&resumed) =>
                    {
                        reply = resumed;
                        decoder = SseDecoder::default();
                    }
                    // No stream to pick up: the response is read until it is done.
                    _ => break 'read Some(json!({ "id": of, "status": "in_progress" })),
                }
            }
        };
        loop {
            let Some(now) = response.take() else {
                return Err(Ended::Failed(
                    "the reply is not a response object".to_owned(),
                ));
            };
            match now["status"].as_str().unwrap_or("completed") {
                "queued" | "in_progress" => {
                    let Some(of) = text_of(&now["id"]) else {
                        return Err(Ended::Failed("the response has no id".to_owned()));
                    };
                    tokio::select! {
                        biased;
                        _ = cancel.cancelled() => {
                            self.cancel_response(Some(&of)).await;
                            return Err(Ended::Cancelled);
                        }
                        _ = tokio::time::sleep(POLL) => {}
                    }
                    let read = self.fetch(&format!("/{of}"), false).await?;
                    response = serde_json::from_str(&body_of(read).await?).ok();
                }
                "cancelled" => return Err(Ended::Cancelled),
                "failed" => {
                    let message = text_of(&now["error"]["message"]);
                    return Err(Ended::Failed(
                        message.unwrap_or_else(|| "the response failed".to_owned()),
                    ));
                }
                // `completed`, or `incomplete`: it said what it could.
                _ => {
                    // What a stream did not deliver of the text is delivered now.
                    let text = output_text(&now);
                    if let Some(rest) = text.strip_prefix(said.as_str()) {
                        boxed(relay.push(rest)).await;
                    }
                    return Ok(now);
                }
            }
        }
    }

    /// Tell the server to stop a background response. Its tools are the gateway's, so what
    /// the run committed is already known.
    async fn cancel_response(&self, id: Option<&str>) {
        let Some(id) = id else { return };
        let headers: Vec<_> = self.bearer().into_iter().collect();
        let url = self.responses_url(&format!("/{id}/cancel"));
        let sent = send_json(
            reqwest::Method::POST,
            &url,
            Some("{}".to_owned()),
            &headers,
            false,
        );
        if let Err(NotPosted::Unsent(error) | NotPosted::Unknown(error)) = sent.await {
            debug!(backend = %self.spec.name, %error, "interaction response cancel not sent");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote(kind: &str) -> Arc<Remote> {
        let entry = json!({ "name": "one", "type": kind, "url": "https://backend.example.com/x",
                            "tool": "run", "model": "m" });
        let spec = serde_json::from_value(entry).expect("a catalog entry");
        Remote::new(spec, Some("tool-secret".to_owned()), ["", ""], Vec::new())
    }

    #[test]
    fn a_webhooks_events_become_the_steps_of_its_task() {
        let events = json!([
            { "type": "progress", "text": " Looking it up. " },
            { "type": "effect", "label": "held the table" },
            { "type": "effect" },
            { "type": "note", "text": "not an event of the task" },
            { "type": "question", "request_id": "q1", "kind": "confirm", "action": "book a table",
              "arguments": { "party": 2 } },
            { "type": "question", "request_id": "q2", "kind": "choose", "prompt": "Which one?",
              "options": ["red", "blue"] },
            { "type": "question", "kind": "choose", "prompt": "Which one?", "options": ["red"] },
            { "type": "question", "kind": "confirm" },
            { "type": "result", "text": "Done." },
            { "type": "failed" },
            { "type": "cancelled" },
        ]);
        let steps = webhook_steps(events);
        let confirm = Ask {
            id: "q1".to_owned(),
            kind: HitlKind::Confirm,
            op_name: "book a table".to_owned(),
            args: json!({ "party": 2 }),
            field: None,
        };
        let choose = Ask::asking("q2", "Which one?", &["red".to_owned(), "blue".to_owned()]);
        assert_eq!(choose.kind, HitlKind::Choose);
        assert_eq!(choose.args["options"], json!(["red", "blue"]));
        // One option is no choice: the caller is asked for a value.
        let value = Ask::asking("", "Which one?", &[]);
        assert_eq!(
            (value.kind, &value.args["kind"]),
            (HitlKind::Value, &json!("value"))
        );
        assert_eq!(
            steps,
            [
                Step::Working(Some("Looking it up.".to_owned())),
                Step::Effect("held the table".to_owned()),
                Step::Ask(confirm),
                Step::Ask(choose),
                Step::Ask(Ask::asking("", "Which one?", &["red".to_owned()])),
                Step::Failed("the webhook's confirmation names no action".to_owned()),
                Step::Done("Done.".to_owned()),
                Step::Failed("the webhook failed".to_owned()),
                Step::Cancelled,
            ]
        );
        // A result's data and artifacts come just before it.
        let ticket = json!({ "url": "https://air.example/t/1", "name": "e-ticket" });
        let result = json!({ "type": "result", "text": "Booked.", "data": { "seat": "14C" },
                             "artifacts": [ticket.clone()] });
        assert_eq!(
            webhook_steps(result),
            [
                Step::Output(TaskOutput {
                    data: Some(json!({ "seat": "14C" })),
                    artifacts: vec![ticket],
                }),
                Step::Done("Booked.".to_owned()),
            ]
        );
        // A form and a sign-in link are for the client's screen.
        let screen = webhook_steps(json!([
            { "type": "question", "request_id": "f1", "kind": "form", "prompt": "Your address?",
              "schema": { "type": "object", "properties": { "street": { "type": "string" } } } },
            { "type": "question", "request_id": "a1", "kind": "auth_url", "prompt": "Sign in.",
              "url": "https://id.example.com/x", "sensitive": true },
            { "type": "question", "request_id": "f2", "kind": "form", "prompt": "No schema." },
        ]));
        let Step::Ask(form) = &screen[0] else {
            panic!("a form question: {screen:?}")
        };
        assert_eq!(
            (form.kind, &form.args["question"]),
            (HitlKind::Form, &json!("Your address?"))
        );
        assert_eq!(
            form.args["schema"]["properties"]["street"]["type"],
            "string"
        );
        let Step::Ask(link) = &screen[1] else {
            panic!("a sign-in question: {screen:?}")
        };
        assert_eq!(link.kind, HitlKind::AuthUrl);
        assert_eq!(
            (&link.args["url"], &link.args["sensitive"]),
            (&json!("https://id.example.com/x"), &json!(true))
        );
        assert!(matches!(&screen[2], Step::Failed(_)));
        // An empty reply, or one that is not JSON, is "taken; more by callback".
        assert!(webhook_steps(Value::Null).is_empty());
        assert!(webhook_steps(json!({})).is_empty());
        // The entry's own secret signs; without one the server's tool secret does.
        assert_eq!(remote("webhook").secret(), Some("tool-secret"));
    }

    /// What a fresh A2A wire of `backend` reads in `result`.
    fn wire_read(backend: &Remote, result: &Value) -> Vec<Step> {
        let mut wire = A2a::new(backend, None);
        wire.read(result)
    }

    #[test]
    fn a_question_carries_its_backends_deadline_and_a_task_its_handle() {
        // Webhook: `expires_in_ms` on the question event.
        let steps = webhook_steps(json!({ "type": "question", "request_id": "q1",
            "kind": "choose", "prompt": "Window or aisle?", "options": ["window", "aisle"],
            "expires_in_ms": 5000 }));
        let [Step::Ask(ask)] = &steps[..] else {
            panic!("{steps:?}");
        };
        assert_eq!(ask.args["expires_in_ms"], 5000);
        // A2A: the status message's metadata.
        let backend = remote("a2a");
        let message = json!({ "role": "ROLE_AGENT", "parts": [{ "text": "Window or aisle?" }],
                              "metadata": { "expires_in_ms": 4000 } });
        let task = json!({ "task": { "id": "tk", "contextId": "ctx-7",
            "status": { "state": "TASK_STATE_INPUT_REQUIRED", "message": message } } });
        let mut wire = A2a::new(&backend, None);
        let [Step::Ask(ask)] = &wire.read(&task)[..] else {
            panic!("an A2A question");
        };
        assert_eq!(ask.args["expires_in_ms"], 4000);
        // The task's context is its handle: a follow-up sends it again.
        assert_eq!(wire.handle().as_deref(), Some("ctx-7"));
        // MCP: the elicitation's `_meta`.
        let request = json!({ "method": "elicitation/create", "params": { "mode": "form",
            "message": "Go on?", "_meta": { "expires_in_ms": 3000 },
            "requestedSchema": { "properties": { "go": { "type": "boolean" } } } } });
        let ask = elicitation("k", &request).expect("a question");
        assert_eq!(ask.args["expires_in_ms"], 3000);
        // Without one, no deadline of the backend's.
        let steps = webhook_steps(json!({ "type": "question", "request_id": "q2",
            "kind": "value", "prompt": "Which day?" }));
        let [Step::Ask(ask)] = &steps[..] else {
            panic!("{steps:?}");
        };
        assert!(ask.args.get("expires_in_ms").is_none());
    }

    #[test]
    fn an_a2a_task_is_read_in_both_spellings_of_its_states() {
        let backend = remote("a2a");
        let mut wire = A2a::new(&backend, None);
        let says = |text: &str| json!({ "role": "ROLE_AGENT", "parts": [{ "text": text }] });
        let task = |state: &str, message: Value| json!({ "id": "tk", "contextId": "ctx", "status": { "state": state, "message": message } });
        // Progress is passed on once per text.
        let working = json!({ "task": task("TASK_STATE_WORKING", says("Checking.")) });
        assert_eq!(
            wire.read(&working),
            [Step::Working(Some("Checking.".to_owned()))]
        );
        assert!(wire.read(&working).is_empty());
        assert_eq!(wire.id.as_deref(), Some("tk"));
        assert_eq!(backend.context.lock().as_deref(), Some("ctx"));
        // The 0.3 spelling, and a bare task as `GetTask` returns it.
        let asks = wire.read(&task("input-required", says("Which day?")));
        assert_eq!(asks, [Step::Ask(Ask::asking("", "Which day?", &[]))]);
        let mut done = task("TASK_STATE_COMPLETED", Value::Null);
        done["artifacts"] = json!([{ "parts": [{ "text": "Tuesday." }, { "url": "x" }] },
                                   { "parts": [{ "text": "At ten." }] }]);
        // Its link part is an artifact of the result.
        let link = TaskOutput {
            data: None,
            artifacts: vec![json!({ "url": "x" })],
        };
        assert_eq!(
            wire.read(&done),
            [
                Step::Output(link),
                Step::Done("Tuesday. At ten.".to_owned())
            ]
        );
        // Without artifacts the status message is the answer.
        let said = task("completed", says("All set."));
        assert_eq!(wire.read(&said), [Step::Done("All set.".to_owned())]);
        assert_eq!(
            wire.read(&task("TASK_STATE_CANCELED", Value::Null)),
            [Step::Cancelled]
        );
        let rejected = wire.read(&task("TASK_STATE_REJECTED", Value::Null));
        assert_eq!(
            rejected,
            [Step::Failed("the agent's task rejected".to_owned())]
        );
        // A sign-in is a link on the client's screen: a part's url, a data part's, or the
        // first address in its text. Without a link it cannot be given.
        let link =
            |message: Value| match &wire_read(&backend, &task("TASK_STATE_AUTH_REQUIRED", message))
                [..]
            {
                [Step::Ask(ask)] if ask.kind == HitlKind::AuthUrl => {
                    Some((ask.args["url"].clone(), ask.args["question"].clone()))
                }
                _ => None,
            };
        let text =
            says("Please sign in at https://id.example.com/authorize?state=7. Then I go on.");
        assert_eq!(
            link(text.clone()),
            Some((
                json!("https://id.example.com/authorize?state=7"),
                json!("Please sign in at https://id.example.com/authorize?state=7. Then I go on.")
            ))
        );
        let part = json!({ "parts": [{ "url": "https://id.example.com/a" }] });
        assert_eq!(
            link(part),
            Some((json!("https://id.example.com/a"), json!("Please sign in.")))
        );
        let data = json!({ "parts": [{ "text": "Sign in, please." }, { "data": { "url": "https://id.example.com/d" } }] });
        assert_eq!(
            link(data),
            Some((json!("https://id.example.com/d"), json!("Sign in, please.")))
        );
        assert!(matches!(
            &wire.read(&task("TASK_STATE_AUTH_REQUIRED", says("Sign in first.")))[..],
            [Step::Failed(error)] if error.contains("sign-in")
        ));
        assert!(matches!(
            &wire.read(&task("auth-required", Value::Null))[..],
            [Step::Failed(error)] if error.contains("no link")
        ));
        // An agent that answers the message outright.
        let direct = json!({ "message": says("We open at nine.") });
        assert_eq!(
            wire.read(&direct),
            [Step::Done("We open at nine.".to_owned())]
        );
    }

    #[test]
    fn an_mcp_result_and_an_elicitation_are_read() {
        let text = |text: &str| json!({ "type": "text", "text": text });
        let result =
            json!({ "content": [text("In stock."), { "type": "image" }, text("Two left.")] });
        assert_eq!(
            tool_result(&result),
            Step::Done("In stock. Two left.".to_owned())
        );
        let failed = json!({ "content": [text("No such item.")], "isError": true });
        assert_eq!(
            tool_result(&failed),
            Step::Failed("No such item.".to_owned())
        );
        // What the result holds besides its text: its structured content and the rest.
        let link = json!({ "type": "resource_link", "uri": "https://shop.example/i/7" });
        let rich = json!({ "content": [text("In stock."), link.clone()],
                           "structuredContent": { "stock": 2 } });
        assert_eq!(
            tool_output(&rich),
            Some(Step::Output(TaskOutput {
                data: Some(json!({ "stock": 2 })),
                artifacts: vec![link],
            }))
        );
        assert_eq!(
            tool_output(&json!({ "content": [text("In stock.")] })),
            None
        );
        assert_eq!(tool_output(&failed), None);

        let form = |field: Value| {
            json!({ "method": "elicitation/create", "params": { "mode": "form", "message": "Go on?",
                    "requestedSchema": { "type": "object", "properties": { "ok": field } } } })
        };
        // A yes or no is a choice of two; an enumeration is a choice of its values.
        let yes_no = elicitation("k", &form(json!({ "type": "boolean" }))).unwrap();
        assert_eq!(yes_no.kind, HitlKind::Choose);
        assert_eq!(yes_no.args["options"], json!(["yes", "no"]));
        assert_eq!(yes_no.field, Some(("ok".to_owned(), true)));
        let listed = elicitation("k", &form(json!({ "type": "string", "enum": ["a", "b"] })));
        assert_eq!(listed.unwrap().args["options"], json!(["a", "b"]));
        let free = elicitation("k", &form(json!({ "type": "string" }))).unwrap();
        assert_eq!((free.kind, free.id.as_str()), (HitlKind::Value, "k"));
        // Several fields are a form for the client's screen, with the server's schema.
        let schema = json!({ "type": "object", "required": ["street", "city"], "properties": {
            "street": { "type": "string" }, "city": { "type": "string" } } });
        let several = json!({ "method": "elicitation/create", "params": { "mode": "form",
                              "message": "Where should it go?", "requestedSchema": schema } });
        let filled = elicitation("k", &several).unwrap();
        assert_eq!((filled.kind, filled.field.as_ref()), (HitlKind::Form, None));
        assert_eq!(
            (&filled.args["schema"], &filled.args["question"]),
            (&schema, &json!("Where should it go?"))
        );
        // A page to open and a request of another kind cannot be answered by voice.
        let url = json!({ "method": "elicitation/create",
                          "params": { "mode": "url", "message": "Sign in", "url": "https://x" } });
        let sign_in = elicitation("k", &url).unwrap();
        assert_eq!(sign_in.kind, HitlKind::AuthUrl);
        assert_eq!(sign_in.args["url"], "https://x");
        assert_eq!(sign_in.args["question"], "Sign in");
        let other = json!({ "method": "sampling/createMessage", "params": {} });
        assert!(elicitation("k", &other).is_none());
        let sampling = json!({ "method": "sampling/createMessage", "params": {} });
        assert!(elicitation("k", &sampling).is_none());
    }

    #[test]
    fn a_question_ends_for_the_backend_as_it_ended_for_the_caller() {
        let op = super::super::types::OpId("op_1".to_owned());
        assert_eq!(outcome(&GateDecision::Allow(op)), "accept");
        assert_eq!(outcome(&GateDecision::Answer(json!("blue"))), "accept");
        assert_eq!(outcome(&GateDecision::Refuse("declined")), "decline");
        assert_eq!(outcome(&GateDecision::Refuse("not_by_voice")), "decline");
        assert_eq!(outcome(&GateDecision::Refuse("timeout")), "timeout");
        assert_eq!(outcome(&GateDecision::Refuse("stale")), "cancel");
    }

    #[test]
    fn a_responses_stream_is_cut_into_events_and_its_text_found() {
        let mut decoder = SseDecoder::default();
        let first = ": keep-alive\n\nevent: response.created\ndata: {\"type\": \"response.created\"}\n\n\
                     data: [DONE]\n\ndata: {\"type\": \"response.output_text.delta\", \"delta\"";
        let events = take_events(&mut decoder, first.as_bytes()).unwrap();
        // The comment and the end marker are no events; half an event waits for its rest.
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["type"], "response.created");
        let events = take_events(&mut decoder, ": \"Hi\"}\n\n".as_bytes()).unwrap();
        assert_eq!(events[0]["delta"], "Hi");
        // A character and a CRLF blank line split between chunks stay whole.
        let event =
            "data: {\"type\": \"response.output_text.delta\", \"delta\": \"caf\u{e9}\"}\r\n\r\n";
        let at = event.find('\u{e9}').unwrap() + 1;
        let end = event.len() - 1;
        let bytes = event.as_bytes();
        assert!(take_events(&mut decoder, &bytes[..at]).unwrap().is_empty());
        assert!(
            take_events(&mut decoder, &bytes[at..end])
                .unwrap()
                .is_empty()
        );
        let events = take_events(&mut decoder, &bytes[end..]).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["delta"], "caf\u{e9}");

        let response = json!({ "output": [
            { "type": "reasoning", "summary": [] },
            { "type": "message", "content": [{ "type": "output_text", "text": "ANSWER:\nAt " },
                                             { "type": "refusal", "refusal": "no" },
                                             { "type": "output_text", "text": "nine." }] },
            { "type": "function_call", "name": "x", "arguments": "{}" },
        ] });
        assert_eq!(output_text(&response), "ANSWER:\nAt nine.");
        assert_eq!(
            remote("openai_responses").responses_url("/r1/cancel"),
            "https://backend.example.com/x/responses/r1/cancel"
        );
    }

    /// Live PR2 (r17): words that go on with a request start a run of the same revision; the
    /// replaced run's end leaves the new run's place, and its pushes are taken.
    #[test]
    fn a_replaced_runs_end_leaves_the_callback_place_of_the_run_that_replaced_it() {
        let token = || Proof::Token("push-token".to_owned());
        let (first, _) = Listening::open("task_goes_on", 1, token());
        let (second, mut events) = Listening::open("task_goes_on", 1, token());
        drop(first);
        let from = CallbackFrom {
            token: Some("push-token"),
            ..CallbackFrom::default()
        };
        let push = r#"{"kind":"status-update","status":{"state":"working"}}"#;
        assert_eq!(callback("task_goes_on", from, push), Ok(()));
        assert!(events.try_recv().is_ok());
        drop(second);
        assert_eq!(
            callback("task_goes_on", from, push),
            Err(CallbackRefused::UnknownTask)
        );
    }

    #[test]
    fn a_callback_needs_the_backends_signature_and_a_fresh_timestamp() {
        let proof = Proof::Signed(Some("hook".to_owned()));
        let (listening, mut events) = Listening::open("task_unit", 2, proof);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let signed_at = |task: &str, at: u64, signed: Option<&str>, body: &str| {
            let at = at.to_string();
            let sender = CallbackFrom {
                timestamp: Some(&at),
                signature: signed,
                ..CallbackFrom::default()
            };
            callback(task, sender, body)
        };
        let send = |at: u64, secret: &str, body: &str| {
            let signed = signature(secret, at, body);
            signed_at("task_unit", at, Some(&signed), body)
        };
        let body = r#"[{"type": "progress", "text": "One."}, {"type": "result", "text": "Two.", "revision": 2}]"#;
        assert_eq!(send(now, "hook", body), Ok(()));
        assert_eq!(events.try_recv().unwrap()["text"], "One.");
        assert_eq!(events.try_recv().unwrap()["text"], "Two.");
        // A replayed request, another secret, no signature, another task, another revision.
        assert_eq!(
            send(now - 301, "hook", body),
            Err(CallbackRefused::Unsigned)
        );
        assert_eq!(
            send(now, "tool-secret", body),
            Err(CallbackRefused::Unsigned)
        );
        let unsigned = signed_at("task_unit", now, None, body);
        assert_eq!(unsigned, Err(CallbackRefused::Unsigned));
        let other = signed_at("task_other", now, None, body);
        assert_eq!(other, Err(CallbackRefused::UnknownTask));
        let old = r#"{"type": "result", "text": "Late.", "revision": 1}"#;
        assert_eq!(send(now, "hook", old), Err(CallbackRefused::Stale));
        assert_eq!(send(now, "hook", "\"text\""), Err(CallbackRefused::Invalid));
        assert_eq!(send(now, "hook", "not json"), Err(CallbackRefused::Invalid));
        // A push token does not stand for a webhook's signature.
        let pushed = CallbackFrom {
            token: Some("hook"),
            ..CallbackFrom::default()
        };
        assert_eq!(
            callback("task_unit", pushed, body),
            Err(CallbackRefused::Unsigned)
        );
        assert!(events.try_recv().is_err());
        drop(listening);
        assert_eq!(send(now, "hook", body), Err(CallbackRefused::UnknownTask));
    }

    #[test]
    fn an_a2a_entry_takes_its_token_endpoint_from_the_card_only_where_its_secret_may_go() {
        let spec = |oauth: Value| {
            let entry = json!({ "name": "trips", "type": "a2a", "url": "http://127.0.0.1:9/a2a",
                                "oauth": oauth });
            serde_json::from_value::<BackendSpec>(entry).expect("an entry")
        };
        let client = json!({ "client_id": "waav", "client_secret_env": "TRIPS_SECRET" });
        let card = |url: &str| {
            json!({ "securitySchemes": { "o": { "oauth2SecurityScheme": { "flows": {
                "clientCredentials": { "tokenUrl": url, "scopes": { "trips": "" } } } } } } })
        };
        let taken = |card: &Value| card_oauth(&spec(client.clone()), card).map(|o| o.token_url);
        // On the entry's own origin, or https: taken.
        let own = "http://127.0.0.1:9/token";
        assert_eq!(taken(&card(own)).as_deref(), Some(own));
        let away = "https://auth.example.com/oauth/token";
        assert_eq!(taken(&card(away)).as_deref(), Some(away));
        // Plain http elsewhere: not taken; a request then fails before anything is sent.
        assert_eq!(taken(&card("http://10.0.0.9/token")).as_deref(), Some(""));
        // 0.3 spelling.
        let old = json!({ "securitySchemes": { "o": { "type": "oauth2", "flows": {
            "clientCredentials": { "tokenUrl": own } } } } });
        assert_eq!(taken(&old).as_deref(), Some(own));
        // The entry's own token endpoint wins; no oauth, nothing.
        let mut named = client.clone();
        named["token_url"] = json!("https://login.example.com/token");
        let named = card_oauth(&spec(named), &card(own)).map(|o| o.token_url);
        assert_eq!(named.as_deref(), Some("https://login.example.com/token"));
        let plain = json!({ "name": "t", "type": "a2a", "url": "http://127.0.0.1:9/a2a" });
        let plain: BackendSpec = serde_json::from_value(plain).expect("an entry");
        assert!(card_oauth(&plain, &card(own)).is_none());
    }

    #[test]
    fn an_a2a_method_has_its_http_json_route() {
        let base = "https://agent.example.com/v1/";
        let route = |method: &str, params: Value| {
            rest_route(base, method, &params)
                .map(|(verb, url, body)| (verb.to_string(), url, body.is_some()))
        };
        let sent = json!({ "message": { "messageId": "m" } });
        assert_eq!(
            route("SendMessage", sent.clone()).ok(),
            Some((
                "POST".into(),
                "https://agent.example.com/v1/message:send".into(),
                true
            ))
        );
        assert_eq!(
            route("SendStreamingMessage", sent).ok(),
            Some((
                "POST".into(),
                "https://agent.example.com/v1/message:stream".into(),
                true
            ))
        );
        assert_eq!(
            route("GetTask", json!({ "id": "t 1/2" })).ok(),
            Some((
                "GET".into(),
                "https://agent.example.com/v1/tasks/t%201%2F2".into(),
                false
            ))
        );
        assert_eq!(
            route("CancelTask", json!({ "id": "t1" })).ok(),
            Some((
                "POST".into(),
                "https://agent.example.com/v1/tasks/t1:cancel".into(),
                true
            ))
        );
        assert_eq!(
            route("GetExtendedAgentCard", json!({})).ok(),
            Some((
                "GET".into(),
                "https://agent.example.com/v1/extendedAgentCard".into(),
                false
            ))
        );
        assert!(route("GetTask", json!({})).is_err());
        assert!(route("ListTasks", json!({})).is_err());
    }

    #[test]
    fn what_an_mcp_servers_get_stream_says_finds_its_call() {
        let entry = "mcp notes https://mcp.example.com/a";
        let elicit = json!({ "jsonrpc": "2.0", "id": 7, "method": "elicitation/create",
            "params": { "message": "Which list?", "_meta": { RELATED_TASK: { "taskId": "tk-1" } } } });
        assert_eq!(
            listener_of(entry, &elicit).as_deref(),
            Some("mcp notes https://mcp.example.com/a task:tk-1")
        );
        let status = json!({ "jsonrpc": "2.0", "method": "notifications/tasks/status",
                             "params": { "taskId": "tk-2", "status": "completed" } });
        assert_eq!(
            listener_of(entry, &status).as_deref(),
            Some("mcp notes https://mcp.example.com/a task:tk-2")
        );
        let progress = json!({ "jsonrpc": "2.0", "method": "notifications/progress",
                               "params": { "progressToken": "p9", "progress": 1, "total": 2 } });
        assert_eq!(
            listener_of(entry, &progress).as_deref(),
            Some("mcp notes https://mcp.example.com/a progress:p9")
        );
        assert_eq!(
            listener_of(
                entry,
                &json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" })
            ),
            None
        );
        // A result's controls: one, or a list; one the gateway cannot read is dropped.
        let one = json!({ "_meta": { "io.waav/control": { "kind": "hold", "request_id": "c1" } } });
        assert!(
            matches!(&result_controls(&one)[..], [Step::Control { id: Some(id), .. }] if id == "c1")
        );
        let two = json!({ "_meta": { "io.waav/control": [{ "kind": "rate", "value": "slower" },
                                                        { "kind": "no-such-kind" }] } });
        assert_eq!(result_controls(&two).len(), 1);
        assert!(result_controls(&json!({ "content": [] })).is_empty());
    }

    #[test]
    fn a_token_is_renewed_before_it_expires() {
        assert_eq!(renew_in(Some(3_600)), Duration::from_secs(3_540));
        assert_eq!(renew_in(Some(90)), Duration::from_secs(45));
        assert_eq!(renew_in(Some(0)), Duration::ZERO);
        assert_eq!(renew_in(None), Duration::from_secs(240));
    }

    #[test]
    fn an_mcp_request_names_its_method_and_a_session_its_id() {
        let base = vec![("MCP-Protocol-Version", "2026-07-28".to_owned())];
        let mut params = json!({ "name": "find_item", "arguments": {} });
        let headers = with_method(base, "tools/call", &mut params);
        let names: Vec<&str> = headers.iter().map(|(name, _)| *name).collect();
        assert_eq!(names, ["MCP-Protocol-Version", "Mcp-Method", "Mcp-Name"]);
        assert_eq!(
            params["_meta"]["io.modelcontextprotocol/clientCapabilities"],
            mcp_client_capabilities()
        );
        assert!(is_request(
            &json!({ "id": 7, "method": "elicitation/create" })
        ));
        assert!(!is_request(&json!({ "method": "notifications/progress" })));
        assert!(!is_request(&json!({ "id": 7, "result": {} })));
        assert_eq!(McpSession::stateless().version, MCP_VERSION);
    }

    #[test]
    fn an_agents_control_is_a_step_of_its_task() {
        let event =
            json!({ "type": "control", "kind": "rate", "value": "slower", "request_id": "c1" });
        match webhook_steps(event).pop() {
            Some(Step::Control { control, id }) => {
                assert_eq!(control.source, ControlSource::Agent);
                assert_eq!(id.as_deref(), Some("c1"));
            }
            other => panic!("not a control: {other:?}"),
        }
        // A control the gateway does not know is dropped.
        assert!(webhook_steps(json!({ "type": "control", "kind": "dance" })).is_empty());
        // An A2A status message asks in a data part, once however often it is read.
        let a2a = remote("a2a");
        let mut wire = A2a::new(&a2a, None);
        let part = json!({ "data": { "type": "control", "kind": "verbosity", "value": "brief" } });
        let task = json!({ "id": "t", "status": { "state": "TASK_STATE_WORKING",
            "message": { "messageId": "m1", "parts": [{ "text": "Working." }, part] } } });
        let steps = wire.read(&task);
        assert!(matches!(steps[0], Step::Control { .. }), "{steps:?}");
        assert_eq!(steps[1], Step::Working(Some("Working.".to_owned())));
        assert!(wire.read(&task).is_empty(), "the same message again");
    }

    #[test]
    fn an_a2a_push_needs_its_tasks_token_and_nothing_else() {
        let token = "t0ken-of-the-task".to_owned();
        let (_listening, mut events) = Listening::open("task_push", 1, Proof::Token(token));
        let body =
            r#"{"statusUpdate": {"taskId": "a1", "status": {"state": "TASK_STATE_WORKING"}}}"#;
        fn with(token: Option<&str>) -> CallbackFrom<'_> {
            CallbackFrom {
                token,
                ..CallbackFrom::default()
            }
        }
        assert_eq!(
            callback("task_push", with(Some("t0ken-of-the-task")), body),
            Ok(())
        );
        assert_eq!(events.try_recv().unwrap()["statusUpdate"]["taskId"], "a1");
        for wrong in [None, Some("t0ken-of-another"), Some("")] {
            assert_eq!(
                callback("task_push", with(wrong), body),
                Err(CallbackRefused::Unsigned)
            );
        }
        // Handed on by another process: the token's digest stands for it, the token itself
        // (or another token's digest) in its place does not.
        let digest = |given: &str| token_digest(given);
        let handed = |digest: &str| {
            let sender = CallbackFrom {
                token_sha256: Some(digest),
                ..CallbackFrom::default()
            };
            callback("task_push", sender, body)
        };
        assert_eq!(handed(&digest("t0ken-of-the-task")), Ok(()));
        assert!(events.try_recv().is_ok(), "taken");
        assert_eq!(digest(" t0ken-of-the-task "), digest("t0ken-of-the-task"));
        for wrong in [digest("t0ken-of-another"), "t0ken-of-the-task".to_owned()] {
            assert_eq!(handed(&wrong), Err(CallbackRefused::Unsigned));
        }
        // What has no proof at all is told before anything is asked.
        assert!(!CallbackFrom::default().has_proof());
        assert!(!with(Some(" ")).has_proof());
        assert!(with(Some("t")).has_proof());
        let half = CallbackFrom {
            timestamp: Some("1"),
            ..CallbackFrom::default()
        };
        assert!(!half.has_proof());
        assert!(
            CallbackFrom {
                signature: Some("sha256=00"),
                ..half
            }
            .has_proof()
        );
        // The relayed form carries the digest, never the token.
        let relayed = super::super::store::RelayedCallback::of(
            "task_push".to_owned(),
            with(Some("t0ken-of-the-task")),
            body.to_owned(),
        );
        let wire = serde_json::to_string(&relayed).unwrap();
        assert!(!wire.contains("t0ken-of-the-task"), "{wire}");
        assert!(wire.contains(&digest("t0ken-of-the-task")));
        assert!(!format!("{relayed:?}").contains(&digest("t0ken-of-the-task")));
        // A webhook's signature does not stand for the token.
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            .to_string();
        let signed = signature("t0ken-of-the-task", now.parse().unwrap(), body);
        let sender = CallbackFrom {
            timestamp: Some(&now),
            signature: Some(&signed),
            ..CallbackFrom::default()
        };
        assert_eq!(
            callback("task_push", sender, body),
            Err(CallbackRefused::Unsigned)
        );
        assert!(events.try_recv().is_err());
    }

    #[test]
    fn an_agents_card_describes_it_and_says_how_it_is_reached() {
        let card = json!({
            "name": "Trips", "description": "Plans trips",
            "skills": [{ "id": "hotels", "name": "Hotels", "description": "Finds hotels." },
                       { "id": "trains" }],
            "url": "https://agent.example.com/rpc",
            "supportedInterfaces": [
                { "url": "https://elsewhere.example.com/rpc", "protocolBinding": "JSONRPC" },
                { "url": "https://agent.example.com/grpc", "protocolBinding": "GRPC" },
            ],
        });
        assert_eq!(
            card_description(&card).as_deref(),
            Some("Plans trips. Skills: Hotels: Finds hotels; trains.")
        );
        assert_eq!(card_description(&json!({})), None);
        // Its JSON-RPC binding on the entry's own origin, before the others; never one
        // elsewhere.
        let entry = "https://agent.example.com/a2a";
        let at = |binding, url: &str| (binding, url.to_owned());
        let rpc = at(Binding::JsonRpc, "https://agent.example.com/rpc");
        assert_eq!(card_binding(&card, entry), rpc);
        let away = json!({ "url": "https://elsewhere.example.com/rpc" });
        assert_eq!(card_binding(&away, entry), at(Binding::JsonRpc, entry));
        assert_eq!(
            card_binding(&Value::Null, entry),
            at(Binding::JsonRpc, entry)
        );
        // Only gRPC: on the entry's host, any port; HTTP+JSON on its origin.
        let grpc =
            json!({ "url": "https://agent.example.com:50051", "preferredTransport": "GRPC" });
        let served = at(Binding::Grpc, "https://agent.example.com:50051");
        assert_eq!(card_binding(&grpc, entry), served);
        let other_host = json!({ "supportedInterfaces": [
            { "url": "https://grpc.example.com:50051", "protocolBinding": "GRPC" }] });
        assert_eq!(
            card_binding(&other_host, entry),
            at(Binding::JsonRpc, entry)
        );
        let rest = json!({ "supportedInterfaces": [
            { "url": "https://agent.example.com:8443/v1", "protocolBinding": "HTTP+JSON" },
            { "url": "https://agent.example.com/v1", "protocolBinding": "HTTP+JSON" },
            { "url": "https://agent.example.com:50051", "protocolBinding": "GRPC" }] });
        let served = at(Binding::Rest, "https://agent.example.com/v1");
        assert_eq!(card_binding(&rest, entry), served);
        // The secret goes where the card's security scheme asks, else as a bearer token.
        let header = |name: &str| (name.to_owned(), "k".to_owned());
        let old = json!({ "securitySchemes": { "a": { "type": "apiKey", "in": "header", "name": "X-Key" } } });
        assert_eq!(card_credential(&old, "k"), header("X-Key"));
        let new = json!({ "securitySchemes": { "a": { "apiKeySecurityScheme":
            { "location": "header", "name": "X-Api-Key" } } } });
        assert_eq!(card_credential(&new, "k"), header("X-Api-Key"));
        let query =
            json!({ "securitySchemes": { "a": { "type": "apiKey", "in": "query", "name": "k" } } });
        let bearer = ("Authorization".to_owned(), "Bearer k".to_owned());
        assert_eq!(card_credential(&query, "k"), bearer);
        let http = json!({ "securitySchemes": { "a": { "httpAuthSecurityScheme": { "scheme": "Bearer" } } } });
        assert_eq!(card_credential(&http, "k"), bearer);
    }

    fn entry(fields: Value) -> BackendSpec {
        let mut entry =
            json!({ "name": "notes", "type": "mcp", "url": "https://mcp.example.com/a" });
        if let (Some(entry), Some(fields)) = (entry.as_object_mut(), fields.as_object()) {
            entry.extend(fields.clone());
        }
        serde_json::from_value(entry).expect("a catalog entry")
    }

    /// Two entries of one server may hold different credentials: what one read (its extended
    /// card, its MCP session, its tool list) is not the other's, and an OAuth token is its
    /// client's for the scope and resource it asked.
    #[test]
    fn what_one_entry_read_or_was_given_is_not_given_to_another_entry() {
        let reader = entry(json!({}));
        let admin = entry(json!({ "name": "notes_admin" }));
        assert_ne!(discovery_key(&reader), discovery_key(&admin));
        assert_eq!(discovery_key(&reader), discovery_key(&entry(json!({}))));
        let client = |scope: &str| OAuthClient {
            token_url: "https://auth.example.com/token".to_owned(),
            client_id: "c".to_owned(),
            client_secret_env: "S".to_owned(),
            scope: Some(scope.to_owned()),
            resource: None,
            client_secret: None,
        };
        assert_ne!(
            token_key(&client("notes.read")),
            token_key(&client("notes.write"))
        );
        assert_eq!(
            token_key(&client("notes.read")),
            token_key(&client("notes.read"))
        );
    }

    #[test]
    fn an_mcp_servers_tools_become_tools_of_the_orchestrator() {
        let listing = json!({ "tools": [
            { "name": "search", "description": "Searches notes.", "annotations": { "readOnlyHint": true },
              "inputSchema": { "type": "object", "properties": { "q": { "type": "string" } } } },
            { "name": "delete.note", "description": "Deletes a note.",
              "annotations": { "destructiveHint": true } },
            { "name": "add", "title": "Adds a note", "annotations": { "destructiveHint": false } },
            { "name": "plain" },
            { "description": "no name" },
        ] });
        let spec = entry(json!({ "tools": ["*"], "url": "https://mcp.example.com/tools-1" }));
        let found = (Instant::now(), Arc::new(listing.clone()));
        DISCOVERED.lock().insert(discovery_key(&spec), found);
        let other =
            entry(json!({ "name": "search", "type": "webhook", "url": "https://h.example.com/" }));
        let tools = agent_tools(&[&other, &spec]);
        let names: Vec<&str> = tools.iter().map(|(tool, _)| tool.name.as_str()).collect();
        // A name taken by another entry gets its entry's name; a model may call each.
        assert_eq!(
            names,
            ["search", "notes_search", "delete_note", "add", "plain"]
        );
        let (search, runs) = &tools[1];
        assert_eq!(search.description, "Searches notes.");
        assert_eq!(
            search.parameters.as_ref().expect("its schema")["properties"]["q"]["type"],
            "string"
        );
        assert_eq!(
            (runs.tool.as_deref(), runs.side_effects),
            (Some("search"), SideEffects::None)
        );
        // Only a tool that may destroy is confirmed; MCP's default for one that is not
        // read-only is that it may.
        let confirms: Vec<bool> = tools.iter().map(|(tool, _)| tool.confirm).collect();
        assert_eq!(confirms, [true, false, true, false, true]);
        assert_eq!(tools[2].1.tool.as_deref(), Some("delete.note"));
        assert_eq!(tools[3].0.description, "Adds a note");
        assert_eq!(
            tools[4].0.parameters,
            Some(json!({ "type": "object", "properties": {} }))
        );
        assert_eq!(tools[2].1.side_effects, SideEffects::Possible);

        // The operator names the tools, and overrides what the server says.
        let named = entry(
            json!({ "tools": ["search", "add"], "url": "https://mcp.example.com/tools-1",
                                  "confirm": { "add": true, "search": true } }),
        );
        let tools = agent_tools(&[&named]);
        let confirms: Vec<(&str, bool)> = tools
            .iter()
            .map(|(tool, _)| (tool.name.as_str(), tool.confirm))
            .collect();
        assert_eq!(confirms, [("search", true), ("add", true)]);
        // An entry that changes nothing confirms nothing, unless it says.
        let reads = entry(
            json!({ "tools": ["*"], "url": "https://mcp.example.com/tools-1",
                                  "side_effects": "none" }),
        );
        assert!(agent_tools(&[&reads]).iter().all(|(tool, _)| !tool.confirm));
        let every = entry(
            json!({ "tools": ["*"], "url": "https://mcp.example.com/tools-1",
                                  "side_effects": "none", "confirm": true }),
        );
        assert!(agent_tools(&[&every]).iter().all(|(tool, _)| tool.confirm));
        // A listing past its catalog_ttl_s is not used; nor a server never read.
        DISCOVERED.lock().insert(
            discovery_key(&spec),
            (Instant::now() - Duration::from_secs(301), Arc::new(listing)),
        );
        assert!(agent_tools(&[&spec]).is_empty());
        // An A2A agent or a webhook is one tool, confirmed when it may change things.
        let hook =
            entry(json!({ "name": "hook", "type": "webhook", "url": "https://h.example.com/" }));
        let (tool, _) = &agent_tools(&[&hook])[0];
        assert_eq!(
            (tool.description.as_str(), tool.confirm, &tool.parameters),
            ("The hook agent.", true, &None)
        );
        let off = entry(
            json!({ "name": "hook", "type": "webhook", "url": "https://h.example.com/",
                                "confirm": false, "description": "Hooks." }),
        );
        let (tool, _) = &agent_tools(&[&off])[0];
        assert_eq!((tool.description.as_str(), tool.confirm), ("Hooks.", false));
    }

    #[test]
    fn a_streamed_a2a_task_is_read_as_it_comes() {
        let backend = remote("a2a");
        let mut wire = A2a::new(&backend, None);
        let says = |text: &str| json!({ "role": "ROLE_AGENT", "parts": [{ "text": text }] });
        // 1.0: the task, then updates of its status and its artifacts.
        let task = json!({ "task": { "id": "t1", "contextId": "c1", "status": { "state": "TASK_STATE_SUBMITTED" } } });
        assert!(wire.update(&task).is_empty());
        let working = json!({ "statusUpdate": { "taskId": "t1", "status":
            { "state": "TASK_STATE_WORKING", "message": says("Looking.") } } });
        assert_eq!(
            wire.update(&working),
            [Step::Working(Some("Looking.".to_owned()))]
        );
        let asks = json!({ "statusUpdate": { "taskId": "t1", "status":
            { "state": "TASK_STATE_INPUT_REQUIRED", "message": says("Budget?") } } });
        assert_eq!(
            wire.update(&asks),
            [Step::Ask(Ask::asking("", "Budget?", &[]))]
        );
        let part = |text: &str, append: bool| {
            json!({ "artifactUpdate": { "taskId": "t1", "append": append,
            "artifact": { "artifactId": "a", "parts": [{ "text": text }] } } })
        };
        assert!(wire.update(&part("Booked,", false)).is_empty());
        assert!(wire.update(&part("two nights.", true)).is_empty());
        let done = json!({ "statusUpdate": { "taskId": "t1", "status": { "state": "TASK_STATE_COMPLETED" } } });
        assert_eq!(
            wire.update(&done),
            [Step::Done("Booked, two nights.".to_owned())]
        );
        assert_eq!(
            (wire.id.as_deref(), wire.handle().as_deref()),
            (Some("t1"), Some("c1"))
        );
        // 0.3 names the kind of each result.
        let mut wire = A2a::new(&backend, None);
        let task = json!({ "kind": "task", "id": "t2", "contextId": "c2", "status": { "state": "working" } });
        assert!(wire.update(&task).is_empty());
        let done = json!({ "kind": "status-update", "taskId": "t2", "status":
            { "state": "completed", "message": says("All set.") }, "final": true });
        assert_eq!(wire.update(&done), [Step::Done("All set.".to_owned())]);
        let reply = json!({ "kind": "message", "role": "agent", "parts": [{ "kind": "text", "text": "Hi." }] });
        assert_eq!(wire.update(&reply), [Step::Done("Hi.".to_owned())]);
    }

    #[test]
    fn a_tools_progress_arguments_and_effect_are_read() {
        assert_eq!(
            progress_text(&json!({ "progress": 1, "total": 4, "message": "Saving." })).as_deref(),
            Some("Saving.")
        );
        assert_eq!(
            progress_text(&json!({ "progress": 1, "total": 4 })).as_deref(),
            Some("25% done.")
        );
        assert_eq!(progress_text(&json!({ "progress": 1 })), None);
        assert_eq!(
            tool_arguments(r#"delete_note {"title":"old list"}"#),
            Some(json!({ "title": "old list" }))
        );
        assert_eq!(tool_arguments("delete the old list"), None);
        assert_eq!(tool_arguments("delete_note [1]"), None);
        // A call of a server's tool that may change things says it ran once it is done.
        let mut spec = entry(json!({ "tools": ["*"] }));
        spec.tool = Some("delete_note".to_owned());
        let backend = Remote::new(spec, None, ["", ""], Vec::new());
        let wire = Mcp {
            backend: &backend,
            id: None,
            poll: POLL,
            said: None,
            answered: HashSet::new(),
            follows: None,
            events: None,
            state: None,
            heard: Heard::new(&backend.spec),
        };
        assert!(wire.reports_effects());
        let done = vec![Step::Done("Deleted.".to_owned())];
        assert_eq!(
            wire.ran(done),
            [
                Step::Effect("ran delete note".to_owned()),
                Step::Done("Deleted.".to_owned())
            ]
        );
        assert_eq!(
            wire.ran(vec![Step::Failed("no".to_owned())]),
            [Step::Failed("no".to_owned())]
        );
    }
}
