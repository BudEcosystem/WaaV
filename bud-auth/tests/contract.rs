//! TC-CRED-02 — the cross-repository wire contract with budapp.
//!
//! `tests/fixtures/voice_table_contract.json` is **byte-identical** to
//! `bud-runtime/services/budapp/tests/fixtures/voice_table_contract.json`. budapp's
//! `test_voice_publisher.py` asserts its publisher *produces* these cases; this suite asserts
//! WaaV *consumes* them.
//!
//! Both halves are needed. Either alone lets the two sides drift, and drift in this contract is
//! silent by nature: budgateway's equivalent config is `deny_unknown_fields`, so a mismatched
//! entry disappears from the table with no error logged anywhere and the endpoint simply stops
//! existing.

use bud_auth::credentials::{CredentialDecryptor, parse_voice_blob};

const CONTRACT: &str = include_str!("fixtures/voice_table_contract.json");

fn cases() -> serde_json::Map<String, serde_json::Value> {
    let v: serde_json::Value = serde_json::from_str(CONTRACT).expect("contract fixture is json");
    v.as_object()
        .expect("contract fixture is an object")
        .iter()
        .filter(|(k, _)| !k.starts_with('_'))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// Every case budapp can publish must parse here.
#[test]
fn every_contract_case_parses() {
    for (name, entry) in cases() {
        let blob = serde_json::json!({ "ep-1": entry }).to_string();
        // The fixture's credential is a hex-shaped placeholder rather than real ciphertext, so
        // parse with decryption disabled — this suite is about SHAPE, not crypto. The crypto
        // path has its own coverage in `credentials.rs`.
        let parsed = parse_voice_blob(&blob, &CredentialDecryptor::disabled());

        match name.as_str() {
            // A case carrying a credential needs a decryptor; without one the endpoint is
            // correctly dropped rather than used with an unopened secret.
            "vendor_backed" | "aws_polly" | "with_deployment_policy" => {
                let map = parsed.unwrap_or_else(|e| panic!("case {name} must parse: {e}"));
                assert!(
                    map.is_empty(),
                    "an endpoint with an unopenable credential was kept; its ciphertext would be \
                     sent to the vendor as a bearer token"
                );
            }
            _ => {
                let map = parsed.unwrap_or_else(|e| panic!("case {name} failed to parse: {e}"));
                assert!(
                    map.contains_key("ep-1"),
                    "case {name} parsed to nothing; budapp and WaaV have drifted"
                );
            }
        }
    }
}

/// `provider_params` is plaintext and per-vendor: a Google deployment's project and location reach
/// the endpoint as budapp published them.
#[test]
fn the_google_case_carries_its_provider_params() {
    let entry = cases()
        .get("google_params")
        .cloned()
        .expect("fixture has a google_params case");
    let blob = serde_json::json!({ "ep-1": entry }).to_string();
    let map = parse_voice_blob(&blob, &CredentialDecryptor::disabled()).expect("parses");
    let ep = map
        .get("ep-1")
        .expect("kept: it carries no credential to open");
    assert_eq!(ep.provider_param("project_id"), Some("acme-speech"));
    assert_eq!(ep.provider_param("location"), Some("us"));
}

/// The self-hosted case is the one FRD §5.2 turns on: a cluster deployment URL riding in the
/// same field vendors use, so the resolver never branches on vendor kind.
#[test]
fn the_self_hosted_case_carries_a_deployment_url_and_no_credential() {
    let entry = cases()
        .get("self_hosted")
        .cloned()
        .expect("fixture has a self_hosted case");
    let blob = serde_json::json!({ "ep-sh": entry }).to_string();

    let map = parse_voice_blob(&blob, &CredentialDecryptor::disabled()).expect("parses");
    let ep = map.get("ep-sh").expect("endpoint present");

    assert_eq!(ep.vendor, "self_hosted");
    assert!(
        ep.api_base
            .as_deref()
            .unwrap_or_default()
            .starts_with("http://"),
        "the deployment URL did not survive; the resolver would have nowhere to send audio"
    );
    assert!(
        ep.credential.is_none(),
        "a self-hosted deployment needs no vendor key"
    );
    assert!(ep.serves("audio_transcription"));
}

/// The minimal case pins what is genuinely required. If this stops parsing, budapp has started
/// depending on a field WaaV treats as optional — or vice versa.
#[test]
fn the_minimal_case_needs_only_vendor_and_capabilities() {
    let entry = cases()
        .get("minimal")
        .cloned()
        .expect("fixture has a minimal case");
    let blob = serde_json::json!({ "ep-min": entry }).to_string();

    let map = parse_voice_blob(&blob, &CredentialDecryptor::disabled()).expect("parses");
    let ep = map.get("ep-min").expect("endpoint present");

    assert_eq!(ep.vendor, "elevenlabs");
    assert!(ep.serves("text_to_speech"));
    assert!(ep.api_base.is_none());
    assert!(ep.model.is_none());
}

/// The fixture must stay in step with the copy in bud-runtime. This asserts the marker that
/// says so is present, so a well-meaning edit that drops the warning is itself caught.
#[test]
fn the_fixture_still_declares_itself_shared() {
    let v: serde_json::Value = serde_json::from_str(CONTRACT).unwrap();
    let comment = v
        .get("_comment")
        .and_then(|c| c.as_str())
        .unwrap_or_default();
    assert!(
        comment.contains("byte-identical") && comment.contains("budapp"),
        "the shared-contract warning was removed; the next editor will not know there are two copies"
    );
}

/// A capability WaaV does not serve must not be silently accepted as one it does.
#[test]
fn capability_matching_is_exact() {
    let blob = r#"{"ep-1":{"vendor":"deepgram","endpoints":["text_to_speech"]}}"#;
    let map = parse_voice_blob(blob, &CredentialDecryptor::disabled()).unwrap();
    let ep = map.get("ep-1").unwrap();

    assert!(ep.serves("text_to_speech"));
    assert!(!ep.serves("text_to_speech_streaming"));
    assert!(!ep.serves("TEXT_TO_SPEECH"));
    assert!(!ep.serves(""));
}

/// The runtime deliberately **accepts** an unknown field and logs it, rather than dropping the
/// endpoint the way budgateway's `deny_unknown_fields` would. That tolerance is right for
/// production and useless as a drift detector, so the contract test does the detecting: every
/// field in the fixture must be one this build actually models.
///
/// Without this, budapp could add a field, its own contract test would go red, and WaaV's would
/// stay green — leaving one half of a two-sided guard.
#[test]
fn no_contract_field_is_unmodelled_by_this_build() {
    // Kept in step with `VoiceEndpointBlob` and the `KNOWN` list in `credentials.rs`.
    const MODELLED: &[&str] = &[
        "vendor",
        "api_base",
        "credential",
        "endpoints",
        "model",
        "voice",
        "language",
        "pricing",
        "config",
        // Voice contract §3 (2026-09-26).
        "provider_params",
        // FRD-022 §6.1: the deployment's Rate limiting and Resilience settings.
        "rate_limits",
        "max_concurrent",
        "retry_config",
        "fallback_models",
    ];

    for (name, entry) in cases() {
        let fields = entry
            .as_object()
            .unwrap_or_else(|| panic!("case {name} is not an object"));
        let unknown: Vec<&String> = fields
            .keys()
            .filter(|k| !MODELLED.contains(&k.as_str()))
            .collect();
        assert!(
            unknown.is_empty(),
            "case {name} carries field(s) {unknown:?} that this build does not model. \
             budapp has added something WaaV ignores — add it to VoiceEndpointBlob and the \
             KNOWN list in credentials.rs, or remove it from the shared fixture."
        );
    }
}

/// FRD-022 §6.1 / TC-CT-01: the deployment policy budapp publishes reaches the endpoint intact.
///
/// The fixture's credential is a placeholder, so it is stripped here: this checks the SHAPE of
/// the policy blocks, which is independent of the credential.
#[test]
fn the_policy_case_carries_its_deployment_policy() {
    let mut entry = cases()
        .remove("with_deployment_policy")
        .expect("the shared fixture has a policy case");
    entry.as_object_mut().unwrap().remove("credential");
    let blob = serde_json::json!({ "ep-1": entry }).to_string();
    let map = parse_voice_blob(&blob, &CredentialDecryptor::disabled()).expect("parses");
    let ep = map.get("ep-1").expect("endpoint kept");
    let p = &ep.policy;
    let rl = p.rate_limits.as_ref().expect("rate_limits parsed");
    assert_eq!(rl.algorithm, resil::RateLimitAlgorithm::TokenBucket);
    assert_eq!(rl.requests_per_second, Some(10));
    assert_eq!(rl.burst_size, Some(15));
    assert_eq!(rl.cache_ttl_ms, 500);
    assert_eq!(rl.local_allowance, 0.8);
    assert_eq!(p.max_concurrent, Some(20));
    let retry = p.retry_config.expect("retry_config parsed");
    assert_eq!(retry.num_retries, 2);
    assert_eq!(retry.max_delay_s, 5.0);
    assert_eq!(p.fallback_models.len(), 2);
    assert_eq!(
        &*p.fallback_models[0],
        "3f2c1a9e-7b4d-4e61-9a0f-2d5c8b7e6a14"
    );
}

/// An entry without policy fields (every entry published before FRD-022) has no policy.
#[test]
fn entries_without_policy_have_none() {
    let entry = cases().remove("minimal").expect("minimal case");
    let blob = serde_json::json!({ "ep-1": entry }).to_string();
    let map = parse_voice_blob(&blob, &CredentialDecryptor::disabled()).expect("parses");
    assert!(map["ep-1"].policy.is_empty());
}
