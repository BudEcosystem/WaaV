//! Comprehensive Mock Provider Servers
//!
//! Simulates all provider connection types with realistic behavior:
//! - HTTP (ElevenLabs, OpenAI, PlayHT)
//! - WebSocket (Deepgram, Cartesia, LMNT)
//! - gRPC (Google)
//!
//! Includes chaos elements:
//! - Random latency variation
//! - Intermittent failures
//! - Connection drops
//! - Rate limiting simulation
//! - Timeout simulation

// Allow dead code in test infrastructure - these utilities may be used by future tests
#![allow(dead_code)]

use futures_util::FutureExt;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::task::JoinHandle;

/// Simple random number generator (no external crate dependency)
fn random_f64() -> f64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .subsec_nanos();
    // Mix with thread ID hash for better distribution (stable API)
    let thread_id = std::thread::current().id();
    let thread_hash = format!("{:?}", thread_id).len() as u32;
    let mixed = nanos.wrapping_mul(thread_hash.wrapping_add(12345));
    (mixed as f64) / (u32::MAX as f64)
}

fn random_u32() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .subsec_nanos()
}

/// Owned handle for background mock servers used by integration/load tests.
///
/// Dropping a raw [`JoinHandle`] detaches the task, letting a mock server keep
/// running after the test body exits. This guard makes that backstop explicit:
/// it aborts unfinished servers on drop, and it turns a top-level mock-server
/// panic into a test failure when the guard is dropped normally.
pub struct MockServerHandle {
    label: &'static str,
    handle: JoinHandle<()>,
    panicked: Arc<AtomicBool>,
    children: Arc<Mutex<Vec<JoinHandle<()>>>>,
}

impl MockServerHandle {
    fn new(
        label: &'static str,
        handle: JoinHandle<()>,
        panicked: Arc<AtomicBool>,
        children: Arc<Mutex<Vec<JoinHandle<()>>>>,
    ) -> Self {
        Self {
            label,
            handle,
            panicked,
            children,
        }
    }

    pub fn is_finished(&self) -> bool {
        self.handle.is_finished()
    }

    pub fn abort(&self) {
        self.handle.abort();
    }
}

impl Drop for MockServerHandle {
    fn drop(&mut self) {
        if !self.handle.is_finished() {
            self.handle.abort();
        }
        match self.children.lock() {
            Ok(mut children) => {
                for child in children.drain(..) {
                    if !child.is_finished() {
                        child.abort();
                    }
                }
            }
            Err(_) => {
                self.panicked.store(true, AtomicOrdering::SeqCst);
            }
        }

        if self.panicked.load(AtomicOrdering::SeqCst) {
            let msg = format!("mock server task '{}' panicked", self.label);
            if std::thread::panicking() {
                eprintln!("{msg}");
            } else {
                panic!("{msg}");
            }
        }
    }
}

#[derive(Clone)]
pub struct MockTaskScope {
    panicked: Arc<AtomicBool>,
    children: Arc<Mutex<Vec<JoinHandle<()>>>>,
}

impl MockTaskScope {
    pub fn detached() -> Self {
        Self {
            panicked: Arc::new(AtomicBool::new(false)),
            children: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn spawn<F>(&self, label: &'static str, future: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let handle = spawn_mock_task(label, Arc::clone(&self.panicked), future);
        self.children
            .lock()
            .expect("mock child task list poisoned")
            .push(handle);
    }
}

pub fn spawn_mock_server<F>(label: &'static str, future: F) -> MockServerHandle
where
    F: Future<Output = ()> + Send + 'static,
{
    let panicked = Arc::new(AtomicBool::new(false));
    let children = Arc::new(Mutex::new(Vec::new()));
    let handle = spawn_mock_task(label, Arc::clone(&panicked), future);
    MockServerHandle::new(label, handle, panicked, children)
}

pub fn spawn_mock_server_with_scope<F, Fut>(label: &'static str, make_future: F) -> MockServerHandle
where
    F: FnOnce(MockTaskScope) -> Fut,
    Fut: Future<Output = ()> + Send + 'static,
{
    let panicked = Arc::new(AtomicBool::new(false));
    let children = Arc::new(Mutex::new(Vec::new()));
    let scope = MockTaskScope {
        panicked: Arc::clone(&panicked),
        children: Arc::clone(&children),
    };
    let handle = spawn_mock_task(label, Arc::clone(&panicked), make_future(scope));
    MockServerHandle::new(label, handle, panicked, children)
}

pub fn spawn_observed_mock_task<F>(label: &'static str, future: F) -> JoinHandle<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    spawn_mock_task(label, Arc::new(AtomicBool::new(false)), future)
}

fn spawn_mock_task<F>(label: &'static str, panicked: Arc<AtomicBool>, future: F) -> JoinHandle<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    tokio::spawn(async move {
        if AssertUnwindSafe(future).catch_unwind().await.is_err() {
            panicked.store(true, AtomicOrdering::SeqCst);
            eprintln!("mock server task '{label}' panicked");
        }
    })
}

/// Realistic provider latency profiles (in milliseconds)
#[derive(Clone, Debug)]
pub struct LatencyProfile {
    pub min_ms: u64,
    pub max_ms: u64,
    pub p50_ms: u64,
    pub p99_ms: u64,
}

impl LatencyProfile {
    /// Deepgram STT WebSocket - very fast streaming
    pub fn deepgram_stt() -> Self {
        Self {
            min_ms: 30,
            max_ms: 150,
            p50_ms: 50,
            p99_ms: 120,
        }
    }

    /// Deepgram TTS WebSocket
    pub fn deepgram_tts() -> Self {
        Self {
            min_ms: 50,
            max_ms: 200,
            p50_ms: 80,
            p99_ms: 180,
        }
    }

    /// ElevenLabs TTS HTTP
    pub fn elevenlabs_tts() -> Self {
        Self {
            min_ms: 100,
            max_ms: 400,
            p50_ms: 180,
            p99_ms: 350,
        }
    }

    /// Google STT gRPC streaming
    pub fn google_stt() -> Self {
        Self {
            min_ms: 40,
            max_ms: 200,
            p50_ms: 60,
            p99_ms: 150,
        }
    }

    /// Google TTS gRPC
    pub fn google_tts() -> Self {
        Self {
            min_ms: 80,
            max_ms: 300,
            p50_ms: 120,
            p99_ms: 250,
        }
    }

    /// OpenAI Realtime WebSocket
    pub fn openai_realtime() -> Self {
        Self {
            min_ms: 100,
            max_ms: 500,
            p50_ms: 200,
            p99_ms: 450,
        }
    }

    /// Cartesia TTS WebSocket
    pub fn cartesia_tts() -> Self {
        Self {
            min_ms: 60,
            max_ms: 250,
            p50_ms: 100,
            p99_ms: 220,
        }
    }

    /// Generate a random latency based on the profile
    pub fn sample(&self) -> Duration {
        // Use exponential distribution to model real-world latency
        let r = random_f64();
        let latency = if r < 0.5 {
            // 50% of requests around p50
            self.p50_ms as f64 + (random_f64() - 0.5) * 20.0
        } else if r < 0.99 {
            // 49% between p50 and p99
            self.p50_ms as f64 + (self.p99_ms - self.p50_ms) as f64 * random_f64()
        } else {
            // 1% tail latency
            self.p99_ms as f64 + random_f64() * (self.max_ms - self.p99_ms) as f64
        };

        Duration::from_millis(latency.max(self.min_ms as f64) as u64)
    }
}

/// Chaos configuration for simulating failures
#[derive(Clone, Debug)]
pub struct ChaosConfig {
    /// Probability of request failure (0.0 - 1.0)
    pub failure_rate: f64,
    /// Probability of timeout (0.0 - 1.0)
    pub timeout_rate: f64,
    /// Probability of connection drop (0.0 - 1.0)
    pub drop_rate: f64,
    /// Probability of rate limit response (0.0 - 1.0)
    pub rate_limit_rate: f64,
    /// Probability of slow response (2x-5x normal latency)
    pub slow_rate: f64,
}

impl Default for ChaosConfig {
    fn default() -> Self {
        Self {
            failure_rate: 0.0,
            timeout_rate: 0.0,
            drop_rate: 0.0,
            rate_limit_rate: 0.0,
            slow_rate: 0.0,
        }
    }
}

impl ChaosConfig {
    /// Realistic production chaos (rare failures)
    pub fn production() -> Self {
        Self {
            failure_rate: 0.001,    // 0.1% failures
            timeout_rate: 0.002,    // 0.2% timeouts
            drop_rate: 0.0005,      // 0.05% drops
            rate_limit_rate: 0.001, // 0.1% rate limits
            slow_rate: 0.01,        // 1% slow responses
        }
    }

    /// High chaos for stress testing
    pub fn stress() -> Self {
        Self {
            failure_rate: 0.05,    // 5% failures
            timeout_rate: 0.03,    // 3% timeouts
            drop_rate: 0.02,       // 2% drops
            rate_limit_rate: 0.05, // 5% rate limits
            slow_rate: 0.1,        // 10% slow responses
        }
    }

    /// Should this request fail?
    pub fn should_fail(&self) -> bool {
        random_f64() < self.failure_rate
    }

    /// Should this request timeout?
    pub fn should_timeout(&self) -> bool {
        random_f64() < self.timeout_rate
    }

    /// Should connection be dropped?
    pub fn should_drop(&self) -> bool {
        random_f64() < self.drop_rate
    }

    /// Should return rate limit?
    pub fn should_rate_limit(&self) -> bool {
        random_f64() < self.rate_limit_rate
    }

    /// Should response be slow?
    pub fn should_slow(&self) -> bool {
        random_f64() < self.slow_rate
    }

    /// Get slow multiplier (2x-5x)
    pub fn slow_multiplier(&self) -> u32 {
        if self.should_slow() {
            random_u32() % 4 + 2 // 2-5x
        } else {
            1
        }
    }
}

/// Statistics collector for mock server
#[derive(Debug, Default)]
pub struct MockStats {
    pub total_requests: AtomicU64,
    pub successful_requests: AtomicU64,
    pub failed_requests: AtomicU64,
    pub timeout_requests: AtomicU64,
    pub dropped_requests: AtomicU64,
    pub rate_limited_requests: AtomicU64,
    pub total_latency_ms: AtomicU64,
}

impl MockStats {
    pub fn record_success(&self, latency_ms: u64) {
        self.total_requests.fetch_add(1, Ordering::Relaxed);
        self.successful_requests.fetch_add(1, Ordering::Relaxed);
        self.total_latency_ms
            .fetch_add(latency_ms, Ordering::Relaxed);
    }

    pub fn record_failure(&self) {
        self.total_requests.fetch_add(1, Ordering::Relaxed);
        self.failed_requests.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_timeout(&self) {
        self.total_requests.fetch_add(1, Ordering::Relaxed);
        self.timeout_requests.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_drop(&self) {
        self.total_requests.fetch_add(1, Ordering::Relaxed);
        self.dropped_requests.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_rate_limit(&self) {
        self.total_requests.fetch_add(1, Ordering::Relaxed);
        self.rate_limited_requests.fetch_add(1, Ordering::Relaxed);
    }

    pub fn summary(&self) -> String {
        let total = self.total_requests.load(Ordering::Relaxed);
        let success = self.successful_requests.load(Ordering::Relaxed);
        let avg_latency = if success > 0 {
            self.total_latency_ms.load(Ordering::Relaxed) / success
        } else {
            0
        };

        format!(
            "Total: {}, Success: {}, Failed: {}, Timeout: {}, Dropped: {}, RateLimited: {}, AvgLatency: {}ms",
            total,
            success,
            self.failed_requests.load(Ordering::Relaxed),
            self.timeout_requests.load(Ordering::Relaxed),
            self.dropped_requests.load(Ordering::Relaxed),
            self.rate_limited_requests.load(Ordering::Relaxed),
            avg_latency
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_server_handle_reports_child_task_panic() {
        let handle = spawn_mock_server_with_scope("parent_mock", |scope| async move {
            scope.spawn("child_mock", async {
                panic!("injected mock child panic");
            });
            tokio::time::sleep(Duration::from_millis(50)).await;
        });

        tokio::time::sleep(Duration::from_millis(100)).await;
        let dropped = std::panic::catch_unwind(AssertUnwindSafe(|| drop(handle)));

        assert!(
            dropped.is_err(),
            "dropping the parent mock handle must report child task panics"
        );
    }
}

pub mod grpc_mock;
pub mod http_mock;
pub mod websocket_mock;
