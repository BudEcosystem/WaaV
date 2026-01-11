# @bud-foundry/widget

Embeddable voice widget for Bud Foundry AI Gateway - Add voice capabilities to any website with a single line of HTML.

## Installation

### CDN (Recommended)

```html
<script src="https://cdn.bud.ai/bud-widget.min.js"></script>
```

### NPM

```bash
npm install @bud-foundry/widget
```

```javascript
import '@bud-foundry/widget';
```

## Quick Start

Add the widget to your HTML with a single tag:

```html
<bud-widget
  data-gateway-url="wss://api.bud.ai/ws"
  data-api-key="your-api-key"
  data-stt-provider="deepgram"
  data-tts-provider="elevenlabs"
  data-tts-voice="rachel">
</bud-widget>
```

The widget appears as a floating microphone button that opens a panel for voice interaction.

## Features

- **Drop-in Integration**: Single HTML tag to add voice to any website
- **Speech-to-Text**: Real-time transcription with multiple providers
- **Text-to-Speech**: High-quality speech synthesis
- **Bidirectional Voice**: Full conversation support
- **Customizable Theme**: Light, dark, and auto modes
- **Positioning**: Bottom-right, bottom-left, or custom positions
- **Performance Metrics**: Optional display of TTFT, latency stats
- **Mobile Responsive**: Works on desktop and mobile devices
- **No Dependencies**: Self-contained web component

---

## Configuration

### HTML Attributes

Configure the widget using `data-*` attributes:

```html
<bud-widget
  data-gateway-url="wss://api.bud.ai/ws"
  data-api-key="your-api-key"
  data-theme="dark"
  data-position="bottom-left"
  data-mode="talk"
  data-show-metrics="true"
  data-stt-provider="deepgram"
  data-stt-language="en-US"
  data-stt-model="nova-3"
  data-tts-provider="elevenlabs"
  data-tts-voice="rachel"
  data-tts-voice-id="21m00Tcm4TlvDq8ikWAM"
  data-tts-model="eleven_turbo_v2">
</bud-widget>
```

### Attribute Reference

| Attribute | Type | Default | Description |
|-----------|------|---------|-------------|
| `data-gateway-url` | string | - | WebSocket URL for the gateway (required) |
| `data-api-key` | string | - | API key for authentication |
| `data-theme` | `light` \| `dark` \| `auto` | `auto` | Widget color theme |
| `data-position` | `bottom-right` \| `bottom-left` \| `top-right` \| `top-left` | `bottom-right` | Widget position |
| `data-mode` | `stt` \| `tts` \| `talk` | `talk` | Operation mode |
| `data-show-metrics` | `true` \| `false` | `false` | Show performance metrics |
| `data-auto-connect` | `true` \| `false` | `false` | Connect on page load |

### STT Configuration

| Attribute | Type | Default | Description |
|-----------|------|---------|-------------|
| `data-stt-provider` | string | `deepgram` | STT provider |
| `data-stt-language` | string | `en-US` | Language code |
| `data-stt-model` | string | - | Provider-specific model |

### TTS Configuration

| Attribute | Type | Default | Description |
|-----------|------|---------|-------------|
| `data-tts-provider` | string | `elevenlabs` | TTS provider |
| `data-tts-voice` | string | - | Voice name |
| `data-tts-voice-id` | string | - | Voice ID (provider-specific) |
| `data-tts-model` | string | - | TTS model |

---

## JavaScript API

Access the widget programmatically through its JavaScript API.

### Getting the Widget Element

```javascript
const widget = document.querySelector('bud-widget');
```

### Methods

#### `speak(text: string): Promise<void>`

Speak text using TTS.

```javascript
await widget.speak('Hello, how can I help you?');
```

#### `startListening(): Promise<void>`

Start microphone recording for STT.

```javascript
await widget.startListening();
```

#### `stopListening(): void`

Stop microphone recording.

```javascript
widget.stopListening();
```

#### `connect(): Promise<void>`

Manually establish WebSocket connection.

```javascript
await widget.connect();
```

#### `disconnect(): void`

Close WebSocket connection.

```javascript
widget.disconnect();
```

#### `open(): void`

Open the widget panel.

```javascript
widget.open();
```

#### `close(): void`

Close the widget panel.

```javascript
widget.close();
```

#### `toggle(): void`

Toggle the widget panel.

```javascript
widget.toggle();
```

#### `getMetrics(): WidgetMetrics`

Get current performance metrics.

```javascript
const metrics = widget.getMetrics();
console.log(`TTFT: ${metrics.stt.ttft}ms`);
console.log(`Messages: ${metrics.messageCount}`);
```

### Properties

| Property | Type | Description |
|----------|------|-------------|
| `connected` | boolean | Whether WebSocket is connected |
| `isListening` | boolean | Whether microphone is active |
| `isOpen` | boolean | Whether panel is open |
| `state` | WidgetState | Current widget state |

---

## Events

Listen for widget events using standard DOM event listeners.

```javascript
const widget = document.querySelector('bud-widget');

widget.addEventListener('transcript', (e) => {
  const { text, is_final, speaker_id } = e.detail;
  console.log(`${is_final ? 'Final' : 'Interim'}: ${text}`);
});

widget.addEventListener('audio', (e) => {
  const { audio, duration } = e.detail;
  console.log(`Audio chunk: ${duration}ms`);
});

widget.addEventListener('connected', () => {
  console.log('WebSocket connected');
});

widget.addEventListener('disconnected', () => {
  console.log('WebSocket disconnected');
});

widget.addEventListener('error', (e) => {
  console.error('Widget error:', e.detail);
});

widget.addEventListener('statechange', (e) => {
  console.log(`State changed: ${e.detail.from} -> ${e.detail.to}`);
});
```

### Event Reference

| Event | Detail | Description |
|-------|--------|-------------|
| `transcript` | `{ text, is_final, speaker_id, confidence, words }` | Transcription result |
| `audio` | `{ audio: ArrayBuffer, duration }` | Audio chunk received |
| `connected` | - | WebSocket connected |
| `disconnected` | - | WebSocket disconnected |
| `error` | `{ message, code }` | Error occurred |
| `statechange` | `{ from, to }` | Widget state changed |
| `speechstart` | - | User started speaking |
| `speechend` | - | User stopped speaking |
| `open` | - | Panel opened |
| `close` | - | Panel closed |

---

## Programmatic Creation

Create widgets dynamically with JavaScript.

### Using Factory Function

```javascript
import { createWidget } from '@bud-foundry/widget';

const widget = createWidget({
  gatewayUrl: 'wss://api.bud.ai/ws',
  apiKey: 'your-api-key',
  theme: 'dark',
  position: 'bottom-left',
  stt: { provider: 'deepgram', language: 'en-US' },
  tts: { provider: 'elevenlabs', voice: 'rachel' }
});

document.body.appendChild(widget);
```

### Using UMD Global

```html
<script src="https://cdn.bud.ai/bud-widget.min.js"></script>
<script>
  const widget = BudWidget.create({
    gatewayUrl: 'wss://api.bud.ai/ws',
    apiKey: 'your-api-key',
    stt: { provider: 'deepgram' },
    tts: { provider: 'elevenlabs' }
  });

  document.body.appendChild(widget);
</script>
```

---

## Theming

### Built-in Themes

```html
<!-- Light theme -->
<bud-widget data-theme="light"></bud-widget>

<!-- Dark theme -->
<bud-widget data-theme="dark"></bud-widget>

<!-- Auto (follows system preference) -->
<bud-widget data-theme="auto"></bud-widget>
```

### Custom CSS Variables

Customize the widget appearance with CSS variables:

```css
bud-widget {
  /* Colors */
  --bud-primary-color: #6366f1;
  --bud-background: #ffffff;
  --bud-surface: #f8fafc;
  --bud-text: #1e293b;
  --bud-text-secondary: #64748b;
  --bud-border: #e2e8f0;

  /* Button */
  --bud-button-size: 56px;
  --bud-button-shadow: 0 4px 12px rgba(0, 0, 0, 0.15);

  /* Panel */
  --bud-panel-width: 380px;
  --bud-panel-max-height: 500px;
  --bud-panel-radius: 16px;

  /* Typography */
  --bud-font-family: system-ui, -apple-system, sans-serif;
  --bud-font-size: 14px;

  /* Animation */
  --bud-transition-duration: 200ms;
}
```

### Dark Theme Variables

```css
bud-widget[data-theme="dark"] {
  --bud-background: #1e293b;
  --bud-surface: #334155;
  --bud-text: #f8fafc;
  --bud-text-secondary: #94a3b8;
  --bud-border: #475569;
}
```

---

## Operation Modes

### STT Only Mode

Only speech-to-text, no TTS playback:

```html
<bud-widget
  data-mode="stt"
  data-stt-provider="deepgram">
</bud-widget>
```

### TTS Only Mode

Only text-to-speech, no microphone:

```html
<bud-widget
  data-mode="tts"
  data-tts-provider="elevenlabs"
  data-tts-voice="rachel">
</bud-widget>
```

### Talk Mode (Default)

Full bidirectional voice conversation:

```html
<bud-widget
  data-mode="talk"
  data-stt-provider="deepgram"
  data-tts-provider="elevenlabs">
</bud-widget>
```

---

## Performance Metrics

Enable metrics display to show real-time performance data:

```html
<bud-widget data-show-metrics="true"></bud-widget>
```

### Displayed Metrics

- **TTFT**: Time to first transcript (STT latency)
- **TTFB**: Time to first byte (TTS latency)
- **Messages**: Total transcript count
- **Latency**: Current WebSocket latency

### Accessing Metrics Programmatically

```javascript
const widget = document.querySelector('bud-widget');
const metrics = widget.getMetrics();

console.log({
  stt: {
    ttft: metrics.stt.ttft,
    transcriptionCount: metrics.stt.transcriptionCount
  },
  tts: {
    ttfb: metrics.tts.ttfb,
    speakCount: metrics.tts.speakCount
  },
  connection: {
    latency: metrics.connection.latency,
    reconnectCount: metrics.connection.reconnectCount
  }
});
```

---

## Examples

### Basic Integration

```html
<!DOCTYPE html>
<html>
<head>
  <title>Voice Assistant</title>
</head>
<body>
  <h1>My Website</h1>

  <script src="https://cdn.bud.ai/bud-widget.min.js"></script>
  <bud-widget
    data-gateway-url="wss://api.bud.ai/ws"
    data-api-key="your-api-key"
    data-stt-provider="deepgram"
    data-tts-provider="elevenlabs">
  </bud-widget>
</body>
</html>
```

### With Event Handling

```html
<bud-widget id="voice-widget"
  data-gateway-url="wss://api.bud.ai/ws"
  data-api-key="your-api-key">
</bud-widget>

<script>
  const widget = document.getElementById('voice-widget');

  widget.addEventListener('transcript', (e) => {
    if (e.detail.is_final) {
      // Send to your AI backend
      fetch('/api/chat', {
        method: 'POST',
        body: JSON.stringify({ message: e.detail.text })
      })
      .then(res => res.json())
      .then(data => {
        // Speak the response
        widget.speak(data.response);
      });
    }
  });
</script>
```

### React Integration

```jsx
import { useEffect, useRef } from 'react';
import '@bud-foundry/widget';

export function VoiceWidget({ onTranscript }) {
  const widgetRef = useRef(null);

  useEffect(() => {
    const widget = widgetRef.current;

    const handleTranscript = (e) => {
      if (e.detail.is_final) {
        onTranscript(e.detail.text);
      }
    };

    widget.addEventListener('transcript', handleTranscript);
    return () => widget.removeEventListener('transcript', handleTranscript);
  }, [onTranscript]);

  return (
    <bud-widget
      ref={widgetRef}
      data-gateway-url="wss://api.bud.ai/ws"
      data-api-key={process.env.NEXT_PUBLIC_BUD_API_KEY}
      data-stt-provider="deepgram"
      data-tts-provider="elevenlabs"
    />
  );
}
```

### Vue Integration

```vue
<template>
  <bud-widget
    ref="widget"
    :data-gateway-url="gatewayUrl"
    :data-api-key="apiKey"
    data-stt-provider="deepgram"
    data-tts-provider="elevenlabs"
    @transcript="onTranscript"
  />
</template>

<script setup>
import '@bud-foundry/widget';
import { ref, onMounted, onUnmounted } from 'vue';

const widget = ref(null);
const gatewayUrl = import.meta.env.VITE_GATEWAY_URL;
const apiKey = import.meta.env.VITE_API_KEY;

const onTranscript = (e) => {
  if (e.detail.is_final) {
    console.log('Transcript:', e.detail.text);
  }
};
</script>
```

---

## Browser Support

- Chrome 66+
- Firefox 60+
- Safari 12+
- Edge 79+

Requires:
- Web Components (Custom Elements v1)
- Web Audio API
- MediaDevices API (getUserMedia)
- WebSocket

---

## Troubleshooting

### Microphone Permission Denied

The widget requires microphone access for STT. Ensure:
1. The page is served over HTTPS (or localhost)
2. User grants microphone permission when prompted

### WebSocket Connection Failed

Check:
1. Gateway URL is correct and accessible
2. API key is valid
3. No CORS issues (gateway should allow your origin)

### No Audio Playback

Ensure:
1. User has interacted with the page (browser autoplay policy)
2. TTS provider and voice are configured correctly
3. Device audio is not muted

---

## Development

### Building

```bash
npm install
npm run build
```

### Development Server

```bash
npm run dev
```

### Running Tests

```bash
npm test
```

---

## License

MIT
