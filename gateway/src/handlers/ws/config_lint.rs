//! Config-message lint: surface unknown / ignored keys as advisories (P0 gw-enforce).
//!
//! ## Why this exists
//!
//! The `/ws` `config` envelope and its nested blocks (`stt_config`, `tts_config`,
//! `livekit`, `dag_config`, `conversation_config`, …) deserialize with serde's
//! default **lenient** policy: an unknown key (a client typo like
//! `diarizationn`, or a *wrong-nesting* mistake like sending `turn_detection` at
//! the top level instead of under `stt_config`) is **silently dropped**. To a
//! client that looks exactly like success — the feature simply never takes
//! effect. This is the single root-cause behind a whole class of SDK bugs (see
//! `SDK_STANDARDIZATION_PLAN.md` "ENFORCEMENT").
//!
//! We deliberately do **not** add `#[serde(deny_unknown_fields)]`: that would
//! *hard-reject* the message and break forward-compatibility (an older gateway
//! talking to a newer SDK that legitimately sends a not-yet-known key). Instead
//! we keep deserialization lenient and emit a non-fatal [`OutgoingMessage::ConfigWarning`]
//! (`code = "unknown_config_keys"`) naming every ignored key path. Loud, but
//! backward-compatible.
//!
//! ## How it works (schema-derived, not a hardcoded key list)
//!
//! The known schema is whatever the typed [`IncomingMessage::Config`] structs
//! accept. Rather than maintain a parallel hand-written allow-list (which would
//! inevitably drift from the structs), we diff the **raw parsed JSON** against a
//! **re-serialization of the successfully-deserialized value**: any key present
//! in the input but absent from the round-trip was dropped by serde, i.e. it is
//! unknown/ignored. This is correct by construction and free of maintenance.
//!
//! Subtleties handled to avoid false positives:
//! * Fields with `#[serde(skip_serializing_if = …)]` do not re-serialize when at
//!   their sentinel (`Option::None`, empty `Vec`/`Map`). A client that sends such
//!   a field as JSON `null` (or omits it) is a no-op, not a typo — so an input
//!   value of `null` is never reported.
//! * The open `extras` passthrough (`ProviderExtras`, a `#[serde(transparent)]`
//!   `Map`) round-trips *every* key verbatim, so its contents are inherently
//!   identical on both sides and never get flagged — exactly the intended escape
//!   hatch.
//! * The diff only descends into a nested object when **both** sides have an
//!   object at that key, so a type-mismatched value is reported at its own path
//!   rather than spuriously recursing.

use super::messages::{MessageClass, MessageRoute, OutgoingMessage, send_with_policy};
use serde_json::Value;
use tokio::sync::mpsc;

/// Stable machine code for the unknown/ignored-config-key advisory.
pub const UNKNOWN_CONFIG_KEYS_CODE: &str = "unknown_config_keys";

/// Compute the dotted paths of every key that is present in `raw` (the JSON the
/// client actually sent) but absent from `accepted` (the same message
/// re-serialized after a successful typed deserialize) — i.e. the keys serde
/// silently dropped.
///
/// `raw` and `accepted` are expected to be the two serializations of the SAME
/// config message. Paths are dotted (`stt_config.features.diarizationn`,
/// `turn_detection`) and returned sorted + de-duplicated for stable output.
pub fn unknown_config_keys(raw: &Value, accepted: &Value) -> Vec<String> {
    let mut out = Vec::new();
    diff_object(raw, accepted, &mut String::new(), &mut out);
    out.sort();
    out.dedup();
    out
}

/// Recursive lock-step walk. Only objects are compared structurally; for any
/// other value kind there is nothing to diff (an unknown key is, by definition,
/// a *key* in some object).
fn diff_object(raw: &Value, accepted: &Value, prefix: &mut String, out: &mut Vec<String>) {
    let (raw_map, accepted_map) = match (raw, accepted) {
        (Value::Object(r), Value::Object(a)) => (r, a),
        // If the accepted side is not an object here (e.g. the client sent an
        // object where the schema expects a scalar), we cannot meaningfully
        // attribute child keys — the value-level mismatch is the client's
        // concern, not an "unknown key". Stop.
        _ => return,
    };

    for (key, raw_val) in raw_map {
        match accepted_map.get(key) {
            None => {
                // Present in input, gone after the round-trip → serde dropped it.
                // A `null` for a known-but-skip_serializing_if field is the one
                // legitimate way a key vanishes without being a typo; treat it
                // as a no-op rather than a warning.
                if !raw_val.is_null() {
                    out.push(join_path(prefix, key));
                }
            }
            Some(accepted_val) => {
                // Known key. Descend only when both sides are objects so nested
                // typos (e.g. stt_config.features.<typo>) are caught too. The
                // open `extras` map round-trips identically, so this descent is
                // a no-op there.
                if raw_val.is_object() && accepted_val.is_object() {
                    let restore_len = prefix.len();
                    if !prefix.is_empty() {
                        prefix.push('.');
                    }
                    prefix.push_str(key);
                    diff_object(raw_val, accepted_val, prefix, out);
                    prefix.truncate(restore_len);
                }
            }
        }
    }
}

fn join_path(prefix: &str, key: &str) -> String {
    if prefix.is_empty() {
        key.to_string()
    } else {
        format!("{prefix}.{key}")
    }
}

/// Build the human-readable advisory message for a set of ignored key paths.
/// Top-level paths that match a well-known *nested* home are given a targeted
/// "did you mean to nest it?" hint (the exact wrong-nesting SDK bug class).
fn advisory_message(paths: &[String]) -> String {
    let mut msg = format!(
        "{} config key(s) were not recognized and silently ignored: {}. \
         They had no effect — check for a typo or wrong nesting.",
        paths.len(),
        paths.join(", ")
    );
    let hints: Vec<String> = paths.iter().filter_map(|p| nesting_hint(p)).collect();
    if !hints.is_empty() {
        msg.push(' ');
        msg.push_str(&hints.join(" "));
    }
    msg
}

/// Targeted hint for a top-level key that is actually a known *nested* field —
/// the precise mistake (`turn_detection` belongs under `stt_config`,
/// `reasoning_effort` under `conversation_config`, …) the SDKs were making.
fn nesting_hint(path: &str) -> Option<String> {
    // Only single-segment (top-level) keys can be a wrong-nesting mistake.
    if path.contains('.') {
        return None;
    }
    let home = match path {
        "turn_detection" => "stt_config",
        "features" => "stt_config or tts_config",
        "reasoning_effort" | "reasoning_model" | "reasoning_route" | "reasoning_budget_ms"
        | "latency_filler" | "latency_filler_after_ms" | "eager_eot" | "mute_strategy"
        | "barge_in_min_words" | "system_prompt" | "max_llm_calls_per_turn" => {
            "conversation_config"
        }
        "enable_recording" => "livekit",
        _ => return None,
    };
    Some(format!("Did you mean to nest `{path}` under `{home}`?"))
}

/// Diff the raw client JSON against the typed-then-reserialized config and, if
/// any keys were silently dropped, emit a single non-fatal
/// [`OutgoingMessage::ConfigWarning`] naming them. No-op when everything was
/// recognized. Never closes the session.
///
/// `raw` is the `serde_json::Value` parsed from the original `config` text;
/// `incoming` is the successfully-parsed [`super::messages::IncomingMessage`]
/// (only the `Config` variant produces warnings — other variants re-serialize to
/// a disjoint shape and are skipped).
pub async fn warn_unknown_config_keys(
    raw: &Value,
    incoming: &super::messages::IncomingMessage,
    message_tx: &mpsc::Sender<MessageRoute>,
) {
    // Only the config envelope is linted. (Re-serializing a non-Config variant
    // would diff against a different `type`, producing noise.)
    if !matches!(incoming, super::messages::IncomingMessage::Config { .. }) {
        return;
    }
    let accepted = match serde_json::to_value(incoming) {
        Ok(v) => v,
        // Should never happen for an already-deserialized value; if it does,
        // skip the lint rather than fail the session.
        Err(e) => {
            tracing::debug!(error = %e, "config lint: re-serialization failed; skipping");
            return;
        }
    };

    let paths = unknown_config_keys(raw, &accepted);
    if paths.is_empty() {
        return;
    }

    tracing::warn!(
        ignored_keys = ?paths,
        "client config contained unknown/ignored keys (silently dropped by serde)"
    );
    // Make the silent drop observable on /metrics too, mirroring the
    // record_degraded idiom used by the reasoning advisories.
    crate::core::metrics::bridge::record_degraded(UNKNOWN_CONFIG_KEYS_CODE, "config_lint");

    send_with_policy(
        message_tx,
        MessageRoute::Outgoing(OutgoingMessage::ConfigWarning {
            code: UNKNOWN_CONFIG_KEYS_CODE.to_string(),
            message: advisory_message(&paths),
            detail: Some(serde_json::json!({ "ignored_keys": paths })),
        }),
        MessageClass::Critical,
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handlers::ws::messages::IncomingMessage;

    /// Round-trip a config message exactly as the live handler does: parse the
    /// raw text into both an `IncomingMessage` and a `Value`, re-serialize the
    /// typed value, and return the dropped key paths.
    fn lint(json: &str) -> Vec<String> {
        let raw: Value = serde_json::from_str(json).expect("raw json parses");
        let incoming: IncomingMessage = serde_json::from_str(json).expect("typed config parses");
        let accepted = serde_json::to_value(&incoming).expect("re-serialize");
        unknown_config_keys(&raw, &accepted)
    }

    const VALID_CORE: &str = r#"{
        "type": "config",
        "audio": true,
        "stt_config": {"provider":"deepgram","language":"en-US","sample_rate":16000,"channels":1,"punctuation":true},
        "tts_config": {"provider":"deepgram","model":"aura-asteria-en"}
    }"#;

    #[test]
    fn correct_config_yields_no_warnings() {
        assert!(lint(VALID_CORE).is_empty(), "a correct config must lint clean");
    }

    #[test]
    fn correct_config_with_nested_features_and_turn_detection_is_clean() {
        // The *right* way to send the advanced surface: nested under stt_config.
        let json = r#"{
            "type": "config",
            "stt_config": {
                "provider":"deepgram","language":"en-US","sample_rate":16000,"channels":1,"punctuation":true,
                "features": {"diarization": true, "keyterms": ["WaaV"]},
                "turn_detection": {"enabled": true, "eager": true}
            },
            "tts_config": {"provider":"deepgram","model":"aura-asteria-en"},
            "conversation_config": {"base_url":"http://x/v1","model":"qwen2.5","reasoning_effort":"minimal"}
        }"#;
        assert!(lint(json).is_empty(), "correctly-nested config must lint clean: {:?}", lint(json));
    }

    #[test]
    fn bogus_top_level_key_is_flagged() {
        let json = r#"{
            "type": "config",
            "stt_config": {"provider":"deepgram","language":"en-US","sample_rate":16000,"channels":1,"punctuation":true},
            "bogus_top_level_key": 123
        }"#;
        assert_eq!(lint(json), vec!["bogus_top_level_key".to_string()]);
    }

    #[test]
    fn misnested_turn_detection_is_flagged_with_hint() {
        // The exact SDK wrong-nesting bug: turn_detection at the TOP level
        // (its real home is stt_config.turn_detection).
        let json = r#"{
            "type": "config",
            "stt_config": {"provider":"deepgram","language":"en-US","sample_rate":16000,"channels":1,"punctuation":true},
            "tts_config": {"provider":"deepgram","model":"aura-asteria-en"},
            "turn_detection": {"enabled": true}
        }"#;
        let paths = lint(json);
        assert_eq!(paths, vec!["turn_detection".to_string()]);
        // And the operator gets a targeted nesting hint.
        let hint = nesting_hint("turn_detection").expect("turn_detection has a nesting hint");
        assert!(hint.contains("stt_config"), "hint should point at stt_config: {hint}");
    }

    #[test]
    fn both_bogus_and_misnested_are_reported_together() {
        let json = r#"{
            "type": "config",
            "stt_config": {"provider":"deepgram","language":"en-US","sample_rate":16000,"channels":1,"punctuation":true},
            "turn_detection": {"enabled": true},
            "reasoning_effort": "minimal",
            "bogus_top_level_key": true
        }"#;
        let paths = lint(json);
        assert_eq!(
            paths,
            vec![
                "bogus_top_level_key".to_string(),
                "reasoning_effort".to_string(),
                "turn_detection".to_string(),
            ]
        );
        let msg = advisory_message(&paths);
        // Names every key…
        assert!(msg.contains("bogus_top_level_key"));
        assert!(msg.contains("turn_detection"));
        assert!(msg.contains("reasoning_effort"));
        // …and gives wrong-nesting hints for the two that have a known home.
        assert!(msg.contains("nest `turn_detection` under `stt_config`"));
        assert!(msg.contains("nest `reasoning_effort` under `conversation_config`"));
    }

    #[test]
    fn nested_typo_under_stt_features_is_flagged() {
        // A typo INSIDE a known nested block is caught too (recursive descent).
        let json = r#"{
            "type": "config",
            "stt_config": {
                "provider":"deepgram","language":"en-US","sample_rate":16000,"channels":1,"punctuation":true,
                "features": {"diarizationn": true}
            }
        }"#;
        assert_eq!(lint(json), vec!["stt_config.features.diarizationn".to_string()]);
    }

    #[test]
    fn extras_passthrough_is_never_flagged() {
        // The open `extras` map legitimately carries arbitrary provider keys —
        // those must NOT be reported as unknown.
        let json = r#"{
            "type": "config",
            "stt_config": {
                "provider":"deepgram","language":"en-US","sample_rate":16000,"channels":1,"punctuation":true,
                "extras": {"some_brand_new_provider_knob": "x", "nested": {"deep": 1}}
            },
            "tts_config": {"provider":"elevenlabs","model":"eleven_multilingual_v2","extras":{"seed":42}}
        }"#;
        assert!(lint(json).is_empty(), "extras keys must never be flagged: {:?}", lint(json));
    }

    #[test]
    fn explicit_null_for_skipped_field_is_not_flagged() {
        // A known field with a `skip_serializing_if = Option::is_none` attribute
        // sent as JSON null deserializes to None and does not re-serialize. That
        // is a no-op, NOT a typo — it must not warn (else every client that
        // spells out optional fields as null gets spurious noise).
        let json = r#"{
            "type": "config",
            "stt_config": {"provider":"deepgram","language":"en-US","sample_rate":16000,"channels":1,"punctuation":true,"api_key": null},
            "conversation_config": {"base_url":"http://x/v1","model":"m","reasoning_model": null, "system_prompt": null}
        }"#;
        assert!(lint(json).is_empty(), "explicit nulls must not be flagged: {:?}", lint(json));
    }

    #[test]
    fn deprecated_audio_disabled_alias_is_not_flagged() {
        // `audio_disabled` is a real (deprecated) field; it round-trips and must
        // not be reported.
        let json = r#"{
            "type": "config",
            "audio_disabled": true,
            "stt_config": {"provider":"deepgram","language":"en-US","sample_rate":16000,"channels":1,"punctuation":true}
        }"#;
        assert!(lint(json).is_empty(), "deprecated audio_disabled must not be flagged: {:?}", lint(json));
    }
}
