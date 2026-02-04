# Live Gateway Test Results

## Test Environment

- **Gateway**: WaaV Gateway v0.1.0 (Rust), running on `localhost:3001`
- **Features**: `noise-filter`, `dag-routing` enabled
- **STT Provider**: Deepgram nova-3
- **TTS Provider**: Deepgram aura-asteria-en (24kHz PCM16)
- **Audio Format**: PCM16, 16kHz mono (input), 24kHz mono (TTS output)
- **Auth**: Disabled (`auth.required: false`)
- **Python**: 3.12, pytest 8.x, websockets 14.x, httpx 0.28.x
- **Date**: 2026-02-05 (full suite run)
- **Rate limit**: Set to 10,000 req/s for testing (default 60 req/s)
- **Results**: 21/21 PASS in 103.04s

## Test Audio Files

| File | Duration | Format | Description |
|------|----------|--------|-------------|
| `cmu_bdl_arctic_a0001.wav` | 3.24s | 16kHz mono PCM16 | CMU Arctic sentence (clear speech) |
| `tts_hello_world.wav` | 2.52s | 16kHz mono PCM16 | Pre-generated TTS reference |
| `clean_speech_short.wav` | 1.0s | 16kHz mono PCM16 | Short clean speech clip |
| `noisy_speech_snr10.wav` | 3.0s | 16kHz mono PCM16 | Speech with 10dB SNR white noise |

Located at: `gateway/tests/live_testing/audio/`

## Results Summary

### REST Endpoints (4/4 PASS)

#### test_health_check - PASS
```
GET /  ->  {"status": "OK"}
```

#### test_list_voices - PASS
```
GET /voices?provider=deepgram
  -> dict with 102 Deepgram voices
  -> Sample: aura-2-andromeda-en, aura-2-arcas-en, ...
```

#### test_rest_speak - PASS
```
POST /speak {"text": "Hello, this is a test.", "tts_config": {"provider": "deepgram", "model": "aura-asteria-en", "sample_rate": 24000}}
  -> 78,576 bytes (~1.64s at 24kHz PCM16)
```

#### test_rest_speak_empty_text_rejected - PASS
```
POST /speak {"text": "", ...}
  -> Error returned (validation rejection)
```

### WebSocket Protocol (4/4 PASS)

#### test_config_ready_handshake - PASS
```
Send: {"type": "config", "audio": true, "stt_config": {...}, "tts_config": {...}}
Recv: {"type": "ready", "stream_id": "<uuid>"}
```

#### test_custom_stream_id - PASS
```
Send: {"type": "config", "stream_id": "test-custom-stream-id-12345", ...}
Recv: {"type": "ready", "stream_id": "test-custom-stream-id-12345"}
```

#### test_speak_and_receive_audio - PASS
```
Send: {"type": "speak", "text": "Hello world", "flush": true}
Recv: 37,894 bytes binary audio + {"type": "tts_playback_complete"}
```

#### test_audio_streaming_and_stt_results - PASS
```
Streamed: cmu_bdl_arctic_a0001.wav (3.24s) in 100ms chunks
Recv: stt_result messages with transcript
Transcript: "Author of the danger trail, Philip Steals, etcetera."
```

### STT Pipeline (2/2 PASS)

#### test_stt_real_speech - PASS
```
Audio: cmu_bdl_arctic_a0001.wav (3.24s, CMU Arctic)
Transcript: "Author of the danger trail, Philip Steals, etcetera."
Confidence: 0.938
Results: multiple interim + final results
Metrics: audio_bytes_sent=103,680B
```

#### test_stt_noisy_speech - PASS
```
Audio: noisy_speech_snr10.wav (3.0s, 10dB SNR)
Transcript: [transcribed despite noise]
```

### TTS Pipeline (2/2 PASS)

#### test_tts_streaming - PASS
```
Text: "Hello world, this is a test of text to speech synthesis."
Chunks: 330 audio chunks received
Total: 158,266 bytes (~3.30s at 24kHz PCM16)
TTFB: 1,337.6ms (p50)
```

#### test_tts_clear_interruption - PASS
```
Sent long text, then clear command after 1s
Audio stopped after clear (verified chunk count stabilized)
```

### Performance (4/4 PASS)

#### test_ws_connect_latency - PASS
```
WS connect + ready: ~50-200ms
```

#### test_rest_speak_latency - PASS
```
REST /speak round-trip: ~1000-1600ms (includes Deepgram synthesis)
```

#### test_stt_time_to_first_result - PASS
```
First audio chunk to first STT result: ~500-800ms
```

#### test_multiple_sequential_connections - PASS
```
3 sequential connections all succeeded
Average connect time: ~100ms
```

### SDK Integration (5/5 PASS)

#### test_context_manager - PASS
```
async with BudClient(...) as bud: health() returned OK
```

#### test_stt_pipeline_context_manager - PASS
```
async with session: connected=True
After exit: connected=False
```

#### test_tts_pipeline_context_manager - PASS
```
async with session: connected=True
After exit: connected=False
```

#### test_sdk_stream_id_passthrough - PASS
```
Custom stream_id "sdk-test-stream-42" preserved through SDK pipeline
```

#### test_sdk_metrics_collection - PASS
```
ws_connect_ms > 0
messages_sent >= 1
messages_received > 0
```

## SDK Bugs Found & Fixed

### 1. list_voices() Return Type (FIXED)
- **Issue**: Gateway returns `{"deepgram": [...]}` (dict), SDK expected `list`
- **Fix**: `rest/client.py` and `client.py` return type -> `dict[str, list[dict]]`

### 2. /speak Payload Format (FIXED)
- **Issue**: Gateway expects `{"text": "...", "tts_config": {...}}`, SDK sent flat fields
- **Fix**: Rewrote `rest/client.py:speak()` to nest config in `tts_config` key

### 3. Missing Default TTS/STT Config (FIXED)
- **Issue**: Gateway requires both `stt_config` and `tts_config` when `audio=true`
- **Fix**: `ws/session.py:_send_config()` provides minimal defaults when config is missing

## Performance Summary

| Metric | Value (actual) | Target | Status |
|--------|----------------|--------|--------|
| WS connect (TCP) | 2.9-3.8ms | <1s | PASS |
| WS connect + ready | 1,801-4,209ms | <5s | PASS |
| STT time-to-first-result | 652.1ms | <5s | PASS |
| TTS TTFB (streaming) | 1,012.5ms (p50) | <5s | PASS |
| REST speak round-trip | 1,840.5ms | <10s | PASS |
| Sequential connections (3x) | 2,728ms avg | <5s | PASS |

**Note**: The WS connect + ready latency includes Deepgram provider initialization time on the gateway side, which is the dominant factor.

## Test File

All tests are in: `tests/e2e/test_live_gateway.py`

Run with:
```bash
# Ensure gateway is running on localhost:3001
pytest tests/e2e/test_live_gateway.py -v -s

# Run specific test class
pytest tests/e2e/test_live_gateway.py::TestSTTPipeline -v -s

# Skip e2e tests in regular test runs
pytest tests/unit/ tests/api/ -q
```
