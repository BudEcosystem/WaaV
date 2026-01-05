# OpenAI STT (Whisper) Provider

WaaV Gateway supports OpenAI's Whisper API for Speech-to-Text transcription.

## Overview

OpenAI's Whisper is a general-purpose speech recognition model capable of:
- Multilingual speech recognition (99 languages)
- Speech translation
- Language identification
- Robust handling of accents, background noise, and technical language

## Supported Models

| Model | Description | Best For |
|-------|-------------|----------|
| `whisper-1` | Original Whisper model | General transcription |
| `gpt-4o-transcribe` | GPT-4o powered transcription | Higher accuracy |
| `gpt-4o-mini-transcribe` | Smaller GPT-4o model | Cost-effective |

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
  "stt": {
    "provider": "openai",
    "model": "whisper-1",
    "language": "en"
  }
}
```

## Configuration Options

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `provider` | string | - | Must be `"openai"` |
| `model` | string | `"whisper-1"` | Whisper model to use |
| `language` | string | `"en"` | ISO-639-1 language code |
| `response_format` | string | `"json"` | Output format: `json`, `text`, `verbose_json`, `srt`, `vtt` |
| `temperature` | float | `0.0` | Sampling temperature (0-1) |

## Implementation Details

### Architecture

Unlike other STT providers that use WebSocket streaming, OpenAI Whisper uses a REST-based approach:

1. Audio is accumulated in a buffer during the session
2. On `flush()` or disconnect, audio is sent to the API
3. Complete transcript is returned

### Audio Format

- **Input**: Raw PCM audio (16-bit, mono)
- **Sample Rate**: 16000 Hz (configurable)
- **Format Conversion**: Automatically converted to WAV for API submission
- **Max File Size**: 25 MB per request

### Response Format

The default `json` format returns:

```json
{
  "text": "Hello, how are you today?"
}
```

The `verbose_json` format includes word-level timestamps:

```json
{
  "text": "Hello, how are you today?",
  "segments": [...],
  "words": [
    { "word": "Hello", "start": 0.0, "end": 0.5 },
    { "word": "how", "start": 0.6, "end": 0.8 },
    ...
  ]
}
```

## Usage Example

### TypeScript

```typescript
import { BudFoundry } from '@bud-foundry/sdk';

const client = new BudFoundry({ baseUrl: 'ws://localhost:3001' });

await client.connect({
  stt: {
    provider: 'openai',
    model: 'whisper-1',
    language: 'en'
  }
});

client.on('transcript', (result) => {
  console.log('Transcript:', result.text);
});

// Send audio...
```

### Python

```python
from bud_foundry import BudFoundry

client = BudFoundry(base_url='ws://localhost:3001')

await client.connect(
    stt=STTConfig(
        provider='openai',
        model='whisper-1',
        language='en'
    )
)

@client.on('transcript')
async def on_transcript(result):
    print(f'Transcript: {result.text}')
```

## Supported Languages

OpenAI Whisper supports 99 languages including:

- English (`en`)
- Spanish (`es`)
- French (`fr`)
- German (`de`)
- Chinese (`zh`)
- Japanese (`ja`)
- Korean (`ko`)
- Arabic (`ar`)
- Hindi (`hi`)
- Portuguese (`pt`)
- And many more...

For the complete list, see [OpenAI Whisper documentation](https://platform.openai.com/docs/guides/speech-to-text).

## Error Handling

| Error Code | Description | Resolution |
|------------|-------------|------------|
| `AuthenticationFailed` | Invalid API key | Check OPENAI_API_KEY |
| `TranscriptionError` | API returned error | Check audio format and size |
| `AudioTooShort` | Audio < 0.1 seconds | Accumulate more audio |

## Limitations

- **Not Real-time**: Unlike WebSocket providers, Whisper requires complete audio
- **Max 25MB**: Audio files larger than 25MB must be chunked
- **REST Latency**: Higher latency than streaming providers due to REST round-trip

## Testing

Run integration tests with:

```bash
OPENAI_API_KEY=sk-... cargo test openai_stt -- --ignored --nocapture
```

## Related

- [OpenAI TTS](./openai-tts.md)
- [OpenAI Realtime](./openai-realtime.md)
- [STT Configuration](./websocket.md#stt-configuration)
