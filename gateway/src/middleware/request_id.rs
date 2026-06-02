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
}
