//! Shared WebSocket reconnection utilities with exponential backoff.
//!
//! This module provides:
//! - [`ReconnectionConfig`] - Configuration for automatic reconnection with exponential backoff
//! - [`ReconnectionState`] - State machine for tracking reconnection attempts
//! - [`ReconnectionManager`] - Manager for coordinating reconnection with backoff
//!
//! # Example
//!
//! ```rust
//! use waav_gateway::core::websocket::{ReconnectionConfig, ReconnectionState, ReconnectionManager};
//!
//! let config = ReconnectionConfig::default();
//! let mut manager = ReconnectionManager::new(config);
//!
//! // On connection loss
//! if manager.should_retry() {
//!     let delay = manager.next_delay();
//!     // Wait delay milliseconds, then reconnect
//!     manager.record_attempt(true); // success
//! }
//! ```

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

// =============================================================================
// Configuration
// =============================================================================

/// Configuration for automatic reconnection behavior.
///
/// Supports exponential backoff with optional jitter to prevent thundering herd.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReconnectionConfig {
    /// Enable automatic reconnection on connection loss.
    /// Default: true
    pub enabled: bool,

    /// Maximum number of reconnection attempts before giving up.
    /// Set to 0 for unlimited attempts.
    /// Default: 5
    pub max_attempts: u32,

    /// Initial delay between reconnection attempts (milliseconds).
    /// Default: 1000ms (1 second)
    pub initial_delay_ms: u64,

    /// Maximum delay between reconnection attempts (milliseconds).
    /// Default: 30000ms (30 seconds)
    pub max_delay_ms: u64,

    /// Multiplier for exponential backoff.
    /// Each attempt multiplies the delay by this factor.
    /// Default: 2.0
    pub backoff_multiplier: f32,

    /// Whether to add jitter to the delay to prevent thundering herd.
    /// Adds up to ±25% random variation to the delay.
    /// Default: true
    pub jitter: bool,

    /// Reset backoff after successful connection lasting this long (ms).
    /// Set to 0 to never auto-reset.
    /// Default: 60000ms (1 minute)
    pub reset_after_ms: u64,
}

impl Default for ReconnectionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_attempts: 5,
            initial_delay_ms: 1000,
            max_delay_ms: 30000,
            backoff_multiplier: 2.0,
            jitter: true,
            reset_after_ms: 60000,
        }
    }
}

impl ReconnectionConfig {
    /// Create a builder for ReconnectionConfig.
    pub fn builder() -> ReconnectionConfigBuilder {
        ReconnectionConfigBuilder::default()
    }

    /// Create a config with reconnection disabled.
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Default::default()
        }
    }

    /// Create a config optimized for aggressive reconnection (low latency apps).
    pub fn aggressive() -> Self {
        Self {
            enabled: true,
            max_attempts: 10,
            initial_delay_ms: 100,
            max_delay_ms: 5000,
            backoff_multiplier: 1.5,
            jitter: true,
            reset_after_ms: 30000,
        }
    }

    /// Create a config optimized for conservative reconnection (high reliability).
    pub fn conservative() -> Self {
        Self {
            enabled: true,
            max_attempts: 3,
            initial_delay_ms: 2000,
            max_delay_ms: 60000,
            backoff_multiplier: 3.0,
            jitter: true,
            reset_after_ms: 120000,
        }
    }

    /// Calculate the delay for a given attempt number using exponential backoff.
    /// Returns the delay in milliseconds.
    ///
    /// # Arguments
    /// * `attempt` - The attempt number (1-based)
    pub fn calculate_delay(&self, attempt: u32) -> u64 {
        let base_delay = self.initial_delay_ms as f64;
        let multiplier = self.backoff_multiplier as f64;

        // Exponential backoff: base_delay * multiplier^(attempt-1)
        let exponent = attempt.saturating_sub(1).min(20) as i32; // Cap to prevent overflow
        let delay = base_delay * multiplier.powi(exponent);
        let delay = delay.min(self.max_delay_ms as f64);

        if self.jitter {
            // Add up to ±25% jitter
            let jitter_range = delay * 0.25;
            let jitter = generate_jitter(jitter_range);
            ((delay + jitter).max(0.0)) as u64
        } else {
            delay as u64
        }
    }

    /// Calculate delay without jitter (deterministic).
    /// Useful for testing and predictable scenarios.
    pub fn calculate_delay_deterministic(&self, attempt: u32) -> u64 {
        let base_delay = self.initial_delay_ms as f64;
        let multiplier = self.backoff_multiplier as f64;

        let exponent = attempt.saturating_sub(1).min(20) as i32;
        let delay = base_delay * multiplier.powi(exponent);
        delay.min(self.max_delay_ms as f64) as u64
    }

    /// Check if more reconnection attempts are allowed.
    pub fn should_retry(&self, attempt: u32) -> bool {
        self.enabled && (self.max_attempts == 0 || attempt < self.max_attempts)
    }
}

// =============================================================================
// Builder
// =============================================================================

/// Builder for ReconnectionConfig with fluent API.
#[derive(Debug, Clone, Default)]
pub struct ReconnectionConfigBuilder {
    config: ReconnectionConfig,
}

impl ReconnectionConfigBuilder {
    /// Set whether reconnection is enabled.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.config.enabled = enabled;
        self
    }

    /// Set maximum reconnection attempts (0 for unlimited).
    pub fn max_attempts(mut self, max_attempts: u32) -> Self {
        self.config.max_attempts = max_attempts;
        self
    }

    /// Set initial delay in milliseconds.
    pub fn initial_delay_ms(mut self, delay_ms: u64) -> Self {
        self.config.initial_delay_ms = delay_ms;
        self
    }

    /// Set maximum delay in milliseconds.
    pub fn max_delay_ms(mut self, delay_ms: u64) -> Self {
        self.config.max_delay_ms = delay_ms;
        self
    }

    /// Set backoff multiplier.
    pub fn backoff_multiplier(mut self, multiplier: f32) -> Self {
        self.config.backoff_multiplier = multiplier;
        self
    }

    /// Set whether to add jitter.
    pub fn jitter(mut self, jitter: bool) -> Self {
        self.config.jitter = jitter;
        self
    }

    /// Set reset threshold in milliseconds.
    pub fn reset_after_ms(mut self, reset_ms: u64) -> Self {
        self.config.reset_after_ms = reset_ms;
        self
    }

    /// Build the ReconnectionConfig.
    pub fn build(self) -> ReconnectionConfig {
        self.config
    }
}

// =============================================================================
// State Machine
// =============================================================================

/// Current state of the reconnection process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconnectionState {
    /// Initial state or after successful long connection.
    Idle,
    /// Currently attempting to reconnect.
    Reconnecting,
    /// Waiting for backoff delay before next attempt.
    WaitingBackoff,
    /// Reconnected successfully.
    Connected,
    /// All retry attempts exhausted.
    Failed,
    /// Reconnection is disabled.
    Disabled,
}

impl ReconnectionState {
    /// Check if in a terminal state (Failed or Disabled).
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            ReconnectionState::Failed | ReconnectionState::Disabled
        )
    }

    /// Check if currently connected.
    pub fn is_connected(&self) -> bool {
        matches!(self, ReconnectionState::Connected)
    }

    /// Check if reconnection is in progress.
    pub fn is_reconnecting(&self) -> bool {
        matches!(
            self,
            ReconnectionState::Reconnecting | ReconnectionState::WaitingBackoff
        )
    }
}

// =============================================================================
// Manager
// =============================================================================

/// Manager for coordinating reconnection with exponential backoff.
///
/// Thread-safe using atomic operations for lock-free hot paths.
pub struct ReconnectionManager {
    config: ReconnectionConfig,
    attempt_count: AtomicU32,
    last_attempt_ns: AtomicU64,
    connected_at_ns: AtomicU64,
    is_connected: AtomicBool,
    is_failed: AtomicBool,
    total_reconnections: AtomicU64,
    total_failures: AtomicU64,
}

impl ReconnectionManager {
    /// Create a new ReconnectionManager with the given configuration.
    pub fn new(config: ReconnectionConfig) -> Self {
        let is_disabled = !config.enabled;
        Self {
            config,
            attempt_count: AtomicU32::new(0),
            last_attempt_ns: AtomicU64::new(0),
            connected_at_ns: AtomicU64::new(0),
            is_connected: AtomicBool::new(false),
            is_failed: AtomicBool::new(is_disabled),
            total_reconnections: AtomicU64::new(0),
            total_failures: AtomicU64::new(0),
        }
    }

    /// Get the current reconnection state.
    pub fn state(&self) -> ReconnectionState {
        if !self.config.enabled {
            return ReconnectionState::Disabled;
        }
        if self.is_failed.load(Ordering::Acquire) {
            return ReconnectionState::Failed;
        }
        if self.is_connected.load(Ordering::Acquire) {
            return ReconnectionState::Connected;
        }
        let attempt = self.attempt_count.load(Ordering::Acquire);
        if attempt == 0 {
            ReconnectionState::Idle
        } else {
            ReconnectionState::WaitingBackoff
        }
    }

    /// Check if reconnection should be attempted.
    pub fn should_retry(&self) -> bool {
        if !self.config.enabled {
            return false;
        }
        if self.is_failed.load(Ordering::Acquire) {
            return false;
        }
        let attempt = self.attempt_count.load(Ordering::Acquire);
        self.config.should_retry(attempt)
    }

    /// Get the next delay in milliseconds based on current attempt count.
    pub fn next_delay(&self) -> u64 {
        let attempt = self.attempt_count.load(Ordering::Acquire);
        self.config.calculate_delay(attempt + 1)
    }

    /// Get the next delay as a Duration.
    pub fn next_delay_duration(&self) -> Duration {
        Duration::from_millis(self.next_delay())
    }

    /// Record the start of a reconnection attempt.
    pub fn start_attempt(&self) {
        self.attempt_count.fetch_add(1, Ordering::AcqRel);
        self.last_attempt_ns
            .store(now_monotonic_ns(), Ordering::Release);
    }

    /// Record the result of a reconnection attempt.
    ///
    /// # Arguments
    /// * `success` - Whether the reconnection succeeded
    pub fn record_attempt(&self, success: bool) {
        if success {
            self.is_connected.store(true, Ordering::Release);
            self.connected_at_ns
                .store(now_monotonic_ns(), Ordering::Release);
            self.attempt_count.store(0, Ordering::Release);
            self.total_reconnections.fetch_add(1, Ordering::Relaxed);
        } else {
            let attempt = self.attempt_count.load(Ordering::Acquire);
            if !self.config.should_retry(attempt) {
                self.is_failed.store(true, Ordering::Release);
                self.total_failures.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Called when connection is lost. Resets connected state and prepares for reconnection.
    pub fn on_connection_lost(&self) {
        let was_connected = self.is_connected.swap(false, Ordering::AcqRel);
        if was_connected {
            // Check if connection lasted long enough to reset backoff
            let connected_at = self.connected_at_ns.load(Ordering::Acquire);
            let now = now_monotonic_ns();
            let connected_duration_ms = (now.saturating_sub(connected_at)) / 1_000_000;

            if self.config.reset_after_ms > 0 && connected_duration_ms >= self.config.reset_after_ms
            {
                // Reset attempt count since connection was stable
                self.attempt_count.store(0, Ordering::Release);
            }
        }
        // Clear failed state to allow new reconnection cycle
        self.is_failed.store(false, Ordering::Release);
    }

    /// Reset the manager to initial state.
    pub fn reset(&self) {
        self.attempt_count.store(0, Ordering::Release);
        self.is_connected.store(false, Ordering::Release);
        self.is_failed
            .store(!self.config.enabled, Ordering::Release);
        self.last_attempt_ns.store(0, Ordering::Release);
        self.connected_at_ns.store(0, Ordering::Release);
    }

    /// Get the current attempt count.
    pub fn attempt_count(&self) -> u32 {
        self.attempt_count.load(Ordering::Acquire)
    }

    /// Get total successful reconnections.
    pub fn total_reconnections(&self) -> u64 {
        self.total_reconnections.load(Ordering::Relaxed)
    }

    /// Get total failed reconnection cycles.
    pub fn total_failures(&self) -> u64 {
        self.total_failures.load(Ordering::Relaxed)
    }

    /// Get a snapshot of current manager statistics.
    pub fn snapshot(&self) -> ReconnectionSnapshot {
        ReconnectionSnapshot {
            state: self.state(),
            attempt_count: self.attempt_count.load(Ordering::Relaxed),
            total_reconnections: self.total_reconnections.load(Ordering::Relaxed),
            total_failures: self.total_failures.load(Ordering::Relaxed),
            is_connected: self.is_connected.load(Ordering::Relaxed),
        }
    }
}

/// Point-in-time snapshot of reconnection manager state.
#[derive(Debug, Clone)]
pub struct ReconnectionSnapshot {
    pub state: ReconnectionState,
    pub attempt_count: u32,
    pub total_reconnections: u64,
    pub total_failures: u64,
    pub is_connected: bool,
}

// =============================================================================
// Utility Functions
// =============================================================================

/// Get current monotonic time in nanoseconds.
fn now_monotonic_ns() -> u64 {
    use std::sync::OnceLock;
    static START: OnceLock<Instant> = OnceLock::new();
    let start = START.get_or_init(Instant::now);
    start.elapsed().as_nanos() as u64
}

/// Generate a pseudo-random jitter value using a simple LCG.
/// This avoids pulling in the rand crate for a simple use case.
fn generate_jitter(range: f64) -> f64 {
    use std::time::SystemTime;
    let seed = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    // Simple LCG: (a * seed + c) mod m
    let random = ((seed.wrapping_mul(1103515245).wrapping_add(12345)) % (1 << 31)) as f64;
    let normalized = random / (1u64 << 31) as f64; // 0.0 to 1.0
    (normalized - 0.5) * 2.0 * range // -range to +range
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // ReconnectionConfig Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_config_default() {
        let config = ReconnectionConfig::default();
        assert!(config.enabled);
        assert_eq!(config.max_attempts, 5);
        assert_eq!(config.initial_delay_ms, 1000);
        assert_eq!(config.max_delay_ms, 30000);
        assert_eq!(config.backoff_multiplier, 2.0);
        assert!(config.jitter);
        assert_eq!(config.reset_after_ms, 60000);
    }

    #[test]
    fn test_config_disabled() {
        let config = ReconnectionConfig::disabled();
        assert!(!config.enabled);
    }

    #[test]
    fn test_config_aggressive() {
        let config = ReconnectionConfig::aggressive();
        assert!(config.enabled);
        assert_eq!(config.max_attempts, 10);
        assert_eq!(config.initial_delay_ms, 100);
        assert!(config.initial_delay_ms < ReconnectionConfig::default().initial_delay_ms);
    }

    #[test]
    fn test_config_conservative() {
        let config = ReconnectionConfig::conservative();
        assert!(config.enabled);
        assert_eq!(config.max_attempts, 3);
        assert!(config.initial_delay_ms > ReconnectionConfig::default().initial_delay_ms);
    }

    #[test]
    fn test_builder() {
        let config = ReconnectionConfig::builder()
            .enabled(true)
            .max_attempts(10)
            .initial_delay_ms(500)
            .max_delay_ms(10000)
            .backoff_multiplier(1.5)
            .jitter(false)
            .reset_after_ms(30000)
            .build();

        assert!(config.enabled);
        assert_eq!(config.max_attempts, 10);
        assert_eq!(config.initial_delay_ms, 500);
        assert_eq!(config.max_delay_ms, 10000);
        assert_eq!(config.backoff_multiplier, 1.5);
        assert!(!config.jitter);
        assert_eq!(config.reset_after_ms, 30000);
    }

    #[test]
    fn test_calculate_delay_deterministic() {
        let config = ReconnectionConfig {
            initial_delay_ms: 1000,
            max_delay_ms: 30000,
            backoff_multiplier: 2.0,
            jitter: false,
            ..Default::default()
        };

        // First attempt: 1000ms
        assert_eq!(config.calculate_delay_deterministic(1), 1000);

        // Second attempt: 2000ms
        assert_eq!(config.calculate_delay_deterministic(2), 2000);

        // Third attempt: 4000ms
        assert_eq!(config.calculate_delay_deterministic(3), 4000);

        // Fourth attempt: 8000ms
        assert_eq!(config.calculate_delay_deterministic(4), 8000);

        // Fifth attempt: 16000ms
        assert_eq!(config.calculate_delay_deterministic(5), 16000);

        // Sixth attempt: capped at 30000ms
        assert_eq!(config.calculate_delay_deterministic(6), 30000);
    }

    #[test]
    fn test_calculate_delay_with_jitter_bounds() {
        let config = ReconnectionConfig {
            initial_delay_ms: 1000,
            jitter: true,
            ..Default::default()
        };

        // Run multiple times to test jitter variance
        for _ in 0..100 {
            let delay = config.calculate_delay(1);
            // With ±25% jitter, delay should be 750-1250
            assert!(
                (750..=1250).contains(&delay),
                "Delay {} should be within 750-1250",
                delay
            );
        }
    }

    #[test]
    fn test_calculate_delay_capped_at_max() {
        let config = ReconnectionConfig {
            initial_delay_ms: 1000,
            max_delay_ms: 5000,
            backoff_multiplier: 10.0,
            jitter: false,
            ..Default::default()
        };

        // Even with high multiplier, delay should be capped
        assert_eq!(config.calculate_delay_deterministic(5), 5000);
        assert_eq!(config.calculate_delay_deterministic(100), 5000);
    }

    #[test]
    fn test_calculate_delay_overflow_protection() {
        let config = ReconnectionConfig {
            initial_delay_ms: 1000,
            max_delay_ms: 30000,
            backoff_multiplier: 2.0,
            jitter: false,
            ..Default::default()
        };

        // Should not overflow with large attempt numbers
        let delay = config.calculate_delay_deterministic(u32::MAX);
        assert!(delay <= 30000);
    }

    #[test]
    fn test_should_retry_within_max_attempts() {
        let config = ReconnectionConfig {
            max_attempts: 5,
            ..Default::default()
        };

        assert!(config.should_retry(0));
        assert!(config.should_retry(1));
        assert!(config.should_retry(4));
        assert!(!config.should_retry(5));
        assert!(!config.should_retry(10));
    }

    #[test]
    fn test_should_retry_unlimited() {
        let config = ReconnectionConfig {
            max_attempts: 0, // Unlimited
            ..Default::default()
        };

        assert!(config.should_retry(0));
        assert!(config.should_retry(100));
        assert!(config.should_retry(1000));
        assert!(config.should_retry(u32::MAX - 1));
    }

    #[test]
    fn test_should_retry_disabled() {
        let config = ReconnectionConfig::disabled();

        assert!(!config.should_retry(0));
        assert!(!config.should_retry(1));
    }

    // -------------------------------------------------------------------------
    // ReconnectionState Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_state_is_terminal() {
        assert!(ReconnectionState::Failed.is_terminal());
        assert!(ReconnectionState::Disabled.is_terminal());
        assert!(!ReconnectionState::Idle.is_terminal());
        assert!(!ReconnectionState::Connected.is_terminal());
        assert!(!ReconnectionState::Reconnecting.is_terminal());
    }

    #[test]
    fn test_state_is_connected() {
        assert!(ReconnectionState::Connected.is_connected());
        assert!(!ReconnectionState::Idle.is_connected());
        assert!(!ReconnectionState::Failed.is_connected());
    }

    #[test]
    fn test_state_is_reconnecting() {
        assert!(ReconnectionState::Reconnecting.is_reconnecting());
        assert!(ReconnectionState::WaitingBackoff.is_reconnecting());
        assert!(!ReconnectionState::Idle.is_reconnecting());
        assert!(!ReconnectionState::Connected.is_reconnecting());
    }

    // -------------------------------------------------------------------------
    // ReconnectionManager Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_manager_initial_state() {
        let config = ReconnectionConfig::default();
        let manager = ReconnectionManager::new(config);

        assert_eq!(manager.state(), ReconnectionState::Idle);
        assert!(manager.should_retry());
        assert_eq!(manager.attempt_count(), 0);
        assert_eq!(manager.total_reconnections(), 0);
        assert_eq!(manager.total_failures(), 0);
    }

    #[test]
    fn test_manager_disabled() {
        let config = ReconnectionConfig::disabled();
        let manager = ReconnectionManager::new(config);

        assert_eq!(manager.state(), ReconnectionState::Disabled);
        assert!(!manager.should_retry());
    }

    #[test]
    fn test_manager_start_attempt() {
        let config = ReconnectionConfig::default();
        let manager = ReconnectionManager::new(config);

        assert_eq!(manager.attempt_count(), 0);

        manager.start_attempt();
        assert_eq!(manager.attempt_count(), 1);

        manager.start_attempt();
        assert_eq!(manager.attempt_count(), 2);
    }

    #[test]
    fn test_manager_successful_reconnection() {
        let config = ReconnectionConfig::default();
        let manager = ReconnectionManager::new(config);

        manager.start_attempt();
        assert_eq!(manager.attempt_count(), 1);

        manager.record_attempt(true);
        assert_eq!(manager.state(), ReconnectionState::Connected);
        assert_eq!(manager.attempt_count(), 0); // Reset on success
        assert_eq!(manager.total_reconnections(), 1);
    }

    #[test]
    fn test_manager_failed_reconnection() {
        let config = ReconnectionConfig {
            max_attempts: 3,
            ..Default::default()
        };
        let manager = ReconnectionManager::new(config);

        // Fail 3 times
        for i in 0..3 {
            manager.start_attempt();
            assert_eq!(manager.attempt_count(), i + 1);
            manager.record_attempt(false);
        }

        assert_eq!(manager.state(), ReconnectionState::Failed);
        assert!(!manager.should_retry());
        assert_eq!(manager.total_failures(), 1);
    }

    #[test]
    fn test_manager_connection_lost() {
        let config = ReconnectionConfig {
            reset_after_ms: 0, // Don't auto-reset
            ..Default::default()
        };
        let manager = ReconnectionManager::new(config);

        // Connect successfully
        manager.start_attempt();
        manager.record_attempt(true);
        assert_eq!(manager.state(), ReconnectionState::Connected);

        // Lose connection
        manager.on_connection_lost();
        assert!(!manager.snapshot().is_connected);
        assert!(manager.should_retry()); // Can retry again
    }

    #[test]
    fn test_manager_reset() {
        let config = ReconnectionConfig::default();
        let manager = ReconnectionManager::new(config);

        // Do some attempts
        manager.start_attempt();
        manager.start_attempt();
        manager.record_attempt(true);

        // Reset
        manager.reset();
        assert_eq!(manager.state(), ReconnectionState::Idle);
        assert_eq!(manager.attempt_count(), 0);
    }

    #[test]
    fn test_manager_next_delay() {
        let config = ReconnectionConfig {
            initial_delay_ms: 1000,
            backoff_multiplier: 2.0,
            jitter: false,
            ..Default::default()
        };
        let manager = ReconnectionManager::new(config);

        // Before any attempts, next delay is for attempt 1
        assert_eq!(manager.next_delay(), 1000);

        // After first attempt
        manager.start_attempt();
        assert_eq!(manager.next_delay(), 2000);

        // After second attempt
        manager.start_attempt();
        assert_eq!(manager.next_delay(), 4000);
    }

    #[test]
    fn test_manager_next_delay_duration() {
        let config = ReconnectionConfig {
            initial_delay_ms: 500,
            jitter: false,
            ..Default::default()
        };
        let manager = ReconnectionManager::new(config);

        assert_eq!(manager.next_delay_duration(), Duration::from_millis(500));
    }

    #[test]
    fn test_manager_snapshot() {
        let config = ReconnectionConfig::default();
        let manager = ReconnectionManager::new(config);

        manager.start_attempt();
        manager.record_attempt(true);

        let snapshot = manager.snapshot();
        assert_eq!(snapshot.state, ReconnectionState::Connected);
        assert_eq!(snapshot.attempt_count, 0);
        assert_eq!(snapshot.total_reconnections, 1);
        assert_eq!(snapshot.total_failures, 0);
        assert!(snapshot.is_connected);
    }

    #[test]
    fn test_manager_thread_safety() {
        use std::sync::Arc;
        use std::thread;

        let config = ReconnectionConfig {
            max_attempts: 0, // Unlimited for this test
            ..Default::default()
        };
        let manager = Arc::new(ReconnectionManager::new(config));

        let mut handles = vec![];

        // Spawn multiple threads doing concurrent operations
        for _ in 0..10 {
            let m = Arc::clone(&manager);
            handles.push(thread::spawn(move || {
                for _ in 0..100 {
                    m.start_attempt();
                    let _ = m.state();
                    let _ = m.should_retry();
                    let _ = m.next_delay();
                }
            }));
        }

        for handle in handles {
            handle.join().expect("Thread panicked");
        }

        // Should have recorded 1000 attempts total
        assert_eq!(manager.attempt_count(), 1000);
    }

    #[test]
    fn test_manager_retry_cycle() {
        let config = ReconnectionConfig {
            max_attempts: 3,
            initial_delay_ms: 100,
            backoff_multiplier: 2.0,
            jitter: false,
            ..Default::default()
        };
        let manager = ReconnectionManager::new(config);

        // Simulate a full retry cycle
        let mut delays = vec![];

        while manager.should_retry() {
            delays.push(manager.next_delay());
            manager.start_attempt();
            manager.record_attempt(false);
        }

        assert_eq!(delays, vec![100, 200, 400]);
        assert_eq!(manager.state(), ReconnectionState::Failed);
    }

    #[test]
    fn test_manager_multiple_connection_cycles() {
        let config = ReconnectionConfig {
            max_attempts: 2,
            reset_after_ms: 0, // Don't auto-reset based on duration
            ..Default::default()
        };
        let manager = ReconnectionManager::new(config);

        // First connection cycle
        manager.start_attempt();
        manager.record_attempt(true);
        assert_eq!(manager.total_reconnections(), 1);

        // Lose connection
        manager.on_connection_lost();
        assert!(manager.should_retry());

        // Second connection cycle
        manager.start_attempt();
        manager.record_attempt(true);
        assert_eq!(manager.total_reconnections(), 2);
    }

    // -------------------------------------------------------------------------
    // Edge Cases
    // -------------------------------------------------------------------------

    #[test]
    fn test_zero_initial_delay() {
        let config = ReconnectionConfig {
            initial_delay_ms: 0,
            jitter: false,
            ..Default::default()
        };

        assert_eq!(config.calculate_delay_deterministic(1), 0);
        assert_eq!(config.calculate_delay_deterministic(5), 0);
    }

    #[test]
    fn test_one_multiplier() {
        let config = ReconnectionConfig {
            initial_delay_ms: 1000,
            backoff_multiplier: 1.0,
            jitter: false,
            ..Default::default()
        };

        // Delay should stay constant with multiplier of 1
        assert_eq!(config.calculate_delay_deterministic(1), 1000);
        assert_eq!(config.calculate_delay_deterministic(5), 1000);
        assert_eq!(config.calculate_delay_deterministic(10), 1000);
    }

    #[test]
    fn test_fractional_multiplier() {
        let config = ReconnectionConfig {
            initial_delay_ms: 1000,
            backoff_multiplier: 0.5,
            max_delay_ms: 30000,
            jitter: false,
            ..Default::default()
        };

        // Delay should decrease with multiplier < 1
        assert_eq!(config.calculate_delay_deterministic(1), 1000);
        assert_eq!(config.calculate_delay_deterministic(2), 500);
        assert_eq!(config.calculate_delay_deterministic(3), 250);
    }

    #[test]
    fn test_max_delay_equals_initial() {
        let config = ReconnectionConfig {
            initial_delay_ms: 1000,
            max_delay_ms: 1000,
            backoff_multiplier: 2.0,
            jitter: false,
            ..Default::default()
        };

        // All delays should be capped at 1000
        assert_eq!(config.calculate_delay_deterministic(1), 1000);
        assert_eq!(config.calculate_delay_deterministic(5), 1000);
    }

    #[test]
    fn test_max_delay_less_than_initial() {
        let config = ReconnectionConfig {
            initial_delay_ms: 2000,
            max_delay_ms: 1000,
            backoff_multiplier: 2.0,
            jitter: false,
            ..Default::default()
        };

        // Initial delay exceeds max, should be capped
        assert_eq!(config.calculate_delay_deterministic(1), 1000);
    }
}
