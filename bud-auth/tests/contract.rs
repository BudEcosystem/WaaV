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
            "vendor_backed" => {
                let map = parsed.expect("vendor_backed must parse");
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
