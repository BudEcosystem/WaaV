# Cartesia API Research Summary

**Date:** January 17, 2026
**Research Focus:** Cartesia Sonic (TTS) and Ink-Whisper (STT) Official APIs

---

## Part 1: Ink-Whisper STT (Speech-to-Text)

### Overview
**Ink-Whisper** is Cartesia's fastest, most affordable speech-to-text model engineered for enterprise deployment in production-grade voice agents. It delivers higher accuracy than baseline Whisper with optimized real-time performance across diverse real-world conditions.

### Available Models

| Model ID | Release Date | Status | Recommendation |
|----------|--------------|--------|-----------------|
| `ink-whisper` | Latest | Stable (Auto-routes to latest) | Prototyping & Development |
| `ink-whisper-2025-06-10` | June 10, 2025 | Stable Snapshot | Production Deployments |
| `ink-whisper-2025-06-04` | June 4, 2025 | Stable Snapshot | Consistency in prod |

### Key Features

- **Dynamic Chunking**: Handles variable-length audio chunks and interruptions gracefully
- **Noise Robustness**: Reliably transcribes speech with background noise
- **Real-world Accuracy**: Handles telephony artifacts, accents, disfluencies
- **Specialized Terminology**: Excels at proper nouns and domain-specific terms
- **Word-Level Timestamps**: Precise timing data for each transcribed word
- **Language Auto-detection**: 99+ languages supported
- **Voice Activity Detection (VAD)**: Configurable volume thresholds and silence detection

### Supported Languages (99+)

**Primary**: English, Mandarin, German, Spanish, Russian, Korean, French, Japanese, Portuguese, Turkish, Polish

**Additional**: Arabic, Swedish, Italian, Hindi, Finnish, Vietnamese, Hebrew, Ukrainian, Greek, Czech, Romanian, Bulgarian, Danish, Norwegian, Hungarian, Thai, Vietnamese, Bengali, Ukrainian, Tamil, Indonesian, Telugu, Gujarati, Kannada, Malayalam, Marathi, Punjabi, and 70+ more

### WebSocket API Details

**Endpoint:** `wss://api.cartesia.ai/stt/websocket`

**Required Query Parameters:**
- `model` – Model ID (e.g., `ink-whisper`)
- `language` – ISO-639-1 format language code (default: `en`)
- `encoding` – Audio format (recommended: `pcm_s16le`)
- `sample_rate` – Sample rate in Hz (recommended: `16000`)
- `api_key` – Authentication key

**Voice Activity Detection (VAD) Parameters:**
- `min_volume` – Volume threshold (0.0–1.0 range, e.g., 0.1 or 0.15)
- `max_silence_duration_secs` – Silence duration before endpointing (seconds, e.g., 0.4 or 2.0)

**Client → Server (Send):**
- Binary audio data in specified encoding/sample rate
- `finalize` command – Flush remaining audio, receive `flush_done` response
- `done` command – Close session, receive `done` confirmation

**Server → Client (Receive):**
| Message Type | Content |
|--------------|---------|
| `transcript` | Transcription results with word-level timing data |
| `flush_done` | Acknowledgment of finalize command |
| `done` | Session closure confirmation |
| `error` | Error details with status information |

**Timestamp Structure (in transcript messages):**
```
word: {
  text: "word_content",
  start: 0.123,  // Start time in seconds
  end: 0.456     // End time in seconds
}
```

### Performance Characteristics

- **Chunking:** Send audio in small chunks (e.g., 100ms intervals) for optimal latency
- **Session Timeout:** WebSocket auto-disconnects after 3 minutes of inactivity
- **Pricing:** 1 credit per 1 second of streamed audio
- **Latency:** Optimized for real-time conversational AI

### Important Implementation Notes

- Audio format must match specified encoding and sample rate
- Support for continuous streaming with dynamic endpointing
- VAD parameters control when speech is considered complete
- Word timestamps enable UI synchronization and real-time captioning

**Official Documentation:** [Cartesia STT API Reference](https://docs.cartesia.ai/api-reference/stt/stt)

---

## Part 2: Sonic TTS (Text-to-Speech)

### Overview
**Sonic-3** is Cartesia's latest, fastest, and most emotive text-to-speech model. It delivers ultra-realistic speech with 135ms model latency, featuring fine-grained controls for volume, speed, and emotion plus automatic laughter generation. Designed for real-time AI agents and interactive applications.

### Available Models

| Model ID | Release Date | Status | Use Case |
|----------|--------------|--------|----------|
| `sonic-3` (base) | Latest | Stable | Auto-routes to latest stable snapshot |
| `sonic-3-2026-01-12` | January 12, 2026 | Stable | Current production snapshot |
| `sonic-3-2025-10-27` | October 27, 2025 | Stable | Previous stable version |
| `sonic-3-latest` | Ongoing | Beta | Feature testing (non-production) |

### Key Features

- **High Naturalness** – Ultra-realistic voice output
- **Accurate Transcript Following** – Precise adherence to input text
- **Industry-Leading Latency** – 135ms model latency
- **Volume Control** – Fine-grained volume adjustment via API/SSML
- **Speed Control** – Adjustable playback speed via API/SSML
- **Emotion Control** – 60+ emotional tones (excited, calm, neutral, sad, friendly, professional, etc.)
- **Laughter Generation** – Insert laughter using `[laughter]` tags
- **SSML Support** – Speech Synthesis Markup Language for advanced control
- **Voice Cloning** – Instant voice cloning from as little as 3 seconds of audio
- **Voice Mixing** – Combine multiple voices
- **Voice Design** – Create custom voices with controlled characteristics

### Supported Languages (42+)

**Primary**: English, French, German, Spanish, Portuguese, Mandarin, Japanese, Hindi, Italian, Korean

**Additional**: Dutch, Polish, Russian, Swedish, Turkish, Tagalog, Bulgarian, Romanian, Arabic, Czech, Greek, Finnish, Croatian, Malay, Slovak, Danish, Tamil, Ukrainian, Hungarian, Norwegian, Vietnamese, Bengali, Thai, Hebrew, Georgian, Indonesian, Telugu, Gujarati, Kannada, Malayalam, Marathi, Punjabi

**Multilingual Support:** Voices are fully multilingual when used with `sonic-3` and automatically adapt to input text language.

### Voice Selection

**Recommended Voices for Different Use Cases:**

| Use Case | Voice Names | Voice IDs | Characteristics |
|----------|------------|-----------|-----------------|
| Voice Agents (Stable) | Katie | `f786b574-daa5-4673-aa0c-cbe3e8534c02` | Stable, realistic, neutral |
| Voice Agents (Stable) | Kiefer | `228fca29-3a0a-435c-8728-5cb483251068` | Stable, realistic, neutral |
| Expressive Characters | Tessa | `6ccbfb76-1fc6-48f7-b71d-91ac6298247b` | Emotive, expressive |
| Expressive Characters | Kyle | `c961b81c-a935-4c17-bfb3-ba2239de8c2f` | Emotive, expressive |

**Note:** Full voice catalog available through Cartesia Playground at https://cartesia.ai/voices

### WebSocket API Details

**Endpoint:** `wss://api.cartesia.ai/tts/websocket`

**Required Query Parameters:**
- `cartesia_version` – API version (`2024-06-10`, `2024-11-13`, or `2025-04-16`)
- `api_key` – Authentication key (query parameter for browser WebSocket compatibility)

**Generation Request Parameters:**

**Core Settings:**
- `model_id` – Model identifier (e.g., `sonic-3`)
- `transcript` – Text to synthesize into audio
- `language` – Language code (e.g., `en`)
- `context_id` – Unique identifier for grouping related generations

**Voice Selection:**
- `voice.mode` – Set to `"id"` for voice selection
- `voice.id` – UUID of desired voice

**Audio Output Format:**
- `output_format.container` – `"raw"` for streaming audio
- `output_format.encoding` – Audio codec:
  - `pcm_s16le` – 16-bit signed PCM (standard)
  - `pcm_f32le` – 32-bit float PCM
  - Other formats via REST endpoint: WAV, MP3
- `output_format.sample_rate` – Sample rate in Hz (common: 8000, 16000, 24000, 44100)

**Control Parameters:**
- `add_timestamps` – Boolean, include word-level timing metadata
- `continue` – Boolean, for continuation handling in streaming

**Emotion & Expression Control:**
- `generation_config.emotion` – Emotion specification (via SSML: `<emotion value='excited'/>`)
- `generation_config.speed` – Speed ratio (via SSML: `<speed ratio="1.05"/>`)
- `generation_config.volume` – Volume level

### Response Message Types

| Response Type | Content |
|---------------|---------|
| Audio Chunk | Base64-encoded audio data, status 206 (partial), step_time metrics |
| Timestamps | Word-level synchronization data with start/end times |
| Flush Done | Acknowledgment of flush command with flush ID tracking |
| Done | Generation completion signal |
| Error | Error details with status codes and context ID |

### SSML Support

Sonic-3 supports Speech Synthesis Markup Language for fine-grained control:

```xml
<emotion value="excited">This is exciting!</emotion>
<speed ratio="1.05">Speak slightly faster</speed>
<volume>Control volume level</volume>
[laughter] Insert automatic laughter
```

### Voice Cloning Features

- **Minimum Audio:** 3 seconds of audio sufficient
- **Quality:** Highly similar and lifelike output
- **Preservation:** Retains unique speaking style, accent, background, emotion, vocal characteristics
- **Output:** Voice sounds identical to original speaker
- **API Method:** `client.voices.clone()` in Python SDK

### Audio Encoding Options

**WebSocket Streaming:**
- `pcm_s16le` – Primary format for WebSocket
- `pcm_f32le` – Alternative format
- Raw PCM with configurable sample rates

**REST/File Endpoint:**
- WAV (RIFF container)
- MP3 (compressed)
- PCM (raw)

**Recommended for Real-time:** Use `pcm_s16le` at 16000 Hz for optimal latency

### Performance Characteristics

- **Model Latency:** 135ms (Sonic-3)
- **Streaming:** First audio chunk arrives in ~135ms
- **Continuous Streaming:** Chunks stream as generation progresses
- **Languages:** Automatic language detection with multilingual voices

### Implementation Guidance

**For Voice Agents:**
- Use stable voices (Katie, Kiefer)
- Keep emotions neutral/professional
- Adjust speed and volume for clarity

**For Interactive/Entertainment:**
- Use emotive voices (Tessa, Kyle)
- Leverage emotion controls
- Include laughter for personality

**For Real-time Performance:**
- Use `pcm_s16le` encoding
- Stream audio in chunks
- Use connection pooling for multiple requests
- Set appropriate sample rates (16000 Hz minimum recommended)

**Official Documentation:** [Cartesia TTS API Reference](https://docs.cartesia.ai/api-reference/tts/tts)

---

## Part 3: Integration Architecture

### API Version Management

Both APIs support versioning via query parameters:
- `cartesia_version` (TTS) or implicit in model selection (STT)
- Versions: `2024-06-10`, `2024-11-13`, `2025-04-16`
- Allows backward compatibility and gradual upgrades

### Authentication

- **Method:** API key authentication
- **Delivery:** Query parameter (WebSocket compatible) or HTTP header
- **Endpoint:** Generate short-lived tokens via `client.auth.access_token()`

### Python SDK Methods

**TTS Operations:**
- `client.tts.bytes()` – Synchronous audio generation
- `client.tts.sse()` – Server-Sent Events streaming
- `client.voices.clone()` – Voice cloning

**STT Operations:**
- `client.stt.transcribe()` – Audio transcription with timestamps

**Utilities:**
- `client.voices.list()` – List available voices
- `client.api_status.get()` – Check API status
- `client.auth.access_token()` – Generate auth tokens

### Zero-Copy Data Flow Recommendations

For the Bud Waav gateway integration:

1. **Audio Buffer Management:**
   - Use shared memory rings for IPC between Rust gateway and Python inference
   - Avoid serialization of audio payloads

2. **WebSocket Streaming:**
   - Stream audio chunks directly from gateway to Cartesia
   - Minimize copying between buffers

3. **Response Handling:**
   - Parse timestamps in-stream without materializing full responses
   - Use async processing for non-blocking I/O

---

## Part 4: Key API Limitations & Constraints

### STT Constraints
- **Session Timeout:** 3 minutes of inactivity
- **Language Requirement:** Must specify language code (no auto-detection in API)
- **VAD:** Volume threshold range 0.0–1.0 (tuning required per environment)
- **Pricing:** 1 credit per 1 second of audio

### TTS Constraints
- **Context ID:** Required for tracking generations
- **Model Versioning:** Use date-versioned models in production for consistency
- **Encoding Support:** WebSocket limited to PCM; compressed formats via REST endpoint
- **Voice Requirements:** Voice ID must be valid UUID or pre-defined voice ID

---

## Official Documentation Links

**Primary Documentation:** [Cartesia Docs](https://docs.cartesia.ai/)

**STT Specific:**
- [STT Models Overview](https://docs.cartesia.ai/build-with-cartesia/stt-models)
- [STT API Reference](https://docs.cartesia.ai/api-reference/stt/stt)

**TTS Specific:**
- [Sonic-3 Models](https://docs.cartesia.ai/build-with-cartesia/tts-models/latest)
- [TTS API Reference](https://docs.cartesia.ai/api-reference/tts/tts)
- [Sonic-3 Product Page](https://cartesia.ai/sonic)

**SDKs & Tools:**
- [Python SDK](https://github.com/cartesia-ai/cartesia-python)
- [JavaScript SDK](https://github.com/cartesia-ai/cartesia-js)
- [PyPI Package](https://pypi.org/project/cartesia/)

**Integration Examples:**
- [LiveKit Integration](https://docs.livekit.io/agents/models/tts/plugins/cartesia/)
- [Voice Agent Example](https://github.com/cartesia-ai/cartesia-livekit-voice-agent)

---

## Summary Table: Quick Reference

| Feature | STT (Ink-Whisper) | TTS (Sonic-3) |
|---------|-------------------|---------------|
| **Endpoint** | `wss://api.cartesia.ai/stt/websocket` | `wss://api.cartesia.ai/tts/websocket` |
| **Primary Model** | `ink-whisper` | `sonic-3` |
| **Language Support** | 99+ languages | 42+ languages |
| **Streaming** | Yes (WebSocket) | Yes (WebSocket) |
| **Timestamps** | Word-level (start/end) | Word-level (optional) |
| **Latency** | Optimized for real-time | 135ms model latency |
| **Voice Selection** | N/A | Voice UUID required |
| **Voice Cloning** | N/A | From 3 seconds of audio |
| **Controls** | VAD (min_volume, silence) | Speed, volume, emotion, laughter |
| **Audio Format (WebSocket)** | `pcm_s16le` (recommended) | `pcm_s16le` (recommended) |
| **Sample Rates** | 16000 Hz recommended | 8000, 16000, 24000, 44100 Hz |
| **Session Timeout** | 3 minutes inactivity | N/A specified |
| **Pricing** | 1 credit/second | 1 credit/second |

---

**End of Research Summary**
