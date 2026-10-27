# WaaV Gateway — Brutal Production-Readiness Review

**Scope reviewed:** entire `WaaV/` tree — Rust gateway (~268k LoC, 501 files), 60+ STT/TTS provider integrations, VAD/turn-detection, DAG engine, OpenAI/Hume realtime, LiveKit/SIP, core infra (cache/auth/rate-limit/HTTP/noise-filter), and the Python/TypeScript/Widget/Dashboard SDKs + plugin system.

**Method:** static review by 9 focused sub-reviews, cross-referenced against the actual gateway wire protocol, plus independent web verification of the highest-impact external-API claims. Context Hub (`chub`) was tried first but its registry carries no voice-provider docs, so provider APIs were verified by web research per the brief.

**One-line verdict:** This is a *real, ambitious, largely-implemented* codebase — not vaporware — but it is **NOT production-ready** and the `v1.0.0` / `build-passing` framing is misleading. The dominant problem is architectural: a flat 8/12-field config that strands almost every advanced provider feature, several integrations that cannot authenticate or connect at all, a default build that silently ships VAD/turn stubs, a broken parallel-DAG path, exploitable rate-limiting/SSRF, and a TypeScript SDK that does not interoperate with its own gateway.

---

## 0. Verification notes (what I independently checked)

| Claim | Result |
|---|---|
| TS SDK sends `punctuate` / reads `wire.text`; gateway requires `punctuation` / sends `transcript` (no serde defaults) | **Confirmed in-repo** — TS streaming cannot connect; transcripts always empty |
| Silero VAD v5 uses a unified `state`/`stateN` tensor (no `h`/`c`) | **Confirmed** — code uses v4-style `h`/`c`/`sr` against the v5 model URL |
| Azure Speech WS needs 2-byte+CRLF header framing + `speech.config` | **Confirmed** — code sends bare binary, no config → Azure STT broken |
| OpenAI `diarized_json` "fabricated" | **Corrected** — it's REAL but only on `gpt-4o-transcribe-diarize`; the code gates it to `gpt-4o-transcribe`/`mini` which reject it, and the diarize model isn't wired |
| Deepgram nova-3 needs `keyterm` not `keywords` | **Confirmed** — code's `keywords=` is ineffective on the default nova-3 |
| Cartesia `speed` mapping wrong | **Partially confirmed** — speed schema is version-dependent (−1..1 for `__experimental_controls`, 0.6–2.0 for sonic-3 `generation_config`); raw `speaking_rate` passthrough is fragile/likely-wrong, but the specific "1.5 always rejected" is model-dependent |
| CereProc TTS "fabricated against non-existent REST API" | **FALSE — agent was wrong.** CereProc ships a real REST v2 API at `api.cerevoice.com/v2/` with Bearer auth. The integration is plausibly correct; downgraded to "verify exact endpoints." |

Take the few provider-protocol claims I could not independently verify (Tinkoff base64-secret/JWT claims, Gnani/Phonexia/Sarvam wire shapes) as **flagged for verification**, not proven.

---

## 1. Systemic / architectural issues (most important)

These cut across the whole product and dwarf any single-provider bug.

### S1 — Flat config strands ~every advanced provider feature *(CRITICAL, design)*
`STTConfig` has 8 fields, `TTSConfig` ~12. They are the *only* structs that cross the dispatch/factory boundary (`plugin::registry::create_stt/create_tts`). Every provider's rich config (`*VoiceSettings`, `CartesiaTTSConfig`, `HumeTTSConfig`, `DeepgramConfig`, …) is built via `from_base(config)` and every per-provider builder (`set_description`, diarization flags, keyterms, stability/style, emotion) is **never invoked on the live path**. Net effect: across 60+ providers you can realistically only do *basic transcript-in / audio-out with language+model+rate*. Diarization, redaction, entity detection, keyterms, voice-settings, emotion, instructions, SSML styles — all defined, all unreachable. The rich `STTResult` metadata types (words/speakers/entities/PII) are mostly dead because nothing requests them and several providers drop the data on parse.

### S2 — Default build ships VAD/turn-detection STUBS *(CRITICAL)*
`Cargo.toml` `default = []`. `silero-vad`, `smart-turn`, `turn-detect`, `turn-ensemble` are all off by default. The stub modules return hardcoded values (`predict → Ok(0.0)`, `is_turn_complete → Ok(false)`, `SileroVAD::new → bail!`). A plain `cargo build --release` produces a "neural VAD / turn detection" gateway that contains **none**. This is a footgun for anyone who doesn't pass the exact feature flags.

### S3 — TTS codec mismatch corrupts audio *(CRITICAL)*
`TTSConfig.audio_format` defaults to `"linear16"`. Several providers (`wellsaid`, `speechify`, `unrealspeech`) hardcode MP3/WAV output but never reconcile `base_config.audio_format`, so the generic chunker runs MP3/WAV bytes through the PCM aggregation path and emits frames labelled `linear16`. Result for a default caller: **noise / WAV-header clicks**. The engine has no codec-reconciliation step.

### S4 — No automatic reconnection anywhere *(HIGH)*
Not one streaming STT/TTS provider, nor Hume EVI, reconnects. On any WS/gRPC error or idle timeout the event loop `break`s and the background task dies; the session is dead until the caller notices. (OpenAI Realtime is the lone exception with real backoff.) For a long-lived voice gateway this is a major availability gap.

### S5 — `emotion_config` is advertised but dead *(HIGH)*
`TTSConfig.emotion_config` is documented as applied by Hume/ElevenLabs/Azure. The only request path that actually reads it is **Cartesia**. For everyone else, callers set emotion and silently get neutral speech. Hume's entire differentiator (the `description` acting-instructions field) is hard-set to `None` on the factory path.

### S6 — "Lock-free, pre-compiled" DAG is partly fictional, and parallel exec is broken *(CRITICAL)*
- **Split branches execute twice**: after a `split`, branch nodes are never removed from `reachable_nodes`, so the outer topo loop re-runs every branch → **duplicate external API calls, duplicate billing, webhooks/LiveKit audio fired twice**. `join` ignores its declared `sources` entirely. No template exercises split/join, so it's effectively untested.
- **"Lock-free data passing" is false**: data flows through `HashMap<NodeIndex, DAGData>` + `.clone()` per edge; the `rtrb` ring buffer is wrapped in a `Mutex` (defeating it) **and is dead code** (never referenced outside its own tests). "Pre-compiled" is half-true — Router/Transform/Join rebuild a fresh Rhai `Engine` per execution, and STT/TTS nodes reconnect to providers per call.

### S7 — Rate limiting is bypassable and self-disabling *(CRITICAL, security)*
`SmartIpKeyExtractor` trusts client-supplied `X-Forwarded-For`/`X-Real-IP`, so an attacker rotates the header per request and gets a fresh token bucket every time. Worse, the limiter is silently disabled when `rate_limit_rps >= 100000` — the "112k RPS" benchmark almost certainly ran with **no rate limiting at all**. Connection limits key on the real peer IP, so the two limits disagree about identity and both collapse behind a proxy.

### S8 — `panic = "abort"` + thousands of `unwrap()/expect()` *(HIGH)*
Release profile sets `panic = "abort"`, so any reachable panic crashes the **entire process**, not one connection (no per-task isolation). Most unwraps are in tests, but several live in hot paths (cache `SystemTime` unwraps, JWT `AuthClaims::new`, IBM/Alibaba serialization, audio reinterpretation). One panic on malformed network data = whole-gateway outage.

### S9 — No `Cargo.lock` committed *(MEDIUM)*
`Cargo.lock` is in `.gitignore`. For a binary/application this means non-reproducible builds — every `cargo build` can pull different transitive versions of `livekit`, `ort`, `tract`, AWS SDKs, etc. A `v1.0.0` "build-passing" binary should pin its lockfile.

### S10 — Downloaded ML models have fake/no integrity verification *(HIGH, security)*
`turn_detect/assets.rs` `get_expected_hash` literally returns `"expected_hash_here"`; `verify_hash` computes SHA-256, never matches, only `warn!`s — never fails. Silero and Smart-Turn download with no hash check at all. Three ONNX models are fetched over the network and executed with zero integrity verification (supply-chain exposure).

---

## 2. Component findings

### 2.1 STT providers

**Tier-1**
- **Deepgram — PARTIAL.** Core streaming/auth/KeepAlive/EU-endpoint correct, but via the factory it's transcript-only: `diarize`/`keyterms`/`redact`/`filler_words` hardcoded off (`deepgram.rs:699-713`); `words`/`speaker` dropped on parse (`:308-329`); `vad_events=true` requested but `UtteranceEnd`/`SpeechStarted` ignored (`:346-372`); dead `utterance_end_ms`; legacy `keywords=` on nova-3 (should be `keyterm=`, **verified**).
- **Google — REAL** (best of set). Genuine v2 gRPC `StreamingRecognize`, OAuth channel, ExplicitDecodingConfig. Default model `latest_long` is a v1 name invalid for v2 (`google/config.rs:33`); diarization/word-offsets not wired.
- **Azure — BROKEN** (**verified**). No `speech.config` message; audio sent as unframed `Message::Binary` (`azure/client.rs:350-442`). Will not transcribe against real Azure. The correct inbound parser makes it deceptively look real.
- **OpenAI — REAL but batch-only.** Buffers and POSTs WAV (20 MB cap); the realtime transcription WS is unused. `response_format=diarized_json` is gated to `gpt-4o-transcribe`/`mini` which reject it (the real diarize model `gpt-4o-transcribe-diarize` isn't wired) — **corrected from "fabricated."**
- **AssemblyAI — PARTIAL.** Correct v3 `/v3/ws` and `Begin`/`Turn`/`Termination` flow, but suspect `speech_model` param values (`universal-streaming-english` vs v3's `universal_streaming`); missing keyterms; no reconnect.
- **Cartesia — BROKEN via factory.** `cartesia_version` is `None` through `from_base`, the only path the gateway uses → required version query param omitted → connection rejected (`cartesia/config.rs:97`, `client.rs:482`). `finalize()` is a stub.
- **ElevenLabs — REAL** (strongest WS). Correct realtime endpoint, `xi-api-key`, base64-in-JSON audio, residency hosts. Hardcoded `1.0` confidence on one path; no reconnect.
- **AWS Transcribe — REAL** (official SDK), but env-only creds and diarization/redaction/vocab hardcoded off via factory.
- **IBM Watson — REAL.** Correct IAM token exchange/caching, regional hosts, `action:start`. Minor serialize-unwrap.
- **Groq — REAL but batch.** Only provider with real retry/backoff + rate-limit handling.

**Regional** (22 reviewed — most are genuinely implemented, which is unusual):
- **Tencent — BROKEN.** HMAC string-to-sign omits the `host+path` prefix (`signature.rs:401-461`) → every connection fails server auth.
- **Tinkoff — BROKEN.** Sends raw `x-api-key`/`x-secret-key` gRPC metadata; real VoiceKit needs a HS256 JWT `Bearer` (`grpc.rs:57-73`). *(flagged, not web-verified)*
- **Phonexia — STUB.** Invented `/ws` + `X-SessionID` protocol; `mod.rs` admits the real gRPC API is "not implemented"; default URL is a placeholder. *(flagged)*
- **Gnani — PARTIAL**, **Sarvam — PARTIAL** (audio likely needs nested `{audio:{data,encoding,sample_rate}}` not a bare base64 string). *(flagged)*
- **REAL:** iflytek, alibaba_cloud, baidu, huawei_cloud, amivoice, speechmatics, gladia, revai, reverie (real streaming + correct auth). **REAL but batch/file-upload only** (no interim results → high latency for live use): bhashini, yandex, sberdevices, naver_clova, viettel_ai, fpt_ai, nectec.
- **Cross-cutting:** transcripts silently dropped under backpressure (`let _ = try_send(...)` on bounded-256 channels) across the set.

### 2.2 TTS providers

- **Deepgram — REAL**, but Aura-2 family (40+ voices) missing from the advertised list; `ulaw` isn't a valid encoding value; HTTP-only (WS TTS unused).
- **ElevenLabs — PARTIAL.** No WS streaming (`/stream-input` absent) → poor TTFB for agents; stability/similarity/style hardcoded & unreachable; default model `eleven_v3` is the wrong (non-realtime) default.
- **Google — REAL** (batch `text:synthesize`, no streaming → high TTFB).
- **Azure — PARTIAL.** SSML only supports `<prosody rate>`; no `mstts:express-as` style/emotion/pitch; `emotion_config` ignored.
- **OpenAI — REAL.** Only gap: `gpt-4o-mini-tts` `instructions` (its whole point) not exposed.
- **Cartesia — PARTIAL/BROKEN.** Raw `speaking_rate` → `generation_config.speed` is version-fragile (**verified** the schema differs by model); language hardcoded `"en"` with a TODO. (Only provider that honors emotion.)
- **AWS Polly — REAL** (SDK; voices enum-gated → new voices need code).
- **IBM Watson — PARTIAL.** `instance_id` only from env → `/instances//v1/synthesize` 404 through the factory.
- **Hume — PARTIAL/STUB-on-path.** `description` + `emotion_config` never wired into the factory path → all requests neutral (its differentiator is dead).
- **LMNT / PlayHT / Murf / Resemble / Smallest — REAL** (PlayHT has the best body coverage: PlayDialog/Play3.0-mini).
- **Speechify — BROKEN(default).** WAV output + `linear16` base default → header pop + mischunked audio.
- **UnrealSpeech / WellSaid — BROKEN(default).** MP3 output played as PCM = noise (see S3).
- **Regional:** **CereProc — verify (NOT fabricated; REST v2 API is real)** — *(corrected)*. **Tinkoff — BROKEN** (hardcoded JWT claims `iss:test_issuer`/`sub:test_user`; secret not base64-decoded; suspect protobuf field numbers) *(flagged)*. **Gnani — PARTIAL** (ASR-subdomain endpoint + Google-TTS-shaped body suggests guessed API) *(flagged)*. **Zalo — REAL** but missing async-URL download retry. **Speechmatics TTS — REAL** (preview TTS exists). The rest (acapela, yandex, sber, bhashini, reverie, iflytek, alibaba, baidu, tencent, huawei, naver, fpt, viettel, prosa, nectec) — **REAL** with correct auth/signing.

### 2.3 VAD & turn detection — REAL but BUGGY (or stub by default)
- **Silero VAD — likely BROKEN at load (verified mismatch).** Feeds v4 inputs (`input`/`sr`/`h`/`c`, hidden 64) to the **v5** model URL, which expects a unified `state` tensor → `session.run()` fails or silently no-ops state. Also: panics (`assert_eq!`) on wrong chunk size; time-based LSTM reset every 5 s mid-utterance (should reset on silence); per-frame allocations.
- **Smart Turn — REAL but BUGGY.** ONNX inference runs synchronously on the tokio worker (no `spawn_blocking`) so the "<50ms timeout" can't cancel CPU work; streaming mel buffer `clear()`s + recomputes + truncates each call → model fed near-empty, possibly transposed (`[1,80,800]` vs Whisper's `[1,800,80]`) input; STFT not centered (Whisper uses reflect-pad center); `add_text_signal` discards the decision → the "audio+text ensemble" never actually ensembles.
- **Turn Detect (LiveKit SmolLM) — REAL but BUGGY.** Hand-built chat template (`<|im_start|>user\n…` only, no system/history) is out-of-distribution; configured `max_context_turns`/per-language thresholds never used; silent `Ok(0.3)` fallback on every unexpected output masks a broken model; fake hash verification (S10).
- **Turn ensemble (turn_decision) — mostly CORRECT** FSM, but the text path isn't combined with audio in the same `process()` call, and it double-counts min-speech/min-silence timers against VAD.

### 2.4 DAG engine
Real & functional: linear STT→LLM→TTS→output, conditional edges, single router, and the full endpoint breadth (HTTP/gRPC/WS/IPC/LiveKit are genuinely implemented, **not** http-only). Broken/stub: split/join parallelism (S6), text-processor plugins ("not yet implemented, passing through"), "aggregate". Security: **WebhookOutputNode has no SSRF validation** (HTTP/WS endpoint nodes do) — a tenant can hit `169.254.169.254`; Rhai conditions/transforms run **synchronously on the async thread with no wall-clock timeout** on client-supplied scripts (op-limit only) → DoS. API-key A/B routing uses unanchored `starts_with` over a HashMap → non-deterministic. DAG defs are accepted inline from any authenticated WS client, so these are exploitable, not theoretical.

### 2.5 Realtime / LiveKit / SIP — genuinely the most mature subsystem
- **OpenAI Realtime — REAL.** Correct WSS URL + `Bearer` + `OpenAI-Beta: realtime=v1`, full event protocol, server-VAD, function calling, real reconnection. Gaps: no barge-in `conversation.item.truncate`; requesting GA `gpt-realtime` silently falls back to `gpt-4o-realtime-preview`.
- **Hume EVI — PARTIAL.** Correct endpoint/auth/prosody parsing, but "automatic reconnection" is **dead code** (never retries, callback never fires); `config_id` hard-set to `None` on the handler path; post-connect callback registration silently no-ops.
- **LiveKit rooms/token — REAL but insecure.** No `.with_ttl()` anywhere (relies on ~6 h default); regular-user tokens granted `room_record`/`room_create`/`room_list` — a leaked end-user token can record/enumerate/create rooms. `create_room` hardcodes `max_participants:3`; SIP dispatch `max_participants` ignored (TODO).
- **SIP — REAL** (trunk/dispatch, signature-verified webhooks, SSRF-checked hook CRUD).
- **S3 recording — REAL** (LiveKit egress → S3, lifecycle wired to teardown).
- Latent: `&[u8]`→`&[i16]` reinterpret in `livekit/client/audio.rs` is host-endian-dependent and technically UB (fine on x86/ARM-LE; use `from_le_bytes`/`bytemuck`).

### 2.6 Core infra
- **Cache — REAL & sound.** XXH3-128 keying is complete (provider+voice+model+format+rate) and correct; `Bytes` zero-copy is real. **But** TTL is hand-rolled (lazy eviction), not moka-native, so expired entries linger until touched — the "38 MB RSS" claim is incompatible with the 500 MB / 5M-entry cache under load.
- **HTTP "pool" — misleading.** It's one shared `reqwest::Client` behind a **4-permit semaphore** (a concurrency cap, not a pool); the 5th concurrent REST call queues. HTTP/2 is not guaranteed (no `http2_prior_knowledge`; ALPN-dependent) so the `http2_*` tuning may be dead.
- **Noise filter — well-engineered but oversold & off by default.** Threading is correct (dedicated OS threads, off the async runtime). But "SNR-adaptive" is amplitude gating (no noise-floor estimate), "echo suppression" is a DFN post-filter not AEC, and the worker pool shrinks permanently / can panic the caller if workers die.
- **Auth — gateway only *signs* JWTs and delegates validation externally** (so alg-confusion is N/A to the gateway); API-secret mode uses constant-time compare. **But** the WS audio path has no explicit auth gate (saved today only because `voice_manager` requires a Config message first); JWT-only deployments can't satisfy the WS first-message auth and become DoS-able.
- **Lifecycle leaks:** reconfiguring `voice_manager` overwrites the old one without `stop()` → leaked provider connections/tasks; one detached `tokio::spawn` **per inbound LiveKit audio frame** (no backpressure/cancellation/ordering).
- **SIMD (pulp) — real** with runtime detection; minor sub-LSB scalar-tail vs SIMD-body scale inconsistency (32767 vs 32768).

### 2.7 SDKs & plugins
- **TypeScript SDK — BROKEN (verified).** Sends `punctuate` (gateway requires `punctuation`, no serde default → config rejected); reads `wire.text`/`session_id` (gateway sends `transcript`/`stream_id` → empty transcripts, null session); realtime client speaks raw OpenAI protocol to the gateway's *normalized* `/realtime` endpoint; phantom `stop/flush/ping`; reconnect flushes audio before re-sending config (audio loss). Needs a `messages.ts`/`realtime.ts` rewrite.
- **Python SDK — PARTIAL (mostly real).** Config/transcript/binary-audio/auth all correct; dead phantom handlers (`tts_audio`/`turn_completed`/`vad_event`/`pong`); fragile reconnect (mid-flight audio loss); always `audio:true`.
- **Widget — REAL** (best SDK), one fragility: requests `AudioContext({sampleRate:16000})` with no resampling fallback → garbled STT on browsers that ignore the rate.
- **Dashboard — REAL** (demo grade).
- **Plugin system — REAL but DOA + UB gaps.** Builtin registry, PHF O(1) lookup, abi_stable ABI, semver-before-init are all genuine. **But** the dynamic loader is dropped immediately after loading in `main.rs:140-153` → every plugin's `shutdown()` runs at startup (plugins dead-on-arrival); `init`/`shutdown`/all FFI callbacks lack `catch_unwind` → UB on panic across the boundary; `enum_dispatch` is a dependency but unused (README claim false); plugins load from any path with no signing/allowlist.

---

## 3. Provider integration scorecard

| Status | STT | TTS |
|---|---|---|
| **REAL (streaming, usable)** | Google, ElevenLabs, IBM, AWS, iflytek, alibaba, baidu, huawei, amivoice, speechmatics, gladia, revai, reverie | Deepgram, OpenAI, AWS Polly, LMNT, PlayHT, Murf, Resemble, Smallest, + most regional (yandex, sber, bhashini, reverie, iflytek, alibaba, baidu, tencent, huawei, naver, fpt, viettel, prosa, nectec, acapela, speechmatics) |
| **REAL but batch-only (high latency)** | OpenAI, Groq, bhashini, yandex, sberdevices, naver, viettel, fpt, nectec | Google |
| **PARTIAL (crippled by config/factory)** | Deepgram, AssemblyAI, Gnani, Sarvam | ElevenLabs, Azure, Cartesia, IBM, Hume, CereProc*, Gnani |
| **BROKEN (won't connect/auth/play)** | Azure, Cartesia, Tencent, Tinkoff | Speechify, UnrealSpeech, WellSaid (default codec), Tinkoff |
| **STUB / fabricated** | Phonexia | — |

\*CereProc downgraded from "fabricated" after verification — endpoints need confirming, but the REST v2 API is real.

---

## 4. Security issues (consolidated)
1. **Rate-limit bypass** via spoofed `X-Forwarded-For` (S7).
2. **Rate limiter auto-disabled** at ≥100k rps (S7).
3. **DAG webhook SSRF** — no URL validation on `WebhookOutputNode` (multi-tenant, client-supplied).
4. **Rhai DoS** — client-supplied expressions run synchronously on the async thread, op-limit only, no wall-clock timeout.
5. **WS audio path lacks an explicit auth gate** (defense-in-depth only by accident).
6. **Over-privileged LiveKit tokens** (record/create/list) with no TTL.
7. **ML model downloads have no real integrity verification** (S10).
8. **Dynamic plugins load from any path, unsigned**, with UB on FFI panic.
9. **Secrets in `Debug`-derived config structs** (one stray `{:?}` dumps all keys).

---

## 5. README / marketing vs reality
- "Lock-free data passing" — **false** (HashMap+clone+Mutex; rtrb is dead code).
- "112k RPS / 38 MB RSS" — **misleading** (limiter likely off; not through the 4-permit REST path; incompatible with the lazy-expiry cache).
- "Automatic reconnection" (TS SDK, Hume) — **false/dead code**.
- "enum_dispatch static dispatch (10x)" — **unused**.
- "Emotion control (Hume/ElevenLabs/Azure)" — **only Cartesia honors it**.
- "Per-language thresholds" (turn detect) — **defined, never used**.
- "SNR-adaptive / echo suppression" noise filter — **amplitude gating / post-filter, not SNR/AEC**, and off by default.
- "70+ providers, all features integrated" — breadth is real, but advanced features are unreachable through the dispatch layer (S1).

---

## 6. Prioritized fix list

**P0 — correctness/security, do before any production traffic**
1. S1: thread provider-specific params through dispatch (typed extra-config map or per-provider config passthrough) — unlocks the whole product.
2. S7: switch to `PeerIpKeyExtractor` (or trust XFF only from a known proxy CIDR); remove the ≥100k auto-disable.
3. S6: fix DAG split (prune `reachable_nodes` after split) + join source-correlation; add SSRF checks to `WebhookOutputNode`; add a wall-clock timeout to Rhai eval (and move it off the async thread).
4. Azure STT framing + `speech.config`; Cartesia `cartesia_version`; Tencent signature host+path; verify/fix Tinkoff auth — or mark these providers unsupported.
5. S3: reconcile `audio_format` with each provider's real codec (fixes WellSaid/Speechify/UnrealSpeech).
6. Plugin loader lifecycle (don't drop it) + `catch_unwind` around all FFI calls.
7. TS SDK: rewrite `messages.ts`/`realtime.ts` to the real protocol (or ship a generated client from the gateway's OpenAPI).

**P1 — reliability**
8. S4: add reconnection with backoff to streaming providers + Hume EVI.
9. S2: make `turn-ensemble` (or a sane VAD) part of `default`, or fail loudly when stubbed.
10. Silero v5 tensor I/O; Smart-Turn `spawn_blocking` + fix the streaming mel buffer + verify tensor layout.
11. S10: real model hash pinning/verification.
12. voice_manager reconfig `stop()`; bounded mpsc for LiveKit frames; LiveKit token TTL + least-privilege grants.

**P2 — quality/claims**
13. Commit `Cargo.lock`; reconsider `panic = "abort"`; audit hot-path unwraps.
14. Add Deepgram Aura-2 list + `keyterm`; ElevenLabs/Cartesia WS streaming; expose OpenAI `instructions`, Hume `description`.
15. Make the README claims match reality (or implement them).

---

## 7. Bottom line
The engineering is real and the breadth is genuinely impressive — dozens of providers are correctly authenticated and wired, the realtime/LiveKit/SIP subsystem is mature, and the cache/SIMD/plugin-registry internals are solid. But shipped as-is this is a **technical preview, not a `v1.0.0` production gateway**: the flat-config bottleneck makes the provider breadth largely cosmetic, several flagship integrations can't connect, the parallel-DAG and rate-limiting paths are unsafe, the default build has no neural VAD, the TS SDK doesn't talk to its own gateway, and the benchmark/marketing claims don't survive contact with the code. Fix the P0 list first; everything else is downstream of S1.
