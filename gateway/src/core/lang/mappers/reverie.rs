//! Reverie TTS — composite speaker code `{lang}_{gender}` (e.g. `hi_female`). The LANGUAGE half is
//! `canonical.iso639_1()`; the gender half stays the existing `ReverieGender` pipeline. This mapper
//! supplies ONLY the language half (the config-build joins it with the gender), replacing the
//! ad-hoc `format!("{}_{}", language, gender)` that consumed a raw client string.
//!
//! Reverie is Indian-language only (22+ Indian langs); a non-Indian request warns and falls back
//! to Hindi.

use crate::core::lang::mapper::{
    LanguageMapper, MappedLanguage, NotationKind, ProviderLanguageSupport,
};
use crate::core::lang::mappers::INDIAN_LANGS;
use crate::core::lang::types::CanonicalLanguage;

pub struct ReverieTtsLanguageMapper;

impl LanguageMapper for ReverieTtsLanguageMapper {
    fn support(&self) -> ProviderLanguageSupport {
        ProviderLanguageSupport {
            provider_id: "reverie-tts",
            kind: NotationKind::Iso6391, // the {lang} half is the ISO-639-1 code
            supports_auto: false,
            supported: INDIAN_LANGS.to_vec(),
        }
    }

    fn map(&self, lang: CanonicalLanguage, _model: &str) -> MappedLanguage {
        if lang == CanonicalLanguage::Auto {
            return MappedLanguage::native(CanonicalLanguage::HiIn.iso639_1())
                .warn("Reverie TTS requires an explicit language; defaulting to Hindi (hi)");
        }
        if !INDIAN_LANGS.contains(&lang) {
            return MappedLanguage::native(CanonicalLanguage::HiIn.iso639_1()).warn(format!(
                "Reverie TTS supports only Indian languages; {} fell back to Hindi (hi)",
                lang.as_bcp47()
            ));
        }
        // The {lang} half — the gender half is appended by the Reverie config build.
        MappedLanguage::native(lang.iso639_1())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reverie_tts_supplies_iso_lang_half() {
        let m = ReverieTtsLanguageMapper;
        assert_eq!(m.map(CanonicalLanguage::HiIn, "").native, "hi");
        assert_eq!(m.map(CanonicalLanguage::TaIn, "").native, "ta");
        assert_eq!(m.map(CanonicalLanguage::EnIn, "").native, "en");
    }

    #[test]
    fn reverie_tts_non_indian_falls_back_to_hindi() {
        let m = ReverieTtsLanguageMapper;
        let r = m.map(CanonicalLanguage::FrFr, "");
        assert_eq!(r.native, "hi");
        assert!(r.has_warnings());
    }
}
