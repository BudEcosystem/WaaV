//! Global reconnect governor — storm control for in-flight reconnects.
//!
//! When an upstream provider (or the network) has a wide outage, every live session that
//! was using that provider will try to reconnect at roughly the same instant. Without a
//! global cap this self-inflicts a **thundering herd**: hundreds of simultaneous TCP+TLS
//! handshakes that overload the provider (or our own egress) right when it is most
//! fragile, turning a brief blip into a sustained outage.
//!
//! The [`ReconnectGovernor`] caps the number of *concurrent in-flight reconnect attempts*
//! across the whole process with a [`tokio::sync::Semaphore`]. A reconnecting task must
//! acquire a permit before it dials; the permit is released (RAII) when the attempt
//! finishes (success or failure). Per-provider backoff/jitter (the
//! [`crate::core::websocket::ReconnectionManager`]) still spreads attempts in time; the
//! governor bounds them in *space* (concurrency) so the instantaneous burst can never
//! exceed the cap regardless of how many sessions are affected.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// A held reconnect slot. Dropping it returns the permit to the governor.
///
/// While held, [`ReconnectGovernor::in_flight`] counts this attempt. The wrapper exists
/// so the in-flight gauge stays exact even on panics/early returns (RAII).
pub struct ReconnectPermit {
    _permit: Option<OwnedSemaphorePermit>,
    in_flight: Arc<AtomicU64>,
}

impl Drop for ReconnectPermit {
    fn drop(&mut self) {
        self.in_flight.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Caps concurrent in-flight reconnects process-wide to prevent a thundering herd.
///
/// Cheap to [`Clone`] (it is `Arc`-backed) so a single governor can be shared across all
/// sessions/providers via [`crate::core::state::CoreState`].
#[derive(Clone)]
pub struct ReconnectGovernor {
    semaphore: Arc<Semaphore>,
    max_concurrent: usize,
    in_flight: Arc<AtomicU64>,
    /// Cumulative number of reconnect attempts that had to wait for a permit.
    total_throttled: Arc<AtomicU64>,
}

impl ReconnectGovernor {
    /// Create a governor allowing at most `max_concurrent` simultaneous reconnects.
    ///
    /// `max_concurrent` is clamped to at least 1 (a cap of 0 would deadlock every
    /// reconnect forever).
    pub fn new(max_concurrent: usize) -> Self {
        let cap = max_concurrent.max(1);
        Self {
            semaphore: Arc::new(Semaphore::new(cap)),
            max_concurrent: cap,
            in_flight: Arc::new(AtomicU64::new(0)),
            total_throttled: Arc::new(AtomicU64::new(0)),
        }
    }

    /// The configured concurrency cap.
    pub fn max_concurrent(&self) -> usize {
        self.max_concurrent
    }

    /// Number of reconnect attempts currently holding a permit.
    pub fn in_flight(&self) -> u64 {
        self.in_flight.load(Ordering::Acquire)
    }

    /// Permits currently available (cap minus in-flight).
    pub fn available_permits(&self) -> usize {
        self.semaphore.available_permits()
    }

    /// Cumulative count of reconnects that had to wait for a free slot.
    pub fn total_throttled(&self) -> u64 {
        self.total_throttled.load(Ordering::Relaxed)
    }

    /// Acquire a reconnect slot, waiting (async) until one is free.
    ///
    /// The returned [`ReconnectPermit`] must be held for the duration of the dial; drop it
    /// when the attempt completes. The number of live permits can never exceed the cap, so
    /// `in_flight() <= max_concurrent()` is an invariant for all callers of this method.
    pub async fn acquire(&self) -> ReconnectPermit {
        if self.semaphore.available_permits() == 0 {
            self.total_throttled.fetch_add(1, Ordering::Relaxed);
        }
        // The semaphore is private and should never be closed. If that invariant
        // is broken by future refactoring, keep the reconnect task from
        // unwinding; the log makes the cap loss visible.
        let permit = match Arc::clone(&self.semaphore).acquire_owned().await {
            Ok(permit) => Some(permit),
            Err(_) => {
                tracing::error!(
                    "reconnect governor semaphore was closed; reconnect proceeds without a cap permit"
                );
                None
            }
        };
        self.in_flight.fetch_add(1, Ordering::AcqRel);
        ReconnectPermit {
            _permit: permit,
            in_flight: Arc::clone(&self.in_flight),
        }
    }

    /// Try to acquire a slot without waiting. Returns `None` if the cap is saturated.
    pub fn try_acquire(&self) -> Option<ReconnectPermit> {
        match Arc::clone(&self.semaphore).try_acquire_owned() {
            Ok(permit) => {
                self.in_flight.fetch_add(1, Ordering::AcqRel);
                Some(ReconnectPermit {
                    _permit: Some(permit),
                    in_flight: Arc::clone(&self.in_flight),
                })
            }
            Err(_) => {
                self.total_throttled.fetch_add(1, Ordering::Relaxed);
                None
            }
        }
    }

    /// Acquire a slot but give up after `timeout` if none becomes free.
    pub async fn acquire_timeout(&self, timeout: Duration) -> Option<ReconnectPermit> {
        tokio::time::timeout(timeout, self.acquire()).await.ok()
    }
}

impl Default for ReconnectGovernor {
    /// A sensible default cap for a single gateway instance.
    fn default() -> Self {
        Self::new(16)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    #[tokio::test]
    async fn acquire_and_release_tracks_in_flight() {
        let gov = ReconnectGovernor::new(4);
        assert_eq!(gov.in_flight(), 0);
        assert_eq!(gov.available_permits(), 4);

        let p1 = gov.acquire().await;
        assert_eq!(gov.in_flight(), 1);
        assert_eq!(gov.available_permits(), 3);

        let p2 = gov.acquire().await;
        assert_eq!(gov.in_flight(), 2);

        drop(p1);
        assert_eq!(gov.in_flight(), 1);
        assert_eq!(gov.available_permits(), 3);

        drop(p2);
        assert_eq!(gov.in_flight(), 0);
        assert_eq!(gov.available_permits(), 4);
    }

    #[tokio::test]
    async fn try_acquire_returns_none_when_saturated() {
        let gov = ReconnectGovernor::new(2);
        let _a = gov.try_acquire().expect("first");
        let _b = gov.try_acquire().expect("second");
        assert!(gov.try_acquire().is_none(), "cap reached -> None");
        assert_eq!(gov.in_flight(), 2);
        assert!(gov.total_throttled() >= 1);
    }

    #[tokio::test]
    async fn cap_of_zero_is_clamped_to_one() {
        let gov = ReconnectGovernor::new(0);
        assert_eq!(gov.max_concurrent(), 1);
        let _p = gov.acquire().await; // would deadlock if cap were truly 0
        assert_eq!(gov.in_flight(), 1);
    }

    #[tokio::test]
    async fn never_exceeds_cap_under_storm() {
        // THE STORM-CONTROL INVARIANT: with N tasks all reconnecting at once,
        // in_flight must never exceed the cap.
        const CAP: usize = 8;
        const TASKS: usize = 500;

        let gov = ReconnectGovernor::new(CAP);
        let max_seen = Arc::new(AtomicU64::new(0));
        let mut handles = Vec::with_capacity(TASKS);

        for _ in 0..TASKS {
            let gov = gov.clone();
            let max_seen = Arc::clone(&max_seen);
            handles.push(tokio::spawn(async move {
                let _permit = gov.acquire().await;
                // Observe the peak concurrency while holding the permit.
                let now = gov.in_flight();
                max_seen.fetch_max(now, Ordering::AcqRel);
                // Hold the slot briefly to force contention.
                tokio::time::sleep(Duration::from_millis(2)).await;
            }));
        }

        for h in handles {
            h.await.unwrap();
        }

        let peak = max_seen.load(Ordering::Acquire);
        assert!(
            peak <= CAP as u64,
            "peak in-flight {peak} exceeded cap {CAP} — thundering herd not contained"
        );
        assert!(peak > 1, "expected real contention, only saw peak {peak}");
        assert_eq!(gov.in_flight(), 0, "all permits returned at end");
        // With 500 tasks and a cap of 8, plenty had to wait.
        assert!(gov.total_throttled() > 0, "expected throttling under storm");
    }

    #[tokio::test]
    async fn acquire_timeout_gives_up_when_saturated() {
        let gov = ReconnectGovernor::new(1);
        let _held = gov.acquire().await;
        let denied = gov.acquire_timeout(Duration::from_millis(20)).await;
        assert!(
            denied.is_none(),
            "should time out while the only slot is held"
        );
    }

    #[tokio::test]
    async fn waiter_proceeds_after_release() {
        let gov = ReconnectGovernor::new(1);
        let held = gov.acquire().await;
        assert_eq!(gov.available_permits(), 0);

        let gov2 = gov.clone();
        let waiter = tokio::spawn(async move {
            let _p = gov2.acquire().await; // blocks until `held` is dropped
            gov2.in_flight()
        });

        // Give the waiter a moment to block, then release.
        tokio::time::sleep(Duration::from_millis(10)).await;
        drop(held);

        let seen = tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("waiter did not finish")
            .unwrap();
        assert_eq!(seen, 1);
    }
}
