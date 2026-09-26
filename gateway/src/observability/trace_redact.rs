//! Trace body/content hardening helpers — the platform's shared content-capture contract
//! (FRD-021 §6.8, GT-24), ported from budgateway's `tensorzero-internal/src/trace_redact.rs`.
//!
//! WaaV records a voice call's request and response bodies on the root SERVER span exactly when
//! the platform records LLM prompts and completions, under the same switch, size cap and
//! redaction. The env var names and defaults are the ones budgateway, budevent and budprompt
//! honour; do not diverge them without updating all four.
//!
//! 1. [`capture_content`] — master opt-out (`BUD_TRACE_CAPTURE_CONTENT`, default `true`). When
//!    `false`, callers set NO content-bearing attribute; ids, sizes, status, error class and every
//!    `bud.voice.*` metric attribute are unaffected.
//! 2. [`truncate_body`] — UTF-8-safe byte cap (`BUD_TRACE_BODY_MAX_BYTES`, default 32768), applied
//!    to every captured body. `opentelemetry_sdk` 0.29 `SpanLimits` has no attribute-length limit,
//!    so this app-side cap is the only backstop against a span the exporter drops.
//! 3. [`redact_secrets`] — best-effort JSON secret-key masking, always on.
//!
//! Audio is never a body here: a TTS response is not captured at all, and an STT upload is
//! described by its form fields and file metadata, never its bytes.

use std::sync::OnceLock;

/// Env var name: master opt-in/opt-out for capturing content-bearing trace attributes.
const CAPTURE_CONTENT_ENV: &str = "BUD_TRACE_CAPTURE_CONTENT";

/// Env var name: maximum number of BYTES of any single captured body attribute value.
const BODY_MAX_BYTES_ENV: &str = "BUD_TRACE_BODY_MAX_BYTES";

/// Default body byte cap (32 KiB) — matches the cross-service contract.
const DEFAULT_BODY_MAX_BYTES: usize = 32_768;

/// Appended when [`truncate_body`] actually truncates, so a reader can tell the value was
/// clipped. NOT counted against the byte cap.
const TRUNCATION_SUFFIX: &str = "...[truncated]";

/// Whether content-bearing trace attributes should be captured.
///
/// Reads `BUD_TRACE_CAPTURE_CONTENT` ONCE (cached) — default `true`. `false`/`0`/`no`/`off`
/// (case-insensitive) disable capture; anything else, including unset, keeps the default.
pub fn capture_content() -> bool {
    static CAPTURE: OnceLock<bool> = OnceLock::new();
    *CAPTURE.get_or_init(|| capture_flag(std::env::var(CAPTURE_CONTENT_ENV).ok().as_deref()))
}

/// The pure parse behind [`capture_content`].
fn capture_flag(raw: Option<&str>) -> bool {
    match raw {
        Some(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "false" | "0" | "no" | "off"
        ),
        None => true,
    }
}

/// The configured body byte cap, read ONCE from `BUD_TRACE_BODY_MAX_BYTES` (cached). Falls back
/// to [`DEFAULT_BODY_MAX_BYTES`] when unset, empty, or unparseable.
fn body_max_bytes() -> usize {
    static MAX: OnceLock<usize> = OnceLock::new();
    *MAX.get_or_init(|| body_max_flag(std::env::var(BODY_MAX_BYTES_ENV).ok().as_deref()))
}

/// The pure parse behind [`body_max_bytes`].
fn body_max_flag(raw: Option<&str>) -> usize {
    raw.and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(DEFAULT_BODY_MAX_BYTES)
}

/// Largest byte index `<= max` that lies on a UTF-8 char boundary of `s`.
///
/// Stand-in for the unstable `str::floor_char_boundary`: walks back from `max` until a boundary
/// is found (at most 3 bytes for valid UTF-8).
fn floor_char_boundary(s: &str, max: usize) -> usize {
    if max >= s.len() {
        return s.len();
    }
    let mut idx = max;
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

/// Truncate `body` to at most `BUD_TRACE_BODY_MAX_BYTES` bytes on a UTF-8 char boundary,
/// appending `...[truncated]` when it cut anything. Bodies within the cap are returned unchanged.
pub fn truncate_body(body: &str) -> String {
    truncate_body_to(body, body_max_bytes())
}

/// Inner truncation with an explicit byte cap (testable without env state).
fn truncate_body_to(body: &str, max_bytes: usize) -> String {
    if body.len() <= max_bytes {
        return body.to_string();
    }
    let boundary = floor_char_boundary(body, max_bytes);
    let mut out = String::with_capacity(boundary + TRUNCATION_SUFFIX.len());
    out.push_str(&body[..boundary]);
    out.push_str(TRUNCATION_SUFFIX);
    out
}

/// Keys (case-insensitive) whose JSON values are masked by [`redact_secrets`].
const SECRET_KEYS: &[&str] = &[
    "token",
    "secret",
    "password",
    "authorization",
    "api_key",
    "apikey",
    "client_secret",
    "signing_secret",
    "access_token",
    "refresh_token",
];

/// Replacement value for a redacted secret.
const REDACTED: &str = "***";

fn is_secret_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    SECRET_KEYS.iter().any(|k| *k == lower)
}

/// Best-effort secret-key redaction for a captured body, always applied independently of
/// [`capture_content`].
///
/// If `body` parses as JSON, every value under a key matching (case-insensitive) a
/// [`SECRET_KEYS`] entry becomes `"***"`, recursively, and the JSON is re-serialised. A body
/// that is not JSON (an SRT transcript, say) is returned unchanged.
pub fn redact_secrets(body: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(body) {
        Ok(mut value) => {
            redact_value(&mut value);
            serde_json::to_string(&value).unwrap_or_else(|_| body.to_string())
        }
        Err(_) => body.to_string(),
    }
}

/// Recursively mask secret-keyed values in place.
fn redact_value(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, val) in map.iter_mut() {
                if is_secret_key(key) {
                    *val = serde_json::Value::String(REDACTED.to_string());
                } else {
                    redact_value(val);
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items.iter_mut() {
                redact_value(item);
            }
        }
        _ => {}
    }
}

/// Redact secrets, then truncate. Redaction runs first so a secret is masked before any clipping
/// could split it.
pub fn sanitize_body(body: &str) -> String {
    truncate_body(&redact_secrets(body))
}

/// Gate + sanitize in one step: `Some(sanitized)` when `capture` is true, `None` when it is false
/// (the caller then sets NO content attribute). Exists to make the disabled path unit-testable
/// without the process-global env cache.
pub fn capture_json_body(capture: bool, body: &str) -> Option<String> {
    capture.then(|| sanitize_body(body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_respects_byte_cap_ascii() {
        let body = "a".repeat(100);
        let out = truncate_body_to(&body, 10);
        assert!(out.starts_with(&"a".repeat(10)));
        assert!(out.ends_with(TRUNCATION_SUFFIX));
        assert_eq!(out.len(), 10 + TRUNCATION_SUFFIX.len());
    }

    #[test]
    fn truncate_no_op_when_within_cap() {
        let body = "short body";
        assert_eq!(truncate_body_to(body, 32_768), body);
        // Exactly at the cap is NOT truncated.
        assert_eq!(truncate_body_to(body, body.len()), body);
    }

    #[test]
    fn truncate_never_splits_multibyte_char() {
        // "€" is 3 bytes (E2 82 AC). Build a string of euros and cut mid-char.
        let body = "€".repeat(10); // 30 bytes
        for max in 0..body.len() {
            let out = truncate_body_to(&body, max);
            let retained = out.strip_suffix(TRUNCATION_SUFFIX).unwrap_or(&out);
            assert!(retained.chars().all(|c| c == '€'), "max={max} out={out:?}");
            assert!(
                retained.len() <= max,
                "max={max} retained_len={}",
                retained.len()
            );
        }
    }

    #[test]
    fn truncate_boundary_floor_keeps_whole_char() {
        // cap=2 inside a 3-byte char → floor to 0 whole chars retained.
        let body = "€abc";
        let out = truncate_body_to(body, 2);
        assert_eq!(out, TRUNCATION_SUFFIX);
    }

    #[test]
    fn redact_masks_nested_secret_keys_keeps_message_text() {
        let body = r#"{
            "token":"xoxb-123-secret",
            "messages":[{"role":"user","content":"hello world"}],
            "authorizations":[{"client_secret":"cs_live_abc"},{"note":"keep me"}],
            "API_KEY":"sk-should-mask"
        }"#;
        let out = redact_secrets(body);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["token"], "***");
        // `authorizations` (plural) is NOT an exact secret key → recurse into it.
        assert_eq!(v["authorizations"][0]["client_secret"], "***");
        assert_eq!(v["authorizations"][1]["note"], "keep me");
        assert_eq!(v["API_KEY"], "***");
        assert_eq!(v["messages"][0]["content"], "hello world");
        assert_eq!(v["messages"][0]["role"], "user");
    }

    #[test]
    fn redact_non_json_passthrough() {
        let body = "not json: token=abc password=def";
        assert_eq!(redact_secrets(body), body);
    }

    #[test]
    fn redact_all_secret_key_variants() {
        for key in SECRET_KEYS {
            let body = format!(r#"{{"{key}":"sensitive","ok":"visible"}}"#);
            let out = redact_secrets(&body);
            let v: serde_json::Value = serde_json::from_str(&out).unwrap();
            assert_eq!(v[*key], "***", "key {key} not redacted");
            assert_eq!(v["ok"], "visible");
        }
    }

    #[test]
    fn sanitize_redacts_then_truncates() {
        let body = r#"{"password":"super-secret-value","note":"x"}"#;
        let out = sanitize_body(body);
        assert!(out.contains("***"));
        assert!(!out.contains("super-secret-value"));
    }

    #[test]
    fn capture_disabled_sets_no_body() {
        let body = r#"{"model":"tts-a","input":"hi"}"#;
        assert_eq!(capture_json_body(false, body), None);
        let captured = capture_json_body(true, body).expect("body when capture on");
        assert!(captured.contains("hi"));
    }

    #[test]
    fn capture_enabled_still_redacts_and_truncates() {
        let body = r#"{"token":"sk-secret","content":"keep"}"#;
        let out = capture_json_body(true, body).unwrap();
        assert!(out.contains("***"));
        assert!(!out.contains("sk-secret"));
        assert!(out.contains("keep"));
    }

    #[test]
    fn capture_content_default_true_when_unset() {
        // The parse is asserted directly: `capture_content` caches process-wide, so a test that
        // set the variable would leak into every other test in the binary.
        assert!(capture_flag(None));
        assert!(capture_flag(Some("true")));
        assert!(capture_flag(Some("1")));
        assert!(capture_flag(Some("anything")));
        assert!(!capture_flag(Some("false")));
        assert!(!capture_flag(Some("FALSE")));
        assert!(!capture_flag(Some("0")));
        assert!(!capture_flag(Some("off")));
        assert!(!capture_flag(Some(" No ")));
    }

    #[test]
    fn the_body_cap_defaults_and_parses() {
        assert_eq!(body_max_flag(None), DEFAULT_BODY_MAX_BYTES);
        assert_eq!(body_max_flag(Some("")), DEFAULT_BODY_MAX_BYTES);
        assert_eq!(body_max_flag(Some("lots")), DEFAULT_BODY_MAX_BYTES);
        assert_eq!(body_max_flag(Some(" 1024 ")), 1024);
        assert_eq!(body_max_flag(Some("262144")), 262_144);
    }
}
