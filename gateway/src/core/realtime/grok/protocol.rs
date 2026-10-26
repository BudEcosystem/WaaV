//! Grok / xAI Realtime [`RealtimeProtocol`] — an OpenAI-PROTOCOL CLONE.
//!
//! xAI's Realtime API is GA-compatible: it speaks the OpenAI GA wire, so this
//! protocol EMBEDS an [`OpenAiProtocol`] and DELEGATES every wire method to it
//! verbatim — overriding ONLY `provider_id`, `connect_spec` (the xAI host), and
//! `map_server_event` (xAI bootstraps the session with a `conversation.created`
//! frame, whereas OpenAI sends `session.created`).

use std::collections::HashMap;
use std::sync::Mutex;

use serde_json::{Map, Value, json};

use super::super::openai::protocol::{OpenAiProtocol, session_patch};
use crate::core::realtime::base::{
    RealtimeConfig, RealtimeResponseOverride, RealtimeResult, RealtimeVerb, ReplayConversationItem,
    TranscriptRole,
};
use crate::core::realtime::openai::ClientEvent;
use crate::core::realtime::scaffold::{
    ConnectSpec, Inbound, OutFrame, ProtocolCaps, RealtimeProtocol, S2sEvent,
};

/// xAI realtime WebSocket base URL. BEST-KNOWN: xAI's realtime surface is behind
/// a key we do not hold, so this endpoint is the documented/best-known form and
/// has NOT been live-probed. The `?model=<model>` query mirrors OpenAI.
pub(crate) const GROK_REALTIME_URL: &str = "wss://api.x.ai/v1/realtime";

/// The model an empty one becomes under the provider fix `grok_voice_default`: xAI's `model`
/// query is one of `grok-voice-latest` (its default), `grok-voice-3.0` and `grok-voice-2.0`
/// (<https://docs.x.ai/voice-realtime.ws.json> `queryParameters`); `grok-realtime` is none.
pub(crate) const GROK_VOICE_DEFAULT: &str = "grok-voice-latest";

/// The xAI server events whose OpenAI-only fields xAI leaves out (its schema lists none of
/// them and its examples omit them, <https://docs.x.ai/voice-realtime.ws.json>), with the
/// fields that take `0` so the shared OpenAI mapping reads the event (fix `grok_events`).
const XAI_ZERO_FIELDS: &[(&str, &[&str])] = &[
    ("input_audio_buffer.speech_started", &["audio_start_ms"]),
    ("input_audio_buffer.speech_stopped", &["audio_end_ms"]),
    (
        "conversation.item.input_audio_transcription.completed",
        &["content_index"],
    ),
    (
        "response.output_audio_transcript.delta",
        &["output_index", "content_index"],
    ),
    (
        "response.output_audio_transcript.done",
        &["output_index", "content_index"],
    ),
    (
        "response.output_text.delta",
        &["output_index", "content_index"],
    ),
];

/// Grok / xAI Realtime protocol. A thin wrapper over [`OpenAiProtocol`]: the GA
/// wire is delegated verbatim; the endpoint + the bootstrap event differ.
pub struct GrokProtocol {
    /// The embedded OpenAI GA protocol — every wire method delegates here.
    inner: OpenAiProtocol,
    /// The model string for the `?model=` query (raw `cfg.model`, or a default).
    model: String,
    /// Provider fix `grok_session`: xAI's own `session.update` and `response.create` fields.
    session: bool,
    /// Provider fix `grok_events`: xAI's server events are read as xAI sends them.
    events: bool,
    /// `grok_events`: each user item's cumulative transcript so far (`.updated`).
    heard: Mutex<HashMap<String, String>>,
    /// `grok_events`: the assistant transcript of the response so far (`.done` may lack it).
    said: Mutex<String>,
    /// `grok_events`: the model xAI's `session.created` / `session.updated` names (live 23 Oct:
    /// `grok-voice-3.0` for `grok-voice-latest`); empty until then.
    named: Mutex<String>,
}

impl GrokProtocol {
    /// Borrow the embedded OpenAI protocol (for the newtype's inherent accessors).
    pub(crate) fn inner(&self) -> &OpenAiProtocol {
        &self.inner
    }

    /// The `session` of xAI's `session.update` (fix `grok_session`): `voice` and
    /// `turn_detection` at the top (`{"type":"server_vad"}`, or `{"type":null}` for manual
    /// turns), the audio formats, the transcription's `language_hint` (and a model the client
    /// named; `whisper-1` is OpenAI's), the instructions and tools. OpenAI's other fields are
    /// not in xAI's schema and are left out.
    fn xai_session(&self, cfg: &RealtimeConfig) -> Value {
        use crate::core::realtime::base::TurnDetectionConfig;
        let openai = serde_json::to_value(self.inner.session_config(cfg)).unwrap_or_default();
        let mut session = Map::new();
        if let Some(instructions) = &cfg.instructions {
            session.insert("instructions".into(), json!(instructions));
        }
        // "Voice identifier. Use a built-in voice (e.g. Eve, Ara) or a custom voice ID."
        if let Some(voice) = cfg
            .voice
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            session.insert("voice".into(), json!(voice));
        }
        // xAI: "`server_vad` for automatic detection, `null` for manual text turns".
        match &cfg.turn_detection {
            Some(TurnDetectionConfig::None) => {
                session.insert("turn_detection".into(), json!({ "type": null }));
            }
            Some(_) => {
                session.insert("turn_detection".into(), json!({ "type": "server_vad" }));
            }
            None => {}
        }
        let mut input = json!({ "format": openai["audio"]["input"]["format"] });
        if let Some(asked) = &cfg.input_audio_transcription {
            let mut transcription = Map::new();
            let model = asked.model.trim();
            if !model.is_empty() && model != "whisper-1" {
                transcription.insert("model".into(), json!(model));
            }
            if let Some(language) = cfg
                .language
                .as_deref()
                .map(str::trim)
                .filter(|l| !l.is_empty())
            {
                transcription.insert("language_hint".into(), json!(language));
            }
            if !transcription.is_empty() {
                input["transcription"] = Value::Object(transcription);
            }
        }
        session.insert(
            "audio".into(),
            json!({ "input": input, "output": { "format": openai["audio"]["output"]["format"] } }),
        );
        if let Some(tools) = openai.get("tools") {
            session.insert("tools".into(), tools.clone());
        }
        Value::Object(session)
    }

    /// A cumulative caller transcript of item `item` (`.updated`, or a `.completed` still in
    /// progress) as its new words, interim; a revision of earlier words waits for the final.
    fn words_so_far(&self, item: String, event: &Map<String, Value>) -> Vec<S2sEvent> {
        let heard = event
            .get("transcript")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let mut items = self.heard.lock().unwrap_or_else(|p| p.into_inner());
        let before = items
            .insert(item.clone(), heard.to_owned())
            .unwrap_or_default();
        let new = heard.strip_prefix(before.as_str()).unwrap_or_default();
        if new.is_empty() {
            vec![S2sEvent::Ignore]
        } else {
            vec![S2sEvent::Transcript {
                role: TranscriptRole::User,
                text: new.to_owned(),
                is_final: false,
                item_id: Some(item),
            }]
        }
    }

    /// One xAI server event in the shape the shared OpenAI mapping reads (fix `grok_events`):
    /// the OpenAI-only fields xAI leaves out, `response.text.delta` (xAI: "Clients should
    /// handle both event names"), the cumulative `.updated` user transcript (and a `.completed`
    /// still `in_progress`) as its new words, a `.done` without its transcript as the deltas it
    /// closes, and a response whose `usage` is xAI's empty one.
    fn xai_event(&self, text: &str) -> Vec<S2sEvent> {
        let Ok(Value::Object(mut event)) = serde_json::from_str::<Value>(text) else {
            return vec![S2sEvent::Ignore];
        };
        let item = event
            .get("item_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let kind = event
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let in_progress = event.get("status").and_then(Value::as_str) == Some("in_progress");
        match kind.as_str() {
            // xAI also sends `.completed` with `status: in_progress` while the caller speaks
            // (live, 23 Oct; its schema has no `status`): the words so far, as `.updated`.
            "conversation.item.input_audio_transcription.updated" => {
                return self.words_so_far(item, &event);
            }
            "conversation.item.input_audio_transcription.completed" if in_progress => {
                return self.words_so_far(item, &event);
            }
            "conversation.item.input_audio_transcription.completed" => {
                let mut items = self.heard.lock().unwrap_or_else(|p| p.into_inner());
                items.remove(&item);
            }
            // xAI's `response.usage` is `{}` (its usage rides at the top of `response.done`,
            // live 23 Oct), which OpenAI's usage cannot read: the event would be dropped.
            "response.created" | "response.done" => {
                // A cancelled response gets no transcript `.done` (live): its words must not
                // open the next response's.
                if kind == "response.created" {
                    self.said.lock().unwrap_or_else(|p| p.into_inner()).clear();
                }
                if let Some(response) = event.get_mut("response").and_then(Value::as_object_mut)
                    && response
                        .get("usage")
                        .is_some_and(|usage| usage.get("total_tokens").is_none())
                {
                    response.remove("usage");
                }
            }
            "response.text.delta" => {
                event.insert("type".into(), json!("response.output_text.delta"));
            }
            "session.created" | "session.updated" => {
                if let Some(model) = event
                    .get("session")
                    .and_then(|session| session.get("model"))
                    .and_then(Value::as_str)
                    .filter(|model| !model.trim().is_empty())
                {
                    *self.named.lock().unwrap_or_else(|p| p.into_inner()) = model.trim().to_owned();
                }
            }
            "response.output_audio_transcript.delta" => {
                let delta = event
                    .get("delta")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                self.said
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .push_str(delta);
            }
            "response.output_audio_transcript.done" => {
                let said =
                    std::mem::take(&mut *self.said.lock().unwrap_or_else(|p| p.into_inner()));
                event.entry("transcript").or_insert(json!(said));
            }
            _ => {}
        }
        let kind = event
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if let Some((_, fields)) = XAI_ZERO_FIELDS.iter().find(|(k, _)| *k == kind) {
            for field in *fields {
                event.entry(*field).or_insert(json!(0));
            }
        }
        self.inner
            .map_server_event(Inbound::Text(&Value::Object(event).to_string()))
    }
}

impl RealtimeProtocol for GrokProtocol {
    type Wire = ClientEvent;

    fn from_config(cfg: &RealtimeConfig) -> RealtimeResult<Self> {
        // Reuse OpenAI's validation (api-key non-empty) + parsing verbatim.
        let inner = OpenAiProtocol::from_config(cfg)?;
        // xAI routes by model string in the query; keep the raw config string so
        // an xAI-specific model name isn't coerced to an OpenAI enum value.
        let model = if !cfg.model.trim().is_empty() {
            cfg.model.trim().to_string()
        } else if cfg.fix("grok_voice_default") {
            GROK_VOICE_DEFAULT.to_string()
        } else {
            "grok-realtime".to_string()
        };
        Ok(Self {
            inner,
            model,
            session: cfg.fix("grok_session"),
            events: cfg.fix("grok_events"),
            heard: Mutex::default(),
            said: Mutex::default(),
            named: Mutex::default(),
        })
    }

    fn provider_id(&self) -> &'static str {
        "grok"
    }

    fn named_model(&self) -> Option<String> {
        self.events
            .then(|| self.named.lock().unwrap_or_else(|p| p.into_inner()).clone())
    }

    fn caps(&self) -> ProtocolCaps {
        // xAI is GA-compatible ⇒ OpenAI GA capabilities.
        self.inner.caps()
    }

    fn connect_spec(&self, cfg: &RealtimeConfig) -> RealtimeResult<ConnectSpec> {
        // Endpoint confirmed against xAI Voice Agent docs; Bearer (server-side)
        // auth. NO `Sec-WebSocket-Protocol` header: xAI uses the WS subprotocol
        // field ONLY to carry the ephemeral client secret on the browser-token
        // path (`xai-client-secret.<token>`); sending an unrecognized `realtime`
        // subprotocol with Bearer auth can get the upgrade rejected by a strict
        // server. (Review wf_0f21536d #3.) Not yet live-probed (no xAI key).
        // SERVER-CONFIG `realtime_endpoint_override` (proxy / self-hosted / mock)
        // WINS over the xAI host when set — Bearer auth rides along unchanged.
        // SSRF-safe: never client-settable (see `apply_endpoint_override`).
        let default_url = format!("{GROK_REALTIME_URL}?model={}", self.model);
        let url = crate::core::realtime::scaffold::apply_endpoint_override(
            &default_url,
            cfg.realtime_endpoint_override.as_deref(),
            None,
        );
        Ok(ConnectSpec::WebSocket {
            url,
            headers: vec![(
                "Authorization".to_string(),
                format!("Bearer {}", cfg.api_key),
            )],
        })
    }

    fn map_server_event(&self, raw: Inbound<'_>) -> Vec<S2sEvent> {
        // xAI bootstraps with `conversation.created` (OpenAI sends
        // `session.created`). The OpenAI `ServerEvent` enum has NO
        // `conversation.created` variant, so intercept the raw text BEFORE
        // delegating: a `conversation.created` frame ⇒ SessionReady; everything
        // else flows through the embedded OpenAI mapping unchanged.
        if let Inbound::Text(text) = raw
            && let Ok(Value::Object(map)) = serde_json::from_str::<Value>(text)
            && map.get("type").and_then(Value::as_str) == Some("conversation.created")
        {
            // Pull a best-effort session/conversation id from the common shapes:
            // top-level `session_id`, or a nested `conversation.id` / `session.id`.
            let session_id = map
                .get("session_id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| {
                    map.get("conversation")
                        .and_then(Value::as_object)
                        .and_then(|c| c.get("id"))
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .or_else(|| {
                    map.get("session")
                        .and_then(Value::as_object)
                        .and_then(|s| s.get("id"))
                        .and_then(Value::as_str)
                        .map(str::to_string)
                });
            return vec![S2sEvent::SessionReady { session_id }];
        }
        match raw {
            Inbound::Text(text) if self.events => self.xai_event(text),
            _ => self.inner.map_server_event(raw),
        }
    }

    // ── every remaining wire method delegates to the embedded OpenAI GA protocol ──

    fn build_session_config(
        &self,
        cfg: &RealtimeConfig,
        resumption: Option<&str>,
    ) -> Vec<Self::Wire> {
        if self.session {
            return vec![ClientEvent::Raw(
                json!({ "type": "session.update", "session": self.xai_session(cfg) }),
            )];
        }
        self.inner.build_session_config(cfg, resumption)
    }

    fn session_update(
        &self,
        old: &RealtimeConfig,
        new: &RealtimeConfig,
    ) -> Option<Vec<Self::Wire>> {
        if !self.session {
            return self.inner.session_update(old, new);
        }
        if !self.inner.fixes.update_delta {
            return Some(self.build_session_config(new, None));
        }
        // xAI documents no restriction on the voice; a new model needs a new connection.
        if old.model.trim() != new.model.trim() {
            return None;
        }
        Some(session_patch(
            &self.xai_session(old),
            &self.xai_session(new),
        ))
    }

    fn encode_user_audio(&self, pcm: &[u8]) -> Self::Wire {
        self.inner.encode_user_audio(pcm)
    }

    fn send_text(&self, text: &str) -> Vec<Self::Wire> {
        self.inner.send_text(text)
    }

    fn create_response(&self, overrides: Option<&RealtimeResponseOverride>) -> Vec<Self::Wire> {
        if !self.session {
            return self.inner.create_response(overrides);
        }
        // xAI's `response.create` takes `modalities`, `instructions` and `metadata`.
        let mut response = Map::new();
        if let Some(overrides) = overrides {
            if let Some(modalities) = &overrides.modalities {
                response.insert("modalities".into(), json!(modalities));
            }
            if let Some(instructions) = &overrides.instructions {
                response.insert("instructions".into(), json!(instructions));
            }
            if let Some(metadata) = &overrides.metadata {
                response.insert("metadata".into(), metadata.clone());
            }
        }
        vec![ClientEvent::Raw(
            json!({ "type": "response.create", "response": Value::Object(response) }),
        )]
    }

    fn commit_turn(&self) -> Vec<Self::Wire> {
        self.inner.commit_turn()
    }

    fn cancel_response(&self) -> Vec<Self::Wire> {
        self.inner.cancel_response()
    }

    fn truncate(&self, item_id: &str, audio_end_ms: u64) -> Vec<Self::Wire> {
        self.inner.truncate(item_id, audio_end_ms)
    }

    fn clear_input_buffer(&self) -> Vec<Self::Wire> {
        self.inner.clear_input_buffer()
    }

    fn format_tool_result(&self, call_id: &str, result: &str) -> Vec<Self::Wire> {
        self.inner.format_tool_result(call_id, result)
    }

    fn replay_item(&self, item: &ReplayConversationItem) -> Vec<Self::Wire> {
        // xAI's message content types are `input_text`, `input_audio`, `text` and `audio`:
        // an assistant line stays `text` whatever OpenAI's fix `openai_replay_output_text`.
        let mut wires = self.inner.replay_item(item);
        for wire in &mut wires {
            if let ClientEvent::ConversationItemCreate { item, .. } = wire {
                for part in item.content.iter_mut().flatten() {
                    if part.content_type == "output_text" {
                        part.content_type = "text".to_owned();
                    }
                }
            }
        }
        wires
    }

    /// xAI's documented verbs (<https://docs.x.ai/voice-realtime.ws.json>): `force_message`
    /// ("a hard-coded, TTS-synthesized line (not model-generated) ... Do not send
    /// `response.create` after this") for exact words; under `grok_session` a guided line is
    /// xAI's `response.create` with `instructions` ("Per-response system prompt override"; xAI's
    /// takes no `tool_choice`); context and a stop as OpenAI's.
    fn verb(&self, verb: &RealtimeVerb<'_>) -> Option<Vec<Self::Wire>> {
        match *verb {
            RealtimeVerb::SayExact(text) => Some(vec![ClientEvent::Raw(json!({
                "type": "conversation.item.create",
                "item": {
                    "type": "force_message",
                    "role": "assistant",
                    "content": [{ "type": "output_text", "text": text }],
                },
            }))]),
            RealtimeVerb::SayGuided(guidance) if self.session => {
                Some(self.create_response(Some(&RealtimeResponseOverride {
                    instructions: Some(guidance.to_owned()),
                    ..Default::default()
                })))
            }
            _ => self.inner.verb(verb),
        }
    }

    fn serialize(&self, msg: &Self::Wire) -> RealtimeResult<OutFrame> {
        self.inner.serialize(msg)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::realtime::base::RealtimeError;

    fn base_cfg() -> RealtimeConfig {
        RealtimeConfig {
            provider: "grok".into(),
            api_key: "xaikey".into(),
            model: "grok-realtime".into(),
            voice: Some("alloy".into()),
            ..Default::default()
        }
    }

    fn proto(cfg: &RealtimeConfig) -> GrokProtocol {
        GrokProtocol::from_config(cfg).unwrap()
    }

    /// from_config validates the api key (delegated to OpenAI) — empty ⇒ Err.
    #[test]
    fn from_config_requires_api_key() {
        let cfg = RealtimeConfig {
            api_key: String::new(),
            ..Default::default()
        };
        assert!(matches!(
            GrokProtocol::from_config(&cfg),
            Err(RealtimeError::AuthenticationFailed(_))
        ));
    }

    /// connect_spec: the xAI URL + Bearer auth (NOT api-key header).
    #[test]
    fn connect_spec_uses_xai_url_and_bearer() {
        let p = proto(&base_cfg());
        let ConnectSpec::WebSocket { url, headers } = p.connect_spec(&base_cfg()).unwrap() else {
            panic!("expected ConnectSpec::WebSocket")
        };
        assert_eq!(url, "wss://api.x.ai/v1/realtime?model=grok-realtime");
        let auth = headers
            .iter()
            .find(|(k, _)| k == "Authorization")
            .map(|(_, v)| v.as_str())
            .unwrap();
        assert_eq!(auth, "Bearer xaikey");
        assert!(
            !headers
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("api-key")),
            "xAI uses Bearer, not the Azure api-key header"
        );
    }

    /// SERVER-CONFIG `realtime_endpoint_override` WINS over the xAI host VERBATIM;
    /// Bearer auth rides along. Server-config-only (never client-settable).
    #[test]
    fn connect_spec_honors_server_endpoint_override() {
        let cfg = RealtimeConfig {
            realtime_endpoint_override: Some("ws://127.0.0.1:9003/grok".into()),
            ..base_cfg()
        };
        let ConnectSpec::WebSocket { url, headers } = proto(&cfg).connect_spec(&cfg).unwrap()
        else {
            panic!("expected WebSocket")
        };
        assert_eq!(url, "ws://127.0.0.1:9003/grok");
        assert!(
            headers
                .iter()
                .any(|(k, v)| k == "Authorization" && v == "Bearer xaikey")
        );
    }

    /// map_server_event override: xAI `conversation.created` ⇒ [SessionReady],
    /// pulling the id from a nested `conversation.id`.
    #[test]
    fn conversation_created_maps_to_session_ready() {
        let p = proto(&base_cfg());
        let raw = r#"{"type":"conversation.created","conversation":{"id":"conv_abc"}}"#;
        match p.map_server_event(Inbound::Text(raw)).as_slice() {
            [S2sEvent::SessionReady { session_id }] => {
                assert_eq!(session_id.as_deref(), Some("conv_abc"));
            }
            other => panic!("expected SessionReady, got {other:?}"),
        }
        // No id present ⇒ still SessionReady, id None.
        let raw2 = r#"{"type":"conversation.created"}"#;
        match p.map_server_event(Inbound::Text(raw2)).as_slice() {
            [S2sEvent::SessionReady { session_id }] => assert!(session_id.is_none()),
            other => panic!("expected SessionReady, got {other:?}"),
        }
    }

    /// A normal OpenAI-shaped event still maps via delegation (e.g. an audio
    /// transcript delta ⇒ a partial assistant Transcript).
    #[test]
    fn normal_openai_event_still_delegates() {
        let p = proto(&base_cfg());
        // session.created (OpenAI bootstrap) flows through the embedded mapping.
        let created = r#"{"type":"session.created","session":{"id":"sess_1","object":"realtime.session","model":"grok","expires_at":0,"modalities":[]}}"#;
        match p.map_server_event(Inbound::Text(created)).as_slice() {
            [S2sEvent::SessionReady { session_id }] => {
                assert_eq!(session_id.as_deref(), Some("sess_1"));
            }
            other => panic!("expected delegated SessionReady, got {other:?}"),
        }
        let delta = r#"{"type":"response.output_audio_transcript.delta","response_id":"r","item_id":"i","output_index":0,"content_index":0,"delta":"hi"}"#;
        match p.map_server_event(Inbound::Text(delta)).as_slice() {
            [S2sEvent::Transcript { text, is_final, .. }] => {
                assert_eq!(text, "hi");
                assert!(!is_final);
            }
            other => panic!("expected delegated Transcript, got {other:?}"),
        }
    }

    /// Delegation smoke test: session.update + audio append are the GA wire.
    #[test]
    fn delegates_ga_wire_verbatim() {
        let p = proto(&base_cfg());
        let session_wires = p.build_session_config(&base_cfg(), None);
        let session_json = match p.serialize(&session_wires[0]).unwrap() {
            OutFrame::Text(s) => s,
            OutFrame::Binary(_) => panic!("text frame expected"),
        };
        assert!(session_json.contains("session.update"));
        let append = p.encode_user_audio(&[9u8, 8, 7]);
        let append_json = match p.serialize(&append).unwrap() {
            OutFrame::Text(s) => s,
            OutFrame::Binary(_) => panic!("text frame expected"),
        };
        assert!(append_json.contains("input_audio_buffer.append"));
    }

    fn fixed(fixes: &[&str]) -> RealtimeConfig {
        RealtimeConfig {
            provider_fixes: fixes.iter().map(|f| (*f).to_owned()).collect(),
            ..base_cfg()
        }
    }

    fn as_json(p: &GrokProtocol, wires: &[ClientEvent]) -> Vec<Value> {
        wires
            .iter()
            .map(|wire| match p.serialize(wire).unwrap() {
                OutFrame::Text(s) => serde_json::from_str(&s).unwrap(),
                OutFrame::Binary(_) => panic!("text frame expected"),
            })
            .collect()
    }

    /// Fix `grok_session` (escape flag `grok_openai_session`): xAI's `session.update` as its
    /// own example has it (`voice` and `turn_detection` under `session`, the transcription's
    /// `language_hint`), and `{"type": null}` for manual turns; without it today's OpenAI frame,
    /// whose voice is an OpenAI one.
    #[test]
    fn the_session_fix_sends_xais_session_update() {
        use crate::core::realtime::base::{InputTranscriptionConfig, TurnDetectionConfig};
        let cfg = |fixes: &[&str], turns: TurnDetectionConfig| RealtimeConfig {
            voice: Some("eve".into()),
            instructions: Some("You are a helpful assistant.".into()),
            language: Some("en".into()),
            input_audio_transcription: Some(InputTranscriptionConfig {
                model: "whisper-1".into(),
            }),
            turn_detection: Some(turns),
            ..fixed(fixes)
        };
        let today = cfg(&[], TurnDetectionConfig::default());
        let p = proto(&today);
        let sent = as_json(&p, &p.build_session_config(&today, None)).remove(0);
        assert_eq!(sent["session"]["audio"]["output"]["voice"], "alloy");
        assert!(sent["session"].get("voice").is_none());

        let on = cfg(&["grok_session"], TurnDetectionConfig::default());
        let p = proto(&on);
        assert_eq!(
            as_json(&p, &p.build_session_config(&on, None)),
            [json!({"type": "session.update", "session": {
                "voice": "eve",
                "instructions": "You are a helpful assistant.",
                "turn_detection": {"type": "server_vad"},
                "audio": {
                    "input": {"format": {"type": "audio/pcm", "rate": 24000},
                              "transcription": {"language_hint": "en"}},
                    "output": {"format": {"type": "audio/pcm", "rate": 24000}}
                }
            }})]
        );
        let manual = cfg(&["grok_session"], TurnDetectionConfig::None);
        let p = proto(&manual);
        let sent = as_json(&p, &p.build_session_config(&manual, None)).remove(0);
        assert_eq!(sent["session"]["turn_detection"], json!({"type": null}));
        // A response names xAI's `modalities`; OpenAI's voice and conversation are not sent.
        let ov = RealtimeResponseOverride {
            modalities: Some(vec!["text".into()]),
            instructions: Some("Say hello.".into()),
            voice: Some("ara".into()),
            out_of_band: true,
            ..Default::default()
        };
        assert_eq!(
            as_json(&p, &p.create_response(Some(&ov))),
            [json!({"type": "response.create",
                    "response": {"modalities": ["text"], "instructions": "Say hello."}})]
        );
        // With the update fix too, an update names only what changed.
        let both = cfg(
            &["grok_session", "openai_update_delta"],
            TurnDetectionConfig::None,
        );
        let mut new = both.clone();
        new.voice = Some("ara".into());
        let p = proto(&both);
        assert_eq!(
            as_json(&p, &p.session_update(&both, &new).unwrap()),
            [json!({"type": "session.update", "session": {"voice": "ara"}})]
        );
        // The escape flag's default: the session fix off, today's OpenAI update.
        let p = proto(&today);
        assert_eq!(
            as_json(&p, &p.session_update(&today, &today).unwrap()).len(),
            1
        );
    }

    /// Fix `grok_events`: xAI's own example frames (no `audio_start_ms`, `content_index`,
    /// `output_index`, a `.done` without its transcript, `response.text.delta`, the cumulative
    /// `.updated`) reach the client; without it the shared OpenAI parse drops them.
    #[test]
    fn the_events_fix_reads_xais_example_frames() {
        let frames = [
            r#"{"event_id":"event_1516","type":"input_audio_buffer.speech_started","item_id":"msg_003"}"#,
            r#"{"event_id":"event_2123","type":"conversation.item.input_audio_transcription.updated","item_id":"msg_003","transcript":"Hello, how are"}"#,
            r#"{"event_id":"event_2124","type":"conversation.item.input_audio_transcription.updated","item_id":"msg_003","transcript":"Hello, how are you"}"#,
            r#"{"event_id":"event_2122","type":"conversation.item.input_audio_transcription.completed","item_id":"msg_003","transcript":"Hello, how are you?"}"#,
            r#"{"event_id":"event_4950","type":"response.output_audio_transcript.delta","response_id":"resp_001","item_id":"msg_008","delta":"Hello! I'm doing"}"#,
            r#"{"event_id":"event_5152","type":"response.output_audio_transcript.done","response_id":"resp_001","item_id":"msg_008"}"#,
            r#"{"type":"response.text.delta","event_id":"event_4950","response_id":"resp_001","item_id":"msg_008","output_index":0,"content_index":0,"delta":"Text response..."}"#,
        ];
        let today = proto(&base_cfg());
        for frame in frames {
            assert!(
                matches!(
                    today.map_server_event(Inbound::Text(frame)).as_slice(),
                    [S2sEvent::Ignore]
                ),
                "{frame}"
            );
        }
        let p = proto(&fixed(&["grok_events"]));
        let got: Vec<String> = frames
            .iter()
            .flat_map(|frame| p.map_server_event(Inbound::Text(frame)))
            .map(|event| match event {
                S2sEvent::Speech(crate::core::realtime::base::SpeechEvent::Started { .. }) => {
                    "speech started".to_owned()
                }
                S2sEvent::Transcript {
                    role,
                    text,
                    is_final,
                    ..
                } => {
                    format!(
                        "{role:?} {} {text}",
                        if is_final { "final" } else { "interim" }
                    )
                }
                other => format!("{other:?}"),
            })
            .collect();
        assert_eq!(
            got,
            [
                "speech started",
                "User interim Hello, how are",
                "User interim  you",
                "User final Hello, how are you?",
                "Assistant interim Hello! I'm doing",
                "Assistant final Hello! I'm doing",
                "Assistant interim Text response...",
            ]
        );
    }

    /// Fix `grok_events` with xAI's live frames (probe of 23 Oct): `.completed` also comes
    /// `in_progress` (the words so far), and `response.done` carries `usage: {}`; without the
    /// fix each in-progress `.completed` is a final (the caller's line logged three times) and
    /// no `response_done` reaches the client.
    #[test]
    fn the_events_fix_reads_xais_live_frames() {
        let frames = [
            r#"{"type":"conversation.item.input_audio_transcription.updated","event_id":"e1","item_id":"u1","transcript":"What is the capital of","content_index":0,"previous_item_id":null}"#,
            r#"{"type":"conversation.item.input_audio_transcription.completed","event_id":"e2","item_id":"u1","transcript":"What is the capital of","content_index":0,"status":"in_progress","previous_item_id":null}"#,
            r#"{"type":"conversation.item.input_audio_transcription.completed","event_id":"e3","item_id":"u1","transcript":"What is the capital of France?","content_index":0,"status":"in_progress","previous_item_id":null}"#,
            r#"{"type":"conversation.item.input_audio_transcription.completed","event_id":"e4","item_id":"u1","transcript":"What is the capital of France?","content_index":0,"status":"completed","previous_item_id":null}"#,
            r#"{"type":"response.done","event_id":"e5","response":{"id":"r1","object":"realtime.response","output":[{"id":"a1","object":"realtime.item","type":"message","status":"completed","role":"assistant","content":[{"type":"audio","transcript":"Paris."}],"replayed":false}],"status":"completed","status_details":"unimplemented","usage":{}},"response_id":"r1","usage":{"input_tokens":94,"output_tokens":20,"total_tokens":114},"previous_item_id":null}"#,
        ];
        let read = |p: &GrokProtocol| -> Vec<String> {
            frames
                .iter()
                .flat_map(|frame| p.map_server_event(Inbound::Text(frame)))
                .filter_map(|event| match event {
                    S2sEvent::Transcript { text, is_final, .. } => Some(format!(
                        "{} {text}",
                        if is_final { "final" } else { "interim" }
                    )),
                    S2sEvent::ResponseDone { response_id } => Some(format!("done {response_id}")),
                    _ => None,
                })
                .collect()
        };
        assert_eq!(
            read(&proto(&base_cfg())),
            [
                "final What is the capital of",
                "final What is the capital of France?",
                "final What is the capital of France?",
            ]
        );
        assert_eq!(
            read(&proto(&fixed(&["grok_events"]))),
            [
                "interim What is the capital of",
                "interim  France?",
                "final What is the capital of France?",
                "done r1",
            ]
        );
    }

    /// Fix `grok_events`: a cancelled response's words (xAI sends it no transcript `.done`,
    /// live 23 Oct) do not open the next response's `.done`.
    #[test]
    fn a_cancelled_response_leaves_no_words_behind() {
        let p = proto(&fixed(&["grok_events"]));
        let frames = [
            r#"{"type":"response.created","response":{"id":"r1","usage":{}}}"#,
            r#"{"type":"response.output_audio_transcript.delta","response_id":"r1","item_id":"a1","delta":"Once upon"}"#,
            r#"{"type":"response.created","response":{"id":"r2","usage":{}}}"#,
            r#"{"type":"response.output_audio_transcript.delta","response_id":"r2","item_id":"a2","delta":"Four."}"#,
            r#"{"type":"response.output_audio_transcript.done","response_id":"r2","item_id":"a2"}"#,
        ];
        let finals: Vec<String> = frames
            .iter()
            .flat_map(|frame| p.map_server_event(Inbound::Text(frame)))
            .filter_map(|event| match event {
                S2sEvent::Transcript {
                    text,
                    is_final: true,
                    ..
                } => Some(text),
                _ => None,
            })
            .collect();
        assert_eq!(finals, ["Four."]);
    }

    /// Fix `grok_voice_default`: a session naming no model asks for `grok-voice-latest`
    /// (xAI's `model` query is `grok-voice-latest`, `grok-voice-3.0` or `grok-voice-2.0`);
    /// without it today's `grok-realtime`. A named model goes as named.
    #[test]
    fn the_default_fix_moves_only_an_empty_model() {
        let url = |model: &str, fixes: &[&str]| {
            let cfg = RealtimeConfig {
                model: model.into(),
                ..fixed(fixes)
            };
            match proto(&cfg).connect_spec(&cfg).unwrap() {
                ConnectSpec::WebSocket { url, .. } => url,
                _ => panic!("expected a WebSocket"),
            }
        };
        assert_eq!(
            url("", &[]),
            "wss://api.x.ai/v1/realtime?model=grok-realtime"
        );
        assert_eq!(
            url("", &["grok_voice_default"]),
            "wss://api.x.ai/v1/realtime?model=grok-voice-latest"
        );
        assert_eq!(
            url("grok-voice-3.0", &["grok_voice_default"]),
            "wss://api.x.ai/v1/realtime?model=grok-voice-3.0"
        );
    }

    /// OpenAI's `openai_replay_output_text` leaves a Grok replay `text` (xAI's content types
    /// are `input_text`, `input_audio`, `text` and `audio`).
    #[test]
    fn a_grok_replay_stays_text() {
        let p = proto(&fixed(&["openai_replay_output_text"]));
        let line = ReplayConversationItem {
            role: TranscriptRole::Assistant,
            text: "Hi.".into(),
        };
        assert_eq!(
            as_json(&p, &p.replay_item(&line))[0]["item"]["content"][0]["type"],
            "text"
        );
    }

    /// r11-fix: xAI's verbs. Exact words are a `force_message` item and nothing else (xAI:
    /// "Do not send `response.create` after this"); a guided line under `grok_session` is xAI's
    /// `response.create` with `instructions` (no OpenAI fields); context is a system item; a
    /// stop is `response.cancel`.
    #[test]
    fn grok_verbs_are_xais_wires() {
        let p = proto(&fixed(&["grok_session"]));
        let said = as_json(
            &p,
            &p.verb(&RealtimeVerb::SayExact("Your table is booked."))
                .unwrap(),
        );
        assert_eq!(
            said,
            vec![
                json!({"type": "conversation.item.create", "item": {"type": "force_message",
                "role": "assistant", "content": [{"type": "output_text", "text": "Your table is booked."}]}})
            ]
        );
        let guided = as_json(&p, &p.verb(&RealtimeVerb::SayGuided("Say hello.")).unwrap());
        assert_eq!(
            guided,
            vec![json!({"type": "response.create", "response": {"instructions": "Say hello."}})]
        );
        let context = as_json(
            &p,
            &p.verb(&RealtimeVerb::AddContext("The caller is Ann."))
                .unwrap(),
        );
        assert_eq!(context.len(), 1);
        assert_eq!(context[0]["type"], "conversation.item.create");
        assert_eq!(context[0]["item"]["role"], "system");
        let stop = as_json(&p, &p.verb(&RealtimeVerb::StopOutput).unwrap());
        assert_eq!(stop, vec![json!({"type": "response.cancel"})]);
        // Without `grok_session` a guided line is OpenAI's (`tool_choice: none`).
        let plain = proto(&base_cfg());
        let guided = as_json(
            &plain,
            &plain.verb(&RealtimeVerb::SayGuided("Say hello.")).unwrap(),
        );
        assert_eq!(guided[0]["response"]["tool_choice"], "none");
    }

    /// r11-fix: under `grok_events` the model xAI names in `session.created` / `.updated` is
    /// the session's model (live 23 Oct: `grok-voice-3.0` for `grok-voice-latest`); pending
    /// until then; without the fix none is read.
    #[test]
    fn the_model_xai_names_is_kept() {
        let p = proto(&fixed(&["grok_events"]));
        assert_eq!(p.named_model().as_deref(), Some(""));
        let created = r#"{"type":"session.created","session":{"id":"s","model":"grok-voice-3.0","voice":"xai_ara"}}"#;
        let _ = p.map_server_event(Inbound::Text(created));
        assert_eq!(p.named_model().as_deref(), Some("grok-voice-3.0"));
        let updated = r#"{"type":"session.updated","session":{"model":"grok-voice-2.0"}}"#;
        let _ = p.map_server_event(Inbound::Text(updated));
        assert_eq!(p.named_model().as_deref(), Some("grok-voice-2.0"));
        assert_eq!(proto(&base_cfg()).named_model(), None);
    }
}
