//! Which vendors can translate, and in what sense.
//!
//! `TranslationConfig::warnings_for` has always known the answer — five vendors of thirty-one
//! translate, in three different ways — and it left the process through no surface at all. So the
//! deployment settings form offered "Target languages" to every vendor whose deployment declares
//! an `audio_translation` capability, including Deepgram and ElevenLabs, which cannot translate.
//! Setting it there changed nothing about the request and reported nothing back: the response was
//! byte-identical to leaving it blank.
//!
//! **Derived, not declared.** A vendor's class is worked out by ASKING `warnings_for` what it
//! would say about a representative request, the same technique the delivery-style matrix uses.
//! Add a vendor to that match and it appears here; move one between arms and its row moves with
//! it. A hand-written copy would be a second thing that can be wrong about the same question.

use serde::Serialize;

use super::standard::{SPEECHMATICS_MAX_TRANSLATION_TARGETS, TranslationConfig};
use crate::core::lang::types::CanonicalLanguage;

/// How a vendor translates, if it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TranslationSupport {
    /// Any target language the canonical vocabulary contains.
    Arbitrary,
    /// English only — the vendor exposes a translate-to-English endpoint and nothing else, so a
    /// target list is silently the wrong shape rather than merely unsupported.
    EnglishOnly,
    /// Supported, but only on the prerecorded/batch transport.
    BatchOnly,
    /// Not at all. The form must not offer target languages for these.
    None,
}

impl TranslationSupport {
    /// Whether a deployment on this vendor can translate on the PRERECORDED path, which is the
    /// one a settings screen configures.
    pub fn available_on_upload(&self) -> bool {
        !matches!(self, Self::None)
    }
}

/// One vendor's translation surface.
#[derive(Debug, Clone, Serialize)]
pub struct TranslationSupportRow {
    /// Provider id, matching the STT feature matrix.
    pub provider: &'static str,
    /// What the vendor can do.
    pub support: TranslationSupport,
    /// The cap on target languages, where the vendor has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_targets: Option<usize>,
}

/// Probe one provider by asking `warnings_for` about representative requests.
fn classify(provider: &str) -> TranslationSupport {
    // A non-English target: the question "can you translate to an arbitrary language".
    let arbitrary = TranslationConfig {
        target_languages: vec![CanonicalLanguage::EsEs],
        translate_to_english: None,
        partials: None,
    };
    let batch_warnings = arbitrary.warnings_for(provider, false);
    let streaming_warnings = arbitrary.warnings_for(provider, true);

    let unsupported = |w: &[String]| w.iter().any(|m| m.contains("not supported by"));
    if unsupported(&batch_warnings) {
        return TranslationSupport::None;
    }
    if unsupported(&streaming_warnings) {
        // Refused on the socket, accepted on the upload: AssemblyAI's Speech-Understanding models.
        return TranslationSupport::BatchOnly;
    }
    if batch_warnings
        .iter()
        .any(|m| m.contains("only supports translate-to-English"))
    {
        return TranslationSupport::EnglishOnly;
    }
    TranslationSupport::Arbitrary
}

/// The cap a vendor places on the number of targets, if any.
fn max_targets(provider: &str) -> Option<usize> {
    // Probed the same way: one target over the documented cap must draw a truncation warning.
    let over_cap = TranslationConfig {
        target_languages: vec![CanonicalLanguage::EsEs; SPEECHMATICS_MAX_TRANSLATION_TARGETS + 1],
        translate_to_english: None,
        partials: None,
    };
    over_cap
        .warnings_for(provider, false)
        .iter()
        .any(|m| m.contains("accepts at most"))
        .then_some(SPEECHMATICS_MAX_TRANSLATION_TARGETS)
}

/// The per-vendor translation matrix, computed from `warnings_for`.
pub fn translation_support_matrix() -> Vec<TranslationSupportRow> {
    crate::core::capabilities::STT_FEATURE_SUPPORT
        .iter()
        .map(|row| TranslationSupportRow {
            provider: row.provider,
            support: classify(row.provider),
            max_targets: max_targets(row.provider),
        })
        .collect()
}

/// How one vendor translates, or `None` when WaaV has no opinion about it.
///
/// `None` is a provider outside the STT matrix — "we do not know", which the form reads as "offer
/// it" rather than "hide it", the same unknown-is-not-unsupported rule everything else here
/// follows. A catalogued vendor that cannot translate answers `Some(None_)`, and the form hides
/// the control.
pub fn translation_support_for(provider: &str) -> Option<TranslationSupport> {
    let provider = crate::core::capabilities::normalize_provider(provider);
    crate::core::capabilities::STT_FEATURE_SUPPORT
        .iter()
        .find(|row| crate::core::capabilities::normalize_provider(row.provider) == provider)
        .map(|row| classify(row.provider))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 🔒 The checked-in snapshot still describes `warnings_for`.
    ///
    /// `translation_matrix.json` is what budapp reads to decide whether to offer the Target
    /// languages control at all. Snapshotting rather than re-deriving is deliberate: a second
    /// derivation could be wrong in the same direction as the first.
    #[test]
    fn the_translation_snapshot_still_matches_the_vocabulary() {
        let computed = serde_json::to_value(translation_support_matrix()).expect("serializable");
        let snapshot: serde_json::Value =
            serde_json::from_str(include_str!("translation_matrix.json"))
                .expect("snapshot is json");

        let a = snapshot.as_array().expect("snapshot is an array");
        let b = computed.as_array().expect("computed is an array");
        assert_eq!(
            a.len(),
            b.len(),
            "vendor count changed: {} -> {}",
            a.len(),
            b.len()
        );
        for (was, now) in a.iter().zip(b.iter()) {
            assert_eq!(
                was, now,
                "translation support changed; regenerate translation_matrix.json"
            );
        }
    }

    /// Prints the snapshot. Run with `--nocapture` to regenerate `translation_matrix.json`.
    #[test]
    fn print_translation_snapshot() {
        println!(
            "TSNAP_BEGIN{}TSNAP_END",
            serde_json::to_string_pretty(&translation_support_matrix()).unwrap()
        );
    }

    #[test]
    fn the_vendors_that_cannot_translate_are_the_majority() {
        // The finding: the form offered target languages to all of them.
        let matrix = translation_support_matrix();
        let can: Vec<_> = matrix
            .iter()
            .filter(|r| r.support.available_on_upload())
            .map(|r| r.provider)
            .collect();
        assert_eq!(matrix.len(), 30);
        assert_eq!(
            can,
            vec!["assemblyai", "gladia", "groq", "openai", "speechmatics"],
            "five of thirty; the rest were being offered a control that does nothing"
        );
    }

    #[test]
    fn the_two_deployed_vendors_cannot_translate() {
        // Both of pde-ditto's transcription vendors. Setting target languages on either changed
        // nothing about the request and reported nothing back.
        assert_eq!(
            translation_support_for("deepgram"),
            Some(TranslationSupport::None)
        );
        assert_eq!(
            translation_support_for("elevenlabs"),
            Some(TranslationSupport::None)
        );
    }

    #[test]
    fn the_three_classes_are_distinguished() {
        // They are genuinely different answers, and a boolean would flatten them: English-only
        // means a target list is the wrong SHAPE, not merely unsupported.
        assert_eq!(
            translation_support_for("speechmatics"),
            Some(TranslationSupport::Arbitrary)
        );
        assert_eq!(
            translation_support_for("openai"),
            Some(TranslationSupport::EnglishOnly)
        );
        assert_eq!(
            translation_support_for("assemblyai"),
            Some(TranslationSupport::BatchOnly)
        );
    }

    #[test]
    fn a_cap_is_reported_where_the_vendor_has_one() {
        let speechmatics = translation_support_matrix()
            .into_iter()
            .find(|r| r.provider == "speechmatics")
            .unwrap();
        assert_eq!(
            speechmatics.max_targets,
            Some(SPEECHMATICS_MAX_TRANSLATION_TARGETS)
        );
        let gladia = translation_support_matrix()
            .into_iter()
            .find(|r| r.provider == "gladia")
            .unwrap();
        assert_eq!(gladia.max_targets, None);
    }

    #[test]
    fn an_uncatalogued_vendor_is_unknown_not_unsupported() {
        assert_eq!(translation_support_for("a-vendor-added-last-week"), None);
        assert_eq!(translation_support_for(""), None);
    }

    #[test]
    fn batch_only_still_counts_as_available_on_an_upload() {
        // The settings screen configures the PRERECORDED path, where AssemblyAI does translate.
        assert!(TranslationSupport::BatchOnly.available_on_upload());
        assert!(!TranslationSupport::None.available_on_upload());
    }
}
