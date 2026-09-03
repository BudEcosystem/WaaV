//! Speechmatics (STT + realtime + flow) — ISO-639-1 two-letter (`en`, `de`, `fr`), BUT Chinese uses
//! ISO-639-3 (`cmn`/`yue`, NOT `zh`) and Norwegian is `no` (not `nb`). `transcription_config.language`
//! is the bare code. Auto-detect = `language="auto"`.

use crate::core::lang::mapper::{
    LanguageMapper, MappedLanguage, NotationKind, ProviderLanguageSupport,
};
use crate::core::lang::types::CanonicalLanguage;

pub struct SpeechmaticsLanguageMapper;

impl LanguageMapper for SpeechmaticsLanguageMapper {
    fn support(&self) -> ProviderLanguageSupport {
        ProviderLanguageSupport {
            provider_id: "speechmatics",
            kind: NotationKind::Iso6391,
            supports_auto: true,
            supported: vec![],
        }
    }

    fn map(&self, lang: CanonicalLanguage, _model: &str) -> MappedLanguage {
        match lang {
            CanonicalLanguage::Auto => MappedLanguage::native("auto"),
            // ISO-639-3 for Chinese (NOT the `zh` macrolanguage).
            CanonicalLanguage::CmnCn | CanonicalLanguage::CmnTw => MappedLanguage::native("cmn"),
            CanonicalLanguage::YueHk => MappedLanguage::native("yue"),
            // Norwegian is `no` at Speechmatics (canonical is nb-NO -> iso639_1 `nb`).
            CanonicalLanguage::NbNo => MappedLanguage::native("no"),
            other => MappedLanguage::native(other.iso639_1()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speechmatics_iso639_1_and_chinese_iso639_3() {
        let m = SpeechmaticsLanguageMapper;
        assert_eq!(m.map(CanonicalLanguage::EnUs, "").native, "en");
        assert_eq!(m.map(CanonicalLanguage::DeDe, "").native, "de");
        // ISO-639-3 cmn/yue, not zh.
        assert_eq!(m.map(CanonicalLanguage::CmnCn, "").native, "cmn");
        assert_eq!(m.map(CanonicalLanguage::YueHk, "").native, "yue");
        // Norwegian special.
        assert_eq!(m.map(CanonicalLanguage::NbNo, "").native, "no");
        // auto native.
        assert_eq!(m.map(CanonicalLanguage::Auto, "").native, "auto");
    }
}
