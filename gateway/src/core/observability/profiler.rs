//! Process-wide latency hub: aggregates every [`TurnTrace`], emits Prometheus +
//! a structured per-turn tracing event, keeps rolling percentiles / a recent-slow
//! ring / a bottleneck tally, and (only when something is listening) broadcasts
//! each completed turn for the live SSE stream.
//!
//! One [`LatencyProfiler`] lives on `CoreState`; every per-session [`TurnProfiler`]
//! hands its closed traces here via the [`TurnSink`] impl. The per-frame
//! [`FrameProfiler`] feeds the hot-path budget straight to Prometheus + the
//! lock-free frame counters used for the frame-skip rate.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use parking_lot::Mutex;
use serde::Serialize;
use tokio::sync::broadcast;

use crate::config::utils::parse_bool;
use crate::core::metrics::bridge;
use crate::core::observability::observer::VoiceObserver;
use crate::core::observability::turn_profile::{Stage, TurnSink, TurnSummary, TurnTrace};

const N_STAGES: usize = 7;
const RECENT_SLOW_CAP: usize = 32;
const BROADCAST_CAP: usize = 256;
/// A turn whose headline latency is at/above this is kept in the recent-slow ring.
const SLOW_THRESHOLD_MS: u64 = 1000;
/// Rolling-window depth for percentiles.
const WINDOW_SAMPLES: usize = 1024;

// =============================================================================
// RollingWindow — bounded rolling percentile window (lifted from latency.rs,
// + p90; operates under an external lock so no internal atomics).
// =============================================================================

/// A bounded rolling window of ns samples with avg/p50/p90/p99/min/max.
#[derive(Debug)]
pub struct RollingWindow {
    samples: VecDeque<u64>,
    max_samples: usize,
    sum_ns: u64,
    total_count: u64,
    min_ns: u64,
    max_ns: u64,
}

impl RollingWindow {
    pub fn new(max_samples: usize) -> Self {
        Self {
            samples: VecDeque::with_capacity(max_samples.min(1024)),
            max_samples: max_samples.max(1),
            sum_ns: 0,
            total_count: 0,
            min_ns: u64::MAX,
            max_ns: 0,
        }
    }

    /// Record a ns sample, evicting the oldest if at capacity.
    pub fn record(&mut self, v_ns: u64) {
        if self.samples.len() >= self.max_samples
            && let Some(old) = self.samples.pop_front()
        {
            self.sum_ns = self.sum_ns.saturating_sub(old);
        }
        self.samples.push_back(v_ns);
        self.sum_ns = self.sum_ns.saturating_add(v_ns);
        self.total_count += 1;
        self.min_ns = self.min_ns.min(v_ns);
        self.max_ns = self.max_ns.max(v_ns);
    }

    /// Total samples ever recorded (not just the window depth).
    pub fn count(&self) -> u64 {
        self.total_count
    }

    /// Compute rolling stats (in ms) over the current window.
    pub fn stats(&self) -> WindowStats {
        if self.samples.is_empty() {
            return WindowStats::default();
        }
        let window_len = self.samples.len() as u64;
        let avg_ns = self.sum_ns / window_len;
        let mut sorted: Vec<u64> = self.samples.iter().copied().collect();
        sorted.sort_unstable();
        let pick = |q: f64| -> u64 {
            let idx = ((sorted.len() as f64 * q) as usize).min(sorted.len() - 1);
            sorted[idx]
        };
        let to_ms = |ns: u64| ns / 1_000_000;
        WindowStats {
            count: self.total_count,
            avg_ms: to_ms(avg_ns),
            p50_ms: to_ms(pick(0.50)),
            p90_ms: to_ms(pick(0.90)),
            p99_ms: to_ms(pick(0.99)),
            min_ms: if self.min_ns == u64::MAX {
                0
            } else {
                to_ms(self.min_ns)
            },
            max_ms: to_ms(self.max_ns),
        }
    }
}

/// Rolling percentile snapshot (milliseconds).
#[derive(Debug, Clone, Default, Serialize)]
pub struct WindowStats {
    pub count: u64,
    pub avg_ms: u64,
    pub p50_ms: u64,
    pub p90_ms: u64,
    pub p99_ms: u64,
    pub min_ms: u64,
    pub max_ms: u64,
}

// =============================================================================
// StageAggregates — the headline + per-stage rolling windows (one Mutex).
// =============================================================================

struct StageAggregates {
    headline: RollingWindow,
    stages: Vec<RollingWindow>, // indexed by Stage::index()
    turns: u64,
    streaming_turns: u64,
}

impl StageAggregates {
    fn new() -> Self {
        Self {
            headline: RollingWindow::new(WINDOW_SAMPLES),
            stages: (0..N_STAGES)
                .map(|_| RollingWindow::new(WINDOW_SAMPLES))
                .collect(),
            turns: 0,
            streaming_turns: 0,
        }
    }
}

// =============================================================================
// ProfilingConfig (from env)
// =============================================================================

/// Runtime configuration for the profiling system (constructed once on `CoreState`).
#[derive(Debug, Clone)]
pub struct ProfilingConfig {
    /// Master switch: assemble + aggregate turns (`WAAV_TURN_PROFILING`, default on).
    pub enabled: bool,
    /// Stream/recent sampling: broadcast every Nth turn (`WAAV_DEBUG_PROFILE_SAMPLE_N`, default 1).
    pub sample_n: u64,
    /// Whether the `/debug/profile*` routes are mounted (`WAAV_DEBUG_PROFILE`).
    pub debug_routes: bool,
    /// Optional extra bearer token for the long-lived SSE (`WAAV_DEBUG_PROFILE_TOKEN`).
    pub token: Option<String>,
}

impl ProfilingConfig {
    pub fn try_from_env() -> Result<Self, String> {
        let enabled = parse_env_bool("WAAV_TURN_PROFILING", true)?;
        let sample_n = parse_env_positive_u64("WAAV_DEBUG_PROFILE_SAMPLE_N", 1)?;
        let debug_routes = parse_env_bool("WAAV_DEBUG_PROFILE", false)?;

        Ok(Self {
            enabled,
            sample_n,
            debug_routes,
            token: std::env::var("WAAV_DEBUG_PROFILE_TOKEN")
                .ok()
                .filter(|s| !s.is_empty()),
        })
    }

    pub fn from_env() -> Self {
        Self::try_from_env().unwrap_or_else(|e| panic!("invalid profiling config: {e}"))
    }
}

fn parse_env_bool(name: &str, default: bool) -> Result<bool, String> {
    match std::env::var(name) {
        Ok(value) => parse_bool(&value).ok_or_else(|| {
            format!("Invalid {name} environment variable: expected true/false/1/0/yes/no")
        }),
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(std::env::VarError::NotUnicode(_)) => {
            Err(format!("{name} environment variable must be valid UTF-8"))
        }
    }
}

fn parse_env_positive_u64(name: &str, default: u64) -> Result<u64, String> {
    match std::env::var(name) {
        Ok(value) => value
            .parse::<u64>()
            .map_err(|e| format!("Invalid {name} environment variable: {e}"))
            .and_then(|n| {
                if n > 0 {
                    Ok(n)
                } else {
                    Err(format!(
                        "{name} environment variable must be greater than zero"
                    ))
                }
            }),
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(std::env::VarError::NotUnicode(_)) => {
            Err(format!("{name} environment variable must be valid UTF-8"))
        }
    }
}

impl Default for ProfilingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            sample_n: 1,
            debug_routes: false,
            token: None,
        }
    }
}

// =============================================================================
// LatencyProfiler — the hub
// =============================================================================

pub struct LatencyProfiler {
    enabled: AtomicBool,
    sample_n: u64,
    slow_threshold_ms: u64,
    broadcast: broadcast::Sender<Arc<TurnTrace>>,
    stream_subscribers: AtomicUsize,
    seq: AtomicU64,
    agg: Mutex<StageAggregates>,
    /// Smart-turn inference window, kept separate so per-frame updates never
    /// contend with per-turn aggregation.
    smart_turn: Mutex<RollingWindow>,
    /// Per-stage extra windows surfaced in realtime_blockers (llm_ttft, tts_ttfb).
    recent_slow: Mutex<VecDeque<Arc<TurnTrace>>>,
    bottleneck_counts: [AtomicU64; N_STAGES],
    frame_total: AtomicU64,
    frame_skips: AtomicU64,
    ws_queue_max: AtomicU64,
    lk_queue_max: AtomicU64,
}

impl LatencyProfiler {
    pub fn new(enabled: bool, sample_n: u64) -> Self {
        let (tx, _rx) = broadcast::channel(BROADCAST_CAP);
        Self {
            enabled: AtomicBool::new(enabled),
            sample_n: sample_n.max(1),
            slow_threshold_ms: SLOW_THRESHOLD_MS,
            broadcast: tx,
            stream_subscribers: AtomicUsize::new(0),
            seq: AtomicU64::new(0),
            agg: Mutex::new(StageAggregates::new()),
            smart_turn: Mutex::new(RollingWindow::new(WINDOW_SAMPLES)),
            recent_slow: Mutex::new(VecDeque::with_capacity(RECENT_SLOW_CAP)),
            bottleneck_counts: std::array::from_fn(|_| AtomicU64::new(0)),
            frame_total: AtomicU64::new(0),
            frame_skips: AtomicU64::new(0),
            ws_queue_max: AtomicU64::new(0),
            lk_queue_max: AtomicU64::new(0),
        }
    }

    pub fn from_config(cfg: &ProfilingConfig) -> Self {
        Self::new(cfg.enabled, cfg.sample_n)
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
    }

    // --- Stream subscription (used by the SSE handler) ---

    /// Subscribe to the live turn stream. Pair with [`subscriber_guard`] so the
    /// zero-broadcast gate re-arms on disconnect.
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<TurnTrace>> {
        self.broadcast.subscribe()
    }

    /// RAII guard that counts an active stream subscriber (gates the broadcast).
    pub fn subscriber_guard(self: &Arc<Self>) -> SubscriberGuard {
        self.stream_subscribers.fetch_add(1, Ordering::Relaxed);
        SubscriberGuard { hub: self.clone() }
    }

    pub fn stream_subscribers(&self) -> usize {
        self.stream_subscribers.load(Ordering::Relaxed)
    }

    // --- Per-frame budget (called by FrameProfiler; lock-free hot path) ---

    pub fn record_frame(&self) {
        self.frame_total.fetch_add(1, Ordering::Relaxed);
        bridge::record_frame();
    }

    pub fn record_frame_skip(&self) {
        self.frame_skips.fetch_add(1, Ordering::Relaxed);
        bridge::record_smart_turn_frame_skip();
    }

    pub fn record_smart_turn(&self, inference_us: u64) {
        bridge::observe_smart_turn_inference_ms(inference_us as f64 / 1000.0);
        self.smart_turn
            .lock()
            .record(inference_us.saturating_mul(1000));
    }

    pub fn record_frame_stage(&self, stage: &'static str, dur_ns: u64) {
        bridge::observe_frame_stage_ms(stage, dur_ns as f64 / 1_000_000.0);
    }

    /// Publish an egress queue depth + track its running max for realtime_blockers.
    pub fn observe_queue_depth(&self, queue: &'static str, depth: u64) {
        bridge::set_queue_depth(queue, depth);
        let slot = match queue {
            "ws_msg" => &self.ws_queue_max,
            "livekit_op" => &self.lk_queue_max,
            _ => return,
        };
        slot.fetch_max(depth, Ordering::Relaxed);
    }

    pub fn observe_queue_latency_ms(&self, queue: &'static str, ms: u64) {
        bridge::set_queue_latency_ms(queue, ms);
    }

    // --- Snapshot for the /debug/profile endpoint ---

    pub fn snapshot(&self) -> ProfileSnapshot {
        let agg = self.agg.lock();
        let stages: Vec<StageStat> = Stage::ALL
            .iter()
            .map(|&s| StageStat {
                stage: s.as_str(),
                stats: agg.stages[s.index()].stats(),
            })
            .collect();
        let bottleneck_histogram: Vec<BottleneckBucket> = Stage::RESPONSE_STAGES
            .iter()
            .map(|&s| BottleneckBucket {
                stage: s.as_str(),
                count: self.bottleneck_counts[s.index()].load(Ordering::Relaxed),
            })
            .collect();
        let current_bottleneck = bottleneck_histogram
            .iter()
            .filter(|b| b.count > 0)
            .max_by_key(|b| b.count)
            .map(|b| b.stage);

        let llm_ttft = agg.stages[Stage::LlmTtft.index()].stats();
        let llm_sentence = agg.stages[Stage::LlmSentence.index()].stats();
        let tts_ttfb = agg.stages[Stage::TtsTtfb.index()].stats();
        let smart_turn = self.smart_turn.lock().stats();
        let frames = self.frame_total.load(Ordering::Relaxed);
        let skips = self.frame_skips.load(Ordering::Relaxed);
        let turns = agg.turns;
        let streaming = agg.streaming_turns;

        let recent_slow_turns = self
            .recent_slow
            .lock()
            .iter()
            .map(|t| t.summary())
            .collect();

        ProfileSnapshot {
            enabled: self.is_enabled(),
            sample_count: agg.headline.count(),
            headline: agg.headline.stats(),
            stages,
            bottleneck_histogram,
            current_bottleneck,
            recent_slow_turns,
            realtime_blockers: RealtimeBlockers {
                smart_turn_inference_ms_p99: smart_turn.p99_ms,
                frame_skip_rate: if frames > 0 {
                    skips as f64 / frames as f64
                } else {
                    0.0
                },
                frames_total: frames,
                frame_skips_total: skips,
                llm_ttft_p50_ms: llm_ttft.p50_ms,
                llm_sentence_p50_ms: llm_sentence.p50_ms,
                tts_ttfb_p50_ms: tts_ttfb.p50_ms,
                streaming_path_used_ratio: if turns > 0 {
                    streaming as f64 / turns as f64
                } else {
                    0.0
                },
                ws_queue_depth_max: self.ws_queue_max.load(Ordering::Relaxed),
                lk_queue_depth_max: self.lk_queue_max.load(Ordering::Relaxed),
            },
        }
    }
}

impl TurnSink for LatencyProfiler {
    fn record_turn(&self, trace: TurnTrace) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        let path = trace.path.as_str();
        let outcome = trace.outcome.as_str();
        let resp_ms = trace.response_latency_ns().map(|n| n / 1_000_000);
        let deltas = trace.stage_deltas();

        // (a) Always-on structured event — works today via RUST_LOG=waav::turn=info.
        tracing::info!(
            target: "waav::turn",
            turn_id = trace.turn_id,
            session = %trace.session_id,
            path,
            outcome,
            streaming = trace.streaming_path,
            response_latency_ms = resp_ms,
            smart_turn_inference_ms = (trace.smart_turn_inference_us / 1000),
            bottleneck = trace.bottleneck.map(|b| b.as_str()).unwrap_or("none"),
            "turn_complete"
        );

        // (b) Prometheus.
        bridge::record_turn_outcome(path, outcome);
        if let Some(ms) = resp_ms {
            bridge::observe_turn_response_latency_ms(path, ms as f64);
        }
        for &(stage, ns) in &deltas {
            let ms = ns as f64 / 1_000_000.0;
            bridge::observe_turn_stage_ms(stage.as_str(), path, ms);
            match stage {
                Stage::LlmTtft => bridge::observe_llm_ttft_ms(path, ms),
                Stage::TtsTtfb => bridge::observe_tts_ttfb_ms(path, ms),
                _ => {}
            }
        }
        for (node, us) in &trace.node_durations_us {
            bridge::observe_dag_node_ms(node.to_string(), *us as f64 / 1000.0);
        }
        if let Some(b) = trace.bottleneck {
            bridge::record_turn_bottleneck(b.as_str());
            self.bottleneck_counts[b.index()].fetch_add(1, Ordering::Relaxed);
        }

        // (c) Rolling aggregates.
        {
            let mut agg = self.agg.lock();
            agg.turns += 1;
            if trace.streaming_path {
                agg.streaming_turns += 1;
            }
            if let Some(ns) = trace.response_latency_ns() {
                agg.headline.record(ns);
            }
            for &(stage, ns) in &deltas {
                agg.stages[stage.index()].record(ns);
            }
        }

        // (d) recent-slow ring + gated broadcast.
        let arc = Arc::new(trace);
        if let Some(ms) = resp_ms
            && ms >= self.slow_threshold_ms
        {
            let mut rs = self.recent_slow.lock();
            if rs.len() >= RECENT_SLOW_CAP {
                rs.pop_front();
            }
            rs.push_back(arc.clone());
        }
        if self.stream_subscribers.load(Ordering::Relaxed) > 0 {
            let n = self.seq.fetch_add(1, Ordering::Relaxed);
            if self.sample_n <= 1 || n.is_multiple_of(self.sample_n) {
                let _ = self.broadcast.send(arc);
            }
        }
    }
}

/// Re-arms the broadcast gate when a stream client disconnects.
pub struct SubscriberGuard {
    hub: Arc<LatencyProfiler>,
}

impl Drop for SubscriberGuard {
    fn drop(&mut self) {
        self.hub.stream_subscribers.fetch_sub(1, Ordering::Relaxed);
    }
}

// =============================================================================
// FrameProfiler — per-frame budget observer (forwards to the hub / Prometheus).
// =============================================================================

/// Per-session observer that records the per-frame realtime budget. Registered
/// alongside the [`crate::core::observability::TurnProfiler`].
pub struct FrameProfiler {
    hub: Arc<LatencyProfiler>,
}

impl FrameProfiler {
    pub fn new(hub: Arc<LatencyProfiler>) -> Self {
        Self { hub }
    }
}

impl VoiceObserver for FrameProfiler {
    fn on_audio_in(&self, _ts_ns: u64) {
        self.hub.record_frame();
    }
    fn on_frame_skipped(&self) {
        self.hub.record_frame_skip();
    }
    fn on_smart_turn(&self, inference_us: u64, _is_complete: bool, _ts_ns: u64) {
        if inference_us > 0 {
            self.hub.record_smart_turn(inference_us);
        }
    }
    fn on_frame_stage(&self, stage: &'static str, dur_ns: u64) {
        self.hub.record_frame_stage(stage, dur_ns);
    }
}

// =============================================================================
// Snapshot payload types
// =============================================================================

#[derive(Debug, Clone, Serialize)]
pub struct ProfileSnapshot {
    pub enabled: bool,
    pub sample_count: u64,
    pub headline: WindowStats,
    pub stages: Vec<StageStat>,
    pub bottleneck_histogram: Vec<BottleneckBucket>,
    pub current_bottleneck: Option<&'static str>,
    pub recent_slow_turns: Vec<TurnSummary>,
    pub realtime_blockers: RealtimeBlockers,
}

#[derive(Debug, Clone, Serialize)]
pub struct StageStat {
    pub stage: &'static str,
    pub stats: WindowStats,
}

#[derive(Debug, Clone, Serialize)]
pub struct BottleneckBucket {
    pub stage: &'static str,
    pub count: u64,
}

/// The actionable realtime signals that drive the optimization follow-ups.
#[derive(Debug, Clone, Serialize)]
pub struct RealtimeBlockers {
    pub smart_turn_inference_ms_p99: u64,
    pub frame_skip_rate: f64,
    pub frames_total: u64,
    pub frame_skips_total: u64,
    pub llm_ttft_p50_ms: u64,
    /// First-token → first-sentence buffering cost (the token-streaming follow-up target).
    pub llm_sentence_p50_ms: u64,
    pub tts_ttfb_p50_ms: u64,
    /// Fraction of turns that used the DAG streaming executor (vs the batch fallback).
    pub streaming_path_used_ratio: f64,
    pub ws_queue_depth_max: u64,
    pub lk_queue_depth_max: u64,
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::observability::turn_profile::{TurnOutcome, TurnPath};
    use serial_test::serial;

    fn cleanup_profile_env() {
        unsafe {
            std::env::remove_var("WAAV_TURN_PROFILING");
            std::env::remove_var("WAAV_DEBUG_PROFILE_SAMPLE_N");
            std::env::remove_var("WAAV_DEBUG_PROFILE");
            std::env::remove_var("WAAV_DEBUG_PROFILE_TOKEN");
        }
    }

    fn trace_with_latency(turn_id: u64, stt_final_ns: u64, audio_out_ns: u64) -> TurnTrace {
        let mut t = TurnTrace::open(turn_id, "s".into(), TurnPath::Conversation, stt_final_ns);
        // a plausible chain so stage deltas + bottleneck populate
        t.llm_request_ns = stt_final_ns + 5_000_000;
        t.llm_first_token_ns = stt_final_ns + 200_000_000; // llm_ttft = 195ms (dominant @612)
        t.llm_first_sentence_ns = stt_final_ns + 260_000_000;
        t.tts_request_ns = stt_final_ns + 262_000_000;
        t.tts_first_audio_ns = stt_final_ns + 430_000_000; // tts_ttfb=168ms, egress=182ms @612
        t.audio_out_ns = audio_out_ns;
        t.smart_turn_inference_us = 18_000;
        t.bottleneck = t.compute_bottleneck();
        t
    }

    #[test]
    #[serial]
    fn profiling_config_rejects_invalid_bool_env() {
        cleanup_profile_env();
        unsafe {
            std::env::set_var("WAAV_DEBUG_PROFILE", "sure");
        }

        let err = ProfilingConfig::try_from_env().expect_err("invalid bool must fail");
        assert!(
            err.contains("WAAV_DEBUG_PROFILE"),
            "error should name bad env var: {err}"
        );

        cleanup_profile_env();
    }

    #[test]
    #[serial]
    fn profiling_config_rejects_invalid_sample_n_env() {
        cleanup_profile_env();
        unsafe {
            std::env::set_var("WAAV_DEBUG_PROFILE_SAMPLE_N", "0");
        }

        let err = ProfilingConfig::try_from_env().expect_err("zero sample_n must fail");
        assert!(
            err.contains("WAAV_DEBUG_PROFILE_SAMPLE_N"),
            "error should name bad env var: {err}"
        );

        unsafe {
            std::env::set_var("WAAV_DEBUG_PROFILE_SAMPLE_N", "many");
        }
        let err = ProfilingConfig::try_from_env().expect_err("non-numeric sample_n must fail");
        assert!(
            err.contains("WAAV_DEBUG_PROFILE_SAMPLE_N"),
            "error should name bad env var: {err}"
        );

        cleanup_profile_env();
    }

    #[test]
    fn rolling_window_percentiles() {
        let mut w = RollingWindow::new(1000);
        for i in 1..=100u64 {
            w.record(i * 1_000_000); // 1ms .. 100ms
        }
        let s = w.stats();
        assert_eq!(s.count, 100);
        assert_eq!(s.min_ms, 1);
        assert_eq!(s.max_ms, 100);
        assert!(
            s.p50_ms >= 49 && s.p50_ms <= 51,
            "p50 ~50ms, got {}",
            s.p50_ms
        );
        assert!(
            s.p90_ms >= 89 && s.p90_ms <= 91,
            "p90 ~90ms, got {}",
            s.p90_ms
        );
        assert!(s.p99_ms >= 98, "p99 near 100ms, got {}", s.p99_ms);
    }

    #[test]
    fn rolling_window_evicts_oldest() {
        let mut w = RollingWindow::new(3);
        for v in [10, 20, 30, 40] {
            w.record(v * 1_000_000);
        }
        let s = w.stats();
        assert_eq!(s.count, 4, "total count keeps climbing");
        assert_eq!(s.min_ms, 10, "min is sticky across eviction");
        assert_eq!(s.max_ms, 40);
    }

    #[test]
    fn record_turn_feeds_aggregates_and_bottleneck() {
        let hub = LatencyProfiler::new(true, 1);
        // 612ms response, llm_ttft is the dominant stage (~195ms).
        hub.record_turn(trace_with_latency(0, 1_000_000_000, 1_612_000_000));
        // Smart-turn inference cost arrives via the per-frame path (FrameProfiler
        // → record_smart_turn), not from the per-turn trace metadata.
        hub.record_smart_turn(18_000);
        let snap = hub.snapshot();
        assert_eq!(snap.sample_count, 1);
        assert_eq!(snap.headline.p50_ms, 612);
        assert_eq!(snap.current_bottleneck, Some("llm_ttft"));
        let llm_bucket = snap
            .bottleneck_histogram
            .iter()
            .find(|b| b.stage == "llm_ttft")
            .unwrap();
        assert_eq!(llm_bucket.count, 1);
        assert!(snap.realtime_blockers.smart_turn_inference_ms_p99 >= 17);
    }

    #[test]
    fn disabled_record_turn_is_noop() {
        let hub = LatencyProfiler::new(false, 1);
        hub.record_turn(trace_with_latency(0, 1_000_000_000, 1_612_000_000));
        assert_eq!(hub.snapshot().sample_count, 0);
    }

    #[test]
    fn recent_slow_keeps_only_slow_turns() {
        let hub = LatencyProfiler::new(true, 1);
        // stt_final must be NONZERO (0 = unset anchor → no headline at all).
        hub.record_turn(trace_with_latency(0, 1_000_000_000, 1_500_000_000)); // 500ms — fast
        hub.record_turn(trace_with_latency(1, 1_000_000_000, 2_500_000_000)); // 1500ms — slow
        let snap = hub.snapshot();
        assert_eq!(snap.recent_slow_turns.len(), 1);
        assert_eq!(snap.recent_slow_turns[0].turn_id, 1);
    }

    #[test]
    fn broadcast_gated_when_no_subscribers() {
        let hub = Arc::new(LatencyProfiler::new(true, 1));
        // No subscriber → seq does not advance, nothing sent.
        hub.record_turn(trace_with_latency(0, 1_000_000_000, 1_612_000_000));
        assert_eq!(hub.seq.load(Ordering::Relaxed), 0);

        // With a subscriber the turn is delivered.
        let mut rx = hub.subscribe();
        let _guard = hub.subscriber_guard();
        hub.record_turn(trace_with_latency(1, 1_000_000_000, 1_612_000_000));
        let got = rx.try_recv().expect("subscribed turn delivered");
        assert_eq!(got.turn_id, 1);
        assert_eq!(got.outcome, TurnOutcome::Completed);
    }

    #[test]
    fn subscriber_guard_rearms_gate_on_drop() {
        let hub = Arc::new(LatencyProfiler::new(true, 1));
        {
            let _g = hub.subscriber_guard();
            assert_eq!(hub.stream_subscribers(), 1);
        }
        assert_eq!(hub.stream_subscribers(), 0);
    }

    #[test]
    fn frame_profiler_counts_frames_and_skips() {
        let hub = Arc::new(LatencyProfiler::new(true, 1));
        let fp = FrameProfiler::new(hub.clone());
        fp.on_audio_in(1);
        fp.on_audio_in(2);
        fp.on_frame_skipped();
        fp.on_smart_turn(12_000, true, 3);
        fp.on_frame_stage("decode", 250_000);
        let snap = hub.snapshot();
        assert_eq!(snap.realtime_blockers.frames_total, 2);
        assert_eq!(snap.realtime_blockers.frame_skips_total, 1);
        assert!(snap.realtime_blockers.frame_skip_rate > 0.0);
        assert!(snap.realtime_blockers.smart_turn_inference_ms_p99 >= 11);
    }
}
