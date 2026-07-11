//! ElevenLabs (TTS `language_code` + STT) — the canonical "ISO-639-1-only downgrade" case. Region
//! is DROPPED (`en-US` → `en`). Cantonese is unsupported (→ warning). The `language_code` body
//! field is only honored by the v2.5 models (Turbo/Flash v2.5); on other models ElevenLabs ignores
//! it, so we attach a model-aware warning.

use crate::core::lang::mapper::{
    LanguageMapper, MappedLanguage, NotationKind, ProviderLanguageSupport,
};
use crate::core::lang::types::CanonicalLanguage;

pub struct ElevenLabsLanguageMapper;

/// Whether `model` honors the `language_code` enforcement (the v2.5 family).
fn model_honors_language(model: &str) -> bool {
    let m = model.to_ascii_lowercase();
    // Empty model == provider default (which is a v2.5-capable multilingual model) → assume honored.
    m.is_empty()
        || m.contains("v2_5")
        || m.contains("v2.5")
        || m.contains("turbo")
        || m.contains("flash")
}

impl LanguageMapper for ElevenLabsLanguageMapper {
    fn support(&self) -> ProviderLanguageSupport {
        ProviderLanguageSupport {
            provider_id: "elevenlabs",
            kind: NotationKind::Iso6391,
            supports_auto: true, // STT include_language_detection; TTS omit == auto
            supported: vec![],
        }
    }

    fn map(&self, lang: CanonicalLanguage, model: &str) -> MappedLanguage {
        // Auto → omit the language field (STT auto-detect / TTS text-driven).
        if lang == CanonicalLanguage::Auto {
            return MappedLanguage::omitted();
        }
        // Cantonese: ElevenLabs has no `yue`; fall back to `zh` + warning.
        if lang == CanonicalLanguage::YueHk {
            let mut m = MappedLanguage::native("zh")
                .warn("ElevenLabs does not support Cantonese (yue); using zh");
            if !model_honors_language(model) {
                m = m.warn(format!(
                    "ElevenLabs ignores language_code on model '{model}' (only v2.5 models enforce it)"
                ));
            }
            return m;
        }
        let native = lang.iso639_1().to_string();
        let mut mapped = MappedLanguage::native(native);
        if !model_honors_language(model) {
            mapped = mapped.warn(format!(
                "ElevenLabs ignores language_code on model '{model}' (only v2.5 models enforce it)"
            ));
        }
        mapped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elevenlabs_downgrades_to_iso639_1() {
        let m = ElevenLabsLanguageMapper;
        // en-US -> "en" (region dropped) — the headline downgrade case.
        assert_eq!(
            m.map(CanonicalLanguage::EnUs, "eleven_turbo_v2_5").native,
            "en"
        );
        assert_eq!(
            m.map(CanonicalLanguage::EsEs, "eleven_turbo_v2_5").native,
            "es"
        );
        assert_eq!(
            m.map(CanonicalLanguage::CmnCn, "eleven_turbo_v2_5").native,
            "zh"
        );
    }

    #[test]
    fn elevenlabs_non_v25_model_warns() {
        let m = ElevenLabsLanguageMapper;
        let r = m.map(CanonicalLanguage::EnUs, "eleven_multilingual_v2");
        assert_eq!(r.native, "en");
        assert!(
            r.has_warnings(),
            "non-v2.5 model must warn it ignores language"
        );
    }

    #[test]
    fn elevenlabs_default_model_no_warning() {
        // Empty model == provider default (v2.5-capable) → no warning.
        let r = ElevenLabsLanguageMapper.map(CanonicalLanguage::EnUs, "");
        assert!(!r.has_warnings());
    }

    #[test]
    fn elevenlabs_cantonese_warns_and_uses_zh() {
        let r = ElevenLabsLanguageMapper.map(CanonicalLanguage::YueHk, "eleven_turbo_v2_5");
        assert_eq!(r.native, "zh");
        assert!(r.has_warnings());
    }

    #[test]
    fn elevenlabs_auto_omits() {
        assert!(
            ElevenLabsLanguageMapper
                .map(CanonicalLanguage::Auto, "")
                .omit
        );
    }
}
