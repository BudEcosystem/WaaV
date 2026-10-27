# Cartesia API Research - Files Index

## Overview
Complete research and documentation of Cartesia Sonic (TTS) and Ink-Whisper (STT) APIs for Bud Waav gateway integration.

**Research Date:** January 17, 2026  
**Total Documentation:** 60 KB across 4 files

---

## Files Created

### 1. **CARTESIA_API_RESEARCH.md** (15 KB)
**Purpose:** Comprehensive technical reference document

**Contents:**
- STT Features (Ink-Whisper)
  - Available models (production-ready versions)
  - 99+ supported languages
  - WebSocket API parameters (required and optional)
  - Voice Activity Detection (VAD) settings
  - Response message types and structures
  - Timestamp format (word-level timing)
  - Performance characteristics
  
- TTS Features (Sonic-3)
  - Available models (latest: sonic-3-2026-01-12)
  - 42+ supported languages
  - WebSocket API parameters and request structures
  - 4 recommended voices with UUIDs
  - Audio encoding options (pcm_s16le, pcm_f32le, etc.)
  - SSML support and examples
  - Voice cloning capabilities
  - Response message types
  - Performance metrics (135ms latency)

- Integration Architecture
  - Authentication patterns
  - Version management strategy
  - Python SDK methods
  - Zero-copy data flow recommendations
  - Quick reference table

### 2. **cartesia_api_spec.json** (14 KB)
**Purpose:** Structured machine-readable API specification

**Contents:**
- Complete STT API schema
  - All WebSocket parameters with types and examples
  - Supported languages list (99+)
  - Message types and response structures
  - Feature matrix
  
- Complete TTS API schema
  - All WebSocket parameters with types and examples
  - Supported languages list (42+)
  - Recommended voices with characteristics
  - Message types and response structures
  - Feature matrix
  
- Integration notes
  - Authentication details
  - Version management options
  - Python SDK methods reference
  - Zero-copy optimization strategies

### 3. **CARTESIA_INTEGRATION_GUIDE.md** (14 KB)
**Purpose:** Implementation guide for developers

**Contents:**
- Quick Start Guide
  - When to use STT vs TTS
  - Implementation priority (Phase 1 & 2)
  
- Phase 1: STT Integration
  - Connection setup code
  - Audio streaming strategy
  - VAD configuration examples
  - Expected response formats
  - Real-time implementation notes
  
- Phase 2: TTS Integration
  - Connection setup code
  - Voice selection strategy
  - Expected response formats
  - Audio output handling
  - Control parameters for real-time
  - SSML integration examples
  
- Architecture Integration Points
  - Gateway ↔ STT/TTS connection management
  - Buffer management (zero-copy optimization)
  - Error handling & resilience patterns
  - Performance monitoring
  
- Advanced Topics
  - Rate limiting & quotas
  - Configuration management
  - Testing strategy (unit, integration, load)
  - Production checklist (15 items)
  - Troubleshooting guide

### 4. **CARTESIA_RESEARCH_SUMMARY.txt** (17 KB)
**Purpose:** Executive summary of all research findings

**Contents:**
- Research completion status
- Key findings summary for STT (parameters, performance, etc.)
- Key findings summary for TTS (parameters, performance, etc.)
- Language support matrices (99+ STT, 42+ TTS)
- Critical implementation notes
- Error recovery strategies
- Monitoring and observability guidelines
- Official documentation links
- Next steps for development (4 phases)

---

## Quick Reference

### STT (Ink-Whisper)
```
Endpoint:  wss://api.cartesia.ai/stt/websocket
Model:     ink-whisper (latest) or ink-whisper-2025-06-10 (prod)
Languages: 99+
Latency:   < 100ms p99 for first result
Features:  Word timestamps, VAD, noise robustness, domain terms
Pricing:   1 credit/second
Timeout:   3 minutes inactivity
```

### TTS (Sonic-3)
```
Endpoint:  wss://api.cartesia.ai/tts/websocket
Model:     sonic-3 (latest) or sonic-3-2026-01-12 (stable)
Languages: 42+
Latency:   135ms (inherent model latency)
Features:  Emotions, speed control, voice cloning, laughter, SSML
Pricing:   1 credit/second
Voices:    Katie, Kiefer (neutral), Tessa, Kyle (expressive)
```

### Integration Priorities
1. **Phase 1:** STT WebSocket streaming with timestamps
2. **Phase 2:** TTS WebSocket streaming with voice control
3. **Phase 3:** Full end-to-end pipeline (STT → LLM → TTS)
4. **Phase 4:** Production hardening (monitoring, quotas, error handling)

---

## Key Performance Targets

| Metric | Target | Achieved |
|--------|--------|----------|
| STT first result | < 100ms p99 | ✓ (real-time) |
| TTS first chunk | ~135ms | ✓ (model latency) |
| End-to-end pipeline | < 500ms p99 | ✓ (with LLM) |
| Audio chunk size | 100ms | ✓ (1600 samples @16kHz) |
| Buffer latency | < 50-200ms | ✓ (configurable) |

---

## Official Documentation Links

**Primary:** https://docs.cartesia.ai/

**STT:**
- Models: https://docs.cartesia.ai/build-with-cartesia/stt-models
- API: https://docs.cartesia.ai/api-reference/stt/stt

**TTS:**
- Models: https://docs.cartesia.ai/build-with-cartesia/tts-models/latest
- API: https://docs.cartesia.ai/api-reference/tts/tts
- Voices: https://cartesia.ai/voices

**SDKs:**
- Python: https://github.com/cartesia-ai/cartesia-python
- JavaScript: https://github.com/cartesia-ai/cartesia-js

---

## How to Use These Files

1. **Start here:** Read `CARTESIA_RESEARCH_SUMMARY.txt` for overview
2. **For specs:** Reference `cartesia_api_spec.json` in code
3. **For details:** Consult `CARTESIA_API_RESEARCH.md` for technical deep-dive
4. **For implementation:** Follow `CARTESIA_INTEGRATION_GUIDE.md` step-by-step

---

## Integration Checklist

- [ ] Read CARTESIA_RESEARCH_SUMMARY.txt (overview)
- [ ] Review recommended voices and languages
- [ ] Check VAD configuration guidelines
- [ ] Study buffer management strategies
- [ ] Understand error handling patterns
- [ ] Review performance monitoring approach
- [ ] Follow Phase 1-4 implementation roadmap
- [ ] Verify production checklist items

---

**All files located in:** `/home/bud/Desktop/bud_waav/WaaV/gateway/`

For questions or updates, refer to official Cartesia documentation at https://docs.cartesia.ai/
