# Yandex SpeechKit Integration

> **Provider:** Yandex SpeechKit
> **Type:** STT + TTS
> **Region:** Russia, CIS, Dubai
> **Status:** Implementation in Progress
> **Last Updated:** 2026-01-13

---

## Overview

Yandex SpeechKit is a cloud-based speech recognition and synthesis service from Yandex Cloud. It provides:

- **Speech-to-Text (STT)**: Real-time streaming and batch recognition
- **Text-to-Speech (TTS)**: Neural network-based voice synthesis with emotional voices
- **Multi-language support**: 15+ languages with focus on Russian and CIS languages
- **Premium voices**: Deep neural network synthesis with natural intonation

### Key Features

| Feature | STT | TTS |
|---------|-----|-----|
| Streaming | Yes (gRPC) | Yes (gRPC v3) |
| REST API | Yes (sync) | Yes (v1) |
| Languages | 15+ | 10+ |
| Emotions | N/A | Yes (Russian) |
| SSML | N/A | Yes |

---

## Authentication

### IAM Token (Recommended)

IAM tokens have a 12-hour lifetime and are the recommended authentication method.

```
Authorization: Bearer <IAM_TOKEN>
```

**Header Format:**
- HTTP: `Authorization: Bearer <IAM_TOKEN>`
- gRPC Metadata: `('authorization', 'Bearer <IAM_TOKEN>')`

### API Key (Alternative)

API keys don't expire but are less secure.

```
Authorization: Api-Key <API_KEY>
```

### Folder ID

Required for user accounts. Service accounts don't need folder ID.

```
x-folder-id: <FOLDER_ID>
```

Or as query parameter: `?folderId=<FOLDER_ID>`

---

## Speech-to-Text (STT)

### API Endpoints

| API Version | Protocol | Endpoint | Use Case |
|-------------|----------|----------|----------|
| Sync v1 | REST | `stt.api.cloud.yandex.net/speech/v1/stt:recognize` | Short audio (<30s) |
| Streaming v3 | gRPC | `stt.api.cloud.yandex.net:443` | Real-time streaming |
| Streaming v2 | gRPC | `stt.api.cloud.yandex.net:443` | Legacy streaming |
| Async | gRPC | `stt.api.cloud.yandex.net:443` | Long audio files |

### Synchronous Recognition (REST)

**Endpoint:** `POST https://stt.api.cloud.yandex.net/speech/v1/stt:recognize`

**Limits:**
- Maximum file size: 1 MB
- Maximum duration: 30 seconds
- Single channel only

**Query Parameters:**

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `lang` | string | No | `ru-RU` | Recognition language |
| `topic` | string | No | `general` | Language model |
| `profanityFilter` | boolean | No | `false` | Filter profanity |
| `rawResults` | boolean | No | `false` | Return numbers as words |
| `format` | string | No | `oggopus` | Audio format: `lpcm`, `oggopus` |
| `sampleRateHertz` | string | No | `48000` | Sample rate: `8000`, `16000`, `48000` |
| `folderId` | string | Yes* | - | Folder ID (required for user accounts) |

**Request:**
```bash
curl -X POST \
  "https://stt.api.cloud.yandex.net/speech/v1/stt:recognize?lang=ru-RU&format=lpcm&sampleRateHertz=16000" \
  -H "Authorization: Bearer ${IAM_TOKEN}" \
  -H "Content-Type: audio/x-pcm" \
  --data-binary @audio.raw
```

**Response:**
```json
{
  "result": "recognized text here"
}
```

### Streaming Recognition (gRPC v3)

**Service:** `speechkit.stt.v3.Recognizer`
**Method:** `RecognizeStreaming` (bidirectional streaming)

**Proto Files:**
- `yandex/cloud/ai/stt/v3/stt_service.proto`
- `yandex/cloud/ai/stt/v3/stt.proto`

**StreamingRequest Message:**
```protobuf
message StreamingRequest {
  oneof Event {
    StreamingOptions session_options = 1;
    AudioChunk chunk = 2;
    SilenceChunk silence_chunk = 3;
    Eou eou = 4;
  }
}
```

**StreamingOptions:**
```protobuf
message StreamingOptions {
  RecognitionModelOptions recognition_model = 1;
  EouClassifierOptions eou_classifier = 2;
  RecognitionClassifierOptions recognition_classifier = 3;
  SpeechAnalysisOptions speech_analysis = 4;
  SpeakerLabelingOptions speaker_labeling = 5;
}
```

**Session Constraints:**
- Maximum session timeout: 5 seconds of no audio
- Send audio at approximately real-time rate
- Single channel audio only

**Response Types:**
- `partial` - Intermediate results during speech
- `final` - Complete utterance results
- `final_refinement` - Normalized final results
- `eou_update` - End-of-utterance marker

### Supported Languages (STT)

| Code | Language |
|------|----------|
| `ru-RU` | Russian |
| `en-US` | English (US) |
| `de-DE` | German |
| `fr-FR` | French |
| `tr-TR` | Turkish |
| `he-IL` | Hebrew |
| `fi-FI` | Finnish |
| `sv-SE` | Swedish |
| `nl-NL` | Dutch |
| `pl-PL` | Polish |
| `pt-BR` | Portuguese |
| `it-IT` | Italian |
| `es-ES` | Spanish |
| `uz-UZ` | Uzbek |
| `kk-KK` | Kazakh |

### Audio Formats (STT)

| Format | Description | Header |
|--------|-------------|--------|
| `lpcm` | Linear PCM without WAV header | `audio/x-pcm` |
| `oggopus` | OGG container with Opus codec | `audio/ogg` |

### Sample Rates

- 8000 Hz (telephone quality)
- 16000 Hz (wideband)
- 48000 Hz (high quality, default)

---

## Text-to-Speech (TTS)

### API Endpoints

| API Version | Protocol | Endpoint | Use Case |
|-------------|----------|----------|----------|
| v1 | REST | `tts.api.cloud.yandex.net/speech/v1/tts:synthesize` | Simple synthesis |
| v3 | gRPC | `tts.api.cloud.yandex.net:443` | Streaming, templates |

### Synchronous Synthesis (REST v1)

**Endpoint:** `POST https://tts.api.cloud.yandex.net/speech/v1/tts:synthesize`

**Limits:**
- Maximum request body: 15 KB
- Maximum text length: 5,000 characters

**Request Parameters (form-urlencoded):**

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `text` | string | Yes* | - | UTF-8 text to synthesize |
| `ssml` | string | Yes* | - | SSML markup (alternative to text) |
| `lang` | string | No | `ru-RU` | Language code |
| `voice` | string | No | - | Voice ID (see voices list) |
| `emotion` | string | No | `neutral` | Emotion/role |
| `speed` | decimal | No | `1.0` | Speech rate (0.1 to 3.0) |
| `format` | string | No | `oggopus` | Output format: `lpcm`, `oggopus`, `mp3` |
| `sampleRateHertz` | string | No | `48000` | Sample rate (LPCM only) |
| `folderId` | string | Yes* | - | Folder ID (required for user accounts) |

**Request:**
```bash
curl -X POST \
  "https://tts.api.cloud.yandex.net/speech/v1/tts:synthesize" \
  -H "Authorization: Bearer ${IAM_TOKEN}" \
  -d "text=Hello, world!" \
  -d "voice=alena" \
  -d "emotion=good" \
  -d "format=mp3" \
  --output speech.mp3
```

**Response:**
Binary audio data in the requested format.

### Streaming Synthesis (gRPC v3)

**Service:** `speechkit.tts.v3.Synthesizer`
**Method:** `UtteranceSynthesis` (server streaming)

**Proto Files:**
- `yandex/cloud/ai/tts/v3/tts_service.proto`
- `yandex/cloud/ai/tts/v3/tts.proto`

**UtteranceSynthesisRequest:**
```protobuf
message UtteranceSynthesisRequest {
  string model = 1;
  oneof Utterance {
    string text = 2;
    TextTemplate text_template = 3;
  }
  repeated Hints hints = 4;
  AudioFormatOptions output_audio_spec = 5;
  LoudnessNormalizationType loudness_normalization_type = 6;
  string unsafe_mode = 7;
}
```

**Hints:**
```protobuf
message Hints {
  oneof Hint {
    string voice = 1;
    string audio_template = 2;
    double speed = 3;
    double volume = 4;
    string role = 5;
    double pitch_shift = 6;
    DurationHint duration = 7;
  }
}
```

### Available Voices

#### Russian (ru-RU) - 16+ voices

| Voice ID | Gender | Emotions/Roles | API |
|----------|--------|----------------|-----|
| `alena` | Female | neutral, good (cheerful) | v1, v3 |
| `filipp` | Male | neutral | v1, v3 |
| `ermil` | Male | neutral, good | v1, v3 |
| `jane` | Female | neutral, good, evil (irritated) | v1, v3 |
| `omazh` | Female | neutral, evil | v1, v3 |
| `zahar` | Male | neutral, good | v1, v3 |
| `marina` | Female | neutral, whisper, friendly | v1, v3 |
| `dasha` | Female | neutral, good, friendly | v3 |
| `julia` | Female | neutral, strict | v3 |
| `lera` | Female | neutral, friendly | v3 |
| `masha` | Female | good, strict, friendly | v3 |
| `alexander` | Male | neutral, good | v3 |
| `kirill` | Male | neutral, strict, good | v3 |
| `anton` | Male | neutral, good | v3 |

#### Other Languages

| Voice ID | Language | Gender | API |
|----------|----------|--------|-----|
| `john` | en-US | Male | v1, v3 |
| `lea` | de-DE | Female | v1, v3 |
| `naomi` | he-IL | Female | v3 |
| `amira` | kk-KZ | Female | v1, v3 |
| `madi` | kk-KZ | Male | v1, v3 |
| `nigora` | uz-UZ | Female | v1, v3 |

### Emotion/Role Values

| Value | Description | Voices |
|-------|-------------|--------|
| `neutral` | Default neutral tone | All |
| `good` | Friendly/cheerful | alena, jane, ermil, zahar, etc. |
| `evil` | Irritated/angry | jane, omazh |
| `strict` | Formal/professional | julia, masha, kirill |
| `friendly` | Warm/approachable | marina, dasha, lera |
| `whisper` | Whispered speech | marina |

### Audio Formats (TTS)

| Format | MIME Type | Description |
|--------|-----------|-------------|
| `lpcm` | `audio/x-pcm` | Linear PCM (raw) |
| `oggopus` | `audio/ogg` | OGG with Opus codec (default) |
| `mp3` | `audio/mpeg` | MP3 |

### SSML Support

TTS v1 supports SSML markup:

```xml
<speak>
  <p>
    <s>This is the first sentence.</s>
    <s>This is the second sentence.</s>
  </p>
  <break time="500ms"/>
  <prosody rate="slow" pitch="low">Speaking slowly.</prosody>
</speak>
```

---

## Pricing

### Speech Recognition (STT)

**Billing Unit:** 15-second segment of single-channel audio

| Mode | Billing |
|------|---------|
| Synchronous | Per 15-second segment |
| Streaming | Per 15-second segment (from session start) |
| Asynchronous | Per 1-second segment (2-channel) |

### Speech Synthesis (TTS)

**API v1:** Per character per month
**API v3:** Per request (250-character units)

| Characters | v3 Billing Units |
|------------|------------------|
| < 250 | 1 |
| 250-500 | 2 |
| 500-750 | 3 |
| etc. | +1 per 250 chars |

**Starting Price:** $0.000020 per unit

---

## Implementation Plan

### Phase 1: STT Implementation

1. **Create config module** (`src/core/stt/yandex/config.rs`)
   - `YandexSttConfig` struct with language, topic, sample_rate
   - `YandexCredentials` struct with api_key OR iam_token
   - Audio format enum

2. **Create messages module** (`src/core/stt/yandex/messages.rs`)
   - REST response types
   - gRPC message wrappers (if using gRPC)
   - Error types

3. **Create provider module** (`src/core/stt/yandex/provider.rs`)
   - Implement `BaseSTT` trait
   - REST-based synchronous recognition
   - Streaming using WebSocket or gRPC

### Phase 2: TTS Implementation

1. **Create config module** (`src/core/tts/yandex/config.rs`)
   - `YandexTtsConfig` struct with voice, emotion, speed, format
   - Voice enum with all supported voices
   - Emotion enum

2. **Create messages module** (`src/core/tts/yandex/messages.rs`)
   - REST response handling
   - Error types

3. **Create provider module** (`src/core/tts/yandex/provider.rs`)
   - Implement `BaseTTS` trait
   - HTTP POST synthesis
   - Audio format handling

### Phase 3: Plugin Registration

1. Add STT provider to plugin system
2. Add TTS provider to plugin system
3. Register aliases: `yandex`, `yandex-speechkit`, `speechkit`

### Phase 4: Testing

1. Unit tests for config parsing
2. Unit tests for message serialization
3. Integration tests with mock server
4. Real API tests (if credentials available)

---

## Best Practices

### Performance Optimization

1. **Reuse IAM tokens**: Cache tokens and refresh before 12-hour expiry
2. **Use streaming for real-time**: gRPC streaming provides lower latency
3. **Batch short requests**: Combine short texts in TTS v3 for efficiency
4. **Select appropriate sample rate**: 16kHz is usually optimal for voice

### Error Handling

1. **Token expiry**: Auto-refresh IAM tokens before expiry
2. **Rate limiting**: Implement exponential backoff
3. **Network errors**: Retry with backoff for transient failures
4. **Invalid audio**: Validate format and sample rate before sending

### Cost Optimization

1. **Use TTS v3 for long texts**: Better per-character pricing
2. **Compress audio for STT**: Use OggOpus to reduce bandwidth
3. **Batch requests where possible**: Reduce per-request overhead

---

## Error Codes

| Code | Description | Action |
|------|-------------|--------|
| 401 | Unauthorized | Check IAM token or API key |
| 403 | Forbidden | Check folder permissions |
| 400 | Bad Request | Validate request parameters |
| 413 | Payload Too Large | Reduce audio file size |
| 429 | Rate Limited | Implement backoff |
| 500 | Server Error | Retry with backoff |

---

## References

- [Official Documentation](https://yandex.cloud/en/docs/speechkit/)
- [API Reference (GitHub)](https://github.com/yandex-cloud/docs/tree/master/en/speechkit)
- [CloudAPI Proto Files](https://github.com/yandex-cloud/cloudapi)
- [Pricing](https://yandex.cloud/en/docs/speechkit/pricing)
- [Python SDK](https://github.com/tikhonp/yandex-speechkit-lib-python)
