//! Notation standardization chassis (ported from `waav-infer-components::standardize`).
//!
//! WaaV exposes ONE canonical notation per concept across its whole API — a developer relies on
//! the same language token (`en-US`, or the bare-ISO shorthand `en`) for EVERY provider — and the
//! gateway maps that canonical token to each provider's native notation internally.
//!
//! Two pieces, mirroring the infer engine:
//!   * [`resolve_alias`] — fold the many spellings a user might type (case, `_`/space separators,
//!     reversed `us-en`, language names, ISO 639-2/3, region subtags, vendor quirks) onto one
//!     canonical [`CanonicalLanguage`] variant.
//!   * [`NotationMap`] — a per-provider `canonical → native` table (native may be `en-US`, the
//!     ISO-639-1 `en`, a stringified numeric model id like `1537`, or a fused token like `16k_zh`),
//!     plus an optional `auto` token, with the supported set for capability reporting.
//!
//! KEY DIFFERENCE from infer's language-only canonical: here the alias-table value is a
//! region-qualified [`CanonicalLanguage`] enum variant (e.g. `en-US`), and [`resolve_alias`] does
//! **not** strip region to base by default — instead the table maps BOTH region-qualified spellings
//! and bare-ISO shorthands to the SAME variant. The base-subtag fallback only kicks in for an
//! *unlisted* region (so a novel `en-ZZ` still resolves to the `en` default region rather than
//! failing), never to collapse a listed region.

use super::types::{CanonicalLanguage, LANG_ALIASES};

/// Fold a user token onto a canonical [`CanonicalLanguage`] via `table` (alias → canonical).
/// Case-insensitive; `_`/spaces are treated as `-`. If the full normalized token isn't found, the
/// base subtag before the first `-` is tried against the bare-ISO shorthands (so an *unlisted*
/// region like `en-ZZ` falls back to the language's default region, e.g. `en` → en-US). Returns
/// `None` if unrecognized (the caller passes the raw token through + emits a config_warning).
pub fn resolve_alias(
    input: &str,
    table: &[(&str, CanonicalLanguage)],
) -> Option<CanonicalLanguage> {
    let norm = input.trim().to_ascii_lowercase().replace([' ', '_'], "-");
    if let Some((_, c)) = table.iter().find(|(a, _)| *a == norm) {
        return Some(*c);
    }
    // Base-subtag fallback: an UNLISTED region (e.g. `en-ZZ`, `es-XX`) resolves to the language's
    // default-region canonical via the bare-ISO shorthand. This never collapses a *listed* region
    // (those matched above) — it only rescues novel/region-qualified spellings we don't enumerate.
    let base = norm.split('-').next().unwrap_or("");
    if !base.is_empty()
        && base != norm
        && let Some((_, c)) = table.iter().find(|(a, _)| *a == base)
    {
        return Some(*c);
    }
    None
}

/// Resolve any language token (alias spelling, region-qualified canonical, bare-ISO shorthand,
/// vendor quirk, name, ISO-639-2/3, or `auto`/`detect`/empty) to a [`CanonicalLanguage`].
///
/// `auto`, `detect`, `any`, `multilingual`, `multi`, and the empty string resolve to
/// [`CanonicalLanguage::Auto`] (they are listed in [`LANG_ALIASES`]; the empty string falls through
/// the base-subtag step to no match, so it is special-cased here). Unknown tokens return `None`
/// (caller passes through verbatim).
pub fn resolve(input: &str) -> Option<CanonicalLanguage> {
    if input.trim().is_empty() {
        return Some(CanonicalLanguage::Auto);
    }
    resolve_alias(input, LANG_ALIASES)
}

/// A per-provider `canonical → native` notation map (+ an optional `auto` native token).
///
/// The native is a `String` so a provider whose notation is a numeric model id (Baidu `1537`) or a
/// fused token (Tencent `16k_zh`) stores it as a string, keeping this type concept-agnostic. The
/// map keys on the canonical's BCP-47 string ([`CanonicalLanguage::as_bcp47`]) so the table works
/// exactly like infer's. Build it fluently: `NotationMap::empty().with(EnUs, "en-US").with_auto(..)`.
#[derive(Clone, Debug, Default)]
pub struct NotationMap {
    /// `canonical.as_bcp47()` → provider-native string.
    to_native: Vec<(&'static str, String)>,
    /// Native token for "auto-detect" (resolved when the canonical is [`CanonicalLanguage::Auto`]).
    auto_native: Option<String>,
}

impl NotationMap {
    /// An empty map (no overrides, no auto token). The base case for the builder.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Build from explicit `(canonical, native)` pairs.
    pub fn new(pairs: impl IntoIterator<Item = (CanonicalLanguage, String)>) -> Self {
        Self {
            to_native: pairs
                .into_iter()
                .map(|(c, n)| (c.as_bcp47(), n))
                .collect(),
            auto_native: None,
        }
    }

    /// Add a single `canonical → native` override, builder-style. A later `.with` for the same
    /// canonical overwrites the earlier one (last write wins).
    pub fn with(mut self, canonical: CanonicalLanguage, native: impl Into<String>) -> Self {
        let key = canonical.as_bcp47();
        let native = native.into();
        if let Some(slot) = self.to_native.iter_mut().find(|(c, _)| *c == key) {
            slot.1 = native;
        } else {
            self.to_native.push((key, native));
        }
        self
    }

    /// Declare the native token for "auto-detect" (resolved when the canonical is `Auto`).
    pub fn with_auto(mut self, native: impl Into<String>) -> Self {
        self.auto_native = Some(native.into());
        self
    }

    /// Map a canonical language to this provider's native notation. `Auto` → the auto token if the
    /// provider supports one, else `None` (caller warns + falls back to the provider default).
    pub fn to_native(&self, canonical: CanonicalLanguage) -> Option<&str> {
        if canonical == CanonicalLanguage::Auto {
            return self.auto_native.as_deref();
        }
        self.lookup(canonical.as_bcp47())
    }

    fn lookup(&self, bcp47: &str) -> Option<&str> {
        self.to_native
            .iter()
            .find(|(c, _)| *c == bcp47)
            .map(|(_, n)| n.as_str())
    }

    /// The supported canonical BCP-47 codes (sorted), for capability reporting.
    pub fn supported(&self) -> Vec<&'static str> {
        let mut v: Vec<&'static str> = self.to_native.iter().map(|(c, _)| *c).collect();
        v.sort_unstable();
        v
    }

    /// Whether this canonical language has an explicit override (not the default fallback).
    pub fn supports(&self, canonical: CanonicalLanguage) -> bool {
        if canonical == CanonicalLanguage::Auto {
            return self.auto_native.is_some();
        }
        self.lookup(canonical.as_bcp47()).is_some()
    }

    /// Whether this provider declares an auto-detect native token.
    pub fn supports_auto(&self) -> bool {
        self.auto_native.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_folds_reversed_and_underscore_and_name() {
        // The live-proven `us-en` reversal bug.
        assert_eq!(resolve("us-en"), Some(CanonicalLanguage::EnUs));
        assert_eq!(resolve("en_US"), Some(CanonicalLanguage::EnUs));
        assert_eq!(resolve("EN-us"), Some(CanonicalLanguage::EnUs));
        assert_eq!(resolve("english"), Some(CanonicalLanguage::EnUs));
        assert_eq!(resolve("eng"), Some(CanonicalLanguage::EnUs));
        assert_eq!(resolve("en"), Some(CanonicalLanguage::EnUs));
    }

    #[test]
    fn resolve_keeps_region_distinct() {
        assert_eq!(resolve("en-GB"), Some(CanonicalLanguage::EnGb));
        assert_eq!(resolve("en-IN"), Some(CanonicalLanguage::EnIn));
        assert_eq!(resolve("en-AU"), Some(CanonicalLanguage::EnAu));
        // en-US != en-GB MUST survive (cross-cutting quirk #8).
        assert_ne!(resolve("en-GB"), resolve("en-US"));
    }

    #[test]
    fn resolve_chinese_and_odia_and_norwegian() {
        assert_eq!(resolve("zh-CN"), Some(CanonicalLanguage::CmnCn));
        assert_eq!(resolve("cmn-Hans-CN"), Some(CanonicalLanguage::CmnCn));
        assert_eq!(resolve("cantonese"), Some(CanonicalLanguage::YueHk));
        assert_eq!(resolve("od-IN"), Some(CanonicalLanguage::OrIn));
        assert_eq!(resolve("no"), Some(CanonicalLanguage::NbNo));
    }

    #[test]
    fn resolve_auto_spellings() {
        for s in ["auto", "detect", "any", "multilingual", "multi", ""] {
            assert_eq!(resolve(s), Some(CanonicalLanguage::Auto), "{s:?} -> Auto");
        }
    }

    #[test]
    fn resolve_unlisted_region_falls_back_to_default_region() {
        // A region we don't enumerate still resolves via the base-subtag fallback to the
        // language's DEFAULT region (en -> en-US), rather than failing.
        assert_eq!(resolve("en-ZZ"), Some(CanonicalLanguage::EnUs));
        assert_eq!(resolve("es-XX"), Some(CanonicalLanguage::EsEs));
    }

    #[test]
    fn resolve_unknown_is_none() {
        assert_eq!(resolve("zzz"), None);
        assert_eq!(resolve("klingon"), None);
    }

    #[test]
    fn notation_map_builder_identity_and_auto() {
        let m = NotationMap::empty()
            .with(CanonicalLanguage::EnUs, "en-US")
            .with(CanonicalLanguage::EsEs, "es-ES")
            .with_auto("multi");
        assert_eq!(m.to_native(CanonicalLanguage::EnUs), Some("en-US"));
        assert_eq!(m.to_native(CanonicalLanguage::Auto), Some("multi"));
        assert_eq!(m.to_native(CanonicalLanguage::FrFr), None); // not in this map
        assert!(m.supports_auto());
        assert!(m.supports(CanonicalLanguage::EsEs));
        assert!(!m.supports(CanonicalLanguage::FrFr));
        assert_eq!(m.supported(), vec!["en-US", "es-ES"]);
    }

    #[test]
    fn notation_map_last_write_wins() {
        let m = NotationMap::empty()
            .with(CanonicalLanguage::CmnCn, "zh-CN")
            .with(CanonicalLanguage::CmnCn, "cmn-CN");
        assert_eq!(m.to_native(CanonicalLanguage::CmnCn), Some("cmn-CN"));
    }

    #[test]
    fn notation_map_numeric_native() {
        // Baidu-style: native tokens are stringified dev_pid ids.
        let m = NotationMap::new([
            (CanonicalLanguage::CmnCn, "1537".to_string()),
            (CanonicalLanguage::EnUs, "1737".to_string()),
            (CanonicalLanguage::YueHk, "1637".to_string()),
        ]);
        assert_eq!(m.to_native(CanonicalLanguage::CmnCn), Some("1537"));
        assert_eq!(m.to_native(CanonicalLanguage::YueHk), Some("1637"));
        assert!(!m.supports_auto());
        assert_eq!(m.to_native(CanonicalLanguage::Auto), None);
    }
}
