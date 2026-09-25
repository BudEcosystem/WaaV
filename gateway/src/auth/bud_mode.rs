//! Bud-mode authentication: the gateway's bridge to the `bud-auth` control plane.
//!
//! WaaV runs in one of two auth modes (FRD-018):
//!
//! * `external` — the original behaviour: sign a JWT, POST it to `auth_service_url`, read
//!   200/401. Kept so WaaV remains deployable outside Bud.
//! * `bud` — resolve identity locally from the same `api_key:{hash}` namespace budgateway
//!   reads, with **zero request-path I/O** and no call to budapp at all.
//!
//! This module is deliberately thin. All of the security-critical logic lives in `bud-auth`,
//! which is a separate crate so it can be tested in seconds instead of rebuilding a WebRTC
//! stack — the difference between a guard that gets exercised on every change and one that does
//! not.

use std::sync::Arc;
use std::time::Duration;

use bud_auth::hydrate::KeyEvent;
use bud_auth::redis_store::{RedisStore, event_from_channel, keyspace_patterns};
use bud_auth::{AuthFailure, BudPlane, ControlPlaneStore, JwtConfig, JwtVerifier, Principal};

use crate::auth::context::Auth;

/// Everything needed to stand the plane up, read from the environment.
pub struct BudModeConfig {
    pub redis_url: String,
    pub redis_db: u8,
    /// PEM for opening RSA-encrypted vendor credentials in `voice_table`.
    pub rsa_private_key_path: Option<String>,
    /// Passphrase for that PEM. Bud's clusters ship an ENCRYPTED PKCS#8 key, so this is the
    /// normal case rather than the exotic one.
    pub rsa_private_key_password: Option<String>,
}

impl BudModeConfig {
    /// `None` when bud mode is not configured, which leaves the external auth path in place.
    pub fn from_env() -> Option<Self> {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    /// The same logic over an injected lookup.
    ///
    /// Tests use this rather than mutating the process environment: `set_var` is global, and a
    /// parallel test runner will leak it into whatever else is running — which is exactly how
    /// an unrelated config test started failing.
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Option<Self> {
        let non_empty = |k: &str| {
            get(k)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };

        let redis_url = non_empty("WAAV_REDIS_URL")?;
        let redis_db = non_empty("WAAV_REDIS_DB")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        Some(Self {
            redis_url,
            redis_db,
            rsa_private_key_path: non_empty("WAAV_RSA_PRIVATE_KEY_PATH"),
            rsa_private_key_password: non_empty("WAAV_RSA_PRIVATE_KEY_PASSWORD"),
        })
    }
}

/// The gateway-facing handle.
pub struct BudMode {
    plane: Arc<BudPlane>,
    store: Arc<RedisStore>,
    db: u8,
}

impl BudMode {
    /// Build the plane and perform the FIRST hydration.
    ///
    /// Boot failing is fatal on purpose: a WaaV that starts with an empty snapshot answers 401
    /// to every valid credential, which looks like a total outage with a healthy-looking pod.
    /// Better to fail the readiness probe and let the rollout stall.
    pub async fn start(cfg: BudModeConfig) -> Result<Arc<Self>, String> {
        let store = Arc::new(RedisStore::new(&cfg.redis_url).map_err(|e| e.to_string())?);

        // JWT acceptance is off unless BOTH an issuer and a non-empty client allowlist are
        // configured. A half-configured verifier would accept a token minted for any client in
        // the realm, which is strictly worse than the feature being off.
        let jwt = match JwtConfig::from_env() {
            Some(jwt_cfg) => {
                let source: Arc<dyn bud_auth::jwt::JwksSource> =
                    Arc::new(HttpJwksSource::new(jwt_cfg.jwks_url.clone()));
                let verifier = Arc::new(JwtVerifier::new(jwt_cfg, source));
                // Prime so the first browser request does not pay for the fetch. A failure here
                // is not fatal: static keys still work, and the first JWT triggers a retry.
                if let Err(e) = verifier.prime().await {
                    tracing::warn!(error = %e, "JWKS prime failed; JWT callers will retry on first use");
                }
                Some(verifier)
            }
            None => {
                tracing::info!(
                    "OIDC not configured (needs OIDC_ISSUER and a non-empty OIDC_ALLOWED_CLIENTS); \
                     JWT callers will be refused and only bud_* keys accepted"
                );
                None
            }
        };

        // The decryptor opens RSA-encrypted vendor credentials at hydration. Its absence is a
        // warning rather than a failure: keyless self-hosted endpoints still work, and an
        // endpoint whose credential cannot be opened is dropped with a named error rather than
        // being used with ciphertext as its bearer token.
        let decryptor = match &cfg.rsa_private_key_path {
            Some(path) => match std::fs::read_to_string(path) {
                Ok(pem) => match bud_auth::CredentialDecryptor::from_pem_with_password(
                    &pem,
                    cfg.rsa_private_key_password.as_deref(),
                ) {
                    Ok(d) => {
                        tracing::info!(path = %path, "vendor-credential decryption enabled");
                        d
                    }
                    Err(e) => {
                        return Err(format!(
                            "RSA private key at {path} could not be loaded: {e}"
                        ));
                    }
                },
                Err(e) => return Err(format!("RSA private key at {path} could not be read: {e}")),
            },
            None => {
                tracing::warn!(
                    "WAAV_RSA_PRIVATE_KEY_PATH is unset; encrypted vendor credentials cannot be \
                     opened and those endpoints will be unusable"
                );
                bud_auth::CredentialDecryptor::disabled()
            }
        };

        let plane = Arc::new(BudPlane::with_decryptor(
            Arc::clone(&store) as Arc<dyn ControlPlaneStore>,
            jwt,
            decryptor,
        ));

        let stats = plane.boot().await.map_err(|e| {
            format!("initial control-plane hydration failed ({e}); refusing to start with an empty auth map")
        })?;
        tracing::info!(
            api_keys = stats.api_keys,
            skipped = stats.skipped,
            "control plane hydrated"
        );

        Ok(Arc::new(Self {
            plane,
            store,
            db: cfg.redis_db,
        }))
    }

    /// The Redis store the plane reads — and the voice-catalog publisher writes to.
    pub fn store(&self) -> &Arc<RedisStore> {
        &self.store
    }

    pub fn plane(&self) -> &Arc<BudPlane> {
        &self.plane
    }

    /// Readiness. False until the first hydration completes.
    pub fn is_ready(&self) -> bool {
        self.plane.is_ready()
    }

    /// Resolve a bearer into the gateway's `Auth` extension.
    pub async fn authenticate(&self, bearer: &str) -> Result<Auth, AuthFailure> {
        let principal: Principal = self.plane.authenticate(bearer).await?;
        // `Auth.id` is what tenant-scoped behaviour keys on — recording paths included, which
        // are already `{bucket}/{prefix}/{auth_id}/{stream_id}`. Using the project id makes that
        // isolation project-grained rather than per-user.
        Ok(Auth::new(
            principal
                .project_id
                .or(principal.user_id)
                .unwrap_or_else(|| "unknown".to_string()),
        ))
    }

    /// Spawn the keyspace subscriber, reconnecting with backoff and re-hydrating each time the
    /// connection comes back.
    ///
    /// The re-hydration is not optional. Events missed while disconnected are missed forever,
    /// so a subscriber that reconnects without re-scanning silently serves a stale auth map —
    /// including credentials revoked during the outage.
    pub fn spawn_keyspace_loop(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let this = Arc::clone(self);
        tokio::spawn(async move {
            let mut backoff = Duration::from_secs(1);
            let max_backoff = Duration::from_secs(30);

            loop {
                match this.run_subscription().await {
                    Ok(()) => {
                        tracing::warn!("control-plane subscription ended; reconnecting");
                        backoff = Duration::from_secs(1);
                    }
                    Err(e) => {
                        tracing::error!(error = %e, backoff_secs = backoff.as_secs(), "control-plane subscription failed");
                        tokio::time::sleep(backoff).await;
                        backoff = (backoff * 2).min(max_backoff);
                        continue;
                    }
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        })
    }

    async fn run_subscription(&self) -> Result<(), String> {
        use futures_util::StreamExt;

        let mut pubsub = self
            .store
            .client()
            .get_async_pubsub()
            .await
            .map_err(|e| e.to_string())?;

        for pattern in keyspace_patterns(self.db) {
            pubsub
                .psubscribe(&pattern)
                .await
                .map_err(|e| format!("psubscribe {pattern}: {e}"))?;
        }

        // Re-hydrate AFTER subscribing, not before: anything that changes between the two would
        // otherwise fall into the gap — missed by the scan and by the subscription both.
        if let Err(e) = self.plane.rehydrate_after_reconnect().await {
            tracing::error!(error = %e, "re-hydration after (re)subscribe failed");
        }

        let mut stream = pubsub.on_message();
        while let Some(msg) = stream.next().await {
            let channel = msg.get_channel_name().to_string();
            let key: String = match msg.get_payload() {
                Ok(k) => k,
                Err(_) => continue,
            };
            if let Some(event) = event_from_channel(&channel).and_then(KeyEvent::parse)
                && let Err(e) = self.plane.on_key_event(&key, event).await
            {
                tracing::warn!(key = %key, error = %e, "failed to apply keyspace event");
            }
        }
        Ok(())
    }
}

/// Fetches JWKS over HTTP.
struct HttpJwksSource {
    url: String,
    client: reqwest::Client,
}

impl HttpJwksSource {
    fn new(url: String) -> Self {
        Self {
            url,
            client: reqwest::Client::builder()
                // Bounded: this runs on the pre-auth path, so a hanging JWKS host must not
                // hold a verification slot open.
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap_or_default(),
        }
    }
}

#[async_trait::async_trait]
impl bud_auth::jwt::JwksSource for HttpJwksSource {
    async fn fetch(&self) -> Result<String, String> {
        let resp = self
            .client
            .get(&self.url)
            .send()
            .await
            .map_err(|e| format!("jwks fetch failed: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("jwks fetch returned {}", resp.status()));
        }
        resp.text()
            .await
            .map_err(|e| format!("jwks body unreadable: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bud_mode_is_absent_without_a_redis_url() {
        // Absence must leave the external auth path in place rather than half-enabling a
        // control plane with nowhere to read from.
        assert!(BudModeConfig::from_lookup(|_| None).is_none());
    }

    #[test]
    fn a_blank_redis_url_counts_as_absent() {
        // An empty value is the shape "not configured" takes in a Helm template.
        assert!(
            BudModeConfig::from_lookup(|k| (k == "WAAV_REDIS_URL").then(|| "   ".to_string()))
                .is_none()
        );
    }

    #[test]
    fn a_configured_url_yields_a_config() {
        let cfg = BudModeConfig::from_lookup(|k| match k {
            "WAAV_REDIS_URL" => Some("redis://localhost:6379/6".to_string()),
            "WAAV_REDIS_DB" => Some("6".to_string()),
            _ => None,
        })
        .expect("configured");
        assert_eq!(cfg.redis_db, 6);
        assert!(cfg.rsa_private_key_path.is_none());
        assert!(cfg.rsa_private_key_password.is_none());
    }

    #[test]
    fn the_key_passphrase_is_read_from_the_environment() {
        // Bud's clusters store an ENCRYPTED PKCS#8 key; dropping the passphrase here makes
        // every vendor credential unreadable and the pod refuse to start.
        let cfg = BudModeConfig::from_lookup(|k| match k {
            "WAAV_REDIS_URL" => Some("redis://localhost:6379".to_string()),
            "WAAV_RSA_PRIVATE_KEY_PATH" => Some("/app/keys/rsa-private-key.pem".to_string()),
            "WAAV_RSA_PRIVATE_KEY_PASSWORD" => Some("s3cret".to_string()),
            _ => None,
        })
        .expect("configured");
        assert_eq!(cfg.rsa_private_key_password.as_deref(), Some("s3cret"));
    }

    #[test]
    fn the_database_defaults_to_zero_when_unparseable() {
        // A malformed value must not silently select a different database from the one
        // budgateway reads — but nor should it stop the process starting.
        let cfg = BudModeConfig::from_lookup(|k| match k {
            "WAAV_REDIS_URL" => Some("redis://localhost:6379".to_string()),
            "WAAV_REDIS_DB" => Some("not-a-number".to_string()),
            _ => None,
        })
        .expect("configured");
        assert_eq!(cfg.redis_db, 0);
    }
}
