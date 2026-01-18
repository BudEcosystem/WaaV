# iFlytek Integration Documentation

> **Provider:** iFlytek (科大讯飞)
> **Website:** https://www.iflytek.com
> **Global Platform:** https://global.xfyun.cn/
> **Status:** Implementation In Progress
> **Last Updated:** 2026-01-13

## Overview

iFlytek is China's leading AI company specializing in speech recognition and synthesis technologies. Founded in 1999, they hold over 70% market share in China's voice AI market and consistently rank among the top in international speech recognition competitions (NIST, CHiME, Blizzard Challenge).

## Capabilities

| Capability | Supported | Notes |
|------------|-----------|-------|
| STT (Short Form) | YES | Max 60 seconds, 30+ languages |
| STT (Real-time) | YES | Up to 5 hours streaming |
| TTS | YES | 15+ languages, multiple voices |
| Voice Cloning | YES | Custom brand voices |
| Translation | YES | Multi-language machine translation |
| Pronunciation Assessment | YES | Language learning focused |

## API Endpoints

### Speech-to-Text (STT)

#### Short Form ASR (Voice Dictation)
- **Endpoint:** `wss://iat-api-sg.xf-yun.com/v2/iat`
- **Max Duration:** 60 seconds
- **Languages:** 30+ including Chinese (Mandarin, Cantonese, 23 dialects), English, Japanese, Korean, Russian, French, Spanish, Thai, Vietnamese, German, Arabic, etc.
- **Use Cases:** Voice search, short voice commands, voice input

#### Real-time ASR (Streaming)
- **Endpoint:** `wss://ist-api-sg.xf-yun.com/v2/ist`
- **Max Duration:** 5 hours
- **Languages:** 25+ with large language model engines
- **Use Cases:** Live transcription, meetings, video subtitles, call centers

### Text-to-Speech (TTS)

#### Online TTS
- **Endpoint:** `wss://tts-api-sg.xf-yun.com/v2/tts`
- **Languages:** 15+ including Chinese, English, Japanese, Indonesian, Russian, French, German, Arabic, Vietnamese, Thai, Korean, Portuguese, Malay, Hindi, Urdu
- **Use Cases:** Voice assistants, audiobooks, accessibility, broadcasting

## Authentication

iFlytek uses HMAC-SHA256 signature-based authentication.

### Credentials Required
- **APPID:** Application identifier
- **APIKey:** 32-character string for signature
- **APISecret:** 32-character secret for HMAC signing

### Signature Generation Process

```
1. Generate timestamp in RFC1123 format (UTC/GMT):
   "Wed, 20 Nov 2024 03:14:25 GMT"

2. Build signature origin string:
   host: {host}
   date: {date}
   GET /v2/iat HTTP/1.1

3. Sign with HMAC-SHA256:
   signature = HMAC-SHA256(signature_origin, APISecret)

4. Encode signature in Base64

5. Build authorization header:
   api_key="{APIKey}",algorithm="hmac-sha256",headers="host date request-line",signature="{base64_signature}"

6. Append to WebSocket URL as query parameter (URL-encoded)
```

### Clock Skew Tolerance
- Maximum allowed deviation: **300 seconds**
- Server rejects requests outside this window

## Request/Response Formats

### STT Request Format (JSON)
```json
{
  "common": {
    "app_id": "your_appid"
  },
  "business": {
    "language": "zh_cn",
    "domain": "iat",
    "accent": "mandarin",
    "vad_eos": 3000,
    "dwa": "wpgs"
  },
  "data": {
    "status": 0,
    "format": "audio/L16;rate=16000",
    "encoding": "raw",
    "audio": "base64_encoded_audio"
  }
}
```

### STT Response Format
```json
{
  "code": 0,
  "message": "success",
  "sid": "session_id",
  "data": {
    "result": {
      "ws": [
        {
          "bg": 0,
          "cw": [{"w": "recognized", "sc": 0.95}]
        }
      ],
      "sn": 1,
      "ls": false,
      "pgs": "apd"
    },
    "status": 1
  }
}
```

### TTS Request Format (JSON)
```json
{
  "common": {
    "app_id": "your_appid"
  },
  "business": {
    "aue": "raw",
    "auf": "audio/L16;rate=16000",
    "vcn": "xiaoyan",
    "speed": 50,
    "volume": 50,
    "pitch": 50,
    "tte": "UTF8"
  },
  "data": {
    "text": "base64_encoded_text",
    "status": 2
  }
}
```

### TTS Response Format
```json
{
  "code": 0,
  "message": "success",
  "sid": "session_id",
  "data": {
    "audio": "base64_encoded_audio",
    "status": 1,
    "ced": "23456"
  }
}
```

## Audio Specifications

### STT Audio Requirements
| Parameter | Value |
|-----------|-------|
| Sample Rate | 16000 Hz (recommended) or 8000 Hz |
| Bit Depth | 16-bit |
| Channels | Mono |
| Encoding | PCM, MP3 (Mandarin/English only), Speex |
| Frame Size | 1280 bytes (PCM @ 16kHz) |
| Frame Interval | 40ms |

### TTS Audio Output
| Parameter | Options |
|-----------|---------|
| Sample Rate | 16000 Hz or 8000 Hz |
| Encoding (aue) | raw (PCM), lame (MP3), speex, speex-wb |
| Text Encoding (tte) | UTF8, GB2312, GBK, BIG5, UNICODE |

## Parameters Reference

### STT Business Parameters

| Parameter | Description | Values | Default |
|-----------|-------------|--------|---------|
| language | Recognition language | zh_cn, en_us, ja_jp, etc. | zh_cn |
| domain | Recognition domain | iat (daily), medical | iat |
| accent | Accent/dialect | mandarin, cantonese, etc. | mandarin |
| vad_eos | End-of-speech silence (ms) | 0-10000 | 2000 |
| dwa | Dynamic correction (Chinese) | wpgs | - |
| ptt | Add punctuation | 0 (off), 1 (on) | 0 |
| nunum | Convert numbers to digits | 0 (off), 1 (on) | 1 |

### TTS Business Parameters

| Parameter | Description | Values | Default |
|-----------|-------------|--------|---------|
| vcn | Voice/speaker name | xiaoyan, john_ce, etc. | xiaoyan |
| speed | Speech rate | 0-100 | 50 |
| volume | Volume level | 0-100 | 50 |
| pitch | Pitch level | 0-100 | 50 |
| aue | Audio encoding | raw, lame, speex, speex-wb | raw |
| auf | Sample rate | audio/L16;rate=16000 or rate=8000 | 16000 |
| bgs | Background sound | 0 (off), 1 (on) | 0 |
| reg | English pronunciation | 0 (auto), 1 (letter), 2 (word) | 0 |
| rdn | Number pronunciation | 0 (auto), 1 (digit), 2 (value), 3 (auto v2) | 0 |

## Available Voices (TTS)

| Voice Code (vcn) | Language | Gender | Description |
|------------------|----------|--------|-------------|
| xiaoyan | Chinese | Female | Standard Mandarin (default) |
| aisjiuxu | Chinese | Male | Young male voice |
| aisxping | Chinese | Female | Female broadcaster |
| aisjinger | Chinese | Female | Sweet female voice |
| aisbabyxu | Chinese | Child | Child voice |
| john_ce | English | Male | American English male |
| catherine | English | Female | American English female |
| luna | Japanese | Female | Japanese female |
| anjali | Hindi | Female | Hindi female |

> **Note:** Additional voices must be enabled in the iFlytek console before use. Attempting to use a non-enabled voice returns error 11200.

## Error Codes

| Code | Description | Resolution |
|------|-------------|------------|
| 0 | Success | - |
| 10005 | APPID authorization failure | Check APPID and credentials |
| 10006 | Insufficient balance | Add credits or subscribe |
| 10007 | Invalid parameter | Check request parameters |
| 10043 | Audio decoding failed | Verify audio format |
| 10160 | Request expired | Check system clock sync |
| 10161 | Base64 decoding failed | Verify base64 encoding |
| 10200 | Read timeout (10s inactivity) | Send data more frequently |
| 10313 | Missing app_id in first frame | Include app_id in common block |
| 11200 | Unauthorized speaker/feature | Enable in console or upgrade |
| 11201 | Daily quota exceeded | Wait for reset or upgrade |

## WebSocket Message Flow

### STT Flow
```
Client                              Server
  |                                    |
  |------ Connect (with auth) -------->|
  |<----- HTTP 101 Upgrade ------------|
  |                                    |
  |------ First frame (status=0) ----->|
  |------ Audio frames (status=1) ---->|
  |------ Last frame (status=2) ------>|
  |                                    |
  |<----- Partial results -------------|
  |<----- Partial results -------------|
  |<----- Final result (status=2) -----|
  |                                    |
  |<----- Server closes connection ----|
```

### TTS Flow
```
Client                              Server
  |                                    |
  |------ Connect (with auth) -------->|
  |<----- HTTP 101 Upgrade ------------|
  |                                    |
  |------ Text request (status=2) ---->|
  |                                    |
  |<----- Audio chunk (status=0) ------|
  |<----- Audio chunk (status=1) ------|
  |<----- Final chunk (status=2) ------|
  |                                    |
  |        (Client closes connection)  |
```

## Pricing

### Free Tier
| Account Type | Calls | Duration | Concurrent Limit |
|--------------|-------|----------|------------------|
| Individual | 100,000 | 90 days | 5 |
| Enterprise | 200,000 | 90 days | 5 |

### Pay-as-You-Go
| Service | Rate |
|---------|------|
| Short Form ASR | $1.40 / 1,000 calls |
| Online TTS | $1.40 / 1,000 calls |
| Machine Translation | $24 / 1M characters |
| Pronunciation Assessment | $3 / 1,000 calls |

### Volume Discounts (Resource Packages)
| Package | Calls | Price | Per 1K Rate | Discount |
|---------|-------|-------|-------------|----------|
| A | 1M | $1,400 | $1.40 | 0% |
| B | 5M | $6,650 | $1.33 | 5% |
| C | 10M+ | $12,600 | $1.26 | 10% |

## Best Practices

### Performance Optimization
1. **Frame Size:** Send exactly 1280 bytes per frame for optimal processing
2. **Frame Interval:** Maintain 40ms interval between frames
3. **Connection Reuse:** Avoid frequent connect/disconnect cycles
4. **Parallel Requests:** Stay within concurrent limit (5 for free tier)

### Error Handling
1. **Clock Sync:** Keep system clock within 300 seconds of UTC
2. **Timeout Prevention:** Send audio frames at regular intervals
3. **Graceful Degradation:** Handle 11201 (quota exceeded) with backoff
4. **Reconnection:** Implement exponential backoff for connection failures

### Audio Quality
1. **Sample Rate:** Use 16kHz for best accuracy
2. **Noise Reduction:** Pre-process audio to reduce background noise
3. **Format:** PCM is fastest; MP3 only for Mandarin/English STT

## Implementation Plan

### Module Structure
```
src/core/stt/iflytek/
├── mod.rs           # Module exports, constants
├── config.rs        # IFlytekSTTConfig, Language, Encoding enums
├── messages.rs      # Request/Response types, error codes
├── auth.rs          # HMAC-SHA256 signature generation
└── client.rs        # IFlytekSTT implementing BaseSTT trait

src/core/tts/iflytek/
├── mod.rs           # Module exports, constants
├── config.rs        # IFlytekTTSConfig, Voice, AudioFormat enums
├── messages.rs      # Request/Response types
└── provider.rs      # IFlytekTTS implementing BaseTTS trait
```

### Key Implementation Details

1. **Authentication Module (`auth.rs`)**
   - RFC1123 date generation
   - HMAC-SHA256 signature creation
   - URL encoding for WebSocket connection

2. **STT Client**
   - WebSocket connection with signature auth
   - Frame-based audio transmission (1280 bytes @ 40ms)
   - Status tracking (0→1→2)
   - Result parsing with dynamic correction support

3. **TTS Provider**
   - Single request with complete text
   - Streaming audio response handling
   - Base64 decode of audio chunks

### Test Plan

#### Unit Tests
- [ ] Signature generation correctness
- [ ] Request/response serialization
- [ ] Error code mapping
- [ ] Audio format validation
- [ ] Config validation

#### Integration Tests (with credentials)
- [ ] STT connection and transcription
- [ ] TTS synthesis for multiple languages
- [ ] Error handling (invalid credentials, quota exceeded)
- [ ] Concurrent request handling

## References

- [iFlytek Global Platform](https://global.xfyun.cn/)
- [Short Form ASR API](https://global.xfyun.cn/doc/asr/voicedictation/API.html)
- [Real-time ASR API](https://global.xfyun.cn/doc/rtasr/rtasr/API.html)
- [Online TTS API](https://global.xfyun.cn/doc/tts/online_tts/API.html)
- [Pricing](https://global.xfyun.cn/doc/platform/pricing.html)
- [Quick Start Guide](https://global.xfyun.cn/doc/platform/quickguide.html)
