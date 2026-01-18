# Phonexia STT Provider

## Overview

Phonexia is an on-premises/private-cloud speech technology provider specializing in Speech-to-Text, Voice Biometrics, Speaker Identification, and Language Identification. Unlike cloud-native providers, Phonexia is designed for self-hosted deployments with 57-64 language support and strong focus on Eastern European languages.

**Provider Type**: On-Premises / Self-Hosted (requires user-configured server URL)

## API Architecture

Phonexia offers two API interfaces:

### 1. gRPC API (Speech Platform 4 - Recommended)
- High-performance, streaming-native
- Protocol Buffer definitions available at [github.com/phonexia/protofiles](https://github.com/phonexia/protofiles)
- Python client: `pip install phonexia-grpc`
- Maximum message size: 4MB (stream for larger audio)

### 2. REST/WebSocket API (Speech Engine - Legacy)
- WebSocket endpoint: `GET /input_stream/websocket`
- Audio format: RAW s16le
- Results via polling, WebSocket, or webhooks

## gRPC Service Definition

### SpeechToText Service

```protobuf
service SpeechToText {
    // Synchronous transcription (streaming request/response)
    rpc Transcribe(stream TranscribeRequest) returns (stream TranscribeResponse);

    // List allowed graphemes and phonemes
    rpc ListAllowedSymbols(ListAllowedSymbolsRequest) returns (ListAllowedSymbolsResponse);
}
```

### Request Messages

```protobuf
message TranscribeRequest {
    bytes audio = 1;  // Audio data chunk
    TranscribeConfig config = 2;  // Optional configuration
}

message TranscribeConfig {
    repeated string preferred_phrases = 1;  // Bias towards specific phrases
    repeated RequestedAdditionalWord additional_words = 2;  // Custom vocabulary
    repeated ResultType result_types = 3;  // Output format selection
}

message RequestedAdditionalWord {
    string spelling = 1;  // Word text
    repeated string pronunciations = 2;  // Phonetic transcriptions (min 3 phonemes each)
}

enum ResultType {
    RESULT_TYPE_ONE_BEST = 0;
    RESULT_TYPE_N_BEST = 1;
    RESULT_TYPE_CONFUSION_NETWORK = 2;
}
```

### Response Messages

```protobuf
message TranscribeResponse {
    TranscribeResult result = 1;
    double processed_audio_length = 2;  // Seconds of audio processed
}

message TranscribeResult {
    OneBest one_best = 1;
    NBest n_best = 2;
    ConfusionNetwork confusion_network = 3;
    repeated AdditionalWord additional_words = 4;
    string language = 5;  // Detected/specified language
}

message OneBest {
    repeated OneBestSegment segments = 1;
}

message OneBestSegment {
    repeated Word words = 1;
}

message Word {
    string text = 1;
    double start_time = 2;  // Seconds from start
    double end_time = 3;
    double confidence = 4;  // 0.0-1.0
}
```

## WebSocket API (Legacy SPE)

### Connection

```
GET /input_stream/websocket?frequency=16000&channels=1
```

### Query Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| frequency | int | Yes | Sample rate (8000-48000 Hz) |
| channels | int | Yes | Number of audio channels (1 or 2) |

### Audio Format

- **Format**: RAW PCM signed 16-bit little-endian (s16le)
- **Encoding**: Linear PCM
- **Bit depth**: 16 bits

### WebSocket Behavior

1. WebSocket remains open until streaming task completes
2. Last result includes `is_last=true` flag
3. Server closes WebSocket after last result
4. On error: error message sent, then connection closed
5. Stream timeout: 10 seconds without data (configurable)

## Authentication

### Token-Based (Recommended)

1. **Login**: POST to `/login` with HTTP Basic Auth credentials
2. **Response**: `X-SessionID` header contains token
3. **Subsequent requests**: Include `X-SessionID: <token>` header

### HTTP Basic Auth

Server can be configured to accept Basic Auth directly without token exchange.

### gRPC Authentication

Use gRPC metadata for authentication:
```python
metadata = [('authorization', f'Bearer {token}')]
stub.Transcribe(request_iterator(), metadata=metadata)
```

## Supported Languages

57-64 languages with strong Eastern European support:

### Eastern European
- Czech, Slovak, Polish, Hungarian, Romanian, Bulgarian
- Serbian, Croatian, Slovenian, Bosnian, Montenegrin
- Ukrainian, Russian, Belarusian

### Western European
- English (US, UK), German, French, Spanish, Italian
- Portuguese, Dutch, Danish, Swedish, Norwegian, Finnish

### Middle Eastern
- Arabic (Gulf, Levantine, Egyptian, MSA)
- Turkish, Persian, Hebrew

### Asian
- Mandarin Chinese, Japanese, Korean
- Thai, Vietnamese, Indonesian

## Audio Requirements

| Parameter | gRPC | WebSocket |
|-----------|------|-----------|
| Sample Rate | 8000-48000 Hz | 8000-48000 Hz |
| Bit Depth | 16-bit | 16-bit |
| Channels | 1 (mono recommended) | 1-2 |
| Format | PCM, WAV, FLAC | RAW s16le only |
| Max Chunk | 4MB | Server-configured |

## Result Types

### One-Best
Single most likely transcription with timestamps and confidence scores.

### N-Best
Multiple alternative transcriptions ranked by probability.

### Confusion Network
Lattice-based output showing all word alternatives at each position, useful for:
- Keyword spotting
- Uncertainty analysis
- Post-processing optimization

## Error Handling

### gRPC Status Codes

| Code | Description |
|------|-------------|
| INVALID_ARGUMENT | Malformed request or invalid parameters |
| RESOURCE_EXHAUSTED | Server capacity exceeded |
| FAILED_PRECONDITION | Service not ready or license issue |
| UNAUTHENTICATED | Invalid or missing credentials |
| PERMISSION_DENIED | Insufficient permissions |

### REST API Error Codes

| Code | Description |
|------|-------------|
| 1003 | Missing required parameters |
| 1004 | Invalid parameter values |
| 1005 | Authentication required |

## Custom Vocabulary

### Preferred Phrases
Bias transcription towards expected phrases:
```python
config = TranscribeConfig(
    preferred_phrases=["technical term", "product name"]
)
```

### Additional Words
Add custom words with pronunciations:
```python
additional_words=[
    RequestedAdditionalWord(
        spelling="WaaV",
        pronunciations=["W AH V", "W EY V"]  # Min 3 phonemes each
    )
]
```

## Performance Characteristics

- **Throughput**: 1,800-3,700 hours/day on 8-core CPU (50% speech content)
- **GPU Support**: Available for Enhanced STT (Whisper-based)
- **Latency**: Depends on deployment configuration
- **Scalability**: Horizontal scaling via microservices

## Deployment Options

### Docker Microservices
```bash
docker pull phonexia/speech-to-text:latest
docker run -p 50051:50051 phonexia/speech-to-text
```

### Virtual Appliance
Pre-configured VM with REST API and web interface.

### Kubernetes
Helm charts available for orchestrated deployment.

## Integration Example

### gRPC Python Client

```python
import grpc
from phonexia.grpc.technologies.speech_to_text.v1 import speech_to_text_pb2
from phonexia.grpc.technologies.speech_to_text.v1 import speech_to_text_pb2_grpc

# Connect to self-hosted server
channel = grpc.insecure_channel('your-phonexia-server:50051')
stub = speech_to_text_pb2_grpc.SpeechToTextStub(channel)

# Configure transcription
config = speech_to_text_pb2.TranscribeConfig(
    result_types=[speech_to_text_pb2.RESULT_TYPE_ONE_BEST]
)

def request_iterator():
    # First request with config
    yield speech_to_text_pb2.TranscribeRequest(config=config)

    # Stream audio chunks
    with open('audio.wav', 'rb') as f:
        while chunk := f.read(4096):
            yield speech_to_text_pb2.TranscribeRequest(audio=chunk)

# Transcribe
for response in stub.Transcribe(request_iterator()):
    if response.result.one_best:
        for segment in response.result.one_best.segments:
            for word in segment.words:
                print(f"{word.text} ({word.start_time:.2f}-{word.end_time:.2f})")
```

### WebSocket Python Client

```python
import websocket
import struct

ws = websocket.create_connection(
    'ws://your-phonexia-server/input_stream/websocket?frequency=16000&channels=1'
)

# Stream audio
with open('audio.raw', 'rb') as f:
    while chunk := f.read(4096):
        ws.send(chunk, opcode=websocket.ABNF.OPCODE_BINARY)

# Signal end of stream
ws.close()
```

## WaaV Integration Notes

### Configuration Structure
```rust
pub struct PhonexiaSTTConfig {
    pub server_url: String,        // User's Phonexia server URL
    pub api_type: PhonexiaApiType, // gRPC or WebSocket
    pub auth_method: PhonexiaAuth, // Token or Basic
    pub username: Option<String>,
    pub password: Option<String>,
    pub token: Option<String>,
    pub sample_rate: u32,
    pub language: String,
    pub result_type: PhonexiaResultType,
    pub preferred_phrases: Vec<String>,
    pub additional_words: Vec<CustomWord>,
}
```

### Key Differences from Cloud Providers
1. **User-configured server URL** - No default endpoint
2. **Self-managed authentication** - Token/Basic auth, not API keys
3. **Multiple API types** - gRPC (preferred) and WebSocket
4. **On-premises deployment** - All processing on user's infrastructure

## Pricing

Contact Phonexia sales for licensing: https://www.phonexia.com

Licensing options:
- Per-server license
- Per-hour processing license
- Enterprise agreements

## Resources

- **Documentation**: https://docs.phonexia.com
- **gRPC Proto Files**: https://github.com/phonexia/protofiles
- **Python Package**: `pip install phonexia-grpc`
- **Product Page**: https://www.phonexia.com/product/speech-to-text/
- **Contact**: info@phonexia.com
