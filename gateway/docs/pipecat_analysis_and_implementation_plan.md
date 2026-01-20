# Pipecat Analysis & Bud Waav Implementation Plan

## Executive Summary

After extensive analysis of Pipecat AI (a battle-tested Python-based voice AI framework), this document identifies:
1. **Architectural patterns** Pipecat uses that could improve Bud Waav
2. **Gaps in Bud Waav** compared to Pipecat's mature implementation
3. **Potential issues** in current Bud Waav provider integrations
4. **Detailed implementation plan** for improvements

---

## Part 1: Pipecat Architecture Analysis

### 1.1 Core Architecture

Pipecat uses a **Frame-based Pipeline Architecture**:

```
┌─────────────────────────────────────────────────────────────────┐
│                         Pipeline                                 │
│  ┌─────────┐   ┌─────────┐   ┌─────────┐   ┌─────────┐          │
│  │ Input   │──▶│   STT   │──▶│   LLM   │──▶│   TTS   │──▶Output │
│  │Transport│   │ Service │   │ Service │   │ Service │          │
│  └─────────┘   └─────────┘   └─────────┘   └─────────┘          │
│       │             │             │             │                │
│       ▼             ▼             ▼             ▼                │
│  ┌──────────────────────────────────────────────────┐           │
│  │              Frame Processing Queue               │           │
│  │   (DataFrame, SystemFrame, ControlFrame)          │           │
│  └──────────────────────────────────────────────────┘           │
└─────────────────────────────────────────────────────────────────┘
```

**Key Components:**

1. **Frames** (`frames.py`):
   - `Frame` - Base class with ID, name, PTS (presentation timestamp), metadata
   - `DataFrame` - Processed in order, cancelled by interruptions
   - `SystemFrame` - Higher priority, not affected by interruptions
   - `ControlFrame` - Control information, cancelled by interruptions
   - `UninterruptibleFrame` - Marker mixin for frames that must complete

2. **Services** (`ai_service.py`, `stt_service.py`, `tts_service.py`):
   - Abstract base classes with lifecycle methods: `start()`, `stop()`, `cancel()`
   - Metrics generation support (`can_generate_metrics()`, TTFB metrics)
   - Event handler registration pattern
   - Model/language switching with automatic reconnection

3. **WebSocket Services** (`websocket_service.py`):
   - Automatic reconnection with exponential backoff
   - Connection verification via ping
   - Concurrent reconnection prevention
   - Graceful disconnect handling

### 1.2 Key Design Patterns in Pipecat

#### Pattern 1: Frame-Based Data Flow
```python
@dataclass
class Frame:
    id: int = field(init=False)
    name: str = field(init=False)
    pts: Optional[int] = field(init=False)  # Presentation timestamp
    metadata: Dict[str, Any] = field(init=False)
    transport_source: Optional[str] = field(init=False)
    transport_destination: Optional[str] = field(init=False)
```

**Why this matters:**
- Enables tracing through the entire pipeline
- Supports multiple audio/video tracks
- Allows frame-level metrics and debugging

#### Pattern 2: Service Lifecycle Management
```python
async def start(self, frame: StartFrame):
    await super().start(frame)
    self._settings["sample_rate"] = self.sample_rate
    await self._connect()

async def stop(self, frame: EndFrame):
    await super().stop(frame)
    await self._disconnect()

async def cancel(self, frame: CancelFrame):
    await super().cancel(frame)
    await self._disconnect()
```

**Why this matters:**
- Clean resource management
- Proper shutdown sequencing
- Interrupt handling

#### Pattern 3: Automatic WebSocket Reconnection
```python
async def _try_reconnect(self, max_retries: int = 3, report_error: Callable):
    if self._reconnect_in_progress:
        return False
    self._reconnect_in_progress = True
    try:
        for attempt in range(1, max_retries + 1):
            try:
                if await self._reconnect_websocket(attempt):
                    return True
            except Exception as e:
                logger.error(f"Reconnection attempt {attempt} failed: {e}")
            wait_time = exponential_backoff_time(attempt)
            await asyncio.sleep(wait_time)
        return False
    finally:
        self._reconnect_in_progress = False
```

**Why this matters:**
- Production resilience
- Prevents connection drop failures
- Handles network instability gracefully

#### Pattern 4: TTFB (Time To First Byte) Metrics
```python
async def start_ttfb_metrics(self):
    """Start TTFB timer."""
    self._ttfb_start = time.time()

async def stop_ttfb_metrics(self):
    """Stop TTFB timer and report."""
    if self._ttfb_start:
        ttfb = time.time() - self._ttfb_start
        await self.push_frame(TTFBMetricsFrame(ttfb=ttfb))
        self._ttfb_start = None
```

**Why this matters:**
- Performance monitoring
- SLA compliance
- Latency debugging

#### Pattern 5: VAD State Machine
```python
class VADState(Enum):
    QUIET = 1      # No voice activity
    STARTING = 2   # Voice beginning, transitioning
    SPEAKING = 3   # Active voice confirmed
    STOPPING = 4   # Voice ending, transitioning

# State transitions with configurable thresholds
VAD_CONFIDENCE = 0.7
VAD_START_SECS = 0.2  # Wait before confirming speech start
VAD_STOP_SECS = 0.8   # Wait before confirming speech end
VAD_MIN_VOLUME = 0.6  # Minimum volume threshold
```

**Why this matters:**
- Reduces false positives
- Smooth user experience
- Configurable per-use-case

---

## Part 2: Identified Gaps & Issues in Bud Waav

### 2.1 Critical Gaps

| Gap | Pipecat Has | Bud Waav Status | Priority |
|-----|-------------|-----------------|----------|
| **Frame/Message Pipeline** | Full frame-based pipeline with PTS | Direct callbacks, no unified message type | P0 |
| **WebSocket Auto-Reconnect** | Exponential backoff, verification | Basic connect/disconnect | P0 |
| **TTFB Metrics** | Built-in per-service | Not implemented | P1 |
| **Interruption Handling** | SystemFrame, UninterruptibleFrame | Manual interrupt signals | P1 |
| **VAD State Machine** | QUIET→STARTING→SPEAKING→STOPPING | Binary speech detection | P1 |
| **Word Timestamps in TTS** | Word-level timing for sync | Not supported | P2 |
| **Service Lifecycle** | start/stop/cancel with frames | connect/disconnect | P2 |

### 2.2 Provider-Specific Issues Found

#### Issue 1: Deepgram STT - No Finalize Support
**Pipecat Implementation:**
```python
async def process_frame(self, frame: Frame, direction: FrameDirection):
    if isinstance(frame, VADUserStoppedSpeakingFrame):
        # https://developers.deepgram.com/docs/finalize
        await self._connection.finalize()
```

**Bud Waav Current:** No `finalize()` call when user stops speaking, which can cause:
- Delayed final transcription
- Missing last words
- Poor turn-taking behavior

**Fix Required:** Add finalize call on speech end detection.

#### Issue 2: ElevenLabs TTS - Missing Word Timestamps
**Pipecat Implementation:**
```python
def calculate_word_times(alignment_info, cumulative_time, partial_word=""):
    """Calculate word timestamps from character alignment."""
    chars = alignment_info["chars"]
    char_start_times_ms = alignment_info["charStartTimesMs"]
    # Build word timing from character-level alignment
```

**Bud Waav Current:** Not processing ElevenLabs alignment data for word timing.

**Fix Required:** Parse alignment_info from ElevenLabs WebSocket messages.

#### Issue 3: WebSocket Reconnection - No Automatic Retry
**Pipecat Implementation:**
```python
async def _receive_task_handler(self, report_error):
    while True:
        try:
            await self._receive_messages()
        except ConnectionClosedError as e:
            should_continue = await self._maybe_try_reconnect(e, message, report_error)
            if not should_continue:
                break
```

**Bud Waav Current:** WebSocket errors cause immediate failure.

**Fix Required:** Implement exponential backoff reconnection.

#### Issue 4: OpenAI Realtime - Missing Session Management
**Pipecat Implementation:**
- Tracks `_context_id` for conversation continuity
- Handles `conversation.item.created`, `conversation.item.updated` events
- Proper function call tracking with `_pending_function_calls`

**Bud Waav Current:** Basic message handling without full session state management.

#### Issue 5: VAD Integration - Binary vs State Machine
**Pipecat Implementation:**
```python
class VADAnalyzer:
    def analyze_audio(self, buffer: bytes) -> VADState:
        confidence = self.voice_confidence(audio_frames)
        volume = self._get_smoothed_volume(audio_frames)
        speaking = confidence >= self._params.confidence and volume >= self._params.min_volume

        # State machine transitions with debouncing
        if speaking:
            match self._vad_state:
                case VADState.QUIET: self._vad_state = VADState.STARTING
                case VADState.STARTING: self._vad_starting_count += 1
                case VADState.STOPPING: self._vad_state = VADState.SPEAKING
```

**Bud Waav Current:** Direct VAD confidence without state transitions.

#### Issue 6: Metrics Collection - No Standardization
**Pipecat Implementation:**
```python
class STTService(AIService):
    async def start_ttfb_metrics(self):
    async def stop_ttfb_metrics(self):
    async def start_processing_metrics(self):
    async def stop_processing_metrics(self):
```

**Bud Waav Current:** No standardized metrics collection across providers.

### 2.3 Missing Provider Features

| Provider | Pipecat Feature | Bud Waav Missing |
|----------|-----------------|------------------|
| Deepgram STT | `finalize()` on VAD stop | Yes |
| Deepgram TTS | Clear/Flush messages | Partial |
| ElevenLabs TTS | Word timestamps from alignment | Yes |
| ElevenLabs TTS | Audio context management (v1 multi API) | Yes |
| OpenAI Realtime | Full event handling (15+ event types) | Partial |
| Cartesia | Pronunciation dictionaries | Yes |
| Azure | Profanity filtering options | Partial |
| Silero VAD | Model state reset timing | Yes |

---

## Part 3: Implementation Plan

### Phase 1: Core Infrastructure (Week 1-2)

#### 1.1 Implement Frame-Based Message System

**Create `src/core/frame/mod.rs`:**
```rust
/// Base frame trait for all messages in the pipeline
pub trait Frame: Send + Sync {
    fn id(&self) -> u64;
    fn name(&self) -> &str;
    fn pts(&self) -> Option<Duration>;  // Presentation timestamp
    fn metadata(&self) -> &HashMap<String, Value>;
}

/// Data frames - processed in order, cancelled by interruptions
pub struct DataFrame {
    pub id: u64,
    pub pts: Option<Duration>,
    pub metadata: HashMap<String, Value>,
}

/// System frames - higher priority, not affected by interruptions
pub struct SystemFrame {
    pub id: u64,
    pub priority: u8,
}

/// Control frames - control information
pub struct ControlFrame {
    pub id: u64,
    pub action: ControlAction,
}

/// Audio frame with sample info
pub struct AudioRawFrame {
    pub audio: Bytes,
    pub sample_rate: u32,
    pub num_channels: u16,
    pub num_frames: usize,
}

/// Transcription result frame
pub struct TranscriptionFrame {
    pub transcript: String,
    pub is_final: bool,
    pub confidence: f32,
    pub language: Option<String>,
    pub timestamp: DateTime<Utc>,
}

/// TTS audio frame
pub struct TTSAudioRawFrame {
    pub audio: Bytes,
    pub sample_rate: u32,
    pub word_timestamps: Option<Vec<WordTiming>>,
}
```

#### 1.2 Implement WebSocket Auto-Reconnect

**Create `src/utils/websocket_resilient.rs`:**
```rust
pub struct ResilientWebSocket {
    url: String,
    reconnect_on_error: bool,
    max_retries: u32,
    reconnect_in_progress: AtomicBool,
    ws: Option<WebSocketStream<MaybeTlsStream<TcpStream>>>,
}

impl ResilientWebSocket {
    pub async fn try_reconnect(&self) -> Result<bool, WsError> {
        if self.reconnect_in_progress.swap(true, Ordering::SeqCst) {
            return Ok(false);  // Already reconnecting
        }

        let _guard = scopeguard::guard((), |_| {
            self.reconnect_in_progress.store(false, Ordering::SeqCst);
        });

        for attempt in 1..=self.max_retries {
            match self.reconnect_websocket(attempt).await {
                Ok(true) => return Ok(true),
                Ok(false) | Err(_) => {
                    let wait_time = exponential_backoff_time(attempt);
                    tokio::time::sleep(wait_time).await;
                }
            }
        }
        Ok(false)
    }

    pub async fn send_with_retry(&self, message: Message) -> Result<(), WsError> {
        match self.send(message.clone()).await {
            Ok(()) => Ok(()),
            Err(e) => {
                if self.try_reconnect().await? {
                    self.send(message).await
                } else {
                    Err(e)
                }
            }
        }
    }
}
```

#### 1.3 Implement TTFB Metrics

**Add to `src/core/metrics/mod.rs`:**
```rust
pub struct ServiceMetrics {
    ttfb_start: Option<Instant>,
    processing_start: Option<Instant>,
    ttfb_histogram: Histogram,
    processing_histogram: Histogram,
}

impl ServiceMetrics {
    pub fn start_ttfb(&mut self) {
        self.ttfb_start = Some(Instant::now());
    }

    pub fn stop_ttfb(&mut self) -> Option<Duration> {
        self.ttfb_start.take().map(|start| {
            let ttfb = start.elapsed();
            self.ttfb_histogram.record(ttfb.as_millis() as f64);
            ttfb
        })
    }

    pub fn start_processing(&mut self) {
        self.processing_start = Some(Instant::now());
    }

    pub fn stop_processing(&mut self) -> Option<Duration> {
        self.processing_start.take().map(|start| {
            let duration = start.elapsed();
            self.processing_histogram.record(duration.as_millis() as f64);
            duration
        })
    }
}
```

### Phase 2: Provider Fixes (Week 3-4)

#### 2.1 Fix Deepgram STT - Add Finalize Support

**Modify `src/core/stt/deepgram/client.rs`:**
```rust
impl DeepgramSTT {
    /// Send finalize message to get final transcription
    pub async fn finalize(&self) -> Result<(), STTError> {
        if let Some(ws) = &self.ws {
            let finalize_msg = json!({
                "type": "Finalize"
            });
            ws.send(Message::Text(finalize_msg.to_string())).await?;
        }
        Ok(())
    }

    /// Handle VAD user stopped speaking
    pub async fn on_user_stopped_speaking(&self) -> Result<(), STTError> {
        // Trigger finalize to get remaining transcription
        self.finalize().await?;
        Ok(())
    }
}
```

#### 2.2 Fix ElevenLabs TTS - Add Word Timestamps

**Modify `src/core/tts/elevenlabs/client.rs`:**
```rust
#[derive(Debug, Deserialize)]
struct AlignmentInfo {
    chars: Vec<char>,
    char_start_times_ms: Vec<u64>,
}

impl ElevenLabsTTS {
    fn calculate_word_times(
        &self,
        alignment: &AlignmentInfo,
        cumulative_time: f64,
        partial_word: &str,
    ) -> (Vec<WordTiming>, String, f64) {
        let mut words = Vec::new();
        let mut word_start_times = Vec::new();
        let mut current_word = partial_word.to_string();
        let mut word_start_time: Option<f64> = None;

        for (i, char) in alignment.chars.iter().enumerate() {
            if *char == ' ' {
                if !current_word.is_empty() {
                    words.push(current_word.clone());
                    word_start_times.push(word_start_time.unwrap_or(0.0));
                    current_word.clear();
                    word_start_time = None;
                }
            } else {
                if word_start_time.is_none() {
                    word_start_time = Some(
                        cumulative_time + (alignment.char_start_times_ms[i] as f64 / 1000.0)
                    );
                }
                current_word.push(*char);
            }
        }

        let word_times: Vec<WordTiming> = words
            .into_iter()
            .zip(word_start_times)
            .map(|(word, start)| WordTiming { word, start_time: start })
            .collect();

        (word_times, current_word, word_start_time.unwrap_or(0.0))
    }
}
```

#### 2.3 Implement VAD State Machine

**Create `src/core/vad/state_machine.rs`:**
```rust
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VADState {
    Quiet,
    Starting,
    Speaking,
    Stopping,
}

pub struct VADParams {
    pub confidence_threshold: f32,
    pub start_secs: f32,
    pub stop_secs: f32,
    pub min_volume: f32,
}

impl Default for VADParams {
    fn default() -> Self {
        Self {
            confidence_threshold: 0.7,
            start_secs: 0.2,
            stop_secs: 0.8,
            min_volume: 0.6,
        }
    }
}

pub struct VADStateMachine {
    state: VADState,
    params: VADParams,
    starting_count: u32,
    stopping_count: u32,
    start_frames_threshold: u32,
    stop_frames_threshold: u32,
    prev_volume: f32,
}

impl VADStateMachine {
    pub fn new(params: VADParams, sample_rate: u32, frames_per_analysis: u32) -> Self {
        let frames_per_sec = sample_rate as f32 / frames_per_analysis as f32;
        Self {
            state: VADState::Quiet,
            params,
            starting_count: 0,
            stopping_count: 0,
            start_frames_threshold: (params.start_secs * frames_per_sec) as u32,
            stop_frames_threshold: (params.stop_secs * frames_per_sec) as u32,
            prev_volume: 0.0,
        }
    }

    pub fn analyze(&mut self, confidence: f32, volume: f32) -> VADState {
        let smoothed_volume = exp_smoothing(volume, self.prev_volume, 0.2);
        self.prev_volume = smoothed_volume;

        let speaking = confidence >= self.params.confidence_threshold
            && smoothed_volume >= self.params.min_volume;

        if speaking {
            match self.state {
                VADState::Quiet => {
                    self.state = VADState::Starting;
                    self.starting_count = 1;
                }
                VADState::Starting => {
                    self.starting_count += 1;
                }
                VADState::Stopping => {
                    self.state = VADState::Speaking;
                    self.stopping_count = 0;
                }
                VADState::Speaking => {}
            }
        } else {
            match self.state {
                VADState::Starting => {
                    self.state = VADState::Quiet;
                    self.starting_count = 0;
                }
                VADState::Speaking => {
                    self.state = VADState::Stopping;
                    self.stopping_count = 1;
                }
                VADState::Stopping => {
                    self.stopping_count += 1;
                }
                VADState::Quiet => {}
            }
        }

        // Check threshold transitions
        if self.state == VADState::Starting && self.starting_count >= self.start_frames_threshold {
            self.state = VADState::Speaking;
            self.starting_count = 0;
        }
        if self.state == VADState::Stopping && self.stopping_count >= self.stop_frames_threshold {
            self.state = VADState::Quiet;
            self.stopping_count = 0;
        }

        self.state
    }
}
```

### Phase 3: Service Lifecycle & Interruption Handling (Week 5-6)

#### 3.1 Implement Service Lifecycle Trait

**Create `src/core/service/lifecycle.rs`:**
```rust
#[async_trait]
pub trait ServiceLifecycle: Send + Sync {
    /// Initialize service resources
    async fn start(&mut self, config: &ServiceConfig) -> Result<(), ServiceError>;

    /// Graceful shutdown
    async fn stop(&mut self) -> Result<(), ServiceError>;

    /// Immediate cancellation
    async fn cancel(&mut self) -> Result<(), ServiceError>;

    /// Check if service is ready
    fn is_ready(&self) -> bool;

    /// Get service state
    fn state(&self) -> ServiceState;
}

#[derive(Debug, Clone, PartialEq)]
pub enum ServiceState {
    Uninitialized,
    Starting,
    Ready,
    Processing,
    Stopping,
    Stopped,
    Error(String),
}
```

#### 3.2 Implement Interruption Handler

**Create `src/core/interruption/mod.rs`:**
```rust
pub struct InterruptionHandler {
    interrupt_tx: broadcast::Sender<InterruptionFrame>,
    active_tasks: Arc<RwLock<Vec<JoinHandle<()>>>>,
}

impl InterruptionHandler {
    pub async fn handle_interruption(&self, frame: InterruptionFrame) {
        // Broadcast interruption to all listeners
        let _ = self.interrupt_tx.send(frame);

        // Cancel non-uninterruptible tasks
        let mut tasks = self.active_tasks.write().await;
        for task in tasks.drain(..) {
            task.abort();
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<InterruptionFrame> {
        self.interrupt_tx.subscribe()
    }
}

pub struct InterruptionFrame {
    pub source: String,
    pub timestamp: Instant,
}
```

### Phase 4: Enhanced Provider Integration (Week 7-8)

#### 4.1 OpenAI Realtime - Full Event Handling

**Events to implement:**
```rust
pub enum OpenAIRealtimeEvent {
    // Session events
    SessionCreated(SessionCreatedEvent),
    SessionUpdated(SessionUpdatedEvent),

    // Input audio events
    InputAudioBufferCommitted(InputAudioBufferCommittedEvent),
    InputAudioBufferCleared(InputAudioBufferClearedEvent),
    InputAudioBufferSpeechStarted(SpeechStartedEvent),
    InputAudioBufferSpeechStopped(SpeechStoppedEvent),

    // Conversation events
    ConversationItemCreated(ConversationItemCreatedEvent),
    ConversationItemInputAudioTranscriptionCompleted(TranscriptionCompletedEvent),
    ConversationItemInputAudioTranscriptionFailed(TranscriptionFailedEvent),
    ConversationItemDeleted(ConversationItemDeletedEvent),

    // Response events
    ResponseCreated(ResponseCreatedEvent),
    ResponseDone(ResponseDoneEvent),
    ResponseOutputItemAdded(ResponseOutputItemAddedEvent),
    ResponseOutputItemDone(ResponseOutputItemDoneEvent),
    ResponseContentPartAdded(ResponseContentPartAddedEvent),
    ResponseContentPartDone(ResponseContentPartDoneEvent),
    ResponseTextDelta(ResponseTextDeltaEvent),
    ResponseTextDone(ResponseTextDoneEvent),
    ResponseAudioDelta(ResponseAudioDeltaEvent),
    ResponseAudioDone(ResponseAudioDoneEvent),
    ResponseAudioTranscriptDelta(AudioTranscriptDeltaEvent),
    ResponseAudioTranscriptDone(AudioTranscriptDoneEvent),

    // Function call events
    ResponseFunctionCallArgumentsDelta(FunctionCallArgumentsDeltaEvent),
    ResponseFunctionCallArgumentsDone(FunctionCallArgumentsDoneEvent),

    // Rate limits
    RateLimitsUpdated(RateLimitsUpdatedEvent),

    // Error
    Error(ErrorEvent),
}
```

#### 4.2 Silero VAD - Proper State Reset

**Add model state reset timing:**
```rust
const MODEL_RESET_STATES_INTERVAL: Duration = Duration::from_secs(5);

impl SileroVAD {
    pub fn analyze(&mut self, audio: &[i16]) -> f32 {
        let confidence = self.model.forward(audio)?;

        // Reset model state periodically to prevent memory growth
        let now = Instant::now();
        if now.duration_since(self.last_reset) >= MODEL_RESET_STATES_INTERVAL {
            self.model.reset_states();
            self.last_reset = now;
        }

        confidence
    }
}
```

### Phase 5: Testing & Validation (Week 9-10)

#### 5.1 Create Differential Tests

Compare Pipecat output vs Bud Waav output for same inputs:

```rust
#[tokio::test]
async fn test_deepgram_stt_matches_pipecat_behavior() {
    // 1. Send same audio to both
    // 2. Compare transcription results
    // 3. Compare timing (TTFB, total time)
    // 4. Compare finalize behavior
}

#[tokio::test]
async fn test_elevenlabs_tts_word_timestamps() {
    // 1. Send text to TTS
    // 2. Verify word timestamps are populated
    // 3. Verify timestamps are monotonically increasing
    // 4. Verify total duration matches audio length
}

#[tokio::test]
async fn test_vad_state_machine_transitions() {
    // 1. Send silent audio -> expect QUIET
    // 2. Send speech audio -> expect STARTING -> SPEAKING
    // 3. Send silent audio -> expect STOPPING -> QUIET
    // 4. Verify debounce timing
}

#[tokio::test]
async fn test_websocket_reconnection() {
    // 1. Connect successfully
    // 2. Simulate connection drop
    // 3. Verify automatic reconnection
    // 4. Verify exponential backoff timing
}
```

---

## Part 4: Provider-Specific Learnings

### Deepgram

**Pipecat Does:**
1. Sends `Finalize` message on VAD speech stop
2. Handles `UtteranceEnd` event for turn detection
3. Uses `LiveOptions` with smart defaults (nova-3-general, interim_results=True)
4. Properly handles deprecated `vad_events` parameter

**Bud Waav Should Add:**
- `finalize()` method
- `UtteranceEnd` event handling
- Deprecation warnings for old parameters

### ElevenLabs

**Pipecat Does:**
1. Tracks `_cumulative_time` for word timestamp calculation
2. Handles partial words across alignment chunks
3. Uses context management for v1 multi API
4. Sends keepalive messages

**Bud Waav Should Add:**
- Word timestamp calculation from alignment
- Partial word tracking
- Audio context management
- Keepalive task

### Azure

**Pipecat Does:**
1. Supports multiple output formats (linear16, mulaw, alaw)
2. Handles SSML with language tags
3. Supports profanity filtering options
4. Region-specific endpoint construction

**Bud Waav Should Add:**
- More output format options
- Profanity filter configuration
- Better SSML support

### Cartesia

**Pipecat Does:**
1. Uses `add_context` for audio context continuity
2. Supports pronunciation dictionaries
3. Handles word timestamps
4. Supports streaming with proper flush

**Bud Waav Should Add:**
- Audio context support
- Pronunciation dictionary support
- Word timestamp extraction

### Google

**Pipecat Does:**
1. Uses gRPC streaming for STT
2. Supports multiple encodings
3. Handles interim results with proper timing
4. Uses service account authentication

**Bud Waav Current Status:** Already has gRPC support, verify alignment with Pipecat patterns.

---

## Part 5: Priority Implementation Matrix

| Item | Impact | Effort | Priority |
|------|--------|--------|----------|
| WebSocket Auto-Reconnect | High | Medium | P0 |
| Frame-Based Pipeline | High | High | P0 |
| Deepgram Finalize | High | Low | P0 |
| TTFB Metrics | Medium | Low | P1 |
| VAD State Machine | High | Medium | P1 |
| ElevenLabs Word Timestamps | Medium | Medium | P1 |
| Interruption Handling | High | Medium | P1 |
| Service Lifecycle | Medium | Medium | P2 |
| OpenAI Realtime Events | Medium | High | P2 |
| Cartesia Audio Context | Low | Medium | P3 |

---

## Part 6: Success Metrics

### After Implementation, Measure:

1. **Reliability**
   - WebSocket reconnection success rate: >99%
   - Provider error recovery rate: >95%

2. **Latency**
   - STT TTFB: <200ms (p99)
   - TTS TTFB: <300ms (p99)
   - VAD latency: <50ms

3. **Accuracy**
   - Word timestamp accuracy: within 50ms of actual
   - VAD false positive rate: <5%
   - Transcription word accuracy: match provider's published WER

4. **Resource Usage**
   - Memory growth over 1hr session: <10%
   - Connection pool efficiency: >90%

---

## Appendix A: File Changes Required

### New Files to Create:
```
src/core/frame/mod.rs          # Frame types
src/core/frame/audio.rs        # Audio frame types
src/core/frame/text.rs         # Text/transcription frames
src/core/frame/control.rs      # Control frames
src/utils/websocket_resilient.rs # Resilient WebSocket
src/core/metrics/ttfb.rs       # TTFB metrics
src/core/vad/state_machine.rs  # VAD state machine
src/core/service/lifecycle.rs  # Service lifecycle trait
src/core/interruption/mod.rs   # Interruption handling
```

### Files to Modify:
```
src/core/stt/deepgram/client.rs     # Add finalize()
src/core/stt/base.rs                # Add ServiceLifecycle impl
src/core/tts/elevenlabs/client.rs   # Add word timestamps
src/core/tts/base.rs                # Add ServiceLifecycle impl
src/core/realtime/openai/client.rs  # Add full event handling
src/utils/noise_filter.rs           # Add VAD state machine integration
```

---

## Appendix B: Pipecat Code References

Key files analyzed:
- `/tmp/pipecat/src/pipecat/services/ai_service.py` - Base service class
- `/tmp/pipecat/src/pipecat/services/stt_service.py` - STT base with metrics
- `/tmp/pipecat/src/pipecat/services/tts_service.py` - TTS base with word timing
- `/tmp/pipecat/src/pipecat/services/websocket_service.py` - Reconnection logic
- `/tmp/pipecat/src/pipecat/services/deepgram/stt.py` - Deepgram STT impl
- `/tmp/pipecat/src/pipecat/services/deepgram/tts.py` - Deepgram TTS impl
- `/tmp/pipecat/src/pipecat/services/elevenlabs/tts.py` - ElevenLabs TTS impl
- `/tmp/pipecat/src/pipecat/services/openai/realtime/llm.py` - OpenAI Realtime
- `/tmp/pipecat/src/pipecat/audio/vad/vad_analyzer.py` - VAD base class
- `/tmp/pipecat/src/pipecat/audio/vad/silero.py` - Silero VAD impl
- `/tmp/pipecat/src/pipecat/frames/frames.py` - Frame definitions

---

---

## Part 7: Deep Dive - New Patterns from Pipecat Analysis (January 2026)

### 7.1 Audio Context Management Pattern (TTSService)

Pipecat implements sophisticated audio context management for TTS services:

```python
class AudioContextTTSService(TTSService):
    """TTS service that correlates generated audio with the text that was requested."""

    def __init__(self, **kwargs):
        super().__init__(**kwargs)
        self._context_id: Optional[str] = None
        self._pending_audio_contexts: Dict[str, TTSAudioContext] = {}
        self._current_audio_context: Optional[TTSAudioContext] = None

    async def push_frame(self, frame: Frame, direction: FrameDirection):
        """Track audio context for correlation."""
        if isinstance(frame, TTSStartedFrame) and frame.context_id:
            self._current_audio_context = self._pending_audio_contexts.get(frame.context_id)
        elif isinstance(frame, TTSAudioRawFrame) and self._current_audio_context:
            frame.metadata["context_text"] = self._current_audio_context.text
```

**Why this matters:**
- Enables tracking which audio corresponds to which text request
- Supports multi-turn conversations with proper attribution
- Essential for logging, analytics, and debugging

**Implementation for Bud Waav:**
```rust
pub struct TTSAudioContext {
    pub context_id: String,
    pub text: String,
    pub created_at: Instant,
    pub word_timestamps: Option<Vec<WordTiming>>,
}

pub struct AudioContextManager {
    pending_contexts: HashMap<String, TTSAudioContext>,
    current_context: Option<TTSAudioContext>,
}

impl AudioContextManager {
    pub fn create_context(&mut self, text: &str) -> String {
        let context_id = Uuid::new_v4().to_string();
        let context = TTSAudioContext {
            context_id: context_id.clone(),
            text: text.to_string(),
            created_at: Instant::now(),
            word_timestamps: None,
        };
        self.pending_contexts.insert(context_id.clone(), context);
        context_id
    }

    pub fn activate_context(&mut self, context_id: &str) -> Option<&TTSAudioContext> {
        self.current_context = self.pending_contexts.remove(context_id);
        self.current_context.as_ref()
    }
}
```

### 7.2 Interruptible TTS Pattern

Pipecat has a sophisticated interruption handling pattern for TTS:

```python
class InterruptibleTTSService(WebsocketTTSService):
    """TTS service that handles interruptions by reconnecting."""

    async def _handle_interruption(self, frame: InterruptionFrame, direction: FrameDirection):
        await super()._handle_interruption(frame, direction)
        if self._bot_speaking:
            # Disconnect and reconnect to clear any pending audio
            await self._disconnect()
            await self._connect()
```

**Critical Pattern - Clear Message on Interruption (Deepgram TTS):**
```python
async def _handle_interruption(self, frame: InterruptionFrame, direction: FrameDirection):
    await super()._handle_interruption(frame, direction)
    if self._websocket:
        try:
            clear_msg = {"type": "Clear"}
            await self._websocket.send(json.dumps(clear_msg))
        except Exception as e:
            logger.error(f"{self} error sending Clear message: {e}")
```

**Implementation for Bud Waav:**
```rust
#[async_trait]
pub trait InterruptibleTTS: BaseTTS {
    async fn handle_interruption(&mut self) -> TTSResult<()> {
        if self.is_speaking() {
            // Send clear message if WebSocket-based
            if let Some(ws) = self.get_websocket() {
                let clear_msg = json!({"type": "Clear"});
                ws.send(Message::Text(clear_msg.to_string())).await?;
            }

            // Reconnect to ensure clean state
            self.disconnect().await?;
            self.connect().await?;
        }
        Ok(())
    }

    fn is_speaking(&self) -> bool;
    fn get_websocket(&self) -> Option<&WebSocket>;
}
```

### 7.3 Text Aggregation and Filtering Pattern

Pipecat's TTSService has sophisticated text handling:

```python
class TTSService(AIService):
    def __init__(
        self,
        *,
        aggregate_sentences: bool = False,  # Aggregate text until sentence boundary
        text_filters: List[Callable[[str], str]] = [],  # Transform text before TTS
        text_filter_fn: Optional[Callable[[str], str]] = None,  # Legacy filter
        send_stop_frames: bool = False,  # Send explicit stop frames
        **kwargs,
    ):
        super().__init__(**kwargs)
        self._aggregate_sentences = aggregate_sentences
        self._text_filters = text_filters
        self._current_sentence = ""
```

**Text Filtering for Common Issues:**
```python
# Filter patterns for common text issues
class TextFilters:
    @staticmethod
    def remove_markdown(text: str) -> str:
        """Remove markdown formatting."""
        # Remove **bold**, *italic*, `code`, etc.
        return re.sub(r'[\*`_~]', '', text)

    @staticmethod
    def expand_abbreviations(text: str) -> str:
        """Expand common abbreviations for better TTS."""
        expansions = {
            "AI": "A.I.",
            "API": "A.P.I.",
            "ML": "M.L.",
            # ... more expansions
        }
        for abbr, expansion in expansions.items():
            text = re.sub(rf'\b{abbr}\b', expansion, text)
        return text
```

**Implementation for Bud Waav:**
```rust
pub struct TextProcessor {
    aggregate_sentences: bool,
    current_sentence: String,
    filters: Vec<Box<dyn TextFilter>>,
}

pub trait TextFilter: Send + Sync {
    fn filter(&self, text: &str) -> String;
}

pub struct MarkdownRemover;
impl TextFilter for MarkdownRemover {
    fn filter(&self, text: &str) -> String {
        // Remove markdown: **bold**, *italic*, `code`
        let re = Regex::new(r"[\*`_~]").unwrap();
        re.replace_all(text, "").to_string()
    }
}

pub struct AbbreviationExpander {
    expansions: HashMap<&'static str, &'static str>,
}

impl TextFilter for AbbreviationExpander {
    fn filter(&self, text: &str) -> String {
        let mut result = text.to_string();
        for (abbr, expansion) in &self.expansions {
            let re = Regex::new(&format!(r"\b{}\b", abbr)).unwrap();
            result = re.replace_all(&result, *expansion).to_string();
        }
        result
    }
}
```

### 7.4 STT Muting and Audio Passthrough Pattern

Pipecat's STTService implements muting and passthrough:

```python
class STTService(AIService):
    def __init__(
        self,
        audio_passthrough=True,  # Pass audio frames downstream
        sample_rate: Optional[int] = None,
        **kwargs,
    ):
        self._audio_passthrough = audio_passthrough
        self._muted: bool = False
        self._user_id: str = ""

    @property
    def is_muted(self) -> bool:
        return self._muted

    async def process_frame(self, frame: Frame, direction: FrameDirection):
        if isinstance(frame, AudioRawFrame):
            await self.process_audio_frame(frame, direction)
            if self._audio_passthrough:
                await self.push_frame(frame, direction)  # Pass through
        elif isinstance(frame, STTMuteFrame):
            self._muted = frame.mute
            logger.debug(f"STT service {'muted' if frame.mute else 'unmuted'}")
```

**Implementation for Bud Waav:**
```rust
pub struct STTStreamConfig {
    pub audio_passthrough: bool,
    pub muted: bool,
    pub user_id: Option<String>,
}

impl BaseSTT {
    pub async fn process_audio_with_passthrough(
        &mut self,
        audio: &AudioRawFrame,
        downstream_tx: Option<&Sender<AudioRawFrame>>,
    ) -> Result<(), STTError> {
        if self.config.muted {
            return Ok(());  // Skip processing when muted
        }

        // Track user_id if present
        if let Some(user_id) = &audio.user_id {
            self.config.user_id = Some(user_id.clone());
        }

        // Process audio for transcription
        self.send_audio(audio.data.clone()).await?;

        // Pass through if enabled
        if self.config.audio_passthrough {
            if let Some(tx) = downstream_tx {
                tx.send(audio.clone()).await?;
            }
        }

        Ok(())
    }

    pub fn set_muted(&mut self, muted: bool) {
        self.config.muted = muted;
        tracing::debug!(muted = muted, "STT mute state changed");
    }
}
```

### 7.5 Segmented STT with VAD Buffer Pattern

Pipecat's SegmentedSTTService buffers audio based on VAD events:

```python
class SegmentedSTTService(STTService):
    """STT service that processes speech in segments using VAD events."""

    def __init__(self, *, sample_rate: Optional[int] = None, **kwargs):
        super().__init__(sample_rate=sample_rate, **kwargs)
        self._audio_buffer = bytearray()
        self._audio_buffer_size_1s = 0  # 1 second of audio
        self._user_speaking = False

    async def start(self, frame: StartFrame):
        await super().start(frame)
        # Calculate 1 second of audio at sample rate (16-bit = 2 bytes/sample)
        self._audio_buffer_size_1s = self.sample_rate * 2

    async def process_audio_frame(self, frame: AudioRawFrame, direction: FrameDirection):
        # Keep growing buffer while speaking
        self._audio_buffer += frame.audio

        # If not speaking, keep only 1 second (for VAD delay compensation)
        if not self._user_speaking and len(self._audio_buffer) > self._audio_buffer_size_1s:
            discarded = len(self._audio_buffer) - self._audio_buffer_size_1s
            self._audio_buffer = self._audio_buffer[discarded:]

    async def _handle_user_stopped_speaking(self, frame: VADUserStoppedSpeakingFrame):
        self._user_speaking = False

        # Create WAV from buffer and transcribe
        content = io.BytesIO()
        wav = wave.open(content, "wb")
        wav.setsampwidth(2)  # 16-bit
        wav.setnchannels(1)
        wav.setframerate(self.sample_rate)
        wav.writeframes(self._audio_buffer)
        wav.close()
        content.seek(0)

        await self.process_generator(self.run_stt(content.read()))
        self._audio_buffer.clear()
```

**Why this matters:**
- Handles REST-based STT providers (OpenAI Whisper, Groq)
- Compensates for VAD detection delay
- Ensures complete utterances are transcribed

**Implementation for Bud Waav:**
```rust
pub struct SegmentedSTTBuffer {
    buffer: Vec<u8>,
    sample_rate: u32,
    buffer_size_1s: usize,
    user_speaking: bool,
}

impl SegmentedSTTBuffer {
    pub fn new(sample_rate: u32) -> Self {
        let buffer_size_1s = (sample_rate * 2) as usize; // 16-bit = 2 bytes/sample
        Self {
            buffer: Vec::with_capacity(buffer_size_1s * 30), // 30 seconds max
            sample_rate,
            buffer_size_1s,
            user_speaking: false,
        }
    }

    pub fn push_audio(&mut self, audio: &[u8]) {
        self.buffer.extend_from_slice(audio);

        // Trim to 1 second when not speaking (VAD delay compensation)
        if !self.user_speaking && self.buffer.len() > self.buffer_size_1s {
            let to_discard = self.buffer.len() - self.buffer_size_1s;
            self.buffer.drain(..to_discard);
        }
    }

    pub fn on_speech_start(&mut self) {
        self.user_speaking = true;
    }

    pub fn on_speech_end(&mut self) -> Vec<u8> {
        self.user_speaking = false;

        // Create WAV format
        let wav_data = self.create_wav();
        self.buffer.clear();
        wav_data
    }

    fn create_wav(&self) -> Vec<u8> {
        let mut wav = Vec::with_capacity(44 + self.buffer.len());

        // WAV header
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&((36 + self.buffer.len()) as u32).to_le_bytes());
        wav.extend_from_slice(b"WAVE");
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes()); // Format chunk size
        wav.extend_from_slice(&1u16.to_le_bytes());  // PCM
        wav.extend_from_slice(&1u16.to_le_bytes());  // Mono
        wav.extend_from_slice(&self.sample_rate.to_le_bytes());
        wav.extend_from_slice(&(self.sample_rate * 2).to_le_bytes()); // Byte rate
        wav.extend_from_slice(&2u16.to_le_bytes());  // Block align
        wav.extend_from_slice(&16u16.to_le_bytes()); // Bits per sample
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(self.buffer.len() as u32).to_le_bytes());
        wav.extend_from_slice(&self.buffer);

        wav
    }
}
```

### 7.6 Event Handler Registration Pattern

Pipecat uses a consistent event handler pattern across all services:

```python
class STTService(AIService):
    def __init__(self, **kwargs):
        super().__init__(**kwargs)
        self._register_event_handler("on_connected")
        self._register_event_handler("on_disconnected")
        self._register_event_handler("on_connection_error")

    # Usage:
    @stt.event_handler("on_connected")
    async def on_connected(stt: STTService):
        logger.debug("STT connected")

    @stt.event_handler("on_connection_error")
    async def on_connection_error(stt: STTService, error: str):
        logger.error(f"STT connection error: {error}")
```

**Implementation for Bud Waav:**
```rust
pub type EventCallback<T> = Arc<dyn Fn(T) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

pub struct EventEmitter<T> {
    handlers: HashMap<String, Vec<EventCallback<T>>>,
}

impl<T: Clone + Send + 'static> EventEmitter<T> {
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
        }
    }

    pub fn register_handler(&mut self, event: &str, handler: EventCallback<T>) {
        self.handlers.entry(event.to_string()).or_default().push(handler);
    }

    pub async fn emit(&self, event: &str, data: T) {
        if let Some(handlers) = self.handlers.get(event) {
            for handler in handlers {
                handler(data.clone()).await;
            }
        }
    }
}

// Usage in STT:
impl DeepgramSTT {
    pub fn new(config: STTConfig) -> Self {
        let mut events = EventEmitter::new();
        // Register default handlers
        events.register_handler("on_connected", Arc::new(|_| {
            Box::pin(async { tracing::info!("STT connected") })
        }));

        Self { events, ..Default::default() }
    }

    pub fn on_connected(&mut self, handler: EventCallback<()>) {
        self.events.register_handler("on_connected", handler);
    }
}
```

---

## Part 8: Bud Waav vs Pipecat - Comparative Analysis

### 8.1 Provider Coverage Comparison

| Category | Bud Waav | Pipecat | Notes |
|----------|----------|---------|-------|
| **STT Providers** | 30+ | 15 | Bud Waav has more Asian/regional providers |
| **TTS Providers** | 60+ | 20 | Bud Waav has more regional coverage |
| **Regional Coverage** | Global (India, China, Russia, SE Asia, Korea, Japan) | US/EU focused | Bud Waav stronger in Asia-Pacific |
| **WebSocket STT** | All major | All major | Parity |
| **gRPC STT** | Google, Tinkoff, Gnani | Google only | Bud Waav stronger |
| **REST STT** | OpenAI, Groq, FPT, Viettel | OpenAI, Groq | Bud Waav has more REST providers |

### 8.2 Architecture Comparison

| Feature | Bud Waav | Pipecat | Gap |
|---------|----------|---------|-----|
| **Core Language** | Rust (async/performance) | Python (asyncio) | Bud Waav faster |
| **Provider Abstraction** | `BaseSTT`/`BaseTTS` traits | `STTService`/`TTSService` classes | Similar |
| **Frame Pipeline** | Direct callbacks | Full frame-based | Pipecat more flexible |
| **Auto-Reconnect** | Basic | Exponential backoff | Pipecat more robust |
| **Metrics** | Partial | Full TTFB/processing | Pipecat better |
| **VAD** | Binary detection | State machine | Pipecat better |
| **Interruption** | Manual | Frame-based | Pipecat better |

### 8.3 Bud Waav Strengths to Preserve

1. **Performance**: Rust's zero-cost abstractions, no GIL
2. **Provider Coverage**: 90+ providers vs Pipecat's 35
3. **Regional Support**: Chinese, Russian, Indian, SE Asian providers
4. **Memory Safety**: Rust guarantees, no runtime errors
5. **Plugin System**: Extensible provider registration
6. **Emotion Support**: Built-in EmotionConfig for expressive TTS

### 8.4 Areas to Improve from Pipecat

1. **Resilience**: Add auto-reconnect with exponential backoff
2. **Metrics**: Standardize TTFB and processing time metrics
3. **VAD**: Implement state machine with debouncing
4. **Lifecycle**: Add proper start/stop/cancel semantics
5. **Interruption**: Add frame-based interruption handling
6. **Word Timing**: Extract word timestamps from providers that support it

---

## Part 9: Extended Implementation Recommendations

### 9.1 Priority 0 (Critical) - Must Implement

1. **WebSocket Auto-Reconnect** - Foundation for production reliability
2. **Deepgram Finalize()** - Improves turn-taking significantly
3. **Basic TTFB Metrics** - Essential for monitoring

### 9.2 Priority 1 (High) - Should Implement

1. **VAD State Machine** - Better UX, fewer false positives
2. **Interruption Framework** - Clean handling of user interrupts
3. **Event Emitter Pattern** - Consistent callback handling
4. **Text Filters** - Better TTS quality

### 9.3 Priority 2 (Medium) - Consider Implementing

1. **Full Frame Pipeline** - More flexible data flow
2. **Audio Context Management** - Better tracking
3. **Segmented STT Buffer** - Support REST-based providers
4. **Service Lifecycle Trait** - Cleaner resource management

### 9.4 Priority 3 (Low) - Nice to Have

1. **Word Timestamps** - Lip sync, karaoke mode
2. **Pronunciation Dictionaries** - Domain-specific TTS
3. **Full OpenAI Realtime Events** - Advanced features

---

## Part 10: Estimated Implementation Effort

| Task | Effort | Risk | Dependencies |
|------|--------|------|--------------|
| WebSocket Auto-Reconnect | 3 days | Low | None |
| Deepgram Finalize | 1 day | Low | None |
| TTFB Metrics | 2 days | Low | None |
| VAD State Machine | 4 days | Medium | None |
| Event Emitter | 2 days | Low | None |
| Text Filters | 2 days | Low | None |
| Interruption Framework | 5 days | Medium | VAD State Machine |
| Frame Pipeline | 10 days | High | Event Emitter |
| Audio Context | 3 days | Low | Frame Pipeline |
| Segmented STT Buffer | 3 days | Low | VAD State Machine |
| Service Lifecycle | 5 days | Medium | Frame Pipeline |
| Word Timestamps | 5 days | Medium | None |

**Total Estimated Effort**: ~45 developer-days (9 weeks)

---

## Conclusion

Pipecat provides a mature, battle-tested reference for voice AI pipelines. By implementing the patterns identified in this analysis, Bud Waav Gateway can achieve:

1. **Higher reliability** through automatic reconnection and proper error handling
2. **Better observability** through standardized metrics and frame tracing
3. **Improved UX** through proper VAD state management and word timestamps
4. **Cleaner architecture** through frame-based pipeline and service lifecycle

The implementation plan spans 10 weeks and prioritizes reliability (P0) over features (P2/P3).

**Key Takeaways:**
- Bud Waav has **superior provider coverage** (90+ vs 35) - preserve this strength
- Pipecat has **better resilience patterns** - adopt these
- Focus on P0/P1 items for immediate production impact
- Frame pipeline (P2) provides long-term architectural benefits but is a larger undertaking

---

*Document last updated: January 2026*
*Based on Pipecat commit: latest main branch*
*Analyzed by: Claude Code*
