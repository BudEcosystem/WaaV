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

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

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
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .subsec_nanos();
    nanos
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
        Self { min_ms: 30, max_ms: 150, p50_ms: 50, p99_ms: 120 }
    }

    /// Deepgram TTS WebSocket
    pub fn deepgram_tts() -> Self {
        Self { min_ms: 50, max_ms: 200, p50_ms: 80, p99_ms: 180 }
    }

    /// ElevenLabs TTS HTTP
    pub fn elevenlabs_tts() -> Self {
        Self { min_ms: 100, max_ms: 400, p50_ms: 180, p99_ms: 350 }
    }

    /// Google STT gRPC streaming
    pub fn google_stt() -> Self {
        Self { min_ms: 40, max_ms: 200, p50_ms: 60, p99_ms: 150 }
    }

    /// Google TTS gRPC
    pub fn google_tts() -> Self {
        Self { min_ms: 80, max_ms: 300, p50_ms: 120, p99_ms: 250 }
    }

    /// OpenAI Realtime WebSocket
    pub fn openai_realtime() -> Self {
        Self { min_ms: 100, max_ms: 500, p50_ms: 200, p99_ms: 450 }
    }

    /// Cartesia TTS WebSocket
    pub fn cartesia_tts() -> Self {
        Self { min_ms: 60, max_ms: 250, p50_ms: 100, p99_ms: 220 }
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
            failure_rate: 0.05,     // 5% failures
            timeout_rate: 0.03,     // 3% timeouts
            drop_rate: 0.02,        // 2% drops
            rate_limit_rate: 0.05,  // 5% rate limits
            slow_rate: 0.1,         // 10% slow responses
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
        self.total_latency_ms.fetch_add(latency_ms, Ordering::Relaxed);
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

pub mod http_mock;
pub mod websocket_mock;
pub mod grpc_mock;
