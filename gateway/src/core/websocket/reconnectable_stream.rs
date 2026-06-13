//! Generic reconnect supervisor for WebSocket-based streaming providers (W-D1).
//!
//! Today the streaming STT/TTS providers (Deepgram, AssemblyAI, …) run an inner event
//! loop that `break`s on the first transport error — a single TCP blip ends the session
//! and every in-flight final after that point is lost. [`ReconnectableStream`] turns that
//! bare loop into a supervised one:
//!
//! 1. **Connect** via a caller-supplied [`WsTransport`] factory.
//! 2. **Restore the *featured* session** via a `restore_session` step (re-send the
//!    Deepgram diarize/keyterm query, Cartesia version, Azure `speech.config`, Hume
//!    `config_id`, …). This is why the standardized-config keystone (W-A0/W-A1) must
//!    precede this: reconnection must restore the *featured* session, not a bare one.
//! 3. **Run** the inner loop until it yields a [`ReconnectOutcome`].
//! 4. On an *eligible* error, consult the per-provider [`CircuitBreaker`] and the global
//!    [`ReconnectGovernor`], wait out the [`ReconnectionManager`] backoff (jitter), and
//!    loop. On an **intentional** disconnect, stop cleanly — no reconnect.
//!
//! The attempt counter resets on the first successful (re)connect, so a long-lived,
//! occasionally-flaky session never exhausts `max_attempts` over its lifetime.
//!
//! The supervisor is transport-agnostic on purpose: the real providers plug their
//! `tokio_tungstenite` socket behind [`WsTransport`], and the chaos tests plug an
//! in-memory mock that drops mid-stream — both exercise identical supervisor logic.

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;

use crate::core::resilience::{CircuitBreaker, CircuitState, ReconnectGovernor};
use crate::core::websocket::{ReconnectionConfig, ReconnectionManager};

/// Why the inner run loop returned.
///
/// The supervisor decides whether to reconnect based on this outcome.
#[derive(Debug)]
pub enum ReconnectOutcome {
    /// The stream finished on purpose (client closed, `CloseStream` ack, EOS). Do NOT
    /// reconnect.
    Completed,
    /// A transport-level failure that is eligible for reconnection (socket reset, idle
    /// timeout, network error). The supervisor will reconnect (subject to breaker +
    /// governor + backoff).
    Reconnectable(StreamError),
    /// A fatal, non-retryable error (auth rejected, malformed config). Do NOT reconnect —
    /// retrying would just fail identically.
    Fatal(StreamError),
}

/// A transport-level streaming error surfaced to the supervisor.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct StreamError(pub String);

impl StreamError {
    pub fn new(msg: impl Into<String>) -> Self {
        StreamError(msg.into())
    }
}

/// An error establishing the featured session after a (re)connect.
#[derive(Debug, Clone, thiserror::Error)]
#[error("session restore failed: {0}")]
pub struct RestoreError(pub String);

impl RestoreError {
    pub fn new(msg: impl Into<String>) -> Self {
        RestoreError(msg.into())
    }
}

/// A live transport produced by the supervisor's `connect` step.
///
/// Implementors own the actual socket. [`restore_session`](WsTransport::restore_session)
/// re-sends the full-feature handshake on the fresh connection; [`run`](WsTransport::run)
/// drives the inner event loop until it yields a [`ReconnectOutcome`].
#[async_trait]
pub trait WsTransport: Send {
    /// Re-send the full-feature handshake on this (fresh) connection so the restored
    /// session is identical to the original — same diarization, keyterms, codec, etc.
    ///
    /// For providers that put their features in the connect URL (Deepgram), this can be a
    /// no-op because the URL already carried them; for providers that send a config
    /// message after connect (Cartesia/Azure/Hume), this re-sends that message.
    async fn restore_session(&mut self) -> Result<(), RestoreError>;

    /// Run the inner event loop until it yields a [`ReconnectOutcome`].
    async fn run(&mut self) -> ReconnectOutcome;
}

/// Tuning for the supervisor itself (separate from the per-attempt backoff).
#[derive(Debug, Clone)]
pub struct ReconnectableStreamConfig {
    /// Backoff/jitter/preset config for the [`ReconnectionManager`].
    pub reconnection: ReconnectionConfig,
    /// A human label used in logs/metrics (the provider name).
    pub provider: String,
}

impl ReconnectableStreamConfig {
    pub fn new(provider: impl Into<String>, reconnection: ReconnectionConfig) -> Self {
        Self {
            reconnection,
            provider: provider.into(),
        }
    }
}

/// How a supervised run ended (the supervisor's own terminal status).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupervisorExit {
    /// D-G2: the breaker went credentials-fatal (3 consecutive sub-stable
    /// connections) — retrying cannot help; the session should surface a
    /// configuration error.
    CredentialsFatal,
    /// Inner loop completed normally (or an intentional disconnect was requested).
    Completed,
    /// A fatal, non-retryable error ended the session.
    Fatal,
    /// Reconnection attempts were exhausted (backoff `max_attempts`).
    Exhausted,
    /// The per-provider circuit breaker is open and stayed open past the deadline.
    CircuitOpen,
}

/// The generic reconnect supervisor.
///
/// `T` is the live transport type produced by the `connect` factory. The supervisor holds
/// the [`ReconnectionManager`] (backoff), an `intentional_disconnect` flag (so a client
/// close is never mistaken for a failure), the per-provider [`CircuitBreaker`], and the
/// shared [`ReconnectGovernor`].
pub struct ReconnectableStream {
    config: ReconnectableStreamConfig,
    manager: ReconnectionManager,
    breaker: Arc<CircuitBreaker>,
    governor: ReconnectGovernor,
    /// Set by [`request_disconnect`](Self::disconnect_handle); when true the supervisor
    /// stops cleanly without reconnecting.
    intentional_disconnect: Arc<AtomicBool>,
    /// Cumulative count of successful (re)connects (dial + restore) over this supervisor's
    /// lifetime — the first connect plus every recovery. Drives `waav_reconnects_total`.
    successful_connects: std::sync::atomic::AtomicU64,
}

/// A cheap handle used to ask a running supervisor to stop reconnecting.
#[derive(Clone)]
pub struct DisconnectHandle {
    flag: Arc<AtomicBool>,
}

impl DisconnectHandle {
    /// Signal the supervisor that the next/current disconnect is intentional.
    pub fn request_disconnect(&self) {
        self.flag.store(true, Ordering::Release);
    }

    pub fn is_disconnect_requested(&self) -> bool {
        self.flag.load(Ordering::Acquire)
    }
}

impl ReconnectableStream {
    /// Create a supervisor with its own (default) breaker and governor.
    pub fn new(config: ReconnectableStreamConfig) -> Self {
        Self::with_breaker_and_governor(
            config,
            Arc::new(CircuitBreaker::with_defaults()),
            ReconnectGovernor::default(),
        )
    }

    /// Create a supervisor that shares a process-wide breaker and governor (the production
    /// wiring: one breaker per provider, one governor per gateway).
    pub fn with_breaker_and_governor(
        config: ReconnectableStreamConfig,
        breaker: Arc<CircuitBreaker>,
        governor: ReconnectGovernor,
    ) -> Self {
        let manager = ReconnectionManager::new(config.reconnection.clone());
        Self {
            config,
            manager,
            breaker,
            governor,
            intentional_disconnect: Arc::new(AtomicBool::new(false)),
            successful_connects: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Share an externally-owned intentional-disconnect flag with this supervisor.
    ///
    /// The production wiring: a streaming client owns an `Arc<AtomicBool>`, clears it when it
    /// (re)connects, and sets it in `disconnect()` *before* firing its transport-shutdown signal.
    /// Handing that same flag to the supervisor here closes the disconnect-vs-server-close race:
    /// if the transport's `run()` happens to report `Reconnectable` (the stream-ended select arm
    /// won over the shutdown arm), the supervisor still observes the flag — either immediately at
    /// the post-`run()` guard, or (if `disconnect()` lands a beat later) at the loop-top check
    /// before the next dial — and completes cleanly instead of doing one spurious reconnect.
    pub fn with_disconnect_flag(mut self, flag: Arc<AtomicBool>) -> Self {
        self.intentional_disconnect = flag;
        self
    }

    /// A handle to request an intentional disconnect from another task.
    pub fn disconnect_handle(&self) -> DisconnectHandle {
        DisconnectHandle {
            flag: Arc::clone(&self.intentional_disconnect),
        }
    }

    /// Mark the next disconnect as intentional (e.g. the client closed the WS).
    pub fn request_disconnect(&self) {
        self.intentional_disconnect.store(true, Ordering::Release);
    }

    /// Cumulative successful (re)connects over this supervisor's lifetime: the first connect
    /// plus every recovery after a drop.
    pub fn total_reconnections(&self) -> u64 {
        self.successful_connects.load(Ordering::Acquire)
    }

    /// The shared circuit breaker (for metrics/readiness).
    pub fn breaker(&self) -> &Arc<CircuitBreaker> {
        &self.breaker
    }

    /// Publish the breaker's current state on the `waav_circuit_breaker_state{provider}` gauge
    /// (W-C1). Called after every breaker outcome so the exposition tracks open/half-open/closed
    /// transitions in near-real-time.
    fn publish_breaker_state(&self) {
        crate::core::metrics::bridge::set_circuit_breaker_state(
            &self.config.provider,
            self.breaker.state().as_code(),
        );
    }

    /// Run the supervised stream to completion.
    ///
    /// `connect` is the transport factory (dials + handshakes). `sleep` is injectable so
    /// tests can drive the backoff deterministically without real wall-clock waits; pass
    /// [`tokio::time::sleep`] in production via [`Self::run`].
    pub async fn run_with_sleep<T, C, CFut, S, SFut>(
        &self,
        mut connect: C,
        mut sleep: S,
    ) -> SupervisorExit
    where
        T: WsTransport,
        C: FnMut() -> CFut,
        CFut: Future<Output = Result<T, StreamError>>,
        S: FnMut(Duration) -> SFut,
        SFut: Future<Output = ()>,
    {
        loop {
            // An intentional disconnect short-circuits everything.
            if self.intentional_disconnect.load(Ordering::Acquire) {
                return SupervisorExit::Completed;
            }

            // Honor the circuit breaker before spending a governed slot or dialing.
            if !self.breaker.allow_request() {
                // D-G2: credentials-fatal is STICKY — no cooldown will ever
                // open it; exit immediately instead of burning the budget.
                if self.breaker.is_permanently_failed() {
                    tracing::error!(
                        provider = %self.config.provider,
                        "breaker is credentials-fatal; stopping reconnection permanently"
                    );
                    return SupervisorExit::CredentialsFatal;
                }
                // Breaker is open (or its single half-open probe is already outstanding).
                // We must still have reconnection budget left, otherwise give up. Wait out
                // the breaker cooldown (approximated by the current backoff delay) and
                // re-check — we do NOT dial while the breaker is open, which is the whole
                // point of the breaker for storm control.
                if !self.manager.should_retry() {
                    return SupervisorExit::CircuitOpen;
                }
                debug_assert!(matches!(
                    self.breaker.state(),
                    CircuitState::Open | CircuitState::HalfOpen
                ));
                let delay = self.manager.next_delay_duration();
                sleep(delay).await;
                continue;
            }

            // Storm control: acquire a governed reconnect slot. Held across the dial +
            // restore so the instantaneous burst can never exceed the cap.
            let _permit = self.governor.acquire().await;

            self.manager.start_attempt();

            // --- Connect ---------------------------------------------------------------
            let mut transport = match connect().await {
                Ok(t) => t,
                Err(e) => {
                    tracing::warn!(
                        provider = %self.config.provider,
                        error = %e,
                        "reconnect dial failed"
                    );
                    self.breaker.record_failure();
                    self.publish_breaker_state();
                    self.manager.record_attempt(false);
                    drop(_permit);
                    if !self.manager.should_retry() {
                        return SupervisorExit::Exhausted;
                    }
                    let delay = self.manager.next_delay_duration();
                    sleep(delay).await;
                    continue;
                }
            };

            // --- Restore the featured session ------------------------------------------
            if let Err(e) = transport.restore_session().await {
                tracing::warn!(
                    provider = %self.config.provider,
                    error = %e,
                    "session restore failed after reconnect; treating as reconnectable"
                );
                self.breaker.record_failure();
                self.publish_breaker_state();
                self.manager.record_attempt(false);
                drop(_permit);
                if !self.manager.should_retry() {
                    return SupervisorExit::Exhausted;
                }
                let delay = self.manager.next_delay_duration();
                sleep(delay).await;
                continue;
            }

            // (Re)connect + restore succeeded — the dial succeeded as a TCP/WS event, so tell
            // the breaker. We do NOT reset the manager's attempt counter yet: a connect that
            // immediately drops must still count against `max_attempts` (otherwise a fast
            // connect→drop flap would retry forever). The counter is reset only after the
            // connection proves *stable* (lasts `reset_after_ms`), which `on_connection_lost`
            // enforces below.
            self.breaker.record_success();
            self.publish_breaker_state();
            self.successful_connects.fetch_add(1, Ordering::AcqRel);
            // The dial+restore window is over; release the governed slot so other sessions
            // can reconnect while this one streams.
            drop(_permit);
            let connected_since = std::time::Instant::now();

            // --- Run the inner loop ----------------------------------------------------
            let outcome = transport.run().await;
            // A connection that survived at least `reset_after_ms` is "stable": clear the
            // backoff so a long-lived, occasionally-flaky session never exhausts its budget
            // over its lifetime.
            let stable_for = connected_since.elapsed();
            let was_stable = self.config.reconnection.reset_after_ms > 0
                && stable_for.as_millis() as u64 >= self.config.reconnection.reset_after_ms;
            if was_stable {
                self.manager.reset();
            }
            // D-G2: feed the connection lifetime to the breaker's quick-fail
            // detector — 3 consecutive handshake-then-die SERVER closes (bad
            // credentials' signature) enter the (recoverable) FATAL state
            // instead of backoff-looping forever. A CLEAN close — a `Completed`
            // outcome or a client-intended disconnect — is NOT a quick-failure
            // signal (review wc71hewlx #0: counting normal short hangups took
            // healthy providers offline gateway-wide).
            let intentional = matches!(outcome, ReconnectOutcome::Completed)
                || self.intentional_disconnect.load(Ordering::Acquire);
            self.breaker.record_connection_closed(stable_for, intentional);
            match outcome {
                ReconnectOutcome::Completed => {
                    return SupervisorExit::Completed;
                }
                ReconnectOutcome::Fatal(e) => {
                    tracing::error!(
                        provider = %self.config.provider,
                        error = %e,
                        "fatal stream error; not reconnecting"
                    );
                    self.breaker.record_failure();
                    self.publish_breaker_state();
                    return SupervisorExit::Fatal;
                }
                ReconnectOutcome::Reconnectable(e) => {
                    // If the client asked us to stop while the loop was running, honor it.
                    if self.intentional_disconnect.load(Ordering::Acquire) {
                        return SupervisorExit::Completed;
                    }
                    tracing::warn!(
                        provider = %self.config.provider,
                        error = %e,
                        "reconnectable stream error; will reconnect"
                    );
                    self.breaker.record_failure();
                    self.publish_breaker_state();
                    if !self.manager.should_retry() {
                        return SupervisorExit::Exhausted;
                    }
                    let delay = self.manager.next_delay_duration();
                    sleep(delay).await;
                    // loop -> reconnect (start_attempt at the top increments the counter)
                }
            }
        }
    }

    /// Run the supervised stream to completion using the real tokio clock.
    pub async fn run<T, C, CFut>(&self, connect: C) -> SupervisorExit
    where
        T: WsTransport,
        C: FnMut() -> CFut,
        CFut: Future<Output = Result<T, StreamError>>,
    {
        self.run_with_sleep(connect, |d| tokio::time::sleep(d)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use tokio::sync::Mutex;

    /// A mock transport whose behaviour is scripted per (re)connect attempt.
    struct MockTransport {
        restore_calls: Arc<AtomicUsize>,
        outcome: ReconnectOutcome,
        restore_result: Result<(), RestoreError>,
    }

    #[async_trait]
    impl WsTransport for MockTransport {
        async fn restore_session(&mut self) -> Result<(), RestoreError> {
            self.restore_calls.fetch_add(1, Ordering::AcqRel);
            self.restore_result.clone()
        }

        async fn run(&mut self) -> ReconnectOutcome {
            // Take ownership of the scripted outcome (run is called once per connect).
            std::mem::replace(&mut self.outcome, ReconnectOutcome::Completed)
        }
    }

    fn no_jitter_aggressive() -> ReconnectionConfig {
        ReconnectionConfig {
            jitter: false,
            initial_delay_ms: 1,
            max_delay_ms: 4,
            ..ReconnectionConfig::aggressive()
        }
    }

    #[tokio::test]
    async fn completes_without_reconnect_on_clean_finish() {
        let cfg = ReconnectableStreamConfig::new("mock", no_jitter_aggressive());
        let stream = ReconnectableStream::new(cfg);
        let restore_calls = Arc::new(AtomicUsize::new(0));

        let rc = Arc::clone(&restore_calls);
        let exit = stream
            .run_with_sleep(
                move || {
                    let rc = Arc::clone(&rc);
                    async move {
                        Ok(MockTransport {
                            restore_calls: rc,
                            outcome: ReconnectOutcome::Completed,
                            restore_result: Ok(()),
                        })
                    }
                },
                |_d| async {},
            )
            .await;

        assert_eq!(exit, SupervisorExit::Completed);
        assert_eq!(restore_calls.load(Ordering::Acquire), 1, "restore runs once on connect");
        assert_eq!(stream.total_reconnections(), 1);
    }

    #[tokio::test]
    async fn reconnects_after_midstream_drop_then_completes() {
        // First run() drops (Reconnectable); second run() completes. Expect exactly one
        // reconnect and restore_session called twice (once per connect).
        let cfg = ReconnectableStreamConfig::new("mock", no_jitter_aggressive());
        let stream = ReconnectableStream::new(cfg);
        let restore_calls = Arc::new(AtomicUsize::new(0));
        let attempt = Arc::new(AtomicUsize::new(0));

        let rc = Arc::clone(&restore_calls);
        let at = Arc::clone(&attempt);
        let exit = stream
            .run_with_sleep(
                move || {
                    let rc = Arc::clone(&rc);
                    let at = Arc::clone(&at);
                    async move {
                        let n = at.fetch_add(1, Ordering::AcqRel);
                        let outcome = if n == 0 {
                            ReconnectOutcome::Reconnectable(StreamError::new("socket reset"))
                        } else {
                            ReconnectOutcome::Completed
                        };
                        Ok(MockTransport {
                            restore_calls: rc,
                            outcome,
                            restore_result: Ok(()),
                        })
                    }
                },
                |_d| async {},
            )
            .await;

        assert_eq!(exit, SupervisorExit::Completed);
        assert_eq!(attempt.load(Ordering::Acquire), 2, "connected twice");
        assert_eq!(
            restore_calls.load(Ordering::Acquire),
            2,
            "restore_session re-sent the featured handshake on reconnect"
        );
    }

    #[tokio::test]
    async fn intentional_disconnect_stops_without_reconnect() {
        let cfg = ReconnectableStreamConfig::new("mock", no_jitter_aggressive());
        let stream = ReconnectableStream::new(cfg);
        let handle = stream.disconnect_handle();
        let restore_calls = Arc::new(AtomicUsize::new(0));

        // The transport will report Reconnectable, but we set the intentional flag so the
        // supervisor must NOT reconnect.
        let rc = Arc::clone(&restore_calls);
        let exit = stream
            .run_with_sleep(
                move || {
                    let rc = Arc::clone(&rc);
                    let handle = handle.clone();
                    async move {
                        // Simulate the client closing right as the loop is about to fail.
                        handle.request_disconnect();
                        Ok(MockTransport {
                            restore_calls: rc,
                            outcome: ReconnectOutcome::Reconnectable(StreamError::new("drop")),
                            restore_result: Ok(()),
                        })
                    }
                },
                |_d| async {},
            )
            .await;

        assert_eq!(exit, SupervisorExit::Completed, "intentional close must not reconnect");
        assert_eq!(restore_calls.load(Ordering::Acquire), 1, "connected exactly once");
    }

    #[tokio::test]
    async fn shared_flag_set_after_run_blocks_the_next_dial() {
        // The disconnect-vs-server-close race (case 2): the transport's run() reports
        // Reconnectable BEFORE disconnect() lands, so the post-run() guard does NOT yet see the
        // flag. disconnect() then sets the *shared* flag during the backoff sleep. The loop-top
        // check at the next iteration must observe it and complete WITHOUT a spurious reconnect.
        let flag = Arc::new(AtomicBool::new(false));
        let cfg = ReconnectableStreamConfig::new("mock", no_jitter_aggressive());
        let stream = ReconnectableStream::new(cfg).with_disconnect_flag(Arc::clone(&flag));
        let attempt = Arc::new(AtomicUsize::new(0));

        let at = Arc::clone(&attempt);
        let exit = stream
            .run_with_sleep(
                move || {
                    let at = Arc::clone(&at);
                    async move {
                        at.fetch_add(1, Ordering::AcqRel);
                        Ok(MockTransport {
                            restore_calls: Arc::new(AtomicUsize::new(0)),
                            // Stream "ended" — eligible for reconnect — but the flag is not set yet.
                            outcome: ReconnectOutcome::Reconnectable(StreamError::new("server close")),
                            restore_result: Ok(()),
                        })
                    }
                },
                // The backoff sleep is where disconnect() lands: set the shared flag here so the
                // loop-top check sees it before dialing again.
                {
                    let flag = Arc::clone(&flag);
                    move |_d| {
                        let flag = Arc::clone(&flag);
                        async move {
                            flag.store(true, Ordering::Release);
                        }
                    }
                },
            )
            .await;

        assert_eq!(exit, SupervisorExit::Completed, "shared flag must stop the next dial");
        assert_eq!(
            attempt.load(Ordering::Acquire),
            1,
            "connected exactly once — no spurious reconnect after the intentional close",
        );
    }

    #[tokio::test]
    async fn fatal_error_does_not_reconnect() {
        let cfg = ReconnectableStreamConfig::new("mock", no_jitter_aggressive());
        let stream = ReconnectableStream::new(cfg);
        let attempt = Arc::new(AtomicUsize::new(0));

        let at = Arc::clone(&attempt);
        let exit = stream
            .run_with_sleep(
                move || {
                    let at = Arc::clone(&at);
                    async move {
                        at.fetch_add(1, Ordering::AcqRel);
                        Ok(MockTransport {
                            restore_calls: Arc::new(AtomicUsize::new(0)),
                            outcome: ReconnectOutcome::Fatal(StreamError::new("401 auth rejected")),
                            restore_result: Ok(()),
                        })
                    }
                },
                |_d| async {},
            )
            .await;

        assert_eq!(exit, SupervisorExit::Fatal);
        assert_eq!(attempt.load(Ordering::Acquire), 1, "fatal -> connected once, no retry");
    }

    #[tokio::test]
    async fn exhausts_after_max_attempts_of_persistent_failure() {
        // Every run() drops; with bounded max_attempts the supervisor must give up.
        let recon = ReconnectionConfig {
            enabled: true,
            max_attempts: 3,
            initial_delay_ms: 1,
            max_delay_ms: 2,
            backoff_multiplier: 1.5,
            jitter: false,
            reset_after_ms: 0,
        };
        let cfg = ReconnectableStreamConfig::new("mock", recon);
        let stream = ReconnectableStream::new(cfg);
        let attempt = Arc::new(AtomicUsize::new(0));

        let at = Arc::clone(&attempt);
        let exit = stream
            .run_with_sleep(
                move || {
                    let at = Arc::clone(&at);
                    async move {
                        at.fetch_add(1, Ordering::AcqRel);
                        Ok(MockTransport {
                            restore_calls: Arc::new(AtomicUsize::new(0)),
                            outcome: ReconnectOutcome::Reconnectable(StreamError::new("flap")),
                            restore_result: Ok(()),
                        })
                    }
                },
                |_d| async {},
            )
            .await;

        assert_eq!(exit, SupervisorExit::Exhausted);
        // First connect + the reconnect attempts (bounded by max_attempts).
        let n = attempt.load(Ordering::Acquire);
        assert!((2..=5).contains(&n), "bounded retries, saw {n}");
    }

    #[tokio::test]
    async fn restore_failure_is_treated_as_reconnectable() {
        // Connect succeeds but restore fails twice, then succeeds and completes.
        let cfg = ReconnectableStreamConfig::new("mock", no_jitter_aggressive());
        let stream = ReconnectableStream::new(cfg);
        let attempt = Arc::new(AtomicUsize::new(0));

        let at = Arc::clone(&attempt);
        let exit = stream
            .run_with_sleep(
                move || {
                    let at = Arc::clone(&at);
                    async move {
                        let n = at.fetch_add(1, Ordering::AcqRel);
                        let restore_result = if n < 2 {
                            Err(RestoreError::new("handshake rejected"))
                        } else {
                            Ok(())
                        };
                        Ok(MockTransport {
                            restore_calls: Arc::new(AtomicUsize::new(0)),
                            outcome: ReconnectOutcome::Completed,
                            restore_result,
                        })
                    }
                },
                |_d| async {},
            )
            .await;

        assert_eq!(exit, SupervisorExit::Completed);
        assert_eq!(attempt.load(Ordering::Acquire), 3, "two restore failures then success");
    }

    #[tokio::test]
    async fn shares_governor_so_concurrent_supervisors_stay_under_cap() {
        // Many supervisors reconnecting at once must never exceed the shared governor cap
        // during their dial+restore window.
        const CAP: usize = 4;
        let governor = ReconnectGovernor::new(CAP);
        let breaker = Arc::new(CircuitBreaker::with_defaults());
        let peak = Arc::new(AtomicUsize::new(0));

        let mut handles = vec![];
        for _ in 0..40 {
            let governor = governor.clone();
            let breaker = Arc::clone(&breaker);
            let peak = Arc::clone(&peak);
            handles.push(tokio::spawn(async move {
                let cfg = ReconnectableStreamConfig::new("mock", no_jitter_aggressive());
                let stream = ReconnectableStream::with_breaker_and_governor(
                    cfg,
                    breaker,
                    governor.clone(),
                );
                let observed = Arc::new(Mutex::new(()));
                let _ = observed;
                stream
                    .run_with_sleep(
                        move || {
                            let governor = governor.clone();
                            let peak = Arc::clone(&peak);
                            async move {
                                // While we (conceptually) hold a slot, record the peak.
                                let inflight = governor.in_flight() as usize;
                                peak.fetch_max(inflight, Ordering::AcqRel);
                                assert!(
                                    inflight <= CAP,
                                    "in-flight {inflight} exceeded cap {CAP}"
                                );
                                tokio::time::sleep(Duration::from_millis(1)).await;
                                Ok(MockTransport {
                                    restore_calls: Arc::new(AtomicUsize::new(0)),
                                    outcome: ReconnectOutcome::Completed,
                                    restore_result: Ok(()),
                                })
                            }
                        },
                        |_d| async {},
                    )
                    .await
            }));
        }
        for h in handles {
            assert_eq!(h.await.unwrap(), SupervisorExit::Completed);
        }
        assert!(peak.load(Ordering::Acquire) <= CAP);
        assert_eq!(governor.in_flight(), 0);
    }
}
