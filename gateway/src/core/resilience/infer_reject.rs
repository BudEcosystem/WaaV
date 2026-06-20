//! GW-3 (breaker half) — classify a WaaV-Infer typed reject-reason against the
//! per-provider [`CircuitBreaker`].
//!
//! `INFER_GATEWAY_INTEGRATION.md` §5.2/§11.2/§12 (GW-3). The native WS v1 wire
//! surfaces lifecycle/admission errors as `Error(InferError{ code, retry_after_ms })`.
//! Three of those codes are **typed reject-reasons** the Infer router emits when the
//! *whole tier* is saturated, warming, or draining — they are NOT a sign that *this*
//! connection is broken:
//!
//! - `admission_rejected` — the router's fair-scheduler shed this request (capacity).
//! - `model_not_ready`    — a cold/warming worker; a Warming restart, not a fault.
//! - `draining`           — the worker is gracefully shutting down for a rollout.
//!
//! The critical correctness property of GW-3's breaker half:
//!
//! > A storm of `admission_rejected` / `model_not_ready` / `draining` must **NOT** trip
//! > the per-provider breaker. If it did, a transient capacity dip or a rolling restart
//! > would *open* the `waav-infer` breaker and take the whole self-hosted tier offline —
//! > the exact failure GW-3 exists to prevent. These are **failover-eligible
//! > non-failures**: the gateway fails the *session* over to a cloud provider (or returns
//! > a clean 503 + `Retry-After`) while leaving the breaker closed so the tier is
//! > re-tried the instant it recovers. The half-open probe likewise treats
//! > `model_not_ready` as "remain open without penalty" so a Warming restart can't flap.
//!
//! Genuine transport/auth failures (a dropped UDS/WS, a real `ProviderError`) DO count as
//! failures and trip the breaker after the configured volume/rate — that is what keeps an
//! actually-down sidecar from being hammered.
//!
//! This module is the *classification + recording* seam only (the "breaker half" of GW-3,
//! @M1). The cross-tier *failover routing* it enables is GW-3's M5 half and lives in the
//! session/handler layer; here we expose the typed verdict it will consume.

use std::time::Duration;

use super::CircuitBreaker;

/// A typed reject-reason carried on a WaaV-Infer `Error(InferError{code, retry_after_ms})`
/// frame (native WS v1, §5.2). Parsed from the wire `code` string by [`InferRejectReason::parse`].
///
/// Only the three *capacity/lifecycle* codes are modelled as distinct variants because only
/// they get the non-failure breaker treatment; every other code (transport, auth, malformed,
/// internal) is a genuine failure and is represented by [`InferRejectReason::HardFailure`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InferRejectReason {
    /// `admission_rejected` — the Infer router shed this request under load (VTC fair-share /
    /// queue full). The tier is up; this request just didn't get a slot. Failover-eligible.
    AdmissionRejected,
    /// `model_not_ready` — a cold/warming worker for the requested model. A Warming restart,
    /// not a fault. Failover-eligible; the half-open probe stays open without penalty.
    ModelNotReady,
    /// `draining` — the worker is gracefully shutting down (rollout). Failover-eligible.
    Draining,
    /// Anything else — a genuine fault (transport drop, auth, malformed, internal). Counts as
    /// a breaker failure.
    HardFailure,
}

impl InferRejectReason {
    /// Parse the wire `code` string of an `InferError`. Unknown / empty codes are treated as a
    /// [`InferRejectReason::HardFailure`] (fail safe: an unrecognised error counts toward the
    /// breaker rather than being silently exempted).
    pub fn parse(code: &str) -> Self {
        match code.trim().to_ascii_lowercase().as_str() {
            "admission_rejected" | "admission-rejected" => Self::AdmissionRejected,
            "model_not_ready" | "model-not-ready" => Self::ModelNotReady,
            "draining" => Self::Draining,
            _ => Self::HardFailure,
        }
    }

    /// Whether this reason makes the *session* eligible for cross-tier failover (Infer → cloud).
    /// All three typed reject-reasons are; a hard failure is handled by the breaker/governor and
    /// the normal reconnect path, not by failing the whole session over.
    pub fn is_failover_eligible(&self) -> bool {
        !matches!(self, Self::HardFailure)
    }
}

/// What the breaker should do with an Infer outcome — the typed GW-3 verdict.
///
/// Modelled as an enum (not a bool) so the call site MUST handle every case in an exhaustive
/// `match`: adding a future verdict (e.g. a graded brown-out) is a compile error at every
/// recording site rather than a silently-defaulted branch. This is the "push correctness to the
/// type level" discipline — a reject-reason can never be *accidentally* counted as a failure,
/// because counting only happens in [`record_infer_outcome`] via this verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakerVerdict {
    /// Count as a breaker success (a clean response / a stable connection).
    Success,
    /// Count as a breaker failure (a genuine transport/auth/internal fault) — drives the rate
    /// toward a trip.
    Failure,
    /// A typed Infer reject-reason: **do not touch the breaker's success/failure window** (so a
    /// capacity/lifecycle storm can't open it), but mark the session failover-eligible. The
    /// half-open probe is left intact (a `model_not_ready` during a probe does not re-open the
    /// breaker — the tier is warming, not broken).
    NonFailureFailover(InferRejectReason),
}

impl BreakerVerdict {
    /// Classify a WaaV-Infer error `code` (from the native WS v1 `Error` frame) into a verdict.
    pub fn for_error_code(code: &str) -> Self {
        match InferRejectReason::parse(code) {
            InferRejectReason::HardFailure => Self::Failure,
            reason => Self::NonFailureFailover(reason),
        }
    }

    /// Whether this verdict makes the session eligible for cross-tier failover.
    pub fn is_failover_eligible(&self) -> bool {
        matches!(self, Self::NonFailureFailover(_))
    }
}

/// Apply a GW-3 verdict to a provider's shared [`CircuitBreaker`].
///
/// This is the ONE place an Infer outcome touches the breaker. A
/// [`BreakerVerdict::NonFailureFailover`] is intentionally a no-op on the breaker window —
/// that is the property that keeps the `waav-infer` breaker closed through an
/// `admission_rejected` / `model_not_ready` / `draining` storm.
pub fn record_infer_outcome(breaker: &CircuitBreaker, verdict: BreakerVerdict) {
    match verdict {
        BreakerVerdict::Success => breaker.record_success(),
        BreakerVerdict::Failure => breaker.record_failure(),
        // The whole point of GW-3's breaker half: a typed reject-reason does NOT move the
        // breaker's failure window. The tier is up (or warming); the request just didn't land.
        BreakerVerdict::NonFailureFailover(_) => {}
    }
}

/// Record the close of an Infer connection against the breaker's credentials-fatal detector
/// (D-G2), honouring GW-3: a sub-stable close whose cause is a *typed reject-reason* is NOT a
/// quick-failure signal (a warming/draining worker dropping us must not look like bad creds),
/// so it is reported as `intentional` (which `record_connection_closed` never counts).
///
/// `reason = None` ⇒ the close had no typed reject-reason (an ordinary transport drop) and is
/// classified normally.
pub fn record_infer_connection_closed(
    breaker: &CircuitBreaker,
    stable_for: Duration,
    intentional: bool,
    reason: Option<InferRejectReason>,
) {
    // A typed reject-reason close is a lifecycle event, not a credentials signature: never let it
    // feed the quick-failure streak that arms the FATAL state.
    let lifecycle = matches!(
        reason,
        Some(InferRejectReason::ModelNotReady)
            | Some(InferRejectReason::Draining)
            | Some(InferRejectReason::AdmissionRejected)
    );
    breaker.record_connection_closed(stable_for, intentional || lifecycle);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::resilience::{CircuitBreaker, CircuitBreakerConfig, CircuitState};

    fn test_config() -> CircuitBreakerConfig {
        // Trip on a clear majority of failures with a small, deterministic volume.
        CircuitBreakerConfig {
            error_rate_threshold: 0.5,
            min_request_volume: 4,
            window_size: 20,
            cooldown: Duration::from_millis(20),
            ..Default::default()
        }
    }

    #[test]
    fn parses_typed_reject_reasons_case_insensitively() {
        assert_eq!(
            InferRejectReason::parse("admission_rejected"),
            InferRejectReason::AdmissionRejected
        );
        assert_eq!(
            InferRejectReason::parse("ADMISSION-REJECTED"),
            InferRejectReason::AdmissionRejected
        );
        assert_eq!(
            InferRejectReason::parse(" model_not_ready "),
            InferRejectReason::ModelNotReady
        );
        assert_eq!(InferRejectReason::parse("draining"), InferRejectReason::Draining);
        // Unknown / transport codes fail safe to HardFailure.
        assert_eq!(InferRejectReason::parse("internal"), InferRejectReason::HardFailure);
        assert_eq!(InferRejectReason::parse(""), InferRejectReason::HardFailure);
    }

    #[test]
    fn reject_reasons_are_failover_eligible_hard_failures_are_not() {
        assert!(BreakerVerdict::for_error_code("admission_rejected").is_failover_eligible());
        assert!(BreakerVerdict::for_error_code("model_not_ready").is_failover_eligible());
        assert!(BreakerVerdict::for_error_code("draining").is_failover_eligible());
        assert!(!BreakerVerdict::for_error_code("provider_error").is_failover_eligible());
    }

    /// THE GW-3 RED TEST: repeated *genuine* failures open the breaker, but a storm of typed
    /// reject-reasons (admission/warming/draining) must leave it CLOSED — otherwise a capacity
    /// dip or a rolling restart would take the whole self-hosted tier offline.
    #[test]
    fn circuit_breaker_opens_on_repeated_failure() {
        // --- A genuine fault storm trips the breaker (the breaker still works). ---
        let cb = CircuitBreaker::new(test_config());
        assert_eq!(cb.state(), CircuitState::Closed);
        for _ in 0..6 {
            record_infer_outcome(&cb, BreakerVerdict::for_error_code("provider_error"));
        }
        assert_eq!(
            cb.state(),
            CircuitState::Open,
            "repeated genuine Infer failures must open the per-provider breaker"
        );

        // --- The GW-3 exemption: typed reject-reasons must NOT trip the breaker. ---
        for code in ["admission_rejected", "model_not_ready", "draining"] {
            let cb = CircuitBreaker::new(test_config());
            for _ in 0..50 {
                record_infer_outcome(&cb, BreakerVerdict::for_error_code(code));
            }
            assert_eq!(
                cb.state(),
                CircuitState::Closed,
                "a storm of `{code}` is a failover-eligible non-failure and must NOT open the breaker"
            );
            assert!(
                cb.allow_request(),
                "the breaker must keep admitting `{code}` so the tier is re-tried the instant it recovers"
            );
            assert_eq!(cb.error_rate(), 0.0, "`{code}` must not move the failure window");
        }
    }

    #[test]
    fn mixed_reject_reasons_do_not_dilute_a_real_failure_trip() {
        // Interleaving reject-reasons with real failures must still let the real failures trip:
        // the reject-reasons are simply invisible to the window, so 4 real failures at threshold
        // 0.5 / volume 4 still open it.
        let cb = CircuitBreaker::new(test_config());
        for _ in 0..10 {
            record_infer_outcome(&cb, BreakerVerdict::for_error_code("model_not_ready"));
        }
        assert_eq!(cb.state(), CircuitState::Closed);
        for _ in 0..4 {
            record_infer_outcome(&cb, BreakerVerdict::Failure);
        }
        assert_eq!(
            cb.state(),
            CircuitState::Open,
            "reject-reasons must not mask a real failure trip"
        );
    }

    #[test]
    fn lifecycle_close_does_not_arm_the_fatal_credentials_state() {
        // D-G2 interaction: a warming/draining worker dropping a sub-stable connection must NOT
        // look like the bad-credentials signature, or a rollout would falsely mark the tier FATAL.
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            max_quick_failures: 3,
            ..test_config()
        });
        for _ in 0..10 {
            record_infer_connection_closed(
                &cb,
                Duration::from_millis(100), // sub-stable
                false,                      // server-side close
                Some(InferRejectReason::ModelNotReady),
            );
        }
        assert!(
            !cb.is_permanently_failed(),
            "a warming-worker close streak must not arm the credentials-fatal state"
        );

        // ...but a genuine sub-stable close streak (no typed reason) still arms it.
        let cb2 = CircuitBreaker::new(CircuitBreakerConfig {
            max_quick_failures: 3,
            ..test_config()
        });
        for _ in 0..3 {
            record_infer_connection_closed(&cb2, Duration::from_millis(100), false, None);
        }
        assert!(
            cb2.is_permanently_failed(),
            "a genuine sub-stable close streak must still arm the credentials-fatal state"
        );
    }

    #[test]
    fn concurrent_reject_storm_keeps_breaker_closed_across_sessions() {
        // Multi-tenant correctness (>=4 concurrent): four "sessions" sharing one provider breaker
        // all see a reject-reason storm at once; the shared breaker must stay closed for all.
        use std::sync::Arc;
        let cb = Arc::new(CircuitBreaker::new(test_config()));
        let mut handles = vec![];
        for _ in 0..4 {
            let cb = Arc::clone(&cb);
            handles.push(std::thread::spawn(move || {
                for _ in 0..100 {
                    record_infer_outcome(
                        &cb,
                        BreakerVerdict::for_error_code("admission_rejected"),
                    );
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(
            cb.state(),
            CircuitState::Closed,
            "a concurrent reject storm across 4 sessions must leave the shared breaker closed"
        );
        assert!(cb.allow_request());
    }
}
