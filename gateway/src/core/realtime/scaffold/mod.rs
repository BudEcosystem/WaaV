//! Shared speech-to-speech (S2S) realtime scaffold.
//!
//! A generic [`RealtimeSession`](session::RealtimeSession) driver implements
//! [`BaseRealtime`](super::base::BaseRealtime) ONCE — owning the reconnect loop,
//! barge-in/truncate/preroll, conversation replay, resilience, and callback
//! dispatch — over a [`RealtimeTransport`](transport::RealtimeTransport). Each
//! provider supplies only a small [`RealtimeProtocol`](protocol::RealtimeProtocol)
//! (session config + wire event mapping + audio encoding).
//!
//! See `WaaV/REALTIME_PROVIDERS_GAP.md` and the approved plan. OpenAI + Hume
//! migrate onto this; the fleet (Azure, Gemini, Deepgram, ElevenLabs, …) builds
//! on it via the per-provider template.

mod event;
#[cfg(test)]
mod mock;
mod protocol;
mod session;
mod transport;

pub use event::{ConnectSpec, Inbound, OutFrame, ProtocolCaps, S2sEvent};
pub use protocol::RealtimeProtocol;
pub use session::RealtimeSession;
pub use transport::{
    RealtimeTransport, RealtimeTransportFactory, WsTextTransport, WsTransportFactory,
};
