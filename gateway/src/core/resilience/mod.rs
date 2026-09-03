//! Resilience primitives for streaming providers (W-D2).
//!
//! Two pieces of shared, provider-agnostic infrastructure that turn the existing
//! [`crate::core::websocket::ReconnectionManager`] (backoff/jitter, per-connection) into a
//! production-grade reconnect supervisor:
//!
//! - [`CircuitBreaker`] — a per-provider failure-rate breaker (`Closed → Open → HalfOpen`)
//!   that stops an endless reconnect loop from hammering a down upstream, and exposes its
//!   state for metrics/readiness (W-C1).
//! - [`ReconnectGovernor`] — a process-wide [`tokio::sync::Semaphore`] cap on the number of
//!   *concurrent in-flight reconnects*, so a wide outage can't make every session
//!   reconnect at once and self-inflict a thundering herd.
//!
//! These are consumed by [`crate::core::websocket::reconnectable_stream::ReconnectableStream`]
//! (W-D1) and are designed to be shared via [`crate::core::state::CoreState`].

pub mod circuit_breaker;
pub mod connect;
pub mod infer_reject;
pub mod reconnect_governor;
pub mod registry;

pub use circuit_breaker::{
    CircuitBreaker, CircuitBreakerConfig, CircuitBreakerSnapshot, CircuitState,
};
pub use connect::{FACTORY_CONNECT_TIMEOUT, WS_CONNECT_TIMEOUT};
pub use infer_reject::{
    BreakerVerdict, InferRejectReason, record_infer_connection_closed, record_infer_outcome,
};
pub use reconnect_governor::{ReconnectGovernor, ReconnectPermit};
pub use registry::{ResilienceHandles, ResilienceRegistry};
