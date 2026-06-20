//! Shared S2S scaffold — the normalized event vocabulary, wire frames,
//! capabilities, and connect spec that decouple the generic
//! [`RealtimeSession`](super::session::RealtimeSession) driver from each
//! provider's [`RealtimeProtocol`](super::protocol::RealtimeProtocol).
//!
//! Design: `WaaV/REALTIME_PROVIDERS_GAP.md` + the approved plan. Two independent
//! analyses (the `OpenAIRealtime` struct = 4 provider-specific fields of ~22; the
//! Pipecat S2S source) converged on this split: the driver owns reconnect +
//! barge-in + resilience + callback dispatch; the protocol owns config + wire
//! event mapping + audio encoding.

use bytes::Bytes;

use super::super::base::{FunctionCallRequest, RealtimeError, SpeechEvent, TranscriptRole};

/// One outbound wire frame the transport writes. Providers serialize their typed
/// `Wire` message into this; the transport doesn't need to know the protocol.
#[derive(Debug, Clone)]
pub enum OutFrame {
    /// A UTF-8 text frame (JSON for OpenAI/Azure/Gemini/Grok/Inworld).
    Text(String),
    /// A raw binary frame (audio for Deepgram/ElevenLabs/AssemblyAI).
    Binary(Bytes),
}

impl OutFrame {
    /// Borrow a received frame as an [`Inbound`] for `map_server_event`
    /// (zero-copy). Invalid UTF-8 text is surfaced as a binary payload.
    pub fn as_inbound(&self) -> Inbound<'_> {
        match self {
            OutFrame::Text(s) => Inbound::Text(s),
            OutFrame::Binary(b) => Inbound::Binary(b),
        }
    }
}

/// One inbound wire payload handed to `map_server_event`. Borrowed (zero-copy)
/// from the transport's just-received message. Hume sends BOTH text events and
/// binary audio frames, so both arms exist.
#[derive(Debug, Clone, Copy)]
pub enum Inbound<'a> {
    /// A UTF-8 text payload (a JSON server event).
    Text(&'a str),
    /// A raw binary payload (often audio output).
    Binary(&'a [u8]),
}

/// Static per-provider capabilities the driver needs (replaces the scattered
/// `audio_format` / `emits_user_turn_frames` logic in the bespoke clients).
#[derive(Debug, Clone, Copy)]
pub struct ProtocolCaps {
    /// `false` ⇒ the GATEWAY's turn policy drives commit/response (Pipecat's
    /// `emits_user_turn_frames`). Surfaced through `BaseRealtime::emits_user_turn_frames`.
    pub emits_user_turn_frames: bool,
    /// Bytes per ms of OUTPUT audio, for the barge-in truncate math (PCM16@24k =
    /// 48, G.711@8k = 8, Hume linear16@44.1k = 88).
    pub output_bytes_per_ms: u64,
    /// Output sample rate stamped onto `RealtimeAudioData` delivered to the client.
    pub output_sample_rate: u32,
    /// Does the provider support server-side item truncation (OpenAI yes;
    /// Hume/Deepgram/ElevenLabs no — the driver then only cancels on barge-in)?
    pub supports_truncate: bool,
    /// Does the gateway manage an input buffer it can clear + preroll-replay
    /// (OpenAI manual mode yes; most bespoke providers no)?
    pub supports_input_buffer: bool,
}

impl Default for ProtocolCaps {
    fn default() -> Self {
        // OpenAI GA defaults (the canonical provider).
        Self {
            emits_user_turn_frames: true,
            output_bytes_per_ms: 48,
            output_sample_rate: 24_000,
            supports_truncate: true,
            supports_input_buffer: true,
        }
    }
}

/// How the driver should open (and re-open) the transport for a provider.
/// Extensible: WebSocket + `RestThenWebSocket` + `BedrockBidi` (AWS Nova Sonic),
/// each behind the same `RealtimeTransportFactory`.
#[derive(Debug, Clone)]
pub enum ConnectSpec {
    /// A direct WebSocket upgrade with explicit request headers.
    WebSocket {
        /// Full `wss://…` URL (model/query already embedded by the protocol).
        url: String,
        /// Extra request headers (Authorization, api-key, Sec-WebSocket-Protocol…).
        headers: Vec<(String, String)>,
    },
    /// THE ULTRAVOX PATTERN: a REST "create call" handshake first, then a plain
    /// WebSocket connect to the join URL the handshake returns.
    ///
    /// The transport factory ([`RestHandshakeWsTransportFactory`]) does an HTTP
    /// `POST create_url` with `headers` + `body` (a JSON string), parses the JSON
    /// response, extracts the WS url at `join_url_pointer` (a TOP-LEVEL response
    /// field name, e.g. `"joinUrl"`), then opens a `WsTextTransport` to that url
    /// with NO extra headers (the join url is pre-authed). Re-dialing on
    /// reconnect re-runs the create-call (a fresh single-use join url each time).
    ///
    /// [`RestHandshakeWsTransportFactory`]:
    /// super::transport::RestHandshakeWsTransportFactory
    RestThenWebSocket {
        /// The REST endpoint that mints a call (Ultravox: `https://api.ultravox.ai/api/calls`).
        create_url: String,
        /// Request headers for the POST (Ultravox: the `X-API-Key` auth header).
        headers: Vec<(String, String)>,
        /// The POST body, a pre-serialized JSON string (the create-call payload).
        body: String,
        /// The TOP-LEVEL field name in the JSON response holding the `wss://…` url
        /// to connect (Ultravox: `"joinUrl"`).
        join_url_pointer: String,
    },
    /// THE AWS NOVA SONIC PATTERN: an Amazon Bedrock
    /// `InvokeModelWithBidirectionalStream` HTTP/2 bidi EVENT STREAM (NOT a
    /// WebSocket). The transport factory
    /// ([`BedrockBidiTransportFactory`](super::transport::BedrockBidiTransportFactory))
    /// builds an `aws-sdk-bedrockruntime` `Client` from the standard
    /// [`aws-config`] default credential chain (env / shared config / IAM role —
    /// exactly like `AwsTranscribeTransport`, so NO credentials live in this
    /// spec), opens the bidi stream for `model_id`, and frames each
    /// [`OutFrame::Text`] JSON event into a `BidirectionalInputPayloadPart`
    /// (input half) / unframes each `BidirectionalOutputPayloadPart` back to
    /// [`OutFrame::Text`] (output half). The audio is base64 INSIDE the JSON
    /// events (the Bedrock stream is event-framed; [`OutFrame::Binary`] is unused).
    ///
    /// Auth is AWS SigV4 via the default chain — there is NO api-key field here
    /// (the Nova Sonic protocol does not require one; credentials are resolved at
    /// connect by `aws-config`). Re-dialing on reconnect rebuilds the bidi stream.
    ///
    /// [`aws-config`]: https://docs.rs/aws-config
    BedrockBidi {
        /// The Bedrock model id (Nova Sonic: `amazon.nova-sonic-v1:0`).
        model_id: String,
        /// AWS region (e.g. `us-east-1`). `None` ⇒ resolved from the environment
        /// (`AWS_REGION` / shared config) by the `aws-config` default chain.
        region: Option<String>,
    },
    /// THE IN-BOX UDS PATTERN (GW-13 UDS half, `INFER_GATEWAY_INTEGRATION.md`
    /// §6.2/§13): a co-located WaaV Infer **sidecar** reached over a **unix domain
    /// socket** instead of loopback TCP/WS. The single-box topology (§7): the
    /// gateway and the Infer engine share a host, so a UDS removes the loopback-TCP
    /// hop entirely (RTT ≈ 2.3 µs). The transport factory ([`WsTransportFactory`])
    /// opens the socket and frames each [`OutFrame`] with a 1-byte kind tag
    /// (Text/Binary) + a `u32` big-endian length prefix — the same Text+Binary
    /// frame vocabulary the WS transport carries, so the protocol's wire mapping is
    /// UNCHANGED (raw-binary audio stays byte-exact, JSON control stays Text).
    /// Re-dialing on reconnect reconnects the socket.
    ///
    /// SSRF-safe: `path` is sourced ONLY from the trusted server config (a
    /// `unix://…` `realtime_endpoint_override`), never from a client request.
    ///
    /// [`WsTransportFactory`]: super::transport::WsTransportFactory
    Unix {
        /// The filesystem path of the sidecar's listening unix domain socket
        /// (e.g. `/run/waav-infer/s2s.sock`).
        path: String,
    },
}

/// Apply a SERVER-CONFIG-ONLY realtime endpoint override (SSRF-safe) to a
/// provider's default connect URL.
///
/// `override_url` is
/// [`RealtimeConfig::realtime_endpoint_override`](super::super::base::RealtimeConfig::realtime_endpoint_override),
/// sourced ONLY from the trusted server config (the `<PROVIDER>_REALTIME_URL`
/// env vars), NEVER from the untrusted client request. The contract:
///
/// - `None` / blank / a value that does NOT start with `ws://`|`wss://`
///   ⇒ return `default_url` unchanged (a malformed/non-ws override is ignored so
///   it can never downgrade to an unintended scheme).
/// - a `ws://`|`wss://` override ⇒ use it VERBATIM. EXCEPTION: when the provider
///   carries its auth in the URL QUERY (gemini's `?key=`, hume's `?api_key=` —
///   pass `auth_query = Some("key=…")`) and the override URL has NO `?` of its
///   own, the auth query is appended so the mock/proxy still receives a
///   well-formed connect URL. Header-auth providers pass `auth_query = None`
///   (their headers ride along untouched).
///
/// This lets any provider be pointed at a proxy / self-hosted / gov-cloud
/// deployment / local mock by SERVER configuration alone.
pub fn apply_endpoint_override(
    default_url: &str,
    override_url: Option<&str>,
    auth_query: Option<&str>,
) -> String {
    let ov = match override_url.map(str::trim).filter(|s| !s.is_empty()) {
        Some(o) if o.starts_with("ws://") || o.starts_with("wss://") => o,
        // Unset, blank, or non-ws scheme ⇒ keep the provider's default (no
        // silent scheme downgrade, no SSRF via a bogus override).
        _ => return default_url.to_string(),
    };
    match auth_query {
        // Query-key provider whose override lacks a query ⇒ append the auth query.
        Some(q) if !q.is_empty() && !ov.contains('?') => format!("{ov}?{q}"),
        _ => ov.to_string(),
    }
}

#[cfg(test)]
mod endpoint_override_tests {
    use super::apply_endpoint_override;

    #[test]
    fn unset_keeps_default() {
        assert_eq!(
            apply_endpoint_override("wss://api.example.com/v1", None, None),
            "wss://api.example.com/v1"
        );
        assert_eq!(
            apply_endpoint_override("wss://api.example.com/v1", Some("   "), None),
            "wss://api.example.com/v1"
        );
    }

    #[test]
    fn non_ws_override_is_ignored() {
        // An http(s) or bogus override must NOT replace a wss default.
        assert_eq!(
            apply_endpoint_override("wss://api.example.com/v1", Some("http://evil"), None),
            "wss://api.example.com/v1"
        );
        assert_eq!(
            apply_endpoint_override("wss://api.example.com/v1", Some("not-a-url"), None),
            "wss://api.example.com/v1"
        );
    }

    #[test]
    fn ws_override_used_verbatim() {
        assert_eq!(
            apply_endpoint_override("wss://api.example.com/v1", Some("ws://127.0.0.1:9/x"), None),
            "ws://127.0.0.1:9/x"
        );
    }

    #[test]
    fn query_key_appended_when_override_has_no_query() {
        assert_eq!(
            apply_endpoint_override(
                "wss://gen.example.com/ws?key=K",
                Some("ws://127.0.0.1:9"),
                Some("key=K")
            ),
            "ws://127.0.0.1:9?key=K"
        );
    }

    #[test]
    fn query_key_not_appended_when_override_already_has_query() {
        assert_eq!(
            apply_endpoint_override(
                "wss://gen.example.com/ws?key=K",
                Some("ws://127.0.0.1:9/join?token=abc"),
                Some("key=K")
            ),
            "ws://127.0.0.1:9/join?token=abc"
        );
    }
}

/// The normalized realtime server event. Every provider's `map_server_event`
/// lowers raw wire payloads to a `Vec` of these (a Vec because some providers —
/// e.g. Gemini — bundle several logical frames into one wire message). The
/// driver owns the ONE dispatcher that turns each `S2sEvent` into the existing
/// `BaseRealtime` callbacks + the truncate/replay/pending-call bookkeeping.
#[derive(Debug, Clone)]
pub enum S2sEvent {
    /// The provider acknowledged the session (id for state/logging).
    SessionReady { session_id: Option<String> },
    /// Partial OR final transcript, user OR assistant. Drives `on_transcript`;
    /// a final transcript is appended to the driver's conversation_log.
    Transcript {
        role: TranscriptRole,
        text: String,
        is_final: bool,
        item_id: Option<String>,
    },
    /// One decoded PCM audio chunk (the protocol already base64-decoded it).
    /// Drives `on_audio` AND the driver's playback/truncate bookkeeping.
    Audio {
        data: Bytes,
        item_id: Option<String>,
        response_id: Option<String>,
    },
    /// A VAD edge. Drives `on_speech_event`.
    Speech(SpeechEvent),
    /// A fully-assembled function/tool call. Drives `on_function_call`.
    FunctionCall(FunctionCallRequest),
    /// Pending-call tracking: some providers (OpenAI `OutputItemAdded`) carry the
    /// `call_id`→`name` mapping BEFORE the args-done event. Driver stores it.
    TrackPendingCall { call_id: String, name: String },
    /// Generation finished (NOT playout — the driver must not clear playback
    /// here, per the truncate invariant). Drives `on_response_done`.
    ResponseDone { response_id: String },
    /// A server-side interruption signal (Gemini `interrupted`, Hume
    /// `user_interruption`). The driver runs cancel + truncate.
    InterruptedByServer,
    /// A resumption handle to carry across reconnects (Gemini session-resumption).
    ResumptionHandle(String),
    /// An outbound frame the driver must send in response to an inbound event
    /// (e.g. ElevenLabs ConvAI ping→pong); the dispatcher routes it to the
    /// transport instead of a callback.
    SendFrame(OutFrame),
    /// A provider error. Drives `on_error`; `fatal` exits the receive loop.
    Error(RealtimeError),
    /// Nothing actionable (keepalive, rate-limits, unknown event).
    Ignore,
}
