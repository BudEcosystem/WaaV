# Realtime / S2S Real-Vendor Validation Runbook

This is the production runbook for validating WaaV's realtime (speech-to-speech)
fleet against the **real vendor** APIs. It is the bridge across the credential
boundary: every provider passes a credential-free **mock** round-trip in CI, but a
mock cannot prove the wire matches the live vendor. This runbook + the companion
tool [`scripts/realtime_vendor_validation.py`](../scripts/realtime_vendor_validation.py)
let an operator who holds a vendor key validate **any** provider against the real
vendor in **one command**.

## Status

| validation tier | providers |
|---|---|
| **Real-vendor validated** | `openai`, `deepgram`, `elevenlabs` |
| **Awaiting a key** (mock-validated only) | `azure`, `grok`, `inworld`, `gemini`, `ultravox`, `nova_sonic`, `speechmatics`, `hume`, `yandex` |
| **All 12** | pass credential-free mock round-trips (CI) |

This document is how to **real-vendor-validate the 9 awaiting a key** and
**re-validate the 3** that already are. Every endpoint / auth / rate / required
field below is sourced from the provider's `src/core/realtime/<p>/protocol.rs`
(`connect_spec`, `caps()`, `from_config`) and the gateway request schema
(`src/handlers/realtime/messages.rs`) — it is **accurate, not guessed**. An
inaccurate runbook is worse than none, so any field the source itself flags as
unverified is carried into the **[FLAG]** column.

---

## The one-command run

```bash
<ENV VARS> python3 scripts/realtime_vendor_validation.py <provider>
```

The tool:

1. **Refuses to run** if the required env var(s) for that provider aren't set,
   printing exactly which to export (it never silently no-ops, and never sends a
   dummy key to the real vendor).
2. **Starts a gateway** (`./target/debug/waav-gateway -c config.yaml`) with those
   env vars passed through and **no `*_REALTIME_URL` override** — so the
   provider's `connect_spec` dials the **real vendor host**. (Or attach to an
   already-running gateway with `--gateway-port N`.)
3. Runs the full S2S round-trip — `config` → audio/text → collect transcript +
   agent audio + events — using the proven harness methodology (wait for
   `session_created`, pace audio at 40 ms, keep-feed silence, ~30 s budget).
4. Reports **PASS/FAIL** with agent-audio bytes, transcripts, events, and on a
   wire mismatch prints the gateway's `error` frame **verbatim**.

List every provider and its required env vars:

```bash
python3 scripts/realtime_vendor_validation.py --list
```

### Prerequisites

- Build the gateway: `cargo build --bin waav-gateway`.
- `pip install websockets` (the only Python dependency).
- A speech clip at `/tmp/question_pcm.raw` (24 kHz mono s16le) — already present
  on the dev host; the tool resamples it to each provider's rate (16 k / 44.1 k)
  and falls back to silence if absent.

---

## Per-provider table (the heart)

Every cell is sourced from `protocol.rs`. **Rate** is the audio sample rate the
tool feeds (the protocol's `caps().output_sample_rate`, or the documented input
rate where in/out differ). **Auth** is the scheme `connect_spec` sets.

| provider | env var(s) to set | real endpoint | auth scheme | rate (Hz) | model / voice example | extra setup |
|---|---|---|---|---|---|---|
| **openai** | `OPENAI_API_KEY` | `wss://api.openai.com/v1/realtime` | `Authorization: Bearer <key>` | 24000 | `gpt-realtime` / `marin` | — |
| **azure** | `AZURE_OPENAI_API_KEY`, `AZURE_OPENAI_ENDPOINT` | `wss://<resource>.openai.azure.com/openai/v1/realtime?deployment=<dep>` | `api-key: <key>` (header) | 24000 | `<deployment>` / `alloy` | `AZURE_OPENAI_ENDPOINT` = resource; `model` = the realtime **deployment** name (set `AZURE_DEPLOYMENT`) |
| **grok** | `GROK_API_KEY` | `wss://api.x.ai/v1/realtime` | `Authorization: Bearer <key>` | 24000 | `grok-realtime` / `alloy` | — (bootstraps with `conversation.created`) |
| **inworld** | `INWORLD_API_KEY`, **`INWORLD_REALTIME_URL`** | `wss://api.inworld.ai/api/v1/realtime/session?key=<session-id>&protocol=realtime` | `Authorization: Basic base64(<key>)` | 24000 | `inworld-realtime` / `alloy` | needs a backend-**minted session id**; the gateway has no server-config slot to inject it, so set `INWORLD_REALTIME_URL` to a pre-authed session ws (see below) |
| **deepgram** | `DEEPGRAM_API_KEY` | `wss://agent.deepgram.com/v1/agent/converse` | `Authorization: Token <key>` | 24000 | `gpt-4o-mini` (think) / `aura-2-thalia-en` (speak) | — (audio-first) |
| **elevenlabs** | `ELEVENLABS_API_KEY`, **`ELEVENLABS_AGENT_ID`** | `wss://api.elevenlabs.io/v1/convai/conversation?agent_id=<id>` | `xi-api-key: <key>` (header) | 16000 | `<agent_id>` (the agent IS the model) | a **pre-created ConvAI agent_id** (set `ELEVENLABS_AGENT_ID`) |
| **gemini** | `GEMINI_API_KEY` | `wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent?key=<key>` | `?key=<key>` (query, no header) | 16000 in / 24000 out | `gemini-2.0-flash-live-001` / `Puck` | — |
| **ultravox** | `ULTRAVOX_API_KEY` | `POST https://api.ultravox.ai/api/calls` → `wss` joinUrl | `X-API-Key: <key>` (on the REST create-call; the joinUrl is pre-authed) | 16000 in / 24000 out | `fixie-ai/ultravox` / `Mark` | none beyond the key — the **gateway does the REST create-call** (`RestThenWebSocket`) |
| **nova_sonic** | `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_REGION` | Amazon Bedrock `InvokeModelWithBidirectionalStream` (HTTP/2 event stream, **not** a WebSocket) | **AWS SigV4** (aws-config default chain) — no api-key | 16000 in / 24000 out | `amazon.nova-sonic-v1:0` / `matthew` | AWS creds with **Bedrock model access** to `amazon.nova-sonic-v1:0` in the region (request access in the Bedrock console first); `AWS_SESSION_TOKEN` honored |
| **speechmatics** | `SPEECHMATICS_API_KEY` | `wss://flow.api.speechmatics.com/v1/flow` | `Authorization: Bearer <key>` (a JWT / temp token) | 16000 | `flow-service-assistant-amelia` (template) / `amelia` | a Flow **portal template_id** (set `SPEECHMATICS_TEMPLATE_ID`); key is passed **as** the Bearer token — supply a JWT/temp token |
| **hume** | `HUME_API_KEY` | `wss://api.hume.ai/v0/evi/chat` | `?api_key=<key>` (query, no header) | 44100 | (no model id) / `kora` | — |
| **yandex** | `YANDEX_API_KEY`, `YANDEX_FOLDER_ID` | `wss://ai.api.cloud.yandex.net/v1/realtime?model=gpt://<folder>/<model>` | `Authorization: Bearer <key>` (IAM `t1.…`) **or** `Api-Key <key>` (static) | 24000 | `speech-realtime-250923` / `alloy` | the Yandex Cloud **folder id** (set `YANDEX_FOLDER_ID`); key may be an IAM token or a static API key |

> The `model` / `voice` / `instructions` and the rest of the `{type:config}`
> message are provider-agnostic (`RealtimeSessionConfig` in
> `src/handlers/realtime/messages.rs`). The handler resolves the per-provider key
> from server config (`src/handlers/realtime/handler.rs`) and injects the
> server-side resource for Azure (`azure_openai_endpoint`) and Yandex
> (`yandex_folder_id`) into the provider's `endpoint` slot.

---

## Per-provider one-command runs

```bash
# openai — Bearer; the canonical text-turn round-trip (real-vendor validated)
OPENAI_API_KEY=sk-... \
  python3 scripts/realtime_vendor_validation.py openai

# azure — api-key header; model = the realtime DEPLOYMENT name
AZURE_OPENAI_API_KEY=... AZURE_OPENAI_ENDPOINT=my-resource AZURE_DEPLOYMENT=my-rt-deploy \
  python3 scripts/realtime_vendor_validation.py azure

# grok / xAI — Bearer
GROK_API_KEY=xai-... \
  python3 scripts/realtime_vendor_validation.py grok

# inworld — needs a pre-authed minted-session ws (see "Inworld" below)
INWORLD_API_KEY=... \
INWORLD_REALTIME_URL='wss://api.inworld.ai/api/v1/realtime/session?key=<minted-session-id>&protocol=realtime' \
  python3 scripts/realtime_vendor_validation.py inworld

# deepgram — Token; audio-first (real-vendor validated)
DEEPGRAM_API_KEY=... \
  python3 scripts/realtime_vendor_validation.py deepgram

# elevenlabs — xi-api-key; model = a pre-created ConvAI agent_id (real-vendor validated)
ELEVENLABS_API_KEY=... ELEVENLABS_AGENT_ID=agent_abc123 \
  python3 scripts/realtime_vendor_validation.py elevenlabs

# gemini — ?key= query
GEMINI_API_KEY=... \
  python3 scripts/realtime_vendor_validation.py gemini

# ultravox — X-API-Key on the gateway's REST create-call
ULTRAVOX_API_KEY=... \
  python3 scripts/realtime_vendor_validation.py ultravox

# nova_sonic — AWS SigV4 (keyless); needs Bedrock model access
AWS_ACCESS_KEY_ID=... AWS_SECRET_ACCESS_KEY=... AWS_REGION=us-east-1 \
  python3 scripts/realtime_vendor_validation.py nova_sonic

# speechmatics — Bearer (JWT/temp token); model = a portal template_id
SPEECHMATICS_API_KEY=<jwt-or-temp-token> SPEECHMATICS_TEMPLATE_ID=flow-service-assistant-amelia \
  python3 scripts/realtime_vendor_validation.py speechmatics

# hume — ?api_key= query
HUME_API_KEY=... \
  python3 scripts/realtime_vendor_validation.py hume

# yandex — Bearer (IAM t1.…) or Api-Key (static); folder id required
YANDEX_API_KEY=... YANDEX_FOLDER_ID=b1g... \
  python3 scripts/realtime_vendor_validation.py yandex
```

Attach to an **already-running** gateway instead of spawning one (the operator's
gateway must already hold the real-vendor creds and have **no** `*_REALTIME_URL`
override). In attach mode the tool re-checks only the **client-side** resources it
puts in the config message (e.g. `ELEVENLABS_AGENT_ID`); the vendor key lives in
the running gateway:

```bash
python3 scripts/realtime_vendor_validation.py openai --gateway-port 3009
```

---

## Reading the output

### Expected PASS

```text
== REAL-VENDOR validation: openai via WaaV ws://127.0.0.1:3009/realtime ==
   endpoint : wss://api.openai.com/v1/realtime
   auth     : Authorization: Bearer <OPENAI_API_KEY>
   rate     : 24000 Hz   pattern: text
   config   : {"type": "config", "provider": "openai", "model": "gpt-realtime", ...}

  [  PASS  ] agent_audio=153600B  first_audio=2510ms  session_created=True
           event_counts={'session_created': 1, 'transcript': 10, 'response_done': 1}
           transcripts=[('assistant', 'Hello'), ..., ('assistant', 'Hello! One ocean is the Atlantic Ocean')]
```

A **PASS** means agent **audio** came back — the full S2S loop closed through the
gateway to the real vendor. `session_created=True` confirms the upstream session
handshake; assistant `transcript`s confirm the response mapping; the byte count is
the decoded agent audio the gateway delivered.

### Reading a wire-mismatch FAIL

On a real-vendor wire divergence the tool prints the gateway's `error` frame
**verbatim** — this is the actionable signal:

```text
  [  FAIL  ] agent_audio=0B  first_audio=—  session_created=False
           event_counts={'error': 1}

  GATEWAY ERROR FRAME(S) (verbatim — a real-vendor wire mismatch shows up here):
    error: <the upstream rejection, e.g. a 400 body / auth failure / bad model>

  [FLAGs to watch on this first real run]:
    - ...
```

- `session_created=False` + an `error` frame ⇒ the **connect or `session.update`
  was rejected** (auth scheme, endpoint path, model id, or a wire-shape mismatch).
  The verbatim text is the vendor's own rejection — match it against the **[FLAG]**
  notes below.
- `session_created=True` but `agent_audio=0` ⇒ the session opened but produced no
  spoken reply in the budget (raise `--budget`, or the turn/VAD config needs a
  tweak for that vendor).
- No audio **and** no error ⇒ a silent upstream stall; re-run and inspect the
  gateway log (path printed on failure, `/tmp/waav_vendor_validation_gateway_<p>.log`).

---

## Known [FLAG]'d per-provider unknowns to watch on the first real run

These are the fields the `protocol.rs` source itself flags as documented-but-not-
byte-verified (no key was held when written). Watch them on the **first** real run.

- **azure** — **GA endpoint path.** WaaV targets `…/openai/v1/realtime` (the GA
  message format, no `api-version` query). If your resource only serves the dated
  `?api-version=…-preview` endpoint, a GA-shaped `session.update` may **400**. Auth
  is the `api-key` **header** (not `Authorization: Bearer`). `model` must be the
  **deployment** name, not a base model id.
- **grok** — **not yet live-probed.** The `wss://api.x.ai/v1/realtime` host + the
  `conversation.created` bootstrap are the best-known/documented form. If the host
  or model name differs, the connect error prints verbatim. No WS subprotocol
  header is sent (xAI uses the subprotocol field only for the browser-token path).
- **inworld** — **session minting.** Inworld requires a backend-minted session id
  passed as `?key=`; WaaV does **not** mint it automatically (a pending REST-
  handshake follow-up) and has **no server-config slot to inject it**. The only
  turnkey real-vendor route is `INWORLD_REALTIME_URL` set to a pre-authed session
  ws (mint the session out-of-band via Inworld's session-create API, then pass the
  resulting `…?key=<session-id>&protocol=realtime` url). Auth is HTTP **Basic**
  (base64 of the key), not Bearer.
- **gemini** — **model id.** `gemini-2.0-flash-live-001` is the broadly-available
  stable Live model; some keys may require a preview native-audio model — override
  via the config `model`. Auth is the `?key=` **query** param (no header).
- **ultravox** — **output sample rate.** The create-call declares 24 kHz output;
  not yet byte-verified against a live stream. The gateway performs the REST
  `create call` (`X-API-Key`) and connects the returned `joinUrl`.
- **nova_sonic** — **Bedrock model access + region.** This is **keyless** (AWS
  SigV4 via the aws-config chain). It will fail at connect unless your AWS identity
  has **model access** to `amazon.nova-sonic-v1:0` in `AWS_REGION` (grant it in the
  Bedrock console). The bidi event-stream wire is unit-validated against the AWS
  docs, not a live capture.
- **speechmatics** — **token exchange + path + output rate.** WaaV passes the key
  **as** the `Authorization: Bearer` token and does **not** perform the
  management-platform token **exchange** — a static api-key may **401**; supply a
  JWT / pre-minted temp token. The connect path `/v1/flow`, the assumed 16 kHz
  output rate, and the transcript field names (`metadata.transcript`,
  `content`) are documented, not byte-verified. `model` = a portal `template_id`.
- **yandex** — **Bearer vs Api-Key.** The auth scheme is chosen by key shape: a
  `t1.`-prefixed value ⇒ `Authorization: Bearer` (the OpenAI-SDK / IAM-token path);
  anything else ⇒ `Authorization: Api-Key` (the static-key path). If the server
  rejects a static key on the Bearer path, mint an IAM token. The `gpt://<folder>/
  <model>` URI is built from `YANDEX_FOLDER_ID` and URL-encoded into `?model=`.
- **hume** — none flagged; EVI output is 44.1 kHz and auth is the `?api_key=`
  query param.

---

## What needs more than a key

Three providers need an extra resource — surfaced by the tool's refusal banner so
the operator isn't surprised:

| provider | beyond the key |
|---|---|
| **elevenlabs** | a **pre-created ConvAI agent_id** (`ELEVENLABS_AGENT_ID`) — the agent *is* the model; the gateway puts it in the config `model`. |
| **nova_sonic** | **AWS Bedrock model access** to `amazon.nova-sonic-v1:0` in the region (no api-key — AWS SigV4 creds with the model-access grant). |
| **inworld** | a **minted session id** + the `INWORLD_REALTIME_URL` override (no in-gateway session-mint path yet). |

Two more need a **server-side resource id** the gateway injects (still just the key
plus one value): **azure** (`AZURE_OPENAI_ENDPOINT` resource + a realtime
deployment in `model`) and **yandex** (`YANDEX_FOLDER_ID`). **speechmatics** needs
a portal **template_id** (`SPEECHMATICS_TEMPLATE_ID`) and a JWT/temp token rather
than a static key.

---

## Related

- [`scripts/realtime_vendor_validation.py`](../scripts/realtime_vendor_validation.py) — the tool
- [OpenAI Realtime](./openai-realtime.md)
- [Hume integration](./hume.md)
- [WebSocket API](./websocket.md)
- [Supported providers](./SUPPORTED_PROVIDERS.md)
