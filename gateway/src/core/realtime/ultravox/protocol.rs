//! Ultravox Realtime [`RealtimeProtocol`] — the FIRST `RestThenWebSocket` provider
//! on the S2S scaffold (a REST "create call" handshake mints a single-use
//! `joinUrl`, then a plain WebSocket runs the session).
//!
//! Ultravox is the hosted realtime API for the open-weight Ultravox speech-to-
//! speech model (`fixie-ai/ultravox`): a single WebSocket runs the whole loop
//! server-side (STT + LLM + TTS) with server VAD + turn-taking + barge-in. Like
//! Deepgram (and unlike the OpenAI family), user mic audio is sent as RAW PCM
//! BINARY frames and agent TTS comes back as RAW PCM BINARY frames; only the
//! transcript / state / barge-in / tool control messages are JSON.
//!
//! What makes Ultravox unique on the scaffold is the CONNECT handshake: you can't
//! open the WS directly — you first `POST https://api.ultravox.ai/api/calls`
//! (auth `X-API-Key`) with the call config, and the response carries a pre-authed
//! `joinUrl` you then connect with no extra auth. That two-step lives entirely in
//! the [`RestHandshakeWsTransportFactory`](crate::core::realtime::scaffold::RestHandshakeWsTransportFactory)
//! (selected via [`transport_factory`](RealtimeProtocol::transport_factory)); the
//! generic driver is unchanged. Because the call is fully configured in the REST
//! body, [`build_session_config`](RealtimeProtocol::build_session_config) is EMPTY.
//!
//! # API Reference (authoritative wire; verified against Pipecat's
//! `services/ultravox/llm.py` — NO Ultravox key held, so unit-validated)
//!
//! - Create call: `POST https://api.ultravox.ai/api/calls`, header
//!   `X-API-Key: <key>`, JSON body `{systemPrompt, model, voice,
//!   medium.serverWebSocket.{inputSampleRate,outputSampleRate}}` ⇒
//!   `{"joinUrl":"wss://…"}`.
//! - Then connect `joinUrl` (a plain WS, pre-authed, NO extra headers).
//! - Audio: RAW PCM BINARY both ways — input @ 16 kHz, output @ 24 kHz.
//! - Server JSON events: `transcript` (role agent/user, text/delta, final),
//!   `state`, `playback_clear_buffer` (barge-in clear), `client_tool_invocation`.
//! - Client JSON: `client_tool_result`, `user_text_message`.
//! - Docs: <https://docs.ultravox.ai/>

use bytes::Bytes;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::core::realtime::base::{
    FunctionCallRequest, RealtimeConfig, RealtimeError, RealtimeResponseOverride, RealtimeResult,
    RealtimeVerb, TranscriptRole, TurnDetectionConfig,
};
use crate::core::realtime::scaffold::{
    ConnectSpec, Inbound, OutFrame, ProtocolCaps, RealtimeProtocol, RealtimeTransportFactory,
    RestHandshakeWsTransportFactory, S2sEvent,
};

/// Ultravox REST "create call" endpoint. A `POST` here (with `X-API-Key`) mints a
/// single-use `joinUrl`; the WS session connects that url.
pub(crate) const ULTRAVOX_CREATE_CALL_URL: &str = "https://api.ultravox.ai/api/calls";

/// The TOP-LEVEL field in the create-call response that holds the WS url.
const JOIN_URL_POINTER: &str = "joinUrl";

/// Default model — the open-weight Ultravox model on Fixie's hosted API.
const DEFAULT_MODEL: &str = "fixie-ai/ultravox";

/// Input PCM sample rate (mic audio sent to Ultravox). The API contract's
/// `medium.serverWebSocket.inputSampleRate`.
const INPUT_SAMPLE_RATE: u32 = 16_000;

/// Output PCM sample rate (agent TTS from Ultravox). The API contract's
/// `medium.serverWebSocket.outputSampleRate`.
const OUTPUT_SAMPLE_RATE: u32 = 24_000;

/// linear16 @ 24 kHz is mono 16-bit ⇒ 2 bytes/sample × 24 samples/ms = 48 B/ms.
/// Drives the barge-in truncate math on the OUTPUT rate.
const OUTPUT_BYTES_PER_MS: u64 = 48;

/// One typed outbound Ultravox wire message. The BINARY split (same shape as
/// Deepgram): `Json` ⇒ a text frame (tool result / user text control), `Audio` ⇒
/// a RAW PCM binary frame (user mic audio — NOT base64/JSON).
#[derive(Debug, Clone)]
pub enum UltravoxClientMsg {
    /// A JSON control message: `client_tool_result`, `user_text_message`.
    Json(Value),
    /// A raw PCM audio frame (the binary path).
    Audio(Bytes),
}

/// The Ultravox Realtime protocol. Stateless: the model/voice/instructions it
/// needs all come from `cfg` (re-passed into `connect_spec`), so it carries only
/// what `from_config` validated.
pub struct UltravoxProtocol {
    /// Cached for `provider_id`/diagnostics; the create-call model (defaulted).
    model: String,
    /// Whether the agent is currently speaking. Ultravox carries NO explicit
    /// turn-end on the wire — the `state` message transitioning speaking→not-
    /// speaking IS the response-completion signal (matching Pipecat), so we track
    /// the edge to fire exactly one `ResponseDone` (review wf_84b03504 #1). Per
    /// session (each session owns its protocol); single-writer (the supervisor
    /// recv loop), `Atomic` only to satisfy `Send + Sync` on the stateless trait.
    was_speaking: AtomicBool,
    /// Provider fix `ultravox_transcripts`: a transcript is `text` (the whole so far) or
    /// `delta` (what is new) of the message `ordinal`; this is each open message's text.
    transcripts: Option<Mutex<HashMap<i64, String>>>,
    /// The call's id (`call_started.callId`): with the fix `ultravox_prior_call` a re-dial
    /// creates its call with `?priorCallId=` so the new call keeps the conversation.
    call_id: Mutex<Option<String>>,
    /// Provider fix `setup_once`: the call is configured at creation only.
    setup_once: bool,
}

/// `modelToolName` "Must match ^[a-zA-Z0-9_-]{1,64}$"
/// (<https://docs.ultravox.ai/api-reference/calls/calls-post>).
fn tool_name_ok(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

impl UltravoxProtocol {
    /// Resolve the model: configured, else the default `fixie-ai/ultravox`.
    fn resolve_model(cfg: &RealtimeConfig) -> String {
        let m = cfg.model.trim();
        if m.is_empty() {
            DEFAULT_MODEL.to_string()
        } else {
            m.to_string()
        }
    }

    /// Build the create-call JSON body from the generic config:
    /// `systemPrompt` ← instructions, `model` ← model (defaulted), `voice` ←
    /// voice, plus the `medium.serverWebSocket` input/output rates. Optional
    /// fields are only emitted when present (skip-if-none discipline — keeps the
    /// payload minimal and lets Ultravox apply its own defaults).
    pub fn create_call_body(&self, cfg: &RealtimeConfig) -> Value {
        let mut body = json!({
            "medium": {
                "serverWebSocket": {
                    "inputSampleRate": INPUT_SAMPLE_RATE,
                    "outputSampleRate": OUTPUT_SAMPLE_RATE,
                }
            },
        });
        // Fix `ultravox_default_model`: an unnamed model is left to Ultravox ("Currently
        // defaults to ultravox-v0.7"); today the legacy alias `fixie-ai/ultravox` is sent.
        if !(cfg.fix("ultravox_default_model") && cfg.model.trim().is_empty()) {
            body["model"] = json!(self.model);
        }
        if let Some(instr) = cfg.instructions.as_deref()
            && !instr.is_empty()
        {
            body["systemPrompt"] = json!(instr);
        }
        // Fix `ultravox_call_settings`: `temperature` ("between 0 and 1") and the end-of-turn
        // silence as `vadSettings.turnEndpointDelay` ("0.384s" by default).
        if cfg.fix("ultravox_call_settings") {
            if let Some(t) = cfg.temperature {
                body["temperature"] = json!(t.clamp(0.0, 1.0));
            }
            if let Some(TurnDetectionConfig::ServerVad {
                silence_duration_ms: Some(ms),
                ..
            }) = &cfg.turn_detection
            {
                body["vadSettings"] =
                    json!({ "turnEndpointDelay": format!("{:.3}s", f64::from(*ms) / 1000.0) });
            }
        }
        if let Some(voice) = cfg.voice.as_deref()
            && !voice.is_empty()
        {
            body["voice"] = json!(voice);
        }
        // Provider fix `ultravox_tools` (off until Ultravox's live probe; escape flag
        // `ultravox_omit_tools`): the session's functions are declared, as client tools the
        // gateway answers with `client_tool_result`. Without it they are silently dropped.
        if cfg.fix("ultravox_tools")
            && let Some(tools) = cfg.tools.as_ref().filter(|t| !t.is_empty())
        {
            body["selectedTools"] = tools.iter().map(selected_tool).collect();
        }
        body
    }
}

/// One function as a call-scoped client tool of the create-call body:
/// `{"temporaryTool": {modelToolName, description, dynamicParameters: [{name, location:
/// PARAMETER_LOCATION_BODY, schema, required}], client: {}}}`. Each top-level property of the
/// function's JSON schema is one body parameter (client tools take body parameters only).
/// Reference: <https://docs.ultravox.ai/api-reference/calls/calls-post> (`selectedTools`,
/// `SelectedTool.temporaryTool`, `BaseToolDefinition`, `DynamicParameter`).
fn selected_tool(tool: &crate::core::realtime::base::ToolDefinition) -> Value {
    let schema = tool.function.parameters.as_ref();
    let required: Vec<&str> = schema
        .and_then(|s| s.get("required"))
        .and_then(Value::as_array)
        .map(|r| r.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let parameters: Vec<Value> = schema
        .and_then(|s| s.get("properties"))
        .and_then(Value::as_object)
        .map(|props| {
            props
                .iter()
                .map(|(name, schema)| {
                    json!({
                        "name": name,
                        "location": "PARAMETER_LOCATION_BODY",
                        "schema": schema,
                        "required": required.contains(&name.as_str()),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let mut temporary = json!({
        "modelToolName": tool.function.name,
        "dynamicParameters": parameters,
        "client": {},
    });
    if let Some(description) = &tool.function.description {
        temporary["description"] = json!(description);
    }
    json!({ "temporaryTool": temporary })
}

impl UltravoxProtocol {
    /// Fix `ultravox_transcripts`: "text: The full text of the transcript so far"; "delta:
    /// The additional transcript text added since the last transcript message"; "Either this
    /// or delta will be set, but not both"; `ordinal` orders the call's messages
    /// (<https://docs.ultravox.ai/apps/datamessages>). An interim carries only what is new (the
    /// driver accumulates interims); a final carries the message's whole text.
    fn transcript(&self, value: &Value) -> Vec<S2sEvent> {
        let Some(open) = &self.transcripts else {
            return vec![S2sEvent::Ignore];
        };
        let ordinal = value
            .get("ordinal")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        let role = match value.get("role").and_then(Value::as_str) {
            Some("user") => TranscriptRole::User,
            _ => TranscriptRole::Assistant,
        };
        let is_final = value.get("final").and_then(Value::as_bool).unwrap_or(false);
        let Ok(mut open) = open.lock() else {
            return vec![S2sEvent::Ignore];
        };
        let so_far = open.entry(ordinal).or_default();
        let new = if let Some(text) = value.get("text").and_then(Value::as_str) {
            let new = text
                .strip_prefix(so_far.as_str())
                .unwrap_or(text)
                .to_string();
            *so_far = text.to_string();
            new
        } else {
            let delta = value.get("delta").and_then(Value::as_str).unwrap_or("");
            so_far.push_str(delta);
            delta.to_string()
        };
        let text = if is_final {
            open.remove(&ordinal).unwrap_or_default()
        } else if new.is_empty() {
            return vec![S2sEvent::Ignore];
        } else {
            new
        };
        vec![S2sEvent::Transcript {
            role,
            text,
            is_final,
            item_id: Some(ordinal.to_string()),
        }]
    }
}

impl RealtimeProtocol for UltravoxProtocol {
    type Wire = UltravoxClientMsg;

    fn from_config(cfg: &RealtimeConfig) -> RealtimeResult<Self> {
        // Ultravox auths the create-call via the `X-API-Key` header — a non-empty
        // key is required (the WS join url itself is pre-authed).
        if cfg.api_key.is_empty() {
            return Err(RealtimeError::AuthenticationFailed(
                "API key is required".to_string(),
            ));
        }
        Ok(Self {
            model: Self::resolve_model(cfg),
            was_speaking: AtomicBool::new(false),
            transcripts: cfg
                .fix("ultravox_transcripts")
                .then(|| Mutex::new(HashMap::new())),
            call_id: Mutex::new(None),
            setup_once: cfg.fix("setup_once"),
        })
    }

    /// THE UNIQUE BIT: Ultravox can't open the WS directly, so it opts into the
    /// REST-handshake-then-WebSocket factory instead of the default plain WS.
    fn transport_factory(&self) -> Arc<dyn RealtimeTransportFactory> {
        Arc::new(RestHandshakeWsTransportFactory)
    }

    fn provider_id(&self) -> &'static str {
        "ultravox"
    }

    fn caps(&self) -> ProtocolCaps {
        ProtocolCaps {
            // Ultravox runs server-side VAD + turn-taking, so it owns turns and
            // emits user-turn frames (final user transcript, barge-in clear).
            emits_user_turn_frames: true,
            output_bytes_per_ms: OUTPUT_BYTES_PER_MS,
            output_sample_rate: OUTPUT_SAMPLE_RATE,
            // No client-driven item truncation (barge-in is server-side via
            // `playback_clear_buffer`; the driver only clears local playback).
            supports_truncate: false,
            // No gateway-managed input buffer (raw binary audio is streamed).
            supports_input_buffer: false,
        }
    }

    fn connect_spec(&self, cfg: &RealtimeConfig) -> RealtimeResult<ConnectSpec> {
        // SERVER-CONFIG `realtime_endpoint_override` (proxy / self-hosted / gov-cloud
        // / local mock) WINS when set — and BYPASSES the REST create-call entirely:
        // Ultravox normally mints a single-use `joinUrl` via `POST /api/calls`, but a
        // proxy/mock can't faithfully serve that REST handshake, so we treat the
        // override as a pre-authed join url and open a PLAIN WebSocket to it directly
        // (NO create-call, NO extra headers — same as a real `joinUrl`). The entire
        // call config normally sent in the REST body is dropped on this path (the
        // override target owns session config). SSRF-safe: never client-settable.
        if let Some(join_url) = cfg
            .realtime_endpoint_override
            .as_deref()
            .map(str::trim)
            .filter(|s| s.starts_with("ws://") || s.starts_with("wss://"))
        {
            return Ok(ConnectSpec::WebSocket {
                url: join_url.to_string(),
                headers: Vec::new(),
            });
        }

        // Fix `ultravox_tools`: a tool name Ultravox refuses would fail the whole create-call;
        // the session is refused naming it instead.
        if cfg.fix("ultravox_tools")
            && let Some(bad) = cfg
                .tools
                .iter()
                .flatten()
                .find(|t| !tool_name_ok(&t.function.name))
        {
            return Err(RealtimeError::InvalidConfiguration(format!(
                "Ultravox tool names must match ^[a-zA-Z0-9_-]{{1,64}}$: {:?} does not",
                bad.function.name
            )));
        }
        // The REST create-call spec: POST the call config; the factory extracts
        // the `joinUrl` and connects it. Auth is the `X-API-Key` header on the
        // POST (NOT on the WS — the join url is pre-authed).
        let body = serde_json::to_string(&self.create_call_body(cfg))
            .map_err(|e| RealtimeError::SerializationError(e.to_string()))?;
        // Fix `ultravox_prior_call`: a re-dial continues the call it replaces: "the new call
        // will use the prior call's message history as its own initial_messages".
        let prior = self
            .call_id
            .lock()
            .ok()
            .and_then(|id| id.clone())
            .filter(|_| cfg.fix("ultravox_prior_call"));
        let create_url = match prior {
            Some(id) => format!(
                "{ULTRAVOX_CREATE_CALL_URL}?priorCallId={}",
                url::form_urlencoded::byte_serialize(id.as_bytes()).collect::<String>()
            ),
            None => ULTRAVOX_CREATE_CALL_URL.to_string(),
        };
        Ok(ConnectSpec::RestThenWebSocket {
            create_url,
            headers: vec![("X-API-Key".to_string(), cfg.api_key.clone())],
            body,
            join_url_pointer: JOIN_URL_POINTER.to_string(),
        })
    }

    fn build_session_config(
        &self,
        _cfg: &RealtimeConfig,
        _resumption: Option<&str>,
    ) -> Vec<Self::Wire> {
        // EMPTY: the entire call (model/voice/prompt/rates) is configured in the
        // REST create-call body, not via a WS message.
        Vec::new()
    }

    /// Fix `setup_once`: no data message changes the call's prompt, voice or tools (the
    /// client messages are `user_text_message`, `forced_agent_message`, `client_tool_result`,
    /// `set_output_medium`, `hang_up`, `ping`, `spawn_thread`); today the update is empty and
    /// reported as applied.
    fn session_update(
        &self,
        _old: &RealtimeConfig,
        new: &RealtimeConfig,
    ) -> Option<Vec<Self::Wire>> {
        (!self.setup_once).then(|| self.build_session_config(new, None))
    }

    /// `forced_agent_message` {content}: "Text content the agent should speak"; a guided line
    /// and context are `user_text_message` with `urgency` `immediate` ("Interrupts the agent if
    /// speaking and starts a new generation immediately") and `later` ("considered during the
    /// next natural generation without forcing a new generation"). No message stops the agent
    /// (<https://docs.ultravox.ai/apps/datamessages>).
    fn verb(&self, verb: &RealtimeVerb<'_>) -> Option<Vec<Self::Wire>> {
        let wire = match *verb {
            RealtimeVerb::SayExact(text) => {
                json!({ "type": "forced_agent_message", "content": text })
            }
            RealtimeVerb::SayGuided(text) => {
                json!({ "type": "user_text_message", "text": text, "urgency": "immediate" })
            }
            RealtimeVerb::AddContext(text) => {
                json!({ "type": "user_text_message", "text": text, "urgency": "later" })
            }
            RealtimeVerb::StopOutput => return None,
        };
        Some(vec![UltravoxClientMsg::Json(wire)])
    }

    fn map_server_event(&self, raw: Inbound<'_>) -> Vec<S2sEvent> {
        // Binary frames are agent TTS audio (raw PCM @ 24 kHz). No base64 — copy
        // the bytes straight through.
        let text = match raw {
            Inbound::Binary(b) => {
                return vec![S2sEvent::Audio {
                    data: Bytes::copy_from_slice(b),
                    item_id: None,
                    response_id: None,
                }];
            }
            Inbound::Text(t) => t,
        };

        let value: Value = match serde_json::from_str(text) {
            Ok(v) => v,
            Err(_) => return vec![S2sEvent::Ignore],
        };
        let msg_type = value.get("type").and_then(Value::as_str).unwrap_or("");

        match msg_type {
            // `{"type":"transcript","role":"agent"|"user","text"/"delta":…,
            // "final":bool}`. agent ⇒ Assistant, user ⇒ User. The text is `text`
            // if present, else the incremental `delta`.
            "transcript" if self.transcripts.is_some() && value.get("ordinal").is_some() => {
                self.transcript(&value)
            }
            // The call's id, for a re-dial with `priorCallId`.
            "call_started" => {
                if let Some(id) = value.get("callId").and_then(Value::as_str)
                    && let Ok(mut call) = self.call_id.lock()
                {
                    *call = Some(id.to_string());
                }
                vec![S2sEvent::Ignore]
            }
            "transcript" => {
                let role = match value.get("role").and_then(Value::as_str) {
                    Some("user") => TranscriptRole::User,
                    // agent (and any non-user) ⇒ assistant.
                    _ => TranscriptRole::Assistant,
                };
                let text = value
                    .get("text")
                    .and_then(Value::as_str)
                    .or_else(|| value.get("delta").and_then(Value::as_str))
                    .unwrap_or("")
                    .to_string();
                let is_final = value.get("final").and_then(Value::as_bool).unwrap_or(false);
                vec![S2sEvent::Transcript {
                    role,
                    text,
                    is_final,
                    item_id: None,
                }]
            }
            // Barge-in: the user interrupted; drop buffered output audio. The
            // driver clears local playback. NO truncate — Ultravox has none.
            "playback_clear_buffer" => vec![S2sEvent::InterruptedByServer],
            // Conversation/agent state (e.g. "thinking"/"speaking"/"listening").
            // Ultravox sends NO explicit turn-end, so a speaking→not-speaking edge
            // IS the response-completion signal (Pipecat treats it the same): fire
            // exactly one `ResponseDone` on that edge (empty id, like Deepgram's
            // AgentAudioDone). "speaking" / a non-transition ⇒ Ignore. (Review
            // wf_84b03504 #1.)
            "state" => {
                let speaking = value.get("state").and_then(Value::as_str) == Some("speaking");
                if speaking {
                    self.was_speaking.store(true, Ordering::Relaxed);
                    vec![S2sEvent::Ignore]
                } else if self.was_speaking.swap(false, Ordering::Relaxed) {
                    vec![S2sEvent::ResponseDone {
                        response_id: String::new(),
                    }]
                } else {
                    vec![S2sEvent::Ignore]
                }
            }
            // Client-side tool call: `{"type":"client_tool_invocation",
            // "toolName":…,"invocationId":…,"parameters":{…}}`. The arguments are
            // a JSON object ⇒ re-stringify for the normalized FunctionCallRequest.
            "client_tool_invocation" => {
                let arguments = match value.get("parameters") {
                    Some(Value::String(s)) => s.clone(),
                    Some(other) => other.to_string(),
                    None => String::new(),
                };
                vec![S2sEvent::FunctionCall(FunctionCallRequest {
                    call_id: value
                        .get("invocationId")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    name: value
                        .get("toolName")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    arguments,
                    item_id: None,
                })]
            }
            // Unknown/unhandled ⇒ nothing actionable.
            _ => vec![S2sEvent::Ignore],
        }
    }

    fn encode_user_audio(&self, pcm: &[u8]) -> Self::Wire {
        // THE binary path: raw PCM as a binary frame — no base64/JSON.
        UltravoxClientMsg::Audio(Bytes::copy_from_slice(pcm))
    }

    fn send_text(&self, text: &str) -> Vec<Self::Wire> {
        // Ultravox supports a user-side text message: `user_text_message`
        // (Pipecat `_send_user_text`). Used as the text-injection surface.
        vec![UltravoxClientMsg::Json(json!({
            "type": "user_text_message",
            "text": text,
        }))]
    }

    fn create_response(&self, _overrides: Option<&RealtimeResponseOverride>) -> Vec<Self::Wire> {
        // Server VAD + turn-taking owns response creation; nothing to send.
        Vec::new()
    }

    fn commit_turn(&self) -> Vec<Self::Wire> {
        // Server VAD owns turn close.
        Vec::new()
    }

    fn cancel_response(&self) -> Vec<Self::Wire> {
        // Barge-in is server-side (`playback_clear_buffer`); no client-driven
        // response cancel. Safest to send nothing.
        Vec::new()
    }

    fn format_tool_result(&self, call_id: &str, result: &str) -> Vec<Self::Wire> {
        // Tool result → `client_tool_result` echoing the `invocationId`; `result`
        // carries the result string (Pipecat `_send_tool_result`).
        vec![UltravoxClientMsg::Json(json!({
            "type": "client_tool_result",
            "invocationId": call_id,
            "result": result,
        }))]
    }

    fn serialize(&self, msg: &Self::Wire) -> RealtimeResult<OutFrame> {
        match msg {
            UltravoxClientMsg::Json(v) => {
                Ok(OutFrame::Text(serde_json::to_string(v).map_err(|e| {
                    RealtimeError::SerializationError(e.to_string())
                })?))
            }
            // THE binary path: audio frames serialize to a raw binary frame.
            UltravoxClientMsg::Audio(b) => Ok(OutFrame::Binary(b.clone())),
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::realtime::base::ReplayConversationItem;

    fn base_cfg() -> RealtimeConfig {
        RealtimeConfig {
            provider: "ultravox".into(),
            api_key: "uvkey".into(),
            model: "fixie-ai/ultravox".into(),
            voice: Some("Mark".into()),
            instructions: Some("You are helpful.".into()),
            ..Default::default()
        }
    }

    fn proto(cfg: &RealtimeConfig) -> UltravoxProtocol {
        UltravoxProtocol::from_config(cfg).unwrap()
    }

    /// from_config rejects an empty api key (the `X-API-Key` create-call needs it).
    #[test]
    fn from_config_requires_api_key() {
        let cfg = RealtimeConfig {
            api_key: String::new(),
            ..Default::default()
        };
        assert!(matches!(
            UltravoxProtocol::from_config(&cfg),
            Err(RealtimeError::AuthenticationFailed(_))
        ));
    }

    /// from_config defaults the model to `fixie-ai/ultravox` when omitted.
    #[test]
    fn from_config_defaults_model() {
        let cfg = RealtimeConfig {
            api_key: "uvkey".into(),
            ..Default::default()
        };
        let p = proto(&cfg);
        assert_eq!(p.model, "fixie-ai/ultravox");
    }

    /// connect_spec ⇒ RestThenWebSocket with the create-call URL, the `X-API-Key`
    /// header, a body carrying systemPrompt/model/voice/medium.serverWebSocket
    /// rates, and the `joinUrl` pointer.
    #[test]
    fn connect_spec_builds_rest_then_ws_create_call() {
        let p = proto(&base_cfg());
        let spec = p.connect_spec(&base_cfg()).unwrap();
        let ConnectSpec::RestThenWebSocket {
            create_url,
            headers,
            body,
            join_url_pointer,
        } = spec
        else {
            panic!("expected RestThenWebSocket, got {spec:?}");
        };
        assert_eq!(create_url, "https://api.ultravox.ai/api/calls");
        assert_eq!(join_url_pointer, "joinUrl");
        // The X-API-Key auth header.
        let api_key_hdr = headers
            .iter()
            .find(|(k, _)| k == "X-API-Key")
            .map(|(_, v)| v.as_str());
        assert_eq!(api_key_hdr, Some("uvkey"));
        // The body is JSON with the expected create-call shape.
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["systemPrompt"], "You are helpful.");
        assert_eq!(v["model"], "fixie-ai/ultravox");
        assert_eq!(v["voice"], "Mark");
        assert_eq!(v["medium"]["serverWebSocket"]["inputSampleRate"], 16000);
        assert_eq!(v["medium"]["serverWebSocket"]["outputSampleRate"], 24000);
    }

    /// SERVER-CONFIG `realtime_endpoint_override` WINS and changes the connect
    /// SHAPE: it BYPASSES the REST create-call and yields a PLAIN `WebSocket`
    /// straight to the override (a pre-authed join url / mock), with NO extra
    /// headers — vs the normal `RestThenWebSocket`. This is the documented
    /// limitation for pointing Ultravox at a mock that can't serve the create-call.
    /// Server-config-only (never client-settable).
    #[test]
    fn connect_spec_override_bypasses_rest_create_call() {
        let cfg = RealtimeConfig {
            realtime_endpoint_override: Some("ws://127.0.0.1:9008/join".into()),
            ..base_cfg()
        };
        let spec = proto(&cfg).connect_spec(&cfg).unwrap();
        let ConnectSpec::WebSocket { url, headers } = spec else {
            panic!("override must yield a PLAIN WebSocket (no REST handshake), got {spec:?}");
        };
        assert_eq!(url, "ws://127.0.0.1:9008/join");
        assert!(
            headers.is_empty(),
            "join url is pre-authed: no extra headers"
        );
    }

    /// create_call_body skips optional fields when absent (no instructions ⇒ no
    /// systemPrompt; no voice ⇒ no voice key) and still defaults the model.
    #[test]
    fn create_call_body_skips_absent_optionals() {
        let cfg = RealtimeConfig {
            api_key: "uvkey".into(),
            ..Default::default()
        };
        let p = proto(&cfg);
        let v = p.create_call_body(&cfg);
        assert_eq!(v["model"], "fixie-ai/ultravox");
        assert!(
            v.get("systemPrompt").is_none(),
            "no instructions ⇒ no systemPrompt"
        );
        assert!(v.get("voice").is_none(), "no voice ⇒ no voice key");
        assert_eq!(v["medium"]["serverWebSocket"]["inputSampleRate"], 16000);
    }

    /// build_session_config is EMPTY (the call is configured in the REST body).
    #[test]
    fn build_session_config_is_empty() {
        let p = proto(&base_cfg());
        assert!(p.build_session_config(&base_cfg(), None).is_empty());
        assert!(
            p.build_session_config(&base_cfg(), Some("resume"))
                .is_empty()
        );
    }

    /// caps: server-VAD turn frames, 48 B/ms @ 24 kHz output, NO truncate, NO
    /// input buffer.
    #[test]
    fn caps_describe_binary_server_vad_provider() {
        let c = proto(&base_cfg()).caps();
        assert!(c.emits_user_turn_frames);
        assert_eq!(c.output_bytes_per_ms, 48);
        assert_eq!(c.output_sample_rate, 24_000);
        assert!(!c.supports_truncate);
        assert!(!c.supports_input_buffer);
    }

    /// THE BINARY-PATH TEST: encode_user_audio ⇒ serialize ⇒ OutFrame::Binary
    /// carrying the EXACT PCM bytes (no base64, no JSON).
    #[test]
    fn encode_user_audio_serializes_to_raw_binary_frame() {
        let p = proto(&base_cfg());
        let pcm: Vec<u8> = vec![0x00, 0x01, 0xFE, 0xFF, 0x7F, 0x80];
        let wire = p.encode_user_audio(&pcm);
        match &wire {
            UltravoxClientMsg::Audio(b) => assert_eq!(b.as_ref(), pcm.as_slice()),
            UltravoxClientMsg::Json(_) => panic!("audio must be the Audio variant"),
        }
        match p.serialize(&wire).unwrap() {
            OutFrame::Binary(b) => assert_eq!(b.as_ref(), pcm.as_slice(), "raw PCM, byte-exact"),
            OutFrame::Text(_) => panic!("audio must serialize to a binary frame, not text"),
        }
    }

    /// map_server_event: a raw BINARY frame ⇒ Audio with byte-exact PCM.
    #[test]
    fn binary_frame_maps_to_audio() {
        let p = proto(&base_cfg());
        let pcm: &[u8] = &[1, 2, 3, 4, 250, 251];
        match p.map_server_event(Inbound::Binary(pcm)).as_slice() {
            [
                S2sEvent::Audio {
                    data,
                    item_id,
                    response_id,
                },
            ] => {
                assert_eq!(data.as_ref(), pcm);
                assert!(item_id.is_none());
                assert!(response_id.is_none());
            }
            other => panic!("expected Audio, got {other:?}"),
        }
    }

    /// map_server_event: agent transcript ⇒ Assistant; user transcript ⇒ User.
    #[test]
    fn transcript_maps_per_role() {
        let p = proto(&base_cfg());
        let agent = r#"{"type":"transcript","role":"agent","text":"hi there","final":true}"#;
        match p.map_server_event(Inbound::Text(agent)).as_slice() {
            [
                S2sEvent::Transcript {
                    role,
                    text,
                    is_final,
                    item_id,
                },
            ] => {
                assert_eq!(*role, TranscriptRole::Assistant);
                assert_eq!(text, "hi there");
                assert!(*is_final);
                assert!(item_id.is_none());
            }
            other => panic!("expected assistant Transcript, got {other:?}"),
        }
        let user = r#"{"type":"transcript","role":"user","text":"hello","final":true}"#;
        match p.map_server_event(Inbound::Text(user)).as_slice() {
            [
                S2sEvent::Transcript {
                    role,
                    text,
                    is_final,
                    ..
                },
            ] => {
                assert_eq!(*role, TranscriptRole::User);
                assert_eq!(text, "hello");
                assert!(*is_final);
            }
            other => panic!("expected user Transcript, got {other:?}"),
        }
    }

    /// map_server_event: a partial agent transcript with `delta` (no `text`) ⇒
    /// non-final Transcript carrying the delta text.
    #[test]
    fn transcript_uses_delta_when_no_text() {
        let p = proto(&base_cfg());
        let raw = r#"{"type":"transcript","role":"agent","delta":"wor","final":false}"#;
        match p.map_server_event(Inbound::Text(raw)).as_slice() {
            [
                S2sEvent::Transcript {
                    role,
                    text,
                    is_final,
                    ..
                },
            ] => {
                assert_eq!(*role, TranscriptRole::Assistant);
                assert_eq!(text, "wor");
                assert!(!*is_final);
            }
            other => panic!("expected delta Transcript, got {other:?}"),
        }
    }

    /// map_server_event: playback_clear_buffer ⇒ InterruptedByServer (barge-in
    /// clear; driver clears local playback; NO truncate).
    #[test]
    fn playback_clear_buffer_maps_to_interrupted() {
        let p = proto(&base_cfg());
        assert!(matches!(
            p.map_server_event(Inbound::Text(r#"{"type":"playback_clear_buffer"}"#))
                .as_slice(),
            [S2sEvent::InterruptedByServer]
        ));
    }

    /// map_server_event: `state` tracks the speaking edge — "speaking" ⇒ Ignore
    /// (but arms the edge); a later non-speaking state ⇒ exactly one ResponseDone;
    /// a non-speaking state with no prior "speaking" ⇒ Ignore (review wf_84b03504 #1).
    #[test]
    fn state_speaking_edge_fires_response_done_once() {
        let p = proto(&base_cfg());
        // Cold non-speaking state (no response in flight) ⇒ Ignore.
        assert!(matches!(
            p.map_server_event(Inbound::Text(r#"{"type":"state","state":"listening"}"#))
                .as_slice(),
            [S2sEvent::Ignore]
        ));
        // Agent starts speaking ⇒ Ignore, but arms the edge.
        assert!(matches!(
            p.map_server_event(Inbound::Text(r#"{"type":"state","state":"speaking"}"#))
                .as_slice(),
            [S2sEvent::Ignore]
        ));
        // speaking → not-speaking ⇒ exactly one ResponseDone.
        assert!(matches!(
            p.map_server_event(Inbound::Text(r#"{"type":"state","state":"listening"}"#))
                .as_slice(),
            [S2sEvent::ResponseDone { .. }]
        ));
        // The edge is consumed — a second non-speaking state does NOT re-fire.
        assert!(matches!(
            p.map_server_event(Inbound::Text(r#"{"type":"state","state":"thinking"}"#))
                .as_slice(),
            [S2sEvent::Ignore]
        ));
    }

    /// map_server_event: client_tool_invocation ⇒ FunctionCall carrying
    /// invocationId/toolName/parameters (object re-stringified to JSON args).
    #[test]
    fn client_tool_invocation_maps_to_function_call() {
        let p = proto(&base_cfg());
        let raw = r#"{"type":"client_tool_invocation","toolName":"get_weather","invocationId":"inv-1","parameters":{"city":"NYC"}}"#;
        match p.map_server_event(Inbound::Text(raw)).as_slice() {
            [S2sEvent::FunctionCall(fc)] => {
                assert_eq!(fc.call_id, "inv-1");
                assert_eq!(fc.name, "get_weather");
                assert_eq!(fc.arguments, r#"{"city":"NYC"}"#);
                assert!(fc.item_id.is_none());
            }
            other => panic!("expected FunctionCall, got {other:?}"),
        }
    }

    /// Unknown JSON + unparseable text ⇒ Ignore.
    #[test]
    fn unknown_and_unparseable_map_to_ignore() {
        let p = proto(&base_cfg());
        assert!(matches!(
            p.map_server_event(Inbound::Text(r#"{"type":"whatever"}"#))
                .as_slice(),
            [S2sEvent::Ignore]
        ));
        assert!(matches!(
            p.map_server_event(Inbound::Text("not json")).as_slice(),
            [S2sEvent::Ignore]
        ));
    }

    /// send_text ⇒ user_text_message; format_tool_result ⇒ client_tool_result;
    /// turn/cancel/response controls are empty (server VAD owns them).
    #[test]
    fn text_tool_and_turn_controls() {
        let p = proto(&base_cfg());
        match p.send_text("hello model").as_slice() {
            [UltravoxClientMsg::Json(v)] => {
                assert_eq!(v["type"], "user_text_message");
                assert_eq!(v["text"], "hello model");
            }
            other => panic!("expected one user_text_message, got {other:?}"),
        }
        match p.format_tool_result("inv-1", "72F sunny").as_slice() {
            [UltravoxClientMsg::Json(v)] => {
                assert_eq!(v["type"], "client_tool_result");
                assert_eq!(v["invocationId"], "inv-1");
                assert_eq!(v["result"], "72F sunny");
            }
            other => panic!("expected client_tool_result, got {other:?}"),
        }
        assert!(p.commit_turn().is_empty());
        assert!(p.cancel_response().is_empty());
        assert!(p.create_response(None).is_empty());
        // Default-empty (no truncate / input buffer / replay).
        assert!(p.truncate("x", 1).is_empty());
        assert!(p.clear_input_buffer().is_empty());
        assert!(
            p.replay_item(&ReplayConversationItem {
                role: TranscriptRole::User,
                text: "x".into(),
            })
            .is_empty()
        );
    }

    /// Provider fix `ultravox_tools`. Off: the create-call body is today's (the session's
    /// functions are not declared). On: `selectedTools` lists each function as a call-scoped
    /// client tool, per the create-call reference
    /// (<https://docs.ultravox.ai/api-reference/calls/calls-post>): `temporaryTool` with
    /// `modelToolName`, `description`, `dynamicParameters` (`location`
    /// `PARAMETER_LOCATION_BODY`, the property's JSON `schema`, `required`) and `client: {}`.
    #[test]
    fn the_tools_fix_declares_the_sessions_functions_as_client_tools() {
        use crate::core::realtime::base::{FunctionDefinition, ToolDefinition};
        let cfg = RealtimeConfig {
            tools: Some(vec![ToolDefinition {
                tool_type: "function".into(),
                function: FunctionDefinition {
                    name: "waav_delegate".into(),
                    description: Some("Hand it over.".into()),
                    parameters: Some(json!({"type": "object",
                        "properties": {"request": {"type": "string"}, "note": {"type": "string"}},
                        "required": ["request"]})),
                },
            }]),
            ..base_cfg()
        };
        let today = json!({
            "model": "fixie-ai/ultravox",
            "medium": {"serverWebSocket": {"inputSampleRate": 16000, "outputSampleRate": 24000}},
            "systemPrompt": "You are helpful.",
            "voice": "Mark",
        });
        assert_eq!(proto(&cfg).create_call_body(&cfg), today);
        let on = RealtimeConfig {
            provider_fixes: vec!["ultravox_tools".into()],
            ..cfg
        };
        let body = proto(&on).create_call_body(&on);
        let mut expected = today;
        expected["selectedTools"] = json!([{"temporaryTool": {
            "modelToolName": "waav_delegate",
            "description": "Hand it over.",
            "dynamicParameters": [
                {"name": "note", "location": "PARAMETER_LOCATION_BODY",
                 "schema": {"type": "string"}, "required": false},
                {"name": "request", "location": "PARAMETER_LOCATION_BODY",
                 "schema": {"type": "string"}, "required": true},
            ],
            "client": {},
        }}]);
        assert_eq!(body, expected);
        // No functions: no `selectedTools`, even with the fix.
        let bare = RealtimeConfig {
            provider_fixes: vec!["ultravox_tools".into()],
            ..base_cfg()
        };
        assert!(
            proto(&bare)
                .create_call_body(&bare)
                .get("selectedTools")
                .is_none()
        );
    }

    fn with_fixes(fixes: &[&str], cfg: RealtimeConfig) -> RealtimeConfig {
        RealtimeConfig {
            provider_fixes: fixes.iter().map(|f| (*f).to_string()).collect(),
            ..cfg
        }
    }

    fn transcript_text(events: &[S2sEvent]) -> (String, bool, Option<String>) {
        match events {
            [
                S2sEvent::Transcript {
                    text,
                    is_final,
                    item_id,
                    ..
                },
            ] => (text.clone(), *is_final, item_id.clone()),
            other => panic!("expected one transcript, got {other:?}"),
        }
    }

    /// `ultravox_transcripts`: `text` is the whole so far and `delta` what is new, per
    /// `ordinal`; an interim carries what is new (the driver accumulates interims), a final
    /// the message's whole text. Off: `text` then `delta` as they come.
    #[test]
    fn the_transcripts_fix_reads_text_and_delta_per_ordinal() {
        let cfg = with_fixes(&["ultravox_transcripts"], base_cfg());
        let p = proto(&cfg);
        let frame = |field: &str, value: &str, fin: bool| {
            format!(
                r#"{{"type":"transcript","role":"agent","medium":"voice","{field}":"{value}","final":{fin},"ordinal":3}}"#
            )
        };
        let run = |raw: String| transcript_text(&p.map_server_event(Inbound::Text(&raw)));
        assert_eq!(
            run(frame("text", "Hel", false)),
            ("Hel".into(), false, Some("3".into()))
        );
        assert_eq!(
            run(frame("text", "Hello", false)).0,
            "lo",
            "only what is new"
        );
        assert_eq!(run(frame("delta", " there", false)).0, " there");
        assert_eq!(
            run(frame("delta", "!", true)),
            ("Hello there!".into(), true, Some("3".into())),
            "a final that is only a delta logs the whole message"
        );
        let off = proto(&base_cfg());
        let raw = frame("delta", "!", true);
        assert_eq!(
            transcript_text(&off.map_server_event(Inbound::Text(&raw))).0,
            "!"
        );
    }

    /// `setup_once`: a mid-session change is refused (no data message changes the call);
    /// off, today's empty update.
    #[test]
    fn setup_once_refuses_a_mid_session_change() {
        let on = with_fixes(&["setup_once"], base_cfg());
        assert!(proto(&on).session_update(&on, &on).is_none());
        assert_eq!(
            proto(&base_cfg())
                .session_update(&base_cfg(), &base_cfg())
                .map(|w| w.len()),
            Some(0)
        );
    }

    /// `ultravox_prior_call`: after `call_started`, a re-dial creates the call with
    /// `?priorCallId=` (the new call takes the prior call's history); off, a fresh call.
    #[test]
    fn the_prior_call_fix_continues_the_call_on_a_re_dial() {
        let create_url =
            |p: &UltravoxProtocol, cfg: &RealtimeConfig| match p.connect_spec(cfg).unwrap() {
                ConnectSpec::RestThenWebSocket { create_url, .. } => create_url,
                other => panic!("expected the REST create-call, got {other:?}"),
            };
        let started = r#"{"type":"call_started","callId":"3f6a1c9e-0d4b-4c35-9a52-6f2b0e7d8a11","mediumRestriction":["voice"]}"#;
        let on = with_fixes(&["ultravox_prior_call"], base_cfg());
        let p = proto(&on);
        assert_eq!(
            create_url(&p, &on),
            ULTRAVOX_CREATE_CALL_URL,
            "the first dial"
        );
        assert!(matches!(
            p.map_server_event(Inbound::Text(started)).as_slice(),
            [S2sEvent::Ignore]
        ));
        assert_eq!(
            create_url(&p, &on),
            format!("{ULTRAVOX_CREATE_CALL_URL}?priorCallId=3f6a1c9e-0d4b-4c35-9a52-6f2b0e7d8a11")
        );
        let off = proto(&base_cfg());
        off.map_server_event(Inbound::Text(started));
        assert_eq!(create_url(&off, &base_cfg()), ULTRAVOX_CREATE_CALL_URL);
    }

    /// `ultravox_default_model`: no model named, none sent ("Currently defaults to
    /// ultravox-v0.7"); a named model is sent. Off: the legacy `fixie-ai/ultravox`.
    #[test]
    fn the_default_model_fix_leaves_an_unnamed_model_to_ultravox() {
        let unnamed = RealtimeConfig {
            model: String::new(),
            ..base_cfg()
        };
        let on = with_fixes(&["ultravox_default_model"], unnamed.clone());
        assert!(proto(&on).create_call_body(&on).get("model").is_none());
        let named = with_fixes(&["ultravox_default_model"], base_cfg());
        assert_eq!(
            proto(&named).create_call_body(&named)["model"],
            "fixie-ai/ultravox"
        );
        assert_eq!(
            proto(&unnamed).create_call_body(&unnamed)["model"],
            "fixie-ai/ultravox"
        );
    }

    /// `ultravox_call_settings`: `temperature` (clamped to 0..1) and the server-VAD silence as
    /// `vadSettings.turnEndpointDelay`; off, neither.
    #[test]
    fn the_call_settings_fix_sends_temperature_and_the_end_of_turn_delay() {
        let cfg = RealtimeConfig {
            temperature: Some(1.4),
            turn_detection: Some(TurnDetectionConfig::ServerVad {
                threshold: None,
                prefix_padding_ms: None,
                silence_duration_ms: Some(384),
                create_response: None,
                interrupt_response: None,
            }),
            ..base_cfg()
        };
        let on = with_fixes(&["ultravox_call_settings"], cfg.clone());
        let body = proto(&on).create_call_body(&on);
        assert_eq!(body["temperature"], json!(1.0));
        assert_eq!(body["vadSettings"], json!({"turnEndpointDelay": "0.384s"}));
        let body = proto(&cfg).create_call_body(&cfg);
        assert!(body.get("temperature").is_none() && body.get("vadSettings").is_none());
    }

    /// `ultravox_tools`: a function name outside `^[a-zA-Z0-9_-]{1,64}$` refuses the session
    /// before the create-call (Ultravox would refuse the whole call).
    #[test]
    fn the_tools_fix_refuses_a_tool_name_ultravox_refuses() {
        use crate::core::realtime::base::{FunctionDefinition, ToolDefinition};
        let tool = |name: &str| ToolDefinition {
            tool_type: "function".into(),
            function: FunctionDefinition {
                name: name.into(),
                description: None,
                parameters: None,
            },
        };
        let cfg = with_fixes(
            &["ultravox_tools"],
            RealtimeConfig {
                tools: Some(vec![tool("lookup.order")]),
                ..base_cfg()
            },
        );
        assert!(matches!(
            proto(&cfg).connect_spec(&cfg),
            Err(RealtimeError::InvalidConfiguration(m)) if m.contains("lookup.order")
        ));
        let cfg = RealtimeConfig {
            tools: Some(vec![tool("lookup_order-2")]),
            ..cfg
        };
        assert!(proto(&cfg).connect_spec(&cfg).is_ok());
    }

    /// The verbs: `forced_agent_message` speaks exact words; guided lines and context are
    /// `user_text_message` with `urgency` `immediate` and `later`; nothing stops the agent.
    #[test]
    fn the_verbs_are_the_documented_data_messages() {
        let p = proto(&base_cfg());
        let json_of = |verb: RealtimeVerb<'_>| match p.verb(&verb).as_deref() {
            Some([UltravoxClientMsg::Json(v)]) => Some(v.clone()),
            None => None,
            other => panic!("one JSON message expected, got {other:?}"),
        };
        assert_eq!(
            json_of(RealtimeVerb::SayExact("One moment.")),
            Some(json!({"type": "forced_agent_message", "content": "One moment."}))
        );
        assert_eq!(
            json_of(RealtimeVerb::SayGuided("Ask for the order number.")),
            Some(
                json!({"type": "user_text_message", "text": "Ask for the order number.", "urgency": "immediate"})
            )
        );
        assert_eq!(
            json_of(RealtimeVerb::AddContext("The order shipped today.")),
            Some(
                json!({"type": "user_text_message", "text": "The order shipped today.", "urgency": "later"})
            )
        );
        assert_eq!(json_of(RealtimeVerb::StopOutput), None);
    }
}
