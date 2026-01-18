# Acapela Cloud TTS Provider

## Overview

Acapela Cloud is a text-to-speech service by Acapela Group (Belgium) offering 250+ AI neural voices across 30+ languages. The service provides HTTP streaming with word/viseme position tracking for lip-sync and highlighting applications.

## API Details

- **Base URL**: `https://www.acapela-cloud.com`
- **Authentication**: Email/password login returning session token
- **Protocol**: HTTP REST with streaming support
- **Documentation**: https://www.acapela-cloud.com/docs_api/

## Authentication

### Login

```http
POST /api/login/
Content-Type: application/x-www-form-urlencoded

email=user@example.com&password=secret
```

**Response (200):**
```json
{"token": "abc123..."}
```

**Error Responses:**
- `401`: Inactive account or invalid credentials
- `400`: General error

### Logout

```http
GET /api/logout/
Authorization: Token abc123...
```

## TTS Synthesis

### Endpoint

```http
GET/POST /api/command/
Authorization: Token abc123...
```

### Required Parameters

| Parameter | Type | Description |
|-----------|------|-------------|
| `voice` | string | Voice identifier (e.g., "alice", "graham") |
| `text` | string | Text to synthesize (max 2048 chars GET, 3000 for stream) |

### Optional Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `output` | string | "stream" | Output mode: stream, file, events |
| `type` | string | "mp3" | Audio format (see Audio Formats) |
| `samplerate` | int | 22050 | Sample rate: 8000-48000 Hz |
| `bitrate` | int | varies | Bitrate: 24-320 kbps (codec-dependent) |
| `speed` | int | 100 | Speech rate: 30-300 (100 = normal) |
| `volume` | int | 32768 | Volume: 50-65535 |
| `shaping` | int | 100 | Voice shaping: 50-150 |
| `wordpos` | string | "off" | Word position events: on/off |
| `mouthpos` | string | "off" | Viseme/mouth animation: on/off |
| `markpos` | string | "off" | Text marker positions: on/off |
| `dico` | string | - | Custom dictionary files (.dic, comma-separated) |
| `application` | string | - | Application ID for per-app statistics |

### Audio Formats

| Format | Extension | Description |
|--------|-----------|-------------|
| `mp3` | .mp3 | MPEG Audio Layer 3 |
| `ogg` | .ogg | Ogg Vorbis |
| `wav` | .wav | Waveform Audio |
| `flac` | .flac | Free Lossless Audio Codec |
| `ac3` | .ac3 | Dolby Digital |
| `asf` | .asf | Advanced Systems Format |
| `wma` | .wma | Windows Media Audio |
| `opus` | .opus | Opus Interactive Audio |
| `aac` | .aac | Advanced Audio Coding |
| `aiff` | .aiff | Audio Interchange File Format |
| `webm` | .webm | WebM Audio |
| `mka` | .mka | Matroska Audio |
| `s16le` | .raw | Raw PCM 16-bit signed little-endian |
| `alaw` | .raw | A-law companding |
| `mulaw` | .raw | μ-law companding |
| `wav_mulaw` | .wav | WAV with μ-law encoding |
| `wav_alaw` | .wav | WAV with A-law encoding |

### Streaming Response

When `output=stream`, the response uses a mixed protocol:

```
type:size\n
content
```

Where:
- `type`: "audio" or "events"
- `size`: Content length in bytes
- `content`: Audio bytes or JSON event data

### Event Format

```json
{
  "Word": [
    {
      "word": "Hello",
      "start_time": 0,
      "end_time": 500,
      "start_sample": 0,
      "end_sample": 8000
    }
  ],
  "Phoneme": [
    {
      "phoneme": "h",
      "viseme": 12,
      "start_time": 0,
      "end_time": 100
    }
  ]
}
```

### Viseme Codes (Disney Standard)

| Code | Phonemes | Mouth Shape |
|------|----------|-------------|
| 0 | silence | Closed |
| 1 | ae, ax, ah | Open |
| 2 | aa | Wide open |
| 3 | ao | Rounded open |
| 4 | ey, eh, uh | Half open |
| 5 | er | R-colored |
| 6 | y, iy, ih, ix | Smile |
| 7 | w, uw | Rounded |
| 8 | ow | O shape |
| 9 | aw | Wide O |
| 10 | oy | OI diphthong |
| 11 | ay | AI diphthong |
| 12 | h | Breath |
| 13 | r | R |
| 14 | l | L |
| 15 | s, z | S/Z |
| 16 | sh, ch, jh, zh | SH |
| 17 | th, dh | TH |
| 18 | f, v | F/V |
| 19 | d, t, n | D/T/N |
| 20 | k, g, ng | K/G |
| 21 | p, b, m | P/B/M |

## Available Voices

### Languages (30+)

Arabic, Catalan, Chinese (Mandarin), Czech, Danish, Dutch (Belgium/Netherlands), English (AU/CA/IN/UK/US/Scotland/North), Faroese, Finnish, French (France/Canada), German, Greek, Hindi, Italian, Japanese, Korean, Norwegian, Polish, Portuguese (Portugal/Brazil), Russian, Sami (North), Spanish (Spain/US), Swedish (Sweden/Finland), Turkish

### Voice Naming Convention

Voice identifiers follow the pattern: `{name}` or `{name}22_HQ`

Examples:
- `alice` - French female
- `graham` - UK English male
- `lily` - US English female
- `sakura` - Japanese female

### Sample Voices by Language

| Language | Female | Male | Child |
|----------|--------|------|-------|
| English (US) | Lily, Taylor, Tamira | Will, Micah, Darius | Alinora, Jorvik |
| English (UK) | Lucy, Rachel, Sophia | Graham, Peter, Harry | Amelia, Arthur, Rosie |
| French | Alice, Claire, Julie | Antoine, Bruno | Elise, Valentin |
| German | Claudia, Julia, Sarah | Andreas, Klaus | Finn, Jonas, Lea |
| Spanish | Ana, Elena, Maria | Antonio | - |
| Italian | Barbara, Chiara, Fabiana | Vittorio | Alessio, Aurora |

## Account Management

### Get Account Info

```http
GET /api/account/
Authorization: Token abc123...
```

**Response:**
```json
{
  "email": "user@example.com",
  "credits": 10000,
  "voices": ["alice", "graham", "lily"],
  "first_name": "John",
  "last_name": "Doe"
}
```

### Usage Statistics

```http
GET /api/stats/?type=credit&interval=month
Authorization: Token abc123...
```

**Types:** voice, command, credit, billing, purchase

## Error Codes

| Code | Description |
|------|-------------|
| 200 | Success |
| 400 | Insufficient credits or invalid parameters |
| 401 | Invalid or missing token |
| 403 | Not authenticated |

## Implementation Notes

1. **Session Management**: Token must be included in Authorization header for all requests after login
2. **Credit System**: Synthesis consumes credits based on text length and voice type
3. **Streaming**: Use `output=stream` for real-time audio delivery
4. **Events**: Enable `wordpos=on` for word-level timing (useful for highlighting)
5. **Visemes**: Enable `mouthpos=on` for lip-sync animation data
6. **Custom Dictionaries**: Upload via `/api/storage/` before referencing in `dico` parameter
7. **Rate Limits**: Not explicitly documented; implement exponential backoff

## Sample Code

### Python

```python
import requests

# Login
resp = requests.post("https://www.acapela-cloud.com/api/login/",
    data={"email": "user@example.com", "password": "secret"})
token = resp.json()["token"]

# Synthesize
headers = {"Authorization": f"Token {token}"}
params = {
    "voice": "alice",
    "text": "Hello, world!",
    "output": "stream",
    "type": "mp3"
}
resp = requests.get("https://www.acapela-cloud.com/api/command/",
    headers=headers, params=params, stream=True)

for chunk in resp.iter_content(chunk_size=4096):
    process_audio(chunk)
```

### cURL

```bash
# Login
TOKEN=$(curl -s -X POST https://www.acapela-cloud.com/api/login/ \
    -d "email=user@example.com&password=secret" | jq -r '.token')

# Synthesize
curl -H "Authorization: Token $TOKEN" \
    "https://www.acapela-cloud.com/api/command/?voice=alice&text=Hello&output=stream&type=mp3" \
    -o output.mp3
```

## Pricing

- Credit-based system
- Credits consumed per character synthesized
- Premium/AI voices may cost more credits
- Contact Acapela for enterprise pricing

## References

- [API Documentation](https://www.acapela-cloud.com/docs_api/)
- [Voice Repertoire](https://www.acapela-group.com/voices/repertoire/)
- [Available Languages](https://www.acapela-group.com/voices/available-languages/)
- [Acapela Cloud Product Page](https://www.acapela-group.com/solutions/acapela-cloud/)
