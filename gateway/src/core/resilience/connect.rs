//! Canonical outbound-dial timeout bounds (fleet-wide WS connect-timeout fix).
//!
//! A bare `tokio_tungstenite::connect_async(..).await` has NO deadline: a
//! SYN-dropped / blackholed TCP or TLS handshake (packet loss, a silently
//! dropping firewall, a hung upstream LB) parks the caller *inside the dial*,
//! where the reconnect supervisor cannot reach it — the circuit breaker never
//! records an outcome and the reconnect-governor permit is never released, so
//! one dead endpoint can pin a session (and its permit) forever.
//!
//! Every outbound WS dial in the gateway must therefore go through
//! [`with_timeout`] with one of the two shared bounds below, mapping
//! [`tokio::time::error::Elapsed`] onto the same transient connect-failure
//! error the site's existing `Err` arm produces (so breaker/backoff
//! classification treats a timeout like any other flaky dial).

use std::time::Duration;

/// Canonical bound for a single outbound WebSocket dial (TCP + TLS + HTTP
/// upgrade). 15s follows the in-tree Bedrock precedent
/// (`BEDROCK_CONNECT_TIMEOUT`) and is far above any healthy TLS+upgrade RTT —
/// a dial that hasn't completed by then is blackholed, not slow.
pub const WS_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// Bound for a whole transport-factory `connect()` call. REST-handshake
/// factories perform a REST POST *then* a WS upgrade, so this is 2x the
/// single-dial bound.
pub const FACTORY_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Bound `fut` by `bound`. Thin wrapper over [`tokio::time::timeout`] so call
/// sites share the constants above and read uniformly.
pub async fn with_timeout<F: std::future::Future>(
    bound: Duration,
    fut: F,
) -> Result<F::Output, tokio::time::error::Elapsed> {
    tokio::time::timeout(bound, fut).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn with_timeout_returns_elapsed_for_never_resolving_future() {
        let res = with_timeout(Duration::from_millis(10), std::future::pending::<()>()).await;
        assert!(res.is_err(), "pending future must time out");
    }

    #[tokio::test]
    async fn with_timeout_passes_through_ready_value() {
        let res = with_timeout(Duration::from_millis(10), async { 42u32 }).await;
        assert_eq!(res.expect("ready future must not time out"), 42);
    }
}
