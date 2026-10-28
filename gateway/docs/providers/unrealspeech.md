# Unreal Speech TTS Provider Integration

> **Status:** IN_PROGRESS
> **Research Date:** 2026-01-13
> **Provider Type:** TTS

---

## 1. Provider Overview

### Basic Information
- **Website:** https://unrealspeech.com
- **API Documentation:** https://docs.unrealspeech.com
- **API Documentation (V8):** https://docs.v8.unrealspeech.com
- **GitHub SDK:** https://github.com/unrealspeech/unrealspeech
- **Pricing:** https://unrealspeech.com/pricing

### Capabilities Matrix
| Capability | Supported | Notes |
|------------|-----------|-------|
| TTS | YES | HTTP streaming with instant response |
| STT | NO | Not provided |
| Voice Cloning | NO | Not provided |
| Streaming | YES | `/stream` endpoint with 300ms latency |
| SSML | NO | Plain text only |

### Technical Specifications
- **Authentication:** Bearer Token via `Authorization` header
- **Protocol:** REST (HTTP POST)
- **Audio Formats:** MP3 (libmp3lame), PCM mu-law (pcm_mulaw)
- **Bitrates:** 16k, 32k, 48k, 64k, 128k, 192k (default), 256k, 320k
- **Languages:** American English, Chinese, Spanish, French, Hindi, Italian, Portuguese
- **Latency:** ~300ms TTFA for `/stream` endpoint

---

## 2. API Endpoints

### 2.1 Stream (Fast)
**Endpoint:** `POST https://api.v8.unrealspeech.com/stream`

**Purpose:** Short, time-sensitive cases (chatbot, real-time)

**Headers:**
```
Authorization: Bearer <api-token>
Content-Type: application/json
```

**Request Body:**
```json
{
  "Text": "Hello, this is a test.",
  "VoiceId": "Sierra",
  "Bitrate": "192k",
  "Speed": 0,
  "Pitch": 1.0,
  "Codec": "libmp3lame"
}
```

**Parameters:**
| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| Text | string | Yes | Text to synthesize (max 1,000 chars) |
| VoiceId | string | Yes | Voice identifier |
| Bitrate | string | No | Audio bitrate (default: 192k) |
| Speed | float | No | Speech speed -1.0 to 1.0 (default: 0) |
| Pitch | float | No | Voice pitch 0.5 to 1.5 (default: 1.0) |
| Codec | string | No | Audio codec (libmp3lame or pcm_mulaw) |

**Response:** Streaming `audio/mpeg` (raw MP3 data)

**Characteristics:**
- Max 1,000 characters
- Synchronous, instant response (~300ms)
- Streams back raw audio data

### 2.2 Speech (Medium)
**Endpoint:** `POST https://api.v8.unrealspeech.com/speech`

**Purpose:** Medium-length text (up to 3,000 characters)

**Request Body:**
```json
{
  "Text": "Longer text content here...",
  "VoiceId": "Daniel",
  "Bitrate": "192k",
  "Speed": 0,
  "Pitch": 1.0,
  "TimestampType": "word"
}
```

**Additional Parameters:**
| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| TimestampType | string | No | "word" or "sentence" for timestamps |

**Response:**
```json
{
  "OutputUri": "https://...",
  "TimestampsUri": "https://..."
}
```

**Characteristics:**
- Max 3,000 characters
- Synchronous, ~1s per 700 characters
- Returns MP3 URL and JSON timestamp URL

### 2.3 Synthesis Tasks (Long)
**Endpoint:** `POST https://api.v8.unrealspeech.com/synthesisTasks`

**Purpose:** Long-form content (audiobooks, etc.)

**Request Body:**
```json
{
  "Text": "Very long text content...",
  "VoiceId": "Noah",
  "Bitrate": "320k",
  "Speed": 0,
  "Pitch": 1.0,
  "TimestampType": "word"
}
```

**Response:**
```json
{
  "TaskId": "task_abc123xyz"
}
```

**Check Status:**
`GET https://api.v8.unrealspeech.com/synthesisTasks/{TaskId}`

**Characteristics:**
- Max 500,000 characters
- Asynchronous, ~1s per 800 characters
- Returns TaskId to poll for completion
- Supports up to 10 hours of audio per request

### 2.4 Stream With Timestamps
**Endpoint:** `POST https://api.v8.unrealspeech.com/streamWithTimestamps`

**Purpose:** Real-time word-level timestamps for highlighting

**Characteristics:**
- WebSocket connection for streaming
- Audio with precise word timing
- Perfect for word-by-word highlighting

---

## 3. Voices

The V8 API documents 38 voices (docs.v8.unrealspeech.com, `VoiceId`). It has no voice-list
endpoint, so `GET /voices` lists these from the gateway's table (`V8_VOICES` in `config.rs`).
The default is **Sierra**, the voice Unreal's own examples use.

| Language | Female | Male |
|----------|--------|------|
| American English | Autumn, Melody, Hannah, Emily, Ivy, Kaitlyn, Luna, Willow, Lauren, Sierra | Noah, Jasper, Caleb, Ronan, Ethan, Daniel, Zane |
| Chinese | Mei, Lian, Ting, Jing | Wei, Jian, Hao, Sheng |
| Spanish | Lucía | Mateo, Javier |
| French | Élodie | |
| Hindi | Ananya, Priya | Arjun, Rohan |
| Italian | Giulia | Luca |
| Portuguese | Camila | Thiago, Rafael |

A documented name is matched without regard to case (`sierra` is sent as `Sierra`; `lucia` and
`elodie` as `Lucía` and `Élodie`). Any other id, such as a custom voice, is sent exactly as given.

### V7 voice ids

V8 renamed every voice ("Migrating from V6 or V7: update the VoiceID"). A V7 id is sent as a
V8 voice of the same gender, and the gateway logs a warning:

| V7 | Sent as |
|----|---------|
| Scarlett | Sierra |
| Liv | Hannah |
| Amy | Emily |
| Dan | Noah |
| Will | Daniel |

The Kokoro-style ids this provider used to list (`af_bella`, `bm_george`, …) and their short names
(`bella`, `george`, …) are still sent as before. They are not in Unreal's V8 list.

---

## 4. Rate Limits

| Plan | Requests/Second |
|------|-----------------|
| Free | 1-2 |
| Basic | 2-8 |
| Pro | 8-32 |

---

## 5. Pricing

| Plan | Price | Characters | Audio Hours | Overage Rate |
|------|-------|------------|-------------|--------------|
| Free | $0 | 1,000,000 | ~22 hours | N/A |
| Basic | $49/mo | - | - | $16/1M chars |
| Plus | $499/mo | 42,000,000 | ~933 hours | $12/1M chars |
| Pro | $1,499/mo | 150,000,000 | ~3,000 hours | $10/1M chars |
| Enterprise | $4,999/mo | 625,000,000 | ~14,000 hours | $8/1M chars |

**Note:** 1 minute of audio ≈ 750 characters (~150 words/minute)

### Cost Comparison
- **11x cheaper** than ElevenLabs
- **10x cheaper** than Play.ht
- **2x cheaper** than Amazon, Microsoft, Google

### Free Tier Requirements
- Must attribute Unreal Speech with link to unrealspeech.com
- Characters reset on 1st of every month
- Paid plans: No attribution required

---

## 6. Implementation Plan

### 6.1 Module Structure
```
src/core/tts/unrealspeech/
├── mod.rs           # Module exports and constants
├── config.rs        # UnrealSpeechTtsConfig, voices, codecs
└── provider.rs      # UnrealSpeechTts implementing BaseTTS
```

### 6.2 Implementation Steps

1. **Create mod.rs**
   - API endpoint constants
   - Module exports

2. **Create config.rs**
   - UnrealSpeechVoice (V8 voice table, V7 migration)
   - UnrealSpeechCodec enum
   - UnrealSpeechTtsConfig struct
   - UnrealSpeechStreamRequest struct

3. **Create provider.rs**
   - UnrealSpeechRequestBuilder implementing TTSRequestBuilder
   - UnrealSpeechTts implementing BaseTTS
   - HTTP streaming via reqwest

4. **Update plugin system**
   - Add to plugin/builtin/mod.rs
   - Register with inventory::submit!

### 6.3 Configuration Mapping

| TTSConfig Field | Unreal Speech Mapping |
|-----------------|----------------------|
| api_key | Authorization: Bearer header |
| voice_id | VoiceId |
| audio_format | Codec (libmp3lame or pcm_mulaw) |

### 6.4 Provider-Specific Options
- `bitrate`: Audio bitrate (16k-320k)
- `speed`: Speech speed (-1.0 to 1.0)
- `pitch`: Voice pitch (0.5 to 1.5)

---

## 7. Testing Plan

### 7.1 Unit Tests
- Config parsing and validation
- Voice enum serialization
- Codec enum serialization
- Request body construction
- Response handling

### 7.2 Integration Tests (with credentials)
- Connection and basic synthesis
- Streaming audio chunks
- Different voices
- Speed/pitch modulation
- Error responses (rate limit, invalid text)

### 7.3 Test Cases
```rust
#[test]
fn test_unrealspeech_config_defaults()
#[test]
fn test_unrealspeech_voice_serialization()
#[test]
fn test_unrealspeech_codec_serialization()
#[test]
fn test_request_body_construction()
#[tokio::test]
async fn test_unrealspeech_connect()
#[tokio::test]
async fn test_unrealspeech_speak_basic()
```

---

## 8. Best Practices

### Performance
- Use `/stream` endpoint for lowest latency (~300ms)
- Use higher bitrate (320k) for quality, lower (64k) for bandwidth
- Cache common phrases for repeated use

### Cost Optimization
- Use free tier for testing (250K chars/month)
- Monitor character usage
- Batch text efficiently

### Security
- Never expose API token in client-side code
- Use server-side proxy for API calls
- Rotate tokens periodically

---

## 9. Error Handling

### HTTP Errors
| Status | Meaning | Action |
|--------|---------|--------|
| 400 | Bad Request | Check text length and parameters |
| 401 | Unauthorized | Verify API token |
| 429 | Rate Limited | Back off, respect rate limits |
| 500 | Server Error | Retry with exponential backoff |

---

## 10. References

- [Unreal Speech Main Site](https://unrealspeech.com)
- [API Documentation](https://docs.unrealspeech.com/reference/getting-started-with-our-api)
- [API Documentation V8](https://docs.v8.unrealspeech.com/)
- [Python SDK](https://github.com/unrealspeech/unrealspeech)
- [Pricing](https://unrealspeech.com/pricing)
