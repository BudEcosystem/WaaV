# AmiVoice (Advanced Media) - STT Provider

> **Status:** IMPLEMENTING
> **Date:** 2026-01-14
> **Type:** STT Only (Speech Recognition)

---

## Overview

AmiVoice is a Japanese speech recognition API service developed by Advanced Media Inc. It specializes in high-accuracy Japanese language recognition with domain-specific engines for medical, legal, and financial terminology. The service operates on the AmiVoice Cloud Platform (ACP).

**Key Differentiators:**
- 40+ years of Japanese speech technology research
- Domain-specific engines (medical, finance, insurance, legal)
- 2.1 million+ word Japanese dictionary
- End-to-end neural engines with multilingual support
- Emotion/sentiment analysis from voice
- All data processed in Japan (data sovereignty)

---

## API Endpoints

### WebSocket (Streaming STT)

| Endpoint | Purpose |
|----------|---------|
| `wss://acp-api.amivoice.com/v1/` | Streaming recognition with logging |
| `wss://acp-api.amivoice.com/v1/nolog/` | Streaming recognition without logging |

### Synchronous HTTP (Batch STT)

| Endpoint | Purpose |
|----------|---------|
| `POST https://acp-api.amivoice.com/v1/recognize` | Batch recognition with logging |
| `POST https://acp-api.amivoice.com/v1/nolog/recognize` | Batch recognition without logging |

### Asynchronous HTTP (Large File Processing)

| Endpoint | Purpose |
|----------|---------|
| `POST https://acp-api.amivoice.com/v2/jobs` | Submit large audio files (>16MB) |
| `GET https://acp-api.amivoice.com/v2/jobs/{job_id}` | Poll for job status |

---

## Authentication

- **Method:** API Key (APPKEY)
- **Header/Parameter:** `authorization={APPKEY}` or `u={APPKEY}`
- **Acquisition:** Register at [acp.amivoice.com](https://acp.amivoice.com/)

---

## Speech Recognition Engines

### End-to-End (E2E) Engines - Next Generation

| Engine ID | Language | Description |
|-----------|----------|-------------|
| `-a2-ja-general` | Japanese | General purpose, high accuracy |
| `-a2-zh-general` | Chinese | General purpose |
| `-a2-multi-general` | Multilingual | Japanese, English, Chinese (single model) |
| `-a2b-ja-general` | Japanese | Batch optimized (higher accuracy) |
| `-a2b-zh-general` | Chinese | Batch optimized |
| `-a2b-multi-general` | Multilingual | Batch optimized |

**Note:** E2E engines do NOT support word registration. Use hybrid engines if custom vocabulary is needed.

### Hybrid Engines - Domain Optimized

**Conversation Engines (telephone/meeting):**

| Engine ID | Domain | Languages |
|-----------|--------|-----------|
| `-a-general` | General | Japanese (8k/16k) |
| `-a-medical` | Medical | Japanese (8k/16k) |
| `-a-finance` | Finance | Japanese (8k/16k) |
| `-a-insurance` | Insurance | Japanese (8k/16k) |
| `-a-general-zh` | General Chinese | Chinese (8k/16k) |
| `-a-general-en` | General English | English (8k/16k) |
| `-a-general-ko` | General Korean | Korean (8k/16k) |

**Voice Input Engines (dictation):**

| Engine ID | Domain |
|-----------|--------|
| `-a-general-input` | General |
| `-a-medical-input` | Medical terminology |
| `-a-finance-input` | Financial terminology |
| `-a-insurance-input` | Insurance terminology |
| `-a-name-input` | Japanese names |
| `-a-address-input` | Japanese addresses |

---

## Audio Format Requirements

| Format | Sample Rate | Bit Depth | Channels |
|--------|-------------|-----------|----------|
| WAV (PCM) | 8kHz, 16kHz | 16-bit | Mono |
| Raw PCM | 8kHz, 16kHz | 16-bit LSB | Mono |

**Format Codes:**
- `16K` - 16-bit PCM, 16kHz
- `8K` - 16-bit PCM, 8kHz
- `LSB16K` - Little-endian PCM, 16kHz
- `LSB8K` - Little-endian PCM, 8kHz

---

## WebSocket Protocol

### Connection Flow

```
1. Connect → wss://acp-api.amivoice.com/v1/
2. Send 's' command (start recognition)
3. Receive 's' response (success/failure)
4. Send 'p' commands (audio data chunks)
5. Receive events (S, E, C, U, A)
6. Send 'e' command (end session)
7. Receive 'e' response
8. Close connection
```

### Commands (Client → Server)

**s Command (Start Session):**
```
s <sample_rate> <engine_id> [key=value ...]

Example:
s 16k -a-general authorization=YOUR_APPKEY resultUpdatedInterval=1000
```

**p Command (Audio Data):**
```
Binary message: p<audio_bytes>
Maximum size: 16MB per command
```

**e Command (End Session):**
```
e
```

### Events (Server → Client)

| Event | Format | Description |
|-------|--------|-------------|
| `S` | `S <timestamp>` | Speech detected start |
| `E` | `E <timestamp>` | Speech detected end |
| `C` | `C` | Recognition processing started |
| `U` | `U <json>` | Intermediate (partial) result |
| `A` | `A <json>` | Final confirmed result |
| `G` | `G <info>` | Server-generated info |

### Result JSON Format

```json
{
  "results": [
    {
      "text": "recognized text",
      "tokens": [
        {
          "written": "今日",
          "confidence": 0.95,
          "starttime": 100,
          "endtime": 300,
          "spoken": "きょう"
        }
      ],
      "tags": [],
      "rulename": ""
    }
  ],
  "text": "full recognized text",
  "code": "0",
  "message": "success"
}
```

---

## Synchronous HTTP Interface

### Request Format (multipart/form-data)

```http
POST /v1/recognize HTTP/1.1
Host: acp-api.amivoice.com
Content-Type: multipart/form-data; boundary=----Boundary

------Boundary
Content-Disposition: form-data; name="u"

YOUR_APPKEY
------Boundary
Content-Disposition: form-data; name="d"

-a-general
------Boundary
Content-Disposition: form-data; name="c"

16K
------Boundary
Content-Disposition: form-data; name="a"
Content-Type: application/octet-stream

[binary_audio_data]
------Boundary--
```

### Parameters

| Parameter | Required | Description |
|-----------|----------|-------------|
| `u` | Yes | APPKEY (authentication) |
| `d` | Yes | Engine ID (e.g., `-a-general`) |
| `a` | Yes | Audio binary data (MUST be last) |
| `c` | No | Audio format (e.g., `16K`, `LSB16K`) |
| `profileId` | No | User profile identifier |
| `profileWords` | No | Custom word definitions |

**Important:** The `a` parameter must be the last part. Parameters after `a` are ignored.

### Response Format

```json
{
  "results": [{
    "text": "recognized text",
    "tokens": [...],
    "tags": [],
    "rulename": ""
  }],
  "text": "full text",
  "code": "0",
  "message": "success"
}
```

---

## Additional Features

### Sentiment/Emotion Analysis

When enabled, the API returns emotion parameters:
- Joy, Anger, Stress, Dissatisfaction, Expectation
- Updated approximately every 2 seconds
- 20 emotion parameters total

### Word Registration (Hybrid Engines Only)

Custom vocabulary can be registered via the `profileWords` parameter:
```
profileWords={"words":[{"written":"AmiVoice","spoken":"あみぼいす","class":"proper_noun"}]}
```

### Speaker Diarization

Available with specific engines. Enable via:
```
segmenterProperties="useDiarizer=1"
```

---

## Pricing

- **Free Tier:** 60-99 minutes/month (varies by engine)
- **Pay-as-you-go:** Starting from 1 yen/hour
- **No initial costs**
- **No contract required**

---

## Implementation Notes

### Recommended Approach for WaaV

1. **Primary Interface:** WebSocket for real-time streaming STT
2. **Reference Implementation:** Similar to Azure WebSocket STT
3. **Complexity:** Medium (proprietary text-based protocol)

### Key Differences from Other Providers

1. Custom text-based protocol (not JSON over WebSocket)
2. Commands use single-letter identifiers (s, p, e)
3. Audio sent with 'p' prefix as binary message
4. Intermediate results via 'U' events
5. Final results via 'A' events

### Error Handling

Response codes in JSON:
- `"code": "0"` - Success
- `"code": "-1"` - Error (see message)

---

## Client Libraries

Official libraries available:
- Java (Java 7+, TLSv1.2)
- Python (2.x/3.x)
- PHP (5/7)
- C# (.NET 4.5.2+)
- C++ (POCO libraries)
- JavaScript (browser)

GitHub: [github.com/advanced-media-inc/amivoice-api-client-library](https://github.com/advanced-media-inc/amivoice-api-client-library)

---

## References

- [AmiVoice Cloud Platform](https://acp.amivoice.com/)
- [API Documentation](https://docs.amivoice.com/en/amivoice-api/manual/)
- [WebSocket Interface](https://docs.amivoice.com/en/amivoice-api/manual/websocket-interface/)
- [Speech Recognition Engines](https://docs.amivoice.com/en/amivoice-api/manual/engines/)
- [Sample Programs](https://docs.amivoice.com/en/amivoice-api/manual/sample-programs/)
- [Client Libraries](https://github.com/advanced-media-inc/amivoice-api-client-library)
