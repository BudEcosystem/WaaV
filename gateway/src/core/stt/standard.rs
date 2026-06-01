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
}

/// Open, typed passthrough for any provider-specific parameter not modeled above — the escape
/// hatch that lets a brand-new provider knob be set without a gateway release. Deserialized
/// per-provider so unknown keys fail loudly rather than silently neutralizing a feature.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderExtras(pub serde_json::Map<String, serde_json::Value>);

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

impl StandardSTTConfig {
    /// Wrap an existing flat config with no advanced features (the additive `From` shim).
    pub fn from_base(base: STTConfig) -> Self {
        Self {
            base,
            features: SttFeatures::default(),
            extras: ProviderExtras::default(),
        }
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
            ..Default::default()
        };
        let json = serde_json::to_string(&f).unwrap();
        let back: SttFeatures = serde_json::from_str(&json).unwrap();
        assert_eq!(f, back);
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
