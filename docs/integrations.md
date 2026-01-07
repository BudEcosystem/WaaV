# WaaV Gateway Integrations

This document provides detailed integration guides for WaaV Gateway's provider ecosystem, security features, and client SDKs.

## Provider Integrations

WaaV Gateway supports 10+ STT providers, 11+ TTS providers, and 2 realtime audio-to-audio providers through a unified API.

### Speech-to-Text (STT) Providers

| Provider | Models | Languages | Key Features |
|----------|--------|-----------|--------------|
| Deepgram | Nova-2, Enhanced | 30+ | Real-time streaming, diarization |
| Google Cloud | Chirp, Latest | 125+ | Automatic punctuation, word timing |
| Microsoft Azure | Various | 100+ | Custom speech models |
| ElevenLabs | Scribe | 30+ | Low-latency streaming |
| OpenAI | Whisper | 99 | High accuracy, word timestamps |
| AssemblyAI | Streaming v3 | 99 | Immutable transcripts |
| Cartesia | Sonic | 20+ | Ultra-low latency |
| AWS Transcribe | Standard | 100+ | Enterprise integration |
| IBM Watson | Various | 30+ | Speaker diarization |
| Groq | Whisper Large v3 | 99 | 216x real-time speed |

### Text-to-Speech (TTS) Providers

| Provider | Voice Types | Latency | Key Features |
|----------|-------------|---------|--------------|
| Deepgram | Aura | ~150ms | Streaming, natural prosody |
| ElevenLabs | Multilingual v2 | ~200ms | Voice cloning, emotions |
| Google Cloud | WaveNet, Neural2, Studio | ~200ms | SSML support |
| Microsoft Azure | Neural | ~150ms | 400+ voices, custom neural |
| OpenAI | TTS-1, TTS-1-HD | ~300ms | 6 voices, HD quality |
| Cartesia | Sonic | ~100ms | Ultra-low latency |
| AWS Polly | Neural, Standard | ~200ms | SSML, speech marks |
| IBM Watson | Neural | ~250ms | Expressive voices |
| Hume AI | Octave | ~200ms | Emotion-aware synthesis |
| LMNT | Various | ~150ms | Voice cloning |
| Play.ht | PlayDialog | ~190ms | Multi-turn, cloning |

### Realtime Audio-to-Audio Providers

| Provider | Model | Features |
|----------|-------|----------|
| OpenAI Realtime | GPT-4o | Full-duplex conversation, function calling |
| Hume EVI | Empathic Voice | Emotion understanding, empathic responses |

## Security Integration

### SSRF Protection

WaaV Gateway validates all webhook URLs to prevent SSRF attacks:

```rust
use waav_gateway::utils::url_validation::validate_webhook_url;

// Production - strict validation
match validate_webhook_url("https://webhook.example.com/events") {
    Ok(()) => println!("URL is safe"),
    Err(e) => println!("URL blocked: {}", e),
}

// Development - allows localhost
match validate_webhook_url_dev("http://localhost:8080/webhook", true) {
    Ok(()) => println!("URL is safe for dev"),
    Err(e) => println!("URL blocked: {}", e),
}
```

**Validation Rules:**
- HTTPS required (HTTP blocked except localhost in dev mode)
- Raw IP addresses blocked (IPv4 and IPv6)
- Private IP ranges blocked after DNS resolution
- Documentation IPs blocked (192.0.2.0/24, 2001:db8::/32)

### Connection Limiting

Configure connection limits in your environment:

```bash
# Rate limiting
RATE_LIMIT_REQUESTS_PER_SECOND=60
RATE_LIMIT_BURST_SIZE=10

# Per-IP limits
MAX_CONNECTIONS_PER_IP=100
```

Or via YAML configuration:

```yaml
security:
  rate_limit_requests_per_second: 60
  rate_limit_burst_size: 10
  max_connections_per_ip: 100
```

### Tenant Isolation

For multi-tenant deployments, recordings are automatically scoped by `auth_id`:

```
# Recording path structure
{bucket}/{prefix}/{auth_id}/{stream_id}/audio.ogg

# Example
s3://my-bucket/recordings/tenant-123/stream-abc/audio.ogg
```

Enable authentication to activate tenant isolation:

```bash
AUTH_REQUIRED=true
AUTH_SERVICE_URL=https://your-auth-service.com/auth
AUTH_SIGNING_KEY_PATH=/path/to/private_key.pem
```

## Client SDK Integration

### WebSocket Connection

```javascript
const ws = new WebSocket('wss://your-gateway.com/ws');

// Configure STT/TTS providers
ws.send(JSON.stringify({
  type: 'config',
  config: {
    stt_provider: 'deepgram',
    tts_provider: 'elevenlabs',
    deepgram_model: 'nova-2',
    elevenlabs_voice_id: 'your-voice-id'
  }
}));

// Handle responses
ws.onmessage = (event) => {
  if (typeof event.data === 'string') {
    const msg = JSON.parse(event.data);
    switch (msg.type) {
      case 'ready':
        console.log('Connected, stream_id:', msg.stream_id);
        break;
      case 'transcript':
        console.log('STT result:', msg.text);
        break;
    }
  } else {
    // Binary audio data from TTS
    playAudio(event.data);
  }
};

// Send audio for STT (16kHz 16-bit PCM)
ws.send(audioBuffer);

// Request TTS
ws.send(JSON.stringify({
  type: 'speak',
  text: 'Hello, world!'
}));
```

### REST API Integration

```bash
# Health check
curl https://your-gateway.com/

# List voices (with auth)
curl -H "Authorization: Bearer $TOKEN" \
  https://your-gateway.com/voices?provider=elevenlabs

# Generate speech
curl -X POST -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"text": "Hello", "provider": "elevenlabs", "voice_id": "xyz"}' \
  https://your-gateway.com/speak \
  --output audio.pcm

# Get LiveKit token
curl -X POST -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"room_name": "my-room", "participant_name": "User", "participant_identity": "user-123"}' \
  https://your-gateway.com/livekit/token
```

### LiveKit Integration

WaaV Gateway integrates with LiveKit for WebRTC-based real-time communication:

1. **Get participant token** from `/livekit/token`
2. **Connect to LiveKit** using the token
3. **WaaV Gateway** joins as an agent to process audio

```javascript
// Client connects to LiveKit
const room = new Room();
await room.connect(livekitUrl, token);

// WaaV Gateway automatically:
// - Joins the room as an agent
// - Subscribes to audio tracks
// - Publishes processed audio
```

### SIP Integration

For telephony integration via LiveKit SIP:

```yaml
sip:
  room_prefix: "sip-"
  allowed_addresses:
    - "192.168.1.0/24"
  hook_secret: "your-webhook-secret"
  hooks:
    - host: "example.com"
      url: "https://webhook.example.com/sip-events"
```

Webhooks are signed with HMAC-SHA256. Verify signatures:

```python
import hmac
import hashlib

def verify_signature(payload: bytes, signature: str, secret: str) -> bool:
    expected = hmac.new(
        secret.encode(),
        payload,
        hashlib.sha256
    ).hexdigest()
    return hmac.compare_digest(f"sha256={expected}", signature)
```

## Performance Tuning

### Connection Pools

HTTP clients use bounded connection pools to prevent resource exhaustion:

```rust
// Default settings
pool_max_idle_per_host: 4-10 connections
pool_idle_timeout: 90 seconds
```

### Channel Sizing

Internal channels use bounded buffers (256 default) to provide backpressure:

```rust
// STT result channels
let (tx, rx) = mpsc::channel::<STTResult>(256);
```

### Idle Timeout

WebSocket connections are closed after 5 minutes of inactivity (with jitter):

```rust
// Base: 300s, Jitter: +/- 30s
// Actual timeout: 270-330 seconds
```

## Monitoring

### Health Checks

```bash
# Basic health
curl https://your-gateway.com/

# Response
{"status": "healthy", "version": "1.0.0"}
```

### Logging

Configure log levels via `RUST_LOG`:

```bash
RUST_LOG=waav_gateway=info,tower_http=debug
```

Key log events:
- Connection lifecycle (connect, disconnect, timeout)
- Provider errors (API failures, rate limits)
- Security events (blocked URLs, auth failures)
- Performance metrics (latency, throughput)
