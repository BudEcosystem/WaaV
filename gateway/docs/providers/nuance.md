# Nuance Mix Integration Research

> **Status:** BLOCKED - Deprecation (EOL May 31, 2027)
> **Research Date:** 2026-01-13
> **Recommendation:** Use Microsoft Azure Speech (already integrated) instead

---

## Executive Summary

**Nuance Mix APIs are scheduled for end-of-life on May 31, 2027.** Microsoft (which acquired Nuance in March 2022 for $19.7B) recommends customers migrate to Azure AI Speech services. Since WaaV Gateway already has Azure Speech integration, implementing Nuance Mix would add a provider with limited lifespan.

---

## Deprecation Timeline

| Date | Event |
|------|-------|
| March 2022 | Microsoft completes acquisition of Nuance |
| August 9, 2024 | Microsoft discontinues sale of Nuance Enterprise hosted/on-premise licenses |
| December 2025 | Hosted support ends |
| June 2026 | On-premise sustaining support ends |
| **May 31, 2027** | **Nuance Mix APIs EOL (ASR, TTS, Dialog)** |
| 2027-2028 | Product family end-of-support dates |

---

## Provider Information

### Company Details
- **Name:** Nuance Communications (Microsoft)
- **Website:** https://www.nuance.com
- **API Documentation:** https://docs.nuance.com/mix/
- **Pricing:** Contact Sales (Enterprise-focused)
- **Acquisition:** Microsoft acquired Nuance in March 2022 for $19.7B

### Supported Services
| Service | Status | Notes |
|---------|--------|-------|
| STT (ASRaaS) | Available until EOL | Krypton engine |
| TTS (TTSaaS) | Available until EOL | Vocalizer engine |
| Neural TTS | Available until EOL | Uses Microsoft Azure voices |
| NLU | Available until EOL | Natural Language Understanding |
| Dialog | Available until EOL | Dialog management |

---

## Technical Specifications

### Service Endpoints (US Region)

| Service | gRPC Endpoint | Port |
|---------|---------------|------|
| ASRaaS | `asr.api.nuance.com` | 443 |
| TTSaaS | `tts.api.nuance.com` | 443 |
| NLUaaS | `nlu.api.nuance.com` | 443 |
| DLGaaS | `dlg.api.nuance.com` | 443 |
| Neural TTS | `tts.api.nuance.com` | 443 (with `x-nuance-tts-neural` header) |
| Event Logs | `log.api.nuance.com` | 443 |
| Mix.api | `mix.api.nuance.com/v4` | 443 |
| **Auth** | `auth.crt.nuance.com` | 443 |

### Other Regions
- **Europe:** Contact Nuance for EU-specific endpoints
- **Canada:** Contact Nuance for CA-specific endpoints
- **Australia:** Contact Nuance for AU-specific endpoints

### Authentication

**OAuth 2.0 Client Credentials Flow:**

1. Obtain service account from Nuance representative
2. Generate client ID and secret
3. Request access token:

```bash
export CLIENT_ID="appID:your_client_id"
export SECRET="your_secret"
curl -s -u "$CLIENT_ID:$SECRET" \
  "https://auth.crt.nuance.com/oauth2/token" \
  -d "grant_type=client_credentials" \
  -d "scope=asr nlu tts dlg"
```

**OAuth Scopes:**
- `asr` - ASRaaS runtime API access
- `nlu` - NLUaaS runtime API access
- `dlg` - DLGaaS runtime API access
- `tts` - TTSaaS runtime API access

**Token Usage:**
```python
call_credentials = grpc.access_token_call_credentials(access_token)
ssl_credentials = grpc.ssl_channel_credentials()
channel_credentials = grpc.composite_channel_credentials(ssl_credentials, call_credentials)
channel = grpc.secure_channel(hostaddr, credentials=channel_credentials)
```

**Important:** Token should be reused until expiry (generating new tokens adds latency and has stricter rate limits).

---

## ASR (Speech-to-Text) Details

### Audio Formats Supported

| Format | Sample Rates | Notes |
|--------|--------------|-------|
| PCM (Linear16) | 8000 Hz, 16000 Hz | Mono only |
| A-law | 8000 Hz | G.711 telephony |
| µ-law | 8000 Hz | G.711 telephony |
| Opus (raw) | 8000 Hz, 16000 Hz | RFC 6716 |
| Ogg Opus | 8000 Hz, 16000 Hz | RFC 7845 |

### Result Types
- **FINAL** (default): Most likely hypothesis only
- **PARTIAL**: Additional hypotheses during recognition
- **IMMUTABLE_PARTIAL**: Refined partial results

### Utterance Detection Modes
- **SINGLE** (default): First utterance only
- **MULTIPLE**: All utterances in stream
- **DISABLED**: No utterance separation

### Languages
- 86+ languages supported
- Example: `en-US`, `es-MX`, `fr-CA`, `de-DE`, `ja-JP`, `cmn-CN`

### Proto File Structure
```
nuance/
├── asr/v1/
│   ├── recognizer.proto
│   ├── resource.proto
│   └── result.proto
└── rpc/
    ├── error_details.proto
    ├── status.proto
    └── status_code.proto
```

---

## TTS (Text-to-Speech) Details

### Audio Formats Supported

| Format | Default Sample Rate | Notes |
|--------|---------------------|-------|
| PCM WAV | 22050 Hz | Default format |
| A-law | Configurable | G.711 |
| µ-law | Configurable | G.711 |
| Opus | Configurable | |
| Ogg Opus | Configurable | |

### Input Types
1. **Plain Text**: Simple text input
2. **SSML**: W3C SSML Specification Version 1.1 (most elements supported)
3. **Nuance Control Codes**: Proprietary tokenized sequences

### Voices

**Total:** 170+ voices across 40+ languages

**Voice Models:**
- **Standard**: Basic synthesis
- **Enhanced**: Higher quality synthesis
- **Multilingual (-Ml suffix)**: Can speak multiple languages

**Sample US English Voices:**
| Voice | Gender | Model | Notes |
|-------|--------|-------|-------|
| Allison | Female | Standard | |
| Ava-Ml | Female | Enhanced | Multilingual (Spanish) |
| Evan | Male | Enhanced | |
| Nathan | Male | Enhanced | |
| Samantha | Female | Standard | |
| Tom | Male | Standard | |
| Zoe-Ml | Female | Enhanced | Multilingual (French, Spanish) |

**Language Coverage:**
- English (US, UK, AU, IN)
- Spanish (MX, ES, AR, CO, CL)
- French (FR, CA, BE)
- German (DE)
- Japanese (JP) - 9 voices
- Chinese (Mandarin, Cantonese)
- And 30+ more languages

### Proto File Structure
```
nuance/
├── tts/v1/
│   └── nuance_tts_v1.proto (or synthesizer.proto)
└── rpc/
    ├── error_details.proto
    ├── status.proto
    └── status_code.proto
```

---

## Why BLOCKED

### Primary Reasons

1. **EOL Date:** May 31, 2027 - Limited remaining lifespan (~16 months from 2026-01)
2. **Microsoft Recommendation:** Azure AI Speech is the official migration path
3. **Existing Integration:** WaaV Gateway already has Azure Speech provider
4. **Enterprise-Only Access:** Requires Nuance representative to obtain service account
5. **No Self-Service:** OAuth 2.0 Client Credentials flow must be enabled by Nuance

### Alternative: Azure Speech (Already Integrated)

WaaV Gateway has **Microsoft Azure Speech** fully integrated with:
- STT: Real-time transcription, 100+ languages
- TTS: 400+ neural voices, 140+ languages/locales
- HD voices (GA March 2025)
- WebSocket and REST APIs
- Similar or better feature parity

---

## If Implementation Were Required

### Implementation Pattern (Not Implemented)

If a customer specifically requires Nuance Mix integration before EOL:

1. **Protocol:** gRPC with tonic crate
2. **Auth:** OAuth 2.0 Client Credentials
3. **Pattern:** Similar to Tinkoff VoiceKit (gRPC bidirectional streaming)
4. **Proto Generation:** Download from Nuance docs, compile with prost

### Estimated Effort
- STT: ~300-400 LOC
- TTS: ~250-350 LOC
- Tests: ~60-80 tests
- Total: 2-3 days

### Files That Would Be Created
```
src/core/stt/nuance/
├── mod.rs
├── config.rs
├── messages.rs  # From proto compilation
├── grpc.rs
└── client.rs

src/core/tts/nuance/
├── mod.rs
├── config.rs
├── messages.rs  # From proto compilation
└── provider.rs
```

---

## References

### Official Documentation
- [Nuance Mix Documentation](https://docs.nuance.com/mix/)
- [ASR gRPC API](https://docs.nuance.com/mix/apis/asr-grpc/)
- [TTS gRPC API](https://docs.nuance.com/mix/apis/tts-grpc/)
- [Neural TTS gRPC API](https://docs.nuance.com/mix/apis/ntts-grpc/)
- [OAuth Authorization](https://docs.nuance.com/mix/apis/mix-api/authorization/authorization_client_credentials/)
- [Runtime APIs Quick Reference](https://docs.nuance.com/mix/apis/quickref/)

### Deprecation Announcements
- [Genesys Cloud Nuance Deprecation Notice](https://help.mypurecloud.com/announcements/deprecation-nuance-recognizer-as-a-service-nuance-tts-and-nuance-mix-bot/)
- [ReadSpeaker Nuance Migration Guide](https://www.readspeaker.com/blog/nuance-migration-plan/)

### GitHub Resources
- [Nuance Mix Demo Client](https://github.com/nuance-communications/mix-demo-client-azstaticwebapps)
- [Nuance TTS Voices Gist](https://gist.github.com/davehorton/47795673f4014a5422ff7ab3dbc28bfb)

---

## Conclusion

**Recommendation:** Do NOT implement Nuance Mix integration. Use Microsoft Azure Speech instead, which:
1. Is already integrated in WaaV Gateway
2. Is Microsoft's official recommended replacement
3. Has active development and no deprecation timeline
4. Provides similar or superior feature coverage
5. Offers self-service access (no sales contact required)

---

*Last Updated: 2026-01-13*
