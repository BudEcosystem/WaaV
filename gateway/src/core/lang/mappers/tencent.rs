//! Tencent ASR — composite `EngineModelType` = sample-rate prefix + language stem (`16k_zh`,
//! `16k_en`, `16k_yue`). The mapper returns the LANGUAGE STEM (`zh`/`en`/`yue`/`ja`/`ko`); the
//! Tencent config builder prepends the `16k_` rate prefix, keeping rate composition where it
//! belongs.

use crate::core::lang::mapper::{
    LanguageMapper, MappedLanguage, NotationKind, ProviderLanguageSupport,
};
use crate::core::lang::types::CanonicalLanguage;

pub struct TencentLanguageMapper;

impl LanguageMapper for TencentLanguageMapper {
    fn support(&self) -> ProviderLanguageSupport {
        ProviderLanguageSupport {
            provider_id: "tencent",
            kind: NotationKind::Iso6391, // the stem is ISO-639-1-ish; rate is composed by the builder
            supports_auto: false,
            supported: vec![
                CanonicalLanguage::CmnCn,
                CanonicalLanguage::EnUs,
                CanonicalLanguage::YueHk,
                CanonicalLanguage::JaJp,
                CanonicalLanguage::KoKr,
            ],
        }
    }

    fn map(&self, lang: CanonicalLanguage, _model: &str) -> MappedLanguage {
        let stem = match lang {
            CanonicalLanguage::CmnCn | CanonicalLanguage::CmnTw => Some("zh"),
            CanonicalLanguage::YueHk => Some("yue"),
            CanonicalLanguage::EnUs | CanonicalLanguage::EnGb => Some("en"),
            CanonicalLanguage::JaJp => Some("ja"),
            CanonicalLanguage::KoKr => Some("ko"),
            _ => None,
        };
        match stem {
            Some(s) => MappedLanguage::native(s),
            None if lang == CanonicalLanguage::Auto => MappedLanguage::native("zh").warn(
                "Tencent ASR requires an explicit language; defaulting to zh (16k_zh)",
            ),
            None => MappedLanguage::native("zh").warn(format!(
                "Tencent ASR does not support {}; defaulting to zh (16k_zh)",
                lang.as_bcp47()
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tencent_returns_lang_stem() {
        let m = TencentLanguageMapper;
        assert_eq!(m.map(CanonicalLanguage::CmnCn, "").native, "zh");
        assert_eq!(m.map(CanonicalLanguage::EnUs, "").native, "en");
        assert_eq!(m.map(CanonicalLanguage::YueHk, "").native, "yue");
        assert_eq!(m.map(CanonicalLanguage::JaJp, "").native, "ja");
        assert!(m.map(CanonicalLanguage::FrFr, "").has_warnings());
    }
}
