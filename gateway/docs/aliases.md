# Server aliases

An alias is a name you define in `config.yaml` for a whole voice setup: speech-to-text,
text-to-speech, the LLM, or a DAG template. A client names it and gets that setup. When you want
to swap providers, change a model or point at another server, you edit the alias; clients don't
change.

A client sends only the name, as `{"type": "config", "alias": "local-voice"}` on `/ws` or as `model`
on the OpenAI-compatible routes (see below). It can never send an alias definition. Fields the
client does send take precedence over the alias's defaults.

## Example

```yaml
aliases:
  local-voice:
    kind: agent
    openai_compatible: true          # reachable as `model` on /v1/audio/* (default false)
    stt:
      provider: openai
      model: local-transcribe
      language: en
      base_url: "http://127.0.0.1:8080"   # this leg's own server (optional)
      credential_ref: local-gw            # operator credential sent to it (optional)
    tts:
      provider: openai
      model: local-speak
      voice: alloy
      base_url: "http://127.0.0.1:8080"
      credential_ref: local-gw
      request_timeout_secs: 150           # a model loading on demand can take longer than 60 s
    llm:
      base_url: "http://127.0.0.1:8080/v1"
      model: local-chat
      credential_ref: local-gw
```

## Fields

| Field | Meaning |
|---|---|
| `kind` | `agent`, `realtime`, `pipeline`, … Advisory; it is echoed in the `ready` message. |
| `stt.provider`, `stt.model`, `stt.language` | Speech-to-text defaults. |
| `stt.base_url`, `stt.credential_ref` | The leg's own server and the credential sent to it. |
| `tts.provider`, `tts.voice`, `tts.model`, `tts.emotion` | Text-to-speech defaults. |
| `tts.base_url`, `tts.credential_ref` | The leg's own server and the credential sent to it. |
| `tts.request_timeout_secs` | Synthesis timeout when the client names none. The default is 60 s; values are clamped to 1–300 s. |
| `llm.base_url`, `llm.model`, `llm.system_prompt`, `llm.reasoning_effort`, `llm.reasoning_model`, `llm.provider_kind`, `llm.credential_ref` | LLM defaults. |
| `dag_template` | Route the session through a server DAG template instead. |
| `realtime.provider`, `realtime.model` | A vendor's own realtime API, relayed by standalone `/v1/realtime` (see [below](#relaying-to-a-vendors-realtime-api)). Both required. |
| `realtime.base_url`, `realtime.credential_ref` | The vendor's address instead of its own host, and the credential sent to it. |
| `realtime.voice`, `realtime.instructions` | The session's defaults; the client may change them. |
| `openai_compatible` | Lets `/v1/audio/speech`, `/v1/audio/transcriptions` and `/v1/realtime` resolve the alias as `model`. |

## A leg's own server

Without `base_url`, a leg goes where its provider always goes; for `openai`, that is
`OPENAI_BASE_URL`. With `base_url`, the leg goes to that server instead. This lets one gateway
mix, for example, a local speech model with OpenAI's cloud voices.

How a `base_url` behaves:

- **Server-side only.** It is never echoed to the client and can't be set by one.
- **Validated at startup** with the egress rules. A private address needs
  `WAAV_PRIVATE_EGRESS_ALLOWLIST=host:port`, and a mistake stops the server rather than failing
  every session.
- **Only for its own provider.** It applies only while the leg keeps the alias's provider. A
  client that picks another provider doesn't get its traffic sent to the alias's server.
- **OpenAI-compatible servers.** An `openai` text-to-speech leg runs through the
  OpenAI-compatible (`self_hosted`) provider at the base's `/v1`. The base may be written with or
  without `/v1`. That provider does not send OpenAI's `instructions` field.

The key a leg with a `base_url` sends is decided in this order:

1. the client's own `api_key`, if it sent one;
2. else the alias's `credential_ref`, which must be declared for that host (a credential bound
   elsewhere is a session error, never a silent fallback);
3. else the gateway's key for the provider, but only if you approved that host (see
   [Egress and operator credentials](egress-and-credentials.md));
4. else no key. If the gateway does hold a key for that provider, the session also gets a
   warning: `config_warning {code: "tts_key_not_sent"}` (or `stt_key_not_sent`).

## The OpenAI-compatible routes (standalone)

Without the Bud control plane, `/v1/audio/speech`, `/v1/audio/transcriptions`,
`/v1/audio/translations` and `/v1/realtime` resolve `model` through aliases marked
`openai_compatible: true`:

- speech uses the alias's `tts` leg;
- transcription and translation use its `stt` leg;
- realtime relays to the vendor its `realtime` block names, or else runs all three legs, `stt`,
  `llm` and `tts` (below).

On these routes a leg's `base_url` must be on an `openai` leg, since it is served as an
OpenAI-compatible server; the server refuses to start otherwise. An alias that isn't marked
answers `404 model_not_found`.

With an external auth responder, a multipart upload is authenticated by its method, path and
headers, with `request_body: {}`, and reaches the handler unread.

```sh
curl -X POST "$WAAV/v1/audio/speech" -H "Authorization: Bearer $KEY" \
     -H 'Content-Type: application/json' -d '{"model":"local-voice","input":"hello","voice":"alloy"}'
curl -X POST "$WAAV/v1/audio/transcriptions" -H "Authorization: Bearer $KEY" \
     -F model=local-voice -F file=@hello.wav
```

### `/v1/realtime` (standalone)

`wss://…/v1/realtime?model=<alias>` speaks the OpenAI Realtime GA protocol over an alias marked
`openai_compatible: true` that has `stt`, `llm` and `tts` legs. The conversation loop runs the
session, as it would a `/ws` client that sent `{"type":"config","alias":"<alias>"}`: speech-to-text,
the LLM and text-to-speech, with the reply's words as `response.output_audio_transcript.delta`.

- **Authentication** uses the gateway's own authenticators (API secrets, then the auth responder).
  The key comes as `Authorization: Bearer`, an `api-key` header, or, from a browser, the
  `openai-insecure-api-key.<key>` subprotocol offered with `realtime`. `?token=` is refused on this
  route. With authentication off, a session without a key is accepted.
- **Audio** is PCM16 at 24 kHz both ways, as in the GA protocol.
- **Turns are the server's.** The session answers when the caller stops speaking (`server_vad`), and
  the caller can interrupt the reply. A typed message (`conversation.item.create`, then
  `response.create`) is a turn too, and `response.cancel` stops the reply.
- **What the caller may set** in `session.update` before the session starts: the voice, the speed
  and the transcription language. Instructions and tools are the server's: the alias's
  `llm.system_prompt`, and no client tools. Text-only output and manual turn detection are refused.
- **Refusals:** `404 model_not_found` for an alias that isn't marked, and
  `400 alias_not_realtime` for one with neither a `realtime` block nor all three legs.

```python
# OpenAI's Python SDK (GA)
async with client.realtime.connect(model="local-voice") as conn:
    await conn.session.update(session={"type": "realtime"})
    ...
```

### Relaying to a vendor's realtime API

An alias with a `realtime` block sends `/v1/realtime?model=<alias>` to that vendor's own realtime
API, exactly as a Bud `realtime_session` deployment is served. Vendors that speak the OpenAI
Realtime GA protocol (`openai`, `azure_openai`, `grok`) are relayed frame for frame. `gemini`,
`deepgram_voice_agent`, `elevenlabs_convai` and `hume_evi` are translated to GA by the gateway.
`nova_sonic` isn't available here: it signs with an AWS key pair in a region, which one alias
credential can't carry.

```yaml
aliases:
  voice-gpt:
    openai_compatible: true            # required: only /v1/realtime reads `realtime`
    realtime:
      provider: openai
      model: gpt-realtime              # the vendor model; the client's `?model=` is the alias
      voice: marin                     # session defaults (optional)
      instructions: "You are a concise support agent."
  voice-azure:
    openai_compatible: true
    realtime:
      provider: azure_openai
      model: my-realtime-deployment    # the Azure deployment name
      base_url: "https://my-resource.openai.azure.com"
      credential_ref: azure-realtime   # declared in WAAV_CREDENTIALS_FILE for *.openai.azure.com
```

- **The vendor and its key are the server's.** The client names the alias and nothing else. The
  vendor model, address and credential never come from the client, and the client sees the alias
  as the session's `model`.
- **Which key is sent:**
  1. Without `base_url`, the session goes to the vendor's own host with the gateway's key for that
     vendor (`OPENAI_API_KEY`, `AZURE_OPENAI_API_KEY`, `GROK_API_KEY`, `GEMINI_API_KEY`, or the
     Deepgram, ElevenLabs or Hume key). `credential_ref` needs a `base_url`.
  2. With `base_url` and `credential_ref`, that credential is sent. It must be declared for the
     base's host.
  3. With `base_url` alone, the gateway's key is sent only if you approved that host for it (see
     [Egress and operator credentials](egress-and-credentials.md)). For Azure, xAI and Gemini, the
     egress rules have no variable to declare, so use a `credential_ref` (or
     `WAAV_PRIVATE_EGRESS_ALLOWLIST`).

  A session that would end up with no key is refused with `502 deployment_misconfigured`. The
  gateway's log says why; the client isn't told hosts or credential names.
- **Checked at startup** (and by `waav-gateway check`): the provider, the model, the `base_url`
  under the egress rules, the credential's hosts, and the vendor request itself, built the same
  way each session builds it. A mistake stops the server.
- **Everything around the session is the relay's**, as on a Bud deployment: the client-event
  policy (the client may change the voice and instructions; MCP tools and stored prompts are
  refused), idle and maximum-length limits, pings, the defaults `session.update` and the session's
  metering span. The Bud deployment rate limits, concurrency caps and circuit breakers don't
  apply; the gateway's connection limits do. Client secrets (`ek_bud_`) need the Bud control plane,
  and so does [interaction mode](interaction_mode.md) on the relay: `interaction.realtime_default`
  does not apply to these sessions (the log says `bud_mode_required`), nor does the alias's own
  `interaction` block.
- **Revalidation** re-checks, on every `response.create` and periodically, that the alias still
  relays. API secrets are fixed for the life of the process, so the key isn't re-checked.
- **With legs as well**, the `realtime` block wins on `/v1/realtime`. The `stt` and `tts` legs keep
  serving `/v1/audio/*`, and `/ws` and `/realtime` ignore the block.
