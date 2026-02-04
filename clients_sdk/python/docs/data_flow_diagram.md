# Data Flow Diagram

## System Overview

```
                           Python SDK (bud_waav)
    ┌─────────────────────────────────────────────────────────┐
    │                                                         │
    │  BudClient                                              │
    │    ├── BudSTT.create()  ──> STTSession                  │
    │    ├── BudTTS.create()  ──> TTSSession                  │
    │    ├── BudTalk.create() ──> TalkSession                 │
    │    ├── BudTranscribe()  ──> TranscribeSession           │
    │    ├── RestClient       ──> httpx.AsyncClient           │
    │    ├── create_realtime()──> BudRealtime                 │
    │    │                                                    │
    │    ├── Lifecycle: WeakSet<Session>                       │
    │    │   register_pipeline / deregister_pipeline           │
    │    │   disconnect_all / get_active_pipeline_count        │
    │    │                                                    │
    │    ├── VoiceActivityDetector [audio/vad.py]              │
    │    │   Audio → RMS Energy → State Machine → Callbacks   │
    │    │                                                    │
    │    ├── MessageQueue [ws/queue.py]                        │
    │    │   Backpressure + Expiration for WS reliability      │
    │    │                                                    │
    │    └── SLOTracker [metrics/slo.py]                       │
    │        Threshold checks on PercentileStats               │
    │                                                         │
    └────────────┬──────────────────────┬─────────────────────┘
                 │ WebSocket            │ HTTP REST
                 │ ws://host/ws         │ http://host/{...}
                 │                      │
    ┌────────────▼──────────────────────▼─────────────────────┐
    │                                                         │
    │                WaaV Gateway (Rust)                       │
    │                                                         │
    │  ┌─────────────┐  ┌──────────┐  ┌──────────────────┐   │
    │  │ WS Handler  │  │ REST API │  │ Realtime Handler │   │
    │  │  /ws        │  │ /health  │  │  /realtime       │   │
    │  │             │  │ /voices  │  │                   │   │
    │  │             │  │ /speak   │  │                   │   │
    │  └──────┬──────┘  └────┬─────┘  └────────┬──────────┘  │
    │         │              │                  │              │
    │  ┌──────▼──────────────▼──────────────────▼──────────┐  │
    │  │              VoiceManager                          │  │
    │  │  ┌─────────────┐  ┌─────────────┐                 │  │
    │  │  │ STT Manager │  │ TTS Manager │                 │  │
    │  │  └──────┬──────┘  └──────┬──────┘                 │  │
    │  └─────────┼────────────────┼────────────────────────┘  │
    │            │                │                            │
    │  ┌────────▼────────┐ ┌─────▼───────────┐               │
    │  │  Noise Filter   │ │  Turn Detection  │               │
    │  │  (DeepFilterNet)│ │  (text/audio)    │               │
    │  └────────┬────────┘ └─────────────────┘               │
    │           │                                             │
    └───────────┼─────────────────────────────────────────────┘
                │
    ┌───────────▼─────────────────────────────────────────────┐
    │                  Cloud Providers                         │
    │  ┌──────────┐ ┌───────────┐ ┌────────┐ ┌───────────┐   │
    │  │ Deepgram │ │ElevenLabs │ │ Google │ │  Azure    │   │
    │  │ STT+TTS  │ │ TTS+Clone │ │STT+TTS │ │ STT+TTS  │   │
    │  └──────────┘ └───────────┘ └────────┘ └───────────┘   │
    └─────────────────────────────────────────────────────────┘
```

## STT Call Chain

```
User Code
    │
    │  bud.stt.create(provider="deepgram", language="en-US", model="nova-3")
    ▼
BudSTT.create()                                [pipelines/stt.py]
    │  Creates STTSession with STTConfig
    ▼
STTSession.__aenter__()                        [pipelines/stt.py]
    │  Calls self._session.connect()
    ▼
WebSocketSession.connect()                     [ws/session.py]
    │  1. websockets.connect(ws://host/ws)
    │  2. _send_config() → {"type":"config","audio":true,"stt_config":{...},"tts_config":{...}}
    │  3. Wait for {"type":"ready","stream_id":"..."}
    │  4. Start _receive_loop() background task
    ▼
User: session.send_audio(pcm_bytes)            [pipelines/stt.py → ws/session.py]
    │  Binary frame sent via WebSocket
    ▼
Gateway: processes audio through STT provider
    │  Returns {"type":"stt_result","transcript":"...","is_final":true}
    ▼
WebSocketSession._receive_loop()               [ws/session.py]
    │  Parses JSON, dispatches to callbacks
    ▼
STTSession._on_stt_result()                    [pipelines/stt.py]
    │  Creates STTResult, emits "transcript" event
    ▼
User callback: session.on("transcript", fn)
```

## TTS Call Chain

```
User Code
    │
    │  bud.tts.create(provider="deepgram", model="aura-asteria-en")
    ▼
BudTTS.create()                                [pipelines/tts.py]
    │  Creates TTSSession with TTSConfig
    ▼
TTSSession.__aenter__()                        [pipelines/tts.py]
    │  Calls self._session.connect()
    ▼
WebSocketSession.connect()                     [ws/session.py]
    │  Same handshake as STT (config → ready)
    ▼
User: session.speak("Hello world")             [pipelines/tts.py → ws/session.py]
    │  Sends {"type":"speak","text":"Hello world","flush":true}
    ▼
Gateway: synthesizes via TTS provider
    │  Returns binary audio frames (PCM16) + {"type":"tts_playback_complete"}
    ▼
WebSocketSession._receive_loop()               [ws/session.py]
    │  Binary → "audio" event
    │  tts_playback_complete → "playback_complete" event
    ▼
User callbacks:
    session.on("audio", fn)                    # PCM audio chunks
    session.on("playback_complete", fn)        # Synthesis done
```

## REST Speak Call Chain

```
User Code
    │
    │  bud.rest.speak("Hello", provider="deepgram")
    ▼
RestClient.speak()                             [rest/client.py]
    │  POST /speak {"text":"Hello","tts_config":{"provider":"deepgram","model":"aura-asteria-en"}}
    ▼
Gateway: /speak handler
    │  Synthesizes full audio
    │  Returns Content-Type: audio/pcm
    ▼
RestClient: returns bytes
    │
    ▼
User: receives PCM audio bytes
```

## Reconnection Flow

```
WebSocket disconnection detected
    │
    ▼
WebSocketSession._receive_loop() catches exception
    │  Emits "disconnected" event
    ▼
If ReconnectConfig.enabled:
    │  delay = initial_delay_ms
    │  for attempt in range(max_retries):
    │      sleep(delay + jitter)
    │      try:
    │          websockets.connect(url)
    │          _send_config()
    │          wait for "ready"
    │          emit "reconnected" event
    │          restart _receive_loop()
    │          BREAK
    │      except:
    │          delay *= multiplier (capped at max_delay_ms)
    ▼
If all retries exhausted:
    │  emit "error" event with ReconnectError
    ▼
Session closed
```

## Metrics Collection Points

```
connect()
    ├── ws_connect_ms: time from websockets.connect() to "ready" response
    │
send_audio(chunk)
    ├── audio_bytes_sent += len(chunk)
    ├── messages_sent += 1
    │
_receive_loop() (each message)
    ├── messages_received += 1
    ├── If binary: audio_bytes_received += len(data)
    ├── If stt_result: stt_ttft recorded (first non-empty result after audio start)
    ├── If tts audio: tts_ttfb recorded (first audio chunk after speak command)
    │
get_metrics()
    └── Returns SessionMetrics with PercentileStats (p50, p95, p99, min, max, mean)
```

## Voice Activity Detection Flow

```
Audio Frame (PCM bytes)
    │
    ▼
VoiceActivityDetector.process(audio)              [audio/vad.py]
    │
    ├── _calculate_energy_db(audio)
    │       RMS = sqrt(sum(s²) / n)
    │       Normalize int16: rms / 32767
    │       dB = 20 * log10(rms)   [floor: -96 dB]
    │
    ├── Exponential smoothing
    │       smoothed = α * energy + (1-α) * prev_smoothed
    │       α = 1 - smoothing_factor
    │
    ├── State machine transition (frame-counted timing)
    │       ┌─────────┐  above threshold  ┌───────────┐  sustained ≥ min_speech_ms  ┌────────┐
    │       │ SILENCE  │ ───────────────► │ UNCERTAIN │ ──────────────────────────► │ SPEECH │
    │       └─────────┘ ◄─────────────── └───────────┘ ◄───────────────────────── └────────┘
    │         drops below               energy returns    drops below threshold
    │         before confirm                              (enters UNCERTAIN)
    │                                                     then sustained ≥ min_silence_ms
    │                                                     → SILENCE
    │
    ├── Pre-speech buffer (deque, maxlen = pre_speech_buffer_ms / frame_duration_ms)
    │       Captures audio before speech confirmation
    │
    ├── Callbacks:
    │       on_speech_start()           → When UNCERTAIN → SPEECH
    │       on_speech_end(pre_speech)   → When post-SPEECH → SILENCE (with buffered audio)
    │       on_frame(VADFrame)          → Every frame
    │
    └── Returns VADFrame(state, energy_db, smoothed_energy_db, is_speech, timestamp_ms)
```

## Message Queue Flow

```
WebSocket Send Path (with backpressure)
    │
    ▼
MessageQueue.enqueue(message)                      [ws/queue.py]
    │
    ├── Expire old messages (age > max_age_ms)
    │
    ├── If queue full (size >= max_size):
    │   ├── drop_oldest=True:  Remove oldest, enqueue new → return True
    │   └── drop_oldest=False: Reject new message → return False
    │
    └── Append (message, timestamp) → return True

MessageQueue.dequeue()
    │
    ├── Expire front messages (age > max_age_ms)
    │
    └── Pop oldest → return message or None

MessageQueue.drain() → [all messages]
MessageQueue.clear() → dropped count
MessageQueue.get_stats() → QueueStats(size, max_size, dropped_count, oldest_age_ms)
```

## SLO Tracking Flow

```
Pipeline Metrics
    │
    ▼
SLOTracker.check(metrics_dict)                     [metrics/slo.py]
    │
    ├── For each SLODefinition:
    │   ├── Lookup metric key in metrics_dict
    │   ├── If percentile specified:
    │   │   └── Extract p50/p95/p99 from PercentileStats or dict
    │   ├── Compare actual value against threshold:
    │   │   ├── LT:  violated if actual >= threshold
    │   │   ├── LTE: violated if actual > threshold
    │   │   ├── GT:  violated if actual <= threshold
    │   │   ├── GTE: violated if actual < threshold
    │   │   └── EQ:  violated if actual != threshold
    │   └── If violated: create SLOViolation, append to history (max 100)
    │
    ├── Update counters: check_count++, pass_count++ (if no violations)
    │
    └── Return list[SLOViolation]

SLOTracker.get_health() → pass_count / check_count (1.0 if no checks)
SLOTracker.get_violations(name?) → filtered violation history
```

## Lifecycle Management Flow

```
Pipeline Session Created
    │
    ▼
BudClient.register_pipeline(session)               [client.py]
    │  Adds to WeakSet (auto-removes on GC)
    ▼
BudClient.get_active_pipeline_count()
    │  Returns len(WeakSet)
    ▼
BudClient.disconnect_all()
    │  For each session in WeakSet:
    │    try session.close() or session.disconnect()
    │  Clear WeakSet
    │  Return count of closed sessions
    ▼
BudClient.close()
    │  1. disconnect_all()   → close all pipeline sessions
    │  2. rest_client.close() → close HTTP client
    ▼
Cleanup complete
```
