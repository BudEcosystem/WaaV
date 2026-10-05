//! The limiter gate and the retry budget.
//!
//! Request rate, not price, is the limit that binds first: two hundred calls at three uploads a
//! turn is 3,000 requests a minute, against Groq's base 400 and OpenAI's first tier of 500. One
//! limiter per vendor host, credential and model keeps uploads under the limit. A rate-limit answer
//! slows the limiter and never opens the breaker, because the breaker is shared by every tenant.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;

/// What the limiter allows: a rate (requests per minute, already at the 80% target) and a cap on
/// concurrent requests.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LimitSpec {
    pub requests_per_minute: f64,
    pub burst: f64,
    pub max_concurrent: usize,
}

impl LimitSpec {
    /// 80% of a published per-minute limit, with a burst of a few seconds' worth.
    pub fn from_rpm(published_rpm: u32, max_concurrent: usize) -> Self {
        let rpm = published_rpm as f64 * 0.8;
        Self {
            requests_per_minute: rpm,
            burst: (rpm / 60.0 * 5.0).max(2.0),
            max_concurrent: max_concurrent.max(1),
        }
    }

    /// No published limit: a generous ceiling that still bounds a runaway session.
    pub fn unlimited() -> Self {
        Self {
            requests_per_minute: 60_000.0,
            burst: 1000.0,
            max_concurrent: 256,
        }
    }
}

/// Why the gate refused a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateRefusal {
    /// No token before the latest instant the caller could wait to.
    QueueTimeout,
    /// An hourly or daily budget of the row is spent; nothing was sent.
    BudgetExhausted,
}

/// What a long window counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowMetric {
    Requests,
    AudioSeconds,
}

/// A row's hourly or daily limit (Groq's audio hours, a daily request cap), at the 80% target.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LongWindow {
    pub span: Duration,
    pub metric: WindowMetric,
    pub limit: f64,
}

impl LongWindow {
    pub fn new(span: Duration, metric: WindowMetric, published: f64) -> Self {
        Self {
            span,
            metric,
            limit: published * 0.8,
        }
    }
}

#[derive(Debug)]
struct WindowLedger {
    window: LongWindow,
    events: std::collections::VecDeque<(Instant, f64)>,
    used: f64,
}

impl WindowLedger {
    fn expire(&mut self, now: Instant) {
        while let Some((at, v)) = self.events.front().copied() {
            if now.saturating_duration_since(at) < self.window.span {
                break;
            }
            self.events.pop_front();
            self.used -= v;
        }
        self.used = self.used.max(0.0);
    }

    fn remaining_fraction(&self) -> f64 {
        ((self.window.limit - self.used) / self.window.limit.max(1e-9)).clamp(0.0, 1.0)
    }
}

/// What a new session is told about a key at setup.
#[derive(Debug, Clone, PartialEq)]
pub enum Admission {
    Ok,
    /// Less than a fifth of an hourly or daily budget is left (`stt_capacity_low`).
    CapacityLow {
        remaining: f64,
    },
    /// Three uploads lost to rate limits within 30 s (`stt_overloaded`).
    Overloaded {
        retry_after: Duration,
    },
}

/// Losses to rate limits within this span mark a key overloaded.
const OVERLOAD_SPAN: Duration = Duration::from_secs(30);
const OVERLOAD_LOSSES: usize = 3;
const OVERLOAD_HOLD: Duration = Duration::from_secs(60);

/// Holds a concurrency slot for one request.
#[derive(Debug)]
pub struct GatePass {
    _slot: OwnedSemaphorePermit,
    pub waited: Duration,
}

#[derive(Debug)]
struct Bucket {
    tokens: f64,
    last: Instant,
    /// The vendor asked us to wait until this instant.
    paused_until: Option<Instant>,
    /// Requests refused or delayed since the last pressure report.
    pressure: u32,
}

/// One limiter key: host, credential and model.
#[derive(Debug)]
pub struct Limiter {
    spec: Mutex<LimitSpec>,
    bucket: Mutex<Bucket>,
    slots: Arc<Semaphore>,
    windows: Mutex<Vec<WindowLedger>>,
    losses: Mutex<std::collections::VecDeque<Instant>>,
    overloaded_until: Mutex<Option<Instant>>,
}

impl Limiter {
    pub fn new(spec: LimitSpec) -> Self {
        Self {
            bucket: Mutex::new(Bucket {
                tokens: spec.burst,
                last: Instant::now(),
                paused_until: None,
                pressure: 0,
            }),
            slots: Arc::new(Semaphore::new(spec.max_concurrent)),
            spec: Mutex::new(spec),
            windows: Mutex::new(Vec::new()),
            losses: Mutex::new(Default::default()),
            overloaded_until: Mutex::new(None),
        }
    }

    /// The row's hourly and daily limits. Ledgers of windows already known are kept.
    pub fn set_windows(&self, windows: &[LongWindow]) {
        let mut g = self.windows.lock();
        if g.len() == windows.len() && g.iter().zip(windows).all(|(l, w)| l.window == *w) {
            return;
        }
        *g = windows
            .iter()
            .map(|w| WindowLedger {
                window: *w,
                events: Default::default(),
                used: 0.0,
            })
            .collect();
    }

    /// One request reached the vendor with this much audio.
    pub fn record_upload(&self, audio: Duration) {
        let now = Instant::now();
        for l in self.windows.lock().iter_mut() {
            let v = match l.window.metric {
                WindowMetric::Requests => 1.0,
                WindowMetric::AudioSeconds => audio.as_secs_f64(),
            };
            l.events.push_back((now, v));
            l.used += v;
        }
    }

    /// Whether every long window has room for one more request.
    fn windows_open(&self, now: Instant) -> bool {
        let mut g = self.windows.lock();
        g.iter_mut().all(|l| {
            l.expire(now);
            match l.window.metric {
                WindowMetric::Requests => l.used + 1.0 <= l.window.limit,
                WindowMetric::AudioSeconds => l.used < l.window.limit,
            }
        })
    }

    /// An upload lost to a rate limit (the vendor's or this gate's): three within 30 s mark the
    /// key overloaded for a minute.
    pub fn note_rate_limited_loss(&self) {
        let now = Instant::now();
        let mut l = self.losses.lock();
        l.push_back(now);
        while l
            .front()
            .is_some_and(|t| now.saturating_duration_since(*t) > OVERLOAD_SPAN)
        {
            l.pop_front();
        }
        if l.len() >= OVERLOAD_LOSSES {
            *self.overloaded_until.lock() = Some(now + OVERLOAD_HOLD);
            l.clear();
        }
    }

    /// What a session starting on this key is told.
    pub fn admission(&self) -> Admission {
        let now = Instant::now();
        if let Some(until) = *self.overloaded_until.lock()
            && until > now
        {
            return Admission::Overloaded {
                retry_after: until - now,
            };
        }
        let mut g = self.windows.lock();
        let remaining = g
            .iter_mut()
            .map(|l| {
                l.expire(now);
                l.remaining_fraction()
            })
            .fold(1.0f64, f64::min);
        if remaining < 0.2 {
            Admission::CapacityLow { remaining }
        } else {
            Admission::Ok
        }
    }

    fn refill(&self, b: &mut Bucket, now: Instant) {
        let spec = *self.spec.lock();
        let elapsed = now.saturating_duration_since(b.last).as_secs_f64();
        b.tokens = (b.tokens + elapsed * spec.requests_per_minute / 60.0).min(spec.burst);
        b.last = now;
    }

    /// Whether a request could start at once, without waiting. Used by the engine's merge rule:
    /// a unit that is not the turn's last starts only when the limiter has headroom.
    pub fn has_headroom(&self) -> bool {
        let now = Instant::now();
        let mut b = self.bucket.lock();
        self.refill(&mut b, now);
        b.paused_until.is_none_or(|t| t <= now)
            && b.tokens >= 1.0
            && self.slots.available_permits() > 0
    }

    /// Wait for a token and a slot, no later than `latest`.
    pub async fn acquire(&self, latest: Instant) -> Result<GatePass, GateRefusal> {
        let start = Instant::now();
        if !self.windows_open(start) {
            self.bucket.lock().pressure += 1;
            return Err(GateRefusal::BudgetExhausted);
        }
        loop {
            let now = Instant::now();
            let wait = {
                let mut b = self.bucket.lock();
                self.refill(&mut b, now);
                match b.paused_until {
                    Some(t) if t > now => Some(t - now),
                    _ => {
                        if b.tokens >= 1.0 {
                            b.tokens -= 1.0;
                            None
                        } else {
                            let rate = self.spec.lock().requests_per_minute / 60.0;
                            Some(Duration::from_secs_f64(
                                ((1.0 - b.tokens) / rate.max(1e-6)).max(0.001),
                            ))
                        }
                    }
                }
            };
            match wait {
                None => break,
                Some(w) => {
                    if now + w > latest {
                        self.bucket.lock().pressure += 1;
                        return Err(GateRefusal::QueueTimeout);
                    }
                    self.bucket.lock().pressure += 1;
                    tokio::time::sleep(w).await;
                }
            }
        }
        let slot =
            match tokio::time::timeout_at(latest, Arc::clone(&self.slots).acquire_owned()).await {
                Ok(Ok(slot)) => slot,
                _ => {
                    // Return the token we took: nothing was sent.
                    self.bucket.lock().tokens += 1.0;
                    return Err(GateRefusal::QueueTimeout);
                }
            };
        Ok(GatePass {
            _slot: slot,
            waited: Instant::now().saturating_duration_since(start),
        })
    }

    /// A rate-limit answer: pause the key until the vendor's `Retry-After` (at most 60 s), and
    /// drain the bucket so the next requests space out.
    pub fn observe_rate_limited(&self, retry_after: Option<Duration>) {
        let now = Instant::now();
        let mut b = self.bucket.lock();
        let pause = retry_after
            .unwrap_or(Duration::from_millis(1000))
            .min(Duration::from_secs(60));
        b.paused_until = Some(b.paused_until.map_or(now + pause, |t| t.max(now + pause)));
        b.tokens = b.tokens.min(0.0);
        b.pressure += 1;
    }

    /// Requests delayed or refused since the last call, then reset.
    pub fn take_pressure(&self) -> u32 {
        std::mem::take(&mut self.bucket.lock().pressure)
    }

    /// A changed limit (a deployment override, a new capability map) applies from now on: tokens
    /// earned at the old rate are kept up to the new burst, and concurrency slots are added or
    /// retired (a slot in use is retired when it is released).
    pub fn set_spec(&self, spec: LimitSpec) {
        let mut b = self.bucket.lock();
        self.refill(&mut b, Instant::now());
        let mut current = self.spec.lock();
        if *current == spec {
            return;
        }
        b.tokens = b.tokens.min(spec.burst);
        if spec.max_concurrent > current.max_concurrent {
            self.slots
                .add_permits(spec.max_concurrent - current.max_concurrent);
        } else if spec.max_concurrent < current.max_concurrent {
            let retire = current.max_concurrent - spec.max_concurrent;
            let retired = self.slots.forget_permits(retire);
            if retired < retire {
                let slots = Arc::clone(&self.slots);
                let rest = (retire - retired) as u32;
                tokio::spawn(async move {
                    if let Ok(p) = slots.acquire_many_owned(rest).await {
                        p.forget();
                    }
                });
            }
        }
        *current = spec;
    }
}

/// Limiters by key, shared by every session of the process.
#[derive(Debug, Default)]
pub struct LimiterRegistry {
    map: Mutex<HashMap<String, Arc<Limiter>>>,
}

impl LimiterRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// The limiter for a key, created with `spec` on first use and brought to `spec` after: the
    /// latest session's limits (its deployment's override, the current map) are the key's.
    pub fn get(&self, key: &str, spec: LimitSpec) -> Arc<Limiter> {
        let limiter = Arc::clone(
            self.map
                .lock()
                .entry(key.to_string())
                .or_insert_with(|| Arc::new(Limiter::new(spec))),
        );
        limiter.set_spec(spec);
        limiter
    }
}

/// Second requests may be at most 10% of requests, plus a burst of 10, per provider and credential.
#[derive(Debug)]
pub struct RetryBudget {
    inner: Mutex<(f64, f64)>, // (tokens, ratio)
    cap: f64,
}

impl Default for RetryBudget {
    fn default() -> Self {
        Self::new(0.1, 10.0)
    }
}

impl RetryBudget {
    pub fn new(ratio: f64, burst: f64) -> Self {
        Self {
            inner: Mutex::new((burst, ratio)),
            cap: burst,
        }
    }

    /// Every first request earns a tenth of a token.
    pub fn on_request(&self) {
        let mut g = self.inner.lock();
        g.0 = (g.0 + g.1).min(self.cap);
    }

    /// Spend a token for a second request.
    pub fn try_spend(&self) -> bool {
        let mut g = self.inner.lock();
        // Ten tenths summed in floating point fall just short of one.
        if g.0 >= 1.0 - 1e-9 {
            g.0 = (g.0 - 1.0).max(0.0);
            true
        } else {
            false
        }
    }
}

/// Retry budgets by provider and credential.
#[derive(Debug, Default)]
pub struct BudgetRegistry {
    map: Mutex<HashMap<String, Arc<RetryBudget>>>,
}

impl BudgetRegistry {
    pub fn get(&self, key: &str) -> Arc<RetryBudget> {
        Arc::clone(self.map.lock().entry(key.to_string()).or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A changed limit reaches the live limiter (found on pde-ditto: a deployment override of
    /// Azure's 3-requests-a-minute default never applied until the gateway restarted).
    #[tokio::test(start_paused = true)]
    async fn a_changed_limit_applies_to_the_limiter_already_in_use() {
        let reg = LimiterRegistry::new();
        let slow = reg.get("k", LimitSpec::from_rpm(3, 1));
        while slow.has_headroom() {
            let _ = slow.acquire(Instant::now()).await.unwrap();
        }
        // At 2.4 a minute the next token is 25 s away.
        tokio::time::advance(Duration::from_millis(500)).await;
        assert!(!slow.has_headroom());

        let fast = reg.get("k", LimitSpec::from_rpm(600, 4));
        assert!(
            Arc::ptr_eq(&slow, &fast),
            "one limiter per key, updated in place"
        );
        tokio::time::advance(Duration::from_millis(500)).await;
        assert!(fast.has_headroom(), "the new rate refills the bucket");
        let passes: Vec<_> = futures_util_join(&fast, 4).await;
        assert_eq!(
            passes.len(),
            4,
            "four requests at once under the new concurrency"
        );

        // Back down: the slots and the burst shrink with it.
        drop(passes);
        // Plenty of tokens, so only a slot can refuse below.
        tokio::time::advance(Duration::from_secs(3)).await;
        let narrow = reg.get("k", LimitSpec::from_rpm(600, 1));
        let one = narrow
            .acquire(Instant::now() + Duration::from_secs(1))
            .await
            .unwrap();
        assert!(!narrow.has_headroom(), "the one slot is taken");
        assert!(
            narrow
                .acquire(Instant::now() + Duration::from_millis(50))
                .await
                .is_err()
        );
        drop(one);
    }

    async fn futures_util_join(l: &Limiter, n: usize) -> Vec<GatePass> {
        let mut out = Vec::new();
        for _ in 0..n {
            out.push(
                l.acquire(Instant::now() + Duration::from_secs(1))
                    .await
                    .unwrap(),
            );
        }
        out
    }

    #[tokio::test(start_paused = true)]
    async fn an_hourly_budget_refuses_at_the_gate_and_frees_up_as_the_hour_moves() {
        let l = Limiter::new(LimitSpec::unlimited());
        l.set_windows(&[LongWindow::new(
            Duration::from_secs(3600),
            WindowMetric::AudioSeconds,
            100.0,
        )]);
        for _ in 0..8 {
            l.acquire(Instant::now() + Duration::from_secs(1))
                .await
                .unwrap();
            l.record_upload(Duration::from_secs(10));
        }
        assert_eq!(l.admission(), Admission::CapacityLow { remaining: 0.0 });
        assert_eq!(
            l.acquire(Instant::now() + Duration::from_secs(1))
                .await
                .unwrap_err(),
            GateRefusal::BudgetExhausted
        );
        tokio::time::advance(Duration::from_secs(3601)).await;
        assert_eq!(l.admission(), Admission::Ok);
        assert!(
            l.acquire(Instant::now() + Duration::from_secs(1))
                .await
                .is_ok()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_daily_request_cap_counts_requests() {
        let l = Limiter::new(LimitSpec::unlimited());
        l.set_windows(&[LongWindow::new(
            Duration::from_secs(86_400),
            WindowMetric::Requests,
            10.0,
        )]);
        for _ in 0..7 {
            l.record_upload(Duration::from_secs(3));
        }
        assert!(
            matches!(l.admission(), Admission::CapacityLow { .. }),
            "{:?}",
            l.admission()
        );
        l.record_upload(Duration::from_secs(3));
        assert!(
            l.acquire(Instant::now() + Duration::from_secs(1))
                .await
                .is_err()
        );
        // The same windows again keep their ledger.
        l.set_windows(&[LongWindow::new(
            Duration::from_secs(86_400),
            WindowMetric::Requests,
            10.0,
        )]);
        assert!(
            l.acquire(Instant::now() + Duration::from_secs(1))
                .await
                .is_err()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn three_rate_limited_losses_in_thirty_seconds_mark_the_key_overloaded() {
        let l = Limiter::new(LimitSpec::unlimited());
        l.note_rate_limited_loss();
        tokio::time::advance(Duration::from_secs(31)).await;
        l.note_rate_limited_loss();
        l.note_rate_limited_loss();
        assert_eq!(l.admission(), Admission::Ok, "the first loss aged out");
        l.note_rate_limited_loss();
        assert!(matches!(l.admission(), Admission::Overloaded { .. }));
        tokio::time::advance(Duration::from_secs(61)).await;
        assert_eq!(l.admission(), Admission::Ok);
    }

    #[tokio::test(start_paused = true)]
    async fn a_burst_passes_then_requests_are_spaced_at_the_rate() {
        let l = Limiter::new(LimitSpec {
            requests_per_minute: 60.0,
            burst: 2.0,
            max_concurrent: 10,
        });
        let far = Instant::now() + Duration::from_secs(60);
        let a = l.acquire(far).await.unwrap();
        let b = l.acquire(far).await.unwrap();
        assert_eq!(a.waited, Duration::ZERO);
        assert_eq!(b.waited, Duration::ZERO);
        let c = l.acquire(far).await.unwrap();
        assert!(c.waited >= Duration::from_millis(990), "{:?}", c.waited);
    }

    #[tokio::test(start_paused = true)]
    async fn a_request_that_cannot_start_before_its_latest_instant_is_refused_and_counted() {
        let l = Limiter::new(LimitSpec {
            requests_per_minute: 6.0,
            burst: 1.0,
            max_concurrent: 10,
        });
        let soon = Instant::now() + Duration::from_millis(1500);
        let _a = l.acquire(soon).await.unwrap();
        assert!(!l.has_headroom());
        assert_eq!(
            l.acquire(soon).await.unwrap_err(),
            GateRefusal::QueueTimeout
        );
        assert!(l.take_pressure() >= 1);
        assert_eq!(l.take_pressure(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn concurrency_is_capped_and_a_slot_frees_on_drop() {
        let l = Limiter::new(LimitSpec {
            requests_per_minute: 6000.0,
            burst: 100.0,
            max_concurrent: 1,
        });
        let soon = Instant::now() + Duration::from_millis(100);
        let a = l.acquire(soon).await.unwrap();
        assert!(l.acquire(soon).await.is_err());
        drop(a);
        assert!(
            l.acquire(Instant::now() + Duration::from_millis(100))
                .await
                .is_ok()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_rate_limit_answer_pauses_the_key_for_the_retry_after() {
        let l = Limiter::new(LimitSpec::from_rpm(400, 10));
        l.observe_rate_limited(Some(Duration::from_secs(2)));
        assert!(!l.has_headroom());
        let pass = l
            .acquire(Instant::now() + Duration::from_secs(10))
            .await
            .unwrap();
        assert!(pass.waited >= Duration::from_secs(2));
    }

    #[test]
    fn limit_spec_targets_eighty_percent_of_the_published_rate() {
        let s = LimitSpec::from_rpm(400, 4);
        assert!((s.requests_per_minute - 320.0).abs() < 1e-9);
        assert!(s.burst >= 2.0);
    }

    #[test]
    fn the_retry_budget_is_a_tenth_of_requests_plus_a_burst() {
        let b = RetryBudget::new(0.1, 2.0);
        assert!(b.try_spend());
        assert!(b.try_spend());
        assert!(!b.try_spend());
        for _ in 0..10 {
            b.on_request();
        }
        assert!(b.try_spend());
        assert!(!b.try_spend());
    }

    #[test]
    fn registries_share_one_limiter_per_key() {
        let r = LimiterRegistry::new();
        let a = r.get("k", LimitSpec::unlimited());
        let b = r.get("k", LimitSpec::from_rpm(1, 1));
        assert!(Arc::ptr_eq(&a, &b));
        let budgets = BudgetRegistry::default();
        assert!(Arc::ptr_eq(&budgets.get("x"), &budgets.get("x")));
    }
}
