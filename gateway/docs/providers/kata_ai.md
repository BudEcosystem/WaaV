# Kata.ai Provider Documentation

> **Provider #51** | Indonesian Conversational AI Platform
> **Company:** Kata.ai (PT YesBoss Group Indonesia)
> **Website:** https://kata.ai | https://katalabs.io
> **Status:** BLOCKED - No Public API Access
> **Last Updated:** 2026-01-14

---

## Overview

Kata.ai is an Indonesian conversational AI company founded in 2015 that specializes in Natural Language Processing (NLP) for Bahasa Indonesia. They power virtual assistants for major corporations in Indonesia across financial services, telecommunications, retail, and FMCG industries.

### Company Details

- **Founded:** 2015
- **Location:** Jakarta, Indonesia
- **Enterprise Clients:** 100+ (Bank BRI, Telkomsel, Unilever, Indosat, CIMB Niaga, etc.)
- **Languages Supported:** 30+ languages (Indonesian, English, etc.)

---

## Products

### Kata Platform
Core conversational AI development platform for building chatbots.

### Kata Voice
Voice-to-text and text-to-voice API integrated into their conversational AI platform.

**Claimed Features:**
- Speech-to-Text API
- Text-to-Speech API
- Voice Analytics
- Topic Classification
- Sentiment Analysis
- PSTN Integration
- Call Recording & Analysis

### Other Products
- Kata CX (Customer Experience)
- Kata Omnichat
- Kata Flow
- Kata NL (Natural Language)
- AI Call Agents
- Digital Avatars

---

## Integration Status: BLOCKED

### Reason for Blocking

After extensive research, Kata.ai's voice APIs are **NOT publicly accessible** for the following reasons:

1. **Enterprise-Only Access**: Kata Voice is marketed exclusively to enterprise clients
2. **No Public API Documentation**: The docs.kata.ai website only covers chatbot platform features, not voice APIs
3. **Contact Sales Required**: All voice API access requires direct contact with their sales team
4. **Integrated Platform**: Voice APIs are part of their conversational AI platform, not standalone services
5. **No Developer Portal**: No self-service API key generation or sandbox environment

### Research Conducted

| Source | Finding |
|--------|---------|
| https://docs.kata.ai | Only chatbot documentation, no voice API docs |
| https://kata.ai/products/kata-voice | Product page with no technical details |
| https://github.com/kata-ai | 34 repositories, none related to voice/speech |
| https://katalabs.io | Marketing site, no API documentation |
| Web searches | No public API endpoints or authentication methods found |

---

## Pricing

### Conversational AI Platform
- Starting from IDR 5,000,000/month (~$300-320 USD)
- Custom enterprise pricing available
- Contact: business@kata.ai

### Voice API
- Enterprise pricing only
- Contact sales for quotes
- Phone: +62-21-50982692

---

## Alternative Indonesian Providers

Since Kata.ai is not publicly accessible, consider these alternatives for Indonesian language support:

| Provider | Status | Languages |
|----------|--------|-----------|
| Prosa.ai | [DONE] | Indonesian, English |
| Google Cloud | [DONE] | Indonesian + 125 languages |
| Azure Speech | [DONE] | Indonesian + 100 languages |
| Deepgram | [DONE] | Indonesian + 36 languages |

---

## Potential Future Integration

If Kata.ai releases public API documentation in the future, the integration would likely follow this approach:

### STT Integration Points
- REST API for file transcription
- WebSocket for real-time streaming (if supported)
- Indonesian language model

### TTS Integration Points
- REST API for synthesis
- Indonesian voice options
- Audio format support

### Required Information (When Available)
- API base URL
- Authentication method (API key, OAuth, etc.)
- Request/response formats
- Rate limits
- Pricing per request

---

## Contact Information

For enterprise inquiries:
- **Email:** business@kata.ai
- **Phone:** +62-21-50982692
- **Website:** https://kata.ai/contact

---

## References

- [Kata.ai Website](https://kata.ai)
- [Kata Labs](https://katalabs.io)
- [Kata.ai Documentation](https://docs.kata.ai)
- [Kata.ai GitHub](https://github.com/kata-ai)
- [The Asian Banker - Kata.ai Article](https://www.theasianbanker.com/updates-and-articles/kata-ai-expands-conversational-ai-for-financial-services-and-beyond)
