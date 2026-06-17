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
pub mod notation;
pub mod types;

// ---- Public re-exports (the surface the rest of the gateway uses) ----------

pub use mapper::{
    LanguageMapper, MappedLanguage, NotationKind, ProviderLanguageSupport, to_provider_language,
};
pub use mappers::get_language_mapper;
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
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_language_resolves_then_maps() {
        // us-en -> EnUs -> deepgram "en-US".
        let m = map_language("us-en", "deepgram", "");
        assert_eq!(m.native, "en-US");
        assert!(!m.has_warnings());

        // en-US -> elevenlabs "en" (downgrade).
        assert_eq!(map_language("en-US", "elevenlabs", "eleven_turbo_v2_5").native, "en");
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
        assert_eq!(by("google-stt").example_cmn_cn.as_deref(), Some("cmn-Hans-CN"));
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
