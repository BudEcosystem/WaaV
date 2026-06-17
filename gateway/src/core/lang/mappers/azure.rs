//! Azure (STT + TTS + voice-live realtime) — BCP-47 `ll-RR`, but Chinese is `zh-CN`/`zh-TW`/`zh-HK`
//! (Azure uses `zh-*`, NOT `cmn-*` — diverges from Polly/Google-TTS). Auto-detect needs a candidate
//! locale LIST (`AutoDetectSourceLanguageConfig`); with none supplied we warn + supply a small
//! default locale set via companions.

use crate::core::lang::mapper::{
    LanguageMapper, MappedLanguage, NotationKind, ProviderLanguageSupport,
};
use crate::core::lang::types::CanonicalLanguage;

pub struct AzureLanguageMapper;

/// A small default candidate locale set used when Auto is requested without explicit candidates.
const AZURE_DEFAULT_LID_CANDIDATES: &str = "en-US,es-ES,fr-FR,de-DE,zh-CN";

impl LanguageMapper for AzureLanguageMapper {
    fn support(&self) -> ProviderLanguageSupport {
        ProviderLanguageSupport {
            provider_id: "azure",
            kind: NotationKind::Bcp47,
            supports_auto: true, // via AutoDetectSourceLanguageConfig candidate list
            supported: vec![],
        }
    }

    fn map(&self, lang: CanonicalLanguage, _model: &str) -> MappedLanguage {
        match lang {
            CanonicalLanguage::CmnCn => MappedLanguage::native("zh-CN"),
            CanonicalLanguage::CmnTw => MappedLanguage::native("zh-TW"),
            CanonicalLanguage::YueHk => MappedLanguage::native("zh-HK"),
            CanonicalLanguage::Auto => MappedLanguage::native("en-US")
                .companion("auto_detect_candidates", AZURE_DEFAULT_LID_CANDIDATES)
                .warn(
                    "Azure auto-detect requires a candidate locale list; using a default set \
                     (en-US,es-ES,fr-FR,de-DE,zh-CN) — override via extras",
                ),
            other => MappedLanguage::native(other.as_bcp47()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn azure_chinese_is_zh_not_cmn() {
        let m = AzureLanguageMapper;
        assert_eq!(m.map(CanonicalLanguage::CmnCn, "").native, "zh-CN"); // NOT cmn-CN
        assert_eq!(m.map(CanonicalLanguage::EnUs, "").native, "en-US");
        assert_eq!(m.map(CanonicalLanguage::EsEs, "").native, "es-ES");
    }

    #[test]
    fn azure_auto_supplies_candidate_list() {
        let a = AzureLanguageMapper.map(CanonicalLanguage::Auto, "");
        assert!(a.has_warnings());
        assert!(
            a.companions
                .iter()
                .any(|(k, _)| *k == "auto_detect_candidates")
        );
    }
}
