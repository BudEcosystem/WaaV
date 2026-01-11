# @bud-foundry/sdk

TypeScript SDK for Bud Foundry AI Gateway - Speech-to-Text, Text-to-Speech, Realtime Audio-to-Audio, and Voice AI.

## Installation

```bash
npm install @bud-foundry/sdk
# or
yarn add @bud-foundry/sdk
# or
pnpm add @bud-foundry/sdk
```

## Quick Start

```typescript
import { BudClient } from '@bud-foundry/sdk';

const bud = new BudClient({
  baseUrl: 'http://localhost:3001',
  apiKey: 'your-api-key'
});

// STT (Speech-to-Text)
const stt = await bud.stt.connect({ provider: 'deepgram' });
stt.on('transcript', (result) => {
  console.log(result.is_final ? `Final: ${result.text}` : `Interim: ${result.text}`);
});
await stt.startListening();

// TTS (Text-to-Speech)
const tts = await bud.tts.connect({ provider: 'elevenlabs' });
await tts.speak('Hello from WaaV!');

// Bidirectional Voice
const talk = await bud.talk.connect({
  stt: { provider: 'deepgram' },
  tts: { provider: 'elevenlabs' }
});
await talk.startListening();
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
- **Browser & Node.js**: Works in both environments

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

```typescript
import { BudClient, STTConfig, TTSConfig, FeatureFlags } from '@bud-foundry/sdk';

const bud = new BudClient({
  baseUrl: 'https://api.bud.ai',
  apiKey: 'bud_xxx',
  timeout: 30000
});

// With feature flags
const stt = await bud.stt.connect({
  config: {
    provider: 'deepgram',
    language: 'en-US',
    model: 'nova-3'
  } as STTConfig,
  features: {
    vad: true,
    noiseCancellation: true,
    speakerDiarization: true
  } as FeatureFlags
});
```

---

## Speech-to-Text (STT)

```typescript
import { BudClient, STTResult } from '@bud-foundry/sdk';

const bud = new BudClient({ baseUrl: 'http://localhost:3001' });

// Connect to STT
const stt = await bud.stt.connect({
  provider: 'deepgram',
  model: 'nova-3',
  language: 'en-US'
});

// Handle events
stt.on('transcript', (result: STTResult) => {
  if (result.is_final) {
    console.log(`Final: ${result.text}`);
  } else {
    console.log(`Interim: ${result.text}`);
  }
});

stt.on('error', (error) => {
  console.error('STT error:', error);
});

// Start listening (requests microphone permission)
await stt.startListening();

// Or send audio manually
await stt.sendAudio(audioBuffer);

// Get metrics
const metrics = stt.getMetrics();
console.log(`TTFT p95: ${metrics.stt.ttft.p95}ms`);

// Stop and disconnect
await stt.stopListening();
await stt.disconnect();
```

---

## Text-to-Speech (TTS)

```typescript
import { BudClient, AudioEvent } from '@bud-foundry/sdk';

const bud = new BudClient({ baseUrl: 'http://localhost:3001' });

// Connect to TTS
const tts = await bud.tts.connect({
  provider: 'elevenlabs',
  voice: 'rachel',
  model: 'eleven_turbo_v2'
});

// Handle audio events
tts.on('audio', (event: AudioEvent) => {
  // Play audio chunk
  audioPlayer.play(event.audio);
});

tts.on('complete', () => {
  console.log('Speech complete');
});

// Speak text
await tts.speak('Hello, how can I help you?');

// Speak with flush (wait for completion)
await tts.speak('Final message', { flush: true });

// Disconnect
await tts.disconnect();
```

---

## Bidirectional Voice (Talk)

```typescript
import { BudClient, TranscriptEvent, AudioEvent } from '@bud-foundry/sdk';

const bud = new BudClient({ baseUrl: 'http://localhost:3001' });

// Connect to Talk
const talk = await bud.talk.connect({
  stt: { provider: 'deepgram' },
  tts: { provider: 'elevenlabs', voice: 'rachel' }
});

// Handle events
talk.on('transcript', (event: TranscriptEvent) => {
  console.log(`User said: ${event.text}`);

  // Generate response and speak
  const response = generateResponse(event.text);
  talk.speak(response);
});

talk.on('audio', (event: AudioEvent) => {
  audioPlayer.play(event.audio);
});

talk.on('speechStart', () => {
  console.log('User started speaking');
});

talk.on('speechEnd', () => {
  console.log('User stopped speaking');
});

// Start listening
await talk.startListening();

// Stop and disconnect
await talk.stopListening();
await talk.disconnect();
```

---

## Realtime Audio-to-Audio

Full-duplex bidirectional audio streaming with LLM integration.

### OpenAI Realtime

```typescript
import { BudRealtime, RealtimeConfig, RealtimeProvider } from '@bud-foundry/sdk';

const realtime = new BudRealtime({
  provider: RealtimeProvider.OpenAIRealtime,
  apiKey: 'your-openai-key',
  model: 'gpt-4o-realtime-preview',
  systemPrompt: 'You are a helpful voice assistant.',
  voice: 'alloy',  // alloy, ash, ballad, coral, echo, sage, shimmer, verse
  temperature: 0.8
});

// Register event handlers
realtime.on('transcript', (e) => console.log(`[${e.role}] ${e.text}`));
realtime.on('audio', (e) => playAudio(e.audio));
realtime.on('functionCall', handleFunctionCall);

// Connect and stream
await realtime.connect('wss://gateway.example.com/realtime');
await realtime.sendAudio(audioBuffer);

// Register tools for function calling
realtime.registerTool({
  name: 'get_weather',
  description: 'Get current weather for a location',
  parameters: {
    type: 'object',
    properties: {
      location: { type: 'string', description: 'City name' }
    },
    required: ['location']
  }
});

// Submit function result
await realtime.submitFunctionResult(callId, { temperature: 72, unit: 'F' });

// Disconnect
await realtime.disconnect();
```

### Hume EVI (Empathic Voice Interface)

```typescript
import { BudRealtime, RealtimeConfig, RealtimeProvider } from '@bud-foundry/sdk';

const realtime = new BudRealtime({
  provider: RealtimeProvider.HumeEVI,
  apiKey: 'your-hume-key',
  eviVersion: '3',  // EVI version (1, 2, 3, or 4-mini)
  voiceId: 'your-voice-id',
  systemPrompt: 'You are an empathetic assistant.',
  verboseTranscription: true
});

// Emotion events include 48 emotion dimensions
realtime.on('emotion', (e) => {
  console.log(`Dominant: ${e.dominant} (${e.emotions[e.dominant].toFixed(2)})`);
});

realtime.on('transcript', (e) => console.log(e.text));

await realtime.connect('wss://gateway.example.com/realtime');
```

### Realtime Events

| Event | Type | Description |
|-------|------|-------------|
| `audio` | `RealtimeAudioChunk` | Audio output chunk from assistant |
| `transcript` | `RealtimeTranscript` | Transcript (user or assistant) |
| `functionCall` | `FunctionCallEvent` | Tool/function call from LLM |
| `emotion` | `EmotionEvent` | Emotion scores (Hume EVI only) |
| `connected` | - | Connection established |
| `disconnected` | - | Connection closed |
| `stateChange` | `StateChangeEvent` | State transition |
| `error` | `Error` | Error occurred |

---

## Emotion Control

Unified emotion system with 22 emotions and 15 delivery styles.

### Using Emotions

```typescript
import {
  BudClient,
  Emotion,
  DeliveryStyle,
  EmotionIntensityLevel
} from '@bud-foundry/sdk';

const bud = new BudClient({ baseUrl: 'http://localhost:3001' });

// With TTS configuration
const tts = await bud.tts.connect({
  provider: 'elevenlabs',
  voice: 'rachel',
  emotion: Emotion.Happy,
  emotionIntensity: EmotionIntensityLevel.High,
  deliveryStyle: DeliveryStyle.Cheerful
});

await tts.speak('Great to meet you!');

// With Hume (natural language emotion)
const humeTts = await bud.tts.connect({
  provider: 'hume',
  voice: 'Kora',
  emotion: Emotion.Excited,
  emotionIntensity: 0.8,
  deliveryStyle: DeliveryStyle.Enthusiastic,
  actingInstructions: 'whispered with excitement'  // Hume-specific
});

await humeTts.speak('I have amazing news!');
```

### Supported Emotions (22)

```typescript
enum Emotion {
  Neutral = 'neutral',
  Happy = 'happy',
  Sad = 'sad',
  Angry = 'angry',
  Fearful = 'fearful',
  Surprised = 'surprised',
  Disgusted = 'disgusted',
  Excited = 'excited',
  Calm = 'calm',
  Anxious = 'anxious',
  Confident = 'confident',
  Confused = 'confused',
  Empathetic = 'empathetic',
  Sarcastic = 'sarcastic',
  Hopeful = 'hopeful',
  Disappointed = 'disappointed',
  Curious = 'curious',
  Grateful = 'grateful',
  Proud = 'proud',
  Embarrassed = 'embarrassed',
  Content = 'content',
  Bored = 'bored'
}
```

### Delivery Styles (15)

```typescript
enum DeliveryStyle {
  Normal = 'normal',
  Whispered = 'whispered',
  Shouted = 'shouted',
  Rushed = 'rushed',
  Measured = 'measured',
  Monotone = 'monotone',
  Expressive = 'expressive',
  Professional = 'professional',
  Casual = 'casual',
  Storytelling = 'storytelling',
  Soft = 'soft',
  Loud = 'loud',
  Cheerful = 'cheerful',
  Serious = 'serious',
  Formal = 'formal'
}
```

---

## Voice Cloning

Clone voices from audio samples or descriptions.

```typescript
import { BudClient, VoiceCloneRequest, VoiceCloneProvider } from '@bud-foundry/sdk';

const bud = new BudClient({ baseUrl: 'http://localhost:3001' });

// Clone from audio samples (ElevenLabs)
const audioFile = await fetch('voice_sample.mp3');
const audioBase64 = btoa(String.fromCharCode(...new Uint8Array(await audioFile.arrayBuffer())));

const cloneResult = await bud.voice.clone({
  provider: VoiceCloneProvider.ElevenLabs,
  name: 'My Custom Voice',
  description: 'Professional male voice',
  audioSamples: [audioBase64],
  removeBackgroundNoise: true,
  labels: { gender: 'male', accent: 'american' }
});

console.log(`Cloned voice ID: ${cloneResult.voiceId}`);

// Use the cloned voice
const tts = await bud.tts.connect({
  provider: 'elevenlabs',
  voiceId: cloneResult.voiceId
});

await tts.speak('This is my cloned voice!');

// Clone from description (Hume)
const humeClone = await bud.voice.clone({
  provider: VoiceCloneProvider.Hume,
  name: 'Warm Narrator',
  description: 'A warm, friendly narrator with a slight British accent',
  sampleText: 'Hello, welcome to our story today.'
});
```

---

## DAG Configuration

Configure custom audio processing pipelines with DAG routing.

```typescript
import {
  BudClient,
  DAGConfig,
  DAGDefinition,
  DAGNode,
  DAGEdge,
  DAGNodeType,
  validateDAGDefinition,
  getBuiltinTemplate
} from '@bud-foundry/sdk';

const bud = new BudClient({ baseUrl: 'http://localhost:3001' });

// Define a custom voice bot pipeline
const dag: DAGDefinition = {
  id: 'voice-bot-v1',
  name: 'Voice Bot Pipeline',
  version: '1.0',
  nodes: [
    { id: 'input', type: DAGNodeType.AudioInput },
    { id: 'stt', type: DAGNodeType.STTProvider, config: { provider: 'deepgram' } },
    { id: 'llm', type: DAGNodeType.LLM, config: { provider: 'openai', model: 'gpt-4' } },
    { id: 'tts', type: DAGNodeType.TTSProvider, config: { provider: 'elevenlabs' } },
    { id: 'output', type: DAGNodeType.AudioOutput }
  ],
  edges: [
    { from: 'input', to: 'stt' },
    { from: 'stt', to: 'llm', condition: 'is_final == true' },
    { from: 'llm', to: 'tts' },
    { from: 'tts', to: 'output' }
  ]
};

// Validate DAG before use
const validation = validateDAGDefinition(dag);
if (!validation.valid) {
  console.error('DAG validation errors:', validation.errors);
}

// Use DAG with WebSocket session
const talk = await bud.talk.connect({
  dagConfig: {
    definition: dag,
    enableMetrics: true,
    timeoutMs: 30000
  }
});

// Or use a pre-built template
const templateTalk = await bud.talk.connect({
  dagConfig: { template: 'voice-assistant' }
});

// Available templates
const templates = ['simple-stt', 'simple-tts', 'voice-assistant'];
const voiceAssistant = getBuiltinTemplate('voice-assistant');
```

---

## Audio Features

Configure turn detection, noise filtering, and VAD.

```typescript
import {
  AudioFeatures,
  TurnDetectionConfig,
  NoiseFilterConfig,
  ExtendedVADConfig,
  VADModeType,
  createAudioFeatures
} from '@bud-foundry/sdk';

// Full configuration
const features: AudioFeatures = {
  turnDetection: {
    enabled: true,
    threshold: 0.5,
    silenceMs: 500,
    prefixPaddingMs: 200,
    createResponseMs: 300
  },
  noiseFiltering: {
    enabled: true,
    strength: 'medium'  // 'low' | 'medium' | 'high'
  },
  vad: {
    enabled: true,
    threshold: 0.5,
    mode: VADModeType.Normal  // Normal | Aggressive | VeryAggressive
  }
};

// Or use helper function
const autoFeatures = createAudioFeatures({
  turnDetection: { enabled: true, threshold: 0.6 },
  noiseFiltering: { enabled: true, strength: 'high' },
  vad: { enabled: true, mode: 'aggressive' }
});
```

---

## Metrics Collection

Track performance metrics for latency optimization.

```typescript
import { BudClient, MetricsCollector, PercentileStats } from '@bud-foundry/sdk';

const bud = new BudClient({ baseUrl: 'http://localhost:3001' });

const stt = await bud.stt.connect({ provider: 'deepgram' });

// Get metrics after processing
const metrics = stt.getMetrics();

console.log('STT Metrics:');
console.log(`  TTFT p50: ${metrics.stt.ttft.p50}ms`);
console.log(`  TTFT p95: ${metrics.stt.ttft.p95}ms`);
console.log(`  TTFT p99: ${metrics.stt.ttft.p99}ms`);
console.log(`  Total transcriptions: ${metrics.stt.transcriptionCount}`);

console.log('TTS Metrics:');
console.log(`  TTFB p50: ${metrics.tts.ttfb.p50}ms`);
console.log(`  TTFB p95: ${metrics.tts.ttfb.p95}ms`);
console.log(`  Total speak calls: ${metrics.tts.speakCount}`);

// Reset metrics
stt.resetMetrics();
```

---

## Audio Utilities

Built-in audio recording and playback for browser environments.

### Audio Recorder

```typescript
import { AudioRecorder, AudioFormat } from '@bud-foundry/sdk';

const recorder = new AudioRecorder({
  sampleRate: 16000,
  channels: 1,
  format: AudioFormat.PCM16
});

// Start recording
await recorder.start();

// Get audio data
recorder.on('data', (audioBuffer: ArrayBuffer) => {
  // Send to STT
  stt.sendAudio(audioBuffer);
});

// Stop recording
recorder.stop();
```

### Audio Player

```typescript
import { AudioPlayer, AudioFormat } from '@bud-foundry/sdk';

const player = new AudioPlayer({
  sampleRate: 24000,
  channels: 1,
  format: AudioFormat.PCM16
});

// Play audio chunks
player.play(audioBuffer);

// Queue audio
player.queue(audioBuffer1);
player.queue(audioBuffer2);

// Stop playback
player.stop();

// Clear queue
player.clear();
```

### Voice Activity Detection (VAD)

```typescript
import { VAD, VADEvent } from '@bud-foundry/sdk';

const vad = new VAD({
  threshold: 0.5,
  silenceMs: 500
});

vad.on('speechStart', () => {
  console.log('Speech started');
});

vad.on('speechEnd', () => {
  console.log('Speech ended');
});

// Process audio
vad.process(audioBuffer);
```

---

## Recording Management

Track and download recordings.

```typescript
import {
  BudClient,
  RecordingFilter,
  RecordingStatus,
  RecordingFormat
} from '@bud-foundry/sdk';

const bud = new BudClient({ baseUrl: 'http://localhost:3001' });

// List recordings
const recordings = await bud.recordings.list({
  roomName: 'my-room',
  status: RecordingStatus.Completed,
  format: RecordingFormat.WAV,
  limit: 10
});

for (const recording of recordings.recordings) {
  console.log(`${recording.streamId}: ${recording.duration}s (${recording.size} bytes)`);
}

// Download recording
const audioBlob = await bud.recordings.download('abc123');
```

---

## LiveKit Integration

```typescript
import { BudClient, LiveKitConfig } from '@bud-foundry/sdk';

const bud = new BudClient({ baseUrl: 'http://localhost:3001' });

// Generate LiveKit token
const { token, roomName, identity, livekitUrl } = await bud.livekit.getToken({
  roomName: 'my-room',
  identity: 'user-123',
  name: 'John Doe',
  ttl: 3600,
  metadata: JSON.stringify({ role: 'participant' })
});

// Connect with LiveKit configuration
const talk = await bud.talk.connect({
  stt: { provider: 'deepgram' },
  tts: { provider: 'elevenlabs' },
  livekit: {
    roomName: 'my-room',
    identity: 'user-123'
  }
});
```

---

## Error Handling

```typescript
import {
  BudClient,
  BudError,
  ConnectionError,
  APIError,
  STTError,
  TTSError
} from '@bud-foundry/sdk';

const bud = new BudClient({ baseUrl: 'http://localhost:3001' });

try {
  const stt = await bud.stt.connect({ provider: 'deepgram' });
  await stt.startListening();
} catch (error) {
  if (error instanceof ConnectionError) {
    console.error(`Connection failed: ${error.message}`);
  } else if (error instanceof STTError) {
    console.error(`STT error: ${error.message}`);
  } else if (error instanceof APIError) {
    console.error(`API error: ${error.statusCode} - ${error.message}`);
  } else if (error instanceof BudError) {
    console.error(`General error: ${error.message}`);
  }
}
```

---

## TypeScript Types

The SDK exports comprehensive TypeScript types for full type safety.

```typescript
// Core types
import type {
  // Configuration
  STTConfig,
  TTSConfig,
  FeatureFlags,
  LiveKitConfig,
  DAGConfig,
  DAGDefinition,
  DAGNode,
  DAGEdge,

  // Results
  STTResult,
  TranscriptEvent,
  AudioEvent,
  WordInfo,
  Voice,

  // Realtime
  RealtimeConfig,
  RealtimeTranscript,
  RealtimeAudioChunk,
  RealtimeSpeechEvent,

  // Emotions
  Emotion,
  DeliveryStyle,
  EmotionIntensityLevel,
  EmotionConfig,
  ProsodyScores,

  // Audio Features
  AudioFeatures,
  TurnDetectionConfig,
  NoiseFilterConfig,
  VADConfig,
  ExtendedVADConfig,

  // Voice Cloning
  VoiceCloneRequest,
  VoiceCloneResponse,
  VoiceCloneProvider,
  VoiceCloneStatus,

  // Recordings
  RecordingInfo,
  RecordingFilter,
  RecordingList,
  RecordingStatus,
  RecordingFormat,

  // Metrics
  MetricsSummary,
  STTMetrics,
  TTSMetrics,
  PercentileStats,

  // Providers
  STTProvider,
  TTSProvider,
  RealtimeProvider
} from '@bud-foundry/sdk';
```

---

## Browser Support

The SDK works in modern browsers with WebSocket and Web Audio API support:

- Chrome 66+
- Firefox 60+
- Safari 12+
- Edge 79+

### Bundler Configuration

For Webpack, Vite, or other bundlers, no special configuration is needed. The SDK uses standard ES modules.

```typescript
// ESM import
import { BudClient } from '@bud-foundry/sdk';

// Dynamic import
const { BudClient } = await import('@bud-foundry/sdk');
```

---

## Node.js Support

The SDK works in Node.js 18+ with the `ws` package for WebSocket support.

```bash
npm install @bud-foundry/sdk ws
```

```typescript
import { BudClient } from '@bud-foundry/sdk';
import WebSocket from 'ws';

// Node.js requires WebSocket polyfill
globalThis.WebSocket = WebSocket as any;

const bud = new BudClient({ baseUrl: 'http://localhost:3001' });
```

---

## License

MIT
