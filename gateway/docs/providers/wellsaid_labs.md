# WellSaid Labs TTS Provider Integration

> **Status:** IN_PROGRESS
> **Research Date:** 2026-01-13
> **Provider Type:** TTS + AI Director (Voice Customization)

---

## 1. Provider Overview

### Basic Information
- **Website:** https://www.wellsaid.io
- **API Documentation:** https://docs.wellsaidlabs.com/
- **Developer Portal:** https://developer.wellsaidlabs.com/
- **GitHub Examples:** https://github.com/wellsaid-labs/simple-api-example

### Capabilities Matrix
| Capability | Supported | Notes |
|------------|-----------|-------|
| TTS | YES | HTTP streaming, 10x real-time render speed |
| STT | NO | - |
| Audio-to-Audio | NO | - |
| Voice Cloning | YES | Enterprise feature, custom voices |
| AI Director | YES | Pitch, tempo, loudness, respelling (Caruso model) |
| Streaming | YES | Real-time audio streaming via HTTP |

### Technical Specifications
- **Authentication:** API Key via `X-Api-Key` header
- **Protocol:** REST (HTTP POST for TTS, GET for metadata)
- **Audio Formats:** MP3 (streaming)
- **Sample Rates:** Up to 96kHz
- **Languages:** English (Caruso), 15+ languages (Legacy model)
- **Voices:** 170+ voice avatars

### Compliance
- SOC2 Certified
- GDPR Compliant
- EU AI Act-Ready
- Commercial usage rights included

---

## 2. API Endpoints

### 2.1 Text-to-Speech Streaming
**Endpoint:** `POST https://api.wellsaidlabs.com/v1/tts/stream`

**Headers:**
```
X-Api-Key: <your-api-key>
Accept: audio/mpeg
Content-Type: application/json
```

**Request Body:**
```json
{
  "text": "Hello, this is a test.",
  "speaker_id": 3,
  "model": "caruso"
}
```

**Parameters:**
| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| text | string | Yes | Text to synthesize (max 1000 chars default) |
| speaker_id | integer | Yes | Voice avatar ID from avatars endpoint |
| model | string | No | "caruso" or "legacy" (default: legacy) |

**Response:** Streamed MP3 audio data

### 2.2 List Available Avatars
**Endpoint:** `GET https://api.wellsaidlabs.com/v1/tts/avatars`

**Headers:**
```
X-Api-Key: <your-api-key>
```

**Response:**
```json
{
  "avatars": [
    {
      "name": "Alana B.",
      "id": 3,
      "style": "Narration",
      "gender": "Female",
      "accent_type": "United States",
      "characteristics": ["Clear", "Crisp", "Focused"],
      "preview_audio": "https://...",
      "locale": "en_US",
      "language": "English",
      "language_variant": "United States",
      "otherTags": ["featured"],
      "source": null
    }
  ]
}
```

### 2.3 Respelling Suggestions (Future)
**Endpoint:** `GET https://api.wellsaidlabs.com/v1/respelling_suggestions`

Provides pronunciation suggestions for words.

### 2.4 Clips Management (Future)
**Endpoint:** `https://api.wellsaidlabs.com/v1/clips`

Manages generated audio clips.

---

## 3. Models

### 3.1 Caruso Model
- **Description:** Next-generation AI voice engine with studio-quality output
- **Latency:** 30% faster than legacy (~500ms per 30 characters)
- **Languages:** English only
- **Features:** AI Director capabilities (pitch, tempo, loudness, respelling)
- **Quality:** Highest fidelity, studio-grade

### 3.2 Legacy Model
- **Description:** Original WellSaid voice model
- **Languages:** All supported languages (15+)
- **Features:** Standard TTS without AI Director
- **Use Case:** Multilingual applications

---

## 4. AI Director (Caruso Model Only)

AI Director enables fine-tuned speech delivery using XML tags embedded in the text.

### 4.1 Tempo Tag
Controls speech pace.

**Range:** 0.5 to 2.5 (default: 1)
**Suggested Values:** 0.5, 0.6, 0.7, 0.8, 0.9, 1, 1.3, 1.6, 1.9, 2.3, 2.5

| Value | Effect |
|-------|--------|
| 0.5 | Deliberate, thoughtful |
| 1.0 | Conversational |
| 2.5 | Rapid, energetic |

**Example:**
```xml
<tempo value="0.7">This will be spoken more slowly.</tempo>
```

### 4.2 Loudness Tag
Controls volume intensity.

**Range:** -20 to 10 (default: 0)
**Suggested Values:** -20, -12, -8, -4, -2, 0, 2, 4, 6, 8, 10

| Value | Effect |
|-------|--------|
| -20 | Subdued, intimate (whisper) |
| 0 | Natural, balanced |
| 10 | Assertive, emphatic |

**Example:**
```xml
<loudness value="-10">This is a softer tone.</loudness>
```

### 4.3 Pitch Tag
Controls tonal height.

**Range:** -250 to 500 (default: 0)
**Suggested Values:** -250, -200, -150, -100, -50, 0, 100, 200, 300, 400, 500

| Value | Effect |
|-------|--------|
| -250 | Deep, solemn |
| 0 | Natural |
| 500 | High-pitched, playful |

**Example:**
```xml
<pitch value="-150">This sounds deeper and more serious.</pitch>
```

### 4.4 Respell Tag
Provides inline pronunciation.

**Example:**
```xml
The word <respell value="tuh-may-toe">tomato</respell> is delicious.
```

### 4.5 Nesting Tags
Tags can be nested for combined effects:

```xml
<pitch value="-200"><tempo value="0.5"><loudness value="-5">
This text will be deep, slow, and soft.
</loudness></tempo></pitch>
```

### 4.6 Creating Pauses
Apply tempo to punctuation with loudness below -40 to prevent breath sounds:

```xml
Hello.<tempo value="0.3"><loudness value="-50">,</loudness></tempo> World.
```

---

## 5. Rate Limits

### Default Limits
| Limit Type | Value |
|------------|-------|
| Requests per second | 3 |
| Characters per request | 1,000 |
| Monthly quota | Usage-based (no hard cap) |

### Response Headers
| Header | Description |
|--------|-------------|
| x-quota-limit | Maximum requests in current timeframe |
| x-quota-remaining | Requests remaining in timeframe |
| x-quota-reset | UNIX timestamp for quota reset |
| x-rate-limit-limit | Rate limit max requests |
| x-rate-limit-remaining | Rate limit requests remaining |
| x-rate-limit-reset | Rate limit reset timestamp |

### Handling Rate Limits
- Implement exponential backoff on 429 responses
- Monitor response headers for quota tracking
- Contact WellSaid for higher rate limits (enterprise)

---

## 6. Voice Avatars

### Voice Styles
| Style | Use Case |
|-------|----------|
| Narration | Clear, informative content |
| Promo | Confident, energetic marketing |
| Conversational | Approachable dialogue |
| Character | Creative, distinctive personas |
| Custom | Enterprise voice cloning |

### Language Support
- **Caruso Model:** English (US, UK, AU, etc.)
- **Legacy Model:** English, Spanish, French, German, Italian, Portuguese, Japanese, Korean, Chinese (Mandarin/Cantonese), Arabic (26 regional variants), Turkish, Dutch, Polish, Danish, Swedish, Persian

### Sample Speaker IDs
| ID | Name | Style | Accent |
|----|------|-------|--------|
| 3 | Alana B. | Narration | US |
| 8 | Sofia H. | Narration | US |
| 13 | Jeremy G. | Narration | US |
| 26 | Wade C. | Promo | US |
| 45 | Nicole L. | Conversational | US |
| 80 | Marcus G. | Conversational | US |

---

## 7. Pricing

### API Pricing
- **Per Character:** $0.024

### Plans
| Plan | Price | Downloads | Voices |
|------|-------|-----------|--------|
| Maker | $49/month | 250 | 24 |
| Creative | $99/month | 750 | 53 |
| Business | $179/month | 9,000 | All |
| Enterprise | Custom | Unlimited | All + API |

### Free Trial
- Duration: 14 days
- API calls: 50
- Voices: All available
- Downloads: Not permitted

---

## 8. Implementation Plan

### 8.1 Module Structure
```
src/core/tts/wellsaid/
├── mod.rs           # Module exports and constants
├── config.rs        # WellSaidTtsConfig, WellSaidModel
└── provider.rs      # WellSaidTts implementing BaseTTS
```

### 8.2 Implementation Steps

1. **Create config.rs**
   - WellSaidModel enum (Caruso, Legacy)
   - WellSaidTtsConfig struct
   - AI Director tag builder helpers
   - From TTSConfig conversion

2. **Create provider.rs**
   - WellSaidRequestBuilder implementing TTSRequestBuilder
   - WellSaidTts implementing BaseTTS
   - list_avatars() static method
   - HTTP streaming via reqwest

3. **Create mod.rs**
   - API URL constants
   - Default values
   - Public exports

4. **Update tts/mod.rs**
   - Add wellsaid module
   - Update factory function

5. **Update plugin registration**
   - Add to builtin/mod.rs
   - Add to dispatch.rs PHF map

### 8.3 Configuration Mapping

| TTSConfig Field | WellSaid Mapping |
|-----------------|------------------|
| api_key | X-Api-Key header |
| voice_id | speaker_id (integer) |
| model | "caruso" or "legacy" |
| audio_format | Always MP3 (streaming) |
| sample_rate | Not configurable (up to 96kHz auto) |

### 8.4 AI Director Integration

Create helper methods for AI Director tags:
- `with_tempo(value: f32)` - Wrap text with tempo tag
- `with_loudness(value: i32)` - Wrap text with loudness tag
- `with_pitch(value: i32)` - Wrap text with pitch tag
- `with_respell(word: &str, pronunciation: &str)` - Respelling

---

## 9. Testing Plan

### 9.1 Unit Tests
- Config parsing and validation
- Model enum serialization
- AI Director tag generation
- Request body construction
- Response header parsing

### 9.2 Integration Tests (with credentials)
- Connection and avatar listing
- Basic TTS synthesis
- Caruso model with AI Director
- Legacy model multilingual
- Rate limit handling
- Error responses (invalid API key, invalid speaker_id)

### 9.3 Test Cases
```rust
#[test]
fn test_wellsaid_config_defaults()
#[test]
fn test_wellsaid_model_serialization()
#[test]
fn test_ai_director_tempo_tag()
#[test]
fn test_ai_director_nested_tags()
#[test]
fn test_request_body_construction()
#[tokio::test]
async fn test_wellsaid_connect()
#[tokio::test]
async fn test_wellsaid_speak_basic()
#[tokio::test]
async fn test_wellsaid_speak_with_ai_director()
```

---

## 10. Best Practices

### Performance
- Use streaming endpoint for real-time applications
- Batch longer texts into 1000-char chunks
- Cache avatar list (doesn't change frequently)
- Monitor rate limit headers

### Quality
- Use Caruso model for English content
- Apply AI Director tags sparingly for natural results
- Test voice selection in WellSaid Studio first
- Use suggested values for AI Director parameters

### Cost Optimization
- Pre-process text to remove unnecessary characters
- Cache frequently used audio
- Use Legacy model for non-English content

### Security
- Never expose API key in client-side code
- Use server-side proxy for API calls
- Rotate API keys periodically

---

## 11. Error Handling

### Expected Error Responses
| Status | Meaning | Action |
|--------|---------|--------|
| 400 | Bad Request | Check request body format |
| 401 | Unauthorized | Verify API key |
| 403 | Forbidden | Check account permissions |
| 404 | Not Found | Verify speaker_id exists |
| 429 | Rate Limited | Implement backoff, retry later |
| 500 | Server Error | Retry with exponential backoff |

### Response Validation
- Verify Content-Type is audio/mpeg
- Check for empty response body
- Monitor x-quota-remaining header

---

## 12. References

- [WellSaid API Documentation](https://docs.wellsaidlabs.com/)
- [API Reference](https://docs.wellsaidlabs.com/reference/getting-started-with-your-api)
- [Model Selection Guide](https://docs.wellsaidlabs.com/reference/model-selection-with-the-api)
- [AI Director Guide](https://docs.wellsaidlabs.com/reference/using-ai-director-with-the-api)
- [Available Voice Avatars](https://docs.wellsaidlabs.com/reference/available-voice-avatars)
- [GitHub Example](https://github.com/wellsaid-labs/simple-api-example)
- [WellSaid Help Center](https://help.wellsaidlabs.com/)
