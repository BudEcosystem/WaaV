# Cartesia APIs Integration Guide for Bud Waav Gateway

**Date:** January 17, 2026
**For:** Bud Waav AI Gateway Integration

---

## Quick Start: API Selection

### When to Use Ink-Whisper STT
- Real-time speech-to-text in voice agents
- Need word-level timestamps for UI synchronization
- Processing audio in 100ms chunks for latency optimization
- Requires speaker diarization or multi-language support
- Domain-specific terminology transcription

### When to Use Sonic-3 TTS
- Real-time voice synthesis for interactive agents
- Need emotional expression or personality
- Require voice cloning from audio samples
- Multiple language output with same API
- Speed/volume adjustments mid-conversation

---

## Implementation Priority

### Phase 1: STT Integration (Ink-Whisper)
**Objective:** Enable real-time speech transcription with word-level timestamps

#### Connection Setup
```python
# WebSocket connection parameters
connection_params = {
    "model": "ink-whisper",
    "language": "en",
    "encoding": "pcm_s16le",
    "sample_rate": 16000,
    "min_volume": 0.1,
    "max_silence_duration_secs": 0.4,
    "api_key": "<CARTESIA_API_KEY>"
}

# Endpoint
endpoint = "wss://api.cartesia.ai/stt/websocket"
```

#### Audio Streaming Strategy
1. **Chunk Size:** 100ms intervals at 16000 Hz = 1600 samples per chunk
2. **Buffer Management:** Use ring buffers for zero-copy audio passing
3. **VAD Configuration:**
   - `min_volume=0.1`: Aggressive (low threshold, captures quiet speech)
   - `min_volume=0.15`: Moderate (balanced)
   - `max_silence_duration_secs=0.4`: Short gaps don't trigger endpointing
   - `max_silence_duration_secs=2.0`: Allow longer pauses (for thinking/hesitation)

#### Expected Response Format
```json
{
  "type": "transcript",
  "words": [
    {
      "text": "hello",
      "start": 0.12,
      "end": 0.45
    },
    {
      "text": "world",
      "start": 0.46,
      "end": 0.78
    }
  ],
  "is_final": false
}
```

#### Real-time Implementation Notes
- Process `is_final=false` results for low-latency UI updates
- Consolidate results when `is_final=true` arrives
- Timestamps enable synchronized text highlighting in UI
- Session auto-disconnects after 3 minutes; implement reconnection logic

---

### Phase 2: TTS Integration (Sonic-3)
**Objective:** Enable real-time voice synthesis with emotional expression

#### Connection Setup
```python
# WebSocket connection parameters
connection_params = {
    "cartesia_version": "2025-04-16",
    "api_key": "<CARTESIA_API_KEY>"
}

# Generation request
generation_request = {
    "model_id": "sonic-3",
    "transcript": "Hello, this is a test.",
    "language": "en",
    "context_id": "unique-request-id-123",
    "voice": {
        "mode": "id",
        "id": "f786b574-daa5-4673-aa0c-cbe3e8534c02"  # Katie (agent voice)
    },
    "output_format": {
        "container": "raw",
        "encoding": "pcm_s16le",
        "sample_rate": 16000
    },
    "generation_config": {
        "speed": 1.0,
        "emotion": "professional"
    },
    "add_timestamps": True
}

# Endpoint
endpoint = "wss://api.cartesia.ai/tts/websocket"
```

#### Voice Selection Strategy

**For Agent Personalities:**
- **Katie** (`f786b574-daa5-4673-aa0c-cbe3e8534c02`): Professional, neutral, stable
- **Kiefer** (`228fca29-3a0a-435c-8728-5cb483251068`): Professional, neutral, stable
- **Use emotion=`professional` or `neutral`** for consistent agent behavior

**For Expressive/Entertainment:**
- **Tessa** (`6ccbfb76-1fc6-48f7-b71d-91ac6298247b`): Emotive, expressive
- **Kyle** (`c961b81c-a935-4c17-bfb3-ba2239de8c2f`): Emotive, expressive
- **Use emotion tags:** `excited`, `calm`, `sad`, `friendly`, etc.

**For Custom Voices:**
- Clone from 3+ seconds of audio: `client.voices.clone(audio_bytes)`
- Returns new voice ID for use in subsequent requests

#### Expected Response Format
```json
{
  "type": "audio_chunk",
  "audio": "base64_encoded_pcm_data",
  "step_time": 0.023,  // Time in seconds
  "status": 206  // Partial content indicator
}
```

#### Audio Output Handling
- First chunk arrives in ~135ms (Sonic-3 latency)
- Subsequent chunks stream continuously
- Decode base64 chunks to PCM for playback
- Implement audio buffer for smooth playback (50-200ms buffer recommended)

#### Control Parameters for Real-Time
```python
# Speed adjustment for varied pacing
speed_configs = {
    "slow": 0.85,    # 15% slower
    "normal": 1.0,
    "fast": 1.15     # 15% faster
}

# Emotion intensity
emotion_map = {
    "professional": 0.3,
    "friendly": 0.6,
    "excited": 0.9
}

# Volume control
volume_levels = {
    "quiet": -6,
    "normal": 0,
    "loud": 6
}
```

#### SSML Integration Examples
```xml
<!-- Combined emotion and speed -->
<speed ratio="1.1">
  <emotion value="excited">
    This is exciting news!
  </emotion>
</speed>

<!-- Automatic laughter insertion -->
I find that [laughter] quite amusing.

<!-- Volume control -->
<volume level="loud">IMPORTANT ANNOUNCEMENT</volume>
```

---

## Architecture Integration Points

### 1. Gateway ↔ STT/TTS Connection Management

**Responsibility: Rust Gateway**
```rust
// Pseudo-code
struct CartesiaConnection {
    stt_websocket: WebSocket,
    tts_websocket: WebSocket,
    auth_token: String,
    rate_limiter: RateLimiter,
}

impl CartesiaConnection {
    fn stream_audio_to_stt(&mut self, chunk: &[u8]) {
        // Non-blocking write to STT WebSocket
        self.stt_websocket.send_binary(chunk)?;
    }

    fn receive_transcription(&mut self) -> Result<Transcript> {
        // Parse incoming transcript messages
    }

    fn stream_transcript_to_tts(&mut self, text: &str) -> Result<()> {
        // Send text to TTS, receive audio
    }

    fn stream_audio_chunk(&mut self) -> Result<Vec<u8>> {
        // Receive audio chunk from TTS
    }
}
```

**Responsibility: Python Inference Engine**
```python
# Async processing of audio
async def process_audio_stream(self, audio_queue):
    async for chunk in audio_queue:
        # Send to Cartesia STT
        transcript = await self.cartesia_stt.transcribe(chunk)

        # Process with LLM
        response = await self.llm.generate(transcript)

        # Send to Cartesia TTS
        async for audio_chunk in self.cartesia_tts.synthesize(response):
            yield audio_chunk
```

### 2. Buffer Management (Zero-Copy Optimization)

**For STT Audio Input:**
```python
# Ring buffer for incoming audio
class AudioRingBuffer:
    def __init__(self, capacity: int = 16000 * 10):  # 10 seconds at 16kHz
        self.buffer = numpy.zeros(capacity, dtype=numpy.float32)
        self.write_pos = 0
        self.read_pos = 0

    def push_chunk(self, chunk: numpy.ndarray) -> numpy.ndarray:
        # Return view for STT without copying
        return numpy.frombuffer(self.buffer, dtype=numpy.float32)

    def get_read_view(self, frames: int) -> numpy.ndarray:
        # Zero-copy read view
        return self.buffer[self.read_pos:self.read_pos + frames]
```

**For TTS Audio Output:**
```python
# Lock-free queue for audio output
from crossbeam_channel import unbounded

audio_output_queue = unbounded()

# Producer (TTS thread)
for audio_chunk in tts_stream:
    audio_output_queue.send(audio_chunk)  # Wait-free

# Consumer (playback thread)
while True:
    chunk = audio_output_queue.recv()
    speaker.play(chunk)
```

### 3. Error Handling & Resilience

**STT Error Recovery:**
```python
class SttResilience:
    def __init__(self):
        self.reconnect_attempts = 3
        self.backoff_ms = [100, 500, 2000]

    async def reconnect_with_backoff(self):
        for attempt in range(self.reconnect_attempts):
            try:
                await self.connect_stt()
                return
            except Exception as e:
                await asyncio.sleep(self.backoff_ms[attempt] / 1000)
                if attempt == self.reconnect_attempts - 1:
                    raise
```

**TTS Error Recovery:**
```python
class TtsResilience:
    def __init__(self):
        self.fallback_voices = [
            "f786b574-daa5-4673-aa0c-cbe3e8534c02",  # Katie (primary)
            "228fca29-3a0a-435c-8728-5cb483251068",  # Kiefer (fallback)
        ]

    async def synthesize_with_fallback(self, text: str):
        for voice_id in self.fallback_voices:
            try:
                async for chunk in self.tts.synthesize(text, voice_id):
                    yield chunk
                return
            except Exception as e:
                logger.warning(f"Voice {voice_id} failed: {e}")
                continue
        raise Exception("All TTS voices exhausted")
```

### 4. Performance Monitoring

**Latency Tracking:**
```python
# Measure end-to-end latency
class LatencyTracker:
    def track_stt_latency(self, chunk_start_time):
        # Time from audio capture to first transcript
        self.metrics['stt_first_result_latency'].record(
            time.time() - chunk_start_time
        )

    def track_tts_latency(self, request_time):
        # Time from TTS request to first audio chunk
        self.metrics['tts_first_chunk_latency'].record(
            time.time() - request_time
        )

    def track_end_to_end(self, user_speech_start, audio_playback_start):
        # Full pipeline latency
        self.metrics['e2e_latency'].record(
            audio_playback_start - user_speech_start
        )
```

**Expected Latencies (p99):**
- STT first result: < 100ms
- TTS first chunk: ~135ms (model latency)
- End-to-end turn: < 500ms (with LLM)

---

## API Rate Limiting & Quotas

### Cartesia Pricing Model
- **1 credit = 1 second of audio**
- Charged per second of streamed audio
- Applies to both STT and TTS

### Recommended Quota Management
```python
class QuotaManager:
    def __init__(self, credits_per_minute: int = 600):
        self.credit_budget = credits_per_minute
        self.window_start = time.time()
        self.credits_used = 0

    def check_quota(self, duration_seconds: float) -> bool:
        if time.time() - self.window_start > 60:
            self.window_start = time.time()
            self.credits_used = 0

        if self.credits_used + duration_seconds > self.credit_budget:
            return False

        self.credits_used += duration_seconds
        return True
```

### Multi-User Fairness
```python
# Per-user credit limits
USER_LIMITS = {
    "premium": 10000,      # credits/month
    "standard": 1000,
    "free": 100
}

class UserQuotaManager:
    def get_available_credits(self, user_id: str) -> float:
        used = self.fetch_usage(user_id)
        limit = USER_LIMITS[self.get_tier(user_id)]
        return limit - used
```

---

## Configuration Management

### Environment Variables
```bash
# .env.production
CARTESIA_API_KEY=sk-...
CARTESIA_STT_MODEL=ink-whisper
CARTESIA_TTS_MODEL=sonic-3
CARTESIA_TTS_DEFAULT_VOICE=f786b574-daa5-4673-aa0c-cbe3e8534c02
CARTESIA_SAMPLE_RATE=16000
CARTESIA_ENCODING=pcm_s16le
CARTESIA_VAD_MIN_VOLUME=0.1
CARTESIA_VAD_MAX_SILENCE_SECS=0.4
```

### Feature Flags
```python
# Feature control for gradual rollout
FEATURES = {
    "voice_cloning": {"enabled": True, "rollout_percent": 50},
    "emotion_control": {"enabled": True, "rollout_percent": 100},
    "laughter_generation": {"enabled": False, "rollout_percent": 0},
}
```

---

## Testing Strategy

### Unit Tests
```python
def test_stt_websocket_connection():
    # Verify connection parameters
    # Mock WebSocket responses
    # Verify timestamp parsing

def test_tts_audio_generation():
    # Verify audio encoding
    # Test voice ID validation
    # Verify base64 decoding

def test_voice_selection():
    # Verify voice IDs exist
    # Test voice cloning endpoint
    # Verify emotion mapping
```

### Integration Tests
```python
async def test_end_to_end_conversation():
    # 1. Send audio to STT
    # 2. Receive transcript with timestamps
    # 3. Generate response with LLM
    # 4. Send to TTS
    # 5. Receive audio chunks
    # 6. Verify latency SLO
```

### Load Tests
```python
async def test_concurrent_users(num_users: int = 100):
    # Simulate N concurrent conversations
    # Track per-user latencies
    # Monitor resource utilization
    # Verify graceful degradation at limits
```

---

## Production Checklist

- [ ] STT connection with automatic reconnection implemented
- [ ] TTS connection with voice fallback implemented
- [ ] Audio buffer management (zero-copy optimized)
- [ ] Timestamp synchronization for UI
- [ ] Error handling and circuit breaker patterns
- [ ] Rate limiting and quota enforcement
- [ ] Monitoring and observability
- [ ] Load testing with target QPS
- [ ] Latency SLO verification (< 500ms e2e)
- [ ] Documentation for ops team
- [ ] Automated alerting for failures
- [ ] Graceful degradation strategies
- [ ] Cost monitoring and attribution
- [ ] User analytics integration
- [ ] Performance dashboards

---

## Troubleshooting

### STT Issues

**Problem:** "Session timeout after 3 minutes"
- **Solution:** Implement session refresh/reconnection logic before timeout

**Problem:** VAD triggering too aggressively
- **Solution:** Increase `min_volume` (0.15+) or `max_silence_duration_secs` (2.0+)

**Problem:** Low accuracy on domain-specific terms
- **Solution:** Pre-process text to capitalize proper nouns, use punctuation hints

### TTS Issues

**Problem:** First chunk latency spike
- **Solution:** Pre-warm TTS connection, use connection pooling

**Problem:** Voice sounds robotic
- **Solution:** Reduce emotion value (0.3-0.6), adjust speed ratio

**Problem:** Audio buffer underruns causing glitches
- **Solution:** Increase buffer size to 200ms, implement adaptive buffering

---

## References

- [Cartesia STT API](https://docs.cartesia.ai/api-reference/stt/stt)
- [Cartesia TTS API](https://docs.cartesia.ai/api-reference/tts/tts)
- [Python SDK](https://github.com/cartesia-ai/cartesia-python)
- [Sonic-3 Model Card](https://docs.cartesia.ai/build-with-cartesia/tts-models/latest)
- [Ink-Whisper Model Card](https://docs.cartesia.ai/build-with-cartesia/stt-models)

---

**End of Integration Guide**
