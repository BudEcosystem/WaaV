# WaaV Gateway - Supported Providers

> **Last Updated:** 2026-01-17
> **Total Providers:** 70+ Cloud API Providers
> **Implemented:** 46 | **Blocked:** 10 | **Planned:** 14

WaaV Gateway provides unified access to 70+ cloud-based Speech-to-Text (STT), Text-to-Speech (TTS), and Audio-to-Audio providers worldwide.

---

## Quick Navigation

- [STT Providers](#speech-to-text-stt-providers) (27 implemented)
- [TTS Providers](#text-to-speech-tts-providers) (32 implemented)
- [Realtime/Audio-to-Audio Providers](#realtime-audio-to-audio-providers) (2 implemented)
- [Provider by Region](#providers-by-region)
- [Feature Comparison](#feature-comparison)
- [Configuration Guide](#configuration-guide)
- [Blocked Providers](#blocked-providers)

---

## Speech-to-Text (STT) Providers

### Global Cloud Leaders

| Provider | Protocol | Languages | Key Features | Env Variable |
|----------|----------|-----------|--------------|--------------|
| **Deepgram** | WebSocket | 36+ | Nova-2 model, real-time streaming, keyword boosting | `DEEPGRAM_API_KEY` |
| **Google Cloud** | gRPC | 125+ | Chirp 3, speaker diarization, word timestamps | `GOOGLE_APPLICATION_CREDENTIALS` |
| **Microsoft Azure** | WebSocket | 100+ | Custom speech models, pronunciation assessment | `AZURE_SPEECH_SUBSCRIPTION_KEY` |
| **OpenAI** | REST | 57+ | Whisper model, translation support, gpt-4o-transcribe | `OPENAI_API_KEY` |
| **ElevenLabs** | WebSocket | 29+ | Voice activity detection, multilingual | `ELEVENLABS_API_KEY` |
| **AssemblyAI** | WebSocket | 99 | Streaming API v3, end-of-turn detection, LeMUR AI | `ASSEMBLYAI_API_KEY` |
| **Cartesia** | WebSocket | 15+ | Ink-Whisper, ultra-low latency | `CARTESIA_API_KEY` |
| **Amazon Transcribe** | AWS SDK | 100+ | PII redaction, speaker diarization, streaming | `AWS_ACCESS_KEY_ID` |
| **IBM Watson** | WebSocket | 30+ | Smart formatting, custom acoustic models | `IBM_WATSON_API_KEY` |
| **Groq** | REST | 57+ | 216x real-time speed, whisper-large-v3-turbo | `GROQ_API_KEY` |

### European Providers

| Provider | Protocol | Languages | Key Features | Env Variable |
|----------|----------|-----------|--------------|--------------|
| **Speechmatics** | WebSocket | 55+ | Enhanced operating point, medical model | `SPEECHMATICS_API_KEY` |
| **Gladia** | WebSocket | 100+ | <300ms latency, code-switching, solaria-1 | `GLADIA_API_KEY` |
| **Rev AI** | WebSocket | 9 streaming | Speaker detection, custom vocabulary | `REVAI_API_KEY` |
| **Phonexia** | WebSocket | 57-64 | On-premises, voice biometrics | `PHONEXIA_TOKEN` |
| **Acapela Group** | REST | 30+ | Email/password auth, custom dictionaries | `ACAPELA_EMAIL`, `ACAPELA_PASSWORD` |
| **Cereproc** | REST | 10+ | Celtic languages, emotional voices | `CEREPROC_EMAIL`, `CEREPROC_PASSWORD` |

### Russia/CIS Providers

| Provider | Protocol | Languages | Key Features | Env Variable |
|----------|----------|-----------|--------------|--------------|
| **Yandex SpeechKit** | REST | 14+ | Russian optimization, emotion detection | `YANDEX_API_KEY` |
| **Tinkoff VoiceKit** | gRPC | 2 | JWT auth, custom VAD, Russian/English | `TINKOFF_API_KEY` |
| **SberDevices** | REST | 2 | OAuth 2.0, SaluteSpeech, Russian/English | `SBERDEVICES_CLIENT_ID` |

### India Regional Providers

| Provider | Protocol | Languages | Key Features | Env Variable |
|----------|----------|-----------|--------------|--------------|
| **Sarvam AI** | WebSocket | 11+ | Saarika v2.5, 22 Indian languages | `SARVAM_API_KEY` |
| **Gnani.ai** | gRPC/REST | 14+ | Voice biometrics, Indic languages | `GNANI_TOKEN` |
| **Reverie** | WebSocket | 22+ | Code-mixed Hinglish, hot-word boosting | `REVERIE_API_KEY` |
| **AI4Bharat/Bhashini** | REST | 22+ | Government ULCA API, free service | `BHASHINI_USER_ID` |

### China & East Asia Providers

| Provider | Protocol | Languages | Key Features | Env Variable |
|----------|----------|-----------|--------------|--------------|
| **iFlytek** | WebSocket | 18+ | HMAC-SHA256 auth, Chinese dialects | `IFLYTEK_APPID` |
| **Alibaba Cloud** | WebSocket | 25+ | Qwen3-ASR, Paraformer, DashScope | `DASHSCOPE_API_KEY` |
| **Baidu AI** | WebSocket/REST | 10+ | Chinese dialects, emotion detection | `BAIDU_API_KEY` |
| **Tencent Cloud** | WebSocket | 17 | TC3-HMAC-SHA256, hotword boosting | `TENCENT_SECRET_ID` |
| **Huawei Cloud** | WebSocket | 12 | IAM token auth, Sichuan/Cantonese | `HUAWEI_PROJECT_ID` |
| **NAVER CLOVA** | REST | 5 | Korean optimization, NeuVis technology | `NAVER_CLIENT_ID` |
| **AmiVoice** | WebSocket | 4 | Japanese specialty, medical/finance | `AMIVOICE_APPKEY` |

### Southeast Asia Providers

| Provider | Protocol | Languages | Key Features | Env Variable |
|----------|----------|-----------|--------------|--------------|
| **Zalo AI** | REST | 1 | Vietnamese, Northern/Southern accents | `ZALO_API_KEY` |
| **FPT.AI** | REST | 1 | Vietnamese, 7 voices | `FPT_API_KEY` |
| **Viettel AI** | REST | 1 | Vietnamese, 96% accuracy | `VIETTEL_TOKEN` |
| **Prosa.ai** | WebSocket/REST | 1 | Indonesian, stt-general models | `PROSA_API_KEY` |
| **NECTEC** | REST | 1 | Thai government AI, Partii engines | `NECTEC_API_KEY` |

---

## Text-to-Speech (TTS) Providers

### Global Cloud Leaders

| Provider | Protocol | Voices | Languages | Key Features | Env Variable |
|----------|----------|--------|-----------|--------------|--------------|
| **Deepgram** | REST | 10+ | 10+ | Aura voices, natural prosody | `DEEPGRAM_API_KEY` |
| **Google Cloud** | gRPC | 380+ | 75+ | WaveNet, Neural2, Studio | `GOOGLE_APPLICATION_CREDENTIALS` |
| **Microsoft Azure** | WebSocket | 400+ | 140+ | Neural voices, SSML, emotion | `AZURE_SPEECH_SUBSCRIPTION_KEY` |
| **OpenAI** | REST | 10 | 57+ | TTS-1/TTS-1-HD, gpt-4o-mini-tts | `OPENAI_API_KEY` |
| **ElevenLabs** | WebSocket | 1000+ | 29+ | Voice cloning, emotional expression | `ELEVENLABS_API_KEY` |
| **Cartesia** | WebSocket | 50+ | 40+ | Sonic 3, <100ms latency, cloning | `CARTESIA_API_KEY` |
| **Amazon Polly** | AWS SDK | 60+ | 30+ | Neural/Generative engines, SSML | `AWS_ACCESS_KEY_ID` |
| **IBM Watson** | HTTP | 30+ | 15+ | V3 neural, rate/pitch control | `IBM_WATSON_API_KEY` |

### Voice Cloning Specialists

| Provider | Protocol | Languages | Latency | Key Features | Env Variable |
|----------|----------|-----------|---------|--------------|--------------|
| **Hume AI** | HTTP/WS | 11+ | ~200ms | Octave, emotion control, 48 emotions | `HUME_API_KEY` |
| **LMNT** | HTTP | 22+ | ~150ms | Voice cloning, top_p/temperature | `LMNT_API_KEY` |
| **Play.ht** | HTTP | 36+ | ~190ms | PlayDialog multi-turn, cloning | `PLAYHT_API_KEY` |
| **Murf.ai** | HTTP | 35+ | ~130ms | Falcon/Gen2, 12 regional endpoints | `MURF_API_KEY` |
| **WellSaid Labs** | HTTP | 20+ | ~500ms | Caruso AI Director, 200+ avatars | `WELLSAID_API_KEY` |
| **Resemble AI** | HTTP | 149+ | ~350ms | Chatterbox, paralinguistic tags | `RESEMBLE_API_KEY` |
| **Speechify** | HTTP | 50+ | ~200ms | Simba models, 1000+ voices | `SPEECHIFY_API_KEY` |
| **Unreal Speech** | HTTP | 8 | ~300ms | Kokoro voices, cost-effective | `UNREALSPEECH_API_KEY` |
| **Smallest.ai** | HTTP/WS | 16 | ~100ms | Lightning model, Indian languages | `SMALLEST_API_KEY` |

### European Providers

| Provider | Protocol | Voices | Languages | Key Features | Env Variable |
|----------|----------|--------|-----------|--------------|--------------|
| **Speechmatics** | HTTP | 4 | 1 | English only, <200ms TTFA | `SPEECHMATICS_API_KEY` |
| **Acapela Group** | REST | 250+ | 30+ | 17 audio formats, viseme data | `ACAPELA_EMAIL` |
| **Cereproc** | REST | 20+ | 10+ | Celtic languages, emotions | `CEREPROC_EMAIL` |

### Russia/CIS Providers

| Provider | Protocol | Voices | Languages | Key Features | Env Variable |
|----------|----------|--------|-----------|--------------|--------------|
| **Yandex SpeechKit** | REST | 29+ | 4 | 6 emotions, Russian optimization | `YANDEX_API_KEY` |
| **Tinkoff VoiceKit** | gRPC | 2 | 1 | Alyona/Dorofeev, SSML | `TINKOFF_API_KEY` |
| **SberDevices** | REST | 7 | 2 | SaluteSpeech, SSML | `SBERDEVICES_CLIENT_ID` |

### India Regional Providers

| Provider | Protocol | Voices | Languages | Key Features | Env Variable |
|----------|----------|--------|-----------|--------------|--------------|
| **Sarvam AI** | REST | 20+ | 11+ | Bulbul model, Indian languages | `SARVAM_API_KEY` |
| **Gnani.ai** | REST | 24+ | 12+ | Multi-speaker, Indic languages | `GNANI_TOKEN` |
| **Reverie** | REST | 36+ | 22+ | Male/female per language | `REVERIE_API_KEY` |
| **AI4Bharat/Bhashini** | REST | 44+ | 22+ | Government API, free | `BHASHINI_USER_ID` |

### China & East Asia Providers

| Provider | Protocol | Voices | Languages | Key Features | Env Variable |
|----------|----------|--------|-----------|--------------|--------------|
| **iFlytek** | WebSocket | 9+ | 15+ | Speed/volume/pitch control | `IFLYTEK_APPID` |
| **Alibaba Cloud** | WebSocket | 70+ | 25+ | CosyVoice, Qwen3-TTS | `DASHSCOPE_API_KEY` |
| **Baidu AI** | REST | 40+ | 10+ | Chinese dialects, emotion | `BAIDU_API_KEY` |
| **Tencent Cloud** | REST | 70+ | 17 | Premium/emotional voices | `TENCENT_SECRET_ID` |
| **Huawei Cloud** | REST/WS | 13+ | 2 | Premium voices, text splitting | `HUAWEI_PROJECT_ID` |
| **NAVER CLOVA** | REST | 100+ | 5 | NeuVis neural, emotion control | `NAVER_CLIENT_ID` |

### Southeast Asia Providers

| Provider | Protocol | Voices | Languages | Key Features | Env Variable |
|----------|----------|--------|-----------|--------------|--------------|
| **Zalo AI** | REST | 4 | 1 | Vietnamese, Northern/Southern | `ZALO_API_KEY` |
| **FPT.AI** | REST | 7 | 1 | Vietnamese, MP3/WAV | `FPT_API_KEY` |
| **Viettel AI** | REST | 12 | 1 | Vietnamese, speed control | `VIETTEL_TOKEN` |
| **Prosa.ai** | REST | 9 | 1 | Indonesian, pitch/tempo | `PROSA_API_KEY` |
| **NECTEC** | REST | 2 | 1 | Thai, VAJA9 engine, free | `NECTEC_API_KEY` |

---

## Realtime Audio-to-Audio Providers

| Provider | Protocol | Key Features | Use Case | Env Variable |
|----------|----------|--------------|----------|--------------|
| **OpenAI Realtime** | WebSocket | GPT-4o, full-duplex, function calling, VAD | Conversational AI | `OPENAI_API_KEY` |
| **Hume AI EVI** | WebSocket | EVI 3, 48 emotions, empathic responses | Emotional AI | `HUME_API_KEY` |

### OpenAI Realtime Features
- Full-duplex audio streaming with GPT-4o
- Real-time voice activity detection
- Function calling during conversation
- Multiple voices: alloy, ash, ballad, coral, echo, fable, nova, onyx, sage, shimmer
- WebSocket endpoint: `wss://api.openai.com/v1/realtime`

### Hume EVI Features
- 48 emotion dimensions measured in real-time
- Empathic response generation
- Context continuity across utterances
- Voice activity detection
- EVI 3 (recommended), EVI 2 (deprecated)
- WebSocket endpoint: `wss://api.hume.ai/v0/evi/chat`

---

## Providers by Region

### North America
- Deepgram, OpenAI, AssemblyAI, Amazon (Transcribe/Polly), IBM Watson, Groq, ElevenLabs, LMNT, Play.ht, Hume AI, WellSaid Labs, Resemble AI, Speechify, Unreal Speech

### Europe
- Google Cloud, Microsoft Azure, Speechmatics (UK), Gladia (France), Rev AI, Phonexia, Acapela Group, Cereproc (Scotland)

### Russia/CIS
- Yandex SpeechKit, Tinkoff VoiceKit, SberDevices SaluteSpeech

### India
- Sarvam AI, Gnani.ai, Reverie, AI4Bharat/Bhashini, Smallest.ai

### China
- iFlytek (科大讯飞), Alibaba Cloud DashScope, Baidu AI, Tencent Cloud, Huawei Cloud

### East Asia
- NAVER CLOVA (Korea), AmiVoice (Japan)

### Southeast Asia
- Zalo AI (Vietnam), FPT.AI (Vietnam), Viettel AI (Vietnam), Prosa.ai (Indonesia), NECTEC (Thailand)

---

## Feature Comparison

### Speed Comparison (STT)

| Tier | Providers | Typical Latency |
|------|-----------|-----------------|
| **Ultra-Fast** | Groq (216x RT), Deepgram | <100ms |
| **Fast** | Gladia, AssemblyAI, Cartesia | 100-300ms |
| **Standard** | Google, Azure, OpenAI | 300-500ms |

### Speed Comparison (TTS)

| Tier | Providers | Time-to-First-Audio |
|------|-----------|---------------------|
| **Ultra-Low** | Smallest.ai (~100ms), Cartesia (<100ms), LMNT (~150ms) | <200ms |
| **Low** | Play.ht (~190ms), Murf.ai (~130ms), Hume AI (~200ms) | 150-250ms |
| **Standard** | ElevenLabs, Azure, Resemble AI (~350ms) | 250-500ms |

### Voice Cloning Support

| Provider | Clone Time | Min Audio | Max Files | Features |
|----------|------------|-----------|-----------|----------|
| **ElevenLabs** | ~1 min | 30s | N/A | Instant + Professional |
| **Play.ht** | ~1 min | 30s | N/A | Instant |
| **LMNT** | ~1 min | 5s | 20 (250MB) | Enhancement option |
| **Resemble AI** | ~1 min | 10s | N/A | Rapid + Professional |
| **Hume AI** | ~1 min | 15s | N/A | Voice design |
| **Cartesia** | ~1 min | 10s | N/A | Instant |

### Emotion Control

| Provider | Method | Emotions Supported |
|----------|--------|-------------------|
| **Hume AI** | Natural language description | 48 dimensions |
| **ElevenLabs** | Audio tags in text | 5+ basic emotions |
| **Azure** | SSML prosody | 10+ emotions |
| **Tencent Cloud** | Voice selection | 10 emotional voices |
| **NAVER CLOVA** | Parameter | 3 levels |

---

## Configuration Guide

### Environment Variables

```bash
# Global Leaders
export DEEPGRAM_API_KEY="your-key"
export GOOGLE_APPLICATION_CREDENTIALS="/path/to/credentials.json"
export AZURE_SPEECH_SUBSCRIPTION_KEY="your-key"
export AZURE_SPEECH_REGION="eastus"
export OPENAI_API_KEY="your-key"
export ELEVENLABS_API_KEY="your-key"
export ASSEMBLYAI_API_KEY="your-key"
export CARTESIA_API_KEY="your-key"
export AWS_ACCESS_KEY_ID="your-key"
export AWS_SECRET_ACCESS_KEY="your-secret"
export IBM_WATSON_API_KEY="your-key"
export IBM_WATSON_INSTANCE_ID="your-instance"
export GROQ_API_KEY="gsk_your-key"

# Voice Cloning Specialists
export HUME_API_KEY="your-key"
export LMNT_API_KEY="your-key"
export PLAYHT_API_KEY="your-key"
export PLAYHT_USER_ID="your-user-id"
export MURF_API_KEY="your-key"
export WELLSAID_API_KEY="your-key"
export RESEMBLE_API_KEY="your-key"
export SPEECHIFY_API_KEY="your-key"
export UNREALSPEECH_API_KEY="your-key"
export SMALLEST_API_KEY="your-key"

# European Providers
export SPEECHMATICS_API_KEY="your-key"
export GLADIA_API_KEY="your-key"
export REVAI_API_KEY="your-key"
export ACAPELA_EMAIL="email"
export ACAPELA_PASSWORD="password"
export CEREPROC_EMAIL="email"
export CEREPROC_PASSWORD="password"

# Russia/CIS
export YANDEX_API_KEY="folder_id:api_key"
export TINKOFF_API_KEY="api_key|secret_key"
export SBERDEVICES_CLIENT_ID="client_id:client_secret"

# India Regional
export SARVAM_API_KEY="your-key"
export GNANI_TOKEN="your-token"
export GNANI_ACCESS_KEY="your-key"
export REVERIE_API_KEY="your-key"
export BHASHINI_USER_ID="user_id|ulca_api_key"

# China & East Asia
export IFLYTEK_APPID="app_id|api_key|api_secret"
export DASHSCOPE_API_KEY="your-key"
export BAIDU_API_KEY="api_key|secret_key"
export TENCENT_SECRET_ID="secret_id"
export TENCENT_SECRET_KEY="secret_key"
export HUAWEI_PROJECT_ID="your-project-id"
export HUAWEI_IAM_USER="username"
export HUAWEI_IAM_PASSWORD="password"
export NAVER_CLIENT_ID="your-client-id"
export NAVER_CLIENT_SECRET="your-secret"

# Southeast Asia
export ZALO_API_KEY="your-key"
export FPT_API_KEY="your-key"
export VIETTEL_TOKEN="your-token"
export PROSA_API_KEY="your-key"
export NECTEC_API_KEY="your-key"
```

### WebSocket Configuration Examples

```json
// Deepgram STT + ElevenLabs TTS
{
  "type": "config",
  "config": {
    "stt_provider": "deepgram",
    "tts_provider": "elevenlabs",
    "deepgram_model": "nova-2",
    "elevenlabs_voice_id": "21m00Tcm4TlvDq8ikWAM"
  }
}

// Groq STT (ultra-fast) + Hume AI TTS
{
  "type": "config",
  "config": {
    "stt_provider": "groq",
    "tts_provider": "hume",
    "groq_model": "whisper-large-v3-turbo",
    "hume_voice_id": "your-voice-id"
  }
}

// India: Gnani STT + Sarvam TTS
{
  "type": "config",
  "config": {
    "stt_provider": "gnani",
    "tts_provider": "sarvam",
    "language": "hi-IN"
  }
}

// China: Alibaba Cloud STT + TTS
{
  "type": "config",
  "config": {
    "stt_provider": "alibaba_cloud",
    "tts_provider": "alibaba_cloud",
    "alibaba_stt_model": "qwen3-asr-flash-realtime",
    "alibaba_tts_model": "cosyvoice-v3-flash"
  }
}
```

---

## Blocked Providers

The following providers cannot be integrated due to API limitations:

| Provider | Reason | Alternative |
|----------|--------|-------------|
| **Otter.ai** | Enterprise/Beta API only | AssemblyAI, Deepgram |
| **Verbit** | Enterprise workflow required | Rev AI, Speechmatics |
| **SpeechText.AI** | No real-time streaming | Groq, Deepgram |
| **Speechly** | Acquired by Roblox (2023) | AssemblyAI, Deepgram |
| **ReadSpeaker** | Enterprise only, no public API | Azure, Google Cloud |
| **Nuance** | EOL May 2027 | Azure Speech Services |
| **Kakao** | Original API terminated (2022) | NAVER CLOVA |
| **NTT COTOHA** | Service terminated (2024) | Google Cloud, Azure |
| **Vbee** | API docs inaccessible | Zalo AI, FPT.AI |
| **Kata.ai** | Enterprise platform only | Prosa.ai |
| **CoRover** | Chatbot platform, no standalone API | Bhashini |

---

## Planned Providers

The following providers are planned for future releases:

### Middle East & Africa
- NeuralSpace (115+ languages)
- Sestek/Knovvu (Turkey)
- Lahajati (108 Arabic dialects)
- AzReco (Azerbaijan)
- ISSAI (Kazakh/Turkic)
- Intron Health (African medical)
- Lelapa AI (South Africa)
- Lesan AI (Ethiopia)

### Latin America
- Vozy

### Model Hosting Platforms
- DeepInfra
- Replicate

### Specialized/Regional
- AlfaNum (Balkans)
- Aseto AI (Greek)
- Elhuyar (Basque)
- ABAIR (Irish Gaelic)
- SignAll (ASL)
- Signapse (BSL)
- Botnoi Voice (Thailand)

---

## Provider Selection Guide

| Use Case | Recommended STT | Recommended TTS |
|----------|-----------------|-----------------|
| **Ultra-fast transcription** | Groq, Deepgram | Cartesia, Smallest.ai |
| **Low latency streaming** | Deepgram, Gladia | LMNT, Play.ht, Cartesia |
| **High accuracy** | AssemblyAI, Google | Azure Neural, Google WaveNet |
| **Voice cloning** | - | ElevenLabs, Play.ht, Resemble AI |
| **Emotion control** | - | Hume AI, Azure SSML |
| **Multi-language (global)** | Google (125+), AssemblyAI (99) | Azure (140+), Google (75+) |
| **Indian languages** | Sarvam AI, Gnani, Bhashini | Sarvam AI, Reverie, Bhashini |
| **Chinese languages** | Alibaba, iFlytek, Tencent | Alibaba CosyVoice, iFlytek |
| **Vietnamese** | Zalo AI, FPT.AI, Viettel AI | Zalo AI, FPT.AI, Viettel AI |
| **Cost-effective** | Groq ($0.04/hr), Deepgram | OpenAI, Deepgram |
| **Enterprise/HIPAA** | Azure, Google, AWS, IBM | Azure, Google, AWS, IBM |
| **Conversational AI** | - | OpenAI Realtime, Hume EVI |
| **On-premises** | Phonexia | - |

---

## Version History

- **2026-01-17**: Added NECTEC (Thailand), updated provider counts
- **2026-01-14**: Added Tencent Cloud, Huawei Cloud, NAVER CLOVA, AmiVoice
- **2026-01-13**: Added 20+ providers including Southeast Asia region
- **2026-01-07**: Added Hume AI, LMNT, Play.ht
- **2026-01-06**: Initial release with 25 providers

---

For detailed API documentation and implementation guides, see the [provider documentation](./providers/) directory.
