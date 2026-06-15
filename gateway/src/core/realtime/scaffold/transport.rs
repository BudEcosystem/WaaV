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

/// Factory for [`WsTextTransport`]: opens the WS upgrade from a
/// [`ConnectSpec::WebSocket`], applying the protocol's auth/headers via
/// `into_client_request()` (the standard WS upgrade headers are generated for us
/// — the lesson from the Sarvam/DashScope header bugs).
pub struct WsTransportFactory;

#[async_trait]
impl RealtimeTransportFactory for WsTransportFactory {
    async fn connect(&self, spec: ConnectSpec) -> RealtimeResult<Box<dyn RealtimeTransport>> {
        let ConnectSpec::WebSocket { url, headers } = spec;
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
}
