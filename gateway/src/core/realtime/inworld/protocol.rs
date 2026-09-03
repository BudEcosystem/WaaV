//! Inworld Realtime [`RealtimeProtocol`] — an OpenAI-PROTOCOL CLONE.
//!
//! Inworld's Realtime API speaks the OpenAI GA wire (it bootstraps with
//! `session.created`, exactly like OpenAI), so this protocol EMBEDS an
//! [`OpenAiProtocol`] and DELEGATES every wire method to it verbatim —
//! overriding ONLY `provider_id` + `connect_spec` (the Inworld host + auth).

use base64::prelude::{BASE64_STANDARD, Engine as _};

use super::super::openai::protocol::OpenAiProtocol;
use crate::core::realtime::base::{
    RealtimeConfig, RealtimeError, RealtimeResponseOverride, RealtimeResult, ReplayConversationItem,
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
}

impl InworldProtocol {
    /// Borrow the embedded OpenAI protocol (for the newtype's inherent accessors).
    pub(crate) fn inner(&self) -> &OpenAiProtocol {
        &self.inner
    }
}

impl RealtimeProtocol for InworldProtocol {
    type Wire = ClientEvent;

    fn from_config(cfg: &RealtimeConfig) -> RealtimeResult<Self> {
        // Reuse OpenAI's validation (api-key non-empty) + model/voice parsing
        // verbatim. (The Inworld-specific model name lives in the embedded inner;
        // it is NOT used in the connect URL — Inworld routes by session, not model.)
        let inner = OpenAiProtocol::from_config(cfg)?;
        Ok(Self { inner })
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
        let auth = format!("Basic {}", BASE64_STANDARD.encode(cfg.api_key.as_bytes()));

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
        self.inner.build_session_config(cfg, resumption)
    }

    fn map_server_event(&self, raw: Inbound<'_>) -> Vec<S2sEvent> {
        self.inner.map_server_event(raw)
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
}
