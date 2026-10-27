//! `/v1/realtime` on a standalone gateway (no Bud control plane), relayed to the vendor a server
//! alias's `realtime` block names.
//!
//! On the Bud path a `realtime_session` deployment's `voice_table` entry says which vendor, model,
//! address and credential a session reaches. A standalone gateway has no `voice_table`; its
//! operator's equivalent is a server alias marked `openai_compatible: true` with a `realtime`
//! block. This module turns that block into the same [`VoiceEndpoint`] value the Bud path reads,
//! and from there the session is the Bud path's own: [`session::engage`] builds the relay request
//! (`upstream::build`) or the translate plan (`facade::TranslatePlan::build`), and the relay and
//! facade loops run it unchanged. Constructing the `VoiceEndpoint` is the seam because every
//! later stage — the builders, the defaults `session.update`, the client policy, the metering
//! attribution — already reads exactly that value; a second, alias-shaped input to each of them
//! would be a fork of all of them. `AppState::resolve_alias_voice_endpoint` makes the same choice
//! for `/v1/audio/*`.
//!
//! **The key** follows the rule every alias leg follows ([`alias_leg_key`]), with no client key
//! (the handshake's bearer is the gateway's key, never the vendor's):
//!
//! * no `base_url`: the vendor's own host (a constant of the relay or of the facade's provider)
//!   with the gateway's realtime key for that vendor (`ServerConfig::get_realtime_api_key`) — the
//!   key goes only to the host it was issued for;
//! * a `base_url`: the alias's `credential_ref`, which must be declared for that host; else the
//!   gateway's key only where the operator approved that host for it
//!   (`core::client_destination::operator_approved`); else the session is refused, since every
//!   realtime vendor needs a key.
//!
//! Both are checked at startup ([`validate`]) as well as per session, so a mistake stops the
//! server rather than failing every session.
//!
//! **The caller** is [`CallerCheck::Standalone`]: no Bud principal, so no Bud-only path (client
//! secret minting, Bud revalidation, a Bud `/ws` session) can be reached with it, and revalidation
//! ([`still_served`]) re-checks that the alias still relays.

use std::collections::BTreeMap;

use axum::http::{HeaderMap, StatusCode};
use bud_auth::{
    AliasMetadata, Principal, PrincipalKind, RealtimeDefaults, RealtimeSettings, VoiceEndpoint,
    VoiceEndpointSettings,
};
use tracing::{debug, warn};

use crate::core::alias::{AliasDefinition, AliasLegEndpoint, AliasRealtime, alias_leg_key};
use crate::state::AppState;

use super::cascade::{self, StandaloneAlias};
use super::handshake::{HandshakeError, REALTIME_CAPABILITY};
use super::session::{self, Caller, CallerCheck, Prepared, Session};
use super::{facade, upstream};

/// Vendors the relay serves on the Bud path whose leg an alias cannot describe, and why.
const UNSUPPORTED: &[(&str, &str)] = &[(
    "nova_sonic",
    "Nova 2 Sonic signs its requests with an AWS key pair in a region, which an alias's single \
     credential cannot carry (and the gateway's own AWS identity is never used for it)",
)];

/// Stands in for the key when [`validate`] builds the vendor request at startup: the builders
/// need one, and nothing is dialled.
const VALIDATION_KEY: &str = "alias-validation-placeholder";

/// The standalone `/v1/realtime` handshake: authenticate, resolve `?model` to an
/// `openai_compatible` alias, then relay it to its `realtime` vendor when it has one, else run its
/// stt/llm/tts legs in the conversation loop. A `realtime` block wins over legs on this route; the
/// legs keep serving `/v1/audio/*` and `/ws`.
pub async fn prepare_standalone(
    state: &AppState,
    query: Option<&str>,
    headers: &HeaderMap,
    path: &str,
) -> Result<Session, HandshakeError> {
    let alias = cascade::resolve_alias(state, query, headers, path).await?;
    if alias.def.realtime.is_some() {
        prepare(state, alias)
            .await
            .map(|p| Session::Vendor(Box::new(p)))
    } else {
        cascade::prepare_alias(alias).map(|p| Session::Cascade(Box::new(p)))
    }
}

/// The session id the relay's breakers, admission, logs and metering know an alias by (the same
/// `alias:<name>` `/v1/audio/*` uses).
fn endpoint_id(alias: &str) -> String {
    format!("alias:{alias}")
}

/// A standalone alias's `realtime` block as a session (the Bud path's [`session::engage`]).
pub async fn prepare(state: &AppState, alias: StandaloneAlias) -> Result<Prepared, HandshakeError> {
    let StandaloneAlias { name, def, auth } = alias;
    let Some(leg) = def.realtime.as_ref() else {
        return Err(misconfigured(&name));
    };
    let vendor = vendor_of(leg).map_err(|why| {
        warn!(alias = %name, error = %why, "alias realtime leg refused");
        misconfigured(&name)
    })?;
    let key = leg_key(leg, &vendor, || {
        state.config.get_realtime_api_key(key_provider(&vendor))
    })
    .map_err(|why| {
        // The reason names hosts and credentials, which stay server-side: the log has it.
        warn!(alias = %name, vendor = %vendor, error = %why, "alias realtime leg has no usable key");
        HandshakeError::new(
            StatusCode::BAD_GATEWAY,
            "deployment_misconfigured",
            format!(
                "Alias '{name}' has no usable credential for its realtime vendor; the gateway's \
                 log says why."
            ),
        )
    })?;
    debug!(alias = %name, vendor = %vendor, auth_id = ?auth.id, "standalone realtime alias resolved");
    let caller = Caller {
        principal: Principal {
            project_id: None,
            api_key_id: None,
            user_id: None,
            via: PrincipalKind::ApiKey,
            expires_at: None,
        },
        check: CallerCheck::Standalone {
            alias: name.clone(),
        },
        via_client_secret: false,
    };
    let endpoint = voice_endpoint(leg, &vendor, key);
    session::engage(state, caller, endpoint_id(&name), name, endpoint, None).await
}

fn misconfigured(alias: &str) -> HandshakeError {
    HandshakeError::new(
        StatusCode::BAD_GATEWAY,
        "deployment_misconfigured",
        format!("Alias '{alias}' has no realtime vendor this gateway can serve."),
    )
}

/// A standalone session's revalidation (D-17): the alias still relays. `None` on a Bud gateway,
/// which never prepares a standalone caller, so a stray one is revoked rather than trusted.
pub fn still_served(state: &AppState, alias: &str) -> Option<AliasMetadata> {
    if state.bud_mode.is_some() {
        return None;
    }
    crate::core::alias::global_aliases()
        .resolve(alias)
        .filter(|d| d.openai_compatible && d.realtime.is_some())
        .map(|_| AliasMetadata::default())
}

fn nonempty(v: Option<&str>) -> Option<String> {
    v.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// The leg's vendor, lower-cased, if the relay or the facade serves it.
fn vendor_of(leg: &AliasRealtime) -> Result<String, String> {
    let vendor = nonempty(leg.provider.as_deref())
        .ok_or_else(|| "realtime.provider is required (the vendor to relay to)".to_string())?
        .to_ascii_lowercase();
    if let Some((_, why)) = UNSUPPORTED.iter().find(|(v, _)| *v == vendor) {
        return Err(format!(
            "realtime.provider {vendor} is not supported here: {why}"
        ));
    }
    if upstream::RELAY_VENDORS.contains(&vendor.as_str()) || facade::is_translate_vendor(&vendor) {
        return Ok(vendor);
    }
    let supported: Vec<&str> = upstream::RELAY_VENDORS
        .iter()
        .copied()
        .chain(facade::translate_vendors())
        .filter(|v| !UNSUPPORTED.iter().any(|(u, _)| u == v))
        .collect();
    Err(format!(
        "realtime.provider {vendor:?} is not a realtime vendor; one of: {}",
        supported.join(", ")
    ))
}

/// The name the gateway keeps its own key for `vendor` under (`get_realtime_api_key`), and that
/// the egress rules judge a destination for that key by (`operator_approved`).
fn key_provider(vendor: &str) -> &str {
    facade::translate_provider(vendor).unwrap_or(vendor)
}

/// The key the vendor leg sends (the module's rule). `operator_key` is the gateway's own key for
/// the vendor; it is read only when the rule allows sending it.
pub fn leg_key(
    leg: &AliasRealtime,
    vendor: &str,
    operator_key: impl FnOnce() -> Option<String>,
) -> Result<String, String> {
    let Some(base_url) = nonempty(leg.base_url.as_deref()) else {
        if leg.credential_ref.is_some() {
            return Err(credential_ref_needs_base_url());
        }
        // The vendor's own host: the gateway's key for it goes there and nowhere else.
        return operator_key()
            .filter(|k| !k.trim().is_empty())
            .ok_or_else(|| {
                format!(
                    "the gateway holds no {vendor} realtime key; configure it, or give the leg a \
                     base_url and a credential_ref"
                )
            });
    };
    let endpoint = AliasLegEndpoint {
        provider: key_provider(vendor).to_string(),
        base_url,
        credential_ref: leg.credential_ref.clone(),
    };
    let key = alias_leg_key(&endpoint, None, operator_key)?;
    if !key.key.is_empty() {
        return Ok(key.key);
    }
    Err(if key.withheld {
        not_approved(vendor, &endpoint.base_url)
    } else {
        format!(
            "no credential is sent to {}: name a credential_ref declared for it",
            endpoint.base_url
        )
    })
}

fn credential_ref_needs_base_url() -> String {
    "realtime.credential_ref needs realtime.base_url: a named credential is bound to the hosts it \
     may be sent to, so name the one it goes to"
        .to_string()
}

fn not_approved(vendor: &str, base_url: &str) -> String {
    format!(
        "realtime.base_url {base_url} is not approved for the gateway's {vendor} key: name a \
         credential_ref declared for that host, or approve the host for the key (a credential \
         named after its variable listing the host, or WAAV_PRIVATE_EGRESS_ALLOWLIST)"
    )
}

/// The block as the `voice_table` entry a Bud deployment would publish for it.
fn voice_endpoint(leg: &AliasRealtime, vendor: &str, key: String) -> VoiceEndpoint {
    let voice = nonempty(leg.voice.as_deref());
    let instructions = nonempty(leg.instructions.as_deref());
    let defaults = (voice.is_some() || instructions.is_some()).then(|| RealtimeDefaults {
        voice: voice.clone(),
        instructions,
        ..Default::default()
    });
    VoiceEndpoint {
        vendor: vendor.to_string(),
        api_base: nonempty(leg.base_url.as_deref()),
        credential: Some(key),
        credential_parts: None,
        endpoints: vec![REALTIME_CAPABILITY.to_string()],
        model: nonempty(leg.model.as_deref()),
        voice,
        language: None,
        config: VoiceEndpointSettings {
            realtime: Some(RealtimeSettings {
                session_type: Some("realtime".to_string()),
                defaults,
                limits: None,
                policy: None,
            }),
            ..Default::default()
        },
        pricing: None,
        provider_params: BTreeMap::new(),
        policy: Default::default(),
    }
}

/// Startup checks for an alias's `realtime` block (`validate_aliases`, `waav-gateway check`): the
/// alias is reachable on `/v1/realtime`, the vendor is one the relay or the facade serves, the
/// model is named, a `base_url` passes the egress rules, the key rule can send a key to it, and
/// the vendor request builds — the same builder the session runs, with a placeholder key, never
/// dialled. Needs the operator's credentials loaded (`initialize_credentials`).
pub fn validate(name: &str, def: &AliasDefinition, leg: &AliasRealtime) -> Result<(), String> {
    let fail = |why: String| format!("alias '{name}': {why}");
    if !def.openai_compatible {
        return Err(fail(
            "a realtime block is served only by /v1/realtime, which resolves aliases marked \
             openai_compatible: true; set it"
                .to_string(),
        ));
    }
    let vendor = vendor_of(leg).map_err(fail)?;
    if nonempty(leg.model.as_deref()).is_none() {
        return Err(fail(format!(
            "realtime.model is required (the {vendor} model the session runs)"
        )));
    }
    match nonempty(leg.base_url.as_deref()) {
        None if leg.credential_ref.is_some() => return Err(fail(credential_ref_needs_base_url())),
        None => {}
        Some(base) => {
            crate::core::net::validate_url_for_ssrf(&base, crate::core::net::HTTP_WS_URL_SCHEMES)
                .map_err(|e| {
                fail(format!(
                    "realtime.base_url {base:?} is not allowed: {e}. A private address needs \
                         WAAV_PRIVATE_EGRESS_ALLOWLIST (host:port)"
                ))
            })?;
            // The key rule's host decision, without reading a secret: a secret that appears after
            // startup (a mounted file) is read per session.
            match nonempty(leg.credential_ref.as_deref()) {
                Some(credential) => {
                    crate::core::credentials::global_credentials()
                        .bind(&credential, &base)
                        .map_err(|e| fail(format!("realtime.credential_ref: {e}")))?;
                }
                None => {
                    if !crate::core::client_destination::operator_approved(
                        key_provider(&vendor),
                        &base,
                    ) {
                        return Err(fail(not_approved(&vendor, &base)));
                    }
                }
            }
        }
    }
    let endpoint = voice_endpoint(leg, &vendor, VALIDATION_KEY.to_string());
    if facade::is_translate_vendor(&vendor) {
        facade::TranslatePlan::build(&endpoint, endpoint.config.realtime.as_ref())
            .map(drop)
            .map_err(|e| fail(format!("realtime: {e}")))
    } else {
        upstream::build(&endpoint, false)
            .map(drop)
            .map_err(|e| fail(format!("realtime: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leg(yaml: &str) -> AliasRealtime {
        serde_yaml::from_str(yaml).unwrap()
    }

    #[test]
    fn without_a_base_url_the_gateways_vendor_key_goes_to_the_vendors_own_host() {
        let l = leg("{ provider: openai, model: gpt-realtime }");
        assert_eq!(
            leg_key(&l, "openai", || Some("sk-op".into())).unwrap(),
            "sk-op"
        );
        assert!(
            leg_key(&l, "openai", || None).is_err(),
            "no key, no session"
        );
        let ep = voice_endpoint(&l, "openai", "sk-op".into());
        let req = upstream::build(&ep, false).unwrap();
        assert_eq!(
            req.url,
            "wss://api.openai.com/v1/realtime?model=gpt-realtime"
        );
    }

    #[test]
    fn a_base_url_gets_the_gateways_key_only_where_the_operator_approved_it() {
        // OPENAI_API_KEY's implicit binding: api.openai.com.
        let approved =
            leg(r#"{ provider: openai, model: m, base_url: "https://api.openai.com/v1" }"#);
        assert_eq!(
            leg_key(&approved, "openai", || Some("sk-op".into())).unwrap(),
            "sk-op"
        );
        let elsewhere =
            leg(r#"{ provider: openai, model: m, base_url: "https://proxy.example.com/v1" }"#);
        let e = leg_key(&elsewhere, "openai", || Some("sk-op".into())).unwrap_err();
        assert!(e.contains("not approved"), "{e}");
        assert!(!e.contains("sk-op"), "never the key: {e}");
    }

    #[test]
    fn a_credential_ref_is_bound_to_its_hosts() {
        let bound = leg(
            r#"{ provider: openai, model: m, base_url: "https://api.openai.com/v1", credential_ref: OPENAI_API_KEY }"#,
        );
        // Bound to api.openai.com; its value is read per session, from the variable itself.
        match leg_key(&bound, "openai", || Some("sk-op".into())) {
            Ok(key) => assert_eq!(
                Some(key),
                crate::core::credentials::env_api_key("OPENAI_API_KEY")
            ),
            Err(e) => assert!(e.contains("holds no value"), "{e}"),
        }
        let mismatch = leg(
            r#"{ provider: openai, model: m, base_url: "https://proxy.example.com/v1", credential_ref: OPENAI_API_KEY }"#,
        );
        let e = leg_key(&mismatch, "openai", || Some("sk-op".into())).unwrap_err();
        assert!(e.contains("may not be sent"), "{e}");
        let no_base = leg("{ provider: openai, model: m, credential_ref: OPENAI_API_KEY }");
        assert!(
            leg_key(&no_base, "openai", || Some("sk-op".into()))
                .unwrap_err()
                .contains("needs realtime.base_url")
        );
    }

    #[test]
    fn the_block_becomes_the_entry_a_bud_deployment_would_publish() {
        let l = leg(
            r#"{ provider: OpenAI, model: gpt-realtime, voice: marin, instructions: "Be brief." }"#,
        );
        let ep = voice_endpoint(&l, "openai", "sk".into());
        assert!(ep.serves(REALTIME_CAPABILITY));
        let settings = ep.config.realtime.as_ref().unwrap();
        assert!(!settings.is_transcription());
        let update =
            super::super::policy::defaults_update(Some(settings), ep.model.as_deref(), "e1")
                .expect("the defaults are applied");
        let v: serde_json::Value = serde_json::from_str(&update).unwrap();
        assert_eq!(v["session"]["audio"]["output"]["voice"], "marin");
        assert_eq!(v["session"]["instructions"], "Be brief.");
        // Without defaults, nothing is sent on the vendor's behalf.
        let bare = voice_endpoint(
            &leg("{ provider: openai, model: m }"),
            "openai",
            "sk".into(),
        );
        assert!(
            super::super::policy::defaults_update(bare.config.realtime.as_ref(), Some("m"), "e2")
                .is_none()
        );
    }

    #[test]
    fn translated_vendors_use_the_gateways_key_for_their_provider() {
        assert_eq!(key_provider("deepgram_voice_agent"), "deepgram");
        assert_eq!(key_provider("hume_evi"), "hume");
        assert_eq!(key_provider("openai"), "openai");
        assert!(
            vendor_of(&leg("{ provider: nova_sonic, model: m }"))
                .unwrap_err()
                .contains("AWS key pair")
        );
        let e = vendor_of(&leg("{ provider: acme, model: m }")).unwrap_err();
        assert!(
            e.contains("openai") && e.contains("gemini") && !e.contains("nova_sonic"),
            "{e}"
        );
    }
}
