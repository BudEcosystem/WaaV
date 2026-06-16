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

use bytes::Bytes;
use serde_json::{json, Value};

use crate::core::realtime::base::{
    FunctionCallRequest, RealtimeConfig, RealtimeError, RealtimeResponseOverride, RealtimeResult,
    SpeechEvent, TranscriptRole,
};
use crate::core::realtime::scaffold::{
    ConnectSpec, Inbound, OutFrame, ProtocolCaps, RealtimeProtocol, S2sEvent,
};

/// Deepgram Voice Agent WebSocket endpoint. Single bidi socket for the whole
/// STT→LLM→TTS loop. LIVE-CAPTURED working endpoint.
pub(crate) const DEEPGRAM_AGENT_URL: &str = "wss://agent.deepgram.com/v1/agent/converse";

/// Linear16 @ 24 kHz: WaaV's canonical realtime PCM rate (matches the OpenAI
/// family) — keeps the gateway audio path uniform across S2S providers. Used for
/// BOTH input + output unless `cfg.{input,output}_audio_format` overrides.
const DEFAULT_SAMPLE_RATE: u32 = 24_000;

/// Default agent TTS voice. `aura-2-*` is Deepgram's current (Aura-2) voice
/// family; `thalia` is the LIVE-CAPTURED working voice.
const DEFAULT_SPEAK_MODEL: &str = "aura-2-thalia-en";

/// Default STT model for the `listen` leg (Deepgram's current flagship).
const DEFAULT_LISTEN_MODEL: &str = "nova-3";

/// Default `think` LLM. Deepgram-managed `open_ai` provider (server holds the
/// creds) — fine for now; an env/cfg think-provider override can land later.
const DEFAULT_THINK_MODEL: &str = "gpt-4o-mini";

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

    /// Build the LIVE-CAPTURED `Settings` JSON from the generic config:
    /// model → `think` LLM, voice → `speak` TTS, instructions → `think.prompt`,
    /// audio in/out linear16 @ rate. Shape matches the capture exactly.
    pub fn settings(&self, cfg: &RealtimeConfig) -> Value {
        let rate = Self::sample_rate(cfg);

        let speak_model = cfg.voice.as_deref().unwrap_or(DEFAULT_SPEAK_MODEL);
        let think_model = if cfg.model.trim().is_empty() {
            DEFAULT_THINK_MODEL
        } else {
            cfg.model.trim()
        };
        // Deepgram's `agent.language` is a bare BCP-47 tag; default "en". WaaV's
        // RealtimeConfig has no dedicated language field, so default for now.
        let language = "en";

        // `think.prompt` is the system prompt. Deepgram is lenient, but only emit
        // the key when we actually have instructions (skip-if-none discipline).
        let mut think = json!({
            "provider": { "type": "open_ai", "model": think_model },
        });
        if let Some(instr) = cfg.instructions.as_deref()
            && !instr.is_empty()
        {
            think["prompt"] = json!(instr);
        }

        json!({
            "type": "Settings",
            "audio": {
                "input": { "encoding": "linear16", "sample_rate": rate },
                "output": { "encoding": "linear16", "sample_rate": rate, "container": "none" },
            },
            "agent": {
                "language": language,
                "listen": { "provider": { "type": "deepgram", "model": DEFAULT_LISTEN_MODEL } },
                "think": think,
                "speak": { "provider": { "type": "deepgram", "model": speak_model } },
            },
        })
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
        })
    }

    fn provider_id(&self) -> &'static str {
        "deepgram"
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
        Ok(ConnectSpec::WebSocket {
            url: DEEPGRAM_AGENT_URL.to_string(),
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
            "SettingsApplied" => vec![S2sEvent::Ignore],
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
            "AgentAudioDone" => vec![S2sEvent::ResponseDone {
                response_id: String::new(),
            }],
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
                            f.get("client_side").and_then(Value::as_bool).unwrap_or(true)
                        })
                        .map(|f| {
                            S2sEvent::FunctionCall(FunctionCallRequest {
                                call_id: f
                                    .get("id")
                                    .and_then(Value::as_str)
                                    .unwrap_or("")
                                    .to_string(),
                                name: f
                                    .get("name")
                                    .and_then(Value::as_str)
                                    .unwrap_or("")
                                    .to_string(),
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
            // Error / Warning (docs): surface the description/message.
            "Error" | "Warning" => {
                let detail = value
                    .get("description")
                    .and_then(Value::as_str)
                    .or_else(|| value.get("message").and_then(Value::as_str))
                    .unwrap_or("unknown Deepgram agent error")
                    .to_string();
                vec![S2sEvent::Error(RealtimeError::ProviderError(detail))]
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
        // Best-effort: Voice Agent has no "user text turn" event; the closest
        // documented control is `InjectAgentMessage`, which makes the AGENT SPEAK
        // the given text (not a user message). Used here as the text-injection
        // surface. (Docs: `InjectAgentMessage`.)
        vec![DeepgramClientMsg::Json(json!({
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
        // Tool result → `FunctionCallResponse` (docs). The `id` echoes the
        // request's function id; `content` carries the result string.
        vec![DeepgramClientMsg::Json(json!({
            "type": "FunctionCallResponse",
            "id": call_id,
            "content": result,
        }))]
    }

    fn serialize(&self, msg: &Self::Wire) -> RealtimeResult<OutFrame> {
        match msg {
            DeepgramClientMsg::Json(v) => Ok(OutFrame::Text(
                serde_json::to_string(v)
                    .map_err(|e| RealtimeError::SerializationError(e.to_string()))?,
            )),
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
        let ConnectSpec::WebSocket { url, headers } = p.connect_spec(&base_cfg()).unwrap();
        assert_eq!(url, "wss://agent.deepgram.com/v1/agent/converse");
        let auth = headers
            .iter()
            .find(|(k, _)| k == "Authorization")
            .map(|(_, v)| v.as_str())
            .unwrap();
        assert_eq!(auth, "Token dgkey", "Deepgram uses Token, not Bearer");
        assert!(
            !auth.starts_with("Bearer"),
            "must NOT be a Bearer header"
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
            other => panic!("expected user Transcript, got {other:?}"),
        }
        let asst = r#"{"type":"ConversationText","role":"assistant","content":"hi!"}"#;
        match p.map_server_event(Inbound::Text(asst)).as_slice() {
            [S2sEvent::Transcript {
                role,
                text,
                is_final,
                ..
            }] => {
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
            [S2sEvent::Audio {
                data,
                item_id,
                response_id,
            }] => {
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
            [S2sEvent::Speech(SpeechEvent::Started {
                audio_start_ms,
                item_id,
            }), S2sEvent::InterruptedByServer] => {
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

    /// map_server_event: Error / Warning ⇒ Error(ProviderError) with the description.
    #[test]
    fn error_and_warning_map_to_error() {
        let p = proto(&base_cfg());
        let err = r#"{"type":"Error","description":"bad audio encoding"}"#;
        match p.map_server_event(Inbound::Text(err)).as_slice() {
            [S2sEvent::Error(RealtimeError::ProviderError(d))] => {
                assert_eq!(d, "bad audio encoding");
            }
            other => panic!("expected Error(ProviderError), got {other:?}"),
        }
        // Warning uses a `message` field instead of `description`.
        let warn = r#"{"type":"Warning","message":"deprecated model"}"#;
        match p.map_server_event(Inbound::Text(warn)).as_slice() {
            [S2sEvent::Error(RealtimeError::ProviderError(d))] => {
                assert_eq!(d, "deprecated model");
            }
            other => panic!("expected Error(ProviderError), got {other:?}"),
        }
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

    /// format_tool_result ⇒ FunctionCallResponse echoing the call id + content.
    #[test]
    fn format_tool_result_builds_function_call_response() {
        let p = proto(&base_cfg());
        match p.format_tool_result("fc1", "72F sunny").as_slice() {
            [DeepgramClientMsg::Json(v)] => {
                assert_eq!(v["type"], "FunctionCallResponse");
                assert_eq!(v["id"], "fc1");
                assert_eq!(v["content"], "72F sunny");
            }
            other => panic!("expected FunctionCallResponse, got {other:?}"),
        }
    }
}
