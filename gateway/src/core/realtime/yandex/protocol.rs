//! Yandex Cloud AI Studio Realtime [`RealtimeProtocol`] — an OpenAI-PROTOCOL CLONE.
//!
//! Yandex's Realtime API (Yandex Cloud AI Studio's speech-to-speech Realtime API)
//! is **OpenAI-Realtime-GA-COMPATIBLE**: the OFFICIAL Yandex SDK example connects
//! with the OpenAI Python SDK (`AsyncOpenAI` + `client.realtime.connect`) against
//! `wss://ai.api.cloud.yandex.net/v1` — so the wire IS the OpenAI GA wire (client
//! `session.update` with nested `audio.format.type="audio/pcm"`,
//! `input_audio_buffer.append`; server `session.updated`,
//! `response.output_audio.delta`, `response.done`, …). This protocol therefore
//! EMBEDS an [`OpenAiProtocol`] and DELEGATES every wire method to it verbatim —
//! the GA wire stays byte-identical to the live-validated OpenAI client —
//! overriding ONLY `provider_id` and `connect_spec`.
//!
//! Yandex differs from OpenAI ONLY in the connect target + auth:
//! - **URL**: `wss://ai.api.cloud.yandex.net/v1/realtime?model=<MODEL-URI>` where
//!   the model is a Yandex model URI `gpt://<FOLDER_ID>/speech-realtime-250923`
//!   (the OpenAI SDK opens exactly this when given `websocket_base_url=wss://…/v1`
//!   + `model=gpt://…`). The `gpt://…` URI is URL-ENCODED into the `?model=` query
//!   so the `:` and `/` it contains don't break the query.
//! - **Auth**: `Authorization` header, scheme chosen by credential TYPE (both
//!   grounded in the official examples + accepted by the server): a Yandex IAM
//!   TOKEN (`t1.…`) ⇒ `Bearer <token>` (the OpenAI-SDK example,
//!   `openai_custom_session_update_example.py`); a STATIC API key ⇒ `Api-Key <key>`
//!   (the raw-aiohttp `voice_agent.py`). The caller supplies the token/key
//!   as `cfg.api_key`.
//!
//! NO Yandex key is held, so this is unit + mock-validated; the wire is GROUNDED in
//! the official SDK example (NOT invented) — hence the verbatim DELEGATION.

use super::super::openai::protocol::OpenAiProtocol;
use crate::core::realtime::base::{
    RealtimeConfig, RealtimeError, RealtimeResponseOverride, RealtimeResult, ReplayConversationItem,
};
use crate::core::realtime::openai::ClientEvent;
use crate::core::realtime::scaffold::{
    ConnectSpec, Inbound, OutFrame, ProtocolCaps, RealtimeProtocol, S2sEvent,
};

/// Yandex Cloud AI Studio Realtime WebSocket base URL. The OpenAI SDK opens
/// `<websocket_base_url>/realtime?model=<MODEL>`; with `websocket_base_url` =
/// `wss://ai.api.cloud.yandex.net/v1` that is this host + `/realtime`. Confirmed
/// from the official Yandex SDK example (`YANDEX_WSS_BASE`).
pub(crate) const YANDEX_REALTIME_URL: &str = "wss://ai.api.cloud.yandex.net/v1/realtime";

/// The default Yandex realtime model NAME (the trailing segment of the model URI
/// `gpt://<FOLDER_ID>/<MODEL_NAME>`). From the official example
/// (`speech-realtime-250923`).
pub(crate) const YANDEX_DEFAULT_MODEL: &str = "speech-realtime-250923";

/// Yandex Realtime protocol. A thin wrapper over [`OpenAiProtocol`]: the GA wire is
/// delegated verbatim; only the connect target (the Yandex host + the `gpt://…`
/// model URI built from the folder id) + Bearer auth differ.
pub struct YandexProtocol {
    /// The embedded OpenAI GA protocol — every wire method delegates here.
    inner: OpenAiProtocol,
    /// The Yandex Cloud folder id (the `<FOLDER_ID>` in the `gpt://<FOLDER_ID>/…`
    /// model URI). REQUIRED. Parsed from `cfg.endpoint` (the generic endpoint slot,
    /// like Azure's resource) — for the realtime handler this is injected from the
    /// server-config `yandex_folder_id` (`YANDEX_FOLDER_ID`).
    folder_id: String,
    /// The fully-built Yandex model URI `gpt://<folder_id>/<model_name>` used in the
    /// `?model=` connect query (kept as a raw string — NOT coerced to an OpenAI enum
    /// — so the Yandex-specific URI survives intact). `model_name` is `cfg.model`
    /// when it looks like a bare model name, else [`YANDEX_DEFAULT_MODEL`].
    model_uri: String,
}

impl YandexProtocol {
    /// Borrow the embedded OpenAI protocol (for the newtype's inherent accessors).
    pub(crate) fn inner(&self) -> &OpenAiProtocol {
        &self.inner
    }

    /// The Yandex Cloud folder id this protocol was built with.
    pub(crate) fn folder_id(&self) -> &str {
        &self.folder_id
    }

    /// The fully-built `gpt://<folder_id>/<model_name>` model URI.
    pub(crate) fn model_uri(&self) -> &str {
        &self.model_uri
    }
}

impl RealtimeProtocol for YandexProtocol {
    type Wire = ClientEvent;

    fn from_config(cfg: &RealtimeConfig) -> RealtimeResult<Self> {
        // Reuse the OpenAI protocol's validation (api-key non-empty, model/voice/
        // audio-format parsing) verbatim. The api-key (a Yandex IAM token or static
        // API key) MUST be non-empty — `OpenAiProtocol::from_config` enforces that.
        let inner = OpenAiProtocol::from_config(cfg)?;

        // Yandex needs the FOLDER ID to build the `gpt://<folder>/…` model URI. Read
        // it from `cfg.endpoint` (the generic endpoint slot — the handler injects
        // the server-config `yandex_folder_id` there, mirroring Azure's resource).
        // Without it we cannot build a valid model URI → fail loudly.
        let folder_id = cfg
            .endpoint
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                RealtimeError::InvalidConfiguration(
                    "Yandex realtime requires the folder id in `endpoint` (YANDEX_FOLDER_ID)"
                        .to_string(),
                )
            })?
            .to_string();

        // The realtime model NAME: `cfg.model` if it looks like a bare model name,
        // else the default. A bare name has NO scheme/slash (an operator who already
        // typed a full `gpt://…` URI, or left it blank/defaulted, falls back to the
        // canonical default so we never double-wrap the URI).
        let model_trimmed = cfg.model.trim();
        let model_name = if model_trimmed.is_empty()
            || model_trimmed.contains("://")
            || model_trimmed.contains('/')
        {
            YANDEX_DEFAULT_MODEL
        } else {
            model_trimmed
        };
        let model_uri = format!("gpt://{folder_id}/{model_name}");

        Ok(Self {
            inner,
            folder_id,
            model_uri,
        })
    }

    fn provider_id(&self) -> &'static str {
        "yandex"
    }

    fn caps(&self) -> ProtocolCaps {
        // Yandex realtime = OpenAI GA capabilities (the embedded protocol's caps —
        // same audio formats, server-VAD default, truncate + input-buffer support).
        self.inner.caps()
    }

    fn connect_spec(&self, cfg: &RealtimeConfig) -> RealtimeResult<ConnectSpec> {
        // URL-ENCODE the `gpt://<folder>/<model>` URI for the `?model=` query: it
        // contains `:` and `/`, which must be percent-encoded so the query is valid
        // (the codebase-standard encoder, used by the Hume/Sarvam/Deepgram URLs).
        let model_encoded: String =
            url::form_urlencoded::byte_serialize(self.model_uri.as_bytes()).collect();
        let default_url = format!("{YANDEX_REALTIME_URL}?model={model_encoded}");

        // Auth: `Authorization: Bearer <token>` — the OpenAI SDK path the official
        // Yandex example uses (`AsyncOpenAI(api_key=<IAM_TOKEN>)` sends Bearer). The
        // token is a Yandex IAM token OR a static API key, supplied as `cfg.api_key`.
        // The `realtime` WS subprotocol matches OpenAI; standard WS-upgrade headers
        // are generated by `into_client_request()` in the transport factory.
        //
        // SERVER-CONFIG `realtime_endpoint_override` (proxy / self-hosted / mock —
        // e.g. `YANDEX_REALTIME_URL`) WINS over the Yandex host when set; the Bearer
        // header rides along unchanged. SSRF-safe: never client-settable.
        let url = crate::core::realtime::scaffold::apply_endpoint_override(
            &default_url,
            cfg.realtime_endpoint_override.as_deref(),
            None,
        );
        // Auth scheme by credential TYPE — both grounded in the official Yandex
        // examples + accepted by the server (review wf_ebb4d28a #1): a Yandex IAM
        // TOKEN (prefix `t1.`, short-lived) ⇒ `Authorization: Bearer <token>` (the
        // OpenAI-SDK path, openai_custom_session_update_example.py); a STATIC API key
        // (the `yandex_api_key`/`YANDEX_API_KEY` named case — does NOT start `t1.`)
        // ⇒ `Authorization: Api-Key <key>` (the raw-aiohttp voice_agent.py path).
        let auth = if cfg.api_key.starts_with("t1.") {
            format!("Bearer {}", cfg.api_key)
        } else {
            format!("Api-Key {}", cfg.api_key)
        };
        Ok(ConnectSpec::WebSocket {
            url,
            headers: vec![
                ("Authorization".to_string(), auth),
                ("Sec-WebSocket-Protocol".to_string(), "realtime".to_string()),
            ],
        })
    }

    // ── every wire method delegates to the embedded OpenAI GA protocol ──
    // Yandex bootstraps with `session.created`/`session.updated` (like OpenAI), so
    // map_server_event is NOT overridden — it delegates too.

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
            provider: "yandex".into(),
            api_key: "iam-token".into(),
            // A bare model name → wrapped into the gpt:// URI; defaults otherwise.
            model: String::new(),
            voice: Some("alloy".into()),
            // The folder id rides in `endpoint` (handler injects yandex_folder_id).
            endpoint: Some("b1gfolder123".into()),
            ..Default::default()
        }
    }

    fn proto(cfg: &RealtimeConfig) -> YandexProtocol {
        YandexProtocol::from_config(cfg).unwrap()
    }

    /// from_config validates the api key (delegated to OpenAI) — empty ⇒ Err.
    #[test]
    fn from_config_requires_api_key() {
        let cfg = RealtimeConfig {
            api_key: String::new(),
            endpoint: Some("b1gfolder123".into()),
            ..Default::default()
        };
        assert!(matches!(
            YandexProtocol::from_config(&cfg),
            Err(RealtimeError::AuthenticationFailed(_))
        ));
    }

    /// from_config requires the folder id (`endpoint`) — empty/missing ⇒
    /// InvalidConfiguration (Yandex can't build the gpt:// model URI alone).
    #[test]
    fn from_config_requires_folder_id() {
        // Missing endpoint.
        let cfg = RealtimeConfig {
            api_key: "k".into(),
            endpoint: None,
            ..Default::default()
        };
        assert!(matches!(
            YandexProtocol::from_config(&cfg),
            Err(RealtimeError::InvalidConfiguration(_))
        ));
        // Present-but-empty endpoint also rejected.
        let cfg_empty = RealtimeConfig {
            api_key: "k".into(),
            endpoint: Some("   ".into()),
            ..Default::default()
        };
        assert!(matches!(
            YandexProtocol::from_config(&cfg_empty),
            Err(RealtimeError::InvalidConfiguration(_))
        ));
    }

    /// The built model URI is `gpt://<folder>/<default-model>` when `model` is
    /// blank, and `gpt://<folder>/<name>` when a bare model name is supplied.
    #[test]
    fn builds_gpt_model_uri() {
        // Blank model → default name.
        assert_eq!(
            proto(&base_cfg()).model_uri(),
            "gpt://b1gfolder123/speech-realtime-250923"
        );
        // Bare model name → used verbatim.
        let cfg = RealtimeConfig {
            model: "speech-realtime-next".into(),
            ..base_cfg()
        };
        assert_eq!(
            proto(&cfg).model_uri(),
            "gpt://b1gfolder123/speech-realtime-next"
        );
        // A full gpt:// URI in `model` is NOT double-wrapped (falls back to default).
        let cfg_uri = RealtimeConfig {
            model: "gpt://other/already-a-uri".into(),
            ..base_cfg()
        };
        assert_eq!(
            proto(&cfg_uri).model_uri(),
            "gpt://b1gfolder123/speech-realtime-250923"
        );
        assert_eq!(proto(&base_cfg()).folder_id(), "b1gfolder123");
    }

    /// connect_spec: the EXACT Yandex URL — the `gpt://<folder>/<model>` URI
    /// URL-ENCODED into the `?model=` query — and `Authorization: Bearer <token>`
    /// (NOT the Azure api-key header).
    #[test]
    fn connect_spec_uses_yandex_url_and_bearer() {
        let p = proto(&base_cfg());
        let ConnectSpec::WebSocket { url, headers } = p.connect_spec(&base_cfg()).unwrap() else {
            panic!("expected ConnectSpec::WebSocket")
        };
        // `gpt://b1gfolder123/speech-realtime-250923` percent-encoded:
        // `:` → %3A, `/` → %2F.
        assert_eq!(
            url,
            "wss://ai.api.cloud.yandex.net/v1/realtime?model=gpt%3A%2F%2Fb1gfolder123%2Fspeech-realtime-250923"
        );
        // The decoded `model` query MUST round-trip back to the gpt:// URI.
        let model_q = url.split("model=").nth(1).unwrap();
        let decoded: String = url::form_urlencoded::parse(format!("model={model_q}").as_bytes())
            .find(|(k, _)| k == "model")
            .map(|(_, v)| v.into_owned())
            .unwrap();
        assert_eq!(decoded, "gpt://b1gfolder123/speech-realtime-250923");
        // Auth scheme by credential TYPE. base_cfg's key ("iam-token") does NOT
        // start `t1.` ⇒ treated as a STATIC API key ⇒ `Api-Key <key>`.
        let auth = headers
            .iter()
            .find(|(k, _)| k == "Authorization")
            .map(|(_, v)| v.as_str())
            .unwrap();
        assert_eq!(auth, "Api-Key iam-token");
        // A real IAM token (`t1.` prefix) ⇒ `Bearer <token>` (the OpenAI-SDK path).
        let cfg_iam = RealtimeConfig {
            api_key: "t1.realIamToken".into(),
            ..base_cfg()
        };
        let ConnectSpec::WebSocket { headers: h2, .. } =
            proto(&cfg_iam).connect_spec(&cfg_iam).unwrap()
        else {
            panic!("expected ConnectSpec::WebSocket")
        };
        let auth2 = h2
            .iter()
            .find(|(k, _)| k == "Authorization")
            .map(|(_, v)| v.as_str())
            .unwrap();
        assert_eq!(auth2, "Bearer t1.realIamToken");
        // Auth rides the standard `Authorization` header — NOT a separate Azure-style
        // `api-key` HEADER key.
        assert!(
            !headers
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("api-key")),
            "Yandex uses the Authorization header, not the Azure api-key header"
        );
        // The `realtime` WS subprotocol matches OpenAI.
        assert!(
            headers
                .iter()
                .any(|(k, v)| k == "Sec-WebSocket-Protocol" && v == "realtime")
        );
    }

    /// SERVER-CONFIG `realtime_endpoint_override` WINS over the Yandex host
    /// VERBATIM (proxy / mock); the Authorization header rides along. Server-config-
    /// only (never client-settable).
    #[test]
    fn connect_spec_honors_server_endpoint_override() {
        let cfg = RealtimeConfig {
            realtime_endpoint_override: Some("ws://127.0.0.1:9005/yandex".into()),
            ..base_cfg()
        };
        let ConnectSpec::WebSocket { url, headers } = proto(&cfg).connect_spec(&cfg).unwrap()
        else {
            panic!("expected WebSocket")
        };
        assert_eq!(url, "ws://127.0.0.1:9005/yandex");
        // base_cfg's static key ("iam-token", not `t1.`) ⇒ Api-Key; the auth header
        // rides along unchanged when the override wins.
        assert!(
            headers
                .iter()
                .any(|(k, v)| k == "Authorization" && v == "Api-Key iam-token")
        );
    }

    /// Delegation smoke test: session.update + audio append produce the SAME GA
    /// wire as OpenAI (because they delegate to the embedded OpenAiProtocol).
    #[test]
    fn delegates_ga_wire_verbatim() {
        let p = proto(&base_cfg());
        let session_wires = p.build_session_config(&base_cfg(), None);
        let session_json = match p.serialize(&session_wires[0]).unwrap() {
            OutFrame::Text(s) => s,
            OutFrame::Binary(_) => panic!("text frame expected"),
        };
        assert!(session_json.contains("session.update"));
        assert!(session_json.contains("\"type\":\"realtime\""));
        assert!(session_json.contains("output_modalities"));

        let append = p.encode_user_audio(&[0u8, 1, 2, 3]);
        let append_json = match p.serialize(&append).unwrap() {
            OutFrame::Text(s) => s,
            OutFrame::Binary(_) => panic!("text frame expected"),
        };
        assert!(append_json.contains("input_audio_buffer.append"));
    }

    /// Yandex bootstraps with `session.created` (like OpenAI) — delegated mapping.
    #[test]
    fn session_created_maps_via_delegation() {
        let p = proto(&base_cfg());
        let created = r#"{"type":"session.created","session":{"id":"sess_y","object":"realtime.session","model":"yandex","expires_at":0,"modalities":[]}}"#;
        match p.map_server_event(Inbound::Text(created)).as_slice() {
            [S2sEvent::SessionReady { session_id }] => {
                assert_eq!(session_id.as_deref(), Some("sess_y"));
            }
            other => panic!("expected delegated SessionReady, got {other:?}"),
        }
    }
}
