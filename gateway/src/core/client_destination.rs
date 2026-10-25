//! Client-chosen destinations on vendor calls that carry the OPERATOR's credential.
//!
//! A standalone speech leg whose client sent no key of its own uses the operator's vendor key
//! (`ServerConfig::get_api_key`). Most providers also read a destination from the client's
//! `extras`: `endpoint_override` on nearly every STT and TTS provider, `custom_endpoint` (Naver
//! Clova, Murf), `custom_callback_url` (Bhashini), `base_url` / `api_base` (segmented STT), and
//! `use_https: false` (Baidu) downgrades the transport. Each of those passed only the SSRF check,
//! which admits any PUBLIC host, so a key holder could send the operator's Deepgram, Azure, IBM
//! or Huawei credential — passwords and OAuth client secrets included — to a host of their
//! choosing. Bud mode replaces a client's extras with the deployment's (`handlers/ws/bud_legs`);
//! this module is the standalone rule, shared by every route that builds a provider from client
//! config (`/ws`, `POST /transcribe`).
//!
//! The rule: when the call carries the operator's credential, a client-chosen destination is
//! accepted only if the operator approved it —
//!
//! * an exact `host:port` in `WAAV_PRIVATE_EGRESS_ALLOWLIST`;
//! * a loopback target the `WAAV_ALLOW_LOOPBACK_ENDPOINTS` escape hatch permits (local mocks);
//! * a host the operator DECLARED for that provider's key: a credential named after the key's
//!   variable (`DEEPGRAM_API_KEY: {env: DEEPGRAM_API_KEY, hosts: ["proxy.corp:443"]}`), or one
//!   of the implicit vendor bindings (`OPENAI_API_KEY` → `api.openai.com`).
//!
//! A client that sends its own key may point it anywhere (the SSRF rules still apply).

use crate::core::credentials::global_credentials;
use crate::core::net::{loopback_allowance, private_egress_allowlist};

/// `extras` keys whose value is a whole URL a provider dials.
pub const URL_KEYS: &[&str] = &[
    "endpoint_override",
    "custom_endpoint",
    "custom_callback_url",
    "base_url",
    "api_base",
];

/// Providers whose `api_base` is already confined to the vendor's own domains by the provider
/// (Azure Speech: `providers::azure::region` accepts only Microsoft-hosted resource names).
const VENDOR_BOUNDED_API_BASE: &[&str] = &["azure", "microsoft-azure", "microsoft_azure"];

/// The client-chosen destinations in `extras` for `provider`: `(key, value)` pairs.
fn destinations<'a>(
    provider: &str,
    extras: &'a serde_json::Map<String, serde_json::Value>,
) -> Vec<(&'static str, &'a str)> {
    let provider = provider.trim().to_ascii_lowercase();
    URL_KEYS
        .iter()
        .filter(|k| !(**k == "api_base" && VENDOR_BOUNDED_API_BASE.contains(&provider.as_str())))
        .filter_map(|k| {
            extras
                .get(*k)
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(|v| (*k, v))
        })
        .collect()
}

/// Whether the operator approved `url` as a destination for `provider`'s operator key.
pub fn operator_approved(provider: &str, url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    let Some(host) = parsed.host_str() else {
        return false;
    };
    let port = parsed.port_or_known_default();
    if private_egress_allowlist()
        .iter()
        .any(|hp| Some(hp.port) == port && hp.host.eq_ignore_ascii_case(host))
    {
        return true;
    }
    if loopback_allowance().permits(port) && waav_segmented_stt::net::is_loopback_host(host) {
        return true;
    }
    crate::config::provider_key_variable(provider)
        .is_some_and(|var| global_credentials().bind(var, url).is_ok())
}

/// Why a client config may not use these destinations with the operator's key, or `None`.
///
/// `role` names the config block in the message (`stt_config`, `tts_config`, `config`).
pub fn refusal(
    role: &str,
    provider: &str,
    extras: &serde_json::Map<String, serde_json::Value>,
) -> Option<String> {
    for (key, url) in destinations(provider, extras) {
        if !operator_approved(provider, url) {
            let host = url::Url::parse(url)
                .ok()
                .and_then(|u| u.host_str().map(str::to_string))
                .unwrap_or_else(|| url.to_string());
            return Some(format!(
                "{role}.extras.{key} points the gateway's {provider} credential at '{host}', which \
                 the operator has not approved. Send your own api_key to use a destination of \
                 your choice, or ask the operator to allow it (a credential for this provider's \
                 key listing the host, or WAAV_PRIVATE_EGRESS_ALLOWLIST)"
            ));
        }
    }
    if extras.get("use_https").and_then(|v| v.as_bool()) == Some(false) {
        return Some(format!(
            "{role}.extras.use_https=false would send the gateway's {provider} credential without \
             TLS; send your own api_key to do that"
        ));
    }
    None
}

/// Env var: `allow` (default) or `deny`. With `deny`, a standalone gateway refuses every
/// endpoint a client chooses — `conversation_config.base_url` / `reasoning_base_url`, a named
/// operator credential, an inline `dag_config.definition`, and STT/TTS `extras` destinations — as
/// Bud mode does: clients name aliases and server templates, which carry the operator's choices.
pub const CLIENT_ENDPOINTS_ENV: &str = "WAAV_CLIENT_ENDPOINTS";

/// `allow` / `deny`; anything else is refused at parse (the caller fails closed).
pub fn parse_client_endpoints(value: Option<&str>) -> Result<bool, String> {
    match value.map(|v| v.trim().to_ascii_lowercase()) {
        None => Ok(false),
        Some(v) if v.is_empty() || v == "allow" => Ok(false),
        Some(v) if v == "deny" => Ok(true),
        Some(v) => Err(format!(
            "{CLIENT_ENDPOINTS_ENV} must be allow or deny, got '{v}'"
        )),
    }
}

/// Whether client-chosen endpoints are refused (`WAAV_CLIENT_ENDPOINTS=deny`). Read once; a
/// malformed value DENIES (a security switch an operator set must not silently mean "allow").
pub fn client_endpoints_denied() -> bool {
    static DENIED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *DENIED.get_or_init(|| {
        match parse_client_endpoints(std::env::var(CLIENT_ENDPOINTS_ENV).ok().as_deref()) {
            Ok(denied) => denied,
            Err(e) => {
                tracing::error!(error = %e, "refusing client-chosen endpoints (fail closed)");
                true
            }
        }
    })
}

/// The client-chosen destinations in a leg's `extras`, whoever's key it carries: what
/// `WAAV_CLIENT_ENDPOINTS=deny` refuses.
pub fn any_destination(
    role: &str,
    provider: &str,
    extras: &serde_json::Map<String, serde_json::Value>,
) -> Option<String> {
    destinations(provider, extras).first().map(|(key, _)| {
        format!(
            "{role}.extras.{key} is not accepted by this gateway: it does not let clients choose \
             endpoints ({CLIENT_ENDPOINTS_ENV}=deny). Use an alias, whose endpoints the operator \
             configured"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn extras(v: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
        v.as_object().cloned().unwrap()
    }

    #[test]
    fn a_public_override_with_the_operator_key_is_refused_on_every_url_key() {
        for key in URL_KEYS {
            let e = extras(json!({ *key: "wss://attacker.example/listen" }));
            let msg = refusal("stt_config", "deepgram", &e).unwrap_or_default();
            assert!(
                msg.contains(key) && msg.contains("attacker.example"),
                "{key}: {msg}"
            );
        }
    }

    #[test]
    fn the_vendor_own_host_is_approved_through_its_implicit_binding() {
        // OPENAI_API_KEY is implicitly bound to api.openai.com.
        let e = extras(json!({ "endpoint_override": "https://api.openai.com/v1" }));
        assert_eq!(refusal("stt_config", "openai", &e), None);
        let e = extras(json!({ "base_url": "https://evil.example/v1" }));
        assert!(refusal("stt_config", "openai", &e).is_some());
    }

    #[test]
    fn azure_api_base_is_left_to_the_provider_which_confines_it_to_microsoft() {
        let e = extras(json!({ "api_base": "https://my-res.cognitiveservices.azure.com" }));
        assert_eq!(refusal("tts_config", "azure", &e), None);
        let e = extras(json!({ "endpoint_override": "https://attacker.example" }));
        assert!(refusal("tts_config", "azure", &e).is_some());
    }

    #[test]
    fn a_transport_downgrade_is_refused_and_unrelated_extras_pass() {
        let e = extras(json!({ "use_https": false }));
        assert!(refusal("tts_config", "baidu", &e).is_some());
        let e = extras(
            json!({ "use_https": true, "region": "westeurope", "callback": "https://x.example" }),
        );
        assert_eq!(refusal("tts_config", "baidu", &e), None);
        assert_eq!(
            refusal("tts_config", "deepgram", &serde_json::Map::new()),
            None
        );
    }

    #[test]
    fn the_client_endpoint_switch_parses_strictly() {
        assert_eq!(parse_client_endpoints(None), Ok(false));
        assert_eq!(parse_client_endpoints(Some("")), Ok(false));
        assert_eq!(parse_client_endpoints(Some("allow")), Ok(false));
        assert_eq!(parse_client_endpoints(Some(" DENY ")), Ok(true));
        assert!(parse_client_endpoints(Some("off")).is_err());
    }

    #[test]
    fn any_destination_names_the_field_whoever_owns_the_key() {
        let e = extras(json!({ "endpoint_override": "https://api.openai.com" }));
        let msg = any_destination("stt_config", "openai", &e).unwrap();
        assert!(msg.contains("stt_config.extras.endpoint_override"), "{msg}");
        assert_eq!(
            any_destination("stt_config", "openai", &serde_json::Map::new()),
            None
        );
    }

    #[test]
    fn an_unknown_provider_has_no_operator_binding_to_approve_with() {
        assert!(!operator_approved(
            "no-such-vendor",
            "https://api.example.com"
        ));
        assert!(!operator_approved("deepgram", "not a url"));
    }
}
