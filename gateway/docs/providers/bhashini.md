# Bhashini (AI4Bharat/ULCA) Integration Documentation

> **Provider:** #36 AI4Bharat / Bhashini
> **Status:** Ready for Implementation
> **Research Date:** 2026-01-13
> **Type:** STT + TTS + Translation

---

## Executive Summary

Bhashini is a Government of India initiative (MeitY) providing AI-powered language technology for all 22 scheduled Indian languages. The platform offers STT (ASR), TTS, and Translation services through a pipeline-based REST API. It's free for PoC usage, with paid plans available for production.

---

## Provider Information

### Company Details
- **Name:** Bhashini (ULCA - Universal Language Contribution APIs)
- **Website:** https://bhashini.gov.in
- **Research Partner:** AI4Bharat (IIT Madras)
- **API Documentation:** https://bhashini.gitbook.io/bhashini-apis
- **GitHub:** https://github.com/bhashini-dibd/ulca
- **Pricing:** Free for PoC; Contact for production pricing

### Supported Services
| Service | Type | Description |
|---------|------|-------------|
| ASR | STT | Automatic Speech Recognition for 22+ Indian languages |
| TTS | TTS | Text-to-Speech for 13+ Indian languages |
| NMT | Translation | Neural Machine Translation between Indian languages |
| Pipeline | Combined | ASR+NMT, NMT+TTS, ASR+NMT+TTS combinations |

---

## API Architecture

### Pipeline-Based Architecture

Bhashini uses a **3-step pipeline architecture**:

```
1. Pipeline Search (Optional)
   └─> Returns Pipeline IDs

2. Pipeline Config (Required)
   └─> Returns: Callback URL + Auth Key + Service IDs

3. Pipeline Compute (Required)
   └─> Returns: ASR transcription / TTS audio / Translation
```

### API Endpoints

| Purpose | URL | Method |
|---------|-----|--------|
| Registration | `https://bhashini.gov.in/ulca/user/register` | Web |
| Login | `https://bhashini.gov.in/ulca/user/login` | Web |
| Profile/API Keys | `https://bhashini.gov.in/ulca/profile` | Web |
| Pipeline Config | `https://meity-auth.ulcacontrib.org/ulca/apis/v0/model/getModelsPipeline` | POST |
| Pipeline Compute | `https://dhruva-api.bhashini.gov.in/services/inference/pipeline` | POST |

### Available Pipeline IDs
| Provider | Pipeline ID |
|----------|-------------|
| MeitY | `64392f96daac500b55c543cd` |
| AI4Bharat | `643930aa521a4b1ba0f4c41d` |

---

## Authentication

### Credential Types
1. **userID**: User identifier from ULCA profile
2. **ulcaApiKey**: API key generated in ULCA profile (max 5 keys per user)
3. **inferenceApiKey**: Obtained from Pipeline Config response

### Authentication Flow

```
┌─────────────────────────────────────────────────────────────┐
│ 1. Pipeline Config Request                                   │
│    Headers: userID, ulcaApiKey                               │
│    Body: pipelineTasks, pipelineRequestConfig                │
└───────────────────────┬─────────────────────────────────────┘
                        │
                        ▼
┌─────────────────────────────────────────────────────────────┐
│ 2. Pipeline Config Response                                  │
│    Returns: callbackUrl, inferenceApiKey (name + value)      │
│             serviceIds for each task type                    │
└───────────────────────┬─────────────────────────────────────┘
                        │
                        ▼
┌─────────────────────────────────────────────────────────────┐
│ 3. Pipeline Compute Request                                  │
│    URL: callbackUrl from config response                     │
│    Headers: Authorization = inferenceApiKey value            │
│    Body: pipelineTasks (with serviceId), inputData           │
└─────────────────────────────────────────────────────────────┘
```

---

## ASR (Speech-to-Text) Details

### Supported Languages

| Language | Code | Service ID Example |
|----------|------|-------------------|
| Hindi | hi | ai4bharat/conformer-hi-gpu--t4 |
| Tamil | ta | ai4bharat/conformer-multilingual-dravidian-gpu--t4 |
| Telugu | te | ai4bharat/conformer-multilingual-dravidian-gpu--t4 |
| Kannada | kn | ai4bharat/conformer-multilingual-dravidian-gpu--t4 |
| Malayalam | ml | ai4bharat/conformer-multilingual-dravidian-gpu--t4 |
| Bengali | bn | ai4bharat/conformer-multilingual-indo_aryan-gpu--t4 |
| Marathi | mr | ai4bharat/conformer-multilingual-indo_aryan-gpu--t4 |
| Gujarati | gu | ai4bharat/conformer-multilingual-indo_aryan-gpu--t4 |
| Punjabi | pa | ai4bharat/conformer-multilingual-indo_aryan-gpu--t4 |
| Odia | or | ai4bharat/conformer-multilingual-indo_aryan-gpu--t4 |
| Urdu | ur | ai4bharat/conformer-multilingual-indo_aryan-gpu--t4 |
| Assamese | as | bhashini/iitm/asr-misc--gpu--t4 |
| Sanskrit | sa | bhashini/iitm/asr-misc--gpu--t4 |
| English | en | ai4bharat/whisper-medium-en--gpu--t4 |
| Bhojpuri | bho | bhashini/iisc/asr-bho-t4 |
| Maithili | mai | bhashini/iisc/asr-mai-t4 |

### Audio Formats
| Format | Platform | Notes |
|--------|----------|-------|
| WAV | Android/Web | Preferred, base64 encoded |
| FLAC | iOS | Preferred for iOS |
| MP3 | All | Supported |

### Sample Rates
- Minimum: 8000 Hz
- Recommended: 16000 Hz

### ASR Request Format
```json
{
  "pipelineTasks": [
    {
      "taskType": "asr",
      "config": {
        "language": {
          "sourceLanguage": "hi"
        },
        "serviceId": "ai4bharat/conformer-hi-gpu--t4",
        "audioFormat": "wav",
        "samplingRate": 16000
      }
    }
  ],
  "inputData": {
    "audio": [
      {
        "audioContent": "<base64-encoded-audio>"
      }
    ]
  }
}
```

### ASR Response Format
```json
{
  "pipelineResponse": [
    {
      "taskType": "asr",
      "output": [
        {
          "source": "transcribed text here"
        }
      ]
    }
  ]
}
```

---

## TTS (Text-to-Speech) Details

### Supported Languages

| Language | Code | Service ID |
|----------|------|------------|
| Hindi | hi | ai4bharat/indic-tts-coqui-indo_aryan-gpu--t4 |
| Tamil | ta | ai4bharat/indic-tts-coqui-dravidian-gpu--t4 |
| Telugu | te | ai4bharat/indic-tts-coqui-dravidian-gpu--t4 |
| Kannada | kn | ai4bharat/indic-tts-coqui-dravidian-gpu--t4 |
| Malayalam | ml | ai4bharat/indic-tts-coqui-dravidian-gpu--t4 |
| Bengali | bn | ai4bharat/indic-tts-coqui-indo_aryan-gpu--t4 |
| Marathi | mr | ai4bharat/indic-tts-coqui-indo_aryan-gpu--t4 |
| Gujarati | gu | ai4bharat/indic-tts-coqui-indo_aryan-gpu--t4 |
| Punjabi | pa | ai4bharat/indic-tts-coqui-indo_aryan-gpu--t4 |
| Odia | or | ai4bharat/indic-tts-coqui-indo_aryan-gpu--t4 |
| Assamese | as | ai4bharat/indic-tts-coqui-misc-gpu--t4 |
| English | en | Bhashini/IITM/TTS |

### Voice Options
| Gender | Code |
|--------|------|
| Male | `"male"` |
| Female | `"female"` |

### Sample Rates
- Default: 8000 Hz
- Supported: 8000, 16000, 22050 Hz

### TTS Request Format
```json
{
  "pipelineTasks": [
    {
      "taskType": "tts",
      "config": {
        "language": {
          "sourceLanguage": "hi"
        },
        "serviceId": "ai4bharat/indic-tts-coqui-indo_aryan-gpu--t4",
        "gender": "female",
        "samplingRate": 8000
      }
    }
  ],
  "inputData": {
    "input": [
      {
        "source": "नमस्ते, आप कैसे हैं?"
      }
    ]
  }
}
```

### TTS Response Format
```json
{
  "pipelineResponse": [
    {
      "taskType": "tts",
      "audio": [
        {
          "audioContent": "<base64-encoded-audio>"
        }
      ]
    }
  ]
}
```

---

## Implementation Plan

### Architecture Decision

**Approach:** Dynamic Plugin (REST API)

**Rationale:**
- Pipeline-based API requires multi-step flow (config → compute)
- No WebSocket/gRPC support - HTTP REST only
- Audio is base64 encoded (not streaming)
- Suitable for batch processing, not real-time streaming

### Implementation Structure

```
src/core/stt/bhashini/
├── mod.rs           # Module exports, constants
├── config.rs        # BhashiniSttConfig, BhashiniLanguage, BhashiniAudioFormat
├── messages.rs      # PipelineConfigRequest/Response, ComputeRequest/Response
└── client.rs        # BhashiniStt implementing BaseSTT

src/core/tts/bhashini/
├── mod.rs           # Module exports, constants
├── config.rs        # BhashiniTtsConfig, BhashiniVoiceGender
├── messages.rs      # TTS request/response structures
└── provider.rs      # BhashiniTts implementing BaseTTS
```

### Key Implementation Details

#### 1. Pipeline Config Caching
- Cache pipeline config response (contains serviceIds and callbackUrl)
- Refresh when config changes or on auth error
- Store: callbackUrl, inferenceApiKey, serviceIds per task type

#### 2. Audio Buffering (STT)
- Bhashini uses REST API with base64 audio
- Buffer audio chunks until flush/disconnect
- Convert to WAV/FLAC format
- Base64 encode entire audio for compute request

#### 3. Audio Decoding (TTS)
- Response contains base64-encoded audio
- Decode to raw bytes
- Stream to callback

#### 4. Language Resolution
- Use ISO-639 codes (hi, ta, te, bn, etc.)
- Map to appropriate serviceId based on language family:
  - Dravidian: ta, te, kn, ml
  - Indo-Aryan: hi, bn, mr, gu, pa, or, ur
  - Misc: as, sa, en

### Testing Plan

#### Unit Tests (~50 tests)
1. Config validation tests
2. Language code mapping tests
3. Service ID selection tests
4. Request/response serialization tests
5. Base64 encoding/decoding tests
6. Audio format conversion tests

#### Integration Tests (~10 tests)
1. Pipeline config API call
2. ASR compute call with Hindi audio
3. TTS compute call with Hindi text
4. Combined ASR+TTS pipeline
5. Error handling (auth failure, rate limit)

---

## Error Handling

### HTTP Status Codes
| Code | Meaning | Action |
|------|---------|--------|
| 200 | Success | Process response |
| 400 | Bad Request | Check payload format |
| 401 | Unauthorized | Refresh credentials |
| 403 | Forbidden | Check API key validity |
| 429 | Rate Limited | Exponential backoff |
| 500 | Server Error | Retry with backoff |

### Error Response Format
```json
{
  "error": {
    "code": "INVALID_LANGUAGE",
    "message": "Language 'xyz' is not supported for ASR"
  }
}
```

---

## Best Practices

### Performance Optimization
1. **Cache Pipeline Config**: Avoid repeated config calls
2. **Use Appropriate Audio Format**: WAV for Android/Web, FLAC for iOS
3. **Batch Processing**: Bhashini is optimized for batch, not real-time
4. **Language-Specific Models**: Use dedicated models when available

### Security
1. Store credentials securely (environment variables)
2. Never log API keys
3. Use HTTPS for all requests
4. Validate audio content before sending

### Limitations
1. **Not Real-Time**: REST API with base64 audio, not streaming
2. **PoC Usage**: Free tier is for PoC only; contact for production
3. **Model Availability**: Not all languages have all models
4. **Latency**: Higher latency due to pipeline architecture

---

## References

### Official Documentation
- [Bhashini APIs GitBook](https://bhashini.gitbook.io/bhashini-apis)
- [ULCA Portal](https://bhashini.gov.in/ulca)
- [Available Models](https://dibd-bhashini.gitbook.io/bhashini-apis/available-models-for-usage)

### GitHub Resources
- [ULCA Repository](https://github.com/bhashini-dibd/ulca)
- [AI4Bharat Indic-TTS](https://github.com/AI4Bharat/Indic-TTS)
- [Python Client](https://github.com/dteklavya/bhashini_translator)

### Research
- [AI4Bharat Models Portal](https://models.ai4bharat.org/)
- [AI4Bharat Research](https://ai4bharat.iitm.ac.in/)

---

*Last Updated: 2026-01-13*
