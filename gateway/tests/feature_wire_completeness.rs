//! Feature-wire completeness gate (credential-free).
//!
//! Live testing proved that a feature can be accepted into `StandardSTTConfig` and then silently
//! DROPPED before it ever reaches the wire (Sarvam, Alibaba DashScope). The per-provider live tests
//! can only cover the handful of vendors we hold keys for, so this gate gives credential-free,
//! fleet-level proof that advertised standardized features actually reach the outgoing request — by
//! driving each provider's REAL `from_standard` + request builder and asserting the wire string.
//!
//! Each case builds a feature-rich `StandardSTTConfig` (typed features + `ProviderExtras`
//! passthrough), runs it through the provider's `from_standard` and its public URL builder, and
//! asserts every feature it claims to honor appears on the wire. A `from_standard` that maps a
//! feature to a config field but never serializes it (the bug class this guards) fails here.
//!
//! Scope: providers whose request builder is publicly callable without a network/key (the WS-URL
//! providers). REST/gRPC/multipart and private-builder providers are covered by their own in-module
//! wire-assert unit tests; extending this gate to them is gated on the `endpoint_override` harness.

use serde_json::json;
use waav_gateway::core::stt::STTConfig;
use waav_gateway::core::stt::assemblyai::AssemblyAISTTConfig;
use waav_gateway::core::stt::cartesia::CartesiaSTTConfig;
use waav_gateway::core::stt::elevenlabs::ElevenLabsSTTConfig;
use waav_gateway::core::stt::revai::RevAISTTConfig;
use waav_gateway::core::stt::sarvam::SarvamSTTConfig;
use waav_gateway::core::stt::standard::{ProviderExtras, SttFeatures, StandardSTTConfig};

/// Build a `StandardSTTConfig` for `provider` from a typed feature set + extras passthrough.
fn std_cfg(provider: &str, features: SttFeatures, extras: ProviderExtras) -> StandardSTTConfig {
    StandardSTTConfig {
        base: STTConfig {
            provider: provider.to_string(),
            api_key: "test-key".to_string(),
            language: "en".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
            model: String::new(),
        },
        features,
        extras,
    }
}

/// Assert every needle appears in the built URL (each represents a feature reaching the wire).
fn assert_all(url: &str, needles: &[&str]) {
    for n in needles {
        assert!(url.contains(n), "feature param `{n}` missing from wire URL:\n{url}");
    }
}

#[test]
fn cartesia_stt_features_reach_websocket_url() {
    // endpointing_ms -> max_silence_duration_secs (typed); access_token (extras) replaces api_key.
    let mut extras = ProviderExtras::default();
    extras.0.insert("access_token".into(), json!("tok-123"));
    let std = std_cfg(
        "cartesia",
        SttFeatures {
            endpointing_ms: Some(900),
            ..Default::default()
        },
        extras,
    );
    let cfg = CartesiaSTTConfig::from_standard(&std);
    let url = cfg.build_websocket_url(&std.base.api_key);
    assert_all(
        &url,
        &[
            "access_token=tok-123",         // extras passthrough auth
            "max_silence_duration_secs=",   // endpointing_ms -> typed silence
            "model=",                       // model rides the URL
            "encoding=",                    // codec rides the URL
            "cartesia_version=",            // version header carried as query
        ],
    );
}

#[test]
fn elevenlabs_stt_features_reach_websocket_url() {
    let std = std_cfg(
        "elevenlabs",
        SttFeatures {
            word_timestamps: Some(true),
            diarization: Some(true),
            entity_detection: Some(true),
            language_detection: Some(true),
            filler_words: Some(false),
            keyterms: Some(vec!["WaaV".into(), "Bud".into()]),
            ..Default::default()
        },
        ProviderExtras::default(),
    );
    let cfg = ElevenLabsSTTConfig::from_standard(&std);
    let url = cfg.build_websocket_url();
    assert_all(
        &url,
        &[
            "include_timestamps=true",      // word_timestamps
            "diarization=true",             // diarization
            "entity_detection=true",        // entity_detection
            "include_language_detection=",  // language_detection
            "no_verbatim=",                 // filler_words=false -> no_verbatim
            "keyterms=",                    // keyterm biasing
        ],
    );
}

#[test]
fn revai_stt_features_reach_websocket_url() {
    let mut extras = ProviderExtras::default();
    extras.0.insert("priority".into(), json!("speed"));
    extras
        .0
        .insert("max_connection_wait_seconds".into(), json!(30));
    let std = std_cfg(
        "revai",
        SttFeatures {
            diarization: Some(true),
            profanity_filter: Some(true),
            filler_words: Some(false),
            ..Default::default()
        },
        extras,
    );
    let cfg = RevAISTTConfig::from_standard(&std).expect("revai from_standard");
    let url = cfg.build_websocket_url();
    assert_all(
        &url,
        &[
            "enable_speaker_switch=true",       // diarization
            "filter_profanity=true",            // profanity_filter
            "remove_disfluencies=true",         // filler_words=false
            "priority=",                        // extras passthrough
            "max_connection_wait_seconds=",     // extras passthrough
        ],
    );
}

#[test]
fn assemblyai_stt_features_reach_websocket_url() {
    let mut extras = ProviderExtras::default();
    extras.0.insert("domain".into(), json!("medical"));
    extras.0.insert("vad_threshold".into(), json!(0.6));
    let std = std_cfg(
        "assemblyai",
        SttFeatures {
            diarization: Some(true),
            language_detection: Some(true),
            keyterms: Some(vec!["WaaV".into()]),
            ..Default::default()
        },
        extras,
    );
    let cfg = AssemblyAISTTConfig::from_standard(&std);
    let url = cfg.build_websocket_url();
    assert_all(
        &url,
        &[
            "speaker_labels=true",      // diarization
            "language_detection=true",  // language_detection
            "keyterms_prompt=",         // keyterm biasing
            "domain=",                  // extras passthrough
            "vad_threshold=",           // extras passthrough
        ],
    );
}

#[test]
fn sarvam_stt_features_reach_websocket_url() {
    // vad_events -> vad_signals (typed); transcription mode + ASR prompt biasing (extras).
    let mut extras = ProviderExtras::default();
    extras.0.insert("mode".into(), json!("formatted"));
    extras.0.insert("prompt".into(), json!("WaaV gateway"));
    let std = std_cfg(
        "sarvam",
        SttFeatures {
            vad_events: Some(true),
            ..Default::default()
        },
        extras,
    );
    let cfg = SarvamSTTConfig::from_standard(&std);
    let url = cfg.build_websocket_url();
    assert_all(
        &url,
        &[
            "vad_signals=true", // vad_events -> speech-activity signals
            "mode=formatted",   // extras transcription mode
            "prompt=",          // extras ASR biasing prompt
        ],
    );
}
