# WaaV Gateway - Supported Integrations

This document lists all speech-to-text (STT), text-to-speech (TTS), and audio-to-audio providers supported by WaaV Gateway.

---

## Currently Supported Providers

### Speech-to-Text (STT)

| Provider | Protocol | Key Features | Environment Variable |
|----------|----------|--------------|---------------------|
| **Deepgram** | WebSocket | Nova-2 model, 36+ languages, real-time streaming | `DEEPGRAM_API_KEY` |
| **Google Cloud** | gRPC | 125+ languages, speaker diarization, word-level timestamps | `GOOGLE_APPLICATION_CREDENTIALS` |
| **Microsoft Azure** | WebSocket | 100+ languages, custom speech models, pronunciation assessment | `AZURE_SPEECH_SUBSCRIPTION_KEY`, `AZURE_SPEECH_REGION` |
| **ElevenLabs** | WebSocket | Multilingual transcription, voice activity detection | `ELEVENLABS_API_KEY` |
| **OpenAI** | REST | Whisper model, 57+ languages, translation support | `OPENAI_API_KEY` |
| **AssemblyAI** | WebSocket | Streaming API v3, 99 languages, immutable transcripts, end-of-turn detection | `ASSEMBLYAI_API_KEY` |
| **Cartesia** | WebSocket | Low-latency streaming, word-level timestamps | `CARTESIA_API_KEY` |
| **Amazon Transcribe** | AWS SDK | 100+ languages, streaming, speaker diarization, content redaction | `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY` |
| **IBM Watson** | WebSocket | 30+ languages, speaker diarization, smart formatting, background noise suppression | `IBM_WATSON_API_KEY`, `IBM_WATSON_INSTANCE_ID` |
| **Groq** | REST | Ultra-fast Whisper (216x real-time), translation to English | `GROQ_API_KEY` |

### Text-to-Speech (TTS)

| Provider | Protocol | Key Features | Environment Variable |
|----------|----------|--------------|---------------------|
| **Deepgram** | REST | Aura voices, natural prosody | `DEEPGRAM_API_KEY` |
| **Google Cloud** | gRPC | WaveNet, Neural2, Studio voices, 220+ voices | `GOOGLE_APPLICATION_CREDENTIALS` |
| **Microsoft Azure** | WebSocket | 400+ neural voices, 140+ languages, SSML support | `AZURE_SPEECH_SUBSCRIPTION_KEY`, `AZURE_SPEECH_REGION` |
| **ElevenLabs** | WebSocket | Voice cloning, 29 languages, emotional expression | `ELEVENLABS_API_KEY` |
| **OpenAI** | REST | TTS-1/TTS-1-HD models, 6 voices | `OPENAI_API_KEY` |
| **Cartesia** | WebSocket | Sonic model, ultra-low latency, voice cloning | `CARTESIA_API_KEY` |
| **Amazon Polly** | AWS SDK | 60+ voices, neural/standard/generative engines, SSML, 30+ languages | `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY` |
| **IBM Watson** | HTTP | 30+ V3 neural voices, 15+ languages, SSML, rate/pitch control | `IBM_WATSON_API_KEY`, `IBM_WATSON_INSTANCE_ID` |
| **Hume AI** | HTTP/WebSocket | Octave TTS, natural language emotion control, voice cloning, acting instructions | `HUME_API_KEY` |
| **LMNT** | HTTP | Low-latency (~150ms), voice cloning, 22+ languages, top_p/temperature control | `LMNT_API_KEY` |
| **Play.ht** | HTTP | Low-latency (~190ms), PlayDialog multi-turn, 36+ languages, voice cloning | `PLAYHT_API_KEY`, `PLAYHT_USER_ID` |

### Audio-to-Audio (Realtime)

| Provider | Protocol | Key Features | Environment Variable |
|----------|----------|--------------|---------------------|
| **OpenAI Realtime** | WebSocket | GPT-4o, full-duplex audio streaming, function calling, voice activity detection | `OPENAI_API_KEY` |
| **Hume AI EVI** | WebSocket | EVI 3 empathic voice interface, 48 emotion dimensions, prosody analysis, empathic response generation | `HUME_API_KEY` |

---

## Provider Details

### Deepgram

- **Website:** https://deepgram.com
- **Documentation:** https://developers.deepgram.com
- **Capabilities:** STT, TTS
- **Supported Models:** Nova-2, Nova, Enhanced, Base
- **Audio Formats:** PCM, WAV, MP3, FLAC, OGG
- **Sample Rates:** 8kHz - 48kHz

### Google Cloud Speech

- **Website:** https://cloud.google.com/speech-to-text
- **Documentation:** https://cloud.google.com/speech-to-text/docs
- **Capabilities:** STT, TTS
- **Authentication:** Service Account JSON file
- **Voice Types:** WaveNet, Neural2, Studio, Standard
- **Special Features:** Speaker diarization, automatic punctuation, profanity filtering

### Microsoft Azure Speech Services

- **Website:** https://azure.microsoft.com/services/cognitive-services/speech-services
- **Documentation:** https://docs.microsoft.com/azure/cognitive-services/speech-service
- **Capabilities:** STT, TTS
- **Regions:** 30+ Azure regions worldwide
- **Special Features:** Custom speech models, pronunciation assessment, SSML support

### ElevenLabs

- **Website:** https://elevenlabs.io
- **Documentation:** https://docs.elevenlabs.io
- **Capabilities:** STT, TTS, Voice Cloning
- **Voice Cloning:** Instant and Professional cloning
- **Special Features:** Emotional expression, 29 languages

### OpenAI

- **Website:** https://openai.com
- **Documentation:** https://platform.openai.com/docs
- **Capabilities:** STT (Whisper), TTS, Realtime Audio-to-Audio
- **STT Models:** whisper-1, gpt-4o-transcribe, gpt-4o-mini-transcribe
- **TTS Models:** tts-1, tts-1-hd, gpt-4o-mini-tts
- **TTS Voices:** alloy, ash, ballad, coral, echo, fable, nova, onyx, sage, shimmer
- **Realtime Model:** gpt-4o-realtime-preview
- **Special Features:** Translation, full-duplex audio, function calling

### Hume AI

- **Website:** https://hume.ai
- **Documentation:** https://dev.hume.ai/docs
- **Capabilities:** TTS (Octave), Audio-to-Audio (EVI)
- **TTS Protocol:** HTTP streaming + WebSocket
- **EVI Protocol:** WebSocket (full-duplex)
- **TTS Features:**
  - Natural language emotion control via `description` field (max 100 chars)
  - Acting instructions (e.g., "happy, energetic", "whispered fearfully")
  - Voice cloning with 15+ seconds of audio samples
  - Speed control (0.5 to 2.0)
  - Instant mode for low-latency streaming
  - 11 languages supported
- **EVI Features (Empathic Voice Interface):**
  - Full-duplex audio streaming
  - 48 emotion dimensions measured in real-time (prosody analysis)
  - Empathic response generation
  - Voice activity detection
  - Context continuity across utterances
- **Prosody Dimensions (48 emotions):**
  - admiration, amusement, anger, anxiety, awe, boredom, calmness
  - concentration, confusion, contemplation, contempt, contentment
  - desire, determination, disappointment, disgust, distress, doubt
  - ecstasy, embarrassment, empathic_pain, enthusiasm, envy, excitement
  - fear, guilt, horror, interest, joy, love, nostalgia, pain, pride
  - realization, relief, sadness, satisfaction, shame, surprise_negative
  - surprise_positive, sympathy, tiredness, triumph
- **EVI Versions:**
  - EVI 3 (recommended)
  - EVI 2 (deprecated, sunset August 30, 2025)
  - EVI 1 (deprecated, sunset August 30, 2025)
- **Pricing:**
  - EVI 3: Starting ~$0.02/min (volume), $0.072/min standard
  - Octave TTS: Free tier (10K chars), Starter $3/mo (30K chars), Creator $10/mo (100K chars)
- **Authentication:** API key header (`X-HUME-API-KEY`)

### LMNT

- **Website:** https://lmnt.com
- **Documentation:** https://docs.lmnt.com
- **Capabilities:** TTS, Voice Cloning
- **Protocol:** HTTP streaming
- **Typical Latency:** ~150ms
- **Max Text Length:** 5000 characters per request
- **Languages:** 22+ languages (auto-detect, Arabic, German, English, Spanish, French, Hindi, Indonesian, Italian, Japanese, Korean, Dutch, Polish, Portuguese, Russian, Swedish, Thai, Turkish, Ukrainian, Urdu, Vietnamese, Chinese)
- **Audio Formats:**
  - **MP3** - 96kbps, streamable (default)
  - **PCM S16LE** - 16-bit signed PCM, streamable
  - **PCM F32LE** - 32-bit float PCM, streamable
  - **µ-law** - 8-bit G711, streamable
  - **WebM** - Opus codec, streamable
  - **WAV** - Not streamable
  - **AAC** - Not streamable
- **Sample Rates:** 8000 Hz, 16000 Hz, 24000 Hz (default)
- **Voice Parameters:**
  - `top_p` (0-1): Speech stability control (default: 0.8)
  - `temperature` (≥0): Expressiveness range (default: 1.0)
  - `speed` (0.25-2.0): Speech rate control (default: 1.0)
  - `seed`: Deterministic output (optional)
- **Voice Cloning:**
  - Minimum audio: 5 seconds
  - Maximum files: 20 attachments
  - Maximum total size: 250 MB
  - Supported formats: WAV, MP3, MP4, M4A, WebM
  - Enhancement option: Process noisy audio automatically
- **Endpoints:**
  - HTTP Streaming: `POST https://api.lmnt.com/v1/ai/speech/bytes`
  - Voice List: `GET https://api.lmnt.com/v1/ai/voice/list`
  - Voice Clone: `POST https://api.lmnt.com/v1/ai/voice`
- **Authentication:** `X-API-Key` header

### Play.ht

- **Website:** https://play.ht
- **Documentation:** https://docs.play.ht
- **Capabilities:** TTS, Voice Cloning
- **Protocol:** HTTP streaming
- **Typical Latency:** ~190ms (Play3.0-mini), ~350ms (PlayDialog)
- **Max Text Length:** 20,000 characters per request
- **Languages:** 36+ languages (Afrikaans, Albanian, Amharic, Arabic, Bengali, Bulgarian, Catalan, Croatian, Czech, Danish, Dutch, English, French, Galician, German, Greek, Hebrew, Hindi, Hungarian, Indonesian, Italian, Japanese, Korean, Malay, Mandarin, Polish, Portuguese, Russian, Serbian, Spanish, Swedish, Tagalog, Thai, Turkish, Ukrainian, Urdu, Xhosa)
- **Voice Engines/Models:**
  - **Play3.0-mini** - Fast, multilingual, 36+ languages (~190ms TTFA)
  - **PlayDialog** - Expressive, two-speaker dialogue support (~350ms TTFA)
  - **PlayDialogMultilingual** - Multilingual dialogue support
  - **PlayDialogArabic** - Arabic dialogue support
  - **PlayHT2.0-turbo** - Legacy English only (~230ms TTFA)
- **Audio Formats:**
  - **MP3** - Streamable (default)
  - **WAV** - Streamable
  - **PCM Mulaw** - Streamable
  - **FLAC** - Streamable
  - **OGG** - Streamable
  - **Raw** - PCM, Streamable
- **Sample Rates:** 8000 Hz, 16000 Hz, 24000 Hz, 44100 Hz, 48000 Hz (default)
- **Voice Parameters:**
  - `speed` (0.5-2.0): Playback speed control (default: 1.0)
  - `temperature` (0.0-1.0): Randomness control
  - `seed`: Deterministic output
  - `quality`: Audio quality tier (draft/standard/premium)
  - `text_guidance`: Text adherence (Play3.0, PlayHT2.0)
  - `voice_guidance`: Voice adherence (Play3.0, PlayHT2.0)
  - `style_guidance`: Style adherence (Play3.0 only)
  - `repetition_penalty`: Repetition control
- **PlayDialog Parameters:**
  - `voice_2`: Second speaker voice URL
  - `turn_prefix`: First speaker identifier (e.g., "S1:")
  - `turn_prefix_2`: Second speaker identifier (e.g., "S2:")
  - `voice_conditioning_seconds`: Reference audio duration
  - `num_candidates`: Number of candidates for ranking
- **Voice Cloning:**
  - Instant voice clones from 30+ second audio samples
  - Uses multipart/form-data upload
- **Endpoints:**
  - HTTP Streaming: `POST https://api.play.ht/api/v2/tts/stream`
  - Voice List: `GET https://api.play.ht/api/v2/voices`
  - Voice Clone: `POST https://api.play.ht/api/v2/cloned-voices/instant`
  - WebSocket Auth: `POST https://api.play.ht/api/v4/websocket-auth`
- **Authentication:** Dual-header authentication
  - `X-USER-ID`: Your Play.ht user ID
  - `AUTHORIZATION`: Your Play.ht API key

### AssemblyAI

- **Website:** https://assemblyai.com
- **Documentation:** https://www.assemblyai.com/docs
- **Capabilities:** STT
- **API Version:** Streaming API v3
- **Languages:** 99 languages supported
- **Special Features:**
  - Immutable transcripts (transcripts never modified after delivery)
  - End-of-turn detection with configurable confidence threshold
  - Word-level timestamps
  - Automatic language detection (multilingual model)
  - Regional endpoints (US and EU)
- **Audio Encoding:** PCM S16LE, PCM Mu-law
- **Sample Rates:** 8kHz - 48kHz

### Cartesia

- **Website:** https://cartesia.ai
- **Documentation:** https://docs.cartesia.ai
- **Capabilities:** STT, TTS, Voice Cloning
- **Model:** Sonic
- **Special Features:** Ultra-low latency (<100ms), voice cloning, emotion control

### Amazon Transcribe

- **Website:** https://aws.amazon.com/transcribe
- **Documentation:** https://docs.aws.amazon.com/transcribe
- **Capabilities:** STT
- **Protocol:** AWS SDK (HTTP/2 streaming)
- **Languages:** 100+ languages
- **Regions:** 16 AWS regions (us-east-1, us-west-2, eu-west-1, etc.)
- **Audio Formats:** PCM (16-bit LE), FLAC, OGG-OPUS
- **Sample Rates:** 8kHz - 48kHz
- **Special Features:**
  - Real-time streaming transcription
  - Speaker diarization (2-10 speakers)
  - Custom vocabularies and language models
  - Content redaction (PII masking)
  - Automatic language detection
  - Partial results stabilization (high/medium/low)
  - Channel identification for multi-channel audio
- **Authentication:** AWS access keys, IAM roles, or AWS credentials file

### Amazon Polly

- **Website:** https://aws.amazon.com/polly
- **Documentation:** https://docs.aws.amazon.com/polly
- **Capabilities:** TTS
- **Protocol:** AWS SDK (SynthesizeSpeech API)
- **Voices:** 60+ voices across 30+ languages
- **Engines:** Standard, Neural, Long-Form, Generative
- **Output Formats:** MP3, OGG-Vorbis, PCM (16-bit LE)
- **Sample Rates:** MP3/OGG (8-24kHz), PCM (8-16kHz)
- **Special Features:**
  - Neural voices for natural-sounding speech
  - SSML support for pronunciation control
  - Custom lexicons (up to 5 per request)
  - Long-form engine for audiobooks/articles
  - Generative engine for highest quality
- **Authentication:** AWS access keys, IAM roles, or AWS credentials file

### IBM Watson Speech Services

- **Website:** https://www.ibm.com/cloud/watson-speech-to-text
- **Documentation:** https://cloud.ibm.com/apidocs/speech-to-text, https://cloud.ibm.com/apidocs/text-to-speech
- **Capabilities:** STT, TTS
- **STT Protocol:** WebSocket (real-time streaming)
- **TTS Protocol:** HTTP REST
- **Languages:** 30+ languages supported
- **Regions:** us-south (Dallas), us-east (Washington DC), eu-de (Frankfurt), eu-gb (London), au-syd (Sydney), jp-tok (Tokyo), kr-seo (Seoul)
- **STT Features:**
  - Real-time streaming transcription
  - Speaker diarization (speaker labels)
  - Smart formatting (numbers, dates, currencies)
  - Word-level timestamps and confidence
  - Background audio suppression
  - Profanity filtering and redaction
  - Custom language and acoustic models
  - Low-latency mode for faster interim results
- **TTS Features:**
  - 30+ V3 neural voices across 15+ languages
  - SSML support for prosody control
  - Rate and pitch adjustment (-100% to +100%)
  - Multiple audio formats: WAV, MP3, OGG-Opus, OGG-Vorbis, FLAC, WebM, L16 (PCM), μ-law, A-law
  - Custom pronunciation dictionaries (up to 2 per request)
- **TTS Voices (Selected):**
  - **US English:** Allison, Emily, Henry, Kevin, Lisa, Michael, Olivia
  - **UK English:** Charlotte, James, Kate
  - **German:** Birgit, Dieter, Erika
  - **Spanish:** Enrique, Laura, Sofia
  - **French:** Nicolas, Renee, Louise (Canadian)
  - **Japanese:** Emi
  - **Korean:** Hyunjun, Siwoo, Youngmi, Yuna
  - **Chinese:** LiNa, WangWei, ZhangJing
- **Authentication:** IAM token-based (API key exchanged for bearer token)

### Groq (Whisper)

- **Website:** https://groq.com
- **Documentation:** https://console.groq.com/docs/speech-to-text
- **Capabilities:** STT
- **Protocol:** HTTP REST (OpenAI-compatible API format)
- **Models:**
  - **whisper-large-v3:** 10.3% WER, 189x real-time, $0.111/hour
  - **whisper-large-v3-turbo:** 12% WER, 216x real-time, $0.04/hour (default)
- **Audio Formats:** FLAC, MP3, MP4, MPEG, MPGA, M4A, OGG, WAV, WebM
- **Sample Rates:** Downsampled to 16kHz mono internally
- **File Size Limits:**
  - Free tier: 25MB max
  - Dev tier: 100MB max
- **Features:**
  - Ultra-fast transcription (fastest Whisper hosting)
  - OpenAI-compatible API format
  - Translation endpoint (any language to English)
  - Word and segment-level timestamps (verbose_json format)
  - Automatic retry with exponential backoff for rate limits
  - Silence detection for automatic flushing
- **Response Formats:**
  - `json` - Simple text response
  - `verbose_json` - With timestamps, segments, and words
  - `text` - Plain text output
- **Endpoints:**
  - Transcription: `https://api.groq.com/openai/v1/audio/transcriptions`
  - Translation: `https://api.groq.com/openai/v1/audio/translations`
- **Rate Limits:**
  - Applied at organization level
  - 429 errors include retry-after header
  - Automatic retry with exponential backoff recommended
- **Authentication:** Bearer token (API key starting with `gsk_`)

---

## Configuration Examples

### Environment Variables

```bash
# Deepgram
export DEEPGRAM_API_KEY="your-deepgram-key"

# Google Cloud
export GOOGLE_APPLICATION_CREDENTIALS="/path/to/service-account.json"

# Microsoft Azure
export AZURE_SPEECH_SUBSCRIPTION_KEY="your-azure-key"
export AZURE_SPEECH_REGION="eastus"

# ElevenLabs
export ELEVENLABS_API_KEY="your-elevenlabs-key"

# OpenAI
export OPENAI_API_KEY="your-openai-key"

# AssemblyAI
export ASSEMBLYAI_API_KEY="your-assemblyai-key"

# Cartesia
export CARTESIA_API_KEY="your-cartesia-key"

# AWS (Amazon Transcribe / Polly)
export AWS_ACCESS_KEY_ID="your-aws-access-key"
export AWS_SECRET_ACCESS_KEY="your-aws-secret-key"
export AWS_REGION="us-east-1"  # Optional, defaults to us-east-1

# IBM Watson
export IBM_WATSON_API_KEY="your-ibm-watson-key"
export IBM_WATSON_INSTANCE_ID="your-instance-id"
export IBM_WATSON_REGION="us-south"  # Optional, defaults to us-south

# Groq
export GROQ_API_KEY="gsk_your-groq-api-key"

# Hume AI
export HUME_API_KEY="your-hume-api-key"

# LMNT
export LMNT_API_KEY="your-lmnt-api-key"

# Play.ht
export PLAYHT_API_KEY="your-playht-api-key"
export PLAYHT_USER_ID="your-playht-user-id"
```

### YAML Configuration

```yaml
providers:
  deepgram_api_key: ${DEEPGRAM_API_KEY}
  elevenlabs_api_key: ${ELEVENLABS_API_KEY}
  openai_api_key: ${OPENAI_API_KEY}
  assemblyai_api_key: ${ASSEMBLYAI_API_KEY}
  cartesia_api_key: ${CARTESIA_API_KEY}
  azure:
    subscription_key: ${AZURE_SPEECH_SUBSCRIPTION_KEY}
    region: ${AZURE_SPEECH_REGION}
  google:
    credentials_path: ${GOOGLE_APPLICATION_CREDENTIALS}
  aws:
    access_key_id: ${AWS_ACCESS_KEY_ID}
    secret_access_key: ${AWS_SECRET_ACCESS_KEY}
    region: ${AWS_REGION}
  ibm_watson:
    api_key: ${IBM_WATSON_API_KEY}
    instance_id: ${IBM_WATSON_INSTANCE_ID}
    region: ${IBM_WATSON_REGION}
  groq_api_key: ${GROQ_API_KEY}
  hume_api_key: ${HUME_API_KEY}
  lmnt_api_key: ${LMNT_API_KEY}
  playht:
    api_key: ${PLAYHT_API_KEY}
    user_id: ${PLAYHT_USER_ID}
```

### WebSocket Configuration Message

```json
{
  "type": "config",
  "config": {
    "stt_provider": "assemblyai",
    "tts_provider": "elevenlabs",
    "assemblyai_model": "universal-streaming-english",
    "elevenlabs_voice_id": "21m00Tcm4TlvDq8ikWAM"
  }
}
```

---

## Provider Selection Guide

| Use Case | Recommended STT | Recommended TTS |
|----------|-----------------|-----------------|
| **Ultra-fast** | Groq (216x real-time) | Cartesia, ElevenLabs, LMNT, Play.ht |
| **Low latency** | Deepgram, Cartesia | Cartesia, ElevenLabs, LMNT, Play.ht |
| **High accuracy** | AssemblyAI, Google | Google Neural2, Azure |
| **Voice cloning** | - | ElevenLabs, Cartesia, Hume AI, LMNT, Play.ht |
| **Emotion control** | - | Hume AI, ElevenLabs, Azure |
| **Multi-language** | AssemblyAI (99), Amazon Transcribe (100+), Google (125+), IBM Watson (30+) | Azure (140+), Amazon Polly (30+), Google (40+), Play.ht (36+), LMNT (22+), IBM Watson (15+) |
| **Cost-effective** | Deepgram, Groq ($0.04/hr), OpenAI | OpenAI, Deepgram |
| **Enterprise/HIPAA** | Azure, Google, Amazon Transcribe, IBM Watson | Azure, Google, Amazon Polly, IBM Watson |
| **Conversational AI** | - | OpenAI Realtime, Hume AI EVI |
| **Multi-turn dialogue** | - | Play.ht PlayDialog |

---

## Pricing Reference

WaaV Gateway includes a centralized pricing database at `src/config/pricing.rs` that can be used to estimate costs for all providers. The pricing data is kept up-to-date and provides:

- Per-provider, per-model pricing
- Helper functions: `estimate_stt_cost()`, `estimate_tts_cost()`
- Support for different pricing units (per-hour, per-minute, per-1K chars, per-1M chars)

Example usage:
```rust
use waav_gateway::config::{estimate_stt_cost, estimate_tts_cost};

// Estimate cost for 1 hour of Groq Whisper transcription
let stt_cost = estimate_stt_cost("groq", "whisper-large-v3-turbo", 3600.0);
// Returns: Some(0.04)

// Estimate cost for 1000 characters of ElevenLabs TTS
let tts_cost = estimate_tts_cost("elevenlabs", "eleven_multilingual_v2", 1000);
// Returns: Some(0.24)
```

---

## Coming Soon

The following providers are planned for future releases:

- Speechmatics
- Gladia
- And 60+ more regional providers

See [provider_integration_status.md](provider_integration_status.md) for the full roadmap.
