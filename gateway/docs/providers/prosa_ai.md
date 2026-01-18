# Prosa.ai Integration Documentation

## Overview

Prosa.ai is an Indonesian AI company founded in 2018 that specializes in Natural Language Processing (NLP) technology for Bahasa Indonesia. They offer STT (Speech-to-Text), TTS (Text-to-Speech), and other NLP services optimized for Indonesian language with support for Javanese, Sundanese, and English.

## Company Details

- **Website**: https://prosa.ai
- **Speech API**: https://speech.prosa.ai
- **TTS Product**: https://tts.prosa.ai
- **API Documentation**: https://docs2.prosa.ai
- **Founded**: 2018
- **Location**: Indonesia
- **User Base**: ~300,000 users (as of early 2024), 20,000+ monthly active users

## API Overview

### Base URLs

| Service | URL |
|---------|-----|
| REST API | `https://api.prosa.ai/v2` |
| WebSocket Streaming | `wss://s-api.prosa.ai/v2/speech/stt` |
| API Console | https://console.prosa.ai |

### Authentication

All API requests require the `x-api-key` header with the API key obtained from Prosa API Console.

```
x-api-key: <your_api_key>
```

For WebSocket connections, authentication can be done via:
1. HTTP header: `x-api-key: <api_key>`
2. First message: `{"token": "<api_key>"}`

---

## Speech-to-Text (STT) API

### Recognition Methods

| Method | Use Case | Limits |
|--------|----------|--------|
| **Synchronous** | Short audio, immediate response | Max 60 seconds, 10 MB |
| **Asynchronous** | Long audio, batch processing | Max 4 hours |
| **Streaming** | Real-time transcription | WebSocket connection |

### REST API Endpoints

| Operation | Method | Endpoint |
|-----------|--------|----------|
| List Models | GET | `/speech/stt/models` |
| Submit Request | POST | `/speech/stt` |
| Retrieve Jobs | GET | `/speech/stt` |
| Get Job Details | GET | `/speech/stt/{job_id}` |
| Check Status | GET | `/speech/stt/{job_id}/status` |
| Archive Job | DELETE | `/speech/stt/{job_id}` |
| Count Jobs | GET | `/speech/stt/count` |

### STT Models

| Model ID | Description |
|----------|-------------|
| `stt-general` | General-purpose STT for batch processing |
| `stt-general-online` | Real-time streaming STT |

### STT Request Body

```json
{
  "config": {
    "engine": "stt-general",
    "wait": true,
    "speaker_count": 1,
    "include_filler": false,
    "auto_punctuation": true,
    "enable_spoken_numerals": true,
    "enable_speech_insights": false,
    "enable_voice_insights": false
  },
  "request": {
    "label": "optional_label",
    "data": "base64_encoded_audio",
    "uri": "https://example.com/audio.wav"
  }
}
```

### STT Configuration Parameters

| Parameter | Type | Description |
|-----------|------|-------------|
| `engine` | string | STT model to use |
| `wait` | boolean | Wait for result (sync) or return job_id (async) |
| `speaker_count` | integer | Number of speakers for diarization |
| `include_filler` | boolean | Include filler words (um, uh) |
| `auto_punctuation` | boolean | Auto-add punctuation |
| `enable_spoken_numerals` | boolean | Convert numbers to words |
| `enable_speech_insights` | boolean | Enable speech analytics |
| `enable_voice_insights` | boolean | Enable voice analytics |

### STT Response

```json
{
  "job_id": "uuid",
  "status": "complete",
  "result": {
    "data": [
      {
        "transcript": "transcribed text",
        "time_start": 0.0,
        "time_end": 2.5,
        "channel": 0,
        "speaker": 1
      }
    ]
  },
  "model": {
    "id": "stt-general",
    "language": "id"
  }
}
```

### WebSocket Streaming API

**Endpoint**: `wss://s-api.prosa.ai/v2/speech/stt`

#### Configuration Message

```json
{
  "model": "stt-general-online",
  "label": "session_label",
  "include_partial": true,
  "audio": {
    "format": "wav",
    "channels": 1,
    "sample_rate": 16000
  }
}
```

#### Server Message Types

| Type | Description |
|------|-------------|
| `created` | Session started, contains job_id |
| `status` | Processing status update |
| `partial` | Interim transcription result |
| `result` | Final transcription with timestamps |
| `metadata` | Session summary (duration, quota_used) |
| `error` | Error message |

#### WebSocket Close Codes

| Code | Meaning |
|------|---------|
| 1000 | Success |
| 4000 | Invalid Auth |
| 4001 | Invalid Session Config |
| 4002 | Invalid Model |
| 4005 | Insufficient Quota |
| 4029 | Rate Limited |

### STT Error Codes

| Error Code | Message | Cause |
|------------|---------|-------|
| `auth_invalid_api_key` | Invalid x-api-key | Missing or invalid API key |
| `auth_unauthorized` | Unauthorized | No authentication |
| `forbidden` | Forbidden | No access |
| `quota_insufficient` | Insufficient quota | Not enough quota |
| `quota_empty` | Out of quota | No quota remaining |
| `asr_model_not_found` | ASR model not found | Invalid model |
| `asr_request_file_too_large` | Request too large | Exceeds duration limit |
| `asr_request_invalid_data` | Invalid base64-encoded data | Malformed audio data |
| `asr_request_no_audio_data` | No audio provided | Missing audio |
| `asr_request_unsupported_media_type` | Unsupported media type | Invalid format |
| `asr_request_uri_download_error` | Failed to download file | URI access failed |
| `asr_request_invalid_uri` | URI not supported | Invalid URI scheme |
| `job_not_found` | Job not found | Invalid job_id |
| `job_cancellation_failure` | Unable to cancel job | Job in progress |
| `internal_server_error` | Internal Server Error | Server failure |

---

## Text-to-Speech (TTS) API

### REST API Endpoints

| Operation | Method | Endpoint |
|-----------|--------|----------|
| List Models | GET | `/speech/tts/models` |
| Submit Request | POST | `/speech/tts` |
| Retrieve Jobs | GET | `/speech/tts` |
| Get Job Details | GET | `/speech/tts/{job_id}` |
| Check Status | GET | `/speech/tts/{job_id}/status` |
| Archive Job | DELETE | `/speech/tts/{job_id}` |
| Count Jobs | GET | `/speech/tts/count` |

### TTS Models (Voices)

| Model ID | Voice | Gender | Language | Domain |
|----------|-------|--------|----------|--------|
| `tts-dimas-formal` | Dimas | Male | Indonesian | Formal |
| `tts-dimas-expressive` | Dimas | Male | Indonesian | Expressive |
| `tts-ocha-friendly` | Ocha | Female | Indonesian | Friendly |
| `tts-dini` | Dini | Female | Indonesian | Audiobook |
| `tts-kinanti` | Kinanti | Female | Indonesian | Podcast |
| `tts-darah` | Darah | Female | Indonesian | Formal |
| `tts-abimana` | Abimana | Male | Indonesian | Virtual Assistant |
| `tts-roger` | Roger | Male | English | News |
| `tts-jennifer` | Jennifer | Female | English | News |

**Note**: 40+ voice variations available including custom voices.

### TTS Request Body

```json
{
  "config": {
    "model": "tts-dimas-formal",
    "wait": true,
    "pitch": 0,
    "tempo": 1.0,
    "audio_format": "opus"
  },
  "request": {
    "label": "optional_label",
    "text": "Text to synthesize"
  }
}
```

### TTS Configuration Parameters

| Parameter | Type | Description | Default |
|-----------|------|-------------|---------|
| `model` | string | TTS voice model | Required |
| `wait` | boolean | Wait for audio (sync) or async | true |
| `pitch` | integer | Pitch offset (-10 to 10) | 0 |
| `tempo` | float | Speech speed (0.5 to 2.0) | 1.0 |
| `audio_format` | string | Output format: opus, mp3, wav | opus |
| `as_signed_url` | boolean | Return audio as URL (2hr expiry) | false |

### TTS Limits

| Mode | Character Limit |
|------|-----------------|
| Synchronous (`wait: true`) | 280 characters |
| Asynchronous (`wait: false`) | 5,000 characters |

### TTS Response

```json
{
  "job_id": "uuid",
  "status": "complete",
  "result": {
    "data": "base64_audio_data",
    "duration": 5.2,
    "sample_rate": 48000,
    "channels": 1
  },
  "model": {
    "id": "tts-dimas-formal",
    "language": "id",
    "gender": "male"
  }
}
```

### Audio Output Specifications

| Format | Sample Rate | Channels |
|--------|-------------|----------|
| opus | 48000 Hz | 1 (Mono) |
| mp3 | 48000 Hz | 1 (Mono) |
| wav | 48000 Hz | 1 (Mono) |

---

## Supported Languages

| Language | Code | STT | TTS |
|----------|------|-----|-----|
| Indonesian (Bahasa Indonesia) | id | Yes | Yes |
| English | en | Yes | Yes |
| Javanese | jv | Yes | No |
| Sundanese | su | Yes | No |

---

## Pricing

### TTS Pricing (IDR)

| Package | Price | Characters | Validity |
|---------|-------|------------|----------|
| Free Trial | Free | 5,000 | 14 days |
| Starter | IDR 20,000 (~$1.30) | TBD | TBD |
| Pro | Contact Sales | TBD | TBD |
| Enterprise | Contact Sales | Unlimited | Custom |

### STT Pricing

- Quota-based pricing
- Contact sales for enterprise pricing
- Free tier available for testing

---

## Supported Audio Formats

### STT Input Formats
- WAV
- MP3
- OGG
- FLAC
- AAC
- M4A
- WebM

### TTS Output Formats
- Opus (default)
- MP3
- WAV

---

## Webhook Integration

Register webhooks to receive job completion notifications.

### Webhook Headers

| Header | Description |
|--------|-------------|
| `X-Prosa-Signature` | Request signature for verification |
| `X-Prosa-Event-UUID` | Unique event identifier |
| `X-Prosa-Event` | Event type (job.complete, job.error) |

### Webhook Response

- **Success**: HTTP 204 No Content
- **Error**: HTTP 400 Bad Request

---

## Implementation Plan

### STT Provider Implementation

1. **REST API Client** (Priority: High)
   - Synchronous recognition for short audio
   - Asynchronous recognition for long audio
   - Job status polling
   - Support for base64 and URI audio sources

2. **WebSocket Streaming** (Priority: High)
   - Real-time streaming transcription
   - Partial and final result handling
   - Session management

### TTS Provider Implementation

1. **REST API Client** (Priority: High)
   - Synchronous synthesis for short text
   - Asynchronous synthesis for long text
   - Multiple voice support
   - Audio format selection (opus, mp3, wav)

### Features Implemented

- [x] STT REST API (sync/async)
- [x] STT WebSocket streaming
- [x] TTS REST API (sync/async)
- [ ] Webhook support
- [ ] Speaker diarization
- [ ] Speech insights

---

## WaaV Gateway Implementation

### Source Files

| File | Description |
|------|-------------|
| `src/core/stt/prosa_ai/config.rs` | STT configuration, models, audio formats |
| `src/core/stt/prosa_ai/client.rs` | STT WebSocket streaming client |
| `src/core/stt/prosa_ai/tests.rs` | STT unit tests |
| `src/core/stt/prosa_ai/mod.rs` | STT module exports |
| `src/core/tts/prosa_ai/config.rs` | TTS configuration, voices, formats |
| `src/core/tts/prosa_ai/client.rs` | TTS REST API client |
| `src/core/tts/prosa_ai/tests.rs` | TTS unit tests |
| `src/core/tts/prosa_ai/mod.rs` | TTS module exports |

### Provider Registration

| Property | STT | TTS |
|----------|-----|-----|
| Provider ID | `prosa-ai` | `prosa-ai` |
| Aliases | `prosa_ai-stt`, `prosa-stt`, `prosa`, `prosaid`, `prosaai` | `prosa_ai-tts`, `prosa-tts`, `prosaid-tts`, `prosaai-tts` |

### Test Coverage

| Category | Tests |
|----------|-------|
| STT Tests | 101 |
| TTS Tests | 102 |
| **Total** | **203** |

### Configuration Example

```yaml
# config.yaml
stt:
  provider: prosa-ai
  api_key: ${PROSA_API_KEY}
  model: stt-general-online
  sample_rate: 16000
  channels: 1

tts:
  provider: prosa-ai
  api_key: ${PROSA_API_KEY}
  voice_id: dimas-formal
  audio_format: opus
```

### Environment Variables

| Variable | Description |
|----------|-------------|
| `PROSA_API_KEY` | Prosa.ai API key from console.prosa.ai |

---

## Test Plan

### Unit Tests

1. **Configuration Tests**
   - Config validation (empty API key, invalid params)
   - Config defaults
   - Voice/model parsing

2. **Response Parsing Tests**
   - Success responses
   - Error responses
   - Partial/final transcripts

3. **Client Tests**
   - Connection management
   - Audio buffering
   - Callback handling

### Integration Tests

1. **STT Integration**
   - Short audio transcription
   - Long audio (async) transcription
   - Streaming transcription
   - Multi-channel audio

2. **TTS Integration**
   - Short text synthesis
   - Long text (async) synthesis
   - Voice selection
   - Audio format selection

---

## Code Examples

### STT REST Example (Python)

```python
import requests
import base64

api_key = "your_api_key"
audio_data = base64.b64encode(open("audio.wav", "rb").read()).decode()

response = requests.post(
    "https://api.prosa.ai/v2/speech/stt",
    headers={"x-api-key": api_key},
    json={
        "config": {
            "engine": "stt-general",
            "wait": True
        },
        "request": {
            "data": audio_data
        }
    }
)

result = response.json()
print(result["result"]["data"][0]["transcript"])
```

### TTS REST Example (Python)

```python
import requests
import base64

api_key = "your_api_key"

response = requests.post(
    "https://api.prosa.ai/v2/speech/tts",
    headers={"x-api-key": api_key},
    json={
        "config": {
            "model": "tts-dimas-formal",
            "wait": True,
            "audio_format": "wav"
        },
        "request": {
            "text": "Selamat pagi, Indonesia!"
        }
    }
)

result = response.json()
audio_data = base64.b64decode(result["result"]["data"])
with open("output.wav", "wb") as f:
    f.write(audio_data)
```

### WebSocket Streaming Example (Python)

```python
import asyncio
import websockets
import json

async def stream_stt():
    api_key = "your_api_key"
    uri = "wss://s-api.prosa.ai/v2/speech/stt"

    async with websockets.connect(uri, extra_headers={"x-api-key": api_key}) as ws:
        # Send config
        await ws.send(json.dumps({
            "model": "stt-general-online",
            "include_partial": True
        }))

        # Wait for session created
        response = await ws.recv()
        print(f"Session: {response}")

        # Send audio chunks
        with open("audio.wav", "rb") as f:
            while chunk := f.read(16000):
                await ws.send(chunk)

        # Signal end of audio
        await ws.send(b"")

        # Receive transcriptions
        while True:
            try:
                msg = await ws.recv()
                data = json.loads(msg)
                if data["type"] == "result":
                    print(f"Final: {data['transcript']}")
                elif data["type"] == "partial":
                    print(f"Partial: {data['transcript']}")
            except websockets.ConnectionClosed:
                break

asyncio.run(stream_stt())
```

---

## References

- [Prosa.ai Main Site](https://prosa.ai)
- [Speech API Portal](https://speech.prosa.ai)
- [TTS Product Page](https://tts.prosa.ai)
- [API Documentation v2](https://docs2.prosa.ai)
- [STT Overview](https://docs2.prosa.ai/speech/stt/overview/)
- [TTS Overview](https://docs2.prosa.ai/speech/tts/overview/)
- [STT REST API](https://docs2.prosa.ai/speech/stt/rest/api/)
- [TTS REST API](https://docs2.prosa.ai/speech/tts/rest/api/)
- [STT Streaming](https://docs2.prosa.ai/speech/stt/streaming/getting_started/)
