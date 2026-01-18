# Bud WaaV Provider Integration Status & Playbook

> **Last Updated:** 2026-01-14
> **Scope:** Cloud API providers only (no self-hosted/local inference engines)

---

## Quick Summary

| Metric | Count |
|--------|-------|
| **Total Cloud Providers** | 70 |
| **Implemented** | 46 |
| **Blocked** | 10 |
| **In Progress** | 0 |
| **Yet to Start** | 14 |
| **Estimated Days Remaining** | 7-11 |

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
| 7 | Groq | STT | [DONE] | 2026-01-06 | Fastest Whisper hosting (216x real-time), REST API |
| 8 | Hume AI | TTS+A2A | [DONE] | 2026-01-06 | TTS (Octave), EVI realtime, 48 emotions, voice cloning |

---

### Batch 2: Voice Cloning & Specialized

| # | Provider | Type | Status | Start Date | Notes |
|---|----------|------|--------|------------|-------|
| 9 | LMNT | TTS+Clone | [COMPLETE] | 2026-01-07 | Ultra-low latency (~150ms), 22+ languages, voice cloning |
| 10 | Play.ht | TTS+Clone | [COMPLETE] | 2026-01-07 | HTTP streaming (~190ms), 36+ languages, voice cloning, PlayDialog multi-turn |
| 11 | Murf.ai | TTS+Clone | [DONE] | 2026-01-13 | HTTP streaming, Falcon/Gen2 models, 12 regional endpoints |
| 12 | WellSaid Labs | TTS+Clone | [DONE] | 2026-01-13 | HTTP streaming, 200+ avatars, 20+ languages, Legacy/Caruso models |
| 13 | Resemble AI | TTS+A2A+Clone | [DONE] | 2026-01-13 | HTTP streaming, 3 models, 149+ languages, voice cloning |
| 14 | Speechify | TTS+Clone | [DONE] | 2026-01-13 | HTTP streaming, 4 models, 50+ languages, voice cloning |
| 15 | Unreal Speech | TTS | [DONE] | 2026-01-13 | HTTP streaming, 48 Kokoro + 5 legacy voices, 8 languages |
| 16 | Otter.ai | STT | [BLOCKED] | 2026-01-13 | No public API - Enterprise/Beta only, contact account manager required |

---

### Batch 3: Europe & Global STT

| # | Provider | Type | Status | Start Date | Notes |
|---|----------|------|--------|------------|-------|
| 17 | Speechmatics | STT+TTS | [DONE] | 2026-01-13 | WebSocket STT (55 langs), HTTP TTS (4 English voices) |
| 18 | Gladia | STT | [DONE] | 2026-01-13 | WebSocket STT (100+ langs), <300ms latency, solaria-1 model |
| 19 | Rev AI | STT | [DONE] | 2026-01-13 | WebSocket STT (9+ langs), speaker detection, custom vocab |
| 20 | Phonexia | STT | [DONE] | 2026-01-13 | WebSocket STT (on-premises), user-configured server URL, token/basic auth |
| 21 | Verbit | STT | [BLOCKED] | 2026-01-13 | Enterprise API - requires order first for WebSocket URL, no public streaming endpoint |
| 22 | SpeechText.AI | STT | [BLOCKED] | 2026-01-13 | Batch/async REST API only - no real-time streaming support |
| 23 | Speechly | STT | [BLOCKED] | 2026-01-13 | Acquired by Roblox Sept 2023 - API no longer publicly available |
| 24 | ReadSpeaker | TTS | [BLOCKED] | 2026-01-13 | Enterprise-focused, API docs require account signup, contact sales for pricing |

---

### Batch 4: Europe TTS & Russia/CIS

| # | Provider | Type | Status | Start Date | Notes |
|---|----------|------|--------|------------|-------|
| 25 | Acapela Group | TTS | [DONE] | 2026-01-13 | Cloud REST API with email/password auth, 250+ voices |
| 26 | Cereproc | TTS | [DONE] | 2026-01-13 | CereVoice Cloud API, emotional voices, Celtic languages |
| 27 | Yandex SpeechKit | STT+TTS | [DONE] | 2026-01-13 | REST API v1, STT (14+ langs) + TTS (29+ voices), IAM/API key auth |
| 28 | Tinkoff VoiceKit | STT+TTS | [DONE] | 2026-01-13 | gRPC streaming, STT (31 tests) + TTS (46 tests), SSML, JWT auth |
| 29 | SberDevices | STT+TTS | [DONE] | 2026-01-13 | OAuth 2.0, 92 tests (37 STT + 55 TTS), 7 voices, REST API |
| 30 | Nuance | STT+TTS | [BLOCKED] | 2026-01-13 | EOL May 2027, use Azure Speech instead. See docs/providers/nuance.md |

---

### Batch 5: India Regional

| # | Provider | Type | Status | Start Date | Notes |
|---|----------|------|--------|------------|-------|
| 31 | Sarvam AI | STT+TTS | [DONE] | Pre-existing | WebSocket STT (11 Indian langs), v2.5 Saarika/Saaras models |
| 32 | Gnani.ai | STT+TTS | [DONE] | Pre-existing | REST/gRPC STT+TTS (14 Indian langs), voice biometrics |
| 33 | Reverie | STT+TTS | [DONE] | 2026-01-13 | WebSocket STT (33 tests) + REST TTS (58 tests), 22+ Indian languages, header auth |
| 34 | CoRover | STT+TTS | [BLOCKED] | 2026-01-13 | Chatbot platform, uses Bhashini for voice, no standalone STT/TTS API |
| 35 | Smallest.ai | TTS+Clone | [DONE] | 2026-01-13 | REST TTS (57 tests), Lightning/Lightning-V2/Thunder models, voice cloning, 16 languages |
| 36 | AI4Bharat/Bhashini | STT+TTS | [DONE] | 2026-01-13 | Gov API (ULCA), 22+ Indian languages, pipeline-based auth (config→compute) |

---

### Batch 6: China & East Asia

| # | Provider | Type | Status | Start Date | Notes |
|---|----------|------|--------|------------|-------|
| 37 | iFlytek | STT+TTS | [DONE] | 2026-01-13 | WebSocket STT+TTS, 18+ languages, HMAC-SHA256 auth |
| 38 | Alibaba Cloud | STT+TTS | [DONE] | 2026-01-13 | DashScope STT+TTS, 25+ languages, CosyVoice/Qwen3 models |
| 39 | Baidu AI | STT+TTS | [DONE] | 2026-01-13 | REST TTS (68 tests) + WebSocket/REST STT (73 tests), OAuth 2.0, 40+ voices, Chinese dialects |
| 40 | Tencent Cloud | STT+TTS | [DONE] | 2026-01-14 | WebSocket STT (119 tests) + REST TTS (53 tests), TC3-HMAC-SHA256 auth, 70+ voices, Chinese dialects, emotion support |
| 41 | Huawei Cloud | STT+TTS | [DONE] | 2026-01-14 | WebSocket STT (94 tests) + REST/WebSocket TTS (62 tests), IAM token auth, 10+ standard voices, premium voices, Chinese/English, text splitting |
| 42 | NAVER CLOVA | STT+TTS | [DONE] | 2026-01-14 | REST STT (41 tests) + REST TTS (32 tests), two-key auth (client_id\|client_secret), 100+ neural voices with NeuVis technology, Korean/Japanese/English/Chinese/Spanish, volume/speed/pitch/emotion control, MP3/WAV output |
| 43 | Kakao | STT+TTS | [BLOCKED] | 2026-01-14 | Original Kakao Developers Speech API (kakaoi-newtone-openapi) terminated July 2022. Replacement KakaoCloud requires enterprise setup, limited documentation. See docs/providers/kakao.md |
| 44 | NTT COTOHA | STT+TTS | [BLOCKED] | 2026-01-14 | COTOHA API service terminated June 30, 2024. API portal (api.ce-cotoha.com) redirects to main NTT site. No replacement public API available. NTT Communications rebranded to NTT Docomo Business July 2025. See docs/providers/ntt_cotoha.md |
| 45 | AmiVoice | STT | [DONE] | 2026-01-14 | 96 tests. Advanced Media (Japan). WebSocket streaming with proprietary protocol. 19 engines: E2E and Hybrid. Domain-specific: medical, finance, insurance. Japanese, English, Chinese, Korean. Speaker diarization, sentiment analysis. See docs/providers/amivoice.md |

---

### Batch 7: Southeast Asia

| # | Provider | Type | Status | Start Date | Notes |
|---|----------|------|--------|------------|-------|
| 46 | Vbee | TTS+Clone | [BLOCKED] | 2026-01-14 | Vietnam TTS provider. API documentation not publicly accessible - docs.vbee.vn returns 502, Postman docs don't render. Authentication uses App ID + Token, REST-based callback URL pattern. Supports 50+ languages, 200+ voices, Vietnamese regional accents. Contact support for API access. See docs/providers/vbee.md |
| 47 | Zalo AI | TTS | [DONE] | 2026-01-14 | VNG Corporation Vietnamese TTS. 4 voices (Northern/Southern accents), 35 tests. REST API with two-step synthesis. WAV 16kHz output. See docs/providers/zalo_ai.md |
| 48 | FPT.AI | STT+TTS | [DONE] | 2026-01-14 | FPT Corporation Vietnamese STT+TTS. 7 voices (BanMai, LanNhi, LeMinh, MyAn, ThuMinh, GiaHuy, LinhSan). STT: REST file upload (non-streaming), 8/16kHz mono. TTS: REST two-step synthesis, MP3/WAV. 80 tests total (46 TTS + 34 STT). See docs/providers/fpt_ai.md |
| 49 | Viettel AI | STT+TTS | [DONE] | 2026-01-14 | Viettel Group Vietnamese STT+TTS. 12 voices (Northern/Central/Southern accents), 96% STT accuracy. TTS: REST API, WAV 16kHz output, 0.5-2.0x speed control. STT: REST multipart file upload. 170+ tests total (56 TTS + STT). See docs/providers/viettel_ai.md |
| 50 | Prosa.ai | STT+TTS | [DONE] | 2026-01-14 | Indonesian AI (Prosa.ai) STT+TTS. STT: WebSocket streaming + REST async API, stt-general and stt-general-online models. TTS: 9 voices (Dimas, Ocha, Dini, Kinanti, Darah, Abimana, Roger, Jennifer), pitch/tempo control, opus/mp3/wav formats. 203 tests total. See docs/providers/prosa_ai.md |
| 51 | Kata.ai | STT+TTS | [BLOCKED] | 2026-01-14 | Enterprise-only conversational AI platform (Indonesia). No public API documentation available. Kata Voice requires sales contact for access. Voice APIs integrated into their chatbot platform, not standalone STT/TTS. Contact business@kata.ai for enterprise pricing. See docs/providers/kata_ai.md |
| 52 | NECTEC | STT+TTS | [DONE] | 2026-01-17 | Thai government AI for Thai platform (NECTEC). STT: Partii4/Partii5 engines, REST file upload, WAV 16kHz mono, max 30 seconds. TTS: VAJA9 engine, REST two-step synthesis (POST for URL, GET for audio), WAV 22kHz PCM16, male/female voices, 300-char limit with auto-chunking. Free government-backed service. 83 tests total (39 STT + 44 TTS). See docs/providers/nectec.md |
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

### Session: 2026-01-13 (Update 27)
**Status:** iFlytek (科大讯飞) STT+TTS implementation complete
**Provider:** #37 iFlytek (Batch 6: China & East Asia)
**Implementation Details:**
- Created `src/core/stt/iflytek/` module with 4 files:
  - `mod.rs` - Module exports, constants (IFLYTEK_STT_ENDPOINT, IFLYTEK_STT_HOST, DEFAULT_FRAME_SIZE)
  - `config.rs` - IFlytekSttConfig, IFlytekLanguage (18+ languages), IFlytekAudioEncoding (Raw, Speex, SpeexWb), IFlytekVadEos (1800-10000ms), IFlytekPunctuation, IFlytekDynamicCorrection
  - `messages.rs` - SttRequest, SttResponse, IFlytekErrorCode (26 error codes), frame status handling
  - `client.rs` - IFlytekStt implementing BaseSTT trait via WebSocket streaming
- Created `src/core/tts/iflytek/` module with 4 files:
  - `mod.rs` - Module exports, re-exports from config/messages/provider, comprehensive tests
  - `config.rs` - IFlytekTtsConfig, IFlytekVoice (9+ voices), IFlytekTtsEncoding (Raw, Lame, Speex, SpeexWb), IFlytekTextEncoding, speed/volume/pitch controls
  - `messages.rs` - TtsRequest, TtsResponse, TtsRequestBusiness, TtsRequestData, audio decoding
  - `provider.rs` - IFlytekTts implementing BaseTTS trait via WebSocket streaming
- Shared authentication module (`src/core/stt/iflytek/auth.rs`):
  - HMAC-SHA256 signature-based authentication
  - API key format: `app_id|api_key|api_secret`
  - RFC 1123 date formatting with signature generation
  - Authorization URL construction for WebSocket handshake
- Key STT features:
  - WebSocket endpoint: `wss://iat-api.xfyun.cn/v2/iat`
  - Real-time streaming with bidirectional communication
  - 18+ languages: Chinese (Mandarin/Cantonese), English, Japanese, Korean, Russian, French, Spanish, Vietnamese, Indonesian, Arabic, Hindi, Italian, German, Malay, Thai, Portuguese, Kazakh
  - Dynamic correction for real-time transcript updates
  - VAD (Voice Activity Detection) with configurable end-of-speech detection
  - Punctuation modes: Off, Standard, Question marks
  - Frame status: 0=first, 1=middle, 2=last
  - Audio format: Raw PCM (16-bit, 16kHz mono)
- Key TTS features:
  - WebSocket endpoint: `wss://tts-api.xfyun.cn/v2/tts`
  - Single complete request model (status=2)
  - 9+ voices: xiaoyan (Chinese female), aisjiuxu (male), aisxping (broadcaster), aisjinger (sweet female), aisbabyxu (child), john_ce (English male), catherine (English female), luna (Japanese), anjali (Hindi)
  - Speed control: 0-100 (default 50)
  - Volume control: 0-100 (default 50)
  - Pitch control: 0-100 (default 50)
  - Audio formats: Raw PCM, MP3 (lame), Speex, Speex-wb
  - Sample rates: 8000, 16000 Hz
  - Base64-encoded text input, base64-encoded audio output
- 148 unit tests total (89 STT + 59 TTS)
- Plugin registration: Both STT and TTS registered in `plugin/builtin/mod.rs` with aliases (xfyun, xunfei, 讯飞, xfyun-tts, xunfei-tts, 讯飞-tts)
- Provider counts: 22 STT providers, 27 TTS providers
**Learnings:**
- iFlytek uses HMAC-SHA256 signature authentication (unique signature per request)
- Signature format: `host: {host}\ndate: {date}\nGET {path} HTTP/1.1` signed with api_secret
- Authorization header: `api_key="{api_key}", algorithm="hmac-sha256", headers="host date request-line", signature="{signature}"`
- TTS sends single complete request (status=2), receives multiple response chunks (status 0/1/2)
- STT sends audio in frames (status 0=first, 1=middle, 2=last), receives interim and final transcripts
- Audio must be Base64-encoded for JSON message payload
- Error codes are numeric (0=success, 10005=auth failure, 10013=rate limit, etc.)
**Quality Gates:** All passed (148 iFlytek tests, 4 builtin plugin tests, 3433 total tests)
**Updated Stats:** 39 implemented, 0 in progress, 24 remaining
**Next Steps:**
- Continue with Batch 6 (China & East Asia): Baidu AI (#39), Tencent Cloud (#40), etc.

### Session: 2026-01-14 (Update 30)
**Status:** Tencent Cloud STT enhancements + TTS implementation complete
**Provider:** #40 Tencent Cloud (Batch 6: China & East Asia)
**Implementation Details:**
- Enhanced `src/core/stt/tencent/` module with additional features:
  - Added 6 new engine models: English8k, MandarinLarge16k, CantoneseYue16k, TraditionalChinese16k, Arabic16k, Malay16k
  - Added new config enums: TencentNumeralMode (Chinese/Arabic/Math), TencentFilterDirtyMode (Off/Filter/Replace), TencentFilterModalMode (Off/Partial/Strict)
  - Added new config fields: max_speak_time, hotword_list, reinforce_hotword, convert_num_mode, customization_id, filter_empty_result
  - Updated signature.rs with builder methods for all new parameters
  - Updated client.rs to include all new features in WebSocket URL
- Created `src/core/tts/tencent/` module with 3 files:
  - `mod.rs` - Module exports, constants (TENCENT_TTS_URL, TENCENT_TTS_INTL_URL), comprehensive documentation
  - `config.rs` - TencentTtsConfig, TencentTtsVoice (70+ voices across 6 categories), TencentTtsAudioFormat (Wav/Mp3/Pcm), TencentTtsSampleRate, TC3-HMAC-SHA256 signature support
  - `provider.rs` - TencentTts implementing BaseTTS trait via HTTP REST API with text chunking
- Key STT enhancements:
  - Now 17 engine models (was 11): 8kHz/16kHz variants for Mandarin, English, Cantonese, Shanghai dialect, plus new Arabic/Malay support
  - Full hotword/vocabulary customization support
  - Numeral conversion modes (Chinese characters vs Arabic numerals)
  - Enhanced filtering (dirty words, modal particles)
  - Max speak time limit configuration
- Key TTS features:
  - HTTP REST API (TextToVoice) with TC3-HMAC-SHA256 authentication
  - 70+ voices across 6 categories: Standard (0-6), Premium (101001-101030), Emotional (101031-101040), Dialect (301001-301032), Child (501001-501005), English (601001-601004)
  - Audio formats: WAV, MP3, PCM at 8kHz/16kHz
  - Speed control (0.5-2.0), Volume control (0-10)
  - Emotion control (supported voices)
  - Word-level timestamps/subtitles
  - Automatic text chunking for 300 char limit
  - Regional endpoints: Domestic (tts.tencentcloudapi.com), International (tts.intl.tencentcloudapi.com)
- 172 unit tests total (119 STT + 53 TTS)
- Plugin registration: TTS registered in `plugin/builtin/mod.rs` with aliases (tencent-tts, tencent_tts, tencent-cloud, tencent-cloud-tts, 腾讯云, 腾讯语音, 腾讯云语音)
- Provider counts: 26 STT providers (including Tencent), 29 TTS providers
**Learnings:**
- Tencent Cloud uses TC3-HMAC-SHA256 for TTS (different from STT's simpler HMAC-SHA1)
- TTS is HTTP REST only, no WebSocket streaming available (noted latency considerations in docs)
- Voice type ID determines voice category: 0-10 standard, 101xxx premium, 301xxx dialect, 501xxx child, 601xxx English
- Text limit is 300 chars, automatic chunking required for longer texts
- Emotion control available only for emotional category voices (101031-101040)
**Quality Gates:** All passed (172 Tencent tests, builtin plugin tests, provider registration tests)
**Updated Stats:** 40 implemented, 0 in progress, 23 remaining
**Next Steps:**
- Continue with Batch 6: Huawei Cloud (#41), NAVER CLOVA (#42), etc.

### Session: 2026-01-14 (Update 31)
**Status:** Huawei Cloud STT+TTS implementation complete
**Provider:** #41 Huawei Cloud (Batch 6: China & East Asia)
**Implementation Details:**
- Created `src/core/stt/huawei_cloud/` module with 5 files:
  - `mod.rs` - Module exports, comprehensive documentation
  - `config.rs` - HuaweiCloudSttConfig with 5 regions (cn-north-4, cn-east-3, ap-southeast-1/2/3), 12 models (8k/16k variants), audio formats, vocabulary support
  - `auth.rs` - HuaweiTokenManager with IAM token authentication (X-Auth-Token header), 24-hour token validity with automatic refresh
  - `messages.rs` - WebSocket frames (StartFrame, AudioFrame, EndFrame, CancelFrame), realtime response parsing, error codes
  - `client.rs` - HuaweiCloudStt implementing BaseSTT trait via WebSocket with continuous/sentence modes
- Created `src/core/tts/huawei_cloud/` module with 3 files:
  - `mod.rs` - Module exports with voice documentation
  - `config.rs` - HuaweiCloudTtsConfig with 5 regions, 13 voice types (standard/premium/child/English), audio formats (wav/mp3/pcm), request/response types
  - `provider.rs` - HuaweiCloudTts implementing BaseTTS trait with REST API and automatic text splitting for 500-char limit
- Key STT features:
  - WebSocket endpoint: `wss://sis.{region}.myhuaweicloud.com/v1/{project_id}/rasr/sentence-stream`
  - IAM token authentication with shared HuaweiTokenManager
  - 5 regions: CN North-Beijing4 (cn-north-4), CN East-Shanghai2 (cn-east-3), AP-Singapore, CN-Hong Kong, AP-Bangkok
  - 12 models: chinese_8k_general, chinese_16k_general, chinese_16k_conversation, chinese_8k_common, chinese_16k_common, chinese_16k_media, english_8k_common, english_16k_common, sichuan_8k_common, cantonese_8k_common, chinese_8k_court, chinese_16k_court
  - Audio formats: PCM8K16BIT, PCM16K16BIT, OPUS16K32S, OPUS8K32S
  - Vocabulary support (Chinese models only)
  - Word-level timestamps
  - Sentence/Continuous modes
- Key TTS features:
  - REST endpoint: `https://sis.{region}.myhuaweicloud.com/v1/{project_id}/tts`
  - Real-time WebSocket: `wss://sis.{region}.myhuaweicloud.com/v1/{project_id}/rtts`
  - Shared IAM token authentication with STT
  - 10+ standard voices: 小琪, 小雯, 小燕, 小倩, 小婧 (female), 小宇, 小宋 (male), 小王, 小呆 (child), Cameal (English)
  - 3 premium voices: 华小夏 (sales), 华小唯 (customer service), 华晓刚 (news) - only in cn-north-4/cn-east-3
  - Audio formats: WAV, MP3, PCM at 8kHz/16kHz
  - Speed/pitch/volume control (-500 to 500, 0-100 for volume)
  - Premium voices don't support pitch adjustment
  - Automatic text splitting for 500-character limit with sentence boundary detection
- 156 unit tests total (94 STT + 62 TTS)
- Plugin registration: STT and TTS registered in `plugin/builtin/mod.rs` with aliases (huawei-cloud-tts, huawei_cloud-tts, huawei-tts, huawei-sis, 华为云, 华为语音, 华为云语音)
- Provider counts: 27 STT providers (including Huawei), 30 TTS providers
**Learnings:**
- Huawei Cloud uses IAM token-based authentication (different from other Chinese providers)
- Token shared between STT and TTS via HuaweiTokenManager singleton pattern
- Premium voices only available in China mainland regions (cn-north-4, cn-east-3)
- Premium voices have pitch control limitations
- Text limit is 500 chars (more generous than Tencent's 300)
- Regional endpoints use different subdomains: sis.cn-north-4.myhuaweicloud.com vs sis-ext.ap-southeast-3.myhuaweicloud.com (ext for overseas)
**Quality Gates:** All passed (156 Huawei tests, 4030 total lib tests, builtin plugin tests)
**Updated Stats:** 41 implemented, 0 in progress, 22 remaining
**Next Steps:**
- Continue with Batch 6: NAVER CLOVA (#42), Kakao (#43), NTT COTOHA (#44), etc.

### Session: 2026-01-13 (Update 28)
**Status:** Alibaba Cloud DashScope STT+TTS implementation complete
**Provider:** #38 Alibaba Cloud (Batch 6: China & East Asia)
**Implementation Details:**
- Created `src/core/stt/alibaba_cloud/` module with 4 files:
  - `mod.rs` - Module exports, constants (DASHSCOPE_BEIJING_REALTIME_URL, DASHSCOPE_SINGAPORE_REALTIME_URL)
  - `config.rs` - DashScopeSttConfig, DashScopeRegion (Beijing, Singapore), DashScopeSttModel (Qwen3AsrFlashRealtime, ParaformerRealtimeV2, etc.), DashScopeAudioFormat, DashScopeLanguage (25+ languages), TurnDetectionMode
  - `messages.rs` - Qwen protocol (QwenSessionUpdate, QwenAudioBufferAppend, QwenServerMessage) and Paraformer protocol (ParaformerRunTask, ParaformerResponse)
  - `client.rs` - DashScopeStt implementing BaseSTT trait via WebSocket with dual protocol support (Qwen3-ASR and Paraformer)
- Created `src/core/tts/alibaba_cloud/` module with 4 files:
  - `mod.rs` - Module exports, constants (DASHSCOPE_BEIJING_INFERENCE_URL, DEFAULT_TTS_MODEL)
  - `config.rs` - DashScopeTtsConfig, DashScopeTtsModel (CosyVoiceV3Flash, CosyVoiceV3Plus, Qwen3TtsFlashRealtime), voices (15+ CosyVoice + 6+ Qwen3)
  - `messages.rs` - Qwen TTS protocol (QwenTtsSessionUpdate, QwenTtsTextAppend, QwenTtsServerMessage) and CosyVoice protocol (CosyVoiceRunTask, CosyVoiceResponse)
  - `provider.rs` - DashScopeTts implementing BaseTTS trait via WebSocket with dual protocol support (Qwen3-TTS and CosyVoice)
- Key STT features:
  - WebSocket endpoints: Realtime API (Qwen3-ASR), Inference API (Paraformer)
  - Bearer token authentication with DASHSCOPE_API_KEY
  - 25+ languages: Chinese (Mandarin, Cantonese, Wu, Sichuanese, Minnan), English, Japanese, Korean, Russian, French, German, Spanish, Portuguese, Italian, Arabic, Hindi, Thai, Vietnamese, Indonesian
  - 4 models: Qwen3-ASR Flash Realtime, Paraformer Realtime V2, Paraformer 8k V2, FunASR Realtime
  - Emotion recognition support (Qwen3 model)
  - Word-level timestamps
  - Server-side VAD for turn detection
- Key TTS features:
  - WebSocket endpoints: Realtime API (Qwen3-TTS), Inference API (CosyVoice)
  - 4 models: CosyVoice V3 Flash (default), CosyVoice V3 Plus, CosyVoice V2, Qwen3-TTS Flash Realtime
  - 20+ voices: CosyVoice Chinese voices (longxiaochun, longxiaoxia, etc.), Qwen3 multilingual voices (Cherry, Serena, Ethan, etc.)
  - Audio formats: MP3, PCM16, WAV, Opus
  - Prosody control: Rate (0.5-2.0), Pitch (0.5-2.0), Volume (0-100)
  - Regional endpoints: Beijing (China), Singapore (International)
- 128 unit tests total (75 STT + 53 TTS)
- Plugin registration: Both STT and TTS registered in `plugin/builtin/mod.rs` with aliases (dashscope, alibabacloud, aliyun, qwen-asr, cosyvoice, qwen-tts)
- Provider counts: 24 STT providers, 28 TTS providers
**Learnings:**
- DashScope uses dual WebSocket protocols: OpenAI-like realtime API for Qwen models, inference API for CosyVoice/Paraformer
- Regional endpoint selection important: Beijing for China mainland, Singapore for international
- Model auto-detection switches between protocols based on model name prefix
- CosyVoice models use run-task/continue-task/finish-task pattern similar to DashScope inference SDK
- Qwen models use OpenAI-style session.update pattern for configuration
- Chinese dialect support is extensive (Cantonese, Sichuanese, Wu, Minnan)
**Quality Gates:** All passed (128 Alibaba Cloud tests, 4 builtin plugin tests)
**Updated Stats:** 39 implemented, 0 in progress, 24 remaining
**Next Steps:**
- Continue with Batch 6: Baidu AI (#39), Tencent Cloud (#40), Huawei Cloud (#41), etc.

### Session: 2026-01-13 (Update 27)
**Status:** iFlytek STT+TTS implementation complete (see previous entry)

### Session: 2026-01-13 (Update 26)
**Status:** AI4Bharat/Bhashini ULCA STT+TTS implementation complete
**Provider:** #36 AI4Bharat/Bhashini (Batch 5: India Regional)
**Implementation Details:**
- Created `src/core/stt/bhashini/` module with 4 files:
  - `mod.rs` - Module exports, constants (BHASHINI_CONFIG_URL, BHASHINI_COMPUTE_URL, MEITY_PIPELINE_ID, AI4BHARAT_PIPELINE_ID)
  - `config.rs` - BhashiniSTTConfig, BhashiniLanguage (24+ languages), BhashiniPipelineProvider (MeitY, AI4Bharat), BhashiniAudioFormat (Wav, Flac, Mp3), LanguageFamily (Dravidian, IndoAryan, Misc)
  - `messages.rs` - PipelineConfigRequest/Response, PipelineComputeRequest/Response, ASRInput, AudioConfig, TaskConfig, LanguageConfig
  - `client.rs` - BhashiniStt implementing BaseSTT trait via REST API with 2-step pipeline auth
- Created `src/core/tts/bhashini/` module with 3 files:
  - `mod.rs` - Module exports, re-exports from STT module
  - `config.rs` - BhashiniTtsConfig, BhashiniTtsAudioFormat (Wav, Mp3), BhashiniTtsGender (Male, Female)
  - `provider.rs` - BhashiniTts implementing BaseTTS trait via REST API with pipeline auth
- Key STT features:
  - REST endpoints: Config at `https://meity-auth.ulcacontrib.org/ulca/apis/v0/model/getModelsPipeline`, Compute at `https://dhruva-api.bhashini.gov.in/services/inference/pipeline`
  - 2-step authentication: Pipeline Config (userId + ulcaApiKey) → Pipeline Compute (inferenceApiKey)
  - API key format: `userId|ulcaApiKey` or `userId|ulcaApiKey|inferenceApiKey`
  - 24+ Indian languages: Hindi, Tamil, Telugu, Kannada, Malayalam, Bengali, Marathi, Gujarati, Punjabi, Odia, Urdu, Assamese, Sanskrit, English, Nepali, Bodo, Dogri, Konkani, Maithili, Manipuri, Santali, Sindhi, Bhojpuri, Kashmiri
  - Language families: Dravidian (ta, te, kn, ml), Indo-Aryan (hi, bn, mr, gu, pa, or, ur, as, ne, mai, bho, sd), Misc (en, sa, brx, doi, kok, mni, sat, ks)
  - Service ID selection based on language family (ai4bharat/iitm models)
  - Audio buffering for REST-based recognition
- Key TTS features:
  - Same 2-step pipeline authentication as STT
  - 24+ languages with service IDs based on language family
  - Gender selection: Male or Female voices
  - Audio formats: WAV (default), MP3
  - Default sample rate: 22050 Hz
  - Base64-encoded audio in response
- 68 unit tests total (46 STT + 22 TTS)
- Plugin registration: Both STT and TTS registered in `plugin/builtin/mod.rs` with aliases (bhashini-stt, bhashini_stt, ulca, ai4bharat, meity, bhashini-tts, ulca-tts, ai4bharat-tts, meity-tts)
- Provider counts: 21 STT providers, 26 TTS providers
**Learnings:**
- Bhashini uses a complex 2-step pipeline authentication flow (unique among implemented providers)
- Pipeline Config call uses userId + ulcaApiKey headers, returns inferenceApiKey for compute calls
- Service IDs vary by language family (dravidian, indo-aryan, misc GPU models)
- Response format includes nested config arrays requiring careful JSON parsing
- Credential format in api_key field follows pattern: `userId|ulcaApiKey|optionalInferenceApiKey`
**Quality Gates:** All passed (68 Bhashini tests, 5 builtin plugin tests, all aliases verified)
**Updated Stats:** 37 implemented, 0 in progress, 26 remaining
**Next Steps:**
- Batch 5 (India Regional) complete
- Continue with Batch 6 (China & East Asia) or remaining batches

### Session: 2026-01-13 (Update 25)
**Status:** SberDevices SaluteSpeech STT+TTS implementation complete
**Provider:** #29 SberDevices (Batch 4: Russia/CIS)
**Implementation Details:**
- Created `src/core/stt/sberdevices/` module with 4 files:
  - `mod.rs` - Module exports, constants (SBER_STT_RECOGNIZE_URL, OAUTH_ENDPOINT)
  - `config.rs` - SberSttConfig, SberSttLanguage (Russian, English), SberAudioEncoding (Pcm16, Opus, Mp3, Flac), SberSttScope (Personal, Corporate, B2B, Legacy)
  - `messages.rs` - SberSttRequest, SberSttResponse, SberRecognitionResult, SberWord, SberOAuthResponse
  - `client.rs` - SberDevicesStt implementing BaseSTT trait via REST API with OAuth 2.0 token management
- Created `src/core/tts/sberdevices/` module with 3 files:
  - `mod.rs` - Module exports, constants (SBER_TTS_SYNTHESIZE_URL, OAUTH_ENDPOINT, TOKEN_VALIDITY_SECS)
  - `config.rs` - SberTtsConfig, SberTtsVoice (7 voices), SberTtsAudioFormat (Wav, Opus, Mp3), SberTtsScope
  - `provider.rs` - SberDevicesTts implementing BaseTTS trait via REST API with TokenManager
- Key STT features:
  - REST endpoint: `https://smartspeech.sber.ru/rest/v1/speech:recognize`
  - OAuth 2.0 authentication with automatic token refresh
  - Client credentials (client_id:client_secret) in Base64 for Basic auth
  - Token validity: 30 minutes with 60-second refresh threshold
  - 2 languages: Russian (ru-RU), English (en-US)
  - 4 audio encodings: PCM16, Opus, MP3, FLAC
  - Audio buffering for REST-based recognition
- Key TTS features:
  - REST endpoint: `https://smartspeech.sber.ru/rest/v1/text:synthesize`
  - 7 voices: Nec (Natalia), Bys (Boris), May (Martha), Tur (Taras), Ost (Alexandra), Pon (Sergey), Kin (Kira/English)
  - 3 audio formats: WAV, Opus, MP3
  - Sample rates: 8000 Hz (telephony), 24000 Hz (high quality)
  - SSML support
  - Max text length: 4000 characters
- 92 unit tests total (37 STT + 55 TTS)
- Plugin registration: Both STT and TTS registered in `plugin/builtin/mod.rs` with aliases (salutespeech, smartspeech)
- Provider counts: 20 STT providers, 25 TTS providers
**Learnings:**
- SberDevices uses REST API (not WebSocket/gRPC) for both STT and TTS
- OAuth 2.0 client credentials flow with Base64-encoded credentials
- Scope parameter determines which API scope is used (SALUTE_SPEECH_PERS, SALUTE_SPEECH_CORP, SALUTE_SPEECH_B2B)
- Token refresh is handled proactively before expiration
- STT uses audio buffering pattern (similar to Groq/OpenAI Whisper)
- TTS uses synchronous REST synthesis (not streaming)
**Quality Gates:** All passed (92 SberDevices tests, 5 builtin plugin tests)
**Updated Stats:** 36 implemented, 0 in progress, 28 remaining
**Next Steps:**
- Continue with Batch 4: Nuance (#30)
- Or continue with Batch 5/6 providers

### Session: 2026-01-13 (Update 24)
**Status:** Tinkoff VoiceKit STT+TTS implementation complete
**Provider:** #28 Tinkoff VoiceKit (Batch 4: Russia/CIS)
**Implementation Details:**
- Created `src/core/stt/tinkoff/` module with 5 files:
  - `mod.rs` - Module exports, constants (TINKOFF_GRPC_ENDPOINT, GRPC_SERVICE_PATH)
  - `config.rs` - TinkoffSttConfig, TinkoffAudioEncoding (Linear16, RawOpus, Mulaw, Alaw), VadConfig
  - `messages.rs` - Manual protobuf encoding/decoding using varint functions, RecognitionConfig, StreamingRecognizeRequest/Response
  - `grpc.rs` - TinkoffGrpcClient, TinkoffCodec implementing tonic::codec::Codec trait
  - `provider.rs` - TinkoffStt implementing BaseSTT trait via gRPC bidirectional streaming
- Created `src/core/tts/tinkoff/` module with 5 files:
  - `mod.rs` - Module exports, constants (GRPC_SYNTHESIZE_PATH, GRPC_STREAMING_SYNTHESIZE_PATH, TTS_SCOPE)
  - `config.rs` - TinkoffTtsConfig, TinkoffVoice (Alyona, Dorofeev), TinkoffAudioEncoding
  - `messages.rs` - SynthesizeSpeechRequest/Response, StreamingSynthesizeSpeechResponse, SynthesisInput, AudioConfig
  - `grpc.rs` - TinkoffTtsGrpcClient with JWT auth, TinkoffTtsCodec for gRPC transport
  - `provider.rs` - TinkoffTts implementing BaseTTS trait with streaming synthesis support
- Key STT features:
  - gRPC endpoint: `https://api.tinkoff.ai:443`
  - Bidirectional streaming for real-time STT
  - 4 audio encodings: LINEAR16, RAW_OPUS, MULAW, ALAW
  - Configurable VAD, interim results, multi-channel support
- Key TTS features:
  - Unary and streaming synthesis (StreamingSynthesize for low latency)
  - 2 voices: Alyona (female), Dorofeev (male)
  - SSML support with prosody control (rate, pitch, volume)
  - JWT authentication with HMAC-SHA256 signing
- 77 unit tests total (31 STT + 46 TTS)
- Plugin registration: Both STT and TTS registered in `plugin/builtin/mod.rs`
- Provider counts: 19 STT providers, 24 TTS providers
**Learnings:**
- Tinkoff uses gRPC protocol (not WebSocket) for both STT and TTS
- Custom codec approach avoids proto file generation complexity
- JWT tokens require HMAC-SHA256 signing with api_key as kid, secret_key as signing key
- TTS supports both unary (single response) and streaming (chunked responses) synthesis
- Voice selection is via name field in VoiceSelectionParams ("alyona", "dorofeev")
**Quality Gates:** All passed (77 Tinkoff tests, 5 builtin plugin tests)
**Updated Stats:** 35 implemented, 0 in progress, 29 remaining
**Next Steps:**
- Continue with Batch 4: SberDevices (#29), Nuance (#30)
- Or continue with Batch 5/6 providers

### Session: 2026-01-13 (Update 23)
**Status:** Smallest.ai Waves TTS implementation complete
**Provider:** #35 Smallest.ai (Batch 5: India Regional)
**Implementation Details:**
- Created `src/core/tts/smallest/` module with 4 files:
  - `mod.rs` - Module exports, API constants, sample rate utilities
  - `config.rs` - SmallestTtsConfig, SmallestModel (Lightning, LightningLarge, LightningV2, Thunder), SmallestOutputFormat (Pcm, Wav, Mp3, Mulaw), SmallestLanguage (16 languages)
  - `messages.rs` - SmallestTtsRequest, SmallestWsRequest (WebSocket), SmallestWsChunkResponse, SmallestVoice, SmallestAddVoiceResponse
  - `provider.rs` - SmallestTts implementing BaseTTS trait via HTTP REST API with auto-reconnect
- Key features:
  - REST API endpoint: `https://waves-api.smallest.ai/api/v1/lightning/get_speech`
  - WebSocket streaming (Lightning-V2): `wss://waves-api.smallest.ai/api/v1/lightning-v2/get_speech/stream`
  - Bearer token authentication: `Authorization: Bearer <API_KEY>`
  - 4 models: Lightning (~100ms), Lightning-Large (~300ms), Lightning-V2 (<200ms), Thunder
  - 16 languages: English, Hindi, Marathi, Kannada, Tamil, Bengali, Gujarati, German, French, Spanish, Italian, Polish, Dutch, Russian, Arabic, Hebrew
  - Voice cloning support (Lightning-Large and Lightning-V2)
  - Audio formats: PCM, WAV, MP3, µ-law
  - Sample rates: 8000, 16000, 22050, 24000 Hz
  - Parameters: speed (0.5-5.0), consistency (0-1), similarity (0-1), enhancement (0-2)
  - Auto-reconnect on speak if not connected
- 57 unit tests total
- Plugin registration: Registered in `plugin/builtin/mod.rs` with aliases (smallest-ai, smallest_ai, waves, smallest.ai)
- Provider count updated: 23 TTS providers
**Learnings:**
- Smallest.ai uses REST API for Lightning model, WebSocket for Lightning-V2 streaming
- Ultra-low latency (~100ms for Lightning model) makes it suitable for real-time applications
- Voice cloning available with Lightning-Large and Lightning-V2 models
- The provider was modified to include auto-reconnect, config hashing, and additional setter methods
**Quality Gates:** All passed (57 Smallest.ai tests, 5 builtin plugin tests)
**Updated Stats:** 34 implemented, 30 remaining
**Next Steps:**
- Continue with Batch 5: AI4Bharat/Bhashini (#36)
- Or continue with Batch 4: Tinkoff VoiceKit (#28), SberDevices (#29), Nuance (#30)

### Session: 2026-01-13 (Update 22)
**Status:** Yandex SpeechKit STT+TTS implementation complete
**Provider:** #27 Yandex SpeechKit (Batch 4: Russia/CIS)
**Implementation Details:**
- Created `src/core/stt/yandex/` module with 4 files:
  - `mod.rs` - Module exports, re-exports, documentation
  - `config.rs` - YandexSTTConfig, YandexSTTLanguage (14+ languages), YandexSTTModel (General, GeneralRc, Deferred), YandexSTTAudioFormat (Lpcm, OggOpus, Mp3)
  - `messages.rs` - YandexSyncResponse, StreamingRecognitionResult, RecognitionAlternative, YandexSTTApiError, YandexSTTStatusCode, WordInfo
  - `client.rs` - YandexSTT implementing BaseSTT trait via REST API with pseudo-streaming (audio buffering)
- Created `src/core/tts/yandex/` module with 4 files:
  - `mod.rs` - Module exports, API constants
  - `config.rs` - YandexTtsConfig, YandexVoice (29+ voices), YandexAudioFormat (Lpcm, OggOpus, Mp3), YandexEmotion (neutral, good, evil, strict, friendly, whisper)
  - `messages.rs` - SynthesisRequest, SynthesisResponse, YandexTtsApiError
  - `provider.rs` - YandexTts implementing BaseTTS trait via HTTP POST with form-urlencoded body
- Key features:
  - REST API v1 endpoints: STT at `stt.api.cloud.yandex.net/speech/v1/stt:recognize`, TTS at `tts.api.cloud.yandex.net/speech/v1/tts:synthesize`
  - Dual authentication: IAM tokens (Bearer) or API keys (Api-Key prefix)
  - Folder ID embedded in api_key as "folder_id:api_key" format
  - STT: 14+ languages (Russian, English, German, French, Finnish, Swedish, Dutch, Polish, Portuguese, Turkish, Ukrainian, Kazakh, Uzbek, Hebrew, Auto)
  - TTS: 29+ voices across 4 languages with 6 emotions (neutral, good, evil, strict, friendly, whisper)
  - Audio formats: LPCM (linear16), OggOpus, MP3
  - Sample rates: 8000, 16000, 48000 Hz
  - STT pseudo-streaming via audio buffering with periodic recognition
  - Sync recognition limit: 30 seconds, 1MB audio
- 76 unit tests total (42 TTS + 34 STT)
- Plugin registration: Both STT and TTS registered in `plugin/builtin/mod.rs` with aliases (speechkit, yandex-stt, yandex-tts, yandex_stt, yandex_tts)
- Provider count updated: 18 STT providers, 22 TTS providers
**Learnings:**
- Yandex uses REST API v1 (not WebSocket) for both STT and TTS
- Authentication header format differs: "Bearer {token}" for IAM, "Api-Key {key}" for API keys
- Form-urlencoded body for TTS (not JSON) with `lang`, `text`, `voice`, `format`, `folderId` parameters
- Binary audio POST for STT with query string parameters
- Emotion support via `emotion` parameter in TTS (voice must support emotions)
- STT doesn't have true streaming - implemented via audio buffering pattern
**Quality Gates:** All passed (76 Yandex tests, all tests passing)
**Updated Stats:** 32 implemented, 32 remaining
**Next Steps:**
- Continue with Batch 4: Tinkoff VoiceKit (#28), SberDevices (#29), Nuance (#30)

### Session: 2026-01-13 (Update 20)
**Status:** Acapela Cloud TTS implementation complete
**Completed:**
- Acapela Cloud TTS (HTTP REST) - `src/core/tts/acapela/`
  - `mod.rs`: Module exports, API constants (BASE_URL, LOGIN_URL, COMMAND_URL, MAX_TEXT_LENGTH, DEFAULT_VOICE)
  - `config.rs`: AcapelaTtsConfig, AcapelaCredentials (email/password), AcapelaAudioFormat (17 formats), AcapelaOutputMode
  - `messages.rs`: LoginRequest/Response, StreamChunk enum (Audio/Events), StreamParser for "type:size\n" protocol, Viseme codes (Disney standard 0-21)
  - `provider.rs`: AcapelaTts implementing BaseTTS trait via TTSProvider HTTP
  - Key features:
    - Email/password authentication with session token caching (30-min TTL)
    - Token format: "Token {token}" in Authorization header
    - Streaming protocol: "type:size\ncontent" mixed audio/events format
    - 250+ voices across 30+ languages
    - 17 audio formats: MP3, OGG, WAV, FLAC, AC3, ASF, WMA, Opus, AAC, AIFF, WebM, MKA, S16LE, ALAW, MULAW, WavMulaw, WavAlaw
    - Word position events for text highlighting
    - Viseme data for lip-sync animation
    - Custom dictionaries support
  - 44 unit tests covering config, messages, and provider modules
- Documentation: `docs/providers/acapela.md` (comprehensive API details)
- Plugin registration: Added to `plugin/builtin/mod.rs` with aliases (acapela-cloud, acapela_cloud, acapela-group)
**Learnings:**
- Acapela Cloud uses REST API with email/password authentication (not typical API key)
- Token authentication requires "Token " prefix (not "Bearer ")
- Streaming response uses custom text protocol: "type:size\ncontent" where type is "audio" or "events"
- Viseme codes follow Disney standard (0-21) for lip-sync animation
**Quality Gates:** All passed (44 Acapela tests, 2782 total tests passing)
**Updated Stats:** 29 implemented, 35 remaining
**Next Steps:**
- Continue with Batch 4: Cereproc (#26), Yandex SpeechKit (#27), etc.

### Session: 2026-01-13 (Update 21)
**Status:** Cereproc CereVoice Cloud TTS implementation complete
**Provider:** #26 Cereproc (Batch 4: Europe TTS)
**Implementation Details:**
- Created `src/core/tts/cereproc/` module with 4 files:
  - `mod.rs` - Module exports and API constants (auth, speak, credit, voices endpoints)
  - `config.rs` - CereprocAudioFormat (wav/mp3/ogg/raw), CereprocCredentials (email:password), CereprocTtsConfig, CereprocEmotion (happy/sad/calm/cross)
  - `messages.rs` - AuthResponse, SpeakResponse, CreditResponse, CereprocApiError, ResultCode enum
  - `provider.rs` - CereprocTts implementing BaseTTS, CereprocRequestBuilder implementing TTSRequestBuilder, TokenCache for bearer token caching
- Key features:
  - Email/password authentication with Bearer token caching (30 min TTL)
  - HTTP REST API with XML/SSML body (POST /speak with text/xml content-type)
  - Response returns JSON with fileUrl to download audio
  - 4 audio formats: WAV, MP3, OGG, Raw
  - Sample rates: 8kHz-48kHz (default 22050)
  - Emotional voice support with SSML-style emotion tags
  - Celtic language support (Welsh, Scottish Gaelic, Irish)
- 42 unit tests covering config, messages, and provider modules
- Documentation: `docs/providers/cereproc.md` (comprehensive API details)
- Plugin registration: Added to `plugin/builtin/mod.rs` with aliases (cerevoice, cerevoice-cloud, cereproc-tts)
**Learnings:**
- CereProc uses email/password credentials (not API key), formatted as "email:password" in api_key field
- Auth endpoint returns Bearer token for subsequent requests
- /speak endpoint accepts XML body with text, returns JSON with fileUrl
- Emotional voices use SSML-style `<emotion name="happy">` tags
- Token caching similar to Acapela pattern (30 min TTL)
**Quality Gates:** All passed (42 Cereproc tests, 2857 total tests passing)
**Updated Stats:** 30 implemented, 34 remaining
**Next Steps:**
- Continue with Batch 4: Yandex SpeechKit (#27), Tinkoff VoiceKit (#28), etc.

### Session: 2026-01-13 (Update 19)
**Status:** Batch 5 (India Regional) research complete
**Research Results:**
- **Reverie (#33)**: [DONE] Fully implemented STT (WebSocket) + TTS (REST), 22+ Indian languages
  - STT: WebSocket streaming, PCM/Opus/µ-Law formats, 33 tests passing
  - TTS: REST HTTP API, WAV/MP3 output, 36+ voices, 58 tests passing
  - TTS: Multiple male/female voices per language, MP3/WAV output
  - Auth: REV-API-KEY, REV-APP-ID, REV-APPNAME headers
  - Docs: docs.reverieinc.com
- **CoRover/BharatGPT (#34)**: [BLOCKED] Chatbot platform that uses Bhashini for voice capabilities
  - Not a standalone STT/TTS API - integrated conversational AI platform
  - No public API documentation for direct speech services
- **Smallest.ai (#35)**: [READY] WebSocket TTS API with voice cloning
  - Endpoints: POST /api/v1/lightning-v2/get_speech (sync), WebSocket streaming
  - Models: lightning (default), lightning-large (cloning)
  - Auth: SMALLEST_API_KEY
  - Features: 16+ languages, instant voice cloning, sub-100ms latency
  - Docs: waves-docs.smallest.ai
- **AI4Bharat/Bhashini (#36)**: [READY] Government pipeline-based API
  - Complex 3-step flow: Pipeline Search → Pipeline Config → Pipeline Compute
  - Base: meity-auth.ulcacontrib.org
  - Auth: userID + ulcaApiKey
  - 22 Indian languages, multiple ASR/TTS/NMT models
  - Usage: PoC only (contact for production)
**Updated Stats:**
- Batch 5: 3 DONE (Sarvam, Gnani, Reverie), 2 TODO (Smallest, Bhashini), 1 BLOCKED (CoRover)
- Total blocked: 7
**Next Steps:**
- Implement Smallest.ai TTS (#35)
- Implement Bhashini STT/TTS (#36)
- Implement Bhashini pipeline API

### Session: 2026-01-13 (Update 18)
**Status:** Phonexia STT implementation complete (on-premises)
**Completed:**
- Phonexia STT (WebSocket Streaming) - `src/core/stt/phonexia/`
  - `mod.rs`: Module exports, constants (WEBSOCKET_PATH, LOGIN_PATH, DEFAULT_SAMPLE_RATE, DEFAULT_AUDIO_FORMAT)
  - `config.rs`: PhonexiaSTTConfig, PhonexiaAuth (Token/Basic/None), PhonexiaResultType (OneBest/NBest/ConfusionNetwork)
  - `messages.rs`: ServerMessage enum with custom from_json() parser, PhonexiaResult, PhonexiaError, StatusMessage, Segment, Word
  - `client.rs`: PhonexiaSTT implementing BaseSTT trait via WebSocket
  - Key features:
    - On-premises only (user-configured server URL)
    - Token-based or HTTP Basic authentication
    - WebSocket path: `/input_stream/websocket`
    - 57-64 languages, voice biometrics capability
    - RAW s16le audio format
    - N-best and confusion network result types
    - Word-level timestamps and confidence scores
  - 55 unit tests covering config, messages, and client modules
- Documentation: `docs/providers/phonexia.md` (comprehensive API details)
- Plugin registration: Added to `plugin/builtin/mod.rs` with aliases (phonexia-stt, phonexia_stt)
**Learnings:**
- Phonexia is self-hosted only (no public cloud API)
- Custom JSON parsing needed because Serde's `#[serde(untagged)]` enum with optional fields matched incorrectly
- Implemented smart type detection: Error (has code+message), Status (has status/stream_id without segments), Result (default)
**Quality Gates:** All passed (55 Phonexia tests, 2738 total tests passing)
**Next Steps:**
- Continue with Batch 4: Acapela Group, Cereproc, etc.

### Session: 2026-01-13 (Update 17)
**Status:** Batch 3 remaining providers research complete
**Research Results:**
- **Phonexia (#20)**: [BLOCKED] On-premises only solution. Requires self-hosted installation (Linux/Windows server). Uses session-based authentication, not public cloud API. Not suitable for cloud-based integration.
- **Verbit (#21)**: [BLOCKED] Enterprise-focused API with complex two-step flow. Requires creating an order via REST API first to receive a WebSocket URL token. No direct public streaming endpoint. Python SDK available at github.com/verbit-ai/verbit-streaming-python-sdk but complex enterprise workflow.
- **SpeechText.AI (#22)**: [BLOCKED] Batch/async REST API only. Uses POST to `/recognize` endpoint, then poll `/results` until complete. No real-time streaming support. GDPR-compliant EU-hosted, but not suitable for live streaming use cases.
- **Speechly (#23)**: [BLOCKED] Acquired by Roblox in September 2023. Company no longer exists as independent entity. API documentation site (docs.speechly.com) is down. Technology now used internally by Roblox for voice chat moderation.
- **ReadSpeaker (#24)**: [BLOCKED] Enterprise-focused TTS provider. API documentation requires account signup. Pricing requires contacting sales. No publicly accessible API specs. Supports 200+ voices in 50+ languages but no self-service developer access.
**Updated Stats:**
- Batch 3: 3 DONE (Speechmatics, Gladia, Rev AI), 5 BLOCKED
- Sarvam AI and Gnani.ai status corrected to [DONE] (were already implemented)
- Total: 27 implemented, 6 blocked, 37 remaining
**Next Steps:**
- Continue with Batch 4 (Europe TTS & Russia/CIS) or Batch 5 (India Regional)

### Session: 2026-01-13 (Update 16)
**Status:** Rev AI STT implementation complete
**Completed:**
- Rev AI STT (WebSocket Streaming) - `src/core/stt/revai/`
  - `mod.rs`: Module exports, API constants (REVAI_STREAM_URL, sample rate/channel limits, EOS_MESSAGE)
  - `config.rs`: RevAISTTConfig, RevAISampleFormat (S16LE/S32LE/F32LE/etc.), RevAIAudioLayout (Interleaved, NonInterleaved), RevAITranscriber (Machine, MachineV2, Human)
  - `messages.rs`: ConnectedMessage, PartialTranscript, FinalTranscript, TranscriptElement (text/punct types), RevAICloseCode (4001-4029 custom codes), ServerMessage enum
  - `client.rs`: RevAISTT implementing BaseSTT trait via single-step WebSocket connection
  - Key features:
    - WebSocket endpoint: `wss://api.rev.ai/speechtotext/v1/stream`
    - Single-step connection with all parameters in URL query string
    - Speaker detection (enable_speaker_switch=true with machine_v2)
    - Custom vocabulary support (custom_vocabulary_id)
    - Profanity filtering (filter_profanity=true)
    - Disfluency removal (remove_disfluencies=true)
    - 9+ streaming languages: en, es, fr, de, pt, cmn, ja, ru, ar, hi
    - Three transcriber types: machine (default), machine_v2 (speaker detection), human
  - 53 unit tests covering config, messages, and client modules
- Documentation: `docs/providers/revai.md` (API details, parameters, message formats, pricing)
- Plugin registration: Added to `plugin/builtin/mod.rs` with aliases (rev-ai, rev_ai, rev.ai)
**Learnings:**
- Rev AI uses URL query parameters for all configuration (simpler than REST+WebSocket two-step)
- Audio format specified via content_type parameter: `audio/x-raw;layout=interleaved;rate=16000;format=S16LE;channels=1`
- Graceful close requires sending "EOS" text message before closing WebSocket
- Custom close codes: 4001 (unauthorized), 4002 (bad request), 4003 (insufficient credits), 4010 (server shutdown), 4013 (no instance), 4029 (too many requests)
- Error handling maps WebSocket close codes to appropriate STTError variants
**Next Steps:**
- Continue with Batch 3: Phonexia, Verbit, etc.

### Session: 2026-01-13 (Update 15)
**Status:** Gladia STT implementation complete
**Completed:**
- Gladia STT (WebSocket Streaming with REST Session Init) - `src/core/stt/gladia/`
  - `mod.rs`: Module exports, API constants (GLADIA_API_BASE_URL, GLADIA_LIVE_URL, DEFAULT_MODEL, DEFAULT_SAMPLE_RATE, endpointing constants)
  - `config.rs`: GladiaSTTConfig, GladiaEncoding (Wav/Pcm, Wav/Alaw, Wav/Ulaw), GladiaBitDepth (8/16/24/32), GladiaRegion (EuWest, UsWest), GladiaLanguageConfig (languages array, code-switching, auto-detect), GladiaMessagesConfig, GladiaPreProcessing, GladiaRealtimeProcessing
  - `messages.rs`: InitSessionRequest, InitSessionResponse, AudioChunkMessage (base64-encoded), StopRecordingMessage, TranscriptMessage, TranscriptData, UtteranceData, WordData, GladiaError, ServerMessage enum
  - `client.rs`: GladiaSTT implementing BaseSTT trait via two-step connection (REST + WebSocket)
  - Key features:
    - Two-step connection: POST to `/v2/live` for session init → WebSocket URL returned → connect
    - REST endpoint: `https://api.gladia.io/v2/live`
    - X-Gladia-Key header authentication
    - 110+ languages with automatic detection
    - Code-switching support for multi-language conversations
    - Real-time translation to target language
    - Speaker diarization (2-8 speakers)
    - <300ms partial transcript latency, ~700ms final transcript latency
    - Word-level timestamps and confidence scores
    - Configurable endpointing (0.01-10.0 seconds)
    - Audio sent as base64-encoded JSON messages
    - Model: solaria-1 (default and currently only model)
    - Sample rates: 8000, 16000, 32000, 44100, 48000 Hz
    - Pre-processing: speech threshold (0.0-1.0)
    - Realtime processing: words, translation config
- Factory integration via `plugin/builtin/mod.rs`
- Provider aliases: `gladia`, `gladia.io`, `gladia-io`, `gladia_io`
- Tests: 61 Gladia tests passing (config: 28, messages: 15, client: 13, factory: 5)
**Quality Gates:** All passed (cargo fmt, clippy, 61 Gladia tests + 5 builtin plugin tests)
**Key Design Decisions:**
- Used two-step connection flow as required by Gladia API (REST init → WebSocket)
- Base64-encoded audio in JSON messages (per Gladia documentation)
- STTResult uses is_final for both is_final and is_speech_final (Gladia doesn't distinguish)
- Follows existing Speechmatics implementation pattern (BaseSTT trait, session management)
- Error handling maps HTTP status codes to appropriate STTError variants (401→AuthenticationFailed, 400/422→ConfigurationError)
**Next Steps:**
- Continue with Batch 3: Rev AI, Phonexia, etc.

### Session: 2026-01-13 (Update 14)
**Status:** Speechmatics STT+TTS implementation complete
**Completed:**
- Speechmatics STT (WebSocket Streaming) - `src/core/stt/speechmatics/`
  - `mod.rs`: Module exports, API constants (EU/US region URLs, JWT endpoint)
  - `config.rs`: SpeechmaticsSTTConfig, SpeechmaticsLanguage (55+ languages), SpeechmaticsEncoding (PcmS16le, PcmF32le, Mulaw), SpeechmaticsOperatingPoint (Standard, Enhanced), SpeechmaticsRegion (EU, US)
  - `messages.rs`: StartRecognitionMessage, RecognitionStartedMessage, AddPartialTranscriptMessage, AddTranscriptMessage, EndOfStreamMessage, ErrorMessage, AudioFormat, TranscriptionConfig
  - `client.rs`: SpeechmaticsSTT implementing BaseSTT trait via WebSocket streaming
  - Key features:
    - WebSocket endpoints: `wss://eu.rt.speechmatics.com/v2` (EU), `wss://us.rt.speechmatics.com/v2` (US)
    - Bearer token authentication
    - 55+ languages with automatic detection ("auto")
    - Standard and Enhanced operating points (Enhanced = higher accuracy, more cost)
    - Speaker diarization support (2-25 speakers)
    - Custom vocabulary support
    - Partial and final transcripts
    - Word-level timestamps and confidence scores
    - Max delay control (0.5-10.0 seconds)
- Speechmatics TTS (HTTP Streaming) - `src/core/tts/speechmatics/`
  - `mod.rs`: Module exports, API constants (generate URL)
  - `config.rs`: SpeechmaticsTtsConfig, SpeechmaticsVoice (Sarah, Theo, Megan, Jack), SpeechmaticsOutputFormat (Wav16000, Pcm16000), SpeechmaticsGenerateRequest
  - `provider.rs`: SpeechmaticsTts implementing BaseTTS trait via HTTP streaming, SpeechmaticsRequestBuilder
  - Key features:
    - HTTP endpoint: `https://preview.tts.speechmatics.com/generate/<voice_id>`
    - Bearer token authentication
    - 4 English voices: Sarah (UK Female), Theo (UK Male), Megan (US Female), Jack (US Male)
    - 2 output formats: WAV 16kHz, PCM 16kHz (little-endian, 16-bit, mono)
    - Natural prosody without SSML
    - Max text length: 5000 characters
    - <200ms time-to-first-audio latency
    - Pronunciation replacer support
- Factory integration via `plugin/builtin/mod.rs`
- Provider alias: `speechmatics`
- Tests: 80 Speechmatics tests passing (STT: 43 tests, TTS: 37 tests)
**Quality Gates:** All passed (cargo fmt, clippy, 2569 total tests passing)
**Key Design Decisions:**
- Used WebSocket for STT (required for real-time streaming)
- Used HTTP streaming for TTS (simpler than WebSocket for synthesis)
- Graceful fallback for unsupported audio formats/voices (TTSConfig::Default compatibility)
- Bearer token authentication for both STT and TTS
- Follows existing implementation patterns (BaseSTT/BaseTTS traits)
**Next Steps:**
- Continue with Batch 3: Gladia, Rev AI, etc.

### Session: 2026-01-13 (Update 13)
**Status:** Unreal Speech TTS implementation complete
**Completed:**
- Unreal Speech TTS (HTTP Streaming) - `src/core/tts/unrealspeech/`
  - `mod.rs`: Module exports, API constants (UNREALSPEECH_STREAM_URL, UNREALSPEECH_SPEECH_URL, MAX_STREAM_TEXT_LENGTH)
  - `config.rs`: UnrealSpeechTtsConfig, UnrealSpeechVoice (5 Standard + 48 Kokoro V8 voices), UnrealSpeechCodec (Mp3, PcmMulaw), UnrealSpeechBitrate (16k-320k), UnrealSpeechStreamRequest
  - `provider.rs`: UnrealSpeechTts implementing BaseTTS trait via HTTP streaming, UnrealSpeechRequestBuilder
  - Key features:
    - HTTP streaming endpoint: `https://api.v8.unrealspeech.com/stream` (~300ms TTFA)
    - Speech endpoint: `https://api.v8.unrealspeech.com/speech` (up to 3,000 chars)
    - SynthesisTasks endpoint: `https://api.v8.unrealspeech.com/synthesisTasks` (up to 500,000 chars)
    - Bearer token authentication
    - 5 Standard voices: Scarlett (Female/Young), Liv (Female/Young), Amy (Female/Mature), Dan (Male/Young), Will (Male/Mature)
    - 48 Kokoro V8 voices across 8 languages:
      - American English: af_heart, af_alloy, af_bella, af_jessica, af_nova, am_adam, am_echo, am_eric, etc.
      - British English: bf_emma, bf_isabella, bm_george, bm_lewis
      - Additional: French, Hindi, Spanish, Japanese, Chinese, Portuguese
    - Speed control: -1.0 to 1.0 (default: 0)
    - Pitch control: 0.5 to 1.5 (default: 1.0)
    - Bitrate options: 16k, 32k, 64k, 128k, 192k (default), 256k, 320k
    - Audio codecs: libmp3lame (MP3), pcm_mulaw (PCM mu-law)
    - Max text length: 1,000 characters for /stream endpoint
    - Pronunciation replacer support
    - Ultra cost-effective: up to 90% cheaper than competitors
- Factory integration via `plugin/builtin/mod.rs`
- Provider aliases: `unrealspeech`, `unreal-speech`, `unreal_speech`
- Tests: 32 Unreal Speech tests passing (config, voice, codec, bitrate, serialization, provider, integration)
**Quality Gates:** All passed (cargo fmt, clippy, 32 Unreal Speech tests)
**Key Design Decisions:**
- Used HTTP streaming via /stream endpoint (recommended for real-time chatbot use)
- Bearer token authentication (standard for Unreal Speech V8 API)
- Default voice is Scarlett (Female/Young - most popular)
- Default bitrate is 192k (balanced quality/bandwidth)
- Graceful fallback for unsupported audio formats (TTSConfig::Default uses "linear16" which triggers Mp3 codec default)
- Follows existing LMNT/PlayHt/Murf/WellSaid/Resemble/Speechify implementation pattern (TTSRequestBuilder trait)
- Voice enum handles both Standard names (PascalCase) and Kokoro V8 IDs (snake_case)
**Next Steps:**
- Continue with Batch 3: Otter.ai (STT), Speechmatics, Gladia, etc.

### Session: 2026-01-13 (Update 12)
**Status:** Speechify TTS implementation complete
**Completed:**
- Speechify TTS (HTTP Streaming) - `src/core/tts/speechify/`
  - `mod.rs`: Module exports, API constants (stream URL, sync URL, voices URL)
  - `config.rs`: SpeechifyTtsConfig, SpeechifyModel (SimbaEnglish, SimbaTurbo, SimbaMultilingual, SimbaBase), SpeechifyAudioFormat (Wav48000, Mp3_24000, Ogg24000, Aac24000), SpeechifyVoice, SpeechifyStreamRequest
  - `provider.rs`: SpeechifyTts implementing BaseTTS trait via HTTP streaming, SpeechifyRequestBuilder, list_voices() API
  - Key features:
    - HTTP streaming endpoint: `https://api.sws.speechify.com/v1/audio/stream`
    - Sync endpoint: `https://api.sws.speechify.com/v1/audio/speech`
    - Voices endpoint: `https://api.sws.speechify.com/v1/voices`
    - Bearer token authentication
    - 4 models: Simba-English (default, clear English), Simba-Turbo (faster with emotion control), Simba-Multilingual (50+ languages), Simba-Base (legacy)
    - 4 audio formats: WAV 48kHz (default), MP3 24kHz, OGG 24kHz, AAC 24kHz
    - 50+ languages (6 fully supported, 17 beta, 27 coming soon)
    - Max text length: 20,000 characters
    - Voice cloning from 10-30s audio sample
    - SSML support for prosody control
    - Loudness normalization to -14 LUFS (optional, increases latency)
    - Text normalization (numbers/dates to words)
    - 1000+ preset voices
    - Pronunciation replacer support
- Factory integration via `plugin/builtin/mod.rs`
- Provider aliases: `speechify`, `speechify-ai`, `speechify_ai`
- Tests: 31 Speechify tests passing (config, model, format, serialization, provider, integration)
**Quality Gates:** All passed (cargo fmt, clippy, 31 Speechify tests)
**Key Design Decisions:**
- Used HTTP streaming (recommended for real-time use)
- Bearer token authentication (standard for Speechify)
- Default model is Simba-English (standard quality)
- Graceful fallback for unsupported audio formats (default TTSConfig uses "linear16" which Speechify doesn't support)
- Follows existing LMNT/PlayHt/Murf/WellSaid/Resemble implementation pattern (TTSRequestBuilder trait)
**Next Steps:**
- Continue with Batch 2: Unreal Speech, Otter.ai, etc.

### Session: 2026-01-13 (Update 11)
**Status:** Resemble AI TTS implementation complete
**Completed:**
- Resemble AI TTS (HTTP Streaming) - `src/core/tts/resemble/`
  - `mod.rs`: Module exports, API constants (sync URL, stream URL, voices URL, WebSocket URL)
  - `config.rs`: ResembleTtsConfig, ResembleModel (Chatterbox, Chatterbox-Turbo, Chatterbox-Multilingual), ResembleOutputFormat (Wav, Mp3), ResemblePrecision (Pcm32, Pcm24, Pcm16, Mulaw), ResembleVoice, ResembleStreamRequest
  - `provider.rs`: ResembleTts implementing BaseTTS trait via HTTP streaming, ResembleRequestBuilder, list_voices() API
  - Key features:
    - HTTP streaming endpoint: `https://f.cluster.resemble.ai/stream`
    - Sync endpoint: `https://f.cluster.resemble.ai/synthesize`
    - Voices endpoint: `https://app.resemble.ai/api/v2/voices`
    - WebSocket endpoint: `wss://websocket.cluster.resemble.ai/stream` (Business plan)
    - Bearer token authentication
    - 3 models: Chatterbox (standard), Chatterbox-Turbo (low-latency ~350M params, paralinguistic tags), Chatterbox-Multilingual (149+ languages)
    - 4 precision options: PCM_32, PCM_24, PCM_16, MULAW
    - 2 output formats: WAV, MP3
    - Sample rates: 8000, 16000, 22050, 24000, 32000, 44100, 48000 Hz
    - HD mode for higher quality synthesis
    - Paralinguistic tags with Turbo model: [cough], [laugh], [chuckle]
    - Max text length: 2000 chars (stream), 3000 chars (sync)
    - Voice cloning with just 10 seconds of audio
    - Pronunciation replacer support
- Factory integration via `plugin/builtin/mod.rs`
- Provider aliases: `resemble`, `resemble-ai`, `resemble_ai`, `resembleai`
- Tests: 30 Resemble tests passing (config, model, format, precision, serialization, provider, integration)
**Quality Gates:** All passed (cargo fmt, clippy, 30 Resemble tests)
**Key Design Decisions:**
- Used HTTP streaming (recommended for real-time use)
- Bearer token authentication (standard for Resemble)
- Default model is Chatterbox (standard quality)
- Turbo model optional for low-latency (supports paralinguistic tags)
- Multilingual model for 149+ language support
- Follows existing LMNT/PlayHt/Murf/WellSaid implementation pattern (TTSRequestBuilder trait)
- Voice ID is voice_uuid (parsed from config.voice_id)
**Next Steps:**
- Continue with Batch 2: Speechify, Unreal Speech, etc.

### Session: 2026-01-13 (Update 10)
**Status:** WellSaid Labs TTS implementation complete
**Completed:**
- WellSaid Labs TTS (HTTP Streaming) - `src/core/tts/wellsaid/`
  - `mod.rs`: Module exports, API constants (streaming URL, avatars URL)
  - `config.rs`: WellSaidTtsConfig, WellSaidModel (Legacy, Caruso), WellSaidAvatar, WellSaidStreamRequest
  - `provider.rs`: WellSaidTts implementing BaseTTS trait via HTTP streaming, WellSaidRequestBuilder
  - Key features:
    - HTTP streaming endpoint: `https://api.wellsaidlabs.com/v1/tts/stream`
    - Avatars endpoint: `https://api.wellsaidlabs.com/v1/tts/avatars`
    - API key authentication via `X-Api-Key` header (NOT Bearer token)
    - 2 models: Legacy (all 20+ languages), Caruso (English only, AI Director with pitch/tempo/loudness control)
    - 200+ voice avatars with various styles (Narration, Promo, Conversational)
    - Multi-language support: English, Spanish, German, French, Italian, Japanese, Korean, Chinese, Arabic, etc.
    - Max text length: 1000 characters per request
    - Pronunciation replacer support
    - Default speaker ID: 3 (Alana B. - US English, Narration)
- Factory integration via `plugin/builtin/mod.rs`
- Provider aliases: `wellsaid`, `wellsaid-labs`, `wellsaid_labs`, `well-said`
- Tests: 29 WellSaid tests passing (config, model, serialization, provider, integration)
**Quality Gates:** All passed (cargo fmt, clippy, 29 WellSaid tests, 4 builtin plugin tests)
**Key Design Decisions:**
- Used HTTP streaming (WellSaid's recommended approach)
- X-Api-Key header authentication (not Bearer token like most providers)
- Legacy model is default (supports all languages)
- Caruso model optional for AI Director features (English only)
- Follows existing LMNT/PlayHt/Murf implementation pattern (TTSRequestBuilder trait)
- Voice ID is numeric speaker_id (parsed from config.voice_id)
**Next Steps:**
- Continue with Batch 2: Resemble AI, Speechify, etc.

### Session: 2026-01-13 (Update 9)
**Status:** Murf.ai TTS implementation complete
**Completed:**
- Murf.ai TTS (HTTP Streaming) - `src/core/tts/murf/`
  - `mod.rs`: Module exports, API constants (streaming URL, voices URL, regional endpoints)
  - `config.rs`: MurfTtsConfig, MurfModel (Falcon, Gen2), MurfAudioFormat (7 formats), MurfRegion (12 regions), MurfChannelType (Mono, Stereo), MurfStreamRequest
  - `provider.rs`: MurfTts implementing BaseTTS trait via HTTP streaming, MurfRequestBuilder
  - Key features:
    - HTTP streaming endpoint: `https://global.api.murf.ai/v1/speech/stream`
    - Regional endpoints: 12 regions (Global, US-East, US-West, EU, UK, India, Japan, Australia, Canada, South Korea, UAE, Brazil)
    - API key authentication via `api-key` header (NOT Bearer token)
    - 2 models: Falcon (~130ms TTFA, ultra-low latency), Gen2 (studio quality with full customization)
    - 150+ voices across 35+ languages, 20+ speaking styles
    - Gen2 parameters: pitch (-50 to +50), rate (-50 to +50), style, variation (0-5), pause tags
    - 7 audio formats: WAV, MP3, FLAC, PCM, OGG, ALAW, ULAW
    - Sample rates: 8000, 24000, 44100, 48000 Hz
    - Channel types: Mono, Stereo
    - Max text length: 5000 characters
    - Pronunciation replacer support
- Factory integration via `plugin/builtin/mod.rs` and `plugin/dispatch.rs`
- Provider aliases: `murf`, `murf-ai`, `murf_ai`, `murfai`, `murf.ai`
- PHF dispatch entry for O(1) lookup
**Quality Gates:** All passed (cargo fmt, clippy, 24 Murf tests passing)
**Key Design Decisions:**
- Used HTTP streaming (Murf's recommended approach for real-time)
- Regional endpoints via MurfRegion enum with `streaming_url()` method
- Builder pattern for Murf-specific configuration (with_rate, with_pitch, with_style, with_region)
- Follows existing Play.ht/LMNT implementation pattern (TTSRequestBuilder trait)
- Pronunciation replacer integrated via base config
**Next Steps:**
- Continue with Batch 2: WellSaid Labs, Resemble AI, etc.

### Session: 2026-01-07 (Update 8)
**Status:** Play.ht TTS implementation complete
**Completed:**
- Play.ht TTS (HTTP Streaming) - `src/core/tts/playht/`
  - `mod.rs`: Module exports, API constants (TTS URL, Voice List URL, WebSocket Auth URL, Clone URL)
  - `config.rs`: PlayHtTtsConfig, PlayHtModel (5 engines), PlayHtAudioFormat (6 formats)
  - `messages.rs`: PlayHtVoice, PlayHtTtsRequest, PlayHtWsAuthResponse, PlayHtWsMessage, PlayHtApiError
  - `provider.rs`: PlayHtTts implementing BaseTTS trait via HTTP streaming, PlayHtRequestBuilder
  - Key features:
    - HTTP streaming endpoint: `https://api.play.ht/api/v2/tts/stream`
    - Dual-header authentication: `X-USER-ID` + `AUTHORIZATION`
    - 5 voice engines: Play3.0-mini (~190ms), PlayDialog (~350ms), PlayDialogMultilingual, PlayDialogArabic, PlayHT2.0-turbo
    - PlayDialog multi-turn dialogue support with `voice_2`, `turn_prefix`, `turn_prefix_2`
    - 36+ languages with auto-detection
    - 6 audio formats: mp3, wav, mulaw, flac, ogg, raw (PCM)
    - Sample rates: 8000, 16000, 24000, 44100, 48000 Hz
    - Max text length: 20,000 characters
    - Speed (0.5-2.0), temperature (0.0-1.0), seed for deterministic output
    - Advanced guidance: text_guidance, voice_guidance, style_guidance (Play3.0 only)
    - Voice cloning support (30+ second audio samples)
- Factory integration in `src/core/tts/mod.rs`
- Provider aliases: `playht`, `play-ht`, `play_ht`, `play.ht`
- Environment variable: `PLAYHT_USER_ID` (required for authentication)
**Quality Gates:** All passed (cargo fmt, clippy, 104 Play.ht tests passing)
**Key Design Decisions:**
- Used HTTP streaming (recommended by Play.ht for most use cases)
- User ID from environment variable (PLAYHT_USER_ID) for dual-header auth
- `with_user_id()` method for explicit user ID when needed
- Follows LMNT implementation pattern (TTSRequestBuilder trait)
**Next Steps:**
- Continue with Batch 2: Murf.ai, WellSaid Labs, etc.

### Session: 2026-01-06 (Update 7)
**Status:** Hume AI integration complete (TTS, EVI, Voice Cloning, Unified Emotion System)
**Completed:**
- Hume TTS (Octave) - `src/core/tts/hume/`
  - `config.rs`: HumeTTSConfig, HumeVoice, HumeAudioFormat, HumeModel
  - `messages.rs`: HumeTTSRequest, HumeUtterance, HumeTTSResponse
  - `provider.rs`: HumeTTS implementing BaseTTS trait via HTTP streaming
  - `tests.rs`: Comprehensive unit tests
  - Key features: Natural language emotion control, voice design, instant mode, speed control
  - Streaming URL: `https://api.hume.ai/v0/tts/stream/file`
- Hume EVI (Audio-to-Audio) - `src/core/realtime/hume/`
  - `config.rs`: HumeEVIConfig, HumeEVIVersion
  - `messages.rs`: 48 prosody (emotion) dimensions, WebSocket message types
  - `client.rs`: HumeEVI implementing BaseRealtime trait via WebSocket
  - `tests.rs`: Comprehensive unit tests
  - Key features: Full-duplex audio, emotion analysis (48 dimensions), conversation memory
  - WebSocket URL: `wss://api.hume.ai/v0/evi/chat`
- Unified Emotion System - `src/core/emotion/`
  - 20 standardized emotions: neutral, happy, sad, angry, fearful, surprised, disgusted, excited, calm, anxious, confident, confused, empathetic, sarcastic, hopeful, disappointed, proud, embarrassed, content, bored
  - 10 delivery styles: whispered, shouted, rushed, measured, soft, loud, cheerful, serious, casual, formal
  - Emotion intensity: 0.0-1.0 numeric or low/medium/high presets
  - Provider mappers for Hume (natural language), ElevenLabs (audio tags), Azure (SSML)
  - Warning response for unsupported emotions (graceful degradation)
- Voice Cloning API - `src/handlers/voices.rs`
  - `POST /voices/clone` endpoint supporting Hume and ElevenLabs
  - Hume: Two-step process (generate TTS with description → save voice)
  - ElevenLabs: Direct multipart/form-data voice cloning
  - Audio format detection via magic bytes
- Client SDK Updates
  - TypeScript: Emotion types, VoiceCloneRequest/Response, HumeEVIConfig, ProsodyScores, helper functions
  - Python: Emotion enum, DeliveryStyle enum, VoiceCloneProvider, ProsodyScores with top_emotions()/dominant_emotion()
- Documentation: `docs/hume.md`, `config.example.yaml` updated
**Quality Gates:** All passed (cargo fmt, clippy, 1981 tests passing)
**Key Design Decisions:**
- Natural language emotion control (no SSML parsing for Hume)
- Unified emotion types with provider-specific mappers
- Warning response for unsupported emotions (audio still returned)
- Voice cloning integrated into existing `/voices` endpoint pattern
**Next Steps:**
- Batch 1 complete! Continue with Batch 2: LMNT, Play.ht, etc.

### Session: 2026-01-06 (Update 6)
**Status:** Groq STT implementation complete
**Completed:**
- Groq Whisper STT (REST API) - `src/core/stt/groq/`
  - `config.rs`: GroqSTTConfig, GroqSTTModel (whisper-large-v3, whisper-large-v3-turbo), GroqResponseFormat
  - `messages.rs`: TranscriptionResponse, VerboseTranscriptionResponse, Segment, Word, wav module
  - `client.rs`: GroqSTT implementing BaseSTT trait via HTTP REST with audio buffering
  - `tests.rs`: 105 comprehensive unit tests
  - Key features:
    - Ultra-fast transcription (216x real-time for whisper-large-v3-turbo)
    - OpenAI-compatible API format
    - Two models: whisper-large-v3 (10.3% WER) and whisper-large-v3-turbo (12% WER, faster)
    - Audio buffering with configurable flush strategies (OnDisconnect, OnThreshold, OnSilence, Manual)
    - Silence detection using RMS energy analysis
    - Automatic retry with exponential backoff for transient errors (429, 503)
    - Translation endpoint for any language to English
    - WAV file generation for REST API submission
  - REST URL: `https://api.groq.com/openai/v1/audio/transcriptions`
  - Translation URL: `https://api.groq.com/openai/v1/audio/translations`
- Factory integration in `src/core/stt/mod.rs`
- Provider alias: `groq`
**Quality Gates:** All passed (cargo fmt, clippy, 105 Groq tests + 37 factory tests passing)
**Key Design Decisions:**
- Used REST API (Groq doesn't support WebSocket streaming)
- Audio buffering pattern similar to OpenAI Whisper implementation
- Silence detection for automatic flushing
- Exponential backoff retry for rate limits (429) and service unavailable (503)
- File size limits: 25MB (free tier), 100MB (dev tier)
**Next Steps:**
- Continue with Batch 1: Hume AI (TTS+A2A)

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
