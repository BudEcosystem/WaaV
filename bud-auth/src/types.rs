//! Wire types for the Bud control plane.
//!
//! These mirror what budapp writes into Redis. Every field is optional-shaped so a budapp
//! that predates a field, or one that ships a new one, never turns a rollout skew into an
//! auth outage — the same tolerance budgateway's `UserProjects` documents.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// One entry in an API key's alias allowlist: `api_key:{hash}` maps alias -> this.
///
/// Mirrors budgateway's `ApiKeyMetadata`. Unknown fields are IGNORED rather than rejected:
/// budgateway's model config uses `deny_unknown_fields` and drops mismatched entries with no
/// error at all, which is a documented silent-failure source. We accept and log instead
/// (see `parse::parse_api_key_blob`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AliasMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub router_id: Option<String>,
    /// `model`, `adapter`, `guardrail`, `router` or `agent`, stamped by budapp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<i64>,
}

/// The `__metadata__` block carried alongside the aliases in an `api_key:{hash}` blob.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthMetadata {
    #[serde(default)]
    pub api_key_id: Option<String>,
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub api_key_project_id: Option<String>,
}

/// A key's alias allowlist: alias name -> what it points at.
pub type AliasMap = HashMap<String, AliasMetadata>;

/// `user_projects:{sub}` — what a Keycloak subject may reach.
///
/// Per-`sub` rather than one global map: a single blob covering every user would mean one bad
/// write locks everybody out. Every field is optional-shaped for rollout-skew tolerance.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct UserProjects {
    /// budapp's `User.id` — NOT the Keycloak `sub`. Attribution joins on this.
    #[serde(default)]
    pub user_id: Option<String>,
    /// budapp's own `user_type == CLIENT` decision, made where the user record lives.
    #[serde(default)]
    pub published_only: bool,
    #[serde(default)]
    pub default_project_id: Option<String>,
    #[serde(default)]
    pub projects: Vec<String>,
}

impl UserProjects {
    /// Matches budapp's existing precedent (`get_all_active_projects(...)[0]`) rather than
    /// inventing a second rule.
    pub fn default_project(&self) -> Option<String> {
        self.default_project_id
            .clone()
            .or_else(|| self.projects.first().cloned())
    }

    /// True when this subject gets only the published overlay.
    ///
    /// An empty project list counts: a user scoped to nothing must not silently receive the
    /// project-scoped tier's behaviour.
    pub fn is_published_only(&self) -> bool {
        self.published_only || self.projects.is_empty()
    }
}

/// The identity a verified JWT resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedIdentity {
    pub sub: String,
    /// Unix seconds. Used to bound how long the verification may be cached.
    pub exp: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alias_metadata_tolerates_unknown_fields() {
        // A budapp that ships a new field must not 401 the estate.
        let v: AliasMetadata =
            serde_json::from_str(r#"{"endpoint_id":"e1","brand_new_field":42}"#).unwrap();
        assert_eq!(v.endpoint_id.as_deref(), Some("e1"));
    }

    #[test]
    fn alias_metadata_tolerates_missing_fields() {
        // A budapp that predates a field must not 401 the estate either.
        let v: AliasMetadata = serde_json::from_str("{}").unwrap();
        assert_eq!(v, AliasMetadata::default());
    }

    #[test]
    fn user_projects_tolerates_skew_in_both_directions() {
        let older: UserProjects = serde_json::from_str(r#"{"user_id":"u"}"#).unwrap();
        assert!(!older.published_only);
        assert!(older.projects.is_empty());

        let newer: UserProjects =
            serde_json::from_str(r#"{"user_id":"u","published_only":true,"projects":[],"future":1}"#)
                .unwrap();
        assert!(newer.published_only);
    }

    /// TC-JWT-12 groundwork: the published-only decision.
    #[test]
    fn empty_project_list_is_published_only() {
        let blob = UserProjects {
            user_id: Some("u".into()),
            published_only: false,
            default_project_id: None,
            projects: vec![],
        };
        assert!(
            blob.is_published_only(),
            "a user scoped to no projects must not inherit the project-scoped tier"
        );
    }

    #[test]
    fn explicit_flag_wins_over_a_populated_list() {
        let blob = UserProjects {
            user_id: Some("u".into()),
            published_only: true,
            default_project_id: None,
            projects: vec!["p1".into()],
        };
        assert!(blob.is_published_only());
    }

    #[test]
    fn default_project_prefers_the_explicit_field() {
        let blob = UserProjects {
            user_id: None,
            published_only: false,
            default_project_id: Some("explicit".into()),
            projects: vec!["first".into()],
        };
        assert_eq!(blob.default_project().as_deref(), Some("explicit"));
    }

    #[test]
    fn default_project_falls_back_to_the_first_listed() {
        let blob = UserProjects {
            user_id: None,
            published_only: false,
            default_project_id: None,
            projects: vec!["first".into(), "second".into()],
        };
        assert_eq!(blob.default_project().as_deref(), Some("first"));
    }
}
