# OpenAI Realtime API Provider

WaaV Gateway supports OpenAI's Realtime API for full-duplex audio-to-audio streaming.

## Overview

OpenAI Realtime API enables:
- Bidirectional audio streaming
- Natural conversation with GPT-4o
- Voice Activity Detection (VAD)
- Function calling during audio sessions
- Input transcription
- Low-latency responses

## Supported Models

| Model | Description |
|-------|-------------|
| `gpt-4o-realtime-preview` | Full GPT-4o model |
| `gpt-4o-mini-realtime-preview` | Cost-effective mini model |

## Supported Voices

| Voice | Description |
|-------|-------------|
| `alloy` | Neutral, versatile |
| `ash` | Warm, conversational |
| `ballad` | Expressive, emotional |
| `coral` | Clear, professional |
| `echo` | Soft, calming |
| `sage` | Wise, measured |
| `shimmer` | Bright, energetic |
| `verse` | Poetic, flowing |

## Configuration

### Environment Variable

```bash
export OPENAI_API_KEY="sk-..."
```

### WebSocket Connection

Connect to the `/realtime` endpoint:

```
ws://localhost:3001/realtime
```

### Config Message

```json
{
  "type": "config",
  "provider": "openai",
  "model": "gpt-4o-realtime-preview",
  "voice": "alloy",
  "instructions": "You are a helpful assistant.",
  "turn_detection": "server_vad",
  "vad": {
    "enabled": true,
    "threshold": 0.5,
    "silence_duration_ms": 500,
    "prefix_padding_ms": 300
  },
  "input_transcription": {
    "enabled": true,
    "model": "whisper-1"
  },
  "temperature": 0.8
}
```

## Configuration Options

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `provider` | string | - | Must be `"openai"` |
| `model` | string | `"gpt-4o-realtime-preview"` | Realtime model |
| `voice` | string | `"alloy"` | Output voice |
| `instructions` | string | `""` | System instructions |
| `turn_detection` | string | `"server_vad"` | `"server_vad"` or `"none"` |
| `vad.enabled` | bool | `true` | Enable VAD |
| `vad.threshold` | float | `0.5` | VAD sensitivity (0-1) |
| `vad.silence_duration_ms` | int | `500` | Silence before turn end |
| `vad.prefix_padding_ms` | int | `300` | Audio padding |
| `input_transcription.enabled` | bool | `true` | Transcribe input |
| `input_transcription.model` | string | `"whisper-1"` | Transcription model |
| `temperature` | float | `0.8` | Response temperature |
| `max_response_tokens` | int/string | `"inf"` | Token limit |

## Audio Format

| Parameter | Value |
|-----------|-------|
| Sample Rate | 24000 Hz |
| Channels | 1 (mono) |
| Bit Depth | 16-bit |
| Encoding | PCM little-endian |

Both input and output audio use the same format.

## Protocol

### Client to Server

1. **Config Message** (JSON): Configure the session
2. **Audio Data** (Binary): Send PCM audio chunks

### Server to Client

1. **Ready Message**: Session established
   ```json
   { "type": "ready", "session_id": "..." }
   ```

2. **Transcript**: User input transcription
   ```json
   {
     "type": "transcript",
     "role": "user",
     "text": "Hello, how are you?",
     "is_final": true
   }
   ```

3. **Transcript**: Assistant response text
   ```json
   {
     "type": "transcript",
     "role": "assistant",
     "text": "I'm doing well, thank you!",
     "is_final": true
   }
   ```

4. **Audio Data** (Binary): Assistant voice response

5. **Speech Events**: VAD events
   ```json
   { "type": "speech_started", "audio_ms": 1500 }
   { "type": "speech_stopped", "audio_ms": 3200 }
   ```

6. **Error**: Error occurred
   ```json
   { "type": "error", "message": "..." }
   ```

## Implementation Details

### Connection Flow

```
Client                              Gateway                         OpenAI
  |                                    |                               |
  |-- ws://host/realtime ------------->|                               |
  |                                    |-- wss://api.openai.com ------>|
  |                                    |<-- session.created -----------|
  |<-- { type: "ready" } --------------|                               |
  |                                    |                               |
  |-- { type: "config", ... } -------->|                               |
  |                                    |-- session.update ------------>|
  |                                    |                               |
  |-- [binary audio] ----------------->|                               |
  |                                    |-- input_audio_buffer.append ->|
  |                                    |                               |
  |                                    |<-- speech_started ------------|
  |<-- { type: "speech_started" } -----|                               |
  |                                    |                               |
  |                                    |<-- speech_stopped ------------|
  |<-- { type: "speech_stopped" } -----|                               |
  |                                    |                               |
  |                                    |<-- transcription.completed ---|
  |<-- { type: "transcript", role: "user" } ---|                       |
  |                                    |                               |
  |                                    |<-- response.audio.delta ------|
  |<-- [binary audio] -----------------|                               |
  |                                    |                               |
  |                                    |<-- response.audio_transcript.delta
  |<-- { type: "transcript", role: "assistant" }                       |
```

### VAD Behavior

When `turn_detection: "server_vad"`:
1. OpenAI automatically detects speech start/end
2. Responses are triggered automatically after silence
3. No manual `response.create` needed

When `turn_detection: "none"`:
1. Client must manually trigger responses
2. Use for push-to-talk interfaces

## Usage Example

### TypeScript

```typescript
import { createRealtimeConfig, RealtimeSessionConfig } from '@bud-foundry/sdk';

// Create config with defaults
const config: RealtimeSessionConfig = createRealtimeConfig('openai', {
  voice: 'nova',
  instructions: 'You are a helpful assistant.',
});

// Connect to realtime endpoint
const ws = new WebSocket('ws://localhost:3001/realtime');

ws.onopen = () => {
  // Send config
  ws.send(JSON.stringify({
    type: 'config',
    ...config
  }));
};

ws.onmessage = (event) => {
  if (typeof event.data === 'string') {
    const msg = JSON.parse(event.data);

    if (msg.type === 'transcript') {
      console.log(`[${msg.role}] ${msg.text}`);
    } else if (msg.type === 'ready') {
      console.log('Session ready!');
      startAudioStream();
    }
  } else {
    // Binary audio data
    playAudio(event.data);
  }
};

function startAudioStream() {
  // Get microphone and send audio chunks
  navigator.mediaDevices.getUserMedia({ audio: true })
    .then(stream => {
      const audioContext = new AudioContext({ sampleRate: 24000 });
      // ... process and send audio
    });
}
```

### Python

```python
import asyncio
import websockets
import json

async def realtime_session():
    uri = 'ws://localhost:3001/realtime'

    async with websockets.connect(uri) as ws:
        # Send config
        await ws.send(json.dumps({
            'type': 'config',
            'provider': 'openai',
            'model': 'gpt-4o-realtime-preview',
            'voice': 'nova',
            'instructions': 'You are a helpful assistant.'
        }))

        async for message in ws:
            if isinstance(message, str):
                data = json.loads(message)

                if data['type'] == 'ready':
                    print('Session ready!')
                    # Start sending audio

                elif data['type'] == 'transcript':
                    print(f"[{data['role']}] {data['text']}")

            else:
                # Binary audio response
                await play_audio(message)

asyncio.run(realtime_session())
```

## Error Handling

| Error | Description | Resolution |
|-------|-------------|------------|
| `AuthenticationFailed` | Invalid API key | Check OPENAI_API_KEY |
| `NotConnected` | Send before connect | Wait for ready message |
| `ConnectionFailed` | WebSocket failed | Check network/API status |
| `RateLimitExceeded` | API rate limit | Implement backoff |

## Testing

Run integration tests with:

```bash
OPENAI_API_KEY=sk-... cargo test openai_realtime -- --ignored --nocapture
```

## Limitations

- **Requires API Key**: Direct API access needed
- **Beta API**: Subject to changes
- **Cost**: Higher cost than STT/TTS separately
- **Audio Only**: Text input supported but optimized for voice

## Best Practices

1. **Use VAD**: Server-side VAD handles turn-taking automatically
2. **Handle Interruptions**: Be prepared for speech to stop mid-response
3. **Buffer Audio**: Buffer outgoing audio for smooth playback
4. **Error Recovery**: Implement reconnection logic

## Related

- [OpenAI STT](./openai-stt.md)
- [OpenAI TTS](./openai-tts.md)
- [WebSocket API](./websocket.md)
