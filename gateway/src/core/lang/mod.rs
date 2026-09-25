//! Unified Language System for STT/TTS/realtime providers (P2 of the SDK standardization plan).
//!
//! A developer passes ONE canonical, provider-agnostic language token — a region-qualified BCP-47
//! code such as `en-US`, `es-ES`, `hi-IN`, `cmn-CN` (Mandarin), or simply the bare-ISO shorthand
//! `en` / `hi` — and the gateway maps it to EACH provider's native notation internally. This is the
//! core of the "switch models without client edits" promise: the same `language` string works on
//! Deepgram (`en-US`), ElevenLabs (`en`), Speechmatics (`en`), iFlytek (`en_us`), Baidu (`1737`), …
//!
//! # Architecture (mirrors [`crate::core::emotion`])
//!
//! ```text
//!   client "us-en" ──resolve──▶ CanonicalLanguage::EnUs ──to_provider_language(.., provider, model)──▶
//!       ├─ deepgram      → "en-US"      (identity BCP-47)
//!       ├─ elevenlabs    → "en"         (ISO-639-1 downgrade)
//!       ├─ iflytek       → "en_us"      (underscore + accent companion for zh)
//!       ├─ baidu (STT)   → "1737"       (numeric dev_pid model id)
//!       ├─ tencent       → "en"         (stem; builder prepends "16k_")
//!       ├─ google-stt    → "cmn-Hans-CN" / google-tts → "cmn-CN"  (MODEL-AWARE)
//!       └─ sarvam        → "od-IN" for Odia (nonstandard vendor quirk on the way OUT)
//! ```
//!
//! The chassis is ported from the infer engine's `standardize` module: [`notation::resolve_alias`]
//! + [`notation::NotationMap`], with the KEY DIFFERENCE that the canonical is a region-qualified
//! [`CanonicalLanguage`] enum (region matters for STT/TTS locale), not a language-only string.
//!
//! # Call site
//!
//! [`to_provider_language`] (or the string-input convenience [`map_language`]) is called at the
//! config→provider boundary (`to_standard_stt` / `to_standard_tts` in `handlers::ws::config`), so no
//! provider code ever sees a raw client string. On an unsupported language→provider it emits a
//! `config_warning` advisory (via [`MappedLanguage::warnings`]) and falls back to the provider
//! default — NEVER a hard 400. Already-native values resolve as identity.

pub mod mapper;
pub mod mappers;
pub mod models;
pub mod notation;
pub mod types;

// ---- Public re-exports (the surface the rest of the gateway uses) ----------

pub use mapper::{
    LanguageMapper, MappedLanguage, NotationKind, ProviderLanguageSupport, to_provider_language,
};
pub use mappers::get_language_mapper;
pub use models::{MODEL_LANGUAGE_SUPPORT, ModelLanguageRow, model_language_support};
pub use notation::{NotationMap, resolve, resolve_alias};
pub use types::{CanonicalLanguage, LANG_ALIASES};

// =============================================================================
// String-input convenience
// =============================================================================

/// Resolve a raw client language string AND map it to `provider`'s native notation in one call —
/// the convenience the config→provider boundary uses.
///
/// * Recognized token (canonical, alias, vendor quirk, name, `auto`) → resolved to
///   [`CanonicalLanguage`] then mapped.
/// * Unrecognized token → passed through to the provider VERBATIM (additive/safe) with a
///   `config_warning` advisory; never a hard failure (the infer "pass-through + warn" rule).
///
/// `model` disambiguates the model-aware providers (Google STT vs TTS, ElevenLabs v2.5).
pub fn map_language(raw: &str, provider: &str, model: &str) -> MappedLanguage {
    match resolve(raw) {
        Some(canonical) => to_provider_language(canonical, provider, model),
        None => MappedLanguage::native(raw.to_string()).warn(format!(
            "unrecognized language token '{raw}'; passed to {provider} as-is"
        )),
    }
}

// =============================================================================
// Capability matrix (for the /capabilities route + SDK mirror)
// =============================================================================

/// One row of the language-support matrix: a provider + how it handles language. Serializable for
/// the `/capabilities` REST surface (the `notation` kind is a stable lowercase string token so this
/// row stays decoupled from the OpenAPI schema derive).
#[derive(Debug, Clone, serde::Serialize)]
pub struct LanguageSupportRow {
    /// Provider id.
    pub provider: &'static str,
    /// Default notation kind (`bcp47` / `iso6391` / `underscore` / `none`).
    pub notation: &'static str,
    /// Whether the provider auto-detects (the canonical `auto` is honored natively).
    pub supports_auto: bool,
    /// Example: canonical `cmn-CN` rendered to this provider's native string (shows the Chinese
    /// fork at a glance). `None` if the provider takes no language param.
    pub example_cmn_cn: Option<String>,
    /// Example: canonical `en-US` rendered natively.
    pub example_en_us: Option<String>,
    /// The canonical languages this provider accepts, as BCP-47 tokens.
    ///
    /// **Empty means "no gating"** — the provider takes any canonical language at its default
    /// notation — NOT "supports nothing". The distinction matters to every consumer: a UI that
    /// read empty as unsupported would offer no languages at all for the majority of providers,
    /// which are exactly the broad BCP-47 ones.
    ///
    /// Added for FRD-018 Part III: the gate existed on `ProviderLanguageSupport` and was not on
    /// the HTTP surface, so nothing outside this crate could answer "which languages does this
    /// vendor take" — and budadmin rendered a free-text box.
    pub supported: Vec<&'static str>,
}

/// The providers whose language handling the matrix enumerates (the ones with a registered mapper;
/// everything else uses the safe BCP-47 generic default).
const MATRIX_PROVIDERS: &[&str] = &[
    "deepgram",
    "azure",
    "aws-transcribe",
    "aws-polly",
    "nova-sonic",
    "gemini",
    "yandex",
    "sarvam",
    "reverie",
    "reverie-tts",
    "elevenlabs",
    "openai",
    "openai-tts",
    "cartesia",
    "assemblyai",
    "speechmatics",
    "google-stt",
    "google-tts",
    "iflytek",
    "baidu",
    "baidu-tts",
    "tencent",
    "hume",
];

/// Build the full language-support matrix — the canonical→native rendering for every enumerated
/// provider. Unit-tested; exposed by the `/capabilities` route.
pub fn language_support_matrix() -> Vec<LanguageSupportRow> {
    MATRIX_PROVIDERS
        .iter()
        .map(|&provider| {
            let mapper = get_language_mapper(provider);
            let support = mapper.support();
            let render = |lang: CanonicalLanguage| -> Option<String> {
                let m = mapper.map(lang, "");
                if m.omit && m.native.is_empty() {
                    None
                } else {
                    Some(m.native)
                }
            };
            LanguageSupportRow {
                provider,
                notation: support.kind.as_str(),
                supports_auto: support.supports_auto,
                example_cmn_cn: render(CanonicalLanguage::CmnCn),
                example_en_us: render(CanonicalLanguage::EnUs),
                supported: support.supported.iter().map(|l| l.as_bcp47()).collect(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 🔒 The checked-in snapshot still describes the mappers.
    ///
    /// `language_matrix.json` is what budapp reads to build its own copy of this data — the
    /// settings form offers a language list instead of a free-text box, and needs to know which
    /// vendors gate. Snapshotting rather than re-parsing is deliberate: a parser would be a
    /// second implementation of the thing it describes, and could be wrong in the same direction
    /// as the code it checks.
    ///
    /// Regenerate deliberately, by running this test with the file deleted and reading the
    /// failure — never by editing the JSON to match.
    #[test]
    fn the_language_snapshot_still_matches_the_mappers() {
        let canonical: Vec<&str> = CanonicalLanguage::all()
            .iter()
            .map(|c| c.as_bcp47())
            .collect();
        let computed = serde_json::json!({
            "canonical": canonical,
            "providers": language_support_matrix(),
            "model_restrictions": models::MODEL_LANGUAGE_SUPPORT,
        });

        let snapshot: serde_json::Value =
            serde_json::from_str(include_str!("language_matrix.json")).expect("snapshot is json");

        assert_eq!(
            snapshot["canonical"], computed["canonical"],
            "the canonical language value space changed; regenerate language_matrix.json"
        );
        // The per-MODEL gate. budapp mirrors this so the settings form can stop offering 44
        // languages for an English-only model; a drift here is a picker that lies.
        assert_eq!(
            snapshot["model_restrictions"], computed["model_restrictions"],
            "the per-model language restrictions changed; regenerate language_matrix.json"
        );

        // Compared per provider so a failure names the one that moved rather than printing two
        // 23-element arrays and leaving the reader to diff them.
        let a = snapshot["providers"]
            .as_array()
            .expect("snapshot providers");
        let b = computed["providers"]
            .as_array()
            .expect("computed providers");
        assert_eq!(
            a.len(),
            b.len(),
            "provider count changed: {} -> {}",
            a.len(),
            b.len()
        );
        for (want, got) in a.iter().zip(b.iter()) {
            assert_eq!(
                want, got,
                "language support drifted for {}",
                got["provider"]
            );
        }
    }

    #[test]
    fn an_ungated_provider_means_unrestricted_not_unsupported() {
        // The distinction every consumer has to get right: `supported: []` is "takes any
        // canonical language", and 15 of 23 providers are in that state. Reading it as
        // "supports nothing" would offer no languages at all for most vendors.
        let matrix = language_support_matrix();
        let ungated = matrix.iter().filter(|r| r.supported.is_empty()).count();
        assert!(
            ungated > 0,
            "if every provider gates, the empty-means-any rule is dead code"
        );

        let deepgram = matrix.iter().find(|r| r.provider == "deepgram").unwrap();
        assert!(deepgram.supported.is_empty());
        assert!(
            get_language_mapper("deepgram")
                .support()
                .supports(CanonicalLanguage::DeDe),
            "an ungated provider must answer `supports` affirmatively for any language"
        );
    }

    #[test]
    fn map_language_resolves_then_maps() {
        // us-en -> EnUs -> deepgram "en-US".
        let m = map_language("us-en", "deepgram", "");
        assert_eq!(m.native, "en-US");
        assert!(!m.has_warnings());

        // en-US -> elevenlabs "en" (downgrade).
        assert_eq!(
            map_language("en-US", "elevenlabs", "eleven_turbo_v2_5").native,
            "en"
        );
    }

    #[test]
    fn map_language_unknown_passes_through_with_warning() {
        let m = map_language("klingon", "deepgram", "");
        assert_eq!(m.native, "klingon"); // verbatim pass-through
        assert!(m.has_warnings());
    }

    #[test]
    fn matrix_shows_chinese_fork() {
        let matrix = language_support_matrix();
        let by = |p: &str| matrix.iter().find(|r| r.provider == p).unwrap().clone();

        // The 8-way Chinese fork the research calls out.
        assert_eq!(by("deepgram").example_cmn_cn.as_deref(), Some("zh-CN"));
        assert_eq!(by("azure").example_cmn_cn.as_deref(), Some("zh-CN"));
        assert_eq!(by("aws-polly").example_cmn_cn.as_deref(), Some("cmn-CN"));
        assert_eq!(
            by("google-stt").example_cmn_cn.as_deref(),
            Some("cmn-Hans-CN")
        );
        assert_eq!(by("google-tts").example_cmn_cn.as_deref(), Some("cmn-CN"));
        assert_eq!(by("iflytek").example_cmn_cn.as_deref(), Some("zh_cn"));
        assert_eq!(by("baidu").example_cmn_cn.as_deref(), Some("1537"));
        assert_eq!(by("tencent").example_cmn_cn.as_deref(), Some("zh"));
        assert_eq!(by("speechmatics").example_cmn_cn.as_deref(), Some("cmn"));
        assert_eq!(by("baidu-tts").example_cmn_cn.as_deref(), Some("zh"));
        // hume takes no language param.
        assert_eq!(by("hume").example_cmn_cn, None);
    }

    #[test]
    fn matrix_en_us_across_groups() {
        let matrix = language_support_matrix();
        let by = |p: &str| matrix.iter().find(|r| r.provider == p).unwrap().clone();
        assert_eq!(by("deepgram").example_en_us.as_deref(), Some("en-US")); // identity
        assert_eq!(by("elevenlabs").example_en_us.as_deref(), Some("en")); // downgrade
        assert_eq!(by("iflytek").example_en_us.as_deref(), Some("en_us")); // underscore
        assert_eq!(by("baidu").example_en_us.as_deref(), Some("1737")); // numeric
    }
}
