# SberDevices SaluteSpeech Provider Documentation

> **Provider:** SberDevices (Sber)
> **Product:** SaluteSpeech (formerly SmartSpeech)
> **Region:** Russia, CIS
> **Research Date:** 2026-01-13

---

## Overview

SaluteSpeech is Sber's cloud-based speech recognition and synthesis platform. It provides high-quality Russian TTS using a modified Tacotron 2 architecture with BERT-based prosody control, resulting in natural-sounding speech.

---

## Capabilities Matrix

| Capability | Supported | Notes |
|------------|-----------|-------|
| STT | YES | Streaming (gRPC) + Sync (REST) |
| TTS | YES | REST API with SSML support |
| Audio-to-Audio | NO | - |
| Voice Cloning | NO | Brand voice requires contact |
| Streaming STT | YES | gRPC bidirectional |
| Streaming TTS | YES | Via gRPC |

---

## Authentication

### OAuth 2.0 Token-Based

**OAuth Endpoint:** `https://ngw.devices.sberbank.ru:9443/api/v2/oauth`

**Process:**
1. Create project at [Sber Studio](https://developers.sber.ru/studio/login)
2. Generate Client ID and Client Secret
3. Create Base64 authorization key: `Base64(ClientID:ClientSecret)`
4. Request access token with Basic auth

**Token Request:**
```http
POST /api/v2/oauth HTTP/1.1
Host: ngw.devices.sberbank.ru:9443
Content-Type: application/x-www-form-urlencoded
Accept: application/json
RqUID: <unique-uuid4>
Authorization: Basic <base64-credentials>

scope=SALUTE_SPEECH_PERS
```

**Token Response:**
```json
{
  "access_token": "<jwt-token>",
  "expires_at": 1704067200000
}
```

**Token Validity:** 30 minutes

**Scopes:**
- `SALUTE_SPEECH_PERS` - Individuals (5 concurrent streams)
- `SALUTE_SPEECH_CORP` - Organizations postpaid (10 concurrent streams)
- `SALUTE_SPEECH_B2B` - Organizations prepaid
- `SBER_SPEECH` - Legacy enterprise

---

## STT (Speech Recognition)

### REST API - Synchronous

**Endpoint:** `POST https://smartspeech.sber.ru/rest/v1/speech:recognize`

**Limits:**
- Max audio size: 2 MB
- Max duration: 1 minute
- Multi-channel: first channel only

**Request Headers:**
```http
Authorization: Bearer <access_token>
Content-Type: audio/x-pcm;bit=16;rate=16000
```

**Response:**
```json
{
  "result": ["Распознанный текст"],
  "status": 200
}
```

### gRPC API - Streaming

**Endpoint:** `smartspeech.sber.ru:443`

**Method:** `Recognize` (bidirectional streaming)

**Proto Location:** Available at Sber developer docs

**Request Message (`RecognitionRequest`):**
- `options`: RecognitionOptions (first message)
- `audio_chunk`: bytes (subsequent messages)

**Max Message Size:** 4 MB
**Max Chunk Duration:** 2 seconds

### Supported Languages

| Code | Language |
|------|----------|
| ru-RU | Russian (default) |
| en-US | English |
| kk-KZ | Kazakh |
| ky-KG | Kyrgyz |
| uz-UZ | Uzbek |

### Supported Audio Formats

| Format | Max Channels | Sample Rate |
|--------|--------------|-------------|
| PCM_S16LE (WAV) | 8 | 8,000-96,000 Hz |
| OPUS | 1 | Any |
| MP3 | 2 | Any |
| FLAC | 8 | Any |
| ALAW | 8 | 8,000-96,000 Hz |
| MULAW | 8 | 8,000-96,000 Hz |

---

## TTS (Speech Synthesis)

### REST API

**Endpoint:** `POST https://smartspeech.sber.ru/rest/v1/text:synthesize`

**Max Request:** 4,000 characters (including SSML markup)

**Request Headers:**
```http
Authorization: Bearer <access_token>
Content-Type: application/text
```

**Query Parameters:**
- `voice`: Voice ID (e.g., `Nec_24000`)
- `format`: Output format (`wav`, `opus`, `mp3`)
- `sample_rate`: 8000 or 24000

### Available Voices

| ID | Name | Gender | Language |
|----|------|--------|----------|
| Nec | Natalia | Female | ru-RU |
| Bys | Boris | Male | ru-RU |
| May | Martha | Female | ru-RU |
| Tur | Taras | Male | ru-RU |
| Ost | Alexandra | Female | ru-RU |
| Pon | Sergey | Male | ru-RU |
| Kin | Kira | Female | en-US |

**Voice Format:** `{VoiceID}_{SampleRate}` (e.g., `Nec_24000`, `Bys_8000`)

### Sample Rates

- 8000 Hz (telephony)
- 24000 Hz (high quality)

### SSML Support

Supported SSML tags:
- `<speak>` - Root element
- `<break>` - Pauses
- `<prosody>` - Rate, pitch, volume
- `<say-as>` - Number, date formats
- `<sub>` - Substitutions
- `<phoneme>` - Phonetic pronunciation

---

## Rate Limits

| Plan | Concurrent Streams | Notes |
|------|-------------------|-------|
| SALUTE_SPEECH_PERS | 5 | Individuals |
| SALUTE_SPEECH_CORP | 10 | Organizations |

### Free Tier (Non-Commercial)
- STT: 100 minutes/month
- TTS: 200,000 characters/month

---

## Pricing

| Service | Price (RUB) |
|---------|-------------|
| STT | 1,200 RUB / 1,000 minutes |
| TTS | 1,000 RUB / 1,000,000 characters |

---

## Integration Pattern

### Recommended Approach

**STT:** REST API for simplicity (streaming gRPC for real-time)
**TTS:** REST API with streaming response

### Reference Implementation

Similar to: Yandex SpeechKit (Russian provider, OAuth auth)

### Complexity

- **STT:** Medium (OAuth + REST/gRPC)
- **TTS:** Low (OAuth + REST)

### Estimated LOC

- STT: ~500 lines
- TTS: ~400 lines

---

## Implementation Notes

1. **OAuth Token Caching:** Cache tokens for ~29 minutes, refresh when < 1 minute remaining
2. **RqUID Header:** Required unique UUID4 for each OAuth request
3. **TLS Required:** All endpoints use HTTPS/TLS
4. **Voice ID Format:** Append sample rate to voice ID (e.g., `Nec_24000`)
5. **SSML:** Requires `<speak>` wrapper tag

---

## SDKs and References

- **Python:** [salute-speech](https://pypi.org/project/salute-speech/)
- **TypeScript:** [@lobbyboy/salutespeech-sdk](https://www.npmjs.com/package/@lobbyboy/salutespeech-sdk)
- **C#/.NET:** [SaluteSpeechTools](https://github.com/MaximGorshunov/SaluteSpeechTools)
- **Go:** [salute_speech_api](https://pkg.go.dev/github.com/saintbyte/salute_speech_api)
- **Documentation:** [developers.sber.ru/docs/ru/salutespeech](https://developers.sber.ru/docs/ru/salutespeech)

---

## Blockers/Concerns

1. **Russia Focus:** Primary market is Russia/CIS
2. **Documentation:** Mostly in Russian
3. **gRPC Proto:** Need to fetch proto files from Sber docs
4. **No WebSocket:** REST + gRPC only (no native WebSocket)

---

## Test Plan

1. Unit tests for OAuth token generation
2. Unit tests for request/response serialization
3. Integration test with real credentials
4. Test all 7 TTS voices
5. Test STT with Russian audio
6. Test SSML rendering
