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
| `openai_compatible` | Lets `/v1/audio/speech` and `/v1/audio/transcriptions` resolve the alias as `model`. |

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

Without the Bud control plane, `/v1/audio/speech`, `/v1/audio/transcriptions` and
`/v1/audio/translations` resolve `model` through aliases marked `openai_compatible: true`:

- speech uses the alias's `tts` leg;
- transcription and translation use its `stt` leg.

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
