//! Language mapper trait, the [`MappedLanguage`] result, and the per-provider mapping rules.
//!
//! Mirrors the proven [`crate::core::emotion::mapper`] chassis: a [`LanguageMapper`] trait, a
//! [`MappedLanguage`] output carrying the native string + graceful-degrade warnings, and a
//! [`ProviderLanguageSupport`] descriptor. The per-provider implementations live in
//! [`super::mappers`]; the registry is [`super::mappers::get_language_mapper`].
//!
//! The mapping is model-AWARE: [`LanguageMapper::map`] takes the `model` string so the few
//! providers whose native notation depends on the service/model (Google STT `cmn-Hans-CN` vs
//! Google TTS `cmn-CN` for the SAME canonical) can disambiguate.

use super::types::CanonicalLanguage;
use serde::{Deserialize, Serialize};

// =============================================================================
// Notation kind
// =============================================================================

/// How a provider spells a language by DEFAULT (before per-canonical overrides).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotationKind {
    /// Region-qualified BCP-47 verbatim (`en-US`). Deepgram, Azure, AWS, Sarvam, Google TTS, ….
    Bcp47,
    /// ISO-639-1 two-letter downgrade, region dropped (`en`). ElevenLabs, OpenAI STT, Cartesia,
    /// AssemblyAI, Speechmatics, Whisper-family.
    Iso6391,
    /// Underscore-joined `lc_RR` (`en_us`). iFlytek.
    Underscore,
    /// No language parameter at all — language is inferred from text / voice description, or the
    /// provider is auto-detect-only. Hume TTS, OpenAI TTS, Nova-Sonic, Gemini native-audio.
    None,
}

impl NotationKind {
    /// A stable lowercase token for the capability surface (`bcp47`, `iso6391`,
    /// `underscore`, `none`). Avoids leaning on a derived schema for the
    /// `/capabilities` row.
    pub const fn as_str(&self) -> &'static str {
        match self {
            NotationKind::Bcp47 => "bcp47",
            NotationKind::Iso6391 => "iso6391",
            NotationKind::Underscore => "underscore",
            NotationKind::None => "none",
        }
    }
}

// =============================================================================
// Provider language support descriptor
// =============================================================================

/// Describes how one provider handles language — its notation kind, whether it auto-detects, and
/// the canonical set it explicitly supports. Mirrors [`crate::core::emotion::ProviderEmotionSupport`].
#[derive(Debug, Clone)]
pub struct ProviderLanguageSupport {
    /// Provider identifier (`"deepgram"`, `"elevenlabs"`, …).
    pub provider_id: &'static str,
    /// The default notation kind.
    pub kind: NotationKind,
    /// Whether the provider can auto-detect / code-switch (the canonical `Auto` is honored
    /// natively, not warn-and-defaulted).
    pub supports_auto: bool,
    /// The canonical languages this provider supports. Empty means "treat the default notation as
    /// universally applicable" (no per-language gating, used by the broad BCP-47 providers).
    pub supported: Vec<CanonicalLanguage>,
}

impl ProviderLanguageSupport {
    /// Whether a specific canonical language is supported. An empty `supported` set means the
    /// provider accepts any language at its default notation (no gating).
    pub fn supports(&self, lang: CanonicalLanguage) -> bool {
        if lang == CanonicalLanguage::Auto {
            return self.supports_auto;
        }
        self.supported.is_empty() || self.supported.contains(&lang)
    }
}

// =============================================================================
// Mapped language output (mirrors MappedEmotion)
// =============================================================================

/// The result of mapping a canonical language to a provider's native notation.
///
/// `native` is the string the provider's config-build should use (it may be the empty string for
/// the SPECIAL providers that take no language parameter, in which case the provider keeps its own
/// behavior). `warnings` carries any graceful-degrade advisories (unsupported language, auto
/// requested where the provider can't detect, …) that the config layer surfaces as a
/// `config_warning`. NEVER a hard error.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MappedLanguage {
    /// The provider-native language string (`"en-US"`, `"en"`, `"zh_cn"`, `"1537"`, `"hi_female"`
    /// half, …). Empty string == "omit the language field / no-op" (SPECIAL providers).
    pub native: String,
    /// Whether the language field should be OMITTED entirely (distinct from native==""): used for
    /// the auto/native-detect case where sending nothing is the correct behavior.
    pub omit: bool,
    /// Graceful-degrade advisories. Each becomes a `config_warning`; never fatal.
    pub warnings: Vec<String>,
    /// Optional structured companions a provider needs alongside the bare language string, e.g.
    /// iFlytek's `accent=mandarin`, AWS Transcribe's `{identify_language, language_options}`. Keys
    /// are provider-defined; the provider config-build reads what it knows.
    pub companions: Vec<(&'static str, String)>,
}

impl MappedLanguage {
    /// A plain native string, no warnings.
    pub fn native(s: impl Into<String>) -> Self {
        Self {
            native: s.into(),
            ..Default::default()
        }
    }

    /// "Omit the language field" (auto/native-detect). Empty native, `omit = true`.
    pub fn omitted() -> Self {
        Self {
            omit: true,
            ..Default::default()
        }
    }

    /// Attach a warning, builder-style.
    pub fn warn(mut self, w: impl Into<String>) -> Self {
        self.warnings.push(w.into());
        self
    }

    /// Attach a companion key/value, builder-style.
    pub fn companion(mut self, key: &'static str, value: impl Into<String>) -> Self {
        self.companions.push((key, value.into()));
        self
    }

    /// Whether any warnings were produced.
    pub fn has_warnings(&self) -> bool {
        !self.warnings.is_empty()
    }

    /// Format warnings as a single `;`-joined advisory string, or `None` if clean.
    pub fn format_warnings(&self) -> Option<String> {
        if self.warnings.is_empty() {
            None
        } else {
            Some(self.warnings.join("; "))
        }
    }
}

// =============================================================================
// LanguageMapper trait
// =============================================================================

/// Maps a [`CanonicalLanguage`] to one provider's native notation. Each provider implements this
/// (or reuses [`super::mappers::DefaultLanguageMapper`] parameterized by a [`NotationKind`] + an
/// override [`super::notation::NotationMap`]).
pub trait LanguageMapper: Send + Sync {
    /// The provider's language-support descriptor.
    fn support(&self) -> ProviderLanguageSupport;

    /// Map `lang` (model-aware) to this provider's native notation, with any warnings.
    ///
    /// `model` is the requested STT/TTS model (may be empty); the few model-aware providers
    /// (Google STT vs TTS, ElevenLabs v2.5-only language enforcement, …) branch on it.
    fn map(&self, lang: CanonicalLanguage, model: &str) -> MappedLanguage;

    /// The provider identifier.
    fn provider_id(&self) -> &'static str {
        self.support().provider_id
    }
}

// =============================================================================
// Public entry point
// =============================================================================

/// Map a canonical language to `provider`'s native notation (model-aware). The single call the
/// config→provider boundary uses. Never fails — unsupported languages come back as a default +
/// warning (see [`MappedLanguage`]).
///
/// `provider` is matched case-insensitively (and across the provider's alias spellings) by
/// [`super::mappers::get_language_mapper`].
pub fn to_provider_language(
    lang: CanonicalLanguage,
    provider: &str,
    model: &str,
) -> MappedLanguage {
    super::mappers::get_language_mapper(provider).map(lang, model)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapped_language_builders() {
        let m = MappedLanguage::native("en-US");
        assert_eq!(m.native, "en-US");
        assert!(!m.has_warnings());
        assert!(!m.omit);

        let w = MappedLanguage::native("hi-IN").warn("non-Indian language fell back to hi-IN");
        assert!(w.has_warnings());
        assert_eq!(
            w.format_warnings().unwrap(),
            "non-Indian language fell back to hi-IN"
        );

        let o = MappedLanguage::omitted();
        assert!(o.omit);
        assert_eq!(o.native, "");

        let c = MappedLanguage::native("zh_cn").companion("accent", "mandarin");
        assert_eq!(c.companions, vec![("accent", "mandarin".to_string())]);
    }

    #[test]
    fn support_gating() {
        let s = ProviderLanguageSupport {
            provider_id: "x",
            kind: NotationKind::Bcp47,
            supports_auto: true,
            supported: vec![CanonicalLanguage::EnUs, CanonicalLanguage::EsEs],
        };
        assert!(s.supports(CanonicalLanguage::EnUs));
        assert!(!s.supports(CanonicalLanguage::FrFr));
        assert!(s.supports(CanonicalLanguage::Auto)); // supports_auto = true

        let universal = ProviderLanguageSupport {
            provider_id: "y",
            kind: NotationKind::Bcp47,
            supports_auto: false,
            supported: vec![], // empty == no gating
        };
        assert!(universal.supports(CanonicalLanguage::FrFr));
        assert!(!universal.supports(CanonicalLanguage::Auto)); // supports_auto = false
    }
}
