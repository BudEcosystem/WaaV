//! Prometheus bridge for the `metrics` facade (W-C1 observability).
//!
//! [`crate::core::metrics::ProviderMetrics`] already collects per-provider request/TTFB/error
//! counters as lock-free atomics, but those counters were instantiated *nowhere* and there was
//! no `/metrics` endpoint to scrape them. This module is the missing bridge: it
//!
//! 1. installs a process-global Prometheus recorder exactly once ([`metrics_handle`]), and
//! 2. provides typed emit helpers ([`record_request`], [`observe_ttfb_ms`], [`record_error`],
//!    [`set_circuit_breaker_state`]) that the `ProviderMetrics` facade and the resilience
//!    layer call so every `record_*` simultaneously feeds the in-memory snapshot *and* the
//!    Prometheus exposition served at `GET /metrics`.
//!
//! The series names are the W-C1 / E13 contract:
//!
//! | series                                                  | type      | labels                       |
//! |---------------------------------------------------------|-----------|------------------------------|
//! | `waav_provider_requests_total`                          | counter   | `provider,channel,outcome`   |
//! | `waav_provider_ttfb_ms`                                 | histogram | `provider,channel`           |
//! | `waav_provider_errors_total`                            | counter   | `provider,channel,kind`      |
//! | `waav_circuit_breaker_state`                            | gauge     | `provider`                   |
//!
//! The recorder is global and one-shot: [`set_global_recorder`](metrics::set_global_recorder)
//! can only succeed once per process, so [`metrics_handle`] memoizes the handle in a
//! [`OnceLock`]. Subsequent `AppState::new` calls (tests, embedders) reuse the same handle and
//! the same global recorder — the macros below always record into whatever global recorder is
//! installed, so emissions from every `AppState` land in the one exposition.

use std::sync::OnceLock;

use metrics::{counter, gauge, histogram};
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};

// =============================================================================
// Series names (the E13 contract — keep these stable; dashboards depend on them)
// =============================================================================

/// Total provider requests, labelled `provider`, `channel` (`stt`/`tts`/`realtime`), `outcome`.
pub const REQUESTS_TOTAL: &str = "waav_provider_requests_total";
/// Time-to-first-byte in milliseconds (histogram), labelled `provider`, `channel`.
pub const TTFB_MS: &str = "waav_provider_ttfb_ms";
/// Total provider errors, labelled `provider`, `channel`, `kind` (`error`/`timeout`/`rate_limit`).
pub const ERRORS_TOTAL: &str = "waav_provider_errors_total";
/// Circuit-breaker state gauge (0=closed, 1=half_open, 2=open), labelled `provider`.
pub const CIRCUIT_BREAKER_STATE: &str = "waav_circuit_breaker_state";
/// Total streaming reconnect attempts, labelled `provider`, `outcome`
/// (`success`=reconnected, `failure`=dial/restore failed, `exhausted`=budget spent,
/// `circuit_open`=breaker rejected the attempt). Makes the reconnect path observable (W-C1).
pub const RECONNECTS_TOTAL: &str = "waav_reconnects_total";

/// Histogram buckets (milliseconds) for TTFB. Chosen to straddle realtime voice TTFBs
/// (a good streaming STT/TTS first byte is tens-to-hundreds of ms; the long tail captures
/// cold connects / rate-limited retries).
const TTFB_BUCKETS_MS: &[f64] = &[
    5.0, 10.0, 25.0, 50.0, 75.0, 100.0, 150.0, 200.0, 300.0, 500.0, 750.0, 1000.0, 2000.0, 5000.0,
];

// =============================================================================
// Global recorder
// =============================================================================

static HANDLE: OnceLock<Option<PrometheusHandle>> = OnceLock::new();

/// Install (once) the process-global Prometheus recorder and return a render handle.
///
/// Returns `None` only if a *different* global recorder was already installed by something
/// outside our control (then our emit helpers are no-ops against that foreign recorder, and
/// `/metrics` reports that the exporter is unavailable). On the happy path every call returns
/// `Some(handle)` referring to the single recorder we own.
pub fn metrics_handle() -> Option<PrometheusHandle> {
    HANDLE
        .get_or_init(|| {
            let builder = PrometheusBuilder::new()
                .set_buckets_for_metric(Matcher::Full(TTFB_MS.to_string()), TTFB_BUCKETS_MS)
                .expect("static TTFB bucket list is non-empty and valid");

            match builder.install_recorder() {
                Ok(handle) => {
                    describe_series();
                    prime_series();
                    Some(handle)
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "A global metrics recorder is already installed; \
                         WaaV provider metrics will not be exported at /metrics"
                    );
                    None
                }
            }
        })
        .clone()
}

/// Render the current Prometheus text exposition, or an empty body if the recorder is absent.
pub fn render() -> String {
    metrics_handle().map(|h| h.render()).unwrap_or_default()
}

/// Prime the always-present series with a neutral `provider="none"` zero sample so the metric
/// names render from boot (a Prometheus metric only appears in the exposition once it has at
/// least one sample). The TTFB *histogram* is intentionally NOT primed: it should only appear
/// after a real first-byte observation, so `/metrics` truthfully reflects measured latency.
fn prime_series() {
    // Zero increments create the labelled series without inflating any real count.
    counter!(
        REQUESTS_TOTAL,
        "provider" => "none", "channel" => "none", "outcome" => "none",
    )
    .increment(0);
    counter!(
        ERRORS_TOTAL,
        "provider" => "none", "channel" => "none", "kind" => "none",
    )
    .increment(0);
    // Gauge at the closed-state code (0); a neutral placeholder series.
    gauge!(CIRCUIT_BREAKER_STATE, "provider" => "none").set(0.0);
    counter!(
        RECONNECTS_TOTAL,
        "provider" => "none", "outcome" => "none",
    )
    .increment(0);
}

/// Register human-readable descriptions + units so the exposition carries `# HELP`/`# TYPE`.
fn describe_series() {
    metrics::describe_counter!(
        REQUESTS_TOTAL,
        "Total STT/TTS/realtime provider requests by provider, channel, and outcome"
    );
    metrics::describe_histogram!(
        TTFB_MS,
        metrics::Unit::Milliseconds,
        "Provider time-to-first-byte in milliseconds by provider and channel"
    );
    metrics::describe_counter!(
        ERRORS_TOTAL,
        "Total provider errors by provider, channel, and error kind"
    );
    metrics::describe_gauge!(
        CIRCUIT_BREAKER_STATE,
        "Per-provider circuit-breaker state (0=closed, 1=half_open, 2=open)"
    );
    metrics::describe_counter!(
        RECONNECTS_TOTAL,
        "Total streaming reconnect attempts by provider and outcome \
         (success/failure/exhausted/circuit_open)"
    );
}

// =============================================================================
// Emit helpers (called by ProviderMetrics and the resilience layer)
// =============================================================================

/// Record a terminal request outcome (`success` / `error`) on `waav_provider_requests_total`.
pub fn record_request(provider: &str, channel: &str, outcome: &str) {
    counter!(
        REQUESTS_TOTAL,
        "provider" => provider.to_string(),
        "channel" => channel.to_string(),
        "outcome" => outcome.to_string(),
    )
    .increment(1);
}

/// Observe a TTFB sample (milliseconds) on the `waav_provider_ttfb_ms` histogram.
pub fn observe_ttfb_ms(provider: &str, channel: &str, ttfb_ms: f64) {
    histogram!(
        TTFB_MS,
        "provider" => provider.to_string(),
        "channel" => channel.to_string(),
    )
    .record(ttfb_ms);
}

/// Increment `waav_provider_errors_total` for an error of `kind`
/// (`error` / `timeout` / `rate_limit`).
pub fn record_error(provider: &str, channel: &str, kind: &str) {
    counter!(
        ERRORS_TOTAL,
        "provider" => provider.to_string(),
        "channel" => channel.to_string(),
        "kind" => kind.to_string(),
    )
    .increment(1);
}

/// Publish a provider's circuit-breaker state on `waav_circuit_breaker_state`
/// (0=closed, 1=half_open, 2=open).
pub fn set_circuit_breaker_state(provider: &str, state_code: u8) {
    gauge!(CIRCUIT_BREAKER_STATE, "provider" => provider.to_string()).set(state_code as f64);
}

/// Record a reconnect attempt outcome on `waav_reconnects_total`. `outcome` is one of
/// `success` / `failure` / `exhausted` / `circuit_open`. Emitted from the streaming reconnect
/// path so reconnects are observable (W-C1).
pub fn record_reconnect(provider: &str, outcome: &str) {
    counter!(
        RECONNECTS_TOTAL,
        "provider" => provider.to_string(),
        "outcome" => outcome.to_string(),
    )
    .increment(1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_is_installed_and_renders() {
        // First touch installs the global recorder; subsequent calls reuse it.
        let h = metrics_handle();
        assert!(h.is_some(), "global Prometheus recorder must install");
        // Drive one of each series so the exposition is non-trivial.
        record_request("unit-provider", "tts", "success");
        observe_ttfb_ms("unit-provider", "tts", 42.0);
        record_error("unit-provider", "tts", "timeout");
        set_circuit_breaker_state("unit-provider", 2);
        record_reconnect("unit-provider", "success");

        let text = render();
        assert!(text.contains(REQUESTS_TOTAL), "requests series present");
        assert!(text.contains(TTFB_MS), "ttfb histogram present: {text}");
        assert!(text.contains(ERRORS_TOTAL), "errors series present");
        assert!(
            text.contains(CIRCUIT_BREAKER_STATE),
            "circuit-breaker gauge present"
        );
        assert!(text.contains(RECONNECTS_TOTAL), "reconnects counter present: {text}");
        assert!(
            text.contains("unit-provider"),
            "provider label present in exposition"
        );
    }
}
