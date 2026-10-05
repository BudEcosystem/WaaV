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
}

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

    pub fn set_spec(&self, spec: LimitSpec) {
        *self.spec.lock() = spec;
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

    /// The limiter for a key, created with `spec` on first use.
    pub fn get(&self, key: &str, spec: LimitSpec) -> Arc<Limiter> {
        Arc::clone(
            self.map
                .lock()
                .entry(key.to_string())
                .or_insert_with(|| Arc::new(Limiter::new(spec))),
        )
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
