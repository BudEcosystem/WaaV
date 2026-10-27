//! Attaches interaction mode to a `/ws` session: `prepare` decides at config time whether the
//! session runs it, `start` spawns the analyser, the speech worker and the actor.
//!
//! A session with no `interaction` value, its own or the server's default, never comes here.
//! One that asked and cannot have the mode gets a `config_warning` and runs exactly as it would
//! have without the value.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{Value, json};
use tokio::sync::RwLock;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tracing::{debug, info, warn};

use crate::auth::SessionCredential;
use crate::core::agent::text::transform_for_speech;
use crate::core::agent::{AgentBrain, AgentSessionConfig};
use crate::core::audio::PumpControl;
use crate::core::conversation::TurnAttribution;
use crate::core::interaction::actor::{self, ActorDeps, Background, InteractionHandle, Msg};
use crate::core::interaction::background::TextBackend;
use crate::core::interaction::bud_agent::AgentBackground;
use crate::core::interaction::config::{
    self, BackendKind, InteractionConfig, ParseCtx, ServerInteraction, Tri,
};
use crate::core::interaction::persona::Persona;
use crate::core::interaction::types::PumpEvent;
use crate::core::interaction::{analyser, front_model, speech, tasks, tenant};
use crate::core::stt::STTResult;
use crate::core::voice_manager::VoiceManager;
use crate::livekit::CallerTap;
use crate::state::{AppState, ResolvedVoiceAgent};

use super::config::{AgentWebSocketConfig, ConversationWebSocketConfig};
use super::config_handler::{bud_llm_leg, send_config_warning, turn_attribution};
use super::messages::{MessageClass, MessageRoute, OutgoingMessage, send_with_policy};
use super::state::ConnectionState;

/// How far ahead of playout the pump delivers the agent's audio: what a client may still hold
/// when a cut reaches it. A room frames and paces the audio itself, so it gets less.
const WS_LEAD_MS: u64 = 150;
const ROOM_LEAD_MS: u64 = 40;
/// Caller audio frames that may wait for the analyser; a frame that finds the queue full is
/// dropped (the speech-to-text still gets it).
const TAP_FRAMES: usize = 64;

/// The parts of a `config` message that decide whether and how interaction mode runs.
#[derive(Debug, Clone, Copy)]
pub(super) struct Surface {
    pub audio: bool,
    /// The caller is in a LiveKit room (a SIP call is one too): the mode runs, on the room's
    /// audio.
    pub livekit: bool,
    /// Why the session's DAG cannot be the background (`dag_config`, `dag_audio_output`,
    /// `dag_no_text_output`); `None` without a DAG, or with one that can.
    pub dag: Option<&'static str>,
    /// A voice-agent session that cannot run it (see [`AgentSession::unfit`]).
    pub agent: bool,
}

/// A voice-agent session: the agent it resolved to and the session's own `agent` block.
#[derive(Clone, Copy)]
pub(super) struct AgentSession<'a> {
    pub resolved: &'a ResolvedVoiceAgent,
    pub config: &'a AgentWebSocketConfig,
}

impl AgentSession<'_> {
    /// Whether this agent session cannot run the mode: its replies are text only, or its
    /// turns start when the client says so. The mode speaks, and ends the turns itself.
    pub(super) fn unfit(&self) -> bool {
        self.config.text_only == Some(true) || self.resolved.entry.turn_detection.kind == "manual"
    }
}

/// The `interaction` block of a voice-agent session and whether the session's own value was
/// refused: the agent entry's block (the agent author's), else the server's `agent_default`.
/// A session's own value counts only where the agent lets a session override `interaction`.
/// The block comes with `asked` and `operator` as [`prepare`] takes them.
pub(super) fn agent_request(
    entry: &bud_auth::voice_agent::VoiceAgentEntry,
    sent: Option<Value>,
    server: &ServerInteraction,
) -> (Option<(Value, bool, bool)>, bool) {
    let asked = sent.is_some() || entry.interaction.is_some();
    let refused = sent.is_some() && !entry.allows_override("interaction");
    let request = match sent.filter(|_| !refused) {
        Some(own) => Some((own, asked, false)),
        None => {
            let default = || server.agent_default.clone().filter(|_| server.enabled);
            let block = entry.interaction.clone().or_else(default);
            block.map(|block| (block, asked, true))
        }
    };
    (request, refused)
}

/// Why a session of this shape runs without interaction mode, as a reason code and a clause
/// for the client; `None` when it can run it. `background`: the session has an LLM for its
/// tasks (a `conversation_config`, or a usable `interaction.tasks.llm`).
pub(super) fn surface_check(
    surface: Surface,
    background: bool,
) -> Option<(&'static str, &'static str)> {
    if surface.agent {
        let why = "it does not run on a voice agent that answers in text or takes manual turns";
        Some(("agent", why))
    } else if let Some(reason) = surface.dag {
        let why = match reason {
            "dag_audio_output" => "the DAG produces audio itself",
            "dag_no_text_output" => "the DAG has no text_output node to speak",
            _ => "it does not run with this DAG",
        };
        Some((reason, why))
    } else if !surface.audio {
        Some(("audio_disabled", "it needs audio"))
    } else if !background {
        let why = "it needs a conversation_config or interaction.tasks.llm";
        Some(("no_background", why))
    } else {
        None
    }
}

/// A session that will run interaction mode: what `prepare` built and `start` attaches.
pub(super) struct Prepared {
    vm: Arc<VoiceManager>,
    server: Arc<ServerInteraction>,
    cfg: InteractionConfig,
    ctl: Arc<PumpControl>,
    pump_tx: UnboundedSender<PumpEvent>,
    pump_rx: UnboundedReceiver<PumpEvent>,
    /// The caller's audio, from the VoiceManager's tap or the room's.
    audio: mpsc::Receiver<analyser::TapFrame>,
    /// The speech-to-text feed, for the echo canceller's audio.
    stt_tx: mpsc::Sender<Bytes>,
    /// A LiveKit session: how the room hands over its one caller.
    caller_tap: Option<CallerTap>,
    /// The keys the room's caller presses, for the actor.
    keys: Option<UnboundedReceiver<char>>,
    background: Background,
    system_prompt: Option<String>,
    credential: Option<SessionCredential>,
    attribution: Option<TurnAttribution>,
    /// The session has a `conversation_config` of its own or an agent (not only
    /// `interaction.tasks.llm`).
    plain_path: bool,
    /// A voice agent's static greeting, as it is spoken.
    greeting: Option<String>,
    /// A LiveKit session on a server with a SIP leg: a `human` control may refer a SIP caller
    /// (`controls.human.transfer_to`).
    sip: Option<Arc<AppState>>,
}

impl Prepared {
    /// The line the session opens with, once the client is listening.
    pub(super) fn greeting(&self) -> Option<String> {
        self.greeting.clone()
    }

    /// The session's DAG is its background: the graph runs once per task, not once per turn.
    pub(super) fn runs_graph(&self) -> bool {
        matches!(self.background, Background::Text(_))
    }

    /// For the room's client, before it connects.
    pub(super) fn caller_tap(&self) -> Option<CallerTap> {
        self.caller_tap.clone()
    }
}

/// Whether a policy pack speaks a session language (`stt_config.language`): the English one,
/// or one of the operator's `interaction.packs_dir`. A session that names no language is
/// taken to be in English.
pub(super) fn pack_language(language: &str) -> bool {
    crate::core::interaction::persona::pack_of(language).is_some()
}

/// What the session's tenant keeps: the persona it selected by id, when it is the tenant's
/// own, and what is known about the caller whose signed identity the session carries.
async fn caller_and_persona(
    cfg: &mut InteractionConfig,
    server: &ServerInteraction,
    asked: bool,
    state: &Arc<RwLock<ConnectionState>>,
    message_tx: &mpsc::Sender<MessageRoute>,
) {
    let tenant = crate::handlers::tasks::owner(&state.read().await.auth).to_owned();
    let front = &mut cfg.front;
    if server.tenant_personas && front.spec.is_none() && !Persona::packaged(&front.persona) {
        front.spec = tenant::persona(&tenant, &front.persona).await;
    }
    let Some(token) = front.identity_token.take() else {
        return;
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let path = json!({ "path": "interaction.front.caller.identity_token" });
    let (code, message) = match server.caller_memory_key.as_deref() {
        Some(key) => match tenant::verify(&token, key, &tenant, now) {
            Some(sub) => {
                let (facts, wish) = tokio::join!(
                    tenant::facts(key, &tenant, &sub),
                    tenant::language_wish(key, &tenant, &sub)
                );
                front.memory = facts;
                front.language_wish = wish;
                front.caller = Some(sub);
                return;
            }
            None => (
                "caller_identity_rejected",
                "The caller identity token is not valid for this tenant now; the call runs without caller memory.",
            ),
        },
        None => (
            "caller_memory_unavailable",
            "This server keeps no caller memory; the identity token is ignored.",
        ),
    };
    if asked {
        send_config_warning(message_tx, code, message.to_owned(), Some(path)).await;
    }
}

/// A speech-to-text model its provider serves for files only: its streaming endpoint refuses
/// it, so no live turn can be heard (Deepgram's Whisper models).
pub(super) fn file_only_stt(provider: &str, model: &str) -> bool {
    provider.trim().eq_ignore_ascii_case("deepgram")
        && crate::core::stt::deepgram::is_file_only(model)
}

/// The mode is refused for a file-only speech-to-text model (see [`file_only_stt`]).
pub(super) async fn refuse_file_only(asked: bool, message_tx: &mpsc::Sender<MessageRoute>) {
    let why = "its speech-to-text model transcribes files only, not a live stream";
    unavailable(asked, message_tx, "stt_file_only", why).await;
}

/// Tell a session that asked for the mode why it runs without it.
async fn unavailable(
    asked: bool,
    message_tx: &mpsc::Sender<MessageRoute>,
    reason: &str,
    why: &str,
) {
    if !asked {
        debug!(
            reason,
            "the server's default interaction block does not apply"
        );
        return;
    }
    warn!(reason, "interaction mode asked for and not available");
    let code = match reason {
        "no_background" => "interaction_needs_background",
        "front_unavailable" => "interaction_refused",
        "dag_audio_output" => "interaction_dag_audio_output",
        "dag_no_text_output" => "interaction_dag_no_text_output",
        _ => "interaction_surface_unavailable",
    };
    send_config_warning(
        message_tx,
        code,
        format!("Interaction mode is not available on this session: {why}. It runs without it."),
        Some(json!({ "surface": "ws", "reason": reason })),
    )
    .await;
}

/// Decide whether the session runs interaction mode. `request` is the session's `interaction`
/// value, the block of its alias (`operator`), or the server's default block. A session that
/// sent the value or named the alias (`asked`) gets every finding as a `config_warning`.
///
/// `None`: the session runs as it would without the block, and nothing was touched. `Some`:
/// the VoiceManager now plays through the metered pump, hands the caller's audio to the tap
/// and no longer runs its own smart-turn processor, so this must be called before the egress
/// callbacks are registered, and `start` must follow.
#[allow(clippy::too_many_arguments)]
pub(super) async fn prepare(
    request: &Value,
    asked: bool,
    operator: bool,
    surface: Surface,
    conversation: Option<&ConversationWebSocketConfig>,
    agent: Option<AgentSession<'_>>,
    graph: Option<Arc<dyn TextBackend>>,
    stream_id: &str,
    vm: Option<&Arc<VoiceManager>>,
    state: &Arc<RwLock<ConnectionState>>,
    message_tx: &mpsc::Sender<MessageRoute>,
    app_state: &Arc<AppState>,
) -> Option<Prepared> {
    let server = config::server();
    let bud_mode = app_state.bud_mode.is_some();
    // A session that only got the server's default block did not ask: it is told nothing.
    let (mut cfg, mut warnings) = config::parse(request, &server, ParseCtx { bud_mode, operator });
    if let Some(cfg) = cfg.as_mut() {
        // The session's own LLM (its conversation's model, else `tasks.llm`), as an
        // orchestrator of its several `tasks.backends`.
        let own_llm = conversation
            .map(|own| Value::String(own.model.clone()))
            .or_else(|| cfg.tasks.llm.clone());
        let own_background = agent.is_some() || graph.is_some();
        warnings.extend(backends_fit(cfg, own_background, own_llm));
        warnings.extend(orchestrator_fit(cfg, own_background));
    }
    let remote_runs = |cfg: &InteractionConfig| {
        let named = cfg.tasks.backend.as_deref();
        let kind = |name| {
            server
                .task_backends
                .iter()
                .find(|b| b.name == name)
                .map(|b| b.kind)
        };
        named.and_then(kind)
    };
    if let Some(cfg) = cfg
        .as_mut()
        .filter(|cfg| agent.is_some() || graph.is_some() || remote_runs(cfg).is_some())
    {
        // An agent runs its own tools and a DAG its own nodes, and a run of either cannot be
        // started and then dropped unseen: budprompt keeps every run in the conversation, and
        // so does a DAG's LLM node.
        // A Responses model alone calls the session's tools; a run on it is billed, so it
        // is not started before the turn is settled either.
        if remote_runs(cfg) != Some(BackendKind::OpenaiResponses)
            || agent.is_some()
            || graph.is_some()
        {
            cfg.tasks.tools.clear();
        }
        cfg.tasks.llm = None;
        if std::mem::replace(&mut cfg.tasks.speculative_start, Tri::Off) == Tri::On {
            let path = "interaction.tasks.speculative_start";
            let kind = if agent.is_some() {
                "voice-agent session"
            } else if graph.is_some() {
                "DAG session"
            } else {
                "session with a catalog backend"
            };
            warnings.push(config::ConfigWarning {
                code: "interaction_value_ignored",
                message: format!("{path} is off on a {kind}; the value is ignored."),
                detail: json!({ "path": path }),
            });
        }
        // An agent that may not be talked over keeps that.
        if let Some(agent) = agent {
            cfg.duplex.barge_in &= agent.resolved.entry.interruption.enabled;
        }
    }
    for warning in warnings.into_iter().filter(|_| asked) {
        let detail = Some(warning.detail);
        send_config_warning(message_tx, warning.code, warning.message, detail).await;
    }
    let mut cfg = cfg?;
    caller_and_persona(&mut cfg, &server, asked, state, message_tx).await;

    // The background's LLM: the session's conversation loop, else `tasks.llm` in its shape.
    // A voice agent is its own background, and so is a DAG.
    let mut background = conversation
        .filter(|_| graph.is_none())
        .cloned()
        .or_else(|| serde_json::from_value(cfg.tasks.llm.clone()?).ok());
    if let Some(orchestrator) = &cfg.tasks.orchestrator {
        let Some(llm) = orchestrator_llm(orchestrator, background) else {
            let why = "interaction.tasks.orchestrator names a model and the session has no LLM to \
                       run it on, or it is not an object of the tasks.llm shape";
            unavailable(asked, message_tx, "background_unavailable", why).await;
            return None;
        };
        background = Some(llm);
    }
    // An entry of the operator's catalog, named by the block. A voice agent and a DAG are
    // their own backgrounds.
    let named = cfg
        .tasks
        .backend
        .as_deref()
        .filter(|_| agent.is_none() && graph.is_none());
    let remote = named.and_then(|name| server.task_backends.iter().find(|b| b.name == name));
    let has_background =
        agent.is_some() || graph.is_some() || background.is_some() || remote.is_some();
    if let Some((reason, why)) = surface_check(surface, has_background) {
        unavailable(asked, message_tx, reason, why).await;
        return None;
    }
    let vm = vm?;
    let Some(language) =
        crate::core::interaction::persona::pack_of(&vm.get_config().stt_config.language)
    else {
        let why = "its language has no policy pack";
        unavailable(asked, message_tx, "language_unsupported", why).await;
        return None;
    };
    cfg.front.language = language.to_owned();
    // No fallback (chapter 9): without a ready front model the session runs as it would
    // without the mode. Another pack's front prompt reads the caller's language with the
    // pack's lexicon rows as examples, so a pack without one of them has no front either.
    if let Some(row) = crate::core::interaction::persona::front_gap(language) {
        let why = format!("its language pack has no {row} for the front model's examples");
        unavailable(asked, message_tx, "front_unavailable", &why).await;
        return None;
    }
    let front_ready = match &server.front_model {
        Some(front) => front_model::ready(front).await,
        None => false,
    };
    if !front_ready {
        let why = "its front model is not ready (interaction mode decides every turn with it)";
        unavailable(asked, message_tx, "front_unavailable", why).await;
        return None;
    }
    if vm.stt_is_request_response().await && !vm.stt_has_utterance_flush().await {
        // One transcript per connection: the first turn would be the only one heard.
        let why = "its speech-to-text provider cannot end one utterance and keep listening";
        unavailable(asked, message_tx, "stt_file_only", why).await;
        return None;
    }

    let credential = state.read().await.credential.clone();
    let mut llm_config = background.map(|background| background.to_conversation_config());
    let built = match (agent, graph, llm_config.as_mut()) {
        (Some(agent), ..) => agent_background(agent, stream_id, &cfg, credential),
        (None, Some(graph), _) => Ok(Background::Text(graph)),
        (None, None, llm_config) if remote.is_some() => {
            let spec = remote
                .cloned()
                .unwrap_or_else(|| unreachable!("checked by the guard"));
            let operator = llm_config.and_then(|llm| llm.system_prompt.clone());
            Ok(Background::remote(spec, &cfg, &server, operator))
        }
        (None, None, Some(llm_config)) => {
            // The relay speaks sentence by sentence while the answer is generated.
            llm_config.streaming = true;
            let leg = if bud_mode {
                bud_llm_leg(llm_config, credential)
            } else {
                Ok(())
            };
            leg.and_then(|()| Background::llm(llm_config))
        }
        (None, None, None) => return None,
    };
    let background = match built {
        Ok(background) => background,
        Err(error) => {
            unavailable(asked, message_tx, "background_unavailable", &error).await;
            return None;
        }
    };
    let llm_config = llm_config.unwrap_or_default();
    let llm_tier = matches!(background, Background::Llm(..));
    let attribution = if bud_mode && llm_tier {
        turn_attribution(&*state.read().await, &llm_config.model)
    } else {
        None
    };
    let greeting = agent.and_then(|agent| {
        let entry = &agent.resolved.entry;
        let text = entry.greeting.static_text()?;
        Some(transform_for_speech(text, &entry.text_transforms))
    });

    let (pump_tx, pump_rx) = mpsc::unbounded_channel();
    let lead_ms = if surface.livekit {
        ROOM_LEAD_MS
    } else {
        WS_LEAD_MS
    };
    let ctl = Arc::new(PumpControl::new(pump_tx.clone(), lead_ms));
    vm.set_metered_playback(Some(ctl.clone()));
    // A socket client says its caller is on a phone line, or sends 8 kHz audio.
    let input_rate = vm.get_config().stt_config.sample_rate;
    if !surface.livekit && cfg.duplex.echo.phone_line(false, input_rate) {
        ctl.set_phone_leg();
    }
    let (tap, audio) = mpsc::channel::<analyser::TapFrame>(TAP_FRAMES);
    // The speech-to-text's audio (as it came, or the echo canceller's), in order, off the
    // caller's loop and the analyser's: a recogniser that waits (one that reconnects) must not
    // hold the caller's frames back from the analyser (live SF4: no hearing line for 40 s).
    let (stt_tx, mut stt_rx) = mpsc::channel::<Bytes>(analyser::STT_FEED_FRAMES);
    let feed_vm = vm.clone();
    let feed = async move {
        while let Some(audio) = stt_rx.recv().await {
            if let Err(error) = feed_vm.send_stt_audio(audio).await {
                debug!(%error, "interaction audio not sent to the speech-to-text");
            }
        }
    };
    // It ends when the tap and the analyser are gone (here, before the session's tracker
    // exists: a session that does not start still has its frames sent).
    tokio::spawn(feed);
    // While the echo canceller runs, the analyser feeds the speech-to-text: a frame it takes
    // for that is not sent as it came.
    let from_tap = vm.stt_from_tap();
    let take = move |frame: Bytes, tap: &mpsc::Sender<analyser::TapFrame>| {
        let for_stt = from_tap.load(std::sync::atomic::Ordering::Acquire);
        tap.try_send((frame, for_stt)).is_ok() && for_stt
    };
    // The speech-to-text's own copy of a frame goes to the feed; with the feed full the
    // VoiceManager sends it itself, as without the mode.
    let raw = stt_tx.clone();
    let to_stt = move |frame: &Bytes| raw.try_send(frame.clone()).is_ok();
    let mut keys = None;
    let caller_tap = if surface.livekit {
        // The room hands over the frames of its one caller as they arrive, ahead of its
        // noise filter, says when that caller is on a phone line, and passes on its keys.
        let (line, stream_id) = (ctl.clone(), stream_id.to_owned());
        let (key_tx, key_rx) = mpsc::unbounded_channel();
        keys = Some(key_rx);
        // The room's audio reaches the VoiceManager after its noise filter: while the
        // analyser feeds the speech-to-text, that copy is not sent.
        let from_tap = vm.stt_from_tap();
        vm.set_audio_tap(Some(Arc::new(move |frame: &Bytes| {
            from_tap.load(std::sync::atomic::Ordering::Acquire) || to_stt(frame)
        })));
        let caller = CallerTap::new(
            move |frame: &[u8]| {
                take(Bytes::copy_from_slice(frame), &tap);
            },
            move |phone| {
                info!(%stream_id, phone, "interaction mode found the room's caller");
                if phone {
                    line.set_phone_leg();
                }
            },
        );
        Some(caller.on_keys(move |key| {
            let _ = key_tx.send(key);
        }))
    } else {
        vm.set_audio_tap(Some(Arc::new(move |frame: &Bytes| {
            take(frame.clone(), &tap) || to_stt(frame)
        })));
        None
    };
    // The analyser runs the voice-activity and turn models for this session.
    vm.disable_smart_turn_processor().await;

    Some(Prepared {
        vm: vm.clone(),
        server,
        cfg,
        ctl,
        pump_tx,
        pump_rx,
        audio,
        stt_tx,
        caller_tap,
        keys,
        background,
        system_prompt: llm_config.system_prompt,
        credential: llm_config.credential,
        attribution,
        // A DAG session has no plain conversation to step down to.
        plain_path: (llm_tier && conversation.is_some()) || agent.is_some(),
        greeting,
        sip: (surface.livekit && app_state.livekit_sip_handler.is_some())
            .then(|| app_state.clone()),
    })
}

/// The LLM an orchestrator runs on: a model name on the session's own LLM (`own`), or an LLM of
/// its own in the `tasks.llm` shape. Without a system prompt of its own it has the session's.
fn orchestrator_llm(
    orchestrator: &Value,
    own: Option<ConversationWebSocketConfig>,
) -> Option<ConversationWebSocketConfig> {
    let mut llm: ConversationWebSocketConfig = match orchestrator {
        Value::String(model) => {
            let mut llm = own.clone()?;
            llm.model.clone_from(model);
            llm
        }
        block => serde_json::from_value(block.clone()).ok()?,
    };
    if llm.system_prompt.is_none() {
        llm.system_prompt = own.and_then(|own| own.system_prompt);
    }
    // A planning model thinks before it calls agents: the reasoning tier's budget unless one
    // is set, not the voice cap (256 tokens ended gpt-oss-20b's rounds in its reasoning, live).
    llm.max_tokens
        .get_or_insert(crate::core::conversation::REASONING_DEFAULT_MAX_TOKENS);
    Some(llm)
}

/// `tasks.backends` (plan chapter 3, section 3.13): the catalog entries the session's tasks may
/// run on, chosen per task. With `tasks.orchestrator` they are its agents, and it chooses. One
/// entry (with `default_backend`, if that names another, two) and no orchestrator: it does
/// every task. Several and no orchestrator: the session's own LLM (`own_llm`) orchestrates them,
/// choosing by each one's description; without one the first listed does every task. A voice
/// agent or a DAG session ignores the list. The findings of what is ignored.
fn backends_fit(
    cfg: &mut InteractionConfig,
    own_background: bool,
    own_llm: Option<Value>,
) -> Vec<config::ConfigWarning> {
    let ignored = |why: &str| config::ConfigWarning {
        code: "interaction_value_ignored",
        message: format!("interaction.tasks.backends {why}; it is ignored."),
        detail: json!({ "path": "interaction.tasks.backends" }),
    };
    let mut listed = std::mem::take(&mut cfg.tasks.backends);
    if listed.is_empty() {
        return Vec::new();
    }
    if own_background {
        return vec![ignored("does not apply to a voice-agent or DAG session")];
    }
    let mut found = Vec::new();
    if cfg.tasks.orchestrator.is_none() {
        if let Some(named) = cfg.tasks.backend.clone()
            && !listed.contains(&named)
        {
            listed.insert(0, named);
        }
        if listed.len() == 1 {
            cfg.tasks.backend = listed.pop();
            return found;
        }
        match own_llm {
            Some(llm) => cfg.tasks.orchestrator = Some(llm),
            None => {
                found.push(ignored(
                    "needs tasks.orchestrator, or an LLM of the session's, to choose among several (the first does every task)",
                ));
                cfg.tasks.backend = listed.into_iter().next();
                return found;
            }
        }
    }
    for name in listed {
        if !cfg.tasks.agents.contains(&name) {
            cfg.tasks.agents.push(name);
        }
    }
    // The orchestrator chooses: a named backend gives way to it (`orchestrator_fit`).
    cfg.tasks.backend = None;
    found
}

/// `tasks.orchestrator` and `tasks.agents` on a session that can use them: an orchestrator
/// takes the LLM tier, so a voice agent or a DAG session ignores it and a named backend gives
/// way to it; agents without an orchestrator mean nothing. The findings of what is ignored.
fn orchestrator_fit(
    cfg: &mut InteractionConfig,
    own_background: bool,
) -> Vec<config::ConfigWarning> {
    let ignored = |path: &str, why: &str| config::ConfigWarning {
        code: "interaction_value_ignored",
        message: format!("{path} {why}; it is ignored."),
        detail: json!({ "path": path }),
    };
    let mut found = Vec::new();
    if own_background && cfg.tasks.orchestrator.take().is_some() {
        let why = "does not apply to a voice-agent or DAG session";
        found.push(ignored("interaction.tasks.orchestrator", why));
    }
    if cfg.tasks.orchestrator.is_none() && !std::mem::take(&mut cfg.tasks.agents).is_empty() {
        found.push(ignored(
            "interaction.tasks.agents",
            "needs tasks.orchestrator",
        ));
    }
    if cfg.tasks.orchestrator.is_some() && cfg.tasks.backend.take().is_some() {
        let why = "gives way to tasks.orchestrator";
        found.push(ignored("interaction.tasks.default_backend", why));
    }
    found
}

/// A voice agent as the session's background: the client the agent engine uses, the caller's
/// credential and the conversation every run of the session continues.
fn agent_background(
    agent: AgentSession<'_>,
    stream_id: &str,
    cfg: &InteractionConfig,
    credential: Option<SessionCredential>,
) -> Result<Background, String> {
    let base_url = super::bud_legs::llm_base_url().ok_or(
        "voice agents are not available on this gateway: WAAV_LLM_BASE_URL (the Bud gateway) \
         is not configured",
    )?;
    let credential = credential.ok_or("a voice agent needs your Bud API key or token")?;
    let session = AgentSessionConfig {
        session_id: stream_id.to_owned(),
        prompt_name: agent.resolved.prompt_name.clone(),
        version: agent.resolved.version,
        entry: agent.resolved.entry.clone(),
        conversation_id: super::agent::conversation_id_for(stream_id),
        variables: agent.config.variables.clone(),
        text_only: false,
    };
    let marker = cfg.tasks.answer_marker.clone();
    let brain = Arc::new(AgentBrain::new(base_url));
    let background = AgentBackground::new(brain, session, credential, marker);
    Ok(Background::Agent(background))
}

/// Start the session's actor, speech worker and analyser, and put the actor behind the
/// VoiceManager's callbacks. The callbacks the session already registered keep running: the
/// three single-slot ones are chained, and transcripts still go to the client first.
pub(super) async fn start(
    prepared: Prepared,
    stream_id: &str,
    state: &Arc<RwLock<ConnectionState>>,
    message_tx: &mpsc::Sender<MessageRoute>,
) -> Result<InteractionHandle, String> {
    // A LiveKit session's events are also published in its room.
    let room = state.read().await.livekit_operation_queue.clone();
    let Prepared {
        vm,
        server,
        cfg,
        ctl,
        pump_tx,
        pump_rx,
        audio,
        stt_tx,
        caller_tap: _,
        keys,
        background,
        system_prompt,
        credential,
        attribution,
        plain_path,
        greeting: _,
        sip,
    } = prepared;
    let (owner, tracker) = {
        let guard = state.read().await;
        let owner = crate::handlers::tasks::owner(&guard.auth).to_owned();
        (owner, guard.task_tracker.clone())
    };
    let sample_rate = vm.get_config().stt_config.sample_rate;
    let duplex = cfg.duplex.clone();

    // The actor is not on the session's task tracker: after `shutdown` it lives on, at most
    // until the registry's deadline, to report what a tool call already on the wire committed.
    // A caller who comes back does so with the session's resume key, else its `stream_id`.
    let back = tasks::back_id(stream_id, cfg.tasks.resume_key.as_deref());
    let (handle, _actor) = actor::spawn(ActorDeps {
        vm: vm.clone(),
        ctl: ctl.clone(),
        pump_rx,
        speech: speech::spawn(vm.clone(), ctl.clone(), pump_tx),
        out: message_tx.clone(),
        background,
        system_prompt,
        credential,
        attribution,
        server,
        cfg,
        stream_id: stream_id.to_owned(),
        refs_used: tasks::store().refs_used(&owner, stream_id).await,
        owner: owner.clone(),
        request_response: vm.stt_is_request_response().await,
        provider_turns: vm.stt_has_turn_events().await,
        stt_end_turn_after_ms: vm.stt_end_turn_after_ms().await,
        warm: vm.tts_has_cache().await,
        plain_path,
        room,
    });
    // The room's SIP caller may be referred to a person: the SIP leg's own transfer, which
    // reports a failure to the client as `sip_transfer_error`.
    if let Some(app) = sip {
        // The room's recording is paused while the phone caller types a sensitive value.
        let (paused_state, paused_app) = (state.clone(), app.clone());
        let parts = Arc::new(tokio::sync::Mutex::new((false, 1u32)));
        let recording = actor::RecordingPause(Arc::new(move |paused: bool| {
            let (state, app, parts) = (paused_state.clone(), paused_app.clone(), parts.clone());
            let pausing: actor::Referral = Box::pin(async move {
                pause_recording(paused, &state, &app, &parts).await;
            });
            pausing
        }));
        handle.send(Msg::RecordingPause(recording)).await;
        let (state, out) = (state.clone(), message_tx.clone());
        let transfer = actor::SipTransfer(Arc::new(move |to: String| {
            let (state, out, app) = (state.clone(), out.clone(), app.clone());
            let referral: actor::Referral = Box::pin(async move {
                super::command_handler::handle_sip_transfer(to, &state, &out, &app).await;
            });
            referral
        }));
        handle.send(Msg::SipTransfer(transfer)).await;
    }
    let stt = analyser::SttFeed {
        on: vm.stt_from_tap(),
        tx: stt_tx,
    };
    let (analyser, models) = analyser::spawn_loading(
        audio,
        sample_rate,
        ctl.clone(),
        handle.inbox(),
        duplex,
        Some(stt),
    );
    tracker.track("interaction-analyser", analyser);
    if let Some(mut keys) = keys {
        let inbox = handle.clone();
        let forward = async move {
            while let Some(key) = keys.recv().await {
                if !inbox.send(Msg::Dtmf(key)).await {
                    break;
                }
            }
        };
        tracker.track("interaction-keys", tokio::spawn(forward));
    }
    // A caller who is back after a lost connection gets what its tasks came to meanwhile.
    tracker.track(
        "interaction-reconnect",
        tokio::spawn(actor::follow(owner, back, handle.clone())),
    );

    // Transcripts go to the client as on a plain session, then to the actor. An interim may be
    // dropped when the inbox is full; a final waits for room, in order.
    // While a sensitive value is captured the client gets no transcript (the actor still
    // gets the empty results that end a segment).
    let (inbox, out) = (handle.clone(), message_tx.clone());
    let muted = vm.stt_mute_switch();
    vm.on_stt_result(move |result: STTResult| {
        let (inbox, out) = (inbox.clone(), out.clone());
        let muted = muted.load(std::sync::atomic::Ordering::Acquire);
        Box::pin(async move {
            if muted {
                if result.is_final || result.is_speech_final {
                    inbox.send(Msg::Stt(Box::new(result))).await;
                }
                return;
            }
            send_with_policy(
                &out,
                MessageRoute::Outgoing(OutgoingMessage::STTResult {
                    transcript: result.transcript.clone(),
                    is_final: result.is_final,
                    is_speech_final: result.is_speech_final,
                    confidence: result.confidence,
                    segment_transcript: result.segment_transcript.clone(),
                    translations: result.translations.clone(),
                    detected_language: result.detected_language.clone(),
                }),
                MessageClass::Transcript,
            )
            .await;
            // A provider's turn event is never dropped.
            if result.is_final || result.is_speech_final || result.turn_event.is_some() {
                inbox.send(Msg::Stt(Box::new(result))).await;
            } else {
                inbox.try_send(Msg::Stt(Box::new(result)));
            }
        })
    })
    .await
    .map_err(|e| format!("STT result callback: {e}"))?;

    let (previous, inbox) = (vm.stt_error_callback(), handle.clone());
    vm.on_stt_error(move |error| {
        let (previous, inbox) = (previous.clone(), inbox.clone());
        Box::pin(async move {
            let reason = error.to_string();
            if let Some(previous) = previous {
                previous(error).await;
            }
            inbox.send(Msg::SttError(reason)).await;
        })
    })
    .await
    .map_err(|e| format!("STT error callback: {e}"))?;

    let (previous, inbox) = (vm.tts_error_callback(), handle.clone());
    vm.on_tts_error(move |error| {
        let (previous, inbox) = (previous.clone(), inbox.clone());
        Box::pin(async move {
            let reason = error.to_string();
            if let Some(previous) = previous {
                previous(error).await;
            }
            inbox.send(Msg::TtsError(reason)).await;
        })
    })
    .await
    .map_err(|e| format!("TTS error callback: {e}"))?;

    // The provider is done with one synthesis: the pump closes the utterance it was playing.
    // The session's own completion step (flush the egress re-framer, `tts_playback_complete`)
    // runs when the utterance's last frame has gone out, which for metered audio is later than
    // the end of its synthesis; a warm-up line never reaches it.
    let previous = vm.tts_complete_callback();
    if let Some(previous) = previous.clone() {
        ctl.set_on_played(previous);
    }
    vm.on_tts_complete(move || {
        let (previous, ctl) = (previous.clone(), ctl.clone());
        Box::pin(async move {
            // The synthesis slot is free before anything waits for the client.
            if ctl.synth_done()
                && let Some(previous) = previous
            {
                previous().await;
            }
        })
    })
    .await
    .map_err(|e| format!("TTS completion callback: {e}"))?;

    state.write().await.interaction = Some(handle.clone());
    // `ready` waits for the analyser's models, at most `MODELS_WAIT` (the caller's audio is
    // kept until they are there either way).
    let loaded = analyser::models_loaded(models).await;
    info!(
        stream_id,
        models_loaded = loaded,
        "Interaction mode started"
    );
    Ok(handle)
}

/// Stops the room's recording (`paused`) or starts it again. A LiveKit egress cannot be paused:
/// the recording is stopped while a sensitive value is typed on the keypad, and a new one is
/// started after it, `<stream_id>-part<n>` in the same place. `parts`: whether this stopped
/// the recording, and how many parts were made.
async fn pause_recording(
    paused: bool,
    state: &Arc<RwLock<ConnectionState>>,
    app: &AppState,
    parts: &tokio::sync::Mutex<(bool, u32)>,
) {
    let Some(rooms) = app.livekit_room_handler.as_ref() else {
        return;
    };
    let mut parts = parts.lock().await;
    if paused {
        let Some(egress) = state.write().await.recording_egress_id.take() else {
            return;
        };
        match rooms.stop_room_recording(&egress).await {
            Ok(()) => info!(%egress, "interaction call recording stopped for the keypad"),
            Err(error) => {
                warn!(%egress, %error, "interaction call recording not stopped for the keypad")
            }
        }
        parts.0 = true;
        return;
    }
    if !std::mem::take(&mut parts.0) {
        return;
    }
    parts.1 += 1;
    let (room, auth, stream) = {
        let state = state.read().await;
        let room = state.livekit_room_name.clone();
        (room, state.auth.id.clone(), state.stream_id.clone())
    };
    let (Some(room), Some(stream)) = (room, stream) else {
        return;
    };
    let part = format!("{stream}-part{}", parts.1);
    match rooms
        .setup_room_recording(&room, auth.as_deref(), &part)
        .await
    {
        Ok(egress) => {
            info!(%egress, %part, "interaction call recording started again after the keypad");
            state.write().await.recording_egress_id = Some(egress);
        }
        Err(error) => {
            warn!(%error, "interaction call recording not started again after the keypad")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAIN: Surface = Surface {
        audio: true,
        livekit: false,
        dag: None,
        agent: false,
    };

    #[test]
    fn a_sessions_backends_are_one_backend_or_an_orchestrators_agents() {
        let fit = |backends: &[&str],
                   named: Option<&str>,
                   orchestrator: Option<Value>,
                   own: Option<Value>| {
            let mut cfg = InteractionConfig::default();
            cfg.tasks.backends = backends.iter().map(|name| (*name).to_owned()).collect();
            cfg.tasks.backend = named.map(str::to_owned);
            cfg.tasks.orchestrator = orchestrator;
            let found = backends_fit(&mut cfg, false, own);
            (cfg.tasks, found.len())
        };
        // One: it does every task.
        let (tasks, found) = fit(&["crm"], None, None, Some(json!("mock")));
        assert_eq!(
            (tasks.backend.as_deref(), tasks.orchestrator, found),
            (Some("crm"), None, 0)
        );
        // Several: the session's own LLM chooses among them as their orchestrator.
        let (tasks, found) = fit(&["crm", "trips"], None, None, Some(json!("mock")));
        assert_eq!(tasks.orchestrator, Some(json!("mock")));
        assert_eq!(
            (tasks.agents, tasks.backend, found),
            (vec!["crm".to_owned(), "trips".to_owned()], None, 0)
        );
        // A named default backend counts among them.
        let (tasks, _) = fit(&["trips"], Some("crm"), None, Some(json!("mock")));
        assert_eq!(tasks.agents, ["crm", "trips"]);
        // A named orchestrator chooses; they join its agents.
        let (tasks, _) = fit(&["crm"], None, Some(json!("big")), None);
        assert_eq!(
            (tasks.orchestrator, tasks.agents),
            (Some(json!("big")), vec!["crm".to_owned()])
        );
        // Several and no LLM to choose: the first does every task, and the client is told.
        let (tasks, found) = fit(&["crm", "trips"], None, None, None);
        assert_eq!(
            (tasks.backend.as_deref(), tasks.orchestrator, found),
            (Some("crm"), None, 1)
        );
        // A voice agent or a DAG session ignores the list.
        let mut cfg = InteractionConfig::default();
        cfg.tasks.backends = vec!["crm".to_owned()];
        assert_eq!(backends_fit(&mut cfg, true, None).len(), 1);
        assert!(cfg.tasks.backend.is_none() && cfg.tasks.agents.is_empty());
    }

    fn reason(surface: Surface, background: bool) -> Option<&'static str> {
        surface_check(surface, background).map(|(reason, _)| reason)
    }

    #[test]
    fn a_plain_audio_session_with_a_background_can_run_the_mode() {
        assert_eq!(reason(PLAIN, true), None);
    }

    #[test]
    fn an_orchestrator_runs_on_its_model_with_room_to_think() {
        let own: ConversationWebSocketConfig = serde_json::from_value(json!({
            "base_url": "http://llm.test/v1", "model": "small", "system_prompt": "Be brief.",
        }))
        .unwrap();
        // A model name runs on the session's LLM, with the session's prompt.
        let named = orchestrator_llm(&json!("large"), Some(own.clone())).unwrap();
        assert_eq!(
            (named.base_url.as_str(), named.model.as_str()),
            ("http://llm.test/v1", "large")
        );
        assert_eq!(named.system_prompt.as_deref(), Some("Be brief."));
        // Not the voice cap: the reasoning tier's budget, unless the block sets one.
        assert_eq!(named.max_tokens, Some(4096));
        let block = json!({
            "base_url": "http://big.test/v1", "model": "planner",
            "system_prompt": "You plan.", "max_tokens": 900,
        });
        let own_llm = orchestrator_llm(&block, Some(own)).unwrap();
        assert_eq!(own_llm.system_prompt.as_deref(), Some("You plan."));
        assert_eq!(own_llm.max_tokens, Some(900));
        // A model name with no LLM to run it on, or a block of another shape: none.
        assert!(orchestrator_llm(&json!("large"), None).is_none());
        assert!(orchestrator_llm(&json!(7), None).is_none());
    }

    fn entry(extra: Value) -> bud_auth::voice_agent::VoiceAgentEntry {
        let mut blob = json!({
            "prompt_id": "c0de", "version": 1,
            "stt": {"endpoint_id": "stt-1"}, "tts": {"endpoint_id": "tts-1"},
        });
        blob.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        bud_auth::voice_agent::parse_voice_agent_blob(&blob.to_string()).unwrap()
    }

    #[test]
    fn an_agent_sessions_block_is_the_entry_block_then_the_operators() {
        let authors = json!({ "background": { "answer_marker": "ANSWER:" } });
        let operators = json!({ "policy": { "ack_due_ms": 250 } });
        let server = ServerInteraction {
            enabled: true,
            // The general default is for other sessions: an agent never takes it.
            default: Some(json!(true)),
            agent_default: Some(operators.clone()),
            ..ServerInteraction::default()
        };
        let published = entry(json!({ "interaction": authors }));
        let plain = entry(json!({}));

        // budapp's field wins; the session named the agent, so it is told what comes of it.
        assert_eq!(
            agent_request(&published, None, &server),
            (Some((authors.clone(), true, true)), false)
        );
        // Until budapp publishes it: the operator's block, which the session did not ask for.
        assert_eq!(
            agent_request(&plain, None, &server),
            (Some((operators.clone(), false, true)), false)
        );
        let off = ServerInteraction {
            enabled: false,
            ..server.clone()
        };
        assert_eq!(agent_request(&plain, None, &off), (None, false));
        let none = ServerInteraction {
            agent_default: None,
            ..server.clone()
        };
        assert_eq!(agent_request(&plain, None, &none), (None, false));

        // A session's own value is refused, `false` included, unless the agent lets a
        // session override it; then it is a session's block, without the operator's trust.
        assert_eq!(
            agent_request(&published, Some(json!(false)), &server),
            (Some((authors, true, true)), true)
        );
        assert_eq!(
            agent_request(&plain, Some(json!(true)), &server),
            (Some((operators, true, true)), true)
        );
        let open = entry(json!({ "session_overrides": ["interaction"], "interaction": true }));
        assert_eq!(
            agent_request(&open, Some(json!(false)), &server),
            (Some((json!(false), true, false)), false)
        );
    }

    #[test]
    fn each_block_that_owns_the_turns_or_the_audio_rules_it_out() {
        // An agent session that answers in text or takes manual turns.
        let agent = Surface {
            agent: true,
            ..PLAIN
        };
        assert_eq!(reason(agent, true), Some("agent"));
        // A room, and so a phone call, runs it.
        let livekit = Surface {
            livekit: true,
            ..PLAIN
        };
        assert_eq!(reason(livekit, true), None);
        // A DAG that speaks itself, or says nothing; one that could not be read keeps the
        // general reason.
        for why in ["dag_audio_output", "dag_no_text_output", "dag_config"] {
            let dag = Surface {
                dag: Some(why),
                ..PLAIN
            };
            assert_eq!(reason(dag, true), Some(why));
        }
        let silent = Surface {
            audio: false,
            ..PLAIN
        };
        assert_eq!(reason(silent, true), Some("audio_disabled"));
    }

    #[test]
    fn only_an_english_session_has_a_policy_pack() {
        // A multilingual recogniser (`multi`, `auto`) runs on the English pack, each turn's
        // detected language checked against it.
        for language in ["en", "en-US", "EN-gb", "", " en ", "multi", "auto"] {
            assert!(pack_language(language), "{language:?}");
        }
        for language in ["hi", "es-ES", "de"] {
            assert!(!pack_language(language), "{language:?}");
        }
    }

    #[test]
    fn a_session_with_no_background_runs_without_it() {
        assert_eq!(reason(PLAIN, false), Some("no_background"));
    }
}
