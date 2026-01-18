# Resemble AI TTS Provider Integration

> **Status:** IN_PROGRESS
> **Last Updated:** 2026-01-13
> **Provider Type:** TTS (Text-to-Speech), Speech-to-Speech, Voice Cloning

---

## Provider Overview

Resemble AI provides neural voice cloning and text-to-speech with their Chatterbox models. They offer enterprise-grade voice synthesis with deepfake detection capabilities. The platform supports 149+ languages and offers multiple synthesis modes including synchronous, HTTP streaming, and WebSocket streaming.

### Key Features

| Feature | Support |
|---------|---------|
| TTS Streaming | Yes (HTTP + WebSocket) |
| Voice Cloning | Yes (10-second voice cloning) |
| Speech-to-Speech | Yes (Real-time) |
| Languages | 149+ |
| Max Text Length | 3000 characters (sync/ws), 2000 (http stream) |
| Audio Formats | WAV, MP3 |
| Sample Rates | 8000, 16000, 22050, 32000, 44100, 48000 Hz |
| Precision | PCM_32, PCM_24, PCM_16, MULAW |
| HD Mode | Yes (optional) |
| SSML Support | Yes |

### Pricing

| Plan | Features |
|------|----------|
| Starter | Sync + HTTP streaming |
| Business | + WebSocket streaming, higher concurrency |
| Enterprise | Custom limits, on-premises |

---

## API Documentation

### Base URLs

| Service | URL |
|---------|-----|
| Voices API | `https://app.resemble.ai/api/v2` |
| Sync TTS | `https://f.cluster.resemble.ai/synthesize` |
| HTTP Stream | `https://f.cluster.resemble.ai/stream` |
| WebSocket | `wss://websocket.cluster.resemble.ai/stream` |

### Authentication

Resemble AI uses Bearer token authentication:

```http
Authorization: Bearer <YOUR_API_TOKEN>
```

**Environment Variable:**
```bash
RESEMBLE_API_KEY=your-api-key-here
```

API tokens are obtained from: https://app.resemble.ai/account/api

---

## Endpoints

### 1. List Voices

Retrieves available voices for the account.

**Endpoint:** `GET /voices`

**Base URL:** `https://app.resemble.ai/api/v2/voices`

**Headers:**
| Header | Value | Required |
|--------|-------|----------|
| Authorization | Bearer {token} | Yes |

**Query Parameters:**
| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| page | integer | Yes | 1 | Page number (>= 1) |
| page_size | integer | No | 10 | Results per page (10-1000) |
| advanced | boolean | No | false | Include advanced details |

**Response:**
```json
{
  "success": true,
  "page": 1,
  "num_pages": 5,
  "page_size": 10,
  "items": [
    {
      "uuid": "abc123",
      "name": "John",
      "status": "ready",
      "default_language": "en-US",
      "voice_type": "custom",
      "supported_languages": ["en-US", "es-ES"],
      "api_support": {
        "sync": true,
        "async": true,
        "direct_synthesis": true,
        "streaming": true
      },
      "created_at": "2025-01-01T00:00:00Z",
      "updated_at": "2025-01-01T00:00:00Z"
    }
  ]
}
```

---

### 2. Synchronous TTS (Sync)

Returns complete audio in a single response.

**Endpoint:** `POST /synthesize`

**Full URL:** `https://f.cluster.resemble.ai/synthesize`

**Headers:**
| Header | Value | Required |
|--------|-------|----------|
| Authorization | Bearer {token} | Yes |
| Content-Type | application/json | Yes |

**Request Body:**
```json
{
  "voice_uuid": "abc123",
  "data": "Hello, world!",
  "project_uuid": "optional-project-id",
  "title": "optional-clip-title",
  "model": "chatterbox-turbo",
  "precision": "PCM_16",
  "output_format": "wav",
  "sample_rate": 22050,
  "use_hd": false
}
```

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| voice_uuid | string | Yes | Voice identifier |
| data | string | Yes | Text or SSML (max 3000 chars) |
| project_uuid | string | No | Store clip in project |
| title | string | No | Clip label |
| model | string | No | "chatterbox", "chatterbox-turbo", "chatterbox-multilingual" |
| precision | string | No | PCM_32 (default), PCM_24, PCM_16, MULAW |
| output_format | string | No | wav (default), mp3 |
| sample_rate | integer | No | 8000, 16000, 22050, 32000, 44100, 48000 |
| use_hd | boolean | No | Enable HD synthesis (default: false) |

**Response:**
```json
{
  "success": true,
  "audio_content": "base64-encoded-audio",
  "audio_timestamps": [...],
  "duration": 2.5,
  "synth_duration": 0.3,
  "output_format": "wav",
  "sample_rate": 22050,
  "title": "clip-title",
  "issues": []
}
```

---

### 3. HTTP Streaming TTS

Progressive audio delivery via chunked responses.

**Endpoint:** `POST /stream`

**Full URL:** `https://f.cluster.resemble.ai/stream`

**Headers:**
| Header | Value | Required |
|--------|-------|----------|
| Authorization | Bearer {token} | Yes |
| Content-Type | application/json | Yes |

**Request Body:**
```json
{
  "voice_uuid": "abc123",
  "data": "Hello, world!",
  "project_uuid": "optional-project-id",
  "model": "chatterbox-turbo",
  "precision": "PCM_16",
  "sample_rate": 22050,
  "use_hd": false
}
```

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| voice_uuid | string | Yes | Voice identifier |
| data | string | Yes | Text or SSML (max 2000 chars) |
| project_uuid | string | No | Project association |
| model | string | No | Synthesis model |
| precision | string | No | PCM_32, PCM_24, PCM_16, MULAW |
| sample_rate | integer | No | Sample rate in Hz |
| use_hd | boolean | No | Enable HD mode |

**Response:** Chunked WAV data (application/octet-stream)

---

### 4. WebSocket Streaming TTS

Lowest-latency streaming with per-chunk metadata.

**Endpoint:** `wss://websocket.cluster.resemble.ai/stream`

**Requirements:** Business plan or higher

**Request Message:**
```json
{
  "voice_uuid": "abc123",
  "project_uuid": "project-id",
  "data": "Hello, world!",
  "model": "chatterbox-turbo",
  "request_id": "unique-request-id",
  "binary_response": false,
  "output_format": "wav",
  "sample_rate": 22050,
  "precision": "PCM_16",
  "no_audio_header": false
}
```

**Response Messages:**
- **Audio frames:** JSON with base64 audio or binary frames
- **audio_end:** Terminal message with matching request_id
- **Error:** JSON with error_name, status_code, error_params

**Concurrency Limits:**
- 20 simultaneous sessions per cluster
- 20 parallel connections per API key

---

## Models

### Chatterbox
- **ID:** `chatterbox`
- **Type:** Standard model
- **Features:** High quality synthesis

### Chatterbox Turbo
- **ID:** `chatterbox-turbo`
- **Type:** Low-latency model
- **Features:** 350M parameters, single-step decoder, paralinguistic tags
- **Tags:** [cough], [laugh], [chuckle], etc.
- **Requirement:** Rapid English or Pre-Built Library voices

### Chatterbox Multilingual
- **ID:** `chatterbox-multilingual`
- **Languages:** 24+ (Arabic, Danish, German, Greek, English, Spanish, Finnish, French, Hebrew, Hindi, Italian, Japanese, Korean, Malay, Dutch, Norwegian, Polish, Portuguese, Russian, Swedish, Swahili, Turkish, Chinese)

---

## Implementation Plan

### Architecture

```
+---------------------------------------------------------+
|                  Resemble AI TTS Provider                |
+---------------------------------------------------------+
|  ResembleTtsConfig                                       |
|  +-- api_key: String                                     |
|  +-- voice_uuid: String                                  |
|  +-- model: ResembleModel (Chatterbox, ChatterboxTurbo)  |
|  +-- output_format: ResembleOutputFormat (Wav, Mp3)      |
|  +-- precision: ResemblePrecision (PCM_32, PCM_16, etc.) |
|  +-- sample_rate: u32                                    |
|  +-- use_hd: bool                                        |
+---------------------------------------------------------+
|  ResembleTts (BaseTTS implementation)                    |
|  +-- new(TTSConfig) -> Result<Self>                      |
|  +-- connect() -> HTTP client initialization             |
|  +-- speak(text, flush) -> Stream audio                  |
|  +-- list_voices() -> Vec<ResembleVoice>                 |
+---------------------------------------------------------+
|  ResembleRequestBuilder (TTSRequestBuilder)              |
|  +-- build_http_request() -> POST /stream                |
+---------------------------------------------------------+
```

### File Structure

```
src/core/tts/resemble/
+-- mod.rs          # Module exports, constants
+-- config.rs       # ResembleTtsConfig, enums
+-- provider.rs     # ResembleTts, ResembleRequestBuilder
```

### Constants

```rust
pub const RESEMBLE_TTS_SYNC_URL: &str = "https://f.cluster.resemble.ai/synthesize";
pub const RESEMBLE_TTS_STREAM_URL: &str = "https://f.cluster.resemble.ai/stream";
pub const RESEMBLE_VOICES_URL: &str = "https://app.resemble.ai/api/v2/voices";
pub const RESEMBLE_WS_URL: &str = "wss://websocket.cluster.resemble.ai/stream";
pub const MAX_TEXT_LENGTH_SYNC: usize = 3000;
pub const MAX_TEXT_LENGTH_STREAM: usize = 2000;
```

### Configuration

```rust
pub struct ResembleTtsConfig {
    pub api_key: String,
    pub voice_uuid: String,
    pub model: ResembleModel,
    pub output_format: ResembleOutputFormat,
    pub precision: ResemblePrecision,
    pub sample_rate: u32,
    pub use_hd: bool,
}

pub enum ResembleModel {
    Chatterbox,
    ChatterboxTurbo,
    ChatterboxMultilingual,
}

pub enum ResembleOutputFormat {
    Wav,
    Mp3,
}

pub enum ResemblePrecision {
    PCM32,
    PCM24,
    PCM16,
    Mulaw,
}
```

### Request Body

```rust
#[derive(Serialize)]
pub struct ResembleStreamRequest {
    pub voice_uuid: String,
    pub data: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_uuid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub precision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub use_hd: Option<bool>,
}
```

---

## Testing Plan

### Unit Tests

1. `test_config_defaults` - Verify default model and settings
2. `test_config_validation` - Empty API key/voice_uuid fails
3. `test_request_builder_headers` - Bearer token header set correctly
4. `test_stream_request_serialization` - JSON body structure
5. `test_model_enum` - Model string conversion
6. `test_text_length_validation` - Max 2000/3000 chars

### Integration Tests

1. `test_resemble_connection` - Connect/disconnect cycle
2. `test_resemble_list_voices` - Fetch voice list
3. `test_resemble_synthesis` - Generate audio from text
4. `test_resemble_turbo_model` - Low-latency synthesis

---

## Error Handling

| HTTP Status | Meaning | Action |
|-------------|---------|--------|
| 200 | Success | Process audio |
| 400 | Bad Request | Invalid parameters |
| 401 | Unauthorized | Invalid API key |
| 403 | Forbidden | Plan limits |
| 429 | Rate Limited | Retry with backoff |
| 500 | Server Error | Retry |

---

## Quality Gates

- [ ] `cargo fmt --check` passes
- [ ] `cargo clippy -- -D warnings` passes
- [ ] All unit tests pass
- [ ] Integration tests pass (with credentials)
- [ ] Factory registration complete
- [ ] Provider aliases registered
- [ ] Documentation updated
- [ ] Environment variable documented

---

## References

- [Resemble AI Documentation](https://docs.resemble.ai)
- [TTS API Reference](https://docs.resemble.ai/voice-generation/text-to-speech)
- [Streaming HTTP API](https://docs.resemble.ai/api-reference/text-to-speech/stream-synthesize)
- [WebSocket Streaming](https://docs.resemble.ai/voice-generation/text-to-speech/streaming-websocket)
- [Voices API](https://docs.resemble.ai/api-reference/voices/list-voices)
- [Chatterbox (Open Source)](https://github.com/resemble-ai/chatterbox)
