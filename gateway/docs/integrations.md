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

### Text-to-Speech (TTS)

| Provider | Protocol | Key Features | Environment Variable |
|----------|----------|--------------|---------------------|
| **Deepgram** | REST | Aura voices, natural prosody | `DEEPGRAM_API_KEY` |
| **Google Cloud** | gRPC | WaveNet, Neural2, Studio voices, 220+ voices | `GOOGLE_APPLICATION_CREDENTIALS` |
| **Microsoft Azure** | WebSocket | 400+ neural voices, 140+ languages, SSML support | `AZURE_SPEECH_SUBSCRIPTION_KEY`, `AZURE_SPEECH_REGION` |
| **ElevenLabs** | WebSocket | Voice cloning, 29 languages, emotional expression | `ELEVENLABS_API_KEY` |
| **OpenAI** | REST | TTS-1/TTS-1-HD models, 6 voices | `OPENAI_API_KEY` |
| **Cartesia** | WebSocket | Sonic model, ultra-low latency, voice cloning | `CARTESIA_API_KEY` |

### Audio-to-Audio (Realtime)

| Provider | Protocol | Key Features | Environment Variable |
|----------|----------|--------------|---------------------|
| **OpenAI Realtime** | WebSocket | GPT-4o, full-duplex audio streaming, function calling, voice activity detection | `OPENAI_API_KEY` |

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
| **Low latency** | Deepgram, Cartesia | Cartesia, ElevenLabs |
| **High accuracy** | AssemblyAI, Google | Google Neural2, Azure |
| **Voice cloning** | - | ElevenLabs, Cartesia |
| **Multi-language** | AssemblyAI (99), Google (125+) | Azure (140+), Google (40+) |
| **Cost-effective** | Deepgram, OpenAI | OpenAI, Deepgram |
| **Enterprise/HIPAA** | Azure, Google | Azure, Google |
| **Conversational AI** | - | OpenAI Realtime |

---

## Coming Soon

The following providers are planned for future releases:

- Amazon Transcribe / Polly
- IBM Watson Speech
- Groq (Whisper hosting)
- Hume AI (emotional TTS)
- LMNT
- Play.ht
- Speechmatics
- Gladia
- And 60+ more regional providers

See [provider_integration_status.md](provider_integration_status.md) for the full roadmap.
