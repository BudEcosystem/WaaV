# CereProc CereVoice Cloud TTS Provider

## Overview

CereProc (https://www.cereproc.com) is a UK-based speech synthesis company specializing in characterful, emotional text-to-speech voices. Their CereVoice Cloud API provides cloud-based TTS synthesis with support for expressive voices, emotional variations, and Celtic languages.

**API Version:** v2 (REST)
**Base URL:** `https://api.cerevoice.com/v2/`
**Legacy REST URL:** `https://cerevoice.com/rest/rest_1_1.php` (v1)

## Authentication

CereVoice Cloud API v2 uses Bearer token authentication:

1. **Obtain Token**: POST `/auth` with Basic Auth credentials (email/password)
2. **Use Token**: Include `Authorization: Bearer {token}` in subsequent requests

### Token Request

```bash
curl -X POST "https://api.cerevoice.com/v2/auth" \
  -H "Authorization: Basic {base64(email:password)}"
```

### Response

```json
{
  "token": "eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9..."
}
```

## Text-to-Speech Synthesis

### Endpoint

```
POST /speak?voice={voice_name}
```

### Headers

| Header | Value |
|--------|-------|
| Authorization | Bearer {token} |
| Content-Type | text/xml |
| Accept | application/json |

### Request Body

Plain text or SSML/XML markup:

```xml
<doc>Hello. My name is Stuart. I can speak with <emotion name="happy">great enthusiasm!</emotion></doc>
```

### Example Request

```bash
curl -X POST "https://api.cerevoice.com/v2/speak?voice=Stuart" \
  -H "Authorization: Bearer {token}" \
  -H "Content-Type: text/xml" \
  -H "Accept: application/json" \
  -d "<doc>Hello world!</doc>"
```

### Response

```json
{
  "fileUrl": "https://cerevoice.com/files/generated/abc123.wav",
  "charCount": "12",
  "resultCode": "1",
  "resultDescription": "Success"
}
```

## Audio Formats

Supported output formats (query via `ListAudioFormats`):

| Format | Extension | Description |
|--------|-----------|-------------|
| wav | .wav | WAV audio |
| mp3 | .mp3 | MP3 audio |
| ogg | .ogg | OGG Vorbis |
| raw | .raw | Raw PCM |

## Sample Rates

| Rate | Use Case |
|------|----------|
| 22050 | Standard quality (default) |
| 16000 | Telephony |
| 8000 | Low bandwidth |

## Available Voices

CereVoice offers characterful voices across multiple languages:

### English Voices (with emotions)

| Voice | Language | Emotions Available |
|-------|----------|-------------------|
| Adam | en-GB | happy, sad, calm, cross |
| Caitlin | en-IE | happy, sad, calm, cross |
| Heather | en-SC | happy, sad, calm, cross |
| Isabella | en-US | happy, sad, calm, cross |
| Jack | en-GB | happy, sad, calm, cross |
| Jess | en-GB | happy, sad, calm, cross |
| Katherine | en-GB | happy, sad, calm, cross |
| Kirsty | en-SC | happy, sad, calm, cross |
| Laura | en-GB | happy, sad, calm, cross |
| Sarah | en-GB | happy, sad, calm, cross |
| Stuart | en-SC | happy, sad, calm, cross |
| Suzanne | en-US | happy, sad, calm, cross |
| William | en-GB | happy, sad, calm, cross |

### Celtic Languages

| Voice | Language | Description |
|-------|----------|-------------|
| Gwyneth | cy-GB | Welsh female |
| Geraint | cy-GB | Welsh male |
| Seoras | gd-GB | Scottish Gaelic male |
| Ceitidh | gd-GB | Scottish Gaelic female |
| Peadar | ga-IE | Irish Gaelic male |
| Sile | ga-IE | Irish Gaelic female |

### Other Languages

| Voice | Language |
|-------|----------|
| Claire | fr-FR |
| Gudrun | de-DE |
| Nicole | nl-NL |
| Sara | es-ES |
| Mia | it-IT |
| Ylva | sv-SE |

## SSML and Custom Tags

### Emotion Tags

```xml
<emotion name="happy">I'm so excited!</emotion>
<emotion name="sad">I'm feeling down.</emotion>
<emotion name="calm">Everything is peaceful.</emotion>
<emotion name="cross">This is frustrating!</emotion>
```

### Vocal Gestures (Spurt Tags)

Insert non-speech sounds using `<spurt>` tags:

```xml
<spurt audio="g0001_001"><!-- laughter --></spurt>
<spurt audio="g0001_002"><!-- cough --></spurt>
```

Over 50 vocal gestures available for each voice.

### Voice Switching

Use multiple voices in one request:

```xml
<voice name="Stuart">Hello, I'm Stuart.</voice>
<voice name="Katherine">And I'm Katherine.</voice>
```

### Variant Selection

Request alternative pronunciations:

```xml
<usel variant="1">read</usel>
<usel variant="2">read</usel>
```

## Credit System

CereVoice Cloud uses a credit-based billing model:

- **1 credit = 1 character** of input text
- **Free tier**: 10,000 characters/month
- **Paid credits**: 1,000,000 credits for £12.99

### Check Credit Balance

```bash
curl -X GET "https://api.cerevoice.com/v2/credit" \
  -H "Authorization: Bearer {token}"
```

### Response

```json
{
  "freeCredit": "8500",
  "paidCredit": "0",
  "charsAvailable": "8500"
}
```

## Custom Lexicons

Upload custom pronunciation dictionaries:

### Upload Lexicon

```bash
curl -X POST "https://api.cerevoice.com/v2/lexicon" \
  -H "Authorization: Bearer {token}" \
  -H "Content-Type: application/xml" \
  -F "file=@lexicon.xml" \
  -F "language=en" \
  -F "accent=GB"
```

### List Lexicons

```bash
curl -X GET "https://api.cerevoice.com/v2/lexicons" \
  -H "Authorization: Bearer {token}"
```

## Abbreviations

Upload custom abbreviation expansions:

```bash
curl -X POST "https://api.cerevoice.com/v2/abbreviations" \
  -H "Authorization: Bearer {token}" \
  -F "file=@abbreviations.txt" \
  -F "language=en"
```

## Error Codes

| Code | Description |
|------|-------------|
| 1 | Success |
| 0 | General error |
| -1 | Authentication failed |
| -2 | Insufficient credits |
| -3 | Invalid voice |
| -4 | Invalid audio format |
| -5 | Text too long |

## Rate Limits

- **Requests**: No explicit rate limit documented
- **Text length**: Varies by account type
- **Concurrent**: Standard accounts may have limits

## Integration Notes

### WaaV Implementation

The WaaV gateway implementation uses the v2 REST API:

1. **Config**: `src/core/tts/cereproc/config.rs`
   - Email/password credentials
   - Voice selection
   - Audio format options

2. **Messages**: `src/core/tts/cereproc/messages.rs`
   - Auth request/response
   - Speak request/response
   - Error handling

3. **Provider**: `src/core/tts/cereproc/provider.rs`
   - Token caching with refresh
   - HTTP request builder
   - Audio download and streaming

### Configuration Example

```yaml
tts:
  provider: cereproc
  api_key: "user@example.com:password123"
  voice_id: Stuart
  audio_format: mp3
  sample_rate: 22050
```

### Usage

```rust
use waav_gateway::core::tts::cereproc::CereprocTts;
use waav_gateway::core::tts::base::{BaseTTS, TTSConfig};

let config = TTSConfig {
    api_key: "email:password".to_string(),
    voice_id: Some("Stuart".to_string()),
    audio_format: Some("mp3".to_string()),
    ..Default::default()
};

let mut tts = CereprocTts::new(config)?;
tts.connect().await?;
tts.speak("Hello world!", true).await?;
```

## References

- [CereVoice Cloud API v2](https://api.cerevoice.com/v2/)
- [CereProc Website](https://app.cereproc.com/)
- [CereCloud Portal](https://app.cereproc.com/cerecloud/)
