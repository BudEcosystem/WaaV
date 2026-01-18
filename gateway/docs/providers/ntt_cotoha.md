# NTT Communications COTOHA API - BLOCKED

> **Status:** BLOCKED
> **Date:** 2026-01-14
> **Reason:** Service terminated on June 30, 2024

---

## Overview

NTT Communications (now NTT Docomo Business as of July 2025) previously offered COTOHA API, a communication engine providing natural language processing, dialogue systems, and speech recognition for the Japanese language.

## API Status

### COTOHA API (TERMINATED)

**Termination Notice:** The service ended on June 30, 2024. The official announcement states: "Communication Engine 'COTOHA API' は 2024年6月30日をもってサービスを終了しました。"

| Parameter | Value |
|-----------|-------|
| **API Portal** | `https://api.ce-cotoha.com/` |
| **Termination Date** | June 30, 2024 |
| **Status** | **TERMINATED - Redirects to NTT main site** |

### Original API Features (Historical Reference)

The original COTOHA API supported:

**Speech Recognition (STT):**
- High-accuracy Japanese language voice recognition
- Error correction for typical recognition mistakes
- Cloud-based call recording to CSV text conversion
- Contact center integration capabilities

**Natural Language Processing:**
- Japanese dictionary with over 2.1 million words
- Semantic attribute assignment
- Context understanding for Japanese text
- Syntax/morphological analysis
- Named entity recognition
- Sentence type classification
- Sentiment analysis

**Text-to-Speech (TTS):**
- Japanese speech synthesis
- Natural-sounding voice generation
- Contact center voicebot integration

### API Endpoints (Historical)

```
Base URL: https://api.ce-cotoha.com/api/
- /v1/nlp/parse (syntax analysis)
- /v1/nlp/ne (named entity recognition)
- /v1/nlp/sentiment (sentiment analysis)
- /v1/nlp/summary (text summarization)
- /v1/nlp/similarity (sentence similarity)
- /v1/speech/recognize (speech recognition)
- /v1/speech/synthesize (speech synthesis)
```

### Authentication (Historical)

- OAuth 2.0 client credentials flow
- Client ID and Client Secret required
- Access token endpoint: `https://api.ce-cotoha.com/v1/oauth/accesstokens`

---

## Why This Provider is BLOCKED

1. **Service Terminated:** COTOHA API service officially ended on June 30, 2024.

2. **API Portal Unavailable:** The documentation portal at `api.ce-cotoha.com` now redirects to the main NTT website.

3. **No Replacement Service:** NTT Communications has not announced a direct replacement API for external developers.

4. **Company Restructuring:** NTT Communications rebranded to NTT Docomo Business in July 2025, indicating a shift in service focus.

5. **Related Products Not Publicly Available:**
   - COTOHA Voice DX Premium: Enterprise voicebot solution, not a public API
   - COTOHA Translator: Separate translation service, still operational but different from the NLP/Speech API
   - COTOHA Chat & FAQ: Chatbot platform, requires enterprise engagement

---

## Alternatives for Japanese Speech Services

For Japanese language STT/TTS, consider these alternatives:

| Provider | Type | Status | Notes |
|----------|------|--------|-------|
| **AmiVoice (Advanced Media)** | STT | Yet to Start | Japanese-specialized (medical, legal, financial) |
| **Google Cloud** | STT+TTS | DONE | Japanese language support |
| **Microsoft Azure** | STT+TTS | DONE | Japanese language support |
| **AWS Transcribe/Polly** | STT+TTS | DONE | Japanese language support |
| **NAVER CLOVA** | STT+TTS | DONE | Korean-optimized, Japanese support |

---

## References

- [NTT Communications AI Services](https://www.ntt.com/en/services/ai.html)
- [COTOHA Technical Review](https://www.ntt-review.jp/archive/ntttechnical.php?contents=ntr201708fa4.html) (Historical)
- [NTT Docomo Business (formerly NTT Communications)](https://www.ntt.com/business/)

---

## Technical Details (Historical Reference)

### Request Example (Speech Recognition)

```bash
# Historical - No longer functional
curl -X POST "https://api.ce-cotoha.com/api/v1/speech/recognize" \
  -H "Authorization: Bearer {access_token}" \
  -H "Content-Type: audio/wav" \
  --data-binary @audio.wav
```

### Response Format (Historical)

```json
{
  "result": {
    "text": "こんにちは、今日はいい天気ですね",
    "confidence": 0.95,
    "words": [
      {"word": "こんにちは", "start": 0.0, "end": 0.8},
      {"word": "今日は", "start": 0.9, "end": 1.2},
      {"word": "いい", "start": 1.3, "end": 1.5},
      {"word": "天気", "start": 1.6, "end": 1.9},
      {"word": "ですね", "start": 2.0, "end": 2.3}
    ]
  },
  "status": "OK"
}
```

### Supported Audio Formats (Historical)

- WAV: 16-bit PCM, 16kHz mono
- MP3: Supported for some endpoints
- Maximum audio duration: 60 seconds per request

### Rate Limits (Historical)

- Free tier: 1,000 API calls/month
- Standard tier: 10,000 API calls/month
- Enterprise: Unlimited (custom pricing)
