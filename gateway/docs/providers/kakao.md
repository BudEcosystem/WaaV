# Kakao Speech API - BLOCKED

> **Status:** BLOCKED
> **Date:** 2026-01-14
> **Reason:** Original API terminated, replacement requires enterprise setup

---

## Overview

Kakao (also known as Kakao i) is a Korean technology company that previously offered Speech-to-Text (STT) and Text-to-Speech (TTS) APIs through their Kakao Developers platform.

## API Status

### Original Kakao Developers Speech API (TERMINATED)

**Termination Notice:** [Kakao DevTalk Announcement](https://devtalk.kakao.com/t/api-notice-end-of-support-for-vision-translation-and-speech-apis/122817)

| Parameter | Value |
|-----------|-------|
| **STT Endpoint** | `https://kakaoi-newtone-openapi.kakao.com/v1/recognize` |
| **TTS Endpoint** | `https://kakaoi-newtone-openapi.kakao.com/v1/synthesize` |
| **Termination Date** | July 1, 2022 |
| **Grace Period End** | June 30, 2023 |
| **Status** | **TERMINATED - No longer accessible** |

### Original API Features (Historical Reference)

The original API supported:

**STT (Speech Recognition):**
- Endpoint: `POST https://kakaoi-newtone-openapi.kakao.com/v1/recognize`
- Headers:
  - `Authorization: KakaoAK {REST_API_KEY}`
  - `Content-Type: application/octet-stream`
  - `Transfer-Encoding: chunked`
  - `X-DSS-Service: DICTATION`
- Input: WAV audio files
- Output: JSON with transcription text

**TTS (Speech Synthesis):**
- Endpoint: `POST https://kakaoi-newtone-openapi.kakao.com/v1/synthesize`
- Headers:
  - `Authorization: KakaoAK {REST_API_KEY}`
  - `Content-Type: application/xml`
- Input: SSML-formatted text in `<speak>` tags
- Output: MP3 audio binary

**Voice Options (Historical):**
- `WOMAN_READ_CALM` - Female calm reading voice (default)
- `MAN_READ_CALM` - Male calm reading voice
- `WOMAN_DIALOG_BRIGHT` - Female bright conversational voice
- `MAN_DIALOG_BRIGHT` - Male bright conversational voice

**Volume Settings:**
- soft: 0.7
- medium: 1.0 (default)
- loud: 1.4

---

## Replacement: KakaoCloud (Enterprise)

Kakao recommends migrating to **KakaoCloud** for speech services.

### KakaoCloud TTS API

Documentation: [docs.kakaocloud.com](https://docs.kakaocloud.com/en/service/ai-service/tts/general-tts/api/tts-general-tts-api)

**Headers:**
- `x-api-key: {API Key}`
- `Content-Type: application/xml`
- `X-TTS-Engine: plain` or `deep`
- `X-TTS-Encoding: {encoding method}`
- `X-TTS-Samplerate: {sample rate}`

**Engine Types:**
- `plain` - Plain Voice engine
- `deep` - Deep Voice engine (higher quality)

**Voice Options:**
- `Summer` (default)
- `Roman`

**Output Formats:**
- MP3 (22kHz default)

---

## Why This Provider is BLOCKED

1. **Original API Terminated:** The Kakao Developers Speech API (`kakaoi-newtone-openapi.kakao.com`) was terminated on July 1, 2022, with the grace period ending June 30, 2023.

2. **Enterprise-Only Replacement:** KakaoCloud is an enterprise cloud platform (similar to AWS/GCP) that requires:
   - KakaoCloud account creation
   - Enterprise verification
   - Billing setup
   - API key provisioning through their console

3. **Limited Documentation:** The KakaoCloud documentation is:
   - Incomplete (many 404 errors)
   - Missing STT API documentation
   - Limited voice options compared to the original API

4. **Regional Availability:** KakaoCloud services may have regional restrictions and primarily target Korean enterprise customers.

5. **Not a Simple REST API:** Unlike the original Kakao Developers API that worked with a simple REST API key, KakaoCloud requires full cloud platform integration.

---

## Alternatives for Korean Speech Services

For Korean language STT/TTS, consider these alternatives:

| Provider | Type | Status | Notes |
|----------|------|--------|-------|
| **NAVER CLOVA** | STT+TTS | DONE | 100+ voices, Korean-optimized |
| **Google Cloud** | STT+TTS | DONE | Korean language support |
| **Microsoft Azure** | STT+TTS | DONE | Korean language support |
| **AWS Transcribe/Polly** | STT+TTS | DONE | Korean language support |

---

## References

- [Kakao Developers Portal](https://developers.kakao.com/)
- [API Termination Notice](https://devtalk.kakao.com/t/api-notice-end-of-support-for-vision-translation-and-speech-apis/122817)
- [KakaoCloud Documentation](https://docs.kakaocloud.com/)
- [Kakao TTS Home Assistant Integration](https://github.com/miumida/kakao_tts) (Historical reference)
