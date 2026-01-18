# Smallest.ai (Waves) Integration Documentation

> **Provider:** Smallest.ai
> **Product:** Waves TTS API
> **Last Updated:** 2026-01-13
> **Status:** COMPLETE

---

## 1. Overview

Smallest.ai provides the **Waves** platform for ultra-low latency Text-to-Speech (TTS) synthesis. Their flagship **Lightning** model achieves sub-100ms latency for 10 seconds of audio, making it one of the fastest TTS APIs available.

### Key Capabilities

| Feature | Supported | Notes |
|---------|-----------|-------|
| TTS | YES | REST + WebSocket APIs |
| STT | NO | Not available |
| Voice Cloning | YES | Lightning-Large model only |
| Streaming | YES | WebSocket real-time streaming |
| Multi-language | YES | 16+ languages |

### USPs (Unique Selling Points)

1. **Ultra-Low Latency**: Sub-100ms for Lightning model
2. **Voice Cloning**: Quick voice cloning with Lightning-Large
3. **Real-time Streaming**: WebSocket-based streaming for LLM integration
4. **Multiple Models**: Lightning, Lightning-Large, Lightning-V2, Thunder
5. **Enterprise Ready**: HIPAA, SOC 2 Type II, GDPR, ISO 27001 certified

---

## 2. API Architecture

### Base URLs

| Service | URL |
|---------|-----|
| REST API | `https://waves-api.smallest.ai` |
| WebSocket | `wss://waves-api.smallest.ai` |
| Console | `https://console.smallest.ai` |
| Docs | `https://waves-docs.smallest.ai` |

### Authentication

- **Type**: Bearer Token
- **Header**: `Authorization: Bearer <API_KEY>`
- **Key Source**: Console → API Keys tab

### Models

| Model | Latency | Voices | Voice Cloning | Languages |
|-------|---------|--------|---------------|-----------|
| lightning | ~100ms | 7 | NO | en, hi |
| lightning-large | ~300ms | 7+ | YES | en, hi |
| lightning-v2 | <200ms | - | YES | 16+ |
| thunder | ~200ms | 20 | - | Multiple |

---

## 3. REST API Reference

### 3.1 Text-to-Speech (Lightning)

**Endpoint:** `POST /api/v1/lightning/get_speech`

**Request:**
```json
{
  "text": "Hello world",
  "voice_id": "emily",
  "sample_rate": 24000,
  "speed": 1.0,
  "language": "en",
  "output_format": "wav"
}
```

**Parameters:**

| Parameter | Type | Required | Default | Range | Description |
|-----------|------|----------|---------|-------|-------------|
| text | string | YES | - | - | Text to synthesize |
| voice_id | string | YES | - | - | Voice identifier |
| sample_rate | int | NO | 24000 | 8000-24000 | Audio sample rate in Hz |
| speed | float | NO | 1.0 | 0.5-2.0 | Speech speed multiplier |
| language | string | NO | "en" | en/hi | Number pronunciation format |
| output_format | string | NO | "pcm" | pcm/mp3/wav/mulaw | Output audio format |

**Response:** Binary audio data with content-type based on output_format

### 3.2 Get Voices

**Endpoint:** `GET /api/v1/{model}/get_voices`

**Path Parameters:**
- model: `lightning`, `lightning-large`, or `lightning-v2`

**Response:**
```json
{
  "voices": [
    {
      "voiceId": "emily",
      "displayName": "Emily",
      "tags": {
        "language": ["en"],
        "accent": "american",
        "gender": "female"
      }
    }
  ]
}
```

### 3.3 Add Voice (Clone)

**Endpoint:** `POST /api/v1/lightning-large/add_voice`

**Content-Type:** `multipart/form-data`

**Parameters:**
- `displayName` (string): Display name for the voice
- `file` (binary): Audio file for voice cloning

**Response:**
```json
{
  "message": "Voice created successfully",
  "data": {
    "voiceId": "custom_voice_123",
    "model": "lightning-large",
    "status": "ready"
  }
}
```

---

## 4. WebSocket API Reference

### 4.1 Lightning-V2 Streaming

**Endpoint:** `wss://waves-api.smallest.ai/api/v1/lightning-v2/get_speech/stream`

**Query Parameters:**
- `timeout`: Connection timeout (20-60 seconds, default: 20)

**Example:** `wss://waves-api.smallest.ai/api/v1/lightning-v2/get_speech/stream?timeout=60`

### 4.2 Request Message (TTSRequest)

```json
{
  "voice_id": "emily",
  "text": "Hello, how are you?",
  "language": "en",
  "sample_rate": 24000,
  "speed": 1.0,
  "consistency": 0.5,
  "enhancement": 1,
  "similarity": 0,
  "max_buffer_flush_ms": 0,
  "continue": false,
  "flush": false,
  "complete_backoff_ms": 4000
}
```

**Parameters:**

| Parameter | Type | Default | Range | Description |
|-----------|------|---------|-------|-------------|
| voice_id | string | - | - | Voice identifier (required) |
| text | string | - | - | Text to synthesize (required) |
| language | string | "en" | en/hi/mr/kn/ta/bn/gu/de/fr/es/it/pl/nl/ru/ar/he | Language code |
| sample_rate | int | 24000 | - | Audio sample rate in Hz |
| speed | float | 1.0 | 0.1-5.0 | Speech speed multiplier |
| consistency | float | 0.5 | 0-1 | Word repetition/skipping control |
| enhancement | int | 1 | 0-2 | Audio quality enhancement level |
| similarity | float | 0 | 0-1 | Voice similarity to reference |
| max_buffer_flush_ms | int | 0 | 0-1000 | Max wait before flushing output |
| continue | bool | false | - | Buffer and await more input |
| flush | bool | false | - | Force current buffer flush |
| complete_backoff_ms | int | 4000 | 0-10000 | Wait time after last chunk |

### 4.3 Response Messages

**Chunk Response:**
```json
{
  "request_id": "abc123",
  "status": "chunk",
  "data": {
    "audio": "<base64-encoded-audio>"
  }
}
```

**Complete Response:**
```json
{
  "request_id": "abc123",
  "status": "complete",
  "message": "All chunks sent",
  "done": true
}
```

---

## 5. Supported Languages

| Code | Language |
|------|----------|
| en | English |
| hi | Hindi |
| mr | Marathi |
| kn | Kannada |
| ta | Tamil |
| bn | Bengali |
| gu | Gujarati |
| de | German |
| fr | French |
| es | Spanish |
| it | Italian |
| pl | Polish |
| nl | Dutch |
| ru | Russian |
| ar | Arabic |
| he | Hebrew |

---

## 6. Pricing

### Per-Plan Pricing

| Plan | Monthly Cost | TTS (Lightning V1) | TTS (Lightning V2) | Voice Clones |
|------|--------------|--------------------|--------------------|--------------|
| Free | $0 | $0.15/10K chars | - | 5 |
| Personal | $49 | $0.08/10K chars | $0.20/10K chars | 15 |
| Business | $1,999 | $0.05/10K chars | $0.10/10K chars | Unlimited |
| Enterprise | Custom | Custom | Custom | Custom |

### Features by Plan

| Feature | Free | Personal | Business | Enterprise |
|---------|------|----------|----------|-----------|
| Concurrent Requests | 1 | 3 | 15 | Custom |
| TTS Projects | 1 | 10 | 500 | Custom |
| API Access | Limited | Full | Full | Custom |

---

## 7. Error Handling

### HTTP Status Codes

| Code | Description | Action |
|------|-------------|--------|
| 400 | Bad Request | Validate parameters |
| 401 | Unauthorized | Check API key |
| 429 | Rate Limited | Implement backoff |
| 500 | Server Error | Retry with backoff |

### WebSocket Error Format

```json
{
  "error": "invalid_parameter",
  "message": "voice_id is required"
}
```

---

## 8. Best Practices

### Performance Optimization

1. **Use WebSocket for streaming**: Lower latency than REST for real-time applications
2. **Chunk text at sentence boundaries**: ~240 characters per chunk for optimal processing
3. **Set appropriate timeout**: Extend to 60s for sessions with pauses
4. **Use `continue` flag**: For LLM streaming to buffer input

### Audio Quality

1. **Default sample_rate 24000**: Best balance of quality and size
2. **Use enhancement=1**: Good quality without excessive processing
3. **Adjust consistency for clones**: Decrease to prevent skipped words

### Cost Optimization

1. **Use Lightning V1 for simple TTS**: Lower cost per character
2. **Batch requests when possible**: Reduce overhead
3. **Cache common phrases**: Avoid re-synthesizing static content

---

## 9. Integration Plan for Bud WaaV

### Architecture Decision

**Approach**: Dynamic Plugin using TTSRequestBuilder pattern

**Rationale**:
1. REST API for simple synthesis (like Lightning model)
2. WebSocket API for streaming TTS (like Lightning-V2)
3. Similar to Deepgram TTS WebSocket pattern
4. Voice cloning as optional feature

### Implementation Components

1. **SmallestTtsConfig** - Configuration with model, voice_id, speed, etc.
2. **SmallestTtsRequest/Response** - Message types for both REST and WebSocket
3. **SmallestRequestBuilder** - TTSRequestBuilder implementation for REST
4. **SmallestTts** - BaseTTS implementation with WebSocket streaming
5. **Plugin Registration** - Inventory-based registration with aliases

### File Structure

```
src/core/tts/smallest/
├── mod.rs           # Module exports and constants
├── config.rs        # SmallestTtsConfig, SmallestModel, SmallestVoice
├── messages.rs      # Request/response types
└── provider.rs      # SmallestTts implementation
```

### Test Plan

1. **Config Tests**: Model selection, voice validation, parameter ranges
2. **Message Tests**: JSON serialization, response parsing
3. **Request Builder Tests**: HTTP request construction, headers
4. **WebSocket Tests**: Connection, message flow, audio streaming
5. **Integration Tests**: End-to-end synthesis with mock server

---

## 10. References

- **API Documentation**: https://waves-docs.smallest.ai
- **Python SDK**: https://github.com/smallest-inc/smallest-python-sdk
- **Node.js SDK**: https://github.com/smallest-inc/smallest-node-sdk
- **Examples**: https://github.com/smallest-inc/waves-examples
- **Console**: https://console.smallest.ai
- **Pricing**: https://smallest.ai/pricing
- **Support**: support@smallest.ai
- **Discord**: https://discord.gg/Ub25S48hSf

---

## 11. Changelog

| Date | Change |
|------|--------|
| 2026-01-13 | Initial documentation created |
