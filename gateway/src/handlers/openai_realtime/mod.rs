//! `/v1/realtime`: OpenAI Realtime GA on Bud deployments (FRD-023, D-1, D-2).
//!
//! `wss://gateway/v1/realtime?model=<deployment>` speaks the OpenAI Realtime GA protocol, so the
//! unmodified OpenAI SDKs, the Agents SDKs, LiveKit and Pipecat work against it by changing only
//! the base URL and the key. For vendors that already speak GA (OpenAI, Azure OpenAI) frames are
//! RELAYED after policy — full fidelity, no per-event translation. WaaV's native protocol stays
//! on `/realtime`.
//!
//! * [`handshake`] — credential sources and the pre-upgrade rules (§5.2).
//! * [`upstream`] — the vendor URL, auth header and SSRF check (§5.3).
//! * [`policy`] — what crosses the relay, per event (§5.5, §5.6).
//! * [`metering`] — a `voice.turn` per billed record, a `voice.session` per session (§5.10).
//! * [`session`] — the session engine: admission, relay, timers, revalidation, teardown (§5.4).
//! * [`facade`] — the translate engine: GA over WaaV's native providers for vendors without a
//!   GA surface (Gemini Live, Nova 2 Sonic, the per-minute voice agents; §5.7, RT7).
//! * [`client_secrets`] — `POST /v1/realtime/client_secrets`, the `ek_bud_` mint (§5.8).

pub mod client_secrets;
pub mod facade;
pub mod handshake;
pub mod metering;
pub mod policy;
pub mod session;
pub mod upstream;

pub use client_secrets::client_secrets_handler;
pub use session::{RealtimeRuntime, Timings, realtime_ws_handler};

/// The public path (FR-RT-1). The ingress already routes the whole `/v1/realtime` prefix here.
pub const OPENAI_REALTIME_PATH: &str = "/v1/realtime";
/// The client-secret mint path (§5.8).
pub const CLIENT_SECRETS_PATH: &str = "/v1/realtime/client_secrets";
