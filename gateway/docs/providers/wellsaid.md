# WellSaid Labs TTS Provider Integration

> **Status:** DONE ✓
> **Last Updated:** 2026-01-13
> **Provider Type:** TTS (Text-to-Speech)
> **Tests:** 29 unit tests passing

---

## Provider Overview

WellSaid Labs offers premium AI-generated voices with studio-quality output. The API provides 200+ voice avatars across 20+ languages with two models: Legacy (all languages) and Caruso (English only with AI Director features).

### Key Features

| Feature | Support |
|---------|---------|
| TTS Streaming | Yes (HTTP streaming) |
| Voice Cloning | Yes (Custom Voice Avatars - Enterprise) |
| Languages | 20+ (English, Spanish, German, French, Italian, Japanese, Korean, Chinese, Arabic, etc.) |
| Voice Avatars | 200+ production-ready voices |
| AI Director | Yes (Caruso model - pitch, tempo, loudness, respelling) |
| SSML Support | Yes (AI Director markup tags) |
| Audio Formats | MP3 (default), WAV |
| Sample Rates | Configurable |
| Max Text Length | 1000 characters per request |

### Pricing

| Plan | Price | Downloads/Month | Voice Avatars |
|------|-------|-----------------|---------------|
| Maker | $49/mo | 250 | 24 |
| Creative | $99/mo | 750 | 53 |
| Business | $179/mo | 9,000 | All + Integrations |
| Enterprise | Custom | Unlimited | Custom + SSO + Support |

---

## API Documentation

### Base URL

```
https://api.wellsaidlabs.com/v1
```

### Authentication

WellSaid Labs uses API key authentication via the `X-Api-Key` header (NOT Bearer token).

```http
X-Api-Key: YOUR_API_KEY
```

**Important:** The API does not support end-user authentication. Requests should originate from internal or trusted sources only.

### Environment Variable

```bash
WELLSAID_API_KEY=your-api-key-here
```

---

## Endpoints

### 1. Text-to-Speech Streaming

Converts text to audio stream.

**Endpoint:** `POST /tts/stream`

**Headers:**
| Header | Value | Required |
|--------|-------|----------|
| X-Api-Key | API key | Yes |
| Content-Type | application/json | Yes |
| Accept | audio/mpeg | Yes |

**Request Body:**
```json
{
  "speaker_id": 3,
  "text": "Hello, world!",
  "model": "caruso"
}
```

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| speaker_id | number | Yes | Voice avatar ID (see Available Avatars) |
| text | string | Yes | Text to synthesize (max 1000 chars) |
| model | string | No | "legacy" (default) or "caruso" (English only) |

**Response:** Audio stream (MP3)

**Example:**
```bash
curl --location --request POST 'https://api.wellsaidlabs.com/v1/tts/stream' \
  --header 'X-Api-Key: YOUR_API_KEY' \
  --header 'Accept: audio/mpeg' \
  --header 'Content-Type: application/json' \
  --data-raw '{
    "text": "Hello world!",
    "speaker_id": 3
  }' > hello_world.mp3
```

### 2. List Avatars

Retrieves available voice avatars.

**Endpoint:** `GET /tts/avatars`

**Headers:**
| Header | Value | Required |
|--------|-------|----------|
| X-Api-Key | API key | Yes |

**Response:**
```json
[
  {
    "avatar_id": "alana-b",
    "speaker_id": 3,
    "name": "Alana B.",
    "gender": "Female",
    "accent": "US English",
    "style": "Narration",
    "models": ["caruso", "legacy"]
  }
]
```

**Example:**
```bash
curl --location 'https://api.wellsaidlabs.com/v1/tts/avatars' \
  --header 'X-API-KEY: YOUR_API_KEY'
```

---

## Models

### Legacy Model

- **ID:** `"legacy"`
- **Status:** Default model
- **Languages:** All languages
- **Features:** Natural-sounding speech, core voice features
- **Use Case:** Multi-language applications

### Caruso Model

- **ID:** `"caruso"`
- **Languages:** English only
- **Features:** AI Director capabilities
  - Pitch adjustment (`<pitch value="-250">...</pitch>`)
  - Tempo control (`<tempo value="0.8">...</tempo>`)
  - Loudness modification (`<loudness value="+5">...</loudness>`)
  - Respelling for pronunciation

**Caruso Example:**
```json
{
  "speaker_id": 26,
  "model": "caruso",
  "text": "<pitch value=\"-250\">This sentence feels deeper and more serious.</pitch>"
}
```

---

## Voice Avatars (Sample)

| Speaker ID | Name | Gender | Accent | Style |
|------------|------|--------|--------|-------|
| 3 | Alana B. | Female | US English | Narration |
| 8 | Sofia H. | Female | US English | Conversational |
| 26 | Joe F. | Male | US English | Promo |
| 56 | Jarvis H. | Male | England English | Narration |
| 163 | Rachin K. | Male | Hindi English | Conversational |

### Styles
- **Narration:** Informative, Professorial, Trustworthy
- **Promo:** Energetic, Upbeat, Engaging
- **Conversational:** Friendly, Relaxed, Casual

---

## Implementation Plan

### Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    WellSaid TTS Provider                     │
├─────────────────────────────────────────────────────────────┤
│  WellSaidTtsConfig                                          │
│  ├── api_key: String                                        │
│  ├── speaker_id: u32                                        │
│  ├── model: WellSaidModel (Legacy, Caruso)                  │
│  └── format: WellSaidFormat (Mp3, Wav)                      │
├─────────────────────────────────────────────────────────────┤
│  WellSaidTts (BaseTTS implementation)                       │
│  ├── new(TTSConfig) -> Result<Self>                        │
│  ├── connect() -> HTTP client initialization                │
│  ├── speak(text, flush) -> Stream audio                    │
│  └── list_avatars() -> Vec<WellSaidAvatar>                  │
├─────────────────────────────────────────────────────────────┤
│  WellSaidRequestBuilder (TTSRequestBuilder)                 │
│  └── build_http_request() -> POST /tts/stream              │
└─────────────────────────────────────────────────────────────┘
```

### File Structure

```
src/core/tts/wellsaid/
├── mod.rs          # Module exports, constants
├── config.rs       # WellSaidTtsConfig, WellSaidModel, WellSaidFormat
└── provider.rs     # WellSaidTts, WellSaidRequestBuilder
```

### Constants

```rust
pub const WELLSAID_TTS_STREAM_URL: &str = "https://api.wellsaidlabs.com/v1/tts/stream";
pub const WELLSAID_AVATARS_URL: &str = "https://api.wellsaidlabs.com/v1/tts/avatars";
pub const DEFAULT_SPEAKER_ID: u32 = 3; // Alana B.
pub const MAX_TEXT_LENGTH: usize = 1000;
```

### Configuration

```rust
pub struct WellSaidTtsConfig {
    pub api_key: String,
    pub speaker_id: u32,
    pub model: WellSaidModel,
}

pub enum WellSaidModel {
    Legacy,  // Default, all languages
    Caruso,  // English only, AI Director
}
```

### Request Body

```rust
#[derive(Serialize)]
pub struct WellSaidStreamRequest {
    pub speaker_id: u32,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}
```

---

## Testing Plan

### Unit Tests

1. `test_config_defaults` - Verify default speaker_id and model
2. `test_config_validation` - Empty API key fails
3. `test_request_builder_headers` - X-Api-Key header set correctly
4. `test_stream_request_serialization` - JSON body structure
5. `test_model_enum` - Model string conversion
6. `test_text_length_validation` - Max 1000 chars

### Integration Tests

1. `test_wellsaid_connection` - Connect/disconnect cycle
2. `test_wellsaid_list_avatars` - Fetch avatar list
3. `test_wellsaid_synthesis` - Generate audio from text
4. `test_wellsaid_caruso_model` - AI Director features

---

## Error Handling

| HTTP Status | Meaning | Action |
|-------------|---------|--------|
| 200 | Success | Stream audio |
| 400 | Bad Request | Invalid parameters |
| 401 | Unauthorized | Invalid API key |
| 403 | Forbidden | Plan limit exceeded |
| 429 | Rate Limited | Retry with backoff |
| 500 | Server Error | Retry |

---

## Quality Gates

- [x] `cargo fmt --check` passes
- [x] `cargo clippy -- -D warnings` passes
- [x] All unit tests pass (29 tests)
- [ ] Integration tests pass (with credentials)
- [x] Factory registration complete
- [x] Provider aliases registered (`wellsaid`, `wellsaid-labs`, `wellsaid_labs`, `well-said`)
- [x] Documentation updated
- [x] Environment variable documented (`WELLSAID_API_KEY`)

---

## References

- [WellSaid Labs API Documentation](https://docs.wellsaidlabs.com)
- [Getting Started Guide](https://docs.wellsaidlabs.com/reference/getting-started-with-your-api)
- [Available Voice Avatars](https://docs.wellsaidlabs.com/reference/available-voice-avatars)
- [Model Selection](https://docs.wellsaidlabs.com/reference/model-selection-with-the-api)
- [WellSaid Help Center](https://help.wellsaidlabs.com)
