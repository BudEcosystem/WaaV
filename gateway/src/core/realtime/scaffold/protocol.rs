//! `RealtimeProtocol` — the narrow per-provider trait. Everything a realtime
//! vendor *varies*; the [`RealtimeSession`](super::session::RealtimeSession)
//! driver owns everything they *share* (reconnect, barge-in, resilience,
//! callback dispatch, context replay).
//!
//! It is a PURE (non-async) trait: every method maps config/audio/events to/from
//! typed wire messages. All async (connect / send / recv / reconnect) lives in
//! the driver + the [`RealtimeTransport`](super::transport::RealtimeTransport).
//! This keeps providers tiny + trivially unit-testable (serialize a config,
//! parse a captured server event) — the probe-first TDD the plan mandates.

use std::sync::Arc;

use super::super::base::{
    RealtimeConfig, RealtimeResponseOverride, RealtimeResult, RealtimeVerb, ReplayConversationItem,
};
use super::event::{ConnectSpec, Inbound, OutFrame, ProtocolCaps, S2sEvent};
use super::transport::{RealtimeTransportFactory, WsTransportFactory};

/// The per-provider protocol. Implementors are stateless mappers (any session
/// state — accumulated transcript, pending tool calls, playback position — lives
/// in the driver), so every method takes `&self`.
pub trait RealtimeProtocol: Send + Sync + Sized + 'static {
    /// The provider's typed outbound wire message (e.g. OpenAI's `ClientEvent`).
    type Wire: Send + 'static;

    /// Construct the protocol from the generic config (validate the API key, parse
    /// model/voice). Lets `RealtimeSession<P>` implement `BaseRealtime::new`.
    fn from_config(cfg: &RealtimeConfig) -> RealtimeResult<Self>;

    /// The transport factory for this provider. Default = a plain WebSocket;
    /// REST-handshake (Ultravox) and Bedrock-bidi (AWS) override it.
    fn transport_factory(&self) -> Arc<dyn RealtimeTransportFactory> {
        Arc::new(WsTransportFactory)
    }

    /// Stable provider id ("openai", "azure", "deepgram", …) — used for the
    /// circuit-breaker label, metrics, and `get_provider_info`.
    fn provider_id(&self) -> &'static str;

    /// Static capabilities (audio format, server-VAD turn frames, truncate/buffer
    /// support). Drives the driver's mode-aware barge-in + audio stamping.
    fn caps(&self) -> ProtocolCaps;

    /// The PCM16 sample rate the vendor expects on INPUT (FRD-023 §5.7: a GA client sends
    /// 24 kHz; the translate engine resamples to this). Default: the GA rate.
    fn input_sample_rate(&self) -> u32 {
        24_000
    }

    /// The longest a single vendor connection may live (Nova Sonic: 8 minutes). The driver
    /// reconnects proactively before it — at a turn boundary where it can — so the session
    /// outlives the connection. `None`: no cap.
    fn max_connection(&self) -> Option<std::time::Duration> {
        None
    }

    /// Build the connect target (URL + headers) from config. Sync: any async
    /// handshake (Ultravox REST-create, AWS SigV4) is the transport factory's job.
    fn connect_spec(&self, cfg: &RealtimeConfig) -> RealtimeResult<ConnectSpec>;

    /// The initial session-config wire message(s), sent on connect and re-sent on
    /// reconnect. `resumption` is `Some` after a reconnect for session/SDK
    /// providers (Gemini). MUST use `skip_serializing_if` on every optional field
    /// — these APIs kill the session on unknown/explicit-null keys.
    fn build_session_config(
        &self,
        cfg: &RealtimeConfig,
        resumption: Option<&str>,
    ) -> Vec<Self::Wire>;

    /// The wire of a client's mid-session update from `old` to `new` (the merged config).
    /// Default: the whole session config again. `None`: the provider cannot change this on an
    /// open connection, and the update is refused (the session keeps its config).
    fn session_update(
        &self,
        old: &RealtimeConfig,
        new: &RealtimeConfig,
    ) -> Option<Vec<Self::Wire>> {
        let _ = old;
        Some(self.build_session_config(new, None))
    }

    /// A mid-session update from `old` to `new` that the provider takes only on a new
    /// connection resuming the session (Gemini Live, fix `gemini_update_resume`): the driver
    /// keeps the merged config and reconnects with its resumption handle at the next turn
    /// boundary, instead of sending [`session_update`](Self::session_update). Default: never.
    fn update_by_reconnect(&self, old: &RealtimeConfig, new: &RealtimeConfig) -> bool {
        let _ = (old, new);
        false
    }

    /// The driver sends nothing after the session config until the provider's
    /// [`SessionReady`](S2sEvent::SessionReady) on that connection (Gemini Live's
    /// `setupComplete`, fix `gemini_await_setup`). Default: it sends at once.
    fn awaits_session_ready(&self) -> bool {
        false
    }

    /// The model the provider says the session runs (xAI's `session.created` / `.updated`
    /// `session.model`): `None` when the protocol reads none, `Some("")` until the provider
    /// has named it. Reported in `get_provider_info` (and the `/realtime` `session_created`).
    fn named_model(&self) -> Option<String> {
        None
    }

    /// Lower one raw inbound payload to zero-or-more normalized events. Returns a
    /// `Vec` because some providers bundle multiple logical frames per wire
    /// message (Gemini). Pure: session bookkeeping happens in the driver.
    fn map_server_event(&self, raw: Inbound<'_>) -> Vec<S2sEvent>;

    /// Encode a user-audio PCM chunk to a wire message (base64+JSON append for
    /// OpenAI/Azure/Gemini; a raw binary frame for Deepgram/ElevenLabs).
    fn encode_user_audio(&self, pcm: &[u8]) -> Self::Wire;

    /// A plain text user message → wire (OpenAI `conversation.item.create`).
    fn send_text(&self, text: &str) -> Vec<Self::Wire>;

    /// Request a response, optionally with a per-response override → wire
    /// (OpenAI `response.create` with a `response` object).
    fn create_response(&self, overrides: Option<&RealtimeResponseOverride>) -> Vec<Self::Wire>;

    /// Manual-mode turn close (OpenAI `input_audio_buffer.commit`). Empty when the
    /// provider owns turn-taking via server VAD.
    fn commit_turn(&self) -> Vec<Self::Wire>;

    /// Cancel the in-flight response (OpenAI `response.cancel`; Hume `StopAssistant`).
    fn cancel_response(&self) -> Vec<Self::Wire>;

    /// Server-side truncate of a partially-heard item (OpenAI
    /// `conversation.item.truncate`). Default empty ⇒ provider can't truncate.
    fn truncate(&self, _item_id: &str, _audio_end_ms: u64) -> Vec<Self::Wire> {
        Vec::new()
    }

    /// Clear the pending input audio buffer (manual-mode barge-in step). Default
    /// empty ⇒ provider has no client-cleared input buffer.
    fn clear_input_buffer(&self) -> Vec<Self::Wire> {
        Vec::new()
    }

    /// A tool-call result → wire (OpenAI `function_call_output` item).
    fn format_tool_result(&self, call_id: &str, result: &str) -> Vec<Self::Wire>;

    /// Replay one logged conversation item as a wire message WITHOUT requesting a
    /// response (post-reconnect context rebuild). Default empty ⇒ no replay.
    fn replay_item(&self, _item: &ReplayConversationItem) -> Vec<Self::Wire> {
        Vec::new()
    }

    /// Sent on every connect after the session config and the replayed history (Nova Sonic,
    /// fix `nova_history_before_audio`: the audio content opens after the history). Default
    /// empty.
    fn after_replay(&self) -> Vec<Self::Wire> {
        Vec::new()
    }

    /// Sent before a connection is closed on purpose: a disconnect or a planned replacement
    /// (Nova Sonic, fix `nova_close_sequence`: audio `contentEnd`, `promptEnd`, `sessionEnd`).
    /// Default empty.
    fn close_wires(&self) -> Vec<Self::Wire> {
        Vec::new()
    }

    /// One interaction verb → wire. `None` ⇒ the provider has no wire for this verb (the
    /// default for every verb); the driver then refuses it instead of sending nothing.
    fn verb(&self, _verb: &RealtimeVerb<'_>) -> Option<Vec<Self::Wire>> {
        None
    }

    /// Serialize one wire message for the transport: `Text(JSON)` for JSON
    /// providers, `Binary` for raw-frame providers.
    fn serialize(&self, msg: &Self::Wire) -> RealtimeResult<OutFrame>;
}
