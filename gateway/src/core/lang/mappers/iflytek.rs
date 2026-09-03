//! iFlytek (STT IAT/RTASR + TTS) — underscore `lc_RR` (`en_us`, `zh_cn`), plus an `accent=mandarin`
//! companion for Chinese. Replaces the hardcoded `zh_cn` the provider config used to bake in.

use crate::core::lang::mapper::{
    LanguageMapper, MappedLanguage, NotationKind, ProviderLanguageSupport,
};
use crate::core::lang::mappers::bcp47_underscore;
use crate::core::lang::types::CanonicalLanguage;

pub struct IFlytekLanguageMapper;

/// iFlytek's supported language set (the subset expressible in the gateway canonical enum — the
/// vendor also lists Bulgarian, which has no `CanonicalLanguage` variant yet, so it is omitted and
/// would hit the warn-and-default path).
const IFLYTEK_LANGS: &[CanonicalLanguage] = {
    use CanonicalLanguage::*;
    &[
        CmnCn, EnUs, MsMy, IdId, BnIn, HiIn, JaJp, KoKr, ThTh, ViVn, RuRu, DeDe, FrFr, ArSa,
    ]
};

impl LanguageMapper for IFlytekLanguageMapper {
    fn support(&self) -> ProviderLanguageSupport {
        ProviderLanguageSupport {
            provider_id: "iflytek",
            kind: NotationKind::Underscore,
            supports_auto: false,
            supported: IFLYTEK_LANGS.to_vec(),
        }
    }

    fn map(&self, lang: CanonicalLanguage, _model: &str) -> MappedLanguage {
        // Chinese → zh_cn + accent companion.
        if matches!(lang, CanonicalLanguage::CmnCn | CanonicalLanguage::CmnTw) {
            return MappedLanguage::native("zh_cn").companion("accent", "mandarin");
        }
        if lang == CanonicalLanguage::Auto {
            return MappedLanguage::native("zh_cn")
                .companion("accent", "mandarin")
                .warn("iFlytek requires an explicit language; defaulting to zh_cn");
        }
        if !IFLYTEK_LANGS.contains(&lang) {
            return MappedLanguage::native("zh_cn")
                .companion("accent", "mandarin")
                .warn(format!(
                    "iFlytek does not support {}; defaulting to zh_cn",
                    lang.as_bcp47()
                ));
        }
        MappedLanguage::native(bcp47_underscore(lang))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iflytek_underscore_and_accent() {
        let m = IFlytekLanguageMapper;
        assert_eq!(m.map(CanonicalLanguage::EnUs, "").native, "en_us");
        let zh = m.map(CanonicalLanguage::CmnCn, "");
        assert_eq!(zh.native, "zh_cn");
        assert!(
            zh.companions
                .iter()
                .any(|(k, v)| *k == "accent" && v == "mandarin")
        );
    }

    #[test]
    fn iflytek_unsupported_falls_back() {
        let r = IFlytekLanguageMapper.map(CanonicalLanguage::EsEs, "");
        assert_eq!(r.native, "zh_cn");
        assert!(r.has_warnings());
    }
}
