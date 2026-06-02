//! Keystone wiring test (W-A0, extreme TDD RED→GREEN).
//!
//! Proves that an advanced STT feature carried on the live WebSocket config (`features.diarization
//! = true`) survives the SAME conversion the live handler uses and reaches the Deepgram provider
//! config with `diarize` enabled — closing the gap where the live WS/REST path never reached
//! `from_standard` and silently dropped advanced features (BRUTAL_REVIEW.md S1).
//!
//! Before the protocol+wiring change this file does not compile (the WS config can't carry
//! `features` and has no `to_standard_stt`), which is the intended RED state.

use waav_gateway::core::stt::deepgram::DeepgramSTTConfig;
use waav_gateway::core::stt::standard::{SttFeatures, create_stt_standard};
use waav_gateway::handlers::ws::config::STTWebSocketConfig;

/// A WS STT config carrying `features.diarization = true` for Deepgram must, after the SAME
/// conversion the live handler performs (`to_standard_stt`), produce a Deepgram provider config
/// with diarization enabled — and dispatch successfully through `create_stt_standard`.
#[test]
fn ws_stt_features_reach_deepgram_diarize_through_keystone() {
    // 1. The client-supplied live WS config, with an advanced feature set.
    let ws = STTWebSocketConfig {
        provider: "deepgram".to_string(),
        language: "en-US".to_string(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "linear16".to_string(),
        model: "nova-3".to_string(),
        api_key: Some("test-key".to_string()),
        features: SttFeatures {
            diarization: Some(true),
            ..Default::default()
        },
        extras: Default::default(),
    };

    // 2. The SAME conversion the live handler uses to cross the dispatch boundary.
    let std = ws.to_standard_stt("server-key".to_string());

    // The feature is carried through the standardized config (not dropped on the flat path).
    assert_eq!(std.features.diarization, Some(true));
    assert_eq!(std.base.provider, "deepgram");
    assert_eq!(std.base.model, "nova-3");

    // 3a. The Deepgram provider config built from the standardized config has diarize enabled.
    let dg = DeepgramSTTConfig::from_standard(&std);
    assert!(
        dg.diarize,
        "diarization carried on the WS config must reach DeepgramSTTConfig.diarize"
    );

    // 3b. And dispatch through the reachable keystone path constructs the provider.
    assert!(
        create_stt_standard("deepgram", std).is_ok(),
        "create_stt_standard must build Deepgram from the standardized WS-derived config"
    );
}

/// A WS config with an empty client key falls back to the server key, and still carries features.
#[test]
fn ws_stt_to_standard_uses_provided_key_and_carries_features() {
    let ws = STTWebSocketConfig {
        provider: "deepgram".to_string(),
        language: "en-US".to_string(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "linear16".to_string(),
        model: "nova-3".to_string(),
        api_key: None,
        features: SttFeatures {
            diarization: Some(true),
            smart_format: Some(false),
            ..Default::default()
        },
        extras: Default::default(),
    };

    let std = ws.to_standard_stt("resolved-server-key".to_string());
    assert_eq!(std.base.api_key, "resolved-server-key");
    assert_eq!(std.features.diarization, Some(true));

    let dg = DeepgramSTTConfig::from_standard(&std);
    assert!(dg.diarize);
    assert!(!dg.smart_format, "smart_format=false must survive the wire");
}
