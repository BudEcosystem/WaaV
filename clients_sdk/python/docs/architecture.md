# Python SDK Architecture

## Overview

The `bud_waav` Python SDK provides an async-first interface to the WaaV AI Gateway. It supports real-time streaming via WebSocket and batch operations via REST, with built-in metrics, reconnection, and event-driven architecture.

## Directory Structure

```
bud_waav/
    __init__.py           # Public API exports
    client.py             # BudClient - main entry point + lifecycle management
    types.py              # All type definitions (STTConfig, TTSConfig, etc.)
    errors.py             # Exception hierarchy
    rest/
        __init__.py
        client.py         # RestClient - async HTTP client (httpx)
    ws/
        __init__.py
        session.py        # WebSocketSession - core WS session with reconnect
        queue.py          # MessageQueue - buffered queue with backpressure
    pipelines/
        __init__.py
        stt.py            # BudSTT pipeline + STTSession
        tts.py            # BudTTS pipeline + TTSSession
        talk.py           # BudTalk pipeline + TalkSession
        transcribe.py     # BudTranscribe pipeline + TranscribeSession
        realtime.py       # BudRealtime pipeline (OpenAI Realtime, Hume EVI)
    audio/
        __init__.py
        processor.py      # AudioProcessor - resampling, format conversion
        vad.py            # VoiceActivityDetector - energy-based VAD
    metrics/
        __init__.py
        slo.py            # SLOTracker - service level objective monitoring
tests/
    unit/                 # Unit tests (mocked, no network)
    api/                  # API tests (mock HTTP/WS)
    e2e/                  # End-to-end tests (require running gateway)
```

## Component Architecture

```
BudClient
    |
    |-- .stt          -> BudSTT        -> STTSession      -> WebSocketSession
    |-- .tts          -> BudTTS        -> TTSSession       -> WebSocketSession
    |-- .talk         -> BudTalk       -> TalkSession      -> WebSocketSession
    |-- .transcribe   -> BudTranscribe -> TranscribeSession -> WebSocketSession
    |-- .rest         -> RestClient    (httpx.AsyncClient)
    |-- .create_realtime() -> BudRealtime (standalone WS to /realtime)
    |
    |-- Lifecycle: register_pipeline(), deregister_pipeline(), disconnect_all()
    |-- Active pipeline tracking via WeakSet (auto-cleanup on GC)
```

### BudClient (`client.py`)

Main entry point. Constructs URLs, initializes REST client and pipeline factories.

- Converts HTTP URLs to WS URLs automatically (`http://` -> `ws://`)
- Passes API key to all sub-components
- Provides convenience wrappers for REST operations (health, voices, speak, LiveKit, SIP)
- **Lifecycle management**: Tracks active pipeline sessions via `WeakSet`, supports `register_pipeline()`, `deregister_pipeline()`, `get_active_pipeline_count()`, and `disconnect_all()`
- `close()` calls `disconnect_all()` to gracefully close all sessions before closing the REST client

### Pipelines (`pipelines/`)

Each pipeline is a factory + session pair:

- **Factory** (e.g., `BudSTT`): Creates configured sessions via `.create()` method
- **Session** (e.g., `STTSession`): Wraps `WebSocketSession` with pipeline-specific event handling

Pipeline sessions support:
- Event-based API: `session.on("transcript", callback)`
- Async context manager: `async with session: ...`
- Metrics: `session.get_metrics()`

### WebSocketSession (`ws/session.py`)

Core WebSocket session that handles:

1. **Connection lifecycle**: connect, disconnect, reconnect
2. **Config protocol**: Sends `{"type": "config", "audio": true, "stt_config": {...}, "tts_config": {...}}` and waits for `{"type": "ready", "stream_id": "..."}`
3. **Message routing**: Dispatches incoming messages to registered event handlers
4. **Binary audio**: Sends raw PCM audio bytes, receives TTS audio as binary frames
5. **Metrics collection**: TTFT, TTFB, E2E latency, connection timing
6. **Reconnection**: Exponential backoff with jitter via `ReconnectConfig`

**Critical constraint**: The gateway requires BOTH `stt_config` AND `tts_config` in the config message when `audio=true`. The session provides minimal defaults when only one is specified.

### RestClient (`rest/client.py`)

Async HTTP client using `httpx.AsyncClient`:

- Auto-manages client lifecycle (lazy init, close)
- Bearer token authentication
- Endpoint methods: `health()`, `list_voices()`, `speak()`, `create_livekit_token()`, etc.
- `delete()` supports JSON body for DELETE requests (used by LiveKit/SIP endpoints)

**Key format**: The `/speak` endpoint expects `{"text": "...", "tts_config": {"provider": "...", "model": "..."}}` (nested, not flat).

**Verified REST endpoint mappings** (cross-referenced with gateway `routes/api.rs`):
| SDK Method | HTTP | Gateway Path | Body Format |
|---|---|---|---|
| `health()` | GET | `/health` | - |
| `list_voices()` | GET | `/voices` | Returns `{"deepgram": [...]}` |
| `speak()` | POST | `/speak` | `{"text":"...","tts_config":{...}}` |
| `download_recording()` | GET | `/recording/{stream_id}` | Returns `audio/ogg` |
| `delete_sip_hooks()` | DELETE | `/sip/hooks` | `{"hosts": [...]}` |
| `remove_livekit_participant()` | DELETE | `/livekit/participant` | `{"room_name":"...","participant_identity":"..."}` |
| `mute_livekit_participant()` | POST | `/livekit/participant/mute` | `{"room_name":"...","participant_identity":"...","track_sid":"...","muted":true}` |

**Not yet implemented in gateway**: `get_cloned_voice`, `delete_cloned_voice`, `get_recording`, `list_recordings`, `delete_recording`, `get_metrics` (these methods have warning docstrings).

### Types (`types.py`)

Central type definitions:

- `STTConfig`, `TTSConfig`: Pipeline configuration dataclasses
- `STTResult`, `TranscriptEvent`, `AudioEvent`: Event types
- `AudioFeatures`, `TurnDetectionConfig`, `NoiseFilterConfig`: Feature configs
- `DAGConfig`, `DAGDefinition`, `DAGNode`, `DAGEdge`: DAG routing types
- `Emotion`, `DeliveryStyle`, `EmotionIntensityLevel`: Emotion system enums
- `RealtimeConfig`, `RealtimeProvider`: Realtime pipeline types

### Voice Activity Detection (`audio/vad.py`)

Energy-based VAD with state machine for speech detection:

- **`VoiceActivityDetector`**: Core detector with configurable `VADConfig`
- **State machine**: SILENCE → UNCERTAIN → SPEECH (and reverse)
  - SILENCE → UNCERTAIN: Energy crosses threshold
  - UNCERTAIN → SPEECH: Energy sustained for `min_speech_duration_ms`
  - SPEECH → UNCERTAIN: Energy drops below threshold
  - UNCERTAIN → SILENCE: Energy low for `min_silence_duration_ms`
- **Pre-speech buffer**: Circular buffer (`deque`) captures audio before speech confirmation
- **Frame-counted timing**: Uses `frames_in_state * frame_duration_ms` (not wall-clock) for deterministic behavior
- **Event callbacks**: `on_speech_start()`, `on_speech_end(pre_speech_bytes)`, `on_frame(VADFrame)`
- **`_calculate_energy_db()`**: RMS energy in dB, handles int16 and float32 formats

### Message Queue (`ws/queue.py`)

Buffered message queue with backpressure for WebSocket reliability:

- **`MessageQueue`**: FIFO queue supporting text (`str`) and binary (`bytes`) messages
- **Drop policies**: `drop_oldest=True` (default) drops oldest message when full; `drop_oldest=False` rejects incoming
- **Message expiration**: Automatic cleanup of messages older than `max_age_ms`
- **`QueueConfig`**: `max_size` (256), `max_age_ms` (60s), `drop_oldest` (true)
- **`QueueStats`**: Monitoring with size, max_size, dropped_count, oldest_age_ms
- Methods: `enqueue()`, `dequeue()`, `peek()`, `drain()`, `clear()`, `get_stats()`, `reset_stats()`

### SLO Tracker (`metrics/slo.py`)

Service Level Objective monitoring for pipeline metrics:

- **`SLOTracker`**: Manages SLO definitions, checks metrics, tracks violations
- **`SLODefinition`**: Name, metric key, threshold, comparison operator, optional percentile
- **`SLOComparison`**: LT, LTE, GT, GTE, EQ comparison operators
- **Percentile support**: Extracts p50/p95/p99 from `PercentileStats` objects or dicts
- **Violation history**: Limited to 100 per SLO via `deque`
- **Health calculation**: `pass_count / check_count` (1.0 when no checks performed)
- Methods: `add_slo()`, `remove_slo()`, `check(metrics)`, `get_health()`, `get_violations()`, `reset()`

### Errors (`errors.py`)

Exception hierarchy:

```
BudError
    ConnectionError
    TimeoutError
    ReconnectError
    APIError
    STTError
    TTSError
    AuthenticationError
    ProviderError
    DAGError
    RealtimeError
```

## Data Flow

### STT Pipeline

```
User Audio (bytes)
    -> session.send_audio(chunk)
    -> WebSocket binary frame
    -> Gateway
    -> STT Provider (Deepgram nova-3)
    -> Gateway
    -> {"type": "stt_result", "transcript": "...", "is_final": true}
    -> WebSocket text frame
    -> session._on_message()
    -> "transcript" event callback
```

### TTS Pipeline

```
User Text (str)
    -> session.speak("Hello")
    -> {"type": "speak", "text": "Hello", "flush": true}
    -> WebSocket text frame
    -> Gateway
    -> TTS Provider (Deepgram aura-asteria-en)
    -> Gateway
    -> Binary audio frames (PCM16)
    -> WebSocket binary frames
    -> session._on_binary()
    -> "audio" event callback
    (finally)
    -> {"type": "tts_playback_complete"}
    -> "playback_complete" event callback
```

### REST Speak (One-shot)

```
client.rest.speak("Hello", provider="deepgram")
    -> POST /speak {"text": "Hello", "tts_config": {"provider": "deepgram", ...}}
    -> Gateway synthesizes full audio
    -> Returns audio/pcm bytes
```

## Protocol Details

### WebSocket Config Message

```json
{
    "type": "config",
    "audio": true,
    "stream_id": "optional-custom-id",
    "stt_config": {
        "provider": "deepgram",
        "language": "en-US",
        "sample_rate": 16000,
        "channels": 1,
        "punctuation": true,
        "encoding": "linear16",
        "model": "nova-3",
        "api_key": "optional-per-request-key"
    },
    "tts_config": {
        "provider": "deepgram",
        "model": "aura-asteria-en",
        "voice_id": "aura-asteria-en",
        "sample_rate": 24000,
        "audio_format": "linear16",
        "api_key": "optional-per-request-key"
    },
    "audio_features": {
        "turn_detection": { "enabled": true, "threshold": 0.5 },
        "noise_filtering": { "enabled": true, "strength": "medium" },
        "vad": { "enabled": true }
    },
    "dag_config": {
        "template": "voice-assistant"
    }
}
```

### Gateway Ready Response

```json
{
    "type": "ready",
    "stream_id": "uuid-or-custom-id",
    "room_name": "optional-livekit-room",
    "participant_token": "optional-livekit-jwt"
}
```

### STT Result Message

```json
{
    "type": "stt_result",
    "transcript": "hello world",
    "is_final": true,
    "is_speech_final": true,
    "confidence": 0.95,
    "words": [...],
    "speech_started": 0.5,
    "duration": 1.2,
    "language": "en"
}
```

## Testing Strategy

- **Unit tests** (`tests/unit/`): Test all types, configs, sessions, pipelines, VAD, SLO, message queue, and lifecycle with mocked WebSocket/HTTP
- **API tests** (`tests/api/`): Test REST and WS clients against mock servers
- **E2E tests** (`tests/e2e/`): Test against running gateway with real audio and real Deepgram API calls
- **Audio fixtures**: WAV files at `gateway/tests/live_testing/audio/` (CMU Arctic dataset)

**Test count**: 275 total (254 unit/API + 21 e2e)

Key test files:
| File | Tests | Coverage |
|------|-------|----------|
| `test_vad.py` | 21 | Energy calculation, state transitions, callbacks, pre-speech buffer |
| `test_slo.py` | 23 | All comparisons, percentile stats, health, violations, history limits |
| `test_message_queue.py` | 24 | FIFO, drop policies, expiration, stats, large queues |
| `test_lifecycle.py` | 11 | Register, deregister, disconnect_all, weak ref GC cleanup |
| `test_gap_fixes.py` | - | REST endpoint signature fixes, BudClient wrapper methods |

Run tests:
```bash
# Unit + API tests (no network required)
pytest tests/unit/ tests/api/ -q

# E2E tests (requires gateway on localhost:3001)
pytest tests/e2e/ -v -s

# All tests
pytest tests/ -v
```
