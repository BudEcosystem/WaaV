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

// -----------------------------------------------------------------------------
// Live latency-profiling series (per-turn + per-frame realtime budget).
// `path` ∈ {conversation,dag}; `stage`/`outcome`/`queue` are fixed enums; `node`
// is bounded by the DAG. NEVER put session_id/turn_id in labels (unbounded) —
// those ride the per-turn `tracing` event instead.
// -----------------------------------------------------------------------------

/// Headline user-perceived latency: end-of-speech → first response audio (histogram, ms),
/// labelled `path`.
pub const TURN_RESPONSE_LATENCY_MS: &str = "waav_turn_response_latency_ms";
/// Per-stage delta of the response pipeline (histogram, ms), labelled `stage`, `path`.
pub const TURN_STAGE_MS: &str = "waav_turn_stage_ms";
/// Smart-turn ONNX inference cost (histogram, ms). The realtime blocker on the audio hot path.
pub const SMART_TURN_INFERENCE_MS: &str = "waav_smart_turn_inference_ms";
/// Frames dropped because the STT write lock was contended (counter).
pub const SMART_TURN_FRAME_SKIPS_TOTAL: &str = "waav_smart_turn_frame_skips_total";
/// LLM time-to-first-token (histogram, ms), labelled `path`.
pub const LLM_TTFT_MS: &str = "waav_llm_ttft_ms";
/// TTS time-to-first-byte from the turn's perspective (histogram, ms), labelled `path`.
pub const TTS_TTFB_MS: &str = "waav_tts_ttfb_ms";
/// Total turns, labelled `path`, `outcome` (`completed`/`aborted`) (counter).
pub const TURNS_TOTAL: &str = "waav_turns_total";
/// Dominant-stage tally for completed turns, labelled `stage` (counter).
pub const TURN_BOTTLENECK_TOTAL: &str = "waav_turn_bottleneck_total";
/// Per-DAG-node wall time (histogram, ms), labelled `node`.
pub const DAG_NODE_MS: &str = "waav_dag_node_ms";
/// Per-frame stage budget (histogram, ms), labelled `stage`
/// (`receive`/`decode`/`vad`/`smart_turn`/`stt_send`).
pub const FRAME_STAGE_MS: &str = "waav_frame_stage_ms";
/// Total audio frames ingested (counter) — denominator for the frame-skip rate.
pub const FRAME_TOTAL: &str = "waav_frame_total";
/// Egress queue depth (gauge), labelled `queue` (`ws_msg`/`livekit_op`).
pub const QUEUE_DEPTH: &str = "waav_queue_depth";
/// Egress queue latency in ms (gauge), labelled `queue`.
pub const QUEUE_LATENCY_MS: &str = "waav_queue_latency_ms";
/// TTS payload format ≠ declared format (counter), labelled `provider` +
/// `source` (`http`/`cache`). Non-zero means a provider config is wrong or a
/// pre-sniffing cache entry was healed (P0.1).
pub const TTS_FORMAT_MISMATCH_TOTAL: &str = "waav_tts_format_mismatch_total";

/// Histogram buckets (milliseconds) for TTFB. Chosen to straddle realtime voice TTFBs
/// (a good streaming STT/TTS first byte is tens-to-hundreds of ms; the long tail captures
/// cold connects / rate-limited retries).
const TTFB_BUCKETS_MS: &[f64] = &[
    5.0, 10.0, 25.0, 50.0, 75.0, 100.0, 150.0, 200.0, 300.0, 500.0, 750.0, 1000.0, 2000.0, 5000.0,
];

/// Headline response-latency buckets (ms): wide, spanning a snappy 50ms turn to a 10s stall.
const TURN_LATENCY_BUCKETS_MS: &[f64] = &[
    50.0, 100.0, 200.0, 300.0, 500.0, 750.0, 1000.0, 1500.0, 2000.0, 3000.0, 5000.0, 10000.0,
];

/// Per-stage / per-DAG-node delta buckets (ms): one stage of a turn.
const STAGE_BUCKETS_MS: &[f64] = &[
    5.0, 10.0, 25.0, 50.0, 100.0, 200.0, 300.0, 500.0, 750.0, 1000.0, 2000.0, 5000.0,
];

/// LLM time-to-first-token buckets (ms).
const LLM_TTFT_BUCKETS_MS: &[f64] = &[
    50.0, 100.0, 200.0, 300.0, 500.0, 750.0, 1000.0, 1500.0, 2000.0, 3000.0, 5000.0,
];

/// TTS time-to-first-byte buckets (ms).
const TTS_TTFB_BUCKETS_MS: &[f64] = &[
    25.0, 50.0, 75.0, 100.0, 150.0, 200.0, 300.0, 500.0, 750.0, 1000.0, 2000.0,
];

/// Smart-turn ONNX inference buckets (ms): tight — this runs on the audio hot path and a
/// few ms of regression is a realtime regression.
const SMART_TURN_BUCKETS_MS: &[f64] = &[2.0, 5.0, 8.0, 10.0, 15.0, 20.0, 30.0, 50.0, 100.0];

/// Per-frame stage buckets (ms): sub-ms to tens of ms — the realtime per-frame budget.
const FRAME_BUCKETS_MS: &[f64] = &[0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0];

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
                .expect("static TTFB bucket list is non-empty and valid")
                .set_buckets_for_metric(
                    Matcher::Full(TURN_RESPONSE_LATENCY_MS.to_string()),
                    TURN_LATENCY_BUCKETS_MS,
                )
                .expect("static turn-latency bucket list is valid")
                .set_buckets_for_metric(Matcher::Full(TURN_STAGE_MS.to_string()), STAGE_BUCKETS_MS)
                .expect("static stage bucket list is valid")
                .set_buckets_for_metric(Matcher::Full(DAG_NODE_MS.to_string()), STAGE_BUCKETS_MS)
                .expect("static dag-node bucket list is valid")
                .set_buckets_for_metric(Matcher::Full(LLM_TTFT_MS.to_string()), LLM_TTFT_BUCKETS_MS)
                .expect("static llm-ttft bucket list is valid")
                .set_buckets_for_metric(Matcher::Full(TTS_TTFB_MS.to_string()), TTS_TTFB_BUCKETS_MS)
                .expect("static tts-ttfb bucket list is valid")
                .set_buckets_for_metric(
                    Matcher::Full(SMART_TURN_INFERENCE_MS.to_string()),
                    SMART_TURN_BUCKETS_MS,
                )
                .expect("static smart-turn bucket list is valid")
                .set_buckets_for_metric(Matcher::Full(FRAME_STAGE_MS.to_string()), FRAME_BUCKETS_MS)
                .expect("static frame bucket list is valid");

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

    // Latency-profiling counters/gauges: prime so names render from boot. Histograms are
    // intentionally NOT primed (they should appear only after a real measurement).
    counter!(TURNS_TOTAL, "path" => "none", "outcome" => "none").increment(0);
    for s in crate::core::observability::Stage::RESPONSE_STAGES {
        counter!(TURN_BOTTLENECK_TOTAL, "stage" => s.as_str()).increment(0);
    }
    counter!(SMART_TURN_FRAME_SKIPS_TOTAL).increment(0);
    counter!(FRAME_TOTAL).increment(0);
    gauge!(QUEUE_DEPTH, "queue" => "none").set(0.0);
    gauge!(QUEUE_LATENCY_MS, "queue" => "none").set(0.0);
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

    // --- Latency profiling ---
    metrics::describe_histogram!(
        TURN_RESPONSE_LATENCY_MS,
        metrics::Unit::Milliseconds,
        "User-perceived turn latency (end-of-speech to first response audio) by path"
    );
    metrics::describe_histogram!(
        TURN_STAGE_MS,
        metrics::Unit::Milliseconds,
        "Per-stage delta of the response pipeline by stage and path"
    );
    metrics::describe_histogram!(
        SMART_TURN_INFERENCE_MS,
        metrics::Unit::Milliseconds,
        "Smart-turn ONNX inference cost on the audio hot path"
    );
    metrics::describe_counter!(
        SMART_TURN_FRAME_SKIPS_TOTAL,
        "Audio frames dropped due to STT write-lock contention"
    );
    metrics::describe_histogram!(
        LLM_TTFT_MS,
        metrics::Unit::Milliseconds,
        "LLM time-to-first-token by path"
    );
    metrics::describe_histogram!(
        TTS_TTFB_MS,
        metrics::Unit::Milliseconds,
        "TTS time-to-first-byte (turn perspective) by path"
    );
    metrics::describe_counter!(TURNS_TOTAL, "Total turns by path and outcome");
    metrics::describe_counter!(
        TURN_BOTTLENECK_TOTAL,
        "Dominant-stage tally for completed turns"
    );
    metrics::describe_histogram!(
        DAG_NODE_MS,
        metrics::Unit::Milliseconds,
        "Per-DAG-node wall time by node id"
    );
    metrics::describe_histogram!(
        FRAME_STAGE_MS,
        metrics::Unit::Milliseconds,
        "Per-audio-frame stage budget by stage (receive/decode/vad/smart_turn/stt_send)"
    );
    metrics::describe_counter!(FRAME_TOTAL, "Total audio frames ingested");
    metrics::describe_gauge!(QUEUE_DEPTH, "Egress queue depth by queue");
    metrics::describe_gauge!(
        QUEUE_LATENCY_MS,
        metrics::Unit::Milliseconds,
        "Egress queue latency by queue"
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

// -----------------------------------------------------------------------------
// Latency-profiling emit helpers (called by LatencyProfiler / FrameProfiler).
// `path`, `stage`, `outcome`, `queue` are fixed `&'static str` enums — no alloc.
// -----------------------------------------------------------------------------

/// Observe the headline turn latency (ms) on `waav_turn_response_latency_ms`.
pub fn observe_turn_response_latency_ms(path: &'static str, ms: f64) {
    histogram!(TURN_RESPONSE_LATENCY_MS, "path" => path).record(ms);
}

/// Observe a per-stage delta (ms) on `waav_turn_stage_ms`.
pub fn observe_turn_stage_ms(stage: &'static str, path: &'static str, ms: f64) {
    histogram!(TURN_STAGE_MS, "stage" => stage, "path" => path).record(ms);
}

/// Observe a smart-turn inference sample (ms) on `waav_smart_turn_inference_ms`.
pub fn observe_smart_turn_inference_ms(ms: f64) {
    histogram!(SMART_TURN_INFERENCE_MS).record(ms);
}

/// Increment `waav_smart_turn_frame_skips_total`.
pub fn record_smart_turn_frame_skip() {
    counter!(SMART_TURN_FRAME_SKIPS_TOTAL).increment(1);
}

/// Observe LLM time-to-first-token (ms) on `waav_llm_ttft_ms`.
pub fn observe_llm_ttft_ms(path: &'static str, ms: f64) {
    histogram!(LLM_TTFT_MS, "path" => path).record(ms);
}

/// Observe TTS time-to-first-byte (ms) on `waav_tts_ttfb_ms`.
pub fn observe_tts_ttfb_ms(path: &'static str, ms: f64) {
    histogram!(TTS_TTFB_MS, "path" => path).record(ms);
}

/// Increment `waav_turns_total` for a terminal turn outcome.
pub fn record_turn_outcome(path: &'static str, outcome: &'static str) {
    counter!(TURNS_TOTAL, "path" => path, "outcome" => outcome).increment(1);
}

/// Increment `waav_turn_bottleneck_total` for the dominant stage of a completed turn.
pub fn record_turn_bottleneck(stage: &'static str) {
    counter!(TURN_BOTTLENECK_TOTAL, "stage" => stage).increment(1);
}

/// Count a TTS payload whose sniffed format differs from the declared one
/// (`source` ∈ http|cache). Provider label is bounded by the provider fleet.
pub fn record_tts_format_mismatch(provider: &str, source: &'static str) {
    counter!(TTS_FORMAT_MISMATCH_TOTAL, "provider" => provider.to_string(), "source" => source)
        .increment(1);
}

/// Observe a per-DAG-node wall time (ms) on `waav_dag_node_ms`.
///
/// Cardinality guard: node ids are operator-authored but still free-form
/// strings — clamp to 48 chars so a pathological/generated id cannot mint
/// unbounded label values (Prometheus memory growth is per unique label).
pub fn observe_dag_node_ms(node: String, ms: f64) {
    let node = if node.len() > 48 {
        node.chars().take(48).collect()
    } else {
        node
    };
    histogram!(DAG_NODE_MS, "node" => node).record(ms);
}

/// Observe a per-frame stage budget sample (ms) on `waav_frame_stage_ms`.
pub fn observe_frame_stage_ms(stage: &'static str, ms: f64) {
    histogram!(FRAME_STAGE_MS, "stage" => stage).record(ms);
}

/// Increment `waav_frame_total`.
pub fn record_frame() {
    counter!(FRAME_TOTAL).increment(1);
}

/// Publish an egress queue depth on `waav_queue_depth`.
pub fn set_queue_depth(queue: &'static str, depth: u64) {
    gauge!(QUEUE_DEPTH, "queue" => queue).set(depth as f64);
}

/// Publish an egress queue latency (ms) on `waav_queue_latency_ms`.
pub fn set_queue_latency_ms(queue: &'static str, ms: u64) {
    gauge!(QUEUE_LATENCY_MS, "queue" => queue).set(ms as f64);
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

    #[test]
    fn profiling_series_render() {
        let _ = metrics_handle();
        observe_turn_response_latency_ms("conversation", 612.0);
        observe_turn_stage_ms("llm_ttft", "conversation", 480.0);
        observe_smart_turn_inference_ms(18.0);
        record_smart_turn_frame_skip();
        observe_llm_ttft_ms("conversation", 480.0);
        observe_tts_ttfb_ms("conversation", 90.0);
        record_turn_outcome("conversation", "completed");
        record_turn_bottleneck("llm_ttft");
        observe_dag_node_ms("llm".to_string(), 480.0);
        observe_frame_stage_ms("smart_turn", 12.0);
        record_frame();
        set_queue_depth("ws_msg", 3);
        set_queue_latency_ms("livekit_op", 7);

        let text = render();
        for series in [
            TURN_RESPONSE_LATENCY_MS,
            TURN_STAGE_MS,
            SMART_TURN_INFERENCE_MS,
            SMART_TURN_FRAME_SKIPS_TOTAL,
            LLM_TTFT_MS,
            TTS_TTFB_MS,
            TURNS_TOTAL,
            TURN_BOTTLENECK_TOTAL,
            DAG_NODE_MS,
            FRAME_STAGE_MS,
            FRAME_TOTAL,
            QUEUE_DEPTH,
            QUEUE_LATENCY_MS,
        ] {
            assert!(text.contains(series), "series `{series}` must render: {text}");
        }
        assert!(text.contains("path=\"conversation\""), "path label rendered");
        assert!(text.contains("stage=\"llm_ttft\""), "stage label rendered");
    }
}
