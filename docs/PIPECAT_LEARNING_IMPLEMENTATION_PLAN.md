# Pipecat Learning & Implementation Plan for Bud Waav Gateway

**Date**: 2026-01-19
**Status**: Research Complete - Implementation Plan Ready
**Source Analysis**: pipecat-ai/pipecat repository (Battle-tested production framework)

---

## Executive Summary

This document captures comprehensive learnings from analyzing Pipecat AI's codebase and identifies critical gaps in Bud Waav's gateway implementation. Pipecat represents a battle-tested, production-ready framework for voice AI pipelines with over 70+ provider integrations. Key patterns worth adopting include:

1. **WebSocket Service Base Class** - Unified reconnection with exponential backoff
2. **Frame-based Pipeline Architecture** - Priority queues and controlled flow
3. **Event Handler Pattern** - Consistent lifecycle callbacks
4. **Metrics Collection** - TTFB and usage tracking at provider level
5. **Interruption Handling** - Pipeline-wide coordination for interruptions
6. **Audio Context Management** - Multi-turn conversation support

---

## Table of Contents

1. [Architecture Comparison](#1-architecture-comparison)
2. [Critical Patterns from Pipecat](#2-critical-patterns-from-pipecat)
3. [Identified Gaps in Bud Waav](#3-identified-gaps-in-bud-waav)
4. [Implementation Recommendations](#4-implementation-recommendations)
5. [Provider-Specific Learnings](#5-provider-specific-learnings)
6. [Error Handling & Recovery](#6-error-handling--recovery)
7. [Performance Optimizations](#7-performance-optimizations)
8. [Implementation Phases](#8-implementation-phases)
9. [Testing Strategy](#9-testing-strategy)
10. [Risk Assessment](#10-risk-assessment)

---

## 1. Architecture Comparison

### 1.1 Pipecat Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                    Pipecat Pipeline Architecture                │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│   Audio Input  →  [STTService]  →  [LLMService]  →  [TTSService]  →  Audio Output
│                        │                │               │
│                        ▼                ▼               ▼
│                   ┌─────────────────────────────────────────┐
│                   │           Frame Processor Queue          │
│                   │  ┌─────────────────────────────────────┐│
│                   │  │ Priority: SystemFrame > DataFrame   ││
│                   │  │ Two Tasks: Input Task + Process Task││
│                   │  └─────────────────────────────────────┘│
│                   └─────────────────────────────────────────┘
│                                                                 │
│   Key Components:                                               │
│   - WebsocketService: Base class with reconnection              │
│   - FrameProcessor: Priority queue, interruption handling       │
│   - STTService/TTSService: Event handlers, metrics              │
│   - Frames: DataFrame, SystemFrame, ControlFrame               │
└─────────────────────────────────────────────────────────────────┘
```

### 1.2 Bud Waav Current Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                  Bud Waav Gateway Architecture                  │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│   WebSocket Handler  →  VoiceManager  →  STT/TTS Providers     │
│         │                    │                  │               │
│         ▼                    ▼                  ▼               │
│   ┌──────────────┐   ┌─────────────┐   ┌─────────────────────┐ │
│   │ConnectionState│   │  Callbacks  │   │  Provider-specific  │ │
│   │    Manager   │   │   Pattern   │   │   WebSocket Mgmt    │ │
│   └──────────────┘   └─────────────┘   └─────────────────────┘ │
│                                                                 │
│   Strengths:                                                    │
│   - Rich metadata (STTResult with word timing, entities, etc.)  │
│   - Plugin registry for dynamic providers                       │
│   - Emotion configuration for TTS                               │
│   - Comprehensive error types (STTError, TTSError)              │
│                                                                 │
│   Missing (vs Pipecat):                                         │
│   - Unified WebSocket service base class                        │
│   - Frame-based pipeline with priority queues                   │
│   - Consistent event handler pattern across all providers       │
│   - TTFB/metrics at provider level                              │
│   - Pipeline-wide interruption handling                         │
│   - Audio context management for multi-turn                     │
└─────────────────────────────────────────────────────────────────┘
```

### 1.3 Key Architectural Differences

| Aspect | Pipecat | Bud Waav | Gap Severity |
|--------|---------|----------|--------------|
| WebSocket Base Class | ✅ `WebsocketService` with reconnection | ❌ Provider-specific handling | **HIGH** |
| Frame Pipeline | ✅ Priority queue, 2-task model | ❌ Direct callbacks | **MEDIUM** |
| Event Handlers | ✅ on_connected/on_disconnected/on_error | ⚠️ Partial (STT only) | **HIGH** |
| Metrics (TTFB) | ✅ Built-in TTFB, processing time | ❌ Not implemented | **MEDIUM** |
| Interruption | ✅ UninterruptibleFrame, pipeline-wide | ❌ Not implemented | **MEDIUM** |
| Reconnection | ✅ Exponential backoff, jitter | ⚠️ Only in Realtime providers | **HIGH** |
| Audio Context | ✅ Context IDs for multi-turn TTS | ❌ Not implemented | **LOW** |

---

## 2. Critical Patterns from Pipecat

### 2.1 WebSocket Service Base Class

**File**: `/tmp/pipecat/src/pipecat/services/websocket_service.py`

```python
# Key Pattern: Unified WebSocket management with automatic reconnection
class WebsocketService(AIService):
    def __init__(self, *, auto_reconnect: bool = True, max_reconnection_attempts: int = 3):
        self._auto_reconnect = auto_reconnect
        self._max_reconnection_attempts = max_reconnection_attempts
        self._reconnection_attempts = 0
        self._disconnecting = False  # CRITICAL: Prevents reconnect during intentional disconnect

    async def _maybe_try_reconnect(self) -> bool:
        """Attempt reconnection with exponential backoff."""
        if not self._auto_reconnect or self._disconnecting:
            return False
        if self._reconnection_attempts >= self._max_reconnection_attempts:
            return False

        # CRITICAL: Exponential backoff with jitter
        base_delay = min(2 ** self._reconnection_attempts, 30)
        jitter = random.uniform(0, base_delay * 0.1)
        await asyncio.sleep(base_delay + jitter)

        self._reconnection_attempts += 1
        success = await self._try_reconnect()
        if success:
            self._reconnection_attempts = 0
        return success

    async def send_with_retry(self, message: str) -> None:
        """Send with automatic reconnection on failure."""
        if not await self._verify_connection():
            if not await self._maybe_try_reconnect():
                raise WebSocketError("Connection lost and reconnection failed")
        await self._websocket.send(message)
```

**Why This Matters**:
- Centralized reconnection logic avoids code duplication
- `_disconnecting` flag prevents reconnection race conditions
- Exponential backoff with jitter prevents thundering herd
- `send_with_retry` provides consistent error handling

### 2.2 Frame-Based Pipeline Architecture

**File**: `/tmp/pipecat/src/pipecat/processors/frame_processor.py`

```python
# Key Pattern: Priority queue with two-task model
class FrameProcessorQueue:
    """Priority queue: SystemFrame > other frames."""

    def __init__(self):
        self._system_queue = asyncio.Queue()   # High priority
        self._data_queue = asyncio.Queue()     # Normal priority

    async def put(self, frame: Frame):
        if isinstance(frame, SystemFrame):
            await self._system_queue.put(frame)
        else:
            await self._data_queue.put(frame)

    async def get(self) -> Frame:
        # System frames have absolute priority
        if not self._system_queue.empty():
            return await self._system_queue.get()
        return await self._data_queue.get()

class FrameProcessor:
    """Two-task architecture for controlled frame processing."""

    async def _input_task(self):
        """Handle system frames immediately (interruptions, control)."""
        while True:
            frame = await self._input_queue.get()
            if isinstance(frame, SystemFrame):
                await self._process_system_frame(frame)
            else:
                await self._process_queue.put(frame)

    async def _process_task(self):
        """Process data frames in order, respecting interruptions."""
        while True:
            await self._wait_for_interruption.wait()  # Can be paused
            frame = await self._process_queue.get()
            await self._process_data_frame(frame)
```

**Why This Matters**:
- System frames (interruptions) can preempt data processing
- Controlled frame ordering prevents race conditions
- Clear separation of concerns (input handling vs processing)

### 2.3 Event Handler Pattern

**File**: `/tmp/pipecat/src/pipecat/services/stt_service.py`

```python
# Key Pattern: Consistent lifecycle callbacks
class STTService(AIService):
    def __init__(self):
        self._event_handlers = {
            "on_connected": [],
            "on_disconnected": [],
            "on_connection_error": [],
        }

    def _register_event_handler(self, event: str, handler: Callable):
        """Register callback for lifecycle event."""
        self._event_handlers[event].append(handler)

    async def _emit_event(self, event: str, **kwargs):
        """Emit event to all registered handlers."""
        for handler in self._event_handlers[event]:
            await handler(**kwargs)

    async def _on_connected(self):
        """Called when WebSocket connects successfully."""
        await self._emit_event("on_connected", service=self)

    async def _on_connection_error(self, error: Exception):
        """Called when connection error occurs."""
        # CRITICAL: Push error upstream in pipeline
        error_frame = ErrorFrame(
            error=str(error),
            exception=error,
            processor=self.name
        )
        await self.push_error(error_frame)
        await self._emit_event("on_connection_error", service=self, error=error)
```

**Why This Matters**:
- Consistent lifecycle management across all services
- External code can react to connection state changes
- Error frames propagate upstream for pipeline-wide handling

### 2.4 STT Service Patterns: Continuous vs Segmented

**File**: `/tmp/pipecat/src/pipecat/services/stt_service.py`

```python
# Pattern 1: Continuous STT (always streaming)
class STTService(AIService):
    """Continuous streaming - all audio sent to provider."""

    async def process_frame(self, frame: Frame):
        if isinstance(frame, AudioRawFrame):
            await self._send_audio(frame.audio)

# Pattern 2: Segmented STT (VAD-based)
class SegmentedSTTService(STTService):
    """
    Buffer audio during speech, send on speech end.
    CRITICAL: Maintains 1-second lookback buffer for partial speech capture.
    """

    def __init__(self):
        self._audio_buffer = deque(maxlen=int(SAMPLE_RATE * 1))  # 1 second
        self._speech_buffer = []

    async def process_frame(self, frame: Frame):
        if isinstance(frame, AudioRawFrame):
            self._audio_buffer.append(frame.audio)

            if self._speech_active:
                self._speech_buffer.append(frame.audio)

        elif isinstance(frame, UserStartedSpeakingFrame):
            # Include lookback buffer for partial speech
            self._speech_buffer = list(self._audio_buffer)
            self._speech_active = True

        elif isinstance(frame, UserStoppedSpeakingFrame):
            # Send complete utterance to provider
            await self._process_utterance(b"".join(self._speech_buffer))
            self._speech_buffer = []
            self._speech_active = False
```

**Why This Matters**:
- Lookback buffer captures speech that starts mid-frame
- VAD integration prevents unnecessary processing
- Clear separation between continuous and segmented modes

### 2.5 TTS Word Timestamp Calculation

**File**: `/tmp/pipecat/src/pipecat/services/elevenlabs/tts.py`

```python
# Key Pattern: Calculate word timestamps from character alignment
class ElevenLabsTTSService(TTSService):
    async def _calculate_word_times(
        self,
        text: str,
        alignment: List[Dict],  # Characters with times
        audio_duration: float
    ) -> List[WordTimestamp]:
        """Convert character alignment to word timestamps."""
        words = []
        current_word = ""
        word_start = None

        for i, char_data in enumerate(alignment):
            char = char_data["character"]
            time = char_data["time"]

            if char == " " or i == len(alignment) - 1:
                # Word boundary
                if current_word:
                    word_end = time if char == " " else audio_duration
                    words.append(WordTimestamp(
                        word=current_word,
                        start=word_start,
                        end=word_end
                    ))
                current_word = ""
                word_start = None
            else:
                if word_start is None:
                    word_start = time
                current_word += char

        return words
```

**Why This Matters**:
- Enables word-level highlighting during playback
- Character-to-word mapping handles varied provider formats
- Accurate timing supports karaoke-style UIs

### 2.6 Audio Context Management for Multi-Turn TTS

**File**: `/tmp/pipecat/src/pipecat/services/elevenlabs/tts.py`

```python
# Key Pattern: Audio context for consistent voice across turns
class AudioContextWordTTSService(TTSService):
    """
    Manages audio contexts for multi-turn conversations.
    Context preserves voice characteristics across utterances.
    """

    def __init__(self):
        self._context_id: Optional[str] = None
        self._context_audio: bytes = b""  # Previous audio for context

    async def create_audio_context(self) -> str:
        """Create new context for conversation turn."""
        self._context_id = str(uuid.uuid4())
        self._context_audio = b""
        return self._context_id

    async def synthesize_with_context(
        self,
        text: str,
        context_id: str
    ) -> AsyncIterator[bytes]:
        """
        Synthesize with previous audio as context.
        Enables more natural prosody across utterances.
        """
        # Send previous audio as context (voice characteristics)
        payload = {
            "text": text,
            "context_id": context_id,
            "previous_audio": base64.b64encode(self._context_audio).decode()
        }

        async for chunk in self._stream_synthesis(payload):
            self._context_audio = chunk  # Update context for next turn
            yield chunk
```

**Why This Matters**:
- Voice consistency across conversation turns
- More natural prosody transitions
- Supports long-form conversational TTS

### 2.7 Metrics Collection

**File**: `/tmp/pipecat/src/pipecat/services/ai_service.py`

```python
# Key Pattern: Built-in metrics tracking
class AIService(FrameProcessor):
    def __init__(self):
        self._ttfb_time: Optional[float] = None
        self._processing_start: Optional[float] = None

    async def start_ttfb(self):
        """Mark start of processing for TTFB calculation."""
        self._processing_start = time.time()

    async def stop_ttfb(self):
        """Calculate and emit TTFB metric."""
        if self._processing_start:
            self._ttfb_time = time.time() - self._processing_start
            await self.push_frame(MetricsFrame(
                ttfb=self._ttfb_time,
                processor=self.name
            ))
            self._processing_start = None

    def report_usage(
        self,
        input_tokens: int = 0,
        output_tokens: int = 0,
        audio_seconds: float = 0.0
    ):
        """Report usage metrics for billing/monitoring."""
        metrics = {
            "processor": self.name,
            "input_tokens": input_tokens,
            "output_tokens": output_tokens,
            "audio_seconds": audio_seconds,
            "ttfb": self._ttfb_time
        }
        # Emit to pipeline for aggregation
        asyncio.create_task(self.push_frame(UsageMetricsFrame(**metrics)))
```

**Why This Matters**:
- TTFB critical for real-time voice applications
- Usage tracking enables cost monitoring
- Metrics propagate through pipeline for aggregation

---

## 3. Identified Gaps in Bud Waav

### 3.1 HIGH Priority Gaps

#### Gap 1: No Unified WebSocket Service Base Class

**Current State in Bud Waav**:
- Each STT provider (Deepgram, ElevenLabs, Azure, etc.) implements its own WebSocket handling
- Reconnection logic is inconsistent or missing
- No shared exponential backoff implementation

**Files Affected**:
- `src/core/stt/deepgram.rs` - Custom WebSocket handling
- `src/core/stt/elevenlabs/client.rs` - Different WebSocket pattern
- `src/core/stt/cartesia/client.rs` - Yet another pattern
- `src/core/tts/cartesia/provider.rs` - TTS WebSocket handling

**Risk**: Connection failures during production use will have inconsistent behavior. Some providers may reconnect, others may fail silently.

#### Gap 2: Inconsistent Event Handler Pattern

**Current State in Bud Waav**:
- `BaseSTT` trait has `on_result` and `on_error` callbacks
- `BaseTTS` trait has `AudioCallback` with `on_audio`, `on_error`, `on_complete`
- No unified `on_connected`/`on_disconnected` pattern
- Event handlers not propagated to VoiceManager level

**Risk**: External systems cannot reliably react to connection lifecycle changes.

#### Gap 3: Reconnection Only in Realtime Providers

**Current State in Bud Waav**:
- `ReconnectionConfig` exists in `src/core/realtime/base.rs`
- Only OpenAI Realtime and Hume Realtime use it
- STT providers (Deepgram, ElevenLabs) have no reconnection
- TTS providers have ad-hoc reconnection attempts

**Code Reference** (Realtime has good pattern):
```rust
// src/core/realtime/base.rs:82-109
pub struct ReconnectionConfig {
    pub enabled: bool,
    pub max_attempts: u32,
    pub initial_delay_ms: u64,
    pub max_delay_ms: u64,
    pub backoff_multiplier: f32,
    pub jitter: bool,
}
```

**Risk**: STT/TTS WebSocket connections will drop under network instability without recovery.

### 3.2 MEDIUM Priority Gaps

#### Gap 4: No Frame-Based Pipeline Architecture

**Current State in Bud Waav**:
- Direct callback model from STT → VoiceManager → TTS
- No priority queue for system messages (interruptions)
- No controlled frame ordering

**Risk**: Cannot implement clean interruption handling or prioritize control messages.

#### Gap 5: No TTFB Metrics at Provider Level

**Current State in Bud Waav**:
- No time-to-first-byte tracking
- No processing time metrics
- No usage tracking at provider level

**Files Missing Metrics**:
- All STT providers
- All TTS providers
- VoiceManager

**Risk**: Cannot optimize latency or monitor provider performance in production.

#### Gap 6: No Pipeline Interruption Handling

**Current State in Bud Waav**:
- No concept of `UninterruptibleFrame`
- No coordinated pipeline interruption
- TTS cannot be interrupted mid-stream cleanly

**Risk**: Poor user experience when user interrupts assistant mid-response.

### 3.3 LOW Priority Gaps

#### Gap 7: No Audio Context Management for Multi-Turn TTS

**Current State in Bud Waav**:
- Each TTS call is independent
- No context preservation across turns
- Voice characteristics may vary between utterances

**Risk**: Less natural voice consistency in multi-turn conversations.

---

## 4. Implementation Recommendations

### 4.1 Create WebSocket Service Base Trait (HIGH PRIORITY)

**New File**: `src/core/websocket/service.rs`

```rust
//! Unified WebSocket service base trait for all WebSocket-based providers.
//!
//! Provides:
//! - Automatic reconnection with exponential backoff
//! - Consistent event emission (connected, disconnected, error)
//! - Send-with-retry pattern
//! - Connection verification before operations

use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

/// Reconnection configuration (move from realtime/base.rs to make shared)
pub use crate::core::realtime::base::ReconnectionConfig;

/// Event types for WebSocket lifecycle
#[derive(Debug, Clone)]
pub enum WebSocketEvent {
    Connected,
    Disconnected { reason: Option<String> },
    ConnectionError { error: String, will_retry: bool },
    ReconnectionAttempt { attempt: u32, max_attempts: u32 },
    ReconnectionSuccess { total_attempts: u32 },
    ReconnectionFailed { total_attempts: u32, last_error: String },
}

/// Callback type for WebSocket events
pub type WebSocketEventCallback = Arc<dyn Fn(WebSocketEvent) + Send + Sync>;

/// Base trait for WebSocket-based services
#[async_trait]
pub trait WebSocketService: Send + Sync {
    /// Get the WebSocket URL to connect to
    fn get_websocket_url(&self) -> Result<String, WebSocketError>;

    /// Get HTTP headers for the connection (auth, etc.)
    fn get_connection_headers(&self) -> Vec<(String, String)>;

    /// Get reconnection configuration
    fn get_reconnection_config(&self) -> &ReconnectionConfig;

    /// Check if intentionally disconnecting (suppresses reconnection)
    fn is_disconnecting(&self) -> bool;

    /// Set disconnecting flag
    fn set_disconnecting(&mut self, disconnecting: bool);

    /// Handle successful connection
    async fn on_connected(&mut self);

    /// Handle connection loss
    async fn on_disconnected(&mut self, reason: Option<String>);

    /// Handle incoming WebSocket message
    async fn on_message(&mut self, message: Message) -> Result<(), WebSocketError>;

    /// Register event callback
    fn register_event_callback(&mut self, callback: WebSocketEventCallback);

    // === Provided implementations ===

    /// Attempt reconnection with exponential backoff
    async fn maybe_try_reconnect(&mut self) -> bool {
        let config = self.get_reconnection_config();

        if !config.enabled || self.is_disconnecting() {
            return false;
        }

        let mut attempt = 0u32;
        while config.should_retry(attempt) {
            attempt += 1;

            // Emit reconnection attempt event
            self.emit_event(WebSocketEvent::ReconnectionAttempt {
                attempt,
                max_attempts: config.max_attempts,
            });

            // Calculate delay with exponential backoff + jitter
            let delay_ms = config.calculate_delay(attempt);
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;

            // Check if disconnecting flag set during sleep
            if self.is_disconnecting() {
                return false;
            }

            // Attempt reconnection
            match self.try_reconnect().await {
                Ok(_) => {
                    self.emit_event(WebSocketEvent::ReconnectionSuccess {
                        total_attempts: attempt,
                    });
                    return true;
                }
                Err(e) => {
                    if !config.should_retry(attempt) {
                        self.emit_event(WebSocketEvent::ReconnectionFailed {
                            total_attempts: attempt,
                            last_error: e.to_string(),
                        });
                        return false;
                    }
                    // Continue to next attempt
                }
            }
        }
        false
    }

    /// Try to reconnect (to be implemented by provider)
    async fn try_reconnect(&mut self) -> Result<(), WebSocketError>;

    /// Send message with automatic reconnection on failure
    async fn send_with_retry(&mut self, message: Message) -> Result<(), WebSocketError> {
        // First verify connection
        if !self.verify_connection().await {
            if !self.maybe_try_reconnect().await {
                return Err(WebSocketError::ConnectionLost(
                    "Connection lost and reconnection failed".to_string()
                ));
            }
        }

        // Attempt send
        match self.send_raw(message.clone()).await {
            Ok(_) => Ok(()),
            Err(e) => {
                // Try reconnection on send failure
                if self.maybe_try_reconnect().await {
                    self.send_raw(message).await
                } else {
                    Err(e)
                }
            }
        }
    }

    /// Verify WebSocket connection is alive
    async fn verify_connection(&self) -> bool;

    /// Send raw message without retry
    async fn send_raw(&mut self, message: Message) -> Result<(), WebSocketError>;

    /// Emit event to registered callbacks
    fn emit_event(&self, event: WebSocketEvent);
}
```

### 4.2 Update All STT Providers to Use WebSocket Service

**Pattern for Deepgram** (apply to all WebSocket STT providers):

```rust
// src/core/stt/deepgram.rs - Updated pattern

impl WebSocketService for DeepgramSTT {
    fn get_websocket_url(&self) -> Result<String, WebSocketError> {
        self.build_websocket_url(self.config.as_ref().unwrap())
            .map_err(|e| WebSocketError::UrlError(e.to_string()))
    }

    fn get_connection_headers(&self) -> Vec<(String, String)> {
        vec![
            ("Authorization".to_string(), format!("Token {}", self.config.as_ref().unwrap().base.api_key)),
        ]
    }

    fn get_reconnection_config(&self) -> &ReconnectionConfig {
        &self.reconnection_config
    }

    // ... implement other required methods
}

// Add reconnection config to DeepgramSTT struct
pub struct DeepgramSTT {
    config: Option<DeepgramSTTConfig>,
    state: ConnectionState,
    reconnection_config: ReconnectionConfig,  // NEW
    disconnecting: bool,  // NEW
    event_callbacks: Vec<WebSocketEventCallback>,  // NEW
    // ... existing fields
}
```

### 4.3 Add TTFB Metrics to Providers (MEDIUM PRIORITY)

**New File**: `src/core/metrics.rs`

```rust
//! Provider-level metrics collection for latency and usage tracking.

use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// Metrics for a single operation
#[derive(Debug, Clone)]
pub struct OperationMetrics {
    /// Provider name (e.g., "deepgram", "elevenlabs")
    pub provider: String,
    /// Operation type (e.g., "stt", "tts")
    pub operation: String,
    /// Time to first byte (None if not applicable)
    pub ttfb: Option<Duration>,
    /// Total processing time
    pub processing_time: Duration,
    /// Input size (bytes for audio, tokens for text)
    pub input_size: u64,
    /// Output size
    pub output_size: u64,
    /// Whether operation succeeded
    pub success: bool,
    /// Error message if failed
    pub error: Option<String>,
}

/// Metrics collector for providers
pub struct MetricsCollector {
    sender: mpsc::Sender<OperationMetrics>,
    provider: String,
    operation_start: Option<Instant>,
    first_response_received: bool,
    ttfb: Option<Duration>,
}

impl MetricsCollector {
    pub fn new(provider: &str, sender: mpsc::Sender<OperationMetrics>) -> Self {
        Self {
            sender,
            provider: provider.to_string(),
            operation_start: None,
            first_response_received: false,
            ttfb: None,
        }
    }

    /// Mark start of operation (call before sending request)
    pub fn start_operation(&mut self) {
        self.operation_start = Some(Instant::now());
        self.first_response_received = false;
        self.ttfb = None;
    }

    /// Mark first response received (calculates TTFB)
    pub fn mark_first_response(&mut self) {
        if !self.first_response_received {
            if let Some(start) = self.operation_start {
                self.ttfb = Some(start.elapsed());
                self.first_response_received = true;
            }
        }
    }

    /// Complete operation and emit metrics
    pub async fn complete_operation(
        &mut self,
        input_size: u64,
        output_size: u64,
        success: bool,
        error: Option<String>,
    ) {
        let processing_time = self.operation_start
            .map(|s| s.elapsed())
            .unwrap_or_default();

        let metrics = OperationMetrics {
            provider: self.provider.clone(),
            operation: "stt".to_string(), // Or TTS, etc.
            ttfb: self.ttfb,
            processing_time,
            input_size,
            output_size,
            success,
            error,
        };

        let _ = self.sender.send(metrics).await;

        // Reset state
        self.operation_start = None;
        self.first_response_received = false;
        self.ttfb = None;
    }
}
```

### 4.4 Add Event Handler Pattern to All Providers

**Update to BaseSTT Trait** (`src/core/stt/base.rs`):

```rust
/// Event types for STT lifecycle
#[derive(Debug, Clone)]
pub enum STTEvent {
    Connected,
    Disconnected { reason: Option<String> },
    ConnectionError { error: STTError, will_retry: bool },
    StreamingStarted,
    StreamingStopped,
}

/// Callback type for STT events
pub type STTEventCallback =
    Arc<dyn Fn(STTEvent) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

#[async_trait::async_trait]
pub trait BaseSTT: Send + Sync {
    // ... existing methods ...

    /// Register a callback for lifecycle events (NEW)
    async fn on_event(&mut self, callback: STTEventCallback) -> Result<(), STTError>;

    /// Emit event to registered callbacks (for implementors)
    async fn emit_event(&self, event: STTEvent);
}
```

### 4.5 Implement Interruption Handling (MEDIUM PRIORITY)

**New File**: `src/core/pipeline/interruption.rs`

```rust
//! Pipeline interruption handling for clean TTS interruption.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::Notify;

/// Interruption coordinator for pipeline-wide interruption handling
pub struct InterruptionCoordinator {
    /// Flag indicating an interruption is in progress
    interrupted: AtomicBool,
    /// Notify waiters when interruption ends
    interruption_ended: Arc<Notify>,
}

impl InterruptionCoordinator {
    pub fn new() -> Self {
        Self {
            interrupted: AtomicBool::new(false),
            interruption_ended: Arc::new(Notify::new()),
        }
    }

    /// Trigger an interruption (e.g., user started speaking)
    pub fn interrupt(&self) {
        self.interrupted.store(true, Ordering::SeqCst);
    }

    /// Clear interruption state
    pub fn clear_interruption(&self) {
        self.interrupted.store(false, Ordering::SeqCst);
        self.interruption_ended.notify_waiters();
    }

    /// Check if currently interrupted
    pub fn is_interrupted(&self) -> bool {
        self.interrupted.load(Ordering::SeqCst)
    }

    /// Wait for interruption to end
    pub async fn wait_for_interruption_end(&self) {
        if self.is_interrupted() {
            self.interruption_ended.notified().await;
        }
    }
}

/// Trait for processors that can be interrupted
pub trait Interruptible {
    /// Get the interruption coordinator
    fn get_interruption_coordinator(&self) -> &InterruptionCoordinator;

    /// Handle interruption (stop current operation cleanly)
    async fn handle_interruption(&mut self);

    /// Check if current frame should be uninterruptible
    fn is_uninterruptible(&self) -> bool {
        false
    }
}
```

---

## 5. Provider-Specific Learnings

### 5.1 ElevenLabs TTS

**Key Learnings from Pipecat**:

1. **WebSocket + HTTP Dual Mode**:
   - WebSocket for streaming with context
   - HTTP for simple one-shot synthesis
   - Choose based on use case

2. **Keepalive Handler**:
   - ElevenLabs WebSocket requires periodic keepalive
   - Pipecat implements background keepalive task

3. **Word Timestamp Calculation**:
   - Character alignment → word boundaries
   - Handle punctuation correctly

**Recommended Changes for Bud Waav**:
- Add WebSocket keepalive task to `src/core/tts/elevenlabs.rs`
- Implement word timestamp calculation

### 5.2 Cartesia TTS

**Key Learnings from Pipecat**:

1. **CJK Language Handling**:
   - Chinese/Japanese/Korean have no word boundaries
   - Each character treated as a word for timestamps

2. **SSML Support**:
   - `<spell>`, `<emotion>`, `<pause>`, `<volume>`, `<speed>` tags
   - Parse and convert to Cartesia format

3. **Context ID Management**:
   - Continue from previous synthesis for voice consistency

**Recommended Changes for Bud Waav**:
- Add CJK word detection to `src/core/tts/cartesia/provider.rs`
- Implement SSML parsing if not present
- Add context continuation support

### 5.3 Deepgram STT

**Key Learnings from Pipecat**:

1. **Smart Formatting**:
   - Auto-detect numbers, dates, currencies
   - Configurable via API parameters

2. **VAD Events**:
   - Use `vad_events=true` for speech boundary detection
   - Enables efficient utterance segmentation

3. **Endpointing Configuration**:
   - Fine-tune silence detection threshold
   - Balance responsiveness vs accuracy

**Current Bud Waav Status**: Already well-implemented in `src/core/stt/deepgram.rs`

**Recommended Improvements**:
- Add reconnection using new WebSocket service base
- Add TTFB metrics

---

## 6. Error Handling & Recovery

### 6.1 Error Propagation Pattern

**From Pipecat**:
```python
# Error frames propagate upstream in pipeline
error_frame = ErrorFrame(
    error=str(exception),
    exception=exception,
    processor=self.name,
    fatal=False  # Indicates if pipeline should terminate
)
await self.push_error(error_frame)
```

**Recommended for Bud Waav**:

```rust
// src/core/errors/pipeline_error.rs

/// Error frame for pipeline propagation
#[derive(Debug, Clone)]
pub struct PipelineError {
    /// Human-readable error message
    pub message: String,
    /// Error type classification
    pub error_type: PipelineErrorType,
    /// Processor that generated the error
    pub source: String,
    /// Whether this error is recoverable
    pub recoverable: bool,
    /// Suggested action
    pub action: ErrorAction,
    /// Original error (for debugging)
    pub original_error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum PipelineErrorType {
    Connection,
    Authentication,
    RateLimit,
    Timeout,
    ProviderError,
    InvalidInput,
    Internal,
}

#[derive(Debug, Clone)]
pub enum ErrorAction {
    Retry,
    Reconnect,
    SwitchProvider,
    NotifyUser,
    Terminate,
}
```

### 6.2 Rate Limit Handling

**From Pipecat**:
- Parse `Retry-After` header
- Implement provider-specific rate limit handling
- Queue requests during rate limit window

**Recommended for Bud Waav**:
- Already have `TTSError::RateLimited` - ensure all providers use it
- Add request queue with rate limit awareness

---

## 7. Performance Optimizations

### 7.1 Channel Buffer Sizes

**From Pipecat Analysis**:
- Input queue: Smaller (16-32) for responsive interruption
- Process queue: Larger (64-128) for throughput
- Output queue: Medium (32-64) for backpressure

**Bud Waav Current**:
- `CHANNEL_BUFFER_SIZE: 1024` in WebSocket handler (may be too large)
- STT result channel: 256 (good)

**Recommendation**:
- Reduce WebSocket handler buffer to 256
- Add configurable buffer sizes

### 7.2 Zero-Copy Audio Handling

**Pipecat Pattern**:
```python
# Use memoryview/buffer protocol for zero-copy
audio_data = memoryview(audio_bytes)
chunk = audio_data[offset:offset+chunk_size]  # No copy
```

**Bud Waav Current**:
- Uses `bytes::Bytes` which is already zero-copy via Arc
- Good pattern already in place

### 7.3 JSON Parsing Optimization

**Pipecat Pattern**:
- Parse once, branch on type
- Avoid re-parsing on error

**Bud Waav Current**:
- `handle_websocket_message` in Deepgram already does this
- Good pattern already in place

---

## 8. Implementation Phases

### Phase 1: WebSocket Service Foundation (Week 1)

**Tasks**:
1. Create `src/core/websocket/service.rs` with base trait
2. Create `src/core/websocket/mod.rs` module
3. Move `ReconnectionConfig` to shared location
4. Add `WebSocketEvent` enum
5. Write unit tests for reconnection logic

**Files to Create/Modify**:
- `src/core/websocket/mod.rs` (NEW)
- `src/core/websocket/service.rs` (NEW)
- `src/core/realtime/base.rs` (move ReconnectionConfig)
- `src/core/mod.rs` (add websocket module)

### Phase 2: STT Provider Updates (Week 2)

**Tasks**:
1. Update `DeepgramSTT` to implement `WebSocketService`
2. Update `ElevenLabsSTT` to implement `WebSocketService`
3. Update `AzureSTT` to implement `WebSocketService`
4. Update `CartesiaSTT` to implement `WebSocketService`
5. Add event handler pattern to all STT providers
6. Integration tests for reconnection

**Files to Modify**:
- `src/core/stt/deepgram.rs`
- `src/core/stt/elevenlabs/client.rs`
- `src/core/stt/azure/client.rs`
- `src/core/stt/cartesia/client.rs`
- `src/core/stt/base.rs` (add events)

### Phase 3: TTS Provider Updates (Week 3)

**Tasks**:
1. Update `CartesiaTTS` WebSocket handling
2. Add reconnection to all WebSocket TTS providers
3. Add event handler pattern to all TTS providers
4. Add word timestamp calculation improvements

**Files to Modify**:
- `src/core/tts/cartesia/provider.rs`
- `src/core/tts/elevenlabs.rs`
- `src/core/tts/base.rs` (add events)

### Phase 4: Metrics & Interruption (Week 4)

**Tasks**:
1. Create `src/core/metrics.rs` module
2. Add TTFB tracking to STT providers
3. Add TTFB tracking to TTS providers
4. Create `src/core/pipeline/interruption.rs`
5. Integrate interruption with VoiceManager
6. End-to-end latency tests

**Files to Create/Modify**:
- `src/core/metrics.rs` (NEW)
- `src/core/pipeline/mod.rs` (NEW)
- `src/core/pipeline/interruption.rs` (NEW)
- `src/core/voice_manager/manager.rs`

### Phase 5: Testing & Documentation (Week 5)

**Tasks**:
1. Write comprehensive unit tests
2. Write integration tests for reconnection scenarios
3. Performance benchmarks (TTFB, throughput)
4. Update architecture documentation
5. Add examples and usage documentation

---

## 9. Testing Strategy

### 9.1 Unit Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_reconnection_exponential_backoff() {
        let config = ReconnectionConfig::default();

        // First attempt: ~1000ms
        let delay1 = config.calculate_delay(1);
        assert!(delay1 >= 750 && delay1 <= 1250); // With jitter

        // Second attempt: ~2000ms
        let delay2 = config.calculate_delay(2);
        assert!(delay2 >= 1500 && delay2 <= 2500);

        // Third attempt: ~4000ms
        let delay3 = config.calculate_delay(3);
        assert!(delay3 >= 3000 && delay3 <= 5000);
    }

    #[tokio::test]
    async fn test_reconnection_max_attempts() {
        let config = ReconnectionConfig {
            max_attempts: 3,
            ..Default::default()
        };

        assert!(config.should_retry(0));
        assert!(config.should_retry(1));
        assert!(config.should_retry(2));
        assert!(!config.should_retry(3));
    }

    #[tokio::test]
    async fn test_disconnecting_flag_prevents_reconnect() {
        // Test that setting disconnecting flag prevents reconnection attempts
    }
}
```

### 9.2 Integration Tests

```rust
#[tokio::test]
async fn test_deepgram_reconnection_on_network_failure() {
    // 1. Connect to Deepgram
    // 2. Start streaming audio
    // 3. Simulate network failure
    // 4. Verify reconnection attempts
    // 5. Verify audio continues after reconnection
}

#[tokio::test]
async fn test_pipeline_interruption() {
    // 1. Start TTS playback
    // 2. Trigger interruption (user speaking)
    // 3. Verify TTS stops within latency budget
    // 4. Verify STT receives audio
    // 5. Clear interruption
    // 6. Verify TTS can resume
}
```

### 9.3 Performance Tests

```rust
#[tokio::test]
async fn test_stt_ttfb_budget() {
    // Target: < 200ms TTFB for STT
    let mut metrics_rx = setup_metrics_collector();
    let mut stt = create_deepgram_stt();

    stt.connect().await.unwrap();
    stt.send_audio(test_audio()).await.unwrap();

    let metrics = metrics_rx.recv().await.unwrap();
    assert!(metrics.ttfb.unwrap() < Duration::from_millis(200));
}
```

---

## 10. Risk Assessment

### 10.1 High Risk Items

| Risk | Mitigation |
|------|------------|
| Breaking existing provider implementations | Feature flag for new WebSocket service, gradual migration |
| Performance regression from added layers | Benchmark before/after, optimize hot paths |
| Reconnection storms under network issues | Jitter in backoff, circuit breaker pattern |

### 10.2 Medium Risk Items

| Risk | Mitigation |
|------|------------|
| Metrics overhead affecting latency | Sampling, async emission |
| Complex interruption state management | State machine, comprehensive tests |
| Provider-specific edge cases | Provider-specific unit tests |

### 10.3 Low Risk Items

| Risk | Mitigation |
|------|------------|
| Documentation drift | Automated doc generation |
| Test coverage gaps | CI coverage requirements |

---

## Appendix A: File Reference

### Pipecat Files Analyzed

| File | Lines | Purpose |
|------|-------|---------|
| `services/websocket_service.py` | 234 | WebSocket base class |
| `services/stt_service.py` | 328 | STT service patterns |
| `services/tts_service.py` | 400+ | TTS service patterns |
| `processors/frame_processor.py` | 1032 | Frame pipeline |
| `frames/frames.py` | 500+ | Frame definitions |
| `services/deepgram/stt.py` | 400+ | Deepgram STT impl |
| `services/deepgram/tts.py` | 300+ | Deepgram TTS impl |
| `services/elevenlabs/tts.py` | 1121 | ElevenLabs TTS impl |
| `services/cartesia/tts.py` | 846 | Cartesia TTS impl |

### Bud Waav Files to Modify

| File | Changes |
|------|---------|
| `src/core/websocket/mod.rs` | NEW - Module definition |
| `src/core/websocket/service.rs` | NEW - WebSocket base trait |
| `src/core/metrics.rs` | NEW - Metrics collection |
| `src/core/pipeline/mod.rs` | NEW - Pipeline module |
| `src/core/pipeline/interruption.rs` | NEW - Interruption handling |
| `src/core/stt/base.rs` | Add event handlers |
| `src/core/tts/base.rs` | Add event handlers |
| `src/core/stt/deepgram.rs` | Implement WebSocketService |
| `src/core/stt/elevenlabs/client.rs` | Implement WebSocketService |
| `src/core/tts/cartesia/provider.rs` | Add reconnection |
| `src/core/voice_manager/manager.rs` | Integrate interruption |

---

## Appendix B: Code Snippets from Pipecat

### B.1 Complete Reconnection Logic

```python
# From pipecat/services/websocket_service.py

async def _maybe_try_reconnect(self) -> bool:
    """Attempt to reconnect to the WebSocket server.

    Returns:
        True if reconnection was successful, False otherwise.
    """
    if not self._auto_reconnect:
        logger.debug(f"{self}: Auto-reconnect disabled")
        return False

    if self._disconnecting:
        logger.debug(f"{self}: Skipping reconnect - disconnecting intentionally")
        return False

    if self._reconnection_attempts >= self._max_reconnection_attempts:
        logger.error(
            f"{self}: Max reconnection attempts ({self._max_reconnection_attempts}) reached"
        )
        return False

    # Exponential backoff with jitter
    base_delay = min(2 ** self._reconnection_attempts, 30)  # Cap at 30 seconds
    jitter = random.uniform(0, base_delay * 0.1)  # 10% jitter
    delay = base_delay + jitter

    logger.info(
        f"{self}: Attempting reconnection {self._reconnection_attempts + 1}/"
        f"{self._max_reconnection_attempts} in {delay:.2f}s"
    )

    await asyncio.sleep(delay)
    self._reconnection_attempts += 1

    try:
        await self._try_reconnect()
        self._reconnection_attempts = 0  # Reset on success
        logger.info(f"{self}: Reconnection successful")
        return True
    except Exception as e:
        logger.warning(f"{self}: Reconnection failed: {e}")
        return await self._maybe_try_reconnect()  # Try again
```

### B.2 Frame Priority Queue

```python
# From pipecat/processors/frame_processor.py

class FrameProcessorQueue:
    def __init__(self):
        self._system_queue: asyncio.Queue = asyncio.Queue()
        self._data_queue: asyncio.Queue = asyncio.Queue()

    async def put(self, frame: Frame):
        """Add frame to appropriate queue based on type."""
        if isinstance(frame, SystemFrame):
            await self._system_queue.put(frame)
        else:
            await self._data_queue.put(frame)

    async def get(self) -> Frame:
        """Get next frame, prioritizing system frames."""
        # Non-blocking check for system frames
        try:
            return self._system_queue.get_nowait()
        except asyncio.QueueEmpty:
            pass

        # Wait for either queue
        done, pending = await asyncio.wait(
            [
                asyncio.create_task(self._system_queue.get()),
                asyncio.create_task(self._data_queue.get()),
            ],
            return_when=asyncio.FIRST_COMPLETED
        )

        # Cancel pending task
        for task in pending:
            task.cancel()

        # Return result from completed task
        return done.pop().result()
```

---

## Summary

This learning and implementation plan provides a comprehensive roadmap for improving Bud Waav's gateway based on battle-tested patterns from Pipecat AI. The highest priority items are:

1. **WebSocket Service Base Class** - Unified reconnection logic
2. **Event Handler Pattern** - Consistent lifecycle callbacks
3. **Reconnection for All Providers** - Not just Realtime

Implementation should follow the phased approach over 5 weeks, with comprehensive testing at each phase. The risk is manageable with proper feature flagging and gradual migration.

The end result will be a more robust, production-ready voice AI gateway with:
- Reliable connection handling under network instability
- Comprehensive metrics for monitoring and optimization
- Clean interruption handling for responsive user experience
- Consistent patterns across all providers

---

## Appendix C: Additional Analysis Findings (January 2026)

### C.1 Bud Waav Strengths Discovered

During deeper analysis, several strong patterns were identified in Bud Waav that should be preserved:

#### HTTP-Based TTS Providers Have Good Auto-Connect Pattern

Both `DeepgramTTS` and `CartesiaTTS` use HTTP REST APIs with a simple but effective auto-reconnect pattern:

```rust
// src/core/tts/deepgram.rs:173-179 and src/core/tts/cartesia/provider.rs:527-532
async fn speak(&mut self, text: &str, flush: bool) -> TTSResult<()> {
    // Handle reconnection if needed
    if !self.is_ready() {
        tracing::info!("TTS not ready, attempting to connect...");
        self.connect().await?;
    }
    // ... proceed with speak
}
```

**Recommendation**: Keep this pattern for HTTP-based TTS. The WebSocket service base class is primarily needed for WebSocket-based STT providers.

#### ReconnectionConfig Already Exists and Is Well-Designed

The `ReconnectionConfig` in `src/core/realtime/base.rs:82-157` is excellent:

```rust
pub struct ReconnectionConfig {
    pub enabled: bool,
    pub max_attempts: u32,
    pub initial_delay_ms: u64,
    pub max_delay_ms: u64,
    pub backoff_multiplier: f32,
    pub jitter: bool,
}

impl ReconnectionConfig {
    pub fn calculate_delay(&self, attempt: u32) -> u64 {
        // Exponential backoff with jitter
        let delay = base_delay * multiplier.powi(attempt.saturating_sub(1) as i32);
        let delay = delay.min(self.max_delay_ms as f64);
        if self.jitter {
            let jitter_range = delay * 0.25;
            let jitter = rand_jitter(jitter_range);
            (delay + jitter) as u64
        } else {
            delay as u64
        }
    }

    pub fn should_retry(&self, attempt: u32) -> bool {
        self.enabled && (self.max_attempts == 0 || attempt < self.max_attempts)
    }
}
```

**Recommendation**: Move `ReconnectionConfig` to `src/core/websocket/mod.rs` and reuse for STT providers.

#### Config Hashing for Caching

Both TTS providers use xxHash3-128 for config+text caching:

```rust
fn compute_tts_config_hash(config: &TTSConfig) -> String {
    let hash = xxh3_128(s.as_bytes());
    format!("{hash:032x}")
}
```

**Recommendation**: Keep this pattern - it enables efficient TTS response caching.

### C.2 Refined Priority Assessment

Based on detailed analysis, the priority should be refined:

| Item | Original Priority | Refined Priority | Reason |
|------|-------------------|------------------|--------|
| WebSocket Service Base Class | HIGH | **HIGH** | STT providers need this urgently |
| Event Handler Pattern | HIGH | **MEDIUM** | HTTP TTS already auto-reconnects |
| TTFB Metrics | MEDIUM | **HIGH** | Critical for latency monitoring in production |
| Interruption Handling | MEDIUM | **MEDIUM** | Important for UX but not blocking |
| Audio Context for TTS | LOW | **LOW** | Nice-to-have for prosody |

### C.3 Provider Implementation Status

| Provider | Type | Transport | Reconnection Status |
|----------|------|-----------|---------------------|
| Deepgram STT | STT | WebSocket | ❌ **MISSING** - Has keepalive only |
| ElevenLabs STT | STT | WebSocket | ❌ **MISSING** |
| Azure STT | STT | WebSocket | ⚠️ Unclear |
| Cartesia STT | STT | WebSocket | ❌ **MISSING** |
| Deepgram TTS | TTS | HTTP REST | ✅ Auto-connect in speak() |
| Cartesia TTS | TTS | HTTP REST | ✅ Auto-connect in speak() |
| ElevenLabs TTS | TTS | HTTP REST | ⚠️ Check needed |
| OpenAI Realtime | Realtime | WebSocket | ✅ Full reconnection support |

### C.4 Immediate Action Items

1. **Week 1 Focus**:
   - Move `ReconnectionConfig` to shared `src/core/websocket/mod.rs`
   - Create `WebSocketService` trait with reconnection logic
   - Update `DeepgramSTT` to implement `WebSocketService`

2. **Week 2 Focus**:
   - Add TTFB metrics collection to all providers
   - Create `MetricsCollector` struct
   - Add metrics endpoint to REST API

3. **Quick Wins**:
   - Add `on_connected`/`on_disconnected` event callbacks to `BaseSTT` trait
   - Ensure all WebSocket STT providers emit these events

### C.5 Pipecat Reference Files for Future Implementation

When implementing each feature, reference these Pipecat files:

| Feature | Pipecat Reference File | Key Lines |
|---------|------------------------|-----------|
| WebSocket reconnection | `services/websocket_service.py` | `_maybe_try_reconnect()`, `_try_reconnect()` |
| Exponential backoff | `utils/network.py` | `exponential_backoff_time()` |
| Event handlers | `services/stt_service.py` | `_emit_event()`, `_register_event_handler()` |
| TTFB metrics | `services/ai_service.py` | `start_ttfb()`, `stop_ttfb()` |
| Frame pipeline | `processors/frame_processor.py` | `FrameProcessorQueue`, input/process tasks |
| Interruption | `frames/frames.py` | `UninterruptibleFrame` mixin |
| Word timestamps | `services/elevenlabs/tts.py` | `_calculate_word_times()` |

---

## Appendix D: Deep Analysis Findings - Production Readiness Gaps (January 2026)

This section documents findings from a comprehensive second-pass analysis focusing on production readiness aspects: observability, scalability, real-time performance, and stability.

### D.1 Critical Observability Gaps

#### D.1.1 Observer Pattern (P0 - CRITICAL)

**Pipecat Pattern**: Non-intrusive observer pattern for pipeline monitoring
```python
# pipecat/observers/base_observer.py
class BaseObserver(BaseObject):
    """Non-intrusive monitoring without modifying pipeline structure."""

    async def on_process_frame(self, data: FrameProcessed):
        """Called when frame is processed by a processor."""
        pass

    async def on_push_frame(self, data: FramePushed):
        """Called when frame is pushed between processors."""
        pass

# pipecat/observers/loggers/metrics_log_observer.py
class MetricsLogObserver(BaseObserver):
    """Observer to log metrics activity to the console."""
    def __init__(self, include_metrics: Optional[Set[Type[MetricsData]]] = None):
        self._include_metrics = include_metrics
        self._frames_seen = set()  # Frame deduplication
```

**Bud Waav Gap**: No observer pattern. Monitoring requires modifying provider code or adding middleware.

**Recommended Implementation**:
```rust
// src/core/observability/observer.rs
pub trait Observer: Send + Sync {
    fn on_stt_result(&self, processor: &str, result: &STTResult, latency: Duration);
    fn on_tts_started(&self, processor: &str, text: &str);
    fn on_tts_audio_chunk(&self, processor: &str, chunk_size: usize);
    fn on_error(&self, processor: &str, error: &AppError);
    fn on_connection_state(&self, processor: &str, state: ConnectionState);
}

pub struct ObserverRegistry {
    observers: Vec<Arc<dyn Observer>>,
}
```

#### D.1.2 User-to-Bot Latency Tracking (P0 - CRITICAL)

**Pipecat Pattern**: Dedicated observer for end-to-end latency measurement
```python
# pipecat/observers/loggers/user_bot_latency_log_observer.py
class UserBotLatencyLogObserver(BaseObserver):
    """Measures time between user stopping speech and bot starting speech."""

    def __init__(self):
        self._user_stopped_time = 0
        self._latencies = []  # Historical latencies for statistics

    async def on_push_frame(self, data: FramePushed):
        if isinstance(data.frame, VADUserStoppedSpeakingFrame):
            self._user_stopped_time = time.time()
        elif isinstance(data.frame, BotStartedSpeakingFrame) and self._user_stopped_time:
            latency = time.time() - self._user_stopped_time
            self._latencies.append(latency)
            self._log_latency(latency)

    def _log_summary(self):
        avg = mean(self._latencies)
        min_lat = min(self._latencies)
        max_lat = max(self._latencies)
        logger.info(f"LATENCY - Avg: {avg:.3f}s, Min: {min_lat:.3f}s, Max: {max_lat:.3f}s")
```

**Bud Waav Gap**: No end-to-end latency tracking from user speech to bot response.

#### D.1.3 Per-Processor Metrics System (P1 - HIGH)

**Pipecat Pattern**: Built-in metrics for every frame processor
```python
# pipecat/processors/metrics/frame_processor_metrics.py
class FrameProcessorMetrics:
    async def start_ttfb_metrics(self, report_only_initial_ttfb):
        self._start_ttfb_time = time.time()
        self._should_report_ttfb = not report_only_initial_ttfb

    async def stop_ttfb_metrics(self) -> Optional[MetricsFrame]:
        self._last_ttfb_time = time.time() - self._start_ttfb_time
        return MetricsFrame(data=[TTFBMetricsData(...)])

    async def start_processing_metrics(self):
        self._start_processing_time = time.time()

    async def start_llm_usage_metrics(self, tokens: LLMTokenUsage):
        # Track prompt_tokens, completion_tokens, cache_read, reasoning
        pass

    async def start_tts_usage_metrics(self, text: str):
        # Track character count
        pass
```

**Bud Waav Gap**: Only has `QueueStats` in LiveKit operations. No per-provider TTFB or usage metrics.

### D.2 Real-Time Performance Gaps

#### D.2.1 Audio Chunking for Interruption Handling (P0 - CRITICAL)

**Pipecat Pattern**: Audio is chunked to enable responsive interruption
```python
# pipecat/transports/base_output.py
BOT_VAD_STOP_SECS = 0.35

# Chunk size: 10ms * CHUNKS (configurable)
audio_bytes_10ms = int(sample_rate / 100) * channels * 2
self._audio_chunk_size = audio_bytes_10ms * params.audio_out_10ms_chunks

async def handle_audio_frame(self, frame: OutputAudioRawFrame):
    self._audio_buffer.extend(resampled)
    while len(self._audio_buffer) >= self._audio_chunk_size:
        chunk = cls(bytes(self._audio_buffer[:self._audio_chunk_size]), ...)
        await self._audio_queue.put(chunk)
        self._audio_buffer = self._audio_buffer[self._audio_chunk_size:]
```

**Bud Waav Gap**: Audio frames passed through without chunking. Large frames delay interruption response.

#### D.2.2 Bot Speaking Detection (P1 - HIGH)

**Pipecat Pattern**: Sophisticated bot speech state tracking
```python
# Bot started/stopped speaking events with debouncing
async def _bot_currently_speaking(self):
    await self._bot_started_speaking()

    diff_time = time.time() - self._bot_speaking_frame_time
    if diff_time >= self._bot_speaking_frame_period:  # 0.2s
        await self._transport.broadcast_frame(BotSpeakingFrame)
        self._bot_speaking_frame_time = time.time()

    self._bot_speech_last_time = time.time()

async def _maybe_bot_currently_speaking(self, frame):
    if not is_silence(frame.audio):
        await self._bot_currently_speaking()
    else:
        silence_duration = time.time() - self._bot_speech_last_time
        if silence_duration > BOT_VAD_STOP_SECS:  # 0.35s
            await self._bot_stopped_speaking()
```

**Bud Waav Gap**: No bot speaking state tracking. Cannot detect when bot stops speaking to enable user input.

#### D.2.3 Frame Priority Queue (P1 - HIGH)

**Pipecat Pattern**: Two-tier priority system for frame processing
```python
# pipecat/processors/frame_processor.py
class FrameProcessorQueue(asyncio.PriorityQueue):
    HIGH_PRIORITY = 1  # SystemFrame
    LOW_PRIORITY = 2   # DataFrame, ControlFrame

    async def put(self, item):
        frame, _, _ = item
        if isinstance(frame, SystemFrame):
            self.__high_counter += 1
            await super().put((self.HIGH_PRIORITY, self.__high_counter, item))
        else:
            self.__low_counter += 1
            await super().put((self.LOW_PRIORITY, self.__low_counter, item))
```

**Bud Waav Gap**: `OperationPriority` exists in LiveKit but not applied to STT/TTS pipeline.

#### D.2.4 Direct Mode for Ultra-Low Latency (P2 - MEDIUM)

**Pipecat Pattern**: Skip queues for minimal latency paths
```python
class FrameProcessor:
    def __init__(self, *, enable_direct_mode: bool = False, ...):
        self._enable_direct_mode = enable_direct_mode

    async def queue_frame(self, frame, direction, callback):
        if self._enable_direct_mode:
            await self.__process_frame(frame, direction, callback)  # Skip queue
        else:
            await self.__input_queue.put((frame, direction, callback))
```

**Bud Waav Gap**: No way to bypass queues for latency-critical paths.

### D.3 Stability and Resilience Gaps

#### D.3.1 Graceful Shutdown with Signal Handling (P0 - CRITICAL)

**Pipecat Pattern**: Comprehensive shutdown management
```python
# pipecat/pipeline/runner.py
class PipelineRunner:
    def __init__(self, *, handle_sigint: bool = True, handle_sigterm: bool = False,
                 force_gc: bool = False, ...):
        if handle_sigint:
            loop.add_signal_handler(signal.SIGINT, lambda *args: self._sig_handler())
        if handle_sigterm:
            loop.add_signal_handler(signal.SIGTERM, lambda *args: self._sig_handler())

    async def stop_when_done(self):
        """Graceful: process all queued frames, then stop."""
        await asyncio.gather(*[t.stop_when_done() for t in self._tasks.values()])

    async def cancel(self):
        """Immediate: cancel all running tasks."""
        await asyncio.gather(*[t.cancel() for t in self._tasks.values()])

    def _gc_collect(self):
        collected = gc.collect()
        logger.debug(f"GC: collected {collected} objects, uncollectable: {gc.garbage}")
```

**Bud Waav Gap**: Has shutdown operations but no signal handling or graceful drain.

#### D.3.2 Comprehensive Cleanup Pattern (P1 - HIGH)

**Pipecat Pattern**: Hierarchical cleanup with proper ordering
```python
# Every processor has cleanup()
async def cleanup(self):
    await super().cleanup()
    await self.__cancel_input_task()
    await self.__cancel_process_task()
    if self._metrics:
        await self._metrics.cleanup()

# Pipeline cleans up all processors
async def _cleanup_processors(self):
    for p in self._processors:
        await p.cleanup()
```

**Bud Waav Gap**: Cleanup exists but inconsistent across providers.

#### D.3.3 Rate Limits Handling (P2 - MEDIUM)

**Pipecat Pattern**: Handle rate limit events from providers
```python
# pipecat/services/openai/realtime/events.py
class RateLimitsUpdated(ServerEvent):
    type: Literal["rate_limits.updated"]
    rate_limits: List[Dict[str, Any]]
```

**Bud Waav Gap**: Rate limit responses from providers not handled specially.

### D.4 Audio Processing Gaps

#### D.4.1 VAD State Machine (P1 - HIGH)

**Pipecat Pattern**: Proper state machine with configurable thresholds
```python
# pipecat/audio/vad/vad_analyzer.py
class VADState(Enum):
    QUIET = 1      # No voice
    STARTING = 2   # Voice beginning (debounce)
    SPEAKING = 3   # Active voice
    STOPPING = 4   # Voice ending (debounce)

class VADParams:
    confidence: float = 0.7      # Minimum confidence
    start_secs: float = 0.2      # Time before confirming start
    stop_secs: float = 0.8       # Time before confirming stop
    min_volume: float = 0.6      # Minimum volume threshold
```

**Bud Waav Gap**: Relies on provider VAD. No local VAD state machine with configurable debouncing.

#### D.4.2 Audio Mixer Support (P2 - MEDIUM)

**Pipecat Pattern**: Mix audio from multiple sources
```python
# pipecat/transports/base_output.py
self._mixer: Optional[BaseAudioMixer] = None

async def with_mixer(vad_stop_secs) -> AsyncGenerator[Frame, None]:
    silence = b"\x00" * self._audio_chunk_size
    while True:
        frame = self._audio_queue.get_nowait()
        if isinstance(frame, OutputAudioRawFrame):
            frame.audio = await self._mixer.mix(frame.audio)
        yield frame
```

**Bud Waav Gap**: No audio mixing capability for multi-source scenarios.

#### D.4.3 Stream Resampling (P2 - MEDIUM)

**Pipecat Pattern**: Dedicated stream vs file resamplers
```python
def create_stream_resampler(**kwargs) -> BaseAudioResampler:
    """For real-time streaming - maintains state between chunks."""
    return SOXRStreamAudioResampler(**kwargs)

def create_file_resampler(**kwargs) -> BaseAudioResampler:
    """For batch processing - no state needed."""
    return SOXRAudioResampler(**kwargs)
```

**Bud Waav Gap**: Uses single resampler approach. No streaming-optimized variant.

### D.5 Clock and Timing Gaps

#### D.5.1 Presentation Timestamps (P2 - MEDIUM)

**Pipecat Pattern**: Clock system with presentation timestamps
```python
# Frames have pts (presentation timestamp)
class Frame:
    pts: Optional[int] = None  # Presentation timestamp in nanoseconds

# Clock-based timed frame delivery
async def _clock_task_handler(self):
    while running:
        timestamp, _, frame = await self._clock_queue.get()
        current_time = self._transport.get_clock().get_time()
        if timestamp > current_time:
            wait_time = nanoseconds_to_seconds(timestamp - current_time)
            await asyncio.sleep(wait_time)
        await self._transport.push_frame(frame)
```

**Bud Waav Gap**: No presentation timestamp system. Frames processed immediately without timing.

### D.6 Production Readiness Matrix

| Category | Feature | Pipecat | Bud Waav | Priority | Effort |
|----------|---------|---------|----------|----------|--------|
| **Observability** | Observer Pattern | ✅ | ❌ | P0 | Medium |
| | User-Bot Latency | ✅ | ❌ | P0 | Low |
| | Per-Processor Metrics | ✅ | ⚠️ Partial | P1 | Medium |
| | Metrics Filtering | ✅ | ❌ | P2 | Low |
| **Real-Time** | Audio Chunking | ✅ | ❌ | P0 | Medium |
| | Bot Speaking Detection | ✅ | ❌ | P1 | Medium |
| | Frame Priority Queue | ✅ | ⚠️ LiveKit only | P1 | Medium |
| | Direct Mode | ✅ | ❌ | P2 | Low |
| **Stability** | Signal Handling | ✅ | ❌ | P0 | Low |
| | Graceful Drain | ✅ | ⚠️ Partial | P1 | Medium |
| | Rate Limit Handling | ✅ | ❌ | P2 | Low |
| **Audio** | VAD State Machine | ✅ | ❌ | P1 | High |
| | Audio Mixer | ✅ | ❌ | P2 | High |
| | Stream Resampler | ✅ | ⚠️ Partial | P2 | Medium |
| **Timing** | Presentation Timestamps | ✅ | ❌ | P2 | Medium |

### D.7 Revised Implementation Timeline

Based on priority and dependencies:

#### Phase 1: Critical Observability & Stability (Week 1-2)
1. Implement Observer trait and registry
2. Add signal handling (SIGINT/SIGTERM) to gateway
3. Implement User-Bot latency observer
4. Add graceful shutdown with drain

#### Phase 2: Real-Time Performance (Week 3-4)
1. Implement audio chunking in TTS output
2. Add bot speaking detection with debouncing
3. Extend priority queue to STT/TTS pipeline
4. Implement WebSocket service base class (from original plan)

#### Phase 3: Audio Processing (Week 5-6)
1. Implement local VAD state machine
2. Add VAD parameters (confidence, start_secs, stop_secs)
3. Create stream-optimized resampler
4. Add TTFB metrics to all providers

#### Phase 4: Advanced Features (Week 7-8)
1. Add direct mode for low-latency paths
2. Implement rate limit handling
3. Add presentation timestamp system
4. Audio mixer support (if needed)

### D.8 Pipecat Reference Files Summary

| Component | File | Key Classes/Functions |
|-----------|------|----------------------|
| Observer Base | `observers/base_observer.py` | `BaseObserver`, `FrameProcessed`, `FramePushed` |
| Metrics Observer | `observers/loggers/metrics_log_observer.py` | `MetricsLogObserver` |
| Latency Observer | `observers/loggers/user_bot_latency_log_observer.py` | `UserBotLatencyLogObserver` |
| Frame Processor | `processors/frame_processor.py` | `FrameProcessorQueue`, `FrameProcessor` |
| Processor Metrics | `processors/metrics/frame_processor_metrics.py` | `FrameProcessorMetrics` |
| Output Transport | `transports/base_output.py` | `MediaSender`, audio chunking, bot speaking |
| Pipeline Runner | `pipeline/runner.py` | `PipelineRunner`, signal handling |
| VAD Analyzer | `audio/vad/vad_analyzer.py` | `VADAnalyzer`, `VADState`, `VADParams` |
| Audio Utils | `audio/utils.py` | `is_silence`, `create_stream_resampler` |
| AI Service | `services/ai_service.py` | `AIService`, model management |

---

**Document Updated**: 2026-01-19
**Analysis Depth**: Comprehensive two-pass source code review focusing on production readiness
**Status**: Ready for implementation - includes original findings plus deep-dive on observability, real-time performance, stability, and audio processing
