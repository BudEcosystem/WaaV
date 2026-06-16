//! Speechmatics Flow [`RealtimeProtocol`] — the conversational / speech-to-speech
//! API on top of Speechmatics' streaming STT. RAW-BINARY-frame audio (like
//! Deepgram/Ultravox), NOT base64+JSON.
//!
//! Speechmatics Flow ("Voice AI") is a single bidirectional WebSocket that runs
//! the whole S2S loop server-side: STT → LLM → TTS with server-side VAD +
//! turn-taking + barge-in. The agent's behaviour (language, prompt, LLM, TTS
//! voice, custom dictionary) is bound to a *template* created in the Speechmatics
//! Portal and selected by `template_id`. Like Deepgram (and unlike the OpenAI
//! family), [`SpeechmaticsProtocol`] streams user mic audio as RAW PCM BINARY
//! frames and receives agent TTS as RAW PCM BINARY frames; only the
//! `StartConversation` config + transcript/lifecycle/control events are JSON.
//!
//! # API Reference (authoritative wire — RESEARCHED from the official
//! `speechmatics/speechmatics-flow` Python SDK + Flow API-reference docs; NO
//! Speechmatics Flow key held, so unit-validated. Every field that could not be
//! pinned VERBATIM is FLAGGED `[FLAG …]` here AND in the implementation report.)
//!
//! - Endpoint: `wss://flow.api.speechmatics.com/v1/flow`. The SDK takes a full
//!   `ConnectionSettings.url` (its docstring example is `wss://localhost:9000`);
//!   the PRODUCTION host `flow.api.speechmatics.com` is confirmed from the docs.
//!   `[FLAG]` the exact PATH (`/v1/flow`) is the documented connect path but was
//!   not visible in raw SDK source — overridable via `cfg.endpoint`.
//! - Auth: `Authorization: Bearer <token>` (SDK: `extra_headers["Authorization"]
//!   = f"Bearer {auth_token}"`). The token is a Speechmatics temporary/JWT token.
//!   The SDK optionally mints one via `GET https://mp.speechmatics.com/v1/api_keys
//!   ?type=flow` (Bearer <api_key>). `[DECISION]` WaaV passes `cfg.api_key` AS THE
//!   BEARER TOKEN directly — the caller supplies either a long-lived JWT or a
//!   pre-minted temp token. `[FLAG]` the in-gateway management-platform token
//!   EXCHANGE is NOT performed (a static api-key passed as Bearer may be rejected
//!   live; a JWT/temp-token will work).
//! - `StartConversation` (client→server JSON):
//!   `{message:"StartConversation", audio_format:{type:"raw", encoding:"pcm_s16le",
//!   sample_rate:16000}, conversation_config:{template_id, template_variables?}}`
//!   — VERBATIM from `models.py` (`AudioSettings.asdict`, `ConversationConfig`).
//! - Audio in: RAW PCM BINARY frames (SDK: `await websocket.send(audio_chunk)`),
//!   acked by the server with `AudioAdded`. Audio out: RAW PCM BINARY frames
//!   (SDK: `if isinstance(message, (bytes, bytearray)): … ServerMessageType.AddAudio`),
//!   which the CLIENT acks with `AudioReceived`.
//! - Server events (`ServerMessageType`, VERBATIM from `models.py`):
//!   `ConversationStarted`, `AudioAdded`, `AddPartialTranscript`, `AddTranscript`,
//!   `ResponseStarted`, `ResponseCompleted`, `ResponseInterrupted`, `prompt`,
//!   `ToolInvoke`, `ConversationEnding`, `ConversationEnded`, `Info`, `Warning`,
//!   `Error`, `Debug`.
//! - Transcript shape (Speechmatics RT schema, shared with streaming STT):
//!   `{message, format, metadata:{transcript, start_time, end_time}, results:[…]}`
//!   — `metadata.transcript` is the full text. AddTranscript/AddPartialTranscript
//!   are the USER's speech. `[FLAG]` `metadata.transcript` field path verified
//!   against the RT docs, not a live Flow capture.
//! - `ResponseStarted`/`ResponseCompleted`/`ResponseInterrupted` carry the AGENT's
//!   spoken text in a `content` field (docs: "contains the textual content of the
//!   utterance"). `[FLAG]` the field NAME `content` is the documented description
//!   but not byte-verified.
//! - Docs: <https://docs.speechmatics.com/flow-api-ref>,
//!   <https://github.com/speechmatics/speechmatics-flow>

use bytes::Bytes;
use serde_json::{json, Value};

use crate::core::realtime::base::{
    FunctionCallRequest, RealtimeConfig, RealtimeError, RealtimeResponseOverride, RealtimeResult,
    TranscriptRole,
};
use crate::core::realtime::scaffold::{
    ConnectSpec, Inbound, OutFrame, ProtocolCaps, RealtimeProtocol, S2sEvent,
};

/// Speechmatics Flow WebSocket endpoint. Single bidi socket for the whole
/// STT→LLM→TTS loop. Host confirmed from the docs; path is the documented connect
/// path. `[FLAG]` overridable via `cfg.endpoint`.
pub(crate) const SPEECHMATICS_FLOW_URL: &str = "wss://flow.api.speechmatics.com/v1/flow";

/// PCM s16le @ 16 kHz: the Flow SDK default (`AudioSettings.sample_rate = 16000`,
/// `encoding = "pcm_s16le"`). Used for BOTH input + output. `[FLAG]` Flow's OUTPUT
/// rate is assumed to match the input/SDK default 16 kHz; not separately verified.
const DEFAULT_SAMPLE_RATE: u32 = 16_000;

/// SDK default input encoding (`AudioSettings.encoding`). The only other allowed
/// value is `pcm_f32le`.
const DEFAULT_ENCODING: &str = "pcm_s16le";

/// Default conversation template. The SDK defaults `ConversationConfig.template_id`
/// to `"default"`; a portal-defined template id (e.g.
/// `flow-service-assistant-amelia`) is the real-world value.
const DEFAULT_TEMPLATE_ID: &str = "default";

/// pcm_s16le @ 16 kHz is mono 16-bit ⇒ 2 bytes/sample × 16 samples/ms = 32 B/ms.
/// Drives the barge-in truncate math on the OUTPUT rate.
const OUTPUT_BYTES_PER_MS: u64 = 32;

/// One typed outbound Flow wire message. The defining BINARY split: `Json` ⇒ a
/// text frame (`StartConversation` / `AudioReceived` ack / `ToolResult`), `Audio`
/// ⇒ a RAW PCM binary frame (user mic audio — NOT base64/JSON, the implicit
/// `AddAudio`).
#[derive(Debug, Clone)]
pub enum SpeechmaticsClientMsg {
    /// A JSON control/config message (`StartConversation`, `AudioReceived`,
    /// `ToolResult`, `AudioEnded`).
    Json(Value),
    /// A raw PCM audio frame (the binary `AddAudio` path).
    Audio(Bytes),
}

/// The Speechmatics Flow protocol. Stateless: the template/instructions/voice/
/// sample-rate it needs come from `cfg` (re-passed into `build_session_config`),
/// so it carries only what `from_config` resolved + validated.
pub struct SpeechmaticsProtocol {
    /// Cached for `caps()`; the live output rate (mirrors the declared input rate).
    output_sample_rate: u32,
}

impl SpeechmaticsProtocol {
    /// Resolve the sample rate. Flow takes a single integer for input; WaaV's
    /// `RealtimeConfig` has no per-rate field, so the SDK default 16 kHz is used.
    fn sample_rate(_cfg: &RealtimeConfig) -> u32 {
        DEFAULT_SAMPLE_RATE
    }

    /// Resolve the conversation `template_id`: `cfg.model` (Flow's "model" IS the
    /// portal template), else `"default"`.
    fn template_id(cfg: &RealtimeConfig) -> String {
        let m = cfg.model.trim();
        if m.is_empty() {
            DEFAULT_TEMPLATE_ID.to_string()
        } else {
            m.to_string()
        }
    }

    /// Build the `StartConversation` JSON from the generic config:
    /// `model` → `conversation_config.template_id`, `instructions` →
    /// `template_variables.persona`, `audio_format` raw pcm_s16le @ rate. Shape
    /// matches the SDK (`models.py`) exactly.
    pub fn start_conversation(&self, cfg: &RealtimeConfig) -> Value {
        let rate = Self::sample_rate(cfg);

        let mut conversation_config = json!({ "template_id": Self::template_id(cfg) });

        // `instructions` → `template_variables.persona`. Flow templates expose
        // overridable variables (the SDK's `--config-file` example uses
        // `persona`/`style`/`context`); `persona` is the closest analogue to a
        // system prompt. `[FLAG]` the variable NAME is template-defined — a
        // template without a `persona` variable will ignore this override.
        if let Some(instr) = cfg.instructions.as_deref()
            && !instr.is_empty()
        {
            conversation_config["template_variables"] = json!({ "persona": instr });
        }

        json!({
            "message": "StartConversation",
            "audio_format": {
                "type": "raw",
                "encoding": DEFAULT_ENCODING,
                "sample_rate": rate,
            },
            "conversation_config": conversation_config,
        })
    }

    /// `{"message":"AudioReceived","seq_no":<n>,"buffering":<secs>}` — the CLIENT
    /// ack for an inbound binary audio frame (the SDK sends this per received audio
    /// chunk). `buffering:0` = WaaV holds no client-side jitter buffer (it forwards
    /// output audio straight through). We use seq_no 0 (the stateless SendFrame ack
    /// path tracks no per-frame counter); the server is tolerant of the content.
    /// `[FLAG]` a strict server may require a monotonically increasing seq_no.
    fn audio_received_ack() -> Value {
        json!({ "message": "AudioReceived", "seq_no": 0, "buffering": 0 })
    }
}

impl RealtimeProtocol for SpeechmaticsProtocol {
    type Wire = SpeechmaticsClientMsg;

    fn from_config(cfg: &RealtimeConfig) -> RealtimeResult<Self> {
        // Flow uses `Authorization: Bearer <token>` — a non-empty token (JWT /
        // temp-token, supplied as `cfg.api_key`) is required.
        if cfg.api_key.trim().is_empty() {
            return Err(RealtimeError::AuthenticationFailed(
                "API key (Bearer token) is required for Speechmatics Flow".to_string(),
            ));
        }
        Ok(Self {
            output_sample_rate: Self::sample_rate(cfg),
        })
    }

    fn provider_id(&self) -> &'static str {
        "speechmatics"
    }

    fn caps(&self) -> ProtocolCaps {
        ProtocolCaps {
            // Flow runs server-side VAD + turn-taking, so it owns turns and emits
            // user-turn frames (`AddTranscript`, `ResponseStarted`).
            emits_user_turn_frames: true,
            output_bytes_per_ms: OUTPUT_BYTES_PER_MS,
            output_sample_rate: self.output_sample_rate,
            // Flow has NO server-side item truncation (barge-in is handled
            // server-side via `ResponseInterrupted`; the gateway only clears local
            // playback).
            supports_truncate: false,
            // No gateway-managed input buffer (raw binary audio is streamed).
            supports_input_buffer: false,
        }
    }

    fn connect_spec(&self, cfg: &RealtimeConfig) -> RealtimeResult<ConnectSpec> {
        // `cfg.endpoint` overrides the default URL (on-prem / preview / path
        // changes). Auth is `Bearer <token>`; the standard WS-upgrade headers are
        // generated by `into_client_request()` in the transport factory.
        let url = cfg
            .endpoint
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(SPEECHMATICS_FLOW_URL)
            .to_string();
        Ok(ConnectSpec::WebSocket {
            url,
            headers: vec![(
                "Authorization".to_string(),
                format!("Bearer {}", cfg.api_key),
            )],
        })
    }

    fn build_session_config(
        &self,
        cfg: &RealtimeConfig,
        _resumption: Option<&str>,
    ) -> Vec<Self::Wire> {
        // The single `StartConversation` message, sent on connect (and re-sent on
        // reconnect — Flow re-applies it on a fresh socket).
        vec![SpeechmaticsClientMsg::Json(self.start_conversation(cfg))]
    }

    fn map_server_event(&self, raw: Inbound<'_>) -> Vec<S2sEvent> {
        // Binary frames are agent TTS audio (raw pcm_s16le @ 16 kHz). No base64 —
        // copy the bytes straight through, and emit an `AudioReceived` ack frame
        // (the SDK acks every inbound audio chunk).
        let text = match raw {
            Inbound::Binary(b) => {
                return vec![
                    S2sEvent::Audio {
                        data: Bytes::copy_from_slice(b),
                        item_id: None,
                        response_id: None,
                    },
                    // The driver routes a SendFrame to the transport (not a callback).
                    S2sEvent::SendFrame(OutFrame::Text(
                        serde_json::to_string(&Self::audio_received_ack())
                            .unwrap_or_else(|_| "{}".to_string()),
                    )),
                ];
            }
            Inbound::Text(t) => t,
        };

        let value: Value = match serde_json::from_str(text) {
            Ok(v) => v,
            Err(_) => return vec![S2sEvent::Ignore],
        };
        // Flow tags every JSON event with a top-level `message` field.
        let msg_type = value.get("message").and_then(Value::as_str).unwrap_or("");

        match msg_type {
            // Session ack: `{"message":"ConversationStarted","id":"<uuid>", …}`.
            // `[FLAG]` the id field name (`id`/`conversation_id`/`request_id`) is
            // not byte-verified; both are probed.
            "ConversationStarted" => vec![S2sEvent::SessionReady {
                session_id: value
                    .get("id")
                    .or_else(|| value.get("conversation_id"))
                    .or_else(|| value.get("request_id"))
                    .and_then(Value::as_str)
                    .map(str::to_string),
            }],
            // FINAL user transcript: the Speechmatics RT schema carries the full
            // text at `metadata.transcript`. AddTranscript = settled USER speech.
            "AddTranscript" => vec![S2sEvent::Transcript {
                role: TranscriptRole::User,
                text: transcript_text(&value),
                is_final: true,
                item_id: None,
            }],
            // PARTIAL user transcript (immediate, may change) = non-final USER speech.
            "AddPartialTranscript" => vec![S2sEvent::Transcript {
                role: TranscriptRole::User,
                text: transcript_text(&value),
                is_final: false,
                item_id: None,
            }],
            // ResponseStarted: the agent is about to speak; the message carries the
            // full text of the utterance. Surface as a NON-FINAL assistant
            // transcript (the matching final arrives with ResponseCompleted).
            "ResponseStarted" => vec![S2sEvent::Transcript {
                role: TranscriptRole::Assistant,
                text: response_content(&value),
                is_final: false,
                item_id: None,
            }],
            // ResponseCompleted: the agent finished THIS response's audio. Carries
            // the textual content just spoken ⇒ a FINAL assistant transcript, then
            // ResponseDone (generation finished; NOT a playout/clear signal). Flow
            // gives no response id.
            "ResponseCompleted" => vec![
                S2sEvent::Transcript {
                    role: TranscriptRole::Assistant,
                    text: response_content(&value),
                    is_final: true,
                    item_id: None,
                },
                S2sEvent::ResponseDone {
                    response_id: String::new(),
                },
            ],
            // ResponseInterrupted: the user barged in; the server already stopped
            // agent audio. Drive the driver's cancel + local-playback clear. NO
            // truncate — Flow has none.
            "ResponseInterrupted" => vec![S2sEvent::InterruptedByServer],
            // Tool/function call: `{"message":"ToolInvoke","id":…,"function":
            // {"name":…,"arguments":…}}`. `[FLAG]` the nesting (`function.name` vs
            // top-level `name`) is not byte-verified; both shapes are probed.
            "ToolInvoke" => {
                let id = value
                    .get("id")
                    .or_else(|| value.get("tool_call_id"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let func = value.get("function");
                let name = func
                    .and_then(|f| f.get("name"))
                    .or_else(|| value.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let arguments = match func.and_then(|f| f.get("arguments")).or_else(|| value.get("arguments")) {
                    Some(Value::String(s)) => s.clone(),
                    Some(other) => other.to_string(),
                    None => String::new(),
                };
                vec![S2sEvent::FunctionCall(FunctionCallRequest {
                    call_id: id,
                    name,
                    arguments,
                    item_id: None,
                })]
            }
            // `AudioAdded` (server ack of our outbound audio) + the conversation-end
            // lifecycle (`ConversationEnding`/`ConversationEnded`) + the LLM-debug
            // `prompt`/`Debug`/`Info` + `AudioEnded` ⇒ nothing actionable.
            "AudioAdded" | "ConversationEnding" | "ConversationEnded" | "prompt" | "Debug"
            | "Info" | "AudioEnded" => vec![S2sEvent::Ignore],
            // Error / Warning: surface the reason/detail/type. NOTE: `message` is
            // Flow's event-type DISCRIMINATOR key (its value is "Error"/"Warning"),
            // NOT a human detail — so it is deliberately NOT in this fallback chain.
            // `[FLAG]` the detail field name (`reason` vs `detail`) is not
            // byte-verified; `type`/`code` are the documented Flow error fields.
            "Error" | "Warning" => {
                let detail = value
                    .get("reason")
                    .and_then(Value::as_str)
                    .or_else(|| value.get("detail").and_then(Value::as_str))
                    .or_else(|| value.get("type").and_then(Value::as_str))
                    .or_else(|| value.get("code").and_then(Value::as_str))
                    .unwrap_or("unknown Speechmatics Flow error")
                    .to_string();
                vec![S2sEvent::Error(RealtimeError::ProviderError(detail))]
            }
            // Unknown/unhandled ⇒ nothing actionable.
            _ => vec![S2sEvent::Ignore],
        }
    }

    fn encode_user_audio(&self, pcm: &[u8]) -> Self::Wire {
        // THE binary path: raw pcm_s16le as a binary frame — no base64/JSON
        // (the implicit `AddAudio`).
        SpeechmaticsClientMsg::Audio(Bytes::copy_from_slice(pcm))
    }

    fn send_text(&self, text: &str) -> Vec<Self::Wire> {
        // Flow's application-input surface is `AddInput` (a message routed to the
        // LLM). `immediate:false` queues it as user input without forcing an
        // interruption. (SDK: `AddInput.asdict`.)
        vec![SpeechmaticsClientMsg::Json(json!({
            "message": "AddInput",
            "input": text,
            "interrupt_response": false,
            "immediate": false,
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
        // Flow has no client-driven response cancel; barge-in is handled
        // server-side (`ResponseInterrupted`). Safest to send nothing.
        Vec::new()
    }

    fn format_tool_result(&self, call_id: &str, result: &str) -> Vec<Self::Wire> {
        // Tool result → `ToolResult` (SDK `ClientMessageType.ToolResult`). The `id`
        // echoes the request's tool id; `content` carries the result string. The
        // `status` enum is `ok`/`failed`/`rejected` (Flow docs) — "ok" for the
        // success path WaaV drives here ("success" is not a valid value and a
        // strict server rejects it; review wf_ca3a8537 #1). `[FLAG]` the `content`
        // vs `result` body key is not byte-verified.
        vec![SpeechmaticsClientMsg::Json(json!({
            "message": "ToolResult",
            "id": call_id,
            "status": "ok",
            "content": result,
        }))]
    }

    fn serialize(&self, msg: &Self::Wire) -> RealtimeResult<OutFrame> {
        match msg {
            SpeechmaticsClientMsg::Json(v) => Ok(OutFrame::Text(
                serde_json::to_string(v)
                    .map_err(|e| RealtimeError::SerializationError(e.to_string()))?,
            )),
            // THE binary path: audio frames serialize to a raw binary frame.
            SpeechmaticsClientMsg::Audio(b) => Ok(OutFrame::Binary(b.clone())),
        }
    }
}

/// Extract the full transcript text from an Add[Partial]Transcript event. The
/// Speechmatics RT schema puts the formatted text at `metadata.transcript`;
/// fall back to joining `results[].alternatives[0].content` if absent.
fn transcript_text(value: &Value) -> String {
    if let Some(t) = value
        .get("metadata")
        .and_then(|m| m.get("transcript"))
        .and_then(Value::as_str)
        && !t.is_empty()
    {
        return t.to_string();
    }
    // Fallback: stitch word contents from results[].alternatives[0].content.
    value
        .get("results")
        .and_then(Value::as_array)
        .map(|results| {
            let words: Vec<&str> = results
                .iter()
                .filter_map(|r| {
                    r.get("alternatives")
                        .and_then(Value::as_array)
                        .and_then(|alts| alts.first())
                        .and_then(|a| a.get("content"))
                        .and_then(Value::as_str)
                })
                .collect();
            words.join(" ")
        })
        .unwrap_or_default()
}

/// Extract the agent's spoken text from a Response* event. The docs describe a
/// `content` field with the utterance text; fall back to `text`/`transcript`.
fn response_content(value: &Value) -> String {
    value
        .get("content")
        .or_else(|| value.get("text"))
        .or_else(|| value.get("transcript"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

// =============================================================================
// Tests (NO Speechmatics Flow key needed — pure config/event mapping)
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::realtime::base::ReplayConversationItem;

    fn base_cfg() -> RealtimeConfig {
        RealtimeConfig {
            provider: "speechmatics".into(),
            api_key: "sm-token".into(),
            model: "flow-service-assistant-amelia".into(),
            voice: Some("amelia".into()),
            instructions: Some("You are a helpful butler.".into()),
            ..Default::default()
        }
    }

    fn proto(cfg: &RealtimeConfig) -> SpeechmaticsProtocol {
        SpeechmaticsProtocol::from_config(cfg).unwrap()
    }

    /// from_config rejects an empty api key (`Bearer <token>` auth requires it).
    #[test]
    fn from_config_requires_api_key() {
        let cfg = RealtimeConfig {
            api_key: String::new(),
            ..Default::default()
        };
        assert!(matches!(
            SpeechmaticsProtocol::from_config(&cfg),
            Err(RealtimeError::AuthenticationFailed(_))
        ));
        // Whitespace-only is also rejected.
        let ws = RealtimeConfig {
            api_key: "   ".into(),
            ..Default::default()
        };
        assert!(matches!(
            SpeechmaticsProtocol::from_config(&ws),
            Err(RealtimeError::AuthenticationFailed(_))
        ));
    }

    /// provider_id is the stable "speechmatics" used for the breaker label/metrics.
    #[test]
    fn provider_id_is_speechmatics() {
        assert_eq!(proto(&base_cfg()).provider_id(), "speechmatics");
    }

    /// connect_spec: the Flow URL + `Bearer <token>` (NOT `Token`/`sm-app`).
    #[test]
    fn connect_spec_uses_flow_url_and_bearer_auth() {
        let p = proto(&base_cfg());
        let ConnectSpec::WebSocket { url, headers } = p.connect_spec(&base_cfg()).unwrap() else {
            panic!("expected ConnectSpec::WebSocket")
        };
        assert_eq!(url, "wss://flow.api.speechmatics.com/v1/flow");
        let auth = headers
            .iter()
            .find(|(k, _)| k == "Authorization")
            .map(|(_, v)| v.as_str())
            .unwrap();
        assert_eq!(auth, "Bearer sm-token", "Flow uses Bearer <token>");
    }

    /// connect_spec honors a `cfg.endpoint` override (on-prem / preview / path).
    #[test]
    fn connect_spec_honors_endpoint_override() {
        let cfg = RealtimeConfig {
            endpoint: Some("wss://flow.example.com/v1/flow".into()),
            ..base_cfg()
        };
        let p = proto(&cfg);
        let ConnectSpec::WebSocket { url, .. } = p.connect_spec(&cfg).unwrap() else {
            panic!("expected WebSocket")
        };
        assert_eq!(url, "wss://flow.example.com/v1/flow");
    }

    /// build_session_config serializes to the SDK `StartConversation` shape:
    /// message=StartConversation, audio_format raw pcm_s16le @ 16k, template_id +
    /// template_variables.persona.
    #[test]
    fn build_session_config_matches_start_conversation() {
        let p = proto(&base_cfg());
        let wires = p.build_session_config(&base_cfg(), None);
        assert_eq!(wires.len(), 1);
        let json = match p.serialize(&wires[0]).unwrap() {
            OutFrame::Text(s) => s,
            OutFrame::Binary(_) => panic!("StartConversation must be a text frame"),
        };
        let v: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["message"], "StartConversation");
        assert_eq!(v["audio_format"]["type"], "raw");
        assert_eq!(v["audio_format"]["encoding"], "pcm_s16le");
        assert_eq!(v["audio_format"]["sample_rate"], 16000);
        assert_eq!(
            v["conversation_config"]["template_id"],
            "flow-service-assistant-amelia"
        );
        assert_eq!(
            v["conversation_config"]["template_variables"]["persona"],
            "You are a helpful butler."
        );
    }

    /// Defaults: empty model ⇒ template_id "default"; no instructions ⇒ NO
    /// template_variables (skip-if-none discipline).
    #[test]
    fn start_conversation_applies_defaults_and_skips_empty_vars() {
        let cfg = RealtimeConfig {
            api_key: "sm-token".into(),
            ..Default::default()
        };
        let p = proto(&cfg);
        let v = p.start_conversation(&cfg);
        assert_eq!(v["conversation_config"]["template_id"], "default");
        assert!(
            v["conversation_config"].get("template_variables").is_none(),
            "no instructions ⇒ no template_variables key"
        );
    }

    /// caps: server-VAD turn frames, 32 B/ms @ 16 kHz, NO truncate, NO input buffer.
    #[test]
    fn caps_describe_binary_server_vad_provider() {
        let c = proto(&base_cfg()).caps();
        assert!(c.emits_user_turn_frames);
        assert_eq!(c.output_bytes_per_ms, 32);
        assert_eq!(c.output_sample_rate, 16_000);
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
            SpeechmaticsClientMsg::Audio(b) => assert_eq!(b.as_ref(), pcm.as_slice()),
            SpeechmaticsClientMsg::Json(_) => panic!("audio must be the Audio variant"),
        }
        match p.serialize(&wire).unwrap() {
            OutFrame::Binary(b) => assert_eq!(b.as_ref(), pcm.as_slice(), "raw PCM, byte-exact"),
            OutFrame::Text(_) => panic!("audio must serialize to a binary frame, not text"),
        }
    }

    /// map_server_event: ConversationStarted ⇒ SessionReady carrying the id.
    #[test]
    fn conversation_started_maps_to_session_ready() {
        let p = proto(&base_cfg());
        let raw = r#"{"message":"ConversationStarted","id":"conv-123"}"#;
        match p.map_server_event(Inbound::Text(raw)).as_slice() {
            [S2sEvent::SessionReady { session_id }] => {
                assert_eq!(session_id.as_deref(), Some("conv-123"));
            }
            other => panic!("expected SessionReady, got {other:?}"),
        }
    }

    /// map_server_event: AddTranscript ⇒ FINAL USER transcript from
    /// metadata.transcript; AddPartialTranscript ⇒ non-final USER transcript.
    #[test]
    fn transcripts_map_to_user_role_with_finality() {
        let p = proto(&base_cfg());
        let fin = r#"{"message":"AddTranscript","metadata":{"transcript":"hello there","start_time":0.0,"end_time":1.0},"results":[]}"#;
        match p.map_server_event(Inbound::Text(fin)).as_slice() {
            [S2sEvent::Transcript {
                role,
                text,
                is_final,
                item_id,
            }] => {
                assert_eq!(*role, TranscriptRole::User);
                assert_eq!(text, "hello there");
                assert!(*is_final);
                assert!(item_id.is_none());
            }
            other => panic!("expected final user Transcript, got {other:?}"),
        }
        let part = r#"{"message":"AddPartialTranscript","metadata":{"transcript":"hel"},"results":[]}"#;
        match p.map_server_event(Inbound::Text(part)).as_slice() {
            [S2sEvent::Transcript {
                role,
                text,
                is_final,
                ..
            }] => {
                assert_eq!(*role, TranscriptRole::User);
                assert_eq!(text, "hel");
                assert!(!*is_final);
            }
            other => panic!("expected partial user Transcript, got {other:?}"),
        }
    }

    /// map_server_event: the `results[]` fallback when metadata.transcript is absent.
    #[test]
    fn transcript_falls_back_to_results_contents() {
        let p = proto(&base_cfg());
        let raw = r#"{"message":"AddTranscript","metadata":{},"results":[{"alternatives":[{"content":"hello"}]},{"alternatives":[{"content":"world"}]}]}"#;
        match p.map_server_event(Inbound::Text(raw)).as_slice() {
            [S2sEvent::Transcript { text, .. }] => assert_eq!(text, "hello world"),
            other => panic!("expected Transcript, got {other:?}"),
        }
    }

    /// map_server_event: ResponseStarted ⇒ non-final ASSISTANT transcript (the
    /// agent's planned utterance text).
    #[test]
    fn response_started_maps_to_nonfinal_assistant_transcript() {
        let p = proto(&base_cfg());
        let raw = r#"{"message":"ResponseStarted","content":"Good evening, sir."}"#;
        match p.map_server_event(Inbound::Text(raw)).as_slice() {
            [S2sEvent::Transcript {
                role,
                text,
                is_final,
                ..
            }] => {
                assert_eq!(*role, TranscriptRole::Assistant);
                assert_eq!(text, "Good evening, sir.");
                assert!(!*is_final, "ResponseStarted ⇒ non-final");
            }
            other => panic!("expected assistant Transcript, got {other:?}"),
        }
    }

    /// map_server_event: ResponseCompleted ⇒ [FINAL assistant Transcript,
    /// ResponseDone] (the utterance just spoken + generation done).
    #[test]
    fn response_completed_maps_to_final_transcript_then_done() {
        let p = proto(&base_cfg());
        let raw = r#"{"message":"ResponseCompleted","content":"How may I help?"}"#;
        match p.map_server_event(Inbound::Text(raw)).as_slice() {
            [
                S2sEvent::Transcript {
                    role,
                    text,
                    is_final,
                    ..
                },
                S2sEvent::ResponseDone { response_id },
            ] => {
                assert_eq!(*role, TranscriptRole::Assistant);
                assert_eq!(text, "How may I help?");
                assert!(*is_final);
                assert!(response_id.is_empty());
            }
            other => panic!("expected [final Transcript, ResponseDone], got {other:?}"),
        }
    }

    /// map_server_event: ResponseInterrupted ⇒ InterruptedByServer (barge-in;
    /// driver clears playback; NO truncate).
    #[test]
    fn response_interrupted_maps_to_interrupted() {
        let p = proto(&base_cfg());
        let raw = r#"{"message":"ResponseInterrupted","content":"How may"}"#;
        assert!(matches!(
            p.map_server_event(Inbound::Text(raw)).as_slice(),
            [S2sEvent::InterruptedByServer]
        ));
    }

    /// map_server_event: a raw BINARY frame ⇒ [Audio (byte-exact PCM),
    /// SendFrame(AudioReceived ack)].
    #[test]
    fn binary_frame_maps_to_audio_and_ack() {
        let p = proto(&base_cfg());
        let pcm: &[u8] = &[1, 2, 3, 4, 250, 251];
        match p.map_server_event(Inbound::Binary(pcm)).as_slice() {
            [
                S2sEvent::Audio {
                    data,
                    item_id,
                    response_id,
                },
                S2sEvent::SendFrame(OutFrame::Text(ack)),
            ] => {
                assert_eq!(data.as_ref(), pcm);
                assert!(item_id.is_none());
                assert!(response_id.is_none());
                let v: Value = serde_json::from_str(ack).unwrap();
                assert_eq!(v["message"], "AudioReceived");
            }
            other => panic!("expected [Audio, SendFrame(ack)], got {other:?}"),
        }
    }

    /// map_server_event: ToolInvoke ⇒ FunctionCall (nested `function` shape).
    #[test]
    fn tool_invoke_maps_to_function_call() {
        let p = proto(&base_cfg());
        let raw = r#"{"message":"ToolInvoke","id":"tc-1","function":{"name":"get_weather","arguments":"{\"city\":\"NYC\"}"}}"#;
        match p.map_server_event(Inbound::Text(raw)).as_slice() {
            [S2sEvent::FunctionCall(fc)] => {
                assert_eq!(fc.call_id, "tc-1");
                assert_eq!(fc.name, "get_weather");
                assert_eq!(fc.arguments, r#"{"city":"NYC"}"#);
            }
            other => panic!("expected FunctionCall, got {other:?}"),
        }
    }

    /// map_server_event: AudioAdded / ConversationEnding / ConversationEnded /
    /// prompt / Info ⇒ Ignore (lifecycle/ack/debug).
    #[test]
    fn lifecycle_and_ack_events_map_to_ignore() {
        let p = proto(&base_cfg());
        for raw in [
            r#"{"message":"AudioAdded","seq_no":3}"#,
            r#"{"message":"ConversationEnding"}"#,
            r#"{"message":"ConversationEnded"}"#,
            r#"{"message":"prompt","content":"…"}"#,
            r#"{"message":"Info","type":"recognition_quality"}"#,
        ] {
            assert!(
                matches!(p.map_server_event(Inbound::Text(raw)).as_slice(), [S2sEvent::Ignore]),
                "expected Ignore for {raw}"
            );
        }
    }

    /// map_server_event: Error / Warning ⇒ Error(ProviderError) with the reason.
    #[test]
    fn error_and_warning_map_to_error() {
        let p = proto(&base_cfg());
        let err = r#"{"message":"Error","type":"invalid_audio_type","reason":"bad encoding"}"#;
        match p.map_server_event(Inbound::Text(err)).as_slice() {
            [S2sEvent::Error(RealtimeError::ProviderError(d))] => assert_eq!(d, "bad encoding"),
            other => panic!("expected Error(ProviderError), got {other:?}"),
        }
        // Warning with only a `type` (no reason) ⇒ falls back to the type.
        let warn = r#"{"message":"Warning","type":"unsupported_translation_pair"}"#;
        match p.map_server_event(Inbound::Text(warn)).as_slice() {
            [S2sEvent::Error(RealtimeError::ProviderError(d))] => {
                assert_eq!(d, "unsupported_translation_pair");
            }
            other => panic!("expected Error(ProviderError), got {other:?}"),
        }
    }

    /// Unknown JSON + unparseable text ⇒ Ignore.
    #[test]
    fn unknown_and_unparseable_map_to_ignore() {
        let p = proto(&base_cfg());
        assert!(matches!(
            p.map_server_event(Inbound::Text(r#"{"message":"SomethingNew"}"#))
                .as_slice(),
            [S2sEvent::Ignore]
        ));
        assert!(matches!(
            p.map_server_event(Inbound::Text("not json")).as_slice(),
            [S2sEvent::Ignore]
        ));
    }

    /// send_text ⇒ AddInput; turn/cancel controls empty (server VAD owns them);
    /// defaults (truncate / input-buffer / replay) empty.
    #[test]
    fn send_text_adds_input_and_controls_are_empty() {
        let p = proto(&base_cfg());
        match p.send_text("remember my name is Sam").as_slice() {
            [SpeechmaticsClientMsg::Json(v)] => {
                assert_eq!(v["message"], "AddInput");
                assert_eq!(v["input"], "remember my name is Sam");
                assert_eq!(v["interrupt_response"], false);
            }
            other => panic!("expected one AddInput, got {other:?}"),
        }
        assert!(p.commit_turn().is_empty());
        assert!(p.cancel_response().is_empty());
        assert!(p.create_response(None).is_empty());
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

    /// format_tool_result ⇒ ToolResult echoing the call id + content.
    #[test]
    fn format_tool_result_builds_tool_result() {
        let p = proto(&base_cfg());
        match p.format_tool_result("tc-1", "72F sunny").as_slice() {
            [SpeechmaticsClientMsg::Json(v)] => {
                assert_eq!(v["message"], "ToolResult");
                assert_eq!(v["id"], "tc-1");
                assert_eq!(v["content"], "72F sunny");
                // status is the Flow enum ok/failed/rejected — "ok", NOT "success".
                assert_eq!(v["status"], "ok");
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }

    /// Factory create-by-name via the registry: canonical + alias + case-insensitive.
    #[test]
    fn factory_creates_by_name_case_insensitive() {
        use crate::core::realtime::create_realtime_provider;
        for name in ["speechmatics", "SPEECHMATICS", "Speechmatics", "flow"] {
            let provider = create_realtime_provider(name, base_cfg())
                .unwrap_or_else(|e| panic!("registry failed for {name}: {e:?}"));
            assert_eq!(
                provider.get_provider_info()["provider"],
                "speechmatics",
                "name {name} ⇒ speechmatics"
            );
            assert!(!provider.is_ready());
        }
    }
}
