# Egress and operator credentials (standalone)

This page is for operators running WaaV without the Bud control plane. It covers where the
gateway sends the vendor keys you configure, and how you approve other destinations. Under Bud
mode, every leg uses its deployment's credential and address, and none of this applies.

## The rule

A client may always send its own key (`api_key` in `stt_config`, `tts_config` or
`conversation_config`) and point it anywhere the SSRF rules allow.

When a client sends no key, the gateway uses yours. The key is then sent only to:

- the vendor's own hosts;
- hosts you approved for it.

A client cannot redirect your key by naming a destination. This covers:

- `extras.endpoint_override`, `custom_endpoint`, `custom_callback_url`, `base_url` and `api_base`;
- a conversation `base_url`;
- a DAG node.

Such a session is refused with an `error` that names the field.

## Approving a destination

The gateway approves a host for one of your keys when any of these holds:

| How | Example |
|---|---|
| A named credential for the key, listing the host | `WAAV_CREDENTIALS_JSON={"DEEPGRAM_API_KEY":{"env":"DEEPGRAM_API_KEY","hosts":["dg-proxy.corp:443"]}}` |
| An exact private `host:port` | `WAAV_PRIVATE_EGRESS_ALLOWLIST=127.0.0.1:8080` |
| A loopback mock (development) | `WAAV_ALLOW_LOOPBACK_ENDPOINTS=8080` (ports) or `=1` (any port) |

Notes on these:

- **Credentials file.** `WAAV_CREDENTIALS_FILE` (YAML or JSON) works the same way as
  `WAAV_CREDENTIALS_JSON`.
- **Which name to use.** Name the credential after the variable the gateway reads the key from:
  `DEEPGRAM_API_KEY`, `ELEVENLABS_API_KEY`, `AZURE_SPEECH_SUBSCRIPTION_KEY`, `IBM_WATSON_API_KEY`,
  `GNANI_TOKEN`, and so on. Providers that read their own key from the environment use the same
  rule: `MURF_API_KEY`, `WELLSAID_API_KEY`, `TINKOFF_API_KEY`, `DASHSCOPE_API_KEY` and so on.
- **What counts as the client's own credential.** `api_key`, or in `extras`: `access_token`
  (Google, Cartesia), an `aws_access_key_id` / `aws_secret_access_key` pair, or Gnani's `token`
  and `access_key`. Any other leg is treated as carrying your key.
- **Vendor hosts without a declaration.** These keys reach their vendor's own hosts with no
  declaration: OpenAI, Anthropic, Google AI, Groq, Mistral, Together, Fireworks, Perplexity,
  Cohere and Sarvam, plus the speech vendors Deepgram, ElevenLabs, Cartesia, AssemblyAI, Hume
  and Azure Speech.
- **`OPENAI_BASE_URL`.** The exact `host:port` of `OPENAI_BASE_URL` also counts as an OpenAI host.
  The speech legs already send `OPENAI_API_KEY` there, so a conversation `base_url` pointing at
  the same server gets the key too.
- **Escape hatch scope.** `WAAV_ALLOW_LOOPBACK_ENDPOINTS` exempts loopback only. It never covers
  link-local, cloud-metadata or RFC 1918 addresses, and this holds for segmented
  speech-to-text uploads as well.

When a configured default key is not sent because the target is not one of its hosts, the
session gets this warning, and the gateway logs it:

```json
{"type": "config_warning", "code": "llm_key_not_sent", "detail": {"credential": "OPENAI_API_KEY", "host": "127.0.0.1:8080"}}
```

## Credentials clients may not name

A credential with `"clients": false` is kept for your own configuration: aliases
(`llm.credential_ref`) and server DAG templates. A client that names it, through `credential_ref`,
`api_key: "${NAME}"` or a client DAG's LLM node, is refused.

```json
{"local-gw": {"env": "LOCAL_GW_KEY", "hosts": ["127.0.0.1:8080"], "private": true, "clients": false}}
```

## Refusing client-chosen endpoints

`WAAV_CLIENT_ENDPOINTS=deny` (default `allow`) makes a standalone gateway accept only what you
configured, as Bud mode does. Clients name aliases and DAG templates. The following are then
refused:

- `conversation_config.base_url` and `reasoning_base_url`;
- any operator credential a client names;
- an inline `dag_config.definition`;
- every STT/TTS `extras` destination, whoever's key it carries.

The check runs before an alias is applied, so an alias's own `base_url` and `credential_ref` still
work. A value other than `allow` or `deny` refuses, logged as an error: a security switch never
falls back to "allow".

## DAG provider nodes

An STT, TTS or realtime node with no `config.api_key` uses the gateway's key for its provider,
just as a `/ws` leg does. `${DEEPGRAM_API_KEY}` (the provider's own variable) means the same thing
and resolves from the gateway's configuration, environment or YAML. A client DAG may not name any
other variable.
