# Smart Turn Architecture

This document describes the Smart Turn audio-based turn detection system, including multi-language support, model lifecycle management, and scaling strategies.

## Overview

Smart Turn uses an audio-based semantic approach to detect when a speaker has finished their turn. Unlike text-based turn detection, it analyzes raw waveforms using a Whisper encoder with a classification head, making it inherently multilingual and language-agnostic.

## Multi-Language Support

### How It Works

Smart Turn v3 operates on raw audio waveforms, not transcripts. The model uses:

1. **Whisper Tiny Encoder**: Extracts acoustic features from mel spectrograms
2. **Linear Classifier**: Binary classification (turn complete vs incomplete)

Since the model processes acoustic patterns rather than text, it naturally supports multiple languages without explicit language configuration.

### Supported Languages (14)

| Language | Code | Notes |
|----------|------|-------|
| English | EN | Primary training language |
| French | FR | Full support |
| German | DE | Full support |
| Spanish | ES | Full support |
| Portuguese | PT | Full support |
| Chinese | ZH | Mandarin |
| Japanese | JA | Full support |
| Hindi | HI | Full support |
| Italian | IT | Full support |
| Korean | KO | Full support |
| Dutch | NL | Full support |
| Polish | PL | Full support |
| Russian | RU | Full support |
| Turkish | TR | Full support |

### Language Detection

No language configuration is required. The model automatically handles multilingual audio because:
- Turn boundaries are language-agnostic (pauses, intonation patterns)
- Whisper's mel features capture universal acoustic patterns
- The classifier was trained on multilingual data

## Model Lifecycle Management

### Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                    Gateway Server Process                        │
├─────────────────────────────────────────────────────────────────┤
│  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐  │
│  │ VoiceManager 1  │  │ VoiceManager 2  │  │ VoiceManager N  │  │
│  │ (Connection A)  │  │ (Connection B)  │  │ (Connection N)  │  │
│  │                 │  │                 │  │                 │  │
│  │ SmartTurnProc.  │  │ SmartTurnProc.  │  │ SmartTurnProc.  │  │
│  │  ├─ SileroVAD   │  │  ├─ SileroVAD   │  │  ├─ SileroVAD   │  │
│  │  ├─ MelExtract  │  │  ├─ MelExtract  │  │  ├─ MelExtract  │  │
│  │  ├─ Detector    │  │  ├─ Detector    │  │  ├─ Detector    │  │
│  │  └─ Decision    │  │  └─ Decision    │  │  └─ Decision    │  │
│  └─────────────────┘  └─────────────────┘  └─────────────────┘  │
│                                                                  │
│  ┌────────────────────────────────────────────────────────────┐ │
│  │                    ONNX Runtime Pool                        │ │
│  │  ┌──────────────┐ ┌──────────────┐ ┌──────────────┐        │ │
│  │  │ Session Pool │ │ Session Pool │ │ Session Pool │ ...    │ │
│  │  │ (Silero VAD) │ │ (Smart Turn) │ │ (Shared)     │        │ │
│  │  └──────────────┘ └──────────────┘ └──────────────┘        │ │
│  └────────────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────────┘
```

### Model Loading Strategy

#### Current Implementation (Per-Connection)

Each VoiceManager/SmartTurnProcessor has its own ONNX session:

```rust
// In SmartTurnProcessor::new()
let detector = SmartTurnDetector::new(config.detector_config.clone())
    .await  // Loads model once per processor
```

**Pros:**
- Thread-safe without locking
- Independent state per connection
- Simple implementation

**Cons:**
- Memory overhead (~8MB per connection for Smart Turn model)
- Model load time (~100-500ms) on first connection

#### Memory Usage Estimate

| Component | Memory (per instance) |
|-----------|----------------------|
| Smart Turn Model | ~8 MB |
| Silero VAD Model | ~2 MB |
| Mel Extractor Buffers | ~1 MB |
| Audio Buffers | ~0.5 MB |
| **Total per connection** | **~11.5 MB** |

### Optimization: Shared Model Pool (Future)

For high-concurrency scenarios, a shared model pool can be implemented:

```rust
// Proposed: Global model pool
lazy_static! {
    static ref SMART_TURN_SESSION_POOL: Arc<SessionPool<SmartTurnSession>> = {
        Arc::new(SessionPool::new(
            || SmartTurnDetector::load_model(),
            pool_size: num_cpus::get(),
        ))
    };
}

// Usage in SmartTurnProcessor
pub async fn predict(&self, mel_frames: &[Vec<f32>]) -> Result<SmartTurnResult> {
    let session = SMART_TURN_SESSION_POOL.acquire().await?;
    let result = session.run_inference(mel_frames)?;
    // Session automatically returned to pool on drop
    Ok(result)
}
```

## Scaling Strategies

### Horizontal Scaling (Recommended)

Deploy multiple gateway instances behind a load balancer:

```
                    ┌──────────────────┐
                    │  Load Balancer   │
                    │  (HAProxy/Nginx) │
                    └────────┬─────────┘
                             │
          ┌──────────────────┼──────────────────┐
          │                  │                  │
    ┌─────▼─────┐     ┌─────▼─────┐     ┌─────▼─────┐
    │ Gateway 1 │     │ Gateway 2 │     │ Gateway N │
    │ (8 cores) │     │ (8 cores) │     │ (8 cores) │
    │ ~100 conn │     │ ~100 conn │     │ ~100 conn │
    └───────────┘     └───────────┘     └───────────┘
```

**Capacity Planning:**
- Each gateway: ~100 concurrent WebSocket connections
- Memory: ~1.5 GB per gateway (11.5 MB × 100 + overhead)
- CPU: 8 cores recommended (1 core can handle ~12-15 concurrent inferences)

### Vertical Scaling

For single-instance deployments:

| Connections | Recommended Specs |
|------------|-------------------|
| 1-10 | 2 cores, 2 GB RAM |
| 10-50 | 4 cores, 4 GB RAM |
| 50-100 | 8 cores, 8 GB RAM |
| 100-200 | 16 cores, 16 GB RAM |

### Resource Throttling

Configure resource limits to prevent overload:

```yaml
# config.yaml
smart_turn:
  # Maximum concurrent inferences
  max_concurrent_inferences: 16

  # Queue size for pending inferences
  inference_queue_size: 100

  # Timeout for queued inferences (ms)
  inference_queue_timeout_ms: 5000

  # Skip inference if queue full (degrade gracefully)
  skip_on_overload: true
```

### Kubernetes Deployment

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: waav-gateway
spec:
  replicas: 3  # Start with 3, scale based on load
  template:
    spec:
      containers:
      - name: gateway
        image: waav/gateway:latest
        resources:
          requests:
            memory: "2Gi"
            cpu: "4"
          limits:
            memory: "4Gi"
            cpu: "8"
        env:
        - name: RUST_LOG
          value: "info"
        - name: SMART_TURN_ENABLED
          value: "true"
---
apiVersion: autoscaling/v2
kind: HorizontalPodAutoscaler
metadata:
  name: waav-gateway-hpa
spec:
  scaleTargetRef:
    apiVersion: apps/v1
    kind: Deployment
    name: waav-gateway
  minReplicas: 2
  maxReplicas: 20
  metrics:
  - type: Resource
    resource:
      name: cpu
      target:
        type: Utilization
        averageUtilization: 70
  - type: Pods
    pods:
      metric:
        name: websocket_connections
      target:
        type: AverageValue
        averageValue: "80"
```

## Performance Characteristics

### Inference Latency

| Model | CPU (modern) | CPU (AWS t3) | Notes |
|-------|-------------|--------------|-------|
| Smart Turn v3.2 | 10-15ms | 60-70ms | int8 quantized |
| Silero VAD | 0.1-0.5ms | 1-2ms | Very lightweight |
| Mel Extraction | 1-5ms | 5-10ms | FFT operations |

### End-to-End Latency Budget

```
Audio chunk arrives (32ms @ 512 samples)
    │
    ├─ Silero VAD: 0.5ms
    │
    ├─ Mel Extraction: 3ms
    │
    ├─ Smart Turn Inference: 15ms
    │
    └─ Decision Engine: 0.1ms

Total: ~20ms (well within 32ms chunk window)
```

### Throughput

With smart batching and model pooling:

| Configuration | Throughput |
|---------------|------------|
| Single core, single model | ~60 inferences/sec |
| 4 cores, pooled | ~200 inferences/sec |
| 8 cores, pooled | ~400 inferences/sec |

## Configuration Reference

### Enable Smart Turn

```yaml
# config.yaml
smart_turn:
  enabled: true

  # VAD Configuration
  vad:
    threshold: 0.5
    chunk_size: 512
    sample_rate: 16000
    state_reset_interval_secs: 5.0  # Reset LSTM state periodically

  # Detector Configuration
  detector:
    model_path: "models/smart_turn.onnx"  # Optional: auto-downloads if missing
    threshold: 0.7
    hysteresis_frames: 3
    min_speech_ms: 300
    num_threads: 1

  # Decision Engine
  decision:
    audio_threshold: 0.7
    silence_threshold_ms: 3000
    max_turn_duration_ms: 30000
```

### Environment Variables

```bash
# Enable smart turn
export SMART_TURN_ENABLED=true

# Model path (optional)
export SMART_TURN_MODEL_PATH=/path/to/smart_turn.onnx

# Logging
export RUST_LOG=waav_gateway::core::smart_turn=debug
```

## Monitoring

### Metrics to Track

| Metric | Description | Alert Threshold |
|--------|-------------|-----------------|
| `smart_turn_inference_latency_ms` | Inference time | > 50ms p99 |
| `smart_turn_queue_depth` | Pending inferences | > 50 |
| `smart_turn_model_memory_bytes` | Model memory usage | > 100 MB |
| `smart_turn_accuracy` | Turn detection accuracy | < 90% |

### Prometheus Metrics

```rust
// Exposed metrics
smart_turn_inferences_total{result="complete|incomplete"}
smart_turn_inference_duration_seconds
smart_turn_vad_detections_total{speech="true|false"}
smart_turn_model_load_duration_seconds
```

## Troubleshooting

### High Latency

1. Check CPU utilization (`top`, `htop`)
2. Verify model is loaded (check startup logs)
3. Reduce `inference_interval_frames` for less frequent inference
4. Enable model pooling if many connections

### Memory Issues

1. Limit concurrent connections
2. Enable model pooling (shared sessions)
3. Reduce mel buffer size
4. Monitor with `RUST_LOG=debug` for memory leaks

### Model Download Failures

1. Check network connectivity to HuggingFace
2. Pre-download model: `wget -O models/smart_turn.onnx https://huggingface.co/pipecat-ai/smart-turn-v3/resolve/main/smart-turn-v3.2-cpu.onnx`
3. Set `model_path` explicitly in config

## References

- [Smart Turn GitHub](https://github.com/pipecat-ai/smart-turn)
- [Smart Turn HuggingFace](https://huggingface.co/pipecat-ai/smart-turn-v3)
- [Pipecat Documentation](https://docs.pipecat.ai/server/utilities/smart-turn/smart-turn-overview)
- [Daily Blog: Smart Turn v3](https://www.daily.co/blog/announcing-smart-turn-v3-with-cpu-inference-in-just-12ms/)
