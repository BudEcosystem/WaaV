# Hume AI Integration

Hume AI provides advanced voice AI with emotional intelligence. WaaV Gateway integrates:
- **Octave TTS**: Text-to-speech with natural language emotion control
- **EVI** (Empathic Voice Interface): Full-duplex audio-to-audio with 48-dimension emotion analysis
- **Voice Cloning**: Create custom voices from audio samples or descriptions

## Configuration

### Environment Variables

```bash
export HUME_API_KEY="your-hume-api-key"
```

### YAML Configuration

```yaml
providers:
  hume_api_key: "your-hume-api-key"  # ENV: HUME_API_KEY

# TTS Configuration
tts:
  provider: hume
```

## Hume TTS (Octave)

Octave is Hume's text-to-speech model with natural language emotion control. Unlike SSML-based providers, Hume uses plain English descriptions for emotions.

### Basic Usage

```rust
use waav_gateway::core::tts::{create_tts_provider, TTSConfig};

let config = TTSConfig {
    api_key: "your-api-key".to_string(),
    voice: Some("Kora".to_string()),
    ..Default::default()
};

let tts = create_tts_provider("hume", config)?;
tts.connect().await?;
tts.speak("Hello, world!").await?;
```

### Emotion Control

Hume uses natural language for emotion control via the `acting_instructions` field (max 100 characters):

```rust
let config = HumeTTSConfig {
    acting_instructions: Some("happy, energetic".to_string()),
    ..Default::default()
};
```

Examples:
- `"whispered fearfully"` - Fear with whispered delivery
- `"sarcastic, dry"` - Sarcastic tone
- `"warm, inviting"` - Friendly and welcoming
- `"rushed, urgent"` - Fast-paced, urgent delivery
- `"calm, measured"` - Slow, deliberate speech

### Configuration Options

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `voice` | String | Kora | Voice name from Hume library |
| `voice_id` | String | - | Voice UUID (overrides voice name) |
| `voice_description` | String | - | Natural language voice design prompt |
| `acting_instructions` | String | - | Emotion/style instructions (max 100 chars) |
| `speed` | f32 | 1.0 | Speaking rate (0.5 to 2.0) |
| `trailing_silence` | f32 | 0.0 | Silence after speech in seconds |
| `instant_mode` | bool | true | Enable low-latency mode |
| `sample_rate` | u32 | 24000 | Audio sample rate (8000-48000) |

### Available Voices

| Voice | Gender | Style |
|-------|--------|-------|
| Kora | Female | Warm, conversational |
| Dacher | Male | Clear, professional |
| Aura | Female | Calm, measured |
| Finnegan | Male | Energetic, enthusiastic |

### API Details

- **Streaming endpoint**: `POST https://api.hume.ai/v0/tts/stream/file`
- **Synchronous endpoint**: `POST https://api.hume.ai/v0/tts/file`
- **Authentication**: `X-HUME-API-KEY` header
- **Audio format**: Raw PCM audio bytes (streaming)

## Hume EVI (Audio-to-Audio)

EVI (Empathic Voice Interface) provides full-duplex audio streaming with real-time emotion analysis of user speech and empathic response generation.

### Basic Usage

```rust
use waav_gateway::core::realtime::{create_realtime_provider, RealtimeConfig};

let config = RealtimeConfig {
    api_key: "your-api-key".to_string(),
    provider: "hume".to_string(),
    ..Default::default()
};

let realtime = create_realtime_provider("hume", config)?;
realtime.connect().await?;

// Send audio
realtime.send_audio(audio_bytes).await?;

// Receive transcripts, emotions, and response audio via callbacks
```

### Emotion Analysis (Prosody Scores)

EVI analyzes 48 emotion dimensions in user speech:

```rust
pub struct ProsodyScores {
    pub admiration: f32,
    pub adoration: f32,
    pub aesthetic_appreciation: f32,
    pub amusement: f32,
    pub anger: f32,
    pub anxiety: f32,
    pub awe: f32,
    pub awkwardness: f32,
    pub boredom: f32,
    pub calmness: f32,
    pub concentration: f32,
    pub confusion: f32,
    pub contemplation: f32,
    pub contempt: f32,
    pub contentment: f32,
    pub craving: f32,
    pub desire: f32,
    pub determination: f32,
    pub disappointment: f32,
    pub disgust: f32,
    pub distress: f32,
    pub doubt: f32,
    pub ecstasy: f32,
    pub embarrassment: f32,
    pub empathic_pain: f32,
    pub enthusiasm: f32,
    pub entrancement: f32,
    pub envy: f32,
    pub excitement: f32,
    pub fear: f32,
    pub guilt: f32,
    pub horror: f32,
    pub interest: f32,
    pub joy: f32,
    pub love: f32,
    pub nostalgia: f32,
    pub pain: f32,
    pub pride: f32,
    pub realization: f32,
    pub relief: f32,
    pub romance: f32,
    pub sadness: f32,
    pub satisfaction: f32,
    pub shame: f32,
    pub surprise_negative: f32,
    pub surprise_positive: f32,
    pub sympathy: f32,
    pub tiredness: f32,
    pub triumph: f32,
}
```

### EVI Configuration

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `config_id` | String | - | Pre-configured EVI config from Hume dashboard |
| `resumed_chat_group_id` | String | - | Resume a previous conversation |
| `evi_version` | String | "3" | EVI version (1, 2, 3, or 4-mini) |
| `voice_id` | String | - | Voice UUID to use |
| `verbose_transcription` | bool | false | Enable detailed transcription info |
| `system_prompt` | String | - | Override system prompt |

### WebSocket API

- **Endpoint**: `wss://api.hume.ai/v0/evi/chat`
- **Authentication**: Query parameter `apiKey=...`

#### Outgoing Messages

```json
// Audio input
{
  "type": "audio_input",
  "data": "<base64-encoded-audio>"
}

// Session settings
{
  "type": "session_settings",
  "system_prompt": "You are a helpful assistant"
}

// Pause/resume
{
  "type": "pause_assistant_audio"
}
```

#### Incoming Messages

```json
// User transcript with emotions
{
  "type": "user_message",
  "message": { "content": "Hello" },
  "from_text": false,
  "models": {
    "prosody": {
      "scores": { "joy": 0.8, "interest": 0.6, ... }
    }
  }
}

// Assistant response
{
  "type": "assistant_message",
  "message": { "content": "Hi there!" },
  "from_text": true
}

// Audio output
{
  "type": "audio_output",
  "data": "<base64-encoded-audio>"
}

// Assistant finished speaking
{
  "type": "assistant_end"
}
```

## Voice Cloning

Create custom voices from audio samples or natural language descriptions.

### REST Endpoint

`POST /voices/clone`

### Request Body

```json
{
  "provider": "hume",
  "name": "my-custom-voice",
  "description": "A warm, friendly voice with a slight British accent",
  "audio_samples": ["<base64-audio>"],
  "sample_text": "This is sample text for voice generation."
}
```

For Hume, voice cloning uses the voice design API:
1. Generate speech with the description
2. Save the resulting voice with a name

### Response

```json
{
  "voice_id": "uuid-of-cloned-voice",
  "name": "my-custom-voice",
  "provider": "hume",
  "status": "ready",
  "created_at": "2026-01-06T12:00:00Z"
}
```

### Using Cloned Voices

```rust
let config = HumeTTSConfig {
    voice_id: Some("uuid-of-cloned-voice".to_string()),
    ..Default::default()
};
```

## Unified Emotion System

WaaV Gateway provides a unified emotion system that works across providers. For Hume, emotions are mapped to natural language descriptions.

### Supported Emotions

| Emotion | Hume Mapping |
|---------|--------------|
| `happy` | "happy, cheerful" |
| `sad` | "melancholic, sad" |
| `angry` | "frustrated, angry" |
| `fearful` | "frightened, fearful" |
| `surprised` | "surprised, astonished" |
| `excited` | "excited, energetic" |
| `calm` | "calm, measured" |
| `anxious` | "anxious, worried" |
| `confident` | "confident, assured" |
| `sarcastic` | "sarcastic, dry" |

### Delivery Styles

| Style | Hume Mapping |
|-------|--------------|
| `whispered` | "whispered" |
| `shouted` | "shouted, loud" |
| `rushed` | "rushed, urgent" |
| `measured` | "deliberate, measured" |
| `soft` | "soft, gentle" |

### Using Unified Emotions

```json
{
  "type": "speak",
  "text": "I'm so happy to help you!",
  "tts": {
    "provider": "hume",
    "emotion": "happy",
    "emotionIntensity": 0.8,
    "deliveryStyle": "cheerful"
  }
}
```

The gateway automatically converts this to Hume's `acting_instructions`:
```
"very happy, cheerful"
```

## Pricing

| Product | Pricing |
|---------|---------|
| **Octave TTS** | Free tier (10K chars), Starter $3/mo (30K chars), Creator $10/mo (100K chars) |
| **EVI** | Starting ~$0.02/min (volume), $0.072/min standard |

## Error Handling

### Common Errors

| Error | Cause | Solution |
|-------|-------|----------|
| `401 Unauthorized` | Invalid API key | Verify HUME_API_KEY is set correctly |
| `429 Too Many Requests` | Rate limit exceeded | Implement backoff, upgrade tier |
| `400 Bad Request` | Invalid acting_instructions | Keep under 100 characters |
| `Voice not found` | Invalid voice name/ID | Check voice exists in Hume dashboard |

### Reconnection

EVI supports automatic reconnection with session resumption:

```rust
let config = HumeEVIConfig {
    resumed_chat_group_id: Some("previous-chat-id".to_string()),
    ..Default::default()
};
```

## Best Practices

1. **Emotion Instructions**: Keep `acting_instructions` concise (max 100 chars). Use comma-separated descriptors.

2. **Voice Design**: For custom voices, provide detailed natural language descriptions including accent, tone, pace, and personality.

3. **Instant Mode**: Use `instant_mode: true` (default) for real-time applications. Disable for higher quality non-real-time synthesis.

4. **EVI Sessions**: Reuse `chat_group_id` to maintain conversation context across reconnections.

5. **Emotion Analysis**: Use `getTopEmotions()` helper to extract the dominant emotions from prosody scores.

## SDK Examples

### TypeScript

```typescript
import { WaavClient, TTSConfig, HumeEVIConfig, ProsodyScores, getTopEmotions } from '@bud-foundry/waav-client';

// TTS with emotion
const ttsConfig: TTSConfig = {
  provider: 'hume',
  voice: 'Kora',
  emotion: 'happy',
  emotionIntensity: 'high',
  deliveryStyle: 'cheerful',
};

// EVI realtime
const eviConfig: HumeEVIConfig = {
  eviVersion: '3',
  systemPrompt: 'You are a helpful assistant.',
};

// Process emotion scores
function handleEmotions(scores: ProsodyScores) {
  const top = getTopEmotions(scores, 3);
  console.log('Top emotions:', top);
}
```

### Python

```python
from bud_foundry.types import TTSConfig, HumeEVIConfig, ProsodyScores, Emotion, DeliveryStyle

# TTS with emotion
tts_config = TTSConfig(
    provider="hume",
    voice="Kora",
    emotion=Emotion.HAPPY,
    emotion_intensity=0.8,
    delivery_style=DeliveryStyle.CHEERFUL,
)

# EVI realtime
evi_config = HumeEVIConfig(
    evi_version="3",
    system_prompt="You are a helpful assistant.",
)

# Process emotion scores
def handle_emotions(scores: ProsodyScores):
    top = scores.top_emotions(3)
    print(f"Top emotions: {top}")
    dominant = scores.dominant_emotion()
    print(f"Dominant: {dominant}")
```

## Related Documentation

- [Hume AI Documentation](https://dev.hume.ai)
- [OpenAI Realtime](./openai-realtime.md) - Similar audio-to-audio architecture
- [Voice Cloning API](./api-reference.md#voice-cloning)
- [Unified Emotion System](./emotion-system.md)
