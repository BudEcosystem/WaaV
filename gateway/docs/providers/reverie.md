# Reverie Language Technologies STT/TTS Integration

## Overview

Reverie Language Technologies provides Speech-to-Text (STT) and Text-to-Speech (TTS) APIs optimized for Indian languages. The platform supports 22 official Indian languages with dialect-agnostic recognition and bi-lingual (code-mixed) speech handling.

**Website:** https://reverieinc.com
**API Documentation:** https://docs.reverieinc.com
**Developer Portal:** https://revup.reverieinc.com (API key signup)

## Supported Features

### Core Capabilities
- **Real-time Streaming STT** via WebSocket
- **File-based STT** (up to 5 minutes)
- **Batch STT** (long-form audio processing)
- **Text-to-Speech** with 36+ voices
- **SSML Support** for TTS
- **Punctuation & Capitalization** (en, hi)

### Supported Languages (22 Indian Languages + English)
| Code | Language | STT | TTS |
|------|----------|-----|-----|
| `hi` | Hindi | Yes | Yes |
| `en` | English | Yes | Yes |
| `ta` | Tamil | Yes | Yes |
| `te` | Telugu | Yes | Yes |
| `bn` | Bengali | Yes | Yes |
| `mr` | Marathi | Yes | Yes |
| `gu` | Gujarati | Yes | Yes |
| `kn` | Kannada | Yes | Yes |
| `ml` | Malayalam | Yes | Yes |
| `pa` | Punjabi | Yes | Yes |
| `or` | Odia (Oriya) | Yes | Yes |
| `as` | Assamese | Yes | Yes |
| `ur` | Urdu | Yes | Yes |
| `ks` | Kashmiri | Yes | - |
| `sd` | Sindhi | Yes | - |
| `ne` | Nepali | Yes | - |
| `sa` | Sanskrit | Yes | - |
| `kok` | Konkani | Yes | - |
| `mni` | Manipuri | Yes | - |
| `brx` | Bodo | Yes | - |
| `sat` | Santhali | Yes | - |
| `mai` | Maithili | Yes | - |
| `doi` | Dogri | Yes | - |

## Streaming STT API Specification

### WebSocket Endpoint
```
wss://revapi.reverieinc.com/stream
```

### Authentication
Credentials passed as query parameters:
```
wss://revapi.reverieinc.com/stream?appname=stt_stream&apikey=<REV_API_KEY>&appid=<REV_APP_ID>&...
```

### Query Parameters

| Parameter | Required | Default | Description |
|-----------|----------|---------|-------------|
| `appname` | Yes | `stt_stream` | Fixed value for streaming STT |
| `apikey` | Yes | - | Reverie API key |
| `appid` | Yes | - | Reverie Application ID |
| `src_lang` | Yes | - | Source language code (e.g., `hi`, `en`) |
| `domain` | No | `generic` | Domain for vocabulary optimization |
| `format` | No | `16k_int16` | Audio format specification |
| `timeout` | No | `15` | Connection timeout (max 180s) |
| `silence` | No | `1` | Silence detection timeout (max 30s) |
| `logging` | No | `true` | Logging mode: `true`, `no_audio`, `no_transcript`, `false` |
| `punctuate` | No | `true` | Enable punctuation (en, hi only) |
| `continuous` | No | `0` | Continue after silence: `0`/`false` or `1`/`true` |

### Audio Formats

| Format Code | Description |
|-------------|-------------|
| `16k_int16` | Signed 16-bit PCM, 16kHz (default) |
| `16k_uint8` | Unsigned 8-bit PCM, 16kHz |
| `8k_int16` | Signed 16-bit PCM, 8kHz |
| `8k_uint8` | Unsigned 8-bit PCM, 8kHz |
| `opus_16k` | Opus encoded, 16kHz |
| `opus_8k` | Opus encoded, 8kHz |
| `ogg_opus` | Opus in Ogg container |
| `16k_ulaw` | µ-Law encoded, 16kHz |
| `8k_ulaw` | µ-Law encoded, 8kHz |

### Message Protocol

#### Client → Server

**Binary Messages:** Audio data chunks (recommended 1024+ bytes per chunk)

**End-of-Stream:** Send `--EOF--` as binary message to signal end of audio

#### Server → Client

**Partial/Final Response:**
```json
{
  "id": "unique-session-id",
  "text": "transcribed text",
  "final": false,
  "cause": null,
  "success": true,
  "confidence": "0.95",
  "display_text": "Transcribed Text"
}
```

**Final Response (connection close):**
```json
{
  "id": "unique-session-id",
  "text": "final transcription",
  "final": true,
  "cause": "EOF received",
  "success": true,
  "confidence": "0.92",
  "display_text": "Final Transcription"
}
```

**Cause Values:**
- `timeout` - Connection timeout reached
- `silence detected` - Silence timeout triggered
- `EOF received` - Client sent EOF marker

### Limits
- **Max Audio Duration:** 180 seconds (3 minutes) per stream
- **Default Timeout:** 15 seconds
- **Max Silence:** 30 seconds

## TTS API Specification

### REST Endpoint
```
POST https://revapi.reverieinc.com/
```

### Authentication Headers
```
REV-API-KEY: <your_api_key>
REV-APP-ID: <your_app_id>
REV-APPNAME: tts
speaker: <speaker_code>
Content-Type: application/json
```

### Request Body
```json
{
  "text": ["Text to synthesize", "Or multiple strings"],
  "speed": 1.0,
  "pitch": 0,
  "sample_rate": 22050,
  "format": "WAV"
}
```

Or with SSML:
```json
{
  "ssml": "<speak>SSML content here</speak>",
  "speed": 1.0,
  "pitch": 0,
  "sample_rate": 22050,
  "format": "WAV"
}
```

### Request Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `text` | string/array | - | Plain text input (required if no ssml) |
| `ssml` | string | - | SSML input (takes precedence over text) |
| `speed` | float | `1.0` | Speech rate: 0.5 (slow) to 1.5 (fast) |
| `pitch` | float | `0` | Pitch adjustment: -3 to +3 semitones |
| `sample_rate` | int | `22050` | Output sample rate in Hz |
| `format` | string | `WAV` | Output format: `WAV`, `MP3` |

### Supported Sample Rates
- 8000 Hz
- 11025 Hz
- 16000 Hz
- 22050 Hz (default)
- 24000 Hz
- 44100 Hz
- 48000 Hz

### Available Voices (36+ speakers)

**Hindi:**
- `hi_male`, `hi_male_2`, `hi_male_3`, `hi_male_4`
- `hi_female`, `hi_female_2`, `hi_female_3`

**English:**
- `en_male`, `en_male_2`
- `en_female`, `en_female_2`

**Regional Languages:** (pattern: `{lang}_male`, `{lang}_female`)
- Bengali: `bn_male`, `bn_female`
- Kannada: `kn_male`, `kn_female`
- Malayalam: `ml_male`, `ml_female`
- Tamil: `ta_male`, `ta_female`
- Telugu: `te_male`, `te_female`
- Gujarati: `gu_male`, `gu_female`
- Odia: `or_male`, `or_female`
- Assamese: `as_male`, `as_female`
- Marathi: `mr_male`, `mr_female`
- Punjabi: `pa_male`, `pa_female`

### Response
- **Success (200):** Binary audio data in requested format
- **Error (400/403):** JSON with error details

## Pricing

| Service | Price |
|---------|-------|
| STT Streaming | Contact sales |
| STT File/Batch | Contact sales |
| TTS | Contact sales |
| Free Trial | Available via RevUp portal |

## Implementation Plan

### Architecture

Reverie will be implemented as a Dynamic Plugin following the existing STT/TTS provider pattern:

```
src/core/stt/reverie/
├── mod.rs           # Module exports, constants
├── config.rs        # ReverieSTTConfig struct
├── messages.rs      # WebSocket message types
└── client.rs        # ReverieSTT implementing BaseSTT trait

src/core/tts/reverie/
├── mod.rs           # Module exports, constants
├── config.rs        # ReverieTTSConfig struct
└── client.rs        # ReverieTTS implementing TTSProvider trait
```

### STT Configuration Types

```rust
// Language codes
pub enum ReverieLanguage {
    Hindi,      // hi
    English,    // en
    Tamil,      // ta
    Telugu,     // te
    Bengali,    // bn
    Marathi,    // mr
    Gujarati,   // gu
    Kannada,    // kn
    Malayalam,  // ml
    Punjabi,    // pa
    Odia,       // or
    Assamese,   // as
    Urdu,       // ur
    // ... other languages
}

// Audio format
pub enum ReverieAudioFormat {
    Pcm16kInt16,    // 16k_int16 (default)
    Pcm16kUint8,    // 16k_uint8
    Pcm8kInt16,     // 8k_int16
    Pcm8kUint8,     // 8k_uint8
    Opus16k,        // opus_16k
    Opus8k,         // opus_8k
    OggOpus,        // ogg_opus
    Ulaw16k,        // 16k_ulaw
    Ulaw8k,         // 8k_ulaw
}

// Logging mode
pub enum ReverieLogging {
    True,           // Store audio and transcripts
    NoAudio,        // No audio, keep transcripts
    NoTranscript,   // Keep audio, no transcripts
    False,          // No logging
}

// Configuration
pub struct ReverieSTTConfig {
    pub api_key: String,
    pub app_id: String,
    pub language: ReverieLanguage,
    pub domain: String,
    pub format: ReverieAudioFormat,
    pub timeout: u32,
    pub silence: u32,
    pub logging: ReverieLogging,
    pub punctuate: bool,
    pub continuous: bool,
}
```

### TTS Configuration Types

```rust
// Speaker voice
pub struct ReverieSpeaker {
    language: ReverieLanguage,
    gender: Gender,
    variant: u8,
}

// TTS Audio format
pub enum ReverieTTSFormat {
    Wav,
    Mp3,
}

// Configuration
pub struct ReverieTTSConfig {
    pub api_key: String,
    pub app_id: String,
    pub speaker: ReverieSpeaker,
    pub speed: f32,          // 0.5 - 1.5
    pub pitch: f32,          // -3 to +3
    pub sample_rate: u32,    // 8000, 11025, 16000, 22050, 24000, 44100, 48000
    pub format: ReverieTTSFormat,
}
```

### Connection Flow (STT)

1. Build WebSocket URL with query parameters
2. Connect via WebSocket handshake
3. Send binary audio chunks
4. Receive partial/final JSON responses
5. Send `--EOF--` binary message to end stream
6. Receive final response and close

### Best Practices

1. **Audio Chunks:** Send 1024+ byte chunks for optimal performance
2. **Sample Rate:** Use 16kHz for best quality
3. **Timeout:** Set appropriate timeout based on expected audio length
4. **Silence Detection:** Configure based on use case (IVR vs dictation)
5. **Continuous Mode:** Enable for long-form transcription
6. **Error Handling:** Handle connection drops gracefully

## Test Plan

### Unit Tests
1. Configuration validation (empty API key, invalid language)
2. Audio format string generation
3. URL construction with parameters
4. Message parsing (partial, final, error)
5. TTS speaker code generation

### Integration Tests
1. WebSocket connection lifecycle
2. Audio streaming with real API
3. Partial/final hypothesis handling
4. Error code handling
5. EOF flow
6. TTS audio generation
7. SSML handling

### Edge Cases
1. Empty audio
2. Very long audio (approach 180s limit)
3. Multiple languages
4. Silence detection
5. Connection timeout
6. Invalid credentials

## Environment Variables

```bash
REVERIE_API_KEY=your_api_key
REVERIE_APP_ID=your_app_id
```

## References

- [Streaming STT API](https://docs.reverieinc.com/reference/speech-to-text-streaming-api)
- [File STT API](https://docs.reverieinc.com/speech-to-text-file-api)
- [Batch STT API](https://docs.reverieinc.com/speech-to-text-batch-api)
- [TTS API](https://docs.reverieinc.com/endpoints/text-to-speech)
- [Language Codes](https://docs.reverieinc.com/reference/speech-to-text-streaming-api/language-codes)
- [Audio Formats](https://docs.reverieinc.com/usage-guides/speech-to-text/supported-audio-formats)
- [Speaker Codes](https://docs.reverieinc.com/reference/text-to-speech-api/supporting-speaker-code)
- [Python SDK](https://github.com/reverieinc/python-sdk)
