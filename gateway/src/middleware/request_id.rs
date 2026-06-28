//! Request-id / trace-context correlation middleware (W-C1 / E13).
//!
//! Every request must carry a propagated correlation id so the 280+ existing `tracing` calls can
//! be tied together per request, and so a client/operator can correlate a response with logs.
//!
//! This middleware:
//! 1. **reads or mints** a correlation id — it honors an inbound `x-request-id`, else derives one
//!    from a W3C `traceparent` (`00-<trace-id>-<span-id>-01` → uses the 32-hex trace-id), else
//!    mints a fresh UUIDv4;
//! 2. **enters a `tracing::Span`** carrying `request_id` for the duration of the handler, so all
//!    nested `tracing` events inherit the field automatically (no per-call plumbing);
//! 3. **echoes** the id back on the response `x-request-id` header;
//! 4. **stashes** the id in a request extension ([`RequestId`]) so handlers that make outbound
//!    provider calls can forward it on provider request headers.
//!
//! It is mounted as the outermost layer (in `main.rs`) so it wraps auth, rate-limit, and the
//! handlers — the id exists before anything else logs.

use axum::{
    extract::Request,
    http::{HeaderName, HeaderValue},
    middleware::Next,
    response::Response,
};
use tracing::Instrument;

/// Canonical correlation-id header.
pub const REQUEST_ID_HEADER: &str = "x-request-id";
/// W3C trace-context header we derive an id from when `x-request-id` is absent.
pub const TRACEPARENT_HEADER: &str = "traceparent";

/// The resolved correlation id, stored as a request extension so handlers can forward it to
/// outbound provider requests (`req.headers_mut().insert("x-request-id", id)`).
#[derive(Debug, Clone)]
pub struct RequestId(pub String);

impl RequestId {
    /// The id as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Extract a valid `x-request-id` from the request, if present and ASCII-clean.
fn inbound_request_id(req: &Request) -> Option<String> {
    req.headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.len() <= 200 && s.is_ascii())
        .map(str::to_string)
}

/// Derive an id from a W3C `traceparent` header (`version-traceid-spanid-flags`).
/// Returns the 32-hex trace-id portion when well-formed.
fn traceparent_trace_id(req: &Request) -> Option<String> {
    let tp = req
        .headers()
        .get(TRACEPARENT_HEADER)
        .and_then(|v| v.to_str().ok())?;
    let parts: Vec<&str> = tp.split('-').collect();
    // version(2) - trace-id(32) - parent-id(16) - flags(2)
    if parts.len() == 4
        && parts[1].len() == 32
        && parts[1].chars().all(|c| c.is_ascii_hexdigit())
        && parts[1] != "0".repeat(32)
    {
        Some(parts[1].to_string())
    } else {
        None
    }
}

/// Resolve the correlation id: inbound `x-request-id` → `traceparent` trace-id → fresh UUIDv4.
fn resolve_request_id(req: &Request) -> String {
    inbound_request_id(req)
        .or_else(|| traceparent_trace_id(req))
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
}

/// **Mint or propagate a W3C `traceparent`** the gateway forwards on the WaaV Infer handshake (GW-17).
///
/// - If `inbound_trace_id` is a valid 32-hex, non-all-zero trace id (e.g. the request's own correlation id
///   derived from an inbound `traceparent`), it is REUSED — so one distributed trace spans the inbound
///   caller, the gateway, and Infer.
/// - Otherwise a fresh 128-bit trace id is minted (from a UUIDv4).
///
/// A fresh non-zero 64-bit span id (the gateway's current span) is always minted, and the `sampled` flag
/// is set. The result is the canonical `00-<trace32>-<span16>-01` string the Infer `SessionConfig::trace`
/// (a W3C traceparent) deserializes — so the engine parents its per-turn / per-stage spans under it.
pub fn mint_traceparent(inbound_trace_id: Option<&str>) -> String {
    let trace = inbound_trace_id
        .map(str::trim)
        .filter(|id| is_hex32_nonzero(id))
        .map(|id| id.to_ascii_lowercase())
        .unwrap_or_else(|| hex_of(uuid::Uuid::new_v4().as_bytes()));
    // A fresh, non-zero 64-bit span id (the first 8 bytes of a UUIDv4, forced non-zero so the W3C
    // all-zero-span rejection never trips).
    let mut span = [0u8; 8];
    span.copy_from_slice(&uuid::Uuid::new_v4().as_bytes()[..8]);
    span[7] |= 1;
    format!("00-{trace}-{}-01", hex_of(&span))
}

/// Whether `s` is a well-formed W3C `traceparent` the Infer engine will accept (4 dash fields:
/// `2hex-32hex-16hex-2hex`, neither id all-zero). The gateway validates before injecting so a malformed
/// value can never make the engine's `session.config` deserialization fail (the trace field is typed).
pub fn is_w3c_traceparent(s: &str) -> bool {
    let parts: Vec<&str> = s.trim().split('-').collect();
    parts.len() == 4
        && parts[0].len() == 2
        && parts[0].chars().all(|c| c.is_ascii_hexdigit())
        && is_hex32_nonzero(parts[1])
        && is_hex_nonzero(parts[2], 16)
        && parts[3].len() == 2
        && parts[3].chars().all(|c| c.is_ascii_hexdigit())
}

/// A 32-hex (128-bit), non-all-zero id (the W3C trace-id shape).
fn is_hex32_nonzero(s: &str) -> bool {
    is_hex_nonzero(s, 32)
}

/// A `len`-char lower/upper hex string that is not all-zero (W3C ids must be non-zero).
fn is_hex_nonzero(s: &str, len: usize) -> bool {
    s.len() == len && s.chars().all(|c| c.is_ascii_hexdigit()) && s.chars().any(|c| c != '0')
}

/// Lower-case hex of a byte slice (no external `hex` dep).
fn hex_of(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(char::from_digit((b >> 4) as u32, 16).unwrap());
        out.push(char::from_digit((b & 0x0f) as u32, 16).unwrap());
    }
    out
}

/// Axum middleware: resolve + propagate the correlation id, run the handler inside a span that
/// carries `request_id`, and echo the id on the response.
pub async fn request_id_middleware(mut req: Request, next: Next) -> Response {
    let request_id = resolve_request_id(&req);

    // Make the id available to handlers (for outbound provider headers) and ensure the inbound
    // request carries a normalized `x-request-id` (so downstream extractors see it too).
    req.extensions_mut().insert(RequestId(request_id.clone()));
    if let Ok(hv) = HeaderValue::from_str(&request_id) {
        req.headers_mut()
            .insert(HeaderName::from_static(REQUEST_ID_HEADER), hv);
    }

    // Enter a span so every nested `tracing` event inherits `request_id`.
    let span = tracing::info_span!("request", request_id = %request_id);

    let mut response = next.run(req).instrument(span).await;

    // Echo the id back to the caller.
    if let Ok(hv) = HeaderValue::from_str(&request_id) {
        response
            .headers_mut()
            .insert(HeaderName::from_static(REQUEST_ID_HEADER), hv);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;

    fn req_with(headers: &[(&'static str, &str)]) -> Request {
        let mut b = Request::builder().uri("/");
        for (k, v) in headers {
            b = b.header(*k, *v);
        }
        b.body(Body::empty()).unwrap()
    }

    #[test]
    fn honors_inbound_request_id() {
        let req = req_with(&[(REQUEST_ID_HEADER, "abc-123")]);
        assert_eq!(resolve_request_id(&req), "abc-123");
    }

    #[test]
    fn derives_from_traceparent() {
        let tp = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
        let req = req_with(&[(TRACEPARENT_HEADER, tp)]);
        assert_eq!(
            resolve_request_id(&req),
            "4bf92f3577b34da6a3ce929d0e0e4736"
        );
    }

    #[test]
    fn mints_uuid_when_absent() {
        let req = req_with(&[]);
        let id = resolve_request_id(&req);
        // UUIDv4 string form is 36 chars with hyphens.
        assert_eq!(id.len(), 36);
        assert_eq!(id.matches('-').count(), 4);
    }

    #[test]
    fn ignores_blank_request_id_and_mints() {
        let req = req_with(&[(REQUEST_ID_HEADER, "   ")]);
        let id = resolve_request_id(&req);
        assert_eq!(id.len(), 36, "blank inbound id is ignored, fresh uuid minted");
    }

    #[test]
    fn ignores_all_zero_traceparent() {
        let tp = "00-00000000000000000000000000000000-00f067aa0ba902b7-01";
        let req = req_with(&[(TRACEPARENT_HEADER, tp)]);
        let id = resolve_request_id(&req);
        assert_eq!(id.len(), 36, "all-zero trace-id is invalid; mint instead");
    }

    #[test]
    fn mint_traceparent_reuses_inbound_trace_id() {
        // A valid inbound 32-hex trace id is reused (one trace spans inbound → gateway → Infer).
        let inbound = "4bf92f3577b34da6a3ce929d0e0e4736";
        let tp = mint_traceparent(Some(inbound));
        assert!(is_w3c_traceparent(&tp), "minted a well-formed traceparent: {tp}");
        assert!(tp.starts_with(&format!("00-{inbound}-")), "reused the inbound trace id: {tp}");
        assert!(tp.ends_with("-01"), "sampled flag set");
        // The span id is fresh + non-zero (never the all-zero the engine rejects).
        let span = tp.split('-').nth(2).unwrap();
        assert_eq!(span.len(), 16);
        assert!(span.chars().any(|c| c != '0'), "span id is non-zero");
    }

    #[test]
    fn mint_traceparent_mints_fresh_when_absent_or_invalid() {
        for inbound in [None, Some("not-hex"), Some("abc-123"), Some(&*"0".repeat(32))] {
            let tp = mint_traceparent(inbound);
            assert!(is_w3c_traceparent(&tp), "fresh minted traceparent is valid: {tp} (from {inbound:?})");
        }
        // Two fresh mints differ (a real 128-bit id, not a constant).
        assert_ne!(mint_traceparent(None), mint_traceparent(None));
    }

    #[test]
    fn is_w3c_traceparent_rejects_malformed() {
        assert!(is_w3c_traceparent("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"));
        for bad in [
            "",
            "00-4bf9",                                                  // too few fields
            "00-4bf92f3577b34da6a3ce929d0e0e47-00f067aa0ba902b7-01",    // trace too short
            "00-00000000000000000000000000000000-00f067aa0ba902b7-01",  // all-zero trace
            "00-4bf92f3577b34da6a3ce929d0e0e4736-0000000000000000-01",  // all-zero span
            "00-zzf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",  // non-hex
        ] {
            assert!(!is_w3c_traceparent(bad), "must reject `{bad}`");
        }
    }
}
