//! Per-provider circuit breaker for streaming connections.
//!
//! A circuit breaker prevents a failing upstream from being hammered by an endless
//! reconnect loop. It is a three-state machine:
//!
//! ```text
//!            error-rate >= threshold (with min volume)
//!   ┌────────┐ ───────────────────────────────────────► ┌──────┐
//!   │ Closed │                                           │ Open │
//!   └────────┘ ◄───────────────────────────────────────  └──────┘
//!        ▲          probe succeeds (Closed)                  │
//!        │                                                   │ cooldown elapsed
//!        │                                                   ▼
//!        │   probe fails (back to Open)              ┌───────────┐
//!        └───────────────────────────────────────── │ HalfOpen  │
//!                                                    └───────────┘
//! ```
//!
//! - **Closed**: traffic flows normally. Successes/failures are tallied over a
//!   sliding window. When the failure *rate* crosses [`CircuitBreakerConfig::error_rate_threshold`]
//!   AND the window has at least [`CircuitBreakerConfig::min_request_volume`] samples,
//!   the breaker trips to **Open**.
//! - **Open**: all calls are rejected immediately ([`CircuitBreaker::allow_request`]
//!   returns `false`) until [`CircuitBreakerConfig::cooldown`] elapses, at which point
//!   the next [`CircuitBreaker::allow_request`] returns `true` and moves the breaker to
//!   **HalfOpen** (a single trial is permitted).
//! - **HalfOpen**: a single probe is allowed. If it succeeds the breaker closes and the
//!   window resets; if it fails the breaker re-opens and the cooldown restarts.
//!
//! The breaker is `Send + Sync` and lock-free on the hot path (atomics only), so it can
//! be shared across the reconnect supervisor, the metrics exporter (W-C1), and the
//! readiness probe.

use std::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::time::{Duration, Instant};

/// Observable state of a [`CircuitBreaker`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Normal operation — requests flow and outcomes are tallied.
    Closed,
    /// Tripped — requests are rejected until the cooldown elapses.
    Open,
    /// Cooldown elapsed — a single probe request is permitted.
    HalfOpen,
}

impl CircuitState {
    /// A stable string label for metrics/serialization.
    pub fn as_str(&self) -> &'static str {
        match self {
            CircuitState::Closed => "closed",
            CircuitState::Open => "open",
            CircuitState::HalfOpen => "half_open",
        }
    }

    /// A stable numeric code for `waav_circuit_breaker_state` gauges (W-C1).
    /// 0 = closed (healthy), 1 = half-open (probing), 2 = open (tripped).
    pub fn as_code(&self) -> u8 {
        match self {
            CircuitState::Closed => 0,
            CircuitState::HalfOpen => 1,
            CircuitState::Open => 2,
        }
    }
}

// Internal numeric encoding of the state for the atomic.
const STATE_CLOSED: u8 = 0;
const STATE_OPEN: u8 = 1;
const STATE_HALF_OPEN: u8 = 2;

/// Configuration for a [`CircuitBreaker`].
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// Failure *rate* in `[0.0, 1.0]` at or above which the breaker trips. Default 0.5.
    pub error_rate_threshold: f64,
    /// Minimum number of samples in the window before the rate is trusted. Below this,
    /// the breaker never trips (avoids tripping on a single early failure). Default 5.
    pub min_request_volume: u32,
    /// Sliding-window size: only the most recent `window_size` outcomes are counted.
    /// Default 20.
    pub window_size: u32,
    /// How long the breaker stays Open before allowing a half-open probe. Default 5s.
    pub cooldown: Duration,
    /// D-G2: a connection that survived LESS than this is a "quick failure"
    /// (bad credentials pass the TLS handshake, then the server closes —
    /// rate-based tripping never converges on that shape). Default 5s.
    pub min_stable_duration: Duration,
    /// D-G2: consecutive quick failures at which the breaker goes
    /// PERMANENTLY failed (sticky open — no half-open probe). Default 3.
    pub max_quick_failures: u32,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            error_rate_threshold: 0.5,
            min_request_volume: 5,
            window_size: 20,
            cooldown: Duration::from_secs(5),
            min_stable_duration: Duration::from_secs(5),
            max_quick_failures: 3,
        }
    }
}

impl CircuitBreakerConfig {
    /// A breaker tuned for fast tripping/recovery (low-latency streaming).
    pub fn aggressive() -> Self {
        Self {
            error_rate_threshold: 0.4,
            min_request_volume: 3,
            window_size: 10,
            cooldown: Duration::from_secs(2),
            min_stable_duration: Duration::from_secs(5),
            max_quick_failures: 3,
        }
    }

    /// A breaker tuned to tolerate transient blips before tripping.
    pub fn conservative() -> Self {
        Self {
            error_rate_threshold: 0.7,
            min_request_volume: 10,
            window_size: 50,
            cooldown: Duration::from_secs(15),
            min_stable_duration: Duration::from_secs(5),
            max_quick_failures: 3,
        }
    }
}

/// A point-in-time snapshot of a breaker's counters (for metrics/health endpoints).
#[derive(Debug, Clone)]
pub struct CircuitBreakerSnapshot {
    pub state: CircuitState,
    pub successes: u32,
    pub failures: u32,
    pub total_trips: u64,
    pub error_rate: f64,
}

/// A lock-free, thread-safe circuit breaker.
///
/// Counters use a fixed sliding window of the most recent `window_size` outcomes. The
/// window is approximated by halving both counters whenever their sum reaches
/// `window_size` — this keeps the rate responsive to recent behaviour without storing a
/// per-sample ring buffer, and keeps every operation O(1) and allocation-free.
///
/// If the breaker carries a `label` (set by [`CircuitBreaker::with_label`], which the
/// [`crate::core::resilience::ResilienceRegistry`] uses to stamp the provider name on each
/// breaker it creates), then **every state transition self-publishes** the
/// `waav_circuit_breaker_state{provider=<label>}` gauge (W-C1). This is what makes the gauge
/// truthful for *all* providers — including the inline Deepgram/AssemblyAI reconnect loops that
/// only ever called the `record_reconnect` counter — without each call site having to remember
/// to emit the gauge: it is now a property of tripping the breaker itself.
pub struct CircuitBreaker {
    config: CircuitBreakerConfig,
    /// Encoded [`CircuitState`] (`STATE_*`).
    state: AtomicU8,
    successes: AtomicU32,
    failures: AtomicU32,
    /// Monotonic nanos at which the breaker last opened (start of cooldown).
    opened_at_ns: AtomicU64,
    /// Total number of times the breaker has tripped to Open (cumulative).
    total_trips: AtomicU64,
    /// D-G2: consecutive connections that died before `min_stable_duration`.
    quick_failures: AtomicU32,
    /// D-G2: sticky credentials-fatal flag — once set, `allow_request` is
    /// false FOREVER (no half-open probe; backoff cannot fix bad creds).
    permanently_failed: std::sync::atomic::AtomicBool,
    /// Optional metrics label (the provider name). When set, every state transition publishes
    /// `waav_circuit_breaker_state{provider=<label>}` so the gauge tracks the breaker in
    /// near-real-time. `None` for anonymous breakers (e.g. unit-test fixtures) which stay silent.
    label: Option<String>,
}

impl CircuitBreaker {
    /// Create a breaker in the Closed state (no metrics label — stays silent on the gauge).
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            config,
            state: AtomicU8::new(STATE_CLOSED),
            successes: AtomicU32::new(0),
            failures: AtomicU32::new(0),
            opened_at_ns: AtomicU64::new(0),
            total_trips: AtomicU64::new(0),
            quick_failures: AtomicU32::new(0),
            permanently_failed: std::sync::atomic::AtomicBool::new(false),
            label: None,
        }
    }

    /// Create a labelled breaker. The `label` (the provider name) is stamped on every
    /// `waav_circuit_breaker_state` sample this breaker publishes on a transition, so the gauge
    /// is attributed to the right provider. The [`crate::core::resilience::ResilienceRegistry`]
    /// uses this for every per-provider breaker it owns.
    pub fn with_label(config: CircuitBreakerConfig, label: impl Into<String>) -> Self {
        let mut cb = Self::new(config);
        cb.label = Some(label.into());
        // Seed the gauge at the initial (closed) state so the series exists from creation and a
        // scrape before the first failure reports "closed" rather than absent.
        cb.publish_state();
        cb
    }

    /// Create a breaker with the default config.
    pub fn with_defaults() -> Self {
        Self::new(CircuitBreakerConfig::default())
    }

    /// The current observable state.
    ///
    /// This is a pure read — it does NOT transition Open→HalfOpen even if the cooldown has
    /// elapsed. The Open→HalfOpen transition only happens through [`allow_request`], which
    /// is the gate the supervisor actually consults. (Otherwise a metrics scrape could
    /// "use up" the half-open probe.)
    pub fn state(&self) -> CircuitState {
        match self.state.load(Ordering::Acquire) {
            STATE_OPEN => CircuitState::Open,
            STATE_HALF_OPEN => CircuitState::HalfOpen,
            _ => CircuitState::Closed,
        }
    }

    /// Whether a request (e.g. a reconnect attempt) may proceed *now*.
    ///
    /// - Closed → always `true`.
    /// - Open → `false` until the cooldown elapses, then transitions to HalfOpen and
    ///   returns `true` exactly once (the probe). Concurrent callers race for the single
    ///   probe via a CAS; losers see HalfOpen and are denied.
    /// - HalfOpen → `false` (the single probe is already outstanding).
    pub fn allow_request(&self) -> bool {
        // D-G2: a credentials-fatal breaker never half-opens — retrying a
        // bad key forever is the failure shape this exists to stop.
        if self.permanently_failed.load(Ordering::Acquire) {
            return false;
        }
        match self.state.load(Ordering::Acquire) {
            STATE_CLOSED => true,
            STATE_HALF_OPEN => false,
            _ /* STATE_OPEN */ => {
                let opened = self.opened_at_ns.load(Ordering::Acquire);
                let elapsed = now_ns().saturating_sub(opened);
                if elapsed >= self.config.cooldown.as_nanos() as u64 {
                    // Cooldown elapsed: try to claim the single probe slot.
                    let claimed = self
                        .state
                        .compare_exchange(
                            STATE_OPEN,
                            STATE_HALF_OPEN,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        )
                        .is_ok();
                    if claimed {
                        // We transitioned Open → HalfOpen (probing): reflect it on the gauge.
                        self.publish_state();
                    }
                    claimed
                } else {
                    false
                }
            }
        }
    }

    /// Record a successful outcome.
    ///
    /// - HalfOpen → close the breaker and reset the window (recovery confirmed).
    /// - Closed → tally toward the window.
    pub fn record_success(&self) {
        if self.state.load(Ordering::Acquire) == STATE_HALF_OPEN {
            self.close_and_reset();
            return;
        }
        self.successes.fetch_add(1, Ordering::AcqRel);
        self.maybe_decay_window();
    }

    /// Record a failed outcome.
    ///
    /// - HalfOpen → re-open and restart the cooldown (probe failed).
    /// - Closed → tally toward the window and trip if the rate crosses the threshold.
    pub fn record_failure(&self) {
        if self.state.load(Ordering::Acquire) == STATE_HALF_OPEN {
            self.trip();
            return;
        }
        self.failures.fetch_add(1, Ordering::AcqRel);
        self.maybe_decay_window();
        self.maybe_trip();
    }

    /// D-G2: record a connection's lifetime at close. Sub-stable lifetimes
    /// (a handshake that succeeds then dies — bad credentials' signature)
    /// count consecutively; at `max_quick_failures` the breaker goes
    /// PERMANENTLY failed. A stable connection resets the count.
    pub fn record_connection_closed(&self, stable_for: Duration) {
        if stable_for >= self.config.min_stable_duration {
            self.quick_failures.store(0, Ordering::Release);
            return;
        }
        let n = self.quick_failures.fetch_add(1, Ordering::AcqRel) + 1;
        if n >= self.config.max_quick_failures {
            tracing::error!(
                provider = self.label.as_deref().unwrap_or("unknown"),
                quick_failures = n,
                "circuit breaker PERMANENTLY failed: {} consecutive sub-{}s                  connections (credentials/config fatal — backoff cannot fix this)",
                n,
                self.config.min_stable_duration.as_secs(),
            );
            self.permanently_failed.store(true, Ordering::Release);
            self.trip();
        }
    }

    /// D-G2: whether the breaker is credentials-fatal (sticky).
    pub fn is_permanently_failed(&self) -> bool {
        self.permanently_failed.load(Ordering::Acquire)
    }

    /// Current failure rate over the window, `0.0` if there are no samples.
    pub fn error_rate(&self) -> f64 {
        let s = self.successes.load(Ordering::Acquire) as f64;
        let f = self.failures.load(Ordering::Acquire) as f64;
        let total = s + f;
        if total == 0.0 { 0.0 } else { f / total }
    }

    /// Total number of times the breaker has tripped to Open.
    pub fn total_trips(&self) -> u64 {
        self.total_trips.load(Ordering::Relaxed)
    }

    /// A snapshot of the current counters.
    pub fn snapshot(&self) -> CircuitBreakerSnapshot {
        CircuitBreakerSnapshot {
            state: self.state(),
            successes: self.successes.load(Ordering::Relaxed),
            failures: self.failures.load(Ordering::Relaxed),
            total_trips: self.total_trips.load(Ordering::Relaxed),
            error_rate: self.error_rate(),
        }
    }

    /// Force the breaker back to Closed and clear the window (administrative reset).
    pub fn reset(&self) {
        self.close_and_reset();
    }

    // --- internals -----------------------------------------------------------------

    /// Publish this breaker's current state on the `waav_circuit_breaker_state{provider}` gauge,
    /// if it carries a label. A no-op for anonymous breakers. Cheap and lock-free (one atomic
    /// load + a gauge set); safe to call on every transition.
    fn publish_state(&self) {
        if let Some(label) = self.label.as_deref() {
            crate::core::metrics::bridge::set_circuit_breaker_state(label, self.state().as_code());
        }
    }

    fn close_and_reset(&self) {
        self.successes.store(0, Ordering::Release);
        self.failures.store(0, Ordering::Release);
        let prev = self.state.swap(STATE_CLOSED, Ordering::AcqRel);
        // Only re-publish on an actual transition into Closed (avoid spamming the gauge on every
        // healthy success, which calls record_success → maybe_decay but not close_and_reset).
        if prev != STATE_CLOSED {
            self.publish_state();
        }
    }

    fn trip(&self) {
        let prev = self.state.swap(STATE_OPEN, Ordering::AcqRel);
        self.opened_at_ns.store(now_ns(), Ordering::Release);
        // Only count a trip / re-publish the gauge when we actually transition into Open from a
        // non-Open state.
        if prev != STATE_OPEN {
            self.total_trips.fetch_add(1, Ordering::Relaxed);
            self.publish_state();
        }
    }

    fn maybe_trip(&self) {
        let s = self.successes.load(Ordering::Acquire);
        let f = self.failures.load(Ordering::Acquire);
        let total = s + f;
        if total < self.config.min_request_volume {
            return;
        }
        let rate = f as f64 / total as f64;
        if rate >= self.config.error_rate_threshold
            && self.state.load(Ordering::Acquire) == STATE_CLOSED
        {
            self.trip();
        }
    }

    /// Keep the counters bounded to roughly `window_size` recent samples by halving both
    /// when their sum reaches the window size (exponential decay of old samples).
    fn maybe_decay_window(&self) {
        let s = self.successes.load(Ordering::Acquire);
        let f = self.failures.load(Ordering::Acquire);
        if s + f >= self.config.window_size {
            // Halve both, preserving the ratio. Round so a lone failure isn't erased.
            self.successes.store(s / 2, Ordering::Release);
            self.failures.store(f.div_ceil(2), Ordering::Release);
        }
    }
}

/// Monotonic nanoseconds since process start (shared clock; cheap).
fn now_ns() -> u64 {
    use std::sync::OnceLock;
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_nanos() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fast_cooldown_config() -> CircuitBreakerConfig {
        CircuitBreakerConfig {
            error_rate_threshold: 0.5,
            min_request_volume: 4,
            window_size: 20,
            cooldown: Duration::from_millis(30),
            ..Default::default()
        }
    }

    #[test]
    fn starts_closed_and_allows_requests() {
        let cb = CircuitBreaker::with_defaults();
        assert_eq!(cb.state(), CircuitState::Closed);
        assert!(cb.allow_request());
        assert_eq!(cb.total_trips(), 0);
    }

    #[test]
    fn does_not_trip_below_min_volume() {
        // 100% failure rate but only 3 samples (< min_request_volume = 5 default).
        let cb = CircuitBreaker::with_defaults();
        for _ in 0..3 {
            cb.record_failure();
        }
        assert_eq!(cb.state(), CircuitState::Closed, "must not trip below min volume");
        assert!(cb.allow_request());
    }

    #[test]
    fn breaker_trips_fatal_after_3_substable_connections() {
        // D-G2: bad credentials pass the handshake then die fast — the
        // rate-based breaker never converges; the quick-fail detector must
        // go PERMANENTLY open (no half-open probe, ever).
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            cooldown: Duration::from_millis(1),
            ..Default::default()
        });
        for _ in 0..2 {
            cb.record_connection_closed(Duration::from_millis(300));
            assert!(!cb.is_permanently_failed());
        }
        cb.record_connection_closed(Duration::from_millis(300));
        assert!(cb.is_permanently_failed(), "3rd quick failure = fatal");
        // Past ANY cooldown, allow_request stays false (sticky — distinct
        // from a rate trip's half-open).
        std::thread::sleep(Duration::from_millis(5));
        assert!(!cb.allow_request(), "credentials-fatal never half-opens");
        assert!(!cb.allow_request());
    }

    #[test]
    fn stable_connection_resets_quick_failure_count() {
        let cb = CircuitBreaker::with_defaults();
        cb.record_connection_closed(Duration::from_millis(100));
        cb.record_connection_closed(Duration::from_millis(100));
        // A stable connection (≥ min_stable_duration) resets the streak.
        cb.record_connection_closed(Duration::from_secs(60));
        cb.record_connection_closed(Duration::from_millis(100));
        cb.record_connection_closed(Duration::from_millis(100));
        assert!(
            !cb.is_permanently_failed(),
            "streak must reset on a stable connection (flaky ≠ fatal)"
        );
    }

    #[test]
    fn rate_trip_still_half_opens() {
        // Regression: the ordinary rate trip keeps its half-open recovery.
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            error_rate_threshold: 0.5,
            min_request_volume: 2,
            window_size: 10,
            cooldown: Duration::from_millis(1),
            ..Default::default()
        });
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
        std::thread::sleep(Duration::from_millis(5));
        assert!(cb.allow_request(), "rate trip half-opens after cooldown");
        cb.record_success();
        assert_eq!(cb.state(), CircuitState::Closed, "probe success closes");
    }

    #[test]
    fn trips_open_when_error_rate_crosses_threshold() {
        let cb = CircuitBreaker::new(fast_cooldown_config());
        // 4 failures, 0 successes -> rate 1.0 >= 0.5, volume 4 >= min 4.
        for _ in 0..4 {
            cb.record_failure();
        }
        assert_eq!(cb.state(), CircuitState::Open);
        assert!(!cb.allow_request(), "open breaker rejects immediately");
        assert_eq!(cb.total_trips(), 1);
    }

    #[test]
    fn does_not_trip_when_rate_below_threshold() {
        let cb = CircuitBreaker::new(fast_cooldown_config());
        // 6 successes + 2 failures = rate 0.25 < 0.5, volume 8 >= 4.
        for _ in 0..6 {
            cb.record_success();
        }
        for _ in 0..2 {
            cb.record_failure();
        }
        assert_eq!(cb.state(), CircuitState::Closed);
        assert!(cb.error_rate() < 0.5);
    }

    #[test]
    fn open_transitions_to_half_open_after_cooldown() {
        let cb = CircuitBreaker::new(fast_cooldown_config());
        for _ in 0..4 {
            cb.record_failure();
        }
        assert_eq!(cb.state(), CircuitState::Open);
        assert!(!cb.allow_request());

        std::thread::sleep(Duration::from_millis(45));

        // First allow_request after cooldown claims the half-open probe.
        assert!(cb.allow_request(), "probe should be allowed after cooldown");
        assert_eq!(cb.state(), CircuitState::HalfOpen);
        // The single probe is now outstanding; further requests are denied.
        assert!(!cb.allow_request(), "only one probe permitted in half-open");
    }

    #[test]
    fn half_open_success_closes_breaker() {
        let cb = CircuitBreaker::new(fast_cooldown_config());
        for _ in 0..4 {
            cb.record_failure();
        }
        std::thread::sleep(Duration::from_millis(45));
        assert!(cb.allow_request());
        assert_eq!(cb.state(), CircuitState::HalfOpen);

        cb.record_success();
        assert_eq!(cb.state(), CircuitState::Closed, "successful probe closes breaker");
        assert!(cb.allow_request());
        // Window was reset on close.
        assert_eq!(cb.error_rate(), 0.0);
    }

    #[test]
    fn half_open_failure_reopens_breaker() {
        let cb = CircuitBreaker::new(fast_cooldown_config());
        for _ in 0..4 {
            cb.record_failure();
        }
        let trips_after_first = cb.total_trips();
        std::thread::sleep(Duration::from_millis(45));
        assert!(cb.allow_request());
        assert_eq!(cb.state(), CircuitState::HalfOpen);

        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open, "failed probe re-opens breaker");
        assert_eq!(cb.total_trips(), trips_after_first + 1, "re-open counts as a trip");
        // And immediately denies again.
        assert!(!cb.allow_request());
    }

    #[test]
    fn full_recovery_cycle() {
        let cb = CircuitBreaker::new(fast_cooldown_config());
        // Trip.
        for _ in 0..4 {
            cb.record_failure();
        }
        assert_eq!(cb.state(), CircuitState::Open);
        // Cooldown -> half-open probe -> success -> closed.
        std::thread::sleep(Duration::from_millis(45));
        assert!(cb.allow_request());
        cb.record_success();
        assert_eq!(cb.state(), CircuitState::Closed);
        // Healthy traffic keeps it closed.
        for _ in 0..10 {
            cb.record_success();
        }
        assert_eq!(cb.state(), CircuitState::Closed);
        assert!(cb.allow_request());
    }

    #[test]
    fn window_decays_so_old_failures_age_out() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            error_rate_threshold: 0.5,
            min_request_volume: 4,
            window_size: 10,
            cooldown: Duration::from_millis(50),
            ..Default::default()
        });
        // Seed some old failures but stay under the trip threshold by interleaving success.
        cb.record_failure();
        cb.record_failure();
        // Now a long run of successes should drive the rate down via window decay.
        for _ in 0..40 {
            cb.record_success();
        }
        assert!(cb.error_rate() < 0.5, "rate should decay: {}", cb.error_rate());
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn snapshot_reports_counters() {
        let cb = CircuitBreaker::new(fast_cooldown_config());
        cb.record_success();
        cb.record_failure();
        let snap = cb.snapshot();
        assert_eq!(snap.state, CircuitState::Closed);
        assert_eq!(snap.successes, 1);
        assert_eq!(snap.failures, 1);
        assert!((snap.error_rate - 0.5).abs() < 1e-9);
    }

    #[test]
    fn state_code_and_str_are_stable() {
        assert_eq!(CircuitState::Closed.as_code(), 0);
        assert_eq!(CircuitState::HalfOpen.as_code(), 1);
        assert_eq!(CircuitState::Open.as_code(), 2);
        assert_eq!(CircuitState::Closed.as_str(), "closed");
        assert_eq!(CircuitState::Open.as_str(), "open");
        assert_eq!(CircuitState::HalfOpen.as_str(), "half_open");
    }

    #[test]
    fn reset_forces_closed() {
        let cb = CircuitBreaker::new(fast_cooldown_config());
        for _ in 0..4 {
            cb.record_failure();
        }
        assert_eq!(cb.state(), CircuitState::Open);
        cb.reset();
        assert_eq!(cb.state(), CircuitState::Closed);
        assert!(cb.allow_request());
    }

    #[test]
    fn labelled_breaker_publishes_open_state_on_the_gauge() {
        // W-C1 RED: a labelled breaker that trips Open must publish state-code 2 (open) on the
        // `waav_circuit_breaker_state{provider}` gauge — NOT leave it stuck at 0 (closed). This is
        // the bug the inline Deepgram/AssemblyAI loops had: they bumped the reconnect counter but
        // never moved the gauge, so an open breaker still read "closed" to dashboards/readiness.
        use crate::core::metrics::bridge;
        // Touch the recorder so the global exposition exists.
        let _ = bridge::metrics_handle();

        let provider = "gauge-test-open";
        let cb = CircuitBreaker::with_label(fast_cooldown_config(), provider);
        // Freshly labelled: seeded at closed (code 0).
        assert!(
            gauge_value(&bridge::render(), provider)
                .map(|v| v == 0.0)
                .unwrap_or(true),
            "a fresh breaker should report closed (0) or be unseeded"
        );

        // Trip it.
        for _ in 0..4 {
            cb.record_failure();
        }
        assert_eq!(cb.state(), CircuitState::Open);

        let v = gauge_value(&bridge::render(), provider)
            .expect("open breaker must have published a gauge sample");
        assert_eq!(
            v, 2.0,
            "open breaker must report state-code 2 (open) on waav_circuit_breaker_state, got {v}"
        );

        // Recovery must walk the gauge back down: cooldown → half-open (1) → success → closed (0).
        std::thread::sleep(Duration::from_millis(45));
        assert!(cb.allow_request(), "probe allowed after cooldown");
        let v = gauge_value(&bridge::render(), provider).expect("half-open sample");
        assert_eq!(v, 1.0, "half-open breaker must report state-code 1, got {v}");

        cb.record_success();
        assert_eq!(cb.state(), CircuitState::Closed);
        let v = gauge_value(&bridge::render(), provider).expect("closed sample");
        assert_eq!(v, 0.0, "recovered breaker must report state-code 0 (closed), got {v}");
    }

    /// Parse the `waav_circuit_breaker_state{provider="<provider>"}` sample value out of a
    /// Prometheus text exposition, if present.
    fn gauge_value(exposition: &str, provider: &str) -> Option<f64> {
        let needle = format!("provider=\"{provider}\"");
        exposition
            .lines()
            .filter(|l| l.starts_with("waav_circuit_breaker_state") && l.contains(&needle))
            .filter_map(|l| l.rsplit(' ').next())
            .filter_map(|v| v.parse::<f64>().ok())
            .next_back()
    }

    #[test]
    fn anonymous_breaker_does_not_publish_a_gauge() {
        // A breaker without a label (unit-test fixtures, ReconnectableStream::new default) must
        // stay silent on the gauge — no phantom provider="" series.
        use crate::core::metrics::bridge;
        let _ = bridge::metrics_handle();
        let cb = CircuitBreaker::new(fast_cooldown_config());
        for _ in 0..4 {
            cb.record_failure();
        }
        assert_eq!(cb.state(), CircuitState::Open);
        // No empty-provider open sample should appear from this breaker.
        assert!(
            gauge_value(&bridge::render(), "").is_none()
                || gauge_value(&bridge::render(), "").unwrap_or(0.0) != 2.0,
            "anonymous breaker must not publish an open gauge under an empty provider label"
        );
    }

    #[test]
    fn concurrent_half_open_probe_is_single() {
        use std::sync::Arc;
        let cb = Arc::new(CircuitBreaker::new(fast_cooldown_config()));
        for _ in 0..4 {
            cb.record_failure();
        }
        std::thread::sleep(Duration::from_millis(45));

        // Many threads race for the single probe; exactly one must win.
        let granted = Arc::new(AtomicU32::new(0));
        let mut handles = vec![];
        for _ in 0..16 {
            let cb = Arc::clone(&cb);
            let granted = Arc::clone(&granted);
            handles.push(std::thread::spawn(move || {
                if cb.allow_request() {
                    granted.fetch_add(1, Ordering::AcqRel);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(
            granted.load(Ordering::Acquire),
            1,
            "exactly one probe should be granted under contention"
        );
    }
}
