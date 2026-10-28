//! The policy: what the agent says and when, from end of turn to the last relayed sentence.
//!
//! What the actor does with the actions:
//! - `Task`: every task command comes from here (adoption, discarding a provisional run,
//!   revision, cancel, answers), except the provisional run the actor starts itself on
//!   `RequestFront { speculate: true }`;
//! - `Settled`: for the floor (`FloorMachine::on_settled`), the metrics and the wire;
//! - `Speak` and `Drop`: for the floor. A `Speak` with empty verbatim words and `last` only
//!   closes its relay (the terminal task event comes after the last sentence was offered);
//! - the model front's decision reaches the policy as `ActKnown` and later `Done` or `Failed`
//!   (a test may send `Done` alone).

use std::collections::VecDeque;
use std::sync::Arc;

use super::config::{InteractionConfig, PolicyConfig, Tri};
use super::events::WireEvent;
use super::floor::RANK_URGENT;
use super::front_model::{self, followable};
use super::guard;
use super::hitl::{self, Binding, Parsed};
use super::persona::{POLAR_QUESTION, Persona};
use super::rules;
use super::types::{
    APPROVAL_REQUIRED, Act, ActDecision, ActSource, Announce, AnsweredBy, Board, CutCause, Effects,
    EndOfTurn, EotSource, FrontEvent, GuardOutcome, HitlId, HitlKind, HitlOutcome, HitlRequest,
    ItemId, Kind, Lane, LaneEvent, Ms, Narration, PolicyAction, PolicyInput, Reading, ReadingKind,
    Say, StopReason, TaskCmd, TaskEvent, TaskEventKind, TaskId, TaskRef, TaskState, TaskView,
    TurnId, Utterance, WaitReason,
};

/// Priorities: a reply to the latest turn and a client `speak`; a question or confirmation;
/// a relayed answer or a failure notice; progress; holding and check-in; warm-up.
const RANK_REPLY: u8 = 2;
const RANK_QUESTION: u8 = 3;
const RANK_RELAY: u8 = 4;
/// A task's update said at a slot, or its offer (chapter 8).
const RANK_UPDATE: u8 = 5;
const RANK_PROGRESS: u8 = 7;
const RANK_HOLD: u8 = 8;
const RANK_WARM: u8 = 9;
/// A progress or holding line nobody could say for this long is stale.
const UNSPOKEN_EXPIRY_MS: Ms = 3_000;
/// The delegated turns the fast-answer median looks at.
const FAST_WINDOW: usize = 5;
/// Closed questions remembered, to tell a late client answer from an unknown id.
const CLOSED_KEPT: usize = 16;
/// A task that completed this long ago is "just finished": a request that nearly repeats it
/// is asked about, like one that repeats a live task.
const JUST_FINISHED_MS: Ms = 30_000;
/// Slots an update waits at for the narrator's line before it is left to the screen.
const NARRATION_TRIES: u8 = 3;
/// Caller turns in a row in another language before the session steps down.
const FOREIGN_TURNS: u8 = 2;
/// An unanswered choice offer becomes an agreed wait after this long.
const OFFER_TIMEOUT_MS: Ms = 10_000;
/// Where a form, a sign-in link or a sensitive value is asked: the client's screen.
const SCREEN: &str = "your screen";
/// Where a caller on a phone line types a sensitive value.
const KEYPAD: &str = "your keypad";
/// Nobody spoke for this long and nothing is to play: the conversation is idle, and an update
/// is said rather than offered (chapter 8).
const IDLE_MS: Ms = 4_000;
/// A delegated request that has no task this long after its turn ended owes no answer.
const OWED_BACKSTOP_MS: Ms = 5_000;
/// What asking a question takes: a question that would expire sooner after the agent's
/// current speech takes the floor (chapter 8's deadline questions).
const ASK_MS: Ms = 2_500;
/// An urgent question breaks in with its lead-in ("Quick question.") only when this much is
/// left before its deadline. Live MA2 (5 s): the question took 1.2 to 1.3 s to say, the
/// caller's answer began 0.75 s after it and took 1.3 s, and the request recogniser's final
/// and the reply's reading 0.6 to 1.0 s more; the 1.2 s lead-in on top made every answer
/// late (5.38 to 5.39 s).
const LEAD_IN_LEFT_MS: Ms = 6_000;
/// A caller who goes on with a request this soon after its turn ended (the eager window)
/// adds to it: a hesitant request stays one task (and the floor takes back a line of the
/// agent's that nobody heard yet).
pub(super) const GOING_ON_MS: Ms = 1_500;
/// A turn that points back ("when does it land?") is about the last update the caller heard
/// for this long.
const BACK_REFERENCE_MS: Ms = 120_000;
/// The source of the follow-up a caller's yes to "undo it?" starts.
const UNDO_ACCEPTED: &str = "undo_accepted";
/// Results of finished tasks kept for follow-ups.
const RESULTS_KEPT: usize = 32;

/// The labels the front reads a reply with (chapter 9), and what each means. The
/// meanings are the prompt's; no word of the reply decides.
const YES: &str = "yes";
const NO: &str = "no";
const OTHER: &str = "other";
const WEAK: &str = "okay";
const CHANGE: &str = "change";
const CANCEL: &str = "cancel";
const ANSWER: &str = "answer";
const WAIT: &str = "keep";
const STOP: &str = "stop";
const SECOND: &str = "second";
const NEITHER: &str = "neither";
/// What `other` means.
const ELSE: &str = "asks or says something else";
pub(super) const YES_NO: [(&str, &str); 3] = [
    (YES, "agrees or wants to hear it (go on, sure)"),
    (NO, "declines (not now, leave it)"),
    (OTHER, ELSE),
];
pub(super) const CONFIRM: [(&str, &str); 6] = [
    (YES, "agrees, plainly"),
    (WEAK, "agrees, but only with an okay, a maybe or an I guess"),
    (NO, "declines"),
    (CHANGE, "wants it done with other details"),
    (CANCEL, "wants the whole request dropped"),
    (OTHER, ELSE),
];
pub(super) const CHOOSE: [(&str, &str); 4] = [
    (ANY, "has no preference: any of them is fine"),
    (NONE, "wants none of them"),
    (CANCEL, "wants the whole request dropped"),
    (OTHER, ELSE),
];
pub(super) const VALUE: [(&str, &str); 2] = [
    (ANSWER, "gives the value that was asked for"),
    (CANCEL, "wants to drop the whole request (never mind)"),
];
pub(super) const WAIT_OR_STOP: [(&str, &str); 3] = [
    (
        WAIT,
        "is happy to wait (keep going, no rush, take your time)",
    ),
    (STOP, "wants it stopped"),
    (OTHER, ELSE),
];
pub(super) const WHICH: [(&str, &str); 4] = [
    (CHANGE, "changes that request"),
    (SECOND, "wants a second one as well"),
    (NEITHER, "wants neither"),
    (OTHER, ELSE),
];
const NONE: &str = "none";
/// A choice answered "any of them": the caller's words go to the agent, which picks.
const ANY: &str = "any";
pub(super) const NEW: [(&str, &str); 1] = [("new", "asks for something else")];
/// A follow-up the front named that asks for nothing: the turn is the persona's thanks line.
const THANKS: (&str, &str) = ("thanks", "only thanks or says goodbye");

/// The labels the front reads a caller's answer to an open question against (chapter 9: no
/// phrase list decides what the caller meant): one per option of a choice, then the kind's own.
/// The cascade's policy and a speech-to-speech session read an answer alike.
pub(super) fn answer_labels(request: &HitlRequest) -> Vec<(String, String)> {
    let own: &[(&str, &str)] = match request.kind {
        HitlKind::Confirm => &CONFIRM,
        HitlKind::Choose => &CHOOSE,
        _ => &VALUE,
    };
    let options = (request.kind == HitlKind::Choose).then_some(&request.options);
    let options = options.into_iter().flatten().enumerate();
    let options = options.map(|(at, option)| ((at + 1).to_string(), option.clone()));
    options
        .chain(
            own.iter()
                .map(|(label, means)| ((*label).to_owned(), (*means).to_owned())),
        )
        .collect()
}

/// What the caller's answer (`said`) means by the front's label for it.
pub(super) fn answer_of(request: &HitlRequest, label: &str, said: &str) -> Parsed {
    match (request.kind, label) {
        (_, CANCEL) => Parsed::Cancel,
        (HitlKind::Confirm, YES) => Parsed::Accept { strong: true },
        (HitlKind::Confirm, WEAK) => Parsed::Accept { strong: false },
        (HitlKind::Confirm, CHANGE) => Parsed::Decline {
            correction: Some(said.trim().to_owned()),
        },
        (HitlKind::Confirm, NO) | (HitlKind::Choose, NONE) => Parsed::Decline { correction: None },
        (HitlKind::Value, ANSWER) | (HitlKind::Choose, ANY) => {
            Parsed::Value(said.trim().to_owned())
        }
        (HitlKind::Choose, label) => match label.parse::<usize>() {
            Ok(at) if (1..=request.options.len()).contains(&at) => Parsed::Choice(at - 1),
            _ => Parsed::Unclear,
        },
        _ => Parsed::Unclear,
    }
}

/// The caller's latest request whose task started at most [`front_model::JUST_STARTED_S`]
/// before `at` and still runs.
fn just_started(at: Ms, board: &Board) -> Option<&TaskView> {
    let young =
        |view: &&TaskView| at.saturating_sub(view.started_at) < front_model::JUST_STARTED_S * 1_000;
    board
        .tasks
        .iter()
        .rev()
        .filter(|view| view.lineage.parent_id.is_none())
        .find(|view| view.state.is_live() && !view.cancelling)
        .filter(young)
}

/// What a caller's turn may answer, while the front reads it.
enum Asked {
    /// A question on the screen: a yes or a no to it answers nothing.
    Screen(Say),
    /// The open question: a confirmation, a choice or a value.
    Open(HitlRequest),
    /// "Keep waiting, or should I stop it?"
    Choice,
    Undo(UndoOffer),
    Updates(UpdateOffer),
    /// "Change it, or a second one?"
    Which(Repeat),
    /// A request the front delegated that may ask for one of these tasks again.
    Same(ActDecision, Vec<(TaskRef, String)>),
    /// A follow-up the front named of this finished task (its title, and what the agent that
    /// did it does): whether it is about it.
    Follows(ActDecision, TaskRef, String, Option<String>),
    /// A request the front delegated while this task (its title) had just started: whether it
    /// is about it (owner, 26 Oct).
    Started(ActDecision, TaskRef, String),
}

/// The rows of the answers to what the caller was asked, in their order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Screen,
    Open,
    Choice,
    Undo,
    Updates,
    Which,
    Front,
}

impl Stage {
    fn next(self) -> Stage {
        match self {
            Stage::Screen => Stage::Open,
            Stage::Open => Stage::Choice,
            Stage::Choice => Stage::Undo,
            Stage::Undo => Stage::Updates,
            Stage::Updates => Stage::Which,
            Stage::Which | Stage::Front => Stage::Front,
        }
    }
}

/// A turn the front is reading: the turn's end (`None` for a settled request), the row the
/// answers go on from when it answers none, and what it may answer.
struct Pending {
    turn: TurnId,
    eot: Option<EndOfTurn>,
    stage: Stage,
    asked: Asked,
}

/// The latest caller turn.
struct Turn {
    id: TurnId,
    e: Ms,
    /// When the caller began this turn.
    onset: Ms,
    text: String,
    settled: bool,
    /// The model front's act is known and its line is still to come.
    line_for: Option<Act>,
    /// The front's own acknowledgement line.
    line: Option<String>,
    ack_due: Option<Ms>,
    /// Something must be audible by this time.
    audible_by: Option<Ms>,
    acked: bool,
    /// The caller was already asked whether this request is a second one.
    asked_twice: bool,
    /// The request that had just started when the turn ended ([`just_started`]): the front
    /// was shown it working, so the turn is read against it even if it completes before the
    /// decision does.
    started: Option<TaskRef>,
}

/// A request that nearly repeats a task, held while the caller is asked which they mean.
struct Repeat {
    text: String,
    task: TaskRef,
    /// The question, when the caller had heard it, and when the wait for its answer ends
    /// (set once it was heard or lost).
    item: ItemId,
    heard_at: Option<Ms>,
    deadline: Option<Ms>,
}

/// The answer of one task as it is relayed.
struct Relay {
    task: TaskId,
    item: ItemId,
    /// The caller heard the start of it.
    part_heard: bool,
}

/// The open question and how far the caller has heard it.
struct Asking {
    request: HitlRequest,
    items: Vec<ItemId>,
    readback: Option<ItemId>,
    ask: Option<ItemId>,
    readback_heard_at: Option<Ms>,
    /// When the question itself began to play.
    asked_at: Option<Ms>,
    reask_at: Option<Ms>,
    deadline: Ms,
    /// One re-ask each: after silence, a weak yes, an early yes, an unclear answer.
    reasked: [bool; 4],
    /// The caller cut the question before hearing it.
    reoffer: bool,
    /// A question with a deadline that could not wait for a slot: it takes the floor
    /// ([`RANK_URGENT`]), introduced by one short line when it breaks in.
    urgent: bool,
    /// The relayed answer it broke into, which resumes after it.
    interrupted: Option<ItemId>,
    /// Asked on the client's screen (a form, a sign-in link, a sensitive value): this line
    /// says so, and no spoken words answer it.
    screen: Option<Say>,
    /// A sensitive value typed on a phone's keypad: the keys pressed so far. The `screen`
    /// line says how, and no spoken words answer it.
    keyed: Option<String>,
    /// When the last key of a typed value was pressed: after `policy.hitl_key_gap_ms`
    /// without another the keys so far are the answer, as if `#` had been pressed.
    key_at: Option<Ms>,
    /// A sensitive value is captured on the screen or the keypad: the speech-to-text is
    /// muted meanwhile, and the caller who speaks anyway is pointed there once.
    capture: bool,
    spoke_over: bool,
}

const REASK_SILENCE: usize = 0;
const REASK_WEAK: usize = 1;
const REASK_NOT_HEARD: usize = 2;
const REASK_UNCLEAR: usize = 3;

/// A task's update in the inbox (chapter 8): what an orchestrator's child came to.
#[derive(Debug, Clone)]
struct Update {
    task: TaskId,
    revision: u32,
    parent: Option<TaskId>,
    title: String,
    /// What a failure says; `None` for a result, which the narrator words.
    failure: Option<Say>,
    /// The result, or what went wrong.
    summary: String,
    at: Ms,
    /// It was offered and not taken: it waits for an idle moment, or for the caller to ask.
    offered: bool,
    /// The narrator gave no line for it this many times.
    tries: u8,
    /// The rule that refused the narrator's last line for it.
    refused: Option<&'static str>,
}

impl Update {
    /// Inbox order: failures first, then the updates the caller asked to hear, then by age.
    fn order(&self, told: bool) -> (bool, bool, Ms) {
        (self.failure.is_none(), !told, self.at)
    }
}

/// The offer of updates ("I've got the answer on ... Want it now?").
struct UpdateOffer {
    item: ItemId,
    tasks: Vec<TaskId>,
    /// Its words, for the front's reading of the answer.
    asked: String,
    /// Its answer counts until then; set once it was heard.
    until: Option<Ms>,
}

/// The caller's latest request, whose answer they are still owed: the turn, its task once it
/// exists, when the turn ended, and what the caller said for it.
struct Owed {
    task: Option<TaskId>,
    at: Ms,
    words: String,
}

/// A follow-up question the narrator is answering from a finished task's result; without an
/// answer it becomes a task on the task's agent.
struct Answering {
    item: ItemId,
    turn: TurnId,
    task: TaskRef,
    text: String,
    at: Ms,
    title: String,
    summary: String,
}

/// "It's already done. Do you want me to undo it?" about a finished task.
struct UndoOffer {
    item: ItemId,
    task: TaskRef,
    /// Its words, for the front's reading of the answer.
    asked: String,
    /// The caller's words that asked to cancel it: the compensating task's request.
    text: String,
    until: Option<Ms>,
}

/// The stretch in which at least one task is running.
struct Wait {
    started: Ms,
    /// Progress and holding lines said in this wait.
    updates: u32,
    last_progress: Option<Ms>,
    held: bool,
    /// The choice offer was made in this wait.
    offered: bool,
    /// The latest progress text that is still to be said, and its task.
    progress: Option<(String, Option<(TaskId, u32)>)>,
}

impl Wait {
    fn new(started: Ms) -> Self {
        Wait {
            started,
            updates: 0,
            last_progress: None,
            held: false,
            offered: false,
            progress: None,
        }
    }

    /// When the kept progress text may be said: not in the first `progress_min_wait_ms` of
    /// the wait, `progress_min_gap_ms` after the one before, and only while the wait has
    /// updates left.
    fn progress_due(&self, cfg: &PolicyConfig) -> Option<Ms> {
        self.progress.as_ref()?;
        let first = self.started + Ms::from(cfg.progress_min_wait_ms);
        let apart = |last| first.max(last + Ms::from(cfg.progress_min_gap_ms));
        (self.updates < cfg.updates_max_per_wait).then_some(self.last_progress.map_or(first, apart))
    }
}

pub struct Policy {
    cfg: PolicyConfig,
    speculate: bool,
    persona: Arc<Persona>,
    next_item: u64,
    turn: Option<Turn>,
    relays: Vec<Relay>,
    asking: Option<Asking>,
    /// "Change it, or a second one?" is open.
    repeat: Option<Repeat>,
    /// The choice offer was said; its answer counts until this time.
    offer: Option<Ms>,
    /// The client shows questions on a screen (`hitl.channels` holds `client`).
    client: bool,
    /// The caller is on a phone line and `hitl.channels` holds `dtmf`: a confirmation may be
    /// answered with keys 1 and 2, and a sensitive value is typed on the keypad.
    keypad: bool,
    /// The tasks that completed lately, and when.
    finished: VecDeque<(TaskId, Ms)>,
    /// Caller turns in a row in another language.
    foreign_turns: u8,
    /// The session has a plain conversation path to step down to.
    step_down: bool,
    /// The agents that are one MCP tool each: a task on one is a single call.
    single_calls: Vec<String>,
    /// What each agent of the session does (its name, its own description).
    agent_topics: Vec<(String, String)>,
    /// Stepped down: every turn goes to the background, and only its answer is spoken.
    plain: bool,
    /// The task whose caller was last told that a value is never taken by voice.
    declined: Option<TaskId>,
    closed: VecDeque<HitlId>,
    /// Front questions on their way: hearing one opens the answer window.
    asked: Vec<ItemId>,
    answer_window_until: Ms,
    wait: Option<Wait>,
    /// When the agent last spoke (or the wait began, or the caller's turn ended).
    last_speech: Ms,
    user_speaking: bool,
    /// E to first answer sentence of the latest delegated turns.
    first_sentence_ms: VecDeque<u32>,
    /// From submitting an answer sentence to its first audio, as last measured (0 before).
    synth_ms: Ms,
    tts_failed: bool,
    /// Task updates waiting for a slot of the conversation (chapter 8).
    inbox: Vec<Update>,
    /// `tasks.announce`.
    announce: Announce,
    /// The caller's own word on how one task's updates reach them.
    told: Vec<(TaskId, Announce)>,
    update_offer: Option<UpdateOffer>,
    /// Updates the front is wording, with the turn each answers (a yes to the offer).
    narrating: Vec<(ItemId, Update, Option<TurnId>)>,
    /// Updates handed to the floor and not yet heard.
    saying: Vec<(ItemId, Update)>,
    /// Nothing of the agent's is queued or playing (the floor's word, before each step).
    quiet: bool,
    /// The caller has a screen: an update points at it instead of reading the details out.
    screen: bool,
    owed: Option<Owed>,
    /// What last held the inbox back (logged when it changes).
    held: Option<&'static str>,
    /// When the caller last spoke, and when the agent's audio last started or ended.
    caller_at: Ms,
    agent_at: Ms,
    /// A child's question waiting for a slot (chapter 8): the registry has it open, the
    /// floor and the turn rules do not see it until it is asked.
    deferred: Option<HitlRequest>,
    /// How long the agent's speech still to be heard lasts, and its items (the floor's word,
    /// before each step).
    left_ms: Ms,
    queued: Vec<ItemId>,
    /// The agent's recent lines and when each ended (the floor's heard timeline, before each
    /// step): what a task's update may not say again.
    said: Vec<(Ms, String)>,
    /// The results of finished tasks, for follow-ups.
    results: VecDeque<(TaskId, String)>,
    /// The session has an orchestrator with agents, whose questions may break in.
    orchestrated: bool,
    /// The task whose update the caller heard last, and when.
    last_update: Option<(TaskId, Ms)>,
    answering: Vec<Answering>,
    undo: Option<UndoOffer>,
    /// The turn the front is reading.
    reading: Option<Pending>,
    /// When the timers last ran.
    timed_at: Ms,
    /// Tests read a turn at once, from the lexicon (test data).
    #[cfg(test)]
    reader: Option<fn(&Policy, &Pending) -> Option<String>>,
}

impl Policy {
    pub fn new(cfg: &InteractionConfig, persona: Arc<Persona>) -> Self {
        Policy {
            cfg: cfg.policy.clone(),
            speculate: cfg.tasks.speculative_start != Tri::Off,
            persona,
            next_item: 1,
            turn: None,
            relays: Vec::new(),
            asking: None,
            repeat: None,
            offer: None,
            client: cfg.hitl_client,
            keypad: false,
            finished: VecDeque::new(),
            foreign_turns: 0,
            step_down: false,
            single_calls: Vec::new(),
            agent_topics: Vec::new(),
            plain: false,
            declined: None,
            closed: VecDeque::new(),
            asked: Vec::new(),
            answer_window_until: 0,
            wait: None,
            last_speech: 0,
            user_speaking: false,
            first_sentence_ms: VecDeque::new(),
            synth_ms: 0,
            tts_failed: false,
            inbox: Vec::new(),
            announce: cfg.tasks.announce,
            told: Vec::new(),
            update_offer: None,
            narrating: Vec::new(),
            saying: Vec::new(),
            quiet: true,
            screen: false,
            owed: None,
            held: None,
            caller_at: 0,
            agent_at: 0,
            deferred: None,
            left_ms: 0,
            queued: Vec::new(),
            said: Vec::new(),
            results: VecDeque::new(),
            orchestrated: cfg.tasks.orchestrator.is_some() && !cfg.tasks.agents.is_empty(),
            last_update: None,
            answering: Vec::new(),
            undo: None,
            reading: None,
            timed_at: 0,
            #[cfg(test)]
            reader: Some(tests::lexicon_reader),
        }
    }

    /// What the floor holds still to be heard: how long it lasts and its items, and the
    /// agent's recent lines, as of now.
    pub fn set_speech(&mut self, left_ms: Ms, queued: Vec<ItemId>, said: Vec<(Ms, String)>) {
        self.left_ms = left_ms;
        self.queued = queued;
        self.said = said;
    }

    /// The agent said a line apart from the floor (a failure line the pump played, chapter
    /// 10), from `at` on: the holding line counts its silence from there. Live (take 15) a
    /// holding line due from the wait's silence played right after "Thanks for waiting, I'm
    /// back.": "Still working on it, thanks for waiting." (20.2 s, 22.3 s).
    pub fn spoke_apart(&mut self, at: Ms) {
        self.last_speech = self.last_speech.max(at);
    }

    /// The registry's board as the floor and the turn rules see it: a question waiting for a
    /// slot is not open for them yet.
    pub fn shown(&self, mut board: Board) -> Board {
        let deferred = |open: &HitlRequest| self.deferred.as_ref().is_some_and(|d| d.id == open.id);
        if board.open_question.as_ref().is_some_and(deferred) {
            board.open_question = None;
        }
        board
    }

    /// An answer sentence took `ms` from its submit to its first audio. `alone`: nothing was
    /// ahead of it, so that is the session's synthesis time. Otherwise it also waited for the
    /// line before it, and can only show that a synthesis is shorter than was thought.
    pub fn synth_took(&mut self, ms: Ms, alone: bool) {
        self.synth_ms = if alone { ms } else { ms.min(self.synth_ms) };
    }

    /// Whether the session may step down when the caller keeps speaking another language: it
    /// needs a plain conversation path (a `conversation_config`). Without one the caller is
    /// asked to repeat, turn after turn.
    pub fn allow_step_down(&mut self, on: bool) {
        self.step_down = on;
    }

    /// The session's agents that are one MCP tool each (`set_single_calls`).
    pub fn set_single_calls(&mut self, single_calls: Vec<String>) {
        self.single_calls = single_calls;
    }

    /// What the session's agents do (name, own description): a follow-up is read apart
    /// against what the agent that did the task does.
    pub fn set_agent_topics(&mut self, topics: Vec<(String, String)>) {
        self.agent_topics = topics;
    }

    /// The session goes on as the plain path: every turn the background's.
    pub fn step_down(&mut self) {
        self.plain = true;
    }

    /// Whether the caller can answer on a keypad (see [`Policy::keypad`]), as of now.
    /// `tasks.announce` changed mid-session (an `updates` control, chapter 9).
    pub fn set_announce(&mut self, announce: Announce) {
        self.announce = announce;
    }

    pub fn set_keypad(&mut self, on: bool) {
        self.keypad = on;
    }

    /// What the floor holds (`quiet`: nothing of the agent's to play), and whether the caller
    /// has a screen, as of now.
    pub fn set_floor(&mut self, quiet: bool, screen: bool) {
        self.quiet = quiet;
        self.screen = screen;
    }

    /// Something waits for a quiet floor: an update, a question, or a progress line.
    pub fn waits_for_quiet(&self) -> bool {
        !self.quiet
            && (!self.inbox.is_empty()
                || self.deferred.is_some()
                || self
                    .wait
                    .as_ref()
                    .is_some_and(|wait| wait.progress.is_some()))
    }

    /// The settings after an `interaction_update`; they apply from the next decision.
    pub fn set_config(&mut self, cfg: PolicyConfig, client: bool) {
        self.cfg = cfg;
        self.client = client;
    }

    pub fn step(&mut self, now: Ms, input: PolicyInput, board: &Board) -> Vec<PolicyAction> {
        let mut out = Vec::new();
        let plain = self.plain;
        // A question that waits for a slot is not open for the turn rules yet.
        let shown;
        let board = if self.deferred.is_some() {
            shown = self.shown(board.clone());
            &shown
        } else {
            board
        };
        match input {
            PolicyInput::EndOfTurn(turn) => self.end_of_turn(now, turn, board, &mut out),
            PolicyInput::Eager { turn, text } => {
                let categories = self.persona.sensitive_categories();
                let front_decides = board.open_question.is_none()
                    && !self.answer_expected(now)
                    && self.repeat.is_none()
                    && !self.plain
                    && rules::sensitive_value(&text, categories).is_none();
                if self.speculate && front_decides {
                    out.push(PolicyAction::Task(TaskCmd::StartProvisional {
                        turn,
                        text,
                        e_at: None,
                    }));
                }
            }
            PolicyInput::EagerWithdrawn { .. } => {
                if board.provisional.is_some() {
                    out.push(PolicyAction::Task(TaskCmd::DiscardProvisional {
                        reason: "eager_withdrawn",
                    }));
                }
            }
            PolicyInput::UserSpeaking { on } => {
                self.user_speaking = on;
                self.caller_at = now;
                if let Some(asking) = &mut self.asking
                    && asking.reask_at.is_some()
                {
                    // The re-ask counts silence from here.
                    asking.reask_at = Some(now + Ms::from(self.cfg.hitl_reprompt_after_ms));
                }
                // No timers here: they run with the `Tick` that ends the actor's step. When a
                // turn ends, its words follow this input, and a question the caller cut is
                // offered again only if those words were not its answer (a yes said over the
                // re-ask was lost: the question started over before the yes was looked at).
                return out;
            }
            PolicyInput::Front(event) => self.on_front(event, board, &mut out),
            PolicyInput::Lane(event) => self.on_lane(now, event, &mut out),
            PolicyInput::Task(event) => self.on_task(now, event, board, &mut out),
            PolicyInput::ClientAnswer { request, outcome } => {
                // A question on the screen may be answered there before it was asked aloud.
                let waiting = self.deferred.take_if(|deferred| deferred.id == request);
                if waiting.is_some()
                    || self
                        .asking
                        .as_ref()
                        .is_some_and(|asking| asking.request.id == request)
                {
                    out.push(PolicyAction::Task(TaskCmd::Answer {
                        request,
                        outcome,
                        by: AnsweredBy::Client,
                    }));
                } else {
                    let (code, message) = if self.closed.contains(&request) {
                        ("question_closed", "the question was already answered")
                    } else {
                        ("unknown_request_id", "no question has this request_id")
                    };
                    out.push(error(code, message));
                }
            }
            PolicyInput::ClientCancel { task } => {
                match board.tasks.iter().find(|view| view.id == task) {
                    Some(view) => out.push(PolicyAction::Task(TaskCmd::Cancel {
                        task: view.task_ref.clone(),
                        by: "client",
                    })),
                    None => out.push(error("task_unknown", "no task has this task_id")),
                }
            }
            PolicyInput::ClientSpeak { text } => {
                let utterance = self.utterance(Kind::Client, RANK_REPLY, Say::Verbatim(text));
                out.push(PolicyAction::Speak(utterance));
            }
            PolicyInput::ControlLine { kind, say } => {
                let utterance = self.utterance(kind, RANK_REPLY, say);
                out.push(PolicyAction::Speak(utterance));
            }
            PolicyInput::Dtmf { digit } => self.on_key(now, digit, &mut out),
            PolicyInput::CapturedSpeech => {
                // The speech-to-text heard none of it: the caller is pointed at the screen or
                // the keypad, once per question.
                if let Some(asking) = self
                    .asking
                    .as_mut()
                    .filter(|asking| asking.capture && !asking.spoke_over)
                {
                    asking.spoke_over = true;
                    let channel = if asking.keyed.is_some() {
                        KEYPAD
                    } else {
                        SCREEN
                    };
                    let say = filled("sensitive.dont_say", [("channel", channel.to_owned())]);
                    out.push(PolicyAction::Speak(self.utterance(
                        Kind::Notice,
                        RANK_REPLY,
                        say,
                    )));
                }
            }
            PolicyInput::Delivered { request } => {
                // A sign-in link asked on the screen is on its way there: the caller hears so
                // after the line that announced it. A question that closes first drops it.
                let open = self.asking.as_ref().filter(|asking| {
                    asking.request.id == request
                        && asking.request.kind == HitlKind::AuthUrl
                        && asking.screen.is_some()
                });
                if let Some(request) = open.map(|asking| asking.request.clone()) {
                    let line = self.ask(&request, Kind::Notice, template("auth_url.after"));
                    if let Some(asking) = self.asking.as_mut() {
                        asking.items.push(line.item);
                    }
                    out.push(PolicyAction::Speak(line));
                }
            }
            PolicyInput::TaskSeen { task } => self.seen(&task, &mut out),
            PolicyInput::Narrated { item, line } => self.narrated(item, line, &mut out),
            PolicyInput::Read { turn, label } => {
                if let Some(pending) = self.reading.take_if(|pending| pending.turn == turn) {
                    self.replied(now, pending, label, board, &mut out);
                }
            }
            PolicyInput::Ready => {
                // An orchestrator's children may break into an answer: their two fixed lines
                // are made ahead, first, so a short deadline is not spent on their synthesis
                // (warm-ups stop once a task runs: last in the list, a caller's first request
                // left them unmade).
                let breaks_in = ["question.interrupt", "question.resume"]
                    .into_iter()
                    .filter(|_| self.orchestrated)
                    .filter_map(|id| self.persona.resolve(&template(id), 0));
                let lines: Vec<String> = breaks_in.chain(self.persona.warm_lines()).collect();
                for line in lines {
                    let warm = self.utterance(Kind::Warm, RANK_WARM, Say::Verbatim(line));
                    out.push(PolicyAction::Speak(Utterance {
                        discard: true,
                        ..warm
                    }));
                }
            }
            PolicyInput::Tick => {}
        }
        self.timers(now, board, &mut out);
        self.timed_at = now;
        if plain {
            // Stepped down: the pack's lines are in a language the caller does not speak.
            let pack_line = |action: &PolicyAction| match action {
                PolicyAction::Speak(line) => !matches!(line.kind, Kind::Relay | Kind::Client),
                _ => false,
            };
            out.retain(|action| !pack_line(action));
        }
        out
    }

    /// The next time `step(now, PolicyInput::Tick, ..)` has something to do.
    pub fn next_wakeup(&self) -> Option<Ms> {
        let turn = self.turn.as_ref();
        let asking = self.asking.as_ref();
        // The acknowledgement waits while the caller speaks again (`timers`).
        let acking = turn.filter(|_| !self.user_speaking);
        [
            acking.and_then(|turn| turn.ack_due),
            acking.and_then(|turn| turn.audible_by),
            asking.and_then(|asking| asking.reask_at),
            asking.map(|asking| asking.deadline),
            asking
                .and_then(|asking| asking.key_at)
                .map(|at| at + Ms::from(self.cfg.hitl_key_gap_ms)),
            self.repeat.as_ref().and_then(|repeat| repeat.deadline),
            self.wait_line_due().map(|(due, _)| due),
            self.wait
                .as_ref()
                .and_then(|wait| wait.progress_due(&self.cfg)),
            // An update waits for the conversation to fall idle, or for a request that never
            // got a task to stop holding it back.
            (!self.inbox.is_empty()).then_some(self.idle_at()),
            self.owed
                .as_ref()
                .filter(|owed| owed.task.is_none() && !self.inbox.is_empty())
                .map(|owed| owed.at + OWED_BACKSTOP_MS),
            self.update_offer.as_ref().and_then(|offer| offer.until),
            self.undo.as_ref().and_then(|undo| undo.until),
            // A question waiting for a slot: when it can wait no longer.
            self.deferred
                .as_ref()
                .map(|deferred| deferred.expires_at.saturating_sub(ASK_MS + self.left_ms)),
        ]
        .into_iter()
        .flatten()
        // What was due by the last step's timers and is still there waits for an input (a
        // slot, the caller's turn, the open question closing), not for the clock: woken for it
        // the actor stepped every millisecond (an update waiting through a long answer, an
        // undo offer nobody answered for the rest of the call).
        .filter(|at| *at > self.timed_at)
        .min()
    }

    /// A sensitive value is being captured on the screen or the keypad (the capture window of
    /// chapter 4).
    pub fn capturing(&self) -> bool {
        self.asking.as_ref().is_some_and(|asking| asking.capture)
    }

    /// The open question's value is typed on the keypad.
    pub fn keyed(&self) -> bool {
        self.asking
            .as_ref()
            .is_some_and(|asking| asking.keyed.is_some())
    }

    /// The reading the caller's spoken answer to the open question will get, its words not
    /// known yet: the front primes it while the question is said (live MA2).
    pub fn open_reading(&self) -> Option<Reading> {
        let asking = self.asking.as_ref()?;
        let spoken = asking.screen.is_none() && asking.keyed.is_none() && !asking.capture;
        let pending = Pending {
            turn: TurnId(0),
            eot: None,
            stage: Stage::Choice,
            asked: Asked::Open(asking.request.clone()),
        };
        spoken.then(|| self.reading(&pending))
    }

    /// A question is open, or the answer window after one is.
    pub fn answer_expected(&self, now: Ms) -> bool {
        self.asking.is_some() || now < self.answer_window_until
    }

    // ---- settling a turn

    fn end_of_turn(
        &mut self,
        now: Ms,
        mut eot: EndOfTurn,
        board: &Board,
        out: &mut Vec<PolicyAction>,
    ) {
        self.user_speaking = false;
        self.last_speech = now;
        self.caller_at = now;
        // The caller went on before their last turn was decided, within `GOING_ON_MS` of its
        // end: the pause was not the end of the turn, and this turn carries its words. Live
        // (launch demo take 22, 28 Oct): "Great. Can you also get" ended at a pause (Smart
        // Turn was sure), "the download numbers in the terminal?" came 0.3 s later as a turn
        // of its own, and its words alone were read as a follow-up of the finished browser
        // task: the browser agent was asked for "the download numbers in the terminal" and
        // asked back which project was meant.
        let onset = eot.onset_at.unwrap_or(eot.at);
        if let Some(last) = self.turn.as_ref().filter(|last| !last.settled)
            && last.id != eot.turn
            && onset <= last.e + GOING_ON_MS
            && !last.text.trim().is_empty()
        {
            eot.text = format!("{} {}", last.text.trim(), eot.text.trim());
        }
        self.turn = Some(Turn {
            id: eot.turn,
            e: eot.at,
            onset: eot.onset_at.unwrap_or(eot.at),
            text: eot.text.clone(),
            settled: false,
            line_for: None,
            line: None,
            ack_due: None,
            audible_by: None,
            acked: false,
            asked_twice: false,
            started: just_started(eot.at, board).map(|view| view.task_ref.clone()),
        });
        // Row 1: nothing the caller said to the agent.
        let echo = eot.suspected_echo && !self.answers_after_question(&eot);
        if echo || eot.text.trim().is_empty() {
            return self.settle(decision(Act::Wait), true, board, out);
        }
        // Stepped down: the plain path. Every turn is the background's.
        if self.plain {
            return self.settle(fallback(), true, board, out);
        }
        // A turn in another language never binds and never starts a task.
        if eot.foreign {
            return self.foreign_turn(eot.turn, board, out);
        }
        // A card number, a password or a one-time code is never taken by voice: the caller is
        // told so, and the words go no further.
        let categories = self.persona.sensitive_categories();
        // While the screen asks for such a value, its digits alone are that value.
        let asked_value = self
            .asking
            .as_ref()
            .filter(|asking| asking.screen.is_some())
            .and_then(|asking| asking.request.question.as_deref())
            .and_then(|question| rules::sensitive_category(question, categories))
            .filter(|_| rules::says_digits(&eot.text));
        if let Some(category) = rules::sensitive_value(&eot.text, categories).or(asked_value) {
            let label = ("field_label", format!("your {category}"));
            let asking = self
                .asking
                .as_ref()
                .filter(|asking| asking.screen.is_some());
            let say = if let Some(asking) = asking {
                let keypad = asking.keyed.is_some();
                let channel = if keypad { KEYPAD } else { SCREEN };
                filled("sensitive.dont_say", [("channel", channel.to_owned())])
            } else {
                filled("sensitive.decline", [label])
            };
            return self.turn_away(eot.turn, Act::Refuse, "sensitive_value", say, board, out);
        }
        // Rows 2 to 7: the answer to what the caller was asked. A choice or a value is
        // whatever the caller says, so it only counts once the question has begun to play:
        // words from before it (the question opened while the caller was still talking) go
        // the ordinary way, and the question is asked when this turn is settled.
        self.answers(now, eot, Stage::Screen, board, out);
    }

    /// Rows 2 to 7 from `stage` on: the turn as the answer to the question on the screen, the
    /// open question, the choice offer, the undo offer, the offer of updates and "change it,
    /// or a second one?", in that order. The front reads each in context (chapter 9: no
    /// phrase list decides what the caller meant); a turn that answers none of them is the
    /// front's to decide as any other.
    fn answers(
        &mut self,
        now: Ms,
        eot: EndOfTurn,
        mut stage: Stage,
        board: &Board,
        out: &mut Vec<PolicyAction>,
    ) {
        let screen = self
            .asking
            .as_ref()
            .and_then(|asking| asking.screen.clone());
        let began = eot.onset_at.unwrap_or(eot.at);
        loop {
            let asked = match stage {
                // A yes or a no to a question on the screen answers nothing and asks for
                // nothing: the caller is pointed at the screen again.
                Stage::Screen => screen.clone().map(Asked::Screen),
                Stage::Open => match board.open_question.clone() {
                    Some(request)
                        if screen.is_none()
                            && (request.kind == HitlKind::Confirm
                                || self.asked_before(&eot, &request)) =>
                    {
                        // Words that were hard to hear, or a question answered on the screen,
                        // answer nothing.
                        let on_screen = matches!(request.kind, HitlKind::Form | HitlKind::AuthUrl);
                        if eot.unclear || on_screen {
                            return self.answer(now, &eot, &request, Parsed::Unclear, board, out);
                        }
                        Some(Asked::Open(request))
                    }
                    _ => None,
                },
                // "Keep waiting, or should I stop it?"
                Stage::Choice => self
                    .offer
                    .take()
                    .filter(|until| now <= *until)
                    .map(|_| Asked::Choice),
                // "It's already done; undo it?": yes starts the compensating task on the agent
                // that did it, no leaves it as it is.
                Stage::Undo => match self.undo.take() {
                    Some(undo) if undo.until.is_some_and(|until| began <= until) => {
                        Some(Asked::Undo(undo))
                    }
                    Some(undo) => {
                        out.push(PolicyAction::Drop { item: undo.item });
                        None
                    }
                    None => None,
                },
                // The offer of updates: yes plays them, no keeps them for later.
                Stage::Updates => match self.update_offer.take() {
                    Some(offer) if offer.until.is_some_and(|until| began <= until) => {
                        Some(Asked::Updates(offer))
                    }
                    Some(offer) => {
                        out.push(PolicyAction::Drop { item: offer.item });
                        if offer.until.is_none() {
                            // Never heard: it is offered again at the next slot.
                            let offered = self.inbox.iter_mut();
                            let offered =
                                offered.filter(|update| offer.tasks.contains(&update.task));
                            offered.for_each(|update| update.offered = false);
                        }
                        None
                    }
                    None => None,
                },
                // "Change it, or a second one?". The caller is not asked the same thing about
                // other words.
                Stage::Which => self.repeat.take().map(|repeat| {
                    out.push(PolicyAction::Drop { item: repeat.item });
                    if let Some(turn) = &mut self.turn {
                        turn.asked_twice = true;
                    }
                    Asked::Which(repeat)
                }),
                Stage::Front => return self.front_decides(eot, board, out),
            };
            stage = stage.next();
            if let Some(asked) = asked {
                let pending = Pending {
                    turn: eot.turn,
                    eot: Some(eot),
                    stage,
                    asked,
                };
                return self.read(now, pending, board, out);
            }
        }
    }

    /// The front reads the turn against what it may answer; its label continues the turn
    /// ([`Policy::replied`]).
    fn read(&mut self, now: Ms, pending: Pending, board: &Board, out: &mut Vec<PolicyAction>) {
        #[cfg(test)]
        if let Some(reader) = self.reader {
            let label = reader(self, &pending);
            return self.replied(now, pending, label, board, out);
        }
        let _ = (now, board);
        out.push(PolicyAction::Read(self.reading(&pending)));
        self.reading = Some(pending);
    }

    /// What the front is given for one reading: the agent's question (or the caller's tasks),
    /// the caller's words and the labels.
    fn reading(&self, pending: &Pending) -> Reading {
        let resolve = |say: &Say| self.persona.resolve(say, 0).unwrap_or_default();
        let agent = |text: &str| front_model::reading_context("Agent", text);
        let labels = |labels: &[(&str, &str)]| -> Vec<(String, String)> {
            labels
                .iter()
                .map(|(label, means)| ((*label).to_owned(), (*means).to_owned()))
                .collect()
        };
        let reply = ReadingKind::Reply;
        let (kind, context, labels) = match &pending.asked {
            Asked::Screen(say) => (reply, agent(&resolve(say)), labels(&YES_NO)),
            Asked::Open(request) => {
                let question = match request.kind {
                    HitlKind::Confirm => {
                        let question = self.persona.render_confirm(request).join(" ");
                        question.trim().to_owned()
                    }
                    _ => resolve(&self.question_say(request)),
                };
                (reply, agent(&question), answer_labels(request))
            }
            Asked::Choice => (
                reply,
                agent(&resolve(&template("choice_offer"))),
                labels(&WAIT_OR_STOP),
            ),
            Asked::Undo(undo) => (reply, agent(&undo.asked), labels(&YES_NO)),
            Asked::Updates(offer) => (reply, agent(&offer.asked), labels(&YES_NO)),
            Asked::Which(_) => (
                reply,
                agent(&resolve(&template("duplicate.ask"))),
                labels(&WHICH),
            ),
            Asked::Same(_, tasks) => {
                let mut lines = vec!["Earlier requests:".to_owned()];
                let mut options = Vec::new();
                for (at, (_, title)) in tasks.iter().enumerate() {
                    lines.push(front_model::reading_context(&(at + 1).to_string(), title));
                    options.push(((at + 1).to_string(), title.clone()));
                }
                options.extend(labels(&NEW));
                (ReadingKind::Same, lines.join("\n"), options)
            }
            Asked::Follows(_, _, title, topic) => {
                let mut options = vec![("1".to_owned(), title.clone())];
                options.extend(labels(&NEW));
                options.extend(labels(&[THANKS]));
                let context = front_model::follows_context(title, topic.as_deref());
                (ReadingKind::Follows, context, options)
            }
            Asked::Started(_, _, title) => {
                let mut options = vec![("1".to_owned(), title.clone())];
                options.extend(labels(&NEW));
                let context = front_model::started_context(title);
                (ReadingKind::Started, context, options)
            }
        };
        let said = match &pending.eot {
            Some(eot) => eot.text.clone(),
            None => self
                .turn
                .as_ref()
                .map(|turn| turn.text.clone())
                .unwrap_or_default(),
        };
        Reading {
            turn: pending.turn,
            kind,
            context,
            said,
            labels,
        }
    }

    /// The front's label for the turn it read: the row it answers goes on, or the next row
    /// is tried. Without a label (the front had none in time) a question asked is asked
    /// again, and an offer is not taken.
    fn replied(
        &mut self,
        now: Ms,
        pending: Pending,
        label: Option<String>,
        board: &Board,
        out: &mut Vec<PolicyAction>,
    ) {
        let Pending {
            turn,
            eot,
            stage,
            asked,
        } = pending;
        if self.settled(turn).is_none() {
            return;
        }
        let label = label.as_deref().unwrap_or(OTHER);
        let Some(eot) = eot else {
            if let Some(turn) = &mut self.turn {
                turn.asked_twice = true;
            }
            // A follow-up the front named: about something else, it is a new request, and words
            // that ask for nothing are thanks (the persona's line, not an answer from the
            // result: live rehearsal 1, "Thanks, Eva." was answered with the release check);
            // with no reading in time the front's word stands.
            if let Asked::Follows(decision, ..) = asked {
                let decision = match label {
                    OTHER | "1" => decision,
                    label if label == THANKS.0 => ActDecision {
                        act: Act::Social,
                        task: None,
                        say: None,
                        intent: Some("thanks"),
                        ..decision
                    },
                    _ => ActDecision {
                        act: Act::Delegate,
                        task: None,
                        ..decision
                    },
                };
                return self.settle(decision, true, board, out);
            }
            // A request delegated beside a task just started: about it, it updates that task;
            // otherwise (or with no reading in time) the front's word stands. About a task that
            // completed while the words were read, they are the request its answer (on its way)
            // answers: no second task (live PR8, r17: the relay webhook answered 2 s after the
            // request, 0.4 s after the follow-on's words).
            if let Asked::Started(decision, task, _) = asked {
                let view = board.tasks.iter().find(|view| view.task_ref == task);
                let answered = view.is_some_and(|view| view.state == TaskState::Completed);
                let decision = match label {
                    "1" => ActDecision {
                        act: if answered { Act::Wait } else { Act::Update },
                        task: Some(task),
                        ..decision
                    },
                    _ => decision,
                };
                return self.settle(decision, true, board, out);
            }
            // A delegated request that may repeat a task (the turn is settled already).
            let Asked::Same(decision, tasks) = asked else {
                return;
            };
            let same = label
                .parse::<usize>()
                .ok()
                .and_then(|at| tasks.get(at.wrapping_sub(1)));
            return match same {
                Some((task, _)) => self.ask_which(turn, task.clone(), board, out),
                None => self.settle(decision, true, board, out),
            };
        };
        match asked {
            Asked::Screen(say) => match label {
                YES | NO => self.turn_away(eot.turn, Act::Wait, "screen_question", say, board, out),
                _ => self.answers(now, eot, stage, board, out),
            },
            Asked::Open(request) => {
                let parsed = answer_of(&request, label, &eot.text);
                self.answer(now, &eot, &request, parsed, board, out);
            }
            Asked::Choice => match label {
                STOP => {
                    let stop = ActDecision {
                        source: ActSource::Model,
                        ..decision(Act::Abort)
                    };
                    self.settle(stop, true, board, out);
                }
                WAIT => {
                    let keep = template("hitl.accept");
                    self.turn_away(eot.turn, Act::Wait, "choice_offer", keep, board, out);
                }
                _ => self.answers(now, eot, stage, board, out),
            },
            Asked::Undo(undo) => match label {
                YES => {
                    if let Some(turn) = &mut self.turn {
                        turn.text = undo.text;
                    }
                    let undo = ActDecision {
                        task: Some(undo.task),
                        source: ActSource::RuleOverride(UNDO_ACCEPTED),
                        ..decision(Act::FollowUp)
                    };
                    self.settle(undo, true, board, out);
                }
                NO => {
                    let leave = template("hitl.timeout");
                    self.turn_away(eot.turn, Act::Wait, "undo_offer", leave, board, out);
                }
                _ => self.answers(now, eot, stage, board, out),
            },
            Asked::Updates(offer) => match label {
                YES => self.take_offer(&eot, &offer, board, out),
                NO => {
                    let keep = template("hitl.accept");
                    self.turn_away(eot.turn, Act::Wait, "update_offer", keep, board, out);
                }
                _ => self.answers(now, eot, stage, board, out),
            },
            Asked::Which(repeat) => {
                let live = |view: &TaskView| view.task_ref == repeat.task && view.state.is_live();
                let act = match label {
                    SECOND => Some(Act::Delegate),
                    // A task that is over cannot be revised: the change is a request of its own.
                    CHANGE if board.tasks.iter().any(live) => Some(Act::Update),
                    CHANGE => Some(Act::Delegate),
                    _ => None,
                };
                if let Some(act) = act {
                    if board.provisional.is_some() {
                        out.push(PolicyAction::Task(TaskCmd::DiscardProvisional {
                            reason: "answer",
                        }));
                    }
                    if let Some(turn) = &mut self.turn {
                        turn.text = repeat.text;
                    }
                    let answered = ActDecision {
                        task: Some(repeat.task),
                        source: ActSource::Model,
                        ..decision(act)
                    };
                    return self.settle(answered, true, board, out);
                }
                if label == NEITHER {
                    let leave = template("hitl.cancel");
                    return self.turn_away(
                        eot.turn,
                        Act::Wait,
                        "near_duplicate",
                        leave,
                        board,
                        out,
                    );
                }
                self.answers(now, eot, stage, board, out);
            }
            Asked::Same(..) | Asked::Follows(..) | Asked::Started(..) => {}
        }
    }

    /// Row 8: the front decides every other turn in context (chapter 9).
    fn front_decides(&mut self, eot: EndOfTurn, board: &Board, out: &mut Vec<PolicyAction>) {
        // A continuer, a cancel, a repeat, a status question, an amendment, a control: the
        // lexicon's phrase lists are its prompt's examples, never a decision of their own. A
        // run started eagerly on the same words goes on; words that were hard to hear start
        // none.
        let mut speculate = self.speculate && !eot.unclear;
        if board.provisional.is_some() {
            if eot.eager_same == Some(true) {
                speculate = false;
                out.push(PolicyAction::Task(TaskCmd::ConfirmProvisional {
                    e_at: eot.at,
                }));
            } else {
                out.push(PolicyAction::Task(TaskCmd::DiscardProvisional {
                    reason: "eager_different",
                }));
            }
        }
        let live = board.tasks.iter().any(|view| view.state.is_live());
        let allowed = Act::ALL
            .into_iter()
            .filter(|act| match act {
                // No backchannel is emitted in this build; "say that again" is a control.
                Act::Backchannel | Act::Echo => false,
                Act::Update | Act::Abort | Act::Progress => live,
                Act::FollowUp => board.tasks.iter().any(followable),
                _ => true,
            })
            .collect();
        out.push(PolicyAction::RequestFront {
            turn: eot.turn,
            text: eot.text,
            allowed,
            speculate,
        });
    }

    /// Settles a turn that starts nothing: the caller is told why in one line.
    fn turn_away(
        &mut self,
        turn: TurnId,
        act: Act,
        rule: &'static str,
        say: Say,
        board: &Board,
        out: &mut Vec<PolicyAction>,
    ) {
        if let Some(latest) = &mut self.turn {
            latest.settled = true;
        }
        out.push(PolicyAction::Settled {
            turn,
            act,
            source: ActSource::RuleOverride(rule),
        });
        if board.provisional.is_some() {
            out.push(PolicyAction::Task(TaskCmd::DiscardProvisional {
                reason: rule,
            }));
        }
        out.push(PolicyAction::Speak(self.reply(turn, Kind::Notice, say)));
    }

    /// A turn in another language (the recogniser's language field, or the front's `wait`
    /// `foreign`): the caller is asked to say it again. After `FOREIGN_TURNS` in a row a
    /// session with a plain path hears the persona's notice and steps down.
    fn foreign_turn(&mut self, turn: TurnId, board: &Board, out: &mut Vec<PolicyAction>) {
        self.foreign_turns = self.foreign_turns.saturating_add(1);
        if !(self.step_down && self.foreign_turns >= FOREIGN_TURNS) {
            let again = template("repeat_request");
            return self.turn_away(turn, Act::Question, "language", again, board, out);
        }
        let notice = Say::Bank {
            group: "social",
            intent: Some("language_not_supported"),
        };
        self.turn_away(turn, Act::Refuse, "language", notice, board, out);
        // What is still open was asked in English.
        out.extend(
            self.repeat
                .take()
                .map(|repeat| PolicyAction::Drop { item: repeat.item }),
        );
        self.plain = true;
        out.push(PolicyAction::StepDown);
    }

    /// Whether the turn began after the caller had heard a question whose answers are its own
    /// words: a choice, a value, or "change it, or a second one?". Such a turn is the caller's
    /// even when the echo filter suspects it ("a second one"): an echo of the question begins
    /// while it plays, not after it.
    fn answers_after_question(&self, eot: &EndOfTurn) -> bool {
        let choice = self
            .asking
            .as_ref()
            .filter(|asking| asking.request.kind != HitlKind::Confirm)
            .and_then(|asking| asking.readback_heard_at);
        let which = self.repeat.as_ref().and_then(|repeat| repeat.heard_at);
        let onset = eot.onset_at.unwrap_or(eot.at);
        let margin = Ms::from(self.cfg.heard_margin_ms);
        choice
            .or(which)
            .is_some_and(|heard| onset >= heard + margin)
    }

    /// The live or just-finished tasks a new request may repeat, with their titles, newest
    /// first. Only on a session whose tools can change something: a second lookup costs
    /// nothing, and a caller who asks the same question again wants its answer again. A
    /// finished task that committed nothing is a lookup too (live B1: "And which one is the
    /// largest?" after the planets answer was read as that request again and asked "change
    /// it, or a second one?"). Whether it repeats one is the front's to read (chapter 9: no
    /// word overlap decides).
    fn repeatable(&self, at: Ms, board: &Board) -> Vec<(TaskRef, String)> {
        let lately = |view: &TaskView| {
            let done = |(task, done): &(TaskId, Ms)| {
                *task == view.id && at.saturating_sub(*done) <= JUST_FINISHED_MS
            };
            view.effects != Effects::NoneCommitted && self.finished.iter().any(done)
        };
        board
            .tasks
            .iter()
            .rev()
            .filter(|view| view.side_effects)
            .filter(|view| (view.state.is_live() && !view.cancelling) || lately(view))
            .map(|view| (view.task_ref.clone(), view.title.clone()))
            .collect()
    }

    /// The caller is asked whether a request that repeats `task` changes it or is a second one.
    fn ask_which(&mut self, id: TurnId, task: TaskRef, board: &Board, out: &mut Vec<PolicyAction>) {
        let Some(text) = self.turn.as_ref().map(|turn| turn.text.clone()) else {
            return;
        };
        out.push(PolicyAction::Settled {
            turn: id,
            act: Act::Question,
            source: ActSource::RuleOverride("near_duplicate"),
        });
        if board.provisional.is_some() {
            out.push(PolicyAction::Task(TaskCmd::DiscardProvisional {
                reason: "near_duplicate",
            }));
        }
        let ask = self.reply(id, Kind::Question, template("duplicate.ask"));
        self.asked.push(ask.item);
        self.repeat = Some(Repeat {
            text,
            task,
            item: ask.item,
            heard_at: None,
            deadline: None,
        });
        out.push(PolicyAction::Speak(ask));
    }

    /// Whether `turn` is the latest turn, and settled.
    fn settled(&self, turn: TurnId) -> Option<bool> {
        let latest = self.turn.as_ref().filter(|latest| latest.id == turn)?;
        Some(latest.settled)
    }

    fn on_front(&mut self, event: FrontEvent, board: &Board, out: &mut Vec<PolicyAction>) {
        match event {
            FrontEvent::ActKnown {
                turn,
                act,
                task,
                source,
                ..
            } => {
                if self.settled(turn) == Some(false) {
                    // Its line comes with `Done`.
                    let known = ActDecision {
                        task,
                        source,
                        ..decision(act)
                    };
                    self.settle(known, false, board, out);
                }
            }
            // The front reads the caller's words as another language: no turn of the
            // conversation's (its `wait` settled it at the act). A turn it reads otherwise ends
            // a run of them.
            FrontEvent::Done { turn, decision } if decision.wait == Some(WaitReason::Foreign) => {
                if self.settled(turn).is_some() {
                    self.foreign_turn(turn, board, out);
                }
            }
            FrontEvent::Done { turn, decision } => match self.settled(turn) {
                Some(false) => {
                    self.foreign_turns = 0;
                    self.settle(decision, true, board, out)
                }
                Some(true) => {
                    self.foreign_turns = 0;
                    self.front_line(turn, decision.say, decision.intent, out)
                }
                None => {}
            },
            FrontEvent::Failed { turn } => match self.settled(turn) {
                // No act: no guess either (chapter 9). The caller is asked to say it again.
                Some(false) => self.not_decided(turn, board, out),
                Some(true) => self.front_line(turn, None, None, out),
                None => {}
            },
            // The actor applies a control; its turn is settled by the `Done` that follows.
            FrontEvent::Unavailable | FrontEvent::Control { .. } => {}
            FrontEvent::Notify {
                turn,
                task,
                announce,
            } => {
                let Some(view) = board.tasks.iter().find(|view| view.task_ref == task) else {
                    return;
                };
                // The turn's own line is the task's status (a progress act).
                let _ = turn;
                self.told.retain(|(told, _)| *told != view.id);
                self.told.push((view.id.clone(), announce));
            }
        }
    }

    /// The front gave no decision for the turn in its budget: nothing is guessed, the caller
    /// is asked to say it again.
    fn not_decided(&mut self, turn: TurnId, board: &Board, out: &mut Vec<PolicyAction>) {
        if let Some(latest) = &mut self.turn {
            latest.settled = true;
        }
        out.push(PolicyAction::Settled {
            turn,
            act: Act::Question,
            source: ActSource::RuleOverride("front_failed"),
        });
        if board.provisional.is_some() {
            out.push(PolicyAction::Task(TaskCmd::DiscardProvisional {
                reason: "front_failed",
            }));
        }
        let again = self.reply(turn, Kind::Question, template("repeat_request"));
        self.asked.push(again.item);
        out.push(PolicyAction::Speak(again));
    }

    /// The model front's line for a turn settled at its first character.
    fn front_line(
        &mut self,
        turn: TurnId,
        line: Option<String>,
        intent: Option<&'static str>,
        out: &mut Vec<PolicyAction>,
    ) {
        // The line of a delegate still read against a task just started is its decision's,
        // for whichever act the reading makes it.
        if let Some(Pending {
            turn: of,
            asked: Asked::Started(decision, ..),
            ..
        }) = &mut self.reading
            && *of == turn
        {
            decision.say = line;
            decision.intent = intent;
            return;
        }
        let Some(latest) = &mut self.turn else {
            return;
        };
        if let Some(act) = latest.line_for.take() {
            self.speak_front(turn, act, line, intent, out);
        } else if latest.ack_due.is_some() {
            latest.line = line;
        }
    }

    /// Social, question and refuse: the front's line, or the persona's bank line.
    fn speak_front(
        &mut self,
        turn: TurnId,
        act: Act,
        line: Option<String>,
        intent: Option<&'static str>,
        out: &mut Vec<PolicyAction>,
    ) {
        let (kind, group) = match act {
            // A social line with neither words nor an intent would be a guess.
            Act::Social if line.is_none() && intent.is_none() => return,
            Act::Social => (Kind::Answer, "social"),
            Act::Question => (Kind::Question, "clarify"),
            Act::Refuse => (Kind::Answer, "refuse"),
            _ => return,
        };
        let say = line.map_or(Say::Bank { group, intent }, Say::Front);
        let reply = self.reply(turn, kind, say);
        if act == Act::Question {
            self.asked.push(reply.item);
        }
        out.push(PolicyAction::Speak(reply));
    }

    /// Row 9, and rows 1 and 3 to 7 with their decision already made. `complete: false`: the
    /// model front's act is known and its line is still to come.
    fn settle(
        &mut self,
        decision: ActDecision,
        complete: bool,
        board: &Board,
        out: &mut Vec<PolicyAction>,
    ) {
        let Some(turn) = &mut self.turn else {
            return;
        };
        turn.settled = true;
        let (id, e, onset, text) = (turn.id, turn.e, turn.onset, turn.text.clone());
        let started = turn.started.clone();
        let plain = self.plain;
        let ask_which = !turn.asked_twice && !plain;
        let named = decision
            .task
            .as_ref()
            .and_then(|task| board.tasks.iter().find(|view| view.task_ref == *task));
        let target = named
            .filter(|view| view.state.is_live())
            .or_else(|| board.tasks.iter().rev().find(|view| view.state.is_live()));
        let finished = named.or(board.tasks.last()).filter(|_| target.is_none());
        let presence = decision.intent == Some("presence");
        // An act on a task that does not run: a new request, or nothing to stop.
        let act = match decision.act {
            Act::Update if target.is_none() => Act::Delegate,
            Act::Progress if target.is_none() && !presence => Act::Delegate,
            Act::Abort if target.is_none() && finished.is_none() => Act::Wait,
            act => act,
        };
        // A hesitant request stays one task: the front marks words that go on with the request
        // the caller made a moment ago as an update of it, and they join it.
        if act == Act::Update
            && !plain
            && let Some(task) = self.goes_on(decision.task.as_ref(), onset, board)
        {
            out.push(PolicyAction::Settled {
                turn: id,
                act: Act::Update,
                source: ActSource::RuleOverride("request_goes_on"),
            });
            if board.provisional.is_some() {
                out.push(PolicyAction::Task(TaskCmd::DiscardProvisional {
                    reason: "request_goes_on",
                }));
            }
            out.push(PolicyAction::Task(TaskCmd::Continue {
                task,
                text: text.clone(),
            }));
            if let Some(owed) = &mut self.owed {
                owed.at = e;
                owed.words = text;
            }
            // No second acknowledgement; only something audible if nothing is for long.
            if let Some(turn) = &mut self.turn {
                turn.audible_by = Some(e + Ms::from(self.cfg.ack_hard_deadline_ms));
            }
            return;
        }
        // A request about a finished task: a follow-up on the agent that did it (chapter 8).
        let done: Vec<&TaskView> = board.tasks.iter().filter(|view| followable(view)).collect();
        let heard_last = self.last_update.as_ref();
        let last = heard_last
            .filter(|(_, at)| e.saturating_sub(*at) <= BACK_REFERENCE_MS)
            .and_then(|(task, _)| done.iter().find(|view| view.id == *task))
            .map(|view| view.task_ref.clone());
        let named = decision.task.clone();
        // The front names the finished task a follow-up is about (chapter 9: no word check of
        // titles or "it").
        // A task that was one tool call has no conversation to go on with: what follows it is
        // a new request, the orchestrator's to place (live MA7: "Delete my note called Old
        // Shopping List." after a notes search went to the search tool again).
        let single = |task: &TaskRef| {
            let view = done.iter().find(|view| view.task_ref == *task);
            let agent = view.and_then(|view| view.lineage.agent.as_ref());
            agent.is_some_and(|agent| self.single_calls.contains(agent))
        };
        // Words that name another agent of the session are a request for that agent's work,
        // not more of the finished task's: the orchestrator places them (live take 23).
        let elsewhere = |task: &TaskRef| {
            let view = done.iter().find(|view| view.task_ref == *task);
            let named = view.and_then(|view| {
                let agent = view.lineage.agent.as_deref()?;
                rules::other_agent_named(&text, agent, &view.title, &self.agent_topics)
            });
            if let Some(other) = named {
                tracing::debug!(task = %task.0, other, "interaction follow-up names another agent");
            }
            named.is_some()
        };
        let follow = match act {
            Act::FollowUp => named
                .filter(|task| done.iter().any(|view| view.task_ref == *task))
                .or_else(|| last.clone())
                .filter(|task| !single(task))
                .filter(|task| !elsewhere(task)),
            _ => None,
        };
        if let Some(task) = follow {
            // Read apart whether the words are about that task (live MA7).
            let view = done.iter().find(|view| view.task_ref == task);
            let title = view.map(|view| view.title.clone()).unwrap_or_default();
            let agent = view.and_then(|view| view.lineage.agent.as_ref());
            let topic = agent.and_then(|agent| {
                let topics = self.agent_topics.iter();
                topics.into_iter().find(|(name, _)| name == agent)
            });
            let topic = topic.map(|(_, what)| what.clone());
            if ask_which && decision.source == ActSource::Model {
                let pending = Pending {
                    turn: id,
                    eot: None,
                    stage: Stage::Front,
                    asked: Asked::Follows(decision, task, title, topic),
                };
                return self.read(e, pending, board, out);
            }
            return self.follow_up(id, e, text, task, decision, board, out);
        }
        // An act on a finished task the front named with nothing left to follow up.
        let act = if act == Act::FollowUp {
            Act::Delegate
        } else {
            act
        };
        // A request that may repeat a task is not started beside it without a word: the front
        // reads whether it does, and the caller says whether it changes that one or is a
        // second one.
        let tasks = if act == Act::Delegate && ask_which {
            self.repeatable(e, board)
        } else {
            Vec::new()
        };
        if !tasks.is_empty() {
            let pending = Pending {
                turn: id,
                eot: None,
                stage: Stage::Front,
                asked: Asked::Same(decision, tasks),
            };
            return self.read(e, pending, board, out);
        }
        // A request delegated while a task had just started is read apart against it (owner,
        // 26 Oct: live PR2/PR8's "Can you check its delivery status?" right after "Where is my
        // parcel?" started a second task); every other turn is as the front decided. A task
        // that completed after the turn ended is still read against (its answer, on its way,
        // may answer the words: live PR8, r17-verify, the relay answered 33 ms after the
        // follow-on's end, before its decision).
        let modelled = decision.source == ActSource::Model;
        if act == Act::Delegate
            && ask_which
            && modelled
            && let Some(started) = started
            && let Some(view) = board.tasks.iter().find(|view| view.task_ref == started)
            && !view.cancelling
            && (view.state.is_live() || view.state == TaskState::Completed)
        {
            let started = Asked::Started(decision, view.task_ref.clone(), view.title.clone());
            let pending = Pending {
                turn: id,
                eot: None,
                stage: Stage::Front,
                asked: started,
            };
            return self.read(e, pending, board, out);
        }
        let adopt = act == Act::Delegate && board.provisional.is_some();
        out.push(PolicyAction::Settled {
            turn: id,
            act,
            source: decision.source,
        });
        if act != Act::Delegate && board.provisional.is_some() {
            out.push(PolicyAction::Task(TaskCmd::DiscardProvisional {
                reason: act.as_str(),
            }));
        }
        // Something an agent did is done: undoing it is a task of its own, on that agent, and
        // the caller says yes to it first.
        let undoable = |view: &&TaskView| followable(view) && view.side_effects;
        if act == Act::Abort
            && target.is_none()
            && let Some(view) = finished.filter(undoable)
        {
            let title = rules::spoken_title(&view.title);
            let title = ("task_title", possessed(&title).to_owned());
            let say = filled("followup.undo_offer", [title]);
            let asked = self.persona.resolve(&say, 0).unwrap_or_default();
            let ask = self.reply(id, Kind::Question, say);
            self.asked.push(ask.item);
            self.undo = Some(UndoOffer {
                item: ask.item,
                task: view.task_ref.clone(),
                asked,
                text,
                until: None,
            });
            out.push(PolicyAction::Speak(ask));
            return;
        }
        match (act, target) {
            (Act::Delegate, _) => {
                out.push(PolicyAction::Task(TaskCmd::Delegate {
                    turn: id,
                    text: text.clone(),
                    adopt,
                }));
                // Its answer comes before any update.
                self.owed = Some(Owed {
                    task: None,
                    at: e,
                    words: text,
                });
                let fast = self.fast_answers();
                let due = if fast {
                    self.cfg.fast_answer_ms
                } else {
                    self.cfg.ack_due_ms
                };
                // Stepped down, nothing is said before the answer.
                if let Some(turn) = self.turn.as_mut().filter(|_| !plain) {
                    turn.line = decision.say;
                    turn.ack_due = Some(e + Ms::from(due));
                    turn.audible_by = Some(e + Ms::from(self.cfg.ack_hard_deadline_ms));
                }
            }
            (Act::Update, Some(view)) => {
                // Scratch the old answer if part of it was heard, then acknowledge.
                if self.stop_relay(&view.id, out) {
                    let withdrawal = self.reply(id, Kind::Notice, bank("withdrawal"));
                    out.push(PolicyAction::Speak(withdrawal));
                }
                out.push(PolicyAction::Task(TaskCmd::Revise {
                    task: view.task_ref.clone(),
                    correction: text,
                }));
                let say = decision.say.map_or(bank("update_ack"), Say::Front);
                let ack = self.reply(id, Kind::Ack, say);
                let expires_at = e + Ms::from(self.cfg.amend_ack_target_ms) + UNSPOKEN_EXPIRY_MS;
                out.push(PolicyAction::Speak(Utterance {
                    expires_at: Some(expires_at),
                    ..ack
                }));
            }
            (Act::Abort, Some(view)) => {
                self.stop_relay(&view.id, out);
                out.push(PolicyAction::Task(TaskCmd::Cancel {
                    task: view.task_ref.clone(),
                    by: "user",
                }));
                let intent = template("cancel.intent");
                out.push(PolicyAction::Speak(self.reply(id, Kind::Notice, intent)));
            }
            (Act::Abort, None) => {
                let done = template("cancel.already_done");
                out.push(PolicyAction::Speak(self.reply(id, Kind::Notice, done)));
            }
            (Act::Echo, _) => {
                out.push(PolicyAction::Speak(self.reply(
                    id,
                    Kind::Answer,
                    Say::Repeat,
                )));
            }
            (Act::Progress, Some(view)) => {
                let title = ("task_title", rules::spoken_title(&view.title));
                let ahead = board
                    .tasks
                    .iter()
                    .find(|other| other.state == TaskState::Working);
                // Several requests run: the three most recent by title, and how many more.
                let requests: Vec<&TaskView> = board
                    .tasks
                    .iter()
                    .rev()
                    .filter(|task| task.lineage.parent_id.is_none() && task.state.is_live())
                    .collect();
                let titles = || {
                    let three = requests.iter().take(3);
                    let titles: Vec<String> =
                        three.map(|t| rules::spoken_title(&t.title)).collect();
                    ("task_titles", titles.join(", "))
                };
                let many = requests.len() > 1 && decision.task.is_none();
                let say = match (presence, view.state, ahead) {
                    (true, _, _) => filled("presence", [title]),
                    _ if many && requests.len() > 3 => {
                        let more = ("count", (requests.len() - 3).to_string());
                        filled("status.many_more", [titles(), more])
                    }
                    _ if many => filled("status.many", [titles()]),
                    (false, TaskState::Submitted, Some(ahead)) => filled(
                        "status.queued",
                        [("behind", rules::spoken_title(&ahead.title))],
                    ),
                    _ => filled("status.working", [title]),
                };
                out.push(PolicyAction::Speak(self.reply(id, Kind::Progress, say)));
            }
            (Act::Progress, None) => {
                self.speak_front(id, Act::Social, None, decision.intent, out);
            }
            (Act::Social | Act::Question | Act::Refuse, _) if complete => {
                self.speak_front(id, act, decision.say, decision.intent, out);
            }
            (Act::Social | Act::Question | Act::Refuse, _) => {
                if let Some(turn) = &mut self.turn {
                    turn.line_for = Some(act);
                }
            }
            _ => {}
        }
    }

    /// The live request the front's update names, when it is the caller's latest request,
    /// whose answer nobody has heard of yet, ended at most [`GOING_ON_MS`] before this turn
    /// began (or after: the caller was already speaking again): the words go on with it.
    fn goes_on(&self, named: Option<&TaskRef>, onset: Ms, board: &Board) -> Option<TaskRef> {
        let owed = self.owed.as_ref()?;
        let task = owed.task.as_ref()?;
        let view = board.tasks.iter().find(|view| view.id == *task)?;
        if named.is_some_and(|named| *named != view.task_ref) {
            return None;
        }
        let live = view.state.is_live() && !view.cancelling && view.lineage.parent_id.is_none();
        let heard = self
            .relays
            .iter()
            .any(|relay| relay.task == *task && relay.part_heard);
        let soon = onset <= owed.at + GOING_ON_MS;
        (live && !heard && soon).then(|| view.task_ref.clone())
    }

    /// A follow-up about the finished task `task`: a question its result may answer is put
    /// to the narrator first; anything else goes to the task's agent.
    #[allow(clippy::too_many_arguments)]
    fn follow_up(
        &mut self,
        id: TurnId,
        e: Ms,
        text: String,
        task: TaskRef,
        decision: ActDecision,
        board: &Board,
        out: &mut Vec<PolicyAction>,
    ) {
        // The caller's yes to undoing it: the compensating task is the agent's at once (the
        // stored result answers nothing about it).
        let compensating = matches!(decision.source, ActSource::RuleOverride(UNDO_ACCEPTED));
        out.push(PolicyAction::Settled {
            turn: id,
            act: Act::FollowUp,
            source: decision.source,
        });
        if board.provisional.is_some() {
            out.push(PolicyAction::Task(TaskCmd::DiscardProvisional {
                reason: "follow_up",
            }));
        }
        let fast = self.fast_answers();
        let due = if fast {
            self.cfg.fast_answer_ms
        } else {
            self.cfg.ack_due_ms
        };
        if let Some(turn) = &mut self.turn {
            turn.line = decision.say;
            turn.ack_due = Some(e + Ms::from(due));
            turn.audible_by = Some(e + Ms::from(self.cfg.ack_hard_deadline_ms));
        }
        let view = board.tasks.iter().find(|view| view.task_ref == task);
        let result = view.and_then(|view| {
            let kept = self.results.iter().find(|(done, _)| *done == view.id);
            kept.map(|(_, result)| (view.title.clone(), result.clone()))
        });
        // The result is asked first: the narrator answers from it or says it does not, and
        // anything it does not answer goes to the agent (no question-shape check, chapter 9).
        match result.filter(|_| !compensating) {
            Some((title, summary)) => {
                let item = self.item();
                out.push(PolicyAction::Narrate(Narration {
                    item,
                    title: title.clone(),
                    summary: summary.clone(),
                    question: Some(text.clone()),
                    retry: None,
                }));
                self.answering.push(Answering {
                    item,
                    turn: id,
                    task,
                    text,
                    at: e,
                    title,
                    summary,
                });
            }
            None => self.hand_on(id, e, task, text, out),
        }
    }

    /// A follow-up for the agent of `task`, which keeps its context.
    fn hand_on(
        &mut self,
        turn: TurnId,
        e: Ms,
        task: TaskRef,
        text: String,
        out: &mut Vec<PolicyAction>,
    ) {
        out.push(PolicyAction::Task(TaskCmd::FollowUp {
            turn,
            task,
            text: text.clone(),
        }));
        self.owed = Some(Owed {
            task: None,
            at: e,
            words: text,
        });
    }

    /// Fast mode: the median of E to first answer sentence over the latest delegated turns
    /// is under `fast_answer_ms`.
    fn fast_answers(&self) -> bool {
        let mut times: Vec<u32> = self.first_sentence_ms.iter().copied().collect();
        times.sort_unstable();
        let median = times.get(times.len() / 2);
        times.len() >= self.cfg.fast_answer_min_turns as usize
            && median.is_some_and(|median| *median < self.cfg.fast_answer_ms)
    }

    // ---- human in the loop

    /// Whether the caller began this turn after the open question began to play. A typed
    /// turn has no audio to place.
    fn asked_before(&self, eot: &EndOfTurn, request: &HitlRequest) -> bool {
        let asked_at = self
            .asking
            .as_ref()
            .filter(|asking| asking.request.id == request.id)
            .and_then(|asking| asking.asked_at);
        eot.source == EotSource::Typed
            || asked_at.is_some_and(|at| eot.onset_at.unwrap_or(eot.at) >= at)
    }

    /// Row 2: the caller's words as the answer to the open question.
    fn answer(
        &mut self,
        now: Ms,
        eot: &EndOfTurn,
        request: &HitlRequest,
        parsed: Parsed,
        board: &Board,
        out: &mut Vec<PolicyAction>,
    ) {
        let heard_at = self
            .asking
            .as_ref()
            .filter(|asking| asking.request.id == request.id)
            .and_then(|asking| asking.readback_heard_at);
        // A choice or a value binds like a yes: only when the caller began to speak after the
        // question, which carries the options, was heard.
        let onset = eot.onset_at.unwrap_or(eot.at);
        let margin = Ms::from(self.cfg.heard_margin_ms);
        let heard =
            eot.source == EotSource::Typed || heard_at.is_some_and(|at| onset >= at + margin);
        let task = board
            .tasks
            .iter()
            .find(|view| view.id == request.task)
            .map(|view| view.task_ref.clone());
        let accept = |value| HitlOutcome::Accept { value };
        // The outcome, the act the turn counts as, the line that follows, and a re-ask.
        let (outcome, act, line, reask) = match parsed {
            Parsed::Accept { strong } if request.kind == HitlKind::Confirm => {
                match hitl::binds(strong, request, eot, heard_at, self.cfg.heard_margin_ms) {
                    Binding::Binds => (Some(accept(None)), Act::Update, Some("hitl.accept"), None),
                    Binding::WeakYes => (None, Act::Question, None, Some(REASK_WEAK)),
                    Binding::NotHeard => (None, Act::Question, None, Some(REASK_NOT_HEARD)),
                }
            }
            Parsed::Accept { .. } | Parsed::Unclear => {
                (None, Act::Question, None, Some(REASK_UNCLEAR))
            }
            Parsed::Decline {
                correction: Some(correction),
            } => {
                if let Some(task) = &task {
                    out.push(PolicyAction::Task(TaskCmd::Revise {
                        task: task.clone(),
                        correction,
                    }));
                }
                (None, Act::Update, Some("hitl.decline_correction"), None)
            }
            Parsed::Decline { correction: None } => {
                let decline = HitlOutcome::Decline { correction: None };
                (Some(decline), Act::Update, Some("hitl.decline"), None)
            }
            Parsed::Cancel => (
                Some(HitlOutcome::Cancel),
                Act::Abort,
                Some("hitl.cancel"),
                None,
            ),
            Parsed::Choice(_) | Parsed::Value(_) if !heard => {
                (None, Act::Question, None, Some(REASK_NOT_HEARD))
            }
            Parsed::Choice(index) => {
                let value = request
                    .options
                    .get(index)
                    .cloned()
                    .map(serde_json::Value::String);
                (Some(accept(value)), Act::Update, Some("hitl.accept"), None)
            }
            Parsed::Value(value) => {
                let value = Some(serde_json::Value::String(value));
                (Some(accept(value)), Act::Update, Some("hitl.accept"), None)
            }
        };
        if let Some(turn) = &mut self.turn {
            turn.settled = true;
        }
        out.push(PolicyAction::Settled {
            turn: eot.turn,
            act,
            source: ActSource::Model,
        });
        if board.provisional.is_some() {
            out.push(PolicyAction::Task(TaskCmd::DiscardProvisional {
                reason: "answer",
            }));
        }
        if let Some(outcome) = outcome {
            out.push(PolicyAction::Task(TaskCmd::Answer {
                request: request.id.clone(),
                outcome,
                by: AnsweredBy::Voice,
            }));
        }
        if let Some(id) = line {
            out.push(PolicyAction::Speak(self.reply(
                eot.turn,
                Kind::Notice,
                template(id),
            )));
        }
        if let Some(which) = reask {
            self.reask(now, which, request, out);
        }
    }

    /// One re-ask of each kind; a second one of the same kind gives the question up.
    fn reask(&mut self, now: Ms, which: usize, request: &HitlRequest, out: &mut Vec<PolicyAction>) {
        let Some(asking) = &mut self.asking else {
            return;
        };
        if std::mem::replace(&mut asking.reasked[which], true) {
            if which != REASK_SILENCE {
                self.time_out(out);
            }
            return;
        }
        if which == REASK_NOT_HEARD && asking.readback_heard_at.is_none() {
            // The read-back is still to come: it plays on, or the caller cut it and the
            // question is offered again. That is the re-ask. A second one after it, with the
            // read-back moved to it, made the yes that follows the question unheard again.
            return;
        }
        asking.reoffer = false;
        let phrase = ("verb_phrase", request.verb_phrase.clone());
        let values: Vec<String> = request
            .params
            .iter()
            .map(|(_, value)| Persona::speak_value(value))
            .collect();
        let options = ("options", request.options.join(", "));
        let say = match (which, request.kind) {
            (REASK_UNCLEAR | REASK_NOT_HEARD, HitlKind::Choose) => {
                filled("hitl.unclear_choose", [options])
            }
            (REASK_UNCLEAR | REASK_NOT_HEARD, HitlKind::Value) => self.question_say(request),
            (REASK_UNCLEAR, HitlKind::Confirm) => template("hitl.unclear_confirm"),
            (REASK_WEAK, _) => filled("confirm.weak_yes_reask", [phrase]),
            (REASK_NOT_HEARD, _) => {
                filled("confirm.not_heard_reask", [("params", values.join(", "))])
            }
            (_, HitlKind::Confirm) => filled("confirm.short_reask", [phrase]),
            // Any other question has no action to name: it is put again as it was asked.
            _ => self.question_say(request),
        };
        let ask = self.ask(request, Kind::Question, say);
        if let Some(asking) = &mut self.asking {
            asking.items.push(ask.item);
            asking.ask = Some(ask.item);
            if which == REASK_NOT_HEARD {
                // This re-ask reads the values back: hearing it is hearing the read-back.
                asking.readback = Some(ask.item);
                asking.readback_heard_at = None;
            }
            asking.reask_at = None;
            if !asking.request.firm {
                asking.deadline = now + Ms::from(self.cfg.hitl_answer_timeout_ms);
            }
        }
        out.push(PolicyAction::Speak(ask));
    }

    fn time_out(&mut self, out: &mut Vec<PolicyAction>) {
        if let Some(asking) = self.asking.take() {
            self.close(&asking, out);
            out.push(PolicyAction::Task(TaskCmd::Answer {
                request: asking.request.id,
                outcome: HitlOutcome::Timeout,
                by: AnsweredBy::Timeout,
            }));
        }
    }

    /// The question is over: what is left of it is taken back, and its id is remembered.
    fn close(&mut self, asking: &Asking, out: &mut Vec<PolicyAction>) {
        let items = asking.items.iter();
        out.extend(items.map(|item| PolicyAction::Drop { item: *item }));
        self.closed.push_back(asking.request.id.clone());
        if self.closed.len() > CLOSED_KEPT {
            self.closed.pop_front();
        }
        // The answer it broke into goes on from the first sentence the caller did not hear,
        // introduced as the persona does it ("Anyway,").
        if asking
            .interrupted
            .is_some_and(|item| self.queued.contains(&item))
        {
            let resume = template("question.resume");
            let resume = Utterance {
                lane: Lane::Main,
                ..self.utterance(Kind::Notice, RANK_QUESTION, resume)
            };
            out.push(PolicyAction::Speak(resume));
        }
    }

    /// A key the caller pressed. While a confirmation is open, once its question began to
    /// play, `1` accepts and `2` declines; while a choice is, key `n` picks its `n`-th option.
    /// A value typed on the keypad ends with `#` (`*` starts it again), or with a pause of
    /// `policy.hitl_key_gap_ms` after a key. Any other key, and a key with no question open,
    /// does nothing.
    fn on_key(&mut self, now: Ms, digit: char, out: &mut Vec<PolicyAction>) {
        if !self.keypad {
            return;
        }
        let Some(asking) = &mut self.asking else {
            return;
        };
        let heard = asking.asked_at.is_some();
        let outcome = match (&mut asking.keyed, asking.request.kind, digit) {
            (Some(keys), _, '0'..='9') => {
                keys.push(digit);
                asking.key_at = Some(now);
                return;
            }
            (Some(keys), _, '*') => {
                keys.clear();
                asking.key_at = None;
                return;
            }
            (Some(keys), _, '#') if !keys.is_empty() => HitlOutcome::Accept {
                value: Some(serde_json::Value::String(std::mem::take(keys))),
            },
            (None, HitlKind::Confirm, '1') if heard => HitlOutcome::Accept { value: None },
            (None, HitlKind::Confirm, '2') if heard => HitlOutcome::Decline { correction: None },
            (None, HitlKind::Choose, '1'..='9') if heard => {
                let at = digit.to_digit(10).map_or(0, |n| n as usize);
                match asking.request.options.get(at.wrapping_sub(1)) {
                    Some(option) => HitlOutcome::Accept {
                        value: Some(serde_json::Value::String(option.clone())),
                    },
                    None => return,
                }
            }
            _ => return,
        };
        self.keyed_answer(outcome, out);
    }

    /// The keys pressed answer the open question.
    fn keyed_answer(&mut self, outcome: HitlOutcome, out: &mut Vec<PolicyAction>) {
        let Some(asking) = &mut self.asking else {
            return;
        };
        asking.key_at = None;
        let line = match outcome {
            HitlOutcome::Decline { .. } => "hitl.decline",
            _ => "hitl.accept",
        };
        let task = Some((asking.request.task.clone(), asking.request.revision));
        out.push(PolicyAction::Task(TaskCmd::Answer {
            request: asking.request.id.clone(),
            outcome,
            by: AnsweredBy::Dtmf,
        }));
        let reply = self.notice(task, template(line));
        out.push(PolicyAction::Speak(reply));
    }

    /// One utterance of the open question; of an urgent one, at the urgent rank.
    fn ask(&mut self, request: &HitlRequest, kind: Kind, say: Say) -> Utterance {
        let urgent = self.asking.as_ref().is_some_and(|asking| asking.urgent);
        let rank = if urgent { RANK_URGENT } else { RANK_QUESTION };
        Utterance {
            task: Some((request.task.clone(), request.revision)),
            question: Some(request.id.clone()),
            ..self.utterance(kind, rank, say)
        }
    }

    /// The words of a `Choose` or `Value` question.
    fn question_say(&self, request: &HitlRequest) -> Say {
        let question = ("question", request.question.clone().unwrap_or_default());
        match request.kind {
            HitlKind::Choose => {
                let options = match request.options.split_last() {
                    Some((last, rest)) if !rest.is_empty() => {
                        format!("{}, or {last}", rest.join(", "))
                    }
                    _ => request.options.join(""),
                };
                filled("choose", [question, ("options", options)])
            }
            _ => filled("value", [question]),
        }
    }

    /// Offers the question: a confirmation is a lead-in, the read-back as its own utterance
    /// that must be heard, and the question.
    fn offer_question(&mut self, now: Ms, out: &mut Vec<PolicyAction>) {
        let Some(request) = self.asking.as_ref().map(|asking| asking.request.clone()) else {
            return;
        };
        let mut items = Vec::new();
        let (mut readback, mut ask) = (None, None);
        let screen = self
            .asking
            .as_ref()
            .and_then(|asking| asking.screen.clone());
        // An urgent question that breaks in says so first, in one short line, when its
        // deadline leaves time for it (`LEAD_IN_LEFT_MS`).
        let breaks_in = !self.quiet || self.user_speaking;
        let lead_in_fits = |asking: &Asking| asking.deadline >= now + LEAD_IN_LEFT_MS;
        let urgent = |asking: &Asking| asking.urgent && lead_in_fits(asking);
        if self.asking.as_ref().is_some_and(urgent) && breaks_in {
            let lead_in = self.ask(&request, Kind::Notice, template("question.interrupt"));
            items.push(lead_in.item);
            out.push(PolicyAction::Speak(lead_in));
        }
        if let Some(say) = screen {
            // Answered on the screen: one line says where.
            let part = self.ask(&request, Kind::Question, say);
            items.push(part.item);
            ask = Some(part.item);
            out.push(PolicyAction::Speak(part));
        } else if request.kind == HitlKind::Confirm {
            let [lead_in, read, question] = self.persona.render_confirm(&request);
            let parts = [
                (lead_in, false, false),
                (read, true, false),
                (question, false, true),
            ];
            for (words, is_readback, is_question) in parts {
                if words.is_empty() {
                    continue;
                }
                let kind = if is_question {
                    Kind::Question
                } else {
                    Kind::Confirm
                };
                let part = self.ask(&request, kind, Say::Verbatim(words));
                items.push(part.item);
                if is_readback {
                    readback = Some(part.item);
                }
                if is_question {
                    ask = Some(part.item);
                }
                out.push(PolicyAction::Speak(part));
            }
            if self.keypad {
                let hint = self.ask(&request, Kind::Confirm, template("confirm.dtmf"));
                items.push(hint.item);
                out.push(PolicyAction::Speak(hint));
            }
        } else {
            // The question names the options: it is what must be heard before an answer binds.
            let say = self.question_say(&request);
            let part = self.ask(&request, Kind::Question, say);
            items.push(part.item);
            ask = Some(part.item);
            readback = Some(part.item);
            out.push(PolicyAction::Speak(part));
            // On the keypad an option is picked by its number (at most nine).
            let numbered = (1..=9).zip(&request.options);
            let keys: Vec<String> = numbered
                .map(|(n, option)| format!("{n} for {option}"))
                .collect();
            if self.keypad && request.kind == HitlKind::Choose && !keys.is_empty() {
                let keys = match keys.split_last() {
                    Some((last, rest)) if !rest.is_empty() => {
                        format!("{}, or {last}", rest.join(", "))
                    }
                    _ => keys.join(""),
                };
                let hint = filled("hitl.choose_dtmf", [("options", keys)]);
                let hint = self.ask(&request, Kind::Confirm, hint);
                items.push(hint.item);
                out.push(PolicyAction::Speak(hint));
            }
        }
        if let Some(asking) = &mut self.asking {
            asking.items = items;
            asking.readback = readback;
            asking.ask = ask;
            asking.readback_heard_at = None;
            asking.asked_at = None;
            asking.reask_at = None;
            asking.reoffer = false;
            // A deadline of the job's own is never moved.
            if !asking.request.firm {
                asking.deadline = asking
                    .deadline
                    .max(now + Ms::from(self.cfg.hitl_answer_timeout_ms));
            }
        }
    }

    /// Opens a task's question: asked by voice, on the screen or the keypad, or declined
    /// when it cannot be asked. `urgent`: its deadline cannot wait for a slot.
    fn open(&mut self, now: Ms, request: HitlRequest, urgent: bool, out: &mut Vec<PolicyAction>) {
        let task = request.task.clone();
        let of_task = Some((task.clone(), request.revision));
        // The question is the reply: no acknowledgement on top of it.
        if let Some(turn) = &mut self.turn {
            turn.ack_due = None;
        }
        // A value that is never taken by voice is not asked for: the caller hears why,
        // and the background is told no.
        let categories = self.persona.sensitive_categories();
        let named = request
            .question
            .as_deref()
            .filter(|_| request.kind != HitlKind::Confirm)
            .and_then(|question| rules::sensitive_category(question, categories));
        let label = match named {
            Some(category) => format!("your {category}"),
            None if request.kind == HitlKind::Form => "the details".to_owned(),
            None => "that".to_owned(),
        };
        let on_screen = request.kind.on_screen() || request.sensitive || named.is_some();
        // On a phone line a value is typed on the keypad; a form or a link needs a
        // screen.
        let keyed = self.keypad && on_screen && request.kind == HitlKind::Value;
        if on_screen && !self.client && !keyed {
            // No screen to ask on, and such a value is never taken by voice.
            let label = ("field_label", label);
            let decline = self.notice(of_task, filled("sensitive.decline", [label]));
            self.declined = Some(task);
            out.push(PolicyAction::Speak(decline));
            out.push(PolicyAction::Task(TaskCmd::Answer {
                request: request.id,
                outcome: HitlOutcome::Decline { correction: None },
                by: AnsweredBy::Policy,
            }));
            return;
        }
        let screen = on_screen.then(|| match request.kind {
            HitlKind::AuthUrl => filled("auth_url.before", [("channel", SCREEN.to_owned())]),
            _ if keyed => filled("sensitive.dtmf", [("field_label", label)]),
            _ => filled("sensitive.screen", [("field_label", label)]),
        });
        let capture = on_screen
            && (request.sensitive || named.is_some() || request.kind == HitlKind::AuthUrl);
        // The relayed answer it breaks into, which resumes after it.
        let interrupted = self.queued.first().copied().filter(|_| urgent);
        self.asking = Some(Asking {
            urgent,
            interrupted,
            deadline: request.expires_at,
            capture,
            spoke_over: false,
            request,
            items: Vec::new(),
            readback: None,
            ask: None,
            readback_heard_at: None,
            asked_at: None,
            reask_at: None,
            reasked: [false; 4],
            reoffer: false,
            screen,
            keyed: keyed.then(String::new),
            key_at: None,
        });
        self.offer_question(now, out);
    }

    /// The child's question that waits for a slot: asked at one, or at once when its deadline
    /// cannot wait for one (it then breaks in), or given up when its deadline passed.
    fn ask_deferred(&mut self, now: Ms, board: &Board, out: &mut Vec<PolicyAction>) {
        let Some(request) = &self.deferred else {
            return;
        };
        if request.expires_at <= now {
            let id = request.id.clone();
            self.deferred = None;
            out.push(PolicyAction::Task(TaskCmd::Answer {
                request: id,
                outcome: HitlOutcome::Timeout,
                by: AnsweredBy::Timeout,
            }));
            return;
        }
        let slot = self.slot(now, board).is_some();
        let urgent = request.expires_at <= now + self.left_ms + ASK_MS;
        if (slot || urgent)
            && self.asking.is_none()
            && let Some(request) = self.deferred.take()
        {
            self.open(now, request, !slot, out);
        }
    }

    // ---- lanes

    fn on_lane(&mut self, now: Ms, event: LaneEvent, out: &mut Vec<PolicyAction>) {
        self.update_lane(&event);
        match event {
            LaneEvent::Started { item, at, .. } => {
                self.last_speech = at;
                self.agent_at = at;
                if let Some(turn) = &mut self.turn {
                    turn.audible_by = None;
                }
                if let Some(relay) = self.relays.iter_mut().find(|relay| relay.item == item) {
                    relay.part_heard = true;
                }
                if let Some(asking) = &mut self.asking
                    && asking.ask == Some(item)
                {
                    asking.asked_at.get_or_insert(at);
                }
            }
            LaneEvent::Heard { item, heard_at } => {
                // A line heard to its end ends a run of synthesis failures (one that only
                // started may still fail: live take 22, each line played and then failed).
                self.tts_failed = false;
                self.last_speech = now;
                self.agent_at = now;
                if let Some(repeat) = self.repeat.as_mut().filter(|repeat| repeat.item == item) {
                    repeat.heard_at = Some(heard_at);
                    repeat.deadline = Some(heard_at + Ms::from(self.cfg.hitl_answer_timeout_ms));
                }
                if let Some(index) = self.asked.iter().position(|asked| *asked == item) {
                    self.asked.swap_remove(index);
                    self.answer_window_until = heard_at + Ms::from(self.cfg.answer_window_ms);
                }
                if let Some(asking) = &mut self.asking {
                    if asking.readback == Some(item) {
                        asking.readback_heard_at = Some(heard_at);
                    }
                    if asking.ask == Some(item) {
                        // A screen question is not asked again by voice.
                        let silence = (!asking.reasked[REASK_SILENCE] && asking.screen.is_none())
                            .then_some(heard_at + Ms::from(self.cfg.hitl_reprompt_after_ms));
                        asking.reask_at = silence;
                        if !asking.request.firm {
                            asking.deadline = heard_at + Ms::from(self.cfg.hitl_answer_timeout_ms);
                        }
                    }
                }
            }
            LaneEvent::Dropped {
                item,
                cause: CutCause::Failure,
                ..
            } => {
                // A line the voice could not give: the client is told, once in a run of
                // failures; the caller hears the health watcher's line for the voice (chapter
                // 10: a stall line when the next line does not succeed either), never "Sorry,
                // I couldn't get that done just now.", which says a task failed. Live (launch
                // demo take 22, 28 Oct): with the voice's completions lost after a refused
                // flush, every line played and then failed on the floor's watchdog; each
                // failure said that notice, the notice's own start ended the run of failures,
                // and the notice failed in turn: it was said every 13 s for six minutes, and a
                // confirmation's question was never heard.
                if !std::mem::replace(&mut self.tts_failed, true) {
                    out.push(error("interaction_tts_failed", "speech synthesis failed"));
                }
                self.unheard(now, item);
            }
            LaneEvent::Dropped { item, .. } | LaneEvent::Returned { item } => {
                self.unheard(now, item);
            }
            LaneEvent::Idle { .. } => {}
        }
    }

    /// `item` never reached the caller in full: a question it asked is asked again, and an
    /// open repeat question waits for its answer from now on.
    fn unheard(&mut self, now: Ms, item: ItemId) {
        self.asked.retain(|asked| *asked != item);
        if let Some(repeat) = self.repeat.as_mut().filter(|repeat| repeat.item == item) {
            // Cut by the caller, whose words are then the answer; else it waits as long.
            repeat.deadline = Some(now + Ms::from(self.cfg.hitl_answer_timeout_ms));
        }
        if let Some(asking) = &mut self.asking
            && asking.items.contains(&item)
        {
            asking.reoffer = true;
        }
    }

    /// An update or its offer was heard, or never reached the caller: then it waits again.
    fn update_lane(&mut self, event: &LaneEvent) {
        let (item, heard_at) = match event {
            LaneEvent::Heard { item, heard_at } => (*item, Some(*heard_at)),
            LaneEvent::Dropped { item, .. } | LaneEvent::Returned { item } => (*item, None),
            LaneEvent::Started { .. } | LaneEvent::Idle { .. } => return,
        };
        let window = Ms::from(self.cfg.answer_window_ms);
        if let Some(offer) = self
            .update_offer
            .as_mut()
            .filter(|offer| offer.item == item)
        {
            match heard_at {
                Some(at) => offer.until = Some(at + window),
                None => {
                    let tasks = std::mem::take(&mut offer.tasks);
                    self.update_offer = None;
                    let offered = self.inbox.iter_mut().filter(|u| tasks.contains(&u.task));
                    offered.for_each(|update| update.offered = false);
                }
            }
        }
        if let Some(undo) = self.undo.as_mut().filter(|undo| undo.item == item) {
            match heard_at {
                Some(at) => undo.until = Some(at + window),
                None => self.undo = None,
            }
        }
        if let Some(at) = self.saying.iter().position(|(said, _)| *said == item) {
            let (_, update) = self.saying.swap_remove(at);
            if let Some(at) = heard_at.filter(|_| update.failure.is_none()) {
                // What a turn that points back ("when does it land?") is about.
                self.last_update = Some((update.task.clone(), at));
            }
            if heard_at.is_none() {
                self.inbox.push(Update {
                    offered: true,
                    ..update
                });
            }
        }
    }

    // ---- tasks

    fn on_task(&mut self, now: Ms, event: TaskEvent, board: &Board, out: &mut Vec<PolicyAction>) {
        let TaskEvent {
            task,
            revision,
            kind,
            state,
            lineage,
            ..
        } = event;
        let of_task = Some((task.clone(), revision));
        // The caller's latest request owes its answer until that answer is over.
        let owed = self.owed.as_mut();
        match (&kind, owed) {
            (TaskEventKind::Created { .. }, Some(owed))
                if owed.task.is_none() && lineage.parent_id.is_none() =>
            {
                owed.task = Some(task.clone());
            }
            (
                TaskEventKind::AnswerDone
                | TaskEventKind::Completed { .. }
                | TaskEventKind::Failed { .. }
                | TaskEventKind::Cancelled { .. }
                | TaskEventKind::Rejected { .. },
                Some(owed),
            ) if owed.task.as_ref() == Some(&task) => self.owed = None,
            _ => {}
        }
        let child = lineage.parent_id.clone();
        if let TaskEventKind::Completed { text, .. } = &kind {
            self.results.push_back((task.clone(), text.clone()));
            if self.results.len() > RESULTS_KEPT {
                self.results.pop_front();
            }
        }
        match kind {
            // An orchestrator's child: its result or failure is an update, said at a slot.
            TaskEventKind::Completed { text, .. } if child.is_some() => {
                let title = self.title_of(&task, board);
                self.inbox.push(Update {
                    task,
                    revision,
                    parent: child,
                    title,
                    failure: None,
                    summary: text,
                    at: now,
                    offered: false,
                    tries: 0,
                    refused: None,
                });
            }
            TaskEventKind::Progress { text } => {
                // Kept, and said by the timers once the wait is old enough. A step without a
                // phrase replaces it: the earlier words no longer say what is being done.
                if let Some(wait) = &mut self.wait {
                    wait.progress = text.map(|text| (text, of_task));
                }
            }
            TaskEventKind::AnswerSentence {
                attempt,
                seq,
                text,
                first_after_e_ms,
            } => {
                if let Some(ms) = first_after_e_ms {
                    self.first_sentence_ms.push_back(ms);
                    if self.first_sentence_ms.len() > FAST_WINDOW {
                        self.first_sentence_ms.pop_front();
                    }
                    out.push(PolicyAction::FirstSentenceAfterE { ms });
                }
                // An answer is ready, and its first audio is one synthesis away. When that is
                // before something must be audible, the acknowledgement is not needed;
                // otherwise it is said now, while the sentence is made, not at the deadline.
                if let Some(turn) = &mut self.turn
                    && turn.ack_due.is_some()
                {
                    let in_time = turn.audible_by.is_none_or(|by| now + self.synth_ms <= by);
                    turn.ack_due = (!in_time).then_some(now);
                }
                if let Some(wait) = &mut self.wait {
                    wait.progress = None;
                }
                // A sentence that asks for a value never taken by voice is not relayed: the
                // caller hears, once for the task, that it cannot be taken.
                let categories = self.persona.sensitive_categories();
                let declined = self.declined.as_ref() == Some(&task);
                if let Some(category) = guard::asks_for_secret(&text, categories) {
                    let label = ("field_label", format!("your {category}"));
                    if !declined {
                        let decline = self.notice(of_task, filled("sensitive.decline", [label]));
                        self.declined = Some(task);
                        out.push(PolicyAction::Speak(decline));
                    }
                    return;
                }
                if declined && guard::asks(&text) {
                    // "Please share it with me now": the same request without its name.
                    return;
                }
                let item = self.relay(&task).item;
                let sentence = Utterance {
                    item,
                    sentence: Some((attempt, seq)),
                    last: false,
                    ..self.relayed(of_task, text)
                };
                out.push(PolicyAction::Speak(sentence));
            }
            TaskEventKind::AnswerStopped { reason, .. } => {
                if self.stop_relay(&task, out) {
                    let say = match reason {
                        StopReason::Revised => bank("withdrawal"),
                        StopReason::Cancelling => template("cancel.intent"),
                        StopReason::MarkerThenToolCall => bank("stalled"),
                        // The `Failed` event that follows says what became of it.
                        StopReason::RunFailed => return,
                    };
                    out.push(PolicyAction::Speak(self.notice(of_task, say)));
                }
            }
            // A child's question waits for a slot of the conversation, unless its deadline
            // cannot wait for one (chapter 8).
            TaskEventKind::InputRequired(request) if child.is_some() => {
                self.deferred = Some(request);
                self.ask_deferred(now, board, out);
            }
            TaskEventKind::InputRequired(request) => self.open(now, request, false, out),
            TaskEventKind::InputResolved {
                request, outcome, ..
            } => {
                self.deferred.take_if(|deferred| deferred.id == request);
                if let Some(asking) = self.asking.take_if(|asking| asking.request.id == request) {
                    self.close(&asking, out);
                }
                match (outcome == HitlOutcome::Timeout, child) {
                    (false, _) => {}
                    // A child's question nobody answered in time: one line at the next slot.
                    (true, Some(parent)) => {
                        let title = (
                            "task_title",
                            rules::spoken_title(&self.title_of(&task, board)),
                        );
                        self.inbox.push(Update {
                            task,
                            revision,
                            parent: Some(parent),
                            title: String::new(),
                            failure: Some(filled("question.expired", [title])),
                            summary: String::new(),
                            at: now,
                            offered: false,
                            tries: 0,
                            refused: None,
                        });
                    }
                    (true, None) => out.push(PolicyAction::Speak(
                        self.notice(of_task, template("hitl.timeout")),
                    )),
                }
            }
            TaskEventKind::Revised => {
                // The revised run is a new wait.
                if let Some(wait) = &mut self.wait {
                    *wait = Wait::new(now);
                    self.last_speech = now;
                }
            }
            // The answer is over for now: an orchestrator waits for its children.
            TaskEventKind::AnswerDone => {
                let relayed = self.relays.iter().position(|relay| relay.task == task);
                if let Some(relay) = relayed.map(|index| self.relays.swap_remove(index)) {
                    let end = self.relayed(of_task, String::new());
                    out.push(PolicyAction::Speak(Utterance {
                        item: relay.item,
                        ..end
                    }));
                }
            }
            TaskEventKind::Completed { .. } => {
                self.finished.push_back((task.clone(), now));
                if self.finished.len() > CLOSED_KEPT {
                    self.finished.pop_front();
                }
                let relayed = self.relays.iter().position(|relay| relay.task == task);
                let item = relayed.map(|index| self.relays.swap_remove(index).item);
                // The answer was relayed sentence by sentence: only its relay closes.
                let end = self.relayed(of_task, String::new());
                out.push(PolicyAction::Speak(Utterance {
                    item: item.unwrap_or(end.item),
                    ..end
                }));
            }
            TaskEventKind::Failed { error, effects } => {
                self.relays.retain(|relay| relay.task != task);
                // Only a failure with nothing committed may say it could not be done: what
                // went through before the failure did get done, and the caller who hears
                // otherwise asks again and gets it twice.
                let say = match effects {
                    // A Bud agent's run was held for a person's approval: trying again
                    // would not help, so the caller is told what it waits for.
                    Effects::NoneCommitted if error.starts_with(APPROVAL_REQUIRED) => {
                        template("approval_needed")
                    }
                    Effects::NoneCommitted => template("failure.retryable"),
                    Effects::Committed(labels) => filled(
                        "failure.with_effects",
                        [("effect_labels", labels.join(" and "))],
                    ),
                    Effects::Unknown => filled(
                        "failure.effects_unknown",
                        [("verb_phrase", "that".to_owned())],
                    ),
                };
                if child.is_some() {
                    let title = self.title_of(&task, board);
                    self.inbox.push(Update {
                        task,
                        revision,
                        parent: child,
                        title,
                        failure: Some(say),
                        summary: error,
                        at: now,
                        offered: false,
                        tries: 0,
                        refused: None,
                    });
                    return;
                }
                out.push(PolicyAction::Speak(self.notice(of_task, say)));
            }
            TaskEventKind::StaleOutcome { label } => {
                // A call the caller's change came too late for. A cancel that waits for it
                // says so itself when it is confirmed.
                let cancelling = board
                    .tasks
                    .iter()
                    .any(|view| view.id == task && view.cancelling);
                if !cancelling {
                    let say = filled("stale_outcome", [("effect_labels", label)]);
                    let notice = self.notice(of_task, say);
                    // The revised run may already be asking for its own call. The caller
                    // must know what went through before answering that: what is left of
                    // the question is taken back, this line is said in its place (as part
                    // of the question, or the floor would hold it until the answer), and
                    // the question is asked again.
                    let open = self.asking.as_ref();
                    match open.map(|asking| (asking.request.id.clone(), asking.items.clone())) {
                        Some((question, items)) => {
                            out.extend(items.into_iter().map(|item| PolicyAction::Drop { item }));
                            out.push(PolicyAction::Speak(Utterance {
                                rank: RANK_QUESTION,
                                question: Some(question),
                                ..notice
                            }));
                            self.offer_question(now, out);
                        }
                        None => out.push(PolicyAction::Speak(notice)),
                    }
                }
            }
            TaskEventKind::CancelUnconfirmed { .. } => {
                let say = template("cancel.unconfirmed_no_confirmation_in_time");
                out.push(PolicyAction::Speak(self.notice(of_task, say)));
            }
            // A child stopped with its orchestrator's request: that request's line says it.
            TaskEventKind::Cancelled {
                by: "parent",
                effects: Effects::NoneCommitted,
            } => {}
            TaskEventKind::Cancelled { effects, .. } => {
                self.relays.retain(|relay| relay.task != task);
                let say = match effects {
                    Effects::NoneCommitted => template("cancel.confirmed_none_committed"),
                    Effects::Committed(labels) => filled(
                        "cancel.confirmed_committed",
                        [("effect_labels", labels.join(" and "))],
                    ),
                    Effects::Unknown => filled(
                        "cancel.confirmed_effects_unknown",
                        [("action_label", "gone through".to_owned())],
                    ),
                };
                out.push(PolicyAction::Speak(self.notice(of_task, say)));
            }
            TaskEventKind::Rejected { .. } => {
                out.push(PolicyAction::Speak(self.notice(of_task, bank("refuse"))));
            }
            // A caller's request that waits for a free place: they hear that it is queued, in
            // the acknowledgement's place. An orchestrator says it of its own children.
            TaskEventKind::Created { .. }
                if state == TaskState::Submitted && lineage.parent_id.is_none() =>
            {
                let ahead: Vec<&TaskView> = board
                    .tasks
                    .iter()
                    .filter(|view| view.lineage.parent_id.is_none() && view.id != task)
                    .filter(|view| view.state.is_live())
                    .collect();
                let say = match ahead.as_slice() {
                    [one] => filled(
                        "status.queued_on_create",
                        [("task_title", rules::spoken_title(&one.title))],
                    ),
                    _ => filled("status.queued_behind", [("count", ahead.len().to_string())]),
                };
                let turn = self.turn.as_mut().map(|turn| {
                    turn.ack_due = None;
                    turn.id
                });
                let line = match turn {
                    Some(turn) => self.reply(turn, Kind::Progress, say),
                    None => self.notice(of_task, say),
                };
                out.push(PolicyAction::Speak(line));
            }
            TaskEventKind::Created { .. }
            | TaskEventKind::Working
            | TaskEventKind::EffectsUpdated(_)
            | TaskEventKind::CancelRequested => {}
        }
    }

    // ---- task updates (chapter 8)

    fn title_of(&self, task: &TaskId, board: &Board) -> String {
        let view = board.tasks.iter().find(|view| view.id == *task);
        view.map(|view| view.title.clone()).unwrap_or_default()
    }

    /// How an update reaches the caller: their own word for its task or its request, else
    /// `tasks.announce`.
    fn announce_of(&self, update: &Update) -> Announce {
        let told = |task: &TaskId| self.told.iter().find(|(told, _)| told == task);
        let own = told(&update.task).or_else(|| update.parent.as_ref().and_then(told));
        own.map_or(self.announce, |(_, announce)| *announce)
    }

    /// When the conversation falls idle: [`IDLE_MS`] after the caller or the agent last spoke.
    fn idle_at(&self) -> Ms {
        self.caller_at.max(self.agent_at) + IDLE_MS
    }

    /// A slot: the agent has nothing to play and owes no answer, nobody is asked anything and
    /// the caller does not hold the floor. `Some(true)` when the conversation is idle too:
    /// neither the caller nor the agent has spoken for [`IDLE_MS`]. At the end of an answer
    /// it is going on.
    fn slot(&self, now: Ms, board: &Board) -> Option<bool> {
        self.no_slot(now, board)
            .is_none()
            .then_some(now >= self.idle_at())
    }

    /// What holds an update back now; `None` at a slot.
    fn no_slot(&self, now: Ms, board: &Board) -> Option<&'static str> {
        // The question waiting for this slot does not hold it.
        let deferred = self.deferred.as_ref().map(|deferred| &deferred.id);
        let not_deferred = |open: &HitlRequest| Some(&open.id) != deferred;
        let owed = self
            .owed
            .as_ref()
            .is_some_and(|owed| owed.task.is_some() || now < owed.at + OWED_BACKSTOP_MS);
        [
            (!self.quiet, "playing"),
            (self.user_speaking, "caller_speaking"),
            (self.turn.as_ref().is_some_and(|turn| !turn.settled), "turn"),
            (owed, "answer_owed"),
            (!self.relays.is_empty(), "answer_relayed"),
            (
                self.asking.is_some() || board.open_question.as_ref().is_some_and(not_deferred),
                "question",
            ),
            (self.repeat.is_some() || self.offer.is_some(), "asked"),
            (
                self.update_offer.is_some() || self.undo.is_some(),
                "offer_open",
            ),
        ]
        .into_iter()
        .find_map(|(held, why)| held.then_some(why))
    }

    /// At a slot, the inbox's updates: said when the conversation is idle or the caller asked
    /// to be told, one offer for the rest when it is going on (`ask`), nothing for `screen`.
    /// A failure is said like a told update.
    fn speak_updates(&mut self, now: Ms, board: &Board, out: &mut Vec<PolicyAction>) {
        // An offer nobody answered: its updates wait for an idle moment. A caller who began
        // to answer in time is heard out.
        let speaking = self.user_speaking;
        let unanswered =
            |offer: &mut UpdateOffer| !speaking && offer.until.is_some_and(|until| until <= now);
        self.update_offer.take_if(unanswered);
        if self.inbox.is_empty() {
            return;
        }
        let held = self.no_slot(now, board);
        if held != std::mem::replace(&mut self.held, held) {
            tracing::debug!(held, waiting = self.inbox.len(), "interaction update slot");
        }
        let Some(idle) = self.slot(now, board) else {
            return;
        };
        let mut inbox = std::mem::take(&mut self.inbox);
        inbox.sort_by_key(|update| update.order(self.told.iter().any(|(t, _)| *t == update.task)));
        let (mut say, mut offer, mut keep) = (Vec::new(), Vec::new(), Vec::new());
        for update in inbox {
            match self.announce_of(&update) {
                Announce::Screen => keep.push(update),
                Announce::Tell => say.push(update),
                _ if idle || update.failure.is_some() => say.push(update),
                Announce::Ask if !update.offered => offer.push(update),
                Announce::Ask => keep.push(update),
            }
        }
        if !say.is_empty() {
            // What is said now goes first; an offer of the rest waits for the next slot.
            keep.extend(offer);
            self.inbox = keep;
            for update in say {
                self.say_update(update, None, out);
            }
            return;
        }
        if let [first, ..] = offer.as_slice() {
            let titles: Vec<String> = offer
                .iter()
                .map(|u| rules::spoken_title(&u.title))
                .collect();
            let say = match titles.as_slice() {
                [title] => filled("offer.result", [("task_title", title.clone())]),
                _ => filled(
                    "update.offer_many",
                    [
                        ("count", titles.len().to_string()),
                        ("task_titles", titles.join(", ")),
                    ],
                ),
            };
            let task = (offer.len() == 1).then(|| (first.task.clone(), first.revision));
            let asked = self.persona.resolve(&say, 0).unwrap_or_default();
            let line = Utterance {
                task,
                ..self.utterance(Kind::Update, RANK_UPDATE, say)
            };
            self.asked.push(line.item);
            self.update_offer = Some(UpdateOffer {
                item: line.item,
                tasks: offer.iter().map(|update| update.task.clone()).collect(),
                asked,
                until: None,
            });
            out.push(PolicyAction::Speak(line));
            keep.extend(offer.into_iter().map(|update| Update {
                offered: true,
                ..update
            }));
        }
        self.inbox = keep;
    }

    /// The caller said yes to the offer: its updates are said, as the reply to the turn.
    fn take_offer(
        &mut self,
        eot: &EndOfTurn,
        offer: &UpdateOffer,
        board: &Board,
        out: &mut Vec<PolicyAction>,
    ) {
        if let Some(turn) = &mut self.turn {
            turn.settled = true;
        }
        out.push(PolicyAction::Settled {
            turn: eot.turn,
            act: Act::Social,
            source: ActSource::RuleOverride("update_offer"),
        });
        if board.provisional.is_some() {
            out.push(PolicyAction::Task(TaskCmd::DiscardProvisional {
                reason: "update_offer",
            }));
        }
        let (taken, kept): (Vec<Update>, Vec<Update>) = std::mem::take(&mut self.inbox)
            .into_iter()
            .partition(|update| offer.tasks.contains(&update.task));
        self.inbox = kept;
        for update in taken {
            self.say_update(update, Some(eot.turn), out);
        }
    }

    /// One update: a failure in its own words, a result through the narrator.
    fn say_update(
        &mut self,
        update: Update,
        reply_to: Option<TurnId>,
        out: &mut Vec<PolicyAction>,
    ) {
        match update.failure.clone() {
            Some(say) => {
                let line = self.update_line(&update, say, reply_to);
                self.saying.push((line.item, update));
                out.push(PolicyAction::Speak(line));
            }
            None => {
                let item = self.item();
                out.push(PolicyAction::Narrate(Narration {
                    item,
                    title: update.title.clone(),
                    summary: update.summary.clone(),
                    question: None,
                    retry: update.refused,
                }));
                self.narrating.push((item, update, reply_to));
            }
        }
    }

    /// The utterance of an update.
    fn update_line(&mut self, update: &Update, say: Say, reply_to: Option<TurnId>) -> Utterance {
        let rank = if reply_to.is_some() {
            RANK_REPLY
        } else {
            RANK_UPDATE
        };
        Utterance {
            task: Some((update.task.clone(), update.revision)),
            reply_to,
            ..self.utterance(Kind::Update, rank, say)
        }
    }

    /// The narrator's line for an update, checked against the task's result; the template
    /// when there is none or it fails. With a screen the line points at it.
    fn narrated(&mut self, item: ItemId, line: Option<String>, out: &mut Vec<PolicyAction>) {
        // A follow-up question: answered from the result, with the update's checks, or handed
        // to the task's agent.
        if let Some(at) = self.answering.iter().position(|asked| asked.item == item) {
            let asked = self.answering.swap_remove(at);
            let source = format!("{} {} {}", asked.title, asked.summary, asked.text);
            let checked = line.map(|line| guard::fit_narration(&line)).filter(|line| {
                guard::narration(line, &source) == GuardOutcome::Pass
                    && carries_result(line, &asked.summary, &asked.text)
            });
            match checked {
                Some(line) => {
                    if let Some(turn) = self.turn.as_mut().filter(|turn| turn.id == asked.turn) {
                        // The answer is the reply: nothing to acknowledge.
                        turn.ack_due = None;
                        turn.audible_by = None;
                    }
                    let reply = self.reply(asked.turn, Kind::Answer, Say::Front(line));
                    out.push(PolicyAction::Speak(reply));
                }
                None => self.hand_on(asked.turn, asked.at, asked.task, asked.text, out),
            }
            return;
        }
        let Some(at) = self.narrating.iter().position(|(known, ..)| *known == item) else {
            return;
        };
        let (_, update, reply_to) = self.narrating.swap_remove(at);
        let caller = self
            .turn
            .as_ref()
            .map(|turn| turn.text.as_str())
            .unwrap_or_default();
        let checked = spoken_update(
            &self.persona,
            &update.title,
            &update.summary,
            caller,
            line,
            self.screen,
            &self.said,
        );
        let say = match checked {
            Ok(say) => say,
            Err(rule) => {
                // No fallback (chapter 9): with no line the front worded and the check
                // passed, the update waits for the next slot, at most three times, and the
                // narrator is told why its line was not used; the client has the result
                // (`task_result`, `task_output`) whatever is said of it.
                let tries = update.tries + 1;
                tracing::debug!(task = %update.task.0, tries, rule, "interaction update not worded");
                if tries < NARRATION_TRIES {
                    let refused = Some(rule).filter(|rule| *rule != NO_LINE);
                    self.inbox.push(Update {
                        tries,
                        refused,
                        ..update
                    });
                }
                return;
            }
        };
        let line = Utterance {
            item,
            ..self.update_line(&update, say, reply_to)
        };
        self.saying.push((item, update));
        out.push(PolicyAction::Speak(line));
    }

    /// The client showed the task's result: no update of it is said any more.
    fn seen(&mut self, task: &TaskId, out: &mut Vec<PolicyAction>) {
        self.inbox.retain(|update| update.task != *task);
        self.narrating.retain(|(_, update, _)| update.task != *task);
        let (said, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.saying)
            .into_iter()
            .partition(|(_, update)| update.task == *task);
        self.saying = kept;
        out.extend(
            said.into_iter()
                .map(|(item, _)| PolicyAction::Drop { item }),
        );
        let offered = self
            .update_offer
            .as_mut()
            .filter(|offer| offer.until.is_none());
        if let Some(offer) = offered {
            offer.tasks.retain(|offered| offered != task);
            if offer.tasks.is_empty() {
                out.push(PolicyAction::Drop { item: offer.item });
                self.update_offer = None;
            }
        }
    }

    fn relay(&mut self, task: &TaskId) -> &mut Relay {
        let index = match self.relays.iter().position(|relay| relay.task == *task) {
            Some(index) => index,
            None => {
                let item = self.item();
                self.relays.push(Relay {
                    task: task.clone(),
                    item,
                    part_heard: false,
                });
                self.relays.len() - 1
            }
        };
        &mut self.relays[index]
    }

    /// Takes back what is left of a task's answer. `true` when the caller heard part of it,
    /// so a line must say that it no longer holds.
    fn stop_relay(&mut self, task: &TaskId, out: &mut Vec<PolicyAction>) -> bool {
        let item = self.item();
        let Some(relay) = self.relays.iter_mut().find(|relay| relay.task == *task) else {
            return false;
        };
        out.push(PolicyAction::Drop { item: relay.item });
        let tell = std::mem::take(&mut relay.part_heard);
        relay.item = item;
        tell
    }

    // ---- timers

    fn timers(&mut self, now: Ms, board: &Board, out: &mut Vec<PolicyAction>) {
        // The wait lasts while a task runs.
        if !board.tasks.iter().any(|view| view.state.is_working()) {
            self.wait = None;
        } else if self.wait.is_none() {
            self.wait = Some(Wait::new(now));
            self.last_speech = now;
        }

        // Not while the caller speaks again: the turn was not over, and the next one that ends
        // takes this one's place.
        if let Some(turn) = self.turn.as_mut().filter(|_| !self.user_speaking) {
            // The acknowledgement when due; at the hard deadline too if it was skipped and
            // nothing has been audible since.
            let due = turn.ack_due.take_if(|due| *due <= now).is_some();
            let silent = turn.audible_by.take_if(|due| *due <= now).is_some();
            if (due || silent) && !std::mem::replace(&mut turn.acked, true) {
                let polar = rules::detect_polar_question(&turn.text);
                let bank = Say::Bank {
                    group: "ack",
                    intent: polar.then_some(POLAR_QUESTION),
                };
                let say = turn.line.take().map_or(bank, Say::Front);
                let (id, e) = (turn.id, turn.e);
                let expires_at = e + Ms::from(self.cfg.ack_hard_deadline_ms) + UNSPOKEN_EXPIRY_MS;
                let ack = self.reply(id, Kind::Ack, say);
                out.push(PolicyAction::Speak(Utterance {
                    expires_at: Some(expires_at),
                    ..ack
                }));
            }
        }

        let unanswered = |repeat: &mut Repeat| repeat.deadline.is_some_and(|at| at <= now);
        if self.repeat.take_if(unanswered).is_some() {
            // Nothing was started, and the caller is told so.
            let held = self.utterance(Kind::Notice, RANK_RELAY, template("hitl.timeout"));
            out.push(PolicyAction::Speak(held));
        }

        let settled = self.turn.as_ref().is_none_or(|turn| turn.settled);
        let reprompt = Ms::from(self.cfg.hitl_reprompt_after_ms);
        let gap = Ms::from(self.cfg.hitl_key_gap_ms);
        let typed = self.asking.as_mut().and_then(|asking| {
            let paused = asking.key_at.is_some_and(|at| at + gap <= now);
            let keys = asking
                .keyed
                .as_mut()
                .filter(|keys| paused && !keys.is_empty())?;
            Some(std::mem::take(keys))
        });
        if let Some(keys) = typed {
            let value = Some(serde_json::Value::String(keys));
            self.keyed_answer(HitlOutcome::Accept { value }, out);
        }
        if let Some(asking) = &mut self.asking {
            let reask = asking.reask_at.is_some_and(|at| at <= now);
            if asking.deadline <= now {
                self.time_out(out);
            } else if reask && self.user_speaking {
                // The re-ask counts silence.
                asking.reask_at = Some(now + reprompt);
            } else if reask {
                let request = asking.request.clone();
                self.reask(now, REASK_SILENCE, &request, out);
            } else if asking.reoffer && settled && !self.user_speaking {
                self.offer_question(now, out);
            }
        }

        // Progress is said only on a real change: none early in a wait, not too often, a
        // few per wait, and never late. Only while the caller waits on it: nothing else of
        // the agent's plays, and the caller is not speaking.
        let cfg = &self.cfg;
        let due = |wait: &&mut Wait| wait.progress_due(cfg).is_some_and(|due| due <= now);
        let waiting = self.quiet && !self.user_speaking;
        if let Some(wait) = self.wait.as_mut().filter(due).filter(|_| waiting)
            && let Some((text, task)) = wait.progress.take()
        {
            wait.updates += 1;
            wait.last_progress = Some(now);
            let line = Utterance {
                task,
                expires_at: Some(now + UNSPOKEN_EXPIRY_MS),
                ..self.utterance(Kind::Progress, RANK_PROGRESS, Say::Verbatim(text))
            };
            out.push(PolicyAction::Speak(line));
        }

        if let Some((due, group)) = self.wait_line_due()
            && due <= now
        {
            let (kind, say) = match group {
                "holding" => (Kind::Hold, bank(group)),
                "choice_offer" => (Kind::ChoiceOffer, template("choice_offer")),
                _ => (Kind::Checkin, bank(group)),
            };
            let line = Utterance {
                expires_at: Some(now + UNSPOKEN_EXPIRY_MS),
                ..self.utterance(kind, RANK_HOLD, say)
            };
            if kind == Kind::ChoiceOffer {
                // Hearing it opens the answer window; its answer counts for a while after.
                self.asked.push(line.item);
                self.offer = Some(now + OFFER_TIMEOUT_MS);
            }
            out.push(PolicyAction::Speak(line));
            self.last_speech = now;
            if let Some(wait) = &mut self.wait {
                if kind == Kind::ChoiceOffer {
                    wait.offered = true;
                } else if !std::mem::replace(&mut wait.held, true) {
                    wait.updates += 1;
                }
            }
        }
        // A question waiting for a slot comes before any update.
        self.ask_deferred(now, board, out);
        self.speak_updates(now, board, out);
    }

    /// The holding line after `hold_after_silence_ms` without agent speech in a wait, as long
    /// again later the choice offer (keep waiting, or stop), then a check-in every
    /// `quiet_checkin_ms`, until the wait is `agreed_wait_max_ms` old. Nothing while the caller
    /// speaks or owes an answer.
    fn wait_line_due(&self) -> Option<(Ms, &'static str)> {
        let wait = self.wait.as_ref()?;
        if self.user_speaking || self.asking.is_some() {
            return None;
        }
        let (after, group) = match (wait.held, wait.offered) {
            (false, _) => (self.cfg.hold_after_silence_ms, "holding"),
            (true, false) => (self.cfg.hold_after_silence_ms, "choice_offer"),
            (true, true) => (self.cfg.quiet_checkin_ms, "check_in"),
        };
        let due = self.last_speech + Ms::from(after);
        (due <= wait.started + Ms::from(self.cfg.agreed_wait_max_ms)).then_some((due, group))
    }

    // ---- utterances

    fn item(&mut self) -> ItemId {
        let item = ItemId(self.next_item);
        self.next_item += 1;
        item
    }

    fn utterance(&mut self, kind: Kind, rank: u8, say: Say) -> Utterance {
        Utterance {
            item: self.item(),
            lane: Lane::Front,
            kind,
            rank,
            say,
            discard: false,
            task: None,
            reply_to: None,
            question: None,
            expires_at: None,
            sentence: None,
            last: true,
        }
    }

    /// A reply to the latest turn.
    fn reply(&mut self, turn: TurnId, kind: Kind, say: Say) -> Utterance {
        Utterance {
            reply_to: Some(turn),
            ..self.utterance(kind, RANK_REPLY, say)
        }
    }

    /// What became of a task, said in the relay's place.
    fn notice(&mut self, task: Option<(TaskId, u32)>, say: Say) -> Utterance {
        Utterance {
            task,
            ..self.utterance(Kind::Notice, RANK_RELAY, say)
        }
    }

    /// Words of a task's answer, never altered. With no words and `last` the utterance only
    /// closes the relay.
    fn relayed(&mut self, task: Option<(TaskId, u32)>, words: String) -> Utterance {
        Utterance {
            lane: Lane::Main,
            task,
            ..self.utterance(Kind::Relay, RANK_RELAY, Say::Verbatim(words))
        }
    }
}

/// The rule [`spoken_update`] names when the narrator gave no line at all.
const NO_LINE: &str = "no_line";

/// What is said of a task's result: the narrator's `line`, cut to the narration's length at a
/// clause boundary when it is longer (`guard::fit_narration`), when it passes the check
/// against the title, the result and the caller's latest words, says something of its own
/// result ([`carries_result`]) and does not say again one of the agent's recent lines
/// (`said`, the floor's heard timeline); otherwise the rule it broke (there is no template to
/// fall back to, chapter 9). With a screen the line points at it.
///
/// Live (launch demo take 15, 27 Oct): the narrator is shown the call's last two lines, and
/// for the browser task's update the 2B model copied the line just said of the writer task.
/// Only the fact check applied, the copy passed it, and the writer's result was said a second
/// time, as the browser task's update.
pub fn spoken_update(
    persona: &Persona,
    title: &str,
    summary: &str,
    caller: &str,
    line: Option<String>,
    screen: bool,
    said: &[(Ms, String)],
) -> Result<Say, &'static str> {
    let line = guard::fit_narration(&line.ok_or(NO_LINE)?);
    let source = format!("{title} {summary} {caller}");
    if let GuardOutcome::Blocked(rule) = guard::narration(&line, &source) {
        return Err(rule);
    }
    // The title is the request: a line made of its words ("I checked the GitHub release")
    // says nothing of the result.
    if !carries_result(&line, summary, title) {
        return Err("no_result");
    }
    if said.iter().any(|(_, said)| guard::repeats(&line, said)) {
        return Err("repeated");
    }
    if !screen {
        return Ok(Say::Front(line));
    }
    let more = persona.resolve(&template("update.screen"), 0);
    Ok(Say::Front(
        format!("{line} {}", more.unwrap_or_default())
            .trim()
            .to_owned(),
    ))
}

/// Whether an answer from a result says something of it that the question did not: a number,
/// or a word of four letters or more of the result. A line about the result instead of from
/// it ("the result does not say") is no answer (live, a 2B model wrote one in place of NONE).
fn carries_result(line: &str, result: &str, question: &str) -> bool {
    let words = |text: &str| -> Vec<String> {
        text.split(|c: char| !(c.is_alphanumeric() || c == ':'))
            .map(|word| word.trim_matches(':').to_lowercase())
            .filter(|word| word.chars().count() >= 4 || word.chars().any(|c| c.is_ascii_digit()))
            .collect()
    };
    let (result, question) = (words(result), words(question));
    words(line)
        .iter()
        .any(|word| result.contains(word) && !question.contains(word))
}

fn decision(act: Act) -> ActDecision {
    ActDecision {
        act,
        task: None,
        wait: None,
        say: None,
        intent: None,
        source: ActSource::Lexicon,
    }
}

/// The delegate decision of a turn the front did not decide.
fn fallback() -> ActDecision {
    ActDecision {
        source: ActSource::Fallback,
        ..decision(Act::Delegate)
    }
}

fn bank(group: &'static str) -> Say {
    Say::Bank {
        group,
        intent: None,
    }
}

fn template(id: &'static str) -> Say {
    filled(id, [])
}

/// A template with its slots.
fn filled<const N: usize>(id: &'static str, slots: [(&'static str, String); N]) -> Say {
    Say::Template {
        id,
        slots: slots.into(),
    }
}

/// A title to follow "Your": without its own "your", "the" or "a".
fn possessed(title: &str) -> &str {
    ["your ", "the ", "a ", "an "]
        .iter()
        .find_map(|lead| title.strip_prefix(lead))
        .unwrap_or(title)
}

/// The first sentence of a text: up to its first full stop, question or exclamation mark that
/// ends a word, or the end of its first line.
fn first_sentence(text: &str) -> &str {
    let text = text.trim();
    let line = text.lines().next().unwrap_or_default();
    let mut ends = line.match_indices(['.', '!', '?']).map(|(at, _)| at + 1);
    let end = ends.find(|at| line[*at..].chars().next().is_none_or(char::is_whitespace));
    line[..end.unwrap_or(line.len())].trim()
}

fn error(code: &str, message: &str) -> PolicyAction {
    PolicyAction::Emit(WireEvent::Error {
        message: message.to_owned(),
        code: code.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::super::floor::FloorMachine;
    use super::super::types::{
        Attribution, Class, DuplexAction, EotSource, FloorCmd, Lineage, RunId, TaskRef, TaskView,
        UttId, WaitReason,
    };
    use super::*;

    const REQUEST: &str = "I need a flight to Denver on Friday";

    /// The caller's answers to "change it, or a second one?" (test data).
    const SECOND_ONE: [&str; 9] = [
        "second",
        "another",
        "new one",
        "both",
        "as well",
        "separate",
        "one more",
        "also",
        "additional",
    ];
    const CHANGE_IT: [&str; 8] = [
        "change",
        "update",
        "replace",
        "instead",
        "same one",
        "first one",
        "modify",
        "correct",
    ];
    /// The caller's answers to the choice offer (test data).
    const KEEP_WAITING: [&str; 12] = [
        "wait",
        "keep",
        "hold",
        "fine",
        "okay",
        "yes",
        "yeah",
        "sure",
        "no rush",
        "take your time",
        "go on",
        "carry on",
    ];
    const STOP_IT: [&str; 3] = ["stop", "cancel", "forget"];

    /// The front's reading of a turn, as the lexicon reads it (test data: the gateway asks the
    /// front, `PolicyAction::Read`).
    pub(super) fn lexicon_reader(policy: &Policy, pending: &Pending) -> Option<String> {
        use super::super::persona::{any_phrase, has_phrase, norm_phrase};
        let lexicon = policy.persona.lexicon();
        let said = match &pending.eot {
            Some(eot) => eot.text.clone(),
            None => policy.turn.as_ref()?.text.clone(),
        };
        let norm = norm_phrase(&said);
        let says = |phrases: &[&str]| phrases.iter().any(|phrase| has_phrase(&norm, phrase));
        let yes =
            any_phrase(&norm, &lexicon.strong_accept) || any_phrase(&norm, &lexicon.weak_accept);
        let no = any_phrase(&norm, &lexicon.decline);
        let label = match &pending.asked {
            Asked::Screen(_) if yes || no => YES,
            Asked::Screen(_) => OTHER,
            Asked::Open(request) => match hitl::parse_answer(&said, request, lexicon) {
                Parsed::Accept { strong: true } => YES,
                Parsed::Accept { strong: false } => WEAK,
                Parsed::Decline { correction: None } if request.kind == HitlKind::Choose => NONE,
                Parsed::Decline { correction: None } => NO,
                Parsed::Decline {
                    correction: Some(_),
                } => CHANGE,
                Parsed::Cancel => CANCEL,
                Parsed::Choice(at) => return Some((at + 1).to_string()),
                Parsed::Value(_) => ANSWER,
                Parsed::Unclear => OTHER,
            },
            Asked::Choice if says(&STOP_IT) => STOP,
            Asked::Choice if says(&KEEP_WAITING) => WAIT,
            Asked::Choice => OTHER,
            Asked::Undo(_) | Asked::Updates(_) if yes && !no => YES,
            Asked::Undo(_) | Asked::Updates(_) if no && !yes => NO,
            Asked::Undo(_) | Asked::Updates(_) => OTHER,
            Asked::Which(_) => match (says(&SECOND_ONE), says(&CHANGE_IT)) {
                (true, false) => SECOND,
                (false, true) => CHANGE,
                _ if any_phrase(&norm, &lexicon.cancel) || no => NEITHER,
                _ => OTHER,
            },
            Asked::Same(_, tasks) => {
                let same =
                    |(_, title): &(TaskRef, String)| rules::near_duplicate(&said, title, lexicon);
                return Some(match tasks.iter().position(same) {
                    Some(at) => (at + 1).to_string(),
                    None => "new".to_owned(),
                });
            }
            // Test data: the front's follow-up stands, and so does its delegate.
            Asked::Follows(..) => "1",
            Asked::Started(..) => "new",
        };
        Some(label.to_owned())
    }

    fn policy() -> Policy {
        Policy::new(&InteractionConfig::default(), Persona::load("default"))
    }

    fn turn(turn: u64, at: Ms, text: &str) -> EndOfTurn {
        EndOfTurn {
            turn: TurnId(turn),
            at,
            acoustic_end: Some(at.saturating_sub(300)),
            onset_at: Some(at.saturating_sub(1_500)),
            text: text.to_owned(),
            source: EotSource::SttSpeechFinal,
            eager_same: None,
            class: Class::Interrupt,
            attribution: Attribution::High,
            suspected_echo: false,
            unclear: false,
            foreign: false,
        }
    }

    fn says(turn_id: u64, at: Ms, text: &str) -> PolicyInput {
        PolicyInput::EndOfTurn(turn(turn_id, at, text))
    }

    fn view(alias: &str, state: TaskState, title: &str) -> TaskView {
        TaskView {
            id: TaskId(format!("task_{alias}")),
            task_ref: TaskRef(alias.to_owned()),
            revision: 1,
            title: title.to_owned(),
            state,
            cancelling: false,
            effects: Effects::NoneCommitted,
            open_question: None,
            side_effects: false,
            started_at: 0,
            lineage: Default::default(),
        }
    }

    fn working() -> Board {
        Board {
            tasks: vec![view("t1", TaskState::Working, "a flight to Denver")],
            ..Board::default()
        }
    }

    fn task_event(at: Ms, kind: TaskEventKind) -> PolicyInput {
        PolicyInput::Task(TaskEvent {
            task: TaskId("task_t1".to_owned()),
            task_ref: TaskRef("t1".to_owned()),
            seq: 1,
            revision: 1,
            at,
            state: TaskState::Working,
            cancelling: false,
            lineage: Default::default(),
            kind,
        })
    }

    fn sentence(seq: u32, text: &str, first_after_e_ms: Option<u32>) -> TaskEventKind {
        TaskEventKind::AnswerSentence {
            attempt: 0,
            seq,
            text: text.to_owned(),
            first_after_e_ms,
        }
    }

    fn confirm(irreversible: bool) -> HitlRequest {
        HitlRequest {
            id: HitlId("q_task_t1_1".to_owned()),
            task: TaskId("task_t1".to_owned()),
            revision: 1,
            kind: HitlKind::Confirm,
            op: None,
            verb_phrase: "book a table at Rosa's".to_owned(),
            params: vec![
                ("party".to_owned(), serde_json::json!(2)),
                ("day".to_owned(), serde_json::json!("Friday")),
            ],
            question: None,
            options: Vec::new(),
            irreversible,
            expires_at: 60_000,
            ..Default::default()
        }
    }

    fn asking(request: HitlRequest) -> Board {
        Board {
            tasks: vec![view("t1", TaskState::InputRequired, "a table at Rosa's")],
            open_question: Some(request),
            provisional: None,
        }
    }

    fn done(turn: u64, act: Act) -> PolicyInput {
        PolicyInput::Front(FrontEvent::Done {
            turn: TurnId(turn),
            decision: ActDecision {
                source: ActSource::Router,
                ..decision(act)
            },
        })
    }

    /// The front's decision for `turn`: `act` about the task `task`.
    fn done_on(turn: u64, act: Act, task: &str) -> PolicyInput {
        PolicyInput::Front(FrontEvent::Done {
            turn: TurnId(turn),
            decision: ActDecision {
                task: Some(TaskRef(task.to_owned())),
                ..decision(act)
            },
        })
    }

    /// The caller's turn, then the front's decision of it (chapter 9: the front decides
    /// every turn in context): what the decision does.
    fn decided(
        policy: &mut Policy,
        turn: u64,
        at: Ms,
        text: &str,
        board: &Board,
        decision: ActDecision,
    ) -> Vec<PolicyAction> {
        let asked = policy.step(at, says(turn, at, text), board);
        assert!(
            matches!(asked[..], [PolicyAction::RequestFront { .. }]),
            "no word decides a turn: {asked:?}"
        );
        let done = FrontEvent::Done {
            turn: TurnId(turn),
            decision,
        };
        policy.step(at, PolicyInput::Front(done), board)
    }

    fn spoken(actions: &[PolicyAction]) -> Vec<&Utterance> {
        actions
            .iter()
            .filter_map(|action| match action {
                PolicyAction::Speak(utterance) => Some(utterance),
                _ => None,
            })
            .collect()
    }

    /// The words of every `Speak`, as the default persona resolves them.
    fn words(actions: &[PolicyAction]) -> Vec<String> {
        let persona = Persona::load("default");
        spoken(actions)
            .iter()
            .map(|utterance| match &utterance.say {
                Say::Repeat => "<repeat>".to_owned(),
                say => persona
                    .resolve(say, 0)
                    .unwrap_or_else(|| panic!("no words for {say:?}")),
            })
            .collect()
    }

    fn commands(actions: &[PolicyAction]) -> Vec<&TaskCmd> {
        actions
            .iter()
            .filter_map(|action| match action {
                PolicyAction::Task(cmd) => Some(cmd),
                _ => None,
            })
            .collect()
    }

    /// The act the turn was settled with, and whether it adopts the provisional run.
    fn settled(actions: &[PolicyAction]) -> (Act, bool) {
        let adopts = |cmd: &&TaskCmd| matches!(cmd, TaskCmd::Delegate { adopt: true, .. });
        let act = actions
            .iter()
            .find_map(|action| match action {
                PolicyAction::Settled { act, .. } => Some(*act),
                _ => None,
            })
            .unwrap_or_else(|| panic!("not settled: {actions:?}"));
        (act, commands(actions).iter().any(adopts))
    }

    /// A request at `at` that the front delegates at once.
    fn delegate(policy: &mut Policy, turn: u64, at: Ms, board: &Board) -> Vec<PolicyAction> {
        let actions = policy.step(at, says(turn, at, REQUEST), board);
        assert!(
            matches!(actions[..], [PolicyAction::RequestFront { .. }]),
            "{actions:?}"
        );
        policy.step(at, done(turn, Act::Delegate), board)
    }

    /// The question of `request` asked and heard: read-back at 3,000 ms, question at 4,000 ms.
    fn asked(policy: &mut Policy, request: &HitlRequest) -> Vec<PolicyAction> {
        let board = asking(request.clone());
        let actions = policy.step(
            1_000,
            task_event(1_000, TaskEventKind::InputRequired(request.clone())),
            &board,
        );
        let items: Vec<ItemId> = spoken(&actions)
            .iter()
            .map(|utterance| utterance.item)
            .collect();
        for (item, heard_at) in items.iter().zip([2_000, 3_000, 4_000]) {
            let heard = LaneEvent::Heard {
                item: *item,
                heard_at,
            };
            policy.step(heard_at, PolicyInput::Lane(heard), &board);
        }
        actions
    }

    #[test]
    fn the_acknowledgement_is_due_at_e_plus_300() {
        let mut policy = policy();
        let board = Board::default();
        let actions = delegate(&mut policy, 1, 1_000, &board);
        assert_eq!(settled(&actions), (Act::Delegate, false));
        assert!(matches!(
            commands(&actions)[..],
            [TaskCmd::Delegate {
                turn: TurnId(1),
                adopt: false,
                ..
            }]
        ));
        assert!(spoken(&actions).is_empty());
        assert_eq!(policy.next_wakeup(), Some(1_300));
        assert!(policy.step(1_299, PolicyInput::Tick, &board).is_empty());

        let actions = policy.step(1_300, PolicyInput::Tick, &board);
        let [ack] = spoken(&actions)[..] else {
            panic!("one acknowledgement, got {actions:?}");
        };
        assert_eq!(
            (ack.kind, ack.rank, ack.reply_to),
            (Kind::Ack, 2, Some(TurnId(1)))
        );
        assert_eq!(
            ack.say,
            Say::Bank {
                group: "ack",
                intent: None
            }
        );
        // Once: neither a later tick nor the hard deadline repeats it.
        assert_eq!(policy.next_wakeup(), Some(3_000));
        assert!(policy.step(3_000, PolicyInput::Tick, &board).is_empty());
        assert_eq!(policy.next_wakeup(), None);
    }

    #[test]
    fn the_acknowledgement_waits_while_the_caller_speaks_again() {
        let board = Board::default();
        // A caller who only made a sound gets the one that was due, once they stop.
        let mut sound = policy();
        delegate(&mut sound, 1, 1_000, &board);
        sound.step(1_100, PolicyInput::UserSpeaking { on: true }, &board);
        sound.step(1_500, PolicyInput::UserSpeaking { on: false }, &board);
        assert_eq!(sound.next_wakeup(), Some(1_300));
        let actions = sound.step(1_500, PolicyInput::Tick, &board);
        let [ack] = spoken(&actions)[..] else {
            panic!("one acknowledgement");
        };
        assert_eq!(ack.reply_to, Some(TurnId(1)));

        let mut policy = policy();
        delegate(&mut policy, 1, 1_000, &board);
        // The caller goes on before the acknowledgement is due.
        policy.step(1_100, PolicyInput::UserSpeaking { on: true }, &board);
        assert_eq!(policy.next_wakeup(), None);
        assert!(spoken(&policy.step(1_300, PolicyInput::Tick, &board)).is_empty());
        assert!(spoken(&policy.step(3_000, PolicyInput::Tick, &board)).is_empty());
        // The next turn that ends takes its place: it is acknowledged, at its own time.
        policy.step(4_000, PolicyInput::UserSpeaking { on: false }, &board);
        let actions = delegate(&mut policy, 2, 4_000, &board);
        assert!(spoken(&actions).is_empty());
        assert_eq!(policy.next_wakeup(), Some(4_300));
        let actions = policy.step(4_300, PolicyInput::Tick, &board);
        let [ack] = spoken(&actions)[..] else {
            panic!("one acknowledgement");
        };
        assert_eq!(ack.reply_to, Some(TurnId(2)));
    }

    #[test]
    fn a_yes_no_question_gets_no_affirming_acknowledgement() {
        let mut policy = policy();
        let board = Board::default();
        policy.step(
            1_000,
            says(1, 1_000, "Is my flight to Denver on time"),
            &board,
        );
        policy.step(1_000, done(1, Act::Delegate), &board);
        let actions = policy.step(1_300, PolicyInput::Tick, &board);
        assert_eq!(
            spoken(&actions)[0].say,
            Say::Bank {
                group: "ack",
                intent: Some(POLAR_QUESTION)
            }
        );
        assert!(!words(&actions)[0].starts_with("Sure"));
    }

    #[test]
    fn the_acknowledgement_is_skipped_when_an_answer_sentence_is_ready() {
        let mut policy = policy();
        delegate(&mut policy, 1, 1_000, &Board::default());
        let board = working();
        let actions = policy.step(
            1_200,
            task_event(1_200, sentence(0, "It leaves at nine.", Some(200))),
            &board,
        );
        assert!(matches!(
            actions[0],
            PolicyAction::FirstSentenceAfterE { ms: 200 }
        ));
        let [relay] = spoken(&actions)[..] else {
            panic!("one relayed sentence, got {actions:?}");
        };
        assert_eq!(
            (relay.kind, relay.rank, relay.lane),
            (Kind::Relay, 4, Lane::Main)
        );
        assert_eq!(relay.say, Say::Verbatim("It leaves at nine.".to_owned()));
        assert_eq!((relay.sentence, relay.last), (Some((0, 0)), false));
        assert!(spoken(&policy.step(1_300, PolicyInput::Tick, &board)).is_empty());

        // Nothing audible by E + 2,000 ms: the acknowledgement after all.
        let actions = policy.step(3_000, PolicyInput::Tick, &board);
        assert_eq!(spoken(&actions)[0].kind, Kind::Ack);

        // Once the sentence plays there is nothing to make up for.
        let mut policy = self::policy();
        delegate(&mut policy, 1, 1_000, &Board::default());
        let actions = policy.step(
            1_200,
            task_event(1_200, sentence(0, "It leaves at nine.", Some(200))),
            &board,
        );
        let item = spoken(&actions)[0].item;
        let started = LaneEvent::Started {
            item,
            utt: UttId(1),
            at: 1_400,
        };
        policy.step(1_400, PolicyInput::Lane(started), &board);
        assert!(spoken(&policy.step(3_000, PolicyInput::Tick, &board)).is_empty());
    }

    #[test]
    fn a_ready_answer_behind_a_slow_synthesiser_is_acknowledged_at_once() {
        let board = working();
        let answer = || task_event(1_200, sentence(0, "It leaves at nine.", Some(200)));
        // 1,800 ms to the first audio: the answer is audible by E + 2,000 ms, so no line.
        let mut policy = policy();
        policy.synth_took(1_800, true);
        delegate(&mut policy, 1, 1_000, &Board::default());
        let kinds = |actions: &[PolicyAction]| -> Vec<Kind> {
            spoken(actions).iter().map(|said| said.kind).collect()
        };
        assert_eq!(kinds(&policy.step(1_200, answer(), &board)), [Kind::Relay]);
        assert!(spoken(&policy.step(1_300, PolicyInput::Tick, &board)).is_empty());

        // 1,801 ms: it would not be. The acknowledgement is said now, before the answer.
        let mut policy = self::policy();
        policy.synth_took(1_801, true);
        // A sentence that waited behind another line took at most that long.
        policy.synth_took(2_400, false);
        delegate(&mut policy, 1, 1_000, &Board::default());
        let actions = policy.step(1_200, answer(), &board);
        assert_eq!(kinds(&actions), [Kind::Relay, Kind::Ack]);
        let ranks: Vec<u8> = spoken(&actions).iter().map(|said| said.rank).collect();
        assert_eq!(ranks, [4, 2], "the floor starts the lower rank first");
        // Once, and not again when it would have been due.
        assert!(spoken(&policy.step(1_300, PolicyInput::Tick, &board)).is_empty());
        let more = task_event(1_400, sentence(1, "It costs forty dollars.", None));
        assert_eq!(kinds(&policy.step(1_400, more, &board)), [Kind::Relay]);

        // The answer that followed the acknowledgement began 900 ms after its submit: the
        // synthesiser is faster than was thought, and the next ready answer goes first again.
        policy.synth_took(900, false);
        delegate(&mut policy, 2, 10_000, &board);
        assert_eq!(kinds(&policy.step(10_200, answer(), &board)), [Kind::Relay]);
    }

    #[test]
    fn the_relay_keeps_one_item_and_the_terminal_event_closes_it() {
        let mut policy = policy();
        let board = working();
        let first = policy.step(
            1_000,
            task_event(1_000, sentence(0, "It leaves at nine.", Some(900))),
            &board,
        );
        let second = policy.step(
            1_500,
            task_event(1_500, sentence(1, "It costs forty dollars.", None)),
            &board,
        );
        let item = spoken(&first)[0].item;
        assert_eq!(spoken(&second)[0].item, item);
        assert_eq!(spoken(&second)[0].sentence, Some((0, 1)));

        let completed = TaskEventKind::Completed {
            text: "It leaves at nine. It costs forty dollars.".to_owned(),
            effects: Effects::NoneCommitted,
        };
        let actions = policy.step(1_600, task_event(1_600, completed), &Board::default());
        let [end] = spoken(&actions)[..] else {
            panic!("the end of the relay, got {actions:?}");
        };
        assert_eq!((end.item, end.last), (item, true));
        assert_eq!(end.say, Say::Verbatim(String::new()));
    }

    #[test]
    fn fast_mode_starts_after_three_fast_answers() {
        let mut policy = policy();
        let board = Board::default();
        for (turn, e) in [(1, 10_000), (2, 20_000), (3, 30_000)] {
            delegate(&mut policy, turn, e, &board);
            assert_eq!(policy.next_wakeup(), Some(e + 300));
            policy.step(
                e + 500,
                task_event(e + 500, sentence(0, "It leaves at nine.", Some(500))),
                &board,
            );
        }
        delegate(&mut policy, 4, 40_000, &board);
        assert_eq!(policy.next_wakeup(), Some(40_700));
        assert!(policy.step(40_699, PolicyInput::Tick, &board).is_empty());
        assert_eq!(
            spoken(&policy.step(40_700, PolicyInput::Tick, &board))[0].kind,
            Kind::Ack
        );

        // Three slow answers move the median of the last five back over 700 ms.
        for (turn, e) in [(5, 50_000), (6, 60_000), (7, 70_000)] {
            delegate(&mut policy, turn, e, &board);
            let late = task_event(e + 900, sentence(0, "It leaves at nine.", Some(900)));
            policy.step(e + 900, late, &board);
        }
        delegate(&mut policy, 8, 80_000, &board);
        assert_eq!(policy.next_wakeup(), Some(80_300));
    }

    #[test]
    fn progress_waits_4_s_keeps_5_s_apart_and_stops_at_three() {
        let mut policy = policy();
        let board = working();
        let progress = |at, text: &str| {
            let text = Some(text.to_owned());
            task_event(at, TaskEventKind::Progress { text })
        };
        // The wait starts at 1,000 ms. A phrase from before its 4 s mark is kept, and the
        // latest one is said when the mark passes.
        assert!(policy.step(1_000, PolicyInput::Tick, &board).is_empty());
        assert!(
            policy
                .step(1_500, progress(1_500, "Looking."), &board)
                .is_empty()
        );
        assert!(
            policy
                .step(2_500, progress(2_500, "Checking flights."), &board)
                .is_empty()
        );
        assert_eq!(policy.next_wakeup(), Some(5_000));
        assert!(policy.step(4_999, PolicyInput::Tick, &board).is_empty());
        let actions = policy.step(5_000, PolicyInput::Tick, &board);
        let [line] = spoken(&actions)[..] else {
            panic!("one progress line, got {actions:?}");
        };
        assert_eq!(
            (line.kind, line.rank, line.expires_at),
            (Kind::Progress, 7, Some(8_000))
        );
        assert_eq!(line.say, Say::Verbatim("Checking flights.".to_owned()));
        assert!(policy.step(5_001, PolicyInput::Tick, &board).is_empty());
        // The holding line at 9,000 ms is the second update of this wait.
        assert_eq!(
            spoken(&policy.step(9_000, PolicyInput::Tick, &board))[0].kind,
            Kind::Hold
        );
        // The next phrase waits until the last one is 5 s back; past it, a phrase is said
        // as it comes.
        assert!(
            policy
                .step(9_500, progress(9_500, "Comparing fares."), &board)
                .is_empty()
        );
        assert_eq!(policy.next_wakeup(), Some(10_000));
        let actions = policy.step(10_000, PolicyInput::Tick, &board);
        assert_eq!(
            spoken(&actions)[0].say,
            Say::Verbatim("Comparing fares.".to_owned())
        );
        // Three updates were said in this wait: no more, and 8 s after the last line the
        // caller is offered the choice.
        let actions = policy.step(20_000, progress(20_000, "Booking."), &board);
        let kinds: Vec<Kind> = spoken(&actions).iter().map(|line| line.kind).collect();
        assert_eq!(kinds, [Kind::ChoiceOffer]);
        assert!(policy.step(30_000, PolicyInput::Tick, &board).is_empty());
    }

    /// Live (launch demo take 15): "Thanks for waiting, I'm back." (a failure line the pump
    /// plays, apart from the floor) at 20.2 s, then the holding line due from the wait's
    /// silence at 22.3 s. A line said apart counts as speech: the hold waits 8 s from its end.
    #[test]
    fn a_line_said_apart_from_the_floor_puts_the_holding_line_off() {
        let mut policy = policy();
        let board = working();
        policy.step(1_000, PolicyInput::Tick, &board);
        assert_eq!(policy.next_wakeup(), Some(9_000));
        policy.spoke_apart(8_200);
        policy.spoke_apart(10_300);
        assert!(policy.step(9_000, PolicyInput::Tick, &board).is_empty());
        assert!(policy.step(18_299, PolicyInput::Tick, &board).is_empty());
        let actions = policy.step(18_300, PolicyInput::Tick, &board);
        assert_eq!(spoken(&actions)[0].kind, Kind::Hold, "{actions:?}");
    }

    #[test]
    fn a_kept_progress_phrase_is_dropped_when_it_no_longer_holds() {
        let board = working();
        let phrase = || {
            let text = Some("Checking flights.".to_owned());
            task_event(1_500, TaskEventKind::Progress { text })
        };
        // A step without a phrase began: the kept words are about the step before it.
        let mut policy = self::policy();
        policy.step(1_000, PolicyInput::Tick, &board);
        policy.step(1_500, phrase(), &board);
        let silent = task_event(2_000, TaskEventKind::Progress { text: None });
        policy.step(2_000, silent, &board);
        assert!(policy.step(5_000, PolicyInput::Tick, &board).is_empty());
        // The answer began: it is what the caller hears next.
        let mut policy = self::policy();
        policy.step(1_000, PolicyInput::Tick, &board);
        policy.step(1_500, phrase(), &board);
        let answer = task_event(3_000, sentence(0, "Two flights match.", None));
        assert_eq!(spoken(&policy.step(3_000, answer, &board)).len(), 1);
        assert!(policy.step(5_000, PolicyInput::Tick, &board).is_empty());
        // The request was revised: the new run is a new wait.
        let mut policy = self::policy();
        policy.step(1_000, PolicyInput::Tick, &board);
        policy.step(1_500, phrase(), &board);
        policy.step(3_000, task_event(3_000, TaskEventKind::Revised), &board);
        assert!(policy.step(7_000, PolicyInput::Tick, &board).is_empty());
    }

    #[test]
    fn the_holding_line_comes_after_8_s_then_the_choice_offer_and_a_check_in_every_30_s() {
        let mut policy = policy();
        let board = working();
        policy.step(1_000, PolicyInput::Tick, &board);
        assert_eq!(policy.next_wakeup(), Some(9_000));
        assert!(policy.step(8_999, PolicyInput::Tick, &board).is_empty());
        let actions = policy.step(9_000, PolicyInput::Tick, &board);
        let [hold] = spoken(&actions)[..] else {
            panic!("one holding line, got {actions:?}");
        };
        assert_eq!((hold.kind, hold.rank), (Kind::Hold, 8));
        assert_eq!(
            hold.say,
            Say::Bank {
                group: "holding",
                intent: None
            }
        );

        // As long again, once: keep waiting, or stop?
        assert_eq!(policy.next_wakeup(), Some(17_000));
        let actions = policy.step(17_000, PolicyInput::Tick, &board);
        assert_eq!(
            words(&actions),
            ["It's still going. Do you want to keep waiting, or should I stop it?"]
        );
        assert_eq!(spoken(&actions)[0].kind, Kind::ChoiceOffer);

        assert_eq!(policy.next_wakeup(), Some(47_000));
        let actions = policy.step(47_000, PolicyInput::Tick, &board);
        assert_eq!(spoken(&actions)[0].kind, Kind::Checkin);
        assert_eq!(
            spoken(&actions)[0].say,
            Say::Bank {
                group: "check_in",
                intent: None
            }
        );
        // Agent speech moves the next one.
        let item = spoken(&actions)[0].item;
        let heard = LaneEvent::Heard {
            item,
            heard_at: 41_300,
        };
        policy.step(49_000, PolicyInput::Lane(heard), &board);
        assert_eq!(policy.next_wakeup(), Some(79_000));
        // Not while the caller speaks; none once the wait is 600 s old; none with no task.
        policy.step(60_000, PolicyInput::UserSpeaking { on: true }, &board);
        assert_eq!(policy.next_wakeup(), None);
        policy.step(61_000, PolicyInput::UserSpeaking { on: false }, &board);
        policy.step(
            590_000,
            PolicyInput::Lane(LaneEvent::Heard {
                item,
                heard_at: 590_000,
            }),
            &board,
        );
        assert_eq!(policy.next_wakeup(), None);
        policy.step(600_000, PolicyInput::Tick, &Board::default());
        assert_eq!(policy.next_wakeup(), None);
    }

    #[test]
    fn row_1_echo_or_no_words_is_a_wait_with_no_task() {
        let mut policy = policy();
        let provisional = Board {
            provisional: Some(RunId(1)),
            ..Board::default()
        };
        let echo = EndOfTurn {
            suspected_echo: true,
            ..turn(1, 1_000, "I found two flights")
        };
        let actions = policy.step(1_000, PolicyInput::EndOfTurn(echo), &provisional);
        assert_eq!(settled(&actions), (Act::Wait, false));
        assert!(matches!(
            commands(&actions)[..],
            [TaskCmd::DiscardProvisional { .. }]
        ));
        assert!(spoken(&actions).is_empty());

        let failed = EndOfTurn {
            source: EotSource::SttFailed,
            ..turn(2, 2_000, "")
        };
        let actions = policy.step(2_000, PolicyInput::EndOfTurn(failed), &Board::default());
        assert!(
            matches!(actions[..], [PolicyAction::Settled { act: Act::Wait, .. }]),
            "{actions:?}"
        );
        assert_eq!(policy.next_wakeup(), None);
    }

    #[test]
    fn row_2_a_strong_yes_after_the_read_back_binds() {
        let mut policy = policy();
        let request = confirm(true);
        let actions = asked(&mut policy, &request);
        // Lead-in, the read-back as its own utterance that must be heard, the question.
        let parts = spoken(&actions);
        assert_eq!(
            words(&actions),
            [
                "To confirm:",
                "book a table at Rosa's, 2, Friday.",
                "Do you want me to do that?"
            ]
        );
        assert!(
            parts
                .iter()
                .all(|part| part.rank == 3 && part.question == Some(request.id.clone()))
        );
        assert_eq!(
            parts.iter().map(|part| part.kind).collect::<Vec<_>>(),
            [Kind::Confirm, Kind::Confirm, Kind::Question]
        );
        assert!(policy.answer_expected(4_500));

        let actions = policy.step(6_000, says(1, 6_000, "yes"), &asking(request));
        assert_eq!(settled(&actions), (Act::Update, false));
        assert!(matches!(
            commands(&actions)[..],
            [TaskCmd::Answer {
                outcome: HitlOutcome::Accept { value: None },
                by: AnsweredBy::Voice,
                ..
            }]
        ));
        assert_eq!(words(&actions), ["Okay."]);
    }

    #[test]
    fn row_2_a_weak_yes_is_asked_once_more_and_then_given_up() {
        let mut policy = policy();
        let request = confirm(true);
        asked(&mut policy, &request);
        let board = asking(request);
        let actions = policy.step(6_000, says(1, 6_000, "okay"), &board);
        assert_eq!(settled(&actions).0, Act::Question);
        assert!(commands(&actions).is_empty());
        assert_eq!(
            words(&actions),
            ["Just to be sure: do you want me to book a table at Rosa's?"]
        );

        let actions = policy.step(9_000, says(2, 9_000, "okay"), &board);
        assert!(matches!(
            commands(&actions)[..],
            [TaskCmd::Answer {
                outcome: HitlOutcome::Timeout,
                by: AnsweredBy::Timeout,
                ..
            }]
        ));
        assert!(!policy.answer_expected(9_000));
        // The registry's event says it aloud.
        let resolved = TaskEventKind::InputResolved {
            request: HitlId("q_task_t1_1".to_owned()),
            outcome: HitlOutcome::Timeout,
            by: AnsweredBy::Timeout,
            redacted: false,
        };
        let actions = policy.step(9_000, task_event(9_000, resolved), &working());
        assert_eq!(words(&actions), ["No problem, I'll hold off for now."]);
    }

    #[test]
    fn row_2_a_yes_just_after_the_read_back_is_asked_again_with_the_values() {
        let mut policy = policy();
        let request = confirm(true);
        let board = asking(request.clone());
        let actions = policy.step(
            1_000,
            task_event(1_000, TaskEventKind::InputRequired(request)),
            &board,
        );
        // The read-back was heard at 4,000. This yes began at 3,300, and its transcript comes
        // when nothing of the question may be left to play.
        let heard = LaneEvent::Heard {
            item: spoken(&actions)[1].item,
            heard_at: 4_000,
        };
        policy.step(4_000, PolicyInput::Lane(heard), &board);
        let actions = policy.step(4_800, says(1, 4_800, "yes"), &board);
        assert!(commands(&actions).is_empty());
        assert_eq!(
            words(&actions),
            ["Okay, that's 2, Friday. Do you want me to do that?"]
        );

        // Hearing the re-ask is hearing the read-back: the next yes binds.
        let heard = LaneEvent::Heard {
            item: spoken(&actions)[0].item,
            heard_at: 7_000,
        };
        policy.step(7_000, PolicyInput::Lane(heard), &board);
        let actions = policy.step(9_000, says(2, 9_000, "yes"), &board);
        assert!(matches!(
            commands(&actions)[..],
            [TaskCmd::Answer {
                outcome: HitlOutcome::Accept { .. },
                ..
            }]
        ));
    }

    #[test]
    fn row_2_a_yes_during_the_read_back_waits_for_the_question_that_is_still_to_come() {
        let mut policy = policy();
        let request = confirm(true);
        let board = asking(request.clone());
        let actions = policy.step(
            1_000,
            task_event(1_000, TaskEventKind::InputRequired(request)),
            &board,
        );
        let readback = spoken(&actions)[1].item;
        // Nothing is accepted and nothing is added: the read-back and the question go on (or
        // are offered again when the yes cut them), and they are the re-ask.
        let actions = policy.step(1_800, says(1, 1_800, "yes"), &board);
        assert_eq!(settled(&actions).0, Act::Question);
        assert!(commands(&actions).is_empty() && words(&actions).is_empty());

        let heard = LaneEvent::Heard {
            item: readback,
            heard_at: 5_000,
        };
        policy.step(5_000, PolicyInput::Lane(heard), &board);
        let actions = policy.step(8_000, says(2, 8_000, "yes"), &board);
        assert!(matches!(
            commands(&actions)[..],
            [TaskCmd::Answer {
                outcome: HitlOutcome::Accept { .. },
                ..
            }]
        ));
    }

    #[test]
    fn row_2_a_second_yes_during_the_read_back_gives_the_question_up() {
        let mut policy = policy();
        let request = confirm(true);
        let board = asking(request.clone());
        policy.step(
            1_000,
            task_event(1_000, TaskEventKind::InputRequired(request)),
            &board,
        );
        policy.step(1_800, says(1, 1_800, "yes"), &board);
        let actions = policy.step(2_600, says(2, 2_600, "yes"), &board);
        assert!(matches!(
            commands(&actions)[..],
            [TaskCmd::Answer {
                outcome: HitlOutcome::Timeout,
                by: AnsweredBy::Timeout,
                ..
            }]
        ));
    }

    #[test]
    fn a_deadline_the_timers_already_saw_does_not_wake_the_actor_again() {
        // An undo offer the caller heard and never answered: it waits for their next turn. Its
        // time once passed, the actor is not woken for it again and again.
        let mut policy = policy();
        policy.undo = Some(UndoOffer {
            item: ItemId(7),
            task: TaskRef("t1".to_owned()),
            asked: "Undo it?".to_owned(),
            text: "cancel the booking".to_owned(),
            until: Some(5_000),
        });
        assert_eq!(policy.next_wakeup(), Some(5_000));
        policy.step(6_000, PolicyInput::Tick, &Board::default());
        assert!(
            policy.next_wakeup().is_none_or(|at| at > 6_000),
            "{:?}",
            policy.next_wakeup()
        );
        assert!(
            policy.undo.is_some(),
            "the offer is still there for the next turn"
        );
    }

    #[test]
    fn the_front_reads_an_answer_and_no_word_decides_it() {
        // The gateway's own path: the policy asks the front for the reading of the turn
        // (chapter 9) and acts on the label it names, whatever the words.
        let mut policy = policy();
        policy.reader = None;
        let request = confirm(true);
        asked(&mut policy, &request);
        let board = asking(request);
        let actions = policy.step(6_000, says(1, 6_000, "yes, but make it four"), &board);
        let [PolicyAction::Read(reading)] = &actions[..] else {
            panic!("a reading, got {actions:?}");
        };
        assert_eq!(reading.turn, TurnId(1));
        assert_eq!(reading.kind, ReadingKind::Reply);
        assert!(
            reading.context.starts_with("Agent: \u{ab}"),
            "{}",
            reading.context
        );
        assert_eq!(reading.said, "yes, but make it four");
        let labels: Vec<&str> = reading
            .labels
            .iter()
            .map(|(label, _)| label.as_str())
            .collect();
        assert_eq!(labels, ["yes", "okay", "no", "change", "cancel", "other"]);
        // A label for another turn is not this one's.
        let stale = PolicyInput::Read {
            turn: TurnId(9),
            label: Some("no".to_owned()),
        };
        assert!(policy.step(6_100, stale, &board).is_empty());
        let read = PolicyInput::Read {
            turn: TurnId(1),
            label: Some("change".to_owned()),
        };
        let actions = policy.step(6_150, read, &board);
        let [TaskCmd::Revise { task, correction }] = commands(&actions)[..] else {
            panic!("a revision, got {actions:?}");
        };
        assert_eq!(
            (task.0.as_str(), correction.as_str()),
            ("t1", "yes, but make it four")
        );

        // No reading in time: the question is asked again, nothing is guessed.
        let actions = policy.step(8_000, says(2, 8_000, "yes"), &board);
        assert!(matches!(actions[..], [PolicyAction::Read(_)]));
        let none = PolicyInput::Read {
            turn: TurnId(2),
            label: None,
        };
        let actions = policy.step(8_100, none, &board);
        assert!(commands(&actions).is_empty(), "{actions:?}");
        assert_eq!(settled(&actions).0, Act::Question);

        // A request on a session whose tool books: the front reads whether it repeats the
        // running booking, and a new one starts at once.
        let mut policy = super::tests::policy();
        policy.reader = None;
        let mut booking = view("t1", TaskState::Working, "book a table for two at Rosa's");
        booking.side_effects = true;
        let board = Board {
            tasks: vec![booking],
            ..Board::default()
        };
        let delegate = ActDecision {
            source: ActSource::Model,
            ..decision(Act::Delegate)
        };
        let actions = decided(
            &mut policy,
            1,
            1_000,
            "book a taxi to Rosa's",
            &board,
            delegate,
        );
        let [PolicyAction::Read(reading)] = &actions[..] else {
            panic!("a reading, got {actions:?}");
        };
        assert_eq!(reading.kind, ReadingKind::Same);
        assert!(reading.context.contains("book a table for two at Rosa's"));
        let new = PolicyInput::Read {
            turn: TurnId(1),
            label: Some("new".to_owned()),
        };
        let actions = policy.step(1_100, new, &board);
        assert_eq!(settled(&actions).0, Act::Delegate);
        assert!(
            commands(&actions)
                .iter()
                .any(|cmd| matches!(cmd, TaskCmd::Delegate { .. }))
        );

        // Just finished: a booking that committed something may be asked for again, an answer
        // that committed nothing is a lookup (live B1: a question about the planets answer).
        for (effects, read) in [
            (Effects::Committed(vec!["booked".to_owned()]), true),
            (Effects::Unknown, true),
            (Effects::NoneCommitted, false),
        ] {
            let mut policy = super::tests::policy();
            policy.reader = None;
            let mut done = view("t1", TaskState::Completed, "tell me about the planets");
            done.side_effects = true;
            done.effects = effects;
            policy.finished.push_back((done.id.clone(), 500));
            let board = Board {
                tasks: vec![done],
                ..Board::default()
            };
            let delegate = ActDecision {
                source: ActSource::Model,
                ..decision(Act::Delegate)
            };
            let text = "and which one is the largest?";
            let actions = decided(&mut policy, 1, 1_000, text, &board, delegate);
            let reading = matches!(&actions[..], [PolicyAction::Read(_)]);
            assert_eq!(reading, read, "{actions:?}");
        }
    }

    #[test]
    fn row_2_a_correction_revises_and_a_refusal_declines() {
        let mut policy = policy();
        let request = confirm(true);
        asked(&mut policy, &request);
        let board = asking(request);
        let actions = policy.step(6_000, says(1, 6_000, "yes, but make it four"), &board);
        let [TaskCmd::Revise { task, correction }] = commands(&actions)[..] else {
            panic!("a revision, got {actions:?}");
        };
        assert_eq!((task.0.as_str(), correction.contains("four")), ("t1", true));
        assert_eq!(words(&actions), ["Okay, I'll change that."]);

        let actions = policy.step(8_000, says(2, 8_000, "no"), &board);
        assert!(matches!(
            commands(&actions)[..],
            [TaskCmd::Answer {
                outcome: HitlOutcome::Decline { correction: None },
                ..
            }]
        ));
        assert_eq!(words(&actions), ["Okay, I won't."]);

        let actions = policy.step(9_000, says(3, 9_000, "never mind"), &board);
        assert_eq!(settled(&actions).0, Act::Abort);
        assert!(matches!(
            commands(&actions)[..],
            [TaskCmd::Answer {
                outcome: HitlOutcome::Cancel,
                ..
            }]
        ));

        let actions = policy.step(
            10_000,
            says(4, 10_000, "what time is it over there"),
            &board,
        );
        assert!(commands(&actions).is_empty());
        assert_eq!(words(&actions), ["Sorry, was that a yes or a no?"]);
    }

    #[test]
    fn row_3_a_continuer_is_a_wait() {
        let mut policy = policy();
        let continuer = ActDecision {
            wait: Some(WaitReason::Continuer),
            ..decision(Act::Wait)
        };
        let actions = decided(&mut policy, 1, 1_000, "mm-hm", &working(), continuer);
        assert!(
            matches!(actions[..], [PolicyAction::Settled { act: Act::Wait, .. }]),
            "{actions:?}"
        );
    }

    #[test]
    fn row_4_a_cancel_the_front_reads_stops_the_live_task() {
        let mut policy = policy();
        let cancel = ActDecision {
            task: Some(TaskRef("t1".to_owned())),
            ..decision(Act::Abort)
        };
        let actions = decided(
            &mut policy,
            1,
            1_000,
            "never mind, cancel that",
            &working(),
            cancel,
        );
        assert_eq!(settled(&actions).0, Act::Abort);
        let [TaskCmd::Cancel { task, by: "user" }] = commands(&actions)[..] else {
            panic!("a cancel, got {actions:?}");
        };
        assert_eq!(task.0, "t1");
        assert_eq!(words(&actions), ["Okay, I'll stop that."]);
        assert_eq!(spoken(&actions)[0].rank, 2);
    }

    #[test]
    fn row_5_a_control_settles_the_turn_and_says_nothing_itself() {
        // The actor applies the control (`PolicyInput::ControlLine` for what it says).
        let mut policy = policy();
        let repeat = decision(Act::Control);
        let actions = decided(
            &mut policy,
            1,
            1_000,
            "sorry, what was that?",
            &Board::default(),
            repeat,
        );
        assert_eq!(settled(&actions).0, Act::Control);
        assert!(spoken(&actions).is_empty(), "{actions:?}");
        let line = PolicyInput::ControlLine {
            kind: Kind::Notice,
            say: Say::Repeat,
        };
        let actions = policy.step(1_100, line, &Board::default());
        assert_eq!(spoken(&actions)[0].say, Say::Repeat);
    }

    #[test]
    fn row_6_a_status_or_presence_question_is_answered_from_the_board() {
        let mut policy = policy();
        let status = decision(Act::Progress);
        let actions = decided(&mut policy, 1, 1_000, "is it done yet?", &working(), status);
        assert_eq!(settled(&actions).0, Act::Progress);
        assert_eq!(words(&actions), ["Still working on a flight to Denver."]);
        assert!(commands(&actions).is_empty());

        let presence = ActDecision {
            intent: Some("presence"),
            ..decision(Act::Progress)
        };
        let actions = decided(
            &mut policy,
            2,
            2_000,
            "are you still there?",
            &working(),
            presence,
        );
        assert_eq!(
            words(&actions),
            ["Yes, I'm here. I'm still working on a flight to Denver."]
        );

        let queued = Board {
            tasks: vec![
                view("t1", TaskState::Working, "a flight to Denver"),
                view("t2", TaskState::Submitted, "a hotel"),
            ],
            ..Board::default()
        };
        let asks = ActDecision {
            task: Some(TaskRef("t2".to_owned())),
            ..decision(Act::Progress)
        };
        policy.step(3_000, says(3, 3_000, REQUEST), &queued);
        let front = FrontEvent::Done {
            turn: TurnId(3),
            decision: asks,
        };
        let actions = policy.step(3_000, PolicyInput::Front(front), &queued);
        assert_eq!(
            words(&actions),
            ["I'm still finishing a flight to Denver; I'll do that next."]
        );
    }

    #[test]
    fn a_status_question_is_answered_from_the_task_tree() {
        let child = |alias: &str| TaskView {
            lineage: Lineage {
                parent_id: Some(TaskId("task_t1".to_owned())),
                agent: Some("browser".to_owned()),
                ..Lineage::default()
            },
            ..view(alias, TaskState::Working, "the airline's form")
        };
        let mut board = Board {
            tasks: vec![
                view("t1", TaskState::Working, "a flight to Denver"),
                child("t2"),
                view("t3", TaskState::Submitted, "a hotel in Rome"),
            ],
            ..Board::default()
        };
        let mut policy = policy();
        let actions = decided(
            &mut policy,
            1,
            1_000,
            "is it done yet?",
            &board,
            decision(Act::Progress),
        );
        assert_eq!(settled(&actions).0, Act::Progress);
        // The caller's requests, most recent first; a child is not one.
        assert_eq!(
            words(&actions),
            ["Right now I'm working on a hotel in Rome, a flight to Denver."]
        );
        for (alias, title) in [("t4", "a taxi"), ("t5", "a table"), ("t6", "a gift")] {
            board.tasks.push(view(alias, TaskState::Working, title));
        }
        board.tasks.push(view("t7", TaskState::Completed, "a song"));
        let actions = decided(
            &mut policy,
            2,
            3_000,
            "is it done yet?",
            &board,
            decision(Act::Progress),
        );
        assert_eq!(
            words(&actions),
            ["Right now I'm working on a gift, a table, a taxi, and 2 more."]
        );
    }

    #[test]
    fn a_request_that_waits_for_a_place_is_said_to_be_queued_and_a_child_stop_is_not() {
        let created = |alias: &str, parent: Option<&str>| {
            PolicyInput::Task(TaskEvent {
                task: TaskId(format!("task_{alias}")),
                task_ref: TaskRef(alias.to_owned()),
                state: TaskState::Submitted,
                lineage: Box::new(Lineage {
                    parent_id: parent.map(|parent| TaskId(format!("task_{parent}"))),
                    ..Lineage::default()
                }),
                kind: TaskEventKind::Created {
                    title: "a hotel".to_owned(),
                    side_effects: false,
                    speculative: false,
                    backend: "llm".to_owned(),
                },
                ..match task_event(0, TaskEventKind::Working) {
                    PolicyInput::Task(event) => event,
                    _ => unreachable!(),
                }
            })
        };
        let mut board = working();
        board
            .tasks
            .push(view("t2", TaskState::Submitted, "a hotel"));
        let mut policy = policy();
        let actions = policy.step(1_000, created("t2", None), &board);
        assert_eq!(
            words(&actions),
            ["I'll get to that right after a flight to Denver."]
        );
        for (alias, title) in [("t3", "a taxi"), ("t4", "a table")] {
            board.tasks.push(view(alias, TaskState::Working, title));
        }
        let actions = policy.step(2_000, created("t2", None), &board);
        assert_eq!(
            words(&actions),
            ["That's queued behind 3 other requests; I'll get to it after them."]
        );
        // An orchestrator's queued child, and a child its request stopped, are not said.
        let actions = policy.step(3_000, created("t9", Some("t1")), &board);
        assert!(spoken(&actions).is_empty(), "{actions:?}");
        let stopped = TaskEventKind::Cancelled {
            by: "parent",
            effects: Effects::NoneCommitted,
        };
        let actions = policy.step(4_000, task_event(4_000, stopped), &board);
        assert!(spoken(&actions).is_empty(), "{actions:?}");
    }

    #[test]
    fn row_7_an_amendment_revises_and_scratches_what_was_heard() {
        let mut policy = policy();
        let board = working();
        let amend = || ActDecision {
            task: Some(TaskRef("t1".to_owned())),
            ..decision(Act::Update)
        };
        let actions = decided(
            &mut policy,
            1,
            5_000,
            "actually make it Saturday",
            &board,
            amend(),
        );
        assert_eq!(settled(&actions).0, Act::Update);
        let [TaskCmd::Revise { task, correction }] = commands(&actions)[..] else {
            panic!("a revision, got {actions:?}");
        };
        assert_eq!(
            (task.0.as_str(), correction.as_str()),
            ("t1", "actually make it Saturday")
        );
        let [ack] = spoken(&actions)[..] else {
            panic!("one acknowledgement, got {actions:?}");
        };
        assert_eq!(
            ack.say,
            Say::Bank {
                group: "update_ack",
                intent: None
            }
        );
        assert_eq!((ack.rank, ack.expires_at), (2, Some(9_200)));

        // Part of the old answer was heard: it is taken back first.
        let mut policy = self::policy();
        let relayed = policy.step(
            1_000,
            task_event(1_000, sentence(0, "It leaves at nine.", None)),
            &board,
        );
        let item = spoken(&relayed)[0].item;
        let started = LaneEvent::Started {
            item,
            utt: UttId(1),
            at: 1_100,
        };
        policy.step(1_100, PolicyInput::Lane(started), &board);
        let actions = decided(
            &mut policy,
            1,
            5_000,
            "actually make it Saturday",
            &board,
            amend(),
        );
        assert!(actions.iter().any(
            |action| matches!(action, PolicyAction::Drop { item: dropped } if *dropped == item)
        ));
        let groups: Vec<&Say> = spoken(&actions)
            .iter()
            .map(|utterance| &utterance.say)
            .collect();
        assert_eq!(
            groups,
            [
                &Say::Bank {
                    group: "withdrawal",
                    intent: None
                },
                &Say::Bank {
                    group: "update_ack",
                    intent: None
                }
            ]
        );
        // The registry's report of the same stop adds no second line.
        let stopped = TaskEventKind::AnswerStopped {
            attempt: 0,
            released: 1,
            reason: StopReason::Revised,
        };
        assert!(spoken(&policy.step(5_010, task_event(5_010, stopped), &board)).is_empty());
    }

    #[test]
    fn row_8_any_other_turn_goes_to_the_front() {
        let mut policy = policy();
        let actions = policy.step(1_000, says(1, 1_000, REQUEST), &Board::default());
        let [
            PolicyAction::RequestFront {
                turn: TurnId(1),
                text,
                allowed,
                speculate: true,
            },
        ] = &actions[..]
        else {
            panic!("a front request, got {actions:?}");
        };
        assert_eq!(text, REQUEST);
        assert_eq!(
            allowed,
            &[
                Act::Wait,
                Act::Delegate,
                Act::Social,
                Act::Question,
                Act::Refuse,
                Act::Control
            ]
        );
        let actions = policy.step(2_000, says(2, 2_000, REQUEST), &working());
        let [PolicyAction::RequestFront { allowed, .. }] = &actions[..] else {
            panic!("a front request, got {actions:?}");
        };
        assert!(
            [Act::Update, Act::Abort, Act::Progress]
                .iter()
                .all(|act| allowed.contains(act))
        );

        // Speculation switched off.
        let mut cfg = InteractionConfig::default();
        cfg.tasks.speculative_start = Tri::Off;
        let mut policy = Policy::new(&cfg, Persona::load("default"));
        let actions = policy.step(1_000, says(1, 1_000, REQUEST), &Board::default());
        assert!(matches!(
            actions[..],
            [PolicyAction::RequestFront {
                speculate: false,
                ..
            }]
        ));
    }

    #[test]
    fn row_9_the_front_act_settles_the_turn() {
        // Delegate adopts the provisional run; the model's line is the acknowledgement.
        let provisional = Board {
            provisional: Some(RunId(1)),
            ..Board::default()
        };
        let mut policy = policy();
        policy.step(1_000, says(1, 1_000, REQUEST), &Board::default());
        let known = FrontEvent::ActKnown {
            turn: TurnId(1),
            act: Act::Delegate,
            task: None,
            source: ActSource::Model,
        };
        let actions = policy.step(1_120, PolicyInput::Front(known), &provisional);
        assert_eq!(settled(&actions), (Act::Delegate, true));
        assert!(matches!(
            commands(&actions)[..],
            [TaskCmd::Delegate { adopt: true, .. }]
        ));
        let line = ActDecision {
            say: Some("Okay, let me check flights to Denver.".to_owned()),
            source: ActSource::Model,
            ..decision(Act::Delegate)
        };
        let done = FrontEvent::Done {
            turn: TurnId(1),
            decision: line,
        };
        assert!(
            policy
                .step(1_250, PolicyInput::Front(done), &provisional)
                .is_empty()
        );
        let actions = policy.step(1_300, PolicyInput::Tick, &provisional);
        assert_eq!(words(&actions), ["Okay, let me check flights to Denver."]);

        // Any other act discards it; a social line waits for the model's words.
        let mut policy = self::policy();
        policy.step(
            1_000,
            says(1, 1_000, "thanks so much for that"),
            &Board::default(),
        );
        let known = FrontEvent::ActKnown {
            turn: TurnId(1),
            act: Act::Social,
            task: None,
            source: ActSource::Model,
        };
        let actions = policy.step(1_120, PolicyInput::Front(known), &provisional);
        assert_eq!(settled(&actions), (Act::Social, false));
        assert!(matches!(
            commands(&actions)[..],
            [TaskCmd::DiscardProvisional { reason: "social" }]
        ));
        assert!(spoken(&actions).is_empty());
        let line = ActDecision {
            say: Some("You're welcome.".to_owned()),
            intent: Some("thanks"),
            source: ActSource::Model,
            ..decision(Act::Social)
        };
        let done = FrontEvent::Done {
            turn: TurnId(1),
            decision: line,
        };
        let actions = policy.step(1_400, PolicyInput::Front(done), &Board::default());
        assert_eq!(words(&actions), ["You're welcome."]);
        assert_eq!(spoken(&actions)[0].rank, 2);

        // A decision for an older turn is ignored.
        assert!(
            policy
                .step(1_500, self::done(7, Act::Delegate), &Board::default())
                .is_empty()
        );
    }

    #[test]
    fn a_front_with_no_decision_is_not_replaced_by_a_guess() {
        let mut policy = policy();
        let board = Board::default();
        policy.step(1_000, says(1, 1_000, REQUEST), &board);
        // No timer hands the turn to rules (chapter 9).
        assert_eq!(policy.next_wakeup(), None);
        let failed = FrontEvent::Failed { turn: TurnId(1) };
        let actions = policy.step(1_600, PolicyInput::Front(failed), &board);
        assert!(matches!(
            actions[0],
            PolicyAction::Settled {
                act: Act::Question,
                source: ActSource::RuleOverride("front_failed"),
                ..
            }
        ));
        assert!(commands(&actions).is_empty(), "{actions:?}");
        assert_eq!(spoken(&actions)[0].kind, Kind::Question);
    }

    /// One relayed sentence the caller started to hear, then the answer stops.
    fn stopped_after_hearing(reason: StopReason) -> (Vec<PolicyAction>, ItemId) {
        let mut policy = policy();
        let board = working();
        let relayed = policy.step(
            1_000,
            task_event(1_000, sentence(0, "It leaves at nine.", None)),
            &board,
        );
        let item = spoken(&relayed)[0].item;
        let started = LaneEvent::Started {
            item,
            utt: UttId(1),
            at: 1_100,
        };
        policy.step(1_100, PolicyInput::Lane(started), &board);
        let stopped = TaskEventKind::AnswerStopped {
            attempt: 0,
            released: 1,
            reason,
        };
        let mut actions = policy.step(1_500, task_event(1_500, stopped), &board);
        if reason == StopReason::RunFailed {
            // The failure that follows says what became of the task, once.
            let failed = TaskEventKind::Failed {
                error: "boom".to_owned(),
                effects: Effects::NoneCommitted,
            };
            actions.extend(policy.step(1_510, task_event(1_510, failed), &Board::default()));
        }
        (actions, item)
    }

    #[test]
    fn a_stopped_answer_that_was_partly_heard_is_taken_back() {
        for (reason, line) in [
            (StopReason::Revised, "Scratch that."),
            (StopReason::Cancelling, "Okay, I'll stop that."),
            (StopReason::MarkerThenToolCall, "One moment."),
            (
                StopReason::RunFailed,
                "Sorry, I couldn't get that done just now.",
            ),
        ] {
            let (actions, item) = stopped_after_hearing(reason);
            assert!(
                matches!(actions[0], PolicyAction::Drop { item: dropped } if dropped == item),
                "{reason:?}: {actions:?}"
            );
            assert_eq!(words(&actions), [line], "{reason:?}");
            assert_eq!(spoken(&actions)[0].rank, 4);
        }

        // Nothing was heard: the rest is dropped without a word.
        let mut policy = policy();
        let board = working();
        policy.step(
            1_000,
            task_event(1_000, sentence(0, "It leaves at nine.", None)),
            &board,
        );
        let stopped = TaskEventKind::AnswerStopped {
            attempt: 0,
            released: 1,
            reason: StopReason::Revised,
        };
        let actions = policy.step(1_500, task_event(1_500, stopped), &board);
        assert!(
            matches!(actions[..], [PolicyAction::Drop { .. }]),
            "{actions:?}"
        );
    }

    #[test]
    fn a_failure_after_a_commit_says_what_was_done() {
        let mut policy = policy();
        for (effects, line) in [
            (
                Effects::NoneCommitted,
                "Sorry, I couldn't get that done just now.",
            ),
            (
                Effects::Committed(vec!["booked a table for 4".to_owned()]),
                "It didn't finish, but it had already booked a table for 4.",
            ),
            (
                Effects::Unknown,
                "I'm not sure whether that went through. I'll check before doing anything else.",
            ),
        ] {
            let failed = TaskEventKind::Failed {
                error: "the model returned no answer".to_owned(),
                effects,
            };
            let actions = policy.step(1_500, task_event(1_500, failed), &Board::default());
            assert_eq!(words(&actions), [line]);
        }

        // A Bud agent's run that waits for a person's approval did not fail for a reason a
        // second try would mend: the caller hears what it waits for. Once something was
        // done, that is said instead.
        for (effects, line) in [
            (
                Effects::NoneCommitted,
                "That needs someone's approval first, so I haven't done it.",
            ),
            (
                Effects::Committed(vec!["used refund order".to_owned()]),
                "It didn't finish, but it had already used refund order.",
            ),
        ] {
            let failed = TaskEventKind::Failed {
                error: "approval_required: a manager must approve refunds".to_owned(),
                effects,
            };
            let actions = policy.step(1_600, task_event(1_600, failed), &Board::default());
            assert_eq!(words(&actions), [line]);
        }

        // The same when the caller had heard the start of an answer: the stop says nothing,
        // the failure says what was done.
        let board = working();
        let relayed = policy.step(
            2_000,
            task_event(2_000, sentence(0, "Your table is", None)),
            &board,
        );
        let started = LaneEvent::Started {
            item: spoken(&relayed)[0].item,
            utt: UttId(1),
            at: 2_100,
        };
        policy.step(2_100, PolicyInput::Lane(started), &board);
        let stopped = TaskEventKind::AnswerStopped {
            attempt: 0,
            released: 1,
            reason: StopReason::RunFailed,
        };
        let actions = policy.step(2_500, task_event(2_500, stopped), &board);
        assert!(spoken(&actions).is_empty(), "{actions:?}");
        let failed = TaskEventKind::Failed {
            error: "HTTP 500".to_owned(),
            effects: Effects::Committed(vec!["booked a table for 4".to_owned()]),
        };
        let actions = policy.step(2_510, task_event(2_510, failed), &Board::default());
        assert_eq!(
            words(&actions),
            ["It didn't finish, but it had already booked a table for 4."]
        );
    }

    #[test]
    fn a_call_the_callers_change_came_too_late_for_is_said() {
        let mut policy = policy();
        let stale = || {
            let label = "booked a table for 4".to_owned();
            task_event(2_000, TaskEventKind::StaleOutcome { label })
        };
        let actions = policy.step(2_000, stale(), &working());
        assert_eq!(
            words(&actions),
            ["Before your change, booked a table for 4 had already gone through."]
        );
        // A cancel that waited for the call says it itself when it is confirmed.
        let mut board = working();
        board.tasks[0].cancelling = true;
        assert!(spoken(&policy.step(2_100, stale(), &board)).is_empty());
    }

    #[test]
    fn a_stale_outcome_is_said_before_the_revised_question_is_answered() {
        let mut policy = policy();
        let request = confirm(true);
        let board = asking(request.clone());
        // The revised run asks for its call; then the call of the run before it reports.
        let asked = policy.step(
            1_000,
            task_event(1_000, TaskEventKind::InputRequired(request.clone())),
            &board,
        );
        let first: Vec<ItemId> = spoken(&asked).iter().map(|part| part.item).collect();
        let label = "booked a table for 2".to_owned();
        let stale = task_event(2_000, TaskEventKind::StaleOutcome { label });
        let actions = policy.step(2_000, stale, &board);

        // The question is taken back, the line is said as part of it, and it is asked again.
        let dropped: Vec<ItemId> = actions
            .iter()
            .filter_map(|action| match action {
                PolicyAction::Drop { item } => Some(*item),
                _ => None,
            })
            .collect();
        assert_eq!(dropped, first);
        let said = words(&actions);
        assert_eq!(
            said[0],
            "Before your change, booked a table for 2 had already gone through."
        );
        assert_eq!(said[1..], words(&asked)[..]);
        let parts = spoken(&actions);
        assert!(
            parts
                .iter()
                .all(|part| part.rank == 3 && part.question == Some(request.id.clone())),
            "the floor lets nothing else through while the question is open"
        );
        // The old parts coming back do not start the question a third time.
        for item in first {
            let returned = PolicyInput::Lane(LaneEvent::Returned { item });
            assert!(spoken(&policy.step(2_010, returned, &board)).is_empty());
        }
        assert!(spoken(&policy.step(2_020, PolicyInput::Tick, &board)).is_empty());
    }

    #[test]
    fn a_cancel_is_reported_with_what_was_committed() {
        let mut policy = policy();
        let cancelled = |effects| TaskEventKind::Cancelled {
            by: "user",
            effects,
        };
        for (kind, line) in [
            (cancelled(Effects::NoneCommitted), "It's stopped."),
            (
                cancelled(Effects::Committed(vec![
                    "held the table".to_owned(),
                    "charged the card".to_owned(),
                ])),
                "I've stopped it, but before it stopped it had already held the table and charged the card.",
            ),
            (
                cancelled(Effects::Unknown),
                "I've stopped it, but I can't confirm whether it had already gone through.",
            ),
            (
                TaskEventKind::CancelUnconfirmed {
                    effects: Effects::Unknown,
                },
                "I've asked it to stop; I'll let you know when it's confirmed.",
            ),
        ] {
            let actions = policy.step(1_000, task_event(1_000, kind), &Board::default());
            assert_eq!(words(&actions), [line]);
            assert_eq!(
                (spoken(&actions)[0].kind, spoken(&actions)[0].rank),
                (Kind::Notice, 4)
            );
        }

        // A cancel of a task that already ended.
        let ended = Board {
            tasks: vec![view("t1", TaskState::Completed, "a flight")],
            ..Board::default()
        };
        policy.step(2_000, says(1, 2_000, REQUEST), &ended);
        let abort = ActDecision {
            task: Some(TaskRef("t1".to_owned())),
            ..decision(Act::Abort)
        };
        let front = FrontEvent::Done {
            turn: TurnId(1),
            decision: abort,
        };
        let actions = policy.step(2_000, PolicyInput::Front(front), &ended);
        assert!(commands(&actions).is_empty());
        assert_eq!(words(&actions), ["That had already finished:"]);
    }

    #[test]
    fn an_unanswered_question_is_asked_again_after_8_s_and_times_out_at_20_s() {
        let mut policy = policy();
        let request = confirm(true);
        asked(&mut policy, &request);
        let board = asking(request);
        // The question was heard at 4,000 ms.
        assert_eq!(policy.next_wakeup(), Some(12_000));
        assert!(policy.step(11_999, PolicyInput::Tick, &board).is_empty());
        let actions = policy.step(12_000, PolicyInput::Tick, &board);
        assert_eq!(
            words(&actions),
            ["Do you want me to book a table at Rosa's?"]
        );
        assert_eq!(spoken(&actions)[0].rank, 3);

        assert_eq!(policy.next_wakeup(), Some(32_000));
        let actions = policy.step(32_000, PolicyInput::Tick, &board);
        assert_eq!(
            actions
                .iter()
                .filter(|action| matches!(action, PolicyAction::Drop { .. }))
                .count(),
            4
        );
        assert!(matches!(
            commands(&actions)[..],
            [TaskCmd::Answer {
                outcome: HitlOutcome::Timeout,
                by: AnsweredBy::Timeout,
                ..
            }]
        ));
        assert_eq!(policy.next_wakeup(), None);
    }

    #[test]
    fn an_unanswered_choice_is_asked_again_as_it_was_put() {
        // Live (MA1): an orchestrator's ask_caller choice re-asked "Do you want me to ?".
        let mut policy = policy();
        let request = HitlRequest {
            kind: HitlKind::Choose,
            verb_phrase: String::new(),
            question: Some("Which city are you flying from?".to_owned()),
            options: vec!["Mumbai".to_owned(), "Pune".to_owned()],
            ..confirm(false)
        };
        asked(&mut policy, &request);
        let board = asking(request);
        let due = policy.next_wakeup().expect("a re-ask is due");
        let actions = policy.step(due, PolicyInput::Tick, &board);
        assert_eq!(
            words(&actions),
            ["Which city are you flying from? Mumbai, or Pune?"]
        );
    }

    #[test]
    fn a_choice_answered_with_no_preference_gives_the_agent_the_callers_words() {
        // Live MA1 (gpt-oss-20b): "Which morning flight would you like? 6:00, 7:30, or 9:00?"
        // answered "No preference, just go ahead." was asked again and again.
        let mut policy = policy();
        policy.reader = None;
        let request = HitlRequest {
            kind: HitlKind::Choose,
            verb_phrase: String::new(),
            question: Some("Which morning flight would you like?".to_owned()),
            options: vec!["6:00".to_owned(), "9:00".to_owned()],
            ..confirm(false)
        };
        let board = asking(request.clone());
        for (utt, part) in spoken(&asked(&mut policy, &request)).iter().enumerate() {
            let started = LaneEvent::Started {
                item: part.item,
                utt: UttId(utt as u64 + 1),
                at: 4_500,
            };
            policy.step(4_500, PolicyInput::Lane(started), &board);
        }
        let said = "No preference, just go ahead.";
        let actions = policy.step(6_000, says(1, 6_000, said), &board);
        let [PolicyAction::Read(reading)] = &actions[..] else {
            panic!("a reading, got {actions:?}");
        };
        let labels: Vec<&str> = reading
            .labels
            .iter()
            .map(|(label, _)| label.as_str())
            .collect();
        assert_eq!(labels, ["1", "2", "any", "none", "cancel", "other"]);
        let read = PolicyInput::Read {
            turn: TurnId(1),
            label: Some("any".to_owned()),
        };
        let actions = policy.step(6_100, read, &board);
        let [TaskCmd::Answer { outcome, .. }] = commands(&actions)[..] else {
            panic!("an answer, got {actions:?}");
        };
        assert_eq!(
            outcome,
            &HitlOutcome::Accept {
                value: Some(serde_json::json!(said))
            }
        );
    }

    #[test]
    fn a_question_the_caller_cut_is_offered_again_once_the_turn_is_settled() {
        let mut policy = policy();
        let request = confirm(true);
        let board = asking(request.clone());
        let actions = policy.step(
            1_000,
            task_event(1_000, TaskEventKind::InputRequired(request)),
            &board,
        );
        let readback = spoken(&actions)[1].item;
        let cut = LaneEvent::Dropped {
            item: readback,
            heard_chars: 4,
            cause: CutCause::Interrupt,
        };
        policy.step(1_500, PolicyInput::UserSpeaking { on: true }, &board);
        assert!(
            policy
                .step(1_500, PolicyInput::Lane(cut), &board)
                .is_empty()
        );

        let noise = EndOfTurn {
            source: EotSource::SttFailed,
            ..turn(1, 2_500, "")
        };
        let actions = policy.step(2_500, PolicyInput::EndOfTurn(noise), &board);
        assert_eq!(settled(&actions).0, Act::Wait);
        assert_eq!(spoken(&actions).len(), 3);
        assert!(spoken(&actions).iter().all(|part| part.item != readback));
    }

    #[test]
    fn a_yes_over_the_re_ask_is_the_answer_and_the_question_does_not_start_over() {
        let mut policy = policy();
        let request = confirm(true);
        asked(&mut policy, &request);
        let board = asking(request);
        // Silence: the agent asks again, and the caller says yes over that line, which cuts it.
        let actions = policy.step(13_100, PolicyInput::Tick, &board);
        assert_eq!(spoken(&actions).len(), 1, "the re-ask after 8 s of silence");
        let cut = LaneEvent::Dropped {
            item: spoken(&actions)[0].item,
            heard_chars: 10,
            cause: CutCause::Interrupt,
        };
        policy.step(13_600, PolicyInput::UserSpeaking { on: true }, &board);
        policy.step(13_600, PolicyInput::Lane(cut), &board);
        // The turn ends: the caller is silent, then come the words.
        let silent = PolicyInput::UserSpeaking { on: false };
        assert!(policy.step(14_500, silent, &board).is_empty());
        let actions = policy.step(14_500, says(1, 14_500, "yes"), &board);
        assert!(matches!(
            commands(&actions)[..],
            [TaskCmd::Answer {
                outcome: HitlOutcome::Accept { .. },
                by: AnsweredBy::Voice,
                ..
            }]
        ));
    }

    #[test]
    fn choose_and_value_questions_take_the_callers_answer() {
        let mut policy = policy();
        let choose = HitlRequest {
            kind: HitlKind::Choose,
            question: Some("Which flight do you want:".to_owned()),
            options: vec![
                "the 9 AM".to_owned(),
                "the noon".to_owned(),
                "the 6 PM".to_owned(),
            ],
            ..confirm(false)
        };
        let board = asking(choose.clone());
        let actions = policy.step(
            1_000,
            task_event(1_000, TaskEventKind::InputRequired(choose)),
            &board,
        );
        assert_eq!(
            words(&actions),
            ["Which flight do you want: the 9 AM, the noon, or the 6 PM?"]
        );
        let question = spoken(&actions)[0].item;
        // The caller was still talking when the question opened: those words are not its
        // answer, and the question stays open.
        let early = says(1, 1_050, "and the second thing is the weather in Paris");
        let before = policy.step(1_050, early, &board);
        let answers = |actions: &[PolicyAction]| {
            let answer = |cmd: &&TaskCmd| matches!(cmd, TaskCmd::Answer { .. });
            commands(actions).into_iter().filter(answer).count()
        };
        assert_eq!(answers(&before), 0, "{before:?}");
        assert!(policy.answer_expected(1_100));

        let started = LaneEvent::Started {
            item: question,
            utt: UttId(1),
            at: 1_600,
        };
        policy.step(1_600, PolicyInput::Lane(started), &board);
        // An answer that began while the options were still being read out does not bind:
        // the question plays on.
        let during = policy.step(3_400, says(2, 3_400, "the second one"), &board);
        assert_eq!(answers(&during), 0, "{during:?}");
        assert!(spoken(&during).is_empty(), "{during:?}");
        let heard = LaneEvent::Heard {
            item: question,
            heard_at: 4_000,
        };
        policy.step(4_000, PolicyInput::Lane(heard), &board);
        // 150 ms after the question was heard is the earliest an answer can begin.
        let actions = policy.step(5_650, says(3, 5_650, "the second one"), &board);
        let [
            TaskCmd::Answer {
                outcome: HitlOutcome::Accept { value: Some(value) },
                ..
            },
        ] = commands(&actions)[..]
        else {
            panic!("an answer, got {actions:?}");
        };
        assert_eq!(value, "the noon");
    }

    #[test]
    fn a_client_answer_binds_and_a_late_one_is_refused() {
        let mut policy = policy();
        let request = confirm(true);
        let id = request.id.clone();
        asked(&mut policy, &request);
        let accept = || PolicyInput::ClientAnswer {
            request: id.clone(),
            outcome: HitlOutcome::Accept { value: None },
        };
        let actions = policy.step(5_000, accept(), &asking(request));
        assert!(matches!(
            commands(&actions)[..],
            [TaskCmd::Answer {
                outcome: HitlOutcome::Accept { .. },
                by: AnsweredBy::Client,
                ..
            }]
        ));
        let resolved = TaskEventKind::InputResolved {
            request: id.clone(),
            outcome: HitlOutcome::Accept { value: None },
            by: AnsweredBy::Client,
            redacted: false,
        };
        let actions = policy.step(5_010, task_event(5_010, resolved), &working());
        assert_eq!(
            actions
                .iter()
                .filter(|action| matches!(action, PolicyAction::Drop { .. }))
                .count(),
            3
        );

        let code = |actions: &[PolicyAction]| match actions {
            [PolicyAction::Emit(WireEvent::Error { code, .. })] => code.clone(),
            other => panic!("an error, got {other:?}"),
        };
        assert_eq!(
            code(&policy.step(6_000, accept(), &working())),
            "question_closed"
        );
        let unknown = PolicyInput::ClientAnswer {
            request: HitlId("q_nobody_1".to_owned()),
            outcome: HitlOutcome::Cancel,
        };
        assert_eq!(
            code(&policy.step(6_000, unknown, &working())),
            "unknown_request_id"
        );

        let cancel = PolicyInput::ClientCancel {
            task: TaskId("task_t1".to_owned()),
        };
        let actions = policy.step(7_000, cancel, &working());
        assert!(matches!(
            commands(&actions)[..],
            [TaskCmd::Cancel { by: "client", .. }]
        ));
    }

    /// Live (launch demo take 22, 28 Oct): "Great. Can you also get" ended at a pause and
    /// "the download numbers in the terminal?" came 0.3 s later as a turn of its own, read
    /// alone as a follow-up of the finished browser task. A turn the caller goes on with
    /// before it is decided carries into the next.
    #[test]
    fn a_turn_the_caller_goes_on_with_before_it_is_decided_carries_into_the_next() {
        let asked = |actions: &[PolicyAction]| {
            actions.iter().find_map(|action| match action {
                PolicyAction::RequestFront { text, .. } => Some(text.clone()),
                _ => None,
            })
        };
        let board = Board::default();
        let mut policy = policy();
        let first = policy.step(10_000, says(3, 10_000, "Great. Can you also get"), &board);
        assert_eq!(asked(&first).as_deref(), Some("Great. Can you also get"));
        let mut next = turn(4, 10_300, "the download numbers in the terminal?");
        next.onset_at = Some(10_050);
        let actions = policy.step(10_300, PolicyInput::EndOfTurn(next), &board);
        assert_eq!(
            asked(&actions).as_deref(),
            Some("Great. Can you also get the download numbers in the terminal?")
        );
        assert_eq!(
            policy.turn.as_ref().map(|turn| turn.text.as_str()),
            Some("Great. Can you also get the download numbers in the terminal?")
        );

        // A turn already decided, or one begun well after the last ended, stands alone.
        let mut policy = self::policy();
        policy.step(10_000, says(3, 10_000, "What time is it?"), &board);
        policy.step(10_100, done(3, Act::Delegate), &board);
        let mut next = turn(4, 10_600, "And in Tokyo?");
        next.onset_at = Some(10_200);
        let actions = policy.step(10_600, PolicyInput::EndOfTurn(next), &board);
        assert_eq!(asked(&actions).as_deref(), Some("And in Tokyo?"));
        let mut policy = self::policy();
        policy.step(10_000, says(3, 10_000, "Great. Can you also get"), &board);
        let mut late = turn(4, 14_000, "What's the weather?");
        late.onset_at = Some(12_000);
        let actions = policy.step(14_000, PolicyInput::EndOfTurn(late), &board);
        assert_eq!(asked(&actions).as_deref(), Some("What's the weather?"));
    }

    /// Live (launch demo take 22, 28 Oct): with the voice's completions lost, every line
    /// played and then failed on the floor's watchdog, each failure said "Sorry, I couldn't
    /// get that done just now.", and that notice, which started and then failed too, said it
    /// again: every 13 s for six minutes. A line the voice could not give is reported to the
    /// client once in a run of failures and never said as a failed task (the health watcher
    /// speaks for the voice); only a line heard to its end ends the run.
    #[test]
    fn a_tts_failure_is_reported_once_and_never_said_as_a_failed_task() {
        let mut policy = policy();
        let board = Board::default();
        let failed = |item| {
            PolicyInput::Lane(LaneEvent::Dropped {
                item: ItemId(item),
                heard_chars: 0,
                cause: CutCause::Failure,
            })
        };
        let reported = |actions: &[PolicyAction]| {
            matches!(
                actions,
                [PolicyAction::Emit(WireEvent::Error { code, .. })] if code == "interaction_tts_failed"
            )
        };
        let actions = policy.step(1_000, failed(1), &board);
        assert!(reported(&actions), "{actions:?}");
        assert!(spoken(&actions).is_empty(), "{actions:?}");
        // A line that starts and fails again is the same run: nothing more.
        let started = |item, at| {
            PolicyInput::Lane(LaneEvent::Started {
                item: ItemId(item),
                utt: UttId(item),
                at,
            })
        };
        policy.step(2_000, started(2, 2_000), &board);
        assert!(policy.step(12_000, failed(2), &board).is_empty());
        // A line heard to its end ends the run.
        policy.step(13_000, started(3, 13_000), &board);
        let heard = LaneEvent::Heard {
            item: ItemId(3),
            heard_at: 14_000,
        };
        policy.step(14_000, PolicyInput::Lane(heard), &board);
        let actions = policy.step(15_000, failed(4), &board);
        assert!(reported(&actions), "{actions:?}");
        assert!(spoken(&actions).is_empty());
    }

    #[test]
    fn ready_offers_the_warm_up_lines_acknowledgements_first() {
        let mut policy = policy();
        let actions = policy.step(0, PolicyInput::Ready, &Board::default());
        let lines = words(&actions);
        assert_eq!(lines, Persona::load("default").warm_lines());
        assert!(lines.len() >= 3);
        assert!(
            spoken(&actions)
                .iter()
                .all(|warm| { warm.discard && warm.rank == 9 && warm.kind == Kind::Warm })
        );
        assert_eq!(policy.next_wakeup(), None);
    }

    #[test]
    fn an_eager_end_of_turn_starts_a_run_that_e_confirms_or_discards() {
        let mut policy = policy();
        let provisional = Board {
            provisional: Some(RunId(1)),
            ..Board::default()
        };
        let eager = || PolicyInput::Eager {
            turn: TurnId(1),
            text: REQUEST.to_owned(),
        };
        let actions = policy.step(900, eager(), &Board::default());
        assert!(matches!(
            commands(&actions)[..],
            [TaskCmd::StartProvisional {
                turn: TurnId(1),
                e_at: None,
                ..
            }]
        ));
        let withdrawn = PolicyInput::EagerWithdrawn { turn: TurnId(1) };
        let actions = policy.step(950, withdrawn, &provisional);
        assert!(matches!(
            commands(&actions)[..],
            [TaskCmd::DiscardProvisional { .. }]
        ));

        // The same words at E: the run goes on, and no second one is started.
        let same = EndOfTurn {
            eager_same: Some(true),
            ..turn(1, 1_000, REQUEST)
        };
        let actions = policy.step(1_000, PolicyInput::EndOfTurn(same), &provisional);
        assert!(matches!(
            commands(&actions)[..],
            [TaskCmd::ConfirmProvisional { e_at: 1_000 }]
        ));
        assert!(matches!(
            actions[1],
            PolicyAction::RequestFront {
                speculate: false,
                ..
            }
        ));

        let different = EndOfTurn {
            eager_same: Some(false),
            ..turn(2, 2_000, REQUEST)
        };
        let actions = policy.step(2_000, PolicyInput::EndOfTurn(different), &provisional);
        assert!(matches!(
            commands(&actions)[..],
            [TaskCmd::DiscardProvisional { .. }]
        ));
        assert!(matches!(
            actions[1],
            PolicyAction::RequestFront {
                speculate: true,
                ..
            }
        ));

        // No eager start while an answer is expected.
        let mut policy = self::policy();
        let request = confirm(true);
        asked(&mut policy, &request);
        assert!(policy.step(5_000, eager(), &asking(request)).is_empty());
    }

    #[test]
    fn a_front_question_opens_the_answer_window_when_it_is_heard() {
        let mut policy = policy();
        let board = Board::default();
        policy.step(1_000, says(1, 1_000, REQUEST), &board);
        let actions = policy.step(1_000, done(1, Act::Question), &board);
        let question = spoken(&actions)[0];
        assert_eq!((question.kind, question.rank), (Kind::Question, 2));
        assert_eq!(
            question.say,
            Say::Bank {
                group: "clarify",
                intent: None
            }
        );
        assert!(!policy.answer_expected(1_500));
        let heard = LaneEvent::Heard {
            item: question.item,
            heard_at: 3_000,
        };
        policy.step(2_700, PolicyInput::Lane(heard), &board);
        assert!(policy.answer_expected(6_999));
        assert!(!policy.answer_expected(7_000));
    }

    #[test]
    fn a_reply_displaces_a_holding_line_on_the_floor() {
        let cfg = InteractionConfig::default();
        let persona = Persona::load("default");
        let mut policy = Policy::new(&cfg, persona.clone());
        let mut floor = FloorMachine::new(&cfg, persona, 150);
        let board = working();
        policy.step(1_000, PolicyInput::Tick, &board);
        // The caller is speaking when the holding line comes due, so it queues.
        floor.on_duplex(8_000, &DuplexAction::UserSpeaking(true));
        for action in policy.step(9_000, PolicyInput::Tick, &board) {
            if let PolicyAction::Speak(hold) = action {
                assert_eq!(hold.rank, 8);
                floor.offer(9_000, hold);
            }
        }
        let presence = ActDecision {
            intent: Some("presence"),
            ..decision(Act::Progress)
        };
        let asks = decided(
            &mut policy,
            1,
            10_000,
            "are you still there?",
            &board,
            presence,
        );
        floor.on_duplex(10_000, &DuplexAction::UserSpeaking(false));
        for action in asks {
            match action {
                PolicyAction::Speak(reply) => drop(floor.offer(10_000, reply)),
                PolicyAction::Settled { turn, .. } => floor.on_settled(10_000, turn),
                other => panic!("unexpected {other:?}"),
            }
        }
        let cmds = floor.next_cmds(10_000);
        let [FloorCmd::Submit { utterance, .. }] = &cmds[..] else {
            panic!("the reply first, got {cmds:?}");
        };
        assert!(matches!(
            utterance.say,
            Say::Template { id: "presence", .. }
        ));
    }

    // ---- unclear turns

    #[test]
    fn an_unclear_turn_goes_to_the_front_with_no_run_and_no_lexicon_row() {
        let mut policy = policy();
        let board = working();
        // Heard clearly, "cancel that" stops the task.
        let unclear = EndOfTurn {
            unclear: true,
            ..turn(1, 1_000, "cancel that")
        };
        let actions = policy.step(1_000, PolicyInput::EndOfTurn(unclear), &board);
        assert!(
            matches!(
                actions[..],
                [PolicyAction::RequestFront {
                    speculate: false,
                    ..
                }]
            ),
            "{actions:?}"
        );
        // The front asks the caller to say it again, in the persona's words.
        let actions = policy.step(1_000, done(1, Act::Question), &board);
        assert_eq!(settled(&actions), (Act::Question, false));
        assert_eq!(spoken(&actions)[0].kind, Kind::Question);
        assert!(commands(&actions).is_empty(), "{actions:?}");
    }

    #[test]
    fn an_unclear_yes_does_not_bind() {
        let mut policy = policy();
        let request = confirm(false);
        asked(&mut policy, &request);
        let board = asking(request);
        let yes = EndOfTurn {
            unclear: true,
            ..turn(1, 6_000, "yes")
        };
        let actions = policy.step(6_000, PolicyInput::EndOfTurn(yes), &board);
        assert!(commands(&actions).is_empty(), "{actions:?}");
        assert_eq!(words(&actions), ["Sorry, was that a yes or a no?"]);
    }

    // ---- a request that nearly repeats a task

    const TABLE_FOR_TWO: &str = "Book a table for two at Rosa's on Saturday";
    const TABLE_FOR_FOUR: &str = "Book a table for four at Rosa's on Saturday";
    const WHICH: &str = "Do you want me to change it, or is this a second one?";

    /// A booking on a session whose tools can book.
    fn booking(state: TaskState) -> Board {
        let task = TaskView {
            side_effects: true,
            ..view("t1", state, TABLE_FOR_TWO)
        };
        Board {
            tasks: vec![task],
            ..Board::default()
        }
    }

    /// The same request for four, delegated by the front at `at`.
    fn for_four(policy: &mut Policy, turn: u64, at: Ms, board: &Board) -> Vec<PolicyAction> {
        policy.step(at, says(turn, at, TABLE_FOR_FOUR), board);
        policy.step(at, done(turn, Act::Delegate), board)
    }

    #[test]
    fn a_request_that_nearly_repeats_a_live_task_is_asked_about() {
        let mut policy = policy();
        let board = booking(TaskState::Working);
        let actions = for_four(&mut policy, 1, 1_000, &board);
        assert_eq!(settled(&actions), (Act::Question, false));
        assert!(commands(&actions).is_empty(), "{actions:?}");
        assert_eq!(words(&actions), [WHICH]);
        assert_eq!(spoken(&actions)[0].kind, Kind::Question);

        // On a session that only looks things up the same request is simply run again.
        let lookups = Board {
            tasks: vec![view("t1", TaskState::Working, TABLE_FOR_TWO)],
            ..Board::default()
        };
        let again = for_four(&mut self::policy(), 1, 1_000, &lookups);
        assert_eq!(settled(&again), (Act::Delegate, false));

        // Not heard yet, words that repeat the question are its echo.
        let item = spoken(&actions)[0].item;
        let echo = EndOfTurn {
            suspected_echo: true,
            ..turn(2, 2_500, "or is this a second one")
        };
        let actions = policy.step(2_500, PolicyInput::EndOfTurn(echo), &board);
        assert_eq!(settled(&actions), (Act::Wait, false));
        let heard = LaneEvent::Heard {
            item,
            heard_at: 4_000,
        };
        policy.step(4_000, PolicyInput::Lane(heard), &board);

        // "A second one": the held request becomes a task of its own. The caller began after
        // the question was heard, so the words are theirs although they are the question's.
        let answer = EndOfTurn {
            suspected_echo: true,
            ..turn(2, 6_000, "a second one please")
        };
        let actions = policy.step(6_000, PolicyInput::EndOfTurn(answer), &board);
        let [
            TaskCmd::Delegate {
                turn: TurnId(2),
                text,
                adopt: false,
            },
        ] = commands(&actions)[..]
        else {
            panic!("a new task, got {actions:?}");
        };
        assert_eq!(text, TABLE_FOR_FOUR);
        assert_eq!(settled(&actions), (Act::Delegate, false));
    }

    #[test]
    fn change_it_revises_the_task_and_never_mind_starts_nothing() {
        let mut policy = policy();
        let board = booking(TaskState::Working);
        for_four(&mut policy, 1, 1_000, &board);
        let actions = policy.step(6_000, says(2, 6_000, "change it"), &board);
        let [TaskCmd::Revise { task, correction }] = commands(&actions)[..] else {
            panic!("a revision, got {actions:?}");
        };
        assert_eq!(
            (task.0.as_str(), correction.as_str()),
            ("t1", TABLE_FOR_FOUR)
        );

        let actions = for_four(&mut policy, 3, 20_000, &board);
        assert_eq!(words(&actions), [WHICH]);
        let actions = policy.step(26_000, says(4, 26_000, "no, never mind"), &board);
        assert!(commands(&actions).is_empty(), "{actions:?}");
        assert_eq!(words(&actions), ["Okay, I'll leave it."]);

        // Words that answer nothing are a turn of their own, and the caller who then says
        // the request once more is not asked again.
        let actions = for_four(&mut policy, 5, 40_000, &board);
        assert_eq!(words(&actions), [WHICH]);
        let actions = for_four(&mut policy, 6, 46_000, &board);
        assert_eq!(settled(&actions), (Act::Delegate, false));
    }

    #[test]
    fn an_unanswered_change_or_second_question_starts_nothing() {
        let mut policy = policy();
        let board = booking(TaskState::Working);
        let actions = for_four(&mut policy, 1, 1_000, &board);
        let item = spoken(&actions)[0].item;
        let heard = LaneEvent::Heard {
            item,
            heard_at: 3_000,
        };
        policy.step(3_000, PolicyInput::Lane(heard), &board);
        assert!(policy.answer_expected(3_100));
        let actions = policy.step(22_999, PolicyInput::Tick, &board);
        assert!(commands(&actions).is_empty(), "{actions:?}");
        let actions = policy.step(23_000, PolicyInput::Tick, &board);
        assert!(commands(&actions).is_empty(), "{actions:?}");
        let held = "No problem, I'll hold off for now.".to_owned();
        assert!(words(&actions).contains(&held), "{actions:?}");
    }

    #[test]
    fn a_task_that_just_finished_counts_and_an_older_one_does_not() {
        // A booking that booked: one that committed nothing is no repeat to ask about.
        let booked = Effects::Committed(vec!["booked a table".to_owned()]);
        let completed = || TaskEventKind::Completed {
            text: "Your table is booked.".to_owned(),
            effects: booked.clone(),
        };
        let mut board = booking(TaskState::Completed);
        board.tasks[0].effects = booked.clone();
        let mut policy = policy();
        policy.step(5_000, task_event(5_000, completed()), &board);
        let actions = for_four(&mut policy, 1, 30_000, &board);
        assert_eq!(words(&actions), [WHICH]);
        // A finished task cannot be revised: the change is a request of its own.
        let actions = policy.step(36_000, says(2, 36_000, "change it"), &board);
        let [TaskCmd::Delegate { text, .. }] = commands(&actions)[..] else {
            panic!("a new task, got {actions:?}");
        };
        assert_eq!(text, TABLE_FOR_FOUR);

        let mut policy = self::policy();
        policy.step(5_000, task_event(5_000, completed()), &board);
        let actions = for_four(&mut policy, 1, 35_001, &board);
        assert_eq!(settled(&actions), (Act::Delegate, false));
    }

    // ---- sensitive values

    #[test]
    fn a_card_number_said_out_loud_is_declined_and_starts_nothing() {
        let mut policy = policy();
        let provisional = Board {
            provisional: Some(RunId(1)),
            ..Board::default()
        };
        let card = says(1, 1_000, "my card number is 4111 1111 1111 1111");
        let actions = policy.step(1_000, card, &provisional);
        assert_eq!(settled(&actions), (Act::Refuse, false));
        assert!(
            matches!(commands(&actions)[..], [TaskCmd::DiscardProvisional { .. }]),
            "{actions:?}"
        );
        assert_eq!(
            words(&actions),
            ["I can't take your card number on this call for security reasons."]
        );
        // The words start no eager run either.
        let eager = PolicyInput::Eager {
            turn: TurnId(2),
            text: "the security code is one two three".to_owned(),
        };
        assert!(policy.step(5_000, eager, &Board::default()).is_empty());

        // Naming the category is not saying the value.
        let broken = says(3, 9_000, "my card number is not working");
        let actions = policy.step(9_000, broken, &Board::default());
        assert!(
            matches!(actions[..], [PolicyAction::RequestFront { .. }]),
            "{actions:?}"
        );
    }

    /// The choice offer of a wait, said at 17 s.
    fn offered(policy: &mut Policy, board: &Board) {
        policy.step(1_000, PolicyInput::Tick, board);
        policy.step(9_000, PolicyInput::Tick, board);
        let actions = policy.step(17_000, PolicyInput::Tick, board);
        assert_eq!(spoken(&actions)[0].kind, Kind::ChoiceOffer, "{actions:?}");
    }

    #[test]
    fn the_choice_offer_is_answered_keep_waiting_or_stop() {
        let mut policy = policy();
        let board = working();
        offered(&mut policy, &board);
        let actions = policy.step(19_000, says(4, 19_000, "I'll keep waiting"), &board);
        assert_eq!(words(&actions), ["Okay."]);
        assert!(commands(&actions).is_empty(), "nothing starts: {actions:?}");

        let mut policy = super::tests::policy();
        offered(&mut policy, &board);
        let actions = policy.step(19_000, says(4, 19_000, "just stop it"), &board);
        assert!(
            matches!(commands(&actions)[..], [TaskCmd::Cancel { by: "user", .. }]),
            "{actions:?}"
        );

        // After ten seconds it is an agreed wait: the words are a turn of their own.
        let mut policy = super::tests::policy();
        offered(&mut policy, &board);
        let actions = policy.step(28_000, says(4, 28_000, "yes"), &board);
        assert_ne!(words(&actions), ["Okay."]);
    }

    #[test]
    fn a_form_and_a_sensitive_value_are_asked_on_the_screen_and_answered_from_it() {
        let mut policy = policy();
        let form = HitlRequest {
            kind: HitlKind::Form,
            question: Some("Where should we deliver it?".to_owned()),
            schema: Some(Box::new(serde_json::json!({ "type": "object" }))),
            ..confirm(false)
        };
        let actions = asked(&mut policy, &form);
        assert_eq!(
            words(&actions),
            ["I'm putting a secure form on your screen for the details."]
        );
        // Heard at 2 s, it is not asked again by voice 8 s later.
        assert_ne!(policy.next_wakeup(), Some(10_000));
        // No spoken words answer it, and it is not asked again by voice.
        let board = asking(form.clone());
        let actions = policy.step(5_000, says(4, 5_000, "yes go ahead"), &board);
        assert!(
            !commands(&actions)
                .iter()
                .any(|cmd| matches!(cmd, TaskCmd::Answer { .. })),
            "{actions:?}"
        );
        let value = Some(serde_json::json!({ "street": "1 High St" }));
        let answer = PolicyInput::ClientAnswer {
            request: form.id.clone(),
            outcome: HitlOutcome::Accept { value },
        };
        let actions = policy.step(6_000, answer, &board);
        assert!(
            matches!(
                commands(&actions)[..],
                [TaskCmd::Answer {
                    by: AnsweredBy::Client,
                    ..
                }]
            ),
            "{actions:?}"
        );

        // A value never taken by voice: on the screen, and a caller who says it is told not to.
        let mut policy = super::tests::policy();
        let card = HitlRequest {
            kind: HitlKind::Value,
            question: Some("What is your card number?".to_owned()),
            ..confirm(false)
        };
        let actions = asked(&mut policy, &card);
        assert_eq!(
            words(&actions),
            ["I'm putting a secure form on your screen for your card number."]
        );
        let board = asking(card);
        let actions = policy.step(5_000, says(4, 5_000, "it's 4111 1111 1111 1111"), &board);
        assert_eq!(
            words(&actions),
            ["Please don't say that out loud. Use your screen instead."]
        );

        // A sign-in link.
        let mut policy = super::tests::policy();
        let link = HitlRequest {
            kind: HitlKind::AuthUrl,
            question: Some("Sign in to continue.".to_owned()),
            url: Some("https://id.example.com/x".into()),
            sensitive: true,
            ..confirm(false)
        };
        assert_eq!(
            words(&asked(&mut policy, &link)),
            ["To do that I need you to sign in. I'm sending a secure link to your screen."]
        );
        // Once the link is on the client's connection the caller hears that it was sent: not
        // for another question, and the line goes when the question closes before it plays.
        let board = asking(link.clone());
        let delivered = |request: &HitlId| PolicyInput::Delivered {
            request: request.clone(),
        };
        let other = HitlId("q_other".to_owned());
        assert!(spoken(&policy.step(5_000, delivered(&other), &board)).is_empty());
        let sent = policy.step(5_000, delivered(&link.id), &board);
        assert_eq!(words(&sent), ["I've sent it."]);
        let item = spoken(&sent)[0].item;
        let closed = TaskEventKind::InputResolved {
            request: link.id.clone(),
            outcome: HitlOutcome::Accept { value: None },
            by: AnsweredBy::Client,
            redacted: true,
        };
        let actions = policy.step(6_000, task_event(6_000, closed), &board);
        let dropped = |action: &PolicyAction| matches!(action, PolicyAction::Drop { item: gone } if *gone == item);
        assert!(actions.iter().any(dropped));
        // A form is not a link: nothing is said when it is delivered.
        let mut policy = super::tests::policy();
        let form = HitlRequest {
            kind: HitlKind::Form,
            question: Some("Your address?".to_owned()),
            ..confirm(false)
        };
        asked(&mut policy, &form);
        let board = asking(form.clone());
        assert!(spoken(&policy.step(5_000, delivered(&form.id), &board)).is_empty());
    }

    #[test]
    fn a_caller_on_a_phone_line_answers_with_keys() {
        let key = |digit| PolicyInput::Dtmf { digit };
        let answers = |actions: &[PolicyAction]| -> Vec<(HitlOutcome, AnsweredBy)> {
            commands(actions)
                .into_iter()
                .filter_map(|cmd| match cmd {
                    TaskCmd::Answer { outcome, by, .. } => Some((outcome.clone(), *by)),
                    _ => None,
                })
                .collect()
        };
        // A confirmation: the hint follows the question, and 1 accepts once it began to play.
        let mut policy = policy();
        policy.set_keypad(true);
        let request = confirm(true);
        let actions = asked(&mut policy, &request);
        assert_eq!(
            words(&actions).last().map(String::as_str),
            Some("You can also press 1 for yes, or 2 for no.")
        );
        let board = asking(request.clone());
        assert!(
            policy.step(4_100, key('1'), &board).is_empty(),
            "not asked yet"
        );
        let ask = spoken(&actions)
            .into_iter()
            .find(|u| u.kind == Kind::Question)
            .unwrap()
            .item;
        let started = LaneEvent::Started {
            item: ask,
            utt: UttId(3),
            at: 4_200,
        };
        policy.step(4_200, PolicyInput::Lane(started), &board);
        assert!(policy.step(4_300, key('5'), &board).is_empty());
        let actions = policy.step(4_400, key('1'), &board);
        assert_eq!(
            answers(&actions),
            [(HitlOutcome::Accept { value: None }, AnsweredBy::Dtmf)]
        );
        // 2 declines.
        let mut policy = super::tests::policy();
        policy.set_keypad(true);
        let actions = asked(&mut policy, &request);
        let ask = spoken(&actions)
            .into_iter()
            .find(|u| u.kind == Kind::Question);
        let started = LaneEvent::Started {
            item: ask.unwrap().item,
            utt: UttId(3),
            at: 4_200,
        };
        policy.step(4_200, PolicyInput::Lane(started), &board);
        let actions = policy.step(4_400, key('2'), &board);
        assert_eq!(
            answers(&actions),
            [(HitlOutcome::Decline { correction: None }, AnsweredBy::Dtmf)]
        );

        // A sensitive value is typed, not said; `*` starts again and `#` ends it.
        let mut policy = super::tests::policy();
        policy.set_keypad(true);
        let card = HitlRequest {
            kind: HitlKind::Value,
            question: Some("What is your card number?".to_owned()),
            ..confirm(false)
        };
        let actions = asked(&mut policy, &card);
        assert_eq!(
            words(&actions),
            [
                "For your security, please type your card number on your keypad, then press the pound key."
            ]
        );
        let board = asking(card);
        let actions = policy.step(5_000, says(4, 5_000, "it's 4111 1111 1111 1111"), &board);
        assert!(answers(&actions).is_empty(), "{actions:?}");
        assert_eq!(
            words(&actions),
            ["Please don't say that out loud. Use your keypad instead."]
        );
        for digit in ['9', '*', '4', '1', '1', '1'] {
            assert!(policy.step(6_000, key(digit), &board).is_empty());
        }
        let actions = policy.step(6_500, key('#'), &board);
        let value = Some(serde_json::json!("4111"));
        assert_eq!(
            answers(&actions),
            [(HitlOutcome::Accept { value }, AnsweredBy::Dtmf)]
        );

        // Off a phone line, or without `dtmf`, keys do nothing and nothing mentions them.
        let mut policy = super::tests::policy();
        let actions = asked(&mut policy, &request);
        assert!(!words(&actions).iter().any(|w| w.contains("press")));
        assert!(policy.step(6_000, key('1'), &asking(request)).is_empty());
    }

    /// A typed value with no `#`: a pause of `policy.hitl_key_gap_ms` after the last key ends
    /// it. A choice on the keypad: the hint names a key per option, and key `n` picks the
    /// `n`-th once the question began to play; a key with no option does nothing.
    #[test]
    fn keys_pause_to_end_a_value_and_pick_an_option_by_number() {
        let key = |digit| PolicyInput::Dtmf { digit };
        let answers = |actions: &[PolicyAction]| -> Vec<(HitlOutcome, AnsweredBy)> {
            commands(actions)
                .into_iter()
                .filter_map(|cmd| match cmd {
                    TaskCmd::Answer { outcome, by, .. } => Some((outcome.clone(), *by)),
                    _ => None,
                })
                .collect()
        };
        let mut policy = super::tests::policy();
        policy.set_keypad(true);
        let card = HitlRequest {
            kind: HitlKind::Value,
            question: Some("What is your card number?".to_owned()),
            ..confirm(false)
        };
        asked(&mut policy, &card);
        let board = asking(card);
        for (at, digit) in [(6_000, '4'), (6_400, '2')] {
            assert!(policy.step(at, key(digit), &board).is_empty());
        }
        assert_eq!(policy.next_wakeup(), Some(11_400));
        assert!(answers(&policy.step(11_399, PolicyInput::Tick, &board)).is_empty());
        let actions = policy.step(11_400, PolicyInput::Tick, &board);
        let value = Some(serde_json::json!("42"));
        assert_eq!(
            answers(&actions),
            [(HitlOutcome::Accept { value }, AnsweredBy::Dtmf)]
        );
        // Nothing typed since: no second answer.
        assert!(answers(&policy.step(20_000, PolicyInput::Tick, &board)).is_empty());

        let mut policy = super::tests::policy();
        policy.set_keypad(true);
        let choose = HitlRequest {
            kind: HitlKind::Choose,
            question: Some("Which city?".to_owned()),
            options: vec!["Mumbai".to_owned(), "Pune".to_owned()],
            ..confirm(false)
        };
        let actions = policy.step(
            1_000,
            task_event(1_000, TaskEventKind::InputRequired(choose.clone())),
            &asking(choose.clone()),
        );
        assert_eq!(
            words(&actions).last().map(String::as_str),
            Some("You can also press 1 for Mumbai, or 2 for Pune.")
        );
        let board = asking(choose);
        assert!(
            policy.step(1_500, key('2'), &board).is_empty(),
            "not asked yet"
        );
        let ask = spoken(&actions)
            .into_iter()
            .find(|u| u.kind == Kind::Question)
            .unwrap()
            .item;
        let started = LaneEvent::Started {
            item: ask,
            utt: UttId(3),
            at: 2_000,
        };
        policy.step(2_000, PolicyInput::Lane(started), &board);
        assert!(policy.step(2_100, key('3'), &board).is_empty());
        let actions = policy.step(2_200, key('2'), &board);
        let value = Some(serde_json::json!("Pune"));
        assert_eq!(
            answers(&actions),
            [(HitlOutcome::Accept { value }, AnsweredBy::Dtmf)]
        );
    }

    #[test]
    fn a_question_for_a_password_is_never_asked() {
        // A client with no screen: `hitl.channels` is voice only.
        let cfg = InteractionConfig {
            hitl_client: false,
            ..InteractionConfig::default()
        };
        let mut policy = Policy::new(&cfg, Persona::load("default"));
        let ask = HitlRequest {
            kind: HitlKind::Value,
            question: Some("What is your account password?".to_owned()),
            ..confirm(false)
        };
        let board = asking(ask.clone());
        let actions = policy.step(
            1_000,
            task_event(1_000, TaskEventKind::InputRequired(ask)),
            &board,
        );
        assert_eq!(
            words(&actions),
            ["I can't take your password on this call for security reasons."]
        );
        assert!(
            matches!(
                commands(&actions)[..],
                [TaskCmd::Answer {
                    outcome: HitlOutcome::Decline { correction: None },
                    by: AnsweredBy::Policy,
                    ..
                }]
            ),
            "{actions:?}"
        );
        assert!(!policy.answer_expected(1_100));
    }

    #[test]
    fn an_answer_that_asks_for_a_password_is_replaced_by_the_decline_once() {
        let mut policy = policy();
        let board = working();
        let asks = sentence(0, "Please tell me your current password.", Some(900));
        let actions = policy.step(2_000, task_event(2_000, asks), &board);
        assert_eq!(
            words(&actions),
            ["I can't take your password on this call for security reasons."]
        );
        assert_eq!(spoken(&actions)[0].kind, Kind::Notice);
        for (seq, again) in [
            (1, "I need your password to go on."),
            (2, "Please share it with me now."),
        ] {
            let again = sentence(seq, again, None);
            let actions = policy.step(2_100, task_event(2_100, again), &board);
            assert!(spoken(&actions).is_empty(), "{actions:?}");
        }
        // A sentence that only names the category is relayed as it is.
        let names = sentence(3, "Your password was reset.", None);
        let actions = policy.step(2_200, task_event(2_200, names), &board);
        assert_eq!(words(&actions), ["Your password was reset."]);
    }

    // ---- another language

    fn foreign(turn_id: u64, at: Ms) -> PolicyInput {
        PolicyInput::EndOfTurn(EndOfTurn {
            foreign: true,
            ..turn(turn_id, at, "necesito una cita con el doctor para el lunes")
        })
    }

    fn steps_down(actions: &[PolicyAction]) -> bool {
        actions
            .iter()
            .any(|action| matches!(action, PolicyAction::StepDown))
    }

    #[test]
    fn two_turns_in_another_language_step_the_session_down() {
        let mut policy = policy();
        policy.allow_step_down(true);
        let board = Board::default();
        let actions = policy.step(1_000, foreign(1, 1_000), &board);
        assert_eq!(settled(&actions), (Act::Question, false));
        assert_eq!(
            words(&actions),
            ["Sorry, I didn't catch that. Could you say it again?"]
        );
        assert!(commands(&actions).is_empty() && !steps_down(&actions));

        let actions = policy.step(9_000, foreign(2, 9_000), &board);
        assert_eq!(
            words(&actions),
            ["Sorry, I can only help in English on this line."]
        );
        assert!(commands(&actions).is_empty() && steps_down(&actions));

        // From here every turn is the background's: no front, no acknowledgement.
        let actions = policy.step(20_000, foreign(3, 20_000), &board);
        assert!(
            matches!(
                commands(&actions)[..],
                [TaskCmd::Delegate {
                    turn: TurnId(3),
                    adopt: false,
                    ..
                }]
            ),
            "{actions:?}"
        );
        assert!(spoken(&actions).is_empty());
        assert_eq!(policy.next_wakeup(), None);
        // The answer is relayed; the pack's own lines are not said any more.
        let board = working();
        let answer = task_event(21_000, sentence(0, "Claro, un momento.", Some(1_000)));
        let actions = policy.step(21_000, answer, &board);
        assert_eq!(words(&actions), ["Claro, un momento."]);
        let actions = policy.step(60_000, PolicyInput::Tick, &board);
        assert!(spoken(&actions).is_empty(), "{actions:?}");
    }

    /// The front reads the words as another language (`wait` `foreign`; owner, 24 Oct: no
    /// word list does): the same as the recogniser's language field, asked again, then
    /// stepped down.
    #[test]
    fn the_fronts_reading_of_another_language_asks_again_then_steps_down() {
        let board = Board::default();
        let mut policy = policy();
        policy.allow_step_down(true);
        let foreign = ActDecision {
            wait: Some(WaitReason::Foreign),
            ..decision(Act::Wait)
        };
        let spanish = "necesito una cita con el doctor para el lunes";
        let actions = decided(&mut policy, 1, 1_000, spanish, &board, foreign.clone());
        assert_eq!(
            words(&actions),
            ["Sorry, I didn't catch that. Could you say it again?"]
        );
        assert!(commands(&actions).is_empty() && !steps_down(&actions));
        let actions = decided(&mut policy, 2, 9_000, spanish, &board, foreign);
        assert_eq!(
            words(&actions),
            ["Sorry, I can only help in English on this line."]
        );
        assert!(commands(&actions).is_empty() && steps_down(&actions));
    }

    #[test]
    fn an_english_turn_resets_the_count_and_a_session_with_no_plain_path_stays() {
        let board = Board::default();
        let mut policy = policy();
        policy.allow_step_down(true);
        policy.step(1_000, foreign(1, 1_000), &board);
        // The front reads the English turn as a request: the run of other-language turns ends.
        decided(
            &mut policy,
            2,
            5_000,
            REQUEST,
            &board,
            decision(Act::Delegate),
        );
        let actions = policy.step(9_000, foreign(3, 9_000), &board);
        assert!(!steps_down(&actions));

        // Raw `/ws` (its background is `interaction.tasks.llm`): the repeat line, every time.
        let mut raw = self::policy();
        for (turn, at) in [(1, 1_000), (2, 9_000), (3, 17_000)] {
            let actions = raw.step(at, foreign(turn, at), &board);
            assert_eq!(
                words(&actions),
                ["Sorry, I didn't catch that. Could you say it again?"]
            );
            assert!(commands(&actions).is_empty() && !steps_down(&actions));
        }
    }

    #[test]
    fn a_status_line_words_the_callers_request_as_a_phrase() {
        let mut policy = policy();
        let title = "Can you check if my bag is included in the fare";
        let board = Board {
            tasks: vec![view("t1", TaskState::Working, title)],
            ..Board::default()
        };
        let presence = ActDecision {
            intent: Some("presence"),
            ..decision(Act::Progress)
        };
        let actions = decided(
            &mut policy,
            1,
            1_000,
            "are you still there",
            &board,
            presence,
        );
        assert_eq!(
            words(&actions),
            [
                "Yes, I'm here. I'm still working on your request to check if your bag is \
              included in the fare."
            ]
        );
    }
    // ---- chapter 8: the update inbox, slots and the narrator

    const FLIGHT: &str = "Book the morning flight to Delhi on Friday";
    const BOOKED: &str = "Booked AI 101 to Delhi on Friday at 9:40. Seat 14C.";

    /// An event of task `alias`, a child of `t1` when `child`.
    fn event_of(
        alias: &str,
        child: bool,
        at: Ms,
        state: TaskState,
        kind: TaskEventKind,
    ) -> PolicyInput {
        PolicyInput::Task(TaskEvent {
            task: TaskId(format!("task_{alias}")),
            task_ref: TaskRef(alias.to_owned()),
            seq: 1,
            revision: 1,
            at,
            state,
            cancelling: false,
            lineage: Box::new(Lineage {
                parent_id: child.then(|| TaskId("task_t1".to_owned())),
                ..Lineage::default()
            }),
            kind,
        })
    }

    fn completed(text: &str) -> TaskEventKind {
        TaskEventKind::Completed {
            text: text.to_owned(),
            effects: Effects::NoneCommitted,
        }
    }

    /// The caller's request t1 runs on an orchestrator, its children t2 (and t3) on agents.
    fn tree(children: &[(&str, &str)]) -> Board {
        let mut tasks = vec![view("t1", TaskState::Working, "your flight and your test")];
        for (alias, title) in children {
            let child = TaskView {
                lineage: Lineage {
                    parent_id: Some(TaskId("task_t1".to_owned())),
                    ..Lineage::default()
                },
                ..view(alias, TaskState::Completed, title)
            };
            tasks.push(child);
        }
        Board {
            tasks,
            ..Board::default()
        }
    }

    fn narrations(actions: &[PolicyAction]) -> Vec<&Narration> {
        actions
            .iter()
            .filter_map(|action| match action {
                PolicyAction::Narrate(narration) => Some(narration),
                _ => None,
            })
            .collect()
    }

    /// The caller asks a general question at `at` (turn 2, task t9) whose one-sentence answer
    /// is being played: the agent's current answer.
    fn answering(policy: &mut Policy, at: Ms, board: &Board) -> ItemId {
        policy.set_floor(true, false);
        delegate(policy, 2, at, board);
        let created = TaskEventKind::Created {
            title: "the capital of Peru".to_owned(),
            side_effects: false,
            speculative: false,
            backend: "llm".to_owned(),
        };
        policy.step(
            at + 10,
            event_of("t9", false, at + 10, TaskState::Working, created),
            board,
        );
        let answer = policy.step(
            at + 900,
            event_of(
                "t9",
                false,
                at + 900,
                TaskState::Working,
                sentence(0, "It is Lima.", None),
            ),
            board,
        );
        let [lima] = spoken(&answer)[..] else {
            panic!("the answer, got {answer:?}");
        };
        let item = lima.item;
        let started = LaneEvent::Started {
            item,
            utt: UttId(9),
            at: at + 1_000,
        };
        policy.step(at + 1_000, PolicyInput::Lane(started), board);
        item
    }

    #[test]
    fn an_update_waits_for_the_end_of_the_answer_then_is_offered() {
        let mut policy = policy();
        let board = tree(&[("t2", FLIGHT)]);
        let lima = answering(&mut policy, 1_000, &board);
        policy.set_floor(false, false);
        // The flight is booked while the answer plays: nothing is said over it.
        let done = event_of("t2", true, 2_000, TaskState::Completed, completed(BOOKED));
        assert!(spoken(&policy.step(2_000, done, &board)).is_empty());
        let end = event_of(
            "t9",
            false,
            2_100,
            TaskState::Completed,
            completed("It is Lima."),
        );
        let closed = policy.step(2_100, end, &board);
        assert!(
            spoken(&closed).iter().all(|line| line.kind == Kind::Relay),
            "{closed:?}"
        );
        // Heard to its end: the floor is quiet, and the offer is said at once.
        let heard = LaneEvent::Heard {
            item: lima,
            heard_at: 2_600,
        };
        policy.step(2_600, PolicyInput::Lane(heard), &board);
        assert!(policy.waits_for_quiet());
        policy.set_floor(true, false);
        let actions = policy.step(2_600, PolicyInput::Tick, &board);
        let [offer] = spoken(&actions)[..] else {
            panic!("one offer, got {actions:?}");
        };
        assert_eq!((offer.kind, offer.rank), (Kind::Update, RANK_UPDATE));
        assert_eq!(
            words(&actions),
            [
                "I've got the answer on your request to book the morning flight to Delhi on \
              Friday. Want it now?"
            ]
        );
        // Offered once: the next slot says nothing more.
        let item = offer.item;
        let heard = LaneEvent::Heard {
            item,
            heard_at: 4_000,
        };
        assert!(spoken(&policy.step(4_000, PolicyInput::Lane(heard), &board)).is_empty());

        // "Yes" plays it: the narrator words it, as the reply to the turn.
        let yes = policy.step(5_000, says(3, 5_000, "yes please"), &board);
        assert_eq!(settled(&yes).0, Act::Social);
        let [narration] = narrations(&yes)[..] else {
            panic!("one narration, got {yes:?}");
        };
        assert_eq!(
            (narration.title.as_str(), narration.summary.as_str()),
            (FLIGHT, BOOKED)
        );
        let line = "Your Friday flight to Delhi is booked, AI 101 at 9:40.".to_owned();
        let item = narration.item;
        let said = policy.step(
            5_300,
            PolicyInput::Narrated {
                item,
                line: Some(line.clone()),
            },
            &board,
        );
        let [update] = spoken(&said)[..] else {
            panic!("the update, got {said:?}");
        };
        assert_eq!(update.say, Say::Front(line));
        assert_eq!(
            (
                update.kind,
                update.rank,
                update.reply_to,
                update.task.clone()
            ),
            (
                Kind::Update,
                RANK_REPLY,
                Some(TurnId(3)),
                Some((TaskId("task_t2".to_owned()), 1))
            )
        );
        assert!(policy.inbox.is_empty());
    }

    #[test]
    fn two_updates_at_one_slot_become_one_offer() {
        let mut policy = policy();
        let board = tree(&[("t2", FLIGHT), ("t3", "Fix the failing date test")]);
        answering(&mut policy, 1_000, &board);
        for (alias, at, text) in [
            ("t2", 1_500, BOOKED),
            ("t3", 1_700, "Fixed it; 42 tests pass."),
        ] {
            let done = event_of(alias, true, at, TaskState::Completed, completed(text));
            assert!(spoken(&policy.step(at, done, &board)).is_empty());
        }
        let end = event_of(
            "t9",
            false,
            2_100,
            TaskState::Completed,
            completed("It is Lima."),
        );
        let actions = policy.step(2_100, end, &board);
        let offers: Vec<String> = words(&actions)
            .into_iter()
            .filter(|w| w.contains("Want"))
            .collect();
        assert_eq!(
            offers,
            [
                "By the way, 2 updates are in: your request to book the morning flight to Delhi on \
              Friday, your request to fix the failing date test. Want them now?"
            ]
        );
        // "No" keeps them for an idle moment: then both are said, without being asked again.
        let offer = spoken(&actions)
            .iter()
            .find(|line| line.kind == Kind::Update)
            .unwrap()
            .item;
        policy.step(
            3_000,
            PolicyInput::Lane(LaneEvent::Heard {
                item: offer,
                heard_at: 3_000,
            }),
            &board,
        );
        let no = policy.step(4_000, says(3, 4_000, "no thanks"), &board);
        assert_eq!(words(&no), ["Okay."]);
        assert!(narrations(&no).is_empty());
        assert!(narrations(&policy.step(7_900, PolicyInput::Tick, &board)).is_empty());
        assert_eq!(policy.next_wakeup(), Some(8_000));
        let idle = policy.step(8_000, PolicyInput::Tick, &board);
        assert_eq!(narrations(&idle).len(), 2, "{idle:?}");
        assert!(spoken(&idle).is_empty(), "the narrator words them first");
    }

    #[test]
    fn a_spoken_update_never_contains_a_number_the_result_lacks() {
        let mut policy = policy();
        let board = tree(&[("t2", FLIGHT)]);
        // Idle: the update is said without an offer.
        let done = event_of("t2", true, 10_000, TaskState::Completed, completed(BOOKED));
        let actions = policy.step(10_000, done, &board);
        let [narration] = narrations(&actions)[..] else {
            panic!("narrated at once, got {actions:?}");
        };
        // A time the result does not have: nothing is said (no template, chapter 9); the
        // update waits for its next slot.
        let item = narration.item;
        let wrong = Some("Your flight to Delhi leaves at 10:15 on Friday.".to_owned());
        let said = policy.step(10_300, PolicyInput::Narrated { item, line: wrong }, &board);
        assert!(spoken(&said).is_empty(), "{said:?}");
        for (line, rule) in [
            (
                "Your flight on IndiGo to Delhi is booked.",
                "name_not_given",
            ),
            ("Booked.", "narration_length"),
            (
                "Your flight is booked. Seat 14C is yours.",
                "narration_sentences",
            ),
        ] {
            let source = format!("{FLIGHT} {BOOKED}");
            let verdict = guard::narration(line, &source);
            let expected = if rule == "narration_length" {
                GuardOutcome::Pass
            } else {
                GuardOutcome::Blocked(rule)
            };
            assert_eq!(verdict, expected, "{line}");
        }
        // A line in time, and a screen attached: it points at the screen.
        let mut screen = self::policy();
        screen.set_floor(true, true);
        let done = event_of("t2", true, 10_000, TaskState::Completed, completed(BOOKED));
        let item = narrations(&screen.step(10_000, done, &board))[0].item;
        let line = Some("Your Delhi flight is booked.".to_owned());
        let said = screen.step(10_400, PolicyInput::Narrated { item, line }, &board);
        let words = words(&said);
        assert!(
            words[0].starts_with("Your Delhi flight is booked."),
            "{words:?}"
        );
    }

    /// Live (launch demo take 15, 27 Oct): the terminal task's 22-word line was refused for
    /// its length on all three tries, the same line each time, and the update was never said.
    #[test]
    fn a_long_update_is_cut_at_a_clause_and_a_refused_one_is_retried_with_its_reason() {
        let counts = "I got the counts with no errors: the x86 Linux build has zero downloads, \
                      the ARM Linux build twelve, and the checksums file fourteen, so the builds \
                      total twelve.";
        let title = "Pull the download numbers: Run curl -s";
        let board = tree(&[("t2", title)]);
        let mut policy = policy();
        let done = event_of("t2", true, 10_000, TaskState::Completed, completed(counts));
        let first = policy.step(10_000, done, &board);
        let [narration] = narrations(&first)[..] else {
            panic!("narrated at once");
        };
        assert_eq!(narration.retry, None);
        let item = narration.item;
        let line = Some(
            "The x86 Linux build has zero downloads, the ARM Linux build twelve, and the \
             checksums file fourteen, so the builds total twelve."
                .to_owned(),
        );
        let said = policy.step(10_300, PolicyInput::Narrated { item, line }, &board);
        assert_eq!(
            words(&said),
            [
                "The x86 Linux build has zero downloads, the ARM Linux build twelve, and the \
              checksums file fourteen."
            ]
        );

        // A line that cannot be cut is refused; at the next slot the narrator is told why.
        let mut policy = self::policy();
        let done = event_of("t2", true, 10_000, TaskState::Completed, completed(counts));
        let item = narrations(&policy.step(10_000, done, &board))[0].item;
        let line = Some(
            "The x86 Linux build and the ARM Linux build and the checksums file were all \
             downloaded twelve or fourteen or zero times in total today."
                .to_owned(),
        );
        let again = policy.step(10_300, PolicyInput::Narrated { item, line }, &board);
        assert!(spoken(&again).is_empty(), "{again:?}");
        // The conversation is idle: this is the next slot.
        let [retry] = narrations(&again)[..] else {
            panic!("asked again at the next slot, got {again:?}");
        };
        assert_eq!(retry.retry, Some("narration_length"));
        // No line at all is no reason to tell.
        let item = retry.item;
        let third = policy.step(10_600, PolicyInput::Narrated { item, line: None }, &board);
        assert_eq!(narrations(&third)[0].retry, None);
    }

    /// Live (launch demo take 15): for the browser task's update the narrator copied the
    /// writer task's update, the call's last line, and it was said again as the browser
    /// task's (64.4 s). An update must say its own result, and nothing the agent just said.
    #[test]
    fn an_update_that_repeats_a_recent_line_or_lacks_its_result_is_not_said() {
        let release = "Version one point one point oh is published as Latest; highlights include \
                       interaction mode, eleven more voice vendors, and SDK fixes.";
        let writer = "The launch announcement has been saved with highlights from the release \
                      notes instead of a placeholder.";
        let board = tree(&[("t2", "Check the GitHub release: Open")]);
        let mut policy = policy();
        policy.set_speech(
            0,
            Vec::new(),
            vec![(9_000, format!("{writer} The details are on your screen."))],
        );
        let done = event_of("t2", true, 10_000, TaskState::Completed, completed(release));
        let item = narrations(&policy.step(10_000, done, &board))[0].item;
        // The copy passes the fact check ("highlights" is in both results) but repeats.
        let copy = Some(writer.to_owned());
        let again = policy.step(10_300, PolicyInput::Narrated { item, line: copy }, &board);
        assert!(spoken(&again).is_empty(), "{again:?}");
        let [retry] = narrations(&again)[..] else {
            panic!("asked again");
        };
        assert_eq!(retry.retry, Some("repeated"));
        // A line about the task instead of its result says nothing of it either.
        let item = retry.item;
        let about = Some("I checked the GitHub release for you.".to_owned());
        let third = policy.step(10_600, PolicyInput::Narrated { item, line: about }, &board);
        assert!(spoken(&third).is_empty(), "{third:?}");
        let [retry] = narrations(&third)[..] else {
            panic!("asked a third time");
        };
        assert_eq!(retry.retry, Some("no_result"));
        // Its own result is said.
        let item = retry.item;
        let own = "Version one point one point oh is published as Latest, with SDK fixes.";
        let line = Some(own.to_owned());
        let said = policy.step(10_900, PolicyInput::Narrated { item, line }, &board);
        assert_eq!(words(&said), [own]);
        assert!(!super::carries_result(
            "I checked the GitHub release for you.",
            release,
            "Check the GitHub release: Open"
        ));
    }

    #[test]
    fn tell_screen_and_the_callers_own_word_decide_how_an_update_is_said() {
        let board = tree(&[("t2", FLIGHT)]);
        let finish = |policy: &mut Policy| {
            answering(policy, 1_000, &board);
            let done = event_of("t2", true, 1_500, TaskState::Completed, completed(BOOKED));
            policy.step(1_500, done, &board);
            let end = event_of(
                "t9",
                false,
                2_100,
                TaskState::Completed,
                completed("It is Lima."),
            );
            policy.step(2_100, end, &board)
        };
        let with = |announce: Announce| {
            let mut cfg = InteractionConfig::default();
            cfg.tasks.announce = announce;
            Policy::new(&cfg, Persona::load("default"))
        };
        // `tell`: the update itself at the end of the answer.
        let told = finish(&mut with(Announce::Tell));
        assert_eq!(narrations(&told).len(), 1);
        // `screen`: nothing, not even when the conversation is idle.
        let mut quiet = with(Announce::Screen);
        let none = finish(&mut quiet);
        assert!(
            narrations(&none).is_empty() && spoken(&none).iter().all(|l| l.kind == Kind::Relay)
        );
        assert!(narrations(&quiet.step(30_000, PolicyInput::Tick, &board)).is_empty());
        // The caller's word for the request wins over `screen`, for its children too.
        let mut asked = with(Announce::Screen);
        asked.step(
            500,
            says(1, 500, "tell me as soon as the flight is booked"),
            &board,
        );
        let notify = FrontEvent::Notify {
            turn: TurnId(1),
            task: TaskRef("t1".to_owned()),
            announce: Announce::Tell,
        };
        asked.step(600, PolicyInput::Front(notify), &board);
        asked.step(600, done(1, Act::Progress), &board);
        assert_eq!(narrations(&finish(&mut asked)).len(), 1);
    }

    #[test]
    fn a_result_the_client_showed_is_not_said_and_the_caller_is_never_talked_over() {
        let board = tree(&[("t2", FLIGHT)]);
        let mut policy = policy();
        policy.step(9_000, PolicyInput::UserSpeaking { on: true }, &board);
        let done = event_of("t2", true, 10_000, TaskState::Completed, completed(BOOKED));
        assert!(narrations(&policy.step(10_000, done, &board)).is_empty());
        let seen = PolicyInput::TaskSeen {
            task: TaskId("task_t2".to_owned()),
        };
        policy.step(10_500, seen, &board);
        policy.step(11_000, PolicyInput::UserSpeaking { on: false }, &board);
        assert!(narrations(&policy.step(16_000, PolicyInput::Tick, &board)).is_empty());
        assert!(policy.inbox.is_empty());
    }

    #[test]
    fn an_update_cut_before_it_was_heard_waits_again() {
        let board = tree(&[("t2", FLIGHT)]);
        let mut policy = policy();
        let done = event_of("t2", true, 10_000, TaskState::Completed, completed(BOOKED));
        let item = narrations(&policy.step(10_000, done, &board))[0].item;
        let line = Some("Your Delhi flight is booked.".to_owned());
        let said = policy.step(10_100, PolicyInput::Narrated { item, line }, &board);
        let item = spoken(&said)[0].item;
        // The caller started to speak before it played.
        policy.step(10_150, PolicyInput::UserSpeaking { on: true }, &board);
        let returned = LaneEvent::Returned { item };
        policy.step(10_200, PolicyInput::Lane(returned), &board);
        policy.step(10_300, says(2, 10_300, "hold on"), &board);
        assert_eq!(policy.inbox.len(), 1);
        assert!(policy.inbox[0].offered);
    }

    // ---- deadline questions, follow-ups and hesitant requests (chapter 8)

    /// The child t2's question with a deadline `expires_at`, `firm` when it is the job's own.
    fn seat_question(expires_at: Ms, firm: bool) -> HitlRequest {
        HitlRequest {
            id: HitlId("q_task_t2_1".to_owned()),
            task: TaskId("task_t2".to_owned()),
            revision: 1,
            kind: HitlKind::Choose,
            question: Some("Window or aisle?".to_owned()),
            options: vec!["window".to_owned(), "aisle".to_owned()],
            expires_at,
            firm,
            ..HitlRequest::default()
        }
    }

    /// t1's flight child t2 is working and asks `request`.
    fn seat_asked(request: &HitlRequest) -> Board {
        let mut board = tree(&[]);
        let child = TaskView {
            lineage: Lineage {
                parent_id: Some(TaskId("task_t1".to_owned())),
                agent: Some("browser".to_owned()),
                ..Lineage::default()
            },
            ..view("t2", TaskState::InputRequired, FLIGHT)
        };
        board.tasks.push(child);
        board.open_question = Some(request.clone());
        board
    }

    fn question_event(at: Ms, request: &HitlRequest) -> PolicyInput {
        let kind = TaskEventKind::InputRequired(request.clone());
        event_of("t2", true, at, TaskState::InputRequired, kind)
    }

    fn resolved(at: Ms, request: &HitlRequest, outcome: HitlOutcome) -> PolicyInput {
        let kind = TaskEventKind::InputResolved {
            request: request.id.clone(),
            outcome,
            by: AnsweredBy::Voice,
            redacted: false,
        };
        event_of("t2", true, at, TaskState::Working, kind)
    }

    #[test]
    fn an_urgent_question_cuts_at_a_clause_and_the_answer_resumes() {
        let mut policy = policy();
        let request = seat_question(2_000 + 8_000, true);
        let board = seat_asked(&request);
        let lima = answering(&mut policy, 1_000, &board);
        // Six seconds of the answer are still to be heard: an 8 s deadline cannot wait for
        // the slot after it. The question breaks in, at the urgent rank, with its lead-in.
        policy.set_floor(false, false);
        policy.set_speech(6_000, vec![lima], Vec::new());
        let actions = policy.step(2_000, question_event(2_000, &request), &board);
        let lines = spoken(&actions);
        assert_eq!(lines.len(), 2, "{actions:?}");
        assert!(lines.iter().all(|line| line.rank == RANK_URGENT));
        assert!(
            lines
                .iter()
                .all(|line| line.question == Some(request.id.clone()))
        );
        assert_eq!(words(&actions)[0], "Quick question.");
        assert!(words(&actions)[1].contains("Window or aisle?"));
        // A firm deadline is never moved by hearing the question.
        let heard = LaneEvent::Heard {
            item: lines[1].item,
            heard_at: 4_000,
        };
        policy.step(4_000, PolicyInput::Lane(heard), &board);
        assert_eq!(
            policy.asking.as_ref().map(|asking| asking.deadline),
            Some(10_000)
        );
        // Answered: the interrupted answer resumes with the persona's "Anyway,".
        let accepted = HitlOutcome::Accept {
            value: Some(serde_json::json!("window")),
        };
        let after = policy.step(5_000, resolved(5_000, &request, accepted), &board);
        let resume = spoken(&after);
        let [anyway] = resume[..] else {
            panic!("the resume line, got {after:?}");
        };
        assert_eq!(words(&after), ["Anyway,"]);
        assert_eq!((anyway.rank, anyway.lane), (RANK_QUESTION, Lane::Main));
    }

    /// Live MA2: with 5 s left the 1.2 s lead-in made the caller's answer late; the question
    /// breaks in on its own.
    #[test]
    fn an_urgent_question_with_little_time_left_breaks_in_without_its_lead_in() {
        let mut policy = policy();
        let request = seat_question(2_000 + 5_000, true);
        let board = seat_asked(&request);
        let lima = answering(&mut policy, 1_000, &board);
        policy.set_floor(false, false);
        policy.set_speech(3_000, vec![lima], Vec::new());
        let actions = policy.step(2_000, question_event(2_000, &request), &board);
        let lines = spoken(&actions);
        assert_eq!(lines.len(), 1, "{actions:?}");
        assert_eq!(lines[0].rank, RANK_URGENT);
        assert!(words(&actions)[0].contains("Window or aisle?"));
        assert!(policy.asking.as_ref().is_some_and(|asking| asking.urgent));
        // Its answer's reading is known but for the words: the front primes it meanwhile.
        let reading = policy.open_reading().expect("a spoken question's reading");
        assert!(reading.context.contains("Window or aisle?"), "{reading:?}");
        assert_eq!(reading.kind, ReadingKind::Reply);
        let labels: Vec<&str> = reading
            .labels
            .iter()
            .map(|(label, _)| label.as_str())
            .collect();
        assert_eq!(&labels[..2], ["1", "2"]);
        assert_eq!(reading.labels[0].1, "window");
    }

    #[test]
    fn an_orchestrators_session_makes_the_break_in_lines_ahead() {
        let warm = |cfg: &InteractionConfig| {
            let mut policy = Policy::new(cfg, Persona::load("default"));
            let actions = policy.step(0, PolicyInput::Ready, &Board::default());
            assert!(spoken(&actions).iter().all(|line| line.discard));
            words(&actions)
        };
        let plain = warm(&InteractionConfig::default());
        let mut cfg = InteractionConfig::default();
        cfg.tasks.orchestrator = Some(serde_json::json!("big"));
        cfg.tasks.agents = vec!["browser".to_owned()];
        let orchestrated = warm(&cfg);
        assert!(!plain.iter().any(|line| line == "Anyway,"));
        assert_eq!(orchestrated.len(), plain.len() + 2);
        // First: warm-ups stop once a task runs (live MA2).
        assert!(orchestrated.starts_with(&["Quick question.".to_owned(), "Anyway,".to_owned()]));
    }

    #[test]
    fn a_question_that_can_wait_is_asked_at_the_next_slot() {
        let mut policy = policy();
        let request = seat_question(2_000 + 20_000, false);
        let board = seat_asked(&request);
        let lima = answering(&mut policy, 1_000, &board);
        policy.set_floor(false, false);
        policy.set_speech(1_000, vec![lima], Vec::new());
        let actions = policy.step(2_000, question_event(2_000, &request), &board);
        assert!(spoken(&actions).is_empty(), "{actions:?}");
        // The floor and the turn rules do not see it yet; a quiet floor brings a look.
        assert_eq!(policy.shown(board.clone()).open_question, None);
        assert!(policy.waits_for_quiet());
        let end = event_of(
            "t9",
            false,
            2_100,
            TaskState::Completed,
            completed("It is Lima."),
        );
        policy.step(2_100, end, &board);
        let heard = LaneEvent::Heard {
            item: lima,
            heard_at: 2_600,
        };
        policy.step(2_600, PolicyInput::Lane(heard), &board);
        policy.set_floor(true, false);
        policy.set_speech(0, Vec::new(), Vec::new());
        let actions = policy.step(2_600, PolicyInput::Tick, &board);
        let lines = spoken(&actions);
        assert_eq!(lines.len(), 1, "no lead-in at a slot: {actions:?}");
        assert_eq!(
            (lines[0].kind, lines[0].rank),
            (Kind::Question, RANK_QUESTION)
        );
        assert_eq!(policy.shown(board.clone()).open_question, Some(request));
    }

    #[test]
    fn a_question_nobody_answered_in_time_is_said_at_the_next_slot() {
        let mut policy = policy();
        let request = seat_question(2_000 + 5_000, true);
        let board = seat_asked(&request);
        let lima = answering(&mut policy, 1_000, &board);
        // The caller speaks the whole time: it never breaks in over them, and expires.
        policy.set_floor(false, false);
        policy.set_speech(0, vec![lima], Vec::new());
        policy.step(1_900, PolicyInput::UserSpeaking { on: true }, &board);
        let actions = policy.step(2_000, question_event(2_000, &request), &board);
        let lines = spoken(&actions);
        assert!(
            lines.iter().all(|line| line.rank == RANK_URGENT),
            "{actions:?}"
        );
        policy.asking = None;
        policy.deferred = Some(request.clone());
        let expired = policy.step(7_000, PolicyInput::Tick, &board);
        assert!(
            commands(&expired).iter().any(|cmd| matches!(
                cmd,
                TaskCmd::Answer {
                    outcome: HitlOutcome::Timeout,
                    ..
                }
            )),
            "{expired:?}"
        );
        let closed = Board {
            open_question: None,
            ..board.clone()
        };
        policy.step(
            7_000,
            resolved(7_000, &request, HitlOutcome::Timeout),
            &closed,
        );
        assert_eq!(policy.inbox.len(), 1);
        // At the next slot (the caller done, the answer heard): one line.
        policy.step(7_100, PolicyInput::UserSpeaking { on: false }, &closed);
        let end = event_of(
            "t9",
            false,
            7_200,
            TaskState::Completed,
            completed("It is Lima."),
        );
        policy.step(7_200, end, &closed);
        policy.set_floor(true, false);
        let said = policy.step(7_300, PolicyInput::Tick, &closed);
        assert_eq!(
            words(&said),
            [
                "A question about your request to book the morning flight to Delhi on Friday \
              could not wait for your answer, so it went on without it."
            ]
        );
    }

    /// t1's flight child t2 completed on the browser agent, with `BOOKED` as its result.
    fn booked(policy: &mut Policy) -> Board {
        let mut board = tree(&[]);
        board.tasks[0].state = TaskState::Completed;
        let child = TaskView {
            lineage: Lineage {
                parent_id: Some(TaskId("task_t1".to_owned())),
                agent: Some("browser".to_owned()),
                ..Lineage::default()
            },
            side_effects: true,
            ..view("t2", TaskState::Completed, FLIGHT)
        };
        board.tasks.push(child);
        policy.step(
            1_000,
            event_of("t2", true, 1_000, TaskState::Completed, completed(BOOKED)),
            &board,
        );
        policy.inbox.clear();
        board
    }

    /// Owner, 26 Oct (live PR2/PR8): a request the front delegated while a task had just
    /// started is read apart against it; about it, it updates that task, otherwise (or with no
    /// reading in time) a new task starts. A task older than that is not read against.
    #[test]
    fn a_request_delegated_beside_a_task_just_started_is_read_against_it() {
        let delegate = ActDecision {
            source: ActSource::Model,
            ..decision(Act::Delegate)
        };
        let parcel = |started_at: Ms| Board {
            tasks: vec![TaskView {
                started_at,
                ..view("t1", TaskState::Working, "Where is my parcel")
            }],
            ..Board::default()
        };
        let words = "Can you check its delivery status?";
        for (label, act) in [
            (Some("1"), Act::Update),
            (Some("new"), Act::Delegate),
            (None, Act::Delegate),
        ] {
            let mut policy = policy();
            policy.reader = None;
            let board = parcel(2_000);
            let actions = decided(&mut policy, 1, 4_000, words, &board, delegate.clone());
            let [PolicyAction::Read(reading)] = &actions[..] else {
                panic!("a reading, got {actions:?}");
            };
            assert_eq!(reading.kind, ReadingKind::Started);
            assert_eq!(
                reading.context,
                "Just asked: 1: \u{ab}Where is my parcel\u{bb}"
            );
            assert_eq!(reading.said, words);
            let read = PolicyInput::Read {
                turn: TurnId(1),
                label: label.map(str::to_owned),
            };
            let actions = policy.step(4_050, read, &board);
            assert_eq!(settled(&actions).0, act, "{label:?}: {actions:?}");
            let started = commands(&actions)
                .iter()
                .any(|cmd| matches!(cmd, TaskCmd::Delegate { .. }));
            assert_eq!(started, act == Act::Delegate, "{actions:?}");
            if act == Act::Update {
                assert!(
                    commands(&actions).iter().any(|cmd| matches!(
                        cmd,
                        TaskCmd::Revise { task, .. } | TaskCmd::Continue { task, .. } if task.0 == "t1"
                    )),
                    "{actions:?}"
                );
            }
        }
        // The decision's line comes while the turn is read: it is the update's acknowledgement.
        let mut policy = policy();
        policy.reader = None;
        let board = parcel(2_000);
        policy.step(4_000, says(1, 4_000, words), &board);
        let known = FrontEvent::ActKnown {
            turn: TurnId(1),
            act: Act::Delegate,
            task: None,
            source: ActSource::Model,
        };
        let asked = policy.step(4_000, PolicyInput::Front(known), &board);
        assert!(matches!(asked[..], [PolicyAction::Read(_)]), "{asked:?}");
        let line = "Let me check its delivery status.";
        let done = FrontEvent::Done {
            turn: TurnId(1),
            decision: ActDecision {
                say: Some(line.to_owned()),
                ..delegate.clone()
            },
        };
        assert!(
            policy
                .step(4_020, PolicyInput::Front(done), &board)
                .is_empty()
        );
        let read = PolicyInput::Read {
            turn: TurnId(1),
            label: Some("1".to_owned()),
        };
        let actions = policy.step(4_050, read, &board);
        assert_eq!(settled(&actions).0, Act::Update);
        assert_eq!(self::words(&actions), [line], "{actions:?}");
        // About a task that completed while the words were read: its answer answers them.
        let mut policy = self::policy();
        policy.reader = None;
        let board = parcel(2_000);
        decided(&mut policy, 1, 4_000, words, &board, delegate.clone());
        let mut done = parcel(2_000);
        done.tasks[0].state = TaskState::Completed;
        let read = PolicyInput::Read {
            turn: TurnId(1),
            label: Some("1".to_owned()),
        };
        let actions = policy.step(4_050, read, &done);
        assert_eq!(settled(&actions).0, Act::Wait, "{actions:?}");
        assert!(commands(&actions).is_empty(), "{actions:?}");
        // ... and one that completed after the turn ended, before its decision (live PR8,
        // r17-verify): the front was shown it working, so the words are still read against it.
        let mut policy = self::policy();
        policy.reader = None;
        policy.step(4_000, says(1, 4_000, words), &parcel(2_000));
        let decision = FrontEvent::Done {
            turn: TurnId(1),
            decision: delegate.clone(),
        };
        let asked = policy.step(4_030, PolicyInput::Front(decision), &done);
        assert!(matches!(asked[..], [PolicyAction::Read(_)]), "{asked:?}");
        let read = PolicyInput::Read {
            turn: TurnId(1),
            label: Some("1".to_owned()),
        };
        let actions = policy.step(4_050, read, &done);
        assert_eq!(settled(&actions).0, Act::Wait, "{actions:?}");
        assert!(commands(&actions).is_empty(), "{actions:?}");
        // A task that had completed before the turn ended is not read against.
        let mut policy = self::policy();
        policy.reader = None;
        let actions = decided(&mut policy, 1, 4_000, words, &done, delegate.clone());
        assert_eq!(settled(&actions).0, Act::Delegate, "{actions:?}");
        // Ten seconds on the task is no longer just started: the front's delegate stands.
        let mut policy = self::policy();
        policy.reader = None;
        let board = parcel(2_000);
        let actions = decided(&mut policy, 1, 12_000, words, &board, delegate);
        assert_eq!(settled(&actions).0, Act::Delegate, "{actions:?}");
    }

    /// Live take 23 of the launch demo (28 Oct): "Great. Can you also get the download numbers
    /// in the terminal?" after the browser's release check was a follow-up of it (the reading
    /// apart had no label in time). Words that name another agent of the session are a new
    /// request, not read apart.
    #[test]
    fn a_follow_up_that_names_another_agent_is_a_new_request() {
        let modelled = |turn: u64| {
            PolicyInput::Front(FrontEvent::Done {
                turn: TurnId(turn),
                decision: ActDecision {
                    task: Some(TaskRef("t2".to_owned())),
                    say: Some("Okay, let me check.".to_owned()),
                    source: ActSource::Model,
                    ..decision(Act::FollowUp)
                },
            })
        };
        let topics = vec![
            (
                "browser".to_owned(),
                "Operates airline websites and books flights.".to_owned(),
            ),
            (
                "terminal".to_owned(),
                "Runs shell commands: downloads and verifies files.".to_owned(),
            ),
        ];
        let mut policy = policy();
        let board = booked(&mut policy);
        policy.reader = None;
        policy.set_agent_topics(topics.clone());
        let words = "Great. Can you also get the download numbers in the terminal?";
        policy.step(10_000, says(3, 10_000, words), &board);
        let actions = policy.step(10_000, modelled(3), &board);
        assert!(
            !actions
                .iter()
                .any(|action| matches!(action, PolicyAction::Read(_))),
            "{actions:?}"
        );
        assert_eq!(settled(&actions).0, Act::Delegate, "{actions:?}");
        assert!(
            commands(&actions)
                .iter()
                .any(|cmd| matches!(cmd, TaskCmd::Delegate { .. })),
            "{actions:?}"
        );
        // A question about the finished task itself is still read apart.
        let mut policy = self::policy();
        let board = booked(&mut policy);
        policy.reader = None;
        policy.set_agent_topics(topics);
        policy.step(20_000, says(4, 20_000, "Which seat did I get?"), &board);
        let actions = policy.step(20_000, modelled(4), &board);
        assert!(
            matches!(actions[..], [PolicyAction::Read(_)]),
            "{actions:?}"
        );
    }

    /// Live rehearsal 1 of the launch demo (28 Oct): after the browser's release check every
    /// turn was a follow-up of it, "Thanks, Eva." too, answered from its result. The reading
    /// apart is shown what the agent that did the task does, and words that ask for nothing
    /// are the persona's thanks line.
    #[test]
    fn a_follow_up_is_read_against_its_agent_and_thanks_are_thanks() {
        let modelled = |turn: u64| {
            PolicyInput::Front(FrontEvent::Done {
                turn: TurnId(turn),
                decision: ActDecision {
                    task: Some(TaskRef("t2".to_owned())),
                    say: Some("Okay, let me check.".to_owned()),
                    source: ActSource::Model,
                    ..decision(Act::FollowUp)
                },
            })
        };
        let mut policy = policy();
        let board = booked(&mut policy);
        policy.reader = None;
        policy.set_agent_topics(vec![(
            "browser".to_owned(),
            "Operates airline websites and books flights.".to_owned(),
        )]);
        policy.step(10_000, says(3, 10_000, "Thanks, Eva."), &board);
        let asked = policy.step(10_000, modelled(3), &board);
        let [PolicyAction::Read(reading)] = &asked[..] else {
            panic!("a reading, got {asked:?}");
        };
        assert_eq!(reading.kind, ReadingKind::Follows);
        assert!(
            reading.context.ends_with(
                "by an agent that \u{ab}Operates airline websites and books flights.\u{bb}"
            ),
            "{}",
            reading.context
        );
        let labels: Vec<&str> = reading.labels.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(labels, ["1", "new", "thanks"]);
        let thanks = PolicyInput::Read {
            turn: TurnId(3),
            label: Some("thanks".to_owned()),
        };
        let actions = policy.step(10_100, thanks, &board);
        assert_eq!(settled(&actions).0, Act::Social, "{actions:?}");
        assert_eq!(words(&actions), ["You're welcome."]);
        assert!(
            narrations(&actions).is_empty(),
            "nothing is answered from the result"
        );
        assert!(
            commands(&actions)
                .iter()
                .all(|cmd| !matches!(cmd, TaskCmd::FollowUp { .. } | TaskCmd::Delegate { .. })),
            "{actions:?}"
        );
        // Without a description of the agent the title alone is shown, as before.
        let mut policy = self::policy();
        let board = booked(&mut policy);
        policy.reader = None;
        policy.step(20_000, says(4, 20_000, "When does it leave?"), &board);
        let asked = policy.step(20_000, modelled(4), &board);
        let [PolicyAction::Read(reading)] = &asked[..] else {
            panic!("a reading, got {asked:?}");
        };
        assert!(
            !reading.context.contains("by an agent"),
            "{}",
            reading.context
        );
    }

    /// Live MA7: a follow-up the front named is read apart against the finished task's title;
    /// about something else it is a new request, and with no reading in time it stands.
    #[test]
    fn a_follow_up_about_something_else_is_a_new_request() {
        let modelled = |turn: u64| {
            PolicyInput::Front(FrontEvent::Done {
                turn: TurnId(turn),
                decision: ActDecision {
                    task: Some(TaskRef("t2".to_owned())),
                    source: ActSource::Model,
                    ..decision(Act::FollowUp)
                },
            })
        };
        let mut policy = policy();
        let board = booked(&mut policy);
        policy.reader = None;
        policy.step(
            10_000,
            says(3, 10_000, "Do I have a note about the dentist?"),
            &board,
        );
        let asked = policy.step(10_000, modelled(3), &board);
        let [PolicyAction::Read(reading)] = &asked[..] else {
            panic!("a reading, got {asked:?}");
        };
        assert_eq!(reading.kind, ReadingKind::Follows);
        assert!(reading.context.contains(FLIGHT), "{}", reading.context);
        let new = PolicyInput::Read {
            turn: TurnId(3),
            label: Some("new".to_owned()),
        };
        let actions = policy.step(10_100, new, &board);
        assert_eq!(settled(&actions).0, Act::Delegate);
        assert!(
            commands(&actions)
                .iter()
                .any(|cmd| matches!(cmd, TaskCmd::Delegate { .. })),
            "{actions:?}"
        );

        let mut policy = self::policy();
        let board = booked(&mut policy);
        policy.reader = None;
        policy.step(20_000, says(4, 20_000, "When does it leave?"), &board);
        policy.step(20_000, modelled(4), &board);
        let none = PolicyInput::Read {
            turn: TurnId(4),
            label: None,
        };
        let actions = policy.step(20_100, none, &board);
        assert_eq!(settled(&actions).0, Act::FollowUp);

        // A task that was one MCP tool call has nothing to go on with: a new request, unread.
        let mut policy = self::policy();
        let board = booked(&mut policy);
        policy.reader = None;
        policy.set_single_calls(vec!["browser".to_owned()]);
        policy.step(
            30_000,
            says(5, 30_000, "Delete my note called Old Shopping List."),
            &board,
        );
        let actions = policy.step(30_000, modelled(5), &board);
        assert_eq!(settled(&actions).0, Act::Delegate, "{actions:?}");
    }

    #[test]
    fn a_follow_up_question_is_answered_from_the_stored_result() {
        let mut policy = policy();
        let board = booked(&mut policy);
        // The front reads it as a follow-up of the flight.
        policy.step(
            10_000,
            says(3, 10_000, "When does the Delhi flight leave?"),
            &board,
        );
        let asked = policy.step(10_000, done_on(3, Act::FollowUp, "t2"), &board);
        assert_eq!(settled(&asked).0, Act::FollowUp);
        let [narration] = narrations(&asked)[..] else {
            panic!("the narrator answers it, got {asked:?}");
        };
        assert_eq!(narration.summary, BOOKED);
        assert_eq!(
            narration.question.as_deref(),
            Some("When does the Delhi flight leave?")
        );
        assert!(
            commands(&asked)
                .iter()
                .all(|cmd| !matches!(cmd, TaskCmd::FollowUp { .. }))
        );
        let item = narration.item;
        let line = Some("It leaves on Friday at 9:40.".to_owned());
        let answered = policy.step(10_400, PolicyInput::Narrated { item, line }, &board);
        let [answer] = spoken(&answered)[..] else {
            panic!("the answer, got {answered:?}");
        };
        assert_eq!(
            (answer.kind, answer.reply_to),
            (Kind::Answer, Some(TurnId(3)))
        );
        assert!(commands(&answered).is_empty(), "no new task");

        // A line about the result instead of from it is no answer either.
        assert!(!super::carries_result(
            "The result does not answer the question.",
            BOOKED,
            "Can it be a window seat?"
        ));
        assert!(super::carries_result(
            "It leaves on Friday at 9:40.",
            BOOKED,
            "When does the Delhi flight leave?"
        ));
        // A line with a time the result lacks is not said: the agent is asked instead.
        policy.step(
            20_000,
            says(4, 20_000, "When does the Delhi flight land?"),
            &board,
        );
        let asked = policy.step(20_000, done_on(4, Act::FollowUp, "t2"), &board);
        let item = narrations(&asked)[0].item;
        let line = Some("It lands at 11:50.".to_owned());
        let handed = policy.step(20_400, PolicyInput::Narrated { item, line }, &board);
        let answers = spoken(&handed);
        assert!(
            answers.iter().all(|line| line.kind != Kind::Answer),
            "{handed:?}"
        );
        let [TaskCmd::FollowUp { task, text, .. }] = commands(&handed)[..] else {
            panic!("a follow-up task, got {handed:?}");
        };
        assert_eq!(
            (task.0.as_str(), text.as_str()),
            ("t2", "When does the Delhi flight land?")
        );
    }

    #[test]
    fn a_follow_up_action_continues_the_same_agent_context() {
        let mut policy = policy();
        let board = booked(&mut policy);
        // The update was heard: "can it be a window seat?" points back at it.
        policy.last_update = Some((TaskId("task_t2".to_owned()), 2_000));
        policy.step(
            10_000,
            says(3, 10_000, "Book a hotel near it as well"),
            &board,
        );
        // The front names it a follow-up; the update heard last is the task it is about.
        let action = policy.step(10_000, done(3, Act::FollowUp), &board);
        assert_eq!(settled(&action).0, Act::FollowUp);
        // The result is asked first; it does not answer this, so the agent gets it.
        let item = narrations(&action)[0].item;
        let action = policy.step(10_400, PolicyInput::Narrated { item, line: None }, &board);
        let [TaskCmd::FollowUp { task, .. }] = commands(&action)[..] else {
            panic!("a follow-up task, got {action:?}");
        };
        assert_eq!(task.0, "t2");
        // A request about nothing finished is a request of its own.
        policy.step(
            30_000,
            says(4, 30_000, "What is the capital of Peru?"),
            &board,
        );
        let other = policy.step(30_000, done(4, Act::Delegate), &board);
        assert_eq!(settled(&other).0, Act::Delegate);

        // Cancelling what is done: a compensating task, after a yes.
        let mut policy =
            super::Policy::new(&InteractionConfig::default(), Persona::load("default"));
        let board = booked(&mut policy);
        policy.step(40_000, says(5, 40_000, "cancel the flight"), &board);
        let offer = policy.step(40_000, done(5, Act::Abort), &board);
        let [ask] = spoken(&offer)[..] else {
            panic!("the offer, got {offer:?}");
        };
        assert!(words(&offer)[0].ends_with("Do you want me to ask for it to be undone?"));
        assert!(commands(&offer).is_empty());
        let item = ask.item;
        let heard = LaneEvent::Heard {
            item,
            heard_at: 41_000,
        };
        policy.step(41_000, PolicyInput::Lane(heard), &board);
        let yes = policy.step(42_000, says(6, 42_000, "yes please"), &board);
        let [TaskCmd::FollowUp { task, text, .. }] = commands(&yes)[..] else {
            panic!("the compensating task, got {yes:?}");
        };
        assert_eq!(
            (task.0.as_str(), text.as_str()),
            ("t2", "cancel the flight")
        );
    }

    #[test]
    fn a_hesitant_request_revises_the_provisional_task() {
        let mut policy = policy();
        let mut board = working();
        policy.step(
            1_000,
            says(1, 1_000, "I need a pet friendly apartment in Miami"),
            &board,
        );
        policy.step(1_000, done(1, Act::Delegate), &board);
        let created = TaskEventKind::Created {
            title: "a pet friendly apartment".to_owned(),
            side_effects: false,
            speculative: false,
            backend: "llm".to_owned(),
        };
        policy.step(1_010, task_event(1_010, created), &board);
        // The caller goes on 200 ms after the end of the turn.
        let more = EndOfTurn {
            onset_at: Some(1_200),
            ..turn(2, 3_000, "and my budget is about fifteen hundred a month")
        };
        policy.step(3_000, PolicyInput::EndOfTurn(more), &board);
        // The front marks the words as going on with the request.
        let actions = policy.step(3_000, done_on(2, Act::Update, "t1"), &board);
        let [TaskCmd::Continue { task, text }] = commands(&actions)[..] else {
            panic!("the same task, got {actions:?}");
        };
        assert_eq!(task.0, "t1");
        assert_eq!(text, "and my budget is about fifteen hundred a month");
        // Long after, a new request is a task of its own.
        board.tasks[0].state = TaskState::Working;
        let later = EndOfTurn {
            onset_at: Some(9_000),
            ..turn(3, 10_000, "and book me a taxi to the airport")
        };
        policy.step(10_000, PolicyInput::EndOfTurn(later), &board);
        let other = policy.step(10_000, done(3, Act::Delegate), &board);
        assert!(
            commands(&other)
                .iter()
                .any(|cmd| matches!(cmd, TaskCmd::Delegate { .. })),
            "{other:?}"
        );
    }
}
