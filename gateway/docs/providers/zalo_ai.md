# Zalo AI STT+TTS Provider

## Status: READY_TO_IMPLEMENT

**Last Updated:** 2026-01-14
**Provider #:** 47
**Priority:** High (Southeast Asia - Vietnam, free tier available)

## Overview

Zalo AI is a Vietnamese AI platform from VNG Corporation (Vietnam's largest tech company) specializing in speech technology for the Vietnamese language. They offer high-quality TTS optimized for Vietnamese with regional accents.

## Company Information

- **Company:** VNG Corporation (Zalo AI division)
- **Country:** Vietnam
- **Website:** https://zalo.ai / https://ai.zalo.cloud
- **Developer Portal:** https://developers.zalo.me
- **Technologies:** STT, TTS
- **Free Tier:** Available

## Text-to-Speech (TTS) API

### Endpoint (from research)

The API is accessible via REST at `api.zalo.ai`. Full endpoint structure requires developer account access.

### Authentication

- **Method:** API Key header
- **Header:** `apikey: {your_api_key}`
- **Get Key:** https://zalo.ai/docs/api/text-to-audio-converter (redirects to ai.zalo.cloud)

### Voice Options

Zalo TTS supports 4 Vietnamese voices:

| Voice ID | Description | Accent |
|----------|-------------|--------|
| 1 | Female Southern (Giọng nữ miền Nam) | Southern Vietnam |
| 2 | Female Northern (Giọng nữ miền Bắc) | Northern Vietnam |
| 3 | Male Southern (Giọng nam miền Nam) | Southern Vietnam |
| 4 | Male Northern (Giọng nam miền Bắc) | Northern Vietnam |

### Parameters

| Parameter | Type | Description |
|-----------|------|-------------|
| `input` / `text` | string | Vietnamese text to synthesize |
| `speaker_id` | int | Voice ID (1-4) |
| `speed` | float | Speech rate [0.8 - 1.2], default 1.0 |

### Python Client Library

Official Python library: https://github.com/iconclub/zalo-tts

```python
from zalo_tts import ZaloTTS

# Constants for voices
ZaloTTS.SOUTH_WOMEN = 1
ZaloTTS.NORTHERN_WOMEN = 2
ZaloTTS.SOUTH_MEN = 3
ZaloTTS.NORTHERN_MEN = 4

# Usage
tts = ZaloTTS(speaker=ZaloTTS.NORTHERN_MEN, api_key="your_api_key")
tts.text_to_speech("Xin chào các bạn")
```

Or via environment variable:
```bash
export ZALO_API_KEY="your_api_key"
```

### Audio Output

- **Format:** MP3 (based on Home Assistant integration)
- **Quality:** Optimized for real-time and high-volume applications

### Use Cases

- News websites
- Voice streaming services
- Chatbots
- Virtual assistants

## Speech-to-Text (STT) API

Limited public documentation available. STT capabilities are mentioned in waav_integrations.json but detailed API specs require developer account access.

## Implementation Approach

### Option 1: Reverse Engineer from Python Library

Download and analyze the ZaloTTS Python library to extract exact API endpoints:

```bash
pip download ZaloTTS --no-deps -d ./temp
unzip ./temp/ZaloTTS-*.whl -d ./temp/zalo_src
# Analyze zalo_tts.py for endpoint details
```

### Option 2: Use as Reference Implementation

Follow the pattern from the Home Assistant integration:
- https://github.com/minhdanh/ha-zalo-tts

### Option 3: Create Developer Account

1. Register at https://developers.zalo.me
2. Create application
3. Get API key
4. Access full API documentation

## Integration Pattern (CONFIRMED)

**Source:** Extracted from ZaloTTS Python library v0.0.3

### TTS API Specification

```
Endpoint: POST https://api.zalo.ai/v1/tts/synthesize

Headers:
  apikey: {api_key}
  Content-Type: application/x-www-form-urlencoded

Body (URL-encoded):
  input={text_to_synthesize}
  speed={0.8 to 1.2}
  speaker_id={1 to 4}

Response (JSON):
{
  "error_code": 0,       // 0 = success, 155/401/500 = error
  "data": {
    "url": "https://..."  // Audio URL to download/stream
  }
}
```

### Audio Format

- **Format:** WAV
- **Sample Rate:** 16000 Hz
- **Channels:** 1 (mono)
- **Sample Width:** 2 bytes (16-bit)

### Error Codes

| Code | Meaning |
|------|---------|
| 0 | Success |
| 155 | Unknown (possibly rate limit) |
| 401 | Authentication error |
| 500 | Server error |

## Related Providers

- **FPT.AI (#48):** Similar Vietnamese speech API
- **Viettel AI (#49):** Vietnamese telecom speech services
- **Vbee (#46):** Vietnamese TTS (BLOCKED - no public API docs)

## References

- [Zalo AI Website](https://zalo.ai)
- [Zalo AI Cloud](https://ai.zalo.cloud)
- [Python Library](https://github.com/iconclub/zalo-tts)
- [Home Assistant Plugin](https://github.com/minhdanh/ha-zalo-tts)
- [Zalo Developers](https://developers.zalo.me)

## Next Steps

1. Download and analyze ZaloTTS Python library source code
2. Extract exact API endpoints and request format
3. Create developer account if needed for STT API
4. Implement TTS provider following existing pattern
5. Add STT provider if documentation becomes available
