# Resemble AI TTS Provider Integration

> **Status:** DONE
> **Research Date:** 2026-01-13
> **Implementation Date:** 2026-01-13
> **Provider Type:** TTS + Speech-to-Speech + Voice Cloning + Deepfake Detection

---

## 1. Provider Overview

### Basic Information
- **Website:** https://www.resemble.ai
- **API Documentation:** https://docs.resemble.ai
- **GitHub (Chatterbox):** https://github.com/resemble-ai/chatterbox
- **Hugging Face:** https://huggingface.co/ResembleAI/chatterbox-turbo

### Capabilities Matrix
| Capability | Supported | Notes |
|------------|-----------|-------|
| TTS | YES | HTTP streaming, sync, WebSocket (Business+) |
| STT | YES | Speech-to-Text with job creation |
| Speech-to-Speech | YES | Voice conversion preserving timing/emotion |
| Voice Cloning | YES | Rapid (10s) and Professional (10min) clones |
| Deepfake Detection | YES | Identity verification and audio watermarking |
| Streaming | YES | HTTP chunked + WebSocket (Business plan) |

### Technical Specifications
- **Authentication:** Bearer Token via `Authorization` header
- **Protocol:** REST (HTTP POST) + WebSocket (Business+)
- **Audio Formats:** WAV, MP3
- **Precisions:** MULAW, PCM_16, PCM_24, PCM_32
- **Sample Rates:** 8000, 16000, 22050, 32000, 44100, 48000 Hz
- **Languages:** 149+ languages
- **Open Source:** Yes - Chatterbox (MIT licensed)

---

## 2. API Endpoints

### 2.1 HTTP Streaming TTS
**Endpoint:** `POST https://f.cluster.resemble.ai/stream`

**Headers:**
```
Authorization: Bearer <api-token>
Content-Type: application/json
```

**Request Body:**
```json
{
  "voice_uuid": "your-voice-uuid",
  "data": "Hello, this is a test.",
  "sample_rate": 44100,
  "precision": "PCM_16",
  "model": "chatterbox-turbo",
  "use_hd": false
}
```

**Parameters:**
| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| voice_uuid | string | Yes | Voice identifier from voices API |
| data | string | Yes | Text or SSML (max 2000 chars) |
| project_uuid | string | No | Project to store clip |
| model | string | No | `chatterbox-turbo` for lower latency |
| precision | string | No | MULAW, PCM_16, PCM_24, PCM_32 (default) |
| sample_rate | number | No | Audio sample rate in Hz |
| use_hd | boolean | No | Enable HD synthesis (default: false) |

**Response:** Streaming `application/octet-stream` (chunked WAV)

### 2.2 Synchronous TTS
**Endpoint:** `POST https://f.cluster.resemble.ai/synthesize`

Same parameters as streaming, but returns complete audio as base64:

```json
{
  "audio_content": "<base64-encoded-audio>",
  "audio_timestamps": {...},
  "duration": 4.02,
  "success": true,
  "output_format": "wav"
}
```

### 2.3 WebSocket Streaming (Business Plan)
**Endpoint:** `wss://websocket.cluster.resemble.ai/stream`

**Connection Flow:**
1. Establish WebSocket connection
2. Send JSON synthesis request
3. Receive streaming audio frames
4. Await `audio_end` message before closing

**Request Parameters:**
| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| voice_uuid | string | Yes | Voice identifier |
| project_uuid | string | Yes | Project attachment |
| data | string | Yes | Text or SSML (max 3000 chars) |
| model | string | No | `chatterbox-turbo` for lower latency |
| request_id | number | No | Auto-increments if omitted |
| binary_response | boolean | No | Raw bytes (true) or JSON (false) |
| output_format | string | No | `wav` or `mp3` |
| sample_rate | number | No | 8000, 16000, 22050, 32000, 44100 Hz |
| precision | string | No | PCM_32, PCM_24, PCM_16, MULAW |
| no_audio_header | boolean | No | Omits WAV headers when true |

**Concurrency Limits:**
- 20 simultaneous sessions per cluster
- 20 parallel connections per API key

### 2.4 List Voices
**Endpoint:** `GET https://app.resemble.ai/api/v2/voices`

**Query Parameters:**
- `page`: Page number
- `page_size`: Results per page

**Response:**
```json
{
  "items": [
    {
      "uuid": "voice-uuid",
      "name": "Voice Name",
      "status": "active",
      "voice_type": "rapid_clone",
      "supported_languages": ["en-US"],
      "api_support": {
        "sync": true,
        "async": true,
        "streaming": true
      }
    }
  ]
}
```

### 2.5 Speech-to-Speech
**Endpoint:** `POST https://f.cluster.resemble.ai/synthesize`

Uses SSML with `<resemble:convert>` tag:

```json
{
  "voice_uuid": "target-voice-uuid",
  "data": "<speak><resemble:convert src=\"https://audio.url/input.wav\"></resemble:convert></speak>",
  "sample_rate": 44100
}
```

**Input Requirements:**
- Format: WAV with single speaker
- Max size: 50 MB
- Max duration: 5 minutes

---

## 3. Models

### 3.1 Chatterbox-Turbo
- **Parameters:** 350M
- **Latency:** 75ms
- **Performance:** 6x faster than real-time on GPU
- **License:** MIT (open source)
- **Features:**
  - Zero-shot voice cloning (5 seconds reference)
  - Emotion control parameter
  - Paralinguistic prompting (sighs, gasps)
  - Built-in PerTH watermarking

### 3.2 Default Model
- Standard Resemble AI synthesis engine
- Higher quality with `use_hd: true`
- Supports all languages

---

## 4. Voice Cloning

### 4.1 Rapid Clone
- **Audio Required:** 10 seconds to 1 minute
- **Processing Time:** ~1 minute
- **Plan Limit:** 500 clones
- **Use Case:** Quick prototyping, testing

### 4.2 Professional Clone
- **Audio Required:** 10 minutes
- **Processing Time:** ~1 hour
- **Plan Limit:** 10 clones
- **Features:**
  - Emotional nuances
  - Speech-to-Speech support
  - Higher fidelity

---

## 5. Pricing

| Plan | Price | Features |
|------|-------|----------|
| Free | $0 | Limited features |
| Creator | $30/month | Enhanced tools, custom voices |
| Professional | $60/month | Priority support, multi-language |
| Business | Custom | WebSocket API, enterprise features |

### API Usage
- **Per Second:** $0.006 (after first 1000 free)
- **Included:** 320,000 seconds/month on paid plans
- **Voice Clones:** 500 rapid + 10 professional per plan

---

## 6. Implementation Plan

### 6.1 Module Structure
```
src/core/tts/resemble/
├── mod.rs           # Module exports and constants
├── config.rs        # ResembleTtsConfig, ResembleModel, ResemblePrecision
└── provider.rs      # ResembleTts implementing BaseTTS
```

### 6.2 Implementation Steps

1. **Create config.rs**
   - ResembleModel enum (ChatterboxTurbo, Default)
   - ResemblePrecision enum (Mulaw, Pcm16, Pcm24, Pcm32)
   - ResembleTtsConfig struct
   - ResembleStreamRequest struct
   - From TTSConfig conversion

2. **Create provider.rs**
   - ResembleRequestBuilder implementing TTSRequestBuilder
   - ResembleTts implementing BaseTTS
   - list_voices() static method
   - HTTP streaming via reqwest

3. **Create mod.rs**
   - API URL constants
   - Default values
   - Public exports

4. **Update tts/mod.rs**
   - Add resemble module
   - Update factory function

5. **Update plugin registration**
   - Add to builtin/mod.rs
   - Add to dispatch.rs PHF map

### 6.3 Configuration Mapping

| TTSConfig Field | Resemble Mapping |
|-----------------|------------------|
| api_key | Authorization: Bearer header |
| voice_id | voice_uuid |
| model | "chatterbox-turbo" or default |
| audio_format | output_format (wav/mp3) |
| sample_rate | sample_rate |

### 6.4 Provider Options
- `precision`: MULAW, PCM_16, PCM_24, PCM_32
- `use_hd`: Enable HD synthesis
- `project_uuid`: Optional project storage

---

## 7. Testing Plan

### 7.1 Unit Tests
- Config parsing and validation
- Model enum serialization
- Precision enum serialization
- Request body construction
- Response parsing

### 7.2 Integration Tests (with credentials)
- Connection and voice listing
- Basic TTS synthesis
- Streaming with chatterbox-turbo
- HD mode synthesis
- Different precisions/sample rates
- Error responses (invalid voice, rate limit)

### 7.3 Test Cases
```rust
#[test]
fn test_resemble_config_defaults()
#[test]
fn test_resemble_model_serialization()
#[test]
fn test_resemble_precision_serialization()
#[test]
fn test_request_body_construction()
#[tokio::test]
async fn test_resemble_connect()
#[tokio::test]
async fn test_resemble_speak_basic()
#[tokio::test]
async fn test_resemble_speak_with_hd()
#[tokio::test]
async fn test_resemble_list_voices()
```

---

## 8. Best Practices

### Performance
- Use `chatterbox-turbo` model for lowest latency (75ms)
- Use HTTP streaming for real-time applications
- Cache voice list (doesn't change frequently)
- Consider `use_hd: false` for faster synthesis

### Quality
- Use `use_hd: true` for highest quality
- Use PCM_32 precision for studio quality
- Use Professional clones for production voices

### Cost Optimization
- Monitor included seconds usage
- Use Rapid clones for testing
- Batch text for efficient synthesis

### Security
- Never expose API token in client-side code
- Use server-side proxy for API calls
- Rotate tokens periodically

---

## 9. Error Handling

### HTTP Errors
| Status | Meaning | Action |
|--------|---------|--------|
| 400 | Bad Request | Check request body format |
| 401 | Unauthorized | Verify API token, check plan |
| 404 | Not Found | Verify voice_uuid exists |
| 429 | Rate Limited | Back off, retry later |
| 500 | Server Error | Retry with exponential backoff |

### WebSocket Errors
- **Unrecoverable:** Connection closes (e.g., 401 ConnectionFailure)
- **Recoverable:** Invalid JSON, missing fields (connection persists)

---

## 10. References

- [Resemble AI Documentation](https://docs.resemble.ai/welcome)
- [HTTP Streaming API](https://docs.resemble.ai/api-reference/text-to-speech/stream-synthesize)
- [WebSocket API](https://docs.resemble.ai/voice-generation/text-to-speech/streaming-websocket)
- [Speech-to-Speech](https://docs.resemble.ai/voice-generation/speech-to-speech)
- [Voice Cloning](https://docs.resemble.ai/voice-creation/voices/clone-overview)
- [Chatterbox GitHub](https://github.com/resemble-ai/chatterbox)
- [LiveKit Plugin](https://docs.livekit.io/reference/python/v1/livekit/plugins/resemble/index.html)
