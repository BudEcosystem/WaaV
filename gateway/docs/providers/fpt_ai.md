# FPT.AI STT+TTS Provider

## Status: READY_TO_IMPLEMENT

**Last Updated:** 2026-01-14
**Provider #:** 48
**Priority:** High (Southeast Asia - Vietnam)

## Overview

FPT.AI is Vietnam's leading AI platform from FPT Technology Innovation Department. They offer high-quality Vietnamese speech synthesis and recognition with regional accent support (Northern, Central, Southern Vietnam).

## Company Information

- **Company:** FPT Corporation (FPT Technology Innovation)
- **Country:** Vietnam
- **Website:** https://fpt.ai
- **Developer Console:** https://console.fpt.ai
- **Voice Maker:** https://voicemaker.fpt.ai
- **Technologies:** STT, TTS
- **Free Tier:** Available for evaluation

## Text-to-Speech (TTS) API

### Endpoint

```
POST https://api.fpt.ai/hmi/tts/v5
```

### Authentication

- **Method:** API Key header
- **Header:** `api_key: {your_api_key}`
- **Get Key:** Register at https://console.fpt.ai

### Request Headers

| Header | Required | Default | Description |
|--------|----------|---------|-------------|
| `api_key` | Yes | - | API key from console.fpt.ai |
| `voice` | No | female | Voice ID (see Voice Options below) |
| `speed` | No | 0 | Speed range: -3 (slow) to +3 (fast), 0 is normal |
| `format` | No | mp3 | Output format: mp3 or wav |
| `callback_url` | No | - | Webhook URL for completion notification |

### Request Body

- **Content-Type:** text/plain
- **Body:** Plain text to synthesize (UTF-8)
- **Character Limit:** 5,000 characters per request
- **Minimum:** 3 characters

### Voice Options

| Voice ID | Gender | Accent | Description |
|----------|--------|--------|-------------|
| banmai | Female | Northern | Default northern female voice |
| lannhi | Female | Northern | Northern female voice |
| leminh | Male | Northern | Warm northern male voice |
| myan | Female | - | Female voice |
| thuminh | Female | - | Female voice |
| giahuy | Male | - | Male voice |
| linhsan | Female | - | Female voice |

**Legacy Voices (v4 API):**
- `female` - Young northern female (slower speech)
- `male` - Older northern male (with breath sounds)
- `hatieumai` - Southern female
- `ngoclam` - Central (Hue) female

### Response Format

```json
{
  "error": 0,
  "async": "https://file.fpt.ai/...",
  "request_id": "xxxx-xxxx-xxxx",
  "message": "Synthesize successful"
}
```

### Error Codes

| Code | Meaning |
|------|---------|
| 0 | Success |
| Non-zero | Error (check message field) |

### Processing Time

- Wait time: 5 seconds to 2 minutes
- Depends on text length
- Use callback_url for async notification

### Example Request

```bash
curl -X POST \
  -H "api_key: YOUR_API_KEY" \
  -H "voice: banmai" \
  -H "speed: 0" \
  -H "format: mp3" \
  -d 'Bạn thấy dịch vụ TTS của FPT có tốt không?' \
  "https://api.fpt.ai/hmi/tts/v5"
```

## Speech-to-Text (STT) API

### Endpoint

```
POST https://api.fpt.ai/hmi/asr/general
```

### Authentication

- **Method:** API Key header
- **Header:** `api_key: {your_api_key}`

### Request Format

- **Method:** POST
- **Content-Type:** audio/wav (or audio file binary)
- **Body:** Raw audio file data

### Response Format

```json
{
  "status": 0,
  "hypotheses": [
    {
      "utterance": "Recognized Vietnamese text"
    }
  ],
  "id": "request-id"
}
```

### Status Codes

| Status | Meaning |
|--------|---------|
| 0 | Success |
| 1 | No voice detected |
| 2 | Canceled |
| 9 | System busy |

### Example Request

```bash
curl -X POST \
  -H "api_key: YOUR_API_KEY" \
  -T "/path/to/audio.wav" \
  "https://api.fpt.ai/hmi/asr/general"
```

## Implementation Notes

### TTS Implementation Pattern

FPT.AI TTS uses a two-step async pattern:
1. POST text to synthesis endpoint
2. Receive JSON with audio URL
3. Download audio from the `async` URL
4. Optionally use `callback_url` for completion notification

**Similar to:** Zalo AI TTS (same region, similar pattern)

### STT Implementation Pattern

FPT.AI STT uses HTTP file upload:
1. POST audio file to ASR endpoint
2. Receive JSON with transcription
3. Handle status codes for errors

**Similar to:** Simple HTTP-based STT providers

### Audio Format

- **TTS Output:** MP3 or WAV
- **STT Input:** Standard audio files (WAV recommended)

## Integration Approach

### TTS Provider

1. Follow `ZaloTts` implementation pattern
2. Two-step synthesis: POST -> URL -> Download
3. Handle async responses with audio URL
4. Support 7 voice options
5. Support speed control (-3 to +3)

### STT Provider

1. Follow HTTP file upload pattern
2. Simple POST with audio data
3. Parse JSON response for transcription
4. Handle status codes

## Environment Variables

```bash
export FPT_AI_API_KEY="your-api-key-from-console-fpt-ai"
```

## References

- [FPT.AI Website](https://fpt.ai)
- [FPT.AI TTS Page](https://fpt.ai/tts)
- [API Documentation](https://docs.fpt.ai/docs/en/speech/api/text-to-speech/)
- [STT Documentation](https://docs.fpt.ai/docs/en/speech/api/speech-to-text/)
- [Console](https://console.fpt.ai)
- [Voice Maker](https://voicemaker.fpt.ai)

## Next Steps

1. Implement TTS provider following Zalo AI pattern
2. Implement STT provider with HTTP file upload
3. Test with Vietnamese text samples
4. Add to plugin registry
5. Update integration status
