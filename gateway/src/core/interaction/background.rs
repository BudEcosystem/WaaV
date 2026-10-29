//! The background: the one interface every backend of a session's tasks implements
//! ([`Backend`]), and the LLM tier's run driver: one run per task revision, with the answer
//! gate, the sentence pump, the tool loop and the gated, signed HTTP tools.
//!
//! The other backends are the Bud agent (`bud_agent.rs`), a text-output DAG (through
//! [`TextBackend`]) and the operator's remote backends (`remote.rs`).
//!
//! A run keeps its own history key on the shared [`LlmClient`], so runs never interleave, and
//! reports to the actor as `Msg::Run(run, RunEvent)`. Its tools live in a registry of its own
//! on a clone of the client (same HTTP pool, same histories), so a tool handler knows its run
//! without a lookup. A configured tool is an HTTP endpoint:
//!
//! ```text
//! POST <ToolSpec.url>
//! X-WaaV-Timestamp: <unix seconds>
//! X-WaaV-Signature: sha256=<hex HMAC-SHA256 of "<timestamp>.<body>" under the tool secret>
//!                                            (tools of the server's own block only)
//! Idempotency-Key: <operation id>            (side-effecting tools only)
//!
//! {"tool": "<name>", "arguments": {...}, "operation_id": "<operation id>" | null}
//! ```
//!
//! A 2xx body is the tool result (JSON, or any text); its string field `effect`, when present,
//! is the label of what was committed. A 4xx means the tool refused and nothing happened.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use hmac::{Hmac, Mac};
use parking_lot::Mutex;
use serde_json::{Value, json};
use sha2::Sha256;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, debug};

use crate::core::agent::text::transform_for_speech;
use crate::core::conversation::{TurnAttribution, turn_span_under};
use crate::core::llm::functions::execute_batch;
use crate::core::llm::{
    ChatMessage, FunctionCallParams, FunctionHandler, FunctionRegistry, FunctionResult, LlmClient,
    LlmError, LlmResponse, LlmResult, MessageRole, RegistryItem, TokenCallback, ToolCall,
    ToolDefinition,
};
use crate::core::metrics::bridge;
use crate::core::net::{
    HTTP_URL_SCHEMES, shared_http_client, ssrf_protected_client_builder, validate_url_for_ssrf,
};
use crate::core::text::{SentenceAggregator, ThinkStripper, strip_markdown_for_tts};
use crate::observability::voice_attrs;
use bud_auth::voice_agent::AgentTextTransforms;

use super::actor::Msg;
use super::config::{ASK_CALLER, ToolSpec};
use super::controls::{Control, ControlKind, ControlSource, same_language};
use super::guard;
use super::turn_record::TurnTrace;
use super::types::{
    GateDecision, GuardOutcome, HitlKind, OpId, Outcome, RunEvent, RunId, TaskId, TaskState,
};

/// Tool rounds of one run before it gives up.
const MAX_ROUNDS: usize = 8;
/// One tool request. A side-effecting request is never abandoned sooner, so its outcome is known.
const TOOL_HTTP_TIMEOUT: Duration = Duration::from_secs(30);
/// Backstop for one tool call, the wait for the caller's answer included.
pub(super) const TOOL_CALL_BACKSTOP: Duration = Duration::from_secs(600);
/// How much of a tool's error body the model is shown.
pub(super) const ERROR_BODY_CHARS: usize = 500;
/// How much of a tool's response is read: the rest is not kept and never reaches the model
/// (the published `result_caps.text_bytes`).
pub(super) const RESULT_MAX_BYTES: usize = 16_384;

/// One run of a task: what the caller asked for and what was said before.
pub struct Job {
    pub run: RunId,
    /// The task and its revision. A run started before its turn was settled has no task yet.
    pub task: Option<(TaskId, u32)>,
    /// The heard ledger as it was before the task's request; `words` follow it.
    pub history: Vec<ChatMessage>,
    /// The caller's words: the original request, then each correction.
    pub words: Vec<String>,
    /// The session's live credential for an LLM leg, when it has one.
    pub api_key: Option<String>,
    /// The session was stepped down to the plain path: the gateway's tools are off.
    pub plain: bool,
    /// What is known about the caller (`Persona::caller_context`), for a backend the gateway
    /// prompts: empty when nothing is.
    pub context: String,
    /// The task's trace (`Lineage::trace_id`): every request of the run to an agent or a tool
    /// carries it in a `traceparent` header.
    pub trace_id: String,
    /// A child task: its answer goes to the orchestrator that started it, not to the caller.
    pub child: bool,
    /// A follow-up: what continues the finished task it follows on its backend (the A2A
    /// `contextId`, the webhook's task id, the MCP task id, the Responses response id).
    pub handle: Option<String>,
    /// The conversation state (chapter 9): `metadata.waav_state` on the webhook and A2A
    /// wires, `_meta["io.waav/state"]` on MCP. `None` on a surface that has none.
    pub state: Option<Value>,
    /// The caller turn the run belongs to: its span is a child of the turn's `voice.turn`, in
    /// the turn's trace (spec 029 D-9). `None`: the run is a trace of its own.
    pub turn: Option<TurnTrace>,
}

tokio::task_local! {
    /// The trace of the task a run works for.
    static TRACE: String;
}

/// Runs `work` for the task trace `trace_id`: each request it makes through [`send_json`]
/// carries a W3C `traceparent` with that trace id and a span id of its own.
pub(super) fn traced<F: Future>(trace_id: String, work: F) -> impl Future<Output = F::Output> {
    TRACE.scope(trace_id, work)
}

/// The `traceparent` of a request made now, when it is made for a task.
fn traceparent() -> Option<String> {
    let trace = TRACE.try_with(Clone::clone).ok()?;
    let span = uuid::Uuid::new_v4().simple().to_string();
    (trace.len() == 32).then(|| format!("00-{trace}-{}-01", &span[..16]))
}

/// The language a run answers in when the caller switched away from the pack's (the state's
/// `language`), by name. Live CT2 (qwen2.5-3b): with the state only in the system prompt the
/// answer after a switch to Spanish stayed English when the history held English lines.
fn reply_in(state: Option<&serde_json::Value>) -> Option<String> {
    let language = state?.get("language")?.as_str()?;
    (!super::controls::same_language(language, super::persona::language()))
        .then(|| super::controls::language_name(language))
}

/// A system prompt with the caller's context after it.
pub(super) fn with_context(system_prompt: &str, context: &str) -> String {
    if context.is_empty() {
        system_prompt.to_owned()
    } else {
        format!("{system_prompt}\n\n{context}").trim().to_owned()
    }
}

/// What does a session's tasks. A run reports to the actor as `Msg::Run(job.run, ..)` and
/// stops when `cancel` fires; its last report is `Finished`, `Failed` or `Cancelled`.
pub trait Backend: Send + Sync {
    /// `tasks.backend` of the settings echo, and `background` of `task_created`.
    fn label(&self) -> String;
    /// `None`: the gateway prompts it and runs the session's tools for it (the LLM tier).
    /// `Some`: it has tools of its own, so it runs one task at a time and none of the
    /// gateway's; the value says whether a run may change something outside the gateway.
    fn own_tools(&self) -> Option<bool>;
    fn start(
        self: Arc<Self>,
        job: Job,
        tx: mpsc::Sender<Msg>,
        cancel: CancellationToken,
    ) -> JoinHandle<()>;
    /// The caller's requests it runs at once: `None` for no limit.
    fn max_concurrent(&self) -> Option<usize> {
        None
    }
    /// The root `voice.turn` of a caller turn whose runs this backend makes (spec 029 D-9),
    /// with what its runs' spans say of the session, so the turn's row names it whether or not
    /// a run starts. A run's own span is its child (`Job::turn`).
    fn turn_span(&self, session_id: &str, turn: u64, input: &str) -> tracing::Span {
        use crate::observability::{trace_redact, voice_attrs};
        let span = crate::voice_turn_span!(parent: None, capability = "conversation", transport = "websocket");
        span.record(voice_attrs::turn::SESSION_ID, session_id);
        span.record(voice_attrs::turn::TURN_INDEX, turn);
        if trace_redact::capture_content() {
            let transcript = trace_redact::sanitize_body(input);
            span.record(voice_attrs::turn::TRANSCRIPT, transcript.as_str());
        }
        span
    }
    /// The caller heard `text` of a run's answer so far; `cut`: they will not hear the rest.
    fn heard(&self, _run: RunId, _text: &str, _cut: bool) {}
    /// The model the gateway prompts for the session, when it has one (the LLM tier): after
    /// the call it writes what is worth remembering about the caller ([`caller_facts`]).
    fn llm(&self) -> Option<Arc<LlmClient>> {
        None
    }
}

/// The LLM tier: any OpenAI-compatible model, prompted by the gateway, with the session's
/// tools. As an orchestrator (`tasks.orchestrator`) it also has the session's agents.
pub struct LlmTier {
    pub llm: Arc<LlmClient>,
    /// Persona preamble and operator prompt.
    pub system_prompt: String,
    /// The operator prompt alone: an orchestrator's control round reads it ([`control_round`]).
    pub operator_prompt: String,
    pub tools: Vec<ToolSpec>,
    pub agents: Vec<AgentTool>,
    pub tool_secret: Option<String>,
    pub session_id: String,
    pub attribution: Option<TurnAttribution>,
}

impl Backend for LlmTier {
    fn label(&self) -> String {
        "llm".to_owned()
    }

    fn own_tools(&self) -> Option<bool> {
        None
    }

    fn llm(&self) -> Option<Arc<LlmClient>> {
        Some(Arc::clone(&self.llm))
    }

    /// What `spawn_run` records of a run: the session, the words and, in Bud mode, the caller.
    fn turn_span(&self, _session_id: &str, turn: u64, input: &str) -> tracing::Span {
        let span = turn_span_under(None, &self.session_id, input, self.attribution.as_ref());
        span.record(voice_attrs::turn::TURN_INDEX, turn);
        span
    }

    fn start(
        self: Arc<Self>,
        job: Job,
        tx: mpsc::Sender<Msg>,
        cancel: CancellationToken,
    ) -> JoinHandle<()> {
        let spec = RunSpec {
            run: job.run,
            history_key: format!("{}#r{}", self.session_id, job.run.0),
            llm: self.llm.clone(),
            api_key: job.api_key,
            system_prompt: with_context(&self.system_prompt, &job.context),
            // Not the caller context: its "prefers brief answers" is no one asking for a change.
            operator_prompt: self.operator_prompt.clone(),
            history: job.history,
            words: job.words,
            // The plain path has no tools: their questions would be asked in English.
            tools: if job.plain {
                Vec::new()
            } else {
                self.tools.clone()
            },
            agents: if job.plain {
                Vec::new()
            } else {
                self.agents.clone()
            },
            tool_secret: self.tool_secret.clone(),
            session_id: self.session_id.clone(),
            attribution: self.attribution.clone(),
            trace_id: job.trace_id,
            reply_in: reply_in(job.state.as_ref()),
            state: job.state,
            turn: job.turn,
        };
        spawn_run(spec, tx, cancel)
    }
}

/// An agent of the catalog as a tool of the orchestrator.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentTool {
    /// The catalog entry's name: the tool's name.
    pub name: String,
    /// The entry's `description`: what the orchestrator reads to choose it.
    pub description: String,
    /// The entry that takes its new tasks while its breaker is open.
    pub fallback: Option<String>,
    /// An MCP server's tool: its input schema. The call's arguments are the request then;
    /// without it the agent takes one `request` in words.
    pub parameters: Option<Value>,
    /// The caller confirms each call, read back from its arguments, before it is sent.
    pub confirm: bool,
}

/// A [`TextBackend`] as the session's backend.
pub struct Text(pub Arc<dyn TextBackend>);

impl Backend for Text {
    fn label(&self) -> String {
        self.0.label().to_owned()
    }

    fn own_tools(&self) -> Option<bool> {
        Some(self.0.side_effects())
    }

    /// A DAG runs one task at a time.
    fn max_concurrent(&self) -> Option<usize> {
        Some(1)
    }

    fn start(
        self: Arc<Self>,
        job: Job,
        tx: mpsc::Sender<Msg>,
        cancel: CancellationToken,
    ) -> JoinHandle<()> {
        let caller = caller_words(&job.history, &job.words);
        spawn_text_run(job.run, self.0.clone(), &job.words, caller, tx, cancel)
    }
}

/// Everything one LLM-tier run needs.
pub struct RunSpec {
    pub run: RunId,
    /// The history key of this run on the shared client: `"<stream_id>#r<run>"`.
    pub history_key: String,
    pub llm: Arc<LlmClient>,
    /// The key for this run's LLM calls: the session's live credential when it has one, else
    /// `None` (the client's configured key).
    pub api_key: Option<String>,
    /// Persona preamble and operator prompt. The answer-marker rule is added here when the run
    /// has tools.
    pub system_prompt: String,
    /// The operator prompt alone (no persona preamble, no caller context): the system prompt
    /// of an orchestrator's control round ([`control_round`]).
    pub operator_prompt: String,
    /// The conversation state when the run began: a control round applies only a change of it.
    pub state: Option<Value>,
    /// The heard ledger as it was before the task's request; `words` follow it.
    pub history: Vec<ChatMessage>,
    /// The caller's words: the original request, then each correction.
    pub words: Vec<String>,
    pub tools: Vec<ToolSpec>,
    /// The orchestrator's agents: calling one starts a child task and returns at once.
    pub agents: Vec<AgentTool>,
    pub tool_secret: Option<String>,
    /// The session, and in Bud mode its caller: both are recorded on the run's `voice.turn`
    /// span, as the conversation loop records them for a turn.
    pub session_id: String,
    pub attribution: Option<TurnAttribution>,
    /// The task's trace, for the `traceparent` of its requests.
    pub trace_id: String,
    /// The caller turn the run belongs to (`Job::turn`).
    pub turn: Option<TurnTrace>,
    /// The language the caller switched to (a `language` control), by name: the request says
    /// so, last, where a model that answered the call so far in another language reads it.
    pub reply_in: Option<String>,
}

/// A background the gateway neither prompts nor gates: it turns a request into the text of an
/// answer. A `/ws` session's text-output DAG is one.
#[async_trait::async_trait]
pub trait TextBackend: Send + Sync {
    /// `tasks.backend` of the settings echo, and `background` of `task_created`.
    fn label(&self) -> &str;
    /// Whether a run may change something outside the gateway.
    fn side_effects(&self) -> bool;
    /// Answer `request` (the caller's request, then each correction on a line of its own).
    /// Every piece of text sent on `out` is spoken, sentence by sentence, as it arrives.
    async fn run(
        &self,
        request: String,
        out: mpsc::Sender<String>,
        cancel: CancellationToken,
    ) -> Result<(), String>;
}

/// Start a run of a [`TextBackend`]. It reports like [`spawn_run`]: sentences while the text
/// arrives, then `Finished`, `Failed` or `Cancelled`. `caller`: what the caller said, for the
/// outbound speech policy.
pub fn spawn_text_run(
    run: RunId,
    backend: Arc<dyn TextBackend>,
    words: &[String],
    caller: String,
    tx: mpsc::Sender<Msg>,
    cancel: CancellationToken,
) -> JoinHandle<()> {
    let request = request_text(words);
    tokio::spawn(async move {
        let (out, mut texts) = mpsc::channel::<String>(16);
        let mut relay = Relay::new(run, &tx, AnswerGate::new(false), caller);
        let work = backend.run(request, out, cancel.clone());
        tokio::pin!(work);
        // The backend holds the sender until it is done: the texts end with it.
        let mut outcome = None;
        let last = loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => break RunEvent::Cancelled,
                text = texts.recv() => match text {
                    Some(text) => {
                        relay.push(&text).await;
                        // Two pieces of text are two stretches of speech.
                        relay.end(false).await;
                    }
                    None => match outcome.take() {
                        Some(Err(error)) => break RunEvent::Failed { error },
                        Some(Ok(())) if relay.answer.is_empty() => {
                            let error = format!("the {} returned no answer", backend.label());
                            break RunEvent::Failed { error };
                        }
                        Some(Ok(())) => break RunEvent::Finished { text: relay.answer.join(" ") },
                        // It let go of the sender early: its result is still to come.
                        None => tokio::select! {
                            biased;
                            _ = cancel.cancelled() => break RunEvent::Cancelled,
                            result = &mut work => outcome = Some(result),
                        },
                    },
                },
                result = &mut work, if outcome.is_none() => outcome = Some(result),
            }
        };
        let _ = tx.send(Msg::Run(run, last)).await;
    })
}

/// Start a run. It reports through `Msg::Run(spec.run, ..)` and stops when `cancel` fires; its
/// last report is `Finished`, `Failed` or `Cancelled`. A side-effecting tool request that is
/// already on the wire is awaited even then, so what it committed is reported first.
pub fn spawn_run(
    spec: RunSpec,
    tx: mpsc::Sender<Msg>,
    cancel: CancellationToken,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let (run, llm, key) = (spec.run, Arc::clone(&spec.llm), spec.history_key.clone());
        // The run is the turn budmetrics records: its LLM calls are made inside this span.
        let request = request_text(&spec.words);
        // A child of its caller turn's span, in the turn's trace (spec 029 D-9).
        let parent = spec.turn.as_ref().and_then(|turn| turn.span.id());
        let span = turn_span_under(
            parent,
            &spec.session_id,
            &request,
            spec.attribution.as_ref(),
        );
        let index = spec.turn.as_ref().map_or(run.0, |turn| turn.index);
        span.record(voice_attrs::turn::TURN_INDEX, index);
        span.record(voice_attrs::leg::LLM_MODEL, llm.config().model.as_str());
        let trace_id = spec.trace_id.clone();
        let driven = traced(trace_id, drive(spec, &tx, &cancel));
        let last = match driven.instrument(span).await {
            Ok(last) => last,
            Err(LlmError::Cancelled) => RunEvent::Cancelled,
            Err(error) => RunEvent::Failed {
                error: error.to_string(),
            },
        };
        llm.remove_history(&key).await;
        // A control round the run aborted (cancelled, failed) did not remove its own.
        llm.remove_history(&control_key(&key)).await;
        let _ = tx.send(Msg::Run(run, last)).await;
    })
}

/// The history key of a run's control round ([`control_round`]).
fn control_key(key: &str) -> String {
    format!("{key}#control")
}

/// The run itself: seed the history, then the caller-owned tool loop (the shape of
/// `llm::run_tool_loop`, with the token callback on every round).
async fn drive(
    spec: RunSpec,
    tx: &mpsc::Sender<Msg>,
    cancel: &CancellationToken,
) -> LlmResult<RunEvent> {
    let (key, run) = (spec.history_key, spec.run);
    let mut system = spec.system_prompt;
    let (lookups, changes) = tool_kinds(&spec.tools);
    // An orchestrator of agents alone does not ask the caller: its agents ask what they need.
    let asks = !spec.tools.is_empty() || spec.agents.is_empty();
    let orchestrator = !spec.agents.is_empty();
    let caller = caller_words(&spec.history, &spec.words);
    let tools = (!spec.tools.is_empty() || !spec.agents.is_empty()).then(|| {
        system.push_str(if asks { ANSWER_RULE } else { MARKER_RULE });
        if !spec.agents.is_empty() {
            system.push_str(ORCHESTRATOR_RULE);
        }
        let sources = vec![spec.operator_prompt.clone(), caller.clone()];
        let calls = ToolCalls::new(spec.run, tx, cancel, spec.tool_secret).named_by(sources);
        let calls = Arc::new(calls);
        let registry = tool_registry(spec.tools, spec.agents, Arc::clone(&calls));
        (Arc::new(registry), calls)
    });
    let llm = match &tools {
        Some((registry, _)) => {
            Arc::new(LlmClient::clone(&spec.llm).with_functions(Arc::clone(registry)))
        }
        None => spec.llm,
    };
    let roles = guard::role_lines(&[&system, &spec.operator_prompt]);
    let mut context = vec![ChatMessage::system(system)];
    // Earlier runs' tool rounds only where there are tools.
    let spoken =
        |message: &ChatMessage| message.tool_calls.is_none() && message.role != MessageRole::Tool;
    context.extend(
        spec.history
            .into_iter()
            .filter(|message| tools.is_some() || spoken(message)),
    );
    llm.set_context(&key, context).await;

    // The token callback is synchronous: it hands each delta to this task, which gates it,
    // cuts sentences and reports them while the round is still streaming.
    let (token_tx, mut tokens) = mpsc::unbounded_channel::<String>();
    let on_token: TokenCallback = Arc::new(move |delta: &str| {
        let _ = token_tx.send(delta.to_owned());
    });
    let mut relay = Relay::new(spec.run, tx, AnswerGate::new(tools.is_some()), caller);
    relay.roles = roles;
    let api_key = spec.api_key.as_deref();
    // Tool handlers watch the run's token themselves; the batch executor must not drop a
    // side-effecting request half way.
    let never = CancellationToken::new();

    // An orchestrator's controls are also asked apart, beside its first round.
    let control = (tools.as_ref()).filter(|_| orchestrator).map(|(_, calls)| {
        Aborting(tokio::spawn(control_round(
            Arc::clone(&llm),
            control_key(&key),
            spec.operator_prompt.clone(),
            request_text(&spec.words),
            spec.state.clone(),
            Arc::clone(calls),
            spec.api_key.clone(),
            cancel.clone(),
        )))
    });
    let mut request = request_text(&spec.words);
    if let Some(language) = &spec.reply_in {
        request.push_str(&format!("\n[Answer in {language}.]"));
    }
    let asked = tokio::time::Instant::now();
    let first = llm.complete(&key, &request, api_key, cancel, Some(Arc::clone(&on_token)));
    let mut response = relay.round(&mut tokens, first).await?;
    // The LLM leg of the turn's record: the round that answered, or that chose a tool.
    let llm_ms = u64::try_from(asked.elapsed().as_millis()).unwrap_or(u64::MAX);
    tracing::Span::current().record(voice_attrs::leg::LLM_DURATION_MS, llm_ms);
    drop_cut_calls(&llm, &key, &mut response).await;
    let mut rounds = 0;
    while let Some((registry, calls)) = tools.as_ref() {
        if response.tool_calls.is_empty() {
            // An orchestrator hears each child's end as a later message, and goes on: it may
            // say the result or start the next step. What it writes from here on is not
            // spoken: each child's result is said as an update, worded for the moment, at a
            // slot of the conversation (chapter 8).
            if calls.waiting() && !std::mem::replace(&mut relay.muted, true) {
                calls.report(RunEvent::Answered).await;
            }
            let Some(outcome) = calls.next_child().await? else {
                break;
            };
            rounds = 0;
            relay.source(&outcome.summary);
            let update = child_update(&outcome);
            let next = llm.complete(&key, &update, api_key, cancel, Some(Arc::clone(&on_token)));
            response = relay.round(&mut tokens, next).await?;
            drop_cut_calls(&llm, &key, &mut response).await;
            continue;
        }
        if rounds == MAX_ROUNDS {
            return Ok(RunEvent::Failed {
                error: format!("no answer after {MAX_ROUNDS} rounds of tool calls"),
            });
        }
        rounds += 1;
        let batch = std::mem::take(&mut response.tool_calls);
        if rounds == 1 && looks_up_only(&batch, &lookups) {
            // A run whose first calls only looked something up is a lookup: its later rounds
            // are not offered a tool that changes something (live D1: the 3B booked the table it
            // had found 3 of 20 times offline, unasked; 0 of 20 without the offer). A change it
            // found is the caller's to ask for, in a run of its own.
            for name in &changes {
                registry.unregister(name);
            }
        }
        let results = execute_batch(registry, &key, &batch, &never, false, 0).await;
        if calls.unconfirmed.load(Ordering::Relaxed) {
            // Nobody knows whether a confirmed call went through. The model is not left to
            // word that, or to do more on top of it: the task ends with what is known, and
            // the caller hears it from the gateway.
            return Ok(RunEvent::Failed {
                error: "a confirmed tool call did not report its outcome".to_owned(),
            });
        }
        let rendered: Vec<(String, String)> = batch
            .iter()
            .zip(results)
            .map(|(call, value)| (call.id.clone(), value.to_string()))
            .collect();
        let round = tool_round(&batch, &rendered);
        llm.add_tool_results_batch(&key, rendered).await;
        if !round.is_empty() {
            let _ = tx.send(Msg::ToolRound(run, round)).await;
        }
        let next = llm.continue_from_history(&key, api_key, cancel, Some(Arc::clone(&on_token)));
        response = relay.round(&mut tokens, next).await?;
        drop_cut_calls(&llm, &key, &mut response).await;
    }
    // The run ends after its control round: a control it applies is reported by the run.
    if let Some(mut control) = control {
        let _ = (&mut control.0).await;
    }
    if relay.answer.is_empty() {
        return Ok(RunEvent::Failed {
            error: "the model returned no answer".to_owned(),
        });
    }
    Ok(RunEvent::Finished {
        text: relay.answer.join(" "),
    })
}

/// The names of a run's own tools that only look something up, and of those that change
/// something (`side_effects`).
fn tool_kinds(tools: &[ToolSpec]) -> (HashSet<String>, Vec<String>) {
    let named = |changes: bool| {
        let kind = tools
            .iter()
            .filter(move |tool| tool.side_effects == changes);
        kind.map(|tool| tool.name.clone())
    };
    (named(false).collect(), named(true).collect())
}

/// Whether every call of a round is to a tool that only looks something up.
fn looks_up_only(batch: &[ToolCall], lookups: &HashSet<String>) -> bool {
    !batch.is_empty() && (batch.iter()).all(|call| lookups.contains(&call.function.name))
}

/// A round's calls and their results as the history of the session's later runs shows them
/// (`Msg::ToolRound`): the questions to the caller left out, their answers may be values given
/// for that call only.
fn tool_round(batch: &[ToolCall], results: &[(String, String)]) -> Vec<ChatMessage> {
    let kept: Vec<ToolCall> = (batch.iter())
        .filter(|call| call.function.name != ASK_CALLER)
        .cloned()
        .collect();
    if kept.is_empty() {
        return Vec::new();
    }
    let ids: HashSet<&str> = kept.iter().map(|call| call.id.as_str()).collect();
    let call = ChatMessage {
        content: None,
        tool_calls: Some(kept.clone()),
        ..ChatMessage::assistant("")
    };
    let answers = (results.iter())
        .filter(|(id, _)| ids.contains(id.as_str()))
        .map(|(id, value)| ChatMessage::tool(id, value));
    std::iter::once(call).chain(answers).collect()
}

/// Whether a tool call arrived whole: its arguments are JSON (or none).
fn whole(call: &ToolCall) -> bool {
    let arguments = call.function.arguments.trim();
    arguments.is_empty() || serde_json::from_str::<Value>(arguments).is_ok()
}

/// A round that ends at the model's token limit leaves its last tool call with half its
/// arguments. Such a call is not made, and it is taken out of the run's history: a model
/// server refuses a request that replays it (llama-server: HTTP 500, "Failed to parse tool
/// call arguments as JSON").
async fn drop_cut_calls(llm: &LlmClient, key: &str, response: &mut LlmResponse) {
    if response.tool_calls.iter().all(whole) {
        return;
    }
    debug!(key, "interaction tool call cut off by the token limit");
    response.tool_calls.retain(whole);
    let mut history = llm.history_snapshot(key).await;
    if let Some(calls) = history.last_mut().and_then(|last| last.tool_calls.as_mut()) {
        calls.retain(whole);
    }
    if let Some(last) = history.last_mut() {
        last.tool_calls.take_if(|calls| calls.is_empty());
    }
    llm.set_context(key, history).await;
}

/// Everything the caller said, for the outbound speech policy: a long number in an answer must
/// be one of theirs.
pub(super) fn caller_words(history: &[ChatMessage], words: &[String]) -> String {
    let said: Vec<&str> = history
        .iter()
        .filter(|message| message.role == MessageRole::User)
        .filter_map(|message| message.content.as_deref())
        .chain(words.iter().map(String::as_str))
        .collect();
    said.join(" ")
}

/// The sentence pump of one run: tokens through the answer gate, sentences to the actor.
pub(super) struct Relay<'a> {
    run: RunId,
    tx: &'a mpsc::Sender<Msg>,
    gate: AnswerGate,
    /// What the caller said, for the outbound speech policy.
    caller: String,
    /// Raised when a round streamed answer text and then called a tool.
    attempt: u32,
    seq: u32,
    /// The sentences of the current attempt.
    pub(super) answer: Vec<String>,
    /// A Bud agent's own cleaning of what it says, applied to each sentence.
    pub(super) transforms: Option<AgentTextTransforms>,
    /// Each sentence passes the outbound speech policy; a child's answer is not spoken.
    pub(super) guarded: bool,
    /// The lines of the system prompt the gateway sent the background that say who it is
    /// ([`guard::role_lines`]): a sentence that repeats one is not spoken
    /// ([`guard::repeats_prompt`]). Empty for a backend the gateway does not prompt.
    pub(super) roles: String,
    /// The sentences are part of the answer the task reports, and are not spoken: an
    /// orchestrator's words after its children's results.
    pub(super) muted: bool,
    /// The first token is reported (`Msg::RunAlive`): the health watcher's first-token
    /// deadline of the session's background (chapter 10).
    first_token_owed: bool,
}

impl<'a> Relay<'a> {
    pub(super) fn new(
        run: RunId,
        tx: &'a mpsc::Sender<Msg>,
        gate: AnswerGate,
        caller: String,
    ) -> Self {
        Self {
            run,
            tx,
            gate,
            caller,
            attempt: 0,
            seq: 0,
            answer: Vec::new(),
            transforms: None,
            guarded: true,
            roles: String::new(),
            muted: false,
            first_token_owed: true,
        }
    }

    /// The run shows its first sign of life (a token, or a backend's answer to the request):
    /// the health watcher's first-token deadline is met.
    pub(super) async fn alive(&mut self) {
        if std::mem::take(&mut self.first_token_owed) {
            let _ = self.tx.send(Msg::RunAlive(self.run)).await;
        }
    }

    /// Text the answer may draw on, as it may on the caller's words: a child task's result.
    pub(super) fn source(&mut self, text: &str) {
        self.caller.push(' ');
        self.caller.push_str(text);
    }

    /// One piece of streamed text: the answer sentences it completes are released.
    pub(super) async fn push(&mut self, token: &str) {
        self.alive().await;
        let sentences = self.gate.push(token);
        self.release(sentences).await;
    }

    /// The end of a round, or of a stretch of text before a tool item: what is left of the
    /// answer is released, and a tool call after answer text is reported.
    pub(super) async fn end(&mut self, tool_call: bool) {
        let end = self.gate.end_round(tool_call);
        self.release(end.sentences).await;
        if end.marker_then_tool {
            let _ = self
                .tx
                .send(Msg::Run(self.run, RunEvent::MarkerThenTool))
                .await;
            self.attempt += 1;
            self.seq = 0;
            self.answer.clear();
        }
    }

    /// Drives one LLM round to its end, releasing answer sentences as its tokens arrive.
    async fn round(
        &mut self,
        tokens: &mut mpsc::UnboundedReceiver<String>,
        call: impl Future<Output = LlmResult<LlmResponse>>,
    ) -> LlmResult<LlmResponse> {
        tokio::pin!(call);
        let mut streamed = false;
        let response = loop {
            tokio::select! {
                biased;
                Some(token) = tokens.recv() => {
                    streamed = true;
                    self.push(&token).await;
                }
                response = &mut call => break response?,
            }
        };
        while let Ok(token) = tokens.try_recv() {
            streamed = true;
            self.push(&token).await;
        }
        if !streamed {
            // A client that does not stream: the reply arrives as one piece.
            self.push(&response.content).await;
        }
        let calls: Vec<&str> = (response.tool_calls.iter())
            .map(|call| call.function.name.as_str())
            .collect();
        debug!(
            run = self.run.0,
            content_chars = response.content.chars().count(),
            finish = response.finish_reason.as_deref(),
            ?calls,
            "interaction run round"
        );
        self.end(!response.tool_calls.is_empty()).await;
        Ok(response)
    }

    async fn release(&mut self, sentences: Vec<String>) {
        for text in sentences {
            let text = match &self.transforms {
                Some(transforms) => transform_for_speech(&text, transforms),
                None => text,
            };
            if text.trim().is_empty() {
                continue;
            }
            let verdict = if !self.guarded {
                GuardOutcome::Pass
            } else if guard::repeats_prompt(&text, &self.roles) {
                GuardOutcome::Blocked("prompt_echo")
            } else {
                guard::outbound(&text, &self.caller)
            };
            if let GuardOutcome::Blocked(rule) = verdict {
                // Never spoken, and not part of the answer the task reports.
                debug!(rule, "interaction sentence withheld by the speech policy");
                bridge::record_degraded("interaction", "speech_policy");
                continue;
            }
            self.answer.push(text.clone());
            if self.muted {
                continue;
            }
            let sentence = RunEvent::Sentence {
                attempt: self.attempt,
                seq: self.seq,
                text,
            };
            self.seq += 1;
            let _ = self.tx.send(Msg::Run(self.run, sentence)).await;
        }
    }
}

/// What the tool handlers of one run share.
pub(super) struct ToolCalls {
    run: RunId,
    tx: mpsc::Sender<Msg>,
    cancel: CancellationToken,
    secret: Option<String>,
    /// Results by operation id: a repeated operation returns the first result.
    results: Mutex<HashMap<OpId, FunctionResult>>,
    /// A confirmed call ended without a known outcome: the run stops after this round.
    pub(super) unconfirmed: AtomicBool,
    /// How each child task this run started ends, until it is heard.
    children: Mutex<Vec<oneshot::Receiver<Outcome>>>,
    /// The children this run started, by agent and request: the same call again gets the
    /// same child (live, gpt-oss-20b called the planner a second time with the same request
    /// in the round after the first call had returned).
    started: Mutex<HashMap<(String, String), FunctionResult>>,
    /// The controls this run applied, by their arguments: the control round and the run's own
    /// rounds asking for the same one apply it once (a second "rate slower" would step twice).
    controls: tokio::sync::Mutex<HashMap<String, FunctionResult>>,
    /// What a `conversation_control` must quote (`asked_by`) to be applied: the operator's
    /// prompt and the caller's words ([`named_in`]). Empty: nothing is applied.
    named_by: Vec<String>,
    /// Cancelled when the run's control round has ended (applied what it applies, or failed).
    round_done: CancellationToken,
}

/// How a tool request ended.
enum Posted {
    Done(Value),
    /// Nothing happened: the tool refused, or the request never left.
    Refused(String),
    /// It may have happened: a server error, a timeout or a broken connection.
    Unknown(String),
}

/// The registry of one run: its configured tools, an orchestrator's agents, and `ask_caller`.
fn tool_registry(
    tools: Vec<ToolSpec>,
    agents: Vec<AgentTool>,
    calls: Arc<ToolCalls>,
) -> FunctionRegistry {
    let registry = FunctionRegistry::new();
    let orchestrator = !agents.is_empty();
    // `drive`'s rule: an orchestrator of agents alone does not ask the caller.
    let asks = !tools.is_empty() || !orchestrator;
    let register = |definition: ToolDefinition, handler: FunctionHandler| {
        let item = RegistryItem::new(definition, handler).with_timeout(TOOL_CALL_BACKSTOP);
        registry.register_item(item);
    };
    for tool in tools {
        let definition =
            ToolDefinition::function(&tool.name, &tool.description, tool.parameters.clone());
        let (calls, tool) = (Arc::clone(&calls), Arc::new(tool));
        let handler: FunctionHandler = Arc::new(move |params: FunctionCallParams| {
            let (calls, tool) = (Arc::clone(&calls), Arc::clone(&tool));
            Box::pin(async move { calls.call(&tool, params.arguments).await })
        });
        register(definition, handler);
    }
    for agent in agents {
        let description = format!(
            "{} Starts a task on this agent and returns at once with its task_ref; the result \
             comes later as a task update.",
            agent.description
        );
        let parameters = agent.parameters.clone().unwrap_or_else(|| {
            json!({
                "type": "object",
                "properties": {
                    "request": {
                        "type": "string",
                        "description": "The task, in full: the agent sees nothing else of the call."
                    }
                },
                "required": ["request"]
            })
        });
        let definition = ToolDefinition::function(&agent.name, description.trim(), parameters);
        let (calls, agent) = (Arc::clone(&calls), Arc::new(agent));
        let handler: FunctionHandler = Arc::new(move |params: FunctionCallParams| {
            let (calls, agent) = (Arc::clone(&calls), Arc::clone(&agent));
            Box::pin(async move { calls.dispatch(&agent, params.arguments).await })
        });
        register(definition, handler);
    }
    if orchestrator {
        // Chapter 9: the orchestrator may steer the conversation itself (switch the language
        // because the caller wrote in Spanish), bounded by the operator.
        register_control(&registry, &calls, control_tool().1);
    }
    if asks {
        let (description, parameters) = ask_caller_tool();
        let definition = ToolDefinition::function(ASK_CALLER, description, parameters);
        let handler: FunctionHandler = Arc::new(move |params: FunctionCallParams| {
            let calls = Arc::clone(&calls);
            Box::pin(async move { calls.ask_caller(params.arguments).await })
        });
        register(definition, handler);
    }
    registry
}

/// `conversation_control` in `registry`, applied through `calls`; `parameters` are
/// [`control_tool`]'s, or [`control_round_parameters`] in a control round.
fn register_control(registry: &FunctionRegistry, calls: &Arc<ToolCalls>, parameters: Value) {
    let description = control_tool().0;
    let definition = ToolDefinition::function(CONVERSATION_CONTROL, description, parameters);
    let calls = Arc::clone(calls);
    let handler: FunctionHandler = Arc::new(move |params: FunctionCallParams| {
        let calls = Arc::clone(&calls);
        Box::pin(async move { calls.control(params.arguments).await })
    });
    registry.register_item(RegistryItem::new(definition, handler).with_timeout(TOOL_CALL_BACKSTOP));
}

/// The control round's frame around the caller's words; a quote that copies it with them
/// quotes the caller ([`named_in`]).
const CALLER_SAID: &str = "The caller said:";

/// What an orchestrator's control round asks about the caller's words (chapter 9).
const CONTROL_ROUND: &str = "Do not answer that or act on it in this message. Only check \
    whether your instructions or the caller ask, at this point of the call, to change how the \
    call itself goes (its language, speed, loudness or how brief the answers are). For each such \
    change, call conversation_control with asked_by set to the words of your instructions or of \
    the caller that ask for it, copied exactly. When nothing asks for one, reply NONE.";

/// [`control_tool`]'s parameters with `value` required too: a control round asks only about
/// changes that take one (rate, volume, verbosity, language).
fn control_round_parameters() -> Value {
    let mut parameters = control_tool().1;
    parameters["required"] = json!(["kind", "value", "asked_by"]);
    parameters
}

/// What `conversation_control` answers when its `asked_by` names nothing (r7-verify): nothing
/// is applied or reported.
const NOT_NAMED: &str = "Not applied: asked_by must be the words of your instructions or of the \
    caller that ask for this change, copied exactly, and these are not. Nothing was changed.";

/// What `conversation_control` answers for a kind it does not change ([`CALL_KINDS`]).
const NOT_THIS_TOOL: &str = "Not applied: this tool changes only the call's language, rate, \
    volume or verbosity. Nothing was changed.";

/// The most other words a quote may leave out between two of its words ([`named_in`]).
const QUOTE_GAP: usize = 3;

/// Whether `quote` (a control's `asked_by`) is words of one of `sources`: its words in their
/// order within one sentence of a source, case and punctuation aside, with at most
/// [`QUOTE_GAP`] words of the sentence left out between two of them. A control that quotes
/// nothing of the operator's prompt or the caller's words is one nobody asked for: it is never
/// applied.
///
/// Not word for word (r11-fix, 24 Oct): for CT3's "... call the tool conversation_control with
/// kind verbosity and value brief." gpt-oss-20b called verbosity brief in 10 of 10 control rounds
/// offline, quoting "verbosity brief" in 3 to 4 of them (more or fewer by the template's current
/// date), which a word-for-word match refused; one word of the sentence ("verbosity") was
/// accepted already. Words in order anywhere in a long sentence were too loose (offline it also
/// quoted "caller's request"), so the gap is bounded. The round's own frame ([`CALLER_SAID`])
/// before a quote is not part of it: the round quoted a caller's request with it in 1 to 3 of 10
/// (on other dates, and most quotes on a server shaped like the live one), and the frame alone
/// names nothing.
fn named_in(quote: &str, sources: &[&str]) -> bool {
    let words = |text: &str| -> Vec<String> {
        text.split(|c: char| !c.is_alphanumeric())
            .filter(|word| !word.is_empty())
            .map(str::to_lowercase)
            .collect()
    };
    // The (non-empty) quote's words in `said`, the first anywhere, each later one at most
    // QUOTE_GAP words after the one before. One pass per quote word over where the quote so
    // far can end (r11-verify: a search that backtracks takes 4^n steps for a quote of n
    // repeated words that fails at its last, e.g. a caller's "no no no ...").
    let follows = |quote: &[String], said: &[String]| {
        let mut ends: Vec<bool> = said.iter().map(|word| *word == quote[0]).collect();
        for word in &quote[1..] {
            ends = (0..said.len())
                .map(|at| {
                    said[at] == *word && ends[at.saturating_sub(QUOTE_GAP + 1)..at].contains(&true)
                })
                .collect();
        }
        ends.contains(&true)
    };
    let mut quote = words(quote);
    if quote.starts_with(&words(CALLER_SAID)) {
        quote.drain(..words(CALLER_SAID).len());
    }
    let mut sentences = sources
        .iter()
        .flat_map(|source| source.split(['.', '!', '?', ';', '\n']));
    !quote.is_empty() && sentences.any(|sentence| follows(&quote, &words(sentence)))
}

/// An orchestrator's control round, beside the first round of its run: the same model gets the
/// operator's prompt (not the persona preamble, the caller context or the answer rules), the
/// caller's words and [`CONTROL_ROUND`], with `conversation_control` as its only tool. A call
/// that changes the call ([`changes_the_call`]) and quotes who asked for it is applied as the
/// tool's (once when the run's own rounds ask for it too, [`ToolCalls::control`]); its text is
/// never spoken.
///
/// Why apart (r7-fix, 21 Oct, gpt-oss-20b at the session's low reasoning effort): in the full
/// request the voice contract's "answer directly" won over an operator's "call
/// conversation_control before you answer the first question" 0 of 10 times, with
/// conversation_control as the only tool too; asked apart, 10 of 10, and 0 of 10 for a plain
/// question or a booking without such an instruction. With `asked_by` (r7-verify, 22 Oct,
/// offline): CT3 10 of 10 twice, a caller's "speak slower" 10 of 10 three times, MA1 to MA7's
/// 13 requests 0 of 10 each; adding "a request for an answer or a task names no such change"
/// to the question took CT3 to 0 of 10.
#[allow(clippy::too_many_arguments)]
async fn control_round(
    llm: Arc<LlmClient>,
    key: String,
    prompt: String,
    words: String,
    state: Option<Value>,
    calls: Arc<ToolCalls>,
    api_key: Option<String>,
    cancel: CancellationToken,
) {
    // However the round ends (applied, failed, aborted with its run), the run's own rounds
    // stop waiting for it ([`ToolCalls::control`]).
    let _done = calls.round_done.clone().drop_guard();
    let registry = Arc::new(FunctionRegistry::new());
    register_control(&registry, &calls, control_round_parameters());
    let llm = LlmClient::clone(&llm).with_functions(Arc::clone(&registry));
    llm.set_context(&key, vec![ChatMessage::system(prompt)])
        .await;
    let asked = format!("{CALLER_SAID} \"{words}\"\n{CONTROL_ROUND}");
    let response = llm
        .complete(&key, &asked, api_key.as_deref(), &cancel, None)
        .await;
    llm.remove_history(&key).await;
    let response = match response {
        Ok(response) => response,
        Err(error) => {
            debug!(%error, "interaction control round failed");
            return;
        }
    };
    // Only what someone named: an unnamed call of the round would wait for the round itself.
    let changes = |call: &ToolCall| {
        let args = serde_json::from_str(&call.function.arguments).unwrap_or(Value::Null);
        let applies = call.function.name == CONVERSATION_CONTROL
            && changes_the_call(&args, state.as_ref())
            && calls.names(&args);
        debug!(call = %call.function.arguments, applies, "interaction control round call");
        applies
    };
    let batch: Vec<ToolCall> = (response.tool_calls.into_iter()).filter(changes).collect();
    if !batch.is_empty() {
        execute_batch(&registry, &key, &batch, &cancel, false, 0).await;
    }
}

/// Whether a control round's call changes how the call goes: its rate, volume, verbosity or
/// language (what [`CONTROL_ROUND`] asks about), to something it is not now. Live MA1 (r7-fix):
/// asked apart about "Book me the morning flight ...", gpt-oss-20b once called `rate normal` on
/// a call at its normal rate; and with the state already brief it asks for `verbosity brief`
/// again on every later turn. Neither is applied, so neither is reported.
fn changes_the_call(args: &Value, state: Option<&Value>) -> bool {
    let value = args["value"].as_str().unwrap_or_default();
    let now = |key: &str| state.map_or(&Value::Null, |state| &state[key]);
    match args["kind"].as_str().unwrap_or_default() {
        "rate" => !(value == "normal" && now("rate").as_f64() == Some(1.0)),
        "volume" => !(value == "normal" && now("volume_db").as_f64() == Some(0.0)),
        "verbosity" => now("verbosity").as_str() != Some(value),
        "language" => !(now("language").as_str()).is_some_and(|now| same_language(now, value)),
        _ => false,
    }
}

/// A task that stops when this is dropped (a run that ends early stops its control round).
struct Aborting(JoinHandle<()>);

impl Drop for Aborting {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl ToolCalls {
    pub(super) fn new(
        run: RunId,
        tx: &mpsc::Sender<Msg>,
        cancel: &CancellationToken,
        secret: Option<String>,
    ) -> Self {
        Self {
            run,
            tx: tx.clone(),
            cancel: cancel.clone(),
            secret,
            results: Mutex::default(),
            unconfirmed: AtomicBool::new(false),
            children: Mutex::default(),
            started: Mutex::default(),
            controls: tokio::sync::Mutex::default(),
            named_by: Vec::new(),
            round_done: CancellationToken::new(),
        }
    }

    /// The words a control must quote to be applied ([`Self::control`]).
    pub(super) fn named_by(mut self, sources: Vec<String>) -> Self {
        self.named_by = sources;
        self
    }

    /// Whether a control's `asked_by` quotes the operator's prompt or the caller's words.
    fn names(&self, args: &Value) -> bool {
        let sources: Vec<&str> = self.named_by.iter().map(String::as_str).collect();
        named_in(args["asked_by"].as_str().unwrap_or_default(), &sources)
    }

    /// An orchestrator's call of an agent tool: a child task on the agent (on its fallback
    /// while its breaker is open), and at once its ref. The child's end comes later
    /// ([`Self::next_child`]).
    async fn dispatch(&self, agent: &AgentTool, args: Value) -> FunctionResult {
        // A server's tool hears its arguments: after its name, they are the child's request.
        let request = match &agent.parameters {
            Some(_) if !args.is_object() => return Err("the arguments must be an object".into()),
            Some(_) => format!("{} {args}", agent.name),
            None => {
                let request = args.get("request").and_then(Value::as_str).map(str::trim);
                let Some(request) = request.filter(|request| !request.is_empty()) else {
                    return Err("request must say what the agent should do".to_owned());
                };
                request.to_owned()
            }
        };
        // An agent is never asked about a task (ORCHESTRATOR_RULE). Live MA1 (r5-decide):
        // gpt-oss-20b called the coder to "Check task t5 status" of the coder's own task t5,
        // the rule notwithstanding; that child's "I don't have information on task t5" was
        // then said to the caller. A call naming a task this run started goes nowhere.
        if let Some(task) = self.own_task_named(&request) {
            return Ok(json!({ "task_ref": task, "note": format!(
                "No agent was called: an agent is never asked about a task. The update of \
                 {task} reaches you by itself in a message that begins \"Task update\"."
            ) }));
        }
        // Chapter 3's gate, from the call's exact arguments, before anything is sent. The
        // same call again under one revision is not asked twice, nor started twice.
        let op = if agent.confirm {
            match self
                .gate(&agent.name, args.clone(), HitlKind::Confirm)
                .await
            {
                GateDecision::Allow(op) => Some(op),
                GateDecision::Refuse(reason) => return Err(refused(reason)),
                GateDecision::Answer(_) => return Err(refused("cancelled")),
            }
        } else {
            None
        };
        let done = op
            .as_ref()
            .and_then(|op| self.results.lock().get(op).cloned());
        if let Some(done) = done {
            return done;
        }
        let key = (agent.name.clone(), request);
        let earlier = self.started.lock().get(&key).cloned();
        let started = match earlier {
            Some(started) => started,
            None => self.start_child(agent, key.1.clone()).await,
        };
        if started.is_ok() {
            self.started.lock().insert(key, started.clone());
        }
        if let Some(op) = op {
            self.results.lock().insert(op, started.clone());
        }
        started
    }

    /// The ref of a task this run started that `request` asks about ("Check task t5 status").
    /// A ref without "task" or "status" beside it is the request's own word ("a cab to
    /// Heathrow T5").
    fn own_task_named(&self, request: &str) -> Option<String> {
        let words: Vec<String> = request
            .split(|c: char| !c.is_ascii_alphanumeric())
            .map(str::to_ascii_lowercase)
            .collect();
        if !words.iter().any(|word| word == "task" || word == "status") {
            return None;
        }
        let started = self.started.lock();
        let refs = started.values().filter_map(|started| {
            let task = started.as_ref().ok()?.get("task_ref")?.as_str()?;
            Some(task.to_ascii_lowercase())
        });
        refs.into_iter().find(|task| words.contains(task))
    }

    /// The child task of an agent call that may go out now.
    async fn start_child(&self, agent: &AgentTool, request: String) -> FunctionResult {
        let name = match &agent.fallback {
            Some(fallback) if super::remote::is_open(&agent.name) => fallback.clone(),
            _ => agent.name.clone(),
        };
        let (reply, answer) = oneshot::channel();
        let (done, end) = oneshot::channel();
        let dispatch = RunEvent::Dispatch {
            agent: name.clone(),
            request,
            reply,
            done,
        };
        self.report(dispatch).await;
        let answer = tokio::select! {
            biased;
            _ = self.cancel.cancelled() => return Err(refused("cancelled")),
            answer = answer => answer,
        };
        match answer {
            Ok(Ok(started)) => {
                self.children.lock().push(end);
                let state = if started.queued { "queued" } else { "working" };
                Ok(json!({ "task_ref": started.task_ref.0, "agent": name, "state": state }))
            }
            Ok(Err(reason)) => Err(format!("the task was not started: {reason}")),
            Err(_) => Err(refused("cancelled")),
        }
    }

    /// Some child of this run has not ended yet.
    fn waiting(&self) -> bool {
        !self.children.lock().is_empty()
    }

    /// How the next of this run's children ended; `None` when none is left to hear.
    async fn next_child(&self) -> LlmResult<Option<Outcome>> {
        loop {
            let pending = std::mem::take(&mut *self.children.lock());
            if pending.is_empty() {
                return Ok(None);
            }
            let (ended, _, rest) = tokio::select! {
                biased;
                _ = self.cancel.cancelled() => return Err(LlmError::Cancelled),
                ended = futures::future::select_all(pending) => ended,
            };
            *self.children.lock() = rest;
            // A child the registry dropped without an end (the session is gone) is not heard.
            if let Ok(outcome) = ended {
                return Ok(Some(outcome));
            }
        }
    }

    /// `conversation_control`: one control of the closed list, applied by the gateway; the
    /// result is its `control_applied` event (applied, partial, unsupported or refused).
    async fn control(&self, args: Value) -> FunctionResult {
        let kind = ControlKind::parse(args["kind"].as_str().unwrap_or_default());
        if !kind.is_some_and(|kind| CALL_KINDS.contains(&kind)) {
            return Err(NOT_THIS_TOOL.to_owned());
        }
        let asked = format!("{}:{}", args["kind"], args["value"]);
        if !self.names(&args) {
            // Who asked is the control round's to say (offline, the run's own round quoted
            // "user" for a caller's "speak slower"): this call gets what the round applied.
            tokio::select! {
                biased;
                _ = self.cancel.cancelled() => return Err(refused("cancelled")),
                _ = self.round_done.cancelled() => {}
            }
            let controls = self.controls.lock().await;
            return (controls.get(&asked).cloned()).unwrap_or_else(|| Err(NOT_NAMED.to_owned()));
        }
        let control = Control::from_json(&args, ControlSource::Agent)?;
        // Held while the control is applied: the other round's same control waits for it.
        let mut controls = self.controls.lock().await;
        if let Some(result) = controls.get(&asked) {
            return result.clone();
        }
        let (reply, applied) = oneshot::channel();
        self.report(RunEvent::Control { control, reply }).await;
        let result = tokio::select! {
            biased;
            _ = self.cancel.cancelled() => Err(refused("cancelled")),
            applied = applied => applied.map_err(|_| refused("cancelled")),
        };
        controls.insert(asked, result.clone());
        result
    }

    pub(super) async fn report(&self, event: RunEvent) {
        let _ = self.tx.send(Msg::Run(self.run, event)).await;
    }

    /// Hands a gate to the registry and waits for its decision; a cancelled run stops waiting.
    pub(super) async fn gate(&self, op_name: &str, args: Value, kind: HitlKind) -> GateDecision {
        let (reply, decision) = oneshot::channel();
        let gate = RunEvent::Gate {
            op_name: op_name.to_owned(),
            args,
            kind,
            reply,
        };
        self.report(gate).await;
        tokio::select! {
            biased;
            _ = self.cancel.cancelled() => GateDecision::Refuse("cancelled"),
            decision = decision => decision.unwrap_or(GateDecision::Refuse("cancelled")),
        }
    }

    /// One call of a configured tool. A side-effecting tool is gated first.
    pub(super) async fn call(&self, tool: &ToolSpec, args: Value) -> FunctionResult {
        let op = if tool.side_effects {
            match self.gate(&tool.name, args.clone(), HitlKind::Confirm).await {
                GateDecision::Allow(op) => Some(op),
                GateDecision::Refuse(reason) => return Err(refused(reason)),
                GateDecision::Answer(_) => return Err(refused("cancelled")),
            }
        } else {
            None
        };
        let done = op
            .as_ref()
            .and_then(|op| self.results.lock().get(op).cloned());
        if let Some(done) = done {
            return done;
        }
        let started = RunEvent::ToolStarted {
            name: tool.name.clone(),
            phrase: tool.progress_phrase.clone(),
            remote: false,
        };
        self.report(started).await;

        let request = self.post(tool, &args, op.as_ref());
        let posted = if op.is_some() {
            request.await
        } else {
            tokio::select! {
                biased;
                _ = self.cancel.cancelled() => return Err(refused("cancelled")),
                posted = request => posted,
            }
        };
        let name = &tool.name;
        let result = match posted {
            Posted::Done(value) => {
                if op.is_some() {
                    let label = value.get("effect").and_then(Value::as_str);
                    let label = label.map_or_else(|| effect_label(tool), str::to_owned);
                    self.report(RunEvent::Committed { label }).await;
                }
                Ok(value)
            }
            Posted::Unknown(error) if op.is_some() => {
                // It may have landed: never let the task claim that nothing was committed.
                let label = format!("tried to {} (unconfirmed)", effect_label(tool));
                self.report(RunEvent::Committed { label }).await;
                self.unconfirmed.store(true, Ordering::Relaxed);
                Err(format!(
                    "{name} did not confirm ({error}). It may or may not have been done; do not retry."
                ))
            }
            Posted::Refused(error) | Posted::Unknown(error) => {
                Err(format!("{name} failed: {error}"))
            }
        };
        if let Some(op) = op {
            self.results.lock().insert(op, result.clone());
        }
        result
    }

    /// The built-in tool: a question for the caller, answered through the same gate.
    pub(super) async fn ask_caller(&self, mut args: Value) -> FunctionResult {
        let kind = match args.get("kind").and_then(Value::as_str) {
            Some("choose") => HitlKind::Choose,
            Some("value") => HitlKind::Value,
            Some("form") => HitlKind::Form,
            _ => return Err("kind must be \"choose\", \"value\" or \"form\"".to_owned()),
        };
        let text = |value: &Value| value.as_str().is_some_and(|text| !text.trim().is_empty());
        let asks = args.get("question").is_some_and(text);
        let options = args.get("options").and_then(Value::as_array);
        let offers = options.is_some_and(|all| all.len() > 1 && all.iter().all(text));
        if !asks {
            return Err("question must be a non-empty string".to_owned());
        }
        if kind == HitlKind::Choose && !offers {
            return Err("choose needs at least two options".to_owned());
        }
        if kind == HitlKind::Form {
            // The fields the caller fills in on the screen, as the schema a client renders.
            args["schema"] = form_schema(&args["fields"])?;
        }
        match self.gate(ASK_CALLER, args, kind).await {
            GateDecision::Answer(answer) => Ok(json!({ "answer": answer })),
            GateDecision::Refuse(reason) => {
                debug!(run = self.run.0, reason, "interaction ask_caller refused");
                Err(refused(reason))
            }
            GateDecision::Allow(_) => Err(refused("cancelled")),
        }
    }

    async fn post(&self, tool: &ToolSpec, args: &Value, op: Option<&OpId>) -> Posted {
        let operation_id = op.map(|op| op.0.as_str());
        let body = json!({ "tool": tool.name, "arguments": args, "operation_id": operation_id })
            .to_string();
        // The signature says the operator's configuration made this call: a tool a session
        // declared itself (its URL, its confirmation rule) never gets one.
        let secret = self.secret.as_deref().filter(|_| tool.signed);
        let mut response = match post_signed(&tool.url, body, secret, operation_id).await {
            Ok(response) => response,
            Err(NotPosted::Unsent(reason)) => return Posted::Refused(reason),
            Err(NotPosted::Unknown(reason)) => return Posted::Unknown(reason),
        };
        let status = response.status();
        // The call has been made: a long or broken body only shortens what the model reads.
        let text = read_capped(&mut response, RESULT_MAX_BYTES).await;
        if status.is_success() {
            return Posted::Done(serde_json::from_str(&text).unwrap_or(Value::String(text)));
        }
        let shown: String = text.chars().take(ERROR_BODY_CHARS).collect();
        let error = format!("HTTP {status}: {shown}");
        if status.is_client_error() {
            Posted::Refused(error)
        } else {
            Posted::Unknown(error)
        }
    }
}

/// The built-in tool as a model sees it: its description and the schema of its arguments.
pub(super) fn ask_caller_tool() -> (&'static str, Value) {
    let parameters = json!({
        "type": "object",
        "properties": {
            "kind": {
                "type": "string",
                "enum": ["choose", "value", "form"],
                "description": "choose: the caller picks one of `options`; value: the caller says a value; form: the caller fills in several `fields` on their screen"
            },
            "question": { "type": "string" },
            "options": { "type": "array", "items": { "type": "string" } },
            "fields": {
                "type": "array",
                "items": { "type": "string" },
                "description": "form only: the name of each detail to fill in, such as street or city"
            }
        },
        "required": ["kind", "question"]
    });
    let description = "Ask the caller a question and wait for the answer. Use it when you need \
                       the caller to pick one of a few options, to give a missing detail, or to \
                       fill in several details at once on their screen (a form).";
    (description, parameters)
}

/// The orchestrator's tool of chapter 9.
pub(super) const CONVERSATION_CONTROL: &str = "conversation_control";

/// What an orchestrator's `conversation_control` applies: how the call itself goes, as its
/// description says. Live MA7 (r7-verify, 22 Oct): gpt-oss-20b's own round called `updates`
/// `ask` and `tell` after starting the planner, which no one had asked for.
const CALL_KINDS: [ControlKind; 4] = [
    ControlKind::Rate,
    ControlKind::Volume,
    ControlKind::Verbosity,
    ControlKind::Language,
];

/// `conversation_control`: the closed list of controls and their values.
pub(super) fn control_tool() -> (&'static str, Value) {
    // Every kind is offered, though only [`CALL_KINDS`] are applied: offered those four alone,
    // CT3's instruction was followed in 0 of 10 control rounds (10 of 10 and 7 of 10 with all,
    // r7-verify, 22 and 23 Oct).
    let kinds: Vec<&str> = ControlKind::ALL.iter().map(|kind| kind.as_str()).collect();
    let parameters = json!({
        "type": "object",
        "properties": {
            "kind": { "type": "string", "enum": kinds },
            "value": {
                "type": "string",
                "description": "rate: slower, faster or normal; volume: louder, quieter or normal; verbosity: brief, normal or detailed; language: a BCP-47 code such as es; repeat: last, answer or a slot (number, time, name, address, code); backchannels: on or off; updates: ask, tell or screen; hold: seconds"
            },
            "asked_by": {
                "type": "string",
                "description": "The words of your instructions or of the caller that name this change, copied exactly."
            }
        },
        "required": ["kind", "asked_by"]
    });
    // Live CT3 (gpt-oss-20b): with the first wording the operator's "call conversation_control
    // before the first answer" was followed in 3 of 5 and 8 of 10 probe rounds, 0 of 1 live; with
    // this one and the rule's sentence, 10 of 10.
    let description = "Change how the conversation itself goes: speak another language, speak \
                       slower or faster, keep the replies brief or detailed. Call it whenever \
                       your instructions or the caller ask for such a change; saying it changes \
                       nothing. The gateway applies it within what the operator allows and the \
                       line can do, and answers with the outcome: applied, partial, unsupported \
                       or refused.";
    (description, parameters)
}

/// The most fields a form of `ask_caller` has, and the longest name of one.
const FORM_FIELDS_MAX: usize = 8;
const FORM_FIELD_CHARS: usize = 40;

/// The JSON Schema of an `ask_caller` form: one string property per named field, all required.
fn form_schema(fields: &Value) -> Result<Value, String> {
    let names: Vec<&str> = fields
        .as_array()
        .map(|all| {
            all.iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .collect()
        })
        .unwrap_or_default();
    let usable = |name: &&str| !name.is_empty() && name.chars().count() <= FORM_FIELD_CHARS;
    let distinct = names.iter().collect::<HashSet<_>>().len() == names.len();
    let all_given = fields.as_array().map(Vec::len) == Some(names.len());
    if names.is_empty()
        || names.len() > FORM_FIELDS_MAX
        || !all_given
        || !distinct
        || !names.iter().all(usable)
    {
        return Err(format!(
            "a form needs `fields`: 1 to {FORM_FIELDS_MAX} different names of at most \
             {FORM_FIELD_CHARS} characters"
        ));
    }
    let properties: serde_json::Map<String, Value> = names
        .iter()
        .map(|name| {
            (
                (*name).to_owned(),
                json!({ "type": "string", "title": name }),
            )
        })
        .collect();
    Ok(json!({ "type": "object", "properties": properties, "required": names }))
}

/// The first `max` bytes of a response body, as text.
pub(super) async fn read_capped(response: &mut reqwest::Response, max: usize) -> String {
    let mut read = Vec::new();
    while read.len() < max
        && let Ok(Some(chunk)) = response.chunk().await
    {
        read.extend_from_slice(&chunk);
    }
    read.truncate(max);
    String::from_utf8_lossy(&read).into_owned()
}

/// What the background is asked after a call with a known caller (caller memory).
const FACTS_PROMPT: &str = "You keep short notes about a returning caller of a phone \
    assistant, so that a later call can serve them better. From the conversation below, write \
    at most three lasting facts about the caller: preferences, needs, how they like to be \
    served. Write only what the caller said about themselves. Each fact on a line of its own, \
    at most 80 characters, with no numbers, names, codes, addresses, amounts or links, and none \
    that repeats a known fact. If nothing is worth keeping, write only NONE.";
/// How long that question may take; the call is over, nobody waits for it.
const FACTS_TIMEOUT: Duration = Duration::from_secs(30);
/// The most facts one call adds.
const FACTS_PER_CALL: usize = 3;

/// What the background model, asked after the call, would remember about the caller: lines
/// that pass the memory's rules (`tenant::check_fact`), at most three. `known`: what is kept
/// already; `conversation`: the call as the caller heard it.
pub async fn caller_facts(
    llm: &LlmClient,
    api_key: Option<&str>,
    known: &[String],
    conversation: &[ChatMessage],
    key: &str,
) -> Vec<String> {
    let line = |message: &ChatMessage| {
        let who = match message.role {
            MessageRole::User => "Caller",
            MessageRole::Assistant => "Agent",
            _ => return None,
        };
        let text = message
            .content
            .as_deref()
            .filter(|text| !text.trim().is_empty())?;
        Some(format!("{who}: {text}"))
    };
    let lines: Vec<String> = conversation.iter().filter_map(line).collect();
    let known: Vec<String> = known.iter().map(|fact| format!("- {fact}")).collect();
    let known = if known.is_empty() {
        "(none)".to_owned()
    } else {
        known.join("\n")
    };
    let request = format!(
        "Known facts:\n{known}\n\nConversation:\n{}",
        lines.join("\n")
    );
    llm.set_context(key, vec![ChatMessage::system(FACTS_PROMPT)])
        .await;
    let cancel = CancellationToken::new();
    let asked = llm.complete(key, &request, api_key, &cancel, None);
    let answer = tokio::time::timeout(FACTS_TIMEOUT, asked).await;
    llm.remove_history(key).await;
    match answer {
        Ok(Ok(response)) => fact_lines(&response.content),
        Ok(Err(error)) => {
            debug!(%error, "the background wrote no caller facts");
            Vec::new()
        }
        Err(_) => {
            debug!("the background wrote no caller facts in time");
            Vec::new()
        }
    }
}

/// The facts of the background's answer: one per line, list marks and a thinking block
/// dropped, those the memory may not keep left out. A line with a colon is a label or a copy
/// of the prompt ("Known facts: ..."), never a fact.
fn fact_lines(answer: &str) -> Vec<String> {
    let answer = answer.rsplit("</think>").next().unwrap_or_default();
    let fact = |line: &str| {
        let line = line.trim().trim_start_matches(['-', '*', '•']);
        let line = line.trim_start_matches(|c: char| c.is_ascii_digit());
        let line = line
            .trim_start_matches(['.', ')'])
            .trim()
            .trim_end_matches('.');
        let keep = !line.eq_ignore_ascii_case("none")
            && !line.contains(':')
            && super::tenant::check_fact(line).is_ok();
        keep.then(|| line.to_owned())
    };
    let mut facts: Vec<String> = answer.lines().filter_map(fact).collect();
    facts.truncate(FACTS_PER_CALL);
    facts
}

/// What a committed call of this tool is called when its response names no `effect`.
fn effect_label(tool: &ToolSpec) -> String {
    tool.confirm_phrase
        .clone()
        .unwrap_or_else(|| tool.name.replace('_', " "))
}

// ---- pure helpers

/// The user message of a run: the caller's request, then each correction on a line of its own
/// that says it is one. Nothing more is said about it: on the 3B model this was measured on,
/// the label alone gave one call with the corrected value in 10 of 10 runs, and every added
/// instruction ("the earlier value must not be used") made it call again with the old one.
pub(super) fn request_text(words: &[String]) -> String {
    let mut lines = words.iter();
    let mut text = lines.next().cloned().unwrap_or_default();
    for change in lines {
        text.push_str("\nCorrection: ");
        text.push_str(change);
    }
    text
}

/// Appended to an orchestrator's system prompt: its agents start tasks and do not wait.
pub(super) const ORCHESTRATOR_RULE: &str = "\n\nSome tools are agents. Calling an agent starts \
    a task on it and returns at once with the task's task_ref; the agent works on while you talk \
    with the caller, and its result reaches you later in a message that begins \"Task update\". \
    Never wait for it or guess its result, and never call an agent to check on a task: its \
    update reaches you by itself. Give an agent the details the caller gave, and do not ask the \
    caller for them again. Give each part of a request to the agent that fits it, \
    in one round when the parts do not depend on each other, and answer general questions \
    yourself. After starting tasks, say in one short sentence after ANSWER: that they are under \
    way. After a task update, give the caller its result in one or two short sentences after \
    ANSWER:, or start the next step. When your instructions or the caller ask to change how the \
    call itself goes (its language, speed, loudness or how brief the answers are), call \
    conversation_control first: writing that you will does not change it.";

/// The message an orchestrator gets when a child task ended.
pub(super) fn child_update(outcome: &Outcome) -> String {
    let task = &outcome.task_ref.0;
    let agent = outcome.lineage.agent.as_deref().unwrap_or("the agent");
    let what = match outcome.state {
        TaskState::Completed => format!("is done. Its result: {}", outcome.summary.trim()),
        TaskState::Cancelled => "was stopped.".to_owned(),
        _ => format!(
            "failed ({}). The caller has already been told that it failed.",
            outcome.summary.trim()
        ),
    };
    format!("Task update (not from the caller): task {task} on {agent} {what}")
}

/// The line a model with tools writes before its final answer.
const ANSWER_MARKER: &str = "ANSWER:";
/// Appended to the system prompt when tools are configured: how to ask the caller (the tool
/// exists only then), and the answer marker. While a round streams the gateway cannot see
/// whether it will end in a tool call, so only text after the marker is spoken early.
pub(super) const ANSWER_RULE: &str = "\n\nTo ask the caller something, call the ask_caller tool with the \
    question and, for a choice, the options. Do not ask in your own words, and do not confirm an \
    action yourself: the system reads it back and asks.\n\nWhen you have everything you need, \
    begin your final answer with a line that says only ANSWER: and write the answer after it. \
    Text before that line is never spoken to the caller. Never call a tool after you have written \
    ANSWER:.";
/// [`ANSWER_RULE`] without its `ask_caller` paragraph: for an orchestrator of agents alone, which
/// has no `ask_caller` (live PR10: the 3B background asked the caller the flight's date in 7 of 8
/// rounds instead of calling the travel agent; without the tool it called the agent in 8 of 8).
pub(super) const MARKER_RULE: &str = "\n\nWhen you have everything you need, begin your final \
    answer with a line that says only ANSWER: and write the answer after it. Text before that \
    line is never spoken to the caller. Never call a tool after you have written ANSWER:.";

/// The answer gate: which streamed text of a round is the answer, cut into speakable sentences
/// (think blocks dropped, the conversation path's sentence aggregator, markdown stripped).
pub struct AnswerGate {
    /// A round may end in a tool call, so its text is held until the marker.
    gated: bool,
    /// The line the answer begins with. Without one a held round is released at its end.
    marker: Option<String>,
    think: ThinkStripper,
    /// The round's text so far while the marker has not come; `None` when text streams.
    draft: Option<String>,
    sentences: SentenceAggregator,
}

/// What the end of a round releases.
#[derive(Debug, PartialEq)]
pub struct RoundEnd {
    pub sentences: Vec<String>,
    /// The round streamed answer text and then called a tool.
    pub marker_then_tool: bool,
}

impl AnswerGate {
    /// `gated`: hold a round's text until the answer marker. Without tools every round is the
    /// answer and sentences are released from the first token.
    pub fn new(gated: bool) -> Self {
        Self::with(gated, gated.then(|| ANSWER_MARKER.to_owned()))
    }

    /// A gate for a backend that runs its own tools (a Bud agent): text is held until
    /// `marker`, the line the agent is declared to write before its final answer. With no
    /// marker everything is held until the end, the first point where a draft and the answer
    /// differ.
    pub fn held(marker: Option<String>) -> Self {
        Self::with(true, marker)
    }

    fn with(gated: bool, marker: Option<String>) -> Self {
        Self {
            gated,
            marker,
            think: ThinkStripper::default(),
            draft: gated.then(String::new),
            sentences: SentenceAggregator::default(),
        }
    }

    /// One streamed token: the answer sentences it completes.
    pub fn push(&mut self, token: &str) -> Vec<String> {
        let text = self.think.push(token);
        self.release(text)
    }

    fn release(&mut self, text: String) -> Vec<String> {
        let answer = match &mut self.draft {
            None => text,
            Some(draft) => {
                draft.push_str(&text);
                let marker = self.marker.as_deref();
                let after = |marker: &str| draft.find(marker).map(|at| at + marker.len());
                let Some(answer_at) = marker.and_then(after) else {
                    return Vec::new();
                };
                let answer = draft.split_off(answer_at);
                self.draft = None;
                answer
            }
        };
        spoken(self.sentences.push_str(&answer))
    }

    /// The end of a round. Without a tool call the rest of the answer is released (the whole
    /// round when the marker never came); with one, what the round wrote was a draft.
    pub fn end_round(&mut self, tool_call: bool) -> RoundEnd {
        let tail = self.think.flush();
        let mut sentences = self.release(tail);
        let marker_then_tool = tool_call && self.gated && self.draft.is_none();
        if tool_call {
            sentences.clear();
            self.sentences.reset();
        } else {
            if let Some(draft) = self.draft.take() {
                sentences.extend(spoken(self.sentences.push_str(&draft)));
            }
            sentences.extend(spoken(self.sentences.flush()));
        }
        self.think = ThinkStripper::default();
        self.draft = self.gated.then(String::new);
        RoundEnd {
            sentences,
            marker_then_tool,
        }
    }
}

/// Sentences as they are spoken: markdown and a leading speaker label stripped, empty ones
/// dropped.
fn spoken(sentences: impl IntoIterator<Item = String>) -> Vec<String> {
    let stripped = sentences.into_iter().map(|sentence| {
        let plain = strip_markdown_for_tts(&sentence);
        let said = guard::strip_speaker_label(&plain);
        if said.len() < plain.len() {
            debug!(sentence = %plain, "interaction speaker label stripped");
        }
        said.to_owned()
    });
    stripped
        .filter(|sentence| !sentence.trim().is_empty())
        .collect()
}

/// What the model is told when a gate does not let a call through.
pub(super) fn refused(reason: &str) -> String {
    let told = match reason {
        "declined" => "The caller declined. Do not do it and do not ask again.",
        "timeout" => "The caller did not answer. Nothing was done.",
        "stale" => "The caller changed the request. Nothing was done.",
        "not_by_voice" => {
            "This value is never taken by voice on this call. Do not ask for it again; say \
             that it has to be given another way."
        }
        _ => "The request was cancelled. Nothing was done.",
    };
    told.to_owned()
}

/// Why [`post_signed`] has no response.
pub(super) enum NotPosted {
    /// Nothing was sent: the address is refused, or nobody answers there.
    Unsent(String),
    /// The request went out and what became of it is not known.
    Unknown(String),
}

/// One signed JSON request of the gateway's to an operator's address: a tool call, a webhook
/// backend's message, or the push of a task's result. With `secret` the request carries
/// `X-WaaV-Signature`; `idempotency` is sent as `Idempotency-Key`.
pub(super) async fn post_signed(
    url: &str,
    body: String,
    secret: Option<&str>,
    idempotency: Option<&str>,
) -> Result<reqwest::Response, NotPosted> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH);
    let timestamp = now.map_or(0, |since_epoch| since_epoch.as_secs());
    let mut headers = vec![("X-WaaV-Timestamp", timestamp.to_string())];
    if let Some(secret) = secret {
        headers.push(("X-WaaV-Signature", signature(secret, timestamp, &body)));
    }
    if let Some(idempotency) = idempotency {
        headers.push(("Idempotency-Key", idempotency.to_owned()));
    }
    post_json(url, body, &headers).await
}

/// One JSON request of the gateway's to an address from the operator's configuration. The
/// host is resolved and checked first, off the async workers (the lookup blocks; a session's
/// parser checks only the scheme).
pub(super) async fn post_json(
    url: &str,
    body: String,
    headers: &[(&str, String)],
) -> Result<reqwest::Response, NotPosted> {
    send_json(reqwest::Method::POST, url, Some(body), headers, false).await
}

/// [`post_json`] with any method; a `GET` has no body. `stream`: the response is read as it
/// is written (server-sent events), so only a silence of [`TOOL_HTTP_TIMEOUT`] ends it, not
/// its length.
pub(super) async fn send_json(
    method: reqwest::Method,
    url: &str,
    body: Option<String>,
    headers: &[(&str, String)],
    stream: bool,
) -> Result<reqwest::Response, NotPosted> {
    let target = url.to_owned();
    let checked =
        tokio::task::spawn_blocking(move || validate_url_for_ssrf(&target, HTTP_URL_SCHEMES));
    match checked.await {
        Ok(Ok(())) => {}
        Ok(Err(reason)) => return Err(NotPosted::Unsent(format!("URL refused: {reason}"))),
        Err(error) => return Err(NotPosted::Unsent(error.to_string())),
    }
    let client = if stream {
        shared_http_client("interaction-streams", || {
            ssrf_protected_client_builder(HTTP_URL_SCHEMES)
                .connect_timeout(TOOL_HTTP_TIMEOUT)
                .read_timeout(TOOL_HTTP_TIMEOUT)
                .http1_only()
                .build()
        })
    } else {
        shared_http_client("interaction-tools", || {
            ssrf_protected_client_builder(HTTP_URL_SCHEMES)
                .timeout(TOOL_HTTP_TIMEOUT)
                .http1_only()
                .build()
        })
    }
    .map_err(|error| NotPosted::Unsent(error.to_string()))?;

    let mut request = client.request(method, url);
    for (name, value) in headers {
        request = request.header(*name, value);
    }
    if let Some(traceparent) = traceparent() {
        request = request.header("traceparent", traceparent);
    }
    if let Some(body) = body {
        // JSON unless the caller names the body's type (an OAuth token request is a form).
        let typed = headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("content-type"));
        if !typed {
            request = request.header("Content-Type", "application/json");
        }
        request = request.body(body);
    }
    request.send().await.map_err(|error| {
        let unsent = error.is_connect();
        let reason = error.without_url().to_string();
        if unsent {
            NotPosted::Unsent(reason)
        } else {
            NotPosted::Unknown(reason)
        }
    })
}

/// The `X-WaaV-Signature` value of a tool call: `sha256=` and the hex HMAC-SHA256 of
/// `"<timestamp>.<body>"` under the server's tool secret.
pub(super) fn signature(secret: &str, timestamp: u64, body: &str) -> String {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(secret.as_bytes())
        .unwrap_or_else(|_| unreachable!("HMAC takes a key of any length"));
    mac.update(format!("{timestamp}.{body}").as_bytes());
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_after_a_language_switch_is_told_the_language_last() {
        let state = |language: &str| serde_json::json!({ "language": language, "version": 2 });
        assert_eq!(reply_in(Some(&state("es"))), Some("Spanish".to_owned()));
        assert_eq!(
            reply_in(Some(&state("en-US"))),
            None,
            "the pack's own language"
        );
        assert_eq!(reply_in(None), None);
    }

    /// The text deltas of an OpenAI-style SSE body, in order.
    fn deltas(sse: &str) -> Vec<String> {
        sse.lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .filter_map(|data| serde_json::from_str::<serde_json::Value>(data).ok())
            .filter_map(|chunk| {
                chunk["choices"][0]["delta"]["content"]
                    .as_str()
                    .map(str::to_owned)
            })
            .collect()
    }

    fn sse(tokens: &[&str]) -> String {
        let mut body = String::new();
        for token in tokens {
            let chunk = serde_json::json!({"choices": [{"index": 0, "delta": {"content": token}}]});
            body.push_str(&format!("data: {chunk}\n\n"));
        }
        body.push_str("data: [DONE]\n\n");
        body
    }

    /// Feeds the stream through the gate the way the token callback does.
    fn stream(gate: &mut AnswerGate, tokens: &[&str]) -> Vec<String> {
        let mut released = Vec::new();
        for token in deltas(&sse(tokens)) {
            released.extend(gate.push(&token));
        }
        released
    }

    fn call(arguments: &str) -> ToolCall {
        ToolCall {
            id: format!("call_{}", arguments.len()),
            call_type: "function".to_owned(),
            function: crate::core::llm::FunctionCall {
                name: "book_table".to_owned(),
                arguments: arguments.to_owned(),
            },
        }
    }

    #[test]
    fn an_agent_is_not_called_about_a_task_the_run_started() {
        let (tx, _rx) = mpsc::channel(1);
        let calls = ToolCalls::new(RunId(7), &tx, &CancellationToken::new(), None);
        let started = Ok(json!({ "task_ref": "t5", "agent": "coder", "state": "working" }));
        let key = ("coder".to_owned(), "Fix the failing test".to_owned());
        calls.started.lock().insert(key, started);
        // Live MA1: the 20B asked the coder about its own child.
        assert_eq!(
            calls.own_task_named("Check task t5 status"),
            Some("t5".to_owned())
        );
        assert_eq!(
            calls.own_task_named("What is T5's status?"),
            Some("t5".to_owned())
        );
        // Another task's ref, or none, is an ordinary request.
        assert_eq!(calls.own_task_named("Check task t6 status"), None);
        assert_eq!(calls.own_task_named("Book a table at 5 pm"), None);
        assert_eq!(calls.own_task_named("Fix the test in t5x.rs"), None);
        assert_eq!(calls.own_task_named("Get a cab to Heathrow T5"), None);
    }

    #[tokio::test]
    async fn a_tool_call_cut_off_by_the_token_limit_is_not_made_and_not_replayed() {
        let (made, cut) = (r#"{"party_size": 2}"#, r#"{"restaurant": "Olive Garden"#);
        assert!(whole(&call(made)) && whole(&call("")) && !whole(&call(cut)));

        let llm = LlmClient::new(crate::core::llm::LlmClientConfig::default());
        let mut round = ChatMessage::assistant("");
        round.tool_calls = Some(vec![call(made), call(cut)]);
        let history = vec![
            ChatMessage::system("You are the booking desk."),
            ChatMessage::user("A table for two, and flowers."),
            round,
        ];
        llm.set_context("run", history).await;
        let mut response = LlmResponse {
            id: String::new(),
            model: String::new(),
            content: String::new(),
            finish_reason: Some("length".to_owned()),
            tool_calls: vec![call(made), call(cut)],
            usage: None,
        };
        drop_cut_calls(&llm, "run", &mut response).await;
        let arguments = |calls: &[ToolCall]| -> Vec<String> {
            let arguments = |call: &ToolCall| call.function.arguments.clone();
            calls.iter().map(arguments).collect()
        };
        assert_eq!(arguments(&response.tool_calls), [made]);
        let history = llm.history_snapshot("run").await;
        assert_eq!(history.len(), 3);
        assert_eq!(arguments(history[2].tool_calls.as_deref().unwrap()), [made]);

        // With nothing whole left the round has no call at all.
        response.tool_calls = vec![call(cut)];
        let mut round = ChatMessage::assistant("");
        round.tool_calls = Some(vec![call(cut)]);
        llm.set_context("run", vec![ChatMessage::user("Flowers."), round])
            .await;
        drop_cut_calls(&llm, "run", &mut response).await;
        assert!(response.tool_calls.is_empty());
        assert!(llm.history_snapshot("run").await[1].tool_calls.is_none());
    }

    #[test]
    fn an_agent_without_a_marker_is_held_until_its_run_ends() {
        let mut gate = AnswerGate::held(None);
        // What it wrote before a tool item was a draft: never released, and no breach.
        assert!(stream(&mut gate, &["Let me check ", "that for you. "]).is_empty());
        let end = gate.end_round(true);
        assert_eq!((end.sentences.len(), end.marker_then_tool), (0, false));
        // The text after the last tool item is the answer, whole, when the run completes.
        let held = stream(
            &mut gate,
            &["Your order shipped ", "on Monday. It arrives Friday."],
        );
        assert!(held.is_empty(), "{held:?}");
        assert_eq!(
            gate.end_round(false).sentences,
            ["Your order shipped on Monday.", "It arrives Friday."]
        );
    }

    #[test]
    fn an_agents_declared_marker_streams_its_answer() {
        let mut gate = AnswerGate::held(Some("FINAL:".to_owned()));
        // The LLM tier's own marker means nothing to it.
        assert!(stream(&mut gate, &["ANSWER: not yet. ", "FIN"]).is_empty());
        assert_eq!(
            stream(&mut gate, &["AL: It shipped on Monday. ", "It arrives"]),
            ["It shipped on Monday."]
        );
        // A tool item after the marker is the breach the registry stops the answer for.
        let end = gate.end_round(true);
        assert_eq!((end.sentences.len(), end.marker_then_tool), (0, true));
        // An agent that leaves the marker out answers at the end of its run.
        assert!(stream(&mut gate, &["It arrives Friday."]).is_empty());
        assert_eq!(gate.end_round(false).sentences, ["It arrives Friday."]);
    }

    #[test]
    fn the_speech_policy_holds_whatever_the_sentence_cutter_makes_of_a_link() {
        let mut gate = AnswerGate::new(false);
        let answer = "You can pay at https://pay.example.org/abc today. Or call 1 800 555 0100 \
                      now. HEARD: okay. Your bag is free.";
        let mut sentences = gate.push(answer);
        sentences.extend(gate.end_round(false).sentences);
        let kept: Vec<&String> = sentences
            .iter()
            .filter(|sentence| guard::outbound(sentence, "") == GuardOutcome::Pass)
            .collect();
        assert_eq!(kept, ["Your bag is free."], "of {sentences:?}");
    }

    #[test]
    fn without_tools_sentences_are_released_from_the_first_token() {
        let mut gate = AnswerGate::new(false);
        let tokens = [
            "Your", " bag", " is", " free", ".", " It", " can", " weigh", " 23", " kilos.",
        ];
        // The first sentence leaves as soon as the next one starts, long before the round ends.
        assert_eq!(stream(&mut gate, &tokens), ["Your bag is free."]);
        assert_eq!(
            gate.end_round(false),
            RoundEnd {
                sentences: vec!["It can weigh 23 kilos.".to_owned()],
                marker_then_tool: false,
            }
        );
    }

    #[test]
    fn with_tools_only_text_after_the_marker_streams() {
        let mut gate = AnswerGate::new(true);
        let tokens = [
            "Let me",
            " think.",
            " The fare is fine.",
            " ANS",
            "WER",
            ":",
            "\nI found",
            " two",
            " flights.",
            " The",
            " first",
            " leaves at nine.",
        ];
        assert_eq!(stream(&mut gate, &tokens), ["I found two flights."]);
        assert_eq!(
            gate.end_round(false),
            RoundEnd {
                sentences: vec!["The first leaves at nine.".to_owned()],
                marker_then_tool: false,
            }
        );
    }

    #[test]
    fn a_round_without_a_marker_is_released_whole_at_its_end() {
        let mut gate = AnswerGate::new(true);
        let tokens = ["I found", " two flights.", " Both", " are direct."];
        assert!(stream(&mut gate, &tokens).is_empty());
        assert_eq!(
            gate.end_round(false),
            RoundEnd {
                sentences: vec![
                    "I found two flights.".to_owned(),
                    "Both are direct.".to_owned()
                ],
                marker_then_tool: false,
            }
        );
    }

    #[test]
    fn a_draft_before_a_tool_call_is_never_released() {
        let mut gate = AnswerGate::new(true);
        assert!(stream(&mut gate, &["I will", " look that up.", " One moment."]).is_empty());
        assert_eq!(
            gate.end_round(true),
            RoundEnd {
                sentences: Vec::new(),
                marker_then_tool: false,
            }
        );
        // The next round starts clean: nothing of the draft leaks into the answer.
        assert_eq!(
            stream(&mut gate, &["ANSWER: It is", " sunny.", " Enjoy."]),
            ["It is sunny."]
        );
        assert_eq!(gate.end_round(false).sentences, ["Enjoy."]);
    }

    #[test]
    fn a_tool_call_after_the_marker_is_reported() {
        let mut gate = AnswerGate::new(true);
        let tokens = ["ANSWER:", " You are", " booked.", " Let me", " also check"];
        assert_eq!(stream(&mut gate, &tokens), ["You are booked."]);
        assert_eq!(
            gate.end_round(true),
            RoundEnd {
                sentences: Vec::new(),
                marker_then_tool: true,
            }
        );
        // The half sentence that was cut does not start the next round's answer.
        assert_eq!(stream(&mut gate, &["ANSWER: Done.", " Bye."]), ["Done."]);
    }

    #[test]
    fn think_blocks_and_markdown_never_reach_speech() {
        let mut gate = AnswerGate::new(true);
        let tokens = [
            "<thi",
            "nk>ANSWER: not this.",
            " Nor this.</think>",
            "ANSWER:",
            " **Two** flights",
            " match.",
            " Done.",
        ];
        assert_eq!(stream(&mut gate, &tokens), ["Two flights match."]);
        assert_eq!(gate.end_round(false).sentences, ["Done."]);

        let mut gate = AnswerGate::new(false);
        assert!(stream(&mut gate, &["<think>quietly</think>", ""]).is_empty());
        assert!(gate.end_round(false).sentences.is_empty());
    }

    /// `openssl dgst -sha256 -hmac Jefe` over `1760000000.{"tool":"book_table"}`.
    const SIGNATURE: &str =
        "sha256=800f7cc03566fdb17e5aef77bad4f584c267f5c7733eb0adf2c51f55690f691d";

    #[test]
    fn a_child_end_reaches_its_orchestrator_as_a_task_update() {
        use super::super::types::{Effects, Lineage, TaskRef};
        let outcome = |state, summary: &str| Outcome {
            update: None,
            task_id: TaskId("task_c".to_owned()),
            task_ref: TaskRef("t2".to_owned()),
            revision: 1,
            seq: 3,
            state,
            summary: summary.to_owned(),
            effects: Effects::NoneCommitted,
            lineage: Lineage {
                agent: Some("browser".to_owned()),
                ..Lineage::default()
            },
        };
        assert_eq!(
            child_update(&outcome(TaskState::Completed, "Booked AI 101.")),
            "Task update (not from the caller): task t2 on browser is done. Its result: Booked AI 101."
        );
        assert_eq!(
            child_update(&outcome(TaskState::Failed, "HTTP 503")),
            "Task update (not from the caller): task t2 on browser failed (HTTP 503). The caller has \
             already been told that it failed."
        );
        assert!(child_update(&outcome(TaskState::Cancelled, "")).ends_with("was stopped."));
        assert!(
            ORCHESTRATOR_RULE.contains("returns at once") && ORCHESTRATOR_RULE.contains("ANSWER:")
        );
        // Live MA1: an agent called to check on another's task, the caller asked again.
        assert!(
            ORCHESTRATOR_RULE.contains("never call an agent to check on a task")
                && ORCHESTRATOR_RULE.contains("do not ask the caller for them again")
        );
    }

    #[tokio::test]
    async fn a_request_made_for_a_task_carries_its_trace() {
        let trace = "4bf92f3577b34da6a3ce929d0e0e4736".to_owned();
        let header = traced(trace.clone(), async { traceparent() }).await;
        let header = header.expect("a traceparent");
        let parts: Vec<&str> = header.split('-').collect();
        assert_eq!((parts[0], parts[1], parts[3]), ("00", trace.as_str(), "01"));
        assert_eq!(parts[2].len(), 16);
        // Outside a task's run there is none.
        assert_eq!(traceparent(), None);
        assert_eq!(traced(String::new(), async { traceparent() }).await, None);
    }

    #[test]
    fn the_signature_is_the_hex_hmac_of_timestamp_dot_body() {
        let body = r#"{"tool":"book_table"}"#;
        assert_eq!(signature("Jefe", 1_760_000_000, body), SIGNATURE);
        assert_ne!(signature("Jefe", 1_760_000_001, body), SIGNATURE);
        assert_ne!(signature("other", 1_760_000_000, body), SIGNATURE);
    }

    #[test]
    fn a_correction_is_framed_for_the_model() {
        let words = |all: &[&str]| all.iter().map(|w| (*w).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            request_text(&words(&["A table for two."])),
            "A table for two."
        );
        assert_eq!(
            request_text(&words(&["A table for two.", "Make it four.", "On Friday."])),
            "A table for two.\nCorrection: Make it four.\nCorrection: On Friday."
        );
    }

    #[test]
    fn a_leading_speaker_label_never_reaches_speech() {
        let mut gate = AnswerGate::new(true);
        let tokens = [
            "ANSWER:",
            " ToFront: Your table",
            " is booked.",
            " Front: See you.",
        ];
        assert_eq!(stream(&mut gate, &tokens), ["Your table is booked."]);
        assert_eq!(gate.end_round(false).sentences, ["See you."]);
    }

    #[tokio::test]
    async fn an_ask_caller_form_goes_to_the_screen_with_a_field_per_name() {
        let (tx, mut rx) = mpsc::channel(4);
        let calls = Arc::new(ToolCalls::new(
            RunId(1),
            &tx,
            &CancellationToken::new(),
            None,
        ));
        let ask = |args: Value| {
            let calls = Arc::clone(&calls);
            tokio::spawn(async move { calls.ask_caller(args).await })
        };
        let form = ask(json!({ "kind": "form", "question": "Your new address?",
                               "fields": ["street", "city"] }));
        let Some(Msg::Run(
            _,
            RunEvent::Gate {
                kind, args, reply, ..
            },
        )) = rx.recv().await
        else {
            panic!("no gate");
        };
        assert_eq!(kind, HitlKind::Form);
        assert_eq!(
            args["schema"],
            json!({ "type": "object", "required": ["street", "city"], "properties": {
                "street": { "type": "string", "title": "street" },
                "city": { "type": "string", "title": "city" } } })
        );
        let filled = json!({ "street": "Harbour Road", "city": "Leith" });
        reply.send(GateDecision::Answer(filled.clone())).unwrap();
        assert_eq!(form.await.unwrap(), Ok(json!({ "answer": filled })));
        // A form without usable fields is the model's mistake: it is told, nobody is asked.
        for fields in [
            json!(null),
            json!([]),
            json!(["street", "street"]),
            json!(["", "city"]),
            json!(["street", 7]),
            json!(vec!["field"; FORM_FIELDS_MAX + 1]),
        ] {
            let refused = ask(json!({ "kind": "form", "question": "Address?", "fields": fields }));
            assert!(
                refused.await.unwrap().unwrap_err().contains("fields"),
                "{fields}"
            );
        }
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn the_facts_the_background_writes_pass_the_memory_rules() {
        let answer = "<think>hm</think>\n- Prefers aisle seats.\n2. Uses a wheelchair\n\
                      * Card ends in 4417\nNONE\nKnown facts: Travels with a dog\n\nLikes quiet rooms\n\
                      Travels with a dog";
        assert_eq!(
            fact_lines(answer),
            [
                "Prefers aisle seats",
                "Uses a wheelchair",
                "Likes quiet rooms"
            ]
        );
        assert!(fact_lines("NONE").is_empty());
    }

    #[test]
    fn a_tool_round_for_later_runs_leaves_out_the_questions_to_the_caller() {
        let mut ask = call(r#"{"kind":"value","question":"Your card number?"}"#);
        ask.function.name = ASK_CALLER.to_owned();
        let book = call("{}");
        let results = [
            (ask.id.clone(), r#"{"answer":"4111"}"#.to_owned()),
            (book.id.clone(), r#"{"status":"booked"}"#.to_owned()),
        ];
        let round = tool_round(&[ask.clone(), book.clone()], &results);
        assert_eq!(round.len(), 2, "{round:?}");
        let calls = round[0].tool_calls.as_ref().expect("the call");
        assert_eq!(round[0].role, MessageRole::Assistant);
        assert_eq!((calls.len(), calls[0].id.as_str()), (1, book.id.as_str()));
        assert_eq!(round[1].role, MessageRole::Tool);
        assert_eq!(round[1].tool_call_id.as_deref(), Some(book.id.as_str()));
        assert_eq!(round[1].content.as_deref(), Some(r#"{"status":"booked"}"#));
        assert!(tool_round(&[ask], &results[..1]).is_empty());
    }

    #[test]
    fn an_orchestrator_of_agents_alone_has_no_ask_caller() {
        let (tx, _rx) = mpsc::channel(4);
        let calls = || {
            Arc::new(ToolCalls::new(
                RunId(1),
                &tx,
                &CancellationToken::new(),
                None,
            ))
        };
        let agent = AgentTool {
            name: "browser".to_owned(),
            description: "A browser agent.".to_owned(),
            fallback: None,
            parameters: None,
            confirm: false,
        };
        let names = |registry: FunctionRegistry| {
            let mut names: Vec<String> = (registry.request_tools().into_iter())
                .map(|tool| tool.function.name)
                .collect();
            names.sort();
            names
        };
        let agents = tool_registry(Vec::new(), vec![agent], calls());
        assert_eq!(names(agents), ["browser", CONVERSATION_CONTROL]);
        let tool = ToolSpec {
            name: "book_table".to_owned(),
            description: "Use it to book a table.".to_owned(),
            parameters: json!({ "type": "object" }),
            url: "http://127.0.0.1:9/t".to_owned(),
            side_effects: true,
            irreversible: false,
            confirm_phrase: None,
            confirm_template: None,
            progress_phrase: None,
            signed: false,
        };
        let own = tool_registry(vec![tool], Vec::new(), calls());
        assert_eq!(names(own), [ASK_CALLER, "book_table"]);
        // The rule it is told is the answer rule without the ask_caller paragraph.
        assert!(ANSWER_RULE.ends_with(MARKER_RULE) && !MARKER_RULE.contains(ASK_CALLER));
    }

    #[test]
    fn a_control_round_applies_only_a_change_of_how_the_call_goes() {
        let state =
            json!({ "rate": 1.0, "volume_db": 0.0, "verbosity": "brief", "language": "en" });
        let changes = |kind: &str, value: &str| {
            changes_the_call(&json!({ "kind": kind, "value": value }), Some(&state))
        };
        assert!(changes("rate", "slower") && changes("volume", "louder"));
        assert!(changes("verbosity", "detailed") && changes("language", "es"));
        // What the call is already: nothing to apply (live MA1's `rate normal`).
        assert!(!changes("rate", "normal") && !changes("volume", "normal"));
        assert!(!changes("verbosity", "brief") && !changes("language", "en"));
        // Not how the call goes: a control round never stops, holds, repeats or ends the call.
        for kind in ["stop", "hold", "repeat", "end", "human", "voice", "updates"] {
            assert!(!changes(kind, "on"), "{kind}");
        }
        // Without a state yet, a normal rate is still a request for one.
        let fresh = |kind: &str, value: &str| {
            changes_the_call(&json!({ "kind": kind, "value": value }), None)
        };
        assert!(fresh("verbosity", "brief") && fresh("rate", "normal"));
    }

    /// r7-verify: an orchestrator's `conversation_control` (its control round's or its own
    /// round's) is applied only when its `asked_by` quotes the operator's prompt or the caller's
    /// words (live MA7: `verbosity brief` and `updates tell`, asked by no one).
    #[tokio::test]
    async fn a_control_is_applied_only_when_someone_named_it() {
        let prompt = "You plan trips. Before you answer, call conversation_control with \
                      kind verbosity and value brief.";
        let words = "Find me a hotel in Goa for two nights. Could you speak a bit slower, please?";
        let (tx, mut rx) = mpsc::channel(4);
        let calls = ToolCalls::new(RunId(7), &tx, &CancellationToken::new(), None)
            .named_by(vec![prompt.to_owned(), words.to_owned()]);
        fn control(kind: &str, value: &str, asked_by: Option<&str>) -> Value {
            json!({ "kind": kind, "value": value, "asked_by": asked_by })
        }
        // Named by no one: no quote, an empty one, words neither said, a word out of order;
        // once the run's control round has ended without applying it.
        calls.round_done.cancel();
        for asked_by in [
            None,
            Some(" .. "),
            Some("keep it short"),
            Some("slower speak"),
        ] {
            let result = calls.control(control("verbosity", "brief", asked_by)).await;
            assert_eq!(result, Err(NOT_NAMED.to_owned()), "{asked_by:?}");
        }
        // Not how the call goes, whoever is quoted (live MA7: `updates tell`).
        let updates = calls.control(control("updates", "tell", Some("Find me a hotel")));
        assert_eq!(updates.await, Err(NOT_THIS_TOOL.to_owned()));
        assert!(rx.try_recv().is_err(), "nothing reported, nothing applied");
        // Named by the operator (case and punctuation aside), or by the caller: applied.
        let named = [
            (
                "verbosity",
                "brief",
                "call Conversation_Control with kind verbosity and value BRIEF",
            ),
            ("rate", "slower", "speak a bit slower"),
        ];
        for (kind, value, asked_by) in named {
            let applying = calls.control(control(kind, value, Some(asked_by)));
            let reply = async {
                match rx.recv().await {
                    Some(Msg::Run(_, RunEvent::Control { control, reply })) => {
                        assert_eq!(control.kind.as_str(), kind);
                        let _ = reply.send(json!({ "outcome": "applied" }));
                    }
                    _ => panic!("no control reported"),
                }
            };
            let (result, ()) = tokio::join!(applying, reply);
            assert_eq!(result, Ok(json!({ "outcome": "applied" })), "{kind}");
        }
        // The run's own round asking without a quote (offline it wrote "user") for what the
        // round applied gets that result, and nothing is applied twice.
        let again = calls.control(control("rate", "slower", Some("user"))).await;
        assert_eq!(again, Ok(json!({ "outcome": "applied" })));
        assert!(rx.try_recv().is_err());
        // The round asks for the quote with a value; the run's own rounds for the quote.
        assert_eq!(
            control_round_parameters()["required"],
            json!(["kind", "value", "asked_by"])
        );
        assert_eq!(control_tool().1["required"], json!(["kind", "asked_by"]));
        let kinds = control_tool().1["properties"]["kind"]["enum"].clone();
        assert_eq!(kinds.as_array().map(Vec::len), Some(ControlKind::ALL.len()));
        assert!(CONTROL_ROUND.contains("asked_by"));
        // No sources (another backend's gate): nothing is named.
        assert!(!named_in("brief", &[]) && named_in("Brief!", &["keep it brief"]));
    }

    /// r11-fix: a quote's words in their order within one sentence, at most three words apart,
    /// are named (CT3's "verbosity brief"), also after the round's own frame; out of order,
    /// further apart, or spread over two sentences, they are not.
    #[test]
    fn a_quote_is_named_by_its_words_within_one_sentence() {
        let prompt = "You hand requests to agents. Before you answer the caller's first question, \
                      call the tool conversation_control with kind verbosity and value brief. \
                      The agents know the caller's account and travel profile, so give them the \
                      request.";
        let words = "What's the capital of Peru?";
        for quote in [
            "verbosity brief",
            "kind verbosity value brief",
            "Verbosity: brief",
            "The caller said: \"What's the capital of Peru?\"",
        ] {
            assert!(named_in(quote, &[prompt, words]), "{quote}");
        }
        // The round's frame alone, or before words no one said, names nothing; nor words
        // with more than three others between them ("caller ... request" of one sentence).
        for quote in [
            "brief verbosity",
            "agents brief",
            "capital brief",
            "The caller said:",
            "The caller said: speak slower",
            "caller's request",
            "first question brief",
        ] {
            assert!(!named_in(quote, &[prompt, words]), "{quote}");
        }
    }

    /// r11-verify: a caller who repeats one word, and a quote of it that fails only at its
    /// last word, is answered at once (a backtracking search took 4^n steps).
    #[test]
    fn a_quote_of_repeated_words_is_checked_in_one_pass() {
        let said = "no ".repeat(400);
        let quote = format!("{}slower", "no ".repeat(60));
        let started = std::time::Instant::now();
        assert!(!named_in(&quote, &[&said]));
        assert!(named_in(&"no ".repeat(60), &[&said]));
        assert!(named_in("no slower", &[&format!("{said} slower")]));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    /// r7-verify: a run cancelled with its control round on the wire aborts the round before
    /// the round removes its history; the run removes it.
    #[tokio::test(flavor = "current_thread")]
    #[allow(clippy::await_holding_lock)]
    async fn a_cancelled_orchestrator_run_leaves_no_history_behind() {
        let _guard = crate::core::net::ssrf_env_lock();
        let previous = std::env::var_os("WAAV_ALLOW_LOOPBACK_ENDPOINTS");
        // SAFETY: test-only env mutation, serialized by core::net::ssrf_env_lock.
        unsafe { std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", "1") };
        // An LLM that takes each request and never answers.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
        let held = tokio::spawn(async move {
            let mut open = Vec::new();
            while let Ok((socket, _)) = listener.accept().await {
                open.push(socket);
            }
        });
        let llm = Arc::new(LlmClient::new(crate::core::llm::LlmClientConfig {
            base_url,
            streaming: true,
            ..Default::default()
        }));
        let agent = AgentTool {
            name: "trips".to_owned(),
            description: "Plans trips.".to_owned(),
            fallback: None,
            parameters: None,
            confirm: false,
        };
        let spec = RunSpec {
            run: RunId(1),
            history_key: "session#r1".to_owned(),
            llm: Arc::clone(&llm),
            api_key: None,
            system_prompt: "You plan trips.".to_owned(),
            operator_prompt: "You plan trips.".to_owned(),
            history: Vec::new(),
            words: vec!["find me a hotel in goa".to_owned()],
            tools: Vec::new(),
            agents: vec![agent],
            tool_secret: None,
            session_id: "session".to_owned(),
            attribution: None,
            trace_id: String::new(),
            reply_in: None,
            state: None,
            turn: None,
        };
        let (tx, mut rx) = mpsc::channel(16);
        let cancel = CancellationToken::new();
        spawn_run(spec, tx, cancel.clone());
        // The run's first round and its control round are both asked.
        for _ in 0..500 {
            if llm.active_session_count().await == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(llm.active_session_count().await, 2);
        cancel.cancel();
        let ended = tokio::time::timeout(Duration::from_secs(10), async {
            while let Some(message) = rx.recv().await {
                if let Msg::Run(_, RunEvent::Cancelled) = message {
                    return true;
                }
            }
            false
        });
        assert!(ended.await.unwrap(), "the run reports its cancellation");
        assert_eq!(llm.active_session_count().await, 0);
        held.abort();
        // SAFETY: restore the process env before releasing the test env lock.
        unsafe {
            match previous {
                Some(previous) => std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", previous),
                None => std::env::remove_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS"),
            }
        }
    }

    #[test]
    fn the_answer_rule_names_the_marker_and_refusals_never_claim_an_effect() {
        assert!(ANSWER_RULE.contains(ANSWER_MARKER));
        for reason in ["timeout", "stale", "cancelled", "discarded"] {
            assert!(refused(reason).ends_with("Nothing was done."), "{reason}");
        }
        assert!(refused("declined").starts_with("The caller declined."));
    }
}
