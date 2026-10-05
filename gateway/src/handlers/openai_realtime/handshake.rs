//! The `/v1/realtime` handshake (FRD-023 §5.2, FR-RT-2…5, S-2, S-3).
//!
//! Everything here happens BEFORE the upgrade, so every refusal is an HTTP response in OpenAI's
//! error envelope — the shape an OpenAI SDK reads (`error.message`, `error.code`).
//!
//! The credential rules are the security-relevant part:
//!
//! * three sources — `Authorization: Bearer`, the `api-key` header (Azure-mode clients) and the
//!   `openai-insecure-api-key.<cred>` subprotocol (browsers cannot set headers);
//! * `?token=` is REFUSED here (it stays valid on `/ws` and `/realtime` for existing WaaV
//!   clients): query strings land in access logs;
//! * the credential never leaves this process: it is not echoed in the 101 (only `realtime` is
//!   selected), and [`Credential`]'s `Debug` is redacted so it cannot reach a log line.

use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};

/// The capability a `/v1/realtime` deployment serves (FRD D-4).
pub const REALTIME_CAPABILITY: &str = "realtime_session";
/// The subprotocol the server selects in the 101. The Node `ws` package fails a handshake in
/// which the client offered protocols and the server selected none.
pub const SUBPROTOCOL: &str = "realtime";
/// The credential-bearing subprotocol prefix OpenAI clients send from a browser.
pub const KEY_SUBPROTOCOL_PREFIX: &str = "openai-insecure-api-key.";
/// The beta shape, removed by OpenAI on 2026-05-12.
const BETA_SUBPROTOCOL: &str = "openai-beta.realtime-v1";
/// Parse budget per message (S-8).
pub const MAX_MESSAGE_BYTES: usize = 10 * 1024 * 1024;

/// Where the credential came from. Recorded (never the value) so a log can say how a caller
/// authenticated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialSource {
    Bearer,
    ApiKeyHeader,
    Subprotocol,
}

impl CredentialSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bearer => "authorization",
            Self::ApiKeyHeader => "api-key",
            Self::Subprotocol => "subprotocol",
        }
    }
}

/// A caller's credential. `Debug` never prints the value (S-2, TC-SEC-07).
#[derive(Clone)]
pub struct Credential {
    value: String,
    pub source: CredentialSource,
}

impl Credential {
    pub fn new(value: impl Into<String>, source: CredentialSource) -> Self {
        Self {
            value: value.into(),
            source,
        }
    }

    /// The raw credential. Callers pass it to the auth plane and nowhere else.
    pub fn expose(&self) -> &str {
        &self.value
    }

    /// An `ek_bud_` client secret (§5.8) rather than a Bud key or a JWT.
    pub fn is_client_secret(&self) -> bool {
        self.value.starts_with(crate::auth::ephemeral::TOKEN_PREFIX)
    }
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credential")
            .field("value", &"[redacted]")
            .field("source", &self.source)
            .finish()
    }
}

/// A handshake that passed the pre-auth rules.
#[derive(Debug)]
pub struct Handshake {
    /// The deployment the caller named (`?model=`): an alias or an endpoint UUID.
    pub model: String,
    pub credential: Credential,
}

/// A refusal before the upgrade, rendered in OpenAI's error envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandshakeError {
    pub status: StatusCode,
    pub kind: &'static str,
    pub code: &'static str,
    pub message: String,
    pub param: Option<&'static str>,
    pub retry_after: Option<u64>,
    /// Bud's structured detail (`stt_live_unsupported`'s `reason`, …); absent from OpenAI's errors.
    pub details: Option<serde_json::Value>,
}

impl HandshakeError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        let kind = match status {
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => "invalid_request_error",
            StatusCode::TOO_MANY_REQUESTS => "rate_limit_error",
            s if s.is_server_error() => "api_error",
            _ => "invalid_request_error",
        };
        Self {
            status,
            kind,
            code,
            message: message.into(),
            param: None,
            retry_after: None,
            details: None,
        }
    }

    pub fn details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }

    pub fn param(mut self, param: &'static str) -> Self {
        self.param = Some(param);
        self
    }

    pub fn retry_after(mut self, secs: u64) -> Self {
        self.retry_after = Some(secs.max(1));
        self
    }
}

impl IntoResponse for HandshakeError {
    fn into_response(self) -> Response {
        let mut body = serde_json::json!({
            "error": {
                "message": self.message,
                "type": self.kind,
                "param": self.param,
                "code": self.code,
            }
        });
        if let Some(d) = self.details {
            body["error"]["details"] = d;
        }
        let mut resp = (self.status, axum::Json(body)).into_response();
        if let Some(secs) = self.retry_after
            && let Ok(v) = axum::http::HeaderValue::from_str(&secs.to_string())
        {
            resp.headers_mut()
                .insert(axum::http::header::RETRY_AFTER, v);
        }
        resp
    }
}

/// Every value of every `Sec-WebSocket-Protocol` header, comma-split and trimmed.
pub fn offered_subprotocols(headers: &HeaderMap) -> Vec<String> {
    headers
        .get_all(axum::http::header::SEC_WEBSOCKET_PROTOCOL)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

/// The credential, from the first source present: `Authorization: Bearer` > `api-key` >
/// subprotocol. A malformed `Authorization` header is a refusal, not a fall-through: a caller who
/// sent one meant it.
pub fn extract_credential(headers: &HeaderMap) -> Result<Option<Credential>, HandshakeError> {
    if let Some(value) = headers.get(axum::http::header::AUTHORIZATION) {
        let text = value.to_str().map_err(|_| invalid_credential())?;
        let token = text
            .strip_prefix("Bearer ")
            .or_else(|| text.strip_prefix("bearer "))
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .ok_or_else(invalid_credential)?;
        return Ok(Some(Credential::new(token, CredentialSource::Bearer)));
    }
    if let Some(value) = headers.get("api-key") {
        let token = value
            .to_str()
            .map(str::trim)
            .ok()
            .filter(|t| !t.is_empty())
            .ok_or_else(invalid_credential)?;
        return Ok(Some(Credential::new(token, CredentialSource::ApiKeyHeader)));
    }
    for proto in offered_subprotocols(headers) {
        if let Some(token) = proto.strip_prefix(KEY_SUBPROTOCOL_PREFIX)
            && !token.is_empty()
        {
            return Ok(Some(Credential::new(token, CredentialSource::Subprotocol)));
        }
    }
    Ok(None)
}

fn invalid_credential() -> HandshakeError {
    HandshakeError::new(
        StatusCode::UNAUTHORIZED,
        "invalid_api_key",
        "The credential could not be read. Send `Authorization: Bearer <key>`, an `api-key` \
         header, or the `openai-insecure-api-key.<key>` subprotocol.",
    )
}

/// Apply the §5.2 handshake rules, in this order: the beta shape, the query, the credential.
pub fn parse(query: Option<&str>, headers: &HeaderMap) -> Result<Handshake, HandshakeError> {
    let beta_subprotocol = offered_subprotocols(headers)
        .iter()
        .any(|p| p.eq_ignore_ascii_case(BETA_SUBPROTOCOL));
    if headers.contains_key("openai-beta") || beta_subprotocol {
        return Err(HandshakeError::new(
            StatusCode::BAD_REQUEST,
            "beta_api_shape_disabled",
            "The Realtime beta interface is not supported. Remove the `OpenAI-Beta` header \
             (or the `openai-beta.realtime-v1` subprotocol) and use the GA interface.",
        ));
    }

    let mut model: Option<String> = None;
    for (key, value) in url::form_urlencoded::parse(query.unwrap_or("").as_bytes()) {
        match key.as_ref() {
            "token" => {
                return Err(HandshakeError::new(
                    StatusCode::BAD_REQUEST,
                    "use_subprotocol",
                    "Credentials are not accepted in the query string on /v1/realtime (query \
                     strings are logged). Use `Authorization: Bearer`, the `api-key` header, or \
                     the `openai-insecure-api-key.<key>` subprotocol.",
                )
                .param("token"));
            }
            "call_id" => {
                return Err(HandshakeError::new(
                    StatusCode::BAD_REQUEST,
                    "unsupported_parameter",
                    "`call_id` (sideband control of a WebRTC or SIP call) is not supported by \
                     this gateway.",
                )
                .param("call_id"));
            }
            "model" => model = Some(value.trim().to_string()).filter(|m| !m.is_empty()),
            // `intent=transcription` is accepted and ignored: the deployment decides the session
            // type. Everything else is ignored and never forwarded upstream.
            _ => {}
        }
    }
    let Some(model) = model else {
        return Err(HandshakeError::new(
            StatusCode::BAD_REQUEST,
            "model_required",
            "`model` is required: connect to /v1/realtime?model=<your realtime deployment>.",
        )
        .param("model"));
    };

    let Some(credential) = extract_credential(headers)? else {
        return Err(HandshakeError::new(
            StatusCode::UNAUTHORIZED,
            "missing_api_key",
            "You didn't provide a credential. Send `Authorization: Bearer <key>`, an `api-key` \
             header, or the `openai-insecure-api-key.<key>` subprotocol.",
        ));
    };

    Ok(Handshake { model, credential })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.append(
                axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                v.parse().unwrap(),
            );
        }
        h
    }

    #[test]
    fn tc_hs_01_bearer() {
        let hs = parse(
            Some("model=rt"),
            &headers(&[("authorization", "Bearer bud_k")]),
        )
        .unwrap();
        assert_eq!(hs.model, "rt");
        assert_eq!(hs.credential.expose(), "bud_k");
        assert_eq!(hs.credential.source, CredentialSource::Bearer);
    }

    #[test]
    fn tc_hs_02_api_key_header() {
        let hs = parse(Some("model=rt"), &headers(&[("api-key", "bud_k")])).unwrap();
        assert_eq!(hs.credential.source, CredentialSource::ApiKeyHeader);
    }

    #[test]
    fn tc_hs_03_subprotocol_credential() {
        let hs = parse(
            Some("model=rt"),
            &headers(&[(
                "sec-websocket-protocol",
                "realtime, openai-insecure-api-key.bud_k",
            )]),
        )
        .unwrap();
        assert_eq!(hs.credential.expose(), "bud_k");
        assert_eq!(hs.credential.source, CredentialSource::Subprotocol);
    }

    #[test]
    fn tc_hs_04_extra_subprotocols_are_tolerated() {
        let hs = parse(
            Some("model=rt"),
            &headers(&[(
                "sec-websocket-protocol",
                "realtime, openai-agents-sdk.v0.18, openai-project.p, openai-insecure-api-key.k",
            )]),
        )
        .unwrap();
        assert_eq!(hs.credential.expose(), "k");
    }

    #[test]
    fn tc_hs_06_model_is_required() {
        let err = parse(None, &headers(&[("authorization", "Bearer k")])).unwrap_err();
        assert_eq!(
            (err.status, err.code),
            (StatusCode::BAD_REQUEST, "model_required")
        );
        let err = parse(Some("model="), &headers(&[("authorization", "Bearer k")])).unwrap_err();
        assert_eq!(err.code, "model_required");
    }

    #[test]
    fn tc_hs_07_query_token_is_refused() {
        let err = parse(Some("token=bud_k&model=rt"), &HeaderMap::new()).unwrap_err();
        assert_eq!(
            (err.status, err.code),
            (StatusCode::BAD_REQUEST, "use_subprotocol")
        );
        assert!(!err.message.contains("bud_k"));
    }

    #[test]
    fn tc_hs_08_the_beta_shape_is_refused() {
        let err = parse(
            Some("model=rt"),
            &headers(&[
                ("authorization", "Bearer k"),
                ("openai-beta", "realtime=v1"),
            ]),
        )
        .unwrap_err();
        assert_eq!(err.code, "beta_api_shape_disabled");
        let err = parse(
            Some("model=rt"),
            &headers(&[(
                "sec-websocket-protocol",
                "realtime, openai-beta.realtime-v1, openai-insecure-api-key.k",
            )]),
        )
        .unwrap_err();
        assert_eq!(err.code, "beta_api_shape_disabled");
    }

    #[test]
    fn tc_hs_09_call_id_is_refused() {
        let err = parse(
            Some("model=rt&call_id=rtc_x"),
            &headers(&[("authorization", "Bearer k")]),
        )
        .unwrap_err();
        assert_eq!(err.code, "unsupported_parameter");
    }

    #[test]
    fn tc_hs_10_intent_is_ignored() {
        let hs = parse(
            Some("model=rt&intent=transcription"),
            &headers(&[("authorization", "Bearer k")]),
        )
        .unwrap();
        assert_eq!(hs.model, "rt");
    }

    #[test]
    fn no_credential_is_a_401() {
        let err = parse(Some("model=rt"), &HeaderMap::new()).unwrap_err();
        assert_eq!(
            (err.status, err.code),
            (StatusCode::UNAUTHORIZED, "missing_api_key")
        );
    }

    #[test]
    fn a_malformed_authorization_header_is_refused_not_skipped() {
        let err = parse(
            Some("model=rt"),
            &headers(&[("authorization", "Basic abc"), ("api-key", "k")]),
        )
        .unwrap_err();
        assert_eq!(err.status, StatusCode::UNAUTHORIZED);
    }

    /// TC-SEC-07 (unit half): the credential cannot reach a log through `Debug`.
    #[test]
    fn tc_sec_07_the_credential_debug_is_redacted() {
        let hs = parse(
            Some("model=rt"),
            &headers(&[("authorization", "Bearer bud_secret_value")]),
        )
        .unwrap();
        let printed = format!("{hs:?}");
        assert!(!printed.contains("bud_secret_value"), "{printed}");
        assert!(printed.contains("[redacted]"));
    }

    #[test]
    fn the_error_envelope_is_openai_shaped() {
        let resp = HandshakeError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limit_exceeded",
            "slow down",
        )
        .retry_after(3)
        .into_response();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(resp.headers()[axum::http::header::RETRY_AFTER], "3");
    }
}
