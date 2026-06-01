//! Standardized TTS configuration (W1 keystone for TTS — mirrors `stt/standard.rs`).
//!
//! The flat `TTSConfig` is the only struct crossing the dispatch/factory boundary, so advanced
//! TTS features (voice settings, emotion, instructions, SSML, …) are unreachable on the live
//! path (BRUTAL_REVIEW.md S1/S5). This additive layer wraps the flat config with typed
//! [`TtsFeatures`] + the open [`ProviderExtras`] passthrough. Each provider gains a
//! `from_standard` mapping (added across the fleet by the migration workflow).

use super::base::TTSConfig;
pub use crate::core::stt::standard::ProviderExtras;
use serde::{Deserialize, Serialize};

/// Canonical, provider-agnostic advanced TTS features. Every field is `Option`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TtsFeatures {
    /// Speaking speed multiplier / level (provider-specific range).
    pub speed: Option<f32>,
    /// Pitch adjustment.
    pub pitch: Option<f32>,
    /// Volume / loudness adjustment.
    pub volume: Option<f32>,
    /// ElevenLabs-style voice stability.
    pub stability: Option<f32>,
    /// ElevenLabs-style similarity boost.
    pub similarity_boost: Option<f32>,
    /// ElevenLabs-style style exaggeration.
    pub style: Option<f32>,
    /// ElevenLabs-style speaker boost.
    pub use_speaker_boost: Option<bool>,
    /// Emotion / delivery (e.g. "happy", "cheerful"); maps to provider emotion controls.
    pub emotion: Option<String>,
    /// Free-form delivery instructions (OpenAI gpt-4o-mini-tts `instructions`, Hume `description`).
    pub instructions: Option<String>,
    /// Treat the input text as SSML (unlocks Azure `mstts:express-as`, etc.).
    pub ssml: Option<bool>,
    /// Synthesis language override.
    pub language: Option<String>,
    /// Request word-level timestamps in the response.
    pub word_timestamps: Option<bool>,
    /// Prefer the provider's streaming endpoint (lower TTFB).
    pub streaming: Option<bool>,
    /// Determinism seed where the provider supports it.
    pub seed: Option<u64>,
    /// Output sample rate override.
    pub sample_rate: Option<u32>,
}

/// The standardized TTS config crossing the dispatch boundary: flat base + typed features +
/// open passthrough. Providers map it via `from_standard`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StandardTTSConfig {
    #[serde(flatten)]
    pub base: TTSConfig,
    #[serde(default)]
    pub features: TtsFeatures,
    #[serde(default)]
    pub extras: ProviderExtras,
}

impl StandardTTSConfig {
    /// Wrap an existing flat config with no advanced features (the additive shim).
    pub fn from_base(base: TTSConfig) -> Self {
        Self {
            base,
            features: TtsFeatures::default(),
            extras: ProviderExtras::default(),
        }
    }
}

impl From<TTSConfig> for StandardTTSConfig {
    fn from(base: TTSConfig) -> Self {
        Self::from_base(base)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_base_has_no_features() {
        let cfg = StandardTTSConfig::from_base(TTSConfig::default());
        assert_eq!(cfg.features, TtsFeatures::default());
        assert!(cfg.extras.is_empty());
    }

    #[test]
    fn tts_features_roundtrip_serde() {
        let f = TtsFeatures {
            stability: Some(0.7),
            instructions: Some("speak cheerfully".into()),
            ssml: Some(true),
            ..Default::default()
        };
        let json = serde_json::to_string(&f).unwrap();
        let back: TtsFeatures = serde_json::from_str(&json).unwrap();
        assert_eq!(f, back);
    }

    #[test]
    fn standard_tts_config_flattens_base_in_json() {
        let cfg = StandardTTSConfig {
            base: TTSConfig {
                provider: "elevenlabs".into(),
                ..Default::default()
            },
            features: TtsFeatures {
                stability: Some(0.5),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
        };
        let v = serde_json::to_value(&cfg).unwrap();
        assert_eq!(v["provider"], "elevenlabs");
        assert_eq!(v["features"]["stability"], 0.5);
    }
}
