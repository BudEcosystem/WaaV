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
//!
//! **Which span** (FRD-021 §6.7) depends on the route:
//!
//! * `/`, `/ready`, `/livez`, `/readyz`, `/metrics` — none. The id is still resolved, stashed and
//!   echoed; only the exported span goes. Before, every kubelet probe and every scrape exported a
//!   single-span trace: 99.5% of WaaV's traces, ~68k a day, none of them a call.
//! * `/v1/audio/speech|transcriptions|translations` — an HTTP SERVER span named
//!   `POST /v1/audio/speech` (etc.), carrying the OTel HTTP conventions, the call's attribution
//!   and its bodies, so a voice call is listed in the trace UI like an LLM call. Switched off by
//!   `WAAV_HTTP_SERVER_SPAN=false` (the deploy-order escape of the FRD-021 plan).
//! * everything else — today's INTERNAL `request` span, unchanged (NG-10).

use axum::{
    extract::Request,
    http::{HeaderName, HeaderValue},
    middleware::Next,
    response::Response,
};
use tracing::Instrument;

use crate::observability::voice_span::{self, RootSpan};

/// Env var: whether `/v1/audio/*` requests get the HTTP SERVER root span. Default on.
pub const HTTP_SERVER_SPAN_ENV: &str = "WAAV_HTTP_SERVER_SPAN";

/// The operability routes: the kubelet's probes and Prometheus' scrape. No span.
const PROBE_PATHS: &[&str] = &["/", "/ready", "/livez", "/readyz", "/metrics"];

/// The routes whose root is the HTTP SERVER span. Each is its own route template (none takes a
/// path parameter), so the path is the route.
const AUDIO_ROUTES: &[&str] = &[
    "/v1/audio/speech",
    "/v1/audio/transcriptions",
    "/v1/audio/translations",
];

/// What a request's root span is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RootKind {
    /// No span at all.
    Probe,
    /// The HTTP SERVER span, for this route template.
    Server(&'static str),
    /// The INTERNAL `request` span every route had before FRD-021.
    Internal,
}

fn root_kind(path: &str, server_span_enabled: bool) -> RootKind {
    if PROBE_PATHS.contains(&path) {
        return RootKind::Probe;
    }
    match AUDIO_ROUTES.iter().copied().find(|r| *r == path) {
        Some(route) if server_span_enabled => RootKind::Server(route),
        _ => RootKind::Internal,
    }
}

/// Whether the HTTP SERVER root is on: read once; `false`/`0`/`no`/`off` disable it.
pub fn http_server_span_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| server_span_flag(std::env::var(HTTP_SERVER_SPAN_ENV).ok().as_deref()))
}

/// The pure parse behind [`http_server_span_enabled`].
fn server_span_flag(raw: Option<&str>) -> bool {
    !matches!(
        raw.map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("false" | "0" | "no" | "off")
    )
}

/// The scheme the CLIENT used: the proxy's `x-forwarded-proto` when it is one we recognise,
/// else the request URI's, else `http` — WaaV itself listens in plain HTTP behind the ingress.
fn request_scheme(req: &Request) -> String {
    req.headers()
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(|v| v.trim().to_ascii_lowercase())
        .filter(|v| v == "http" || v == "https")
        .or_else(|| req.uri().scheme_str().map(str::to_string))
        .unwrap_or_else(|| "http".to_string())
}

/// The host the client addressed, without its port (OTel `server.address`).
fn server_address(req: &Request) -> Option<String> {
    let host = req
        .headers()
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .or_else(|| req.uri().host().map(str::to_string))?;
    let host = host.trim();
    let bare = if host.starts_with('[') {
        // An IPv6 literal keeps its brackets; only a port after them is dropped.
        host.split_inclusive(']').next().unwrap_or(host)
    } else {
        host.split(':').next().unwrap_or(host)
    };
    Some(bare.to_string()).filter(|h| !h.is_empty() && h.len() <= 255)
}

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
    const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX_DIGITS[(b >> 4) as usize] as char);
        out.push(HEX_DIGITS[(b & 0x0f) as usize] as char);
    }
    out
}

/// Axum middleware: resolve + propagate the correlation id, run the handler inside the route's
/// root span (see the module docs for which), and echo the id on the response.
pub async fn request_id_middleware(mut req: Request, next: Next) -> Response {
    let request_id = resolve_request_id(&req);

    // Make the id available to handlers (for outbound provider headers) and ensure the inbound
    // request carries a normalized `x-request-id` (so downstream extractors see it too).
    req.extensions_mut().insert(RequestId(request_id.clone()));
    if let Ok(hv) = HeaderValue::from_str(&request_id) {
        req.headers_mut()
            .insert(HeaderName::from_static(REQUEST_ID_HEADER), hv);
    }

    let mut response = match root_kind(req.uri().path(), http_server_span_enabled()) {
        // No span: a probe is not a call, and exporting one per poll buried every call there was.
        RootKind::Probe => next.run(req).await,
        RootKind::Server(route) => {
            let method = req.method().as_str().to_string();
            let path = req.uri().path().to_string();
            let span = voice_span::server_root_span(
                &method,
                route,
                &path,
                &request_scheme(&req),
                &request_id,
            );
            if let Some(agent) = req
                .headers()
                .get(axum::http::header::USER_AGENT)
                .and_then(|v| v.to_str().ok())
                .map(str::trim)
                .filter(|v| !v.is_empty())
            {
                span.record(voice_span::http::USER_AGENT, agent);
            }
            if let Some(host) = server_address(&req) {
                span.record(voice_span::http::SERVER_ADDRESS, host.as_str());
            }
            // The handler records the call's attribution, units and bodies on this same span.
            req.extensions_mut().insert(RootSpan(span.clone()));
            let response = next.run(req).instrument(span.clone()).await;
            voice_span::record_http_outcome(&span, response.status());
            response
        }
        // Enter a span so every nested `tracing` event inherits `request_id`.
        RootKind::Internal => {
            let span = tracing::info_span!("request", request_id = %request_id);
            next.run(req).instrument(span).await
        }
    };

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

    /// FRD-021 §6.7 / TC-TRACE-02: which root each route gets.
    #[test]
    fn probes_get_no_span_audio_gets_the_server_root_and_the_rest_is_unchanged() {
        for probe in ["/", "/ready", "/livez", "/readyz", "/metrics"] {
            assert_eq!(root_kind(probe, true), RootKind::Probe, "{probe}");
            assert_eq!(root_kind(probe, false), RootKind::Probe, "{probe}");
        }
        for route in [
            "/v1/audio/speech",
            "/v1/audio/transcriptions",
            "/v1/audio/translations",
        ] {
            assert_eq!(root_kind(route, true), RootKind::Server(route));
            // TC-TRACE-07: the switch restores today's root.
            assert_eq!(root_kind(route, false), RootKind::Internal);
        }
        for other in [
            "/voices",
            "/speak",
            "/ws",
            "/v1/realtime",
            "/v1/audio/speech/extra",
            "/metricsx",
        ] {
            assert_eq!(root_kind(other, true), RootKind::Internal, "{other}");
        }
    }

    #[test]
    fn the_server_span_switch_defaults_on() {
        assert!(server_span_flag(None));
        assert!(server_span_flag(Some("true")));
        assert!(server_span_flag(Some("1")));
        assert!(server_span_flag(Some("")));
        for off in ["false", "0", "no", "off", " OFF ", "False"] {
            assert!(!server_span_flag(Some(off)), "{off}");
        }
    }

    #[test]
    fn the_server_address_drops_the_port_and_the_scheme_follows_the_proxy() {
        let r = req_with(&[("host", "waav.test:3001")]);
        assert_eq!(server_address(&r).as_deref(), Some("waav.test"));
        let r = req_with(&[("host", "[::1]:3001")]);
        assert_eq!(server_address(&r).as_deref(), Some("[::1]"));
        assert_eq!(server_address(&req_with(&[])), None);

        assert_eq!(request_scheme(&req_with(&[])), "http");
        assert_eq!(
            request_scheme(&req_with(&[("x-forwarded-proto", "https")])),
            "https"
        );
        assert_eq!(
            request_scheme(&req_with(&[("x-forwarded-proto", "gopher")])),
            "http"
        );
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
        assert_eq!(resolve_request_id(&req), "4bf92f3577b34da6a3ce929d0e0e4736");
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
        assert_eq!(
            id.len(),
            36,
            "blank inbound id is ignored, fresh uuid minted"
        );
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
        assert!(
            is_w3c_traceparent(&tp),
            "minted a well-formed traceparent: {tp}"
        );
        assert!(
            tp.starts_with(&format!("00-{inbound}-")),
            "reused the inbound trace id: {tp}"
        );
        assert!(tp.ends_with("-01"), "sampled flag set");
        // The span id is fresh + non-zero (never the all-zero the engine rejects).
        let span = tp.split('-').nth(2).unwrap();
        assert_eq!(span.len(), 16);
        assert!(span.chars().any(|c| c != '0'), "span id is non-zero");
    }

    #[test]
    fn hex_of_formats_lowercase_without_fallible_digit_conversion() {
        assert_eq!(hex_of(&[]), "");
        assert_eq!(
            hex_of(&[0x00, 0x01, 0x0f, 0x10, 0xab, 0xff]),
            "00010f10abff"
        );
    }

    #[test]
    fn mint_traceparent_mints_fresh_when_absent_or_invalid() {
        for inbound in [
            None,
            Some("not-hex"),
            Some("abc-123"),
            Some(&*"0".repeat(32)),
        ] {
            let tp = mint_traceparent(inbound);
            assert!(
                is_w3c_traceparent(&tp),
                "fresh minted traceparent is valid: {tp} (from {inbound:?})"
            );
        }
        // Two fresh mints differ (a real 128-bit id, not a constant).
        assert_ne!(mint_traceparent(None), mint_traceparent(None));
    }

    #[test]
    fn is_w3c_traceparent_rejects_malformed() {
        assert!(is_w3c_traceparent(
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"
        ));
        for bad in [
            "",
            "00-4bf9",                                                 // too few fields
            "00-4bf92f3577b34da6a3ce929d0e0e47-00f067aa0ba902b7-01",   // trace too short
            "00-00000000000000000000000000000000-00f067aa0ba902b7-01", // all-zero trace
            "00-4bf92f3577b34da6a3ce929d0e0e4736-0000000000000000-01", // all-zero span
            "00-zzf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01", // non-hex
        ] {
            assert!(!is_w3c_traceparent(bad), "must reject `{bad}`");
        }
    }
}
