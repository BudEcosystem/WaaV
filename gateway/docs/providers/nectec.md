# NECTEC (AI for Thai) Provider Documentation

> **Provider #52** | Thai Government STT + TTS Provider
> **Organization:** NECTEC (National Electronics and Computer Technology Center)
> **Website:** https://aiforthai.in.th
> **Status:** ✅ DONE - 83 tests (39 STT + 44 TTS)
> **Last Updated:** 2026-01-17

---

## Overview

NECTEC (National Electronics and Computer Technology Center) is a Thai government research institution under NSTDA (National Science and Technology Development Agency). They provide AI for Thai, a platform offering free AI services for Thai language processing including Speech-to-Text (Partii) and Text-to-Speech (VAJA).

### Key Features

- **Government Initiative**: Free service for Thai language processing
- **Thai Language Optimized**: Native support for Thai language
- **Multiple STT Engines**: Partii4 (legacy) and Partii5 (newer)
- **VAJA TTS**: Text-to-speech with male/female voices
- **REST API**: Simple HTTP-based API
- **Python SDK**: Official `aift` library available on PyPI

---

## Authentication

### API Key Authentication

AI for Thai uses API key authentication via the `Apikey` header.

**Getting an API Key:**
1. Register at https://aiforthai.in.th
2. Navigate to Developer section
3. Generate an API key
4. Use key in API requests

**API Key Header:**
```
Apikey: YOUR_API_KEY
```

---

## Speech-to-Text (STT) API

### Partii5 Endpoint (Recommended)

```
POST https://api.aiforthai.in.th/partii5-poc
```

### Partii4 Endpoint (Legacy)

```
POST https://api.aiforthai.in.th/partii-webapi
```

### Request Headers

| Header | Value | Required |
|--------|-------|----------|
| `Apikey` | Your API key | Yes |
| `X-lib` | Client library identifier | No |
| `Content-Type` | `multipart/form-data` | Yes |

### Audio Requirements

| Parameter | Value |
|-----------|-------|
| Format | WAV only |
| Sample Rate | 16,000 Hz |
| Bit Depth | 16-bit Linear PCM |
| Channels | 1 (Mono) |
| Max Duration | 30 seconds |
| Max File Size | 1 MB |

### Partii5 Request

```bash
curl -X POST "https://api.aiforthai.in.th/partii5-poc" \
  -H "Apikey: YOUR_API_KEY" \
  -F "file=@audio.wav;type=audio/wav"
```

### Partii5 Response

```json
{
  "content": "สวัสดีครับ วันนี้อากาศดีมาก"
}
```

### Partii4 Request

```bash
curl -X POST "https://api.aiforthai.in.th/partii-webapi" \
  -H "Apikey: YOUR_API_KEY" \
  -F "wavfile=@audio.wav;type=audio/wav" \
  -F "outputlevel=--uttlevel" \
  -F "outputformat=--txt"
```

### Partii4 Parameters

| Parameter | Type | Description |
|-----------|------|-------------|
| `wavfile` | file | WAV audio file |
| `outputlevel` | string | Output level: `--uttlevel` (utterance level) |
| `outputformat` | string | Output format: `--txt`, `--json` |

### Partii4 Response

```json
{
  "message": "สวัสดีครับ วันนี้อากาศดีมาก"
}
```

---

## Text-to-Speech (TTS) API

### VAJA9 Endpoint

```
POST https://api.aiforthai.in.th/vaja9/synth_audiovisual
```

### Request Headers

| Header | Value | Required |
|--------|-------|----------|
| `Apikey` | Your API key | Yes |
| `X-lib` | Client library identifier | No |
| `Content-Type` | `application/json` | Yes |

### Request Body

```json
{
  "input_text": "สวัสดีครับ",
  "speaker": 0,
  "phrase_break": 0,
  "audiovisual": 0
}
```

### Request Parameters

| Parameter | Type | Description |
|-----------|------|-------------|
| `input_text` | string | Thai text to synthesize (max 300 chars per request) |
| `speaker` | integer | Voice selection: 0 = male, 1 = female |
| `phrase_break` | integer | Phrase break control (0 = default) |
| `audiovisual` | integer | Audiovisual mode (0 = audio only) |

### Text Length Limits

| Scenario | Limit |
|----------|-------|
| Per request | 300 characters |
| Long text | Split into chunks and concatenate |

### Response

```json
{
  "wav_url": "https://api.aiforthai.in.th/temp/audio_123456.wav"
}
```

### Audio Download

After receiving the response, download the audio file from `wav_url`:

```bash
curl -H "Apikey: YOUR_API_KEY" "https://api.aiforthai.in.th/temp/audio_123456.wav" -o output.wav
```

### Available Voices

| Speaker ID | Voice | Language |
|------------|-------|----------|
| 0 | Male | Thai |
| 1 | Female | Thai |

### cURL Example

```bash
# Step 1: Request synthesis
curl -X POST "https://api.aiforthai.in.th/vaja9/synth_audiovisual" \
  -H "Apikey: YOUR_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"input_text": "สวัสดีครับ", "speaker": 0}'

# Step 2: Download audio from wav_url in response
curl -H "Apikey: YOUR_API_KEY" "https://api.aiforthai.in.th/temp/audio_123.wav" -o output.wav
```

---

## Rate Limits

AI for Thai operates on a freemium model with rate limiting:

| Tier | Description |
|------|-------------|
| Per Minute | User rate limit per minute |
| Per Day | User rate limit per day |
| Per Month | User rate limit per month |
| System-wide | Global rate limit per second |

**Notes:**
- Free service for education and testing only
- Commercial use prohibited without license
- Rate limits may be adjusted without notice
- Contact business@nectec.or.th for commercial licensing

---

## Error Handling

### Common HTTP Status Codes

| Code | Description |
|------|-------------|
| 200 | Success |
| 401 | Invalid or missing API key |
| 429 | Rate limit exceeded |
| 500 | Internal server error |

---

## Python SDK

### Installation

```bash
pip install aift
```

### Setup

```python
from aift import setting
setting.set_api_key('YOUR_API_KEY')
```

### STT Usage

```python
from aift.speech.stt import partii4, partii5

# Using Partii5 (recommended)
result = partii5.transcribe('audio.wav', return_json=True)
print(result['content'])

# Using Partii4 (legacy)
result = partii4.transcribe('audio.wav', return_json=True)
print(result['message'])
```

### TTS Usage

```python
from aift.speech import tts

# Male voice (speaker=0)
tts.convert('สวัสดีครับ', 'output.wav', speaker=0)

# Female voice (speaker=1)
tts.convert('สวัสดีค่ะ', 'output.wav', speaker=1)
```

---

## WaaV Gateway Implementation ✅

### STT Implementation (39 tests)

1. **File:** `src/core/stt/nectec/config.rs` ✅
   - `NectecSttConfig` struct
   - `NectecSttModel` enum (Partii4/Partii5)
   - Audio format validation
   - Response types for both engines

2. **File:** `src/core/stt/nectec/client.rs` ✅
   - `NectecStt` implementing `BaseSTT` trait
   - Multipart form data upload
   - WAV file wrapping with headers
   - Response parsing for both engines

3. **File:** `src/core/stt/nectec/mod.rs` ✅
   - Module exports and unit tests

### TTS Implementation (44 tests)

1. **File:** `src/core/tts/nectec/config.rs` ✅
   - `NectecTtsConfig` struct
   - `NectecVoice` enum (Male/Female)
   - `Vaja9Request`/`Vaja9Response` types
   - Text chunking for long text (300 char limit)

2. **File:** `src/core/tts/nectec/client.rs` ✅
   - `NectecTts` implementing `BaseTTS` trait
   - Two-step synthesis (POST for URL, GET for audio)
   - WAV audio handling (22kHz PCM16)
   - Auto-connect on speak

3. **File:** `src/core/tts/nectec/mod.rs` ✅
   - Module exports and unit tests

### Provider Registration ✅

- **Provider ID:** `nectec`
- **STT Aliases:** `aiforthai`, `ai4thai`, `partii`, `partii5`, `partii4`, `nectec-stt`
- **TTS Aliases:** `aiforthai-tts`, `ai4thai-tts`, `vaja9`, `vaja`, `nectec-tts`

### Configuration Example

```yaml
# config.yaml
stt:
  provider: nectec
  api_key: ${NECTEC_API_KEY}
  model: partii5  # or partii4

tts:
  provider: nectec
  api_key: ${NECTEC_API_KEY}
  voice_id: male  # or female
```

### Environment Variables

| Variable | Description |
|----------|-------------|
| `NECTEC_API_KEY` | AI for Thai API key |
| `AIFORTHAI_API_KEY` | Alias for NECTEC_API_KEY |

---

## References

- [AI for Thai Platform](https://aiforthai.in.th)
- [NECTEC Website](https://www.nectec.or.th)
- [Partii Project](http://party.openservice.in.th)
- [VAJA TTS](https://www.nectec.or.th/en/innovation/service-innovation/vaja8.html)
- [aift Python Library](https://pypi.org/project/aift/)
- [AIforThai GitHub](https://github.com/AIforThai/aiforthai)
