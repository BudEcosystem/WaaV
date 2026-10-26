//! Deepgram Voice Agent [`RealtimeProtocol`] — the FIRST raw-BINARY-frame provider
//! on the S2S scaffold (OpenAI/Azure/Grok/Inworld are all JSON+base64).
//!
//! Deepgram's Voice Agent ("Converse") API is a single bidirectional WebSocket
//! that runs the whole S2S loop server-side: STT (`listen`) → LLM (`think`) →
//! TTS (`speak`) with server-side VAD + turn-taking + barge-in. Unlike the
//! OpenAI family, user audio is sent as RAW linear16 BINARY frames (NOT a
//! base64'd JSON event), and agent TTS audio comes back as RAW linear16 BINARY
//! frames. Only the session config + control + transcript/lifecycle events are
//! JSON.
//!
//! The wire here is the LIVE-CAPTURED oracle (Pipecat does not implement Voice
//! Agent): a JSON `Settings` message on connect, then binary audio in/out, plus
//! JSON server events (`Welcome`, `SettingsApplied`, `ConversationText`,
//! `AgentAudioDone`, `UserStartedSpeaking`, …). See per-arm comments.
//!
//! Docs: <https://developers.deepgram.com/docs/voice-agent> and the Voice Agent
//! WebSocket message reference (<https://developers.deepgram.com/docs/voice-agent-v1-migration>
//! / agent settings + server messages).

use std::collections::HashMap;
use std::sync::Mutex;

use bytes::Bytes;
use serde_json::{Value, json};

use crate::core::realtime::base::{
    FunctionCallRequest, RealtimeConfig, RealtimeError, RealtimeResponseOverride, RealtimeResult,
    RealtimeVerb, SpeechEvent, TranscriptRole,
};
use crate::core::realtime::scaffold::{
    ConnectSpec, Inbound, OutFrame, ProtocolCaps, RealtimeProtocol, S2sEvent,
};

/// Deepgram Voice Agent WebSocket endpoint. Single bidi socket for the whole
/// STT→LLM→TTS loop. LIVE-CAPTURED working endpoint.
pub(crate) const DEEPGRAM_AGENT_URL: &str = "wss://agent.deepgram.com/v1/agent/converse";

/// The Voice Agent in Deepgram's EU region (probed live 2026-10-15: `agent.eu.deepgram.com`
/// does not resolve; the EU API host serves the agent path).
pub(crate) const DEEPGRAM_AGENT_EU_URL: &str = "wss://api.eu.deepgram.com/v1/agent/converse";

/// The warning code of an `InjectAgentMessage` the agent ignored because it was replying.
const INJECT_DURING_SPEECH: &str = "INJECT_AGENT_MESSAGE_DURING_AGENT_SPEECH";

/// The error code of a `Settings` message the agent refused (a field it does not take with the
/// chosen models): the same settings are refused on every reconnect.
const INVALID_SETTINGS: &str = "INVALID_SETTINGS";

/// Who sent the latest injected line: the client's typed text (`send_text`), which this
/// protocol sends again when the agent ignored it, or the gateway's own verb, which the
/// interaction executor sends again (`S2sEvent::InjectionRefused`).
#[derive(Debug, Default)]
enum Injected {
    #[default]
    None,
    Client(String),
    Gateway,
}

/// Linear16 @ 24 kHz: WaaV's canonical realtime PCM rate (matches the OpenAI
/// family) — keeps the gateway audio path uniform across S2S providers. Used for
/// BOTH input + output unless `cfg.{input,output}_audio_format` overrides.
const DEFAULT_SAMPLE_RATE: u32 = 24_000;

/// Default agent TTS voice. `aura-2-*` is Deepgram's current (Aura-2) voice
/// family; `thalia` is the LIVE-CAPTURED working voice.
const DEFAULT_SPEAK_MODEL: &str = "aura-2-thalia-en";

/// Default STT model for the `listen` leg (Deepgram's current flagship).
const DEFAULT_LISTEN_MODEL: &str = "nova-3";

/// Default `think` LLM, on Deepgram's managed `open_ai` provider (Deepgram holds the creds).
const DEFAULT_THINK_MODEL: &str = "gpt-4o-mini";

/// The think provider of a model Deepgram runs for the agent, by its name (Deepgram's
/// think-model catalogue): Anthropic's Claude, Google's Gemini, the open models Groq and NVIDIA
/// serve, OpenAI's otherwise.
fn think_provider_of(model: &str) -> &'static str {
    let model = model.trim().to_ascii_lowercase();
    if model.starts_with("claude") {
        "anthropic"
    } else if model.starts_with("gemini") {
        "google"
    } else if model.starts_with("openai/gpt-oss") {
        "groq"
    } else if model.starts_with("nvidia/") {
        "nvidia"
    } else {
        "open_ai"
    }
}

/// `agent.language`: the language subtag of the code Deepgram takes for it (`de-DE` and `de`
/// are `de`, Mandarin `zh`, `auto` `multi`); `en` when unset.
fn agent_language(language: Option<&str>) -> String {
    match language.map(str::trim).filter(|l| !l.is_empty()) {
        None => "en".to_owned(),
        Some(language) => {
            let native = crate::core::lang::map_language(language, "deepgram", "").native;
            native.split('-').next().unwrap_or_default().to_owned()
        }
    }
}

/// linear16 @ 24 kHz is mono 16-bit ⇒ 2 bytes/sample × 24 samples/ms = 48 B/ms.
const OUTPUT_BYTES_PER_MS: u64 = 48;

/// One typed outbound Voice Agent wire message. The defining BINARY split:
/// `Json` ⇒ a text frame (Settings / control), `Audio` ⇒ a RAW linear16 binary
/// frame (user mic audio — NOT base64/JSON).
#[derive(Debug, Clone)]
pub enum DeepgramClientMsg {
    /// A JSON control/config message: `Settings`, `KeepAlive`,
    /// `InjectAgentMessage`, `UpdatePrompt`, `FunctionCallResponse`.
    Json(Value),
    /// A raw linear16 PCM audio frame (the binary path).
    Audio(Bytes),
}

/// The Deepgram Voice Agent protocol. Stateless: the model/voice/language/
/// instructions/sample-rate it needs all come from `cfg` (re-passed into
/// `build_session_config`), so it carries only what `from_config` validated.
pub struct DeepgramProtocol {
    /// Cached for `caps()`; the live output rate the server is told to emit.
    output_sample_rate: u32,
    /// Provider fix `deepgram_user_text`: typed text is the user's (`InjectUserMessage`).
    user_text: bool,
    /// Provider fix `deepgram_tools`: the session's functions go to the agent's `think`.
    tools: bool,
    /// Provider fix `deepgram_update_prompt`: a mid-session update is `UpdatePrompt` and
    /// `UpdateSpeak`, never a second `Settings`.
    update_prompt: bool,
    /// The name of each open function call, by id: Deepgram refuses a
    /// `FunctionCallResponse` without it (`UNPARSABLE_CLIENT_MESSAGE`, probed 2026-10-15).
    calls: Mutex<HashMap<String, String>>,
    /// The latest injected line, and the client's line the agent ignored, owed after its reply.
    injected: Mutex<(Injected, Option<String>)>,
}

impl DeepgramProtocol {
    /// Resolve the configured (or default) sample rate. Deepgram accepts a single
    /// integer per direction; WaaV's canonical S2S rate is 24 kHz.
    fn sample_rate(cfg: &RealtimeConfig) -> u32 {
        // `input_audio_format` in WaaV is an encoding hint ("linear16"/"pcm16"…),
        // not a rate; there is no per-rate field on RealtimeConfig, so the S2S
        // canonical 24 kHz is used (matches the OpenAI family + the live capture).
        let _ = cfg;
        DEFAULT_SAMPLE_RATE
    }

    /// The client's injected line, remembered in case the agent ignores it while replying.
    fn client_line(&self, wire: Value) -> DeepgramClientMsg {
        if let Ok(mut injected) = self.injected.lock() {
            injected.0 = Injected::Client(wire.to_string());
        }
        DeepgramClientMsg::Json(wire)
    }

    /// Build the LIVE-CAPTURED `Settings` JSON from the generic config:
    /// model → `think` LLM (its vendor `think_provider`, else by its name), voice → `speak`
    /// TTS, the transcription model → `listen` (nova-3 when unset or OpenAI's `whisper-1`),
    /// language → `agent.language`, instructions → `think.prompt`, audio in/out linear16 @
    /// rate. The session's tools go to `think.functions` with the `deepgram_tools` fix, and
    /// always in interaction mode (the delegate function is among them).
    pub fn settings(&self, cfg: &RealtimeConfig) -> Value {
        let rate = Self::sample_rate(cfg);

        let speak_model = cfg.voice.as_deref().unwrap_or(DEFAULT_SPEAK_MODEL);
        let think_model = if cfg.model.trim().is_empty() {
            DEFAULT_THINK_MODEL
        } else {
            cfg.model.trim()
        };
        let think_type = cfg
            .think_provider
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| think_provider_of(think_model));
        let language = agent_language(cfg.language.as_deref());
        // The handler fills an unset transcription model with OpenAI's `whisper-1`.
        let listen_model = cfg
            .input_audio_transcription
            .as_ref()
            .map(|t| t.model.trim())
            .filter(|m| !m.is_empty() && *m != "whisper-1")
            .unwrap_or(DEFAULT_LISTEN_MODEL);

        // `think.prompt` is the system prompt. Deepgram is lenient, but only emit
        // the key when we actually have instructions (skip-if-none discipline).
        let mut think = json!({
            "provider": { "type": think_type, "model": think_model },
        });
        if let Some(instr) = cfg.instructions.as_deref()
            && !instr.is_empty()
        {
            think["prompt"] = json!(instr);
        }
        let tools = cfg.tools.as_deref().unwrap_or_default();
        let delegate = tools
            .iter()
            .any(|t| t.function.name == crate::core::interaction::provider::DELEGATE_TOOL);
        if (self.tools || delegate) && !tools.is_empty() {
            let functions: Vec<Value> = tools
                .iter()
                .map(|t| {
                    let mut function = json!({
                        "name": t.function.name,
                        "parameters": t.function.parameters.clone()
                            .unwrap_or_else(|| json!({"type": "object", "properties": {}})),
                    });
                    if let Some(description) = &t.function.description {
                        function["description"] = json!(description);
                    }
                    function
                })
                .collect();
            think["functions"] = json!(functions);
        }

        let mut settings = json!({
            "type": "Settings",
            "audio": {
                "input": { "encoding": "linear16", "sample_rate": rate },
                "output": { "encoding": "linear16", "sample_rate": rate, "container": "none" },
            },
            "agent": {
                "listen": { "provider": { "type": "deepgram", "model": listen_model } },
                "think": think,
                "speak": { "provider": { "type": "deepgram", "model": speak_model } },
            },
        });
        // A Flux (`/v2/listen`) listen model refuses `agent.language` (`INVALID_SETTINGS`,
        // probed 2026-10-15): it is sent for the other listen models only.
        if !crate::core::stt::deepgram::is_flux(listen_model) {
            settings["agent"]["language"] = json!(language);
        }
        settings
    }
}

impl RealtimeProtocol for DeepgramProtocol {
    type Wire = DeepgramClientMsg;

    fn from_config(cfg: &RealtimeConfig) -> RealtimeResult<Self> {
        // Deepgram uses `Authorization: Token <key>` — a non-empty key is required.
        if cfg.api_key.is_empty() {
            return Err(RealtimeError::AuthenticationFailed(
                "API key is required".to_string(),
            ));
        }
        Ok(Self {
            output_sample_rate: Self::sample_rate(cfg),
            user_text: cfg.fix("deepgram_user_text"),
            tools: cfg.fix("deepgram_tools"),
            update_prompt: cfg.fix("deepgram_update_prompt"),
            calls: Mutex::new(HashMap::new()),
            injected: Mutex::new((Injected::None, None)),
        })
    }

    fn provider_id(&self) -> &'static str {
        "deepgram"
    }

    fn input_sample_rate(&self) -> u32 {
        // One rate both ways (the `Settings` audio block).
        self.output_sample_rate
    }

    fn caps(&self) -> ProtocolCaps {
        ProtocolCaps {
            // Deepgram runs server-side VAD + turn-taking, so it owns turns and
            // emits user-turn frames (`UserStartedSpeaking`, transcripts).
            emits_user_turn_frames: true,
            output_bytes_per_ms: OUTPUT_BYTES_PER_MS,
            output_sample_rate: self.output_sample_rate,
            // Voice Agent has NO server-side item truncation (barge-in is handled
            // server-side; the gateway only clears local playback).
            supports_truncate: false,
            // No gateway-managed input buffer (raw binary audio is streamed).
            supports_input_buffer: false,
        }
    }

    fn connect_spec(&self, cfg: &RealtimeConfig) -> RealtimeResult<ConnectSpec> {
        // Deepgram auth scheme is `Token <key>`, NOT `Bearer`. The standard
        // WS-upgrade headers are generated by `into_client_request()` in the
        // transport factory.
        //
        // SERVER-CONFIG `realtime_endpoint_override` (proxy / self-hosted / gov-cloud
        // / local mock) WINS over the Voice Agent host when set — the `Token` header
        // rides along unchanged. SSRF-safe: never client-settable.
        let host = match cfg.region.as_deref().map(|r| r.trim().to_ascii_lowercase()) {
            None => DEEPGRAM_AGENT_URL,
            Some(region) if region.is_empty() || region == "us" => DEEPGRAM_AGENT_URL,
            Some(region) if region == "eu" => DEEPGRAM_AGENT_EU_URL,
            Some(region) => {
                return Err(RealtimeError::InvalidConfiguration(format!(
                    "Deepgram Voice Agent region {region:?} is not known; use \"us\" or \"eu\""
                )));
            }
        };
        let url = crate::core::realtime::scaffold::apply_endpoint_override(
            host,
            cfg.realtime_endpoint_override.as_deref(),
            None,
        );
        Ok(ConnectSpec::WebSocket {
            url,
            headers: vec![(
                "Authorization".to_string(),
                format!("Token {}", cfg.api_key),
            )],
        })
    }

    fn build_session_config(
        &self,
        cfg: &RealtimeConfig,
        _resumption: Option<&str>,
    ) -> Vec<Self::Wire> {
        // The single `Settings` message, sent on connect (and re-sent on
        // reconnect — Deepgram re-applies it on a fresh socket).
        vec![DeepgramClientMsg::Json(self.settings(cfg))]
    }

    fn session_update(
        &self,
        old: &RealtimeConfig,
        new: &RealtimeConfig,
    ) -> Option<Vec<Self::Wire>> {
        if !self.update_prompt {
            return Some(self.build_session_config(new, None));
        }
        // `Settings` is taken once per connection: a changed prompt is `UpdatePrompt`, a
        // changed voice `UpdateSpeak` (<https://developers.deepgram.com/docs/voice-agent-update-prompt>,
        // `.../voice-agent-update-speak`). Anything else cannot change on the open socket.
        let (mut before, mut after) = (self.settings(old), self.settings(new));
        let mut wires = Vec::new();
        let prompt = after["agent"]["think"]["prompt"].take();
        if before["agent"]["think"]["prompt"].take() != prompt
            && let Some(prompt) = prompt.as_str()
        {
            wires.push(json!({ "type": "UpdatePrompt", "prompt": prompt }));
        }
        let speak = after["agent"]["speak"].take();
        if before["agent"]["speak"].take() != speak {
            wires.push(json!({ "type": "UpdateSpeak", "speak": speak }));
        }
        (before == after).then(|| wires.into_iter().map(DeepgramClientMsg::Json).collect())
    }

    fn map_server_event(&self, raw: Inbound<'_>) -> Vec<S2sEvent> {
        // Binary frames are agent TTS audio (linear16 @ 24 kHz). No base64 —
        // copy the bytes straight through.
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
            // Session ack: `{"type":"Welcome","request_id":"<uuid>"}`.
            "Welcome" => vec![S2sEvent::SessionReady {
                session_id: value
                    .get("request_id")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            }],
            // Config ack — nothing actionable.
            "SettingsApplied" | "PromptUpdated" | "SpeakUpdated" => vec![S2sEvent::Ignore],
            // FINAL transcript, per role:
            // `{"type":"ConversationText","role":"assistant"|"user","content":"…"}`.
            "ConversationText" => {
                let role = match value.get("role").and_then(Value::as_str) {
                    Some("user") => TranscriptRole::User,
                    // assistant (and any non-user) ⇒ assistant.
                    _ => TranscriptRole::Assistant,
                };
                let text = value
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                vec![S2sEvent::Transcript {
                    role,
                    text,
                    is_final: true,
                    item_id: None,
                }]
            }
            // `History` is a DUPLICATE of ConversationText — ignore to avoid
            // double-logging the conversation.
            "History" => vec![S2sEvent::Ignore],
            // Agent finished THIS response's audio (generation done, NOT a
            // playout/clear signal). Deepgram gives no response id.
            // A client line the agent ignored during this reply goes out again now.
            "AgentAudioDone" => {
                let mut events = vec![S2sEvent::ResponseDone {
                    response_id: String::new(),
                }];
                if let Some(line) = self.injected.lock().ok().and_then(|mut i| i.1.take()) {
                    events.push(S2sEvent::SendFrame(OutFrame::Text(line)));
                }
                events
            }
            // Barge-in: the user interrupted; the server already stopped agent
            // audio. Emit a Speech(Started) edge + InterruptedByServer so the
            // driver clears local playback. NO truncate — Voice Agent has none.
            // (Docs: `UserStartedSpeaking`.)
            "UserStartedSpeaking" => vec![
                S2sEvent::Speech(SpeechEvent::Started {
                    audio_start_ms: 0,
                    item_id: None,
                }),
                S2sEvent::InterruptedByServer,
            ],
            // Informational lifecycle (docs: `AgentThinking` / `AgentStartedSpeaking`).
            // No clean SpeechEvent mapping (output-side, not a user-VAD edge), so
            // ignore.
            "AgentThinking" | "AgentStartedSpeaking" => vec![S2sEvent::Ignore],
            // Tool call: `{"type":"FunctionCallRequest","functions":[{"id","name",
            // "arguments","client_side",…}]}` — one FunctionCall per CLIENT-SIDE
            // function. SERVER-SIDE functions (`client_side:false`) are executed by
            // Deepgram itself; the client must NOT reply, so we skip them (emitting
            // a FunctionCall would invite a spurious FunctionCallResponse). (Review
            // wf_c40fc67c #1.)
            "FunctionCallRequest" => value
                .get("functions")
                .and_then(Value::as_array)
                .map(|funcs| {
                    funcs
                        .iter()
                        .filter(|f| {
                            f.get("client_side")
                                .and_then(Value::as_bool)
                                .unwrap_or(true)
                        })
                        .map(|f| {
                            let text = |key: &str| {
                                f.get(key).and_then(Value::as_str).unwrap_or("").to_string()
                            };
                            let (call_id, name) = (text("id"), text("name"));
                            if let Ok(mut calls) = self.calls.lock() {
                                calls.insert(call_id.clone(), name.clone());
                            }
                            S2sEvent::FunctionCall(FunctionCallRequest {
                                call_id,
                                name,
                                // `arguments` is documented as a JSON string; if a
                                // server ever sends an object, re-stringify it.
                                arguments: match f.get("arguments") {
                                    Some(Value::String(s)) => s.clone(),
                                    Some(other) => other.to_string(),
                                    None => String::new(),
                                },
                                item_id: None,
                            })
                        })
                        .collect()
                })
                .unwrap_or_else(|| vec![S2sEvent::Ignore]),
            // Error / Warning (docs): surface the description/message. A `Warning` is not an
            // error: the agent goes on. Refused settings are a refusal (no retry helps). An
            // injected line the agent ignored while replying goes out again after the reply:
            // the client's from here, the gateway's from the interaction executor.
            "Error" | "Warning" => {
                let detail = value
                    .get("description")
                    .and_then(Value::as_str)
                    .or_else(|| value.get("message").and_then(Value::as_str))
                    .unwrap_or("unknown Deepgram agent error")
                    .to_string();
                let code = value.get("code").and_then(Value::as_str);
                let error = match (msg_type, code) {
                    ("Warning", _) => RealtimeError::ProviderWarning {
                        code: code.map(str::to_owned),
                        message: detail,
                    },
                    (_, Some(INVALID_SETTINGS)) => RealtimeError::Refused(detail),
                    _ => RealtimeError::ProviderError(detail),
                };
                let mut events = vec![S2sEvent::Error(error)];
                if code == Some(INJECT_DURING_SPEECH) {
                    let mut injected = self.injected.lock().unwrap_or_else(|e| e.into_inner());
                    match &injected.0 {
                        Injected::Client(line) => injected.1 = Some(line.clone()),
                        _ => events.push(S2sEvent::InjectionRefused),
                    }
                }
                events
            }
            // Unknown/unhandled (e.g. keepalive acks) ⇒ nothing actionable.
            _ => vec![S2sEvent::Ignore],
        }
    }

    fn encode_user_audio(&self, pcm: &[u8]) -> Self::Wire {
        // THE binary path: raw linear16 PCM as a binary frame — no base64/JSON.
        DeepgramClientMsg::Audio(Bytes::copy_from_slice(pcm))
    }

    fn send_text(&self, text: &str) -> Vec<Self::Wire> {
        // Provider fix `deepgram_user_text` (plan W6 Decision 4): the user typed this, so it
        // is a user message the agent answers (`InjectUserMessage{content}`), not words the
        // agent reads aloud. Off until Deepgram's live probe; the escape flag
        // `deepgram_text_as_agent_message` keeps the line below.
        if self.user_text {
            return vec![self.client_line(json!({
                "type": "InjectUserMessage",
                "content": text,
            }))];
        }
        // Best-effort: Voice Agent has no "user text turn" event; the closest
        // documented control is `InjectAgentMessage`, which makes the AGENT SPEAK
        // the given text (not a user message). Used here as the text-injection
        // surface. (Docs: `InjectAgentMessage`.)
        vec![self.client_line(json!({
            "type": "InjectAgentMessage",
            "content": text,
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
        // Voice Agent has no client-driven response cancel; barge-in is handled
        // server-side (`UserStartedSpeaking`). Safest to send nothing.
        Vec::new()
    }

    fn format_tool_result(&self, call_id: &str, result: &str) -> Vec<Self::Wire> {
        // Tool result → `FunctionCallResponse` (docs). The `id` echoes the request's function
        // id, `name` its function (required: without it Deepgram answers
        // `UNPARSABLE_CLIENT_MESSAGE`), `content` carries the result string.
        let name = self
            .calls
            .lock()
            .ok()
            .and_then(|mut calls| calls.remove(call_id))
            .unwrap_or_default();
        vec![DeepgramClientMsg::Json(json!({
            "type": "FunctionCallResponse",
            "id": call_id,
            "name": name,
            "content": result,
        }))]
    }

    fn verb(&self, verb: &RealtimeVerb<'_>) -> Option<Vec<Self::Wire>> {
        // The documented injection wires. (`send_text` keeps its own, older shape.)
        let wire = match *verb {
            RealtimeVerb::SayExact(words) => json!({
                "type": "InjectAgentMessage",
                "message": words,
            }),
            RealtimeVerb::SayGuided(guidance) => json!({
                "type": "InjectUserMessage",
                "content": guidance,
            }),
            // No context-only message and no client-driven cancel on this wire.
            RealtimeVerb::AddContext(_) | RealtimeVerb::StopOutput => return None,
        };
        if let Ok(mut injected) = self.injected.lock() {
            injected.0 = Injected::Gateway;
        }
        Some(vec![DeepgramClientMsg::Json(wire)])
    }

    fn serialize(&self, msg: &Self::Wire) -> RealtimeResult<OutFrame> {
        match msg {
            DeepgramClientMsg::Json(v) => {
                Ok(OutFrame::Text(serde_json::to_string(v).map_err(|e| {
                    RealtimeError::SerializationError(e.to_string())
                })?))
            }
            // THE binary path: audio frames serialize to a raw binary frame.
            DeepgramClientMsg::Audio(b) => Ok(OutFrame::Binary(b.clone())),
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
            provider: "deepgram".into(),
            api_key: "dgkey".into(),
            model: "gpt-4o-mini".into(),
            voice: Some("aura-2-thalia-en".into()),
            instructions: Some("You are helpful.".into()),
            ..Default::default()
        }
    }

    fn proto(cfg: &RealtimeConfig) -> DeepgramProtocol {
        DeepgramProtocol::from_config(cfg).unwrap()
    }

    /// from_config rejects an empty api key (`Token <key>` auth requires it).
    #[test]
    fn from_config_requires_api_key() {
        let cfg = RealtimeConfig {
            api_key: String::new(),
            ..Default::default()
        };
        assert!(matches!(
            DeepgramProtocol::from_config(&cfg),
            Err(RealtimeError::AuthenticationFailed(_))
        ));
    }

    /// connect_spec: the EXACT Voice Agent URL + `Token <key>` (NOT Bearer).
    #[test]
    fn connect_spec_uses_agent_url_and_token_auth() {
        let p = proto(&base_cfg());
        let ConnectSpec::WebSocket { url, headers } = p.connect_spec(&base_cfg()).unwrap() else {
            panic!("expected ConnectSpec::WebSocket")
        };
        assert_eq!(url, "wss://agent.deepgram.com/v1/agent/converse");
        let auth = headers
            .iter()
            .find(|(k, _)| k == "Authorization")
            .map(|(_, v)| v.as_str())
            .unwrap();
        assert_eq!(auth, "Token dgkey", "Deepgram uses Token, not Bearer");
        assert!(!auth.starts_with("Bearer"), "must NOT be a Bearer header");
    }

    /// SERVER-CONFIG `realtime_endpoint_override` WINS over the Voice Agent host
    /// VERBATIM; the `Token` header rides along. Server-config-only (never
    /// client-settable).
    #[test]
    fn connect_spec_honors_server_endpoint_override() {
        let cfg = RealtimeConfig {
            realtime_endpoint_override: Some("ws://127.0.0.1:9005/dg".into()),
            ..base_cfg()
        };
        let ConnectSpec::WebSocket { url, headers } = proto(&cfg).connect_spec(&cfg).unwrap()
        else {
            panic!("expected WebSocket")
        };
        assert_eq!(url, "ws://127.0.0.1:9005/dg");
        assert!(
            headers
                .iter()
                .any(|(k, v)| k == "Authorization" && v == "Token dgkey")
        );
    }

    /// build_session_config serializes to the LIVE-CAPTURED Settings shape:
    /// type=Settings, audio.input/output linear16, agent.listen/think/speak.
    #[test]
    fn build_session_config_matches_captured_settings() {
        let p = proto(&base_cfg());
        let wires = p.build_session_config(&base_cfg(), None);
        assert_eq!(wires.len(), 1);
        let json = match p.serialize(&wires[0]).unwrap() {
            OutFrame::Text(s) => s,
            OutFrame::Binary(_) => panic!("Settings must be a text frame"),
        };
        // Re-parse for structural assertions (field order is irrelevant).
        let v: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["type"], "Settings");
        assert_eq!(v["audio"]["input"]["encoding"], "linear16");
        assert_eq!(v["audio"]["input"]["sample_rate"], 24000);
        assert_eq!(v["audio"]["output"]["encoding"], "linear16");
        assert_eq!(v["audio"]["output"]["sample_rate"], 24000);
        assert_eq!(v["audio"]["output"]["container"], "none");
        assert_eq!(v["agent"]["language"], "en");
        assert_eq!(v["agent"]["listen"]["provider"]["type"], "deepgram");
        assert_eq!(v["agent"]["listen"]["provider"]["model"], "nova-3");
        assert_eq!(v["agent"]["think"]["provider"]["type"], "open_ai");
        assert_eq!(v["agent"]["think"]["provider"]["model"], "gpt-4o-mini");
        assert_eq!(v["agent"]["think"]["prompt"], "You are helpful.");
        assert_eq!(v["agent"]["speak"]["provider"]["type"], "deepgram");
        assert_eq!(v["agent"]["speak"]["provider"]["model"], "aura-2-thalia-en");
    }

    /// Defaults: empty model ⇒ gpt-4o-mini think; no voice ⇒ aura-2-thalia-en
    /// speak; no instructions ⇒ NO `prompt` key (skip-if-none discipline).
    #[test]
    fn settings_apply_defaults_and_skip_empty_prompt() {
        let cfg = RealtimeConfig {
            api_key: "dgkey".into(),
            ..Default::default()
        };
        let p = proto(&cfg);
        let v = p.settings(&cfg);
        assert_eq!(v["agent"]["think"]["provider"]["model"], "gpt-4o-mini");
        assert_eq!(v["agent"]["speak"]["provider"]["model"], "aura-2-thalia-en");
        assert!(
            v["agent"]["think"].get("prompt").is_none(),
            "no instructions ⇒ no prompt key"
        );
    }

    /// THE BINARY-PATH TEST: encode_user_audio ⇒ serialize ⇒ OutFrame::Binary
    /// carrying the EXACT PCM bytes (no base64, no JSON).
    #[test]
    fn encode_user_audio_serializes_to_raw_binary_frame() {
        let p = proto(&base_cfg());
        let pcm: Vec<u8> = vec![0x00, 0x01, 0xFE, 0xFF, 0x7F, 0x80];
        let wire = p.encode_user_audio(&pcm);
        // It is the Audio variant (not Json).
        match &wire {
            DeepgramClientMsg::Audio(b) => assert_eq!(b.as_ref(), pcm.as_slice()),
            DeepgramClientMsg::Json(_) => panic!("audio must be the Audio variant"),
        }
        // And it serializes to a binary frame with the exact bytes.
        match p.serialize(&wire).unwrap() {
            OutFrame::Binary(b) => assert_eq!(b.as_ref(), pcm.as_slice(), "raw PCM, byte-exact"),
            OutFrame::Text(_) => panic!("audio must serialize to a binary frame, not text"),
        }
    }

    /// caps: server-VAD turn frames, 48 B/ms @ 24 kHz, NO truncate, NO input buffer.
    #[test]
    fn caps_describe_binary_server_vad_provider() {
        let c = proto(&base_cfg()).caps();
        assert!(c.emits_user_turn_frames);
        assert_eq!(c.output_bytes_per_ms, 48);
        assert_eq!(c.output_sample_rate, 24_000);
        assert!(!c.supports_truncate);
        assert!(!c.supports_input_buffer);
    }

    /// map_server_event: Welcome ⇒ SessionReady carrying request_id.
    #[test]
    fn welcome_maps_to_session_ready() {
        let p = proto(&base_cfg());
        let raw = r#"{"type":"Welcome","request_id":"req-123"}"#;
        match p.map_server_event(Inbound::Text(raw)).as_slice() {
            [S2sEvent::SessionReady { session_id }] => {
                assert_eq!(session_id.as_deref(), Some("req-123"));
            }
            other => panic!("expected SessionReady, got {other:?}"),
        }
    }

    /// map_server_event: SettingsApplied ⇒ Ignore (config ack).
    #[test]
    fn settings_applied_maps_to_ignore() {
        let p = proto(&base_cfg());
        assert!(matches!(
            p.map_server_event(Inbound::Text(r#"{"type":"SettingsApplied"}"#))
                .as_slice(),
            [S2sEvent::Ignore]
        ));
    }

    /// map_server_event: ConversationText user + assistant ⇒ FINAL transcript.
    #[test]
    fn conversation_text_maps_to_final_transcript_per_role() {
        let p = proto(&base_cfg());
        let user = r#"{"type":"ConversationText","role":"user","content":"hello there"}"#;
        match p.map_server_event(Inbound::Text(user)).as_slice() {
            [
                S2sEvent::Transcript {
                    role,
                    text,
                    is_final,
                    item_id,
                },
            ] => {
                assert_eq!(*role, TranscriptRole::User);
                assert_eq!(text, "hello there");
                assert!(*is_final);
                assert!(item_id.is_none());
            }
            other => panic!("expected user Transcript, got {other:?}"),
        }
        let asst = r#"{"type":"ConversationText","role":"assistant","content":"hi!"}"#;
        match p.map_server_event(Inbound::Text(asst)).as_slice() {
            [
                S2sEvent::Transcript {
                    role,
                    text,
                    is_final,
                    ..
                },
            ] => {
                assert_eq!(*role, TranscriptRole::Assistant);
                assert_eq!(text, "hi!");
                assert!(*is_final);
            }
            other => panic!("expected assistant Transcript, got {other:?}"),
        }
    }

    /// map_server_event: History ⇒ Ignore (avoid double-logging ConversationText).
    #[test]
    fn history_maps_to_ignore() {
        let p = proto(&base_cfg());
        let raw = r#"{"type":"History","role":"assistant","content":"hi!"}"#;
        assert!(
            matches!(
                p.map_server_event(Inbound::Text(raw)).as_slice(),
                [S2sEvent::Ignore]
            ),
            "History must be ignored to avoid double-logging"
        );
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

    /// map_server_event: AgentAudioDone ⇒ ResponseDone (empty response_id).
    #[test]
    fn agent_audio_done_maps_to_response_done() {
        let p = proto(&base_cfg());
        match p
            .map_server_event(Inbound::Text(r#"{"type":"AgentAudioDone"}"#))
            .as_slice()
        {
            [S2sEvent::ResponseDone { response_id }] => assert!(response_id.is_empty()),
            other => panic!("expected ResponseDone, got {other:?}"),
        }
    }

    /// map_server_event: UserStartedSpeaking ⇒ [Speech(Started), InterruptedByServer]
    /// (barge-in; driver clears playback; NO truncate).
    #[test]
    fn user_started_speaking_maps_to_speech_then_interrupt() {
        let p = proto(&base_cfg());
        match p
            .map_server_event(Inbound::Text(r#"{"type":"UserStartedSpeaking"}"#))
            .as_slice()
        {
            [
                S2sEvent::Speech(SpeechEvent::Started {
                    audio_start_ms,
                    item_id,
                }),
                S2sEvent::InterruptedByServer,
            ] => {
                assert_eq!(*audio_start_ms, 0);
                assert!(item_id.is_none());
            }
            other => panic!("expected [Speech(Started), InterruptedByServer], got {other:?}"),
        }
    }

    /// map_server_event: AgentThinking / AgentStartedSpeaking ⇒ Ignore (informational).
    #[test]
    fn agent_lifecycle_events_map_to_ignore() {
        let p = proto(&base_cfg());
        assert!(matches!(
            p.map_server_event(Inbound::Text(r#"{"type":"AgentThinking"}"#))
                .as_slice(),
            [S2sEvent::Ignore]
        ));
        assert!(matches!(
            p.map_server_event(Inbound::Text(
                r#"{"type":"AgentStartedSpeaking","total_latency":0.5}"#
            ))
            .as_slice(),
            [S2sEvent::Ignore]
        ));
    }

    /// map_server_event: FunctionCallRequest ⇒ one FunctionCall per function.
    #[test]
    fn function_call_request_maps_per_function() {
        let p = proto(&base_cfg());
        let raw = r#"{"type":"FunctionCallRequest","functions":[{"id":"fc1","name":"get_weather","arguments":"{\"city\":\"NYC\"}"},{"id":"fc2","name":"get_time","arguments":"{}"}]}"#;
        match p.map_server_event(Inbound::Text(raw)).as_slice() {
            [S2sEvent::FunctionCall(a), S2sEvent::FunctionCall(b)] => {
                assert_eq!(a.call_id, "fc1");
                assert_eq!(a.name, "get_weather");
                assert_eq!(a.arguments, r#"{"city":"NYC"}"#);
                assert!(a.item_id.is_none());
                assert_eq!(b.call_id, "fc2");
                assert_eq!(b.name, "get_time");
                assert_eq!(b.arguments, "{}");
            }
            other => panic!("expected two FunctionCalls, got {other:?}"),
        }
    }

    /// map_server_event: SERVER-SIDE functions (`client_side:false`) are skipped —
    /// Deepgram executes them, the client must not reply. Only the client-side one
    /// surfaces. (Review wf_c40fc67c #1.)
    #[test]
    fn function_call_request_skips_server_side() {
        let p = proto(&base_cfg());
        let raw = r#"{"type":"FunctionCallRequest","functions":[{"id":"srv","name":"lookup","arguments":"{}","client_side":false},{"id":"cli","name":"get_weather","arguments":"{}","client_side":true}]}"#;
        match p.map_server_event(Inbound::Text(raw)).as_slice() {
            [S2sEvent::FunctionCall(a)] => {
                assert_eq!(a.call_id, "cli");
                assert_eq!(a.name, "get_weather");
            }
            other => panic!("expected only the client-side FunctionCall, got {other:?}"),
        }
        // All-server-side ⇒ no events emitted (empty vec, not Ignore).
        let all_srv = r#"{"type":"FunctionCallRequest","functions":[{"id":"s","name":"x","arguments":"{}","client_side":false}]}"#;
        assert!(p.map_server_event(Inbound::Text(all_srv)).is_empty());
    }

    /// map_server_event: an Error is a provider error (refused settings a refusal), a Warning
    /// a warning, each with the description.
    #[test]
    fn errors_warnings_and_refused_settings() {
        let p = proto(&base_cfg());
        let err = r#"{"type":"Error","description":"bad audio encoding"}"#;
        match p.map_server_event(Inbound::Text(err)).as_slice() {
            [S2sEvent::Error(RealtimeError::ProviderError(d))] => {
                assert_eq!(d, "bad audio encoding");
            }
            other => panic!("expected Error(ProviderError), got {other:?}"),
        }
        let refused = r#"{"type":"Error","description":"Cannot specify agent.language when using the Deepgram V2 (Flux) listen API.","code":"INVALID_SETTINGS"}"#;
        match p.map_server_event(Inbound::Text(refused)).as_slice() {
            [S2sEvent::Error(RealtimeError::Refused(d))] => assert!(d.contains("agent.language")),
            other => panic!("expected Error(Refused), got {other:?}"),
        }
        // Warning uses a `message` field instead of `description`.
        let warn = r#"{"type":"Warning","message":"deprecated model"}"#;
        match p.map_server_event(Inbound::Text(warn)).as_slice() {
            [
                S2sEvent::Error(RealtimeError::ProviderWarning {
                    code: None,
                    message,
                }),
            ] => {
                assert_eq!(message, "deprecated model");
            }
            other => panic!("expected Error(ProviderWarning), got {other:?}"),
        }
    }

    /// The probed warning of an injected line the agent ignored while replying: the client
    /// gets the warning; the gateway's own line is said refused (the interaction executor sends
    /// it again), the client's line goes out again from here when the agent's reply is done.
    #[test]
    fn a_line_the_agent_ignored_goes_out_again_after_its_reply() {
        let p = proto(&base_cfg());
        let ignored = r#"{"type":"Warning","description":"Ignored InjectAgentMessage because the agent is already in the middle of a reply.","code":"INJECT_AGENT_MESSAGE_DURING_AGENT_SPEECH"}"#;
        let done = r#"{"type":"AgentAudioDone"}"#;
        // The gateway's verb.
        p.verb(&RealtimeVerb::SayExact("One moment.")).unwrap();
        match p.map_server_event(Inbound::Text(ignored)).as_slice() {
            [
                S2sEvent::Error(RealtimeError::ProviderWarning {
                    code: Some(code),
                    message,
                }),
                S2sEvent::InjectionRefused,
            ] => {
                assert_eq!(code, INJECT_DURING_SPEECH);
                assert!(message.contains("middle of a reply"));
            }
            other => panic!("expected the warning and the refusal, got {other:?}"),
        }
        assert!(matches!(
            p.map_server_event(Inbound::Text(done)).as_slice(),
            [S2sEvent::ResponseDone { .. }]
        ));
        // The client's typed line.
        let sent = match p.send_text("Our kitchen closes at ten.").as_slice() {
            [DeepgramClientMsg::Json(v)] => v.to_string(),
            other => panic!("expected one JSON frame, got {other:?}"),
        };
        assert!(matches!(
            p.map_server_event(Inbound::Text(ignored)).as_slice(),
            [S2sEvent::Error(RealtimeError::ProviderWarning { .. })]
        ));
        match p.map_server_event(Inbound::Text(done)).as_slice() {
            [
                S2sEvent::ResponseDone { .. },
                S2sEvent::SendFrame(OutFrame::Text(again)),
            ] => {
                assert_eq!(again, &sent);
            }
            other => panic!("expected the reply's end and the line again, got {other:?}"),
        }
        // Owed once: the next reply's end sends nothing.
        assert!(matches!(
            p.map_server_event(Inbound::Text(done)).as_slice(),
            [S2sEvent::ResponseDone { .. }]
        ));
    }

    /// Unknown JSON + unparseable text ⇒ Ignore.
    #[test]
    fn unknown_and_unparseable_map_to_ignore() {
        let p = proto(&base_cfg());
        assert!(matches!(
            p.map_server_event(Inbound::Text(r#"{"type":"KeepAliveAck"}"#))
                .as_slice(),
            [S2sEvent::Ignore]
        ));
        assert!(matches!(
            p.map_server_event(Inbound::Text("not json")).as_slice(),
            [S2sEvent::Ignore]
        ));
    }

    /// send_text ⇒ InjectAgentMessage (best-effort text injection); empty
    /// turn/cancel (server VAD owns them).
    #[test]
    fn send_text_injects_and_turn_controls_are_empty() {
        let p = proto(&base_cfg());
        match p.send_text("speak this").as_slice() {
            [DeepgramClientMsg::Json(v)] => {
                assert_eq!(v["type"], "InjectAgentMessage");
                assert_eq!(v["content"], "speak this");
            }
            other => panic!("expected one InjectAgentMessage, got {other:?}"),
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

    /// The interaction verbs use the documented injection wires: `SayExact` makes the agent
    /// speak the words (`InjectAgentMessage.message`), `SayGuided` is a user message the agent
    /// answers (`InjectUserMessage.content`). No wire for the other two.
    #[test]
    fn verbs_say_exact_and_say_guided_have_wires() {
        let p = proto(&base_cfg());
        match p.verb(&RealtimeVerb::SayExact("One moment.")).as_deref() {
            Some([DeepgramClientMsg::Json(v)]) => assert_eq!(
                *v,
                json!({"type": "InjectAgentMessage", "message": "One moment."})
            ),
            other => panic!("expected one InjectAgentMessage, got {other:?}"),
        }
        match p
            .verb(&RealtimeVerb::SayGuided("Say you are checking."))
            .as_deref()
        {
            Some([DeepgramClientMsg::Json(v)]) => assert_eq!(
                *v,
                json!({"type": "InjectUserMessage", "content": "Say you are checking."})
            ),
            other => panic!("expected one InjectUserMessage, got {other:?}"),
        }
        assert!(p.verb(&RealtimeVerb::AddContext("x")).is_none());
        assert!(p.verb(&RealtimeVerb::StopOutput).is_none());
    }

    /// format_tool_result ⇒ FunctionCallResponse echoing the call id + content.
    #[test]
    fn format_tool_result_builds_function_call_response() {
        let p = proto(&base_cfg());
        let call = r#"{"type":"FunctionCallRequest","functions":[{"id":"fc1","name":"get_weather","arguments":"{}","client_side":true}]}"#;
        p.map_server_event(Inbound::Text(call));
        // The probed wire: `name` is required (without it Deepgram answers
        // UNPARSABLE_CLIENT_MESSAGE), and an answered call is forgotten.
        let response =
            |p: &DeepgramProtocol| match p.format_tool_result("fc1", "72F sunny").as_slice() {
                [DeepgramClientMsg::Json(v)] => v.clone(),
                other => panic!("expected FunctionCallResponse, got {other:?}"),
            };
        assert_eq!(
            response(&p),
            json!({"type": "FunctionCallResponse", "id": "fc1", "name": "get_weather", "content": "72F sunny"})
        );
        assert_eq!(response(&p)["name"], "");
    }

    /// The session config names the think model's vendor (given, or by the model's name), the
    /// listen model, the language and, with the `deepgram_tools` fix or in interaction mode,
    /// the functions; the region picks the host.
    /// Fix `deepgram_update_prompt` (escape flag `deepgram_settings_resend`): a changed prompt
    /// or voice mid-session is `UpdatePrompt` / `UpdateSpeak`; a change `Settings` alone could
    /// make is refused; without the fix the whole `Settings` goes again (today).
    #[test]
    fn a_mid_session_update_is_update_prompt_and_update_speak_with_the_fix() {
        let old = base_cfg();
        let mut new = base_cfg();
        new.instructions = Some("Answer in French.".into());
        new.voice = Some("aura-2-luna-en".into());
        let json = |wires: Vec<DeepgramClientMsg>| -> Vec<Value> {
            wires
                .into_iter()
                .map(|wire| match wire {
                    DeepgramClientMsg::Json(v) => v,
                    DeepgramClientMsg::Audio(_) => panic!("no audio"),
                })
                .collect()
        };
        let today = json(proto(&old).session_update(&old, &new).unwrap());
        assert_eq!(today.len(), 1);
        assert_eq!(today[0]["type"], "Settings");

        let mut fixed_cfg = base_cfg();
        fixed_cfg.provider_fixes = vec!["deepgram_update_prompt".into()];
        let fixed = proto(&fixed_cfg);
        assert_eq!(
            json(fixed.session_update(&old, &new).unwrap()),
            [
                json!({ "type": "UpdatePrompt", "prompt": "Answer in French." }),
                json!({ "type": "UpdateSpeak",
                        "speak": { "provider": { "type": "deepgram", "model": "aura-2-luna-en" } } }),
            ]
        );
        assert!(fixed.session_update(&old, &old).unwrap().is_empty());
        let mut other = base_cfg();
        other.model = "gpt-4.1".into();
        assert!(fixed.session_update(&old, &other).is_none());
        let acked = fixed.map_server_event(Inbound::Text(r#"{"type":"PromptUpdated"}"#));
        assert!(matches!(acked.as_slice(), [S2sEvent::Ignore]));
    }

    #[test]
    fn the_session_config_chooses_think_listen_language_functions_and_region() {
        let think = |model: &str, given: Option<&str>| {
            let cfg = RealtimeConfig {
                model: model.into(),
                think_provider: given.map(Into::into),
                ..base_cfg()
            };
            proto(&cfg).settings(&cfg)["agent"]["think"]["provider"].clone()
        };
        assert_eq!(
            think("claude-haiku-4-5", None),
            json!({"type": "anthropic", "model": "claude-haiku-4-5"})
        );
        assert_eq!(think("gemini-2.5-flash", None)["type"], "google");
        assert_eq!(think("openai/gpt-oss-20b", None)["type"], "groq");
        assert_eq!(
            think("nvidia/nemotron-3.5-lightning-30b-a3b", None)["type"],
            "nvidia"
        );
        assert_eq!(think("gpt-4.1-mini", None)["type"], "open_ai");
        assert_eq!(
            think("my-model", Some("aws_bedrock"))["type"],
            "aws_bedrock"
        );
        let listen = |model: &str| {
            let cfg = RealtimeConfig {
                language: Some("de-DE".into()),
                input_audio_transcription: Some(crate::core::realtime::InputTranscriptionConfig {
                    model: model.into(),
                }),
                ..base_cfg()
            };
            proto(&cfg).settings(&cfg)["agent"].clone()
        };
        let v = listen("nova-3");
        assert_eq!(v["language"], "de");
        assert_eq!(v["listen"]["provider"]["model"], "nova-3");
        // A Flux listen model takes no `agent.language` (Deepgram: INVALID_SETTINGS).
        for flux in ["flux-general-en", "flux-general-multi"] {
            let v = listen(flux);
            assert_eq!(v["listen"]["provider"]["model"], flux);
            assert!(v.get("language").is_none(), "{flux}: {v}");
        }
        // OpenAI's default transcription model the handler fills in is no listen model.
        let cfg = RealtimeConfig {
            language: Some("auto".into()),
            input_audio_transcription: Some(crate::core::realtime::InputTranscriptionConfig {
                model: "whisper-1".into(),
            }),
            ..base_cfg()
        };
        let v = proto(&cfg).settings(&cfg);
        assert_eq!(v["agent"]["listen"]["provider"]["model"], "nova-3");
        assert_eq!(v["agent"]["language"], "multi");
        // Functions: off by default, on with the fix, on when the delegate is among them.
        let tool = |name: &str| crate::core::realtime::ToolDefinition {
            tool_type: "function".into(),
            function: crate::core::realtime::FunctionDefinition {
                name: name.into(),
                description: Some("Check a table.".into()),
                parameters: None,
            },
        };
        let with = |names: &[&str], fixes: &[&str]| {
            let cfg = RealtimeConfig {
                tools: Some(names.iter().map(|n| tool(n)).collect()),
                provider_fixes: fixes.iter().map(|f| (*f).into()).collect(),
                ..base_cfg()
            };
            proto(&cfg).settings(&cfg)["agent"]["think"]
                .get("functions")
                .cloned()
        };
        assert_eq!(with(&["check_table"], &[]), None);
        assert_eq!(
            with(&["check_table"], &["deepgram_tools"]),
            Some(
                json!([{"name": "check_table", "description": "Check a table.",
                         "parameters": {"type": "object", "properties": {}}}])
            )
        );
        let delegate = crate::core::interaction::provider::DELEGATE_TOOL;
        assert_eq!(
            with(&["check_table", delegate], &[])
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            2
        );
        // Region.
        let url = |region: Option<&str>| {
            let cfg = RealtimeConfig {
                region: region.map(Into::into),
                ..base_cfg()
            };
            match proto(&cfg).connect_spec(&cfg) {
                Ok(ConnectSpec::WebSocket { url, .. }) => url,
                Ok(_) => panic!("expected WebSocket"),
                Err(e) => e.to_string(),
            }
        };
        assert_eq!(url(None), DEEPGRAM_AGENT_URL);
        assert_eq!(url(Some("US")), DEEPGRAM_AGENT_URL);
        assert_eq!(
            url(Some("eu")),
            "wss://api.eu.deepgram.com/v1/agent/converse"
        );
        assert!(url(Some("mars")).contains("not known"));
    }

    /// Provider fix `deepgram_user_text`. Off (no fix in force): today's frame, byte for byte.
    /// On: Deepgram's documented user injection, `{"type":"InjectUserMessage","content":"..."}`
    /// (<https://developers.deepgram.com/docs/voice-agent-inject-user-message>), so the agent
    /// answers the typed text instead of reading it aloud. The verbs do not change.
    #[test]
    fn the_user_text_fix_sends_typed_text_as_the_users() {
        let text = |p: &DeepgramProtocol| match p.serialize(&p.send_text("Is it open?")[0]) {
            Ok(OutFrame::Text(s)) => s,
            other => panic!("expected a text frame, got {other:?}"),
        };
        let off = proto(&base_cfg());
        assert_eq!(
            text(&off),
            serde_json::to_string(&json!({"type": "InjectAgentMessage", "content": "Is it open?"}))
                .unwrap()
        );
        let cfg = RealtimeConfig {
            provider_fixes: vec!["deepgram_user_text".into()],
            ..base_cfg()
        };
        let on = proto(&cfg);
        assert_eq!(
            text(&on),
            serde_json::to_string(&json!({"type": "InjectUserMessage", "content": "Is it open?"}))
                .unwrap()
        );
        assert_eq!(on.send_text("x").len(), 1);
        for verb in [RealtimeVerb::SayExact("a"), RealtimeVerb::SayGuided("b")] {
            let wire = |p: &DeepgramProtocol| format!("{:?}", p.verb(&verb));
            assert_eq!(wire(&off), wire(&on));
        }
        // The session config is unchanged by the fix.
        let settings = |p: &DeepgramProtocol, cfg: &RealtimeConfig| {
            p.serialize(&p.build_session_config(cfg, None)[0])
                .map(|f| format!("{f:?}"))
                .unwrap()
        };
        assert_eq!(settings(&off, &base_cfg()), settings(&on, &cfg));
    }
}
