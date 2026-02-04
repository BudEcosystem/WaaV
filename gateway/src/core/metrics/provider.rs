//! Provider performance metrics with TTFB (Time-To-First-Byte) tracking.
//!
//! This module provides atomic, lock-free metrics collection for STT/TTS providers.
//!
//! # Metrics Tracked
//!
//! - Request count (total, successful, failed)
//! - Time-to-first-byte (TTFB): min, max, sum for average
//! - Total processing time
//! - Error count by category
//!
//! # Example
//!
//! ```rust
//! let metrics = ProviderMetrics::new("deepgram");
//!
//! // Start timing a request
//! let mut timer = metrics.start_request();
//!
//! // ... make HTTP request ...
//!
//! // Record when first byte arrives
//! timer.record_first_byte();
//!
//! // ... process response ...
//!
//! // Finish timing (records total time)
//! timer.finish();
//!
//! // Get metrics snapshot
//! let snapshot = metrics.snapshot();
//! println!("Average TTFB: {}ms", snapshot.ttfb_avg_ms);
//! ```

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

// =============================================================================
// Provider Metrics
// =============================================================================

/// Atomic metrics for a single provider.
///
/// All operations are lock-free using atomic memory operations,
/// making this safe for concurrent access from multiple threads.
pub struct ProviderMetrics {
    /// Provider name for identification
    provider_name: String,

    /// Total request count
    request_count: AtomicU64,

    /// Successful request count
    success_count: AtomicU64,

    /// Failed request count
    error_count: AtomicU64,

    /// Sum of all TTFB measurements (nanoseconds)
    ttfb_sum_ns: AtomicU64,

    /// Number of TTFB samples (requests that got first byte)
    ttfb_count: AtomicU64,

    /// Maximum TTFB observed (nanoseconds)
    ttfb_max_ns: AtomicU64,

    /// Minimum TTFB observed (nanoseconds)
    ttfb_min_ns: AtomicU64,

    /// Sum of all request durations (nanoseconds)
    processing_sum_ns: AtomicU64,

    /// Number of completed requests
    processing_count: AtomicU64,

    /// Timeout count
    timeout_count: AtomicU64,

    /// Rate limit count (429 responses)
    rate_limit_count: AtomicU64,
}

impl ProviderMetrics {
    /// Create new metrics for a provider.
    pub fn new(provider_name: &str) -> Self {
        Self {
            provider_name: provider_name.to_string(),
            request_count: AtomicU64::new(0),
            success_count: AtomicU64::new(0),
            error_count: AtomicU64::new(0),
            ttfb_sum_ns: AtomicU64::new(0),
            ttfb_count: AtomicU64::new(0),
            ttfb_max_ns: AtomicU64::new(0),
            ttfb_min_ns: AtomicU64::new(u64::MAX),
            processing_sum_ns: AtomicU64::new(0),
            processing_count: AtomicU64::new(0),
            timeout_count: AtomicU64::new(0),
            rate_limit_count: AtomicU64::new(0),
        }
    }

    /// Get the provider name.
    pub fn provider_name(&self) -> &str {
        &self.provider_name
    }

    /// Start timing a new request.
    ///
    /// Returns a timer that should be used to record timing events.
    /// The timer automatically records metrics when dropped or `finish()` is called.
    pub fn start_request(&self) -> RequestTimer<'_> {
        self.request_count.fetch_add(1, Ordering::Relaxed);
        RequestTimer {
            metrics: self,
            start_time: Instant::now(),
            first_byte_time: None,
            finished: false,
        }
    }

    /// Record a successful request (for manual tracking without timer).
    pub fn record_success(&self) {
        self.success_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a failed request.
    pub fn record_error(&self) {
        self.error_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a timeout.
    pub fn record_timeout(&self) {
        self.timeout_count.fetch_add(1, Ordering::Relaxed);
        self.error_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a rate limit (429 response).
    pub fn record_rate_limit(&self) {
        self.rate_limit_count.fetch_add(1, Ordering::Relaxed);
        self.error_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a TTFB sample (internal use by RequestTimer).
    fn record_ttfb(&self, ttfb_ns: u64) {
        self.ttfb_sum_ns.fetch_add(ttfb_ns, Ordering::Relaxed);
        self.ttfb_count.fetch_add(1, Ordering::Relaxed);

        // Update max (using compare-exchange loop)
        loop {
            let current_max = self.ttfb_max_ns.load(Ordering::Relaxed);
            if ttfb_ns <= current_max {
                break;
            }
            if self
                .ttfb_max_ns
                .compare_exchange_weak(current_max, ttfb_ns, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                break;
            }
        }

        // Update min
        loop {
            let current_min = self.ttfb_min_ns.load(Ordering::Relaxed);
            if ttfb_ns >= current_min {
                break;
            }
            if self
                .ttfb_min_ns
                .compare_exchange_weak(current_min, ttfb_ns, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                break;
            }
        }
    }

    /// Record total processing time (internal use by RequestTimer).
    fn record_processing_time(&self, duration_ns: u64) {
        self.processing_sum_ns
            .fetch_add(duration_ns, Ordering::Relaxed);
        self.processing_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Get a snapshot of current metrics.
    pub fn snapshot(&self) -> ProviderMetricsSnapshot {
        let ttfb_count = self.ttfb_count.load(Ordering::Relaxed);
        let processing_count = self.processing_count.load(Ordering::Relaxed);

        let ttfb_avg_ms = if ttfb_count > 0 {
            (self.ttfb_sum_ns.load(Ordering::Relaxed) / ttfb_count / 1_000_000) as u32
        } else {
            0
        };

        let ttfb_min_ns = self.ttfb_min_ns.load(Ordering::Relaxed);
        let ttfb_min_ms = if ttfb_min_ns == u64::MAX {
            0
        } else {
            (ttfb_min_ns / 1_000_000) as u32
        };

        let processing_avg_ms = if processing_count > 0 {
            (self.processing_sum_ns.load(Ordering::Relaxed) / processing_count / 1_000_000) as u32
        } else {
            0
        };

        ProviderMetricsSnapshot {
            provider_name: self.provider_name.clone(),
            request_count: self.request_count.load(Ordering::Relaxed),
            success_count: self.success_count.load(Ordering::Relaxed),
            error_count: self.error_count.load(Ordering::Relaxed),
            ttfb_avg_ms,
            ttfb_min_ms,
            ttfb_max_ms: (self.ttfb_max_ns.load(Ordering::Relaxed) / 1_000_000) as u32,
            ttfb_sample_count: ttfb_count,
            processing_avg_ms,
            timeout_count: self.timeout_count.load(Ordering::Relaxed),
            rate_limit_count: self.rate_limit_count.load(Ordering::Relaxed),
        }
    }

    /// Reset all metrics to zero.
    pub fn reset(&self) {
        self.request_count.store(0, Ordering::Relaxed);
        self.success_count.store(0, Ordering::Relaxed);
        self.error_count.store(0, Ordering::Relaxed);
        self.ttfb_sum_ns.store(0, Ordering::Relaxed);
        self.ttfb_count.store(0, Ordering::Relaxed);
        self.ttfb_max_ns.store(0, Ordering::Relaxed);
        self.ttfb_min_ns.store(u64::MAX, Ordering::Relaxed);
        self.processing_sum_ns.store(0, Ordering::Relaxed);
        self.processing_count.store(0, Ordering::Relaxed);
        self.timeout_count.store(0, Ordering::Relaxed);
        self.rate_limit_count.store(0, Ordering::Relaxed);
    }
}

// =============================================================================
// Request Timer
// =============================================================================

/// Timer for tracking a single request's timing.
///
/// Created by [`ProviderMetrics::start_request()`].
/// Records metrics when `finish()` is called or when dropped.
pub struct RequestTimer<'a> {
    metrics: &'a ProviderMetrics,
    start_time: Instant,
    first_byte_time: Option<Instant>,
    finished: bool,
}

impl<'a> RequestTimer<'a> {
    /// Record when the first byte of response arrives.
    ///
    /// Call this as soon as you receive the first byte from the provider.
    /// Only the first call has effect; subsequent calls are ignored.
    pub fn record_first_byte(&mut self) {
        if self.first_byte_time.is_none() {
            self.first_byte_time = Some(Instant::now());
        }
    }

    /// Check if first byte has been recorded.
    pub fn has_first_byte(&self) -> bool {
        self.first_byte_time.is_some()
    }

    /// Get elapsed time since request start (milliseconds).
    pub fn elapsed_ms(&self) -> u64 {
        self.start_time.elapsed().as_millis() as u64
    }

    /// Get TTFB if first byte was recorded (milliseconds).
    pub fn ttfb_ms(&self) -> Option<u64> {
        self.first_byte_time
            .map(|t| (t - self.start_time).as_millis() as u64)
    }

    /// Finish timing and record metrics.
    ///
    /// Records TTFB (if first byte was recorded) and total processing time.
    /// Marks request as successful.
    pub fn finish(mut self) {
        self.do_finish(true);
    }

    /// Finish timing with error status.
    ///
    /// Records processing time but marks request as failed.
    pub fn finish_with_error(mut self) {
        self.do_finish(false);
    }

    fn do_finish(&mut self, success: bool) {
        if self.finished {
            return;
        }
        self.finished = true;

        let total_ns = self.start_time.elapsed().as_nanos() as u64;
        self.metrics.record_processing_time(total_ns);

        if let Some(first_byte) = self.first_byte_time {
            let ttfb_ns = (first_byte - self.start_time).as_nanos() as u64;
            self.metrics.record_ttfb(ttfb_ns);
        }

        if success {
            self.metrics.record_success();
        } else {
            self.metrics.record_error();
        }
    }
}

impl Drop for RequestTimer<'_> {
    fn drop(&mut self) {
        if !self.finished {
            // Auto-record as success if not explicitly finished
            self.do_finish(true);
        }
    }
}

// =============================================================================
// Metrics Snapshot
// =============================================================================

/// Point-in-time snapshot of provider metrics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderMetricsSnapshot {
    /// Provider name
    pub provider_name: String,

    /// Total requests initiated
    pub request_count: u64,

    /// Successful requests
    pub success_count: u64,

    /// Failed requests (includes timeouts and rate limits)
    pub error_count: u64,

    /// Average TTFB in milliseconds
    pub ttfb_avg_ms: u32,

    /// Minimum TTFB in milliseconds
    pub ttfb_min_ms: u32,

    /// Maximum TTFB in milliseconds
    pub ttfb_max_ms: u32,

    /// Number of TTFB samples collected
    pub ttfb_sample_count: u64,

    /// Average total processing time in milliseconds
    pub processing_avg_ms: u32,

    /// Timeout count
    pub timeout_count: u64,

    /// Rate limit (429) count
    pub rate_limit_count: u64,
}

impl ProviderMetricsSnapshot {
    /// Calculate success rate (0.0 to 1.0).
    pub fn success_rate(&self) -> f64 {
        if self.request_count == 0 {
            return 1.0;
        }
        self.success_count as f64 / self.request_count as f64
    }

    /// Calculate error rate (0.0 to 1.0).
    pub fn error_rate(&self) -> f64 {
        if self.request_count == 0 {
            return 0.0;
        }
        self.error_count as f64 / self.request_count as f64
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    // -------------------------------------------------------------------------
    // Construction Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_metrics_creation() {
        let metrics = ProviderMetrics::new("deepgram");
        assert_eq!(metrics.provider_name(), "deepgram");

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.provider_name, "deepgram");
        assert_eq!(snapshot.request_count, 0);
        assert_eq!(snapshot.success_count, 0);
        assert_eq!(snapshot.error_count, 0);
        assert_eq!(snapshot.ttfb_avg_ms, 0);
    }

    // -------------------------------------------------------------------------
    // Request Counter Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_request_count_increment() {
        let metrics = ProviderMetrics::new("test");

        let _timer1 = metrics.start_request();
        let _timer2 = metrics.start_request();
        let _timer3 = metrics.start_request();

        assert_eq!(metrics.snapshot().request_count, 3);
    }

    #[test]
    fn test_success_count() {
        let metrics = ProviderMetrics::new("test");

        let timer1 = metrics.start_request();
        timer1.finish(); // Success

        let timer2 = metrics.start_request();
        timer2.finish(); // Success

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.request_count, 2);
        assert_eq!(snapshot.success_count, 2);
        assert_eq!(snapshot.error_count, 0);
    }

    #[test]
    fn test_error_count() {
        let metrics = ProviderMetrics::new("test");

        let timer = metrics.start_request();
        timer.finish_with_error();

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.request_count, 1);
        assert_eq!(snapshot.success_count, 0);
        assert_eq!(snapshot.error_count, 1);
    }

    #[test]
    fn test_manual_error_recording() {
        let metrics = ProviderMetrics::new("test");

        metrics.record_error();
        metrics.record_error();

        assert_eq!(metrics.snapshot().error_count, 2);
    }

    // -------------------------------------------------------------------------
    // TTFB Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_ttfb_recording() {
        let metrics = ProviderMetrics::new("test");

        let mut timer = metrics.start_request();
        thread::sleep(Duration::from_millis(5));
        timer.record_first_byte();
        thread::sleep(Duration::from_millis(5));
        timer.finish();

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.ttfb_sample_count, 1);
        assert!(snapshot.ttfb_avg_ms >= 5);
        assert!(snapshot.ttfb_avg_ms < 15); // Should be ~5ms, not 10ms
    }

    #[test]
    fn test_ttfb_multiple_samples() {
        let metrics = ProviderMetrics::new("test");

        // Three requests with different TTFBs
        for delay_ms in [10, 20, 30] {
            let mut timer = metrics.start_request();
            thread::sleep(Duration::from_millis(delay_ms));
            timer.record_first_byte();
            timer.finish();
        }

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.ttfb_sample_count, 3);
        // Average should be roughly (10+20+30)/3 = 20ms
        assert!(snapshot.ttfb_avg_ms >= 15 && snapshot.ttfb_avg_ms <= 35);
    }

    #[test]
    fn test_ttfb_min_max() {
        let metrics = ProviderMetrics::new("test");

        // First: 20ms TTFB
        let mut timer = metrics.start_request();
        thread::sleep(Duration::from_millis(20));
        timer.record_first_byte();
        timer.finish();

        // Second: 5ms TTFB (new min)
        let mut timer = metrics.start_request();
        thread::sleep(Duration::from_millis(5));
        timer.record_first_byte();
        timer.finish();

        // Third: 50ms TTFB (new max)
        let mut timer = metrics.start_request();
        thread::sleep(Duration::from_millis(50));
        timer.record_first_byte();
        timer.finish();

        let snapshot = metrics.snapshot();
        assert!(snapshot.ttfb_min_ms >= 5 && snapshot.ttfb_min_ms <= 10);
        assert!(snapshot.ttfb_max_ms >= 45 && snapshot.ttfb_max_ms <= 60);
    }

    #[test]
    fn test_ttfb_not_recorded_without_first_byte() {
        let metrics = ProviderMetrics::new("test");

        let timer = metrics.start_request();
        // Never call record_first_byte()
        timer.finish();

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.ttfb_sample_count, 0);
        assert_eq!(snapshot.ttfb_avg_ms, 0);
    }

    #[test]
    fn test_first_byte_only_recorded_once() {
        let metrics = ProviderMetrics::new("test");

        let mut timer = metrics.start_request();
        thread::sleep(Duration::from_millis(5));
        timer.record_first_byte();
        thread::sleep(Duration::from_millis(20));
        timer.record_first_byte(); // Should be ignored
        timer.finish();

        let snapshot = metrics.snapshot();
        // TTFB should be ~5ms, not ~25ms
        assert!(snapshot.ttfb_avg_ms >= 5 && snapshot.ttfb_avg_ms < 20);
    }

    // -------------------------------------------------------------------------
    // Processing Time Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_processing_time() {
        let metrics = ProviderMetrics::new("test");

        let timer = metrics.start_request();
        thread::sleep(Duration::from_millis(20));
        timer.finish();

        let snapshot = metrics.snapshot();
        assert!(snapshot.processing_avg_ms >= 20 && snapshot.processing_avg_ms <= 40);
    }

    // -------------------------------------------------------------------------
    // Special Error Types Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_timeout_recording() {
        let metrics = ProviderMetrics::new("test");

        metrics.record_timeout();
        metrics.record_timeout();

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.timeout_count, 2);
        assert_eq!(snapshot.error_count, 2); // Timeouts also count as errors
    }

    #[test]
    fn test_rate_limit_recording() {
        let metrics = ProviderMetrics::new("test");

        metrics.record_rate_limit();

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.rate_limit_count, 1);
        assert_eq!(snapshot.error_count, 1); // Rate limits also count as errors
    }

    // -------------------------------------------------------------------------
    // Timer Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_timer_has_first_byte() {
        let metrics = ProviderMetrics::new("test");

        let mut timer = metrics.start_request();
        assert!(!timer.has_first_byte());

        timer.record_first_byte();
        assert!(timer.has_first_byte());
    }

    #[test]
    fn test_timer_elapsed_ms() {
        let metrics = ProviderMetrics::new("test");

        let timer = metrics.start_request();
        thread::sleep(Duration::from_millis(10));

        let elapsed = timer.elapsed_ms();
        assert!(elapsed >= 10 && elapsed < 30);
    }

    #[test]
    fn test_timer_ttfb_ms() {
        let metrics = ProviderMetrics::new("test");

        let mut timer = metrics.start_request();
        assert!(timer.ttfb_ms().is_none());

        thread::sleep(Duration::from_millis(10));
        timer.record_first_byte();

        let ttfb = timer.ttfb_ms().unwrap();
        assert!(ttfb >= 10 && ttfb < 30);
    }

    #[test]
    fn test_timer_auto_finish_on_drop() {
        let metrics = ProviderMetrics::new("test");

        {
            let mut timer = metrics.start_request();
            timer.record_first_byte();
            // Timer dropped without explicit finish()
        }

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.request_count, 1);
        assert_eq!(snapshot.success_count, 1); // Auto-finished as success
        assert_eq!(snapshot.ttfb_sample_count, 1);
    }

    #[test]
    fn test_timer_double_finish_ignored() {
        let metrics = ProviderMetrics::new("test");

        let mut timer = metrics.start_request();
        timer.record_first_byte();

        // Manually finish
        timer.do_finish(true);

        // Finish again (should be ignored)
        timer.do_finish(true);

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.success_count, 1); // Only counted once
        assert_eq!(snapshot.ttfb_sample_count, 1);
    }

    // -------------------------------------------------------------------------
    // Reset Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_reset() {
        let metrics = ProviderMetrics::new("test");

        // Generate some metrics
        for _ in 0..5 {
            let mut timer = metrics.start_request();
            timer.record_first_byte();
            timer.finish();
        }
        metrics.record_error();

        // Verify metrics exist
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.request_count, 5);
        assert!(snapshot.ttfb_sample_count > 0);

        // Reset
        metrics.reset();

        // Verify all cleared
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.request_count, 0);
        assert_eq!(snapshot.success_count, 0);
        assert_eq!(snapshot.error_count, 0);
        assert_eq!(snapshot.ttfb_sample_count, 0);
    }

    // -------------------------------------------------------------------------
    // Snapshot Rate Calculations Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_success_rate() {
        let metrics = ProviderMetrics::new("test");

        for _ in 0..8 {
            metrics.start_request().finish();
        }
        for _ in 0..2 {
            metrics.start_request().finish_with_error();
        }

        let snapshot = metrics.snapshot();
        assert!((snapshot.success_rate() - 0.8).abs() < 0.01);
        assert!((snapshot.error_rate() - 0.2).abs() < 0.01);
    }

    #[test]
    fn test_success_rate_no_requests() {
        let metrics = ProviderMetrics::new("test");
        let snapshot = metrics.snapshot();

        assert_eq!(snapshot.success_rate(), 1.0);
        assert_eq!(snapshot.error_rate(), 0.0);
    }

    // -------------------------------------------------------------------------
    // Thread Safety Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_concurrent_requests() {
        let metrics = Arc::new(ProviderMetrics::new("test"));
        let mut handles = vec![];

        for _ in 0..10 {
            let m = metrics.clone();
            handles.push(thread::spawn(move || {
                for _ in 0..100 {
                    let mut timer = m.start_request();
                    timer.record_first_byte();
                    timer.finish();
                }
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.request_count, 1000);
        assert_eq!(snapshot.success_count, 1000);
        assert_eq!(snapshot.ttfb_sample_count, 1000);
    }

    #[test]
    fn test_concurrent_min_max_updates() {
        let metrics = Arc::new(ProviderMetrics::new("test"));
        let mut handles = vec![];

        // Multiple threads updating min/max concurrently
        for delay in [5, 10, 15, 20, 25] {
            let m = metrics.clone();
            handles.push(thread::spawn(move || {
                for _ in 0..20 {
                    let mut timer = m.start_request();
                    thread::sleep(Duration::from_millis(delay));
                    timer.record_first_byte();
                    timer.finish();
                }
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        let snapshot = metrics.snapshot();
        // Min should be ~5ms, max should be ~25ms
        assert!(snapshot.ttfb_min_ms >= 5 && snapshot.ttfb_min_ms <= 15);
        assert!(snapshot.ttfb_max_ms >= 20 && snapshot.ttfb_max_ms <= 40);
    }

    // -------------------------------------------------------------------------
    // Edge Cases
    // -------------------------------------------------------------------------

    #[test]
    fn test_very_fast_request() {
        let metrics = ProviderMetrics::new("test");

        let mut timer = metrics.start_request();
        timer.record_first_byte();
        timer.finish();

        // Should handle sub-millisecond times
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.ttfb_sample_count, 1);
        // TTFB might be 0ms due to rounding
        assert!(snapshot.ttfb_avg_ms <= 5);
    }

    #[test]
    fn test_empty_provider_name() {
        let metrics = ProviderMetrics::new("");
        assert_eq!(metrics.provider_name(), "");
    }

    #[test]
    fn test_unicode_provider_name() {
        let metrics = ProviderMetrics::new("провайдер_日本語");
        assert_eq!(metrics.provider_name(), "провайдер_日本語");
    }
}
