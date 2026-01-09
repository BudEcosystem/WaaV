# DAG-Based Routing System

WaaV Gateway's DAG (Directed Acyclic Graph) routing system enables flexible, customizable voice processing pipelines. Users can define complex audio processing workflows that route data through multiple providers, endpoints, and transformations.

## Overview

The DAG routing system allows you to:

- **Define custom processing pipelines** - Chain STT, TTS, LLM, and custom processors
- **Route to external services** - HTTP, gRPC, WebSocket, IPC, and LiveKit endpoints
- **Conditional routing** - Use Rhai expressions or simple switch patterns
- **Parallel processing** - Split/Join patterns for concurrent branch execution
- **A/B testing** - Route based on API key identity or custom conditions
- **Low latency** - Pre-compiled graphs with lock-free data passing

## Enabling DAG Routing

DAG routing is a compile-time feature. Enable it in your build:

```bash
# Build with DAG routing support
cargo build --release --features dag-routing

# Run with DAG routing
cargo run --features dag-routing
```

## Quick Start

### Simple Voice Pipeline

Send a DAG configuration in your WebSocket config message:

```json
{
  "type": "config",
  "stream_id": "session-123",
  "audio": true,
  "stt_config": { "provider": "deepgram" },
  "tts_config": { "provider": "elevenlabs" },
  "dag": {
    "id": "simple-voice-bot",
    "name": "Simple Voice Bot",
    "nodes": [
      { "id": "input", "type": "audio_input" },
      { "id": "stt", "type": "stt_provider", "provider": "deepgram" },
      { "id": "output", "type": "text_output", "destination": "web_socket" }
    ],
    "edges": [
      { "from": "input", "to": "stt" },
      { "from": "stt", "to": "output" }
    ],
    "entry_node": "input",
    "exit_nodes": ["output"]
  }
}
```

## DAG Definition Schema

### Top-Level Structure

```yaml
# YAML format (also supports JSON)
id: voice-bot-v1
name: Voice Bot Pipeline
version: "1.0.0"

nodes:
  - # Node definitions

edges:
  - # Edge definitions

entry_node: input        # Starting node ID
exit_nodes: [output]     # Terminal node IDs

api_key_routes:          # Optional: API key-based routing overrides
  tenant_a: custom_stt
  tenant_b: premium_stt

config:                  # Optional: Global DAG configuration
  node_timeout_ms: 30000
  max_concurrent_executions: 10
  enable_metrics: true
  enable_tracing: false
```

### Node Types

#### Input Nodes

```yaml
# Audio input from WebSocket or LiveKit
- id: audio_in
  type: audio_input

# Text input from WebSocket messages
- id: text_in
  type: text_input
```

#### Provider Nodes

```yaml
# Speech-to-Text provider
- id: stt
  type: stt_provider
  provider: deepgram       # Provider name (deepgram, google, azure, etc.)
  model: nova-2            # Optional: Model override
  language: en-US          # Optional: Language override

# Text-to-Speech provider
- id: tts
  type: tts_provider
  provider: elevenlabs
  voice_id: 21m00Tcm4TlvDq8ikWAM
  model: eleven_turbo_v2

# Realtime provider (e.g., OpenAI Realtime)
- id: realtime
  type: realtime_provider
  provider: openai
  model: gpt-4o-realtime-preview
```

#### Endpoint Nodes

```yaml
# HTTP endpoint
- id: llm
  type: http_endpoint
  url: "https://api.openai.com/v1/chat/completions"
  method: POST
  headers:
    Authorization: "Bearer ${OPENAI_API_KEY}"
    Content-Type: "application/json"
  timeout_ms: 30000

# gRPC endpoint
- id: inference
  type: grpc_endpoint
  address: "localhost:50051"
  service: "inference.InferenceService"
  method: "Process"
  timeout_ms: 10000

# WebSocket client endpoint
- id: external_ws
  type: websocket_endpoint
  url: "wss://api.example.com/stream"
  headers:
    Authorization: "Bearer ${API_KEY}"

# IPC shared memory endpoint (for local inference engines)
- id: whisper_local
  type: ipc_endpoint
  shm_name: /waav_whisper_shm
  input_format: pcm16
  output_format: json

# LiveKit WebRTC endpoint
- id: livekit
  type: livekit_endpoint
  room: null               # Use connection's room
  track_type: audio
```

#### Output Nodes

```yaml
# Audio output to WebSocket or LiveKit
- id: audio_out
  type: audio_output
  destination: web_socket  # web_socket, livekit, endpoint, broadcast, discard

# Text output (STT results, responses)
- id: text_out
  type: text_output
  destination: web_socket

# Webhook notification (fire-and-forget)
- id: webhook
  type: webhook_output
  url: "https://hooks.example.com/transcript"
  headers:
    X-API-Key: "${WEBHOOK_SECRET}"
```

#### Processing Nodes

```yaml
# Plugin-based processor
- id: vad
  type: processor
  plugin: silero_vad

# Data transformer (Rhai script)
- id: format_response
  type: transform
  script: |
    #{
      "messages": [
        #{ "role": "user", "content": transcript }
      ],
      "model": "gpt-4"
    }

# Passthrough (no-op, for graph organization)
- id: junction
  type: passthrough
```

#### Control Flow Nodes

```yaml
# Split (broadcast to parallel branches)
- id: split
  type: split
  branches: [branch_a, branch_b, branch_c]

# Join (aggregate parallel results)
- id: join
  type: join
  sources: [branch_a, branch_b]
  strategy: first          # first, all, best, merge
  selector: "results.max_by(|r| r.confidence)"  # For 'best' strategy
  merge_script: |          # For 'merge' strategy
    let combined = "";
    for r in results { combined += r.text; }
    combined

# Router (conditional branching)
- id: router
  type: router
  routes:
    - condition: "transcript.len() > 100"
      target: long_handler
      priority: 10
    - condition: "language == 'es-ES'"
      target: spanish_handler
    - target: default_handler
      default: true
```

### Edge Definitions

```yaml
edges:
  # Simple edge
  - from: input
    to: stt

  # Conditional edge with Rhai expression
  - from: stt
    to: llm
    condition: "is_final == true && transcript.len() > 5"
    priority: 10

  # Switch pattern (simpler than full expression)
  - from: stt
    to: handler
    switch:
      field: "language"
      cases:
        "en-US": english_handler
        "es-ES": spanish_handler
      default: default_handler

  # Edge with data transformation
  - from: stt
    to: llm
    transform: |
      #{
        "text": transcript,
        "timestamp": timestamp
      }

  # Ring buffer configuration
  - from: audio_in
    to: stt
    buffer_capacity: 8192  # Samples (default: 4096)
```

## Conditional Routing

### Rhai Expressions

DAG uses [Rhai](https://rhai.rs) for expression evaluation. Available context variables:

| Variable | Type | Description |
|----------|------|-------------|
| `transcript` | String | STT transcript text |
| `is_final` | bool | Whether result is final |
| `is_speech_final` | bool | Whether speech segment is complete |
| `confidence` | f64 | STT confidence score (0.0-1.0) |
| `language` | String | Detected language code |
| `stream_id` | String | Session stream ID |
| `api_key_id` | String | Authenticated API key identifier |
| `timestamp` | i64 | Unix timestamp in milliseconds |

Example expressions:

```yaml
# Check for final results with confidence
condition: "is_final && confidence > 0.8"

# Route based on language
condition: "language.starts_with('en')"

# Complex routing logic
condition: |
  let is_question = transcript.ends_with("?");
  let is_long = transcript.len() > 50;
  is_question || (is_final && is_long)
```

### Switch Patterns

For simple field matching, use switch patterns (more performant than expressions):

```yaml
switch:
  field: "stt_result.language"
  cases:
    "en-US": english_handler
    "en-GB": english_handler
    "es-ES": spanish_handler
    "fr-FR": french_handler
  default: default_handler
```

### API Key-Based Routing

Route entire pipelines based on authenticated API key:

```yaml
api_key_routes:
  # Route tenant_a to a custom STT node
  tenant_a: custom_stt_node
  # Route premium users to high-quality pipeline
  premium_*: premium_pipeline  # Wildcard patterns supported
```

## Parallel Processing (Split/Join)

Execute multiple branches concurrently:

```yaml
nodes:
  - id: split
    type: split
    branches: [stt_a, stt_b]

  - id: stt_a
    type: stt_provider
    provider: deepgram

  - id: stt_b
    type: stt_provider
    provider: google

  - id: join
    type: join
    sources: [stt_a, stt_b]
    strategy: best
    selector: "results.max_by(|r| r.confidence)"

edges:
  - from: input
    to: split
  - from: split
    to: stt_a
  - from: split
    to: stt_b
  - from: stt_a
    to: join
  - from: stt_b
    to: join
```

Join strategies:

| Strategy | Description |
|----------|-------------|
| `first` | Return first completed result |
| `all` | Wait for all, return array |
| `best` | Select using `selector` expression |
| `merge` | Combine using `merge_script` |

## Example Pipelines

### Voice Bot with LLM

```yaml
id: voice-bot-llm
name: Voice Bot with LLM
nodes:
  - id: input
    type: audio_input
  - id: stt
    type: stt_provider
    provider: deepgram
    model: nova-2
  - id: transform_request
    type: transform
    script: |
      #{
        "model": "gpt-4",
        "messages": [
          #{ "role": "system", "content": "You are a helpful assistant." },
          #{ "role": "user", "content": transcript }
        ]
      }
  - id: llm
    type: http_endpoint
    url: "https://api.openai.com/v1/chat/completions"
    method: POST
    headers:
      Authorization: "Bearer ${OPENAI_API_KEY}"
      Content-Type: "application/json"
  - id: extract_response
    type: transform
    script: "data.choices[0].message.content"
  - id: tts
    type: tts_provider
    provider: elevenlabs
  - id: output
    type: audio_output
    destination: web_socket

edges:
  - from: input
    to: stt
  - from: stt
    to: transform_request
    condition: "is_speech_final"
  - from: transform_request
    to: llm
  - from: llm
    to: extract_response
  - from: extract_response
    to: tts
  - from: tts
    to: output

entry_node: input
exit_nodes: [output]
```

### A/B Testing STT Providers

```yaml
id: ab-test-stt
name: A/B Test STT Providers
nodes:
  - id: input
    type: audio_input
  - id: router
    type: router
    routes:
      - condition: "api_key_id.starts_with('test_')"
        target: deepgram_stt
      - target: google_stt
        default: true
  - id: deepgram_stt
    type: stt_provider
    provider: deepgram
  - id: google_stt
    type: stt_provider
    provider: google
  - id: output
    type: text_output
    destination: web_socket

edges:
  - from: input
    to: router
  - from: router
    to: deepgram_stt
  - from: router
    to: google_stt
  - from: deepgram_stt
    to: output
  - from: google_stt
    to: output

entry_node: input
exit_nodes: [output]
```

### Local Inference with IPC

```yaml
id: local-inference
name: Local Whisper + Kokoro
nodes:
  - id: input
    type: audio_input
  - id: whisper_stt
    type: ipc_endpoint
    shm_name: /waav_whisper_shm
    input_format: pcm16
  - id: llm
    type: grpc_endpoint
    address: "localhost:50051"
    service: "llm.LLMService"
    method: "Generate"
  - id: kokoro_tts
    type: ipc_endpoint
    shm_name: /waav_kokoro_shm
    output_format: pcm16
  - id: output
    type: audio_output
    destination: livekit
  - id: webhook
    type: webhook_output
    url: "https://logs.example.com/transcript"

edges:
  - from: input
    to: whisper_stt
  - from: whisper_stt
    to: llm
    condition: "transcript.len() > 0"
  - from: whisper_stt
    to: webhook  # Log all transcripts
  - from: llm
    to: kokoro_tts
  - from: kokoro_tts
    to: output

entry_node: input
exit_nodes: [output]
```

## Performance Considerations

### Compile-Time Optimization

- DAGs are compiled once at connection setup
- Rhai expressions are pre-compiled to AST
- Topological order is pre-computed for execution
- Ring buffers are pre-allocated for edges

### Runtime Performance

| Component | Latency Target |
|-----------|----------------|
| DAG compilation | < 100ms |
| Condition evaluation | < 1μs |
| Inter-node transfer | Zero-copy |
| Audio hot path | < 10ms |

### Best Practices

1. **Minimize nodes in hot path** - Each node adds overhead
2. **Use switch patterns over expressions** when possible
3. **Pre-allocate buffer capacity** for high-throughput edges
4. **Use IPC endpoints** for local inference (lower latency than HTTP)
5. **Enable metrics** only in development/debugging

## Metrics and Monitoring

Enable metrics collection in the DAG config:

```yaml
config:
  enable_metrics: true
  enable_tracing: true
```

Metrics are exposed via the application's metrics endpoint and include:

- Node execution time (per-node histogram)
- Edge transfer latency
- Buffer utilization
- Error rates

## Error Handling

### Node Retry Configuration

```yaml
- id: llm
  type: http_endpoint
  url: "https://api.openai.com/..."
  retry_on_failure: true
  max_retries: 3
  timeout_ms: 30000
```

### Error Propagation

- Node failures return errors through the DAG
- Failed conditions are logged but don't stop execution
- External endpoint timeouts use configured `timeout_ms`

### Fallback Patterns

When DAG execution fails, the system falls back to standard VoiceManager processing. This ensures the connection remains functional even if the DAG has issues.

## Security

### Environment Variable Substitution

Use `${VAR_NAME}` for sensitive values:

```yaml
headers:
  Authorization: "Bearer ${OPENAI_API_KEY}"
  X-API-Key: "${WEBHOOK_SECRET}"
```

### API Key Isolation

- Each connection's DAG runs in isolation
- API key-based routing ensures tenant separation
- IPC endpoints require appropriate permissions

## Limitations

- DAG templates (`dag_template` field) not yet implemented
- Dynamic DAG modification not supported (reconnect required)
- Maximum 100 nodes per DAG (configurable)
- Expression evaluation timeout: 100ms

## Troubleshooting

### Common Issues

**DAG compilation fails:**
- Check for typos in node/edge IDs
- Ensure entry_node and exit_nodes exist
- Verify no cycles in the graph

**Conditions not matching:**
- Use `enable_tracing: true` to log evaluations
- Check variable names match expected context
- Verify expression syntax is valid Rhai

**Audio not flowing:**
- Confirm `audio_input` is the entry node
- Check edge conditions aren't too restrictive
- Verify buffer_capacity is sufficient

### Debug Logging

Set `RUST_LOG` for detailed DAG execution logs:

```bash
RUST_LOG=waav_gateway::dag=debug cargo run --features dag-routing
```
