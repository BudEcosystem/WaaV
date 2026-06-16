//! Google Gemini Live ("BidiGenerateContent") [`RealtimeProtocol`] — a
//! base64+JSON speech-to-speech provider. Gemini runs the whole STT→LLM→TTS
//! loop server-side with server VAD, turn-taking, and barge-in; the model IS
//! the agent.
//!
//! Gemini Live is the provider WaaV's S2S scaffold's MULTI-FRAME `Vec<S2sEvent>`,
//! session RESUMPTION, and SendFrame features were built for:
//!
//! - **MULTI-FRAME**: a single `serverContent` wire message bundles a
//!   `modelTurn.parts[]` array (audio `inlineData` parts AND `text` parts) plus
//!   `turnComplete`/`interrupted` flags. [`map_server_event`] returns ONE
//!   [`S2sEvent`] PER part (each audio part ⇒ `Audio`, each text part ⇒
//!   `Transcript`), then appends `InterruptedByServer` / `ResponseDone` — so one
//!   wire message lowers to N normalized events. This is THE reason
//!   `map_server_event` returns a `Vec`.
//! - **RESUMPTION**: the server periodically emits `sessionResumptionUpdate` with
//!   a `newHandle`; it lowers to [`S2sEvent::ResumptionHandle`], the driver
//!   stores it, and on reconnect feeds it back into
//!   [`build_session_config`](RealtimeProtocol::build_session_config) as the
//!   `resumption` arg (⇒ `setup.sessionResumption.handle`), resuming the session
//!   instead of starting fresh.
//!
//! Wire (raw `BidiGenerateContent` JSON; field names verified against Pipecat's
//! `services/google/gemini_live/llm.py`, which drives the same API through the
//! google-genai SDK):
//! - Connect: `wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent?key=<API_KEY>`
//!   — the API key is a QUERY param (`?key=`), NOT a header.
//! - FIRST outbound = `setup`:
//!   `{"setup":{"model":"models/<model>","generationConfig":{"responseModalities":["AUDIO"],"speechConfig":{…}},"systemInstruction":{"parts":[{"text":…}]},"tools":[…],"sessionResumption":{"handle":<h-or-null>}}}`.
//!   Server replies `{"setupComplete":{}}` ⇒ `SessionReady`.
//! - Audio in: `{"realtimeInput":{"mediaChunks":[{"mimeType":"audio/pcm;rate=16000","data":<base64 pcm>}]}}`.
//! - Audio out (24 kHz): `serverContent.modelTurn.parts[].inlineData{mimeType:"audio/pcm;rate=24000",data:<base64>}`.
//!
//! Docs: <https://ai.google.dev/gemini-api/docs/live>

use base64::prelude::*;
use bytes::Bytes;
use serde_json::{Value, json};

use crate::core::realtime::base::{
    FunctionCallRequest, RealtimeConfig, RealtimeError, RealtimeResponseOverride, RealtimeResult,
    TranscriptRole,
};
use crate::core::realtime::scaffold::{
    ConnectSpec, Inbound, OutFrame, ProtocolCaps, RealtimeProtocol, S2sEvent,
};

/// Gemini Live BidiGenerateContent WebSocket endpoint. The `?key=<API_KEY>`
/// query is appended in `connect_spec` (Gemini auths by QUERY param, not header).
pub(crate) const GEMINI_LIVE_URL: &str = "wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent";

/// Current Gemini Live model (the half-cascade audio model). Used when
/// `cfg.model` is empty. Pipecat's default is a `gemini-2.5-flash-native-audio-*`
/// preview; the broadly-available stable Live model is `gemini-2.0-flash-live-001`.
pub(crate) const GEMINI_LIVE_DEFAULT_MODEL: &str = "gemini-2.0-flash-live-001";

/// Gemini Live OUTPUT is 24 kHz mono 16-bit PCM ⇒ 2 B/sample × 24 samples/ms =
/// 48 B/ms. (Pipecat: `self._sample_rate = 24000`, output mime
/// `audio/pcm;rate=24000`.)
const OUTPUT_BYTES_PER_MS: u64 = 48;
/// Output sample rate stamped onto delivered audio (24 kHz).
const OUTPUT_SAMPLE_RATE: u32 = 24_000;
/// Gemini Live INPUT is 16 kHz mono 16-bit PCM. (Pipecat sends
/// `audio/pcm;rate={frame.sample_rate}`; the canonical realtime input rate.)
const INPUT_SAMPLE_RATE: u32 = 16_000;

/// The Gemini Live protocol. Stateless: the only provider-specific state the wire
/// mappings need is the resolved model (embedded in the `setup` message) and the
/// api key (re-read from `cfg` in `connect_spec`).
pub struct GeminiProtocol {
    /// The resolved Gemini Live model (e.g. `gemini-2.0-flash-live-001`). The
    /// `setup.model` field carries it as `models/<model>`.
    model: String,
}

impl GeminiProtocol {
    /// The resolved model id (for the newtype's inherent `model()` accessor +
    /// tests). NOT prefixed with `models/` — that prefix is added at wire time.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Resolve the model: `cfg.model` if set, else the current Live default.
    fn resolve_model(cfg: &RealtimeConfig) -> String {
        let model = cfg.model.trim();
        if model.is_empty() {
            GEMINI_LIVE_DEFAULT_MODEL.to_string()
        } else {
            model.to_string()
        }
    }

    /// Build the `generationConfig.speechConfig` from `cfg.voice` (a Gemini
    /// prebuilt voice name like `Puck`/`Charon`/`Kore`). Omitted entirely when no
    /// voice is set (skip-if-none discipline — Gemini kills the session on
    /// explicit-null/unknown keys, and a missing `speechConfig` ⇒ the model's
    /// default voice).
    fn speech_config(cfg: &RealtimeConfig) -> Option<Value> {
        let voice = cfg.voice.as_deref().map(str::trim).filter(|v| !v.is_empty())?;
        Some(json!({
            "voiceConfig": { "prebuiltVoiceConfig": { "voiceName": voice } }
        }))
    }

    /// Build the `tools` array from `cfg.tools` (Gemini groups all function
    /// declarations under a single `{"functionDeclarations":[…]}` tool). Returns
    /// `None` when no tools — the field is then omitted from `setup`.
    fn tools(cfg: &RealtimeConfig) -> Option<Value> {
        let tools = cfg.tools.as_ref()?;
        if tools.is_empty() {
            return None;
        }
        let decls: Vec<Value> = tools
            .iter()
            .map(|t| {
                let mut decl = json!({ "name": t.function.name });
                if let Some(desc) = &t.function.description {
                    decl["description"] = json!(desc);
                }
                if let Some(params) = &t.function.parameters {
                    decl["parameters"] = params.clone();
                }
                decl
            })
            .collect();
        Some(json!([{ "functionDeclarations": decls }]))
    }

    /// Lower one `serverContent` object into per-part events. Each `inlineData`
    /// part ⇒ `Audio` (base64-decoded); each `text` part ⇒ `Transcript`
    /// (Assistant, `is_final` = `turnComplete`). Then `interrupted` ⇒
    /// `InterruptedByServer` and `turnComplete` ⇒ `ResponseDone` are appended.
    /// THIS is the MULTI-FRAME case: one wire message → a `Vec` of N events.
    fn map_server_content(sc: &Value) -> Vec<S2sEvent> {
        let mut out = Vec::new();

        let turn_complete = sc
            .get("turnComplete")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let interrupted = sc.get("interrupted").and_then(Value::as_bool).unwrap_or(false);

        // ONE S2sEvent PER part of modelTurn.parts[].
        if let Some(parts) = sc
            .get("modelTurn")
            .and_then(|mt| mt.get("parts"))
            .and_then(Value::as_array)
        {
            for part in parts {
                // Audio part: inlineData{mimeType:"audio/pcm;rate=24000",data:b64}.
                // Gate on an `audio/` mimeType so a future model returning a
                // non-audio inlineData blob (image / thought-signature) is NOT
                // decoded + played as raw PCM (review wf_d0b9713b #3). A missing
                // mimeType ⇒ assume audio (Gemini always sends audio/pcm here).
                if let Some(inline) = part.get("inlineData") {
                    let is_audio = inline
                        .get("mimeType")
                        .and_then(Value::as_str)
                        .map(|m| m.starts_with("audio/"))
                        .unwrap_or(true);
                    if is_audio
                        && let Some(b64) = inline.get("data").and_then(Value::as_str)
                        && let Ok(bytes) = BASE64_STANDARD.decode(b64)
                    {
                        out.push(S2sEvent::Audio {
                            data: Bytes::from(bytes),
                            item_id: None,
                            response_id: None,
                        });
                    }
                }
                // Text part: a model-emitted text chunk (AUDIO modality emits an
                // optional text transcript; TEXT modality emits the response).
                if let Some(text) = part.get("text").and_then(Value::as_str)
                    && !text.is_empty()
                {
                    out.push(S2sEvent::Transcript {
                        role: TranscriptRole::Assistant,
                        text: text.to_string(),
                        is_final: turn_complete,
                        item_id: None,
                    });
                }
            }
        }

        // Server-driven turn transcription (when enabled in setup):
        // inputTranscription ⇒ User, outputTranscription ⇒ Assistant.
        if let Some(text) = sc
            .get("inputTranscription")
            .and_then(|t| t.get("text"))
            .and_then(Value::as_str)
            && !text.is_empty()
        {
            out.push(S2sEvent::Transcript {
                role: TranscriptRole::User,
                text: text.to_string(),
                is_final: false,
                item_id: None,
            });
        }
        if let Some(text) = sc
            .get("outputTranscription")
            .and_then(|t| t.get("text"))
            .and_then(Value::as_str)
            && !text.is_empty()
        {
            out.push(S2sEvent::Transcript {
                role: TranscriptRole::Assistant,
                text: text.to_string(),
                is_final: turn_complete,
                item_id: None,
            });
        }

        // Barge-in: the server stopped the model turn (user interrupted). The
        // driver runs cancel + clears local playback (Gemini has no truncate).
        if interrupted {
            out.push(S2sEvent::InterruptedByServer);
        }
        // Generation finished (NOT playout — driver must not clear playback here).
        if turn_complete {
            // Gemini gives no response id; use a stable label for on_response_done.
            out.push(S2sEvent::ResponseDone {
                response_id: "gemini-turn".to_string(),
            });
        }

        if out.is_empty() {
            out.push(S2sEvent::Ignore);
        }
        out
    }

    /// Lower a `toolCall` object into one `FunctionCall` per `functionCalls[]`
    /// entry. Each call carries `id` (may be absent on Vertex), `name`, and
    /// `args` (a JSON object stringified into `arguments`).
    fn map_tool_call(tc: &Value) -> Vec<S2sEvent> {
        let Some(calls) = tc.get("functionCalls").and_then(Value::as_array) else {
            return vec![S2sEvent::Ignore];
        };
        let events: Vec<S2sEvent> = calls
            .iter()
            .map(|call| {
                let call_id = call
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let name = call
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                // `args` is a JSON object; stringify it for the generic
                // FunctionCallRequest.arguments (a JSON string). Default "{}".
                let arguments = call
                    .get("args")
                    .map(|a| a.to_string())
                    .unwrap_or_else(|| "{}".to_string());
                S2sEvent::FunctionCall(FunctionCallRequest {
                    call_id,
                    name,
                    arguments,
                    item_id: None,
                })
            })
            .collect();
        if events.is_empty() {
            vec![S2sEvent::Ignore]
        } else {
            events
        }
    }
}

impl RealtimeProtocol for GeminiProtocol {
    type Wire = Value;

    fn from_config(cfg: &RealtimeConfig) -> RealtimeResult<Self> {
        // Gemini auths via the `?key=<API_KEY>` query param — a non-empty key is
        // required.
        if cfg.api_key.is_empty() {
            return Err(RealtimeError::AuthenticationFailed(
                "API key is required".to_string(),
            ));
        }
        Ok(Self {
            model: Self::resolve_model(cfg),
        })
    }

    fn provider_id(&self) -> &'static str {
        "gemini"
    }

    fn caps(&self) -> ProtocolCaps {
        ProtocolCaps {
            // Gemini runs server-side VAD + turn-taking, so it owns turns and
            // emits user-turn frames (interrupted, input transcription).
            emits_user_turn_frames: true,
            output_bytes_per_ms: OUTPUT_BYTES_PER_MS,
            output_sample_rate: OUTPUT_SAMPLE_RATE,
            // Gemini has no client-driven item truncation (barge-in is server-side
            // via `interrupted`; the driver only clears local playback).
            supports_truncate: false,
            // No gateway-managed input buffer (audio streams as JSON chunks).
            supports_input_buffer: false,
        }
    }

    fn connect_spec(&self, cfg: &RealtimeConfig) -> RealtimeResult<ConnectSpec> {
        // The api key goes in the `?key=` QUERY param (NOT a header). The standard
        // WS-upgrade headers are generated by `into_client_request()` in the
        // transport factory.
        //
        // SERVER-CONFIG `realtime_endpoint_override` (proxy / self-hosted / gov-cloud
        // / local mock) WINS over the Live host when set. Because Gemini auths via
        // the QUERY (not a header), the `?key=` is appended to the override when the
        // override carries no query of its own (so a proxy still gets a well-formed
        // url). SSRF-safe: never client-settable (see `apply_endpoint_override`).
        let default_url = format!("{GEMINI_LIVE_URL}?key={}", cfg.api_key);
        let auth_query = format!("key={}", cfg.api_key);
        let url = crate::core::realtime::scaffold::apply_endpoint_override(
            &default_url,
            cfg.realtime_endpoint_override.as_deref(),
            Some(&auth_query),
        );
        Ok(ConnectSpec::WebSocket {
            url,
            headers: Vec::new(),
        })
    }

    fn build_session_config(
        &self,
        cfg: &RealtimeConfig,
        resumption: Option<&str>,
    ) -> Vec<Self::Wire> {
        // generationConfig: AUDIO modality + the optional speechConfig (voice).
        let mut generation_config = json!({ "responseModalities": ["AUDIO"] });
        if let Some(sc) = Self::speech_config(cfg) {
            generation_config["speechConfig"] = sc;
        }
        if let Some(temp) = cfg.temperature {
            generation_config["temperature"] = json!(temp);
        }

        // setup.model carries `models/<model>` (Gemini's fully-qualified form).
        let mut setup = json!({
            "model": format!("models/{}", self.model),
            "generationConfig": generation_config,
        });

        // systemInstruction (skip-if-none): {"parts":[{"text":…}]}.
        if let Some(instr) = cfg.instructions.as_deref()
            && !instr.is_empty()
        {
            setup["systemInstruction"] = json!({ "parts": [{ "text": instr }] });
        }

        // tools (skip-if-none): one {"functionDeclarations":[…]} entry.
        if let Some(tools) = Self::tools(cfg) {
            setup["tools"] = tools;
        }

        // Input transcription: enable when the gateway requested it. Empty object
        // = "transcribe user audio with defaults".
        if cfg.input_audio_transcription.is_some() {
            setup["inputAudioTranscription"] = json!({});
        }

        // Output (assistant) transcription: ALWAYS on, so AUDIO-modality responses
        // ALSO surface the assistant's text transcript — matching OpenAI/Deepgram/
        // ElevenLabs (which deliver assistant transcripts by default) and Pipecat
        // (which enables this unconditionally). Without it Gemini emits audio only
        // and the `outputTranscription` mapping can never fire (review wf_d0b9713b #1).
        setup["outputAudioTranscription"] = json!({});

        // sessionResumption: on a RECONNECT, carry the stored handle so the server
        // resumes the session instead of starting fresh. THE resumption flow.
        // `{"sessionResumption":{"handle":<h>}}`; on a FRESH connect we still send
        // `{"sessionResumption":{}}` to ASK the server to issue handles (Gemini
        // only emits sessionResumptionUpdate if resumption was requested).
        setup["sessionResumption"] = match resumption {
            Some(handle) => json!({ "handle": handle }),
            None => json!({}),
        };

        vec![json!({ "setup": setup })]
    }

    fn map_server_event(&self, raw: Inbound<'_>) -> Vec<S2sEvent> {
        // Gemini Live is JSON-only (audio is base64 inside JSON); a binary frame
        // is never expected.
        let text = match raw {
            Inbound::Text(t) => t,
            Inbound::Binary(_) => return vec![S2sEvent::Ignore],
        };

        let value: Value = match serde_json::from_str(text) {
            Ok(v) => v,
            Err(_) => return vec![S2sEvent::Ignore],
        };

        // setupComplete ⇒ SessionReady (the session handshake finished).
        if value.get("setupComplete").is_some() {
            return vec![S2sEvent::SessionReady { session_id: None }];
        }

        // serverContent — the MULTI-FRAME case (audio + text parts + turn flags).
        if let Some(sc) = value.get("serverContent") {
            return Self::map_server_content(sc);
        }

        // toolCall ⇒ one FunctionCall per functionCalls[] entry.
        if let Some(tc) = value.get("toolCall") {
            return Self::map_tool_call(tc);
        }

        // sessionResumptionUpdate ⇒ ResumptionHandle (driver stores it, feeds it
        // back into build_session_config on reconnect). Only emit when the server
        // marks it resumable AND provides a non-empty handle.
        if let Some(update) = value.get("sessionResumptionUpdate") {
            let resumable = update
                .get("resumable")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if let Some(handle) = update.get("newHandle").and_then(Value::as_str)
                && resumable
                && !handle.is_empty()
            {
                return vec![S2sEvent::ResumptionHandle(handle.to_string())];
            }
            return vec![S2sEvent::Ignore];
        }

        // toolCallCancellation: the server cancelled pending tool calls; nothing
        // for the gateway to forward (the call ids would already be in flight).
        // goAway: the server warns of an imminent disconnect — the scaffold's
        // reconnect (with the stored resumption handle) handles it. Both ⇒ Ignore.
        vec![S2sEvent::Ignore]
    }

    fn encode_user_audio(&self, pcm: &[u8]) -> Self::Wire {
        // base64-in-JSON: realtimeInput.mediaChunks[0] with the 16 kHz input mime.
        json!({
            "realtimeInput": {
                "mediaChunks": [{
                    "mimeType": format!("audio/pcm;rate={INPUT_SAMPLE_RATE}"),
                    "data": BASE64_STANDARD.encode(pcm),
                }]
            }
        })
    }

    fn send_text(&self, text: &str) -> Vec<Self::Wire> {
        // A typed user turn → clientContent with turnComplete=true so the model
        // generates a response. `{"clientContent":{"turns":[{"role":"user",
        // "parts":[{"text":…}]}],"turnComplete":true}}`.
        vec![json!({
            "clientContent": {
                "turns": [{ "role": "user", "parts": [{ "text": text }] }],
                "turnComplete": true,
            }
        })]
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
        // Gemini has no client-driven response cancel; barge-in is server-side
        // (`interrupted`). Safest to send nothing.
        Vec::new()
    }

    fn format_tool_result(&self, call_id: &str, result: &str) -> Vec<Self::Wire> {
        // Tool result → toolResponse.functionResponses[]. Gemini keys the
        // response by the function-call `id` and wraps the payload in `response`.
        // The result string is parsed back to JSON when possible (Gemini wants a
        // structured object), falling back to `{"result": <string>}`.
        let response: Value = serde_json::from_str(result)
            .ok()
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({ "result": result }));
        vec![json!({
            "toolResponse": {
                "functionResponses": [{ "id": call_id, "response": response }]
            }
        })]
    }

    fn serialize(&self, msg: &Self::Wire) -> RealtimeResult<OutFrame> {
        // Gemini is JSON-only (audio is base64 inside JSON) ⇒ ALWAYS a text frame.
        Ok(OutFrame::Text(serde_json::to_string(msg).map_err(|e| {
            RealtimeError::SerializationError(e.to_string())
        })?))
    }
}

// =============================================================================
// Tests — THE ORACLE (no live Gemini key held; unit-validated against the
// authoritative BidiGenerateContent wire + the Pipecat reference).
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::realtime::base::{FunctionDefinition, ToolDefinition};

    fn base_cfg() -> RealtimeConfig {
        RealtimeConfig {
            provider: "gemini".into(),
            api_key: "gkey".into(),
            model: "gemini-2.0-flash-live-001".into(),
            ..Default::default()
        }
    }

    fn proto(cfg: &RealtimeConfig) -> GeminiProtocol {
        GeminiProtocol::from_config(cfg).unwrap()
    }

    /// from_config rejects an empty api key (the `?key=` query auth requires it).
    #[test]
    fn from_config_requires_api_key() {
        let cfg = RealtimeConfig {
            api_key: String::new(),
            ..base_cfg()
        };
        assert!(matches!(
            GeminiProtocol::from_config(&cfg),
            Err(RealtimeError::AuthenticationFailed(_))
        ));
    }

    /// from_config defaults the model to the current Live model when empty.
    #[test]
    fn from_config_defaults_model() {
        let cfg = RealtimeConfig {
            api_key: "gkey".into(),
            model: String::new(),
            ..Default::default()
        };
        let p = GeminiProtocol::from_config(&cfg).unwrap();
        assert_eq!(p.model(), GEMINI_LIVE_DEFAULT_MODEL);
    }

    /// connect_spec: the api key is in the `?key=` QUERY (NOT a header), on the
    /// exact BidiGenerateContent URL.
    #[test]
    fn connect_spec_puts_key_in_query_not_header() {
        let p = proto(&base_cfg());
        let ConnectSpec::WebSocket { url, headers } = p.connect_spec(&base_cfg()).unwrap() else {
            panic!("expected ConnectSpec::WebSocket")
        };
        assert!(
            url.starts_with(GEMINI_LIVE_URL),
            "URL must be the BidiGenerateContent endpoint: {url}"
        );
        assert!(url.contains("BidiGenerateContent"), "URL: {url}");
        assert!(url.ends_with("?key=gkey"), "key must be the ?key= query: {url}");
        // NO auth header at all (Gemini auths by query param).
        assert!(headers.is_empty(), "Gemini sends NO headers, got: {headers:?}");
    }

    /// SERVER-CONFIG `realtime_endpoint_override` WINS over the Live host; because
    /// Gemini auths by QUERY, the `?key=` is APPENDED when the override has no query
    /// of its own. Server-config-only (never client-settable).
    #[test]
    fn connect_spec_honors_server_endpoint_override_appends_key() {
        let cfg = RealtimeConfig {
            realtime_endpoint_override: Some("ws://127.0.0.1:9007/gem".into()),
            ..base_cfg()
        };
        let ConnectSpec::WebSocket { url, .. } = proto(&cfg).connect_spec(&cfg).unwrap() else {
            panic!("expected WebSocket")
        };
        assert_eq!(url, "ws://127.0.0.1:9007/gem?key=gkey");
    }

    /// When the override ALREADY carries a query, the `?key=` is NOT re-appended
    /// (the override is used verbatim — its query owns auth routing to the proxy).
    #[test]
    fn connect_spec_override_with_query_used_verbatim() {
        let cfg = RealtimeConfig {
            realtime_endpoint_override: Some("ws://127.0.0.1:9007/gem?key=mockkey".into()),
            ..base_cfg()
        };
        let ConnectSpec::WebSocket { url, .. } = proto(&cfg).connect_spec(&cfg).unwrap() else {
            panic!("expected WebSocket")
        };
        assert_eq!(url, "ws://127.0.0.1:9007/gem?key=mockkey");
    }

    /// build_session_config(None): a `setup` with responseModalities AUDIO + the
    /// systemInstruction, and a (fresh) sessionResumption REQUEST object.
    #[test]
    fn build_session_config_fresh_setup() {
        let cfg = RealtimeConfig {
            instructions: Some("Be terse.".into()),
            voice: Some("Puck".into()),
            ..base_cfg()
        };
        let p = proto(&cfg);
        let wires = p.build_session_config(&cfg, None);
        assert_eq!(wires.len(), 1);
        let setup = &wires[0]["setup"];
        assert_eq!(setup["model"], "models/gemini-2.0-flash-live-001");
        assert_eq!(setup["generationConfig"]["responseModalities"][0], "AUDIO");
        assert_eq!(
            setup["generationConfig"]["speechConfig"]["voiceConfig"]["prebuiltVoiceConfig"]
                ["voiceName"],
            "Puck"
        );
        assert_eq!(setup["systemInstruction"]["parts"][0]["text"], "Be terse.");
        // Assistant transcription is ALWAYS requested so AUDIO responses surface a
        // text transcript (review wf_d0b9713b #1).
        assert!(setup["outputAudioTranscription"].is_object());
        // Fresh connect ⇒ an EMPTY sessionResumption object (asks for handles),
        // with NO handle field.
        assert!(setup["sessionResumption"].is_object());
        assert!(setup["sessionResumption"].get("handle").is_none());
    }

    /// THE RESUMPTION FLOW: build_session_config(Some("h")) ⇒ the setup carries
    /// sessionResumption.handle == "h" (fed back on reconnect).
    #[test]
    fn build_session_config_carries_resumption_handle() {
        let p = proto(&base_cfg());
        let wires = p.build_session_config(&base_cfg(), Some("resume-token-xyz"));
        assert_eq!(wires.len(), 1);
        assert_eq!(
            wires[0]["setup"]["sessionResumption"]["handle"],
            "resume-token-xyz"
        );
    }

    /// build_session_config emits `tools` (grouped functionDeclarations) and
    /// inputAudioTranscription when requested.
    #[test]
    fn build_session_config_tools_and_transcription() {
        use crate::core::realtime::base::InputTranscriptionConfig;
        let cfg = RealtimeConfig {
            tools: Some(vec![ToolDefinition {
                tool_type: "function".into(),
                function: FunctionDefinition {
                    name: "get_weather".into(),
                    description: Some("Look up weather".into()),
                    parameters: Some(json!({"type":"object","properties":{}})),
                },
            }]),
            input_audio_transcription: Some(InputTranscriptionConfig {
                model: "whatever".into(),
            }),
            ..base_cfg()
        };
        let p = proto(&cfg);
        let setup = &p.build_session_config(&cfg, None)[0]["setup"];
        let decl = &setup["tools"][0]["functionDeclarations"][0];
        assert_eq!(decl["name"], "get_weather");
        assert_eq!(decl["description"], "Look up weather");
        assert_eq!(decl["parameters"]["type"], "object");
        assert!(setup["inputAudioTranscription"].is_object());
    }

    /// encode_user_audio ⇒ realtimeInput.mediaChunks[0] with base64 PCM + the
    /// 16 kHz input mime; serializes to a TEXT frame (never binary).
    #[test]
    fn encode_user_audio_is_base64_in_json() {
        let p = proto(&base_cfg());
        let pcm: Vec<u8> = vec![0x00, 0x01, 0xFE, 0xFF, 0x7F, 0x80];
        let wire = p.encode_user_audio(&pcm);
        let chunk = &wire["realtimeInput"]["mediaChunks"][0];
        assert_eq!(chunk["mimeType"], "audio/pcm;rate=16000");
        let b64 = chunk["data"].as_str().expect("data is base64 string");
        assert_eq!(BASE64_STANDARD.decode(b64).unwrap(), pcm, "base64 round-trips");
        match p.serialize(&wire).unwrap() {
            OutFrame::Text(s) => assert!(s.contains("realtimeInput")),
            OutFrame::Binary(_) => panic!("Gemini audio must be a JSON text frame"),
        }
    }

    /// THE PLURALITY TEST (MULTI-FRAME): a serverContent with TWO parts (one
    /// inlineData audio + one text) and turnComplete:true ⇒ a Vec of exactly
    /// [Audio, Transcript(final), ResponseDone] — one wire message → N events.
    #[test]
    fn server_content_multi_frame_maps_to_vec() {
        let p = proto(&base_cfg());
        let pcm: &[u8] = &[1, 2, 3, 4, 250, 251];
        let b64 = BASE64_STANDARD.encode(pcm);
        let raw = format!(
            r#"{{"serverContent":{{"modelTurn":{{"parts":[{{"inlineData":{{"mimeType":"audio/pcm;rate=24000","data":"{b64}"}}}},{{"text":"hello there"}}]}},"turnComplete":true}}}}"#
        );
        let events = p.map_server_event(Inbound::Text(&raw));
        assert_eq!(
            events.len(),
            3,
            "expected [Audio, Transcript, ResponseDone], got {events:?}"
        );
        match &events[0] {
            S2sEvent::Audio { data, .. } => assert_eq!(data.as_ref(), pcm),
            other => panic!("event[0] must be Audio, got {other:?}"),
        }
        match &events[1] {
            S2sEvent::Transcript {
                role,
                text,
                is_final,
                ..
            } => {
                assert_eq!(*role, TranscriptRole::Assistant);
                assert_eq!(text, "hello there");
                assert!(*is_final, "turnComplete ⇒ Transcript is_final");
            }
            other => panic!("event[1] must be Transcript, got {other:?}"),
        }
        match &events[2] {
            S2sEvent::ResponseDone { .. } => {}
            other => panic!("event[2] must be ResponseDone, got {other:?}"),
        }
    }

    /// MULTI-FRAME with interrupted:true ⇒ the Vec includes InterruptedByServer.
    #[test]
    fn server_content_interrupted_pushes_interrupt() {
        let p = proto(&base_cfg());
        let raw = r#"{"serverContent":{"modelTurn":{"parts":[{"text":"partial"}]},"interrupted":true}}"#;
        let events = p.map_server_event(Inbound::Text(raw));
        assert!(
            events
                .iter()
                .any(|e| matches!(e, S2sEvent::InterruptedByServer)),
            "interrupted:true must yield InterruptedByServer, got {events:?}"
        );
        // No turnComplete ⇒ the text Transcript is NOT final and there is no
        // ResponseDone.
        assert!(events.iter().any(|e| matches!(
            e,
            S2sEvent::Transcript { is_final: false, .. }
        )));
        assert!(!events.iter().any(|e| matches!(e, S2sEvent::ResponseDone { .. })));
    }

    /// Multiple audio parts in one serverContent ⇒ one Audio per part.
    #[test]
    fn server_content_multiple_audio_parts() {
        let p = proto(&base_cfg());
        let a = BASE64_STANDARD.encode([1u8, 2]);
        let b = BASE64_STANDARD.encode([3u8, 4]);
        let raw = format!(
            r#"{{"serverContent":{{"modelTurn":{{"parts":[{{"inlineData":{{"mimeType":"audio/pcm;rate=24000","data":"{a}"}}}},{{"inlineData":{{"mimeType":"audio/pcm;rate=24000","data":"{b}"}}}}]}}}}}}"#
        );
        let events = p.map_server_event(Inbound::Text(&raw));
        let audio_count = events
            .iter()
            .filter(|e| matches!(e, S2sEvent::Audio { .. }))
            .count();
        assert_eq!(audio_count, 2, "two inlineData parts ⇒ two Audio events");
    }

    /// inputTranscription ⇒ User Transcript; outputTranscription ⇒ Assistant.
    #[test]
    fn transcription_maps_per_role() {
        let p = proto(&base_cfg());
        let user = r#"{"serverContent":{"inputTranscription":{"text":"what time is it"}}}"#;
        match p.map_server_event(Inbound::Text(user)).as_slice() {
            [S2sEvent::Transcript { role, text, .. }] => {
                assert_eq!(*role, TranscriptRole::User);
                assert_eq!(text, "what time is it");
            }
            other => panic!("expected User Transcript, got {other:?}"),
        }
        let asst = r#"{"serverContent":{"outputTranscription":{"text":"it is noon"}}}"#;
        match p.map_server_event(Inbound::Text(asst)).as_slice() {
            [S2sEvent::Transcript { role, text, .. }] => {
                assert_eq!(*role, TranscriptRole::Assistant);
                assert_eq!(text, "it is noon");
            }
            other => panic!("expected Assistant Transcript, got {other:?}"),
        }
    }

    /// setupComplete ⇒ SessionReady.
    #[test]
    fn setup_complete_maps_to_session_ready() {
        let p = proto(&base_cfg());
        match p.map_server_event(Inbound::Text(r#"{"setupComplete":{}}"#)).as_slice() {
            [S2sEvent::SessionReady { session_id }] => assert!(session_id.is_none()),
            other => panic!("expected SessionReady, got {other:?}"),
        }
    }

    /// THE RESUMPTION INBOUND: sessionResumptionUpdate (resumable + newHandle) ⇒
    /// ResumptionHandle("…").
    #[test]
    fn resumption_update_maps_to_handle() {
        let p = proto(&base_cfg());
        let raw = r#"{"sessionResumptionUpdate":{"newHandle":"handle-abc","resumable":true}}"#;
        match p.map_server_event(Inbound::Text(raw)).as_slice() {
            [S2sEvent::ResumptionHandle(h)] => assert_eq!(h, "handle-abc"),
            other => panic!("expected ResumptionHandle, got {other:?}"),
        }
        // A non-resumable update (no usable handle yet) ⇒ Ignore.
        let not_yet = r#"{"sessionResumptionUpdate":{"resumable":false}}"#;
        assert!(matches!(
            p.map_server_event(Inbound::Text(not_yet)).as_slice(),
            [S2sEvent::Ignore]
        ));
    }

    /// toolCall ⇒ one FunctionCall per functionCalls[] entry (id/name/args).
    #[test]
    fn tool_call_maps_to_function_calls() {
        let p = proto(&base_cfg());
        let raw = r#"{"toolCall":{"functionCalls":[{"id":"call_1","name":"get_weather","args":{"city":"NYC"}},{"id":"call_2","name":"get_time","args":{}}]}}"#;
        let events = p.map_server_event(Inbound::Text(raw));
        assert_eq!(events.len(), 2, "two functionCalls ⇒ two FunctionCall events");
        match &events[0] {
            S2sEvent::FunctionCall(req) => {
                assert_eq!(req.call_id, "call_1");
                assert_eq!(req.name, "get_weather");
                let args: Value = serde_json::from_str(&req.arguments).unwrap();
                assert_eq!(args["city"], "NYC");
            }
            other => panic!("expected FunctionCall, got {other:?}"),
        }
        match &events[1] {
            S2sEvent::FunctionCall(req) => assert_eq!(req.name, "get_time"),
            other => panic!("expected FunctionCall, got {other:?}"),
        }
    }

    /// goAway / toolCallCancellation / unknown / non-JSON / binary ⇒ Ignore.
    #[test]
    fn go_away_cancellation_unknown_and_binary_ignore() {
        let p = proto(&base_cfg());
        for raw in [
            r#"{"goAway":{"timeLeft":"5s"}}"#,
            r#"{"toolCallCancellation":{"ids":["call_1"]}}"#,
            r#"{"somethingNew":{}}"#,
            "not json",
        ] {
            assert!(
                matches!(p.map_server_event(Inbound::Text(raw)).as_slice(), [S2sEvent::Ignore]),
                "expected Ignore for {raw}"
            );
        }
        assert!(matches!(
            p.map_server_event(Inbound::Binary(&[1, 2, 3])).as_slice(),
            [S2sEvent::Ignore]
        ));
    }

    /// send_text ⇒ clientContent with turnComplete; format_tool_result ⇒
    /// toolResponse.functionResponses; turn/cancel controls are empty (server VAD).
    #[test]
    fn send_text_tool_result_and_empty_turn_controls() {
        let p = proto(&base_cfg());
        match p.send_text("typed turn").as_slice() {
            [v] => {
                assert_eq!(v["clientContent"]["turns"][0]["role"], "user");
                assert_eq!(
                    v["clientContent"]["turns"][0]["parts"][0]["text"],
                    "typed turn"
                );
                assert_eq!(v["clientContent"]["turnComplete"], true);
            }
            other => panic!("expected one clientContent, got {other:?}"),
        }
        // Object result ⇒ passed through as the structured response.
        match p.format_tool_result("call_1", r#"{"temp":"72F"}"#).as_slice() {
            [v] => {
                let fr = &v["toolResponse"]["functionResponses"][0];
                assert_eq!(fr["id"], "call_1");
                assert_eq!(fr["response"]["temp"], "72F");
            }
            other => panic!("expected toolResponse, got {other:?}"),
        }
        // Non-JSON result ⇒ wrapped as {"result": <string>}.
        match p.format_tool_result("call_2", "72F sunny").as_slice() {
            [v] => {
                assert_eq!(
                    v["toolResponse"]["functionResponses"][0]["response"]["result"],
                    "72F sunny"
                );
            }
            other => panic!("expected wrapped toolResponse, got {other:?}"),
        }
        // Server VAD owns turns: create/commit/cancel all empty.
        assert!(p.create_response(None).is_empty());
        assert!(p.commit_turn().is_empty());
        assert!(p.cancel_response().is_empty());
        // Default-empty truncate / input-buffer (provider lacks them).
        assert!(p.truncate("x", 1).is_empty());
        assert!(p.clear_input_buffer().is_empty());
    }

    /// caps: server-VAD turn frames, 48 B/ms @ 24 kHz output, NO truncate, NO
    /// input buffer.
    #[test]
    fn caps_describe_server_vad_pcm24k_output() {
        let c = proto(&base_cfg()).caps();
        assert!(c.emits_user_turn_frames);
        assert_eq!(c.output_bytes_per_ms, 48);
        assert_eq!(c.output_sample_rate, 24_000);
        assert!(!c.supports_truncate);
        assert!(!c.supports_input_buffer);
    }
}
