//! What a verified identity may reach.
//!
//! Two tiers, matching the line budapp already draws with `user_type == CLIENT`:
//!
//! * `user_projects:{sub}` present and project-scoped -> that user's project deployments.
//! * absent, or marked published-only -> the published overlay, the same non-per-token surface
//!   client keys already get.
//!
//! **This module reads published state; it does not reimplement Bud policy.** Absent,
//! unparseable, or an unexpected shape all fall to the published overlay — never to wider
//! access — so a Bud-side policy change degrades WaaV toward *less* access, which fails safe.

use std::sync::Arc;

use crate::store::{ControlPlaneStore, StoreError};
use crate::types::{AliasMap, UserProjects};

pub const USER_PROJECTS_PREFIX: &str = "user_projects:";
pub const PROJECT_MODELS_PREFIX: &str = "project_models:";

/// Which tier a resolution landed in, and whether it may be cached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthzTier {
    /// The user's own project deployments.
    ProjectScoped,
    /// The published overlay, reached authoritatively — cacheable.
    Published,
    /// The published overlay, reached because the lookup FAILED.
    ///
    /// Invariant 5: this must never be cached. Caching a degraded answer turns a momentary
    /// Redis wobble into a full `authz_ttl` of silently reduced access.
    PublishedDegraded,
}

impl AuthzTier {
    pub fn is_cacheable(self) -> bool {
        !matches!(self, Self::PublishedDegraded)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProjectScoped => "project_scoped",
            Self::Published => "published",
            Self::PublishedDegraded => "published_degraded",
        }
    }
}

pub struct Resolution {
    pub aliases: Arc<AliasMap>,
    pub tier: AuthzTier,
}

/// Resolve what `sub` may reach.
///
/// `published` is the overlay every caller can see; it is passed in rather than read here so a
/// single cached copy is shared across every resolution.
pub async fn resolve(
    store: &dyn ControlPlaneStore,
    sub: &str,
    published: Arc<AliasMap>,
) -> Resolution {
    let degraded = |_e: StoreError| Resolution {
        aliases: Arc::clone(&published),
        tier: AuthzTier::PublishedDegraded,
    };

    let raw = match store.get(&format!("{USER_PROJECTS_PREFIX}{sub}")).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "authz: user_projects lookup failed; degrading for this request only");
            return degraded(e);
        }
    };

    let Some(raw) = raw else {
        // Genuinely absent — the steady state for a client-shaped user, and for every user
        // until budapp starts publishing. Authoritative, so cacheable.
        return Resolution {
            aliases: published,
            tier: AuthzTier::Published,
        };
    };

    let blob: UserProjects = match serde_json::from_str(&raw) {
        Ok(b) => b,
        Err(e) => {
            // An unrecognised shape is a Bud-side change we do not understand. Fail toward
            // LESS access, and do not cache the conclusion — the next deploy may fix it.
            tracing::error!(error = %e, "authz: unparseable user_projects blob; falling back to published");
            return Resolution {
                aliases: published,
                tier: AuthzTier::PublishedDegraded,
            };
        }
    };

    if blob.is_published_only() {
        return Resolution {
            aliases: published,
            tier: AuthzTier::Published,
        };
    }

    // Project-scoped: union the `project_models:{id}` blobs budapp already maintains. Reading
    // them directly is what removes a per-token fan-out to budapp.
    let mut union = AliasMap::new();
    for project in &blob.projects {
        match store
            .get(&format!("{PROJECT_MODELS_PREFIX}{project}"))
            .await
        {
            Ok(Some(raw)) => match crate::hydrate::parse_api_key_blob(&raw) {
                Ok((aliases, _)) => union.extend(aliases),
                Err(e) => {
                    tracing::warn!(project = %project, error = %e, "authz: unparseable project_models blob");
                }
            },
            Ok(None) => {}
            Err(e) => {
                // A partial union is a WRONG answer, not a smaller one: it would silently deny
                // a project the user really has. Degrade wholesale instead.
                tracing::warn!(project = %project, error = %e, "authz: project_models lookup failed");
                return degraded(e);
            }
        }
    }

    Resolution {
        aliases: Arc::new(union),
        tier: AuthzTier::ProjectScoped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::MemoryStore;
    use crate::types::AliasMetadata;

    fn overlay() -> Arc<AliasMap> {
        let mut m = AliasMap::new();
        m.insert(
            "published-tts".into(),
            AliasMetadata {
                endpoint_id: Some("pub-1".into()),
                ..Default::default()
            },
        );
        Arc::new(m)
    }

    fn project_blob(alias: &str) -> String {
        format!(r#"{{"{alias}":{{"endpoint_id":"e-{alias}","project_id":"p1"}}}}"#)
    }

    /// TC-JWT-12
    #[tokio::test]
    async fn published_only_users_see_only_the_overlay() {
        let store = MemoryStore::new();
        store.set(
            "user_projects:u1",
            r#"{"user_id":"u1","published_only":true,"projects":["p1"]}"#,
        );
        let r = resolve(&store, "u1", overlay()).await;
        assert_eq!(r.tier, AuthzTier::Published);
        assert!(r.aliases.contains_key("published-tts"));
    }

    #[tokio::test]
    async fn an_absent_blob_is_authoritative_and_cacheable() {
        let store = MemoryStore::new();
        let r = resolve(&store, "nobody", overlay()).await;
        assert_eq!(r.tier, AuthzTier::Published);
        assert!(
            r.tier.is_cacheable(),
            "an authoritative absence must be cacheable"
        );
    }

    #[tokio::test]
    async fn project_scoped_users_get_the_union_of_their_projects() {
        let store = MemoryStore::new();
        store.set(
            "user_projects:u1",
            r#"{"user_id":"u1","published_only":false,"projects":["p1","p2"]}"#,
        );
        store.set("project_models:p1", &project_blob("alpha"));
        store.set("project_models:p2", &project_blob("beta"));

        let r = resolve(&store, "u1", overlay()).await;

        assert_eq!(r.tier, AuthzTier::ProjectScoped);
        assert!(r.aliases.contains_key("alpha"));
        assert!(r.aliases.contains_key("beta"));
    }

    /// TC-JWT-05 — invariant 5. The whole point of the `PublishedDegraded` tier.
    #[tokio::test]
    async fn a_transient_failure_degrades_but_is_not_cacheable() {
        let store = MemoryStore::new();
        store.set_down(true);

        let r = resolve(&store, "u1", overlay()).await;

        assert_eq!(r.tier, AuthzTier::PublishedDegraded);
        assert!(
            !r.tier.is_cacheable(),
            "a degraded authorization was marked cacheable; a Redis wobble becomes a TTL of reduced access"
        );
    }

    /// The next request must retry rather than inherit the degraded answer.
    #[tokio::test]
    async fn the_next_request_after_a_wobble_gets_the_full_tier() {
        let store = MemoryStore::new();
        store.set(
            "user_projects:u1",
            r#"{"user_id":"u1","published_only":false,"projects":["p1"]}"#,
        );
        store.set("project_models:p1", &project_blob("alpha"));

        store.set_down(true);
        assert_eq!(
            resolve(&store, "u1", overlay()).await.tier,
            AuthzTier::PublishedDegraded
        );

        store.set_down(false);
        let r = resolve(&store, "u1", overlay()).await;
        assert_eq!(r.tier, AuthzTier::ProjectScoped);
        assert!(r.aliases.contains_key("alpha"));
    }

    /// TC-JWT-13 — an unrecognised shape must fail toward LESS access.
    #[tokio::test]
    async fn an_unknown_blob_shape_falls_back_to_published() {
        let store = MemoryStore::new();
        store.set(
            "user_projects:u1",
            r#"{"totally":"different","shape":[1,2]}"#,
        );

        let r = resolve(&store, "u1", overlay()).await;

        assert!(
            matches!(r.tier, AuthzTier::Published | AuthzTier::PublishedDegraded),
            "an unknown blob shape widened access instead of narrowing it"
        );
        assert!(r.aliases.contains_key("published-tts"));
        assert_eq!(r.aliases.len(), 1, "the overlay must not gain entries");
    }

    #[tokio::test]
    async fn outright_garbage_falls_back_and_is_not_cached() {
        let store = MemoryStore::new();
        store.set("user_projects:u1", "}{not json");
        let r = resolve(&store, "u1", overlay()).await;
        assert_eq!(r.tier, AuthzTier::PublishedDegraded);
        assert!(!r.tier.is_cacheable());
    }

    /// A user scoped to an empty project list must not silently receive the project tier.
    #[tokio::test]
    async fn an_empty_project_list_lands_in_the_published_tier() {
        let store = MemoryStore::new();
        store.set(
            "user_projects:u1",
            r#"{"user_id":"u1","published_only":false,"projects":[]}"#,
        );
        assert_eq!(
            resolve(&store, "u1", overlay()).await.tier,
            AuthzTier::Published
        );
    }

    /// A partial union is a wrong answer, not a smaller one.
    #[tokio::test]
    async fn a_failed_project_read_degrades_wholesale() {
        struct FlakyStore(MemoryStore);
        #[async_trait::async_trait]
        impl ControlPlaneStore for FlakyStore {
            async fn get(&self, key: &str) -> Result<Option<String>, StoreError> {
                if key == "project_models:p2" {
                    return Err(StoreError::Unavailable("flaky".into()));
                }
                self.0.get(key).await
            }
            async fn scan(
                &self,
                p: &str,
            ) -> Result<std::collections::HashMap<String, String>, StoreError> {
                self.0.scan(p).await
            }
        }

        let inner = MemoryStore::new();
        inner.set(
            "user_projects:u1",
            r#"{"user_id":"u1","published_only":false,"projects":["p1","p2"]}"#,
        );
        inner.set("project_models:p1", &project_blob("alpha"));
        let store = FlakyStore(inner);

        let r = resolve(&store, "u1", overlay()).await;

        assert_eq!(
            r.tier,
            AuthzTier::PublishedDegraded,
            "a partial union was returned; the user would be silently denied a project they hold"
        );
    }
}
