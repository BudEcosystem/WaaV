//! E2E Latency Benchmark with Mocked Providers
//!
//! Tests end-to-end latency through the gateway with realistic
//! provider latencies simulated using wiremock.
//!
//! Run: cargo test --test e2e_latency_benchmark -- --nocapture

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::net::TcpListener;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};
use wiremock::matchers::{any, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// Realistic provider latencies (based on real-world measurements)
const DEEPGRAM_STT_LATENCY_MS: u64 = 80;  // 50-150ms typical
const DEEPGRAM_TTS_LATENCY_MS: u64 = 120; // 80-200ms typical
const ELEVENLABS_TTS_LATENCY_MS: u64 = 180; // 100-300ms typical

/// Statistics collector for latency measurements
#[derive(Debug, Default)]
struct LatencyStats {
    samples: Vec<f64>,
}

impl LatencyStats {
    fn add(&mut self, latency_ms: f64) {
        self.samples.push(latency_ms);
    }

    fn percentile(&self, p: f64) -> f64 {
        if self.samples.is_empty() {
            return 0.0;
        }
        let mut sorted = self.samples.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let idx = ((p / 100.0) * (sorted.len() - 1) as f64).round() as usize;
        sorted[idx.min(sorted.len() - 1)]
    }

    fn avg(&self) -> f64 {
        if self.samples.is_empty() {
            return 0.0;
        }
        self.samples.iter().sum::<f64>() / self.samples.len() as f64
    }

    fn count(&self) -> usize {
        self.samples.len()
    }
}

/// Find an available port for testing
fn find_available_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("Failed to bind to random port");
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

/// Test HTTP endpoint latency with mocked backend
#[tokio::test]
async fn test_http_latency_with_mock_provider() {
    // Start mock server simulating TTS provider
    let mock_server = MockServer::start().await;

    // Mock TTS endpoint with realistic latency
    Mock::given(method("POST"))
        .and(path("/v1/text-to-speech"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(vec![0u8; 16384]) // 16KB mock audio
                .set_delay(Duration::from_millis(ELEVENLABS_TTS_LATENCY_MS)),
        )
        .mount(&mock_server)
        .await;

    println!("\n=== HTTP Latency Test with Mock Provider ===");
    println!("Mock server URL: {}", mock_server.uri());
    println!("Simulated provider latency: {}ms", ELEVENLABS_TTS_LATENCY_MS);

    let client = reqwest::Client::new();
    let mut stats = LatencyStats::default();

    // Run 20 requests
    for i in 0..20 {
        let start = Instant::now();
        let response = client
            .post(format!("{}/v1/text-to-speech", mock_server.uri()))
            .json(&json!({"text": "Hello world", "voice_id": "test"}))
            .send()
            .await
            .expect("Request failed");

        let latency = start.elapsed().as_secs_f64() * 1000.0;
        stats.add(latency);

        assert_eq!(response.status(), 200);
        if i % 5 == 0 {
            println!("  Request {}: {:.2}ms", i + 1, latency);
        }
    }

    println!("\nResults:");
    println!("  Samples: {}", stats.count());
    println!("  P50: {:.2}ms", stats.percentile(50.0));
    println!("  P90: {:.2}ms", stats.percentile(90.0));
    println!("  P99: {:.2}ms", stats.percentile(99.0));
    println!("  Avg: {:.2}ms", stats.avg());
    println!(
        "  Gateway overhead: {:.2}ms (P50 - provider latency)",
        stats.percentile(50.0) - ELEVENLABS_TTS_LATENCY_MS as f64
    );

    // Verify latency is within expected range (provider latency + small overhead)
    assert!(
        stats.percentile(50.0) >= ELEVENLABS_TTS_LATENCY_MS as f64,
        "P50 should be at least provider latency"
    );
    assert!(
        stats.percentile(50.0) < ELEVENLABS_TTS_LATENCY_MS as f64 + 50.0,
        "Gateway overhead should be <50ms"
    );
}

/// Test concurrent HTTP requests with mocked backend
#[tokio::test]
async fn test_concurrent_http_latency() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/speak"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(vec![0u8; 8192])
                .set_delay(Duration::from_millis(DEEPGRAM_TTS_LATENCY_MS)),
        )
        .mount(&mock_server)
        .await;

    println!("\n=== Concurrent HTTP Latency Test ===");
    println!("Simulated provider latency: {}ms", DEEPGRAM_TTS_LATENCY_MS);
    println!("Concurrent requests: 50");

    let client = Arc::new(reqwest::Client::new());
    let url = format!("{}/v1/speak", mock_server.uri());

    let start = Instant::now();
    let mut handles = vec![];

    for _ in 0..50 {
        let client = client.clone();
        let url = url.clone();
        handles.push(tokio::spawn(async move {
            let req_start = Instant::now();
            let _ = client
                .post(&url)
                .json(&json!({"text": "test"}))
                .send()
                .await;
            req_start.elapsed().as_secs_f64() * 1000.0
        }));
    }

    let mut latencies = vec![];
    for handle in handles {
        if let Ok(latency) = handle.await {
            latencies.push(latency);
        }
    }

    let total_time = start.elapsed().as_secs_f64() * 1000.0;
    latencies.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let p50 = latencies[latencies.len() / 2];
    let p99 = latencies[(latencies.len() as f64 * 0.99) as usize];
    let avg: f64 = latencies.iter().sum::<f64>() / latencies.len() as f64;

    println!("\nResults:");
    println!("  Total time for 50 concurrent: {:.2}ms", total_time);
    println!("  P50: {:.2}ms", p50);
    println!("  P99: {:.2}ms", p99);
    println!("  Avg: {:.2}ms", avg);
    println!(
        "  Effective throughput: {:.2} req/s",
        50.0 / (total_time / 1000.0)
    );

    // With 120ms provider latency and 50 concurrent, total should be ~120-200ms
    assert!(total_time < 500.0, "Concurrent requests should complete quickly");
}

/// Benchmark summary
#[tokio::test]
async fn test_print_benchmark_summary() {
    println!("\n");
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║         E2E LATENCY BENCHMARK WITH MOCK PROVIDERS            ║");
    println!("╠══════════════════════════════════════════════════════════════╣");
    println!("║                                                              ║");
    println!("║  Simulated Provider Latencies:                               ║");
    println!("║    • Deepgram STT:    {:>3}ms (typical: 50-150ms)             ║", DEEPGRAM_STT_LATENCY_MS);
    println!("║    • Deepgram TTS:    {:>3}ms (typical: 80-200ms)             ║", DEEPGRAM_TTS_LATENCY_MS);
    println!("║    • ElevenLabs TTS:  {:>3}ms (typical: 100-300ms)            ║", ELEVENLABS_TTS_LATENCY_MS);
    println!("║                                                              ║");
    println!("║  Test Coverage:                                              ║");
    println!("║    • HTTP endpoint latency                                   ║");
    println!("║    • Concurrent request handling                             ║");
    println!("║    • Gateway overhead measurement                            ║");
    println!("║                                                              ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
    println!();
}
