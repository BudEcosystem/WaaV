pub mod auth;
pub mod config;
pub mod core;
#[cfg(feature = "dag-routing")]
pub mod dag;
pub mod docs;
pub mod errors;
pub mod handlers;
pub mod init;
pub mod livekit;
pub mod middleware;
pub mod observability;
pub mod plugin;
pub mod routes;
pub mod state;
pub mod utils;

// Re-export commonly used items for convenience
pub use config::ServerConfig;
pub use core::*;
pub use errors::app_error::{AppError, AppResult};
pub use errors::auth_error::{AuthError, AuthResult};
pub use plugin::global_registry;
pub use state::AppState;

/// Replenish interval, in milliseconds, for a limit expressed in requests per second.
///
/// `tower_governor`'s `GovernorConfigBuilder::per_second(n)` reads like "n requests per
/// second" and is not: it assigns `self.period = Duration::from_secs(n)`, the interval after
/// which ONE token is replenished. Passing a configured 60 rps straight into it therefore
/// produced one request per minute — about 3600x tighter than the number said — and the
/// Kubernetes readiness probe started getting 429 as soon as the burst was spent.
///
/// Floored at 1ms because the builder rejects a zero interval, so anything above 1000 rps is
/// treated as effectively unlimited rather than as an error.
pub fn rate_limit_period_ms(requests_per_second: u32) -> u64 {
    std::cmp::max(1, 1000 / std::cmp::max(1, requests_per_second) as u64)
}

#[cfg(test)]
mod rate_limit_tests {
    use super::rate_limit_period_ms;

    #[test]
    fn a_configured_rate_becomes_the_right_interval() {
        // The regression: 60 rps must be one token every ~16ms, NOT every 60 seconds.
        assert_eq!(rate_limit_period_ms(60), 16);
        assert_eq!(rate_limit_period_ms(1), 1000);
        assert_eq!(rate_limit_period_ms(100), 10);
    }

    #[test]
    fn the_interval_is_never_zero() {
        // Duration::from_millis(0) is rejected by the builder, which would panic at startup.
        assert_eq!(rate_limit_period_ms(10_000), 1);
        assert_eq!(rate_limit_period_ms(0), 1000);
    }

    #[test]
    fn a_readiness_probe_is_never_starved_at_the_configured_default() {
        // The chart ships 60 rps and the kubelet polls /ready every 5s. Under the old
        // `per_second(60)` reading the probe was starved after the burst; assert the interval
        // is comfortably shorter than the probe period rather than merely "not 60s".
        assert!(rate_limit_period_ms(60) < 5_000);
    }
}
