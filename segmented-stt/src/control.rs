//! The control record (Release 2): one Redis key in Bud's control plane that narrows the rollout
//! switch on a running gateway, without a restart and without touching a live call.
//!
//! ```json
//! {"disabled_deployments": ["stt-scribe-prod", "8c1f…"], "disabled_rows": ["openai:gpt-transcribe", "groq:*"]}
//! ```
//!
//! It is read at session start: a session whose deployment (name or id) or capability row is listed
//! is treated as one the switch does not cover, so it takes today's path or today's refusal. It can
//! only switch the engine off, never on. A record that is absent, empty or unreadable disables
//! nothing.

use std::collections::HashSet;

use serde::Deserialize;

/// The Redis key Bud writes.
pub const CONTROL_KEY: &str = "waav:stt_live:control";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ControlRecord {
    deployments: HashSet<String>,
    rows: HashSet<String>,
    providers: HashSet<String>,
}

#[derive(Deserialize)]
struct Wire {
    #[serde(default)]
    disabled_deployments: Vec<String>,
    #[serde(default)]
    disabled_rows: Vec<String>,
}

impl ControlRecord {
    /// Parse the record. Entries are compared without case; a row entry `provider:*` disables every
    /// row of that provider.
    pub fn parse(json: &str) -> Result<Self, String> {
        let w: Wire =
            serde_json::from_str(json).map_err(|e| format!("unreadable control record: {e}"))?;
        let mut out = Self::default();
        for d in w.disabled_deployments {
            let d = d.trim().to_lowercase();
            if !d.is_empty() {
                out.deployments.insert(d);
            }
        }
        for r in w.disabled_rows {
            let r = r.trim().to_lowercase();
            match r.strip_suffix(":*") {
                Some("") => {}
                Some(p) => {
                    out.providers.insert(p.to_string());
                }
                None if !r.is_empty() => {
                    out.rows.insert(r);
                }
                None => {}
            }
        }
        Ok(out)
    }

    pub fn is_empty(&self) -> bool {
        self.deployments.is_empty() && self.rows.is_empty() && self.providers.is_empty()
    }

    /// Whether the record switches the engine off for this session. `deployment` are the names a
    /// Bud deployment goes by (its name and its id); `row_id` is the capability row matched.
    pub fn disables(&self, deployment: &[&str], row_id: &str) -> bool {
        let row = row_id.trim().to_lowercase();
        let provider = row.split(':').next().unwrap_or_default();
        deployment
            .iter()
            .any(|d| self.deployments.contains(&d.trim().to_lowercase()))
            || self.rows.contains(&row)
            || self.providers.contains(provider)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_listed_deployment_row_or_provider_is_switched_off() {
        let c = ControlRecord::parse(
            r#"{"disabled_deployments": ["STT-Scribe-Prod"], "disabled_rows": ["openai:gpt-transcribe", "groq:*"]}"#,
        )
        .unwrap();
        assert!(c.disables(&["stt-scribe-prod", "id-1"], "elevenlabs:scribe_v2"));
        assert!(c.disables(&["id-1"], "openai:gpt-transcribe"));
        assert!(c.disables(&[], "groq:whisper-large-v3-turbo"));
        assert!(!c.disables(&["other"], "openai:whisper-1"));
        assert!(!c.disables(&[], "elevenlabs:scribe_v2"));
    }

    #[test]
    fn an_empty_or_partial_record_disables_nothing() {
        assert!(ControlRecord::parse("{}").unwrap().is_empty());
        assert!(
            ControlRecord::parse(r#"{"disabled_rows": ["", ":*"]}"#)
                .unwrap()
                .is_empty()
        );
        assert!(!ControlRecord::default().disables(&["x"], "openai:gpt-transcribe"));
    }

    #[test]
    fn an_unreadable_record_is_an_error_the_caller_logs() {
        assert!(ControlRecord::parse("not json").is_err());
        assert!(ControlRecord::parse(r#"{"disabled_rows": "openai:*"}"#).is_err());
    }
}
