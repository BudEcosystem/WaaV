//! Shared types of interaction mode: the vocabulary the pure machines (`duplex`, `floor`,
//! `tasks`, `policy`) and the actor exchange. Data only.

use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

/// Milliseconds since session start. The machines take `now: Ms` and never read a clock.
pub type Ms = u64;

/// One piece of agent audio (the pump tag).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct UttId(pub u64);
/// One planned utterance; the sentences of one relay share it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ItemId(pub u64);
/// Caller turn sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TurnId(pub u64);
/// One background run (provisional or adopted).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RunId(pub u64);
/// `"task_"` + 32 hex.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TaskId(pub String);
/// `"t1"`, `"t2"`, never reused; the only task name a model sees.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TaskRef(pub String);
/// `"q_<task_id>_<n>"`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HitlId(pub String);
/// `"op_<task_id>_<rev>_<n>"`; sent as `Idempotency-Key`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct OpId(pub String);

// ---- front

/// What the front decides for one caller turn. The first letters are unique, so a streamed
/// decision is known at its first generated character.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Act {
    Wait,
    Backchannel,
    Delegate,
    Update,
    Abort,
    Social,
    Echo,
    Progress,
    Question,
    Refuse,
    /// About a finished task (chapter 8): a question its stored result may answer, or more
    /// work for the agent that did it, with that agent's context.
    FollowUp,
    /// A conversation control (chapter 9): "stop", "slower please", "say that again".
    Control,
}

impl Act {
    pub const ALL: [Act; 12] = [
        Act::Wait,
        Act::Backchannel,
        Act::Delegate,
        Act::Update,
        Act::Abort,
        Act::Social,
        Act::Echo,
        Act::Progress,
        Act::Question,
        Act::Refuse,
        Act::FollowUp,
        Act::Control,
    ];

    /// The wire name (the same string serde writes).
    pub fn as_str(self) -> &'static str {
        match self {
            Act::Wait => "wait",
            Act::Backchannel => "backchannel",
            Act::Delegate => "delegate",
            Act::Update => "update",
            Act::Abort => "abort",
            Act::Social => "social",
            Act::Echo => "echo",
            Act::Progress => "progress",
            Act::Question => "question",
            Act::Refuse => "refuse",
            Act::FollowUp => "follow_up",
            Act::Control => "control",
        }
    }

    /// The act whose name starts with `c`.
    pub fn from_first_char(c: char) -> Option<Act> {
        Act::ALL.into_iter().find(|act| act.as_str().starts_with(c))
    }

    /// The acts whose reply is the front model's own line (the policy's `speak_front`). Every
    /// other act's reply is the gateway's: an acknowledgement, a status, a repeat, a template.
    pub fn speaks_its_line(self) -> bool {
        matches!(self, Act::Social | Act::Question | Act::Refuse)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitReason {
    Incomplete,
    Continuer,
    SideTalk,
    Nothing,
    /// The caller speaks another language than the conversation's: set by the gateway from the
    /// front's language reading (no word list: owner, 24 Oct), never offered in the decision's
    /// grammar.
    Foreign,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActSource {
    Lexicon,
    Router,
    Model,
    RuleOverride(&'static str),
    Fallback,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ActDecision {
    pub act: Act,
    pub task: Option<TaskRef>,
    pub wait: Option<WaitReason>,
    /// Guard-passed front line.
    pub say: Option<String>,
    pub intent: Option<&'static str>,
    pub source: ActSource,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FrontEvent {
    ActKnown {
        turn: TurnId,
        act: Act,
        task: Option<TaskRef>,
        source: ActSource,
    },
    Done {
        turn: TurnId,
        decision: ActDecision,
    },
    Failed {
        turn: TurnId,
    },
    /// A failed probe, or 3 misses in a row.
    Unavailable,
    /// The model's `control` act: the caller steers the conversation. Sent before `Done`.
    Control {
        turn: TurnId,
        control: super::controls::Control,
    },
    /// The model front heard the caller say how a task's update should reach them ("tell me
    /// as soon as it's booked", "don't tell me about it"). Sent before the turn's `Done`.
    Notify {
        turn: TurnId,
        task: TaskRef,
        announce: Announce,
    },
}

/// How a task's update reaches the caller when the conversation is going on
/// (`tasks.announce`, or the caller's own word for one task).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Announce {
    /// One short offer at the end of the agent's answer; a yes plays it.
    #[default]
    Ask,
    /// The short update itself at the end of the answer.
    Tell,
    /// Never spoken unless the caller asks: the client shows it.
    Screen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardOutcome {
    Pass,
    /// The rule that blocked the line.
    Blocked(&'static str),
}

// ---- floor, lanes, speech

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Floor {
    Listening,
    Thinking,
    UserSpeaking,
    FrontSpeaking,
    MainSpeaking,
    Yielding,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Lane {
    Front,
    Main,
}

/// The wire `kind` of an agent line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Ack,
    Answer,
    Progress,
    Hold,
    /// "It's still going. Do you want to keep waiting, or should I stop it?"
    ChoiceOffer,
    Relay,
    Question,
    Confirm,
    Notice,
    Checkin,
    /// A task's update said at a slot of the conversation (the narrator's line), or the offer
    /// of one ("I've got the answer on your flight. Want it now?").
    Update,
    /// A client's `speak` line. On the wire it is a `notice`: the published list of kinds has
    /// no value of its own for it.
    Client,
    Warm,
    /// A listening sound: rendered into the clip bank at session start, played by the
    /// backchannel lane.
    #[serde(rename = "backchannel")]
    Clip,
    /// A line said because a service failed (chapter 10): rendered at session start, played
    /// by the playback pump.
    Failure,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Say {
    /// The persona picks and rotates the line.
    Bank {
        group: &'static str,
        intent: Option<&'static str>,
    },
    /// A template of `templates.en.json`.
    Template {
        id: &'static str,
        slots: Vec<(&'static str, String)>,
    },
    /// Guard-passed front line.
    Front(String),
    /// Background sentence, never altered.
    Verbatim(String),
    /// The last fully heard agent line.
    Repeat,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Utterance {
    pub item: ItemId,
    pub lane: Lane,
    pub kind: Kind,
    /// 1..9, 1 is the most urgent.
    pub rank: u8,
    pub say: Say,
    /// Warm-up: synthesised into the TTS cache, never played.
    pub discard: bool,
    /// The task and its revision.
    pub task: Option<(TaskId, u32)>,
    pub reply_to: Option<TurnId>,
    pub question: Option<HitlId>,
    pub expires_at: Option<Ms>,
    /// `(attempt, seq)` of a relayed answer sentence.
    pub sentence: Option<(u32, u32)>,
    pub last: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CutCause {
    Interrupt,
    SideTalkTimeout,
    PreEmpted,
    Client,
    Failure,
    Shutdown,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LaneEvent {
    Started {
        item: ItemId,
        utt: UttId,
        at: Ms,
    },
    Heard {
        item: ItemId,
        heard_at: Ms,
    },
    Dropped {
        item: ItemId,
        heard_chars: usize,
        cause: CutCause,
    },
    Returned {
        item: ItemId,
    },
    Idle {
        at: Ms,
    },
}

/// Floor to actor.
#[derive(Debug, Clone, PartialEq)]
pub enum FloorCmd {
    /// One utterance for the speech worker, with the words it resolved to (empty: it has
    /// none and cannot be spoken).
    Submit {
        utt: UttId,
        utterance: Utterance,
        text: String,
    },
    Cut(CutCause),
    /// These warm-ups (warm-up lines, listening sounds, failure lines) were in synthesis when
    /// a line owed to the caller came: their synthesis is dropped (nothing of them plays) and
    /// they wait in the queue again.
    DropWarm(Vec<UttId>),
    /// An urgent line waits for the cut of the relayed answer it breaks into: its words go
    /// into the speech cache now (synthesised, never played), and are said from the cut. A
    /// pre-emption cut keeps this synthesis ([`FloorMachine::prerendering`]).
    ///
    /// [`FloorMachine::prerendering`]: super::floor::FloorMachine::prerendering
    Prerender {
        utt: UttId,
        text: String,
    },
}

/// Floor to duplex.
#[derive(Debug, Clone, PartialEq)]
pub struct FloorSnapshot {
    pub floor: Floor,
    pub agent_audible: bool,
    pub playing: Option<(ItemId, Kind)>,
    pub recent_agent_lines: Vec<(Ms, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PumpEvent {
    Started {
        utt: UttId,
        at: Ms,
    },
    Idle {
        utt: UttId,
        delivered_ms: u64,
        at: Ms,
    },
    SynthDone {
        utt: UttId,
    },
    /// Speak refused, TTS error, or the synthesis watchdog.
    Failed {
        utt: UttId,
    },
    FirstDuckedFrame {
        at: Ms,
    },
}

/// Actor to speech worker, handled in order.
#[derive(Debug, Clone, PartialEq)]
pub enum SpeechCmd {
    Speak {
        utt: UttId,
        text: String,
        discard: bool,
    },
    /// Synthesise `text` into the pump's clip bank.
    Clip {
        utt: UttId,
        text: String,
    },
    Cut,
    /// A cut that leaves the provider's synthesis of a line rendered ahead (a discard) going:
    /// only the agent's audio stops.
    CutAudio,
    /// The speaking rate of what follows: the session's own times this (`front.caller.pace`).
    Rate(f32),
    /// The voice of what follows (a language switch, chapter 9).
    Voice(String),
}

// ---- duplex

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    Interrupt,
    Backchannel,
    TalkingToOthers,
    Background,
    Noise,
    Echo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attribution {
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DuplexEvent {
    Onset {
        at: Ms,
        level_db: f32,
        reflex_ducked: bool,
    },
    /// The level of the voiced audio since the onset, every 128 ms of it.
    Level {
        level_db: f32,
    },
    Offset {
        at: Ms,
        voiced_ms: u32,
    },
    /// Emitted at 200, 700 and 1500 ms of silence.
    Silence {
        at: Ms,
        ms: u32,
    },
    SmartTurn {
        at: Ms,
        p: f32,
    },
    /// The agent's own audio was heard on the caller's channel: the gate explained voiced
    /// audio, or the canceller took it out. Sent at most every 128 ms while it lasts.
    Echo {
        at: Ms,
    },
    /// Whether a barge-in opens on the caller's first words and not on a bare onset (a phone
    /// line with no echo canceller).
    WordsFirst(bool),
    Degraded(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EotSource {
    SttSpeechFinal,
    SmartTurn,
    SilenceCeiling,
    Typed,
    /// An error, or no final within 3 s of an upload.
    SttFailed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EndOfTurn {
    pub turn: TurnId,
    /// E.
    pub at: Ms,
    pub acoustic_end: Option<Ms>,
    pub onset_at: Option<Ms>,
    /// Empty when the source is `SttFailed`.
    pub text: String,
    pub source: EotSource,
    pub eager_same: Option<bool>,
    pub class: Class,
    pub attribution: Attribution,
    pub suspected_echo: bool,
    /// The recogniser reported a real confidence below the pack's `unclear_confidence`. The
    /// duplex machine leaves this and `foreign` `false`; the actor fills them in.
    pub unclear: bool,
    /// The turn is in another language than the policy pack's.
    pub foreign: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateCmd {
    Duck,
    Pause,
    Resume,
    Open,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DuplexAction {
    Gate(GateCmd),
    Cut(CutCause),
    Classified(Class),
    UserSpeaking(bool),
    /// The caller's words over the agent so far: the front is asked about them (chapter 9).
    Interim {
        turn: TurnId,
        text: String,
    },
    Eager {
        turn: TurnId,
        text: String,
    },
    EagerWithdrawn {
        turn: TurnId,
    },
    ClaimSegment,
    /// The caller paused in a long turn that is not finished: a listening sound may play.
    /// `text` is what the turn has said so far, `voiced_ms` how much the caller spoke in it.
    Backchannel {
        text: String,
        voiced_ms: u32,
    },
    /// `upload: false` discards the buffer.
    FinalizeStt {
        upload: bool,
    },
    /// A turn is held on words that may be finished ("Where are you from?"): the text
    /// end-of-turn model is asked to read them (`Duplex::on_words`).
    JudgeWords {
        turn: TurnId,
        text: String,
    },
    EndOfTurn(EndOfTurn),
}

// ---- tasks

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Submitted,
    Working,
    InputRequired,
    Completed,
    Failed,
    Cancelled,
    Rejected,
    Unknown,
}

impl TaskState {
    /// The task still has work to do or an answer to get.
    pub fn is_live(self) -> bool {
        self.is_working() || self == TaskState::InputRequired
    }

    /// The task makes the caller wait.
    pub fn is_working(self) -> bool {
        matches!(self, TaskState::Submitted | TaskState::Working)
    }

    /// The task is over.
    pub fn is_terminal(self) -> bool {
        !self.is_live()
    }
}

/// What a task has committed. On the wire: `"none_committed"`, a list of labels, or `"unknown"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effects {
    NoneCommitted,
    Committed(Vec<String>),
    Unknown,
}

impl Serialize for Effects {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Effects::NoneCommitted => serializer.serialize_str("none_committed"),
            Effects::Committed(labels) => labels.serialize(serializer),
            Effects::Unknown => serializer.serialize_str("unknown"),
        }
    }
}

impl<'de> Deserialize<'de> for Effects {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Wire {
            Labels(Vec<String>),
            Word(String),
        }
        match Wire::deserialize(deserializer)? {
            Wire::Labels(labels) => Ok(Effects::Committed(labels)),
            Wire::Word(word) if word == "none_committed" => Ok(Effects::NoneCommitted),
            Wire::Word(word) if word == "unknown" => Ok(Effects::Unknown),
            Wire::Word(word) => Err(serde::de::Error::custom(format!(
                "not an effects value: {word}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HitlKind {
    #[default]
    Confirm,
    Choose,
    Value,
    /// Fields the client fills in on its screen (`schema`).
    Form,
    /// A link the caller opens on the client's screen to sign in (`url`).
    AuthUrl,
}

impl HitlKind {
    /// Answered on the client's screen, never by voice.
    pub fn on_screen(self) -> bool {
        matches!(self, Self::Form | Self::AuthUrl)
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct HitlRequest {
    pub id: HitlId,
    pub task: TaskId,
    pub revision: u32,
    pub kind: HitlKind,
    /// `Some` = bound to exactly these arguments.
    pub op: Option<OpId>,
    pub verb_phrase: String,
    pub params: Vec<(String, serde_json::Value)>,
    pub question: Option<String>,
    pub options: Vec<String>,
    pub irreversible: bool,
    pub expires_at: Ms,
    /// `Form`: the JSON Schema of the fields. Boxed, as are the others a question rarely has:
    /// every task event carries this request's size.
    pub schema: Option<Box<serde_json::Value>>,
    /// `AuthUrl`: the link.
    pub url: Option<Box<str>>,
    /// The answer is never spoken, sent back to the client or kept: it is asked on the
    /// client's screen only.
    pub sensitive: bool,
    /// `expires_at` is the job's own deadline (`expires_in_ms` of its backend, or its entry's
    /// `question_deadline_ms`): it is never moved, and a question that cannot wait for a
    /// slot of the conversation takes the floor (chapter 8).
    pub firm: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum HitlOutcome {
    Accept { value: Option<serde_json::Value> },
    Decline { correction: Option<String> },
    Cancel,
    Timeout,
    Superseded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AnsweredBy {
    Voice,
    Client,
    Timeout,
    SessionEnd,
    /// The gateway closed the question itself: its value is never taken by voice.
    Policy,
    /// A caller on a phone line pressed keys.
    Dtmf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    Revised,
    Cancelling,
    MarkerThenToolCall,
    RunFailed,
}

/// Where a task sits in the session's task tree. On the wire with every task event, and in
/// `GET /v1/tasks`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Lineage {
    /// The task whose orchestrator run started this one; `None` for a caller's request.
    pub parent_id: Option<TaskId>,
    /// The catalog entry that runs it (`interaction.task_backends[].name`); `None` on the
    /// session's own LLM, Bud agent or DAG.
    pub agent: Option<String>,
    /// 32 hex digits: one per caller's request, inherited by every task it starts, and sent
    /// to each agent as the trace id of a W3C `traceparent` header.
    pub trace_id: String,
    /// A follow-up: the finished task this one continues on the same agent (chapter 8).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub follows: Option<TaskId>,
}

impl Lineage {
    /// A new trace: a caller's request on `agent`.
    pub fn root(agent: Option<String>) -> Self {
        Self {
            parent_id: None,
            agent,
            trace_id: uuid::Uuid::new_v4().simple().to_string(),
            follows: None,
        }
    }
}

/// One task as the policy, the task store and `GET /v1/tasks` see it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskView {
    pub id: TaskId,
    pub task_ref: TaskRef,
    pub revision: u32,
    pub title: String,
    pub state: TaskState,
    pub cancelling: bool,
    pub effects: Effects,
    pub open_question: Option<HitlId>,
    pub side_effects: bool,
    pub started_at: Ms,
    #[serde(flatten)]
    pub lineage: Lineage,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TaskEvent {
    pub task: TaskId,
    pub task_ref: TaskRef,
    pub seq: u64,
    pub revision: u32,
    pub at: Ms,
    pub state: TaskState,
    pub cancelling: bool,
    /// Boxed: every event carries it, and it is read far less often than the rest.
    pub lineage: Box<Lineage>,
    pub kind: TaskEventKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TaskEventKind {
    Created {
        title: String,
        side_effects: bool,
        speculative: bool,
        backend: String,
    },
    Working,
    Progress {
        text: Option<String>,
    },
    AnswerSentence {
        attempt: u32,
        seq: u32,
        text: String,
        first_after_e_ms: Option<u32>,
    },
    AnswerStopped {
        attempt: u32,
        released: u32,
        reason: StopReason,
    },
    /// An orchestrator's answer is over for now: its run waits for its children, whose
    /// results are said as updates. Not on the wire.
    AnswerDone,
    InputRequired(HitlRequest),
    InputResolved {
        request: HitlId,
        outcome: HitlOutcome,
        by: AnsweredBy,
        /// The question was sensitive: the value went to the background, not to the client.
        redacted: bool,
    },
    Revised,
    EffectsUpdated(Effects),
    /// A tool call of a run the caller's change replaced went through after all. The event's
    /// `revision` is the replaced one; the task goes on.
    StaleOutcome {
        label: String,
    },
    Completed {
        text: String,
        effects: Effects,
    },
    Failed {
        error: String,
        effects: Effects,
    },
    CancelRequested,
    CancelUnconfirmed {
        effects: Effects,
    },
    Cancelled {
        by: &'static str,
        effects: Effects,
    },
    Rejected {
        reason: String,
    },
}

/// How a task ended: kept in the task store with its view, pushed when the caller is gone, and
/// handed to the session the caller reconnects with.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Outcome {
    pub task_id: TaskId,
    pub task_ref: TaskRef,
    pub revision: u32,
    pub seq: u64,
    pub state: TaskState,
    /// The answer of a completed task; of any other, what went wrong (empty for a cancel).
    pub summary: String,
    pub effects: Effects,
    #[serde(flatten)]
    pub lineage: Lineage,
    /// The short spoken update of a completed task's result (the narrator's line, or its
    /// template): the push carries it next to the full `summary`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update: Option<String>,
}

impl Outcome {
    /// The outcome a terminal task event carries; `None` for every other event.
    pub fn of(event: &TaskEvent) -> Option<Outcome> {
        let (summary, effects) = match &event.kind {
            TaskEventKind::Completed { text, effects } => (text.clone(), effects.clone()),
            TaskEventKind::Failed { error, effects } => (error.clone(), effects.clone()),
            TaskEventKind::Cancelled { effects, .. } => (String::new(), effects.clone()),
            TaskEventKind::Rejected { reason } => (reason.clone(), Effects::NoneCommitted),
            _ => return None,
        };
        Some(Outcome {
            task_id: event.task.clone(),
            task_ref: event.task_ref.clone(),
            revision: event.revision,
            seq: event.seq,
            state: event.state,
            summary,
            effects,
            lineage: Lineage::clone(&event.lineage),
            update: None,
        })
    }

    /// The outcome as task events at `at`: the last one is the terminal event, and a completed
    /// task's answer comes before it as one sentence, so a session that was not there for the
    /// answer says it.
    pub fn events(&self, at: Ms) -> Vec<TaskEvent> {
        let event = |kind| TaskEvent {
            task: self.task_id.clone(),
            task_ref: self.task_ref.clone(),
            seq: self.seq,
            revision: self.revision,
            at,
            state: self.state,
            cancelling: false,
            lineage: Box::new(self.lineage.clone()),
            kind,
        };
        let (summary, effects) = (self.summary.clone(), self.effects.clone());
        match self.state {
            TaskState::Completed => {
                let sentence = TaskEventKind::AnswerSentence {
                    attempt: 0,
                    seq: 0,
                    text: summary.clone(),
                    first_after_e_ms: None,
                };
                let answer = (!summary.trim().is_empty()).then(|| event(sentence));
                let completed = TaskEventKind::Completed {
                    text: summary,
                    effects,
                };
                answer.into_iter().chain([event(completed)]).collect()
            }
            TaskState::Cancelled => vec![event(TaskEventKind::Cancelled {
                by: "system",
                effects,
            })],
            _ => vec![event(TaskEventKind::Failed {
                error: summary,
                effects,
            })],
        }
    }
}

/// How the error of a run begins when its backend held it for a person's approval (the Bud
/// gateway's `approval_required`): the caller is told that, not that it failed.
pub const APPROVAL_REQUIRED: &str = "approval_required";

/// What a background run reports to the registry.
#[derive(Debug)]
pub enum RunEvent {
    Sentence {
        attempt: u32,
        seq: u32,
        text: String,
    },
    MarkerThenTool,
    ToolStarted {
        name: String,
        phrase: Option<String>,
        /// The backend runs the tool itself (a Bud agent's): what it does is not seen here,
        /// so until the run reports again the call counts as an operation in flight.
        remote: bool,
    },
    Gate {
        op_name: String,
        args: serde_json::Value,
        kind: HitlKind,
        reply: oneshot::Sender<GateDecision>,
    },
    Committed {
        label: String,
    },
    /// The backend works outside the gateway and does not say what it changes (an A2A agent,
    /// an MCP tool), or did not confirm a cancel: from here on the task's effects are not
    /// known.
    EffectsUnseen,
    Finished {
        text: String,
    },
    Failed {
        error: String,
    },
    Cancelled,
    /// The run's answer so far is complete and it waits for its children (an orchestrator).
    Answered,
    /// What continues the task on its backend later (a follow-up): the A2A `contextId`, the
    /// webhook's task id, the MCP task id, the Responses response id.
    Handle(String),
    /// What the backend returned besides the text of its answer: reported before `Finished`.
    Output(Box<TaskOutput>),
    /// An orchestrator run starts a child task on the catalog entry `agent`. `reply` gets the
    /// child's ref at once (or why it was not started); `done` gets how it ended.
    Dispatch {
        agent: String,
        request: String,
        reply: oneshot::Sender<Result<Dispatched, String>>,
        done: oneshot::Sender<Outcome>,
    },
    /// An orchestrator or an agent asks for a conversation control (chapter 9,
    /// `conversation_control`): `reply` gets the `control_applied` event, the tool's result.
    Control {
        control: super::controls::Control,
        reply: oneshot::Sender<serde_json::Value>,
    },
}

/// The structured part of a task's result, published with `task_output`: a backend's
/// structured `data` and the `artifacts` it returned (links, files, structured parts).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct TaskOutput {
    pub data: Option<serde_json::Value>,
    pub artifacts: Vec<serde_json::Value>,
}

/// A child task an orchestrator run started.
#[derive(Debug, Clone, PartialEq)]
pub struct Dispatched {
    pub task_ref: TaskRef,
    /// It waits for a free place on its agent (`max_concurrent`).
    pub queued: bool,
}

impl RunEvent {
    /// The run's last report.
    pub fn is_last(&self) -> bool {
        matches!(
            self,
            Self::Finished { .. } | Self::Failed { .. } | Self::Cancelled
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum GateDecision {
    Allow(OpId),
    /// `stale`, `discarded`, `declined` or `cancelled`.
    Refuse(&'static str),
    Answer(serde_json::Value),
}

#[derive(Debug)]
pub enum TaskCmd {
    StartProvisional {
        turn: TurnId,
        text: String,
        /// `None` = started at an eager end of turn.
        e_at: Option<Ms>,
    },
    ConfirmProvisional {
        e_at: Ms,
    },
    DiscardProvisional {
        reason: &'static str,
    },
    Delegate {
        turn: TurnId,
        text: String,
        adopt: bool,
    },
    Revise {
        task: TaskRef,
        correction: String,
    },
    /// The caller went on with the request after a pause: its words are part of it (a
    /// hesitant request stays one task).
    Continue {
        task: TaskRef,
        text: String,
    },
    /// More work about a finished task: a new task on the agent that did it, with its
    /// continuation handle while the handle lasts (`tasks.follow_up_ttl_s`).
    FollowUp {
        turn: TurnId,
        task: TaskRef,
        text: String,
    },
    Cancel {
        task: TaskRef,
        by: &'static str,
    },
    Answer {
        request: HitlId,
        outcome: HitlOutcome,
        by: AnsweredBy,
    },
    Run {
        run: RunId,
        event: RunEvent,
    },
    /// Who may still read what a task comes to: `push`, the session's push address;
    /// `grace_ms`, the reconnect grace of a lost connection (0 after a clean close).
    SessionEnded {
        push: bool,
        grace_ms: Ms,
    },
    Tick,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TaskEffect {
    StartRun {
        run: RunId,
        task: Option<TaskId>,
        words: Vec<String>,
        /// A child task or a follow-up: the catalog entry it runs on, which hears only its
        /// request.
        agent: Option<String>,
        /// The task's trace (`Lineage::trace_id`); a provisional run's becomes its task's.
        trace_id: String,
        /// A follow-up (its answer is the caller's, spoken), and the handle it continues.
        follow_up: bool,
        handle: Option<String>,
    },
    CancelRun {
        run: RunId,
    },
    Emit(TaskEvent),
    Store(TaskView),
}

/// Registry to policy.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Board {
    pub tasks: Vec<TaskView>,
    pub open_question: Option<HitlRequest>,
    pub provisional: Option<RunId>,
}

// ---- policy

#[derive(Debug, Clone, PartialEq)]
pub enum PolicyInput {
    EndOfTurn(EndOfTurn),
    /// A line the control executor says (a repeat, an honest "I can't do that here").
    ControlLine {
        kind: Kind,
        say: Say,
    },
    Eager {
        turn: TurnId,
        text: String,
    },
    EagerWithdrawn {
        turn: TurnId,
    },
    UserSpeaking {
        on: bool,
    },
    Front(FrontEvent),
    Lane(LaneEvent),
    Task(TaskEvent),
    ClientAnswer {
        request: HitlId,
        outcome: HitlOutcome,
    },
    ClientCancel {
        task: TaskId,
    },
    ClientSpeak {
        text: String,
    },
    /// A key the caller on a phone line pressed: `0`-`9`, `*` or `#`.
    Dtmf {
        digit: char,
    },
    /// The caller spoke while a sensitive value was captured: the speech-to-text heard silence.
    CapturedSpeech,
    /// A question's `task_question` is on the client's connection (a sign-in link).
    Delivered {
        request: HitlId,
    },
    /// The client's `task_seen`: the caller has read the task's result on the screen.
    TaskSeen {
        task: TaskId,
    },
    /// The narrator's line for a [`Narration`]: `None` when the front had none in time.
    Narrated {
        item: ItemId,
        line: Option<String>,
    },
    /// The front's label for a [`Reading`]: `None` when it had none in time.
    Read {
        turn: TurnId,
        label: Option<String>,
    },
    Ready,
    Tick,
}

/// The caller's preferred length of answers (`front.caller.verbosity`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verbosity {
    Brief,
    Normal,
    Detailed,
}

/// The caller's preferred pace of speech (`front.caller.pace`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Pace {
    Slower,
    Normal,
    Faster,
}

impl Pace {
    /// The speaking rate against the session's own: 15% slower or faster.
    pub fn rate_factor(self) -> f32 {
        match self {
            Pace::Slower => 0.85,
            Pace::Normal => 1.0,
            Pace::Faster => 1.15,
        }
    }
}

#[derive(Debug)]
pub enum PolicyAction {
    RequestFront {
        turn: TurnId,
        text: String,
        allowed: Vec<Act>,
        speculate: bool,
    },
    Settled {
        turn: TurnId,
        act: Act,
        source: ActSource,
    },
    Speak(Utterance),
    Drop {
        item: ItemId,
    },
    Task(TaskCmd),
    Emit(super::events::WireEvent),
    FirstSentenceAfterE {
        ms: u32,
    },
    /// The caller keeps speaking another language: from now on the session runs without the
    /// front and the pack (`interaction_language_step_down`).
    StepDown,
    /// A task's update is to be said: the front words it, and its line comes back as
    /// `PolicyInput::Narrated` with the same item.
    Narrate(Narration),
    /// The front reads what the caller's turn means against what it may answer; the label
    /// comes back as `PolicyInput::Read` with the same turn.
    Read(Reading),
}

/// A caller's turn the front reads in context (chapter 9: no phrase list decides what the
/// caller meant): the label of `labels` it means.
#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    pub turn: TurnId,
    pub kind: ReadingKind,
    /// What the turn is read against: the agent's question, or the caller's tasks.
    pub context: String,
    /// The caller's words.
    pub said: String,
    /// Each label and what it means.
    pub labels: Vec<(String, String)>,
}

/// What a [`Reading`] asks of the front.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadingKind {
    /// A reply to the agent's question.
    Reply,
    /// Whether a new request repeats one of the caller's tasks (its number, or `new`).
    Same,
    /// Whether the words are about the one finished task the front named a follow-up of (`1`),
    /// or about something else (`new`).
    Follows,
    /// Whether words the front delegated while a task had just started are about that task
    /// (`1`) or a new request (`new`).
    Started,
    /// Whether a caller who asks again for the number in the agent's last line wants only the
    /// number (`number`) or the whole line (`line`).
    Number,
    /// The language the caller speaks: a code, the pack's first (`front_model::
    /// language_reading`; owner, 24 Oct: no word list or script check decides it).
    Language,
}

/// What the narrator is given for one spoken update (chapter 8).
#[derive(Debug, Clone, PartialEq)]
pub struct Narration {
    pub item: ItemId,
    pub title: String,
    /// The task's result.
    pub summary: String,
    /// The caller's question the result may answer (a follow-up); `None` for an update.
    pub question: Option<String>,
    /// The rule that refused the narrator's last line for this update, which it is told on
    /// this try; `None` on the first.
    pub retry: Option<&'static str>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn act_first_letters_are_unique_and_round_trip() {
        for act in Act::ALL {
            let first = act.as_str().chars().next().unwrap();
            assert_eq!(Act::from_first_char(first), Some(act));
            assert_eq!(serde_json::to_value(act).unwrap(), act.as_str());
        }
        assert_eq!(Act::from_first_char('x'), None);
    }

    #[test]
    fn effects_wire_forms() {
        for (effects, wire) in [
            (Effects::NoneCommitted, serde_json::json!("none_committed")),
            (Effects::Unknown, serde_json::json!("unknown")),
            (
                Effects::Committed(vec!["table hold at Rosa's".to_owned()]),
                serde_json::json!(["table hold at Rosa's"]),
            ),
        ] {
            assert_eq!(serde_json::to_value(&effects).unwrap(), wire);
        }
    }
}
