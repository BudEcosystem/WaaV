# Bud Foundry Client SDK & Testing Dashboard Implementation Plan

## Overview

Create a comprehensive client ecosystem for the Bud Foundry AI gateway:
1. **TypeScript SDK** (`@bud-foundry/sdk`) - Full-featured SDK for developers
2. **Python SDK** (`bud-foundry`) - Async-first Python SDK with sync support
3. **Embeddable Widget** (`bud-widget.js`) - Single-script voice widget for any website
4. **Testing Dashboard** - Complete UI with performance metrics & SLO monitoring

**Location:** `/home/bud/Desktop/bud_waav/WaaV/clients_sdk/`

---

## CLAUDE.md Compliance

This plan adheres to the requirements in `/home/bud/.claude/CLAUDE.md`:

### Architecture Compliance
- **All SDKs are CLIENT libraries** that communicate WITH the Sayna gateway (not bypassing it)
- **REST + WebSocket only** - SDKs connect to `POST /speak`, `GET /voices`, `POST /livekit/token`, `/ws`, etc.
- **No Python HTTP servers** - Python SDK uses `httpx`/`websockets` as a CLIENT, not a server
- **Gateway is the only entry point** - All audio inference goes through Sayna gateway

```
Client App
    │
    ▼
┌─────────────────────────────────────┐
│  @bud-foundry/sdk (TypeScript)      │
│  bud-foundry (Python)               │  ← CLIENT SDKs
│  bud-widget.js                      │
└─────────────────────────────────────┘
    │ REST / WebSocket
    ▼
┌─────────────────────────────────────┐
│     Rust Gateway (Sayna)            │  ← ONLY entry point
│  - /ws (WebSocket real-time)        │
│  - /speak, /voices, /livekit/*      │
└─────────────────────────────────────┘
```

### Implementation Requirements
1. **No stub code** - Every function will be production-ready with full implementation
2. **No duplicate code** - Verified no existing SDK code exists in repository
3. **Integrated with core system** - SDKs map directly to Sayna's documented API
4. **Type contracts from source** - Types derived from `sayna/src/handlers/ws/messages.rs` and `config.rs`

### Pre-Implementation Verification (Completed)
- ✅ Searched `**/clients_sdk/**/*` - No existing SDK code found
- ✅ Searched `**/sdk/**/*` - Only WebRTC build artifacts (not client SDKs)
- ✅ Searched `**/*widget*` - Only system library headers (not our widget)
- ✅ Reviewed Sayna WebSocket protocol in `messages.rs` and `config.rs`
- ✅ Reviewed Sayna REST endpoints in handlers

---

## Research-Backed Best Practices

Sources: [Pragmatic Engineer - Building Great SDKs](https://newsletter.pragmaticengineer.com/p/building-great-sdks), [Azure SDK TypeScript Guidelines](https://azure.github.io/azure-sdk/typescript_design.html), [Twilio Voice AI Latency Guide](https://www.twilio.com/en-us/blog/developers/best-practices/guide-core-latency-ai-voice-agents), [HTTPX Async Support](https://www.python-httpx.org/async/)

### SDK Design Principles
- **Language-idiomatic code**: Python uses snake_case, TypeScript uses camelCase
- **Zero/minimal dependencies**: Use native fetch/WebSocket, httpx/websockets
- **Type-safe**: Full TypeScript strict mode, Python type hints
- **Dual sync/async**: Python SDK supports both patterns
- **Backward compatible**: Avoid breaking changes for LLM training data compatibility

### Latency Optimization (Voice AI)
- **TTFT is critical**: LLM Time-to-First-Token accounts for 90%+ of voice loop latency
- **Streaming at every stage**: STT, LLM, TTS all must stream
- **Deepgram STT**: Effectively 0ms additional latency with streaming
- **Target benchmarks**: Sub-1-second voice loop (STT + LLM + TTS)

### WebSocket Reconnection
- **Exponential backoff with jitter**: Prevent thundering herd
- **Defaults**: Initial 1000ms, max 30000ms, decay 1.5x
- **Buffer on disconnect**: Queue messages during reconnection

---

## Directory Structure

```
bud_waav/WaaV/clients_sdk/
├── typescript/                   # TypeScript SDK (@bud-foundry/sdk)
│   ├── src/
│   │   ├── index.ts              # Main exports
│   │   ├── bud.ts                # BudClient main class
│   │   ├── pipelines/            # Modality-specific clients
│   │   │   ├── stt.ts            # BudSTT class
│   │   │   ├── tts.ts            # BudTTS class
│   │   │   ├── talk.ts           # BudTalk (bidirectional voice)
│   │   │   ├── transcribe.ts     # BudTranscribe (batch/file)
│   │   │   └── index.ts          # Pipeline exports
│   │   ├── rest/
│   │   │   ├── index.ts          # RestClient
│   │   │   ├── livekit.ts        # LiveKit operations
│   │   │   └── sip.ts            # SIP operations
│   │   ├── ws/
│   │   │   ├── index.ts          # WebSocket session
│   │   │   ├── messages.ts       # Message types
│   │   │   └── reconnect.ts      # Reconnection logic
│   │   ├── audio/
│   │   │   ├── processor.ts      # Audio conversion
│   │   │   ├── player.ts         # PCM playback
│   │   │   └── vad.ts            # Voice Activity Detection
│   │   ├── metrics/
│   │   │   ├── collector.ts      # Performance metrics
│   │   │   ├── slo.ts            # SLO tracking
│   │   │   └── types.ts          # Metric types
│   │   ├── types/                # All TypeScript types
│   │   ├── errors/               # Error classes
│   │   └── utils/                # Utilities
│   ├── package.json
│   ├── tsconfig.json
│   └── README.md
│
├── python/                       # Python SDK (bud-foundry)
│   ├── bud_foundry/
│   │   ├── __init__.py           # Main exports
│   │   ├── client.py             # BudClient
│   │   ├── pipelines/
│   │   │   ├── __init__.py
│   │   │   ├── stt.py            # BudSTT
│   │   │   ├── tts.py            # BudTTS
│   │   │   ├── talk.py           # BudTalk
│   │   │   └── transcribe.py     # BudTranscribe
│   │   ├── rest/
│   │   │   ├── __init__.py
│   │   │   ├── client.py         # Async REST client (httpx)
│   │   │   ├── livekit.py
│   │   │   └── sip.py
│   │   ├── ws/
│   │   │   ├── __init__.py
│   │   │   ├── session.py        # WebSocket session (websockets)
│   │   │   └── messages.py
│   │   ├── audio/
│   │   │   ├── __init__.py
│   │   │   ├── processor.py      # PCM conversion
│   │   │   └── vad.py            # VAD wrapper
│   │   ├── metrics/
│   │   │   ├── __init__.py
│   │   │   ├── collector.py      # Performance metrics
│   │   │   └── slo.py            # SLO tracking
│   │   ├── types.py              # Type definitions
│   │   └── errors.py             # Exception classes
│   ├── pyproject.toml
│   ├── tests/
│   └── README.md
│
├── widget/                       # Embeddable Widget (bud-widget.js)
│   ├── src/
│   │   ├── index.ts
│   │   ├── widget.ts             # BudWidget Web Component
│   │   ├── config.ts
│   │   ├── state.ts
│   │   ├── websocket.ts
│   │   ├── audio/
│   │   │   ├── recorder.ts
│   │   │   ├── player.ts
│   │   │   └── vad.ts
│   │   ├── ui/
│   │   │   ├── styles.css
│   │   │   ├── icons.ts
│   │   │   └── components/
│   │   └── types.ts
│   ├── dist/
│   │   └── bud-widget.js         # <50KB gzipped
│   └── README.md
│
└── dashboard/                    # Testing Dashboard
    ├── index.html
    ├── styles.css
    ├── js/
    │   ├── app.js
    │   ├── state.js
    │   ├── websocket.js
    │   ├── metrics.js            # Performance metrics display
    │   ├── logger.js
    │   ├── audio.js
    │   └── components/
    │       ├── connection.js
    │       ├── stt.js
    │       ├── tts.js
    │       ├── livekit.js
    │       ├── sip.js
    │       ├── api-explorer.js
    │       ├── ws-debug.js
    │       ├── audio-tools.js
    │       └── metrics-panel.js  # SLO & performance dashboard
    └── README.md
```

---

## Performance Metrics & SLO Tracking

### Metrics Collected (OpenTelemetry Compatible)

| Metric | Description | Target SLO |
|--------|-------------|------------|
| `bud.stt.ttft_ms` | Time to First Token (STT) | < 200ms p95 |
| `bud.tts.ttfb_ms` | Time to First Byte (TTS) | < 150ms p95 |
| `bud.e2e.latency_ms` | End-to-end voice loop | < 1000ms p95 |
| `bud.ws.connect_ms` | WebSocket connection time | < 100ms p95 |
| `bud.ws.reconnects` | Reconnection count | < 1/min |
| `bud.audio.buffer_underruns` | Audio playback gaps | 0 |
| `bud.throughput.chars_per_sec` | TTS characters/second | > 100 |
| `bud.memory.heap_mb` | SDK memory usage | < 50MB |

### Metrics API

```typescript
// TypeScript
const metrics = session.getMetrics();
console.log(metrics.stt.ttft);      // { p50: 120, p95: 180, p99: 220, last: 165 }
console.log(metrics.tts.ttfb);      // { p50: 80, p95: 140, p99: 200, last: 95 }
console.log(metrics.e2e.latency);   // { p50: 450, p95: 800, p99: 1100 }

// Subscribe to real-time metrics
session.on('metrics', (m) => dashboard.updateLatency(m.e2e.latency));
```

```python
# Python
metrics = await session.get_metrics()
print(f"STT TTFT p95: {metrics.stt.ttft.p95}ms")
print(f"E2E latency p95: {metrics.e2e.latency.p95}ms")
```

---

## Feature Flags (Audio Processing Options)

### Available Feature Flags

| Flag | Description | Default | Provider Support |
|------|-------------|---------|------------------|
| `vad` | Voice Activity Detection | `true` | All |
| `noise_cancellation` | Noise suppression (DeepFilterNet) | `false` | Gateway |
| `speaker_diarization` | Multi-speaker identification | `false` | Deepgram |
| `interim_results` | Partial STT results | `true` | All |
| `punctuation` | Auto-punctuation | `true` | All |
| `profanity_filter` | Filter profane words | `false` | Deepgram, Azure |
| `smart_format` | Smart text formatting | `true` | Deepgram |
| `word_timestamps` | Per-word timing | `false` | All |
| `echo_cancellation` | Browser echo cancellation | `true` | Browser |
| `filler_words` | Include um, uh, etc. | `false` | Deepgram |

### Feature Flag Usage

```typescript
// TypeScript - BudSTT with feature flags
const session = await bud.stt.connect({
  provider: 'deepgram',
  language: 'en-US',
  model: 'nova-3',
  features: {
    vad: true,
    noise_cancellation: true,
    speaker_diarization: true,
    interim_results: true,
    word_timestamps: true
  }
});
```

```python
# Python - BudSTT with feature flags
session = await bud.stt.connect(
    provider="deepgram",
    language="en-US",
    model="nova-3",
    features=Features(
        vad=True,
        noise_cancellation=True,
        speaker_diarization=True,
        interim_results=True,
        word_timestamps=True
    )
)
```

---

## Phase 1: TypeScript SDK (`@bud-foundry/sdk`)

### 1.1 Pipeline Classes (Bud Naming)

```typescript
import { BudClient } from '@bud-foundry/sdk';

const bud = new BudClient({
  baseUrl: 'https://api.bud.ai',
  apiKey: 'bud_xxx'
});

// Pipeline-specific clients
const stt = bud.stt;       // BudSTT - Speech-to-Text
const tts = bud.tts;       // BudTTS - Text-to-Speech
const talk = bud.talk;     // BudTalk - Bidirectional voice
```

### 1.2 BudSTT - Speech-to-Text

```typescript
// Real-time streaming STT
const session = await bud.stt.connect({
  provider: 'deepgram',
  language: 'en-US',
  model: 'nova-3',
  features: { vad: true, speaker_diarization: true, interim_results: true }
});

session.on('transcript', (result) => {
  console.log(`[Speaker ${result.speaker_id}] ${result.text} (final: ${result.is_final})`);
});
session.on('metrics', (m) => console.log(`TTFT: ${m.ttft}ms`));

session.sendAudio(pcmData);
await session.close();

// Batch transcription
const result = await bud.stt.transcribe(audioFile, { language: 'en-US' });
```

### 1.3 BudTTS - Text-to-Speech

```typescript
// Streaming TTS
const session = await bud.tts.connect({
  provider: 'elevenlabs',
  voice: 'rachel',
  model: 'eleven_turbo_v2'
});

session.on('audio', (chunk) => player.play(chunk));
session.on('metrics', (m) => console.log(`TTFB: ${m.ttfb}ms`));

await session.speak('Hello, how can I help?');

// One-shot TTS (REST)
const audio = await bud.tts.synthesize('Hello world', {
  provider: 'deepgram',
  voice: 'aura-asteria-en'
});
```

### 1.4 BudTalk - Bidirectional Voice

```typescript
// Full voice conversation with metrics
const session = await bud.talk.connect({
  stt: { provider: 'deepgram', language: 'en-US', model: 'nova-3' },
  tts: { provider: 'elevenlabs', voice: 'rachel' },
  features: {
    vad: true,
    noise_cancellation: true,
    echo_cancellation: true
  },
  livekit: { room_name: 'voice-room' }
});

session.on('transcript', (t) => processUserInput(t.text));
session.on('audio', (chunk) => player.play(chunk));
session.on('metrics', (m) => {
  console.log(`E2E: ${m.e2e.latency}ms, TTFT: ${m.stt.ttft}ms, TTFB: ${m.tts.ttfb}ms`);
});

session.sendAudio(micData);
await session.speak('I understand. Let me help you with that.');
```

---

## Phase 2: Python SDK (`bud-foundry`)

### 2.1 Async-First Design

```python
from bud_foundry import BudClient
from bud_foundry.types import Features
import asyncio

async def main():
    bud = BudClient(base_url="https://api.bud.ai", api_key="bud_xxx")

    # BudSTT with feature flags
    async with bud.stt.connect(
        provider="deepgram",
        language="en-US",
        model="nova-3",
        features=Features(
            vad=True,
            speaker_diarization=True,
            noise_cancellation=True
        )
    ) as session:
        async for result in session.transcribe_stream(audio_generator()):
            print(f"[Speaker {result.speaker_id}] {result.text}")
            if result.is_final:
                metrics = session.get_metrics()
                print(f"TTFT: {metrics.stt.ttft.last}ms")

asyncio.run(main())
```

### 2.2 Sync Wrapper

```python
from bud_foundry.sync import BudClient

bud = BudClient(base_url="https://api.bud.ai", api_key="bud_xxx")

# One-shot TTS
audio = bud.tts.synthesize("Hello world", provider="deepgram", voice="aura-asteria-en")

# Batch transcription
result = bud.stt.transcribe("audio.wav", language="en-US")
print(result.text)
```

### 2.3 Dependencies

```toml
[project]
name = "bud-foundry"
dependencies = [
    "httpx>=0.25.0",
    "websockets>=12.0",
    "pydantic>=2.0",
]
```

---

## Phase 3: Embeddable Widget (`bud-widget.js`)

### 3.1 Usage

```html
<!-- Minimal -->
<script src="https://cdn.bud.ai/bud-widget.js"></script>
<bud-widget
  data-gateway-url="wss://api.bud.ai/ws"
  data-auth-token="bud_xxx">
</bud-widget>

<!-- Full configuration with feature flags -->
<bud-widget
  data-gateway-url="wss://api.bud.ai/ws"
  data-stt-provider="deepgram"
  data-tts-provider="elevenlabs"
  data-tts-voice="rachel"
  data-theme="dark"
  data-position="bottom-right"
  data-mode="vad"
  data-vad="true"
  data-noise-cancellation="true"
  data-show-metrics="true">
</bud-widget>
```

### 3.2 Features

- **Shadow DOM** for style isolation
- **Push-to-talk** and **VAD modes**
- **Feature flags** (VAD, noise cancellation, etc.)
- **Real-time metrics** display (TTFT, TTFB, E2E)
- **Provider selection** UI
- **Accessibility** (WCAG 2.1 AA)
- **Themes** (light/dark/auto)
- **< 50KB** gzipped

### 3.3 JavaScript API

```javascript
const widget = Bud.widget({
  gatewayUrl: 'wss://api.bud.ai/ws',
  apiKey: 'bud_xxx',
  stt: { provider: 'deepgram', language: 'en-US' },
  tts: { provider: 'elevenlabs', voice: 'rachel' },
  features: {
    vad: true,
    noise_cancellation: true,
    speaker_diarization: false
  },
  on: {
    transcript: (e) => myApp.process(e.text),
    metrics: (m) => console.log(`TTFT: ${m.stt.ttft}ms`),
    error: (e) => console.error(e)
  }
});

widget.speak('Hello!');
widget.connect();
widget.disconnect();
widget.getMetrics();
```

---

## Phase 4: Testing Dashboard

### 4.1 Panels

| Panel | Features |
|-------|----------|
| **Connection** | Server URL, auth token, connect/disconnect, status indicator |
| **STT** | Provider selection, feature flags (VAD, diarization, noise), microphone input, file upload |
| **TTS** | Provider selection, voice picker (from /voices), text input, speak button, audio playback |
| **LiveKit** | Token generation, room listing, room details, participant management |
| **SIP** | Hook listing, add/delete hooks, transfer testing |
| **API Explorer** | Generic REST client, method/endpoint/body inputs, response viewer |
| **WS Debug** | Message templates, raw JSON input, message log with filters |
| **Audio Tools** | Device selection, waveform visualization, level meters, file upload |
| **Metrics** | Real-time SLO dashboard, TTFT/TTFB/E2E graphs, percentile tracking |

### 4.2 Metrics Panel Features

```
┌─────────────────────────────────────────────────────────────────────┐
│ PERFORMANCE METRICS                                     [Export CSV]│
├─────────────────────────────────────────────────────────────────────┤
│  STT TTFT        TTS TTFB        E2E Latency       WS Connect      │
│  ┌────────┐      ┌────────┐      ┌────────┐        ┌────────┐      │
│  │ 165ms  │      │  95ms  │      │ 780ms  │        │  45ms  │      │
│  │  p95   │      │  p95   │      │  p95   │        │  p95   │      │
│  └────────┘      └────────┘      └────────┘        └────────┘      │
├─────────────────────────────────────────────────────────────────────┤
│  SLO Status:  ✅ STT < 200ms   ✅ TTS < 150ms   ⚠️ E2E < 1000ms     │
├─────────────────────────────────────────────────────────────────────┤
│  [═══════════════════════▓▓▓░░░░░░░░░░░░░░░░░░] Latency Timeline   │
└─────────────────────────────────────────────────────────────────────┘
```

### 4.3 Technology

- **Vanilla JavaScript** - No framework, no build step
- **Web Components** - Modular UI components
- **CSS Grid** - Responsive layout
- **Web Audio API** - Audio visualization
- **Canvas** - Real-time metrics graphs
- **< 200KB** total size

### 4.4 Layout

```
┌─────────────────────────────────────────────────────────────────────┐
│ BUD FOUNDRY DASHBOARD                           [Theme] [Settings]  │
├─────────────────────────────────────────────────────────────────────┤
│ Server: [ws://localhost:3001] Token: [bud_xxx] [Connect] ● Ready   │
├─────────────────────────────────────────────────────────────────────┤
│ [STT] [TTS] [LiveKit] [SIP] [API] [WS Debug] [Audio] [Metrics]     │
├────────────────────────────────────┬────────────────────────────────┤
│                                    │    METRICS & REQUEST LOG       │
│     ACTIVE PANEL CONTENT           │  TTFT: 165ms  TTFB: 95ms      │
│                                    │  ▸ POST /livekit/token  45ms   │
│   Feature Flags:                   │  ▸ WS: config                  │
│   ☑ VAD  ☑ Noise Cancel  ☐ Diariz │  ▸ WS: ready (TTFT: 165ms)     │
├────────────────────────────────────┴────────────────────────────────┤
│ [▁▂▃▄▅▆▇█▇▆▅▄▃▂▁] Audio Visualization              Volume: [====●] │
└─────────────────────────────────────────────────────────────────────┘
```

---

## Detailed Implementation Tasks

**Save Location:** `/home/bud/Desktop/bud_waav/WaaV/clients_sdk/docs/plans_tasks.md`

---

### Phase 0: Environment Setup

#### 0.1 Directory Structure Creation
- [ ] `mkdir -p /home/bud/Desktop/bud_waav/WaaV/clients_sdk/{typescript,python,widget,dashboard,docs}`
- [ ] `mkdir -p /home/bud/Desktop/bud_waav/WaaV/clients_sdk/typescript/{src,tests,dist}`
- [ ] `mkdir -p /home/bud/Desktop/bud_waav/WaaV/clients_sdk/typescript/src/{pipelines,rest,ws,audio,metrics,types,errors,utils}`
- [ ] `mkdir -p /home/bud/Desktop/bud_waav/WaaV/clients_sdk/python/{bud_foundry,tests}`
- [ ] `mkdir -p /home/bud/Desktop/bud_waav/WaaV/clients_sdk/python/bud_foundry/{pipelines,rest,ws,audio,metrics}`
- [ ] `mkdir -p /home/bud/Desktop/bud_waav/WaaV/clients_sdk/widget/{src,dist}`
- [ ] `mkdir -p /home/bud/Desktop/bud_waav/WaaV/clients_sdk/widget/src/{audio,ui}`
- [ ] `mkdir -p /home/bud/Desktop/bud_waav/WaaV/clients_sdk/dashboard/{js,css}`
- [ ] `mkdir -p /home/bud/Desktop/bud_waav/WaaV/clients_sdk/dashboard/js/components`

#### 0.2 TypeScript Environment
- [ ] Create `typescript/package.json` with name `@bud-foundry/sdk`, version `0.1.0`
- [ ] Add dev dependencies: `typescript@5.x`, `rollup`, `@rollup/plugin-typescript`, `@rollup/plugin-node-resolve`, `tslib`
- [ ] Add dev dependencies: `vitest`, `@vitest/coverage-v8`, `msw` (mock service worker for tests)
- [ ] Create `typescript/tsconfig.json` with `strict: true`, `target: ES2022`, `module: ESNext`
- [ ] Create `typescript/rollup.config.js` for ESM + CJS + UMD builds
- [ ] Run `cd typescript && npm install`
- [ ] Verify: `npm run build` succeeds with empty index.ts

#### 0.3 Python Environment
- [ ] Create `python/pyproject.toml` with name `bud-foundry`, version `0.1.0`
- [ ] Add dependencies: `httpx>=0.25.0`, `websockets>=12.0`, `pydantic>=2.0`
- [ ] Add dev dependencies: `pytest>=8.0`, `pytest-asyncio>=0.23`, `pytest-cov`, `ruff`, `mypy`
- [ ] Create `python/.python-version` with `3.11`
- [ ] Run `cd python && python -m venv .venv && source .venv/bin/activate && pip install -e ".[dev]"`
- [ ] Verify: `pytest --version` works

#### 0.4 Widget Environment
- [ ] Create `widget/package.json` with name `bud-widget`
- [ ] Add dev dependencies: `typescript`, `rollup`, `@rollup/plugin-typescript`, `rollup-plugin-terser`, `postcss`, `cssnano`
- [ ] Create `widget/tsconfig.json` targeting ES2020, DOM lib
- [ ] Run `cd widget && npm install`

#### 0.5 Dashboard Environment (No Build Step)
- [ ] Create `dashboard/index.html` with basic HTML5 structure
- [ ] Create `dashboard/styles.css` with CSS reset
- [ ] Create `dashboard/js/app.js` as entry point
- [ ] Verify: Opens in browser without errors

---

### Phase 1: TypeScript SDK (`@bud-foundry/sdk`)

#### 1.1 Core Types (`typescript/src/types/`)
- [ ] **1.1.1** Create `types/config.ts` - STTConfig, TTSConfig, LiveKitConfig interfaces (from `sayna/src/handlers/ws/config.rs`)
- [ ] **1.1.2** Create `types/messages.ts` - IncomingMessage, OutgoingMessage types (from `sayna/src/handlers/ws/messages.rs`)
- [ ] **1.1.3** Create `types/stt.ts` - STTResult, TranscriptEvent interfaces
- [ ] **1.1.4** Create `types/tts.ts` - TTSConfig, Voice, SpeakOptions interfaces
- [ ] **1.1.5** Create `types/livekit.ts` - LiveKitTokenRequest, LiveKitTokenResponse, RoomInfo interfaces
- [ ] **1.1.6** Create `types/sip.ts` - SIPHook, SIPTransferRequest interfaces
- [ ] **1.1.7** Create `types/metrics.ts` - PercentileStats, MetricsSummary, SLOStatus interfaces
- [ ] **1.1.8** Create `types/features.ts` - FeatureFlags interface (vad, noise_cancellation, etc.)
- [ ] **1.1.9** Create `types/index.ts` - Re-export all types

**Unit Tests (`typescript/tests/types/`):**
- [ ] **T1.1.1** `config.test.ts` - Type validation for STTConfig defaults
- [ ] **T1.1.2** `messages.test.ts` - JSON serialization/deserialization matches Rust format

#### 1.2 Error Classes (`typescript/src/errors/`)
- [ ] **1.2.1** Create `errors/base.ts` - BudError base class with code, message, cause
- [ ] **1.2.2** Create `errors/connection.ts` - ConnectionError, TimeoutError, ReconnectError
- [ ] **1.2.3** Create `errors/api.ts` - APIError (status code, response body)
- [ ] **1.2.4** Create `errors/stt.ts` - STTError, TranscriptionError
- [ ] **1.2.5** Create `errors/tts.ts` - TTSError, SynthesisError
- [ ] **1.2.6** Create `errors/index.ts` - Re-export all errors

**Unit Tests:**
- [ ] **T1.2.1** `errors.test.ts` - Error inheritance, serialization, cause chaining

#### 1.3 Metrics Collector (`typescript/src/metrics/`)
- [ ] **1.3.1** Create `metrics/types.ts` - PercentileTracker, MetricPoint, SLOThreshold
- [ ] **1.3.2** Create `metrics/percentile.ts` - TDigest-based percentile calculation (p50, p95, p99)
- [ ] **1.3.3** Create `metrics/collector.ts` - MetricsCollector class with record(), getMetrics(), reset()
- [ ] **1.3.4** Create `metrics/slo.ts` - SLOTracker with thresholds, violations, status
- [ ] **1.3.5** Create `metrics/index.ts` - Re-export

**Unit Tests:**
- [ ] **T1.3.1** `percentile.test.ts` - Accurate p50/p95/p99 calculation with 1000 samples
- [ ] **T1.3.2** `collector.test.ts` - Record metrics, sliding window, memory bounds
- [ ] **T1.3.3** `slo.test.ts` - Threshold violations, status transitions

**Performance Tests:**
- [ ] **P1.3.1** `metrics.perf.test.ts` - 100K record() calls < 100ms, memory < 1MB

#### 1.4 REST Client (`typescript/src/rest/`)
- [ ] **1.4.1** Create `rest/client.ts` - RestClient class with fetch wrapper, auth headers, timeout
- [ ] **1.4.2** Create `rest/endpoints.ts` - Endpoint constants (/, /voices, /speak, /livekit/*, /sip/*)
- [ ] **1.4.3** Create `rest/health.ts` - health() method returning HealthStatus
- [ ] **1.4.4** Create `rest/voices.ts` - listVoices() returning Voice[]
- [ ] **1.4.5** Create `rest/speak.ts` - speak(text, config) returning AudioBuffer
- [ ] **1.4.6** Create `rest/livekit.ts` - createToken(), getRoomInfo(), listRooms()
- [ ] **1.4.7** Create `rest/sip.ts` - listHooks(), createHook(), deleteHook()
- [ ] **1.4.8** Create `rest/recording.ts` - getRecording(streamId) returning Blob
- [ ] **1.4.9** Create `rest/index.ts` - Re-export RestClient

**Unit Tests (with MSW mocks):**
- [ ] **T1.4.1** `client.test.ts` - Auth header injection, timeout handling, error mapping
- [ ] **T1.4.2** `health.test.ts` - Health endpoint response parsing
- [ ] **T1.4.3** `voices.test.ts` - Voice list parsing, provider filtering
- [ ] **T1.4.4** `speak.test.ts` - Audio response handling, error cases
- [ ] **T1.4.5** `livekit.test.ts` - Token generation, room operations

**API Tests (against real Sayna at localhost:3001):**
- [ ] **A1.4.1** `api/health.api.test.ts` - Real health check response
- [ ] **A1.4.2** `api/voices.api.test.ts` - Real voice list retrieval
- [ ] **A1.4.3** `api/speak.api.test.ts` - Real TTS generation (skip if no API key)

#### 1.5 WebSocket Session (`typescript/src/ws/`)
- [ ] **1.5.1** Create `ws/connection.ts` - WebSocketConnection class with connect(), disconnect(), send()
- [ ] **1.5.2** Create `ws/reconnect.ts` - ReconnectStrategy with exponential backoff, jitter, max retries
- [ ] **1.5.3** Create `ws/messages.ts` - Message serialization/deserialization, binary handling
- [ ] **1.5.4** Create `ws/session.ts` - WebSocketSession class with event emitter, message routing
- [ ] **1.5.5** Create `ws/events.ts` - Event types: ready, transcript, audio, error, close, metrics
- [ ] **1.5.6** Create `ws/queue.ts` - MessageQueue for buffering during reconnection
- [ ] **1.5.7** Create `ws/index.ts` - Re-export WebSocketSession

**Unit Tests:**
- [ ] **T1.5.1** `reconnect.test.ts` - Backoff timing, jitter bounds, max retries
- [ ] **T1.5.2** `messages.test.ts` - JSON/binary serialization roundtrip
- [ ] **T1.5.3** `session.test.ts` - Event emission, message routing, state machine

**Integration Tests (against real Sayna):**
- [ ] **I1.5.1** `ws/session.integration.test.ts` - Connect, send config, receive ready
- [ ] **I1.5.2** `ws/reconnect.integration.test.ts` - Simulate disconnect, verify reconnection

#### 1.6 Audio Utilities (`typescript/src/audio/`)
- [ ] **1.6.1** Create `audio/processor.ts` - AudioProcessor class for Float32 ↔ Int16 conversion
- [ ] **1.6.2** Create `audio/player.ts` - PCMPlayer class using Web Audio API AudioWorklet
- [ ] **1.6.3** Create `audio/recorder.ts` - AudioRecorder class with MediaStream input
- [ ] **1.6.4** Create `audio/vad.ts` - VAD wrapper (simple energy-based or WebRTC VAD)
- [ ] **1.6.5** Create `audio/resampler.ts` - Resample between sample rates (16kHz ↔ 24kHz ↔ 48kHz)
- [ ] **1.6.6** Create `audio/index.ts` - Re-export

**Unit Tests:**
- [ ] **T1.6.1** `processor.test.ts` - Float32 ↔ Int16 accuracy, edge cases (clipping, silence)
- [ ] **T1.6.2** `resampler.test.ts` - Resample quality verification

**Performance Tests:**
- [ ] **P1.6.1** `audio.perf.test.ts` - Process 10 seconds of audio < 10ms

#### 1.7 Pipeline Classes (`typescript/src/pipelines/`)
- [ ] **1.7.1** Create `pipelines/base.ts` - BasePipeline abstract class with connect(), close(), getMetrics()
- [ ] **1.7.2** Create `pipelines/stt.ts` - BudSTT class with connect(), sendAudio(), on('transcript')
- [ ] **1.7.3** Create `pipelines/tts.ts` - BudTTS class with connect(), speak(), on('audio')
- [ ] **1.7.4** Create `pipelines/talk.ts` - BudTalk class combining STT+TTS with unified events
- [ ] **1.7.5** Create `pipelines/transcribe.ts` - BudTranscribe for batch file transcription
- [ ] **1.7.6** Create `pipelines/index.ts` - Re-export all pipelines

**Unit Tests:**
- [ ] **T1.7.1** `stt.test.ts` - Config validation, event emission, audio sending
- [ ] **T1.7.2** `tts.test.ts` - Speak command, audio reception, metrics
- [ ] **T1.7.3** `talk.test.ts` - Combined STT+TTS flow, interruption handling

**Integration Tests:**
- [ ] **I1.7.1** `stt.integration.test.ts` - Real STT with audio file (requires Deepgram key)
- [ ] **I1.7.2** `tts.integration.test.ts` - Real TTS generation (requires ElevenLabs key)

#### 1.8 Main Client (`typescript/src/`)
- [ ] **1.8.1** Create `bud.ts` - BudClient class with rest, stt, tts, talk, transcribe properties
- [ ] **1.8.2** Create `index.ts` - Main entry point, export BudClient and all types

**Unit Tests:**
- [ ] **T1.8.1** `bud.test.ts` - Client initialization, pipeline access

**End-to-End Tests:**
- [ ] **E1.8.1** `e2e/full-flow.e2e.test.ts` - Connect, transcribe audio, generate TTS response

---

### Phase 2: Python SDK (`bud-foundry`)

#### 2.1 Type Definitions (`python/bud_foundry/`)
- [ ] **2.1.1** Create `types.py` - Pydantic models: STTConfig, TTSConfig, LiveKitConfig, Features
- [ ] **2.1.2** Create `messages.py` - Pydantic models: IncomingMessage, OutgoingMessage variants
- [ ] **2.1.3** Create `errors.py` - Exception classes: BudError, ConnectionError, APIError, STTError, TTSError

**Unit Tests:**
- [ ] **T2.1.1** `tests/test_types.py` - Pydantic validation, serialization
- [ ] **T2.1.2** `tests/test_messages.py` - JSON roundtrip matching Rust format

#### 2.2 Metrics (`python/bud_foundry/metrics/`)
- [ ] **2.2.1** Create `metrics/collector.py` - MetricsCollector class with record(), get_metrics()
- [ ] **2.2.2** Create `metrics/percentile.py` - Percentile calculation (TDigest or reservoir sampling)
- [ ] **2.2.3** Create `metrics/slo.py` - SLOTracker with thresholds
- [ ] **2.2.4** Create `metrics/__init__.py` - Re-export

**Unit Tests:**
- [ ] **T2.2.1** `tests/metrics/test_collector.py` - Record and retrieve metrics
- [ ] **T2.2.2** `tests/metrics/test_percentile.py` - Accurate percentile calculation

#### 2.3 REST Client (`python/bud_foundry/rest/`)
- [ ] **2.3.1** Create `rest/client.py` - AsyncRestClient with httpx, auth, timeout
- [ ] **2.3.2** Create `rest/health.py` - health() coroutine
- [ ] **2.3.3** Create `rest/voices.py` - list_voices() coroutine
- [ ] **2.3.4** Create `rest/speak.py` - speak() coroutine
- [ ] **2.3.5** Create `rest/livekit.py` - create_token(), get_room_info() coroutines
- [ ] **2.3.6** Create `rest/sip.py` - list_hooks(), create_hook() coroutines
- [ ] **2.3.7** Create `rest/__init__.py` - Re-export

**Unit Tests (with httpx mock):**
- [ ] **T2.3.1** `tests/rest/test_client.py` - Auth, timeout, error handling
- [ ] **T2.3.2** `tests/rest/test_voices.py` - Voice list parsing

**API Tests:**
- [ ] **A2.3.1** `tests/api/test_health.py` - Real health endpoint
- [ ] **A2.3.2** `tests/api/test_voices.py` - Real voice listing

#### 2.4 WebSocket Session (`python/bud_foundry/ws/`)
- [ ] **2.4.1** Create `ws/connection.py` - AsyncWebSocketConnection using `websockets` library
- [ ] **2.4.2** Create `ws/session.py` - WebSocketSession with async iterator, events
- [ ] **2.4.3** Create `ws/reconnect.py` - Reconnection logic with backoff
- [ ] **2.4.4** Create `ws/__init__.py` - Re-export

**Unit Tests:**
- [ ] **T2.4.1** `tests/ws/test_session.py` - Message handling, event emission

**Integration Tests:**
- [ ] **I2.4.1** `tests/integration/test_ws.py` - Real WebSocket connection

#### 2.5 Audio Utilities (`python/bud_foundry/audio/`)
- [ ] **2.5.1** Create `audio/processor.py` - PCM conversion, numpy-based
- [ ] **2.5.2** Create `audio/vad.py` - VAD wrapper
- [ ] **2.5.3** Create `audio/__init__.py` - Re-export

**Unit Tests:**
- [ ] **T2.5.1** `tests/audio/test_processor.py` - Float32 ↔ Int16 conversion

#### 2.6 Pipelines (`python/bud_foundry/pipelines/`)
- [ ] **2.6.1** Create `pipelines/stt.py` - BudSTT async context manager
- [ ] **2.6.2** Create `pipelines/tts.py` - BudTTS async context manager
- [ ] **2.6.3** Create `pipelines/talk.py` - BudTalk combining STT+TTS
- [ ] **2.6.4** Create `pipelines/transcribe.py` - BudTranscribe for batch
- [ ] **2.6.5** Create `pipelines/__init__.py` - Re-export

**Unit Tests:**
- [ ] **T2.6.1** `tests/pipelines/test_stt.py` - Config, event handling
- [ ] **T2.6.2** `tests/pipelines/test_tts.py` - Speak, audio handling

#### 2.7 Main Client (`python/bud_foundry/`)
- [ ] **2.7.1** Create `client.py` - BudClient class with stt, tts, talk, transcribe
- [ ] **2.7.2** Create `sync/client.py` - Sync wrapper using asyncio.run()
- [ ] **2.7.3** Create `__init__.py` - Main exports

**End-to-End Tests:**
- [ ] **E2.7.1** `tests/e2e/test_flow.py` - Full async flow

---

### Phase 3: Embeddable Widget (`bud-widget.js`)

#### 3.1 Core Widget
- [ ] **3.1.1** Create `widget/src/widget.ts` - BudWidget Web Component class extending HTMLElement
- [ ] **3.1.2** Create `widget/src/config.ts` - WidgetConfig parsing from data attributes
- [ ] **3.1.3** Create `widget/src/state.ts` - WidgetState enum (idle, connecting, listening, speaking)
- [ ] **3.1.4** Create `widget/src/websocket.ts` - WebSocket connection handler
- [ ] **3.1.5** Create `widget/src/index.ts` - customElements.define('bud-widget', BudWidget)

#### 3.2 Audio Components
- [ ] **3.2.1** Create `widget/src/audio/recorder.ts` - Microphone recording with getUserMedia
- [ ] **3.2.2** Create `widget/src/audio/player.ts` - PCM playback via AudioWorklet
- [ ] **3.2.3** Create `widget/src/audio/vad.ts` - Voice activity detection

#### 3.3 UI Components
- [ ] **3.3.1** Create `widget/src/ui/styles.css` - Shadow DOM styles, themes
- [ ] **3.3.2** Create `widget/src/ui/icons.ts` - SVG icons (mic, speaker, loading)
- [ ] **3.3.3** Create `widget/src/ui/button.ts` - Main control button component
- [ ] **3.3.4** Create `widget/src/ui/metrics.ts` - Metrics overlay component

#### 3.4 Build
- [ ] **3.4.1** Create `widget/rollup.config.js` - Bundle to single file
- [ ] **3.4.2** Run build, verify < 50KB gzipped

**Unit Tests:**
- [ ] **T3.4.1** `widget/tests/config.test.ts` - Data attribute parsing
- [ ] **T3.4.2** `widget/tests/state.test.ts` - State transitions

---

### Phase 4: Testing Dashboard

#### 4.1 Structure
- [ ] **4.1.1** Create `dashboard/index.html` - Main HTML with CSS Grid layout
- [ ] **4.1.2** Create `dashboard/styles.css` - Complete styling, dark/light themes
- [ ] **4.1.3** Create `dashboard/js/app.js` - Main application entry

#### 4.2 Core Modules
- [ ] **4.2.1** Create `dashboard/js/state.js` - Global state management
- [ ] **4.2.2** Create `dashboard/js/websocket.js` - WebSocket connection handler
- [ ] **4.2.3** Create `dashboard/js/metrics.js` - Metrics collection and display
- [ ] **4.2.4** Create `dashboard/js/logger.js` - Request/response logging
- [ ] **4.2.5** Create `dashboard/js/audio.js` - Audio utilities

#### 4.3 Panel Components
- [ ] **4.3.1** Create `dashboard/js/components/connection.js` - Server URL, auth, connect button
- [ ] **4.3.2** Create `dashboard/js/components/stt.js` - STT panel with feature flags
- [ ] **4.3.3** Create `dashboard/js/components/tts.js` - TTS panel with voice picker
- [ ] **4.3.4** Create `dashboard/js/components/livekit.js` - LiveKit operations
- [ ] **4.3.5** Create `dashboard/js/components/sip.js` - SIP hooks management
- [ ] **4.3.6** Create `dashboard/js/components/api-explorer.js` - Generic REST client
- [ ] **4.3.7** Create `dashboard/js/components/ws-debug.js` - WebSocket message inspector
- [ ] **4.3.8** Create `dashboard/js/components/audio-tools.js` - Device selection, visualization
- [ ] **4.3.9** Create `dashboard/js/components/metrics-panel.js` - SLO dashboard

**Manual Testing Checklist:**
- [ ] Connection panel connects to localhost:3001
- [ ] STT panel transcribes microphone input
- [ ] TTS panel plays synthesized speech
- [ ] LiveKit panel generates tokens
- [ ] Metrics panel shows real-time latencies

---

### Test Summary

| Category | TypeScript | Python | Widget | Dashboard |
|----------|-----------|--------|--------|-----------|
| Unit Tests | 25 | 15 | 5 | - |
| Integration Tests | 5 | 3 | - | - |
| API Tests | 5 | 5 | - | - |
| E2E Tests | 1 | 1 | - | Manual |
| Performance Tests | 3 | - | - | - |
| **Total** | **39** | **24** | **5** | Manual |

---

### Regression Test Suite

#### TypeScript Regression (`typescript/tests/regression/`)
- [ ] **R1** WebSocket reconnection after server restart
- [ ] **R2** Binary audio frame boundary handling
- [ ] **R3** Concurrent speak() calls (queue behavior)
- [ ] **R4** Memory leak check after 1000 sessions
- [ ] **R5** Large transcript handling (>64KB)

#### Python Regression (`python/tests/regression/`)
- [ ] **R1** Async context manager cleanup on exception
- [ ] **R2** Connection timeout during high latency
- [ ] **R3** Concurrent session creation
- [ ] **R4** Memory stability with 100 sequential sessions

---

### Performance Benchmarks

| Benchmark | Target | File |
|-----------|--------|------|
| SDK initialization | < 5ms | `benchmarks/init.bench.ts` |
| Message serialization (1000 msgs) | < 10ms | `benchmarks/serialize.bench.ts` |
| Audio conversion (10s @ 16kHz) | < 5ms | `benchmarks/audio.bench.ts` |
| Metrics recording (100K points) | < 100ms | `benchmarks/metrics.bench.ts` |
| Widget bundle parse time | < 50ms | `benchmarks/widget.bench.ts` |

---

## Critical Source Files

| File | Purpose |
|------|---------|
| `sayna/src/handlers/ws/messages.rs` | WebSocket message types (IncomingMessage, OutgoingMessage) |
| `sayna/src/handlers/ws/config.rs` | STT/TTS/LiveKit config structures |
| `sayna/src/handlers/livekit/*.rs` | LiveKit REST endpoints |
| `sayna/src/handlers/sip/*.rs` | SIP REST endpoints |
| `sayna/src/handlers/voices.rs` | /voices endpoint |
| `sayna/src/handlers/speak.rs` | /speak endpoint |
| `sayna/docs/openapi.yaml` | Complete API specification |

---

## Design Principles

1. **Simple by default, powerful when needed** - Basic usage is 3 lines, full control available
2. **Type-safe** - Full TypeScript strict mode, Python type hints
3. **Zero/minimal dependencies** - Native fetch/WebSocket, httpx/websockets
4. **Tree-shakeable** - Import only what you need
5. **Browser + Node** - SDK works everywhere
6. **Metrics-first** - Built-in performance tracking and SLO monitoring
7. **Feature flags** - Easy toggles for VAD, noise cancellation, diarization
8. **Accessible** - Widget meets WCAG 2.1 AA
9. **Small footprint** - SDK < 15KB, Widget < 50KB gzipped

---

## Example: Complete Voice Bot (TypeScript)

```typescript
import { BudClient, AudioProcessor } from '@bud-foundry/sdk';

const bud = new BudClient({
  baseUrl: 'https://api.bud.ai',
  apiKey: process.env.BUD_API_KEY
});

// Start bidirectional voice session with feature flags
const session = await bud.talk.connect({
  stt: { provider: 'deepgram', language: 'en-US', model: 'nova-3' },
  tts: { provider: 'elevenlabs', voice: 'rachel' },
  features: {
    vad: true,
    noise_cancellation: true,
    speaker_diarization: true,
    interim_results: true
  }
});

// Create audio player
const player = AudioProcessor.createPCMPlayer(24000);

// Handle events with metrics
session.on('ready', () => console.log('Connected!'));
session.on('transcript', (r) => r.is_final && handleUserSpeech(r.text));
session.on('audio', (data) => player.play(data));
session.on('metrics', (m) => {
  console.log(`TTFT: ${m.stt.ttft}ms, TTFB: ${m.tts.ttfb}ms, E2E: ${m.e2e.latency}ms`);
});
session.on('error', (e) => console.error(e));

// Send microphone audio
const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
const processor = new AudioProcessor(stream, 16000);
processor.on('data', (pcm) => session.sendAudio(pcm));

// Respond to user
async function handleUserSpeech(text: string) {
  const response = await generateAIResponse(text);
  await session.speak(response);
}
```

## Example: Complete Voice Bot (Python)

```python
import asyncio
from bud_foundry import BudClient
from bud_foundry.types import Features

async def main():
    bud = BudClient(base_url="https://api.bud.ai", api_key="bud_xxx")

    async with bud.talk.connect(
        stt={"provider": "deepgram", "language": "en-US", "model": "nova-3"},
        tts={"provider": "elevenlabs", "voice": "rachel"},
        features=Features(
            vad=True,
            noise_cancellation=True,
            speaker_diarization=True
        )
    ) as session:
        async for event in session:
            if event.type == "transcript" and event.is_final:
                response = await generate_ai_response(event.text)
                await session.speak(response)

                # Log metrics
                metrics = session.get_metrics()
                print(f"TTFT: {metrics.stt.ttft.last}ms, E2E: {metrics.e2e.latency.last}ms")

asyncio.run(main())
```

---

## Additional Sources

- [Pragmatic Engineer - Building Great SDKs](https://newsletter.pragmaticengineer.com/p/building-great-sdks)
- [Azure SDK TypeScript Guidelines](https://azure.github.io/azure-sdk/typescript_design.html)
- [Twilio Voice AI Latency Guide](https://www.twilio.com/en-us/blog/developers/best-practices/guide-core-latency-ai-voice-agents)
- [Modal - One-Second Voice-to-Voice Latency](https://modal.com/blog/low-latency-voice-bot)
- [HTTPX Async Support](https://www.python-httpx.org/async/)
- [WebSocket Reconnect Best Practices](https://apidog.com/blog/websocket-reconnect/)
- [OpenTelemetry Metrics](https://opentelemetry.io/)
