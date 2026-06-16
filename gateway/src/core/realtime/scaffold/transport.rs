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
            ConnectSpec::BedrockBidi { .. } => Err(RealtimeError::ConnectionFailed(
                "WsTransportFactory does not support BedrockBidi; \
                 use BedrockBidiTransportFactory"
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
pub struct BedrockBidiTransport {
    /// Outbound input events → the SDK's input event-stream sender (via the
    /// channel-fed `async_stream` installed at connect). Cloned `send()` surface.
    input_tx: mpsc::Sender<BidiInputEvent>,
    /// This connection's OWN output event receiver (owned outright, dropped with
    /// the transport) — the bidi stream's server→client half. (`event_receiver`
    /// is a private module; the public re-export is via `primitives::event_stream`,
    /// exactly as `AwsTranscribeTransport` references its result stream.)
    output_rx: aws_sdk_bedrockruntime::primitives::event_stream::EventReceiver<
        BidiOutputEvent,
        aws_sdk_bedrockruntime::types::error::InvokeModelWithBidirectionalStreamOutputError,
    >,
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
                RealtimeError::ConnectionFailed(
                    "Bedrock bidi input stream closed".to_string(),
                )
            })
    }

    async fn recv(&mut self) -> Option<RealtimeResult<OutFrame>> {
        // Mirror the Transcribe result loop: skip non-payload events, surface a
        // decoded JSON event as a Text frame, map a stream error to Some(Err),
        // and a clean end (`Ok(None)`) to None (no reconnect).
        loop {
            match self.output_rx.recv().await {
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
/// half. Credentials are AWS SigV4 via the default chain — NO api-key.
pub struct BedrockBidiTransportFactory;

#[async_trait]
impl RealtimeTransportFactory for BedrockBidiTransportFactory {
    async fn connect(&self, spec: ConnectSpec) -> RealtimeResult<Box<dyn RealtimeTransport>> {
        let ConnectSpec::BedrockBidi { model_id, region } = spec else {
            return Err(RealtimeError::ConnectionFailed(
                "BedrockBidiTransportFactory only supports ConnectSpec::BedrockBidi".to_string(),
            ));
        };

        // Build the AWS config + Bedrock client from the DEFAULT credential chain
        // (env / shared config / IAM role), exactly like AwsTranscribeTransport.
        // Region: the spec's, else resolved by the loader from the environment.
        let mut loader = aws_config::defaults(BehaviorVersion::latest());
        if let Some(r) = region {
            loader = loader.region(aws_config::Region::new(r));
        }
        let aws_config = loader.load().await;
        let client = BedrockClient::new(&aws_config);

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

        // Open the bidi stream. `.body(stream.into())` + `.send()` mirrors
        // Transcribe's `.audio_stream(stream.into()).send_with(&client)`.
        let output = client
            .invoke_model_with_bidirectional_stream()
            .model_id(model_id)
            .body(input_stream.into())
            .send()
            .await
            .map_err(|e| {
                RealtimeError::ConnectionFailed(format!(
                    "failed to open Bedrock bidirectional stream: {e}"
                ))
            })?;

        Ok(Box::new(BedrockBidiTransport {
            input_tx,
            output_rx: output.body,
        }))
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
        let in_bytes = in_event.as_chunk().unwrap().bytes().unwrap().as_ref().to_vec();
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
            BedrockBidiTransportFactory.connect(spec).await,
            Err(RealtimeError::ConnectionFailed(_))
        ));
    }
}
