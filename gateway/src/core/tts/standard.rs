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
    /// Streaming latency-optimization tier 0-4, trading quality for lower TTFB
    /// (ElevenLabs `optimize_streaming_latency`).
    #[serde(default)]
    pub optimize_streaming_latency: Option<u8>,
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

/// Create a TTS provider from the standardized config — the reachable W1 keystone path, mirroring
/// [`crate::core::stt::standard::create_stt_standard`].
///
/// For providers that implement `from_standard`, advanced features (voice settings, emotion,
/// instructions, SSML, …) are honored END-TO-END through this dispatch — closing S1/S5 for TTS.
/// Providers not yet migrated fall back to the flat base config (graceful degradation; the
/// remaining providers gain a dispatch arm via workflow W2). Additive: it does not change the
/// existing `create_tts_provider` path.
pub fn create_tts_standard(
    provider: &str,
    config: StandardTTSConfig,
) -> super::base::TTSResult<Box<dyn super::base::BaseTTS>> {
    match provider.to_lowercase().as_str() {
        "deepgram" => Ok(Box::new(super::deepgram::DeepgramTTS::from_standard(&config)?)),
        "acapela" => Ok(Box::new(super::acapela::AcapelaTts::from_standard(&config)?)),
        "alibaba_cloud" | "alibaba-cloud" => {
            Ok(Box::new(super::alibaba_cloud::DashScopeTts::from_standard(&config)?))
        }
        "aws_polly" | "aws-polly" | "amazon-polly" | "polly" => {
            Ok(Box::new(super::aws_polly::AwsPollyTTS::from_standard(&config)?))
        }
        "azure" | "microsoft-azure" => {
            Ok(Box::new(super::azure::AzureTTS::from_standard(&config)?))
        }
        "baidu" => Ok(Box::new(super::baidu::BaiduTts::from_standard(&config)?)),
        "bhashini" => Ok(Box::new(super::bhashini::BhashiniTts::from_standard(&config)?)),
        "cartesia" => Ok(Box::new(super::cartesia::CartesiaTTS::from_standard(&config)?)),
        "cereproc" => Ok(Box::new(super::cereproc::CereprocTts::from_standard(&config)?)),
        "elevenlabs" => {
            Ok(Box::new(super::elevenlabs::ElevenLabsTTS::from_standard(&config)?))
        }
        "fpt_ai" | "fpt-ai" => Ok(Box::new(super::fpt_ai::FptTts::from_standard(&config)?)),
        "gnani" => Ok(Box::new(super::gnani::GnaniTTS::from_standard(&config)?)),
        "google" => Ok(Box::new(super::google::GoogleTTS::from_standard(&config)?)),
        "huawei_cloud" | "huawei-cloud" => {
            Ok(Box::new(super::huawei_cloud::HuaweiCloudTts::from_standard(&config)?))
        }
        "hume" => Ok(Box::new(super::hume::HumeTTS::from_standard(&config)?)),
        "ibm_watson" | "ibm-watson" => {
            Ok(Box::new(super::ibm_watson::IbmWatsonTTS::from_standard(&config)?))
        }
        "iflytek" => Ok(Box::new(super::iflytek::IFlytekTts::from_standard(&config)?)),
        "lmnt" | "lmnt-ai" | "lmnt_ai" => {
            Ok(Box::new(super::lmnt::LmntTts::from_standard(&config)?))
        }
        "murf" | "murf-ai" | "murf_ai" | "murf.ai" => {
            Ok(Box::new(super::murf::MurfTts::from_standard(&config)?))
        }
        "naver_clova" | "naver-clova" | "naver" | "clova" => {
            Ok(Box::new(super::naver_clova::NaverClovaTts::from_standard(&config)?))
        }
        "nectec" | "vaja9" | "vaja" => {
            Ok(Box::new(super::nectec::NectecTts::from_standard(&config)?))
        }
        "openai" => Ok(Box::new(super::openai::OpenAITTS::from_standard(&config)?)),
        "playht" | "play_ht" | "play-ht" | "play.ht" => {
            Ok(Box::new(super::playht::PlayHtTts::from_standard(&config)?))
        }
        "prosa_ai" | "prosa-ai" | "prosa" => {
            Ok(Box::new(super::prosa_ai::ProsaTts::from_standard(&config)?))
        }
        "resemble" | "resemble_ai" | "resemble-ai" => {
            Ok(Box::new(super::resemble::ResembleTts::from_standard(&config)?))
        }
        "reverie" => Ok(Box::new(super::reverie::ReverieTts::from_standard(&config)?)),
        "sberdevices" | "sber" | "sber_devices" | "sber-devices" => {
            Ok(Box::new(super::sberdevices::SberDevicesTts::from_standard(&config)?))
        }
        "smallest" | "smallest_ai" | "smallest-ai" => {
            Ok(Box::new(super::smallest::SmallestTts::from_standard(&config)?))
        }
        "speechify" => Ok(Box::new(super::speechify::SpeechifyTts::from_standard(&config)?)),
        "speechmatics" => {
            Ok(Box::new(super::speechmatics::SpeechmaticsTts::from_standard(&config)?))
        }
        "tencent" => Ok(Box::new(super::tencent::TencentTts::from_standard(&config)?)),
        "tinkoff" => Ok(Box::new(super::tinkoff::TinkoffTts::from_standard(&config)?)),
        "unrealspeech" | "unreal_speech" | "unreal-speech" => {
            Ok(Box::new(super::unrealspeech::UnrealSpeechTts::from_standard(&config)?))
        }
        "viettel_ai" | "viettel-ai" | "viettel" => {
            Ok(Box::new(super::viettel_ai::ViettelTts::from_standard(&config)?))
        }
        "wellsaid" | "wellsaid_labs" | "wellsaid-labs" => {
            Ok(Box::new(super::wellsaid::WellSaidTts::from_standard(&config)?))
        }
        "yandex" => Ok(Box::new(super::yandex::YandexTts::from_standard(&config)?)),
        "zalo_ai" | "zalo-ai" | "zalo" => {
            Ok(Box::new(super::zalo_ai::ZaloTts::from_standard(&config)?))
        }
        // Not-yet-migrated providers use the flat path; advanced features stay at provider
        // defaults until they gain a `from_standard` dispatch arm (tracked by W2).
        _ => super::create_tts_provider(provider, config.base),
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
            optimize_streaming_latency: Some(3),
            ..Default::default()
        };
        let json = serde_json::to_string(&f).unwrap();
        let back: TtsFeatures = serde_json::from_str(&json).unwrap();
        assert_eq!(f, back);
    }

    #[test]
    fn optimize_streaming_latency_defaults_to_none_and_is_serde_default() {
        // Additive guarantee: the new field defaults to `None` and a payload that omits it still
        // deserializes (serde-default), so older configs keep working.
        let f = TtsFeatures::default();
        assert!(f.optimize_streaming_latency.is_none());

        let back: TtsFeatures = serde_json::from_str("{}").unwrap();
        assert_eq!(back, TtsFeatures::default());

        // The ElevenLabs latency tier survives as a numeric value.
        let tier: TtsFeatures =
            serde_json::from_str(r#"{"optimize_streaming_latency":4}"#).unwrap();
        assert_eq!(tier.optimize_streaming_latency, Some(4));
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

    #[test]
    fn create_tts_standard_constructs_deepgram_via_keystone_path() {
        // End-to-end parity with the STT side: the standardized TTS config (with an advanced
        // feature) builds a real provider through the dispatch helper — proving the keystone path
        // is reachable for TTS, not just a per-provider method.
        let cfg = StandardTTSConfig {
            base: TTSConfig {
                provider: "deepgram".into(),
                api_key: "k".into(),
                voice_id: Some("aura-asteria-en".into()),
                ..Default::default()
            },
            features: TtsFeatures {
                sample_rate: Some(24000),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
        };
        assert!(create_tts_standard("deepgram", cfg).is_ok());
    }

    // `#[tokio::test]`: the Google arm builds a `TTSGoogleAuthClient`, whose token cache must be
    // constructed inside a Tokio runtime. All constructors here are synchronous; the runtime is
    // only an ambient requirement.
    #[tokio::test]
    async fn create_tts_standard_constructs_migrated_providers_via_keystone_path() {
        // Each newly migrated provider builds through the dispatch helper with an advanced feature
        // set, proving the keystone path reaches the provider struct's `from_standard` (not just a
        // per-provider method).
        let acapela = StandardTTSConfig {
            base: TTSConfig {
                provider: "acapela".into(),
                api_key: "user@example.com:pw".into(),
                voice_id: Some("alice".into()),
                ..Default::default()
            },
            features: TtsFeatures {
                speed: Some(1.5),
                sample_rate: Some(16000),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
        };
        assert!(create_tts_standard("acapela", acapela).is_ok());

        let alibaba = StandardTTSConfig {
            base: TTSConfig {
                provider: "alibaba_cloud".into(),
                api_key: "k".into(),
                voice_id: Some("longxiaochun".into()),
                model: "cosyvoice-v3-flash".into(),
                ..Default::default()
            },
            features: TtsFeatures {
                speed: Some(1.25),
                sample_rate: Some(24000),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
        };
        assert!(create_tts_standard("alibaba_cloud", alibaba).is_ok());

        let polly = StandardTTSConfig {
            base: TTSConfig {
                provider: "aws_polly".into(),
                voice_id: Some("Joanna".into()),
                model: "neural".into(),
                ..Default::default()
            },
            features: TtsFeatures {
                ssml: Some(true),
                language: Some("en-GB".into()),
                // The config maps `output_format` at its PCM default; override the standard's
                // default 24000 rate with a PCM-valid one so validation passes.
                sample_rate: Some(16000),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
        };
        assert!(create_tts_standard("aws_polly", polly).is_ok());

        let azure = StandardTTSConfig {
            base: TTSConfig {
                provider: "azure".into(),
                api_key: "key".into(),
                voice_id: Some("en-US-JennyNeural".into()),
                ..Default::default()
            },
            features: TtsFeatures {
                ssml: Some(true),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
        };
        assert!(create_tts_standard("azure", azure).is_ok());

        let baidu = StandardTTSConfig {
            base: TTSConfig {
                provider: "baidu".into(),
                api_key: "api_key|secret_key".into(),
                voice_id: Some("0".into()),
                ..Default::default()
            },
            features: TtsFeatures {
                speed: Some(1.5),
                pitch: Some(8.0),
                volume: Some(10.0),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
        };
        assert!(create_tts_standard("baidu", baidu).is_ok());

        let bhashini = StandardTTSConfig {
            base: TTSConfig {
                provider: "bhashini".into(),
                api_key: "user|ulca_key".into(),
                voice_id: Some("hi".into()),
                sample_rate: Some(22050),
                ..Default::default()
            },
            features: TtsFeatures {
                sample_rate: Some(16000),
                language: Some("ta".into()),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
        };
        assert!(create_tts_standard("bhashini", bhashini).is_ok());

        let cartesia = StandardTTSConfig {
            base: TTSConfig {
                provider: "cartesia".into(),
                api_key: "k".into(),
                voice_id: Some("a0e99841-438c-4a64-b679-ae501e7d6091".into()),
                ..Default::default()
            },
            features: TtsFeatures {
                speed: Some(1.3),
                emotion: Some("happy".into()),
                sample_rate: Some(16000),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
        };
        assert!(create_tts_standard("cartesia", cartesia).is_ok());

        let cereproc = StandardTTSConfig {
            base: TTSConfig {
                provider: "cereproc".into(),
                api_key: "user@example.com:password123".into(),
                voice_id: Some("Stuart".into()),
                ..Default::default()
            },
            features: TtsFeatures {
                emotion: Some("happy".into()),
                sample_rate: Some(16000),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
        };
        assert!(create_tts_standard("cereproc", cereproc).is_ok());

        let elevenlabs = StandardTTSConfig {
            base: TTSConfig {
                provider: "elevenlabs".into(),
                api_key: "k".into(),
                voice_id: Some("21m00Tcm4TlvDq8ikWAM".into()),
                ..Default::default()
            },
            features: TtsFeatures {
                stability: Some(0.7),
                similarity_boost: Some(0.9),
                style: Some(0.3),
                use_speaker_boost: Some(true),
                speed: Some(1.4),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
        };
        assert!(create_tts_standard("elevenlabs", elevenlabs).is_ok());

        let fpt_ai = StandardTTSConfig {
            base: TTSConfig {
                provider: "fpt-ai".into(),
                api_key: "k".into(),
                voice_id: Some("banmai".into()),
                ..Default::default()
            },
            features: TtsFeatures {
                speed: Some(1.5),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
        };
        assert!(create_tts_standard("fpt-ai", fpt_ai).is_ok());

        let gnani = StandardTTSConfig {
            base: TTSConfig {
                provider: "gnani".into(),
                voice_id: Some("Hi-IN".into()),
                ..Default::default()
            },
            features: TtsFeatures {
                language: Some("Ta-IN".into()),
                sample_rate: Some(16000),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
        };
        assert!(create_tts_standard("gnani", gnani).is_ok());

        // A self-contained, valid service-account credential (test-only RSA key) so the
        // credential-deriving Google constructor builds its auth client without any network call.
        const GOOGLE_TEST_SERVICE_ACCOUNT_JSON: &str = r#"{
  "type": "service_account",
  "project_id": "creds-project-123",
  "private_key_id": "test-key-id",
  "client_email": "test@creds-project-123.iam.gserviceaccount.com",
  "client_id": "1234567890",
  "private_key": "-----BEGIN PRIVATE KEY-----\nMIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC0dEowKy753Kjw\nP8a593MGNp91QpbTYl6VGND6bRXvkXr1h4Bb37e0OWofS/skiWI6omPU34bEuTgC\n83kfKILtWkb5URVs58QX7TO9w84eA+ePxHGoWL7Myohwrh5fWruk4f4t3rXqjlAK\ne5+yslho0ufEMB4thPDAMK7yQt3grCK6Ybe+en96aiaz4yR4xxKkI5l5KEVacF/i\n4wDszefD8n7+f9K+VhIZRTbuowRlwwn3/79BNmLNcSt5MgTQeA/Ayhl6uyrXNkTm\npj5rRum/13G0QAzPJJiYmYm/zNBR94Mvk4JM9T9kOZjEH4s1q7E/vfRoX6+qeVud\nduosFrz7AgMBAAECggEAIuXCWyJeyU9VFHEg+2HRSshRehnQlTyW0fqkn2ltLpFR\n2B3GQv42xpG75iWJgf1Xk8NHzykTJQQ0ws1XBSGOgFxPEXQO0qrXj1D+CprNR5y1\nsWXqHQZcj7ozPKdPlF01oKNbxn8layDudbiGn8ZBtrPiwlwT2fW1oVVI3+zyf7o2\n7QGbhq6ZXz9U3TAwTa+5nB8R8JNAO2xwwFKrtQLB9mtNuuYzk5mV8PV6sqKEqk+x\n4AjUYgO3BJ6jO+Cg9DfMV6WYQF8E9CvR4raOhKMFcjZQ/rAZz5QhFDLbWltxN6Qd\nnMiMshuP0rHOsUkQWr5suf8Wp4hqsb8zYgXklVHrwQKBgQDpIhzdV1RKCprUHioV\nGa1H3Z2lvrndMAZ8OgtdeagfImzICsloF3KvTpAAdkWN8cia/sjP3MKA8UzJGgF0\nb9N3XI4VsBIXcuL6Sz3hc0M2o017L0yCesNK/ffxygjwrHOZ4SUCerK4whddr7g1\n6l6DfoL6peiyT6+NSnUYKtyVMQKBgQDGJ21xKUHroiGR6vspMCh4fREKPKY4XJLu\nBrz5tIrCGmguUrBY690shtn7SQjsriTEN9EVjRc6xLfrY+tD4zvAhINwn6bTvdS5\nco6DmvKRwiyUX4VdqAEktwg2dLgLstHQKS6FGaMolRe9YWgRTwrv8BhsJzw/MxRb\ncu+Wt4gZ6wKBgQCzY9FsLC+qzaA3yoI9PEXO/+O3zxv77GGBI7TtF5jbZETqZQp3\ns1tHNB+wi1GYGM1xHs5szAVK7OJV+FHYQ9gnh6u5WoOBUaEAUfdqzKOSnnQXbtzj\npg0yXlx0zC626ywE4270CnANpSQPrhAERLS3YBjvP8zfsFt4UCvsDccwcQKBgFS0\nX/1CpLJEgVMt/qVxt6sh01nr6SYotIpZiQi5G6OzxBshL88jLE2va5kWdGEwY/kY\n3yD2ShrOIszVzqkbhtxaCRHovVjASiHoDXHGl7ClL4dReeI6Qhrevv0AUfh2PWhd\nYkx1VCCx8w76h5D2l/dPTDFXaFKf1DDvZemolN53AoGAftepQ1IBdcm23x4Z+/0X\nR8IOzZWxdgLO7llUPxrYr4xXKB/lTdwMwTezGAQSwGG8amTXH3Kh2TItg1kcEqnw\n0pv1w3pEXtsXF5+2JJZlpkDholWm4Sr371WLoibqZxur4i3s/oXc7g5sV8FK4Gc+\nQLrApvC3ECyPJuv/tZ/KPrM=\n-----END PRIVATE KEY-----\n"
}"#;
        let google = StandardTTSConfig {
            base: TTSConfig {
                provider: "google".into(),
                api_key: GOOGLE_TEST_SERVICE_ACCOUNT_JSON.into(),
                voice_id: Some("en-US-Wavenet-D".into()),
                ..Default::default()
            },
            features: TtsFeatures {
                pitch: Some(4.0),
                volume: Some(-3.0),
                speed: Some(1.5),
                language: Some("es-ES".into()),
                sample_rate: Some(48000),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
        };
        assert!(create_tts_standard("google", google).is_ok());

        let huawei_cloud = StandardTTSConfig {
            base: TTSConfig {
                provider: "huawei_cloud".into(),
                api_key: "user|pass|domain|project123".into(),
                voice_id: Some("xiaoyan".into()),
                ..Default::default()
            },
            features: TtsFeatures {
                speed: Some(1.0),
                pitch: Some(100.0),
                volume: Some(80.0),
                sample_rate: Some(16000),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
        };
        assert!(create_tts_standard("huawei_cloud", huawei_cloud).is_ok());

        let hume = StandardTTSConfig {
            base: TTSConfig {
                provider: "hume".into(),
                api_key: "k".into(),
                voice_id: Some("Kora".into()),
                ..Default::default()
            },
            features: TtsFeatures {
                instructions: Some("warm, friendly, inviting".into()),
                speed: Some(1.2),
                sample_rate: Some(24000),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
        };
        assert!(create_tts_standard("hume", hume).is_ok());

        let ibm_watson = StandardTTSConfig {
            base: TTSConfig {
                provider: "ibm_watson".into(),
                api_key: "k".into(),
                voice_id: Some("en-US_AllisonV3Voice".into()),
                ..Default::default()
            },
            features: TtsFeatures {
                speed: Some(1.25),
                pitch: Some(-10.0),
                sample_rate: Some(22050),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
        };
        assert!(create_tts_standard("ibm_watson", ibm_watson).is_ok());

        let iflytek = StandardTTSConfig {
            base: TTSConfig {
                provider: "iflytek".into(),
                api_key: "app_id|api_key|api_secret".into(),
                voice_id: Some("xiaoyan".into()),
                sample_rate: Some(16000),
                ..Default::default()
            },
            features: TtsFeatures {
                speed: Some(1.0),
                pitch: Some(60.0),
                volume: Some(70.0),
                sample_rate: Some(16000),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
        };
        assert!(create_tts_standard("iflytek", iflytek).is_ok());

        let lmnt = StandardTTSConfig {
            base: TTSConfig {
                provider: "lmnt".into(),
                api_key: "k".into(),
                voice_id: Some("lily".into()),
                ..Default::default()
            },
            features: TtsFeatures {
                speed: Some(1.5),
                stability: Some(0.9),
                language: Some("en".into()),
                seed: Some(12345),
                sample_rate: Some(16000),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
        };
        assert!(create_tts_standard("lmnt", lmnt).is_ok());

        let murf = StandardTTSConfig {
            base: TTSConfig {
                provider: "murf".into(),
                api_key: "k".into(),
                voice_id: Some("en-US-natalie".into()),
                model: "GEN2".into(),
                ..Default::default()
            },
            features: TtsFeatures {
                speed: Some(10.0),
                pitch: Some(-5.0),
                emotion: Some("Conversational".into()),
                language: Some("en-US".into()),
                sample_rate: Some(44100),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
        };
        assert!(create_tts_standard("murf", murf).is_ok());

        let naver_clova = StandardTTSConfig {
            base: TTSConfig {
                provider: "naver-clova".into(),
                api_key: "client_id|client_secret".into(),
                voice_id: Some("clara".into()),
                ..Default::default()
            },
            features: TtsFeatures {
                speed: Some(4.0),
                pitch: Some(3.0),
                volume: Some(-2.0),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
        };
        assert!(create_tts_standard("naver-clova", naver_clova).is_ok());

        let nectec = StandardTTSConfig {
            base: TTSConfig {
                provider: "nectec".into(),
                api_key: "test_key".into(),
                voice_id: Some("female".into()),
                ..Default::default()
            },
            features: TtsFeatures::default(),
            extras: ProviderExtras::default(),
        };
        assert!(create_tts_standard("nectec", nectec).is_ok());

        let openai = StandardTTSConfig {
            base: TTSConfig {
                provider: "openai".into(),
                api_key: "k".into(),
                voice_id: Some("nova".into()),
                model: "tts-1-hd".into(),
                ..Default::default()
            },
            features: TtsFeatures {
                speed: Some(1.5),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
        };
        assert!(create_tts_standard("openai", openai).is_ok());

        let mut playht_extras = serde_json::Map::new();
        playht_extras.insert("user_id".into(), serde_json::json!("user-123"));
        let playht = StandardTTSConfig {
            base: TTSConfig {
                provider: "playht".into(),
                api_key: "k".into(),
                voice_id: Some("s3://voice-cloning-zero-shot/manifest.json".into()),
                ..Default::default()
            },
            features: TtsFeatures {
                speed: Some(1.4),
                style: Some(0.6),
                language: Some("en".into()),
                seed: Some(42),
                sample_rate: Some(24000),
                ..Default::default()
            },
            extras: ProviderExtras(playht_extras),
        };
        assert!(create_tts_standard("playht", playht).is_ok());

        let mut prosa_extras = serde_json::Map::new();
        prosa_extras.insert("label".into(), serde_json::json!("greeting"));
        let prosa = StandardTTSConfig {
            base: TTSConfig {
                provider: "prosa_ai".into(),
                api_key: "k".into(),
                ..Default::default()
            },
            features: TtsFeatures {
                speed: Some(1.5),
                pitch: Some(3.0),
                ..Default::default()
            },
            extras: ProviderExtras(prosa_extras),
        };
        assert!(create_tts_standard("prosa_ai", prosa).is_ok());

        let mut resemble_extras = serde_json::Map::new();
        resemble_extras.insert("project_uuid".into(), serde_json::json!("proj-123"));
        resemble_extras.insert("use_hd".into(), serde_json::json!(true));
        let resemble = StandardTTSConfig {
            base: TTSConfig {
                provider: "resemble".into(),
                api_key: "k".into(),
                voice_id: Some("voice-uuid".into()),
                ..Default::default()
            },
            features: TtsFeatures {
                sample_rate: Some(44100),
                ..Default::default()
            },
            extras: ProviderExtras(resemble_extras),
        };
        assert!(create_tts_standard("resemble", resemble).is_ok());

        let mut reverie_extras = serde_json::Map::new();
        reverie_extras.insert("format".into(), serde_json::json!("mp3"));
        let reverie = StandardTTSConfig {
            base: TTSConfig {
                provider: "reverie".into(),
                api_key: "k".into(),
                voice_id: Some("hi_female".into()),
                model: "app-id-123".into(),
                ..Default::default()
            },
            features: TtsFeatures {
                speed: Some(1.4),
                pitch: Some(2.0),
                language: Some("hi".into()),
                sample_rate: Some(16000),
                ..Default::default()
            },
            extras: ProviderExtras(reverie_extras),
        };
        assert!(create_tts_standard("reverie", reverie).is_ok());

        let mut sber_extras = serde_json::Map::new();
        sber_extras.insert("connection_timeout_secs".into(), serde_json::json!(15));
        let sberdevices = StandardTTSConfig {
            base: TTSConfig {
                provider: "sberdevices".into(),
                api_key: "client_id:client_secret".into(),
                voice_id: Some("Nec".into()),
                ..Default::default()
            },
            features: TtsFeatures {
                sample_rate: Some(8000),
                ..Default::default()
            },
            extras: ProviderExtras(sber_extras),
        };
        assert!(create_tts_standard("sberdevices", sberdevices).is_ok());

        let mut smallest_extras = serde_json::Map::new();
        smallest_extras.insert("enhancement".into(), serde_json::json!(2));
        let smallest = StandardTTSConfig {
            base: TTSConfig {
                provider: "smallest".into(),
                api_key: "k".into(),
                voice_id: Some("emily".into()),
                model: "lightning".into(),
                ..Default::default()
            },
            features: TtsFeatures {
                speed: Some(1.5),
                stability: Some(0.8),
                similarity_boost: Some(0.6),
                language: Some("en".into()),
                sample_rate: Some(16000),
                ..Default::default()
            },
            extras: ProviderExtras(smallest_extras),
        };
        assert!(create_tts_standard("smallest", smallest).is_ok());

        let mut speechify_extras = serde_json::Map::new();
        speechify_extras.insert("loudness_normalization".into(), serde_json::json!(true));
        let speechify = StandardTTSConfig {
            base: TTSConfig {
                provider: "speechify".into(),
                api_key: "k".into(),
                voice_id: Some("george".into()),
                ..Default::default()
            },
            features: TtsFeatures {
                language: Some("es-ES".into()),
                ..Default::default()
            },
            extras: ProviderExtras(speechify_extras),
        };
        assert!(create_tts_standard("speechify", speechify).is_ok());

        let speechmatics = StandardTTSConfig {
            base: TTSConfig {
                provider: "speechmatics".into(),
                api_key: "k".into(),
                voice_id: Some("jack".into()),
                audio_format: Some("pcm".into()),
                ..Default::default()
            },
            features: TtsFeatures {
                // Speechmatics has no prosody surface; these stay capability gaps.
                speed: Some(1.5),
                sample_rate: Some(48000),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
        };
        assert!(create_tts_standard("speechmatics", speechmatics).is_ok());

        let tencent = StandardTTSConfig {
            base: TTSConfig {
                provider: "tencent".into(),
                api_key: "secret_id|secret_key".into(),
                voice_id: Some("0".into()),
                ..Default::default()
            },
            features: TtsFeatures {
                speed: Some(1.5),
                volume: Some(8.0),
                word_timestamps: Some(true),
                emotion: Some("happy".into()),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
        };
        assert!(create_tts_standard("tencent", tencent).is_ok());

        let mut tinkoff_extras = serde_json::Map::new();
        tinkoff_extras.insert("connection_timeout_secs".into(), serde_json::json!(20));
        let tinkoff = StandardTTSConfig {
            base: TTSConfig {
                provider: "tinkoff".into(),
                api_key: "k".into(),
                voice_id: Some("alyona".into()),
                ..Default::default()
            },
            features: TtsFeatures {
                speed: Some(2.0),
                pitch: Some(5.0),
                volume: Some(-6.0),
                sample_rate: Some(48000),
                ..Default::default()
            },
            extras: ProviderExtras(tinkoff_extras),
        };
        assert!(create_tts_standard("tinkoff", tinkoff).is_ok());

        let mut unrealspeech_extras = serde_json::Map::new();
        unrealspeech_extras.insert("bitrate".into(), serde_json::json!(320));
        let unrealspeech = StandardTTSConfig {
            base: TTSConfig {
                provider: "unrealspeech".into(),
                api_key: "k".into(),
                voice_id: Some("Dan".into()),
                ..Default::default()
            },
            features: TtsFeatures {
                speed: Some(0.5),
                pitch: Some(1.2),
                ..Default::default()
            },
            extras: ProviderExtras(unrealspeech_extras),
        };
        assert!(create_tts_standard("unrealspeech", unrealspeech).is_ok());

        let viettel_ai = StandardTTSConfig {
            base: TTSConfig {
                provider: "viettel_ai".into(),
                api_key: "test_token".into(),
                voice_id: Some("doanngocle".into()),
                ..Default::default()
            },
            features: TtsFeatures {
                speed: Some(1.5),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
        };
        assert!(create_tts_standard("viettel_ai", viettel_ai).is_ok());

        let wellsaid = StandardTTSConfig {
            base: TTSConfig {
                provider: "wellsaid".into(),
                api_key: "k".into(),
                voice_id: Some("26".into()),
                model: "caruso".into(),
                ..Default::default()
            },
            features: TtsFeatures {
                // WellSaid stores only voice/model; prosody features are capability gaps.
                speed: Some(1.5),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
        };
        assert!(create_tts_standard("wellsaid", wellsaid).is_ok());

        let yandex = StandardTTSConfig {
            base: TTSConfig {
                provider: "yandex".into(),
                api_key: "AQVN1234567890".into(),
                voice_id: Some("alena".into()),
                ..Default::default()
            },
            features: TtsFeatures {
                speed: Some(1.5),
                emotion: Some("cheerful".into()),
                language: Some("en-US".into()),
                sample_rate: Some(16000),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
        };
        assert!(create_tts_standard("yandex", yandex).is_ok());

        let zalo_ai = StandardTTSConfig {
            base: TTSConfig {
                provider: "zalo_ai".into(),
                api_key: "test_key".into(),
                voice_id: Some("male_north".into()),
                ..Default::default()
            },
            features: TtsFeatures {
                speed: Some(1.1),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
        };
        assert!(create_tts_standard("zalo_ai", zalo_ai).is_ok());
    }
}
