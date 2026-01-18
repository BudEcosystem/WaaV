# Gladia STT Provider Integration

> **Status:** COMPLETE
> **Implementation Date:** 2026-01-13
> **Provider Type:** STT

---

## 1. Provider Overview

### Basic Information
- **Website:** https://www.gladia.io
- **API Documentation:** https://docs.gladia.io
- **Live STT Docs:** https://docs.gladia.io/api-reference/live-flow
- **Session Init Docs:** https://docs.gladia.io/api-reference/v2/live/init
- **Pricing:** https://www.gladia.io/pricing
- **Languages:** https://docs.gladia.io/chapters/limits-and-specifications/languages

### Capabilities Matrix
| Capability | Supported | Notes |
|------------|-----------|-------|
| STT | YES | WebSocket streaming, 100+ languages |
| TTS | NO | Not provided |
| Voice Cloning | NO | Not provided |
| Streaming | YES | WebSocket with <300ms latency |
| Translation | YES | 99 target languages |
| Diarization | YES | Speaker identification |
| Code-Switching | YES | Automatic multi-language detection |

### Technical Specifications
- **Authentication:** API Key via `X-Gladia-Key` header
- **Protocol:** REST init + WebSocket streaming
- **Model:** solaria-1 (default, only option)
- **Audio Formats:** wav/pcm, wav/alaw, wav/ulaw
- **Sample Rates:** 8000, 16000, 32000, 44100, 48000 Hz
- **Bit Depths:** 8, 16, 24, 32 bits
- **Channels:** 1-8
- **Latency:** <300ms partial, ~700ms final for 3s utterance

---

## 2. STT API (WebSocket Streaming)

### 2.1 Two-Step Connection Process

**Step 1: Initialize Session (REST)**
```
POST https://api.gladia.io/v2/live
```

**Headers:**
```
X-Gladia-Key: <api-key>
Content-Type: application/json
```

**Query Parameters:**
| Parameter | Values | Description |
|-----------|--------|-------------|
| region | us-west, eu-west | Server region (optional) |

**Request Body:**
```json
{
  "encoding": "wav/pcm",
  "sample_rate": 16000,
  "bit_depth": 16,
  "channels": 1,
  "model": "solaria-1",
  "endpointing": 0.05,
  "maximum_duration_without_endpointing": 5,
  "language_config": {
    "languages": ["en"],
    "code_switching": false
  },
  "pre_processing": {
    "audio_enhancer": false,
    "speech_threshold": 0.5
  },
  "realtime_processing": {
    "words_accurate_timestamps": true
  },
  "messages_config": {
    "receive_partial_transcripts": true,
    "receive_final_transcripts": true,
    "receive_speech_events": false,
    "receive_pre_processing_events": false,
    "receive_realtime_processing_events": false,
    "receive_post_processing_events": false
  }
}
```

**Response (201 Created):**
```json
{
  "id": "45463597-20b7-4af7-b3b3-f5fb778203ab",
  "created_at": "2023-12-28T09:04:17.210Z",
  "url": "wss://api.gladia.io/v2/live?token=4a39145c-2844-4557-8f34-34883f7be7d9"
}
```

**Step 2: Connect to WebSocket**
Use the `url` from the response to establish WebSocket connection.

### 2.2 Audio Configuration Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| encoding | string | wav/pcm | Audio encoding (wav/pcm, wav/alaw, wav/ulaw) |
| bit_depth | number | 16 | Bit depth (8, 16, 24, 32) |
| sample_rate | number | 16000 | Sample rate in Hz |
| channels | integer | 1 | Number of audio channels (1-8) |

### 2.3 Session Configuration Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| model | string | solaria-1 | Speech recognition model |
| endpointing | number | 0.05 | Silence duration to end utterance (0.01-10 seconds) |
| maximum_duration_without_endpointing | number | 5 | Force end utterance after this duration (5-60 seconds) |
| custom_metadata | object | null | Key-value pairs for session tracking |

### 2.4 Language Configuration

| Parameter | Type | Description |
|-----------|------|-------------|
| languages | array | ISO 639-1 language codes (e.g., ["en", "es"]) |
| code_switching | boolean | Enable automatic language switching |

### 2.5 Processing Configuration

**Pre-processing:**
| Parameter | Type | Description |
|-----------|------|-------------|
| audio_enhancer | boolean | Enable audio enhancement |
| speech_threshold | number | Speech detection threshold (0-1) |

**Realtime Processing:**
| Parameter | Type | Description |
|-----------|------|-------------|
| words_accurate_timestamps | boolean | Enable word-level timestamps |
| custom_vocabulary | array | Custom words/phrases |
| translation | boolean | Enable real-time translation |
| translation_config.target_languages | array | Target language codes |
| named_entity_recognition | boolean | Enable NER |
| sentiment_analysis | boolean | Enable sentiment analysis |

**Post-processing:**
| Parameter | Type | Description |
|-----------|------|-------------|
| summarization | boolean | Enable summarization |
| summarization_config.type | string | Summarization type |
| chapterization | boolean | Enable chapter detection |

### 2.6 Message Types Configuration

| Parameter | Default | Description |
|-----------|---------|-------------|
| receive_partial_transcripts | true | Receive interim results |
| receive_final_transcripts | true | Receive finalized transcripts |
| receive_speech_events | false | Receive speech start/end events |
| receive_pre_processing_events | false | Receive pre-processing events |
| receive_realtime_processing_events | false | Receive realtime processing events |
| receive_post_processing_events | false | Receive post-processing events |

---

## 3. WebSocket Message Protocol

### 3.1 Sending Audio (Client -> Server)

**Option A: Binary Frame**
Send raw audio bytes directly as WebSocket binary frame.

**Option B: JSON Message**
```json
{
  "type": "audio_chunk",
  "data": {
    "chunk": "<base64-encoded-audio>"
  }
}
```

### 3.2 Stop Recording (Client -> Server)
```json
{
  "type": "stop_recording"
}
```
Or close WebSocket with code 1000.

### 3.3 Transcript Message (Server -> Client)
```json
{
  "type": "transcript",
  "session_id": "4a39145c-2844-4557-8f34-34883f7be7d9",
  "created_at": "2021-09-01T12:00:00.123Z",
  "data": {
    "id": "00-00000011",
    "is_final": true,
    "utterance": {
      "text": "Hello world",
      "language": "en",
      "start": 0.0,
      "end": 1.5,
      "confidence": 0.95,
      "channel": 0,
      "speaker": 1,
      "words": [
        {
          "word": "Hello",
          "start": 0.0,
          "end": 0.5,
          "confidence": 0.98
        },
        {
          "word": "world",
          "start": 0.6,
          "end": 1.5,
          "confidence": 0.92
        }
      ]
    }
  }
}
```

### 3.4 Transcript Fields

**Root Level:**
| Field | Type | Description |
|-------|------|-------------|
| type | string | Always "transcript" |
| session_id | string | Unique session identifier |
| created_at | string | ISO 8601 timestamp |
| data | object | Transcript data container |

**Data Object:**
| Field | Type | Description |
|-------|------|-------------|
| id | string | Utterance identifier (e.g., "00-00000011") |
| is_final | boolean | true for final, false for partial |

**Utterance Object:**
| Field | Type | Description |
|-------|------|-------------|
| text | string | Full transcribed text |
| language | string | ISO 639-1 language code |
| start | number | Start timestamp in seconds |
| end | number | End timestamp in seconds |
| confidence | number | Confidence score (0-1) |
| channel | integer | Audio channel (0-indexed) |
| speaker | integer | Speaker ID (when diarization enabled) |
| words | array | Word-level details |

**Word Object:**
| Field | Type | Description |
|-------|------|-------------|
| word | string | Individual word |
| start | number | Word start time (seconds) |
| end | number | Word end time (seconds) |
| confidence | number | Word confidence (0-1) |

---

## 4. Supported Languages

Gladia supports 100+ languages using ISO 639-1 codes (2-letter) or ISO 639-3 (3-letter when no 639-1 exists).

### Major Languages
| Language | Code | Language | Code |
|----------|------|----------|------|
| English | en | Japanese | ja |
| Spanish | es | Korean | ko |
| French | fr | Portuguese | pt |
| German | de | Russian | ru |
| Chinese | zh | Arabic | ar |
| Italian | it | Hindi | hi |

### Regional Languages
| Language | Code | Language | Code |
|----------|------|----------|------|
| Dutch | nl | Thai | th |
| Polish | pl | Vietnamese | vi |
| Swedish | sv | Indonesian | id |
| Norwegian | no | Tagalog | tl |
| Danish | da | Hebrew | he |
| Finnish | fi | Persian | fa |
| Czech | cs | Turkish | tr |
| Hungarian | hu | Ukrainian | uk |
| Romanian | ro | Bengali | bn |
| Greek | el | Tamil | ta |

### Special Features
- **Automatic Detection:** Use empty `languages` array or omit for auto-detection
- **Code-Switching:** Enable `code_switching: true` for multilingual conversations
- **Translation:** Translate to 99 target languages in real-time

---

## 5. Rate Limits & Concurrency

### Concurrency Limits
| Plan | Pre-recorded | Live |
|------|-------------|------|
| Free | 3 | 1 |
| Self-Serve (Paid) | 25 | 30 |
| Enterprise | On demand | On demand |

### Usage Limits
| Plan | Monthly Limit |
|------|---------------|
| Free | 10 hours |
| Paid | Unlimited |

### Queue Capacity
- Paid plan users can queue up to 300 async requests
- Maximum 25 processed concurrently

---

## 6. Pricing

### Self-Serve Plan
| Type | Price |
|------|-------|
| Real-time | $0.75/hour |
| Async | $0.61/hour + 10h free |

### Scaling Plan
| Type | Price |
|------|-------|
| Real-time | $0.55/hour |
| Async | $0.50/hour |

### Enterprise
- Custom pricing
- Unlimited concurrent requests
- Zero data retention
- SLAs and premium support

### Included Features (All Plans)
- Automatic language detection
- Speaker diarization
- 100+ languages
- GDPR, HIPAA, SOC 2 Type 2 compliance

---

## 7. Implementation Plan

### 7.1 Module Structure
```
src/core/stt/gladia/
├── mod.rs           # Module exports and constants
├── config.rs        # GladiaSTTConfig
├── messages.rs      # WebSocket message types
└── client.rs        # GladiaSTT implementing BaseSTT
```

### 7.2 Implementation Steps

1. **Create config.rs**
   - GladiaRegion enum (UsWest, EuWest)
   - GladiaEncoding enum (WavPcm, WavAlaw, WavUlaw)
   - GladiaBitDepth enum (Bit8, Bit16, Bit24, Bit32)
   - GladiaLanguageConfig struct
   - GladiaSTTConfig struct

2. **Create messages.rs**
   - InitSessionRequest/Response structs
   - AudioChunkMessage struct
   - StopRecordingMessage struct
   - TranscriptMessage struct
   - UtteranceData struct
   - WordData struct

3. **Create client.rs**
   - GladiaSTT implementing BaseSTT
   - Two-step connection (REST init + WebSocket)
   - Audio chunk sending (binary or JSON)
   - Partial and final transcript handling

4. **Update plugin system**
   - Add to plugin/builtin/mod.rs
   - Register STT factory

### 7.3 Configuration Mapping

| STTConfig Field | Gladia Mapping |
|-----------------|----------------|
| api_key | X-Gladia-Key header |
| language | language_config.languages[0] |
| sample_rate | sample_rate |
| encoding | encoding (mapped from STT encoding enum) |

---

## 8. Error Handling

### HTTP Errors (Session Init)
| Status | Meaning |
|--------|---------|
| 400 | Malformed request (validation errors) |
| 401 | Missing or invalid API key |
| 422 | Invalid parameter values |

### WebSocket Close Codes
| Code | Meaning |
|------|---------|
| 1000 | Normal closure |
| 1008 | Policy violation |
| 1011 | Server error |

---

## 9. Testing Plan

### 9.1 Unit Tests
- Config parsing and validation
- Region/encoding enum serialization
- Message serialization/deserialization
- Init request body construction

### 9.2 Integration Tests (with credentials)
- Session initialization
- WebSocket connection
- Real-time transcription
- Partial and final transcript handling

### 9.3 Test Cases
```rust
#[test]
fn test_gladia_stt_config_defaults()
#[test]
fn test_gladia_region_serialization()
#[test]
fn test_gladia_encoding_serialization()
#[test]
fn test_gladia_message_parsing()
#[test]
fn test_gladia_init_request_construction()
#[tokio::test]
async fn test_gladia_session_init()
#[tokio::test]
async fn test_gladia_stt_connect()
#[tokio::test]
async fn test_gladia_transcript_parsing()
```

---

## 10. References

- [Gladia Main Site](https://www.gladia.io)
- [API Documentation](https://docs.gladia.io)
- [Live Workflow](https://docs.gladia.io/api-reference/live-flow)
- [Session Init](https://docs.gladia.io/api-reference/v2/live/init)
- [Transcript Message](https://docs.gladia.io/api-reference/v2/live/message/transcript)
- [Supported Languages](https://docs.gladia.io/chapters/limits-and-specifications/languages)
- [Rate Limits](https://docs.gladia.io/chapters/limits-and-specifications/concurrency)
- [Pricing](https://www.gladia.io/pricing)
- [V1 to V2 Migration](https://docs.gladia.io/chapters/live-stt/migration-from-v1)
- [LiveKit Plugin](https://docs.livekit.io/reference/python/v1/livekit/plugins/gladia/index.html)
