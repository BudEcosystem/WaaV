# OpenAI Parity & Cross-Provider Cross-Check

Status of the OpenAI voice surface in WaaV after the June 2026 parity work, plus a
cross-check of the fixes against **other providers**, **Pipecat's integrations**,
and the **current provider APIs**. Compiled from a 3-agent audit (LLM-adapter
cross-provider, Pipecat/OpenAI-API gaps, other-provider deprecation) + live
validation against the real OpenAI API.

Legend: ✅ done + live-validated · 🟡 done (unit-tested, no live key) · ⏳ deferred (verified finding) · 📋 additive gap

---

## 1. Implemented + live-validated this work

| Area | What | Evidence |
|---|---|---|
| LLM reasoning shape | o-series/gpt-5 → `max_completion_tokens`, suppress sampling | ✅ gpt-5-mini round-trip + full cascade |
| Realtime Beta→GA | header, nested `audio.*` session, renamed audio **and text** events, `gpt-realtime` | ✅ S2S round-trip, marin/cedar, noise reduction |
| Realtime default model | handler `DEFAULT_MODEL` preview → `gpt-realtime` | ✅ unit + live |
| Streaming STT | `gpt-4o-transcribe` `stream=true` SSE → interim+final; `verbose_json`→`json` coerce | ✅ 10 partials + final |
| Per-response override | `create_response_with` → GA `response.create` (modalities/instructions/voice/cap/out-of-band) | ✅ text-only override → `ACKNOWLEDGED` |
| TTS voices/formats | marin/cedar; wav/pcm/mp3/opus | ✅ 6/6 |
| Chat params | `parallel_tool_calls`/`seed`/penalties/`logprobs`/`user` (cascade + DAG) | ✅ unit + live |

## 2. The fixes applied BEYOND OpenAI (cross-provider)

| Fix | Provider(s) | Status |
|---|---|---|
| Reasoning detector no longer false-positives `gpt-5*-chat-latest` (they accept sampling) | OpenAI | 🟡 |
| Reasoning detector catches `codex*`/`computer-use*` + future-proof `o5`/`gpt-6..9` | OpenAI | 🟡 |
| `parallel_tool_calls:false` → Anthropic's native `tool_choice.disable_parallel_tool_use` | Anthropic | 🟡 |
| `seed`/`presence_penalty`/`frequency_penalty` → Gemini `generationConfig` (camelCase) | Gemini | 🟡 |

## 3. Verified cross-provider issues — DEFERRED (need live provider keys to validate safely)

The single root cause for the LLM ones: `is_openai_reasoning_model` is an **OpenAI-id**
classifier doing duty as the **universal** OpenAI-wire request-shape gate. Other
OpenAI-compatible reasoning providers have *contradictory* requirements, so the honest
fix is a small **per-provider capability descriptor** (which of: requires-`max_completion_tokens`,
rejects-sampling, rejects-penalties, rejects-`stop`, `reasoning_effort`-shape), not a longer id list.

| # | Provider | Issue | Why deferred |
|---|---|---|---|
| 1 | xAI **grok-4** (via OpenAI wire) | rejects `stop`/penalties/`reasoning_effort` with hard errors; WaaV sends them | needs a capability table + an xAI key to validate |
| 2 | **DeepSeek-reasoner** | rejects `logprobs` (sampling is merely ignored) | same table; DeepSeek key |
| 3 | **Azure OpenAI** | reasoning *deployment names* don't match id prefixes → `max_tokens` 400; also needs `api-key` header + `?api-version` (LLM backend unwired today) | first-class Azure routing is its own feature |
| 4 | **Groq Qwen3-32B** | `reasoning_effort` only accepts `none`/`default`, not low/med/high | capability table |
| 5 | DashScope **QwQ/Qwen3-OSS** | streaming-only; a non-stream turn 400s | force-stream rule |
| 6 | **Cartesia** | `Cartesia-Version` header pinned `2025-04-16`; current ≈ `2025-11-04`/`2026-03-01` (TTS+STT) | changing a connection-gating version header without a live Cartesia test risks "fails to connect" |
| 7 | Deepgram / Google STT | empty-model defaults `nova-2` / `latest_long` are behind current `nova-3` / `chirp_2-3` | model-default policy + per-language/region check |

## 4. Pipecat & OpenAI-API feature gaps (📋 additive — not bugs)

WaaV already **exceeds** Pipecat on chat params (Pipecat exposes most only via an opaque
`extra` dict) and on TTS formats. True remaining gaps:

**Pipecat-parity (small):** chat **image input** (`image_url`), chat **audio input**
(`input_audio`, gpt-4o-audio-preview), `stream_options.include_usage` (streaming token
stats), TTS `instructions` passthrough *(verify — WaaV's `from_standard` already maps it
for gpt-4o-mini-tts)*, STT `include[]=logprobs`.

**OpenAI-API (no Pipecat pressure):** `reasoning_effort` value-set add `xhigh` (+`none`);
`service_tier` add `scale`; `user` → `safety_identifier`+`prompt_cache_key` (deprecation);
`verbosity`; `store`/`metadata`/`prediction`; chat audio *output*.

**Realtime GA additive:** `truncation` (`retention_ratio`), `tracing`, hosted `prompt`,
session/response `reasoning` (for a reasoning-capable realtime model), **image input**,
**transcription-only session** (`session.type:"transcription"`), **MCP tools**, STT
`timestamp_granularities`/diarization (`gpt-4o-transcribe-diarize`).

**Missing S2S providers (coverage):** Google **Gemini Live**, AWS **Nova Sonic** — both fit
the existing `BaseRealtime` abstraction.

## 5. Recommended next order

1. **Per-provider LLM capability table** — resolves grok/DeepSeek/Groq/Azure-reasoning 400s together (§3 #1-5).
2. **`reasoning_effort` value-set** (`xhigh`/`none`) + **`service_tier=scale`** — tiny correctness.
3. **Chat image/audio input + `stream_options.include_usage`** — the real Pipecat-parity gaps.
4. **Realtime additive fields** (`truncation`/`tracing`/`prompt`/`reasoning`) — small, on the schema already built.
5. **Cartesia version bump** + **Deepgram/Google default refresh** — with a live key each.
6. **Gemini Live / Nova Sonic** S2S providers — larger, new coverage.

## Non-issues (audited, current — no action)
Hume EVI (current endpoint + already rejects deprecated EVI v1/v2), ElevenLabs + Cartesia
model lists, OpenAI o1/o3/o4 detection (no false positives), DeepSeek *sampling* params
(ignored, not 400).
