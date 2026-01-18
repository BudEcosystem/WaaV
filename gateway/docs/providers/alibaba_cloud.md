# Alibaba Cloud Intelligent Speech Integration

> **Provider:** #38 Alibaba Cloud Intelligent Speech (阿里云智能语音)
> **Type:** STT + TTS + Audio-to-Audio + Voice Cloning
> **Batch:** 6 (China & East Asia)
> **Status:** In Progress
> **Last Updated:** 2026-01-13

---

## Overview

Alibaba Cloud's DashScope Model Studio provides comprehensive speech AI services through WebSocket APIs. The platform offers multiple STT and TTS models with support for 25+ languages, voice cloning, and real-time audio-to-audio interactions.

### Key Features

| Feature | Support |
|---------|---------|
| Real-time STT | ✅ WebSocket streaming |
| Real-time TTS | ✅ WebSocket streaming |
| Audio-to-Audio | ✅ Qwen-Omni |
| Voice Cloning | ✅ Instant clone |
| Voice Design | ✅ Custom voices |
| Chinese Dialects | ✅ 5+ dialects |
| Emotion Recognition | ✅ STT |
| SSML Support | ✅ CosyVoice |

---

## API Endpoints

### Regional Endpoints

| Region | Realtime API | Inference API |
|--------|--------------|---------------|
| Beijing (China) | `wss://dashscope.aliyuncs.com/api-ws/v1/realtime` | `wss://dashscope.aliyuncs.com/api-ws/v1/inference` |
| Singapore (International) | `wss://dashscope-intl.aliyuncs.com/api-ws/v1/realtime` | `wss://dashscope-intl.aliyuncs.com/api-ws/v1/inference` |

### Model Query Parameters

Add model to URL query string for realtime endpoints:
- STT: `?model=qwen3-asr-flash-realtime`
- TTS: `?model=qwen3-tts-flash-realtime`
- A2A: `?model=qwen3-omni-flash-realtime`

---

## Authentication

### Bearer Token

```
Authorization: Bearer <DASHSCOPE_API_KEY>
```

**Additional Headers:**
- `OpenAI-Beta: realtime=v1` (for Qwen3-ASR realtime)
- `X-DashScope-WorkSpace: <workspace_id>` (optional)
- `X-DashScope-DataInspection: enable` (optional)

### API Key Sources

1. **International:** https://dashscope-intl.console.aliyun.com/
2. **China:** https://dashscope.console.aliyun.com/

---

## STT Models

### 1. Qwen3-ASR-Flash-Realtime (Recommended)

Latest Qwen3 model for real-time streaming recognition.

**Model IDs:**
- `qwen3-asr-flash-realtime` (stable)
- `qwen3-asr-flash-realtime-2025-10-27` (snapshot)

**Supported Languages:**
Chinese (Mandarin, Sichuanese, Minnan, Wu, Cantonese), English, Japanese, German, Korean, Russian, French, Portuguese, Arabic, Italian, Spanish, Hindi, Indonesian, Thai, Turkish, Ukrainian, Vietnamese, Czech, Danish, Filipino, Finnish, Icelandic, Malay, Norwegian, Polish, Swedish

**Audio Configuration:**
- Sample rates: 8kHz, 16kHz
- Formats: PCM, Opus
- Channels: Mono

**Features:**
- Server VAD (voice activity detection)
- Emotion recognition
- Context biasing (up to 10,000 tokens)
- Turn detection

### 2. Paraformer-Realtime-V2

Multilingual real-time ASR with extensive format support.

**Model IDs:**
- `paraformer-realtime-v2`
- `paraformer-realtime-8k-v2` (telephony)

**Supported Audio Formats:**
PCM, WAV, MP3, Opus, Speex, AAC, AMR-NB

**Features:**
- Vocabulary hotwords
- Disfluency removal
- Semantic punctuation
- Word-level timestamps

---

## TTS Models

### 1. Qwen3-TTS-Flash-Realtime (Recommended)

Latest Qwen3 TTS for real-time streaming synthesis.

**Model IDs:**
- `qwen3-tts-flash-realtime` (stable)
- `qwen3-tts-flash-realtime-2025-09-18` (snapshot)

**Voices:** 49 system voices

**Sample Voices:**
- Cherry, Serena, Ethan, Jennifer, Ryan, Neil, Elias
- Regional dialects: Shanghai-Jada, Beijing-Dylan, Cantonese-Kiki

**Audio Configuration:**
- Sample rates: 8kHz, 16kHz, 24kHz, 48kHz
- Formats: PCM, WAV, MP3, Opus

**Parameters:**
- `speed`: Speech rate
- `pitch`: Pitch adjustment
- `volume`: Volume level
- `bitrate`: Audio quality

### 2. CosyVoice

Premium TTS with voice cloning and SSML support.

**Model IDs:**
- `cosyvoice-v3-plus` (premium)
- `cosyvoice-v3-flash` (fast)
- `cosyvoice-v2` (legacy)

**Features:**
- Voice cloning (instant)
- SSML support
- Word timestamps
- AIGC tagging
- Language hints

**Parameters:**
- `format`: pcm, wav, mp3, opus
- `sample_rate`: 8000-48000 Hz
- `volume`: 0-100
- `rate`: 0.5-2.0 (speed)
- `pitch`: 0.5-2.0

---

## Audio-to-Audio Models

### Qwen3-Omni-Flash-Realtime

Full-duplex real-time multimodal AI.

**Model IDs:**
- `qwen3-omni-flash-realtime` (49 voices)
- `qwen3-omni-turbo-realtime` (17 voices)

**Audio Format:**
- Input: PCM16 @ 16kHz
- Output: PCM24 @ 24kHz (flash) / PCM16 (turbo)

**Interaction Modes:**
1. **VAD Mode:** Server-side voice activity detection
2. **Manual Mode:** Push-to-talk style control

**Features:**
- Full-duplex audio
- Emotion analysis
- Turn detection
- Video input support
- System instructions

**Limits:**
- Max session: 30 minutes
- Rate limit: 20 requests/second

---

## WebSocket Message Formats

### STT (Qwen3-ASR-Flash-Realtime)

#### Session Update (Client → Server)
```json
{
  "type": "session.update",
  "session": {
    "modalities": ["text"],
    "input_audio_format": "pcm16",
    "input_audio_transcription": {
      "sample_rate": 16000,
      "language": "zh"
    },
    "turn_detection": {
      "type": "server_vad",
      "silence_duration_ms": 400
    }
  }
}
```

#### Audio Buffer Append (Client → Server)
```json
{
  "type": "input_audio_buffer.append",
  "audio": "<base64_encoded_audio>"
}
```

#### Transcript Response (Server → Client)
```json
{
  "type": "conversation.item.input_audio_transcription.completed",
  "transcript": "你好世界"
}
```

### STT (Paraformer)

#### Run Task (Client → Server)
```json
{
  "header": {
    "action": "run-task",
    "task_id": "<uuid>",
    "streaming": "duplex"
  },
  "payload": {
    "model": "paraformer-realtime-v2",
    "task": "asr",
    "task_group": "audio",
    "function": "recognition",
    "input": {},
    "parameters": {
      "format": "pcm",
      "sample_rate": 16000,
      "disfluency_removal_enabled": true,
      "punctuation_prediction_enabled": true
    }
  }
}
```

#### Result Generated (Server → Client)
```json
{
  "header": {
    "task_id": "<uuid>",
    "event": "result-generated"
  },
  "payload": {
    "output": {
      "sentence": {
        "begin_time": 0,
        "end_time": 1500,
        "text": "你好世界",
        "words": [
          {"begin_time": 0, "end_time": 500, "text": "你好"},
          {"begin_time": 500, "end_time": 1500, "text": "世界"}
        ],
        "sentence_end": true
      }
    }
  }
}
```

### TTS (Qwen3-TTS-Flash-Realtime)

#### Session Update (Client → Server)
```json
{
  "type": "session.update",
  "session": {
    "voice": "Cherry",
    "response_format": "pcm16",
    "sample_rate": 24000,
    "mode": "server_commit"
  }
}
```

#### Input Text Append (Client → Server)
```json
{
  "type": "input_text_buffer.append",
  "text": "你好世界"
}
```

#### Input Text Commit (Client → Server)
```json
{
  "type": "input_text_buffer.commit"
}
```

#### Audio Delta (Server → Client)
```json
{
  "type": "response.audio.delta",
  "delta": "<base64_encoded_audio>"
}
```

### TTS (CosyVoice)

#### Run Task (Client → Server)
```json
{
  "header": {
    "action": "run-task",
    "task_id": "<uuid>",
    "streaming": "out"
  },
  "payload": {
    "model": "cosyvoice-v3-plus",
    "task": "tts",
    "task_group": "audio",
    "function": "SpeechSynthesizer",
    "input": {},
    "parameters": {
      "voice": "longanyang",
      "format": "mp3",
      "sample_rate": 22050,
      "volume": 50,
      "rate": 1.0,
      "pitch": 1.0
    }
  }
}
```

#### Continue Task - Send Text (Client → Server)
```json
{
  "header": {
    "action": "continue-task",
    "task_id": "<uuid>",
    "streaming": "out"
  },
  "payload": {
    "input": {
      "text": "你好世界"
    }
  }
}
```

#### Finish Task (Client → Server)
```json
{
  "header": {
    "action": "finish-task",
    "task_id": "<uuid>",
    "streaming": "out"
  },
  "payload": {
    "input": {}
  }
}
```

---

## Pricing

### International Region (Singapore)

| Service | Price | Free Tier |
|---------|-------|-----------|
| STT (Qwen-ASR) | $0.000035/second | 36,000 seconds |
| STT (Qwen-ASR Realtime) | $0.000090/second | 36,000 seconds |
| TTS (Qwen) | $0.01/1,000 chars | 10,000 chars |
| TTS (Qwen Realtime) | $0.013/1,000 chars | 10,000 chars |
| TTS (CosyVoice Plus) | $0.0287/1,000 chars | 2,000 chars |
| TTS (CosyVoice Flash) | $0.0143/1,000 chars | 2,000 chars |
| Voice Cloning | $0.01/voice | 1,000 voices |
| Voice Design | $0.2/voice | 10 voices |

### China Region (Beijing)

| Service | Price |
|---------|-------|
| STT (Qwen-ASR) | $0.000032/second |
| STT (Qwen-ASR Realtime) | $0.000047/second |
| TTS (Qwen) | $0.0115/1,000 chars |
| TTS (Qwen Realtime) | $0.0143/1,000 chars |

### Character Counting

- Chinese/Kanji/Hanja: 2 characters
- English/Punctuation/Space: 1 character
- SSML tags: Not counted

---

## Constraints & Limits

| Parameter | Limit |
|-----------|-------|
| TTS chars per request | 300 |
| TTS chars per task | 200,000 |
| STT rate limit | 20 req/sec |
| Session max duration | 30 minutes |
| WebSocket timeout | 23 seconds |
| Audio chunk size | 3,200 bytes (~0.1s) |
| Context biasing tokens | 10,000 |

---

## Error Codes

| Code | Description |
|------|-------------|
| 400 | Bad request (invalid parameters) |
| 401 | Unauthorized (invalid API key) |
| 403 | Forbidden (region/quota issue) |
| 429 | Rate limit exceeded |
| 500 | Internal server error |

---

## Implementation Plan

### Architecture

```
alibaba_cloud/
├── mod.rs           # Module exports
├── config.rs        # DashScopeConfig, Region, Model enums
├── auth.rs          # Bearer token authentication
├── messages.rs      # WebSocket message types (shared)
├── stt/
│   ├── mod.rs
│   ├── qwen_asr.rs  # Qwen3-ASR-Flash-Realtime
│   └── paraformer.rs # Paraformer-Realtime
└── tts/
    ├── mod.rs
    ├── qwen_tts.rs  # Qwen3-TTS-Flash-Realtime
    └── cosyvoice.rs # CosyVoice models
```

### Implementation Priority

1. **Phase 1:** Qwen3-ASR-Flash-Realtime (STT)
2. **Phase 2:** Qwen3-TTS-Flash-Realtime (TTS)
3. **Phase 3:** Paraformer-Realtime (STT alternative)
4. **Phase 4:** CosyVoice (TTS with cloning)
5. **Phase 5:** Qwen-Omni-Realtime (Audio-to-Audio)

### Key Implementation Details

1. **WebSocket Connection:**
   - Use `tokio-tungstenite` for async WebSocket
   - Model specified in URL query parameter
   - Bearer token in Authorization header

2. **Message Protocol:**
   - Qwen models: OpenAI-like realtime format
   - Paraformer/CosyVoice: DashScope inference format

3. **Audio Handling:**
   - STT: Binary frames for audio, JSON for control
   - TTS: JSON for requests, binary or base64 for audio

4. **Error Handling:**
   - Map task-failed events to TTSError/STTError
   - Handle WebSocket close codes
   - Automatic reconnection on transient errors

---

## Testing Plan

### Unit Tests

1. Config parsing and validation
2. Message serialization/deserialization
3. Authentication header generation
4. URL construction with model parameters
5. Voice enum mapping
6. Region endpoint selection

### Integration Tests

1. WebSocket connection establishment
2. STT streaming recognition flow
3. TTS streaming synthesis flow
4. Error handling scenarios
5. Reconnection behavior

### Live API Tests (with credentials)

```bash
DASHSCOPE_API_KEY=xxx cargo test alibaba_cloud -- --ignored --nocapture
```

---

## References

- [DashScope Model Studio Documentation](https://www.alibabacloud.com/help/en/model-studio)
- [Qwen-ASR Realtime API](https://www.alibabacloud.com/help/en/model-studio/qwen-real-time-speech-recognition)
- [Qwen-TTS Realtime API](https://www.alibabacloud.com/help/en/model-studio/qwen-tts-realtime)
- [Paraformer WebSocket API](https://www.alibabacloud.com/help/en/model-studio/websocket-for-paraformer-real-time-service)
- [CosyVoice WebSocket API](https://www.alibabacloud.com/help/en/model-studio/cosyvoice-websocket-api)
- [Qwen-Omni Realtime](https://www.alibabacloud.com/help/en/model-studio/realtime)
- [Model Studio Pricing](https://www.alibabacloud.com/help/en/model-studio/billing-for-model-studio)
