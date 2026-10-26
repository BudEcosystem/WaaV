//! Deepgram: BCP-47 (`zh-CN` for Mandarin, `multi` for auto), then the code each model takes.
//!
//! Deepgram's nova-2 and nova-3 take region codes (`fr-FR`), its older models only some: the
//! `enhanced` and `base` (`general`) tiers and its Whisper take `fr` and refuse `fr-FR` with a
//! 400 at the handshake. The model's own list (`deepgram_models.json`, from Deepgram's model
//! catalogue, less the codes its streaming endpoint refuses) decides: the caller's own code when
//! the model lists it, else the canonical code, else its language subtag when the model lists
//! that. A model the table does not know keeps the region code. Norwegian is `no` (Deepgram
//! refuses `nb`).

use std::collections::HashMap;
use std::sync::OnceLock;

use super::DefaultLanguageMapper;
use crate::core::lang::mapper::{
    LanguageMapper, MappedLanguage, NotationKind, ProviderLanguageSupport,
};
use crate::core::lang::notation::NotationMap;
use crate::core::lang::types::CanonicalLanguage;

pub struct DeepgramLanguageMapper(DefaultLanguageMapper);

impl DeepgramLanguageMapper {
    pub fn new() -> Self {
        Self(
            DefaultLanguageMapper::new("deepgram", NotationKind::Bcp47, "Deepgram").with_overrides(
                NotationMap::empty()
                    .with(CanonicalLanguage::CmnCn, "zh-CN")
                    .with(CanonicalLanguage::CmnTw, "zh-TW")
                    .with(CanonicalLanguage::YueHk, "zh-HK")
                    // Deepgram's Norwegian is `no`; it answers 400 for `nb` and `nb-NO`.
                    .with(CanonicalLanguage::NbNo, "no")
                    // Nova-3 multilingual code-switch (NOT detect_language — streaming gap).
                    .with_auto("multi"),
            ),
        )
    }
}

impl Default for DeepgramLanguageMapper {
    fn default() -> Self {
        Self::new()
    }
}

/// The language codes `model` takes, by its canonical name or the short names Deepgram also
/// answers to (`nova-3` for `nova-3-general`, `enhanced`, `base`, `base-phonecall`).
fn accepted(model: &str) -> Option<&'static [String]> {
    static TABLE: OnceLock<HashMap<String, Vec<String>>> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        #[derive(serde::Deserialize)]
        struct File {
            models: HashMap<String, Vec<String>>,
        }
        serde_json::from_str::<File>(include_str!("../deepgram_models.json"))
            .expect("deepgram_models.json is the packaged model table")
            .models
            .into_iter()
            .map(|(name, codes)| (name.to_ascii_lowercase(), codes))
            .collect()
    });
    let model = model.trim().to_ascii_lowercase();
    let name = match model.as_str() {
        "base" => "general".to_owned(),
        "nova" | "nova-2" | "nova-3" | "enhanced" => format!("{model}-general"),
        "whisper" => "whisper-medium".to_owned(),
        other => other.strip_prefix("base-").unwrap_or(other).to_owned(),
    };
    table.get(&name).map(Vec::as_slice)
}

impl LanguageMapper for DeepgramLanguageMapper {
    fn support(&self) -> ProviderLanguageSupport {
        self.0.support()
    }

    fn map_raw(&self, raw: &str, lang: CanonicalLanguage, model: &str) -> MappedLanguage {
        let mut mapped = self.map(lang, model);
        let raw = raw.trim();
        if !mapped.omit && accepted(model).is_some_and(|codes| codes.iter().any(|c| c == raw)) {
            mapped.native = raw.to_owned();
        }
        mapped
    }

    fn map(&self, lang: CanonicalLanguage, model: &str) -> MappedLanguage {
        let mut mapped = self.0.map(lang, model);
        if let Some(codes) = accepted(model)
            && !mapped.omit
            && !codes.contains(&mapped.native)
        {
            let base = mapped.native.split('-').next().unwrap_or_default();
            if codes.iter().any(|code| code == base) {
                mapped.native = base.to_owned();
            }
        }
        mapped
    }
}

#[cfg(test)]
mod tests {
    use crate::core::lang::map_language;

    /// The code each model takes: the caller's own where the model lists it, region codes where
    /// the model lists them, the language subtag where it lists only that, today's code for a
    /// model the table does not know. The pairs Deepgram refused in the 2026-10-15 matrix
    /// (`no` sent as `nb`/`nb-NO`, `vi` as `vi-VN`, `ms` as `ms-MY`, `hi-Latn` as `hi-IN`,
    /// `es`/`fr` on general-dQw4w9WgXcQ as `es-ES`/`fr-FR`) get the code it took.
    #[test]
    fn each_deepgram_model_gets_a_code_it_takes() {
        let dg = |language: &str, model: &str| map_language(language, "deepgram", model).native;
        for (language, model, want) in [
            ("fr", "enhanced-general", "fr"),
            ("fr-FR", "enhanced", "fr"),
            ("de", "general", "de"),
            ("de", "base", "de"),
            ("ja", "base-general", "ja"),
            ("es", "whisper-large", "es"),
            ("fr", "nova-3", "fr"),
            ("fr-FR", "nova-3", "fr-FR"),
            ("de", "nova-2-general", "de"),
            ("de-DE", "nova-2-general", "de-DE"),
            ("ja", "nova-2", "ja"),
            ("no", "nova-3-general", "no"),
            ("nb", "nova-3-general", "no"),
            ("nb-NO", "nova-3", "no"),
            ("no", "enhanced-general", "no"),
            ("no", "general", "no"),
            ("no", "nova-2-general", "no"),
            ("vi", "nova-3-general", "vi"),
            ("vi-VN", "nova-3", "vi"),
            ("ms", "nova-2-general", "ms"),
            ("hi-Latn", "nova-general", "hi-Latn"),
            ("es", "general-dQw4w9WgXcQ", "es"),
            ("fr", "general-dQw4w9WgXcQ", "fr"),
            ("hi", "nova-3", "hi"),
            ("en", "enhanced-general", "en"),
            ("en-US", "enhanced-general", "en-US"),
            ("en-GB", "enhanced-general", "en"),
            ("zh", "general", "zh"),
            ("cmn-CN", "general", "zh-CN"),
            ("fr", "", "fr-FR"),
            ("fr", "a-model-nobody-listed", "fr-FR"),
            ("auto", "enhanced-general", "multi"),
        ] {
            assert_eq!(dg(language, model), want, "{language} on {model}");
        }
    }
}
