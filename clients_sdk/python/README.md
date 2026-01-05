# bud-foundry

Python SDK for Bud Foundry AI Gateway - Speech-to-Text, Text-to-Speech, and Voice AI.

## Installation

```bash
pip install bud-foundry
```

## Quick Start

```python
from bud_foundry import BudClient
import asyncio

async def main():
    bud = BudClient(base_url="http://localhost:3001", api_key="your-api-key")

    # STT (Speech-to-Text)
    async with bud.stt.connect(provider="deepgram") as session:
        async for result in session.transcribe_stream(audio_generator()):
            print(result.text)

    # TTS (Text-to-Speech)
    async with bud.tts.connect(provider="elevenlabs") as session:
        await session.speak("Hello, world!")

    # Talk (Bidirectional Voice)
    async with bud.talk.connect(
        stt={"provider": "deepgram"},
        tts={"provider": "elevenlabs"}
    ) as session:
        async for event in session:
            if event.type == "transcript":
                print(event.text)

asyncio.run(main())
```

## Features

- **STT (Speech-to-Text)**: Real-time streaming transcription with multiple providers
- **TTS (Text-to-Speech)**: High-quality speech synthesis with voice selection
- **Talk**: Bidirectional voice conversations with VAD and interruption handling
- **Performance Metrics**: Built-in TTFT, TTFB, and E2E latency tracking
- **Feature Flags**: VAD, noise cancellation, speaker diarization, and more

## Configuration

```python
from bud_foundry import BudClient, STTConfig, TTSConfig, FeatureFlags

bud = BudClient(
    base_url="https://api.bud.ai",
    api_key="bud_xxx",
    timeout=30.0
)

# With feature flags
async with bud.stt.connect(
    config=STTConfig(
        provider="deepgram",
        language="en-US",
        model="nova-3"
    ),
    features=FeatureFlags(
        vad=True,
        noise_cancellation=True,
        speaker_diarization=True
    )
) as session:
    # ...
```

## API Reference

### BudClient

Main entry point for the SDK.

```python
bud = BudClient(base_url="http://localhost:3001", api_key="your-key")

# Access pipelines
bud.stt      # Speech-to-Text
bud.tts      # Text-to-Speech
bud.talk     # Bidirectional voice
bud.livekit  # LiveKit operations
bud.sip      # SIP operations

# Health check
health = await bud.health()

# List voices
voices = await bud.list_voices(provider="elevenlabs")
```

### BudSTT

```python
async with bud.stt.connect(provider="deepgram") as session:
    # Stream audio
    async for result in session.transcribe_stream(audio_generator()):
        print(f"[{result.speaker_id}] {result.text}")
        if result.is_final:
            process_final(result.text)

    # Get metrics
    metrics = session.get_metrics()
    print(f"TTFT p95: {metrics.stt.ttft.p95}ms")
```

### BudTTS

```python
async with bud.tts.connect(provider="elevenlabs", voice="rachel") as session:
    # Speak text
    await session.speak("Hello, how can I help you?")

    # Handle audio events
    async for chunk in session:
        player.play(chunk.audio)
```

### BudTalk

```python
async with bud.talk.connect(
    stt={"provider": "deepgram"},
    tts={"provider": "elevenlabs", "voice": "rachel"}
) as session:
    # Handle all events
    async for event in session:
        if event.type == "transcript":
            response = generate_response(event.text)
            await session.speak(response)
        elif event.type == "audio":
            player.play(event.audio)
```

## Sync API

For synchronous usage:

```python
from bud_foundry.sync import BudClient

bud = BudClient(base_url="http://localhost:3001", api_key="your-key")

# One-shot TTS
audio = bud.tts.synthesize("Hello world", provider="deepgram")

# Batch transcription
result = bud.stt.transcribe("audio.wav", language="en-US")
print(result.text)
```

## License

MIT
