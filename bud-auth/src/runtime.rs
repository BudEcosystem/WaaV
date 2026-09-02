//! The plane: boot, the keyspace loop, and request-time resolution.
//!
//! This is where the pieces compose into the thing WaaV's middleware actually calls. It owns
//! the two `hydrate_all` call sites that rule (2) in `hydrate.rs` requires — boot and
//! reconnect — and the readiness signal that stops a pod serving 401s while it waits for its
//! first sweep.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::authz::{self, AuthzTier};
use crate::guards::{Denied, MissGuards};
use crate::hash::hash_api_key;
use crate::hydrate::{self, HydrationStats, KeyEvent};
use crate::jwt::JwtVerifier;
use crate::snapshot::BudAuth;
use crate::store::ControlPlaneStore;
use crate::types::AliasMap;

/// Who is calling, once resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Principal {
    /// Bud project this call is attributed to.
    pub project_id: Option<String>,
    pub api_key_id: Option<String>,
    pub user_id: Option<String>,
    /// How the caller proved identity.
    pub via: PrincipalKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrincipalKind {
    ApiKey,
    Jwt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthFailure {
    /// No credential presented.
    Missing,
    /// Presented but not resolvable.
    Unauthorized,
    /// Refused by a guard rather than by the credential itself.
    Throttled,
    /// The plane has not completed its first hydration.
    NotReady,
}

/// Everything the request path needs, assembled once at startup.
pub struct BudPlane {
    pub auth: BudAuth,
    pub guards: Arc<MissGuards>,
    store: Arc<dyn ControlPlaneStore>,
    jwt: Option<Arc<JwtVerifier>>,
    /// Opens RSA-encrypted vendor credentials at hydration. Absent means encrypted credentials
    /// cannot be used — plaintext and keyless endpoints still work.
    decryptor: crate::credentials::CredentialDecryptor,
    /// The published overlay: what any authenticated caller may reach.
    published: arc_swap::ArcSwap<AliasMap>,
    /// Readiness. A pod that answers before its first sweep answers 401 to valid keys.
    hydrated: AtomicBool,
    /// Millis since `origin` of the last keyspace event, stored **plus one** so that `0` can
    /// mean "no event yet" without colliding with a genuine event at t=0 — which is exactly
    /// what happens on a fast boot, and made this gauge read "never" forever.
    last_event_ms_plus_one: AtomicU64,
    origin: Instant,
}

impl BudPlane {
    pub fn new(store: Arc<dyn ControlPlaneStore>, jwt: Option<Arc<JwtVerifier>>) -> Self {
        Self::with_decryptor(
            store,
            jwt,
            crate::credentials::CredentialDecryptor::disabled(),
        )
    }

    pub fn with_decryptor(
        store: Arc<dyn ControlPlaneStore>,
        jwt: Option<Arc<JwtVerifier>>,
        decryptor: crate::credentials::CredentialDecryptor,
    ) -> Self {
        Self {
            decryptor,
            auth: BudAuth::new(),
            guards: Arc::new(MissGuards::default()),
            store,
            jwt,
            published: arc_swap::ArcSwap::from_pointee(AliasMap::new()),
            hydrated: AtomicBool::new(false),
            last_event_ms_plus_one: AtomicU64::new(0),
            origin: Instant::now(),
        }
    }

    /// Boot — the FIRST of the two required `hydrate_all` call sites.
    pub async fn boot(&self) -> Result<HydrationStats, crate::store::StoreError> {
        let stats = hydrate::hydrate_all(self.store.as_ref(), &self.auth).await?;
        // The voice table is swept in the same breath. Booting with credentials but no
        // endpoints would authenticate every caller and then 404 every audio request.
        let voice =
            hydrate::hydrate_voice_table(self.store.as_ref(), &self.auth, &self.decryptor).await?;
        tracing::info!(
            voice_endpoints = voice.endpoints,
            voice_skipped = voice.skipped,
            "voice table hydrated"
        );
        self.hydrated.store(true, Ordering::SeqCst);
        Ok(stats)
    }

    /// Reconnect — the SECOND required call site. Losing this is invisible until a connection
    /// drops in production, which is why `hydrate_all_has_three_call_sites` asserts it exists.
    pub async fn rehydrate_after_reconnect(
        &self,
    ) -> Result<HydrationStats, crate::store::StoreError> {
        tracing::info!("control-plane connection restored; re-hydrating");
        let stats = hydrate::hydrate_all(self.store.as_ref(), &self.auth).await?;
        let _ =
            hydrate::hydrate_voice_table(self.store.as_ref(), &self.auth, &self.decryptor).await?;
        self.hydrated.store(true, Ordering::SeqCst);
        Ok(stats)
    }

    /// True once the first sweep has completed. Wire this to the readiness probe.
    pub fn is_ready(&self) -> bool {
        self.hydrated.load(Ordering::SeqCst)
    }

    pub fn seconds_since_last_event(&self) -> Option<f64> {
        let stored = self.last_event_ms_plus_one.load(Ordering::Relaxed);
        if stored == 0 {
            return None;
        }
        let last = stored - 1;
        Some((self.origin.elapsed().as_millis() as u64).saturating_sub(last) as f64 / 1000.0)
    }

    /// Feed one keyspace notification into the snapshot.
    pub async fn on_key_event(
        &self,
        key: &str,
        event: KeyEvent,
    ) -> Result<(), crate::store::StoreError> {
        self.last_event_ms_plus_one.store(
            self.origin.elapsed().as_millis() as u64 + 1,
            Ordering::Relaxed,
        );
        hydrate::apply_key_event(
            self.store.as_ref(),
            &self.auth,
            &self.guards,
            &self.decryptor,
            key,
            event,
        )
        .await?;
        Ok(())
    }

    /// Resolve a voice endpoint by id. Hot path for every audio request; no I/O.
    pub fn voice_endpoint(&self, endpoint_id: &str) -> Option<crate::credentials::VoiceEndpoint> {
        self.auth.voice_endpoint(endpoint_id).map(|e| (*e).clone())
    }

    pub fn set_published_overlay(&self, overlay: AliasMap) {
        self.published.store(Arc::new(overlay));
    }

    /// Resolve a raw bearer to a principal.
    ///
    /// The hot path for API keys is a hash and a map probe. Everything slower is behind a
    /// guard, and nothing here calls budapp.
    pub async fn authenticate(&self, raw: &str) -> Result<Principal, AuthFailure> {
        if raw.trim().is_empty() {
            return Err(AuthFailure::Missing);
        }
        if !self.is_ready() {
            return Err(AuthFailure::NotReady);
        }

        let hashed = hash_api_key(raw);

        // 1. In-memory hit — the overwhelmingly common case. No I/O.
        if self.auth.resolve(&hashed).is_some() {
            let md = self.auth.metadata(&hashed);
            return Ok(Principal {
                project_id: md.as_ref().and_then(|m| m.api_key_project_id.clone()),
                api_key_id: md.as_ref().and_then(|m| m.api_key_id.clone()),
                user_id: md.as_ref().and_then(|m| m.user_id.clone()),
                via: PrincipalKind::ApiKey,
            });
        }

        // 2. JWT — verified locally, never escalated to a backend.
        if matches!(
            crate::guards::KeyShape::classify(raw),
            crate::guards::KeyShape::Jwt
        ) {
            if let Some(jwt) = &self.jwt {
                return match jwt.verify(raw).await {
                    Ok(identity) => {
                        let resolution = match jwt.cached_authz(&identity.sub) {
                            Some(entry) => authz::Resolution {
                                aliases: entry.aliases,
                                tier: AuthzTier::ProjectScoped,
                            },
                            None => {
                                let r = authz::resolve(
                                    self.store.as_ref(),
                                    &identity.sub,
                                    self.published.load_full(),
                                )
                                .await;
                                // Invariant 5 enforced at the call site: only an authoritative
                                // resolution is stored.
                                if r.tier.is_cacheable() {
                                    jwt.store_authz(&identity.sub, Arc::clone(&r.aliases));
                                }
                                r
                            }
                        };
                        let _ = resolution;
                        Ok(Principal {
                            project_id: None,
                            api_key_id: None,
                            user_id: Some(identity.sub),
                            via: PrincipalKind::Jwt,
                        })
                    }
                    Err(_) => Err(AuthFailure::Unauthorized),
                };
            }
            return Err(AuthFailure::Unauthorized);
        }

        // 3. Miss escalation — heals gateway/Redis drift. Guarded on all four axes.
        match self.guards.try_escalate(raw, &hashed) {
            Ok(_permit) => {
                match self
                    .store
                    .get(&format!("{}{}", hydrate::API_KEY_PREFIX, hashed))
                    .await
                {
                    Ok(Some(raw_blob)) => {
                        self.guards.record_backend_success();
                        match hydrate::parse_api_key_blob(&raw_blob) {
                            Ok((aliases, md)) => {
                                self.auth.apply([crate::snapshot::Mutation::Upsert {
                                    hashed_key: Arc::from(hashed.as_str()),
                                    aliases: Arc::new(aliases),
                                    metadata: md.clone().map(Arc::new),
                                }]);
                                Ok(Principal {
                                    project_id: md
                                        .as_ref()
                                        .and_then(|m| m.api_key_project_id.clone()),
                                    api_key_id: md.as_ref().and_then(|m| m.api_key_id.clone()),
                                    user_id: md.as_ref().and_then(|m| m.user_id.clone()),
                                    via: PrincipalKind::ApiKey,
                                })
                            }
                            Err(_) => Err(AuthFailure::Unauthorized),
                        }
                    }
                    Ok(None) => {
                        // AUTHORITATIVELY absent — safe to remember.
                        self.guards.record_backend_success();
                        self.guards.record_absent(&hashed);
                        Err(AuthFailure::Unauthorized)
                    }
                    Err(_) => {
                        // Unavailable is NOT absent. Never negatively cache this.
                        self.guards.record_backend_failure();
                        Err(AuthFailure::Unauthorized)
                    }
                }
            }
            Err(Denied::Shape) | Err(Denied::NegativeCache) => Err(AuthFailure::Unauthorized),
            Err(Denied::Overloaded) | Err(Denied::CircuitOpen) => Err(AuthFailure::Throttled),
        }
    }

    /// May this principal use this endpoint alias?
    pub fn is_authorized(&self, raw: &str, alias: &str) -> bool {
        let hashed = hash_api_key(raw);
        if self.auth.lookup_alias(&hashed, alias).is_some() {
            return true;
        }
        if let Some(jwt) = &self.jwt
            && let Some(identity) = jwt.cached_identity(&hashed)
            && let Some(entry) = jwt.cached_authz(&identity.sub)
        {
            return entry.aliases.contains_key(alias);
        }
        self.published.load().contains_key(alias)
    }
}

/// Supervise the control-plane connection: boot, then reconnect-with-backoff, re-hydrating
/// every time the connection comes back.
pub async fn supervise_reconnect<F, Fut>(
    plane: Arc<BudPlane>,
    mut connect: F,
    max_backoff: Duration,
) where
    F: FnMut() -> Fut + Send,
    Fut: std::future::Future<Output = Result<(), String>> + Send,
{
    let mut backoff = Duration::from_secs(1);
    loop {
        match connect().await {
            Ok(()) => {
                backoff = Duration::from_secs(1);
                if let Err(e) = plane.rehydrate_after_reconnect().await {
                    tracing::error!(error = %e, "re-hydration after reconnect failed");
                }
            }
            Err(e) => {
                tracing::error!(error = %e, backoff_secs = backoff.as_secs(), "control-plane connection failed");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(max_backoff);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::MemoryStore;

    fn blob() -> String {
        r#"{"tts":{"endpoint_id":"e1","project_id":"p1"},"__metadata__":{"api_key_id":"ak1","user_id":"u1","api_key_project_id":"p1"}}"#.to_string()
    }

    async fn plane_with(keys: &[(&str, String)]) -> (Arc<MemoryStore>, Arc<BudPlane>) {
        let store = Arc::new(MemoryStore::new());
        for (k, v) in keys {
            store.set(k, v);
        }
        let plane = Arc::new(BudPlane::new(
            Arc::clone(&store) as Arc<dyn ControlPlaneStore>,
            None,
        ));
        plane.boot().await.unwrap();
        (store, plane)
    }

    /// TC-AUTH-02 / TC-AUTH-07 at the plane level.
    #[tokio::test]
    async fn a_known_key_authenticates_with_attribution() {
        let hashed = hash_api_key("bud_live");
        let (_s, plane) = plane_with(&[(&format!("api_key:{hashed}"), blob())]).await;

        let p = plane.authenticate("bud_live").await.unwrap();

        assert_eq!(p.via, PrincipalKind::ApiKey);
        assert_eq!(p.project_id.as_deref(), Some("p1"));
        assert_eq!(p.api_key_id.as_deref(), Some("ak1"));
    }

    /// TC-AUTH-09 — the whole point of the design: the hot path performs no store I/O.
    #[tokio::test]
    async fn a_hit_performs_no_store_io() {
        let hashed = hash_api_key("bud_live");
        let (store, plane) = plane_with(&[(&format!("api_key:{hashed}"), blob())]).await;
        let before = store.gets.load(Ordering::Relaxed);

        for _ in 0..1000 {
            plane.authenticate("bud_live").await.unwrap();
        }

        assert_eq!(
            store.gets.load(Ordering::Relaxed),
            before,
            "the hot path hit the store; 1000 authentications must cost zero reads"
        );
    }

    /// TC-HYD-08 — a pod must not answer before it has hydrated.
    #[tokio::test]
    async fn an_unhydrated_plane_reports_not_ready_rather_than_unauthorized() {
        let store = Arc::new(MemoryStore::new());
        let plane = BudPlane::new(Arc::clone(&store) as Arc<dyn ControlPlaneStore>, None);

        assert!(!plane.is_ready());
        assert_eq!(
            plane.authenticate("bud_anything").await.unwrap_err(),
            AuthFailure::NotReady,
            "an unhydrated pod 401'd a caller instead of reporting itself unready"
        );

        plane.boot().await.unwrap();
        assert!(plane.is_ready());
    }

    #[tokio::test]
    async fn an_empty_bearer_is_missing_not_unauthorized() {
        let (_s, plane) = plane_with(&[]).await;
        assert_eq!(
            plane.authenticate("   ").await.unwrap_err(),
            AuthFailure::Missing
        );
    }

    /// TC-AUTH-08 — revocation propagates through the keyspace event, not a TTL.
    #[tokio::test]
    async fn revocation_propagates_on_the_del_event() {
        let hashed = hash_api_key("bud_live");
        let key = format!("api_key:{hashed}");
        let (store, plane) = plane_with(&[(&key, blob())]).await;
        assert!(plane.authenticate("bud_live").await.is_ok());

        store.remove(&key);
        plane.on_key_event(&key, KeyEvent::Del).await.unwrap();

        assert!(
            plane.authenticate("bud_live").await.is_err(),
            "a revoked credential still authenticates"
        );
    }

    /// Escalation heals drift: a key present in the store but missed at boot.
    #[tokio::test]
    async fn a_miss_escalates_and_then_becomes_a_hit() {
        let (store, plane) = plane_with(&[]).await;
        let hashed = hash_api_key("bud_late");
        store.set(&format!("api_key:{hashed}"), &blob());

        let before = store.gets.load(Ordering::Relaxed);
        assert!(plane.authenticate("bud_late").await.is_ok());
        assert_eq!(store.gets.load(Ordering::Relaxed), before + 1);

        // Now cached: no further reads.
        assert!(plane.authenticate("bud_late").await.is_ok());
        assert_eq!(store.gets.load(Ordering::Relaxed), before + 1);
    }

    /// "Unavailable is not absent" — a transient failure must not be remembered as a denial.
    #[tokio::test]
    async fn a_transient_store_failure_is_not_negatively_cached() {
        let (store, plane) = plane_with(&[]).await;
        let hashed = hash_api_key("bud_flaky");
        store.set(&format!("api_key:{hashed}"), &blob());

        store.set_down(true);
        assert!(plane.authenticate("bud_flaky").await.is_err());

        store.set_down(false);
        assert!(
            plane.authenticate("bud_flaky").await.is_ok(),
            "a transient failure was cached as an authoritative absence"
        );
    }

    /// TC-GUARD-01 at the plane level: junk never reaches the store.
    #[tokio::test]
    async fn junk_never_reaches_the_store() {
        let (store, plane) = plane_with(&[]).await;
        let before = store.gets.load(Ordering::Relaxed);

        for junk in ["garbage", "....", &"x".repeat(50_000)] {
            assert!(plane.authenticate(junk).await.is_err());
        }

        assert_eq!(
            store.gets.load(Ordering::Relaxed),
            before,
            "junk reached the store"
        );
    }

    #[tokio::test]
    async fn authorization_checks_the_alias_allowlist() {
        let hashed = hash_api_key("bud_live");
        let (_s, plane) = plane_with(&[(&format!("api_key:{hashed}"), blob())]).await;

        assert!(plane.is_authorized("bud_live", "tts"));
        assert!(!plane.is_authorized("bud_live", "some-other-endpoint"));
    }

    #[tokio::test]
    async fn the_published_overlay_is_reachable_by_any_caller() {
        let hashed = hash_api_key("bud_live");
        let (_s, plane) = plane_with(&[(&format!("api_key:{hashed}"), blob())]).await;

        let mut overlay = AliasMap::new();
        overlay.insert("public-tts".into(), Default::default());
        plane.set_published_overlay(overlay);

        assert!(plane.is_authorized("bud_live", "public-tts"));
    }

    #[tokio::test]
    async fn staleness_gauge_tracks_events() {
        let (_s, plane) = plane_with(&[]).await;
        assert!(plane.seconds_since_last_event().is_none());
        plane
            .on_key_event("api_key:x", KeyEvent::Del)
            .await
            .unwrap();
        assert!(plane.seconds_since_last_event().is_some());
    }
}

#[cfg(test)]
mod voice_plane_tests {
    use super::*;
    use crate::store::MemoryStore;

    #[tokio::test]
    async fn boot_hydrates_voice_endpoints_alongside_credentials() {
        // Booting with credentials but no endpoints would authenticate every caller and then
        // 404 every audio request — a failure that reads as a routing problem.
        let store = Arc::new(MemoryStore::new());
        store.set(
            "api_key:h1",
            r#"{"tts":{"endpoint_id":"ep-1"},"__metadata__":{"api_key_project_id":"p1"}}"#,
        );
        store.set(
            "voice_table:ep-1",
            r#"{"ep-1":{"vendor":"deepgram","endpoints":["text_to_speech"],"voice":"aura-asteria-en"}}"#,
        );

        let plane = BudPlane::new(Arc::clone(&store) as Arc<dyn ControlPlaneStore>, None);
        plane.boot().await.unwrap();

        let ep = plane
            .voice_endpoint("ep-1")
            .expect("endpoint resolvable after boot");
        assert_eq!(ep.vendor, "deepgram");
        assert_eq!(ep.voice.as_deref(), Some("aura-asteria-en"));
    }

    #[tokio::test]
    async fn an_unknown_endpoint_resolves_to_nothing() {
        let store = Arc::new(MemoryStore::new());
        let plane = BudPlane::new(Arc::clone(&store) as Arc<dyn ControlPlaneStore>, None);
        plane.boot().await.unwrap();
        assert!(plane.voice_endpoint("no-such-endpoint").is_none());
    }

    #[tokio::test]
    async fn voice_resolution_performs_no_store_io() {
        let store = Arc::new(MemoryStore::new());
        store.set(
            "voice_table:ep-1",
            r#"{"ep-1":{"vendor":"deepgram","endpoints":["text_to_speech"]}}"#,
        );
        let plane = BudPlane::new(Arc::clone(&store) as Arc<dyn ControlPlaneStore>, None);
        plane.boot().await.unwrap();
        let before = store.gets.load(Ordering::Relaxed);

        for _ in 0..1000 {
            assert!(plane.voice_endpoint("ep-1").is_some());
        }

        assert_eq!(
            store.gets.load(Ordering::Relaxed),
            before,
            "voice resolution hit the store; it must be a map probe like credential resolution"
        );
    }

    #[tokio::test]
    async fn reconnect_rehydration_reconciles_the_voice_table_too() {
        let store = Arc::new(MemoryStore::new());
        store.set(
            "voice_table:old",
            r#"{"old":{"vendor":"deepgram","endpoints":["text_to_speech"]}}"#,
        );
        let plane = BudPlane::new(Arc::clone(&store) as Arc<dyn ControlPlaneStore>, None);
        plane.boot().await.unwrap();

        // Changes we never saw an event for.
        store.remove("voice_table:old");
        store.set(
            "voice_table:new",
            r#"{"new":{"vendor":"elevenlabs","endpoints":["text_to_speech"]}}"#,
        );

        plane.rehydrate_after_reconnect().await.unwrap();

        assert!(
            plane.voice_endpoint("new").is_some(),
            "an endpoint added during the outage is invisible"
        );
        assert!(
            plane.voice_endpoint("old").is_none(),
            "a deleted endpoint survived and would keep serving with a stale credential"
        );
    }
}

impl BudPlane {
    /// Map an alias in a caller's own allowlist to the endpoint id it points at.
    ///
    /// This is the alias → endpoint indirection *and* the authorization check in one step: a
    /// caller can only resolve aliases their key lists, so an endpoint id they merely guessed
    /// never resolves. Returns `None` when the key is unknown or does not carry the alias.
    pub fn alias_endpoint_id(&self, raw_token: &str, alias: &str) -> Option<String> {
        let hashed = hash_api_key(raw_token);
        if let Some(meta) = self.auth.lookup_alias(&hashed, alias)
            && let Some(id) = meta.endpoint_id
        {
            return Some(id);
        }
        // A JWT caller's allowlist lives in the identity-keyed cache, not the snapshot.
        let jwt = self.jwt.as_ref()?;
        let identity = jwt.cached_identity(&hashed)?;
        let entry = jwt.cached_authz(&identity.sub)?;
        entry.aliases.get(alias).and_then(|m| m.endpoint_id.clone())
    }
}

#[cfg(test)]
mod alias_tests {
    use super::*;
    use crate::store::MemoryStore;

    async fn plane_with_alias() -> (Arc<MemoryStore>, BudPlane) {
        let store = Arc::new(MemoryStore::new());
        let hashed = hash_api_key("bud_alias_test");
        store.set(
            &format!("api_key:{hashed}"),
            r#"{"tts-deepgram":{"endpoint_id":"ep-1","project_id":"p1"}}"#,
        );
        store.set(
            "voice_table:ep-1",
            r#"{"ep-1":{"vendor":"deepgram","endpoints":["text_to_speech"]}}"#,
        );
        let plane = BudPlane::new(Arc::clone(&store) as Arc<dyn ControlPlaneStore>, None);
        plane.boot().await.unwrap();
        (store, plane)
    }

    /// The indirection an end-to-end run caught and the unit tests had missed: callers name an
    /// alias, the voice table is keyed by endpoint id.
    #[tokio::test]
    async fn an_alias_resolves_to_its_endpoint_id() {
        let (_s, plane) = plane_with_alias().await;
        assert_eq!(
            plane
                .alias_endpoint_id("bud_alias_test", "tts-deepgram")
                .as_deref(),
            Some("ep-1")
        );
    }

    /// The same lookup is the authorization boundary: an endpoint id a caller merely guessed
    /// is not in their allowlist, so it does not resolve.
    #[tokio::test]
    async fn an_alias_the_key_does_not_carry_does_not_resolve() {
        let (_s, plane) = plane_with_alias().await;
        assert!(
            plane
                .alias_endpoint_id("bud_alias_test", "someone-elses-endpoint")
                .is_none(),
            "a caller resolved an alias their key does not list"
        );
    }

    #[tokio::test]
    async fn an_unknown_key_resolves_no_alias() {
        let (_s, plane) = plane_with_alias().await;
        assert!(
            plane
                .alias_endpoint_id("bud_not_a_key", "tts-deepgram")
                .is_none()
        );
    }
}
