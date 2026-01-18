# Speechify TTS Provider Integration

> **Status:** DONE
> **Research Date:** 2026-01-13
> **Provider Type:** TTS + Voice Cloning

---

## 1. Provider Overview

### Basic Information
- **Website:** https://speechify.com
- **API Documentation:** https://docs.sws.speechify.com
- **API Console:** https://console.sws.speechify.com
- **GitHub SDK:** https://github.com/SpeechifyInc/speechify-api-sdk-python
- **Recognition:** Apple Design Award 2025 at WWDC

### Capabilities Matrix
| Capability | Supported | Notes |
|------------|-----------|-------|
| TTS | YES | HTTP streaming with chunked transfer encoding |
| STT | NO | Not provided |
| Voice Cloning | YES | Instant cloning from 10-30s audio sample |
| Streaming | YES | `/v1/audio/stream` endpoint |
| SSML | YES | Full SSML support for prosody control |

### Technical Specifications
- **Authentication:** Bearer Token via `Authorization` header
- **Protocol:** REST (HTTP POST)
- **Audio Formats:** WAV (48kHz), MP3 (24kHz), OGG (24kHz), AAC (24kHz)
- **Sample Rates:** 48000 Hz (WAV), 24000 Hz (others)
- **Languages:** 50+ languages (6 fully supported, 17 beta, 27 coming soon)
- **Latency:** 250-300ms TTFA

---

## 2. API Endpoints

### 2.1 Streaming TTS
**Endpoint:** `POST https://api.sws.speechify.com/v1/audio/stream`

**Headers:**
```
Authorization: Bearer <api-token>
Content-Type: application/json
```

**Request Body:**
```json
{
  "input": "Hello, this is a test.",
  "voice_id": "george",
  "model": "simba-english",
  "audio_format": "wav_48000",
  "language": "en-US",
  "loudness_normalization": false,
  "text_normalization": false
}
```

**Parameters:**
| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| input | string | Yes | Text or SSML to synthesize (max 20,000 chars) |
| voice_id | string | Yes | Voice identifier |
| model | string | No | simba-english, simba-turbo, simba-multilingual, simba-base |
| audio_format | string | No | wav_48000, mp3_24000, ogg_24000, aac_24000 |
| language | string | No | ISO-639-1 locale (e.g., en-US, fr-FR) |
| loudness_normalization | boolean | No | Normalize to -14 LUFS (increases latency) |
| text_normalization | boolean | No | Convert numbers/dates to words |

**Response:** Streaming `application/octet-stream` (chunked audio)

### 2.2 Synchronous TTS
**Endpoint:** `POST https://api.sws.speechify.com/v1/audio/speech`

Same parameters as streaming, returns complete audio buffer.

### 2.3 List Voices
**Endpoint:** `GET https://api.sws.speechify.com/v1/voices`

**Response:**
```json
{
  "voices": [
    {
      "id": "george",
      "name": "George",
      "gender": "male",
      "language": "en-US",
      "type": "shared"
    },
    {
      "id": "abc123xyz",
      "name": "My Cloned Voice",
      "gender": "female",
      "language": "en-US",
      "type": "personal"
    }
  ]
}
```

**Voice Types:**
- `shared`: System voices (short names like "george", "henry")
- `personal`: Cloned voices (alphanumeric IDs)

### 2.4 Create Cloned Voice
**Endpoint:** `POST https://api.sws.speechify.com/v1/voices`

**Content-Type:** `multipart/form-data`

**Parameters:**
| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| name | string | Yes | Name for the cloned voice |
| gender | enum | Yes | male, female, notSpecified |
| sample | file | Yes | Audio sample (10-30s, <5MB, WAV/MP3) |
| consent | string | Yes | JSON with fullName and email |
| locale | string | No | Locale code (default: en-US) |
| avatar | file | No | Avatar image |

**Sample Requirements:**
- Duration: 10-30 seconds (max 1 minute)
- Size: Under 5MB
- Quality: Clear speech, minimal background noise
- Format: WAV or MP3 recommended

### 2.5 Delete Voice
**Endpoint:** `DELETE https://api.sws.speechify.com/v1/voices/:id`

### 2.6 Download Voice Sample
**Endpoint:** `GET https://api.sws.speechify.com/v1/voices/:id/sample`

---

## 3. Models

### 3.1 Simba English (Default)
- **Languages:** English only
- **Features:** Clear, natural voice output
- **Use Case:** Standard English TTS applications
- **Voice Cloning:** Zero-shot supported

### 3.2 Simba Turbo
- **Languages:** English only
- **Features:** Faster processing, emotion control
- **Use Case:** Low-latency real-time applications
- **Voice Cloning:** Zero-shot supported

### 3.3 Simba Multilingual
- **Languages:** 50+ languages
- **Features:** Code-switching within sentences
- **Use Case:** Multi-language applications
- **Voice Cloning:** Zero-shot supported
- **Note:** Currently experimental

### 3.4 Simba Base (Legacy)
- **Languages:** English
- **Use Case:** Backward compatibility

---

## 4. Language Support

### Fully Supported (6)
| Language | Locale |
|----------|--------|
| English | en |
| French | fr-FR |
| German | de-DE |
| Spanish | es-ES |
| Portuguese (Brazil) | pt-BR |
| Portuguese (Portugal) | pt-PT |

### Beta Languages (17)
Arabic (ar-AE), Danish (da-DK), Dutch (nl-NL), Estonian (et-EE), Finnish (fi-FI), Greek (el-GR), Hebrew (he-IL), Hindi (hi-IN), Italian (it-IT), Japanese (ja-JP), Norwegian (nb-NO), Polish (pl-PL), Russian (ru-RU), Swedish (sv-SE), Turkish (tr-TR), Ukrainian (uk-UA), Vietnamese (vi-VN)

### Coming Soon (27)
Bengali, Bulgarian, Cantonese, Catalan, Croatian, Czech, Filipino, Georgian, Gujarati, Hungarian, Indonesian, Korean, Malay, Mandarin, Marathi, Nepali, Persian, Romanian, Serbian, Slovak, Tamil, Telugu, Thai, Urdu, and more

---

## 5. Pricing

| Plan | Price | Characters | Features |
|------|-------|------------|----------|
| Starter (Free) | $0 | 50,000 | 100 minutes, 250ms latency, 1000+ voices, SSML |
| Pay-As-You-Go | $10/1M chars | Unlimited | ~2000 minutes, Voice Cloning included |
| Enterprise | Custom | Custom | Security questionnaires, DPA/SLA, Priority support |

### Cost Comparison
- Speechify: $10/1M characters
- Claims: "20x cheaper than competitors"

### Free Tier Limits
- 50,000 characters
- ~100 minutes of audio
- No voice cloning
- All 1000+ preset voices
- SSML support

---

## 6. SSML Support

Speechify supports full Speech Synthesis Markup Language for precise control:

```xml
<speak>
  <prosody rate="slow" pitch="+2st">
    Hello, this is slower and higher pitched.
  </prosody>
  <break time="500ms"/>
  <emphasis level="strong">Important!</emphasis>
</speak>
```

### Supported Tags
- `<prosody>`: rate, pitch, volume
- `<break>`: time-based pauses
- `<emphasis>`: stress level
- `<say-as>`: interpret-as (date, time, currency, etc.)
- `<phoneme>`: custom pronunciation

---

## 7. Implementation Plan

### 7.1 Module Structure
```
src/core/tts/speechify/
├── mod.rs           # Module exports and constants (EXISTS)
├── config.rs        # SpeechifyTtsConfig, models, formats (EXISTS)
└── provider.rs      # SpeechifyTts implementing BaseTTS (TO CREATE)
```

### 7.2 Implementation Steps

1. **Create provider.rs**
   - SpeechifyRequestBuilder implementing TTSRequestBuilder
   - SpeechifyTts implementing BaseTTS
   - list_voices() static method
   - HTTP streaming via reqwest

2. **Update tts/mod.rs**
   - Add speechify module export
   - Add SpeechifyTts to re-exports

3. **Update plugin/builtin/mod.rs**
   - Add speechify metadata function
   - Add speechify factory function
   - Add inventory::submit! for registration

4. **Update plugin/dispatch.rs**
   - Add Speechify to BuiltinTTSProvider enum
   - Add canonical_name match
   - Add PHF map entries with aliases
   - Update BUILTIN_TTS_COUNT
   - Add "speechify" to BUILTIN_TTS_NAMES

5. **Update config.example.yaml**
   - Add SPEECHIFY_API_KEY entry
   - Add speechify TTS settings section

### 7.3 Configuration Mapping

| TTSConfig Field | Speechify Mapping |
|-----------------|-------------------|
| api_key | Authorization: Bearer header |
| voice_id | voice_id |
| model | model (simba-english, etc.) |
| audio_format | audio_format |
| language | language |

### 7.4 Provider Options
- `loudness_normalization`: Normalize to -14 LUFS
- `text_normalization`: Convert numbers to words

---

## 8. Testing Plan

### 8.1 Unit Tests
- Config parsing and validation
- Model enum serialization
- Audio format enum serialization
- Request body construction
- Response parsing
- Voice deserialization

### 8.2 Integration Tests (with credentials)
- Connection and voice listing
- Basic TTS synthesis
- Streaming audio chunks
- Different audio formats
- SSML input
- Error responses (invalid voice, rate limit)

### 8.3 Test Cases
```rust
#[test]
fn test_speechify_config_defaults()
#[test]
fn test_speechify_model_serialization()
#[test]
fn test_speechify_audio_format_serialization()
#[test]
fn test_request_body_construction()
#[tokio::test]
async fn test_speechify_connect()
#[tokio::test]
async fn test_speechify_speak_basic()
#[tokio::test]
async fn test_speechify_list_voices()
```

---

## 9. Best Practices

### Performance
- Use `simba-turbo` for lowest latency
- Use streaming endpoint for real-time applications
- Disable loudness_normalization for lower latency
- Cache voice list (doesn't change frequently)

### Quality
- Use `wav_48000` for highest quality
- Enable loudness_normalization for consistent output
- Use SSML for fine-grained control

### Cost Optimization
- Monitor character usage
- Use free tier for testing
- Batch text for efficient synthesis

### Security
- Never expose API token in client-side code
- Use server-side proxy for API calls
- Rotate tokens periodically

---

## 10. Error Handling

### HTTP Errors
| Status | Meaning | Action |
|--------|---------|--------|
| 400 | Bad Request | Check request body format |
| 401 | Unauthorized | Verify API token |
| 404 | Not Found | Verify voice_id exists |
| 429 | Rate Limited | Back off, retry later |
| 500 | Server Error | Retry with exponential backoff |

### SDK Retry Behavior
- Default: 2 retries with exponential backoff
- Retries on: HTTP 408, 429, 5XX
- Configurable via request_options

---

## 11. Available System Voices

Common preset voices include:
- `george` (default) - Male, English
- `henry` - Male, English
- `jack` - Male, English
- `kristy` - Female, English
- And 1000+ more across 50+ languages

Use `GET /v1/voices` to list all available voices.

---

## 12. References

- [Speechify API Overview](https://docs.sws.speechify.com/)
- [Speechify Quickstart](https://docs.sws.speechify.com/docs/get-started/quickstart)
- [Language Support](https://docs.sws.speechify.com/docs/features/language-support)
- [SSML Guide](https://docs.sws.speechify.com/docs/features/ssml)
- [Voice Cloning](https://docs.sws.speechify.com/docs/features/voice-cloning)
- [Models](https://docs.sws.speechify.com/docs/get-started/models)
- [Pricing](https://speechify.com/pricing-api/)
- [Python SDK](https://github.com/SpeechifyInc/speechify-api-sdk-python)
- [LiveKit Plugin](https://docs.livekit.io/agents/models/tts/plugins/speechify/)
