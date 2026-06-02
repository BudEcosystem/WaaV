//! Storm-control chaos tests (W-D2).
//!
//! An upstream outage makes every affected session try to reconnect at once. Without a
//! global cap that self-inflicts a thundering herd. These tests prove the
//! [`ReconnectGovernor`] bounds the *instantaneous* number of in-flight reconnects
//! process-wide, no matter how many sessions storm in simultaneously, and that the shared
//! governor enforces the same cap when driven through the full [`ReconnectableStream`]
//! supervisor.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use waav_gateway::core::resilience::ReconnectGovernor;
use waav_gateway::core::websocket::reconnectable_stream::{
    ReconnectOutcome, ReconnectableStream, ReconnectableStreamConfig, RestoreError, StreamError,
    SupervisorExit, WsTransport,
};
use waav_gateway::core::websocket::ReconnectionConfig;
use waav_gateway::core::resilience::CircuitBreaker;

/// A trivial transport that just completes immediately after a short hold, used to create
/// contention on the governor.
struct StormTransport {
    hold: Duration,
    peak: Arc<AtomicUsize>,
    governor: ReconnectGovernor,
    cap: usize,
}

#[async_trait]
impl WsTransport for StormTransport {
    async fn restore_session(&mut self) -> Result<(), RestoreError> {
        Ok(())
    }

    async fn run(&mut self) -> ReconnectOutcome {
        // The supervisor already released the governed slot before calling run(); this
        // transport just completes.
        let _ = (&self.peak, &self.governor, self.cap);
        tokio::time::sleep(self.hold).await;
        ReconnectOutcome::Completed
    }
}

fn no_jitter() -> ReconnectionConfig {
    ReconnectionConfig {
        jitter: false,
        initial_delay_ms: 1,
        max_delay_ms: 2,
        ..ReconnectionConfig::aggressive()
    }
}

/// THE primary storm-control assertion: 1000 simultaneous reconnect acquisitions never
/// exceed the configured cap.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn thousand_reconnects_stay_under_cap() {
    const CAP: usize = 12;
    const STORM: usize = 1000;

    let governor = ReconnectGovernor::new(CAP);
    let peak = Arc::new(AtomicUsize::new(0));

    let mut handles = Vec::with_capacity(STORM);
    for _ in 0..STORM {
        let governor = governor.clone();
        let peak = Arc::clone(&peak);
        handles.push(tokio::spawn(async move {
            // Acquire a governed reconnect slot, exactly as the supervisor does before it
            // dials. While the slot is held, the in-flight count must never exceed the cap.
            let _permit = governor.acquire().await;
            let inflight = governor.in_flight() as usize;
            peak.fetch_max(inflight, Ordering::AcqRel);
            assert!(
                inflight <= CAP,
                "in-flight {inflight} exceeded cap {CAP} — thundering herd not contained"
            );
            // Hold the slot briefly to force real overlap across the storm.
            tokio::time::sleep(Duration::from_millis(1)).await;
        }));
    }

    for h in handles {
        h.await.expect("storm task panicked");
    }

    let observed_peak = peak.load(Ordering::Acquire);
    assert!(
        observed_peak <= CAP,
        "observed peak {observed_peak} exceeded cap {CAP}"
    );
    assert!(
        observed_peak > 1,
        "expected genuine contention in a 1000-way storm, saw peak {observed_peak}"
    );
    assert_eq!(governor.in_flight(), 0, "every permit returned after the storm");
    assert!(
        governor.total_throttled() > 0,
        "with 1000 reconnects and a cap of {CAP}, many must have been throttled"
    );
}

/// The same invariant, but driven through the real supervisor with a *shared* governor and
/// breaker (the production wiring). Many supervisors reconnecting at once stay under the cap.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn supervisors_sharing_governor_stay_under_cap() {
    const CAP: usize = 6;
    const SESSIONS: usize = 200;

    let governor = ReconnectGovernor::new(CAP);
    let breaker = Arc::new(CircuitBreaker::with_defaults());
    let peak = Arc::new(AtomicUsize::new(0));

    let mut handles = Vec::with_capacity(SESSIONS);
    for _ in 0..SESSIONS {
        let governor = governor.clone();
        let breaker = Arc::clone(&breaker);
        let peak = Arc::clone(&peak);
        handles.push(tokio::spawn(async move {
            let cfg = ReconnectableStreamConfig::new("storm", no_jitter());
            let stream = ReconnectableStream::with_breaker_and_governor(
                cfg,
                breaker,
                governor.clone(),
            );
            let g = governor.clone();
            let p = Arc::clone(&peak);
            stream
                .run(move || {
                    // The connect closure runs while the supervisor holds a governed slot.
                    let g = g.clone();
                    let p = Arc::clone(&p);
                    async move {
                        let inflight = g.in_flight() as usize;
                        p.fetch_max(inflight, Ordering::AcqRel);
                        assert!(inflight <= CAP, "in-flight {inflight} > cap {CAP}");
                        tokio::time::sleep(Duration::from_millis(1)).await;
                        Ok::<_, StreamError>(StormTransport {
                            hold: Duration::from_millis(1),
                            peak: Arc::clone(&p),
                            governor: g.clone(),
                            cap: CAP,
                        })
                    }
                })
                .await
        }));
    }

    for h in handles {
        let exit = h.await.expect("session panicked");
        assert_eq!(exit, SupervisorExit::Completed);
    }

    assert!(peak.load(Ordering::Acquire) <= CAP);
    assert_eq!(governor.in_flight(), 0);
}
