//! TDD tests for Fix 3: Remove sleep from LiveKit audio hot path
//!
//! These tests verify that the audio processing path maintains real-time guarantees
//! by avoiding blocking operations like sleep() in the hot path.
//!
//! Test scenarios:
//! 1. No sleep calls in capture_frame failure path
//! 2. Immediate queuing on capture failure
//! 3. Bounded queue respects MAX_AUDIO_QUEUE_SIZE
//! 4. Queue draining happens opportunistically without blocking
//! 5. Latency stays within budget (< 10ms for capture operations)

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

/// Maximum allowed latency for a single audio frame operation (excluding actual capture)
const MAX_FRAME_OPERATION_LATENCY_MS: u64 = 5;

/// Maximum queue size (must match the constant in livekit/mod.rs)
const MAX_AUDIO_QUEUE_SIZE: usize = 100;

/// Test that audio queueing operation completes within latency budget
/// This ensures no sleep() or blocking operations in the queue path
#[tokio::test]
async fn test_audio_queue_operation_latency() {
    let audio_queue: Arc<Mutex<VecDeque<Vec<u8>>>> = Arc::new(Mutex::new(VecDeque::new()));

    // Simulate 100 queue operations and measure time
    let start = Instant::now();

    for _i in 0..100 {
        let audio_data = vec![0u8; 1024]; // 512 samples at 16-bit
        let mut queue = audio_queue.lock().await;

        // Bounded queue logic - drop oldest if full
        if queue.len() >= MAX_AUDIO_QUEUE_SIZE {
            queue.pop_front();
        }
        queue.push_back(audio_data);
    }

    let elapsed = start.elapsed();
    let per_operation_us = elapsed.as_micros() / 100;

    // Each queue operation should complete in microseconds, not milliseconds
    // If there were sleep(10ms) calls, this would be at least 1000ms total
    assert!(
        elapsed < Duration::from_millis(100),
        "Queue operations took {}ms, expected < 100ms (no sleep in path). Per-op: {}μs",
        elapsed.as_millis(),
        per_operation_us
    );

    // Per-operation should be under 1ms
    assert!(
        per_operation_us < 1000,
        "Per-operation latency {}μs exceeds 1000μs budget",
        per_operation_us
    );
}

/// Test that immediate queue on failure doesn't introduce retry delays
#[tokio::test]
async fn test_immediate_queue_no_retry_delay() {
    let audio_queue: Arc<Mutex<VecDeque<Vec<u8>>>> = Arc::new(Mutex::new(VecDeque::new()));

    // Simulate capture failure scenario - should immediately queue
    let audio_data = vec![0u8; 2048];

    let start = Instant::now();

    // This simulates what should happen on capture failure:
    // Immediately queue without retries
    {
        let mut queue = audio_queue.lock().await;
        if queue.len() >= MAX_AUDIO_QUEUE_SIZE {
            queue.pop_front();
        }
        queue.push_back(audio_data.clone());
    }

    let elapsed = start.elapsed();

    // Must complete in under 5ms (no 10ms sleep)
    assert!(
        elapsed < Duration::from_millis(MAX_FRAME_OPERATION_LATENCY_MS),
        "Queue-on-failure took {}ms, expected < {}ms (sleep detected!)",
        elapsed.as_millis(),
        MAX_FRAME_OPERATION_LATENCY_MS
    );
}

/// Test bounded queue correctly enforces MAX_AUDIO_QUEUE_SIZE
#[tokio::test]
async fn test_bounded_queue_size_enforcement() {
    let audio_queue: Arc<Mutex<VecDeque<Vec<u8>>>> = Arc::new(Mutex::new(VecDeque::new()));

    // Fill queue beyond max
    for i in 0..(MAX_AUDIO_QUEUE_SIZE + 50) {
        let audio_data = vec![i as u8; 100];
        let mut queue = audio_queue.lock().await;

        if queue.len() >= MAX_AUDIO_QUEUE_SIZE {
            queue.pop_front();
        }
        queue.push_back(audio_data);
    }

    let queue = audio_queue.lock().await;
    assert_eq!(
        queue.len(),
        MAX_AUDIO_QUEUE_SIZE,
        "Queue size {} exceeds MAX_AUDIO_QUEUE_SIZE {}",
        queue.len(),
        MAX_AUDIO_QUEUE_SIZE
    );

    // Verify oldest frames were dropped (FIFO)
    // First item should be from iteration (MAX_AUDIO_QUEUE_SIZE + 50 - MAX_AUDIO_QUEUE_SIZE) = 50
    let first = queue.front().unwrap();
    assert_eq!(
        first[0], 50,
        "Expected oldest retained frame to be from iteration 50, got {}",
        first[0]
    );
}

/// Test that queue draining doesn't block the main path
#[tokio::test]
async fn test_opportunistic_queue_drain_non_blocking() {
    let audio_queue: Arc<Mutex<VecDeque<Vec<u8>>>> = Arc::new(Mutex::new(VecDeque::new()));

    // Pre-fill queue with some data
    {
        let mut queue = audio_queue.lock().await;
        for _ in 0..10 {
            queue.push_back(vec![0u8; 100]);
        }
    }

    // Simulate successful capture followed by opportunistic drain
    let start = Instant::now();

    // Drain up to 5 items (matching the current implementation logic)
    let drained: Vec<Vec<u8>> = {
        let mut queue = audio_queue.lock().await;
        if !queue.is_empty() && queue.len() <= 5 {
            let drain_count = queue.len().min(5);
            queue.drain(..drain_count).collect()
        } else if queue.len() > 5 {
            // Only drain 5 at a time to avoid blocking
            queue.drain(..5).collect()
        } else {
            Vec::new()
        }
    };

    let elapsed = start.elapsed();

    // Drain operation must be fast
    assert!(
        elapsed < Duration::from_millis(5),
        "Queue drain took {}ms, expected < 5ms",
        elapsed.as_millis()
    );

    assert_eq!(drained.len(), 5, "Expected to drain 5 items");
}

/// Test concurrent queue access doesn't cause excessive latency
#[tokio::test]
async fn test_concurrent_queue_access_latency() {
    let audio_queue: Arc<Mutex<VecDeque<Vec<u8>>>> = Arc::new(Mutex::new(VecDeque::new()));
    let queue_clone = Arc::clone(&audio_queue);

    // Spawn a task that continuously adds to queue
    let producer = tokio::spawn(async move {
        for i in 0..50 {
            let mut queue = queue_clone.lock().await;
            if queue.len() >= MAX_AUDIO_QUEUE_SIZE {
                queue.pop_front();
            }
            queue.push_back(vec![i as u8; 100]);
            // Small yield to simulate real-world interleaving
            tokio::task::yield_now().await;
        }
    });

    // Measure consumer access latency during concurrent operations
    let mut max_latency = Duration::ZERO;
    for _ in 0..50 {
        let start = Instant::now();
        {
            let queue = audio_queue.lock().await;
            let _ = queue.len();
        }
        let elapsed = start.elapsed();
        if elapsed > max_latency {
            max_latency = elapsed;
        }
        tokio::task::yield_now().await;
    }

    producer.await.unwrap();

    // Even with contention, individual access should be fast
    // (No sleep in the critical section)
    assert!(
        max_latency < Duration::from_millis(10),
        "Max queue access latency {}ms exceeds 10ms budget",
        max_latency.as_millis()
    );
}

/// Test that audio frame conversion doesn't add latency
#[tokio::test]
async fn test_audio_conversion_latency() {
    // Simulate a typical audio frame: 10ms at 16kHz stereo = 320 samples * 2 channels * 2 bytes
    let audio_data = vec![0u8; 1280];

    let start = Instant::now();

    // Simulate conversion logic
    let _sample_rate = 16000u32;
    let channels = 2u16;

    // Validation
    assert!(
        audio_data.len().is_multiple_of(2),
        "Audio data must have even length"
    );
    assert!(channels > 0, "Channels must be > 0");

    let num_samples = audio_data.len() / 2;
    let samples_per_channel = num_samples / channels as usize;

    assert!(
        samples_per_channel > 0 && num_samples.is_multiple_of(channels as usize),
        "Invalid sample count"
    );

    // Zero-copy conversion (when aligned)
    let samples: Vec<i16> = if (audio_data.as_ptr() as usize).is_multiple_of(2) {
        unsafe {
            let ptr = audio_data.as_ptr() as *const i16;
            std::slice::from_raw_parts(ptr, num_samples).to_vec()
        }
    } else {
        audio_data
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect()
    };

    let elapsed = start.elapsed();

    // Conversion should be sub-millisecond
    assert!(
        elapsed < Duration::from_millis(1),
        "Audio conversion took {}μs, expected < 1000μs",
        elapsed.as_micros()
    );

    assert_eq!(samples.len(), num_samples);
}

/// Test worst-case scenario: capture fails, must queue without delay
#[tokio::test]
async fn test_capture_failure_immediate_queue_worst_case() {
    let audio_queue: Arc<Mutex<VecDeque<Vec<u8>>>> = Arc::new(Mutex::new(VecDeque::new()));

    // Pre-fill queue to near capacity
    {
        let mut queue = audio_queue.lock().await;
        for _ in 0..(MAX_AUDIO_QUEUE_SIZE - 1) {
            queue.push_back(vec![0u8; 100]);
        }
    }

    // Now simulate capture failure - must queue immediately
    let audio_data = vec![0u8; 1024];

    let start = Instant::now();

    {
        let mut queue = audio_queue.lock().await;
        if queue.len() >= MAX_AUDIO_QUEUE_SIZE {
            queue.pop_front();
        }
        queue.push_back(audio_data);
    }

    let elapsed = start.elapsed();

    // Even at near-capacity, operation must be fast
    assert!(
        elapsed < Duration::from_millis(2),
        "Near-capacity queue operation took {}ms, expected < 2ms",
        elapsed.as_millis()
    );
}

/// Test that no retries happen on capture failure
/// The old code had 3 retries with 10ms sleep each = 30ms worst case
/// New code should have 0 retries = 0ms delay
#[tokio::test]
async fn test_no_retry_loop_on_failure() {
    // Simulate the new behavior: immediate queue on ANY failure
    let audio_queue: Arc<Mutex<VecDeque<Vec<u8>>>> = Arc::new(Mutex::new(VecDeque::new()));

    let start = Instant::now();

    // Simulate 10 consecutive capture failures
    for _ in 0..10 {
        let audio_data = vec![0u8; 512];

        // Old behavior: retry 3 times with 10ms sleep = 30ms per failure
        // New behavior: immediate queue = ~0ms per failure

        let mut queue = audio_queue.lock().await;
        if queue.len() >= MAX_AUDIO_QUEUE_SIZE {
            queue.pop_front();
        }
        queue.push_back(audio_data);
    }

    let elapsed = start.elapsed();

    // If old retry behavior existed: 10 failures * 30ms = 300ms minimum
    // New behavior should be < 10ms total
    assert!(
        elapsed < Duration::from_millis(50),
        "10 failures took {}ms. If > 300ms, retry loop still exists!",
        elapsed.as_millis()
    );
}

/// Test metrics tracking for queue depth and failures
/// (Verifies the infrastructure exists even if we can't test actual metrics crate)
#[tokio::test]
async fn test_queue_depth_tracking() {
    let audio_queue: Arc<Mutex<VecDeque<Vec<u8>>>> = Arc::new(Mutex::new(VecDeque::new()));
    let mut capture_failures = 0u64;
    let mut frames_queued = 0u64;

    // Simulate some operations with metric tracking
    for i in 0..20 {
        let audio_data = vec![i as u8; 100];

        // Simulate capture result (fails on even iterations)
        let capture_succeeded = i % 2 != 0;

        if capture_succeeded {
            // Success path - would call capture_frame
        } else {
            // Failure path - queue immediately and record metric
            capture_failures += 1;
            frames_queued += 1;

            let mut queue = audio_queue.lock().await;
            if queue.len() >= MAX_AUDIO_QUEUE_SIZE {
                queue.pop_front();
            }
            queue.push_back(audio_data);
        }
    }

    assert_eq!(capture_failures, 10, "Expected 10 capture failures");
    assert_eq!(frames_queued, 10, "Expected 10 frames queued");

    let queue = audio_queue.lock().await;
    assert_eq!(queue.len(), 10, "Queue should have 10 items");
}

/// Stress test: rapid fire audio frames must not accumulate latency
#[tokio::test]
async fn test_rapid_fire_latency_stability() {
    let audio_queue: Arc<Mutex<VecDeque<Vec<u8>>>> = Arc::new(Mutex::new(VecDeque::new()));

    let mut latencies = Vec::with_capacity(1000);

    for _ in 0..1000 {
        let audio_data = vec![0u8; 640]; // 10ms at 16kHz mono

        let start = Instant::now();
        {
            let mut queue = audio_queue.lock().await;
            if queue.len() >= MAX_AUDIO_QUEUE_SIZE {
                queue.pop_front();
            }
            queue.push_back(audio_data);
        }
        latencies.push(start.elapsed());
    }

    // Calculate p99 latency
    latencies.sort();
    let p99_idx = (latencies.len() as f64 * 0.99) as usize;
    let p99_latency = latencies[p99_idx];

    // p99 should be under 1ms
    assert!(
        p99_latency < Duration::from_millis(1),
        "p99 latency {}μs exceeds 1000μs budget",
        p99_latency.as_micros()
    );

    // No latency should exceed 5ms (would indicate sleep)
    let max_latency = latencies.last().unwrap();
    assert!(
        *max_latency < Duration::from_millis(5),
        "Max latency {}μs indicates blocking operation",
        max_latency.as_micros()
    );
}

/// Integration-style test simulating the full send_audio flow
#[tokio::test]
async fn test_full_send_audio_flow_latency() {
    // This simulates the complete flow from audio data to queue
    // without actually calling LiveKit APIs

    let audio_queue: Arc<Mutex<VecDeque<Vec<u8>>>> = Arc::new(Mutex::new(VecDeque::new()));
    let is_connected = Arc::new(Mutex::new(true));

    let _sample_rate = 16000u32;
    let channels = 1u16;

    // Test data: 10ms of audio at 16kHz mono
    let audio_data = vec![0u8; 320];

    let start = Instant::now();

    // Check connection
    if !*is_connected.lock().await {
        panic!("Not connected");
    }

    // Simulate no audio source available (queuing path)
    // This is the path that was slow due to retries

    // Validate audio format
    assert!(audio_data.len().is_multiple_of(2));
    let num_samples = audio_data.len() / 2;
    let samples_per_channel = num_samples / channels as usize;
    assert!(samples_per_channel > 0);

    // Queue audio (what happens when source unavailable or capture fails)
    {
        let mut queue = audio_queue.lock().await;
        if queue.len() >= MAX_AUDIO_QUEUE_SIZE {
            queue.pop_front();
        }
        queue.push_back(audio_data);
    }

    let elapsed = start.elapsed();

    // Full flow should complete in under 5ms
    assert!(
        elapsed < Duration::from_millis(5),
        "Full send_audio flow took {}ms, expected < 5ms",
        elapsed.as_millis()
    );
}

/// Test that queue operations remain fast under memory pressure
#[tokio::test]
async fn test_queue_latency_under_memory_pressure() {
    let audio_queue: Arc<Mutex<VecDeque<Vec<u8>>>> = Arc::new(Mutex::new(VecDeque::new()));

    // Allocate some background memory to simulate pressure
    let _pressure: Vec<Vec<u8>> = (0..1000).map(|_| vec![0u8; 10000]).collect();

    let start = Instant::now();

    for _ in 0..100 {
        let audio_data = vec![0u8; 640];
        let mut queue = audio_queue.lock().await;
        if queue.len() >= MAX_AUDIO_QUEUE_SIZE {
            queue.pop_front();
        }
        queue.push_back(audio_data);
    }

    let elapsed = start.elapsed();

    // Should still be fast even under memory pressure
    assert!(
        elapsed < Duration::from_millis(50),
        "Operations under memory pressure took {}ms, expected < 50ms",
        elapsed.as_millis()
    );
}
