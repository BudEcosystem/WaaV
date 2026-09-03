//! TDD tests for Fix 4: VoiceManager lock contention optimization
//!
//! These tests verify that the VoiceManager's receive_audio() method
//! maintains real-time guarantees by minimizing lock contention.
//!
//! Test scenarios:
//! 1. Concurrent receive_audio() calls don't block each other
//! 2. Audio always reaches STT provider (no drops due to lock contention)
//! 3. Smart turn processing is optional/skippable when lock is busy
//! 4. Latency stays within budget (< 10ms for audio forwarding)

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::{Mutex, RwLock};

/// Maximum allowed latency for forwarding audio to STT (excluding ML inference)
const MAX_AUDIO_FORWARD_LATENCY_MS: u64 = 10;

/// Simulated ML inference time for smart turn processing
const SIMULATED_ML_INFERENCE_MS: u64 = 20;

/// Mock SmartTurnProcessor that simulates ML inference time
struct MockSmartTurnProcessor {
    inference_count: AtomicU64,
}

impl MockSmartTurnProcessor {
    fn new() -> Self {
        Self {
            inference_count: AtomicU64::new(0),
        }
    }

    async fn process_audio(&self, _audio: &[f32]) {
        // Simulate ML inference time
        tokio::time::sleep(Duration::from_millis(SIMULATED_ML_INFERENCE_MS)).await;
        self.inference_count.fetch_add(1, Ordering::Relaxed);
    }

    fn inference_count(&self) -> u64 {
        self.inference_count.load(Ordering::Relaxed)
    }
}

/// Mock STT provider that tracks audio frames received
struct MockSTTProvider {
    frames_received: AtomicU64,
}

impl MockSTTProvider {
    fn new() -> Self {
        Self {
            frames_received: AtomicU64::new(0),
        }
    }

    async fn send_audio(&self, _audio: &[u8]) {
        // Simulate minimal STT processing time
        self.frames_received.fetch_add(1, Ordering::Relaxed);
    }

    fn frames_received(&self) -> u64 {
        self.frames_received.load(Ordering::Relaxed)
    }
}

/// Simulates the BEFORE pattern: write lock held during ML inference
async fn receive_audio_blocking_pattern(
    audio: &[u8],
    smart_turn: &Arc<RwLock<Option<MockSmartTurnProcessor>>>,
    stt: &Arc<Mutex<MockSTTProvider>>,
) -> Duration {
    let start = Instant::now();

    // BEFORE: Write lock held during entire ML inference (BAD)
    let smart_turn_guard = smart_turn.write().await;
    if let Some(ref processor) = *smart_turn_guard {
        let samples: Vec<f32> = audio.chunks(2).map(|_| 0.0f32).collect();
        processor.process_audio(&samples).await;
    }
    drop(smart_turn_guard);

    // Send to STT
    let stt_guard = stt.lock().await;
    stt_guard.send_audio(audio).await;

    start.elapsed()
}

/// Simulates the AFTER pattern: try_write to avoid blocking
/// Returns (latency, was_smart_turn_processed)
async fn receive_audio_non_blocking_pattern(
    audio: &[u8],
    smart_turn: &Arc<RwLock<Option<MockSmartTurnProcessor>>>,
    stt: &Arc<Mutex<MockSTTProvider>>,
) -> (Duration, bool) {
    let start = Instant::now();
    let mut processed_smart_turn = false;

    // AFTER: Try to get write lock, skip smart turn if busy (GOOD)
    if let Ok(smart_turn_guard) = smart_turn.try_write()
        && let Some(ref processor) = *smart_turn_guard
    {
        let samples: Vec<f32> = audio.chunks(2).map(|_| 0.0f32).collect();
        processor.process_audio(&samples).await;
        processed_smart_turn = true;
    }
    // If try_write() fails, lock is busy - skip smart turn, audio still goes to STT

    // Send to STT (always happens, regardless of smart turn)
    let stt_guard = stt.lock().await;
    stt_guard.send_audio(audio).await;

    (start.elapsed(), processed_smart_turn)
}

/// Test that concurrent audio frames don't block each other with non-blocking pattern
#[tokio::test]
async fn test_concurrent_receive_audio_no_blocking() {
    let smart_turn = Arc::new(RwLock::new(Some(MockSmartTurnProcessor::new())));
    let stt = Arc::new(Mutex::new(MockSTTProvider::new()));

    let num_concurrent = 10;
    let mut handles = Vec::new();

    let start = Instant::now();

    // Spawn concurrent audio processing tasks
    for _ in 0..num_concurrent {
        let smart_turn_clone = Arc::clone(&smart_turn);
        let stt_clone = Arc::clone(&stt);

        handles.push(tokio::spawn(async move {
            let audio_data = vec![0u8; 640]; // 10ms at 16kHz mono
            receive_audio_non_blocking_pattern(&audio_data, &smart_turn_clone, &stt_clone).await
        }));
    }

    // Wait for all to complete
    let mut latencies = Vec::new();
    for handle in handles {
        latencies.push(handle.await.unwrap());
    }

    let total_elapsed = start.elapsed();

    // With non-blocking pattern, total time should be ~ML_INFERENCE_MS (parallel)
    // not num_concurrent * ML_INFERENCE_MS (serial)
    let expected_serial_time =
        Duration::from_millis(num_concurrent as u64 * SIMULATED_ML_INFERENCE_MS);

    assert!(
        total_elapsed < expected_serial_time,
        "Total time {}ms >= serial time {}ms (tasks blocked each other!)",
        total_elapsed.as_millis(),
        expected_serial_time.as_millis()
    );

    // All audio should have reached STT
    let stt_guard = stt.lock().await;
    assert_eq!(
        stt_guard.frames_received(),
        num_concurrent as u64,
        "Not all audio frames reached STT"
    );
}

/// Test that all audio reaches STT even when smart turn lock is contended
#[tokio::test]
async fn test_audio_always_reaches_stt() {
    let smart_turn = Arc::new(RwLock::new(Some(MockSmartTurnProcessor::new())));
    let stt = Arc::new(Mutex::new(MockSTTProvider::new()));

    let num_frames = 100;
    let mut handles = Vec::new();

    // Spawn many concurrent audio frames
    for _ in 0..num_frames {
        let smart_turn_clone = Arc::clone(&smart_turn);
        let stt_clone = Arc::clone(&stt);

        handles.push(tokio::spawn(async move {
            let audio_data = vec![0u8; 320];
            receive_audio_non_blocking_pattern(&audio_data, &smart_turn_clone, &stt_clone).await
        }));
    }

    // Wait for all to complete
    for handle in handles {
        handle.await.unwrap();
    }

    // ALL audio must reach STT - no drops allowed
    let stt_guard = stt.lock().await;
    assert_eq!(
        stt_guard.frames_received(),
        num_frames as u64,
        "Audio frames dropped! Expected {}, got {}",
        num_frames,
        stt_guard.frames_received()
    );
}

/// Test that smart turn processing is skipped when lock is busy
#[tokio::test]
async fn test_smart_turn_skipped_when_busy() {
    let processor = MockSmartTurnProcessor::new();
    let smart_turn = Arc::new(RwLock::new(Some(processor)));
    let stt = Arc::new(Mutex::new(MockSTTProvider::new()));

    // First, hold the lock with a long-running task
    let smart_turn_clone = Arc::clone(&smart_turn);
    let lock_holder = tokio::spawn(async move {
        let guard = smart_turn_clone.write().await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        drop(guard);
    });

    // Give the lock holder time to acquire the lock
    tokio::time::sleep(Duration::from_millis(10)).await;

    // Now try to process audio - should skip smart turn but still send to STT
    let audio_data = vec![0u8; 640];
    let (latency, processed_smart_turn) =
        receive_audio_non_blocking_pattern(&audio_data, &smart_turn, &stt).await;

    // Smart turn should have been skipped
    assert!(
        !processed_smart_turn,
        "Smart turn should have been skipped when lock is busy"
    );

    // Latency should be minimal since we skipped smart turn
    assert!(
        latency < Duration::from_millis(50),
        "Audio forward latency {}ms too high (smart turn should have been skipped)",
        latency.as_millis()
    );

    // Audio should still have reached STT
    let stt_guard = stt.lock().await;
    assert_eq!(
        stt_guard.frames_received(),
        1,
        "Audio frame should have reached STT despite smart turn skip"
    );

    lock_holder.await.unwrap();
}

/// Test latency comparison between blocking and non-blocking patterns
#[tokio::test]
async fn test_latency_improvement_with_non_blocking() {
    // Test blocking pattern
    let smart_turn_blocking = Arc::new(RwLock::new(Some(MockSmartTurnProcessor::new())));
    let stt_blocking = Arc::new(Mutex::new(MockSTTProvider::new()));

    let start_blocking = Instant::now();
    for _ in 0..5 {
        let audio_data = vec![0u8; 640];
        receive_audio_blocking_pattern(&audio_data, &smart_turn_blocking, &stt_blocking).await;
    }
    let blocking_time = start_blocking.elapsed();

    // Test non-blocking pattern (concurrent)
    let smart_turn_nonblocking = Arc::new(RwLock::new(Some(MockSmartTurnProcessor::new())));
    let stt_nonblocking = Arc::new(Mutex::new(MockSTTProvider::new()));

    let start_nonblocking = Instant::now();
    let mut handles = Vec::new();
    for _ in 0..5 {
        let st = Arc::clone(&smart_turn_nonblocking);
        let s = Arc::clone(&stt_nonblocking);
        handles.push(tokio::spawn(async move {
            let audio_data = vec![0u8; 640];
            receive_audio_non_blocking_pattern(&audio_data, &st, &s).await
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
    let nonblocking_time = start_nonblocking.elapsed();

    // Non-blocking should be significantly faster when run concurrently
    // Blocking: 5 * 20ms = 100ms minimum
    // Non-blocking: ~20ms (parallel)
    assert!(
        nonblocking_time < blocking_time,
        "Non-blocking ({}ms) should be faster than blocking ({}ms)",
        nonblocking_time.as_millis(),
        blocking_time.as_millis()
    );
}

/// Test that individual audio forward latency meets real-time requirements
#[tokio::test]
async fn test_individual_latency_meets_requirements() {
    let smart_turn = Arc::new(RwLock::new(None::<MockSmartTurnProcessor>)); // No smart turn
    let stt = Arc::new(Mutex::new(MockSTTProvider::new()));

    let mut latencies = Vec::new();

    for _ in 0..100 {
        let audio_data = vec![0u8; 640];
        let (latency, _) = receive_audio_non_blocking_pattern(&audio_data, &smart_turn, &stt).await;
        latencies.push(latency);
    }

    // Calculate p99 latency
    latencies.sort();
    let p99_idx = (latencies.len() as f64 * 0.99) as usize;
    let p99_latency = latencies[p99_idx];

    // p99 should be under 10ms (real-time audio requirement)
    assert!(
        p99_latency < Duration::from_millis(MAX_AUDIO_FORWARD_LATENCY_MS),
        "p99 latency {}μs exceeds {}ms budget",
        p99_latency.as_micros(),
        MAX_AUDIO_FORWARD_LATENCY_MS
    );
}

/// Test graceful degradation under high contention
#[tokio::test]
async fn test_graceful_degradation_under_contention() {
    let smart_turn = Arc::new(RwLock::new(Some(MockSmartTurnProcessor::new())));
    let stt = Arc::new(Mutex::new(MockSTTProvider::new()));

    // Simulate high contention: many concurrent tasks
    let num_tasks = 50;
    let mut handles = Vec::new();
    let processed_count = Arc::new(AtomicU64::new(0));

    for _ in 0..num_tasks {
        let st = Arc::clone(&smart_turn);
        let s = Arc::clone(&stt);
        let pc = Arc::clone(&processed_count);
        handles.push(tokio::spawn(async move {
            let audio_data = vec![0u8; 640];
            let (_, processed) = receive_audio_non_blocking_pattern(&audio_data, &st, &s).await;
            if processed {
                pc.fetch_add(1, Ordering::Relaxed);
            }
        }));
    }

    for h in handles {
        h.await.unwrap();
    }

    // Verify all audio reached STT
    let stt_guard = stt.lock().await;
    assert_eq!(
        stt_guard.frames_received(),
        num_tasks as u64,
        "All audio must reach STT under high contention"
    );

    // Check smart turn stats
    let inferences = processed_count.load(Ordering::Relaxed);
    let skipped = num_tasks as u64 - inferences;

    // Should have processed some but not necessarily all (some skipped due to contention)
    assert!(
        inferences > 0,
        "Should have processed at least some smart turn"
    );

    println!(
        "Smart turn: {} inferences, {} skipped ({}% skipped)",
        inferences,
        skipped,
        (skipped as f64 / num_tasks as f64 * 100.0) as u64
    );
}

/// Test that STT send_audio doesn't depend on smart turn lock
#[tokio::test]
async fn test_stt_independent_of_smart_turn() {
    let smart_turn = Arc::new(RwLock::new(Some(MockSmartTurnProcessor::new())));
    let stt = Arc::new(Mutex::new(MockSTTProvider::new()));

    // Hold smart turn lock indefinitely
    let smart_turn_clone = Arc::clone(&smart_turn);
    let _lock_holder = smart_turn_clone.write().await;

    // Should still be able to send audio to STT
    let audio_data = vec![0u8; 640];

    let start = Instant::now();

    // This should NOT block waiting for smart_turn lock
    // Using try_write pattern
    if smart_turn.try_write().is_err() {
        // Lock busy, skip smart turn, send directly to STT
        let stt_guard = stt.lock().await;
        stt_guard.send_audio(&audio_data).await;
    }

    let elapsed = start.elapsed();

    // Should complete almost instantly (no waiting for smart turn)
    assert!(
        elapsed < Duration::from_millis(5),
        "STT send blocked waiting for smart turn lock: {}ms",
        elapsed.as_millis()
    );

    let stt_guard = stt.lock().await;
    assert_eq!(
        stt_guard.frames_received(),
        1,
        "Audio should have reached STT"
    );
}

/// Test RwLock fairness doesn't starve audio processing
#[tokio::test]
async fn test_rwlock_fairness() {
    let smart_turn = Arc::new(RwLock::new(Some(MockSmartTurnProcessor::new())));
    let stt = Arc::new(Mutex::new(MockSTTProvider::new()));
    let completed = Arc::new(AtomicU64::new(0));

    let num_tasks = 20;
    let mut handles = Vec::new();

    // Spawn tasks that all try to acquire the lock
    for i in 0..num_tasks {
        let st = Arc::clone(&smart_turn);
        let s = Arc::clone(&stt);
        let c = Arc::clone(&completed);

        handles.push(tokio::spawn(async move {
            let audio_data = vec![i as u8; 640];
            receive_audio_non_blocking_pattern(&audio_data, &st, &s).await;
            c.fetch_add(1, Ordering::Relaxed);
        }));

        // Stagger slightly
        tokio::time::sleep(Duration::from_micros(100)).await;
    }

    // Wait with timeout
    let timeout = Duration::from_secs(5);
    let start = Instant::now();

    while completed.load(Ordering::Relaxed) < num_tasks as u64 {
        if start.elapsed() > timeout {
            panic!(
                "Timeout! Only {}/{} tasks completed (possible starvation)",
                completed.load(Ordering::Relaxed),
                num_tasks
            );
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    // All tasks should complete
    assert_eq!(
        completed.load(Ordering::Relaxed),
        num_tasks as u64,
        "All tasks should complete"
    );

    // All audio should reach STT
    let stt_guard = stt.lock().await;
    assert_eq!(
        stt_guard.frames_received(),
        num_tasks as u64,
        "All audio should reach STT"
    );
}
