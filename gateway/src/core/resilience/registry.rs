//! Process-global resilience registry (W-D2 cross-session wiring).
//!
//! The [`CircuitBreaker`] and [`ReconnectGovernor`] primitives are individually correct, but
//! the prior phase constructed a *fresh* governor + breaker inside every session's connect
//! path. That defeats their entire purpose:
//!
//! - **Storm control** only works if the [`ReconnectGovernor`] cap is shared *process-wide*.
//!   A per-session governor caps that session's own reconnects — which never exceed 1 — so a
//!   thousand sessions reconnecting at once still produce a thousand concurrent dials.
//! - **Provider-level tripping** only works if every session of the same provider shares one
//!   [`CircuitBreaker`]. A per-session breaker can never observe a failure from another
//!   session, so a down upstream is hammered by every live session independently.
//!
//! [`ResilienceRegistry`] fixes both: it owns the **single** governor and a
//! `provider → Arc<CircuitBreaker>` map, is constructed **once** at startup (in
//! [`crate::core::state::CoreState`]), and hands the *same* shared handles to every session of
//! a given provider via [`ResilienceRegistry::handles_for`].

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use super::{CircuitBreaker, ReconnectGovernor};

/// The shared resilience handles a single streaming session uses for its reconnect loop.
///
/// Cheap to [`Clone`] (both fields are `Arc`/`Arc`-backed). Threaded from
/// [`ResilienceRegistry::handles_for`] into a provider's connect path so all sessions of the
/// same provider trip the same breaker, and all sessions process-wide share one governor cap.
#[derive(Clone)]
pub struct ResilienceHandles {
    /// The process-global reconnect governor (one per gateway). Caps concurrent in-flight
    /// reconnects across *all* providers and sessions.
    pub governor: Arc<ReconnectGovernor>,
    /// The per-provider circuit breaker (one per provider, shared by all its sessions). A trip
    /// in one session is visible to every other session of the same provider.
    pub breaker: Arc<CircuitBreaker>,
}

impl std::fmt::Debug for ResilienceHandles {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResilienceHandles")
            .field("governor_cap", &self.governor.max_concurrent())
            .field("breaker_state", &self.breaker.state())
            .finish()
    }
}

/// Owns the single process-global [`ReconnectGovernor`] and a lazily-populated
/// `provider → Arc<CircuitBreaker>` map. Constructed once at startup and shared via
/// [`crate::core::state::CoreState`].
pub struct ResilienceRegistry {
    governor: Arc<ReconnectGovernor>,
    /// provider (lowercased) → its single shared breaker. Lazily created on first request so a
    /// provider that is never used costs nothing.
    breakers: RwLock<HashMap<String, Arc<CircuitBreaker>>>,
}

impl ResilienceRegistry {
    /// Create a registry with the given process-wide reconnect concurrency cap.
    pub fn new(max_concurrent_reconnects: usize) -> Self {
        Self {
            governor: Arc::new(ReconnectGovernor::new(max_concurrent_reconnects)),
            breakers: RwLock::new(HashMap::new()),
        }
    }

    /// The shared process-global governor.
    pub fn governor(&self) -> &Arc<ReconnectGovernor> {
        &self.governor
    }

    /// The single shared breaker for `provider`, creating it on first use. Subsequent calls for
    /// the same provider (case-insensitive) return the *same* `Arc`, so a trip in one session is
    /// visible to every other session of that provider.
    pub fn breaker_for(&self, provider: &str) -> Arc<CircuitBreaker> {
        let key = provider.to_lowercase();
        // Fast path: already present.
        if let Some(b) = self
            .breakers
            .read()
            .expect("resilience breaker map poisoned")
            .get(&key)
        {
            return Arc::clone(b);
        }
        // Slow path: insert under the write lock, re-checking in case of a race. The breaker is
        // stamped with the provider label so every state transition self-publishes the
        // `waav_circuit_breaker_state{provider}` gauge (W-C1) — this is what makes the gauge
        // truthful for the inline Deepgram/AssemblyAI reconnect loops (and every other provider),
        // not just the ones routed through the ReconnectableStream supervisor.
        let mut map = self.breakers.write().expect("resilience breaker map poisoned");
        Arc::clone(map.entry(key.clone()).or_insert_with(|| {
            Arc::new(CircuitBreaker::with_label(
                super::CircuitBreakerConfig::default(),
                key,
            ))
        }))
    }

    /// The bundled shared handles (governor + per-provider breaker) for `provider`, ready to be
    /// threaded into that provider's connect path.
    pub fn handles_for(&self, provider: &str) -> ResilienceHandles {
        ResilienceHandles {
            governor: Arc::clone(&self.governor),
            breaker: self.breaker_for(provider),
        }
    }
}

impl Default for ResilienceRegistry {
    /// A sensible default for a single gateway instance: governor cap 16.
    fn default() -> Self {
        Self::new(16)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_provider_returns_the_same_breaker() {
        let reg = ResilienceRegistry::new(8);
        let a = reg.breaker_for("deepgram");
        let b = reg.breaker_for("Deepgram"); // case-insensitive
        assert!(Arc::ptr_eq(&a, &b), "same provider must share one breaker");
    }

    #[test]
    fn different_providers_get_distinct_breakers() {
        let reg = ResilienceRegistry::new(8);
        let dg = reg.breaker_for("deepgram");
        let aai = reg.breaker_for("assemblyai");
        assert!(
            !Arc::ptr_eq(&dg, &aai),
            "distinct providers must have independent breakers"
        );
    }

    #[test]
    fn trip_in_one_handle_is_visible_to_another_handle_of_same_provider() {
        // THE CROSS-SESSION INVARIANT: two "sessions" (two handle grabs) of the same provider
        // share one breaker, so a trip recorded via one is observed by the other.
        let reg = ResilienceRegistry::new(8);
        let session_a = reg.handles_for("deepgram");
        let session_b = reg.handles_for("deepgram");

        // Session A sees a burst of failures and trips its breaker.
        for _ in 0..10 {
            session_a.breaker.record_failure();
        }
        assert_eq!(
            session_a.breaker.state(),
            crate::core::resilience::CircuitState::Open
        );
        // Session B — a *different* session — sees the open breaker and is denied a reconnect.
        assert!(
            !session_b.breaker.allow_request(),
            "a trip in session A must be visible to session B (shared provider breaker)"
        );
    }

    #[test]
    fn registry_breaker_is_labelled_and_publishes_the_provider_gauge() {
        // The breaker the registry hands out must self-publish the
        // `waav_circuit_breaker_state{provider}` gauge on a trip — this is what closes the gap for
        // the inline Deepgram/AssemblyAI reconnect loops, which grab the registry breaker but never
        // emitted the gauge themselves.
        use crate::core::metrics::bridge;
        let _ = bridge::metrics_handle();

        let reg = ResilienceRegistry::new(8);
        let provider = "registry-gauge-probe";
        let b = reg.breaker_for(provider);
        // Drive it open (default config: min volume 5, threshold 0.5).
        for _ in 0..10 {
            b.record_failure();
        }
        assert_eq!(b.state(), crate::core::resilience::CircuitState::Open);

        let exposition = bridge::render();
        let needle = format!("provider=\"{provider}\"");
        let sample = exposition
            .lines()
            .filter(|l| l.starts_with("waav_circuit_breaker_state") && l.contains(&needle))
            .filter_map(|l| l.rsplit(' ').next())
            .filter_map(|v| v.parse::<f64>().ok())
            .next_back();
        assert_eq!(
            sample,
            Some(2.0),
            "registry-created breaker must publish state-code 2 (open) under its provider label"
        );
    }

    #[test]
    fn governor_cap_is_process_global_across_handles() {
        // Two sessions (even of different providers) share the one governor, so its cap bounds
        // their *combined* in-flight reconnects.
        let reg = ResilienceRegistry::new(8);
        let a = reg.handles_for("deepgram");
        let b = reg.handles_for("assemblyai");
        assert!(
            Arc::ptr_eq(&a.governor, &b.governor),
            "all sessions must share one process-global governor"
        );
        assert_eq!(a.governor.max_concurrent(), 8);
    }
}
