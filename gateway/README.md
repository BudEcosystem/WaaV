# WaaV Gateway

A high-performance real-time voice processing server built in Rust that provides unified Speech-to-Text (STT) and Text-to-Speech (TTS) services through WebSocket and REST APIs.

## Features

- **Unified Voice API**: Single interface for multiple STT/TTS providers
- **Real-time Processing**: WebSocket-based bidirectional audio streaming
- **LiveKit Integration**: WebRTC audio streaming with room-based communication
- **Advanced Noise Filtering**: Optional DeepFilterNet integration (`noise-filter` feature)
- **Plugin Architecture**: Extensible capability-based plugin system with O(1) provider lookup
- **25 Built-in Providers**: Comprehensive coverage of cloud and enterprise STT/TTS services
  - **STT (11 providers)**: Deepgram, Google Cloud, ElevenLabs, Microsoft Azure, OpenAI Whisper, AssemblyAI, Cartesia, AWS Transcribe, IBM Watson, Groq, Gnani
  - **TTS (12 providers)**: Deepgram Aura, ElevenLabs, Google Cloud, Microsoft Azure, OpenAI, Cartesia Sonic, AWS Polly, IBM Watson, Hume AI, LMNT, Play.ht, Gnani
  - **Realtime (2 providers)**: OpenAI GPT-4o Realtime, Hume EVI
- **Indic Language Support**: Gnani provider with 14 STT languages and 12 TTS languages
- **Audio-Disabled Mode**: Development mode without API keys

## Quick Start

### Prerequisites

- Docker
- At least one provider API key (optional - can run in audio-disabled mode)

### Run with Docker

```bash
docker run -d \
  -p 3001:3001 \
  -e DEEPGRAM_API_KEY=your-key \
  waav-gatewayai/waav-gateway
```

The server will be available at `http://localhost:3001`.

### Docker Compose

```yaml
version: "3.9"
services:
  waav-gateway:
    image: waav-gatewayai/waav-gateway
    ports:
      - "3001:3001"
    environment:
      DEEPGRAM_API_KEY: ${DEEPGRAM_API_KEY}
      ELEVENLABS_API_KEY: ${ELEVENLABS_API_KEY}
      CACHE_PATH: /data/cache
    volumes:
      - waav-gateway-cache:/data/cache

volumes:
  waav-gateway-cache: {}
```

For complete Docker documentation including LiveKit integration, see [docs/docker.md](docs/docker.md).

### Running Without API Keys (Audio-Disabled Mode)

You can run WaaV Gateway without Deepgram or ElevenLabs API keys by using the audio-disabled mode. Simply start the server without configuring the API keys, then send a WebSocket configuration message with `audio_disabled: true`:

```json
{
  "type": "config",
  "config": {
    "audio_disabled": true,
    "stt_provider": "deepgram",
    "tts_provider": "elevenlabs"
  }
}
```

This mode is useful for:
- Local development and testing
- UI/UX development without audio processing
- Testing WebSocket message flows
- Debugging non-audio features

## Authentication (Optional)

WaaV Gateway supports customer-based authentication that delegates token validation to an external authentication service. When enabled, protected API endpoints require a valid bearer token.

### Enabling Authentication

Add to your `.env` file:
```env
AUTH_REQUIRED=true
AUTH_SERVICE_URL=https://your-auth-service.com/auth
AUTH_SIGNING_KEY_PATH=/path/to/auth_private_key.pem
AUTH_TIMEOUT_SECONDS=5
```

Generate signing keys:
```bash
# Generate RSA private key
openssl genrsa -out auth_private_key.pem 2048

# Extract public key (share with auth service)
openssl rsa -in auth_private_key.pem -pubout -out auth_public_key.pem
```

### Making Authenticated Requests

```bash
curl -X POST http://localhost:3001/speak \
  -H "Authorization: Bearer your-token-here" \
  -H "Content-Type: application/json" \
  -d '{"text": "Hello world"}'
```

For complete authentication setup and architecture details, see [docs/authentication.md](docs/authentication.md).

## Security Features

WaaV Gateway includes comprehensive security hardening for production deployments:

### SSRF Protection
All webhook URLs are validated to prevent Server-Side Request Forgery attacks:
- HTTPS required for all webhooks
- Raw IP addresses blocked (both IPv4 and IPv6)
- DNS resolution checked against private IP ranges
- Configurable for development mode with `validate_webhook_url_dev()`

### Connection Limiting
Per-IP connection limits prevent resource exhaustion:
- Configurable via `MAX_CONNECTIONS_PER_IP` (default: 100)
- Global rate limiting via `RATE_LIMIT_REQUESTS_PER_SECOND` (default: 60/s)
- Burst allowance via `RATE_LIMIT_BURST_SIZE` (default: 10)

### Tenant Isolation
Multi-tenant deployments benefit from automatic resource scoping:
- Recording paths include `auth_id` for tenant isolation
- WebSocket sessions tracked by authentication context
- SIP hooks support per-tenant webhook routing

### Resource Management
Production-grade resource controls:
- Bounded channels prevent memory exhaustion under load
- HTTP connection pools with explicit limits and idle timeouts
- WebSocket idle timeout with jitter (prevents thundering herd)
- Pre-deserialization message size validation

## API Endpoints

### WebSocket

- **Endpoint**: `/ws`
- **Protocol**: WebSocket
- **Purpose**: Real-time bidirectional audio streaming and control

#### Message Types

**Configuration Message**:
```json
{
  "type": "config",
  "config": {
    "stt_provider": "deepgram",
    "tts_provider": "elevenlabs",
    "audio_disabled": false,
    "deepgram_model": "nova-2",
    "elevenlabs_voice_id": "voice_id_here"
  }
}
```

**Groq STT Configuration** (ultra-fast Whisper):
```json
{
  "type": "config",
  "config": {
    "stt_provider": "groq",
    "groq_model": "whisper-large-v3-turbo",
    "language": "en"
  }
}
```

**Gnani STT Configuration** (Indic languages):
```json
{
  "type": "config",
  "config": {
    "stt_provider": "gnani",
    "language": "hi-IN",
    "tts_provider": "gnani",
    "gnani_language_code": "Hi-IN",
    "gnani_voice_name": "speaker1"
  }
}
```

Gnani supports 14 STT languages (kn-IN, hi-IN, ta-IN, te-IN, gu-IN, mr-IN, bn-IN, ml-IN, pa-guru-IN, ur-IN, en-IN, en-GB, en-US, en-SG) and 12 TTS languages with multi-speaker support.

**Audio Input**: Binary audio data (16kHz, 16-bit PCM)

**Text Input**:
```json
{
  "type": "text",
  "text": "Convert this text to speech"
}
```

### REST API

- **Health Check**: `GET /` - Server health check endpoint
- **Voices**: `GET /voices` - List available TTS voices (requires auth if `AUTH_REQUIRED=true`)
- **Speak**: `POST /speak` - Generate speech from text (requires auth if `AUTH_REQUIRED=true`)
- **LiveKit Token**: `POST /livekit/token` - Generate LiveKit participant token (requires auth if `AUTH_REQUIRED=true`)
- **LiveKit Webhook**: `POST /livekit/webhook` - Webhook endpoint for LiveKit events (unauthenticated, uses LiveKit signature verification)
  - Called by LiveKit to deliver room and participant events
  - Validates requests using LiveKit's JWT signature mechanism
  - Logs SIP-related attributes for phone call troubleshooting
  - See [docs/livekit_webhook.md](docs/livekit_webhook.md) for details

## Architecture Overview

### Plugin Architecture

WaaV Gateway uses a **capability-based plugin architecture** that enables dynamic provider registration with O(1) lookup performance. For complete plugin documentation, see [docs/plugins.md](docs/plugins.md).

```mermaid
%%{init: {'theme': 'base', 'themeVariables': { 'primaryColor': '#4a90d9', 'primaryTextColor': '#ffffff', 'primaryBorderColor': '#2c5aa0', 'lineColor': '#5c6bc0', 'secondaryColor': '#81c784', 'tertiaryColor': '#fff3e0', 'background': '#fafafa', 'mainBkg': '#ffffff', 'nodeBorder': '#424242', 'clusterBkg': '#f5f5f5', 'clusterBorder': '#bdbdbd', 'titleColor': '#212121', 'edgeLabelBackground': '#ffffff'}}}%%
flowchart TB
    subgraph Registration["Plugin Registration Layer"]
        direction LR
        INV["inventory crate<br/>(compile-time)"]
        PHF["PHF Static Map<br/>(O(1) lookup)"]
        DASH["DashMap Registry<br/>(runtime)"]
        INV --> PHF --> DASH
    end

    subgraph Capabilities["Capability Types"]
        direction LR
        STT_CAP["STTCapability<br/>11 providers"]
        TTS_CAP["TTSCapability<br/>12 providers"]
        RT_CAP["RealtimeCapability<br/>2 providers"]
        PROC_CAP["AudioProcessor<br/>VAD, Noise Filter"]
        MW_CAP["Middleware<br/>Auth, Rate Limit"]
        WS_CAP["WSHandler<br/>Custom Messages"]
    end

    subgraph Isolation["Safety Layer"]
        CATCH["catch_unwind<br/>Panic Isolation"]
        LIFE["Lifecycle Manager<br/>State Machine"]
    end

    Registration --> Capabilities
    Capabilities --> Isolation

    style Registration fill:#e3f2fd,stroke:#1976d2,stroke-width:2px
    style Capabilities fill:#e8f5e9,stroke:#388e3c,stroke-width:2px
    style Isolation fill:#fff3e0,stroke:#f57c00,stroke-width:2px
```

**Key Features:**
- **O(1) Provider Lookup**: PHF perfect hash maps for built-in providers
- **Panic Isolation**: Plugin panics are caught and converted to errors
- **Lifecycle Management**: Plugins transition through defined states
- **Concurrent Access**: Thread-safe DashMap for runtime registration

### Core Components

```mermaid
%%{init: {'theme': 'base', 'themeVariables': { 'primaryColor': '#4a90d9', 'primaryTextColor': '#ffffff', 'primaryBorderColor': '#2c5aa0', 'lineColor': '#5c6bc0', 'secondaryColor': '#81c784', 'tertiaryColor': '#fff3e0', 'background': '#fafafa', 'mainBkg': '#ffffff', 'nodeBorder': '#424242', 'clusterBkg': '#f5f5f5', 'clusterBorder': '#bdbdbd', 'titleColor': '#212121'}}}%%
graph TB
    subgraph Gateway["WaaV Gateway"]
        WS["WebSocket Handler<br/>/ws endpoint"]
        REST["REST API<br/>/speak, /voices"]
        REG["Plugin Registry<br/>O(1) Provider Lookup"]
        VM["VoiceManager<br/>Central Coordinator"]
        NF["DeepFilterNet<br/>Noise Reduction"]
        LK["LiveKit Integration<br/>WebRTC Streaming"]
    end

    subgraph Providers["Provider System (25 providers)"]
        STT["STT Providers<br/>(11)"]
        TTS["TTS Providers<br/>(12)"]
        RT["Realtime Providers<br/>(2)"]
    end

    subgraph STTList["STT Providers"]
        DG_S["Deepgram"]
        GC_S["Google Cloud"]
        AZ_S["Azure"]
        EL_S["ElevenLabs"]
        OA_S["OpenAI"]
        AA_S["AssemblyAI"]
        CA_S["Cartesia"]
        AWS_S["AWS Transcribe"]
        IBM_S["IBM Watson"]
        GQ_S["Groq"]
        GN_S["Gnani"]
    end

    subgraph TTSList["TTS Providers"]
        DG_T["Deepgram"]
        GC_T["Google Cloud"]
        AZ_T["Azure"]
        EL_T["ElevenLabs"]
        OA_T["OpenAI"]
        CA_T["Cartesia"]
        AWS_T["AWS Polly"]
        IBM_T["IBM Watson"]
        HU_T["Hume AI"]
        LM_T["LMNT"]
        PH_T["Play.ht"]
        GN_T["Gnani"]
    end

    Client((Client)) --> WS
    Client --> REST
    WS --> VM
    REST --> VM
    VM --> REG
    REG --> STT
    REG --> TTS
    REG --> RT
    VM --> NF
    VM --> LK
    STT --> STTList
    TTS --> TTSList
    RT --> OA_RT["OpenAI Realtime<br/>GPT-4o"]
    RT --> HU_RT["Hume EVI<br/>Empathic Voice"]
    LK --> LiveKit[(LiveKit Server)]

    style Gateway fill:#e3f2fd,stroke:#1976d2,stroke-width:2px
    style Providers fill:#e8f5e9,stroke:#388e3c,stroke-width:2px
    style STTList fill:#fff3e0,stroke:#f57c00,stroke-width:1px
    style TTSList fill:#fce4ec,stroke:#c2185b,stroke-width:1px
```

### Request Flow

```mermaid
%%{init: {'theme': 'base', 'themeVariables': { 'primaryColor': '#4a90d9', 'primaryTextColor': '#212121', 'lineColor': '#5c6bc0', 'actorBkg': '#e3f2fd', 'actorBorder': '#1976d2', 'actorTextColor': '#212121', 'signalColor': '#424242', 'signalTextColor': '#212121', 'labelBoxBkgColor': '#f5f5f5', 'labelBoxBorderColor': '#bdbdbd', 'labelTextColor': '#212121', 'loopTextColor': '#212121', 'noteBkgColor': '#fff3e0', 'noteTextColor': '#212121', 'noteBorderColor': '#f57c00', 'activationBkgColor': '#e8f5e9', 'activationBorderColor': '#388e3c'}}}%%
sequenceDiagram
    participant C as Client
    participant WS as WebSocket Handler
    participant REG as Plugin Registry
    participant VM as VoiceManager
    participant NF as Noise Filter
    participant STT as STT Provider
    participant TTS as TTS Provider

    Note over C,TTS: Speech-to-Text Flow
    C->>WS: 1. Connect to /ws
    C->>WS: 2. Send config message
    WS->>REG: Lookup provider (O(1))
    REG->>VM: Create provider instance
    VM-->>C: Ready confirmation

    C->>WS: 3. Send audio data (PCM)
    WS->>NF: Filter audio (optional)
    NF->>STT: Clean audio
    STT->>VM: Transcription result
    VM->>WS: Send text to client
    WS-->>C: {"type": "transcript", "text": "..."}

    Note over C,TTS: Text-to-Speech Flow
    C->>WS: 4. Send text message
    WS->>VM: Process TTS request
    VM->>TTS: Generate speech
    TTS->>VM: Audio data
    VM->>WS: Stream audio
    WS-->>C: Binary audio chunks
```

### Audio Processing Pipeline

```mermaid
%%{init: {'theme': 'base', 'themeVariables': { 'primaryColor': '#4a90d9', 'primaryTextColor': '#212121', 'primaryBorderColor': '#2c5aa0', 'lineColor': '#5c6bc0', 'background': '#fafafa', 'mainBkg': '#ffffff', 'nodeBorder': '#424242', 'clusterBkg': '#f5f5f5', 'clusterBorder': '#bdbdbd'}}}%%
flowchart LR
    subgraph Input["Input"]
        A["Audio Input<br/>16kHz 16-bit PCM"]
    end

    subgraph Processing["WaaV Gateway Processing"]
        B{"Noise Filter<br/>Enabled?"}
        C["DeepFilterNet<br/>Noise Reduction"]
        D["STT Provider<br/>(via Plugin Registry)"]
    end

    subgraph Output["Output"]
        E["Text Output<br/>JSON Response"]
    end

    A --> B
    B -->|Yes| C
    B -->|No| D
    C --> D
    D --> E

    style Input fill:#e3f2fd,stroke:#1976d2,stroke-width:2px
    style Processing fill:#f5f5f5,stroke:#bdbdbd,stroke-width:2px
    style Output fill:#e8f5e9,stroke:#388e3c,stroke-width:2px
    style A fill:#bbdefb,stroke:#1976d2
    style B fill:#fff3e0,stroke:#f57c00
    style C fill:#ffecb3,stroke:#ffa000
    style D fill:#fce4ec,stroke:#c2185b
    style E fill:#c8e6c9,stroke:#388e3c
```

### LiveKit Integration

```mermaid
%%{init: {'theme': 'base', 'themeVariables': { 'primaryColor': '#4a90d9', 'primaryTextColor': '#212121', 'primaryBorderColor': '#2c5aa0', 'lineColor': '#5c6bc0', 'background': '#fafafa', 'mainBkg': '#ffffff', 'nodeBorder': '#424242', 'clusterBkg': '#f5f5f5', 'clusterBorder': '#bdbdbd'}}}%%
flowchart TB
    subgraph Clients["Clients"]
        Web["Web Client"]
        Mobile["Mobile App"]
        SIP["SIP Phone"]
    end

    subgraph LiveKit["LiveKit Server"]
        Room["Room Manager"]
        Track["Audio Tracks"]
    end

    subgraph Gateway["WaaV Gateway"]
        LKI["LiveKit Integration"]
        REG["Plugin Registry"]
        VM2["VoiceManager"]
        Providers["STT/TTS/Realtime<br/>(25 providers)"]
    end

    Web & Mobile & SIP --> Room
    Room --> Track
    Track <--> LKI
    LKI <--> VM2
    VM2 <--> REG
    REG <--> Providers

    style Clients fill:#e3f2fd,stroke:#1976d2,stroke-width:2px
    style LiveKit fill:#e8f5e9,stroke:#388e3c,stroke-width:2px
    style Gateway fill:#fff3e0,stroke:#f57c00,stroke-width:2px
```

## Development

For local development, you can build and run from source.

### Prerequisites

- Rust 1.88.0 or later
- Optional: ONNX Runtime (for turn detection feature)

### Building from Source

```bash
# Development build
cargo build

# Release build (optimized)
cargo build --release

# Run the server
cargo run

# Run with a config file
cargo run -- -c config.yaml
```

### Feature Flags

WaaV Gateway exposes several Cargo features that gate heavyweight subsystems:

- `turn-detect`: ONNX-based speech turn detection
- `noise-filter`: DeepFilterNet noise suppression pipeline
- `openapi`: OpenAPI 3.1 specification generation

```bash
# Run with turn detection
cargo run --features turn-detect

# Run with noise filter
cargo run --features noise-filter

# Run with multiple features
cargo run --features turn-detect,noise-filter,openapi
```

The Docker image includes `turn-detect` and `noise-filter` by default.

### Testing

```bash
# Run all tests
cargo test

# Run specific test
cargo test test_name

# Run with output
cargo test -- --nocapture
```

### Code Quality

```bash
# Format code
cargo fmt

# Run linter
cargo clippy

# Check for security vulnerabilities
cargo audit
```

### Building Docker Image Locally

```bash
# Build Docker image
docker build -t waav-gateway .

# Run container
docker run -p 3001:3001 --env-file .env waav-gateway
```

## Configuration

### Environment Variables

| Variable | Description | Default | Required |
|----------|-------------|---------|----------|
| `DEEPGRAM_API_KEY` | Deepgram API authentication | - | No* |
| `ELEVENLABS_API_KEY` | ElevenLabs API authentication | - | No* |
| `GOOGLE_APPLICATION_CREDENTIALS` | Path to Google Cloud service account JSON | - | No* |
| `AZURE_SPEECH_SUBSCRIPTION_KEY` | Azure Speech Services subscription key | - | No* |
| `AZURE_SPEECH_REGION` | Azure region (e.g., eastus, westeurope) | `eastus` | No* |
| `OPENAI_API_KEY` | OpenAI API key (for STT, TTS, and Realtime) | - | No* |
| `ASSEMBLYAI_API_KEY` | AssemblyAI API key (for streaming STT) | - | No* |
| `CARTESIA_API_KEY` | Cartesia API key (for STT/TTS) | - | No* |
| `AWS_ACCESS_KEY_ID` | AWS access key (for Transcribe/Polly) | - | No* |
| `AWS_SECRET_ACCESS_KEY` | AWS secret key (for Transcribe/Polly) | - | No* |
| `IBM_WATSON_API_KEY` | IBM Watson API key (for STT/TTS) | - | No* |
| `IBM_WATSON_INSTANCE_ID` | IBM Watson service instance ID | - | No* |
| `GROQ_API_KEY` | Groq API key (for ultra-fast Whisper STT) | - | No* |
| `PLAYHT_API_KEY` | Play.ht API key (for TTS) | - | No* |
| `PLAYHT_USER_ID` | Play.ht user ID (for TTS authentication) | - | No* |
| `HUME_API_KEY` | Hume AI API key (for EVI realtime and TTS) | - | No* |
| `LMNT_API_KEY` | LMNT API key (for ultra-low latency TTS) | - | No* |
| `GNANI_TOKEN` | Gnani.ai authentication token (for Indic STT/TTS) | - | No* |
| `GNANI_ACCESS_KEY` | Gnani.ai access key (for Indic STT/TTS) | - | No* |
| `GNANI_CERTIFICATE_PATH` | Path to Gnani SSL certificate (for mTLS auth) | - | No* |
| `LIVEKIT_URL` | LiveKit server WebSocket URL | `ws://localhost:7880` | No |
| `LIVEKIT_API_KEY` | LiveKit API key (for webhooks and token generation) | - | No*** |
| `LIVEKIT_API_SECRET` | LiveKit API secret (for webhooks and token generation) | - | No*** |
| `HOST` | Server bind address | `0.0.0.0` | No |
| `PORT` | Server port | `3001` | No |
| `AUTH_REQUIRED` | Enable authentication | `false` | No |
| `AUTH_API_SECRETS_JSON` | API secrets JSON array (`[{id, secret}]`) | - | Yes** |
| `AUTH_API_SECRET` | Legacy single API secret | - | No**** |
| `AUTH_API_SECRET_ID` | Legacy API secret id for `AUTH_API_SECRET` | `default` | No |
| `AUTH_SERVICE_URL` | External auth service endpoint | - | Yes** |
| `AUTH_SIGNING_KEY_PATH` | Path to JWT signing private key | - | Yes** |
| `AUTH_TIMEOUT_SECONDS` | Auth request timeout | `5` | No |

*Not required when using audio-disabled mode
**Required when `AUTH_REQUIRED=true` for the auth method you choose
***Required for LiveKit webhook validation and token generation features
****Legacy single-secret fallback; prefer `AUTH_API_SECRETS_JSON`

### SIP Configuration (Optional)

WaaV Gateway supports first-class SIP configuration for managing SIP-specific settings. See [docs/sip_config.md](docs/sip_config.md) for detailed documentation.

**YAML Configuration:**
```yaml
sip:
  room_prefix: "sip-"
  allowed_addresses:
    - "192.168.1.0/24"
    - "10.0.0.1"
  hook_secret: "your-signing-secret"  # Required if hooks configured
  hooks:
    - host: "example.com"
      url: "https://webhook.example.com/events"
```

**Important:** All SIP webhook forwarding requests are signed with HMAC-SHA256. You **must** configure `hook_secret` (or per-hook `secret` overrides) if using `sip.hooks`. See [docs/livekit_webhook.md#webhook-signing](docs/livekit_webhook.md#webhook-signing) for signature verification examples.

**Environment Variables:**
- `SIP_ROOM_PREFIX`: Room name prefix for SIP calls (required if SIP enabled)
- `SIP_ALLOWED_ADDRESSES`: Comma-separated IP addresses/CIDRs
- `SIP_HOOK_SECRET`: Global signing secret for webhook authentication (min 16 chars)
- `SIP_HOOKS_JSON`: JSON array of webhook configurations (with optional per-hook `secret` field)

## Performance Considerations

- **DeepFilterNet**: CPU-intensive processing uses thread pooling
- **Audio Buffering**: Optimized chunk processing for low latency
- **Connection Reuse**: Provider connections are maintained for efficiency
- **Async Processing**: Non-blocking WebSocket message handling
- **Memory Management**: Careful buffer management in audio loops

## Extending WaaV Gateway

WaaV Gateway's plugin architecture makes it easy to add custom providers:

### Adding a New Provider

1. Implement the appropriate trait (`BaseSTT`, `BaseTTS`, or `BaseRealtime`)
2. Create a metadata function describing your provider
3. Register with `inventory::submit!`

```rust
use waav_gateway::plugin::prelude::*;

fn my_provider_metadata() -> ProviderMetadata {
    ProviderMetadata::stt("my-provider", "My Custom STT")
        .with_features(["streaming", "word-timestamps"])
}

inventory::submit! {
    PluginConstructor::stt("my-provider", my_provider_metadata, create_my_provider)
}
```

For complete plugin development documentation, see [docs/plugins.md](docs/plugins.md).

## Contributing

1. Review the development rules in `.cursor/rules/`:
   - `rust.mdc`: Rust best practices
   - `core.mdc`: Business logic specifications
   - `axum.mdc`: Framework patterns
   - `livekit.mdc`: LiveKit integration details

2. Review plugin documentation in [docs/plugins.md](docs/plugins.md) for extending providers
3. Follow the existing code patterns and conventions
4. Add tests for new features
5. Ensure `cargo fmt` and `cargo clippy` pass

## Support

For issues, questions, or contributions, please visit the [GitHub repository](https://github.com/waav-gatewayai/waav-gateway).
