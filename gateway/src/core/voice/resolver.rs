//! Server-side voice resolution (P4): score the `/voices` catalog for a target
//! provider against a [`VoiceDescriptor`] and return a concrete `voice_id`.
//!
//! The resolver is a PURE function of `(Vec<Voice>, VoiceDescriptor, default_id)`
//! so it is fully unit-testable with a stubbed catalog and no provider key. On no
//! match above the threshold it returns the provider's default voice + a
//! `config_warning` — it is a HARD REQUIREMENT that resolution never fails the
//! request (never a 400).

use super::types::{Age, Gender, VoiceDescriptor};
use crate::handlers::voices::Voice;

/// The outcome of resolving a [`VoiceDescriptor`] over a catalog.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedVoice {
    /// The chosen provider `voice_id`.
    pub voice_id: String,
    /// Whether the id is the provider default fallback (no descriptor match).
    pub used_default: bool,
    /// Advisory warning (surfaced as `config_warning`) — present on a miss or a
    /// weak/partial match.
    pub warning: Option<String>,
}

impl ResolvedVoice {
    #[inline]
    fn matched(voice_id: String) -> Self {
        Self {
            voice_id,
            used_default: false,
            warning: None,
        }
    }

    #[inline]
    fn defaulted(voice_id: String, warning: String) -> Self {
        Self {
            voice_id,
            used_default: true,
            warning: Some(warning),
        }
    }
}

/// Minimum score (out of the weights below) a candidate must reach to be accepted
/// over the provider default. One strong signal (gender OR locale) clears it.
const MATCH_THRESHOLD: i32 = 30;

// Per-criterion weights. Gender + locale are the strong signals; name_hint is the
// single strongest (an explicit name request).
const W_NAME_HINT: i32 = 100;
const W_GENDER: i32 = 40;
const W_LOCALE_EXACT: i32 = 40;
const W_LANG_FAMILY: i32 = 20;
const W_ACCENT_STRING: i32 = 15;
const W_AGE: i32 = 20;
const W_STYLE: i32 = 18;
/// Applied when a locale is explicitly requested but a candidate matches neither its
/// language family nor its region. Large enough to disqualify the candidate outright
/// (a wrong-language voice is worse than the provider default), so no other signal
/// can rescue it.
const W_LOCALE_MISMATCH_PENALTY: i32 = 1000;

/// Resolve `descriptor` over `catalog` (the candidate voices for the TARGET
/// provider), falling back to `default_voice_id` on no adequate match.
///
/// * `catalog` empty → default + warning (caller maps unfetchable → empty).
/// * highest score ≥ [`MATCH_THRESHOLD`] → that voice. Ties broken by lowest `id`
///   (alphabetical) for deterministic, reproducible resolution.
/// * otherwise → `default_voice_id` + a descriptive `config_warning`.
pub fn resolve_voice(
    descriptor: &VoiceDescriptor,
    catalog: &[Voice],
    default_voice_id: &str,
) -> ResolvedVoice {
    if catalog.is_empty() {
        return ResolvedVoice::defaulted(
            default_voice_id.to_string(),
            format!(
                "no voice catalog available to match descriptor {{{}}}; using default '{default_voice_id}'",
                descriptor.describe()
            ),
        );
    }

    // Score every candidate; keep the best (ties → lowest id).
    let mut best: Option<(i32, &Voice)> = None;
    for voice in catalog {
        let score = score_voice(descriptor, voice);
        best = match best {
            None => Some((score, voice)),
            Some((bs, bv)) => {
                if score > bs || (score == bs && voice.id < bv.id) {
                    Some((score, voice))
                } else {
                    Some((bs, bv))
                }
            }
        };
    }

    match best {
        Some((score, voice)) if score >= MATCH_THRESHOLD => {
            ResolvedVoice::matched(voice.id.clone())
        }
        _ => ResolvedVoice::defaulted(
            default_voice_id.to_string(),
            format!(
                "no voice matched descriptor {{{}}}; using default '{default_voice_id}'",
                descriptor.describe()
            ),
        ),
    }
}

/// Score a single catalog [`Voice`] against the descriptor. Higher is better; 0
/// means nothing matched.
fn score_voice(d: &VoiceDescriptor, v: &Voice) -> i32 {
    let mut score = 0;

    // name_hint: substring match against the voice name OR id (strongest).
    if let Some(hint) = d.name_hint.as_ref().filter(|s| !s.trim().is_empty()) {
        let h = hint.trim().to_lowercase();
        if v.name.to_lowercase().contains(&h) || v.id.to_lowercase().contains(&h) {
            score += W_NAME_HINT;
        }
    }

    // gender exact.
    if let Some(g) = d.gender
        && Gender::from_str(&v.gender) == Some(g)
    {
        score += W_GENDER;
    }

    // locale: exact (en-US == en-US) > language-family (any en-*) > accent-string.
    if let Some(locale) = d.locale.as_ref().filter(|s| !s.trim().is_empty()) {
        let want = locale.trim().to_lowercase();
        let want_lang = primary_lang(&want);
        let want_region = region_of(&want);
        // The catalog normalizes language to a NAME ("English") and accent to a
        // region name ("American"); match against both plus the voice id (which
        // frequently embeds the locale, e.g. "en-US-JennyNeural"/"aura-asteria-en").
        let voice_lang_name = v.language.to_lowercase();
        let id_lower = v.id.to_lowercase();

        let region_matches = want_region
            .as_deref()
            .map(|r| {
                accent_matches_region(&v.accent, r)
                    || id_lower.contains(&format!("-{r}"))
                    || id_lower.contains(&format!("_{r}"))
            })
            .unwrap_or(false);
        let lang_matches = lang_name_matches(&voice_lang_name, &want_lang)
            || id_lower.contains(&format!("-{want_lang}"))
            || id_lower.contains(&format!("{want_lang}-"));

        if lang_matches && region_matches {
            score += W_LOCALE_EXACT;
        } else if lang_matches {
            score += W_LANG_FAMILY;
        } else if region_matches {
            score += W_ACCENT_STRING;
        } else {
            // A locale was explicitly requested but the candidate matches NEITHER
            // its language family NOR its region — a hard mismatch. Penalize so a
            // secondary signal (e.g. gender alone) cannot rescue a wrong-language
            // voice: a `ja-JP` request must never resolve to an English voice.
            score -= W_LOCALE_MISMATCH_PENALTY;
        }
    } else if let Some(accent) = d.accent.as_ref().filter(|s| !s.trim().is_empty()) {
        // Free accent string when no locale: match against the catalog accent.
        if v.accent.to_lowercase().contains(&accent.trim().to_lowercase()) {
            score += W_ACCENT_STRING;
        }
    }

    // age: matched against labels embedded in name/description-bearing fields. The
    // unified Voice struct doesn't carry age directly, so we match the age token in
    // the name/id (best-effort; ElevenLabs voices often encode it).
    if let Some(age) = d.age {
        let hay = format!("{} {}", v.name.to_lowercase(), v.id.to_lowercase());
        if age_token_matches(&hay, age) {
            score += W_AGE;
        }
    }

    // style/timbre: substring match against name + id (the catalog's descriptive
    // surface). Deepgram ids/names and ElevenLabs names carry timbre words.
    if let Some(style) = d.style.as_ref().filter(|s| !s.trim().is_empty()) {
        let hay = format!("{} {}", v.name.to_lowercase(), v.id.to_lowercase());
        if hay.contains(&style.trim().to_lowercase()) {
            score += W_STYLE;
        }
    }

    score
}

/// Primary language subtag of a BCP-47 string ("en" of "en-US").
fn primary_lang(locale: &str) -> String {
    locale.split('-').next().unwrap_or(locale).to_string()
}

/// Region subtag of a BCP-47 string ("us" of "en-US"), lowercased.
fn region_of(locale: &str) -> Option<String> {
    locale.split('-').nth(1).map(|r| r.to_lowercase())
}

/// Whether a catalog language NAME ("English") corresponds to a primary subtag
/// ("en"). Uses the same canonical mapping the catalog uses on the way in.
fn lang_name_matches(lang_name: &str, primary_subtag: &str) -> bool {
    // Direct: catalog stored the raw code (e.g. "en") rather than a name.
    if lang_name == primary_subtag {
        return true;
    }
    let expected = match primary_subtag {
        "en" => "english",
        "es" => "spanish",
        "fr" => "french",
        "de" => "german",
        "hi" => "hindi",
        "pt" => "portuguese",
        "it" => "italian",
        "ja" => "japanese",
        "ko" => "korean",
        "zh" | "cmn" | "yue" => "chinese",
        "nl" => "dutch",
        "ru" => "russian",
        "ar" => "arabic",
        "pl" => "polish",
        "tr" => "turkish",
        "sv" => "swedish",
        "ta" => "tamil",
        "te" => "telugu",
        "ml" => "malayalam",
        "mr" => "marathi",
        other => other,
    };
    lang_name.contains(expected)
}

/// Whether a catalog accent string ("American") corresponds to a BCP-47 region
/// subtag ("us"). The inverse of the catalog's `extract_accent_from_code`.
fn accent_matches_region(accent: &str, region: &str) -> bool {
    let a = accent.to_lowercase();
    let expected = match region {
        "us" => "american",
        "gb" | "uk" => "british",
        "au" => "australian",
        "in" => "indian",
        "ca" => "canadian",
        "ie" => "irish",
        "nz" => "new zealand",
        "za" => "south african",
        "mx" => "mexican",
        "br" => "brazilian",
        "fr" => "french",
        "de" => "german",
        "es" => "spain",
        "it" => "italian",
        _ => region,
    };
    a.contains(expected)
}

/// Whether an age band's tokens appear in a (name+id) haystack.
fn age_token_matches(hay: &str, age: Age) -> bool {
    let tokens: &[&str] = match age {
        Age::Young => &["young", "youth", "teen"],
        Age::MiddleAged => &["middle", "adult", "mature"],
        Age::Old => &["old", "older", "senior", "elder"],
    };
    tokens.iter().any(|t| hay.contains(t))
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::voice::types::Gender;

    fn v(id: &str, name: &str, accent: &str, gender: &str, language: &str) -> Voice {
        Voice {
            id: id.to_string(),
            sample: String::new(),
            name: name.to_string(),
            accent: accent.to_string(),
            gender: gender.to_string(),
            language: language.to_string(),
        }
    }

    /// A Deepgram-shaped stub catalog (Aura naming, region accent, gender tag).
    fn deepgram_catalog() -> Vec<Voice> {
        vec![
            v("aura-asteria-en", "Asteria", "American", "Female", "English"),
            v("aura-luna-en", "Luna", "American", "Female", "English"),
            v("aura-orion-en", "Orion", "American", "Male", "English"),
            v("aura-zeus-en", "Zeus", "American", "Male", "English"),
        ]
    }

    /// An ElevenLabs-shaped stub catalog (opaque ids, accent + gender from labels).
    fn elevenlabs_catalog() -> Vec<Voice> {
        vec![
            v(
                "21m00Tcm4TlvDq8ikWAM",
                "Rachel",
                "american",
                "Female",
                "English",
            ),
            v(
                "ErXwobaYiN019PkySvjV",
                "Antoni",
                "american",
                "Male",
                "English",
            ),
            v("AZnzlk1XvdvUeBnXmlld", "Domi", "american", "Female", "English"),
        ]
    }

    /// Azure-shaped stub (short_name ids carrying the locale, locale-derived accent).
    fn azure_catalog() -> Vec<Voice> {
        vec![
            v("en-US-JennyNeural", "Jenny", "American", "Female", "English"),
            v("en-US-GuyNeural", "Guy", "American", "Male", "English"),
            v("en-GB-SoniaNeural", "Sonia", "British", "Female", "English"),
        ]
    }

    #[test]
    fn resolves_female_enus_warm_deepgram_to_asteria() {
        // The plan's litmus test. style "warm" doesn't substring-match any name, but
        // gender+locale alone clear the threshold; tie among females broken by id →
        // "aura-asteria-en" (alphabetically first among asteria/luna).
        let d = VoiceDescriptor {
            gender: Some(Gender::Female),
            locale: Some("en-US".into()),
            style: Some("warm".into()),
            ..Default::default()
        };
        let r = resolve_voice(&d, &deepgram_catalog(), "aura-asteria-en");
        assert_eq!(r.voice_id, "aura-asteria-en");
        assert!(!r.used_default);
        assert!(r.warning.is_none());
    }

    #[test]
    fn resolves_female_enus_elevenlabs_to_rachel() {
        // Females: Rachel + Domi. Tie broken by id → "21m00..." < "AZnzlk..." → Rachel.
        let d = VoiceDescriptor {
            gender: Some(Gender::Female),
            locale: Some("en-US".into()),
            ..Default::default()
        };
        let r = resolve_voice(&d, &elevenlabs_catalog(), "EXAVITQu4vr4xnSDxMaL");
        assert_eq!(r.voice_id, "21m00Tcm4TlvDq8ikWAM");
        assert!(!r.used_default);
    }

    #[test]
    fn resolves_female_enus_azure_to_jenny() {
        // en-US + female → Jenny (Guy is male; Sonia is en-GB).
        let d = VoiceDescriptor {
            gender: Some(Gender::Female),
            locale: Some("en-US".into()),
            ..Default::default()
        };
        let r = resolve_voice(&d, &azure_catalog(), "en-US-AriaNeural");
        assert_eq!(r.voice_id, "en-US-JennyNeural");
    }

    #[test]
    fn name_hint_dominates() {
        // Asking for "zeus" by name beats the gender/locale signals for others.
        let d = VoiceDescriptor {
            gender: Some(Gender::Female), // contradicts Zeus (male) but name_hint wins
            name_hint: Some("zeus".into()),
            ..Default::default()
        };
        let r = resolve_voice(&d, &deepgram_catalog(), "aura-asteria-en");
        assert_eq!(r.voice_id, "aura-zeus-en");
    }

    #[test]
    fn locale_region_distinguishes_gb_from_us() {
        // Female + en-GB → Sonia (exact locale beats en-US females on accent).
        let d = VoiceDescriptor {
            gender: Some(Gender::Female),
            locale: Some("en-GB".into()),
            ..Default::default()
        };
        let r = resolve_voice(&d, &azure_catalog(), "en-US-JennyNeural");
        assert_eq!(r.voice_id, "en-GB-SoniaNeural");
    }

    #[test]
    fn no_match_returns_default_with_warning() {
        // Japanese male against an English-only catalog → default + warning, never panic.
        let d = VoiceDescriptor {
            gender: Some(Gender::Male),
            locale: Some("ja-JP".into()),
            ..Default::default()
        };
        let r = resolve_voice(&d, &deepgram_catalog(), "aura-asteria-en");
        assert!(r.used_default);
        assert_eq!(r.voice_id, "aura-asteria-en");
        let w = r.warning.unwrap();
        assert!(w.contains("no voice matched"));
        assert!(w.contains("ja-JP"));
    }

    #[test]
    fn empty_catalog_returns_default_with_warning() {
        let d = VoiceDescriptor {
            gender: Some(Gender::Female),
            ..Default::default()
        };
        let r = resolve_voice(&d, &[], "aura-asteria-en");
        assert!(r.used_default);
        assert_eq!(r.voice_id, "aura-asteria-en");
        assert!(r.warning.unwrap().contains("no voice catalog"));
    }

    #[test]
    fn style_substring_breaks_gender_tie() {
        // Two female en-US voices; style "luna" pushes Luna above Asteria.
        let d = VoiceDescriptor {
            gender: Some(Gender::Female),
            locale: Some("en-US".into()),
            style: Some("luna".into()),
            ..Default::default()
        };
        let r = resolve_voice(&d, &deepgram_catalog(), "aura-asteria-en");
        assert_eq!(r.voice_id, "aura-luna-en");
    }

    #[test]
    fn accent_string_without_locale_matches() {
        let d = VoiceDescriptor {
            gender: Some(Gender::Male),
            accent: Some("american".into()),
            ..Default::default()
        };
        let r = resolve_voice(&d, &deepgram_catalog(), "aura-asteria-en");
        // Males: Orion + Zeus, tie broken by id → Orion.
        assert_eq!(r.voice_id, "aura-orion-en");
        assert!(!r.used_default);
    }

    #[test]
    fn deterministic_tie_break_is_lowest_id() {
        // Pure gender match, two males; lowest id wins reproducibly.
        let d = VoiceDescriptor {
            gender: Some(Gender::Male),
            ..Default::default()
        };
        let r1 = resolve_voice(&d, &deepgram_catalog(), "x");
        let r2 = resolve_voice(&d, &deepgram_catalog(), "x");
        assert_eq!(r1.voice_id, r2.voice_id);
        assert_eq!(r1.voice_id, "aura-orion-en");
    }
}
