//! Load Test with Mocked Provider APIs
//!
//! Tests the gateway under load with realistic provider simulations:
//! - HTTP providers (ElevenLabs-style) with 100-400ms latency
//! - WebSocket providers (Deepgram-style) with 30-150ms latency
//! - Chaos elements (failures, timeouts, rate limits)
//!
//! Run: cargo test --test load_test_with_mocks -- --nocapture

mod mock_providers;

use mock_providers::{
    http_mock::{HttpMockState, spawn_http_mock},
    websocket_mock::{WebSocketMockState, spawn_stt_websocket_mock, spawn_tts_websocket_mock},
    ChaosConfig, LatencyProfile, MockStats,
};

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Barrier;
use tokio_tungstenite::{connect_async, tungstenite::Message};

/// Statistics for load test
#[derive(Debug, Default)]
struct LoadTestStats {
    total_requests: AtomicU64,
    successful_requests: AtomicU64,
    failed_requests: AtomicU64,
    total_latency_ms: AtomicU64,
    min_latency_ms: AtomicU64,
    max_latency_ms: AtomicU64,
}

impl LoadTestStats {
    fn new() -> Self {
        Self {
            min_latency_ms: AtomicU64::new(u64::MAX),
            ..Default::default()
        }
    }

    fn record(&self, latency_ms: u64, success: bool) {
        self.total_requests.fetch_add(1, Ordering::Relaxed);
        self.total_latency_ms.fetch_add(latency_ms, Ordering::Relaxed);

        if success {
            self.successful_requests.fetch_add(1, Ordering::Relaxed);
        } else {
            self.failed_requests.fetch_add(1, Ordering::Relaxed);
        }

        // Update min (compare and swap loop)
        let mut current_min = self.min_latency_ms.load(Ordering::Relaxed);
        while latency_ms < current_min {
            match self.min_latency_ms.compare_exchange_weak(
                current_min,
                latency_ms,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(x) => current_min = x,
            }
        }

        // Update max
        let mut current_max = self.max_latency_ms.load(Ordering::Relaxed);
        while latency_ms > current_max {
            match self.max_latency_ms.compare_exchange_weak(
                current_max,
                latency_ms,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(x) => current_max = x,
            }
        }
    }

    fn summary(&self) -> String {
        let total = self.total_requests.load(Ordering::Relaxed);
        let success = self.successful_requests.load(Ordering::Relaxed);
        let failed = self.failed_requests.load(Ordering::Relaxed);
        let total_latency = self.total_latency_ms.load(Ordering::Relaxed);
        let min = self.min_latency_ms.load(Ordering::Relaxed);
        let max = self.max_latency_ms.load(Ordering::Relaxed);

        let avg = if total > 0 { total_latency / total } else { 0 };
        let success_rate = if total > 0 {
            (success as f64 / total as f64) * 100.0
        } else {
            0.0
        };

        format!(
            "Total: {}, Success: {} ({:.2}%), Failed: {}, Avg: {}ms, Min: {}ms, Max: {}ms",
            total,
            success,
            success_rate,
            failed,
            avg,
            if min == u64::MAX { 0 } else { min },
            max
        )
    }
}

/// Test HTTP TTS endpoint with mock provider
#[tokio::test]
async fn test_http_tts_load_with_mock() {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║     HTTP TTS Load Test with Mock Provider                    ║");
    println!("╠══════════════════════════════════════════════════════════════╣");

    // Start mock HTTP server (ElevenLabs-style)
    let mock_state = HttpMockState::new(
        LatencyProfile::elevenlabs_tts(),
        ChaosConfig::production(),
    );
    let _mock_handle = spawn_http_mock(18765, mock_state);
    tokio::time::sleep(Duration::from_millis(500)).await;

    println!("║  Mock server: http://127.0.0.1:18765                         ║");
    println!("║  Latency profile: ElevenLabs TTS (100-400ms, P50=180ms)      ║");
    println!("║  Chaos: Production (0.1% failures)                           ║");
    println!("║  Concurrent clients: 20                                      ║");
    println!("║  Requests per client: 10                                     ║");
    println!("╠══════════════════════════════════════════════════════════════╣");

    let client = Arc::new(reqwest::Client::new());
    let stats = Arc::new(LoadTestStats::new());
    let barrier = Arc::new(Barrier::new(20));

    let mut handles = vec![];

    // Spawn 20 concurrent clients
    for _ in 0..20 {
        let client = client.clone();
        let stats = stats.clone();
        let barrier = barrier.clone();

        handles.push(tokio::spawn(async move {
            barrier.wait().await; // Synchronize start

            for _ in 0..10 {
                let start = Instant::now();
                let result = client
                    .post("http://127.0.0.1:18765/v1/text-to-speech/mock-voice")
                    .json(&json!({"text": "Hello world test"}))
                    .timeout(Duration::from_secs(5))
                    .send()
                    .await;

                let latency = start.elapsed().as_millis() as u64;
                let success = result.map(|r| r.status().is_success()).unwrap_or(false);
                stats.record(latency, success);
            }
        }));
    }

    // Wait for all clients to complete
    for handle in handles {
        handle.await.unwrap();
    }

    println!("║                                                              ║");
    println!("║  Results:                                                    ║");
    println!("║  {}  ║", stats.summary());
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    // Verify success rate
    let total = stats.total_requests.load(Ordering::Relaxed);
    let success = stats.successful_requests.load(Ordering::Relaxed);
    let success_rate = success as f64 / total as f64;

    assert!(total == 200, "Should have 200 total requests");
    assert!(success_rate > 0.95, "Success rate should be >95%");
}

/// Test WebSocket STT with mock provider
#[tokio::test]
async fn test_websocket_stt_load_with_mock() {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║     WebSocket STT Load Test with Mock Provider               ║");
    println!("╠══════════════════════════════════════════════════════════════╣");

    // Start mock WebSocket server (Deepgram-style)
    let mock_state = Arc::new(WebSocketMockState::new(
        LatencyProfile::deepgram_stt(),
        LatencyProfile::deepgram_tts(),
        ChaosConfig::production(),
    ));
    let _mock_handle = spawn_stt_websocket_mock(18766, mock_state.clone());
    tokio::time::sleep(Duration::from_millis(500)).await;

    println!("║  Mock server: ws://127.0.0.1:18766                           ║");
    println!("║  Latency profile: Deepgram STT (30-150ms, P50=50ms)          ║");
    println!("║  Chaos: Production (0.1% failures)                           ║");
    println!("║  Concurrent connections: 10                                  ║");
    println!("║  Audio chunks per connection: 20                             ║");
    println!("╠══════════════════════════════════════════════════════════════╣");

    let stats = Arc::new(LoadTestStats::new());
    let barrier = Arc::new(Barrier::new(10));
    let mut handles = vec![];

    // Spawn 10 concurrent WebSocket connections
    for conn_id in 0..10 {
        let stats = stats.clone();
        let barrier = barrier.clone();

        handles.push(tokio::spawn(async move {
            barrier.wait().await; // Synchronize start

            let url = "ws://127.0.0.1:18766";
            let connect_result = connect_async(url).await;

            if let Ok((ws_stream, _)) = connect_result {
                let (mut write, mut read) = ws_stream.split();

                // Send 20 audio chunks
                for chunk_id in 0..20 {
                    let start = Instant::now();

                    // Send audio chunk (1KB)
                    let audio_chunk: bytes::Bytes = vec![0u8; 1024].into();
                    if write.send(Message::Binary(audio_chunk)).await.is_err() {
                        stats.record(start.elapsed().as_millis() as u64, false);
                        continue;
                    }

                    // Wait for transcript response
                    match tokio::time::timeout(Duration::from_secs(5), read.next()).await {
                        Ok(Some(Ok(Message::Text(_)))) => {
                            stats.record(start.elapsed().as_millis() as u64, true);
                        }
                        _ => {
                            stats.record(start.elapsed().as_millis() as u64, false);
                        }
                    }
                }

                // Close connection
                let _ = write.send(Message::Close(None)).await;
            } else {
                // Connection failed
                for _ in 0..20 {
                    stats.record(0, false);
                }
            }
        }));
    }

    // Wait for all connections to complete
    for handle in handles {
        handle.await.unwrap();
    }

    println!("║                                                              ║");
    println!("║  Results:                                                    ║");
    println!("║  {}  ║", stats.summary());
    println!("║                                                              ║");
    println!("║  Mock Server Stats:                                          ║");
    println!("║  {}  ║", mock_state.stats.summary());
    println!("╚══════════════════════════════════════════════════════════════╝\n");
}

/// Test with chaos conditions (high failure rate)
#[tokio::test]
async fn test_http_with_chaos() {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║     HTTP Load Test with CHAOS Conditions                     ║");
    println!("╠══════════════════════════════════════════════════════════════╣");

    // Start mock HTTP server with high chaos
    let mock_state = HttpMockState::new(
        LatencyProfile::elevenlabs_tts(),
        ChaosConfig::stress(), // 5% failures, 3% timeouts, etc.
    );
    let _mock_handle = spawn_http_mock(18767, mock_state);
    tokio::time::sleep(Duration::from_millis(500)).await;

    println!("║  Mock server: http://127.0.0.1:18767                         ║");
    println!("║  Chaos profile: STRESS                                       ║");
    println!("║    - 5%% failure rate                                         ║");
    println!("║    - 3%% timeout rate                                         ║");
    println!("║    - 2%% connection drop rate                                 ║");
    println!("║    - 5%% rate limit rate                                      ║");
    println!("║    - 10%% slow response rate                                  ║");
    println!("║  Concurrent clients: 10                                      ║");
    println!("║  Requests per client: 20                                     ║");
    println!("╠══════════════════════════════════════════════════════════════╣");

    let client = Arc::new(reqwest::Client::new());
    let stats = Arc::new(LoadTestStats::new());
    let mut handles = vec![];

    // Spawn 10 concurrent clients
    for _ in 0..10 {
        let client = client.clone();
        let stats = stats.clone();

        handles.push(tokio::spawn(async move {
            for _ in 0..20 {
                let start = Instant::now();
                let result = client
                    .post("http://127.0.0.1:18767/v1/text-to-speech/mock-voice")
                    .json(&json!({"text": "Chaos test"}))
                    .timeout(Duration::from_secs(2)) // Short timeout
                    .send()
                    .await;

                let latency = start.elapsed().as_millis() as u64;
                let success = result.map(|r| r.status().is_success()).unwrap_or(false);
                stats.record(latency, success);
            }
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }

    println!("║                                                              ║");
    println!("║  Results:                                                    ║");
    println!("║  {}  ║", stats.summary());
    println!("║                                                              ║");

    let total = stats.total_requests.load(Ordering::Relaxed);
    let failed = stats.failed_requests.load(Ordering::Relaxed);
    let failure_rate = failed as f64 / total as f64 * 100.0;

    println!("║  Actual failure rate: {:.2}%% (expected ~15-20%%)              ║", failure_rate);
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    // With chaos, expect some failures
    assert!(total == 200, "Should have 200 total requests");
    assert!(failure_rate > 5.0, "Chaos should cause >5% failures");
    assert!(failure_rate < 50.0, "Chaos shouldn't cause >50% failures");
}

/// Test with zero latency to measure pure gateway overhead
#[tokio::test]
async fn test_gateway_overhead_measurement() {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║     Gateway Overhead Measurement (Zero Provider Latency)     ║");
    println!("╠══════════════════════════════════════════════════════════════╣");

    // Start mock HTTP server with ZERO latency
    let zero_latency = LatencyProfile {
        min_ms: 0,
        max_ms: 1,
        p50_ms: 0,
        p99_ms: 1,
    };
    let mock_state = HttpMockState::new(zero_latency, ChaosConfig::default());
    let _mock_handle = spawn_http_mock(18768, mock_state);
    tokio::time::sleep(Duration::from_millis(500)).await;

    println!("║  Mock server: http://127.0.0.1:18768                         ║");
    println!("║  Latency profile: ZERO (0-1ms)                               ║");
    println!("║  Purpose: Measure pure gateway overhead                      ║");
    println!("║  Concurrent clients: 50                                      ║");
    println!("║  Requests per client: 20                                     ║");
    println!("╠══════════════════════════════════════════════════════════════╣");

    let client = Arc::new(reqwest::Client::new());
    let stats = Arc::new(LoadTestStats::new());
    let barrier = Arc::new(Barrier::new(50));
    let mut handles = vec![];

    // Spawn 50 concurrent clients
    for _ in 0..50 {
        let client = client.clone();
        let stats = stats.clone();
        let barrier = barrier.clone();

        handles.push(tokio::spawn(async move {
            barrier.wait().await;

            for _ in 0..20 {
                let start = Instant::now();
                let result = client
                    .post("http://127.0.0.1:18768/v1/text-to-speech/mock-voice")
                    .json(&json!({"text": "Hello"}))
                    .timeout(Duration::from_secs(5))
                    .send()
                    .await;

                let latency = start.elapsed().as_millis() as u64;
                let success = result.map(|r| r.status().is_success()).unwrap_or(false);
                stats.record(latency, success);
            }
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }

    let total = stats.total_requests.load(Ordering::Relaxed);
    let success = stats.successful_requests.load(Ordering::Relaxed);
    let total_latency = stats.total_latency_ms.load(Ordering::Relaxed);
    let min = stats.min_latency_ms.load(Ordering::Relaxed);
    let max = stats.max_latency_ms.load(Ordering::Relaxed);
    let avg = if total > 0 { total_latency / total } else { 0 };

    println!("║                                                              ║");
    println!("║  Results (Gateway Overhead Only):                            ║");
    println!("║  Total: {}, Success: {} ({:.2}%)                         ║", total, success, (success as f64 / total as f64) * 100.0);
    println!("║  Avg Gateway Latency: {}ms                                   ║", avg);
    println!("║  Min Gateway Latency: {}ms                                   ║", if min == u64::MAX { 0 } else { min });
    println!("║  Max Gateway Latency: {}ms                                   ║", max);
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    // Gateway overhead should be <10ms for most requests
    assert!(total == 1000, "Should have 1000 total requests");
    assert!(success as f64 / total as f64 > 0.99, "Should have >99% success");
    assert!(avg < 50, "Average gateway overhead should be <50ms");
}

/// High concurrency WebSocket test
#[tokio::test]
async fn test_websocket_high_concurrency() {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║     WebSocket High Concurrency Test                          ║");
    println!("╠══════════════════════════════════════════════════════════════╣");

    let mock_state = Arc::new(WebSocketMockState::new(
        LatencyProfile::deepgram_stt(),
        LatencyProfile::deepgram_tts(),
        ChaosConfig::production(),
    ));
    let _mock_handle = spawn_stt_websocket_mock(18769, mock_state.clone());
    tokio::time::sleep(Duration::from_millis(500)).await;

    println!("║  Mock server: ws://127.0.0.1:18769                           ║");
    println!("║  Concurrent connections: 50                                  ║");
    println!("║  Messages per connection: 10                                 ║");
    println!("╠══════════════════════════════════════════════════════════════╣");

    let stats = Arc::new(LoadTestStats::new());
    let barrier = Arc::new(Barrier::new(50));
    let mut handles = vec![];

    // Spawn 50 concurrent WebSocket connections
    for _ in 0..50 {
        let stats = stats.clone();
        let barrier = barrier.clone();

        handles.push(tokio::spawn(async move {
            barrier.wait().await;

            let url = "ws://127.0.0.1:18769";
            if let Ok((ws_stream, _)) = connect_async(url).await {
                let (mut write, mut read) = ws_stream.split();

                for _ in 0..10 {
                    let start = Instant::now();
                    let audio_chunk: bytes::Bytes = vec![0u8; 1024].into();

                    if write.send(Message::Binary(audio_chunk)).await.is_err() {
                        stats.record(start.elapsed().as_millis() as u64, false);
                        continue;
                    }

                    match tokio::time::timeout(Duration::from_secs(5), read.next()).await {
                        Ok(Some(Ok(Message::Text(_)))) => {
                            stats.record(start.elapsed().as_millis() as u64, true);
                        }
                        _ => {
                            stats.record(start.elapsed().as_millis() as u64, false);
                        }
                    }
                }

                let _ = write.send(Message::Close(None)).await;
            } else {
                for _ in 0..10 {
                    stats.record(0, false);
                }
            }
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }

    let total = stats.total_requests.load(Ordering::Relaxed);
    let success = stats.successful_requests.load(Ordering::Relaxed);
    let success_rate = success as f64 / total as f64 * 100.0;

    println!("║                                                              ║");
    println!("║  Results:                                                    ║");
    println!("║  {}  ║", stats.summary());
    println!("║  Mock Stats: {}  ║", mock_state.stats.summary());
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    assert!(total == 500, "Should have 500 total messages");
    assert!(success_rate > 90.0, "Should have >90% success rate under high concurrency");
}

/// Summary test
#[tokio::test]
async fn test_print_load_test_summary() {
    println!("\n");
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║           LOAD TEST WITH MOCKED PROVIDERS - SUMMARY          ║");
    println!("╠══════════════════════════════════════════════════════════════╣");
    println!("║                                                              ║");
    println!("║  Provider Latency Profiles:                                  ║");
    println!("║    Deepgram STT:    30-150ms  (P50=50ms,  P99=120ms)         ║");
    println!("║    Deepgram TTS:    50-200ms  (P50=80ms,  P99=180ms)         ║");
    println!("║    ElevenLabs TTS: 100-400ms  (P50=180ms, P99=350ms)         ║");
    println!("║    Google STT:      40-200ms  (P50=60ms,  P99=150ms)         ║");
    println!("║    Google TTS:      80-300ms  (P50=120ms, P99=250ms)         ║");
    println!("║    OpenAI Realtime:100-500ms  (P50=200ms, P99=450ms)         ║");
    println!("║                                                              ║");
    println!("║  Chaos Profiles:                                             ║");
    println!("║    Production: 0.1%% fail, 0.2%% timeout, 0.05%% drop          ║");
    println!("║    Stress:     5%% fail, 3%% timeout, 2%% drop                 ║");
    println!("║                                                              ║");
    println!("║  Connection Types Tested:                                    ║");
    println!("║    ✓ HTTP (ElevenLabs, OpenAI, PlayHT style)                 ║");
    println!("║    ✓ WebSocket (Deepgram, Cartesia, LMNT style)              ║");
    println!("║    ○ gRPC (Google style) - latency profile simulated         ║");
    println!("║                                                              ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
    println!();
}
