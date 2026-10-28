//! Interaction mode when a speech-to-speech provider owns the voice (plan sections 4.2 and
//! 9.2): regime C, where the provider converses and WaaV is its background brain, and regime
//! B, where the provider speaks and WaaV's policy decides the words that bind.
//!
//! The provider gets one function, [`DELEGATE_TOOL`], and a few lines of instructions. When it
//! calls the function the session runs the request as a task, with the same registry, tool
//! gate, policy templates and backends as the cascade. The result goes back as the call's
//! output; where the exact words matter (a confirmation, a result in regime B or one that
//! committed something) the gateway says them itself through the executor's speech slot:
//! `say_exact` where the provider has it, else guided speech whose words are checked.
//!
//! What the provider says on its own is read from its transcript. A sentence that claims
//! something was done, or promises something, that no task of the session supports, and a
//! number nobody gave it, is one `unverified_claim`; with `speech.claim_policy: correct` the
//! policy's template corrects it once per response.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::auth::SessionCredential;
use crate::core::llm::{ChatMessage, trim_history};

use super::actor::{self, Background, Msg};
use super::background::{Backend, Job};
use super::clips;
use super::config::{FrontModelServer, InteractionConfig, ServerInteraction};
use super::events::{self, WireEvent};
use super::executor::{ExecHandle, Verb};
use super::front_model;
use super::front_supervisor;
use super::guard;
use super::health::{self, LineKind, Service, Watcher};
#[cfg(test)]
use super::hitl;
use super::hitl::Parsed;
use super::persona::{self, Persona};
use super::tasks::{self, Registry};
use super::types::{
    Act, ActDecision, AnsweredBy, Effects, FrontEvent, HitlId, HitlKind, HitlOutcome, HitlRequest,
    Kind, Ms, Outcome, Reading, ReadingKind, RunId, Say, TaskCmd, TaskEffect, TaskEvent,
    TaskEventKind, TaskId, TaskState, TurnId,
};

/// The function a provider calls to hand a request to WaaV.
pub const DELEGATE_TOOL: &str = "waav_delegate";

/// The conversation a background run gets: the newest messages of the session, trimmed in
/// blocks to between half of this and this.
const HISTORY_KEPT: usize = 40;
/// The numbers the provider may repeat besides its instructions' own: the newest ones said.
const SOURCE_NUMBERS_KEPT: usize = 512;

/// The delegate function's description and JSON-schema parameters.
pub fn delegate_tool() -> (&'static str, Value) {
    (
        "Hand the caller's request to the back office, which looks things up and does things \
         for the caller. Call it for every request to find something out or to change \
         something, with the request in the caller's words. Its result says what happened.",
        json!({
            "type": "object",
            "properties": {
                "request": {
                    "type": "string",
                    "description": "The caller's request, in the caller's words."
                }
            },
            "required": ["request"]
        }),
    )
}

/// What the gateway adds to the provider's instructions in each regime.
pub fn instructions(regime: Regime) -> &'static str {
    match regime {
        Regime::Delegating => {
            "For anything the caller wants looked up or done, call the function waav_delegate \
             with their request and keep talking with them while it runs. Never say that \
             something was done, booked, sent, cancelled or changed unless a waav_delegate \
             result says so, and never make up numbers, times or prices. When a result \
             arrives, tell the caller what it says."
        }
        Regime::ProviderSpeech => {
            "For anything the caller wants looked up or done, call the function waav_delegate \
             with their request. Never say that something was done, booked, sent, cancelled \
             or changed unless a waav_delegate result says so, and never make up numbers, \
             times or prices. When a result says it was read to the caller, do not repeat it."
        }
    }
}

/// The session's own instructions with the regime's `suffix` after them, once.
pub fn with_instructions(own: Option<&str>, suffix: &str) -> String {
    match own {
        Some(own) if own.contains(suffix) => own.to_owned(),
        Some(own) if !own.trim().is_empty() => format!("{own}\n\n{suffix}"),
        _ => suffix.to_owned(),
    }
}

/// The regime a provider session runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Regime {
    /// B: the provider speaks, the gateway's policy decides the words that bind.
    ProviderSpeech,
    /// C: the provider converses; the gateway is its background.
    Delegating,
}

impl Regime {
    pub fn as_str(self) -> &'static str {
        match self {
            Regime::ProviderSpeech => "provider_speech",
            Regime::Delegating => "delegating",
        }
    }
}

/// What the resolver decided for the session, as the session uses it.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub regime: Regime,
    /// The gateway acknowledges a delegated request (`filler_owner: gateway`, regime B).
    pub gateway_ack: bool,
    /// The speech path of a confirmation of an irreversible action, and of anything else
    /// asked by voice (the resolver's `confirm_irreversible` and `confirm_reversible`).
    pub confirm_irreversible: String,
    pub confirm_reversible: String,
    /// The provider speaks given words verbatim (`say_exact` has a wire).
    pub exact: bool,
    /// The gateway can have the provider say a line at all (`say_exact` or `say_guided`).
    pub can_say: bool,
    /// `speech.claim_policy: correct`.
    pub correct_claims: bool,
    /// The half-cascade (`speech.voice_owner: gateway`): the provider listens and writes its
    /// replies as text, and the gateway checks each sentence before its own voice says it.
    pub half: bool,
    /// The provider's delegate calls are provisional for this long (chapter 6, provider
    /// speculation): adopted when its turn ends, dropped unseen when it withdraws one.
    pub adopt_by: Option<Ms>,
    /// Delegate results carry the provider's delivery `scheduling` (Gemini, provider fix
    /// `gemini_response_scheduling`).
    pub scheduling: bool,
    /// The gateway's lines are announced with `audio_segment` (every surface but `/realtime`).
    pub segments: bool,
}

impl Plan {
    /// The plan of a resolution that turned the mode on in regime B or C, or the cascade's
    /// half-cascade variant over the provider; `None` for any other.
    pub fn of(resolution: &Value, verbs: [bool; 4], correct_claims: bool) -> Option<Plan> {
        let half = resolution["variant"] == "half_cascade";
        let regime = match resolution["regime"].as_str()? {
            "ProviderSpeech" => Regime::ProviderSpeech,
            "Delegating" => Regime::Delegating,
            // The gateway says every result itself, as in B.
            "Cascade" if half => Regime::ProviderSpeech,
            _ => return None,
        };
        let text = |key: &str| resolution[key].as_str().unwrap_or_default().to_owned();
        Some(Plan {
            regime,
            gateway_ack: regime == Regime::ProviderSpeech
                && resolution["filler_owner"] == "gateway",
            confirm_irreversible: text("confirm_irreversible"),
            confirm_reversible: text("confirm_reversible"),
            exact: verbs[0] && !half,
            can_say: verbs[0] || verbs[1] || half,
            correct_claims: correct_claims || half,
            half,
            adopt_by: None,
            scheduling: false,
            segments: false,
        })
    }
}

/// What the provider did, from its callbacks.
#[derive(Debug, Clone, PartialEq)]
pub enum ProviderEvent {
    /// The caller's words, final.
    Heard(String),
    /// The provider's words of one output item, final.
    Said(String),
    /// More of the provider's words of the response in progress, as they come.
    Saying(String),
    Call {
        call_id: String,
        name: String,
        arguments: String,
    },
    ResponseDone,
    /// The provider withdrew a call it made (Gemini `toolCallCancellation`, provider fix
    /// `gemini_tool_call_cancellation`).
    CallCancelled(String),
    /// The caller started (`true`) or stopped speaking, by the provider's (or the gateway's)
    /// voice activity detection.
    CallerSpeaking(bool),
    /// The provider stopped its turn for the caller (Gemini `interrupted`): a provisional
    /// call it does not withdraw soon after is the caller's.
    Interrupted,
    /// The provider's instructions as the client set them (the relay): their numbers are the
    /// provider's to say.
    Instructions(String),
    /// The provider reported an error (chapter 10: the voice may be failing).
    Failed(String),
    /// The provider's connection is back after it was lost.
    Reconnected,
    /// The provider's connection is lost for good: the session ends with the exit line.
    Gone(String),
}

/// Where the session's own audio goes: into the output the provider's audio takes to the
/// caller (the socket, or the room), as PCM16 at the provider's output rate.
pub type AudioOut = Box<dyn Fn(bytes::Bytes) + Send + Sync>;

/// What the session works with.
pub struct ProviderDeps {
    pub plan: Plan,
    pub cfg: InteractionConfig,
    pub server: Arc<ServerInteraction>,
    pub background: Background,
    pub system_prompt: Option<String>,
    pub credential: Option<SessionCredential>,
    pub exec: ExecHandle,
    /// One client event, as JSON.
    pub out: Box<dyn Fn(String) + Send + Sync>,
    pub stream_id: String,
    pub owner: String,
    /// The provider's instructions: numbers in them are the provider's to say.
    pub instructions: String,
    /// Where the lines said when the provider fails are played (chapter 10); `None`: the
    /// surface plays none (the session still ends with `session_failed`).
    pub audio: Option<AudioOut>,
    /// The provider's output sample rate.
    pub output_rate: u32,
    /// The gateway's own voice (`speech.voice`, else the server's `failure_voice`): the
    /// half-cascade's speech and the gateway's clips. `None`: the session has none.
    pub voice: Option<clips::Render>,
    /// Where the gateway's voice plays when not into `audio` (the relay: as responses of the
    /// gateway's own that say so).
    pub voice_audio: Option<AudioOut>,
    /// The room's caller line: whether the caller is on a phone (a keypad).
    pub phone: Option<Arc<crate::core::audio::playback_pump::PumpControl>>,
}

/// Start the session. `tx` is the sender of `inbox`: the executor, the provider's callbacks
/// and the background runs report on it. The session ends at `Msg::Shutdown`.
pub fn spawn(
    deps: ProviderDeps,
    tx: mpsc::Sender<Msg>,
    inbox: mpsc::Receiver<Msg>,
) -> JoinHandle<()> {
    let persona = deps.cfg.front.load_persona();
    let mut registry = Registry::new(&deps.cfg);
    registry.set_sensitive(persona.sensitive_categories().to_vec());
    let background = actor::backend(
        deps.background,
        &deps.cfg,
        &deps.server,
        &persona,
        deps.system_prompt,
        &deps.stream_id,
        None,
        Vec::new(),
    );
    registry.set_owner(&deps.owner);
    registry.set_backend(
        background.label(),
        background.own_tools(),
        background.max_concurrent(),
    );
    let (voiced, rendered) = mpsc::unbounded_channel();
    let session = Session {
        voice: deps.voice,
        voice_audio: deps.voice_audio,
        phone: deps.phone,
        speaker: Speaker::default(),
        voiced,
        saying: String::new(),
        judged: 0,
        reported: HashSet::new(),
        provisional: None,
        asked_question: false,
        front: deps.server.front_model.clone(),
        lane: front_model::Lane::take(),
        front_down: None,
        readings: HashMap::new(),
        next_reading: 0,
        latest: None,
        stored: tasks::Session {
            owner: deps.owner,
            id: deps.stream_id.clone(),
            push_url: None,
            grace: Duration::ZERO,
            resume: None,
        },
        plan: deps.plan,
        persona,
        registry,
        background,
        exec: deps.exec,
        out: deps.out,
        tx,
        stream_id: deps.stream_id,
        credential: deps.credential,
        t0: Instant::now(),
        runs: HashMap::new(),
        calls: HashMap::new(),
        seen: HashSet::new(),
        delegated: HashSet::new(),
        call: None,
        next_turn: 0,
        next_line: 0,
        lines: HashMap::new(),
        current: None,
        said: String::new(),
        asked: None,
        answered: None,
        history: Vec::new(),
        sources: Sources::new(&deps.instructions),
        corrected: false,
        failure: Failure::new(&deps.cfg, deps.audio, deps.output_rate),
        cfg: deps.cfg,
    };
    tokio::spawn(session.run(inbox, rendered))
}

/// How the words of a gateway line are checked against the provider's transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Check {
    /// Word for word (`say_exact`).
    Exact,
    /// Every number and content word of the key words, and no other number (guided).
    Entities,
}

#[derive(Debug, Clone, PartialEq)]
enum Purpose {
    Question(HitlId),
    Relay,
    Correction,
    Ack,
}

#[derive(Debug, Clone)]
struct Line {
    words: String,
    /// The words that must be heard; the whole line for a word-for-word check.
    keys: String,
    check: Check,
    purpose: Purpose,
}

/// The open question and how far it got.
struct Asked {
    request: HitlRequest,
    /// Its line: `None` when it is on the client's screen only.
    line: Option<u64>,
    /// The provider said it, and its words passed the check.
    heard: bool,
    /// Its words failed the check: a spoken answer does not count.
    void: bool,
    reasked: bool,
    /// The keys typed for it so far (`hitl.channels: dtmf`).
    keys: String,
}

struct Session {
    plan: Plan,
    cfg: InteractionConfig,
    persona: Arc<Persona>,
    registry: Registry,
    background: Arc<dyn Backend>,
    exec: ExecHandle,
    out: Box<dyn Fn(String) + Send + Sync>,
    tx: mpsc::Sender<Msg>,
    stream_id: String,
    credential: Option<SessionCredential>,
    stored: tasks::Session,
    t0: Instant,
    runs: HashMap<RunId, CancellationToken>,
    /// The delegate call each task answers, and the connection it came on.
    calls: HashMap<TaskId, (String, u64)>,
    /// The client's `task_seen`: tasks whose result the caller read on the screen.
    seen: HashSet<TaskId>,
    /// Every delegate call's id: a withdrawn one is never the client's.
    delegated: HashSet<String>,
    /// The call of the task being created.
    call: Option<(String, u64)>,
    next_turn: u64,
    next_line: u64,
    lines: HashMap<u64, Line>,
    /// The gateway line whose response is playing.
    current: Option<u64>,
    /// The provider's words of the response in progress.
    said: String,
    asked: Option<Asked>,
    /// The question answered last, and when: the provider may pass the caller's answer on
    /// as a request.
    answered: Option<(HitlRequest, Ms)>,
    /// The newest messages, at most [`HISTORY_KEPT`] (trimmed in blocks).
    history: Vec<ChatMessage>,
    /// What the provider may repeat: its instructions, the caller's words, results and the
    /// gateway's own lines.
    sources: Sources,
    /// A correction was spoken for the response in progress.
    corrected: bool,
    failure: Failure,
    voice: Option<clips::Render>,
    voice_audio: Option<AudioOut>,
    phone: Option<Arc<crate::core::audio::playback_pump::PumpControl>>,
    speaker: Speaker,
    /// Where a render of the gateway's voice reports: `(cut, line, audio)`.
    voiced: mpsc::UnboundedSender<Rendered>,
    /// The provider's words of the response in progress as they came, and how many of their
    /// sentences were judged.
    saying: String,
    judged: usize,
    /// Sentences of the response in progress judged before its end.
    reported: HashSet<String>,
    /// A delegate call that waits for its adoption.
    provisional: Option<Provisional>,
    /// The provider's last words asked the caller something.
    asked_question: bool,
    /// The session's fast front model (chapter 9; owner, 24 Oct: it reads the caller's answer,
    /// a cancel and a continuer, no word list does), its lane, and the generation it was lost at.
    front: Option<FrontModelServer>,
    lane: front_model::Lane,
    front_down: Option<u64>,
    /// The front's readings out, by the turn id each was asked under.
    readings: HashMap<TurnId, Pending>,
    next_reading: u64,
    /// The reading of the caller's latest turn, while it may still say it was a continuer.
    latest: Option<TurnId>,
}

/// A delegate call whose request waits for the front's reading.
#[derive(Debug, Clone)]
struct Call {
    call_id: String,
    epoch: u64,
    request: String,
}

/// What a front reading of the caller's words is for.
#[derive(Debug, Clone)]
enum Pending {
    /// An answer to `request`: `apply` where the spoken answer counts (the caller heard the
    /// question as it binds), else only whether the words answer it (a delegate call that
    /// passes on the answer just given). `then_turn`: the words are read as a turn next when
    /// they answer nothing.
    Answer {
        request: Box<HitlRequest>,
        said: String,
        apply: bool,
        call: Option<Call>,
        then_turn: bool,
    },
    /// What a turn means: a cancel of the live task, or (`continuer`) a continuer the
    /// provider is not expected to answer.
    Turn {
        said: String,
        continuer: bool,
        call: Option<Call>,
    },
}

/// A delegate call before its adoption (chapter 6, provider speculation).
struct Provisional {
    call_id: String,
    epoch: u64,
    turn: TurnId,
    request: String,
    adopt_at: Ms,
    /// Its run started (a backend that runs its own tools waits for the adoption).
    started: bool,
}

/// One line said in the gateway's own voice.
#[derive(Debug, Clone)]
struct Voiced {
    id: u64,
    words: String,
    /// The gateway line it is (`lines`); `None`: a sentence of the provider's (half-cascade).
    line: Option<u64>,
    kind: Kind,
    /// A gateway clip over a provider that speaks: it waits for the provider to be quiet, and
    /// the provider's audio is held while it plays.
    clip: bool,
}

/// A render's result: the cut it began under, the line, its audio.
type Rendered = (u64, u64, Result<crate::core::tts::AudioData, String>);

/// The gateway's voice: one line rendered at a time, played in order.
#[derive(Default)]
struct Speaker {
    queue: VecDeque<Voiced>,
    rendering: Option<Voiced>,
    /// The lines playing, each with when it ends.
    playing: VecDeque<(Ms, Voiced)>,
    until: Ms,
    /// One more at each cut: a render begun before it is dropped.
    cut: u64,
    next: u64,
}

/// Chapter 10 on a speech-to-speech session: the provider is the session's voice, its ears
/// and its front, so its errors, a lost connection and a caller's turn it does not answer are
/// one service failing (`voice`). The lines are the server's default set (the operator's
/// `interaction.failure_voice`), played into the session's output audio at a gap.
struct Failure {
    watcher: Watcher,
    audio: Option<AudioOut>,
    rate: u32,
    /// The caller ended a turn then, and the provider has not answered since.
    waiting: Option<Ms>,
    /// The provider fails since then: anything it does after it mends it.
    since: Option<Ms>,
    /// The provider's last words, call or response end (its audio is the executor's clock).
    active_at: Option<Ms>,
    caller_speaking: bool,
    caller_quiet_since: Option<Ms>,
    /// What the caller's turn in progress asks of the provider: `Some(false)` when its words
    /// were only a continuer the provider may leave unanswered.
    turn_expected: Option<bool>,
    /// The failure line playing until then.
    playing_until: Ms,
    /// The `audio_segment` of the failure line playing, and when it ends.
    segment: Option<(String, Ms)>,
    degraded: bool,
    /// Why the provider is gone, when it is.
    gone: Option<String>,
    /// The exit line was said: the session fails at this time.
    exiting: Option<Ms>,
    failed: bool,
}

impl Failure {
    fn new(cfg: &InteractionConfig, audio: Option<AudioOut>, rate: u32) -> Self {
        Failure {
            watcher: Watcher::new(cfg.failure.max_wait_s, cfg.failure.follow_up),
            audio,
            rate,
            waiting: None,
            since: None,
            active_at: None,
            caller_speaking: false,
            caller_quiet_since: None,
            turn_expected: None,
            playing_until: 0,
            segment: None,
            degraded: false,
            gone: None,
            exiting: None,
            failed: false,
        }
    }
}

/// A failure line starts only once the caller has been silent this long.
const FAILURE_PAUSE_MS: Ms = 500;
/// A gateway clip that waits for the provider to be quiet looks again this often.
const CLIP_POLL_MS: Ms = 100;
/// The registry keeps a provisional run this long past the session's own adoption time.
const PROVISIONAL_SLACK_MS: Ms = 1_000;
/// A provisional call the provider does not withdraw this long after it stopped its turn for
/// the caller is adopted (plan chapter 6: Gemini, 300 ms after `interrupted`).
const ADOPT_AFTER_INTERRUPTED_MS: Ms = 300;
/// The exit line ends the session this long after it played out.
const EXIT_SLACK_MS: Ms = 500;

impl Session {
    fn now(&self) -> Ms {
        u64::try_from(self.t0.elapsed().as_millis()).unwrap_or(Ms::MAX)
    }

    async fn run(
        mut self,
        mut inbox: mpsc::Receiver<Msg>,
        mut rendered: mpsc::UnboundedReceiver<Rendered>,
    ) {
        info!(stream_id = %self.stream_id, regime = self.plan.regime.as_str(), half = self.plan.half, "interaction provider session started");
        // The persona's prompt into the lane's decisions' slot before the caller's first turn,
        // as a cascade session's warm-up does: read cold, a first turn's reading (the whole
        // prompt, about 3,000 tokens) took 1.2 to over 1.5 s on the loaded GPU, past its budget
        // (live SS2, r15: "Uh-huh." 1.4 s into the call was not read, and a stall line played).
        if let Some(server) = self.front.clone() {
            let persona = self.persona.clone();
            tokio::spawn(front_model::LANE.scope(self.lane.0, async move {
                front_model::warm(&server, &persona).await;
            }));
        }
        loop {
            let wake = self.wakeup();
            let msg = tokio::select! {
                msg = inbox.recv() => msg,
                Some((cut, id, audio)) = rendered.recv() => {
                    self.rendered(cut, id, audio);
                    let now = self.now();
                    self.voice_tick(now);
                    continue;
                }
                () = tokio::time::sleep_until(wake.unwrap_or_else(Instant::now)), if wake.is_some() => {
                    let now = self.now();
                    self.apply(TaskCmd::Tick);
                    self.expire(now);
                    self.adopt_due(now);
                    self.voice_tick(now);
                    self.health(now);
                    continue;
                }
            };
            match msg {
                None | Some(Msg::Shutdown { .. }) => break,
                Some(msg) => self.on(msg),
            }
            let now = self.now();
            self.voice_tick(now);
            self.health(now);
        }
        // Nobody comes back to a provider session, and it has no push address.
        self.apply(TaskCmd::SessionEnded {
            push: false,
            grace_ms: 0,
        });
        // A task whose operation is on the wire finishes, as the `/ws` actor's do: by its
        // run's report, or by the registry's deadline. The runs it did not keep are cancelled.
        while let Some(deadline) = self.registry.next_wakeup() {
            let at = self.t0 + Duration::from_millis(deadline);
            tokio::select! {
                msg = inbox.recv() => match msg {
                    Some(msg @ Msg::Run(..)) => self.on(msg),
                    Some(_) => {}
                    None => break,
                },
                () = tokio::time::sleep_until(at) => self.apply(TaskCmd::Tick),
            }
        }
        for cancel in self.runs.values() {
            cancel.cancel();
        }
        info!(stream_id = %self.stream_id, "interaction provider session ended");
    }

    fn wakeup(&self) -> Option<Instant> {
        let question = self.asked.as_ref().map(|asked| asked.request.expires_at);
        let adopt = self.provisional.as_ref().map(|p| p.adopt_at);
        let next = [
            self.registry.next_wakeup(),
            question,
            self.health_wakeup(),
            adopt,
            self.voice_wakeup(),
        ]
        .into_iter()
        .flatten()
        .min()?;
        Some(self.t0 + Duration::from_millis(next))
    }

    /// An open question nobody answered in time.
    fn expire(&mut self, now: Ms) {
        if let Some(asked) = &self.asked
            && now >= asked.request.expires_at
        {
            let request = asked.request.id.clone();
            self.apply(TaskCmd::Answer {
                request: request.clone(),
                outcome: HitlOutcome::Timeout,
                by: AnsweredBy::Timeout,
            });
            // A question the registry no longer holds is over all the same.
            if self
                .asked
                .as_ref()
                .is_some_and(|asked| asked.request.id == request)
            {
                self.asked = None;
            }
        }
    }

    fn on(&mut self, msg: Msg) {
        match msg {
            Msg::Provider(event) => self.on_provider(event),
            // The provider speaks for itself: no control of the conversation is the gateway's
            // to apply here (the call goes on, so "the call has ended" would be untrue).
            Msg::Run(_, super::types::RunEvent::Control { control, reply }) => {
                let _ = reply.send(s2s_control_refused(&control));
            }
            Msg::Run(run, event) => {
                if event.is_last() {
                    self.runs.remove(&run);
                }
                self.apply(TaskCmd::Run { run, event });
            }
            Msg::TaskAnswer {
                request_id,
                outcome,
            } => self.apply(TaskCmd::Answer {
                request: HitlId(request_id),
                outcome,
                by: AnsweredBy::Client,
            }),
            Msg::TaskCancel { task_id } => {
                let task = self
                    .registry
                    .board()
                    .tasks
                    .into_iter()
                    .find(|t| t.id.0 == task_id);
                match task {
                    Some(task) => self.apply(TaskCmd::Cancel {
                        task: task.task_ref,
                        by: "client",
                    }),
                    None => self.send(&WireEvent::Error {
                        message: format!("No task {task_id} in this session."),
                        code: "unknown_task".to_owned(),
                    }),
                }
            }
            Msg::LineSent { line } => {
                self.current = Some(line);
                self.said.clear();
                self.segment_of(line, "start");
            }
            Msg::LineEnded { line, spoken } => {
                self.segment_of(line, "end");
                self.line_ended(line, spoken);
            }
            // Unknown ids are ignored, as on `/ws`.
            Msg::TaskSeen { task_id } => {
                self.seen.insert(TaskId(task_id));
            }
            Msg::Update(patch) => self.update(&patch),
            Msg::Dtmf(key) => self.key(key),
            Msg::Control(control) => self.control(control),
            // The front's readings of the caller's words.
            Msg::Read { turn, label, .. } => self.on_read(turn, label),
            Msg::Front(FrontEvent::Done { turn, decision }) => self.on_act(turn, Some(decision)),
            Msg::Front(FrontEvent::Failed { turn }) => self.on_act(turn, None),
            other => debug!(?other, "not a provider session message"),
        }
    }

    fn on_provider(&mut self, event: ProviderEvent) {
        match event {
            ProviderEvent::Heard(text) if !text.trim().is_empty() => {
                let now = self.now();
                // The caller's turn ends with its words too (a provider may send no speech
                // events), unless the provider answered it already: its transcript may come
                // after its answer. A caller's "uh-huh" or "okay" asks nothing: the provider
                // is not expected to answer it (it may ignore it on purpose), unless it had
                // just asked something; the front reads whether it is one (`on_act`).
                let stall = Ms::from(self.cfg.failure.stall_after_ms);
                if !self.provider_active_since(now.saturating_sub(stall)) {
                    self.failure.waiting = Some(now);
                }
                self.failure.turn_expected = Some(true);
                // A provider that reports only the start of the caller's speech (Deepgram):
                // the turn's words end it.
                if std::mem::take(&mut self.failure.caller_speaking) {
                    self.failure.caller_quiet_since = Some(now);
                }
                self.remember(ChatMessage::user(text.clone()));
                self.sources.add(&text);
                let continuer = !self.asked_question;
                self.heard(&text, continuer);
            }
            ProviderEvent::Heard(_) => {}
            ProviderEvent::CallerSpeaking(speaking) => {
                let now = self.now();
                if speaking {
                    // The caller talks over the gateway's voice: it stops.
                    self.cut_voice(now, "barge_in");
                }
                let failure = &mut self.failure;
                failure.caller_speaking = speaking;
                if speaking {
                    failure.waiting = None;
                    failure.turn_expected = None;
                    // A new turn: the last one's continuer reading no longer applies.
                    self.latest = None;
                } else {
                    failure.caller_quiet_since = Some(now);
                    // A turn whose words were heard as a continuer is not waited on.
                    if failure.turn_expected != Some(false) {
                        failure.waiting = Some(now);
                    }
                }
            }
            ProviderEvent::Instructions(text) => {
                self.sources.instructions.extend(numbers(&text));
            }
            ProviderEvent::Interrupted => {
                let due = self.now() + ADOPT_AFTER_INTERRUPTED_MS;
                if let Some(provisional) = self.provisional.as_mut() {
                    provisional.adopt_at = provisional.adopt_at.min(due);
                }
            }
            ProviderEvent::Failed(reason) => {
                let now = self.now();
                let stall = Ms::from(self.cfg.failure.stall_after_ms);
                // A single error that the provider's next answer mends plays nothing.
                debug!(stream_id = %self.stream_id, %reason, "interaction provider error");
                self.failing(now, now + stall);
            }
            ProviderEvent::Reconnected => self.failure.active_at = Some(self.now()),
            ProviderEvent::Gone(reason) => {
                warn!(stream_id = %self.stream_id, %reason, "interaction provider gone");
                self.failure.watcher.gone(Service::Voice);
                self.failure.gone = Some(reason);
            }
            ProviderEvent::Saying(delta) => {
                self.failure.active_at = Some(self.now());
                self.saying(&delta);
            }
            ProviderEvent::Said(text) => {
                self.failure.active_at = Some(self.now());
                self.asked_question = text.trim_end().ends_with('?');
                if self.plan.half && self.current.is_none() {
                    self.half_said(&text);
                }
                if !self.said.is_empty() {
                    self.said.push(' ');
                }
                self.said.push_str(text.trim());
            }
            ProviderEvent::Call {
                call_id,
                name,
                arguments,
            } if name == DELEGATE_TOOL => {
                self.failure.active_at = Some(self.now());
                self.delegate(call_id, &arguments);
            }
            // The session's own function: the client answers it.
            ProviderEvent::Call { .. } => {}
            ProviderEvent::CallCancelled(call_id) => {
                // A provisional call withdrawn before its adoption was never the caller's:
                // nothing of it is seen.
                if self
                    .provisional
                    .as_ref()
                    .is_some_and(|p| p.call_id == call_id)
                {
                    if let Some(provisional) = self.provisional.take() {
                        info!(stream_id = %self.stream_id, call = %call_id, "interaction provisional call withdrawn");
                        if provisional.started {
                            self.apply(TaskCmd::DiscardProvisional {
                                reason: "withdrawn",
                            });
                        }
                    }
                    return;
                }
                self.withdrawn(&call_id);
            }
            ProviderEvent::ResponseDone => {
                self.failure.active_at = Some(self.now());
                // The provider's turn ended with its call standing: the call is the caller's.
                self.adopt();
                if self.current.is_none() {
                    let words = std::mem::take(&mut self.said);
                    if !words.is_empty() {
                        self.remember(ChatMessage::assistant(words.clone()));
                        // The half-cascade judged each sentence before it was said.
                        if !self.plan.half {
                            self.monitor(&words);
                        }
                    }
                    self.corrected = false;
                }
                self.saying.clear();
                self.judged = 0;
                self.reported.clear();
            }
        }
    }

    /// The caller's words: an answer to the open question the caller heard, else a cancel of
    /// the live task, or (`continuer`) a continuer. The session's front reads which (owner, 24
    /// Oct: no word list reads the caller's meaning); its answer comes back on the inbox.
    fn heard(&mut self, text: &str, continuer: bool) {
        // Only a question the caller heard as it binds takes a spoken answer.
        let open = self
            .asked
            .as_ref()
            .filter(|asked| asked.line.is_some() && asked.heard && !asked.void)
            .map(|asked| asked.request.clone());
        let pending = match open {
            Some(request) => Pending::Answer {
                request: Box::new(request),
                said: text.to_owned(),
                apply: true,
                call: None,
                then_turn: true,
            },
            None => Pending::Turn {
                said: text.to_owned(),
                continuer,
                call: None,
            },
        };
        if let Some(turn) = self.ask_front(pending) {
            self.latest = continuer.then_some(turn);
        }
    }

    /// Asks the session's front to read `pending`; the turn it is asked under, or `None` when
    /// there is nothing to read or the front is not ready (no fallback, chapter 9: nothing is
    /// taken from the words then).
    fn ask_front(&mut self, pending: Pending) -> Option<TurnId> {
        let live = self
            .registry
            .board()
            .tasks
            .iter()
            .any(|t| t.state.is_live());
        if let Pending::Turn {
            continuer, call, ..
        } = &pending
            && !continuer
            && !live
        {
            // Nothing to cancel and no continuer to hear: the call is a request.
            if let Some(call) = call.clone() {
                self.delegate_request(call.call_id, call.epoch, call.request);
            }
            return None;
        }
        // Test data, never a gateway path: a session of the unit tests without a front reads
        // the words with the lexicon, as the policy's tests do.
        #[cfg(test)]
        if self.front.is_none() {
            self.next_reading += 1;
            let turn = TurnId(self.next_reading);
            let board = self.registry.board();
            let read = lexicon_front(turn, &pending, self.persona.lexicon(), &board);
            let _ = self.tx.try_send(read);
            self.readings.insert(turn, pending);
            return Some(turn);
        }
        let Some(server) = self.front_ready() else {
            if let Pending::Answer {
                call: Some(call), ..
            }
            | Pending::Turn {
                call: Some(call), ..
            } = pending
            {
                self.delegate_request(call.call_id, call.epoch, call.request);
            }
            return None;
        };
        self.next_reading += 1;
        let turn = TurnId(self.next_reading);
        let tx = self.tx.clone();
        let budget = Duration::from_millis(self.cfg.front.total_budget_ms.into());
        let lane = self.lane.0;
        match &pending {
            Pending::Answer { request, said, .. } => {
                let question = self
                    .asked
                    .as_ref()
                    .filter(|asked| asked.request.id == request.id)
                    .and_then(|asked| asked.line)
                    .and_then(|line| self.lines.get(&line))
                    .map(|line| line.words.clone())
                    .or_else(|| request.question.clone())
                    .unwrap_or_default();
                let reading = Reading {
                    turn,
                    kind: ReadingKind::Reply,
                    context: front_model::reading_context("Agent", &question),
                    said: said.clone(),
                    labels: super::policy::answer_labels(request),
                };
                tokio::spawn(front_model::LANE.scope(lane, async move {
                    let asked = Instant::now();
                    let label = front_model::read(&server, &reading, budget).await;
                    let ms = u64::try_from(asked.elapsed().as_millis()).unwrap_or(u64::MAX);
                    let _ = tx.send(Msg::Read { turn, label, ms }).await;
                }));
            }
            Pending::Turn { said, .. } => {
                let board = self.registry.board();
                let agent = self
                    .history
                    .iter()
                    .rev()
                    .find(|message| message.role == crate::core::llm::MessageRole::Assistant)
                    .and_then(|message| message.content.clone());
                let request = front_model::relay_request(
                    &self.persona,
                    turn,
                    said,
                    &board,
                    agent.as_deref(),
                    self.now(),
                    self.cfg.front.total_budget_ms,
                    self.persona.call_header_for(&self.cfg.front),
                );
                let persona = self.persona.clone();
                tokio::spawn(front_model::LANE.scope(lane, async move {
                    let event = match front_model::act_of(&server, &persona, &request).await {
                        Some(decision) => FrontEvent::Done { turn, decision },
                        None => FrontEvent::Failed { turn },
                    };
                    let _ = tx.send(Msg::Front(event)).await;
                }));
            }
        }
        self.readings.insert(turn, pending);
        Some(turn)
    }

    /// The session's front model, when it is ready (the supervisor's gate, as on the cascade).
    fn front_ready(&mut self) -> Option<FrontModelServer> {
        let server = self.front.clone()?;
        match front_supervisor::gate(self.front_down, front_supervisor::status()) {
            front_supervisor::Gate::Use => Some(server),
            front_supervisor::Gate::Back => {
                self.front_down = None;
                info!(stream_id = %self.stream_id, "interaction front model is back");
                Some(server)
            }
            front_supervisor::Gate::Lost(generation) => {
                self.front_down = Some(generation);
                warn!(stream_id = %self.stream_id, "interaction front model unavailable: the caller's words are not read");
                None
            }
            front_supervisor::Gate::Wait => None,
        }
    }

    /// The front's label for an answer it read (`None`: none in its budget).
    fn on_read(&mut self, turn: TurnId, label: Option<String>) {
        let Some(Pending::Answer {
            request,
            said,
            apply,
            call,
            then_turn,
        }) = self.readings.remove(&turn)
        else {
            return;
        };
        let parsed = label.as_deref().map_or(Parsed::Unclear, |label| {
            super::policy::answer_of(&request, label, &said)
        });
        debug!(stream_id = %self.stream_id, turn = turn.0, ?label, "interaction provider answer read");
        // A delegate call passes on an answer to a confirmation (also a weak yes it does not
        // take): the call is not a request.
        let answers = matches!(
            parsed,
            Parsed::Accept { .. } | Parsed::Decline { .. } | Parsed::Cancel
        );
        let outcome = match parsed {
            Parsed::Accept { strong } if strong || !request.irreversible => {
                Some(HitlOutcome::Accept { value: None })
            }
            Parsed::Decline { correction } => Some(HitlOutcome::Decline { correction }),
            Parsed::Cancel => Some(HitlOutcome::Cancel),
            Parsed::Choice(n) => request.options.get(n).map(|o| HitlOutcome::Accept {
                value: Some(json!(o)),
            }),
            Parsed::Value(v) => Some(HitlOutcome::Accept {
                value: Some(json!(v)),
            }),
            Parsed::Accept { .. } | Parsed::Unclear => None,
        };
        let still_open = self.asked.as_ref().is_some_and(|asked| {
            asked.request.id == request.id && asked.line.is_some() && asked.heard && !asked.void
        });
        if apply
            && still_open
            && let Some(outcome) = outcome.clone()
        {
            self.apply(TaskCmd::Answer {
                request: request.id.clone(),
                outcome,
                by: AnsweredBy::Voice,
            });
        }
        match call {
            Some(call) if answers => {
                let output = self.scheduled(json!({"status": "answer_received"}), "SILENT");
                self.exec
                    .tool_result(call.call_id, call.epoch, output, false);
            }
            call if outcome.is_none() && then_turn => {
                let pending = Pending::Turn {
                    said,
                    continuer: false,
                    call,
                };
                self.ask_front(pending);
            }
            Some(call) => self.delegate_request(call.call_id, call.epoch, call.request),
            None => {}
        }
    }

    /// The front's act for a turn it read (`None`: none in its budget): an abort cancels the
    /// live task it names (the newest one when it names none), a `wait` (a continuer) is not
    /// waited on.
    fn on_act(&mut self, turn: TurnId, decision: Option<ActDecision>) {
        let Some(Pending::Turn {
            continuer, call, ..
        }) = self.readings.remove(&turn)
        else {
            return;
        };
        let act = decision.as_ref().map(|decision| decision.act);
        debug!(stream_id = %self.stream_id, turn = turn.0, ?act, "interaction provider turn read");
        let tasks = self.registry.board().tasks;
        let named = decision.as_ref().and_then(|decision| decision.task.clone());
        let live = tasks
            .iter()
            .filter(|t| t.state.is_live())
            .rev()
            .find(|t| named.as_ref().is_none_or(|named| *named == t.task_ref))
            .or_else(|| tasks.iter().rev().find(|t| t.state.is_live()));
        if act == Some(Act::Abort)
            && let Some(task) = live
        {
            info!(stream_id = %self.stream_id, task = %task.id.0, "the caller cancelled the task");
            self.apply(TaskCmd::Cancel {
                task: task.task_ref.clone(),
                by: "user",
            });
            if let Some(call) = call {
                let output = self.scheduled(json!({"status": "cancelling"}), "SILENT");
                self.exec
                    .tool_result(call.call_id, call.epoch, output, false);
            }
            return;
        }
        // A `wait` asks nothing of the provider: a continuer, or nothing new (the 2B front
        // names "Uh-huh." and "Okay." `wait` `nothing` as often as `continuer`, r14).
        if continuer && act == Some(Act::Wait) && self.latest == Some(turn) {
            self.failure.waiting = None;
            self.failure.turn_expected = Some(false);
        }
        if let Some(call) = call {
            self.delegate_request(call.call_id, call.epoch, call.request);
        }
    }

    /// The provider withdrew a call. A delegate call's task is cancelled (its effects, if any,
    /// are reported as the registry reports a cancel) and no result goes back on the call;
    /// any other call is the client's, which is told with `function_call_cancelled`.
    fn withdrawn(&mut self, call_id: &str) {
        if !self.delegated.contains(call_id) {
            (self.out)(json!({"type": "function_call_cancelled", "call_id": call_id}).to_string());
            return;
        }
        let task = self
            .calls
            .iter()
            .find(|(_, (id, _))| id == call_id)
            .map(|(task, _)| task.clone());
        let Some(task) = task else {
            return;
        };
        self.calls.remove(&task);
        let live = self
            .registry
            .board()
            .tasks
            .into_iter()
            .find(|t| t.id == task && t.state.is_live());
        if let Some(live) = live {
            info!(stream_id = %self.stream_id, task = %task.0, "the provider withdrew the call of a task");
            self.apply(TaskCmd::Cancel {
                task: live.task_ref,
                by: "system",
            });
        }
    }

    /// The provider handed over a request.
    fn delegate(&mut self, call_id: String, arguments: &str) {
        self.delegated.insert(call_id.clone());
        let request = serde_json::from_str::<Value>(arguments)
            .ok()
            .and_then(|args| args["request"].as_str().map(str::to_owned))
            .unwrap_or_else(|| arguments.to_owned());
        let epoch = self.exec.epoch();
        if request.trim().is_empty() {
            let output = json!({"status": "refused", "reason": "the request was empty"});
            let output = self.scheduled(output, "WHEN_IDLE");
            self.exec.tool_result(call_id, epoch, output, true);
            return;
        }
        // A call made while a gateway line plays is the provider's reply to the line.
        if self.current.is_some() {
            let output =
                json!({"status": "ignored", "reason": "this was not a request of the caller's"});
            let output = self.scheduled(output, "SILENT");
            self.exec.tool_result(call_id, epoch, output, false);
            return;
        }
        // The caller's answer to the gateway's question, or a cancel, passed on as a request:
        // the front reads which (`on_read`, `on_act`) before it is taken as a request.
        self.passed_on(Call {
            call_id,
            epoch,
            request,
        });
    }

    /// A delegate call's request, once it is no answer and no cancel.
    fn delegate_request(&mut self, call_id: String, epoch: u64, request: String) {
        // One call waits for its adoption at a time: an earlier one is the caller's now.
        self.adopt();
        self.next_turn += 1;
        let turn = TurnId(self.next_turn);
        if let Some(adopt_by) = self.plan.adopt_by {
            let now = self.now();
            // A backend that runs its own tools (an agent, a remote system) is costly or may
            // commit something: nothing of the call starts before its adoption.
            let started = self.background.own_tools().is_none();
            if started {
                self.apply(TaskCmd::StartProvisional {
                    turn,
                    text: request.clone(),
                    e_at: None,
                });
                self.registry
                    .hold_provisional(now + adopt_by + PROVISIONAL_SLACK_MS);
            }
            info!(stream_id = %self.stream_id, call = %call_id, started, adopt_by, "interaction provisional call");
            self.provisional = Some(Provisional {
                call_id,
                epoch,
                turn,
                request,
                adopt_at: now + adopt_by,
                started,
            });
            return;
        }
        self.create(call_id, epoch, turn, request, false);
    }

    /// The call waiting for its adoption is the caller's request now.
    fn adopt(&mut self) {
        let Some(provisional) = self.provisional.take() else {
            return;
        };
        info!(stream_id = %self.stream_id, call = %provisional.call_id, "interaction provisional call adopted");
        if provisional.started {
            let now = self.now();
            self.apply(TaskCmd::ConfirmProvisional { e_at: now });
        }
        let Provisional {
            call_id,
            epoch,
            turn,
            request,
            started,
            ..
        } = provisional;
        self.create(call_id, epoch, turn, request, started);
    }

    /// A call nobody withdrew within the provider's `adopt_by` is adopted.
    fn adopt_due(&mut self, now: Ms) {
        if self.provisional.as_ref().is_some_and(|p| now >= p.adopt_at) {
            self.adopt();
        }
    }

    /// The task of a delegate call; `adopt`: its provisional run is the task's.
    fn create(&mut self, call_id: String, epoch: u64, turn: TurnId, request: String, adopt: bool) {
        self.call = Some((call_id, epoch));
        self.apply(TaskCmd::Delegate {
            turn,
            text: request,
            adopt,
        });
        match self.call.take() {
            // No task was made: too many are running.
            Some((call_id, epoch)) => {
                let output = json!({"status": "refused", "reason": "too many requests are running; try again when one has finished"});
                let output = self.scheduled(output, "WHEN_IDLE");
                self.exec.tool_result(call_id, epoch, output, true);
            }
            None if self.plan.gateway_ack => {
                let bank = Say::Bank {
                    group: "ack",
                    intent: None,
                };
                let ack = self.persona.resolve(&bank, turn.0);
                if let Some(ack) = ack {
                    self.say(ack.clone(), ack, Purpose::Ack);
                }
            }
            None => {}
        }
    }

    /// A delegated request that may be the caller's answer to the open (or just answered)
    /// confirmation, or a cancel of the live task: the front reads it, and it is taken as that
    /// (`answer_received`, `cancelling`) or as the request it is.
    fn passed_on(&mut self, call: Call) {
        const ANSWER_WINDOW_MS: Ms = 15_000;
        let now = self.now();
        let open = self.asked.as_ref().map(|asked| {
            // It counts as the caller's answer only where the spoken one would.
            let apply = asked.line.is_some() && asked.heard && !asked.void;
            (asked.request.clone(), apply)
        });
        let recent = self
            .answered
            .as_ref()
            .filter(|(_, at)| now.saturating_sub(*at) <= ANSWER_WINDOW_MS)
            .map(|(question, _)| (question.clone(), false));
        let confirm = open
            .or(recent)
            .filter(|(question, _)| question.kind == HitlKind::Confirm);
        let pending = match confirm {
            Some((request, apply)) => Pending::Answer {
                request: Box::new(request),
                said: call.request.clone(),
                apply,
                call: Some(call),
                then_turn: true,
            },
            None => Pending::Turn {
                said: call.request.clone(),
                continuer: false,
                call: Some(call),
            },
        };
        self.ask_front(pending);
    }

    fn apply(&mut self, cmd: TaskCmd) {
        let now = self.now();
        for effect in self.registry.apply(now, cmd) {
            match effect {
                TaskEffect::StartRun {
                    run,
                    task,
                    words,
                    trace_id,
                    ..
                } => self.start_run(run, task, words, trace_id),
                TaskEffect::CancelRun { run } => {
                    if let Some(cancel) = self.runs.remove(&run) {
                        cancel.cancel();
                    }
                }
                TaskEffect::Store(view) => tasks::store().write(&self.stored, view),
                TaskEffect::Emit(event) => self.on_task(event),
            }
        }
    }

    fn start_run(
        &mut self,
        run: RunId,
        task: Option<TaskId>,
        words: Vec<String>,
        trace_id: String,
    ) {
        let cancel = CancellationToken::new();
        self.runs.insert(run, cancel.clone());
        let revision = u32::try_from(words.len()).unwrap_or(u32::MAX);
        let job = Job {
            run,
            task: task.map(|task| (task, revision)),
            history: self.history.clone(),
            words,
            api_key: self.credential.as_ref().map(SessionCredential::current),
            plain: false,
            context: self.persona.caller_context(&self.cfg.front),
            trace_id,
            child: false,
            handle: None,
            state: None,
        };
        self.background.clone().start(job, self.tx.clone(), cancel);
    }

    fn send(&self, event: &WireEvent) {
        match serde_json::to_string(event) {
            Ok(json) => (self.out)(json),
            Err(error) => warn!(%error, "interaction event not serialised"),
        }
    }

    fn on_task(&mut self, event: TaskEvent) {
        if let Some(outcome) = Outcome::of(&event) {
            tasks::store().result(&self.stored, &outcome);
        }
        let mut spoken = false;
        let mut prompt = None;
        match &event.kind {
            TaskEventKind::Created { .. } => {
                if let Some(call) = self.call.take() {
                    self.calls.insert(event.task.clone(), call);
                }
            }
            TaskEventKind::InputRequired(request) => {
                let (words, voice) = self.ask(request.clone());
                spoken = voice;
                prompt = Some(words);
            }
            TaskEventKind::InputResolved { .. } => {
                let now = self.now();
                self.answered = self.asked.take().map(|asked| (asked.request, now));
            }
            TaskEventKind::Completed { text, effects } => {
                spoken = self.deliver(&event.task, "completed", text, effects);
            }
            TaskEventKind::Failed { error, effects } => {
                spoken = self.deliver(&event.task, "failed", error, effects);
            }
            TaskEventKind::Cancelled { effects, .. } => {
                spoken = self.deliver(&event.task, "cancelled", "", effects);
            }
            TaskEventKind::Rejected { reason } => {
                spoken = self.deliver(&event.task, "rejected", reason, &Effects::NoneCommitted);
            }
            _ => {}
        }
        let Some(mut wire) = events::task_event_to_wire(&event, spoken) else {
            return;
        };
        match &mut wire {
            WireEvent::TaskCreated { origin, .. } => *origin = "provider_tool",
            WireEvent::TaskQuestion { prompt: shown, .. } => {
                if let Some(prompt) = prompt {
                    *shown = prompt;
                }
            }
            _ => {}
        }
        self.send(&wire);
    }

    /// Put the question to the caller by the resolver's path: `(words, spoken)`.
    fn ask(&mut self, request: HitlRequest) -> (String, bool) {
        let words = match request.kind {
            HitlKind::Confirm => self.persona.render_confirm(&request).join(" "),
            HitlKind::Choose => {
                let question = request.question.clone().unwrap_or_default();
                let slots = vec![
                    ("question", question),
                    ("options", request.options.join(" or ")),
                ];
                self.persona
                    .resolve(
                        &Say::Template {
                            id: "choose",
                            slots,
                        },
                        0,
                    )
                    .unwrap_or_default()
            }
            _ => request
                .question
                .clone()
                .unwrap_or_else(|| request.verb_phrase.clone()),
        };
        let mut keys = match request.kind {
            HitlKind::Confirm => self.persona.render_confirm(&request)[1].clone(),
            _ => words.clone(),
        };
        // What is said: the question, and for a caller on a phone line which keys answer it
        // (the screen shows the question alone).
        let mut said = words.clone();
        if let Some(hint) = self.keypad_hint(&request) {
            said = format!("{said} {hint}");
            keys = format!("{keys} {hint}");
        }
        let path = if request.sensitive || request.kind.on_screen() {
            "client_ui"
        } else if request.kind == HitlKind::Confirm && request.irreversible {
            self.plan.confirm_irreversible.as_str()
        } else {
            self.plan.confirm_reversible.as_str()
        };
        info!(stream_id = %self.stream_id, path, request = %request.id.0, "interaction provider question");
        let id = request.id.clone();
        // A gateway clip: the gateway's own voice says the question while the provider's
        // audio is held (the half-cascade's every line is in that voice).
        let clip = self.voice.is_some()
            && matches!(
                path,
                "gateway_clip_same_voice" | "gateway_clip_different_voice" | "exact_waav_tts"
            );
        let voice = match path {
            "native_exact" | "say_guided_exact_words_checked" => true,
            _ if clip => true,
            "unanswerable_no_verbatim_channel" => {
                // No channel can carry the exact words: the action is not taken.
                self.asked = None;
                self.apply(TaskCmd::Answer {
                    request: id,
                    outcome: HitlOutcome::Decline { correction: None },
                    by: AnsweredBy::Policy,
                });
                return (words, false);
            }
            // `client_ui`; a gateway clip with no gateway voice: the client's screen asks.
            _ => false,
        };
        if voice {
            let note = "The back office is asking the caller a question. Their answer goes to the back office: do not call waav_delegate for it.";
            self.exec.send(Verb::AddContext(note.to_owned()));
        }
        // A clip in a voice other than the provider's is introduced, and the provider is told
        // the system read the details in another voice (plan chapter 6, 3.5.1).
        if clip && path == "gateway_clip_different_voice" {
            let intro = Say::Template {
                id: "confirm.other_voice",
                slots: Vec::new(),
            };
            if let Some(intro) = self.persona.resolve(&intro, 0) {
                said = format!("{intro} {said}");
            }
            let note = format!(
                "The system is reading these details to the caller in another voice: \"{said}\". Do not repeat or contradict them."
            );
            self.exec.send(Verb::AddContext(note));
        }
        let line = voice.then(|| {
            if clip {
                self.voice_line(said, keys, Purpose::Question(id), true)
            } else {
                self.say(said, keys, Purpose::Question(id))
            }
        });
        if !voice {
            let note = format!(
                "A question for the caller is shown on their screen: \"{words}\". Ask them to answer it there."
            );
            self.exec.send(Verb::AddContext(note));
        }
        self.asked = Some(Asked {
            request,
            line,
            heard: false,
            void: false,
            reasked: false,
            keys: String::new(),
        });
        (words, voice)
    }

    /// The keypad hint said after a question to a caller on a phone line whose session takes
    /// keypad answers (`hitl.channels: dtmf`), as on the cascade: 1 and 2 for a confirmation,
    /// key n for option n of a choice (at most nine). `None` for any other question.
    fn keypad_hint(&self, request: &HitlRequest) -> Option<String> {
        let phone = self.phone.as_ref().is_some_and(|line| line.phone_leg());
        if !self.cfg.hitl_dtmf || !phone || request.sensitive {
            return None;
        }
        let say = match request.kind {
            HitlKind::Confirm => Say::Template {
                id: "confirm.dtmf",
                slots: Vec::new(),
            },
            HitlKind::Choose if !request.options.is_empty() => {
                let keys: Vec<String> = (1..=9)
                    .zip(&request.options)
                    .map(|(n, option)| format!("{n} for {option}"))
                    .collect();
                let keys = match keys.split_last() {
                    Some((last, rest)) if !rest.is_empty() => {
                        format!("{}, or {last}", rest.join(", "))
                    }
                    _ => keys.join(""),
                };
                Say::Template {
                    id: "hitl.choose_dtmf",
                    slots: vec![("options", keys)],
                }
            }
            _ => return None,
        };
        self.persona.resolve(&say, 0)
    }

    /// Say a gateway line: verbatim where the provider can, else guided and checked; in the
    /// half-cascade, in the gateway's own voice.
    fn say(&mut self, words: String, keys: String, purpose: Purpose) -> u64 {
        if self.plan.half {
            return self.voice_line(words, keys, purpose, false);
        }
        self.next_line += 1;
        let line = self.next_line;
        let (verb, check) = if self.plan.exact {
            (Verb::SayExact(words.clone()), Check::Exact)
        } else {
            let guided = format!(
                "Do not call any function. Say exactly this to the caller, word for word, and nothing else: {words}"
            );
            (Verb::SayGuided(guided), Check::Entities)
        };
        self.sources.add(&words);
        self.lines.insert(
            line,
            Line {
                words,
                keys,
                check,
                purpose,
            },
        );
        self.exec.say(line, verb);
        line
    }

    /// The result of a delegated task goes back to the provider as the call's output. Where
    /// the exact words matter (regime B, or a result that committed something) the gateway
    /// says the result itself and the provider is told not to repeat it. Returns whether the
    /// gateway speaks it.
    fn deliver(&mut self, task: &TaskId, status: &str, text: &str, effects: &Effects) -> bool {
        let Some((call_id, epoch)) = self.calls.remove(task) else {
            return false;
        };
        self.sources.add(text);
        let committed = matches!(effects, Effects::Committed(labels) if !labels.is_empty());
        // The caller read it on the screen (`task_seen`): nobody reads it out.
        let seen = self.seen.remove(task);
        let exact = !seen
            && self.plan.can_say
            && status == "completed"
            && !text.trim().is_empty()
            && (self.plan.regime == Regime::ProviderSpeech || committed);
        let mut output = json!({"status": status, "result": text, "effects": effects});
        if exact {
            output["note"] = json!(
                "The system has read this result to the caller word for word. Do not repeat it."
            );
        } else if seen {
            output["note"] =
                json!("The caller has read this result on their screen. Do not read it out.");
        }
        // Gemini's delivery: nothing to say, at once when something was committed or failed,
        // else when the provider is idle.
        let when = if exact || seen {
            "SILENT"
        } else if committed || status == "failed" {
            "INTERRUPT"
        } else {
            "WHEN_IDLE"
        };
        let output = self.scheduled(output, when);
        self.exec
            .tool_result(call_id, epoch, output, !exact && !seen);
        if exact {
            self.say(text.to_owned(), text.to_owned(), Purpose::Relay);
        }
        exact
    }

    /// A gateway line's response ended: its words against what it had to say.
    fn line_ended(&mut self, line: u64, spoken: bool) {
        if self.current == Some(line) {
            self.current = None;
        }
        let said = std::mem::take(&mut self.said);
        self.corrected = false;
        let Some(sent) = self.lines.remove(&line) else {
            return;
        };
        if !said.is_empty() {
            self.remember(ChatMessage::assistant(said.clone()));
        }
        // An acknowledgement binds nothing: its words are not checked.
        let passed = sent.purpose == Purpose::Ack
            || (spoken && words_match(&sent, &said, self.persona.lexicon()));
        info!(
            stream_id = %self.stream_id,
            line,
            required = %sent.words,
            said = %said,
            passed,
            "interaction exact-words check"
        );
        if !passed {
            self.unverified("entity", "ExactWords", &said, false, false);
        }
        let Purpose::Question(id) = sent.purpose else {
            return;
        };
        let Some(asked) = self.asked.as_mut().filter(|asked| asked.request.id == id) else {
            return;
        };
        if passed {
            asked.heard = true;
            asked.void = false;
            return;
        }
        // The caller did not hear the question as it binds: a spoken answer does not count.
        asked.void = true;
        if spoken && !asked.reasked {
            asked.reasked = true;
            let next = self.say(sent.words, sent.keys, Purpose::Question(id));
            if let Some(asked) = self.asked.as_mut() {
                asked.line = Some(next);
            }
        } else {
            let note = format!(
                "A question for the caller is shown on their screen: \"{}\". Ask them to answer it there.",
                sent.words
            );
            self.exec.send(Verb::AddContext(note));
        }
    }

    // ---- the gateway's own voice (half-cascade, gateway clips)

    /// A gateway line said in the gateway's own voice. `clip`: over a provider that speaks,
    /// which is held while it plays.
    fn voice_line(&mut self, words: String, keys: String, purpose: Purpose, clip: bool) -> u64 {
        self.next_line += 1;
        let line = self.next_line;
        self.sources.add(&words);
        let kind = match purpose {
            Purpose::Question(_) => Kind::Question,
            Purpose::Relay => Kind::Relay,
            Purpose::Correction => Kind::Notice,
            Purpose::Ack => Kind::Ack,
        };
        self.lines.insert(
            line,
            Line {
                words: words.clone(),
                keys,
                check: Check::Exact,
                purpose,
            },
        );
        self.voice_say(words, Some(line), kind, clip);
        line
    }

    fn voice_say(&mut self, words: String, line: Option<u64>, kind: Kind, clip: bool) {
        if self.voice.is_none() {
            warn!(stream_id = %self.stream_id, "interaction gateway voice missing; line not said");
            if let Some(line) = line {
                self.voiced_done(line, false);
            }
            return;
        }
        self.speaker.next += 1;
        let id = self.speaker.next;
        self.speaker.queue.push_back(Voiced {
            id,
            words,
            line,
            kind,
            clip,
        });
        let now = self.now();
        self.speak_next(now);
    }

    /// The next line goes to the voice: one render at a time, never while the caller speaks,
    /// a clip only once the provider is quiet.
    fn speak_next(&mut self, now: Ms) {
        if self.speaker.rendering.is_some() || self.failure.caller_speaking {
            return;
        }
        let Some(next) = self.speaker.queue.front() else {
            return;
        };
        let provider_quiet = !self.exec.clock().audible() && self.current.is_none();
        if next.clip && !provider_quiet && self.speaker.playing.is_empty() {
            return;
        }
        let (Some(voice), Some(next)) = (self.voice.clone(), self.speaker.queue.pop_front()) else {
            return;
        };
        let (cut, id, words, voiced) = (
            self.speaker.cut,
            next.id,
            next.words.clone(),
            self.voiced.clone(),
        );
        debug!(stream_id = %self.stream_id, id, at = now, "interaction gateway voice renders a line");
        self.speaker.rendering = Some(next);
        tokio::spawn(async move {
            let audio = voice(words).await;
            let _ = voiced.send((cut, id, audio));
        });
    }

    /// A line's audio: played after what the voice still plays.
    fn rendered(&mut self, cut: u64, id: u64, audio: Result<crate::core::tts::AudioData, String>) {
        let now = self.now();
        if cut != self.speaker.cut || self.speaker.rendering.as_ref().is_none_or(|v| v.id != id) {
            return;
        }
        let Some(voiced) = self.speaker.rendering.take() else {
            return;
        };
        let audio = match audio {
            Ok(audio) if !audio.data.is_empty() => audio,
            Ok(_) | Err(_) => {
                let error = audio.err().unwrap_or_else(|| "no audio".to_owned());
                warn!(stream_id = %self.stream_id, %error, words = %voiced.words, "interaction gateway voice failed a line");
                if let Some(line) = voiced.line {
                    self.voiced_done(line, false);
                }
                self.speak_next(now);
                return;
            }
        };
        let audio = clips::resampled(&audio, self.failure.rate);
        let ms = clips::duration_ms(&audio);
        let Some(out) = self.voice_audio.as_ref().or(self.failure.audio.as_ref()) else {
            if let Some(line) = voiced.line {
                self.voiced_done(line, false);
            }
            return;
        };
        out(audio.data.clone().into());
        let start = self.speaker.until.max(now);
        self.speaker.until = start + ms;
        if voiced.clip {
            // The provider's audio is held while the clip plays.
            self.exec.gate().hold(self.speaker.until - now);
        }
        info!(stream_id = %self.stream_id, id = voiced.id, ms, words = %voiced.words, "interaction gateway voice line");
        self.segment(&format!("v_{}", voiced.id), "start", voiced.kind);
        self.speaker.playing.push_back((self.speaker.until, voiced));
        self.speak_next(now);
    }

    /// The lines that played to their end.
    fn voice_tick(&mut self, now: Ms) {
        while self
            .speaker
            .playing
            .front()
            .is_some_and(|(until, _)| *until <= now)
        {
            let Some((_, voiced)) = self.speaker.playing.pop_front() else {
                break;
            };
            self.segment(&format!("v_{}", voiced.id), "end", voiced.kind);
            if let Some(line) = voiced.line {
                self.voiced_done(line, true);
            }
        }
        self.speak_next(now);
    }

    fn voice_wakeup(&self) -> Option<Ms> {
        let ends = self.speaker.playing.front().map(|(until, _)| *until);
        // A clip waits for the provider to be quiet.
        let poll = (self.speaker.rendering.is_none() && !self.speaker.queue.is_empty())
            .then(|| self.now() + CLIP_POLL_MS);
        [ends, poll].into_iter().flatten().min()
    }

    /// The caller talks over the gateway's voice (or a stop): what it plays is dropped. A
    /// question is said again after the caller; the provider's own sentences are not.
    fn cut_voice(&mut self, now: Ms, reason: &'static str) -> bool {
        let playing = !self.speaker.playing.is_empty() || self.speaker.rendering.is_some();
        if !playing {
            return false;
        }
        self.speaker.cut += 1;
        self.speaker.until = now;
        let mut again = Vec::new();
        let mut cut = Vec::new();
        let rendering = self.speaker.rendering.take();
        let playing: Vec<Voiced> = self.speaker.playing.drain(..).map(|(_, v)| v).collect();
        for voiced in playing.into_iter().chain(rendering) {
            cut.push(format!("v_{}", voiced.id));
            self.segment(&format!("v_{}", voiced.id), "end", voiced.kind);
            let question = voiced
                .line
                .and_then(|line| self.lines.get(&line))
                .is_some_and(|line| matches!(line.purpose, Purpose::Question(_)));
            if question {
                again.push(voiced);
            } else if let Some(line) = voiced.line {
                self.voiced_done(line, false);
            }
        }
        for voiced in again.into_iter().rev() {
            self.speaker.queue.push_front(voiced);
        }
        // What was still to come of that reply is not said either (its sentences, its
        // correction, an acknowledgement); a question or a task's result still is.
        let waiting = std::mem::take(&mut self.speaker.queue);
        for voiced in waiting {
            let keep = voiced
                .line
                .and_then(|line| self.lines.get(&line))
                .is_some_and(|line| matches!(line.purpose, Purpose::Question(_) | Purpose::Relay));
            if keep {
                self.speaker.queue.push_back(voiced);
            } else if let Some(line) = voiced.line {
                self.voiced_done(line, false);
            }
        }
        info!(stream_id = %self.stream_id, reason, "interaction gateway voice cut");
        self.send(&WireEvent::AgentAudioStopped {
            utterance_ids: cut,
            reason,
            delivered_ms: Vec::new(),
            t_ms: now,
        });
        true
    }

    /// A line of the gateway's voice played to its end (`heard`) or not.
    fn voiced_done(&mut self, line: u64, heard: bool) {
        let Some(sent) = self.lines.remove(&line) else {
            return;
        };
        info!(stream_id = %self.stream_id, line, heard, words = %sent.words, "interaction gateway voice line done");
        if heard {
            self.remember(ChatMessage::assistant(sent.words.clone()));
        }
        let Purpose::Question(id) = sent.purpose else {
            return;
        };
        let Some(asked) = self.asked.as_mut().filter(|asked| asked.request.id == id) else {
            return;
        };
        if heard {
            asked.heard = true;
            asked.void = false;
        } else {
            asked.void = true;
            let note = format!(
                "A question for the caller is shown on their screen: \"{}\". Ask them to answer it there.",
                sent.words
            );
            self.exec.send(Verb::AddContext(note));
        }
    }

    /// The half-cascade: each sentence of the provider's reply is judged before the gateway's
    /// voice says it. One that claims what no task supports, or names a number nobody gave,
    /// is not said: the correction is, once per response.
    fn half_said(&mut self, text: &str) {
        let all: Vec<String> = sentences(text).map(str::to_owned).collect();
        for sentence in all {
            match self.verdict(&sentence) {
                None => self.voice_say(sentence, None, Kind::Answer, false),
                Some((kind, rule)) => {
                    let correct = !self.corrected;
                    if correct {
                        self.corrected = true;
                        let template = Say::Template {
                            id: "regime_c_correction",
                            slots: Vec::new(),
                        };
                        if let Some(words) = self.persona.resolve(&template, 0) {
                            self.voice_line(words.clone(), words, Purpose::Correction, false);
                        }
                    }
                    self.unverified(kind, rule, &sentence, correct, true);
                }
            }
        }
    }

    /// The provider's words as they come (B and C): with `speech.claim_policy: correct`, a
    /// finished sentence that claims what no task supports stops the provider before the rest
    /// of its audio plays, and the correction follows.
    fn saying(&mut self, delta: &str) {
        self.saying.push_str(delta);
        if !self.plan.correct_claims || self.plan.half || self.current.is_some() || self.corrected {
            return;
        }
        let text = self.saying.clone();
        let all: Vec<&str> = sentences(&text).collect();
        let done = if text.trim_end().ends_with(['.', '!', '?']) {
            all.len()
        } else {
            all.len().saturating_sub(1)
        };
        for sentence in all.iter().take(done).skip(self.judged) {
            let Some((kind, rule)) = self.verdict(sentence) else {
                continue;
            };
            self.reported.insert((*sentence).to_owned());
            self.corrected = true;
            self.exec.send(Verb::StopOutput);
            let now = self.now();
            self.send(&WireEvent::AgentAudioStopped {
                utterance_ids: Vec::new(),
                reason: "policy",
                delivered_ms: Vec::new(),
                t_ms: now,
            });
            let template = Say::Template {
                id: "regime_c_correction",
                slots: Vec::new(),
            };
            if let Some(words) = self.persona.resolve(&template, 0) {
                self.say(words.clone(), words, Purpose::Correction);
            }
            self.unverified(kind, rule, sentence, true, true);
            break;
        }
        self.judged = self.judged.max(done);
    }

    /// `audio_segment` of a gateway line the provider says (not on `/realtime`).
    fn segment_of(&self, line: u64, phase: &'static str) {
        let Some(sent) = self.lines.get(&line) else {
            return;
        };
        let kind = match sent.purpose {
            Purpose::Question(_) => Kind::Question,
            Purpose::Relay => Kind::Relay,
            Purpose::Correction => Kind::Notice,
            Purpose::Ack => Kind::Ack,
        };
        self.segment(&format!("g_{line}"), phase, kind);
    }

    fn segment(&self, utterance_id: &str, phase: &'static str, kind: Kind) {
        if !self.plan.segments || !self.cfg.events_segments {
            return;
        }
        self.send(&WireEvent::AudioSegment {
            phase,
            utterance_id: utterance_id.to_owned(),
            lane: "main",
            kind,
            task_id: None,
            t_ms: self.now(),
        });
    }

    /// A delegate result's output, with Gemini's delivery `scheduling` where the session
    /// sends it.
    fn scheduled(&self, mut output: Value, when: &str) -> String {
        if self.plan.scheduling {
            output["waav_scheduling"] = json!(when);
        }
        output.to_string()
    }

    // ---- the client's messages

    /// `interaction_update` mid-session: the paths a session may change, as on `/ws`.
    fn update(&mut self, patch: &Value) {
        let (applied, warnings) = super::config::apply_update(&mut self.cfg, patch);
        info!(stream_id = %self.stream_id, ?applied, refused = warnings.len(), "interaction provider session updated");
        let degraded = if self.failure.degraded {
            vec![Service::Voice.degraded().to_owned()]
        } else {
            Vec::new()
        };
        self.send(&WireEvent::InteractionUpdated {
            applied,
            refused: events::Refused::of(warnings),
            degraded,
            state: None,
            failure: None,
        });
    }

    /// A key of the caller's keypad (`hitl.channels: dtmf`): 1 accepts the open confirmation,
    /// 2 declines it; another question takes digits, `*` clears them and `#` sends them.
    fn key(&mut self, key: char) {
        if !self.cfg.hitl_dtmf {
            return;
        }
        let Some(asked) = self.asked.as_mut() else {
            return;
        };
        let confirm = asked.request.kind == HitlKind::Confirm;
        // Key n picks option n of a choice (the keypad hint names them).
        let option = (asked.request.kind == HitlKind::Choose)
            .then(|| key.to_digit(10))
            .flatten()
            .and_then(|n| asked.request.options.get((n as usize).checked_sub(1)?));
        let outcome = match key {
            _ if option.is_some() => HitlOutcome::Accept {
                value: option.map(|option| json!(option)),
            },
            '1' if confirm => HitlOutcome::Accept { value: None },
            '2' if confirm => HitlOutcome::Decline { correction: None },
            '0'..='9' if !confirm => {
                asked.keys.push(key);
                return;
            }
            '*' => {
                asked.keys.clear();
                return;
            }
            '#' if !asked.keys.is_empty() => HitlOutcome::Accept {
                value: Some(Value::String(std::mem::take(&mut asked.keys))),
            },
            _ => return,
        };
        let request = asked.request.id.clone();
        info!(stream_id = %self.stream_id, request = %request.0, "interaction provider question answered on the keypad");
        self.apply(TaskCmd::Answer {
            request,
            outcome,
            by: AnsweredBy::Dtmf,
        });
    }

    /// A conversation control of the client's (the socket or the room's topic): a stop stops
    /// the provider's speech and the gateway's; the provider runs the rest of the conversation.
    fn control(&mut self, control: super::controls::Control) {
        use super::controls::{ControlKind, Outcome as Applied};
        let (outcome, detail) = match control.kind {
            ControlKind::Stop | ControlKind::Hold => {
                self.exec.send(Verb::StopOutput);
                let now = self.now();
                if !self.cut_voice(now, "client_clear") {
                    self.send(&WireEvent::AgentAudioStopped {
                        utterance_ids: Vec::new(),
                        reason: "client_clear",
                        delivered_ms: Vec::new(),
                        t_ms: now,
                    });
                }
                (Applied::Applied, "the speech was stopped".to_owned())
            }
            _ => {
                let refused = s2s_control_refused(&control);
                let detail = refused["detail"].as_str().unwrap_or_default().to_owned();
                (Applied::Unsupported, detail)
            }
        };
        self.send(&WireEvent::ControlApplied {
            kind: control.kind,
            value: control.value,
            source: control.source,
            outcome,
            detail,
            state_version: 0,
            text: None,
        });
    }

    // ---- when the provider fails (chapter 10)

    /// The provider answered, spoke or reconnected after `at`.
    fn provider_active_since(&self, at: Ms) -> bool {
        let audio = self.exec.clock().last().map(|last| self.ms_of(last));
        [audio, self.failure.active_at]
            .into_iter()
            .flatten()
            .any(|then| then > at)
    }

    fn ms_of(&self, at: Instant) -> Ms {
        u64::try_from(at.saturating_duration_since(self.t0).as_millis()).unwrap_or(Ms::MAX)
    }

    /// The provider fails from `since` on; a stall line may play for it from `due`.
    fn failing(&mut self, since: Ms, due: Ms) {
        if self.failure.since.is_none() {
            self.failure.since = Some(since);
        }
        if self.failure.watcher.failing(Service::Voice, due) {
            info!(stream_id = %self.stream_id, "interaction provider failing");
        }
    }

    fn health_wakeup(&self) -> Option<Ms> {
        if self.failure.failed {
            return None;
        }
        let now = self.now();
        let stall = Ms::from(self.cfg.failure.stall_after_ms);
        let failure = &self.failure;
        [
            failure.watcher.next_wakeup(),
            failure.waiting.map(|at| at + stall),
            failure.caller_quiet_since.map(|at| at + FAILURE_PAUSE_MS),
            Some(failure.playing_until),
            failure.segment.as_ref().map(|(_, until)| *until),
            failure.exiting,
            // A failing provider is looked at until it is back.
            failure.since.map(|_| now + 250),
            failure.gone.as_ref().map(|_| now + 250),
        ]
        .into_iter()
        .flatten()
        .filter(|at| *at > now)
        .min()
    }

    /// The health signals that are deadlines, then what the watcher plays.
    fn health(&mut self, now: Ms) {
        if self.failure.failed {
            return;
        }
        if let Some((segment, until)) = self.failure.segment.take() {
            if now >= until {
                self.segment(&segment, "end", Kind::Failure);
            } else {
                self.failure.segment = Some((segment, until));
            }
        }
        if let Some(at) = self.failure.exiting {
            if now >= at {
                self.session_failed(now);
            }
            return;
        }
        let stall = Ms::from(self.cfg.failure.stall_after_ms);
        // A caller's turn the provider does not answer.
        if let Some(at) = self.failure.waiting {
            if self.provider_active_since(at) {
                self.failure.waiting = None;
            } else if now >= at + stall {
                self.failing(at, now);
            }
        }
        // Anything the provider does after it began to fail mends it.
        if let Some(since) = self.failure.since
            && self.provider_active_since(since)
        {
            self.failure.since = None;
            self.failure.waiting = None;
            if self.failure.watcher.recovered(Service::Voice) {
                info!(stream_id = %self.stream_id, "interaction provider back");
            }
            if std::mem::take(&mut self.failure.degraded) {
                self.updated(None);
            }
        }
        let failure = &self.failure;
        let has_lines = failure.audio.is_some() && clips::default_set().is_some();
        let quiet = !failure.caller_speaking
            && failure
                .caller_quiet_since
                .is_none_or(|at| now >= at + FAILURE_PAUSE_MS);
        let provider_audible = self.exec.clock().audible();
        let line_playing = now < failure.playing_until;
        // The provider spoke again before the back line could: the call goes on.
        if provider_audible && failure.watcher.back_owed() {
            self.failure.watcher.drop_back();
        }
        let slot = health::Slot {
            stall: quiet && !provider_audible && !line_playing && has_lines,
            back: !self.failure.caller_speaking && !provider_audible && !line_playing && has_lines,
            caller_silent: quiet && !line_playing,
        };
        if let Some(line) = self.failure.watcher.next(now, slot) {
            self.play_failure(now, line);
        }
    }

    /// One line of the server's default set, at the provider's rate, into the session's
    /// output audio; `interaction_updated` names it, the history gets a note.
    fn play_failure(&mut self, now: Ms, line: health::Line) {
        let exit = matches!(line.kind, LineKind::Exit | LineKind::ExitFollowUp);
        let set = clips::default_set();
        let of = |kind: LineKind| -> Vec<(clips::FailureLine, crate::core::tts::AudioData)> {
            let lines = set.iter().flat_map(|set| set.lines.iter());
            lines
                .filter(|(said, _)| said.kind == kind)
                .cloned()
                .collect()
        };
        let mut lines = of(line.kind);
        if lines.is_empty() {
            lines = match line.kind {
                LineKind::Hearing => of(LineKind::Stall),
                LineKind::ExitFollowUp => of(LineKind::Exit),
                _ => lines,
            };
        }
        if !exit && line.kind != LineKind::Back {
            self.failure.degraded = true;
        }
        let chosen = (!lines.is_empty()).then(|| lines[line.n % lines.len()].clone());
        let (Some((said, audio)), Some(out)) = (chosen, self.failure.audio.as_ref()) else {
            warn!(stream_id = %self.stream_id, kind = line.kind.as_str(), "interaction failure line missing");
            if exit {
                self.failure.exiting = Some(now);
            }
            return;
        };
        let audio = clips::resampled(&audio, self.failure.rate);
        let ms = clips::duration_ms(&audio);
        out(audio.data.clone().into());
        self.failure.playing_until = now + ms;
        let segment = format!("f_{now}");
        self.segment(&segment, "start", Kind::Failure);
        self.failure.segment = Some((segment, now + ms));
        info!(stream_id = %self.stream_id, id = %said.id, kind = line.kind.as_str(), ms, "interaction failure line");
        let what = match line.kind {
            LineKind::Back => "is back",
            LineKind::Exit | LineKind::ExitFollowUp => "stayed down",
            LineKind::Stall | LineKind::Hearing => "failed",
        };
        self.remember(ChatMessage::user(format!(
            "[the voice {what}; the agent said: {}]",
            said.text
        )));
        if exit {
            self.failure.exiting = Some(now + ms + EXIT_SLACK_MS);
        }
        self.updated(Some(events::FailureNote {
            line: said.id,
            kind: line.kind.as_str(),
            service: Service::Voice.as_str(),
            text: said.text,
        }));
    }

    /// `interaction_updated` with what is degraded now, and the failure line, if any.
    fn updated(&self, failure: Option<events::FailureNote>) {
        let degraded = if self.failure.degraded {
            vec![Service::Voice.degraded().to_owned()]
        } else {
            Vec::new()
        };
        self.send(&WireEvent::InteractionUpdated {
            applied: Vec::new(),
            refused: Vec::new(),
            degraded,
            state: None,
            failure,
        });
    }

    /// The exit line was said: the client gets `session_failed` with the live tasks, and the
    /// surface closes the socket.
    fn session_failed(&mut self, now: Ms) {
        self.failure.failed = true;
        let pending = events::Pending::live(self.registry.board());
        let reason = match &self.failure.gone {
            Some(reason) => format!("the speech-to-speech provider is gone: {reason}"),
            None => format!(
                "the speech-to-speech provider did not recover within {} s of the first stall line",
                self.cfg.failure.max_wait_s
            ),
        };
        warn!(stream_id = %self.stream_id, %reason, pending = pending.len(), "interaction session failed");
        self.send(&WireEvent::SessionFailed {
            service: Service::Voice.as_str(),
            reason,
            pending,
            follow_up: self.cfg.failure.follow_up,
            t_ms: now,
        });
    }

    fn remember(&mut self, message: ChatMessage) {
        self.history.push(message);
        // In blocks, as a session's own history: each run's conversation then starts where the
        // last one's did, and the run's server reuses its cached prefix.
        trim_history(&mut self.history, HISTORY_KEPT);
    }

    /// What the provider said on its own: one verdict per sentence that no task supports.
    fn monitor(&mut self, words: &str) {
        for sentence in sentences(words) {
            // Judged already while it was said.
            if self.reported.contains(sentence) {
                continue;
            }
            let Some((kind, rule)) = self.verdict(sentence) else {
                continue;
            };
            let correct = self.plan.correct_claims && !self.corrected;
            if correct {
                self.corrected = true;
                let template = Say::Template {
                    id: "regime_c_correction",
                    slots: Vec::new(),
                };
                if let Some(words) = self.persona.resolve(&template, 0) {
                    self.say(words.clone(), words, Purpose::Correction);
                }
            }
            self.unverified(kind, rule, sentence, correct, false);
        }
    }

    /// What is wrong with a sentence of the provider's: a claim no task supports, or a number
    /// nobody gave.
    fn verdict(&self, sentence: &str) -> Option<(&'static str, &'static str)> {
        guard::provider_claim(sentence)
            .filter(|(kind, _)| !self.supports(kind, sentence))
            .or_else(|| {
                unsourced_number(sentence, &self.sources).then_some(("entity", "UnsourcedNumber"))
            })
    }

    /// Whether a task of the session backs the claim: something committed, a confirmed
    /// cancel for a cancel, any finished task for a plain "done", a live task for a promise.
    fn supports(&self, kind: &str, sentence: &str) -> bool {
        let tasks = self.registry.board().tasks;
        if kind == "outcome" {
            return tasks.iter().any(|t| t.state.is_live());
        }
        let norm = format!(" {} ", persona::norm_phrase(sentence));
        let any = |words: &[&str]| words.iter().any(|w| norm.contains(&format!(" {w} ")));
        let committed = tasks.iter().any(|t| {
            t.state == TaskState::Completed
                && matches!(&t.effects, Effects::Committed(labels) if !labels.is_empty())
        });
        if committed {
            return true;
        }
        if any(&["cancelled", "canceled", "stopped"]) {
            return tasks.iter().any(|t| t.state == TaskState::Cancelled);
        }
        let plain = ["done", "finished", "completed", "all set"];
        any(&plain)
            && tasks
                .iter()
                .any(|t| t.state == TaskState::Completed && !t.side_effects)
    }

    fn unverified(
        &self,
        kind: &'static str,
        rule: &'static str,
        text: &str,
        corrected: bool,
        stopped: bool,
    ) {
        warn!(stream_id = %self.stream_id, kind, rule, text, corrected, stopped, "interaction unverified provider claim");
        self.send(&WireEvent::UnverifiedClaim {
            kind,
            rule,
            text: text.to_owned(),
            corrected,
            output_stopped: stopped,
            t_ms: self.now(),
        });
    }
}

/// The sentences of a transcript.
fn sentences(text: &str) -> impl Iterator<Item = &str> {
    // The guard's own cut: a stop ends a sentence only before a space, so "12.50" stays whole.
    guard::sentences(text)
        .into_iter()
        .filter(|s| s.chars().any(char::is_alphanumeric))
}

const UNITS: [&str; 20] = [
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
];
const TENS: [&str; 8] = [
    "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
];

/// The numbers in a text, digits or words ("twenty-one" and "21" are both 21; "7:30" is 7
/// and 30; "1,200" is 1200).
fn numbers(text: &str) -> Vec<u64> {
    let lower = text.to_lowercase().replace(',', "");
    let tokens: Vec<String> = lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
        .collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let t = &tokens[i];
        let digits: String = t.chars().take_while(char::is_ascii_digit).collect();
        if !digits.is_empty() {
            if let Ok(n) = digits.parse() {
                out.push(n);
            }
        } else if let Some(n) = UNITS.iter().position(|u| u == t) {
            out.push(n as u64);
        } else if let Some(n) = TENS.iter().position(|u| u == t) {
            let tens = (n as u64 + 2) * 10;
            match tokens
                .get(i + 1)
                .and_then(|u| UNITS[1..10].iter().position(|w| w == u))
            {
                Some(unit) => {
                    out.push(tens + unit as u64 + 1);
                    i += 1;
                }
                None => out.push(tens),
            }
        }
        i += 1;
    }
    out
}

/// The numbers a provider may repeat, read once from each source as it comes: every one of
/// its instructions, and the newest [`SOURCE_NUMBERS_KEPT`] of the rest.
struct Sources {
    instructions: HashSet<u64>,
    recent: VecDeque<u64>,
}

impl Sources {
    fn new(instructions: &str) -> Self {
        Self {
            instructions: numbers(instructions).into_iter().collect(),
            recent: VecDeque::new(),
        }
    }

    fn add(&mut self, text: &str) {
        for n in numbers(text) {
            if self.instructions.contains(&n) {
                continue;
            }
            if let Some(at) = self.recent.iter().position(|known| *known == n) {
                self.recent.remove(at);
            } else if self.recent.len() == SOURCE_NUMBERS_KEPT {
                self.recent.pop_front();
            }
            self.recent.push_back(n);
        }
    }

    fn holds(&self, n: u64) -> bool {
        self.instructions.contains(&n) || self.recent.contains(&n)
    }
}

/// A number in the sentence that no source holds.
fn unsourced_number(sentence: &str, sources: &Sources) -> bool {
    numbers(sentence).into_iter().any(|n| !sources.holds(n))
}

/// Words compared loosely: lower case, no punctuation, numbers as digits.
fn plain_words(text: &str) -> Vec<String> {
    persona::norm_phrase(text)
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_owned())
        .filter(|w| !w.is_empty())
        .collect()
}

/// The exact-words check of a gateway line.
fn words_match(line: &Line, said: &str, lexicon: &persona::Lexicon) -> bool {
    if said.trim().is_empty() {
        return false;
    }
    let mut want = numbers(&line.keys);
    let mut got = numbers(said);
    want.sort_unstable();
    want.dedup();
    got.sort_unstable();
    got.dedup();
    match line.check {
        Check::Exact => plain_words(&line.words) == plain_words(said),
        Check::Entities => {
            let heard = plain_words(said);
            let stem = |w: &str| w.chars().take(5).collect::<String>();
            let content = plain_words(&line.keys).into_iter().filter(|w| {
                w.len() > 3
                    && w.chars().all(char::is_alphabetic)
                    && !lexicon.content_word_stop_list.iter().any(|s| s == w)
            });
            let words_heard = content
                .map(|w| stem(&w))
                .all(|w| heard.iter().any(|h| stem(h) == w));
            want == got && words_heard
        }
    }
}

/// The `control_applied` an agent hears for a control it asked for on a speech-to-speech
/// session: the provider runs the conversation, so the gateway applies none.
fn s2s_control_refused(control: &super::controls::Control) -> Value {
    json!({
        "kind": control.kind.as_str(),
        "value": control.value,
        "source": control.source,
        "outcome": "unsupported",
        "detail": "a speech-to-speech session takes no conversation controls: the provider runs the conversation",
    })
}

/// The unit tests' front (test data, never a gateway path): the lexicon reads an answer, a
/// cancel cue over a live task is an abort, and words that are all continuers a continuer.
#[cfg(test)]
fn lexicon_front(
    turn: TurnId,
    pending: &Pending,
    lexicon: &persona::Lexicon,
    board: &super::types::Board,
) -> Msg {
    match pending {
        Pending::Answer { request, said, .. } => {
            let label = match hitl::parse_answer(said, request, lexicon) {
                Parsed::Accept { strong: true } => "yes".to_owned(),
                Parsed::Accept { strong: false } => "okay".to_owned(),
                Parsed::Decline { correction: None } if request.kind == HitlKind::Choose => {
                    "none".to_owned()
                }
                Parsed::Decline { correction: None } => "no".to_owned(),
                Parsed::Decline { .. } => "change".to_owned(),
                Parsed::Cancel => "cancel".to_owned(),
                Parsed::Choice(at) => (at + 1).to_string(),
                Parsed::Value(_) => "answer".to_owned(),
                Parsed::Unclear => "other".to_owned(),
            };
            Msg::Read {
                turn,
                label: Some(label),
                ms: 0,
            }
        }
        Pending::Turn { said, .. } => {
            let norm = persona::norm_phrase(said);
            let live = board.tasks.iter().any(|t| t.state.is_live());
            let known: HashSet<&str> = lexicon
                .continuers
                .iter()
                .flat_map(|c| c.split_whitespace())
                .collect();
            let words: Vec<&str> = norm.split_whitespace().collect();
            let continuer = !words.is_empty() && words.iter().all(|w| known.contains(w));
            let (act, wait) = if live && persona::any_phrase(&norm, &lexicon.cancel_cues) {
                (Act::Abort, None)
            } else if continuer {
                (Act::Wait, Some(super::types::WaitReason::Continuer))
            } else {
                (Act::Delegate, None)
            };
            let decision = ActDecision {
                act,
                task: None,
                wait,
                say: None,
                intent: None,
                source: super::types::ActSource::Model,
            };
            Msg::Front(FrontEvent::Done { turn, decision })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::types::RunEvent;
    use super::*;

    #[test]
    fn an_agents_control_on_a_speech_to_speech_session_is_unsupported_not_the_call_ended() {
        let control = super::super::controls::Control::from_json(
            &json!({ "kind": "hold" }),
            super::super::controls::ControlSource::Agent,
        )
        .expect("a control");
        let applied = s2s_control_refused(&control);
        assert_eq!(applied["outcome"], "unsupported");
        assert_eq!(applied["kind"], "hold");
        assert_eq!(applied["source"], "agent");
        let detail = applied["detail"].as_str().unwrap_or_default();
        assert!(detail.contains("speech-to-speech"), "{detail}");
        assert!(!detail.contains("ended"), "{detail}");
    }

    fn lexicon() -> persona::Lexicon {
        Persona::load("default").lexicon().clone()
    }

    fn line(words: &str, keys: &str, check: Check) -> Line {
        Line {
            words: words.to_owned(),
            keys: keys.to_owned(),
            check,
            purpose: Purpose::Relay,
        }
    }

    /// A backend whose own tool is on the wire when the session ends, and reports after.
    struct SlowTool(Arc<std::sync::atomic::AtomicBool>);

    impl Backend for SlowTool {
        fn label(&self) -> String {
            "slow".to_owned()
        }
        fn own_tools(&self) -> Option<bool> {
            Some(true)
        }
        fn start(
            self: Arc<Self>,
            job: Job,
            tx: mpsc::Sender<Msg>,
            cancel: CancellationToken,
        ) -> JoinHandle<()> {
            tokio::spawn(async move {
                let started = RunEvent::ToolStarted {
                    name: "book".to_owned(),
                    phrase: None,
                    remote: true,
                };
                let _ = tx.send(Msg::Run(job.run, started)).await;
                tokio::select! {
                    () = cancel.cancelled() => {
                        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
                    }
                    () = tokio::time::sleep(Duration::from_secs(5)) => {
                        let done = RunEvent::Finished { text: "Booked.".to_owned() };
                        let _ = tx.send(Msg::Run(job.run, done)).await;
                    }
                }
            })
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_task_whose_operation_is_on_the_wire_finishes_after_the_session_ends() {
        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (exec, _pending) = super::super::executor::handle();
        let (tx, inbox) = mpsc::channel(64);
        let resolution = json!({"regime": "Delegating", "filler_owner": "provider"});
        let deps = ProviderDeps {
            plan: Plan::of(&resolution, [true; 4], false).unwrap(),
            cfg: InteractionConfig::default(),
            server: Arc::new(ServerInteraction::default()),
            background: Background::Remote(Arc::new(SlowTool(cancelled.clone()))),
            system_prompt: None,
            credential: None,
            exec,
            out: Box::new(|_| {}),
            stream_id: "provider-drain-test".to_owned(),
            owner: "provider-drain-owner".to_owned(),
            instructions: String::new(),
            audio: None,
            output_rate: 24_000,
            voice: None,
            voice_audio: None,
            phone: None,
        };
        let session = spawn(deps, tx.clone(), inbox);
        let call = ProviderEvent::Call {
            call_id: "c1".to_owned(),
            name: DELEGATE_TOOL.to_owned(),
            arguments: json!({"request": "book a table"}).to_string(),
        };
        tx.send(Msg::Provider(call)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        tx.send(Msg::Shutdown { clean: true }).await.unwrap();
        session.await.unwrap();
        let views = tasks::store().list("provider-drain-owner", Some("provider-drain-test"));
        assert_eq!(views.len(), 1, "{views:?}");
        assert_eq!(views[0].state, TaskState::Completed, "{views:?}");
        assert!(!cancelled.load(std::sync::atomic::Ordering::SeqCst));
    }

    /// The server's default set of failure lines: each line a 400 ms tone at 16 kHz.
    fn install_default_lines() {
        let tone: Vec<u8> = (0..6_400)
            .flat_map(|n: i32| (((n % 40) - 20) as i16 * 800).to_le_bytes())
            .collect();
        let audio = crate::core::tts::AudioData {
            data: tone,
            sample_rate: 16_000,
            format: "pcm".to_owned(),
            duration_ms: None,
        };
        let lines = persona::Persona::load("default").failure_lines();
        clips::install_default_set(Some(clips::DefaultSet {
            voice: clips::Voice {
                provider: "server-default".to_owned(),
                voice: "default".to_owned(),
                language: "en".to_owned(),
            },
            lines: lines
                .into_iter()
                .map(|line| (line, audio.clone()))
                .collect(),
        }));
    }

    /// A provider session whose events and played audio are kept, with `max_wait_s` 10.
    fn failing_session(
        stream_id: &str,
    ) -> (
        mpsc::Sender<Msg>,
        JoinHandle<()>,
        Arc<std::sync::Mutex<Vec<(Ms, Value)>>>,
        Arc<std::sync::Mutex<Vec<(Ms, usize)>>>,
    ) {
        install_default_lines();
        let (exec, _pending) = super::super::executor::handle();
        let (tx, inbox) = mpsc::channel(64);
        let resolution = json!({"regime": "ProviderSpeech", "filler_owner": "provider"});
        let mut cfg = InteractionConfig::default();
        cfg.failure.max_wait_s = 10;
        let t0 = Instant::now();
        let at = move || u64::try_from(t0.elapsed().as_millis()).unwrap_or_default();
        let (events, played) = (Arc::default(), Arc::default());
        let (seen, heard): (
            Arc<std::sync::Mutex<Vec<(Ms, Value)>>>,
            Arc<std::sync::Mutex<Vec<(Ms, usize)>>>,
        ) = (Arc::clone(&events), Arc::clone(&played));
        let deps = ProviderDeps {
            plan: Plan::of(&resolution, [true; 4], false).unwrap(),
            cfg,
            server: Arc::new(ServerInteraction::default()),
            background: Background::Remote(Arc::new(Counts(Arc::default()))),
            system_prompt: None,
            credential: None,
            exec,
            out: Box::new(move |json| {
                let event = serde_json::from_str(&json).unwrap();
                seen.lock().unwrap().push((at(), event));
            }),
            stream_id: stream_id.to_owned(),
            owner: "provider-failure-owner".to_owned(),
            instructions: String::new(),
            audio: Some(Box::new(move |pcm: bytes::Bytes| {
                heard.lock().unwrap().push((at(), pcm.len()));
            })),
            output_rate: 24_000,
            voice: None,
            voice_audio: None,
            phone: None,
        };
        let session = spawn(deps, tx.clone(), inbox);
        (tx, session, events, played)
    }

    fn failures(events: &[(Ms, Value)]) -> Vec<(Ms, String)> {
        let kind = |e: &Value| e["failure"]["kind"].as_str().map(str::to_owned);
        events
            .iter()
            .filter_map(|(at, e)| Some((*at, kind(e)?)))
            .collect()
    }

    /// Chapter 10 on a speech-to-speech session: a caller's turn the provider never answers
    /// gets a stall line from the server's default set, into the session's own audio at the
    /// provider's rate, then (the provider staying down for `max_wait_s`) the exit line and
    /// `session_failed`; an error the provider's next answer mends plays nothing; a provider
    /// lost for good ends the call at once.
    #[tokio::test(start_paused = true)]
    async fn a_provider_that_stops_answering_gets_stall_lines_then_the_exit_line() {
        let (tx, session, events, played) = failing_session("provider-failure-test");
        let send = |event| tx.send(Msg::Provider(event));
        send(ProviderEvent::CallerSpeaking(true)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(1_000)).await;
        send(ProviderEvent::CallerSpeaking(false)).await.unwrap();
        send(ProviderEvent::Heard("What time do you open?".into()))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_secs(25)).await;
        let seen = events.lock().unwrap().clone();
        let lines = failures(&seen);
        let kinds: Vec<&str> = lines.iter().map(|(_, kind)| kind.as_str()).collect();
        assert_eq!(kinds, ["stall", "stall", "exit"], "{seen:?}");
        // The first stall at the stall time after the turn ended, then 8 s apart; the exit at
        // `max_wait_s` after the first stall.
        assert!((3_500..=3_600).contains(&lines[0].0), "{lines:?}");
        assert!(lines[1].0 >= lines[0].0 + 8_000, "{lines:?}");
        assert!(lines[2].0 >= lines[0].0 + 10_000, "{lines:?}");
        let stalled = &seen
            .iter()
            .find(|(_, e)| e["failure"].is_object())
            .unwrap()
            .1;
        assert_eq!(stalled["failure"]["service"], "voice");
        assert_eq!(stalled["degraded"], json!(["voice_unavailable"]));
        // Each line was played at the provider's rate: 400 ms at 24 kHz.
        let played = played.lock().unwrap().clone();
        assert_eq!(played.len(), 3, "{played:?}");
        assert!(
            played.iter().all(|(_, bytes)| *bytes == 19_200),
            "{played:?}"
        );
        let failed: Vec<&Value> = seen
            .iter()
            .map(|(_, e)| e)
            .filter(|e| e["type"] == "session_failed")
            .collect();
        assert_eq!(failed.len(), 1, "{seen:?}");
        assert_eq!(failed[0]["service"], "voice");
        assert_eq!(failed[0]["follow_up"], false);
        tx.send(Msg::Shutdown { clean: true }).await.unwrap();
        session.await.unwrap();

        // An error the provider's next answer mends within the stall time plays nothing.
        let (tx, session, events, _) = failing_session("provider-failure-mended");
        tx.send(Msg::Provider(ProviderEvent::Failed("a hiccup".into())))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(1_000)).await;
        tx.send(Msg::Provider(ProviderEvent::Said(
            "We open at nine.".into(),
        )))
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_secs(10)).await;
        assert!(failures(&events.lock().unwrap()).is_empty());
        // One that it does not mend gets a stall line; its answer then means it is back.
        tx.send(Msg::Provider(ProviderEvent::Failed("down".into())))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_secs(4)).await;
        tx.send(Msg::Provider(ProviderEvent::Reconnected))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_secs(2)).await;
        let seen = events.lock().unwrap().clone();
        let kinds: Vec<String> = failures(&seen).into_iter().map(|(_, k)| k).collect();
        assert_eq!(kinds, ["stall", "back"], "{seen:?}");
        assert!(seen.last().unwrap().1["degraded"].is_null(), "{seen:?}");
        tx.send(Msg::Shutdown { clean: true }).await.unwrap();
        session.await.unwrap();

        // A provider lost for good: the exit line at once, then `session_failed`.
        let (tx, session, events, _) = failing_session("provider-failure-gone");
        tx.send(Msg::Provider(ProviderEvent::Gone("dial failed".into())))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_secs(3)).await;
        let seen = events.lock().unwrap().clone();
        let kinds: Vec<String> = failures(&seen).into_iter().map(|(_, k)| k).collect();
        assert_eq!(kinds, ["exit"], "{seen:?}");
        let failed = seen.iter().find(|(_, e)| e["type"] == "session_failed");
        let reason = failed.map(|(_, e)| e["reason"].as_str().unwrap_or_default().to_owned());
        assert!(
            reason.is_some_and(|r| r.contains("dial failed")),
            "{seen:?}"
        );
        tx.send(Msg::Shutdown { clean: true }).await.unwrap();
        session.await.unwrap();

        // A caller's "uh-huh" the provider leaves unanswered on purpose gets no stall line; a
        // "yeah" to the provider's own question is waited on (r3-s2s).
        let (tx, session, events, _) = failing_session("provider-continuer-test");
        let send = |event| tx.send(Msg::Provider(event));
        send(ProviderEvent::CallerSpeaking(true)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(600)).await;
        send(ProviderEvent::CallerSpeaking(false)).await.unwrap();
        send(ProviderEvent::Heard("Uh-huh.".into())).await.unwrap();
        tokio::time::sleep(Duration::from_secs(8)).await;
        assert!(failures(&events.lock().unwrap()).is_empty());
        // Its words before the end of the turn (a provider that reports them first).
        send(ProviderEvent::CallerSpeaking(true)).await.unwrap();
        send(ProviderEvent::Heard("Okay, right.".into()))
            .await
            .unwrap();
        send(ProviderEvent::CallerSpeaking(false)).await.unwrap();
        tokio::time::sleep(Duration::from_secs(8)).await;
        assert!(failures(&events.lock().unwrap()).is_empty());
        send(ProviderEvent::Said("Shall I book it?".into()))
            .await
            .unwrap();
        send(ProviderEvent::ResponseDone).await.unwrap();
        tokio::time::sleep(Duration::from_secs(1)).await;
        send(ProviderEvent::CallerSpeaking(true)).await.unwrap();
        send(ProviderEvent::CallerSpeaking(false)).await.unwrap();
        send(ProviderEvent::Heard("Yeah.".into())).await.unwrap();
        tokio::time::sleep(Duration::from_secs(4)).await;
        let lines = failures(&events.lock().unwrap());
        assert_eq!(lines.len(), 1, "{lines:?}");
        tx.send(Msg::Shutdown { clean: true }).await.unwrap();
        session.await.unwrap();
        clips::install_default_set(None);
    }

    /// A backend that keeps how much conversation each run was given.
    struct Counts(Arc<std::sync::Mutex<Vec<usize>>>);

    impl Backend for Counts {
        fn label(&self) -> String {
            "counts".to_owned()
        }
        fn own_tools(&self) -> Option<bool> {
            Some(false)
        }
        fn start(
            self: Arc<Self>,
            job: Job,
            _tx: mpsc::Sender<Msg>,
            _cancel: CancellationToken,
        ) -> JoinHandle<()> {
            self.0.lock().unwrap().push(job.history.len());
            tokio::spawn(async {})
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_long_call_gives_a_run_only_the_newest_conversation() {
        let given = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (exec, _pending) = super::super::executor::handle();
        let (tx, inbox) = mpsc::channel(256);
        let resolution = json!({"regime": "Delegating", "filler_owner": "provider"});
        let deps = ProviderDeps {
            plan: Plan::of(&resolution, [true; 4], false).unwrap(),
            cfg: InteractionConfig::default(),
            server: Arc::new(ServerInteraction::default()),
            background: Background::Remote(Arc::new(Counts(given.clone()))),
            system_prompt: None,
            credential: None,
            exec,
            out: Box::new(|_| {}),
            stream_id: "provider-history-test".to_owned(),
            owner: "provider-history-owner".to_owned(),
            instructions: String::new(),
            audio: None,
            output_rate: 24_000,
            voice: None,
            voice_audio: None,
            phone: None,
        };
        let session = spawn(deps, tx.clone(), inbox);
        for n in 0..3 * HISTORY_KEPT {
            let heard = ProviderEvent::Heard(format!("I said thing number {n}."));
            tx.send(Msg::Provider(heard)).await.unwrap();
        }
        let call = ProviderEvent::Call {
            call_id: "c1".to_owned(),
            name: DELEGATE_TOOL.to_owned(),
            arguments: json!({"request": "look it up"}).to_string(),
        };
        tx.send(Msg::Provider(call)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        tx.send(Msg::Shutdown { clean: true }).await.unwrap();
        session.await.unwrap();
        // Held in blocks: never more than HISTORY_KEPT, never less than half of it.
        let given = given.lock().unwrap().clone();
        assert_eq!(given.len(), 1);
        assert!(
            (HISTORY_KEPT / 2..=HISTORY_KEPT).contains(&given[0]),
            "{given:?}"
        );
    }

    /// A backend that does nothing until it is cancelled, and reports that.
    struct Waits(Arc<std::sync::atomic::AtomicBool>);

    impl Backend for Waits {
        fn label(&self) -> String {
            "waits".to_owned()
        }
        fn own_tools(&self) -> Option<bool> {
            Some(true)
        }
        fn start(
            self: Arc<Self>,
            job: Job,
            tx: mpsc::Sender<Msg>,
            cancel: CancellationToken,
        ) -> JoinHandle<()> {
            tokio::spawn(async move {
                cancel.cancelled().await;
                self.0.store(true, std::sync::atomic::Ordering::SeqCst);
                let _ = tx.send(Msg::Run(job.run, RunEvent::Cancelled)).await;
            })
        }
    }

    /// What the executor sent the provider.
    #[derive(Clone, Default)]
    struct Sent(Arc<std::sync::Mutex<Vec<String>>>);

    #[async_trait::async_trait]
    impl super::super::executor::Wire for Sent {
        async fn verb(&mut self, verb: &Verb) -> crate::core::realtime::RealtimeResult<()> {
            self.0.lock().unwrap().push(format!("{verb:?}"));
            Ok(())
        }
        async fn tool_result(
            &mut self,
            call_id: &str,
            _: &str,
            _: bool,
        ) -> crate::core::realtime::RealtimeResult<()> {
            self.0.lock().unwrap().push(format!("tool:{call_id}"));
            Ok(())
        }
        async fn reply(&mut self) -> crate::core::realtime::RealtimeResult<()> {
            Ok(())
        }
    }

    /// Provider fix `gemini_tool_call_cancellation`: a withdrawn delegate call cancels its task
    /// and gets no result; a withdrawn call of the client's own is passed on to the client.
    #[tokio::test(start_paused = true)]
    async fn a_withdrawn_delegate_call_cancels_its_task_and_the_clients_call_is_passed_on() {
        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, inbox) = mpsc::channel(64);
        let sent = Sent::default();
        let exec = super::super::executor::spawn(Box::new(sent.clone()), tx.clone(), 24_000);
        let events = Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
        let seen = events.clone();
        let resolution = json!({"regime": "Delegating", "filler_owner": "provider"});
        let deps = ProviderDeps {
            plan: Plan::of(&resolution, [true; 4], false).unwrap(),
            cfg: InteractionConfig::default(),
            server: Arc::new(ServerInteraction::default()),
            background: Background::Remote(Arc::new(Waits(cancelled.clone()))),
            system_prompt: None,
            credential: None,
            exec,
            out: Box::new(move |json| {
                seen.lock()
                    .unwrap()
                    .push(serde_json::from_str(&json).unwrap());
            }),
            stream_id: "provider-withdraw-test".to_owned(),
            owner: "provider-withdraw-owner".to_owned(),
            instructions: String::new(),
            audio: None,
            output_rate: 24_000,
            voice: None,
            voice_audio: None,
            phone: None,
        };
        let session = spawn(deps, tx.clone(), inbox);
        let call = ProviderEvent::Call {
            call_id: "c1".to_owned(),
            name: DELEGATE_TOOL.to_owned(),
            arguments: json!({"request": "book a table"}).to_string(),
        };
        tx.send(Msg::Provider(call)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        for id in ["c1", "client-call"] {
            let withdrawn = ProviderEvent::CallCancelled(id.to_owned());
            tx.send(Msg::Provider(withdrawn)).await.unwrap();
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        tx.send(Msg::Shutdown { clean: true }).await.unwrap();
        session.await.unwrap();
        assert!(
            cancelled.load(std::sync::atomic::Ordering::SeqCst),
            "the run was cancelled"
        );
        let views = tasks::store().list("provider-withdraw-owner", Some("provider-withdraw-test"));
        assert_eq!(views.len(), 1, "{views:?}");
        assert_eq!(views[0].state, TaskState::Cancelled, "{views:?}");
        assert!(
            !sent.0.lock().unwrap().iter().any(|s| s == "tool:c1"),
            "no result on a withdrawn call: {:?}",
            sent.0.lock().unwrap()
        );
        let events = events.lock().unwrap();
        let passed: Vec<&Value> = events
            .iter()
            .filter(|e| e["type"] == "function_call_cancelled")
            .collect();
        assert_eq!(
            passed,
            [&json!({"type": "function_call_cancelled", "call_id": "client-call"})]
        );
        assert!(
            events
                .iter()
                .any(|e| e["type"] == "task_cancelled" && e["by"] == "system"),
            "{events:?}"
        );
    }

    #[test]
    fn numbers_are_read_as_digits_or_words() {
        assert_eq!(numbers("a table for two at 7:30"), [2, 7, 30]);
        assert_eq!(numbers("twenty-one people, 1,200 dollars"), [21, 1200]);
        assert_eq!(numbers("Saturday the 10th at seven"), [10, 7]);
        assert!(numbers("no numbers here").is_empty());
    }

    #[test]
    fn guided_words_pass_with_the_same_numbers_and_content_words() {
        let lex = lexicon();
        let asked = line(
            "To confirm: Book a table, 2, 7 p.m. Do you want me to do that?",
            "Book a table, 2, 7 p.m.",
            Check::Entities,
        );
        assert!(words_match(
            &asked,
            "Just to confirm, book a table for two at seven pm, shall I?",
            &lex
        ));
        assert!(
            !words_match(&asked, "Book a table for three at seven?", &lex),
            "a number changed"
        );
        assert!(
            !words_match(
                &asked,
                "Shall I cancel the reservation for two at seven?",
                &lex
            ),
            "the action changed"
        );
        assert!(!words_match(&asked, "", &lex), "nothing was said");
    }

    #[test]
    fn exact_words_pass_only_word_for_word() {
        let lex = lexicon();
        let said = line(
            "Your order ships Tuesday.",
            "Your order ships Tuesday.",
            Check::Exact,
        );
        assert!(words_match(&said, "your order ships tuesday", &lex));
        assert!(!words_match(&said, "Your order ships on Tuesday.", &lex));
    }

    #[test]
    fn a_number_nobody_gave_is_unsourced() {
        let mut sources = Sources::new("You are the booking line of Rosa's, open until 11.");
        sources.add("I want a table for two at seven.");
        sources.add("The result: booked for 2 at 19:00");
        assert!(!unsourced_number("Your table for two is at 7.", &sources));
        assert!(!unsourced_number("We are open until 11.", &sources));
        assert!(unsourced_number("That will be 45 dollars.", &sources));

        // A long call keeps the newest numbers said, and every number of the instructions.
        for n in 1_000..1_000 + SOURCE_NUMBERS_KEPT as u64 {
            sources.add(&n.to_string());
        }
        assert_eq!(sources.recent.len(), SOURCE_NUMBERS_KEPT);
        assert!(unsourced_number("At 19:00.", &sources), "said too long ago");
        assert!(!unsourced_number("Until 11, at 1000.", &sources));
        // A number said again is the newest again.
        sources.add("1000");
        sources.add("99999");
        assert!(!unsourced_number("1000", &sources));
        assert!(unsourced_number("1001", &sources));
    }

    #[test]
    fn sentences_split_on_their_ends() {
        let all: Vec<&str> = sentences("Okay! I've booked it. Anything else?").collect();
        assert_eq!(all, ["Okay!", "I've booked it.", "Anything else?"]);
        let all: Vec<&str> = sentences("It is 12.50 dollars, all booked.").collect();
        assert_eq!(all, ["It is 12.50 dollars, all booked."]);
    }

    #[test]
    fn the_plan_follows_the_resolution() {
        let resolution = json!({"regime": "ProviderSpeech", "filler_owner": "gateway",
                                "confirm_irreversible": "client_ui",
                                "confirm_reversible": "say_guided_exact_words_checked"});
        let plan = Plan::of(&resolution, [false, true, true, true], true).unwrap();
        assert_eq!(plan.regime, Regime::ProviderSpeech);
        assert!(plan.gateway_ack && !plan.exact && plan.can_say && plan.correct_claims);
        let delegating = json!({"regime": "Delegating", "filler_owner": "provider"});
        assert_eq!(
            Plan::of(&delegating, [true; 4], false).unwrap().regime,
            Regime::Delegating
        );
        assert!(Plan::of(&json!({"regime": "Cascade"}), [true; 4], false).is_none());
    }

    /// What the provider was given for each function call: its output, and whether it was
    /// asked to answer.
    #[derive(Clone, Default)]
    struct Outputs(Arc<std::sync::Mutex<Vec<(String, String, bool)>>>);

    #[async_trait::async_trait]
    impl super::super::executor::Wire for Outputs {
        async fn verb(&mut self, _: &Verb) -> crate::core::realtime::RealtimeResult<()> {
            Ok(())
        }
        async fn tool_result(
            &mut self,
            call_id: &str,
            output: &str,
            reply: bool,
        ) -> crate::core::realtime::RealtimeResult<()> {
            let given = (call_id.to_owned(), output.to_owned(), reply);
            self.0.lock().unwrap().push(given);
            Ok(())
        }
        async fn reply(&mut self) -> crate::core::realtime::RealtimeResult<()> {
            Ok(())
        }
    }

    /// `task_seen` on `/realtime`: the result of a task the caller read on the screen goes
    /// to the provider with no response asked for, so it is not read out; another task's is.
    #[tokio::test(start_paused = true)]
    async fn a_result_the_caller_read_on_the_screen_is_not_read_out() {
        let (tx, inbox) = mpsc::channel(64);
        let outputs = Outputs::default();
        let exec = super::super::executor::spawn(Box::new(outputs.clone()), tx.clone(), 24_000);
        let events = Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
        let kept = events.clone();
        let resolution = json!({"regime": "Delegating", "filler_owner": "provider"});
        let deps = ProviderDeps {
            plan: Plan::of(&resolution, [true; 4], false).unwrap(),
            cfg: InteractionConfig::default(),
            server: Arc::new(ServerInteraction::default()),
            background: Background::Remote(Arc::new(SlowTool(Arc::default()))),
            system_prompt: None,
            credential: None,
            exec,
            out: Box::new(move |json| {
                kept.lock()
                    .unwrap()
                    .push(serde_json::from_str(&json).unwrap());
            }),
            stream_id: "provider-seen-test".to_owned(),
            owner: "provider-seen-owner".to_owned(),
            instructions: String::new(),
            audio: None,
            output_rate: 24_000,
            voice: None,
            voice_audio: None,
            phone: None,
        };
        let session = spawn(deps, tx.clone(), inbox);
        for call_id in ["c1", "c2"] {
            let call = ProviderEvent::Call {
                call_id: call_id.to_owned(),
                name: DELEGATE_TOOL.to_owned(),
                arguments: json!({"request": format!("book table {call_id}")}).to_string(),
            };
            tx.send(Msg::Provider(call)).await.unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let created: Vec<String> = events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e["type"] == "task_created")
            .map(|e| e["task_id"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(created.len(), 2, "{created:?}");
        let task_id = created[0].clone();
        tx.send(Msg::TaskSeen { task_id }).await.unwrap();
        tokio::time::sleep(Duration::from_secs(6)).await;
        tx.send(Msg::Shutdown { clean: true }).await.unwrap();
        session.await.unwrap();
        let outputs = outputs.0.lock().unwrap().clone();
        let of = |id: &str| outputs.iter().find(|(call, ..)| call == id).cloned();
        let (_, seen, reply) = of("c1").expect("c1's result");
        assert!(
            !reply && seen.contains("read this result on their screen"),
            "{seen}"
        );
        let (_, other, reply) = of("c2").expect("c2's result");
        assert!(reply && !other.contains("screen"), "{other}");
    }

    // ---- the gateway's voice, speculation, the client's messages (r3-s2s)

    /// What the provider was sent: verbs and function outputs, in order.
    #[derive(Clone, Default)]
    struct Log(Arc<std::sync::Mutex<Vec<String>>>);

    #[async_trait::async_trait]
    impl super::super::executor::Wire for Log {
        async fn verb(&mut self, verb: &Verb) -> crate::core::realtime::RealtimeResult<()> {
            self.0.lock().unwrap().push(format!("verb:{verb:?}"));
            Ok(())
        }
        async fn tool_result(
            &mut self,
            call_id: &str,
            output: &str,
            reply: bool,
        ) -> crate::core::realtime::RealtimeResult<()> {
            let line = format!("tool:{call_id}:{reply}:{output}");
            self.0.lock().unwrap().push(line);
            Ok(())
        }
        async fn reply(&mut self) -> crate::core::realtime::RealtimeResult<()> {
            Ok(())
        }
    }

    /// A backend of the LLM tier (the gateway runs its tools) that asks to book a table and
    /// finishes once it may; it waits until then, or until it is cancelled.
    struct Books(Arc<std::sync::atomic::AtomicUsize>);

    impl Backend for Books {
        fn label(&self) -> String {
            "books".to_owned()
        }
        fn own_tools(&self) -> Option<bool> {
            None
        }
        fn start(
            self: Arc<Self>,
            job: Job,
            tx: mpsc::Sender<Msg>,
            cancel: CancellationToken,
        ) -> JoinHandle<()> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            tokio::spawn(async move {
                let (reply, decision) = tokio::sync::oneshot::channel();
                let gate = RunEvent::Gate {
                    op_name: "book_table".to_owned(),
                    args: json!({"party": 2}),
                    kind: HitlKind::Confirm,
                    reply,
                };
                let _ = tx.send(Msg::Run(job.run, gate)).await;
                tokio::select! {
                    () = cancel.cancelled() => {
                        let _ = tx.send(Msg::Run(job.run, RunEvent::Cancelled)).await;
                    }
                    decided = decision => {
                        let event = match decided {
                            Ok(super::super::types::GateDecision::Allow(_)) => {
                                RunEvent::Finished { text: "Booked for two.".to_owned() }
                            }
                            _ => RunEvent::Finished { text: "Not booked.".to_owned() },
                        };
                        let _ = tx.send(Msg::Run(job.run, event)).await;
                    }
                }
            })
        }
    }

    struct Rig {
        tx: mpsc::Sender<Msg>,
        session: JoinHandle<()>,
        events: Arc<std::sync::Mutex<Vec<Value>>>,
        /// The words the gateway's voice was asked to say.
        voiced: Arc<std::sync::Mutex<Vec<String>>>,
        /// The bytes of the session's own audio.
        played: Arc<std::sync::Mutex<Vec<usize>>>,
        wire: Log,
    }

    impl Rig {
        fn new(
            resolution: Value,
            tweak: impl FnOnce(&mut Plan),
            cfg: InteractionConfig,
            backend: Arc<dyn Backend>,
            instructions: &str,
            stream_id: &str,
        ) -> Rig {
            Self::on_line(
                resolution,
                tweak,
                cfg,
                backend,
                instructions,
                stream_id,
                None,
            )
        }

        /// [`Rig::new`] on a room's caller line.
        fn on_line(
            resolution: Value,
            tweak: impl FnOnce(&mut Plan),
            cfg: InteractionConfig,
            backend: Arc<dyn Backend>,
            instructions: &str,
            stream_id: &str,
            phone: Option<Arc<crate::core::audio::playback_pump::PumpControl>>,
        ) -> Rig {
            let (tx, inbox) = mpsc::channel(256);
            let wire = Log::default();
            let exec = super::super::executor::spawn(Box::new(wire.clone()), tx.clone(), 24_000);
            let mut plan = Plan::of(&resolution, [false, true, true, true], false).unwrap();
            tweak(&mut plan);
            let events = Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
            let (voiced, played) = (Arc::default(), Arc::default());
            let (seen, said, heard): (
                Arc<std::sync::Mutex<Vec<Value>>>,
                Arc<std::sync::Mutex<Vec<String>>>,
                Arc<std::sync::Mutex<Vec<usize>>>,
            ) = (events.clone(), Arc::clone(&voiced), Arc::clone(&played));
            // 200 ms of a tone at 16 kHz for every line.
            let voice: clips::Render = Arc::new(move |words: String| {
                said.lock().unwrap().push(words);
                Box::pin(async {
                    let tone: Vec<u8> = (0..3_200)
                        .flat_map(|n: i32| (((n % 40) - 20) as i16 * 800).to_le_bytes())
                        .collect();
                    Ok(crate::core::tts::AudioData {
                        data: tone,
                        sample_rate: 16_000,
                        format: "pcm".to_owned(),
                        duration_ms: None,
                    })
                })
            });
            let deps = ProviderDeps {
                plan,
                cfg,
                server: Arc::new(ServerInteraction::default()),
                background: Background::Remote(backend),
                system_prompt: None,
                credential: None,
                exec,
                out: Box::new(move |json| {
                    seen.lock()
                        .unwrap()
                        .push(serde_json::from_str(&json).unwrap());
                }),
                stream_id: stream_id.to_owned(),
                owner: format!("{stream_id}-owner"),
                instructions: instructions.to_owned(),
                audio: Some(Box::new(move |pcm: bytes::Bytes| {
                    heard.lock().unwrap().push(pcm.len());
                })),
                output_rate: 24_000,
                voice: Some(voice),
                voice_audio: None,
                phone,
            };
            let session = spawn(deps, tx.clone(), inbox);
            Rig {
                tx,
                session,
                events,
                voiced,
                played,
                wire,
            }
        }

        async fn provider(&self, event: ProviderEvent) {
            self.tx.send(Msg::Provider(event)).await.unwrap();
        }

        async fn call(&self, call_id: &str, request: &str) {
            let call = ProviderEvent::Call {
                call_id: call_id.to_owned(),
                name: DELEGATE_TOOL.to_owned(),
                arguments: json!({ "request": request }).to_string(),
            };
            self.provider(call).await;
        }

        fn of(&self, kind: &str) -> Vec<Value> {
            let events = self.events.lock().unwrap();
            events
                .iter()
                .filter(|e| e["type"] == kind)
                .cloned()
                .collect()
        }

        async fn end(self) -> Vec<String> {
            self.tx.send(Msg::Shutdown { clean: true }).await.unwrap();
            self.session.await.unwrap();
            self.wire.0.lock().unwrap().clone()
        }
    }

    fn half_cascade() -> Value {
        json!({"regime": "Cascade", "variant": "half_cascade", "filler_owner": "gateway",
               "confirm_irreversible": "exact_waav_tts", "confirm_reversible": "exact_waav_tts"})
    }

    /// The half-cascade: each sentence the provider writes is judged before the gateway's voice
    /// says it. A claim no task supports is not said (the correction is, once), and
    /// `unverified_claim` says the output was stopped; the caller talking over the voice cuts
    /// it; a delegated booking's confirmation and result are said in that voice, and the
    /// caller's spoken yes binds once the question was heard.
    #[tokio::test(start_paused = true)]
    async fn the_half_cascade_says_only_the_checked_sentences_in_the_gateways_voice() {
        let runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let rig = Rig::new(
            half_cascade(),
            |plan| plan.segments = true,
            InteractionConfig::default(),
            Arc::new(Books(runs.clone())),
            "We open at 9 in the morning.",
            "provider-half-test",
        );
        let plan = Plan::of(&half_cascade(), [false, true, true, true], false).unwrap();
        assert!(plan.half && plan.can_say && !plan.exact && plan.correct_claims);
        rig.provider(ProviderEvent::Heard("When do you open?".into()))
            .await;
        rig.provider(ProviderEvent::Said(
            "We open at 9 in the morning. Your table is booked.".into(),
        ))
        .await;
        rig.provider(ProviderEvent::ResponseDone).await;
        tokio::time::sleep(Duration::from_secs(2)).await;
        let voiced = rig.voiced.lock().unwrap().clone();
        assert_eq!(voiced[0], "We open at 9 in the morning.", "{voiced:?}");
        assert!(voiced[1].starts_with("Just to be clear"), "{voiced:?}");
        assert_eq!(voiced.len(), 2, "the claim was not said: {voiced:?}");
        let claims = rig.of("unverified_claim");
        assert_eq!(claims.len(), 1, "{claims:?}");
        assert_eq!(claims[0]["text"], "Your table is booked.");
        assert_eq!(claims[0]["output_stopped"], true);
        assert_eq!(claims[0]["corrected"], true);
        // Played at the provider's rate: 200 ms at 24 kHz each; each line's segment.
        assert_eq!(*rig.played.lock().unwrap(), [9_600, 9_600]);
        let segments = rig.of("audio_segment");
        assert_eq!(segments.len(), 4, "{segments:?}");
        assert_eq!(segments[0]["kind"], "answer");

        // The caller cuts in while a sentence plays: it stops, the rest of the reply is not
        // said.
        rig.provider(ProviderEvent::Said(
            "We are on Main Street. We have a terrace. We serve brunch.".into(),
        ))
        .await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        rig.provider(ProviderEvent::CallerSpeaking(true)).await;
        tokio::time::sleep(Duration::from_secs(1)).await;
        rig.provider(ProviderEvent::CallerSpeaking(false)).await;
        rig.provider(ProviderEvent::ResponseDone).await;
        tokio::time::sleep(Duration::from_secs(1)).await;
        let stopped = rig.of("agent_audio_stopped");
        assert_eq!(stopped.len(), 1, "{stopped:?}");
        assert_eq!(stopped[0]["reason"], "barge_in");
        assert!(
            !stopped[0]["utterance_ids"].as_array().unwrap().is_empty(),
            "{stopped:?}"
        );

        // A booking: the confirmation in the gateway's voice; the caller's yes binds once it
        // was heard; the result is said by the gateway.
        rig.call("c1", "book a table for two").await;
        tokio::time::sleep(Duration::from_secs(1)).await;
        let asked = rig.of("task_question");
        assert_eq!(asked.len(), 1, "{:?}", rig.events.lock().unwrap());
        assert_eq!(asked[0]["spoken"], true);
        rig.provider(ProviderEvent::Heard("Yes please.".into()))
            .await;
        tokio::time::sleep(Duration::from_secs(1)).await;
        let answered = rig.of("task_answer_received");
        assert_eq!(answered.len(), 1, "{:?}", rig.events.lock().unwrap());
        assert_eq!(answered[0]["by"], "voice");
        tokio::time::sleep(Duration::from_secs(1)).await;
        let voiced = rig.voiced.lock().unwrap().clone();
        assert!(
            voiced.iter().any(|w| w == "Booked for two."),
            "the result in the gateway's voice: {voiced:?}"
        );
        let result = rig.of("task_result");
        assert_eq!(result[0]["spoken"], "full", "{result:?}");
        let wire = rig.end().await;
        let output = wire.iter().find(|w| w.starts_with("tool:c1:")).unwrap();
        assert!(
            output.starts_with("tool:c1:false:") && output.contains("word for word"),
            "{output}"
        );
        assert!(
            !wire.iter().any(|w| w.starts_with("verb:SayGuided")),
            "{wire:?}"
        );
        assert_eq!(runs.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    /// Provider speculation (chapter 6): a delegate call is provisional until the provider's
    /// turn ends; then it is the task (`speculative: true`, the run already going). A call the
    /// provider withdraws first is never seen: no task event and no result. One nobody
    /// withdraws within `adopt_by` is adopted then.
    #[tokio::test(start_paused = true)]
    async fn a_provisional_call_is_adopted_at_the_turns_end_and_a_withdrawn_one_is_never_seen() {
        let runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let rig = Rig::new(
            json!({"regime": "Delegating", "filler_owner": "provider"}),
            |plan| plan.adopt_by = Some(10_000),
            InteractionConfig::default(),
            Arc::new(Books(runs.clone())),
            "",
            "provider-speculation-test",
        );
        rig.call("c1", "book a table for two").await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(rig.of("task_created").is_empty(), "provisional: unseen");
        assert_eq!(
            runs.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "but running"
        );
        rig.provider(ProviderEvent::ResponseDone).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        let created = rig.of("task_created");
        assert_eq!(created.len(), 1, "{created:?}");
        assert_eq!(created[0]["speculative"], true);
        assert_eq!(
            runs.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "adopted, not run again"
        );
        assert_eq!(rig.of("task_question").len(), 1, "its question replayed");
        rig.tx
            .send(Msg::TaskAnswer {
                request_id: rig.of("task_question")[0]["request_id"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
                outcome: HitlOutcome::Decline { correction: None },
            })
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;

        // Withdrawn before the turn ended: nothing of it is seen.
        rig.call("c2", "and a taxi").await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        rig.provider(ProviderEvent::CallCancelled("c2".into()))
            .await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        rig.provider(ProviderEvent::ResponseDone).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(rig.of("task_created").len(), 1);
        assert!(rig.of("task_cancelled").is_empty());

        // Nobody withdraws and the turn never ends: adopted at `adopt_by`.
        rig.call("c3", "book again").await;
        tokio::time::sleep(Duration::from_secs(9)).await;
        assert_eq!(rig.of("task_created").len(), 1);
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert_eq!(rig.of("task_created").len(), 2);
        let wire = rig.end().await;
        assert!(!wire.iter().any(|w| w.starts_with("tool:c2:")), "{wire:?}");
    }

    /// The client's messages on a provider session: a keypad 1 answers the open confirmation
    /// (`hitl.channels: dtmf`), `interaction_update` applies what a session may change and
    /// refuses the rest, a `stop` control stops the provider, another is unsupported; Gemini's
    /// delivery `scheduling` rides on each result where the session sends it.
    #[tokio::test(start_paused = true)]
    async fn keys_updates_and_controls_reach_a_provider_session() {
        let mut cfg = InteractionConfig::default();
        cfg.hitl_dtmf = true;
        let rig = Rig::new(
            json!({"regime": "Delegating", "filler_owner": "provider",
                   "confirm_irreversible": "client_ui", "confirm_reversible": "client_ui"}),
            |plan| plan.scheduling = true,
            cfg,
            Arc::new(Books(Arc::default())),
            "",
            "provider-messages-test",
        );
        rig.call("c1", "book a table for two").await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(rig.of("task_question").len(), 1);
        for key in ['5', '1'] {
            rig.tx.send(Msg::Dtmf(key)).await.unwrap();
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        let answered = rig.of("task_answer_received");
        assert_eq!(answered.len(), 1, "{:?}", rig.events.lock().unwrap());
        assert_eq!(answered[0]["by"], "dtmf");
        assert_eq!(answered[0]["outcome"], "accept");

        let patch = json!({"policy": {"hitl_answer_timeout_ms": 30000}, "regime": "cascade"});
        rig.tx.send(Msg::Update(patch)).await.unwrap();
        let control = |raw: Value| {
            super::super::controls::Control::from_json(
                &raw,
                super::super::controls::ControlSource::Client,
            )
            .unwrap()
        };
        let stop = control(json!({"kind": "stop"}));
        rig.tx.send(Msg::Control(stop)).await.unwrap();
        let rate = control(json!({"kind": "rate", "value": "slower"}));
        rig.tx.send(Msg::Control(rate)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        let updated = rig.of("interaction_updated");
        assert_eq!(updated.len(), 1, "{updated:?}");
        assert_eq!(
            updated[0]["applied"],
            json!(["policy.hitl_answer_timeout_ms"])
        );
        assert_eq!(updated[0]["refused"][0]["path"], "regime");
        let applied = rig.of("control_applied");
        assert_eq!(applied.len(), 2, "{applied:?}");
        assert_eq!(applied[0]["outcome"], "applied");
        assert_eq!(applied[1]["outcome"], "unsupported");
        assert_eq!(rig.of("agent_audio_stopped")[0]["reason"], "client_clear");
        let wire = rig.end().await;
        assert!(wire.iter().any(|w| w == "verb:StopOutput"), "{wire:?}");
        let output = wire.iter().find(|w| w.starts_with("tool:c1:")).unwrap();
        assert!(
            output.contains(r#""waav_scheduling":"WHEN_IDLE""#),
            "{output}"
        );
    }

    /// `speech.claim_policy: correct` in regime B: a finished sentence of the provider's that
    /// claims what no task supports stops the provider as it is said (before the rest of its
    /// audio), the correction is queued, and the claim is reported once.
    #[tokio::test(start_paused = true)]
    async fn a_claim_caught_as_it_is_said_stops_the_provider() {
        let rig = Rig::new(
            json!({"regime": "ProviderSpeech", "filler_owner": "provider"}),
            |plan| plan.correct_claims = true,
            InteractionConfig::default(),
            Arc::new(Books(Arc::default())),
            "",
            "provider-early-stop-test",
        );
        for delta in ["Sure. Your table ", "is booked. And"] {
            rig.provider(ProviderEvent::Saying(delta.into())).await;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        let claims = rig.of("unverified_claim");
        assert_eq!(claims.len(), 1, "{claims:?}");
        assert_eq!(claims[0]["output_stopped"], true);
        rig.provider(ProviderEvent::Said(
            "Sure. Your table is booked. And".into(),
        ))
        .await;
        rig.provider(ProviderEvent::ResponseDone).await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(rig.of("unverified_claim").len(), 1, "reported once");
        assert_eq!(rig.of("agent_audio_stopped")[0]["reason"], "policy");
        let wire = rig.end().await;
        let stop = wire.iter().position(|w| w == "verb:StopOutput");
        let correction = wire.iter().position(|w| w.contains("Just to be clear"));
        assert!(
            stop.is_some() && correction.is_some() && stop < correction,
            "{wire:?}"
        );
    }

    /// A gateway clip (`gateway_clip_same_voice`): the question is said in the gateway's voice
    /// once the provider is quiet, and the provider's audio is held while it plays.
    #[tokio::test(start_paused = true)]
    async fn a_question_by_gateway_clip_is_said_in_the_gateways_voice_over_a_held_provider() {
        let rig = Rig::new(
            json!({"regime": "ProviderSpeech", "filler_owner": "provider",
                   "confirm_irreversible": "gateway_clip_same_voice",
                   "confirm_reversible": "gateway_clip_same_voice"}),
            |_| {},
            InteractionConfig::default(),
            Arc::new(Books(Arc::default())),
            "",
            "provider-clip-test",
        );
        rig.call("c1", "book a table for two").await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        let asked = rig.of("task_question");
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0]["spoken"], true);
        let voiced = rig.voiced.lock().unwrap().clone();
        assert_eq!(voiced.len(), 1, "{voiced:?}");
        assert!(voiced[0].contains("Do you want me"), "{voiced:?}");
        let wire = rig.end().await;
        assert!(!wire.iter().any(|w| w.starts_with("verb:Say")), "{wire:?}");
    }

    /// A caller on a phone line hears which keys answer the question: the provider says the
    /// keypad hint with it (the screen shows the question alone), a choice's option n is key
    /// n. Off a phone line nothing mentions keys.
    #[tokio::test(start_paused = true)]
    async fn a_phone_caller_hears_the_keypad_hint_with_a_provider_question() {
        let mut cfg = InteractionConfig::default();
        cfg.hitl_dtmf = true;
        let resolution = json!({"regime": "ProviderSpeech", "filler_owner": "provider",
                                "confirm_irreversible": "say_guided_exact_words_checked",
                                "confirm_reversible": "say_guided_exact_words_checked"});
        let line = Arc::new(crate::core::audio::playback_pump::PumpControl::new(
            mpsc::unbounded_channel().0,
            0,
        ));
        line.set_phone_leg();
        let rig = Rig::on_line(
            resolution.clone(),
            |_| {},
            cfg.clone(),
            Arc::new(Books(Arc::default())),
            "",
            "provider-keypad-hint-test",
            Some(line),
        );
        rig.call("c1", "book a table for two").await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        let asked = rig.of("task_question");
        assert_eq!(asked.len(), 1);
        let shown = asked[0]["prompt"].as_str().unwrap_or_default().to_owned();
        assert!(
            !shown.contains("press"),
            "the screen shows the question: {shown}"
        );
        let wire = rig.end().await;
        let said: Vec<&String> = wire
            .iter()
            .filter(|w| w.starts_with("verb:SayGuided"))
            .collect();
        assert_eq!(said.len(), 1, "{wire:?}");
        assert!(
            said[0].contains("Do you want me")
                && said[0].contains("You can also press 1 for yes, or 2 for no."),
            "{said:?}"
        );

        // Off a phone line: no hint.
        let rig = Rig::new(
            resolution,
            |_| {},
            cfg,
            Arc::new(Books(Arc::default())),
            "",
            "provider-no-keypad-hint-test",
        );
        rig.call("c1", "book a table for two").await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        let wire = rig.end().await;
        assert!(!wire.iter().any(|w| w.contains("press")), "{wire:?}");
    }

    /// A gateway clip in a voice other than the provider's begins with the introduction, and
    /// the provider is told the system read the details in another voice.
    #[tokio::test(start_paused = true)]
    async fn a_different_voice_clip_is_introduced_and_the_provider_told() {
        let rig = Rig::new(
            json!({"regime": "Delegating", "filler_owner": "provider",
                   "confirm_irreversible": "gateway_clip_different_voice",
                   "confirm_reversible": "gateway_clip_different_voice"}),
            |_| {},
            InteractionConfig::default(),
            Arc::new(Books(Arc::default())),
            "",
            "provider-other-voice-test",
        );
        rig.call("c1", "book a table for two").await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        let voiced = rig.voiced.lock().unwrap().clone();
        assert_eq!(voiced.len(), 1, "{voiced:?}");
        assert!(
            voiced[0].starts_with("Here are the details from our system:")
                && voiced[0].contains("Do you want me"),
            "{voiced:?}"
        );
        let asked = rig.of("task_question");
        assert!(
            !asked[0]["prompt"]
                .as_str()
                .unwrap_or_default()
                .contains("our system")
        );
        let wire = rig.end().await;
        assert!(
            wire.iter()
                .any(|w| w.starts_with("verb:AddContext") && w.contains("in another voice")),
            "{wire:?}"
        );
    }

    /// Gemini: a provisional call the provider does not withdraw within 300 ms of stopping its
    /// turn for the caller (`interrupted`) is adopted then; one it withdraws in that time is
    /// never seen.
    #[tokio::test(start_paused = true)]
    async fn a_provisional_call_is_adopted_300_ms_after_an_interruption() {
        let rig = Rig::new(
            json!({"regime": "Delegating", "filler_owner": "provider"}),
            |plan| plan.adopt_by = Some(10_000),
            InteractionConfig::default(),
            Arc::new(Books(Arc::default())),
            "",
            "provider-interrupted-test",
        );
        rig.call("c1", "book a table for two").await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        rig.provider(ProviderEvent::Interrupted).await;
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert!(rig.of("task_created").is_empty(), "not before 300 ms");
        tokio::time::sleep(Duration::from_millis(100)).await;
        let created = rig.of("task_created");
        assert_eq!(created.len(), 1, "{created:?}");
        assert_eq!(created[0]["speculative"], true);
        let request = rig.of("task_question")[0]["request_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let outcome = HitlOutcome::Decline { correction: None };
        let answer = Msg::TaskAnswer {
            request_id: request,
            outcome,
        };
        rig.tx.send(answer).await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;

        // Withdrawn 200 ms after the interruption: never seen.
        rig.call("c2", "and a taxi").await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        rig.provider(ProviderEvent::Interrupted).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        rig.provider(ProviderEvent::CallCancelled("c2".into()))
            .await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(rig.of("task_created").len(), 1);
        let wire = rig.end().await;
        assert!(!wire.iter().any(|w| w.starts_with("tool:c2:")), "{wire:?}");
    }
}
