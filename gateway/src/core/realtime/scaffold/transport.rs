//! `RealtimeTransport` — the bytes-in/bytes-out plumbing the driver talks to,
//! decoupled from BOTH the protocol (what the frames mean) and the connection
//! mechanism (WebSocket / REST-then-WS / Bedrock bidi HTTP-2 stream).
//!
//! Verified in-tree precedent: WaaV already drives a non-WS bidirectional HTTP/2
//! event-stream (`aws-sdk-transcribestreaming`) behind a transport trait
//! (`core/websocket/reconnectable_stream.rs` `WsTransport` + `AwsTranscribeTransport`).
//! So one driver + a swappable transport absorbs all three cases; AWS Nova Sonic
//! is "just another `(protocol, transport)` pair" in a later phase.

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use std::str::FromStr;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderName, HeaderValue};
use tokio_tungstenite::tungstenite::Message;

use super::super::base::{RealtimeError, RealtimeResult};
use super::event::{ConnectSpec, OutFrame};

/// A live bidirectional realtime transport. The driver owns the state machine +
/// `S2sEvent` dispatch; the transport owns only the framing.
#[async_trait]
pub trait RealtimeTransport: Send {
    /// Push one already-serialized outbound frame.
    async fn send(&mut self, frame: OutFrame) -> RealtimeResult<()>;

    /// Pull the next inbound frame. `None` = clean close (no reconnect);
    /// `Some(Err)` = transport error (the driver decides whether to reconnect).
    /// Ping/Pong keepalives are handled internally and never surface here.
    async fn recv(&mut self) -> Option<RealtimeResult<OutFrame>>;

    /// Best-effort graceful close.
    async fn close(&mut self) {}
}

/// Builds a fresh transport per (re)connect. Held by the driver's supervisor so
/// it can re-dial on connection loss.
#[async_trait]
pub trait RealtimeTransportFactory: Send + Sync {
    /// Open a transport for the given spec (may do an async handshake: WS upgrade,
    /// REST-create-call, SigV4 stream open).
    async fn connect(&self, spec: ConnectSpec) -> RealtimeResult<Box<dyn RealtimeTransport>>;
}

// =============================================================================
// WebSocket transport (serves JSON-text AND binary-frame providers)
// =============================================================================

type WsStream = tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
>;

/// A tokio-tungstenite WebSocket transport. Handles BOTH text-JSON providers
/// (OpenAI/Azure/Gemini/Grok/Inworld) and binary-frame providers
/// (Deepgram/ElevenLabs/AssemblyAI) — the protocol decides which `OutFrame`
/// variant it serializes to, and inbound `Message::Text`/`Message::Binary` map
/// straight back to `OutFrame::Text`/`OutFrame::Binary`.
pub struct WsTextTransport {
    sink: futures_util::stream::SplitSink<WsStream, Message>,
    stream: futures_util::stream::SplitStream<WsStream>,
}

#[async_trait]
impl RealtimeTransport for WsTextTransport {
    async fn send(&mut self, frame: OutFrame) -> RealtimeResult<()> {
        let msg = match frame {
            OutFrame::Text(s) => Message::Text(s.into()),
            OutFrame::Binary(b) => Message::Binary(b),
        };
        self.sink
            .send(msg)
            .await
            .map_err(|e| RealtimeError::WebSocketError(e.to_string()))
    }

    async fn recv(&mut self) -> Option<RealtimeResult<OutFrame>> {
        loop {
            match self.stream.next().await? {
                Ok(Message::Text(t)) => return Some(Ok(OutFrame::Text(t.to_string()))),
                Ok(Message::Binary(b)) => return Some(Ok(OutFrame::Binary(b))),
                Ok(Message::Ping(data)) => {
                    // Respond to keepalive and keep waiting for real data.
                    if let Err(e) = self.sink.send(Message::Pong(data)).await {
                        return Some(Err(RealtimeError::WebSocketError(e.to_string())));
                    }
                }
                Ok(Message::Pong(_)) | Ok(Message::Frame(_)) => {}
                Ok(Message::Close(_)) => return None,
                Err(e) => return Some(Err(RealtimeError::WebSocketError(e.to_string()))),
            }
        }
    }

    async fn close(&mut self) {
        let _ = self.sink.send(Message::Close(None)).await;
    }
}

/// Open a WS upgrade to `url` with `headers`, applying the auth/headers via
/// `into_client_request()` (the standard WS upgrade headers are generated for us
/// — the lesson from the Sarvam/DashScope header bugs). Shared by both the plain
/// [`WsTransportFactory`] and the REST-handshake
/// [`RestHandshakeWsTransportFactory`] (which calls it with the pre-authed join
/// url + NO extra headers).
async fn connect_ws(
    url: String,
    headers: Vec<(String, String)>,
) -> RealtimeResult<Box<dyn RealtimeTransport>> {
    let mut request = url
        .into_client_request()
        .map_err(|e| RealtimeError::ConnectionFailed(format!("bad ws url: {e}")))?;
    let hdrs = request.headers_mut();
    for (k, v) in headers {
        let name = HeaderName::from_str(&k)
            .map_err(|e| RealtimeError::ConnectionFailed(format!("bad header {k}: {e}")))?;
        let val = HeaderValue::from_str(&v)
            .map_err(|e| RealtimeError::ConnectionFailed(format!("bad header value: {e}")))?;
        hdrs.insert(name, val);
    }
    let (ws, _resp) = tokio_tungstenite::connect_async(request)
        .await
        .map_err(|e| RealtimeError::ConnectionFailed(e.to_string()))?;
    let (sink, stream) = ws.split();
    Ok(Box::new(WsTextTransport { sink, stream }))
}

/// Factory for [`WsTextTransport`]: opens the WS upgrade from a
/// [`ConnectSpec::WebSocket`], applying the protocol's auth/headers via
/// `into_client_request()` (the standard WS upgrade headers are generated for us
/// — the lesson from the Sarvam/DashScope header bugs).
pub struct WsTransportFactory;

#[async_trait]
impl RealtimeTransportFactory for WsTransportFactory {
    async fn connect(&self, spec: ConnectSpec) -> RealtimeResult<Box<dyn RealtimeTransport>> {
        match spec {
            ConnectSpec::WebSocket { url, headers } => connect_ws(url, headers).await,
            ConnectSpec::RestThenWebSocket { .. } => Err(RealtimeError::ConnectionFailed(
                "WsTransportFactory does not support RestThenWebSocket; \
                 use RestHandshakeWsTransportFactory"
                    .to_string(),
            )),
        }
    }
}

// =============================================================================
// REST-handshake-then-WebSocket transport (Ultravox)
// =============================================================================

/// Pull the WS join url out of a create-call JSON response. `pointer` is the
/// TOP-LEVEL field name (e.g. `"joinUrl"`). Factored out (no live POST) so the
/// extraction is unit-testable. Errs if the field is missing or not a string.
fn extract_join_url(response: &serde_json::Value, pointer: &str) -> RealtimeResult<String> {
    response
        .get(pointer)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            RealtimeError::ConnectionFailed(format!(
                "create-call response missing string field {pointer:?}: {response}"
            ))
        })
}

/// Factory for the ULTRAVOX pattern: a REST "create call" `POST` mints a
/// single-use `joinUrl`, then a plain WebSocket connects that pre-authed url.
///
/// Implements [`RealtimeTransportFactory`] so the generic driver re-dials it on
/// connection loss exactly like a plain WS factory — a CONTAINED transport-layer
/// addition; the driver (`session.rs`) is unchanged. The protocol opts in via
/// `RealtimeProtocol::transport_factory()`.
///
/// Reuses the gateway's existing HTTP client (`reqwest`, already a workspace dep
/// used by every other provider's REST calls). The returned transport is a plain
/// [`WsTextTransport`] over the join url (binary audio + JSON control), so the
/// raw-binary `OutFrame` path Ultravox needs works unchanged.
pub struct RestHandshakeWsTransportFactory;

#[async_trait]
impl RealtimeTransportFactory for RestHandshakeWsTransportFactory {
    async fn connect(&self, spec: ConnectSpec) -> RealtimeResult<Box<dyn RealtimeTransport>> {
        match spec {
            ConnectSpec::RestThenWebSocket {
                create_url,
                headers,
                body,
                join_url_pointer,
            } => {
                // 1. POST the create-call. Reuse the shared reqwest client.
                let mut req = reqwest::Client::new()
                    .post(&create_url)
                    .header(reqwest::header::CONTENT_TYPE, "application/json")
                    .body(body);
                for (k, v) in headers {
                    req = req.header(k, v);
                }
                let resp = req.send().await.map_err(|e| {
                    RealtimeError::ConnectionFailed(format!("create-call POST failed: {e}"))
                })?;

                // 2. Non-2xx ⇒ surface the status + body (auth/quota errors).
                let status = resp.status();
                let text = resp.text().await.map_err(|e| {
                    RealtimeError::ConnectionFailed(format!(
                        "create-call response read failed: {e}"
                    ))
                })?;
                if !status.is_success() {
                    return Err(RealtimeError::ConnectionFailed(format!(
                        "create-call returned {status}: {text}"
                    )));
                }

                // 3. Parse JSON + extract the join url.
                let json: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
                    RealtimeError::ConnectionFailed(format!(
                        "create-call response not JSON: {e}: {text}"
                    ))
                })?;
                let join_url = extract_join_url(&json, &join_url_pointer)?;

                // 4. Connect the pre-authed join url (NO extra headers).
                connect_ws(join_url, Vec::new()).await
            }
            ConnectSpec::WebSocket { url, headers } => {
                // Not needed by Ultravox, but harmless: a plain WS still works.
                connect_ws(url, headers).await
            }
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The join-url extraction the REST-handshake factory does: present string
    /// field at the pointer ⇒ that url.
    #[test]
    fn extract_join_url_reads_pointer_field() {
        let resp = json!({ "joinUrl": "wss://example.ultravox.ai/call/abc", "callId": "abc" });
        assert_eq!(
            extract_join_url(&resp, "joinUrl").unwrap(),
            "wss://example.ultravox.ai/call/abc"
        );
    }

    /// A DIFFERENT pointer name still works (the field name is configurable).
    #[test]
    fn extract_join_url_honors_custom_pointer() {
        let resp = json!({ "ws_url": "wss://x" });
        assert_eq!(extract_join_url(&resp, "ws_url").unwrap(), "wss://x");
    }

    /// Missing field ⇒ Err (ConnectionFailed), not a panic.
    #[test]
    fn extract_join_url_missing_field_errors() {
        let resp = json!({ "callId": "abc" });
        assert!(matches!(
            extract_join_url(&resp, "joinUrl"),
            Err(RealtimeError::ConnectionFailed(_))
        ));
    }

    /// Field present but not a string (or empty) ⇒ Err.
    #[test]
    fn extract_join_url_non_string_or_empty_errors() {
        let num = json!({ "joinUrl": 42 });
        assert!(matches!(
            extract_join_url(&num, "joinUrl"),
            Err(RealtimeError::ConnectionFailed(_))
        ));
        let empty = json!({ "joinUrl": "" });
        assert!(matches!(
            extract_join_url(&empty, "joinUrl"),
            Err(RealtimeError::ConnectionFailed(_))
        ));
    }

    /// The plain `WsTransportFactory` explicitly rejects `RestThenWebSocket`
    /// (callers must use `RestHandshakeWsTransportFactory`).
    #[tokio::test]
    async fn ws_factory_rejects_rest_then_ws_spec() {
        let spec = ConnectSpec::RestThenWebSocket {
            create_url: "https://api.ultravox.ai/api/calls".to_string(),
            headers: vec![],
            body: "{}".to_string(),
            join_url_pointer: "joinUrl".to_string(),
        };
        assert!(matches!(
            WsTransportFactory.connect(spec).await,
            Err(RealtimeError::ConnectionFailed(_))
        ));
    }
}
