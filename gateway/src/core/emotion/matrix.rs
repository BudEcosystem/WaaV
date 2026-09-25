//! Which emotions each TTS vendor actually honours.
//!
//! `ProviderEmotionSupport` has carried an exact per-vendor emotion list since the emotion system
//! was written, and until now it left the process through no surface at all — not
//! `/capabilities`, not the snapshot budapp reads. So the deployment settings form offered all 44
//! canonical emotions to every vendor, including the ones that honour eight.
//!
//! That is the same defect the feature matrix exists to remove, one field over, and a worse
//! instance of it: the per-vendor answer was not merely discoverable from a vendor's docs, it was
//! already computed in this crate.
//!
//! **Derived, not declared.** A vendor's row is whatever
//! [`get_mapper_for_provider`](crate::core::emotion::mappers::get_mapper_for_provider) returns for
//! it, so this cannot drift from the mappers it describes — change a mapper's `supported_emotions`
//! and the row changes with it. The universe of providers is
//! [`crate::core::capabilities::TTS_FEATURE_SUPPORT`], which is every vendor a TTS deployment can
//! resolve to; a vendor there with no emotion mapper gets the fallback, whose support is honestly
//! empty.

use serde::Serialize;

use super::mapper::EmotionMethod;
use super::types::Emotion;

/// One vendor's emotion surface.
#[derive(Debug, Clone, Serialize)]
pub struct EmotionSupportRow {
    /// Provider id, matching the TTS feature matrix.
    pub provider: &'static str,
    /// Whether the vendor honours any emotion at all. `false` means the settings form should not
    /// offer the control, not that it should offer it empty.
    pub supports_emotions: bool,
    /// The canonical emotions this vendor honours, serialized as the snake_case tokens the API
    /// takes. Empty when `supports_emotions` is false.
    pub emotions: &'static [Emotion],
    /// Whether an intensity value reaches the vendor. A vendor that ignores it should not be
    /// offered the control: the operator sets a number and nothing anywhere changes.
    pub supports_intensity: bool,
    /// Whether a delivery style reaches the vendor at all.
    ///
    /// Still carried alongside [`styles`](Self::styles) because it is the mapper's own
    /// declaration, and a vendor that declares no style support gets the control hidden outright
    /// rather than filtered down to one entry.
    pub supports_style: bool,
    /// The delivery styles that actually change what this vendor is sent.
    ///
    /// **Derived behaviourally**, not declared: no mapper overrides `supports_style(&style)`, so
    /// the only honest source is the mapping itself. Each style is mapped and compared against
    /// the no-style baseline on the fields that reach the wire. A style whose mapping is
    /// identical to sending no style is inert — the operator picks it, the request does not
    /// change, and nothing anywhere says so. Cartesia has ten of those and OpenAI three.
    ///
    /// `Normal` is always kept: it IS the baseline, so it compares equal by construction, and
    /// dropping it would leave no way to say "no particular style".
    ///
    /// Styles that map to the SAME output as each other are all kept. They are coarse, not
    /// inert — ElevenLabs funnels ten of them into one professional register — and hiding a
    /// control that does change the request would be the opposite mistake.
    pub styles: Vec<super::types::DeliveryStyle>,
    /// Whether the vendor accepts a free-form description (Hume's natural-language surface).
    pub supports_free_description: bool,
    /// How the vendor expresses emotion, for display.
    pub method: EmotionMethod,
}

/// The per-vendor emotion matrix, computed from the mappers.
pub fn emotion_support_matrix() -> Vec<EmotionSupportRow> {
    crate::core::capabilities::TTS_FEATURE_SUPPORT
        .iter()
        .map(|row| {
            let support = super::mappers::get_mapper_for_provider(row.provider).get_support();
            EmotionSupportRow {
                provider: row.provider,
                supports_emotions: support.supports_emotions,
                emotions: support.supported_emotions,
                supports_intensity: support.supports_intensity,
                supports_style: support.supports_style,
                styles: if support.supports_style {
                    effective_styles(row.provider)
                } else {
                    Vec::new()
                },
                supports_free_description: support.supports_free_description,
                method: support.method,
            }
        })
        .collect()
}

/// The parts of a mapping that reach the vendor.
///
/// Warnings are excluded deliberately: a warning is the mapper SAYING it could not do anything,
/// so counting it as a difference would mark every unsupported style as supported — the exact
/// inversion this derivation exists to avoid.
fn wire_fingerprint(m: &super::mapper::MappedEmotion) -> String {
    format!(
        "{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}",
        m.description,
        m.ssml_style,
        m.ssml_style_degree,
        m.stability,
        m.similarity_boost,
        m.style,
        m.inline_tags,
        m.instruction_text,
        m.native_emotion,
        m.emotion_array,
        m.speed,
    )
}

/// The delivery styles that change what a provider is sent.
///
/// Probed rather than declared — see [`EmotionSupportRow::styles`].
fn effective_styles(provider: &str) -> Vec<super::types::DeliveryStyle> {
    use super::types::{DeliveryStyle, EmotionConfig};
    let mapper = super::mappers::get_mapper_for_provider(provider);
    let baseline = wire_fingerprint(&mapper.map_emotion(&EmotionConfig::default()));
    DeliveryStyle::all()
        .iter()
        .copied()
        .filter(|style| {
            // `Normal` is the baseline itself; it compares equal by construction and is the way
            // to say "no particular style", so it is never filtered out.
            if *style == DeliveryStyle::Normal {
                return true;
            }
            let config = EmotionConfig {
                style: Some(*style),
                ..Default::default()
            };
            wire_fingerprint(&mapper.map_emotion(&config)) != baseline
        })
        .collect()
}

/// The delivery styles one vendor honours, or `None` when WaaV has no opinion.
///
/// `None` for a provider outside the matrix. `Some(&[])` for a catalogued vendor with no style
/// support at all, which hides the control — a different answer, and the distinction is the same
/// one [`emotions_for_provider`] draws.
pub fn styles_for_provider(provider: &str) -> Option<Vec<super::types::DeliveryStyle>> {
    let provider = crate::core::capabilities::normalize_provider(provider);
    crate::core::capabilities::TTS_FEATURE_SUPPORT
        .iter()
        .find(|row| crate::core::capabilities::normalize_provider(row.provider) == provider)
        .map(|row| {
            let support = super::mappers::get_mapper_for_provider(row.provider).get_support();
            if support.supports_style {
                effective_styles(row.provider)
            } else {
                Vec::new()
            }
        })
}

/// The emotions one vendor honours, or `None` when WaaV has no opinion.
///
/// `None` for a provider outside the matrix — "we do not know", which the form must read as
/// "offer everything" rather than "offer nothing". A vendor that is in the matrix and supports no
/// emotions answers `Some(&[])`, and the form hides the control entirely. The two are different
/// answers and the distinction is the whole reason both exist.
pub fn emotions_for_provider(provider: &str) -> Option<&'static [Emotion]> {
    // Separator-insensitive, for the reason spelled out on `capabilities::find`: budapp says
    // `aws-polly` and this table says `aws_polly`, and an exact match answers "unknown" — which
    // the form reads as "offer all 44 emotions" for a vendor known to honour none.
    let provider = crate::core::capabilities::normalize_provider(provider);
    crate::core::capabilities::TTS_FEATURE_SUPPORT
        .iter()
        .find(|row| crate::core::capabilities::normalize_provider(row.provider) == provider)
        .map(|row| {
            super::mappers::get_mapper_for_provider(row.provider)
                .get_support()
                .supported_emotions
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_matrix_covers_every_tts_vendor() {
        let matrix = emotion_support_matrix();
        assert_eq!(
            matrix.len(),
            crate::core::capabilities::TTS_FEATURE_SUPPORT.len()
        );
        assert_eq!(matrix.len(), 34);
    }

    #[test]
    fn the_vendors_that_honour_emotions_honour_fewer_than_all_of_them() {
        // The finding this module exists for. If every emotion-capable vendor took all 44, the
        // form would be right to offer all 44 and this whole surface would be noise.
        let narrower: Vec<_> = emotion_support_matrix()
            .into_iter()
            .filter(|r| r.supports_emotions && r.emotions.len() < Emotion::all().len())
            .map(|r| (r.provider, r.emotions.len()))
            .collect();
        assert!(
            !narrower.is_empty(),
            "no vendor is narrower than the canonical list; the filter would be pointless"
        );
    }

    #[test]
    fn a_vendor_with_no_emotion_support_is_empty_not_absent() {
        // Deepgram has a real fallback mapper, so it is IN the matrix and honestly empty — which
        // tells the form to hide the control. Absent would mean "unknown", which tells it to
        // offer everything, and those must not be the same answer.
        let deepgram = emotions_for_provider("deepgram").expect("deepgram is a TTS vendor");
        assert!(deepgram.is_empty());
        let row = emotion_support_matrix()
            .into_iter()
            .find(|r| r.provider == "deepgram")
            .unwrap();
        assert!(!row.supports_emotions);
    }

    #[test]
    fn an_uncatalogued_vendor_is_unknown_not_unsupported() {
        assert_eq!(emotions_for_provider("a-vendor-added-last-week"), None);
        assert_eq!(emotions_for_provider(""), None);
    }

    #[test]
    fn the_lookup_trims_and_ignores_case() {
        assert_eq!(
            emotions_for_provider("  ElevenLabs ").map(<[Emotion]>::len),
            emotions_for_provider("elevenlabs").map(<[Emotion]>::len)
        );
    }

    #[test]
    fn every_listed_emotion_is_canonical() {
        // A vendor naming an emotion outside the canonical vocabulary would render an option the
        // form could offer and the validator would then refuse.
        for row in emotion_support_matrix() {
            for e in row.emotions {
                assert!(
                    Emotion::all().contains(e),
                    "{} honours {e:?}, which is not in Emotion::all()",
                    row.provider
                );
            }
        }
    }

    #[test]
    fn support_flags_are_consistent_with_the_list() {
        // `supports_emotions: false` with a non-empty list would make `supports_emotion()` answer
        // false for every entry in its own list.
        for row in emotion_support_matrix() {
            if !row.supports_emotions {
                assert!(
                    row.emotions.is_empty(),
                    "{} claims no emotion support but lists {} of them",
                    row.provider,
                    row.emotions.len()
                );
            }
        }
    }

    #[test]
    fn the_inert_styles_are_filtered_out_and_the_coarse_ones_are_not() {
        use crate::core::emotion::types::DeliveryStyle;
        let by = |p: &str| styles_for_provider(p).unwrap();

        // Cartesia is the case that justifies this: ten styles map to exactly what sending no
        // style maps to, so choosing "whispered" on a Cartesia deployment changes nothing about
        // the request and nothing anywhere says so.
        let cartesia = by("cartesia");
        assert!(!cartesia.contains(&DeliveryStyle::Whispered));
        assert!(!cartesia.contains(&DeliveryStyle::Shouted));
        assert!(cartesia.len() < DeliveryStyle::all().len());

        // ElevenLabs funnels ten styles into one professional register. They are COARSE, not
        // inert — each still changes the request relative to no style — so all of them stay.
        // Filtering on indistinguishability would remove controls that work.
        let elevenlabs = by("elevenlabs");
        assert_eq!(
            elevenlabs.len(),
            DeliveryStyle::all().len(),
            "every ElevenLabs style changes the request, even where several agree"
        );
    }

    #[test]
    fn normal_survives_everywhere_it_is_offered() {
        // `Normal` IS the baseline, so it compares equal by construction. Filtering it out would
        // leave a picker with no way to say "no particular style".
        use crate::core::emotion::types::DeliveryStyle;
        for row in emotion_support_matrix() {
            if row.supports_style {
                assert!(
                    row.styles.contains(&DeliveryStyle::Normal),
                    "{} lost Normal",
                    row.provider
                );
            }
        }
    }

    #[test]
    fn a_vendor_with_no_style_support_gets_an_empty_list_not_all_of_them() {
        // Empty hides the control; `None` (absent) would mean "unknown, offer everything".
        assert_eq!(styles_for_provider("deepgram"), Some(Vec::new()));
        assert_eq!(styles_for_provider("a-vendor-added-last-week"), None);
    }

    #[test]
    fn a_warning_is_not_evidence_that_a_style_landed() {
        // The inversion this derivation had to avoid: a mapper that warns "cannot do that style"
        // would differ from the baseline in `warnings` alone, and a naive comparison would read
        // that as support. `wire_fingerprint` excludes warnings for exactly this reason.
        use crate::core::emotion::mapper::MappedEmotion;
        let quiet = MappedEmotion::default();
        let noisy = MappedEmotion {
            warnings: vec!["style not supported".into()],
            ..Default::default()
        };
        assert_eq!(wire_fingerprint(&quiet), wire_fingerprint(&noisy));
    }

    /// 🔒 The checked-in snapshot still describes the mappers.
    ///
    /// `emotion_matrix.json` is what budapp reads to build its own copy — the settings form
    /// filters the emotion picker with it. Snapshotting rather than re-parsing is deliberate, for
    /// the same reason as the language matrix: a parser would be a second implementation of the
    /// thing it describes, and could be wrong in the same direction as the code it checks.
    ///
    /// Regenerate deliberately, by running `print_snapshot` and reading the output — never by
    /// editing the JSON until the test goes quiet.
    #[test]
    fn the_emotion_snapshot_still_matches_the_mappers() {
        let computed = serde_json::to_value(emotion_support_matrix()).expect("serializable");
        let snapshot: serde_json::Value =
            serde_json::from_str(include_str!("emotion_matrix.json")).expect("snapshot is json");

        let a = snapshot.as_array().expect("snapshot is an array");
        let b = computed.as_array().expect("computed is an array");
        assert_eq!(
            a.len(),
            b.len(),
            "vendor count changed: {} -> {}",
            a.len(),
            b.len()
        );
        // Compared per vendor so a failure names the one that moved rather than printing two
        // 37-element arrays and leaving the reader to diff them.
        for (was, now) in a.iter().zip(b.iter()) {
            assert_eq!(
                was, now,
                "emotion support changed; regenerate emotion_matrix.json"
            );
        }
    }

    /// Prints the snapshot. Run with `--nocapture` to regenerate `emotion_matrix.json`.
    #[test]
    fn print_snapshot() {
        println!(
            "SNAPSHOT_BEGIN{}SNAPSHOT_END",
            serde_json::to_string_pretty(&emotion_support_matrix()).unwrap()
        );
    }

    /// Not an assertion — a printout, so the numbers quoted in the spec come from the code rather
    /// than from a grep over it. Run with `--nocapture`.
    #[test]
    fn report_the_per_vendor_counts() {
        let total = Emotion::all().len();
        for row in emotion_support_matrix() {
            if row.supports_emotions || row.supports_style || row.supports_free_description {
                println!(
                    "{:<14} emotions={:<3} of {total}  intensity={:<5} style={:<5} free_desc={}",
                    row.provider,
                    row.emotions.len(),
                    row.supports_intensity,
                    row.supports_style,
                    row.supports_free_description
                );
            }
        }
    }
}
