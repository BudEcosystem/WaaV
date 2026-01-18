# Vbee TTS Provider

## Status: RESEARCH_NEEDED

**Last Updated:** 2026-01-14
**Provider #:** 46
**Priority:** Medium (Southeast Asia - Vietnam)

## Overview

Vbee is a leading Vietnamese AI company specializing in Text-to-Speech and voice cloning technology. They offer TTS services with support for Vietnamese regional accents and multiple languages.

## Company Information

- **Company:** Vbee (AI voice technology company)
- **Country:** Vietnam
- **Website:** https://vbee.vn
- **API Portal:** https://api.vbee.vn
- **Technologies:** TTS, Voice Cloning

## Advertised Capabilities

Based on marketing materials:
- **50+ Languages** with 200+ voice options
- **Vietnamese Regional Voices:** Northern, Central, and Southern dialects
- **Gender variations:** Male, female, and child voices
- **Emotional speech synthesis** with natural prosody
- **Voice Cloning** technology (launched early 2025)
- **AI Dubbing** with subtitle (.srt) file support

## Audio Formats (Advertised)

- MP3
- WAV
- Additional formats (unspecified)

## API Documentation Status

### Documentation Sources Tried

| Source | URL | Status |
|--------|-----|--------|
| Official API Docs | https://docs.vbee.vn | 502 Error |
| Postman Documentation | https://documenter.getpostman.com/view/12951168/Uz5FHbSd | Not rendering properly |
| API Portal | https://api.vbee.vn | React app, no public API docs |
| GitBook Documentation | https://api-docs.vbee.vn/dac-ta-api | References Postman link only |
| GitHub | https://github.com/vbee-holding | No TTS client library |

### What We Know

From the GitBook documentation page:
- API uses **App ID** and **Token** for authentication
- REST-based API
- Callback URL integration mechanism

### What We Don't Know

- Exact API endpoints
- Authentication header format
- Request/response schemas
- Rate limits
- Error codes
- Voice IDs/options
- Audio format parameters

## Integration Blockers

1. **API Documentation Not Accessible:** Primary documentation sources return errors or don't render
2. **No Public Client Library:** No SDK or example code in GitHub repositories
3. **Postman Documentation Unavailable:** Referenced Postman collection doesn't load properly

## Recommendations

### Option 1: Contact Vbee Directly
- Email: contact@vbee.ai
- Request API documentation and developer access
- Ask for test credentials

### Option 2: Mark as BLOCKED
If unable to obtain documentation after contact attempt, mark as BLOCKED with reason "API documentation not publicly accessible"

### Option 3: Reverse Engineer
If a trial account can be created at api.vbee.vn:
1. Create account
2. Use browser dev tools to capture API calls
3. Document discovered endpoints

## Integration Pattern (If Documentation Becomes Available)

Based on similar TTS providers, likely integration would follow:
```rust
// Estimated structure - NOT VERIFIED
// POST https://api.vbee.vn/v1/synthesize
// Headers:
//   Authorization: Bearer <token>
//   Content-Type: application/json
// Body:
// {
//   "input_text": "Text to synthesize",
//   "voice": "voice_id",
//   "speed": 1.0,
//   "callback_url": "https://your-server/callback"
// }
```

## References

- [Vbee Website](https://vbee.vn)
- [Vbee API Portal](https://api.vbee.vn)
- [Vbee GitHub](https://github.com/vbee-holding)
- [Google Cloud Case Study on Vbee](https://cloud.google.com/customers/vbee)
- [G2 Reviews](https://www.g2.com/products/vbee-ai-voice-studio/reviews)

## Next Steps

1. Attempt to contact Vbee support for API documentation
2. If no response within reasonable timeframe, mark as BLOCKED
3. Document any API details obtained
4. Implement once documentation is available
