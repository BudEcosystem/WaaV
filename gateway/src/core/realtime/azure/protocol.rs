//! Azure OpenAI Realtime [`RealtimeProtocol`] — an OpenAI-PROTOCOL CLONE.
//!
//! Azure OpenAI's Realtime API speaks the EXACT OpenAI GA wire; it differs ONLY
//! in the connect target (the `<resource>.openai.azure.com` host + an
//! `api-version` + `deployment` query) and the auth header (`api-key`, NOT
//! `Authorization: Bearer`). So this protocol EMBEDS an [`OpenAiProtocol`] and
//! DELEGATES every wire method to it verbatim — the GA wire stays byte-identical
//! to the live-validated OpenAI client — overriding ONLY `provider_id` and
//! `connect_spec`.

use super::super::openai::protocol::OpenAiProtocol;
use crate::core::realtime::base::{RealtimeConfig, RealtimeError, RealtimeResponseOverride, RealtimeResult, ReplayConversationItem};
use crate::core::realtime::openai::ClientEvent;
use crate::core::realtime::scaffold::{ConnectSpec, Inbound, OutFrame, ProtocolCaps, RealtimeProtocol, S2sEvent};

/// Azure OpenAI Realtime GA endpoint path. WaaV sends the OpenAI **GA** wire
/// (the nested `session.update` etc.), and Microsoft's docs state the GA message
/// format is served on the `/openai/v1/realtime` path with NO `api-version`
/// query (the dated `?api-version=…-preview` endpoints serve the *preview*
/// protocol and reject GA-shaped `session.update`). So we target the GA path;
/// `deployment` still routes to the Azure deployment. (Review wf_0f21536d #2.)
pub(crate) const AZURE_REALTIME_PATH: &str = "/openai/v1/realtime";

/// Azure OpenAI Realtime protocol. A thin wrapper over [`OpenAiProtocol`]: the GA
/// wire is delegated verbatim; only the endpoint + auth header differ.
pub struct AzureProtocol {
    /// The embedded OpenAI GA protocol — every wire method delegates here.
    inner: OpenAiProtocol,
    /// The Azure resource (the `<resource>` in `<resource>.openai.azure.com`) OR a
    /// full `wss://…`/`https://…` base host. Parsed from `cfg.endpoint`.
    resource: String,
    /// The Azure deployment name (Azure routes by deployment, not model). Taken
    /// verbatim from `cfg.model` — Azure deployment names are arbitrary strings.
    deployment: String,
}

impl AzureProtocol {
    /// Borrow the embedded OpenAI protocol (for the newtype's inherent accessors).
    pub(crate) fn inner(&self) -> &OpenAiProtocol {
        &self.inner
    }

    /// Build the Azure realtime `wss://` URL from the resource + deployment +
    /// api-version. Accepts either a bare resource name (`myres`) or a full
    /// host/URL (`https://myres.openai.azure.com`, `wss://…`, `myres.openai.azure.com`)
    /// in `resource`, normalizing to the `wss://…/openai/realtime?…` form.
    fn build_url(&self) -> String {
        let r = self.resource.trim();
        // Derive the host: strip any scheme + trailing slash; if it already looks
        // like a host (contains a dot) keep it, else treat as a bare resource and
        // append the canonical Azure suffix.
        let host = {
            let no_scheme = r
                .strip_prefix("wss://")
                .or_else(|| r.strip_prefix("https://"))
                .or_else(|| r.strip_prefix("ws://"))
                .or_else(|| r.strip_prefix("http://"))
                .unwrap_or(r);
            let no_path = no_scheme.split('/').next().unwrap_or(no_scheme);
            let trimmed = no_path.trim_end_matches('/');
            if trimmed.contains('.') {
                trimmed.to_string()
            } else {
                format!("{trimmed}.openai.azure.com")
            }
        };
        format!(
            "wss://{host}{AZURE_REALTIME_PATH}?deployment={deployment}",
            deployment = self.deployment,
        )
    }
}

impl RealtimeProtocol for AzureProtocol {
    type Wire = ClientEvent;

    fn from_config(cfg: &RealtimeConfig) -> RealtimeResult<Self> {
        // Reuse the OpenAI protocol's validation (api-key non-empty, model/voice/
        // audio-format parsing) verbatim.
        let inner = OpenAiProtocol::from_config(cfg)?;

        // Azure needs a resource/endpoint to know which host to dial. Read it from
        // `cfg.endpoint` (the server's `azure_openai_endpoint`). Without it we
        // cannot build a valid URL → fail loudly rather than dial a bad host.
        let resource = cfg
            .endpoint
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                RealtimeError::InvalidConfiguration(
                    "Azure OpenAI Realtime requires an endpoint/resource (set azure_openai_endpoint, \
                     e.g. \"my-resource\" or \"https://my-resource.openai.azure.com\")"
                        .to_string(),
                )
            })?
            .to_string();

        // Azure routes by DEPLOYMENT (cfg.model carries the deployment name).
        let deployment = if cfg.model.trim().is_empty() {
            return Err(RealtimeError::InvalidConfiguration(
                "Azure OpenAI Realtime requires a deployment name in `model`".to_string(),
            ));
        } else {
            cfg.model.trim().to_string()
        };

        Ok(Self {
            inner,
            resource,
            deployment,
        })
    }

    fn provider_id(&self) -> &'static str {
        "azure"
    }

    fn caps(&self) -> ProtocolCaps {
        // Azure OpenAI Realtime = OpenAI GA capabilities (same audio formats,
        // server-VAD default, truncate + input-buffer support).
        self.inner.caps()
    }

    fn connect_spec(&self, cfg: &RealtimeConfig) -> RealtimeResult<ConnectSpec> {
        // Azure uses the `api-key` HEADER (NOT `Authorization: Bearer`). The
        // realtime subprotocol header matches OpenAI; standard WS-upgrade headers
        // are generated by `into_client_request()` in the transport factory.
        Ok(ConnectSpec::WebSocket {
            url: self.build_url(),
            headers: vec![
                ("api-key".to_string(), cfg.api_key.clone()),
                ("Sec-WebSocket-Protocol".to_string(), "realtime".to_string()),
            ],
        })
    }

    // ── every wire method delegates to the embedded OpenAI GA protocol ──

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
            provider: "azure".into(),
            api_key: "azkey".into(),
            model: "gpt-realtime-deploy".into(),
            voice: Some("alloy".into()),
            endpoint: Some("my-resource".into()),
            ..Default::default()
        }
    }

    fn proto(cfg: &RealtimeConfig) -> AzureProtocol {
        AzureProtocol::from_config(cfg).unwrap()
    }

    /// from_config validates the api key (delegated to OpenAI) — empty ⇒ Err.
    #[test]
    fn from_config_requires_api_key() {
        let cfg = RealtimeConfig {
            api_key: String::new(),
            model: "d".into(),
            endpoint: Some("r".into()),
            ..Default::default()
        };
        assert!(matches!(
            AzureProtocol::from_config(&cfg),
            Err(RealtimeError::AuthenticationFailed(_))
        ));
    }

    /// from_config requires an endpoint/resource (Azure can't pick a host alone).
    #[test]
    fn from_config_requires_endpoint() {
        let cfg = RealtimeConfig {
            api_key: "k".into(),
            model: "d".into(),
            endpoint: None,
            ..Default::default()
        };
        assert!(matches!(
            AzureProtocol::from_config(&cfg),
            Err(RealtimeError::InvalidConfiguration(_))
        ));
    }

    /// from_config requires a deployment name (in `model`).
    #[test]
    fn from_config_requires_deployment() {
        let cfg = RealtimeConfig {
            api_key: "k".into(),
            model: String::new(),
            endpoint: Some("r".into()),
            ..Default::default()
        };
        assert!(matches!(
            AzureProtocol::from_config(&cfg),
            Err(RealtimeError::InvalidConfiguration(_))
        ));
    }

    /// connect_spec: EXACT Azure URL (resource host + api-version + deployment)
    /// and the `api-key` HEADER (NOT Authorization: Bearer).
    #[test]
    fn connect_spec_uses_azure_url_and_api_key_header() {
        let p = proto(&base_cfg());
        let ConnectSpec::WebSocket { url, headers } = p.connect_spec(&base_cfg()).unwrap();
        assert_eq!(
            url,
            "wss://my-resource.openai.azure.com/openai/v1/realtime?deployment=gpt-realtime-deploy"
        );
        // api-key header carries the raw key; NO Authorization header.
        let names: Vec<&str> = headers.iter().map(|(k, _)| k.as_str()).collect();
        assert!(names.contains(&"api-key"));
        assert!(
            !names.iter().any(|n| n.eq_ignore_ascii_case("Authorization")),
            "Azure uses api-key, not Authorization: Bearer"
        );
        let api_key = headers
            .iter()
            .find(|(k, _)| k == "api-key")
            .map(|(_, v)| v.as_str())
            .unwrap();
        assert_eq!(api_key, "azkey");
    }

    /// A full-URL endpoint (scheme + host + path) normalizes to the wss host form.
    #[test]
    fn connect_spec_accepts_full_endpoint_url() {
        let cfg = RealtimeConfig {
            endpoint: Some("https://contoso.openai.azure.com/".into()),
            ..base_cfg()
        };
        let p = proto(&cfg);
        let ConnectSpec::WebSocket { url, .. } = p.connect_spec(&cfg).unwrap();
        assert_eq!(
            url,
            "wss://contoso.openai.azure.com/openai/v1/realtime?deployment=gpt-realtime-deploy"
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
}
