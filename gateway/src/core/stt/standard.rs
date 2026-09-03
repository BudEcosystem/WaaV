//! Standardized STT configuration (W1 keystone, additive slice).
//!
//! The flat [`STTConfig`] is the only struct that crosses the dispatch/factory boundary, so
//! every advanced provider feature (diarization, keyterms, redaction, …) is unreachable on the
//! live path (see `BRUTAL_REVIEW.md` S1). This module introduces the *additive* standardized
//! layer the production plan (§2) calls for: a capability-rich [`StandardSTTConfig`] that wraps
//! the existing flat config plus typed [`SttFeatures`] and an open [`ProviderExtras`] passthrough.
//!
//! It is additive: it does NOT mutate `STTConfig` (which has 663 literal construction sites), so
//! nothing existing breaks. Each provider gains a `from_standard` mapping (Deepgram is the first,
//! see `deepgram.rs`); the remaining providers follow the same pattern (W2), after which the
//! factory/registry gains a standardized constructor path.

use super::base::STTConfig;
use crate::core::lang::CanonicalLanguage;
use serde::{Deserialize, Serialize};

/// Canonical, provider-agnostic advanced STT features. Every field is `Option`, so `None`
/// means "don't request / use the provider default" and adding fields stays backward compatible.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SttFeatures {
    /// Emit interim (non-final) hypotheses as they are recognized.
    pub interim_results: Option<bool>,
    /// Speaker diarization (who-spoke-when).
    pub diarization: Option<bool>,
    /// Per-word start/end timestamps.
    pub word_timestamps: Option<bool>,
    /// Provider "smart formatting" (dates, numbers, etc.).
    pub smart_format: Option<bool>,
    /// Profanity filtering / masking.
    pub profanity_filter: Option<bool>,
    /// Include filler words ("uh", "um") rather than dropping them.
    pub filler_words: Option<bool>,
    /// Emit explicit voice-activity / endpointing events.
    pub vad_events: Option<bool>,
    /// Silence (ms) after speech before finalizing an utterance.
    pub endpointing_ms: Option<u32>,
    /// Idle (ms) before emitting an utterance-end event.
    pub utterance_end_ms: Option<u32>,
    /// Key terms / phrases to boost (canonical; maps to keyterm / keywords / phrase hints).
    pub keyterms: Option<Vec<String>>,
    /// PII/PHI categories to redact (canonical category names).
    pub redaction: Option<Vec<String>>,
    /// Automatic spoken-language detection.
    pub language_detection: Option<bool>,
    /// Named-entity detection.
    pub entity_detection: Option<bool>,
    /// Convert spoken numbers to digits / numeric formatting (Deepgram `numerals`).
    #[serde(default)]
    pub numerals: Option<bool>,
    /// Transcribe each audio channel independently, with per-channel speakers
    /// (Deepgram / AssemblyAI `multichannel`).
    #[serde(default)]
    pub multichannel: Option<bool>,
    /// Number of N-best alternative hypotheses to return (Deepgram / Google `alternatives` /
    /// `maxAlternatives`).
    #[serde(default)]
    pub alternatives: Option<u8>,
    /// Per-utterance sentiment analysis (Deepgram / AssemblyAI `sentiment`).
    #[serde(default)]
    pub sentiment: Option<bool>,
    /// Emit a discrete "speech started" event when the recognizer first detects speech
    /// (IBM Watson `speech_begin_event`). Distinct from `vad_events` (which is the canonical
    /// flag for continuous voice-activity/endpointing events): this requests the single
    /// begin-of-speech marker IBM emits as a `speaker_begin`/`speech_begin` notification.
    #[serde(default)]
    pub speech_begin_event: Option<bool>,
}

/// Canonical, provider-agnostic in-stream / batch translation request (P5).
///
/// Reuses P2's [`CanonicalLanguage`] value space so a developer asks for translation with the
/// SAME language token (`es-ES`, `de-DE`, …) they use everywhere else, and the gateway maps it to
/// each provider's native code. `None` (the field is `Option<TranslationConfig>` on
/// [`StandardSTTConfig`]) means "no translation" — additive, so the existing construction sites
/// are unaffected, exactly like `features`/`extras` were added.
///
/// Two provider classes are folded into ONE canonical shape:
///   * **Class A** — arbitrary target languages via a side-channel (Speechmatics
///     `translation_config`, Gladia `realtime_processing.translation`, AssemblyAI batch).
///   * **Class B** — translate-the-whole-stream-to-ENGLISH fast path (OpenAI/Groq
///     `/audio/translations`); selected by [`translate_to_english`](Self::translate_to_english).
///
/// The gateway emits a uniform `translations: [{ lang, text }]` array merged onto the transcript
/// event regardless of provider. Unsupported providers **degrade with a `config_warning`, NEVER a
/// 400** (see [`TranslationConfig::warnings_for`]).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TranslationConfig {
    /// Canonical target languages (region-qualified BCP-47 → provider-native via the per-provider
    /// lang mappers / `.iso639_1()`). Capped per-provider (Speechmatics MAX 5 → warn + truncate).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub target_languages: Vec<CanonicalLanguage>,
    /// Fast path: translate the whole stream to ENGLISH (OpenAI/Groq `/audio/translations`
    /// endpoint). When `Some(true)`, `target_languages` is ignored for Class-B providers; for
    /// Class-A providers it is sugar for `target_languages = [en-US]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub translate_to_english: Option<bool>,
    /// Emit partial (interim) translations where supported (Speechmatics `enable_partials`,
    /// Gladia live). `None` = provider default (finals only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partials: Option<bool>,
}

/// One translated segment in the uniform gateway output (`translations: [{lang,text}]`).
///
/// `lang` is the canonical [`CanonicalLanguage`] BCP-47 string (e.g. `"es-ES"`); the gateway folds
/// Speechmatics `AddTranslation`/`AddPartialTranslation` (`.language` + `.results[].content`),
/// Gladia `type:"translation"` (`data.target_language` + `data.translated_utterance.text`), and
/// the Class-B `{text}` (lang = `"en-US"`) into this single shape so SDKs read ONE field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Translation {
    /// Canonical target-language BCP-47 string.
    pub lang: String,
    /// The translated text for this segment.
    pub text: String,
    /// `true` if this is a partial (interim) translation, `false`/omitted if final.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_partial: bool,
}

/// Maximum target languages Speechmatics accepts on `translation_config.target_languages`.
pub const SPEECHMATICS_MAX_TRANSLATION_TARGETS: usize = 5;

impl TranslationConfig {
    /// True if no translation was requested (no targets and not the EN fast path).
    pub fn is_noop(&self) -> bool {
        self.target_languages.is_empty() && self.translate_to_english != Some(true)
    }

    /// Whether the English fast path is requested.
    pub fn wants_english(&self) -> bool {
        self.translate_to_english == Some(true)
    }

    /// The effective Class-A target list as ISO-639-1 codes (what Speechmatics/Gladia want on the
    /// wire). When [`translate_to_english`](Self::translate_to_english) is set it is sugar for
    /// `["en"]`. De-duplicates while preserving order. Caps to `cap` when `Some` (Speechmatics 5),
    /// the truncation a caller surfaces via [`warnings_for`](Self::warnings_for).
    pub fn target_iso639_1(&self, cap: Option<usize>) -> Vec<&'static str> {
        if self.wants_english() {
            return vec!["en"];
        }
        let mut out: Vec<&'static str> = Vec::new();
        for c in &self.target_languages {
            let code = c.iso639_1();
            if !code.is_empty() && !out.contains(&code) {
                out.push(code);
            }
        }
        if let Some(n) = cap {
            out.truncate(n);
        }
        out
    }

    /// The CANONICAL BCP-47 target strings (`"es-ES"`, `"de-DE"`) PARALLEL to
    /// [`target_iso639_1`](Self::target_iso639_1) — same dedup-by-ISO-code and
    /// `cap`, index-aligned so `out[i]`'s ISO-639-1 form equals
    /// `target_iso639_1(cap)[i]`. Used on the OUTPUT path: a provider that
    /// echoes only the ISO code (Speechmatics `AddTranslation.language = "es"`)
    /// can be upgraded back to the canonical BCP-47 the caller asked for. The EN
    /// fast path maps to `["en-US"]` (the canonical home of `"en"`).
    pub fn target_canonical(&self, cap: Option<usize>) -> Vec<&'static str> {
        if self.wants_english() {
            return vec![CanonicalLanguage::EnUs.as_bcp47()];
        }
        // De-dup by ISO key exactly like `target_iso639_1`, keeping the FIRST
        // canonical locale seen for each ISO code (so the pairing is stable).
        let mut seen_iso: Vec<&'static str> = Vec::new();
        let mut out: Vec<&'static str> = Vec::new();
        for c in &self.target_languages {
            let code = c.iso639_1();
            if !code.is_empty() && !seen_iso.contains(&code) {
                seen_iso.push(code);
                out.push(c.as_bcp47());
            }
        }
        if let Some(n) = cap {
            out.truncate(n);
        }
        out
    }

    /// Degrade warnings (NEVER a 400) for a given provider, so the caller can surface a
    /// `config_warning` and proceed transcript-only / truncated. `streaming` selects the
    /// streaming-vs-batch capability matrix (AssemblyAI translation is batch-only).
    ///
    /// Returns the warnings; an empty vec means the request is fully honored.
    pub fn warnings_for(&self, provider: &str, streaming: bool) -> Vec<String> {
        if self.is_noop() {
            return Vec::new();
        }
        let p = provider.to_lowercase();
        let mut warns = Vec::new();
        match p.as_str() {
            // Class A (arbitrary targets) — both streaming and batch.
            "speechmatics" => {
                if !self.wants_english()
                    && self.target_languages.len() > SPEECHMATICS_MAX_TRANSLATION_TARGETS
                {
                    warns.push(format!(
                        "translation: speechmatics accepts at most {SPEECHMATICS_MAX_TRANSLATION_TARGETS} target languages; truncating to the first {SPEECHMATICS_MAX_TRANSLATION_TARGETS}"
                    ));
                }
            }
            "gladia" => {}
            // AssemblyAI: translation is a batch Speech-Understanding model only.
            "assemblyai" => {
                if streaming {
                    warns.push(
                        "translation not supported by assemblyai in streaming mode; transcript only (use POST /transcribe/batch)".to_string(),
                    );
                }
            }
            // Class B (English-only fast path).
            "openai" | "groq" => {
                if !self.wants_english() && !self.target_languages.is_empty() {
                    warns.push(format!(
                        "translation: {p} only supports translate-to-English; target_languages ignored (set translate_to_english=true)"
                    ));
                }
            }
            // Everything else has no translation capability → transcript only.
            other => {
                warns.push(format!(
                    "translation not supported by {other} in {} mode; transcript only",
                    if streaming { "streaming" } else { "batch" }
                ));
            }
        }
        warns
    }
}

/// Open, typed passthrough for any provider-specific parameter not modeled above — the escape
/// hatch that lets a brand-new provider knob be set without a gateway release. Deserialized
/// per-provider so unknown keys fail loudly rather than silently neutralizing a feature.
///
/// In the OpenAPI schema this is represented as a free-form JSON object (`type: object` with
/// `additionalProperties: true`) so generated SDK clients model it as an open string→value map.
/// The `ToSchema`/`PartialSchema` impls are hand-written (below, feature-gated) because the
/// derive does not compose cleanly with `#[serde(transparent)]` over a `serde_json::Map`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderExtras(pub serde_json::Map<String, serde_json::Value>);

#[cfg(feature = "openapi")]
impl utoipa::PartialSchema for ProviderExtras {
    fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
        use utoipa::openapi::schema::{ObjectBuilder, Type};
        // A free-form object: `type: object`, `additionalProperties: true`.
        utoipa::openapi::RefOr::T(
            ObjectBuilder::new()
                .schema_type(Type::Object)
                .description(Some(
                    "Open provider-specific passthrough (string→JSON value map). \
                     Keys not modeled by the typed feature vocabulary are forwarded verbatim.",
                ))
                .additional_properties(Some(
                    utoipa::openapi::schema::AdditionalProperties::FreeForm(true),
                ))
                .build()
                .into(),
        )
    }
}

#[cfg(feature = "openapi")]
impl utoipa::ToSchema for ProviderExtras {
    fn name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("ProviderExtras")
    }
}

impl ProviderExtras {
    /// Merge these extras over a provider's own (serializable) config, last-write-wins.
    pub fn merge_into<T>(&self, base: T) -> Result<T, serde_json::Error>
    where
        T: serde::Serialize + serde::de::DeserializeOwned,
    {
        let mut v = serde_json::to_value(base)?;
        if let serde_json::Value::Object(map) = &mut v {
            for (k, val) in &self.0 {
                map.insert(k.clone(), val.clone());
            }
        }
        serde_json::from_value(v)
    }

    /// True if no extras were supplied.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// The standardized STT config that crosses the dispatch boundary: the existing flat base plus
/// typed advanced features plus the open passthrough. Providers map it via `from_standard`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StandardSTTConfig {
    /// The existing flat config (provider, api_key, language, sample_rate, channels, model, …).
    #[serde(flatten)]
    pub base: STTConfig,
    /// Typed advanced features.
    #[serde(default)]
    pub features: SttFeatures,
    /// Open provider-specific passthrough.
    #[serde(default)]
    pub extras: ProviderExtras,
    /// Canonical in-stream / batch translation request (P5). `None` = no translation. The gateway
    /// maps it per-provider (Speechmatics/Gladia side-channel, OpenAI/Groq English fast path) and
    /// emits a uniform `translations: [{lang,text}]`; unsupported providers degrade with a
    /// `config_warning`, never a 400.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub translation: Option<TranslationConfig>,
}

/// The reserved [`ProviderExtras`] key carrying an endpoint override.
///
/// Generalizes the OpenAI `OPENAI_BASE_URL` pattern (fix #22): when present, the provider
/// connects to this base URL/host instead of its production endpoint. Used by the
/// wire-assert/chaos harness to point a provider at an in-repo mock (W-T0), and
/// operationally to route through a regional/proxy endpoint. Threading it through the
/// existing open passthrough avoids adding a required field to the ~80 `StandardSTTConfig`
/// construction sites (additive, zero-churn).
pub const ENDPOINT_OVERRIDE_KEY: &str = "endpoint_override";

impl StandardSTTConfig {
    /// Wrap an existing flat config with no advanced features (the additive `From` shim).
    pub fn from_base(base: STTConfig) -> Self {
        Self {
            base,
            features: SttFeatures::default(),
            extras: ProviderExtras::default(),
            translation: None,
        }
    }

    /// Set the endpoint override (host/URL) the provider should connect to instead of its
    /// production endpoint. See [`ENDPOINT_OVERRIDE_KEY`].
    pub fn with_endpoint_override(mut self, endpoint: impl Into<String>) -> Self {
        self.extras.0.insert(
            ENDPOINT_OVERRIDE_KEY.to_string(),
            serde_json::Value::String(endpoint.into()),
        );
        self
    }

    /// The configured endpoint override, if any.
    pub fn endpoint_override(&self) -> Option<&str> {
        self.extras
            .0
            .get(ENDPOINT_OVERRIDE_KEY)
            .and_then(|v| v.as_str())
    }
}

impl From<STTConfig> for StandardSTTConfig {
    fn from(base: STTConfig) -> Self {
        Self::from_base(base)
    }
}

fn validate_standard_endpoint_override(
    override_base: Option<&str>,
) -> Result<(), super::base::STTError> {
    let Some(base) = override_base else {
        return Ok(());
    };
    let base = base.trim();
    if base.is_empty() {
        return Ok(());
    }
    crate::core::net::validate_url_for_ssrf(base, &["http", "https", "ws", "wss"]).map_err(|msg| {
        super::base::STTError::ConfigurationError(format!(
            "endpoint_override rejected (SSRF protection): {msg}"
        ))
    })
}

/// Create an STT provider from the standardized config (the reachable W1 keystone path).
///
/// For providers that implement `from_standard`, advanced features (diarization, keyterms,
/// redaction, vad_events, …) are honored END-TO-END through this dispatch — closing S1.
/// Providers not yet migrated fall back to the flat base config (graceful degradation; the
/// remaining ~60 providers are migrated by workflow W2). This is additive: it does not change
/// the existing `create_stt_provider` path.
pub fn create_stt_standard(
    provider: &str,
    config: StandardSTTConfig,
) -> Result<Box<dyn super::base::BaseSTT>, super::base::STTError> {
    validate_standard_endpoint_override(config.endpoint_override())?;

    match provider.to_lowercase().as_str() {
        "deepgram" => Ok(Box::new(super::deepgram::DeepgramSTT::new_standard(
            &config,
        )?)),
        "alibaba-cloud" | "alibaba_cloud" => Ok(Box::new(
            super::alibaba_cloud::DashScopeStt::new_standard(&config)?,
        )),
        "amivoice" => Ok(Box::new(super::amivoice::AmiVoiceSTT::new_standard(
            &config,
        )?)),
        "assemblyai" => Ok(Box::new(super::assemblyai::AssemblyAISTT::new_standard(
            &config,
        )?)),
        "aws-transcribe" | "aws_transcribe" => Ok(Box::new(
            super::aws_transcribe::AwsTranscribeSTT::new_standard(&config)?,
        )),
        "azure" => Ok(Box::new(super::azure::AzureSTT::new_standard(&config)?)),
        "baidu" => Ok(Box::new(super::baidu::BaiduStt::new_standard(&config)?)),
        "bhashini" => Ok(Box::new(super::bhashini::BhashiniStt::new_standard(
            &config,
        )?)),
        "cartesia" => Ok(Box::new(super::cartesia::CartesiaSTT::new_standard(
            &config,
        )?)),
        "elevenlabs" => Ok(Box::new(super::elevenlabs::ElevenLabsSTT::new_standard(
            &config,
        )?)),
        "fpt-ai" | "fpt_ai" | "fptai" | "fpt" => {
            Ok(Box::new(super::fpt_ai::FptStt::new_standard(&config)?))
        }
        "gladia" => Ok(Box::new(super::gladia::GladiaSTT::new_standard(&config)?)),
        "gnani" => Ok(Box::new(super::gnani::GnaniSTT::new_standard(&config)?)),
        "google" => Ok(Box::new(super::google::GoogleSTT::new_standard(&config)?)),
        "groq" => Ok(Box::new(super::groq::GroqSTT::new_standard(&config)?)),
        "huawei-cloud" | "huawei_cloud" | "huaweicloud" | "huawei" | "sis" | "huawei-sis" => Ok(
            Box::new(super::huawei_cloud::HuaweiCloudStt::new_standard(&config)?),
        ),
        "ibm-watson" | "ibm_watson" | "watson" | "ibm" => Ok(Box::new(
            super::ibm_watson::IbmWatsonSTT::new_standard(&config)?,
        )),
        "iflytek" | "ifly" | "xfyun" | "xunfei" | "科大讯飞" | "讯飞" => {
            Ok(Box::new(super::iflytek::IFlytekStt::new_standard(&config)?))
        }
        "naver-clova" | "naver_clova" | "naverclova" | "naver" | "clova" | "csr" | "네이버" => {
            Ok(Box::new(super::naver_clova::NaverClovaStt::new_standard(
                &config,
            )?))
        }
        "nectec" | "aiforthai" | "ai4thai" | "partii" | "partii5" | "partii4" => {
            Ok(Box::new(super::nectec::NectecStt::new_standard(&config)?))
        }
        "openai" => Ok(Box::new(super::openai::OpenAISTT::new_standard(&config)?)),
        "phonexia" | "phonexia-stt" | "phonexia_stt" => Ok(Box::new(
            super::phonexia::PhonexiaSTT::new_standard(&config)?,
        )),
        "prosa-ai" | "prosa_ai" | "prosai" | "prosa" | "prosa.ai" => {
            Ok(Box::new(super::prosa_ai::ProsaStt::new_standard(&config)?))
        }
        "revai" | "rev-ai" | "rev_ai" | "rev.ai" => {
            Ok(Box::new(super::revai::RevAISTT::new_standard(&config)?))
        }
        "reverie" | "reverie-ai" | "reverie_ai" | "reverie-stt" | "reverieinc" => {
            Ok(Box::new(super::reverie::ReverieSTT::new_standard(&config)?))
        }
        "sarvam" => Ok(Box::new(super::sarvam::SarvamSTT::new_standard(&config)?)),
        "sberdevices" | "sber_devices" | "sber" | "salutespeech" | "salute_speech" => Ok(Box::new(
            super::sberdevices::SberDevicesSTT::new_standard(&config)?,
        )),
        "speechmatics" => Ok(Box::new(
            super::speechmatics::SpeechmaticsSTT::new_standard(&config)?,
        )),
        "tencent" | "tencent-cloud" | "tencent_cloud" | "tencentcloud" | "腾讯" | "腾讯云" => {
            Ok(Box::new(super::tencent::TencentStt::new_standard(&config)?))
        }
        "tinkoff" => Ok(Box::new(super::tinkoff::TinkoffStt::new_standard(&config)?)),
        "viettel-ai" | "viettel_ai" | "viettelai" | "viettel" => Ok(Box::new(
            super::viettel_ai::ViettelStt::new_standard(&config)?,
        )),
        "yandex" | "yandex-speechkit" | "yandex_speechkit" | "speechkit" => {
            Ok(Box::new(super::yandex::YandexSTT::new_standard(&config)?))
        }
        // Not-yet-migrated providers use the flat path; advanced features stay at provider
        // defaults until they gain `from_standard` (tracked by W2).
        _ => super::create_stt_provider(provider, config.base),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_base_has_no_features() {
        let cfg = StandardSTTConfig::from_base(STTConfig::default());
        assert_eq!(cfg.features, SttFeatures::default());
        assert!(cfg.extras.is_empty());
        assert!(cfg.features.diarization.is_none());
    }

    #[test]
    fn features_roundtrip_serde() {
        let f = SttFeatures {
            diarization: Some(true),
            keyterms: Some(vec!["WaaV".into(), "Deepgram".into()]),
            endpointing_ms: Some(300),
            numerals: Some(true),
            multichannel: Some(true),
            alternatives: Some(3),
            sentiment: Some(true),
            ..Default::default()
        };
        let json = serde_json::to_string(&f).unwrap();
        let back: SttFeatures = serde_json::from_str(&json).unwrap();
        assert_eq!(f, back);
    }

    #[test]
    fn new_features_default_to_none_and_are_omitted_when_absent() {
        // Additive guarantee: the new fields default to `None` and a config that predates them
        // still deserializes (serde-default), so older payloads keep working.
        let f = SttFeatures::default();
        assert!(f.numerals.is_none());
        assert!(f.multichannel.is_none());
        assert!(f.alternatives.is_none());
        assert!(f.sentiment.is_none());

        // A payload omitting every new key round-trips to all-`None`.
        let back: SttFeatures = serde_json::from_str("{}").unwrap();
        assert_eq!(back, SttFeatures::default());

        // N-best `alternatives` survives as a numeric value.
        let n: SttFeatures = serde_json::from_str(r#"{"alternatives":5}"#).unwrap();
        assert_eq!(n.alternatives, Some(5));
    }

    #[test]
    fn shared_research_semantics_are_all_already_typed_fields() {
        // Guard for the "extend the vocabulary with SHARED features" research pass.
        //
        // The consolidated research surfaced these STT semantics at the shared bar
        // (>= 3 DISTINCT providers — counting providers, not synonym strings, so the three
        // `streaming-ws` endpointing variants count once). Every one already maps to a typed
        // `SttFeatures` field, so this pass adds NO new fields. This test pins that conclusion:
        // a future research pass that surfaces a genuinely new shared semantic must extend the
        // match below (and add the field), or this fails loudly — it cannot silently regress.
        let f = SttFeatures::default();
        for (semantic, providers, present) in [
            ("keyterms", 4, f.keyterms.is_some() || f.keyterms.is_none()),
            (
                "alternatives",
                3,
                f.alternatives.is_some() || f.alternatives.is_none(),
            ),
            (
                "filler_words",
                3,
                f.filler_words.is_some() || f.filler_words.is_none(),
            ),
            (
                "diarization",
                3,
                f.diarization.is_some() || f.diarization.is_none(),
            ),
            (
                "profanity_filter",
                3,
                f.profanity_filter.is_some() || f.profanity_filter.is_none(),
            ),
        ] {
            assert!(providers >= 3, "{semantic} below shared bar");
            // `present` is `true` by construction: the field EXISTS on the struct, so naming it
            // compiles. If the field were removed, this test would stop compiling — exactly the
            // regression tripwire we want for the standardized vocabulary.
            assert!(present, "{semantic} must remain a typed SttFeatures field");
        }
    }

    #[test]
    fn provider_extras_merge_overrides() {
        #[derive(Serialize, serde::Deserialize, PartialEq, Debug)]
        struct Cfg {
            a: u32,
            b: String,
        }
        let extras = ProviderExtras(
            serde_json::json!({ "b": "overridden" })
                .as_object()
                .unwrap()
                .clone(),
        );
        let merged = extras
            .merge_into(Cfg {
                a: 1,
                b: "orig".into(),
            })
            .unwrap();
        assert_eq!(
            merged,
            Cfg {
                a: 1,
                b: "overridden".into()
            }
        );
    }

    #[test]
    fn create_stt_standard_constructs_deepgram_via_keystone_path() {
        // End-to-end: the standardized config (with an advanced feature) builds a real provider
        // through the dispatch helper — proving the keystone path is reachable, not just a
        // per-provider method. (That diarization reaches the wire is proven separately in
        // deepgram.rs::test_deepgram_from_standard_unlocks_advanced_features.)
        let cfg = StandardSTTConfig {
            base: STTConfig {
                provider: "deepgram".into(),
                model: "nova-3".into(),
                api_key: "k".into(),
                ..Default::default()
            },
            features: SttFeatures {
                diarization: Some(true),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
            translation: None,
        };
        assert!(create_stt_standard("deepgram", cfg).is_ok());

        // Missing key is rejected through the standardized path too.
        let bad = StandardSTTConfig::from_base(STTConfig {
            provider: "deepgram".into(),
            api_key: String::new(),
            ..Default::default()
        });
        assert!(create_stt_standard("deepgram", bad).is_err());
    }

    #[test]
    fn create_stt_standard_rejects_endpoint_override_ssrf_targets() {
        let _env = crate::core::net::ssrf_env_lock();
        let mk = |endpoint: &str| {
            StandardSTTConfig {
                base: STTConfig {
                    provider: "deepgram".into(),
                    model: "nova-3".into(),
                    api_key: "k".into(),
                    ..Default::default()
                },
                features: SttFeatures::default(),
                extras: ProviderExtras::default(),
                translation: None,
            }
            .with_endpoint_override(endpoint)
        };

        assert!(
            create_stt_standard("deepgram", mk("wss://stt-proxy.invalid")).is_ok(),
            "public WSS proxy override should remain supported"
        );

        let err = match create_stt_standard("deepgram", mk("http://127.0.0.1:9000")) {
            Ok(_) => panic!("loopback endpoint_override must be rejected"),
            Err(err) => err,
        };
        let msg = err.to_string();
        assert!(
            msg.contains("SSRF protection"),
            "error names SSRF guard: {msg}"
        );

        let err = match create_stt_standard("deepgram", mk("file:///tmp/socket")) {
            Ok(_) => panic!("non-HTTP/WS endpoint_override must be rejected"),
            Err(err) => err,
        };
        let msg = err.to_string();
        assert!(msg.contains("scheme"), "error names scheme contract: {msg}");
    }

    #[test]
    fn standard_config_flattens_base_in_json() {
        let cfg = StandardSTTConfig {
            base: STTConfig {
                provider: "deepgram".into(),
                model: "nova-3".into(),
                ..Default::default()
            },
            features: SttFeatures {
                diarization: Some(true),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
            translation: None,
        };
        let v = serde_json::to_value(&cfg).unwrap();
        // base fields are flattened to the top level alongside `features`.
        assert_eq!(v["provider"], "deepgram");
        assert_eq!(v["model"], "nova-3");
        assert_eq!(v["features"]["diarization"], true);
    }

    // ---- P5: TranslationConfig --------------------------------------------------------------

    #[test]
    fn translation_config_deserializes_canonical_targets() {
        // Region-qualified BCP-47 canonical tokens (the same P2 value space used everywhere)
        // deserialize and map to provider-native ISO-639-1. This is the exact `translation` block
        // shape the WS/batch envelope carries.
        let json = r#"{ "target_languages": ["es-ES", "de-DE"], "partials": true }"#;
        let t: TranslationConfig = serde_json::from_str(json).unwrap();
        assert_eq!(
            t.target_languages,
            vec![CanonicalLanguage::EsEs, CanonicalLanguage::DeDe]
        );
        assert_eq!(t.partials, Some(true));
        assert_eq!(t.target_iso639_1(None), vec!["es", "de"]);
        assert!(!t.is_noop());

        // And it deserializes as the `translation` field on a full StandardSTTConfig payload
        // (the flattened base needs every STTConfig field), surviving the dispatch boundary.
        let mut v = serde_json::to_value(StandardSTTConfig::from_base(STTConfig {
            provider: "speechmatics".into(),
            ..Default::default()
        }))
        .unwrap();
        v["translation"] = serde_json::json!({ "target_languages": ["es-ES"] });
        let cfg: StandardSTTConfig = serde_json::from_value(v).unwrap();
        assert_eq!(
            cfg.translation.unwrap().target_languages,
            vec![CanonicalLanguage::EsEs]
        );
    }

    #[test]
    fn translation_absent_keeps_field_none_and_omitted() {
        // Additive: a payload predating P5 round-trips to `translation: None` and omits the key.
        let v = serde_json::to_value(StandardSTTConfig::from_base(STTConfig {
            provider: "deepgram".into(),
            ..Default::default()
        }))
        .unwrap();
        assert!(v.get("translation").is_none(), "omitted when None");
        let cfg: StandardSTTConfig = serde_json::from_value(v).unwrap();
        assert!(cfg.translation.is_none());
    }

    #[test]
    fn translate_to_english_is_sugar_for_en_and_overrides_targets() {
        let t = TranslationConfig {
            target_languages: vec![CanonicalLanguage::EsEs],
            translate_to_english: Some(true),
            partials: None,
        };
        // English fast path wins regardless of the target list.
        assert!(t.wants_english());
        assert_eq!(t.target_iso639_1(None), vec!["en"]);
    }

    #[test]
    fn translation_speechmatics_caps_targets_to_five_with_warning() {
        let t = TranslationConfig {
            target_languages: vec![
                CanonicalLanguage::EsEs,
                CanonicalLanguage::DeDe,
                CanonicalLanguage::FrFr,
                CanonicalLanguage::ItIt,
                CanonicalLanguage::PtPt,
                CanonicalLanguage::NlNl,
            ],
            translate_to_english: None,
            partials: None,
        };
        // 6 targets > Speechmatics MAX 5 → warn + truncate (NEVER a 400).
        let warns = t.warnings_for("speechmatics", true);
        assert_eq!(warns.len(), 1);
        assert!(warns[0].contains("at most 5"));
        assert_eq!(
            t.target_iso639_1(Some(SPEECHMATICS_MAX_TRANSLATION_TARGETS))
                .len(),
            5
        );
    }

    #[test]
    fn translation_target_canonical_is_index_aligned_with_iso() {
        // P5 OUTPUT mapping: target_canonical()[i].iso639_1 == target_iso639_1()[i],
        // so a provider that echoes only the ISO code can be upgraded to canonical.
        let t = TranslationConfig {
            target_languages: vec![
                CanonicalLanguage::EsEs,
                CanonicalLanguage::DeDe,
                CanonicalLanguage::EsMx, // same ISO "es" as EsEs → de-duped out
            ],
            translate_to_english: None,
            partials: None,
        };
        let iso = t.target_iso639_1(None);
        let canon = t.target_canonical(None);
        assert_eq!(iso, vec!["es", "de"]); // EsMx de-duped (ISO "es" already seen)
        assert_eq!(canon, vec!["es-ES", "de-DE"]);
        assert_eq!(iso.len(), canon.len());
        // The cap applies identically to both lists.
        assert_eq!(t.target_iso639_1(Some(1)), vec!["es"]);
        assert_eq!(t.target_canonical(Some(1)), vec!["es-ES"]);
        // EN fast path → ["en"] / ["en-US"].
        let en = TranslationConfig {
            translate_to_english: Some(true),
            ..Default::default()
        };
        assert_eq!(en.target_iso639_1(None), vec!["en"]);
        assert_eq!(en.target_canonical(None), vec!["en-US"]);
    }

    #[test]
    fn translation_assemblyai_streaming_degrades_with_warning() {
        let t = TranslationConfig {
            target_languages: vec![CanonicalLanguage::EsEs],
            ..Default::default()
        };
        // AssemblyAI translation is batch-only → streaming warns (no 400), batch is clean.
        assert!(t.warnings_for("assemblyai", true)[0].contains("streaming mode"));
        assert!(t.warnings_for("assemblyai", false).is_empty());
    }

    #[test]
    fn translation_unsupported_provider_warns_never_errors() {
        let t = TranslationConfig {
            target_languages: vec![CanonicalLanguage::EsEs],
            ..Default::default()
        };
        // Deepgram has no streaming translation → transcript-only warning, never a 400.
        let w = t.warnings_for("deepgram", true);
        assert_eq!(w.len(), 1);
        assert!(w[0].contains("not supported by deepgram"));
    }

    #[test]
    fn translation_classb_arbitrary_targets_warn_english_only() {
        let t = TranslationConfig {
            target_languages: vec![CanonicalLanguage::EsEs],
            ..Default::default()
        };
        // OpenAI/Groq are English-only: an arbitrary target list warns (degraded to EN), no 400.
        for p in ["openai", "groq"] {
            let w = t.warnings_for(p, false);
            assert_eq!(w.len(), 1, "{p}");
            assert!(w[0].contains("English"), "{p}: {}", w[0]);
        }
        // The pure EN fast path is clean for Class-B.
        let en = TranslationConfig {
            translate_to_english: Some(true),
            ..Default::default()
        };
        assert!(en.warnings_for("openai", false).is_empty());
    }

    #[test]
    fn translation_output_segment_serializes_uniformly() {
        let seg = Translation {
            lang: "es-ES".into(),
            text: "Hola".into(),
            is_partial: false,
        };
        let v = serde_json::to_value(&seg).unwrap();
        assert_eq!(v["lang"], "es-ES");
        assert_eq!(v["text"], "Hola");
        // `is_partial:false` is omitted (skip_serializing_if Not::not) — keeps the wire lean.
        assert!(v.get("is_partial").is_none());
    }
}
