# Speechmatics STT + TTS Provider Integration

> **Status:** COMPLETE
> **Implementation Date:** 2026-01-13
> **Provider Type:** STT + TTS

---

## 1. Provider Overview

### Basic Information
- **Website:** https://www.speechmatics.com
- **API Documentation:** https://docs.speechmatics.com
- **STT Realtime Docs:** https://docs.speechmatics.com/api-ref/realtime-transcription-websocket
- **TTS Docs:** https://docs.speechmatics.com/text-to-speech/quickstart
- **Pricing:** https://www.speechmatics.com/pricing

### Capabilities Matrix
| Capability | Supported | Notes |
|------------|-----------|-------|
| STT | YES | WebSocket streaming, 55+ languages |
| TTS | YES | HTTP streaming, 4 English voices (preview) |
| Voice Cloning | NO | Not provided |
| Streaming | YES | WebSocket STT, HTTP chunked TTS |
| SSML | NO | Natural prosody from text |

### Technical Specifications
- **Authentication:** Bearer Token (API Key or JWT)
- **STT Protocol:** WebSocket
- **TTS Protocol:** REST HTTP
- **Audio Formats:** PCM (f32le, s16le, mulaw), WAV, MP3, AAC, OGG, FLAC
- **Languages:** 55+ (STT), English only (TTS preview)
- **Latency:** <1s STT, <200ms TTS first chunk

---

## 2. STT API (WebSocket Streaming)

### 2.1 Connection
**Endpoint (EU):** `wss://eu.rt.speechmatics.com/v2`
**Endpoint (US):** `wss://us.rt.speechmatics.com/v2`

**Server-Side Authentication:**
```
Authorization: Bearer <api-key>
```

**Client-Side Authentication (JWT):**
```
wss://eu.rt.speechmatics.com/v2?jwt=<temporary-key>
```

### 2.2 Generate JWT Token
**Endpoint:** `POST https://mp.speechmatics.com/v1/api_keys?type=rt`

**Headers:**
```
Authorization: Bearer <api-key>
Content-Type: application/json
```

**Request Body:**
```json
{
  "ttl": 3600
}
```

**Response:**
```json
{
  "key_value": "eyJhbG..."
}
```

### 2.3 Message Flow

#### StartRecognition (Client → Server)
```json
{
  "message": "StartRecognition",
  "transcription_config": {
    "language": "en",
    "operating_point": "enhanced",
    "enable_partials": true,
    "max_delay": 2.0,
    "enable_entities": true,
    "diarization": "speaker",
    "speaker_diarization_config": {
      "max_speakers": 4
    }
  },
  "audio_format": {
    "type": "raw",
    "encoding": "pcm_s16le",
    "sample_rate": 16000
  }
}
```

#### RecognitionStarted (Server → Client)
```json
{
  "message": "RecognitionStarted",
  "id": "session-uuid"
}
```

#### AddAudio (Client → Server)
Binary WebSocket frame containing raw audio data.

#### AudioAdded (Server → Client)
```json
{
  "message": "AudioAdded",
  "seq_no": 1
}
```

#### AddPartialTranscript (Server → Client)
```json
{
  "message": "AddPartialTranscript",
  "format": "2.9",
  "metadata": {
    "start_time": 0.0,
    "end_time": 1.5,
    "transcript": "Hello world"
  },
  "results": [
    {
      "type": "word",
      "start_time": 0.0,
      "end_time": 0.5,
      "alternatives": [
        {"content": "Hello", "confidence": 0.95}
      ]
    }
  ]
}
```

#### AddTranscript (Server → Client)
```json
{
  "message": "AddTranscript",
  "format": "2.9",
  "metadata": {
    "start_time": 0.0,
    "end_time": 2.0,
    "transcript": "Hello world."
  },
  "results": [
    {
      "type": "word",
      "start_time": 0.0,
      "end_time": 0.5,
      "alternatives": [
        {"content": "Hello", "confidence": 0.98}
      ]
    },
    {
      "type": "punctuation",
      "start_time": 2.0,
      "end_time": 2.0,
      "alternatives": [
        {"content": ".", "confidence": 1.0}
      ]
    }
  ]
}
```

#### EndOfStream (Client → Server)
```json
{
  "message": "EndOfStream",
  "last_seq_no": 100
}
```

#### EndOfTranscript (Server → Client)
```json
{
  "message": "EndOfTranscript"
}
```

### 2.4 Configuration Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| language | string | Yes | ISO language code (e.g., "en", "auto") |
| operating_point | string | No | "standard" or "enhanced" (default: standard) |
| enable_partials | bool | No | Enable partial transcripts (default: false) |
| max_delay | float | No | Max delay in seconds (0-10, default: 2.0) |
| enable_entities | bool | No | Enable entity recognition |
| diarization | string | No | "none" or "speaker" |
| speaker_diarization_config.max_speakers | int | No | Max speakers (1-20) |
| additional_vocab | array | No | Custom vocabulary words |
| punctuation_overrides.permitted_marks | array | No | Allowed punctuation |
| punctuation_overrides.sensitivity | float | No | 0.0-1.0 |

### 2.5 Audio Formats

**Raw Audio:**
| Encoding | Description |
|----------|-------------|
| pcm_f32le | 32-bit float, little-endian |
| pcm_s16le | 16-bit signed integer, little-endian |
| mulaw | μ-law encoded (8-bit) |

**File-Based:**
WAV, MP3, AAC, OGG, MPEG, AMR, M4A, MP4, FLAC

### 2.6 Supported Languages

| Language | Code | Language | Code |
|----------|------|----------|------|
| Arabic | ar | Japanese | ja |
| Basque | eu | Korean | ko |
| Bengali | bn | Mandarin | cmn |
| Cantonese | yue | Norwegian | no |
| Czech | cs | Polish | pl |
| Danish | da | Portuguese | pt |
| Dutch | nl | Romanian | ro |
| English | en | Russian | ru |
| Finnish | fi | Spanish | es |
| French | fr | Swedish | sv |
| German | de | Thai | th |
| Greek | el | Turkish | tr |
| Hebrew | he | Ukrainian | uk |
| Hindi | hi | Vietnamese | vi |
| Hungarian | hu | Welsh | cy |
| Indonesian | id | Automatic | auto |
| Italian | it | ... and more | |

---

## 3. TTS API (HTTP Streaming)

### 3.1 Endpoint
**URL:** `POST https://preview.tts.speechmatics.com/generate/<voice_id>`

**Headers:**
```
Authorization: Bearer <api-key>
Content-Type: application/json
```

**Request Body:**
```json
{
  "text": "Hello, this is a test."
}
```

**Query Parameters:**
| Parameter | Values | Default | Description |
|-----------|--------|---------|-------------|
| output_format | wav_16000, pcm_16000 | wav_16000 | Audio output format |

**Response:** Streaming audio data (WAV or raw PCM)

### 3.2 Voices

| Voice ID | Name | Gender | Accent |
|----------|------|--------|--------|
| sarah | Sarah | Female | UK English |
| theo | Theo | Male | UK English |
| megan | Megan | Female | US English |
| jack | Jack | Male | US English |

### 3.3 Audio Output

| Format | Sample Rate | Bit Depth | Channels |
|--------|-------------|-----------|----------|
| wav_16000 | 16 kHz | 16-bit | Mono |
| pcm_16000 | 16 kHz | 16-bit | Mono (little-endian) |

### 3.4 Latency
- First audio chunk: <200ms
- Subsequent chunks: Faster than real-time

---

## 4. Rate Limits

| Plan | STT | TTS |
|------|-----|-----|
| Free | 480 min/month | 1M chars/month |
| Pay-as-you-go | Unlimited | Unlimited |

---

## 5. Pricing

### STT Pricing
| Type | Price per Hour |
|------|---------------|
| Standard accuracy | ~$0.48/hour |
| Enhanced accuracy | ~$0.72/hour |
| Volume discount | 20% off after 500 hours/month |

### TTS Pricing
| Type | Price |
|------|-------|
| TTS (preview) | $0.011 per 1,000 characters |

---

## 6. Implementation Plan

### 6.1 Module Structure
```
src/core/stt/speechmatics/
├── mod.rs           # Module exports and constants
├── config.rs        # SpeechmaticsSTTConfig
├── messages.rs      # WebSocket message types
└── client.rs        # SpeechmaticsSTT implementing BaseSTT

src/core/tts/speechmatics/
├── mod.rs           # Module exports and constants
├── config.rs        # SpeechmaticsTTSConfig
└── provider.rs      # SpeechmaticsTTS implementing BaseTTS
```

### 6.2 Implementation Steps

1. **Create STT config.rs**
   - SpeechmaticsLanguage enum (55+ languages)
   - SpeechmaticsOperatingPoint enum (Standard, Enhanced)
   - SpeechmaticsEncoding enum (PcmF32le, PcmS16le, Mulaw)
   - SpeechmaticsRegion enum (EU, US)
   - SpeechmaticsSTTConfig struct

2. **Create STT messages.rs**
   - StartRecognitionMessage
   - RecognitionStartedMessage
   - AddPartialTranscriptMessage
   - AddTranscriptMessage
   - EndOfStreamMessage
   - ErrorMessage

3. **Create STT client.rs**
   - SpeechmaticsSTT implementing BaseSTT
   - WebSocket connection with JWT refresh
   - Binary audio streaming
   - Partial and final transcript handling

4. **Create TTS config.rs**
   - SpeechmaticsVoice enum (Sarah, Theo, Megan, Jack)
   - SpeechmaticsOutputFormat enum (Wav16000, Pcm16000)
   - SpeechmaticsTTSConfig struct

5. **Create TTS provider.rs**
   - SpeechmaticsTTS implementing BaseTTS
   - HTTP streaming via reqwest
   - TTSRequestBuilder implementation

6. **Update plugin system**
   - Add to plugin/builtin/mod.rs
   - Register STT and TTS factories

### 6.3 Configuration Mapping

**STT:**
| STTConfig Field | Speechmatics Mapping |
|-----------------|---------------------|
| api_key | Authorization: Bearer header |
| language | language in transcription_config |
| sample_rate | sample_rate in audio_format |
| encoding | encoding in audio_format |

**TTS:**
| TTSConfig Field | Speechmatics Mapping |
|-----------------|---------------------|
| api_key | Authorization: Bearer header |
| voice_id | URL path parameter (/generate/{voice}) |
| audio_format | output_format query parameter |

---

## 7. Testing Plan

### 7.1 Unit Tests
- Config parsing and validation
- Language enum serialization
- Message serialization/deserialization
- Request body construction

### 7.2 Integration Tests (with credentials)
- WebSocket connection
- Real-time transcription
- TTS synthesis
- JWT token refresh

### 7.3 Test Cases
```rust
#[test]
fn test_speechmatics_stt_config_defaults()
#[test]
fn test_speechmatics_language_serialization()
#[test]
fn test_speechmatics_message_parsing()
#[test]
fn test_speechmatics_tts_voice_enum()
#[tokio::test]
async fn test_speechmatics_stt_connect()
#[tokio::test]
async fn test_speechmatics_tts_speak()
```

---

## 8. Error Handling

### WebSocket Close Codes
| Code | Meaning |
|------|---------|
| 1003 | Unsupported data type |
| 1008 | Policy violation |
| 1011 | Server error |
| 4001 | Invalid config |
| 4002 | Invalid auth |
| 4003 | Quota exceeded |
| 4004-4013 | Various errors |

### HTTP Errors
| Status | Meaning |
|--------|---------|
| 400 | Bad request |
| 401 | Unauthorized |
| 429 | Rate limited |
| 500 | Server error |

---

## 9. References

- [Speechmatics Main Site](https://www.speechmatics.com)
- [API Documentation](https://docs.speechmatics.com)
- [STT Realtime API](https://docs.speechmatics.com/api-ref/realtime-transcription-websocket)
- [TTS Quickstart](https://docs.speechmatics.com/text-to-speech/quickstart)
- [Languages](https://docs.speechmatics.com/introduction/supported-languages)
- [Pricing](https://www.speechmatics.com/pricing)
