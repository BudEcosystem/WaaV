//! `/v1/realtime?model=prompt:<agent>` — a Bud voice agent behind the OpenAI Realtime GA protocol
//! (spec 025 D-5, §5.6). The third engine beside `Relay` and `Translate`: `Cascade`.
//!
//! The session runs a `/ws` voice-agent session IN-PROCESS — the same legs, turn-taking, agent engine,
//! metering and revalidation a native `/ws` client gets — and translates between the two protocols:
//! GA client events become `/ws` actions (audio, commit, typed turns, cancel, truncate), and the `/ws`
//! session's messages become GA server events. One cascade core; two front-ends.
//!
//! [`CascadeGa`] is the translation, pure and unit-tested. [`run`] is the socket loop around it.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use axum::http::{HeaderMap, StatusCode};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use bytes::Bytes;
use futures_util::StreamExt;
use serde_json::{Map, Value, json};
use tokio::sync::{RwLock, mpsc};
use tracing::{debug, info, warn};

use crate::handlers::ws::config::{AgentWebSocketConfig, STTWebSocketConfig, TTSWebSocketConfig};
use crate::handlers::ws::messages::{IncomingMessage, MessageRoute, OutgoingMessage};
use crate::handlers::ws::state::ConnectionState;
use crate::middleware::connection_limit::ConnectionSlot;
use crate::state::{AppState, ResolvedVoiceAgent, VoiceAgentRefusal};

use super::handshake::{self, HandshakeError};
use super::session::{CLIENT_QUEUE, Caller, Outbound, gateway_error, next_event_id, spawn_writer};

/// The GA audio format: PCM16 mono at 24 kHz, both directions.
const GA_RATE: u32 = 24_000;
/// How long the session waits for a `session.update` before starting the agent with its defaults.
const START_GRACE: Duration = Duration::from_millis(600);
/// Client audio held while the agent's legs connect (~10 s of 24 kHz PCM16).
const MAX_HELD_AUDIO: usize = 480_000;
const CORE_QUEUE: usize = 1024;

/// Is this `?model=` a voice agent?
pub fn is_agent_model(model: &str) -> bool {
    model.trim().starts_with("prompt:")
}

/// Everything decided before the upgrade.
pub struct PreparedCascade {
    pub caller: Caller,
    pub agent: ResolvedVoiceAgent,
    pub model: String,
    pub credential: String,
}

impl std::fmt::Debug for PreparedCascade {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedCascade")
            .field("model", &self.model)
            .field("version", &self.agent.version)
            .finish()
    }
}

/// Authenticate and resolve the agent (§5.4 steps 1-3). Legs are resolved and admitted by the
/// in-process `/ws` session once the socket is open; a refusal there is a GA `error` + close.
pub async fn prepare(
    state: &AppState,
    query: Option<&str>,
    headers: &HeaderMap,
) -> Result<PreparedCascade, HandshakeError> {
    let hs = handshake::parse(query, headers)?;
    if state.bud_mode.is_none() {
        return Err(HandshakeError::new(
            StatusCode::NOT_FOUND,
            "bud_mode_required",
            "Voice agents are Bud agents; this gateway has no Bud control plane.",
        ));
    }
    if hs.credential.is_client_secret() {
        // NG-5: an `ek_bud_` holder has no credential budgateway would accept for the agent's turns.
        return Err(HandshakeError::new(
            StatusCode::UNAUTHORIZED,
            "client_secret_unsupported",
            "Client secrets are not supported for voice agents yet: connect with your Bud API key or \
             token (from a backend, or the playground).",
        ));
    }
    let caller = super::session::authenticate(state, &hs.credential).await?;
    let agent = state
        .resolve_voice_agent(&hs.model, hs.credential.expose())
        .await
        .map_err(|why| match why {
            VoiceAgentRefusal::NotFound => HandshakeError::new(
                StatusCode::NOT_FOUND,
                "model_not_found",
                format!(
                    "Agent '{}' not found, or this credential cannot reach it.",
                    hs.model
                ),
            )
            .param("model"),
            VoiceAgentRefusal::NotVoiceEnabled => HandshakeError::new(
                StatusCode::NOT_FOUND,
                "agent_not_voice_enabled",
                format!(
                    "Agent '{}' has no voice: give its version an STT and a TTS deployment in its \
                     voice settings.",
                    hs.model
                ),
            )
            .param("model"),
        })?;
    Ok(PreparedCascade {
        caller,
        model: hs.model,
        credential: hs.credential.expose().to_string(),
        agent,
    })
}

// =============================================================================================
// The translation
// =============================================================================================

/// What a GA client event asks the session to do.
#[derive(Debug, Clone, PartialEq)]
pub enum ClientAct {
    /// Start the agent session now (the first event that needs it).
    Start,
    Audio(Bytes),
    /// Manual mode: end of the user's audio — commit it as a turn.
    Commit,
    Clear,
    /// A typed user turn.
    Turn(String),
    Cancel,
    Truncate(u64),
    Variables(Map<String, Value>),
    TextOnly(bool),
    Manual(bool),
    Auth(String),
}

#[derive(Debug, Default)]
struct UserItem {
    id: Option<String>,
    started_ms: u64,
}

#[derive(Debug)]
struct Resp {
    id: String,
    turn: u64,
    greeting: bool,
    item_id: Option<String>,
    item_index: u64,
    next_index: u64,
    transcript: String,
    output: Vec<Value>,
    tools: HashMap<String, (u64, Value)>,
    bud_response_id: Option<String>,
}

/// GA ⇄ `/ws` agent translation for one session.
pub struct CascadeGa {
    session_id: String,
    model: String,
    prompt_name: String,
    version: i64,
    stt_endpoint: String,
    voice: Option<String>,
    speed: Option<f64>,
    language: Option<String>,
    allow: Vec<String>,
    text_only: bool,
    manual: bool,
    /// The agent takes turns semantically (SmartTurn at its eagerness), not on silence.
    semantic: bool,
    /// The caller may cut the agent's answer (`interruption.enabled`).
    interrupt_response: bool,
    /// Semantic turn-taking eagerness: the agent's, or the caller's where the agent allows it.
    eagerness: String,
    eagerness_changed: bool,
    variables: Option<Map<String, Value>>,
    started: bool,
    turns_ran: bool,
    user: UserItem,
    /// A typed user message waiting for `response.create`.
    typed: Option<String>,
    response: Option<Resp>,
    /// The assistant item of each turn, for `conversation.item.truncated`.
    items: HashMap<u64, String>,
    last_item_id: Option<String>,
    clock: std::time::Instant,
}

fn new_id(prefix: &str) -> String {
    format!("{prefix}_bud_{}", uuid::Uuid::new_v4().simple())
}

fn event(kind: &str, body: Value) -> String {
    let mut m = match body {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    m.insert("type".into(), Value::from(kind));
    m.insert("event_id".into(), Value::from(next_event_id()));
    Value::Object(m).to_string()
}

fn refuse(param: &str, message: &str, event_id: Option<&str>) -> String {
    gateway_error("event_not_allowed", message, Some(param), event_id)
}

fn ga_usage(usage: &Value) -> Value {
    let input = usage
        .get("input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output = usage
        .get("output_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cached = usage
        .get("input_tokens_details")
        .and_then(|d| d.get("cached_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    json!({
        "total_tokens": usage.get("total_tokens").and_then(Value::as_u64).unwrap_or(input + output),
        "input_tokens": input,
        "output_tokens": output,
        "input_token_details": {"text_tokens": input, "audio_tokens": 0, "cached_tokens": cached},
        "output_token_details": {"text_tokens": output, "audio_tokens": 0},
    })
}

/// The error type GA clients expect for a code.
fn error_kind(code: &str) -> &'static str {
    match code {
        "upstream_error"
        | "upstream_unavailable"
        | "internal_error"
        | "server_shutdown"
        | "session_limit"
        | "idle_timeout" => "server_error",
        _ => "invalid_request_error",
    }
}

impl CascadeGa {
    pub fn new(session_id: &str, model: &str, agent: &ResolvedVoiceAgent) -> Self {
        let e = &agent.entry;
        Self {
            session_id: session_id.to_string(),
            model: model.to_string(),
            prompt_name: agent.prompt_name.clone(),
            version: agent.version,
            stt_endpoint: e.stt.endpoint_id.clone(),
            voice: e.tts.voice.clone(),
            speed: e.tts.speed,
            language: e.stt.language.clone(),
            allow: e.session_overrides.clone(),
            text_only: false,
            manual: e.turn_detection.kind == "manual",
            semantic: e.turn_detection.kind == "semantic",
            interrupt_response: e.interruption.enabled,
            eagerness: e.turn_detection.eagerness.clone(),
            eagerness_changed: false,
            variables: None,
            started: false,
            turns_ran: false,
            user: UserItem::default(),
            typed: None,
            response: None,
            items: HashMap::new(),
            last_item_id: None,
            clock: std::time::Instant::now(),
        }
    }

    pub fn is_started(&self) -> bool {
        self.started
    }

    pub fn mark_started(&mut self) {
        self.started = true;
    }

    fn allows(&self, setting: &str) -> bool {
        self.allow.iter().any(|a| a == setting)
    }

    /// The GA session object (instructions and tools are the agent's, never shown: §5.6).
    pub fn session_object(&self) -> Value {
        let format = json!({"type": "audio/pcm", "rate": GA_RATE});
        let turn_detection = if self.manual {
            Value::Null
        } else if self.semantic {
            json!({"type": "semantic_vad", "eagerness": self.eagerness,
                   "create_response": true, "interrupt_response": self.interrupt_response})
        } else {
            json!({"type": "server_vad", "create_response": true,
                   "interrupt_response": self.interrupt_response})
        };
        json!({
            "object": "realtime.session",
            "type": "realtime",
            "id": self.session_id,
            "model": self.model,
            "output_modalities": [if self.text_only { "text" } else { "audio" }],
            "instructions": "",
            "tools": [],
            "tool_choice": "auto",
            "prompt": {"id": self.prompt_name, "version": self.version.to_string()},
            "audio": {
                "input": {"format": format, "turn_detection": turn_detection,
                          "transcription": {"model": self.stt_endpoint, "language": self.language}},
                "output": {"format": format, "voice": self.voice, "speed": self.speed},
            },
        })
    }

    pub fn session_created(&self) -> String {
        event("session.created", json!({"session": self.session_object()}))
    }

    /// The `/ws` config the in-process session starts with.
    pub fn core_config(
        &self,
    ) -> (
        AgentWebSocketConfig,
        STTWebSocketConfig,
        Option<TTSWebSocketConfig>,
    ) {
        let agent = AgentWebSocketConfig {
            id: format!("prompt:{}", self.prompt_name),
            version: Some(self.version),
            variables: self.variables.clone(),
            text_only: Some(self.text_only),
        };
        let mut stt = crate::handlers::ws::config::default_agent_stt_config();
        stt.sample_rate = GA_RATE;
        if self.allows("stt.language")
            && let Some(l) = self.language.clone()
        {
            stt.language = l;
        }
        let tts = serde_json::from_value::<TTSWebSocketConfig>(json!({
            "audio_format": "linear16",
            "sample_rate": GA_RATE,
            "client_playback_rate": GA_RATE,
            "voice_id": if self.allows("tts.voice") { self.voice.clone() } else { None },
            "speaking_rate": if self.allows("tts.speed") { self.speed } else { None },
        }))
        .ok();
        if self.eagerness_changed && self.allows("turn_detection.eagerness") {
            stt.turn_detection = Some(crate::handlers::ws::config::TurnDetectionWsConfig {
                enabled: true,
                threshold: crate::handlers::ws::bud_legs::eagerness_threshold(&self.eagerness),
                eager: false,
            });
        }
        (agent, stt, tts)
    }

    // ---------------------------------------------------------------------------------------
    // Client -> session
    // ---------------------------------------------------------------------------------------

    /// One GA client event: events for the client now, and acts for the session.
    pub fn client(&mut self, raw: &str) -> (Vec<String>, Vec<ClientAct>) {
        let mut out = Vec::new();
        let mut acts = Vec::new();
        let Ok(ev) = serde_json::from_str::<Value>(raw) else {
            out.push(gateway_error(
                "invalid_event",
                "The event is not JSON.",
                None,
                None,
            ));
            return (out, acts);
        };
        let kind = ev
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let eid = ev
            .get("event_id")
            .and_then(Value::as_str)
            .map(str::to_string);
        let eid = eid.as_deref();
        match kind.as_str() {
            "session.update" => self.session_update(&ev, eid, &mut out, &mut acts),
            "input_audio_buffer.append" => {
                match ev
                    .get("audio")
                    .and_then(Value::as_str)
                    .map(|a| BASE64.decode(a))
                {
                    Some(Ok(pcm)) if pcm.len() % 2 == 0 => {
                        if !self.started {
                            acts.push(ClientAct::Start);
                        }
                        acts.push(ClientAct::Audio(Bytes::from(pcm)));
                    }
                    _ => out.push(gateway_error(
                        "invalid_event",
                        "`audio` must be base64 PCM16 (audio/pcm, 24 kHz).",
                        Some("audio"),
                        eid,
                    )),
                }
            }
            "input_audio_buffer.commit" => {
                let item = self.user_item(&mut out);
                out.push(event(
                    "input_audio_buffer.committed",
                    json!({"previous_item_id": Value::Null, "item_id": item}),
                ));
                if self.manual {
                    acts.push(ClientAct::Commit);
                }
            }
            "input_audio_buffer.clear" => {
                acts.push(ClientAct::Clear);
                out.push(event("input_audio_buffer.cleared", json!({})));
            }
            "conversation.item.create" => self.item_create(&ev, eid, &mut out, &mut acts),
            "response.create" => {
                let r = ev.get("response").cloned().unwrap_or(Value::Null);
                if r.get("instructions")
                    .and_then(Value::as_str)
                    .is_some_and(|s| !s.is_empty())
                {
                    out.push(refuse(
                        "response.instructions",
                        "A voice agent's instructions are its own; per-response instructions are not accepted.",
                        eid,
                    ));
                    return (out, acts);
                }
                if r.get("tools")
                    .and_then(Value::as_array)
                    .is_some_and(|t| !t.is_empty())
                {
                    out.push(refuse(
                        "response.tools",
                        "A voice agent's tools are its own (NG-2).",
                        eid,
                    ));
                    return (out, acts);
                }
                if !self.started {
                    acts.push(ClientAct::Start);
                }
                match self.typed.take() {
                    Some(text) => acts.push(ClientAct::Turn(text)),
                    None if self.manual => acts.push(ClientAct::Commit),
                    None => {}
                }
            }
            "response.cancel" => acts.push(ClientAct::Cancel),
            "conversation.item.truncate" => {
                let ms = ev.get("audio_end_ms").and_then(Value::as_u64).unwrap_or(0);
                acts.push(ClientAct::Truncate(ms));
                if let Some(item) = ev.get("item_id").and_then(Value::as_str) {
                    out.push(event(
                        "conversation.item.truncated",
                        json!({"item_id": item, "content_index": 0, "audio_end_ms": ms}),
                    ));
                }
            }
            "bud.session.auth" => match ev.get("token").and_then(Value::as_str) {
                Some(t) if !t.is_empty() => acts.push(ClientAct::Auth(t.to_string())),
                _ => out.push(gateway_error(
                    "invalid_event",
                    "`token` is required.",
                    Some("token"),
                    eid,
                )),
            },
            "conversation.item.retrieve" | "conversation.item.delete" => {
                out.push(refuse(
                    &kind,
                    "A voice agent's conversation lives in budprompt; items cannot be retrieved or deleted here.",
                    eid,
                ));
            }
            other => out.push(refuse(
                other,
                &format!("`{other}` is not supported on a voice-agent session."),
                eid,
            )),
        }
        (out, acts)
    }

    fn session_update(
        &mut self,
        ev: &Value,
        eid: Option<&str>,
        out: &mut Vec<String>,
        acts: &mut Vec<ClientAct>,
    ) {
        let empty = Map::new();
        let s = ev
            .get("session")
            .and_then(Value::as_object)
            .unwrap_or(&empty);
        if s.get("type")
            .and_then(Value::as_str)
            .is_some_and(|t| t != "realtime")
        {
            out.push(refuse(
                "session.type",
                "A voice agent is a `realtime` session.",
                eid,
            ));
            return;
        }
        if s.get("instructions")
            .and_then(Value::as_str)
            .is_some_and(|i| !i.trim().is_empty())
        {
            out.push(refuse(
                "session.instructions",
                "A voice agent's instructions are its own (set in the agent); `instructions` is ignored.",
                eid,
            ));
        }
        if s.get("tools")
            .and_then(Value::as_array)
            .is_some_and(|t| !t.is_empty())
        {
            out.push(refuse(
                "session.tools",
                "A voice agent's tools are its own (MCP, native, A2A); client tools are not supported (NG-2).",
                eid,
            ));
        }
        if let Some(p) = s.get("prompt").filter(|p| !p.is_null()) {
            let id = p.get("id").and_then(Value::as_str).unwrap_or_default();
            let id = id.strip_prefix("prompt:").unwrap_or(id);
            let version = p.get("version").and_then(|v| {
                v.as_str()
                    .map(str::to_string)
                    .or_else(|| v.as_i64().map(|n| n.to_string()))
            });
            if !id.is_empty() && id != self.prompt_name {
                out.push(refuse(
                    "session.prompt.id",
                    "The session's agent is fixed by `?model`.",
                    eid,
                ));
                return;
            }
            if version
                .as_deref()
                .is_some_and(|v| v != self.version.to_string())
            {
                out.push(refuse(
                    "session.prompt.version",
                    "The session's agent version is pinned at connect.",
                    eid,
                ));
                return;
            }
            if let Some(vars) = p.get("variables").and_then(Value::as_object) {
                if self.turns_ran {
                    out.push(refuse(
                        "session.prompt.variables",
                        "Variables are fixed once the conversation has started.",
                        eid,
                    ));
                } else {
                    self.variables = Some(vars.clone());
                    acts.push(ClientAct::Variables(vars.clone()));
                }
            }
        }
        if let Some(m) = s.get("output_modalities").and_then(Value::as_array) {
            let text_only = m.iter().any(|x| x == "text") && !m.iter().any(|x| x == "audio");
            if text_only != self.text_only {
                if self.started {
                    out.push(refuse(
                        "session.output_modalities",
                        "Output modalities are set when the session starts.",
                        eid,
                    ));
                } else {
                    self.text_only = text_only;
                    acts.push(ClientAct::TextOnly(text_only));
                }
            }
        }
        if let Some(audio) = s.get("audio").and_then(Value::as_object) {
            for side in ["input", "output"] {
                if let Some(f) = audio
                    .get(side)
                    .and_then(|x| x.get("format"))
                    .filter(|f| !f.is_null())
                {
                    let ok = f
                        .get("type")
                        .and_then(Value::as_str)
                        .is_none_or(|t| t == "audio/pcm")
                        && f.get("rate")
                            .is_none_or(|r| r.as_u64() == Some(GA_RATE as u64));
                    if !ok {
                        out.push(refuse(
                            &format!("session.audio.{side}.format"),
                            "Voice agents speak audio/pcm at 24 kHz.",
                            eid,
                        ));
                    }
                }
            }
            if let Some(input) = audio.get("input").and_then(Value::as_object) {
                if let Some(td) = input.get("turn_detection") {
                    let manual = td.is_null();
                    if manual != self.manual {
                        self.manual = manual;
                        acts.push(ClientAct::Manual(manual));
                    }
                    if let Some(eagerness) = td.get("eagerness").and_then(Value::as_str) {
                        const PARAM: &str = "session.audio.input.turn_detection.eagerness";
                        if !matches!(eagerness, "auto" | "low" | "medium" | "high") {
                            out.push(gateway_error(
                                "invalid_value",
                                "Eagerness is one of auto, low, medium or high.",
                                Some(PARAM),
                                eid,
                            ));
                        } else if eagerness != self.eagerness {
                            if !self.semantic {
                                out.push(refuse(
                                    PARAM,
                                    "This agent does not use semantic turn-taking, so eagerness does not apply.",
                                    eid,
                                ));
                            } else if self.allows("turn_detection.eagerness") && !self.started {
                                self.eagerness = eagerness.to_string();
                                self.eagerness_changed = true;
                            } else {
                                out.push(refuse(
                                    PARAM,
                                    "This agent's turn-taking eagerness is fixed by the agent (or the session has started).",
                                    eid,
                                ));
                            }
                        }
                    }
                }
                if let Some(lang) = input
                    .get("transcription")
                    .and_then(|t| t.get("language"))
                    .and_then(Value::as_str)
                {
                    if self.allows("stt.language") && !self.started {
                        self.language = Some(lang.to_string());
                    } else if Some(lang) != self.language.as_deref() {
                        out.push(refuse(
                            "session.audio.input.transcription.language",
                            "This agent's language is fixed.",
                            eid,
                        ));
                    }
                }
            }
            if let Some(output) = audio.get("output").and_then(Value::as_object) {
                if let Some(voice) = output.get("voice").and_then(Value::as_str)
                    && Some(voice) != self.voice.as_deref()
                {
                    if self.allows("tts.voice") && !self.started {
                        self.voice = Some(voice.to_string());
                    } else {
                        out.push(refuse(
                            "session.audio.output.voice",
                            "This agent's voice is fixed by the agent (or the session has started).",
                            eid,
                        ));
                    }
                }
                if let Some(speed) = output.get("speed").and_then(Value::as_f64)
                    && Some(speed) != self.speed
                {
                    if self.allows("tts.speed") && !self.started {
                        self.speed = Some(speed);
                    } else {
                        out.push(refuse(
                            "session.audio.output.speed",
                            "This agent's speaking speed is fixed.",
                            eid,
                        ));
                    }
                }
            }
        }
        if !self.started {
            acts.push(ClientAct::Start);
        }
        out.push(event(
            "session.updated",
            json!({"session": self.session_object()}),
        ));
    }

    fn item_create(
        &mut self,
        ev: &Value,
        eid: Option<&str>,
        out: &mut Vec<String>,
        acts: &mut Vec<ClientAct>,
    ) {
        let item = ev.get("item").cloned().unwrap_or(Value::Null);
        let is_user_message = item
            .get("type")
            .and_then(Value::as_str)
            .is_none_or(|t| t == "message")
            && item.get("role").and_then(Value::as_str) == Some("user");
        if !is_user_message {
            out.push(refuse(
                "item.type",
                "A voice-agent session accepts typed user messages only (no function_call_output: NG-2).",
                eid,
            ));
            return;
        }
        let text: String = item
            .get("content")
            .and_then(Value::as_array)
            .map(|parts| {
                parts
                    .iter()
                    .filter_map(|p| p.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();
        if text.trim().is_empty() {
            out.push(gateway_error(
                "invalid_event",
                "The message has no text.",
                Some("item.content"),
                eid,
            ));
            return;
        }
        let id = item
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| new_id("item"));
        let added = json!({"id": id, "object": "realtime.item", "type": "message", "role": "user",
                           "status": "completed", "content": [{"type": "input_text", "text": text}]});
        out.push(event(
            "conversation.item.added",
            json!({"previous_item_id": self.last_item_id, "item": added}),
        ));
        out.push(event("conversation.item.done", json!({"item": added})));
        self.last_item_id = Some(id);
        self.typed = Some(match self.typed.take() {
            Some(prev) => format!("{prev} {text}"),
            None => text,
        });
        if !self.started {
            acts.push(ClientAct::Start);
        }
    }

    // ---------------------------------------------------------------------------------------
    // Session -> client
    // ---------------------------------------------------------------------------------------

    fn user_item(&mut self, out: &mut Vec<String>) -> String {
        if let Some(id) = &self.user.id {
            return id.clone();
        }
        let id = new_id("item");
        self.user.started_ms = self.clock.elapsed().as_millis() as u64;
        let item = json!({"id": id, "object": "realtime.item", "type": "message", "role": "user",
                          "status": "in_progress", "content": [{"type": "input_audio", "transcript": Value::Null}]});
        out.push(event(
            "conversation.item.added",
            json!({"previous_item_id": self.last_item_id, "item": item}),
        ));
        out.push(event(
            "input_audio_buffer.speech_started",
            json!({"audio_start_ms": self.user.started_ms, "item_id": id}),
        ));
        self.last_item_id = Some(id.clone());
        self.user.id = Some(id.clone());
        id
    }

    fn close_user(&mut self, transcript: &str, out: &mut Vec<String>) {
        let Some(id) = self.user.id.take() else {
            return;
        };
        let now = self.clock.elapsed().as_millis() as u64;
        out.push(event(
            "input_audio_buffer.speech_stopped",
            json!({"audio_end_ms": now, "item_id": id}),
        ));
        out.push(event(
            "input_audio_buffer.committed",
            json!({"previous_item_id": Value::Null, "item_id": id}),
        ));
        out.push(event(
            "conversation.item.input_audio_transcription.completed",
            json!({"item_id": id, "content_index": 0, "transcript": transcript}),
        ));
        let item = json!({"id": id, "object": "realtime.item", "type": "message", "role": "user",
                          "status": "completed", "content": [{"type": "input_audio", "transcript": transcript}]});
        out.push(event("conversation.item.done", json!({"item": item})));
    }

    fn start_response(&mut self, turn: u64, greeting: bool, out: &mut Vec<String>) {
        let id = new_id("resp");
        out.push(event(
            "response.created",
            json!({"response": {"id": id, "object": "realtime.response", "status": "in_progress",
                                "output": [], "usage": Value::Null,
                                "metadata": {"bud_agent": self.prompt_name, "bud_turn": turn}}}),
        ));
        self.response = Some(Resp {
            id,
            turn,
            greeting,
            item_id: None,
            item_index: 0,
            next_index: 0,
            transcript: String::new(),
            output: Vec::new(),
            tools: HashMap::new(),
            bud_response_id: None,
        });
    }

    /// The assistant message item of the response in flight, announced on its first output.
    fn message_item(&mut self, out: &mut Vec<String>) -> Option<(String, String, u64)> {
        let last = self.last_item_id.clone();
        let text_only = self.text_only;
        let r = self.response.as_mut()?;
        if let Some(id) = &r.item_id {
            return Some((r.id.clone(), id.clone(), r.item_index));
        }
        let item_id = new_id("item");
        r.item_index = r.next_index;
        r.next_index += 1;
        r.item_id = Some(item_id.clone());
        let part = if text_only {
            json!({"type": "text", "text": ""})
        } else {
            json!({"type": "audio", "transcript": ""})
        };
        let item = json!({"id": item_id, "object": "realtime.item", "type": "message", "role": "assistant",
                          "status": "in_progress", "content": []});
        out.push(event(
            "response.output_item.added",
            json!({"response_id": r.id, "output_index": r.item_index, "item": item}),
        ));
        out.push(event(
            "conversation.item.added",
            json!({"previous_item_id": last, "item": item}),
        ));
        out.push(event(
            "response.content_part.added",
            json!({"response_id": r.id, "item_id": item_id, "output_index": r.item_index, "content_index": 0, "part": part}),
        ));
        let (rid, idx) = (r.id.clone(), r.item_index);
        self.items.insert(r.turn, item_id.clone());
        self.last_item_id = Some(item_id.clone());
        Some((rid, item_id, idx))
    }

    fn finish_response(&mut self, status: &str, usage: Option<&Value>, out: &mut Vec<String>) {
        let text_only = self.text_only;
        let Some(mut r) = self.response.take() else {
            return;
        };
        // Tools still open at the end did not complete.
        let open: Vec<(String, (u64, Value))> = r.tools.drain().collect();
        for (_id, (idx, mut item)) in open {
            item["status"] = json!("incomplete");
            out.push(event(
                "response.output_item.done",
                json!({"response_id": r.id, "output_index": idx, "item": item}),
            ));
            r.output.push(item);
        }
        if let Some(item_id) = r.item_id.clone() {
            let item_status = if status == "completed" {
                "completed"
            } else {
                "incomplete"
            };
            let (content, part) = if text_only {
                (
                    json!([{"type": "output_text", "text": r.transcript}]),
                    json!({"type": "text", "text": r.transcript}),
                )
            } else {
                (
                    json!([{"type": "output_audio", "transcript": r.transcript}]),
                    json!({"type": "audio", "transcript": r.transcript}),
                )
            };
            let base = |extra: Value| {
                let mut m = json!({"response_id": r.id, "item_id": item_id, "output_index": r.item_index, "content_index": 0});
                if let (Some(b), Some(e)) = (m.as_object_mut(), extra.as_object()) {
                    b.extend(e.clone());
                }
                m
            };
            if text_only {
                out.push(event(
                    "response.output_text.done",
                    base(json!({"text": r.transcript})),
                ));
            } else {
                out.push(event("response.output_audio.done", base(json!({}))));
                out.push(event(
                    "response.output_audio_transcript.done",
                    base(json!({"transcript": r.transcript})),
                ));
            }
            out.push(event(
                "response.content_part.done",
                base(json!({"part": part})),
            ));
            let item = json!({"id": item_id, "object": "realtime.item", "type": "message", "role": "assistant",
                              "status": item_status, "content": content});
            out.push(event(
                "response.output_item.done",
                json!({"response_id": r.id, "output_index": r.item_index, "item": item}),
            ));
            out.push(event("conversation.item.done", json!({"item": item})));
            r.output.push(item);
        }
        let mut response = json!({"id": r.id, "object": "realtime.response", "status": status,
                                  "output": r.output, "usage": usage.map(ga_usage),
                                  "metadata": {"bud_agent": self.prompt_name, "bud_turn": r.turn,
                                               "bud_response_id": r.bud_response_id}});
        if status == "cancelled" {
            response["status_details"] = json!({"type": "cancelled", "reason": "turn_detected"});
        } else if status == "failed" {
            response["status_details"] = json!({"type": "failed"});
        } else if status == "incomplete" {
            response["status_details"] = json!({"type": "incomplete", "reason": "content_filter"});
        }
        if r.greeting {
            response["metadata"]["bud_greeting"] = json!(true);
        }
        out.push(event("response.done", json!({"response": response})));
    }

    /// A message from the in-process `/ws` session. `Err(close)` ends the GA session.
    pub fn core(&mut self, msg: &OutgoingMessage) -> Vec<String> {
        let mut out = Vec::new();
        match msg {
            OutgoingMessage::STTResult {
                transcript,
                is_final,
                ..
            } => {
                if transcript.trim().is_empty() {
                    return out;
                }
                let item = self.user_item(&mut out);
                if *is_final {
                    out.push(event(
                        "conversation.item.input_audio_transcription.delta",
                        json!({"item_id": item, "content_index": 0, "delta": format!("{} ", transcript.trim())}),
                    ));
                }
            }
            OutgoingMessage::AgentResponseStarted {
                turn_index,
                kind,
                input,
            } => {
                if kind == "agent" {
                    self.turns_ran = true;
                    if let Some(text) = input.as_deref() {
                        if self.user.id.is_some() {
                            self.close_user(text, &mut out);
                        }
                    }
                }
                if self.response.is_some() {
                    self.finish_response("cancelled", None, &mut out);
                }
                self.start_response(*turn_index, kind == "greeting", &mut out);
            }
            OutgoingMessage::AgentResponseCreated {
                turn_index,
                response_id,
            } => {
                if let Some(r) = self.response.as_mut().filter(|r| r.turn == *turn_index) {
                    r.bud_response_id = Some(response_id.clone());
                }
            }
            OutgoingMessage::AssistantTranscript { turn_index, delta } => {
                if self.response.as_ref().is_none_or(|r| r.turn != *turn_index) {
                    return out;
                }
                let Some((rid, iid, idx)) = self.message_item(&mut out) else {
                    return out;
                };
                if let Some(r) = self.response.as_mut() {
                    r.transcript.push_str(delta);
                }
                let kind = if self.text_only {
                    "response.output_text.delta"
                } else {
                    "response.output_audio_transcript.delta"
                };
                out.push(event(
                    kind,
                    json!({"response_id": rid, "item_id": iid, "output_index": idx, "content_index": 0, "delta": delta}),
                ));
            }
            OutgoingMessage::AgentTool {
                turn_index,
                item_id,
                name,
                status,
            } => {
                let Some(r) = self.response.as_mut().filter(|r| r.turn == *turn_index) else {
                    return out;
                };
                if status == "in_progress" {
                    let idx = r.next_index;
                    r.next_index += 1;
                    let item = json!({"id": item_id, "object": "realtime.item", "type": "mcp_call",
                                      "name": name, "server_label": "bud", "arguments": "", "status": "in_progress"});
                    out.push(event(
                        "response.output_item.added",
                        json!({"response_id": r.id, "output_index": idx, "item": item}),
                    ));
                    out.push(event(
                        "response.mcp_call.in_progress",
                        json!({"output_index": idx, "item_id": item_id}),
                    ));
                    r.tools.insert(item_id.clone(), (idx, item));
                } else if let Some((idx, mut item)) = r.tools.remove(item_id) {
                    let ok = status == "completed";
                    item["status"] = json!(if ok { "completed" } else { "failed" });
                    let kind = if ok {
                        "response.mcp_call.completed"
                    } else {
                        "response.mcp_call.failed"
                    };
                    out.push(event(
                        kind,
                        json!({"output_index": idx, "item_id": item_id}),
                    ));
                    out.push(event(
                        "response.output_item.done",
                        json!({"response_id": r.id, "output_index": idx, "item": item}),
                    ));
                    r.output.push(item);
                }
            }
            OutgoingMessage::AgentOutput {
                response_id,
                output,
                ..
            } => {
                let rid = self.response.as_ref().map(|r| r.id.clone());
                out.push(event(
                    "bud.agent.output",
                    json!({"response_id": rid, "bud_response_id": response_id, "output": output}),
                ));
            }
            OutgoingMessage::AgentResponseDone {
                turn_index,
                status,
                usage,
                ..
            } => {
                if self
                    .response
                    .as_ref()
                    .is_some_and(|r| r.turn == *turn_index)
                {
                    self.finish_response(status, usage.as_ref(), &mut out);
                }
            }
            OutgoingMessage::AgentTruncated {
                turn_index,
                audio_end_ms,
                ..
            } => {
                if let Some(item) = self.items.get(turn_index) {
                    out.push(event(
                        "conversation.item.truncated",
                        json!({"item_id": item, "content_index": 0, "audio_end_ms": audio_end_ms}),
                    ));
                }
            }
            OutgoingMessage::AgentError { code, message } => {
                out.push(super::policy::error_event(
                    &next_event_id(),
                    error_kind(code),
                    code,
                    message,
                    None,
                    None,
                ));
            }
            OutgoingMessage::Error { message } => {
                // Leg refusals and setup failures are `code: message`.
                let (code, text) = match message.split_once(": ") {
                    Some((c, t)) if c.chars().all(|ch| ch.is_ascii_lowercase() || ch == '_') => {
                        (c.to_string(), t.to_string())
                    }
                    _ => ("session_error".to_string(), message.clone()),
                };
                out.push(super::policy::error_event(
                    &next_event_id(),
                    error_kind(&code),
                    &code,
                    &text,
                    None,
                    None,
                ));
            }
            OutgoingMessage::Authenticated { .. } => {
                out.push(event("bud.session.authenticated", json!({})));
            }
            _ => {}
        }
        out
    }

    /// TTS audio (24 kHz PCM16) for the response in flight.
    pub fn audio(&mut self, pcm: &[u8]) -> Vec<String> {
        let mut out = Vec::new();
        if self.text_only || pcm.is_empty() || self.response.is_none() {
            return out;
        }
        let Some((rid, iid, idx)) = self.message_item(&mut out) else {
            return out;
        };
        out.push(event(
            "response.output_audio.delta",
            json!({"response_id": rid, "item_id": iid, "output_index": idx, "content_index": 0, "delta": BASE64.encode(pcm)}),
        ));
        out
    }
}

// =============================================================================================
// The socket loop
// =============================================================================================

async fn send(client: &mpsc::Sender<Outbound>, events: Vec<String>) -> bool {
    for e in events {
        if client
            .send(Outbound::Frame(Message::Text(e.into())))
            .await
            .is_err()
        {
            return false;
        }
    }
    true
}

async fn close(client: &mpsc::Sender<Outbound>, code: u16, reason: &str, error: Option<String>) {
    let _ = client
        .send(Outbound::Close {
            error,
            code,
            reason: reason.to_string(),
        })
        .await;
}

/// Run one voice-agent session over the GA protocol.
pub async fn run(
    state: Arc<AppState>,
    p: PreparedCascade,
    socket: WebSocket,
    slot: Option<ConnectionSlot>,
) {
    let _slot = slot;
    let (sink, mut stream) = socket.split();
    let (client, writer) = spawn_writer(sink, CLIENT_QUEUE);
    let session_id = format!("sess_bud_{}", uuid::Uuid::new_v4().simple());
    let mut ga = CascadeGa::new(&session_id, &p.model, &p.agent);
    info!(session = %session_id, agent = %p.agent.prompt_name, version = p.agent.version, "voice agent realtime session opened");
    if !send(&client, vec![ga.session_created()]).await {
        return;
    }

    // The in-process `/ws` session.
    let auth = match state.bud_mode.as_ref() {
        Some(bud) => bud.authenticate(&p.credential).await.unwrap_or_default(),
        None => Default::default(),
    };
    let conn = Arc::new(RwLock::new(ConnectionState::with_auth(auth)));
    {
        let mut c = conn.write().await;
        c.credential = Some(crate::auth::SessionCredential::new(p.credential.clone()));
        c.caller_check = Some(p.caller.check.clone());
    }
    let (core_tx, mut core_rx) = mpsc::channel::<MessageRoute>(CORE_QUEUE);
    let grace = tokio::time::sleep(START_GRACE);
    tokio::pin!(grace);
    let mut held: Vec<Bytes> = Vec::new();
    let mut held_bytes = 0usize;
    let mut pending: Vec<ClientAct> = Vec::new();

    let end: (u16, &'static str, Option<String>) = loop {
        tokio::select! {
            biased;
            _ = state.shutdown.cancelled() => {
                break (1012, "server_shutdown", Some(gateway_error("server_shutdown", "The gateway is restarting; reconnect.", None, None)));
            }
            route = core_rx.recv() => match route {
                None => break (1011, "session_ended", None),
                Some(MessageRoute::Outgoing(msg)) => {
                    if !send(&client, ga.core(&msg)).await { break (1000, "client_gone", None); }
                }
                Some(MessageRoute::Binary(pcm)) => {
                    if !send(&client, ga.audio(&pcm)).await { break (1000, "client_gone", None); }
                }
                Some(MessageRoute::Close) => break (1000, "session_closed", None),
                Some(MessageRoute::CloseWith { code, reason }) => {
                    let reason: &'static str = match reason.as_str() {
                        "session_revoked" => "session_revoked",
                        "idle_timeout" => "idle_timeout",
                        "session_limit" => "session_limit",
                        _ => "session_closed",
                    };
                    break (code, reason, None);
                }
            },
            _ = &mut grace, if !ga.is_started() => {
                if !start_core(&state, &conn, &core_tx, &mut ga, &mut held, &mut pending).await {
                    break (1011, "setup_failed", None);
                }
            }
            msg = stream.next() => match msg {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => break (1000, "client_closed", None),
                Some(Ok(Message::Text(text))) => {
                    let (events, acts) = ga.client(&text);
                    if !send(&client, events).await { break (1000, "client_gone", None); }
                    let mut start = false;
                    for act in acts {
                        match act {
                            ClientAct::Start => start = true,
                            ClientAct::Audio(pcm) if !ga.is_started() || conn.read().await.voice_manager.is_none() => {
                                if held_bytes + pcm.len() <= MAX_HELD_AUDIO {
                                    held_bytes += pcm.len();
                                    held.push(pcm);
                                }
                            }
                            other if !ga.is_started() => pending.push(other),
                            other => apply(&state, &conn, &core_tx, other).await,
                        }
                    }
                    if start && !ga.is_started()
                        && !start_core(&state, &conn, &core_tx, &mut ga, &mut held, &mut pending).await
                    {
                        break (1011, "setup_failed", None);
                    }
                }
                Some(Ok(Message::Binary(_))) => {
                    let _ = send(&client, vec![gateway_error("invalid_event", "Send audio as input_audio_buffer.append events.", None, None)]).await;
                }
                Some(Ok(_)) => {}
            },
        }
    };

    debug!(session = %session_id, reason = end.1, "voice agent realtime session ending");
    // Flush what the core already said (e.g. the error before a revocation close).
    while let Ok(route) = core_rx.try_recv() {
        if let MessageRoute::Outgoing(msg) = route {
            let _ = send(&client, ga.core(&msg)).await;
        }
    }
    close(&client, end.0, end.1, end.2).await;
    drop(client);
    crate::handlers::ws::handler::teardown_session(&conn, &state).await;
    let _ = tokio::time::timeout(Duration::from_secs(2), writer).await;
    info!(session = %session_id, reason = end.1, "voice agent realtime session closed");
}

/// Start the in-process `/ws` agent session with the GA session's settings, then replay what the
/// client sent while it connected. `false` when the session could not start (the client was told why).
async fn start_core(
    state: &Arc<AppState>,
    conn: &Arc<RwLock<ConnectionState>>,
    core_tx: &mpsc::Sender<MessageRoute>,
    ga: &mut CascadeGa,
    held: &mut Vec<Bytes>,
    pending: &mut Vec<ClientAct>,
) -> bool {
    ga.mark_started();
    let (agent, stt, tts) = ga.core_config();
    let keep = crate::handlers::ws::config_handler::handle_config_message(
        Some(ga.session_id.clone()),
        Some(true),
        Some(stt),
        tts,
        None,
        None,
        None,
        Some(agent),
        None,
        conn,
        core_tx,
        state,
    )
    .await;
    let engine = conn.read().await.agent.clone();
    let Some(engine) = engine.filter(|_| keep) else {
        warn!("voice agent realtime session could not start its agent");
        // The core sent its refusal as `error` (+ a close); the loop relays and ends.
        return keep && conn.read().await.voice_manager.is_some();
    };
    if ga.manual {
        engine.set_manual(true);
    }
    for pcm in held.drain(..) {
        crate::handlers::ws::audio_handler::handle_audio_message(pcm, conn, core_tx).await;
    }
    for act in pending.drain(..) {
        apply(state, conn, core_tx, act).await;
    }
    true
}

/// Apply one client act to the running agent session.
async fn apply(
    state: &Arc<AppState>,
    conn: &Arc<RwLock<ConnectionState>>,
    core_tx: &mpsc::Sender<MessageRoute>,
    act: ClientAct,
) {
    let engine = conn.read().await.agent.clone();
    match act {
        ClientAct::Start => {}
        ClientAct::Audio(pcm) => {
            crate::handlers::ws::audio_handler::handle_audio_message(pcm, conn, core_tx).await;
        }
        ClientAct::Commit => {
            let _ = crate::handlers::ws::audio_handler::handle_audio_end(conn, core_tx).await;
            if let Some(engine) = engine {
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    engine.commit_input().await;
                });
            }
        }
        ClientAct::Clear => {
            if let Some(engine) = engine {
                engine.clear_input();
            }
        }
        ClientAct::Turn(text) => {
            if let Some(engine) = engine {
                engine.start_turn(text).await;
            }
        }
        ClientAct::Cancel => {
            if let Some(engine) = engine {
                engine.cancel_response().await;
            }
        }
        ClientAct::Truncate(ms) => {
            if let Some(engine) = engine {
                engine.truncate_at(ms);
            }
        }
        ClientAct::Variables(vars) => {
            if let Some(engine) = engine
                && let Err(e) = engine.set_variables(vars)
            {
                let _ = core_tx
                    .send(MessageRoute::Outgoing(OutgoingMessage::AgentError {
                        code: "invalid_variables".into(),
                        message: e.into(),
                    }))
                    .await;
            }
        }
        ClientAct::TextOnly(on) => {
            if let Some(engine) = engine {
                engine.set_text_only(on);
            }
        }
        ClientAct::Manual(on) => {
            if let Some(engine) = engine {
                engine.set_manual(on);
            }
        }
        ClientAct::Auth(token) => {
            crate::handlers::ws::processor::handle_incoming_message(
                IncomingMessage::Auth { token },
                conn,
                core_tx,
                state,
            )
            .await;
        }
    }
}

#[cfg(test)]
#[path = "cascade_tests.rs"]
mod tests;
