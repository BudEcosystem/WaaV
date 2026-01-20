# Optimal Implementation Plan for Verified Gaps

**Date**: 2026-01-19
**Author**: Claude (Analysis-driven)
**Scope**: 8 verified gaps from Pipecat comparison

---

## Executive Summary

This document provides a detailed, impact-aware implementation plan for the 8 verified gaps identified in Bud Waav Gateway. Each implementation is designed to:

1. **Minimize Impact** - Changes propagate uniformly without breaking existing providers
2. **Leverage Existing Patterns** - Reuse `ReconnectionConfig`, callback patterns, and trait structures
3. **Maintain Backward Compatibility** - Default implementations prevent breaking changes
4. **Enable Incremental Rollout** - Features can be adopted provider-by-provider

---

## Architecture Principles

### Provider Propagation Strategy

Based on codebase analysis:

| Provider Type | Count | Propagation Method |
|--------------|-------|-------------------|
| HTTP-based STT | 26/31 | Add default impl to `BaseSTT` trait |
| HTTP-based TTS | 26/37 | Add default impl to `BaseTTS` trait → delegates to `TTSProvider` |
| WebSocket STT | 5/31 | Manual implementation required |
| WebSocket/gRPC TTS | 11/37 | Manual implementation required |

**Key Insight**: 70%+ of providers can be updated automatically via trait default implementations.

### Shared Infrastructure Locations

| Component | Location | Purpose |
|-----------|----------|---------|
| `ReconnectionConfig` | `src/core/realtime/base.rs:83-157` | Exponential backoff with jitter |
| `STTConnectionState` | `src/core/stt/base.rs:577-588` | Connection state enum |
| `InterruptionState` | `src/core/voice_manager/state.rs:32-75` | Lock-free interruption control |
| Plugin Registry | `src/plugin/registry.rs` | Provider factory pattern |

---

## Gap 1: Observer Pattern (P0 - Critical)

### Current State
- Single callback per event type (`STTCallback`, `TTSAudioCallback`)
- Tightly coupled to VoiceManager
- No multi-observer support

### Impact Analysis

| File | Lines | Impact | Risk |
|------|-------|--------|------|
| `src/core/voice_manager/manager.rs` | 37-63, 528-908 | Modify callback fields | Medium |
| `src/core/voice_manager/callbacks.rs` | 1-87 | Add Observer trait | Low |
| All consumers of VoiceManager | Various | Must adopt new registration API | High if breaking |

### Optimal Implementation

**Step 1: Create Observer Infrastructure** (`src/core/observability/mod.rs`)

```rust
// src/core/observability/mod.rs
use std::sync::Arc;
use parking_lot::RwLock;
use uuid::Uuid;

/// Observer trait for non-intrusive monitoring
pub trait VoiceObserver: Send + Sync {
    /// Called when STT result is received
    fn on_stt_result(&self, _result: &STTResult, _latency_ns: u64) {}

    /// Called when TTS audio chunk is delivered
    fn on_tts_chunk(&self, _chunk: &AudioData, _ttfb_ns: Option<u64>) {}

    /// Called when TTS completes
    fn on_tts_complete(&self, _total_duration_ms: u64) {}

    /// Called on connection state change
    fn on_connection_state(&self, _provider: &str, _old: ConnectionState, _new: ConnectionState) {}

    /// Called on error
    fn on_error(&self, _provider: &str, _error: &dyn std::error::Error) {}

    /// Called when bot starts speaking (first TTS chunk)
    fn on_bot_speaking_started(&self, _timestamp_ns: u64) {}

    /// Called when bot stops speaking (silence detected)
    fn on_bot_speaking_stopped(&self, _duration_ms: u64) {}
}

/// Registry for managing multiple observers
pub struct ObserverRegistry {
    observers: RwLock<Vec<(Uuid, Arc<dyn VoiceObserver>)>>,
}

impl ObserverRegistry {
    pub fn new() -> Self {
        Self {
            observers: RwLock::new(Vec::with_capacity(4)),
        }
    }

    /// Register an observer, returns ID for unregistration
    pub fn register(&self, observer: Arc<dyn VoiceObserver>) -> Uuid {
        let id = Uuid::new_v4();
        self.observers.write().push((id, observer));
        id
    }

    /// Unregister an observer by ID
    pub fn unregister(&self, id: Uuid) -> bool {
        let mut observers = self.observers.write();
        if let Some(pos) = observers.iter().position(|(oid, _)| *oid == id) {
            observers.remove(pos);
            true
        } else {
            false
        }
    }

    /// Notify all observers of STT result (inline, no allocation)
    #[inline]
    pub fn notify_stt_result(&self, result: &STTResult, latency_ns: u64) {
        for (_, observer) in self.observers.read().iter() {
            observer.on_stt_result(result, latency_ns);
        }
    }

    // ... similar notify_* methods for each event
}

impl Default for ObserverRegistry {
    fn default() -> Self {
        Self::new()
    }
}
```

**Step 2: Integrate into VoiceManager** (`src/core/voice_manager/manager.rs`)

```rust
// Add to VoiceManager struct (line ~45)
observer_registry: Arc<ObserverRegistry>,

// Modify on_stt_result wrapper (line ~562-588)
// BEFORE invoking user callback:
let latency_ns = segment_start_time.elapsed().as_nanos() as u64;
self.observer_registry.notify_stt_result(&result, latency_ns);

// THEN invoke user callback as before
if let Some(callback) = self.stt_callback.read().as_ref() {
    callback(result.clone()).await;
}
```

**Step 3: Backward Compatibility**

```rust
// Keep existing on_stt_result() API unchanged
pub async fn on_stt_result<F>(&self, callback: F) -> VoiceManagerResult<()>
where
    F: Fn(STTResult) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync + 'static,
{
    // Existing implementation unchanged
    *self.stt_callback.write() = Some(Arc::new(callback));
    Ok(())
}

// ADD new observer registration method
pub fn register_observer(&self, observer: Arc<dyn VoiceObserver>) -> Uuid {
    self.observer_registry.register(observer)
}

pub fn unregister_observer(&self, id: Uuid) -> bool {
    self.observer_registry.unregister(id)
}
```

### Mitigation Strategy

| Risk | Mitigation |
|------|------------|
| Performance overhead from observer iteration | Use `parking_lot::RwLock` (faster than std), inline notify methods |
| Breaking existing callback API | Keep existing API, add observer as additional path |
| Memory growth from registered observers | Use `Vec` with pre-allocated capacity (4), not unbounded |

### Testing Checklist

- [ ] Unit test: Observer registration/unregistration
- [ ] Unit test: Multiple observers receive same event
- [ ] Unit test: Observer removal during iteration (no panic)
- [ ] Integration test: Observer + existing callback coexist
- [ ] Performance test: Measure overhead with 0, 1, 4 observers

---

## Gap 2: User-to-Bot Latency Tracking (P0 - Critical)

### Current State
- `segment_start_ms` tracked in `SpeechFinalState` (`state.rs:27`)
- `turn_detection_last_fired_ms` available (`state.rs:22`)
- No aggregation or reporting

### Impact Analysis

| File | Lines | Impact | Risk |
|------|-------|--------|------|
| `src/core/observability/latency.rs` | New file | Create latency observer | None |
| `src/core/voice_manager/manager.rs` | ~700-745 | Capture TTS start time | Low |

### Optimal Implementation

**Step 1: Create Latency Observer** (`src/core/observability/latency.rs`)

```rust
use std::sync::atomic::{AtomicU64, Ordering};
use std::collections::VecDeque;
use parking_lot::Mutex;

/// Tracks user-to-bot latency with rolling window
pub struct UserBotLatencyObserver {
    /// Timestamp when user stopped speaking (speech_final fired)
    user_stopped_ns: AtomicU64,

    /// Rolling window of latency samples (for p50/p99)
    latencies_ns: Mutex<VecDeque<u64>>,

    /// Maximum samples to keep
    max_samples: usize,

    /// Running sum for average calculation
    sum_ns: AtomicU64,
    count: AtomicU64,
}

impl UserBotLatencyObserver {
    pub fn new(max_samples: usize) -> Self {
        Self {
            user_stopped_ns: AtomicU64::new(0),
            latencies_ns: Mutex::new(VecDeque::with_capacity(max_samples)),
            max_samples,
            sum_ns: AtomicU64::new(0),
            count: AtomicU64::new(0),
        }
    }

    /// Get current metrics
    pub fn get_metrics(&self) -> LatencyMetrics {
        let latencies = self.latencies_ns.lock();
        let count = self.count.load(Ordering::Relaxed);

        if count == 0 {
            return LatencyMetrics::default();
        }

        let avg_ns = self.sum_ns.load(Ordering::Relaxed) / count;

        // Calculate percentiles from sorted copy
        let mut sorted: Vec<u64> = latencies.iter().copied().collect();
        sorted.sort_unstable();

        let p50 = sorted.get(sorted.len() / 2).copied().unwrap_or(0);
        let p99_idx = (sorted.len() as f64 * 0.99) as usize;
        let p99 = sorted.get(p99_idx.min(sorted.len().saturating_sub(1))).copied().unwrap_or(0);

        LatencyMetrics {
            avg_ms: (avg_ns / 1_000_000) as u32,
            p50_ms: (p50 / 1_000_000) as u32,
            p99_ms: (p99 / 1_000_000) as u32,
            sample_count: count,
        }
    }
}

impl VoiceObserver for UserBotLatencyObserver {
    fn on_stt_result(&self, result: &STTResult, _latency_ns: u64) {
        // Record when user stopped speaking
        if result.is_speech_final || result.is_final {
            let now_ns = std::time::Instant::now().elapsed().as_nanos() as u64;
            self.user_stopped_ns.store(now_ns, Ordering::Release);
        }
    }

    fn on_tts_chunk(&self, _chunk: &AudioData, ttfb_ns: Option<u64>) {
        // On FIRST TTS chunk (ttfb_ns is Some), calculate latency
        if let Some(_ttfb) = ttfb_ns {
            let user_stopped = self.user_stopped_ns.load(Ordering::Acquire);
            if user_stopped > 0 {
                let now_ns = std::time::Instant::now().elapsed().as_nanos() as u64;
                let latency = now_ns.saturating_sub(user_stopped);

                // Record sample
                let mut latencies = self.latencies_ns.lock();
                if latencies.len() >= self.max_samples {
                    if let Some(old) = latencies.pop_front() {
                        self.sum_ns.fetch_sub(old, Ordering::Relaxed);
                        self.count.fetch_sub(1, Ordering::Relaxed);
                    }
                }
                latencies.push_back(latency);
                self.sum_ns.fetch_add(latency, Ordering::Relaxed);
                self.count.fetch_add(1, Ordering::Relaxed);

                // Reset for next interaction
                self.user_stopped_ns.store(0, Ordering::Release);
            }
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct LatencyMetrics {
    pub avg_ms: u32,
    pub p50_ms: u32,
    pub p99_ms: u32,
    pub sample_count: u64,
}
```

**Step 2: Integrate with VoiceManager**

```rust
// In VoiceManager::new() or start()
let latency_observer = Arc::new(UserBotLatencyObserver::new(1000));
self.observer_registry.register(latency_observer.clone());

// Expose metrics via health endpoint
pub fn get_latency_metrics(&self) -> LatencyMetrics {
    self.latency_observer.get_metrics()
}
```

### Dependencies
- **Requires**: Gap 1 (Observer Pattern)

### Testing Checklist
- [ ] Unit test: Latency calculation accuracy
- [ ] Unit test: Rolling window eviction
- [ ] Unit test: Percentile calculations
- [ ] Integration test: End-to-end latency measurement

---

## Gap 3: Signal Handling (P0 - Critical)

### Current State
- **No signal handling** in `src/main.rs`
- Server blocks indefinitely on `axum::serve()` or `axum_server::bind_rustls().serve()`
- Component Drop implementations exist but require explicit call

### Impact Analysis

| File | Lines | Impact | Risk |
|------|-------|--------|------|
| `src/main.rs` | 308-339 | Replace serve with select! | Low |
| `src/state/mod.rs` | 22-40 | Add shutdown coordination | Low |
| `src/handlers/ws/handler.rs` | 114-250 | Add shutdown broadcast | Medium |
| All WebSocket connections | Various | Must handle graceful close | Medium |

### Optimal Implementation

**Step 1: Add Shutdown Coordination to AppState** (`src/state/mod.rs`)

```rust
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

// Add to AppState struct
pub struct AppState {
    // ... existing fields ...

    /// Global cancellation token for graceful shutdown
    pub shutdown_token: CancellationToken,

    /// Broadcast channel for shutdown notification
    pub shutdown_tx: broadcast::Sender<()>,
}

impl AppState {
    pub fn new(config: ServerConfig) -> anyhow::Result<Arc<Self>> {
        let shutdown_token = CancellationToken::new();
        let (shutdown_tx, _) = broadcast::channel::<()>(1);

        // ... existing initialization ...

        Ok(Arc::new(Self {
            // ... existing fields ...
            shutdown_token,
            shutdown_tx,
        }))
    }

    /// Initiate graceful shutdown
    pub async fn shutdown(&self) {
        info!("Initiating graceful shutdown...");

        // 1. Signal all components
        self.shutdown_token.cancel();
        let _ = self.shutdown_tx.send(());

        // 2. Wait for WebSocket connections to drain (with timeout)
        let drain_timeout = Duration::from_secs(30);
        let start = Instant::now();
        while self.active_ws_connections.load(Ordering::Relaxed) > 0 {
            if start.elapsed() > drain_timeout {
                warn!("Shutdown timeout: {} connections still active",
                      self.active_ws_connections.load(Ordering::Relaxed));
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        // 3. Cleanup LiveKit if present
        if let Some(ref livekit) = self.livekit_room_handler {
            // LiveKit has its own shutdown
        }

        info!("Graceful shutdown complete");
    }
}
```

**Step 2: Modify main.rs** (`src/main.rs`)

```rust
use tokio::signal;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // ... existing setup (lines 69-305) ...

    // Create shutdown signal future
    let shutdown_signal = async {
        let ctrl_c = async {
            signal::ctrl_c()
                .await
                .expect("Failed to install Ctrl+C handler");
        };

        #[cfg(unix)]
        let terminate = async {
            signal::unix::signal(signal::unix::SignalKind::terminate())
                .expect("Failed to install SIGTERM handler")
                .recv()
                .await;
        };

        #[cfg(not(unix))]
        let terminate = std::future::pending::<()>();

        tokio::select! {
            _ = ctrl_c => info!("Received Ctrl+C, initiating graceful shutdown"),
            _ = terminate => info!("Received SIGTERM, initiating graceful shutdown"),
        }
    };

    // Clone app_state for shutdown handler
    let shutdown_app_state = app_state.clone();

    // Start server with graceful shutdown
    let socket_addr: SocketAddr = format!("{}:{}", host, port).parse()?;

    if let Some(tls_config) = tls_config {
        // TLS server
        let rustls_config = /* ... existing TLS setup ... */;

        tokio::select! {
            result = axum_server::bind_rustls(socket_addr, rustls_config)
                .serve(app.into_make_service_with_connect_info::<SocketAddr>()) => {
                if let Err(e) = result {
                    error!("Server error: {}", e);
                }
            }
            _ = shutdown_signal => {
                shutdown_app_state.shutdown().await;
            }
        }
    } else {
        // HTTP server
        let listener = TcpListener::bind(socket_addr).await?;

        tokio::select! {
            result = axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>()
            ) => {
                if let Err(e) = result {
                    error!("Server error: {}", e);
                }
            }
            _ = shutdown_signal => {
                shutdown_app_state.shutdown().await;
            }
        }
    }

    info!("Server shutdown complete");
    Ok(())
}
```

**Step 3: Update WebSocket Handler** (`src/handlers/ws/handler.rs`)

```rust
// Add shutdown subscription in handle_websocket (around line 130)
let mut shutdown_rx = app_state.shutdown_tx.subscribe();

// Modify the receiver loop (around line 210-240)
loop {
    tokio::select! {
        biased; // Check shutdown first

        _ = shutdown_rx.recv() => {
            // Send close frame to client
            if let Err(e) = sender.send(Message::Close(Some(CloseFrame {
                code: CloseCode::Away,
                reason: "Server shutting down".into(),
            }))).await {
                debug!("Failed to send close frame: {}", e);
            }
            break;
        }

        msg = receiver.next() => {
            match msg {
                Some(Ok(msg)) => { /* existing handling */ }
                Some(Err(e)) => { break; }
                None => { break; }
            }
        }
    }
}
```

### Testing Checklist
- [ ] Manual test: Ctrl+C triggers graceful shutdown
- [ ] Manual test: SIGTERM triggers graceful shutdown
- [ ] Integration test: Active WebSocket connections receive close frame
- [ ] Integration test: Server drains requests before exit

---

## Gap 4: Bot Speaking Detection (P1 - High)

### Current State
- `is_completed` flag in `InterruptionState` (set true after TTS complete)
- No tracking of when bot **starts** speaking
- No silence detection for bot output

### Impact Analysis

| File | Lines | Impact | Risk |
|------|-------|--------|------|
| `src/core/voice_manager/manager.rs` | 700-745 | Track first chunk arrival | Low |
| `src/core/voice_manager/state.rs` | New additions | Add speaking state | Low |
| Observer integration | Via registry | Notify observers | Low |

### Optimal Implementation

**Step 1: Add Speaking State** (`src/core/voice_manager/state.rs`)

```rust
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Tracks bot speaking state with atomic operations (lock-free hot path)
pub struct BotSpeakingState {
    /// Is bot currently speaking?
    is_speaking: AtomicBool,

    /// Timestamp of last audio chunk (nanoseconds since boot)
    last_audio_ns: AtomicU64,

    /// Timestamp when speaking started
    speaking_start_ns: AtomicU64,

    /// Silence threshold to consider bot stopped (nanoseconds)
    silence_threshold_ns: u64,
}

impl BotSpeakingState {
    pub fn new(silence_threshold_ms: u64) -> Self {
        Self {
            is_speaking: AtomicBool::new(false),
            last_audio_ns: AtomicU64::new(0),
            speaking_start_ns: AtomicU64::new(0),
            silence_threshold_ns: silence_threshold_ms * 1_000_000,
        }
    }

    /// Called when audio chunk is sent to output
    /// Returns Some(start_event) if this is the first chunk
    #[inline]
    pub fn on_audio_sent(&self) -> Option<BotSpeakingStarted> {
        let now_ns = now_monotonic_ns();

        let was_speaking = self.is_speaking.swap(true, Ordering::AcqRel);
        self.last_audio_ns.store(now_ns, Ordering::Release);

        if !was_speaking {
            self.speaking_start_ns.store(now_ns, Ordering::Release);
            Some(BotSpeakingStarted { timestamp_ns: now_ns })
        } else {
            None
        }
    }

    /// Check if bot has stopped speaking (silence exceeded threshold)
    /// Call this periodically (e.g., every 50ms)
    #[inline]
    pub fn check_silence(&self) -> Option<BotSpeakingStopped> {
        if !self.is_speaking.load(Ordering::Acquire) {
            return None;
        }

        let last = self.last_audio_ns.load(Ordering::Acquire);
        let now_ns = now_monotonic_ns();
        let silence_duration = now_ns.saturating_sub(last);

        if silence_duration > self.silence_threshold_ns {
            self.is_speaking.store(false, Ordering::Release);
            let start = self.speaking_start_ns.load(Ordering::Acquire);
            Some(BotSpeakingStopped {
                speaking_duration_ns: last.saturating_sub(start),
            })
        } else {
            None
        }
    }

    /// Reset state (e.g., on TTS clear)
    pub fn reset(&self) {
        self.is_speaking.store(false, Ordering::Release);
        self.last_audio_ns.store(0, Ordering::Release);
        self.speaking_start_ns.store(0, Ordering::Release);
    }

    /// Fast check if bot is speaking
    #[inline]
    pub fn is_speaking(&self) -> bool {
        self.is_speaking.load(Ordering::Acquire)
    }
}

#[derive(Debug, Clone)]
pub struct BotSpeakingStarted {
    pub timestamp_ns: u64,
}

#[derive(Debug, Clone)]
pub struct BotSpeakingStopped {
    pub speaking_duration_ns: u64,
}

#[inline]
fn now_monotonic_ns() -> u64 {
    // Use monotonic time for accurate duration measurements
    use std::time::Instant;
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let start = START.get_or_init(Instant::now);
    start.elapsed().as_nanos() as u64
}
```

**Step 2: Integrate into VoiceManager**

```rust
// Add to VoiceManager struct
bot_speaking_state: Arc<BotSpeakingState>,

// In TTS audio callback (manager.rs ~700-745)
// After existing interruption timing update:
if let Some(start_event) = self.bot_speaking_state.on_audio_sent() {
    self.observer_registry.notify_bot_speaking_started(start_event.timestamp_ns);
}

// Create background task for silence detection
fn spawn_silence_detector(
    bot_speaking_state: Arc<BotSpeakingState>,
    observer_registry: Arc<ObserverRegistry>,
    shutdown_token: CancellationToken,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(50));
        loop {
            tokio::select! {
                _ = shutdown_token.cancelled() => break,
                _ = interval.tick() => {
                    if let Some(stop_event) = bot_speaking_state.check_silence() {
                        observer_registry.notify_bot_speaking_stopped(
                            stop_event.speaking_duration_ns / 1_000_000
                        );
                    }
                }
            }
        }
    })
}
```

### Configuration

```rust
// In VoiceManagerConfig
pub struct VoiceManagerConfig {
    // ... existing fields ...

    /// Silence threshold to consider bot stopped speaking (milliseconds)
    /// Default: 350ms (matches Pipecat's BOT_VAD_STOP_SECS = 0.35)
    pub bot_silence_threshold_ms: u64,
}

impl Default for VoiceManagerConfig {
    fn default() -> Self {
        Self {
            // ... existing defaults ...
            bot_silence_threshold_ms: 350,
        }
    }
}
```

### Dependencies
- **Requires**: Gap 1 (Observer Pattern) for notifications
- **Integrates with**: Gap 2 (User-Bot Latency) for end-to-end measurement

---

## Gap 5: VAD State Machine (P1 - High)

### Current State
- Provider-side VAD (Deepgram, Azure, etc.)
- Optional ONNX turn detection (feature-gated)
- No local VAD state machine with debouncing

### Impact Analysis

| File | Lines | Impact | Risk |
|------|-------|--------|------|
| `src/core/audio/vad.rs` | New file | Create VAD state machine | None |
| `src/core/voice_manager/manager.rs` | Audio receive path | Optional VAD processing | Low |

### Optimal Implementation

**Step 1: Create VAD State Machine** (`src/core/audio/vad.rs`)

```rust
//! Local VAD (Voice Activity Detection) state machine with configurable debouncing.
//!
//! This provides client-side VAD independent of STT provider, useful for:
//! - Faster interruption detection
//! - Provider-agnostic speech boundary detection
//! - Custom debouncing behavior

use std::sync::atomic::{AtomicU32, Ordering};

/// VAD state transitions: QUIET → STARTING → SPEAKING → STOPPING → QUIET
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum VADState {
    /// No speech detected
    Quiet = 0,
    /// Speech beginning, within debounce window
    Starting = 1,
    /// Confirmed speech in progress
    Speaking = 2,
    /// Speech ending, within debounce window
    Stopping = 3,
}

impl From<u8> for VADState {
    fn from(v: u8) -> Self {
        match v {
            0 => VADState::Quiet,
            1 => VADState::Starting,
            2 => VADState::Speaking,
            3 => VADState::Stopping,
            _ => VADState::Quiet,
        }
    }
}

/// Configuration for VAD behavior
#[derive(Debug, Clone)]
pub struct VADParams {
    /// Voice confidence threshold (0.0 to 1.0)
    pub confidence_threshold: f32,

    /// Minimum consecutive frames to confirm speech start
    pub start_debounce_frames: u32,

    /// Minimum consecutive frames to confirm speech end
    pub stop_debounce_frames: u32,

    /// Minimum audio volume to consider (0.0 to 1.0)
    pub min_volume: f32,
}

impl Default for VADParams {
    fn default() -> Self {
        Self {
            confidence_threshold: 0.7,
            start_debounce_frames: 4,   // ~200ms at 20fps
            stop_debounce_frames: 16,   // ~800ms at 20fps
            min_volume: 0.6,
        }
    }
}

/// VAD analyzer with state machine
pub struct VADAnalyzer {
    state: VADState,
    params: VADParams,
    starting_count: u32,
    stopping_count: u32,
}

impl VADAnalyzer {
    pub fn new(params: VADParams) -> Self {
        Self {
            state: VADState::Quiet,
            params,
            starting_count: 0,
            stopping_count: 0,
        }
    }

    /// Process a VAD frame and return state transition if any
    ///
    /// # Arguments
    /// * `confidence` - Voice confidence score (0.0 to 1.0)
    /// * `volume` - Audio volume/RMS (0.0 to 1.0)
    ///
    /// # Returns
    /// * Tuple of (current_state, Some(transition_event) if state changed)
    pub fn analyze(&mut self, confidence: f32, volume: f32) -> (VADState, Option<VADTransition>) {
        let is_speaking = confidence >= self.params.confidence_threshold
            && volume >= self.params.min_volume;

        let old_state = self.state;

        match (self.state, is_speaking) {
            // QUIET + voice → start debouncing
            (VADState::Quiet, true) => {
                self.state = VADState::Starting;
                self.starting_count = 1;
            }

            // STARTING + voice → increment counter, maybe transition
            (VADState::Starting, true) => {
                self.starting_count += 1;
                if self.starting_count >= self.params.start_debounce_frames {
                    self.state = VADState::Speaking;
                    return (self.state, Some(VADTransition::SpeechStarted));
                }
            }

            // STARTING + no voice → reset to quiet
            (VADState::Starting, false) => {
                self.state = VADState::Quiet;
                self.starting_count = 0;
            }

            // SPEAKING + no voice → start stopping debounce
            (VADState::Speaking, false) => {
                self.state = VADState::Stopping;
                self.stopping_count = 1;
            }

            // STOPPING + no voice → increment counter, maybe transition
            (VADState::Stopping, false) => {
                self.stopping_count += 1;
                if self.stopping_count >= self.params.stop_debounce_frames {
                    self.state = VADState::Quiet;
                    return (self.state, Some(VADTransition::SpeechEnded));
                }
            }

            // STOPPING + voice → back to speaking
            (VADState::Stopping, true) => {
                self.state = VADState::Speaking;
                self.stopping_count = 0;
            }

            // QUIET + no voice, SPEAKING + voice → no change
            _ => {}
        }

        (self.state, None)
    }

    /// Get current state
    pub fn state(&self) -> VADState {
        self.state
    }

    /// Reset to quiet state
    pub fn reset(&mut self) {
        self.state = VADState::Quiet;
        self.starting_count = 0;
        self.stopping_count = 0;
    }
}

/// State transition events
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VADTransition {
    /// Speech started (after debounce confirmation)
    SpeechStarted,
    /// Speech ended (after debounce confirmation)
    SpeechEnded,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_state_transitions() {
        let mut vad = VADAnalyzer::new(VADParams {
            confidence_threshold: 0.5,
            start_debounce_frames: 2,
            stop_debounce_frames: 3,
            min_volume: 0.1,
        });

        // Start quiet
        assert_eq!(vad.state(), VADState::Quiet);

        // First voice frame → Starting
        let (state, trans) = vad.analyze(0.8, 0.5);
        assert_eq!(state, VADState::Starting);
        assert!(trans.is_none());

        // Second voice frame → Speaking (debounce complete)
        let (state, trans) = vad.analyze(0.8, 0.5);
        assert_eq!(state, VADState::Speaking);
        assert_eq!(trans, Some(VADTransition::SpeechStarted));

        // Silence frames → Stopping
        let (state, _) = vad.analyze(0.2, 0.5);
        assert_eq!(state, VADState::Stopping);

        // More silence → still Stopping
        let (state, _) = vad.analyze(0.2, 0.5);
        assert_eq!(state, VADState::Stopping);

        // Third silence → Quiet (debounce complete)
        let (state, trans) = vad.analyze(0.2, 0.5);
        assert_eq!(state, VADState::Quiet);
        assert_eq!(trans, Some(VADTransition::SpeechEnded));
    }
}
```

**Step 2: Optional Integration with VoiceManager**

```rust
// In VoiceManagerConfig
pub struct VoiceManagerConfig {
    // ... existing fields ...

    /// Enable local VAD state machine (optional, provider VAD takes precedence)
    pub local_vad_enabled: bool,

    /// Local VAD parameters
    pub local_vad_params: Option<VADParams>,
}

// In VoiceManager
pub struct VoiceManager {
    // ... existing fields ...

    /// Optional local VAD analyzer
    vad_analyzer: Option<Mutex<VADAnalyzer>>,
}

// In receive_audio() - optional VAD processing
if let Some(ref vad) = self.vad_analyzer {
    // Extract confidence/volume from audio (requires WebRTC VAD or similar)
    let confidence = extract_voice_confidence(&audio_data);
    let volume = calculate_rms(&audio_data);

    let (state, transition) = vad.lock().analyze(confidence, volume);
    if let Some(trans) = transition {
        match trans {
            VADTransition::SpeechStarted => {
                self.observer_registry.notify_local_vad_speech_started();
            }
            VADTransition::SpeechEnded => {
                self.observer_registry.notify_local_vad_speech_ended();
            }
        }
    }
}
```

### Dependencies
- **Optional**: WebRTC VAD or similar for confidence extraction
- **Integrates with**: Gap 1 (Observer Pattern)

---

## Gap 6: TTFB Metrics (P1 - High)

### Current State
- No time-to-first-byte tracking for STT/TTS providers
- `QueueStats` exists in LiveKit operations only

### Impact Analysis

| File | Lines | Impact | Risk |
|------|-------|--------|------|
| `src/core/metrics/provider_metrics.rs` | New file | Create metrics infrastructure | None |
| `src/core/tts/provider.rs` | ~300-350 | Track TTFB in send_request | Low |
| `src/core/stt/*/client.rs` | Various | Track TTFB per provider | Medium (31 files) |

### Optimal Implementation

**Step 1: Create Provider Metrics** (`src/core/metrics/provider_metrics.rs`)

```rust
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use parking_lot::RwLock;

/// Metrics for a single provider
pub struct ProviderMetrics {
    provider_name: String,

    // Request counts
    request_count: AtomicU64,
    error_count: AtomicU64,

    // TTFB tracking (nanoseconds)
    ttfb_sum_ns: AtomicU64,
    ttfb_count: AtomicU64,
    ttfb_max_ns: AtomicU64,
    ttfb_min_ns: AtomicU64,

    // Total processing time
    processing_sum_ns: AtomicU64,
}

impl ProviderMetrics {
    pub fn new(provider_name: &str) -> Self {
        Self {
            provider_name: provider_name.to_string(),
            request_count: AtomicU64::new(0),
            error_count: AtomicU64::new(0),
            ttfb_sum_ns: AtomicU64::new(0),
            ttfb_count: AtomicU64::new(0),
            ttfb_max_ns: AtomicU64::new(0),
            ttfb_min_ns: AtomicU64::new(u64::MAX),
            processing_sum_ns: AtomicU64::new(0),
        }
    }

    /// Start timing a request
    pub fn start_request(&self) -> RequestTimer<'_> {
        self.request_count.fetch_add(1, Ordering::Relaxed);
        RequestTimer {
            metrics: self,
            start_time: Instant::now(),
            first_byte_time: None,
        }
    }

    /// Get current metrics snapshot
    pub fn snapshot(&self) -> ProviderMetricsSnapshot {
        let ttfb_count = self.ttfb_count.load(Ordering::Relaxed);
        let ttfb_avg_ms = if ttfb_count > 0 {
            (self.ttfb_sum_ns.load(Ordering::Relaxed) / ttfb_count / 1_000_000) as u32
        } else {
            0
        };

        ProviderMetricsSnapshot {
            provider_name: self.provider_name.clone(),
            request_count: self.request_count.load(Ordering::Relaxed),
            error_count: self.error_count.load(Ordering::Relaxed),
            ttfb_avg_ms,
            ttfb_max_ms: (self.ttfb_max_ns.load(Ordering::Relaxed) / 1_000_000) as u32,
            ttfb_min_ms: {
                let min = self.ttfb_min_ns.load(Ordering::Relaxed);
                if min == u64::MAX { 0 } else { (min / 1_000_000) as u32 }
            },
        }
    }

    /// Record an error
    pub fn record_error(&self) {
        self.error_count.fetch_add(1, Ordering::Relaxed);
    }
}

/// Timer for a single request
pub struct RequestTimer<'a> {
    metrics: &'a ProviderMetrics,
    start_time: Instant,
    first_byte_time: Option<Instant>,
}

impl<'a> RequestTimer<'a> {
    /// Record time to first byte
    pub fn record_first_byte(&mut self) {
        if self.first_byte_time.is_none() {
            self.first_byte_time = Some(Instant::now());
        }
    }

    /// Finish timing and record metrics
    pub fn finish(self) {
        let total_ns = self.start_time.elapsed().as_nanos() as u64;
        self.metrics.processing_sum_ns.fetch_add(total_ns, Ordering::Relaxed);

        if let Some(first_byte) = self.first_byte_time {
            let ttfb_ns = (first_byte - self.start_time).as_nanos() as u64;

            self.metrics.ttfb_sum_ns.fetch_add(ttfb_ns, Ordering::Relaxed);
            self.metrics.ttfb_count.fetch_add(1, Ordering::Relaxed);

            // Update max (using compare-exchange loop)
            loop {
                let current_max = self.metrics.ttfb_max_ns.load(Ordering::Relaxed);
                if ttfb_ns <= current_max {
                    break;
                }
                if self.metrics.ttfb_max_ns
                    .compare_exchange_weak(current_max, ttfb_ns, Ordering::Relaxed, Ordering::Relaxed)
                    .is_ok()
                {
                    break;
                }
            }

            // Update min
            loop {
                let current_min = self.metrics.ttfb_min_ns.load(Ordering::Relaxed);
                if ttfb_ns >= current_min {
                    break;
                }
                if self.metrics.ttfb_min_ns
                    .compare_exchange_weak(current_min, ttfb_ns, Ordering::Relaxed, Ordering::Relaxed)
                    .is_ok()
                {
                    break;
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProviderMetricsSnapshot {
    pub provider_name: String,
    pub request_count: u64,
    pub error_count: u64,
    pub ttfb_avg_ms: u32,
    pub ttfb_max_ms: u32,
    pub ttfb_min_ms: u32,
}
```

**Step 2: Integrate with TTS Provider** (`src/core/tts/provider.rs`)

```rust
// Add to TTSProvider struct
pub struct TTSProvider {
    // ... existing fields ...
    metrics: Arc<ProviderMetrics>,
}

// In send_request_dyn (around line 300)
pub async fn send_request_dyn(
    builder: &dyn TTSRequestBuilder,
    req_manager: Arc<ReqManager>,
    text: &str,
    sender: mpsc::Sender<Result<Vec<u8>, TTSError>>,
    cancel_token: CancellationToken,
    cache_and_key: Option<(Arc<CacheStore>, String)>,
    previous_text_store: Arc<RwLock<Option<String>>>,
    metrics: Arc<ProviderMetrics>,  // NEW PARAMETER
) -> Result<(), TTSError> {
    let mut timer = metrics.start_request();

    // ... existing request setup ...

    let response = match timeout(Duration::from_secs(60), request.send()).await {
        Ok(Ok(resp)) => {
            timer.record_first_byte(); // TTFB captured here
            resp
        }
        Ok(Err(e)) => {
            metrics.record_error();
            return Err(TTSError::NetworkError(e.to_string()));
        }
        Err(_) => {
            metrics.record_error();
            return Err(TTSError::TimeoutError("Request timed out".to_string()));
        }
    };

    // ... rest of implementation ...

    timer.finish();
    Ok(())
}
```

**Step 3: Add to BaseSTT Trait** (with default implementation)

```rust
// In src/core/stt/base.rs
pub trait BaseSTT: Send + Sync {
    // ... existing methods ...

    /// Get provider metrics (optional, default returns None)
    fn get_metrics(&self) -> Option<&ProviderMetrics> {
        None
    }
}
```

### Propagation Strategy

For **TTS providers (26 HTTP-based)**:
- Add `metrics` field to `TTSProvider` struct
- All HTTP providers automatically get metrics via delegation

For **STT providers (31 total)**:
- Add `metrics` field to base struct
- Implement TTFB tracking in WebSocket `on_message` handler
- First transcription result = TTFB

---

## Gap 7: STT Reconnection (P1 - High)

### Current State
- `ReconnectionConfig` exists at `src/core/realtime/base.rs:83-157`
- Used by OpenAI Realtime and Hume providers
- **NOT used by STT providers** (only keepalive, no auto-reconnect)

### Impact Analysis

| File | Lines | Impact | Risk |
|------|-------|--------|------|
| `src/core/stt/base.rs` | 480-556 | Add reconnection methods to trait | Low |
| `src/core/websocket/reconnection.rs` | New file | Move ReconnectionConfig to shared location | Low |
| 31 STT providers | Various | Incremental adoption | Medium |

### Optimal Implementation

**Step 1: Move ReconnectionConfig to Shared Location**

```bash
# Move (or copy for now to avoid breaking realtime)
cp src/core/realtime/base.rs::ReconnectionConfig → src/core/websocket/reconnection.rs
```

```rust
// src/core/websocket/mod.rs
pub mod reconnection;
pub use reconnection::ReconnectionConfig;

// src/core/websocket/reconnection.rs
// Copy ReconnectionConfig from realtime/base.rs (lines 82-171)
// This is the same implementation, just in a shared location
```

**Step 2: Add to BaseSTT Trait** (with default implementation)

```rust
// In src/core/stt/base.rs
use crate::core::websocket::ReconnectionConfig;

pub trait BaseSTT: Send + Sync {
    // ... existing methods ...

    /// Get reconnection configuration (default: disabled)
    fn reconnection_config(&self) -> Option<&ReconnectionConfig> {
        None
    }

    /// Set reconnection configuration
    fn set_reconnection_config(&mut self, _config: ReconnectionConfig) {
        // Default: no-op (provider doesn't support reconnection)
    }

    /// Attempt reconnection with backoff
    /// Returns true if reconnected, false if should give up
    async fn try_reconnect(&mut self) -> Result<bool, STTError> {
        // Default implementation using ReconnectionConfig
        let config = match self.reconnection_config() {
            Some(c) if c.enabled => c.clone(),
            _ => return Ok(false), // Reconnection disabled
        };

        let mut attempt = 0u32;
        while config.should_retry(attempt) {
            attempt += 1;
            let delay = config.calculate_delay(attempt);

            tracing::info!(
                provider = self.get_provider_info(),
                attempt = attempt,
                delay_ms = delay,
                "Attempting reconnection"
            );

            tokio::time::sleep(Duration::from_millis(delay)).await;

            match self.connect().await {
                Ok(()) => {
                    tracing::info!(
                        provider = self.get_provider_info(),
                        attempt = attempt,
                        "Reconnection successful"
                    );
                    return Ok(true);
                }
                Err(e) => {
                    tracing::warn!(
                        provider = self.get_provider_info(),
                        attempt = attempt,
                        error = %e,
                        "Reconnection attempt failed"
                    );
                }
            }
        }

        tracing::error!(
            provider = self.get_provider_info(),
            max_attempts = config.max_attempts,
            "Reconnection failed after max attempts"
        );
        Ok(false)
    }
}
```

**Step 3: Implement in Deepgram (Example)**

```rust
// src/core/stt/deepgram.rs

pub struct DeepgramSTT {
    // ... existing fields ...
    reconnection_config: Option<ReconnectionConfig>,
}

impl DeepgramSTT {
    /// Handle connection error with automatic reconnection
    async fn handle_connection_error(&mut self, error: STTError) -> Result<(), STTError> {
        // Notify error callback
        if let Some(callback) = self.error_callback.lock().await.as_ref() {
            callback(error.clone()).await;
        }

        // Attempt reconnection
        match self.try_reconnect().await {
            Ok(true) => {
                // Re-register callbacks after reconnection
                // (they're still stored, just need to wire up to new connection)
                Ok(())
            }
            Ok(false) => Err(error),
            Err(e) => Err(e),
        }
    }
}

impl BaseSTT for DeepgramSTT {
    // ... existing implementations ...

    fn reconnection_config(&self) -> Option<&ReconnectionConfig> {
        self.reconnection_config.as_ref()
    }

    fn set_reconnection_config(&mut self, config: ReconnectionConfig) {
        self.reconnection_config = Some(config);
    }
}
```

**Step 4: Modify WebSocket Event Loop** (in each provider)

```rust
// In the main WebSocket event loop (e.g., deepgram.rs start_connection)
loop {
    tokio::select! {
        // ... existing branches ...

        msg = ws_stream.next() => {
            match msg {
                Some(Ok(message)) => {
                    // Process message...
                }
                Some(Err(e)) => {
                    // Connection error - attempt reconnection
                    let error = STTError::NetworkError(e.to_string());
                    if self.handle_connection_error(error).await.is_err() {
                        break; // Give up
                    }
                    // Reconnected - continue loop
                }
                None => {
                    // WebSocket closed - attempt reconnection
                    let error = STTError::ConnectionFailed("WebSocket closed".to_string());
                    if self.handle_connection_error(error).await.is_err() {
                        break;
                    }
                }
            }
        }
    }
}
```

### Rollout Strategy

| Phase | Providers | Timeline |
|-------|-----------|----------|
| 1 | Deepgram, ElevenLabs | Week 4 |
| 2 | Azure, Google, Cartesia | Week 5 |
| 3 | AssemblyAI, IBM Watson, AWS | Week 6 |
| 4 | Remaining 23 providers | Week 7-8 |

---

## Gap 8: Frame Priority Queue (P2 - Medium)

### Current State
- `OperationPriority` exists in `src/livekit/operations.rs`
- STT/TTS frames processed FIFO without priority

### Impact Analysis

| File | Lines | Impact | Risk |
|------|-------|--------|------|
| `src/core/pipeline/queue.rs` | New file | Create priority queue | Low |
| `src/core/tts/provider.rs` | ~500-637 | Use priority in dispatcher | Medium |

### Optimal Implementation

This is lower priority since FIFO works well for most cases. Implement only if latency-sensitive control messages need prioritization.

```rust
// src/core/pipeline/queue.rs
use std::cmp::Ordering;
use std::collections::BinaryHeap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FramePriority {
    /// System frames (interrupts, clears) - processed first
    System = 0,
    /// High priority frames (user speech)
    High = 1,
    /// Normal frames (TTS output)
    Normal = 2,
    /// Low priority frames (logging, metrics)
    Low = 3,
}

pub struct PriorityFrame<T> {
    pub frame: T,
    pub priority: FramePriority,
    pub sequence: u64,
}

impl<T> Ord for PriorityFrame<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        // Lower priority value = higher priority (processed first)
        // For equal priority, lower sequence = older = first
        match (self.priority as u8).cmp(&(other.priority as u8)) {
            Ordering::Equal => self.sequence.cmp(&other.sequence).reverse(),
            other => other.reverse(),
        }
    }
}

impl<T> PartialOrd for PriorityFrame<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<T> PartialEq for PriorityFrame<T> {
    fn eq(&self, other: &Self) -> bool {
        self.priority == other.priority && self.sequence == other.sequence
    }
}

impl<T> Eq for PriorityFrame<T> {}

pub struct FramePriorityQueue<T> {
    queue: BinaryHeap<PriorityFrame<T>>,
    sequence_counter: u64,
}

impl<T> FramePriorityQueue<T> {
    pub fn new() -> Self {
        Self {
            queue: BinaryHeap::new(),
            sequence_counter: 0,
        }
    }

    pub fn push(&mut self, frame: T, priority: FramePriority) {
        self.sequence_counter += 1;
        self.queue.push(PriorityFrame {
            frame,
            priority,
            sequence: self.sequence_counter,
        });
    }

    pub fn pop(&mut self) -> Option<T> {
        self.queue.pop().map(|pf| pf.frame)
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
}
```

---

## Implementation Timeline

```
Week 1: Foundation
├── Day 1: Signal handling (#3) - Immediate production benefit
├── Day 2-3: Observer trait + registry (#1) - Foundation for all metrics
├── Day 4-5: Observer integration with VoiceManager

Week 2: Metrics & Detection
├── Day 1-2: UserBotLatencyObserver (#2)
├── Day 3-5: Bot Speaking Detection (#4)

Week 3: Provider Metrics
├── Day 1-3: TTFB Metrics infrastructure (#6)
├── Day 4-5: TTFB integration with TTS providers

Week 4: STT Reconnection (Phase 1)
├── Day 1: Move ReconnectionConfig to shared location
├── Day 2-3: Deepgram STT reconnection (#7)
├── Day 4-5: ElevenLabs STT reconnection

Week 5: STT Reconnection (Phase 2)
├── Day 1-2: Azure STT reconnection
├── Day 3-4: Google STT reconnection
├── Day 5: Cartesia STT reconnection

Week 6: VAD & Testing
├── Day 1-4: VAD State Machine (#5)
├── Day 5: Integration testing for all gaps

Week 7-8: Polish
├── Priority Queue (#8) - if needed
├── Remaining STT reconnection (23 providers)
├── Documentation updates
├── Performance testing & optimization
```

---

## Testing Strategy

### Unit Tests (Per Gap)

| Gap | Test Focus | Coverage Target |
|-----|-----------|-----------------|
| #1 Observer | Registration, notification, concurrency | 90% |
| #2 Latency | Calculation accuracy, percentiles | 95% |
| #3 Signal | Signal reception, shutdown sequence | 80% |
| #4 Speaking | State transitions, timing accuracy | 90% |
| #5 VAD | State machine transitions | 95% |
| #6 TTFB | Timing accuracy, atomic operations | 90% |
| #7 Reconnection | Backoff calculation, retry logic | 95% |
| #8 Priority | Queue ordering, edge cases | 90% |

### Integration Tests

1. **End-to-end latency flow**: User speaks → Bot responds → Latency measured
2. **Graceful shutdown**: Signal → Drain → Clean exit
3. **Reconnection under network failure**: Simulate disconnect → Verify reconnect
4. **Observer notification ordering**: Multiple observers receive events in order

### Performance Tests

1. **Observer overhead**: Measure with 0, 1, 4, 10 observers
2. **Latency tracking overhead**: Compare with/without tracking
3. **Reconnection impact**: Measure audio gap during reconnection
4. **Memory usage**: Verify no leaks during extended operation

---

## Risk Mitigation Summary

| Risk | Probability | Impact | Mitigation |
|------|------------|--------|------------|
| Breaking existing callbacks | Low | High | Keep existing API, add observer as additional path |
| Performance regression | Low | Medium | Benchmark before/after, use atomics for hot paths |
| Reconnection state corruption | Medium | High | Add state validation, comprehensive tests |
| Signal handling platform issues | Low | Medium | Test on Linux, macOS, Windows; use tokio::signal |
| Observer memory growth | Low | Low | Use bounded registry, document limits |

---

## Files to Create/Modify Summary

### New Files

| File | Purpose |
|------|---------|
| `src/core/observability/mod.rs` | Observer trait and registry |
| `src/core/observability/latency.rs` | User-bot latency tracking |
| `src/core/audio/vad.rs` | VAD state machine |
| `src/core/metrics/provider_metrics.rs` | TTFB and provider metrics |
| `src/core/websocket/mod.rs` | Shared WebSocket utilities |
| `src/core/websocket/reconnection.rs` | Shared ReconnectionConfig |
| `src/core/pipeline/queue.rs` | Priority queue (optional) |

### Modified Files

| File | Changes |
|------|---------|
| `src/main.rs` | Signal handling, graceful shutdown |
| `src/state/mod.rs` | Shutdown coordination |
| `src/core/voice_manager/manager.rs` | Observer integration, speaking detection |
| `src/core/voice_manager/state.rs` | BotSpeakingState |
| `src/core/stt/base.rs` | Reconnection methods |
| `src/core/tts/provider.rs` | TTFB metrics |
| `src/handlers/ws/handler.rs` | Shutdown broadcast |
| 31 STT providers | Reconnection support (incremental) |

---

**Document Created**: 2026-01-19
**Status**: Ready for implementation
