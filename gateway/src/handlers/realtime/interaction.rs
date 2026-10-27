//! Interaction mode on `/realtime`: regimes B and C over the session's speech-to-speech
//! provider (`core::interaction::provider`). `prepare` decides at config time, from the
//! regime resolver, whether the session runs the mode and adds the delegate function to the
//! provider's session; `start` puts the executor between the handler and the provider.
//!
//! A session that asked and cannot have the mode gets one `warning` per finding and runs
//! exactly as it would have without the field.

use std::sync::Arc;

use serde_json::{Value, json};
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::core::interaction::actor::{Background, Msg};
use crate::core::interaction::clips::{self, Render};
use crate::core::interaction::config::{self, InteractionConfig, ParseCtx, ServerInteraction};
use crate::core::interaction::executor::{self, ExecHandle, Pending, Signal, Steered};
use crate::core::interaction::provider::{
    self, AudioOut, DELEGATE_TOOL, Plan, ProviderDeps, ProviderEvent,
};
use crate::core::interaction::regime::{self, VERB_NAMES};
use crate::core::realtime::{BaseRealtime, FunctionCallRequest, TranscriptRole};
use crate::handlers::ws::config::ConversationWebSocketConfig;

use super::messages::{
    FunctionConfig, RealtimeMessageRoute, RealtimeOutgoingMessage, RealtimeSessionConfig,
    ToolConfig, send_realtime_with_policy,
};

/// The background of a provider session: a backend of the operator's catalog that the block
/// names, else the LLM of `tasks.llm`. With the operator's system prompt. `None`: the block
/// names neither.
fn background(
    cfg: &InteractionConfig,
    server: &ServerInteraction,
) -> Option<Result<(Background, Option<String>), String>> {
    let llm: Option<ConversationWebSocketConfig> = cfg
        .tasks
        .llm
        .clone()
        .and_then(|llm| serde_json::from_value(llm).ok());
    let system_prompt = llm.as_ref().and_then(|llm| llm.system_prompt.clone());
    let named = cfg.tasks.backend.as_deref();
    if let Some(spec) = named.and_then(|name| server.task_backends.iter().find(|b| b.name == name))
    {
        let remote = Background::remote(spec.clone(), cfg, server, system_prompt);
        return Some(Ok((remote, None)));
    }
    let mut llm = llm?.to_conversation_config();
    llm.streaming = true;
    Some(Background::llm(&llm).map(|background| (background, system_prompt)))
}

/// The `/ws` session whose voice is a realtime provider (`interaction.speech.realtime`).
pub(crate) const WS_SURFACE: &str = "ws_realtime_voice";

/// The release a surface is resolved at: this build serves the `/ws` realtime voice, a
/// Release 4 surface, while the other surfaces stay at [`regime::RELEASE`].
fn release(surface: &str) -> u64 {
    if surface == WS_SURFACE {
        regime::RELEASE.max(4)
    } else {
        regime::RELEASE
    }
}

/// The regime resolver's answer for a speech-to-speech session of this gateway.
pub(crate) fn resolve(
    surface: &str,
    provider: &str,
    model: Option<&str>,
    cfg: Option<&InteractionConfig>,
    server: &ServerInteraction,
    has_background: bool,
    bud_mode: bool,
) -> Value {
    let block = cfg.map_or(Value::Null, |cfg| cfg.speech.block());
    regime::resolve_session(&json!({
        "surface": surface,
        "provider": provider,
        "model": model,
        "release": release(surface),
        "today": regime::today(),
        // `/realtime` and `/ws` applied the lifecycle to the session before (`handler.rs`).
        "lifecycle": !matches!(surface, "realtime" | WS_SURFACE),
        "bud_mode": bud_mode,
        "interaction": block,
        "server": {"enabled": server.enabled, "build_features": true,
                   "truth_ack": server.truth_ack, "front_model": {}},
        "has_background": has_background,
    }))
}

/// The verbs this build can send the provider, by name.
fn verbs_of(provider: &str) -> Value {
    VERB_NAMES
        .iter()
        .zip(regime::verbs(provider))
        .map(|(name, supported)| ((*name).to_owned(), Value::Bool(supported)))
        .collect::<serde_json::Map<_, _>>()
        .into()
}

/// The resolver's findings as warnings: `(code, message, detail)`. A refusal comes first.
fn findings(resolution: &Value, surface: &str, provider: &str) -> Vec<(String, String, Value)> {
    let mut out = Vec::new();
    if let Some(code) = resolution["refusal"]
        .as_str()
        .filter(|c| *c != "interaction_absent")
    {
        let mut detail =
            json!({"surface": surface, "provider": provider, "verbs": verbs_of(provider)});
        if let Some(found) = resolution["detail"].as_object() {
            for (key, value) in found {
                detail[key] = value.clone();
            }
        }
        let message = match code {
            "truth_ack_required" => "Interaction mode with this provider needs the operator to accept its own speech as monitored (interaction.truth_ack); the session runs without it.".to_owned(),
            "interaction_needs_background" => "Interaction mode needs a background: interaction.tasks.llm or a backend of the operator's (tasks.default_backend). The session runs without it.".to_owned(),
            _ => format!("Interaction mode is not available on this session ({code}); it runs without it."),
        };
        out.push((code.to_owned(), message, detail));
    }
    for code in resolution["warnings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        let message = format!("Interaction mode: {code}.");
        out.push((
            code.to_owned(),
            message,
            json!({"surface": surface, "provider": provider}),
        ));
    }
    out
}

/// What the session was told it runs: the `/ws` settings echo with the regime and the
/// provider's part.
fn echo(
    cfg: &InteractionConfig,
    plan: &Plan,
    resolution: &Value,
    backend: &str,
    provider: &str,
) -> Value {
    let mut echo = config::echo(cfg, backend, &[]);
    echo["regime"] = plan.regime.as_str().into();
    echo["speech"] = json!({
        "provider": provider,
        "verbs": verbs_of(provider),
        "truth": resolution["truth"],
        "filler_owner": resolution["filler_owner"],
        "confirm_irreversible": resolution["confirm_irreversible"],
        "confirm_reversible": resolution["confirm_reversible"],
        "claim_policy": cfg.speech.claim_policy,
        "delegate_tool": DELEGATE_TOOL,
        // `gateway` in the half-cascade: WaaV's voice says every word.
        "voice_owner": resolution["voice_owner"],
    });
    echo
}

/// The provider gets the delegate function and the regime's instructions after the session's
/// own, and transcribes the caller, whose words answer the gateway's questions. Also on each
/// `update_session`, which replaces both.
pub(super) fn steer(config: &mut RealtimeSessionConfig, suffix: &str) {
    let tools = config.tools.get_or_insert_with(Vec::new);
    if !tools.iter().any(|tool| tool.function.name == DELEGATE_TOOL) {
        let (description, parameters) = provider::delegate_tool();
        tools.push(ToolConfig {
            tool_type: "function".to_owned(),
            function: FunctionConfig {
                name: DELEGATE_TOOL.to_owned(),
                description: Some(description.to_owned()),
                parameters: Some(parameters),
            },
        });
    }
    let instructions = provider::with_instructions(config.instructions.as_deref(), suffix);
    config.instructions = Some(instructions);
    config.transcribe_input.get_or_insert(true);
}

/// What every speech-to-speech surface decided: the session runs the mode with these.
pub(crate) struct Planned {
    pub(crate) plan: Plan,
    pub(crate) cfg: InteractionConfig,
    pub(crate) background: Background,
    pub(crate) system_prompt: Option<String>,
    pub(crate) echo: Value,
    /// The regime's instructions, after the session's own.
    pub(crate) suffix: &'static str,
    /// The provider's id (`regime::provider_id`).
    pub(crate) provider: &'static str,
}

impl Planned {
    /// What the session runs on, once the executor is up. `audio`: where its own lines (the
    /// failure lines, the gateway's voice) play, at `output_rate`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn deps(
        self,
        exec: ExecHandle,
        out: Box<dyn Fn(String) + Send + Sync>,
        stream_id: &str,
        owner: &str,
        instructions: String,
        audio: Option<AudioOut>,
        output_rate: u32,
        voice: Option<Render>,
    ) -> ProviderDeps {
        ProviderDeps {
            plan: self.plan,
            cfg: self.cfg,
            server: config::server(),
            background: self.background,
            system_prompt: self.system_prompt,
            credential: None,
            exec,
            out,
            stream_id: stream_id.to_owned(),
            owner: owner.to_owned(),
            instructions,
            audio,
            output_rate,
            voice,
            voice_audio: None,
            phone: None,
        }
    }
}

/// The surfaces that play the gateway's own audio (the half-cascade's voice, its clips). The
/// relay plays it as responses of the gateway's own among the vendor's GA events.
const VOICED_SURFACES: [&str; 3] = ["realtime", WS_SURFACE, "v1_realtime_relay"];

/// The voice a session's gateway lines are said in: `speech.voice`, else the server's
/// `failure_voice` (`None`: neither, or not a `tts_config`).
fn voice_value(cfg: &InteractionConfig, server: &ServerInteraction) -> Option<Value> {
    let value = cfg
        .speech_voice
        .clone()
        .or_else(|| server.failure_voice.clone())?;
    serde_json::from_value::<crate::handlers::ws::config::TTSWebSocketConfig>(value.clone())
        .ok()
        .map(|_| value)
}

/// The session's gateway voice, ready to say lines (see [`voice_value`]).
pub(crate) fn voice_of(
    cfg: &InteractionConfig,
    state: &Arc<crate::state::AppState>,
) -> Option<Render> {
    let value = voice_value(cfg, &config::server())?;
    match clips::gateway_voice(&value, state) {
        Ok((voice, render)) => {
            info!(provider = %voice.provider, voice = %voice.voice, "interaction gateway voice of a speech-to-speech session");
            Some(render)
        }
        Err(error) => {
            warn!(%error, "interaction gateway voice not usable");
            None
        }
    }
}

/// A session that runs the mode: what `decide` found and `start` uses.
pub(crate) struct Prepared {
    pub(crate) planned: Planned,
    /// The provider's instructions with the regime's.
    instructions: String,
    pub(crate) inbox: mpsc::Sender<Msg>,
    rx: mpsc::Receiver<Msg>,
    pub(crate) exec: ExecHandle,
    pending: Pending,
}

/// One finding for the client: `(code, message, detail)`.
type Finding = (String, String, Value);

/// The decision every speech-to-speech surface makes on an `interaction` block: parse it,
/// resolve the regime for the provider, build the background. `None`: the session runs
/// without the mode. The findings, in order, are the surface's to send or log.
pub(crate) fn plan(
    raw: &Value,
    ctx: ParseCtx,
    server: &ServerInteraction,
    surface: &str,
    provider_name: &str,
    model: Option<&str>,
) -> (Option<Planned>, Vec<Finding>) {
    let (cfg, warnings) = config::parse(raw, server, ctx);
    let mut out: Vec<Finding> = warnings
        .into_iter()
        .map(|w| (w.code.to_owned(), w.message, w.detail))
        .collect();
    let Some(mut cfg) = cfg else {
        return (None, out);
    };
    // A session's own `tasks.speculative_start` means nothing here: the provider decides
    // what is a task.
    cfg.tasks.speculative_start = config::Tri::Off;
    // An orchestrator runs on the cascade's LLM tier only.
    if cfg.tasks.orchestrator.take().is_some() {
        cfg.tasks.agents.clear();
        let path = "interaction.tasks.orchestrator";
        let message = format!("{path} runs on cascade /ws sessions only; it is ignored.");
        out.push((
            "interaction_value_ignored".to_owned(),
            message,
            serde_json::json!({ "path": path }),
        ));
    }
    // `tasks.backends`: no orchestrator chooses among them here, so the session's named
    // backend, else the first listed, does its tasks.
    let listed = std::mem::take(&mut cfg.tasks.backends);
    if let Some(first) = listed.first().filter(|_| cfg.tasks.backend.is_none()) {
        cfg.tasks.backend = Some(first.clone());
    }
    if listed
        .iter()
        .any(|name| cfg.tasks.backend.as_ref() != Some(name))
    {
        let path = "interaction.tasks.backends";
        let message = format!(
            "{path}: one backend does the tasks of a speech-to-speech session (an orchestrator \
             chooses among several on cascade /ws sessions only); the others are ignored."
        );
        out.push((
            "interaction_value_ignored".to_owned(),
            message,
            serde_json::json!({ "path": path }),
        ));
    }
    let provider = regime::provider_id(provider_name);
    let built = background(&cfg, server);
    let resolution = resolve(
        surface,
        provider,
        model,
        Some(&cfg),
        server,
        built.is_some(),
        ctx.bud_mode,
    );
    out.extend(findings(&resolution, surface, provider));
    if resolution["interaction"] != "on" {
        return (None, out);
    }
    let unavailable = |message: String, detail: Value| {
        (
            "interaction_surface_unavailable".to_owned(),
            message,
            detail,
        )
    };
    // No fallback (chapter 9; owner, 24 Oct): the session's fast front reads the caller's
    // answer to a question, a cancel and a continuer (no word list does), so a server without a
    // front model, or with the one it runs not ready, runs the session without the mode.
    let front_ready = server.front_model.as_ref().is_some_and(|front| {
        let supervised = crate::core::interaction::front_supervisor::status();
        front.command.is_none() || supervised.is_none_or(|(ready, _)| ready)
    });
    if !front_ready {
        let detail =
            json!({"surface": surface, "provider": provider, "reason": "front_unavailable"});
        let message = "Interaction mode reads the caller's words with the server's front model, \
                       which is not ready (interaction.front_model); the session runs without it."
            .to_owned();
        out.push(unavailable(message, detail));
        return (None, out);
    }
    let correct = cfg.speech.claim_policy == "correct";
    let planned_regime = Plan::of(&resolution, regime::verbs(provider), correct);
    // The half-cascade speaks in the gateway's own voice: on a surface that plays the
    // gateway's audio, with a voice to speak in.
    let voiceless = planned_regime.as_ref().is_some_and(|plan| plan.half)
        && (!VOICED_SURFACES.contains(&surface) || voice_value(&cfg, server).is_none());
    let Some(mut plan) = planned_regime.filter(|_| !voiceless) else {
        let reason = if voiceless && VOICED_SURFACES.contains(&surface) {
            "gateway_voice_unavailable"
        } else {
            "half_cascade"
        };
        let detail = json!({"surface": surface, "provider": provider, "reason": reason,
                            "regime": resolution["regime"], "variant": resolution["variant"]});
        let message = match reason {
            "gateway_voice_unavailable" => "The half-cascade speaks in the gateway's own voice: give interaction.speech.voice (a tts_config) or have the operator set interaction.failure_voice. The session runs without interaction mode.".to_owned(),
            _ => "The half-cascade (WaaV's voice over the provider's listening) is not available on this surface; the session runs without interaction mode.".to_owned(),
        };
        out.push(unavailable(message, detail));
        return (None, out);
    };
    // `/realtime` has no `audio_segment` (plan chapter 6, events).
    plan.segments = surface != "realtime";
    let (background, system_prompt) = match built {
        Some(Ok(built)) => built,
        Some(Err(error)) => {
            let detail = json!({"surface": surface, "reason": "background_unavailable"});
            let message =
                format!("Interaction mode is not available: {error}. The session runs without it.");
            out.push(unavailable(message, detail));
            return (None, out);
        }
        None => return (None, out),
    };
    let echo = echo(&cfg, &plan, &resolution, &background.label(), provider);
    info!(
        provider,
        surface,
        regime = plan.regime.as_str(),
        "interaction mode on a speech-to-speech session"
    );
    let suffix = provider::instructions(plan.regime);
    let planned = Planned {
        plan,
        cfg,
        background,
        system_prompt,
        echo,
        suffix,
        provider,
    };
    (Some(planned), out)
}

async fn warn_client(tx: &mpsc::Sender<RealtimeMessageRoute>, (code, message, detail): Finding) {
    let warning = RealtimeOutgoingMessage::ConfigWarning {
        code,
        message,
        detail,
    };
    send_realtime_with_policy(tx, RealtimeMessageRoute::Outgoing(warning)).await;
}

/// Decide whether the session runs interaction mode. `Some`: the session config now carries
/// the delegate function and the regime's instructions, and the provider's input transcript
/// is on.
pub(super) async fn prepare(
    config: &mut RealtimeSessionConfig,
    provider_name: &str,
    message_tx: &mpsc::Sender<RealtimeMessageRoute>,
) -> Option<Prepared> {
    let (prepared, findings) = decide(config, provider_name, "realtime");
    for finding in findings {
        warn_client(message_tx, finding).await;
    }
    prepared
}

/// [`prepare`] for any speech-to-speech surface of this gateway (`realtime`, or
/// [`WS_SURFACE`]): the session's findings, in order, are returned for the surface to send.
pub(crate) fn decide(
    config: &mut RealtimeSessionConfig,
    provider_name: &str,
    surface: &str,
) -> (Option<Prepared>, Vec<Finding>) {
    let Some(raw) = config.interaction.clone() else {
        return (None, Vec::new());
    };
    let model = config.model.clone();
    let server = config::server();
    let (planned, findings) = plan(
        &raw,
        ParseCtx::default(),
        &server,
        surface,
        provider_name,
        model.as_deref(),
    );
    let Some(mut planned) = planned else {
        return (None, findings);
    };
    // Provider speculation and Gemini's delivery scheduling, where the session's provider
    // fixes let the session see a withdrawn call, and send `scheduling`.
    let fixes = config::session_fixes(config.provider_fixes.as_ref()).0;
    let fix = |name: &str| fixes.iter().any(|on| on == name);
    if planned.provider == "gemini" {
        if fix("gemini_tool_call_cancellation") {
            planned.plan.adopt_by = regime::adopt_by(planned.provider, model.as_deref());
        }
        planned.plan.scheduling = fix("gemini_response_scheduling");
    }
    // The half-cascade: the provider writes its replies as text.
    if planned.plan.half {
        config.modalities = Some(vec!["text".to_owned()]);
        config.text_only = true;
    }
    steer(config, planned.suffix);
    let instructions = config.instructions.clone().unwrap_or_default();
    let (inbox, rx) = mpsc::channel(256);
    let (exec, pending) = executor::handle();
    let prepared = Prepared {
        planned,
        instructions,
        inbox,
        rx,
        exec,
        pending,
    };
    (Some(prepared), findings)
}

/// What the provider's callbacks do besides their plain-session work. The same on every
/// surface; each surface sends the provider's audio and words on its own way.
#[derive(Clone)]
pub(crate) struct Tap {
    pub(crate) inbox: mpsc::Sender<Msg>,
    pub(crate) exec: ExecHandle,
    /// The half-cascade: the provider's audio never reaches the caller.
    pub(crate) mute: bool,
}

impl Tap {
    pub(crate) async fn provider(&self, event: ProviderEvent) {
        let _ = self.inbox.send(Msg::Provider(event)).await;
    }

    pub(crate) fn signal(&self, signal: Signal) {
        self.exec.signal(signal);
    }

    /// The provider's words: its first words open a response of its own; the final words of
    /// both sides are read.
    pub(crate) async fn transcript(&self, role: &TranscriptRole, text: &str, is_final: bool) {
        match (role, is_final) {
            (TranscriptRole::Assistant, false) => {
                self.signal(Signal::Responding);
                self.provider(ProviderEvent::Saying(text.to_owned())).await;
            }
            (TranscriptRole::Assistant, true) => {
                self.provider(ProviderEvent::Said(text.to_owned())).await;
            }
            (TranscriptRole::User, true) => {
                self.provider(ProviderEvent::Heard(text.to_owned())).await;
            }
            (TranscriptRole::User, false) => {}
        }
    }

    /// `false`: the output gate holds the provider's audio (a stop is in force).
    pub(crate) fn audio(&self, bytes: usize) -> bool {
        if self.mute || !self.exec.gate().pass() {
            return false;
        }
        self.signal(Signal::Audio(bytes));
        true
    }

    /// A function call is part of a response of the provider's own. A call of the delegate
    /// function is the gateway's and is taken (`None`); any other is the session's.
    pub(crate) async fn call(&self, call: FunctionCallRequest) -> Option<FunctionCallRequest> {
        self.signal(Signal::Responding);
        // The delegate function's calls are the gateway's: the client never sees them.
        if call.name != DELEGATE_TOOL {
            return Some(call);
        }
        let event = ProviderEvent::Call {
            call_id: call.call_id,
            name: call.name,
            arguments: call.arguments,
        };
        self.provider(event).await;
        None
    }

    pub(crate) async fn response_done(&self) {
        self.provider(ProviderEvent::ResponseDone).await;
        self.signal(Signal::ResponseDone);
    }

    /// The caller started or stopped speaking (the provider's speech events).
    pub(crate) async fn caller(&self, speaking: bool) {
        self.provider(ProviderEvent::CallerSpeaking(speaking)).await;
    }

    /// The provider reported an error: the health watcher hears of it (chapter 10).
    pub(crate) async fn failed(&self, error: &crate::core::realtime::RealtimeError) {
        if !matches!(
            error,
            crate::core::realtime::RealtimeError::ProviderWarning { .. }
        ) {
            self.provider(ProviderEvent::Failed(error.to_string()))
                .await;
        }
    }

    /// The provider's connection came back, or is lost for good (`error`): the session plays
    /// the exit line and ends with `session_failed`, after which the surface closes.
    pub(crate) async fn reconnection(&self, success: bool, error: Option<String>) {
        if success {
            self.signal(Signal::Reconnected);
            self.provider(ProviderEvent::Reconnected).await;
        } else {
            let reason =
                error.unwrap_or_else(|| "the connection could not be re-established".into());
            self.provider(ProviderEvent::Gone(reason)).await;
        }
    }
}

/// Whether a session event is `session_failed`: the surface closes the socket after it.
pub(crate) fn is_session_failed(json: &str) -> bool {
    json.starts_with(r#"{"type":"session_failed""#)
}

/// Start the executor and the session over the connected provider. Returns the handler's
/// provider: the executor's `Steered`.
pub(super) fn start(
    prepared: Prepared,
    provider: Box<dyn BaseRealtime>,
    stream_id: &str,
    owner: &str,
    message_tx: &mpsc::Sender<RealtimeMessageRoute>,
    voice: Option<Render>,
) -> Box<dyn BaseRealtime> {
    // Events (and the session's own audio) leave in order without the session waiting for
    // the client; `session_failed` closes the socket after it.
    let (events, mut queued) = mpsc::unbounded_channel::<RealtimeMessageRoute>();
    let tx = message_tx.clone();
    tokio::spawn(async move {
        while let Some(route) = queued.recv().await {
            let failed =
                matches!(&route, RealtimeMessageRoute::Json(json) if is_session_failed(json));
            send_realtime_with_policy(&tx, route).await;
            if failed {
                send_realtime_with_policy(&tx, RealtimeMessageRoute::Close).await;
            }
        }
    });
    let audio_events = events.clone();
    let out = Box::new(move |json| {
        if events.send(RealtimeMessageRoute::Json(json)).is_err() {
            warn!("interaction event after the session ended");
        }
    });
    let audio: AudioOut = Box::new(move |pcm| {
        let _ = audio_events.send(RealtimeMessageRoute::Audio(pcm));
    });
    start_with(
        prepared,
        provider,
        stream_id,
        owner,
        out,
        Some(audio),
        voice,
        None,
    )
}

/// [`start`] with the surface's own way out for the session's events (one JSON each) and its
/// own audio (the failure lines and the gateway's voice, PCM16 at the provider's output
/// rate). `phone`: a room's caller line (a caller on a phone hears the keypad hints).
#[allow(clippy::too_many_arguments)]
pub(crate) fn start_with(
    prepared: Prepared,
    provider: Box<dyn BaseRealtime>,
    stream_id: &str,
    owner: &str,
    out: Box<dyn Fn(String) + Send + Sync>,
    audio: Option<AudioOut>,
    voice: Option<Render>,
    phone: Option<Arc<crate::core::audio::playback_pump::PumpControl>>,
) -> Box<dyn BaseRealtime> {
    let Prepared {
        planned,
        instructions,
        inbox,
        rx,
        exec,
        pending,
    } = prepared;
    let info = provider.get_provider_info();
    let rates = provider.audio_rates();
    let output_rate = rates.map_or(24_000, |(_, out)| out);
    executor::start(
        &exec,
        pending,
        Box::new(provider),
        inbox.clone(),
        output_rate,
    );
    let mut deps = planned.deps(
        exec.clone(),
        out,
        stream_id,
        owner,
        instructions,
        audio,
        output_rate,
        voice,
    );
    deps.phone = phone;
    provider::spawn(deps, inbox, rx);
    Box::new(Steered::new(exec, info, rates))
}

/// `Arc` for the tap in the callbacks.
pub(crate) fn tap(prepared: &Prepared) -> Arc<Tap> {
    Arc::new(Tap {
        inbox: prepared.inbox.clone(),
        exec: prepared.exec.clone(),
        mute: prepared.planned.plan.half,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `/ws` realtime voice is a Release 4 surface that this build serves: regimes B and C
    /// resolve there as on `/realtime`, with the operator's acceptance of the provider's speech.
    #[test]
    fn the_rooms_realtime_voice_resolves_as_realtime_does() {
        let server = ServerInteraction {
            enabled: true,
            truth_ack: vec!["openai".to_owned()],
            ..ServerInteraction::default()
        };
        let cfg = InteractionConfig::default();
        for surface in ["realtime", WS_SURFACE] {
            let resolved = resolve(surface, "openai", None, Some(&cfg), &server, true, false);
            assert_eq!(resolved["interaction"], "on", "{surface}: {resolved}");
            assert_eq!(resolved["regime"], "ProviderSpeech", "{surface}");
        }
        let gemini = resolve(WS_SURFACE, "gemini", None, Some(&cfg), &server, true, false);
        assert_eq!(gemini["refusal"], "truth_ack_required", "{gemini}");
        assert_eq!(release(WS_SURFACE), 4);
        assert_eq!(release("realtime"), regime::RELEASE);
    }

    /// The half-cascade (`speech.voice_owner: gateway` on an OpenAI provider) runs on the
    /// surfaces that play the gateway's audio (`/realtime`, a room, the relay), with a voice to
    /// speak in; with no voice the session runs without the mode and says why.
    #[test]
    fn the_half_cascade_needs_the_gateways_voice_and_a_surface_that_plays_it() {
        let block = json!({"speech": {"voice_owner": "gateway"},
                           "tasks": {"llm": {"base_url": "https://llm.example.com/v1", "model": "m"}}});
        let mut server = ServerInteraction {
            enabled: true,
            failure_voice: Some(json!({"provider": "deepgram"})),
            front_model: Some(crate::core::interaction::config::FrontModelServer {
                url: "http://127.0.0.1:9".to_owned(),
                ..Default::default()
            }),
            ..ServerInteraction::default()
        };
        let ctx = ParseCtx::default();
        // Without a front model the session runs without the mode, and says why (owner, 24
        // Oct: the front reads the caller's answer, a cancel and a continuer).
        let frontless = ServerInteraction {
            front_model: None,
            ..server.clone()
        };
        let (planned, findings) = plan(&block, ctx, &frontless, "realtime", "openai", None);
        assert!(planned.is_none());
        let reason = findings.last().map(|f| f.2["reason"].clone());
        assert_eq!(reason, Some(json!("front_unavailable")), "{findings:?}");
        for surface in ["realtime", WS_SURFACE] {
            let (planned, findings) = plan(&block, ctx, &server, surface, "openai", None);
            let planned = planned.unwrap_or_else(|| panic!("{surface}: {findings:?}"));
            assert!(planned.plan.half, "{surface}");
            assert_eq!(planned.plan.segments, surface != "realtime");
            assert_eq!(planned.echo["regime"], "provider_speech");
        }
        // The relay's block is the operator's, under the Bud control plane.
        let relay = ParseCtx {
            bud_mode: true,
            operator: true,
        };
        // r4-left: the relay plays the gateway's voice as responses of its own.
        let (planned, findings) = plan(&block, relay, &server, "v1_realtime_relay", "openai", None);
        let planned = planned.unwrap_or_else(|| panic!("relay: {findings:?}"));
        assert!(planned.plan.half && planned.plan.segments);
        server.failure_voice = None;
        let (planned, findings) = plan(&block, ctx, &server, "realtime", "openai", None);
        assert!(planned.is_none());
        let reason = findings.last().map(|f| f.2["reason"].clone());
        assert_eq!(
            reason,
            Some(json!("gateway_voice_unavailable")),
            "{findings:?}"
        );
        // The session's own voice is enough.
        let mut own = block.clone();
        own["speech"]["voice"] = json!({"provider": "deepgram", "voice_id": "aura-2-thalia-en"});
        let (planned, _) = plan(&own, ctx, &server, "realtime", "openai", None);
        assert!(planned.is_some_and(|planned| planned.plan.half));
    }
}
