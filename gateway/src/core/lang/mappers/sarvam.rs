//! Sarvam (STT bulbul + TTS bulbul) — BCP-47-with-IN-region, but Odia is the NONSTANDARD `od-IN`
//! (the canonical is the correct ISO `or-IN`; Sarvam's quirk is emitted on the way OUT). Indian
//! languages + `en-IN` only; a non-Indian request warns and falls back to `hi-IN`.

use crate::core::lang::mapper::{
    LanguageMapper, MappedLanguage, NotationKind, ProviderLanguageSupport,
};
use crate::core::lang::mappers::INDIAN_LANGS;
use crate::core::lang::types::CanonicalLanguage;

pub struct SarvamLanguageMapper;

impl LanguageMapper for SarvamLanguageMapper {
    fn support(&self) -> ProviderLanguageSupport {
        ProviderLanguageSupport {
            provider_id: "sarvam",
            kind: NotationKind::Bcp47,
            // STT can omit language_code for auto-detect; TTS target_language_code is required, so
            // the registry-level support reports auto=true (STT) and the config layer warns on the
            // TTS-required path. We report true (the STT-friendly default).
            supports_auto: true,
            supported: INDIAN_LANGS.to_vec(),
        }
    }

    fn map(&self, lang: CanonicalLanguage, _model: &str) -> MappedLanguage {
        match lang {
            // STT supports auto-detect (omit language_code / "unknown").
            CanonicalLanguage::Auto => MappedLanguage::omitted(),
            // Odia: emit Sarvam's nonstandard od-IN.
            CanonicalLanguage::OrIn => MappedLanguage::native("od-IN"),
            other if INDIAN_LANGS.contains(&other) => MappedLanguage::native(other.as_bcp47()),
            other => MappedLanguage::native("hi-IN").warn(format!(
                "Sarvam supports only Indian languages; {} fell back to hi-IN",
                other.as_bcp47()
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sarvam_indian_languages_pass() {
        let m = SarvamLanguageMapper;
        assert_eq!(m.map(CanonicalLanguage::HiIn, "").native, "hi-IN");
        assert_eq!(m.map(CanonicalLanguage::TaIn, "").native, "ta-IN");
        assert_eq!(m.map(CanonicalLanguage::EnIn, "").native, "en-IN");
    }

    #[test]
    fn sarvam_odia_emits_nonstandard_od_in() {
        let m = SarvamLanguageMapper;
        // canonical or-IN -> Sarvam's quirky od-IN on the way OUT.
        assert_eq!(m.map(CanonicalLanguage::OrIn, "").native, "od-IN");
    }

    #[test]
    fn sarvam_non_indian_falls_back() {
        let m = SarvamLanguageMapper;
        let r = m.map(CanonicalLanguage::FrFr, "");
        assert_eq!(r.native, "hi-IN");
        assert!(r.has_warnings());
    }

    #[test]
    fn sarvam_auto_omits() {
        assert!(SarvamLanguageMapper.map(CanonicalLanguage::Auto, "").omit);
    }
}
