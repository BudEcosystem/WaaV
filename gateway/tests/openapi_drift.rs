//! OpenAPI drift gate (plan W-K1 / exit E9).
//!
//! The hand-written TypeScript/Python SDKs historically drifted from the WaaV wire contract
//! (e.g. `punctuate` vs `punctuation`, `transcript` vs `text`) because nothing tied the SDK
//! field names to the server's actual message structs. The structural fix is to make the
//! OpenAPI spec — generated *from* the wire structs via `utoipa::ToSchema` — the single source
//! of truth, and to commit it (`docs/openapi.yaml`) so SDKs are generated from it.
//!
//! This test is the CI gate that keeps the committed spec honest: it regenerates the spec
//! **in-memory** from the live Rust types and asserts byte-for-byte equality with the committed
//! `docs/openapi.yaml`. Any change to a WS wire struct (a renamed field, a new variant, a new
//! `features` knob) that lands without regenerating the spec fails this test — so wire drift can
//! no longer ship silently.
//!
//! Regenerating after an intentional wire change:
//!   BLESS_OPENAPI=1 cargo test --features openapi --test openapi_drift
//! (or `cargo run --features openapi -- openapi -f yaml -o docs/openapi.yaml`).
//!
//! The whole file is gated on the `openapi` feature so the default test build is unaffected.
#![cfg(feature = "openapi")]

use std::path::PathBuf;

/// Absolute path to the committed spec, relative to the crate manifest.
fn committed_spec_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs/openapi.yaml")
}

/// Regenerate the OpenAPI YAML from the live wire structs (same code path as the CLI exporter
/// and the committed artifact).
fn regenerate_spec() -> String {
    waav_gateway::docs::openapi::spec_yaml().expect("OpenAPI YAML generation must succeed")
}

#[test]
fn openapi_spec_matches_committed_artifact() {
    let path = committed_spec_path();
    let generated = regenerate_spec();

    // Opt-in regeneration: rewrite the committed artifact instead of asserting. This lets an
    // intentional wire change re-bless the spec in one command (it is NOT the default, so CI
    // never auto-blesses).
    if std::env::var_os("BLESS_OPENAPI").is_some() {
        std::fs::write(&path, &generated)
            .unwrap_or_else(|e| panic!("failed to write {}: {e}", path.display()));
        eprintln!("blessed {}", path.display());
        return;
    }

    let committed = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "committed OpenAPI spec missing at {} ({e}). Generate it with: \
             cargo run --features openapi -- openapi -f yaml -o docs/openapi.yaml",
            path.display()
        )
    });

    if committed != generated {
        // Surface a compact, actionable diff summary rather than dumping two 2.5k-line files.
        let committed_lines: Vec<&str> = committed.lines().collect();
        let generated_lines: Vec<&str> = generated.lines().collect();
        let mut first_diff = None;
        for (i, (c, g)) in committed_lines
            .iter()
            .zip(generated_lines.iter())
            .enumerate()
        {
            if c != g {
                first_diff = Some((i + 1, *c, *g));
                break;
            }
        }
        let detail = match first_diff {
            Some((line, c, g)) => {
                format!("first divergence at line {line}:\n  committed: {c}\n  generated: {g}")
            }
            None => format!(
                "line counts differ (committed {} vs generated {})",
                committed_lines.len(),
                generated_lines.len()
            ),
        };
        panic!(
            "OpenAPI drift detected: the wire structs changed but docs/openapi.yaml was not \
             regenerated.\n{detail}\n\nRegenerate with:\n  \
             cargo run --features openapi -- openapi -f yaml -o docs/openapi.yaml\n  (or \
             BLESS_OPENAPI=1 cargo test --features openapi --test openapi_drift)"
        );
    }
}

#[test]
fn openapi_spec_covers_the_websocket_wire_contract() {
    // Coverage guard: the spec must contain every wire struct SDKs are generated from. If a new
    // WS message/config type is added but not registered in `components(schemas(...))`, the SDK
    // would silently lack it — so assert their schema names are present.
    let spec = regenerate_spec();
    for required in [
        // WS message envelopes
        "IncomingMessage:",
        "OutgoingMessage:",
        "UnifiedMessage:",
        // WS config structs
        "STTWebSocketConfig:",
        "TTSWebSocketConfig:",
        "LiveKitWebSocketConfig:",
        "DAGWebSocketConfig:",
        "ConversationWebSocketConfig:",
        // standardized advanced-feature vocabulary
        "SttFeatures:",
        "TtsFeatures:",
        "ProviderExtras:",
    ] {
        assert!(
            spec.contains(required),
            "OpenAPI spec is missing schema `{required}` — register it in \
             src/docs/openapi.rs `components(schemas(...))`"
        );
    }

    // The protocol-version pin must be emitted in the `ready` envelope so SDKs can assert it.
    assert!(
        spec.contains("protocol_version"),
        "OpenAPI spec must document the `ready` envelope's protocol_version field"
    );
}

#[test]
fn protocol_version_pin_is_stable() {
    // The compiled-in protocol version is the SDK contract anchor. A change here is a deliberate
    // wire-protocol bump and must travel with a regenerated spec; this asserts the value the
    // committed spec was generated against.
    assert_eq!(
        waav_gateway::handlers::ws::messages::PROTOCOL_VERSION,
        "1.0",
        "PROTOCOL_VERSION changed; regenerate docs/openapi.yaml and bump SDK expectations"
    );
}
