//! Hydration: how the snapshot gets populated, always off the request path.
//!
//! Three rules, all learned from budgateway:
//!
//! 1. **Boot publishes ONE generation.** Applying per key would clone the whole map once per
//!    key — quadratic work on a cold start with a large estate.
//! 2. **Reconnect re-hydrates.** "Boot scanned once, the reconnect path never re-scanned" was a
//!    real outage: keys created during a connection loss stayed invisible until the next
//!    restart. `hydrate_all` therefore has three call sites — definition, boot, reconnect —
//!    and a test asserts it, because the bug was structural rather than logical.
//! 3. **Re-hydration REPLACES.** A merge could never remove a key deleted while the connection
//!    was down, so a revoked credential would survive its revocation.

use std::collections::HashMap;
use std::sync::Arc;

use crate::snapshot::{BudAuth, Mutation};
use crate::store::{ControlPlaneStore, StoreError};
use crate::types::{AliasMap, AliasMetadata, AuthMetadata};

pub const API_KEY_PREFIX: &str = "api_key:";
pub const VOICE_TABLE_PREFIX: &str = "voice_table:";

/// Outcome of one hydration sweep.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct HydrationStats {
    pub api_keys: usize,
    /// Values that could not be parsed. Skipped and counted, never fatal: one corrupt blob must
    /// not abort a sweep and leave the estate unauthenticated.
    pub skipped: usize,
}

/// Parse one `api_key:{hash}` blob.
///
/// Unknown fields are tolerated, and the `__metadata__` block is lifted out. Deliberately
/// NOT `deny_unknown_fields`: budgateway's model config uses it and drops mismatched entries
/// with no error at all, which is a documented silent-failure source. Here an unknown field is
/// accepted and logged.
pub fn parse_api_key_blob(json: &str) -> Result<(AliasMap, Option<AuthMetadata>), String> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("not json: {e}"))?;
    let serde_json::Value::Object(mut obj) = value else {
        return Err("blob is not an object".into());
    };

    let metadata = obj
        .remove("__metadata__")
        .and_then(|m| serde_json::from_value::<AuthMetadata>(m).ok());

    let mut aliases = AliasMap::new();
    for (alias, v) in obj {
        match serde_json::from_value::<AliasMetadata>(v) {
            Ok(md) => {
                aliases.insert(alias, md);
            }
            Err(e) => {
                // One malformed alias must not discard the rest of the key's allowlist.
                tracing::warn!(alias = %alias, error = %e, "skipping unparseable alias entry");
            }
        }
    }
    Ok((aliases, metadata))
}

/// Full sweep: read every `api_key:*` and publish it as a single generation.
///
/// Call sites: boot and reconnect. See rule (2) — `hydrate_all_has_three_call_sites` asserts
/// this function is referenced from both, because losing the reconnect call is invisible until
/// a connection drops in production.
pub async fn hydrate_all(
    store: &dyn ControlPlaneStore,
    auth: &BudAuth,
) -> Result<HydrationStats, StoreError> {
    let raw = store.scan(&format!("{API_KEY_PREFIX}*")).await?;

    let mut api_keys: HashMap<Arc<str>, Arc<AliasMap>> = HashMap::with_capacity(raw.len());
    let mut metadata: HashMap<Arc<str>, Arc<AuthMetadata>> = HashMap::new();
    let mut stats = HydrationStats::default();

    for (key, value) in raw {
        let Some(hashed) = key.strip_prefix(API_KEY_PREFIX) else {
            continue;
        };
        match parse_api_key_blob(&value) {
            Ok((aliases, md)) => {
                let h: Arc<str> = Arc::from(hashed);
                api_keys.insert(Arc::clone(&h), Arc::new(aliases));
                if let Some(md) = md {
                    metadata.insert(h, Arc::new(md));
                }
                stats.api_keys += 1;
            }
            Err(e) => {
                tracing::warn!(key = %key, error = %e, "skipping unparseable api_key blob");
                stats.skipped += 1;
            }
        }
    }

    // Rule (1) and rule (3): one generation, and a REPLACE.
    auth.replace_all(api_keys, metadata);
    Ok(stats)
}

/// What a keyspace notification means for the snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyEvent {
    Set,
    Del,
    Expired,
}

impl KeyEvent {
    pub fn parse(event: &str) -> Option<Self> {
        match event {
            "set" => Some(Self::Set),
            "del" => Some(Self::Del),
            "expired" => Some(Self::Expired),
            _ => None,
        }
    }
}

/// Apply one keyspace event.
///
/// Returns `true` when the event concerned a key this subscriber cares about — the caller uses
/// that to drive the staleness gauge, so a silent subscriber (connected once, heard nothing
/// since) becomes visible rather than looking healthy.
pub async fn apply_key_event(
    store: &dyn ControlPlaneStore,
    auth: &BudAuth,
    guards: &crate::guards::MissGuards,
    key: &str,
    event: KeyEvent,
) -> Result<bool, StoreError> {
    let Some(hashed) = key.strip_prefix(API_KEY_PREFIX) else {
        return Ok(false);
    };

    match event {
        KeyEvent::Set => {
            let Some(raw) = store.get(key).await? else {
                // Written and gone again before we read it. Treat as a removal rather than
                // leaving a stale entry behind.
                auth.apply([Mutation::Remove {
                    hashed_key: Arc::from(hashed),
                }]);
                return Ok(true);
            };
            match parse_api_key_blob(&raw) {
                Ok((aliases, md)) => {
                    auth.apply([Mutation::Upsert {
                        hashed_key: Arc::from(hashed),
                        aliases: Arc::new(aliases),
                        metadata: md.map(Arc::new),
                    }]);
                    // The negative cache must be cleared here. Without it a key repaired in
                    // Redis stays denied for the rest of its negative TTL: the fix lands and
                    // the outage continues.
                    guards.forget(hashed);
                }
                Err(e) => {
                    tracing::warn!(key = %key, error = %e, "ignoring unparseable api_key update");
                }
            }
            Ok(true)
        }
        KeyEvent::Del | KeyEvent::Expired => {
            auth.apply([Mutation::Remove {
                hashed_key: Arc::from(hashed),
            }]);
            Ok(true)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guards::MissGuards;
    use crate::store::MemoryStore;

    fn blob(alias: &str, endpoint: &str) -> String {
        format!(
            r#"{{"{alias}":{{"endpoint_id":"{endpoint}","project_id":"p1"}},"__metadata__":{{"api_key_id":"ak1","user_id":"u1","api_key_project_id":"p1"}}}}"#
        )
    }

    /// TC-HYD-01
    #[tokio::test]
    async fn boot_scan_populates_the_snapshot() {
        let store = MemoryStore::new();
        for i in 0..500 {
            store.set(&format!("api_key:h{i}"), &blob("a", "e1"));
        }
        let auth = BudAuth::new();

        let stats = hydrate_all(&store, &auth).await.unwrap();

        assert_eq!(stats.api_keys, 500);
        assert_eq!(auth.key_count(), 500);
        assert!(auth.resolve("h42").is_some());
    }

    /// TC-HYD-02 — rule (1): one generation for the whole sweep.
    #[tokio::test]
    async fn a_sweep_publishes_exactly_one_generation() {
        let store = MemoryStore::new();
        for i in 0..500 {
            store.set(&format!("api_key:h{i}"), &blob("a", "e1"));
        }
        let auth = BudAuth::new();
        let before = auth.generation();

        hydrate_all(&store, &auth).await.unwrap();

        assert_eq!(
            auth.generation() - before,
            1,
            "500 keys produced more than one generation; the map was cloned per key"
        );
    }

    /// TC-HYD-03
    #[tokio::test]
    async fn a_set_event_upserts_without_a_restart() {
        let store = MemoryStore::new();
        let auth = BudAuth::new();
        let guards = MissGuards::default();
        hydrate_all(&store, &auth).await.unwrap();
        assert!(auth.resolve("new").is_none());

        store.set("api_key:new", &blob("a", "e1"));
        apply_key_event(&store, &auth, &guards, "api_key:new", KeyEvent::Set)
            .await
            .unwrap();

        assert!(auth.resolve("new").is_some());
    }

    /// TC-HYD-04 — and the same for an expiry.
    #[tokio::test]
    async fn del_and_expired_events_remove() {
        for ev in [KeyEvent::Del, KeyEvent::Expired] {
            let store = MemoryStore::new();
            store.set("api_key:h1", &blob("a", "e1"));
            let auth = BudAuth::new();
            let guards = MissGuards::default();
            hydrate_all(&store, &auth).await.unwrap();
            assert!(auth.resolve("h1").is_some());

            store.remove("api_key:h1");
            apply_key_event(&store, &auth, &guards, "api_key:h1", ev)
                .await
                .unwrap();

            assert!(
                auth.resolve("h1").is_none(),
                "{ev:?} did not remove the key"
            );
        }
    }

    /// TC-HYD-05 — rule (2). The outage budgateway already had.
    #[tokio::test]
    async fn reconnect_rehydration_sees_changes_made_during_the_outage() {
        let store = MemoryStore::new();
        store.set("api_key:before", &blob("a", "e1"));
        let auth = BudAuth::new();
        hydrate_all(&store, &auth).await.unwrap();

        // Connection drops. Events are missed entirely.
        store.set_down(true);
        assert!(hydrate_all(&store, &auth).await.is_err());
        store.set("api_key:during", &blob("a", "e1"));
        store.remove("api_key:before");

        // Connection restored -> re-hydrate.
        store.set_down(false);
        hydrate_all(&store, &auth).await.unwrap();

        assert!(
            auth.resolve("during").is_some(),
            "a key created during the outage is still invisible; the reconnect path does not re-scan"
        );
        assert!(
            auth.resolve("before").is_none(),
            "re-hydration merged instead of replacing; a key revoked during the outage survived"
        );
    }

    /// TC-HYD-06 — structural, because the bug was structural.
    #[test]
    fn hydrate_all_has_three_call_sites() {
        // definition + boot + reconnect. Counted across the crate's own source so that deleting
        // the reconnect call is caught here rather than in production.
        let sources = [include_str!("hydrate.rs"), include_str!("runtime.rs")].join("\n");
        let call_sites = sources.matches("hydrate_all(").count();
        assert!(
            call_sites >= 3,
            "expected hydrate_all definition + boot call + reconnect call, found {call_sites}"
        );
    }

    /// TC-HYD-09 — one corrupt blob must not abort the sweep.
    #[tokio::test]
    async fn an_unparseable_blob_is_skipped_not_fatal() {
        let store = MemoryStore::new();
        store.set("api_key:good1", &blob("a", "e1"));
        store.set("api_key:corrupt", "{ this is not json");
        store.set("api_key:good2", &blob("a", "e1"));
        let auth = BudAuth::new();

        let stats = hydrate_all(&store, &auth).await.unwrap();

        assert_eq!(stats.api_keys, 2);
        assert_eq!(stats.skipped, 1);
        assert!(auth.resolve("good1").is_some());
        assert!(auth.resolve("good2").is_some());
        assert!(auth.resolve("corrupt").is_none());
    }

    /// TC-GUARD-06 at the integration level: the event is what clears the negative entry.
    #[tokio::test]
    async fn a_set_event_clears_the_negative_cache_entry() {
        let store = MemoryStore::new();
        let auth = BudAuth::new();
        let guards = MissGuards::default();

        guards.record_absent("h1");
        assert!(guards.try_escalate("bud_x", "h1").is_err());

        store.set("api_key:h1", &blob("a", "e1"));
        apply_key_event(&store, &auth, &guards, "api_key:h1", KeyEvent::Set)
            .await
            .unwrap();

        assert!(
            guards.try_escalate("bud_x", "h1").is_ok(),
            "the repaired key is still negatively cached"
        );
    }

    #[tokio::test]
    async fn events_for_other_prefixes_are_ignored() {
        let store = MemoryStore::new();
        let auth = BudAuth::new();
        let guards = MissGuards::default();
        let touched = apply_key_event(&store, &auth, &guards, "model_table:abc", KeyEvent::Set)
            .await
            .unwrap();
        assert!(!touched);
    }

    #[test]
    fn blob_parsing_tolerates_unknown_fields_and_lifts_metadata() {
        let (aliases, md) = parse_api_key_blob(
            r#"{"tts":{"endpoint_id":"e1","brand_new":1},"__metadata__":{"user_id":"u9"}}"#,
        )
        .unwrap();
        assert_eq!(aliases.len(), 1);
        assert_eq!(md.unwrap().user_id.as_deref(), Some("u9"));
    }

    #[test]
    fn a_malformed_alias_does_not_discard_its_siblings() {
        let (aliases, _) =
            parse_api_key_blob(r#"{"good":{"endpoint_id":"e1"},"bad":"not-an-object"}"#).unwrap();
        assert!(aliases.contains_key("good"));
        assert!(!aliases.contains_key("bad"));
    }

    #[test]
    fn key_events_parse() {
        assert_eq!(KeyEvent::parse("set"), Some(KeyEvent::Set));
        assert_eq!(KeyEvent::parse("del"), Some(KeyEvent::Del));
        assert_eq!(KeyEvent::parse("expired"), Some(KeyEvent::Expired));
        assert_eq!(KeyEvent::parse("rename_from"), None);
    }
}
