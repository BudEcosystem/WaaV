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
//! The Closed/Open/HalfOpen state machine is the shared one in
//! [`waav_segmented_stt::breaker`], the same that guards segmented sessions' uploads: a half-open
//! probe that reports nothing (cancelled, or an answer that says nothing about the vendor) is
//! replaced after a lease instead of denying every caller until restart. This type adds the
//! credentials-FATAL state, the per-provider gauge and the presets on top.
//!
//! The breaker is `Send + Sync` and shared across the reconnect supervisor, the metrics exporter
//! (W-C1), and the readiness probe.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use waav_segmented_stt::breaker::{Breaker, BreakerConfig, BreakerState};

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
/// The shortest a half-open probe may stay outstanding before the next caller probes instead.
const MIN_PROBE_LEASE: Duration = Duration::from_secs(15);

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
    /// D-G2: consecutive quick failures at which the breaker enters the
    /// FATAL state (credentials/config signature). Default 3.
    pub max_quick_failures: u32,
    /// D-G2: how long the FATAL state waits before allowing ONE recovery
    /// probe (review wc71hewlx #1 — the fatal state must be recoverable, not
    /// restart-only; a transient cause or a false-positive heals, while a
    /// truly-bad key only retries once per this window). Default 60s.
    pub fatal_cooldown: Duration,
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
            fatal_cooldown: Duration::from_secs(60),
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
            fatal_cooldown: Duration::from_secs(60),
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
            fatal_cooldown: Duration::from_secs(60),
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

/// A thread-safe circuit breaker.
///
/// The window holds the most recent `window_size` outcomes exactly.
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
    /// The shared Closed/Open/HalfOpen state machine.
    core: Breaker,
    /// D-G2: consecutive connections that died before `min_stable_duration`.
    quick_failures: AtomicU32,
    /// D-G2: monotonic nanos at which the FATAL state was last (re)entered —
    /// the start of the `fatal_cooldown` before a recovery probe is allowed.
    fatal_opened_at_ns: AtomicU64,
    /// D-G2: the credentials-fatal flag — while set, `allow_request` admits one recovery probe
    /// per `fatal_cooldown` (backoff cannot fix bad creds).
    permanently_failed: std::sync::atomic::AtomicBool,
    /// Optional metrics label (the provider name). When set, every state transition publishes
    /// `waav_circuit_breaker_state{provider=<label>}` so the gauge tracks the breaker in
    /// near-real-time. `None` for anonymous breakers (e.g. unit-test fixtures) which stay silent.
    label: Option<String>,
}

impl CircuitBreaker {
    /// Create a breaker in the Closed state (no metrics label — stays silent on the gauge).
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self::build(config, None)
    }

    /// Create a labelled breaker. The `label` (the provider name) is stamped on every
    /// `waav_circuit_breaker_state` sample this breaker publishes on a transition, so the gauge
    /// is attributed to the right provider. The [`crate::core::resilience::ResilienceRegistry`]
    /// uses this for every per-provider breaker it owns.
    pub fn with_label(config: CircuitBreakerConfig, label: impl Into<String>) -> Self {
        let cb = Self::build(config, Some(label.into()));
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
        circuit_state(self.core.state())
    }

    /// Whether a request (e.g. a reconnect attempt) may proceed *now*.
    ///
    /// - Closed → always `true`.
    /// - Open → `false` until the cooldown elapses, then transitions to HalfOpen and
    ///   returns `true` exactly once (the probe). Concurrent callers race for the single
    ///   probe; losers see HalfOpen and are denied.
    /// - HalfOpen → `false` while the probe is outstanding; a probe that has reported nothing
    ///   for its lease is replaced, so a lost probe cannot deny every caller until restart.
    pub fn allow_request(&self) -> bool {
        // D-G2 (review wc71hewlx #1): the FATAL state is RECOVERABLE, not
        // restart-only. It denies requests until `fatal_cooldown` elapses,
        // then admits ONE probe (CAS-claimed, like half-open) and re-arms the
        // cooldown — a stable probe clears the fatal state, a fast-dying probe
        // leaves it armed. A truly-bad key thus retries at most once per
        // `fatal_cooldown` (no tight loop); a transient cause / false-positive
        // heals on its own.
        if self.permanently_failed.load(Ordering::Acquire) {
            let opened = self.fatal_opened_at_ns.load(Ordering::Acquire);
            let elapsed = now_ns().saturating_sub(opened);
            if elapsed < self.config.fatal_cooldown.as_nanos() as u64 {
                return false;
            }
            // Claim the single probe slot: re-stamp the timestamp so a loser
            // (and the next caller) waits another full cooldown.
            return self
                .fatal_opened_at_ns
                .compare_exchange(opened, now_ns(), Ordering::AcqRel, Ordering::Acquire)
                .is_ok();
        }
        self.core.allow()
    }

    /// Record a successful outcome.
    ///
    /// - HalfOpen → close the breaker and reset the window (recovery confirmed).
    /// - Closed → tally toward the window.
    pub fn record_success(&self) {
        self.core.record_unscoped(Some(true));
    }

    /// Record a failed outcome.
    ///
    /// - HalfOpen → re-open and restart the cooldown (probe failed).
    /// - Closed → tally toward the window and trip if the rate crosses the threshold.
    pub fn record_failure(&self) {
        self.core.record_unscoped(Some(false));
    }

    /// Record an outcome that says nothing about the upstream (the caller's own malformed
    /// request): it is not counted, and a half-open probe that got it is abandoned so the next
    /// caller probes, instead of the breaker staying half-open.
    pub fn record_neutral(&self) {
        self.core.record_unscoped(None);
    }

    /// D-G2: record a connection's lifetime at close. Sub-stable, NON-clean
    /// closes (a handshake that succeeds then the SERVER drops it — bad
    /// credentials' signature) count consecutively; at `max_quick_failures`
    /// the breaker enters the (recoverable) FATAL state. A STABLE connection
    /// resets the count AND clears any fatal state (the creds work now).
    ///
    /// `intentional` (review wc71hewlx #0): a CLIENT-initiated close (a normal
    /// short hangup, an IVR probe) is NOT a quick-failure signal — counting it
    /// took healthy providers offline gateway-wide after three short legit
    /// calls. Clean closes never increment the streak.
    pub fn record_connection_closed(&self, stable_for: Duration, intentional: bool) {
        if intentional {
            return;
        }
        if stable_for >= self.config.min_stable_duration {
            // A connection proved itself: the creds/config are fine now.
            self.quick_failures.store(0, Ordering::Release);
            if self.permanently_failed.swap(false, Ordering::AcqRel) {
                tracing::info!(
                    provider = self.label.as_deref().unwrap_or("unknown"),
                    "circuit breaker recovered from FATAL: a stable connection re-established"
                );
            }
            return;
        }
        let n = self.quick_failures.fetch_add(1, Ordering::AcqRel) + 1;
        if n >= self.config.max_quick_failures {
            // Enter (or re-arm) the FATAL state and start its cooldown.
            let first = !self.permanently_failed.swap(true, Ordering::AcqRel);
            self.fatal_opened_at_ns.store(now_ns(), Ordering::Release);
            if first {
                tracing::error!(
                    provider = self.label.as_deref().unwrap_or("unknown"),
                    quick_failures = n,
                    fatal_cooldown_s = self.config.fatal_cooldown.as_secs(),
                    "circuit breaker FATAL: {} consecutive sub-{}s server-side closes \
                     (credentials/config signature); recovery probe after the cooldown",
                    n,
                    self.config.min_stable_duration.as_secs(),
                );
            }
            self.core.force_open();
        }
    }

    /// D-G2: whether the breaker is in the (recoverable) FATAL state.
    pub fn is_permanently_failed(&self) -> bool {
        self.permanently_failed.load(Ordering::Acquire)
    }

    /// Current failure rate over the window, `0.0` if there are no samples.
    pub fn error_rate(&self) -> f64 {
        let (s, f) = self.core.counts();
        if s + f == 0 {
            0.0
        } else {
            f as f64 / (s + f) as f64
        }
    }

    /// Total number of times the breaker has tripped to Open.
    pub fn total_trips(&self) -> u64 {
        self.core.total_trips()
    }

    /// A snapshot of the current counters.
    pub fn snapshot(&self) -> CircuitBreakerSnapshot {
        let (successes, failures) = self.core.counts();
        CircuitBreakerSnapshot {
            state: self.state(),
            successes: successes as u32,
            failures: failures as u32,
            total_trips: self.total_trips(),
            error_rate: self.error_rate(),
        }
    }

    /// Force the breaker back to Closed and clear the window (administrative reset).
    pub fn reset(&self) {
        self.core.reset();
    }

    // --- internals -----------------------------------------------------------------

    /// A labelled breaker publishes its gauge from inside each operation that changes the state,
    /// under the state machine's one lock: no transition is missed or published out of order.
    fn build(config: CircuitBreakerConfig, label: Option<String>) -> Self {
        let core_config = BreakerConfig {
            window: config.window_size.max(1) as usize,
            min_requests: config.min_request_volume as usize,
            failure_ratio: config.error_rate_threshold,
            cooldown: config.cooldown,
            probe_lease: config.cooldown.max(MIN_PROBE_LEASE),
        };
        let core = match label.clone() {
            Some(provider) => Breaker::with_transition_observer(
                core_config,
                Box::new(move |state| {
                    crate::core::metrics::bridge::set_circuit_breaker_state(
                        &provider,
                        circuit_state(state).as_code(),
                    )
                }),
            ),
            None => Breaker::new(core_config),
        };
        Self {
            config,
            core,
            quick_failures: AtomicU32::new(0),
            fatal_opened_at_ns: AtomicU64::new(0),
            permanently_failed: std::sync::atomic::AtomicBool::new(false),
            label,
        }
    }

    /// Publish this breaker's current state on the `waav_circuit_breaker_state{provider}` gauge,
    /// if it carries a label. A no-op for anonymous breakers.
    fn publish_state(&self) {
        if let Some(label) = self.label.as_deref() {
            crate::core::metrics::bridge::set_circuit_breaker_state(label, self.state().as_code());
        }
    }
}

fn circuit_state(state: BreakerState) -> CircuitState {
    match state {
        BreakerState::Open => CircuitState::Open,
        BreakerState::HalfOpen => CircuitState::HalfOpen,
        BreakerState::Closed => CircuitState::Closed,
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
        assert_eq!(
            cb.state(),
            CircuitState::Closed,
            "must not trip below min volume"
        );
        assert!(cb.allow_request());
    }

    #[test]
    fn breaker_trips_fatal_after_3_substable_server_closes() {
        // D-G2: bad credentials pass the handshake then the SERVER drops fast
        // — the rate-based breaker never converges; the quick-fail detector
        // enters the (recoverable) FATAL state.
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            cooldown: Duration::from_millis(1),
            fatal_cooldown: Duration::from_millis(20),
            ..Default::default()
        });
        for _ in 0..2 {
            cb.record_connection_closed(Duration::from_millis(300), false);
            assert!(!cb.is_permanently_failed());
        }
        cb.record_connection_closed(Duration::from_millis(300), false);
        assert!(cb.is_permanently_failed(), "3rd quick failure = fatal");
        // Within the fatal cooldown: denied.
        assert!(
            !cb.allow_request(),
            "fatal state denies within the cooldown"
        );
        // After the fatal cooldown: exactly ONE recovery probe is admitted,
        // then re-armed (review wc71hewlx #1: recoverable, not restart-only).
        std::thread::sleep(Duration::from_millis(25));
        assert!(
            cb.allow_request(),
            "fatal state admits a probe after the cooldown"
        );
        assert!(!cb.allow_request(), "only ONE probe per fatal cooldown");
        // A STABLE recovery connection clears the fatal state entirely.
        cb.record_connection_closed(Duration::from_secs(60), false);
        assert!(
            !cb.is_permanently_failed(),
            "a stable connection heals the fatal state"
        );
        assert!(cb.allow_request());
    }

    #[test]
    fn intentional_close_is_not_a_quick_failure() {
        // review wc71hewlx #0: normal short hangups (intentional closes) must
        // NOT count — three legit short calls used to kill a provider
        // gateway-wide.
        let cb = CircuitBreaker::with_defaults();
        for _ in 0..10 {
            cb.record_connection_closed(Duration::from_millis(200), true);
        }
        assert!(
            !cb.is_permanently_failed(),
            "clean short closes never trip the fatal state"
        );
    }

    #[test]
    fn stable_connection_resets_quick_failure_count() {
        let cb = CircuitBreaker::with_defaults();
        cb.record_connection_closed(Duration::from_millis(100), false);
        cb.record_connection_closed(Duration::from_millis(100), false);
        // A stable connection (≥ min_stable_duration) resets the streak.
        cb.record_connection_closed(Duration::from_secs(60), false);
        cb.record_connection_closed(Duration::from_millis(100), false);
        cb.record_connection_closed(Duration::from_millis(100), false);
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
        assert_eq!(
            cb.state(),
            CircuitState::Closed,
            "successful probe closes breaker"
        );
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
        assert_eq!(
            cb.state(),
            CircuitState::Open,
            "failed probe re-opens breaker"
        );
        assert_eq!(
            cb.total_trips(),
            trips_after_first + 1,
            "re-open counts as a trip"
        );
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
        assert!(
            cb.error_rate() < 0.5,
            "rate should decay: {}",
            cb.error_rate()
        );
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
        assert_eq!(
            v, 1.0,
            "half-open breaker must report state-code 1, got {v}"
        );

        cb.record_success();
        assert_eq!(cb.state(), CircuitState::Closed);
        let v = gauge_value(&bridge::render(), provider).expect("closed sample");
        assert_eq!(
            v, 0.0,
            "recovered breaker must report state-code 0 (closed), got {v}"
        );
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
