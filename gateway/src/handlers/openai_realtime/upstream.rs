//! The vendor leg of a relayed session (FRD-023 §5.3, FR-CRED-2/3, S-7).
//!
//! The URL, the auth header and the model come from the deployment's `voice_table` entry and from
//! nowhere else: not the client, not WaaV's environment (D-5). A client-controlled host on this
//! path is exactly the bug that made Helicone remove realtime (S-7), so the only host that is not
//! a vendor constant — an `api_base` — is SSRF-validated before it is dialled.

use std::time::Duration;

use bud_auth::VoiceEndpoint;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

/// The relay-capable vendors (D-2): they speak OpenAI Realtime GA themselves. xAI joined in RT7
/// (WP-RT7.3); the vendors without a GA surface are served by the translate engine
/// ([`super::facade`]).
pub const RELAY_VENDORS: &[&str] = &["openai", "azure_openai", "grok"];

const OPENAI_DEFAULT_BASE: &str = "https://api.openai.com/v1";
/// xAI's GA-compatible realtime surface (CONTRACTS C7): `wss://api.x.ai/v1/realtime?model=…`.
const XAI_DEFAULT_BASE: &str = "https://api.x.ai/v1";

pub type UpstreamSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Why the vendor leg could not be built or opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpstreamError {
    /// The deployment's vendor is not served by the relay (yet).
    UnsupportedVendor(String),
    /// The deployment carries no usable credential.
    MissingCredential,
    /// The deployment names no vendor model.
    MissingModel,
    /// An Azure deployment without its resource endpoint.
    MissingApiBase,
    /// The `api_base` is not a URL, or failed SSRF validation.
    InvalidApiBase(String),
    /// Connecting or upgrading failed.
    Connect(String),
    /// No answer within the connect deadline.
    Timeout,
}

impl std::fmt::Display for UpstreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedVendor(v) => {
                write!(f, "realtime sessions are not served for vendor '{v}'")
            }
            Self::MissingCredential => write!(f, "the deployment has no usable vendor credential"),
            Self::MissingModel => write!(f, "the deployment names no vendor model"),
            Self::MissingApiBase => write!(
                f,
                "an Azure OpenAI deployment needs its resource endpoint (api_base)"
            ),
            Self::InvalidApiBase(why) => write!(f, "the deployment's api_base was refused: {why}"),
            Self::Connect(why) => write!(f, "could not connect to the vendor: {why}"),
            Self::Timeout => write!(f, "the vendor did not answer within the connect deadline"),
        }
    }
}

/// A built vendor request. `Debug` never prints header values: one of them is the credential.
#[derive(Clone)]
pub struct UpstreamRequest {
    pub url: String,
    pub headers: Vec<(&'static str, String)>,
    /// The host an SSRF check must clear, when the URL is not a vendor constant.
    pub needs_ssrf_check: bool,
}

impl std::fmt::Debug for UpstreamRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpstreamRequest")
            .field("url", &self.url)
            .field(
                "headers",
                &self.headers.iter().map(|(k, _)| *k).collect::<Vec<_>>(),
            )
            .finish()
    }
}

/// `https://…` → `wss://…`, `http://…` → `ws://…`; `ws(s)` kept. Trailing slashes trimmed.
fn to_ws_base(base: &str) -> Result<String, UpstreamError> {
    let base = base.trim().trim_end_matches('/');
    let converted = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else if base.starts_with("wss://") || base.starts_with("ws://") {
        base.to_string()
    } else {
        return Err(UpstreamError::InvalidApiBase(
            "api_base must be an http(s) or ws(s) URL".into(),
        ));
    };
    url::Url::parse(&converted).map_err(|e| UpstreamError::InvalidApiBase(e.to_string()))?;
    Ok(converted)
}

fn query_value(v: &str) -> String {
    url::form_urlencoded::byte_serialize(v.as_bytes()).collect()
}

/// Build the vendor request for a deployment (FRD §5.3 table).
///
/// A transcription deployment connects with `?intent=transcription`; its model is set by the
/// defaults `session.update` (`audio.input.transcription.model`).
pub fn build(
    endpoint: &VoiceEndpoint,
    transcription: bool,
) -> Result<UpstreamRequest, UpstreamError> {
    let vendor = endpoint.vendor.trim().to_ascii_lowercase();
    let credential = endpoint
        .credential
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .ok_or(UpstreamError::MissingCredential)?
        .to_string();
    let model = endpoint
        .model
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty());
    let query = |model: Option<&str>| -> Result<String, UpstreamError> {
        if transcription {
            Ok("intent=transcription".to_string())
        } else {
            Ok(format!(
                "model={}",
                query_value(model.ok_or(UpstreamError::MissingModel)?)
            ))
        }
    };

    match vendor.as_str() {
        // xAI speaks GA on its own host (WP-RT7.3): Bearer, the vendor model in the query. Its
        // quirks are on the event stream, not the handshake (`policy::vendor_event`).
        "openai" | "grok" => {
            let default_base = if vendor == "grok" {
                XAI_DEFAULT_BASE
            } else {
                OPENAI_DEFAULT_BASE
            };
            let api_base = endpoint
                .api_base
                .as_deref()
                .filter(|b| !b.trim().is_empty());
            let base = to_ws_base(api_base.unwrap_or(default_base))?;
            Ok(UpstreamRequest {
                url: format!("{base}/realtime?{}", query(model)?),
                headers: vec![("authorization", format!("Bearer {credential}"))],
                needs_ssrf_check: api_base.is_some(),
            })
        }
        "azure_openai" => {
            let api_base = endpoint
                .api_base
                .as_deref()
                .filter(|b| !b.trim().is_empty())
                .ok_or(UpstreamError::MissingApiBase)?;
            let parsed = url::Url::parse(api_base.trim())
                .map_err(|e| UpstreamError::InvalidApiBase(e.to_string()))?;
            let host = parsed
                .host_str()
                .ok_or_else(|| UpstreamError::InvalidApiBase("api_base has no host".into()))?;
            let authority = match parsed.port() {
                Some(port) => format!("{host}:{port}"),
                None => host.to_string(),
            };
            let scheme = if parsed.scheme() == "http" {
                "ws"
            } else {
                "wss"
            };
            // GA: no `api-version` — the v1 URL answers 401 with one (FRD §5.3). The Azure
            // deployment name is the `model`.
            Ok(UpstreamRequest {
                url: format!(
                    "{scheme}://{authority}/openai/v1/realtime?{}",
                    query(model)?
                ),
                headers: vec![("api-key", credential)],
                needs_ssrf_check: true,
            })
        }
        other => Err(UpstreamError::UnsupportedVendor(other.to_string())),
    }
}

/// SSRF-validate the request's host (blocking DNS, so off the async workers).
pub async fn validate(req: &UpstreamRequest) -> Result<(), UpstreamError> {
    if !req.needs_ssrf_check {
        return Ok(());
    }
    let url = req.url.clone();
    tokio::task::spawn_blocking(move || {
        crate::core::net::validate_url_for_ssrf(&url, &["ws", "wss"])
    })
    .await
    .map_err(|e| UpstreamError::InvalidApiBase(format!("validation task failed: {e}")))?
    .map_err(UpstreamError::InvalidApiBase)
}

/// Open the vendor socket within `deadline`. No extensions are offered (no permessage-deflate:
/// frames are parsed, S-8).
pub async fn connect(
    req: &UpstreamRequest,
    deadline: Duration,
) -> Result<UpstreamSocket, UpstreamError> {
    let mut request = req
        .url
        .as_str()
        .into_client_request()
        .map_err(|e| UpstreamError::Connect(e.to_string()))?;
    for (name, value) in &req.headers {
        let value = value
            .parse()
            .map_err(|_| UpstreamError::Connect(format!("header {name} is not a valid value")))?;
        request.headers_mut().insert(*name, value);
    }
    let config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
        .max_message_size(Some(super::handshake::MAX_MESSAGE_BYTES))
        .max_frame_size(Some(super::handshake::MAX_MESSAGE_BYTES));
    match tokio::time::timeout(
        deadline,
        tokio_tungstenite::connect_async_with_config(request, Some(config), true),
    )
    .await
    {
        Ok(Ok((socket, _response))) => Ok(socket),
        // The error text can carry the URL; never the headers.
        Ok(Err(e)) => Err(UpstreamError::Connect(e.to_string())),
        Err(_) => Err(UpstreamError::Timeout),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn endpoint(vendor: &str, api_base: Option<&str>, model: Option<&str>) -> VoiceEndpoint {
        let mut entry = serde_json::json!({"vendor": vendor, "endpoints": ["realtime_session"]});
        if let Some(b) = api_base {
            entry["api_base"] = b.into();
        }
        if let Some(m) = model {
            entry["model"] = m.into();
        }
        let blob = serde_json::json!({ "ep": entry }).to_string();
        let mut ep = bud_auth::credentials::parse_voice_blob(
            &blob,
            &bud_auth::CredentialDecryptor::disabled(),
        )
        .unwrap()
        .remove("ep")
        .unwrap();
        ep.credential = Some("sk-vendor".into());
        ep
    }

    /// TC-UP-01 — OpenAI: GA URL, Bearer, no beta header.
    #[test]
    fn tc_up_01_openai_url_and_auth() {
        let req = build(&endpoint("openai", None, Some("gpt-realtime-2.1")), false).unwrap();
        assert_eq!(
            req.url,
            "wss://api.openai.com/v1/realtime?model=gpt-realtime-2.1"
        );
        assert_eq!(
            req.headers,
            vec![("authorization", "Bearer sk-vendor".to_string())]
        );
        assert!(
            !req.needs_ssrf_check,
            "the vendor constant needs no SSRF check"
        );
        assert!(
            !req.headers
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("openai-beta"))
        );
    }

    /// TC-UP-02 🔒 — Azure GA URL: `/openai/v1/realtime`, `api-key`, and NO `api-version`.
    #[test]
    fn tc_up_02_azure_ga_url_has_no_api_version() {
        let req = build(
            &endpoint(
                "azure_openai",
                Some("https://r.openai.azure.com/"),
                Some("rt-prod"),
            ),
            false,
        )
        .unwrap();
        assert_eq!(
            req.url,
            "wss://r.openai.azure.com/openai/v1/realtime?model=rt-prod"
        );
        assert!(!req.url.contains("api-version"));
        assert_eq!(req.headers, vec![("api-key", "sk-vendor".to_string())]);
        assert!(req.needs_ssrf_check);
    }

    /// TC-XL-06 (unit half) — xAI is relayed: its GA endpoint, the vendor model verbatim, Bearer,
    /// and no SSRF check for the vendor constant; an `api_base` is validated like OpenAI's.
    #[test]
    fn tc_xl_06_xai_url_and_bearer() {
        let req = build(&endpoint("grok", None, Some("grok-voice-2")), false).unwrap();
        assert_eq!(req.url, "wss://api.x.ai/v1/realtime?model=grok-voice-2");
        assert_eq!(
            req.headers,
            vec![("authorization", "Bearer sk-vendor".to_string())]
        );
        assert!(!req.needs_ssrf_check);
        let req = build(
            &endpoint("grok", Some("https://xai-proxy.example/v1"), Some("m")),
            false,
        )
        .unwrap();
        assert_eq!(req.url, "wss://xai-proxy.example/v1/realtime?model=m");
        assert!(req.needs_ssrf_check);
    }

    /// TC-UP-03 — an `api_base` has its scheme converted and keeps its path.
    #[test]
    fn tc_up_03_api_base_scheme_conversion() {
        let req = build(
            &endpoint(
                "openai",
                Some("https://proxy.example/v1"),
                Some("gpt-realtime-2.1"),
            ),
            false,
        )
        .unwrap();
        assert_eq!(
            req.url,
            "wss://proxy.example/v1/realtime?model=gpt-realtime-2.1"
        );
        assert!(req.needs_ssrf_check);
    }

    /// TC-UP-04 🔒 — a private `api_base` is refused before any connect.
    #[tokio::test]
    async fn tc_up_04_ssrf_refuses_private_hosts() {
        for base in [
            "http://10.0.0.5",
            "http://169.254.169.254/v1",
            "http://localhost:9000",
        ] {
            let req = build(&endpoint("openai", Some(base), Some("m")), false).unwrap();
            let err = validate(&req).await.expect_err(base);
            assert!(
                matches!(err, UpstreamError::InvalidApiBase(_)),
                "{base}: {err:?}"
            );
        }
    }

    /// TC-UP-05 — the vendor model comes from `voice_table`, never the client.
    #[test]
    fn tc_up_05_model_is_the_deployments() {
        let req = build(&endpoint("openai", None, Some("gpt-realtime-2.1")), false).unwrap();
        assert!(req.url.ends_with("model=gpt-realtime-2.1"));
    }

    #[test]
    fn a_transcription_deployment_connects_with_the_transcription_intent() {
        let req = build(&endpoint("openai", None, Some("gpt-4o-transcribe")), true).unwrap();
        assert_eq!(
            req.url,
            "wss://api.openai.com/v1/realtime?intent=transcription"
        );
    }

    #[test]
    fn missing_pieces_are_named() {
        let mut ep = endpoint("openai", None, Some("m"));
        ep.credential = None;
        assert_eq!(
            build(&ep, false).unwrap_err(),
            UpstreamError::MissingCredential
        );
        assert_eq!(
            build(&endpoint("openai", None, None), false).unwrap_err(),
            UpstreamError::MissingModel
        );
        assert_eq!(
            build(&endpoint("azure_openai", None, Some("d")), false).unwrap_err(),
            UpstreamError::MissingApiBase
        );
        assert!(matches!(
            build(&endpoint("gemini", None, Some("m")), false).unwrap_err(),
            UpstreamError::UnsupportedVendor(_)
        ));
    }

    /// TC-SEC-07 (unit half): the vendor credential cannot reach a log through `Debug`.
    #[test]
    fn the_request_debug_never_prints_the_credential() {
        let req = build(&endpoint("openai", None, Some("m")), false).unwrap();
        let printed = format!("{req:?}");
        assert!(!printed.contains("sk-vendor"), "{printed}");
    }
}
