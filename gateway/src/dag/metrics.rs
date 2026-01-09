//! DAG execution metrics collection
//!
//! This module provides metrics collection for DAG execution including
//! latency histograms, throughput counters, and error tracking.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use parking_lot::RwLock;

/// Metrics collector for DAG execution
///
/// Thread-safe metrics collection using atomic counters and lock-free data structures
/// where possible. Uses RwLock for histogram data that requires more complex updates.
#[derive(Debug)]
pub struct DAGMetrics {
    /// Total executions count
    total_executions: AtomicU64,

    /// Successful executions count
    successful_executions: AtomicU64,

    /// Failed executions count
    failed_executions: AtomicU64,

    /// Cancelled executions count
    cancelled_executions: AtomicU64,

    /// Total execution time (microseconds)
    total_execution_time_us: AtomicU64,

    /// Per-node metrics
    node_metrics: RwLock<HashMap<String, NodeMetrics>>,

    /// Per-endpoint metrics
    endpoint_metrics: RwLock<HashMap<String, EndpointMetrics>>,

    /// Latency histogram buckets (microseconds)
    /// Buckets: <1ms, <5ms, <10ms, <50ms, <100ms, <500ms, <1s, >1s
    latency_histogram: [AtomicU64; 8],

    /// When metrics collection started
    start_time: Instant,
}

impl DAGMetrics {
    /// Create a new metrics collector
    pub fn new() -> Self {
        Self {
            total_executions: AtomicU64::new(0),
            successful_executions: AtomicU64::new(0),
            failed_executions: AtomicU64::new(0),
            cancelled_executions: AtomicU64::new(0),
            total_execution_time_us: AtomicU64::new(0),
            node_metrics: RwLock::new(HashMap::new()),
            endpoint_metrics: RwLock::new(HashMap::new()),
            latency_histogram: Default::default(),
            start_time: Instant::now(),
        }
    }

    /// Record a successful execution
    pub fn record_success(&self, duration: Duration) {
        self.total_executions.fetch_add(1, Ordering::Relaxed);
        self.successful_executions.fetch_add(1, Ordering::Relaxed);
        self.record_latency(duration);
    }

    /// Record a failed execution
    pub fn record_failure(&self, duration: Duration) {
        self.total_executions.fetch_add(1, Ordering::Relaxed);
        self.failed_executions.fetch_add(1, Ordering::Relaxed);
        self.record_latency(duration);
    }

    /// Record a cancelled execution
    pub fn record_cancellation(&self) {
        self.total_executions.fetch_add(1, Ordering::Relaxed);
        self.cancelled_executions.fetch_add(1, Ordering::Relaxed);
    }

    /// Record execution latency
    fn record_latency(&self, duration: Duration) {
        let us = duration.as_micros() as u64;
        self.total_execution_time_us.fetch_add(us, Ordering::Relaxed);

        // Update histogram bucket
        let bucket = match us {
            0..=999 => 0,         // <1ms
            1000..=4999 => 1,     // <5ms
            5000..=9999 => 2,     // <10ms
            10000..=49999 => 3,   // <50ms
            50000..=99999 => 4,   // <100ms
            100000..=499999 => 5, // <500ms
            500000..=999999 => 6, // <1s
            _ => 7,               // >1s
        };
        self.latency_histogram[bucket].fetch_add(1, Ordering::Relaxed);
    }

    /// Record node execution
    pub fn record_node_execution(&self, node_id: &str, duration: Duration, success: bool) {
        let mut metrics = self.node_metrics.write();
        let node = metrics.entry(node_id.to_string()).or_insert_with(NodeMetrics::new);
        node.record(duration, success);
    }

    /// Record endpoint call
    pub fn record_endpoint_call(
        &self,
        endpoint_type: &str,
        endpoint_id: &str,
        duration: Duration,
        success: bool,
    ) {
        let key = format!("{}:{}", endpoint_type, endpoint_id);
        let mut metrics = self.endpoint_metrics.write();
        let endpoint = metrics.entry(key).or_insert_with(EndpointMetrics::new);
        endpoint.record(duration, success);
    }

    /// Get total execution count
    pub fn total_executions(&self) -> u64 {
        self.total_executions.load(Ordering::Relaxed)
    }

    /// Get successful execution count
    pub fn successful_executions(&self) -> u64 {
        self.successful_executions.load(Ordering::Relaxed)
    }

    /// Get failed execution count
    pub fn failed_executions(&self) -> u64 {
        self.failed_executions.load(Ordering::Relaxed)
    }

    /// Get success rate (0.0 - 1.0)
    pub fn success_rate(&self) -> f64 {
        let total = self.total_executions();
        if total == 0 {
            return 1.0;
        }
        self.successful_executions() as f64 / total as f64
    }

    /// Get average execution time
    pub fn average_execution_time(&self) -> Duration {
        let total = self.total_executions();
        if total == 0 {
            return Duration::ZERO;
        }
        let total_us = self.total_execution_time_us.load(Ordering::Relaxed);
        Duration::from_micros(total_us / total)
    }

    /// Get executions per second (throughput)
    pub fn executions_per_second(&self) -> f64 {
        let elapsed = self.start_time.elapsed().as_secs_f64();
        if elapsed == 0.0 {
            return 0.0;
        }
        self.total_executions() as f64 / elapsed
    }

    /// Get latency percentiles
    pub fn latency_percentiles(&self) -> LatencyPercentiles {
        let buckets: Vec<u64> = self.latency_histogram
            .iter()
            .map(|a| a.load(Ordering::Relaxed))
            .collect();

        let total: u64 = buckets.iter().sum();
        if total == 0 {
            return LatencyPercentiles::default();
        }

        // Calculate percentiles from histogram
        let mut cumulative = 0u64;
        let mut p50 = Duration::ZERO;
        let mut p90 = Duration::ZERO;
        let mut p99 = Duration::ZERO;
        let mut p999 = Duration::ZERO;

        let bucket_bounds = [
            Duration::from_micros(1000),   // <1ms
            Duration::from_micros(5000),   // <5ms
            Duration::from_micros(10000),  // <10ms
            Duration::from_micros(50000),  // <50ms
            Duration::from_micros(100000), // <100ms
            Duration::from_micros(500000), // <500ms
            Duration::from_micros(1000000), // <1s
            Duration::from_secs(10),       // >1s (use 10s as max)
        ];

        for (i, &count) in buckets.iter().enumerate() {
            cumulative += count;
            let percentile = (cumulative as f64 / total as f64) * 100.0;

            if p50.is_zero() && percentile >= 50.0 {
                p50 = bucket_bounds[i];
            }
            if p90.is_zero() && percentile >= 90.0 {
                p90 = bucket_bounds[i];
            }
            if p99.is_zero() && percentile >= 99.0 {
                p99 = bucket_bounds[i];
            }
            if p999.is_zero() && percentile >= 99.9 {
                p999 = bucket_bounds[i];
            }
        }

        LatencyPercentiles { p50, p90, p99, p999 }
    }

    /// Get node metrics for a specific node
    pub fn get_node_metrics(&self, node_id: &str) -> Option<NodeMetricsSnapshot> {
        self.node_metrics.read().get(node_id).map(|m| m.snapshot())
    }

    /// Get all node metrics
    pub fn all_node_metrics(&self) -> HashMap<String, NodeMetricsSnapshot> {
        self.node_metrics
            .read()
            .iter()
            .map(|(k, v)| (k.clone(), v.snapshot()))
            .collect()
    }

    /// Get endpoint metrics
    pub fn get_endpoint_metrics(&self, key: &str) -> Option<EndpointMetricsSnapshot> {
        self.endpoint_metrics.read().get(key).map(|m| m.snapshot())
    }

    /// Get all endpoint metrics
    pub fn all_endpoint_metrics(&self) -> HashMap<String, EndpointMetricsSnapshot> {
        self.endpoint_metrics
            .read()
            .iter()
            .map(|(k, v)| (k.clone(), v.snapshot()))
            .collect()
    }

    /// Get a summary of all metrics
    pub fn summary(&self) -> MetricsSummary {
        MetricsSummary {
            total_executions: self.total_executions(),
            successful_executions: self.successful_executions(),
            failed_executions: self.failed_executions(),
            cancelled_executions: self.cancelled_executions.load(Ordering::Relaxed),
            success_rate: self.success_rate(),
            average_latency: self.average_execution_time(),
            throughput: self.executions_per_second(),
            percentiles: self.latency_percentiles(),
            uptime: self.start_time.elapsed(),
        }
    }

    /// Reset all metrics
    pub fn reset(&self) {
        self.total_executions.store(0, Ordering::Relaxed);
        self.successful_executions.store(0, Ordering::Relaxed);
        self.failed_executions.store(0, Ordering::Relaxed);
        self.cancelled_executions.store(0, Ordering::Relaxed);
        self.total_execution_time_us.store(0, Ordering::Relaxed);
        for bucket in &self.latency_histogram {
            bucket.store(0, Ordering::Relaxed);
        }
        self.node_metrics.write().clear();
        self.endpoint_metrics.write().clear();
    }
}

impl Default for DAGMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Per-node metrics
#[derive(Debug)]
struct NodeMetrics {
    executions: u64,
    successes: u64,
    failures: u64,
    total_time_us: u64,
    min_time_us: u64,
    max_time_us: u64,
}

impl NodeMetrics {
    fn new() -> Self {
        Self {
            executions: 0,
            successes: 0,
            failures: 0,
            total_time_us: 0,
            min_time_us: u64::MAX,
            max_time_us: 0,
        }
    }

    fn record(&mut self, duration: Duration, success: bool) {
        let us = duration.as_micros() as u64;
        self.executions += 1;
        if success {
            self.successes += 1;
        } else {
            self.failures += 1;
        }
        self.total_time_us += us;
        self.min_time_us = self.min_time_us.min(us);
        self.max_time_us = self.max_time_us.max(us);
    }

    fn snapshot(&self) -> NodeMetricsSnapshot {
        NodeMetricsSnapshot {
            executions: self.executions,
            successes: self.successes,
            failures: self.failures,
            average_time: if self.executions > 0 {
                Duration::from_micros(self.total_time_us / self.executions)
            } else {
                Duration::ZERO
            },
            min_time: if self.min_time_us == u64::MAX {
                Duration::ZERO
            } else {
                Duration::from_micros(self.min_time_us)
            },
            max_time: Duration::from_micros(self.max_time_us),
        }
    }
}

/// Per-endpoint metrics
#[derive(Debug)]
struct EndpointMetrics {
    calls: u64,
    successes: u64,
    failures: u64,
    total_time_us: u64,
    timeouts: u64,
}

impl EndpointMetrics {
    fn new() -> Self {
        Self {
            calls: 0,
            successes: 0,
            failures: 0,
            total_time_us: 0,
            timeouts: 0,
        }
    }

    fn record(&mut self, duration: Duration, success: bool) {
        self.calls += 1;
        if success {
            self.successes += 1;
        } else {
            self.failures += 1;
        }
        self.total_time_us += duration.as_micros() as u64;
    }

    fn snapshot(&self) -> EndpointMetricsSnapshot {
        EndpointMetricsSnapshot {
            calls: self.calls,
            successes: self.successes,
            failures: self.failures,
            timeouts: self.timeouts,
            average_time: if self.calls > 0 {
                Duration::from_micros(self.total_time_us / self.calls)
            } else {
                Duration::ZERO
            },
        }
    }
}

/// Snapshot of node metrics
#[derive(Debug, Clone)]
pub struct NodeMetricsSnapshot {
    pub executions: u64,
    pub successes: u64,
    pub failures: u64,
    pub average_time: Duration,
    pub min_time: Duration,
    pub max_time: Duration,
}

/// Snapshot of endpoint metrics
#[derive(Debug, Clone)]
pub struct EndpointMetricsSnapshot {
    pub calls: u64,
    pub successes: u64,
    pub failures: u64,
    pub timeouts: u64,
    pub average_time: Duration,
}

/// Latency percentiles
#[derive(Debug, Clone, Default)]
pub struct LatencyPercentiles {
    pub p50: Duration,
    pub p90: Duration,
    pub p99: Duration,
    pub p999: Duration,
}

/// Complete metrics summary
#[derive(Debug, Clone)]
pub struct MetricsSummary {
    pub total_executions: u64,
    pub successful_executions: u64,
    pub failed_executions: u64,
    pub cancelled_executions: u64,
    pub success_rate: f64,
    pub average_latency: Duration,
    pub throughput: f64,
    pub percentiles: LatencyPercentiles,
    pub uptime: Duration,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_creation() {
        let metrics = DAGMetrics::new();
        assert_eq!(metrics.total_executions(), 0);
        assert_eq!(metrics.success_rate(), 1.0); // No failures = 100% success
    }

    #[test]
    fn test_record_success() {
        let metrics = DAGMetrics::new();
        metrics.record_success(Duration::from_millis(5));
        metrics.record_success(Duration::from_millis(10));

        assert_eq!(metrics.total_executions(), 2);
        assert_eq!(metrics.successful_executions(), 2);
        assert_eq!(metrics.failed_executions(), 0);
        assert_eq!(metrics.success_rate(), 1.0);
    }

    #[test]
    fn test_record_failure() {
        let metrics = DAGMetrics::new();
        metrics.record_success(Duration::from_millis(5));
        metrics.record_failure(Duration::from_millis(10));

        assert_eq!(metrics.total_executions(), 2);
        assert_eq!(metrics.successful_executions(), 1);
        assert_eq!(metrics.failed_executions(), 1);
        assert_eq!(metrics.success_rate(), 0.5);
    }

    #[test]
    fn test_node_metrics() {
        let metrics = DAGMetrics::new();
        metrics.record_node_execution("stt_node", Duration::from_millis(10), true);
        metrics.record_node_execution("stt_node", Duration::from_millis(20), true);
        metrics.record_node_execution("stt_node", Duration::from_millis(5), false);

        let node = metrics.get_node_metrics("stt_node").unwrap();
        assert_eq!(node.executions, 3);
        assert_eq!(node.successes, 2);
        assert_eq!(node.failures, 1);
    }

    #[test]
    fn test_latency_histogram() {
        let metrics = DAGMetrics::new();

        // Record various latencies
        metrics.record_success(Duration::from_micros(500));   // <1ms bucket
        metrics.record_success(Duration::from_millis(3));     // <5ms bucket
        metrics.record_success(Duration::from_millis(50));    // <50ms bucket
        metrics.record_success(Duration::from_millis(500));   // <500ms bucket

        let percentiles = metrics.latency_percentiles();
        // With 4 entries, p50 should be around the 2nd bucket
        assert!(percentiles.p50 <= Duration::from_millis(50));
    }

    #[test]
    fn test_summary() {
        let metrics = DAGMetrics::new();
        metrics.record_success(Duration::from_millis(10));

        let summary = metrics.summary();
        assert_eq!(summary.total_executions, 1);
        assert_eq!(summary.successful_executions, 1);
        assert!(summary.uptime > Duration::ZERO);
    }

    #[test]
    fn test_reset() {
        let metrics = DAGMetrics::new();
        metrics.record_success(Duration::from_millis(10));
        metrics.record_node_execution("test", Duration::from_millis(5), true);

        metrics.reset();

        assert_eq!(metrics.total_executions(), 0);
        assert!(metrics.get_node_metrics("test").is_none());
    }
}
