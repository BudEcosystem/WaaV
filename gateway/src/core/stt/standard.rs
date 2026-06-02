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
                .additional_properties(Some(utoipa::openapi::schema::AdditionalProperties::FreeForm(
                    true,
                )))
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
    match provider.to_lowercase().as_str() {
        "deepgram" => Ok(Box::new(super::deepgram::DeepgramSTT::new_standard(&config)?)),
        "alibaba-cloud" | "alibaba_cloud" => Ok(Box::new(
            super::alibaba_cloud::DashScopeStt::new_standard(&config)?,
        )),
        "amivoice" => Ok(Box::new(super::amivoice::AmiVoiceSTT::new_standard(&config)?)),
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
        "naver-clova" | "naver_clova" | "naverclova" | "naver" | "clova" | "csr" | "네이버" => Ok(
            Box::new(super::naver_clova::NaverClovaStt::new_standard(&config)?),
        ),
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
        "reverie" | "reverie-ai" | "reverie_ai" | "reverie-stt" | "reverieinc" => Ok(Box::new(
            super::reverie::ReverieSTT::new_standard(&config)?,
        )),
        "sarvam" => Ok(Box::new(super::sarvam::SarvamSTT::new_standard(&config)?)),
        "sberdevices" | "sber_devices" | "sber" | "salutespeech" | "salute_speech" => Ok(Box::new(
            super::sberdevices::SberDevicesSTT::new_standard(&config)?,
        )),
        "speechmatics" => Ok(Box::new(super::speechmatics::SpeechmaticsSTT::new_standard(
            &config,
        )?)),
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
        };
        let v = serde_json::to_value(&cfg).unwrap();
        // base fields are flattened to the top level alongside `features`.
        assert_eq!(v["provider"], "deepgram");
        assert_eq!(v["model"], "nova-3");
        assert_eq!(v["features"]["diarization"], true);
    }
}
