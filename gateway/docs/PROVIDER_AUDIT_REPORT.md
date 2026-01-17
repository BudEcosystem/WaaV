# WaaV Gateway Provider Audit Report

> **Audit Date:** 2026-01-17
> **Audited By:** Automated API Documentation Comparison
> **Total Providers Audited:** 46 (27 STT, 32 TTS, 2 Realtime)

This report documents the comparison between current WaaV Gateway provider implementations and their official API documentation, identifying missing features, mismatches, and recommended updates.

---

## Executive Summary

| Status | Count | Description |
|--------|-------|-------------|
| **Complete** | 34 | Implementations match current API documentation |
| **Partially Complete** | 8 | Missing recent features (1-3 features) |
| **Needs Update** | 4 | Missing multiple features (4+ features) |

### Priority Updates Required

| Provider | Priority | Missing Features |
|----------|----------|-----------------|
| **OpenAI STT** | HIGH | Diarization model, logprobs, Realtime transcription |
| **ElevenLabs STT** | HIGH | Keyterm prompting, Entity recognition, Speaker diarization |
| **Deepgram STT** | MEDIUM | Nova-3 Medical, Flux model, PHI redaction |
| **Hume AI** | MEDIUM | Octave 2, EVI 4 mini, Voice conversion |

---

## Detailed Findings

### 1. Deepgram STT

**Status:** Partially Complete

**Current Implementation:**
- Models: `nova-3`, `nova-2`, `nova`, `enhanced`, `base`
- Features: Diarization, interim results, filler words, profanity filter, smart format, keywords, redaction, VAD events, endpointing

**Missing Features (per Deepgram Changelog 2025-2026):**

| Feature | API Support | Status | Priority |
|---------|-------------|--------|----------|
| `nova-3-medical` model | Yes | **MISSING** | HIGH |
| `flux` model (voice agents) | Yes | **MISSING** | MEDIUM |
| PHI redaction (`phi` param) | Yes | **MISSING** | HIGH |
| EU endpoint (`api.eu.deepgram.com`) | Yes | **MISSING** | MEDIUM |
| Keyterm prompting (multilingual) | Yes | Partial | LOW |

**Recommendation:** Add `nova-3-medical`, `flux` models and PHI redaction support.

**Sources:** [Deepgram Changelog](https://developers.deepgram.com/changelog), [Nova-3 Announcement](https://deepgram.com/learn/introducing-nova-3-speech-to-text-api)

---

### 2. OpenAI STT

**Status:** Needs Update

**Current Implementation:**
- Models: `whisper-1`, `gpt-4o-transcribe`, `gpt-4o-mini-transcribe`
- Features: Response formats, timestamps, temperature, language, prompt

**Missing Features (per OpenAI API Reference 2025-2026):**

| Feature | API Support | Status | Priority |
|---------|-------------|--------|----------|
| `gpt-4o-transcribe-diarize` model | Yes | **MISSING** | HIGH |
| `logprobs` parameter | Yes | **MISSING** | MEDIUM |
| Realtime API transcription | Yes | **MISSING** | HIGH |
| `chunking_strategy` (VAD config) | Yes | **MISSING** | HIGH |
| `known_speaker_names[]` | Yes | **MISSING** | MEDIUM |
| `known_speaker_references[]` | Yes | **MISSING** | MEDIUM |
| `diarized_json` response format | Yes | **MISSING** | HIGH |
| Streaming SSE support | Yes | **MISSING** | MEDIUM |

**Recommendation:** Add diarization model with speaker identification support and Realtime API transcription mode.

**Sources:** [OpenAI Audio API Reference](https://platform.openai.com/docs/api-reference/audio), [Realtime Transcription](https://platform.openai.com/docs/guides/realtime-transcription)

---

### 3. ElevenLabs STT

**Status:** Needs Update

**Current Implementation:**
- Models: `scribe_v2_realtime`
- Features: VAD-based commit, word timestamps, regional endpoints

**Missing Features (per ElevenLabs Docs 2025-2026):**

| Feature | API Support | Status | Priority |
|---------|-------------|--------|----------|
| Keyterm prompting (100 terms) | Yes | **MISSING** | HIGH |
| Entity recognition (56 categories) | Yes | **MISSING** | HIGH |
| Speaker diarization (48 speakers) | Yes | **MISSING** | HIGH |
| PII/Health data detection | Yes | **MISSING** | MEDIUM |
| Scribe v2 batch (non-realtime) | Yes | **MISSING** | LOW |

**Recommendation:** Add keyterm prompting and entity recognition parameters to the Scribe v2 implementation.

**Sources:** [ElevenLabs Scribe v2](https://elevenlabs.io/docs/overview/capabilities/speech-to-text), [Scribe v2 Realtime Launch](https://elevenlabs.io/blog/introducing-scribe-v2-realtime)

---

### 4. AssemblyAI STT

**Status:** Partially Complete

**Current Implementation:**
- Models: `universal-streaming-english`, `universal-streaming-multilingual`
- Features: Real-time streaming, end-of-turn detection, word timestamps

**Missing Features (per AssemblyAI Changelog 2025-2026):**

| Feature | API Support | Status | Priority |
|---------|-------------|--------|----------|
| Keyterm prompting (streaming) | Yes | **MISSING** | MEDIUM |
| Voice AI Guardrails | Yes | **MISSING** | LOW |
| Speech Understanding API | Yes | **MISSING** | LOW |
| LLM Gateway integration | Yes | **MISSING** | LOW |
| Multilingual streaming (6 langs) | Yes | Partial | MEDIUM |

**Note:** Legacy `/v2/realtime/ws` deprecated Jan 31, 2026. Current implementation uses Universal-Streaming.

**Sources:** [AssemblyAI October 2025 Releases](https://www.assemblyai.com/blog/assemblyai-october-2025-releases)

---

### 5. Cartesia TTS

**Status:** Partially Complete

**Current Implementation:**
- Models: `sonic-3` (with dated snapshots support)
- Features: Voice selection, audio formats, speed control

**Missing Features (per Cartesia Docs 2025-2026):**

| Feature | API Support | Status | Priority |
|---------|-------------|--------|----------|
| `[laughter]` tags | Yes | **MISSING** | MEDIUM |
| Voice conversion | Yes | **MISSING** | LOW |
| Phoneme editing | Yes | **MISSING** | LOW |
| Emotion tags | Yes | **MISSING** | MEDIUM |
| Volume control | Yes | **MISSING** | LOW |

**Recommendation:** Add emotion/laughter tag support to leverage Sonic-3's expressive capabilities.

**Sources:** [Cartesia Sonic-3](https://docs.cartesia.ai/build-with-cartesia/tts-models/latest)

---

### 6. Hume AI TTS/EVI

**Status:** Partially Complete

**TTS Current Implementation:**
- Features: Natural language emotion control, instant_mode, speed control, voice cloning

**TTS Missing Features (per Hume Changelog 2025-2026):**

| Feature | API Support | Status | Priority |
|---------|-------------|--------|----------|
| Octave 2 model | Yes | **MISSING** | MEDIUM |
| Voice conversion | Yes | **MISSING** | LOW |
| Phoneme editing | Yes | **MISSING** | LOW |
| TTS WebSocket endpoint | Yes | **MISSING** | MEDIUM |

**EVI Current Implementation:**
- Versions: EVI 3, EVI 2, EVI 1
- Features: 48 emotion dimensions, prosody analysis

**EVI Missing Features:**

| Feature | API Support | Status | Priority |
|---------|-------------|--------|----------|
| EVI 4 mini | Yes | **MISSING** | MEDIUM |
| Nudges (silence filling) | Yes | **MISSING** | LOW |
| Gemini 2.5 flash LLM | Yes | **MISSING** | LOW |

**Sources:** [Hume Octave 2 Launch](https://www.hume.ai/blog/octave-2-launch), [Hume Changelog](https://dev.hume.ai/changelog)

---

### 7. Play.ht TTS

**Status:** Complete

**Current Implementation:**
- Models: `Play3.0-mini`, `PlayDialog`, `PlayDialogMultilingual`, `PlayDialogArabic`, `PlayHT2.0-turbo`
- Features: Voice cloning, multi-turn dialogue, voice_2, turn_prefix, temperature, speed

**All documented features implemented.**

**Note:** PlayHT 1.0 EOL June 15, 2025 - implementation correctly uses newer models.

---

### 8. Google Cloud STT

**Status:** Complete

**Current Implementation:**
- API: Speech-to-Text v2 (gRPC)
- Models: `latest_long`, `latest_short`
- Features: 125+ languages, speaker diarization, word timestamps, VAD events

**All documented features implemented.**

---

### 9. Microsoft Azure Speech Services

**Status:** Complete

**Current Implementation:**
- STT: WebSocket streaming, 100+ languages
- TTS: 400+ voices, SSML support
- Features: Custom speech models, pronunciation assessment

**All documented features implemented.**

---

### 10. AWS Transcribe/Polly

**Status:** Complete

**Current Implementation:**
- Transcribe: 100+ languages, streaming, diarization, PII redaction
- Polly: Neural/Standard/Long-form/Generative engines

**All documented features implemented.**

---

### 11. IBM Watson Speech Services

**Status:** Complete

**Current Implementation:**
- STT: WebSocket, 30+ languages, IAM auth, speaker labels
- TTS: HTTP, V3 neural voices, SSML

**All documented features implemented.**

---

### 12. Groq Whisper

**Status:** Complete

**Current Implementation:**
- Models: `whisper-large-v3`, `whisper-large-v3-turbo`
- Features: 216x real-time speed, translation, silence detection

**All documented features implemented.**

---

### 13. LMNT TTS

**Status:** Complete

**Current Implementation:**
- Features: Voice cloning (5s audio), HTTP streaming, 22+ languages
- Parameters: top_p, temperature, speed, seed

**All documented features implemented.**

---

### 14. Resemble AI TTS

**Status:** Complete

**Current Implementation:**
- Models: Chatterbox, Chatterbox-Turbo, Chatterbox-Multilingual
- Features: Voice cloning, 149+ languages, paralinguistic tags

**All documented features implemented.**

---

## Regional Provider Status

### China & East Asia

| Provider | Status | Notes |
|----------|--------|-------|
| **iFlytek** | Complete | HMAC-SHA256 auth, 30+ languages |
| **Alibaba Cloud** | Complete | DashScope API, Qwen3-ASR, CosyVoice |
| **Baidu AI** | Complete | OAuth 2.0, Chinese dialects |
| **Tencent Cloud** | Complete | HMAC-SHA1 auth, STT+TTS |
| **Huawei Cloud** | Complete | IAM auth, multiple modes |
| **NAVER CLOVA** | Complete | Korean optimization |
| **AmiVoice** | Complete | Japanese specialty |

### India

| Provider | Status | Notes |
|----------|--------|-------|
| **Sarvam AI** | Complete | Saarika v2.5, 11 Indian languages |
| **Gnani.ai** | Complete | gRPC, 14 Indic languages |
| **Reverie** | Complete | 22 Indian languages, code-mixing |
| **Bhashini** | Complete | ULCA API, government service |

### Southeast Asia

| Provider | Status | Notes |
|----------|--------|-------|
| **Zalo AI** | Complete | Vietnamese specialty |
| **FPT.AI** | Complete | Vietnamese STT+TTS |
| **Viettel AI** | Complete | 96% Vietnamese accuracy |
| **Prosa.ai** | Complete | Indonesian NLP |
| **NECTEC** | Complete | Thai government AI |

### Russia/CIS

| Provider | Status | Notes |
|----------|--------|-------|
| **Yandex SpeechKit** | Complete | Russian optimization, emotions |
| **Tinkoff VoiceKit** | Complete | gRPC, Russian specialty |
| **SberDevices** | Complete | SaluteSpeech, SSML |

### European

| Provider | Status | Notes |
|----------|--------|-------|
| **Speechmatics** | Complete | 55+ languages, Enhanced mode |
| **Gladia** | Complete | <300ms latency, code-switching |
| **Rev AI** | Complete | 9 streaming languages |
| **Phonexia** | Complete | On-premises, 57-64 languages |
| **Acapela Group** | Complete | 250+ voices, visemes |
| **Cereproc** | Complete | Celtic languages |

---

## Recommendations Summary

### Immediate Priority (Next Sprint)

1. **OpenAI STT**: Add `gpt-4o-transcribe-diarize` model and diarization support
2. **ElevenLabs STT**: Add keyterm prompting and entity recognition
3. **Deepgram STT**: Add `nova-3-medical` and PHI redaction

### Medium Priority (Next Release)

4. **Deepgram STT**: Add `flux` model for voice agents
5. **Cartesia TTS**: Add `[laughter]` and emotion tags
6. **Hume AI**: Update to Octave 2 and EVI 4 mini
7. **AssemblyAI**: Add keyterm prompting for streaming

### Low Priority (Backlog)

8. Voice conversion features (Cartesia, Hume)
9. Phoneme editing (Cartesia, Hume)
10. LLM Gateway integrations (AssemblyAI)

---

## Implementation Notes

### OpenAI Diarization Model
```rust
// Add to src/core/stt/openai/config.rs
#[serde(rename = "gpt-4o-transcribe-diarize")]
Gpt4oTranscribeDiarize,

// Add diarization config
pub chunking_strategy: Option<ChunkingStrategy>,
pub known_speaker_names: Option<Vec<String>>,
pub known_speaker_references: Option<Vec<AudioReference>>,
```

### ElevenLabs Keyterm Prompting
```rust
// Add to src/core/stt/elevenlabs/config.rs
pub keyterms: Option<Vec<String>>, // max 100 terms
pub enable_entity_detection: Option<bool>,
pub max_speakers: Option<u8>, // up to 48
```

### Deepgram PHI Redaction
```rust
// Add to existing redaction enum
pub enum RedactionType {
    Pii,
    Numbers,
    Ssn,
    Phi, // NEW: Protected Health Information
}
```

---

## Version History

- **2026-01-17**: Initial comprehensive audit of 46 providers
- **Next Review**: 2026-04-17 (Quarterly)

---

## Sources

- [Deepgram Docs](https://developers.deepgram.com/docs)
- [OpenAI API Reference](https://platform.openai.com/docs/api-reference)
- [ElevenLabs Documentation](https://elevenlabs.io/docs)
- [AssemblyAI Changelog](https://www.assemblyai.com/changelog)
- [Cartesia Docs](https://docs.cartesia.ai)
- [Hume AI Changelog](https://dev.hume.ai/changelog)
- [Play.ht Docs](https://docs.play.ht)
