//! Grok / xAI Realtime [`RealtimeProtocol`] — an OpenAI-PROTOCOL CLONE.
//!
//! xAI's Realtime API is GA-compatible: it speaks the OpenAI GA wire, so this
//! protocol EMBEDS an [`OpenAiProtocol`] and DELEGATES every wire method to it
//! verbatim — overriding ONLY `provider_id`, `connect_spec` (the xAI host), and
//! `map_server_event` (xAI bootstraps the session with a `conversation.created`
//! frame, whereas OpenAI sends `session.created`).

use serde_json::Value;

use super::super::openai::protocol::OpenAiProtocol;
use crate::core::realtime::base::{RealtimeConfig, RealtimeResponseOverride, RealtimeResult, ReplayConversationItem};
use crate::core::realtime::openai::ClientEvent;
use crate::core::realtime::scaffold::{ConnectSpec, Inbound, OutFrame, ProtocolCaps, RealtimeProtocol, S2sEvent};

/// xAI realtime WebSocket base URL. BEST-KNOWN: xAI's realtime surface is behind
/// a key we do not hold, so this endpoint is the documented/best-known form and
/// has NOT been live-probed. The `?model=<model>` query mirrors OpenAI.
pub(crate) const GROK_REALTIME_URL: &str = "wss://api.x.ai/v1/realtime";

/// Grok / xAI Realtime protocol. A thin wrapper over [`OpenAiProtocol`]: the GA
/// wire is delegated verbatim; the endpoint + the bootstrap event differ.
pub struct GrokProtocol {
    /// The embedded OpenAI GA protocol — every wire method delegates here.
    inner: OpenAiProtocol,
    /// The model string for the `?model=` query (raw `cfg.model`, or a default).
    model: String,
}

impl GrokProtocol {
    /// Borrow the embedded OpenAI protocol (for the newtype's inherent accessors).
    pub(crate) fn inner(&self) -> &OpenAiProtocol {
        &self.inner
    }
}

impl RealtimeProtocol for GrokProtocol {
    type Wire = ClientEvent;

    fn from_config(cfg: &RealtimeConfig) -> RealtimeResult<Self> {
        // Reuse OpenAI's validation (api-key non-empty) + parsing verbatim.
        let inner = OpenAiProtocol::from_config(cfg)?;
        // xAI routes by model string in the query; keep the raw config string so
        // an xAI-specific model name isn't coerced to an OpenAI enum value.
        let model = if cfg.model.trim().is_empty() {
            "grok-realtime".to_string()
        } else {
            cfg.model.trim().to_string()
        };
        Ok(Self { inner, model })
    }

    fn provider_id(&self) -> &'static str {
        "grok"
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
        self.inner.map_server_event(raw)
    }

    // ── every remaining wire method delegates to the embedded OpenAI GA protocol ──

    fn build_session_config(
        &self,
        cfg: &RealtimeConfig,
        resumption: Option<&str>,
    ) -> Vec<Self::Wire> {
        self.inner.build_session_config(cfg, resumption)
    }

    fn encode_user_audio(&self, pcm: &[u8]) -> Self::Wire {
        self.inner.encode_user_audio(pcm)
    }

    fn send_text(&self, text: &str) -> Vec<Self::Wire> {
        self.inner.send_text(text)
    }

    fn create_response(&self, overrides: Option<&RealtimeResponseOverride>) -> Vec<Self::Wire> {
        self.inner.create_response(overrides)
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
            !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("api-key")),
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
        let ConnectSpec::WebSocket { url, headers } = proto(&cfg).connect_spec(&cfg).unwrap() else {
            panic!("expected WebSocket")
        };
        assert_eq!(url, "ws://127.0.0.1:9003/grok");
        assert!(headers.iter().any(|(k, v)| k == "Authorization" && v == "Bearer xaikey"));
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
}
