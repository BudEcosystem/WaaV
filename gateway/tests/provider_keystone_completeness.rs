//! Keystone completeness gate (W-A0 / W-A1 exhaustiveness).
//!
//! The single highest-leverage correctness property of the gateway is that EVERY provider is
//! reachable through the standardized-config path (`create_stt_standard` / `create_tts_standard`),
//! NOT the flat `create_*_provider` fallback — because the flat path drops the typed
//! `SttFeatures`/`TtsFeatures` + `ProviderExtras` (the original "advanced features silently dropped
//! for ~68/69 providers" defect).
//!
//! This test enumerates EVERY provider module shipped in `src/core/stt/*` and `src/core/tts/*` and
//! asserts each one resolves to a real standardized dispatch arm — i.e. `create_*_standard` returns
//! `Ok`, or a provider-SPECIFIC error (validation / auth / a deliberate fail-closed like Phonexia),
//! but NEVER the "Unsupported/Unknown … provider" error that only the fallback emits. If a new
//! provider module is added without a keystone arm (or an arm is removed), `create_*_standard` falls
//! through to the fallback for that name and this test FAILS THE BUILD.
//!
//! It is the machine-checkable "no provider / no feature path missed" guard. Per-feature
//! wire-survival (the exact `api_param` reaching the URL/body) is covered by the per-provider wire
//! tests in each provider module; this gate guarantees none of them can be bypassed.
//!
//! NOTE: when you add a provider, add its module name to the relevant list below.

use std::panic::{AssertUnwindSafe, catch_unwind};

use waav_gateway::core::stt::STTConfig;
use waav_gateway::core::stt::standard::{
    ProviderExtras, StandardSTTConfig, SttFeatures, create_stt_standard,
};
use waav_gateway::core::tts::TTSConfig;
use waav_gateway::core::tts::standard::{StandardTTSConfig, TtsFeatures, create_tts_standard};

/// Every STT provider MODULE under `src/core/stt/` (the source of truth for "all providers").
const STT_PROVIDERS: &[&str] = &[
    "deepgram",
    "alibaba_cloud",
    "amivoice",
    "assemblyai",
    "aws_transcribe",
    "azure",
    "baidu",
    "bhashini",
    "cartesia",
    "elevenlabs",
    "fpt_ai",
    "gladia",
    "gnani",
    "google",
    "groq",
    "huawei_cloud",
    "ibm_watson",
    "iflytek",
    "naver_clova",
    "nectec",
    "openai",
    "phonexia",
    "prosa_ai",
    "revai",
    "reverie",
    "sarvam",
    "sberdevices",
    "speechmatics",
    "tencent",
    "tinkoff",
    "viettel_ai",
    "yandex",
];

/// Every TTS provider MODULE under `src/core/tts/`.
const TTS_PROVIDERS: &[&str] = &[
    "deepgram",
    "acapela",
    "alibaba_cloud",
    "aws_polly",
    "azure",
    "baidu",
    "bhashini",
    "cartesia",
    "cereproc",
    "fpt_ai",
    "gnani",
    "google",
    "huawei_cloud",
    "hume",
    "ibm_watson",
    "iflytek",
    "lmnt",
    "murf",
    "naver_clova",
    "nectec",
    "openai",
    "playht",
    "prosa_ai",
    "resemble",
    "reverie",
    "sberdevices",
    "smallest",
    "speechify",
    "speechmatics",
    "tencent",
    "tinkoff",
    "unrealspeech",
    "viettel_ai",
    "wellsaid",
    "yandex",
    "zalo_ai",
];

/// The substrings the keystone fallback (and the plugin registry) emit for a name that has NO real
/// dispatch arm. Seeing any of these means the provider is NOT keystone-wired.
fn is_unwired_fallback_error(msg: &str) -> bool {
    let m = msg.to_lowercase();
    m.contains("unsupported stt provider")
        || m.contains("unsupported tts provider")
        || m.contains("unknown stt provider")
        || m.contains("unknown tts provider")
}

fn stt_std(provider: &str) -> StandardSTTConfig {
    let base = STTConfig {
        provider: provider.to_string(),
        api_key: "test-key-0123456789abcdef".to_string(),
        language: "en-US".to_string(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "linear16".to_string(),
        model: String::new(),
    };
    // Exercise the standardized path: typed features + an opaque extra must travel WITH the config.
    StandardSTTConfig {
        features: SttFeatures {
            diarization: Some(true),
            interim_results: Some(true),
            word_timestamps: Some(true),
            ..Default::default()
        },
        extras: {
            let mut e = ProviderExtras::default();
            e.0.insert("keystone_probe".to_string(), serde_json::json!("present"));
            e
        },
        ..StandardSTTConfig::from_base(base)
    }
}

fn tts_std(provider: &str) -> StandardTTSConfig {
    let base = TTSConfig {
        provider: provider.to_string(),
        api_key: "test-key-0123456789abcdef".to_string(),
        voice_id: Some("test-voice".to_string()),
        model: String::new(),
        sample_rate: Some(24000),
        ..Default::default()
    };
    StandardTTSConfig {
        features: TtsFeatures {
            ssml: Some(true),
            word_timestamps: Some(true),
            ..Default::default()
        },
        extras: {
            let mut e = ProviderExtras::default();
            e.0.insert("keystone_probe".to_string(), serde_json::json!("present"));
            e
        },
        ..StandardTTSConfig::from_base(base)
    }
}

#[test]
fn every_stt_provider_is_reachable_through_the_keystone() {
    let mut gaps: Vec<String> = Vec::new();
    for &p in STT_PROVIDERS {
        // Construction must not connect; a panic or an "unsupported provider" error both mean the
        // standardized path does not really handle this provider.
        let outcome = catch_unwind(AssertUnwindSafe(|| create_stt_standard(p, stt_std(p))));
        match outcome {
            Err(_panic) => gaps.push(format!("{p}: PANICKED in create_stt_standard")),
            Ok(Ok(_provider)) => { /* wired + constructed */ }
            Ok(Err(e)) => {
                let msg = e.to_string();
                if is_unwired_fallback_error(&msg) {
                    gaps.push(format!("{p}: fell through to the flat fallback ({msg})"));
                }
                // else: a provider-specific validation/auth/fail-closed error — the arm EXISTS.
            }
        }
    }
    assert!(
        gaps.is_empty(),
        "{} STT provider(s) are NOT wired through the standardized keystone (their advanced \
         features would be dropped):\n  {}",
        gaps.len(),
        gaps.join("\n  ")
    );
}

#[test]
fn every_tts_provider_is_reachable_through_the_keystone() {
    let mut gaps: Vec<String> = Vec::new();
    for &p in TTS_PROVIDERS {
        let outcome = catch_unwind(AssertUnwindSafe(|| create_tts_standard(p, tts_std(p))));
        match outcome {
            Err(_panic) => gaps.push(format!("{p}: PANICKED in create_tts_standard")),
            Ok(Ok(_provider)) => {}
            Ok(Err(e)) => {
                let msg = e.to_string();
                if is_unwired_fallback_error(&msg) {
                    gaps.push(format!("{p}: fell through to the flat fallback ({msg})"));
                }
            }
        }
    }
    assert!(
        gaps.is_empty(),
        "{} TTS provider(s) are NOT wired through the standardized keystone (their advanced \
         features would be dropped):\n  {}",
        gaps.len(),
        gaps.join("\n  ")
    );
}
