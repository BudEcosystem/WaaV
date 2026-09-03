//! Baidu — STT language IS a numeric `dev_pid` model id; TTS uses a `lan` enum where Cantonese is
//! the quirky `ct` (not `yue`).

use crate::core::lang::mapper::{
    LanguageMapper, MappedLanguage, NotationKind, ProviderLanguageSupport,
};
use crate::core::lang::types::CanonicalLanguage;

/// Baidu STT: the language IS the `dev_pid` numeric model id. Mandarin→1537, English→1737,
/// Cantonese→1637. The mapper returns the dev_pid string; the Baidu client parses it back to its
/// model enum.
pub struct BaiduSttLanguageMapper;

impl LanguageMapper for BaiduSttLanguageMapper {
    fn support(&self) -> ProviderLanguageSupport {
        ProviderLanguageSupport {
            provider_id: "baidu",
            kind: NotationKind::None, // not a textual code; a model id
            supports_auto: false,
            supported: vec![
                CanonicalLanguage::CmnCn,
                CanonicalLanguage::EnUs,
                CanonicalLanguage::YueHk,
            ],
        }
    }

    fn map(&self, lang: CanonicalLanguage, _model: &str) -> MappedLanguage {
        match lang {
            CanonicalLanguage::CmnCn => MappedLanguage::native("1537"),
            CanonicalLanguage::EnUs | CanonicalLanguage::EnGb => MappedLanguage::native("1737"),
            CanonicalLanguage::YueHk => MappedLanguage::native("1637"),
            CanonicalLanguage::Auto => MappedLanguage::native("1537").warn(
                "Baidu STT requires an explicit language; defaulting to Mandarin (dev_pid 1537)",
            ),
            other => MappedLanguage::native("1537").warn(format!(
                "Baidu STT does not support {}; defaulting to Mandarin (dev_pid 1537)",
                other.as_bcp47()
            )),
        }
    }
}

/// Baidu TTS: `lan` enum string. Mandarin→`zh`, English→`en`, Cantonese→`ct` (NOT `yue` — the
/// Baidu quirk).
pub struct BaiduTtsLanguageMapper;

impl LanguageMapper for BaiduTtsLanguageMapper {
    fn support(&self) -> ProviderLanguageSupport {
        ProviderLanguageSupport {
            provider_id: "baidu-tts",
            kind: NotationKind::Iso6391,
            supports_auto: false,
            supported: vec![
                CanonicalLanguage::CmnCn,
                CanonicalLanguage::EnUs,
                CanonicalLanguage::YueHk,
            ],
        }
    }

    fn map(&self, lang: CanonicalLanguage, _model: &str) -> MappedLanguage {
        match lang {
            CanonicalLanguage::CmnCn | CanonicalLanguage::CmnTw => MappedLanguage::native("zh"),
            CanonicalLanguage::YueHk => MappedLanguage::native("ct"), // quirk: not `yue`
            CanonicalLanguage::EnUs | CanonicalLanguage::EnGb => MappedLanguage::native("en"),
            CanonicalLanguage::Auto => MappedLanguage::native("zh")
                .warn("Baidu TTS requires an explicit language; defaulting to zh"),
            other => MappedLanguage::native("zh").warn(format!(
                "Baidu TTS does not support {}; defaulting to zh",
                other.as_bcp47()
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baidu_stt_returns_dev_pid() {
        let m = BaiduSttLanguageMapper;
        assert_eq!(m.map(CanonicalLanguage::CmnCn, "").native, "1537");
        assert_eq!(m.map(CanonicalLanguage::EnUs, "").native, "1737");
        assert_eq!(m.map(CanonicalLanguage::YueHk, "").native, "1637");
        assert!(m.map(CanonicalLanguage::Auto, "").has_warnings());
    }

    #[test]
    fn baidu_tts_cantonese_is_ct() {
        let m = BaiduTtsLanguageMapper;
        assert_eq!(m.map(CanonicalLanguage::CmnCn, "").native, "zh");
        assert_eq!(m.map(CanonicalLanguage::EnUs, "").native, "en");
        assert_eq!(m.map(CanonicalLanguage::YueHk, "").native, "ct"); // the quirk
    }
}
