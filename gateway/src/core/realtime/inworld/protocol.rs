//! Inworld Realtime [`RealtimeProtocol`] — an OpenAI-PROTOCOL CLONE.
//!
//! Inworld's Realtime API speaks the OpenAI GA wire (it bootstraps with
//! `session.created`, exactly like OpenAI), so this protocol EMBEDS an
//! [`OpenAiProtocol`] and DELEGATES every wire method to it verbatim —
//! overriding ONLY `provider_id` + `connect_spec` (the Inworld host + auth).

use base64::prelude::{BASE64_STANDARD, Engine as _};

use super::super::openai::protocol::OpenAiProtocol;
use crate::core::realtime::base::{
    RealtimeConfig, RealtimeError, RealtimeResponseOverride, RealtimeResult, RealtimeVerb,
    ReplayConversationItem,
};
use crate::core::realtime::openai::ClientEvent;
use crate::core::realtime::scaffold::{
    ConnectSpec, Inbound, OutFrame, ProtocolCaps, RealtimeProtocol, S2sEvent,
};

/// Inworld realtime session-WebSocket base URL (NO query). Inworld's documented
/// realtime endpoint is `…/api/v1/realtime/session?key=<session-id>&protocol=realtime`
/// (docs.inworld.ai/docs/realtime/connect/websocket) with server-side HTTP Basic
/// auth — NOT the OpenAI-style `/v1/realtime?model=…` + Bearer. (Review wf_0f21536d #1.)
pub(crate) const INWORLD_REALTIME_URL: &str = "wss://api.inworld.ai/api/v1/realtime/session";

/// Inworld Realtime protocol. A thin wrapper over [`OpenAiProtocol`]: the GA wire
/// is delegated verbatim; only the connect target + auth differ. NOTE: Inworld's
/// connect needs a backend-MINTED session id (passed as `?key=`), which WaaV does
/// not yet mint automatically (that is a REST session-create handshake — a Phase-5
/// `RestHandshake` transport follow-up). Until then the session id is supplied
/// out-of-band via `RealtimeConfig.endpoint`; `connect_spec` errors without it
/// rather than dialing a provably-dead URL.
pub struct InworldProtocol {
    /// The embedded OpenAI GA protocol — every wire method delegates here.
    inner: OpenAiProtocol,
    /// Provider fix `inworld_session_key`: the session id this session dials with when no
    /// `endpoint` names one ("key: Session ID from your app").
    session_key: Option<String>,
    /// Provider fix `inworld_transcript_index`: a user transcript without `content_index`
    /// (optional in Inworld's reference) is read.
    transcript_index: bool,
    /// Provider fix `inworld_response_shape`: `response.create` in Inworld's shape.
    response_shape: bool,
}

/// The transcription model Inworld documents: "`inworld/inworld-stt-1` is the default for
/// realtime voice agents" (<https://docs.inworld.ai/realtime/usage/using-realtime-models.md>).
const INWORLD_STT_MODEL: &str = "inworld/inworld-stt-1";

impl InworldProtocol {
    /// Borrow the embedded OpenAI protocol (for the newtype's inherent accessors).
    pub(crate) fn inner(&self) -> &OpenAiProtocol {
        &self.inner
    }

    /// Fix `inworld_response_shape`: Inworld's `response.create` `response` has `voice`
    /// ("Override voice for this response") and no `audio`, and `conversation` is "the literal
    /// `"auto"` ... or an explicit conversation ID string" (no `none`: an out-of-band response
    /// goes in the default conversation).
    fn shaped(&self, mut wires: Vec<ClientEvent>) -> Vec<ClientEvent> {
        if !self.response_shape {
            return wires;
        }
        for wire in &mut wires {
            if let ClientEvent::ResponseCreate { response: Some(_) } = wire
                && let Ok(mut event) = serde_json::to_value(&*wire)
            {
                let response = &mut event["response"];
                if let Some(voice) = response
                    .as_object_mut()
                    .and_then(|r| r.remove("audio"))
                    .and_then(|a| a.pointer("/output/voice").cloned())
                {
                    response["voice"] = voice;
                }
                if response["conversation"] == "none"
                    && let Some(r) = response.as_object_mut()
                {
                    r.remove("conversation");
                }
                *wire = ClientEvent::Raw(event);
            }
        }
        wires
    }
}

impl RealtimeProtocol for InworldProtocol {
    type Wire = ClientEvent;

    fn from_config(cfg: &RealtimeConfig) -> RealtimeResult<Self> {
        // Reuse OpenAI's validation (api-key non-empty) + model/voice parsing
        // verbatim. (The Inworld-specific model name lives in the embedded inner;
        // it is NOT used in the connect URL — Inworld routes by session, not model.)
        let inner = OpenAiProtocol::from_config(cfg)?;
        Ok(Self {
            inner,
            session_key: cfg
                .fix("inworld_session_key")
                .then(|| uuid::Uuid::new_v4().to_string()),
            transcript_index: cfg.fix("inworld_transcript_index"),
            response_shape: cfg.fix("inworld_response_shape"),
        })
    }

    fn provider_id(&self) -> &'static str {
        "inworld"
    }

    fn caps(&self) -> ProtocolCaps {
        // GA wire ⇒ OpenAI GA capabilities.
        self.inner.caps()
    }

    fn connect_spec(&self, cfg: &RealtimeConfig) -> RealtimeResult<ConnectSpec> {
        // Inworld realtime requires a backend-MINTED session id passed as `?key=`,
        // plus server-side HTTP **Basic** auth (base64 of the api-key) — NOT Bearer,
        // and NOT a `?model=` query. WaaV does not yet mint the session (that needs
        // Inworld's REST session-create handshake — a Phase-5 `RestHandshake`
        // transport follow-up), so the session id must be supplied out-of-band via
        // `endpoint`. Without it, fail loudly rather than dial a dead URL.
        // Fix `inworld_basic_key`: the Portal's API key is already `base64(key_id:secret)`:
        // "Do not encode a copied Base64 credential again"; only a `key_id:secret` pair is
        // encoded, once (<https://docs.inworld.ai/portal/api-key-auth>). Today every key is.
        let auth = if cfg.fix("inworld_basic_key") && !cfg.api_key.contains(':') {
            format!("Basic {}", cfg.api_key)
        } else {
            format!("Basic {}", BASE64_STANDARD.encode(cfg.api_key.as_bytes()))
        };

        // SERVER-CONFIG `realtime_endpoint_override` (proxy / self-hosted / gov-cloud
        // / local mock) WINS over the documented host. It is a SEPARATE field from
        // `endpoint` (the session id): when set we dial it VERBATIM and DON'T require
        // a minted session id — the override target (a proxy/mock) owns session
        // routing. The Basic-auth header rides along unchanged. SSRF-safe: never
        // client-settable (see `apply_endpoint_override`).
        if let Some(override_url) = cfg
            .realtime_endpoint_override
            .as_deref()
            .map(str::trim)
            .filter(|s| s.starts_with("ws://") || s.starts_with("wss://"))
        {
            return Ok(ConnectSpec::WebSocket {
                url: override_url.to_string(),
                headers: vec![("Authorization".to_string(), auth)],
            });
        }

        let session = cfg
            .endpoint
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .or(self.session_key.as_deref())
            .ok_or_else(|| {
                RealtimeError::InvalidConfiguration(
                    "Inworld realtime needs a minted session id: set `endpoint` to the \
                     session id returned by Inworld's session-create API (automatic \
                     minting is a pending REST-handshake follow-up)"
                        .to_string(),
                )
            })?;
        Ok(ConnectSpec::WebSocket {
            url: format!("{INWORLD_REALTIME_URL}?key={session}&protocol=realtime"),
            headers: vec![("Authorization".to_string(), auth)],
        })
    }

    // ── every wire method delegates to the embedded OpenAI GA protocol ──
    // Inworld bootstraps with `session.created` (like OpenAI), so map_server_event
    // is NOT overridden — it delegates too.

    fn build_session_config(
        &self,
        cfg: &RealtimeConfig,
        resumption: Option<&str>,
    ) -> Vec<Self::Wire> {
        let mut wires = self.inner.build_session_config(cfg, resumption);
        crate::core::realtime::openai::clone::edit_session(&mut wires, |session| {
            // Fix `openai_clone_voice`: Inworld's own voice ("e.g. `Dennis`").
            if cfg.fix("openai_clone_voice") {
                crate::core::realtime::openai::clone::set_voice(session, cfg.voice.as_deref());
            }
            // Fix `inworld_model`: "If you omit `model`, the default model
            // (`google-ai-studio/gemini-2.5-flash`) is used".
            if cfg.fix("inworld_model") && !cfg.model.trim().is_empty() {
                session["model"] = cfg.model.trim().into();
            }
            // Fix `inworld_tool_response`: with `auto_tool_response` (default `true`) Inworld
            // answers a tool result itself - "Do not send `response.create` after sending the
            // tool result"; `false` "gives OpenAI-compatible behavior", which the gateway's
            // tool results follow.
            if cfg.fix("inworld_tool_response") {
                session["providerData"] = serde_json::json!({ "auto_tool_response": false });
            }
            // Fix `inworld_stt_default`: OpenAI's `whisper-1` (the gateway's default when the
            // client names none) is unknown to Inworld, and "the rest of the `session.update`
            // is not applied".
            if cfg.fix("inworld_stt_default")
                && let Some(model) = session
                    .pointer_mut("/audio/input/transcription/model")
                    .filter(|m| m.as_str() == Some("whisper-1"))
            {
                *model = INWORLD_STT_MODEL.into();
            }
        });
        wires
    }

    fn map_server_event(&self, raw: Inbound<'_>) -> Vec<S2sEvent> {
        // Fix `inworld_transcript_index`: `content_index` is optional in Inworld's
        // `conversation.item.input_audio_transcription.completed`.
        if self.transcript_index
            && let Inbound::Text(text) = raw
            && text.contains("conversation.item.input_audio_transcription.completed")
            && let Ok(mut v) = serde_json::from_str::<serde_json::Value>(text)
            && let Some(event) = v.as_object_mut()
            && !event.contains_key("content_index")
        {
            event.insert("content_index".into(), 0.into());
            return self.inner.map_server_event(Inbound::Text(&v.to_string()));
        }
        self.inner.map_server_event(raw)
    }

    fn encode_user_audio(&self, pcm: &[u8]) -> Self::Wire {
        self.inner.encode_user_audio(pcm)
    }

    fn send_text(&self, text: &str) -> Vec<Self::Wire> {
        self.inner.send_text(text)
    }

    fn create_response(&self, overrides: Option<&RealtimeResponseOverride>) -> Vec<Self::Wire> {
        self.shaped(self.inner.create_response(overrides))
    }

    /// `response.speak` {text}: "Text to speak, verbatim"; the other verbs as OpenAI's
    /// (<https://docs.inworld.ai/api-reference/realtimeAPI/realtime/realtime-websocket.md>).
    fn verb(&self, verb: &RealtimeVerb<'_>) -> Option<Vec<Self::Wire>> {
        match *verb {
            RealtimeVerb::SayExact(text) => Some(vec![ClientEvent::Raw(serde_json::json!({
                "type": "response.speak",
                "text": text,
            }))]),
            _ => self.inner.verb(verb).map(|wires| self.shaped(wires)),
        }
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
        self.inner.replay_item(item)
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

    fn base_cfg() -> RealtimeConfig {
        RealtimeConfig {
            provider: "inworld".into(),
            api_key: "inkey".into(),
            model: "inworld-realtime".into(),
            voice: Some("alloy".into()),
            ..Default::default()
        }
    }

    fn proto(cfg: &RealtimeConfig) -> InworldProtocol {
        InworldProtocol::from_config(cfg).unwrap()
    }

    /// from_config validates the api key (delegated to OpenAI) — empty ⇒ Err.
    #[test]
    fn from_config_requires_api_key() {
        let cfg = RealtimeConfig {
            api_key: String::new(),
            ..Default::default()
        };
        assert!(matches!(
            InworldProtocol::from_config(&cfg),
            Err(RealtimeError::AuthenticationFailed(_))
        ));
    }

    /// connect_spec WITHOUT a session id (no `endpoint`) ⇒ InvalidConfiguration:
    /// Inworld needs a backend-minted session, so we error rather than dial a
    /// provably-dead URL.
    #[test]
    fn connect_spec_requires_session_id() {
        let p = proto(&base_cfg());
        assert!(matches!(
            p.connect_spec(&base_cfg()),
            Err(RealtimeError::InvalidConfiguration(_))
        ));
    }

    /// connect_spec WITH a session id (via `endpoint`): the documented Inworld
    /// session URL (`?key=<session>&protocol=realtime`) + server-side HTTP **Basic**
    /// auth (base64 of the api-key) — NOT Bearer, NOT a `?model=` query, NO WS
    /// subprotocol header.
    #[test]
    fn connect_spec_uses_session_url_and_basic_auth() {
        let cfg = RealtimeConfig {
            endpoint: Some("sess-abc123".into()),
            ..base_cfg()
        };
        let p = proto(&cfg);
        let ConnectSpec::WebSocket { url, headers } = p.connect_spec(&cfg).unwrap() else {
            panic!("expected ConnectSpec::WebSocket")
        };
        assert_eq!(
            url,
            "wss://api.inworld.ai/api/v1/realtime/session?key=sess-abc123&protocol=realtime"
        );
        let auth = headers
            .iter()
            .find(|(k, _)| k == "Authorization")
            .map(|(_, v)| v.as_str())
            .unwrap();
        // Basic base64("inkey") == "aW5rZXk=" — server-side Basic, not Bearer.
        assert_eq!(auth, "Basic aW5rZXk=");
        assert!(
            !headers.iter().any(|(k, _)| k == "Sec-WebSocket-Protocol"),
            "Inworld server-side Basic auth uses NO WS subprotocol header"
        );
    }

    /// SERVER-CONFIG `realtime_endpoint_override` WINS VERBATIM and BYPASSES the
    /// minted-session-id requirement (the override target — a proxy/mock — owns
    /// session routing). NOTE there is NO `endpoint` here, yet connect_spec
    /// SUCCEEDS (vs `connect_spec_requires_session_id`). Basic auth rides along.
    /// Server-config-only (never client-settable).
    #[test]
    fn connect_spec_honors_server_endpoint_override_without_session_id() {
        let cfg = RealtimeConfig {
            realtime_endpoint_override: Some("ws://127.0.0.1:9004/inworld".into()),
            ..base_cfg() // no `endpoint` / session id
        };
        let ConnectSpec::WebSocket { url, headers } = proto(&cfg).connect_spec(&cfg).unwrap()
        else {
            panic!("expected WebSocket")
        };
        assert_eq!(url, "ws://127.0.0.1:9004/inworld");
        assert!(
            headers
                .iter()
                .any(|(k, v)| k == "Authorization" && v == "Basic aW5rZXk=")
        );
    }

    /// Inworld bootstraps with session.created (like OpenAI) — delegated mapping.
    #[test]
    fn session_created_maps_via_delegation() {
        let p = proto(&base_cfg());
        let created = r#"{"type":"session.created","session":{"id":"sess_x","object":"realtime.session","model":"inworld","expires_at":0,"modalities":[]}}"#;
        match p.map_server_event(Inbound::Text(created)).as_slice() {
            [S2sEvent::SessionReady { session_id }] => {
                assert_eq!(session_id.as_deref(), Some("sess_x"));
            }
            other => panic!("expected delegated SessionReady, got {other:?}"),
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
        let append = p.encode_user_audio(&[1u8, 2, 3, 4]);
        let append_json = match p.serialize(&append).unwrap() {
            OutFrame::Text(s) => s,
            OutFrame::Binary(_) => panic!("text frame expected"),
        };
        assert!(append_json.contains("input_audio_buffer.append"));
    }

    fn fixed(fixes: &[&str], cfg: RealtimeConfig) -> (InworldProtocol, RealtimeConfig) {
        let cfg = RealtimeConfig {
            provider_fixes: fixes.iter().map(|f| (*f).to_string()).collect(),
            ..cfg
        };
        (proto(&cfg), cfg)
    }

    fn connect(p: &InworldProtocol, cfg: &RealtimeConfig) -> (String, String) {
        let ConnectSpec::WebSocket { url, headers } = p.connect_spec(cfg).unwrap() else {
            panic!("expected WebSocket")
        };
        let auth = headers
            .into_iter()
            .find(|(k, _)| k == "Authorization")
            .unwrap()
            .1;
        (url, auth)
    }

    fn session_json(p: &InworldProtocol, cfg: &RealtimeConfig) -> serde_json::Value {
        let wires = p.build_session_config(cfg, None);
        match p.serialize(&wires[0]).unwrap() {
            OutFrame::Text(s) => {
                serde_json::from_str::<serde_json::Value>(&s).unwrap()["session"].clone()
            }
            OutFrame::Binary(_) => panic!("text frame"),
        }
    }

    /// `inworld_basic_key`: the Portal's Base64 key goes as it is (`Basic $INWORLD_API_KEY`);
    /// a `key_id:secret` pair is encoded once. Off: every key is encoded.
    #[test]
    fn the_basic_key_fix_sends_the_copied_key_as_it_is() {
        let cfg = RealtimeConfig {
            api_key: "a2V5X2lkOnNlY3JldA==".into(),
            endpoint: Some("sess-1".into()),
            ..base_cfg()
        };
        let (p, on) = fixed(&["inworld_basic_key"], cfg.clone());
        assert_eq!(connect(&p, &on).1, "Basic a2V5X2lkOnNlY3JldA==");
        let pair = RealtimeConfig {
            api_key: "key_id:secret".into(),
            ..on.clone()
        };
        assert_eq!(
            connect(&proto(&pair), &pair).1,
            "Basic a2V5X2lkOnNlY3JldA=="
        );
        assert_eq!(
            connect(&proto(&cfg), &cfg).1,
            "Basic YTJWNVgybGtPbk5sWTNKbGRBPT0="
        );
    }

    /// `inworld_session_key`: with no `endpoint` the session dials its own id
    /// (`?key=<id>&protocol=realtime`), the same on every dial. Off: refused.
    #[test]
    fn the_session_key_fix_dials_its_own_session_id() {
        let (p, on) = fixed(&["inworld_session_key"], base_cfg());
        let (url, _) = connect(&p, &on);
        let key = url
            .strip_prefix(&format!("{INWORLD_REALTIME_URL}?key="))
            .and_then(|rest| rest.strip_suffix("&protocol=realtime"))
            .expect("the documented session URL");
        assert!(uuid::Uuid::parse_str(key).is_ok(), "{url}");
        assert_eq!(connect(&p, &on).0, url, "a re-dial keeps the id");
        assert!(proto(&base_cfg()).connect_spec(&base_cfg()).is_err());
    }

    /// The session fixes: Inworld's voice as given (`openai_clone_voice`), the model
    /// (`inworld_model`), `providerData.auto_tool_response: false` (`inworld_tool_response`)
    /// and Inworld's transcription model for OpenAI's `whisper-1` (`inworld_stt_default`).
    /// Off: `alloy`, no model, no providerData, `whisper-1`.
    #[test]
    fn the_session_fixes_send_inworlds_voice_model_and_transcription() {
        let cfg = RealtimeConfig {
            voice: Some("Clive".into()),
            model: "openai/gpt-4.1-mini".into(),
            input_audio_transcription: Some(crate::core::realtime::InputTranscriptionConfig {
                model: "whisper-1".into(),
            }),
            ..base_cfg()
        };
        let all = [
            "openai_clone_voice",
            "inworld_model",
            "inworld_tool_response",
            "inworld_stt_default",
        ];
        let (p, on) = fixed(&all, cfg.clone());
        let session = session_json(&p, &on);
        assert_eq!(session["audio"]["output"]["voice"], "Clive");
        assert_eq!(session["model"], "openai/gpt-4.1-mini");
        assert_eq!(
            session["providerData"],
            serde_json::json!({"auto_tool_response": false})
        );
        assert_eq!(
            session["audio"]["input"]["transcription"]["model"],
            INWORLD_STT_MODEL
        );
        let today = session_json(&proto(&cfg), &cfg);
        assert_eq!(today["audio"]["output"]["voice"], "alloy");
        assert!(today.get("model").is_none() && today.get("providerData").is_none());
        assert_eq!(
            today["audio"]["input"]["transcription"]["model"],
            "whisper-1"
        );
        // No voice named: none is sent (Inworld's default `Dennis`).
        let unnamed = RealtimeConfig { voice: None, ..on };
        assert!(
            session_json(&p, &unnamed)["audio"]["output"]
                .get("voice")
                .is_none()
        );
    }

    /// `inworld_transcript_index`: a user transcript without `content_index` is read.
    #[test]
    fn the_transcript_index_fix_reads_a_transcript_without_content_index() {
        let raw = r#"{"type":"conversation.item.input_audio_transcription.completed","event_id":"e1","item_id":"item_1","transcript":"Hello there"}"#;
        let (p, _) = fixed(&["inworld_transcript_index"], base_cfg());
        assert!(
            p.map_server_event(Inbound::Text(raw))
                .iter()
                .any(|e| matches!(e, S2sEvent::Transcript { text, .. } if text == "Hello there"))
        );
        assert!(matches!(
            proto(&base_cfg())
                .map_server_event(Inbound::Text(raw))
                .as_slice(),
            [S2sEvent::Ignore]
        ));
    }

    /// `inworld_response_shape`: the per-response voice is `response.voice` and an
    /// out-of-band response has no `conversation` `none`.
    #[test]
    fn the_response_shape_fix_puts_the_voice_at_the_top() {
        let overrides = RealtimeResponseOverride {
            voice: Some("Olivia".into()),
            out_of_band: true,
            ..Default::default()
        };
        let response = |p: &InworldProtocol| match p
            .serialize(&p.create_response(Some(&overrides))[0])
            .unwrap()
        {
            OutFrame::Text(s) => {
                serde_json::from_str::<serde_json::Value>(&s).unwrap()["response"].clone()
            }
            OutFrame::Binary(_) => panic!("text frame"),
        };
        let (p, _) = fixed(&["inworld_response_shape"], base_cfg());
        let shaped = response(&p);
        assert_eq!(shaped["voice"], "Olivia");
        assert!(shaped.get("audio").is_none() && shaped.get("conversation").is_none());
        let today = response(&proto(&base_cfg()));
        assert_eq!(today["audio"]["output"]["voice"], "Olivia");
        assert_eq!(today["conversation"], "none");
    }

    /// The verbs: `response.speak` speaks exact words; the others as OpenAI's.
    #[test]
    fn the_verbs_speak_exact_words_with_response_speak() {
        let p = proto(&base_cfg());
        let speak = p
            .verb(&RealtimeVerb::SayExact("One moment, please."))
            .unwrap();
        match p.serialize(&speak[0]).unwrap() {
            OutFrame::Text(s) => assert_eq!(
                serde_json::from_str::<serde_json::Value>(&s).unwrap(),
                serde_json::json!({"type": "response.speak", "text": "One moment, please."})
            ),
            OutFrame::Binary(_) => panic!("text frame"),
        }
        for verb in [
            RealtimeVerb::SayGuided("x"),
            RealtimeVerb::AddContext("x"),
            RealtimeVerb::StopOutput,
        ] {
            assert!(p.verb(&verb).is_some(), "{verb:?}");
        }
    }
}
