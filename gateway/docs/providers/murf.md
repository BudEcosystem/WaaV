# Murf.ai Provider Integration

> **Research Date:** 2026-01-13
> **Status:** Implementation Ready
> **Provider Type:** TTS (Text-to-Speech) + Voice Cloning (Enterprise)

---

## Table of Contents

1. [Overview](#overview)
2. [Capabilities Matrix](#capabilities-matrix)
3. [Technical Specifications](#technical-specifications)
4. [API Endpoints](#api-endpoints)
5. [Authentication](#authentication)
6. [Request/Response Formats](#requestresponse-formats)
7. [Models](#models)
8. [Voice Library](#voice-library)
9. [Speech Customization](#speech-customization)
10. [Pricing](#pricing)
11. [Rate Limits & Concurrency](#rate-limits--concurrency)
12. [Regional Endpoints](#regional-endpoints)
13. [SDK Support](#sdk-support)
14. [Best Practices](#best-practices)
15. [Known Issues & Limitations](#known-issues--limitations)
16. [Implementation Plan](#implementation-plan)
17. [Testing Plan](#testing-plan)

---

## Overview

Murf.ai is a professional AI voice generator platform providing high-quality text-to-speech synthesis with 150+ voices across 35+ languages. The platform offers two primary models:

- **Falcon (Beta)**: Ultra-low latency (~130ms TTFA), optimized for real-time conversational AI
- **Gen2**: Studio-quality output with more customization options

### Key Differentiators

- **Ultra-fast Falcon model**: 55ms model latency, sub-130ms time-to-first-audio
- **High concurrency**: Supports 10,000+ concurrent calls
- **Multi-native locale**: Seamless language switching within sentences
- **Pronunciation accuracy**: 99.37% accuracy for critical applications
- **12 global regions**: Edge deployment for consistent low latency

### Official Resources

- **Website**: https://murf.ai
- **API Documentation**: https://murf.ai/api/docs
- **API Dashboard**: https://murf.ai/api/dashboard
- **Python SDK**: https://github.com/murf-ai/murf-python-sdk
- **PyPI Package**: https://pypi.org/project/murf/
- **Help Center**: https://help.murf.ai

---

## Capabilities Matrix

| Capability | Supported | Notes |
|------------|-----------|-------|
| **TTS** | YES | Primary offering - 150+ voices |
| **STT** | NO | Not available |
| **Audio-to-Audio** | NO | Voice Changer available (transforms recordings) |
| **Voice Cloning** | Enterprise Only | Requires 90 min recordings, 4-week processing |
| **Streaming** | YES | HTTP streaming + WebSocket (Beta) |
| **Real-time** | YES | Falcon model optimized for real-time |
| **SSML** | Partial | Pause tags `[pause <duration>]` supported |
| **Emotions/Styles** | YES | 20+ speaking styles per voice |

---

## Technical Specifications

### Audio Formats Supported

| Format | Description | Use Case |
|--------|-------------|----------|
| **WAV** | Uncompressed, low-latency | High quality, minimal processing |
| **MP3** | Compressed, widely supported | General use, file size optimization |
| **FLAC** | Lossless compression | Archival, high quality with compression |
| **PCM** | Raw audio data | Real-time streaming, custom processing |
| **OGG** | Efficient streaming format | Web streaming |
| **ALAW** | Telephony format (8kHz mono) | Phone systems (G.711) |
| **ULAW** | Telephony format (8kHz mono) | Phone systems (G.711) |

### Sample Rates

- 8000 Hz (telephony)
- 24000 Hz (default for streaming)
- 44100 Hz (CD quality)
- 48000 Hz (professional)

### Channel Types

- **Mono**: Single channel (default)
- **Stereo**: Dual channel

### Authentication

- **Method**: API Key in header
- **Header Name**: `api-key`
- **Format**: `api-key: <your_api_key>`

---

## API Endpoints

### Synthesize Speech (Non-Streaming)

```
POST https://api.murf.ai/v1/speech/generate
```

Returns a JSON response with a downloadable audio URL.

### HTTP Streaming

```
POST https://global.api.murf.ai/v1/speech/stream
```

Returns chunked audio data in real-time.

### WebSocket Streaming (Beta)

```
WSS wss://global.api.murf.ai/v1/speech/stream-input
```

Bidirectional streaming for interactive applications.

### List Voices

```
GET https://api.murf.ai/v1/speech/voices
```

Returns all available voices with metadata.

### Regional Streaming Endpoints

| Region | Endpoint | Default Concurrency |
|--------|----------|---------------------|
| **Global Router** | `https://global.api.murf.ai/v1/speech/stream` | Auto-routes to nearest |
| **US-East** | `https://us-east.api.murf.ai/v1/speech/stream` | 15 |
| **US-West** | `https://us-west.api.murf.ai/v1/speech/stream` | 2 |
| **India** | `https://in.api.murf.ai/v1/speech/stream` | 2 |
| **Canada** | `https://ca.api.murf.ai/v1/speech/stream` | 2 |
| **EU Central** | `https://eu.api.murf.ai/v1/speech/stream` | 2 |
| **UK** | `https://uk.api.murf.ai/v1/speech/stream` | 2 |
| **Japan** | `https://jp.api.murf.ai/v1/speech/stream` | 2 |
| **Australia** | `https://au.api.murf.ai/v1/speech/stream` | 2 |
| **South Korea** | `https://kr.api.murf.ai/v1/speech/stream` | 2 |
| **UAE** | `https://ae.api.murf.ai/v1/speech/stream` | 2 |
| **Brazil** | `https://br.api.murf.ai/v1/speech/stream` | 2 |

---

## Authentication

### API Key Setup

1. Create account at https://murf.ai
2. Navigate to API Dashboard: https://murf.ai/api/dashboard
3. Generate API Key
4. Store securely as environment variable: `MURF_API_KEY`

### Request Headers

```http
Content-Type: application/json
api-key: <your_api_key>
```

### Environment Variable

```bash
export MURF_API_KEY="your_api_key_here"
```

---

## Request/Response Formats

### Synthesize Speech Request (Non-Streaming)

```json
{
  "text": "Hello, this is a test of Murf.ai text to speech.",
  "voiceId": "en-US-natalie",
  "format": "MP3",
  "sampleRate": 44100,
  "channelType": "MONO",
  "modelVersion": "GEN2",
  "rate": 0,
  "pitch": 0,
  "style": "Conversational",
  "encodeAsBase64": false
}
```

### Synthesize Speech Response (Non-Streaming)

```json
{
  "audioFile": "https://api.murf.ai/audio/download/<id>",
  "audioLengthInSeconds": 3.5,
  "status": "success"
}
```

### Streaming Request (HTTP)

```json
{
  "text": "Hello, this is streaming audio.",
  "voiceId": "Matthew",
  "multiNativeLocale": "en-US",
  "model": "FALCON",
  "format": "PCM",
  "sampleRate": 24000
}
```

### Streaming Response

Returns raw audio bytes with `Transfer-Encoding: chunked`

### WebSocket Message Format (Beta)

**Connection**: `wss://global.api.murf.ai/v1/speech/stream-input`

**Send Text**:
```json
{
  "type": "text",
  "text": "Hello world",
  "contextId": "unique-session-id"
}
```

**Receive Audio**: Binary frames containing PCM audio data

---

## Models

### Falcon (Beta)

- **Latency**: ~55ms model latency, sub-130ms TTFA
- **Concurrency**: 10,000+ concurrent calls
- **Use Case**: Real-time conversational AI, voice agents
- **Pricing**: $0.01 per 1,000 characters
- **Languages**: Multi-native locale support
- **Accuracy**: 99.37% pronunciation accuracy

**Supported Parameters**:
- `voiceId` (required)
- `text` (required)
- `multiNativeLocale`
- `format`
- `sampleRate`
- `channelType`

### Gen2

- **Latency**: Higher than Falcon (studio-quality focus)
- **Use Case**: Pre-recorded content, multimedia production
- **Features**: Full customization (pitch, rate, style, pauses, variations)

**Additional Parameters**:
- `rate` (-50 to 50)
- `pitch` (-50 to 50)
- `style`
- `pronunciationDictionary`
- `audioDuration`
- `variation` (0-5)
- Pause tags `[pause <duration>]`

---

## Voice Library

### Voice ID Format

Two formats are supported:
- **Full format**: `en-US-natalie`
- **Short format**: `natalie` (name only)

### Available Languages (13 languages, 18 dialects)

| Language | Locale Codes |
|----------|--------------|
| English | en-US, en-GB, en-AU, en-IN, en-SCO |
| Spanish | es-ES, es-MX |
| French | fr-FR |
| German | de-DE |
| Italian | it-IT |
| Portuguese | pt-BR |
| Hindi | hi-IN |
| Bengali | bn-IN |
| Tamil | ta-IN |
| Dutch | nl-NL |
| Korean | ko-KR |
| Chinese | zh-CN |
| Polish | pl-PL |

### Voice Categories

- **133+ voices** total
- **Young Adult**: 88 voices (39 male, 49 female)
- **20+ speaking styles** per voice

### Retrieving Voice List

```bash
curl -X GET "https://api.murf.ai/v1/speech/voices" \
  -H "api-key: $MURF_API_KEY"
```

---

## Speech Customization

### Rate (Speed)

- **Range**: -50 to 50
- **Default**: 0
- **Effect**: Higher values = faster speech

### Pitch

- **Range**: -50 to 50
- **Default**: 0
- **Effect**: Higher = treble, Lower = bass

### Style

Available styles vary by voice. Common styles include:
- Conversational
- Promotional
- Newscast
- Inspirational
- Sad
- Angry
- Promo

### Pronunciation Dictionary

```json
{
  "pronunciationDictionary": {
    "API": "A P I",
    "Murf": "murf"
  }
}
```

### Pause Tags (Gen2 only)

Insert pauses in text:
```
Hello [pause 0.5s] world [pause 1s] how are you?
```
- **Range**: 0.1s to 5s

### Audio Duration (Gen2 only)

Force output to specific duration:
```json
{
  "audioDuration": 10
}
```

### Variation (Gen2 only)

- **Range**: 0 to 5
- **Default**: 1
- **Effect**: Dynamic delivery with pause, pitch, speed variations

### Multi-Native Locale (Falcon)

Enable natural pronunciation across languages:
```json
{
  "multiNativeLocale": "en-US"
}
```

---

## Pricing

### Pay-As-You-Go (API)

| Feature | Price |
|---------|-------|
| Characters (Falcon) | $0.01 per 1,000 chars |
| Characters (Gen2) | $0.03 per 1,000 chars |
| Minimum Purchase | $2 |

### Studio Plans

| Plan | Price (Monthly) | Features |
|------|-----------------|----------|
| Free | $0 | Limited, no downloads, non-commercial |
| Creator | $29/mo | 120 minutes/month |
| Pro | $39/mo ($26 annual) | All 120+ voices, commercial rights |
| Business | $79-99/mo | Team features |
| Enterprise | Custom | Unlimited, voice cloning, dedicated support |

### Cost Comparison

For 1 million characters:
- **Falcon**: $10
- **Gen2**: $30
- **ElevenLabs**: ~$18-30
- **Play.ht**: ~$20-30

---

## Rate Limits & Concurrency

### Plan Limits

| Plan | API Keys | Concurrency | Rate Limit |
|------|----------|-------------|------------|
| Free Trial | 1 | 5 | 1,000 req/min |
| Pay-As-You-Go | 3 | 15 | 10,000 req/min |
| Enterprise | Custom | Custom | Custom |

### Streaming Concurrency

- HTTP Streaming: Each request = 1 concurrent slot
- WebSocket: Each unique `contextId` = 1 concurrent slot
- WebSocket connections: 10x streaming concurrency limit
- WebSocket timeout: 3 minutes inactivity

### Regional Concurrency

- US-East: 15 concurrent (highest)
- All other regions: 2 concurrent (default)

---

## Regional Endpoints

### Data Residency

For strict data residency requirements, use region-specific endpoints:

```
https://{region}.api.murf.ai/v1/speech/stream
```

**Note**: Do NOT use `global.api.murf.ai` if strict residency is required.

### Latency Optimization

1. Use region closest to your users
2. US-East has highest concurrency (15)
3. Global router auto-selects nearest region
4. Edge deployment ensures consistent latency

---

## SDK Support

### Python SDK

**Installation**:
```bash
pip install murf
```

**Synchronous Example**:
```python
from murf import Murf

client = Murf(api_key="YOUR_API_KEY")

response = client.text_to_speech.generate(
    format="MP3",
    sample_rate=44100,
    text="Hello, world!",
    voice_id="en-US-natalie",
)
```

**Async Example**:
```python
import asyncio
from murf import AsyncMurf

client = AsyncMurf(api_key="YOUR_API_KEY")

async def main():
    response = await client.text_to_speech.generate(
        format="MP3",
        sample_rate=44100,
        text="Hello, world!",
        voice_id="en-US-natalie",
    )

asyncio.run(main())
```

**Streaming Example**:
```python
from murf import Murf

client = Murf(api_key="YOUR_API_KEY")

# Stream audio chunks
for chunk in client.text_to_speech.stream(
    text="Hello, streaming world!",
    voice_id="en-US-natalie",
    model="FALCON",
    format="PCM",
    sample_rate=24000,
):
    # Process chunk (raw audio bytes)
    process_audio(chunk)
```

**Error Handling**:
```python
from murf.core.api_error import ApiError

try:
    response = client.text_to_speech.generate(...)
except ApiError as e:
    print(f"Status: {e.status_code}")
    print(f"Error: {e.body}")
```

### JavaScript/TypeScript SDK

**Not officially available**. Use REST API directly with `fetch` or `axios`.

---

## Best Practices

### Performance Optimization

1. **Use Falcon model** for real-time applications
2. **Select nearest region** for lowest latency
3. **Use PCM format** for streaming (lowest processing overhead)
4. **Choose 24000 Hz sample rate** for streaming (good balance)
5. **Use connection pooling** for high-volume requests

### Audio Quality

1. **Use Gen2 model** for pre-recorded content
2. **Use WAV or FLAC** for highest quality
3. **Use 44100 Hz or 48000 Hz** for professional output
4. **Apply style customization** for appropriate context

### Cost Optimization

1. **Use Falcon** ($0.01/1K chars) vs Gen2 ($0.03/1K chars)
2. **Cache frequently used audio** to avoid re-generation
3. **Batch text processing** when real-time not required
4. **Monitor character usage** via dashboard

### Error Handling

1. Implement exponential backoff for 429 (rate limit) errors
2. Handle 5XX errors with retry logic
3. Set appropriate timeouts (default: 60s)
4. Validate voice IDs before requests

### Security

1. Store API keys in environment variables
2. Never expose keys in client-side code
3. Use `encodeAsBase64: true` for zero data retention
4. Use region-specific endpoints for data residency compliance

---

## Known Issues & Limitations

### Current Limitations

1. **Voice Cloning**: Not self-service, requires Enterprise contact
2. **SSML**: Limited support (only pause tags)
3. **JavaScript SDK**: Not officially available
4. **WebSocket**: Still in Beta, may have stability issues
5. **Regional Concurrency**: Non-US-East regions limited to 2 concurrent

### Audio Format Restrictions

1. **ALAW/ULAW**: Only 8000 Hz mono
2. **Base64 encoding**: Increases payload size by ~33%

### Rate Limit Considerations

1. Free tier very limited (5 concurrent, 1K req/min)
2. Contact sales for higher limits
3. WebSocket connections timeout after 3 minutes idle

---

## Implementation Plan

### Integration Approach: Dynamic Plugin

Based on `how_to_add_new_provider.md`, implement as a **core TTS provider** (not dynamic plugin) since:
1. HTTP streaming is the primary interface
2. No special dependencies required
3. Follows existing Play.ht implementation pattern

### File Structure

```
src/core/tts/murf/
├── mod.rs          # Module exports, constants
├── config.rs       # MurfTtsConfig, MurfModel, MurfAudioFormat
├── messages.rs     # API request/response types, voice types
├── provider.rs     # MurfTts implementing BaseTTS trait
└── tests.rs        # Unit tests (inline or separate)
```

### Implementation Steps

1. **Create config.rs**
   - `MurfTtsConfig` struct
   - `MurfModel` enum (Falcon, Gen2)
   - `MurfAudioFormat` enum (MP3, WAV, PCM, etc.)
   - `MurfRegion` enum for regional endpoints
   - Default values and validation

2. **Create messages.rs**
   - `MurfTtsRequest` for API requests
   - `MurfTtsResponse` for non-streaming response
   - `MurfVoice` for voice metadata
   - `MurfStyle` enum for speaking styles
   - Error types

3. **Create provider.rs**
   - `MurfTts` struct implementing `BaseTTS`
   - HTTP streaming implementation using `reqwest`
   - Request builder pattern
   - Audio callback handling
   - Connection pooling via `reqwest::Client`

4. **Create mod.rs**
   - Module exports
   - API URL constants
   - Provider aliases

5. **Register with factory**
   - Add to `src/core/tts/mod.rs`
   - Add aliases: `murf`, `murf.ai`, `murf-ai`, `murf_ai`
   - Add to supported providers list

6. **Add environment variable**
   - `MURF_API_KEY` in config loading

### Key Design Decisions

1. **Use HTTP Streaming** (not WebSocket) for initial implementation
2. **Default to Falcon model** for lowest latency
3. **Support both models** via config parameter
4. **Implement regional endpoint selection**
5. **Follow Play.ht implementation pattern** for consistency

---

## Testing Plan

### Unit Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_defaults() {
        let config = MurfTtsConfig::default();
        assert_eq!(config.model, MurfModel::Falcon);
        assert_eq!(config.sample_rate, 24000);
        assert_eq!(config.format, MurfAudioFormat::Pcm);
    }

    #[test]
    fn test_config_from_base() {
        let base = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: "en-US-natalie".to_string(),
            ..Default::default()
        };
        let config = MurfTtsConfig::from_base(&base);
        assert_eq!(config.voice_id, "en-US-natalie");
    }

    #[test]
    fn test_requires_api_key() {
        let config = TTSConfig::default();
        let result = MurfTts::new(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_request_serialization() {
        let request = MurfTtsRequest {
            text: "Hello".to_string(),
            voice_id: "en-US-natalie".to_string(),
            model: MurfModel::Falcon,
            format: MurfAudioFormat::Pcm,
            sample_rate: 24000,
            ..Default::default()
        };
        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"voiceId\""));
        assert!(json.contains("\"model\":\"FALCON\""));
    }

    #[test]
    fn test_model_enum_serialization() {
        assert_eq!(serde_json::to_string(&MurfModel::Falcon).unwrap(), "\"FALCON\"");
        assert_eq!(serde_json::to_string(&MurfModel::Gen2).unwrap(), "\"GEN2\"");
    }

    #[test]
    fn test_regional_endpoint_selection() {
        assert_eq!(
            MurfRegion::UsEast.streaming_url(),
            "https://us-east.api.murf.ai/v1/speech/stream"
        );
        assert_eq!(
            MurfRegion::Global.streaming_url(),
            "https://global.api.murf.ai/v1/speech/stream"
        );
    }

    #[test]
    fn test_voice_id_formats() {
        // Both formats should be accepted
        assert!(MurfTtsConfig::validate_voice_id("en-US-natalie").is_ok());
        assert!(MurfTtsConfig::validate_voice_id("natalie").is_ok());
    }
}
```

### Integration Tests

```rust
// tests/murf_integration.rs

fn get_api_key() -> Option<String> {
    std::env::var("MURF_API_KEY").ok()
}

#[tokio::test]
#[ignore]
async fn test_murf_connection() {
    let Some(api_key) = get_api_key() else {
        println!("Skipping: MURF_API_KEY not set");
        return;
    };

    let config = TTSConfig {
        api_key,
        voice_id: "en-US-natalie".to_string(),
        ..Default::default()
    };

    let mut tts = MurfTts::new(config).unwrap();
    assert!(!tts.is_ready());

    tts.connect().await.unwrap();
    assert!(tts.is_ready());

    tts.disconnect().await.unwrap();
    assert!(!tts.is_ready());
}

#[tokio::test]
#[ignore]
async fn test_murf_streaming_tts() {
    let Some(api_key) = get_api_key() else {
        println!("Skipping: MURF_API_KEY not set");
        return;
    };

    let config = TTSConfig {
        api_key,
        voice_id: "en-US-natalie".to_string(),
        ..Default::default()
    };

    let mut tts = MurfTts::new(config).unwrap();
    tts.connect().await.unwrap();

    let received_audio = Arc::new(AtomicBool::new(false));
    let received_clone = received_audio.clone();

    tts.on_audio(Arc::new(move |audio: Bytes, _| {
        assert!(!audio.is_empty());
        received_clone.store(true, Ordering::SeqCst);
    })).unwrap();

    tts.speak("Hello from Murf AI", true).await.unwrap();

    // Wait for audio
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(received_audio.load(Ordering::SeqCst));
}

#[tokio::test]
#[ignore]
async fn test_murf_gen2_with_customization() {
    let Some(api_key) = get_api_key() else {
        println!("Skipping: MURF_API_KEY not set");
        return;
    };

    let config = TTSConfig {
        api_key,
        voice_id: "en-US-natalie".to_string(),
        provider_options: Some(serde_json::json!({
            "model": "GEN2",
            "rate": 10,
            "pitch": -5,
            "style": "Conversational"
        })),
        ..Default::default()
    };

    let mut tts = MurfTts::new(config).unwrap();
    tts.connect().await.unwrap();

    let received = Arc::new(AtomicBool::new(false));
    let received_clone = received.clone();

    tts.on_audio(Arc::new(move |_, _| {
        received_clone.store(true, Ordering::SeqCst);
    })).unwrap();

    tts.speak("Testing Gen2 with customization", true).await.unwrap();

    tokio::time::sleep(Duration::from_secs(5)).await;
    assert!(received.load(Ordering::SeqCst));
}

#[tokio::test]
#[ignore]
async fn test_murf_list_voices() {
    let Some(api_key) = get_api_key() else {
        println!("Skipping: MURF_API_KEY not set");
        return;
    };

    let voices = MurfTts::list_voices(&api_key).await.unwrap();
    assert!(!voices.is_empty());

    // Check for known voice
    let natalie = voices.iter().find(|v| v.voice_id.contains("natalie"));
    assert!(natalie.is_some());
}
```

### Manual Testing Commands

```bash
# Run unit tests
cargo test murf --lib

# Run integration tests (with API key)
MURF_API_KEY=xxx cargo test murf -- --ignored --nocapture

# Test via WebSocket client
wscat -c "ws://localhost:3000/ws"
# Send: {"type":"config","tts_config":{"provider":"murf","voice_id":"en-US-natalie"}}
# Send: {"type":"speak","text":"Hello from Murf"}
```

---

## References

- [Murf API Documentation](https://murf.ai/api/docs)
- [Murf API Quickstart](https://murf.ai/api/docs/introduction/quickstart)
- [Murf Python SDK](https://github.com/murf-ai/murf-python-sdk)
- [Murf Falcon Model](https://murf.ai/api/docs/text-to-speech-models/falcon)
- [Murf Streaming Documentation](https://murf.ai/api/docs/text-to-speech/streaming)
- [Murf Speech Customization](https://murf.ai/api/docs/text-to-speech/speech-customization)
- [Murf Rate Limits](https://murf.ai/api/docs/resources/rate-limits)
- [Murf Data Residency](https://murf.ai/api/docs/text-to-speech/data-residency)
- [Murf Help Center](https://help.murf.ai)
