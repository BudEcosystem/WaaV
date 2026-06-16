//! AWS Nova Sonic Realtime [`RealtimeProtocol`] — the FIRST `BedrockBidi` provider
//! on WaaV's S2S scaffold (an Amazon Bedrock `InvokeModelWithBidirectionalStream`
//! HTTP/2 event stream, NOT a WebSocket).
//!
//! Amazon Nova Sonic (`amazon.nova-sonic-v1:0`) is AWS's speech-to-speech model: a
//! single bidirectional Bedrock stream runs the whole loop server-side (ASR + LLM
//! + TTS) with server VAD + turn-taking + barge-in. Audio is base64 PCM carried
//! INSIDE event JSON (the Bedrock stream is event-framed; there are no separate
//! binary frames). Auth is AWS SigV4 via the `aws-config` default credential chain
//! (env / shared config / IAM role) — there is NO api-key.
//!
//! What makes Nova Sonic unique on the scaffold is the TRANSPORT: the Bedrock bidi
//! stream lives entirely in the
//! [`BedrockBidiTransportFactory`](crate::core::realtime::scaffold::BedrockBidiTransportFactory)
//! (selected via [`transport_factory`](RealtimeProtocol::transport_factory)); the
//! generic driver (`session.rs`) is unchanged. The factory mirrors the in-tree
//! `AwsTranscribeTransport` (another non-WS bidi HTTP/2 event stream behind a
//! transport trait), so adding Nova Sonic is "just another `(protocol, transport)`
//! pair".
//!
//! # API Reference (authoritative wire — verified VERBATIM against the AWS Bedrock
//! / Nova Sonic docs; NO AWS key held, so unit-validated — see field-by-field
//! flags in the implementation report)
//!
//! - Bedrock op: `InvokeModelWithBidirectionalStream`, modelId
//!   `amazon.nova-sonic-v1:0`. Every event is a JSON object wrapped in a top-level
//!   `{"event": { … }}` key.
//! - Client→server sequence: `sessionStart`(inferenceConfiguration) →
//!   `promptStart`(audioOutputConfiguration + toolConfiguration) →
//!   `contentStart`(role:SYSTEM, type:TEXT) + `textInput`(system prompt) +
//!   `contentEnd` → `contentStart`(role:USER, type:AUDIO) + repeated
//!   `audioInput`(base64 PCM) + `contentEnd`.
//! - Server→client: `completionStart` / `contentStart` / `textOutput`(transcript)
//!   / `audioOutput`(base64 PCM) / `toolUse` / `contentEnd` / `completionEnd`. The
//!   transcript ROLE (USER ASR vs ASSISTANT) + `generationStage`
//!   (FINAL/SPECULATIVE) are carried on the `contentStart` that PRECEDES each
//!   `textOutput` (the `textOutput` itself carries only `content` + ids), so the
//!   protocol tracks the current text content's role/stage across those two events.
//! - Docs: <https://docs.aws.amazon.com/nova/latest/userguide/speech.html>,
//!   <https://docs.aws.amazon.com/nova/latest/userguide/input-events.html>,
//!   <https://docs.aws.amazon.com/nova/latest/userguide/output-events.html>

use base64::prelude::*;
use bytes::Bytes;
use serde_json::{json, Value};
use std::sync::Mutex;

use crate::core::realtime::base::{
    FunctionCallRequest, RealtimeConfig, RealtimeError, RealtimeResponseOverride, RealtimeResult,
    TranscriptRole,
};
use crate::core::realtime::scaffold::{
    BedrockBidiTransportFactory, ConnectSpec, Inbound, OutFrame, ProtocolCaps, RealtimeProtocol,
    RealtimeTransportFactory, S2sEvent,
};
use std::sync::Arc;

/// Default Bedrock model id for Nova Sonic (speech-to-speech v1).
pub(crate) const DEFAULT_MODEL: &str = "amazon.nova-sonic-v1:0";

/// Default Nova Sonic voice when none is configured (an English voice from the
/// documented `voiceId` set: matthew | tiffany | amy | …).
const DEFAULT_VOICE: &str = "matthew";

/// Nova Sonic OUTPUT audio: PCM (`audio/lpcm`) 16-bit mono @ 24 kHz (the
/// `audioOutputConfiguration` we request). 24 samples/ms × 2 bytes = 48 B/ms,
/// which drives the barge-in truncate math on the OUTPUT rate.
const OUTPUT_SAMPLE_RATE: u32 = 24_000;

/// Bytes/ms of OUTPUT audio (PCM16 @ 24 kHz). See [`OUTPUT_SAMPLE_RATE`].
const OUTPUT_BYTES_PER_MS: u64 = 48;

/// Nova Sonic INPUT audio sample rate we declare in `audioInputConfiguration`
/// (PCM 16-bit mono @ 16 kHz — the gateway's standard mic rate; Nova Sonic accepts
/// 8/16/24 kHz).
const INPUT_SAMPLE_RATE: u32 = 16_000;

/// The current text-content role/stage tracked across a `contentStart`→`textOutput`
/// pair. Nova Sonic puts the role (USER ASR vs ASSISTANT) + `generationStage`
/// (FINAL vs SPECULATIVE) on the `contentStart`, while the following `textOutput`
/// carries only `content` — so we stash it here on `contentStart` and apply it when
/// the `textOutput` arrives.
#[derive(Clone, Copy)]
struct TextContentState {
    role: TranscriptRole,
    /// `true` ⇒ `generationStage == FINAL` (a settled transcript); `false` ⇒
    /// `SPECULATIVE` (a preview of planned speech) ⇒ a non-final transcript.
    is_final: bool,
}

/// The AWS Nova Sonic Realtime protocol. Carries the session-stable Bedrock event
/// identifiers (`prompt_name` ties every event together; `audio_content_name` is
/// the single user-audio container) plus the resolved model/voice/region and the
/// system instructions, and tracks the current text-content role/stage for
/// transcript mapping. Auth is AWS SigV4 via `aws-config` — no api-key is stored.
pub struct NovaSonicProtocol {
    /// Bedrock model id (defaulted to `amazon.nova-sonic-v1:0`).
    model_id: String,
    /// Output voice id (defaulted).
    voice: String,
    /// AWS region for the Bedrock client. `None` ⇒ resolved from the environment
    /// by the `aws-config` default chain at connect.
    region: Option<String>,
    /// System instructions (the SYSTEM `textInput` in `build_session_config`).
    instructions: Option<String>,
    /// Session-stable `promptName` — one UUID per session, included in EVERY event
    /// (the docs require the same identifier across all events).
    prompt_name: String,
    /// The single user-audio content container id, reused by every `audioInput`
    /// (all audio frames share one content block until the session ends).
    audio_content_name: String,
    /// The current text-content role/stage, set on a TEXT `contentStart` and read
    /// on the following `textOutput`. Single-writer (the driver's recv loop);
    /// `Mutex` only to satisfy `Send + Sync` on the `&self` trait.
    current_text: Mutex<Option<TextContentState>>,
}

impl NovaSonicProtocol {
    /// Resolve the model id: configured, else `amazon.nova-sonic-v1:0`.
    fn resolve_model(cfg: &RealtimeConfig) -> String {
        let m = cfg.model.trim();
        if m.is_empty() {
            DEFAULT_MODEL.to_string()
        } else {
            m.to_string()
        }
    }

    /// Resolve the AWS region. Order: `cfg.endpoint` (used DIRECTLY as the region
    /// string when non-empty — Nova Sonic reuses the generic `endpoint` config slot
    /// to carry the region, since it has no URL) → env `AWS_REGION` → `None` (let
    /// `aws-config` resolve from its default chain). Kept here so `connect_spec`
    /// stays pure; the env read for the `None` case happens in the SDK loader.
    fn resolve_region(cfg: &RealtimeConfig) -> Option<String> {
        cfg.endpoint
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .or_else(|| std::env::var("AWS_REGION").ok().filter(|s| !s.is_empty()))
    }

    /// Build the `audioOutputConfiguration` object (shared by `promptStart`).
    fn audio_output_config(&self) -> Value {
        json!({
            "mediaType": "audio/lpcm",
            "sampleRateHertz": OUTPUT_SAMPLE_RATE,
            "sampleSizeBits": 16,
            "channelCount": 1,
            "voiceId": self.voice,
            "encoding": "base64",
            "audioType": "SPEECH",
        })
    }

    /// Build the `audioInputConfiguration` object (the USER audio `contentStart`).
    fn audio_input_config() -> Value {
        json!({
            "mediaType": "audio/lpcm",
            "sampleRateHertz": INPUT_SAMPLE_RATE,
            "sampleSizeBits": 16,
            "channelCount": 1,
            "audioType": "SPEECH",
            "encoding": "base64",
        })
    }

    /// Build the `toolConfiguration` from the generic config's tools, or `None`
    /// when no tools are configured (skip the key — Nova Sonic applies its own
    /// default). Each tool's `inputSchema.json` is a STRINGIFIED JSON schema (per
    /// the docs), so we re-stringify the parameters object.
    fn tool_configuration(cfg: &RealtimeConfig) -> Option<Value> {
        let tools = cfg.tools.as_ref()?;
        if tools.is_empty() {
            return None;
        }
        let specs: Vec<Value> = tools
            .iter()
            .map(|t| {
                let mut spec = json!({ "name": t.function.name });
                if let Some(desc) = &t.function.description {
                    spec["description"] = json!(desc);
                }
                // inputSchema.json is a STRINGIFIED JSON schema (per the docs);
                // default to an empty object schema when no parameters are given.
                let schema = t
                    .function
                    .parameters
                    .as_ref()
                    .map(Value::to_string)
                    .unwrap_or_else(|| "{}".to_string());
                spec["inputSchema"] = json!({ "json": schema });
                json!({ "toolSpec": spec })
            })
            .collect();
        Some(json!({ "tools": specs }))
    }

    /// `{"event": {"audioInput": {promptName, contentName, content}}}` — one
    /// base64-PCM user-audio chunk into the single audio content container.
    fn audio_input_event(&self, pcm: &[u8]) -> Value {
        json!({
            "event": {
                "audioInput": {
                    "promptName": self.prompt_name,
                    "contentName": self.audio_content_name,
                    "content": BASE64_STANDARD.encode(pcm),
                }
            }
        })
    }
}

impl RealtimeProtocol for NovaSonicProtocol {
    type Wire = Value;

    fn from_config(cfg: &RealtimeConfig) -> RealtimeResult<Self> {
        // Nova Sonic authenticates via AWS credentials (the `aws-config` default
        // chain — env / shared config / IAM role), NOT an api-key. So — UNLIKE
        // every other realtime provider — we do NOT require `cfg.api_key` to be
        // non-empty here; credentials are resolved at connect by the transport's
        // `aws-config` loader (exactly like `AwsTranscribeSTT`).
        Ok(Self {
            model_id: Self::resolve_model(cfg),
            voice: cfg
                .voice
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(DEFAULT_VOICE)
                .to_string(),
            region: Self::resolve_region(cfg),
            instructions: cfg
                .instructions
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string),
            prompt_name: uuid::Uuid::new_v4().to_string(),
            audio_content_name: uuid::Uuid::new_v4().to_string(),
            current_text: Mutex::new(None),
        })
    }

    /// THE UNIQUE BIT: Nova Sonic runs over an Amazon Bedrock bidi HTTP/2 event
    /// stream, so it opts into the Bedrock-bidi transport factory instead of the
    /// default plain WebSocket.
    fn transport_factory(&self) -> Arc<dyn RealtimeTransportFactory> {
        Arc::new(BedrockBidiTransportFactory)
    }

    fn provider_id(&self) -> &'static str {
        "nova_sonic"
    }

    fn caps(&self) -> ProtocolCaps {
        ProtocolCaps {
            // Nova Sonic runs server-side VAD + turn-taking + barge-in, so it owns
            // turns and emits user-turn frames (the USER ASR `textOutput`).
            emits_user_turn_frames: true,
            output_bytes_per_ms: OUTPUT_BYTES_PER_MS,
            output_sample_rate: OUTPUT_SAMPLE_RATE,
            // No client-driven item truncation (barge-in is server-side; the driver
            // only clears local playback on the interruption notification).
            supports_truncate: false,
            // No gateway-managed input buffer (audio streams as base64-in-JSON).
            supports_input_buffer: false,
        }
    }

    fn connect_spec(&self, _cfg: &RealtimeConfig) -> RealtimeResult<ConnectSpec> {
        // The Bedrock bidi spec: model id + region. Credentials come from the
        // `aws-config` default chain in the transport factory (NOT in the spec),
        // exactly like `AwsTranscribeTransport`.
        Ok(ConnectSpec::BedrockBidi {
            model_id: self.model_id.clone(),
            region: self.region.clone(),
        })
    }

    fn build_session_config(
        &self,
        cfg: &RealtimeConfig,
        _resumption: Option<&str>,
    ) -> Vec<Self::Wire> {
        // The Nova Sonic bootstrap sequence (each a `{"event": {…}}` JSON), in the
        // documented order, leaving the session ready for `audioInput`:
        //   sessionStart → promptStart → (SYSTEM contentStart/textInput/contentEnd)
        //   → (USER AUDIO contentStart).
        let mut events = Vec::new();

        // 1. sessionStart(inferenceConfiguration). Inference params are optional;
        //    emit only what's set (skip-if-none discipline keeps the payload
        //    minimal and lets Nova Sonic apply its own defaults).
        let mut inference = serde_json::Map::new();
        if let Some(max) = cfg.max_response_output_tokens
            && max > 0
        {
            inference.insert("maxTokens".to_string(), json!(max));
        }
        if let Some(temp) = cfg.temperature {
            inference.insert("temperature".to_string(), json!(temp));
        }
        let mut session_start = serde_json::Map::new();
        if !inference.is_empty() {
            session_start.insert(
                "inferenceConfiguration".to_string(),
                Value::Object(inference),
            );
        }
        events.push(json!({ "event": { "sessionStart": Value::Object(session_start) } }));

        // 2. promptStart(audioOutputConfiguration [+ toolConfiguration]).
        let mut prompt_start = json!({
            "promptName": self.prompt_name,
            "textOutputConfiguration": { "mediaType": "text/plain" },
            "audioOutputConfiguration": self.audio_output_config(),
            "toolUseOutputConfiguration": { "mediaType": "application/json" },
        });
        if let Some(tool_cfg) = Self::tool_configuration(cfg) {
            prompt_start["toolConfiguration"] = tool_cfg;
        }
        events.push(json!({ "event": { "promptStart": prompt_start } }));

        // 3. SYSTEM prompt as contentStart(TEXT) / textInput / contentEnd — only
        //    when instructions are present (a fresh content block id).
        if let Some(instr) = &self.instructions {
            let sys_content = uuid::Uuid::new_v4().to_string();
            events.push(json!({
                "event": {
                    "contentStart": {
                        "promptName": self.prompt_name,
                        "contentName": sys_content,
                        "type": "TEXT",
                        "interactive": false,
                        "role": "SYSTEM",
                        "textInputConfiguration": { "mediaType": "text/plain" },
                    }
                }
            }));
            events.push(json!({
                "event": {
                    "textInput": {
                        "promptName": self.prompt_name,
                        "contentName": sys_content,
                        "content": instr,
                    }
                }
            }));
            events.push(json!({
                "event": {
                    "contentEnd": {
                        "promptName": self.prompt_name,
                        "contentName": sys_content,
                    }
                }
            }));
        }

        // 4. USER AUDIO contentStart — opens the single audio container so the
        //    session is ready for `audioInput`. (The matching contentEnd is sent on
        //    disconnect / turn close by the driver via `commit_turn`; Nova Sonic
        //    keeps ALL audio frames in this one container.)
        events.push(json!({
            "event": {
                "contentStart": {
                    "promptName": self.prompt_name,
                    "contentName": self.audio_content_name,
                    "type": "AUDIO",
                    "interactive": true,
                    "role": "USER",
                    "audioInputConfiguration": Self::audio_input_config(),
                }
            }
        }));

        events
    }

    fn map_server_event(&self, raw: Inbound<'_>) -> Vec<S2sEvent> {
        // Nova Sonic is event-framed JSON only (audio is base64 INSIDE the JSON);
        // a raw binary frame should never arrive on the Bedrock stream.
        let text = match raw {
            Inbound::Text(t) => t,
            Inbound::Binary(_) => return vec![S2sEvent::Ignore],
        };

        let value: Value = match serde_json::from_str(text) {
            Ok(v) => v,
            Err(_) => return vec![S2sEvent::Ignore],
        };
        // Every Nova Sonic event is `{"event": { <name>: {…} }}`.
        let Some(event) = value.get("event").and_then(Value::as_object) else {
            return vec![S2sEvent::Ignore];
        };
        let Some((name, body)) = event.iter().next() else {
            return vec![S2sEvent::Ignore];
        };

        match name.as_str() {
            // contentStart establishes the role + generationStage for the FOLLOWING
            // textOutput (and the type for audio). We stash the TEXT role/stage so
            // the subsequent textOutput maps to the right transcript role/finality.
            "contentStart" => {
                if body.get("type").and_then(Value::as_str) == Some("TEXT") {
                    let role = match body.get("role").and_then(Value::as_str) {
                        Some("USER") => TranscriptRole::User,
                        // ASSISTANT (and anything else) ⇒ assistant.
                        _ => TranscriptRole::Assistant,
                    };
                    // additionalModelFields is a STRINGIFIED JSON object carrying
                    // {"generationStage":"FINAL"|"SPECULATIVE"}. FINAL ⇒ settled
                    // transcript; SPECULATIVE ⇒ a preview ⇒ non-final.
                    let is_final = body
                        .get("additionalModelFields")
                        .and_then(Value::as_str)
                        .and_then(|s| serde_json::from_str::<Value>(s).ok())
                        .and_then(|v| {
                            v.get("generationStage")
                                .and_then(Value::as_str)
                                .map(|st| st == "FINAL")
                        })
                        // No stage field ⇒ treat as final (a settled block).
                        .unwrap_or(true);
                    if let Ok(mut guard) = self.current_text.lock() {
                        *guard = Some(TextContentState { role, is_final });
                    }
                }
                vec![S2sEvent::Ignore]
            }
            // textOutput carries only `content` + ids; the role/finality come from
            // the contentStart we tracked above (default Assistant/final if we
            // somehow missed it).
            "textOutput" => {
                let text = body
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let state = self
                    .current_text
                    .lock()
                    .ok()
                    .and_then(|g| *g)
                    .unwrap_or(TextContentState {
                        role: TranscriptRole::Assistant,
                        is_final: true,
                    });
                vec![S2sEvent::Transcript {
                    role: state.role,
                    text,
                    is_final: state.is_final,
                    item_id: body
                        .get("contentId")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                }]
            }
            // audioOutput carries base64 PCM in `content`. Decode to raw bytes.
            "audioOutput" => {
                let Some(b64) = body.get("content").and_then(Value::as_str) else {
                    return vec![S2sEvent::Ignore];
                };
                match BASE64_STANDARD.decode(b64) {
                    Ok(bytes) => vec![S2sEvent::Audio {
                        data: Bytes::from(bytes),
                        item_id: body
                            .get("contentId")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        response_id: body
                            .get("completionId")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                    }],
                    Err(_) => vec![S2sEvent::Ignore],
                }
            }
            // toolUse: {toolName, toolUseId, content (stringified JSON args)}.
            "toolUse" => {
                let arguments = match body.get("content") {
                    Some(Value::String(s)) => s.clone(),
                    Some(other) => other.to_string(),
                    None => String::new(),
                };
                vec![S2sEvent::FunctionCall(FunctionCallRequest {
                    call_id: body
                        .get("toolUseId")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    name: body
                        .get("toolName")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    arguments,
                    item_id: body
                        .get("contentId")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                })]
            }
            // contentEnd: a content block closed. stopReason INTERRUPTED ⇒ barge-in
            // (the driver clears local playback). Other stopReasons are not turn-
            // complete (completionEnd is the response-done signal), so Ignore.
            "contentEnd" => {
                if body.get("stopReason").and_then(Value::as_str) == Some("INTERRUPTED") {
                    vec![S2sEvent::InterruptedByServer]
                } else {
                    vec![S2sEvent::Ignore]
                }
            }
            // completionEnd: the model finished this response generation. Drives
            // on_response_done (the response/completion id is not separately needed).
            "completionEnd" => vec![S2sEvent::ResponseDone {
                response_id: body
                    .get("completionId")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            }],
            // completionStart / usageEvent / unknown ⇒ nothing actionable.
            _ => vec![S2sEvent::Ignore],
        }
    }

    fn encode_user_audio(&self, pcm: &[u8]) -> Self::Wire {
        // base64-in-JSON: an `audioInput` event into the single audio container.
        self.audio_input_event(pcm)
    }

    fn send_text(&self, text: &str) -> Vec<Self::Wire> {
        // A user text message as a one-shot TEXT content block (contentStart(TEXT,
        // USER) / textInput / contentEnd) using a fresh content id. This is the
        // text-injection surface (audio is the primary input).
        let content_name = uuid::Uuid::new_v4().to_string();
        vec![
            json!({
                "event": {
                    "contentStart": {
                        "promptName": self.prompt_name,
                        "contentName": content_name,
                        "type": "TEXT",
                        "interactive": false,
                        "role": "USER",
                        "textInputConfiguration": { "mediaType": "text/plain" },
                    }
                }
            }),
            json!({
                "event": {
                    "textInput": {
                        "promptName": self.prompt_name,
                        "contentName": content_name,
                        "content": text,
                    }
                }
            }),
            json!({
                "event": {
                    "contentEnd": {
                        "promptName": self.prompt_name,
                        "contentName": content_name,
                    }
                }
            }),
        ]
    }

    fn create_response(&self, _overrides: Option<&RealtimeResponseOverride>) -> Vec<Self::Wire> {
        // Server VAD + turn-taking owns response creation; nothing to send.
        Vec::new()
    }

    fn commit_turn(&self) -> Vec<Self::Wire> {
        // Server VAD owns turn close; Nova Sonic keeps all audio in one container
        // (closed on disconnect), so there is no per-turn commit event.
        Vec::new()
    }

    fn cancel_response(&self) -> Vec<Self::Wire> {
        // Barge-in is server-side (the INTERRUPTED contentEnd); there is no
        // client-driven response-cancel event. Safest to send nothing.
        Vec::new()
    }

    fn format_tool_result(&self, call_id: &str, result: &str) -> Vec<Self::Wire> {
        // A tool result is a TOOL content block: contentStart(TOOL, role TOOL,
        // toolResultInputConfiguration referencing the toolUseId) / toolResult /
        // contentEnd, using a fresh content id.
        let content_name = uuid::Uuid::new_v4().to_string();
        vec![
            json!({
                "event": {
                    "contentStart": {
                        "promptName": self.prompt_name,
                        "contentName": content_name,
                        "interactive": false,
                        "type": "TOOL",
                        "role": "TOOL",
                        "toolResultInputConfiguration": {
                            "toolUseId": call_id,
                            "type": "TEXT",
                            "textInputConfiguration": { "mediaType": "text/plain" },
                        }
                    }
                }
            }),
            json!({
                "event": {
                    "toolResult": {
                        "promptName": self.prompt_name,
                        "contentName": content_name,
                        "content": result,
                    }
                }
            }),
            json!({
                "event": {
                    "contentEnd": {
                        "promptName": self.prompt_name,
                        "contentName": content_name,
                    }
                }
            }),
        ]
    }

    fn serialize(&self, msg: &Self::Wire) -> RealtimeResult<OutFrame> {
        // Every Nova Sonic wire message is a JSON event ⇒ a Text frame (the
        // BedrockBidi transport wraps its bytes into a Bedrock input payload part).
        Ok(OutFrame::Text(
            serde_json::to_string(msg)
                .map_err(|e| RealtimeError::SerializationError(e.to_string()))?,
        ))
    }
}

// =============================================================================
// Tests (NO AWS key needed — pure config/event mapping)
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::realtime::base::{
        FunctionDefinition, ReplayConversationItem, ToolDefinition,
    };

    fn base_cfg() -> RealtimeConfig {
        RealtimeConfig {
            provider: "nova_sonic".into(),
            api_key: String::new(), // Nova Sonic uses AWS creds, NOT an api-key.
            model: DEFAULT_MODEL.into(),
            voice: Some("tiffany".into()),
            instructions: Some("You are helpful.".into()),
            ..Default::default()
        }
    }

    fn proto(cfg: &RealtimeConfig) -> NovaSonicProtocol {
        NovaSonicProtocol::from_config(cfg).unwrap()
    }

    /// from_config does NOT require an api-key (Nova Sonic auths via AWS creds) —
    /// an empty api_key still constructs (the keyless-provider contract).
    #[test]
    fn from_config_does_not_require_api_key() {
        let cfg = RealtimeConfig {
            api_key: String::new(),
            ..Default::default()
        };
        assert!(NovaSonicProtocol::from_config(&cfg).is_ok());
    }

    /// from_config defaults the model + voice when omitted.
    #[test]
    fn from_config_defaults_model_and_voice() {
        let cfg = RealtimeConfig {
            api_key: String::new(),
            ..Default::default()
        };
        let p = proto(&cfg);
        assert_eq!(p.model_id, DEFAULT_MODEL);
        assert_eq!(p.voice, DEFAULT_VOICE);
    }

    /// connect_spec ⇒ BedrockBidi with the model id (region from cfg.endpoint or
    /// env, else None) — and NO credentials in the spec (they come from aws-config).
    #[test]
    fn connect_spec_builds_bedrock_bidi() {
        let cfg = RealtimeConfig {
            endpoint: Some("us-east-1".into()),
            ..base_cfg()
        };
        let p = proto(&cfg);
        let spec = p.connect_spec(&cfg).unwrap();
        let ConnectSpec::BedrockBidi { model_id, region } = spec else {
            panic!("expected BedrockBidi, got {spec:?}");
        };
        assert_eq!(model_id, DEFAULT_MODEL);
        assert_eq!(region.as_deref(), Some("us-east-1"));
    }

    /// provider_id is the stable "nova_sonic" used for the breaker label/metrics.
    #[test]
    fn provider_id_is_nova_sonic() {
        assert_eq!(proto(&base_cfg()).provider_id(), "nova_sonic");
    }

    /// caps: server-VAD turn frames, 48 B/ms @ 24 kHz output, NO truncate, NO
    /// input buffer.
    #[test]
    fn caps_describe_server_vad_provider() {
        let c = proto(&base_cfg()).caps();
        assert!(c.emits_user_turn_frames);
        assert_eq!(c.output_bytes_per_ms, 48);
        assert_eq!(c.output_sample_rate, 24_000);
        assert!(!c.supports_truncate);
        assert!(!c.supports_input_buffer);
    }

    /// build_session_config emits the documented bootstrap sequence in order:
    /// sessionStart → promptStart → SYSTEM contentStart/textInput/contentEnd →
    /// USER AUDIO contentStart. Each is wrapped in the top-level `event` key, and
    /// all share the SAME promptName.
    #[test]
    fn build_session_config_emits_bootstrap_sequence() {
        let cfg = RealtimeConfig {
            temperature: Some(0.7),
            max_response_output_tokens: Some(1024),
            ..base_cfg()
        };
        let p = proto(&cfg);
        let events = p.build_session_config(&cfg, None);

        // Pull the single inner event name from each wrapped `{"event":{name:…}}`.
        let names: Vec<String> = events
            .iter()
            .map(|e| {
                e["event"]
                    .as_object()
                    .and_then(|o| o.keys().next())
                    .cloned()
                    .unwrap_or_default()
            })
            .collect();
        assert_eq!(
            names,
            vec![
                "sessionStart",
                "promptStart",
                "contentStart",
                "textInput",
                "contentEnd",
                "contentStart",
            ],
            "bootstrap order must match the Nova Sonic input-event flow"
        );

        // sessionStart carries the inference config we set. (temperature is an f32
        // ⇒ compare the f64 view within tolerance, not bit-exact against 0.7.)
        let temp = events[0]["event"]["sessionStart"]["inferenceConfiguration"]["temperature"]
            .as_f64()
            .unwrap();
        assert!((temp - 0.7).abs() < 1e-6, "temperature ≈ 0.7, got {temp}");
        assert_eq!(
            events[0]["event"]["sessionStart"]["inferenceConfiguration"]["maxTokens"],
            1024
        );

        // promptStart carries the 24 kHz lpcm audio output config + the voice.
        let ao = &events[1]["event"]["promptStart"]["audioOutputConfiguration"];
        assert_eq!(ao["mediaType"], "audio/lpcm");
        assert_eq!(ao["sampleRateHertz"], 24000);
        assert_eq!(ao["voiceId"], "tiffany");

        // SYSTEM block: contentStart(role SYSTEM, type TEXT) + textInput(content).
        assert_eq!(events[2]["event"]["contentStart"]["role"], "SYSTEM");
        assert_eq!(events[2]["event"]["contentStart"]["type"], "TEXT");
        assert_eq!(events[3]["event"]["textInput"]["content"], "You are helpful.");

        // USER AUDIO container opened (type AUDIO, role USER, interactive true),
        // 16 kHz lpcm input.
        let last = &events[5]["event"]["contentStart"];
        assert_eq!(last["type"], "AUDIO");
        assert_eq!(last["role"], "USER");
        assert_eq!(last["interactive"], true);
        assert_eq!(
            last["audioInputConfiguration"]["sampleRateHertz"],
            INPUT_SAMPLE_RATE
        );

        // ALL events share one promptName, and it equals the audio content's owner.
        let prompt_name = events[1]["event"]["promptStart"]["promptName"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(!prompt_name.is_empty());
        for e in &events {
            if let Some(inner) = e["event"].as_object().and_then(|o| o.values().next())
                && let Some(pn) = inner.get("promptName")
            {
                assert_eq!(pn.as_str().unwrap(), prompt_name, "all events share promptName");
            }
        }
        // The USER AUDIO contentStart uses the SAME contentName encode_user_audio reuses.
        assert_eq!(
            events[5]["event"]["contentStart"]["contentName"]
                .as_str()
                .unwrap(),
            p.audio_content_name
        );
    }

    /// build_session_config omits the SYSTEM block when there are no instructions
    /// (and still opens the audio container).
    #[test]
    fn build_session_config_skips_system_when_no_instructions() {
        let cfg = RealtimeConfig {
            instructions: None,
            api_key: String::new(),
            ..Default::default()
        };
        let p = proto(&cfg);
        let events = p.build_session_config(&cfg, None);
        let names: Vec<&str> = events
            .iter()
            .map(|e| e["event"].as_object().unwrap().keys().next().unwrap().as_str())
            .collect();
        // No SYSTEM contentStart/textInput/contentEnd — just session/prompt/audio.
        assert_eq!(names, vec!["sessionStart", "promptStart", "contentStart"]);
        assert_eq!(events[2]["event"]["contentStart"]["type"], "AUDIO");
    }

    /// build_session_config wires configured tools into promptStart.toolConfiguration
    /// with a STRINGIFIED inputSchema.json.
    #[test]
    fn build_session_config_wires_tools() {
        let cfg = RealtimeConfig {
            tools: Some(vec![ToolDefinition {
                tool_type: "function".into(),
                function: FunctionDefinition {
                    name: "get_weather".into(),
                    description: Some("Get weather".into()),
                    parameters: Some(
                        json!({"type":"object","properties":{"city":{"type":"string"}}}),
                    ),
                },
            }]),
            ..base_cfg()
        };
        let p = proto(&cfg);
        let events = p.build_session_config(&cfg, None);
        let tools = &events[1]["event"]["promptStart"]["toolConfiguration"]["tools"];
        assert_eq!(tools[0]["toolSpec"]["name"], "get_weather");
        // inputSchema.json is a STRING (stringified schema).
        let schema = tools[0]["toolSpec"]["inputSchema"]["json"].as_str().unwrap();
        let parsed: Value = serde_json::from_str(schema).unwrap();
        assert_eq!(parsed["properties"]["city"]["type"], "string");
    }

    /// encode_user_audio ⇒ an audioInput event with base64 PCM in `content`, the
    /// session promptName, and the audio container's contentName; serialize ⇒ Text.
    #[test]
    fn encode_user_audio_is_base64_in_json() {
        let p = proto(&base_cfg());
        let pcm: Vec<u8> = vec![0x00, 0x01, 0xFE, 0xFF, 0x7F, 0x80];
        let wire = p.encode_user_audio(&pcm);
        let ai = &wire["event"]["audioInput"];
        assert_eq!(ai["promptName"].as_str().unwrap(), p.prompt_name);
        assert_eq!(ai["contentName"].as_str().unwrap(), p.audio_content_name);
        assert_eq!(
            BASE64_STANDARD.decode(ai["content"].as_str().unwrap()).unwrap(),
            pcm,
            "audio is base64-encoded PCM inside the JSON event"
        );
        // serialize ⇒ a Text frame (NOT binary — Bedrock is event-framed).
        match p.serialize(&wire).unwrap() {
            OutFrame::Text(s) => assert!(s.contains("audioInput")),
            OutFrame::Binary(_) => panic!("Nova Sonic audio must serialize to a Text frame"),
        }
    }

    /// map_server_event: an ASSISTANT TEXT contentStart (SPECULATIVE) followed by a
    /// textOutput ⇒ a non-final Assistant transcript carrying the content.
    #[test]
    fn assistant_speculative_text_maps_to_nonfinal_transcript() {
        let p = proto(&base_cfg());
        let cs = r#"{"event":{"contentStart":{"type":"TEXT","role":"ASSISTANT","additionalModelFields":"{\"generationStage\":\"SPECULATIVE\"}","contentId":"c1"}}}"#;
        assert!(matches!(
            p.map_server_event(Inbound::Text(cs)).as_slice(),
            [S2sEvent::Ignore]
        ));
        let to = r#"{"event":{"textOutput":{"content":"hello there","contentId":"c1"}}}"#;
        match p.map_server_event(Inbound::Text(to)).as_slice() {
            [S2sEvent::Transcript { role, text, is_final, item_id }] => {
                assert_eq!(*role, TranscriptRole::Assistant);
                assert_eq!(text, "hello there");
                assert!(!*is_final, "SPECULATIVE ⇒ non-final");
                assert_eq!(item_id.as_deref(), Some("c1"));
            }
            other => panic!("expected assistant Transcript, got {other:?}"),
        }
    }

    /// map_server_event: a USER TEXT contentStart (FINAL ASR) followed by a
    /// textOutput ⇒ a FINAL User transcript (the ASR result).
    #[test]
    fn user_final_asr_text_maps_to_final_user_transcript() {
        let p = proto(&base_cfg());
        let cs = r#"{"event":{"contentStart":{"type":"TEXT","role":"USER","additionalModelFields":"{\"generationStage\":\"FINAL\"}","contentId":"u1"}}}"#;
        assert!(matches!(
            p.map_server_event(Inbound::Text(cs)).as_slice(),
            [S2sEvent::Ignore]
        ));
        let to = r#"{"event":{"textOutput":{"content":"what is the weather","contentId":"u1"}}}"#;
        match p.map_server_event(Inbound::Text(to)).as_slice() {
            [S2sEvent::Transcript { role, text, is_final, .. }] => {
                assert_eq!(*role, TranscriptRole::User);
                assert_eq!(text, "what is the weather");
                assert!(*is_final, "FINAL ⇒ final");
            }
            other => panic!("expected user Transcript, got {other:?}"),
        }
    }

    /// map_server_event: audioOutput ⇒ Audio with byte-exact base64-decoded PCM.
    #[test]
    fn audio_output_maps_to_decoded_audio() {
        let p = proto(&base_cfg());
        let pcm: &[u8] = &[1, 2, 3, 4, 250, 251];
        let b64 = BASE64_STANDARD.encode(pcm);
        let raw = format!(
            r#"{{"event":{{"audioOutput":{{"content":"{b64}","contentId":"a1","completionId":"comp1"}}}}}}"#
        );
        match p.map_server_event(Inbound::Text(&raw)).as_slice() {
            [S2sEvent::Audio { data, item_id, response_id }] => {
                assert_eq!(data.as_ref(), pcm);
                assert_eq!(item_id.as_deref(), Some("a1"));
                assert_eq!(response_id.as_deref(), Some("comp1"));
            }
            other => panic!("expected Audio, got {other:?}"),
        }
    }

    /// map_server_event: toolUse ⇒ FunctionCall carrying toolName/toolUseId/content.
    #[test]
    fn tool_use_maps_to_function_call() {
        let p = proto(&base_cfg());
        let raw = r#"{"event":{"toolUse":{"toolName":"get_weather","toolUseId":"tu-1","content":"{\"city\":\"NYC\"}","contentId":"t1"}}}"#;
        match p.map_server_event(Inbound::Text(raw)).as_slice() {
            [S2sEvent::FunctionCall(fc)] => {
                assert_eq!(fc.call_id, "tu-1");
                assert_eq!(fc.name, "get_weather");
                assert_eq!(fc.arguments, r#"{"city":"NYC"}"#);
                assert_eq!(fc.item_id.as_deref(), Some("t1"));
            }
            other => panic!("expected FunctionCall, got {other:?}"),
        }
    }

    /// map_server_event: an INTERRUPTED contentEnd ⇒ InterruptedByServer (barge-in);
    /// a normal END_TURN contentEnd ⇒ Ignore (completionEnd is the done signal).
    #[test]
    fn content_end_interrupted_maps_to_interrupted() {
        let p = proto(&base_cfg());
        let interrupted = r#"{"event":{"contentEnd":{"stopReason":"INTERRUPTED","type":"AUDIO"}}}"#;
        assert!(matches!(
            p.map_server_event(Inbound::Text(interrupted)).as_slice(),
            [S2sEvent::InterruptedByServer]
        ));
        let end_turn = r#"{"event":{"contentEnd":{"stopReason":"END_TURN","type":"AUDIO"}}}"#;
        assert!(matches!(
            p.map_server_event(Inbound::Text(end_turn)).as_slice(),
            [S2sEvent::Ignore]
        ));
    }

    /// map_server_event: completionEnd ⇒ ResponseDone (carrying the completion id).
    #[test]
    fn completion_end_maps_to_response_done() {
        let p = proto(&base_cfg());
        let raw = r#"{"event":{"completionEnd":{"completionId":"comp1","stopReason":"END_TURN"}}}"#;
        match p.map_server_event(Inbound::Text(raw)).as_slice() {
            [S2sEvent::ResponseDone { response_id }] => assert_eq!(response_id, "comp1"),
            other => panic!("expected ResponseDone, got {other:?}"),
        }
    }

    /// map_server_event: completionStart / usageEvent / unknown / unparseable /
    /// missing-event-wrapper ⇒ Ignore.
    #[test]
    fn benign_and_unknown_events_map_to_ignore() {
        let p = proto(&base_cfg());
        for raw in [
            r#"{"event":{"completionStart":{"completionId":"x"}}}"#,
            r#"{"event":{"usageEvent":{"totalTokens":5}}}"#,
            r#"{"event":{"somethingNew":{}}}"#,
            r#"{"no_event_wrapper":true}"#,
            "not json",
        ] {
            assert!(
                matches!(p.map_server_event(Inbound::Text(raw)).as_slice(), [S2sEvent::Ignore]),
                "expected Ignore for {raw}"
            );
        }
        // A stray binary frame (should never happen on Bedrock) ⇒ Ignore.
        assert!(matches!(
            p.map_server_event(Inbound::Binary(&[1, 2, 3])).as_slice(),
            [S2sEvent::Ignore]
        ));
    }

    /// send_text ⇒ a USER TEXT content block (contentStart/textInput/contentEnd)
    /// sharing one fresh contentName, with the message in textInput.content.
    #[test]
    fn send_text_emits_user_text_content_block() {
        let p = proto(&base_cfg());
        let msgs = p.send_text("hello model");
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0]["event"]["contentStart"]["role"], "USER");
        assert_eq!(msgs[0]["event"]["contentStart"]["type"], "TEXT");
        assert_eq!(msgs[1]["event"]["textInput"]["content"], "hello model");
        // One shared contentName across the block.
        let cn = msgs[0]["event"]["contentStart"]["contentName"].as_str().unwrap();
        assert_eq!(msgs[1]["event"]["textInput"]["contentName"], cn);
        assert_eq!(msgs[2]["event"]["contentEnd"]["contentName"], cn);
    }

    /// format_tool_result ⇒ a TOOL content block referencing the toolUseId, with
    /// the result string in toolResult.content.
    #[test]
    fn format_tool_result_emits_tool_content_block() {
        let p = proto(&base_cfg());
        let msgs = p.format_tool_result("tu-1", r#"{"temp":"72F"}"#);
        assert_eq!(msgs.len(), 3);
        let cs = &msgs[0]["event"]["contentStart"];
        assert_eq!(cs["type"], "TOOL");
        assert_eq!(cs["role"], "TOOL");
        assert_eq!(cs["toolResultInputConfiguration"]["toolUseId"], "tu-1");
        assert_eq!(msgs[1]["event"]["toolResult"]["content"], r#"{"temp":"72F"}"#);
    }

    /// Turn/response controls are empty (server VAD owns them); defaults (truncate,
    /// input-buffer clear, replay) are empty too.
    #[test]
    fn turn_and_default_controls_are_empty() {
        let p = proto(&base_cfg());
        assert!(p.create_response(None).is_empty());
        assert!(p.commit_turn().is_empty());
        assert!(p.cancel_response().is_empty());
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
}
