# WaaV Infer ↔ WaaV Gateway — Optimal Integration Plan

**Status:** design of record (greenfield against a partly-specified seam). **Date:** 2026-06. **Rev: v3** (owner decisions D1–D6 incorporated — §15).
**Companions:** `WaaV/inferv2/INFER_SPEC.md` (§4, §13, §14 gateway contract + GW-1..GW-7), `WaaV/inferv2/INFER_ENGINE_V2.md` (the engine architecture — its §3/§6/§7 router + control-plane + transport rules are reconciled here), `WaaV/inferv2/INFER_FINAL.md`, `WaaV/SDK_STANDARDIZATION_PLAN.md`.
**Method:** 8-domain exhaustive code read of `WaaV/gateway/` (~280K LoC) + `waav-infer/` + the spec → two adversarial review passes (code-verification + completeness/optimality) → **owner decisions D1–D6**. Load-bearing claims are `file:line`-anchored.

> **Rev v3 (owner decisions):** **D1** — at 10s/100s of servers Infer runs as a **standalone GPU tier** behind its own router, not a per-host sidecar (§7). **D2** — Infer hosts **audio models only**; text LLMs/LRMs stay **outside** Infer, the gateway talks to them (§1, §10). **D3** — **full barge-in is a now/M1 requirement**, targeting the Kyutai / Thinking-Machines *interaction-model* full-duplex experience (new §6.4). **D4** — keep the clean split (no gateway OpenAI facade). **D5** — native-S2S ships on loopback-ws first, UDS/datagram later. **D6** — **all routing/queuing/prioritization/scheduling/worker-dispatch is owned by the Inference server**; the gateway addresses Infer's router endpoint and owns only cross-tier failover + tenant labels (§11) — gateway-side pool routing (old GW-15) is **removed**. Prior rev-v2 corrections in §16.

---

## 0. TL;DR

WaaV Infer is the **self-hosted inference tier behind the gateway's existing provider seams** — not a new subsystem. The gateway stays the voice-AI **control / transport / orchestration plane**; Infer is the **model-forward plane** (and, at scale, owns its own routing/scheduling). They meet at three trait seams the gateway already has — `BaseSTT`, `BaseTTS`, `BaseRealtime` — through one plugin registry.

**The seven decisions that define the integration:**

1. **Infer is a registered gateway provider `"waav-infer"`** — one adapter crate plugged into the `inventory`→PHF→`DashMap` registry (`src/plugin/registry.rs`) like any provider; zero changes to handlers/routes/`VoiceManager`/lifecycle.
2. **Topology is a *spectrum*, config-selected** (§7, **D1**): **edge/CPU** → in-process; **single box / small** → sidecar over UDS; **fleet (10s–100s servers)** → **standalone Infer GPU tier behind Infer's own router**, the gateway fleet talking to it over the network. Crash physics + runtime isolation (`INFER_ENGINE_V2.md` §17.2) keep GPU inference out-of-process at every tier above edge.
3. **Two regimes.** *Cascade-over-Infer* = `BaseSTT`+`BaseTTS`, the gateway's Smart-Turn/eager-EoT/barge-in reused (the **clean** fit, Unmute-style). *Native-S2S-Infer* = `BaseRealtime` via the S2S scaffold, **the full-duplex "interaction model"** (Moshi/Kyutai-class) — turn-taking/interruption intrinsic to the model, gateway a thin passthrough (the **gold-standard** experience, §6.4).
4. **The gateway owns the audio front-of-pipeline; Infer owns the model forward.** Gateway: VAD (Silero v5), turn detection (Smart-Turn-v3), *inline* noise (DeepFilterNet), codec/transport, `StreamResampler`, **barge-in**. Infer: PCM16 LE 16 kHz mono at the wire (f32 in-process) → PCM + events; model-specific mel/fbank stays inside Infer.
5. **Infer hosts audio models only** (**D2**): STT, TTS, and S2S — *including an S2S model with an intrinsic LLM*. **Standalone text LLMs / reasoning models live outside Infer** (vLLM/llama.cpp/cloud); the gateway's `LlmAdapter` talks to them. LLM serving/scaling is deliberately not Infer's job.
6. **The Inference server owns routing, queuing, prioritization, scheduling, and worker-dispatch** (**D6**). The gateway addresses **one logical Infer endpoint (the router)** and does *not* load-balance across workers. The gateway keeps only **cross-tier failover** (whole Infer fleet rejects/down → cloud provider) and **passing tenant/class labels** down for Infer's fair-scheduler.
7. **Full barge-in is a first-class, now (M1) requirement** (**D3**), across *every* path — cascade (instant LLM-cancel, already live on the conversation path), DAG (the GW-8 fix, pulled forward), and native-S2S (model-intrinsic). The bar is the Kyutai / Thinking-Machines interaction model; **Full-Duplex-Bench** is the acceptance harness.

**One canonical notation end-to-end** via a two-hop standardizer (gateway provider-facing BCP-47/`VoiceDescriptor`/emotion/LoRA → adapter → Infer model-native ISO-639-1/precision/device).

**The regime cheat-sheet:**

| | Cascade-over-Infer (Unmute-style) | Native-S2S-Infer (interaction model) |
|---|---|---|
| Gateway seam | `BaseSTT`+`BaseTTS` (own transport → **UDS OK**) | `BaseRealtime` via S2S scaffold (**ws today**, UDS=GW-13) |
| Turn-taking / interruption | **Gateway** Smart-Turn-v3 + eager-EoT + instant barge-in | **Model**-intrinsic (`emits_user_turn_frames()=true`); gateway forwards `cancel`/`truncate` |
| LLM | **external** (vlLM/cloud), swappable | **intrinsic** to the S2S model |
| Latency target | sub-second first-audio | 160–200 ms (Moshi-class) |
| Fit / value | clean; LLM-swappable; broadest compatibility | the truest full-duplex experience; gold standard |
| Maturity | M1 (incl. **full barge-in**) | M3 (full-duplex + Full-Duplex-Bench gate) |

---

## 1. The integration thesis (+ the D2 scope boundary)

WaaV Infer is "the self-hosted inference tier of the WaaV voice gateway, and also runs standalone" (`INFER_SPEC.md:13,24`). The relationship is **bidirectionally clean**: the gateway treats Infer as a provider behind `BaseSTT`/`BaseTTS`/`BaseRealtime` and DAG nodes — local inference gains the gateway's VAD, turn detection, noise reduction, barge-in, DAG, observability, cloud-failover **for free**; Infer standalone serves its native WS + OpenAI-compat APIs to the ecosystem audience (Pipecat, LiveKit, OpenWebUI) the gateway deliberately does not serve.

Scope boundaries (`INFER_SPEC.md:74-83`): the gateway owns orchestration/VAD/turn/noise/barge-in/transport; Infer **consumes** gateway signals, never re-implements them. Code-confirmed: Infer has **no VAD, no Silero, no endpointing, no inline-cascade noise** (its `components/noise.rs` is a Gaussian latent sampler for flow-TTS; it *does* ship a standalone `Enhancer` — `enhance.rs` — as an opt-in enhancement task, off the STT cascade — never double-denoise).

**The D2 model-scope boundary (owner-set, now explicit):**
- **Infer hosts audio models:** STT (Whisper/Parakeet/…), TTS (Kokoro/Orpheus/…), and **S2S** — including a full-duplex S2S model whose LLM is **intrinsic** (Moshi-class: the LLM *is* part of the audio-token model).
- **Infer does NOT host standalone text LLMs or reasoning models (LRMs).** "LLM inference and scaling is hard" — it stays a dedicated tier (vLLM/llama.cpp/cloud) the gateway reaches through its existing `LlmAdapter` (`core/llm/adapter.rs`, any OpenAI-compatible `base_url`). This matches `INFER_SPEC.md:83` ("text LLMs route to vLLM/llama.cpp/cloud … Infer's AR machinery exists for audio-token models").
- **Consequence for the two-tier reasoning design (§10):** the fast tier and the reasoning tier are *both external* LLM endpoints; the latency win comes from **co-locating that external LLM** near the gateway, not from Infer hosting it.

---

## 2. Starting point — exists vs. unbuilt

### 2.1 Gateway seams that exist (reused verbatim)

| Seam | Where |
|---|---|
| Plugin registry | `src/plugin/registry.rs` (`global_registry()`; `inventory::collect!`→PHF→`DashMap`; panic-isolated factory calls registry.rs:326) |
| Provider traits | `core/stt/base.rs:597`, `core/tts/base.rs:269`, `core/realtime/base.rs:640` |
| S2S scaffold | `core/realtime/scaffold/{session,protocol,transport,event}.rs` — `RealtimeSession<P>`: reconnect supervisor, breaker/governor, barge-in/truncate/preroll, replay |
| Standardization | `core/{lang,voice,emotion,alias}/` (re-exported `core/mod.rs:91`) |
| Resilience | `core/resilience/{circuit_breaker,reconnect_governor,registry}.rs`, `core/websocket/reconnectable_stream.rs` |
| Transport | `src/livekit/`, `src/handlers/sip/`, `core/audio/{codec,opus_codec,resampler,output_chunker}.rs` |
| Front-of-pipeline | `core/{silero_vad,smart_turn,turn_decision,turn}/`, `utils/noise_filter.rs`, `core/voice_manager/manager.rs` |
| Reasoning orchestration | `core/conversation/mod.rs`, `core/llm/{mod,adapter}.rs` |
| DAG | `src/dag/` (feature `dag-routing`) |

### 2.2 Infer side that exists

6-crate workspace; **`waav-infer-protocol`** (`ClientFrame`/`ServerFrame` ws.rs; `SessionConfig`/`AudioSpec::pcm16`/`Conditioning` session.rs; `Transcript` stt.rs; `ChunkMeta` tts.rs; `InferError` with `is_lifecycle()`/`retriable()`/`http_status()` error.rs); engine seam `fn transcribe(&mut self,&[f32])->Result<String,InferError>` / `fn synthesize(...)->Result<Vec<(ChunkMeta,Vec<i16>)>,InferError>` (`waav-infer-core/src/model.rs:96,125`); config-arch model registry (model.rs:177); standalone `Enhancer` (enhance.rs); OpenAI-compat REST (`/v1/audio/*`, `/v1/models`) + native WS (`/v1/ws`) + `/livez`/`/readyz`/`/metrics`; Infer standardizer (`waav-infer-components/src/standardize.rs`). The inferv2 design also names **`waav-infer-router`** (prefix-affinity, VTC fair-share) and **`waav-infer-control`** (used/total-slots, reject-reason, per-replica admission cap) — the components D6 puts in charge of routing/scheduling.

### 2.3 Unbuilt (the work surface)

No `waav-infer-provider`/`waav-gateway-provider-api` crate; gateway is single-crate (GW-1 structural); zero `waav-infer` wiring. S2S scaffold has **no UDS transport** (`ConnectSpec` = `WebSocket`/`RestThenWebSocket`/`BedrockBidi`; `apply_endpoint_override` rejects non-`ws://` — event.rs:169). `TTSConfig` has **only `voice_id`** (no conditioning — tts/base.rs). **No OpenTelemetry** (inbound `traceparent` collapsed to a log id — request_id.rs:74). The DAG turn loop awaits the whole turn (`config_handler.rs:2877`), so mid-turn barge-in queues — the **GW-8 fix, now pulled to M1**.

---

## 3. The seam map

```
   ┌─────────── CLIENT TRANSPORT ───────────┐
   │  WebRTC (LiveKit) · SIP (LiveKit SIP)   │   phone/G.711/opus terminate in LiveKit;
   │  raw /ws (PCM16/opus) · /realtime (WS)  │   gateway only ever sees clean PCM16
   └───────────────────┬─────────────────────┘   (auth · rate-limit · conn-limit · request-id)
   ┌───────────────────▼──────── GATEWAY: FRONT-OF-PIPELINE ───────────────────┐
   │  StreamResampler→16k · Silero v5 VAD · Smart-Turn-v3 (EoT) · eager-EoT ·    │
   │  DeepFilterNet (inline) · opus/G.711 · 20ms egress chunker · BARGE-IN       │
   └───────────────────┬───────────────────────────────────────────────────────┘
                       │   clean PCM16 16k mono in / PCM + events out
   ┌───────────────────▼──────── GATEWAY: ORCHESTRATION ───────────────────────┐
   │  VoiceManager (cascade)  │  RealtimeSession<P> (S2S, thin passthrough)      │
   │  ConversationOrchestrator (LLM loop → EXTERNAL LLM tier; reasoning-opt)     │
   │  DAGExecutor (per-turn cancellable — GW-8)                                  │
   │  cross-tier failover (Infer-fleet rejects → cloud) + tenant/class labels    │
   └───────────────────┬───────────────────────────────────────────────────────┘
                       │   global_registry().create_{stt,tts,realtime}("waav-infer", cfg)
   ┌───────────────────▼──────── waav-infer-provider (THE ADAPTER) ─────────────┐
   │  BaseSTT + BaseTTS (UDS-capable) │ BaseRealtime (ws today; UDS/datagram GW-13)│
   │  canonical→model-native (hop 2) · From/Into ↔ protocol wire · traceparent   │
   │   topology = config:  in_process(edge) │ sidecar/UDS(box) │ remote→Infer ROUTER(fleet) │
   └───────────────────┬───────────────────────────────────────────────────────┘
                       │   native WS v1 (ClientFrame/ServerFrame)
   ┌───────────────────▼──────── WaaV INFER (owns routing/queue/sched — D6) ────┐
   │  waav-infer-router: prefix-affinity · VTC fair-share · worker dispatch       │
   │  waav-infer-control: used/total-slots · reject-reason · per-replica cap       │
   │  admission (schedulability+duty ledger) · lockstep AR + step-bucket ·        │
   │  3-tier KV · stage-DAG · config-arch registry · {STT,TTS,S2S} workers        │
   └────────────────────────────────────────────────────────────────────────────┘
```

The boundary is **at the model** (gateway DAG = inter-service/turn-granularity; Infer stage-DAG = intra-model/frame-granularity, §9). At fleet scale the adapter's "remote" endpoint is **Infer's router**, not a worker (D6).

---

## 4. Division of labor

| Concern | Owner | Note |
|---|---|---|
| WebRTC / SIP / RTP / G.711 media | **Gateway → LiveKit (+ LiveKit SIP)** | gateway is the "waav-ai" participant; sees clean PCM16 |
| opus / codec / jitter / resample | **Gateway** | `core/audio/*`; `StreamResampler` |
| VAD / turn detection / EoT | **Gateway** (cascade) / **Model** (S2S) | Silero v5; Smart-Turn-v3 |
| inline (live-path) noise | **Gateway** | DeepFilterNet (`utils/noise_filter.rs`) |
| standalone enhancement model | **Infer (opt-in task)** | `enhance.rs`; off the STT cascade; never double-denoise |
| **barge-in** | **Gateway** (cascade+DAG) / **Model** (S2S) | now/M1 across all paths (§6.4, §9) |
| LLM loop / reasoning-opt | **Gateway** orchestrates | the LLMs themselves are **external** (D2) |
| **text-LLM / LRM serving** | **External tier** (vLLM/llama.cpp/cloud) | **not Infer** (D2); via `LlmAdapter` |
| standardization (client-facing / model-native) | **Gateway / Infer adapter** | two-hop (§8.2) |
| auth / tenancy / quotas / recording | **Gateway** | |
| **routing / queuing / prioritization / scheduling / worker-dispatch** | **Infer (router + control plane)** | **D6** — the gateway addresses one router endpoint, does not route across workers |
| cross-tier failover (Infer-fleet ↔ cloud) | **Gateway** | the gateway can see "the whole Infer tier is down"; the router can't |
| per-tenant fairness | **Infer VTC** (gateway passes labels) | gateway may add a coarse audio-second admission quota |
| model forward / GPU admission / KV / numerics | **Infer** | |
| resilience (network-shaped) | **Gateway** | breaker/governor/replay on the crash path |
| distributed tracing | **Both (new)** | gateway OTel + Infer stage spans under `traceparent` (GW-17) |

**Wire format (pinned):** the native-WS-v1 seam carries **PCM16 LE mono** (`AudioSpec::pcm16`); the adapter converts gateway-internal f32 ↔ i16. In-process (T2) passes f32 directly to `transcribe(&[f32])`. 8 kHz telephony: DeepFilterNet is 48 kHz-only → prefer a 16 kHz denoiser on the STT-ingress path (Infer FR-D4).

---

## 5. Regime A — Cascade-over-Infer (the clean fit, Unmute-style)

### 5.1 What the adapter implements

- **`BaseSTT`** (`stt/base.rs:597`): `new`/`connect`/`disconnect`/`is_ready`/`send_audio(Bytes)`/`on_result(STTResultCallback)`/`on_error`/`apply_settings_delta`/`ttfs_p99_ms`/`set_resilience` + **new `finalize(id)`** (GW-4). Egress is a **callback** (base.rs:589); three-level finality (base.rs:237) maps from Infer's `Transcript`.
- **`BaseTTS`** (`tts/base.rs:269`): `new`/`speak`/`flush`/`speak_with_context`/`on_audio_context_interrupted` (→ **new `clear_context`**, GW-4)/`on_audio(AudioCallback)`. `AudioData{sample_rate,…}` from `ChunkMeta` — **preserve `sample_rate`** (playback/barge-in timing). **Asymmetry to fix (GW-5):** `set_resilience`/`apply_settings_delta` are STT-only today; extend to `BaseTTS`.

### 5.2 Native WS v1 ↔ trait mapping

| Gateway call | `ClientFrame` | `ServerFrame` → gateway |
|---|---|---|
| `connect()` | `SessionConfig{model,task,language,audio:pcm16}` | `Ready{session_id,protocol_version,model_digest}` |
| `send_audio` (STT) | *(binary PCM16)* | `Transcript`→`STTResult` (callback) |
| `finalize(id)` | `Finalize{id}` | `Finalized{id}` |
| `speak(text,flush)` | `Speak{text,context_id,utterance_id,flush}` | `ChunkMeta`+binary→`AudioData` |
| `flush()` / `clear_context(id)` | `Flush{id}` / `Clear{context_id}` | `Flushed{id}` / `Cleared{context_id}` |
| `apply_settings_delta` (STT; TTS via GW-5) | `SessionUpdate{language,keyterms,speed}` | — |
| keepalive | `Keepalive` | — |
| (lifecycle/errors) | — | `Error(InferError{code,retry_after_ms})`→GW-3 ; `Close{reason}` |

### 5.3 Turn-taking & barge-in reused wholesale

Infer-STT results → `ControllerSignal::{SttInterim,SttFinal}` → `TurnController`; Smart-Turn-v3 runs on the same 16 kHz buffer in `receive_audio` (manager.rs:521) → `SmartTurn` signal → eager-EoT (`conversation/mod.rs:1700`). Barge-in cancels Infer-TTS via `clear_tts()` → `on_audio_context_interrupted` (GW-4 `clear_context`→`Clear`) + epoch bump (manager.rs:822). **Backchannel discipline:** `MinWordsStart` (`turn/strategies/min_words.rs`) already prevents "mhm/yeah" from barging in — exactly the Unmute behavior. Feed Infer-STT the gateway's already-resampled 16 kHz buffer so Infer skips its edge resample.

### 5.4 Keystone gate (corrected) + GW-12 wire test

`provider_keystone_completeness` is a **`cargo test` assertion, not a build gate**, over **hardcoded** provider lists, and a lazy adapter still passes via the flat factory → it does **not** guarantee typed features reach Infer. GW-12 therefore carries a **dedicated per-provider wire test** asserting keyterms/diarization/language survive `From`/`Into` to `SessionConfig`/`SessionUpdate`.

---

## 6. Regime B — Native-S2S-Infer (the full-duplex interaction model)

### 6.1 What the adapter implements

`InferProtocol: RealtimeProtocol` (`scaffold/protocol.rs:23`): `build_session_config`→`SessionConfig{task:S2S}`; `encode_user_audio`→Infer frame; `map_server_event`→`S2sEvent::{Audio,Transcript,Speech,ResponseDone}`; `truncate`/`create_response`→`Clear`/`Speak`. `ProtocolCaps` sets native rate + **`emits_user_turn_frames=true`** (from an Infer manifest capability flag; an integration test asserts the cascade Smart-Turn is bypassed iff true).

### 6.2 Transport reality (D5)

The scaffold is **ws-only** today (no `ConnectSpec::Unix`). **D5: ship native-S2S on loopback `ws://` first** (same-box TCP-HOL is negligible), add **GW-13** (`ConnectSpec::Unix` for sidecar parity + a **datagram/UDP-QUIC media path for the *remote* fleet case** — `INFER_ENGINE_V2.md` §3-pillar-7: WS-over-TCP loses ~12 frames per drop) later. Cascade is unaffected (its `BaseSTT`/`BaseTTS` adapters own transport and can speak UDS now).

### 6.3 LiveKit/SIP glue (GW-9, pulled to M3)

A "realtime transport mode" in `config_handler.rs` feeds the LiveKit ingress callback (clean PCM, bounded ordered channel) to `BaseRealtime::send_audio` and drives `on_audio`→`client.send_tts_audio` (the DAG `endpoint` node already does this bridge — `dag/nodes/endpoint.rs:1406`).

### 6.4 The full-duplex interaction-model target (D3) — the experience bar

The owner requirement: barge-in **now, in full**, with an experience as close as possible to the **Kyutai (Moshi/Unmute)** and **Thinking-Machines** *interaction models*. What that means, grounded in those sources:

- **No artificial turn boundaries.** Interaction models "process 200 ms of input and generate 200 ms of output continuously," perceiving and responding *concurrently* — interruptions, overlaps, and backchannels emerge as simultaneous token streams, not as a turn state machine. Interactivity is **part of the model**, not bolted on.
- **Latency:** Moshi is **160–200 ms** mic-to-speaker; interaction-model turn-taking ≈ **0.40 s**. Our targets: **native-S2S ≤ ~200 ms**, **cascade sub-second** first-audio.
- **Behaviors to hit:** interrupt the user / be interrupted instantly; **backchannel** ("uh-huh", "I see") without yielding the turn; handle **overlap**; prompt-stop on real interruption (Moshi 0.42–1.16 s stop).

**How WaaV delivers it on each path — barge-in is M1, full-duplex S2S is M3:**

1. **Native-S2S-Infer = the gold standard.** A Moshi-class full-duplex model hosted by Infer owns interruption/backchannel/overlap intrinsically (its dual-stream models user+assistant audio jointly; inferv2 already has the duplex `MultiStreamSlot` + EoT head + `AcousticDelayRing`). **The gateway must get out of the way:** `emits_user_turn_frames()=true` bypasses the cascade Smart-Turn/`TurnController`; the gateway streams audio bidirectionally and forwards only `cancel`/`truncate`; for a continuous-duplex model even those degrade to no-ops (barge-in is simply the model hearing the user). This is the path that *truly* matches Kyutai. **GW-18:** Infer hosts/serves a full-duplex S2S model; the gateway thin-passthrough; **acceptance = Full-Duplex-Bench** (interruption, backchannel, overlap, smooth-turn-taking) at ≤200 ms.
2. **Cascade-over-Infer = the Unmute-style approximation** (for swappable external LLMs). The gateway already has the pieces and they're live on the `ConversationOrchestrator` path: **instant barge-in-cancels-LLM** (compute-cancel first, unconditionally — `conversation/mod.rs:1118`), **eager-EoT** (speculate before the user fully stops), **Smart-Turn-v3 semantic VAD**, **backchannel-immune** `MinWordsStart`, **latency_filler** to mask think-time. With a **co-located external fast LLM** (D2) this hits sub-second first-audio. It won't match 160 ms intrinsic overlap, but it's the Unmute trade (any LLM, swappable).
3. **DAG path barge-in = GW-8, pulled to M1.** The cancel-token already exists and nodes honor it (`context.rs:62`, `executor.rs:200`); the only gap is the StreamDriver awaiting the whole turn (`config_handler.rs:2877`). **Fix now:** spawn the turn future + hold its cancel handle so the next `Started` interrupts the in-flight Infer node — so barge-in works on *every* path, not just the conversation path.

**Net D3 plan:** M1 ships full barge-in on cascade (conversation **and** DAG) + the backchannel discipline; M3 ships the native-S2S full-duplex model behind a Full-Duplex-Bench gate. The interaction-model experience is a named acceptance target, not an afterthought.

### 6.5 Inherited for free

Registering `InferProtocol` makes `/realtime` + the DAG `realtime_provider` node inherit the scaffold's reconnect supervisor, breaker/governor, barge-in/truncate/preroll, and ≤100-item replay.

---

## 7. Topology & deployment (D1 — a spectrum, not one default)

**Crash physics + runtime isolation keep GPU inference out-of-process above edge** (CUDA sticky errors / native aborts uncatchable; `INFER_ENGINE_V2.md` §17.2 forbids GPU work on tokio worker threads). Latency does not pick the topology (UDS RTT ≈ 2.3 µs vs 20 ms frames).

| Scale | Topology | How the gateway reaches Infer | Resilience / notes |
|---|---|---|---|
| **Edge / CPU** (kiosk, robot, phone-adjacent) | **in-process** (`infer-inproc`, GW-6) | same process, f32 direct | CPU/ORT-tier models only (Kokoro/Piper/VAD); warning-labeled; non-default cargo feature |
| **Single box / small** | **sidecar over UDS** | local UDS, native WS v1 | sidecar crash = provider error (breaker/governor handle it, GW-3); supervised per §4.2b (pidfile/adopt/pipe-EOF/restart-on-death-not-readyz); **peer-cred (`SO_PEERCRED`) auth + separate admin socket** |
| **Fleet (10s–100s servers)** ← **D1** | **standalone Infer GPU tier behind `waav-infer-router`** | network (WS/TLS; datagram media GW-13) to **one router endpoint** | the gateway is a network client of the router; **Infer owns routing/queue/sched/dispatch** (D6); gateway owns cross-tier failover + tenant labels; bearer-key auth; admin verbs on a separate control plane |

**The fleet shape (D1 + D6):** a horizontally-scaled **gateway tier** (stateless except the flagged single-node `batch_jobs`/conn-counters) talks to a **standalone Infer tier** — `waav-infer serve` GPU workers behind `waav-infer-router`. The gateway provider's `mode=remote` endpoint is the **router**, which does prefix-affinity, VTC fair-share, queuing, prioritization, scheduling, and worker dispatch. Adding GPUs scales Infer; adding gateway nodes scales transport/orchestration; the two scale independently. **NFR-P7:** the gateway-added overhead (adapter + hop) stays ≤ 5 ms p99 (CO-corrected) for the local/sidecar case; for the fleet case the network RTT to the router is the dominant adapter cost and is budgeted separately. **GW-2 co-residency** (CPU-pin the gateway's Silero/Smart-Turn) applies only where the gateway shares a GPU box with Infer (single-box/small); in the fleet topology the gateway nodes are GPU-less, so GW-2 is moot there.

---

## 8. Endpoints & standardization

### 8.1 Endpoint asymmetry

**The gateway is NOT OpenAI-wire-compatible** (no `/v1/audio/*`, `/v1/chat/completions`, `/v1/models` routes — those strings are outbound provider URLs). Native surface: `/speak`, `/voices`, `/voices/clone`, `/transcribe/batch`, `/capabilities/languages`, `/dag/*`, WS `/ws` + `/realtime`, ops `/livez`/`/readyz`/`/metrics`. **Infer *is* OpenAI-compatible** → ecosystem clients hit Infer standalone; gateway clients use the native surface; the gateway calls Infer over native WS v1. **(D4: keep this split — no gateway OpenAI facade.)**

### 8.2 The two-hop standardizer

| Concept | Gateway canonical (client-facing) | Hop 2 → Infer model-native | Mechanism |
|---|---|---|---|
| Language | region-BCP-47 (`en-US`,`cmn-CN`) | ISO-639-1 (`en`,`zh`) | **new `InferLanguageMapper`** (like `ElevenLabsLanguageMapper`) |
| Voice | `VoiceDescriptor`+`resolve_voice` | Infer voice-bank id | Infer `TtsModel::voices()` → `/voices` catalog |
| Voice (LoRA) | a voice id naming a LoRA adapter | Infer LoRA-swap id (`INFER_ENGINE_V2.md` R5) | model/voice rows carry LoRA ids |
| **Voice-clone conditioning** | **new `conditioning` on `TTSConfig`** (ref-audio / embedding handle) — absent today | Infer `Conditioning::{voice-bank,speaker-embedding,prompt-audio}` (FR-E10) | **GW-14**: voice-registration→Infer `POST /v1/voices` bridge; biometric/GDPR governance (§16.7) |
| Model | provider `model` / P3 alias | Infer `/v1/models[].id` | P3 alias `fast-stt→{provider:"waav-infer",model:"parakeet-tdt"}`; aggregate Infer `/v1/models` (GW-10) |
| Emotion | `Emotion`(43)+`DeliveryStyle`(27)→`MappedEmotion` | `instruction_text`/`inline_tags` | existing mapper |
| Precision/device | *(none — Infer-only)* | `fp16`/`cuda` | `ProviderExtras` open-map passthrough |

Enforcement: the round-trip parity test covers language, voice-bank ids, emotion→instruction-text, and LoRA voices.

### 8.3 Discovery (GW-10)

A `waav-infer` row generator calls Infer `/v1/models` (+`supported_languages()`,`voices()`) → `/capabilities/languages` + `/voices`; add the planned general `GET /capabilities` (+`/models`) aggregating cloud + Infer.

---

## 9. DAG composition

Gateway DAG (inter-service/turn) treats an Infer model as a **single opaque node** (`stt_provider[infer]→llm→tts_provider[infer]`; native S2S = one `realtime_provider[infer]`); Infer's stage-DAG stays hidden. Resolved through the same registry (`dag/nodes/provider.rs:234`) — zero gateway-DAG changes. **Rejected:** exposing Infer's internal stages as gateway nodes (destroys lockstep/CUDA-graph atomicity). **Avoid the `ipc_endpoint` trap** (request/response, non-streaming — endpoint.rs:1095); use the provider path.

**Barge-in (GW-8, now M1 — D3):** the cancel-token plumbing already exists (`context.rs:62`, `executor.rs:200/225`, `:1628` `select!` on `cancelled()`); the only gap is the StreamDriver awaiting `execute_from(...).await` for the whole turn (`config_handler.rs:2877`). Fix: spawn the turn future + hold its cancel handle so a new `Started` cancels the in-flight Infer node. Pulled forward so **every** path (conversation, DAG, native-S2S) supports full barge-in in M1/M3.

---

## 10. Reasoning-model optimizations — with external LLMs (D2)

The gateway's eight reasoning levers are **implemented** (`conversation/mod.rs`): `latency_filler`, two-tier `reasoning_model`, `reasoning_effort` dial, sentence-aggregation (`<think>`-stripped), parallel-fire, **barge-in-cancels-LLM**, the `reasoning_budget_ms` stall-watchdog, the cost budget.

**The Infer/D2 effect:**
- **Both tiers are external LLM endpoints** (D2 — Infer hosts no text LLM). The latency win comes from **co-locating the external fast LLM** near the gateway (zero/low network hop), *not* from Infer hosting it. DX unchanged: `model:"<co-located-fast>"` + `reasoning_model:"<cloud-reasoner>"`, both via `LlmAdapter` (`base_url`).
- **`latency_filler` becomes reasoning-tier-only** — a co-located fast model's TTFT (~0.17 s) is already under the 800 ms `wait_ms`; the filler stays essential for the reasoning tier (think-time is compute).
- **The stall-watchdog matters more** — a saturated co-located LLM that streams one token then stalls is the mid-call dead-air case `reasoning_budget_ms` covers.
- **Reasoning-opt knobs become load-adaptive LO-shed** — under Infer GPU pressure (audio side) the gateway disables eager-EoT/escalation/enhancement (Infer's LO-shed classes) before any hard reject (§11).
- **Exception (D2):** a native-S2S model with an *intrinsic* LLM needs none of this — reasoning/turn-taking is inside the model (that's the §6.4 gold-standard path).

---

## 11. Scale, distribution, performance, latency, fairness

### 11.1 Two scale models (perf numbers — indicative, not a guarded SLO)

- **Gateway = concurrency/queue-bound.** Indicative bare-HTTP (k6, `GET /`): measured peak **~107,887 RPS** (a summary line cites 112,528; reports are internally inconsistent), P50 0.343 ms / P99 ~1.65 ms, RSS ~38 MB; soft cliff **~28,000 VUs** — **bare-HTTP virtual users, not voice sessions**, from an `#[ignore]`d, **unasserted** harness needing an external gateway. Indicative of "the gateway is not the bottleneck," not an SLO. (Different runs: the 28k run peaked 54,102 RPS.)
- **Infer = GPU-capacity-bound** (64–128 streams/GB10; AR decode flat 1→64).

### 11.2 Admission + failover (D6: Infer owns routing/scheduling)

1. **Infer owns intra-tier routing/queuing/prioritization/scheduling/dispatch** (D6). The gateway sends a request to **one router endpoint**; the router does prefix-affinity (cloned-voice → the worker holding its prefix-KV, ~7× TTFA — `INFER_ENGINE_V2.md` §6.4), VTC fair-share, queuing, and worker dispatch. **The gateway does not load-balance across workers.**
2. **Gateway front-door admission** stays (network-bound, generous: `max_websocket_connections`, 60 rps/IP, per-IP 100).
3. **Cross-tier failover (the gateway's job):** when the **whole Infer tier** signals saturation/down — Infer's typed `reject-reason` (`admission_rejected`/`model_not_ready`/`draining`) surfaced through the adapter — **GW-3** classifies it as non-failure/failover-eligible/Retry-After-honoring and the gateway fails the session over to a **cloud provider** (or returns a clean 503). The half-open probe treats `model_not_ready` as "remain open without penalty" so a Warming restart doesn't flap.
4. **Cold-start:** extend GW-3 so the gateway consumes Infer's `/v1/models` per-model state and surfaces `retry_after` meaningfully (the router handles warm-worker selection internally).
5. **Per-tenant fairness:** **Infer's VTC** enforces it per-frame; the **gateway passes tenant/class labels** and may add a coarse audio-second admission quota. Neither did this before — now an explicit split.

### 11.3 Distribution (D1 + D6 — mostly Infer's job now)

The previous draft put a replica pool + affinity router in the **gateway** (old GW-15/16). **D6 moves that into Infer.** The gateway side reduces to:
- **point `mode=remote` at Infer's router endpoint** (one logical address per Infer tier/pool; the router fans out);
- **forward a conditioning fingerprint** for the router's prefix-affinity (**GW-16, reduced** — the gateway supplies the key, Infer's router owns residency/placement);
- **own cross-tier failover** (Infer-tier ↔ cloud) and tenant labels.
- **(old GW-15 — gateway-side multi-backend pool routing — is removed.)** Autoscaling, worker health, load-balancing, and queue/priority are the Infer control-plane's job.

### 11.4 End-to-end latency

```
 client → [LiveKit/SIP/ws] → StreamResampler→16k → Silero VAD + Smart-Turn ~12ms CPU
   → adapter → Infer router → worker → STT (lockstep)
   → ConversationOrchestrator → co-located external fast LLM ~0.17s | reasoning tier masked by filler
   → Infer TTS → 20ms egress chunker → resample→client → transport
   ── OR (native-S2S): client ⇄ gateway thin passthrough ⇄ Infer duplex model, ~160–200ms ──
```

Big win: removing provider network round-trips by being local; adapter+UDS ≤5 ms (single-box) / router RTT (fleet) is noise vs a 200 ms TTFA budget. Native-S2S is the lowest-latency path (no cascade hops).

---

## 12. Resilience reconciliation

Gateway resilience is **network-shaped**; Infer's is **compute-shaped**. **Sidecar/remote (default above edge):** a crash is a UDS/WS drop = a provider error; breaker (+credentials-fatal detector), governor, featured-restore+`AudioReplayBuffer`, and **GW-3 lifecycle exemption** apply; chaos-gated by the existing suites (a `waav-infer` mock plugs in via the `WsTransport` seam). **In-process (edge):** the network layer is inert and replaced by Infer's compute spine (NaN→reject-frame, frame-watchdog, brownout, cell isolation); the gateway's per-session `catch_unwind` still guards non-abort panics. **Auth:** peer-cred (UDS, single-box) / bearer (remote) / separate admin control-plane. **Tracing (GW-17):** OTel layer + outbound `traceparent` so Infer stage spans parent under the gateway profiler (none today — request_id.rs:74).

---

## 13. The work plan

### 13.1 The deltas (GW-1..GW-18; v3)

| Δ | What | Milestone |
|---|---|---|
| **GW-1** | Extract `waav-gateway-provider-api` crate (traits+`STTResult`+configs+`PluginConstructor`); gateway `provider-waav-infer` feature — breaks the inventory cycle | M1 |
| **GW-2** | Per-model EP override / co-residency CPU-pin (single-box only; moot in the GPU-less gateway fleet) | M1 |
| **GW-3** | Breaker treats `admission_rejected`/`model_not_ready`/`draining` as non-failure → **cross-tier failover** + cold-start `retry_after` surfacing | M1 (breaker) / M5 (failover routing) |
| **GW-4** | `BaseSTT::finalize(id)` + `BaseTTS::clear_context(id)` | M1 |
| **GW-5** | `STTConfig`/`TTSConfig` typed `endpoint`/`mode`; + `set_resilience`/`apply_settings_delta` on `BaseTTS` | M1 |
| **GW-6** | In-proc bootstrap `OnceLock<InferEngine>` (`infer-inproc`, **edge/CPU only**) | M4 |
| **GW-7** | ~~Realtime beta→GA dialect~~ — **DONE** (`openai/config.rs:24`) | — |
| **GW-8** | **(now M1)** DAG StreamDriver spawns the turn future + holds the cancel handle → **full barge-in on the DAG path** (cancel-token already exists) | **M1** |
| **GW-9** | "Realtime transport mode": LiveKit/SIP media → `RealtimeSession` (native-S2S over a room/call) | M3 |
| **GW-10** | `waav-infer` discovery-row generator + the `/capabilities` (+`/models`) endpoint | M4 |
| **GW-11** | Surface Infer `reject-reason`/`used-total-slots` through the existing 503 + `config_warning`; pass tenant/class labels; eager-EoT/escalation/enhancement as load-adaptive LO-shed | M5 |
| **GW-12** | `InferLanguageMapper` + `ProviderExtras` passthrough + `create_*_standard` arm + **per-provider wire test** | M1 |
| **GW-13** | `ConnectSpec::Unix(path)` + UDS transport in the S2S scaffold; + **datagram (UDP/QUIC) media for the remote fleet** | M3 (UDS) / M5 (datagram) |
| **GW-14** | `conditioning` field on `TTSConfig`/standard arm + voice-registration→Infer `POST /v1/voices` bridge + biometric governance | M3 |
| ~~GW-15~~ | ~~gateway multi-backend pool routing~~ — **REMOVED (D6: Infer's router owns it)** | — |
| **GW-16** | **(reduced)** gateway forwards a **conditioning fingerprint** to Infer's router; Infer owns prefix-affinity/placement | M5 |
| **GW-17** | OTel tracing layer + outbound `traceparent` injection | M4 |
| **GW-18** | **(new, D3)** native full-duplex S2S: Infer serves a Moshi-class model; gateway thin-passthrough; **Full-Duplex-Bench acceptance** (interruption/backchannel/overlap ≤200 ms) | M3 |
| **(topology)** | adapter `mode=remote` → **Infer router endpoint** (the fleet integration, D1); gateway = network client | M5 |

### 13.2 Milestones

- **M1 — Cascade-over-Infer, sidecar, + FULL barge-in (the adoptable core).** `waav-infer-provider` (`BaseSTT`+`BaseTTS` over native WS v1 + UDS); GW-1,2,3(breaker),4,5,8,12. **Acceptance:** `provider:"waav-infer"` across `/speak`,`/transcribe/batch`,`/ws`,**DAG**; **full barge-in on the conversation AND DAG paths** + backchannel-immunity (`MinWordsStart`); the per-provider wire test; protocol-version negotiation + a generated conformance test; NFR-P7 ≤5 ms p99 CO-corrected; `slot_freed_on_disconnect` soak.
- **M3 — Native-S2S full-duplex (the interaction model).** `InferProtocol`; GW-9, GW-13(UDS), GW-14(conditioning), **GW-18**. **Acceptance:** `/realtime` + DAG `realtime_provider[infer]` + LiveKit/SIP drive an Infer full-duplex model; **Full-Duplex-Bench** (interruption/backchannel/overlap) ≤200 ms; `ref_audio_fingerprint_no_crosstalk` once GW-14 lands.
- **M4 — Edge + discovery + tracing.** GW-6 (in-proc CPU/edge), GW-10, GW-17. **Acceptance:** Infer model in `/capabilities`+`/voices`; `infer-inproc` runs a CPU-tier model single-binary; Infer stage spans parent under the gateway profiler.
- **M5 — Fleet (standalone tier).** adapter→Infer router endpoint (D1); GW-3(failover), GW-11, GW-16, GW-13(datagram). **Acceptance:** the gateway fleet drives a standalone Infer tier via the router; cross-tier failover to cloud on `reject-reason` without breaker flap; cloned-voice prefix-affinity TTFA gain (Infer-router-owned); tenant labels honored by Infer VTC.

---

## 14. Critique / self-reflection (resolved + residual)

**Resolved by review + owner decisions:** GW-7 done; GW-8 rescoped *and* pulled to M1 (D3); no UDS in scaffold → GW-13 + loopback-ws (D5); gateway pool routing was wrong → removed, Infer owns routing (D6); keystone gate weaker than stated → GW-12 wire test; perf "sessions" → VUs; Infer ships a standalone denoiser; f32 seam → PCM16; fast-tier-on-Infer dropped (D2 — LLMs external).

**Residual tensions:**
1. **GW-1 structural refactor** (single-crate → provider-api crate; `STTResult` at ~33 sites). *Spike first; minimal scope + `From`/`Into` + CI parity test.*
2. **The S2S/clone seam is a forced fit** (GW-4/5/13/14 are the price of rich semantics through cloud-shaped traits). *Accepted; cascade stays the clean path.*
3. **Native full-duplex depends on Infer serving a Moshi-class model (GW-18).** The gold-standard experience is only as good as that model — if Infer doesn't yet have a strong full-duplex model, M3's interaction-model quality is gated on it. *Track model readiness; the cascade path (M1) is the fallback experience meanwhile.*
4. **Fleet routing now lives in Infer (D6).** This is correct ownership but means **the integration's distribution quality depends on `waav-infer-router`/`waav-infer-control` shipping** — they're designed (inferv2 §6) but unbuilt. *The gateway's M5 is blocked on Infer's router; M1–M4 (single-box/sidecar) are not.*
5. **Cross-tier failover semantics** must distinguish "this worker is busy" (router's problem, retried internally) from "the whole tier is down" (gateway fails over to cloud). *Only the latter is `reject-reason` at the router boundary; GW-3 keys on that.*
6. **Tracing/fairness are new contracts** (GW-17; gateway labels + Infer VTC). *Named, M4/M5.*

**Empirical residuals:** (a) NFR-P7 ≤5 ms (single-box) + router RTT budget (fleet); (b) Full-Duplex-Bench numbers for the chosen S2S model; (c) cascade sub-second first-audio with a co-located external fast LLM; (d) prefix-affinity TTFA gain via Infer's router; (e) GW-2 CPU-pin vs bandwidth reserve (single-box).

---

## 15. Owner decisions (D1–D6) — RESOLVED

| # | Decision | Resolution | Plan impact |
|---|---|---|---|
| **D1** | Sidecar vs standalone at 10s/100s servers | **Standalone Infer GPU tier behind its router** for fleet; sidecar for single-box/small; in-process for edge/CPU | §7 spectrum; M5 fleet milestone; adapter `mode=remote`→router |
| **D2** | Where the fast/reasoning LLM lives | **Outside Infer** (vLLM/cloud, via `LlmAdapter`); Infer hosts audio models only — *except* an S2S model with an intrinsic LLM | §1 scope boundary; §10 rewritten; "Infer-SLM fast tier" dropped |
| **D3** | Barge-in now vs later | **Now, in full, all paths**; native-S2S = Kyutai/TM interaction-model gold standard; Full-Duplex-Bench gate | GW-8→M1; new GW-18; §6.4; M1/M3 acceptance |
| **D4** | Gateway OpenAI facade | **No** — keep the clean split (ecosystem→Infer standalone) | §8.1 |
| **D5** | UDS now vs loopback-ws first | **Loopback-ws first**, UDS/datagram later (GW-13) | §6.2; GW-13 timing |
| **D6** | Who routes/queues/schedules | **The Inference server** (router + control plane); gateway addresses one router endpoint + cross-tier failover | §11.2/11.3; GW-15 removed; GW-16 reduced; §4 |

**No open decisions remain.** Residual items are *engineering* sequencing (the residual tensions, §14), not owner choices.

---

## 16. Revision log

**v2 → v3 (owner decisions D1–D6):** standalone fleet tier (D1, §7); Infer = audio-only, LLMs external (D2, §1/§10); full barge-in now + native full-duplex interaction model (D3, §6.4, GW-8→M1, +GW-18); keep the split (D4); loopback-ws first (D5); **Infer owns routing/queuing/prioritization/scheduling** (D6) → GW-15 removed, GW-16 reduced, §11 rewritten.

**v1 → v2 (adversarial review, 20 corrections):** keystone gate is a CI test not a build gate (§5.4); perf headline is VUs/bare-HTTP (§11.1); Infer ships a standalone `Enhancer` (§4); `transcribe` signature (§2.2); frame mapping added `Keepalive`/`Close` (§5.2); `~0.17 s` not ms (§10); GW-7 done; GW-8 cancel-token already exists; no UDS in the scaffold → GW-13; voice-clone conditioning → GW-14; alias is 1:1 (no pool); multi-tenant fairness owned by neither → split; remote-S2S media should be datagram; no OTel → GW-17; UDS auth contract; `BaseTTS` lacks `set_resilience`; cold-start routing; f32→PCM16 seam; LoRA voices; acceptance gates expanded.

---

### Appendix — primary code anchors

- **Seam:** `src/plugin/{registry,builtin/mod,macros,dispatch}.rs`; `core/stt/base.rs:597`, `core/tts/base.rs:269`, `core/realtime/base.rs:640`; `core/llm/adapter.rs:299`.
- **S2S scaffold:** `core/realtime/scaffold/{session,protocol,transport,event}.rs` (ConnectSpec/`apply_endpoint_override` event.rs:85,163); OpenAI GA `core/realtime/openai/config.rs:24`.
- **Front-of-pipeline / barge-in:** `core/silero_vad/detector.rs`, `core/smart_turn/*`, `core/turn_decision/engine.rs`, `core/turn/{controller,strategies/min_words}.rs`, `core/conversation/mod.rs:1118`, `utils/noise_filter.rs`, `core/voice_manager/manager.rs:521,822`.
- **Transport:** `src/livekit/*`, `src/handlers/sip/*`, `core/audio/{codec,opus_codec,resampler,output_chunker}.rs`, bridge `handlers/ws/config_handler.rs:2199-2308`.
- **DAG:** `dag/{context.rs:62,executor.rs:200,nodes/provider.rs:234,nodes/endpoint.rs:1095}`, `config_handler.rs:2877`.
- **Standardization / discovery:** `core/{lang,voice,emotion,alias}/` (alias 1:1 alias/mod.rs:159), `core/{stt,tts}/standard.rs`, `handlers/{capabilities,voices}.rs`, `middleware/request_id.rs:74`.
- **Infer:** `waav-infer-protocol/src/{ws,session,stt,tts,error}.rs`, `waav-infer-core/src/{model.rs:96,125,177,enhance.rs}`, `waav-infer-components/src/standardize.rs`, `waav-infer-server/src/{lib,ws,ingress}.rs`; **`waav-infer-router`/`waav-infer-control`** (inferv2 §6, the D6 owners).
- **Spec:** `WaaV/inferv2/INFER_SPEC.md` §4/§13/§14, FR-G/A/D/E10; `WaaV/inferv2/INFER_ENGINE_V2.md` §3 (transport/fairness), §6 (router/admission), §7 (perf), §17.2 (runtime isolation).
- **D3 experience grounding:** Thinking Machines *interaction models* (thinkingmachines.ai/blog/interaction-models); Kyutai Moshi/Unmute (kyutai.org); Full-Duplex-Bench (arXiv 2503.04721 / 2507.23159 / 2510.07838).
