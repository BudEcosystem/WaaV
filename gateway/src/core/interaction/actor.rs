//! The one tokio actor per call: it owns the pure machines (`duplex`, `floor`, `tasks`,
//! `policy`), runs them in a fixed order for every message and performs their effects.
//!
//! One loop turn: the duplex machine reads the trigger, the floor takes pump events and duplex
//! actions, the registry takes run events and task commands, the policy sees every derived
//! input, and the floor says what to submit or cut. Those steps never await; an effect is a
//! channel send, an atomic store or a spawned task. Client events are collected during the turn
//! and sent after it.
//!
//! Agent speech on the wire: one `assistant_transcript` with the words when an utterance starts
//! playing (`heard: "none"`), and one with the same `utterance_id` and an empty `delta` when all
//! of its audio was delivered (`"full"`) or it was cut after a part of it (`"partial"`). An
//! utterance whose synthesis failed is sent with its words and stays at `"none"`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;
use tokio::task::{AbortHandle, JoinHandle};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::auth::SessionCredential;
use crate::core::agent::spoken::{SpeechRate, SpokenLedger};
use crate::core::audio::PumpControl;
use crate::core::conversation::TurnAttribution;
use crate::core::llm::{ChatMessage, LlmClient, MessageRole};
use crate::core::metrics::bridge;
use crate::core::stt::STTResult;
use crate::core::tts::AudioData;
use crate::core::voice_manager::VoiceManager;
use crate::handlers::ws::messages::{
    MessageClass, MessageRoute, OutgoingMessage, send_with_policy,
};
use crate::livekit::{LiveKitOperation, OperationQueue};
use crate::plugin::dispatch::{BuiltinSTTProvider, resolve_stt_provider};

use super::analyser;
use super::background::{self, AgentTool, Backend, Job, LlmTier, TextBackend};
use super::bud_agent::AgentBackground;
use super::clips::{self, Bank, FailureLine};
use super::config::{
    self, BackendSpec, FrontModelServer, Intensity, InteractionConfig, ServerInteraction, Tri,
};
use super::controls::{self, Caps, Control, ControlKind, ControlSource, ConversationState, Effect};
use super::duplex::Duplex;
use super::events::{self, Refused, Sentence, WireEvent};
use super::floor::FloorMachine;
use super::front_model::{self, FrontRequest};
use super::front_pool;
use super::front_supervisor;
use super::health::{self, LineKind, Service, Watcher};
use super::persona::{self, Persona};
use super::policy::{self, Policy};
use super::rules::{self, RuleCtx};
use super::tasks::{self, AgentSpec, Registry};
use super::tenant;
use super::turn_record::{TurnRecord, TurnTrace};
use super::types::{
    Act, ActDecision, ActSource, Class, CutCause, DuplexAction, DuplexEvent, EotSource, FloorCmd,
    FloorSnapshot, FrontEvent, GateCmd, HitlId, HitlKind, HitlOutcome, ItemId, Kind, Lane,
    LaneEvent, Ms, Narration, Outcome, PolicyAction, PolicyInput, PumpEvent, Reading, RunEvent,
    RunId, Say, SpeechCmd, TaskCmd, TaskEffect, TaskEvent, TaskEventKind, TaskId, TaskState,
    TaskView, TurnId, UttId, Utterance, Verbosity, WaitReason,
};

/// The actor's inbox. Pump events arrive on the pump's own channel.
#[derive(Debug)]
pub enum Msg {
    /// Boxed: a result is three times the size of any other message.
    Stt(Box<STTResult>),
    SttError(String),
    TtsError(String),
    Duplex(DuplexEvent),
    Front(FrontEvent),
    Run(RunId, RunEvent),
    /// An upload was asked for and nothing was buffered: no final will come for it.
    SttEmpty,
    /// The client's `audio_end`.
    AudioEnd,
    TaskAnswer {
        request_id: String,
        outcome: HitlOutcome,
    },
    TaskCancel {
        task_id: String,
    },
    /// The client's `task_seen`: the caller read the task's result on the screen.
    TaskSeen {
        task_id: String,
    },
    /// The front's verdict on the caller's words over the agent (an interim result).
    FrontInterim(FrontEvent),
    /// The client's `control`, checked.
    Control(super::controls::Control),
    /// The session's voice provider's voices by language (for a language switch), found
    /// after the session started.
    Voices(Vec<(String, String)>),
    /// `Voices` will come: a caller's or client's language switch waits for it.
    VoicesComing,
    /// The voice provider's list, read for a language or voice switch (chapter 9).
    VoiceCatalog(Vec<crate::handlers::voices::Voice>),
    /// A recogniser opened in another language for a switch that waits for it, or why not.
    SttOpened {
        language: String,
        stt: Result<NextStt, String>,
    },
    /// The room's SIP leg can refer the caller elsewhere (`controls.human.transfer_to`).
    SipTransfer(SipTransfer),
    /// The front's wording of a task's update (`None`: none in time), and how long it took.
    Narrated {
        item: ItemId,
        line: Option<String>,
        ms: u64,
    },
    /// The text end-of-turn model's reading of the words a turn is held on
    /// (`DuplexAction::JudgeWords`).
    WordsJudged {
        text: String,
        finished: bool,
    },
    /// A pair on another front server, for a session whose pair was lost (spec 029 SF-5).
    FrontPlaced(Box<front_pool::Placement>),
    /// The front's label for a turn it read (`None`: none in time), and how long it took.
    Read {
        turn: TurnId,
        label: Option<String>,
        ms: u64,
    },
    /// The front's reading of the language of a turn's words: the code it wrote (`None`: none
    /// in time), and how long it took.
    Language {
        turn: TurnId,
        code: Option<String>,
        ms: u64,
    },
    /// The front's reading of a caller's repeat of the number in the agent's last line:
    /// `number` (only it) or `line` (the whole line); `None`: none in time.
    Number {
        control: Control,
        label: Option<String>,
        ms: u64,
    },
    /// A key the room's caller pressed on a phone (`0`-`9`, `*`, `#`).
    Dtmf(char),
    /// A question's `task_question` is on the client's connection (sign-in links only).
    Delivered(HitlId),
    /// The client's `playback_position`: how much of an utterance it has played.
    PlaybackPosition {
        utterance_id: String,
        played_ms: u64,
    },
    Update(serde_json::Value),
    ClientSpeak(String),
    ClientClear,
    TypedTurn(String),
    Ready,
    /// The front's decisions' slot is warm (`front_model::warm`).
    FrontWarm,
    /// How a task that an earlier connection of this caller left running has ended.
    Outcome(Outcome),
    /// The session is over. `clean`: the client closed it, so nobody is coming back for
    /// what its tasks still do.
    Shutdown {
        clean: bool,
    },
    /// A speech-to-speech provider's session (`provider`): what the provider did.
    Provider(super::provider::ProviderEvent),
    /// The provider's executor sent a gateway line.
    LineSent {
        line: u64,
    },
    /// The response of a gateway line ended. `spoken: false`: the provider had no wire for
    /// it, or the connection went down under it.
    LineEnded {
        line: u64,
        spoken: bool,
    },
    /// A listening sound of the bank was checked (`clips::check`): line `line`, the operator's
    /// recording or the rendered clip, and the clip or why it was refused.
    ClipChecked {
        line: usize,
        recorded: bool,
        clip: Result<AudioData, String>,
    },
    /// A background run's first token: the health watcher's first-token deadline is met.
    RunAlive(RunId),
    /// A round of a run's tool calls and their results (`background::tool_round`).
    ToolRound(RunId, Vec<ChatMessage>),
    /// The operator's answer to a webhook hand-off to a person (`controls.human.url`).
    Person(super::remote::PersonAnswer),
    /// The call's recording, paused while a sensitive value is typed on the keypad.
    RecordingPause(RecordingPause),
}

/// A recogniser opened for a language switch, not yet hearing the caller. One that is never
/// swapped in (a later switch replaced it, or the session ended) is disconnected, off the
/// actor, whatever its provider: a vendor's stream is never left open by a drop.
pub struct NextStt(Option<Box<dyn crate::core::stt::BaseSTT>>);

impl NextStt {
    fn take(mut self) -> Option<Box<dyn crate::core::stt::BaseSTT>> {
        self.0.take()
    }
}

impl Drop for NextStt {
    fn drop(&mut self) {
        let (Some(mut stt), Ok(runtime)) = (self.0.take(), tokio::runtime::Handle::try_current())
        else {
            return;
        };
        runtime.spawn(async move {
            if let Err(error) = stt.disconnect().await {
                debug!(%error, "the unused recogniser did not close cleanly");
            }
        });
    }
}

impl std::fmt::Debug for NextStt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("NextStt")
    }
}

/// The referral of a SIP caller to a number, under way.
pub type Referral = std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>;

/// Refers the room's SIP caller to a number (the existing SIP leg's transfer).
#[derive(Clone)]
pub struct SipTransfer(pub Arc<dyn Fn(String) -> Referral + Send + Sync>);

impl std::fmt::Debug for SipTransfer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SipTransfer")
    }
}

/// Stops (`true`) and starts again (`false`) the call's recording: it is paused while a
/// sensitive value is typed on the keypad.
#[derive(Clone)]
pub struct RecordingPause(pub Arc<dyn Fn(bool) -> Referral + Send + Sync>);

impl std::fmt::Debug for RecordingPause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RecordingPause")
    }
}

/// How long a language or voice switch waits for the provider's voice list (Deepgram's
/// /v1/models took 10 to 18 s from this machine's network on 19 October; 20 s cut it off
/// once), and for a recogniser opened in the new language.
const VOICE_LIST_WAIT: Duration = Duration::from_secs(30);
const STT_OPEN_WAIT: Duration = Duration::from_secs(10);
/// The persona's line before the caller is put through to a person.
const HUMAN_LINE: &str = "control.human_transfer";
/// Said while the operator finds a person (a webhook hand-off that waits for its answer).
const HUMAN_FINDING: &str = "control.human_finding";
/// Said when the operator found no one, or did not answer in `controls.human.wait_s`.
const HUMAN_NO_ANSWER: &str = "control.human_no_answer";

const INBOX_CAPACITY: usize = 256;
/// How long `shutdown` waits for the actor: a teardown must not hang on an egress channel
/// nobody reads any more.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(2);
/// Misses of the model front in a row (no act within its budget, or no decision) after which
/// it counts as unavailable.
const FRONT_MISSES: u8 = 3;
/// How a finished child's result begins in the call's history (not the caller's words).
const EARLIER_RESULT: &str = "Result of an earlier task, for reference (not from the caller; \
    the caller has been told of it):";
/// With nothing sent to the client this long, a `heartbeat` goes out.
const HEARTBEAT_MS: Ms = 5_000;
/// The `degraded` reason of a session whose model front does not answer (chapter 9).
const FRONT_UNAVAILABLE: &str = "front_unavailable";
/// A `hold` with no time given waits this long for the caller (seconds), then checks in.
const HOLD_S: u32 = 30;
/// How a control note begins in the call's history (`controls::history_note`).
const NOTE_START: char = '[';
/// Listening sounds play as utterances numbered from here, apart from the floor's.
const CLIP_BASE: u64 = 1 << 40;
/// The `degraded` reason of a session none of whose listening sounds passed the clip checks.
const NO_LISTENING_SOUNDS: &str = "backchannel_unavailable";
/// The lines said when a service fails (chapter 10) are rendered as bank lines numbered from
/// here, and play as utterances numbered from [`FAILURE_UTT`].
const FAILURE_LINE: usize = 1 << 20;
const FAILURE_UTT: u64 = CLIP_BASE + (1 << 30);
/// A failure line starts only once the caller has been silent this long.
const FAILURE_PAUSE_MS: Ms = 500;
/// While the front model the gateway runs is down, its supervisor is looked at this often.
const FRONT_POLL_MS: Ms = 500;
/// The exit line ends the session this long after it should have played out, if the pump
/// never says it has.
const EXIT_SLACK_MS: Ms = 2_000;
/// Less voiced audio than this during a capture window is a cough, not speech.
const CAPTURED_SPEECH_MS: u32 = 250;
/// A listening sound that must yield fades out over this long (chapter 5's
/// `clip_yield_fade_ms`).
const CLIP_FADE_MS: u32 = 40;
/// An utterance the client reports played to within this of its length was heard in full
/// (the client reports at most four times a second).
const PLAYED_TOLERANCE_MS: u64 = 300;
/// A client that has reported nothing for this long is no longer reporting: the pump's
/// estimate of what was heard stands.
const REPORT_LAPSE_MS: Ms = 2_000;
/// The words of this many latest utterances are kept for `playback_position`.
const RECENT_KEPT: usize = 16;
/// The tool rounds kept for the history of later runs, newest last.
const TOOL_ROUNDS_KEPT: usize = 16;
/// How far back a request settled by a later turn is looked for in the ledger: the request,
/// a note or two, the question and its answer.
const REQUEST_LINES_BACK: usize = 6;
/// Whether a speech-to-text provider's result confidence is the vendor's own number for the
/// words. Every other provider fills the field with a constant or a number of another kind:
/// 1.0 (OpenAI without log-probabilities, ElevenLabs, Alibaba, Baidu, Naver, Cartesia finals),
/// 0.95 or 0.9 (Bhashini, Yandex, Sber, FPT, NECTEC, Sarvam and iFlytek when the vendor sent
/// none), 0.5 (Groq without log-probabilities), 0.0 (Gnani, which sends none; Cartesia,
/// ElevenLabs and Speechmatics interims), or a figure the gateway derives from log-probabilities (OpenAI, Groq).
fn passes_vendor_confidence(provider: &str) -> bool {
    use BuiltinSTTProvider as P;
    matches!(
        resolve_stt_provider(provider),
        Some(
            P::Deepgram
                | P::Gladia
                | P::RevAI
                | P::Phonexia
                | P::Tinkoff
                | P::AwsTranscribe
                | P::IbmWatson
                | P::Speechmatics
        )
    )
}

/// The recogniser's own confidence in a final result, when it reports one: what the vendor
/// sent (`vendor_confidence`: Google, Azure), or the field of a provider that passes the
/// vendor's number through. There 0.0 and 1.0 stand for "not sent", so neither counts.
fn real_confidence(provider: &str, result: &STTResult) -> Option<f32> {
    let passed =
        passes_vendor_confidence(provider) && result.confidence > 0.0 && result.confidence < 1.0;
    result
        .vendor_confidence
        .or(passed.then_some(result.confidence))
}

/// The session's side of the actor: the inbox, the settings echo and the shutdown.
#[derive(Clone)]
pub struct InteractionHandle {
    tx: mpsc::Sender<Msg>,
    echo: Arc<serde_json::Value>,
    stopped: CancellationToken,
}

impl InteractionHandle {
    /// The handle of a session whose loop is not this actor (a `/ws` realtime voice): `tx` is
    /// its inbox, `echo` its settings, and its loop cancels `stopped` once it has ended.
    pub fn new(tx: mpsc::Sender<Msg>, echo: serde_json::Value, stopped: CancellationToken) -> Self {
        Self {
            tx,
            echo: Arc::new(echo),
            stopped,
        }
    }

    /// The inbox itself, for a task that reports to the actor on its own (the analyser).
    pub fn inbox(&self) -> mpsc::Sender<Msg> {
        self.tx.clone()
    }

    /// The resolved settings, sent as `ready.interaction`.
    pub fn echo(&self) -> serde_json::Value {
        (*self.echo).clone()
    }

    /// For a callback that must not wait: `false` when the message was dropped (the inbox is
    /// full, or the actor is gone).
    pub fn try_send(&self, msg: Msg) -> bool {
        match self.tx.try_send(msg) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                bridge::record_degraded("interaction", "inbox_full");
                false
            }
            Err(TrySendError::Closed(_)) => false,
        }
    }

    /// `false` when the actor is gone.
    pub async fn send(&self, msg: Msg) -> bool {
        self.tx.send(msg).await.is_ok()
    }

    /// Ends a session its client closed: speech is cut and every task is told. Returns once
    /// the actor has done that (or after two seconds); a task whose tool call was already
    /// allowed still finishes after it.
    pub async fn shutdown(&self) {
        self.end(true).await
    }

    /// Ends a session whose connection was lost. As [`Self::shutdown`], and the caller may
    /// come back for what its tasks still do (`tasks.reconnect_grace_s`).
    pub async fn dropped(&self) {
        self.end(false).await
    }

    async fn end(&self, clean: bool) {
        if self.tx.send(Msg::Shutdown { clean }).await.is_ok() {
            let _ = tokio::time::timeout(SHUTDOWN_WAIT, self.stopped.cancelled()).await;
        }
    }
}

/// Brings a session what the tasks of its caller's earlier connection came to: the session is
/// the same owner's with the same `stream_id`, and it is back within that connection's grace.
/// What ended meanwhile comes first, each task once. Runs until the session ends: what its
/// own tasks come to after that is for the next connection, not for this one.
pub async fn follow(owner: String, stream_id: String, session: InteractionHandle) {
    let mut earlier = tasks::store().follow(&owner, &stream_id);
    let (mut claimed, mut seen) = (false, HashSet::new());
    loop {
        // Asked again with each outcome: the earlier connection may end after this one began.
        let backlog = match claimed {
            true => None,
            false => tasks::store().claim(&owner, &stream_id).await,
        };
        claimed |= backlog.is_some();
        for outcome in backlog.unwrap_or_default() {
            if seen.insert(outcome.task_id.clone()) && !session.send(Msg::Outcome(outcome)).await {
                return;
            }
        }
        let outcome = tokio::select! {
            // First: the session's end comes before any outcome of its own tasks.
            biased;
            _ = session.stopped.cancelled() => return,
            outcome = earlier.next() => outcome,
        };
        let Some(outcome) = outcome else {
            return;
        };
        // Unclaimed, it is in the log the next claim returns, or it is not this session's.
        if claimed
            && seen.insert(outcome.task_id.clone())
            && !session.send(Msg::Outcome(outcome)).await
        {
            return;
        }
    }
}

/// What does the session's tasks.
#[derive(Clone)]
pub enum Background {
    /// The legs of `core::conversation::resolve_llm_legs`. Runs use the reasoning tier when
    /// there is one.
    Llm(Arc<LlmClient>, Option<Arc<LlmClient>>),
    /// The session's Bud agent: one budprompt run per task.
    Agent(Arc<AgentBackground>),
    /// A backend that answers in text on its own (the session's text-output DAG): one run
    /// per task.
    Text(Arc<dyn TextBackend>),
    /// An entry of the operator's catalog: a webhook, an A2A agent, an MCP task tool or an
    /// OpenAI Responses model.
    Remote(Arc<dyn Backend>),
}

impl Background {
    /// `tasks.backend` of the settings echo, and `background` of `task_created`.
    pub(crate) fn label(&self) -> String {
        match self {
            Self::Llm(..) => "llm".to_owned(),
            Self::Agent(agent) => Backend::label(&**agent),
            Self::Text(backend) => backend.label().to_owned(),
            Self::Remote(backend) => backend.label(),
        }
    }

    /// An entry of the operator's catalog, prompted as the LLM tier is: the persona's preamble,
    /// then the operator's prompt.
    pub(crate) fn remote(
        spec: super::config::BackendSpec,
        cfg: &InteractionConfig,
        server: &ServerInteraction,
        operator_prompt: Option<String>,
    ) -> Self {
        let preamble = cfg.front.load_persona().background_preamble();
        let operator = operator_prompt.unwrap_or_default();
        let (secret, tools) = (server.tool_secret.clone(), cfg.tasks.tools.clone());
        Self::Remote(super::remote::Remote::new(
            spec,
            secret,
            [&preamble, &operator],
            tools,
        ))
    }

    /// The LLM legs of a conversation config.
    pub(crate) fn llm(llm: &crate::core::conversation::ConversationConfig) -> Result<Self, String> {
        // Keyed as the conversation loop's legs are (`core::credentials`); a withheld key is
        // logged there.
        crate::core::conversation::resolve_llm_legs(llm)
            .map(|legs| Self::Llm(legs.llm, legs.reasoning_llm))
            .map_err(|e| e.to_string())
    }
}

/// What runs a session's tasks: the LLM tier is prompted with the persona's preamble, then
/// the operator's prompt; as an orchestrator it has `agents` as tools.
#[allow(clippy::too_many_arguments)]
pub(super) fn backend(
    background: Background,
    cfg: &InteractionConfig,
    server: &ServerInteraction,
    persona: &Persona,
    system_prompt: Option<String>,
    stream_id: &str,
    attribution: Option<TurnAttribution>,
    agents: Vec<AgentTool>,
) -> Arc<dyn Backend> {
    match background {
        Background::Llm(fast, reasoning) => {
            let prompts = [
                persona.background_preamble(),
                system_prompt.unwrap_or_default(),
            ];
            Arc::new(LlmTier {
                llm: reasoning.unwrap_or(fast),
                system_prompt: prompts
                    .iter()
                    .map(|prompt| prompt.trim())
                    .filter(|prompt| !prompt.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n\n"),
                operator_prompt: prompts[1].trim().to_owned(),
                tools: cfg.tasks.tools.clone(),
                agents,
                tool_secret: server.tool_secret.clone(),
                session_id: stream_id.to_owned(),
                attribution,
            })
        }
        Background::Agent(agent) => agent,
        Background::Text(backend) => Arc::new(background::Text(backend)),
        Background::Remote(backend) => backend,
    }
}

/// The catalog entries of an orchestrator session's `tasks.agents`. None without
/// `tasks.orchestrator`.
fn listed_agents<'a>(
    cfg: &InteractionConfig,
    server: &'a ServerInteraction,
) -> Vec<&'a BackendSpec> {
    let entry = |name: &String| server.task_backends.iter().find(|spec| &spec.name == name);
    let named = cfg
        .tasks
        .agents
        .iter()
        .filter(|_| cfg.tasks.orchestrator.is_some());
    named.filter_map(entry).collect()
}

/// The tools of an orchestrator session's agents (`remote::agent_tools`), the backends they
/// and their fallbacks run on, and those as the registry knows them. Nothing without
/// `tasks.orchestrator`.
fn catalog_agents(
    cfg: &InteractionConfig,
    server: &ServerInteraction,
) -> (
    Vec<AgentTool>,
    HashMap<String, Arc<dyn Backend>>,
    HashMap<String, AgentSpec>,
) {
    let entry = |name: &str| server.task_backends.iter().find(|spec| spec.name == name);
    let listed = listed_agents(cfg, server);
    let called = super::remote::agent_tools(&listed);
    let fallbacks = listed
        .iter()
        .filter_map(|spec| entry(spec.fallback.as_deref()?));
    let fallbacks = fallbacks.map(|spec| (spec.name.clone(), spec.clone()));
    let runs: Vec<(String, BackendSpec)> = called
        .iter()
        .map(|(tool, spec)| (tool.name.clone(), spec.clone()))
        .chain(fallbacks)
        .collect();
    let mut specs: HashMap<String, AgentSpec> = runs
        .iter()
        .map(|(name, spec)| (name.clone(), AgentSpec::of(spec)))
        .collect();
    for (tool, _) in &called {
        if let Some(spec) = specs.get_mut(&tool.name) {
            spec.direct_follow_up = tool.parameters.is_none() && !tool.confirm;
        }
    }
    // A child hears only its request: no persona prompt, no history, no tools of the session.
    let runs = runs
        .into_iter()
        .map(|(name, spec)| {
            let remote =
                super::remote::Remote::new(spec, server.tool_secret.clone(), ["", ""], Vec::new());
            (name, remote as Arc<dyn Backend>)
        })
        .collect();
    let tools = called.into_iter().map(|(tool, _)| tool).collect();
    (tools, runs, specs)
}

/// How often a session whose front pair was lost asks for one elsewhere (spec 029 SF-5).
const REPLACE_EVERY_MS: Ms = 2_000;

/// The longest the text end-of-turn model is waited for: a reading later than the hold it
/// could end (`duplex::HOLD_MS`) ends nothing.
const WORDS_JUDGE_WAIT: Duration = Duration::from_millis(800);

/// What the actor works with. It never reads `config::server()`.
pub struct ActorDeps {
    pub vm: Arc<VoiceManager>,
    /// The metered pump's control; it is also the session clock.
    pub ctl: Arc<PumpControl>,
    pub pump_rx: UnboundedReceiver<PumpEvent>,
    /// The speech worker.
    pub speech: UnboundedSender<SpeechCmd>,
    /// The session's egress channel.
    pub out: mpsc::Sender<MessageRoute>,
    pub background: Background,
    /// The operator's system prompt; the persona's preamble goes in front of it.
    pub system_prompt: Option<String>,
    /// The session's live credential for the LLM legs, when it has one.
    pub credential: Option<SessionCredential>,
    /// A Bud-mode session's caller, for the `voice.turn` record of each LLM-tier run.
    pub attribution: Option<TurnAttribution>,
    pub server: Arc<ServerInteraction>,
    pub cfg: InteractionConfig,
    pub stream_id: String,
    /// The task store's owner key (`handlers::tasks::owner`).
    pub owner: String,
    /// The highest task ref earlier connections of this session used
    /// (`TaskStore::refs_used`).
    pub refs_used: u32,
    /// The speech-to-text answers once per upload (`VoiceManager::stt_is_request_response`).
    pub request_response: bool,
    /// The speech-to-text runs its own turn model (`VoiceManager::stt_has_turn_events`).
    pub provider_turns: bool,
    /// When the speech-to-text can be asked to end the caller's turn now: its endpointing
    /// silence (`VoiceManager::stt_end_turn_after_ms`).
    pub stt_end_turn_after_ms: Option<u32>,
    /// The speech provider serves repeated lines from the TTS cache
    /// (`VoiceManager::tts_has_cache`); without one a warm-up would only cost requests.
    pub warm: bool,
    /// The session has a `conversation_config`: the plain path a caller who keeps speaking
    /// another language is stepped down to.
    pub plain_path: bool,
    /// A LiveKit session: the room's operation queue. Every event sent to the client is also
    /// published in the room, on the data topic [`ROOM_TOPIC`].
    pub room: Option<OperationQueue>,
    /// The session's pair on a front server the gateways share (spec 029 §5.8); `None` when
    /// the gateway runs its front itself.
    pub placement: Option<front_pool::Placement>,
}

/// The room data topic of a LiveKit session's interaction events.
pub const ROOM_TOPIC: &str = "waav.interaction";

/// Start the actor of one session.
pub fn spawn(deps: ActorDeps) -> (InteractionHandle, JoinHandle<()>) {
    let (tx, inbox) = mpsc::channel(INBOX_CAPACITY);
    let stopped = CancellationToken::new();
    // The failure lines that are ready before the session starts (chapter 10).
    let failure = failure_entries(&deps.cfg, &deps.server, &deps.vm);
    let mut echo = config::echo(&deps.cfg, &deps.background.label(), &[]);
    echo["failure"]["lines"] = failure_set(&failure, pcm_voice(&deps.vm)).into();
    echo["failure"]["follow_up"] = follow_up(&deps.cfg).into();
    let handle = InteractionHandle {
        echo: Arc::new(echo),
        stopped: stopped.clone(),
        // The actor keeps only a weak sender, so it ends when the session lets go of it.
        tx: tx.clone(),
    };
    let language = deps.cfg.front.language.clone();
    let tx = tx.downgrade();
    let run = async move {
        let _open = OpenSession::new();
        // An orchestrator's agents are described by what they say of themselves, and the
        // card of the session's own A2A backend says how it is reached (its binding, its
        // credential's scheme), whether it streams and whether it pushes its tasks' updates.
        let server = Arc::clone(&deps.server);
        let mut read = listed_agents(&deps.cfg, &server);
        let own = deps.cfg.tasks.backend.as_ref();
        let own_a2a = |spec: &&BackendSpec| {
            spec.kind == super::config::BackendKind::A2a && own == Some(&spec.name)
        };
        read.extend(server.task_backends.iter().filter(own_a2a));
        super::remote::discover(&read).await;
        let (mut actor, pump_rx) = Actor::new(deps, tx);
        actor.failure_lines = failure;
        actor.run(inbox, pump_rx, stopped).await;
    };
    // The rules and the guard read the session's pack.
    (handle, tokio::spawn(persona::scope(&language, run)))
}

/// The interaction sessions this process runs now (`sessions_open`).
static SESSIONS_OPEN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// One running session, counted while it lives.
struct OpenSession;

impl OpenSession {
    fn new() -> Self {
        SESSIONS_OPEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self
    }
}

impl Drop for OpenSession {
    fn drop(&mut self) {
        SESSIONS_OPEN.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    }
}

/// How many interaction sessions this process runs now: work for no call (the server's
/// default failure lines) waits for none to be open.
pub fn sessions_open() -> usize {
    SESSIONS_OPEN.load(std::sync::atomic::Ordering::Relaxed)
}

/// What woke the loop.
enum Trigger {
    Msg(Msg),
    Pump(PumpEvent),
    Wake,
}

/// The latest caller turn, as the effects need it.
struct Turn {
    id: TurnId,
    e: Ms,
    /// When the caller stopped speaking, as far as the analyser saw it.
    speech_end: Option<Ms>,
    /// `false` when the caller was talking to someone else.
    addressed: bool,
    /// Typed by the client: there was no audio, so it cannot be the agent's own line.
    typed: bool,
    /// The recogniser named the words' language (its model's reading, `foreign`): the front
    /// is not asked to read it again.
    language_named: bool,
    /// The recogniser was not sure of the words.
    unclear: bool,
    /// The caller's words already cut the agent (a barge-in over it).
    barged: bool,
    act: Option<Act>,
    /// Still to be measured from E: the first agent audio, and the first answer sentence.
    first_audible_owed: bool,
    answer_owed: bool,
}

/// An utterance handed to the speech worker and not yet heard or dropped.
struct Spoken {
    utt: UttId,
    text: String,
    utterance: Utterance,
    started: bool,
    /// The audio the pump delivered of it, as of its last `Idle`.
    delivered_ms: u64,
    /// When it went to the synthesiser, and whether nothing was playing or in synthesis
    /// ahead of it: its first audio then tells how long a synthesis takes.
    submitted: Ms,
    alone: bool,
}

/// An utterance the pump has played out, whose hearing waits for the client's report.
struct Held {
    spoken: Spoken,
    /// The floor's `Heard`, with the pump's estimate of when.
    event: LaneEvent,
    since: Ms,
}

/// A turn the model front decides.
struct FrontWait {
    turn: TurnId,
    /// The request: stopped when its answer can no longer be used, which frees the model's
    /// slot for the next decision.
    request: AbortHandle,
    /// The front reads the language of the words (owner, 24 Oct: no word list decides it): the
    /// decision's events are held until it has (`language_known`).
    reading: bool,
    /// The words the language reading is still to read (taken when it is asked).
    words: Option<String>,
    /// The turn's words (the number reading's, `read_number`).
    said: String,
    /// The reading: another language than the pack's. `None` while it is out.
    other: Option<bool>,
    /// The language reading's request while it is out (one of the actor's asides).
    asked: Option<AbortHandle>,
    held: Vec<FrontEvent>,
}

impl FrontWait {
    /// Stops the decision and a language reading still out, which would otherwise hold a slot
    /// of the server beside the next turn's decision (live G1, r14: a superseded turn's
    /// reading ran 0.4-1.1 s on). Whether a reading was out (its aside ends here).
    fn abort(self) -> bool {
        self.request.abort();
        self.asked.inspect(AbortHandle::abort).is_some()
    }
}

/// The front's request about the caller's words over the agent, and when it was asked.
/// How long a prompt primed at the caller's first sound over the agent serves its interims.
const PRIMED_MS: Ms = 10_000;

/// The acts an interim over the agent may be.
fn interim_acts() -> Vec<Act> {
    Act::ALL
        .into_iter()
        .filter(|act| !matches!(act, Act::Backchannel | Act::Echo | Act::Question))
        .collect()
}

struct Interim {
    turn: TurnId,
    asked: Ms,
    /// When the turn's first interim was asked about.
    since: Ms,
    request: AbortHandle,
    /// Its act is known.
    known: bool,
    /// The newest words of the turn, asked about once this request has its act.
    next: Option<String>,
    /// The words it asks about.
    text: String,
    /// Its turn ended while it was out: kept until the turn's decision has its act, as a stop
    /// it names cuts the agent first (`late_interim`).
    ended: bool,
}

/// Client events with their send class, in order. A question whose delivery the actor waits
/// for (a sign-in link) carries its id.
pub(crate) type Events = UnboundedSender<(String, MessageClass, Option<HitlId>)>;

/// The one task that puts client events on the session's egress channel, so the actor's loop
/// never waits for a client that reads slowly. A room gets the same JSON on its data topic
/// (its queue's worker publishes it; a failure there is the worker's to log). It ends when
/// the sender is dropped and everything sent has gone out. An event with a question's id is
/// reported to the actor (`Msg::Delivered`) once the client's connection has taken it.
pub(crate) fn forward(
    out: mpsc::Sender<MessageRoute>,
    room: Option<OperationQueue>,
    inbox: mpsc::WeakSender<Msg>,
) -> (Events, JoinHandle<()>) {
    let (events, mut queued) = mpsc::unbounded_channel::<(String, MessageClass, Option<HitlId>)>();
    let forwarder = tokio::spawn(async move {
        while let Some((json, class, delivered)) = queued.recv().await {
            if let Some(room) = &room
                && let Ok(data) = serde_json::from_str(&json)
            {
                let publish = LiveKitOperation::SendDataMessage {
                    topic: ROOM_TOPIC.to_owned(),
                    data,
                    response_tx: oneshot::channel().0,
                    retry_count: 0,
                };
                let _ = room.queue(publish).await;
            }
            let Some(request) = delivered else {
                send_with_policy(&out, MessageRoute::Json(json), class).await;
                continue;
            };
            // Never waits on the actor: it may be waiting for this task (`drain`).
            if out.send(MessageRoute::Json(json)).await.is_ok()
                && let Some(inbox) = inbox.upgrade()
            {
                let _ = inbox.try_send(Msg::Delivered(request));
            }
        }
    });
    (events, forwarder)
}

struct Actor {
    vm: Arc<VoiceManager>,
    ctl: Arc<PumpControl>,
    speech: UnboundedSender<SpeechCmd>,
    out: mpsc::Sender<MessageRoute>,
    room: Option<OperationQueue>,
    /// The forwarder of client events and its task.
    events: (Events, JoinHandle<()>),
    warm: bool,
    /// What does the session's tasks.
    background: Arc<dyn Backend>,
    /// The catalog entries an orchestrator starts child tasks on, by name.
    agents: HashMap<String, Arc<dyn Backend>>,
    credential: Option<SessionCredential>,
    /// The run of each task revision and what the caller heard of its answer, for a backend
    /// that is told ([`Backend::heard`]).
    answers: HashMap<(TaskId, u32), (RunId, String)>,
    server: Arc<ServerInteraction>,
    cfg: InteractionConfig,
    stream_id: String,
    /// The session in the task store.
    stored: tasks::Session,
    /// Set when the session ended: until when its caller may come back for what its tasks
    /// still do, or `None` when nobody can (the client closed the session).
    gone: Option<Option<tokio::time::Instant>>,
    tx: mpsc::WeakSender<Msg>,
    persona: Arc<Persona>,

    duplex: Duplex,
    /// The latest caller turn's `voice.turn` record (spec 029 D-9).
    record: Option<TurnRecord>,
    /// The caller turn each live run records itself in, with the run's task: a child task's
    /// run records itself in the turn of its parent's.
    traces: HashMap<RunId, (Option<TaskId>, TurnTrace)>,
    /// The process's text end-of-turn model, which reads the words a turn is held on.
    text_turn: Option<Arc<tokio::sync::RwLock<crate::core::turn_detect::TurnDetector>>>,
    floor: FloorMachine,
    registry: Registry,
    policy: Policy,

    /// The gateway runs the front model (`front_supervisor`) and this session lost it at
    /// that readiness generation: nothing is decided until it is ready again.
    front_down: Option<u64>,
    /// The session's pair on a shared front server (spec 029 §5.8).
    placement: Option<front_pool::Placement>,
    /// When a pair elsewhere was last asked for, after the session's was lost.
    replace_asked: Option<Ms>,
    /// When the front became unavailable: the mode ends past the restart window.
    front_lost_at: Option<Ms>,
    /// The session has a plain conversation to go on as (`ActorDeps::plain_path`).
    plain_path: bool,
    interim: Option<Interim>,
    /// The act an interim of the turn over the agent named that takes the floor only when
    /// the next interim names it too (`interim_cut`).
    over_named: Option<(TurnId, Act)>,
    /// When the caller's words over the agent last cut it (`DuplexAction::Cut`, a barge-in).
    barge_cut_at: Option<Ms>,
    /// When the caller's sound over the agent last had its decision's prompt put into the
    /// front's cache (`prime_interim`): the turn's interims keep that clock.
    primed: Option<Ms>,
    /// Readings and narrations still out on the interims' slot, and whether an agent line
    /// started meanwhile: its priming waits for them (`prime_line`).
    asides: usize,
    prime_owed: bool,
    /// The session's front warm-up while it runs: the first turn's decision drops what is left
    /// of it (`ask_front`).
    warming: Option<tokio::task::AbortHandle>,
    /// The session's lane on the front server: the slots its requests use.
    lane: front_model::Lane,
    /// What each of an orchestrator's agents does, by its own description (`agent_tools`).
    agent_topics: Vec<(String, String)>,
    /// The conversation controls (chapter 9): the state, what the session can do, a control
    /// turn the background answers
    /// ("what do you mean?"), a hold and the lines it keeps back, and what the caller did not
    /// hear of the lines a cut stopped (for `resume`).
    state: ConversationState,
    caps: Caps,
    delegate_turn: Option<TurnId>,
    hold_until: Option<Ms>,
    held_lines: Vec<Utterance>,
    unheard: Vec<String>,
    /// The voice list is on its way, and a language or voice switch asked for before it (or
    /// before the recogniser opened for it) came; whether the list was asked for.
    voices_owed: bool,
    language_owed: Option<Control>,
    /// The orchestrator's tool waiting for that switch's result.
    owed_reply: Option<oneshot::Sender<serde_json::Value>>,
    voices_asked: bool,
    /// The recogniser's language; one opened in another language for a switch (swapped in at
    /// the caller's next pause once `stt_due`), and the language one is being opened in.
    stt_language: String,
    stt_next: Option<(String, NextStt)>,
    stt_due: bool,
    stt_opening: Option<String>,
    /// A line said again slower or louder: that line only, at this rate and this much louder.
    line_style: Option<(Say, Option<f32>, f32)>,
    /// The room's SIP transfer; a hand-off to a person due once its line is said (`true`: the
    /// SIP leg refers the call); how the actor closes the socket when it ends the session.
    sip: Option<SipTransfer>,
    /// The call's recording (a room with one), and whether it is paused now.
    recording: Option<RecordingPause>,
    recording_paused: bool,
    handoff: Option<bool>,
    close_reason: Option<(u16, &'static str)>,
    /// Updates whose wording waits for the front to be back.
    unnarrated: Vec<Narration>,
    /// When the last event went to the client (`heartbeat`); `None` before `ready`.
    last_sent: Option<Ms>,
    front: Option<FrontWait>,
    front_misses: u8,
    runs: HashMap<RunId, CancellationToken>,
    /// In the order the floor submitted them.
    live: VecDeque<Spoken>,
    /// The utterance with the provider: the one a speech-synthesis error is about.
    synth: Option<UttId>,
    /// A failure was read from the pump; the provider's own report of it may still come.
    error_owed: bool,
    /// What the caller said and heard, oldest first. It only grows.
    ledger: Vec<ChatMessage>,
    /// The ledger's length before the latest turn: the history of a run for that turn.
    mark: usize,
    /// The same, kept per task for the runs of its later revisions.
    marks: HashMap<TaskId, usize>,
    /// The tool calls of the session's runs and their results, each after the ledger's line of
    /// the request it served (`at`): a later run's history shows what was done for a request
    /// like its own. Without them a small model repeated the lines it saw after an earlier
    /// request ("Got it, I'll check now.", "To confirm:") and called nothing (live D1/D2).
    tool_rounds: VecDeque<(usize, Vec<ChatMessage>)>,
    /// Where the request of each live run is in the ledger.
    run_at: HashMap<RunId, usize>,
    rate: SpeechRate,
    turn: Option<Turn>,
    /// An onset over agent audio whose first ducked frame has not gone out yet.
    barge_onset: Option<Ms>,
    floor_wire: (&'static str, Option<Lane>, &'static str),
    degraded: Vec<&'static str>,
    /// The speech-to-text provider's name.
    stt_provider: String,
    /// Of the final results since the last end of turn: the lowest real confidence, and the
    /// language the recogniser named.
    confidence: Option<f32>,
    language: Option<String>,
    /// When the front model was last asked: its next request carries the lines said since.
    front_asked: Ms,
    /// Stepped down after turns in another language: no front, no tools, a new history.
    plain: bool,
    /// The listening sound that plays, the next clip to use, and when one last played.
    clip: Option<UttId>,
    clip_next: u64,
    last_backchannel: Option<Ms>,
    /// The pauses a listening sound could have played at, and the sounds of this caller turn.
    pauses: u64,
    turn_clips: u32,
    /// The listening sounds while their bank is built, and the clips in synthesis for it
    /// (utterance, bank line).
    bank: Option<Bank>,
    clip_renders: Vec<(UttId, usize)>,
    /// A sensitive value is asked on the screen or the keypad: the speech-to-text hears
    /// silence and its words reach nobody.
    capture: bool,
    /// The client's latest `playback_position` (utterance, ms played, when it came), and the
    /// most it reported of each utterance.
    position: Option<(UttId, u64, Ms)>,
    played: HashMap<UttId, (u64, Ms)>,
    /// What the pump played out and the client has not reported hearing yet, oldest first.
    held: Vec<Held>,
    /// Held events on their way to the policy: they pass without being held again.
    released: HashSet<ItemId>,
    /// The words of the latest utterances.
    recent: VecDeque<(UttId, String)>,
    /// Chapter 10: the lines said when a service fails, the health watcher, the line that
    /// plays, the exit under way and whether the session is over for it.
    failure_lines: Vec<FailureEntry>,
    watcher: Watcher,
    /// The failure lines playing, with their words.
    failure_playing: Vec<(UttId, String)>,
    failure_next: u64,
    exiting: Option<(Service, Ms)>,
    closed: bool,
    /// Each run of the session's background (not an agent's child) must give its first sign
    /// of life by this time (`failure.stall_after_ms`), or the background stalls: its first
    /// token, or a backend's answer to the request where the backend works on its own.
    first_token_due: HashMap<RunId, Ms>,
    /// The recogniser streams: the caller's voiced audio since its last result, and since
    /// when the caller speaks.
    request_response: bool,
    unheard_ms: u32,
    voiced_since: Option<Ms>,
    /// Since when the caller is silent (the analyser's offset): a failure line starts only in
    /// a pause of the caller's, whatever the turn's state (a recogniser that is down never
    /// ends the turn).
    silent_since: Option<Ms>,
    /// A turn the front model was down for: it is asked once the model is back.
    undecided: Option<(TurnId, String, Vec<Act>, bool)>,
}

/// A line said when a service fails, and its audio once it is made: in the session's voice
/// (recorded, cached or rendered), else the server's default set's.
struct FailureEntry {
    line: FailureLine,
    audio: Option<AudioData>,
    own_voice: bool,
    /// A render of it was offered and has not ended (queued or in synthesis).
    rendering: bool,
}

/// The line of a clip render made in a voice the session no longer has: its audio is taken
/// from the pump and dropped.
const STALE_RENDER: usize = usize::MAX;

/// The session's voice, as its clips and failure lines are kept: the provider, the voice
/// (`default` when it names none) and the pack's language.
fn session_voice(vm: &VoiceManager, persona: &Persona) -> clips::Voice {
    let tts = &vm.get_config().tts_config;
    clips::Voice {
        provider: tts.provider.clone(),
        voice: tts.voice_id.clone().unwrap_or_else(|| "default".to_owned()),
        language: persona.language().to_owned(),
    }
}

/// The session's audio is PCM16: recordings and kept renderings can be played in it.
fn pcm_voice(vm: &VoiceManager) -> bool {
    let format = vm.get_config().tts_config.audio_format.clone();
    crate::core::tts::sniff::is_linear_pcm16(format.as_deref().unwrap_or("pcm"))
}

/// The persona's failure lines with the operator's words (`failure.lines`), and the audio of
/// each that needs no synthesis: recorded, or rendered before in this voice.
fn failure_entries(
    cfg: &InteractionConfig,
    server: &ServerInteraction,
    vm: &VoiceManager,
) -> Vec<FailureEntry> {
    let persona = cfg.front.load_persona();
    let voice = session_voice(vm, &persona);
    let (pcm, rate) = (pcm_voice(vm), vm.get_config().tts_config.sample_rate);
    let dir = server.clips_dir.as_deref().map(std::path::Path::new);
    let lines = persona.failure_lines().into_iter().map(|mut line| {
        if let Some(text) = cfg.failure.lines.get(&line.id) {
            line.text.clone_from(text);
        }
        line
    });
    lines
        .map(|line| {
            let ready = pcm.then(|| clips::failure_ready(dir, &voice, &line, rate));
            let audio = ready.flatten().map(|(audio, _)| audio);
            FailureEntry {
                own_voice: audio.is_some(),
                line,
                audio,
                rendering: false,
            }
        })
        .collect()
}

/// Which failure lines `ready` reports: all in the session's voice, the server's default
/// set until the session's are rendered, or none until then.
fn failure_set(entries: &[FailureEntry], pcm: bool) -> &'static str {
    if !entries.is_empty() && entries.iter().all(|entry| entry.own_voice) {
        "session"
    } else if pcm && clips::default_set().is_some() {
        "default"
    } else {
        "none"
    }
}

/// The session has a way to get back to the caller after a call that ended on a failure:
/// a push address, the operator's follow-up, or caller memory.
fn follow_up(cfg: &InteractionConfig) -> bool {
    cfg.tasks.push_url.is_some() || cfg.failure.follow_up || cfg.front.caller.is_some()
}

impl Actor {
    fn new(deps: ActorDeps, tx: mpsc::WeakSender<Msg>) -> (Self, UnboundedReceiver<PumpEvent>) {
        let cfg = deps.cfg;
        let text_turn = deps.vm.turn_detector();
        let persona = cfg.front.load_persona();
        let floor = FloorMachine::new(&cfg, persona.clone(), deps.ctl.lead_ms());
        let speed = deps.vm.get_config().tts_config.speaking_rate;
        let mut policy = Policy::new(&cfg, persona.clone());
        policy.allow_step_down(deps.plain_path);
        let mut registry = Registry::new(&cfg);
        registry.skip_refs(deps.refs_used);
        registry.set_sensitive(persona.sensitive_categories().to_vec());
        registry.set_owner(&deps.owner);
        // The catalog entries of the session: the one that runs the caller's requests, and an
        // orchestrator's agents.
        let named = matches!(deps.background, Background::Remote(_))
            .then(|| cfg.tasks.backend.clone())
            .flatten();
        let (max_wait_s, promise) = (cfg.failure.max_wait_s, follow_up(&cfg));
        let (tools, agents, mut specs) = catalog_agents(&cfg, &deps.server);
        let agent_topics: Vec<(String, String)> = (tools.iter())
            .map(|tool| (tool.name.clone(), tool.description.clone()))
            .collect();
        let single_calls = (tools.iter())
            .filter(|tool| tool.parameters.is_some())
            .map(|tool| tool.name.clone())
            .collect();
        policy.set_single_calls(single_calls);
        policy.set_agent_topics(agent_topics.clone());
        let mut own = deps.server.task_backends.iter();
        if let Some(own) = own.find(|spec| named.as_ref() == Some(&spec.name)) {
            specs.insert(own.name.clone(), AgentSpec::of(own));
        }
        registry.set_catalog(named, specs);
        let background = backend(
            deps.background,
            &cfg,
            &deps.server,
            &persona,
            deps.system_prompt,
            &deps.stream_id,
            deps.attribution,
            tools,
        );
        registry.set_backend(
            background.label(),
            background.own_tools(),
            background.max_concurrent(),
        );
        // The conversation state starts from the caller's settings; what the session can do is
        // completed by `Msg::Caps` once its voice is known.
        let stt_language = deps.vm.get_config().stt_config.language.clone();
        let rate = cfg.front.pace.map_or(1.0, |pace| pace.rate_factor());
        let backchannels = cfg.duplex.backchannel != Tri::Off;
        let mut state =
            ConversationState::new(&cfg.front.language, rate, cfg.front.verbosity, backchannels);
        state.updates = cfg.tasks.announce;
        // Besides its own language, the recogniser hears those its provider takes: a second
        // one is opened in the language at a switch (chapter 9).
        let stt = &deps.vm.get_config().stt_config;
        let reopens = cfg.controls.languages.iter().filter(|code| {
            !crate::core::lang::map_language(code, &stt.provider, &stt.model).has_warnings()
        });
        let caps = Caps {
            hears_any: matches!(stt_language.trim(), "" | "multi" | "auto"),
            hears: std::iter::once(stt_language.clone())
                .chain(reopens.cloned())
                .collect(),
            packs: vec![cfg.front.language.clone()],
            screen: cfg.hitl_client,
            own_voice: deps.vm.get_config().tts_config.voice_id.clone(),
            ..Caps::default()
        };
        let actor = Actor {
            stt_provider: deps.vm.get_config().stt_config.provider.clone(),
            confidence: None,
            language: None,
            front_asked: 0,
            plain: false,
            clip: None,
            clip_next: 0,
            last_backchannel: None,
            pauses: 0,
            turn_clips: 0,
            bank: None,
            clip_renders: Vec::new(),
            capture: false,
            position: None,
            played: HashMap::new(),
            held: Vec::new(),
            released: HashSet::new(),
            recent: VecDeque::new(),
            background,
            agents,
            answers: HashMap::new(),
            credential: deps.credential,
            duplex: Duplex::new(
                cfg.duplex.clone(),
                persona.lexicon().clone(),
                deps.request_response,
            )
            .with_provider_turns(deps.provider_turns)
            .with_turn_end_on_claim(deps.stt_end_turn_after_ms)
            .with_words_judge(text_turn.is_some()),
            record: None,
            traces: HashMap::new(),
            text_turn,
            registry,
            policy,
            floor_wire: floor.wire_state(),
            floor,
            vm: deps.vm,
            ctl: deps.ctl,
            speech: deps.speech,
            events: forward(deps.out.clone(), deps.room.clone(), tx.clone()),
            out: deps.out,
            room: deps.room,
            warm: deps.warm,
            server: deps.server,
            stored: tasks::Session {
                owner: deps.owner,
                id: deps.stream_id.clone(),
                // A session's own address (`allow_client_push`) stays in this process: no other
                // process holds its secret to sign a push for it (`deliver`).
                push_url: cfg
                    .tasks
                    .push_url
                    .clone()
                    .filter(|_| cfg.tasks.push_secret.is_none()),
                grace: Duration::from_secs(cfg.tasks.reconnect_grace_s.into()),
                resume: cfg.tasks.resume_key.clone(),
            },
            gone: None,
            cfg,
            stream_id: deps.stream_id,
            tx,
            persona,
            front: None,
            front_down: None,
            replace_asked: None,
            front_lost_at: None,
            plain_path: deps.plain_path,
            interim: None,
            over_named: None,
            barge_cut_at: None,
            primed: None,
            asides: 0,
            prime_owed: false,
            warming: None,
            lane: deps
                .placement
                .as_ref()
                .map_or_else(front_model::Lane::take, |placed| {
                    front_model::Lane::pinned(placed.pair())
                }),
            placement: deps.placement,
            agent_topics,
            state,
            caps,
            delegate_turn: None,
            hold_until: None,
            held_lines: Vec::new(),
            unheard: Vec::new(),
            voices_owed: false,
            language_owed: None,
            owed_reply: None,
            voices_asked: false,
            stt_language,
            stt_next: None,
            stt_due: false,
            stt_opening: None,
            line_style: None,
            sip: None,
            recording: None,
            recording_paused: false,
            handoff: None,
            close_reason: None,
            unnarrated: Vec::new(),
            last_sent: None,
            front_misses: 0,
            runs: HashMap::new(),
            live: VecDeque::new(),
            synth: None,
            error_owed: false,
            ledger: Vec::new(),
            mark: 0,
            marks: HashMap::new(),
            tool_rounds: VecDeque::new(),
            run_at: HashMap::new(),
            rate: SpeechRate::new(speed.map(f64::from)),
            turn: None,
            barge_onset: None,
            degraded: Vec::new(),
            failure_lines: Vec::new(),
            watcher: Watcher::new(max_wait_s, promise),
            failure_playing: Vec::new(),
            failure_next: 0,
            exiting: None,
            closed: false,
            first_token_due: HashMap::new(),
            request_response: deps.request_response,
            unheard_ms: 0,
            voiced_since: None,
            silent_since: None,
            undecided: None,
        };
        // The caller's pace sets the speaking rate before the first line is synthesised.
        if let Some(pace) = actor.cfg.front.pace {
            let _ = actor.speech.send(SpeechCmd::Rate(pace.rate_factor()));
        }
        (actor, deps.pump_rx)
    }

    async fn run(
        mut self,
        mut inbox: mpsc::Receiver<Msg>,
        mut pump_rx: UnboundedReceiver<PumpEvent>,
        stopped: CancellationToken,
    ) {
        // Whatever ends the actor, a waiting `shutdown` returns.
        let _stopped = stopped.clone().drop_guard();
        // Whether a `rate` control can be applied (chapter 9).
        self.caps.rate = self.vm.tts_takes_rate().await;
        // When the operator allows a language switch: the voices of the session's speech
        // provider by language.
        if !self.cfg.controls.languages.is_empty() {
            self.fetch_voices();
        }
        let clean = loop {
            let now = self.ctl.now_ms();
            let wait = self
                .next_wakeup()
                .map(|at| Duration::from_millis(at.saturating_sub(now).max(1)));
            let trigger = tokio::select! {
                // Pump events first: what was played before a message arrived is known when
                // the message is handled.
                biased;
                Some(event) = pump_rx.recv() => Trigger::Pump(event),
                msg = inbox.recv() => match msg {
                    Some(Msg::Shutdown { clean }) => break clean,
                    // The session let go of the actor without a word.
                    None => break false,
                    Some(msg) => Trigger::Msg(msg),
                },
                _ = tokio::time::sleep(wait.unwrap_or_default()), if wait.is_some() => Trigger::Wake,
            };
            let now = self.ctl.now_ms();
            let stepped = catch_unwind(AssertUnwindSafe(|| self.step(now, trigger)));
            if stepped.is_err() {
                return self.fail().await;
            }
            // A service stayed down: the exit line was said (chapter 10). Nobody comes back
            // to this connection; its tasks follow `tasks.session_end`.
            if self.closed {
                break true;
            }
        };

        self.end(self.ctl.now_ms(), clean);
        self.remember_caller();
        self.drain().await;
        if self.closed {
            let (code, reason) = self.close_reason.unwrap_or((1011, "session_failed"));
            let close = MessageRoute::CloseWith {
                code,
                reason: reason.to_owned(),
            };
            let _ = self.out.send(close).await;
        }
        stopped.cancel();
        // A task that runs on after the session (`tasks.session_end`, or a tool call already
        // allowed) ends by its run, or is stopped at the registry's deadline. A queued one
        // may still start: the inbox stays open for its run.
        let _open = self.tx.upgrade();
        while let Some(deadline) = self.registry.next_wakeup() {
            let now = self.ctl.now_ms();
            let wait = Duration::from_millis(deadline.saturating_sub(now).max(1));
            let cmd = tokio::select! {
                msg = inbox.recv() => match msg {
                    Some(Msg::Run(run, event)) => TaskCmd::Run { run, event },
                    Some(_) => continue,
                    None => break,
                },
                _ = tokio::time::sleep(wait) => TaskCmd::Tick,
            };
            self.task_unspoken(self.ctl.now_ms(), cmd);
        }
    }

    fn next_wakeup(&self) -> Option<Ms> {
        [
            self.policy.next_wakeup(),
            self.duplex.next_wakeup(),
            self.registry.next_wakeup(),
            self.floor.next_wakeup(),
            self.hold_until,
            self.front_lost_at.map(|at| {
                at + Ms::try_from(front_supervisor::RESTART_WAIT.as_millis()).unwrap_or(Ms::MAX)
            }),
            self.held.first().map(|held| {
                let reported = self.position.map_or(0, |(_, _, at)| at);
                held.since.max(reported) + REPORT_LAPSE_MS
            }),
            self.last_sent.map(|at| at + HEARTBEAT_MS),
            self.health_wakeup(),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    /// One loop turn, in the fixed order: duplex, floor, registry, policy, floor commands.
    fn step(&mut self, now: Ms, trigger: Trigger) {
        if self.ctl.phone_leg() {
            self.floor.set_phone_line();
        }
        let mut inputs = VecDeque::new();
        let mut failed = None;
        // The floor's timers first, so the duplex machine reads a current snapshot.
        let mut lanes = self.floor.tick(now);
        lanes.extend(self.release_held(now, false));
        if lanes.iter().any(is_failure) {
            bridge::record_degraded("interaction", "tts_stalled");
            self.error_owed = true;
        }
        let floor = self.floor.snapshot();
        let expected = self.policy.answer_expected(now);

        let actions = match trigger {
            Trigger::Msg(msg) => {
                self.on_msg(now, msg, &floor, &mut inputs, &mut lanes, &mut failed)
            }
            Trigger::Pump(event) => {
                let actions = self.duplex.tick(now, &floor, expected);
                self.on_pump(now, event, &mut lanes, &mut failed);
                actions
            }
            Trigger::Wake => self.duplex.tick(now, &floor, expected),
        };
        for action in actions {
            lanes.extend(self.floor.on_duplex(now, &action));
            self.on_duplex(now, action, &mut inputs);
        }
        // What was played comes before what the caller did about it.
        for lane in lanes.into_iter().rev() {
            inputs.push_front(PolicyInput::Lane(lane));
        }

        if self.registry.next_wakeup().is_some_and(|at| at <= now) {
            self.task(now, TaskCmd::Tick, &mut inputs);
        }
        self.front_gone(now);
        self.hold_over(now, &mut inputs);
        self.health(now, &mut inputs);
        inputs.push_back(PolicyInput::Tick);

        loop {
            while let Some(input) = inputs.pop_front() {
                self.decide(now, input, failed, &mut inputs);
            }
            self.floor
                .on_board(now, &self.policy.shown(self.registry.board()));
            self.floor.set_speech_rate(self.rate.chars_per_ms());
            let cmds = self.floor.next_cmds(now);
            let idle = cmds.is_empty();
            for cmd in cmds {
                let lanes = match cmd {
                    FloorCmd::Submit {
                        utt,
                        utterance,
                        text,
                    } => self.submit(now, utt, utterance, text),
                    FloorCmd::Cut(cause) => self.cut(now, cause),
                    FloorCmd::DropWarm(utts) => self.drop_warm(&utts),
                    FloorCmd::Prerender { utt, text } => self.prerender(utt, text),
                };
                inputs.extend(lanes.into_iter().map(PolicyInput::Lane));
            }
            // What the floor's timers found while it looked for commands.
            inputs.extend(self.floor.tick(now).into_iter().map(PolicyInput::Lane));
            if idle && inputs.is_empty() && self.floor.quiet() && self.policy.waits_for_quiet() {
                // The floor fell quiet after the policy last looked: an update may be said.
                inputs.push_back(PolicyInput::Tick);
            }
            if idle && inputs.is_empty() {
                break;
            }
        }

        self.sync_capture();
        let state = self.floor.wire_state();
        if self.cfg.events_floor && state != self.floor_wire {
            self.floor_wire = state;
            let (holder, lane, reason) = state;
            let event = WireEvent::FloorState {
                holder,
                lane,
                reason,
                t_ms: now,
            };
            self.send(&event, MessageClass::Transcript);
        }
        if self.last_sent.is_some_and(|at| now >= at + HEARTBEAT_MS) {
            self.send(
                &WireEvent::Heartbeat { t_ms: now },
                MessageClass::Transcript,
            );
        }
    }

    /// Steps 1 and 2 for an inbox message: the duplex machine's actions, and what the message
    /// means for the floor, the registry and the policy.
    fn on_msg(
        &mut self,
        now: Ms,
        msg: Msg,
        floor: &FloorSnapshot,
        inputs: &mut VecDeque<PolicyInput>,
        lanes: &mut Vec<LaneEvent>,
        failed: &mut Option<UttId>,
    ) -> Vec<DuplexAction> {
        let expected = self.policy.answer_expected(now);
        match msg {
            Msg::Stt(result) => {
                // The recogniser answers: it is well (chapter 10).
                self.unheard_ms = 0;
                self.voiced_since = self.voiced_since.map(|_| now);
                self.service_recovered(Service::Recogniser);
                // A speech-final carries the whole segment.
                let text = if result.is_speech_final {
                    result.turn_transcript()
                } else {
                    &result.transcript
                };
                let (is_final, speech_final) = (result.is_final, result.is_speech_final);
                if is_final && !text.trim().is_empty() {
                    let sure = real_confidence(&self.stt_provider, &result);
                    self.confidence = match (self.confidence, sure) {
                        (Some(lowest), Some(sure)) => Some(lowest.min(sure)),
                        (lowest, sure) => lowest.or(sure),
                    };
                    if result.detected_language.is_some() {
                        self.language.clone_from(&result.detected_language);
                    }
                }
                let mut actions =
                    self.duplex
                        .on_stt(now, text, is_final, speech_final, floor, expected);
                if let Some(event) = result.turn_event {
                    actions.extend(self.duplex.on_turn_event(event, floor));
                }
                return actions;
            }
            Msg::SttError(error) => {
                warn!(stream_id = %self.stream_id, %error, "interaction speech-to-text failed");
                self.service_failing(Service::Recogniser, now + self.stall_after());
                return self.duplex.on_stt_error(now);
            }
            // The speech-final that will not come: the turn ends with the words it has.
            Msg::SttEmpty => return self.duplex.on_stt(now, "", true, true, floor, expected),
            Msg::AudioEnd => return self.duplex.on_audio_end(now),
            Msg::Duplex(event) => {
                debug!(stream_id = %self.stream_id, ?event, "interaction audio event");
                match event {
                    DuplexEvent::Onset { at, .. } => {
                        self.floor.on_voice(true, at);
                        self.voiced_since = Some(at);
                        self.silent_since = None;
                        // The caller speaks again: a listening sound stops, and so does a
                        // stall line (never over the caller).
                        self.stop_clip(now);
                        self.stop_failure_line(now);
                        if floor.agent_audible {
                            self.barge_onset = Some(at);
                            self.prime_interim(now);
                        }
                    }
                    // Speech while a sensitive value is captured: the speech-to-text heard
                    // silence, and the caller is told where the value goes.
                    DuplexEvent::Offset { voiced_ms, .. }
                        if self.capture && voiced_ms >= CAPTURED_SPEECH_MS =>
                    {
                        inputs.push_back(PolicyInput::CapturedSpeech)
                    }
                    DuplexEvent::Degraded(reason) => self.degrade(reason),
                    _ => {}
                }
                if let DuplexEvent::Offset { voiced_ms, at } = &event {
                    self.voiced_since = None;
                    self.silent_since = Some(*at);
                    self.unheard_ms = self.unheard_ms.saturating_add(*voiced_ms);
                    self.swap_recogniser();
                }
                // A pause of the caller's, from their last voiced audio: an urgent question
                // may start in it.
                if let DuplexEvent::Offset { at, .. } = &event {
                    let lag = Ms::from(analyser::OFFSET_LAG_MS);
                    self.floor.on_voice(false, at.saturating_sub(lag));
                }
                return self.duplex.on_event(now, event, floor, expected);
            }
            Msg::TypedTurn(text) => return self.duplex.on_typed(now, &text),
            Msg::TtsError(error) => {
                warn!(stream_id = %self.stream_id, %error, "interaction speech synthesis failed");
                // A provider reports its error and then completes. When the completion was
                // read first the utterance has failed already, and this report is its tail.
                if !std::mem::take(&mut self.error_owed)
                    && let Some(utt) = self.synth.take()
                {
                    bridge::record_degraded("interaction", "tts_failed");
                    self.ctl.fail(utt);
                    *failed = Some(utt);
                    self.clip_rendered(now, utt, lanes);
                    lanes.extend(self.floor.on_pump(now, PumpEvent::Failed { utt }));
                }
            }
            Msg::Front(event) => self.on_front(now, event, inputs),
            Msg::FrontInterim(event) => self.on_interim(now, event, inputs, lanes),
            Msg::Control(control) => {
                self.control(now, control, inputs);
            }
            Msg::Voices(voices) => self.voices_arrived(now, voices, inputs),
            Msg::VoiceCatalog(listed) => {
                let own = self.caps.own_voice.clone();
                let voices = controls::voices_by_language(&listed, own.as_deref());
                debug!(stream_id = %self.stream_id, voices = listed.len(), languages = voices.len(), "interaction voice list read");
                self.caps.catalog = listed;
                self.voices_asked = true;
                self.voices_arrived(now, voices, inputs);
            }
            Msg::VoicesComing => self.voices_owed = true,
            Msg::SttOpened { language, stt } => {
                self.stt_opening = None;
                match stt {
                    Ok(stt) => self.stt_next = Some((language, stt)),
                    Err(error) => {
                        warn!(stream_id = %self.stream_id, %language, %error, "interaction recogniser not opened in the new language");
                        let gone = |code: &String| controls::same_language(code, &language);
                        self.caps.hears.retain(|code| !gone(code));
                    }
                }
                self.settle_owed(now, inputs);
            }
            Msg::SipTransfer(transfer) => self.sip = Some(transfer),
            Msg::RecordingPause(recording) => self.recording = Some(recording),
            Msg::Person(answer) => self.person_answered(now, answer, inputs),
            Msg::RunAlive(run) => {
                self.first_token_due.remove(&run);
            }
            // An orchestrator's `conversation_control`: applied now, the result to its tool.
            Msg::Run(_, RunEvent::Control { control, reply }) => {
                let applied = self.control(now, control, inputs);
                // A switch that waits for the voice list or a recogniser answers when applied.
                if applied.is_null() && self.language_owed.is_some() {
                    self.owed_reply = Some(reply);
                } else {
                    let _ = reply.send(applied);
                }
            }
            Msg::ToolRound(run, round) => self.tool_round(run, round),
            Msg::Run(run, event) => {
                self.first_token_due.remove(&run);
                if event.is_last() {
                    self.runs.remove(&run);
                    self.run_at.remove(&run);
                    self.traces.remove(&run);
                }
                self.task(now, TaskCmd::Run { run, event }, inputs);
            }
            Msg::TaskAnswer {
                request_id,
                outcome,
            } => inputs.push_back(PolicyInput::ClientAnswer {
                request: HitlId(request_id),
                outcome,
            }),
            Msg::TaskCancel { task_id } => inputs.push_back(PolicyInput::ClientCancel {
                task: TaskId(task_id),
            }),
            Msg::TaskSeen { task_id } => inputs.push_back(PolicyInput::TaskSeen {
                task: TaskId(task_id),
            }),
            Msg::Narrated { item, line, ms } => {
                debug!(
                    stream_id = %self.stream_id,
                    item = item.0,
                    ms,
                    line = line.as_deref().unwrap_or_default(),
                    "interaction narrator"
                );
                bridge::observe_turn_stage_ms("narrator", "interaction", ms as f64);
                inputs.push_back(PolicyInput::Narrated { item, line });
                self.aside_done();
            }
            Msg::WordsJudged { text, finished } => {
                debug!(stream_id = %self.stream_id, finished, "interaction held words read");
                return self.duplex.on_words(now, &text, finished, floor, expected);
            }
            Msg::FrontPlaced(placement) => {
                info!(
                    stream_id = %self.stream_id,
                    server = %placement.server().url,
                    pair = placement.pair(),
                    "interaction front pair taken on another server"
                );
                self.lane = front_model::Lane::pinned(placement.pair());
                self.placement = Some(*placement);
                self.front_down = None;
                self.replace_asked = None;
                self.front_back();
                // The new pair has this session's prompt in neither slot.
                self.warm_front();
            }
            Msg::Read { turn, label, ms } => {
                debug!(
                    stream_id = %self.stream_id,
                    turn = turn.0,
                    ms,
                    label = label.as_deref().unwrap_or_default(),
                    "interaction front reading"
                );
                bridge::observe_turn_stage_ms("front_read", "interaction", ms as f64);
                inputs.push_back(PolicyInput::Read { turn, label });
                self.aside_done();
            }
            Msg::Language { turn, code, ms } => {
                debug!(stream_id = %self.stream_id, turn = turn.0, ms, code = code.as_deref().unwrap_or_default(), "interaction front language");
                bridge::observe_turn_stage_ms("front_language", "interaction", ms as f64);
                // A reading stopped with its turn (`FrontWait::abort`) ended its aside then.
                if let Some(wait) = self.front.as_mut().filter(|wait| wait.turn == turn)
                    && wait.asked.take().is_some()
                {
                    let pack = persona::language();
                    wait.other =
                        Some(code.is_some_and(|code| !controls::same_language(&code, pack)));
                    self.aside_done();
                    self.language_known(now, inputs);
                }
            }
            Msg::Number {
                mut control,
                label,
                ms,
            } => {
                debug!(stream_id = %self.stream_id, turn = control.turn, ms, label = label.as_deref().unwrap_or_default(), "interaction front number reading");
                bridge::observe_turn_stage_ms("front_read", "interaction", ms as f64);
                self.aside_done();
                // A newer turn of the caller's is its own.
                let latest = self.turn.as_ref().map(|turn| turn.id.0);
                if latest == control.turn {
                    if label.as_deref() == Some("line") {
                        control.value = None;
                    }
                    self.control(now, control, inputs);
                }
            }
            Msg::Dtmf(digit) => inputs.push_back(PolicyInput::Dtmf { digit }),
            Msg::Delivered(request) => inputs.push_back(PolicyInput::Delivered { request }),
            Msg::PlaybackPosition {
                utterance_id,
                played_ms,
            } => lanes.extend(self.report_played(now, &utterance_id, played_ms)),
            Msg::Update(patch) => {
                self.update(&patch);
                // Listening sounds turned on mid-session: their bank is made now.
                if self.cfg.duplex.backchannel != Tri::Off {
                    self.build_bank(now, lanes);
                }
            }
            // A blank line has no audio and no completion: it would hold the one synthesis
            // slot until the watchdog.
            Msg::ClientSpeak(text) if text.trim().is_empty() => {}
            Msg::ClientSpeak(text) => inputs.push_back(PolicyInput::ClientSpeak { text }),
            Msg::ClientClear => {
                let cut = DuplexAction::Cut(CutCause::Client);
                lanes.extend(self.floor.on_duplex(now, &cut));
            }
            Msg::Outcome(outcome) => {
                info!(
                    stream_id = %self.stream_id,
                    task_id = %outcome.task_id.0,
                    state = ?outcome.state,
                    "interaction task of an earlier connection ended"
                );
                inputs.extend(outcome.events(now).into_iter().map(PolicyInput::Task));
            }
            Msg::Ready => {
                // The client has `ready`: from now on a quiet session sends a heartbeat.
                self.last_sent = Some(now);
                if self.warm {
                    inputs.push_back(PolicyInput::Ready);
                }
                // The listening sounds, made once in the session's voice, and the lines said when
                // a service fails (chapter 10).
                if self.cfg.duplex.backchannel != Tri::Off {
                    self.build_bank(now, lanes);
                }
                self.build_failure_lines(now, true, &[], lanes);
                // The language this caller asked for on an earlier call (caller memory), once
                // the client listens.
                let wish = self.cfg.front.language_wish.take();
                let wish = wish.filter(|code| !controls::same_language(code, &self.state.language));
                let wish = wish.and_then(|code| {
                    Control::new(
                        ControlKind::Language,
                        Some(&code),
                        ControlSource::Memory,
                        None,
                    )
                    .ok()
                });
                if let Some(control) = wish {
                    self.control(now, control, inputs);
                }
                self.warm_front();
            }
            Msg::FrontWarm => {
                // The interims' slot is warmed only while the caller is quiet and no turn has
                // been decided: read beside the first turn's decision, its 3,250 tokens took
                // 1.6 s on the loaded GPU and that decision past its budget (live MA3, MA1, D1:
                // "Sorry, I didn't catch that." to the first request). Otherwise the next agent
                // line primes it (`prime_line`).
                let quiet = self.front.is_none() && !self.floor.user_speaking();
                if quiet
                    && self.front_asked == 0
                    && let Some((server, _)) = self.front_model()
                {
                    let persona = self.persona.clone();
                    let warming = self.spawn_front(async move {
                        front_model::warm_interims(&server, &persona).await;
                    });
                    self.warming = Some(warming.abort_handle());
                }
            }
            Msg::ClipChecked {
                line,
                recorded,
                clip,
            } => match clip {
                Ok(clip) => {
                    if let Some(bank) = &mut self.bank {
                        bank.passed(line, clip);
                    }
                    self.install_bank();
                }
                Err(reason) => {
                    let text = self.bank.as_ref().map(|bank| bank.line(line).1.to_owned());
                    info!(stream_id = %self.stream_id, ?text, recorded, %reason, "interaction listening sound refused");
                    if recorded {
                        self.render_clip(now, line, lanes);
                    } else {
                        self.clip_failed(now, line, lanes);
                    }
                }
            },
            // The loop ends on it before a step.
            Msg::Shutdown { .. } => {}
            // A provider session's messages; this actor never gets them.
            Msg::Provider(_) | Msg::LineSent { .. } | Msg::LineEnded { .. } => {}
        }
        self.duplex.tick(now, floor, expected)
    }

    /// Step 2 for a pump event.
    fn on_pump(
        &mut self,
        now: Ms,
        event: PumpEvent,
        lanes: &mut Vec<LaneEvent>,
        failed: &mut Option<UttId>,
    ) {
        debug!(stream_id = %self.stream_id, ?event, "interaction pump event");
        match event {
            // A listening sound: on the wire, and the lane is free again when it has played.
            PumpEvent::Started { utt, .. } if Some(utt) == self.clip => {
                self.segment(now, "start", utt, "backchannel", Kind::Clip, None);
                return;
            }
            PumpEvent::Idle { utt, .. } if Some(utt) == self.clip => {
                self.clip = None;
                self.segment(now, "end", utt, "backchannel", Kind::Clip, None);
                return;
            }
            // A line said because a service failed (chapter 10).
            PumpEvent::Started { utt, .. } if self.failure_at(utt).is_some() => {
                self.segment(now, "start", utt, "front", Kind::Failure, None);
                // The agent is speaking: no holding line is due over it, nor right after it.
                self.policy.spoke_apart(now);
                return;
            }
            PumpEvent::Idle { utt, .. } | PumpEvent::Failed { utt }
                if self.failure_at(utt).is_some() =>
            {
                if let Some(at) = self.failure_at(utt) {
                    let (_, said) = self.failure_playing.remove(at);
                    // In the heard timeline, so the next line does not say its words again.
                    if matches!(event, PumpEvent::Idle { .. }) {
                        self.floor.said_apart(now, said);
                        self.policy.spoke_apart(now);
                    }
                }
                self.segment(now, "end", utt, "front", Kind::Failure, None);
                if self.exiting.is_some() && self.failure_playing.is_empty() {
                    self.finish_exit(now);
                }
                return;
            }
            PumpEvent::Idle {
                utt, delivered_ms, ..
            } => {
                if let Some(spoken) = self.live.iter_mut().find(|spoken| spoken.utt == utt) {
                    spoken.delivered_ms = delivered_ms;
                }
            }
            _ => {}
        }
        match event {
            PumpEvent::SynthDone { utt } | PumpEvent::Failed { utt } => {
                self.clip_rendered(now, utt, lanes);
            }
            _ => {}
        }
        match event {
            PumpEvent::SynthDone { utt } if self.synth == Some(utt) => self.synth = None,
            PumpEvent::Failed { utt } => {
                bridge::record_degraded("interaction", "tts_failed");
                self.synth.take_if(|synth| *synth == utt);
                *failed = Some(utt);
            }
            // The whole utterance has played: its length calibrates the speaking rate.
            PumpEvent::Idle {
                utt, delivered_ms, ..
            } if self.synth != Some(utt) => {
                if let Some(spoken) = self.live.iter().find(|spoken| spoken.utt == utt) {
                    self.rate.observe(spoken.text.chars().count(), delivered_ms);
                }
            }
            PumpEvent::FirstDuckedFrame { at } => {
                if let Some(onset) = self.barge_onset.take() {
                    let ms = at.saturating_sub(onset) as f64;
                    bridge::observe_turn_stage_ms("duck", "interaction", ms);
                }
            }
            _ => {}
        }
        let events = self.floor.on_pump(now, event);
        self.error_owed |= events.iter().any(is_failure);
        lanes.extend(events);
    }

    /// Ask the text end-of-turn model whether the words a turn is held on are finished. A
    /// reading later than the hold it could end ends nothing, and a failed one is "not
    /// finished": the hold runs out as it would without the model.
    fn judge_words(&self, turn: TurnId, text: String) {
        let Some(model) = self.text_turn.clone() else {
            return;
        };
        let (tx, stream_id) = (self.tx.clone(), self.stream_id.clone());
        tokio::spawn(async move {
            let read = async { model.read().await.is_turn_complete(&text).await };
            let finished = match tokio::time::timeout(WORDS_JUDGE_WAIT, read).await {
                Ok(Ok(finished)) => finished,
                Ok(Err(error)) => {
                    debug!(%stream_id, turn = turn.0, %error, "interaction text end-of-turn model failed");
                    false
                }
                Err(_) => false,
            };
            if let Some(tx) = tx.upgrade() {
                let _ = tx.send(Msg::WordsJudged { text, finished }).await;
            }
        });
    }

    /// The effects of a duplex action, and the policy input it becomes.
    fn on_duplex(&mut self, now: Ms, action: DuplexAction, inputs: &mut VecDeque<PolicyInput>) {
        match action {
            DuplexAction::Backchannel { text, voiced_ms } => {
                self.backchannel(now, &text, voiced_ms)
            }
            DuplexAction::Gate(cmd) => self.ctl.gate(cmd),
            // The floor hands it back as `FloorCmd::Cut`, and the cut opens the gate. With
            // nothing left to cut only the gate is left: a paused pump must not stay paused.
            DuplexAction::Cut(cause) => {
                if cause == CutCause::Interrupt {
                    self.barge_cut_at = Some(now);
                }
                if !self.floor.cut_pending() {
                    self.ctl.gate(GateCmd::Open);
                }
            }
            // The floor returns what a barge-in cut: a cut relay is not queued again, an
            // unasked question is offered again once the turn is settled, the task runs on.
            DuplexAction::Classified(class) => {
                debug!(stream_id = %self.stream_id, ?class, "interaction overlap classified")
            }
            DuplexAction::UserSpeaking(on) => {
                // A caller who speaks ends a hold: their turn is the check-in.
                if on && self.hold_until.is_some() {
                    self.release_hold(now, inputs);
                    self.updated(Vec::new(), Vec::new());
                }
                inputs.push_back(PolicyInput::UserSpeaking { on })
            }
            DuplexAction::Interim { turn, text } => self.ask_interim(now, turn, text),
            DuplexAction::Eager { turn, text } => {
                debug!(stream_id = %self.stream_id, turn = turn.0, "interaction eager end of turn");
                self.mark = self.ledger.len();
                inputs.push_back(PolicyInput::Eager { turn, text });
            }
            DuplexAction::EagerWithdrawn { turn } => {
                debug!(stream_id = %self.stream_id, turn = turn.0, "interaction eager end withdrawn");
                inputs.push_back(PolicyInput::EagerWithdrawn { turn })
            }
            DuplexAction::JudgeWords { turn, text } => self.judge_words(turn, text),
            DuplexAction::ClaimSegment => {
                let vm = self.vm.clone();
                tokio::spawn(async move {
                    vm.force_speech_final("interaction_endpoint").await;
                });
            }
            DuplexAction::FinalizeStt { upload } => {
                let (vm, tx) = (self.vm.clone(), self.tx.clone());
                tokio::spawn(async move {
                    let msg = match vm.finalize_utterance(upload).await {
                        Err(error) => Msg::SttError(error.to_string()),
                        Ok(false) if upload => Msg::SttEmpty,
                        Ok(_) => return,
                    };
                    if let Some(tx) = tx.upgrade() {
                        let _ = tx.send(msg).await;
                    }
                });
            }
            DuplexAction::EndOfTurn(mut turn) => {
                // A listening sound never plays into the agent's reply.
                self.stop_clip(now);
                self.turn_clips = 0;
                let (confidence, language) = (self.confidence.take(), self.language.take());
                if turn.source != EotSource::Typed {
                    let floor = persona::guard_lexicon().rule_layer_cues.unclear_confidence;
                    turn.unclear = confidence.is_some_and(|sure| f64::from(sure) < floor);
                }
                // After a switch to another language (chapter 9) the caller's turns in it are
                // the conversation's own. The recogniser's own language field is a model's
                // reading; the words are read by the front (`wait` `foreign`), not a word list.
                let pack_language =
                    controls::same_language(&self.state.language, persona::language());
                turn.foreign = !turn.text.is_empty()
                    && pack_language
                    && rules::foreign_language(language.as_deref());
                let language_named = language.is_some_and(|code| !code.trim().is_empty());
                debug!(
                    stream_id = %self.stream_id,
                    turn = turn.turn.0,
                    source = ?turn.source,
                    class = ?turn.class,
                    attribution = ?turn.attribution,
                    eager_same = ?turn.eager_same,
                    "interaction end of turn"
                );
                if turn.source == EotSource::SttFailed {
                    bridge::record_degraded("interaction", "stt_timeout");
                }
                let said = !turn.text.is_empty() && !turn.suspected_echo;
                if said {
                    // The turn's record: the root of its trace (spec 029 D-9). The previous
                    // turn's ends with the runs it started.
                    let span = self
                        .background
                        .turn_span(&self.stream_id, turn.turn.0, &turn.text);
                    self.record = Some(TurnRecord::open(span, turn.turn, turn.at));
                }
                self.mark = self.ledger.len();
                // A value that is never taken by voice is kept out of every later run.
                let categories = self.persona.sensitive_categories();
                if said && rules::sensitive_value(&turn.text, categories).is_none() {
                    self.ledger.push(ChatMessage::user(&turn.text));
                }
                self.turn = Some(Turn {
                    id: turn.turn,
                    e: turn.at,
                    speech_end: turn.acoustic_end,
                    addressed: turn.class != Class::TalkingToOthers,
                    typed: turn.source == EotSource::Typed,
                    language_named,
                    unclear: turn.unclear,
                    // Only a turn whose words did cut the agent: a barge-in left undecided
                    // at the end of the turn lets the agent go on, and the turn's own stop
                    // cuts it (live S1: "Stop talking." whose interims did not cut was applied
                    // as a stop that cut nothing, the line played on).
                    barged: turn.class == Class::Interrupt
                        && self
                            .barge_cut_at
                            .is_some_and(|at| turn.onset_at.is_none_or(|onset| at >= onset)),
                    act: None,
                    first_audible_owed: said,
                    answer_owed: said,
                });
                inputs.push_back(PolicyInput::EndOfTurn(turn));
            }
        }
    }

    /// Step 3: one command through the registry. Its events wait in `inputs` for the policy.
    fn task(&mut self, now: Ms, cmd: TaskCmd, inputs: &mut VecDeque<PolicyInput>) {
        for effect in self.registry.apply(now, cmd) {
            match effect {
                TaskEffect::StartRun {
                    run,
                    task,
                    words,
                    agent,
                    trace_id,
                    follow_up,
                    handle,
                } => {
                    let follow = follow_up.then_some(handle);
                    self.start_run(run, task.as_ref(), words, agent, trace_id, follow)
                }
                TaskEffect::CancelRun { run } => {
                    self.first_token_due.remove(&run);
                    if let Some(cancel) = self.runs.remove(&run) {
                        cancel.cancel();
                    }
                }
                TaskEffect::Store(view) => tasks::store().write(&self.stored, view),
                TaskEffect::Emit(event) => {
                    if matches!(event.kind, TaskEventKind::Created { .. }) {
                        self.marks.insert(event.task.clone(), self.mark);
                        // A caller's request: the task of the turn that made it.
                        if event.lineage.parent_id.is_none()
                            && let Some(record) = &mut self.record
                        {
                            record.task(&event.task_ref.0);
                        }
                    }
                    // The result is published the moment it exists, whatever is said of it.
                    let output = self.registry.output(&event.task);
                    if let Some(wire) = events::task_output(&event, output) {
                        self.send_task(&wire, None);
                    }
                    if let (Some(_), TaskEventKind::Completed { text, .. }) =
                        (&event.lineage.parent_id, &event.kind)
                    {
                        // A child's whole result stays in the call's history: "tell me more"
                        // relays it, as it relays an answer that was said.
                        let title = self.registry.title(&event.task).unwrap_or_default();
                        let note = format!("{EARLIER_RESULT} {title}: {}", text.trim());
                        self.ledger.push(ChatMessage::user(note));
                    }
                    if let Some(outcome) = Outcome::of(&event) {
                        tasks::store().result(&self.stored, &outcome);
                        if let Some(back_by) = self.gone {
                            // The caller is gone: the result is theirs when they come back
                            // in time, and the push address's otherwise.
                            let now = tokio::time::Instant::now();
                            let left = back_by.map(|until| until.saturating_duration_since(now));
                            self.deliver(outcome, left);
                        }
                    }
                    inputs.push_back(PolicyInput::Task(event));
                }
            }
        }
    }

    /// What signs a push to `tasks.push_url`: the session's own `push_secret` (a client's
    /// address, `allow_client_push`), else the server's secret.
    fn push_secret(&self) -> Option<String> {
        let own = self.cfg.tasks.push_secret.clone();
        own.or_else(|| self.server.tool_secret.clone())
    }

    /// What a task came to after the session ended, for a caller who comes back or for the
    /// push. A completed task's carries its short spoken update when the narrator words one in
    /// its budget and it passes the checks of an update said in the call; else only the
    /// summary (there is no template, chapter 9).
    fn deliver(&mut self, mut outcome: Outcome, left: Option<Duration>) {
        let secret = self.push_secret();
        let mut stored = self.stored.clone();
        stored.push_url.clone_from(&self.cfg.tasks.push_url);
        if outcome.state != TaskState::Completed {
            return tasks::store().deliver(&stored, outcome, left, secret);
        }
        let title = self.registry.title(&outcome.task_id).unwrap_or_default();
        let persona = self.persona.clone();
        let front = self.front_model().map(|(server, _)| server);
        let budget = Duration::from_millis(self.cfg.front.total_budget_ms.into());
        self.spawn_front(async move {
            let line = match &front {
                Some(server) => {
                    let summary = &outcome.summary;
                    let language = persona.language();
                    let narrated = front_model::narrate(
                        server,
                        &[],
                        &title,
                        summary,
                        None,
                        language,
                        None,
                        budget,
                    );
                    narrated.await
                }
                None => None,
            };
            // No template (chapter 9): the narrator's line, else the summary alone.
            let say =
                policy::spoken_update(&persona, &title, &outcome.summary, "", line, false, &[]);
            outcome.update = say.ok().and_then(|say| persona.resolve(&say, 0));
            tasks::store().deliver(&stored, outcome, left, secret);
        });
    }

    /// A registry command after the session ended: its events go to the client, nobody speaks.
    fn task_unspoken(&mut self, now: Ms, cmd: TaskCmd) {
        let mut events = VecDeque::new();
        self.task(now, cmd, &mut events);
        for event in events {
            if let PolicyInput::Task(event) = event {
                self.emit_task(&event, &[]);
            }
        }
    }

    /// The ledger's first `end` lines, each request followed by the tool rounds of its runs;
    /// without the requests `left_out` names, and their rounds.
    fn history(&self, end: usize, left_out: impl Fn(&ChatMessage) -> bool) -> Vec<ChatMessage> {
        let mut history = Vec::with_capacity(end);
        for (n, line) in self.ledger[..end].iter().enumerate() {
            if left_out(line) {
                continue;
            }
            history.push(line.clone());
            let rounds = self.tool_rounds.iter().filter(|(at, _)| *at == n);
            history.extend(rounds.flat_map(|(_, round)| round.iter().cloned()));
        }
        history
    }

    /// Where a run's history ends: at the line of its request. A request settled by a later
    /// turn (the caller's "A second one, please." to "change it, or a second one?", a turn
    /// often kept out of the ledger as the question's echo) starts from where it was said a
    /// few lines before, as any request: with the question in its history and the request
    /// again as its words, the 3B background read it as asked again and booked nothing (live
    /// D2).
    fn request_line(&self, mark: usize, request: Option<&String>) -> usize {
        let at = mark.min(self.ledger.len());
        let said =
            |line: &ChatMessage| line.role == MessageRole::User && line.content.as_ref() == request;
        if self.ledger.get(at).is_some_and(said) {
            return at;
        }
        let back = at.saturating_sub(REQUEST_LINES_BACK);
        (self.ledger[back..at].iter().rposition(said)).map_or(at, |n| back + n)
    }

    /// A run's tool round, kept after the line of its request (when the ledger has one: a
    /// private value is never in it).
    fn tool_round(&mut self, run: RunId, round: Vec<ChatMessage>) {
        let Some(&at) = self.run_at.get(&run) else {
            return;
        };
        let request = self.ledger.get(at);
        if round.is_empty() || !request.is_some_and(|line| line.role == MessageRole::User) {
            return;
        }
        if self.tool_rounds.len() == TOOL_ROUNDS_KEPT {
            self.tool_rounds.pop_front();
        }
        self.tool_rounds.push_back((at, round));
    }

    /// `follow`: a follow-up of a finished task, with the handle that continues it.
    fn start_run(
        &mut self,
        run: RunId,
        task: Option<&TaskId>,
        words: Vec<String>,
        agent: Option<String>,
        trace_id: String,
        follow: Option<Option<String>>,
    ) {
        let Some(tx) = self.tx.upgrade() else {
            return;
        };
        // A child task runs on its agent and hears only its request; so does a follow-up,
        // on the agent of the task it follows (the session's own backend when that is it).
        let backend = match &agent {
            Some(name) => self
                .agents
                .get(name)
                .cloned()
                .or_else(|| follow.is_some().then(|| self.background.clone())),
            None => Some(self.background.clone()),
        };
        let Some(backend) = backend else {
            let error = format!(
                "{} is not an agent of this session",
                agent.unwrap_or_default()
            );
            let _ = tx.try_send(Msg::Run(run, RunEvent::Failed { error }));
            return;
        };
        // What the agent hears: only its request, for a child and for a follow-up.
        let alone = agent.is_some();
        let child = alone && follow.is_none();
        // A task's runs all start from what was said before its request.
        let mark = task.and_then(|task| self.marks.get(task)).copied();
        let cancel = CancellationToken::new();
        self.runs.insert(run, cancel.clone());
        // The session's background gives its first sign of life in time, or it stalls
        // (chapter 10). An agent's child is the agent's business (chapter 8).
        if agent.is_none() {
            let due = self.ctl.now_ms() + self.stall_after();
            self.first_token_due.insert(run, due);
        }
        let at = self.request_line(mark.unwrap_or(self.mark), words.first());
        self.run_at.insert(run, at);
        // An orchestrator acts on its own request only. The caller's other live requests have
        // runs of their own: shown to it, it started them again (live, gpt-oss-20b booked a
        // flight twice).
        let others = if self.agents.is_empty() {
            Vec::new()
        } else {
            self.registry.live_requests()
        };
        let theirs = |message: &ChatMessage| {
            message.role == MessageRole::User
                && (message.content.as_deref()).is_some_and(|said| others.contains(&said))
        };
        let history = self.history(at, theirs);
        // A task's revision is its request and the corrections so far.
        let revision = u32::try_from(words.len()).unwrap_or(u32::MAX);
        let task = task.map(|task| (task.clone(), revision));
        // The caller turn the run records itself in (spec 029 D-9): a child task's run in its
        // parent's, any other in the latest caller turn's, which started it.
        let turn = if child {
            let parent = task
                .as_ref()
                .and_then(|(task, _)| self.registry.parent_of(task));
            (self.traces.values())
                .find(|(task, _)| task.is_some() && *task == parent)
                .map(|(_, trace)| trace.clone())
        } else {
            self.record.as_ref().map(TurnRecord::trace)
        };
        if let Some(trace) = &turn {
            let id = task.as_ref().map(|(task, _)| task.clone());
            self.traces.insert(run, (id, trace.clone()));
        }
        if let Some(task) = &task {
            self.answers.insert(task.clone(), (run, String::new()));
            // The answers of the latest runs are the ones still heard.
            self.answers
                .retain(|_, (kept, _)| kept.0 + RECENT_KEPT as u64 >= run.0);
        }
        let job = Job {
            run,
            task,
            history: if alone { Vec::new() } else { history },
            words,
            api_key: self.credential.as_ref().map(SessionCredential::current),
            plain: self.plain,
            // Stepped down, the run is the plain path's: nothing of the front's.
            context: if self.plain || alone {
                String::new()
            } else {
                // The conversation state as a system note once a control changed it.
                let note = (self.state.version > 0).then(|| self.state.note());
                let context = self.persona.caller_context(&self.cfg.front);
                [Some(context), note]
                    .into_iter()
                    .flatten()
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n\n")
            },
            trace_id,
            child,
            handle: follow.flatten(),
            state: (!self.plain).then(|| self.state.to_json()),
            turn,
        };
        backend.start(job, tx, cancel);
    }

    /// Step 4: one input through the policy, then what the policy asks for.
    fn decide(
        &mut self,
        now: Ms,
        input: PolicyInput,
        failed: Option<UttId>,
        inputs: &mut VecDeque<PolicyInput>,
    ) {
        let task_event = match &input {
            PolicyInput::Lane(event) => {
                if self.hold(now, event, inputs) {
                    return;
                }
                self.on_lane(event, failed);
                None
            }
            PolicyInput::Task(event) => Some(event.clone()),
            _ => None,
        };
        // Keys answer only a caller on a phone line, and only when the session takes them.
        let phone = self.ctl.phone_leg();
        self.policy.set_keypad(self.cfg.hitl_dtmf && phone);
        // A caller with a screen is pointed at it; a phone line has none.
        self.policy
            .set_floor(self.floor.quiet(), self.cfg.hitl_client && !phone);
        let left = self.floor.chars_left() as f64 / self.rate.chars_per_ms().max(1e-3);
        let said = self.floor.recent_agent_lines();
        self.policy
            .set_speech(left as Ms, self.floor.relay_items(), said);
        let actions = self.policy.step(now, input, &self.registry.board());
        if let Some(event) = task_event {
            // What was not said of a stopped or cancelled answer is never heard.
            let stopped = matches!(
                event.kind,
                TaskEventKind::AnswerStopped { .. } | TaskEventKind::Cancelled { .. }
            );
            if stopped {
                self.heard_answer(Some(&(event.task.clone(), event.revision)), "", true);
            }
            self.emit_task(&event, &actions);
        }
        for action in actions {
            match action {
                PolicyAction::RequestFront {
                    turn,
                    text,
                    allowed,
                    speculate,
                } => self.ask_front(now, turn, text, allowed, speculate, inputs),
                PolicyAction::Settled { turn, act, source } => {
                    // Who decided the turn is on no wire event: the log is where it shows.
                    debug!(
                        stream_id = %self.stream_id,
                        turn = turn.0,
                        act = act.as_str(),
                        ?source,
                        "interaction turn settled"
                    );
                    self.floor.on_settled(now, turn);
                    bridge::record_turn_outcome("interaction", act.as_str());
                    if let Some(record) = self.record.as_ref().filter(|record| record.turn == turn)
                    {
                        record.settled(act);
                    }
                    if let Some(latest) = self.turn.as_mut().filter(|latest| latest.id == turn) {
                        latest.act = Some(act);
                        let ms = now.saturating_sub(latest.e) as f64;
                        bridge::observe_turn_stage_ms("front_act", "interaction", ms);
                    }
                }
                // A hold keeps the agent's lines back until it is over; what the caller
                // asked for is said.
                PolicyAction::Speak(utterance)
                    if self.hold_until.is_some()
                        && !matches!(utterance.kind, Kind::Notice | Kind::Warm | Kind::Client) =>
                {
                    self.held_lines.push(utterance);
                }
                PolicyAction::Speak(utterance) => {
                    let lanes = self.floor.offer(now, utterance);
                    inputs.extend(lanes.into_iter().map(PolicyInput::Lane));
                }
                PolicyAction::Drop { item } => {
                    let lanes = self.floor.drop_item(now, item);
                    inputs.extend(lanes.into_iter().map(PolicyInput::Lane));
                }
                PolicyAction::Task(cmd) => self.task(now, cmd, inputs),
                PolicyAction::Emit(event) => self.send(&event, MessageClass::Critical),
                PolicyAction::FirstSentenceAfterE { ms } => {
                    bridge::observe_turn_stage_ms("e_to_first_sentence", "interaction", ms.into())
                }
                PolicyAction::StepDown => self.step_down(
                    "interaction_language_step_down",
                    "The caller keeps speaking a language interaction mode has no pack for. The \
                     session continues as a plain conversation with an empty history.",
                    "language_step_down",
                ),
                PolicyAction::Narrate(narration) => self.narrate(narration, inputs),
                PolicyAction::Read(reading) => self.read(reading, inputs),
            }
        }
    }

    /// The front reads what the caller's turn means against what it may answer (chapter 9).
    /// Without the front the turn has no label: the policy asks again, or takes no offer.
    fn read(&mut self, reading: Reading, inputs: &mut VecDeque<PolicyInput>) {
        let Some((server, tx)) = self.front_model() else {
            inputs.push_back(PolicyInput::Read {
                turn: reading.turn,
                label: None,
            });
            return;
        };
        let budget = Duration::from_millis(self.cfg.front.total_budget_ms.into());
        self.asides += 1;
        self.spawn_front(async move {
            let asked = tokio::time::Instant::now();
            let label = front_model::read(&server, &reading, budget).await;
            let ms = u64::try_from(asked.elapsed().as_millis()).unwrap_or(u64::MAX);
            let turn = reading.turn;
            let _ = tx.send(Msg::Read { turn, label, ms }).await;
        });
    }

    /// The front words a task's update from the last turns of the call. There is no template
    /// (chapter 9): while the front is unavailable the update waits for it.
    fn narrate(&mut self, narration: Narration, _inputs: &mut VecDeque<PolicyInput>) {
        let Some((server, tx)) = self.front_model() else {
            self.unnarrated.push(narration);
            return;
        };
        // The last two lines of the call, the caller's and the agent's.
        let spoken = self.ledger.iter().rev().filter_map(|message| {
            let said = message
                .content
                .clone()
                .filter(|_| message.role != MessageRole::System)?;
            let caller = message.role == MessageRole::User;
            let note = said.starts_with(EARLIER_RESULT) || said.starts_with(NOTE_START);
            (!(caller && note)).then_some((caller, said))
        });
        let mut turns: Vec<(bool, String)> = spoken.take(2).collect();
        turns.reverse();
        // Its last line did not give its own result, and was most likely one of these lines
        // copied (live, take 15: the writer task's update, said as the browser task's): asked
        // again, the front sees the task and its result alone.
        if matches!(narration.retry, Some("repeated" | "no_result")) {
            turns.clear();
        }
        // The front's budget, for an update as for a follow-up question: there is no template
        // to fall back to (chapter 9) and an update without a line waits for its next slot. A
        // budget of its own (1.2 s) ran out in 24 of the 53 narrations of the r4-verify runs
        // (the updates went unsaid); alone the 2B front's line took 0.5 to 1.1 s, and it
        // waited up to 0.4 s for its slot (r5-decide).
        let budget = Duration::from_millis(self.cfg.front.total_budget_ms.into());
        let persona = self.persona.clone();
        self.asides += 1;
        self.spawn_front(async move {
            let asked = tokio::time::Instant::now();
            let Narration {
                item,
                title,
                summary,
                question,
                retry,
            } = narration;
            let question = question.as_deref();
            let language = persona.language();
            let line = front_model::narrate(
                &server, &turns, &title, &summary, question, language, retry, budget,
            )
            .await;
            let ms = u64::try_from(asked.elapsed().as_millis()).unwrap_or(u64::MAX);
            let _ = tx.send(Msg::Narrated { item, line, ms }).await;
        });
    }

    /// The caller keeps speaking a language the pack does not have. The session goes on as the
    /// plain path would: every turn straight to the background, with no front, no tools and
    /// none of the call so far in its history. The client is told once.
    fn step_down(&mut self, code: &str, message: &str, reason: &'static str) {
        self.plain = true;
        if let Some(wait) = self.front.take()
            && wait.abort()
        {
            self.aside_done();
        }
        if let Some(interim) = self.interim.take() {
            interim.request.abort();
        }
        self.ledger.clear();
        // A run of before keeps no place in the new ledger (its tool round is not kept).
        self.tool_rounds.clear();
        self.run_at.clear();
        self.marks.clear();
        self.mark = 0;
        self.policy.step_down();
        bridge::record_degraded("interaction", reason);
        info!(stream_id = %self.stream_id, reason, "interaction stepped down");
        let warning = OutgoingMessage::ConfigWarning {
            code: code.to_owned(),
            message: message.to_owned(),
            detail: Some(serde_json::json!({
                "surface": "ws", "pack": persona::language(), "reason": reason
            })),
        };
        if let Ok(json) = serde_json::to_string(&warning) {
            let _ = self.events.0.send((json, MessageClass::Critical, None));
        }
    }

    /// The front model and the inbox its task reports to; `None` while the model the gateway
    /// runs is not ready (chapter 9: no new decisions until it is back).
    fn front_model(&mut self) -> Option<(FrontModelServer, mpsc::Sender<Msg>)> {
        if self.plain {
            return None;
        }
        let server = match &self.placement {
            Some(placement) if placement.lost() => {
                self.replace_front();
                return None;
            }
            Some(placement) => placement.server().clone(),
            None => self.server.front_model.clone()?,
        };
        match front_supervisor::gate(self.front_down, front_supervisor::status()) {
            front_supervisor::Gate::Use => {}
            // Started again since this session lost it: the model decides again.
            front_supervisor::Gate::Back => {
                self.front_down = None;
                self.front_back();
            }
            front_supervisor::Gate::Lost(generation) => {
                self.front_down = Some(generation);
                self.front_lost();
                return None;
            }
            front_supervisor::Gate::Wait => return None,
        }
        Some((server, self.tx.upgrade()?))
    }

    /// The session's pair can no longer be used (spec 029 SF-2, SF-5): its lease could not be
    /// proven, or its server was lost or restarted. The front is down for the session until a
    /// pair elsewhere is held; one is asked for every [`REPLACE_EVERY_MS`], and the mode ends
    /// as for a lost front when none is held within the restart window.
    fn replace_front(&mut self) {
        let now = self.ctl.now_ms();
        if self.front_down.is_none() {
            warn!(stream_id = %self.stream_id, "interaction front pair lost; asking for another");
            self.front_down = Some(0);
            self.front_lost();
        }
        if self
            .replace_asked
            .is_some_and(|at| now < at + REPLACE_EVERY_MS)
        {
            return;
        }
        let (Some(pool), Some(tx)) = (front_pool::get(), self.tx.upgrade()) else {
            return;
        };
        self.replace_asked = Some(now);
        let session = self.stream_id.clone();
        tokio::spawn(async move {
            if let Ok(placement) = pool.place(&session).await {
                let _ = tx.send(Msg::FrontPlaced(Box::new(placement))).await;
            }
        });
    }

    /// The model front failed this session: it is reported, and the mode ends when it is not
    /// back within the supervisor's restart window (chapter 9).
    fn front_lost(&mut self) {
        if let Some((_, generation)) = front_supervisor::status() {
            self.front_down.get_or_insert(generation);
        }
        if self.front_lost_at.is_none() {
            self.front_lost_at = Some(self.ctl.now_ms());
            warn!(stream_id = %self.stream_id, "interaction front model unavailable");
        }
        // A turn waits for it: the caller hears a stall line (chapter 10).
        if self.front.is_some() || self.undecided.is_some() {
            self.service_failing(Service::Front, self.ctl.now_ms());
        }
        self.degrade(FRONT_UNAVAILABLE);
    }

    /// The front server the session asks: its pair's, else the configured one.
    fn front_server(&self) -> Option<FrontModelServer> {
        match &self.placement {
            Some(placement) => Some(placement.server().clone()),
            None => self.server.front_model.clone(),
        }
    }

    /// Put the session's prompt in its decisions' slot; the warm-up is also the probe.
    fn warm_front(&mut self) {
        if let Some((server, tx)) = self.front_model() {
            let persona = self.persona.clone();
            let warming = self.spawn_front(async move {
                let msg = if front_model::warm(&server, &persona).await {
                    Msg::FrontWarm
                } else {
                    Msg::Front(FrontEvent::Unavailable)
                };
                let _ = tx.send(msg).await;
            });
            self.warming = Some(warming.abort_handle());
        }
    }

    /// The model front answers again: the updates that waited for it are worded now.
    fn front_back(&mut self) {
        self.front_misses = 0;
        if self.front_lost_at.take().is_some() {
            let mut inputs = VecDeque::new();
            for narration in std::mem::take(&mut self.unnarrated) {
                self.narrate(narration, &mut inputs);
            }
            self.degraded.retain(|reason| *reason != FRONT_UNAVAILABLE);
            info!(stream_id = %self.stream_id, "interaction front model is back");
            self.watcher.recovered(Service::Front);
            self.updated(Vec::new(), Vec::new());
        }
    }

    /// The front stayed away past the restart window: interaction mode ends and the session
    /// goes on as a plain conversation, where it has one.
    fn front_gone(&mut self, now: Ms) {
        let window = Ms::try_from(front_supervisor::RESTART_WAIT.as_millis()).unwrap_or(Ms::MAX);
        let gone = self.front_lost_at.is_some_and(|at| now >= at + window);
        if gone && !self.plain && self.plain_path {
            self.step_down(
                "interaction_ended",
                "The front model did not come back within its restart window. Interaction \
                 mode ended; the session continues as a plain conversation with an empty \
                 history.",
                "front_unavailable",
            );
            self.undecided = None;
            self.watcher.recovered(Service::Front);
        } else if gone && !self.plain {
            // Nothing to go on as: the call ends with the exit line (chapter 10).
            self.watcher.gone(Service::Front);
        }
    }

    /// The policy wants the front's act for a turn: the provisional run starts now and the
    /// model decides. With the front unavailable nothing is decided (chapter 9: no rule
    /// router to fall back to).
    fn ask_front(
        &mut self,
        now: Ms,
        turn: TurnId,
        text: String,
        allowed: Vec<Act>,
        speculate: bool,
        inputs: &mut VecDeque<PolicyInput>,
    ) {
        let latest = self.turn.as_ref().filter(|latest| latest.id == turn);
        let (e_at, addressed) = latest.map_or((now, true), |latest| (latest.e, latest.addressed));
        let typed = latest.is_some_and(|latest| latest.typed);
        let named = latest.is_some_and(|latest| latest.language_named);
        let unclear = latest.is_some_and(|latest| latest.unclear);
        let barged = latest.is_some_and(|latest| latest.barged);
        // A newer turn: the earlier one's line would be dropped.
        if let Some(earlier) = self.front.take()
            && earlier.abort()
        {
            self.aside_done();
        }
        // A warm-up still under way would share the server's batches with the decision (and
        // a slot's with it): on the loaded GPU the interims' slot's warm-up, 3,250 tokens read
        // in 1.6 s beside it, took the first turn's decision past its budget (live MA3, MA1,
        // D1: "Sorry, I didn't catch that." to the first request). The decision itself fills
        // the decisions' slot's cache, the next agent line primes the interims'.
        if let Some(warming) = self.warming.take() {
            warming.abort();
        }
        // The turn ended: newer words of it over the agent are no longer asked, but a request
        // about them still out is kept until this decision has its act (`late_interim`): live
        // S1, the cue's interim dropped here, the cut waited ~0.7 s for the decision's act.
        // Not when the turn's words already cut the agent: there is nothing left for it to cut,
        // and what the agent began to say since is left alone (live S1, r8-verify: the cue's
        // interim, back 1.3 s after it was asked, cut the client's next line before its cue).
        match self.interim.take() {
            Some(mut interim) if interim.turn == turn && !barged => {
                interim.ended = true;
                interim.next = None;
                self.interim = Some(interim);
            }
            Some(interim) => interim.request.abort(),
            None => {}
        }
        self.primed = None;
        let Some((server, tx)) = self.front_model() else {
            debug!(stream_id = %self.stream_id, turn = turn.0, "interaction turn not decided: no front");
            // The model the gateway runs is down: the turn is asked once it is back, and the
            // caller hears a stall line meanwhile (chapter 10).
            if self.front_down.is_some() {
                self.undecided = Some((turn, text, allowed, speculate));
                self.service_failing(Service::Front, now);
            }
            return;
        };
        if speculate && allowed.contains(&Act::Delegate) {
            let start = TaskCmd::StartProvisional {
                turn,
                text: text.clone(),
                e_at: Some(e_at),
            };
            self.task(now, start, inputs);
        }
        let said = text.clone();
        let request =
            self.front_request(now, turn, text, allowed, addressed, unclear, typed, false);
        self.front_asked = now;
        let deciding = self.spawn_front(front_model::decide(
            server,
            self.persona.clone(),
            request,
            tx,
        ));
        // The language of the words is read once the decision's act is known, while the
        // conversation is in its pack's language (after a switch the caller's turns in it are
        // the conversation's own): `language_known`. A recogniser that named the language
        // read it already (live L1, r14-verify: the reading put the acknowledgement 0.6 s
        // later on every request).
        let reading = !named && controls::same_language(&self.state.language, persona::language());
        self.front = Some(FrontWait {
            turn,
            request: deciding.abort_handle(),
            reading,
            words: reading.then(|| said.clone()),
            said,
            other: None,
            asked: None,
            held: Vec::new(),
        });
    }

    /// One decision request: the words, the facts of the turn, the lines said since the model
    /// was last asked, the live tasks and the conversation state's news.
    #[allow(clippy::too_many_arguments)]
    fn front_request(
        &mut self,
        now: Ms,
        turn: TurnId,
        text: String,
        allowed: Vec<Act>,
        addressed: bool,
        unclear: bool,
        typed: bool,
        interim: bool,
    ) -> FrontRequest {
        let board = self.registry.board();
        let floor = self.floor.snapshot();
        let ctx = RuleCtx {
            now,
            addressed,
            unclear,
            answer_expected: self.policy.answer_expected(now),
            board: &board,
            // The self-echo check is for audio: typed words that repeat the agent's last
            // line are the client's own.
            recent_agent_lines: if typed {
                &[]
            } else {
                &floor.recent_agent_lines
            },
            over_agent: interim,
            // The policy's row 1 settles every turn the duplex machine took as the agent's
            // echo (unless it answers a read-back): a finished turn it asks about is the
            // caller's, whatever words of the agent's it repeats.
            turn_echo: (!interim).then_some(false),
        };
        let view = rules::rule_view(&text, &ctx, &self.persona);
        // The agent is quiet only between lines, and not after a line the caller talked over.
        let speaking = floor.agent_audible || floor.playing.is_some();
        let quiet_ms = self
            .floor
            .quiet_since()
            .filter(|_| !speaking)
            .map(|ended| now.saturating_sub(ended));
        FrontRequest {
            turn,
            text,
            allowed,
            view,
            // The prompt's contract: the lines said since the model was last asked, and the
            // tasks that are live. A finished task that stays listed as "result ready" reads
            // as news on every later turn: with four of them listed, two small models named
            // `social` or `wait` for four of five plain requests, and `delegate` for all five
            // (4 of 5 on the smaller one) once they were gone.
            said: floor
                .recent_agent_lines
                .into_iter()
                .filter(|(ended, _)| *ended >= self.front_asked)
                .map(|(_, line)| line)
                .collect(),
            // ... and the latest finished task an agent did: the front names a follow-up of
            // it in context (chapter 9: no word check of its title or of "it"). An agent's
            // task under a request still under way is not listed: the request stands for it
            // (live MA1/MA2: with the orchestrator's request and its child both listed, the
            // 2B front read a new request and a question as `wait`, and a question as a
            // `follow_up` of the child that had just ended).
            tasks: {
                let under_way = |task: &TaskView| {
                    task.lineage.parent_id.as_ref().is_some_and(|parent| {
                        let parent = board.tasks.iter().find(|other| other.id == *parent);
                        parent.is_some_and(|parent| parent.state.is_live())
                    })
                };
                let listed: Vec<bool> = board.tasks.iter().map(|task| !under_way(task)).collect();
                let latest_done = board
                    .tasks
                    .iter()
                    .zip(&listed)
                    .rposition(|(task, listed)| *listed && front_model::followable(task));
                let tasks = board.tasks.into_iter().enumerate();
                tasks
                    .filter(|(at, task)| {
                        listed[*at] && (task.state.is_live() || Some(*at) == latest_done)
                    })
                    .map(|(_, task)| task)
                    .collect()
            },
            now,
            total_budget_ms: self.cfg.front.total_budget_ms,
            header: self.persona.call_header_for(&self.cfg.front),
            interim,
            languages: self.cfg.controls.languages.clone(),
            agents: self.agent_topics.clone(),
            quiet_ms,
            // Words typed while the agent speaks did not talk over it.
            agent_speaking: speaking && !typed,
        }
    }

    /// The caller's words over the agent: the front is asked what they are, and its verdict
    /// takes the floor, lets the agent go on or pauses it. A request whose act is not known
    /// yet is not cancelled for newer words: they wait and are asked once it is (live S1: with
    /// interims 200 ms apart and the loaded front taking 250-700 ms to an act, each request
    /// was cancelled by the next and the cut came only when the words stopped changing). One
    /// whose act is known and did not cut (a wait, or a control read on to its kind) gives way
    /// to them at once (live S1: "stop talking" waited behind the kind of "zob"'s control and
    /// the cue was never cut).
    fn ask_interim(&mut self, now: Ms, turn: TurnId, text: String) {
        let waiting = |i: &&mut Interim| i.turn == turn && !i.known && !i.ended;
        if let Some(asking) = self.interim.as_mut().filter(waiting) {
            asking.next = Some(text);
            return;
        }
        self.ask_interim_now(now, turn, text);
    }

    fn ask_interim_now(&mut self, now: Ms, turn: TurnId, text: String) {
        // The observation's clock is the one primed at the caller's first sound, else the
        // turn's first interim: the turn's interims differ from the primed prompt only in the
        // caller's words, and the server reads the rest from its prompt cache (live S1: each
        // interim read the whole observation again, 70 to 450 ms on the loaded GB10).
        let mut since = self.primed.filter(|at| now < at + PRIMED_MS).unwrap_or(now);
        if let Some(earlier) = self.interim.take() {
            earlier.request.abort();
            if earlier.turn == turn {
                since = earlier.since;
            }
        }
        let Some((server, tx)) = self.front_model() else {
            return;
        };
        let allowed = interim_acts();
        let mut request =
            self.front_request(now, turn, text.clone(), allowed, true, false, false, true);
        request.now = since;
        let deciding = self.spawn_front(front_model::decide(
            server,
            self.persona.clone(),
            request,
            tx,
        ));
        self.interim = Some(Interim {
            turn,
            asked: now,
            since,
            request: deciding.abort_handle(),
            known: false,
            next: None,
            text,
            ended: false,
        });
    }

    /// The caller makes a sound over the agent: the decision about their words will be asked
    /// on the first interim, so its prompt (all but the words) goes into the front's cache now,
    /// while the recogniser has none yet. Once per stretch of overlap.
    fn prime_interim(&mut self, now: Ms) {
        if self.interim.is_some() || self.primed.is_some_and(|at| now < at + PRIMED_MS) {
            return;
        }
        self.prime(now);
    }

    /// An agent line starts: on a front server with a slot for the interims the prompt of a
    /// decision about words over it is primed now, while the agent speaks, so the caller's
    /// first sound finds it in the cache (live S1: primed at the first sound, the interim
    /// waited 300-800 ms behind the priming on the loaded GPU). With one slot the priming
    /// waits for the first sound, as it would delay the turn's decisions.
    ///
    /// A reading or a narration still out goes first: they share the interims' slot, and the
    /// priming put ahead of the answer from a stored result (its reading had taken the slot's
    /// cache, so the whole prompt was read again) made it miss the front's budget in every such
    /// case of the r4 runs (MA3: 4 of 4). The line is primed when they are back.
    fn prime_line(&mut self, now: Ms) {
        let second = self
            .front_server()
            .is_some_and(|server| front_model::slots(&server) > 1);
        if !second || self.interim.is_some() {
            return;
        }
        if self.asides > 0 {
            self.prime_owed = true;
            return;
        }
        // While a spoken question is open its answer's reading is primed instead (live MA2:
        // the reading was 290 to 357 ms of a 5 s deadline).
        if let Some((server, _)) = self.front_model()
            && let Some(reading) = self.policy.open_reading()
        {
            self.spawn_front(front_model::prime_reading(server, reading));
            return;
        }
        self.prime(now);
    }

    /// A reading or a narration is back: a line that started meanwhile is primed now.
    fn aside_done(&mut self) {
        self.asides = self.asides.saturating_sub(1);
        if self.asides == 0 && std::mem::take(&mut self.prime_owed) {
            self.prime_line(self.ctl.now_ms());
        }
    }

    /// A request to the front server, in the session's lane.
    fn spawn_front<F>(&self, request: F) -> tokio::task::JoinHandle<F::Output>
    where
        F: std::future::Future + Send + 'static,
        F::Output: Send + 'static,
    {
        tokio::spawn(front_model::LANE.scope(self.lane.0, request))
    }

    fn prime(&mut self, now: Ms) {
        let Some((server, _)) = self.front_model() else {
            return;
        };
        let allowed = interim_acts();
        let request = self.front_request(
            now,
            TurnId(0),
            String::new(),
            allowed,
            true,
            false,
            false,
            true,
        );
        self.primed = Some(now);
        self.spawn_front(front_model::prime(server, self.persona.clone(), request));
    }

    /// The front's verdict on an interim: a sure reading takes the floor (`interim_cut`); a
    /// continuer or side talk is known at the end of the answer.
    fn on_interim(
        &mut self,
        now: Ms,
        event: FrontEvent,
        inputs: &mut VecDeque<PolicyInput>,
        lanes: &mut Vec<LaneEvent>,
    ) {
        let Some(interim) = &self.interim else {
            return;
        };
        if interim.ended {
            return self.late_interim(now, event, inputs);
        }
        let (turn, asked, known) = (interim.turn, interim.asked, interim.known);
        // A line comes only after its act: a request replaced at its wait may have sent it.
        if matches!(event, FrontEvent::Done { .. }) && !known {
            return;
        }
        let class = match &event {
            FrontEvent::ActKnown { turn: of, act, .. } if *of == turn && *act != Act::Wait => {
                self.interim_cut(turn, *act)
            }
            // A control, read on to its kind: only a stop (also a misheard
            // "stop talking") is sure; the 2B front names a hold for "mm-hm" and "aha" too
            // (gate set, r3-fix).
            FrontEvent::Control { turn: of, control } if *of == turn => match control.kind {
                ControlKind::Stop => Some(Class::Interrupt),
                // A hold cuts as any other act does: once the turn's next interim reads it
                // too ("wait a second" over the agent; owner, 24 Oct: no cue list says so).
                ControlKind::Hold => {
                    let hold = (turn, Act::Control);
                    let agreed = self.over_named.replace(hold) == Some(hold);
                    agreed.then_some(Class::Interrupt)
                }
                _ => None,
            },
            FrontEvent::Done { turn: of, decision } if *of == turn => match decision.wait {
                Some(WaitReason::Continuer) => Some(Class::Backchannel),
                Some(WaitReason::SideTalk) => Some(Class::TalkingToOthers),
                _ => None,
            },
            _ => None,
        };
        let next = match &event {
            FrontEvent::Unavailable => {
                self.interim = None;
                return self.front_lost();
            }
            FrontEvent::ActKnown { .. } => self.interim.as_mut().and_then(|interim| {
                interim.known = true;
                // Words that interrupt need no second verdict; newer words are asked now (a
                // second reading of the same act takes the floor).
                let next = interim.next.take();
                next.filter(|_| class.is_none())
            }),
            FrontEvent::Done { .. } | FrontEvent::Failed { .. } => {
                let next = self
                    .interim
                    .as_mut()
                    .and_then(|interim| interim.next.take());
                if next.is_none() {
                    self.interim = None;
                }
                next
            }
            _ => None,
        };
        // A wait on older words: the newest are asked now.
        if let Some(text) = next {
            self.ask_interim_now(now, turn, text);
        }
        let Some(class) = class else {
            return;
        };
        let ms = now.saturating_sub(asked);
        debug!(stream_id = %self.stream_id, turn = turn.0, ?class, ms, "interaction front interim verdict");
        bridge::observe_turn_stage_ms("front_interim", "interaction", ms as f64);
        let floor = self.floor.snapshot();
        for action in self.duplex.front_verdict(now, turn, class, &floor) {
            lanes.extend(self.floor.on_duplex(now, &action));
            self.on_duplex(now, action, inputs);
        }
    }

    /// The verdict of an interim still out when its turn ended (`ask_front` keeps it): the
    /// front's stop (a `control` read on to its kind) cuts the agent before the turn's
    /// decision has its act. Anything else is the decision's to
    /// settle; its act drops the request (`on_front`), and its own stop does not cut again.
    fn late_interim(&mut self, now: Ms, event: FrontEvent, inputs: &mut VecDeque<PolicyInput>) {
        let Some(interim) = &self.interim else {
            return;
        };
        let turn = interim.turn;
        let stop = match &event {
            // Its kind is read on (no stop-cue list decides, owner, 24 Oct).
            FrontEvent::ActKnown {
                turn: of,
                act: Act::Control,
                ..
            } if *of == turn => return,
            FrontEvent::Control { turn: of, control } if *of == turn => {
                control.kind == ControlKind::Stop
            }
            _ => false,
        };
        if let Some(interim) = self.interim.take() {
            interim.request.abort();
        }
        let deciding = self.front.as_ref().is_some_and(|wait| wait.turn == turn);
        if !(stop && deciding && self.floor.snapshot().agent_audible) {
            return;
        }
        debug!(stream_id = %self.stream_id, turn = turn.0, "interaction late interim cut");
        self.stop_speaking(now, inputs);
        if let Some(latest) = self.turn.as_mut().filter(|latest| latest.id == turn) {
            latest.barged = true;
        }
    }

    /// Whether an interim's act over the agent takes the floor. Only a sure reading does (live
    /// L6: the interim "aha" of an "uh-huh" was named `control` and cut the agent): a control
    /// is read on to its kind and cuts only as the front's stop (no stop-cue list, owner, 24
    /// Oct), any other act once the turn's next interim names it too. Otherwise the turn's
    /// final decides, or the barge-in's own deadline.
    fn interim_cut(&mut self, turn: TurnId, act: Act) -> Option<Class> {
        if act == Act::Control {
            return None;
        }
        let agreed = self.over_named.replace((turn, act)) == Some((turn, act));
        agreed.then_some(Class::Interrupt)
    }

    /// An event of the model front about a caller turn. Only the turn it still decides is
    /// passed on.
    fn on_front(&mut self, now: Ms, event: FrontEvent, inputs: &mut VecDeque<PolicyInput>) {
        let turn = match &event {
            FrontEvent::ActKnown { turn, .. }
            | FrontEvent::Done { turn, .. }
            | FrontEvent::Failed { turn } => *turn,
            FrontEvent::Unavailable => return self.front_lost(),
            // The caller's word on a task's updates, before the turn's decision.
            FrontEvent::Notify { turn, .. } => {
                if let Some(wait) = self
                    .front
                    .as_mut()
                    .filter(|wait| wait.turn == *turn && wait.reading)
                {
                    wait.held.push(event);
                    return self.language_known(now, inputs);
                }
                if self.front.as_ref().is_some_and(|wait| wait.turn == *turn) {
                    inputs.push_back(PolicyInput::Front(event));
                }
                return;
            }
            // The caller steers the conversation: applied now, before the turn is settled. A
            // stop cuts at once; any other control waits for the language reading.
            FrontEvent::Control { turn, control } => {
                if let Some(wait) = self.front.as_mut().filter(|wait| {
                    wait.turn == *turn && wait.reading && control.kind != ControlKind::Stop
                }) {
                    wait.held.push(event);
                    return self.language_known(now, inputs);
                }
                if self.front.as_ref().is_some_and(|wait| wait.turn == *turn) {
                    let control = control.clone();
                    if !self.read_number(&control) {
                        self.control(now, control, inputs);
                    }
                }
                return;
            }
        };
        // Held until the front has read the words' language (`language_known`).
        if let Some(wait) = self
            .front
            .as_mut()
            .filter(|wait| wait.turn == turn && wait.reading)
        {
            wait.held.push(event);
            return self.language_known(now, inputs);
        }
        let Some(wait) = self.front.take_if(|wait| wait.turn == turn) else {
            return;
        };
        // The decision has its act: an interim of the turn kept past its end is not asked.
        if let Some(interim) = self.interim.take_if(|i| i.ended && i.turn == turn) {
            interim.request.abort();
        }
        let event = match event {
            // A control is applied, and its turn settled, once its kind is known. A stop the
            // caller said over the agent whose interims did not cut cuts it at its kind, which
            // the decoder sends before the rest of the line (live S1: the cue cut at the end of
            // its turn came 2.1 to 2.3 s after the voice, the decision's whole line read first).
            FrontEvent::ActKnown {
                act: Act::Control, ..
            } => {
                self.front_back();
                self.front = Some(wait);
                return;
            }
            FrontEvent::ActKnown { .. } => {
                self.front_back();
                self.front = Some(wait);
                event
            }
            FrontEvent::Failed { .. } => {
                self.front_missed();
                event
            }
            // "What do you mean?" and "tell me more" are the background's to answer: the turn
            // is delegated with its words.
            FrontEvent::Done { turn, decision } if self.delegate_turn.take() == Some(turn) => {
                let decision = ActDecision {
                    act: Act::Delegate,
                    source: ActSource::RuleOverride("control_for_the_background"),
                    ..decision
                };
                FrontEvent::Done { turn, decision }
            }
            done => done,
        };
        inputs.push_back(PolicyInput::Front(event));
    }

    /// The decision's events wait for the front's reading of the words' language. It is asked
    /// at the decision's act, so the act is the decision's alone (two requests batched together
    /// on the server read the same prompt differently: live PR10, "Also, fix the failing test"
    /// read `progress` beside the reading, `delegate` alone); a wait, a control or a failed
    /// decision needs none (a wish to talk in another language is the decision's `control`).
    /// Another language makes the turn the front's `wait` `foreign` (the policy asks again,
    /// then steps down); otherwise the events go on in order.
    fn language_known(&mut self, now: Ms, inputs: &mut VecDeque<PolicyInput>) {
        let Some(wait) = self.front.as_mut().filter(|wait| wait.reading) else {
            return;
        };
        let act = wait.held.iter().find_map(|event| match event {
            FrontEvent::ActKnown { act, .. } => Some(Some(*act)),
            FrontEvent::Done { decision, .. } => Some(Some(decision.act)),
            FrontEvent::Failed { .. } => Some(None),
            _ => None,
        });
        let turn = wait.turn;
        let foreign = match (act, wait.other) {
            // The act is still to come.
            (None, _) => return,
            (Some(Some(Act::Wait | Act::Control)) | Some(None), _) => false,
            (Some(Some(_)), Some(other)) => other,
            (Some(Some(_)), None) => {
                if let Some(words) = wait.words.take() {
                    self.read_language(turn, &words);
                    // Without the front no code comes: the turn goes on as decided.
                    return self.language_known(now, inputs);
                }
                return;
            }
        };
        if foreign {
            // Its reading is back.
            if let Some(wait) = self.front.take() {
                wait.abort();
            }
            debug!(stream_id = %self.stream_id, turn = turn.0, "interaction front reads another language");
            let decision = ActDecision {
                act: Act::Wait,
                task: None,
                wait: Some(WaitReason::Foreign),
                say: None,
                intent: None,
                source: ActSource::Model,
            };
            inputs.push_back(PolicyInput::Front(FrontEvent::Done { turn, decision }));
            return;
        }
        let Some(wait) = self.front.as_mut() else {
            return;
        };
        wait.reading = false;
        for event in std::mem::take(&mut wait.held) {
            self.on_front(now, event, inputs);
        }
    }

    /// Asks the front which language the turn's words are in (`Msg::Language`); without the
    /// front no code comes, and the turn is the decision's.
    fn read_language(&mut self, turn: TurnId, words: &str) {
        let Some((server, tx)) = self.front_model() else {
            if let Some(wait) = self.front.as_mut().filter(|wait| wait.turn == turn) {
                wait.other = Some(false);
            }
            return;
        };
        let budget = Duration::from_millis(self.cfg.front.total_budget_ms.into());
        let language = front_model::language_reading(turn, words);
        let reading = self.spawn_front(async move {
            let asked = tokio::time::Instant::now();
            let code = front_model::read(&server, &language, budget).await;
            let ms = u64::try_from(asked.elapsed().as_millis()).unwrap_or(u64::MAX);
            let _ = tx.send(Msg::Language { turn, code, ms }).await;
        });
        // Asked only for the turn the front decides (`language_known`).
        if let Some(wait) = self.front.as_mut().filter(|wait| wait.turn == turn) {
            wait.asked = Some(reading.abort_handle());
            self.asides += 1;
        }
    }

    /// A caller's repeat of a number or a code while the agent's last line holds a number is
    /// read apart (owner, 26 Oct: live CT1's "Sorry, can you say that again?" was the number):
    /// whether the caller asks for only the number or the whole line (`Msg::Number`). Whether
    /// it was asked; without the front the control is applied as the decision named it.
    fn read_number(&mut self, control: &Control) -> bool {
        let slot = control.value.as_deref();
        if control.kind != ControlKind::Repeat || !matches!(slot, Some("number" | "code")) {
            return false;
        }
        let Some(line) = self
            .floor
            .last_heard()
            .filter(|line| controls::slot_of(line, "number").is_some())
            .map(str::to_owned)
        else {
            return false;
        };
        let Some(said) = self.front.as_ref().map(|wait| wait.said.clone()) else {
            return false;
        };
        let Some((server, tx)) = self.front_model() else {
            return false;
        };
        let budget = Duration::from_millis(self.cfg.front.total_budget_ms.into());
        let turn = TurnId(control.turn.unwrap_or_default());
        let reading = front_model::number_reading(turn, &line, &said);
        let control = control.clone();
        self.asides += 1;
        self.spawn_front(async move {
            let asked = tokio::time::Instant::now();
            let label = front_model::read(&server, &reading, budget).await;
            let ms = u64::try_from(asked.elapsed().as_millis()).unwrap_or(u64::MAX);
            let _ = tx.send(Msg::Number { control, label, ms }).await;
        });
        true
    }

    fn front_missed(&mut self) {
        self.front_misses += 1;
        if self.front_misses >= FRONT_MISSES {
            self.front_lost();
        }
    }

    /// One conversation control, whoever asked (chapter 9): the operator's bounds and the
    /// session's capabilities decide what is applied ([`controls::apply`]), the actor does it,
    /// the history gets a note, the client `control_applied` and the new state, and what was
    /// not applied in full is said in the persona's words. Returns the `control_applied`
    /// event, the `conversation_control` tool's result.
    fn control(
        &mut self,
        now: Ms,
        control: Control,
        inputs: &mut VecDeque<PolicyInput>,
    ) -> serde_json::Value {
        // Another voice is chosen from the provider's list, read when first needed.
        if control.kind == ControlKind::Voice {
            self.fetch_voices();
        }
        // The voice list is still on its way (a vendor's list took 10 s live): a switch asked
        // for now would be "no voice" for a language the provider has (live CT2).
        let switch = matches!(control.kind, ControlKind::Language | ControlKind::Voice);
        if switch && self.voices_owed {
            debug!(stream_id = %self.stream_id, "interaction switch waits for the voice list");
            // A later switch replaces one still waiting (its orchestrator's tool is told so).
            self.language_owed = Some(control);
            self.owed_reply = None;
            return serde_json::Value::Null;
        }
        // A switch to a language the recogniser does not hear opens a second one in it first;
        // the switch waits for it (and is unsupported if it does not open).
        if let Some(code) = self.recogniser_for(&control) {
            self.open_recogniser(code);
            self.language_owed = Some(control);
            self.owed_reply = None;
            return serde_json::Value::Null;
        }
        let human = &self.cfg.controls.human;
        let by_sip = human.transfer_to.is_some() && self.sip.is_some() && self.ctl.phone_leg();
        self.caps.transfer = by_sip || human.url.is_some();
        let applied = controls::apply(&control, &mut self.state, &self.cfg.controls, &self.caps);
        info!(
            stream_id = %self.stream_id,
            kind = control.kind.as_str(),
            value = control.value.as_deref().unwrap_or_default(),
            source = ?control.source,
            outcome = ?applied.outcome,
            detail = %applied.detail,
            "interaction control"
        );
        let note = controls::history_note(&control, &applied, &self.state);
        self.ledger.push(ChatMessage::user(note));
        let mut say = None;
        let mut screen = None;
        // A repeat or a spelling is an answer; the rest are notices.
        let kind = if matches!(applied.effect, Effect::Repeat { .. } | Effect::Spell(_)) {
            Kind::Answer
        } else {
            Kind::Notice
        };
        let caller_turn = control
            .turn
            .filter(|_| control.source == ControlSource::Caller)
            .map(TurnId);
        // A caller's stop whose words already cut the agent over its line does not cut what
        // the agent began to say since (live S1: the next line, cut unheard).
        let barged = caller_turn.is_some_and(|turn| {
            self.turn
                .as_ref()
                .is_some_and(|latest| latest.id == turn && latest.barged)
        });
        match applied.effect.clone() {
            Effect::None => {}
            Effect::Cut if barged => {}
            Effect::Cut => self.stop_speaking(now, inputs),
            Effect::Hold(seconds) => {
                if !barged {
                    self.stop_speaking(now, inputs);
                }
                let wait = Ms::from(seconds.unwrap_or(HOLD_S)) * 1_000;
                self.hold_until = Some(now + wait);
            }
            Effect::Resume => {
                self.hold_until = None;
                let rest = std::mem::take(&mut self.unheard).join(" ");
                say = Some(rest)
                    .filter(|rest| !rest.trim().is_empty())
                    .map(Say::Verbatim);
                self.release_hold(now, inputs);
            }
            Effect::Repeat {
                slot,
                rate,
                gain_db,
            } => {
                let last = self.floor.last_heard().map(str::to_owned);
                let again = match (slot, last) {
                    (_, None) => template("control.nothing_to_repeat"),
                    (None, Some(_)) => Say::Repeat,
                    (Some(slot), Some(line)) => controls::slot_of(&line, &slot)
                        .map_or_else(|| template("control.nothing_to_repeat"), Say::Verbatim),
                };
                // Slower or louder: that line only (`submit`).
                if rate.is_some() || gain_db > 0.0 {
                    self.line_style = Some((again.clone(), rate, gain_db));
                }
                say = Some(again);
            }
            Effect::Spell(slot) => {
                let last = self.floor.last_heard().unwrap_or_default().to_owned();
                let slots = slot.map_or(vec!["code".to_owned(), "number".to_owned()], |s| vec![s]);
                let found = slots.iter().find_map(|slot| controls::slot_of(&last, slot));
                say = Some(found.map_or_else(
                    || template("control.nothing_to_repeat"),
                    |found| Say::Verbatim(controls::spelt(&found)),
                ));
            }
            // "What do you mean?" is the background's to answer from the call's history.
            Effect::Clarify => self.delegate_turn = caller_turn,
            Effect::Rate(rate) => {
                let _ = self.speech.send(SpeechCmd::Rate(rate));
            }
            Effect::Volume(db) => self.ctl.set_volume(db),
            Effect::Verbosity(verbosity) => {
                self.cfg.front.verbosity = Some(verbosity);
                // "Tell me more" also relays a full answer (chapter 8).
                if verbosity == Verbosity::Detailed {
                    self.delegate_turn = caller_turn;
                }
            }
            Effect::Language { voice, code } => {
                if let Some(voice) = voice {
                    let _ = self.speech.send(SpeechCmd::Voice(voice));
                }
                // The recogniser opened in the language hears the caller from their next
                // pause. Without a reviewed pack the gateway's own lines are kept to the
                // minimum (`answers_only`): no listening sounds at all.
                if self
                    .stt_next
                    .as_ref()
                    .is_some_and(|(next, _)| *next == code)
                {
                    self.stt_due = true;
                    self.swap_recogniser();
                }
                let bank = applied.outcome == controls::Outcome::Applied;
                self.revoice(now, bank, inputs);
            }
            Effect::Voice(voice) => {
                let _ = self.speech.send(SpeechCmd::Voice(voice));
                let language = &self.state.language;
                let mut packs = self.caps.packs.iter();
                let bank = packs.any(|pack| controls::same_language(pack, language));
                self.revoice(now, bank, inputs);
            }
            // The caller hears the persona's line, then is put through (`hand_off`): the SIP
            // leg refers the call, or the operator gets the request and the session ends.
            // A webhook hand-off that waits for the operator: "finding a person" now, the
            // line and the hand-off when the person answers (`Msg::Person`).
            Effect::Human => {
                let waits = !by_sip && self.cfg.controls.human.wait_s() > 0;
                if waits {
                    say = Some(template(HUMAN_FINDING));
                } else {
                    self.handoff = Some(by_sip);
                    say = Some(template(HUMAN_LINE));
                }
                self.request_human(now, by_sip);
            }
            Effect::Backchannels(on) => {
                self.cfg.duplex.backchannel = if on { Tri::On } else { Tri::Off };
                self.duplex.set_config(self.cfg.duplex.clone());
                if on && self.bank.is_none() && self.ctl.clips() == 0 {
                    let mut lanes = Vec::new();
                    self.build_bank(now, &mut lanes);
                    inputs.extend(lanes.into_iter().map(PolicyInput::Lane));
                } else if !on {
                    self.stop_clip(now);
                }
            }
            Effect::Updates(announce) => {
                self.cfg.tasks.announce = announce;
                self.policy.set_announce(announce);
            }
            Effect::ToScreen(slot) => {
                let last = self.floor.last_heard().unwrap_or_default().to_owned();
                let text = match &slot {
                    Some(slot) => controls::slot_of(&last, slot),
                    None => Some(last).filter(|last| !last.is_empty()),
                };
                say = Some(if text.is_some() {
                    template("control.on_screen")
                } else {
                    template("control.nothing_to_repeat")
                });
                screen = text;
            }
            Effect::End => {
                say = Some(Say::Bank {
                    group: "social",
                    intent: Some("goodbye"),
                })
            }
        }
        let event = WireEvent::ControlApplied {
            kind: control.kind,
            value: control.value.clone(),
            source: control.source,
            outcome: applied.outcome,
            detail: applied.detail.clone(),
            state_version: self.state.version,
            text: screen,
        };
        self.send(&event, MessageClass::Critical);
        // A wish of an earlier call that cannot be met now is not answered: nobody asked.
        let unasked = control.source == ControlSource::Memory
            && matches!(
                applied.outcome,
                controls::Outcome::Unsupported | controls::Outcome::Refused
            );
        if let Some(say) = applied.line.or(say).filter(|_| !unasked) {
            inputs.push_back(PolicyInput::ControlLine { kind, say });
        }
        // The caller's language wish is kept for their next call (caller memory).
        if control.kind == ControlKind::Language
            && matches!(
                control.source,
                ControlSource::Caller | ControlSource::Client
            )
        {
            self.remember_language(control.value.as_deref().unwrap_or_default());
        }
        self.updated(Vec::new(), Vec::new());
        serde_json::to_value(&event).unwrap_or_default()
    }

    /// The voice provider's list, asked for once and bounded: a language or voice switch
    /// asked for before it came waits for it.
    fn fetch_voices(&mut self) {
        if std::mem::replace(&mut self.voices_asked, true) {
            return;
        }
        self.voices_owed = true;
        let (tts, tx) = (self.vm.get_config().tts_config.clone(), self.tx.clone());
        tokio::spawn(async move {
            let listed =
                crate::handlers::voices::fetch_catalog_shared(&tts.provider, tts.api_key.trim());
            let listed = tokio::time::timeout(VOICE_LIST_WAIT, listed).await;
            // Without the list no other voice can be chosen: said once, so an "unsupported"
            // switch can be told from a provider that has no such voice (the reason is not
            // logged: a vendor's error may carry its request).
            let listed = listed.ok().flatten().and_then(Result::ok);
            let listed = listed.unwrap_or_else(|| {
                warn!(provider = %tts.provider, "interaction voice list not read: no switch to another voice");
                Vec::new()
            });
            if let Some(tx) = tx.upgrade() {
                let _ = tx.send(Msg::VoiceCatalog(listed)).await;
            }
        });
    }

    fn voices_arrived(
        &mut self,
        now: Ms,
        voices: Vec<(String, String)>,
        inputs: &mut VecDeque<PolicyInput>,
    ) {
        self.caps.voices.extend(voices);
        self.voices_owed = false;
        self.settle_owed(now, inputs);
    }

    /// The switch that waited for the voice list or a recogniser is applied now; an
    /// orchestrator that asked for it gets its result.
    fn settle_owed(&mut self, now: Ms, inputs: &mut VecDeque<PolicyInput>) {
        let Some(control) = self.language_owed.take() else {
            return;
        };
        let reply = self.owed_reply.take();
        let applied = self.control(now, control, inputs);
        match reply {
            // Now it waits for the recogniser.
            Some(reply) if applied.is_null() && self.language_owed.is_some() => {
                self.owed_reply = Some(reply);
            }
            Some(reply) => {
                let _ = reply.send(applied);
            }
            None => {}
        }
    }

    /// The language a switch needs a second recogniser in: the switch would be applied, the
    /// recogniser in use does not hear it, and none was opened in it yet.
    fn recogniser_for(&self, control: &Control) -> Option<String> {
        let code = control
            .value
            .as_deref()
            .filter(|_| control.kind == ControlKind::Language)?;
        let opened = self.stt_next.as_ref().map(|(next, _)| next.as_str());
        let heard = [
            Some(self.stt_language.as_str()),
            opened,
            self.stt_opening.as_deref(),
        ];
        if self.caps.hears_any
            || heard
                .iter()
                .flatten()
                .any(|lang| controls::same_language(lang, code))
        {
            return None;
        }
        let mut state = self.state.clone();
        let dry = controls::apply(control, &mut state, &self.cfg.controls, &self.caps);
        let would = matches!(
            dry.outcome,
            controls::Outcome::Applied | controls::Outcome::Partial
        );
        would.then(|| code.to_owned())
    }

    /// A second recogniser on the session's settings in `code`, off the actor (bounded).
    fn open_recogniser(&mut self, code: String) {
        let stt = &self.vm.get_config().stt_config;
        let native = crate::core::lang::map_language(&code, &stt.provider, &stt.model).native;
        info!(stream_id = %self.stream_id, language = %code, %native, "interaction recogniser opening in the new language");
        self.stt_opening = Some(code.clone());
        let (vm, tx) = (self.vm.clone(), self.tx.clone());
        tokio::spawn(async move {
            let opened = tokio::time::timeout(STT_OPEN_WAIT, vm.open_stt_in(&native)).await;
            let stt = match opened {
                Ok(Ok(stt)) => Ok(NextStt(Some(stt))),
                Ok(Err(error)) => Err(error.to_string()),
                Err(_) => Err(format!("not open within {} s", STT_OPEN_WAIT.as_secs())),
            };
            if let Some(tx) = tx.upgrade() {
                let _ = tx
                    .send(Msg::SttOpened {
                        language: code,
                        stt,
                    })
                    .await;
            }
        });
    }

    /// The recogniser opened for a switch hears the caller from now on, once they are silent:
    /// what they are saying is heard to its end by the one that heard its start.
    fn swap_recogniser(&mut self) {
        if !self.stt_due || self.voiced_since.is_some() {
            return;
        }
        let Some((language, next)) = self.stt_next.take() else {
            return;
        };
        let Some(next) = next.take() else {
            return;
        };
        self.stt_due = false;
        info!(stream_id = %self.stream_id, %language, "interaction recogniser switched");
        self.stt_language = language;
        let vm = self.vm.clone();
        tokio::spawn(async move { vm.swap_stt(next).await });
    }

    /// The voice changed (a voice or language switch): the listening sounds and the failure
    /// lines were made in the old one, so none plays until they are made again in the new
    /// voice (no listening sounds at all when `bank` is false).
    fn revoice(&mut self, now: Ms, bank: bool, inputs: &mut VecDeque<PolicyInput>) {
        self.ctl.set_clips(Vec::new());
        self.bank = None;
        // What the old voice is rendering now is not kept (its audio is dropped when it
        // comes); a line whose render still waits in the queue gets it in the new voice, so it
        // is not asked for twice. One whose render failed is rendered again.
        let mut rendering = Vec::new();
        for (_, line) in &mut self.clip_renders {
            rendering.push(std::mem::replace(line, STALE_RENDER));
        }
        let lines = self.failure_lines.iter_mut().enumerate();
        let mut queued = Vec::new();
        for (n, entry) in lines {
            if entry.rendering && !rendering.contains(&(FAILURE_LINE + n)) {
                queued.push(n);
            }
            entry.audio = None;
            entry.own_voice = false;
        }
        let mut lanes = Vec::new();
        if bank && self.cfg.duplex.backchannel != Tri::Off {
            self.build_bank(now, &mut lanes);
        }
        self.build_failure_lines(now, false, &queued, &mut lanes);
        inputs.extend(lanes.into_iter().map(PolicyInput::Lane));
    }

    /// A caller's or client's language wish, kept with the verified caller's memory.
    fn remember_language(&self, code: &str) {
        let (Some(sub), Some(key)) = (
            self.cfg.front.caller.clone(),
            self.server.caller_memory_key.clone(),
        ) else {
            return;
        };
        let (tenant, code) = (self.stored.owner.clone(), code.to_owned());
        tokio::spawn(async move {
            match tenant::remember_language(&key, &tenant, &sub, &code).await {
                Ok(()) => info!(%code, "the caller's language wish was kept"),
                Err(error) => warn!(%error, "the caller's language wish was not kept"),
            }
        });
    }

    /// The live tasks, as `session_failed` and `human_requested` list them.
    fn pending(&self) -> Vec<events::Pending> {
        events::Pending::live(self.registry.board())
    }

    /// A `human` control: the client gets `human_requested`; without the SIP leg the operator
    /// gets the same, signed, with the session's state and what was said.
    fn request_human(&mut self, now: Ms, by_sip: bool) {
        let event = WireEvent::HumanRequested {
            via: if by_sip { "sip" } else { "webhook" },
            pending: self.pending(),
            t_ms: now,
        };
        self.send(&event, MessageClass::Critical);
        let Some(url) = self.cfg.controls.human.url.clone().filter(|_| !by_sip) else {
            return;
        };
        let mut body = serde_json::to_value(&event).unwrap_or_default();
        let history: Vec<serde_json::Value> = self
            .ledger
            .iter()
            .map(|message| serde_json::json!({ "role": message.role, "content": message.content }))
            .collect();
        if let Some(body) = body.as_object_mut() {
            body.remove("t_ms");
            body.insert("stream_id".to_owned(), self.stream_id.clone().into());
            body.insert("state".to_owned(), self.state.to_json());
            body.insert("history".to_owned(), history.into());
        }
        let (secret, stream_id) = (self.server.tool_secret.clone(), self.stream_id.clone());
        let key = format!("{stream_id}:human_requested");
        let wait = self.cfg.controls.human.wait_s();
        if wait == 0 {
            tokio::spawn(async move {
                let body = body.to_string();
                super::store::push_body(&url, secret.as_deref(), &stream_id, body, &key).await;
            });
            return;
        }
        let (tx, wait) = (self.tx.clone(), Duration::from_secs(wait.into()));
        tokio::spawn(async move {
            let answer = super::remote::ask_for_person(&url, secret, body, &key, wait).await;
            if let Some(tx) = tx.upgrade() {
                let _ = tx.send(Msg::Person(answer)).await;
            }
        });
    }

    /// The operator answered a hand-off to a person: the persona's line and the hand-off when
    /// one takes the call, an honest line (and the call goes on) when nobody does.
    fn person_answered(
        &mut self,
        now: Ms,
        answer: super::remote::PersonAnswer,
        inputs: &mut VecDeque<PolicyInput>,
    ) {
        info!(stream_id = %self.stream_id, answer = answer.as_str(), "interaction person answer");
        let event = WireEvent::HumanAnswered {
            outcome: answer.as_str(),
            t_ms: now,
        };
        self.send(&event, MessageClass::Critical);
        let line = if answer == super::remote::PersonAnswer::Connected {
            self.handoff = Some(false);
            HUMAN_LINE
        } else {
            HUMAN_NO_ANSWER
        };
        let say = template(line);
        inputs.push_back(PolicyInput::ControlLine {
            kind: Kind::Notice,
            say,
        });
    }

    /// The persona's line before a hand-off was said (or cut): the SIP leg refers the call,
    /// else the session ends here (its tasks per `tasks.session_end`).
    fn hand_off(&mut self) {
        let Some(by_sip) = self.handoff.take() else {
            return;
        };
        info!(stream_id = %self.stream_id, by_sip, "interaction caller put through to a person");
        let to = self.cfg.controls.human.transfer_to.clone();
        match (by_sip, &self.sip, to) {
            (true, Some(SipTransfer(transfer)), Some(to)) => {
                tokio::spawn(transfer(to));
            }
            _ => {
                self.closed = true;
                self.close_reason = Some((1000, "transferred"));
            }
        }
    }

    /// A `stop` or a `hold`: the agent stops speaking now (chapter 5's cut); the tasks run on.
    fn stop_speaking(&mut self, now: Ms, inputs: &mut VecDeque<PolicyInput>) {
        let cut = DuplexAction::Cut(CutCause::Interrupt);
        let lanes = self.floor.on_duplex(now, &cut);
        inputs.extend(lanes.into_iter().map(PolicyInput::Lane));
        self.on_duplex(now, cut, inputs);
    }

    /// A hold is over (the time ran out or the caller spoke): the lines it kept back are
    /// offered again, in order.
    fn release_hold(&mut self, now: Ms, inputs: &mut VecDeque<PolicyInput>) {
        self.hold_until = None;
        if self.state.held {
            self.state.held = false;
            self.state.version += 1;
        }
        for line in std::mem::take(&mut self.held_lines) {
            let lanes = self.floor.offer(now, line);
            inputs.extend(lanes.into_iter().map(PolicyInput::Lane));
        }
    }

    /// The hold's time ran out: a check-in, then what it kept back.
    fn hold_over(&mut self, now: Ms, inputs: &mut VecDeque<PolicyInput>) {
        if self.hold_until.is_none_or(|until| now < until) {
            return;
        }
        inputs.push_back(PolicyInput::ControlLine {
            kind: Kind::Checkin,
            say: Say::Bank {
                group: "check_in",
                intent: None,
            },
        });
        self.release_hold(now, inputs);
        self.updated(Vec::new(), Vec::new());
    }

    /// A capability was lost for the rest of the session: counted and told to the client once.
    fn degrade(&mut self, reason: &'static str) {
        if self.degraded.contains(&reason) {
            return;
        }
        self.degraded.push(reason);
        bridge::record_degraded("interaction", reason);
        self.updated(Vec::new(), Vec::new());
    }

    fn updated(&mut self, applied: Vec<String>, refused: Vec<Refused>) {
        self.updated_with(applied, refused, None);
    }

    fn updated_with(
        &mut self,
        applied: Vec<String>,
        refused: Vec<Refused>,
        failure: Option<events::FailureNote>,
    ) {
        let event = WireEvent::InteractionUpdated {
            applied,
            refused,
            degraded: self
                .degraded
                .iter()
                .map(|code| (*code).to_owned())
                .collect(),
            state: Some(self.state.to_json()),
            failure,
        };
        self.send(&event, MessageClass::Critical);
    }

    /// An `interaction_update` from the client.
    fn update(&mut self, patch: &serde_json::Value) {
        let pace = self.cfg.front.pace;
        let (applied, warnings) = config::apply_update(&mut self.cfg, patch);
        // A new pace changes the speaking rate from the next line on, where the provider
        // takes a rate.
        if self.cfg.front.pace != pace {
            let factor = self.cfg.front.pace.map_or(1.0, |pace| pace.rate_factor());
            let _ = self.speech.send(SpeechCmd::Rate(factor));
            self.state.rate = factor;
            self.state.version += 1;
        }
        if let Some(verbosity) = self.cfg.front.verbosity {
            self.state.verbosity = verbosity;
        }
        // How the tasks' updates reach the caller from now on (`tasks.announce`).
        if self.cfg.tasks.announce != self.state.updates {
            self.state.updates = self.cfg.tasks.announce;
            self.state.version += 1;
        }
        self.policy.set_announce(self.cfg.tasks.announce);
        self.policy
            .set_config(self.cfg.policy.clone(), self.cfg.hitl_client);
        self.floor
            .set_hold_after_silence(self.cfg.policy.hold_after_silence_ms);
        let duplex = self.cfg.duplex.clone();
        self.ctl
            .set_duck(duplex.duck_db, duplex.duck_ramp_ms, duplex.barge_in);
        self.duplex.set_config(duplex);
        let refused = warnings
            .into_iter()
            .map(|warning| {
                let detail = |key: &str| warning.detail[key].as_str();
                let path = detail("path").unwrap_or_default();
                Refused {
                    path: path.trim_start_matches("interaction.").to_owned(),
                    reason: detail("reason").unwrap_or(warning.code).to_owned(),
                }
            })
            .collect();
        self.updated(applied, refused);
    }

    /// `FloorCmd::Submit`: the words go to the speech worker.
    fn submit(
        &mut self,
        now: Ms,
        utt: UttId,
        utterance: Utterance,
        text: String,
    ) -> Vec<LaneEvent> {
        if text.trim().is_empty() {
            let say = &utterance.say;
            warn!(stream_id = %self.stream_id, ?say, "interaction utterance cannot be spoken");
            return self.floor.on_pump(now, PumpEvent::Failed { utt });
        }
        if matches!(utterance.kind, Kind::Clip | Kind::Failure) {
            let line = (u64::MAX - utterance.item.0) as usize;
            self.clip_renders.push((utt, line));
            self.synth = Some(utt);
            let _ = self.speech.send(SpeechCmd::Clip { utt, text });
            return Vec::new();
        }
        let discard = utterance.discard;
        // A line said again slower or louder: that line only.
        let style = (!discard)
            .then(|| self.line_style.take_if(|(say, ..)| *say == utterance.say))
            .flatten();
        if !discard {
            // A warm-up line is never heard, so nothing of it is kept.
            self.recent.push_back((utt, text.clone()));
            if self.recent.len() > RECENT_KEPT {
                self.recent.pop_front();
            }
            self.live.push_back(Spoken {
                utt,
                text: text.clone(),
                utterance,
                started: false,
                delivered_ms: 0,
                submitted: now,
                alone: self.live.is_empty(),
            });
        }
        self.synth = Some(utt);
        let (rate, gain_db) = style.map_or((None, 0.0), |(_, rate, gain)| (rate, gain));
        if let Some(rate) = rate {
            let _ = self.speech.send(SpeechCmd::Rate(rate));
        }
        if gain_db > 0.0 {
            self.ctl.set_line_gain(utt, gain_db);
        }
        let _ = self.speech.send(SpeechCmd::Speak { utt, text, discard });
        if rate.is_some() {
            let _ = self.speech.send(SpeechCmd::Rate(self.state.rate));
        }
        Vec::new()
    }

    /// `FloorCmd::DropWarm`: the provider drops the warm-ups' synthesis (nothing of the agent's
    /// plays while one is in synthesis); a listening sound or failure line is rendered again
    /// when the floor gives it back.
    fn drop_warm(&mut self, utts: &[UttId]) -> Vec<LaneEvent> {
        debug!(stream_id = %self.stream_id, ?utts, "interaction warm-up gives way");
        self.clip_renders.retain(|(utt, _)| !utts.contains(utt));
        self.synth = None;
        let _ = self.speech.send(SpeechCmd::Cut);
        Vec::new()
    }

    /// `FloorCmd::Prerender`: an urgent line's words go into the speech cache, as a warm-up
    /// line's do, while the relayed answer it breaks into still plays.
    fn prerender(&mut self, utt: UttId, text: String) -> Vec<LaneEvent> {
        debug!(stream_id = %self.stream_id, utt = utt.0, "interaction urgent line rendered ahead");
        self.synth = Some(utt);
        let _ = self.speech.send(SpeechCmd::Speak {
            utt,
            text,
            discard: true,
        });
        Vec::new()
    }

    /// `FloorCmd::Cut`: all agent audio stops now. The floor is told how much of each
    /// utterance the caller heard. A pre-emption keeps the synthesis of the urgent line
    /// rendered ahead of it (nothing else is with the provider then).
    fn cut(&mut self, now: Ms, cause: CutCause) -> Vec<LaneEvent> {
        // The pump's report first: the worker's own cut after it returns nothing.
        let report = if cause == CutCause::PreEmpted && self.floor.prerendering() {
            let report = self.ctl.cut_keeping_warm();
            let _ = self.speech.send(SpeechCmd::CutAudio);
            report
        } else {
            let report = self.ctl.cut();
            let _ = self.speech.send(SpeechCmd::Cut);
            self.synth = None;
            report
        };
        let delivered = |utt: UttId| {
            let found = report.iter().find(|(reported, _)| *reported == utt);
            found.map_or(0, |(_, ms)| *ms)
        };
        let heard: Vec<(UttId, usize)> = self
            .live
            .iter()
            .map(|spoken| {
                // What the client reported playing; else what was delivered, less the pump's
                // lead, which had not played yet.
                let estimate = delivered(spoken.utt).saturating_sub(self.ctl.lead_ms());
                let played = self.played.get(&spoken.utt).map_or(estimate, |(ms, _)| *ms);
                let prefix = heard_prefix(&spoken.text, played, self.rate.chars_per_ms());
                (spoken.utt, prefix.chars().count())
            })
            .collect();
        if !self.live.is_empty() {
            let utts = self.live.iter().map(|spoken| spoken.utt);
            let event = WireEvent::AgentAudioStopped {
                utterance_ids: utts.clone().map(utterance_id).collect(),
                reason: stop_reason(cause),
                delivered_ms: utts.map(delivered).collect(),
                t_ms: now,
            };
            self.send(&event, MessageClass::Critical);
        }
        // What the pump had played out and the client not yet: heard as far as it reported.
        let mut lanes = self.release_held(now, true);
        lanes.extend(self.floor.on_cut(now, cause, &heard));
        // A listening sound in synthesis went with the cut: it is made again.
        for (_, line) in std::mem::take(&mut self.clip_renders) {
            if line != STALE_RENDER {
                self.render_clip(now, line, &mut lanes);
            }
        }
        lanes
    }

    /// How often listening sounds play: the persona's intensity (unset: medium, as on a web
    /// line), none on a phone line unless `duplex.backchannel` is `true`, and at most
    /// `duplex.backchannel.intensity`.
    fn backchannel_level(&self) -> Intensity {
        let line = match self.cfg.duplex.backchannel {
            Tri::Off => return Intensity::Off,
            Tri::Auto if self.ctl.phone_leg() => return Intensity::Off,
            Tri::Auto | Tri::On => Intensity::Medium,
        };
        let level = self.persona.backchannel_intensity().unwrap_or(line);
        self.cfg
            .duplex
            .backchannel_intensity
            .map_or(level, |cap| cap.min(level))
    }

    /// A pause in a long caller turn: a listening sound, when the session has them, after
    /// enough of the caller's speech for the intensity, nothing said lately (a listening sound
    /// or the agent's own line: a caller who has just stopped the agent is not urged on) and
    /// not too many in this turn, the turn names no private value, no sensitive value is being
    /// captured, and not at the pauses the intensity skips.
    fn backchannel(&mut self, now: Ms, text: &str, voiced_ms: u32) {
        let medium = self.persona.backchannel_timing();
        let Some(timing) = self.backchannel_level().timing(medium) else {
            return;
        };
        let lately = [self.last_backchannel, self.floor.last_line_at()]
            .into_iter()
            .flatten()
            .any(|at| now < at + timing.gap_ms);
        let enough = voiced_ms >= timing.min_voiced_ms
            && timing.per_turn.is_none_or(|most| self.turn_clips < most);
        let private =
            rules::sensitive_category(text, self.persona.sensitive_categories()).is_some();
        let failure = !self.failure_playing.is_empty();
        if !enough
            || lately
            || private
            || self.plain
            || self.capture
            || self.clip.is_some()
            || failure
        {
            return;
        }
        // Not at every pause: a listener who never misses one sounds mechanical.
        self.pauses += 1;
        if self.pauses.is_multiple_of(timing.skip_every) {
            debug!(stream_id = %self.stream_id, "interaction listening sound skipped");
            return;
        }
        let utt = UttId(CLIP_BASE + self.clip_next);
        if self.ctl.play_clip(self.clip_next as usize, utt) {
            debug!(stream_id = %self.stream_id, utt = utt.0, "interaction listening sound");
            self.clip_next += 1;
            self.clip = Some(utt);
            self.last_backchannel = Some(now);
            self.turn_clips += 1;
        } else {
            debug!(stream_id = %self.stream_id, "interaction listening sound wanted, none made yet");
        }
    }

    /// The listening sounds of the persona, each tried as the operator's recording and then
    /// rendered in the session's voice; none plays before all are checked.
    fn build_bank(&mut self, now: Ms, lanes: &mut Vec<LaneEvent>) {
        if self.bank.is_some() {
            return;
        }
        let bank = Bank::new(
            self.persona.backchannel_clips(),
            self.persona.backchannel_fallback(),
        );
        let lines = bank.len();
        self.bank = Some(bank);
        for line in 0..lines {
            self.try_clip(now, line, lanes);
        }
    }

    /// Line `line` of the bank: the operator's recording when `interaction.clips_dir` has one
    /// for the session's provider, voice and language, else rendered.
    fn try_clip(&mut self, now: Ms, line: usize, lanes: &mut Vec<LaneEvent>) {
        let Some(bank) = &self.bank else {
            return;
        };
        let id = bank.line(line).0.to_owned();
        let voice = self.voice();
        let parts = [voice.provider.as_str(), &voice.voice, &voice.language, &id];
        // A recording is PCM16: a session whose audio is another format renders its clips.
        let recording = (self.server.clips_dir.as_deref()).filter(|_| pcm_voice(&self.vm));
        let rate = self.vm.get_config().tts_config.sample_rate;
        match recording.and_then(|dir| clips::recorded(dir.as_ref(), &parts, rate)) {
            Some(clip) => self.check_clip(line, true, clip),
            None => self.render_clip(now, line, lanes),
        }
    }

    /// Synthesised as a warm-up line is, its audio kept by the pump for the checks.
    fn render_clip(&mut self, now: Ms, line: usize, lanes: &mut Vec<LaneEvent>) {
        // A listening sound may be made while the caller speaks (the floor); a failure line
        // waits for a quiet moment.
        let (kind, text) = match line.checked_sub(FAILURE_LINE) {
            Some(n) => (
                Kind::Failure,
                self.failure_lines.get_mut(n).map(|entry| {
                    entry.rendering = true;
                    entry.line.text.clone()
                }),
            ),
            None => (
                Kind::Clip,
                self.bank.as_ref().map(|bank| bank.line(line).1.to_owned()),
            ),
        };
        let Some(text) = text else {
            return;
        };
        // Behind the warm-up lines (rank 9): the first turn's acknowledgement is one of those,
        // and a listening sound or a failure line is needed later (live L1/L3/B1: with the 9
        // failure lines first, the first acknowledgements were synthesised while the caller
        // waited, 0.5-0.7 s, and the warm-up lines came 27 s into the call).
        let clip = Utterance {
            item: ItemId(u64::MAX - line as u64),
            lane: Lane::Front,
            kind,
            rank: 10,
            say: Say::Verbatim(text),
            discard: true,
            task: None,
            reply_to: None,
            question: None,
            expires_at: None,
            sentence: None,
            last: true,
        };
        lanes.extend(self.floor.offer(now, clip));
    }

    /// The checks run off the actor: the recogniser may take an upload.
    fn check_clip(&self, line: usize, recorded: bool, clip: AudioData) {
        let Some(bank) = &self.bank else {
            return;
        };
        let text = bank.line(line).1.to_owned();
        let (vm, tx) = (self.vm.clone(), self.tx.clone());
        tokio::spawn(async move {
            let cheap = vm.stt_is_request_response().await;
            let stt = cheap.then(|| vm.get_config().stt_config.clone());
            let clip = clips::check(clip, &text, stt).await;
            if let Some(tx) = tx.upgrade() {
                let _ = tx
                    .send(Msg::ClipChecked {
                        line,
                        recorded,
                        clip,
                    })
                    .await;
            }
        });
    }

    /// A pump event of a clip in synthesis: its audio goes to the checks, or it failed.
    fn clip_rendered(&mut self, now: Ms, utt: UttId, lanes: &mut Vec<LaneEvent>) {
        let Some(at) = self
            .clip_renders
            .iter()
            .position(|(render, _)| *render == utt)
        else {
            return;
        };
        let (_, line) = self.clip_renders.swap_remove(at);
        if line == STALE_RENDER {
            let _ = self.ctl.take_captured(utt);
            return;
        }
        if let Some(n) = line.checked_sub(FAILURE_LINE) {
            return self.failure_rendered(n, self.ctl.take_captured(utt));
        }
        match self.ctl.take_captured(utt) {
            Some(clip) => self.check_clip(line, false, clip),
            None => {
                let text = self.bank.as_ref().map(|bank| bank.line(line).1.to_owned());
                info!(stream_id = %self.stream_id, ?text, "interaction listening sound not rendered");
                self.clip_failed(now, line, lanes);
            }
        }
    }

    /// Line `line` has no usable clip: the pack's fallback word is tried in its place.
    fn clip_failed(&mut self, now: Ms, line: usize, lanes: &mut Vec<LaneEvent>) {
        if let Some(fallback) = self.bank.as_mut().and_then(|bank| bank.failed(line)) {
            self.try_clip(now, fallback, lanes);
        }
        self.install_bank();
    }

    /// Every line is settled: the clips that passed are the bank. With none the session makes
    /// no listening sounds, and says so once.
    fn install_bank(&mut self) {
        let Some(clips) = self.bank.as_mut().and_then(Bank::ready) else {
            return;
        };
        info!(stream_id = %self.stream_id, clips = clips.len(), "interaction listening sounds ready");
        let none = clips.is_empty();
        self.ctl.set_clips(clips);
        if none {
            self.degrade(NO_LISTENING_SOUNDS);
        }
    }

    /// The caller speaks again, or their turn ended: the listening sound yields. One not
    /// started yet is dropped; one that plays fades out over 40 ms.
    fn stop_clip(&mut self, now: Ms) {
        if let Some(utt) = self.clip.take() {
            self.ctl.fade_out(utt, CLIP_FADE_MS);
            self.segment(now, "end", utt, "backchannel", Kind::Clip, None);
        }
    }

    // ---- when services fail (chapter 10)

    /// The session's voice as its clips and failure lines are kept: another one after a voice
    /// or language switch.
    fn voice(&self) -> clips::Voice {
        let mut voice = session_voice(&self.vm, &self.persona);
        if let Some(switched) = &self.state.voice {
            voice.voice.clone_from(switched);
        }
        voice
    }

    /// `failure.stall_after_ms`.
    fn stall_after(&self) -> Ms {
        Ms::from(self.cfg.failure.stall_after_ms)
    }

    /// The failure lines that need synthesis are rendered in the session's voice, as the
    /// listening sounds are; the server's default set stands in until they are (and for any
    /// that fails its check).
    /// `stand_ins`: the server's default set plays until they are; after a switch of voice
    /// nothing does (as for the listening sounds).
    /// `queued`: lines whose render already waits in the queue.
    fn build_failure_lines(
        &mut self,
        now: Ms,
        stand_ins: bool,
        queued: &[usize],
        lanes: &mut Vec<LaneEvent>,
    ) {
        let pcm = pcm_voice(&self.vm);
        let rate = self.vm.get_config().tts_config.sample_rate;
        let default = clips::default_set().filter(|_| pcm && stand_ins);
        let voice = self.voice();
        let dir = self.server.clips_dir.clone();
        let dir = dir.as_deref().map(std::path::Path::new);
        for n in 0..self.failure_lines.len() {
            let entry = &mut self.failure_lines[n];
            // Recorded or rendered since the session was set up (another session's).
            let ready = (!entry.own_voice && pcm)
                .then(|| clips::failure_ready(dir, &voice, &entry.line, rate))
                .flatten();
            if let Some((audio, _)) = ready {
                entry.audio = Some(audio);
                entry.own_voice = true;
            }
            if entry.own_voice {
                continue;
            }
            if entry.audio.is_none()
                && let Some(audio) = default.as_ref().and_then(|set| set.stand_in(&entry.line))
            {
                entry.audio =
                    Some(rate.map_or_else(|| audio.clone(), |rate| clips::resampled(audio, rate)));
            }
            if !queued.contains(&n) {
                self.render_clip(now, FAILURE_LINE + n, lanes);
            }
        }
    }

    /// A failure line rendered in the session's voice: checked, kept for later sessions,
    /// and used from now on. One that fails keeps the default set's.
    fn failure_rendered(&mut self, n: usize, audio: Option<AudioData>) {
        let voice = self.voice();
        let dir = self.server.clips_dir.as_deref().map(std::path::Path::new);
        let Some(entry) = self.failure_lines.get_mut(n) else {
            return;
        };
        entry.rendering = false;
        let checked = audio.ok_or_else(|| "not rendered".to_owned());
        match checked.and_then(|audio| clips::check_line(&audio)) {
            Ok(audio) => {
                clips::keep_rendered(dir, &voice, &entry.line, &audio);
                entry.audio = Some(audio);
                entry.own_voice = true;
            }
            Err(reason) => info!(
                stream_id = %self.stream_id,
                id = %entry.line.id,
                %reason,
                default = entry.audio.is_some(),
                "interaction failure line refused"
            ),
        }
        if self.failure_lines.iter().all(|entry| entry.own_voice) {
            info!(stream_id = %self.stream_id, lines = self.failure_lines.len(), "interaction failure lines ready");
        }
    }

    /// `service` fails: a stall line may play for it from `due` on.
    fn service_failing(&mut self, service: Service, due: Ms) {
        if self.watcher.failing(service, due) {
            info!(stream_id = %self.stream_id, service = service.as_str(), "interaction service failing");
        }
    }

    /// `service` works again: what it degraded is no longer listed.
    fn service_recovered(&mut self, service: Service) {
        if !self.watcher.recovered(service) {
            return;
        }
        info!(stream_id = %self.stream_id, service = service.as_str(), "interaction service back");
        let code = service.degraded();
        if service != Service::Front && self.degraded.contains(&code) {
            self.degraded.retain(|listed| *listed != code);
            self.updated(Vec::new(), Vec::new());
        }
    }

    /// The health signals that are deadlines, then what the watcher plays (chapter 10). A
    /// turn the front model was down for is asked once it is back.
    fn health(&mut self, now: Ms, inputs: &mut VecDeque<PolicyInput>) {
        // The model the gateway runs: looked at until it is back.
        if self.front_down.is_some() {
            let _ = self.front_model();
        }
        if self.front_down.is_none()
            && let Some((turn, text, allowed, speculate)) = self.undecided.take()
            && self.turn.as_ref().is_some_and(|latest| latest.id == turn)
        {
            info!(stream_id = %self.stream_id, turn = turn.0, "interaction turn asked again: the front is back");
            self.ask_front(now, turn, text, allowed, speculate, inputs);
        }
        // The background's first token, while the caller waits for a task.
        let late = self.first_token_due.values().any(|due| now >= *due);
        let waits = || {
            self.registry
                .board()
                .tasks
                .iter()
                .any(|task| task.state.is_live())
        };
        if late && waits() {
            self.service_failing(Service::Background, now);
        } else if !late && self.watcher.is_failing(Service::Background) {
            self.service_recovered(Service::Background);
        }
        // A synthesis that gives no audio.
        let stall = self.stall_after();
        if let Some((utt, since)) = self.floor.synth_waiting_since()
            && now >= since + stall
            && !self.ctl.has_audio(utt)
        {
            self.service_failing(Service::Voice, now);
        }
        // A streaming recogniser that gives nothing for the caller's speech.
        let speaking = self.voiced_since.map_or(0, |at| now.saturating_sub(at));
        let unheard = Ms::from(self.unheard_ms) + speaking;
        if !self.request_response && !self.capture && unheard >= Ms::from(health::UNHEARD_SPEECH_MS)
        {
            self.service_failing(Service::Recogniser, now);
        }
        if let Some((_, until)) = self.exiting
            && now >= until
        {
            return self.finish_exit(now);
        }
        if self.exiting.is_some() {
            return;
        }
        let paused = |at: Ms| now >= at + FAILURE_PAUSE_MS;
        let caller_silent = self.voiced_since.is_none() && self.silent_since.is_none_or(paused);
        let agent = self.floor.snapshot().agent_audible;
        // The agent spoke again before the back line could: the caller hears the call go on.
        if agent && self.watcher.back_owed() {
            self.watcher.drop_back();
        }
        let failure = !self.failure_playing.is_empty();
        // A line that has no audio yet (rendered at session start, no default set) waits.
        let has = |kinds: &[LineKind]| {
            let lines = self.failure_lines.iter();
            lines
                .into_iter()
                .any(|entry| kinds.contains(&entry.line.kind) && entry.audio.is_some())
        };
        let stalls = has(&[LineKind::Stall, LineKind::Hearing]);
        let slot = health::Slot {
            stall: caller_silent && !agent && !failure && self.clip.is_none() && stalls,
            // A recovery is news: it may follow the caller's last word at once.
            back: self.voiced_since.is_none()
                && !agent
                && self.clip.is_none()
                && has(&[LineKind::Back]),
            caller_silent: caller_silent && !failure,
        };
        if let Some(line) = self.watcher.next(now, slot) {
            self.play_failure(now, line);
        }
    }

    /// The next time a health deadline passes (only those still ahead: one that passed has
    /// been acted on, or waits for a slot that a message opens).
    fn health_wakeup(&self) -> Option<Ms> {
        let now = self.ctl.now_ms();
        let stall = self.stall_after();
        let recogniser = (!self.request_response).then(|| {
            let left =
                Ms::from(health::UNHEARD_SPEECH_MS).saturating_sub(Ms::from(self.unheard_ms));
            self.voiced_since.map(|at| at + left)
        });
        [
            self.watcher.next_wakeup(),
            self.first_token_due.values().min().copied(),
            self.floor
                .synth_waiting_since()
                .map(|(_, since)| since + stall),
            recogniser.flatten(),
            self.exiting.map(|(_, until)| until),
            self.front_down.map(|_| now + FRONT_POLL_MS),
            self.silent_since.map(|at| at + FAILURE_PAUSE_MS),
        ]
        .into_iter()
        .flatten()
        .filter(|at| *at > now)
        .min()
    }

    /// One failure line through the pump: the `n`th of its kind that has audio (a hearing
    /// line falls back to a stall line, the promise of a follow-up to the plain exit), with
    /// a note in the history and `interaction_updated` naming it.
    fn play_failure(&mut self, now: Ms, line: health::Line) {
        let of = |kind: LineKind| {
            let lines = self.failure_lines.iter();
            lines
                .filter(|entry| entry.line.kind == kind && entry.audio.is_some())
                .collect::<Vec<_>>()
        };
        let mut lines = of(line.kind);
        if lines.is_empty() {
            lines = match line.kind {
                LineKind::Hearing => of(LineKind::Stall),
                LineKind::ExitFollowUp => of(LineKind::Exit),
                _ => lines,
            };
        }
        let chosen = (!lines.is_empty()).then(|| lines[line.n % lines.len()]);
        let chosen = chosen.and_then(|entry| Some((entry.line.clone(), entry.audio.clone()?)));
        let exit = matches!(line.kind, LineKind::Exit | LineKind::ExitFollowUp);
        let service = line.service;
        if !exit && line.kind != LineKind::Back && !self.degraded.contains(&service.degraded()) {
            self.degraded.push(service.degraded());
            bridge::record_degraded("interaction", service.degraded());
        }
        let Some((said, audio)) = chosen else {
            warn!(stream_id = %self.stream_id, kind = line.kind.as_str(), "interaction failure line missing");
            if exit {
                self.exiting = Some((service, now));
            }
            return;
        };
        let utt = UttId(FAILURE_UTT + self.failure_next);
        self.failure_next += 1;
        let played = self.ctl.play(audio.clone(), utt);
        if played {
            self.failure_playing.push((utt, said.text.clone()));
        }
        info!(
            stream_id = %self.stream_id,
            id = %said.id,
            kind = line.kind.as_str(),
            service = service.as_str(),
            played,
            "interaction failure line"
        );
        let what = match line.kind {
            LineKind::Back => "is back",
            LineKind::Exit | LineKind::ExitFollowUp => "stayed down",
            LineKind::Stall | LineKind::Hearing => "failed",
        };
        let note = format!(
            "[the {} {what}; the agent said: {}]",
            service.as_str(),
            said.text
        );
        self.ledger.push(ChatMessage::user(note));
        if exit {
            let ms = clips::duration_ms(&audio);
            self.exiting = Some((service, now + ms + EXIT_SLACK_MS));
        }
        let failure = events::FailureNote {
            line: said.id,
            kind: line.kind.as_str(),
            service: service.as_str(),
            text: said.text,
        };
        self.updated_with(Vec::new(), Vec::new(), Some(failure));
    }

    /// Where failure line `utt` is in [`Actor::failure_playing`], while it plays.
    fn failure_at(&self, utt: UttId) -> Option<usize> {
        self.failure_playing
            .iter()
            .position(|(playing, _)| *playing == utt)
    }

    /// The caller speaks over a stall or back line: it fades out (never over the caller).
    /// The exit line is not stopped.
    fn stop_failure_line(&mut self, now: Ms) {
        if self.exiting.is_some() {
            return;
        }
        for (utt, _) in std::mem::take(&mut self.failure_playing) {
            self.ctl.fade_out(utt, CLIP_FADE_MS);
            self.segment(now, "end", utt, "front", Kind::Failure, None);
        }
    }

    /// The exit line was said: the client gets `session_failed` with what was pending, the
    /// push address the same, and the session ends (its tasks per `tasks.session_end`).
    fn finish_exit(&mut self, now: Ms) {
        let Some((service, _)) = self.exiting.take() else {
            return;
        };
        let pending = self.pending();
        let reason = match service {
            Service::Front => "the front model was not back within its restart window".to_owned(),
            service => format!(
                "the {} did not recover within {} s of the first stall line",
                service.as_str(),
                self.cfg.failure.max_wait_s
            ),
        };
        warn!(stream_id = %self.stream_id, service = service.as_str(), %reason, pending = pending.len(), "interaction session failed");
        bridge::record_degraded("interaction", "session_failed");
        let event = WireEvent::SessionFailed {
            service: service.as_str(),
            reason,
            pending,
            follow_up: follow_up(&self.cfg),
            t_ms: now,
        };
        self.send(&event, MessageClass::Critical);
        if let Some(url) = self.cfg.tasks.push_url.clone() {
            let mut body = serde_json::to_value(&event).unwrap_or_default();
            if let Some(body) = body.as_object_mut() {
                body.remove("t_ms");
                body.insert("stream_id".to_owned(), self.stream_id.clone().into());
            }
            let (secret, stream_id) = (self.push_secret(), self.stream_id.clone());
            let key = format!("{stream_id}:session_failed");
            tokio::spawn(async move {
                let body = body.to_string();
                super::store::push_body(&url, secret.as_deref(), &stream_id, body, &key).await;
            });
        }
        self.closed = true;
    }

    /// Opens or closes the capture window of a sensitive value asked on the screen or the
    /// keypad (plan chapter 4): while it is open the speech-to-text hears silence of the same
    /// length and its results go to nobody, so the value never reaches a transcript. The
    /// voice activity model still hears the caller.
    fn sync_capture(&mut self) {
        let capture = self.policy.capturing();
        if capture != self.capture {
            self.capture = capture;
            self.vm.mute_stt(capture);
            info!(stream_id = %self.stream_id, capture, "interaction sensitive capture window");
        }
        // The call's recording holds nothing of a value typed on the keypad (its tones).
        let paused = capture && self.policy.keyed();
        if paused != self.recording_paused
            && let Some(RecordingPause(pause)) = &self.recording
        {
            self.recording_paused = paused;
            info!(stream_id = %self.stream_id, paused, "interaction call recording paused for the keypad");
            tokio::spawn(pause(paused));
        }
    }

    /// `audio_segment`, with `events.segments`.
    fn segment(
        &mut self,
        now: Ms,
        phase: &'static str,
        utt: UttId,
        lane: &'static str,
        kind: Kind,
        task_id: Option<String>,
    ) {
        if self.cfg.events_segments {
            let event = WireEvent::AudioSegment {
                phase,
                utterance_id: utterance_id(utt),
                lane,
                kind: wire_kind(kind),
                task_id,
                t_ms: now,
            };
            self.send(&event, MessageClass::Transcript);
        }
    }

    /// The client's `playback_position`. From the first one on, what the client reports is
    /// what was heard: "say that again" says the line it reports playing, and an utterance
    /// the pump played out is heard only once the client reports playing it to its end.
    fn report_played(&mut self, now: Ms, id: &str, played_ms: u64) -> Vec<LaneEvent> {
        let Some(utt) = id
            .strip_prefix("u_")
            .and_then(|n| n.parse().ok())
            .map(UttId)
            .filter(|utt| utt.0 < CLIP_BASE)
        else {
            return Vec::new();
        };
        let most = self.played.entry(utt).or_insert((0, now));
        if played_ms >= most.0 {
            *most = (played_ms, now);
        }
        self.played
            .retain(|other, _| other.0 + RECENT_KEPT as u64 >= utt.0);
        self.position = Some((utt, played_ms, now));
        let line = self.recent.iter().find(|(recent, _)| *recent == utt);
        if let Some((_, text)) = line.filter(|_| played_ms > 0) {
            self.floor.set_repeat(text.clone());
        }
        self.release_held(now, false)
    }

    /// Whether `event` waits for the client: the floor's `Heard` of an utterance, once the
    /// client reports what it plays. The end of an utterance's audio is sent here.
    fn hold(&mut self, now: Ms, event: &LaneEvent, inputs: &mut VecDeque<PolicyInput>) -> bool {
        let (LaneEvent::Heard { item, .. } | LaneEvent::Dropped { item, .. }) = *event else {
            return false;
        };
        if self.released.remove(&item) {
            return false;
        }
        let Some(spoken) = self
            .live
            .iter()
            .find(|spoken| spoken.utterance.item == item)
        else {
            return false;
        };
        if spoken.started {
            let (utt, lane, kind) = (spoken.utt, spoken.utterance.lane, spoken.utterance.kind);
            let task = spoken
                .utterance
                .task
                .as_ref()
                .map(|(task, _)| task.0.clone());
            let lane = if lane == Lane::Main { "main" } else { "front" };
            self.segment(now, "end", utt, lane, kind, task);
        }
        if self.position.is_none() || !matches!(event, LaneEvent::Heard { .. }) {
            return false;
        }
        let Some(spoken) = self.take_live(|spoken| spoken.utterance.item == item) else {
            return false;
        };
        self.held.push(Held {
            spoken,
            event: event.clone(),
            since: now,
        });
        let lanes = self.release_held(now, false);
        inputs.extend(lanes.into_iter().map(PolicyInput::Lane));
        true
    }

    /// The held utterances the client's reports now settle, in order: heard when it played
    /// one to its end, heard in part when it went past one or the speech was cut, and as the
    /// pump estimated when it stopped reporting.
    fn release_held(&mut self, now: Ms, cut: bool) -> Vec<LaneEvent> {
        let mut lanes = Vec::new();
        let lapsed = self
            .position
            .is_none_or(|(_, _, at)| now >= at + REPORT_LAPSE_MS);
        while let Some(held) = self.held.first() {
            let utt = held.spoken.utt;
            let item = held.spoken.utterance.item;
            let played = self.played.get(&utt).copied();
            let past = self.position.is_some_and(|(at, _, _)| at > utt);
            let event = match played {
                Some((ms, at)) if ms + PLAYED_TOLERANCE_MS >= held.spoken.delivered_ms => {
                    LaneEvent::Heard { item, heard_at: at }
                }
                _ if past || cut => {
                    let ms = played.map_or(0, |(ms, _)| ms);
                    let prefix = heard_prefix(&held.spoken.text, ms, self.rate.chars_per_ms());
                    LaneEvent::Dropped {
                        item,
                        heard_chars: prefix.chars().count(),
                        cause: CutCause::Client,
                    }
                }
                _ if lapsed && now >= held.since + REPORT_LAPSE_MS => held.event.clone(),
                _ => break,
            };
            let held = self.held.remove(0);
            self.live.push_front(held.spoken);
            self.released.insert(item);
            lanes.push(event);
        }
        lanes
    }

    /// The effects of a lane event: transcripts, the heard ledger and the latency metrics.
    /// `failed` is the utterance a pump event of this loop turn reported as failed.
    fn on_lane(&mut self, event: &LaneEvent, failed: Option<UttId>) {
        match *event {
            LaneEvent::Started { utt, at, .. } => {
                self.error_owed = false;
                let Some(spoken) = self.live.iter_mut().find(|spoken| spoken.utt == utt) else {
                    return;
                };
                // The voice speaks again (chapter 10): a real synthesis, not a line from the
                // speech cache (live SF3: a cached line played while the vendor was down).
                let voice_back =
                    self.watcher.is_failing(Service::Voice) && !self.ctl.was_cached(utt);
                spoken.started = true;
                let (lane, kind) = (spoken.utterance.lane, spoken.utterance.kind);
                // A new answer: what an earlier cut left unheard is not resumed any more.
                if matches!(kind, Kind::Relay | Kind::Answer | Kind::Update) {
                    self.unheard.clear();
                }
                let task = spoken
                    .utterance
                    .task
                    .as_ref()
                    .map(|(task, _)| task.0.clone());
                if spoken.utterance.kind == Kind::Relay {
                    // An answer sentence is never in the cache: this is a real synthesis.
                    let took = at.saturating_sub(spoken.submitted);
                    self.policy.synth_took(took, spoken.alone);
                }
                if let Some(record) = &mut self.record {
                    let answer = matches!(kind, Kind::Relay | Kind::Answer);
                    let reply = spoken.utterance.reply_to == Some(record.turn);
                    record.audible(at, answer, reply);
                }
                if let Some(turn) = &mut self.turn {
                    let ms = at.saturating_sub(turn.e) as f64;
                    if std::mem::take(&mut turn.first_audible_owed) {
                        bridge::observe_turn_response_latency_ms("interaction_first_audible", ms);
                        // What the caller waited: from the end of their speech, which the
                        // analyser reports one silence mark after the last voiced chunk.
                        if let Some(end) = turn.speech_end {
                            let spoke_until = end.saturating_sub(Ms::from(analyser::OFFSET_LAG_MS));
                            bridge::observe_turn_response_latency_ms(
                                "interaction_first_audible_from_speech_end",
                                at.saturating_sub(spoke_until) as f64,
                            );
                        }
                    }
                    if spoken.utterance.kind == Kind::Relay && std::mem::take(&mut turn.answer_owed)
                    {
                        bridge::observe_turn_response_latency_ms("interaction_answer", ms);
                    }
                }
                let event = transcript(self.turn.as_ref(), spoken, Some(&spoken.text), "none");
                self.send(&event, MessageClass::Transcript);
                let lane = if lane == Lane::Main { "main" } else { "front" };
                self.segment(at, "start", utt, lane, kind, task);
                if voice_back {
                    self.service_recovered(Service::Voice);
                }
                self.prime_line(self.ctl.now_ms());
            }
            LaneEvent::Heard { item, .. } => {
                let heard = |spoken: &Spoken| spoken.utterance.item == item;
                let Some(spoken) = self.take_live(heard) else {
                    return;
                };
                self.line_ended(&spoken);
                let event = transcript(self.turn.as_ref(), &spoken, None, "full");
                self.send(&event, MessageClass::Transcript);
                if spoken.utterance.kind == Kind::Relay {
                    self.heard_answer(spoken.utterance.task.as_ref(), &spoken.text, false);
                }
                // Stepped down, the history is the plain path's: the caller and the answers.
                if !self.plain || spoken.utterance.kind == Kind::Relay {
                    self.ledger.push(ChatMessage::assistant(spoken.text));
                }
            }
            LaneEvent::Dropped {
                item,
                heard_chars,
                cause,
            } => {
                // A failure names its utterance when a pump event reported it.
                let failed = failed.filter(|_| cause == CutCause::Failure);
                let by_utt = self.take_live(|spoken| Some(spoken.utt) == failed);
                let dropped = by_utt.or_else(|| self.take_live(|s| s.utterance.item == item));
                let Some(spoken) = dropped else {
                    return;
                };
                self.line_ended(&spoken);
                // A line of the agent's that the voice could not give (chapter 10): a stall is
                // due unless the next one succeeds within the stall time.
                if cause == CutCause::Failure {
                    let due = self.ctl.now_ms() + self.stall_after();
                    self.service_failing(Service::Voice, due);
                }
                let heard: String = spoken.text.chars().take(heard_chars).collect();
                if spoken.utterance.kind == Kind::Relay {
                    self.heard_answer(spoken.utterance.task.as_ref(), &heard, true);
                }
                // What the caller cut off is said again on `resume`, from its first sentence
                // they did not hear in full (chapter 9).
                let said_again = matches!(
                    spoken.utterance.kind,
                    Kind::Relay | Kind::Answer | Kind::Update
                );
                if cause == CutCause::Interrupt && said_again {
                    let rest = unheard_rest(&spoken.text, heard_chars);
                    if !rest.is_empty() {
                        self.unheard.push(rest.to_owned());
                    }
                }
                if !heard.is_empty() {
                    let event = transcript(self.turn.as_ref(), &spoken, None, "partial");
                    self.send(&event, MessageClass::Transcript);
                    // The start of an acknowledgement or a holding line says nothing: in the
                    // background's history "Got it," read as the answer it should give next.
                    let filler = matches!(
                        spoken.utterance.kind,
                        Kind::Ack | Kind::Hold | Kind::ChoiceOffer | Kind::Checkin | Kind::Progress
                    );
                    // The history has what the caller heard, and where they cut it off.
                    if !filler && cause == CutCause::Interrupt {
                        self.ledger
                            .push(ChatMessage::assistant(format!("{heard} {CUT_MARKER}")));
                    } else if !filler {
                        self.ledger.push(ChatMessage::assistant(heard));
                    }
                } else if cause == CutCause::Failure && !spoken.started {
                    // No audio came of it: the client still gets the words.
                    let words = Some(spoken.text.as_str());
                    let event = transcript(self.turn.as_ref(), &spoken, words, "none");
                    self.send(&event, MessageClass::Transcript);
                }
            }
            LaneEvent::Returned { item } => {
                // Never heard: the one with the provider. One still queued was never ours.
                let returned = self.take_live(|s| s.utterance.item == item && !s.started);
                if let Some(spoken) = &returned {
                    self.line_ended(spoken);
                }
                if let Some(spoken) = returned.filter(|s| s.utterance.kind == Kind::Relay) {
                    self.heard_answer(spoken.utterance.task.as_ref(), "", true);
                }
            }
            LaneEvent::Idle { .. } => {}
        }
    }

    /// The caller heard `heard` more of the answer of a task revision. `cut`: the rest of
    /// that answer will not be heard, which an agent's next run is told.
    fn heard_answer(&mut self, answer: Option<&(TaskId, u32)>, heard: &str, cut: bool) {
        let Some((run, text)) = answer.and_then(|answer| self.answers.get_mut(answer)) else {
            return;
        };
        if !heard.is_empty() {
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(heard);
        }
        self.background.heard(*run, text, cut);
    }

    /// A line of the agent's ended, heard or cut: after the persona's line of a hand-off to a
    /// person, the caller is put through.
    fn line_ended(&mut self, spoken: &Spoken) {
        let human = matches!(&spoken.utterance.say, Say::Template { id, .. } if *id == HUMAN_LINE);
        if human {
            self.hand_off();
        }
    }

    fn take_live(&mut self, which: impl Fn(&Spoken) -> bool) -> Option<Spoken> {
        let at = self.live.iter().position(which)?;
        self.live.remove(at)
    }

    /// The client event of a task event. `actions` is what the policy does about it.
    fn emit_task(&mut self, event: &TaskEvent, actions: &[PolicyAction]) {
        let said = |action: &PolicyAction| match action {
            PolicyAction::Speak(utterance) => self.persona.resolve(&utterance.say, 0),
            _ => None,
        };
        let spoken = actions
            .iter()
            .any(|action| matches!(action, PolicyAction::Speak(_)));
        let Some(mut wire) = events::task_event_to_wire(event, spoken) else {
            return;
        };
        let mut link = None;
        if let WireEvent::TaskQuestion { prompt, .. } = &mut wire {
            // The exact words the question is asked with (not the line that breaks in). A
            // question that waits for a slot keeps its own words for the screen.
            let asked = |action: &&PolicyAction| !matches!(action, PolicyAction::Speak(line) if line.kind == Kind::Notice);
            let words: Vec<String> = actions.iter().filter(asked).filter_map(said).collect();
            if !words.is_empty() {
                *prompt = words.join(" ");
            }
            // A sign-in link: the caller is told once it is on its way to the screen.
            if let TaskEventKind::InputRequired(request) = &event.kind
                && request.kind == HitlKind::AuthUrl
            {
                link = Some(request.id.clone());
            }
        }
        self.send_task(&wire, link);
    }

    /// A task's event to the client, the room, and the readers of `GET /v1/tasks/events`.
    fn send_task(&mut self, wire: &WireEvent, delivered: Option<HitlId>) {
        self.send_with(wire, MessageClass::Critical, delivered);
        if let Ok(event) = serde_json::to_value(wire) {
            tasks::store().publish(&self.stored, event);
        }
    }

    fn send(&mut self, event: &WireEvent, class: MessageClass) {
        self.send_with(event, class, None);
    }

    fn send_with(&mut self, event: &WireEvent, class: MessageClass, delivered: Option<HitlId>) {
        if self.last_sent.is_some() {
            self.last_sent = Some(self.ctl.now_ms());
        }
        if let Ok(json) = serde_json::to_string(event) {
            let _ = self.events.0.send((json, class, delivered));
        }
    }

    /// Waits until every event sent so far is on the session's egress channel; later events
    /// go through a new forwarder.
    async fn drain(&mut self) {
        let fresh = forward(self.out.clone(), self.room.clone(), self.tx.clone());
        let (events, forwarder) = std::mem::replace(&mut self.events, fresh);
        drop(events);
        let _ = forwarder.await;
    }

    /// The session is over: speech stops and every task is told. A task that goes on (its
    /// tool call is on the wire, or `tasks.session_end` keeps it) ends for whoever comes for
    /// it: after a lost connection the caller, for the session's grace; else the push.
    /// After a call with a verified caller (caller memory), the background model writes what
    /// is worth keeping about the caller, on its own task: nobody waits for it.
    fn remember_caller(&self) {
        let (Some(sub), Some(key), Some(llm)) = (
            self.cfg.front.caller.clone(),
            self.server.caller_memory_key.clone(),
            self.background.llm(),
        ) else {
            return;
        };
        let spoke = |message: &ChatMessage| message.role == MessageRole::User;
        if !self.ledger.iter().any(spoke) {
            return;
        }
        let (conversation, known) = (self.ledger.clone(), self.cfg.front.memory.clone());
        let api_key = self.credential.as_ref().map(SessionCredential::current);
        let (tenant, history_key) = (
            self.stored.owner.clone(),
            format!("{}#memory", self.stream_id),
        );
        tokio::spawn(async move {
            let facts = background::caller_facts(
                &llm,
                api_key.as_deref(),
                &known,
                &conversation,
                &history_key,
            )
            .await;
            if facts.is_empty() {
                return;
            }
            match tenant::add_facts(&key, &tenant, &sub, facts).await {
                Ok(added) => info!(added, "the background added to the caller's memory"),
                Err(error) => warn!(%error, "the caller's memory was not written"),
            }
        });
    }

    fn end(&mut self, now: Ms, clean: bool) {
        self.cut(now, CutCause::Shutdown);
        // A grace of zero is a session nobody comes back to. With a resume key the caller may
        // call back after a hang-up too.
        let back = !clean || self.stored.resume.is_some();
        let grace = Some(self.stored.grace).filter(|grace| back && !grace.is_zero());
        let ended = TaskCmd::SessionEnded {
            push: self.cfg.tasks.push_url.is_some(),
            grace_ms: grace.map_or(0, |grace| grace.as_millis() as Ms),
        };
        self.task_unspoken(now, ended);
        self.gone = Some(grace.map(|grace| tokio::time::Instant::now() + grace));
        if grace.is_some() && self.registry.next_wakeup().is_some() {
            tasks::store().detach(&self.stored);
        }
        let (chunk_avg_us, chunk_max_us) = self.ctl.chunk_cost().avg_max_us();
        let (frame_avg_us, frame_max_us) = self.ctl.frame_cost().avg_max_us();
        let phone_leg = self.ctl.phone_leg();
        info!(
            stream_id = %self.stream_id,
            turns = self.turn.as_ref().map_or(0, |turn| turn.id.0),
            tasks = self.registry.board().tasks.len(),
            phone_leg,
            echo_canceller = self.cfg.duplex.echo.aec_on(phone_leg),
            analyser_chunk_avg_us = chunk_avg_us,
            analyser_chunk_max_us = chunk_max_us,
            pump_frame_avg_us = frame_avg_us,
            pump_frame_max_us = frame_max_us,
            "interaction_session_summary"
        );
    }

    /// A machine panicked: speech and runs stop, the client is told, the actor ends. The
    /// session's socket and its speech-to-text forwarding are not touched.
    async fn fail(&mut self) {
        self.ctl.cut();
        let _ = self.speech.send(SpeechCmd::Cut);
        for (_, cancel) in self.runs.drain() {
            cancel.cancel();
        }
        // Nothing will finish these tasks; the store must not keep them as running.
        for mut view in self.registry.board().tasks {
            if view.state.is_live() {
                view.state = TaskState::Unknown;
                view.open_question = None;
                tasks::store().write(&self.stored, view);
            }
        }
        bridge::record_degraded("interaction", "actor_panicked");
        let event = WireEvent::Error {
            message: "interaction mode stopped after an internal error".to_owned(),
            code: "interaction_failed".to_owned(),
        };
        self.send(&event, MessageClass::Critical);
        self.drain().await;
    }
}

/// The mark a line the caller cut off carries in the history (chapter 9).
const CUT_MARKER: &str = "[cut off by the caller]";

/// The part of `text` from the first sentence the caller did not hear in full, after
/// `heard_chars` of it were heard.
fn unheard_rest(text: &str, heard_chars: usize) -> &str {
    let heard: usize = text.chars().take(heard_chars).map(char::len_utf8).sum();
    let start = text[..heard]
        .rfind(['.', '!', '?'])
        .map_or(0, |end| end + 1);
    text[start..].trim()
}

fn template(id: &'static str) -> Say {
    Say::Template {
        id,
        slots: Vec::new(),
    }
}

fn is_failure(event: &LaneEvent) -> bool {
    matches!(
        event,
        LaneEvent::Dropped {
            cause: CutCause::Failure,
            ..
        }
    )
}

fn utterance_id(utt: UttId) -> String {
    format!("u_{}", utt.0)
}

/// The wire `kind` of an utterance: the published kinds have no value for a client's own line.
fn wire_kind(kind: Kind) -> Kind {
    match kind {
        Kind::Client => Kind::Notice,
        kind => kind,
    }
}

/// The `agent_audio_stopped` reason of a cut.
fn stop_reason(cause: CutCause) -> &'static str {
    match cause {
        CutCause::Interrupt | CutCause::SideTalkTimeout => "barge_in",
        CutCause::Client => "client_clear",
        CutCause::PreEmpted | CutCause::Shutdown => "policy",
        CutCause::Failure => "provider",
    }
}

/// The start of `text` that `played_ms` of audio covered, cut back to a whole word.
fn heard_prefix(text: &str, played_ms: u64, chars_per_ms: f64) -> String {
    let mut spoken = SpokenLedger::default();
    spoken.push(text, text, true, 0);
    spoken.spoken_prefix(played_ms, chars_per_ms).text
}

/// The `assistant_transcript` of an utterance: with its words when it starts playing, without
/// them when only `heard` is new.
fn transcript(
    turn: Option<&Turn>,
    spoken: &Spoken,
    words: Option<&str>,
    heard: &'static str,
) -> WireEvent {
    let utterance = &spoken.utterance;
    let relay = utterance.kind == Kind::Relay;
    let front = matches!(utterance.say, Say::Front(_));
    WireEvent::AssistantTranscript {
        turn_index: turn.map_or(0, |turn| turn.id.0),
        delta: words.unwrap_or_default().to_owned(),
        source: match (relay, front) {
            (true, _) => "background",
            (false, true) => "front",
            (false, false) => "gateway",
        },
        kind: wire_kind(utterance.kind),
        verbatim: relay,
        act: turn
            .filter(|turn| front && utterance.reply_to == Some(turn.id))
            .and_then(|turn| turn.act),
        task_id: utterance.task.as_ref().map(|(task, _)| task.0.clone()),
        utterance_id: utterance_id(spoken.utt),
        sentence: utterance
            .sentence
            .map(|(attempt, seq)| Sentence { attempt, seq }),
        heard,
    }
}

#[cfg(test)]
mod tests {
    use super::super::types::ItemId;
    use super::*;
    use crate::core::llm::LlmClientConfig;
    use crate::core::stt::STTConfig;
    use crate::core::tts::TTSConfig;
    use crate::core::voice_manager::VoiceManagerConfig;

    /// A recogniser that counts its disconnects.
    struct Counted(Arc<std::sync::atomic::AtomicUsize>);

    #[async_trait::async_trait]
    impl crate::core::stt::BaseSTT for Counted {
        fn new(_: STTConfig) -> Result<Self, crate::core::stt::STTError> {
            Ok(Counted(Arc::default()))
        }
        async fn connect(&mut self) -> Result<(), crate::core::stt::STTError> {
            Ok(())
        }
        async fn disconnect(&mut self) -> Result<(), crate::core::stt::STTError> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
        fn is_ready(&self) -> bool {
            true
        }
        async fn send_audio(&mut self, _: bytes::Bytes) -> Result<(), crate::core::stt::STTError> {
            Ok(())
        }
        async fn on_result(
            &mut self,
            _: crate::core::stt::STTResultCallback,
        ) -> Result<(), crate::core::stt::STTError> {
            Ok(())
        }
        async fn on_error(
            &mut self,
            _: crate::core::stt::STTErrorCallback,
        ) -> Result<(), crate::core::stt::STTError> {
            Ok(())
        }
        fn get_config(&self) -> Option<&STTConfig> {
            None
        }
        async fn update_config(&mut self, _: STTConfig) -> Result<(), crate::core::stt::STTError> {
            Ok(())
        }
        fn get_provider_info(&self) -> &'static str {
            "counted"
        }
    }

    /// A recogniser opened for a switch that is never swapped in (a later switch replaced it,
    /// or the session ended) is disconnected, whatever its provider; one swapped in is not.
    #[tokio::test]
    async fn an_unused_recogniser_is_disconnected_not_dropped() {
        let closed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let next = NextStt(Some(Box::new(Counted(closed.clone()))));
        drop(next);
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
        assert_eq!(closed.load(std::sync::atomic::Ordering::SeqCst), 1);
        let used = NextStt(Some(Box::new(Counted(closed.clone()))));
        let stt = used.take().expect("the recogniser");
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
        assert_eq!(closed.load(std::sync::atomic::Ordering::SeqCst), 1);
        drop(stt);
    }

    fn spoken(kind: Kind, say: Say) -> Spoken {
        Spoken {
            utt: UttId(7),
            text: "The store opens at nine.".to_owned(),
            utterance: Utterance {
                item: ItemId(3),
                lane: Lane::Main,
                kind,
                rank: 4,
                say,
                discard: false,
                task: Some((TaskId("task_ab".to_owned()), 1)),
                reply_to: Some(TurnId(2)),
                question: None,
                expires_at: None,
                sentence: Some((0, 1)),
                last: false,
            },
            started: true,
            submitted: 0,
            alone: true,
            delivered_ms: 0,
        }
    }

    fn turn(act: Act) -> Turn {
        Turn {
            id: TurnId(2),
            e: 1_000,
            speech_end: None,
            addressed: true,
            typed: false,
            language_named: false,
            unclear: false,
            barged: false,
            act: Some(act),
            first_audible_owed: false,
            answer_owed: false,
        }
    }

    #[test]
    fn a_relayed_sentence_is_a_verbatim_background_transcript() {
        let relay = spoken(
            Kind::Relay,
            Say::Verbatim("The store opens at nine.".into()),
        );
        let turn = turn(Act::Delegate);
        let started = transcript(Some(&turn), &relay, Some(&relay.text), "none");
        assert_eq!(
            serde_json::to_value(&started).unwrap(),
            serde_json::json!({
                "type": "assistant_transcript", "turn_index": 2,
                "delta": "The store opens at nine.", "source": "background", "kind": "relay",
                "verbatim": true, "task_id": "task_ab", "utterance_id": "u_7",
                "sentence": {"attempt": 0, "seq": 1}, "heard": "none",
            })
        );
        // The update when it was heard repeats everything but the words.
        let heard = serde_json::to_value(transcript(Some(&turn), &relay, None, "full")).unwrap();
        assert_eq!(heard["delta"], "");
        assert_eq!(heard["heard"], "full");
        assert_eq!(heard["utterance_id"], "u_7");
    }

    #[test]
    fn only_a_line_the_front_wrote_carries_its_act() {
        let turn = turn(Act::Social);
        let front = spoken(Kind::Answer, Say::Front("You're welcome.".into()));
        let event = serde_json::to_value(transcript(Some(&turn), &front, None, "full")).unwrap();
        assert_eq!(
            (&event["source"], &event["act"]),
            (&"front".into(), &"social".into())
        );

        let bank = Say::Bank {
            group: "ack",
            intent: None,
        };
        let event = transcript(Some(&turn), &spoken(Kind::Ack, bank), None, "full");
        let event = serde_json::to_value(event).unwrap();
        assert_eq!(event["source"], "gateway");
        assert_eq!(event["verbatim"], false);
        assert!(event.get("act").is_none());
    }

    #[test]
    fn the_heard_prefix_ends_on_a_whole_word() {
        let text = "The store opens at nine in the morning.";
        // 900 ms at 14.5 characters a second is 13 characters: "The store ope|ns".
        assert_eq!(heard_prefix(text, 900, 0.0145), "The store");
        assert_eq!(heard_prefix(text, 1_100, 0.0145), "The store opens");
        assert_eq!(heard_prefix(text, 0, 0.0145), "");
        assert_eq!(heard_prefix(text, 60_000, 0.0145), text);
    }

    /// What the loop does when a machine panics, on providers that are never connected and
    /// with no session left to start a run for.
    #[tokio::test]
    async fn after_a_panic_speech_and_tasks_stop_and_the_client_is_told() {
        let stt = STTConfig {
            provider: "deepgram".to_owned(),
            api_key: "test_key".to_owned(),
            ..Default::default()
        };
        let tts = TTSConfig {
            provider: "deepgram".to_owned(),
            api_key: "test_key".to_owned(),
            ..Default::default()
        };
        let vm = VoiceManager::new(VoiceManagerConfig::new(stt, tts), None).expect("built");
        let (pump_tx, pump_rx) = mpsc::unbounded_channel();
        let (speech, mut spoken) = mpsc::unbounded_channel();
        let (out, mut sent) = mpsc::channel(16);
        let (inbox, _) = mpsc::channel(1);
        let owner = "owner-of-the-panic-test";
        let deps = ActorDeps {
            vm: Arc::new(vm),
            ctl: Arc::new(PumpControl::new(pump_tx, 150)),
            pump_rx,
            speech,
            out,
            background: Background::Llm(Arc::new(LlmClient::new(LlmClientConfig::default())), None),
            system_prompt: None,
            credential: None,
            attribution: None,
            server: Arc::default(),
            cfg: InteractionConfig::default(),
            stream_id: "stream-of-the-panic-test".to_owned(),
            owner: owner.to_owned(),
            refs_used: 0,
            request_response: false,
            provider_turns: false,
            stt_end_turn_after_ms: None,
            warm: true,
            plain_path: false,
            room: None,
            placement: None,
        };
        let (mut actor, _pump_rx) = Actor::new(deps, inbox.downgrade());
        drop(inbox);

        // A delegated turn: its task is working when the machines give up.
        let turn = Msg::TypedTurn("what time does the store open".to_owned());
        actor.step(0, Trigger::Msg(turn));
        // The front's decision (this test has no front model to ask).
        let delegate = ActDecision {
            act: Act::Delegate,
            task: None,
            wait: None,
            say: None,
            intent: None,
            source: ActSource::Model,
        };
        let done = FrontEvent::Done {
            turn: TurnId(1),
            decision: delegate,
        };
        actor.decide(0, PolicyInput::Front(done), None, &mut VecDeque::new());
        actor.drain().await;
        assert_eq!(
            tasks::store().list(owner, None)[0].state,
            TaskState::Working
        );

        actor.fail().await;
        let cmds: Vec<SpeechCmd> = std::iter::from_fn(|| spoken.try_recv().ok()).collect();
        assert_eq!(cmds.last(), Some(&SpeechCmd::Cut));
        let mut last = None;
        while let Ok(MessageRoute::Json(event)) = sent.try_recv() {
            last = Some(event);
        }
        let last: serde_json::Value = serde_json::from_str(&last.expect("an event")).unwrap();
        assert_eq!(last["type"], "error");
        assert_eq!(last["code"], "interaction_failed");
        // Nothing will finish the task: the store does not keep it as running.
        assert_eq!(
            tasks::store().list(owner, None)[0].state,
            TaskState::Unknown
        );
    }

    #[test]
    fn only_a_vendors_own_confidence_counts() {
        let result =
            |confidence: f32| STTResult::new("flight to the".to_owned(), true, true, confidence);
        // These providers pass the vendor's number through.
        assert_eq!(real_confidence("deepgram", &result(0.31)), Some(0.31));
        assert_eq!(real_confidence("revai", &result(0.42)), Some(0.42));
        assert_eq!(real_confidence("aws-transcribe", &result(0.6)), Some(0.6));
        // 0.0 and 1.0 stand for "not sent" there.
        assert_eq!(real_confidence("deepgram", &result(0.0)), None);
        assert_eq!(real_confidence("gladia", &result(1.0)), None);
        // The constants and derived figures of the others are no confidence.
        for (provider, filled) in [
            ("groq", 0.5),
            ("openai", 0.3),
            ("cartesia", 0.0),
            ("sarvam", 0.95),
            ("elevenlabs", 0.2),
            ("no-such-provider", 0.1),
        ] {
            assert_eq!(
                real_confidence(provider, &result(filled)),
                None,
                "{provider}"
            );
        }
        // What the vendor itself reported always counts.
        let mut google = result(0.0);
        google.vendor_confidence = Some(0.44);
        assert_eq!(real_confidence("google", &google), Some(0.44));
    }

    #[test]
    fn a_cut_names_its_reason_on_the_wire() {
        assert_eq!(stop_reason(CutCause::Interrupt), "barge_in");
        assert_eq!(stop_reason(CutCause::SideTalkTimeout), "barge_in");
        assert_eq!(stop_reason(CutCause::Client), "client_clear");
        assert_eq!(stop_reason(CutCause::PreEmpted), "policy");
        assert_eq!(stop_reason(CutCause::Failure), "provider");
    }
}
