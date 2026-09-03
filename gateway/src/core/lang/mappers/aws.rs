//! AWS Polly (TTS) + AWS Transcribe (STT).
//!
//! Polly: BCP-47 enum values, but Arabic is the BARE `arb` (no region), Cantonese is UNSUPPORTED
//! (→ warning + fallback). The language is mostly voice-derived; this mapper supplies the
//! `language_code` for voice selection/validation.
//!
//! Transcribe: BCP-47 verbatim, Chinese as `zh-CN`/`zh-TW`. Auto is NOT a flag — it sets
//! `IdentifyLanguage=true` + REQUIRES a `LanguageOptions` candidate list; we surface that via
//! companions and warn that WaaV cannot supply candidates (falling back to en-US) when none exist.

use crate::core::lang::mapper::{
    LanguageMapper, MappedLanguage, NotationKind, ProviderLanguageSupport,
};
use crate::core::lang::types::CanonicalLanguage;

/// AWS Polly (TTS).
pub struct AwsPollyLanguageMapper;

impl LanguageMapper for AwsPollyLanguageMapper {
    fn support(&self) -> ProviderLanguageSupport {
        ProviderLanguageSupport {
            provider_id: "aws-polly",
            kind: NotationKind::Bcp47,
            supports_auto: false,
            supported: vec![],
        }
    }

    fn map(&self, lang: CanonicalLanguage, _model: &str) -> MappedLanguage {
        match lang {
            // Polly Arabic is the bare `arb` (no region).
            CanonicalLanguage::ArSa => MappedLanguage::native("arb"),
            // Polly DOES use cmn-CN (diverges from Deepgram/Azure's zh-CN).
            CanonicalLanguage::CmnCn => MappedLanguage::native("cmn-CN"),
            // Cantonese is unsupported by Polly.
            CanonicalLanguage::YueHk => MappedLanguage::native("cmn-CN")
                .warn("AWS Polly does not support Cantonese (yue-HK); using Mandarin cmn-CN"),
            // TTS has no detection; the voice fixes the language. Default to en-US (no warning —
            // Polly selects by voice, language is validation-only).
            CanonicalLanguage::Auto => MappedLanguage::native("en-US"),
            other => MappedLanguage::native(other.as_bcp47()),
        }
    }
}

/// AWS Transcribe (STT).
pub struct AwsTranscribeLanguageMapper;

impl LanguageMapper for AwsTranscribeLanguageMapper {
    fn support(&self) -> ProviderLanguageSupport {
        ProviderLanguageSupport {
            provider_id: "aws-transcribe",
            kind: NotationKind::Bcp47,
            supports_auto: true, // via IdentifyLanguage + candidate list
            supported: vec![],
        }
    }

    fn map(&self, lang: CanonicalLanguage, _model: &str) -> MappedLanguage {
        match lang {
            CanonicalLanguage::CmnCn => MappedLanguage::native("zh-CN"),
            CanonicalLanguage::CmnTw => MappedLanguage::native("zh-TW"),
            CanonicalLanguage::YueHk => MappedLanguage::native("zh-HK"),
            CanonicalLanguage::ArSa => MappedLanguage::native("ar-SA"),
            CanonicalLanguage::Auto => {
                // IdentifyLanguage needs a candidate LIST. WaaV has no candidates here → fall back
                // to en-US + companions signalling the intent, plus a warning.
                MappedLanguage::native("en-US")
                    .companion("identify_language", "true")
                    .warn(
                        "AWS Transcribe auto-detect needs a LanguageOptions candidate list; \
                         none supplied — defaulting to en-US (set identify_language + candidates \
                         via extras to enable)",
                    )
            }
            other => MappedLanguage::native(other.as_bcp47()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polly_arabic_is_bare_arb() {
        let m = AwsPollyLanguageMapper;
        assert_eq!(m.map(CanonicalLanguage::ArSa, "").native, "arb");
        assert_eq!(m.map(CanonicalLanguage::CmnCn, "").native, "cmn-CN");
        assert_eq!(m.map(CanonicalLanguage::EnUs, "").native, "en-US");
    }

    #[test]
    fn polly_cantonese_warns() {
        let r = AwsPollyLanguageMapper.map(CanonicalLanguage::YueHk, "");
        assert!(r.has_warnings());
    }

    #[test]
    fn transcribe_chinese_is_zh_and_auto_companion() {
        let m = AwsTranscribeLanguageMapper;
        assert_eq!(m.map(CanonicalLanguage::CmnCn, "").native, "zh-CN");
        let a = m.map(CanonicalLanguage::Auto, "");
        assert_eq!(a.native, "en-US");
        assert!(a.has_warnings());
        assert!(a.companions.iter().any(|(k, _)| *k == "identify_language"));
    }
}
