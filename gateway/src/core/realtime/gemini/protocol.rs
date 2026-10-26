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
    RealtimeVerb, SpeechEvent, TranscriptRole, TurnDetectionConfig,
};
use crate::core::realtime::scaffold::{
    ConnectSpec, Inbound, OutFrame, ProtocolCaps, RealtimeProtocol, S2sEvent, UsageReport,
};

/// A protobuf `Duration` in its JSON form (`"12.5s"`).
fn parse_proto_duration(s: &str) -> Option<std::time::Duration> {
    let secs: f64 = s.trim().strip_suffix('s')?.trim().parse().ok()?;
    (secs.is_finite() && secs >= 0.0).then(|| std::time::Duration::from_secs_f64(secs))
}

/// Gemini Live BidiGenerateContent WebSocket endpoint. The `?key=<API_KEY>`
/// query is appended in `connect_spec` (Gemini auths by QUERY param, not header).
pub(crate) const GEMINI_LIVE_URL: &str = "wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent";

/// The Gemini Live model used when `cfg.model` is empty (FRD-023 F-4). It was
/// `gemini-2.0-flash-live-001`, which Google shut down on 2025-12-09; `gemini-3.8-live` is on
/// Google's pricing page as of 2026-09-24. A Bud deployment always names its model
/// (`voice_table.model`), so this default serves only the native `/realtime` path.
pub(crate) const GEMINI_LIVE_DEFAULT_MODEL: &str = "gemini-3.8-live";

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
    /// Provider fix `gemini_tool_call_cancellation`: `toolCallCancellation` is mapped.
    map_cancellation: bool,
    /// Provider fix `gemini_interaction_status`: `serverContent.interactionStatus` is mapped.
    map_status: bool,
    /// Provider fix `gemini_realtime_audio`: the caller's audio is `realtimeInput.audio`
    /// (`mediaChunks` is deprecated in the Live API).
    audio_input: bool,
    /// Provider fix `gemini_response_name`: a function response names its function.
    response_name: bool,
    /// Provider fix `setup_once`: a changed setup is refused, never sent on the open socket
    /// (the Live API takes `setup` only as its first message).
    setup_once: bool,
    /// The name of each open function call, by id (`gemini_response_name`).
    calls: std::sync::Mutex<std::collections::HashMap<String, String>>,
    /// Provider fix `gemini_response_scheduling`: an interaction session's delegate result
    /// names its delivery (`waav_scheduling` in the result becomes `scheduling`).
    scheduling: bool,
    /// Provider fix `gemini_turn_transcripts`: each turn's transcripts end in one final per
    /// role (`turn` holds the caller's and the model's words of the turn so far).
    turn_transcripts: bool,
    turn: std::sync::Mutex<(String, String)>,
    /// Provider fix `gemini_context_compression`: `setup.contextWindowCompression`.
    compression: bool,
    /// Provider fix `gemini_turn_detection`: the session's turn detection is
    /// `realtimeInputConfig`; `manual` turns are the client's `activityStart` / `activityEnd`.
    turn_detection: bool,
    manual: bool,
    /// Provider fix `gemini_await_setup`: nothing else goes before `setupComplete`.
    await_setup: bool,
    /// Provider fix `gemini_update_resume`: an update is a new connection resuming the session.
    update_resume: bool,
    /// Provider fix `gemini_max_output_tokens`: `generationConfig.maxOutputTokens`.
    max_tokens: bool,
    /// Provider fix `gemini_inbound`: thought parts are not transcripts; `voiceActivity` is
    /// the caller's speech start and end.
    inbound: bool,
}

/// The model name without `models/`, in lower case.
fn bare_model(model: &str) -> String {
    model
        .trim()
        .trim_start_matches("models/")
        .to_ascii_lowercase()
}

/// Whether a function response may carry `scheduling` on `model`: Gemini 3.8 Live has
/// "Function scheduling (`SILENT`, `WHEN_IDLE`, `INTERRUPTED`)"; on its extended-thinking
/// variant "Blocking mode and function scheduling are not supported", and older models run
/// functions sequentially (<https://ai.google.dev/gemini-api/docs/live-api/capabilities>).
fn scheduling_supported(model: &str) -> bool {
    let model = bare_model(model);
    model.starts_with("gemini-3.8-live") && !model.starts_with("gemini-3.8-live-extended-thinking")
}

/// The `realtimeInput` that opens the client's next turn when Gemini's own activity detection
/// is off (fix `gemini_turn_detection`, manual turns).
fn activity_start() -> Value {
    json!({ "realtimeInput": { "activityStart": {} } })
}

/// The `behavior` a function declaration carries under the provider fix
/// `gemini_tool_behavior` (plan W6 Decision 6), by model: on `gemini-3.8-live` a client's
/// function blocks and the gateway's delegate function does not; on the extended-thinking
/// model every function is non-blocking (async only); older models get none (synchronous
/// only). Enum `Behavior` {`BLOCKING`, `NON_BLOCKING`} of google-genai's `FunctionDeclaration`
/// (<https://raw.githubusercontent.com/googleapis/python-genai/main/google/genai/types.py>).
fn tool_behavior(model: &str, name: &str) -> Option<&'static str> {
    let model = bare_model(model);
    if model.starts_with("gemini-3.8-live-extended-thinking") {
        Some("NON_BLOCKING")
    } else if model.starts_with("gemini-3.8-live") {
        Some(
            if name == crate::core::interaction::provider::DELEGATE_TOOL {
                "NON_BLOCKING"
            } else {
                "BLOCKING"
            },
        )
    } else {
        None
    }
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
        let voice = cfg
            .voice
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())?;
        Some(json!({
            "voiceConfig": { "prebuiltVoiceConfig": { "voiceName": voice } }
        }))
    }

    /// Build the `tools` array from `cfg.tools` (Gemini groups all function
    /// declarations under a single `{"functionDeclarations":[…]}` tool). Returns
    /// `None` when no tools — the field is then omitted from `setup`.
    fn tools(&self, cfg: &RealtimeConfig) -> Option<Value> {
        let tools = cfg.tools.as_ref()?;
        if tools.is_empty() {
            return None;
        }
        let behavior = cfg.fix("gemini_tool_behavior");
        // Fix `gemini_json_schema`: the client's JSON Schema is `parametersJsonSchema` ("in JSON
        // Schema format"), not `parameters` ("a select subset of an OpenAPI 3.0 schema object"),
        // and the required `description` is never left out.
        let json_schema = cfg.fix("gemini_json_schema");
        let decls: Vec<Value> = tools
            .iter()
            .map(|t| {
                let mut decl = json!({ "name": t.function.name });
                if let Some(desc) = &t.function.description {
                    decl["description"] = json!(desc);
                } else if json_schema {
                    decl["description"] = json!("");
                }
                if let Some(params) = &t.function.parameters {
                    let key = if json_schema {
                        "parametersJsonSchema"
                    } else {
                        "parameters"
                    };
                    decl[key] = params.clone();
                }
                if let Some(value) =
                    tool_behavior(&self.model, &t.function.name).filter(|_| behavior)
                {
                    decl["behavior"] = json!(value);
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
    fn map_server_content(&self, sc: &Value) -> Vec<S2sEvent> {
        let mut out = Vec::new();

        let turn_complete = sc
            .get("turnComplete")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let interrupted = sc
            .get("interrupted")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        // ONE S2sEvent PER part of modelTurn.parts[].
        if let Some(parts) = sc
            .get("modelTurn")
            .and_then(|mt| mt.get("parts"))
            .and_then(Value::as_array)
        {
            for part in parts {
                // `Part.thought`: "Indicates if the part is thought from the model" (fix
                // `gemini_inbound`): never audio or a transcript.
                if self.inbound && part.get("thought").and_then(Value::as_bool) == Some(true) {
                    continue;
                }
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
                    out.push(self.heard(TranscriptRole::Assistant, text, turn_complete));
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
            out.push(self.heard(TranscriptRole::User, text, false));
        }
        if let Some(text) = sc
            .get("outputTranscription")
            .and_then(|t| t.get("text"))
            .and_then(Value::as_str)
            && !text.is_empty()
        {
            out.push(self.heard(TranscriptRole::Assistant, text, turn_complete));
        }
        // Fix `gemini_turn_transcripts`: the turn's words, one final per role, when the model's
        // turn ends or is cut. Transcription carries no end of its own (`text`, `languageCode`,
        // offsets), and arrives "independently of the other server messages".
        if self.turn_transcripts && (turn_complete || interrupted) {
            let (caller, model) =
                std::mem::take(&mut *self.turn.lock().unwrap_or_else(|p| p.into_inner()));
            for (role, text) in [
                (TranscriptRole::User, caller),
                (TranscriptRole::Assistant, model),
            ] {
                if !text.is_empty() {
                    out.push(S2sEvent::Transcript {
                        role,
                        text,
                        is_final: true,
                        item_id: None,
                    });
                }
            }
        }

        // Barge-in: the server stopped the model turn (user interrupted). The
        // driver runs cancel + clears local playback (Gemini has no truncate).
        if interrupted {
            out.push(S2sEvent::InterruptedByServer);
        }
        // Provider fix `gemini_interaction_status`: whether the server still works after its
        // turn (extended thinking). `turnComplete` keeps producing `ResponseDone`. Values
        // `IN_PROGRESS` / `IDLE` (`REQUIRES_ACTION` is the deprecated `IDLE`):
        // <https://ai.google.dev/api/live> `BidiGenerateContentServerContent.interactionStatus`.
        let status = sc.get("interactionStatus").and_then(Value::as_str);
        if let Some(status) = status.filter(|_| self.map_status) {
            match status {
                "IN_PROGRESS" => out.push(S2sEvent::ProviderBusy(true)),
                "IDLE" | "REQUIRES_ACTION" => out.push(S2sEvent::ProviderBusy(false)),
                _ => {}
            }
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

    /// One transcript chunk. Under the fix `gemini_turn_transcripts` it is interim and kept
    /// for the turn's final; without it today's: a user chunk interim, a model chunk final
    /// when it rides with `turnComplete`.
    fn heard(&self, role: TranscriptRole, text: &str, turn_complete: bool) -> S2sEvent {
        let is_final = if self.turn_transcripts {
            let mut turn = self.turn.lock().unwrap_or_else(|p| p.into_inner());
            match role {
                TranscriptRole::User => turn.0.push_str(text),
                TranscriptRole::Assistant => turn.1.push_str(text),
            }
            false
        } else {
            role == TranscriptRole::Assistant && turn_complete
        };
        S2sEvent::Transcript {
            role,
            text: text.to_string(),
            is_final,
            item_id: None,
        }
    }

    /// `realtimeInputConfig` from the session's turn detection (fix `gemini_turn_detection`):
    /// manual turns turn `automaticActivityDetection` off; server VAD's `silence_duration_ms`
    /// is `silenceDurationMs` ("duration of detected non-speech ... before end-of-speech is
    /// committed"), and `interrupt_response: false` is `activityHandling: NO_INTERRUPTION`.
    /// OpenAI's `prefix_padding_ms` (audio kept before speech) is not Gemini's `prefixPaddingMs`
    /// ("duration of detected speech before start-of-speech is committed"), so it is not sent.
    fn realtime_input_config(cfg: &RealtimeConfig) -> Option<Value> {
        let mut config = serde_json::Map::new();
        let interrupt = match cfg.turn_detection.as_ref()? {
            TurnDetectionConfig::None => {
                return Some(json!({ "automaticActivityDetection": { "disabled": true } }));
            }
            TurnDetectionConfig::ServerVad {
                silence_duration_ms,
                interrupt_response,
                ..
            } => {
                if let Some(ms) = silence_duration_ms {
                    config.insert(
                        "automaticActivityDetection".into(),
                        json!({ "silenceDurationMs": ms }),
                    );
                }
                *interrupt_response
            }
            TurnDetectionConfig::SemanticVad {
                interrupt_response, ..
            } => *interrupt_response,
        };
        if interrupt == Some(false) {
            config.insert("activityHandling".into(), json!("NO_INTERRUPTION"));
        }
        (!config.is_empty()).then_some(Value::Object(config))
    }

    /// `usageMetadata` → one cumulative [`UsageReport`]. Per modality from the `*TokensDetails`
    /// lists (`TEXT`, `AUDIO`, `IMAGE`; `VIDEO` frames bill as image); a vendor that sends only
    /// the totals has them attributed to text rather than dropped. `cachedContentTokenCount` is a
    /// SUBSET of `promptTokenCount` (Google's contract), which is what the cost formula expects;
    /// thinking tokens bill at the output-text rate.
    fn map_usage(u: &Value) -> S2sEvent {
        fn n(v: Option<&Value>) -> u64 {
            v.and_then(Value::as_u64).unwrap_or(0)
        }
        /// `(text, audio, image)` from a `[{modality, tokenCount}]` list, or `None` if absent.
        fn split(list: Option<&Value>) -> Option<(u64, u64, u64)> {
            let list = list?.as_array()?;
            let mut out = (0, 0, 0);
            for d in list {
                let count = n(d.get("tokenCount"));
                match d.get("modality").and_then(Value::as_str) {
                    Some("AUDIO") => out.1 += count,
                    Some("IMAGE") | Some("VIDEO") => out.2 += count,
                    _ => out.0 += count,
                }
            }
            Some(out)
        }
        let prompt =
            split(u.get("promptTokensDetails")).unwrap_or((n(u.get("promptTokenCount")), 0, 0));
        let tool = split(u.get("toolUsePromptTokensDetails")).unwrap_or((
            n(u.get("toolUsePromptTokenCount")),
            0,
            0,
        ));
        let cached = split(u.get("cacheTokensDetails")).unwrap_or((
            n(u.get("cachedContentTokenCount")),
            0,
            0,
        ));
        let response =
            split(u.get("responseTokensDetails")).unwrap_or((n(u.get("responseTokenCount")), 0, 0));
        let mut tokens = crate::core::realtime_cost::RealtimeUsage {
            input_text: prompt.0 + tool.0,
            input_audio: prompt.1 + tool.1,
            input_image: prompt.2 + tool.2,
            cached_text: cached.0,
            cached_audio: cached.1,
            cached_image: cached.2,
            output_text: response.0 + n(u.get("thoughtsTokenCount")),
            output_audio: response.1,
        };
        // A cached count above its class is a vendor inconsistency; it must never make the
        // uncached remainder negative (that would REDUCE the bill).
        tokens.cached_text = tokens.cached_text.min(tokens.input_text);
        tokens.cached_audio = tokens.cached_audio.min(tokens.input_audio);
        tokens.cached_image = tokens.cached_image.min(tokens.input_image);
        S2sEvent::Usage(UsageReport {
            tokens,
            seconds: None,
            cumulative: true,
        })
    }

    /// Everything but `usageMetadata`, which rides on any message.
    fn map_message(&self, value: &Value) -> Vec<S2sEvent> {
        // goAway: the connection closes after `timeLeft`; the driver reconnects with the
        // resumption handle at the next turn boundary (FRD-023 RT7.1, TC-XL-04).
        if let Some(ga) = value.get("goAway") {
            let time_left = ga
                .get("timeLeft")
                .and_then(Value::as_str)
                .and_then(parse_proto_duration);
            return vec![S2sEvent::GoAway { time_left }];
        }

        // serverContent — the MULTI-FRAME case (audio + text parts + turn flags).
        if let Some(sc) = value.get("serverContent") {
            return self.map_server_content(sc);
        }

        // toolCall ⇒ one FunctionCall per functionCalls[] entry.
        if let Some(tc) = value.get("toolCall") {
            let events = Self::map_tool_call(tc);
            if self.response_name
                && let Ok(mut calls) = self.calls.lock()
            {
                for event in &events {
                    if let S2sEvent::FunctionCall(call) = event {
                        calls.insert(call.call_id.clone(), call.name.clone());
                    }
                }
            }
            return events;
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

        // toolCallCancellation `{"ids":[…]}` (<https://ai.google.dev/api/live>
        // `BidiGenerateContentToolCallCancellation`): with the provider fix
        // `gemini_tool_call_cancellation` the withdrawn calls are named; else ⇒ Ignore.
        if let Some(ids) = value
            .get("toolCallCancellation")
            .filter(|_| self.map_cancellation)
            .and_then(|c| c.get("ids"))
            .and_then(Value::as_array)
        {
            let call_ids: Vec<String> = ids
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect();
            if !call_ids.is_empty() {
                return vec![S2sEvent::FunctionCallCancelled { call_ids }];
            }
        }
        vec![S2sEvent::Ignore]
    }

    fn encode_user_audio_chunk(&self, pcm: &[u8]) -> Value {
        let blob = json!({
            "mimeType": format!("audio/pcm;rate={INPUT_SAMPLE_RATE}"),
            "data": BASE64_STANDARD.encode(pcm),
        });
        // `realtimeInput.audio` (<https://ai.google.dev/api/live> `BidiGenerateContentRealtimeInput`).
        if self.audio_input {
            return json!({ "realtimeInput": { "audio": blob } });
        }
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
            map_cancellation: cfg.fix("gemini_tool_call_cancellation"),
            map_status: cfg.fix("gemini_interaction_status"),
            audio_input: cfg.fix("gemini_realtime_audio"),
            response_name: cfg.fix("gemini_response_name"),
            setup_once: cfg.fix("setup_once"),
            calls: Default::default(),
            scheduling: cfg.fix("gemini_response_scheduling"),
            turn_transcripts: cfg.fix("gemini_turn_transcripts"),
            turn: Default::default(),
            compression: cfg.fix("gemini_context_compression"),
            turn_detection: cfg.fix("gemini_turn_detection"),
            manual: matches!(cfg.turn_detection, Some(TurnDetectionConfig::None)),
            await_setup: cfg.fix("gemini_await_setup"),
            update_resume: cfg.fix("gemini_update_resume"),
            max_tokens: cfg.fix("gemini_max_output_tokens"),
            inbound: cfg.fix("gemini_inbound"),
        })
    }

    fn provider_id(&self) -> &'static str {
        "gemini"
    }

    fn input_sample_rate(&self) -> u32 {
        INPUT_SAMPLE_RATE
    }

    fn caps(&self) -> ProtocolCaps {
        ProtocolCaps {
            // Gemini runs server-side VAD + turn-taking, so it owns turns and
            // emits user-turn frames (interrupted, input transcription) — unless the client
            // takes the turns (fix `gemini_turn_detection`, manual).
            emits_user_turn_frames: !(self.turn_detection && self.manual),
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
        // Fix `gemini_max_output_tokens`: the client's cap (`-1`, no cap, sends nothing).
        if let Some(tokens) = cfg
            .max_response_output_tokens
            .filter(|t| *t > 0 && self.max_tokens)
        {
            generation_config["maxOutputTokens"] = json!(tokens);
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
        if let Some(tools) = self.tools(cfg) {
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
        // Fix `gemini_context_compression`: "Without compression, audio-only sessions are
        // limited to 15 minutes" (session management guide); the sliding window with Google's
        // defaults (trigger at 80% of the context window, keep half of it).
        if self.compression {
            setup["contextWindowCompression"] = json!({ "slidingWindow": {} });
        }
        if self.turn_detection
            && let Some(config) = Self::realtime_input_config(cfg)
        {
            setup["realtimeInputConfig"] = config;
        }

        vec![json!({ "setup": setup })]
    }

    fn awaits_session_ready(&self) -> bool {
        // "Clients should wait for a `BidiGenerateContentSetupComplete` message before sending
        // any additional messages." (fix `gemini_await_setup`)
        self.await_setup
    }

    fn update_by_reconnect(&self, old: &RealtimeConfig, new: &RealtimeConfig) -> bool {
        // "You cannot update the configuration while the connection is open. However, you can
        // change the configuration parameters, except the model, when pausing and resuming via
        // the session resumption mechanism." (fix `gemini_update_resume`)
        self.update_resume && Self::resolve_model(old) == Self::resolve_model(new)
    }

    fn session_update(&self, old: &RealtimeConfig, new: &RealtimeConfig) -> Option<Vec<Value>> {
        // `setup` is the Live API's first message only (fix `setup_once`): a second one
        // mid-session is refused by the server.
        if self.setup_once || self.update_resume {
            return None;
        }
        let _ = old;
        Some(self.build_session_config(new, None))
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
            let mut events = vec![S2sEvent::SessionReady { session_id: None }];
            // Manual turns (fix `gemini_turn_detection`): the client's first turn opens now.
            if self.turn_detection
                && self.manual
                && let Ok(frame) = self.serialize(&activity_start())
            {
                events.push(S2sEvent::SendFrame(frame));
            }
            return events;
        }

        // usageMetadata rides on any server message (usually the turn's last). It goes FIRST,
        // so it lands on the response the same message completes (FRD-023 §5.7, TC-XL-03).
        let mut events = self.map_message(&value);
        // `voiceActivity` (fix `gemini_inbound`): `ACTIVITY_START` "Start of sentence signal",
        // `ACTIVITY_END` "End of sentence signal", at `audioOffset` into the audio stream.
        if let Some(activity) = value.get("voiceActivity").filter(|_| self.inbound) {
            let at = activity
                .get("audioOffset")
                .and_then(Value::as_str)
                .and_then(parse_proto_duration)
                .map_or(0, |d| d.as_millis() as u64);
            let speech = match activity.get("type").and_then(Value::as_str) {
                Some("ACTIVITY_START") => Some(SpeechEvent::Started {
                    audio_start_ms: at,
                    item_id: None,
                }),
                Some("ACTIVITY_END") => Some(SpeechEvent::Stopped {
                    audio_end_ms: at,
                    item_id: None,
                }),
                _ => None,
            };
            if let Some(speech) = speech {
                events.retain(|e| !matches!(e, S2sEvent::Ignore));
                events.push(S2sEvent::Speech(speech));
            }
        }
        if let Some(u) = value.get("usageMetadata").map(Self::map_usage) {
            events.retain(|e| !matches!(e, S2sEvent::Ignore));
            events.insert(0, u);
        }
        events
    }

    fn encode_user_audio(&self, pcm: &[u8]) -> Self::Wire {
        self.encode_user_audio_chunk(pcm)
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
        // Manual turns (fix `gemini_turn_detection`): "The client then sends activityStart and
        // activityEnd": this turn ends and the next opens. Else server VAD owns turn close.
        if self.turn_detection && self.manual {
            return vec![
                json!({ "realtimeInput": { "activityEnd": {} } }),
                activity_start(),
            ];
        }
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
        let mut response: Value = serde_json::from_str(result)
            .ok()
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({ "result": result }));
        // `FunctionResponse.scheduling` (fix `gemini_response_scheduling`): when the model
        // tells the caller of a non-blocking call's result. Only interaction mode names it.
        // Only on a model with function scheduling ([`scheduling_supported`]).
        let scheduling = response
            .as_object_mut()
            .filter(|_| self.scheduling)
            .and_then(|object| object.remove("waav_scheduling"))
            .filter(|when| matches!(when.as_str(), Some("INTERRUPT" | "WHEN_IDLE" | "SILENT")))
            .filter(|_| scheduling_supported(&self.model));
        let mut function_response = json!({ "id": call_id, "response": response });
        if let Some(when) = scheduling {
            function_response["scheduling"] = when;
        }
        // `FunctionResponse.name` (fix `gemini_response_name`): the function the call named.
        let name = self
            .calls
            .lock()
            .ok()
            .and_then(|mut calls| calls.remove(call_id))
            .filter(|_| self.response_name);
        if let Some(name) = name {
            function_response["name"] = json!(name);
        }
        vec![json!({
            "toolResponse": {
                "functionResponses": [function_response]
            }
        })]
    }

    /// `AddContext` is a user turn the model does not answer yet: `clientContent` with
    /// `turnComplete: false` ("If true, indicates that the server content generation should
    /// start with the currently accumulated prompt"). The executor sends it only while the
    /// model is idle. Gemini has no wire for the other verbs.
    fn verb(&self, verb: &RealtimeVerb<'_>) -> Option<Vec<Self::Wire>> {
        match *verb {
            RealtimeVerb::AddContext(text) => Some(vec![json!({
                "clientContent": {
                    "turns": [{ "role": "user", "parts": [{ "text": text }] }],
                    "turnComplete": false,
                }
            })]),
            _ => None,
        }
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

    /// F-4 — the default is a model Google still serves. `gemini-2.0-flash-live-001` was shut
    /// down on 2025-12-09; a default that no longer exists fails every session that relies on it.
    #[test]
    fn f4_the_default_model_is_current() {
        assert_eq!(GEMINI_LIVE_DEFAULT_MODEL, "gemini-3.8-live");
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
        assert!(
            url.ends_with("?key=gkey"),
            "key must be the ?key= query: {url}"
        );
        // NO auth header at all (Gemini auths by query param).
        assert!(
            headers.is_empty(),
            "Gemini sends NO headers, got: {headers:?}"
        );
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
            setup["generationConfig"]["speechConfig"]["voiceConfig"]["prebuiltVoiceConfig"]["voiceName"],
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
        assert_eq!(
            BASE64_STANDARD.decode(b64).unwrap(),
            pcm,
            "base64 round-trips"
        );
        match p.serialize(&wire).unwrap() {
            OutFrame::Text(s) => assert!(s.contains("realtimeInput")),
            OutFrame::Binary(_) => panic!("Gemini audio must be a JSON text frame"),
        }
    }

    /// The fixes behind the escape flags `gemini_media_chunks`, `gemini_response_id_only` and
    /// `allow_setup_resend`: off, today's wire; on, audio as `realtimeInput.audio`, a function
    /// response with the call's `name`, and a mid-session setup refused.
    #[test]
    fn audio_function_response_and_setup_follow_their_fixes() {
        let call = r#"{"toolCall":{"functionCalls":[{"id":"c1","name":"get_weather","args":{}}]}}"#;
        let today = proto(&base_cfg());
        assert!(today.encode_user_audio(&[1, 2])["realtimeInput"]["mediaChunks"][0].is_object());
        today.map_server_event(Inbound::Text(call));
        let reply = &today.format_tool_result("c1", "{}")[0];
        assert!(
            reply["toolResponse"]["functionResponses"][0]
                .get("name")
                .is_none()
        );
        let cfg = base_cfg();
        assert!(
            today
                .session_update(&cfg, &cfg)
                .is_some_and(|w| w[0].get("setup").is_some())
        );

        let mut cfg = base_cfg();
        cfg.provider_fixes = [
            "gemini_realtime_audio",
            "gemini_response_name",
            "setup_once",
        ]
        .map(str::to_owned)
        .to_vec();
        let fixed = proto(&cfg);
        let audio = fixed.encode_user_audio(&[1, 2]);
        assert_eq!(
            audio["realtimeInput"]["audio"]["mimeType"],
            "audio/pcm;rate=16000"
        );
        assert!(audio["realtimeInput"].get("mediaChunks").is_none());
        fixed.map_server_event(Inbound::Text(call));
        let reply = &fixed.format_tool_result("c1", "{}")[0];
        let response = &reply["toolResponse"]["functionResponses"][0];
        assert_eq!(
            (response["id"].as_str(), response["name"].as_str()),
            (Some("c1"), Some("get_weather"))
        );
        // A call it did not make has no name to give.
        assert!(
            fixed.format_tool_result("c9", "{}")[0]["toolResponse"]["functionResponses"][0]
                .get("name")
                .is_none()
        );
        assert!(fixed.session_update(&cfg, &cfg).is_none());
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
        let raw =
            r#"{"serverContent":{"modelTurn":{"parts":[{"text":"partial"}]},"interrupted":true}}"#;
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
            S2sEvent::Transcript {
                is_final: false,
                ..
            }
        )));
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, S2sEvent::ResponseDone { .. }))
        );
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

    /// TC-XL-03 (protocol half) — `usageMetadata` becomes ONE cumulative `Usage`, FIRST in the
    /// message's events (so it lands on the response the same message completes), split by
    /// modality; cached tokens stay a subset of their class; thinking bills as output text.
    #[test]
    fn tc_xl_03_usage_metadata_becomes_one_cumulative_usage_first() {
        let p = proto(&base_cfg());
        let raw = json!({
            "serverContent": {"turnComplete": true},
            "usageMetadata": {
                "promptTokenCount": 120, "cachedContentTokenCount": 20,
                "responseTokenCount": 80, "thoughtsTokenCount": 5, "totalTokenCount": 205,
                "promptTokensDetails": [{"modality": "TEXT", "tokenCount": 30},
                                        {"modality": "AUDIO", "tokenCount": 90}],
                "cacheTokensDetails": [{"modality": "TEXT", "tokenCount": 20}],
                "responseTokensDetails": [{"modality": "AUDIO", "tokenCount": 70},
                                          {"modality": "TEXT", "tokenCount": 10}]
            }
        })
        .to_string();
        let evs = p.map_server_event(Inbound::Text(&raw));
        let S2sEvent::Usage(u) = &evs[0] else {
            panic!("usage first, got {evs:?}");
        };
        assert!(
            u.cumulative,
            "a Gemini report is the response's running total"
        );
        let t = u.tokens;
        assert_eq!(
            (t.input_text, t.input_audio, t.cached_text, t.cached_audio),
            (30, 90, 20, 0)
        );
        assert_eq!((t.output_text, t.output_audio), (15, 70));
        assert!(matches!(evs.last(), Some(S2sEvent::ResponseDone { .. })));

        // Totals only: attributed to text, never dropped.
        let raw = json!({"usageMetadata": {"promptTokenCount": 50, "responseTokenCount": 25}})
            .to_string();
        let evs = p.map_server_event(Inbound::Text(&raw));
        let [S2sEvent::Usage(u)] = evs.as_slice() else {
            panic!("{evs:?}");
        };
        assert_eq!((u.tokens.input_text, u.tokens.output_text), (50, 25));
    }

    /// TC-XL-04 (protocol half) — `goAway` carries its deadline to the driver.
    #[test]
    fn tc_xl_04_go_away_carries_time_left() {
        let p = proto(&base_cfg());
        let evs = p.map_server_event(Inbound::Text(r#"{"goAway":{"timeLeft":"12.5s"}}"#));
        assert!(
            matches!(evs.as_slice(), [S2sEvent::GoAway { time_left: Some(d) }] if *d == std::time::Duration::from_millis(12_500)),
            "{evs:?}"
        );
        let evs = p.map_server_event(Inbound::Text(r#"{"goAway":{}}"#));
        assert!(
            matches!(evs.as_slice(), [S2sEvent::GoAway { time_left: None }]),
            "{evs:?}"
        );
    }

    /// setupComplete ⇒ SessionReady.
    #[test]
    fn setup_complete_maps_to_session_ready() {
        let p = proto(&base_cfg());
        match p
            .map_server_event(Inbound::Text(r#"{"setupComplete":{}}"#))
            .as_slice()
        {
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
        assert_eq!(
            events.len(),
            2,
            "two functionCalls ⇒ two FunctionCall events"
        );
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

    /// toolCallCancellation / unknown / non-JSON / binary ⇒ Ignore. (`goAway` is acted on:
    /// `tc_xl_04_go_away_carries_time_left`.)
    #[test]
    fn go_away_cancellation_unknown_and_binary_ignore() {
        let p = proto(&base_cfg());
        for raw in [
            r#"{"toolCallCancellation":{"ids":["call_1"]}}"#,
            r#"{"somethingNew":{}}"#,
            "not json",
        ] {
            assert!(
                matches!(
                    p.map_server_event(Inbound::Text(raw)).as_slice(),
                    [S2sEvent::Ignore]
                ),
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
        match p
            .format_tool_result("call_1", r#"{"temp":"72F"}"#)
            .as_slice()
        {
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

    // ---- Provider fixes (plan section 11 Lifecycle track): built, off until the live probe.

    fn tool(name: &str) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".into(),
            function: FunctionDefinition {
                name: name.into(),
                description: Some(format!("{name} it")),
                parameters: Some(json!({"type": "object"})),
            },
        }
    }

    fn live38(fixes: &[&str]) -> RealtimeConfig {
        RealtimeConfig {
            model: "gemini-3.8-live".into(),
            tools: Some(vec![tool("book_table"), tool("waav_delegate")]),
            provider_fixes: fixes.iter().map(|f| (*f).to_owned()).collect(),
            ..base_cfg()
        }
    }

    /// With no fix in force the frames are today's, byte for byte: declarations without
    /// `behavior`, `toolCallCancellation` and `interactionStatus` unmapped.
    #[test]
    fn without_the_fixes_the_gemini_wire_is_todays() {
        let cfg = live38(&[]);
        let p = proto(&cfg);
        let setup = p.serialize(&p.build_session_config(&cfg, None)[0]).unwrap();
        let today = json!({"setup": {
            "model": "models/gemini-3.8-live",
            "generationConfig": {"responseModalities": ["AUDIO"]},
            "tools": [{"functionDeclarations": [
                {"name": "book_table", "description": "book_table it", "parameters": {"type": "object"}},
                {"name": "waav_delegate", "description": "waav_delegate it", "parameters": {"type": "object"}},
            ]}],
            "outputAudioTranscription": {},
            "sessionResumption": {},
        }});
        let OutFrame::Text(setup) = setup else {
            panic!("setup is a text frame")
        };
        assert_eq!(setup, serde_json::to_string(&today).unwrap());
        assert!(matches!(
            p.map_server_event(Inbound::Text(r#"{"toolCallCancellation":{"ids":["c1"]}}"#))
                .as_slice(),
            [S2sEvent::Ignore]
        ));
        let busy = r#"{"serverContent":{"turnComplete":true,"interactionStatus":"IN_PROGRESS"}}"#;
        assert!(matches!(
            p.map_server_event(Inbound::Text(busy)).as_slice(),
            [S2sEvent::ResponseDone { .. }]
        ));
    }

    /// `gemini_tool_behavior` (W6 Decision 6). Wire per google-genai's `FunctionDeclaration.
    /// behavior` / enum `Behavior` (`BLOCKING`, `NON_BLOCKING`; types.py, class `Behavior`) and
    /// the Live tools guide's `{"name": ..., "behavior": "NON_BLOCKING"}`
    /// (<https://ai.google.dev/gemini-api/docs/live-api/tools>): on 3.8 Live a client's function
    /// blocks and the gateway's delegate does not; on extended thinking all are non-blocking;
    /// older models get no field.
    #[test]
    fn the_tool_behavior_fix_declares_behavior_per_model() {
        let behaviors = |cfg: &RealtimeConfig| -> Vec<Value> {
            let p = proto(cfg);
            let setup = &p.build_session_config(cfg, None)[0]["setup"];
            setup["tools"][0]["functionDeclarations"]
                .as_array()
                .unwrap()
                .iter()
                .map(|d| d["behavior"].clone())
                .collect()
        };
        let cfg = live38(&["gemini_tool_behavior"]);
        assert_eq!(behaviors(&cfg), [json!("BLOCKING"), json!("NON_BLOCKING")]);
        let thinking = RealtimeConfig {
            model: "gemini-3.8-live-extended-thinking".into(),
            ..cfg.clone()
        };
        assert_eq!(
            behaviors(&thinking),
            [json!("NON_BLOCKING"), json!("NON_BLOCKING")]
        );
        let older = RealtimeConfig {
            model: "gemini-3.1-flash-live-preview".into(),
            ..cfg.clone()
        };
        assert_eq!(behaviors(&older), [Value::Null, Value::Null]);
        // The rest of the declaration is unchanged.
        let p = proto(&cfg);
        let decl =
            &p.build_session_config(&cfg, None)[0]["setup"]["tools"][0]["functionDeclarations"][0];
        assert_eq!(
            *decl,
            json!({"name": "book_table", "description": "book_table it",
                   "parameters": {"type": "object"}, "behavior": "BLOCKING"})
        );
    }

    /// `gemini_tool_call_cancellation`: `{"toolCallCancellation":{"ids":[...]}}`
    /// (<https://ai.google.dev/api/live>, `BidiGenerateContentToolCallCancellation.ids`) names
    /// the withdrawn calls. `gemini_interaction_status`: `serverContent.interactionStatus`
    /// `IN_PROGRESS` / `IDLE` (`REQUIRES_ACTION`: the deprecated `IDLE`), "always sent alongside
    /// `turnComplete`" (same page) is the provider's busy edge; `turnComplete` keeps
    /// producing `ResponseDone`.
    #[test]
    fn the_cancellation_and_status_fixes_map_the_server_messages() {
        let cfg = live38(&["gemini_tool_call_cancellation", "gemini_interaction_status"]);
        let p = proto(&cfg);
        match p
            .map_server_event(Inbound::Text(
                r#"{"toolCallCancellation":{"ids":["c1","c2"]}}"#,
            ))
            .as_slice()
        {
            [S2sEvent::FunctionCallCancelled { call_ids }] => assert_eq!(call_ids, &["c1", "c2"]),
            other => panic!("expected FunctionCallCancelled, got {other:?}"),
        }
        assert!(matches!(
            p.map_server_event(Inbound::Text(r#"{"toolCallCancellation":{"ids":[]}}"#))
                .as_slice(),
            [S2sEvent::Ignore]
        ));
        let busy = r#"{"serverContent":{"turnComplete":true,"interactionStatus":"IN_PROGRESS"}}"#;
        assert!(matches!(
            p.map_server_event(Inbound::Text(busy)).as_slice(),
            [S2sEvent::ProviderBusy(true), S2sEvent::ResponseDone { .. }]
        ));
        for idle in ["IDLE", "REQUIRES_ACTION"] {
            let raw = format!(
                r#"{{"serverContent":{{"turnComplete":true,"interactionStatus":"{idle}"}}}}"#
            );
            assert!(matches!(
                p.map_server_event(Inbound::Text(&raw)).as_slice(),
                [S2sEvent::ProviderBusy(false), S2sEvent::ResponseDone { .. }]
            ));
        }
        // The tool-behavior fix is separate: not listed, no `behavior`.
        let decls =
            &p.build_session_config(&cfg, None)[0]["setup"]["tools"][0]["functionDeclarations"];
        assert!(decls[0].get("behavior").is_none());
    }

    /// Provider fix `gemini_response_scheduling`: the delivery an interaction session names in
    /// a result is the response's `scheduling`; off, the result is sent as it came.
    #[test]
    fn a_delegate_result_names_its_delivery_with_the_fix() {
        let result = r#"{"status":"completed","result":"Booked.","waav_scheduling":"SILENT"}"#;
        let cfg = RealtimeConfig {
            model: "gemini-3.8-live".into(),
            provider_fixes: vec!["gemini_response_scheduling".into()],
            ..base_cfg()
        };
        let sent = &proto(&cfg).format_tool_result("c1", result)[0];
        let response = &sent["toolResponse"]["functionResponses"][0];
        assert_eq!(response["scheduling"], "SILENT");
        assert!(response["response"].get("waav_scheduling").is_none());
        assert_eq!(response["response"]["result"], "Booked.");
        // An unknown value is dropped, not sent.
        let odd = r#"{"result":"x","waav_scheduling":"LATER"}"#;
        let sent = &proto(&cfg).format_tool_result("c2", odd)[0];
        assert!(
            sent["toolResponse"]["functionResponses"][0]
                .get("scheduling")
                .is_none()
        );
        // Off: today's response.
        let sent = &proto(&base_cfg()).format_tool_result("c1", result)[0];
        let response = &sent["toolResponse"]["functionResponses"][0];
        assert!(response.get("scheduling").is_none());
        assert_eq!(response["response"]["waav_scheduling"], "SILENT");
    }

    /// `gemini_response_scheduling` only where the model schedules: not on 3.8 Live Extended
    /// Thinking ("Blocking mode and function scheduling are not supported") nor on older
    /// models; the gateway's own key never reaches the vendor.
    #[test]
    fn scheduling_goes_only_to_a_model_that_schedules() {
        let result = r#"{"result":"Booked.","waav_scheduling":"SILENT"}"#;
        for model in [
            "gemini-3.8-live-extended-thinking",
            "gemini-3.1-flash-live-preview",
        ] {
            let cfg = RealtimeConfig {
                model: model.into(),
                provider_fixes: vec!["gemini_response_scheduling".into()],
                ..base_cfg()
            };
            let sent = &proto(&cfg).format_tool_result("c1", result)[0];
            let response = &sent["toolResponse"]["functionResponses"][0];
            assert!(response.get("scheduling").is_none(), "{model}: {sent}");
            assert_eq!(
                response["response"],
                json!({"result": "Booked."}),
                "{model}"
            );
        }
    }

    /// The setup of these fixes with nothing asked of them is today's.
    fn setup_of(cfg: &RealtimeConfig) -> Value {
        proto(cfg).build_session_config(cfg, None).remove(0)["setup"].clone()
    }

    /// `gemini_turn_transcripts`: transcription has no end of its own, so each role's words
    /// of a turn end in one final when the model's turn completes or is cut, and the next
    /// turn starts empty. Without it the caller's words are never final.
    #[test]
    fn the_turn_transcripts_fix_ends_each_turn_in_one_final_per_role() {
        let chunks = [
            r#"{"serverContent":{"inputTranscription":{"text":"Book a "}}}"#,
            r#"{"serverContent":{"inputTranscription":{"text":"table."}}}"#,
            r#"{"serverContent":{"outputTranscription":{"text":"Sure, "}}}"#,
            r#"{"serverContent":{"outputTranscription":{"text":"for when?"}}}"#,
            r#"{"serverContent":{"turnComplete":true}}"#,
            r#"{"serverContent":{"inputTranscription":{"text":"Eight."}}}"#,
            r#"{"serverContent":{"interrupted":true}}"#,
        ];
        let finals = |p: &GeminiProtocol| -> Vec<String> {
            chunks
                .iter()
                .flat_map(|c| p.map_server_event(Inbound::Text(c)))
                .filter_map(|e| match e {
                    S2sEvent::Transcript {
                        role,
                        text,
                        is_final: true,
                        ..
                    } => Some(format!("{role:?}: {text}")),
                    _ => None,
                })
                .collect()
        };
        assert!(finals(&proto(&base_cfg())).is_empty());
        let fixed = RealtimeConfig {
            provider_fixes: vec!["gemini_turn_transcripts".into()],
            ..base_cfg()
        };
        assert_eq!(
            finals(&proto(&fixed)),
            [
                "User: Book a table.",
                "Assistant: Sure, for when?",
                "User: Eight."
            ]
        );
        // The chunks are still interim, as they come.
        let p = proto(&fixed);
        assert!(matches!(
            p.map_server_event(Inbound::Text(chunks[2])).as_slice(),
            [S2sEvent::Transcript {
                is_final: false,
                ..
            }]
        ));
    }

    /// `gemini_context_compression` (`{"slidingWindow":{}}`, the session guide's own config;
    /// without it audio sessions end at 15 minutes) and `gemini_max_output_tokens`
    /// (`generationConfig.maxOutputTokens`): off, the setup is today's.
    #[test]
    fn the_compression_and_token_fixes_add_their_setup_fields() {
        let cfg = RealtimeConfig {
            max_response_output_tokens: Some(256),
            ..base_cfg()
        };
        let today = setup_of(&cfg);
        assert!(today.get("contextWindowCompression").is_none());
        assert!(today["generationConfig"].get("maxOutputTokens").is_none());
        let fixed = RealtimeConfig {
            provider_fixes: vec![
                "gemini_context_compression".into(),
                "gemini_max_output_tokens".into(),
            ],
            ..cfg.clone()
        };
        let setup = setup_of(&fixed);
        assert_eq!(
            setup["contextWindowCompression"],
            json!({"slidingWindow": {}})
        );
        assert_eq!(setup["generationConfig"]["maxOutputTokens"], 256);
        let unlimited = RealtimeConfig {
            max_response_output_tokens: Some(-1),
            ..fixed
        };
        assert!(
            setup_of(&unlimited)["generationConfig"]
                .get("maxOutputTokens")
                .is_none()
        );
    }

    /// `gemini_turn_detection`: manual turns turn Gemini's activity detection off
    /// (`realtimeInputConfig.automaticActivityDetection.disabled`), open a turn with
    /// `activityStart` once `setupComplete` arrives, and a commit is `activityEnd` then the next
    /// `activityStart`; server VAD's silence is `silenceDurationMs` and "do not interrupt" is
    /// `NO_INTERRUPTION`. Off: no `realtimeInputConfig`, a commit sends nothing.
    #[test]
    fn the_turn_detection_fix_maps_the_sessions_turns() {
        let with = |turns: TurnDetectionConfig, fixes: &[&str]| RealtimeConfig {
            turn_detection: Some(turns),
            provider_fixes: fixes.iter().map(|f| (*f).to_owned()).collect(),
            ..base_cfg()
        };
        let manual = with(TurnDetectionConfig::None, &[]);
        assert!(setup_of(&manual).get("realtimeInputConfig").is_none());
        assert!(proto(&manual).commit_turn().is_empty());
        assert!(proto(&manual).caps().emits_user_turn_frames);

        let manual = with(TurnDetectionConfig::None, &["gemini_turn_detection"]);
        let p = proto(&manual);
        assert_eq!(
            setup_of(&manual)["realtimeInputConfig"],
            json!({"automaticActivityDetection": {"disabled": true}})
        );
        assert!(!p.caps().emits_user_turn_frames);
        assert_eq!(
            p.commit_turn(),
            [
                json!({"realtimeInput": {"activityEnd": {}}}),
                json!({"realtimeInput": {"activityStart": {}}})
            ]
        );
        match p
            .map_server_event(Inbound::Text(r#"{"setupComplete":{}}"#))
            .as_slice()
        {
            [
                S2sEvent::SessionReady { .. },
                S2sEvent::SendFrame(OutFrame::Text(frame)),
            ] => {
                assert_eq!(frame, r#"{"realtimeInput":{"activityStart":{}}}"#);
            }
            other => panic!("expected SessionReady then activityStart, got {other:?}"),
        }

        let vad = with(
            TurnDetectionConfig::ServerVad {
                threshold: Some(0.5),
                prefix_padding_ms: Some(300),
                silence_duration_ms: Some(800),
                create_response: Some(true),
                interrupt_response: Some(false),
            },
            &["gemini_turn_detection"],
        );
        assert_eq!(
            setup_of(&vad)["realtimeInputConfig"],
            json!({"automaticActivityDetection": {"silenceDurationMs": 800},
                   "activityHandling": "NO_INTERRUPTION"})
        );
        assert!(proto(&vad).commit_turn().is_empty());
        let unset = RealtimeConfig {
            provider_fixes: vec!["gemini_turn_detection".into()],
            ..base_cfg()
        };
        assert!(setup_of(&unset).get("realtimeInputConfig").is_none());
    }

    /// `gemini_update_resume`: an update is made by resuming on a new connection (the model
    /// cannot change), never a second `setup` on the open one; `gemini_await_setup` holds
    /// every other message until `setupComplete`. The scaffold side is in `scaffold/mock.rs`.
    #[test]
    fn the_update_and_setup_fixes_tell_the_driver() {
        let p = proto(&base_cfg());
        assert!(!p.awaits_session_ready());
        assert!(!p.update_by_reconnect(&base_cfg(), &base_cfg()));
        let fixed = RealtimeConfig {
            provider_fixes: vec!["gemini_update_resume".into(), "gemini_await_setup".into()],
            ..base_cfg()
        };
        let p = proto(&fixed);
        assert!(p.awaits_session_ready());
        let mut new = fixed.clone();
        new.instructions = Some("Answer in French.".into());
        assert!(p.update_by_reconnect(&fixed, &new));
        assert!(p.session_update(&fixed, &new).is_none());
        new.model = "gemini-3.8-live".into();
        assert!(!p.update_by_reconnect(&fixed, &new));
        // The new connection's setup: the stored handle and the new instructions.
        let mut new = fixed.clone();
        new.instructions = Some("Answer in French.".into());
        let setup = p.build_session_config(&new, Some("h-1")).remove(0);
        assert_eq!(
            setup["setup"]["sessionResumption"],
            json!({"handle": "h-1"})
        );
        assert_eq!(
            setup["setup"]["systemInstruction"],
            json!({"parts": [{"text": "Answer in French."}]})
        );
    }

    /// `gemini_json_schema`: a client's JSON Schema is `parametersJsonSchema` and the required
    /// `description` is sent (empty when the client gave none). Off: today's `parameters`.
    #[test]
    fn the_json_schema_fix_declares_parameters_as_json_schema() {
        let schema = json!({"type": "object", "additionalProperties": false,
                            "properties": {"time": {"type": "string"}}});
        let cfg = |fixes: &[&str]| RealtimeConfig {
            tools: Some(vec![ToolDefinition {
                tool_type: "function".into(),
                function: FunctionDefinition {
                    name: "book_table".into(),
                    description: None,
                    parameters: Some(schema.clone()),
                },
            }]),
            provider_fixes: fixes.iter().map(|f| (*f).to_owned()).collect(),
            ..base_cfg()
        };
        let decl =
            |fixes: &[&str]| setup_of(&cfg(fixes))["tools"][0]["functionDeclarations"][0].clone();
        assert_eq!(
            decl(&[]),
            json!({"name": "book_table", "parameters": schema})
        );
        assert_eq!(
            decl(&["gemini_json_schema"]),
            json!({"name": "book_table", "description": "", "parametersJsonSchema": schema})
        );
    }

    /// `gemini_inbound`: a `thought` part is neither audio nor a transcript, and
    /// `voiceActivity` is the caller's speech start and end. Off: today's mapping.
    #[test]
    fn the_inbound_fix_skips_thoughts_and_maps_voice_activity() {
        let thought = r#"{"serverContent":{"modelTurn":{"parts":[{"text":"Plan: ask the time.","thought":true},{"text":"For when?"}]}}}"#;
        let start = r#"{"voiceActivity":{"type":"ACTIVITY_START","audioOffset":"1.5s"}}"#;
        let end = r#"{"voiceActivity":{"type":"ACTIVITY_END"}}"#;
        let texts = |p: &GeminiProtocol| -> Vec<String> {
            p.map_server_event(Inbound::Text(thought))
                .into_iter()
                .filter_map(|e| match e {
                    S2sEvent::Transcript { text, .. } => Some(text),
                    _ => None,
                })
                .collect()
        };
        let today = proto(&base_cfg());
        assert_eq!(texts(&today), ["Plan: ask the time.", "For when?"]);
        assert!(matches!(
            today.map_server_event(Inbound::Text(start)).as_slice(),
            [S2sEvent::Ignore]
        ));
        let p = proto(&RealtimeConfig {
            provider_fixes: vec!["gemini_inbound".into()],
            ..base_cfg()
        });
        assert_eq!(texts(&p), ["For when?"]);
        assert!(matches!(
            p.map_server_event(Inbound::Text(start)).as_slice(),
            [S2sEvent::Speech(SpeechEvent::Started {
                audio_start_ms: 1500,
                ..
            })]
        ));
        assert!(matches!(
            p.map_server_event(Inbound::Text(end)).as_slice(),
            [S2sEvent::Speech(SpeechEvent::Stopped { .. })]
        ));
    }

    /// `AddContext` is a user turn the model does not answer yet (`turnComplete: false`); the
    /// other verbs have no Gemini wire.
    #[test]
    fn add_context_is_client_content_without_turn_complete() {
        let p = proto(&base_cfg());
        assert_eq!(
            p.verb(&RealtimeVerb::AddContext("The booking is confirmed.")),
            Some(vec![json!({"clientContent": {
                "turns": [{"role": "user", "parts": [{"text": "The booking is confirmed."}]}],
                "turnComplete": false}})])
        );
        for verb in [
            RealtimeVerb::SayExact("x"),
            RealtimeVerb::SayGuided("x"),
            RealtimeVerb::StopOutput,
        ] {
            assert!(p.verb(&verb).is_none());
        }
    }
}
