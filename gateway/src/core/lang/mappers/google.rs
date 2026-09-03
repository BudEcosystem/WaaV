//! Google STT vs TTS — the canonical MODEL-AWARE divergence.
//!
//! The SAME canonical `cmn-CN` renders as `cmn-Hans-CN` for Google STT V2 (script-qualified) but
//! `cmn-CN` for Google TTS (NO script subtag). This is why `to_provider_language` is model/service
//! aware. Both are otherwise BCP-47 verbatim with region preserved.

use crate::core::lang::mapper::{
    LanguageMapper, MappedLanguage, NotationKind, ProviderLanguageSupport,
};
use crate::core::lang::types::CanonicalLanguage;

/// Google STT V2: BCP-47, but Chinese is SCRIPT-qualified (`cmn-Hans-CN`, `cmn-Hant-TW`,
/// `yue-Hant-HK`). `language_codes` is a LIST on the wire; we return the single primary code.
pub struct GoogleSttLanguageMapper;

impl LanguageMapper for GoogleSttLanguageMapper {
    fn support(&self) -> ProviderLanguageSupport {
        ProviderLanguageSupport {
            provider_id: "google",
            kind: NotationKind::Bcp47,
            supports_auto: true, // V2 supports language auto-detection via a candidate list
            supported: vec![],
        }
    }

    fn map(&self, lang: CanonicalLanguage, _model: &str) -> MappedLanguage {
        match lang {
            CanonicalLanguage::Auto => MappedLanguage::omitted(),
            CanonicalLanguage::CmnCn => MappedLanguage::native("cmn-Hans-CN"),
            CanonicalLanguage::CmnTw => MappedLanguage::native("cmn-Hant-TW"),
            CanonicalLanguage::YueHk => MappedLanguage::native("yue-Hant-HK"),
            other => MappedLanguage::native(other.as_bcp47()),
        }
    }
}

/// Google TTS: BCP-47, Chinese WITHOUT a script subtag (`cmn-CN`, `yue-HK`). Diverges from
/// Google STT on the SAME canonical.
pub struct GoogleTtsLanguageMapper;

impl LanguageMapper for GoogleTtsLanguageMapper {
    fn support(&self) -> ProviderLanguageSupport {
        ProviderLanguageSupport {
            provider_id: "google-tts",
            kind: NotationKind::Bcp47,
            supports_auto: false, // TTS is text-driven; language fixes the voice
            supported: vec![],
        }
    }

    fn map(&self, lang: CanonicalLanguage, _model: &str) -> MappedLanguage {
        match lang {
            // TTS has no detection; default to en-US + warning.
            CanonicalLanguage::Auto => MappedLanguage::native("en-US")
                .warn("Google TTS has no language detection; defaulting to en-US"),
            // cmn-CN/yue-HK keep their canonical BCP-47 form (no script subtag) — i.e. the default.
            other => MappedLanguage::native(other.as_bcp47()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn google_stt_script_qualifies_chinese() {
        let m = GoogleSttLanguageMapper;
        assert_eq!(m.map(CanonicalLanguage::EnUs, "").native, "en-US");
        assert_eq!(m.map(CanonicalLanguage::CmnCn, "").native, "cmn-Hans-CN");
        assert_eq!(m.map(CanonicalLanguage::CmnTw, "").native, "cmn-Hant-TW");
        assert_eq!(m.map(CanonicalLanguage::YueHk, "").native, "yue-Hant-HK");
        assert!(m.map(CanonicalLanguage::Auto, "").omit);
    }

    #[test]
    fn google_tts_no_script_subtag() {
        let m = GoogleTtsLanguageMapper;
        assert_eq!(m.map(CanonicalLanguage::EnUs, "").native, "en-US");
        // SAME canonical cmn-CN, DIFFERENT native vs STT — the model-aware case.
        assert_eq!(m.map(CanonicalLanguage::CmnCn, "").native, "cmn-CN");
        assert_eq!(m.map(CanonicalLanguage::YueHk, "").native, "yue-HK");
    }

    #[test]
    fn same_canonical_diverges_across_google_services() {
        // The headline model-aware proof.
        let stt = GoogleSttLanguageMapper.map(CanonicalLanguage::CmnCn, "");
        let tts = GoogleTtsLanguageMapper.map(CanonicalLanguage::CmnCn, "");
        assert_ne!(stt.native, tts.native);
        assert_eq!(stt.native, "cmn-Hans-CN");
        assert_eq!(tts.native, "cmn-CN");
    }
}
