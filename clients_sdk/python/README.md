# bud-foundry

Python SDK for Bud Foundry AI Gateway - Speech-to-Text, Text-to-Speech, Realtime Audio-to-Audio, and Voice AI.

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
- **Realtime**: Full-duplex audio-to-audio with OpenAI Realtime and Hume EVI
- **Emotion Control**: Unified 22-emotion system with delivery styles
- **Voice Cloning**: Clone voices with ElevenLabs, PlayHT, and Hume
- **DAG Pipelines**: Configure custom audio processing workflows
- **Audio Features**: Turn detection, noise filtering, VAD
- **Performance Metrics**: Built-in TTFT, TTFB, and E2E latency tracking

---

## Providers

### STT Providers (10)

| Provider | ID | Features |
|----------|-----|----------|
| Deepgram | `deepgram` | Streaming, word-timestamps, diarization |
| Google Cloud | `google` | Streaming, word-timestamps, diarization |
| Azure | `azure` | Streaming, word-timestamps, punctuation |
| Cartesia | `cartesia` | Streaming, low-latency |
| OpenAI Whisper | `openai-whisper` | Batch transcription, 57+ languages |
| AssemblyAI | `assemblyai` | Streaming, diarization, sentiment |
| AWS Transcribe | `aws-transcribe` | Streaming, 100+ languages |
| IBM Watson | `ibm-watson` | Streaming, diarization |
| Groq | `groq` | Ultra-fast (216x real-time) |
| Gateway | `gateway` | Local Whisper inference |

### TTS Providers (12)

| Provider | ID | Features |
|----------|-----|----------|
| Deepgram | `deepgram` | Streaming, 102 Aura voices |
| ElevenLabs | `elevenlabs` | Streaming, voice cloning, emotion |
| Google Cloud | `google` | SSML, neural voices |
| Azure | `azure` | Streaming, SSML, 400+ voices |
| Cartesia | `cartesia` | Low-latency, voice cloning |
| OpenAI | `openai` | tts-1, tts-1-hd, 11 voices |
| AWS Polly | `aws-polly` | SSML, neural/generative engines |
| IBM Watson | `ibm-watson` | SSML, rate/pitch control |
| Hume | `hume` | Emotion control, voice cloning |
| LMNT | `lmnt` | Low-latency (~150ms), voice cloning |
| PlayHT | `playht` | Voice cloning, emotion |
| Kokoro | `kokoro` | Local inference |

### Realtime Providers (2)

| Provider | ID | Features |
|----------|-----|----------|
| OpenAI Realtime | `openai-realtime` | Full-duplex, function calling, VAD |
| Hume EVI | `hume-evi` | Full-duplex, 48 emotion dimensions |

---

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

---

## Realtime Audio-to-Audio

Full-duplex bidirectional audio streaming with LLM integration.

### OpenAI Realtime

```python
from bud_foundry.pipelines.realtime import BudRealtime, RealtimeConfig, RealtimeProvider

realtime = BudRealtime(RealtimeConfig(
    provider=RealtimeProvider.OPENAI_REALTIME,
    api_key="your-openai-key",
    model="gpt-4o-realtime-preview",
    system_prompt="You are a helpful voice assistant.",
    voice_id="alloy",  # alloy, ash, ballad, coral, echo, sage, shimmer, verse
    temperature=0.8
))

# Register event handlers
realtime.on("transcript", lambda e: print(f"[{e.role}] {e.text}"))
realtime.on("audio", lambda e: play_audio(e.audio))
realtime.on("function_call", handle_function_call)

# Connect and stream
await realtime.connect("wss://gateway.example.com/realtime")
await realtime.send_audio(audio_bytes)

# Register tools for function calling
realtime.register_tool(ToolDefinition(
    name="get_weather",
    description="Get current weather for a location",
    parameters={
        "type": "object",
        "properties": {
            "location": {"type": "string", "description": "City name"}
        },
        "required": ["location"]
    }
))
```

### Hume EVI (Empathic Voice Interface)

```python
from bud_foundry.pipelines.realtime import BudRealtime, RealtimeConfig, RealtimeProvider

realtime = BudRealtime(RealtimeConfig(
    provider=RealtimeProvider.HUME_EVI,
    api_key="your-hume-key",
    evi_version="3",  # EVI version (1, 2, 3, or 4-mini)
    voice_id="your-voice-id",
    system_prompt="You are an empathetic assistant.",
    verbose_transcription=True
))

# Emotion events include 48 emotion dimensions
realtime.on("emotion", lambda e: print(f"Dominant: {e.dominant} ({e.emotions[e.dominant]:.2f})"))
realtime.on("transcript", lambda e: print(f"{e.text}"))

await realtime.connect("wss://gateway.example.com/realtime")
```

### Realtime Events

| Event | Description |
|-------|-------------|
| `audio` | Audio output chunk from assistant |
| `transcript` | Transcript (user or assistant) |
| `function_call` | Tool/function call from LLM |
| `emotion` | Emotion scores (Hume EVI only) |
| `connected` | Connection established |
| `disconnected` | Connection closed |
| `state_change` | State transition |
| `error` | Error occurred |

---

## Emotion Control

Unified emotion system with 22 emotions and 15 delivery styles.

### Using Emotions

```python
from bud_foundry.types import Emotion, DeliveryStyle, EmotionIntensityLevel

# With TTS configuration
async with bud.tts.connect(
    config=TTSConfig(
        provider="elevenlabs",
        voice="rachel",
        emotion=Emotion.HAPPY,
        emotion_intensity=EmotionIntensityLevel.HIGH,
        delivery_style=DeliveryStyle.CHEERFUL
    )
) as session:
    await session.speak("Great to meet you!")

# With Hume (natural language emotion)
async with bud.tts.connect(
    config=TTSConfig(
        provider="hume",
        voice="Kora",
        emotion=Emotion.EXCITED,
        emotion_intensity=0.8,
        delivery_style=DeliveryStyle.ENTHUSIASTIC,
        acting_instructions="whispered with excitement"  # Hume-specific
    )
) as session:
    await session.speak("I have amazing news!")
```

### Supported Emotions (22)

```python
class Emotion(str, Enum):
    NEUTRAL = "neutral"
    HAPPY = "happy"
    SAD = "sad"
    ANGRY = "angry"
    FEARFUL = "fearful"
    SURPRISED = "surprised"
    DISGUSTED = "disgusted"
    EXCITED = "excited"
    CALM = "calm"
    ANXIOUS = "anxious"
    CONFIDENT = "confident"
    CONFUSED = "confused"
    EMPATHETIC = "empathetic"
    SARCASTIC = "sarcastic"
    HOPEFUL = "hopeful"
    DISAPPOINTED = "disappointed"
    CURIOUS = "curious"
    GRATEFUL = "grateful"
    PROUD = "proud"
    EMBARRASSED = "embarrassed"
    CONTENT = "content"
    BORED = "bored"
```

### Delivery Styles (15)

```python
class DeliveryStyle(str, Enum):
    NORMAL = "normal"
    WHISPERED = "whispered"
    SHOUTED = "shouted"
    RUSHED = "rushed"
    MEASURED = "measured"
    MONOTONE = "monotone"
    EXPRESSIVE = "expressive"
    PROFESSIONAL = "professional"
    CASUAL = "casual"
    STORYTELLING = "storytelling"
    SOFT = "soft"
    LOUD = "loud"
    CHEERFUL = "cheerful"
    SERIOUS = "serious"
    FORMAL = "formal"
```

### Emotion Provider Support

| Provider | Emotion | Intensity | Delivery Style | Natural Language |
|----------|---------|-----------|----------------|------------------|
| ElevenLabs | via SSML | via SSML | via SSML | - |
| Azure | SSML styles | SSML styledegree | SSML role | - |
| Hume | Natural | 0.0-1.0 | Acting instructions | acting_instructions |
| Cartesia | Voice mixing | - | - | - |
| PlayHT | Emotion param | - | - | - |

---

## Voice Cloning

Clone voices from audio samples or descriptions.

```python
from bud_foundry.types import VoiceCloneRequest, VoiceCloneProvider

# Clone from audio samples (ElevenLabs)
with open("voice_sample.mp3", "rb") as f:
    audio_base64 = base64.b64encode(f.read()).decode()

clone_result = await bud.voice.clone(VoiceCloneRequest(
    provider=VoiceCloneProvider.ELEVENLABS,
    name="My Custom Voice",
    description="Professional male voice",
    audio_samples=[audio_base64],
    remove_background_noise=True,
    labels={"gender": "male", "accent": "american"}
))

print(f"Cloned voice ID: {clone_result.voice_id}")

# Use the cloned voice
async with bud.tts.connect(
    config=TTSConfig(
        provider="elevenlabs",
        voice_id=clone_result.voice_id
    )
) as session:
    await session.speak("This is my cloned voice!")

# Clone from description (Hume)
clone_result = await bud.voice.clone(VoiceCloneRequest(
    provider=VoiceCloneProvider.HUME,
    name="Warm Narrator",
    description="A warm, friendly narrator with a slight British accent",
    sample_text="Hello, welcome to our story today."
))
```

---

## DAG Configuration

Configure custom audio processing pipelines with DAG routing.

```python
from bud_foundry.types import DAGConfig, DAGDefinition, DAGNode, DAGEdge, DAGNodeType

# Define a custom voice bot pipeline
dag = DAGDefinition(
    id="voice-bot-v1",
    name="Voice Bot Pipeline",
    version="1.0",
    nodes=[
        DAGNode(id="input", type=DAGNodeType.AUDIO_INPUT),
        DAGNode(id="stt", type=DAGNodeType.STT_PROVIDER, config={"provider": "deepgram"}),
        DAGNode(id="llm", type=DAGNodeType.LLM, config={"provider": "openai", "model": "gpt-4"}),
        DAGNode(id="tts", type=DAGNodeType.TTS_PROVIDER, config={"provider": "elevenlabs"}),
        DAGNode(id="output", type=DAGNodeType.AUDIO_OUTPUT)
    ],
    edges=[
        DAGEdge(from_node="input", to_node="stt"),
        DAGEdge(from_node="stt", to_node="llm", condition="is_final == true"),
        DAGEdge(from_node="llm", to_node="tts"),
        DAGEdge(from_node="tts", to_node="output")
    ]
)

# Use DAG with WebSocket session
async with bud.talk.connect(
    dag_config=DAGConfig(
        definition=dag,
        enable_metrics=True,
        timeout_ms=30000
    )
) as session:
    async for event in session:
        print(event)

# Or use a pre-built template
async with bud.talk.connect(
    dag_config=DAGConfig(template="voice-assistant")
) as session:
    # ...
```

### Built-in DAG Templates

```python
from bud_foundry.types import get_builtin_template

# Available templates
templates = ["simple-stt", "simple-tts", "voice-assistant"]

# Get template definition
voice_assistant = get_builtin_template("voice-assistant")
```

---

## Audio Features

Configure turn detection, noise filtering, and VAD.

```python
from bud_foundry.types import (
    AudioFeatures, TurnDetectionConfig, NoiseFilterConfig,
    ExtendedVADConfig, VADModeType, create_audio_features
)

# Full configuration
features = AudioFeatures(
    turn_detection=TurnDetectionConfig(
        enabled=True,
        threshold=0.5,
        silence_ms=500,
        prefix_padding_ms=200,
        create_response_ms=300
    ),
    noise_filtering=NoiseFilterConfig(
        enabled=True,
        strength="medium"  # "low", "medium", "high"
    ),
    vad=ExtendedVADConfig(
        enabled=True,
        threshold=0.5,
        mode=VADModeType.NORMAL  # NORMAL, AGGRESSIVE, VERY_AGGRESSIVE
    )
)

# Or use helper function
features = create_audio_features(
    turn_detection={"enabled": True, "threshold": 0.6},
    noise_filtering={"enabled": True, "strength": "high"},
    vad={"enabled": True, "mode": "aggressive"}
)
```

---

## Recording Management

Track and download recordings.

```python
from bud_foundry.types import RecordingFilter, RecordingStatus, RecordingFormat

# List recordings
recordings = await bud.recordings.list(RecordingFilter(
    room_name="my-room",
    status=RecordingStatus.COMPLETED,
    format=RecordingFormat.WAV,
    limit=10
))

for recording in recordings.recordings:
    print(f"{recording.stream_id}: {recording.duration}s ({recording.size} bytes)")

# Download recording
audio_bytes = await bud.recordings.download(stream_id="abc123")
```

---

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
bud.voice    # Voice cloning operations
bud.recordings  # Recording management

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

---

## OpenAI Provider

Use OpenAI's Whisper for transcription and TTS-1/TTS-1-HD for speech synthesis:

```python
from bud_foundry import BudClient, STTConfig, TTSConfig

bud = BudClient(base_url="http://localhost:3001", api_key="your-api-key")

# OpenAI STT (Whisper)
async with bud.stt.connect(
    config=STTConfig(
        provider="openai",
        model="whisper-1",  # or "gpt-4o-transcribe", "gpt-4o-mini-transcribe"
        language="en"
    )
) as session:
    async for result in session.transcribe_stream(audio_generator()):
        print(f"Transcript: {result.text}")

# OpenAI TTS
async with bud.tts.connect(
    config=TTSConfig(
        provider="openai",
        model="tts-1-hd",  # or "tts-1", "gpt-4o-mini-tts"
        voice="nova"       # alloy, ash, ballad, coral, echo, fable, onyx, nova, sage, shimmer, verse
    )
) as session:
    await session.speak("Hello from OpenAI!")
```

### OpenAI Models

**STT Models:**
- `whisper-1` - Original Whisper model
- `gpt-4o-transcribe` - GPT-4o optimized transcription
- `gpt-4o-mini-transcribe` - Smaller, faster transcription

**TTS Models:**
- `tts-1` - Standard quality, low latency
- `tts-1-hd` - High-definition quality
- `gpt-4o-mini-tts` - GPT-4o mini TTS

**TTS Voices:**
alloy, ash, ballad, coral, echo, fable, onyx, nova, sage, shimmer, verse

---

## Hume EVI Prosody Scores

When using Hume EVI, you receive 48 emotion dimensions in real-time.

```python
from bud_foundry.types import ProsodyScores

# Emotion events include full prosody scores
def on_emotion(event):
    scores = ProsodyScores(**event.emotions)

    # Get top 3 emotions
    top_emotions = scores.top_emotions(3)
    print("Top emotions:", top_emotions)

    # Get dominant emotion
    dominant = scores.dominant_emotion()
    print(f"Dominant: {dominant[0]} ({dominant[1]:.2f})")

realtime.on("emotion", on_emotion)
```

### All 48 Prosody Dimensions

```
admiration, adoration, aesthetic_appreciation, amusement, anger,
anxiety, awe, awkwardness, boredom, calmness, concentration,
confusion, contemplation, contempt, contentment, craving, desire,
determination, disappointment, disgust, distress, doubt, ecstasy,
embarrassment, empathic_pain, enthusiasm, entrancement, envy,
excitement, fear, guilt, horror, interest, joy, love, nostalgia,
pain, pride, realization, relief, romance, sadness, satisfaction,
shame, surprise_negative, surprise_positive, sympathy, tiredness, triumph
```

---

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

---

## Error Handling

```python
from bud_foundry.errors import (
    BudError,
    ConnectionError,
    APIError,
    STTError,
    TTSError
)

try:
    async with bud.stt.connect(provider="deepgram") as session:
        async for result in session.transcribe_stream(audio_generator()):
            print(result.text)
except ConnectionError as e:
    print(f"Connection failed: {e}")
except STTError as e:
    print(f"STT error: {e}")
except BudError as e:
    print(f"General error: {e}")
```

---

## License

MIT
