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

/// The environment variables a provider's OPERATOR credential is read from: the one
/// `ServerConfig::get_api_key` reads, and those providers read themselves when a leg has no key
/// (Murf's `MURF_API_KEY`, Tinkoff's key pair, …). A credential declared under any of these names
/// approves its hosts for that provider. Provider names are compared with `-` and `.` as `_`.
pub fn operator_key_variables(provider: &str) -> Vec<&'static str> {
    let mut vars: Vec<&'static str> = crate::config::provider_key_variable(provider)
        .into_iter()
        .collect();
    let normalized = provider
        .trim()
        .to_ascii_lowercase()
        .replace(['-', '.'], "_");
    let own: &[&'static str] = match normalized.as_str() {
        "alibaba_cloud" | "alibaba" | "dashscope" => &["DASHSCOPE_API_KEY"],
        "amivoice" => &["AMIVOICE_APPKEY"],
        "fpt_ai" | "fpt" => &["FPT_AI_API_KEY"],
        "gladia" => &["GLADIA_API_KEY"],
        "gnani" | "gnani_ai" | "vachana" => &["GNANI_TOKEN", "GNANI_ACCESS_KEY"],
        "nectec" => &["NECTEC_API_KEY"],
        "reverie" => &["REVERIE_APP_ID"],
        "sarvam" => &["SARVAM_API_KEY"],
        "speechmatics" => &["SPEECHMATICS_API_KEY"],
        "tinkoff" | "tinkoff_voicekit" => &[
            "TINKOFF_API_KEY",
            "TINKOFF_SECRET_KEY",
            "TINKOFF_VOICEKIT_API_KEY",
            "TINKOFF_VOICEKIT_SECRET_KEY",
        ],
        "acapela" => &["ACAPELA_API_KEY"],
        "murf" => &["MURF_API_KEY"],
        "resemble" => &["RESEMBLE_API_KEY"],
        "sberdevices" | "salute" | "salutespeech" | "smartspeech" => &[
            "SBERDEVICES_API_KEY",
            "SALUTE_SPEECH_API_KEY",
            "SMARTSPEECH_API_KEY",
        ],
        "speechify" => &["SPEECHIFY_API_KEY"],
        "unrealspeech" | "unreal_speech" => &["UNREALSPEECH_API_KEY"],
        "wellsaid" => &["WELLSAID_API_KEY"],
        "zalo_ai" | "zalo" => &["ZALO_API_KEY"],
        _ => &[],
    };
    for var in own {
        if !vars.contains(var) {
            vars.push(var);
        }
    }
    vars
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
    let registry = global_credentials();
    operator_key_variables(provider)
        .into_iter()
        .any(|var| registry.bind(var, url).is_ok())
}

/// Whether the client brought a credential of its own for this leg: `api_key`, or one in `extras`
/// that providers use in its place (`access_token` for Google and Cartesia, an AWS key pair for
/// Transcribe and Polly, Gnani's token and access key). Without one, the leg may carry the
/// OPERATOR's credential — whether the server holds it in its config or the provider reads it
/// from the environment itself, as about thirty providers do — so its destination is judged.
pub fn client_supplies_credential(
    client_api_key: Option<&str>,
    extras: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    let set = |k: &str| {
        extras
            .get(k)
            .and_then(|v| v.as_str())
            .is_some_and(|v| !v.trim().is_empty())
    };
    client_api_key.is_some_and(|k| !k.trim().is_empty())
        || set("access_token")
        || (set("aws_access_key_id") && set("aws_secret_access_key"))
        || (set("token") && set("access_key"))
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
                "{role}.extras.{key} points this {provider} leg at '{host}', which the operator \
                 has not approved for the gateway's {provider} credential (a leg without a \
                 credential of the client's own uses the gateway's). Send your own api_key to \
                 choose a destination, or ask the operator to allow the host (a credential for \
                 this provider's key listing it, or WAAV_PRIVATE_EGRESS_ALLOWLIST)"
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
    fn a_leg_is_judged_unless_the_client_brought_its_own_credential() {
        let none = serde_json::Map::new();
        assert!(!client_supplies_credential(None, &none));
        assert!(!client_supplies_credential(Some("  "), &none));
        assert!(client_supplies_credential(Some("dg-own"), &none));
        let token = extras(json!({ "access_token": "ya29.client" }));
        assert!(client_supplies_credential(None, &token));
        let aws = extras(json!({ "aws_access_key_id": "AKIA", "aws_secret_access_key": "s" }));
        assert!(client_supplies_credential(None, &aws));
        let half = extras(json!({ "aws_access_key_id": "AKIA" }));
        assert!(
            !client_supplies_credential(None, &half),
            "half a key pair is not the client's credential"
        );
        let gnani = extras(json!({ "token": "t", "access_key": "k" }));
        assert!(client_supplies_credential(None, &gnani));
    }

    /// About thirty providers read their key from the environment themselves (Murf's
    /// `MURF_API_KEY`), so the gateway's config holding no key does not mean no key is sent.
    #[test]
    fn providers_that_read_their_own_key_are_judged_and_approvable_by_that_name() {
        let e = extras(json!({ "endpoint_override": "https://attacker.example" }));
        let msg = refusal("tts_config", "murf", &e).expect("refused");
        assert!(
            msg.contains("murf") && msg.contains("attacker.example"),
            "{msg}"
        );
        assert_eq!(operator_key_variables("murf"), vec!["MURF_API_KEY"]);
        assert_eq!(
            operator_key_variables("tinkoff"),
            vec![
                "TINKOFF_API_KEY",
                "TINKOFF_SECRET_KEY",
                "TINKOFF_VOICEKIT_API_KEY",
                "TINKOFF_VOICEKIT_SECRET_KEY"
            ]
        );
        assert_eq!(operator_key_variables("deepgram"), vec!["DEEPGRAM_API_KEY"]);
        assert_eq!(
            operator_key_variables("gnani"),
            vec!["GNANI_TOKEN", "GNANI_ACCESS_KEY"]
        );
        assert!(operator_key_variables("fpt-ai").contains(&"FPT_AI_API_KEY"));
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
