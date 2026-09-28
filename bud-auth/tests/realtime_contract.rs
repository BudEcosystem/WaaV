//! TC-PUB-15 — the realtime half of the `voice_table` wire contract (FRD-023 §5.3, CONTRACTS C1).
//!
//! `tests/fixtures/realtime_voice_entry.json` is **byte-identical** to
//! `bud-runtime/services/budapp/tests/fixtures/realtime_voice_entry.json`. budapp's publisher test
//! asserts it BUILDS exactly that entry; this suite asserts WaaV PARSES it into the settings the
//! relay enforces. Either half alone lets the two sides drift, and drift here is silent: a
//! `realtime` block WaaV cannot read degrades to vendor defaults and an open policy.

use bud_auth::credentials::{CredentialDecryptor, parse_voice_blob};

const FIXTURE: &str = include_str!("fixtures/realtime_voice_entry.json");

fn parse(json: &str) -> bud_auth::VoiceEndpoint {
    let map = parse_voice_blob(json, &CredentialDecryptor::disabled()).expect("blob parses");
    map.into_values().next().expect("one endpoint")
}

#[test]
fn the_fixture_parses_into_a_realtime_endpoint() {
    let ep = parse(FIXTURE);
    assert_eq!(ep.vendor, "openai");
    assert!(ep.serves("realtime_session"));
    assert_eq!(ep.model.as_deref(), Some("gpt-realtime-2.1"));
    assert_eq!(ep.policy.max_concurrent, Some(20));
}

#[test]
fn the_realtime_block_round_trips() {
    let ep = parse(FIXTURE);
    let rt = ep.config.realtime.as_ref().expect("realtime block parsed");
    assert_eq!(rt.session_type.as_deref(), Some("realtime"));

    let d = rt.defaults.as_ref().expect("defaults");
    assert_eq!(d.voice.as_deref(), Some("marin"));
    assert_eq!(
        d.instructions.as_deref(),
        Some("You are a helpful assistant.")
    );
    assert_eq!(
        d.output_modalities.as_deref(),
        Some(&["audio".to_string()][..])
    );
    assert_eq!(
        d.turn_detection,
        Some(serde_json::json!({"type": "semantic_vad", "eagerness": "auto"}))
    );
    let tr = d.input_transcription.as_ref().expect("input_transcription");
    assert_eq!(tr.model.as_deref(), Some("gpt-4o-mini-transcribe"));
    assert_eq!(tr.language.as_deref(), Some("en"));
    assert_eq!(d.noise_reduction.as_deref(), Some("near_field"));
    assert_eq!(d.max_output_tokens, Some(4096));
    assert_eq!(d.speed, Some(1.0));

    let l = rt.limits.as_ref().expect("limits");
    assert_eq!(l.max_session_seconds, Some(3600));
    assert_eq!(l.idle_timeout_seconds, Some(300));

    let p = rt.policy.clone().unwrap_or_default();
    assert!(p.allows_client_instructions());
    assert!(!p.allows_mcp_tools());
    assert!(!p.allows_prompt_references());
    assert!(p.allows_image_input());
    assert!(p.allows_transcription_model("gpt-4o-mini-transcribe"));
    assert!(!p.allows_transcription_model("gpt-4o-transcribe"));
}

#[test]
fn the_token_price_carries_its_rates_and_no_cost_per_unit() {
    let ep = parse(FIXTURE);
    let pricing = ep.pricing.expect("a token price with rates is usable");
    assert_eq!(pricing.unit, "token");
    assert_eq!(pricing.per_units, 1_000_000);
    assert_eq!(pricing.rates.get("input_audio"), Some(&32.0));
    assert_eq!(pricing.rates.get("output_audio"), Some(&64.0));
    assert_eq!(pricing.rates.get("cached_input_audio"), Some(&0.4));
    assert_eq!(pricing.rates.get("transcription_per_minute"), Some(&0.003));
    assert_eq!(pricing.rates.len(), 9);
}

/// FRD-023 §5.3: "a malformed block drops that block with a warning, never the endpoint".
#[test]
fn a_malformed_realtime_block_drops_the_block_not_the_endpoint() {
    let blob = serde_json::json!({"ep-1": {
        "vendor": "openai",
        "endpoints": ["realtime_session"],
        "model": "gpt-realtime-2.1",
        "config": {"realtime": "not an object"}
    }})
    .to_string();
    let ep = parse(&blob);
    assert!(ep.serves("realtime_session"), "the endpoint must survive");
    assert!(ep.config.realtime.is_none());
}

#[test]
fn a_malformed_field_inside_realtime_drops_only_that_field() {
    let blob = serde_json::json!({"ep-1": {
        "vendor": "openai",
        "endpoints": ["realtime_session"],
        "config": {"realtime": {
            "session_type": "realtime",
            "defaults": {"speed": "fast"},
            "policy": {"allow_mcp_tools": true}
        }}
    }})
    .to_string();
    let ep = parse(&blob);
    let rt = ep.config.realtime.expect("block kept");
    assert_eq!(rt.session_type.as_deref(), Some("realtime"));
    assert!(
        rt.defaults.is_none(),
        "the malformed defaults block is dropped"
    );
    assert!(
        rt.policy.unwrap().allows_mcp_tools(),
        "the good sibling survives"
    );
}

#[test]
fn a_token_price_without_rates_is_unusable_not_zero() {
    // A token price with no rates would price every response at zero. The endpoint is served
    // unpriced instead, and the turn says so (FRD-023 §5.10).
    let blob = serde_json::json!({"ep-1": {
        "vendor": "openai",
        "endpoints": ["realtime_session"],
        "pricing": {"unit": "token", "per_units": 1000000, "currency": "USD"}
    }})
    .to_string();
    assert!(parse(&blob).pricing.is_none());
}

#[test]
fn unknown_rate_keys_are_ignored_not_fatal() {
    let blob = serde_json::json!({"ep-1": {
        "vendor": "openai",
        "endpoints": ["realtime_session"],
        "pricing": {"unit": "token", "per_units": 1000000,
                    "rates": {"input_audio": 32, "output_audio": 64, "input_video": 1, "output_text": "24"}}
    }})
    .to_string();
    let pricing = parse(&blob).pricing.expect("usable");
    assert_eq!(pricing.rates.get("input_video"), None);
    assert_eq!(
        pricing.rates.get("output_text"),
        Some(&24.0),
        "numeric strings are numbers"
    );
    assert_eq!(pricing.rates.len(), 3);
}

#[test]
fn a_minute_price_on_session_time_needs_no_rates() {
    let blob = serde_json::json!({"ep-1": {
        "vendor": "openai",
        "endpoints": ["realtime_session"],
        "pricing": {"unit": "minute", "cost_per_unit": 0.06, "per_units": 1}
    }})
    .to_string();
    let pricing = parse(&blob).pricing.expect("usable");
    assert_eq!(pricing.unit, "minute");
    assert!(pricing.rates.is_empty());
}

#[test]
fn an_endpoint_without_realtime_config_has_none() {
    let blob = serde_json::json!({"ep-1": {"vendor": "deepgram", "endpoints": ["text_to_speech"]}})
        .to_string();
    assert!(parse(&blob).config.realtime.is_none());
}

/// S-5: a deployment that says nothing about vendor-org resources does not expose them. The
/// fixture sets every flag explicitly, so this is the case that pins the DEFAULTS.
#[test]
fn absent_policy_fields_take_the_secure_defaults() {
    let blob = serde_json::json!({"ep-1": {
        "vendor": "openai",
        "endpoints": ["realtime_session"],
        "config": {"realtime": {"session_type": "realtime", "policy": {}}}
    }})
    .to_string();
    let policy = parse(&blob).config.realtime.expect("block").policy();
    assert!(!policy.allows_mcp_tools(), "MCP tools reach the vendor org");
    assert!(
        !policy.allows_prompt_references(),
        "stored prompts belong to the vendor org"
    );
    assert!(policy.allows_client_instructions());
    assert!(policy.allows_image_input());
    assert!(
        policy.allows_transcription_model("anything"),
        "no allowlist = any model"
    );

    let none = bud_auth::RealtimePolicy::default();
    assert!(!none.allows_mcp_tools() && !none.allows_prompt_references());
}
