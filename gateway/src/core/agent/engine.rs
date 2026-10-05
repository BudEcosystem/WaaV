//! The voice-agent turn engine (spec 025 §5.5, §5.7, §5.8).
//!
//! One engine per voice session. It turns end-of-user-turn events into agent turns — one budprompt
//! run per utterance through budgateway, streamed into TTS sentence by sentence — and owns what
//! happens when the user talks over the agent:
//!
//! * the TTS queue is cleared and the run's stream is dropped (budprompt records it with whatever it
//!   had delivered, and a best-effort `/cancel` names it cancelled);
//! * the SPOKEN prefix of the reply is computed from the audio actually played, and sent with the
//!   NEXT turn as `bud_truncate`, so budprompt replays only what the user heard (D-9);
//! * turns never overlap on one conversation: a new user turn ends the live one first (FR-TURN-2).
//!
//! The engine is transport-agnostic. It speaks through [`SpeechOut`] (the session's VoiceManager in
//! production, a recorder in tests), runs turns through [`TurnBackend`] ([`AgentBrain`] in
//! production) and reports everything a front-end shows as [`AgentSignal`]s on a channel — the `/ws`
//! front-end maps them to native messages, the `/v1/realtime` front-end to GA events.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use tokio::time::Instant;

use async_trait::async_trait;
use bud_auth::voice_agent::VoiceAgentEntry;
use futures::StreamExt;
use futures::stream::BoxStream;
use parking_lot::Mutex;
use serde_json::{Map, Value};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, warn};

use super::brain::{AgentBrain, AgentEvent, AgentTurnRequest, Truncate, TurnFailure};
use super::spoken::{SpeechRate, SpokenLedger};
use super::text::{
    CodeFenceFilter, SpeakFieldExtractor, SpeechChunker, chunk_separator, transform_for_speech,
};
use crate::core::text::ThinkStripper;

// =============================================================================================
// Seams
// =============================================================================================

/// Where the engine's speech goes. The VoiceManager in production.
#[async_trait]
pub trait SpeechOut: Send + Sync + 'static {
    /// The TTS clear epoch: a clear after capture invalidates later speaks of that turn.
    fn clear_epoch(&self) -> usize;
    /// Enqueue one chunk; `false` when a clear (barge-in) happened since `epoch`, or it failed.
    async fn speak(&self, text: &str, epoch: usize, interruptible: bool) -> bool;
    /// Stop speaking: drop queued and playing interruptible audio.
    async fn clear(&self);
    /// Whether the caller is (estimated to be) hearing the agent right now.
    fn is_audible(&self) -> bool;
    /// Monotonic total of TTS audio delivered to the caller, in ms.
    fn audio_out_ms(&self) -> u64;
    /// Delivered audio not played yet, in ms.
    fn playout_remaining_ms(&self) -> u64;
}

#[async_trait]
impl SpeechOut for crate::core::voice_manager::VoiceManager {
    fn clear_epoch(&self) -> usize {
        crate::core::voice_manager::VoiceManager::clear_epoch(self)
    }
    async fn speak(&self, text: &str, epoch: usize, interruptible: bool) -> bool {
        match self.speak_if_epoch(text, true, interruptible, epoch).await {
            Ok(spoken) => spoken,
            Err(e) => {
                warn!(error = %e, "agent speech failed");
                false
            }
        }
    }
    async fn clear(&self) {
        if let Err(e) = self.clear_tts().await {
            warn!(error = %e, "clearing agent speech failed");
        }
    }
    fn is_audible(&self) -> bool {
        self.is_bot_speaking()
    }
    fn audio_out_ms(&self) -> u64 {
        crate::core::voice_manager::VoiceManager::audio_out_ms(self)
    }
    fn playout_remaining_ms(&self) -> u64 {
        crate::core::voice_manager::VoiceManager::playout_remaining_ms(self)
    }
}

/// What runs a turn. [`AgentBrain`] in production.
#[async_trait]
pub trait TurnBackend: Send + Sync + 'static {
    async fn open(
        &self,
        request: &AgentTurnRequest,
        bearer: &str,
        traceparent: Option<&str>,
    ) -> Result<BoxStream<'static, Result<AgentEvent, TurnFailure>>, TurnFailure>;
    async fn cancel(&self, response_id: &str, prompt_name: &str, bearer: &str);
}

#[async_trait]
impl TurnBackend for AgentBrain {
    async fn open(
        &self,
        request: &AgentTurnRequest,
        bearer: &str,
        traceparent: Option<&str>,
    ) -> Result<BoxStream<'static, Result<AgentEvent, TurnFailure>>, TurnFailure> {
        AgentBrain::open(self, request, bearer, traceparent)
            .await
            .map(|s| s.boxed())
    }
    async fn cancel(&self, response_id: &str, prompt_name: &str, bearer: &str) {
        AgentBrain::cancel(self, response_id, prompt_name, bearer).await
    }
}

// =============================================================================================
// Signals to the front-end
// =============================================================================================

/// How a response ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnStatus {
    Completed,
    /// Barge-in or `response.cancel`.
    Cancelled,
    Failed,
    /// A governance withhold or another incomplete ending.
    Incomplete,
}

impl TurnStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
            Self::Incomplete => "incomplete",
        }
    }
}

/// What kind of response a turn is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnKind {
    /// An agent turn: budprompt answered.
    Agent,
    /// The static greeting: spoken by WaaV, never part of the agent's history.
    Greeting,
    /// Something the gateway says itself (a lost turn on a segmented session): never part of the
    /// agent's history, always interruptible.
    Notice,
}

/// Everything a front-end shows, in the order it happened.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentSignal {
    ResponseStarted {
        turn: u64,
        kind: TurnKind,
        /// The user's words that started it (None for a greeting).
        input: Option<String>,
    },
    /// budprompt's id for the run (the one `/v1/responses/{id}` and the console show).
    ResponseCreated { turn: u64, response_id: String },
    /// Text as it is SPOKEN (audio mode) or generated (text mode).
    Transcript { turn: u64, delta: String },
    /// A server-side tool's progress: `in_progress`, `completed` or `failed`.
    Tool {
        turn: u64,
        item_id: String,
        name: String,
        status: &'static str,
    },
    /// A structured-output agent's JSON (D-13).
    Output {
        turn: u64,
        response_id: Option<String>,
        output: Value,
    },
    ResponseDone {
        turn: u64,
        kind: TurnKind,
        response_id: Option<String>,
        status: TurnStatus,
        usage: Option<Value>,
        /// What the caller heard (audio mode) or was shown (text mode).
        transcript: String,
    },
    /// The reply was cut: history keeps `spoken` (+ marker) from the next turn on.
    Truncated {
        turn: u64,
        response_id: Option<String>,
        spoken: String,
        audio_end_ms: u64,
    },
    /// A non-fatal problem the client should hear about (§5.8).
    Error { code: &'static str, message: String },
    /// The caller was silent for `idle.end_after_silence_ms`: the front-end ends the session.
    Idle,
}

// =============================================================================================
// The engine
// =============================================================================================

/// What a session runs an agent with.
#[derive(Debug, Clone)]
pub struct AgentSessionConfig {
    pub session_id: String,
    /// The name the caller addressed the agent by, without `prompt:`.
    pub prompt_name: String,
    pub version: i64,
    pub entry: Arc<VoiceAgentEntry>,
    /// `conv_voice_<session>` — every turn of the session continues one budprompt conversation.
    pub conversation_id: String,
    pub variables: Option<Map<String, Value>>,
    /// GA `output_modalities: ["text"]`: no TTS; text goes to the client as it streams.
    pub text_only: bool,
}

/// Backoff before each retry of a turn whose request never reached the agent.
const OPEN_RETRY_BACKOFF: [Duration; 2] = [Duration::from_millis(250), Duration::from_millis(750)];

/// How long a turn waits for its audio to finish synthesizing before `ResponseDone` (GA order:
/// every audio delta precedes `response.done`).
const MAX_DRAIN: Duration = Duration::from_secs(30);
const DRAIN_QUIET: Duration = Duration::from_millis(600);
/// How often fillers and the drain are checked.
const TICK: Duration = Duration::from_millis(100);

/// Where a turn is in its filler list (D-16). Each turn starts from the first phrase.
#[derive(Debug, Default)]
struct FillerCursor {
    next: usize,
    /// When the last filler of this turn was due; `None` until the first.
    last_at: Option<Instant>,
}

/// Whether the list's next filler is due: the first once the answer is slow to start, each next
/// one while the caller is still waiting (nothing of the reply heard yet, or a tool still running).
fn list_filler_due(
    fillers: &bud_auth::voice_agent::AgentFillers,
    last_at: Option<Instant>,
    started: Instant,
    spoke_reply: bool,
    tool_running: bool,
) -> bool {
    match last_at {
        None => {
            !spoke_reply
                && !tool_running
                && fillers.slow_response_after_ms > 0
                && started.elapsed() >= Duration::from_millis(fillers.slow_response_after_ms)
        }
        Some(at) => {
            (!spoke_reply || tool_running)
                && fillers.follow_up_after_ms > 0
                && at.elapsed() >= Duration::from_millis(fillers.follow_up_after_ms)
        }
    }
}

impl FillerCursor {
    fn take(&mut self, messages: &[String]) -> Option<String> {
        let phrase = messages.get(self.next)?.clone();
        self.next += 1;
        Some(phrase)
    }
}

#[derive(Debug)]
struct TurnShared {
    kind: TurnKind,
    response_id: Option<String>,
    ledger: SpokenLedger,
    /// Every character generated (audio mode) — what history holds unless truncated.
    generated: String,
    /// What the caller was shown (text mode) or heard so far (audio mode).
    transcript: String,
    /// Set once this turn has been truncated, so a later estimate never overrides an exact one.
    truncated_exact: bool,
}

impl TurnShared {
    fn new(kind: TurnKind) -> Self {
        Self {
            kind,
            response_id: None,
            ledger: SpokenLedger::default(),
            generated: String::new(),
            transcript: String::new(),
            truncated_exact: false,
        }
    }
}

struct ActiveTurn {
    id: u64,
    token: CancellationToken,
    shared: Arc<Mutex<TurnShared>>,
}

/// One voice session's agent.
pub struct AgentEngine {
    cfg: Mutex<AgentSessionConfig>,
    entry: Arc<VoiceAgentEntry>,
    backend: Arc<dyn TurnBackend>,
    speech: Arc<dyn SpeechOut>,
    credential: crate::auth::SessionCredential,
    signals: mpsc::UnboundedSender<AgentSignal>,
    turn: Mutex<Option<ActiveTurn>>,
    /// The most recent finished turn, while its audio may still be playing.
    last: Mutex<Option<(u64, Arc<Mutex<TurnShared>>)>>,
    pending_truncate: Mutex<Option<Truncate>>,
    next_turn: AtomicU64,
    rate: Mutex<SpeechRate>,
    idle_generation: AtomicU64,
    /// At least one agent turn has started (variables can no longer change: D-14).
    started: AtomicBool,
    closed: AtomicBool,
    /// `manual` turn detection: transcribed speech waits here until the client commits it.
    manual_input: Mutex<String>,
    /// The client's choice of manual turns (GA `turn_detection: null`), over the agent's.
    manual_override: Mutex<Option<bool>>,
    /// Segmented sessions: the latest caller turn whose final transcript was handled, so a client
    /// commit waits for the transcript of the turn it sealed instead of a fixed sleep.
    final_turn: tokio::sync::watch::Sender<u64>,
}

impl std::fmt::Debug for AgentEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let cfg = self.cfg.lock();
        f.debug_struct("AgentEngine")
            .field("session_id", &cfg.session_id)
            .field("agent", &cfg.prompt_name)
            .field("version", &cfg.version)
            .finish()
    }
}

/// Whether `text` is only a backchannel the agent ignores while it speaks ("uh-huh", "okay").
pub fn is_ignored_phrase(text: &str, phrases: &[String]) -> bool {
    let norm = |s: &str| -> String {
        s.chars()
            .filter(|c| c.is_alphanumeric() || c.is_whitespace() || *c == '-')
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    };
    let t = norm(text);
    !t.is_empty() && phrases.iter().any(|p| norm(p) == t)
}

impl AgentEngine {
    pub fn new(
        cfg: AgentSessionConfig,
        backend: Arc<dyn TurnBackend>,
        speech: Arc<dyn SpeechOut>,
        credential: crate::auth::SessionCredential,
        signals: mpsc::UnboundedSender<AgentSignal>,
    ) -> Arc<Self> {
        let entry = Arc::clone(&cfg.entry);
        let rate = SpeechRate::new(entry.tts.speed);
        Arc::new(Self {
            cfg: Mutex::new(cfg),
            entry,
            backend,
            speech,
            credential,
            signals,
            turn: Mutex::new(None),
            last: Mutex::new(None),
            pending_truncate: Mutex::new(None),
            next_turn: AtomicU64::new(0),
            rate: Mutex::new(rate),
            idle_generation: AtomicU64::new(0),
            started: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            manual_input: Mutex::new(String::new()),
            manual_override: Mutex::new(None),
            final_turn: tokio::sync::watch::channel(0).0,
        })
    }

    /// A segmented session handled the final transcript of caller turn `turn_id`.
    pub fn note_final_turn(&self, turn_id: u64) {
        self.final_turn.send_if_modified(|t| {
            let newer = turn_id > *t;
            if newer {
                *t = turn_id;
            }
            newer
        });
    }

    /// Wait, at most `timeout`, until the final transcript of caller turn `turn_id` was handled.
    pub async fn wait_final_turn(&self, turn_id: u64, timeout: std::time::Duration) -> bool {
        let mut rx = self.final_turn.subscribe();
        tokio::time::timeout(timeout, rx.wait_for(|t| *t >= turn_id))
            .await
            .is_ok_and(|r| r.is_ok())
    }

    /// Whether `text` is probably the agent's own words coming back (no echo cancellation on the
    /// caller's side): the whole of it appears in what the agent is saying or just said.
    pub fn is_probable_echo(&self, text: &str) -> bool {
        let norm = |s: &str| -> String {
            s.chars()
                .filter(|c| c.is_alphanumeric() || c.is_whitespace())
                .collect::<String>()
                .to_lowercase()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        };
        let t = norm(text);
        if t.split_whitespace().count() < 2 {
            return false;
        }
        let spoken = {
            let active = self
                .turn
                .lock()
                .as_ref()
                .map(|a| a.shared.lock().generated.clone());
            let last = self
                .last
                .lock()
                .as_ref()
                .map(|(_, s)| s.lock().generated.clone());
            format!(
                "{} {}",
                active.unwrap_or_default(),
                last.unwrap_or_default()
            )
        };
        norm(&spoken).contains(&t)
    }

    /// Say `text` as the gateway's own notice: not part of the agent's history, interruptible.
    pub async fn speak_notice(self: &Arc<Self>, text: &str) {
        let text = text.trim().to_string();
        if text.is_empty() || self.is_closed() {
            return;
        }
        let id = self.next_turn.fetch_add(1, Ordering::AcqRel);
        let token = CancellationToken::new();
        let shared = Arc::new(Mutex::new(TurnShared::new(TurnKind::Notice)));
        *self.turn.lock() = Some(ActiveTurn {
            id,
            token: token.clone(),
            shared: Arc::clone(&shared),
        });
        self.signal(AgentSignal::ResponseStarted {
            turn: id,
            kind: TurnKind::Notice,
            input: None,
        });
        let text_only = self.cfg.lock().text_only;
        let epoch = self.speech.clear_epoch();
        let mut spoke = true;
        if !text_only {
            let speech = transform_for_speech(&text, &self.entry.text_transforms);
            let mark = self.speech.audio_out_ms();
            spoke = self.speech.speak(&speech, epoch, true).await;
            if spoke {
                shared.lock().ledger.push(&text, &speech, false, mark);
            }
        }
        if spoke {
            shared.lock().transcript = text.clone();
            self.signal(AgentSignal::Transcript {
                turn: id,
                delta: text,
            });
        }
        if !text_only {
            self.drain(&token, &shared).await;
        }
        let status = if token.is_cancelled() {
            TurnStatus::Cancelled
        } else {
            TurnStatus::Completed
        };
        self.finish(id, &shared, status, None);
    }

    pub fn entry(&self) -> &Arc<VoiceAgentEntry> {
        &self.entry
    }

    fn signal(&self, s: AgentSignal) {
        let _ = self.signals.send(s);
    }

    /// The agent's required variables the session has not supplied (D-14).
    pub fn missing_variables(&self) -> Vec<String> {
        let cfg = self.cfg.lock();
        self.entry
            .input
            .required_variables
            .iter()
            .filter(|name| {
                cfg.variables
                    .as_ref()
                    .is_none_or(|v| v.get(name.as_str()).is_none_or(Value::is_null))
            })
            .cloned()
            .collect()
    }

    /// Supply the session's variables (GA `session.prompt.variables`). Refused once a turn ran: the
    /// agent's structured input is per session, and changing it mid-conversation changes the agent.
    pub fn set_variables(&self, vars: Map<String, Value>) -> Result<(), &'static str> {
        if self.started.load(Ordering::Acquire) {
            return Err("prompt variables are fixed once the conversation has started");
        }
        self.cfg.lock().variables = Some(vars);
        Ok(())
    }

    /// GA `output_modalities`: text only, or audio.
    pub fn set_text_only(&self, text_only: bool) {
        self.cfg.lock().text_only = text_only;
    }

    pub fn has_active_turn(&self) -> bool {
        self.turn.lock().is_some()
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    // ---------------------------------------------------------------------------------------
    // Turn-taking entry points
    // ---------------------------------------------------------------------------------------

    /// Act on turn-decision events from the session's `TurnController`.
    pub async fn handle_turn_events(self: &Arc<Self>, events: &[crate::core::turn::TurnEvent]) {
        use crate::core::turn::TurnEvent;
        for event in events {
            match event {
                TurnEvent::Started {
                    interrupt: true, ..
                }
                | TurnEvent::BargeInMopUp => {
                    if self.accepts_speech() {
                        self.barge_in().await;
                    }
                }
                TurnEvent::Stopped { transcript, .. } => {
                    let t = transcript.trim();
                    if t.is_empty() {
                        continue;
                    }
                    // A new turn would end what is still greeting or answering: where that may not
                    // be cut, what the caller said meanwhile is not a turn.
                    if !self.accepts_speech() {
                        debug!("caller speech while the agent answers, interruptions off: dropped");
                        continue;
                    }
                    self.start_turn(t.to_string()).await;
                }
                _ => {}
            }
        }
    }

    /// Whether the caller's speech is input now: always while the agent is idle; while it greets
    /// or answers (thinking, or still heard), only if that may be cut — `greeting.interruptible`
    /// for the greeting, `interruption.enabled` for an answer. Speech that is not input is neither
    /// a turn nor an interruption, so nothing the caller says can cut what may not be cut.
    pub fn accepts_speech(&self) -> bool {
        let kind = match self.turn.lock().as_ref() {
            Some(a) => Some(a.shared.lock().kind),
            None if self.speech.is_audible() => {
                self.last.lock().as_ref().map(|(_, s)| s.lock().kind)
            }
            None => None,
        };
        match kind {
            Some(TurnKind::Greeting) => self.entry.greeting.interruptible,
            Some(TurnKind::Agent) => self.entry.interruption.enabled,
            Some(TurnKind::Notice) | None => true,
        }
    }

    /// Whether turns start only when the client commits (`turn_detection.type == "manual"`).
    pub fn is_manual(&self) -> bool {
        self.manual_override
            .lock()
            .unwrap_or(self.entry.turn_detection.kind == "manual")
    }

    /// Manual turns on or off for this session (GA `audio.input.turn_detection: null`).
    pub fn set_manual(&self, manual: bool) {
        *self.manual_override.lock() = Some(manual);
    }

    /// Manual mode: hold a final transcript until the client commits.
    pub fn append_input(&self, text: &str) {
        let t = text.trim();
        if t.is_empty() {
            return;
        }
        let mut buf = self.manual_input.lock();
        if !buf.is_empty() {
            buf.push(' ');
        }
        buf.push_str(t);
    }

    /// Manual mode: drop the held transcript (GA `input_audio_buffer.clear`).
    pub fn clear_input(&self) {
        self.manual_input.lock().clear();
    }

    /// Manual mode: run a turn over the held transcript. `false` when there was nothing to commit.
    pub async fn commit_input(self: &Arc<Self>) -> bool {
        let input = std::mem::take(&mut *self.manual_input.lock());
        if input.trim().is_empty() {
            return false;
        }
        self.start_turn(input).await;
        true
    }

    /// The caller stopped talking (or typed): run an agent turn for `input`.
    ///
    /// A turn still running is ended first — as a barge-in, so its reply is truncated to what was
    /// heard. Turns never overlap on one conversation (FR-TURN-2).
    pub async fn start_turn(self: &Arc<Self>, input: String) {
        if self.is_closed() {
            return;
        }
        self.poke_idle();
        if self.has_active_turn() || self.speech.is_audible() {
            self.barge_in().await;
        }
        let missing = self.missing_variables();
        let id = self.next_turn.fetch_add(1, Ordering::AcqRel);
        if !missing.is_empty() {
            self.signal(AgentSignal::Error {
                code: "missing_variables",
                message: format!(
                    "This agent needs the session's variables before it can answer: {}. Send them \
                     in session.prompt.variables (or config.agent.variables).",
                    missing.join(", ")
                ),
            });
            return;
        }
        self.started.store(true, Ordering::Release);
        let token = CancellationToken::new();
        let shared = Arc::new(Mutex::new(TurnShared::new(TurnKind::Agent)));
        *self.turn.lock() = Some(ActiveTurn {
            id,
            token: token.clone(),
            shared: Arc::clone(&shared),
        });
        let engine = Arc::clone(self);
        let span = tracing::Span::current();
        let turn_shared = Arc::clone(&shared);
        let run = tokio::spawn(tracing::Instrument::instrument(
            async move { engine.run_turn(id, token, turn_shared, input).await },
            span,
        ));
        // A bug that panics the turn must not wedge the session (no `response.done`, the turn
        // slot held forever): end it as failed, the way any other failure ends.
        let engine = Arc::clone(self);
        tokio::spawn(async move {
            if let Err(e) = run.await
                && e.is_panic()
            {
                engine.recover_panicked_turn(id, &shared).await;
            }
        });
    }

    /// The turn task panicked: stop its speech, tell the caller, cancel the run at budprompt, and
    /// free the turn slot. A turn already superseded (barged in) only gets its `ResponseDone`.
    async fn recover_panicked_turn(self: &Arc<Self>, id: u64, shared: &Arc<Mutex<TurnShared>>) {
        error!(
            turn = id,
            "agent turn task panicked; ending the turn as failed"
        );
        let still_live = self.turn.lock().as_ref().is_some_and(|t| t.id == id);
        let rid = shared.lock().response_id.clone();
        if let Some(rid) = rid {
            let backend = Arc::clone(&self.backend);
            let name = self.cfg.lock().prompt_name.clone();
            let bearer = self.credential.current();
            tokio::spawn(async move { backend.cancel(&rid, &name, &bearer).await });
        }
        if still_live && !self.is_closed() {
            let text_only = self.cfg.lock().text_only;
            if !text_only {
                self.speech.clear().await;
            }
            let failure = TurnFailure::Internal("The agent's turn failed unexpectedly.".into());
            let epoch = self.speech.clear_epoch();
            self.on_failure(id, shared, epoch, &failure, text_only)
                .await;
            if !text_only {
                let token = CancellationToken::new();
                self.drain(&token, shared).await;
            }
        }
        self.finish(id, shared, TurnStatus::Failed, None);
    }

    /// The caller talked over the agent (or the client asked to stop it): stop speaking, end the
    /// live turn, and remember what was heard for the next turn's history.
    pub async fn barge_in(self: &Arc<Self>) {
        let active = self.turn.lock().take();
        let target = match active {
            Some(a) => {
                a.token.cancel();
                Some((a.id, a.shared))
            }
            None if self.speech.is_audible() => self.last.lock().clone(),
            None => None,
        };
        // Measure BEFORE the clear: clearing resets the playout estimate.
        if let Some((id, shared)) = target
            && !self.cfg.lock().text_only
        {
            self.truncate_turn(id, &shared, None);
        }
        self.speech.clear().await;
    }

    /// GA `response.cancel`: stop the live response without the caller having spoken.
    pub async fn cancel_response(self: &Arc<Self>) {
        self.barge_in().await;
    }

    /// GA `conversation.item.truncate`: the client says exactly how much it played.
    pub fn truncate_at(&self, audio_end_ms: u64) {
        let target = {
            let active = self.turn.lock();
            match active.as_ref() {
                Some(a) => Some((a.id, Arc::clone(&a.shared))),
                None => self.last.lock().clone(),
            }
        };
        if let Some((id, shared)) = target {
            self.truncate_turn(id, &shared, Some(audio_end_ms));
        }
    }

    fn truncate_turn(&self, id: u64, shared: &Arc<Mutex<TurnShared>>, exact_ms: Option<u64>) {
        let mut s = shared.lock();
        if s.kind != TurnKind::Agent || (s.truncated_exact && exact_ms.is_none()) {
            return;
        }
        let played = exact_ms.unwrap_or_else(|| {
            s.ledger.played_ms(
                self.speech.audio_out_ms(),
                self.speech.playout_remaining_ms(),
            )
        });
        let prefix = s
            .ledger
            .spoken_prefix(played, self.rate.lock().chars_per_ms());
        // Generated text never sent to TTS was never heard either: compare against the generation.
        let generated = s.generated.trim().to_string();
        let heard_all = prefix.complete
            && prefix.text.trim() == s.ledger.reply_text().trim()
            && normalized(&generated) == normalized(&s.ledger.reply_text());
        if heard_all {
            return;
        }
        if exact_ms.is_some() {
            s.truncated_exact = true;
        }
        let response_id = s.response_id.clone();
        if let Some(rid) = response_id.as_ref() {
            *self.pending_truncate.lock() = Some(Truncate {
                response_id: rid.clone(),
                output_text: prefix.text.clone(),
            });
        }
        drop(s);
        debug!(
            turn = id,
            heard_chars = prefix.chars,
            "agent reply truncated to what was heard"
        );
        self.signal(AgentSignal::Truncated {
            turn: id,
            response_id,
            spoken: prefix.text,
            audio_end_ms: played,
        });
    }

    /// Speak the agent's static greeting, if it has one. Never part of the agent's history.
    pub async fn greet(self: &Arc<Self>) {
        let Some(text) = self.entry.greeting.static_text().map(str::to_string) else {
            return;
        };
        let id = self.next_turn.fetch_add(1, Ordering::AcqRel);
        let token = CancellationToken::new();
        let shared = Arc::new(Mutex::new(TurnShared::new(TurnKind::Greeting)));
        *self.turn.lock() = Some(ActiveTurn {
            id,
            token: token.clone(),
            shared: Arc::clone(&shared),
        });
        self.signal(AgentSignal::ResponseStarted {
            turn: id,
            kind: TurnKind::Greeting,
            input: None,
        });
        let text_only = self.cfg.lock().text_only;
        let epoch = self.speech.clear_epoch();
        let mut spoke = true;
        if !text_only {
            let speech = transform_for_speech(&text, &self.entry.text_transforms);
            let mark = self.speech.audio_out_ms();
            spoke = self
                .speech
                .speak(&speech, epoch, self.entry.greeting.interruptible)
                .await;
            if spoke {
                shared.lock().ledger.push(&text, &speech, false, mark);
            }
        }
        if spoke {
            shared.lock().transcript = text.clone();
            self.signal(AgentSignal::Transcript {
                turn: id,
                delta: text.clone(),
            });
        }
        if !text_only {
            self.drain(&token, &shared).await;
        }
        let status = if token.is_cancelled() {
            TurnStatus::Cancelled
        } else {
            TurnStatus::Completed
        };
        self.finish(id, &shared, status, None);
    }

    // ---------------------------------------------------------------------------------------
    // One turn
    // ---------------------------------------------------------------------------------------

    async fn run_turn(
        self: Arc<Self>,
        id: u64,
        token: CancellationToken,
        shared: Arc<Mutex<TurnShared>>,
        input: String,
    ) {
        let started = Instant::now();
        let (cfg, text_only) = {
            let c = self.cfg.lock();
            (c.clone(), c.text_only)
        };
        // D-19: an agent turn is the ROOT of its own trace; budgateway and budprompt continue it.
        let span = crate::voice_turn_span!(parent: None, capability = "agent_turn", transport = "websocket");
        let _end = EndSpanOnDrop(span.clone());
        {
            use crate::observability::voice_attrs::{realtime, turn};
            span.record(turn::SESSION_ID, cfg.session_id.as_str());
            span.record(turn::TURN_INDEX, id);
            span.record(turn::ENDPOINT_NAME, cfg.prompt_name.as_str());
            if let Some(p) = cfg.entry.project_id.as_deref() {
                span.record(turn::PROJECT_ID, p);
            }
            span.record(realtime::COMPONENT, "agent");
            if crate::observability::trace_redact::capture_content() {
                span.record(
                    turn::TRANSCRIPT,
                    crate::observability::trace_redact::sanitize_body(&input).as_str(),
                );
            }
        }
        self.signal(AgentSignal::ResponseStarted {
            turn: id,
            kind: TurnKind::Agent,
            input: Some(input.clone()),
        });

        let truncate = self.pending_truncate.lock().take();
        let request = AgentTurnRequest {
            prompt_name: cfg.prompt_name.clone(),
            version: cfg.version,
            variables: cfg.variables.clone(),
            input,
            conversation_id: cfg.conversation_id.clone(),
            truncate: truncate.clone(),
            session_id: cfg.session_id.clone(),
            turn_index: id,
        };
        let bearer = self.credential.current();
        let traceparent = traceparent_of(&span);
        let epoch = self.speech.clear_epoch();

        let mut filler = FillerCursor::default();
        let mut ticker = tokio::time::interval(TICK);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        // A request that never reached the agent is tried again (bounded, with backoff) before the
        // caller hears the degradation message: no run exists yet, so nothing can be duplicated.
        let open = async {
            let mut attempt = 0;
            loop {
                let r = tokio::select! {
                    biased;
                    _ = token.cancelled() => None,
                    r = tracing::Instrument::instrument(self.backend.open(&request, &bearer, traceparent.as_deref()), span.clone()) => Some(r),
                };
                match r {
                    Some(Err(f)) if f.retryable() && attempt < OPEN_RETRY_BACKOFF.len() => {
                        let wait = OPEN_RETRY_BACKOFF[attempt];
                        attempt += 1;
                        debug!(turn = id, attempt, error = %f.message(), "agent request did not reach the agent; retrying");
                        tokio::select! {
                            biased;
                            _ = token.cancelled() => break None,
                            _ = tokio::time::sleep(wait) => {}
                        }
                    }
                    other => break other,
                }
            }
        };
        tokio::pin!(open);
        // The wait for budprompt to answer at all is the caller's wait too (it lists an agent's
        // tools before it streams): the fillers run on the same schedule while it lasts.
        let opened = loop {
            tokio::select! {
                biased;
                r = &mut open => break r,
                _ = ticker.tick(), if !text_only => {
                    if list_filler_due(&self.entry.fillers, filler.last_at, started, false, false) {
                        filler.last_at = Some(Instant::now());
                        if let Some(phrase) = filler.take(&self.entry.fillers.messages) {
                            self.speak_aside(id, &shared, &phrase, epoch).await;
                        }
                    }
                }
            }
        };
        let mut stream = match opened {
            None => {
                self.restore_truncate(truncate);
                self.finish(id, &shared, TurnStatus::Cancelled, None);
                return;
            }
            Some(Err(failure)) => {
                // budprompt never saw this request: the truncate it carried still applies.
                self.restore_truncate(truncate);
                record_failure(&span, &failure);
                self.on_failure(id, &shared, epoch, &failure, text_only)
                    .await;
                self.finish(id, &shared, TurnStatus::Failed, None);
                return;
            }
            Some(Ok(s)) => s,
        };

        let speak_field = self
            .entry
            .structured_output
            .speak_field
            .clone()
            .filter(|f| !f.is_empty());
        let mut pump = Pump {
            started,
            chunker: SpeechChunker::default(),
            fence: CodeFenceFilter::new(&self.entry.text_transforms.code_blocks),
            think: ThinkStripper::default(),
            extractor: speak_field.map(SpeakFieldExtractor::new),
            spoke_reply: false,
            first_audio_at: None,
            first_token_at: None,
        };
        let mut tools: HashMap<String, (String, Instant, bool)> = HashMap::new();
        let mut status = TurnStatus::Completed;
        let mut usage: Option<Value> = None;
        let mut failure: Option<TurnFailure> = None;

        loop {
            tokio::select! {
                biased;
                _ = token.cancelled() => {
                    status = TurnStatus::Cancelled;
                    break;
                }
                ev = stream.next() => match ev {
                    None => break,
                    Some(Ok(AgentEvent::Created { response_id, .. })) => {
                        // `response.created` and `response.in_progress` both carry the run: announce it once.
                        let first = {
                            let mut s = shared.lock();
                            let first = s.response_id.as_deref() != Some(response_id.as_str());
                            s.response_id = Some(response_id.clone());
                            first
                        };
                        if first {
                            span.record(crate::observability::voice_attrs::realtime::RESPONSE_ID, response_id.as_str());
                            self.signal(AgentSignal::ResponseCreated { turn: id, response_id });
                        }
                    }
                    Some(Ok(AgentEvent::TextDelta { delta, .. })) => {
                        if pump.first_token_at.is_none() {
                            pump.first_token_at = Some(started.elapsed());
                        }
                        if !self.on_delta(id, &shared, &mut pump, &delta, epoch, text_only, &token).await {
                            status = TurnStatus::Cancelled;
                            break;
                        }
                    }
                    Some(Ok(AgentEvent::ToolStarted { item_id, name, .. })) => {
                        // The text before a tool call is a finished thought ("Let me check."): say it
                        // now. Held, it would be heard after the tool's fillers, run into the answer.
                        if !text_only
                            && let Some(held) = pump.chunker.flush()
                            && !self.speak_chunk(id, &shared, &mut pump, &held, epoch, &token).await
                        {
                            status = TurnStatus::Cancelled;
                            break;
                        }
                        self.signal(AgentSignal::Tool { turn: id, item_id: item_id.clone(), name: name.clone(), status: "in_progress" });
                        tools.insert(item_id, (name, Instant::now(), false));
                    }
                    Some(Ok(AgentEvent::ToolFinished { item_id, name, ok })) => {
                        tools.remove(&item_id);
                        self.signal(AgentSignal::Tool { turn: id, item_id, name, status: if ok { "completed" } else { "failed" } });
                    }
                    Some(Ok(AgentEvent::Completed { usage: u, .. })) => {
                        usage = u;
                        break;
                    }
                    Some(Ok(AgentEvent::Failed { code, message })) => {
                        failure = Some(TurnFailure::Upstream(format!("{code}: {message}")));
                        break;
                    }
                    Some(Ok(AgentEvent::Incomplete { status: s, reason, .. })) => {
                        status = if s == "cancelled" { TurnStatus::Cancelled } else { TurnStatus::Incomplete };
                        debug!(turn = id, status = %s, reason = ?reason, "agent run ended incomplete");
                        break;
                    }
                    Some(Err(f)) => {
                        failure = Some(f);
                        break;
                    }
                },
                _ = ticker.tick() => {
                    if text_only {
                        continue;
                    }
                    // Fillers (D-16): a running tool's own phrase, or the list in order while the wait lasts.
                    let fillers = &self.entry.fillers;
                    if fillers.tool_call_after_ms > 0 {
                        let due: Option<String> = tools
                            .values_mut()
                            .find(|(_, since, done)| !*done && since.elapsed() >= Duration::from_millis(fillers.tool_call_after_ms))
                            .map(|(name, _, done)| { *done = true; name.clone() });
                        if let Some(name) = due {
                            let own = fillers
                                .use_tool_status_messages
                                .then(|| self.entry.tool_phrase(&name).map(str::to_string))
                                .flatten();
                            // Taken even when the tool has its own phrase: that phrase fills this
                            // slot, so the list's next phrase still lands at its point in the wait.
                            let listed = filler.take(&fillers.messages);
                            if let Some(phrase) = own.or(listed) {
                                // Timed from when it was due, like the list's phrases: speaking
                                // waits on synthesis, which would stretch the next gap.
                                filler.last_at = Some(Instant::now());
                                self.speak_aside(id, &shared, &phrase, epoch).await;
                            }
                        }
                    }
                    if list_filler_due(fillers, filler.last_at, started, pump.spoke_reply, !tools.is_empty()) {
                        filler.last_at = Some(Instant::now());
                        if let Some(phrase) = filler.take(&fillers.messages) {
                            self.speak_aside(id, &shared, &phrase, epoch).await;
                        }
                    }
                }
            }
        }

        if status == TurnStatus::Cancelled {
            // The barge-in already truncated and cleared. Stop the run at budprompt too: closing
            // the stream records it as abandoned; the cancel records it as cancelled (best-effort).
            drop(stream);
            let rid = shared.lock().response_id.clone();
            if let Some(rid) = rid {
                let backend = Arc::clone(&self.backend);
                let name = cfg.prompt_name.clone();
                let bearer = self.credential.current();
                tokio::spawn(async move { backend.cancel(&rid, &name, &bearer).await });
            }
            span.record(crate::observability::voice_attrs::turn::BARGE_IN, true);
            span.record(
                crate::observability::voice_attrs::realtime::RESPONSE_STATUS,
                "cancelled",
            );
            self.finish(id, &shared, TurnStatus::Cancelled, usage);
            return;
        }

        // The tail: whatever the chunker still holds.
        let still_live = self
            .flush_pump(id, &shared, &mut pump, epoch, text_only, &token)
            .await;
        if !still_live {
            self.finish(id, &shared, TurnStatus::Cancelled, usage);
            return;
        }
        if let Some(x) = pump.extractor.as_ref()
            && let Some(output) = x.output()
        {
            let rid = shared.lock().response_id.clone();
            self.signal(AgentSignal::Output {
                turn: id,
                response_id: rid,
                output,
            });
        }
        if let Some(f) = failure.as_ref() {
            status = TurnStatus::Failed;
            record_failure(&span, f);
            self.on_failure(id, &shared, epoch, f, text_only).await;
        }
        if status == TurnStatus::Completed && !pump.spoke_reply && !text_only {
            debug!(turn = id, "agent turn produced no speakable text");
        }
        if !text_only {
            self.drain(&token, &shared).await;
            if token.is_cancelled() {
                // Barged in while the tail was playing: the barge-in truncated already.
                self.finish(id, &shared, TurnStatus::Cancelled, usage);
                return;
            }
            self.calibrate(&shared);
        }
        {
            use crate::observability::voice_attrs::{realtime, turn};
            if let Some(ttfa) = pump.first_audio_at {
                span.record(turn::RESPONSE_LATENCY_MS, ttfa.as_millis() as u64);
            }
            if let Some(ttft) = pump.first_token_at {
                span.record(
                    crate::observability::voice_attrs::leg::LLM_DURATION_MS,
                    ttft.as_millis() as u64,
                );
            }
            span.record(turn::CHARACTERS, shared.lock().ledger.speech_chars() as u64);
            span.record(realtime::RESPONSE_STATUS, status.as_str());
        }
        self.finish(id, &shared, status, usage);
    }

    /// One delta of generated text. Returns `false` when the turn has been cancelled.
    #[allow(clippy::too_many_arguments)]
    async fn on_delta(
        &self,
        id: u64,
        shared: &Arc<Mutex<TurnShared>>,
        pump: &mut Pump,
        delta: &str,
        epoch: usize,
        text_only: bool,
        token: &CancellationToken,
    ) -> bool {
        shared.lock().generated.push_str(delta);
        let text = match pump.extractor.as_mut() {
            Some(x) => x.push(delta),
            None => delta.to_string(),
        };
        if text.is_empty() {
            return true;
        }
        let text = pump.think.push(&text);
        if text_only {
            if !text.is_empty() {
                shared.lock().transcript.push_str(&text);
                pump.spoke_reply = true;
                self.signal(AgentSignal::Transcript {
                    turn: id,
                    delta: text,
                });
            }
            return true;
        }
        let text = pump.fence.push(&text);
        for chunk in pump.chunker.push(&text) {
            if !self
                .speak_chunk(id, shared, pump, &chunk, epoch, token)
                .await
            {
                return false;
            }
        }
        true
    }

    async fn flush_pump(
        &self,
        id: u64,
        shared: &Arc<Mutex<TurnShared>>,
        pump: &mut Pump,
        epoch: usize,
        text_only: bool,
        token: &CancellationToken,
    ) -> bool {
        let tail = pump.think.flush();
        if text_only {
            if !tail.is_empty() {
                shared.lock().transcript.push_str(&tail);
                self.signal(AgentSignal::Transcript {
                    turn: id,
                    delta: tail,
                });
            }
            return !token.is_cancelled();
        }
        let mut rest = pump.fence.push(&tail);
        rest.push_str(&pump.fence.flush());
        let mut chunks = pump.chunker.push(&rest);
        chunks.extend(pump.chunker.flush());
        for chunk in chunks {
            if !self
                .speak_chunk(id, shared, pump, &chunk, epoch, token)
                .await
            {
                return false;
            }
        }
        !token.is_cancelled()
    }

    /// Speak one chunk of the reply. `false` when a barge-in got there first.
    async fn speak_chunk(
        &self,
        id: u64,
        shared: &Arc<Mutex<TurnShared>>,
        pump: &mut Pump,
        chunk: &str,
        epoch: usize,
        token: &CancellationToken,
    ) -> bool {
        if token.is_cancelled() {
            return false;
        }
        let speech = transform_for_speech(chunk, &self.entry.text_transforms);
        if speech.is_empty() {
            return true;
        }
        // The playback mark is taken BEFORE the speak: audio can start flowing the moment the chunk is
        // queued, and a mark taken after it would count that audio as already played.
        let mark = self.speech.audio_out_ms();
        if !self.speech.speak(&speech, epoch, true).await {
            return false;
        }
        if pump.first_audio_at.is_none() {
            pump.first_audio_at = Some(pump.started.elapsed());
        }
        pump.spoke_reply = true;
        let delta = {
            let mut s = shared.lock();
            s.ledger.push(chunk, &speech, true, mark);
            let delta = format!("{}{chunk}", chunk_separator(&s.transcript, chunk));
            s.transcript.push_str(&delta);
            delta
        };
        self.signal(AgentSignal::Transcript { turn: id, delta });
        true
    }

    /// A filler or a spoken notice: heard, shown, never part of the agent's history.
    async fn speak_aside(
        &self,
        id: u64,
        shared: &Arc<Mutex<TurnShared>>,
        phrase: &str,
        epoch: usize,
    ) {
        let speech = transform_for_speech(phrase, &self.entry.text_transforms);
        let mark = self.speech.audio_out_ms();
        if speech.is_empty() || !self.speech.speak(&speech, epoch, true).await {
            return;
        }
        let delta = {
            let mut s = shared.lock();
            s.ledger.push(phrase, &speech, false, mark);
            let delta = format!("{}{phrase}", chunk_separator(&s.transcript, phrase));
            s.transcript.push_str(&delta);
            delta
        };
        self.signal(AgentSignal::Transcript { turn: id, delta });
    }

    /// The §5.8 table: what the caller hears and is told for a failed turn.
    async fn on_failure(
        &self,
        id: u64,
        shared: &Arc<Mutex<TurnShared>>,
        epoch: usize,
        failure: &TurnFailure,
        text_only: bool,
    ) {
        warn!(turn = id, code = failure.code(), error = %failure.message(), "agent turn failed");
        self.signal(AgentSignal::Error {
            code: failure.code(),
            message: failure.message().to_string(),
        });
        let spoken = match failure {
            TurnFailure::ApprovalRequired(_) => Some(self.entry.approval_message.clone()),
            f if f.speaks_degradation() => Some(self.entry.degradation_message.clone()),
            _ => None,
        };
        let Some(phrase) = spoken else { return };
        if text_only {
            shared.lock().transcript.push_str(&phrase);
            self.signal(AgentSignal::Transcript {
                turn: id,
                delta: phrase,
            });
        } else {
            self.speak_aside(id, shared, &phrase, epoch).await;
        }
    }

    fn restore_truncate(&self, truncate: Option<Truncate>) {
        if let Some(t) = truncate {
            let mut pending = self.pending_truncate.lock();
            if pending.is_none() {
                *pending = Some(t);
            }
        }
    }

    /// Wait until this turn's audio has been synthesized and delivered (GA: every audio delta
    /// before `response.done`). Ends early on cancellation.
    async fn drain(&self, token: &CancellationToken, shared: &Arc<Mutex<TurnShared>>) {
        let (expected_ms, has_audio) = {
            let s = shared.lock();
            let chars = s.ledger.speech_chars();
            let rate = self.rate.lock().chars_per_ms().max(0.001);
            ((chars as f64 / rate) as u64, chars > 0)
        };
        if !has_audio {
            return;
        }
        let deadline =
            Instant::now() + MAX_DRAIN.min(Duration::from_millis(expected_ms * 2 + 3_000));
        let mut last = self.speech.audio_out_ms();
        let mut quiet_since = Instant::now();
        loop {
            if token.is_cancelled() || Instant::now() >= deadline {
                return;
            }
            tokio::select! {
                _ = token.cancelled() => return,
                _ = tokio::time::sleep(TICK) => {}
            }
            let now = self.speech.audio_out_ms();
            if now != last {
                last = now;
                quiet_since = Instant::now();
                continue;
            }
            let emitted = shared.lock().ledger.emitted_ms(now);
            if quiet_since.elapsed() >= DRAIN_QUIET && emitted * 2 >= expected_ms {
                return;
            }
        }
    }

    /// Fold a reply that played to the end into the session's speaking rate.
    fn calibrate(&self, shared: &Arc<Mutex<TurnShared>>) {
        let s = shared.lock();
        let emitted = s.ledger.emitted_ms(self.speech.audio_out_ms());
        self.rate.lock().observe(s.ledger.speech_chars(), emitted);
    }

    fn finish(
        self: &Arc<Self>,
        id: u64,
        shared: &Arc<Mutex<TurnShared>>,
        status: TurnStatus,
        usage: Option<Value>,
    ) {
        {
            let mut turn = self.turn.lock();
            if turn.as_ref().is_some_and(|t| t.id == id) {
                *turn = None;
            }
        }
        *self.last.lock() = Some((id, Arc::clone(shared)));
        let (kind, response_id, transcript) = {
            let s = shared.lock();
            (s.kind, s.response_id.clone(), s.transcript.clone())
        };
        self.signal(AgentSignal::ResponseDone {
            turn: id,
            kind,
            response_id,
            status,
            usage,
            transcript,
        });
        self.poke_idle();
    }

    // ---------------------------------------------------------------------------------------
    // Idle and teardown
    // ---------------------------------------------------------------------------------------

    /// Any activity re-arms the idle timer; silence for `idle.end_after_silence_ms` ends the session.
    pub fn poke_idle(self: &Arc<Self>) {
        let after = Duration::from_millis(self.entry.idle.end_after_silence_ms);
        if after.is_zero() || self.is_closed() {
            return;
        }
        let generation = self.idle_generation.fetch_add(1, Ordering::AcqRel) + 1;
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(after).await;
                let Some(engine) = weak.upgrade() else { return };
                if engine.idle_generation.load(Ordering::Acquire) != generation
                    || engine.is_closed()
                {
                    return;
                }
                if engine.has_active_turn() || engine.speech.is_audible() {
                    continue;
                }
                engine.signal(AgentSignal::Idle);
                return;
            }
        });
    }

    /// End the session: stop any turn (its run is cancelled at budprompt) and stop listening.
    pub async fn shutdown(self: &Arc<Self>) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Some(active) = self.turn.lock().take() {
            active.token.cancel();
        }
    }
}

/// Per-turn text pipeline state.
struct Pump {
    /// When the turn started (end of the caller's speech, as the turn detector decided it).
    started: Instant,
    chunker: SpeechChunker,
    fence: CodeFenceFilter,
    think: ThinkStripper,
    extractor: Option<SpeakFieldExtractor>,
    spoke_reply: bool,
    first_audio_at: Option<Duration>,
    first_token_at: Option<Duration>,
}

/// `tracing-opentelemetry` ends a span at its LAST EXIT, not when it closes, and the turn span is
/// entered only while the request opens. One more exit on the way out ends it with the turn, on
/// every path (panics included).
struct EndSpanOnDrop(tracing::Span);

impl Drop for EndSpanOnDrop {
    fn drop(&mut self) {
        self.0.in_scope(|| {});
    }
}

fn normalized(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn record_failure(span: &tracing::Span, f: &TurnFailure) {
    span.record(
        crate::observability::voice_attrs::turn::ERROR_TYPE,
        f.code(),
    );
    span.record(
        crate::observability::voice_attrs::realtime::RESPONSE_STATUS,
        "failed",
    );
}

/// The W3C `traceparent` of `span`, so budgateway → budprompt continue the agent turn's trace.
fn traceparent_of(span: &tracing::Span) -> Option<String> {
    use opentelemetry::trace::TraceContextExt;
    use tracing_opentelemetry::OpenTelemetrySpanExt;
    let ctx = span.context();
    let sc = ctx.span().span_context().clone();
    sc.is_valid().then(|| {
        format!(
            "00-{}-{}-{:02x}",
            sc.trace_id(),
            sc.span_id(),
            sc.trace_flags().to_u8()
        )
    })
}

#[cfg(test)]
#[path = "engine_tests.rs"]
mod tests;
