//! The one circuit-breaker state machine: segmented sessions' upload breakers, and under the
//! gateway's `CircuitBreaker` (its streaming reconnects and HTTP upload clients).
//!
//! It opens after a share of recent outcomes failed for reasons that say the vendor is unhealthy
//! (the caller decides which: 5xx, network and timeouts, never a refused request), stays open for a
//! cooldown, then admits exactly one probe. A probe that is cancelled or answered with something
//! that says nothing about the vendor does not strand it: the probe is abandoned and the next
//! caller becomes the probe; a probe that never reports is replaced after its lease.
//!
//! Two ways to report. A caller holding the [`Admission`] it was given ([`Breaker::try_acquire`],
//! [`Breaker::record`]) has a stale permit ignored: a request admitted before a trip cannot close
//! or reopen the breaker. A caller that cannot carry one ([`Breaker::allow`],
//! [`Breaker::record_unscoped`]) reports against the current state.

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
    Normal {
        generation: u64,
    },
    /// The single half-open probe: one request, no second.
    Probe {
        generation: u64,
    },
}

#[derive(Debug)]
struct Inner {
    state: BreakerState,
    outcomes: VecDeque<bool>,
    opened_at: Option<Instant>,
    probe_claimed_at: Option<Instant>,
    generation: u64,
    trips: u64,
}

impl Inner {
    fn open(&mut self, now: Instant) {
        if self.state != BreakerState::Open {
            self.trips += 1;
        }
        self.state = BreakerState::Open;
        self.opened_at = Some(now);
        self.generation += 1;
    }

    fn close(&mut self) {
        self.state = BreakerState::Closed;
        self.outcomes.clear();
        self.generation += 1;
    }

    /// A half-open probe that ends without a verdict: the next caller becomes the probe.
    fn abandon_probe(&mut self) {
        self.state = BreakerState::Open;
        self.probe_claimed_at = None;
    }

    /// One outcome in the closed state's window; a failure that brings the failure share to the
    /// threshold opens it. A success never does, even one that fills the window to its minimum.
    fn push(&mut self, ok: bool, cfg: &BreakerConfig, now: Instant) {
        if self.outcomes.len() == cfg.window {
            self.outcomes.pop_front();
        }
        self.outcomes.push_back(ok);
        if ok {
            return;
        }
        let failures = self.outcomes.iter().filter(|o| !**o).count();
        if self.outcomes.len() >= cfg.min_requests
            && failures as f64 / self.outcomes.len() as f64 >= cfg.failure_ratio
        {
            self.open(now);
        }
    }
}

/// Told the new state, under the breaker's lock, each time an operation changes it.
pub type OnTransition = Box<dyn Fn(BreakerState) + Send + Sync>;

pub struct Breaker {
    cfg: BreakerConfig,
    inner: Mutex<Inner>,
    on_transition: Option<OnTransition>,
}

impl std::fmt::Debug for Breaker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Breaker")
            .field("cfg", &self.cfg)
            .field("inner", &self.inner)
            .field("on_transition", &self.on_transition.is_some())
            .finish()
    }
}

impl Breaker {
    pub fn new(cfg: BreakerConfig) -> Self {
        Self {
            cfg,
            inner: Mutex::new(Inner {
                state: BreakerState::Closed,
                outcomes: VecDeque::new(),
                opened_at: None,
                probe_claimed_at: None,
                generation: 0,
                trips: 0,
            }),
            on_transition: None,
        }
    }

    /// A breaker that reports every change of state to `on_transition`, from inside the operation
    /// that made it: nothing in between can be missed or reported out of order. It runs under
    /// the breaker's lock, so it must not call back into the breaker.
    pub fn with_transition_observer(cfg: BreakerConfig, on_transition: OnTransition) -> Self {
        Self {
            on_transition: Some(on_transition),
            ..Self::new(cfg)
        }
    }

    /// Run `op` under the lock and report a change of state it made.
    fn update<T>(&self, op: impl FnOnce(&mut Inner) -> T) -> T {
        let mut g = self.inner.lock();
        let before = g.state;
        let out = op(&mut g);
        if g.state != before
            && let Some(on_transition) = &self.on_transition
        {
            on_transition(g.state);
        }
        out
    }

    pub fn config(&self) -> &BreakerConfig {
        &self.cfg
    }

    pub fn state(&self) -> BreakerState {
        self.inner.lock().state
    }

    /// Successes and failures in the closed state's window.
    pub fn counts(&self) -> (usize, usize) {
        let g = self.inner.lock();
        let failures = g.outcomes.iter().filter(|o| !**o).count();
        (g.outcomes.len() - failures, failures)
    }

    /// How many times the breaker has opened.
    pub fn total_trips(&self) -> u64 {
        self.inner.lock().trips
    }

    /// May a request go now? For callers that cannot carry an [`Admission`]: an elapsed cooldown
    /// admits this call as the probe.
    pub fn allow(&self) -> bool {
        !matches!(self.try_acquire(true), Admission::Denied)
    }

    /// Report an outcome against the current state, for callers without an [`Admission`]: in the
    /// closed state it counts toward the window, a half-open breaker closes on a success and
    /// reopens on a failure, and `None` (an answer that says nothing about the vendor) abandons a
    /// half-open probe.
    pub fn record_unscoped(&self, outcome: Option<bool>) {
        let now = Instant::now();
        self.update(|g| match (g.state, outcome) {
            (BreakerState::Closed, Some(ok)) => g.push(ok, &self.cfg, now),
            (BreakerState::HalfOpen, Some(true)) => g.close(),
            (BreakerState::HalfOpen, Some(false)) => g.open(now),
            (BreakerState::HalfOpen, None) => g.abandon_probe(),
            _ => {}
        })
    }

    /// Back to closed with an empty window (an operator's reset).
    pub fn reset(&self) {
        self.update(Inner::close);
    }

    /// `allow_probe = false` for a second request: it never takes the probe.
    pub fn try_acquire(&self, allow_probe: bool) -> Admission {
        let now = Instant::now();
        self.update(|g| match g.state {
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
        })
    }

    /// Record a request's outcome. `healthy`: a success, or a failure that says nothing about the
    /// vendor (`None` means "do not count").
    pub fn record(&self, admission: Admission, outcome: Option<bool>) {
        let now = Instant::now();
        self.update(|g| match admission {
            Admission::Denied => {}
            Admission::Probe { generation } => {
                if generation != g.generation || g.state != BreakerState::HalfOpen {
                    return;
                }
                match outcome {
                    Some(true) => g.close(),
                    Some(false) => g.open(now),
                    // Cancelled or refused: abandon the probe so the next caller becomes it.
                    None => g.abandon_probe(),
                }
            }
            Admission::Normal { generation } => {
                let Some(ok) = outcome else { return };
                if generation != g.generation || g.state != BreakerState::Closed {
                    return;
                }
                g.push(ok, &self.cfg, now);
            }
        })
    }

    /// The silent-host rule: a host that answers nothing at all is opened at once.
    pub fn force_open(&self) {
        let now = Instant::now();
        self.update(|g| g.open(now));
    }
}

/// Breakers by row, host and credential.
#[derive(Debug, Default)]
pub struct BreakerRegistry {
    map: Mutex<HashMap<String, Arc<Breaker>>>,
    cfg: BreakerConfig,
}

impl BreakerRegistry {
    pub fn new(cfg: BreakerConfig) -> Self {
        Self {
            map: Mutex::default(),
            cfg,
        }
    }

    pub fn get(&self, key: &str) -> Arc<Breaker> {
        Arc::clone(
            self.map
                .lock()
                .entry(key.to_string())
                .or_insert_with(|| Arc::new(Breaker::new(self.cfg))),
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

    fn fail_n(b: &Breaker, n: usize) {
        for _ in 0..n {
            let a = b.try_acquire(true);
            b.record(a, Some(false));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn it_opens_on_failures_and_admits_one_probe_after_the_cooldown() {
        let b = Breaker::new(cfg());
        fail_n(&b, 4);
        assert_eq!(b.state(), BreakerState::Open);
        assert_eq!(b.try_acquire(true), Admission::Denied);
        tokio::time::advance(Duration::from_secs(11)).await;
        assert_eq!(
            b.try_acquire(false),
            Admission::Denied,
            "a second request never takes the probe"
        );
        let probe = b.try_acquire(true);
        assert!(matches!(probe, Admission::Probe { .. }));
        assert_eq!(b.try_acquire(true), Admission::Denied, "only one probe");
        b.record(probe, Some(true));
        assert_eq!(b.state(), BreakerState::Closed);
    }

    #[tokio::test(start_paused = true)]
    async fn a_cancelled_probe_does_not_strand_the_breaker() {
        let b = Breaker::new(cfg());
        fail_n(&b, 4);
        tokio::time::advance(Duration::from_secs(11)).await;
        let probe = b.try_acquire(true);
        b.record(probe, None);
        let next = b.try_acquire(true);
        assert!(
            matches!(next, Admission::Probe { .. }),
            "the next caller becomes the probe at once"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_probe_that_never_reports_is_replaced_after_its_lease() {
        let b = Breaker::new(cfg());
        fail_n(&b, 4);
        tokio::time::advance(Duration::from_secs(11)).await;
        let _lost = b.try_acquire(true);
        assert_eq!(b.try_acquire(true), Admission::Denied);
        tokio::time::advance(Duration::from_secs(16)).await;
        assert!(matches!(b.try_acquire(true), Admission::Probe { .. }));
    }

    #[test]
    fn outcomes_that_say_nothing_about_the_vendor_are_not_counted() {
        let b = Breaker::new(cfg());
        for _ in 0..10 {
            let a = b.try_acquire(true);
            b.record(a, None);
        }
        assert_eq!(b.state(), BreakerState::Closed);
    }

    #[tokio::test(start_paused = true)]
    async fn an_unscoped_caller_drives_the_same_states() {
        let b = Breaker::new(cfg());
        for _ in 0..4 {
            assert!(b.allow());
            b.record_unscoped(Some(false));
        }
        assert_eq!(b.state(), BreakerState::Open);
        assert_eq!(b.total_trips(), 1);
        assert!(!b.allow());
        tokio::time::advance(Duration::from_secs(11)).await;
        assert!(b.allow(), "the cooldown admits the probe");
        assert!(!b.allow(), "only one probe");
        // A refused request (a 4xx) says nothing about the vendor: the probe is abandoned and the
        // next caller probes, instead of the breaker staying half-open for good.
        b.record_unscoped(None);
        assert!(b.allow());
        b.record_unscoped(Some(true));
        assert_eq!(b.state(), BreakerState::Closed);
        assert_eq!(b.counts(), (0, 0));
        b.record_unscoped(Some(true));
        b.record_unscoped(Some(false));
        assert_eq!(b.counts(), (1, 1));
        b.reset();
        assert_eq!(b.counts(), (0, 0));
        assert_eq!(b.total_trips(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn every_change_of_state_is_reported_once_in_order() {
        let seen: Arc<Mutex<Vec<BreakerState>>> = Arc::default();
        let sink = Arc::clone(&seen);
        let b = Breaker::with_transition_observer(cfg(), Box::new(move |s| sink.lock().push(s)));
        for _ in 0..4 {
            assert!(b.allow());
            b.record_unscoped(Some(false));
        }
        assert!(!b.allow());
        tokio::time::advance(Duration::from_secs(11)).await;
        assert!(b.allow());
        b.record_unscoped(None);
        assert!(b.allow());
        b.record_unscoped(Some(true));
        b.record_unscoped(Some(true));
        b.force_open();
        b.reset();
        assert_eq!(
            *seen.lock(),
            vec![
                BreakerState::Open,
                BreakerState::HalfOpen,
                BreakerState::Open,
                BreakerState::HalfOpen,
                BreakerState::Closed,
                BreakerState::Open,
                BreakerState::Closed,
            ]
        );
    }

    #[test]
    fn a_success_never_opens_the_breaker() {
        let b = Breaker::new(cfg());
        b.record_unscoped(Some(false));
        b.record_unscoped(Some(false));
        b.record_unscoped(Some(true));
        b.record_unscoped(Some(true));
        assert_eq!(
            b.state(),
            BreakerState::Closed,
            "the window is half failures, after a success"
        );
        b.record_unscoped(Some(false));
        assert_eq!(b.state(), BreakerState::Open, "the next failure trips it");
    }

    #[test]
    fn a_stale_permit_cannot_move_the_breaker() {
        let b = Breaker::new(cfg());
        let old = b.try_acquire(true);
        b.force_open();
        b.record(old, Some(true));
        assert_eq!(b.state(), BreakerState::Open);
    }
}
