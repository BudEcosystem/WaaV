//! Which authenticators a standalone (non-Bud) gateway consults, in which order, and what an
//! external auth responder must answer to admit a caller.
//!
//! Two operator knobs, read once at startup from the environment (like `WAAV_REDIS_URL` and
//! `WAAV_ALLOW_LOOPBACK_ENDPOINTS`):
//!
//! * `AUTH_MODE` = `chain` (default) | `api_secret` | `responder`.
//!   - `chain`: a bearer that matches a configured API secret is admitted as that secret's id;
//!     any other bearer is asked of the auth responder (`AUTH_SERVICE_URL`). This is what
//!     `docs/authentication.md` has always promised ("Both methods can coexist … admin access via
//!     secret + user access via JWT"); the middleware used to answer 401 on a secret mismatch and
//!     never call the responder.
//!   - `api_secret`: only the secret list; a configured responder is ignored (startup WARN).
//!   - `responder`: only the responder; a configured secret list is ignored (startup WARN).
//! * `AUTH_RESPONDER_REQUIRE_IDENTITY` = `true` (default) | `false`. A `200` from the responder
//!   admits a caller only when its JSON body names who the caller is (see
//!   [`identity_from_responder_body`]). `false` restores the legacy behaviour (any `200` admits,
//!   with no identity) and logs a startup WARN: an identity-less caller gets no tenant prefix on
//!   LiveKit rooms or recordings.
//!
//! Bud mode (the control plane) outranks both, as before; [`AuthPolicy::startup_report`] now says
//! so instead of silently ignoring the other settings.

use crate::config::ServerConfig;

/// Env var: which authenticators to consult (`chain` | `api_secret` | `responder`).
pub const AUTH_MODE_ENV: &str = "AUTH_MODE";
/// Env var: whether a responder `200` must carry an identity (default `true`).
pub const REQUIRE_IDENTITY_ENV: &str = "AUTH_RESPONDER_REQUIRE_IDENTITY";

/// Longest identity accepted from a responder (it becomes a room/recording path prefix).
const MAX_IDENTITY_LEN: usize = 256;

/// Responder body keys that may carry the caller's identity, in precedence order. `id` is the
/// documented field (`Auth.id`); the others are the names budapp/bud-auth (`project_id`,
/// `user_id`), multi-tenant responders (`tenant_id`) and OIDC introspection (`sub`) use, so a
/// responder written against any of them works unchanged.
pub const IDENTITY_KEYS: &[&str] = &["id", "tenant_id", "project_id", "user_id", "sub"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    /// API secrets first, then the responder for anything that is not a secret.
    Chain,
    /// API secrets only.
    ApiSecret,
    /// Responder only.
    Responder,
}

impl AuthMode {
    pub fn parse(raw: &str) -> Result<Self, String> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "" | "chain" | "both" => Ok(Self::Chain),
            "api_secret" | "api-secret" | "secret" => Ok(Self::ApiSecret),
            "responder" | "jwt" | "auth_service" => Ok(Self::Responder),
            other => Err(format!(
                "{AUTH_MODE_ENV}={other:?} is not one of chain | api_secret | responder"
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthPolicy {
    pub mode: AuthMode,
    pub require_identity: bool,
}

impl Default for AuthPolicy {
    fn default() -> Self {
        Self {
            mode: AuthMode::Chain,
            require_identity: true,
        }
    }
}

impl AuthPolicy {
    /// Read the policy from the environment. An unparseable value is a startup error, never a
    /// silent default: both knobs decide who gets in.
    pub fn from_env() -> Result<Self, String> {
        Self::from_values(
            std::env::var(AUTH_MODE_ENV).ok().as_deref(),
            std::env::var(REQUIRE_IDENTITY_ENV).ok().as_deref(),
        )
    }

    /// The pure parse behind [`Self::from_env`].
    pub fn from_values(mode: Option<&str>, require_identity: Option<&str>) -> Result<Self, String> {
        let mode = match mode {
            Some(m) => AuthMode::parse(m)?,
            None => AuthMode::Chain,
        };
        let require_identity = match require_identity {
            None => true,
            Some(v) if v.trim().is_empty() => true,
            Some(v) => crate::config::utils::parse_bool(v).ok_or_else(|| {
                format!("{REQUIRE_IDENTITY_ENV}={v:?} must be one of true/false/1/0/yes/no")
            })?,
        };
        Ok(Self {
            mode,
            require_identity,
        })
    }

    /// Whether the API-secret list is consulted.
    pub fn secrets_active(&self, config: &ServerConfig) -> bool {
        config.has_api_secret_auth() && self.mode != AuthMode::Responder
    }

    /// Whether the external auth responder is consulted.
    pub fn responder_active(&self, config: &ServerConfig) -> bool {
        config.has_jwt_auth() && self.mode != AuthMode::ApiSecret
    }

    /// Log, once at startup, which authenticators are live and which configured ones are not.
    /// Every "configured but ignored" case is a WARN: before this, an operator who set both an
    /// API secret and `AUTH_SERVICE_URL` saw "JWT authentication enabled with service: …" and a
    /// responder that was never called.
    pub fn startup_report(&self, config: &ServerConfig, bud_mode: bool) {
        if !config.auth_required {
            if config.has_api_secret_auth() || config.has_jwt_auth() {
                tracing::warn!(
                    "AUTH_REQUIRED=false: the configured API secrets / AUTH_SERVICE_URL are NOT \
                     consulted; every request is admitted without an identity"
                );
            }
            return;
        }
        if bud_mode {
            if config.has_api_secret_auth() || config.has_jwt_auth() {
                tracing::warn!(
                    "Bud control plane (WAAV_REDIS_URL) is the identity source; the configured \
                     API secrets / AUTH_SERVICE_URL are ignored"
                );
            }
            return;
        }
        let secrets = self.secrets_active(config);
        let responder = self.responder_active(config);
        match (secrets, responder) {
            (true, true) => tracing::info!(
                "auth chain: API secret, then auth responder for any other bearer ({AUTH_MODE_ENV}=chain)"
            ),
            (true, false) => tracing::info!("auth: API secrets only"),
            (false, true) => tracing::info!("auth: auth responder only"),
            (false, false) => tracing::error!(
                "AUTH_REQUIRED=true but {AUTH_MODE_ENV} leaves no authenticator active; every \
                 request will be refused"
            ),
        }
        if config.has_jwt_auth() && !responder {
            tracing::warn!(
                "AUTH_SERVICE_URL is configured but {AUTH_MODE_ENV}=api_secret: the auth responder \
                 is never called"
            );
        }
        if config.has_api_secret_auth() && !secrets {
            tracing::warn!(
                "API secrets are configured but {AUTH_MODE_ENV}=responder: they are never matched"
            );
        }
        if responder && !self.require_identity {
            tracing::warn!(
                "{REQUIRE_IDENTITY_ENV}=false: an auth responder answering 200 without an identity \
                 admits the caller anonymously (no tenant prefix on rooms or recordings)"
            );
        }
    }
}

/// The caller's identity from a responder's `200` body, or `None` when it names nobody.
///
/// The body must be a JSON object with a non-empty string under one of [`IDENTITY_KEYS`]
/// (first match wins). Whitespace is trimmed; control characters and identities longer than
/// [`MAX_IDENTITY_LEN`] are refused (the id becomes a path / room-name prefix).
pub fn identity_from_responder_body(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let object = value.as_object()?;
    IDENTITY_KEYS.iter().find_map(|key| {
        let id = object.get(*key)?.as_str()?.trim();
        (!id.is_empty() && id.len() <= MAX_IDENTITY_LEN && !id.chars().any(char::is_control))
            .then(|| id.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_chain_and_identity_required() {
        assert_eq!(
            AuthPolicy::from_values(None, None),
            Ok(AuthPolicy::default())
        );
        assert_eq!(AuthPolicy::default().mode, AuthMode::Chain);
        assert!(AuthPolicy::default().require_identity);
    }

    #[test]
    fn modes_parse_and_garbage_is_an_error() {
        assert_eq!(AuthMode::parse("api_secret"), Ok(AuthMode::ApiSecret));
        assert_eq!(AuthMode::parse("Responder"), Ok(AuthMode::Responder));
        assert!(AuthMode::parse("secret-or-maybe").is_err());
        assert!(AuthPolicy::from_values(None, Some("perhaps")).is_err());
        assert!(
            !AuthPolicy::from_values(None, Some("false"))
                .unwrap()
                .require_identity
        );
    }

    #[test]
    fn an_empty_or_identity_less_200_names_nobody() {
        for body in [
            "",
            "{}",
            "OK",
            "null",
            "[]",
            r#"{"id":""}"#,
            r#"{"id":"  "}"#,
            r#"{"id":null}"#,
            r#"{"id":42}"#,
            r#"{"valid":true}"#,
            "{\"id\":\"a\\u0000b\"}",
        ] {
            assert_eq!(identity_from_responder_body(body), None, "{body}");
        }
        let long = format!(r#"{{"id":"{}"}}"#, "a".repeat(MAX_IDENTITY_LEN + 1));
        assert_eq!(identity_from_responder_body(&long), None);
    }

    #[test]
    fn identity_keys_in_precedence_order() {
        assert_eq!(
            identity_from_responder_body(r#"{"id":"p1"}"#).as_deref(),
            Some("p1")
        );
        assert_eq!(
            identity_from_responder_body(r#"{"sub":"u9","tenant_id":"t1"}"#).as_deref(),
            Some("t1")
        );
        assert_eq!(
            identity_from_responder_body(r#"{"user_id":" u2 "}"#).as_deref(),
            Some("u2")
        );
    }
}
