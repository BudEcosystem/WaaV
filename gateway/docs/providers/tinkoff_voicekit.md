# Tinkoff VoiceKit Integration Documentation

> **Provider:** Tinkoff VoiceKit
> **Product:** VoiceKit gRPC API
> **Last Updated:** 2026-01-13
> **Status:** IN_PROGRESS

---

## 1. Overview

Tinkoff VoiceKit is a Russian speech technology platform developed by Tinkoff Bank, offering enterprise-grade Speech-to-Text (STT) and Text-to-Speech (TTS) services via gRPC protocol. It's optimized for Russian language with high accuracy and low latency streaming capabilities.

### Key Capabilities

| Feature | Supported | Notes |
|---------|-----------|-------|
| STT | YES | gRPC streaming and batch |
| TTS | YES | gRPC streaming synthesis |
| Streaming STT | YES | Bidirectional real-time |
| Streaming TTS | YES | Chunked audio output |
| Multi-channel | YES | Up to 8 channels |
| SSML | YES | Full SSML support for TTS |

### USPs (Unique Selling Points)

1. **Russian Language Specialization**: Optimized for Russian with high accuracy
2. **gRPC Protocol**: Low-latency bidirectional streaming
3. **Multiple Recognition Modes**: Sync, streaming, long-running batch
4. **Enterprise Grade**: PCI DSS compliant, Russian data localization
5. **Flexible VAD**: Configurable Voice Activity Detection

---

## 2. API Architecture

### Base URLs

| Service | URL |
|---------|-----|
| gRPC Endpoint | `api.tinkoff.ai:443` |
| STT Proto | `tinkoff/cloud/stt/v1/stt.proto` |
| TTS Proto | `tinkoff/cloud/tts/v1/tts.proto` |
| Console | `https://voicekit.tinkoff.ru` |
| Docs | `https://voicekit.tinkoff.ru/docs` |

### Authentication

- **Type**: JWT with HMAC-SHA256 signing
- **Credentials**:
  - `TINKOFF_VOICEKIT_API_KEY`: API key (used as `kid` in JWT header)
  - `TINKOFF_VOICEKIT_SECRET_KEY`: Secret key (base64url-encoded HMAC key)
- **JWT Expiration**: 600 seconds default

**JWT Header:**
```json
{
  "alg": "HS256",
  "typ": "JWT",
  "kid": "<API_KEY>"
}
```

**JWT Payload:**
```json
{
  "iss": "test_issuer",
  "sub": "test_user",
  "aud": "tinkoff.cloud.stt",  // or "tinkoff.cloud.tts"
  "exp": <unix_timestamp>
}
```

**gRPC Metadata:**
```
authorization: Bearer <JWT_TOKEN>
```

### Proto Files

Proto definitions available at: https://github.com/Tinkoff/voicekit-examples/tree/master/apis

---

## 3. STT API Reference

### 3.1 Service Definition

```protobuf
service SpeechToText {
    // Synchronous recognition - single request/response
    rpc Recognize(RecognizeRequest) returns (RecognizeResponse);

    // Bidirectional streaming - real-time recognition
    rpc StreamingRecognize(stream StreamingRecognizeRequest)
        returns (stream StreamingRecognizeResponse);

    // Long-running async recognition for large files
    rpc LongRunningRecognize(LongRunningRecognizeRequest)
        returns (google.longrunning.Operation);

    // Client streaming with single response
    rpc StreamingUnaryRecognize(stream StreamingRecognizeRequest)
        returns (RecognizeResponse);
}
```

### 3.2 Audio Encodings

| Encoding | Value | Description |
|----------|-------|-------------|
| LINEAR16 | 1 | Uncompressed 16-bit signed little-endian PCM |
| RAW_OPUS | 2 | Opus encoded audio without container |
| MULAW | 5 | 8-bit G.711 mu-law |
| ALAW | 6 | 8-bit G.711 A-law |
| MPEG_AUDIO | 8 | MP3 audio |
| FLAC | 16 | FLAC lossless audio |

### 3.3 Recognition Config

```protobuf
message RecognitionConfig {
    AudioEncoding encoding = 1;
    uint32 sample_rate_hertz = 2;           // 1000-48000
    string language_code = 3;                // "ru-RU"
    uint32 max_alternatives = 4;             // Up to 10
    bool profanity_filter = 5;
    repeated SpeechContext speech_contexts = 6;
    bool enable_automatic_punctuation = 7;
    string model = 8;
    bool enable_denormalization = 9;
    bool enable_sentiment_analysis = 10;
    int32 num_channels = 11;                 // 1-8
    VADConfig vad = 12;
}
```

### 3.4 VAD Configuration

```protobuf
message VADConfig {
    float min_speech_duration = 1;        // seconds
    float max_speech_duration = 2;        // seconds
    float silence_duration_threshold = 3; // seconds
    float silence_prob_threshold = 4;     // 0.0-1.0
}
```

### 3.5 Streaming Recognition Request

```protobuf
message StreamingRecognizeRequest {
    oneof streaming_request {
        StreamingRecognitionConfig streaming_config = 1;
        bytes audio_content = 2;
    }
}

message StreamingRecognitionConfig {
    RecognitionConfig config = 1;
    bool single_utterance = 2;     // Stop after first utterance
    bool interim_results = 3;       // Return interim (partial) results
}
```

### 3.6 Streaming Response

```protobuf
message StreamingRecognizeResponse {
    repeated StreamingRecognitionResult results = 1;
    EndpointDetectionType endpoint_detection_type = 2;
}

message StreamingRecognitionResult {
    repeated SpeechRecognitionAlternative alternatives = 1;
    bool is_final = 2;
    float stability = 3;
    Duration recognition_start_offset = 4;
    Duration recognition_end_offset = 5;
}

message SpeechRecognitionAlternative {
    string transcript = 1;
    float confidence = 2;
    repeated WordInfo words = 3;
}
```

---

## 4. TTS API Reference

### 4.1 Service Definition

```protobuf
service TextToSpeech {
    // List available voices
    rpc ListVoices(ListVoicesRequest) returns (ListVoicesResponse);

    // Synchronous synthesis
    rpc Synthesize(SynthesizeSpeechRequest) returns (SynthesizeSpeechResponse);

    // Streaming synthesis
    rpc StreamingSynthesize(SynthesizeSpeechRequest)
        returns (stream StreamingSynthesizeSpeechResponse);
}
```

### 4.2 Voices

| Voice ID | Gender | Language | Description |
|----------|--------|----------|-------------|
| alyona | Female | ru-RU | Default Russian female voice |
| dorofeev | Male | ru-RU | Russian male voice |

### 4.3 Audio Formats

| Format | Sample Rates |
|--------|--------------|
| LINEAR16 | 8000, 16000, 22050, 24000, 44100, 48000 |
| RAW_OPUS | 8000, 16000, 24000, 48000 |
| MULAW | 8000 |
| ALAW | 8000 |

### 4.4 Synthesis Request

```protobuf
message SynthesizeSpeechRequest {
    SynthesisInput input = 1;
    VoiceSelectionParams voice = 2;
    AudioConfig audio_config = 3;
}

message SynthesisInput {
    oneof input_source {
        string text = 1;       // Plain text
        string ssml = 2;       // SSML markup
    }
}

message VoiceSelectionParams {
    string language_code = 1;  // "ru-RU"
    string name = 2;           // "alyona", "dorofeev"
    SsmlVoiceGender ssml_gender = 3;
}

message AudioConfig {
    AudioEncoding audio_encoding = 1;
    float speaking_rate = 2;   // 0.25 to 4.0, default 1.0
    float pitch = 3;           // -20.0 to 20.0 semitones
    float volume_gain_db = 4;  // -96.0 to 16.0 dB
    uint32 sample_rate_hertz = 5;
}
```

### 4.5 Synthesis Response

```protobuf
message SynthesizeSpeechResponse {
    bytes audio_content = 1;
}

message StreamingSynthesizeSpeechResponse {
    bytes audio_chunk = 1;
}
```

### 4.6 SSML Support

Supported SSML tags:

| Tag | Description | Example |
|-----|-------------|---------|
| `<speak>` | Root element | `<speak>Hello</speak>` |
| `<p>` | Paragraph | `<p>First paragraph.</p>` |
| `<s>` | Sentence | `<s>A sentence.</s>` |
| `<break>` | Pause | `<break time="500ms"/>` |
| `<prosody>` | Pitch, rate, volume | `<prosody rate="fast">text</prosody>` |
| `<say-as>` | Interpretation | `<say-as interpret-as="date">2024-01-15</say-as>` |
| `<sub>` | Substitution | `<sub alias="AI">AI</sub>` |
| `<phoneme>` | Pronunciation | `<phoneme alphabet="x-sampa" ph="...">word</phoneme>` |

---

## 5. Supported Languages

| Code | Language |
|------|----------|
| ru-RU | Russian |

---

## 6. Pricing

| Service | Price |
|---------|-------|
| STT Online Processing | 0.48 RUB/minute |
| STT Deferred Processing | 0.18 RUB/minute |
| TTS | Contact Sales |
| Educational Institutions | Free |

Purchase at: https://speech.tinkoff.ru

---

## 7. Error Handling

### gRPC Status Codes

| Code | Description | Action |
|------|-------------|--------|
| INVALID_ARGUMENT | Invalid parameters | Check request format |
| UNAUTHENTICATED | Invalid credentials | Verify API key/secret |
| RESOURCE_EXHAUSTED | Rate limited | Implement backoff |
| INTERNAL | Server error | Retry with backoff |
| UNAVAILABLE | Service unavailable | Retry with backoff |

---

## 8. Best Practices

### STT Optimization

1. **Use streaming for real-time**: StreamingRecognize for live audio
2. **Configure VAD appropriately**: Tune silence thresholds for your use case
3. **Set single_utterance wisely**: True for command recognition, false for continuous
4. **Use interim_results**: For real-time UI feedback
5. **Send audio in chunks**: 100ms chunks for optimal latency

### TTS Optimization

1. **Use streaming for long text**: StreamingSynthesize for faster time-to-first-byte
2. **Leverage SSML**: For better pronunciation control
3. **Batch short texts**: Reduce gRPC connection overhead
4. **Choose appropriate sample rate**: 16000 Hz for telephony, 24000+ for quality

### Connection Management

1. **Reuse gRPC channels**: Create once, reuse for multiple calls
2. **Handle reconnection**: Implement automatic reconnect on failure
3. **Use keepalive**: Configure gRPC keepalive for long-lived connections

---

## 9. Integration Plan for Bud WaaV

### Architecture Decision

**Approach**: Custom gRPC codec (Gnani-style) - NO proto generation needed

**Rationale**:
1. Tinkoff proto files have dependencies on google protos that conflict with existing google-api-proto
2. Custom codec approach (like Gnani) is simpler and avoids build.rs complexity
3. Manual message encoding/decoding provides more control
4. Tonic 0.11 already available in project

### Implementation Components

1. **TinkoffConfig** - Configuration with API key, secret, model settings
2. **TinkoffCodec** - Custom tonic codec for Tinkoff messages
3. **TinkoffStt** - BaseSTT implementation with gRPC streaming
4. **TinkoffTts** - BaseTTS implementation with gRPC streaming
5. **Plugin Registration** - Inventory-based registration

### File Structure

```
src/core/stt/tinkoff/
├── mod.rs           # Module exports and constants
├── config.rs        # TinkoffSttConfig, AudioEncoding enum
├── messages.rs      # Request/response types with manual encoding
├── grpc.rs          # gRPC client, codec, and streaming logic
└── provider.rs      # TinkoffStt implementation (BaseSTT trait)

src/core/tts/tinkoff/
├── mod.rs           # Module exports and constants
├── config.rs        # TinkoffTtsConfig, Voice enum
├── messages.rs      # Request/response types with manual encoding
├── grpc.rs          # gRPC client, codec, and streaming logic
└── provider.rs      # TinkoffTts implementation (BaseTTS trait)
```

### Implementation Phases

#### Phase 1: STT Implementation
1. Create config.rs with TinkoffSttConfig struct
2. Create messages.rs with manual protobuf encoding
3. Create grpc.rs following Gnani pattern
4. Create provider.rs implementing BaseSTT
5. Add tests for each component

#### Phase 2: TTS Implementation
1. Create config.rs with TinkoffTtsConfig struct
2. Create messages.rs with manual protobuf encoding
3. Create grpc.rs following Gnani pattern
4. Create provider.rs implementing BaseTTS
5. Add tests for each component

### Message Encoding

Since we're not generating proto types, we'll manually encode/decode using prost primitives:

```rust
// Example: StreamingRecognitionConfig encoding
use prost::Message;

#[derive(Clone, PartialEq, Message)]
pub struct RecognitionConfig {
    #[prost(enumeration = "AudioEncoding", tag = "1")]
    pub encoding: i32,
    #[prost(uint32, tag = "2")]
    pub sample_rate_hertz: u32,
    #[prost(string, tag = "3")]
    pub language_code: String,
    // ... other fields
}
```

### Authentication Flow

1. Receive API key + secret key in config
2. Create gRPC metadata with x-api-key and x-secret-key headers
3. Attach metadata to all gRPC requests
4. Handle JWT token refresh if using JWT auth

### Test Plan

1. **Config Tests**: API key validation, encoding validation, voice validation
2. **Message Tests**: Manual protobuf encoding/decoding roundtrip
3. **Connection Tests**: gRPC channel establishment, auth headers
4. **STT Streaming Tests**: Audio streaming, interim results, VAD
5. **TTS Streaming Tests**: Text synthesis, SSML, audio output
6. **Integration Tests**: End-to-end with mock gRPC server

---

## 10. References

- **API Documentation**: https://voicekit.tinkoff.ru/docs
- **Proto Files**: https://github.com/Tinkoff/voicekit-examples/tree/master/apis
- **Python SDK**: https://github.com/TinkoffCreditSystems/voicekit_client_python
- **Examples**: https://github.com/TinkoffCreditSystems/voicekit-examples
- **Console**: https://voicekit.tinkoff.ru
- **Support**: support@voicekit.tinkoff.ru

---

## 11. Changelog

| Date | Change |
|------|--------|
| 2026-01-13 | Initial documentation created |
