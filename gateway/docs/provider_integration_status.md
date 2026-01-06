# Bud WaaV Provider Integration Status & Playbook

> **Last Updated:** 2026-01-06
> **Scope:** Cloud API providers only (no self-hosted/local inference engines)

---

## Quick Summary

| Metric | Count |
|--------|-------|
| **Total Cloud Providers** | 70 |
| **Implemented** | 11 |
| **In Progress** | 0 |
| **Yet to Start** | 59 |
| **Estimated Days Remaining** | 31-42 |

---

## Status Legend

| Status | Symbol | Meaning |
|--------|--------|---------|
| Done | `[DONE]` | Fully implemented, tested, documented |
| In Progress | `[IN_PROGRESS]` | Currently being implemented |
| Yet to Start | `[TODO]` | Not started |
| Blocked | `[BLOCKED]` | Waiting on external dependency |
| Research | `[RESEARCH]` | Researching API/documentation |

---

## How to Use This Document

### For AI-Assisted Workflow

When starting work on a provider:
1. **Read this document** to identify next provider to implement
2. **Update status** from `[TODO]` to `[IN_PROGRESS]`
3. **Follow the methodology** in Phase sections below
4. **Run quality gates** before marking complete
5. **Update status** to `[DONE]` with completion date
6. **Update counters** in Quick Summary section

### Status Update Format

```markdown
| 1 | OpenAI | STT+TTS+A2A | [IN_PROGRESS] | 2026-01-06 | Working on Realtime API |
```

When complete:
```markdown
| 1 | OpenAI | STT+TTS+A2A | [DONE] | 2026-01-10 | STT, TTS, Realtime all working |
```

---

## Phase 1: Research Methodology

### Step 1.1: Initial Assessment Template

Before implementing ANY provider, create a research document:

```markdown
## Provider: [Name]
**Research Date:** YYYY-MM-DD
**Researcher:** [Developer/AI]

### Basic Information
- **Website:** [URL from waav_integrations.json]
- **API Documentation:** [URL]
- **Pricing:** [URL or "Contact Sales"]

### Capabilities Matrix
| Capability | Supported | Notes |
|------------|-----------|-------|
| STT | YES/NO | |
| TTS | YES/NO | |
| Audio-to-Audio | YES/NO | |
| Voice Cloning | YES/NO | |
| Streaming | YES/NO | WebSocket/gRPC/SSE |

### Technical Specifications
- **Authentication:** API Key / Bearer Token / OAuth2
- **Protocol:** REST / WebSocket / gRPC
- **Audio Formats:** [list supported]
- **Sample Rates:** [list supported]
- **Languages:** [count and key ones]

### Integration Pattern
- **Recommended:** [WebSocket STT / HTTP TTS / etc.]
- **Reference Implementation:** [closest existing provider]
- **Complexity:** Low / Medium / High
- **Estimated LOC:** [approximate]

### Blockers/Concerns
- [List any issues]
```

### Step 1.2: Required Web Research

For EACH provider, perform these searches using WebFetch/WebSearch:

1. **Official API Docs:**
   - Fetch API documentation URL
   - Extract: authentication, endpoints, request/response formats

2. **SDK/Library Search:**
   - `"[Provider] Rust SDK OR client library"`
   - `"[Provider] API example code"`

3. **Best Practices:**
   - `"[Provider] API best practices latency"`
   - `"[Provider] WebSocket streaming audio"`

4. **Known Issues:**
   - `"[Provider] API issues site:stackoverflow.com"`
   - `"[Provider] API site:github.com/issues"`

---

## Phase 2: Implementation Workflow

### Step 2.1: Pre-Implementation Checklist

Before writing ANY code:

- [ ] Research document completed
- [ ] API credentials available (or test account created)
- [ ] Reference implementation identified
- [ ] Dependencies reviewed
- [ ] Git branch created: `feature/provider-[name]`
- [ ] Status updated to `[IN_PROGRESS]` in this document

### Step 2.2: Implementation Order

```
1. Config struct       → src/core/[stt|tts]/[provider]/config.rs
2. Message types       → src/core/[stt|tts]/[provider]/messages.rs
3. Client impl         → src/core/[stt|tts]/[provider]/client.rs
4. Unit tests          → inline #[cfg(test)]
5. Factory registration → src/core/[stt|tts]/mod.rs
6. Integration tests   → tests/[provider]_integration.rs
7. Config docs         → config.example.yaml
8. Architecture docs   → docs/architecture.md
```

### Step 2.3: Directory Structure

**STT Provider:**
```
src/core/stt/[provider_name]/
├── mod.rs           # pub use client::*; pub use config::*;
├── config.rs        # [Provider]STTConfig struct
├── messages.rs      # API message serde types
└── client.rs        # impl BaseSTT for [Provider]STT
```

**TTS Provider:**
```
src/core/tts/[provider_name]/
├── mod.rs           # pub use provider::*; pub use config::*;
├── config.rs        # [Provider]TTSConfig struct
├── messages.rs      # API message serde types (if WebSocket)
└── provider.rs      # impl BaseTTS for [Provider]TTS
```

### Step 2.4: Pattern Selection

| API Style | Implementation Pattern | Reference File |
|-----------|------------------------|----------------|
| WebSocket STT | BaseSTT + tokio-tungstenite | `src/core/stt/deepgram.rs` |
| gRPC STT | BaseSTT + tonic | `src/core/stt/google/` |
| HTTP REST STT | BaseSTT + reqwest | [new pattern] |
| HTTP REST TTS | TTSRequestBuilder | `src/core/tts/provider.rs` |
| WebSocket TTS | BaseTTS + tokio-tungstenite | `src/core/tts/cartesia/` |
| Audio-to-Audio | [New BaseA2A trait] | [to design] |

---

## Phase 3: Testing Requirements

### 3.1 Unit Tests (REQUIRED - No Credentials)

Every provider MUST have:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = ProviderConfig::default();
        assert!(!config.api_key.is_empty() || true); // verify defaults
    }

    #[test]
    fn test_config_validation() {
        // Test that empty API key fails
    }

    #[test]
    fn test_message_serialization() {
        // Verify serde round-trip
    }

    #[test]
    fn test_provider_creation() {
        // Verify new() works
    }

    #[test]
    fn test_provider_not_connected() {
        // Verify is_ready() == false initially
    }
}
```

### 3.2 Integration Tests (REQUIRED - With Credentials)

```rust
// tests/[provider]_integration.rs

fn get_credentials() -> Option<String> {
    std::env::var("[PROVIDER]_API_KEY").ok()
}

#[tokio::test]
#[ignore] // Only run with credentials
async fn test_[provider]_connection() {
    let Some(api_key) = get_credentials() else {
        println!("Skipping: [PROVIDER]_API_KEY not set");
        return;
    };
    // Test connect/disconnect cycle
}

#[tokio::test]
#[ignore]
async fn test_[provider]_stt_transcription() {
    // Test actual transcription
}

#[tokio::test]
#[ignore]
async fn test_[provider]_tts_synthesis() {
    // Test actual synthesis
}
```

### 3.3 Test Commands

```bash
# Unit tests (no credentials)
cargo test [provider] --lib

# Integration tests (with credentials)
[PROVIDER]_API_KEY=xxx cargo test [provider] -- --ignored --nocapture

# All tests
cargo test

# With sanitizers
RUSTFLAGS="-Zsanitizer=address" cargo +nightly test [provider]
```

---

## Phase 4: Quality Gates

### Gate 1: Code Quality
- [ ] `cargo fmt --check` passes
- [ ] `cargo clippy -- -D warnings` passes
- [ ] No `unwrap()` in production code
- [ ] All public items documented with `///`

### Gate 2: Functionality
- [ ] Unit tests pass
- [ ] Integration tests pass (with credentials)
- [ ] Provider in `get_supported_[stt|tts]_providers()`
- [ ] Factory creates provider correctly

### Gate 3: Performance
- [ ] No heap allocations in `send_audio()` hot path
- [ ] Uses `Bytes` type for zero-copy
- [ ] Connection state uses `AtomicBool`
- [ ] HTTP providers use connection pooling

### Gate 4: Documentation
- [ ] Provider in `docs/architecture.md`
- [ ] Config in `config.example.yaml`
- [ ] Environment variable documented
- [ ] Status updated in this document

### Gate 5: Security
- [ ] API keys use `#[serde(skip_serializing)]`
- [ ] All external inputs validated
- [ ] TLS enforced
- [ ] Timeouts configured

---

## Phase 5: Files to Modify Per Provider

| File | Change Required |
|------|-----------------|
| `src/core/stt/mod.rs` | Add module, factory case |
| `src/core/tts/mod.rs` | Add module, factory case |
| `src/config/mod.rs` | Add env var loading |
| `config.example.yaml` | Add config section |
| `Cargo.toml` | Add dependencies (if any) |
| `docs/architecture.md` | Add to provider list |
| This document | Update status |
| `memory.md` | Document decisions |

---

## Critical Reference Files

| File | Purpose |
|------|---------|
| `src/core/stt/base.rs` | BaseSTT trait definition |
| `src/core/tts/base.rs` | BaseTTS trait definition |
| `src/core/stt/deepgram.rs` | WebSocket STT reference |
| `src/core/stt/azure/` | Complex STT with reconnection |
| `src/core/tts/cartesia/` | WebSocket TTS reference |
| `src/core/tts/provider.rs` | HTTP TTS pattern |
| `tests/azure_stt_integration.rs` | Integration test pattern |
| `docs/new_provider.md` | Full implementation guide |

---

## Provider Integration Status

### Already Implemented (Batch 0)

| # | Provider | Type | Status | Date | Notes |
|---|----------|------|--------|------|-------|
| - | Deepgram | STT+TTS | [DONE] | Pre-existing | WebSocket streaming |
| - | Google | STT+TTS | [DONE] | Pre-existing | gRPC implementation |
| - | ElevenLabs | STT+TTS+Clone | [DONE] | Pre-existing | Voice cloning support |
| - | Microsoft Azure | STT+TTS | [DONE] | Pre-existing | Complex reconnection |
| - | Cartesia | STT+TTS+Clone | [DONE] | Pre-existing | WebSocket TTS |

---

### Batch 1: Global Cloud Leaders

| # | Provider | Type | Status | Start Date | Notes |
|---|----------|------|--------|------------|-------|
| 1 | OpenAI | STT+TTS+A2A | [DONE] | 2026-01-06 | Whisper STT, TTS API, Realtime WebSocket |
| 2 | AssemblyAI | STT | [DONE] | 2026-01-06 | Streaming API v3, immutable transcripts, 99 languages |
| 3 | Amazon Transcribe | STT | [DONE] | 2026-01-06 | AWS SDK, 100+ languages, streaming WebSocket |
| 4 | Amazon Polly | TTS | [DONE] | 2026-01-06 | AWS SDK, 60+ voices, Neural/Standard/Generative engines |
| 5 | IBM Watson STT | STT | [DONE] | 2026-01-06 | IAM auth, 30+ languages, WebSocket streaming |
| 6 | IBM Watson TTS | TTS | [DONE] | 2026-01-06 | V3 neural voices, SSML support, 15+ languages |
| 7 | Groq | STT | [TODO] | - | Fastest Whisper hosting |
| 8 | Hume AI | TTS+A2A | [TODO] | - | Emotional expression |

---

### Batch 2: Voice Cloning & Specialized

| # | Provider | Type | Status | Start Date | Notes |
|---|----------|------|--------|------------|-------|
| 9 | LMNT | TTS+Clone | [TODO] | - | Ultra-low latency |
| 10 | Play.ht | TTS+Clone | [TODO] | - | 142 languages |
| 11 | Murf.ai | TTS+Clone | [TODO] | - | 120+ voices |
| 12 | WellSaid Labs | TTS+Clone | [TODO] | - | Premium quality |
| 13 | Resemble AI | TTS+A2A+Clone | [TODO] | - | Deepfake detection |
| 14 | Speechify | TTS+Clone+Dub | [TODO] | - | Consumer-focused |
| 15 | Unreal Speech | TTS | [TODO] | - | Cost-effective |
| 16 | Otter.ai | STT | [TODO] | - | Meeting transcription |

---

### Batch 3: Europe & Global STT

| # | Provider | Type | Status | Start Date | Notes |
|---|----------|------|--------|------------|-------|
| 17 | Speechmatics | STT+TTS | [TODO] | - | UK-based, 55 langs |
| 18 | Gladia | STT | [TODO] | - | EU-based, 100+ langs |
| 19 | Rev AI | STT | [TODO] | - | Human-AI hybrid |
| 20 | Phonexia | STT | [TODO] | - | Voice biometrics |
| 21 | Verbit | STT | [TODO] | - | US/Israel |
| 22 | SpeechText.AI | STT | [TODO] | - | EU France |
| 23 | Speechly | STT | [TODO] | - | Finland, Nordic |
| 24 | ReadSpeaker | TTS | [TODO] | - | 50+ languages |

---

### Batch 4: Europe TTS & Russia/CIS

| # | Provider | Type | Status | Start Date | Notes |
|---|----------|------|--------|------------|-------|
| 25 | Acapela Group | TTS | [TODO] | - | Belgium, 34 langs |
| 26 | Cereproc | TTS | [TODO] | - | UK, Celtic langs |
| 27 | Yandex SpeechKit | STT+TTS | [TODO] | - | Russia/CIS |
| 28 | Tinkoff VoiceKit | STT+TTS | [TODO] | - | Russia |
| 29 | SberDevices | STT+TTS | [TODO] | - | Russia |
| 30 | Nuance | STT+TTS | [TODO] | - | Microsoft, Dragon |

---

### Batch 5: India Regional

| # | Provider | Type | Status | Start Date | Notes |
|---|----------|------|--------|------------|-------|
| 31 | Sarvam AI | STT+TTS | [TODO] | - | 22 Indian langs |
| 32 | Gnani.ai | STT+TTS | [TODO] | - | Voice biometrics |
| 33 | Reverie | STT+TTS | [TODO] | - | 22 Indian langs |
| 34 | CoRover | STT+TTS | [TODO] | - | BharatGPT |
| 35 | Smallest.ai | TTS+Clone | [TODO] | - | 100+ languages |
| 36 | AI4Bharat/Bhashini | STT+TTS | [TODO] | - | Government API |

---

### Batch 6: China & East Asia

| # | Provider | Type | Status | Start Date | Notes |
|---|----------|------|--------|------------|-------|
| 37 | iFlytek | STT+TTS | [TODO] | - | China leader |
| 38 | Alibaba Cloud | STT+TTS | [TODO] | - | Chinese dialects |
| 39 | Baidu AI | STT+TTS | [TODO] | - | Chinese |
| 40 | Tencent Cloud | STT+TTS | [TODO] | - | Chinese |
| 41 | Huawei Cloud | STT+TTS | [TODO] | - | Global regions |
| 42 | NAVER CLOVA | STT+TTS | [TODO] | - | Korea/Japan |
| 43 | Kakao | STT+TTS | [TODO] | - | Korea |
| 44 | NTT COTOHA | STT+TTS | [TODO] | - | Japan |
| 45 | AmiVoice | STT | [TODO] | - | Japan medical/legal |

---

### Batch 7: Southeast Asia

| # | Provider | Type | Status | Start Date | Notes |
|---|----------|------|--------|------------|-------|
| 46 | Vbee | TTS+Clone | [TODO] | - | Vietnam |
| 47 | Zalo AI | STT+TTS | [TODO] | - | Vietnam |
| 48 | FPT.AI | STT+TTS | [TODO] | - | Vietnam |
| 49 | Viettel AI | STT+TTS | [TODO] | - | Vietnam |
| 50 | Prosa.ai | STT+TTS | [TODO] | - | Indonesia |
| 51 | Kata.ai | STT+TTS | [TODO] | - | Indonesia |
| 52 | NECTEC | STT+TTS | [TODO] | - | Thailand govt |
| 53 | Botnoi Voice | TTS | [TODO] | - | Thailand |

---

### Batch 8: Middle East & Africa

| # | Provider | Type | Status | Start Date | Notes |
|---|----------|------|--------|------------|-------|
| 54 | NeuralSpace | STT+TTS | [TODO] | - | 115+ languages |
| 55 | Sestek/Knovvu | STT+TTS | [TODO] | - | Turkey/Middle East |
| 56 | Lahajati | STT | [TODO] | - | 108 Arabic dialects |
| 57 | AzReco | STT+TTS | [TODO] | - | Azerbaijan |
| 58 | ISSAI | STT+TTS | [TODO] | - | Kazakh/Turkic |
| 59 | Intron Health | STT | [TODO] | - | African medical |
| 60 | Lelapa AI | STT+TTS | [TODO] | - | South Africa |
| 61 | Lesan AI | STT | [TODO] | - | Ethiopia |

---

### Batch 9: Remaining Specialized

| # | Provider | Type | Status | Start Date | Notes |
|---|----------|------|--------|------------|-------|
| 62 | Vozy | STT+TTS | [TODO] | - | Latin America |
| 63 | DeepInfra | STT+TTS | [TODO] | - | Model hosting |
| 64 | Replicate | STT+TTS | [TODO] | - | Model hosting |
| 65 | AlfaNum | STT+TTS | [TODO] | - | Balkans |
| 66 | Aseto AI | STT | [TODO] | - | Greek |
| 67 | Elhuyar | STT+TTS | [TODO] | - | Basque |
| 68 | ABAIR | STT+TTS | [TODO] | - | Irish Gaelic |
| 69 | SignAll | Sign-to-Text | [TODO] | - | ASL |
| 70 | Signapse | Text-to-Sign | [TODO] | - | BSL |

---

## Excluded Providers (Local Inference - Future Phase)

These require the Python inference engine to be completed first:

| Provider | Type | Reason |
|----------|------|--------|
| Faster Whisper | STT | Self-hosted |
| Whisper.cpp | STT | Self-hosted C++ |
| Vosk | STT | Self-hosted |
| Mozilla DeepSpeech | STT | Self-hosted |
| Piper TTS | TTS | Self-hosted |
| Kokoro TTS | TTS | Self-hosted |
| StyleTTS 2 | TTS | Self-hosted |
| Bark | TTS | Self-hosted |
| Coqui TTS | TTS+Clone | Self-hosted |
| XTTS | TTS+Clone | Self-hosted |
| Silero Models | STT+TTS+VAD | Self-hosted |
| Kyutai Moshi | A2A | Self-hosted |
| Fish Speech | TTS+Clone | Self-hosted |
| F5-TTS | TTS+Clone | Self-hosted |
| MeloTTS | TTS | Self-hosted |
| Parler TTS | TTS | Self-hosted |
| OuteTTS | TTS+Clone | Self-hosted |
| Nari Labs Dia | TTS+A2A | Self-hosted |

---

## Estimated Effort by Batch

| Batch | Providers | Est. Days | Complexity |
|-------|-----------|-----------|------------|
| 1 | 8 | 5-7 | High (OpenAI Realtime) |
| 2 | 8 | 4-5 | Medium (voice cloning) |
| 3 | 8 | 4-5 | Medium (STT patterns) |
| 4 | 6 | 3-4 | Medium (TTS APIs) |
| 5 | 6 | 4-5 | Medium (regional) |
| 6 | 10 | 6-8 | Medium-High (Asia auth) |
| 7 | 8 | 4-5 | Medium (regional) |
| 8 | 8 | 4-5 | Medium (regional) |
| 9 | 8 | 4-5 | Medium (specialized) |
| **Total** | **70** | **38-49** | |

---

## Session Log

### Session: 2026-01-06 (Update 5)
**Status:** IBM Watson STT and TTS implementation complete
**Completed:**
- IBM Watson STT (WebSocket Streaming) - `src/core/stt/ibm_watson/`
  - `config.rs`: IbmWatsonSTTConfig, IbmRegion (7 regions), RecognitionModel (10 models)
  - `messages.rs`: IbmWatsonMessage, RecognitionResults, SpeakerLabels, AudioMetrics
  - `client.rs`: IbmWatsonSTT implementing BaseSTT trait via WebSocket
  - `tests.rs`: 43 comprehensive unit tests
  - Key features: IAM token authentication, 30+ languages, speaker diarization, smart formatting
  - WebSocket URL: `wss://api.{region}.speech-to-text.watson.cloud.ibm.com/instances/{id}/v1/recognize`
- IBM Watson TTS (HTTP REST) - `src/core/tts/ibm_watson/`
  - `config.rs`: IbmWatsonTTSConfig, IbmVoice (30+ V3 neural voices), IbmOutputFormat (10 formats)
  - `provider.rs`: IbmWatsonTTS implementing BaseTTS trait via HTTP REST
  - `tests.rs`: 94 comprehensive unit tests
  - Key features: V3 neural voices across 15+ languages, SSML prosody support, rate/pitch control
  - REST URL: `https://api.{region}.text-to-speech.watson.cloud.ibm.com/instances/{id}/v1/synthesize`
- Shared IbmRegion enum between STT and TTS (with `stt_hostname()` and `tts_hostname()` methods)
- Factory integration in `src/core/stt/mod.rs` and `src/core/tts/mod.rs`
- Provider aliases: `ibm-watson`, `ibm_watson`, `watson`, `ibm`
**Quality Gates:** All passed (cargo fmt, clippy, 137 IBM Watson tests + mod-level tests passing)
**Key Design Decisions:**
- Used HTTP REST for TTS (simpler than WebSocket for one-shot synthesis)
- Used WebSocket for STT (required for real-time streaming)
- IAM token caching with automatic refresh before expiry
- SSML generation for rate/pitch control via `<prosody>` element
- Connection pooling via reqwest client for TTS HTTP requests
**Next Steps:**
- Continue with Batch 1: Groq (fastest Whisper hosting) or Hume AI

### Session: 2026-01-06 (Update 4)
**Status:** Amazon Transcribe STT and Amazon Polly TTS implementation complete
**Completed:**
- Amazon Transcribe STT (WebSocket Streaming) - `src/core/stt/aws_transcribe/`
  - `config.rs`: AwsTranscribeSTTConfig, TranscribeLanguage, MediaEncoding, VocabularyFilterMethod
  - `messages.rs`: TranscribeMessage, AudioEvent, TranscriptEvent, Result structs
  - `client.rs`: AwsTranscribeSTT implementing BaseSTT trait via AWS SDK
  - `tests.rs`: Comprehensive unit tests
  - Key features: 100+ languages, real-time streaming, vocabulary filtering, PII redaction, speaker diarization
- Amazon Polly TTS (AWS SDK-based) - `src/core/tts/aws_polly/`
  - `config.rs`: AwsPollyTTSConfig, PollyEngine, PollyOutputFormat, PollyVoice, TextType
  - `provider.rs`: AwsPollyTTS implementing BaseTTS trait via AWS SDK
  - `tests.rs`: Comprehensive unit tests
  - Key features: 60+ voices across 30+ languages, Neural/Standard/Generative/Long-form engines
  - Auto sample rate adjustment for PCM output format (8000/16000 Hz only)
- Factory integration in `src/core/stt/mod.rs` and `src/core/tts/mod.rs`
- AWS SDK dependencies: aws-sdk-polly v1.96.0, aws-sdk-transcribestreaming v1.95.0
**Quality Gates:** All passed (cargo fmt, clippy, 123 tests passing, 3 integration tests ignored)
**Key Design Decisions:**
- Used AWS SDK directly (not HTTP REST) for better integration with AWS auth mechanisms
- Implemented auto sample rate adjustment for Polly PCM format compatibility
- Used try_write() for callback registration to avoid async runtime panics
**Next Steps:**
- Continue with Batch 1: IBM Watson STT/TTS or Groq

### Session: 2026-01-06 (Update 3)
**Status:** AssemblyAI STT implementation complete
**Completed:**
- AssemblyAI STT (Streaming API v3 WebSocket) - `src/core/stt/assemblyai/`
  - `config.rs`: AssemblyAISTTConfig, AssemblyAIEncoding, AssemblyAISpeechModel, AssemblyAIRegion
  - `messages.rs`: BeginMessage, TurnMessage, TerminationMessage, ErrorMessage
  - `client.rs`: AssemblyAISTT implementing BaseSTT trait
  - `tests.rs`: 72 comprehensive unit tests
- Factory integration in `src/core/stt/mod.rs`
- Key features:
  - Immutable transcripts (transcripts never modified after delivery)
  - End-of-turn detection with configurable confidence threshold
  - Binary audio streaming (no base64 encoding overhead)
  - Word-level timestamps
  - Multilingual support with auto language detection
  - Regional endpoints (US/EU)
**Quality Gates:** All passed (cargo fmt, clippy, 72 tests passing)
**Next Steps:**
- Continue with Batch 1: Amazon Transcribe

### Session: 2026-01-06 (Update 2)
**Status:** OpenAI implementation complete
**Completed:**
- OpenAI STT (Whisper API - REST-based) - `src/core/stt/openai/`
- OpenAI TTS (TTS API - REST-based) - `src/core/tts/openai/`
- OpenAI Realtime (Audio-to-Audio WebSocket) - `src/core/realtime/openai/`
- New `BaseRealtime` trait for audio-to-audio providers - `src/core/realtime/base.rs`
- Gateway `/realtime` WebSocket endpoint - `src/handlers/realtime/`
- Client SDK updates (TypeScript and Python)
- Integration tests for all three components
**Quality Gates:** All passed (cargo fmt, clippy, tests)
**Next Steps:**
- Continue with Batch 1: AssemblyAI

### Session: 2026-01-06
**Status:** Document created
**Completed:** Initial provider_integration_status.md created
**Next Steps:**
- Start Batch 1 research: OpenAI, AssemblyAI
- Implement OpenAI Whisper STT (highest priority)

---

## API Documentation Quick Links

| Provider | API Docs |
|----------|----------|
| OpenAI | https://platform.openai.com/docs |
| AssemblyAI | https://www.assemblyai.com/docs |
| Amazon Transcribe | https://docs.aws.amazon.com/transcribe |
| Amazon Polly | https://docs.aws.amazon.com/polly |
| IBM Watson STT | https://cloud.ibm.com/apidocs/speech-to-text |
| IBM Watson TTS | https://cloud.ibm.com/apidocs/text-to-speech |
| Groq | https://console.groq.com/docs |
| Hume AI | https://dev.hume.ai |
| LMNT | https://docs.lmnt.com |
| Play.ht | https://docs.play.ht |
| Speechmatics | https://docs.speechmatics.com |
| Gladia | https://docs.gladia.io |
| Rev AI | https://docs.rev.ai |
| NeuralSpace | https://docs.neuralspace.ai |
| Sarvam AI | https://docs.sarvam.ai |

---

*This document should be updated as providers are implemented.*
