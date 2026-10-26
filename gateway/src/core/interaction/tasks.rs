//! The task state machine of one session. The process-wide store behind `GET /v1/tasks` is
//! [`super::store`].
//!
//! A task is one delegated request. It owns at most one live background run; a revision
//! replaces the run. A side-effecting tool call of a run arrives as a gate: the registry asks
//! the caller (one question open per session, the rest wait in order) and answers the gate
//! itself on the run's `oneshot`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::LazyLock;

use parking_lot::Mutex;
use tokio::sync::oneshot;

use super::config::{
    BackendSpec, InteractionConfig, SessionEnd, SideEffects, TaskLimits, ToolSpec,
};
use super::persona::Persona;
use super::rules;
pub use super::store::{Published, Session, Stored, TaskStore, back_id, store};
use super::types::{
    AnsweredBy, Board, Dispatched, Effects, GateDecision, HitlId, HitlKind, HitlOutcome,
    HitlRequest, Lineage, Ms, OpId, Outcome, RunEvent, RunId, StopReason, TaskCmd, TaskEffect,
    TaskEvent, TaskEventKind, TaskId, TaskOutput, TaskRef, TaskState, TaskView, TurnId,
};

/// A Bud agent's runs share one conversation, which takes one run at a time.
const MAX_ACTIVE_AGENT: usize = 1;
/// How often a request queued only by its tenant's limit looks for a free place.
const TENANT_POLL_MS: Ms = 250;
/// A provisional run is adopted by E + 600 ms or discarded.
const PROVISIONAL_DEADLINE_MS: Ms = 600;
/// A run started at an eager end of turn is discarded if E has not come by then.
const EAGER_CEILING_MS: Ms = 1_500;
/// How long a cancel waits for an operation in flight before it reports `unknown` effects.
const CANCEL_WAIT_MS: Ms = 10_000;
/// How long an operation in flight may outlive its session.
const SESSION_END_WAIT_MS: Ms = 30_000;
/// The title of a task: the start of the caller's words.
const TITLE_MAX_CHARS: usize = 60;
/// The ended requests a session keeps (with their children) for follow-ups and its board;
/// an older one is forgotten (the task store keeps its last state).
const ENDED_KEPT: usize = 32;

/// What a held tool call is told when its question closes without an accept.
fn refusal(outcome: &HitlOutcome) -> &'static str {
    match outcome {
        HitlOutcome::Accept { .. } | HitlOutcome::Decline { .. } => "declined",
        HitlOutcome::Cancel => "cancelled",
        HitlOutcome::Timeout => "timeout",
        HitlOutcome::Superseded => "stale",
    }
}

fn refuse(event: RunEvent, reason: &'static str) {
    match event {
        RunEvent::Gate { reply, .. } => {
            let _ = reply.send(GateDecision::Refuse(reason));
        }
        RunEvent::Dispatch { reply, .. } => {
            let _ = reply.send(Err(reason.to_owned()));
        }
        RunEvent::Control { reply, .. } => {
            let refused = serde_json::json!({ "outcome": "refused", "detail": reason });
            let _ = reply.send(refused);
        }
        _ => {}
    }
}

/// The caller's requests that run now, per tenant, in this process (`max_live_tenant`).
static TENANT_LIVE: LazyLock<Mutex<HashMap<String, usize>>> = LazyLock::new(Mutex::default);

fn tenant_live(owner: &str) -> usize {
    TENANT_LIVE.lock().get(owner).copied().unwrap_or(0)
}

fn tenant_change(owner: &str, held: bool) {
    let mut live = TENANT_LIVE.lock();
    let count = live.entry(owner.to_owned()).or_default();
    if held {
        *count += 1;
    } else {
        *count = count.saturating_sub(1);
        if *count == 0 {
            live.remove(owner);
        }
    }
}

/// A catalog entry a session's tasks run on, as the registry needs it.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentSpec {
    /// `background` of `task_created`: `<type>:<name>`.
    pub label: String,
    pub side_effects: bool,
    pub max_concurrent: Option<usize>,
    pub question_deadline_ms: Option<Ms>,
    /// A follow-up may run on it directly, with the caller's words. Not on an MCP server's
    /// tool (it takes arguments, not words) nor on an agent whose calls the caller confirms
    /// (the orchestrator's gate would be passed by): those follow-ups go to the orchestrator.
    pub direct_follow_up: bool,
}

impl AgentSpec {
    pub fn of(spec: &BackendSpec) -> Self {
        Self {
            label: spec.label(),
            side_effects: spec.side_effects == SideEffects::Possible,
            max_concurrent: spec.max_concurrent.map(|n| n as usize),
            question_deadline_ms: spec.question_deadline_ms,
            direct_follow_up: true,
        }
    }
}

/// The first sentence of a result: what a follow-up whose handle expired is told of it.
fn first_sentence(text: &str) -> &str {
    let text = text.trim();
    let end = text
        .char_indices()
        .find(|(at, c)| matches!(c, '.' | '!' | '?') && text[at + 1..].starts_with(' '))
        .map_or(text.len(), |(at, _)| at + 1);
    &text[..end]
}

fn title(text: &str) -> String {
    let mut title = String::new();
    for word in text.split_whitespace() {
        if !title.is_empty() && title.chars().count() + 1 + word.chars().count() > TITLE_MAX_CHARS {
            break;
        }
        if !title.is_empty() {
            title.push(' ');
        }
        title.extend(word.chars().take(TITLE_MAX_CHARS));
    }
    title.trim_end_matches(['.', ',', '?', '!']).to_owned()
}

/// A run started before its turn was settled. Nothing of it is visible until it is adopted.
struct Provisional {
    run: RunId,
    turn: TurnId,
    e_at: Option<Ms>,
    deadline: Ms,
    /// Everything the run reported, a held gate included, in order.
    events: Vec<RunEvent>,
    /// The trace its requests carry: the task's when it is adopted.
    trace_id: String,
}

/// One gated tool call under one revision.
struct Op {
    id: OpId,
    revision: u32,
    name: String,
    args: serde_json::Value,
    /// `None` while the caller is being asked.
    allowed: Option<bool>,
}

/// A gate waiting for its turn to be asked.
struct Held {
    task: usize,
    op: Option<OpId>,
    op_name: String,
    args: serde_json::Value,
    kind: HitlKind,
    reply: oneshot::Sender<GateDecision>,
}

/// The one question open in the session.
struct Question {
    task: usize,
    request: HitlRequest,
    reply: oneshot::Sender<GateDecision>,
}

struct Task {
    view: TaskView,
    /// The view last handed out as `TaskEffect::Store`.
    stored: Option<TaskView>,
    /// The caller's words: the request, then each correction.
    words: Vec<String>,
    /// The live run; `None` while queued and once the task is over.
    run: Option<RunId>,
    seq: u64,
    /// E of the request, or of the latest correction, when the registry was told it; else the
    /// time of that command.
    e_at: Ms,
    /// The next sentence is the first since `e_at`.
    first_due: bool,
    /// A run counts its attempts from 0; this is added to make them unique in the task.
    attempt_base: u32,
    attempts_used: u32,
    /// The attempt of the latest sentence and how many sentences it has released.
    attempt: u32,
    released: u32,
    ops: Vec<Op>,
    /// What the task's tool calls reported as committed.
    labels: Vec<String>,
    /// The backend said it does not report what it changes: the effects stay unknown.
    unseen: bool,
    /// An allowed tool call has not reported back.
    in_flight: bool,
    /// A run that was replaced while its allowed tool call was on the wire, and the revision
    /// it ran under: the call is not abandoned, so what it did is still to come.
    replaced: Option<(RunId, u32)>,
    questions: u32,
    cancel_by: &'static str,
    /// When the wait for an operation in flight ends (a cancel, or the end of the session),
    /// or, after the session ended, when the task is stopped.
    deadline: Option<Ms>,
    /// The session is over and the task runs on until then (`tasks.session_end`).
    orphan_until: Option<Ms>,
    /// The task an orchestrator run started it for, and who hears how it ended.
    parent: Option<usize>,
    done: Option<oneshot::Sender<Outcome>>,
    /// A caller's request that holds one of its tenant's places.
    tenant_held: bool,
    /// What the backend returned besides its answer (`task_output`).
    output: Option<TaskOutput>,
    /// What continues the task on its backend (`RunEvent::Handle`), its answer once it
    /// completed, and when it ended: a follow-up reads them (chapter 8).
    handle: Option<String>,
    result: Option<String>,
    ended_at: Option<Ms>,
    /// A follow-up of a finished task: it runs on that task's agent, and its answer is said.
    follow_up: bool,
}

impl Task {
    /// Started and not over: it holds a place.
    fn holds(&self) -> bool {
        self.view.state.is_live() && self.view.state != TaskState::Submitted
    }
}

/// The tasks, runs and open question of one session.
pub struct Registry {
    /// What runs the tasks: `llm`, `agent:<name>` or `dag`.
    backend: String,
    /// The caller's requests the backend runs at once (a Bud agent's or a DAG's one).
    backend_max: Option<usize>,
    limits: TaskLimits,
    /// The catalog entry that runs the caller's requests, when one does.
    entry: Option<String>,
    /// The catalog entries an orchestrator run may start tasks on (with their fallbacks), and
    /// the session's own entry.
    agents: HashMap<String, AgentSpec>,
    /// The tenant (`handlers::tasks::owner`), for `max_live_tenant`.
    owner: String,
    /// The latest time the registry was told.
    clock: Ms,
    /// The backend runs tools of its own that may change something.
    remote_effects: bool,
    tools: Vec<ToolSpec>,
    answer_timeout_ms: Ms,
    /// The value categories that are never taken by voice: a question for one is sensitive.
    sensitive: Vec<String>,
    tasks: Vec<Task>,
    /// Every run ever started for a task, the replaced ones included.
    runs: HashMap<RunId, usize>,
    provisional: Option<Provisional>,
    question: Option<Question>,
    waiting: VecDeque<Held>,
    /// Caller turns that already caused a task.
    turns: HashSet<TurnId>,
    next_run: u64,
    next_ref: u32,
    ended: bool,
    /// Which live tasks run on after the session (`tasks.session_end`), and for how long.
    session_end: SessionEnd,
    orphan_max_ms: Ms,
    /// How long a finished task's continuation handle is used (`tasks.follow_up_ttl_s`).
    follow_up_ttl_ms: Ms,
}

impl Registry {
    pub fn new(cfg: &InteractionConfig) -> Self {
        Self {
            backend: "llm".to_owned(),
            backend_max: None,
            limits: cfg.tasks.limits,
            entry: None,
            agents: HashMap::new(),
            owner: String::new(),
            clock: 0,
            remote_effects: false,
            tools: cfg.tasks.tools.clone(),
            answer_timeout_ms: Ms::from(cfg.policy.hitl_answer_timeout_ms),
            sensitive: Vec::new(),
            tasks: Vec::new(),
            runs: HashMap::new(),
            provisional: None,
            question: None,
            waiting: VecDeque::new(),
            turns: HashSet::new(),
            next_run: 1,
            next_ref: 0,
            ended: false,
            session_end: cfg.tasks.session_end,
            orphan_max_ms: Ms::from(cfg.tasks.orphan_max_s) * 1000,
            follow_up_ttl_ms: Ms::from(cfg.tasks.follow_up_ttl_s) * 1000,
        }
    }

    /// The session's background is the Bud agent `name`: one run at a time, and no tools of
    /// the gateway's. `side_effects`: the agent has tools, so a task may change something.
    pub fn set_agent(&mut self, name: &str, side_effects: bool) {
        self.set_backend(
            format!("agent:{name}"),
            Some(side_effects),
            Some(MAX_ACTIVE_AGENT),
        );
    }

    /// What runs the session's tasks ([`Backend::label`], [`Backend::own_tools`],
    /// [`Backend::max_concurrent`]). A backend with tools of its own (a Bud agent, a DAG, a
    /// webhook) runs none of the gateway's tools; its `side_effects` say whether a task may
    /// change something outside the gateway.
    ///
    /// [`Backend::label`]: super::background::Backend::label
    /// [`Backend::own_tools`]: super::background::Backend::own_tools
    /// [`Backend::max_concurrent`]: super::background::Backend::max_concurrent
    pub fn set_backend(&mut self, label: String, own_tools: Option<bool>, max: Option<usize>) {
        self.backend = label;
        self.backend_max = max;
        if let Some(side_effects) = own_tools {
            self.remote_effects = side_effects;
            self.tools.clear();
        }
    }

    /// The catalog entries of the session: `entry` runs the caller's requests (a named
    /// backend), `agents` take an orchestrator's child tasks.
    pub fn set_catalog(&mut self, entry: Option<String>, agents: HashMap<String, AgentSpec>) {
        self.entry = entry;
        self.agents = agents;
    }

    /// The tenant whose `max_live_tenant` the caller's requests count against.
    pub fn set_owner(&mut self, owner: &str) {
        owner.clone_into(&mut self.owner);
    }

    /// Earlier connections of this session used the task refs up to `t<used>`: a ref is never
    /// used twice in a session.
    /// The persona's sensitive value categories (`Persona::sensitive_categories`): a question
    /// that names one is sensitive, and its answer never reaches the client.
    pub fn set_sensitive(&mut self, categories: Vec<String>) {
        self.sensitive = categories;
    }

    /// The provisional run waits for its adoption until `deadline` at least: a provider's
    /// delegate call is adopted when the provider's turn ends (chapter 6, `adopt_by`).
    pub fn hold_provisional(&mut self, deadline: Ms) {
        if let Some(provisional) = &mut self.provisional {
            provisional.deadline = provisional.deadline.max(deadline);
        }
    }

    pub fn skip_refs(&mut self, used: u32) {
        self.next_ref = self.next_ref.max(used);
    }

    pub fn apply(&mut self, now: Ms, cmd: TaskCmd) -> Vec<TaskEffect> {
        let mut out = Vec::new();
        self.clock = self.clock.max(now);
        match cmd {
            TaskCmd::StartProvisional { turn, text, e_at } => {
                self.discard(&mut out);
                if self.room(None) && !self.ended && !self.turns.contains(&turn) {
                    let run = self.new_run();
                    let trace_id = Lineage::root(None).trace_id;
                    self.provisional = Some(Provisional {
                        run,
                        turn,
                        e_at,
                        deadline: match e_at {
                            Some(e_at) => e_at + PROVISIONAL_DEADLINE_MS,
                            None => now + EAGER_CEILING_MS,
                        },
                        events: Vec::new(),
                        trace_id: trace_id.clone(),
                    });
                    out.push(TaskEffect::StartRun {
                        run,
                        task: None,
                        words: vec![text],
                        agent: None,
                        trace_id,
                        follow_up: false,
                        handle: None,
                    });
                }
            }
            TaskCmd::ConfirmProvisional { e_at } => {
                if let Some(provisional) = &mut self.provisional {
                    provisional.e_at = Some(e_at);
                    provisional.deadline = e_at + PROVISIONAL_DEADLINE_MS;
                }
            }
            TaskCmd::DiscardProvisional { .. } => self.discard(&mut out),
            TaskCmd::Delegate { turn, text, adopt } => {
                self.delegate(now, turn, text, adopt, &mut out)
            }
            TaskCmd::Revise { task, correction } => {
                self.revise(now, &task, correction, false, &mut out)
            }
            TaskCmd::Continue { task, text } => self.revise(now, &task, text, true, &mut out),
            TaskCmd::FollowUp { turn, task, text } => {
                self.follow_up(now, turn, &task, text, &mut out)
            }
            TaskCmd::Cancel { task, by } => {
                if let Some(idx) = self.find(&task) {
                    let answered = if by == "client" {
                        AnsweredBy::Client
                    } else {
                        AnsweredBy::Voice
                    };
                    self.cancel(now, idx, by, answered, &mut out);
                }
            }
            TaskCmd::Answer {
                request,
                outcome,
                by,
            } => {
                // Only the open question can be answered; a late or unknown id changes nothing.
                let open = self.question.as_ref().filter(|q| q.request.id == request);
                if let Some(idx) = open.map(|question| question.task) {
                    let cancel = outcome == HitlOutcome::Cancel;
                    self.resolve(now, outcome, by, &mut out);
                    if cancel {
                        let who = if by == AnsweredBy::Client {
                            "client"
                        } else {
                            "user"
                        };
                        self.cancel(now, idx, who, by, &mut out);
                    }
                }
            }
            TaskCmd::Run { run, event } => self.on_run(now, run, event, &mut out),
            TaskCmd::SessionEnded { push, grace_ms } => {
                self.end_session(now, push, grace_ms, &mut out)
            }
            TaskCmd::Tick => {
                self.tick(now, &mut out);
                // A place may have come free in another session of the tenant.
                self.start_queued(now, &mut out);
            }
        }
        for task in &mut self.tasks {
            if task.stored.as_ref() != Some(&task.view) {
                task.stored = Some(task.view.clone());
                out.push(TaskEffect::Store(task.view.clone()));
            }
        }
        self.forget_ended();
        out
    }

    /// Forgets the oldest ended requests beyond [`ENDED_KEPT`], each with its children once
    /// all of them are over: a long session's tasks stay bounded.
    fn forget_ended(&mut self) {
        if self.tasks.len() <= ENDED_KEPT {
            return;
        }
        let over = |task: &Task| {
            task.view.state.is_terminal()
                && task.deadline.is_none()
                && !task.in_flight
                && task.replaced.is_none()
        };
        let ended = |idx: usize| {
            self.tasks[idx].parent.is_none()
                && self
                    .tasks
                    .iter()
                    .enumerate()
                    .all(|(at, task)| (at != idx && task.parent != Some(idx)) || over(task))
        };
        let families: Vec<usize> = (0..self.tasks.len()).filter(|&idx| ended(idx)).collect();
        let Some(excess) = families.len().checked_sub(ENDED_KEPT).filter(|n| *n > 0) else {
            return;
        };
        let gone: HashSet<usize> = families[..excess].iter().copied().collect();
        let forgotten = |idx: usize, task: &Task| {
            gone.contains(&idx) || task.parent.is_some_and(|parent| gone.contains(&parent))
        };
        let mut moved = vec![None; self.tasks.len()];
        let mut next = 0;
        for (idx, task) in self.tasks.iter().enumerate() {
            if !forgotten(idx, task) {
                moved[idx] = Some(next);
                next += 1;
            }
        }
        let mut idx = 0;
        self.tasks.retain(|_| {
            idx += 1;
            moved[idx - 1].is_some()
        });
        for task in &mut self.tasks {
            task.parent = task.parent.and_then(|parent| moved[parent]);
        }
        self.runs.retain(|_, idx| match moved[*idx] {
            Some(to) => {
                *idx = to;
                true
            }
            None => false,
        });
        // An open or waiting question belongs to a live task, which is kept.
        for at in (self.question.iter_mut().map(|q| &mut q.task))
            .chain(self.waiting.iter_mut().map(|held| &mut held.task))
        {
            *at = moved[*at].unwrap_or(*at);
        }
    }

    pub fn board(&self) -> Board {
        Board {
            tasks: self.tasks.iter().map(|task| task.view.clone()).collect(),
            open_question: self.question.as_ref().map(|q| q.request.clone()),
            provisional: self.provisional.as_ref().map(|provisional| provisional.run),
        }
    }

    /// The task's title.
    pub fn title(&self, task: &TaskId) -> Option<String> {
        let task = self.tasks.iter().find(|known| known.view.id == *task)?;
        Some(task.view.title.clone())
    }

    /// What the task's backend returned besides its answer, when it returned anything.
    pub fn output(&self, task: &TaskId) -> Option<&TaskOutput> {
        let task = self.tasks.iter().find(|known| known.view.id == *task)?;
        task.output.as_ref()
    }

    /// What the caller asked for in each of their requests that is still live.
    pub fn live_requests(&self) -> Vec<&str> {
        self.tasks
            .iter()
            .filter(|task| task.parent.is_none() && task.view.state.is_live())
            .filter_map(|task| task.words.first().map(String::as_str))
            .collect()
    }

    /// The next time `apply(now, TaskCmd::Tick)` has something to do.
    pub fn next_wakeup(&self) -> Option<Ms> {
        let provisional = self
            .provisional
            .as_ref()
            .map(|provisional| provisional.deadline);
        let waits = self.tasks.iter().filter_map(|task| task.deadline);
        // A request queued by its tenant's limit looks again for a free place: it frees up
        // in another session.
        let queued = |task: &Task| task.parent.is_none() && task.view.state == TaskState::Submitted;
        let tenant = (self.limits.max_live_tenant.is_some() && self.tasks.iter().any(queued))
            .then_some(self.clock + TENANT_POLL_MS);
        waits.chain(provisional).chain(tenant).min()
    }

    fn find(&self, task_ref: &TaskRef) -> Option<usize> {
        self.tasks
            .iter()
            .position(|task| &task.view.task_ref == task_ref)
    }

    /// Whether one more task may start: a caller's request (`None`) within the session's,
    /// the backend's and the tenant's limits; a child within its agent's `max_concurrent`.
    fn room(&self, agent: Option<&str>) -> bool {
        let below = |max: Option<usize>, live: usize| max.is_none_or(|max| live < max);
        let held = |agent: Option<&str>| {
            let on = |task: &&Task| {
                task.holds()
                    && task.parent.is_some() == agent.is_some()
                    && (agent.is_none() || task.view.lineage.agent.as_deref() == agent)
            };
            self.tasks.iter().filter(on).count()
        };
        match agent {
            Some(agent) => {
                let max = self.agents.get(agent).and_then(|spec| spec.max_concurrent);
                below(max, held(Some(agent)))
            }
            None => {
                let live = held(None);
                let tenant = self.limits.max_live_tenant.map(|max| max as usize);
                below(self.limits.max_live.map(|max| max as usize), live)
                    && below(self.backend_max, live)
                    && below(tenant, tenant_live(&self.owner))
            }
        }
    }

    fn new_run(&mut self) -> RunId {
        let run = RunId(self.next_run);
        self.next_run += 1;
        run
    }

    fn emit(&mut self, idx: usize, now: Ms, kind: TaskEventKind, out: &mut Vec<TaskEffect>) {
        let task = &mut self.tasks[idx];
        task.seq += 1;
        out.push(TaskEffect::Emit(TaskEvent {
            task: task.view.id.clone(),
            task_ref: task.view.task_ref.clone(),
            seq: task.seq,
            revision: task.view.revision,
            at: now,
            state: task.view.state,
            cancelling: task.view.cancelling,
            lineage: Box::new(task.view.lineage.clone()),
            kind,
        }));
    }

    /// Drops the provisional run: its held gate is refused and nothing of it was ever shown.
    fn discard(&mut self, out: &mut Vec<TaskEffect>) {
        if let Some(provisional) = self.provisional.take() {
            for event in provisional.events {
                refuse(event, "discarded");
            }
            out.push(TaskEffect::CancelRun {
                run: provisional.run,
            });
        }
    }

    fn delegate(
        &mut self,
        now: Ms,
        turn: TurnId,
        text: String,
        adopt: bool,
        out: &mut Vec<TaskEffect>,
    ) {
        // Only this turn's own run can be adopted.
        if !(adopt && self.provisional.as_ref().is_some_and(|p| p.turn == turn)) {
            self.discard(out);
        }
        // One utterance, one task.
        if self.ended || !self.turns.insert(turn) {
            self.discard(out);
            return;
        }
        // Work beyond a limit waits; nothing is refused for count.
        let start = self.room(None);
        if !start {
            self.discard(out);
        }
        let provisional = self.provisional.take();
        // An orchestrator's request may change what its agents may change.
        let side_effects = self.remote_effects
            || self.tools.iter().any(|tool| tool.side_effects)
            || self
                .agents
                .iter()
                .any(|(name, agent)| agent.side_effects && self.entry.as_ref() != Some(name));
        let mut lineage = Lineage::root(self.entry.clone());
        if let Some(provisional) = &provisional {
            // The adopted run's requests already carry its trace.
            lineage.trace_id.clone_from(&provisional.trace_id);
        }
        let idx = self.push_task(now, text, side_effects, start, lineage, None);
        if let Some(e_at) = provisional.as_ref().and_then(|p| p.e_at) {
            self.tasks[idx].e_at = e_at;
        }
        let created = TaskEventKind::Created {
            title: self.tasks[idx].view.title.clone(),
            side_effects,
            speculative: provisional.is_some(),
            backend: self.backend.clone(),
        };
        self.emit(idx, now, created, out);
        match provisional {
            // Adopted: the run goes on, and what it reported so far is replayed in order.
            Some(provisional) => {
                self.hold_tenant(idx);
                self.tasks[idx].run = Some(provisional.run);
                self.runs.insert(provisional.run, idx);
                for event in provisional.events {
                    self.on_run(now, provisional.run, event, out);
                }
            }
            None if start => self.start_run(idx, out),
            None => {}
        }
    }

    /// A child task of `parent`'s orchestrator run on the catalog entry `agent`: it hears
    /// only `request`, carries its parent's trace, and how it ends goes to `done`.
    #[allow(clippy::too_many_arguments)]
    fn dispatch(
        &mut self,
        now: Ms,
        parent: usize,
        agent: String,
        request: String,
        reply: oneshot::Sender<Result<Dispatched, String>>,
        done: oneshot::Sender<Outcome>,
        out: &mut Vec<TaskEffect>,
    ) {
        let Some(spec) = self.agents.get(&agent).cloned() else {
            let _ = reply.send(Err(format!("{agent} is not an agent of this session")));
            return;
        };
        let start = self.room(Some(&agent));
        let lineage = Lineage {
            parent_id: Some(self.tasks[parent].view.id.clone()),
            agent: Some(agent),
            trace_id: self.tasks[parent].view.lineage.trace_id.clone(),
            follows: None,
        };
        let side_effects = spec.side_effects;
        let child = (parent, done);
        let idx = self.push_task(now, request, side_effects, start, lineage, Some(child));
        let created = TaskEventKind::Created {
            title: self.tasks[idx].view.title.clone(),
            side_effects,
            speculative: false,
            backend: spec.label,
        };
        self.emit(idx, now, created, out);
        let task_ref = self.tasks[idx].view.task_ref.clone();
        let _ = reply.send(Ok(Dispatched {
            task_ref,
            queued: !start,
        }));
        if start {
            self.start_run(idx, out);
        }
    }

    /// More work about the finished task `task`: a caller's request of its own on the agent
    /// that did it. While the task's handle lasts (`tasks.follow_up_ttl_s` after it ended) the
    /// run continues it; after that the request carries a line of the old result instead. A
    /// task that ran on no catalog entry, or is not over, gets an ordinary request.
    fn follow_up(
        &mut self,
        now: Ms,
        turn: TurnId,
        task: &TaskRef,
        text: String,
        out: &mut Vec<TaskEffect>,
    ) {
        let over = |idx: &usize| self.tasks[*idx].view.state.is_terminal();
        let followed = self.find(task).filter(over);
        let agent = followed.and_then(|idx| self.tasks[idx].view.lineage.agent.clone());
        let direct = |agent: &String| {
            self.agents
                .get(agent)
                .is_none_or(|spec| spec.direct_follow_up)
        };
        let agent = agent.filter(direct);
        let (Some(followed), Some(agent)) = (followed, agent) else {
            return self.delegate(now, turn, text, false, out);
        };
        self.discard(out);
        // One utterance, one task.
        if self.ended || !self.turns.insert(turn) {
            return;
        }
        let followed = &self.tasks[followed];
        let fresh = followed.handle.is_some()
            && followed
                .ended_at
                .is_some_and(|at| now < at + self.follow_up_ttl_ms);
        let named = title(&text);
        let words = match followed.result.as_deref().map(first_sentence) {
            Some(result) if !fresh && !result.is_empty() => {
                format!(
                    "{text}\n(Earlier, \"{}\" came to: {result})",
                    followed.view.title
                )
            }
            _ => text,
        };
        let spec = self.agents.get(&agent);
        let side_effects = spec.map_or(followed.view.side_effects, |spec| spec.side_effects);
        let backend = spec.map_or_else(|| self.backend.clone(), |spec| spec.label.clone());
        let mut lineage = Lineage::root(Some(agent));
        lineage.follows = Some(followed.view.id.clone());
        let side_effects = side_effects || followed.view.side_effects;
        let start = self.room(None);
        let idx = self.push_task(now, words, side_effects, start, lineage, None);
        self.tasks[idx].follow_up = true;
        self.tasks[idx].view.title = named;
        let created = TaskEventKind::Created {
            title: self.tasks[idx].view.title.clone(),
            side_effects,
            speculative: false,
            backend,
        };
        self.emit(idx, now, created, out);
        if start {
            self.start_run(idx, out);
        }
    }

    /// A new task, working or queued; `child`: its parent and who hears how it ends.
    fn push_task(
        &mut self,
        now: Ms,
        text: String,
        side_effects: bool,
        start: bool,
        lineage: Lineage,
        child: Option<(usize, oneshot::Sender<Outcome>)>,
    ) -> usize {
        self.next_ref += 1;
        let (parent, done) = child.unzip();
        self.tasks.push(Task {
            view: TaskView {
                id: TaskId(format!("task_{}", uuid::Uuid::new_v4().simple())),
                task_ref: TaskRef(format!("t{}", self.next_ref)),
                revision: 1,
                title: title(&text),
                state: if start {
                    TaskState::Working
                } else {
                    TaskState::Submitted
                },
                cancelling: false,
                effects: Effects::NoneCommitted,
                open_question: None,
                side_effects,
                started_at: now,
                lineage,
            },
            stored: None,
            words: vec![text],
            run: None,
            seq: 0,
            e_at: now,
            first_due: true,
            attempt_base: 0,
            attempts_used: 0,
            attempt: 0,
            released: 0,
            ops: Vec::new(),
            labels: Vec::new(),
            unseen: false,
            in_flight: false,
            replaced: None,
            questions: 0,
            cancel_by: "user",
            deadline: None,
            orphan_until: None,
            parent,
            done,
            tenant_held: false,
            output: None,
            handle: None,
            result: None,
            ended_at: None,
            follow_up: false,
        });
        self.tasks.len() - 1
    }

    /// A caller's request that starts takes one of its tenant's places until it ends.
    fn hold_tenant(&mut self, idx: usize) {
        let task = &mut self.tasks[idx];
        if task.parent.is_none() && !std::mem::replace(&mut task.tenant_held, true) {
            tenant_change(&self.owner, true);
        }
    }

    fn start_run(&mut self, idx: usize, out: &mut Vec<TaskEffect>) {
        self.hold_tenant(idx);
        let run = self.new_run();
        self.runs.insert(run, idx);
        let task = &mut self.tasks[idx];
        task.run = Some(run);
        task.attempt_base = task.attempts_used;
        let own_agent = task.parent.is_some() || task.follow_up;
        let follows = task.view.lineage.follows.clone();
        // A follow-up continues what it follows while that task's handle lasts.
        let handle = (follows.as_ref())
            .and_then(|follows| self.tasks.iter().find(|known| known.view.id == *follows))
            .filter(|followed| {
                followed
                    .ended_at
                    .is_some_and(|at| self.clock < at + self.follow_up_ttl_ms)
            })
            .and_then(|followed| followed.handle.clone());
        let task = &self.tasks[idx];
        out.push(TaskEffect::StartRun {
            run,
            task: Some(task.view.id.clone()),
            words: task.words.clone(),
            agent: task.view.lineage.agent.clone().filter(|_| own_agent),
            trace_id: task.view.lineage.trace_id.clone(),
            follow_up: task.follow_up,
            handle,
        });
    }

    /// Starts every queued task that has room now, oldest first.
    fn start_queued(&mut self, now: Ms, out: &mut Vec<TaskEffect>) {
        // After the session only a task that runs on starts.
        let ended = self.ended;
        for idx in 0..self.tasks.len() {
            let task = &self.tasks[idx];
            let queued =
                task.view.state == TaskState::Submitted && (!ended || task.orphan_until.is_some());
            let agent = task.parent.and(task.view.lineage.agent.clone());
            if queued && self.room(agent.as_deref()) {
                self.tasks[idx].view.state = TaskState::Working;
                self.start_run(idx, out);
                self.emit(idx, now, TaskEventKind::Working, out);
            }
        }
    }

    /// Cancels the live tasks `idx`'s orchestrator run started: nobody would hear them.
    fn stop_children(&mut self, now: Ms, idx: usize, out: &mut Vec<TaskEffect>) {
        for child in 0..self.tasks.len() {
            let task = &self.tasks[child];
            if task.parent == Some(idx) && task.view.state.is_live() {
                self.cancel(now, child, "parent", AnsweredBy::Voice, out);
            }
        }
    }

    /// A correction of a live task (`more`: the caller's further words of the same request,
    /// which join its last words instead of correcting them).
    fn revise(
        &mut self,
        now: Ms,
        task: &TaskRef,
        correction: String,
        more: bool,
        out: &mut Vec<TaskEffect>,
    ) {
        let Some(idx) = self.find(task) else { return };
        let view = &self.tasks[idx].view;
        if self.ended || view.state.is_terminal() || view.cancelling {
            return;
        }
        self.close_gates(idx, now, HitlOutcome::Superseded, AnsweredBy::Voice, out);
        self.stop_answer(idx, now, StopReason::Revised, out);
        // The revised run starts what it needs again.
        self.stop_children(now, idx, out);
        let task = &mut self.tasks[idx];
        if task.in_flight {
            // The call is on the wire and is not abandoned: its outcome is still owed.
            task.replaced = task.run.map(|run| (run, task.view.revision));
        }
        task.view.revision += 1;
        match task.words.last_mut().filter(|_| more) {
            Some(words) => {
                words.push(' ');
                words.push_str(&correction);
            }
            None => task.words.push(correction),
        }
        task.e_at = now;
        task.first_due = true;
        task.in_flight = false;
        let replaced = task.run.take();
        self.emit(idx, now, TaskEventKind::Revised, out);
        if let Some(run) = replaced {
            out.push(TaskEffect::CancelRun { run });
            self.start_run(idx, out);
        }
    }

    fn cancel(
        &mut self,
        now: Ms,
        idx: usize,
        by: &'static str,
        answered: AnsweredBy,
        out: &mut Vec<TaskEffect>,
    ) {
        let task = &mut self.tasks[idx];
        if task.view.state.is_terminal() || task.view.cancelling {
            return;
        }
        task.view.cancelling = true;
        task.cancel_by = by;
        self.emit(idx, now, TaskEventKind::CancelRequested, out);
        self.close_gates(idx, now, HitlOutcome::Cancel, answered, out);
        self.stop_answer(idx, now, StopReason::Cancelling, out);
        self.stop_children(now, idx, out);
        let task = &mut self.tasks[idx];
        if let Some(run) = task.run {
            out.push(TaskEffect::CancelRun { run });
        }
        if task.in_flight || task.replaced.is_some() {
            // The operation may still commit: the truth is known when the run reports, or not at all.
            task.deadline = Some(now + CANCEL_WAIT_MS);
        } else {
            self.settle_cancelled(idx, now, out);
        }
    }

    fn settle_cancelled(&mut self, idx: usize, now: Ms, out: &mut Vec<TaskEffect>) {
        let task = &self.tasks[idx];
        let cancelled = TaskEventKind::Cancelled {
            by: task.cancel_by,
            effects: task.view.effects.clone(),
        };
        self.settle(idx, now, TaskState::Cancelled, cancelled, out);
    }

    /// Ends a task: its questions closed, the terminal event, the run slot handed on.
    fn settle(
        &mut self,
        idx: usize,
        now: Ms,
        state: TaskState,
        kind: TaskEventKind,
        out: &mut Vec<TaskEffect>,
    ) {
        self.close_gates(idx, now, HitlOutcome::Superseded, AnsweredBy::Timeout, out);
        let task = &mut self.tasks[idx];
        task.view.state = state;
        task.view.cancelling = false;
        task.ended_at = Some(now);
        if let TaskEventKind::Completed { text, .. } = &kind {
            task.result = Some(text.clone());
        }
        task.run = None;
        task.in_flight = false;
        task.deadline = None;
        if std::mem::take(&mut task.tenant_held) {
            tenant_change(&self.owner, false);
        }
        let done = task.done.take();
        self.emit(idx, now, kind, out);
        // The orchestrator run hears how its child ended.
        if let (Some(done), Some(TaskEffect::Emit(event))) = (done, out.last())
            && let Some(outcome) = Outcome::of(event)
        {
            let _ = done.send(outcome);
        }
        // What the session's end decided for each task stands.
        if !self.ended {
            self.stop_children(now, idx, out);
        }
        self.start_queued(now, out);
    }

    /// `AnswerStopped` when sentences of the current attempt were released.
    fn stop_answer(&mut self, idx: usize, now: Ms, reason: StopReason, out: &mut Vec<TaskEffect>) {
        let task = &mut self.tasks[idx];
        let released = std::mem::take(&mut task.released);
        if released > 0 {
            let stopped = TaskEventKind::AnswerStopped {
                attempt: task.attempt,
                released,
                reason,
            };
            self.emit(idx, now, stopped, out);
        }
    }

    /// Closes what the task asks or waits to ask: its open question resolves with `outcome`,
    /// its waiting gates are refused.
    fn close_gates(
        &mut self,
        idx: usize,
        now: Ms,
        outcome: HitlOutcome,
        by: AnsweredBy,
        out: &mut Vec<TaskEffect>,
    ) {
        let (closed, waiting) = std::mem::take(&mut self.waiting)
            .into_iter()
            .partition(|held| held.task == idx);
        self.waiting = waiting;
        for held in closed {
            let _ = held.reply.send(GateDecision::Refuse(refusal(&outcome)));
        }
        if self.question.as_ref().is_some_and(|q| q.task == idx) {
            self.resolve(now, outcome, by, out);
        }
    }

    /// A gated tool call of a task's live run.
    fn gate(&mut self, now: Ms, mut held: Held, out: &mut Vec<TaskEffect>) {
        let task = &mut self.tasks[held.task];
        if held.kind == HitlKind::Confirm {
            let revision = task.view.revision;
            let same = |op: &&Op| {
                op.revision == revision && op.name == held.op_name && op.args == held.args
            };
            if let Some(op) = task.ops.iter().find(same) {
                // The same call again under one revision: the first decision stands.
                let decision = match op.allowed {
                    Some(true) => GateDecision::Allow(op.id.clone()),
                    Some(false) => GateDecision::Refuse("declined"),
                    None => GateDecision::Refuse("stale"),
                };
                let allowed = matches!(decision, GateDecision::Allow(_));
                task.in_flight = held.reply.send(decision).is_ok() && allowed;
                return;
            }
            let n = task.ops.iter().filter(|op| op.revision == revision).count() + 1;
            let id = OpId(format!("op_{}_{revision}_{n}", task.view.id.0));
            task.ops.push(Op {
                id: id.clone(),
                revision,
                name: held.op_name.clone(),
                args: held.args.clone(),
                allowed: None,
            });
            held.op = Some(id);
        }
        self.waiting.push_back(held);
        self.ask_next(now, out);
    }

    /// Opens the next waiting gate as the session's question, if none is open.
    fn ask_next(&mut self, now: Ms, out: &mut Vec<TaskEffect>) {
        if self.question.is_some() {
            return;
        }
        let Some(held) = self.waiting.pop_front() else {
            return;
        };
        // An agent's questions may have less time: the deadline its backend gave the question
        // (`expires_in_ms`), else its entry's `question_deadline_ms`. Either is the job's own,
        // and never moved.
        let agent = self.tasks[held.task].view.lineage.agent.as_ref();
        let given = held
            .args
            .get("expires_in_ms")
            .and_then(serde_json::Value::as_u64);
        let deadline =
            given.or_else(|| agent.and_then(|agent| self.agents.get(agent)?.question_deadline_ms));
        let answer_ms =
            deadline.map_or(self.answer_timeout_ms, |ms| ms.min(self.answer_timeout_ms));
        let task = &mut self.tasks[held.task];
        task.questions += 1;
        let id = HitlId(format!("q_{}_{}", task.view.id.0, task.questions));
        let confirm = held.kind == HitlKind::Confirm;
        let tool = self.tools.iter().find(|tool| tool.name == held.op_name);
        let mut request = HitlRequest {
            id: id.clone(),
            task: task.view.id.clone(),
            revision: task.view.revision,
            kind: held.kind,
            op: held.op,
            verb_phrase: String::new(),
            params: Vec::new(),
            question: None,
            options: Vec::new(),
            // A call the configuration does not know is treated as the strictest kind.
            irreversible: confirm && tool.is_none_or(|tool| tool.irreversible),
            expires_at: now + answer_ms,
            firm: deadline.is_some(),
            ..HitlRequest::default()
        };
        if confirm {
            request.verb_phrase = tool
                .and_then(|tool| tool.confirm_phrase.clone())
                .unwrap_or_else(|| held.op_name.replace('_', " "));
            if let serde_json::Value::Object(args) = held.args {
                request.params = args.into_iter().collect();
                // Read back in the order the tool's schema lists what it requires (the
                // order its author thinks in), anything else after that.
                let required = tool.and_then(|tool| tool.parameters["required"].as_array());
                let rank = |name: &str| required?.iter().position(|field| field == name);
                request
                    .params
                    .sort_by_key(|(name, _)| rank(name).unwrap_or(usize::MAX));
            }
            // The tool's own read-back, its slots filled from the arguments that will run.
            if let Some(template) = tool.and_then(|tool| tool.confirm_template.as_deref()) {
                let mut read = template.to_owned();
                for (name, value) in &request.params {
                    read = read.replace(&format!("{{{name}}}"), &Persona::speak_value(value));
                }
                request.question = (!read.contains('{')).then_some(read);
            }
        } else {
            let text = |value: &serde_json::Value| value.as_str().map(str::to_owned);
            request.question = held.args.get("question").and_then(text);
            if let Some(options) = held.args.get("options").and_then(|value| value.as_array()) {
                request.options = options.iter().filter_map(text).collect();
            }
            let schema = held.args.get("schema").filter(|schema| schema.is_object());
            request.schema = schema.cloned().map(Box::new);
            request.url = held
                .args
                .get("url")
                .and_then(text)
                .map(String::into_boxed_str);
            let named = request
                .question
                .as_deref()
                .and_then(|question| rules::sensitive_category(question, &self.sensitive));
            request.sensitive = held.args.get("sensitive") == Some(&serde_json::Value::Bool(true))
                || held.kind == HitlKind::AuthUrl
                || named.is_some();
        }
        task.view.state = TaskState::InputRequired;
        task.view.open_question = Some(id);
        self.question = Some(Question {
            task: held.task,
            request: request.clone(),
            reply: held.reply,
        });
        self.emit(held.task, now, TaskEventKind::InputRequired(request), out);
    }

    /// Closes the open question and answers its gate.
    fn resolve(
        &mut self,
        now: Ms,
        outcome: HitlOutcome,
        by: AnsweredBy,
        out: &mut Vec<TaskEffect>,
    ) {
        let Some(question) = self.question.take() else {
            return;
        };
        let task = &mut self.tasks[question.task];
        task.view.open_question = None;
        task.view.state = TaskState::Working;
        let decision = match (&outcome, &question.request.op) {
            (HitlOutcome::Accept { .. }, Some(op)) => GateDecision::Allow(op.clone()),
            (HitlOutcome::Accept { value }, None) => {
                GateDecision::Answer(value.clone().unwrap_or_default())
            }
            // The gateway closed it: the value is never taken by voice.
            (_, _) if by == AnsweredBy::Policy => GateDecision::Refuse("not_by_voice"),
            (other, _) => GateDecision::Refuse(refusal(other)),
        };
        let allowed = matches!(decision, GateDecision::Allow(_));
        let op = |op: &&mut Op| Some(&op.id) == question.request.op.as_ref();
        if let Some(op) = task.ops.iter_mut().find(op) {
            op.allowed = Some(allowed);
        }
        task.in_flight = question.reply.send(decision).is_ok() && allowed;
        // A sensitive value goes to the background only, never back to the client.
        let redacted = question.request.sensitive;
        let outcome = match outcome {
            HitlOutcome::Accept { .. } if redacted => HitlOutcome::Accept { value: None },
            outcome => outcome,
        };
        let resolved = TaskEventKind::InputResolved {
            request: question.request.id,
            outcome,
            by,
            redacted,
        };
        self.emit(question.task, now, resolved, out);
        self.ask_next(now, out);
    }

    fn on_run(&mut self, now: Ms, run: RunId, event: RunEvent, out: &mut Vec<TaskEffect>) {
        if let Some(provisional) = self.provisional.as_mut().filter(|p| p.run == run) {
            provisional.events.push(event);
            return;
        }
        let Some(&idx) = self.runs.get(&run) else {
            // A discarded run.
            return refuse(event, "stale");
        };
        let task = &mut self.tasks[idx];
        let live = task.run == Some(run);
        // The run a revision replaced while its call was on the wire, and its revision.
        let stale = task.replaced.filter(|(replaced, _)| *replaced == run);
        if matches!(event, RunEvent::EffectsUnseen) && (live || stale.is_some()) {
            task.unseen = true;
            task.view.effects = Effects::Unknown;
            if !task.view.cancelling {
                self.emit(
                    idx,
                    now,
                    TaskEventKind::EffectsUpdated(Effects::Unknown),
                    out,
                );
            }
        }
        let task = &mut self.tasks[idx];
        if let RunEvent::Committed { label } = &event {
            // Kept even from a run that was replaced: the effect is real.
            task.labels.push(label.clone());
            if !task.unseen {
                task.view.effects = Effects::Committed(task.labels.clone());
            }
            let updated = match stale {
                // What went through before the caller's change, under the revision it was
                // made for.
                Some(_) => TaskEventKind::StaleOutcome {
                    label: label.clone(),
                },
                None => TaskEventKind::EffectsUpdated(task.view.effects.clone()),
            };
            self.emit(idx, now, updated, out);
            if let (Some((_, revision)), Some(TaskEffect::Emit(emitted))) = (stale, out.last_mut())
            {
                emitted.revision = revision;
            }
        }
        if !live && stale.is_none() {
            return refuse(event, "stale");
        }
        if let RunEvent::ToolStarted { remote: true, .. } = &event {
            if live {
                self.tasks[idx].in_flight = true;
            }
        } else if !matches!(event, RunEvent::ToolStarted { .. }) {
            // Tool calls run one at a time, so any later report means the allowed call is over
            // and what it committed is known.
            let ended = self.ended;
            let task = &mut self.tasks[idx];
            if live {
                if ended && std::mem::take(&mut task.in_flight) {
                    // The caller has hung up and the call that was let finish has reported:
                    // the run gets as long again to word its answer, for whoever reads the
                    // result, or runs on as the session's end said.
                    let wait = now + SESSION_END_WAIT_MS;
                    task.deadline = Some(task.orphan_until.map_or(wait, |until| until.max(wait)));
                }
                task.in_flight = false;
            } else {
                task.replaced = None;
            }
            if task.view.effects == Effects::Unknown && !task.unseen {
                task.view.effects = if task.labels.is_empty() {
                    Effects::NoneCommitted
                } else {
                    Effects::Committed(task.labels.clone())
                };
            }
        }
        let task = &self.tasks[idx];
        let waiting = task.in_flight || task.replaced.is_some();
        if task.view.cancelling {
            // Only an operation in flight keeps a cancelled task open.
            if !waiting {
                refuse(event, "cancelled");
                self.settle_cancelled(idx, now, out);
            }
            return;
        }
        if !live {
            return refuse(event, "stale");
        }
        let effects = self.tasks[idx].view.effects.clone();
        match event {
            // A child's answer is its orchestrator's to say.
            RunEvent::Sentence { .. } if self.tasks[idx].parent.is_some() => {}
            RunEvent::Sentence { attempt, seq, text } => {
                let task = &mut self.tasks[idx];
                let attempt = task.attempt_base + attempt;
                if attempt != task.attempt {
                    task.attempt = attempt;
                    task.released = 0;
                }
                task.attempts_used = attempt + 1;
                task.released += 1;
                let since_e = u32::try_from(now.saturating_sub(task.e_at)).unwrap_or(u32::MAX);
                let sentence = TaskEventKind::AnswerSentence {
                    attempt,
                    seq,
                    text,
                    first_after_e_ms: std::mem::take(&mut task.first_due).then_some(since_e),
                };
                self.emit(idx, now, sentence, out);
            }
            RunEvent::MarkerThenTool => {
                self.stop_answer(idx, now, StopReason::MarkerThenToolCall, out)
            }
            RunEvent::ToolStarted { phrase, .. } => {
                self.emit(idx, now, TaskEventKind::Progress { text: phrase }, out)
            }
            RunEvent::Gate {
                op_name,
                args,
                kind,
                reply,
            } if !self.ended => {
                let held = Held {
                    task: idx,
                    op: None,
                    op_name,
                    args,
                    kind,
                    reply,
                };
                self.gate(now, held, out);
            }
            // The session is over: nobody can be asked.
            gate @ RunEvent::Gate { .. } => refuse(gate, "cancelled"),
            RunEvent::Dispatch {
                agent,
                request,
                reply,
                done,
            } if !self.ended => self.dispatch(now, idx, agent, request, reply, done, out),
            dispatch @ RunEvent::Dispatch { .. } => refuse(dispatch, "the call has ended"),
            // The actor applies a control before the registry sees the run's events.
            control @ RunEvent::Control { .. } => refuse(control, "the call has ended"),
            RunEvent::Committed { .. } | RunEvent::EffectsUnseen => {}
            RunEvent::Output(output) => self.tasks[idx].output = Some(*output),
            RunEvent::Handle(handle) => self.tasks[idx].handle = Some(handle),
            // A child's answer is never spoken, so it has nothing to end.
            RunEvent::Answered if self.tasks[idx].parent.is_some() => {}
            RunEvent::Answered => self.emit(idx, now, TaskEventKind::AnswerDone, out),
            RunEvent::Finished { text } => {
                let completed = TaskEventKind::Completed { text, effects };
                self.settle(idx, now, TaskState::Completed, completed, out);
            }
            RunEvent::Failed { error } => {
                self.stop_answer(idx, now, StopReason::RunFailed, out);
                let failed = TaskEventKind::Failed { error, effects };
                self.settle(idx, now, TaskState::Failed, failed, out);
            }
            RunEvent::Cancelled => {
                self.tasks[idx].cancel_by = "system";
                self.settle_cancelled(idx, now, out);
            }
        }
    }

    /// The session is over. A task whose tool call is on the wire finishes that call. Which
    /// other live tasks run on, and until when, is `tasks.session_end`'s: with `auto` a task
    /// that may change something, or whose result has a reader (`push`), runs on for
    /// `orphan_max_s`; one whose only reader is the caller of a lost connection runs on for
    /// the grace (`grace_ms`). A task that runs on can ask nobody: its open question is
    /// closed as unanswered. The rest are cancelled.
    fn end_session(&mut self, now: Ms, push: bool, grace_ms: Ms, out: &mut Vec<TaskEffect>) {
        self.ended = true;
        self.discard(out);
        let orphan_until = now + self.orphan_max_ms;
        for task in &mut self.tasks {
            let until = match self.session_end {
                SessionEnd::CancelAll => None,
                SessionEnd::ContinueAll => Some(orphan_until),
                SessionEnd::Auto if task.view.side_effects || push => Some(orphan_until),
                SessionEnd::Auto if grace_ms > 0 => Some(orphan_until.min(now + grace_ms)),
                SessionEnd::Auto => None,
            };
            let live = task.view.state.is_live() && !task.view.cancelling;
            task.orphan_until = until.filter(|until| live && *until > now);
        }
        for idx in 0..self.tasks.len() {
            let task = &mut self.tasks[idx];
            if task.in_flight {
                // The operation finishes and the run goes on to its answer: the task ends by
                // its run, as `unknown` when the call never reports, or stopped when the
                // answer does not come.
                task.deadline = Some(now + SESSION_END_WAIT_MS);
            } else if let Some(until) = task.orphan_until {
                task.deadline = Some(until);
                self.close_gates(idx, now, HitlOutcome::Timeout, AnsweredBy::SessionEnd, out);
            } else {
                self.cancel(now, idx, "system", AnsweredBy::SessionEnd, out);
            }
        }
    }

    fn tick(&mut self, now: Ms, out: &mut Vec<TaskEffect>) {
        if self.provisional.as_ref().is_some_and(|p| now >= p.deadline) {
            self.discard(out);
        }
        for idx in 0..self.tasks.len() {
            let task = &mut self.tasks[idx];
            if !task.deadline.is_some_and(|deadline| now >= deadline) {
                continue;
            }
            task.deadline = None;
            let reported = !task.in_flight && task.replaced.is_none();
            if self.ended
                && let Some(run) = task.run
            {
                out.push(TaskEffect::CancelRun { run });
            }
            if self.ended && reported {
                // The call reported and the run did not end in time: it is stopped, and the
                // task ends with what it really did.
                task.cancel_by = "system";
                self.settle_cancelled(idx, now, out);
                continue;
            }
            // The operation never reported: what it did is not known.
            let task = &mut self.tasks[idx];
            task.view.effects = Effects::Unknown;
            if self.ended {
                let failed = TaskEventKind::Failed {
                    error: "the session ended before the operation was confirmed".to_owned(),
                    effects: Effects::Unknown,
                };
                self.settle(idx, now, TaskState::Unknown, failed, out);
            } else {
                let unconfirmed = TaskEventKind::CancelUnconfirmed {
                    effects: Effects::Unknown,
                };
                self.emit(idx, now, unconfirmed, out);
            }
        }
    }
}

impl Drop for Registry {
    /// A session that ends with requests still running gives their tenant places back.
    fn drop(&mut self) {
        let held = self.tasks.iter().filter(|task| task.tenant_held).count();
        for _ in 0..held {
            tenant_change(&self.owner, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool(name: &str, side_effects: bool, irreversible: bool) -> ToolSpec {
        ToolSpec {
            name: name.to_owned(),
            description: String::new(),
            parameters: json!({"type": "object"}),
            url: "https://tools.example/hook".to_owned(),
            side_effects,
            irreversible,
            confirm_phrase: side_effects.then(|| "book a table".to_owned()),
            progress_phrase: None,
            signed: true,
            confirm_template: None,
        }
    }

    fn registry() -> Registry {
        let mut cfg = InteractionConfig::default();
        cfg.tasks.tools = vec![tool("book_table", true, true), tool("search", false, false)];
        Registry::new(&cfg)
    }

    fn events(effects: &[TaskEffect]) -> Vec<&TaskEvent> {
        effects
            .iter()
            .filter_map(|effect| match effect {
                TaskEffect::Emit(event) => Some(event),
                _ => None,
            })
            .collect()
    }

    fn kinds(effects: &[TaskEffect]) -> Vec<&TaskEventKind> {
        events(effects)
            .into_iter()
            .map(|event| &event.kind)
            .collect()
    }

    fn started(effects: &[TaskEffect]) -> Vec<RunId> {
        effects
            .iter()
            .filter_map(|effect| match effect {
                TaskEffect::StartRun { run, .. } => Some(*run),
                _ => None,
            })
            .collect()
    }

    fn cancelled_runs(effects: &[TaskEffect]) -> Vec<RunId> {
        effects
            .iter()
            .filter_map(|effect| match effect {
                TaskEffect::CancelRun { run } => Some(*run),
                _ => None,
            })
            .collect()
    }

    fn delegate(registry: &mut Registry, now: Ms, turn: u64) -> (RunId, Vec<TaskEffect>) {
        let effects = registry.apply(
            now,
            TaskCmd::Delegate {
                turn: TurnId(turn),
                text: format!("request {turn}"),
                adopt: false,
            },
        );
        (started(&effects)[0], effects)
    }

    fn run(registry: &mut Registry, now: Ms, run: RunId, event: RunEvent) -> Vec<TaskEffect> {
        registry.apply(now, TaskCmd::Run { run, event })
    }

    fn sentence(seq: u32) -> RunEvent {
        RunEvent::Sentence {
            attempt: 0,
            seq,
            text: format!("Sentence {seq}."),
        }
    }

    /// A `book_table` gate and the receiver its decision arrives on.
    fn gate(party: u64) -> (RunEvent, oneshot::Receiver<GateDecision>) {
        let (reply, decision) = oneshot::channel();
        let event = RunEvent::Gate {
            op_name: "book_table".to_owned(),
            args: json!({"party": party}),
            kind: HitlKind::Confirm,
            reply,
        };
        (event, decision)
    }

    fn accept(registry: &mut Registry, now: Ms) -> Vec<TaskEffect> {
        let request = registry
            .board()
            .open_question
            .expect("a question is open")
            .id;
        registry.apply(
            now,
            TaskCmd::Answer {
                request,
                outcome: HitlOutcome::Accept { value: None },
                by: AnsweredBy::Voice,
            },
        )
    }

    fn t(n: u32) -> TaskRef {
        TaskRef(format!("t{n}"))
    }

    #[test]
    fn the_read_back_follows_the_schemas_required_list() {
        let mut cfg = InteractionConfig::default();
        let mut book = tool("book_table", true, true);
        book.parameters = json!({"type": "object", "required": ["restaurant", "party", "day"]});
        cfg.tasks.tools = vec![book];
        let mut registry = Registry::new(&cfg);
        let (first, _) = delegate(&mut registry, 1_000, 1);
        let (reply, _decision) = oneshot::channel();
        let event = RunEvent::Gate {
            op_name: "book_table".to_owned(),
            args: json!({"day": "Friday", "note": "window", "party": 4, "restaurant": "Rosa's"}),
            kind: HitlKind::Confirm,
            reply,
        };
        run(&mut registry, 1_500, first, event);
        let request = registry.board().open_question.expect("a question is open");
        let names: Vec<&str> = request
            .params
            .iter()
            .map(|(name, _)| name.as_str())
            .collect();
        assert_eq!(names, ["restaurant", "party", "day", "note"]);
    }

    /// A task whose `book_table` call was accepted and has not reported back. The session
    /// ends with `cancel_all`: only the call that is out finishes.
    fn with_operation_in_flight() -> (Registry, RunId) {
        let mut registry = registry();
        registry.session_end = SessionEnd::CancelAll;
        let (first, _) = delegate(&mut registry, 1_000, 1);
        let (event, mut decision) = gate(4);
        run(&mut registry, 1_500, first, event);
        accept(&mut registry, 4_000);
        assert!(matches!(decision.try_recv(), Ok(GateDecision::Allow(_))));
        (registry, first)
    }

    #[test]
    fn a_tools_own_read_back_is_filled_and_a_sensitive_answer_never_reaches_the_client() {
        let mut cfg = InteractionConfig::default();
        let mut booking = tool("book_table", true, true);
        booking.confirm_template = Some("book a table for {party}".to_owned());
        cfg.tasks.tools = vec![booking];
        let mut registry = Registry::new(&cfg);
        registry.set_sensitive(vec!["card number".to_owned()]);
        let (run_id, _) = delegate(&mut registry, 0, 1);
        let (event, _decision) = gate(4);
        let asked = run(&mut registry, 10, run_id, event);
        let Some(TaskEventKind::InputRequired(request)) = kinds(&asked).pop() else {
            panic!("a confirmation: {asked:?}");
        };
        assert_eq!(request.question.as_deref(), Some("book a table for 4"));
        registry.apply(
            20,
            TaskCmd::Answer {
                request: request.id.clone(),
                outcome: HitlOutcome::Decline { correction: None },
                by: AnsweredBy::Client,
            },
        );

        // A value question that names a sensitive category: the answer goes to the run only.
        let (reply, mut decision) = oneshot::channel();
        let ask = RunEvent::Gate {
            op_name: "ask_caller".to_owned(),
            args: json!({ "kind": "value", "question": "What is your card number?" }),
            kind: HitlKind::Value,
            reply,
        };
        let asked = run(&mut registry, 30, run_id, ask);
        let Some(TaskEventKind::InputRequired(request)) = kinds(&asked).pop() else {
            panic!("a question: {asked:?}");
        };
        assert!(request.sensitive);
        let card = json!("4111 1111 1111 1111");
        let answered = registry.apply(
            40,
            TaskCmd::Answer {
                request: request.id.clone(),
                outcome: HitlOutcome::Accept {
                    value: Some(card.clone()),
                },
                by: AnsweredBy::Client,
            },
        );
        assert_eq!(decision.try_recv(), Ok(GateDecision::Answer(card)));
        assert!(
            matches!(
                kinds(&answered)[..],
                [
                    TaskEventKind::InputResolved {
                        outcome: HitlOutcome::Accept { value: None },
                        redacted: true,
                        ..
                    },
                    ..
                ]
            ),
            "{answered:?}"
        );

        // A form and a sign-in link carry their schema and their link.
        let (reply, _decision) = oneshot::channel();
        let form = RunEvent::Gate {
            op_name: "ask_caller".to_owned(),
            args: json!({ "question": "Your address?", "schema": { "type": "object" },
                          "url": "https://id.example.com/x" }),
            kind: HitlKind::AuthUrl,
            reply,
        };
        let asked = run(&mut registry, 50, run_id, form);
        let Some(TaskEventKind::InputRequired(request)) = kinds(&asked).pop() else {
            panic!("a question: {asked:?}");
        };
        assert_eq!(request.url.as_deref(), Some("https://id.example.com/x"));
        assert_eq!(
            request.schema.as_deref(),
            Some(&json!({ "type": "object" }))
        );
        assert!(request.sensitive, "a sign-in link is always sensitive");
    }

    #[test]
    fn a_delegate_starts_a_run_and_relays_its_answer() {
        let mut registry = registry();
        let (first, effects) = delegate(&mut registry, 1_000, 1);
        let created = events(&effects)[0];
        assert!(created.task.0.starts_with("task_") && created.task.0.len() == 37);
        assert_eq!(
            (created.task_ref.clone(), created.seq, created.revision),
            (t(1), 1, 1)
        );
        assert_eq!(created.state, TaskState::Working);
        assert_eq!(
            created.kind,
            TaskEventKind::Created {
                title: "request 1".to_owned(),
                side_effects: true,
                speculative: false,
                backend: "llm".to_owned(),
            }
        );
        assert!(
            effects
                .iter()
                .any(|effect| matches!(effect, TaskEffect::Store(_)))
        );

        let effects = run(&mut registry, 1_400, first, sentence(0));
        assert_eq!(
            kinds(&effects),
            [&TaskEventKind::AnswerSentence {
                attempt: 0,
                seq: 0,
                text: "Sentence 0.".to_owned(),
                first_after_e_ms: Some(400),
            }]
        );
        // A sentence does not change the view, so nothing is stored.
        assert!(
            !effects
                .iter()
                .any(|effect| matches!(effect, TaskEffect::Store(_)))
        );
        let effects = run(&mut registry, 1_600, first, sentence(1));
        assert!(matches!(
            kinds(&effects)[0],
            TaskEventKind::AnswerSentence {
                first_after_e_ms: None,
                ..
            }
        ));

        let finished = RunEvent::Finished {
            text: "Sentence 0. Sentence 1.".to_owned(),
        };
        let effects = run(&mut registry, 1_700, first, finished);
        assert_eq!(
            kinds(&effects),
            [&TaskEventKind::Completed {
                text: "Sentence 0. Sentence 1.".to_owned(),
                effects: Effects::NoneCommitted,
            }]
        );
        assert_eq!(registry.board().tasks[0].state, TaskState::Completed);
        assert_eq!(registry.next_wakeup(), None);
    }

    #[test]
    fn a_provisional_run_is_adopted_without_a_restart_and_replayed_in_order() {
        let mut registry = registry();
        let effects = registry.apply(
            1_000,
            TaskCmd::StartProvisional {
                turn: TurnId(1),
                text: "what is my baggage allowance".to_owned(),
                e_at: Some(1_000),
            },
        );
        let provisional = started(&effects)[0];
        assert!(matches!(
            effects[..],
            [TaskEffect::StartRun { task: None, .. }]
        ));
        assert_eq!(registry.board().provisional, Some(provisional));
        assert_eq!(registry.next_wakeup(), Some(1_600));

        // Invisible: its reports are held.
        assert!(run(&mut registry, 1_200, provisional, sentence(0)).is_empty());
        assert!(run(&mut registry, 1_250, provisional, sentence(1)).is_empty());
        assert!(registry.board().tasks.is_empty());

        let effects = registry.apply(
            1_300,
            TaskCmd::Delegate {
                turn: TurnId(1),
                text: "what is my baggage allowance".to_owned(),
                adopt: true,
            },
        );
        assert!(
            started(&effects).is_empty(),
            "adoption must not start a second run"
        );
        assert!(cancelled_runs(&effects).is_empty());
        let kinds = kinds(&effects);
        assert!(matches!(
            kinds[0],
            TaskEventKind::Created {
                speculative: true,
                ..
            }
        ));
        assert!(matches!(
            kinds[1],
            TaskEventKind::AnswerSentence {
                seq: 0,
                first_after_e_ms: Some(300),
                ..
            }
        ));
        assert!(matches!(
            kinds[2],
            TaskEventKind::AnswerSentence { seq: 1, .. }
        ));
        assert_eq!(registry.board().provisional, None);

        // The same run goes on as the task's run.
        let effects = run(&mut registry, 1_500, provisional, sentence(2));
        assert!(matches!(
            self::kinds(&effects)[0],
            TaskEventKind::AnswerSentence { seq: 2, .. }
        ));
    }

    #[test]
    fn any_other_settle_discards_the_provisional_run() {
        let mut registry = registry();
        let start = |registry: &mut Registry, turn: u64| {
            let effects = registry.apply(
                1_000,
                TaskCmd::StartProvisional {
                    turn: TurnId(turn),
                    text: "hello".to_owned(),
                    e_at: Some(1_000),
                },
            );
            started(&effects)[0]
        };

        let first = start(&mut registry, 1);
        let (event, mut decision) = gate(2);
        run(&mut registry, 1_100, first, event);
        let effects = registry.apply(1_200, TaskCmd::DiscardProvisional { reason: "social" });
        assert_eq!(effects, [TaskEffect::CancelRun { run: first }]);
        assert_eq!(decision.try_recv(), Ok(GateDecision::Refuse("discarded")));
        assert_eq!(registry.board(), Board::default());
        // A late report of the discarded run changes nothing.
        assert!(run(&mut registry, 1_300, first, sentence(0)).is_empty());

        // A delegate that does not adopt starts its own run.
        let second = start(&mut registry, 2);
        let effects = registry.apply(
            1_400,
            TaskCmd::Delegate {
                turn: TurnId(2),
                text: "hello".to_owned(),
                adopt: false,
            },
        );
        assert_eq!(cancelled_runs(&effects), [second]);
        assert_eq!(started(&effects).len(), 1);
        assert!(matches!(
            kinds(&effects)[0],
            TaskEventKind::Created {
                speculative: false,
                ..
            }
        ));

        // A newer provisional run replaces an older one.
        let third = start(&mut registry, 3);
        let effects = registry.apply(
            1_500,
            TaskCmd::StartProvisional {
                turn: TurnId(4),
                text: "again".to_owned(),
                e_at: Some(1_500),
            },
        );
        assert_eq!(cancelled_runs(&effects), [third]);
    }

    #[test]
    fn a_provisional_run_is_discarded_at_e_plus_600() {
        let mut registry = registry();
        let effects = registry.apply(
            1_000,
            TaskCmd::StartProvisional {
                turn: TurnId(1),
                text: "hello".to_owned(),
                e_at: Some(1_000),
            },
        );
        let provisional = started(&effects)[0];
        assert!(registry.apply(1_599, TaskCmd::Tick).is_empty());
        assert_eq!(registry.board().provisional, Some(provisional));
        let effects = registry.apply(1_600, TaskCmd::Tick);
        assert_eq!(effects, [TaskEffect::CancelRun { run: provisional }]);
        assert_eq!(registry.next_wakeup(), None);

        // Adoption comes too late: a fresh run starts.
        let effects = registry.apply(
            1_700,
            TaskCmd::Delegate {
                turn: TurnId(1),
                text: "hello".to_owned(),
                adopt: true,
            },
        );
        assert_eq!(started(&effects).len(), 1);
        assert_ne!(started(&effects)[0], provisional);
    }

    #[test]
    fn an_eager_run_has_a_ceiling_until_e_is_confirmed() {
        let mut registry = registry();
        let eager = TaskCmd::StartProvisional {
            turn: TurnId(1),
            text: "hello".to_owned(),
            e_at: None,
        };
        registry.apply(1_000, eager);
        assert_eq!(registry.next_wakeup(), Some(2_500));
        registry.apply(1_800, TaskCmd::ConfirmProvisional { e_at: 1_800 });
        assert_eq!(registry.next_wakeup(), Some(2_400));

        let mut registry = self::registry();
        let eager = TaskCmd::StartProvisional {
            turn: TurnId(1),
            text: "hello".to_owned(),
            e_at: None,
        };
        let provisional = started(&registry.apply(1_000, eager))[0];
        assert!(registry.apply(2_499, TaskCmd::Tick).is_empty());
        let effects = registry.apply(2_500, TaskCmd::Tick);
        assert_eq!(effects, [TaskEffect::CancelRun { run: provisional }]);
    }

    #[test]
    fn a_gate_is_held_until_adoption_then_bound_to_the_exact_arguments() {
        let mut registry = registry();
        let effects = registry.apply(
            1_000,
            TaskCmd::StartProvisional {
                turn: TurnId(1),
                text: "book a table for four".to_owned(),
                e_at: Some(1_000),
            },
        );
        let provisional = started(&effects)[0];
        let (event, mut decision) = gate(4);
        assert!(run(&mut registry, 1_200, provisional, event).is_empty());
        assert!(
            decision.try_recv().is_err(),
            "the call is held while provisional"
        );
        assert_eq!(registry.board().open_question, None);

        let effects = registry.apply(
            1_300,
            TaskCmd::Delegate {
                turn: TurnId(1),
                text: "book a table for four".to_owned(),
                adopt: true,
            },
        );
        let asked = events(&effects)[1];
        let task = asked.task.0.clone();
        assert_eq!(asked.state, TaskState::InputRequired);
        let TaskEventKind::InputRequired(request) = &asked.kind else {
            panic!("expected a question, got {:?}", asked.kind);
        };
        assert_eq!(request.id, HitlId(format!("q_{task}_1")));
        assert_eq!(request.kind, HitlKind::Confirm);
        assert_eq!(request.op, Some(OpId(format!("op_{task}_1_1"))));
        assert_eq!(request.verb_phrase, "book a table");
        assert_eq!(request.params, [("party".to_owned(), json!(4))]);
        assert!(request.irreversible);
        assert_eq!(request.expires_at, 1_300 + 20_000);
        assert_eq!(registry.board().open_question.as_ref(), Some(request));
        assert!(
            decision.try_recv().is_err(),
            "still held: the caller has not answered"
        );

        let effects = accept(&mut registry, 5_000);
        assert_eq!(
            decision.try_recv(),
            Ok(GateDecision::Allow(OpId(format!("op_{task}_1_1"))))
        );
        let resolved = events(&effects)[0];
        assert_eq!(resolved.state, TaskState::Working);
        assert!(matches!(
            &resolved.kind,
            TaskEventKind::InputResolved {
                outcome: HitlOutcome::Accept { .. },
                by: AnsweredBy::Voice,
                ..
            }
        ));
        assert_eq!(registry.board().open_question, None);

        // The effect is recorded when the run reports it.
        let committed = RunEvent::Committed {
            label: "table booking at Rosa's".to_owned(),
        };
        let effects = run(&mut registry, 5_400, provisional, committed);
        let labels = Effects::Committed(vec!["table booking at Rosa's".to_owned()]);
        assert_eq!(
            kinds(&effects),
            [&TaskEventKind::EffectsUpdated(labels.clone())]
        );
        assert_eq!(registry.board().tasks[0].effects, labels);
    }

    #[test]
    fn a_decline_refuses_the_call_and_a_late_answer_changes_nothing() {
        let mut registry = registry();
        let (first, _) = delegate(&mut registry, 1_000, 1);
        let (event, mut decision) = gate(4);
        run(&mut registry, 1_500, first, event);
        let request = registry.board().open_question.unwrap().id;
        let decline = || TaskCmd::Answer {
            request: request.clone(),
            outcome: HitlOutcome::Decline { correction: None },
            by: AnsweredBy::Client,
        };
        let effects = registry.apply(3_000, decline());
        assert_eq!(decision.try_recv(), Ok(GateDecision::Refuse("declined")));
        assert!(matches!(
            kinds(&effects)[0],
            TaskEventKind::InputResolved {
                by: AnsweredBy::Client,
                ..
            }
        ));
        // First answer wins.
        assert!(registry.apply(3_100, decline()).is_empty());

        // The same call again is refused without a second question.
        let (event, mut decision) = gate(4);
        assert!(run(&mut registry, 3_200, first, event).is_empty());
        assert_eq!(decision.try_recv(), Ok(GateDecision::Refuse("declined")));
    }

    #[test]
    fn a_revise_refuses_the_held_call_as_stale_and_starts_a_new_run() {
        let mut registry = registry();
        let (first, _) = delegate(&mut registry, 1_000, 1);
        run(&mut registry, 1_200, first, sentence(0));
        let (event, mut decision) = gate(2);
        run(&mut registry, 1_500, first, event);
        let old_question = registry.board().open_question.unwrap().id;

        let effects = registry.apply(
            2_000,
            TaskCmd::Revise {
                task: t(1),
                correction: "actually make it four".to_owned(),
            },
        );
        assert_eq!(decision.try_recv(), Ok(GateDecision::Refuse("stale")));
        let seen = events(&effects);
        assert!(matches!(
            &seen[0].kind,
            TaskEventKind::InputResolved {
                outcome: HitlOutcome::Superseded,
                ..
            }
        ));
        assert_eq!(
            seen[1].kind,
            TaskEventKind::AnswerStopped {
                attempt: 0,
                released: 1,
                reason: StopReason::Revised,
            }
        );
        assert_eq!((seen[1].revision, seen[2].revision), (1, 2));
        assert_eq!(seen[2].kind, TaskEventKind::Revised);
        assert_eq!(cancelled_runs(&effects), [first]);
        let second = started(&effects)[0];
        assert!(effects.iter().any(|effect| matches!(
            effect,
            TaskEffect::StartRun { run, task, words, .. } if *run == second
                && task.as_ref() == Some(&seen[0].task)
                && words == &["request 1".to_owned(), "actually make it four".to_owned()]
        )));

        // An answer to the superseded question is void, and the old run is no longer heard.
        let late = TaskCmd::Answer {
            request: old_question,
            outcome: HitlOutcome::Accept { value: None },
            by: AnsweredBy::Voice,
        };
        assert!(registry.apply(2_100, late).is_empty());
        assert!(run(&mut registry, 2_200, first, sentence(1)).is_empty());
        let (event, mut decision) = gate(2);
        run(&mut registry, 2_300, first, event);
        assert_eq!(decision.try_recv(), Ok(GateDecision::Refuse("stale")));

        // The new run asks again under revision 2, and its sentences are a new attempt.
        let (event, _decision) = gate(4);
        let effects = run(&mut registry, 2_500, second, event);
        let TaskEventKind::InputRequired(request) = kinds(&effects)[0] else {
            panic!("expected a question");
        };
        assert_eq!(request.revision, 2);
        assert!(request.op.as_ref().unwrap().0.ends_with("_2_1"));
        assert!(request.id.0.ends_with("_2"));
        accept(&mut registry, 2_600);
        let effects = run(&mut registry, 3_000, second, sentence(0));
        assert!(matches!(
            kinds(&effects)[0],
            TaskEventKind::AnswerSentence {
                attempt: 1,
                seq: 0,
                first_after_e_ms: Some(1_000),
                ..
            }
        ));
    }

    #[test]
    fn a_repeated_operation_is_asked_and_allowed_once() {
        let (mut registry, first) = with_operation_in_flight();
        let task = registry.board().tasks[0].id.0.clone();
        let committed = RunEvent::Committed {
            label: "table booking".to_owned(),
        };
        run(&mut registry, 4_500, first, committed);

        // The same name and arguments under the same revision: the same operation id, no question.
        let (event, mut decision) = gate(4);
        assert!(run(&mut registry, 5_000, first, event).is_empty());
        assert_eq!(
            decision.try_recv(),
            Ok(GateDecision::Allow(OpId(format!("op_{task}_1_1"))))
        );
        assert_eq!(registry.board().open_question, None);

        // Other arguments are another operation.
        let (event, _decision) = gate(6);
        let effects = run(&mut registry, 5_500, first, event);
        let TaskEventKind::InputRequired(request) = kinds(&effects)[0] else {
            panic!("expected a question");
        };
        assert_eq!(request.op, Some(OpId(format!("op_{task}_1_2"))));
    }

    #[test]
    fn a_cancel_with_nothing_in_flight_is_confirmed_at_once() {
        let mut registry = registry();
        let (first, _) = delegate(&mut registry, 1_000, 1);
        run(&mut registry, 1_200, first, sentence(0));
        let (event, mut decision) = gate(4);
        run(&mut registry, 1_500, first, event);

        let cancel = TaskCmd::Cancel {
            task: t(1),
            by: "user",
        };
        let effects = registry.apply(2_000, cancel);
        assert_eq!(decision.try_recv(), Ok(GateDecision::Refuse("cancelled")));
        assert_eq!(cancelled_runs(&effects), [first]);
        let seen = events(&effects);
        assert_eq!(seen[0].kind, TaskEventKind::CancelRequested);
        assert!(seen[0].cancelling);
        assert!(matches!(
            &seen[1].kind,
            TaskEventKind::InputResolved {
                outcome: HitlOutcome::Cancel,
                by: AnsweredBy::Voice,
                ..
            }
        ));
        assert!(matches!(
            &seen[2].kind,
            TaskEventKind::AnswerStopped {
                reason: StopReason::Cancelling,
                released: 1,
                ..
            }
        ));
        assert_eq!(
            seen[3].kind,
            TaskEventKind::Cancelled {
                by: "user",
                effects: Effects::NoneCommitted,
            }
        );
        assert_eq!(
            (seen[3].state, seen[3].cancelling),
            (TaskState::Cancelled, false)
        );
        assert_eq!(registry.next_wakeup(), None);

        // The run's own report and a second cancel change nothing.
        assert!(run(&mut registry, 2_100, first, RunEvent::Cancelled).is_empty());
        let again = TaskCmd::Cancel {
            task: t(1),
            by: "user",
        };
        assert!(registry.apply(2_200, again).is_empty());
    }

    #[test]
    fn a_cancel_after_a_commit_reports_the_effects() {
        let (mut registry, first) = with_operation_in_flight();
        let committed = RunEvent::Committed {
            label: "table hold at Rosa's".to_owned(),
        };
        run(&mut registry, 4_500, first, committed);
        let cancel = TaskCmd::Cancel {
            task: t(1),
            by: "client",
        };
        let effects = registry.apply(5_000, cancel);
        assert_eq!(
            kinds(&effects)[1],
            &TaskEventKind::Cancelled {
                by: "client",
                effects: Effects::Committed(vec!["table hold at Rosa's".to_owned()]),
            }
        );
    }

    #[test]
    fn a_cancel_waits_for_an_operation_in_flight() {
        // The operation commits while the cancel waits.
        let (mut registry, first) = with_operation_in_flight();
        let cancel = || TaskCmd::Cancel {
            task: t(1),
            by: "user",
        };
        let effects = registry.apply(5_000, cancel());
        assert_eq!(kinds(&effects), [&TaskEventKind::CancelRequested]);
        assert_eq!(cancelled_runs(&effects), [first]);
        assert_eq!(registry.next_wakeup(), Some(15_000));
        let view = &registry.board().tasks[0];
        assert_eq!((view.state, view.cancelling), (TaskState::Working, true));
        let committed = RunEvent::Committed {
            label: "table booking".to_owned(),
        };
        let effects = run(&mut registry, 6_000, first, committed);
        let labels = Effects::Committed(vec!["table booking".to_owned()]);
        assert_eq!(
            kinds(&effects),
            [
                &TaskEventKind::EffectsUpdated(labels.clone()),
                &TaskEventKind::Cancelled {
                    by: "user",
                    effects: labels,
                }
            ]
        );
        assert_eq!(registry.next_wakeup(), None);

        // The operation ends without committing.
        let (mut registry, first) = with_operation_in_flight();
        registry.apply(5_000, cancel());
        let effects = run(&mut registry, 6_000, first, RunEvent::Cancelled);
        assert_eq!(
            kinds(&effects),
            [&TaskEventKind::Cancelled {
                by: "user",
                effects: Effects::NoneCommitted,
            }]
        );
    }

    #[test]
    fn a_cancel_is_unconfirmed_after_ten_seconds_without_a_report() {
        let (mut registry, first) = with_operation_in_flight();
        let cancel = TaskCmd::Cancel {
            task: t(1),
            by: "user",
        };
        registry.apply(5_000, cancel);
        assert!(registry.apply(14_999, TaskCmd::Tick).is_empty());
        let effects = registry.apply(15_000, TaskCmd::Tick);
        let unconfirmed = events(&effects)[0];
        assert_eq!(
            unconfirmed.kind,
            TaskEventKind::CancelUnconfirmed {
                effects: Effects::Unknown,
            }
        );
        // Never shown as cancelled while the truth is unknown.
        assert_eq!(
            (unconfirmed.state, unconfirmed.cancelling),
            (TaskState::Working, true)
        );
        assert_eq!(registry.board().tasks[0].effects, Effects::Unknown);
        assert_eq!(registry.next_wakeup(), None);

        // The report arrives after all: now the cancel is confirmed, with the truth.
        let effects = run(&mut registry, 20_000, first, RunEvent::Cancelled);
        assert_eq!(
            kinds(&effects),
            [&TaskEventKind::Cancelled {
                by: "user",
                effects: Effects::NoneCommitted,
            }]
        );
    }

    #[test]
    fn the_answer_cancel_cancels_the_task() {
        let mut registry = registry();
        let (first, _) = delegate(&mut registry, 1_000, 1);
        let (event, mut decision) = gate(4);
        run(&mut registry, 1_500, first, event);
        let request = registry.board().open_question.unwrap().id;
        let answer = TaskCmd::Answer {
            request,
            outcome: HitlOutcome::Cancel,
            by: AnsweredBy::Voice,
        };
        let effects = registry.apply(3_000, answer);
        assert_eq!(decision.try_recv(), Ok(GateDecision::Refuse("cancelled")));
        let kinds = kinds(&effects);
        assert!(matches!(kinds[0], TaskEventKind::InputResolved { .. }));
        assert_eq!(kinds[1], &TaskEventKind::CancelRequested);
        assert_eq!(
            kinds[2],
            &TaskEventKind::Cancelled {
                by: "user",
                effects: Effects::NoneCommitted,
            }
        );
    }

    #[test]
    fn limits_come_from_config_and_excess_work_waits() {
        // No gateway limit by default: ten requests all run.
        let mut registry = registry();
        for turn in 1..=10 {
            let (_, effects) = delegate(&mut registry, 1_000, turn);
            assert_eq!(events(&effects)[0].state, TaskState::Working);
        }

        let mut cfg = InteractionConfig::default();
        cfg.tasks.limits.max_live = Some(3);
        let mut registry = Registry::new(&cfg);
        let runs: Vec<RunId> = (1..=3)
            .map(|turn| delegate(&mut registry, 1_000, turn).0)
            .collect();
        // No place: nothing is started speculatively either.
        let speculative = TaskCmd::StartProvisional {
            turn: TurnId(4),
            text: "request 4".to_owned(),
            e_at: Some(1_000),
        };
        assert!(registry.apply(1_000, speculative).is_empty());

        // Beyond the limit work waits, in order; nothing is refused for count.
        for turn in 4..=7 {
            let delegate = TaskCmd::Delegate {
                turn: TurnId(turn),
                text: format!("request {turn}"),
                adopt: false,
            };
            let effects = registry.apply(1_100, delegate);
            assert!(started(&effects).is_empty());
            assert_eq!(events(&effects)[0].state, TaskState::Submitted);
        }
        assert_eq!(registry.board().tasks.len(), 7);

        // The same utterance cannot cause a second task.
        let again = TaskCmd::Delegate {
            turn: TurnId(1),
            text: "request 1".to_owned(),
            adopt: false,
        };
        assert!(registry.apply(1_300, again).is_empty());

        // A finished task hands its place to the oldest queued one.
        let finished = RunEvent::Finished {
            text: "Done.".to_owned(),
        };
        let effects = run(&mut registry, 2_000, runs[0], finished);
        let seen = events(&effects);
        assert!(matches!(seen[0].kind, TaskEventKind::Completed { .. }));
        assert_eq!(
            (seen[1].task_ref.clone(), &seen[1].kind),
            (t(4), &TaskEventKind::Working)
        );
        assert_eq!(seen[1].state, TaskState::Working);
        assert_eq!(started(&effects).len(), 1);

        // A queued task is cancelled without a run.
        let cancel = TaskCmd::Cancel {
            task: t(5),
            by: "user",
        };
        let effects = registry.apply(2_100, cancel);
        assert!(cancelled_runs(&effects).is_empty());
        assert!(matches!(
            kinds(&effects)[1],
            TaskEventKind::Cancelled { .. }
        ));
    }

    #[test]
    fn a_tenants_limit_counts_the_requests_of_all_its_sessions() {
        let mut cfg = InteractionConfig::default();
        cfg.tasks.limits.max_live_tenant = Some(1);
        let owner = format!("tenant-{}", uuid::Uuid::new_v4().simple());
        let mut one = Registry::new(&cfg);
        one.set_owner(&owner);
        let mut two = Registry::new(&cfg);
        two.set_owner(&owner);
        let (first, _) = delegate(&mut one, 1_000, 1);
        let request = TaskCmd::Delegate {
            turn: TurnId(1),
            text: "request 1".to_owned(),
            adopt: false,
        };
        let effects = two.apply(1_000, request);
        assert!(started(&effects).is_empty());
        assert_eq!(events(&effects)[0].state, TaskState::Submitted);
        // The queued request looks again while it waits.
        assert_eq!(two.next_wakeup(), Some(1_000 + TENANT_POLL_MS));
        let finished = RunEvent::Finished {
            text: "Done.".to_owned(),
        };
        run(&mut one, 1_200, first, finished);
        let effects = two.apply(1_250, TaskCmd::Tick);
        assert_eq!(started(&effects).len(), 1);
        assert_eq!(kinds(&effects), [&TaskEventKind::Working]);
        // A session that ends gives its places back.
        drop(two);
        assert_eq!(tenant_live(&owner), 0);
    }

    /// An orchestrator's registry with the catalog entries `browser` (at most `browsers` at
    /// once, may change things) and `coder` (no limit, changes nothing).
    fn orchestrator(browsers: Option<usize>) -> Registry {
        let mut registry = registry();
        let spec = |label: &str, side_effects, max_concurrent| AgentSpec {
            label: label.to_owned(),
            side_effects,
            max_concurrent,
            question_deadline_ms: Some(5_000),
            direct_follow_up: true,
        };
        let agents = [
            ("browser".to_owned(), spec("a2a:browser", true, browsers)),
            ("coder".to_owned(), spec("webhook:coder", false, None)),
        ];
        registry.set_catalog(None, agents.into_iter().collect());
        registry
    }

    /// The parent's run starts a child on `agent`: its reply, and how it will end.
    fn dispatch(
        registry: &mut Registry,
        now: Ms,
        parent: RunId,
        agent: &str,
        request: &str,
    ) -> (
        Vec<TaskEffect>,
        Result<Dispatched, String>,
        oneshot::Receiver<Outcome>,
    ) {
        let (reply, mut answer) = oneshot::channel();
        let (done, end) = oneshot::channel();
        let event = RunEvent::Dispatch {
            agent: agent.to_owned(),
            request: request.to_owned(),
            reply,
            done,
        };
        let effects = run(registry, now, parent, event);
        let answer = answer.try_recv().expect("answered at once");
        (effects, answer, end)
    }

    #[test]
    fn an_orchestrator_starts_two_agents_in_one_round_and_hears_both_results() {
        let mut registry = orchestrator(None);
        let (parent, effects) = delegate(&mut registry, 1_000, 1);
        let request = events(&effects)[0].clone();
        assert_eq!(request.lineage.parent_id, None);
        assert_eq!(request.lineage.trace_id.len(), 32);
        // The request may change what its agents may change.
        assert!(matches!(
            request.kind,
            TaskEventKind::Created {
                side_effects: true,
                ..
            }
        ));

        let (effects, flight, mut flight_end) = dispatch(
            &mut registry,
            1_100,
            parent,
            "browser",
            "Book a flight to Delhi",
        );
        let flight = flight.expect("started");
        assert_eq!((flight.task_ref.clone(), flight.queued), (t(2), false));
        let created = events(&effects)[0];
        assert_eq!(created.lineage.parent_id, Some(request.task.clone()));
        assert_eq!(created.lineage.agent.as_deref(), Some("browser"));
        assert_eq!(created.lineage.trace_id, request.lineage.trace_id);
        assert!(matches!(
            &created.kind,
            TaskEventKind::Created { backend, .. } if backend == "a2a:browser"
        ));
        // The child runs on its agent, with its request only, in its parent's trace.
        let child_run = match &effects[..] {
            [
                _,
                TaskEffect::StartRun {
                    run,
                    words,
                    agent,
                    trace_id,
                    ..
                },
                ..,
            ] => {
                assert_eq!(words, &["Book a flight to Delhi".to_owned()]);
                assert_eq!(agent.as_deref(), Some("browser"));
                assert_eq!(trace_id, &request.lineage.trace_id);
                *run
            }
            other => panic!("{other:?}"),
        };
        let (_, test, mut test_end) = dispatch(
            &mut registry,
            1_100,
            parent,
            "coder",
            "Fix the failing test",
        );
        assert_eq!(test.expect("started").task_ref, t(3));
        assert_eq!(registry.board().tasks.len(), 3);

        // A child's answer is not said: its orchestrator hears it.
        let effects = run(&mut registry, 5_000, child_run, sentence(0));
        assert!(events(&effects).is_empty());
        let effects = run(
            &mut registry,
            5_100,
            child_run,
            finished("Booked: AI 101 at 9:40."),
        );
        assert!(matches!(
            kinds(&effects)[0],
            TaskEventKind::Completed { .. }
        ));
        let outcome = flight_end.try_recv().expect("the parent hears the end");
        assert_eq!(
            (outcome.task_ref, outcome.state, outcome.summary.as_str()),
            (t(2), TaskState::Completed, "Booked: AI 101 at 9:40.")
        );
        assert_eq!(outcome.lineage.agent.as_deref(), Some("browser"));
        assert!(test_end.try_recv().is_err(), "the other child still runs");
        let still = registry.board().tasks;
        assert_eq!(
            (still[0].state, still[2].state),
            (TaskState::Working, TaskState::Working)
        );
    }

    #[test]
    fn an_agents_limit_queues_its_children_and_stopping_the_request_stops_them() {
        let mut registry = orchestrator(Some(1));
        let (parent, _) = delegate(&mut registry, 1_000, 1);
        let (effects, _, _) = dispatch(&mut registry, 1_100, parent, "browser", "Book a flight");
        let first = started(&effects)[0];
        let (effects, queued, _) =
            dispatch(&mut registry, 1_100, parent, "browser", "Book a hotel");
        assert!(queued.expect("taken").queued);
        assert!(started(&effects).is_empty());
        assert_eq!(events(&effects)[0].state, TaskState::Submitted);
        // Another agent is not held up by this one's limit.
        let (effects, _, _) = dispatch(&mut registry, 1_100, parent, "coder", "Fix the test");
        assert_eq!(started(&effects).len(), 1);
        // An unknown agent starts nothing.
        let (_, refused, _) = dispatch(&mut registry, 1_100, parent, "mailer", "Send it");
        assert!(refused.is_err());

        // The agent's place frees: the queued child starts.
        let failed = RunEvent::Failed {
            error: "HTTP 503".to_owned(),
        };
        let effects = run(&mut registry, 2_000, first, failed);
        assert_eq!(started(&effects).len(), 1);

        // The caller stops the request: its children stop with it, as stopped by their parent.
        let cancel = TaskCmd::Cancel {
            task: t(1),
            by: "user",
        };
        let effects = registry.apply(3_000, cancel);
        let by_parent =
            |kind: &&TaskEventKind| matches!(kind, TaskEventKind::Cancelled { by: "parent", .. });
        assert_eq!(kinds(&effects).into_iter().filter(by_parent).count(), 2);
        let requested = kinds(&effects)
            .into_iter()
            .filter(|kind| **kind == TaskEventKind::CancelRequested)
            .count();
        assert_eq!(requested, 3, "the request and its two live children");
        assert!(
            registry
                .board()
                .tasks
                .iter()
                .all(|task| !task.state.is_working() || task.cancelling)
        );
    }

    #[test]
    fn an_agents_question_has_its_deadline() {
        let mut registry = orchestrator(None);
        let (parent, _) = delegate(&mut registry, 1_000, 1);
        let (effects, _, _) = dispatch(&mut registry, 1_100, parent, "browser", "Book a seat");
        let child = started(&effects)[0];
        let (reply, _answer) = oneshot::channel();
        let question = RunEvent::Gate {
            op_name: "ask_caller".to_owned(),
            args: serde_json::json!({ "kind": "value", "question": "Window or aisle?" }),
            kind: HitlKind::Value,
            reply,
        };
        let effects = run(&mut registry, 2_000, child, question);
        let TaskEventKind::InputRequired(request) = kinds(&effects)[0] else {
            panic!("{effects:?}");
        };
        assert_eq!(request.expires_at, 2_000 + 5_000);
    }

    #[test]
    fn a_backends_deadline_is_the_questions_and_never_moved() {
        let mut registry = orchestrator(None);
        let (parent, _) = delegate(&mut registry, 1_000, 1);
        let (effects, _, _) = dispatch(&mut registry, 1_100, parent, "coder", "Fix the test");
        let child = started(&effects)[0];
        let (reply, _answer) = oneshot::channel();
        // `expires_in_ms` of the backend wins over the entry's default and the session's.
        let args = serde_json::json!({ "kind": "value", "question": "Which branch?",
                                       "expires_in_ms": 3_000 });
        let question = RunEvent::Gate {
            op_name: "ask_caller".to_owned(),
            args,
            kind: HitlKind::Value,
            reply,
        };
        let effects = run(&mut registry, 2_000, child, question);
        let TaskEventKind::InputRequired(request) = kinds(&effects)[0] else {
            panic!("{effects:?}");
        };
        assert_eq!((request.expires_at, request.firm), (2_000 + 3_000, true));
        // A question with no deadline of its own is the session's, and not firm.
        let mut plain = super::tests::registry();
        let (run_id, _) = delegate(&mut plain, 1_000, 1);
        let (reply, _answer) = oneshot::channel();
        let question = RunEvent::Gate {
            op_name: "ask_caller".to_owned(),
            args: serde_json::json!({ "kind": "value", "question": "Which day?" }),
            kind: HitlKind::Value,
            reply,
        };
        let effects = run(&mut plain, 2_000, run_id, question);
        let TaskEventKind::InputRequired(request) = kinds(&effects)[0] else {
            panic!("{effects:?}");
        };
        assert_eq!((request.expires_at, request.firm), (2_000 + 20_000, false));
    }

    #[test]
    fn a_follow_up_continues_the_agent_with_its_handle_and_after_the_ttl_with_a_summary() {
        let mut registry = orchestrator(None);
        let (parent, _) = delegate(&mut registry, 1_000, 1);
        let (effects, _, _) = dispatch(&mut registry, 1_100, parent, "browser", "Book a flight");
        let child = started(&effects)[0];
        run(
            &mut registry,
            1_200,
            child,
            RunEvent::Handle("ctx-flight".to_owned()),
        );
        let done = RunEvent::Finished {
            text: "Booked AI 101 at 9:40. Seat 14C.".to_owned(),
        };
        run(&mut registry, 5_000, child, done);
        // More work about it: a request of the caller's own, on the same agent, continuing it.
        let effects = registry.apply(
            6_000,
            TaskCmd::FollowUp {
                turn: TurnId(2),
                task: t(2),
                text: "Book the return too".to_owned(),
            },
        );
        let created = events(&effects)[0].clone();
        assert_eq!(created.lineage.parent_id, None);
        assert_eq!(created.lineage.agent.as_deref(), Some("browser"));
        assert_eq!(
            created.lineage.follows.as_ref().map(|task| task.0.as_str()),
            Some(registry.tasks[1].view.id.0.as_str())
        );
        let start = effects.iter().find_map(|effect| match effect {
            TaskEffect::StartRun {
                agent,
                words,
                follow_up,
                handle,
                ..
            } => Some((agent.clone(), words.clone(), *follow_up, handle.clone())),
            _ => None,
        });
        assert_eq!(
            start,
            Some((
                Some("browser".to_owned()),
                vec!["Book the return too".to_owned()],
                true,
                Some("ctx-flight".to_owned())
            ))
        );
        // After `tasks.follow_up_ttl_s` the handle is not used: the request names the result.
        let late = 5_000 + 1_800_000;
        let effects = registry.apply(
            late,
            TaskCmd::FollowUp {
                turn: TurnId(3),
                task: t(2),
                text: "And a hotel".to_owned(),
            },
        );
        let start = effects.iter().find_map(|effect| match effect {
            TaskEffect::StartRun { words, handle, .. } => Some((words.clone(), handle.clone())),
            _ => None,
        });
        assert_eq!(
            start,
            Some((
                vec![
                    "And a hotel\n(Earlier, \"Book a flight\" came to: Booked AI 101 at 9:40.)"
                        .to_owned()
                ],
                None
            ))
        );
        // A task that ran on no agent of the catalog: an ordinary request.
        let mut plain = super::tests::registry();
        let (first, _) = delegate(&mut plain, 1_000, 1);
        let finished = RunEvent::Finished {
            text: "Done.".to_owned(),
        };
        run(&mut plain, 2_000, first, finished);
        let effects = plain.apply(
            3_000,
            TaskCmd::FollowUp {
                turn: TurnId(2),
                task: t(1),
                text: "and again".to_owned(),
            },
        );
        assert_eq!(events(&effects)[0].lineage.follows, None);

        // An agent that takes arguments, or whose calls are confirmed: the follow-up goes to
        // the orchestrator, never past its gate to the agent.
        let mut gated = orchestrator(None);
        gated
            .agents
            .get_mut("browser")
            .expect("browser")
            .direct_follow_up = false;
        let (parent, _) = delegate(&mut gated, 1_000, 1);
        let (effects, _, _) = dispatch(&mut gated, 1_100, parent, "browser", "Delete the note");
        let child = started(&effects)[0];
        let deleted = RunEvent::Finished {
            text: "Deleted.".to_owned(),
        };
        run(&mut gated, 2_000, child, deleted);
        let effects = gated.apply(
            3_000,
            TaskCmd::FollowUp {
                turn: TurnId(2),
                task: t(2),
                text: "remind me to call mom, it's urgent".to_owned(),
            },
        );
        let created = events(&effects)[0].clone();
        assert_eq!(
            (created.lineage.agent, created.lineage.follows),
            (None, None)
        );
        let agent = effects.iter().find_map(|effect| match effect {
            TaskEffect::StartRun { agent, .. } => Some(agent.clone()),
            _ => None,
        });
        assert_eq!(agent, Some(None));
    }

    #[test]
    fn a_hesitant_request_joins_its_words_into_one_task() {
        let mut registry = registry();
        let (first, _) = delegate(&mut registry, 1_000, 1);
        let effects = registry.apply(
            2_000,
            TaskCmd::Continue {
                task: t(1),
                text: "and my budget is 1500".to_owned(),
            },
        );
        assert!(cancelled_runs(&effects).contains(&first));
        let words = effects.iter().find_map(|effect| match effect {
            TaskEffect::StartRun { words, .. } => Some(words.clone()),
            _ => None,
        });
        assert_eq!(
            words,
            Some(vec!["request 1 and my budget is 1500".to_owned()])
        );
        assert_eq!(registry.tasks.len(), 1);
        assert_eq!(registry.tasks[0].view.revision, 2);
    }

    /// A registry whose background is a Bud agent with tools.
    fn agent_registry() -> Registry {
        let mut registry = registry();
        registry.set_agent("support", true);
        registry
    }

    #[test]
    fn an_agent_runs_one_task_at_a_time_and_queues_the_next() {
        let mut registry = agent_registry();
        // Nothing is started before the turn is settled: a run of the agent cannot be dropped
        // unseen. (The session's policy never asks; the registry has room, so it would.)
        let (first, effects) = delegate(&mut registry, 1_000, 1);
        let created = events(&effects)[0];
        assert_eq!(
            created.kind,
            TaskEventKind::Created {
                title: "request 1".to_owned(),
                // The gateway's own tools are gone; the agent's may change something.
                side_effects: true,
                speculative: false,
                backend: "agent:support".to_owned(),
            }
        );
        assert_eq!(created.state, TaskState::Working);

        // The conversation is taken: a second request waits, and nothing starts for it.
        let second = TaskCmd::Delegate {
            turn: TurnId(2),
            text: "request 2".to_owned(),
            adopt: false,
        };
        let effects = registry.apply(1_100, second);
        assert!(started(&effects).is_empty());
        assert_eq!(events(&effects)[0].state, TaskState::Submitted);
        let speculative = TaskCmd::StartProvisional {
            turn: TurnId(3),
            text: "request 3".to_owned(),
            e_at: Some(1_200),
        };
        assert!(registry.apply(1_200, speculative).is_empty());

        // The first run ends: the queued task gets the conversation.
        let finished = RunEvent::Finished {
            text: "Done.".to_owned(),
        };
        let effects = run(&mut registry, 2_000, first, finished);
        let seen = events(&effects);
        assert!(matches!(seen[0].kind, TaskEventKind::Completed { .. }));
        assert_eq!(
            (seen[1].task_ref.clone(), &seen[1].kind),
            (t(2), &TaskEventKind::Working)
        );
        assert_eq!(started(&effects).len(), 1);

        // A tool-free agent's tasks change nothing.
        let mut tool_free = Registry::new(&InteractionConfig::default());
        tool_free.set_agent("faq", false);
        let (_, effects) = delegate(&mut tool_free, 1_000, 1);
        assert!(matches!(
            events(&effects)[0].kind,
            TaskEventKind::Created {
                side_effects: false,
                ..
            }
        ));
    }

    fn agent_tool(name: &str) -> RunEvent {
        RunEvent::ToolStarted {
            name: name.to_owned(),
            phrase: None,
            remote: true,
        }
    }

    #[test]
    fn a_cancel_waits_for_an_agents_tool_and_says_what_it_did() {
        let cancel = || TaskCmd::Cancel {
            task: t(1),
            by: "user",
        };
        // A tool of the agent's is running: what it does is not seen, so the cancel is not
        // confirmed until the run reports.
        let mut registry = agent_registry();
        let (first, _) = delegate(&mut registry, 1_000, 1);
        run(&mut registry, 1_500, first, agent_tool("refund_order"));
        let effects = registry.apply(2_000, cancel());
        assert_eq!(cancelled_runs(&effects), [first]);
        assert_eq!(kinds(&effects), [&TaskEventKind::CancelRequested]);
        assert_eq!(registry.next_wakeup(), Some(2_000 + CANCEL_WAIT_MS));
        // The run says the tool had started, then that it stopped.
        let started_tool = RunEvent::Committed {
            label: "started refund order".to_owned(),
        };
        let effects = run(&mut registry, 2_300, first, started_tool);
        let labels = Effects::Committed(vec!["started refund order".to_owned()]);
        assert_eq!(
            kinds(&effects),
            [
                &TaskEventKind::EffectsUpdated(labels.clone()),
                &TaskEventKind::Cancelled {
                    by: "user",
                    effects: labels,
                },
            ]
        );
        assert!(run(&mut registry, 2_310, first, RunEvent::Cancelled).is_empty());

        // A tool that had finished is an effect of the task; nothing was running at the
        // cancel, which is confirmed at once with it.
        let mut registry = agent_registry();
        let (first, _) = delegate(&mut registry, 1_000, 1);
        run(&mut registry, 1_500, first, agent_tool("lookup_order"));
        let used = RunEvent::Committed {
            label: "used lookup order".to_owned(),
        };
        run(&mut registry, 1_900, first, used);
        let effects = registry.apply(2_000, cancel());
        assert_eq!(
            kinds(&effects).last(),
            Some(&&TaskEventKind::Cancelled {
                by: "user",
                effects: Effects::Committed(vec!["used lookup order".to_owned()]),
            })
        );

        // The gateway's own tools are another matter: a lookup of the LLM tier that is
        // running does not hold a cancel back.
        let mut registry = Registry::new(&InteractionConfig::default());
        let (first, _) = delegate(&mut registry, 1_000, 1);
        let lookup = RunEvent::ToolStarted {
            name: "search".to_owned(),
            phrase: None,
            remote: false,
        };
        run(&mut registry, 1_500, first, lookup);
        let effects = registry.apply(2_000, cancel());
        assert!(matches!(
            kinds(&effects).last(),
            Some(TaskEventKind::Cancelled {
                effects: Effects::NoneCommitted,
                ..
            })
        ));
    }

    #[test]
    fn a_backend_that_does_not_say_what_it_did_leaves_the_effects_unknown() {
        let remote = || {
            let mut registry = registry();
            registry.set_backend("a2a:calendar".to_owned(), Some(true), Some(1));
            registry
        };
        let working = || RunEvent::ToolStarted {
            name: "calendar".to_owned(),
            phrase: None,
            remote: true,
        };
        // The task exists on the backend: from then on nobody here sees what it changes.
        let mut registry = remote();
        let (first, effects) = delegate(&mut registry, 1_000, 1);
        assert!(matches!(
            kinds(&effects)[0],
            TaskEventKind::Created { side_effects: true, backend, .. } if backend == "a2a:calendar"
        ));
        let effects = run(&mut registry, 1_100, first, RunEvent::EffectsUnseen);
        assert_eq!(
            kinds(&effects),
            [&TaskEventKind::EffectsUpdated(Effects::Unknown)]
        );
        run(&mut registry, 1_110, first, working());
        // A later report does not make them known again, and neither does the answer.
        run(&mut registry, 1_500, first, sentence(0));
        assert_eq!(registry.board().tasks[0].effects, Effects::Unknown);
        let finished = RunEvent::Finished {
            text: "Sentence 0.".to_owned(),
        };
        let effects = run(&mut registry, 1_600, first, finished);
        assert!(matches!(
            kinds(&effects).last(),
            Some(TaskEventKind::Completed {
                effects: Effects::Unknown,
                ..
            })
        ));

        // A cancel waits for the backend, and is then confirmed with what is known: nothing.
        let mut registry = remote();
        let (first, _) = delegate(&mut registry, 1_000, 1);
        run(&mut registry, 1_100, first, RunEvent::EffectsUnseen);
        run(&mut registry, 1_110, first, working());
        let cancel = TaskCmd::Cancel {
            task: t(1),
            by: "user",
        };
        let effects = registry.apply(2_000, cancel);
        assert_eq!(kinds(&effects), [&TaskEventKind::CancelRequested]);
        let effects = run(&mut registry, 2_400, first, RunEvent::Cancelled);
        let cancelled = TaskEventKind::Cancelled {
            by: "user",
            effects: Effects::Unknown,
        };
        assert_eq!(kinds(&effects), [&cancelled]);

        // A backend that reports its effects and does not confirm a cancel: the same, in
        // one report, whatever it had reported before.
        let mut registry = remote();
        let (first, _) = delegate(&mut registry, 1_000, 1);
        let held = RunEvent::Committed {
            label: "held the table".to_owned(),
        };
        run(&mut registry, 1_200, first, held);
        run(&mut registry, 1_210, first, working());
        let cancel = TaskCmd::Cancel {
            task: t(1),
            by: "user",
        };
        registry.apply(2_000, cancel);
        let effects = run(&mut registry, 10_000, first, RunEvent::EffectsUnseen);
        assert_eq!(kinds(&effects), [&cancelled]);

        // A backend the gateway prompts keeps the session's tools.
        let mut registry = self::registry();
        registry.set_backend("openai_responses:research".to_owned(), None, None);
        let (_, effects) = delegate(&mut registry, 1_000, 1);
        assert!(matches!(
            kinds(&effects)[0],
            TaskEventKind::Created { side_effects: true, backend, .. }
                if backend == "openai_responses:research"
        ));
        let (_, effects) = delegate(&mut registry, 1_100, 2);
        assert_eq!(started(&effects).len(), 1);
    }

    #[test]
    fn the_fifty_first_task_of_a_session_runs() {
        let mut registry = registry();
        for turn in 1..=50 {
            let (first, _) = delegate(&mut registry, turn, turn);
            let finished = RunEvent::Finished {
                text: String::new(),
            };
            run(&mut registry, turn, first, finished);
        }
        let (_, effects) = delegate(&mut registry, 100, 51);
        assert_eq!(registry.board().tasks.len(), ENDED_KEPT + 1);
        assert_eq!(events(&effects)[0].state, TaskState::Working);
        assert_eq!(started(&effects).len(), 1);
    }

    #[test]
    fn a_long_session_keeps_its_live_tasks_and_only_the_latest_ended_ones() {
        let mut registry = registry();
        let (asking, _) = delegate(&mut registry, 1, 1);
        let (event, mut decision) = gate(4);
        run(&mut registry, 1, asking, event);
        let mut finished = Vec::new();
        for turn in 2..=60 {
            let (done, _) = delegate(&mut registry, turn, turn);
            let event = RunEvent::Finished {
                text: String::new(),
            };
            run(&mut registry, turn, done, event);
            finished.push(done);
        }
        let board = registry.board();
        assert_eq!(board.tasks.len(), ENDED_KEPT + 1);
        assert_eq!(board.tasks[0].task_ref, t(1), "the asking task is kept");
        assert_eq!(board.tasks[1].task_ref, t(60 - ENDED_KEPT as u32 + 1));
        // A late event of a forgotten run changes nothing; the kept question is answered.
        assert!(run(&mut registry, 70, finished[0], sentence(1)).is_empty());
        accept(&mut registry, 80);
        assert!(matches!(decision.try_recv(), Ok(GateDecision::Allow(_))));
        assert_eq!(registry.board().tasks[0].state, TaskState::Working);
    }

    #[test]
    fn one_question_is_open_at_a_time_and_ask_caller_gets_its_answer() {
        let mut registry = registry();
        let (first, _) = delegate(&mut registry, 1_000, 1);
        let (second, _) = delegate(&mut registry, 1_000, 2);
        let (event, _first_decision) = gate(4);
        run(&mut registry, 1_500, first, event);

        let (reply, mut decision) = oneshot::channel();
        let ask = RunEvent::Gate {
            op_name: "ask_caller".to_owned(),
            args: json!({"kind": "choose", "question": "Which seat?", "options": ["Window", "Aisle"]}),
            kind: HitlKind::Choose,
            reply,
        };
        // The second task's question waits behind the first.
        assert!(run(&mut registry, 1_600, second, ask).is_empty());
        assert_eq!(registry.board().tasks[1].state, TaskState::Working);

        let effects = accept(&mut registry, 3_000);
        let asked = events(&effects)[1];
        assert_eq!(asked.task_ref, t(2));
        let TaskEventKind::InputRequired(request) = &asked.kind else {
            panic!("expected the waiting question");
        };
        assert_eq!((request.kind, request.op.clone()), (HitlKind::Choose, None));
        assert_eq!(request.question.as_deref(), Some("Which seat?"));
        assert_eq!(request.options, ["Window", "Aisle"]);
        assert!(!request.irreversible);
        assert_eq!(request.expires_at, 23_000);

        let answer = TaskCmd::Answer {
            request: request.id.clone(),
            outcome: HitlOutcome::Accept {
                value: Some(json!("Aisle")),
            },
            by: AnsweredBy::Voice,
        };
        registry.apply(4_000, answer);
        assert_eq!(
            decision.try_recv(),
            Ok(GateDecision::Answer(json!("Aisle")))
        );
    }

    #[test]
    fn a_tool_call_after_the_marker_stops_the_answer_and_a_failure_reports_it() {
        let mut registry = registry();
        let (first, _) = delegate(&mut registry, 1_000, 1);
        run(&mut registry, 1_200, first, sentence(0));
        let effects = run(&mut registry, 1_300, first, RunEvent::MarkerThenTool);
        assert_eq!(
            kinds(&effects),
            [&TaskEventKind::AnswerStopped {
                attempt: 0,
                released: 1,
                reason: StopReason::MarkerThenToolCall,
            }]
        );
        let started_tool = RunEvent::ToolStarted {
            remote: false,
            name: "search".to_owned(),
            phrase: Some("Checking availability".to_owned()),
        };
        let effects = run(&mut registry, 1_400, first, started_tool);
        assert_eq!(
            kinds(&effects),
            [&TaskEventKind::Progress {
                text: Some("Checking availability".to_owned()),
            }]
        );
        let next_attempt = RunEvent::Sentence {
            attempt: 1,
            seq: 0,
            text: "Second try.".to_owned(),
        };
        run(&mut registry, 2_000, first, next_attempt);
        let failed = RunEvent::Failed {
            error: "LLM endpoint error".to_owned(),
        };
        let effects = run(&mut registry, 2_100, first, failed);
        assert_eq!(
            kinds(&effects),
            [
                &TaskEventKind::AnswerStopped {
                    attempt: 1,
                    released: 1,
                    reason: StopReason::RunFailed,
                },
                &TaskEventKind::Failed {
                    error: "LLM endpoint error".to_owned(),
                    effects: Effects::NoneCommitted,
                }
            ]
        );
    }

    /// The end of a session nobody reads the results of.
    const HUNG_UP: TaskCmd = TaskCmd::SessionEnded {
        push: false,
        grace_ms: 0,
    };

    #[test]
    fn a_session_end_cancels_runs_and_lets_an_operation_in_flight_finish() {
        let (mut registry, first) = with_operation_in_flight();
        let (second, _) = delegate(&mut registry, 4_100, 2);
        let effects = registry.apply(5_000, HUNG_UP);
        assert_eq!(cancelled_runs(&effects), [second]);
        let seen = events(&effects);
        assert_eq!(seen[0].kind, TaskEventKind::CancelRequested);
        assert_eq!(
            seen[1].kind,
            TaskEventKind::Cancelled {
                by: "system",
                effects: Effects::NoneCommitted,
            }
        );
        assert_eq!(registry.board().tasks[0].state, TaskState::Working);
        assert_eq!(registry.next_wakeup(), Some(35_000));

        // Nothing new starts after the end.
        let late = TaskCmd::Delegate {
            turn: TurnId(9),
            text: "late".to_owned(),
            adopt: false,
        };
        assert!(registry.apply(5_100, late).is_empty());

        // The call reports: the run goes on, with as long again for its answer. Nobody can be
        // asked any more, so a further gated call is refused.
        let booked = Effects::Committed(vec!["table booking".to_owned()]);
        let committed = RunEvent::Committed {
            label: "table booking".to_owned(),
        };
        let effects = run(&mut registry, 6_000, first, committed);
        assert!(cancelled_runs(&effects).is_empty());
        assert_eq!(
            kinds(&effects),
            [&TaskEventKind::EffectsUpdated(booked.clone())]
        );
        assert_eq!(registry.next_wakeup(), Some(36_000));
        let (again, mut refused) = gate(4);
        assert!(run(&mut registry, 6_100, first, again).is_empty());
        assert_eq!(refused.try_recv(), Ok(GateDecision::Refuse("cancelled")));
        let effects = run(&mut registry, 6_500, first, sentence(0));
        assert!(matches!(
            kinds(&effects)[0],
            TaskEventKind::AnswerSentence { .. }
        ));
        assert_eq!(registry.next_wakeup(), Some(36_000));
        let finished = RunEvent::Finished {
            text: "Booked.".to_owned(),
        };
        let effects = run(&mut registry, 7_000, first, finished);
        assert_eq!(
            kinds(&effects),
            [&TaskEventKind::Completed {
                text: "Booked.".to_owned(),
                effects: booked.clone(),
            }]
        );
        assert_eq!(registry.next_wakeup(), None);

        // An answer that does not come in that time: the run is stopped and the task ends
        // with what it really did.
        let (mut registry, first) = with_operation_in_flight();
        registry.apply(5_000, HUNG_UP);
        let committed = RunEvent::Committed {
            label: "table booking".to_owned(),
        };
        run(&mut registry, 6_000, first, committed);
        let effects = registry.apply(36_000, TaskCmd::Tick);
        assert_eq!(cancelled_runs(&effects), [first]);
        assert_eq!(
            kinds(&effects),
            [&TaskEventKind::Cancelled {
                by: "system",
                effects: booked,
            }]
        );
        assert_eq!(registry.next_wakeup(), None);

        // Without a report the task ends as unknown after 30 s.
        let (mut registry, first) = with_operation_in_flight();
        registry.apply(5_000, HUNG_UP);
        let effects = registry.apply(35_000, TaskCmd::Tick);
        assert_eq!(cancelled_runs(&effects), [first]);
        let unknown = events(&effects)[0];
        assert_eq!(unknown.state, TaskState::Unknown);
        assert!(matches!(
            &unknown.kind,
            TaskEventKind::Failed {
                effects: Effects::Unknown,
                ..
            }
        ));
    }

    fn finished(text: &str) -> RunEvent {
        RunEvent::Finished {
            text: text.to_owned(),
        }
    }

    #[test]
    fn a_lookup_runs_on_after_the_session_while_its_result_has_a_reader() {
        let lookups = || {
            let mut cfg = InteractionConfig::default();
            cfg.tasks.tools = vec![tool("search", false, false)];
            Registry::new(&cfg)
        };
        // Nobody can read it: cancelled, as in the call.
        let mut registry = lookups();
        let (first, _) = delegate(&mut registry, 1_000, 1);
        let effects = registry.apply(5_000, HUNG_UP);
        assert_eq!(cancelled_runs(&effects), [first]);
        assert_eq!(registry.board().tasks[0].state, TaskState::Cancelled);

        // A push address reads it: the run goes on, for up to `orphan_max_s`, to its answer.
        let mut registry = lookups();
        let (first, _) = delegate(&mut registry, 1_000, 1);
        let pushed = TaskCmd::SessionEnded {
            push: true,
            grace_ms: 0,
        };
        let effects = registry.apply(5_000, pushed);
        assert!(effects.is_empty(), "{effects:?}");
        assert_eq!(registry.next_wakeup(), Some(5_000 + 1_800_000));
        let effects = run(&mut registry, 20_000, first, finished("It opens at nine."));
        assert_eq!(
            kinds(&effects),
            [&TaskEventKind::Completed {
                text: "It opens at nine.".to_owned(),
                effects: Effects::NoneCommitted,
            }]
        );
        assert_eq!(registry.next_wakeup(), None);

        // The caller of a lost connection reads it for the grace; then it is stopped.
        let mut registry = lookups();
        let (first, _) = delegate(&mut registry, 1_000, 1);
        let lost = TaskCmd::SessionEnded {
            push: false,
            grace_ms: 8_000,
        };
        assert!(registry.apply(5_000, lost).is_empty());
        assert_eq!(registry.next_wakeup(), Some(13_000));
        assert!(registry.apply(12_999, TaskCmd::Tick).is_empty());
        let effects = registry.apply(13_000, TaskCmd::Tick);
        assert_eq!(cancelled_runs(&effects), [first]);
        assert_eq!(
            kinds(&effects),
            [&TaskEventKind::Cancelled {
                by: "system",
                effects: Effects::NoneCommitted,
            }]
        );
    }

    #[test]
    fn a_task_that_may_change_something_runs_on_and_is_stopped_at_orphan_max() {
        // `auto`: the session's tasks may book, so they run on with nobody to read them;
        // the one that waits for the caller's yes is told nobody answered.
        let mut registry = registry();
        registry.orphan_max_ms = 60_000;
        let (first, _) = delegate(&mut registry, 1_000, 1);
        let (second, _) = delegate(&mut registry, 1_100, 2);
        let (event, mut decision) = gate(4);
        run(&mut registry, 1_500, second, event);
        assert!(registry.board().open_question.is_some());
        let effects = registry.apply(5_000, HUNG_UP);
        assert!(cancelled_runs(&effects).is_empty());
        assert!(matches!(
            kinds(&effects)[..],
            [TaskEventKind::InputResolved {
                outcome: HitlOutcome::Timeout,
                by: AnsweredBy::SessionEnd,
                ..
            }]
        ));
        assert_eq!(decision.try_recv(), Ok(GateDecision::Refuse("timeout")));
        assert_eq!(registry.next_wakeup(), Some(65_000));
        // Nothing new is asked, and nothing new starts.
        let (again, mut refused) = gate(5);
        run(&mut registry, 6_000, second, again);
        assert_eq!(refused.try_recv(), Ok(GateDecision::Refuse("cancelled")));
        run(&mut registry, 7_000, first, finished("Done."));
        // `orphan_max_s` is over: the run is stopped and says what it did.
        let effects = registry.apply(65_000, TaskCmd::Tick);
        assert_eq!(cancelled_runs(&effects), [second]);
        assert_eq!(
            kinds(&effects),
            [&TaskEventKind::Cancelled {
                by: "system",
                effects: Effects::NoneCommitted,
            }]
        );
        let states: Vec<_> = registry.board().tasks.iter().map(|t| t.state).collect();
        assert_eq!(states, [TaskState::Completed, TaskState::Cancelled]);

        // `continue_all`: a queued task starts when a slot frees after the call.
        let mut registry = agent_registry();
        registry.session_end = SessionEnd::ContinueAll;
        let (first, _) = delegate(&mut registry, 1_000, 1);
        registry.apply(
            1_100,
            TaskCmd::Delegate {
                turn: TurnId(2),
                text: "and the other one".to_owned(),
                adopt: false,
            },
        );
        assert!(registry.apply(5_000, HUNG_UP).is_empty());
        let effects = run(&mut registry, 6_000, first, finished("Done."));
        assert_eq!(started(&effects).len(), 1, "the queued task starts");
        assert_eq!(registry.board().tasks[1].state, TaskState::Working);

        // `cancel_all`: nothing runs on.
        let mut registry = registry_with(SessionEnd::CancelAll);
        let (first, _) = delegate(&mut registry, 1_000, 1);
        let pushed = TaskCmd::SessionEnded {
            push: true,
            grace_ms: 60_000,
        };
        let effects = registry.apply(5_000, pushed);
        assert_eq!(cancelled_runs(&effects), [first]);
        assert_eq!(registry.next_wakeup(), None);
    }

    fn registry_with(session_end: SessionEnd) -> Registry {
        let mut registry = registry();
        registry.session_end = session_end;
        registry
    }

    #[test]
    fn an_effect_of_a_replaced_run_is_still_recorded() {
        let (mut registry, first) = with_operation_in_flight();
        let revise = TaskCmd::Revise {
            task: t(1),
            correction: "make it six".to_owned(),
        };
        registry.apply(4_200, revise);
        let committed = RunEvent::Committed {
            label: "table booking".to_owned(),
        };
        let effects = run(&mut registry, 4_500, first, committed);
        // Reported as the outcome of the revision it was made under; the task goes on.
        let stale = events(&effects)[0];
        let label = "table booking".to_owned();
        assert_eq!(
            stale.kind,
            TaskEventKind::StaleOutcome {
                label: label.clone()
            }
        );
        let task = &registry.board().tasks[0];
        assert_eq!((stale.revision, task.revision), (1, 2));
        assert_eq!(task.effects, Effects::Committed(vec![label]));
        assert_eq!(task.state, TaskState::Working);
    }

    #[test]
    fn a_cancel_after_a_revision_waits_for_the_call_that_was_on_the_wire() {
        let revise = || TaskCmd::Revise {
            task: t(1),
            correction: "make it six".to_owned(),
        };
        let (mut registry, first) = with_operation_in_flight();
        registry.apply(4_200, revise());
        // "Never mind": the replaced run's call may still land, so nothing is claimed yet.
        let cancel = TaskCmd::Cancel {
            task: t(1),
            by: "user",
        };
        let effects = registry.apply(4_300, cancel);
        assert_eq!(kinds(&effects), [&TaskEventKind::CancelRequested]);
        assert_eq!(registry.next_wakeup(), Some(14_300));
        // It lands: the cancel is confirmed with what the call did.
        let label = "table booking".to_owned();
        let committed = RunEvent::Committed {
            label: label.clone(),
        };
        let effects = run(&mut registry, 4_500, first, committed);
        assert_eq!(
            kinds(&effects),
            [
                &TaskEventKind::StaleOutcome {
                    label: label.clone()
                },
                &TaskEventKind::Cancelled {
                    by: "user",
                    effects: Effects::Committed(vec![label]),
                }
            ]
        );
        assert_eq!(registry.next_wakeup(), None);

        // The same at the end of the session; here the tool refused, so nothing was done.
        let (mut registry, first) = with_operation_in_flight();
        registry.apply(4_200, revise());
        let effects = registry.apply(5_000, HUNG_UP);
        assert_eq!(kinds(&effects), [&TaskEventKind::CancelRequested]);
        let effects = run(&mut registry, 5_200, first, RunEvent::Cancelled);
        assert_eq!(
            kinds(&effects),
            [&TaskEventKind::Cancelled {
                by: "system",
                effects: Effects::NoneCommitted,
            }]
        );
    }

    #[test]
    fn the_title_is_the_start_of_the_request() {
        assert_eq!(
            title("  What's my baggage allowance?  "),
            "What's my baggage allowance"
        );
        let long = "please find me a flight from Boston to Denver next Friday morning before nine";
        assert_eq!(
            title(long),
            "please find me a flight from Boston to Denver next Friday"
        );
        assert_eq!(title(&"x".repeat(100)).chars().count(), 60);
    }
}
