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
