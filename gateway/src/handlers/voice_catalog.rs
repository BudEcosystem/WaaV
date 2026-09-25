//! Publishes each text-to-speech deployment's voice list to Redis, for budapp's settings page.
//!
//! The settings page needs the voices a deployment's ACCOUNT can use — to offer them in the
//! default-voice picker, and to know whether describing a voice (gender, age, accent) can do
//! anything at all. Listing them takes the vendor credential in plaintext, and only WaaV holds
//! that: budapp publishes it as RSA ciphertext and, by design, never handles the key itself. So
//! WaaV lists the voices and writes the result beside the `voice_table:` entry it came from, as
//! `voice_catalog:{endpoint_id}`; budapp reads it back.
//!
//! The entry says why when there is no list, because the two reasons call for different fixes:
//! a vendor WaaV cannot list at all (`unsupported`), and a key the vendor refused to list with
//! (`unavailable` — an ElevenLabs key without `voices_read` is the common case).
//!
//! Refreshed on a short tick for deployments that are new or whose vendor/model/credential
//! changed, and otherwise every ten minutes. Every replica publishes; the entries are identical,
//! so the last write winning is harmless.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use tracing::{debug, warn};

use crate::handlers::voices::{Voice, fetch_catalog_with_key};
use crate::state::AppState;

/// Redis key prefix, beside `voice_table:`. budapp reads `voice_catalog:{endpoint_id}`.
pub const CATALOG_KEY_PREFIX: &str = "voice_catalog:";

/// How often the publisher looks for deployments that are new or changed.
const TICK: Duration = Duration::from_secs(30);
/// How long a published list is trusted before it is fetched again.
const REFRESH: Duration = Duration::from_secs(600);
/// Expiry on each entry, so a deleted deployment's list ages out.
const TTL_SECS: u64 = 1800;

/// Whether a list could be produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogStatus {
    /// `voices` is the account's list.
    Ok,
    /// The vendor can be listed, but refused this deployment's key (or returned nothing).
    Unavailable,
    /// WaaV has no way to list this vendor's voices with a deployment's own key.
    Unsupported,
}

/// One `voice_catalog:{endpoint_id}` entry. Never contains the credential.
#[derive(Debug, Clone, Serialize)]
pub struct PublishedCatalog {
    pub endpoint_id: String,
    pub vendor: String,
    pub model: Option<String>,
    pub status: CatalogStatus,
    /// Why there is no list, in words an operator can act on. `None` when `status` is `ok`.
    pub reason: Option<String>,
    /// Whether a voice DESCRIPTION can be resolved for this deployment. True exactly when the
    /// descriptor resolver would see a non-empty list — the same fetch, the same key. OpenAI's
    /// fixed voices are listed for picking but carry no metadata to match a description against.
    pub descriptor_matching: bool,
    pub voices: Vec<Voice>,
    /// Unix seconds.
    pub fetched_at: u64,
}

/// Build the entry for one deployment.
pub async fn build_catalog(
    endpoint_id: &str,
    vendor: &str,
    model: Option<String>,
    credential: Option<&str>,
) -> PublishedCatalog {
    let entry = |status, reason: Option<String>, descriptor_matching, voices| PublishedCatalog {
        endpoint_id: endpoint_id.to_string(),
        vendor: vendor.to_string(),
        model: model.clone(),
        status,
        reason,
        descriptor_matching,
        voices,
        fetched_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default(),
    };

    // OpenAI publishes no voice-list API; its voices are a fixed set WaaV already validates
    // against. Listed so the picker has them, not matchable because they carry no metadata.
    if matches!(vendor, "openai" | "openai-tts") {
        let voices = waav_openai_audio::speech::OPENAI_VOICES
            .iter()
            .map(|id| Voice {
                id: id.to_string(),
                name: id.to_string(),
                ..Default::default()
            })
            .collect();
        return entry(CatalogStatus::Ok, None, false, voices);
    }

    let key = credential.map(str::trim).filter(|k| !k.is_empty());
    let Some(key) = key else {
        return entry(
            CatalogStatus::Unavailable,
            Some("the deployment has no credential to list voices with".to_string()),
            false,
            Vec::new(),
        );
    };
    match fetch_catalog_with_key(vendor, key).await {
        None => entry(
            CatalogStatus::Unsupported,
            Some(format!(
                "voices cannot be listed for {vendor} deployments; enter a voice id"
            )),
            false,
            Vec::new(),
        ),
        Some(Ok(voices)) if !voices.is_empty() => entry(CatalogStatus::Ok, None, true, voices),
        Some(Ok(_)) => entry(
            CatalogStatus::Unavailable,
            Some(format!(
                "{vendor} returned no voices for this deployment's key"
            )),
            false,
            Vec::new(),
        ),
        Some(Err(e)) => entry(
            CatalogStatus::Unavailable,
            Some(format!(
                "{vendor} would not list voices for this deployment's key: {e}"
            )),
            false,
            Vec::new(),
        ),
    }
}

/// Start the publisher, when WaaV runs under the Bud control plane.
pub fn spawn(state: Arc<AppState>) -> Option<tokio::task::JoinHandle<()>> {
    let bud = state.bud_mode.clone()?;
    Some(tokio::spawn(async move {
        let mut published: HashMap<String, (u64, Instant)> = HashMap::new();
        let mut tick = tokio::time::interval(TICK);
        loop {
            tick.tick().await;
            publish_due(&bud, &mut published).await;
        }
    }))
}

/// A fingerprint of what the list depends on. A changed credential, vendor or model republishes
/// at once rather than after the refresh interval. Hashed in memory only; never written out.
fn fingerprint(vendor: &str, model: Option<&str>, credential: Option<&str>) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (vendor, model, credential).hash(&mut h);
    h.finish()
}

async fn publish_due(
    bud: &crate::auth::bud_mode::BudMode,
    published: &mut HashMap<String, (u64, Instant)>,
) {
    let endpoints = bud.plane().auth.voice_endpoints();
    let live: HashSet<&str> = endpoints.iter().map(|(id, _)| &**id).collect();
    published.retain(|id, _| live.contains(id.as_str()));

    for (id, ep) in &endpoints {
        // A voice list is a synthesis concept; a transcription-only deployment has none.
        if !ep.endpoints.iter().any(|c| c == "text_to_speech") {
            continue;
        }
        let fp = fingerprint(&ep.vendor, ep.model.as_deref(), ep.credential.as_deref());
        if let Some((seen, at)) = published.get(&**id)
            && *seen == fp
            && at.elapsed() < REFRESH
        {
            continue;
        }
        let catalog =
            build_catalog(id, &ep.vendor, ep.model.clone(), ep.credential.as_deref()).await;
        let Ok(json) = serde_json::to_string(&catalog) else {
            continue;
        };
        match bud
            .store()
            .set_ex(&format!("{CATALOG_KEY_PREFIX}{id}"), &json, TTL_SECS)
            .await
        {
            Ok(()) => {
                debug!(
                    endpoint_id = %id,
                    vendor = %ep.vendor,
                    status = ?catalog.status,
                    voices = catalog.voices.len(),
                    "published voice catalog"
                );
                published.insert(id.to_string(), (fp, Instant::now()));
            }
            // Not marked published: the next tick retries.
            Err(e) => warn!(endpoint_id = %id, error = %e, "could not publish voice catalog"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn openai_lists_its_fixed_voices_but_cannot_match_a_description() {
        let c = build_catalog("e1", "openai", Some("tts-1".into()), Some("sk")).await;
        assert_eq!(c.status, CatalogStatus::Ok);
        assert!(!c.descriptor_matching);
        assert!(c.voices.iter().any(|v| v.id == "alloy"));
    }

    #[tokio::test]
    async fn a_vendor_without_a_per_key_list_is_unsupported_and_says_so() {
        let c = build_catalog("e1", "aws-polly", None, Some("k")).await;
        assert_eq!(c.status, CatalogStatus::Unsupported);
        assert!(!c.descriptor_matching);
        assert!(
            c.reason
                .as_deref()
                .unwrap_or_default()
                .contains("aws-polly")
        );
    }

    #[tokio::test]
    async fn no_credential_is_unavailable_without_a_vendor_call() {
        let c = build_catalog("e1", "elevenlabs", None, Some("  ")).await;
        assert_eq!(c.status, CatalogStatus::Unavailable);
        assert!(c.voices.is_empty());
    }

    #[test]
    fn the_published_entry_never_carries_the_credential() {
        let c = PublishedCatalog {
            endpoint_id: "e1".into(),
            vendor: "elevenlabs".into(),
            model: None,
            status: CatalogStatus::Ok,
            reason: None,
            descriptor_matching: true,
            voices: vec![],
            fetched_at: 0,
        };
        let json = serde_json::to_string(&c).unwrap();
        assert!(
            !json.contains("credential") && !json.contains("api_key"),
            "{json}"
        );
    }

    #[test]
    fn a_changed_credential_changes_the_fingerprint() {
        assert_ne!(
            fingerprint("elevenlabs", Some("eleven_v3"), Some("k1")),
            fingerprint("elevenlabs", Some("eleven_v3"), Some("k2"))
        );
    }
}
