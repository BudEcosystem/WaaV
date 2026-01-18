# Baidu AI Cloud Speech Integration

> **Provider #39** | Batch 6: China & East Asia
> **Status:** Implementation Complete ✅
> **Last Updated:** 2026-01-13
> **Tests:** 141 passed (73 STT + 68 TTS)

## Overview

Baidu AI Cloud Speech (百度语音) is China's leading speech technology platform, providing comprehensive STT and TTS services. It's part of Baidu AI Open Platform (百度AI开放平台) and offers specialized support for Chinese dialects.

## Provider Information

| Attribute | Value |
|-----------|-------|
| Provider Name | Baidu AI Cloud Speech |
| Website | https://ai.baidu.com/tech/speech |
| API Documentation | https://ai.baidu.com/ai-doc/SPEECH/ |
| Pricing | https://cloud.baidu.com/product-price/speech.html |
| Regions | China |
| Languages | Chinese (Mandarin, Cantonese, Sichuan, other dialects), English |

## Capabilities Matrix

| Capability | Supported | Notes |
|------------|-----------|-------|
| STT (Short Audio) | YES | REST API, max 60 seconds |
| STT (Real-time) | YES | WebSocket streaming |
| TTS | YES | REST API with multiple voices |
| Voice Cloning | NO | Not available |
| Custom Vocabulary | YES | Mandarin only, max 10,000 entries |

## Authentication

### OAuth 2.0 Access Token

Baidu uses OAuth 2.0 client credentials flow for authentication.

**Token Endpoint:**
```
https://aip.baidubce.com/oauth/2.0/token
```

**Request Parameters:**
| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| grant_type | string | Yes | Fixed: `client_credentials` |
| client_id | string | Yes | API Key from Baidu console |
| client_secret | string | Yes | Secret Key from Baidu console |

**Example Request:**
```bash
curl -X POST "https://aip.baidubce.com/oauth/2.0/token?grant_type=client_credentials&client_id=YOUR_API_KEY&client_secret=YOUR_SECRET_KEY"
```

**Example Response:**
```json
{
  "refresh_token": "25.b55fe1d287...",
  "expires_in": 2592000,
  "access_token": "24.6c5e1ff1...",
  "session_key": "...",
  "scope": "public wise_adapt"
}
```

**Token Validity:** 30 days (2,592,000 seconds)

## STT APIs

### 1. Short Audio Recognition (REST API)

For audio files up to 60 seconds.

**Endpoint:**
```
POST http://vop.baidu.com/server_api
POST http://vop.baidubce.com/server_api  (internal network)
```

**Request Headers:**
```
Content-Type: application/json
```

**Request Body (JSON Method):**
```json
{
  "format": "pcm",
  "rate": 16000,
  "channel": 1,
  "cuid": "unique_user_id",
  "token": "access_token",
  "dev_pid": 1537,
  "speech": "base64_encoded_audio",
  "len": 1024
}
```

**Request Parameters:**
| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| format | string | Yes | Audio format: pcm, wav, amr, m4a |
| rate | int | Yes | Sample rate: 16000 or 8000 |
| channel | int | Yes | Fixed: 1 (mono) |
| cuid | string | Yes | User identifier (max 60 chars) |
| token | string | Yes* | Access token (if not using API Key auth) |
| dev_pid | int | No | Recognition model (default: 1537) |
| lm_id | int | No | Custom model ID |
| speech | string | Yes | Base64-encoded audio |
| len | int | Yes | Original audio size in bytes |

**Recognition Models (dev_pid):**
| ID | Language | Punctuation | Notes |
|----|----------|-------------|-------|
| 1537 | Mandarin Chinese | Yes | Supports custom vocabulary |
| 1737 | English | No | No custom vocabulary |
| 1637 | Cantonese | Yes | No custom vocabulary |
| 1837 | Sichuan dialect | Yes | No custom vocabulary |

**Response:**
```json
{
  "err_no": 0,
  "err_msg": "success.",
  "sn": "481D633F-73BA-726F-49EF-8659ACCC2F3D",
  "result": ["北京天气"],
  "corpus_no": "6890859905390146256"
}
```

**Response Fields:**
| Field | Type | Description |
|-------|------|-------------|
| err_no | int | Error code (0 = success) |
| err_msg | string | Error message |
| sn | string | Unique audio identifier |
| result | array | Recognition results (UTF-8) |
| corpus_no | string | Corpus reference number |

### 2. Real-time Speech Recognition (WebSocket)

For streaming audio recognition.

**Endpoint:**
```
wss://vop.baidu.com/realtime_asr?sn=UUID
```

**Connection Parameters:**
| Parameter | Type | Description |
|-----------|------|-------------|
| sn | string | Session ID (UUID recommended, max 128 chars) |

**Start Frame (JSON):**
```json
{
  "type": "START",
  "data": {
    "appid": "your_app_id",
    "appkey": "your_api_key",
    "dev_pid": 1537,
    "cuid": "unique_user_id",
    "sample": 16000,
    "format": "pcm"
  }
}
```

**Audio Frame:**
- Type: Binary (WebSocket OPCODE_BINARY)
- Format: PCM, 16000 Hz, 16-bit, mono
- Chunk size: 160ms (5120 bytes)
- Send interval: Maximum 5 seconds between frames

**Calculation:**
```
chunk_size = sample_rate * bytes_per_sample * duration_ms / 1000
           = 16000 * 2 * 160 / 1000
           = 5120 bytes
```

**End Frame (JSON):**
```json
{
  "type": "FINISH"
}
```

**Cancel Frame (JSON):**
```json
{
  "type": "CANCEL"
}
```

**Response Messages:**
```json
{
  "err_no": 0,
  "err_msg": "success",
  "type": "MID_TEXT",
  "result": "你好",
  "sn": "xxx"
}
```

**Response Types:**
| Type | Description |
|------|-------------|
| MID_TEXT | Interim result |
| FIN_TEXT | Final result |
| ERROR | Error message |

## TTS API

### Text-to-Speech (REST API)

**Endpoint:**
```
POST http://tsn.baidu.com/text2audio
POST https://tsn.baidu.com/text2audio
```

**Request Parameters:**
| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| tex | string | Yes | Text to synthesize (max 1024 GBK bytes) |
| tok | string | Yes | Access token |
| cuid | string | Yes | User identifier (max 60 chars) |
| ctp | int | Yes | Fixed: 1 (web client) |
| lan | string | Yes | Fixed: "zh" (Chinese-English mixed) |
| spd | int | No | Speed: 0-15 (default: 5) |
| pit | int | No | Pitch: 0-15 (default: 5) |
| vol | int | No | Volume: 0-15 (default: 5) |
| per | int | No | Voice ID |
| aue | int | No | Audio format (default: 3) |
| audio_ctrl | string | No | Sample rate control JSON |

**Voice Options (per parameter):**

*Basic Library:*
| ID | Voice Name | Description |
|----|------------|-------------|
| 0 | 度小美 | Female voice |
| 1 | 度小宇 | Male voice |
| 3 | 度逍遥 | Male voice (basic) |
| 4 | 度丫丫 | Child voice |

*Premium Library:*
| ID | Description |
|----|-------------|
| 5003, 5118 | Premium voices |
| 106, 110, 111 | Additional voices |
| 103, 5 | More voices |

*Premium+ Library:*
| ID | Description |
|----|-------------|
| 4003, 4106, 4115, 4119 | High quality voices |
| 4105, 4117, 4100, 4103 | Additional voices |
| 4144, 4278, 4143, 4140 | More voices |
| 4129, 4149, 4254, 4206, 4226 | Extended voices |

*Large Model Library:*
| ID | Description |
|----|-------------|
| 4189, 4194, 4193, 4195 | AI voices |
| 4196, 4197, 20100, 20101 | Additional AI voices |
| 4257, 4132, 4139, 5977 | More AI voices |
| 4007, 4150, 4134, 4172 | Extended AI voices |

**Audio Formats (aue parameter):**
| Value | Format | Sample Rate |
|-------|--------|-------------|
| 3 | MP3 | 16k/24k |
| 4 | PCM | 16k/24k |
| 5 | PCM | 8k |
| 6 | WAV | 16k/24k |

**Example Request:**
```bash
curl -X POST "http://tsn.baidu.com/text2audio" \
  -d "tex=你好世界" \
  -d "tok=ACCESS_TOKEN" \
  -d "cuid=device_id" \
  -d "ctp=1" \
  -d "lan=zh" \
  -d "spd=5" \
  -d "pit=5" \
  -d "vol=5" \
  -d "per=0" \
  -d "aue=3"
```

**Response:**
- Success: Binary audio data with `Content-Type: audio/mp3` (or appropriate format)
- Error: JSON with `Content-Type: application/json`

**Error Response:**
```json
{
  "err_no": 500,
  "err_msg": "error message",
  "sn": "identifier",
  "idx": 1
}
```

**Text Encoding:**
- URL encode the `tex` parameter
- Double URL encoding recommended for special characters
- Polyphonic characters: Use format like "重(chong2)报集团"

## Audio Specifications

### Supported Formats

| Format | Extensions | Notes |
|--------|------------|-------|
| PCM | .pcm | Uncompressed, 16-bit |
| WAV | .wav | Uncompressed with header |
| AMR | .amr | Compressed |
| M4A | .m4a | AAC-LC codec, CBR 24000-96000 bps |

### Sample Rates

| Rate | Use Case |
|------|----------|
| 16000 Hz | Standard (recommended) |
| 8000 Hz | Telephony (Mandarin only for STT) |

### Audio Requirements

- **Channels:** Mono (1 channel)
- **Bit depth:** 16-bit
- **Byte order:** Little-endian
- **Max duration:** 60 seconds (short audio), 1 hour (real-time)

## Error Codes

### STT Error Codes

| Code | Description |
|------|-------------|
| 0 | Success |
| 2000 | Data empty |
| 3300 | Input parameter error |
| 3301 | Authentication error |
| 3302 | Token invalid or expired |
| 3303 | Audio file too long |
| 3304 | Audio file too large |
| 3305 | Audio quality issue |
| 3306 | Bad audio format |
| 3307 | Audio data error |
| 3308 | Server busy |
| 3309 | Audio recognition timeout |
| 3310 | Audio recognition error |

### TTS Error Codes

| Code | Description |
|------|-------------|
| 500 | Unknown error |
| 501 | Parameter error |
| 502 | Token validation failed |
| 503 | Quota exceeded |

## Rate Limits & Quotas

### Free Tier
- Free quota available upon registration
- Quota resets monthly

### Paid Tiers
- Package pricing (次数包) - valid for 1 year
- Pay-per-use (按量后付费) - tiered pricing based on monthly usage

### Billing Units
- STT: Per request
- TTS: 120 GBK bytes = 1 billing unit (~60 Chinese characters)

## Best Practices

### Authentication
1. Cache access tokens (valid for 30 days)
2. Refresh before expiration
3. Never expose API Key and Secret Key in client code
4. Use HTTPS for token exchange

### STT Optimization
1. Use RAW method instead of JSON for better efficiency
2. Send audio in recommended chunk sizes (160ms for WebSocket)
3. Keep WebSocket connections alive (send data within 5 seconds)
4. Use appropriate dev_pid for the target language

### TTS Optimization
1. Split long text into multiple requests (max 1024 GBK bytes)
2. Use appropriate voice for the content type
3. Cache common phrases
4. Pre-synthesize frequently used audio

## Implementation Plan

### Module Structure
```
src/core/stt/baidu/
├── mod.rs           # Module exports
├── config.rs        # Configuration types
├── messages.rs      # WebSocket message types
└── client.rs        # STT client implementation

src/core/tts/baidu/
├── mod.rs           # Module exports
├── config.rs        # Configuration types
└── provider.rs      # TTS provider implementation
```

### Implementation Steps

1. **Create STT config module**
   - BaiduSttConfig with OAuth credentials
   - BaiduSttModel enum (Mandarin, English, Cantonese, Sichuan)
   - BaiduAudioFormat enum

2. **Create STT messages module**
   - WebSocket start/finish/cancel frames
   - Response parsing

3. **Implement STT client**
   - Token management
   - WebSocket connection
   - Audio streaming

4. **Create TTS config module**
   - BaiduTtsConfig with voice options
   - BaiduVoice enum

5. **Implement TTS provider**
   - REST API calls
   - Audio format handling

6. **Register in plugin system**
   - Add to builtin providers
   - Configure aliases

### Testing Plan

1. **Unit Tests**
   - Config parsing
   - Message serialization/deserialization
   - Token management

2. **Integration Tests**
   - WebSocket connection flow
   - Audio streaming
   - TTS synthesis

3. **Error Handling Tests**
   - Invalid credentials
   - Network failures
   - Rate limiting

## References

- [Baidu AI Open Platform](https://ai.baidu.com/)
- [Speech Recognition Documentation](https://ai.baidu.com/ai-doc/SPEECH/)
- [Real-time ASR WebSocket API](https://github.com/Baidu-AIP/speech_realtime_api)
- [Speech Demo Repository](https://github.com/Baidu-AIP/speech-demo)
- [Pricing Details](https://cloud.baidu.com/product-price/speech.html)
