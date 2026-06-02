//! Provider metrics for performance monitoring.
//!
//! This module provides:
//! - [`ProviderMetrics`] - Per-provider metrics including TTFB
//! - [`RequestTimer`] - RAII timer for request tracking
//! - [`ProviderMetricsSnapshot`] - Point-in-time metrics snapshot
//! - [`bridge`] - the Prometheus exporter bridge (W-C1): installs the global recorder, renders
//!   the `/metrics` exposition, and provides the emit helpers `ProviderMetrics` calls.
//! - [`MetricsRegistry`] - the per-provider [`ProviderMetrics`] holder shared via
//!   [`crate::core::state::CoreState`], so every STT/TTS/realtime request can be timed against a
//!   stable, named metrics instance instead of an ad-hoc one created per request.

pub mod bridge;
pub mod provider;

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;

pub use provider::{ProviderMetrics, ProviderMetricsSnapshot, RequestTimer};

/// Channel labels for [`MetricsRegistry::provider`].
pub mod channel {
    /// Speech-to-text channel label.
    pub const STT: &str = "stt";
    /// Text-to-speech channel label.
    pub const TTS: &str = "tts";
    /// Realtime audio-to-audio channel label.
    pub const REALTIME: &str = "realtime";
}

/// Process-shared holder of per-(provider, channel) [`ProviderMetrics`].
///
/// One registry is created in [`crate::core::state::CoreState`] and shared across all handlers.
/// [`MetricsRegistry::provider`] lazily creates (and memoizes) a `ProviderMetrics` for a given
/// `(provider, channel)` pair, so request timing is attributed to a single stable instance whose
/// `record_*` calls feed both the in-memory snapshot and the Prometheus exposition.
#[derive(Default)]
pub struct MetricsRegistry {
    providers: RwLock<HashMap<(String, String), Arc<ProviderMetrics>>>,
}

impl MetricsRegistry {
    /// Create an empty registry and ensure the global Prometheus recorder is installed so that
    /// `/metrics` renders (and `# HELP`/`# TYPE` lines are registered) even before the first
    /// request is timed.
    pub fn new() -> Self {
        // Install the global recorder eagerly so the exporter is live from boot.
        let _ = bridge::metrics_handle();
        Self {
            providers: RwLock::new(HashMap::new()),
        }
    }

    /// Get (creating on first use) the metrics for a `(provider, channel)` pair.
    pub fn provider(&self, provider: &str, channel: &str) -> Arc<ProviderMetrics> {
        let key = (provider.to_string(), channel.to_string());
        if let Some(m) = self.providers.read().get(&key) {
            return m.clone();
        }
        let mut w = self.providers.write();
        w.entry(key)
            .or_insert_with(|| Arc::new(ProviderMetrics::with_channel(provider, channel)))
            .clone()
    }

    /// Render the current Prometheus text exposition for the `/metrics` endpoint.
    pub fn render(&self) -> String {
        bridge::render()
    }

    /// Publish a provider's circuit-breaker state code (0=closed,1=half_open,2=open).
    pub fn set_circuit_breaker_state(&self, provider: &str, state_code: u8) {
        bridge::set_circuit_breaker_state(provider, state_code);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_memoizes_per_provider_channel() {
        let reg = MetricsRegistry::new();
        let a = reg.provider("deepgram", channel::STT);
        let b = reg.provider("deepgram", channel::STT);
        let c = reg.provider("deepgram", channel::TTS);
        assert!(Arc::ptr_eq(&a, &b), "same (provider,channel) is memoized");
        assert!(
            !Arc::ptr_eq(&a, &c),
            "different channel yields a distinct instance"
        );
        assert_eq!(a.channel(), channel::STT);
        assert_eq!(c.channel(), channel::TTS);
    }

    #[test]
    fn registry_renders_exposition() {
        let reg = MetricsRegistry::new();
        let m = reg.provider("render-test", channel::TTS);
        let mut t = m.start_request();
        t.record_first_byte();
        t.finish();
        let text = reg.render();
        assert!(text.contains("waav_provider_requests_total"));
    }
}
