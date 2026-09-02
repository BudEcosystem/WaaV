//! Local JWT verification for browser, widget and playground callers.
//!
//! Browser traffic carries a Keycloak access token rather than a Bud API key. Verifying it
//! here — rather than asking Bud to mint a session credential — keeps budapp off the session
//! path entirely and avoids a control-plane write per session (spec §5.3.6).
//!
//! Six invariants govern this module. Each exists because something went wrong without it in
//! budgateway, and each has a test that goes red when it is removed:
//!
//! 1. Authentication is cached under the **token hash**; authorization under `sub`.
//! 2. The verifier is absent unless **both** an issuer and a non-empty client allowlist exist.
//! 3. JWKS refetches are floored, so a `kid`-spray cannot amplify outbound traffic.
//! 4. Verification concurrency is bounded — it is CPU work on a pre-auth path.
//! 5. A **degraded** authorization is never cached.
//! 6. Cached entries expire on `min(exp, cap)`, with the cap applied **before** the addition.

use arc_swap::ArcSwap;
use dashmap::DashMap;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::Semaphore;

use crate::hash::hash_api_key;
use crate::types::VerifiedIdentity;

/// Why a token was refused. Kept coarse on purpose: the caller gets 401 either way, and a
/// detailed reason on the wire is a probing oracle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rejection {
    Shape,
    UnknownKid,
    BadSignature,
    Expired,
    WrongIssuer,
    ClientNotAllowed,
    Overloaded,
    NoKeys,
}

impl Rejection {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Shape => "shape",
            Self::UnknownKid => "unknown_kid",
            Self::BadSignature => "bad_signature",
            Self::Expired => "expired",
            Self::WrongIssuer => "wrong_issuer",
            Self::ClientNotAllowed => "client_not_allowed",
            Self::Overloaded => "overloaded",
            Self::NoKeys => "no_keys",
        }
    }
}

#[derive(Debug, Clone)]
pub struct JwtConfig {
    /// Exact `iss` the token must carry.
    pub issuer: String,
    pub jwks_url: String,
    /// Clients whose tokens this gateway accepts, matched against `azp` then `aud`.
    pub allowed_clients: Vec<String>,
    pub leeway_secs: u64,
    /// Hard cap on how long a verified token stays cached, regardless of its own `exp`.
    pub max_entry_ttl: Duration,
    /// How long a resolved authorization is reused. This is the propagation delay for a
    /// membership change.
    pub authz_ttl: Duration,
    /// Floor between JWKS refetches.
    pub jwks_min_refetch: Duration,
    pub negative_ttl: Duration,
    pub cache_capacity: usize,
    pub max_concurrent_verify: usize,
}

impl JwtConfig {
    /// Build from the environment, or `None` to leave JWT acceptance **disabled**.
    ///
    /// Invariant 2. Requires BOTH an issuer and a non-empty allowlist: an issuer without an
    /// allowlist would accept a token minted for any client in the realm, including ones with
    /// entirely different trust properties. Half-configured is strictly worse than off.
    pub fn from_env() -> Option<Self> {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Option<Self> {
        let non_empty = |k: &str| get(k).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        let u64_or = |k: &str, d: u64| {
            get(k)
                .and_then(|v| v.trim().parse::<u64>().ok())
                .unwrap_or(d)
        };

        let issuer = non_empty("OIDC_ISSUER")?;
        let allowed_clients: Vec<String> = non_empty("OIDC_ALLOWED_CLIENTS")?
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        // A comma-only value (`OIDC_ALLOWED_CLIENTS=","`) parses to an empty list. Treating
        // that as "configured" would accept every token in the realm.
        if allowed_clients.is_empty() {
            return None;
        }

        let jwks_url = non_empty("OIDC_JWKS_URL")
            .unwrap_or_else(|| format!("{}/protocol/openid-connect/certs", issuer.trim_end_matches('/')));

        Some(Self {
            issuer,
            jwks_url,
            allowed_clients,
            leeway_secs: u64_or("OIDC_LEEWAY_SECS", 30),
            max_entry_ttl: Duration::from_secs(u64_or("OIDC_MAX_ENTRY_TTL_SECS", 300)),
            authz_ttl: Duration::from_secs(u64_or("OIDC_AUTHZ_TTL_SECS", 300)),
            jwks_min_refetch: Duration::from_secs(u64_or("OIDC_JWKS_MIN_REFETCH_SECS", 60)),
            negative_ttl: Duration::from_secs(u64_or("OIDC_NEGATIVE_TTL_SECS", 60)),
            cache_capacity: u64_or("OIDC_CACHE_CAPACITY", 10_000).max(1) as usize,
            max_concurrent_verify: u64_or("OIDC_MAX_CONCURRENT_VERIFY", 32).max(1) as usize,
        })
    }
}

/// A JWKS document reduced to what verification needs.
#[derive(Default)]
pub struct KeySet {
    keys: HashMap<String, Arc<jsonwebtoken::DecodingKey>>,
}

impl KeySet {
    pub fn len(&self) -> usize {
        self.keys.len()
    }
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// Parse a JWKS document.
    ///
    /// **`oct` keys are refused.** A symmetric key published in a public JWKS is a signing key
    /// anyone who can read the document also holds — accepting one would let any reader forge
    /// tokens. Asymmetric families only.
    pub fn from_jwks_json(doc: &str) -> Result<Self, String> {
        let v: serde_json::Value =
            serde_json::from_str(doc).map_err(|e| format!("jwks is not json: {e}"))?;
        let arr = v
            .get("keys")
            .and_then(|k| k.as_array())
            .ok_or_else(|| "jwks has no `keys` array".to_string())?;

        let mut keys = HashMap::new();
        for k in arr {
            let kty = k.get("kty").and_then(|x| x.as_str()).unwrap_or_default();
            let kid = match k.get("kid").and_then(|x| x.as_str()) {
                Some(kid) if !kid.is_empty() => kid,
                _ => continue,
            };
            let decoding = match kty {
                "RSA" => {
                    let (n, e) = match (
                        k.get("n").and_then(|x| x.as_str()),
                        k.get("e").and_then(|x| x.as_str()),
                    ) {
                        (Some(n), Some(e)) => (n, e),
                        _ => continue,
                    };
                    match jsonwebtoken::DecodingKey::from_rsa_components(n, e) {
                        Ok(d) => d,
                        Err(_) => continue,
                    }
                }
                "EC" => {
                    let (x, y) = match (
                        k.get("x").and_then(|v| v.as_str()),
                        k.get("y").and_then(|v| v.as_str()),
                    ) {
                        (Some(x), Some(y)) => (x, y),
                        _ => continue,
                    };
                    match jsonwebtoken::DecodingKey::from_ec_components(x, y) {
                        Ok(d) => d,
                        Err(_) => continue,
                    }
                }
                // Deliberately skipped, including "oct": see the doc comment.
                _ => continue,
            };
            keys.insert(kid.to_string(), Arc::new(decoding));
        }
        Ok(Self { keys })
    }

    fn get(&self, kid: &str) -> Option<Arc<jsonwebtoken::DecodingKey>> {
        self.keys.get(kid).cloned()
    }
}

/// Where a JWKS document comes from. Abstracted so tests never touch the network and can
/// count fetches (invariant 3).
#[async_trait::async_trait]
pub trait JwksSource: Send + Sync {
    async fn fetch(&self) -> Result<String, String>;
}

/// Authentication: "this exact token string was verified, and is good until `expires_at`."
#[derive(Clone)]
struct AuthEntry {
    sub: Arc<str>,
    exp: u64,
    expires_at: Instant,
}

/// Authorization: what a `sub` may reach, and until when it may be reused.
#[derive(Clone)]
pub struct AuthzEntry {
    pub aliases: Arc<crate::types::AliasMap>,
    pub fresh_until: Instant,
}

pub struct JwtVerifier {
    cfg: JwtConfig,
    keys: ArcSwap<KeySet>,
    source: Arc<dyn JwksSource>,
    /// Millis since `origin` of the last refetch *attempt*. An attempt counts whether or not it
    /// succeeded, so a failing JWKS cannot be used as a retry amplifier.
    last_attempt_ms: AtomicU64,
    origin: Instant,
    refresh_lock: tokio::sync::Mutex<()>,
    verify_slots: Arc<Semaphore>,

    /// `sha256("bud-" + token)` -> the identity it authenticated.
    auth_cache: DashMap<String, AuthEntry>,
    /// `sha256("bud-" + token)` -> when verification permanently failed.
    negative: DashMap<String, Instant>,
    /// `sub` -> resolved authorization.
    authz_cache: DashMap<String, AuthzEntry>,

    pub fetches: AtomicU64,
    pub verifications: AtomicU64,
}

impl JwtVerifier {
    pub fn new(cfg: JwtConfig, source: Arc<dyn JwksSource>) -> Self {
        Self {
            verify_slots: Arc::new(Semaphore::new(cfg.max_concurrent_verify.max(1))),
            cfg,
            keys: ArcSwap::from_pointee(KeySet::default()),
            source,
            last_attempt_ms: AtomicU64::new(0),
            origin: Instant::now(),
            refresh_lock: tokio::sync::Mutex::new(()),
            auth_cache: DashMap::new(),
            negative: DashMap::new(),
            authz_cache: DashMap::new(),
            fetches: AtomicU64::new(0),
            verifications: AtomicU64::new(0),
        }
    }

    fn now_epoch() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    fn now_ms(&self) -> u64 {
        self.origin.elapsed().as_millis() as u64
    }

    /// Fetch the JWKS once at startup so the first request does not pay for it.
    pub async fn prime(&self) -> Result<(), String> {
        self.refresh(true).await
    }

    /// Refresh the key set.
    ///
    /// Invariant 3: unless `force`, refetches are floored at `jwks_min_refetch`. Without the
    /// floor, tokens carrying unknown `kid`s — which anyone can mint, since no signature has
    /// been checked yet — drive unbounded outbound fetches from an unauthenticated path.
    async fn refresh(&self, force: bool) -> Result<(), String> {
        let now = self.now_ms();
        if !force {
            let last = self.last_attempt_ms.load(Ordering::Relaxed);
            let floor = self.cfg.jwks_min_refetch.as_millis() as u64;
            if last != 0 && now.saturating_sub(last) < floor {
                return Err("jwks refetch floored".into());
            }
        }

        let _guard = self.refresh_lock.lock().await;
        // Re-check under the lock: a queue of waiters must not each fire a fetch.
        if !force {
            let last = self.last_attempt_ms.load(Ordering::Relaxed);
            let floor = self.cfg.jwks_min_refetch.as_millis() as u64;
            if last != 0 && self.now_ms().saturating_sub(last) < floor {
                return Err("jwks refetch floored".into());
            }
        }
        // Stamp the ATTEMPT before doing it, so a failure still consumes the window.
        self.last_attempt_ms.store(self.now_ms(), Ordering::Relaxed);
        self.fetches.fetch_add(1, Ordering::Relaxed);

        let doc = self.source.fetch().await?;
        let set = KeySet::from_jwks_json(&doc)?;
        if set.is_empty() {
            return Err("jwks contained no usable keys".into());
        }
        self.keys.store(Arc::new(set));
        Ok(())
    }

    /// Verify a bearer and return the identity it authenticates.
    pub async fn verify(&self, raw: &str) -> Result<VerifiedIdentity, Rejection> {
        if !matches!(crate::guards::KeyShape::classify(raw), crate::guards::KeyShape::Jwt) {
            return Err(Rejection::Shape);
        }
        let hashed = hash_api_key(raw);

        // Invariant 1: the cache is keyed by the token hash. A `sub`-keyed entry would be
        // selected by a value read out of an UNVERIFIED token, so anyone who knows a victim's
        // `sub` could forge a bearer and be authenticated with no signature ever checked.
        if let Some(id) = self.cached_identity(&hashed) {
            return Ok(id);
        }
        if self.is_known_bad(&hashed) {
            return Err(Rejection::BadSignature);
        }

        // Invariant 4: signature checking is CPU work on a pre-auth path.
        let _permit = match Arc::clone(&self.verify_slots).try_acquire_owned() {
            Ok(p) => p,
            Err(_) => return Err(Rejection::Overloaded),
        };

        let header = jsonwebtoken::decode_header(raw).map_err(|_| Rejection::Shape)?;
        let kid = header.kid.ok_or(Rejection::UnknownKid)?;

        let key = match self.keys.load().get(&kid) {
            Some(k) => k,
            None => {
                // Unknown kid — possibly a rotation. One floored refetch, then give up.
                let _ = self.refresh(false).await;
                self.keys.load().get(&kid).ok_or(Rejection::UnknownKid)?
            }
        };

        let mut validation = jsonwebtoken::Validation::new(header.alg);
        validation.set_issuer(&[self.cfg.issuer.as_str()]);
        validation.leeway = self.cfg.leeway_secs;
        validation.validate_exp = true;
        // `aud` is checked manually against the allowlist below, because Keycloak puts the
        // client in `azp` and `aud` varies by realm configuration.
        validation.validate_aud = false;
        validation.required_spec_claims = ["exp", "iss"].iter().map(|s| s.to_string()).collect();

        self.verifications.fetch_add(1, Ordering::Relaxed);
        let decoded = jsonwebtoken::decode::<serde_json::Value>(raw, &key, &validation).map_err(|e| {
            use jsonwebtoken::errors::ErrorKind;
            match e.kind() {
                ErrorKind::ExpiredSignature => Rejection::Expired,
                ErrorKind::InvalidIssuer => Rejection::WrongIssuer,
                _ => Rejection::BadSignature,
            }
        });

        let decoded = match decoded {
            Ok(d) => d,
            Err(r) => {
                // Only a PERMANENT failure is negatively cached. An overload or a transient
                // fetch error must stay retryable.
                if matches!(r, Rejection::BadSignature | Rejection::WrongIssuer) {
                    self.record_bad(&hashed);
                }
                return Err(r);
            }
        };

        let claims = decoded.claims;
        let client = claims
            .get("azp")
            .and_then(|v| v.as_str())
            .or_else(|| claims.get("aud").and_then(|v| v.as_str()));
        let allowed = match client {
            Some(c) => self.cfg.allowed_clients.iter().any(|a| a == c),
            None => false,
        };
        if !allowed {
            tracing::warn!(
                client = ?client,
                "JWT refused: client is not on OIDC_ALLOWED_CLIENTS"
            );
            self.record_bad(&hashed);
            return Err(Rejection::ClientNotAllowed);
        }

        let sub = claims
            .get("sub")
            .and_then(|v| v.as_str())
            .ok_or(Rejection::BadSignature)?
            .to_string();
        let exp = claims.get("exp").and_then(|v| v.as_u64()).unwrap_or(0);

        let identity = VerifiedIdentity { sub, exp };
        self.store_authenticated(&hashed, &identity);
        Ok(identity)
    }

    /// Invariant 6. The only thing that ever evicts an authentication entry is its own
    /// deadline — no Redis key sits behind it, so no `del`/`expired` event will arrive.
    pub fn store_authenticated(&self, hashed: &str, identity: &VerifiedIdentity) {
        let remaining = identity.exp.saturating_sub(Self::now_epoch());
        // Cap FIRST, then add. `exp` is attacker-chosen within a signed token, so
        // `min(now + exp, now + cap)` computed the other way round can overflow into a
        // far-future deadline.
        let ttl = Duration::from_secs(remaining).min(self.cfg.max_entry_ttl);

        if self.auth_cache.len() >= self.cfg.cache_capacity {
            let victim = self.auth_cache.iter().next().map(|e| e.key().clone());
            if let Some(v) = victim {
                self.auth_cache.remove(&v);
            }
        }
        self.auth_cache.insert(
            hashed.to_string(),
            AuthEntry {
                sub: Arc::from(identity.sub.as_str()),
                exp: identity.exp,
                expires_at: Instant::now() + ttl,
            },
        );
    }

    pub fn cached_identity(&self, hashed: &str) -> Option<VerifiedIdentity> {
        let entry = self.auth_cache.get(hashed).map(|e| e.value().clone())?;
        if Instant::now() < entry.expires_at {
            Some(VerifiedIdentity {
                sub: entry.sub.to_string(),
                exp: entry.exp,
            })
        } else {
            self.auth_cache.remove(hashed);
            None
        }
    }

    /// Test-facing: the deadline an entry actually got, so `min(exp, cap)` can be asserted
    /// without sleeping.
    pub fn cached_expiry(&self, hashed: &str) -> Option<Instant> {
        self.auth_cache.get(hashed).map(|e| e.expires_at)
    }

    fn record_bad(&self, hashed: &str) {
        self.negative.insert(hashed.to_string(), Instant::now());
    }

    fn is_known_bad(&self, hashed: &str) -> bool {
        match self.negative.get(hashed).map(|e| *e.value()) {
            Some(at) if at.elapsed() < self.cfg.negative_ttl => true,
            Some(_) => {
                self.negative.remove(hashed);
                false
            }
            None => false,
        }
    }

    // ---- authorization ----

    pub fn cached_authz(&self, sub: &str) -> Option<AuthzEntry> {
        let e = self.authz_cache.get(sub).map(|e| e.value().clone())?;
        if Instant::now() < e.fresh_until {
            Some(e)
        } else {
            self.authz_cache.remove(sub);
            None
        }
    }

    /// Invariant 5 lives at the CALL SITE: only an authoritative resolution may be stored.
    /// A transient failure degrades for that request and is deliberately not cached, so the
    /// next request retries — otherwise a momentary Redis wobble becomes `authz_ttl` of
    /// silently reduced access.
    pub fn store_authz(&self, sub: &str, aliases: Arc<crate::types::AliasMap>) {
        self.authz_cache.insert(
            sub.to_string(),
            AuthzEntry {
                aliases,
                fresh_until: Instant::now() + self.cfg.authz_ttl,
            },
        );
    }

    pub fn config(&self) -> &JwtConfig {
        &self.cfg
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    const JWKS: &str = include_str!("../tests/fixtures/test_jwks.json");
    const PRIV: &str = include_str!("../tests/fixtures/test_rsa_private.pem");
    const ISSUER: &str = "https://auth.test/realms/bud";

    struct StaticSource {
        doc: String,
        calls: AtomicUsize,
        fail: bool,
    }
    impl StaticSource {
        fn ok() -> Arc<Self> {
            Arc::new(Self {
                doc: JWKS.to_string(),
                calls: AtomicUsize::new(0),
                fail: false,
            })
        }
        fn failing() -> Arc<Self> {
            Arc::new(Self {
                doc: String::new(),
                calls: AtomicUsize::new(0),
                fail: true,
            })
        }
        fn calls(&self) -> usize {
            self.calls.load(Ordering::Relaxed)
        }
    }
    #[async_trait::async_trait]
    impl JwksSource for StaticSource {
        async fn fetch(&self) -> Result<String, String> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            if self.fail {
                Err("boom".into())
            } else {
                Ok(self.doc.clone())
            }
        }
    }

    fn cfg() -> JwtConfig {
        JwtConfig {
            issuer: ISSUER.into(),
            jwks_url: "https://auth.test/certs".into(),
            allowed_clients: vec!["bud-widget".into()],
            leeway_secs: 5,
            max_entry_ttl: Duration::from_secs(300),
            authz_ttl: Duration::from_secs(300),
            jwks_min_refetch: Duration::from_secs(60),
            negative_ttl: Duration::from_secs(60),
            cache_capacity: 1000,
            max_concurrent_verify: 32,
        }
    }

    fn token(claims: serde_json::Value, kid: &str) -> String {
        let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
        header.kid = Some(kid.to_string());
        let key = jsonwebtoken::EncodingKey::from_rsa_pem(PRIV.as_bytes()).unwrap();
        jsonwebtoken::encode(&header, &claims, &key).unwrap()
    }

    fn valid_claims(exp_in: i64) -> serde_json::Value {
        let now = JwtVerifier::now_epoch() as i64;
        serde_json::json!({
            "iss": ISSUER, "sub": "user-abc", "azp": "bud-widget",
            "exp": now + exp_in, "iat": now,
        })
    }

    async fn primed(source: Arc<StaticSource>) -> JwtVerifier {
        let v = JwtVerifier::new(cfg(), source as Arc<dyn JwksSource>);
        v.prime().await.unwrap();
        v
    }

    #[tokio::test]
    async fn verifies_a_well_formed_token() {
        let v = primed(StaticSource::ok()).await;
        let id = v.verify(&token(valid_claims(600), "test-key-1")).await.unwrap();
        assert_eq!(id.sub, "user-abc");
    }

    /// TC-JWT-01 — invariant 1, the vulnerability this design exists to prevent.
    ///
    /// A victim authenticates. An attacker who knows their `sub` mints a token carrying it with
    /// a bogus signature. If authentication were cached under `sub`, the attacker's token would
    /// select the victim's entry and be accepted with no signature ever checked.
    #[tokio::test]
    async fn a_forged_token_carrying_a_known_sub_is_rejected() {
        let v = primed(StaticSource::ok()).await;

        v.verify(&token(valid_claims(600), "test-key-1")).await.unwrap();

        // Same `sub`, same header, signature replaced.
        let genuine = token(valid_claims(600), "test-key-1");
        let mut parts: Vec<&str> = genuine.split('.').collect();
        parts[2] = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
        let forged = parts.join(".");

        assert!(
            v.verify(&forged).await.is_err(),
            "a forged token was accepted; authentication is keyed by an unverified claim"
        );
    }

    /// TC-JWT-02 — invariant 2.
    #[test]
    fn verifier_absent_unless_issuer_and_allowlist_are_both_present() {
        let only_issuer =
            JwtConfig::from_lookup(|k| (k == "OIDC_ISSUER").then(|| ISSUER.to_string()));
        assert!(only_issuer.is_none(), "an issuer alone configured a verifier");

        let comma_only = JwtConfig::from_lookup(|k| match k {
            "OIDC_ISSUER" => Some(ISSUER.to_string()),
            "OIDC_ALLOWED_CLIENTS" => Some(" , ,".to_string()),
            _ => None,
        });
        assert!(
            comma_only.is_none(),
            "a comma-only allowlist configured a verifier that accepts the whole realm"
        );

        let both = JwtConfig::from_lookup(|k| match k {
            "OIDC_ISSUER" => Some(ISSUER.to_string()),
            "OIDC_ALLOWED_CLIENTS" => Some("bud-widget".to_string()),
            _ => None,
        });
        assert!(both.is_some());
    }

    /// TC-JWT-03 — invariant 3. A kid-spray must not amplify outbound fetches.
    #[tokio::test]
    async fn jwks_refetch_is_floored_under_a_kid_spray() {
        let src = StaticSource::ok();
        let v = primed(Arc::clone(&src)).await;
        let after_prime = src.calls();

        for i in 0..100 {
            let _ = v.verify(&token(valid_claims(600), &format!("unknown-{i}"))).await;
        }

        assert_eq!(
            src.calls(),
            after_prime + 1,
            "100 unknown kids produced {} fetches; the floor is not holding",
            src.calls() - after_prime
        );
    }

    /// A failing JWKS must not become a retry amplifier either — the attempt is stamped
    /// before the fetch, so failures consume the window too.
    #[tokio::test]
    async fn a_failing_jwks_does_not_amplify() {
        let src = StaticSource::failing();
        let v = JwtVerifier::new(cfg(), Arc::clone(&src) as Arc<dyn JwksSource>);
        for _ in 0..50 {
            let _ = v.verify(&token(valid_claims(600), "test-key-1")).await;
        }
        assert!(src.calls() <= 2, "failing jwks fetched {} times", src.calls());
    }

    /// TC-JWT-04 — invariant 4.
    #[tokio::test]
    async fn verification_concurrency_is_bounded() {
        let mut c = cfg();
        c.max_concurrent_verify = 2;
        let v = JwtVerifier::new(c, StaticSource::ok() as Arc<dyn JwksSource>);
        v.prime().await.unwrap();

        // Hold both slots, then confirm the next attempt is refused rather than queued.
        let held: Vec<_> = (0..2)
            .map(|_| Arc::clone(&v.verify_slots).try_acquire_owned().unwrap())
            .collect();
        let r = v.verify(&token(valid_claims(600), "test-key-1")).await;
        assert_eq!(r.unwrap_err(), Rejection::Overloaded);
        drop(held);
        assert!(v.verify(&token(valid_claims(600), "test-key-1")).await.is_ok());
    }

    /// TC-JWT-06 — invariant 6, both directions.
    #[tokio::test]
    async fn entry_ttl_is_the_lesser_of_exp_and_cap() {
        let mut c = cfg();
        c.max_entry_ttl = Duration::from_secs(300);
        let v = JwtVerifier::new(c, StaticSource::ok() as Arc<dyn JwksSource>);
        v.prime().await.unwrap();

        // exp well beyond the cap -> capped.
        let long = token(valid_claims(3600), "test-key-1");
        v.verify(&long).await.unwrap();
        let capped = v.cached_expiry(&hash_api_key(&long)).unwrap();
        assert!(
            capped.duration_since(Instant::now()) <= Duration::from_secs(301),
            "an attacker-chosen exp outlived the cap"
        );

        // exp inside the cap -> exp wins.
        let short = token(valid_claims(10), "test-key-1");
        v.verify(&short).await.unwrap();
        let by_exp = v.cached_expiry(&hash_api_key(&short)).unwrap();
        assert!(
            by_exp.duration_since(Instant::now()) <= Duration::from_secs(11),
            "a short-lived token was cached for the full cap"
        );
    }

    /// TC-JWT-07
    #[tokio::test]
    async fn wrong_issuer_is_rejected() {
        let v = primed(StaticSource::ok()).await;
        let now = JwtVerifier::now_epoch() as i64;
        let claims = serde_json::json!({
            "iss": "https://evil.test/realms/bud", "sub": "u", "azp": "bud-widget",
            "exp": now + 600, "iat": now,
        });
        assert!(v.verify(&token(claims, "test-key-1")).await.is_err());
    }

    /// TC-JWT-08
    #[tokio::test]
    async fn client_not_on_the_allowlist_is_rejected() {
        let v = primed(StaticSource::ok()).await;
        let now = JwtVerifier::now_epoch() as i64;
        let claims = serde_json::json!({
            "iss": ISSUER, "sub": "u", "azp": "some-other-client",
            "exp": now + 600, "iat": now,
        });
        assert_eq!(
            v.verify(&token(claims, "test-key-1")).await.unwrap_err(),
            Rejection::ClientNotAllowed
        );
    }

    /// TC-JWT-09
    #[tokio::test]
    async fn an_expired_token_is_rejected() {
        let v = primed(StaticSource::ok()).await;
        assert_eq!(
            v.verify(&token(valid_claims(-3600), "test-key-1")).await.unwrap_err(),
            Rejection::Expired
        );
    }

    /// TC-JWT-10 — junk is refused on shape, with no crypto performed.
    #[tokio::test]
    async fn junk_is_refused_before_any_crypto() {
        let v = primed(StaticSource::ok()).await;
        let before = v.verifications.load(Ordering::Relaxed);
        for junk in ["not.a.jwt", "", &"x".repeat(100_000)] {
            assert_eq!(v.verify(junk).await.unwrap_err(), Rejection::Shape);
        }
        assert_eq!(
            v.verifications.load(Ordering::Relaxed),
            before,
            "junk reached the signature check"
        );
    }

    /// An `oct` JWK in a public document is a signing key every reader holds.
    #[test]
    fn oct_jwks_entries_are_refused() {
        let doc = r#"{"keys":[{"kty":"oct","kid":"sym","k":"c2VjcmV0"}]}"#;
        let set = KeySet::from_jwks_json(doc).unwrap();
        assert!(
            set.is_empty(),
            "a symmetric key was accepted from JWKS; anyone who can read it can forge tokens"
        );
    }

    #[test]
    fn malformed_jwks_is_an_error_not_a_panic() {
        assert!(KeySet::from_jwks_json("not json").is_err());
        assert!(KeySet::from_jwks_json(r#"{"nope":1}"#).is_err());
        assert!(KeySet::from_jwks_json(r#"{"keys":[]}"#).unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_verified_token_is_served_from_cache() {
        let v = primed(StaticSource::ok()).await;
        let t = token(valid_claims(600), "test-key-1");
        v.verify(&t).await.unwrap();
        let after_first = v.verifications.load(Ordering::Relaxed);
        v.verify(&t).await.unwrap();
        assert_eq!(
            v.verifications.load(Ordering::Relaxed),
            after_first,
            "a cached token was re-verified"
        );
    }
}
