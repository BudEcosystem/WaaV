//! A breaker for file uploads, separate from the same vendor's streaming breaker.
//!
//! It opens after a share of recent requests failed for reasons that say the vendor is unhealthy
//! (5xx, network, timeouts; never a rate limit or a refused request), stays open for a cooldown,
//! then admits exactly one probe. Only the probe's outcome moves a half-open breaker, and a probe
//! that is cancelled or answered with a 4xx does not strand it: the probe is abandoned and the
//! next caller becomes the probe (the defect the audit found in the shared breaker).

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use tokio::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BreakerConfig {
    /// Outcomes considered.
    pub window: usize,
    /// At least this many outcomes before the breaker can open.
    pub min_requests: usize,
    /// Failure share at which it opens.
    pub failure_ratio: f64,
    pub cooldown: Duration,
    /// A probe that reports nothing for this long is replaced.
    pub probe_lease: Duration,
}

impl Default for BreakerConfig {
    fn default() -> Self {
        Self {
            window: 20,
            min_requests: 8,
            failure_ratio: 0.5,
            cooldown: Duration::from_secs(10),
            probe_lease: Duration::from_secs(15),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakerState {
    Closed,
    Open,
    HalfOpen,
}

/// The answer to "may I send?".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    Denied,
    Normal { generation: u64 },
    /// The single half-open probe: one request, no second.
    Probe { generation: u64 },
}

#[derive(Debug)]
struct Inner {
    state: BreakerState,
    outcomes: VecDeque<bool>,
    opened_at: Option<Instant>,
    probe_claimed_at: Option<Instant>,
    generation: u64,
}

#[derive(Debug)]
pub struct FileBreaker {
    cfg: BreakerConfig,
    inner: Mutex<Inner>,
}

impl FileBreaker {
    pub fn new(cfg: BreakerConfig) -> Self {
        Self {
            cfg,
            inner: Mutex::new(Inner {
                state: BreakerState::Closed,
                outcomes: VecDeque::new(),
                opened_at: None,
                probe_claimed_at: None,
                generation: 0,
            }),
        }
    }

    pub fn state(&self) -> BreakerState {
        self.inner.lock().state
    }

    /// `allow_probe = false` for a second request: it never takes the probe.
    pub fn try_acquire(&self, allow_probe: bool) -> Admission {
        let now = Instant::now();
        let mut g = self.inner.lock();
        match g.state {
            BreakerState::Closed => Admission::Normal {
                generation: g.generation,
            },
            BreakerState::Open => {
                let cooled = g.opened_at.is_some_and(|t| now >= t + self.cfg.cooldown);
                if cooled && allow_probe {
                    g.state = BreakerState::HalfOpen;
                    g.generation += 1;
                    g.probe_claimed_at = Some(now);
                    Admission::Probe {
                        generation: g.generation,
                    }
                } else {
                    Admission::Denied
                }
            }
            BreakerState::HalfOpen => {
                let expired = g
                    .probe_claimed_at
                    .is_none_or(|t| now >= t + self.cfg.probe_lease);
                if expired && allow_probe {
                    g.generation += 1;
                    g.probe_claimed_at = Some(now);
                    Admission::Probe {
                        generation: g.generation,
                    }
                } else {
                    Admission::Denied
                }
            }
        }
    }

    /// Record a request's outcome. `healthy`: a success, or a failure that says nothing about the
    /// vendor (`None` means "do not count").
    pub fn record(&self, admission: Admission, outcome: Option<bool>) {
        let now = Instant::now();
        let mut g = self.inner.lock();
        match admission {
            Admission::Denied => {}
            Admission::Probe { generation } => {
                if generation != g.generation || g.state != BreakerState::HalfOpen {
                    return;
                }
                match outcome {
                    Some(true) => {
                        g.state = BreakerState::Closed;
                        g.outcomes.clear();
                        g.generation += 1;
                    }
                    Some(false) => {
                        g.state = BreakerState::Open;
                        g.opened_at = Some(now);
                        g.generation += 1;
                    }
                    // Cancelled or refused: abandon the probe so the next caller becomes it.
                    None => {
                        g.state = BreakerState::Open;
                        g.probe_claimed_at = None;
                    }
                }
            }
            Admission::Normal { generation } => {
                let Some(ok) = outcome else { return };
                if generation != g.generation || g.state != BreakerState::Closed {
                    return;
                }
                if g.outcomes.len() == self.cfg.window {
                    g.outcomes.pop_front();
                }
                g.outcomes.push_back(ok);
                let failures = g.outcomes.iter().filter(|o| !**o).count();
                if g.outcomes.len() >= self.cfg.min_requests
                    && failures as f64 / g.outcomes.len() as f64 >= self.cfg.failure_ratio
                {
                    g.state = BreakerState::Open;
                    g.opened_at = Some(now);
                    g.generation += 1;
                }
            }
        }
    }

    /// The silent-host rule: a host that answers nothing at all is opened at once.
    pub fn force_open(&self) {
        let mut g = self.inner.lock();
        g.state = BreakerState::Open;
        g.opened_at = Some(Instant::now());
        g.generation += 1;
    }
}

/// Breakers by row, host and credential.
#[derive(Debug, Default)]
pub struct BreakerRegistry {
    map: Mutex<HashMap<String, Arc<FileBreaker>>>,
    cfg: BreakerConfig,
}

impl BreakerRegistry {
    pub fn new(cfg: BreakerConfig) -> Self {
        Self {
            map: Mutex::default(),
            cfg,
        }
    }

    pub fn get(&self, key: &str) -> Arc<FileBreaker> {
        Arc::clone(
            self.map
                .lock()
                .entry(key.to_string())
                .or_insert_with(|| Arc::new(FileBreaker::new(self.cfg))),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> BreakerConfig {
        BreakerConfig {
            window: 4,
            min_requests: 4,
            failure_ratio: 0.5,
            cooldown: Duration::from_secs(10),
            probe_lease: Duration::from_secs(15),
        }
    }

    fn fail_n(b: &FileBreaker, n: usize) {
        for _ in 0..n {
            let a = b.try_acquire(true);
            b.record(a, Some(false));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn it_opens_on_failures_and_admits_one_probe_after_the_cooldown() {
        let b = FileBreaker::new(cfg());
        fail_n(&b, 4);
        assert_eq!(b.state(), BreakerState::Open);
        assert_eq!(b.try_acquire(true), Admission::Denied);
        tokio::time::advance(Duration::from_secs(11)).await;
        assert_eq!(b.try_acquire(false), Admission::Denied, "a second request never takes the probe");
        let probe = b.try_acquire(true);
        assert!(matches!(probe, Admission::Probe { .. }));
        assert_eq!(b.try_acquire(true), Admission::Denied, "only one probe");
        b.record(probe, Some(true));
        assert_eq!(b.state(), BreakerState::Closed);
    }

    #[tokio::test(start_paused = true)]
    async fn a_cancelled_probe_does_not_strand_the_breaker() {
        let b = FileBreaker::new(cfg());
        fail_n(&b, 4);
        tokio::time::advance(Duration::from_secs(11)).await;
        let probe = b.try_acquire(true);
        b.record(probe, None);
        let next = b.try_acquire(true);
        assert!(matches!(next, Admission::Probe { .. }), "the next caller becomes the probe at once");
    }

    #[tokio::test(start_paused = true)]
    async fn a_probe_that_never_reports_is_replaced_after_its_lease() {
        let b = FileBreaker::new(cfg());
        fail_n(&b, 4);
        tokio::time::advance(Duration::from_secs(11)).await;
        let _lost = b.try_acquire(true);
        assert_eq!(b.try_acquire(true), Admission::Denied);
        tokio::time::advance(Duration::from_secs(16)).await;
        assert!(matches!(b.try_acquire(true), Admission::Probe { .. }));
    }

    #[test]
    fn outcomes_that_say_nothing_about_the_vendor_are_not_counted() {
        let b = FileBreaker::new(cfg());
        for _ in 0..10 {
            let a = b.try_acquire(true);
            b.record(a, None);
        }
        assert_eq!(b.state(), BreakerState::Closed);
    }

    #[test]
    fn a_stale_permit_cannot_move_the_breaker() {
        let b = FileBreaker::new(cfg());
        let old = b.try_acquire(true);
        b.force_open();
        b.record(old, Some(true));
        assert_eq!(b.state(), BreakerState::Open);
    }
}
