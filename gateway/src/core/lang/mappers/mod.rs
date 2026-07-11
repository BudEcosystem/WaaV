//! Per-provider language mappers + the registry.
//!
//! Most providers are covered by the generic [`DefaultLanguageMapper`] (a [`NotationKind`] default
//! + an override [`NotationMap`] + an optional supported-set gate + a fallback language). The few
//! genuinely composite / model-aware providers get dedicated mappers in sibling files:
//! [`google`] (STT script-qualified vs TTS), [`baidu`] (numeric dev_pid / `ct`), [`tencent`]
//! (composite `16k_*` stem), [`reverie`] (`{lang}_{gender}` half), [`sarvam`] (`od-IN` + IN-only),
//! [`aws`] (Polly `arb`/no-yue + Transcribe candidate-list auto), [`azure`] (zh-CN + candidate-list
//! auto), [`iflytek`] (underscore + `accent` companion), [`speechmatics`] (ISO-639-1 + cmn/yue/no),
//! [`elevenlabs`] (downgrade + v2.5 enforcement + no-yue).
//!
//! Anything not explicitly registered falls through to a safe BCP-47 pass-through default — the
//! research "GENERIC FALLBACK": the default path is correct for the long tail, and only the listed
//! overrides change it.

mod aws;
mod azure;
mod baidu;
mod elevenlabs;
mod google;
mod iflytek;
mod reverie;
mod sarvam;
mod speechmatics;
mod tencent;

pub use aws::{AwsPollyLanguageMapper, AwsTranscribeLanguageMapper};
pub use azure::AzureLanguageMapper;
pub use baidu::{BaiduSttLanguageMapper, BaiduTtsLanguageMapper};
pub use elevenlabs::ElevenLabsLanguageMapper;
pub use google::{GoogleSttLanguageMapper, GoogleTtsLanguageMapper};
pub use iflytek::IFlytekLanguageMapper;
pub use reverie::ReverieTtsLanguageMapper;
pub use sarvam::SarvamLanguageMapper;
pub use speechmatics::SpeechmaticsLanguageMapper;
pub use tencent::TencentLanguageMapper;

use super::mapper::{LanguageMapper, MappedLanguage, NotationKind, ProviderLanguageSupport};
use super::notation::NotationMap;
use super::types::CanonicalLanguage;

// =============================================================================
// Generic mapper
// =============================================================================

/// The generic, table-driven mapper that covers every provider whose native notation is a simple
/// function of the canonical (BCP-47 verbatim, ISO-639-1 downgrade, or `lc_RR` underscore) plus a
/// small set of per-canonical overrides. The composite/model-aware providers use dedicated mappers.
pub struct DefaultLanguageMapper {
    provider_id: &'static str,
    kind: NotationKind,
    /// Per-canonical native overrides (e.g. Deepgram `cmn-CN` → `zh-CN`).
    overrides: NotationMap,
    /// Canonical languages this provider supports. Empty == no gating (accept any at the default
    /// notation). A requested language outside a non-empty set falls back to `fallback` + warns.
    supported: &'static [CanonicalLanguage],
    /// Fallback canonical when an unsupported language is requested (and the set is non-empty).
    fallback: CanonicalLanguage,
    /// Human label for warnings ("ElevenLabs", "Reverie", …).
    label: &'static str,
}

impl DefaultLanguageMapper {
    pub fn new(provider_id: &'static str, kind: NotationKind, label: &'static str) -> Self {
        Self {
            provider_id,
            kind,
            overrides: NotationMap::empty(),
            supported: &[],
            fallback: CanonicalLanguage::EnUs,
            label,
        }
    }

    pub fn with_overrides(mut self, overrides: NotationMap) -> Self {
        self.overrides = overrides;
        self
    }

    pub fn with_supported(
        mut self,
        supported: &'static [CanonicalLanguage],
        fallback: CanonicalLanguage,
    ) -> Self {
        self.supported = supported;
        self.fallback = fallback;
        self
    }

    /// Render a non-Auto canonical to native via the group default (overrides win).
    fn render_default(&self, lang: CanonicalLanguage) -> String {
        if let Some(n) = self.overrides.to_native(lang) {
            return n.to_string();
        }
        match self.kind {
            NotationKind::Bcp47 => lang.as_bcp47().to_string(),
            NotationKind::Iso6391 => lang.iso639_1().to_string(),
            NotationKind::Underscore => bcp47_underscore(lang),
            NotationKind::None => String::new(),
        }
    }
}

/// `en-US` → `en_us` (lowercase, `-`→`_`). Used by the UNDERSCORE notation kind.
pub(crate) fn bcp47_underscore(lang: CanonicalLanguage) -> String {
    lang.as_bcp47().to_ascii_lowercase().replace('-', "_")
}

impl LanguageMapper for DefaultLanguageMapper {
    fn support(&self) -> ProviderLanguageSupport {
        ProviderLanguageSupport {
            provider_id: self.provider_id,
            kind: self.kind,
            supports_auto: self.overrides.supports_auto(),
            supported: self.supported.to_vec(),
        }
    }

    fn map(&self, lang: CanonicalLanguage, _model: &str) -> MappedLanguage {
        // ---- Auto ----
        if lang == CanonicalLanguage::Auto {
            if let Some(tok) = self.overrides.to_native(CanonicalLanguage::Auto) {
                return MappedLanguage::native(tok.to_string());
            }
            // No auto token. The SPECIAL (None) providers are auto/native by construction —
            // omit the field silently. Everyone else warns and falls back to their default lang.
            if self.kind == NotationKind::None {
                return MappedLanguage::omitted();
            }
            let fallback_native = self.render_default(self.fallback);
            return MappedLanguage::native(fallback_native).warn(format!(
                "auto language detection is not supported by {}; defaulting to {}",
                self.label,
                self.fallback.as_bcp47()
            ));
        }

        // ---- SPECIAL providers take no language param ----
        if self.kind == NotationKind::None {
            // Optionally warn for egregiously-unsupported languages when a supported set is given.
            if !self.supported.is_empty() && !self.supported.contains(&lang) {
                return MappedLanguage::omitted().warn(format!(
                    "{} infers language from text/voice and may not support {}",
                    self.label,
                    lang.as_bcp47()
                ));
            }
            return MappedLanguage::omitted();
        }

        // ---- Gated providers (non-empty supported set) ----
        if !self.supported.is_empty() && !self.supported.contains(&lang) {
            let fallback_native = self.render_default(self.fallback);
            return MappedLanguage::native(fallback_native).warn(format!(
                "{} does not support {}; falling back to {}",
                self.label,
                lang.as_bcp47(),
                self.fallback.as_bcp47()
            ));
        }

        MappedLanguage::native(self.render_default(lang))
    }
}

// =============================================================================
// Registry
// =============================================================================

/// Return the language mapper for `provider` (case-insensitive, alias-aware). Unknown providers get
/// a safe BCP-47 pass-through (the research GENERIC FALLBACK) so a brand-new provider still works.
pub fn get_language_mapper(provider: &str) -> Box<dyn LanguageMapper> {
    match provider.to_lowercase().as_str() {
        // ===== IDENTITY-BCP47 (region preserved; Chinese zh/cmn override) =====
        "deepgram" => Box::new(
            DefaultLanguageMapper::new("deepgram", NotationKind::Bcp47, "Deepgram").with_overrides(
                NotationMap::empty()
                    .with(CanonicalLanguage::CmnCn, "zh-CN")
                    .with(CanonicalLanguage::CmnTw, "zh-TW")
                    .with(CanonicalLanguage::YueHk, "zh-HK")
                    // Nova-3 multilingual code-switch (NOT detect_language — streaming gap).
                    .with_auto("multi"),
            ),
        ),
        "azure" | "microsoft-azure" | "microsoft_azure" => Box::new(AzureLanguageMapper),
        "aws-transcribe" | "aws_transcribe" => Box::new(AwsTranscribeLanguageMapper),
        "aws-polly" | "aws_polly" | "amazon-polly" | "polly" => Box::new(AwsPollyLanguageMapper),
        "nova-sonic" | "nova_sonic" | "novasonic" => Box::new(
            // Realtime S2S: auto-detect + code-switch native; pass BCP-47 as a hint otherwise.
            DefaultLanguageMapper::new("nova-sonic", NotationKind::Bcp47, "Nova-Sonic")
                .with_overrides(NotationMap::empty().with_auto("")),
        ),
        "gemini" | "google-gemini" | "gemini-live" => Box::new(
            // Live native-audio auto-switches 70+ langs; BCP-47 hint; cmn-CN passes (Google accepts cmn).
            DefaultLanguageMapper::new("gemini", NotationKind::Bcp47, "Gemini")
                .with_overrides(NotationMap::empty().with_auto("")),
        ),
        "yandex" | "yandex-speechkit" | "yandex_speechkit" | "speechkit" => Box::new(
            DefaultLanguageMapper::new("yandex", NotationKind::Bcp47, "Yandex")
                .with_overrides(NotationMap::empty().with_auto("auto")),
        ),
        "sarvam" => Box::new(SarvamLanguageMapper),
        "reverie" | "reverie-ai" | "reverie_ai" | "reverie-stt" | "reverieinc" => Box::new(
            // Reverie STT: ISO-639-1 short codes (hi/ta/mr), Indian-only. (TTS is the composite
            // mapper, selected by the TTS dispatch via `get_language_mapper("reverie-tts")`.)
            DefaultLanguageMapper::new("reverie", NotationKind::Iso6391, "Reverie")
                .with_supported(INDIAN_LANGS, CanonicalLanguage::HiIn),
        ),
        "reverie-tts" => Box::new(ReverieTtsLanguageMapper),
        "tinkoff" => Box::new(DefaultLanguageMapper::new(
            "tinkoff",
            NotationKind::Bcp47,
            "Tinkoff",
        )),

        // ===== DOWNGRADE-TO-ISO639-1 =====
        "elevenlabs" | "eleven_labs" => Box::new(ElevenLabsLanguageMapper),
        "openai" => Box::new(
            // STT whisper/gpt-4o-transcribe + realtime transcription: ISO-639-1; auto -> omit.
            DefaultLanguageMapper::new("openai", NotationKind::Iso6391, "OpenAI")
                .with_overrides(NotationMap::empty().with_auto("")),
        ),
        "openai-tts" => Box::new(
            // gpt-4o-mini-tts/tts-1 take NO language param (text auto-detect). Silent.
            DefaultLanguageMapper::new("openai-tts", NotationKind::None, "OpenAI TTS"),
        ),
        "cartesia" => Box::new(
            // Sonic: ISO-639-1; no auto flag (text-driven) -> Auto becomes "en" + warning via the
            // gated fallback path (kind != None, no auto token).
            DefaultLanguageMapper::new("cartesia", NotationKind::Iso6391, "Cartesia"),
        ),
        "assemblyai" => Box::new(
            DefaultLanguageMapper::new("assemblyai", NotationKind::Iso6391, "AssemblyAI")
                .with_supported(ASSEMBLYAI_LANGS, CanonicalLanguage::EnUs)
                // universal-streaming-multilingual: language_detection=true companion handled here.
                .with_overrides(NotationMap::empty().with_auto("__detect__")),
        ),
        "speechmatics" => Box::new(SpeechmaticsLanguageMapper),
        "groq" => Box::new(
            DefaultLanguageMapper::new("groq", NotationKind::Iso6391, "Groq")
                .with_overrides(NotationMap::empty().with_auto("")),
        ),
        "gladia" => Box::new(
            DefaultLanguageMapper::new("gladia", NotationKind::Iso6391, "Gladia")
                .with_overrides(NotationMap::empty().with_auto("")),
        ),
        "fpt-ai" | "fpt_ai" | "fptai" | "fpt" => Box::new(
            DefaultLanguageMapper::new("fpt-ai", NotationKind::Iso6391, "FPT.AI")
                .with_overrides(NotationMap::empty().with_auto("")),
        ),

        // ===== Google (model-aware: STT script-qualified vs TTS) =====
        "google" | "google-stt" | "google_stt" => Box::new(GoogleSttLanguageMapper),
        "google-tts" | "google_tts" => Box::new(GoogleTtsLanguageMapper),

        // ===== UNDERSCORE =====
        "iflytek" | "ifly" | "xfyun" | "xunfei" => Box::new(IFlytekLanguageMapper),

        // ===== ENUM / NUMERIC / COMPOSITE =====
        "baidu" | "baidu-stt" | "baidu_stt" => Box::new(BaiduSttLanguageMapper),
        "baidu-tts" | "baidu_tts" => Box::new(BaiduTtsLanguageMapper),
        "tencent" | "tencent-cloud" | "tencent_cloud" | "tencentcloud" => {
            Box::new(TencentLanguageMapper)
        }

        // ===== SPECIAL (no language param) =====
        "hume" | "hume-ai" | "hume_ai" => Box::new(DefaultLanguageMapper::new(
            "hume",
            NotationKind::None,
            "Hume",
        )),

        // ===== GENERIC FALLBACK: safe BCP-47 pass-through =====
        other => {
            // `other` is a &str into a temporary lowercase String; intern via a static-ish label.
            // We cannot keep `other` (borrowed) as a &'static str, so use a generic id.
            let _ = other;
            Box::new(DefaultLanguageMapper::new(
                "generic",
                NotationKind::Bcp47,
                "this provider",
            ))
        }
    }
}

/// The Indian-language set Reverie/Sarvam-family providers gate on (plus `en-IN`).
pub(crate) const INDIAN_LANGS: &[CanonicalLanguage] = {
    use CanonicalLanguage::*;
    &[
        HiIn, BnIn, TaIn, TeIn, GuIn, KnIn, MlIn, MrIn, PaIn, OrIn, EnIn,
    ]
};

/// AssemblyAI universal-streaming supported languages (en/es/fr/de/it/pt).
pub(crate) const ASSEMBLYAI_LANGS: &[CanonicalLanguage] = {
    use CanonicalLanguage::*;
    &[
        EnUs, EnGb, EnIn, EnAu, EsEs, EsMx, FrFr, DeDe, ItIt, PtBr, PtPt,
    ]
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::lang::mapper::to_provider_language;

    #[test]
    fn deepgram_identity_and_chinese_override_and_auto() {
        let dg = |l| to_provider_language(l, "deepgram", "");
        assert_eq!(dg(CanonicalLanguage::EnUs).native, "en-US");
        assert_eq!(dg(CanonicalLanguage::EsEs).native, "es-ES");
        // zh/cmn override
        assert_eq!(dg(CanonicalLanguage::CmnCn).native, "zh-CN");
        assert_eq!(dg(CanonicalLanguage::YueHk).native, "zh-HK");
        // auto -> "multi"
        let a = dg(CanonicalLanguage::Auto);
        assert_eq!(a.native, "multi");
        assert!(!a.has_warnings());
    }

    #[test]
    fn generic_fallback_is_bcp47_passthrough() {
        let m = to_provider_language(CanonicalLanguage::EnGb, "some-brand-new-provider", "");
        assert_eq!(m.native, "en-GB");
        assert!(!m.has_warnings());
    }

    #[test]
    fn underscore_helper() {
        assert_eq!(bcp47_underscore(CanonicalLanguage::EnUs), "en_us");
        assert_eq!(bcp47_underscore(CanonicalLanguage::CmnCn), "cmn_cn");
    }
}
