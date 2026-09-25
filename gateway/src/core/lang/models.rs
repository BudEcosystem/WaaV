//! Models whose language support is NARROWER than their vendor's.
//!
//! The per-provider gate in [`super::language_support_matrix`] answers "which languages will this
//! VENDOR accept". For most vendors that is the whole story. For Deepgram it is not: language
//! support there is a property of the MODEL, and the specialised models — medical, pharma,
//! phonecall, meeting and the rest — are English-only while `nova-3` itself speaks fifty
//! languages.
//!
//! Without this table the deployment settings form offered all 49 canonical languages for a
//! `nova-3-medical` endpoint, and 44 of them produced
//! `400 Bad Request: No such model/language/tier combination found` at request time — a control
//! that publishes cleanly and can never work, which is the exact defect the feature matrix was
//! built to remove, one level down.
//!
//! # Why this one is DECLARED rather than derived
//!
//! Every other matrix in this crate is computed from the code it describes, so it cannot lie.
//! This one cannot be: it is a fact about a vendor's product, visible only in their docs. That
//! makes it the one table here that can go stale silently, so it is deliberately **narrow**:
//!
//! * Only models a vendor documents as restricted. A model absent from this table is
//!   unrestricted as far as WaaV is concerned, and the vendor remains the authority — which is
//!   the safe direction, because a stale allow-list that REFUSES a working combination is worse
//!   than one that offers a combination the vendor will reject with a clear message.
//! * Only the canonical languages, intersected. `nova-3-medical` accepts `en-CA` and `en-IE`,
//!   which are not in the canonical value space, so they are simply absent rather than added to
//!   it for one model.
//!
//! The backstop is the error path, not this table: a vendor 4xx is now a 400 naming the value
//! (see `handlers::transcribe::classify`), so a combination this table misses still tells the
//! operator exactly what to change.

/// One model's language allow-list.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ModelLanguageRow {
    /// Provider id, matching [`super::LanguageSupportRow::provider`].
    pub provider: &'static str,
    /// The model id as it reaches the vendor — what a `voice_table` entry carries in `model`.
    pub model: &'static str,
    /// The canonical languages this model accepts, as BCP-47 tokens.
    ///
    /// Never empty: an empty list here would mean "supports nothing", and the way to say
    /// "unrestricted" is to leave the model out of the table entirely.
    pub supported: &'static [&'static str],
}

/// The canonical English tags Deepgram's `en`/`en-US`-only models accept.
///
/// Deepgram documents these as "English: `en`, `en-US`". The bare `en` is not a canonical token
/// here, so the intersection is a single entry.
const DEEPGRAM_EN_US_ONLY: &[&str] = &["en-US"];

/// The canonical English tags Deepgram's nova-3 specialised models accept.
///
/// Documented as "English: `en`, `en-US`, `en-AU`, `en-CA`, `en-GB`, `en-IE`, `en-IN`, `en-NZ`".
/// `en-CA` and `en-IE` are outside the canonical value space and `en-ZA` is outside Deepgram's,
/// so the intersection is these five.
const DEEPGRAM_EN_WIDE: &[&str] = &["en-US", "en-GB", "en-IN", "en-AU", "en-NZ"];

/// Every model WaaV knows to be language-restricted.
///
/// Sourced from Deepgram's own models-and-languages page. Kept in the order that page lists them
/// (nova-3, nova-2, base, enhanced, legacy) so the two can be diffed by eye.
pub const MODEL_LANGUAGE_SUPPORT: &[ModelLanguageRow] = &[
    // --- nova-3 specialised -----------------------------------------------------------------
    ModelLanguageRow {
        provider: "deepgram",
        model: "nova-3-medical",
        supported: DEEPGRAM_EN_WIDE,
    },
    ModelLanguageRow {
        provider: "deepgram",
        model: "nova-3-pharma",
        supported: DEEPGRAM_EN_WIDE,
    },
    // --- nova-2 specialised -----------------------------------------------------------------
    ModelLanguageRow {
        provider: "deepgram",
        model: "nova-2-medical",
        supported: DEEPGRAM_EN_US_ONLY,
    },
    ModelLanguageRow {
        provider: "deepgram",
        model: "nova-2-meeting",
        supported: DEEPGRAM_EN_US_ONLY,
    },
    ModelLanguageRow {
        provider: "deepgram",
        model: "nova-2-phonecall",
        supported: DEEPGRAM_EN_US_ONLY,
    },
    ModelLanguageRow {
        provider: "deepgram",
        model: "nova-2-finance",
        supported: DEEPGRAM_EN_US_ONLY,
    },
    ModelLanguageRow {
        provider: "deepgram",
        model: "nova-2-conversationalai",
        supported: DEEPGRAM_EN_US_ONLY,
    },
    ModelLanguageRow {
        provider: "deepgram",
        model: "nova-2-voicemail",
        supported: DEEPGRAM_EN_US_ONLY,
    },
    ModelLanguageRow {
        provider: "deepgram",
        model: "nova-2-video",
        supported: DEEPGRAM_EN_US_ONLY,
    },
    ModelLanguageRow {
        provider: "deepgram",
        model: "nova-2-drivethru",
        supported: DEEPGRAM_EN_US_ONLY,
    },
    ModelLanguageRow {
        provider: "deepgram",
        model: "nova-2-automotive",
        supported: DEEPGRAM_EN_US_ONLY,
    },
    ModelLanguageRow {
        provider: "deepgram",
        model: "nova-2-atc",
        supported: DEEPGRAM_EN_US_ONLY,
    },
    // --- base tier --------------------------------------------------------------------------
    ModelLanguageRow {
        provider: "deepgram",
        model: "base-meeting",
        supported: DEEPGRAM_EN_US_ONLY,
    },
    ModelLanguageRow {
        provider: "deepgram",
        model: "base-phonecall",
        supported: DEEPGRAM_EN_US_ONLY,
    },
    ModelLanguageRow {
        provider: "deepgram",
        model: "base-finance",
        supported: DEEPGRAM_EN_US_ONLY,
    },
    ModelLanguageRow {
        provider: "deepgram",
        model: "base-conversationalai",
        supported: DEEPGRAM_EN_US_ONLY,
    },
    ModelLanguageRow {
        provider: "deepgram",
        model: "base-voicemail",
        supported: DEEPGRAM_EN_US_ONLY,
    },
    ModelLanguageRow {
        provider: "deepgram",
        model: "base-video",
        supported: DEEPGRAM_EN_US_ONLY,
    },
    // --- enhanced tier ----------------------------------------------------------------------
    ModelLanguageRow {
        provider: "deepgram",
        model: "enhanced-meeting",
        supported: DEEPGRAM_EN_US_ONLY,
    },
    ModelLanguageRow {
        provider: "deepgram",
        model: "enhanced-phonecall",
        supported: DEEPGRAM_EN_US_ONLY,
    },
    ModelLanguageRow {
        provider: "deepgram",
        model: "enhanced-finance",
        supported: DEEPGRAM_EN_US_ONLY,
    },
    // --- legacy -----------------------------------------------------------------------------
    ModelLanguageRow {
        provider: "deepgram",
        model: "nova-medical",
        supported: DEEPGRAM_EN_US_ONLY,
    },
    ModelLanguageRow {
        provider: "deepgram",
        model: "nova-phonecall",
        supported: DEEPGRAM_EN_US_ONLY,
    },
];

/// The canonical languages a specific model accepts, or `None` for "no model-level restriction".
///
/// `None` is the answer for every model not in the table, and it means the VENDOR's gate applies —
/// which for most vendors is itself unrestricted. Returning the full canonical list instead would
/// make "we have no information" indistinguishable from "we checked, and it is all 49".
pub fn model_language_support(provider: &str, model: &str) -> Option<&'static [&'static str]> {
    let provider = crate::core::capabilities::normalize_provider(provider);
    let model = model.trim().to_lowercase();
    if provider.is_empty() || model.is_empty() {
        return None;
    }
    MODEL_LANGUAGE_SUPPORT
        .iter()
        .find(|row| {
            crate::core::capabilities::normalize_provider(row.provider) == provider
                && row.model == model
        })
        .map(|row| row.supported)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::lang::types::CanonicalLanguage;

    #[test]
    fn the_deployment_that_started_this_is_covered() {
        // `nova-3-medical` offered 49 languages in the settings form and answered
        // `400 No such model/language/tier combination found` for 44 of them.
        let langs = model_language_support("deepgram", "nova-3-medical").unwrap();
        assert!(langs.contains(&"en-US"));
        assert!(
            !langs.contains(&"de-DE"),
            "the medical model is English-only"
        );
        assert!(!langs.contains(&"ja-JP"));
    }

    #[test]
    fn an_unrestricted_model_answers_none_not_an_empty_list() {
        // The distinction that decides what the form shows. An empty list would mean "supports
        // nothing" and the picker would offer zero languages for every ordinary model.
        assert_eq!(model_language_support("deepgram", "nova-3"), None);
        assert_eq!(model_language_support("elevenlabs", "scribe_v2"), None);
        assert_eq!(model_language_support("deepgram", ""), None);
        assert_eq!(model_language_support("", "nova-3-medical"), None);
    }

    #[test]
    fn the_lookup_trims_and_ignores_case() {
        assert_eq!(
            model_language_support("  DeepGram ", " Nova-3-Medical "),
            model_language_support("deepgram", "nova-3-medical")
        );
    }

    #[test]
    fn every_listed_language_is_canonical() {
        // A row naming a language the value space does not contain would render an option the
        // form could offer and the validator would then refuse.
        let canonical: Vec<&str> = CanonicalLanguage::all()
            .iter()
            .map(|c| c.as_bcp47())
            .collect();
        for row in MODEL_LANGUAGE_SUPPORT {
            for lang in row.supported {
                assert!(
                    canonical.contains(lang),
                    "{}/{} names '{lang}', which is not a canonical language",
                    row.provider,
                    row.model
                );
            }
        }
    }

    #[test]
    fn no_row_is_empty_and_no_model_is_listed_twice() {
        // Empty would mean "supports nothing"; the way to say unrestricted is to be absent.
        // A duplicate would make the lookup depend on declaration order.
        let mut seen = std::collections::BTreeSet::new();
        for row in MODEL_LANGUAGE_SUPPORT {
            assert!(
                !row.supported.is_empty(),
                "{}/{} is empty; omit the row instead",
                row.provider,
                row.model
            );
            assert!(
                seen.insert((row.provider, row.model)),
                "{}/{} is listed twice",
                row.provider,
                row.model
            );
        }
    }

    #[test]
    fn every_restricted_model_belongs_to_a_provider_the_gateway_knows() {
        // A row for a vendor with no language mapper would never be consulted.
        let known: Vec<&str> = crate::core::lang::language_support_matrix()
            .into_iter()
            .map(|r| r.provider)
            .collect();
        for row in MODEL_LANGUAGE_SUPPORT {
            assert!(
                known.contains(&row.provider),
                "{} is not in the language support matrix",
                row.provider
            );
        }
    }

    #[test]
    fn a_restricted_model_is_narrower_than_its_vendor() {
        // The table's whole reason to exist. A row that matched its vendor's own gate would be
        // noise, and one that WIDENED it would be a promise the vendor never made.
        for row in MODEL_LANGUAGE_SUPPORT {
            let vendor = crate::core::lang::language_support_matrix()
                .into_iter()
                .find(|r| r.provider == row.provider)
                .expect("checked by the test above");
            if vendor.supported.is_empty() {
                // An ungated vendor: any non-empty subset of the canonical space is narrower.
                assert!(row.supported.len() < CanonicalLanguage::all().len());
            } else {
                for lang in row.supported {
                    assert!(
                        vendor.supported.contains(lang),
                        "{}/{} allows '{lang}', which its vendor's gate does not",
                        row.provider,
                        row.model
                    );
                }
            }
        }
    }
}
