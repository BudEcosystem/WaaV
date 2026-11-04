//! The Bud agent as the background: one budprompt run per task revision, reached through the
//! agent engine's own client ([`TurnBackend`]) with the caller's credential.
//!
//! The gateway cannot prompt a Bud agent and does not run its tools, so three things differ
//! from the LLM tier:
//!
//! * **The answer.** Text streams to the caller only after the line the agent writes before its
//!   final answer (`background.answer_marker`, [`ANSWER_MARKER`] unless declared). The marker
//!   goes with each run as `bud_answer_marker`, and Bud's agent runtime tells the agent to write
//!   it. A run that never writes it releases the text after its last tool item when it
//!   completes: until then a draft and the answer look the same.
//! * **Tools.** A tool item is budprompt's. It is reported as an operation in flight while it
//!   runs and as an effect of the task when it has run. Only a call budprompt holds for the
//!   caller's approval (an `mcp_approval_request`, spec 029 §7.3) is gated here: the run asks
//!   the caller (`RunEvent::Gate`) while the turn's stream stays open, posts the caller's vote
//!   as a request of its own, and reads on. A declined call commits nothing; a cancel or a
//!   hang-up while the caller is asked declines it first.
//! * **Runs side by side.** Every task of the session continues the session's one budprompt
//!   conversation, and different tasks run at once (`background.max_parallel`, 3 unless
//!   declared): budprompt chains a conversation's turns in the order they arrive and a run's
//!   history skips the turns still running. The runs of ONE task stay in order: a revision
//!   waits for the task's run before it to end, its cancel included. `session.bud_context`
//!   goes with the first run that reaches budprompt, and while it is pending the other runs
//!   wait for that run to be accepted (`response.created`) or to fail before it got there.
//!   What the caller did not hear of an answer is sent with the next run as `bud_truncate`
//!   (one per request: the newest answer cut, the others with the runs after it).

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::Poll;

use futures::StreamExt;
use parking_lot::Mutex;
use serde_json::{Map, Value, json};
use tokio::sync::{Notify, mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, debug};

use crate::auth::SessionCredential;
use crate::core::agent::engine::{
    AgentSpan, agent_turn_span, open_turn, record_failure, traceparent_of,
};
use crate::core::agent::text::SpeakFieldExtractor;
use crate::core::agent::{
    AgentEvent, AgentSessionConfig, AgentTurnRequest, ApprovalVote, ClientExtras, Truncate,
    TurnBackend, TurnFailure,
};
use crate::observability::voice_attrs::realtime;

use super::actor::Msg;
use super::background::{
    ANSWER_MARKER, AnswerGate, Backend, Job, Relay, caller_words, request_text,
};
use super::config::ToolSpec;
use super::turn_record::TurnTrace;
use super::types::{GateDecision, HitlKind, RunEvent, RunId, ToolCall};

/// The marker a Bud agent background writes before its final answer: the one the agent's
/// entry or the operator declares, else [`ANSWER_MARKER`]. The gateway sends it with every run
/// (`bud_answer_marker`) and Bud's agent runtime tells the agent to write it. An agent that
/// leaves it out (a Bud gateway that ignores the field) is spoken whole when its run
/// completes, as without a marker.
pub fn answer_marker(declared: Option<String>) -> Option<String> {
    declared.or_else(|| Some(ANSWER_MARKER.to_owned()))
}

/// How many of the caller's requests a Bud agent runs at once unless `background.max_parallel`
/// says otherwise.
pub const DEFAULT_MAX_PARALLEL: usize = super::config::DEFAULT_MAX_PARALLEL as usize;
/// How many cut answers are owed to budprompt at most; the oldest is forgotten beyond it.
const TRUNCATES_KEPT: usize = 4;

/// The agent of one session and what its runs share.
pub struct AgentBackground {
    backend: Arc<dyn TurnBackend>,
    session: AgentSessionConfig,
    credential: SessionCredential,
    /// `background.answer_marker`: the declared one, or the default (see [`answer_marker`]).
    marker: Option<String>,
    /// `background.max_parallel`: the caller's requests that run at once.
    max_parallel: usize,
    /// Each run that has not ended yet (its cancel at budprompt included), held by it: the
    /// next run of the same task waits for it (`AgentRun::after`).
    live: Mutex<HashMap<RunId, Arc<tokio::sync::Mutex<()>>>>,
    /// budprompt's id of each run that got one.
    responses: Mutex<HashMap<RunId, String>>,
    /// The answers the caller did not hear in full, oldest first, one per response: each run
    /// that starts takes the newest whose run has ended.
    truncates: Mutex<Vec<Truncate>>,
    /// `session.bud_context`, until a run delivered it.
    context: Mutex<Context>,
    /// The run carrying the context was accepted, or ended without reaching budprompt.
    context_settled: Notify,
}

/// `session.bud_context` and the run that carries it.
#[derive(Default)]
struct Context {
    /// The items, until a run reached budprompt with them.
    items: Vec<Value>,
    /// A run was sent: `bud_context` is refused from now on.
    sent: bool,
    /// The run that carries the items, until budprompt accepted it or it ended. The other
    /// runs wait for it: their turns must come after the items in the conversation.
    carrier: Option<RunId>,
}

impl AgentBackground {
    pub fn new(
        backend: Arc<dyn TurnBackend>,
        session: AgentSessionConfig,
        credential: SessionCredential,
        marker: Option<String>,
    ) -> Arc<Self> {
        Self::with_parallel(backend, session, credential, marker, DEFAULT_MAX_PARALLEL)
    }

    /// [`AgentBackground::new`] with `background.max_parallel` (at least 1).
    pub fn with_parallel(
        backend: Arc<dyn TurnBackend>,
        session: AgentSessionConfig,
        credential: SessionCredential,
        marker: Option<String>,
        max_parallel: usize,
    ) -> Arc<Self> {
        Arc::new(Self {
            backend,
            session,
            credential,
            marker,
            max_parallel: max_parallel.max(1),
            live: Mutex::default(),
            responses: Mutex::default(),
            truncates: Mutex::default(),
            context: Mutex::default(),
            context_settled: Notify::new(),
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

    /// `session.bud_context`: message items that go ahead of the input of the agent's first run
    /// that reaches budprompt, which stores them in the session's conversation. Refused once a
    /// run was sent.
    pub fn set_context(&self, items: Vec<Value>) -> Result<(), &'static str> {
        let mut context = self.context.lock();
        if context.sent {
            return Err(
                "The conversation has started: `bud_context` is taken only before the first turn.",
            );
        }
        context.items = items;
        Ok(())
    }

    /// What the caller has heard of the answer of `run`. `cut`: the rest of it will not be
    /// heard, so a later run tells budprompt to keep only this much in its history.
    pub fn heard(&self, run: RunId, heard: &str, cut: bool) {
        let Some(response_id) = self.responses.lock().get(&run).cloned() else {
            // budprompt never knew the run: there is nothing to shorten.
            return;
        };
        let mut owed = self.truncates.lock();
        match owed.iter_mut().find(|t| t.response_id == response_id) {
            Some(known) => heard.clone_into(&mut known.output_text),
            None if cut => {
                owed.push(Truncate {
                    response_id,
                    output_text: heard.to_owned(),
                });
                if owed.len() > TRUNCATES_KEPT {
                    owed.remove(0);
                }
            }
            None => {}
        }
    }

    /// The truncate a run that starts now carries: the newest owed one whose run has ended
    /// (budprompt applies one, to the turn the request continues, and a run still going is
    /// not in a new run's history yet). The others stay owed.
    fn take_truncate(&self) -> Option<Truncate> {
        let running: HashSet<String> = {
            let live = self.live.lock();
            let responses = self.responses.lock();
            (live.keys())
                .filter_map(|run| responses.get(run).cloned())
                .collect()
        };
        let mut owed = self.truncates.lock();
        let at = owed
            .iter()
            .rposition(|t| !running.contains(&t.response_id))?;
        Some(owed.remove(at))
    }

    /// A run never reached budprompt: the truncate it carried is owed again, unless what the
    /// caller heard of that answer was told since.
    fn owe_again(&self, truncate: Option<Truncate>) {
        let Some(truncate) = truncate else { return };
        let mut owed = self.truncates.lock();
        if !owed.iter().any(|t| t.response_id == truncate.response_id) {
            owed.push(truncate);
            if owed.len() > TRUNCATES_KEPT {
                owed.remove(0);
            }
        }
    }

    /// The context `run` carries: the pending items when no other run carries them (`run`
    /// carries them then), nothing when none are pending. While another run carries them it
    /// waits for that run to settle. `None`: cancelled while it waited.
    async fn take_context(&self, run: RunId, cancel: &CancellationToken) -> Option<Vec<Value>> {
        loop {
            let settled = self.context_settled.notified();
            tokio::pin!(settled);
            settled.as_mut().enable();
            {
                let mut context = self.context.lock();
                context.sent = true;
                match context.carrier {
                    None if context.items.is_empty() => return Some(Vec::new()),
                    None => {
                        context.carrier = Some(run);
                        return Some(context.items.clone());
                    }
                    Some(_) => {}
                }
            }
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return None,
                _ = &mut settled => {}
            }
        }
    }

    /// `run` reached budprompt: the context it carries is delivered.
    fn context_delivered(&self, run: RunId) {
        let mut context = self.context.lock();
        if context.carrier == Some(run) {
            context.items.clear();
        }
    }

    /// `run` was accepted, or ended: the runs that waited for the context go on (the next
    /// carries it when it was not delivered).
    fn context_settle(&self, run: RunId, unreached: bool) {
        let mut context = self.context.lock();
        if context.carrier != Some(run) {
            return;
        }
        context.carrier = None;
        if unreached && !context.items.is_empty() {
            // Nothing was sent after all: the context may still be replaced.
            context.sent = false;
        }
        drop(context);
        self.context_settled.notify_waiters();
    }
}

/// Settles the context a run carries however its run ends.
struct ContextHold<'a> {
    agent: &'a AgentBackground,
    run: RunId,
    unreached: bool,
}

impl Drop for ContextHold<'_> {
    fn drop(&mut self) {
        self.agent.context_settle(self.run, self.unreached);
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
    /// The run of the same task before this one (`Job::after`): this one waits for it to end.
    pub after: Option<RunId>,
}

/// Start a run. It reports through `Msg::Run(spec.run, ..)` like an LLM-tier run and stops
/// when `cancel` fires; its last report is `Finished`, `Failed` or `Cancelled`.
pub fn spawn_agent_run(
    spec: AgentRun,
    tx: mpsc::Sender<Msg>,
    cancel: CancellationToken,
) -> JoinHandle<()> {
    let (run, agent) = (spec.run, Arc::clone(&spec.agent));
    // Held until the run has ended, from now: a later run of its task, started before this
    // one is polled, still waits for it.
    let mine = Arc::new(tokio::sync::Mutex::new(()));
    let held = Arc::clone(&mine).try_lock_owned().expect("a new lock");
    let before = {
        let mut live = agent.live.lock();
        let before = spec.after.and_then(|after| live.get(&after).cloned());
        live.insert(run, mine);
        before
    };
    tokio::spawn(async move {
        // The task's run before this one may still be ending: its cancel is delivered first.
        let ready = match before {
            None => true,
            Some(before) => tokio::select! {
                biased;
                _ = cancel.cancelled() => false,
                _ = before.lock() => true,
            },
        };
        let mut traceparent = None;
        let last = if ready {
            let input = request_text(&spec.words);
            // A child of its caller turn's span, in the turn's trace (spec 029 D-9).
            let parent = spec.turn.as_ref().map(|turn| &turn.span);
            let index = spec.turn.as_ref().map_or(run.0, |turn| turn.index);
            let span = agent_turn_span(&agent.session, index, &input, parent, AgentSpan::Run);
            traceparent = traceparent_of(&span);
            let driven = drive(spec, input, &span, &tx, &cancel);
            driven.instrument(span.clone()).await
        } else {
            RunEvent::Cancelled
        };
        let cancelled = matches!(last, RunEvent::Cancelled);
        let _ = tx.send(Msg::Run(run, last)).await;
        // Dropping the stream stopped the run; the cancel records it as cancelled.
        let response = agent.responses.lock().get(&run).cloned();
        if let (true, Some(response_id)) = (cancelled, response) {
            let bearer = agent.credential.current();
            let name = agent.name();
            let traceparent = traceparent.as_deref();
            let backend = &agent.backend;
            backend
                .cancel(&response_id, name, &bearer, traceparent)
                .await;
        }
        agent.live.lock().remove(&run);
        drop(held);
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
        agent_turn_span(&self.session, turn, input, None, AgentSpan::CallerTurn)
    }

    /// `background.max_parallel`: budprompt chains the runs of the session's one conversation
    /// in the order they arrive, so different tasks run at once.
    fn max_concurrent(&self) -> Option<usize> {
        Some(self.max_parallel)
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
            after: job.after,
        };
        spawn_agent_run(spec, tx, cancel)
    }

    fn heard(&self, run: RunId, text: &str, cut: bool) {
        AgentBackground::heard(self, run, text, cut);
    }

    /// The entry's `tool_confirmations` (spec 029 §7.3): the wording of the question a held
    /// call raises. What the author left out is the gate's default: the tool's name is said,
    /// and a strong "yes" is needed.
    fn confirmations(&self) -> Vec<ToolSpec> {
        let Some(confirmations) = &self.session.entry.tool_confirmations else {
            return Vec::new();
        };
        let text = |text: &Option<String>| text.clone().filter(|text| !text.trim().is_empty());
        (confirmations.iter())
            .map(|(name, confirmation)| ToolSpec {
                name: name.clone(),
                description: String::new(),
                parameters: json!({"type": "object"}),
                url: String::new(),
                side_effects: true,
                irreversible: confirmation.irreversible.unwrap_or(true),
                confirm_phrase: text(&confirmation.phrase),
                confirm_template: text(&confirmation.template),
                progress_phrase: None,
                signed: false,
            })
            .collect()
    }

    /// The entry's `approval_message` (FR-APR-1).
    fn approval_message(&self) -> Option<String> {
        let message = &self.session.entry.approval_message;
        (!message.trim().is_empty()).then(|| message.clone())
    }
}

/// A call budprompt holds for the caller's vote, while the caller is asked.
struct Hold {
    /// The hold's id (the escalation's): the vote names it.
    id: String,
    name: String,
    decision: oneshot::Receiver<GateDecision>,
}

/// What the run does next.
enum Next {
    Cancelled,
    /// The caller answered the question of `holds[index]`.
    Decided(usize, GateDecision),
    Event(Option<Result<AgentEvent, TurnFailure>>),
}

/// The first held call whose question was answered: its index and the answer. A question the
/// registry closed without an answer is a cancel.
fn decided(holds: &mut [Hold]) -> impl Future<Output = (usize, GateDecision)> + '_ {
    std::future::poll_fn(move |cx| {
        for (index, hold) in holds.iter_mut().enumerate() {
            if let Poll::Ready(decision) = Pin::new(&mut hold.decision).poll(cx) {
                let decision = decision.unwrap_or(GateDecision::Refuse("cancelled"));
                return Poll::Ready((index, decision));
            }
        }
        Poll::Pending
    })
}

/// The vote an answer is: `Allow` approves, anything else declines with its reason.
fn verdict(decision: &GateDecision) -> (bool, Option<&'static str>) {
    match decision {
        GateDecision::Allow(_) => (true, None),
        GateDecision::Refuse(reason) => (false, Some(reason)),
        GateDecision::Answer(_) => (false, Some("declined")),
    }
}

/// Milliseconds from now to the unix time `at` (seconds), none when it has passed.
/// The time the caller's vote on a held call needs to reach budprompt: the caller's window
/// closes this much before the hold's `bud_expires_at`, so the vote is in before budprompt
/// declines the call on its own.
const VOTE_MARGIN_MS: u64 = 1_500;

fn ms_until(at: f64) -> u64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64());
    ((at - now).max(0.0) * 1000.0) as u64
}

/// Post the caller's vote on `hold`, continuing the run's turn with the run's own bearer. A
/// vote that does not arrive leaves the call to budprompt's deadline, which declines it.
async fn vote(
    agent: &AgentBackground,
    run: RunId,
    hold: &Hold,
    (approve, reason): (bool, Option<&str>),
    traceparent: Option<&str>,
) {
    let Some(previous_response_id) = agent.responses.lock().get(&run).cloned() else {
        debug!(run = run.0, approval = %hold.id, "a held call before the run had an id: no vote");
        return;
    };
    let vote = ApprovalVote {
        previous_response_id,
        approval_request_id: hold.id.clone(),
        approve,
        reason: reason.map(str::to_owned),
    };
    let bearer = agent.credential.current();
    let posted = (agent.backend)
        .vote(&vote, agent.name(), &bearer, traceparent)
        .await;
    if let Err(failure) = posted {
        tracing::warn!(
            run = run.0,
            approval = %hold.id,
            approve,
            code = failure.code(),
            error = failure.message(),
            "the caller's vote did not reach the agent; budprompt declines the call at its deadline"
        );
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
    // While another run carries the conversation before the session, this one waits for it.
    let Some(context) = agent.take_context(run, cancel).await else {
        return RunEvent::Cancelled;
    };
    let mut hold = ContextHold {
        agent,
        run,
        unreached: true,
    };
    let truncate = agent.take_truncate();
    let request = AgentTurnRequest {
        prompt_name: session.prompt_name.clone(),
        version: session.version,
        variables: session.variables.clone(),
        input,
        conversation_id: session.conversation_id.clone(),
        truncate: truncate.clone(),
        session_id: session.session_id.clone(),
        turn_index: run.0,
        extras: ClientExtras {
            items: context,
            ..ClientExtras::default()
        },
        // Bud's agent runtime tells the agent to write it before its final answer.
        answer_marker: agent.marker.clone(),
    };
    let bearer = agent.credential.current();
    let opened = open_turn(&agent.backend, &request, &bearer, span, cancel).await;
    let mut stream = match opened {
        Some(Ok(stream)) => stream,
        unreached => {
            // budprompt never saw the request: the truncate it carried still applies, and so
            // does the context (the hold gives it to the next run).
            agent.owe_again(truncate);
            return match unreached {
                Some(Err(failure)) => failed(span, &failure),
                _ => RunEvent::Cancelled,
            };
        }
    };
    hold.unreached = false;
    agent.context_delivered(run);

    let mut relay = Relay::new(run, tx, AnswerGate::held(agent.marker.clone()), spec.caller);
    relay.transforms = Some(entry.text_transforms.clone());
    // A structured-output agent speaks one field of its JSON.
    let speak_field = entry.structured_output.speak_field.clone();
    let mut spoken_field = speak_field
        .filter(|field| !field.is_empty())
        .map(SpeakFieldExtractor::new);
    // Tool items that started and have not reported back, by item id.
    let mut tools: HashMap<String, (String, tokio::time::Instant)> = HashMap::new();
    // The calls held for the caller's vote whose question is open, and those declined.
    let mut holds: Vec<Hold> = Vec::new();
    let mut declined: HashSet<String> = HashSet::new();
    let traceparent = traceparent_of(span);
    let traceparent = traceparent.as_deref();
    // The client hears of each tool (`tool_started`, `tool_done`) through the actor.
    let tell = async |call: ToolCall| {
        let _ = tx.send(Msg::ToolCall(run, call)).await;
    };
    let report = async |event: RunEvent| {
        let _ = tx.send(Msg::Run(run, event)).await;
    };

    loop {
        let next = tokio::select! {
            biased;
            _ = cancel.cancelled() => Next::Cancelled,
            (index, decision) = decided(&mut holds), if !holds.is_empty() => {
                Next::Decided(index, decision)
            }
            event = stream.next() => Next::Event(event),
        };
        let event = match next {
            Next::Cancelled => {
                // The caller hung up, or the task was cancelled, while asked: a held call is
                // declined before the run is cancelled, and it never ran.
                for hold in std::mem::take(&mut holds) {
                    declined.insert(hold.name.clone());
                    vote(agent, run, &hold, (false, Some("cancelled")), traceparent).await;
                }
                // What a tool that was still running did is not seen here.
                for (tool, _) in tools.values().filter(|(tool, _)| !declined.contains(tool)) {
                    report(RunEvent::Committed {
                        label: label("started", tool),
                    })
                    .await;
                }
                span.record(realtime::RESPONSE_STATUS, "cancelled");
                return RunEvent::Cancelled;
            }
            Next::Decided(index, decision) => {
                let hold = holds.remove(index);
                let verdict = verdict(&decision);
                if !verdict.0 {
                    declined.insert(hold.name.clone());
                }
                // The same stream goes on once budprompt has the vote.
                vote(agent, run, &hold, verdict, traceparent).await;
                continue;
            }
            Next::Event(event) => event,
        };
        match event {
            Some(Ok(AgentEvent::Created { response_id, .. })) => {
                span.record(realtime::RESPONSE_ID, response_id.as_str());
                agent.responses.lock().insert(run, response_id);
                // budprompt has the run in the conversation: the runs waiting for the
                // context go on.
                agent.context_settle(run, false);
                // budprompt works on it: the health watcher's first-token deadline is met, as a
                // remote backend's is when it accepts the request (spec 029 W-9). The run may list
                // its tools and reason for seconds before it writes; that is no stall.
                relay.alive().await;
            }
            Some(Ok(AgentEvent::TextDelta { delta, .. })) => {
                // budprompt writes nothing while it holds a call: the holds were resolved
                // without the caller (a designated approver, budprompt's deadline).
                holds.clear();
                let text = match spoken_field.as_mut() {
                    Some(field) => field.push(&delta),
                    None => delta,
                };
                relay.push(&text).await;
            }
            Some(Ok(AgentEvent::ToolStarted { item_id, name, .. })) => {
                // A call still held runs: someone else approved it.
                if let Some(at) = holds.iter().position(|hold| hold.name == name) {
                    holds.remove(at);
                }
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
                tell(ToolCall::Started {
                    item_id: item_id.clone(),
                    name: name.clone(),
                })
                .await;
                tools.insert(item_id, (name, tokio::time::Instant::now()));
            }
            Some(Ok(
                finished @ (AgentEvent::ToolFinished { .. } | AgentEvent::FunctionCall { .. }),
            )) => {
                let (item_id, name, ok, error) = match finished {
                    AgentEvent::ToolFinished {
                        item_id,
                        name,
                        ok,
                        error,
                    } => (item_id, name, ok, error),
                    AgentEvent::FunctionCall {
                        item_id, name, ok, ..
                    } => (item_id, name, ok, None),
                    _ => continue,
                };
                let since = tools.remove(&item_id).map(|(_, since)| since);
                tell(ToolCall::Done {
                    item_id,
                    name: name.clone(),
                    duration_ms: since.map(|at| at.elapsed().as_millis() as u64),
                    ok,
                    error,
                })
                .await;
                // A call the caller declined did nothing.
                if !ok && declined.remove(&name) {
                    continue;
                }
                // A tool that failed may still have done a part of its work.
                let verb = if ok { "used" } else { "tried" };
                let label = label(verb, &name);
                report(RunEvent::Committed { label }).await;
            }
            Some(Ok(AgentEvent::ApprovalRequest {
                id,
                name,
                arguments,
                expires_at,
                ..
            })) => {
                // What the agent wrote before the call was not its answer.
                relay.end(true).await;
                let mut args = match serde_json::from_str::<Value>(&arguments) {
                    Ok(args @ Value::Object(_)) => args,
                    _ => Value::Object(Map::new()),
                };
                // budprompt's deadline, less the vote's way back, is the question's: the
                // caller's whole window, never moved (`expires_in_ms`).
                if let Some(at) = expires_at {
                    let ms = ms_until(at).saturating_sub(VOTE_MARGIN_MS);
                    args["expires_in_ms"] = Value::from(ms);
                }
                let (reply, decision) = oneshot::channel();
                let gate = RunEvent::Gate {
                    op_name: name.clone(),
                    args,
                    kind: HitlKind::Confirm,
                    reply,
                };
                report(gate).await;
                holds.push(Hold { id, name, decision });
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
                // `approval_required`: held for an approval only a designated person can give.
                let failure = TurnFailure::of_run(&code, &message);
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

    use super::super::types::{APPROVAL_REQUIRED, GateDecision, HitlKind, OpId, ToolCall};
    use super::*;
    use crate::core::agent::ApprovalVote;

    type Reply = Result<Vec<AgentEvent>, TurnFailure>;

    /// A Bud gateway that answers each request with the next scripted run. A run's stream
    /// stays open after its events, as a run that is still working does.
    #[derive(Default)]
    struct Scripted {
        replies: Mutex<VecDeque<Reply>>,
        requests: Mutex<Vec<AgentTurnRequest>>,
        cancels: Mutex<Vec<String>>,
        /// Each vote, with the bearer it went with.
        votes: Mutex<Vec<(ApprovalVote, String)>>,
        /// `vote` and `cancel`, in the order they came.
        order: Mutex<Vec<&'static str>>,
        /// The requests (by their index) whose events the test sends itself, when it wants.
        gated: Mutex<HashMap<usize, futures::channel::mpsc::UnboundedReceiver<AgentEvent>>>,
    }

    impl Scripted {
        /// Request `index` gets the events the returned sender sends, in place of a script.
        fn gate(&self, index: usize) -> futures::channel::mpsc::UnboundedSender<AgentEvent> {
            let (tx, rx) = futures::channel::mpsc::unbounded();
            self.gated.lock().insert(index, rx);
            tx
        }
    }

    #[async_trait]
    impl TurnBackend for Scripted {
        async fn open(
            &self,
            request: &AgentTurnRequest,
            _bearer: &str,
            _traceparent: Option<&str>,
        ) -> Result<BoxStream<'static, Result<AgentEvent, TurnFailure>>, TurnFailure> {
            let index = {
                let mut requests = self.requests.lock();
                requests.push(request.clone());
                requests.len() - 1
            };
            if let Some(gated) = self.gated.lock().remove(&index) {
                return Ok(gated.map(Ok).chain(stream::pending()).boxed());
            }
            let events = self.replies.lock().pop_front().expect("a scripted run")?;
            Ok(stream::iter(events.into_iter().map(Ok))
                .chain(stream::pending())
                .boxed())
        }

        async fn cancel(
            &self,
            response_id: &str,
            _prompt_name: &str,
            _bearer: &str,
            _traceparent: Option<&str>,
        ) {
            self.cancels.lock().push(response_id.to_owned());
            self.order.lock().push("cancel");
        }

        async fn vote(
            &self,
            vote: &ApprovalVote,
            _prompt_name: &str,
            bearer: &str,
            _traceparent: Option<&str>,
        ) -> Result<(), TurnFailure> {
            self.votes.lock().push((vote.clone(), bearer.to_owned()));
            self.order.lock().push("vote");
            Ok(())
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
            session_link: None,
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
            error: None,
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
        start_after(agent, run, request, None)
    }

    /// One run of `request` that revises its task's run `after`.
    fn start_after(
        agent: &Arc<AgentBackground>,
        run: u64,
        request: &str,
        after: Option<u64>,
    ) -> (mpsc::Receiver<Msg>, CancellationToken) {
        let (tx, rx) = mpsc::channel(64);
        let cancel = CancellationToken::new();
        let spec = AgentRun {
            run: RunId(run),
            agent: Arc::clone(agent),
            words: vec![request.to_owned()],
            caller: request.to_owned(),
            turn: None,
            after: after.map(RunId),
        };
        spawn_agent_run(spec, tx, cancel.clone());
        (rx, cancel)
    }

    /// The run's next report, past the health watcher's sign of life (`Msg::RunAlive`).
    async fn next_report(rx: &mut mpsc::Receiver<Msg>) -> Option<Msg> {
        loop {
            match rx.recv().await {
                Some(Msg::RunAlive(_)) | Some(Msg::ToolCall(..)) => continue,
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
                Some(Msg::RunAlive(_)) | Some(Msg::ToolCall(..)) => continue,
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
    async fn the_agents_tools_are_told_with_their_item_duration_and_error() {
        let backend = Arc::new(Scripted::default());
        backend.replies.lock().push_back(Ok(vec![
            created("resp_1"),
            tool_started("lookup_order"),
            AgentEvent::ToolFinished {
                item_id: "item_lookup_order".to_owned(),
                name: "lookup_order".to_owned(),
                ok: false,
                error: Some("upstream 503".to_owned()),
            },
            text("I could not look it up."),
            completed(),
        ]));
        let agent = agent(&backend, None);
        let (tx, mut rx) = mpsc::channel(64);
        let spec = AgentRun {
            run: RunId(3),
            agent: Arc::clone(&agent),
            words: vec!["where is my order".to_owned()],
            caller: "where is my order".to_owned(),
            turn: None,
            after: None,
        };
        spawn_agent_run(spec, tx, CancellationToken::new());
        let mut calls = Vec::new();
        loop {
            let msg = tokio::time::timeout(Duration::from_secs(30), rx.recv())
                .await
                .expect("the run reports");
            match msg {
                Some(Msg::ToolCall(run, call)) => calls.push((run, call)),
                Some(Msg::Run(_, event)) if event.is_last() => break,
                Some(_) => {}
                None => panic!("the run ended without a last report"),
            }
        }
        assert_eq!(calls.len(), 2, "{calls:?}");
        assert_eq!(
            calls[0],
            (
                RunId(3),
                ToolCall::Started {
                    item_id: "item_lookup_order".to_owned(),
                    name: "lookup_order".to_owned()
                }
            )
        );
        match &calls[1] {
            (
                RunId(3),
                ToolCall::Done {
                    item_id,
                    name,
                    duration_ms: Some(_),
                    ok: false,
                    error: Some(error),
                },
            ) => {
                assert_eq!(
                    (item_id.as_str(), name.as_str()),
                    ("item_lookup_order", "lookup_order")
                );
                assert_eq!(error, "upstream 503");
            }
            other => panic!("not the tool's end: {other:?}"),
        }
    }

    #[tokio::test]
    async fn the_context_goes_ahead_of_the_first_run_only() {
        let backend = Arc::new(Scripted::default());
        for id in ["resp_1", "resp_2"] {
            backend
                .replies
                .lock()
                .push_back(Ok(vec![created(id), text("Yes."), completed()]));
        }
        let agent = agent(&backend, None);
        let context = vec![
            json!({"type": "message", "role": "user", "content": "My order is 4417."}),
            json!({"type": "message", "role": "assistant", "content": "Noted."}),
        ];
        assert_eq!(agent.set_context(context.clone()), Ok(()));
        let (mut rx, _cancel) = start(&agent, 1, "did it ship");
        reports(&mut rx).await;
        let (mut rx, _cancel) = start(&agent, 2, "when");
        reports(&mut rx).await;
        let requests = backend.requests.lock().clone();
        let mut want = context.clone();
        want.push(json!({"role": "user", "content": "did it ship"}));
        assert_eq!(requests[0].body()["input"], Value::Array(want));
        assert_eq!(requests[1].body()["input"], "when");
        assert!(
            agent.set_context(context).is_err(),
            "too late once a run was sent"
        );
    }

    #[tokio::test]
    async fn the_context_waits_for_a_run_that_reaches_the_agent() {
        let backend = Arc::new(Scripted::default());
        backend
            .replies
            .lock()
            .push_back(Err(TurnFailure::BadRequest("unavailable".to_owned())));
        backend
            .replies
            .lock()
            .push_back(Ok(vec![created("resp_2"), text("Hi."), completed()]));
        let agent = agent(&backend, None);
        let context = vec![json!({"type": "message", "role": "user", "content": "Earlier."})];
        agent.set_context(context.clone()).unwrap();
        let (mut rx, _cancel) = start(&agent, 1, "hello");
        reports(&mut rx).await;
        let (mut rx, _cancel) = start(&agent, 2, "hello again");
        reports(&mut rx).await;
        let requests = backend.requests.lock().clone();
        let last = requests.last().unwrap().body();
        assert_eq!(last["input"][0], context[0]);
        assert_eq!(
            last["input"][1],
            json!({"role": "user", "content": "hello again"})
        );
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
    async fn the_marker_goes_with_the_request_and_no_marker_sends_none() {
        for (marker, sent) in [(Some("FINAL:"), Some("FINAL:")), (None, None)] {
            let backend = Arc::new(Scripted::default());
            let run = vec![created("resp_1"), text("Hello."), completed()];
            backend.replies.lock().push_back(Ok(run));
            let agent = agent(&backend, marker);
            let (mut rx, _cancel) = start(&agent, 1, "hi");
            reports(&mut rx).await;
            let body = backend.requests.lock()[0].body();
            assert_eq!(
                body.get("bud_answer_marker").and_then(|m| m.as_str()),
                sent,
                "{marker:?}"
            );
        }
    }

    #[tokio::test]
    async fn an_agent_with_no_declared_marker_is_told_the_default_and_streams_after_it() {
        assert_eq!(answer_marker(None).as_deref(), Some("ANSWER:"));
        // A declared marker wins.
        let declared = answer_marker(Some("FINAL:".to_owned()));
        assert_eq!(declared.as_deref(), Some("FINAL:"));

        let backend = Arc::new(Scripted::default());
        backend.replies.lock().push_back(Ok(vec![
            created("resp_1"),
            text("Checking. ANSWER: We open at nine. We close"),
        ]));
        let agent = agent(&backend, answer_marker(None).as_deref());
        let (mut rx, cancel) = start(&agent, 1, "when do you open");
        let first = tokio::time::timeout(Duration::from_secs(30), next_report(&mut rx)).await;
        let Ok(Some(Msg::Run(_, RunEvent::Sentence { text, .. }))) = first else {
            panic!("a sentence before the run ends: {first:?}");
        };
        assert_eq!(text, "We open at nine.");
        let body = backend.requests.lock()[0].body();
        assert_eq!(body["bud_answer_marker"], "ANSWER:");
        cancel.cancel();
        reports(&mut rx).await;
    }

    #[tokio::test]
    async fn an_agent_that_never_writes_the_default_marker_is_spoken_whole_at_completion() {
        // A Bud gateway that ignores `bud_answer_marker`: the agent writes no marker, and is
        // heard exactly as an agent with no marker at all.
        let mut heard = Vec::new();
        for marker in [answer_marker(None), None] {
            let backend = Arc::new(Scripted::default());
            backend.replies.lock().push_back(Ok(vec![
                created("resp_1"),
                text("Let me check. "),
                tool_started("lookup_order"),
                tool_finished("lookup_order", true),
                text("It shipped on Monday. "),
                text("It arrives Friday."),
                completed(),
            ]));
            let agent = agent(&backend, marker.as_deref());
            let (mut rx, _cancel) = start(&agent, 1, "where is my order");
            heard.push(reports(&mut rx).await);
        }
        assert_eq!(heard[0], heard[1]);
        assert_eq!(
            heard[0].last().unwrap(),
            r#"Finished { text: "It shipped on Monday. It arrives Friday." }"#
        );
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
        assert!(agent.truncates.lock().is_empty());
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
    async fn a_revision_waits_for_its_tasks_run_before_it_and_for_its_cancel() {
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
        // The revised request: it is not sent while the task's first run is still on.
        let revised = "refund my order\nCorrection: the other one";
        let (mut second, _cancel) = start_after(&agent, 2, revised, Some(1));
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
        let (mut fourth, cancel) = start_after(&agent, 4, "one more thing, the blue one", Some(3));
        cancel.cancel();
        assert_eq!(reports(&mut fourth).await, ["Cancelled"]);
        assert_eq!(backend.requests.lock().len(), 3);
    }

    async fn settle() {
        for _ in 0..50 {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test]
    async fn the_runs_of_different_tasks_go_at_once_and_a_revision_waits_only_for_its_own() {
        let backend = Arc::new(Scripted::default());
        backend.replies.lock().extend([
            Ok(vec![created("resp_1"), tool_started("lookup_order")]),
            Ok(vec![created("resp_2"), tool_started("check_stock")]),
            Ok(vec![created("resp_3"), text("Done."), completed()]),
        ]);
        let agent = agent(&backend, None);
        assert_eq!(agent.max_concurrent(), Some(DEFAULT_MAX_PARALLEL));
        let (mut first, cancel_first) = start(&agent, 1, "where is my order");
        // Another task: it does not wait for the first run.
        let (mut second, _cancel_second) = start(&agent, 2, "is the blue one in stock");
        for rx in [&mut first, &mut second] {
            assert!(matches!(
                next_report(rx).await,
                Some(Msg::Run(_, RunEvent::ToolStarted { .. }))
            ));
        }
        assert_eq!(backend.requests.lock().len(), 2, "two runs at once");

        // The first task's revision waits for the first run, not for the second.
        let (mut revised, _cancel) = start_after(&agent, 3, "where is my other order", Some(1));
        settle().await;
        assert_eq!(backend.requests.lock().len(), 2);
        cancel_first.cancel();
        reports(&mut first).await;
        let done = reports(&mut revised).await;
        assert_eq!(done.last().unwrap(), r#"Finished { text: "Done." }"#);
        assert_eq!(backend.requests.lock().len(), 3);
        assert_eq!(
            *backend.cancels.lock(),
            ["resp_1"],
            "the second run goes on"
        );

        // `background.max_parallel` is the agent's limit, at least one.
        let session = agent.session.clone();
        let credential = SessionCredential::new("bud-caller-key");
        let brain = Arc::clone(&backend) as Arc<dyn TurnBackend>;
        let one = AgentBackground::with_parallel(brain, session, credential, None, 0);
        assert_eq!(one.max_concurrent(), Some(1));
    }

    #[tokio::test]
    async fn while_the_context_is_on_its_way_the_other_runs_wait_for_its_acceptance() {
        let backend = Arc::new(Scripted::default());
        let first = backend.gate(0);
        backend
            .replies
            .lock()
            .push_back(Ok(vec![created("resp_2"), text("Yes."), completed()]));
        let agent = agent(&backend, None);
        let context = vec![json!({"type": "message", "role": "user", "content": "Earlier."})];
        agent.set_context(context.clone()).unwrap();
        let (mut one, _cancel_one) = start(&agent, 1, "did it ship");
        settle().await;
        let (mut two, _cancel_two) = start(&agent, 2, "and the store hours");
        settle().await;
        // budprompt has not accepted the run with the context: the second is not sent.
        assert_eq!(backend.requests.lock().len(), 1);
        first.unbounded_send(created("resp_1")).unwrap();
        let done = reports(&mut two).await;
        assert_eq!(done.last().unwrap(), r#"Finished { text: "Yes." }"#);
        let requests = backend.requests.lock().clone();
        assert_eq!(requests[0].body()["input"][0], context[0]);
        assert_eq!(requests[1].body()["input"], "and the store hours");
        first.unbounded_send(text("It did.")).unwrap();
        first.unbounded_send(completed()).unwrap();
        reports(&mut one).await;
    }

    #[tokio::test]
    async fn with_no_context_on_its_way_a_run_does_not_wait_for_the_one_before_it() {
        let backend = Arc::new(Scripted::default());
        // The first run is not accepted yet (no `response.created`).
        let _first = backend.gate(0);
        backend
            .replies
            .lock()
            .push_back(Ok(vec![created("resp_2"), text("Yes."), completed()]));
        let agent = agent(&backend, None);
        let (_one, _cancel_one) = start(&agent, 1, "did it ship");
        settle().await;
        let (mut two, _cancel_two) = start(&agent, 2, "and the store hours");
        let done = reports(&mut two).await;
        assert_eq!(done.last().unwrap(), r#"Finished { text: "Yes." }"#);
        assert_eq!(backend.requests.lock().len(), 2);
    }

    #[tokio::test]
    async fn what_the_caller_did_not_hear_of_two_answers_reaches_budprompt_newest_first() {
        let backend = Arc::new(Scripted::default());
        for (id, said) in [
            ("resp_1", "It shipped on Monday."),
            ("resp_2", "We open at nine."),
        ] {
            let run = vec![created(id), text(said), completed()];
            backend.replies.lock().push_back(Ok(run));
        }
        for id in ["resp_3", "resp_4", "resp_5"] {
            let run = vec![created(id), text("Okay."), completed()];
            backend.replies.lock().push_back(Ok(run));
        }
        let agent = agent(&backend, None);
        let (mut one, _c1) = start(&agent, 1, "where is my order");
        let (mut two, _c2) = start(&agent, 2, "when do you open");
        reports(&mut one).await;
        reports(&mut two).await;
        // Both answers were cut.
        agent.heard(RunId(1), "It shipped", true);
        agent.heard(RunId(2), "We open", true);
        for run in 3..=5 {
            let (mut rx, _cancel) = start(&agent, run, "thanks");
            reports(&mut rx).await;
        }
        let truncates: Vec<Option<String>> = backend
            .requests
            .lock()
            .iter()
            .map(|request| request.truncate.as_ref().map(|t| t.response_id.clone()))
            .collect();
        let want = |id: &str| Some(id.to_owned());
        assert_eq!(
            truncates,
            [None, None, want("resp_2"), want("resp_1"), None],
            "one per request, the newest first"
        );
    }

    #[tokio::test]
    async fn an_answer_still_running_is_not_truncated_by_a_run_beside_it() {
        let backend = Arc::new(Scripted::default());
        backend.replies.lock().extend([
            Ok(vec![
                created("resp_1"),
                text("ANSWER: It shipped on Monday. It"),
            ]),
            Ok(vec![created("resp_2"), text("Okay."), completed()]),
            Ok(vec![created("resp_3"), text("Okay."), completed()]),
        ]);
        let agent = agent(&backend, Some("ANSWER:"));
        let (mut one, cancel_one) = start(&agent, 1, "where is my order");
        assert!(matches!(
            next_report(&mut one).await,
            Some(Msg::Run(_, RunEvent::Sentence { .. }))
        ));
        agent.heard(RunId(1), "It shipped", true);
        // Its run is still on: budprompt would not replay it yet, so the truncate waits.
        let (mut two, _c2) = start(&agent, 2, "and the hours");
        reports(&mut two).await;
        cancel_one.cancel();
        reports(&mut one).await;
        settle().await;
        let (mut three, _c3) = start(&agent, 3, "thanks");
        reports(&mut three).await;
        let requests = backend.requests.lock().clone();
        assert_eq!(requests[1].truncate, None);
        assert_eq!(
            requests[2]
                .truncate
                .as_ref()
                .map(|t| t.response_id.as_str()),
            Some("resp_1")
        );
    }

    // ---- Spec 029 §7.3: a tool call budprompt holds for the caller's approval ----

    fn unix_now() -> f64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64()
    }

    /// The hold of `name` with `arguments`, `expires_in_s` from now.
    fn hold(id: &str, name: &str, arguments: &str, expires_in_s: Option<f64>) -> AgentEvent {
        AgentEvent::ApprovalRequest {
            id: id.to_owned(),
            name: name.to_owned(),
            arguments: arguments.to_owned(),
            server_label: Some("orders".to_owned()),
            expires_at: expires_in_s.map(|s| unix_now() + s),
        }
    }

    /// The gate the run raises, past its other reports.
    async fn next_gate(
        rx: &mut mpsc::Receiver<Msg>,
    ) -> (
        String,
        Value,
        HitlKind,
        tokio::sync::oneshot::Sender<GateDecision>,
    ) {
        loop {
            let waited = tokio::time::timeout(Duration::from_secs(30), rx.recv()).await;
            match waited.expect("the run asks") {
                Some(Msg::Run(
                    _,
                    RunEvent::Gate {
                        op_name,
                        args,
                        kind,
                        reply,
                    },
                )) => return (op_name, args, kind, reply),
                Some(Msg::Run(_, event)) if event.is_last() => panic!("ended: {event:?}"),
                Some(_) => {}
                None => panic!("the run ended without asking"),
            }
        }
    }

    async fn votes_reach(backend: &Scripted, n: usize) {
        for _ in 0..2_000 {
            if backend.votes.lock().len() >= n {
                return;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        panic!("{} votes, not {n}", backend.votes.lock().len());
    }

    #[tokio::test]
    async fn a_held_call_asks_the_caller_and_a_yes_approves_it_and_the_turn_goes_on() {
        let backend = Arc::new(Scripted::default());
        let stream = backend.gate(0);
        let agent = agent(&backend, None);
        let (mut rx, _cancel) = start(&agent, 1, "refund my order");
        stream.unbounded_send(created("resp_1")).unwrap();
        stream.unbounded_send(text("Let me do that.")).unwrap();
        let args = r#"{"order_id":"4417","amount":12.5}"#;
        stream
            .unbounded_send(hold("esc_1", "refund_order", args, Some(30.0)))
            .unwrap();
        let (op_name, args, kind, reply) = next_gate(&mut rx).await;
        assert_eq!(
            (op_name.as_str(), kind),
            ("refund_order", HitlKind::Confirm)
        );
        assert_eq!(
            (&args["order_id"], &args["amount"]),
            (&json!("4417"), &json!(12.5))
        );
        // budprompt's deadline, less the vote's way back, is the question's, so it is firm.
        let ms = args["expires_in_ms"].as_u64().expect("a deadline");
        assert!((25_000..=30_000 - VOTE_MARGIN_MS).contains(&ms), "{ms}");
        assert!(
            backend.votes.lock().is_empty(),
            "nothing is voted before the answer"
        );

        reply
            .send(GateDecision::Allow(OpId("op_1".to_owned())))
            .unwrap();
        votes_reach(&backend, 1).await;
        let want = ApprovalVote {
            previous_response_id: "resp_1".to_owned(),
            approval_request_id: "esc_1".to_owned(),
            approve: true,
            reason: None,
        };
        assert_eq!(backend.votes.lock()[0], (want, "bud-caller-key".to_owned()));
        // The same stream goes on: the tool runs, and the answer follows.
        for event in [
            tool_started("refund_order"),
            tool_finished("refund_order", true),
            text("Your refund is on its way."),
            completed(),
        ] {
            stream.unbounded_send(event).unwrap();
        }
        let seen = reports(&mut rx).await;
        assert_eq!(
            seen,
            [
                // The tool shows when it runs, and is reported as used.
                r#"ToolStarted { name: "refund_order", phrase: None, remote: true }"#,
                r#"Committed { label: "used refund order" }"#,
                r#"Sentence { attempt: 0, seq: 0, text: "Your refund is on its way." }"#,
                r#"Finished { text: "Your refund is on its way." }"#,
            ]
        );
        assert_eq!(
            backend.requests.lock().len(),
            1,
            "no run of its own for the vote"
        );
    }

    #[tokio::test]
    async fn a_no_or_no_answer_declines_the_call_and_commits_nothing() {
        for reason in ["declined", "timeout"] {
            let backend = Arc::new(Scripted::default());
            let stream = backend.gate(0);
            let agent = agent(&backend, None);
            let (mut rx, _cancel) = start(&agent, 1, "refund my order");
            stream.unbounded_send(created("resp_1")).unwrap();
            stream
                .unbounded_send(hold("esc_1", "refund_order", "{}", Some(30.0)))
                .unwrap();
            let (_, _, _, reply) = next_gate(&mut rx).await;
            reply.send(GateDecision::Refuse(reason)).unwrap();
            votes_reach(&backend, 1).await;
            let (vote, _) = backend.votes.lock()[0].clone();
            assert_eq!(
                (
                    vote.approve,
                    vote.reason.as_deref(),
                    vote.approval_request_id.as_str()
                ),
                (false, Some(reason), "esc_1")
            );
            stream
                .unbounded_send(text("Okay, I haven't refunded it."))
                .unwrap();
            stream.unbounded_send(completed()).unwrap();
            let seen = reports(&mut rx).await;
            assert!(
                !seen.iter().any(|event| event.starts_with("Committed")),
                "{reason}: {seen:?}"
            );
            assert_eq!(
                seen.last().unwrap(),
                r#"Finished { text: "Okay, I haven't refunded it." }"#
            );
        }
    }

    #[tokio::test]
    async fn a_cancel_while_the_caller_is_asked_declines_the_call_first() {
        let backend = Arc::new(Scripted::default());
        let stream = backend.gate(0);
        let agent = agent(&backend, None);
        let (mut rx, cancel) = start(&agent, 1, "refund my order");
        stream.unbounded_send(created("resp_1")).unwrap();
        stream
            .unbounded_send(hold("esc_1", "refund_order", "{}", None))
            .unwrap();
        let (_, args, _, _reply) = next_gate(&mut rx).await;
        assert!(args.get("expires_in_ms").is_none(), "no deadline given");
        cancel.cancel();
        let seen = reports(&mut rx).await;
        assert_eq!(
            seen,
            ["Cancelled"],
            "the held call is not a tool that started"
        );
        settle().await;
        votes_reach(&backend, 1).await;
        let (vote, bearer) = backend.votes.lock()[0].clone();
        assert_eq!(
            (vote.approve, vote.reason.as_deref(), bearer.as_str()),
            (false, Some("cancelled"), "bud-caller-key")
        );
        for _ in 0..50 {
            tokio::task::yield_now().await;
        }
        assert_eq!(*backend.order.lock(), ["vote", "cancel"]);
    }

    #[tokio::test]
    async fn a_run_failed_for_an_approval_the_caller_cannot_give_is_the_approval_failure() {
        let backend = Arc::new(Scripted::default());
        backend.replies.lock().push_back(Ok(vec![
            created("resp_1"),
            AgentEvent::Failed {
                code: "approval_required".to_owned(),
                message: "a manager must approve refunds".to_owned(),
            },
        ]));
        let agent = agent(&backend, None);
        let (mut rx, _cancel) = start(&agent, 1, "refund my order");
        let error =
            format!(r#"Failed {{ error: "{APPROVAL_REQUIRED}: a manager must approve refunds" }}"#);
        assert_eq!(reports(&mut rx).await, [error]);
    }

    #[test]
    fn the_agents_tool_confirmations_word_its_gates_and_its_approval_message_is_said() {
        let entry = json!({
            "prompt_id": "c0de", "version": 2,
            "stt": {"endpoint_id": "s"}, "tts": {"endpoint_id": "t"},
            "approval_message": "A supervisor has to sign off on that.",
            "tool_confirmations": {
                "refund_order": {"phrase": "refund your order",
                                 "template": "refund order {order_id}", "irreversible": false},
                "create-ticket": {},
            },
        });
        let entry = bud_auth::parse_voice_agent_blob(&entry.to_string()).unwrap();
        let backend = Arc::new(Scripted::default());
        let agent = agent(&backend, None);
        let session = AgentSessionConfig {
            entry: Arc::new(entry),
            ..agent.session.clone()
        };
        let brain = Arc::clone(&backend) as Arc<dyn TurnBackend>;
        let worded = AgentBackground::new(brain, session, SessionCredential::new("k"), None);
        let mut specs = worded.confirmations();
        specs.sort_by(|a, b| a.name.cmp(&b.name));
        let got: Vec<_> = specs
            .iter()
            .map(|spec| {
                (
                    spec.name.as_str(),
                    spec.confirm_phrase.as_deref(),
                    spec.confirm_template.as_deref(),
                    spec.irreversible,
                )
            })
            .collect();
        assert_eq!(
            got,
            [
                // Absent: the name is said, and a strong "yes" is needed.
                ("create-ticket", None, None, true),
                (
                    "refund_order",
                    Some("refund your order"),
                    Some("refund order {order_id}"),
                    false
                ),
            ]
        );
        assert_eq!(
            worded.approval_message().as_deref(),
            Some("A supervisor has to sign off on that.")
        );
        // An agent with none: no wording of its own; its approval message is the default.
        assert!(agent.confirmations().is_empty());
        assert_eq!(
            agent.approval_message().as_deref(),
            Some(bud_auth::voice_agent::DEFAULT_APPROVAL_MESSAGE)
        );
    }

    /// The vote on the wire: a fake Bud gateway streams the turn, holds the call, and goes on
    /// only once the vote came, with the turn's bearer and response id (spec 029 §7.3).
    #[tokio::test]
    async fn the_vote_reaches_the_bud_gateway_and_releases_the_same_stream() {
        use axum::{Router, extract::State, http::HeaderMap, routing::post};
        use bytes::Bytes;

        type Frames = futures::channel::mpsc::UnboundedSender<Result<Bytes, std::io::Error>>;
        #[derive(Clone, Default)]
        struct Fake {
            stream: Arc<Mutex<Option<Frames>>>,
            turns: Arc<Mutex<Vec<Value>>>,
            votes: Arc<Mutex<Vec<(Value, String)>>>,
        }
        fn frame(body: Value) -> Result<Bytes, std::io::Error> {
            let kind = body["type"].as_str().unwrap_or_default().to_owned();
            Ok(Bytes::from(format!("event: {kind}\ndata: {body}\n\n")))
        }
        async fn responses(
            State(fake): State<Fake>,
            headers: HeaderMap,
            body: axum::Json<Value>,
        ) -> axum::response::Response {
            let auth = headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_owned();
            if body["stream"] == true {
                fake.turns.lock().push(body.0);
                let (tx, rx) = futures::channel::mpsc::unbounded();
                for event in [
                    json!({"type": "response.created", "response": {"id": "resp_9"}}),
                    json!({"type": "response.output_item.added", "output_index": 0,
                           "sequence_number": 2,
                           "item": {"type": "mcp_approval_request", "id": "esc_9",
                                    "name": "refund_order",
                                    "arguments": "{\"order_id\":\"4417\"}",
                                    "server_label": "orders",
                                    "bud_expires_at": unix_now() + 30.0}}),
                ] {
                    tx.unbounded_send(frame(event)).unwrap();
                }
                tx.unbounded_send(Ok(Bytes::from_static(b": keep-alive\n\n")))
                    .unwrap();
                *fake.stream.lock() = Some(tx);
                return axum::response::Response::builder()
                    .header("content-type", "text/event-stream")
                    .body(axum::body::Body::from_stream(rx))
                    .unwrap();
            }
            fake.votes.lock().push((body.0, auth));
            let tx = fake.stream.lock().clone().expect("a held turn");
            for event in [
                json!({"type": "response.output_item.added",
                       "item": {"type": "mcp_call", "id": "mcp_1", "name": "refund_order"}}),
                json!({"type": "response.output_item.done",
                       "item": {"type": "mcp_call", "id": "mcp_1", "name": "refund_order",
                                "status": "completed"}}),
                json!({"type": "response.output_text.delta", "delta": "Refunded."}),
                json!({"type": "response.completed", "response": {"output": []}}),
            ] {
                tx.unbounded_send(frame(event)).unwrap();
            }
            axum::Json(json!({"id": "resp_10", "status": "completed"})).into_response()
        }
        use axum::response::IntoResponse;

        let fake = Fake::default();
        let app = Router::new()
            .route("/v1/responses", post(responses))
            .with_state(fake.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let scripted = Arc::new(Scripted::default());
        let session = agent(&scripted, None).session.clone();
        let brain = Arc::new(crate::core::agent::AgentBrain::new(format!(
            "http://{addr}/v1"
        ))) as Arc<dyn TurnBackend>;
        let credential = SessionCredential::new("bud-caller-key");
        let agent = AgentBackground::new(brain, session, credential, None);
        let (mut rx, _cancel) = start(&agent, 1, "refund order 4417");
        let (op_name, args, _, reply) = next_gate(&mut rx).await;
        assert_eq!(
            (op_name.as_str(), &args["order_id"]),
            ("refund_order", &json!("4417"))
        );
        assert!(fake.votes.lock().is_empty());
        reply
            .send(GateDecision::Allow(OpId("op_1".to_owned())))
            .unwrap();
        let seen = reports(&mut rx).await;
        assert_eq!(
            seen,
            [
                // The tool shows when it runs, and is reported as used.
                r#"ToolStarted { name: "refund_order", phrase: None, remote: true }"#,
                r#"Committed { label: "used refund order" }"#,
                r#"Sentence { attempt: 0, seq: 0, text: "Refunded." }"#,
                r#"Finished { text: "Refunded." }"#,
            ]
        );
        let votes = fake.votes.lock().clone();
        assert_eq!(
            votes,
            [(
                json!({"prompt": {"id": "support"}, "previous_response_id": "resp_9", "stream": false,
                       "input": [{"type": "mcp_approval_response",
                                  "approval_request_id": "esc_9", "approve": true}]}),
                "Bearer bud-caller-key".to_owned()
            )]
        );
        assert_eq!(fake.turns.lock().len(), 1);
    }
}
