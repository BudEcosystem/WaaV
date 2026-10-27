# Smallest.ai (Waves) TTS

WaaV's Smallest provider (`provider: smallest`, aliases `smallest-ai`, `smallest_ai`, `smallest.ai`,
`waves`) synthesizes speech with Smallest's Lightning v3.1 models over REST, one request per
utterance. Source: `src/core/tts/smallest/`.

Vendor docs: <https://docs.smallest.ai/models/text-to-speech/overview>.

## Models

Smallest serves two voice pools. The pool is the request body's `model`; each pool has its own
voices, so a voice id from one is not valid in the other.

| `model` (WaaV config) | Pool | Voice cloning |
|---|---|---|
| `lightning_v3.1` (default; also `lightning-v3.1`) | standard | yes |
| `lightning_v3.1_pro` (also `lightning-v3.1-pro`) | premium | no |

Smallest retired every earlier model. `lightning`, `lightning-large` and `lightning-v2` answer
`410 MODEL_DEPRECATED` ("Please migrate to lightning-v3.1"), and `thunder` is gone. WaaV still
accepts those names and serves them with `lightning_v3.1`, logging a warning, so an existing config
keeps working. Voices are not carried across: `emily`, the old default, is in neither v3.1
catalog. The default voice is now `olivia` (standard pool).

## Endpoints

| Use | Endpoint |
|---|---|
| Synthesis (what WaaV calls) | `POST https://api.smallest.ai/waves/v1/tts` |
| Streaming (WebSocket / SSE) | `wss://api.smallest.ai/waves/v1/tts/live`, `POST …/tts/live` |
| Voices (public, no key) | `GET https://api.smallest.ai/waves/v1/lightning-v3.1/get_voices`, `…/lightning-v3.1-pro/get_voices` |

Authentication: `Authorization: Bearer <api_key>`. The key comes from the session's `api_key`
(there is no server-side Smallest key).

## Request

```json
{
  "text": "Hello from WaaV.",
  "voice_id": "olivia",
  "model": "lightning_v3.1",
  "sample_rate": 24000,
  "speed": 1.0,
  "language": "en",
  "output_format": "wav"
}
```

| Field | WaaV source | Values |
|---|---|---|
| `voice_id` | `voice_id` | a voice of the chosen pool |
| `model` | `model` | see above |
| `sample_rate` | `sample_rate` | 8000, 16000, 24000, 44100; anything else snaps to the nearest |
| `speed` | `features.speed` | 0.5 to 2.0 |
| `language` | `features.language` / `language` | `en`, `hi`, `mr`, `kn`, `ta`, `bn`, `gu`, `es`, … (each voice supports a subset) |
| `output_format` | `audio_format` | `pcm`, `wav` (default), `mp3`, `ulaw` (`mulaw` is accepted) |

`stability` and `similarity_boost` are not mapped: they were the retired `lightning-large`'s knobs
and Lightning v3.1 has no parameter for them, so `GET /capabilities/features` does not list them for
Smallest.

## Errors

| Status | Meaning |
|---|---|
| 400 | invalid parameters (e.g. a voice from the other pool) |
| 401 | missing or invalid key |
| 410 | a retired model's endpoint (WaaV no longer calls these) |
| 429 | rate limited |
