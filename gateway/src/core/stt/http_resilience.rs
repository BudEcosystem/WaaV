//! Shared circuit-breaker wiring for request/response (HTTP) STT providers.
//!
//! The WS fleet threads [`crate::core::resilience::ResilienceHandles`] into the generic
//! `ReconnectableStream` supervisor; a plain reqwest provider (OpenAI Whisper, Groq, Bhashini,
//! FPT.AI, NAVER CLOVA, NECTEC, SberDevices, Yandex) has no persistent connection to supervise,
//! so the resilience gap there is UNIFORMITY + OBSERVABILITY: without a breaker consult there
//! are no `waav_circuit_breaker_state{provider}` gauge samples, no unified failure
//! classification, and every session keeps paying doomed upstream round-trips while a provider
//! is down.
//!
//! [`HttpBreaker`] closes that gap minimally. It wraps the SAME per-provider shared
//! [`CircuitBreaker`] the registry hands the WS fleet (injected via `BaseSTT::set_resilience`,
//! exactly like the streaming providers) and exposes the three moments a request/response
//! transport touches a breaker:
//!
//! 1. [`HttpBreaker::check`] BEFORE each upstream HTTP call — an open breaker fails fast with a
//!    typed [`STTError::ConnectionFailed`] so the gateway's failover sees a classified refusal
//!    instead of paying a doomed round-trip. (An elapsed cooldown admits exactly one half-open
//!    probe request, per the breaker's own semantics.)
//! 2. [`HttpBreaker::record_send_error`] when the transport itself fails (connect error,
//!    timeout, TLS) — a breaker failure, same class as the fleet's connect failures.
//! 3. [`HttpBreaker::record_status`] on every received response — the ONE place an HTTP status
//!    is classified against the breaker (mirrors the `record_infer_outcome` seam discipline):
//!    - **2xx** → success; also heals the D-G2 credentials-fatal streak (a 2xx proves the
//!      credentials/config work — the request/response equivalent of a stable connection).
//!    - **401/403** → failure + quick-failure signal (the D-G2 credentials/config signature);
//!      three consecutive auth rejections arm the recoverable FATAL state, exactly like three
//!      sub-stable server-side WS closes.
//!    - **429** → plain failure only — sustained rate-limiting may rate-trip the breaker (so
//!      failover reroutes), but it must NEVER look like bad credentials, so it does not feed
//!      the FATAL streak.
//!    - any other non-success (5xx, validation 4xx, unexpected 3xx) → plain failure. A
//!      per-request malformed payload (400/422) feeds the rate window — five consecutive ones
//!      trip the breaker Open — but never the credentials-FATAL state, so one bad request
//!      cannot take a healthy provider offline gateway-wide.
//!
//! Deliberately NOT here: retry loops (gateway-level failover handles rerouting; providers
//! that already retry, e.g. Groq/Bhashini, simply consult the breaker per attempt) and the
//! [`crate::core::resilience::ReconnectGovernor`] (it caps concurrent *reconnect dials*, which
//! a request/response transport does not perform).

use std::sync::Arc;
use std::time::Duration;

use reqwest::StatusCode;

use super::base::STTError;
use crate::core::resilience::{CircuitBreaker, ResilienceHandles};

/// The `stable_for` reported to [`CircuitBreaker::record_connection_closed`] on an HTTP 2xx.
/// Any value at or above the breaker's `min_stable_duration` (default 5s) resets the D-G2
/// quick-failure streak and clears a FATAL state — a successful authenticated round-trip is the
/// request/response proof that the credentials/config work.
const HTTP_SUCCESS_STABLE_FOR: Duration = Duration::from_secs(600);

/// Per-provider circuit-breaker handle for request/response (HTTP) STT providers.
///
/// Holds the provider's shared [`CircuitBreaker`] once `BaseSTT::set_resilience` has injected
/// the process-global [`ResilienceHandles`] (the VoiceManager and the DAG provider node both do
/// this in production). Before injection — e.g. a direct unit-test construction — every method
/// is a no-op and [`HttpBreaker::check`] always allows, so tests without a registry are
/// unaffected. `Clone` is cheap (an `Arc` + a `&'static str`) so background pseudo-streaming
/// tasks (SberDevices/Yandex) can carry a copy.
#[derive(Clone)]
pub(crate) struct HttpBreaker {
    /// Provider name stamped into the fail-fast error message (the breaker itself already
    /// carries the registry label for the metrics gauge).
    provider: &'static str,
    /// The shared per-provider breaker, once injected. `None` until `set_resilience`.
    breaker: Option<Arc<CircuitBreaker>>,
}

impl HttpBreaker {
    /// A handle with no breaker attached (all operations no-op until [`Self::set_handles`]).
    pub(crate) const fn new(provider: &'static str) -> Self {
        Self {
            provider,
            breaker: None,
        }
    }

    /// Attach the shared per-provider breaker from the injected process-global handles.
    /// The governor half is intentionally dropped — see the module docs.
    pub(crate) fn set_handles(&mut self, handles: ResilienceHandles) {
        self.breaker = Some(handles.breaker);
    }

    /// The shared circuit breaker this provider feeds, if the process-global resilience handles
    /// have been injected (W-D2). Two instances built from the same
    /// [`crate::core::resilience::ResilienceRegistry`] return the *same* `Arc`.
    pub(crate) fn breaker(&self) -> Option<&Arc<CircuitBreaker>> {
        self.breaker.as_ref()
    }

    /// Consult the breaker BEFORE an upstream HTTP call. Open (or FATAL within its cooldown) →
    /// a typed fast refusal WITHOUT contacting the upstream; the gateway's failover sees a
    /// classified [`STTError::ConnectionFailed`] instead of a slow network error. An elapsed
    /// cooldown admits this call as the single half-open probe.
    pub(crate) fn check(&self) -> Result<(), STTError> {
        match &self.breaker {
            Some(b) if !b.allow_request() => Err(STTError::ConnectionFailed(format!(
                "{} circuit breaker is {} — failing fast without contacting the upstream",
                self.provider,
                b.state().as_str()
            ))),
            _ => Ok(()),
        }
    }

    /// Record a transport-level send failure (connect error, timeout, TLS, body write) — same
    /// failure class as the fleet's connect/timeout failures.
    pub(crate) fn record_send_error(&self) {
        if let Some(b) = &self.breaker {
            b.record_failure();
        }
    }

    /// Classify a received HTTP response status against the breaker. See the module docs for
    /// the classification table; this is the ONE recording seam for HTTP statuses.
    pub(crate) fn record_status(&self, status: StatusCode) {
        let Some(b) = &self.breaker else {
            return;
        };
        if status.is_success() {
            b.record_success();
            // A 2xx proves the credentials/config work: heal the D-G2 quick-failure streak /
            // FATAL state exactly like a stable WS connection does.
            b.record_connection_closed(HTTP_SUCCESS_STABLE_FOR, false);
        } else if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            // The credentials/config signature: rate failure + D-G2 quick-failure signal.
            b.record_failure();
            b.record_connection_closed(Duration::ZERO, false);
        } else {
            // 5xx, 429, validation 4xx, unexpected 3xx: a plain breaker failure. 429 lands here
            // deliberately — rate-limiting may rate-trip the breaker but must never arm the
            // credentials-FATAL state.
            b.record_failure();
        }
    }
}

impl std::fmt::Debug for HttpBreaker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpBreaker")
            .field("provider", &self.provider)
            .field("breaker_state", &self.breaker.as_ref().map(|b| b.state()))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::resilience::{CircuitState, ResilienceRegistry};

    fn injected(provider: &'static str) -> HttpBreaker {
        let reg = ResilienceRegistry::new(4);
        let mut hb = HttpBreaker::new(provider);
        hb.set_handles(reg.handles_for(provider));
        hb
    }

    #[test]
    fn uninjected_handle_is_inert_and_always_allows() {
        let hb = HttpBreaker::new("http-inert");
        assert!(hb.check().is_ok());
        // Recording without a breaker must be a silent no-op, not a panic.
        hb.record_send_error();
        hb.record_status(StatusCode::INTERNAL_SERVER_ERROR);
        assert!(hb.breaker().is_none());
        assert!(hb.check().is_ok());
    }

    #[test]
    fn open_breaker_fails_fast_with_typed_refusal() {
        let hb = injected("http-open-refusal");
        let b = hb.breaker().unwrap();
        for _ in 0..10 {
            b.record_failure();
        }
        assert_eq!(b.state(), CircuitState::Open);
        match hb.check() {
            Err(STTError::ConnectionFailed(msg)) => {
                assert!(msg.contains("circuit breaker"), "{msg}");
                assert!(msg.contains("http-open-refusal"), "{msg}");
            }
            other => panic!("expected typed ConnectionFailed refusal, got {other:?}"),
        }
    }

    #[test]
    fn server_errors_and_send_errors_rate_trip_the_breaker() {
        let hb = injected("http-5xx-trip");
        // Default config: min volume 5, threshold 0.5.
        for _ in 0..3 {
            hb.record_status(StatusCode::INTERNAL_SERVER_ERROR);
        }
        for _ in 0..3 {
            hb.record_send_error();
        }
        assert_eq!(hb.breaker().unwrap().state(), CircuitState::Open);
        assert!(hb.check().is_err(), "open breaker must refuse");
    }

    #[test]
    fn three_consecutive_auth_rejections_arm_the_fatal_state() {
        let hb = injected("http-auth-fatal");
        for _ in 0..3 {
            hb.record_status(StatusCode::UNAUTHORIZED);
        }
        assert!(
            hb.breaker().unwrap().is_permanently_failed(),
            "3 consecutive 401s are the credentials signature (D-G2)"
        );
        assert!(hb.check().is_err(), "fatal breaker must refuse fast");
    }

    #[test]
    fn rate_limiting_never_arms_the_fatal_state() {
        let hb = injected("http-429-not-fatal");
        for _ in 0..20 {
            hb.record_status(StatusCode::TOO_MANY_REQUESTS);
        }
        assert!(
            !hb.breaker().unwrap().is_permanently_failed(),
            "429 must never look like bad credentials"
        );
        // It may (correctly) rate-trip the breaker Open so failover reroutes.
        assert_eq!(hb.breaker().unwrap().state(), CircuitState::Open);
    }

    #[test]
    fn a_success_heals_the_auth_streak_and_closes_the_window() {
        let hb = injected("http-success-heals");
        hb.record_status(StatusCode::UNAUTHORIZED);
        hb.record_status(StatusCode::UNAUTHORIZED);
        hb.record_status(StatusCode::OK); // resets the D-G2 streak
        hb.record_status(StatusCode::UNAUTHORIZED);
        hb.record_status(StatusCode::UNAUTHORIZED);
        assert!(
            !hb.breaker().unwrap().is_permanently_failed(),
            "an interleaved 2xx must reset the quick-failure streak (flaky != fatal)"
        );
    }

    #[test]
    fn validation_4xx_is_a_plain_failure_not_fatal() {
        let hb = injected("http-400-not-fatal");
        for _ in 0..10 {
            hb.record_status(StatusCode::BAD_REQUEST);
        }
        assert!(
            !hb.breaker().unwrap().is_permanently_failed(),
            "malformed-request storms rate-trip but must not arm FATAL"
        );
        assert_eq!(hb.breaker().unwrap().state(), CircuitState::Open);
    }
}
