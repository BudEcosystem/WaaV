# Rev AI STT Provider Integration

> **Status:** COMPLETE
> **Implementation Date:** 2026-01-13
> **Provider Type:** STT

---

## 1. Provider Overview

### Basic Information
- **Website:** https://www.rev.ai
- **API Documentation:** https://docs.rev.ai
- **Streaming API Docs:** https://docs.rev.ai/api/streaming/
- **Requests Docs:** https://docs.rev.ai/api/streaming/requests/
- **Responses Docs:** https://docs.rev.ai/api/streaming/responses/
- **Pricing:** https://www.rev.ai/pricing
- **Languages:** https://docs.rev.ai/faq/

### Capabilities Matrix
| Capability | Supported | Notes |
|------------|-----------|-------|
| STT | YES | WebSocket streaming |
| TTS | NO | Not provided |
| Voice Cloning | NO | Not provided |
| Streaming | YES | WebSocket protocol |
| Speaker Detection | YES | machine_v2 model only |
| Custom Vocabulary | YES | Via custom_vocabulary_id |
| Profanity Filter | YES | filter_profanity parameter |
| Disfluency Removal | YES | remove_disfluencies parameter |

### Technical Specifications
- **Authentication:** API Key via query parameter `access_token`
- **Protocol:** WebSocket (wss://)
- **Endpoint:** `wss://api.rev.ai/speechtotext/v1/stream`
- **Audio Formats:** raw (PCM), FLAC, WAV
- **Sample Rates:** 8000-48000 Hz
- **Channels:** 1-10
- **Max Stream Duration:** 3 hours
- **Concurrency Limit:** 10 (configurable via support)

---

## 2. STT API (WebSocket Streaming)

### 2.1 Single-Step WebSocket Connection

Rev AI uses direct WebSocket connection with all parameters in the URL:

**Endpoint:**
```
wss://api.rev.ai/speechtotext/v1/stream
```

**Required Query Parameters:**
| Parameter | Type | Description |
|-----------|------|-------------|
| access_token | string | Rev AI API access token |
| content_type | string | Audio format specification |

**Content Type for Raw Audio:**
```
audio/x-raw;layout=interleaved;rate=16000;format=S16LE;channels=1
```

**Content Type Components:**
| Component | Values | Description |
|-----------|--------|-------------|
| layout | interleaved, non-interleaved | Audio channel layout |
| rate | 8000-48000 | Sample rate in Hz |
| format | S16LE, S32LE, F32LE, etc. | Sample format |
| channels | 1-10 | Number of audio channels |

**Sample Format Values:**
| Format | Description |
|--------|-------------|
| S16LE | Signed 16-bit little-endian (most common) |
| S32LE | Signed 32-bit little-endian |
| F32LE | 32-bit float little-endian |
| S16BE | Signed 16-bit big-endian |

### 2.2 Optional Query Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| language | string | en | Language code (en, es, fr, de, pt, cmn, ja, ru, ar, hi) |
| metadata | string | null | Custom metadata string |
| custom_vocabulary_id | string | null | ID of custom vocabulary to use |
| filter_profanity | boolean | false | Replace profanity with asterisks |
| remove_disfluencies | boolean | false | Remove ums, uhs, etc. |
| delete_after_seconds | integer | null | Auto-delete transcript after N seconds |
| detailed_partials | boolean | false | Include timestamps in partial results |
| start_ts | double | 0.0 | Starting timestamp offset |
| max_segment_duration_seconds | integer | null | Max segment length |
| transcriber | string | machine | machine, machine_v2, or human |
| enable_speaker_switch | boolean | false | Enable speaker detection (machine_v2 only) |
| skip_postprocessing | boolean | false | Skip text normalization |

### 2.3 Full URL Example

```
wss://api.rev.ai/speechtotext/v1/stream?access_token=YOUR_TOKEN&content_type=audio/x-raw;layout=interleaved;rate=16000;format=S16LE;channels=1&language=en&filter_profanity=true
```

---

## 3. WebSocket Message Protocol

### 3.1 Connection Flow

1. Client opens WebSocket with URL containing all parameters
2. Server sends `connected` message with session ID
3. Client sends binary audio data frames
4. Server sends `partial` and `final` transcript messages
5. Client sends "EOS" text message to end stream
6. Server sends final results and closes connection

### 3.2 Connected Message (Server -> Client)

```json
{
  "type": "connected",
  "id": "s1d24ax2fd21"
}
```

| Field | Type | Description |
|-------|------|-------------|
| type | string | Always "connected" |
| id | string | Unique session identifier |

### 3.3 Partial Transcript (Server -> Client)

```json
{
  "type": "partial",
  "ts": 1.01,
  "end_ts": 1.55,
  "elements": [
    {"type": "text", "value": "one"}
  ]
}
```

| Field | Type | Description |
|-------|------|-------------|
| type | string | "partial" |
| ts | double | Start timestamp in seconds |
| end_ts | double | End timestamp in seconds |
| elements | array | Partial word elements |

### 3.4 Final Transcript (Server -> Client)

```json
{
  "type": "final",
  "ts": 1.01,
  "end_ts": 3.2,
  "elements": [
    {"type": "text", "value": "One", "ts": 1.04, "end_ts": 1.55, "confidence": 1.0},
    {"type": "punct", "value": " "},
    {"type": "text", "value": "two", "ts": 1.84, "end_ts": 2.15, "confidence": 1.0},
    {"type": "punct", "value": "."}
  ]
}
```

### 3.5 Element Types

| Type | Fields | Description |
|------|--------|-------------|
| text | value, ts, end_ts, confidence | Recognized word |
| punct | value | Punctuation or whitespace |

**Element Fields:**
| Field | Type | Description |
|-------|------|-------------|
| type | string | "text" or "punct" |
| value | string | Word or punctuation |
| ts | double | Word start time (text only) |
| end_ts | double | Word end time (text only) |
| confidence | double | Recognition confidence 0-1 (text only) |

### 3.6 Audio Data (Client -> Server)

Send raw audio as binary WebSocket frames. Recommended chunk size: 250ms or more.

### 3.7 End of Stream (Client -> Server)

Send text message "EOS" to gracefully close the stream and receive final hypothesis.

---

## 4. Supported Languages

### Streaming Languages (9+)
| Language | Code |
|----------|------|
| English | en |
| Spanish | es |
| French | fr |
| German | de |
| Portuguese (Brazil) | pt |
| Mandarin Chinese | cmn |
| Japanese | ja |
| Russian | ru |
| Arabic | ar |
| Hindi | hi |

Note: Async API supports 58+ languages. Streaming has limited support.

---

## 5. WebSocket Close Codes

| Code | Name | Retryable | Description |
|------|------|-----------|-------------|
| 1000 | Normal | - | Normal closure |
| 4001 | Unauthorized | No | Invalid or missing access token |
| 4002 | Bad Request | No | Invalid content_type, metadata too long, or invalid custom vocabulary |
| 4003 | Insufficient Credits | No | Not enough credits (requires 10 min hold) |
| 4010 | Server Shutting Down | Yes | Server restart, retry connection |
| 4013 | No Instance Available | Yes | No streaming instances, retry connection |
| 4029 | Too Many Requests | No | Exceeded concurrency limit |

### Error Handling Recommendations
- Maximum 5 retry attempts per request
- Only retry for codes 4010 and 4013
- Implement exponential backoff

---

## 6. Rate Limits & Quotas

### Concurrency Limits
| Limit | Default | Notes |
|-------|---------|-------|
| Concurrent Streams | 10 | Configurable via support |
| Max Stream Duration | 3 hours | Per connection |
| Max Connection Wait | 60 seconds | Timeout for instance |

### Credit System
- 10 minutes credit hold on connection
- Billed for greater of: stream duration or audio duration
- Minimum charge: 15 seconds
- Rounded up to nearest second

---

## 7. Pricing

### Transcription Rates
| Service | Price |
|---------|-------|
| Reverb (default) | $0.20/hour |
| Reverb Turbo | $0.10/hour |
| Reverb Foreign Language | $0.30/hour |
| Whisper Models | $0.005/minute |
| Human Transcription | $1.99/minute |

### Free Tier
- 5 hours free credit (Reverb ASR)

### Enterprise
- Volume-based pricing
- Dedicated account manager
- Priority support

---

## 8. Implementation Plan

### 8.1 Module Structure
```
src/core/stt/revai/
├── mod.rs           # Module exports and constants
├── config.rs        # RevAISTTConfig
├── messages.rs      # WebSocket message types
└── client.rs        # RevAISTT implementing BaseSTT
```

### 8.2 Implementation Steps

1. **Create config.rs**
   - RevAISampleFormat enum (S16LE, S32LE, F32LE, S16BE)
   - RevAITranscriber enum (Machine, MachineV2, Human)
   - RevAISTTConfig struct

2. **Create messages.rs**
   - ConnectedMessage struct
   - PartialTranscript struct
   - FinalTranscript struct
   - TranscriptElement struct
   - ServerMessage enum

3. **Create client.rs**
   - RevAISTT implementing BaseSTT
   - Single-step WebSocket connection
   - URL construction with query parameters
   - Binary audio sending
   - Partial and final transcript handling

4. **Update plugin system**
   - Add to plugin/builtin/mod.rs
   - Register STT factory

### 8.3 Configuration Mapping

| STTConfig Field | Rev AI Mapping |
|-----------------|----------------|
| api_key | access_token query param |
| language | language query param |
| sample_rate | rate in content_type |
| encoding | format in content_type |
| channels | channels in content_type |

---

## 9. Testing Plan

### 9.1 Unit Tests
- Config parsing and URL construction
- Sample format enum serialization
- Message deserialization
- Content-type string construction

### 9.2 Integration Tests (with credentials)
- WebSocket connection establishment
- Connected message receipt
- Real-time transcription
- Partial and final transcript handling
- Graceful EOS termination

### 9.3 Test Cases
```rust
#[test]
fn test_revai_stt_config_defaults()
#[test]
fn test_revai_sample_format_serialization()
#[test]
fn test_revai_content_type_construction()
#[test]
fn test_revai_url_construction()
#[test]
fn test_revai_message_parsing()
#[tokio::test]
async fn test_revai_stt_connect()
#[tokio::test]
async fn test_revai_transcript_parsing()
```

---

## 10. References

- [Rev AI Main Site](https://www.rev.ai)
- [API Documentation](https://docs.rev.ai)
- [Streaming Overview](https://docs.rev.ai/api/streaming/)
- [Streaming Requests](https://docs.rev.ai/api/streaming/requests/)
- [Streaming Responses](https://docs.rev.ai/api/streaming/responses/)
- [Example Session](https://docs.rev.ai/api/streaming/example-session/)
- [Billing](https://docs.rev.ai/api/streaming/billing/)
- [Error Recovery](https://docs.rev.ai/resources/tutorials/recover-connection-streaming-api/)
- [Pricing](https://www.rev.ai/pricing)
