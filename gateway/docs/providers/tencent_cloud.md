# Tencent Cloud Speech Integration

> **Provider #40** | Batch 6: China & East Asia
> **Status:** Implementation in Progress
> **Last Updated:** 2026-01-13

## Overview

Tencent Cloud Speech provides comprehensive speech services including Automatic Speech Recognition (ASR) and Text-to-Speech (TTS). Powered by the same technology behind WeChat and Honor of Kings, it offers industry-leading 97% word recognition accuracy with sub-500ms latency.

## Provider Information

| Attribute | Value |
|-----------|-------|
| Provider Name | Tencent Cloud Speech |
| Website | https://cloud.tencent.com/product/asr |
| API Documentation | https://www.tencentcloud.com/document/product/1118 (ASR), https://www.tencentcloud.com/document/product/1154 (TTS) |
| Pricing | https://cloud.tencent.com/product/asr/pricing |
| Regions | China, Global |
| Languages | Chinese (Mandarin, Cantonese, Shanghainese), English, Japanese, Korean, Thai, Vietnamese, Indonesian |

## Capabilities Matrix

| Capability | Supported | Notes |
|------------|-----------|-------|
| STT (Real-time WebSocket) | YES | WebSocket streaming ASR |
| STT (Short Audio) | YES | REST API for batch audio |
| TTS (Sync) | YES | REST API TextToVoice |
| TTS (Async Long) | YES | CreateTtsTask for up to 100,000 chars |
| Voice Cloning | NO | Custom voices available for enterprise |
| Custom Vocabulary | YES | Hot word enhancement |

## Authentication

### Signature Authentication (HMAC-SHA1)

Tencent Cloud uses HMAC-SHA1 signature authentication with the following parameters:

**Required Credentials:**
- `secretid` - Tencent Cloud Secret ID
- `secretkey` - Tencent Cloud Secret Key
- `appid` - Application ID

**Signature Generation Steps:**

1. Sort parameters alphabetically
2. Generate parameter string: `key1=value1&key2=value2`
3. Generate signature: `Base64(HMAC-SHA1(secretkey, param_string))`
4. URL-encode signature

**Example:**
```python
import hmac
import hashlib
import base64
import time

params = {
    'secretid': 'your_secret_id',
    'timestamp': int(time.time()),
    'expired': int(time.time()) + 86400,
    'nonce': random.randint(1, 9999999999),
    'engine_model_type': '16k_zh',
    'voice_id': 'unique_voice_id',
    'voice_format': 4,  # speex
}

# Sort and generate param string
param_str = '&'.join(f'{k}={v}' for k, v in sorted(params.items()))

# Generate signature
signature = base64.b64encode(
    hmac.new(secret_key.encode(), param_str.encode(), hashlib.sha1).digest()
).decode()
```

## STT APIs

### 1. Real-time Speech Recognition (WebSocket)

For streaming audio recognition with real-time results.

**Endpoint:**
```
wss://asr.cloud.tencent.com/asr/v2/<appid>?{request_parameters}
```

**Request Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| secretid | string | Yes | Tencent Cloud Secret ID |
| timestamp | int | Yes | UNIX timestamp (seconds) |
| expired | int | Yes | Signature expiration time |
| nonce | int | Yes | Random positive integer (max 10 digits) |
| engine_model_type | string | Yes | Model: 8k_zh, 16k_zh, 16k_en, etc. |
| voice_id | string | Yes | 16-character unique audio ID |
| voice_format | int | No | Audio format (default: 4) |
| signature | string | Yes | Authentication signature |
| needvad | int | No | Voice detection (0=off, 1=on) |
| hotword_id | string | No | Custom vocabulary ID |
| filter_dirty | int | No | Filter profanity (0=off, 1=on) |
| filter_modal | int | No | Filter modal particles |
| filter_punc | int | No | Filter punctuation |
| word_info | int | No | Word timestamps (0-2) |
| vad_silence_time | int | No | Silence threshold (240-2000ms) |

**Supported Engine Models:**

| Model | Language | Sample Rate |
|-------|----------|-------------|
| 8k_zh | Mandarin Chinese | 8000 Hz |
| 8k_zh_s | Mandarin (short audio) | 8000 Hz |
| 16k_zh | Mandarin Chinese | 16000 Hz |
| 16k_zh_video | Chinese (video) | 16000 Hz |
| 16k_en | English | 16000 Hz |
| 16k_ca | Cantonese | 16000 Hz |
| 16k_ja | Japanese | 16000 Hz |
| 16k_ko | Korean | 16000 Hz |
| 16k_th | Thai | 16000 Hz |
| 16k_vi | Vietnamese | 16000 Hz |
| 16k_id | Indonesian | 16000 Hz |

**Supported Audio Formats (voice_format):**

| Value | Format | Notes |
|-------|--------|-------|
| 1 | PCM | Raw audio |
| 4 | Speex | Default, recommended |
| 6 | SILK | WeChat format |
| 8 | MP3 | Compressed |
| 10 | OPUS | Low latency |
| 12 | WAV | With header |
| 14 | M4A | AAC format |
| 16 | AAC | Compressed |

**Audio Requirements:**
- Sample rate: 8000 Hz or 16000 Hz
- Bit depth: 16 bits
- Channels: Mono only
- Recommended chunk size: 40ms (640 bytes at 8k, 1280 bytes at 16k)

**Response Format:**
```json
{
  "code": 0,
  "message": "success",
  "voice_id": "unique_voice_id",
  "message_id": "abc123",
  "result": {
    "slice_type": 1,
    "index": 0,
    "start_time": 0,
    "end_time": 1000,
    "voice_text_str": "recognized text",
    "word_size": 2,
    "word_list": [
      {"word": "hello", "start_time": 0, "end_time": 500, "stable_flag": 1}
    ]
  },
  "final": 0
}
```

**Response Fields:**

| Field | Type | Description |
|-------|------|-------------|
| code | int | 0 = success, non-zero = error |
| message | string | Status message |
| voice_id | string | Unique audio identifier |
| result.slice_type | int | 0=interim, 1=one-sentence end, 2=final |
| result.voice_text_str | string | Recognized text |
| result.word_list | array | Word-level timestamps |
| final | int | 1 = recognition complete |

**Error Codes:**

| Code | Description |
|------|-------------|
| 4001 | Invalid parameters |
| 4002 | Authentication failure |
| 4003 | Service not activated |
| 4004 | Insufficient quota |
| 4005 | Service not supported |
| 4006 | Audio too long |
| 4007 | Audio decoding failed |
| 4008 | Client upload timeout |
| 5000 | Server error |
| 5001 | Server busy |
| 5002 | Server timeout |

### 2. Short Audio Recognition (REST API)

For recognizing pre-recorded audio files.

**Endpoint:**
```
POST https://asr.cloud.tencent.com/asr/v1/<appid>
```

## TTS APIs

### 1. TextToVoice (Synchronous)

For converting short text to speech synchronously.

**Endpoint:**
```
POST https://tts.intl.tencentcloudapi.com
```

**Request Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| Action | string | Yes | Fixed: "TextToVoice" |
| Version | string | Yes | Fixed: "2019-08-23" |
| Text | string | Yes | Text to synthesize (max 150 Chinese chars / 500 letters) |
| SessionId | string | Yes | Request identifier (UUID format) |
| Volume | float | No | Volume [0, 10], default 0 |
| Speed | float | No | Speed [-2, 6], default 0 |
| VoiceType | int | No | Voice ID (see voice list) |
| PrimaryLanguage | int | No | 1=Chinese (default), 2=English |
| SampleRate | int | No | 16000 (default) or 8000 |
| Codec | string | No | "wav", "mp3", or "pcm" |
| EnableSubtitle | bool | No | Enable timestamps |

**Speed Values:**

| Value | Speed |
|-------|-------|
| -2 | 0.6x |
| -1 | 0.8x |
| 0 | 1.0x (default) |
| 1 | 1.2x |
| 2 | 1.5x |
| 6 | 2.5x |

**Standard Voices (VoiceType):**

| ID | Name | Gender | Language |
|----|------|--------|----------|
| 1001 | Intelligent Woman | Female | Chinese |
| 1002 | Intelligent Man | Male | Chinese |
| 1003 | Mature Man | Male | Chinese |
| 1050 | WeChat Xiaowei | Female | Chinese |
| 1051 | WeChat Xiaowei (Female) | Female | Chinese |

**Premium Voices (VoiceType):**

| ID | Name | Gender | Language |
|----|------|--------|----------|
| 101001 | Intelligent Woman Premium | Female | Chinese |
| 101002 | Intelligent Man Premium | Male | Chinese |
| 101003 | Customer Service Female | Female | Chinese |
| 101004 | Customer Service Male | Male | Chinese |
| 101005 | News Female | Female | Chinese |
| 101006 | News Male | Male | Chinese |
| 101015 | Cantonese Female | Female | Cantonese |
| 101016 | Cantonese Male | Male | Cantonese |
| 101017 | Sichuan Dialect | Female | Sichuan |
| 101050 | English Female | Female | English |
| 101051 | English Male | Male | English |

**Response Format:**
```json
{
  "Response": {
    "Audio": "base64_encoded_audio",
    "SessionId": "request_session_id",
    "RequestId": "unique_request_id",
    "Subtitles": [
      {
        "Text": "word",
        "BeginTime": 0,
        "EndTime": 500,
        "BeginIndex": 0,
        "EndIndex": 1,
        "Phoneme": "word_phoneme"
      }
    ]
  }
}
```

**Error Codes:**

| Code | Description |
|------|-------------|
| InvalidParameterValue.TextEmpty | Text is empty |
| InvalidParameterValue.TextTooLong | Text exceeds limit |
| UnsupportedOperation.ServerNotOpen | Service not activated |
| LimitExceeded.AccessLimit | Rate limit exceeded (20 req/sec) |

### 2. CreateTtsTask (Asynchronous Long Text)

For synthesizing long text up to 100,000 characters asynchronously.

**Endpoint:**
```
POST https://tts.tencentcloudapi.com
```

**Request Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| Action | string | Yes | Fixed: "CreateTtsTask" |
| Version | string | Yes | Fixed: "2019-08-23" |
| Text | string | Yes | Text to synthesize (max 100,000 chars) |
| VoiceType | int | No | Voice ID |
| Volume | float | No | Volume [0, 10] |
| Speed | float | No | Speed [-2, 6] |
| Codec | string | No | "mp3", "wav", "pcm" |
| CallbackUrl | string | No | Callback URL for result |

**Response:**
```json
{
  "Response": {
    "TaskId": "task_id_string",
    "RequestId": "unique_request_id"
  }
}
```

**Polling for Results:**
Use `DescribeTtsTaskStatus` API with `TaskId` to check completion status.

## Rate Limits & Quotas

### ASR
- Real-time: Concurrent connections limit per account
- Short audio: QPS limit per account

### TTS
- TextToVoice: 20 requests/second
- CreateTtsTask: Results within 3 hours, stored for 24 hours

## Pricing

### ASR Pricing
- Real-time streaming: Pay per duration of recognized audio
- Batch recognition: Pay per request

### TTS Pricing
- Standard voices: Lower cost
- Premium voices: Higher cost for better quality
- Billed daily based on character count

## Best Practices

### Authentication
1. Use HMAC-SHA1 signature with proper encoding
2. Refresh signatures before expiration
3. Never expose Secret Key in client code

### ASR Optimization
1. Send audio in 40ms chunks (640 bytes at 8k, 1280 bytes at 16k)
2. Enable VAD for long audio sessions
3. Use appropriate engine model for target language
4. Implement reconnection logic with exponential backoff

### TTS Optimization
1. Keep text within limits (150 Chinese chars / 500 letters for sync)
2. Use CreateTtsTask for long text
3. Cache frequently used audio clips
4. Use appropriate voice type for content

## Implementation Plan

### Module Structure
```
src/core/stt/tencent/
├── mod.rs           # Module exports
├── config.rs        # Configuration types
├── messages.rs      # WebSocket message types
└── client.rs        # STT client implementation

src/core/tts/tencent/
├── mod.rs           # Module exports
├── config.rs        # Configuration types
└── provider.rs      # TTS provider implementation
```

### Implementation Steps

1. **Create STT config module**
   - TencentSttConfig with credentials
   - TencentEngineModel enum (16k_zh, 16k_en, etc.)
   - TencentAudioFormat enum

2. **Create STT messages module**
   - WebSocket request/response messages
   - Signature generation utility

3. **Implement STT client**
   - WebSocket connection with signature auth
   - Audio streaming with proper chunking
   - Result parsing and callbacks

4. **Create TTS config module**
   - TencentTtsConfig with voice options
   - TencentVoiceType enum (standard, premium)
   - Audio format options

5. **Implement TTS provider**
   - REST API client for TextToVoice
   - Optional: CreateTtsTask for long text
   - Base64 audio decoding

6. **Register in plugin system**
   - Add to builtin providers
   - Configure aliases

### Testing Plan

1. **Unit Tests**
   - Config validation
   - Signature generation
   - Message serialization

2. **Integration Tests**
   - WebSocket connection flow
   - Audio streaming
   - TTS synthesis

3. **Error Handling Tests**
   - Invalid credentials
   - Network failures
   - Rate limiting

## References

- [Tencent Cloud ASR Documentation](https://www.tencentcloud.com/document/product/1118)
- [Real-Time ASR WebSocket API](https://www.tencentcloud.com/document/product/1118/53937)
- [Tencent Cloud TTS Documentation](https://www.tencentcloud.com/document/product/1154)
- [TextToVoice API](https://www.tencentcloud.com/document/product/1154/48916)
- [Python Speech SDK](https://github.com/TencentCloud/tencentcloud-speech-sdk-python)
