# OpenAI TTS Provider

WaaV Gateway supports OpenAI's Text-to-Speech API for speech synthesis.

## Overview

OpenAI TTS provides high-quality voice synthesis with:
- Natural-sounding voices
- Multiple quality tiers
- Wide range of output formats
- Speed control

## Supported Models

| Model | Description | Best For |
|-------|-------------|----------|
| `tts-1` | Standard TTS model | Low-latency applications |
| `tts-1-hd` | High-definition model | Higher quality output |
| `gpt-4o-mini-tts` | GPT-4o powered TTS | Enhanced naturalness |

## Supported Voices

| Voice | Description |
|-------|-------------|
| `alloy` | Neutral, versatile |
| `ash` | Warm, conversational |
| `ballad` | Expressive, emotional |
| `coral` | Clear, professional |
| `echo` | Soft, calming |
| `fable` | Storytelling, narrative |
| `onyx` | Deep, authoritative |
| `nova` | Friendly, upbeat |
| `sage` | Wise, measured |
| `shimmer` | Bright, energetic |
| `verse` | Poetic, flowing |

## Configuration

### Environment Variable

```bash
export OPENAI_API_KEY="sk-..."
```

### YAML Configuration

```yaml
providers:
  openai_api_key: "sk-..."
```

### WebSocket Config Message

```json
{
  "type": "config",
  "tts": {
    "provider": "openai",
    "model": "tts-1-hd",
    "voice_id": "nova"
  }
}
```

## Configuration Options

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `provider` | string | - | Must be `"openai"` |
| `model` | string | `"tts-1"` | TTS model to use |
| `voice_id` | string | `"alloy"` | Voice to use |
| `speaking_rate` | float | `1.0` | Speed multiplier (0.25-4.0) |
| `audio_format` | string | `"pcm"` | Output format |

## Audio Formats

| Format | Sample Rate | Description |
|--------|-------------|-------------|
| `pcm` | 24000 Hz | Raw PCM 16-bit little-endian |
| `mp3` | - | MP3 compressed |
| `opus` | - | Opus compressed |
| `aac` | - | AAC compressed |
| `flac` | - | FLAC lossless |
| `wav` | - | WAV container |

For WebSocket streaming, `pcm` format is recommended for lowest latency.

## Implementation Details

### Architecture

OpenAI TTS uses HTTP REST API with streaming response:

1. Text is sent to the API endpoint
2. Audio is streamed back as chunks
3. Chunks are forwarded to the client in real-time

### Speed Control

Speed can be adjusted from 0.25x to 4.0x:
- Values below 0.25 are clamped to 0.25
- Values above 4.0 are clamped to 4.0

```json
{
  "type": "config",
  "tts": {
    "provider": "openai",
    "voice_id": "nova",
    "speaking_rate": 1.2
  }
}
```

## Usage Example

### TypeScript

```typescript
import { BudFoundry } from '@bud-foundry/sdk';

const client = new BudFoundry({ baseUrl: 'ws://localhost:3001' });

await client.connect({
  tts: {
    provider: 'openai',
    model: 'tts-1-hd',
    voice: 'nova'
  }
});

client.on('audio', (chunk) => {
  // Play audio chunk
  playAudio(chunk.data);
});

// Trigger speech
client.speak('Hello, welcome to WaaV Gateway!');
```

### Python

```python
from bud_foundry import BudFoundry, TTSConfig

client = BudFoundry(base_url='ws://localhost:3001')

await client.connect(
    tts=TTSConfig(
        provider='openai',
        model='tts-1-hd',
        voice='nova'
    )
)

@client.on('audio')
async def on_audio(chunk):
    # Play audio chunk
    await play_audio(chunk.data)

# Trigger speech
await client.speak('Hello, welcome to WaaV Gateway!')
```

### REST API

You can also use the `/speak` endpoint directly:

```bash
curl -X POST http://localhost:3001/speak \
  -H "Content-Type: application/json" \
  -d '{
    "text": "Hello, world!",
    "provider": "openai",
    "voice": "nova",
    "model": "tts-1-hd"
  }' \
  --output audio.pcm
```

## Provider Info

Get supported voices and models:

```bash
curl http://localhost:3001/voices?provider=openai
```

Response:

```json
{
  "provider": "openai",
  "api_type": "HTTP REST",
  "default_sample_rate": 24000,
  "supported_models": ["tts-1", "tts-1-hd", "gpt-4o-mini-tts"],
  "supported_voices": ["alloy", "ash", "ballad", "coral", "echo", "fable", "onyx", "nova", "sage", "shimmer", "verse"],
  "supported_formats": ["mp3", "opus", "aac", "flac", "wav", "pcm"],
  "speed_range": { "min": 0.25, "max": 4.0 }
}
```

## Error Handling

| Error Code | Description | Resolution |
|------------|-------------|------------|
| `AuthenticationFailed` | Invalid API key | Check OPENAI_API_KEY |
| `SynthesisFailed` | API returned error | Check input text |
| `RateLimitExceeded` | Too many requests | Implement backoff |

## Testing

Run integration tests with:

```bash
OPENAI_API_KEY=sk-... cargo test openai_tts -- --ignored --nocapture
```

## Related

- [OpenAI STT](./openai-stt.md)
- [OpenAI Realtime](./openai-realtime.md)
- [TTS Configuration](./websocket.md#tts-configuration)
