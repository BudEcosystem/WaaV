//! User-to-Bot latency tracking observer.
//!
//! This module provides latency measurement from when the user stops speaking
//! (speech_final or is_final) to when the bot starts responding (first TTS chunk).
//!
//! # Metrics Provided
//!
//! - Average latency (rolling)
//! - P50 latency
//! - P99 latency
//! - Sample count
//!
//! # Example
//!
//! ```rust
//! let observer = UserBotLatencyObserver::new(1000); // Keep 1000 samples
//! let metrics = observer.get_metrics();
//! println!("Average latency: {}ms", metrics.avg_ms);
//! ```

use crate::core::observability::observer::VoiceObserver;
use crate::core::stt::STTResult;
use crate::core::tts::AudioData;
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

// =============================================================================
// Monotonic Time Utilities
// =============================================================================

/// Get monotonic time in nanoseconds since process start.
///
/// Uses a single static Instant to provide consistent timing across all
/// components without system clock jumps.
#[inline]
pub fn now_monotonic_ns() -> u64 {
    use std::sync::OnceLock;
    static START: OnceLock<Instant> = OnceLock::new();
    let start = START.get_or_init(Instant::now);
    start.elapsed().as_nanos() as u64
}

// =============================================================================
// Latency Metrics
// =============================================================================

/// Snapshot of latency metrics.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LatencyMetrics {
    /// Average latency in milliseconds
    pub avg_ms: u32,
    /// 50th percentile (median) latency in milliseconds
    pub p50_ms: u32,
    /// 99th percentile latency in milliseconds
    pub p99_ms: u32,
    /// Number of latency samples collected
    pub sample_count: u64,
    /// Minimum latency observed in milliseconds
    pub min_ms: u32,
    /// Maximum latency observed in milliseconds
    pub max_ms: u32,
}

// =============================================================================
// User-Bot Latency Observer
// =============================================================================

/// Observer that tracks user-to-bot response latency.
///
/// Latency is measured from when the user stops speaking (is_final or
/// is_speech_final STT result) to when the bot starts responding
/// (first TTS chunk with TTFB).
///
/// # Thread Safety
///
/// This observer is thread-safe and can be shared across threads via `Arc`.
/// It uses atomic operations for hot-path timing and a mutex for the
/// rolling window (only accessed during metrics calculation).
pub struct UserBotLatencyObserver {
    /// Timestamp when user stopped speaking (nanoseconds since process start)
    user_stopped_ns: AtomicU64,

    /// Rolling window of latency samples (nanoseconds)
    latencies_ns: Mutex<VecDeque<u64>>,

    /// Maximum number of samples to keep
    max_samples: usize,

    /// Running sum for efficient average calculation (nanoseconds)
    sum_ns: AtomicU64,

    /// Total count of samples (may exceed max_samples due to rolling)
    total_count: AtomicU64,

    /// Minimum latency observed (nanoseconds)
    min_ns: AtomicU64,

    /// Maximum latency observed (nanoseconds)
    max_ns: AtomicU64,
}

impl UserBotLatencyObserver {
    /// Create a new latency observer with the specified sample window size.
    ///
    /// # Arguments
    /// * `max_samples` - Maximum number of latency samples to keep for percentile calculation
    pub fn new(max_samples: usize) -> Self {
        Self {
            user_stopped_ns: AtomicU64::new(0),
            latencies_ns: Mutex::new(VecDeque::with_capacity(max_samples)),
            max_samples,
            sum_ns: AtomicU64::new(0),
            total_count: AtomicU64::new(0),
            min_ns: AtomicU64::new(u64::MAX),
            max_ns: AtomicU64::new(0),
        }
    }

    /// Get current latency metrics.
    ///
    /// This method acquires a lock on the sample window to calculate percentiles.
    /// Call sparingly (e.g., for health checks, not per-request).
    pub fn get_metrics(&self) -> LatencyMetrics {
        let latencies = self.latencies_ns.lock();
        let count = self.total_count.load(Ordering::Relaxed);

        if count == 0 || latencies.is_empty() {
            return LatencyMetrics::default();
        }

        // Calculate average
        let sum = self.sum_ns.load(Ordering::Relaxed);
        let window_count = latencies.len() as u64;
        let avg_ns = sum / window_count;

        // Calculate percentiles from sorted copy
        let mut sorted: Vec<u64> = latencies.iter().copied().collect();
        sorted.sort_unstable();

        let p50_idx = sorted.len() / 2;
        let p99_idx = ((sorted.len() as f64 * 0.99) as usize).min(sorted.len().saturating_sub(1));

        let p50_ns = sorted.get(p50_idx).copied().unwrap_or(0);
        let p99_ns = sorted.get(p99_idx).copied().unwrap_or(0);

        let min_ns = self.min_ns.load(Ordering::Relaxed);
        let max_ns = self.max_ns.load(Ordering::Relaxed);

        LatencyMetrics {
            avg_ms: (avg_ns / 1_000_000) as u32,
            p50_ms: (p50_ns / 1_000_000) as u32,
            p99_ms: (p99_ns / 1_000_000) as u32,
            sample_count: count,
            min_ms: if min_ns == u64::MAX {
                0
            } else {
                (min_ns / 1_000_000) as u32
            },
            max_ms: (max_ns / 1_000_000) as u32,
        }
    }

    /// Record a latency sample manually (for testing or custom integration).
    ///
    /// # Arguments
    /// * `latency_ns` - Latency in nanoseconds
    pub fn record_latency(&self, latency_ns: u64) {
        let mut latencies = self.latencies_ns.lock();

        // Evict oldest if at capacity
        if latencies.len() >= self.max_samples {
            if let Some(old) = latencies.pop_front() {
                self.sum_ns.fetch_sub(old, Ordering::Relaxed);
            }
        }

        // Add new sample
        latencies.push_back(latency_ns);
        self.sum_ns.fetch_add(latency_ns, Ordering::Relaxed);
        self.total_count.fetch_add(1, Ordering::Relaxed);

        // Update min/max using compare-exchange loops
        loop {
            let current_min = self.min_ns.load(Ordering::Relaxed);
            if latency_ns >= current_min {
                break;
            }
            if self
                .min_ns
                .compare_exchange_weak(current_min, latency_ns, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                break;
            }
        }

        loop {
            let current_max = self.max_ns.load(Ordering::Relaxed);
            if latency_ns <= current_max {
                break;
            }
            if self
                .max_ns
                .compare_exchange_weak(current_max, latency_ns, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                break;
            }
        }
    }

    /// Reset all metrics to initial state.
    pub fn reset(&self) {
        let mut latencies = self.latencies_ns.lock();
        latencies.clear();
        self.user_stopped_ns.store(0, Ordering::Relaxed);
        self.sum_ns.store(0, Ordering::Relaxed);
        self.total_count.store(0, Ordering::Relaxed);
        self.min_ns.store(u64::MAX, Ordering::Relaxed);
        self.max_ns.store(0, Ordering::Relaxed);
    }

    /// Get the timestamp when user stopped speaking (for testing).
    pub fn get_user_stopped_ns(&self) -> u64 {
        self.user_stopped_ns.load(Ordering::Acquire)
    }
}

impl VoiceObserver for UserBotLatencyObserver {
    fn on_stt_result(&self, result: &STTResult, _latency_ns: u64) {
        // Record when user stopped speaking (speech_final or is_final)
        if result.is_speech_final || result.is_final {
            let now_ns = now_monotonic_ns();
            self.user_stopped_ns.store(now_ns, Ordering::Release);
        }
    }

    fn on_tts_chunk(&self, _chunk: &AudioData, ttfb_ns: Option<u64>) {
        // On FIRST TTS chunk (indicated by ttfb_ns being Some), calculate latency
        if ttfb_ns.is_some() {
            let user_stopped = self.user_stopped_ns.load(Ordering::Acquire);
            if user_stopped > 0 {
                let now_ns = now_monotonic_ns();
                let latency_ns = now_ns.saturating_sub(user_stopped);

                // Record the latency sample
                self.record_latency(latency_ns);

                // Reset user_stopped for next interaction
                self.user_stopped_ns.store(0, Ordering::Release);
            }
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // Construction Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_observer_creation() {
        let observer = UserBotLatencyObserver::new(100);
        let metrics = observer.get_metrics();

        assert_eq!(metrics.avg_ms, 0);
        assert_eq!(metrics.p50_ms, 0);
        assert_eq!(metrics.p99_ms, 0);
        assert_eq!(metrics.sample_count, 0);
        assert_eq!(metrics.min_ms, 0);
        assert_eq!(metrics.max_ms, 0);
    }

    #[test]
    fn test_observer_with_different_window_sizes() {
        for size in [1, 10, 100, 1000] {
            let observer = UserBotLatencyObserver::new(size);
            assert_eq!(observer.max_samples, size);
        }
    }

    // -------------------------------------------------------------------------
    // Manual Recording Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_record_single_latency() {
        let observer = UserBotLatencyObserver::new(100);

        observer.record_latency(10_000_000); // 10ms

        let metrics = observer.get_metrics();
        assert_eq!(metrics.avg_ms, 10);
        assert_eq!(metrics.sample_count, 1);
        assert_eq!(metrics.min_ms, 10);
        assert_eq!(metrics.max_ms, 10);
    }

    #[test]
    fn test_record_multiple_latencies() {
        let observer = UserBotLatencyObserver::new(100);

        observer.record_latency(10_000_000); // 10ms
        observer.record_latency(20_000_000); // 20ms
        observer.record_latency(30_000_000); // 30ms

        let metrics = observer.get_metrics();
        assert_eq!(metrics.avg_ms, 20); // (10+20+30)/3 = 20
        assert_eq!(metrics.sample_count, 3);
        assert_eq!(metrics.min_ms, 10);
        assert_eq!(metrics.max_ms, 30);
    }

    #[test]
    fn test_rolling_window_eviction() {
        let observer = UserBotLatencyObserver::new(3); // Small window

        observer.record_latency(10_000_000); // 10ms
        observer.record_latency(20_000_000); // 20ms
        observer.record_latency(30_000_000); // 30ms

        let metrics = observer.get_metrics();
        assert_eq!(metrics.avg_ms, 20); // Window: [10, 20, 30]

        // Add 4th sample, evicts oldest
        observer.record_latency(40_000_000); // 40ms

        let metrics = observer.get_metrics();
        // Window: [20, 30, 40], avg = 30
        assert_eq!(metrics.avg_ms, 30);
        assert_eq!(metrics.sample_count, 4); // Total count keeps increasing
    }

    // -------------------------------------------------------------------------
    // Percentile Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_p50_calculation() {
        let observer = UserBotLatencyObserver::new(100);

        // Add samples: 1, 2, 3, 4, 5 ms
        for i in 1..=5 {
            observer.record_latency(i * 1_000_000);
        }

        let metrics = observer.get_metrics();
        assert_eq!(metrics.p50_ms, 3); // Median of [1,2,3,4,5] is 3
    }

    #[test]
    fn test_p99_calculation() {
        let observer = UserBotLatencyObserver::new(100);

        // Add 100 samples: 1ms to 100ms
        for i in 1..=100 {
            observer.record_latency(i * 1_000_000);
        }

        let metrics = observer.get_metrics();
        // p99 index = 99 (0-indexed), value = 100ms
        assert!(metrics.p99_ms >= 99);
    }

    #[test]
    fn test_percentiles_with_outliers() {
        let observer = UserBotLatencyObserver::new(100);

        // Add mostly low latencies
        for _ in 0..99 {
            observer.record_latency(10_000_000); // 10ms
        }
        // Add one outlier
        observer.record_latency(1000_000_000); // 1000ms

        let metrics = observer.get_metrics();
        assert_eq!(metrics.p50_ms, 10); // Most values are 10ms
        // p99 should capture the outlier region
        assert!(metrics.p99_ms >= 10);
        assert_eq!(metrics.max_ms, 1000);
    }

    // -------------------------------------------------------------------------
    // Min/Max Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_min_max_tracking() {
        let observer = UserBotLatencyObserver::new(100);

        observer.record_latency(50_000_000); // 50ms
        observer.record_latency(10_000_000); // 10ms
        observer.record_latency(100_000_000); // 100ms
        observer.record_latency(25_000_000); // 25ms

        let metrics = observer.get_metrics();
        assert_eq!(metrics.min_ms, 10);
        assert_eq!(metrics.max_ms, 100);
    }

    #[test]
    fn test_min_max_concurrent_updates() {
        use std::sync::Arc;
        use std::thread;

        let observer = Arc::new(UserBotLatencyObserver::new(10000));
        let mut handles = vec![];

        for _ in 0..10 {
            let obs = observer.clone();
            handles.push(thread::spawn(move || {
                for i in 1..=100 {
                    obs.record_latency(i * 1_000_000);
                }
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        let metrics = observer.get_metrics();
        assert_eq!(metrics.min_ms, 1); // 1ms from all threads
        assert_eq!(metrics.max_ms, 100); // 100ms from all threads
    }

    // -------------------------------------------------------------------------
    // Reset Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_reset() {
        let observer = UserBotLatencyObserver::new(100);

        observer.record_latency(10_000_000);
        observer.record_latency(20_000_000);

        observer.reset();

        let metrics = observer.get_metrics();
        assert_eq!(metrics.sample_count, 0);
        assert_eq!(metrics.avg_ms, 0);
    }

    // -------------------------------------------------------------------------
    // VoiceObserver Integration Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_stt_result_records_user_stopped_time() {
        let observer = UserBotLatencyObserver::new(100);

        // Non-final result should NOT record time
        let interim_result = STTResult::new("hello".to_string(), false, false, 0.8);
        observer.on_stt_result(&interim_result, 0);
        assert_eq!(observer.get_user_stopped_ns(), 0);

        // is_final should record time
        let final_result = STTResult::new("hello world".to_string(), true, false, 0.95);
        observer.on_stt_result(&final_result, 0);
        assert!(observer.get_user_stopped_ns() > 0);
    }

    #[test]
    fn test_speech_final_records_user_stopped_time() {
        let observer = UserBotLatencyObserver::new(100);

        let speech_final_result = STTResult::new("hello world".to_string(), false, true, 0.95);
        observer.on_stt_result(&speech_final_result, 0);
        assert!(observer.get_user_stopped_ns() > 0);
    }

    #[test]
    fn test_tts_chunk_without_ttfb_does_not_record() {
        let observer = UserBotLatencyObserver::new(100);

        // Set user stopped time manually
        observer.user_stopped_ns.store(1_000_000, Ordering::Release);

        let audio = AudioData {
            data: vec![0u8; 100],
            sample_rate: 24000,
            format: "pcm".to_string(),
            duration_ms: Some(10),
        };

        // Non-first chunk (no TTFB)
        observer.on_tts_chunk(&audio, None);

        // No latency should be recorded
        assert_eq!(observer.get_metrics().sample_count, 0);
        // user_stopped should still be set
        assert_eq!(observer.get_user_stopped_ns(), 1_000_000);
    }

    #[test]
    fn test_tts_chunk_with_ttfb_records_latency() {
        let observer = UserBotLatencyObserver::new(100);

        // Simulate STT final result
        let final_result = STTResult::new("hello".to_string(), true, false, 0.95);
        observer.on_stt_result(&final_result, 0);

        // Small delay to ensure measurable latency
        std::thread::sleep(std::time::Duration::from_millis(5));

        // Simulate first TTS chunk (with TTFB)
        let audio = AudioData {
            data: vec![0u8; 100],
            sample_rate: 24000,
            format: "pcm".to_string(),
            duration_ms: Some(10),
        };
        observer.on_tts_chunk(&audio, Some(1_000_000)); // TTFB present

        // Latency should be recorded
        let metrics = observer.get_metrics();
        assert_eq!(metrics.sample_count, 1);
        assert!(metrics.avg_ms >= 5); // At least 5ms from sleep

        // user_stopped should be reset
        assert_eq!(observer.get_user_stopped_ns(), 0);
    }

    #[test]
    fn test_tts_chunk_without_prior_stt_does_not_record() {
        let observer = UserBotLatencyObserver::new(100);

        // No prior STT result (user_stopped_ns is 0)
        let audio = AudioData {
            data: vec![0u8; 100],
            sample_rate: 24000,
            format: "pcm".to_string(),
            duration_ms: Some(10),
        };
        observer.on_tts_chunk(&audio, Some(1_000_000));

        // No latency should be recorded since there was no prior user speech
        assert_eq!(observer.get_metrics().sample_count, 0);
    }

    // -------------------------------------------------------------------------
    // Full Interaction Sequence Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_full_interaction_sequence() {
        let observer = UserBotLatencyObserver::new(100);

        // Sequence: User speaks -> STT result -> TTS response
        for _ in 0..10 {
            // User speaks, STT processes
            let result = STTResult::new("test".to_string(), true, false, 0.95);
            observer.on_stt_result(&result, 0);

            // Brief processing delay
            std::thread::sleep(std::time::Duration::from_millis(1));

            // Bot responds with first TTS chunk
            let audio = AudioData {
                data: vec![0u8; 100],
                sample_rate: 24000,
                format: "pcm".to_string(),
                duration_ms: Some(10),
            };
            observer.on_tts_chunk(&audio, Some(500_000));

            // Subsequent chunks (no TTFB)
            observer.on_tts_chunk(&audio, None);
            observer.on_tts_chunk(&audio, None);
        }

        let metrics = observer.get_metrics();
        assert_eq!(metrics.sample_count, 10);
        assert!(metrics.avg_ms >= 1); // At least 1ms per interaction
    }

    // -------------------------------------------------------------------------
    // Edge Cases
    // -------------------------------------------------------------------------

    #[test]
    fn test_zero_latency() {
        let observer = UserBotLatencyObserver::new(100);
        observer.record_latency(0);

        let metrics = observer.get_metrics();
        assert_eq!(metrics.avg_ms, 0);
        assert_eq!(metrics.min_ms, 0);
    }

    #[test]
    fn test_very_high_latency() {
        let observer = UserBotLatencyObserver::new(100);
        observer.record_latency(60_000_000_000); // 60 seconds

        let metrics = observer.get_metrics();
        assert_eq!(metrics.avg_ms, 60000);
        assert_eq!(metrics.max_ms, 60000);
    }

    #[test]
    fn test_window_size_one() {
        let observer = UserBotLatencyObserver::new(1);

        observer.record_latency(10_000_000);
        assert_eq!(observer.get_metrics().avg_ms, 10);

        observer.record_latency(20_000_000);
        assert_eq!(observer.get_metrics().avg_ms, 20); // Only keeps last

        observer.record_latency(30_000_000);
        assert_eq!(observer.get_metrics().avg_ms, 30);
    }

    // -------------------------------------------------------------------------
    // Monotonic Time Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_monotonic_time_increases() {
        let t1 = now_monotonic_ns();
        std::thread::sleep(std::time::Duration::from_millis(1));
        let t2 = now_monotonic_ns();

        assert!(t2 > t1);
    }

    #[test]
    fn test_monotonic_time_consistent() {
        // Multiple calls should return consistent, increasing values
        let times: Vec<u64> = (0..100).map(|_| now_monotonic_ns()).collect();

        for i in 1..times.len() {
            assert!(times[i] >= times[i - 1]);
        }
    }
}
