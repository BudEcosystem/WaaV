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
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use std::str::FromStr;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderName, HeaderValue};

use super::super::base::{RealtimeError, RealtimeResult};
use super::event::{ConnectSpec, OutFrame};
use crate::core::resilience::connect::{WS_CONNECT_TIMEOUT, with_timeout};

const REST_HANDSHAKE_CREATE_URL_SCHEMES: &[&str] = &["http", "https"];
const REST_HANDSHAKE_JOIN_URL_SCHEMES: &[&str] = &["ws", "wss"];

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

type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

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
    // Host only (never the full URL: query strings can carry auth tokens).
    let host = request.uri().host().unwrap_or("<unknown host>").to_string();
    let (ws, _resp) = with_timeout(
        WS_CONNECT_TIMEOUT,
        tokio_tungstenite::connect_async(request),
    )
    .await
    .map_err(|_| {
        RealtimeError::ConnectionFailed(format!(
            "ws connect to {host} timed out after {}s",
            WS_CONNECT_TIMEOUT.as_secs()
        ))
    })?
    .map_err(|e| RealtimeError::ConnectionFailed(e.to_string()))?;
    let (sink, stream) = ws.split();
    Ok(Box::new(WsTextTransport { sink, stream }))
}

/// Factory for [`WsTextTransport`]: opens the WS upgrade from a
/// [`ConnectSpec::WebSocket`], applying the protocol's auth/headers via
/// `into_client_request()` (the standard WS upgrade headers are generated for us
/// — the lesson from the Sarvam/DashScope header bugs).
///
/// It ALSO serves the in-box [`ConnectSpec::Unix`] (GW-13 UDS half): a co-located
/// WaaV Infer sidecar reached over a unix domain socket. The default
/// `RealtimeProtocol::transport_factory()` is this factory, so a provider whose
/// `connect_spec` returns `Unix` (the in-box Infer S2S sidecar) gets UDS for free
/// — no separate factory, the driver re-dials it on reconnect like any other.
pub struct WsTransportFactory;

#[async_trait]
impl RealtimeTransportFactory for WsTransportFactory {
    async fn connect(&self, spec: ConnectSpec) -> RealtimeResult<Box<dyn RealtimeTransport>> {
        match spec {
            ConnectSpec::WebSocket { url, headers } => connect_ws(url, headers).await,
            ConnectSpec::Unix { path } => connect_uds(path).await,
            ConnectSpec::RestThenWebSocket { .. } => Err(RealtimeError::ConnectionFailed(
                "WsTransportFactory does not support RestThenWebSocket; \
                 use RestHandshakeWsTransportFactory"
                    .to_string(),
            )),
            ConnectSpec::BedrockBidi { .. } => Err(RealtimeError::ConnectionFailed(
                "WsTransportFactory does not support BedrockBidi; \
                 use BedrockBidiTransportFactory"
                    .to_string(),
            )),
        }
    }
}

// =============================================================================
// Unix-domain-socket transport (GW-13 UDS half — in-box WaaV Infer S2S sidecar)
// =============================================================================
//
// A co-located Infer sidecar (the single-box topology, INFER_GATEWAY_INTEGRATION
// §6.2/§7) is reached over a UNIX DOMAIN SOCKET, removing the loopback-TCP hop.
// A raw UDS stream has no WebSocket Text/Binary opcodes, so we frame each
// `OutFrame` with a tiny self-delimiting header that preserves the SAME Text +
// Binary vocabulary the WS transport carries — so the protocol's wire mapping is
// untouched and raw-binary audio stays byte-exact (the accuracy-at-the-seam
// invariant). The framing mirrors the in-tree length-prefixed UDS precedent
// (`dag/nodes/endpoint.rs`), extended with a 1-byte KIND tag:
//
//   ┌──────────┬──────────────────┬───────────────┐
//   │ kind: u8 │ len: u32 (BE)    │ payload: len B │
//   └──────────┴──────────────────┴───────────────┘
//     0 = Text (UTF-8 JSON control)   1 = Binary (raw audio, byte-exact)

/// The 1-byte frame-kind tag for a Text (UTF-8 control) UDS frame.
const UDS_KIND_TEXT: u8 = 0;
/// The 1-byte frame-kind tag for a Binary (raw audio) UDS frame.
const UDS_KIND_BINARY: u8 = 1;
/// The fixed UDS frame header size: 1 kind byte + 4 length bytes.
const UDS_HEADER_LEN: usize = 5;
/// Reject an absurd inbound length (a corrupt/hostile peer) before allocating —
/// mirrors the 100 MB sanity bound the in-tree UDS endpoint applies.
const UDS_MAX_FRAME_LEN: usize = 100 * 1024 * 1024;

fn checked_uds_frame_body_len(len: usize) -> RealtimeResult<u32> {
    if len > UDS_MAX_FRAME_LEN {
        return Err(RealtimeError::ProviderError(format!(
            "UDS frame length {len} exceeds the {UDS_MAX_FRAME_LEN}-byte bound"
        )));
    }
    u32::try_from(len).map_err(|_| {
        RealtimeError::ProviderError(format!(
            "UDS frame length {len} exceeds the u32 wire prefix"
        ))
    })
}

/// PURE encoder (unit-testable, no socket): append the kind+length-prefixed wire
/// bytes for one [`OutFrame`] to `out`. Text frames carry the UTF-8 bytes; Binary
/// frames carry the raw audio bytes byte-exact.
fn encode_uds_frame(frame: &OutFrame, out: &mut Vec<u8>) -> RealtimeResult<()> {
    let (kind, body): (u8, &[u8]) = match frame {
        OutFrame::Text(s) => (UDS_KIND_TEXT, s.as_bytes()),
        OutFrame::Binary(b) => (UDS_KIND_BINARY, b.as_ref()),
    };
    let body_len = checked_uds_frame_body_len(body.len())?;
    out.reserve(UDS_HEADER_LEN + body.len());
    out.push(kind);
    out.extend_from_slice(&body_len.to_be_bytes());
    out.extend_from_slice(body);
    Ok(())
}

/// PURE decoder (unit-testable, no socket): try to decode ONE [`OutFrame`] from
/// the front of `buf`. Returns `Some((frame, consumed))` when a whole frame is
/// present (`consumed` = bytes to drain), or `None` when more bytes are needed.
/// An over-long length is surfaced as a typed `Err` (a corrupt/hostile peer),
/// never a panic or a huge allocation.
#[allow(clippy::type_complexity)]
fn decode_uds_frame(buf: &[u8]) -> Option<RealtimeResult<(OutFrame, usize)>> {
    if buf.len() < UDS_HEADER_LEN {
        return None; // not even a full header yet — wait for more.
    }
    let kind = buf[0];
    let len = u32::from_be_bytes([buf[1], buf[2], buf[3], buf[4]]) as usize;
    if len > UDS_MAX_FRAME_LEN {
        return Some(Err(RealtimeError::ProviderError(format!(
            "UDS frame length {len} exceeds the {UDS_MAX_FRAME_LEN}-byte bound"
        ))));
    }
    let end = UDS_HEADER_LEN + len;
    if buf.len() < end {
        return None; // header present, body incomplete — wait for more.
    }
    let body = &buf[UDS_HEADER_LEN..end];
    let frame = match kind {
        UDS_KIND_TEXT => match std::str::from_utf8(body) {
            Ok(s) => OutFrame::Text(s.to_string()),
            Err(e) => {
                return Some(Err(RealtimeError::ProviderError(format!(
                    "UDS text frame is not valid UTF-8: {e}"
                ))));
            }
        },
        UDS_KIND_BINARY => OutFrame::Binary(Bytes::copy_from_slice(body)),
        other => {
            return Some(Err(RealtimeError::ProviderError(format!(
                "unknown UDS frame kind {other} (expected 0=text or 1=binary)"
            ))));
        }
    };
    Some(Ok((frame, end)))
}

/// A length-framed unix-domain-socket transport for the in-box Infer S2S sidecar.
/// Carries the same Text+Binary `OutFrame` vocabulary as the WS transport over a
/// raw `UnixStream`, framed by [`encode_uds_frame`]/[`decode_uds_frame`].
pub struct UdsTransport {
    stream: tokio::net::UnixStream,
    /// Carry-over inbound bytes that did not yet form a whole frame (the stream is
    /// byte-oriented: one read can split or coalesce frames).
    rx_buf: Vec<u8>,
}

#[async_trait]
impl RealtimeTransport for UdsTransport {
    async fn send(&mut self, frame: OutFrame) -> RealtimeResult<()> {
        use tokio::io::AsyncWriteExt;
        let mut out = Vec::with_capacity(UDS_HEADER_LEN);
        encode_uds_frame(&frame, &mut out)?;
        self.stream
            .write_all(&out)
            .await
            .map_err(|e| RealtimeError::ConnectionFailed(format!("UDS write failed: {e}")))
    }

    async fn recv(&mut self) -> Option<RealtimeResult<OutFrame>> {
        use tokio::io::AsyncReadExt;
        loop {
            // First, try to satisfy a frame entirely from the carry-over buffer
            // (no await on the host path — no in-loop host sync beyond the socket).
            match decode_uds_frame(&self.rx_buf) {
                Some(Ok((frame, consumed))) => {
                    self.rx_buf.drain(..consumed);
                    return Some(Ok(frame));
                }
                Some(Err(e)) => return Some(Err(e)),
                None => {} // need more bytes from the socket.
            }
            // Read more bytes (an awaited socket read — bounded by the peer/close,
            // and by the driver's reconnect supervisor + the test timeout).
            let mut chunk = [0u8; 16 * 1024];
            match self.stream.read(&mut chunk).await {
                Ok(0) => return None, // clean EOF ⇒ no reconnect (the WS `Close` analogue).
                Ok(n) => self.rx_buf.extend_from_slice(&chunk[..n]),
                Err(e) => {
                    return Some(Err(RealtimeError::ConnectionFailed(format!(
                        "UDS read failed: {e}"
                    ))));
                }
            }
        }
    }

    async fn close(&mut self) {
        use tokio::io::AsyncWriteExt;
        // Best-effort half-close so the peer sees EOF (the UDS analogue of a WS
        // Close frame). Errors are ignored — the socket may already be gone.
        let _ = self.stream.shutdown().await;
    }
}

/// Open a unix-domain-socket transport to `path` (the in-box Infer S2S sidecar).
/// A missing/unreachable socket is a typed `ConnectionFailed` (the driver's
/// reconnect supervisor + breaker handle it exactly like a WS dial failure).
async fn connect_uds(path: String) -> RealtimeResult<Box<dyn RealtimeTransport>> {
    let stream = tokio::net::UnixStream::connect(&path).await.map_err(|e| {
        RealtimeError::ConnectionFailed(format!("UDS connect to {path:?} failed: {e}"))
    })?;
    Ok(Box::new(UdsTransport {
        stream,
        rx_buf: Vec::new(),
    }))
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

fn validate_rest_handshake_create_url(url: &str) -> RealtimeResult<()> {
    let url = url.trim();
    if url.is_empty() {
        return Err(RealtimeError::ConnectionFailed(
            "REST-handshake create URL rejected (SSRF protection): empty URL".to_string(),
        ));
    }
    crate::core::net::validate_url_for_ssrf(url, REST_HANDSHAKE_CREATE_URL_SCHEMES).map_err(|msg| {
        RealtimeError::ConnectionFailed(format!(
            "REST-handshake create URL rejected (SSRF protection): {msg}"
        ))
    })
}

fn validate_rest_handshake_join_url(url: &str) -> RealtimeResult<String> {
    let url = url.trim();
    if url.is_empty() {
        return Err(RealtimeError::ConnectionFailed(
            "REST-handshake join URL rejected (SSRF protection): empty URL".to_string(),
        ));
    }
    crate::core::net::validate_url_for_ssrf(url, REST_HANDSHAKE_JOIN_URL_SCHEMES)
        .map(|_| url.to_string())
        .map_err(|msg| {
            RealtimeError::ConnectionFailed(format!(
                "REST-handshake join URL rejected (SSRF protection): {msg}"
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
                validate_rest_handshake_create_url(&create_url)?;

                // 1. POST the create-call, validating every redirect target.
                let create_client =
                    crate::core::net::ssrf_protected_client(REST_HANDSHAKE_CREATE_URL_SCHEMES)
                        .map_err(|e| {
                            RealtimeError::ConnectionFailed(format!(
                                "create-call client failed: {e}"
                            ))
                        })?;
                let mut req = create_client
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
                let join_url = validate_rest_handshake_join_url(&join_url)?;

                // 4. Connect the pre-authed join url (NO extra headers).
                connect_ws(join_url, Vec::new()).await
            }
            ConnectSpec::WebSocket { url, headers } => {
                // Not needed by Ultravox, but harmless: a plain WS still works.
                connect_ws(url, headers).await
            }
            ConnectSpec::Unix { .. } => Err(RealtimeError::ConnectionFailed(
                "RestHandshakeWsTransportFactory does not support Unix; \
                 use the default WsTransportFactory for UDS"
                    .to_string(),
            )),
            ConnectSpec::BedrockBidi { .. } => Err(RealtimeError::ConnectionFailed(
                "RestHandshakeWsTransportFactory does not support BedrockBidi; \
                 use BedrockBidiTransportFactory"
                    .to_string(),
            )),
        }
    }
}

// =============================================================================
// AWS Nova Sonic — Bedrock bidirectional HTTP/2 event-stream transport
// =============================================================================
//
// VERIFIED IN-TREE PRECEDENT: this mirrors `AwsTranscribeTransport`
// (`core/stt/aws_transcribe/client.rs`) — `aws-sdk-bedrockruntime`'s
// `InvokeModelWithBidirectionalStream` is the SAME smithy event-stream shape as
// `aws-sdk-transcribestreaming`'s `start_stream_transcription`: an input half
// (`EventStreamSender<InvokeModelWithBidirectionalStreamInput, …>`, fed by an
// async stream of union events) + an output half
// (`EventReceiver<InvokeModelWithBidirectionalStreamOutput, …>`, drained via
// `recv()`). The ONE structural difference vs Transcribe: the scaffold transport
// pushes outbound frames INCREMENTALLY (`send()` over the session lifetime), so
// the input half is fed by an `mpsc` channel whose sender IS the `send()` surface
// (the same channel-fed `async_stream` Transcribe uses for its audio input,
// hoisted to the transport boundary).

use aws_config::BehaviorVersion;
use aws_sdk_bedrockruntime::Client as BedrockClient;
use aws_sdk_bedrockruntime::types::{
    BidirectionalInputPayloadPart, InvokeModelWithBidirectionalStreamInput as BidiInputEvent,
    InvokeModelWithBidirectionalStreamOutput as BidiOutputEvent,
    error::InvokeModelWithBidirectionalStreamInputError as BidiInputError,
};
use aws_smithy_types::Blob;
use tokio::sync::mpsc;

/// Channel depth for outbound Bedrock input events. Nova Sonic input is small
/// JSON events (base64 audio chunks ~20 ms each); 64 absorbs bursts while bounding
/// memory — same order as the Transcribe audio channel.
const BEDROCK_INPUT_CHANNEL_DEPTH: usize = 64;

/// Bounds the whole Bedrock dial (aws-config load + bidi-stream open). Without it,
/// a MISCONFIGURED deployment (no AWS creds/region ⇒ the default chain stalls on
/// slow IMDS timeouts) would slow-fail the client over many backoff retries instead
/// of surfacing a prompt error. Mirrors the connection timeout `AwsTranscribeTransport`
/// applies (the in-tree precedent this transport structurally copies). A correct
/// deployment (creds+region present) completes the dial in well under this bound.
const BEDROCK_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// PURE helper (unit-testable WITHOUT AWS creds): wrap one outbound Nova Sonic
/// event JSON string into the Bedrock SDK's input payload union event. This is
/// the exact transform `send()` applies — the `bytes` blob of a
/// `BidirectionalInputPayloadPart`, lifted into the `Chunk` variant of the input
/// event stream's union type. (Nova Sonic carries its audio as base64 INSIDE this
/// JSON, so there is no separate binary frame.)
fn wrap_input_payload(json: String) -> BidiInputEvent {
    BidiInputEvent::Chunk(
        BidirectionalInputPayloadPart::builder()
            .bytes(Blob::new(json.into_bytes()))
            .build(),
    )
}

/// PURE helper (unit-testable WITHOUT AWS creds): unwrap one inbound Bedrock
/// output payload union event back to the Nova Sonic event JSON string. This is
/// the exact transform `recv()` applies — extract the `Chunk`'s
/// `BidirectionalOutputPayloadPart.bytes` blob and decode it as UTF-8. Returns
/// `None` for a non-`Chunk`/empty/invalid-UTF-8 payload (the recv loop then skips
/// it rather than surfacing a frame).
fn unwrap_output_payload(event: &BidiOutputEvent) -> Option<String> {
    let part = event.as_chunk().ok()?;
    let blob = part.bytes()?;
    String::from_utf8(blob.as_ref().to_vec()).ok()
}

/// A live AWS Nova Sonic transport: the input half of a Bedrock
/// `InvokeModelWithBidirectionalStream` is fed by `input_tx` (each
/// [`OutFrame::Text`] becomes a `BidirectionalInputPayloadPart`), and the output
/// half is the SDK's [`EventReceiver`](aws_sdk_bedrockruntime::primitives::event_stream::EventReceiver)
/// drained by [`recv`](RealtimeTransport::recv). Dropping the transport drops
/// `input_tx` → the input stream ends → the HTTP/2 request finalizes (the same
/// channel-close finalize `AwsTranscribeTransport` relies on).
type BedrockOutput = aws_sdk_bedrockruntime::primitives::event_stream::EventReceiver<
    BidiOutputEvent,
    aws_sdk_bedrockruntime::types::error::InvokeModelWithBidirectionalStreamOutputError,
>;

/// The output half of a Bedrock stream: still opening, or open.
///
/// The SDK's `send()` for `InvokeModelWithBidirectionalStream` does not return at the response
/// headers — it waits for the stream's FIRST output event (`try_recv_initial_response`). Nova
/// Sonic sends nothing until it has input, and the input (the session configuration, then the
/// caller's audio) is written only after the transport exists. Awaiting `send()` inside
/// `connect` therefore deadlocks until the dial timeout (found by the in-process Bedrock mock,
/// FRD-023 TC-XL-05). The open runs in its own task instead: `connect` returns at once, the
/// input channel buffers what the driver writes, and the first `recv` waits for the open.
enum BedrockOutputState {
    Opening(tokio::task::JoinHandle<RealtimeResult<BedrockOutput>>),
    Open(Box<BedrockOutput>),
    Closed,
}

pub struct BedrockBidiTransport {
    /// Outbound input events → the SDK's input event-stream sender (via the
    /// channel-fed `async_stream` installed at connect). Cloned `send()` surface.
    input_tx: mpsc::Sender<BidiInputEvent>,
    /// This connection's OWN output event receiver (owned outright, dropped with
    /// the transport) — the bidi stream's server→client half — once the open completes.
    output: BedrockOutputState,
}

impl Drop for BedrockBidiTransport {
    fn drop(&mut self) {
        if let BedrockOutputState::Opening(handle) = &self.output {
            handle.abort();
        }
    }
}

#[async_trait]
impl RealtimeTransport for BedrockBidiTransport {
    async fn send(&mut self, frame: OutFrame) -> RealtimeResult<()> {
        // Nova Sonic is event-framed JSON: only Text frames are sent (audio is
        // base64 INSIDE the JSON events). A stray Binary frame has no Bedrock
        // representation → surface a clear error rather than silently dropping.
        let json = match frame {
            OutFrame::Text(s) => s,
            OutFrame::Binary(_) => {
                return Err(RealtimeError::ProviderError(
                    "BedrockBidi transport received a Binary frame; Nova Sonic audio is \
                     base64-in-JSON (Text frames only)"
                        .to_string(),
                ));
            }
        };
        self.input_tx
            .send(wrap_input_payload(json))
            .await
            .map_err(|_| {
                RealtimeError::ConnectionFailed("Bedrock bidi input stream closed".to_string())
            })
    }

    async fn recv(&mut self) -> Option<RealtimeResult<OutFrame>> {
        // Mirror the Transcribe result loop: skip non-payload events, surface a
        // decoded JSON event as a Text frame, map a stream error to Some(Err),
        // and a clean end (`Ok(None)`) to None (no reconnect).
        loop {
            let output = match &mut self.output {
                BedrockOutputState::Open(output) => output,
                BedrockOutputState::Closed => return None,
                BedrockOutputState::Opening(handle) => {
                    let opened = match handle.await {
                        Ok(result) => result,
                        Err(e) => Err(RealtimeError::ConnectionFailed(format!(
                            "Bedrock stream open task failed: {e}"
                        ))),
                    };
                    match opened {
                        Ok(output) => {
                            self.output = BedrockOutputState::Open(Box::new(output));
                            continue;
                        }
                        Err(e) => {
                            self.output = BedrockOutputState::Closed;
                            return Some(Err(e));
                        }
                    }
                }
            };
            match output.recv().await {
                Ok(Some(event)) => {
                    if let Some(json) = unwrap_output_payload(&event) {
                        return Some(Ok(OutFrame::Text(json)));
                    }
                    // Empty/unknown payload (e.g. a future union variant) → keep
                    // waiting for the next real event.
                }
                Ok(None) => return None,
                Err(e) => {
                    return Some(Err(RealtimeError::ProviderError(format!(
                        "Bedrock bidi stream error: {e}"
                    ))));
                }
            }
        }
    }

    async fn close(&mut self) {
        // No explicit close frame: dropping `input_tx` ends the input stream so the
        // HTTP/2 request finalizes (the channel-close finalize Transcribe uses).
        // Replacing the sender with a fresh closed channel drops our handle now.
        let (closed_tx, _) = mpsc::channel(1);
        self.input_tx = closed_tx;
    }
}

/// May this dial proceed? In Bud mode a Bedrock stream is signed with the DEPLOYMENT's static
/// keys or not at all — the gateway's own AWS identity (env, shared config, instance role) is
/// never lent to a tenant session (FRD-023 D-5, RT0). Pure, so the rule is testable without
/// flipping the process-wide Bud-mode flag.
pub(crate) fn bedrock_dial_allowed(
    credentials: Option<&crate::core::realtime::base::AwsStaticCredentials>,
    in_bud_mode: bool,
) -> RealtimeResult<()> {
    if credentials.is_none() && in_bud_mode {
        return Err(RealtimeError::AuthenticationFailed(
            "a Bud deployment's Nova Sonic session is signed with the deployment's own AWS keys; \
             the gateway's AWS identity is never used"
                .to_string(),
        ));
    }
    Ok(())
}

/// The Bedrock client for a dial. With static credentials the config is built from them and
/// the spec ALONE — no environment, no shared config, no instance metadata: in Bud mode not even
/// an `AWS_ENDPOINT_URL` may redirect a request signed with a tenant's keys. Without, the
/// `aws-config` default chain (the native path, as before).
async fn bedrock_client(
    factory: &BedrockBidiTransportFactory,
    region: Option<String>,
) -> BedrockClient {
    match &factory.credentials {
        Some(c) => {
            let mut b = aws_sdk_bedrockruntime::Config::builder()
                .behavior_version(BehaviorVersion::latest())
                .credentials_provider(aws_credential_types::Credentials::new(
                    c.access_key_id.clone(),
                    c.secret_access_key.clone(),
                    c.session_token.clone(),
                    None,
                    "bud-voice-table",
                ))
                .region(region.map(aws_config::Region::new));
            if let Some(url) = &factory.endpoint_url {
                b = b.endpoint_url(url.clone());
            }
            if let Some(h) = &factory.http_client {
                b = b.http_client(h.clone());
            }
            BedrockClient::from_conf(b.build())
        }
        None => {
            let mut loader = aws_config::defaults(BehaviorVersion::latest());
            if let Some(r) = region {
                loader = loader.region(aws_config::Region::new(r));
            }
            if let Some(h) = &factory.http_client {
                loader = loader.http_client(h.clone());
            }
            BedrockClient::new(&loader.load().await)
        }
    }
}

/// Factory for the AWS NOVA SONIC pattern: opens an Amazon Bedrock
/// `InvokeModelWithBidirectionalStream` HTTP/2 bidi event stream and returns a
/// [`BedrockBidiTransport`] over it.
///
/// Implements [`RealtimeTransportFactory`] so the generic driver re-dials it on
/// connection loss exactly like the WS factories — a CONTAINED transport-layer
/// addition; the driver (`session.rs`) is unchanged. The protocol opts in via
/// [`transport_factory`](super::protocol::RealtimeProtocol::transport_factory).
///
/// Mirrors `AwsTranscribeTransport`'s connect path EXACTLY: build the
/// `aws-sdk-bedrockruntime` `Client` from the `aws-config` default credential
/// chain (region from the spec, else the environment), install a channel-fed
/// `async_stream` as the input half, `send()` the request, and hand back the
/// output [`EventReceiver`](aws_sdk_bedrockruntime::primitives::event_stream::EventReceiver)
/// half. Credentials are AWS SigV4 — via the default chain, or (a Bud deployment, FRD-023
/// RT7.2) the deployment's static key pair ([`Self::with_credentials`]). NO api-key.
#[derive(Clone, Default)]
pub struct BedrockBidiTransportFactory {
    credentials: Option<crate::core::realtime::base::AwsStaticCredentials>,
    endpoint_url: Option<String>,
    http_client: Option<aws_smithy_runtime_api::client::http::SharedHttpClient>,
}

impl std::fmt::Debug for BedrockBidiTransportFactory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BedrockBidiTransportFactory")
            .field("static_credentials", &self.credentials.is_some())
            .field("endpoint_url", &self.endpoint_url)
            .finish()
    }
}

impl BedrockBidiTransportFactory {
    /// The `aws-config` default credential chain (the native path).
    pub fn default_chain() -> Self {
        Self::default()
    }

    /// Sign with this key pair and nothing else.
    pub fn with_credentials(
        credentials: crate::core::realtime::base::AwsStaticCredentials,
    ) -> Self {
        Self {
            credentials: Some(credentials),
            ..Self::default()
        }
    }

    /// A Bedrock endpoint other than the region's (already SSRF-validated by the caller).
    pub fn endpoint_url(mut self, url: Option<String>) -> Self {
        self.endpoint_url = url;
        self
    }

    /// The HTTP client the SDK dials with. `None`: the SDK's own. Tests pass an in-process
    /// connector that speaks the Bedrock event stream.
    pub fn http_client(
        mut self,
        client: Option<aws_smithy_runtime_api::client::http::SharedHttpClient>,
    ) -> Self {
        self.http_client = client;
        self
    }
}

#[async_trait]
impl RealtimeTransportFactory for BedrockBidiTransportFactory {
    async fn connect(&self, spec: ConnectSpec) -> RealtimeResult<Box<dyn RealtimeTransport>> {
        let ConnectSpec::BedrockBidi { model_id, region } = spec else {
            return Err(RealtimeError::ConnectionFailed(
                "BedrockBidiTransportFactory only supports ConnectSpec::BedrockBidi".to_string(),
            ));
        };
        bedrock_dial_allowed(
            self.credentials.as_ref(),
            crate::auth::bud_mode::process_in_bud_mode(),
        )?;

        // Bound the client build (a default-chain config load can stall on IMDS) so a
        // misconfigured deployment fails fast instead of stalling the client.
        let client = tokio::time::timeout(BEDROCK_CONNECT_TIMEOUT, bedrock_client(self, region))
            .await
            .map_err(|_| {
                RealtimeError::ConnectionFailed(
                    "Bedrock connect timed out (check AWS credentials/region)".to_string(),
                )
            })?;

        // The input half: a bounded channel whose receiver drives an async stream
        // of union events; the sender is the transport's `send()` surface. (Same
        // channel-fed `async_stream` Transcribe attaches as its audio input — here
        // it is the outbound-frame path.)
        let (input_tx, mut input_rx) = mpsc::channel::<BidiInputEvent>(BEDROCK_INPUT_CHANNEL_DEPTH);
        let input_stream = async_stream::stream! {
            while let Some(event) = input_rx.recv().await {
                yield Ok::<BidiInputEvent, BidiInputError>(event);
            }
        };

        // Open the bidi stream in its own task (see `BedrockOutputState`): the SDK returns
        // from `send()` only at the first output event, which Nova sends only after input.
        let open = tokio::spawn(async move {
            client
                .invoke_model_with_bidirectional_stream()
                .model_id(model_id)
                .body(input_stream.into())
                .send()
                .await
                .map(|output| output.body)
                .map_err(|e| {
                    RealtimeError::ConnectionFailed(format!(
                        "failed to open Bedrock bidirectional stream: {e}"
                    ))
                })
        });

        Ok(Box::new(BedrockBidiTransport {
            input_tx,
            output: BedrockOutputState::Opening(open),
        }) as Box<dyn RealtimeTransport>)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn aws_keys() -> crate::core::realtime::base::AwsStaticCredentials {
        crate::core::realtime::base::AwsStaticCredentials {
            access_key_id: "AKIDBUDDEPLOYMENT1".into(),
            secret_access_key: "deployment-secret".into(),
            session_token: None,
        }
    }

    /// FRD-023 RT7.2 🔒 — in Bud mode a Bedrock dial without the deployment's keys is refused;
    /// the default chain (the gateway's own AWS identity) is for the native path only.
    #[test]
    fn rt7_2_bud_mode_never_dials_bedrock_with_the_gateway_identity() {
        assert!(matches!(
            bedrock_dial_allowed(None, true),
            Err(RealtimeError::AuthenticationFailed(_))
        ));
        assert!(bedrock_dial_allowed(Some(&aws_keys()), true).is_ok());
        assert!(bedrock_dial_allowed(None, false).is_ok());
    }

    /// The key pair never reaches a log line through `Debug`.
    #[test]
    fn aws_static_credentials_debug_is_redacted() {
        let printed = format!("{:?}", aws_keys());
        assert!(!printed.contains("AKIDBUDDEPLOYMENT1"), "{printed}");
        assert!(!printed.contains("deployment-secret"), "{printed}");
        let printed = format!(
            "{:?}",
            BedrockBidiTransportFactory::with_credentials(aws_keys())
        );
        assert!(!printed.contains("deployment-secret"), "{printed}");
    }

    /// FRD-023 RT7.2 🔒 — a factory holding a deployment's keys SIGNS WITH THEM (SigV4, the
    /// `bedrock` service, the deployment's region), whatever the process environment holds.
    #[tokio::test]
    async fn rt7_2_static_credentials_sign_the_bedrock_request() {
        use aws_smithy_runtime_api::client::http::{
            HttpClient, HttpConnector, HttpConnectorFuture, HttpConnectorSettings,
            SharedHttpClient, SharedHttpConnector,
        };
        use aws_smithy_runtime_api::client::orchestrator::HttpRequest;
        use aws_smithy_runtime_api::client::runtime_components::RuntimeComponents;
        use aws_smithy_runtime_api::http::{Response, StatusCode};
        use aws_smithy_types::body::SdkBody;
        use std::sync::{Arc, Mutex};

        #[derive(Debug, Clone, Default)]
        struct Capture(Arc<Mutex<Vec<(String, String)>>>);
        impl HttpConnector for Capture {
            fn call(&self, req: HttpRequest) -> HttpConnectorFuture {
                let auth = req
                    .headers()
                    .get("authorization")
                    .unwrap_or_default()
                    .to_string();
                self.0.lock().unwrap().push((req.uri().to_string(), auth));
                HttpConnectorFuture::new(async move {
                    let mut resp =
                        Response::new(StatusCode::try_from(200u16).unwrap(), SdkBody::empty());
                    resp.headers_mut()
                        .insert("content-type", "application/vnd.amazon.eventstream");
                    Ok(resp)
                })
            }
        }
        impl HttpClient for Capture {
            fn http_connector(
                &self,
                _s: &HttpConnectorSettings,
                _c: &RuntimeComponents,
            ) -> SharedHttpConnector {
                SharedHttpConnector::new(self.clone())
            }
        }

        let capture = Capture::default();
        let factory = BedrockBidiTransportFactory::with_credentials(aws_keys())
            .http_client(Some(SharedHttpClient::new(capture.clone())));
        let mut transport = factory
            .connect(ConnectSpec::BedrockBidi {
                model_id: "amazon.nova-2-sonic-v1:0".into(),
                region: Some("eu-north-1".into()),
            })
            .await
            .expect("connect returns at once; the stream opens in the background");
        // The first read waits for the open (here: an empty stream, so it ends).
        let _ = tokio::time::timeout(std::time::Duration::from_secs(10), transport.recv()).await;
        let seen = capture.0.lock().unwrap().clone();
        let (uri, auth) = seen.first().expect("the SDK sent the request");
        assert!(
            uri.starts_with("https://bedrock-runtime.eu-north-1.amazonaws.com/"),
            "{uri}"
        );
        assert!(auth.starts_with("AWS4-HMAC-SHA256 "), "{auth}");
        assert!(
            auth.contains("Credential=AKIDBUDDEPLOYMENT1/")
                && auth.contains("/eu-north-1/bedrock/aws4_request"),
            "signed with the deployment's key in its region: {auth}"
        );
    }

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

    #[test]
    fn rest_handshake_urls_are_ssrf_checked() {
        let _env = crate::core::net::ssrf_env_lock();
        assert!(validate_rest_handshake_create_url("https://api.ultravox.ai/api/calls").is_ok());
        let err = validate_rest_handshake_create_url("file:///tmp/create")
            .expect_err("non-HTTP create URL must be rejected");
        assert!(err.to_string().contains("not allowed"), "{err}");

        assert_eq!(
            validate_rest_handshake_join_url(" wss://join.example.com/call/abc ").unwrap(),
            "wss://join.example.com/call/abc"
        );
        let err = validate_rest_handshake_join_url("https://join.example.com/call/abc")
            .expect_err("non-WebSocket join URL must be rejected");
        assert!(err.to_string().contains("not allowed"), "{err}");
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

    // =========================================================================
    // AWS Nova Sonic — BedrockBidi transport (pure helpers; the LIVE stream needs
    // AWS creds, so we exercise the wrap/unwrap halves directly here).
    // =========================================================================

    use aws_sdk_bedrockruntime::types::BidirectionalOutputPayloadPart;

    /// The OUTBOUND wrap (`send()`'s transform): an event JSON string becomes a
    /// Bedrock input `Chunk` whose payload `bytes` are EXACTLY the JSON bytes.
    #[test]
    fn wrap_input_payload_carries_event_json_bytes() {
        let json = r#"{"event":{"audioInput":{"content":"AAA="}}}"#.to_string();
        let event = wrap_input_payload(json.clone());
        let part = event
            .as_chunk()
            .expect("wrapped input must be the Chunk variant");
        let bytes = part.bytes().expect("Chunk must carry a bytes blob");
        assert_eq!(
            bytes.as_ref(),
            json.as_bytes(),
            "the payload bytes must be the verbatim event JSON"
        );
    }

    /// The INBOUND unwrap (`recv()`'s transform): a Bedrock output `Chunk` whose
    /// payload `bytes` are an event JSON ⇒ that JSON string back.
    #[test]
    fn unwrap_output_payload_decodes_chunk_json() {
        let json = r#"{"event":{"textOutput":{"content":"hi","role":"ASSISTANT"}}}"#;
        let event = BidiOutputEvent::Chunk(
            BidirectionalOutputPayloadPart::builder()
                .bytes(Blob::new(json.as_bytes().to_vec()))
                .build(),
        );
        assert_eq!(
            unwrap_output_payload(&event).as_deref(),
            Some(json),
            "a Chunk's bytes must decode back to the event JSON string"
        );
    }

    /// ROUND-TRIP: wrap then unwrap is identity for the event JSON. (Input and
    /// output payload parts are distinct SDK types, so this asserts the two pure
    /// halves agree on the byte contract — wrap puts JSON bytes in, unwrap reads
    /// the same byte shape out.)
    #[test]
    fn wrap_then_unwrap_round_trips_via_bytes() {
        let json = r#"{"event":{"contentStart":{"type":"AUDIO"}}}"#.to_string();
        // Wrap → pull the bytes the input Chunk holds.
        let in_event = wrap_input_payload(json.clone());
        let in_bytes = in_event
            .as_chunk()
            .unwrap()
            .bytes()
            .unwrap()
            .as_ref()
            .to_vec();
        // Re-frame those exact bytes as an OUTPUT chunk and unwrap.
        let out_event = BidiOutputEvent::Chunk(
            BidirectionalOutputPayloadPart::builder()
                .bytes(Blob::new(in_bytes))
                .build(),
        );
        assert_eq!(unwrap_output_payload(&out_event), Some(json));
    }

    /// An output payload with no bytes (or a non-Chunk) ⇒ None (the recv loop
    /// skips it rather than surfacing a frame).
    #[test]
    fn unwrap_output_payload_none_when_no_bytes() {
        let empty = BidiOutputEvent::Chunk(BidirectionalOutputPayloadPart::builder().build());
        assert_eq!(unwrap_output_payload(&empty), None);
    }

    /// The Bedrock factory rejects a non-`BedrockBidi` spec (callers must use the
    /// WS factories for WS specs) — symmetric to the WS factory's rejection above,
    /// and reachable WITHOUT AWS creds (the guard returns before any AWS call).
    #[tokio::test]
    async fn bedrock_factory_rejects_non_bedrock_spec() {
        let spec = ConnectSpec::WebSocket {
            url: "wss://example/ws".to_string(),
            headers: vec![],
        };
        assert!(matches!(
            BedrockBidiTransportFactory::default_chain()
                .connect(spec)
                .await,
            Err(RealtimeError::ConnectionFailed(_))
        ));
    }

    // =========================================================================
    // GW-13 (UDS half) — ConnectSpec::Unix domain-socket transport
    // =========================================================================

    /// The UDS frame codec is the byte-exact round-trip that carries BOTH a JSON
    /// control frame (Text) AND a raw audio frame (Binary) over the
    /// kind+length-prefixed stream. (Pure, no socket — the wire contract in
    /// isolation; the accuracy invariant: Binary audio survives byte-for-byte.)
    #[test]
    fn uds_frame_codec_round_trips_text_and_binary() {
        // Text control frame.
        let txt = OutFrame::Text(r#"{"type":"session.config","task":"s2s"}"#.to_string());
        let mut buf = Vec::new();
        encode_uds_frame(&txt, &mut buf).expect("text frame length is in range");
        let (decoded, consumed) = decode_uds_frame(&buf)
            .expect("a full frame is present")
            .expect("and decodes Ok");
        assert_eq!(consumed, buf.len());
        match decoded {
            OutFrame::Text(s) => assert_eq!(s, r#"{"type":"session.config","task":"s2s"}"#),
            OutFrame::Binary(_) => panic!("a Text frame must decode back to Text"),
        }

        // Raw binary audio frame — byte-exact (the accuracy-at-the-seam invariant).
        let audio = bytes::Bytes::from(vec![0xde, 0xad, 0xbe, 0xef, 0x00, 0x40]);
        let bin = OutFrame::Binary(audio.clone());
        let mut buf2 = Vec::new();
        encode_uds_frame(&bin, &mut buf2).expect("binary frame length is in range");
        let (decoded2, _) = decode_uds_frame(&buf2)
            .expect("a full binary frame is present")
            .expect("and decodes Ok");
        match decoded2 {
            OutFrame::Binary(b) => assert_eq!(b, audio, "audio rides UDS byte-exact, no base64"),
            OutFrame::Text(_) => panic!("a Binary frame must decode back to Binary"),
        }

        // A partial buffer (fewer than the prefix or fewer than `len` body bytes)
        // ⇒ None (wait for more), NOT a panic or a torn frame.
        assert!(
            decode_uds_frame(&buf[..2]).is_none(),
            "partial prefix ⇒ wait"
        );
        assert!(
            decode_uds_frame(&buf[..buf.len() - 1]).is_none(),
            "partial body ⇒ wait"
        );
    }

    #[test]
    fn uds_frame_codec_rejects_oversize_lengths() {
        let too_large = UDS_MAX_FRAME_LEN + 1;
        let err = checked_uds_frame_body_len(too_large)
            .expect_err("outbound UDS frames over the transport cap must fail");
        assert!(
            err.to_string().contains("exceeds the"),
            "unexpected outbound error: {err}"
        );

        let mut header = Vec::with_capacity(UDS_HEADER_LEN);
        header.push(UDS_KIND_BINARY);
        header.extend_from_slice(&(too_large as u32).to_be_bytes());
        let err = decode_uds_frame(&header)
            .expect("oversize length is rejected before waiting for a body")
            .expect_err("oversize inbound UDS frame must fail");
        assert!(
            err.to_string().contains("exceeds the"),
            "unexpected inbound error: {err}"
        );
    }

    /// **RED→GREEN `connect_spec_unix_uds_connects`** (GW-13 UDS half, §6.2/§13):
    /// the DEFAULT [`WsTransportFactory`] connects a [`ConnectSpec::Unix`] against a
    /// live unix domain socket and returns a [`RealtimeTransport`] that carries the
    /// S2S frame vocabulary BOTH ways — a JSON control frame OUT and a raw binary
    /// audio frame IN — over the in-box UDS. Bounded by a timeout so a hang FAILS.
    #[tokio::test]
    async fn connect_spec_unix_uds_connects() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            // A unique socket path under the test tmp dir (cleaned by the OS tmp).
            let dir = std::env::temp_dir();
            let path = dir.join(format!("waav_uds_connect_{}.sock", std::process::id()));
            let _ = std::fs::remove_file(&path);
            let listener = tokio::net::UnixListener::bind(&path).expect("bind UDS");

            // A tiny in-box "Infer S2S" server: accept ONE connection, read the
            // first OUTBOUND frame (the session.config control text), then push ONE
            // INBOUND binary audio frame back — exercising both directions.
            let server_path = path.clone();
            let server = tokio::spawn(async move {
                let (mut sock, _) = listener.accept().await.expect("accept");
                // Read the kind byte + length prefix + body of the first frame.
                let mut kind = [0u8; 1];
                sock.read_exact(&mut kind).await.expect("read kind");
                let mut len_buf = [0u8; 4];
                sock.read_exact(&mut len_buf).await.expect("read len");
                let n = u32::from_be_bytes(len_buf) as usize;
                let mut body = vec![0u8; n];
                sock.read_exact(&mut body).await.expect("read body");
                let got_text = String::from_utf8(body).expect("control frame is utf-8");
                // Push back ONE binary audio frame: kind=1, len, body.
                let audio = [0x10u8, 0x20, 0x30, 0x40];
                sock.write_all(&[1u8]).await.expect("write kind");
                sock.write_all(&(audio.len() as u32).to_be_bytes())
                    .await
                    .expect("write len");
                sock.write_all(&audio).await.expect("write body");
                sock.flush().await.expect("flush");
                // keep the socket alive briefly so the client read completes.
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                let _ = std::fs::remove_file(&server_path);
                got_text
            });

            // The DEFAULT factory connects the Unix spec (no separate factory needed
            // — the in-box S2S provider uses the default `transport_factory()`).
            let spec = ConnectSpec::Unix {
                path: path.to_string_lossy().to_string(),
            };
            let mut transport = WsTransportFactory
                .connect(spec)
                .await
                .expect("UDS connect must succeed against a live socket");

            // OUT: send the session.config control frame.
            transport
                .send(OutFrame::Text(
                    r#"{"type":"session.config","task":"s2s"}"#.to_string(),
                ))
                .await
                .expect("send control frame OUT over UDS");

            // IN: the server's binary audio frame arrives byte-exact.
            let inbound = transport
                .recv()
                .await
                .expect("a frame IN")
                .expect("inbound frame is Ok");
            match inbound {
                OutFrame::Binary(b) => {
                    assert_eq!(b.as_ref(), &[0x10, 0x20, 0x30, 0x40], "audio IN byte-exact")
                }
                OutFrame::Text(_) => panic!("expected a binary audio frame IN"),
            }

            transport.close().await;
            let server_got = server.await.expect("server task joined");
            assert!(
                server_got.contains("\"task\":\"s2s\""),
                "the server received the session.config control frame OUT over UDS"
            );
        })
        .await
        .expect("the UDS connect test must complete within the bound (no deadlock)");
    }

    /// Symmetric rejection: the Bedrock factory does not speak UDS (callers use the
    /// default WS factory for `Unix`) — a typed error, reachable without AWS.
    #[tokio::test]
    async fn bedrock_factory_rejects_unix_spec() {
        let spec = ConnectSpec::Unix {
            path: "/tmp/x.sock".to_string(),
        };
        assert!(matches!(
            BedrockBidiTransportFactory::default_chain()
                .connect(spec)
                .await,
            Err(RealtimeError::ConnectionFailed(_))
        ));
    }
}
