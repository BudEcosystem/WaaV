//! The one sentence a vendor's error body is trying to say.
//!
//! Every vendor WaaV calls reports failure in its own JSON shape, and handing the raw body to the
//! caller buries the fact they need under the vendor's envelope. Both the batch STT driver and the
//! shared HTTP TTS provider render vendor refusals, so the shapes live here once rather than
//! drifting apart in two copies.

/// Extract the human-readable message from a vendor error body.
///
/// `None` when the body is not JSON or matches no known shape — the caller then shows the raw
/// body, which is still the most information available. Never swallows it.
///
/// Shapes, in the order they are tried:
///
/// * **FastAPI validation list** — `{"detail": [{"loc": [...], "msg": "..."}]}`, rendered as
///   `loc.path: msg` joined with `; `. ElevenLabs' request validation.
/// * **Object under `detail`** — `{"detail": {"code": "...", "message": "..."}}`. ElevenLabs'
///   domain errors (`voice_not_found`, `model_not_found`).
/// * **A string under one of** `detail`, `err_msg` (Deepgram), `error` (AssemblyAI), `message`,
///   `reason`.
/// * **Object under `error`** — `{"error": {"message": "..."}}`. OpenAI and everything that copies
///   its envelope.
pub fn vendor_message(body: &str) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(body).ok()?;

    if let Some(items) = parsed.get("detail").and_then(|d| d.as_array()) {
        let rendered: Vec<String> = items
            .iter()
            .map(|d| {
                let field = d
                    .get("loc")
                    .and_then(|l| l.as_array())
                    .map(|l| {
                        l.iter()
                            .filter_map(|v| v.as_str())
                            .collect::<Vec<_>>()
                            .join(".")
                    })
                    .unwrap_or_default();
                let msg = d.get("msg").and_then(|m| m.as_str()).unwrap_or_default();
                if field.is_empty() {
                    msg.to_string()
                } else {
                    format!("{field}: {msg}")
                }
            })
            .filter(|s| !s.is_empty())
            .collect();
        if !rendered.is_empty() {
            return Some(rendered.join("; "));
        }
    }

    for envelope in ["detail", "error"] {
        if let Some(msg) = parsed
            .get(envelope)
            .and_then(|d| d.get("message").or_else(|| d.get("msg")))
            .and_then(|m| m.as_str())
            .filter(|m| !m.is_empty())
        {
            return Some(msg.to_string());
        }
    }

    for key in ["detail", "err_msg", "error", "message", "reason"] {
        if let Some(msg) = parsed
            .get(key)
            .and_then(|v| v.as_str())
            .filter(|m| !m.is_empty())
        {
            return Some(msg.to_string());
        }
    }
    None
}

/// The vendor's machine-readable error code, when its body carries one.
///
/// `detail.code` (ElevenLabs), then `detail.status`, then `error.code` (OpenAI). Used where the
/// STATUS alone cannot say who can fix the failure — a 403 is a bad key or a plan limit.
pub fn vendor_code(body: &str) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(body).ok()?;
    [("detail", "code"), ("detail", "status"), ("error", "code")]
        .iter()
        .find_map(|(outer, inner)| {
            parsed
                .get(*outer)
                .and_then(|o| o.get(*inner))
                .and_then(|c| c.as_str())
                .filter(|c| !c.is_empty())
                .map(str::to_string)
        })
}

#[cfg(test)]
mod tests {
    use super::{vendor_code, vendor_message};

    #[test]
    fn the_code_is_read_from_each_envelope() {
        assert_eq!(
            vendor_code(r#"{"detail":{"code":"subscription_required","message":"m"}}"#).as_deref(),
            Some("subscription_required")
        );
        assert_eq!(
            vendor_code(r#"{"detail":{"status":"quota_exceeded"}}"#).as_deref(),
            Some("quota_exceeded")
        );
        assert_eq!(
            vendor_code(r#"{"error":{"code":"invalid_api_key"}}"#).as_deref(),
            Some("invalid_api_key")
        );
        assert_eq!(vendor_code("not json"), None);
    }

    #[test]
    fn elevenlabs_domain_error_yields_its_message() {
        // Verbatim from a live `POST /v1/text-to-speech/alloy` against eleven_v3.
        let body = r#"{"detail":{"type":"not_found","code":"voice_not_found","message":"A voice with voice_id 'alloy' was not found.","status":"voice_not_found","request_id":"c65887ada9aa0a2e4f87cf2c24bb6631"}}"#;
        assert_eq!(
            vendor_message(body).as_deref(),
            Some("A voice with voice_id 'alloy' was not found.")
        );
    }

    #[test]
    fn fastapi_validation_list_names_each_field() {
        let body = r#"{"detail":[{"loc":["body","model_id"],"msg":"unknown model"},{"loc":["body"],"msg":"bad"}]}"#;
        assert_eq!(
            vendor_message(body).as_deref(),
            Some("body.model_id: unknown model; body: bad")
        );
    }

    #[test]
    fn openai_envelope_yields_its_message() {
        let body = r#"{"error":{"message":"Invalid voice","type":"invalid_request_error"}}"#;
        assert_eq!(vendor_message(body).as_deref(), Some("Invalid voice"));
    }

    #[test]
    fn single_string_shapes() {
        assert_eq!(vendor_message(r#"{"err_msg":"dg"}"#).as_deref(), Some("dg"));
        assert_eq!(vendor_message(r#"{"error":"aai"}"#).as_deref(), Some("aai"));
        assert_eq!(vendor_message(r#"{"detail":"d"}"#).as_deref(), Some("d"));
        assert_eq!(vendor_message(r#"{"reason":"r"}"#).as_deref(), Some("r"));
    }

    #[test]
    fn unrecognised_bodies_are_none_so_the_caller_keeps_the_raw_text() {
        assert_eq!(vendor_message("upstream exploded"), None);
        assert_eq!(vendor_message(r#"{"status":"failed"}"#), None);
        assert_eq!(vendor_message(r#"{"message":""}"#), None);
    }
}
