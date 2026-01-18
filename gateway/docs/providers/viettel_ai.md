# Viettel AI Provider Documentation

> **Provider #49** | Vietnamese STT + TTS Provider
> **Company:** Viettel Group (Vietnam's largest telecommunications company)
> **Website:** https://viettelai.vn | https://viettelgroup.ai
> **Last Updated:** 2026-01-14

---

## Overview

Viettel AI is a Vietnamese AI platform developed by Viettel Group, Vietnam's largest telecommunications corporation. The platform specializes in Vietnamese language processing with speech recognition (ASR) and text-to-speech (TTS) services optimized for regional Vietnamese accents.

### Key Features

- **High Vietnamese Accuracy**: 96% accuracy for Vietnamese speech recognition
- **Regional Accent Support**: Northern, Central, and Southern Vietnamese accents
- **Deep Neural Technology**: Advanced neural network algorithms optimized for Vietnamese
- **Enterprise-Grade**: Built on Viettel's infrastructure with high security
- **11 Vietnamese Voices**: 5 Northern, 4 Southern, 2 Central regional voices

---

## Authentication

### Token-Based Authentication

Viettel AI uses token-based authentication with 180-day validity.

**Getting a Token:**
1. Register at https://viettelgroup.ai
2. Login and navigate to Dashboard > Token
3. Create a new token
4. Use token in API requests

**Token Header:**
```
token: YOUR_TOKEN_HERE
```

**Note:** Tokens expire after 180 days and must be renewed.

---

## Text-to-Speech (TTS) API

### Synthesis Endpoint

```
POST https://viettelgroup.ai/voice/api/tts/v1/rest/syn
```

### Voice List Endpoint

```
GET https://viettelgroup.ai/voice/api/tts/v1/rest/voices
```

### Request Headers

| Header | Value | Required |
|--------|-------|----------|
| `Content-Type` | `application/json` | Yes |
| `token` | Your API token | Yes |

### Request Body

```json
{
  "text": "Xin chào, tôi là trợ lý ảo Viettel.",
  "voice": "doanngocle",
  "id": "1",
  "without_filter": false,
  "speed": 1.0,
  "tts_return_option": 2
}
```

### Request Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `text` | string | Yes | Text to synthesize (Vietnamese) |
| `voice` | string | Yes | Voice name (e.g., "doanngocle") |
| `id` | string | No | Request identifier |
| `without_filter` | boolean | No | Disable text preprocessing |
| `speed` | float | No | Speech speed (default: 1.0) |
| `tts_return_option` | integer | No | Return format option |

### Available Voices

Viettel AI provides 11 Vietnamese voices categorized by region:

**Northern Voices (5 voices):**
- 3 female voices
- 2 male voices
- Example: `doanngocle`

**Southern Voices (4 voices):**
- 3 female voices
- 1 male voice

**Central Voices (2 voices):**
- 1 female voice
- 1 male voice

**Note:** Get the complete voice list via the voices endpoint.

### cURL Example

```bash
curl --request POST \
  --header "Content-Type: application/json" \
  --header "token: YOUR_TOKEN" \
  --data '{
    "text": "Xin chào",
    "voice": "doanngocle",
    "speed": 1.0,
    "tts_return_option": 2
  }' \
  https://viettelgroup.ai/voice/api/tts/v1/rest/syn > output.wav
```

### Response

Returns audio data directly in WAV format.

---

## Speech-to-Text (STT/ASR) API

### File Decode Endpoint

```
POST https://viettelgroup.ai/voice/api/asr/v1/rest/decode_file
```

### Request Headers

| Header | Value | Required |
|--------|-------|----------|
| `token` | Your API token | Yes (empty for anonymous) |
| `model` | Recognition model code | No (URL parameter) |

### PCM Format Headers

For PCM audio files, include these additional headers:

| Header | Type | Description |
|--------|------|-------------|
| `sample_rate` | float | Sample rate (e.g., 16000) |
| `format` | string | Audio format (e.g., "S16LE" for signed 16-bit little endian) |
| `num_of_channels` | integer | Number of audio channels (1 for mono) |

### cURL Example

```bash
curl --request POST \
  --header "token: YOUR_TOKEN" \
  -F "file=@/path/to/audio.wav" \
  https://viettelgroup.ai/voice/api/asr/v1/rest/decode_file
```

### PCM File Example

```bash
curl --request POST \
  --header "token: YOUR_TOKEN" \
  --header "sample_rate: 16000" \
  --header "format: S16LE" \
  --header "num_of_channels: 1" \
  -F "file=@/path/to/audio.pcm" \
  https://viettelgroup.ai/voice/api/asr/v1/rest/decode_file
```

### Python Example

```python
import requests

url = "https://viettelgroup.ai/voice/api/asr/v1/rest/decode_file"
headers = {
    'token': 'YOUR_TOKEN',
    # For PCM files:
    # 'sample_rate': '16000',
    # 'format': 'S16LE',
    # 'num_of_channels': '1',
}
files = {'file': open('audio.wav', 'rb')}
response = requests.post(url, files=files, headers=headers)
print(response.json())
```

### Response

```json
{
  "status": 0,
  "result": "Xin chào, tôi là Viettel AI"
}
```

### Supported Audio Formats

| Format | Extension | Notes |
|--------|-----------|-------|
| WAV | .wav | Recommended |
| PCM | .pcm | Requires additional headers |
| MP3 | .mp3 | Compressed audio |

### Supported Input Types

- Direct recording
- Phone recording
- Operator recording

---

## Implementation Details

### TTS Return Options

| Value | Description |
|-------|-------------|
| 1 | Return audio URL |
| 2 | Return audio binary |

### Speed Parameter

- Default: 1.0
- Range: Typically 0.5 to 2.0

### Error Handling

| Status Code | Meaning |
|-------------|---------|
| 200 | Success |
| 401 | Invalid or expired token |
| 400 | Bad request |
| 500 | Server error |

---

## Integration Architecture

```
┌────────────────────────────────────────────────────────────────┐
│                     Bud WaaV Gateway                           │
├────────────────────────────────────────────────────────────────┤
│                                                                │
│  ┌──────────────────┐          ┌──────────────────┐           │
│  │   ViettelTts     │          │   ViettelStt     │           │
│  │   (BaseTTS)      │          │   (BaseSTT)      │           │
│  └────────┬─────────┘          └────────┬─────────┘           │
│           │                             │                      │
│           ▼                             ▼                      │
│  ┌──────────────────────────────────────────────────────────┐ │
│  │                    HTTP Client (reqwest)                  │ │
│  └──────────────────────────────────────────────────────────┘ │
│                             │                                  │
└─────────────────────────────┼──────────────────────────────────┘
                              │
                              ▼
              ┌───────────────────────────────┐
              │      Viettel AI Cloud         │
              │   viettelgroup.ai/voice/api   │
              ├───────────────────────────────┤
              │  TTS: /tts/v1/rest/syn        │
              │  STT: /asr/v1/rest/decode_file│
              └───────────────────────────────┘
```

---

## Rust Implementation Notes

### ViettelTtsConfig

```rust
pub struct ViettelTtsConfig {
    pub api_key: String,          // Token for authentication
    pub voice: String,            // Voice name (e.g., "doanngocle")
    pub speed: f32,               // Speech speed (default: 1.0)
    pub without_filter: bool,     // Disable text preprocessing
    pub tts_return_option: u8,    // Return format (2 = binary)
    pub request_timeout_secs: u64,
}
```

### ViettelSttConfig

```rust
pub struct ViettelSttConfig {
    pub api_key: String,          // Token for authentication
    pub sample_rate: u32,         // Audio sample rate
    pub format: String,           // Audio format (e.g., "S16LE")
    pub channels: u16,            // Number of channels
    pub model: Option<String>,    // Optional recognition model
    pub request_timeout_secs: u64,
}
```

---

## Rate Limits & Quotas

- Token validity: 180 days
- Free tier: 60 minutes of STT for new accounts
- Contact Viettel for enterprise pricing: viettelai@viettel.com.vn

---

## Security Considerations

- All API calls use HTTPS
- Token-based authentication with expiration
- Data processed on Viettel's secure infrastructure
- Suitable for enterprise deployments

---

## References

- [Viettel AI Official Site](https://viettelai.vn/en)
- [Viettel Group AI Platform](https://viettelgroup.ai)
- [TTS Documentation](https://viettelgroup.ai/en/document/tts)
- [ASR Documentation](https://viettelgroup.ai/en/document/rest)
- [Speech Synthesis Service](https://viettelgroup.ai/en/service/tts)
- [Speech Recognition Service](https://viettelgroup.ai/en/service/asr)

---

## Changelog

| Date | Change |
|------|--------|
| 2026-01-14 | Initial documentation |
