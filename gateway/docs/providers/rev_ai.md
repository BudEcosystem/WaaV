# Rev AI STT Provider Integration

## Overview

Rev AI provides speech-to-text transcription services with both real-time streaming and asynchronous (batch) APIs. Known for industry-leading English accuracy and human-AI hybrid transcription options.

**Website:** https://www.rev.ai
**API Documentation:** https://docs.rev.ai
**Pricing:** https://www.rev.ai/pricing

## Supported Features

### Core Capabilities
- **Real-time Streaming STT** via WebSocket
- **Asynchronous (Batch) STT** via REST API
- **Human Transcription** (12-24 hour turnaround)
- **Speaker Diarization** (up to 8 speakers English, 6 others)
- **Custom Vocabulary** (up to 6000 phrases for English)
- **Profanity Filtering**
- **Disfluency Removal** (removes "um", "uh")
- **Word-level Timestamps**
- **Confidence Scores**

### Add-on Features
- Language Identification
- Language Translation
- Sentiment Analysis
- Topic Extraction
- Summarization
- Forced Alignment

## Streaming API Specification

### WebSocket Endpoint
```
wss://api.rev.ai/speechtotext/v1/stream
```

### Authentication
Access token passed as query parameter:
```
wss://api.rev.ai/speechtotext/v1/stream?access_token=<REVAI_ACCESS_TOKEN>&content_type=...
```

### Query Parameters

| Parameter | Required | Default | Description |
|-----------|----------|---------|-------------|
| `access_token` | Yes | - | Rev AI API access token |
| `content_type` | Yes | - | Audio format specification |
| `language` | No | `en` | Language code (en, fr, de, it, ja, ko, cmn, pt, es) |
| `metadata` | No | - | Custom metadata string |
| `custom_vocabulary_id` | No | - | ID of pre-created custom vocabulary |
| `filter_profanity` | No | `false` | Replace profanities with asterisks |
| `remove_disfluencies` | No | `false` | Remove filler words (um, uh) |
| `enable_speaker_switch` | No | `false` | Detect speaker changes |
| `detailed_partials` | No | `false` | Include timestamps/confidence in partials |
| `start_ts` | No | `0` | Timestamp offset in seconds |
| `max_segment_duration_seconds` | No | - | Max final hypothesis duration (5-30s) |
| `transcriber` | No | - | Model selection (e.g., `machine_v2`) |
| `skip_postprocessing` | No | `false` | Disable punctuation/normalization |
| `priority` | No | `speed` | Optimization mode (`speed`/`accuracy`) |
| `max_connection_wait_seconds` | No | `60` | Connection timeout (60-600s) |

### Audio Format (content_type)

Raw audio format specification:
```
audio/x-raw;layout=interleaved;rate=16000;format=S16LE;channels=1
```

**Supported Formats:**
- `audio/x-raw` (requires layout, rate, format, channels)
- `audio/x-flac`
- `audio/x-wav`
- All FFmpeg-supported formats (may increase latency)

**Raw Audio Parameters:**
- `layout`: `interleaved` or `non-interleaved`
- `rate`: Sample rate in Hz (8000-48000)
- `format`: `S16LE` (signed 16-bit little-endian) - required for non-English
- `channels`: Number of audio channels (1-10)

### Message Protocol

#### Client → Server

**Binary Messages:** Audio data chunks (recommend 250ms+ chunks)

**Text Messages:** Only `EOS` (End-Of-Stream) to gracefully close connection

#### Server → Client

**Connected Message:**
```json
{
  "type": "connected",
  "id": "s1d24ax2fd21"
}
```

**Partial Hypothesis:**
```json
{
  "type": "partial",
  "ts": 1.01,
  "end_ts": 2.2,
  "elements": [
    {"type": "text", "value": "one"},
    {"type": "text", "value": "tooth"}
  ]
}
```

**Final Hypothesis:**
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

**Element Types:**
- `text`: Transcribed word with optional ts, end_ts, confidence
- `punct`: Punctuation/whitespace

### Error Codes

| Code | Description | Retryable |
|------|-------------|-----------|
| 4001 | Unauthorized/invalid token | No |
| 4002 | Invalid content-type or metadata | No |
| 4003 | Insufficient credits | No |
| 4010 | Server shutting down | Yes |
| 4013 | No streaming instances available | Yes |
| 4029 | Concurrent connection limit exceeded | No |

### Limits

- **Concurrent Streams:** 10 (adjustable via support)
- **Max Stream Duration:** 3 hours
- **Min Billing:** 15 seconds
- **Billing Basis:** Greater of audio duration or stream duration

## Supported Languages (Streaming)

| Code | Language |
|------|----------|
| `en` | English (all accents) |
| `fr` | French |
| `de` | German |
| `it` | Italian |
| `ja` | Japanese |
| `ko` | Korean |
| `cmn` | Mandarin Chinese |
| `pt` | Portuguese |
| `es` | Spanish |

**Note:** The `language` parameter cannot be used with `filter_profanity`, `remove_disfluencies`, or `custom_vocabulary_id` for non-English languages.

## Pricing

| Service | Price |
|---------|-------|
| Reverb Transcription | $0.20/hour |
| Reverb Turbo | $0.10/hour |
| Whisper Fusion/Medium/Large | $0.005/minute |
| Foreign Language (58+) | $0.30/hour |
| Human Transcription | $1.99/minute |
| Language Identification | $0.003/minute |
| Translation (Standard) | $0.002/minute |
| Translation (Premium) | $0.025/minute |
| Sentiment Analysis | $0.0008/10 words |
| Summarization (Standard) | $0.002/minute |
| Topic Extraction | $0.0008/10 words |

**Free Tier:** 5 hours of Reverb ASR credits

## Implementation Plan

### Architecture

Rev AI will be implemented as a Dynamic Plugin following the existing STT provider pattern:

```
src/core/stt/revai/
├── mod.rs           # Module exports, constants
├── config.rs        # RevAISTTConfig struct
├── messages.rs      # WebSocket message types
└── client.rs        # RevAISTT implementing BaseSTT trait
```

### Configuration Types

```rust
// Language enum for supported streaming languages
pub enum RevAILanguage {
    English,      // en
    French,       // fr
    German,       // de
    Italian,      // it
    Japanese,     // ja
    Korean,       // ko
    Mandarin,     // cmn
    Portuguese,   // pt
    Spanish,      // es
}

// Audio encoding
pub enum RevAIEncoding {
    Raw,    // audio/x-raw
    Flac,   // audio/x-flac
    Wav,    // audio/x-wav
}

// Raw audio format
pub enum RevAIRawFormat {
    S16LE,  // Signed 16-bit little-endian (default)
    S16BE,  // Signed 16-bit big-endian
    F32LE,  // Float 32-bit little-endian
}

// Configuration
pub struct RevAISTTConfig {
    pub api_key: String,
    pub language: RevAILanguage,
    pub encoding: RevAIEncoding,
    pub sample_rate: u32,
    pub channels: u8,
    pub filter_profanity: bool,
    pub remove_disfluencies: bool,
    pub enable_speaker_switch: bool,
    pub detailed_partials: bool,
    pub custom_vocabulary_id: Option<String>,
    pub max_segment_duration_seconds: Option<u8>,
    pub skip_postprocessing: bool,
    pub priority: RevAIPriority,  // speed or accuracy
}
```

### WebSocket Message Types

```rust
// Server messages
pub enum RevAIServerMessage {
    Connected { id: String },
    Partial(TranscriptHypothesis),
    Final(TranscriptHypothesis),
}

pub struct TranscriptHypothesis {
    pub ts: f64,
    pub end_ts: f64,
    pub elements: Vec<TranscriptElement>,
}

pub struct TranscriptElement {
    pub element_type: ElementType, // "text" or "punct"
    pub value: String,
    pub ts: Option<f64>,
    pub end_ts: Option<f64>,
    pub confidence: Option<f64>,
}
```

### Connection Flow

1. Build WebSocket URL with query parameters
2. Connect via WebSocket handshake
3. Receive "connected" message with session ID
4. Send binary audio chunks
5. Receive partial/final hypotheses
6. Send "EOS" text message to end stream
7. Receive final hypothesis and close

### Best Practices

1. **Audio Chunks:** Send 250ms+ audio chunks for optimal performance
2. **Raw Audio:** Use `audio/x-raw` for lowest latency
3. **Sample Rate:** Use 16000 Hz for best balance of quality/bandwidth
4. **S16LE Format:** Required for non-English languages
5. **Reconnection:** For long streams approaching 3 hours, establish new connection before timeout
6. **Error Handling:** Implement retry logic for 4010 and 4013 errors

### Test Plan

1. **Unit Tests:**
   - Configuration validation (empty API key, invalid sample rate)
   - Message serialization/deserialization
   - URL construction with parameters
   - Element parsing (text, punct)

2. **Integration Tests:**
   - WebSocket connection lifecycle
   - Audio streaming with real API
   - Partial/final hypothesis handling
   - Error code handling
   - EOS flow

3. **Edge Cases:**
   - Empty audio
   - Very long audio (approach 3hr limit)
   - Speaker switching
   - Multiple languages

## Environment Variables

```bash
REVAI_API_KEY=your_access_token
```

## References

- [Streaming API Overview](https://docs.rev.ai/api/streaming/)
- [Streaming API Requests](https://docs.rev.ai/api/streaming/requests/)
- [Streaming API Responses](https://docs.rev.ai/api/streaming/responses/)
- [Code Samples](https://docs.rev.ai/api/streaming/code-samples/)
- [Example Session](https://docs.rev.ai/api/streaming/example-session/)
- [FAQ](https://docs.rev.ai/faq/)
- [Python SDK](https://github.com/revdotcom/revai-python-sdk)
