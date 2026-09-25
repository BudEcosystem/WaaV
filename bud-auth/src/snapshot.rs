//! The in-memory auth snapshot — the hot path.
//!
//! Resolution at steady state is: hash -> atomic pointer load -> hash-map probe -> refcount
//! bump. No I/O, no locks, no allocation beyond the `Arc` clone. Everything that populates
//! this map runs off the request path (see `redis.rs`).
//!
//! Two structural traps are load-bearing here, both learned from budgateway:
//!
//! 1. **`Arc<ArcSwap<_>>`, not a bare `ArcSwap`.** `BudAuth` is `Clone` and is cloned by value
//!    into every middleware layer. A bare `ArcSwap` would make each clone an independent cell,
//!    so the Redis loop would update a map no request ever reads — estate-wide 401s with a
//!    perfectly healthy-looking Redis.
//!
//! 2. **The writer is serialised.** Copy-on-write is not atomic: two writers that load the same
//!    generation and both store silently drop one delta. A lost `Remove` leaves a revoked
//!    credential valid *forever* — hydration is additive, so nothing ever re-removes it. A lost
//!    `Upsert` 401s a valid key.

use arc_swap::ArcSwap;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::credentials::VoiceEndpoint;
use crate::types::{AliasMap, AuthMetadata};

/// One immutable generation of the auth map.
#[derive(Debug, Default)]
pub struct BudSnapshot {
    /// `sha256("bud-" + key)` -> that key's alias allowlist.
    pub api_keys: HashMap<Arc<str>, Arc<AliasMap>>,
    /// Same hashed key -> attribution.
    pub metadata: HashMap<Arc<str>, Arc<AuthMetadata>>,
    /// Endpoint id -> its voice configuration, credential already decrypted.
    ///
    /// Held in the same generation as the api-key map so a request never sees an endpoint
    /// whose credential has not been resolved yet.
    pub voice: HashMap<Arc<str>, Arc<VoiceEndpoint>>,
}

impl BudSnapshot {
    fn clone_contents(&self) -> Self {
        Self {
            api_keys: self.api_keys.clone(),
            metadata: self.metadata.clone(),
            voice: self.voice.clone(),
        }
    }
}

/// A single atomic change to the auth map.
#[derive(Debug, Clone)]
pub enum Mutation {
    /// Replace this key's alias map. `metadata: None` leaves any EXISTING attribution
    /// untouched, mirroring the Redis path where attribution is written only when the blob
    /// actually carried a `__metadata__` block.
    Upsert {
        hashed_key: Arc<str>,
        aliases: Arc<AliasMap>,
        metadata: Option<Arc<AuthMetadata>>,
    },
    /// Removes BOTH the alias map and the attribution, atomically.
    Remove { hashed_key: Arc<str> },
}

/// Shared, cloneable handle onto the auth snapshot.
#[derive(Clone)]
pub struct BudAuth {
    /// See trap (1) in the module docs. `Arc<ArcSwap<_>>` — never a bare `ArcSwap`.
    snapshot: Arc<ArcSwap<BudSnapshot>>,
    /// See trap (2). `std::sync::Mutex` is correct because no method here is `async`, so the
    /// guard can never be held across an `.await`.
    writer: Arc<Mutex<()>>,
    /// Counts published generations. Boot hydration must publish exactly ONE regardless of how
    /// many keys it loaded; applying per key would clone the whole map N times (TC-HYD-02).
    generations: Arc<std::sync::atomic::AtomicU64>,
}

impl Default for BudAuth {
    fn default() -> Self {
        Self::new()
    }
}

impl BudAuth {
    pub fn new() -> Self {
        Self {
            snapshot: Arc::new(ArcSwap::from_pointee(BudSnapshot::default())),
            writer: Arc::new(Mutex::new(())),
            generations: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }

    /// Resolve a raw bearer to its alias allowlist.
    ///
    /// THE hot path. Returns a shared `Arc`; callers that need to mutate clone the inner map
    /// explicitly, which keeps that cost visible at the call site rather than paying it on
    /// every request (TC-AUTH-04).
    pub fn resolve(&self, hashed_key: &str) -> Option<Arc<AliasMap>> {
        self.snapshot
            .load()
            .api_keys
            .get(hashed_key)
            .map(Arc::clone)
    }

    /// Attribution for a hashed key, if budapp published any.
    pub fn metadata(&self, hashed_key: &str) -> Option<Arc<AuthMetadata>> {
        self.snapshot
            .load()
            .metadata
            .get(hashed_key)
            .map(Arc::clone)
    }

    /// Look up one alias inside a key's allowlist.
    pub fn lookup_alias(
        &self,
        hashed_key: &str,
        alias: &str,
    ) -> Option<crate::types::AliasMetadata> {
        self.snapshot
            .load()
            .api_keys
            .get(hashed_key)
            .and_then(|m| m.get(alias).cloned())
    }

    /// Look up a voice endpoint by id. Hot path for every audio request.
    pub fn voice_endpoint(&self, endpoint_id: &str) -> Option<Arc<VoiceEndpoint>> {
        self.snapshot.load().voice.get(endpoint_id).map(Arc::clone)
    }

    /// Every voice endpoint in the current generation.
    ///
    /// A copy of the `Arc`s, so a long walk (the voice-catalog publisher calls a vendor per
    /// entry) never holds the snapshot and never sees a half-applied mutation.
    pub fn voice_endpoints(&self) -> Vec<(Arc<str>, Arc<VoiceEndpoint>)> {
        self.snapshot
            .load()
            .voice
            .iter()
            .map(|(id, e)| (Arc::clone(id), Arc::clone(e)))
            .collect()
    }

    pub fn voice_count(&self) -> usize {
        self.snapshot.load().voice.len()
    }

    /// Upsert or remove one voice endpoint, leaving everything else untouched.
    ///
    /// Driven by `voice_table:` keyspace events. Without this, a credential rotation or an
    /// endpoint edit would not take effect until the next reconnect — which may be never.
    pub fn mutate_voice(&self, endpoint_id: &str, endpoint: Option<Arc<VoiceEndpoint>>) {
        #[expect(
            clippy::expect_used,
            reason = "a poisoned auth writer is unrecoverable"
        )]
        let _guard = self.writer.lock().expect("bud auth writer mutex poisoned");
        let mut next = self.snapshot.load().clone_contents();
        match endpoint {
            Some(e) => {
                next.voice.insert(Arc::from(endpoint_id), e);
            }
            None => {
                next.voice.remove(endpoint_id);
            }
        }
        self.snapshot.store(Arc::new(next));
        self.generations
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// Replace the whole voice table as one generation, leaving the api-key map untouched.
    pub fn replace_voice(&self, voice: HashMap<Arc<str>, Arc<VoiceEndpoint>>) {
        #[expect(
            clippy::expect_used,
            reason = "a poisoned auth writer is unrecoverable"
        )]
        let _guard = self.writer.lock().expect("bud auth writer mutex poisoned");
        let mut next = self.snapshot.load().clone_contents();
        next.voice = voice;
        self.snapshot.store(Arc::new(next));
        self.generations
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn key_count(&self) -> usize {
        self.snapshot.load().api_keys.len()
    }

    /// How many generations have been published. Test-facing; also the cheapest signal that
    /// hydration is behaving (TC-HYD-02).
    pub fn generation(&self) -> u64 {
        self.generations.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Apply a batch of mutations as ONE new generation.
    ///
    /// Batching is not an optimisation detail — it is how boot hydration avoids cloning the
    /// whole map once per key. Every writer goes through here so the serialisation in trap (2)
    /// cannot be bypassed.
    pub fn apply(&self, mutations: impl IntoIterator<Item = Mutation>) {
        let mutations: Vec<_> = mutations.into_iter().collect();
        if mutations.is_empty() {
            return;
        }

        // Held across the whole load -> clone -> mutate -> store cycle. Without it, two
        // concurrent writers silently drop one another's deltas.
        #[expect(
            clippy::expect_used,
            reason = "a poisoned auth writer is unrecoverable"
        )]
        let _guard = self.writer.lock().expect("bud auth writer mutex poisoned");

        let mut next = self.snapshot.load().clone_contents();
        for m in mutations {
            match m {
                Mutation::Upsert {
                    hashed_key,
                    aliases,
                    metadata,
                } => {
                    next.api_keys.insert(Arc::clone(&hashed_key), aliases);
                    if let Some(md) = metadata {
                        next.metadata.insert(hashed_key, md);
                    }
                }
                Mutation::Remove { hashed_key } => {
                    next.api_keys.remove(&hashed_key);
                    next.metadata.remove(&hashed_key);
                }
            }
        }
        self.snapshot.store(Arc::new(next));
        self.generations
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// Replace the entire map with a freshly hydrated one, as a single generation.
    ///
    /// Used by boot and by post-reconnect re-hydration. Deliberately a REPLACE, not a merge:
    /// a merge could never remove a key deleted while the connection was down.
    pub fn replace_all(
        &self,
        api_keys: HashMap<Arc<str>, Arc<AliasMap>>,
        metadata: HashMap<Arc<str>, Arc<AuthMetadata>>,
    ) {
        #[expect(
            clippy::expect_used,
            reason = "a poisoned auth writer is unrecoverable"
        )]
        let _guard = self.writer.lock().expect("bud auth writer mutex poisoned");
        // Carry the voice table across: `replace_all` re-publishes the api-key generation,
        // and dropping the voice map here would blank every endpoint on the next key event.
        let voice = self.snapshot.load().voice.clone();
        self.snapshot.store(Arc::new(BudSnapshot {
            api_keys,
            metadata,
            voice,
        }));
        self.generations
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::AliasMetadata;

    fn aliases(pairs: &[(&str, &str)]) -> Arc<AliasMap> {
        let mut m = AliasMap::new();
        for (alias, endpoint) in pairs {
            m.insert(
                (*alias).to_string(),
                AliasMetadata {
                    endpoint_id: Some((*endpoint).to_string()),
                    project_id: Some("proj-1".into()),
                    ..Default::default()
                },
            );
        }
        Arc::new(m)
    }

    fn upsert(auth: &BudAuth, key: &str, pairs: &[(&str, &str)]) {
        auth.apply([Mutation::Upsert {
            hashed_key: Arc::from(key),
            aliases: aliases(pairs),
            metadata: None,
        }]);
    }

    /// TC-AUTH-02
    #[test]
    fn known_key_resolves_with_its_aliases() {
        let auth = BudAuth::new();
        upsert(&auth, "h1", &[("tts-deepgram", "e-1")]);

        let got = auth.resolve("h1").expect("seeded key must resolve");
        assert_eq!(
            got.get("tts-deepgram")
                .and_then(|a| a.endpoint_id.as_deref()),
            Some("e-1")
        );
        assert_eq!(
            auth.lookup_alias("h1", "tts-deepgram")
                .and_then(|a| a.project_id),
            Some("proj-1".to_string())
        );
    }

    /// TC-AUTH-03
    #[test]
    fn unknown_key_does_not_resolve() {
        let auth = BudAuth::new();
        upsert(&auth, "h1", &[("a", "e-1")]);
        assert!(auth.resolve("nope").is_none());
        assert!(auth.lookup_alias("h1", "not-an-alias").is_none());
    }

    /// TC-AUTH-04 — the hot path hands out a shared pointer, never a deep copy.
    ///
    /// budgateway removed exactly this clone as "the dominant per-request allocation in auth".
    #[test]
    fn resolve_shares_rather_than_clones() {
        let auth = BudAuth::new();
        upsert(&auth, "h1", &[("a", "e-1")]);

        let first = auth.resolve("h1").unwrap();
        let second = auth.resolve("h1").unwrap();
        assert!(
            Arc::ptr_eq(&first, &second),
            "resolve() deep-copied the alias map; that is the allocation this design exists to avoid"
        );
    }

    /// TC-AUTH-05 — trap (1). Every clone must observe writes through any other clone.
    ///
    /// Remove the `Arc` around `ArcSwap` and this goes red: the clone keeps its own cell, the
    /// Redis loop updates a map no request reads, and the estate 401s.
    #[test]
    fn clones_share_one_cell() {
        let auth = BudAuth::new();
        let handed_to_middleware = auth.clone();

        upsert(&auth, "h1", &[("a", "e-1")]);

        assert!(
            handed_to_middleware.resolve("h1").is_some(),
            "a clone did not observe the write; each clone owns an independent cell"
        );
    }

    /// TC-AUTH-06 — trap (2). Concurrent writers must not drop one another's deltas.
    ///
    /// The dangerous direction is a lost `Remove`: hydration is additive, so nothing ever
    /// re-removes a revoked credential and it stays valid forever.
    #[test]
    fn concurrent_writers_do_not_drop_deltas() {
        use std::thread;

        for _ in 0..40 {
            let auth = BudAuth::new();
            for i in 0..20 {
                upsert(&auth, &format!("k{i}"), &[("a", "e")]);
            }

            let a = auth.clone();
            let b = auth.clone();
            let t1 = thread::spawn(move || {
                a.apply([Mutation::Remove {
                    hashed_key: Arc::from("k7"),
                }])
            });
            let t2 = thread::spawn(move || {
                b.apply([Mutation::Upsert {
                    hashed_key: Arc::from("k99"),
                    aliases: aliases(&[("a", "e")]),
                    metadata: None,
                }])
            });
            t1.join().unwrap();
            t2.join().unwrap();

            assert!(
                auth.resolve("k7").is_none(),
                "a Remove was lost — the revoked credential is valid forever"
            );
            assert!(auth.resolve("k99").is_some(), "an Upsert was lost");
        }
    }

    /// TC-HYD-02 — a batch is one generation, not one per key.
    #[test]
    fn a_batch_publishes_exactly_one_generation() {
        let auth = BudAuth::new();
        let before = auth.generation();

        let batch: Vec<_> = (0..500)
            .map(|i| Mutation::Upsert {
                hashed_key: Arc::from(format!("k{i}").as_str()),
                aliases: aliases(&[("a", "e")]),
                metadata: None,
            })
            .collect();
        auth.apply(batch);

        assert_eq!(auth.key_count(), 500);
        assert_eq!(
            auth.generation() - before,
            1,
            "500 keys produced more than one generation; the map was cloned per key"
        );
    }

    #[test]
    fn an_empty_batch_publishes_nothing() {
        let auth = BudAuth::new();
        let before = auth.generation();
        auth.apply([]);
        assert_eq!(auth.generation(), before);
    }

    #[test]
    fn remove_clears_attribution_atomically() {
        let auth = BudAuth::new();
        auth.apply([Mutation::Upsert {
            hashed_key: Arc::from("h1"),
            aliases: aliases(&[("a", "e")]),
            metadata: Some(Arc::new(AuthMetadata {
                api_key_id: Some("ak-1".into()),
                user_id: Some("u-1".into()),
                api_key_project_id: Some("p-1".into()),
            })),
        }]);
        assert!(auth.metadata("h1").is_some());

        auth.apply([Mutation::Remove {
            hashed_key: Arc::from("h1"),
        }]);
        assert!(auth.resolve("h1").is_none());
        assert!(
            auth.metadata("h1").is_none(),
            "attribution outlived its credential"
        );
    }

    /// `metadata: None` must not wipe existing attribution — it means "unchanged", matching
    /// the Redis path where attribution is written only when the blob carried `__metadata__`.
    #[test]
    fn upsert_without_metadata_preserves_existing_attribution() {
        let auth = BudAuth::new();
        auth.apply([Mutation::Upsert {
            hashed_key: Arc::from("h1"),
            aliases: aliases(&[("a", "e")]),
            metadata: Some(Arc::new(AuthMetadata {
                api_key_id: Some("ak-1".into()),
                ..Default::default()
            })),
        }]);

        upsert(&auth, "h1", &[("b", "e2")]);

        assert_eq!(
            auth.metadata("h1").and_then(|m| m.api_key_id.clone()),
            Some("ak-1".to_string()),
            "a metadata-free upsert wiped attribution"
        );
        assert!(auth.lookup_alias("h1", "b").is_some());
        assert!(
            auth.lookup_alias("h1", "a").is_none(),
            "upsert must REPLACE the alias map, not merge into it"
        );
    }

    /// Replace is how re-hydration removes keys deleted while the connection was down.
    #[test]
    fn replace_all_drops_keys_absent_from_the_new_generation() {
        let auth = BudAuth::new();
        upsert(&auth, "stale", &[("a", "e")]);

        let mut fresh: HashMap<Arc<str>, Arc<AliasMap>> = HashMap::new();
        fresh.insert(Arc::from("current"), aliases(&[("a", "e")]));
        auth.replace_all(fresh, HashMap::new());

        assert!(
            auth.resolve("stale").is_none(),
            "re-hydration merged instead of replacing; a key deleted during the outage survived"
        );
        assert!(auth.resolve("current").is_some());
    }
}
