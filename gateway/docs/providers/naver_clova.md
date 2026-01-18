# NAVER CLOVA Speech Services Integration

> **Provider:** NAVER Cloud Platform (네이버 클라우드 플랫폼)
> **Status:** Implementation In Progress
> **Last Updated:** 2026-01-14
> **Research Source:** Official NAVER Cloud Platform Documentation

---

## Overview

NAVER CLOVA provides comprehensive speech AI services through the NAVER Cloud Platform. The service includes multiple speech recognition (STT) and speech synthesis (TTS) APIs optimized for Korean, Japanese, English, and Chinese languages.

### Service Portfolio

| Service | Type | Description |
|---------|------|-------------|
| CLOVA Speech Recognition (CSR) | STT | Short utterances (< 60 seconds), REST API |
| CLOVA Speech | STT | Long audio/video files, REST + gRPC streaming |
| CLOVA Voice | TTS | 100+ voices, Premium quality, REST API |
| CLOVA Speech Synthesis (CSS) | TTS | Basic voices, REST API (legacy) |

---

## Authentication

All NAVER Cloud Platform APIs use a two-key authentication system:

| Header | Description |
|--------|-------------|
| `X-NCP-APIGW-API-KEY-ID` | Client ID (obtained from NAVER Cloud Console) |
| `X-NCP-APIGW-API-KEY` | Client Secret (obtained from NAVER Cloud Console) |

### Getting API Credentials

1. Create a NAVER Cloud Platform account at https://www.ncloud.com
2. Navigate to: Console → AI·Application Service → AI·NAVER API → Application
3. Register a new application
4. Copy the Client ID and Client Secret

---

## CLOVA Speech Recognition (CSR) - STT

### Description

CLOVA Speech Recognition is optimized for short utterances within 60 seconds. It uses NAVER's proprietary speech recognition technology with the highest recognition rate for Korean.

### API Endpoint

```
POST https://naveropenapi.apigw.ntruss.com/recog/v1/stt?lang={language_code}
```

### Request Headers

| Header | Value | Required |
|--------|-------|----------|
| `X-NCP-APIGW-API-KEY-ID` | Client ID | Yes |
| `X-NCP-APIGW-API-KEY` | Client Secret | Yes |
| `Content-Type` | `application/octet-stream` | Yes |

### Language Codes

| Code | Language | Notes |
|------|----------|-------|
| `Kor` | Korean | Highest recognition accuracy |
| `Eng` | English | |
| `Jpn` | Japanese | |
| `Chn` | Chinese (Simplified) | |

### Request Body

Binary audio data (raw audio stream).

### Audio Requirements

- **Maximum duration:** 60 seconds
- **Minimum sample rate:** 16 kHz
- **Channels:** Mono preferred
- **Format:** PCM, WAV, MP3

### Response Format

```json
{
    "text": "recognized text here"
}
```

### Error Codes

| Code | Description |
|------|-------------|
| 400 | Bad Request - Invalid parameters |
| 401 | Unauthorized - Invalid credentials |
| 413 | Payload Too Large - Audio exceeds 60 seconds |
| 429 | Too Many Requests - Rate limit exceeded |
| 500 | Internal Server Error |

### Usage Limits

- Default: 300,000 seconds/month (30,000 seconds/day)
- Maximum: 30,000,000 seconds/month (adjustable)

---

## CLOVA Speech - Long Audio STT

### Description

CLOVA Speech provides speech recognition for long audio/video files using CLOVA's NEST (Neural End-to-end Speech Transcriber) technology.

### API Endpoints

#### File Upload Recognition

```
POST https://clovaspeech-gw.ncloud.com/recog/v1/stt
```

#### Object Storage Recognition

```
POST https://clovaspeech-gw.ncloud.com/external/v1/recognition/{path}
```

#### gRPC Streaming (Real-time)

```
grpc://clovaspeech-gw.ncloud.com:443
```

### Request Headers

| Header | Value |
|--------|-------|
| `X-CLOVASPEECH-API-KEY` | Secret Key |
| `Content-Type` | `application/json` |

### gRPC Streaming Specifications

- **Format:** PCM (headerless raw wave)
- **Sample Rate:** 16 kHz
- **Channels:** 1 (mono)
- **Bit Depth:** 16 bits

### Request Parameters

```json
{
    "language": "ko-KR",
    "completion": "sync",
    "callback": "https://your-callback-url.com",
    "fullText": true,
    "diarization": {
        "enable": true,
        "speakerCountMin": 2,
        "speakerCountMax": 10
    }
}
```

### Supported Languages (CLOVA Speech)

| Code | Language |
|------|----------|
| `ko-KR` | Korean |
| `en-US` | English (US) |
| `ja` | Japanese |
| `zh-cn` | Chinese (Simplified) |
| `zh-tw` | Chinese (Traditional) |
| `enko` | Korean-English Mixed |

---

## CLOVA Voice - TTS Premium

### Description

CLOVA Voice is the premium TTS service offering 100+ high-quality voices with NeuVis (Neural Voice Synthesis) technology.

### API Endpoint

```
POST https://naveropenapi.apigw.ntruss.com/tts-premium/v1/tts
```

### Request Headers

| Header | Value | Required |
|--------|-------|----------|
| `X-NCP-APIGW-API-KEY-ID` | Client ID | Yes |
| `X-NCP-APIGW-API-KEY` | Client Secret | Yes |
| `Content-Type` | `application/x-www-form-urlencoded` | Yes |

### Request Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `speaker` | string | - | Voice ID (required) |
| `text` | string | - | Text to synthesize (max 2000 chars) |
| `volume` | int | 0 | Volume adjustment (-5 to 5) |
| `speed` | int | 0 | Speed adjustment (-5 to 5) |
| `pitch` | int | 0 | Pitch adjustment (-5 to 5) |
| `emotion` | int | 0 | Emotion intensity (0-2) |
| `emotion-strength` | int | 1 | Emotion strength (0-2) |
| `format` | string | `mp3` | Output format: `mp3` or `wav` |
| `alpha` | int | 0 | Voice alpha (timbre) adjustment |
| `end-pitch` | int | 0 | End pitch adjustment |

### Available Voices (Sample)

#### Korean Voices

| Speaker ID | Gender | Style | Description |
|------------|--------|-------|-------------|
| `nara` | Female | Standard | Clear, professional |
| `nara_call` | Female | Call center | Customer service |
| `nminsang` | Male | Standard | Clear, professional |
| `nhajun` | Male | Child | Young voice |
| `ndain` | Female | Child | Young voice |
| `njiyun` | Female | Standard | Natural |
| `nsujin` | Female | Standard | Friendly |
| `njinho` | Male | Standard | Professional |
| `nminseo` | Female | Standard | Bright |
| `njooahn` | Female | Standard | Calm |
| `nseonghoon` | Male | Standard | Deep |
| `njihun` | Male | Standard | Warm |
| `njangj` | Male | Standard | Authoritative |
| `noyj` | Female | Standard | Gentle |
| `neunyoung` | Female | Standard | Young |
| `nkyuwon` | Male | Standard | Clear |
| `nwoosik` | Male | Standard | Friendly |
| `nkyungtae` | Male | Standard | Professional |

#### Japanese Voices

| Speaker ID | Gender | Style |
|------------|--------|-------|
| `ntomoko` | Female | Standard |
| `nnaomi` | Female | Standard |
| `nsayuri` | Female | Bright |
| `ngoeun` | Female | Calm |
| `neunji` | Female | Natural |
| `nyounghwa` | Male | Standard |

#### English Voices

| Speaker ID | Gender | Style |
|------------|--------|-------|
| `clara` | Female | Standard |
| `matt` | Male | Standard |
| `danna` | Female | Premium |
| `djoey` | Male | Premium |

#### Chinese Voices

| Speaker ID | Gender | Style |
|------------|--------|-------|
| `meimei` | Female | Standard |
| `liangliang` | Male | Standard |

#### Spanish Voices

| Speaker ID | Gender | Style |
|------------|--------|-------|
| `jose` | Male | Standard |
| `carmen` | Female | Standard |

### Response

Binary audio data in the specified format (MP3 or WAV).

### Response Headers

| Header | Description |
|--------|-------------|
| `Content-Type` | `audio/mpeg` or `audio/wav` |
| `Content-Disposition` | Filename information |

### Text Limits

- **Maximum characters per request:** 2,000 characters
- **Recommended chunking:** 500-1000 characters for optimal quality

---

## CLOVA Speech Synthesis (CSS) - Legacy TTS

### Description

The original CLOVA Speech Synthesis service. NAVER recommends using CLOVA Voice for new applications, but CSS remains available for existing applications.

### API Endpoint

```
POST https://naveropenapi.apigw.ntruss.com/voice/v1/tts
```

### Request Headers

| Header | Value |
|--------|-------|
| `X-NCP-APIGW-API-KEY-ID` | Client ID |
| `X-NCP-APIGW-API-KEY` | Client Secret |
| `Content-Type` | `application/x-www-form-urlencoded; charset=UTF-8` |

### Request Parameters

| Parameter | Description |
|-----------|-------------|
| `speaker` | Voice type (mijin, jinho, clara, matt, shinji, meimei, etc.) |
| `text` | Text to synthesize |
| `speed` | Speech speed (-5 to 5, default 0) |

### Legacy Speakers

| Speaker | Language | Gender |
|---------|----------|--------|
| `mijin` | Korean | Female |
| `jinho` | Korean | Male |
| `clara` | English | Female |
| `matt` | English | Male |
| `shinji` | Japanese | Male |
| `meimei` | Chinese | Female |
| `liangliang` | Chinese | Male |

---

## Rate Limits & Quotas

### CSR (Short STT)

| Plan | Monthly Limit | Daily Limit |
|------|---------------|-------------|
| Free | 30,000 seconds | 3,000 seconds |
| Basic | 300,000 seconds | 30,000 seconds |
| Enterprise | 30,000,000 seconds | 10,000,000 seconds |

### CLOVA Voice (TTS)

| Plan | Monthly Limit |
|------|---------------|
| Free | 90,000 characters |
| Basic | 10,000,000 characters |
| Enterprise | Contact Sales |

---

## Pricing

### CSR (Short STT)

- Pay-per-use model based on audio duration
- Pricing varies by volume tier
- Recent 40% price reduction announced

### CLOVA Voice (TTS)

- Pay-per-character model
- Premium voices have higher rates
- Volume discounts available

*Note: For exact pricing, visit the NAVER Cloud Platform pricing page.*

---

## Best Practices

### STT

1. **Audio Quality:** Use 16 kHz or higher sample rate for best accuracy
2. **Noise Reduction:** Apply noise filtering before sending audio
3. **Chunking:** For long audio, use CLOVA Speech instead of CSR
4. **Language Selection:** Explicitly set the language code for better accuracy
5. **Error Handling:** Implement retry logic with exponential backoff

### TTS

1. **Text Preprocessing:** Clean and normalize text before synthesis
2. **Character Limits:** Stay under 2000 characters per request
3. **Chunking Strategy:** Split at sentence boundaries for natural pauses
4. **Voice Selection:** Match voice to content type (news, conversational, etc.)
5. **Format Selection:** Use MP3 for smaller files, WAV for higher quality

---

## Implementation Plan for Bud WaaV

### STT Provider

1. **Primary API:** CLOVA Speech Recognition (CSR) for real-time short utterances
2. **Protocol:** HTTP REST API
3. **Authentication:** X-NCP-APIGW-API-KEY-ID + X-NCP-APIGW-API-KEY headers
4. **Languages:** Korean, Japanese, English, Chinese
5. **Audio Format:** Support PCM, WAV input; resample to 16kHz if needed

### TTS Provider

1. **Primary API:** CLOVA Voice Premium TTS
2. **Protocol:** HTTP REST API
3. **Authentication:** X-NCP-APIGW-API-KEY-ID + X-NCP-APIGW-API-KEY headers
4. **Voices:** Support all 100+ voices with metadata
5. **Output Format:** MP3 (default) and WAV

### Configuration Schema

```rust
// STT Configuration
struct NaverClovaSttConfig {
    client_id: String,
    client_secret: String,
    language: NaverClovaLanguage,  // Kor, Eng, Jpn, Chn
}

// TTS Configuration
struct NaverClovaTtsConfig {
    client_id: String,
    client_secret: String,
    speaker: NaverClovaVoice,
    volume: i8,    // -5 to 5
    speed: i8,     // -5 to 5
    pitch: i8,     // -5 to 5
    emotion: u8,   // 0-2
    format: AudioFormat,  // mp3 or wav
}
```

---

## Test Plan

### Unit Tests

1. Configuration parsing and validation
2. Request building and serialization
3. Response parsing
4. Error handling
5. Voice metadata enumeration

### Integration Tests

1. Basic STT recognition (Korean, English, Japanese)
2. Basic TTS synthesis (multiple voices)
3. Error response handling
4. Rate limit handling
5. Authentication failure handling

### Real API Tests

1. Live audio transcription
2. Live speech synthesis
3. Character limit verification
4. Audio format compatibility
5. Latency measurement

---

## References

- [CLOVA Speech Recognition (CSR) Overview](https://api.ncloud-docs.com/docs/en/ai-naver-clovaspeechrecognition)
- [CLOVA Speech Overview](https://api.ncloud-docs.com/docs/en/ai-application-service-clovaspeech)
- [CLOVA Voice Overview](https://api.ncloud-docs.com/docs/en/ai-naver-clovavoice)
- [TTS Premium API](https://api.ncloud-docs.com/docs/en/ai-naver-clovavoice-ttspremium)
- [CLOVA Voice Examples](https://api.ncloud-docs.com/docs/en/ai-naver-clovavoice-ttspremium-api-example)
- [LiveKit CLOVA STT Plugin](https://docs.livekit.io/agents/integrations/stt/clova/)
- [NAVER Cloud Platform Console](https://www.ncloud.com)
