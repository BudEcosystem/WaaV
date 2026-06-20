# WaaV Infer — Product Specification & System Architecture

**Version:** **v1.0 (r4) — CONVERGED + reuse-folded** (3 adversarial rounds: 76 → 48 → 17 findings, CONVERGED/SHIP; r4 folds in the build-vs-borrow sweep — 10-agent OSS reuse research, ~140 artifacts, catalogued in `WaaV/inferv2/INFER_REUSE.md`) · **Date:** 2026-06-13
**Grounding:** `research/learnings.md` + 28 research notes under `research/notes/` (18 architecture + 10 reuse; web research + direct source reading of 21+ cloned engine/model repos + the WaaV gateway). Every load-bearing claim is traceable; see Appendix D (corpus index). **Companion:** `WaaV/inferv2/INFER_REUSE.md` — the reuse catalog that this revision draws its §17.3 / §19 / FR-D deltas from.
**Method:** first-principles decomposition, KISS bias (boring, verified industry patterns; explicit non-goals), iterative multi-agent adversarial critique until convergence.

---

# Part I — Product

## 0. Executive summary

**WaaV Infer is a portable, production-grade inference engine for voice AI** — speech-to-text, text-to-speech, and (later) speech-to-speech — that serves the open voice-model ecosystem on any hardware, with pull-and-run UX. It is the self-hosted inference tier of the WaaV voice gateway, and also runs standalone.

One sentence of positioning, each clause mapped to a verified competitor gap:

> **Riva-class serving for open voice models, on any hardware, with ollama-class UX.**

- **Riva-class serving**: continuous batching of many concurrent realtime streams per GPU, first-audio latency published per rated (model × device × precision) triple (≤ 250 ms p90 typical on mid-range GPUs; ≤ 200 ms for fp8 flat-codec on H100-class — the NFR-P1 table is normative), measured-capacity admission control — the serving discipline NVIDIA Riva proved, without the NVIDIA-only lock or the $4,500/GPU/yr serving tax (Riva's models are free; NVIDIA prices the engine — that engine is the product WaaV Infer replaces).
- **Open voice models**: the SOTA OSS wave (Parakeet, Whisper, Kokoro, Orpheus, CosyVoice, Chatterbox, Moshi…) that Riva structurally cannot onboard (NeMo-only pipeline) and no DIY tool serves well. New model = **manifest + weights**, not code.
- **Any hardware**: NVIDIA CUDA (incl. GB10 Grace-Blackwell aarch64), AMD ROCm/MIGraphX, Qualcomm Hexagon, Intel CPU/GPU/NPU, Apple, and a guaranteed CPU-SIMD floor — via a two-path backend architecture (ONNX Runtime EPs + a native autoregressive path) behind one Rust trait.
- **ollama-class UX**: `waav pull kokoro && waav run` — content-addressed local store, device-aware quant resolution, curated signed catalog, OpenAI-compatible APIs for instant ecosystem adoption.

**Relationship to the WaaV gateway:** optional and bidirectionally clean. The gateway treats WaaV Infer as just another provider behind its existing `BaseSTT`/`BaseTTS`/`BaseRealtime` traits and DAG nodes — local inference gains the gateway's VAD, turn detection, noise reduction, barge-in, DAG pipelines, observability, and cloud-failover for free. WaaV Infer standalone serves its native WS protocol + OpenAI-compat APIs to any client (Pipecat, LiveKit Agents, OpenWebUI, plain SDKs). Topology (in-process / sidecar / remote) is **configuration, not code**.

**The three headline product guarantees:**
1. **Realtime SLO by construction** — admission control from calibration tables measured **on the operator's actual device** (produced during model warm-up, §8.3b); reject-don't-degrade; streaming-viability + per-(model × device × precision) rated TTFA as normative SLOs (FR-S group). Typical rated first-audio: ≤ 250 ms p90 for 25 Hz-class AR-TTS on L4-class GPUs and for flat-codec AR-TTS on L40S-class; ≤ 100 ms for one-shot TTS on GPU. (Honest arithmetic per model/device in §8.6 — "sub-200 ms everywhere" is a number competitors quote from H100s; we publish the table instead.)
2. **Crash containment by topology** — default sidecar deployment; an inference crash (CUDA sticky error, native abort) never kills the gateway or any non-local session (NFR-R group).
3. **Model onboarding as data** — the signed manifest (execution path + component refs + per-target quant artifacts + codec-LM serving metadata) is the sole unit of onboarding; "supported model" is defined as "passes the porter-kit verify gate" (FR-M group).

## 1. Problem & market

### 1.1 The fragmentation problem, from first principles

Voice-AI inference is fragmented along three orthogonal axes, and every existing tool collapses two while leaving the third broken:

| Axis | Fragmentation today |
|---|---|
| **Model architecture** | Whisper enc-dec, Conformer-RNNT/TDT/CTC, codec-LM AR, flow-matching, VITS one-shot, continuous-latent AR, duplex S2S — each ships its own repo, its own `inference.py`, its own engine assumptions (Orpheus is literally vLLM-only). |
| **Hardware** | Each model "works" on the one or two devices its authors had. CUDA x86 ≠ CUDA aarch64 (on GB10, ORT and llama.cpp ship **no prebuilt GPU binaries** — source builds required; PyTorch only recently gained sbsa wheels). ROCm, Hexagon, Intel are afterthoughts. CPU SIMD potential (NEON/SVE/AVX) is mostly unexploited. |
| **Serving** | One-shot demo scripts vs. the real need: many concurrent, persistent, hard-realtime streams. The DIY tier has **no continuous batching**, documented VRAM-lifecycle races (ollama #9926, llama.cpp #20137, LocalAI #7269, Kokoro-FastAPI #453), cold-start stalls, and mostly no streaming. |

First-principles reduction (from `learnings.md` §2): strip the labels and every voice model is
`(optional encoder) → generator (AR | iterative | one-shot) → intermediate (codec tokens | mel/latent | none) → waveform decoder`.
The *generator shape* dictates the serving machinery. There are ~6 execution paths and ~8 shared components. **An engine that implements those once, portably, covers ~90% of models** — that is the entire architectural bet, and it is grounded in code reading, not marketing (`research/notes/code_models.md`).

### 1.2 Competitive landscape (verified June 2026; full analysis: `spec_competitive.md`)

| | **NVIDIA Riva / NIM** | **DIY tier** (Speaches, Kokoro-FastAPI, whisper.cpp server, LocalAI, sherpa-onnx) | **Cloud APIs** (Deepgram, ElevenLabs, …) | **WaaV Infer** |
|---|---|---|---|---|
| Serving quality | Excellent (Triton+TRT: ASR 12–34 ms @64 streams H100; TTS first-chunk ~70 ms; 192× RTFX) | Poor: no continuous batching, no admission control, VRAM races | Excellent | Riva-class targets (§6) |
| Model coverage | NeMo-only (nemo2riva→riva-build); SOTA OSS wave unreachable | Per-tool, narrow (faster-whisper+Kokoro typical) | Vendor models only | Open ecosystem via manifests |
| Hardware | NVIDIA Ampere+ only, ≥16 GB VRAM; GB10 "limited models" | Mostly CUDA/CPU lottery | n/a | CUDA/ROCm/Hexagon/Intel/Apple/CPU floor |
| Ops | One model per container; up to 30-min first boot | Documented lifecycle bugs | n/a | Multi-model one server; seconds-scale cold start (NFR-P5) |
| Cost | $4,500/GPU/yr (NVAIE) or ~$1–2/GPU/hr | Free, fragile | $0.0048/min STT (Nova-3), ~$0.03/min TTS — five figures/month at scale | OSS engine; one L4-class GPU displaces that bill |
| UX | Enterprise container ritual | Per-tool | API key | `waav pull` / OpenAI-compat / gateway provider |

**Demand evidence:** ollama (173.9k★) has explicitly declined voice since 2024 (#5424); Pipecat lists ~20 cloud STT vendors but exactly **one** local STT; LiveKit's self-hosted story is "point the Riva plugin at your own server" (the local-speech-server plugin category exists with one occupant); HN practitioner threads (Jan 2026): *"I can't find a single reproducible 'here's how I got open weights doing real speech-to-speech locally' writeup."* Kyutai's moshi-server is the existence proof for the architecture (Rust+candle; **batched DSM-ASR at 64 streams × 3× realtime on one L40S, 400 realtime STT streams on one H100** — STT figures, not duplex) but is scoped to Kyutai models only.

### 1.3 Why now
(a) The open-model wave reached production quality (Parakeet beats commercial ASR on benchmarks; Kokoro/Orpheus/CosyVoice2-class TTS is good enough for products) while remaining unserved. (b) Serving techniques (continuous/static batched scheduling, slot-quota KV, calibrated admission) are now well-understood, published, and KISS-implementable for the small-model voice regime. (c) Privacy/compliance/cost pressure makes self-hosted voice a procurement requirement, not a hobby.

## 2. Product definition

### 2.1 What WaaV Infer IS
1. A **voice inference engine** (Rust): loads open STT/TTS/(S2S) models from signed manifests and serves them with hard-realtime scheduling on heterogeneous hardware.
2. A **WaaV gateway provider**: implements the gateway's `BaseSTT`/`BaseTTS`(/`BaseRealtime` later) provider contracts via a single adapter crate, in any topology.
3. A **standalone server** (`waav-infer serve`): native WS protocol + OpenAI-compatible REST/WS + control plane, consumable by Pipecat/LiveKit/any OpenAI SDK.
4. A **model distribution toolchain**: `waav pull/run/list/show/rm`, content-addressed store, curated signed catalog, `waav onboard` porter kit.

### 2.2 What WaaV Infer is NOT (explicit non-goals, with reasons)
| Non-goal | Reason / who owns it |
|---|---|
| Agent/pipeline orchestration, VAD, turn detection, noise reduction, barge-in policy | The gateway (and Pipecat/LiveKit) own orchestration; Infer **consumes** gateway VAD/turn signals, never re-implements them. |
| Transport (WebRTC, SIP, telephony, LiveKit rooms) | Gateway/LiveKit lane. Infer speaks sessions over UDS/WS only. |
| Training, fine-tuning, voice-cloning *studios* | NeMo/TAO/vendor lane. Infer serves conditioning (speaker refs) at inference only. |
| NMT/NLP bundling | Riva's bloat; DAG composes external nodes instead. |
| Hosted SaaS, billing, tenancy | Gateway / business layer. Infer has API keys + quotas only. |
| A model *hosting* service or blob registry server | HF Hub + org mirrors + static signed catalog index (ADR-7). |
| Live mid-utterance session migration across processes | No prior art (vLLM/Triton don't); zero-downtime = rolling overlap + gateway failover (ADR-10). |
| Speaker diarization | v1 non-goal; composable later via DAG (pyannote-class node). Roadmap candidate, not engine core. |
| Punctuation/ITN as a separate stage | Model-native only in v1 (parakeet/nemotron emit punctuation; whisper emits formatted text). A standalone punctuation model is a later catalog component, not engine code. |
| LLM serving in general | Text LLMs route to vLLM/llama.cpp/cloud via the gateway's existing LLM adapter; Infer's AR machinery exists for *audio-token* models. (LLM-based ASR Path C is in scope *later*, §11.) |

### 2.3 Design principles (normative)
P-1 **First principles**: serve generator *shapes*, not model brands.
P-2 **KISS**: the simplest mechanism with production precedent wins; every piece of speculative machinery needs a named trigger to exist (see ADRs).
P-3 **Measured, never modeled**: capacity, readiness, and admission decisions read calibration tables produced on the actual device (Clockwork doctrine).
P-4 **Reject, don't degrade**: past admission capacity, new work is refused fast (429/503/typed error) and the gateway fails over; admitted streams keep their SLO.
P-5 **Crash containment is a topology property**, not a library property (`catch_unwind` cannot catch `SIGABRT`; CUDA sticky errors are process-fatal) — hence sidecar default.
P-6 **CPU floor invariant (functional)**: every catalog model has a CPU-runnable artifact row that passes the porter-kit correctness gates — every model *works* on CPU (development, testing, Batch-class offline jobs). **Realtime rating is a separate, per-device property from calibration**: small models (P3 TTS, STT-A, whisper-int8 single-stream) are realtime-rated on 8-core CPUs; AR-TTS generally is not (the §8.6 bandwidth arithmetic applies to CPUs too). `waav show` and admission surface "not realtime-rated on this device" explicitly — the floor is never silently re-marketed as realtime.
P-7 **Models are data**: manifest + weights; engine code changes only for genuinely new execution paths or components.
P-8 **No backend-specific code outside its adapter crate** (generalizes the gateway's proven `onnx/mod.rs` single-policy-point invariant).
P-9 **Boring observability**: extend the gateway's `waav_*` Prometheus + monotonic-ns profiler conventions; never invent parallel ones.

## 3. Users & use cases

| Persona | Scenario | What they need from Infer |
|---|---|---|
| **U1. WaaV gateway operator going self-hosted** | Compliance/cost: replace Deepgram/ElevenLabs with on-prem for some/all traffic | Drop-in provider (`provider: waav-infer`), gateway features intact (VAD/turn/DAG/barge-in), cloud failover when Infer rejects/down |
| **U2. Voice-agent developer (Pipecat/LiveKit/own stack)** | Wants local STT/TTS without the DIY tier's fragility | OpenAI-compat + native WS; `waav pull`; one server, N models; published capacity numbers |
| **U3. Platform/SRE team** | Runs a fleet; needs predictability | Admission control, drain semantics, metrics/runbooks, rolling upgrades, calibration artifacts |
| **U4. Edge/embedded integrator** | Kiosk, robot, car, phone-adjacent box; maybe no GPU, maybe Hexagon | In-process library mode (CPU/ORT tier), small binaries, offline store, Hexagon path on the roadmap |
| **U5. Model author/porter** | Wants their model served | `waav onboard` porter kit; verify gate; catalog PR; no engine-code fork |

Primary v1 target: **U1 + U2 on a single GPU node (GB10-class, L4/L40S-class, or 4090-class) and CPU-only nodes.** U4 Hexagon and U5 GA tooling are roadmap (§19).

## 4. Deployment topologies & gateway integration

(Decision analysis: `spec_deployment.md`. Latency *cannot* pick the topology — UDS RTT ≈ 2.3 µs, local gRPC ≈ 100–300 µs, shm ≈ 100 ns, all noise against 20 ms audio frames and a 200 ms TTFA budget. **Crash physics and operations pick it**: CUDA sticky errors corrupt the context and are recoverable only by process death (NVIDIA-confirmed); `GGML_ASSERT` → `abort()` is uncatchable in-process. Every production system — vLLM V1 (API⇄EngineCore over ZMQ), TGI (router⇄shards), ollama (per-model runners), Riva (gRPC sidecar), LiveKit Agents (shared inference process) — runs inference out-of-process.)

### 4.1 The three topologies (all supported; one wire protocol; topology = config)

```
T1 — SIDECAR (DEFAULT)                     T2 — IN-PROCESS (edge/CPU tier)        T3 — REMOTE
┌────────────── host ──────────────┐       ┌──────────── host ───────────┐        ┌─ host A ─┐   ┌─ host B ──┐
│ ┌─────────┐ UDS  ┌────────────┐  │       │ ┌─────────────────────────┐ │        │ gateway  │ WS │ waav-infer │
│ │ gateway │◄────►│ waav-infer │  │       │ │ gateway + infer-inproc  │ │        │          │◄──►│  serve     │
│ │         │      │  serve     │  │       │ │ (one binary, lib link)  │ │        └──────────┘TLS └───────────┘
│ └─────────┘      └────────────┘  │       │ └─────────────────────────┘ │
│  supervises: spawn/restart       │       │  CPU/ORT-tier models only   │
└───────────────────────────────────┘       └─────────────────────────────┘
```

- **T1 Sidecar (default).** `waav-infer serve` on the same host, gateway-supervised (spawn, health, restart) or systemd-managed — both first-class. Transport: UDS with the native protocol (§13.2). **SLO: a sidecar crash or upgrade MUST NOT terminate the gateway or any session not bound to local inference; bound sessions fail as a provider error handled by the gateway's existing CircuitBreaker + ReconnectGovernor (W-D1/W-D2) — indistinguishable from a cloud-provider WebSocket drop.** Zero new gateway resilience concepts *on the crash path* (busy/rejection classification is delta GW-3, §14.1).
- **T2 In-process** behind cargo feature `infer-inproc`, with a written warning label: *isolation is a topology property — CUDA sticky errors and native aborts are uncatchable in-process.* Recommended only for: CPU/ORT-tier paths (Kokoro/Piper/VAD-class), embedded single-binary edge deployments, dev/test. (Prior art: Triton's `libtritonserver.so` C API positioned for Jetson; llama.cpp lib; sherpa-onnx.)
- **T3 Remote**: same wire protocol over TCP/WS + TLS + API key. Same binary. For datacenter pools and the LiveKit/Pipecat standalone audience.

### 4.2 GPU ownership & co-residency
- The Infer process **owns its device(s)** with an explicit startup-resolved memory budget (fraction of device or unified memory; counted in one ledger on GB10 — §9). Allocation happens at load/admission time, never on the request path.
- **Requirement (not current behavior):** when co-resident with Infer, gateway aux models (Silero VAD ~2 MB, Smart Turn) MUST be pinned to CPU EPs. Today the gateway's `WAAV_ORT_EP=auto` probes **CUDA first on Linux** (`onnx/mod.rs auto_probe_order()`), so on the default T1 topology the gateway would land its VAD on the GPU and open a second CUDA context. The co-residency profile therefore sets `WAAV_ORT_EP=cpu` (or a per-model EP override, a small gateway addition) — this is named **gateway delta GW-2** (§14.1) and M1 integration work. Alternatively the Infer ledger subtracts a measured gateway-GPU reserve; the CPU pin is the default because the models are tiny and the decoupling is worth more than the speedup.
- **MPS off by default** (a fatal fault in one MPS client kills all clients — documented); plain time-slicing isolates processes (a sticky fault in one process does not poison others). MIG documented for datacenter T3 only. ~300–550 MB CUDA context per process is the co-residency tax (moot on GB10's 128 GB).

### 4.2b Supervised-sidecar lifecycle contract (T1, normative)
The gateway-supervised mode needs more than "spawn and watch" — these rules close the 3am gaps:
1. **Spawn & adopt**: the supervisor writes/reads a pidfile + instance-id; the UDS handshake returns `{engine_version, instance_id, manifest_digests}`. On gateway (re)start, a live healthy sidecar is **adopted, never duplicated** (two engine processes would double-book the device ledger and OOM each other's loads). Stale sockets are unlinked. **Parent-death detection is portable and thread-safe**: the child inherits the read end of a pipe held open by the gateway for its whole lifetime — pipe-EOF ⇒ parent death ⇒ child initiates drain-then-exit (works on Linux + macOS, immune to `PR_SET_PDEATHSIG`'s *thread*-death semantics; PDEATHSIG=SIGTERM may be set additionally on Linux, from a process-lifetime supervisor thread only). **On k8s, gateway-spawn does not apply**: the pattern is two containers, kubelet-supervised, provider in **adopt/connect-only mode** (Appendix F). **systemd is the supervisor of record for production bare-metal** (gateway supervision is the dev/convenience mode).
2. **Restart policy keys on death, not readiness**: restart triggers are process exit, `/livez` failure, or deep-probe-confirmed-dead — **never `/readyz`** (which is legitimately false during Loading/Warming and drain; see FR-O1). Spawn gets a startup budget sized from `waav_infer_model_load_seconds` before any health judgment.
3. **Flap damping**: escalating restart backoff with a ceiling, plus a crash-loop budget — K failures within T (default 3 in 10 min) ⇒ provider quarantined (long cooldown + page) instead of an all-night kill-and-readmit cycle (the GB10 hung-kernel failure mode, vLLM #41725, is *recurring* — design for it). Post-restart **probation**: the provider re-advertises availability only after a minimum stable window + one deep-probe pass; re-admission is jittered to avoid thundering-herd re-binds. (systemd analog: `StartLimitBurst` documented for operator-managed mode.)
4. **Shutdown ordering — honest about who bounds whom**: in gateway-supervised T1, the sidecar's *effective* drain budget is `min(drain.deadline, supervisor-remaining-lifetime)`. The gateway today cancels its WebSocket session loops immediately on SIGTERM and bounds HTTP drain at 30 s (`main.rs` RC6 + `graceful_shutdown(30s)`) — so gateway-bound sidecar sessions die with their gateway sessions, and a 600 s sidecar drain is **only reachable under systemd-managed/standalone/T3 operation** (where non-gateway clients exist). The supervised-mode contract is therefore: gateway signals sidecar drain first, waits up to its own configured grace for sidecar exit, then exits (pipe-EOF backstops). A gateway-side long voice-session drain is a **named gateway roadmap item (GW-8 candidate)**, not assumed by this spec. Appendix F's `terminationGracePeriodSeconds` arithmetic applies to the systemd/standalone profiles where the 600 s budget is real.

### 4.3 Integration seams (existing seams + a short, named list of gateway deltas — §14.1)
| Gateway seam | Used how |
|---|---|
| `BaseSTT` / `BaseTTS` traits (`core/{stt,tts}/base.rs`) | One adapter crate `waav-infer-provider` implements both, with `mode = in_process \| sidecar \| remote` resolved from provider config. Registered as `"waav-infer"` via `inventory::submit!`. **Build reality (was a dependency cycle in r1):** the traits + `PluginConstructor` live inside the `waav-gateway` package, so an external crate registering via inventory would need gateway→provider→gateway. Resolution (ADR-16): extract **`waav-gateway-provider-api`** (traits, `STTResult`/configs, `PluginConstructor`) as a small crate both sides depend on; the gateway binary gains an optional `provider-waav-infer` feature. This is gateway delta **GW-1**, M1 scope. |
| `BaseRealtime` (`core/realtime/base.rs`) | S2S phase (§19 M5): duplex models surface as a realtime provider (24 kHz PCM16, `create_response`/`cancel_response` map to engine session verbs). |
| DAG `IpcEndpointNode`/`GrpcEndpointNode` + custom nodes | Infer models compose in DAG pipelines like any provider; an `InferNode` is sugar over the provider adapter. |
| EP policy (`core/onnx/mod.rs`) | Lifted verbatim into the engine's Path A and **widened** (OpenVINO/MIGraphX/QNN); same env contract style, same degrade-to-CPU + `waav_degraded_total` discipline. |
| Resilience (CircuitBreaker, ReconnectGovernor), profiler, `waav_*` metrics, `/livez`/`/readyz`, RC6 drain token | Reused/extended as-is (§15). |

---

## 5. Functional requirements

Conventions: **MUST/SHOULD/MAY** normative. Every FR is testable; acceptance lives in §18/§19. Grouped: E=engine, S=scheduling/SLO, M=models/packaging, A=API, G=gateway, O=ops, D=audio/data.

### FR-E — Engine core
- **FR-E1** The engine MUST execute models described by manifests across the execution paths: STT-A (frame-synchronous CTC/transducer), STT-B (attention enc-dec, Whisper-class), TTS-P3 (one-shot), TTS-P1 (codec-LM AR; flat-SNAC and depth/RQ codebook layouts), TTS-P1+P2 (AR semantic tokens → flow-matching → vocoder), with STT-C (LLM-ASR), TTS-P2 (pure diffusion/flow), TTS-P4 (continuous-latent AR) and S2S-duplex staged per the roadmap (§19).
- **FR-E2** The engine MUST implement the staged-engine pattern: each model instance is a DAG of stages (e.g. AR-decode → codec-decode), each stage a dedicated scheduler over typed bounded queues; stage archetypes: AR-batch, micro-batch, streaming-vocoder.
- **FR-E3** Sessions are first-class: persistent, stateful, streaming; a session binds to a model instance at admission and never migrates (ADR-10).
- **FR-E4** Cancellation (barge-in) MUST be a queue-jumping control message reaching every stage; AR stages MUST check it per step; effect within ≤ 1 stage tick; freed resources (KV slot, codec window, queue entries) MUST return to pools immediately. Wired to gateway `BaseTTS::clear()`/`flush()` semantics and the native protocol's `clear`→`cleared` ack.
- **FR-E5** The engine MUST run fully offline (no network) once models are pulled.
- **FR-E6** Audio canon: internal 16 kHz mono PCM16 ingress for STT-class, 24 kHz PCM16 default egress for TTS-class; resampling only at edges; every TTS chunk carries `sample_rate` + `format` (gateway `playback_ms` math depends on it).
- **FR-E7** Shared components (DSP/STFT-mel frontend, codec decoders SNAC/Mimi/DAC, vocoders Vocos/HiFTNet/iSTFTNet/BigVGAN-flagged, Euler-CFM solver + DiT hook, tokenizer runtime, AR backbone + KV cache, CAMPPlus speaker encoder, multi-codebook glue) MUST be engine-owned libraries referenced by manifests — never duplicated per model. **Codec/speech-tokenizer *encoders* are in scope as conditioning-prep components** (required for zero-shot cloning, FR-E10); "decoder-only" is a streaming-edge optimization (the decode hot path), not a product constraint.
- **FR-E8** **Text segmenter** (engine-owned): TTS input is segmented (sentence/clause boundaries, max-length caps) before synthesis. P3 one-shot models synthesize per segment (this is what makes P3 *streaming* and gives it a TTFA); AR models roll long inputs across segments with per-segment AR windows (KV quota applies per segment; context carry per model capability). TTFA targets are defined on the **first segment**; the first emitted segment is **duration-capped** (default ≈ 1.4 s estimated audio ≈ 20 chars at ~14 chars/s, when a clause boundary permits; subsequent segments up to 120 chars unless model-capped lower) — this cap is what makes CPU-tier P3 TTFA honest (NFR-P2). Caps are **token-budget-aware, not pure char-count** (some models artifact near their phoneme-token limit even below the char cap — Kokoro-FastAPI's 175/250/450 budget is the production reference). **Reuse:** `icu_segmenter` (ICU4X, UAX-29 + CJK/Thai dictionary word-breaking) + `srx` (LanguageTool abbreviation rules — fixes the splits-on-"Dr."/"e.g."/"3.14" bug); `unicode-segmentation` floor. Text normalization ownership: the segmenter performs minimal universal normalization (`num2words`); full TN/ITN (numbers/dates/currency) is the **`wetext-rs` + `rustfst`** pure-Rust WFST component (consuming NeMo/WeText FAR grammars as data — zero C/C++, §17.1); phoneme-path frontends carry it; raw-text codec-LMs handle it model-internally.
- **FR-E9** **Audio codec policy**: the engine core is PCM-only (FR-E6). Compressed-format transcoding lives at the **server/adapter edge**: ingress decode (multipart transcriptions uploads: wav/mp3/flac/ogg/vorbis via **Symphonia**, pure Rust; **opus ingress via the `opus` crate (libopus) + `ogg`** — *Symphonia has no Opus decoder as of 0.6, so opus does not ride Symphonia*; **WAV µ-law/A-law payloads** decoded with the G.711 crate after `hound` parses the header) and egress encode (FR-A3 `response_format`: wav/pcm native; **opus** via the same libopus link; **flac** pure-Rust; **mp3** behind an `egress-mp3` feature linking LAME (LGPL, dynamically linked, enabled in official binaries with notice); **aac unsupported** → 400 + machine-readable divergence doc). One libopus link serves opus ingress AND egress. When `response_format` is omitted on the compat surface, default = mp3 if the feature is present else wav (documented divergence). Reuse: `audio-codec-algorithms` (G.711, 0BSD), `opus` crate (MIT/BSD), Symphonia (MPL), hound (Apache). This resolves the r1 FR-D1↔FR-A3 contradiction: core stays PCM, the edge owns codecs.
- **FR-E10** **Zero-shot voice cloning** (models that support it: cosyvoice2, chatterbox): conditioning modes are {`voice-bank`, `speaker-embedding` (CAMPPlus), `prompt-audio` in-context}. Reference audio enters via (a) per-session `conditioning.reference_audio` (PCM/wav, length-capped) or (b) a control-plane **voice registration** verb (`POST /v1/voices`: encode once → cached conditioning blob → named voice). Conditioning-prep cost (encoder pass) is accounted at admission; prepared conditioning is cached (SpeakerEmbeddingCache, §9.3) under the biometric-data rules of §16. Cloning is policy-gated per deployment (`conditioning.cloning = enabled | builtin_voices_only`, default `builtin_voices_only` for the public-facing server, `enabled` for gateway deployments). **Voice-bank style mixing** (interpolating style vectors, e.g. `af_sky.4+af_nicole.5` — Kokoros-proven) is a cheap voice-bank superset, exposed when the model supports it (manifest capability). The CAMPPlus speaker encoder + `SpeakerEmbeddingManager` pattern is reused from sherpa-onnx (Apache); CAMPPlus is dual-use for cloning AND the roadmap diarization node.

### FR-S — Scheduling & realtime SLO
- **FR-S1** Normative SLOs per realtime session: **streaming viability** (chunk *i+1* delivered before chunk *i* playout ends; reported as % chunks on-time) and **TTFA/TTFT p90 ≤ the session's rated (model × device × precision) budget** per the NFR-P1 table and local calibration — there is deliberately no blanket number; the per-triple table is the requirement. If FR-M6 rejects all ramp windows for a family, rated TTFA is re-derived at the steady window via the §18.1 lint formula and the published anchors/rated rows move with it — a quality-gate failure moves the number; it never silently fails a milestone or ships degraded audio.
- **FR-S2** The AR-stage scheduler v1 MUST be the **fixed-slot masked static batch** (moshi-server pattern): slot table per (stage, model), one calibrated static batch shape, admission = free slot + calibration check, O(1) abort, synchronous tick loop. **CUDA graphs are an optimization with a mandatory eager fallback** — warmup captures a graph where the backend supports it, otherwise records eager T_step; admission uses whichever was measured. (Note: moshi-server hits its 400-stream numbers *without* graphs; graphs buy headroom for high-steps/s models, they are not load-bearing for correctness.) (P1 evolution: buffer-depletion EDF + watermarks, §8.4.)
- **FR-S3** Admission control MUST consult only calibration tables **measured on this device** (§8.3b; manifest numbers are priors, never admission inputs). Admission is a **two-level test**: (i) per-(model, stage): free slot ∧ KV quota + codec window + workspace reservable ∧ streams < locally-calibrated max; (ii) **per-device (§8.3c): the summed realtime duty of ALL colocated stages across ALL models stays ≤ S** — individually-feasible work that is jointly infeasible is rejected at admission, not shed later. No local table yet ⇒ conservative B_max=1 admission until Warming completes calibration, or typed refusal (config choice). Over capacity → typed rejection (429 quota / 503 saturation + Retry-After; typed `AdmissionRejected` in-process). **Never** admit-and-degrade.
- **FR-S3b** **Calibration lifecycle**: (a) T_step(B) per bucket is measured during Warming **on the operator's device, against the configured warm set** (co-located stages running synthetic co-load), persisted with the device-artifact cache keyed (artifact sha256 × device id × SDK/driver version, **+ warm-set hash**) — driver/SDK key change ⇒ recalibrate at next Warming; **warm-set change ⇒ co-resident tables are stale-but-usable with a stated co-tenancy derate until recalibrated at next idle/Warming** (warm-set changes are admin-rate events); (b) at runtime the scheduler keeps an **EWMA + p99 of T_step per tick**; **drift response is debounced and damped**: requires a sustained breach (default: p99 > S·T_frame for ≥ 3 consecutive ticks AND ≥ 5 s), then shrink effective rated max (stop admitting) → shed Batch → only then shed newest Realtime streams **at a bounded rate (≤ 1 stream per tick interval)** with typed retriable errors (**shed-don't-smear**); capacity re-expands only after a stable window (hysteresis, default 60 s) — a single GC pause or PCIe hiccup must not shed paying streams; emits `waav_degraded_total{component="calibration",reason="t_step_drift"}`; (c) **during a co-resident model's Loading/Warming window the drift response is capped at shrink + shed-Batch** — Realtime shedding is suppressed unless real underruns/deadline-misses are observed (the engine knows the transient is local and bounded; reason=`coresident_warming`), and warmup/calibration kernels are **duty-cycle-paced** whenever other models hold live realtime streams (Warming may take longer under load; NFR-P5 bounds assume an idle device); (d) `waav calibrate` runs a full rating pass on demand; the porter-kit manifest `rated` rows are reference-class documentation only.
- **FR-S4** Two scheduling classes from v1: `Realtime` > `Batch`. Batch (offline transcription/synthesis) fills leftover per-tick token budget (Sarathi-style piggyback) and is the shed class under pressure.
- **FR-S5** Every queue in the system MUST be bounded; saturation behavior is defined (reject/shed), never unbounded growth.
- **FR-S6** Per-stream pacing: generation runs ahead of realtime into a bounded buffer (high watermark ≈ 2 s, parks the stream; low watermark / safety margin 250–400 ms triggers scheduling priority). The high watermark bounds barge-in waste (≤ H seconds discarded).
- **FR-S7** Mandatory scheduler telemetry: per-stage T_step histograms, buffer min levels, deadline-miss counters, admission rejections by reason, batch occupancy (§15.4).

### FR-M — Models, manifests, packaging
- **FR-M1** Manifest schema v1 (TOML authored; canonical-JSON hashed & signed) with REQUIRED fields: `id`, `version`, `family`, `task`, `exec_path`, a **`[license]` block** (`spdx` expression + `upstream_license` for derivatives + required `attribution` text + `notice_files` + `modifications` statement + `use_restrictions` — a bare SPDX string cannot carry real obligations, see §16), `languages`, `steps_per_second` (codec/frame rate — capacity-critical), component refs, conditioning modes (incl. cloning support flags), capability flags (biasing, alignment events, language auto-detect), voice catalog (per-voice license/preview/default), codec-LM serving metadata (`chat_template`, `audio_token_offset`, `frame_layout` ∈ {snac-flat-7, mimi-depth-8, dac-depth, single-fsq, mtp}, stop conditions), **flow/CFM solver metadata** (`n_timesteps`, `cfg_rate`, `t_scheduler` ∈ {linear, cosine, sway, log-norm}, `meanflow` bool, `cfg_zero_star` bool — so a distilled few-step model (~2-step MeanFlow) or a sway-scheduled model "just works" without engine changes; these are the cheap latency/quality levers found in Chatterbox/VoxCPM), streaming state contract, resource block (reference-class calibration priors), and a per-target artifact table (component × format × precision × target, every row sha256-pinned, **≥ 1 row targeting `cpu/any`** — P-6). Artifact rows carry a **guarantee class**: `functional` (correctness + Batch-class, porter-verified) vs `realtime-rated` (calibrated streams ≥ 1 on a named device class).
- **FR-M2** Local store: content-addressed `blobs/sha256-*` + per-tag manifests (ollama mechanics); digest verification on pull AND load; `waav export/import` tarballs for air-gapped.
- **FR-M3** CLI: `waav pull|run|list|show|rm|onboard`; unsuffixed tags resolve device-appropriately via the compute-type ladder (§10.4); explicit tags (`kokoro:1.0-int8`) and `hf://repo[@rev][:quant]` refs work; `waav show` MUST print license, watermark flag, voices, resolved artifacts.
- **FR-M4** Distribution: portable encodings only (ONNX fp32/fp16/int8-QDQ; GGUF Q8_0/Q4_K_M, plus **Q4_0/MXFP4 rows for `target = hexagon`** — the HTP backend offloads only those); safetensors. Device-locked artifacts (TRT engines, QNN context binaries, CoreML bundles, CUDA-graph captures, autotune results) **and local calibration tables** are generated on first load and cached keyed by (artifact sha256 × device id × SDK/driver version), with stated invalidation rules. Engine builds MUST be off the **session-critical path**, defined as: a fresh device serves immediately on the portable artifact + generic EP, builds the optimized artifact on a background lane, and hot-swaps at a session boundary — first-ever boot is allowed to be slower than NFR-P5's *warm* cold-start (the separate first-boot bound is NFR-P5b).
- **FR-M5** Supply chain: OMS/sigstore signing of catalog manifests (keyless CI) with **offline verification designed in**: export tarballs embed sigstore bundles (cert + Rekor inclusion proof); the CLI ships a pinned trusted root with a documented refresh mechanism and a stated max-staleness policy for air-gapped fleets (explicit, logged override); verification **pins issuer + CI-workflow identity** (configurable for org-internal catalogs); air-gapped orgs may **re-sign with their own PKI**; default fail-closed, `--insecure-skip-verify` loud and audited. Pull-time license policy evaluates the **full `[license]` obligation chain**, not the bare SPDX tag (permissive allowlist default; NC requires `--accept-license noncommercial`; GPL components isolated per §16). `provenance.watermark` is a **two-field truth**: `{upstream_declared, applied_by_engine}` — the engine never asserts a watermark it did not apply (§16). `waav.lock` for reproducible fleet deploys. **`model_policy = signed_only`** server config (recommended for production): unsigned `hf://` pulls require `--allow-unsigned`, are labeled `provenance: unsigned` in store + `/v1/models`, and are digest-pinned at first pull. **Verification timing (normative): signatures + trust root are verified at pull/import time only**; the store records the result + sigstore bundle as provenance; **load enforces digest integrity + the recorded provenance label — trust-root staleness never blocks loading an already-verified stored model** (it blocks new pulls/imports). Periodic re-attestation is an explicit admin verb (`waav verify --store`), decoupled from the serving path — so signed_only + air-gap + auto-reload cannot compose into a self-bricking fleet.
- **FR-M6** `waav onboard` porter kit: `init` (sniff + scaffold manifest) / `convert` (pinned-container recipes per family) / `verify` / `publish`. **Verify gate defines "supported"**: STT WER delta ≤ 0.5 abs vs reference on golden set; TTS speaker-sim + UTMOS-proxy deltas within stated bounds; chunked-vs-full **streaming-equivalence** test; per-quant re-verification; RTF/TTFA/steps_per_second recorded into the manifest resource block.
- **FR-M7** Model lifecycle: states `Loading → Warming → Ready ⇄ Degraded → Draining → Failed` (no `Paused` in v1 — it had no named trigger and contradicted required-model auto-reload; a vLLM-sleep-style state is a recorded future trigger in ADR-12's family); EXPLICIT admin load/unload; ollama-style `keep_alive` TTL (duration | 0 | -1=pin) with voice-appropriate default (30–60 min). **Interlocks — labeled (FR-M7a–d):**
  - **FR-M7a** `required` models (= the configured warm set / provider-configured models) are implicitly pinned (`keep_alive = -1`); TTL/LRU apply only to non-required models.
  - **FR-M7b** If a required model is found non-Ready, an **automatic background reload** is armed (admin-equivalent, async) — **bounded**: same escalating backoff + crash-loop budget as §4.2b(3) (K attempts in T ⇒ `Failed` terminal + page); suppressed during version swap and admin-intent unload; alerts on first arming. The "no cold load mid-call" rule governs *sessions waiting on loads*, not reloads.
  - **FR-M7c** Eviction safety: unload only at **zero live sessions ∧ zero in-flight admissions**, via Draining — **bounded by `drain.deadline`**: idle sessions past `session.idle_timeout` are closed; at the deadline remaining sessions are hard-cancelled with a typed error and eviction proceeds (one keepalive-zombie must not pin a model's ledger forever). Admission re-validates Ready at slot-bind and returns a retriable rejection if it lost the race.
  - **FR-M7d** A session-open against a non-Ready, non-required model MAY trigger an async load while rejecting with `retry_after ≈ load estimate` — it never blocks waiting.
  Version swap = load-new → warm → route-new → drain-old (Triton semantics; failed reload leaves old serving) — **requires transient ledger headroom for the new version's footprint; without it the swap degrades to drain-old-first with a brief declared unavailability of that model** (stated, not silent).
- **FR-M8** Curated catalog v1, with milestone and full-obligation license columns ("v1" = the M3 GA set; per-entry obligations audited, not just SPDX tags):
  | Model | Task/path | License (obligations) | Milestone |
  |---|---|---|---|
  | `parakeet-tdt-0.6b-v3` | STT-A; **batch/offline default** (RTFx ~3300); streaming via documented chunked config (2 s cadence) | CC-BY-4.0 — **attribution surfaced** in `waav show` + `/v1/models` | M1 |
  | cache-aware streaming STT (candidate: `nemotron-speech-streaming-0.6b`-class, 40 langs; fallback: sherpa-class streaming zipformer, Apache-2.0) | STT-A **realtime default** (80 ms–1 s partials) | license check at adoption (OQ-4) | M2 |
  | `whisper-large-v3-turbo` | STT-B; **batch transcription** (compat endpoint) at M2; streaming (LocalAgreement) at M4 | MIT | M2/M4 |
  | `kokoro-82M` | TTS-P3 **default** | Apache-2.0 | M1 |
  | Piper starter voices | TTS-P3 (CPU/edge) | MIT engine; **per-voice dataset licenses honored**; drags the GPL G2P helper (§16.4) | M2 |
  | `cosyvoice2-0.5b` | TTS P1+P2 composite; cloning-capable | Apache-2.0 | M2 |
  | `orpheus-3b` | TTS P1 flat-SNAC | **Llama-3.2 Community License derivative** (upstream relabel to Apache-2.0 is not effective): "Built with Llama" display, license copy, naming + AUP flow-down — carried in the `[license]` block; mirror repos include notices | M2 |
  | `chatterbox` | TTS P1+P2; cloning-capable | MIT; Perth watermark — served **unwatermarked unless the watermark component is enabled** (`applied_by_engine` truthful, §16) | M3 |
  | components: `snac-24k`, `mimi` (CC-BY-4.0, attribution), `vocos`, `campplus` | — | per-entry | M1–M2 |
  | deferred: `moshi` (CC-BY-4.0), `csm-1b` (Apache-2.0) | S2S | — | M5 |
  | license-excluded: Fish/OpenAudio-S1 (CC-BY-NC-SA), stock F5-TTS (CC-BY-NC), XTTS-v2 | — | — | — |

### FR-A — APIs (full shapes in §13)
- **FR-A1** Primary API = in-process Rust engine traits (used by the provider adapter and `infer-inproc`).
- **FR-A2** ONE native remote protocol "WaaV Infer WS v1": JSON control frames + raw binary audio frames, identical over UDS (sidecar) and TCP/TLS (remote); versioned handshake; **one session per connection** (KISS; the sidecar provider opens one UDS connection per session — UDS connections are ~free); `chunk_meta` **immediately precedes** its binary frame, per-context ordering guaranteed; session config/update incl. `language` and `keyterms` (biasing); STT events preserving three-level finality (`is_final`/`is_speech_final`/`is_finalized`); TTS `speak{text, context_id?, flush?}` with per-context bounded FIFO; **finalize/flush barrier with correlatable ack**; **cancel-with-ack** that drops queued output; keepalive (default idle timeout 30 s without audio/keepalive, config `session.idle_timeout`); a normative **error-code taxonomy** (§13.5) and **ingress limits table** (§13.6).
- **FR-A3** OpenAI-compat adapters: `POST /v1/audio/speech` and `POST /v1/audio/transcriptions` (+`GET /v1/models`) ship **in M1** (they are thin adapters over the same session API and they are what makes M1 externally adoptable — Speaches-parity is defined by these); `stream_format`/chunked-PCM latency path; `stream=true` SSE; `language` param mapped; WS `/v1/realtime` **GA dialect** at M3 (transcription sessions first; full S2S sessions at M5). Unsupported events/params documented machine-readably. Response formats per FR-E9.
- **FR-A4** Control plane: `/health/live`, `/health/ready`, per-model state in `/v1/models`; `POST /v1/models/{id}/load|unload`; `GET /metrics` (Prometheus); `GET /version`. Static API-key auth; quotas/tenancy remain gateway concerns.
- **FR-A5** Protocol types ship as a shared `waav-infer-protocol` serde crate (server and gateway provider cannot drift); OpenAPI + AsyncAPI committed and generated from the types; CI runs the official `openai` SDK against compat adapters and points the gateway's existing `OPENAI_BASE_URL` live-e2e at Infer.
- **FR-A6** **STT biasing**: `keyterms`/boost list accepted in `session.config`/`session.update` and mapped from the gateway's canonical typed `keyterms` (the gateway already carries these to Deepgram/Watson/Tencent — a local provider that drops them silently breaks the drop-in promise). v1 implementation: shallow-fusion/context biasing on Path-A greedy/beam decoders (NeMo/FlexCTC pattern), Whisper initial-prompt passthrough at M4. Models advertise support via a manifest capability flag; unsupported = machine-readably declared, never silently dropped.

### FR-G — Gateway integration
- **FR-G1** `waav-infer-provider` implements `BaseSTT` + `BaseTTS` (+`BaseRealtime` M5), registered as `"waav-infer"`; topology selected by provider config; all three topologies behind the same provider id.
- **FR-G2** Provider failure semantics: engine crash/protocol errors surface as standard provider errors (reconnect/circuit-breaker apply). **Normative breaker classification over the §13.5 taxonomy (gateway delta GW-3):** `admission_rejected`, `model_not_ready`, `draining` = **non-breaker-counting**, failover-eligible, Retry-After-honoring (they are *normal lifecycle*, not failure — a systemd-managed restart's Warming window must not flap the breaker); connection-refused/reset, `internal`, `stall_timeout` = breaker-counting; the half-open probe treats `model_not_ready` as "remain open without penalty", not a failed trial. Chaos gates: saturation ramp AND restart window — breaker stays Closed for lifecycle codes.
- **FR-G3** Gateway VAD/turn-detect signals flow into Infer sessions (e.g. `is_speech_final` finalize hint → STT finalize barrier; turn-end → TTS flush) — Infer never duplicates these models.
- **FR-G4** The provider MUST pass the gateway's existing provider conformance/live-test suites (mock + live harness) like any other provider.

### FR-O — Operations (full design §15)
- **FR-O1** Three-level health (`/livez` ≡ `/health/live`, `/readyz` ≡ `/health/ready` — both spellings are aliases, declared once here): `/livez` (process), `/readyz` (all *required* models Ready ∧ not draining; **saturation is NOT in readiness by default** — saturation is admission's job via 503+Retry-After; an opt-in `readyz.include_saturation` exists for LB-shedding setups, default off, because fleet-wide saturation flipping every pod unready = zero endpoints = total outage), per-model readiness. *Required* = the configured warm set (provider-configured models), implicitly pinned (FR-M7a). Liveness MUST NOT depend on model state (k8s kill-loop trap); **supervisors MUST NOT restart on `/readyz`** (§4.2b). Health endpoints are served on **both** the data and admin planes; the supervisor consumes the **data-plane** `/livez` (an admin-listener-only failure is `Degraded`-with-alert, never a process kill). Warmup gates Ready (FR-O2).
- **FR-O2** Model Ready ⇔ declarative warmup completed: per-execution-path synthetic requests (STT chunk; TTS sentence through the AR + codec **first-packet path**; vocoder window per batch bucket) flushing cuDNN algo search, CUDA-graph capture, allocator high-water marks.
- **FR-O3** Background **deep probe**, with a defined execution model — **no reserved slot** (r2's permanent probe slot was redundant with skip-under-load and cost up to 25% of capacity on B_max=4 devices): the probe admits like a normal lowest-priority request **only when a free slot exists**; **when occupancy leaves no free slot the probe is *skipped, not failed*** — live-stream progress (stall-watchdog silence) substitutes as health evidence under load (the SGLang false-timeout-under-load lesson). Probes arbitrate health only with idle headroom; B_max is all-paying capacity. Micro-batch stages probe via a calibrated side-request. Probe period default 30 s, timeout 20 s. N consecutive failures → Degraded + readiness flip; **Degraded→Ready requires M consecutive successes + a minimum dwell** (hysteresis — no flapping on marginal GPUs). Mandatory: GB10 demonstrably exhibits hung-kernel/zombie-GPU states (vLLM #41725) that surface-level health cannot see.
- **FR-O4** Drain: engine exposes `drain()/shutdown()` and installs **no signal handlers** (vLLM RFC #24885); standalone binary maps SIGTERM→drain; embedded mode driven by the gateway's RC6 token. Drain = readiness 503 + reject admissions immediately; in-flight streams finish bounded by `drain.deadline` (default 600 s — voice calls are minutes, k8s default 30 s is wrong); hard-cancel with clean end-of-stream after deadline; second signal = immediate stop.
- **FR-O5** Layered timeouts: queue deadline (= TTFA budget), first-output deadline, mid-stream progress watchdog (**Realtime-class only**: stall = no *engine* progress for max(2× expected inter-chunk, 1 s); consumer-paced parking — a full pacing buffer the client drains slowly — is NOT a stall; Batch is governed by job wall-clock deadlines plus a starvation gauge, since budget starvation under rated Realtime load is designed behavior), device watchdog (default 30 s) with **crash-only** policy (standalone exits for supervisor restart; embedded quarantines backend + reports unready).
- **FR-O6** Metrics: `waav_infer_*` namespace extending gateway `waav_*` (§15.4 table); reuse `waav_degraded_total` with new component values; histograms not summaries; `_seconds`/`_total` conventions; bounded labels; never per-stream labels.
- **FR-O7** OTel tracing: engine stage spans (admission/encoder/ar_decode/flow/codec_decode) parented under the gateway turn-profiler stages; W3C traceparent across IPC; only durations cross process boundaries.
- **FR-O8** Audio debug: per-stream fixed ring buffer (default last 30 s in+out + event timeline); `dump_on_error` ∈ {off, metadata_only (prod default), redacted, full}; **`metadata_only` is defined to exclude transcript/text payloads** (timings, event types, config, metrics only — transcripts are PII); **`full` requires an explicit PII-acknowledgment config flag**; PII marker + TTL janitor (default 7 d); bundle size caps + disk watermarks (§15.7). The **replay harness** re-executes bundles deterministically (seeded sampling); **load-generation corpora are synthetic/licensed only — dump bundles never silently become test corpora** (they stay inside the dump-policy domain).
- **FR-O9** **Structured logging policy**: JSON logs with mandatory fields `{stream_id, session_id, model, stage, backend/ep, trace_id, turn_id?}` on every stream-lifecycle and error event; rate-limited/sampled hot-path logging (an AR stage at 84 steps/s must not log per step); boot logs the resolved effective config (secrets masked; placeholder-shaped values rejected — the gateway shipped that bug once) + EP/backend + ledger resolution; dump bundles embed the correlating `stream_id`/`trace_id`. The **alert starter pack** (thresholds for readiness flap, Degraded > 5 min, TTFA p99 > budget, stall rate > 2%, waiting > 0 for 5 min, probe failure, drain overrun) ships as Appendix F so operators don't re-derive it.

### FR-D — Audio & data
- **FR-D1** Ingress acceptance: PCM16 LE mono at **{8, 12, 16, 22.05, 24, 44.1, 48} kHz** with edge resampling (rubato — lift the gateway's `StreamResampler` with its filter-history persistence + stale-gap clear + utterance-flush + **8 kHz sub-frame chunking at `in_rate/50`**; 8 kHz is the rate that exposes fixed-chunk latency bugs). **Telephony/8 kHz is a first-class ingress lane** (the contact-center self-host audience, §1.2): G.711 µ-law/A-law at the edge (`audio-codec-algorithms`), upsampled 8→16 kHz into the STT canon. **Opus is the unifying rate-agnostic codec** — one decoder absorbs the whole 8–48 kHz matrix (output rate freely chosen, RFC 6716) with FEC/PLC/DTX for lossy SIP trunks. Per-codec wire transport (RTP/SIP) stays a gateway concern (§2.2).
- **FR-D4** **Noise suppression** (gateway-owned per §2.2; engine-adjacent for the `waav run` mic path + in-proc edge tier): the rate matters — **DeepFilterNet is 48 kHz-only** (a 16→48→16 round-trip tax on the STT lane), so prefer **nnnoiseless** (pure-Rust RNNoise, 16 kHz) on STT ingress and DeepFilterNet-via-`ort` (deepfilter-rt) for the 48 kHz quality tier. Streaming recurrent denoisers thread GRU state as explicit ONNX I/O ("split streaming") — the same pattern as Silero VAD / Smart Turn on Path A.
- **FR-D2** TTS alignment events (word/char timing) SHOULD be emitted when the model supports them (barge-in truncation bookkeeping; OpenAI `conversation.item.truncate` pattern).
- **FR-D3** No payload logging by default; debug capture only via FR-O8 policy gates.

## 6. Non-functional requirements

### NFR-P — Performance (acceptance targets; measured per §18 harness)

**Measurement definitions (normative).** *TTFA*: `speak` frame received by the engine → first audio byte on the wire, **including queue time**, first segment ≤ 120 chars. *Partial latency*: last audio byte of an ingress chunk → partial-transcript emit. *Late chunk*: an audio chunk delivered after its pacing deadline (absorbed by the client-side buffer if non-empty). *Underrun*: client-observable buffer-empty event. A late chunk is not an underrun unless the buffer is empty — so NFR-P4's two clauses are different events, both required.

| ID | Target | Context |
|---|---|---|
| NFR-P1 | AR-TTS TTFA per **rated (model × device × precision) triple** — normative anchors: `cosyvoice2-0.5b` fp16 @ L4-class **≤ 250 ms p90**; `orpheus-3b` int4 @ L40S-class **≤ 250 ms p90**; `orpheus-3b` int4 @ GB10 **≤ 350 ms p90**; `orpheus-3b` fp8 @ H100-class ≤ 200 ms p90 (matches the Baseten TRT-LLM production anchor: 150–200 ms on H100-MIG). On L4-class, flat-codec 3B fp16/fp8 is **bandwidth-infeasible** (§8.6) — the 25 Hz composite class is the L4 recommendation. The **short-first-window ramp is normative for P1 models**: minimum decodable codec window (e.g. 7–14 SNAC tokens with left-pad) emitted before the steady 28-token window, gated by the FR-M6 streaming-equivalence test. | Steady state, rated concurrency; arithmetic in §8.6 |
| NFR-P2 | TTS-P3 (Kokoro-class) TTFA p90 **≤ 100 ms** GPU (first segment ≤ 120 chars); CPU (8-core): **≤ 700 ms p90 with a duration-capped first segment ≤ 1.4 s estimated audio** (≈ 20 chars at ~14 chars/s speech rate, clause-boundary permitting — FR-E8) — honest arithmetic: 1.4 s × measured Kokoro CPU RTF 0.2–0.5 ⇒ 280–700 ms, holding across the full measured band (≤ 500 ms typical at RTF ≤ 0.35). For the record: a 60-char segment is ~4 s of audio ⇒ 0.8–2 s on CPU — both prior revisions failed this conversion; CPUs measuring slower than RTF 0.5 are not CPU-realtime-rated for this model (P-6 surfaces it) | One-shot path, segmented per FR-E8 |
| NFR-P3 | Streaming STT partial latency p90 **≤ 300 ms** on a **cache-aware streaming model** (nemotron-streaming-class, the realtime STT-A default); `parakeet-tdt-0.6b-v3` runs batch/offline (RTFx ~3300) or documented 2 s-chunk streaming — it is not the realtime-partials model. Finalize barrier ack ≤ 150 ms. | Defined t0/t1 above, rated concurrency |
| NFR-P4 | Streaming viability ≥ 99.9% chunks on-time **and** underruns = 0, in a 30-min soak at rated concurrency | Definitions above; both clauses bind |
| NFR-P5 | **Warm cold-start** (artifact + calibration caches present): ready ≤ 10 s (CPU tier) / ≤ 60 s (GPU tier incl. warmup) for catalog defaults — assumes an idle device (co-resident warming is paced and slower by design, FR-S3b(c)) | = NFR-R2 (same split) |
| NFR-P5b | **First-ever boot on a fresh device**: serving begins on portable artifacts within NFR-P5 bounds; optimized device-locked artifacts (TRT/QNN) build on a background lane (minutes allowed) and hot-swap at session boundaries — never blocking readiness | vs NIM's 30-min first boot |
| NFR-P6 | Rated concurrency published per (model × device × precision) from local calibration. Reference anchors (correctly attributed): Kyutai DSM-ASR 400 realtime streams/H100, batch-64 @ 3× RT/L40S (**STT, not duplex** — duplex capacity is O(8–16)/L40S-class by the step-time model, pending M5 calibration); Baseten Orpheus-3B fp8: 16–24 streams/H100-MIG. Riva-NIM comparisons are made **model-matched only** (e.g. parakeet vs Riva-parakeet on the same GPU). | §8.6 |
| NFR-P7 | Gateway-added overhead for local provider ≤ 5 ms p99 vs direct engine call (UDS hop + adapter) | Existing gateway overhead ~12 ms budget intact |

### NFR-R — Reliability
- NFR-R1: Sidecar crash → gateway and non-local sessions unaffected (chaos-gated); bound sessions error within 500 ms and are failover-eligible.
- NFR-R2: Supervisor restart-to-Ready with warm caches (weights page-cached, device artifacts + CUDA graphs + calibration cached): **≤ 10 s CPU tier / ≤ 60 s GPU tier** (same split as NFR-P5 — they are the same bound; §15.2's single-GPU upgrade gap quotes the GPU number).
- NFR-R3: Device OOM is prevented by the §9.2 discipline (admission-time pool arithmetic + worst-case-shape warmup high-water + headroom; debug/soak allocator-delta instrumentation); a runtime device-OOM is classified a bug → Degraded + incident dump. (r2's "allocator wrapper errors on request-path allocation" was not constructible on the pinned stack and is withdrawn.)
- NFR-R4: Drain under load loses zero streams before deadline (chaos-gated).
- NFR-R5: 72-h soak: zero leaks (RSS/VRAM slope ≈ 0), zero deadlocks, zero unbounded queues.

### NFR-H — Hardware portability (tiers; CI matrix §17.5)
| Tier | Targets | Guarantee |
|---|---|---|
| **Tier 1** (prebuilt, benchmarked, release-gated) | linux-x64 CPU; linux-x64 CUDA 12.x; **GB10 aarch64 CUDA 13/sm_121**; macOS-arm64 | linux/GB10: all FR + NFR for rated pairs; nightly perf trend gates. **macOS scope (resolves r1 OQ-5):** catalog defaults (kokoro, parakeet/nemotron, whisper) all FR + NFR-P2/P3 via candle-Metal (AR) + MLAS-CPU (Path A; CoreML-EP is Preview/best-effort); AR-TTS NFR-P1 rated **per device class** — needs ≥ 273 GB/s, i.e. M-Pro/Max-class; GH-hosted M1/M2 runners host correctness gates only, the perf trend gate runs on a self-hosted M-Pro-class box |
| **Tier 2** (built + smoke-tested in CI) | windows-x64 (CPU/DirectML); linux ROCm/MIGraphX; linux-aarch64 Vulkan; Intel OpenVINO | Catalog defaults pass verify; perf best-effort |
| **Tier 3** (compile-checked; roadmap to T2) | android-arm64 + Hexagon (**QNN-EP: streaming-encoder + VAD/turn-class static graphs** — P3 VITS/StyleTTS2-class has data-dependent duration-expansion shapes HTP cannot run without per-model graph surgery, an open porter-kit problem, not shipped scope; **ggml-Hexagon AR: low-steps/s ≤ 1B-backbone class only** — Q4_0/MXFP4 rows, 2 GB HTP session limit; Orpheus-3B-class is bandwidth-excluded on phones by the §8.6 arithmetic); CANN | Documented experimental; named concrete deliverable: static int8-QDQ streaming-encoder artifact row |

- NFR-H1: CPU-only mode passes the **full FR suite for CPU-realtime-rated models** (kokoro, piper, STT-A streaming, whisper-int8 single-stream) and **functional/Batch verification for AR-TTS** (P-6's two guarantee classes).
- NFR-H2: No `#[cfg]` backend selection outside `-backend-*` crates; runtime capability queries everywhere else (P-8).

### NFR-X — Scale & limits (v1 envelope)
- Single node, 1–8 GPUs or CPU-only; models pinned to devices; stateless session router for multi-instance (§8.7). Multi-node fleet routing = normal LB/k8s InferencePool (non-goal to reinvent).
- Session count envelope: up to 1,024 concurrent sessions/node (bounded tables), subject to calibration-rated per-model capacity.

### NFR-C — Compatibility
- NFR-C1: Protocol semver; `protocol_version` asserted in handshake (gateway PROTOCOL_VERSION discipline); additive-only within a major.
- NFR-C2: OpenAI-compat tracked against the GA spec; upstream `openai-openapi` diffed on a schedule in CI.
- NFR-C3: Manifest schema versioned (`schema = 1`); engines reject newer-major manifests with a clear error; engines advertise supported majors and **the catalog dual-publishes both majors for a deprecation window** when the schema majors (a moved default tag must not break not-yet-upgraded fleet engines); the CLI warns on skew.

---

# Part II — System architecture

## 7. System overview

```
┌────────────────────────────── CLIENTS ───────────────────────────────────────────────┐
│  WaaV gateway (provider adapter)  ·  Pipecat / LiveKit / SDKs (native WS, OpenAI)     │
└──────────────┬───────────────────────────────┬───────────────────────────────────────┘
               │ in-proc traits / UDS / TCP+TLS │ REST/WS (OpenAI-compat)  + control plane
┌──────────────┴───────────────────────────────┴───────────────────────────────────────┐
│ (7) API LAYER          native WS v1 server · OpenAI adapters · control plane · auth   │
├───────────────────────────────────────────────────────────────────────────────────────┤
│ (6) SESSION LAYER      session registry · admission control · pacing buffers ·        │
│                        cancellation fan-out · per-session state (KV slot, codec win)  │
├───────────────────────────────────────────────────────────────────────────────────────┤
│ (5) MODEL REGISTRY     manifests · content store · lifecycle (Load→Warm→Ready→Drain)  │
│     & LIFECYCLE        keep_alive TTL · memory ledger · device artifact caches        │
├───────────────────────────────────────────────────────────────────────────────────────┤
│ (4) STAGED ENGINE      per-(model,stage) workers on dedicated OS threads:             │
│                        AR-batch (fixed-slot) · micro-batch · streaming-vocoder        │
│                        typed bounded queues · calibration tables · deep probe (borrows free slot)    │
├───────────────────────────────────────────────────────────────────────────────────────┤
│ (3) EXECUTION PATHS    STT: A frame-sync · B enc-dec · (C LLM-ASR)                    │
│                        TTS: P3 one-shot · P1 codec-LM · P1+P2 composite · (P2, P4)    │
│                        S2S: P1 + N streams (M5)                                       │
├───────────────────────────────────────────────────────────────────────────────────────┤
│ (2) SHARED COMPONENTS  DSP/mel · codec dec (SNAC/Mimi/DAC) · vocoders (Vocos/HiFTNet) │
│                        Euler-CFM+DiT · tokenizers · AR backbone+KV · CAMPPlus ·       │
│                        multi-codebook glue · samplers                                 │
├───────────────────────────────────────────────────────────────────────────────────────┤
│ (1) BACKEND ABSTRACTION  trait InferBackend (capability-queried; P-8)                 │
│     Path A: ORT (CUDA/TRT/OpenVINO/MIGraphX/QNN/DirectML/CoreML/XNNPACK/CPU)          │
│     Path B: candle(+vendored moshi-core) · llama.cpp/ggml (feature) · CPU floor       │
│     compute-type resolver (CT2 ladder) · device caps (unified-mem aware)              │
└───────────────────────────────────────────────────────────────────────────────────────┘
```

Data-plane invariants: audio crosses layers as PCM16 frames (Bytes); tokens/latents stay inside the staged engine; every inter-stage queue is typed and bounded; control messages (cancel/finalize/drain) jump data queues.

## 8. Engine core: sessions, stages, scheduling

(Backed by `spec_scheduler.md` — the deadline-serving canon (Orca, Sarathi, Clockwork, Andes, TokenFlow, VoxServe, EDF/CBS theory) and the production-read moshi-server source.)

### 8.1 The reframe that drives the design
Voice serving is **rate scheduling of persistent streams, not job scheduling**. A stream needs a *sustained* step rate set by the codec frame rate — and that rate varies ~7× by architecture (Mimi-class 12.5 steps/s vs flat-codec Orpheus ~84 steps/s). Throughput beyond the rate is only useful to fill the pacing buffer. Consequences:
1. **The schedulability test is per-tick**: T_step(B_max) ≤ frame budget — a cyclic-executive property, checkable from a measured table.
2. **Buffers convert burst surplus into capacity**: N ≤ S·B·RTF(B)/duty (e.g. 0.8·64·2.1/0.5 ≈ 215 dialogue TTS streams per H100 for DSM-class). Batching is near-free until the deadline binds (DSM-ASR batch 1→256 costs only ~4.6× step time).
3. **Model choice IS a capacity decision** — hence `steps_per_second` is a required manifest field and capacity tables are published per (model × device × precision).

### 8.2 Stage anatomy (uniform)
Every stage = IO-shell + scheduler over typed bounded channels, on a **dedicated named OS thread** with thread-affine device state (warmup, CUDA-graph capture, teardown all on the owning thread — mistral.rs/vLLM-Omni precedent):

```rust
enum StageIn  { Open(SessionInit), Data(Chunk), Finalize{id}, Cancel{reason}, Drain }
enum StageOut { Opened, Data(Chunk), Finalized{id}, Cancelled, Closed(Reason), Error(E) }
trait StageScheduler { fn tick(&mut self, now: Tick) -> TickReport; }  // sync loop; no async inside
```
Archetypes: **AR-batch** (fixed-slot masked batch, §8.3), **micro-batch** (one-shot graphs: P3 TTS, encoders, CFM steps batched by bucket), **streaming-vocoder** (windowed codec/vocoder decode with left-context + crossfade + dynamic first-chunk for TTFA ramp). The codec/vocoder stage is **separately batched and pipelined** — it is frequently the true bottleneck and gets its own concurrency budget.

### 8.3 AR scheduler v1 — fixed-slot masked batch (the production-proven KISS baseline)
Adopted from moshi-server (read at `research/rust/moshi/rust/moshi-server/src/batched_asr.rs`; ~660 lines serving 400 realtime STT streams/H100 — notably **without CUDA graphs**):
- Slot table `Vec<Option<Stream>>` of size B_max (locally calibrated; all slots are paying capacity — the probe borrows a free slot opportunistically, FR-O3); `StreamMask` masks empty slots; static shapes.
- Tick: gather per-slot inputs → one batched step → scatter outputs → per-slot state advance. Idle slots cost a masked lane (acceptable: batching near-free in this regime).
- Admission = free slot ∧ ledger reservation. Abort = clear mask bit + return KV slot (O(1), ≤ 1 tick).
- Schedulable by construction iff T_step(B_max) ≤ S·T_frame (S = safety factor ~0.8), checked against the **local** calibration table at load.
- CUDA graphs: an optimization captured at warmup *where the backend supports it*, with **mandatory eager fallback**; admission always uses the measured T_step of the path actually running (FR-S2). High-steps/s models (Orpheus 84 steps/s) want graphs for headroom; viability cannot depend on them.
- 2 ms idle sleep when no slots active; tick cadence pinned to the model frame rate.

### 8.3b Calibration lifecycle (the load-bearing artifact, made operational — FR-S3b)
Manifest `resources` rows are **reference-class priors** written by the porter kit. Admission consumes only **local tables**: measured during Warming on the operator's device against the configured warm set (T_step per batch bucket, co-located stages under synthetic co-load), persisted with the device-artifact cache and keyed (sha256 × device × SDK/driver × warm-set hash) — driver updates invalidate; warm-set changes derate-until-recalibrated. At runtime, per-tick T_step feeds an EWMA/p99; sustained drift triggers the **debounced, rate-bounded, hysteresis-guarded** shed-don't-smear response (FR-S3b). `waav calibrate` produces full rating runs; rated capacity is queryable (`/v1/models` rated block) so routers and operators plan from measurements, not datasheets.

### 8.3c Device duty ledger — cross-model & cross-stage admission (the joint-feasibility test)
Per-(model, stage) slot tables answer "can this stage take one more stream"; they cannot answer "can this **device**". The device ledger extends §9.1 from bytes to **duty**, closing the r2 CRITICAL (multi-model single-GPU is the flagship config — every gateway voice loop = STT + TTS on one device):

```
duty(stage) = T_step(B_active) × tick_rate          // measured, per calibration; for chunk stages:
                                                     // per-chunk cost × chunk rate (CFM, vocoder, codec)
ADMIT a realtime stream on model m  iff  Σ_{all colocated realtime stages, all models} duty ≤ S   (S ≈ 0.8)
```

- **Every stage type is in the sum** — AR decode *and* the CFM/codec/vocoder stages (which are frequently the true bottleneck) each carry a calibrated duty entry; AR-only admission that the codec stage can't sustain is exactly the bug this prevents (r2-A1).
- The §8.6 bandwidth-floor form (`bytes_touched × steps/s ÷ device_bandwidth`) is the analytical prior for duty; calibration replaces it with measurement (P-3).
- Bandwidth-bound duties compose ≈ additively; the residual second-order interference (cache/SM contention) is precisely what the FR-S3b drift detector backstops — **shed is the backstop, never the mechanism**.
- §8.7's "per-GPU admission sums colocated stage instances" refers to THIS formula; Batch-class work piggybacks only into leftover duty (§8.5).
- The mixed-model soak (M2 gate, §18.4) exercises the joint test at rated load on one GPU.

### 8.4 Scheduler v1.x evolution — buffer-depletion EDF (only when triggered)
Trigger: utilization evidence that fixed-slot idles capacity on bursty TTS workloads (>30% slot idle at peak). Then: per-stream deadline D_i = now + buffer_s − s_safe; schedule by earliest depletion; park streams above high watermark (~2 s); admission via measured-utilization test. This is exactly where Andes/TokenFlow/VoxServe converged; EDF optimality + CBS reservations are the classical guarantees. **Not built before the trigger fires (P-2).**

### 8.5 Mixed work & priorities
Per-tick token budget (Sarathi): realtime decode lanes first; leftover budget admits prefills, then `Batch`-class chunks (offline transcription on the same GPU — mandatory on single-GPU GB10-class boxes that have no MIG). Overload sheds Batch first, then rejects new Realtime; **never** global degradation (EDF domino under overload is why admission is mandatory; measured cliff: TTFC 259 ms @1 → 9.8 s @160 streams on a 0.3B TTS).

### 8.6 Capacity planning (published from local calibration; the table below gives *bandwidth-floor arithmetic + measured anchors*, never invented stream counts)

The governing arithmetic: an AR step must read all weights once → `t_step_floor = weights_bytes / mem_bandwidth`; schedulable iff `t_step(B_max) ≤ 0.8 × (1000/steps_per_second) ms`. Orpheus budget = 11.9 ms/step; CosyVoice2 = 40 ms; Mimi-class = 80 ms.

| Model class | steps/s | H100-class (3.35 TB/s; MIG ~480 GB/s) | **L40S** (864 GB/s) | **L4** (300 GB/s) | **GB10** (273 GB/s unified) |
|---|---|---|---|---|---|
| Mimi-class STT/duplex (12.5 Hz) | 12.5 | **400 STT streams measured** (Kyutai DSM); duplex O(8–16) by step model (7B+depth), M5-calibrated | batch-64 @ 3× RT measured (DSM-ASR) | feasible, calibrate | comfortable |
| Composite TTS (CosyVoice2-class 0.5B, 25 Hz) | 25 | AR light; **CFM+vocoder stage is the binding cost** — counts are calibration outputs (upstream repo shows p99 outliers ≥ 4 concurrent streaming on a 4090), not datasheet derivations | same | **recommended class for L4**; fp16 floor 0.04 ms — AR trivially fits; rated by CFM calibration | **recommended class for GB10** |
| Flat-codec TTS (Orpheus-3B, ~84 steps/s) | ~84 | fp8: **16–24 streams/MIG, TTFB 150–200 ms measured** (Baseten TRT-LLM) | fp16 floor 6.9 ms (tight); fp8 3.5 ms / int4 ≈ 2.5 ms (2.15 GB ÷ 864 GB/s) → O(8–16) streams, calibrate | **fp16 (20 ms) and fp8 (10 ms) floors exceed the 9.5 ms bound — infeasible; int4 (7.2 ms) only, ~O(4–8) streams** | **fp16 infeasible (22 ms floor); int4 ~7.9 ms floor → O(4–8) streams (graph-assisted; bound 9.52 ms leaves ≤1.6 ms overhead headroom); quantization here is admission capacity** |
| One-shot P3 (Kokoro 82M) | n/a | n/a (μs-class) | hundreds of RTFX | hundreds | tens–hundreds; **CPU RTF ≈ 0.2–0.5 measured** (8–32 cores) — realtime on CPU, but not the "0.008" of marketing decks |

Two explicit consequences: (1) **L4 is a GB10 bandwidth-sibling, not an L40S-sibling** — every "L4-class" positioning claim assumes the 25 Hz composite class or int4 flat-codec; (2) prefer low-frame-rate semantic-token TTS on bandwidth-floor devices (GB10/L4); cells without a "measured" marker are filled by Phase-0/install calibration, not asserted.

### 8.7 Multi-GPU / multi-instance
Replicate-and-route with hard session affinity: model instances pinned to GPUs/MIG slices; a stateless router places sessions on the most-free-duty instance using per-instance calibration tables (handles heterogeneous fleets); per-GPU admission = the §8.3c device duty ledger summed over colocated stage instances. No TP/PP for ≤ 4B voice models; no KV migration; no prefill/decode disaggregation (all evaluated non-goals, ADR-12).

## 9. Memory & model lifecycle

(Backed by `spec_memory.md`.)

### 9.1 The ledger
One **device memory ledger** per device, resolved at startup: weights + KV pools + codec windows + workspace + CUDA context reserve (~0.5 GB/process) + ~10% headroom; on unified-memory devices (GB10, Apple) a single ledger covers CPU+GPU and subtracts a gateway-bandwidth reserve; on discrete GPUs a dual ledger adds explicit staging. `DeviceCaps { unified_memory, mem_bytes, mem_bandwidth_gbps, numa_node }` — `unified_memory`/`mem_bytes`/`numa` are runtime-queried; **bandwidth is measured by an install-time microbenchmark (or taken from a built-in device table as prior)** — it is not OS-queryable, and the measured-not-modeled doctrine applies to it like everything else. **No code may assume either memory model** (P-8).

### 9.2 Allocation discipline (honest mechanisms, not magic)
All device allocation happens at **load time** (pools) or **admission time** (slot/quota reservation from pools). Device OOM is prevented by four real mechanisms, not a fictional allocator veto: (a) admission-time pool arithmetic from the ledger; (b) **warmup executes the worst-case batch shape** and records the allocator high-water mark (static shapes ⇒ steady-state allocations recur within the warmed footprint); (c) ~10% ledger headroom; (d) any runtime CUDA-OOM is classified a bug → model Degraded + incident dump (it cannot be "handled"). Debug/soak builds instrument allocator deltas per tick to catch request-path allocation regressions. ORT: the shared per-device arena needs `CreateAndRegisterAllocatorV2`, which `ort` rc exposes only via raw `ort-sys` — a named M0 shim task, with per-session `gpu_mem_limit` arenas as the fallback if the rc API shifts.

### 9.3 KV management (voice-sized; matches what the pinned stack actually provides)
- **v1 KV = per-slot fixed-quota buffers allocated at admission** — the `ScatteredKvCache`/`RotatingKvCache` pattern moshi-core ships and runs in production (`reset_batch_idx` per slot), sized from manifest `kv_quota_tokens` (e.g. Orpheus 1024, CosyVoice2 384), pool ≈ target_streams × quota in the ledger. *r1 said "paged 16-token blocks with free-list" — that is vLLM vocabulary requiring paged-attention gather kernels that candle 0.10 does not have (mistral.rs does it only via a forked candle, which ADR-4 forbids) and that fights the static-shape fixed-slot design. Block-paging is a named-trigger evolution (variable very-long prompts at scale), with the kernel cost on every Path-B backend stated honestly.*
- Whisper cross-attention KV = fixed per-window static buffer; streaming-encoder state = per-slot rings.
- fp8 KV where the backend supports it (capability-queried). **Non-goals:** CoW, radix/prefix-tree reuse, KV swap/offload, vAttention/CUDA-VMM (ADR-12). Prefix caching only for the static speaker/system preamble (SpeakerEmbeddingCache LRU).

### 9.4 Model lifecycle state machine
`Loading → Warming → Ready ⇄ Degraded → Draining → Failed` (no `Paused` v1 — FR-M7)
- Warming = FR-O2 warmup (graph capture, algo search, high-water-mark touch) + local calibration (§8.3b); gates Ready; duty-cycle-paced when co-resident realtime streams exist (FR-S3b(c)).
- `keep_alive` TTL (default 30–60 min; 0 = unload at idle; −1 = pin); LRU among non-pinned; **sessions never wait on loads** — a session-open MAY arm an async load while rejecting with retry_after (FR-M7d); required models auto-reload bounded by FR-M7b. Admin verbs: load/unload.
- Device-artifact caches (TRT engines + timing caches, QNN context binaries, CUDA graphs, autotune results) persist keyed (sha256 × device × SDK/driver) → restart-to-Ready within NFR-R2 (≤ 10 s CPU / ≤ 60 s GPU warm).

### 9.5 Weights & host memory
mmap'd GGUF/safetensors (page-cache shared across restarts and processes); NUMA-aware placement on Grace; ISQ-style requantize-at-load when the manifest row calls for it (CT2 `resolve_compute_type` ladder picks the row).

## 10. Hardware portability layer

(Backed by `learnings.md` §9, `portability.md`, `code_engines.md`, `spec_rust_impl.md`. This is the defining constraint — restated from the user requirement: GB10 matters, and so do CUDA x86, ROCm, Hexagon, Intel, Apple, and plain SIMD CPUs.)

### 10.1 The two-path decision (ADR-1, reaffirmed post-research)
- **Path A — static graphs on ONNX Runtime** (`ort` 2.0, load-dynamic): encoders, CTC/transducer STT, VAD-class, P3 one-shot TTS, vocoders, CFM estimator steps. One graph, many EPs; the gateway's `apply_execution_providers` policy is lifted verbatim and **widened**: `EpKind` + probe order gain OpenVINO, MIGraphX (ROCm-EP is deprecated upstream), QNN; same warn-degrade-CPU + `waav_degraded_total` + `waav_ort_ep` discipline.
- **Path B — native autoregressive** for token-by-token decode with growing KV: **official candle 0.10.x** (pinned; forks forbidden — mistral.rs pins a forked candle and that is a named anti-pattern, ADR-4) + **vendored moshi-core** (streaming transformer, StreamMask batching, Mimi codec, depth-transformer — MIT/Apache) + **llama-cpp-2 behind a feature** for GGUF breadth, Vulkan-everywhere, and the Hexagon NPU road.
- Compilers (TVM/IREE/MLC) are **not** the spine (dynamic shapes, streaming ergonomics, operability); reachable later behind the same trait if a fixed hot model on a fixed SoC demands it.

### 10.2 The backend trait (the single portability seam)
```rust
trait InferBackend {
    fn id(&self) -> BackendId;                       // ort | candle | ggml
    fn devices(&self) -> Vec<DeviceCaps>;            // runtime-enumerated
    fn supports(&self, art: &ArtifactKind, dev: &DeviceCaps) -> Support;  // capability query, not #[cfg]
    fn load(&self, art: &Artifact, dev: &DeviceRef, ct: ComputeType) -> Result<Box<dyn LoadedModel>>;
}
trait LoadedModel { fn session(&self) -> SessionKind; /* StaticGraph(run/io-bind) | ArStep(step/state) */ }
```
Runtime dynamism comes from **two already-shipping C-ABI seams**, not a new plugin ABI: `ORT_DYLIB_PATH` (swap the ORT build per machine) and `GGML_BACKEND_DL` (dlopen `libggml-{cuda,vulkan,hexagon}.so`, score-based selection — exposed today by `llama-cpp-sys-2`'s `dynamic-backends` feature). Backends themselves are compile-time cargo features (ADR-5).

**Cross-backend tensor interop & device residency (normative — the composite P1+P2 path crosses backends):** v1 inter-*stage* handoffs exchange **typed host-memory tensors** — the cost is included in stage calibration and is bounded (a 25 Hz mel chunk is ~100 KB: sub-ms on PCIe, ~free on unified memory). An intra-stage **iterative loop MUST NOT round-trip device↔host per step**: either the Euler `x_{t+1}` update is fused into the ORT estimator graph (one `Run` = one step) or the estimator runs device-resident with io-bound I/O on one backend — never host-solver-calls-device-estimator per step. Cross-backend *device-pointer* sharing is a named-trigger optimization (trigger: measured handoff cost > 5% of the owning stage's budget). The `InferBackend` trait deliberately exposes no cross-backend device-tensor type in v1.

### 10.3 Target × path matrix (normative summary)
| Target | Path A (ORT EP) | Path B (AR) | Notes |
|---|---|---|---|
| CPU x86/ARM (floor) | MLAS+KleidiAI / XNNPACK | candle CPU; ggml CPU (Q-quants) | Always present & correct (P-6) |
| NVIDIA x86 | CUDA EP; TRT cached engines | candle CUDA; ggml CUDA | Prebuilt Tier-1 |
| **GB10 aarch64 (sm_121)** | source-built ORT (no prebuilts exist); TRT preloaded | candle CUDA 13 (cudarc dynamic); ggml CUDA source | Self-hosted CI runner; unified-memory ledger; nightly TTFA gate |
| AMD | MIGraphX EP | ggml HIP / Vulkan | Tier 2 |
| Intel | OpenVINO EP (CPU/iGPU/NPU) | ggml SYCL / Vulkan | Tier 2 |
| Apple | CoreML EP (static) | candle Metal; ggml Metal | Tier 1 (macOS-arm64) |
| Qualcomm Hexagon | QNN EP (int8-QDQ, static shapes, no Loop/If → **streaming-encoder + VAD/turn-class graphs**; P3 VITS-class excluded: data-dependent duration-expansion shapes) | ggml Hexagon HTP (experimental; Q4_0/MXFP4 only, 2 GB session limit → **≤ 1B low-steps/s AR class**; 3B flat-codec bandwidth-excluded) | Tier 3 roadmap |
| Any-GPU Windows | DirectML EP | ggml Vulkan | Tier 2 |

### 10.4 Compute-type resolution (CT2 ladder, adopted verbatim)
`resolve_compute_type(requested, device, model)` resolves int8/fp16/bf16/fp8/Q-quants against real device capability with a documented fallback ladder, then either loads the matching artifact row or ISQ-requantizes at load. Quantization is **orthogonal** to model and device; on bandwidth-starved devices (GB10) it is capacity (§8.6).

### 10.5 Backend conformance suite
Every (backend × device-class) must pass: op-level parity micro-tests for shared components; token-exact goldens for pinned (backend, quant, target) triples; perceptual audio goldens cross-backend (log-mel distance — never bit-exact across backends); calibration-table generation; deep-probe canaries. A backend that cannot pass does not ship in that tier.

## 11. Execution paths & shared components

(Condensed from `learnings.md` §4–§7 — full taxonomy, code-grounded corrections, and per-model pipelines live there and in `notes/code_models.md`. This section is the engineering contract.)

### 11.1 Execution paths (engine contracts)
| Path | Contract (stages) | v1 models | Phase |
|---|---|---|---|
| **STT-A** frame-sync | mel → cache-aware encoder (streaming state) → CTC/TDT greedy (+opt. beam) → detok; finalize barrier flushes encoder state | parakeet-tdt-0.6b-v3 (batch/chunked, M1); cache-aware streaming model per OQ-4 (M2) | M1–M2 |
| **STT-B** enc-dec | mel → windowed encoder → AR text decoder (KV) + LocalAgreement-2 overlap-commit for streaming | whisper-large-v3-turbo (batch M2; streaming M4) | M2–M4 |
| **STT-C** LLM-ASR | audio encoder → projector → LLM decode (reuses P1 substrate) | (later; Qwen3-ASR-class) | M5+ |
| **TTS-P3** one-shot | text frontend/G2P → single graph → waveform (vocoder fused or chained) | kokoro (M1); piper voices (M2) | M1–M2 |
| **TTS-P1** codec-LM | template+tokenize → AR step loop (logit masking to audio vocab; flat-SNAC de-interleave or depth/RQ inner loop) → streaming codec decode (window + crossfade + first-chunk ramp) | orpheus-3b | M2 |
| **TTS-P1+P2** composite | AR semantic tokens (single FSQ codebook) → chunk-aware Euler-CFM → mel → NSF/iSTFT vocoder; flow cache between chunks | cosyvoice2-0.5b (M2); chatterbox fast-follow (M3) | M2–M3 |
| **TTS-P2 / P4** | CFM/diffusion non-AR; AR + per-frame diffusion head + continuous VAE | (later; F5-class needs license-clean retrain; VoxCPM) | M4+ |
| **S2S duplex** | P1 substrate × N interleaved streams + input encoder + inner-monologue offset; Mimi codec | moshi | M5 |

### 11.2 Shared components (engine-owned crates/modules; manifests reference by id)
DSP frontend (STFT/iSTFT/mel; one kernel, four uses — **reuse `kaldi-native-fbank` (Rust) or realfft+rustfft+`mel_spec`**) · codec decoders (snac-24k, mimi, dac — **extract from candle-transformers' in-tree Rust `snac.rs`/`dac.rs`/`mimi/`** + add the sliding-window streaming wrapper; only Mimi is already streaming) — decode is the streaming hot path; **encoders ship as conditioning-prep components** for cloning (FR-E10) · vocoders (vocos default, hiftnet/istftnet for NSF models, bigvgan GPU-flagged — **note: ONNX has no native ISTFT op, so iSTFT vocoders need the `istft-onnx` export recipe in the porter kit**, not an M2 surprise; sherpa-onnx's metadata-driven vocoder dispatch is the wiring reference) · **Euler-CFM solver** (one chunk-aware `solve_euler` + DiT estimator hook — 4+ model families share it; **~40-LOC host-side loop, reference Matcha-TTS/CosyVoice2; candle `mmdit`/`flux` supply the Rust DiT/AdaLN blocks**; the chunk-aware flow cache with −34-frame overlap is the streaming-equivalence enabler) · text frontends (raw-text LLM tokenizers via HF `tokenizers`/`kitoken`; phoneme path via misaki(-rs) / CharsiuG2P-ONNX / espeak-ng behind isolation, §16) · AR backbone runtime (Llama/Qwen2/GPT2-class on candle; GGUF on both B-paths) · KV manager (§9.3) · samplers/logit-processors (incl. audio-vocab masking, CFG) · multi-codebook glue (flat-SNAC, depth/RQ; delay-pattern strips BOC/EOC with `where(code≥vocab→0)` **never `clamp`**; MTP added when a catalog model needs it) · CAMPPlus speaker encoder + SpeakerEmbeddingCache (**reuse the sherpa-onnx `SpeakerEmbeddingManager` pattern + CAMPPlus ONNX**).

### 11.3 What "supporting 90% of models" means operationally
A new model is supported when a manifest can (a) name an existing exec path, (b) reference existing components, (c) carry weights/quant rows that pass `waav onboard verify`. Engine code changes only when a genuinely new path or component appears (e.g. a new codec family) — tracked in the decision log with the porter-kit gap report as evidence.

## 12. Model manifest, packaging & distribution

(Backed by `spec_packaging.md`. Schema sketch: Appendix B.)

- **Manifest** = TOML authored → canonical JSON → sha256 → OMS/sigstore-signed (the NGC/Kaggle-adopted standard; keyless CI). Required fields per FR-M1. The two genuinely novel fields vs all prior art (LM Studio model.yaml, piper voices.json, sherpa configs): the **per-hardware quant/artifact table** and **codec-LM frame_layout/serving metadata**.
- **Store**: `~/.waav/models/{manifests,blobs}` content-addressed (ollama mechanics); verification on pull and load; export/import tarballs.
- **Distribution substrate**: HF Hub (Xet-backed) + WaaV org mirror repos (upstream repos vanish; mirrors pin provenance) + a **static signed catalog index** (git + raw JSON — piper voices.json pattern). Explicit non-goal: running a registry/blob server (ADR-7); OCI/ORAS transport optional later (manifest rows map 1:1 to OCI layers).
- **Resolution**: unsuffixed tag → device-aware artifact row via live device enumeration + CT2 ladder; explicit tags and `hf://repo[@rev][:quant]` always available; metadata sniffing (GGUF KV / ONNX metadata_props / safetensors `__metadata__`) is the fallback for foreign files, manifest remains source of truth.
- **Porter kit** (`waav onboard`): init/convert/verify/publish in pinned containers; conversion reality is encoded in recipes — AR/CFM loops never export to ONNX (graphs are stateless step functions, loops live in the engine); STFT/mel stays engine-side; QNN needs static int8-QDQ; vocoder final convs need mixed precision + calibration sets.
- **Catalog v1**: per FR-M8 with license column and watermark flags; CC-BY-NC excluded or gated behind `--accept-license noncommercial`.

## 13. API surfaces

(Backed by `spec_api.md`. All three surfaces are adapters over ONE internal session abstraction.)

### 13.1 In-process engine traits (primary)
```rust
let engine = InferEngine::open(EngineConfig { store, devices, ledger, .. })?;
let h: SessionHandle = engine.open_session(SessionSpec {
    model: "kokoro", task: Task::Tts, class: Class::Realtime,
    audio: AudioSpec { out_rate: 24_000, .. }, conditioning: Some(voice("af_heart")), ..
})?;            // -> AdmissionRejected{reason, retry_after} on capacity
h.send(SessionIn::Text { text, context_id, flush })?;   // TTS
h.send(SessionIn::Audio(bytes))?;                        // STT
h.send(SessionIn::Finalize { id })?;  h.send(SessionIn::Cancel { .. })?;
while let Some(ev) = h.recv().await { /* Audio{pcm,rate,dur} | Transcript{STTResult} | Finalized{id} | Cancelled | Closed */ }
engine.admin().load("orpheus-3b").await?;  engine.drain(Deadline::secs(600)).await?;  // no signal handlers in lib
```
STT events carry the gateway's full three-level finality + words/confidence (the trait result types are shared with `waav-infer-protocol`).

### 13.2 Native protocol — "WaaV Infer WS v1" (UDS = sidecar; TCP/TLS = remote; same frames)
- **Cardinality**: **one session per connection** (KISS; Deepgram/Cartesia precedent; UDS connections are cheap — the provider opens one per session).
- **Frames**: JSON text frames tagged `"type"` for control/events; **raw binary frames** for audio both directions (no base64 tax). **Association rule: `chunk_meta` immediately precedes its binary frame; per-context ordering is guaranteed within the session.**
- **Handshake**: `session.config{model, task, class, language?, audio{encoding, sample_rate, channels}, conditioning?{voice | reference_audio | speaker_embedding}, keyterms?, protocol_version}` → `ready{session_id, protocol_version, model_digest}`.
- **STT**: binary audio in → `transcript{...STTResult fields incl. is_final/is_speech_final/is_finalized, words?, language?}`; `finalize{id}` → `finalized{id}` (barrier with correlatable ack — Deepgram Finalize / Cartesia flush_done / Kyutai Marker pattern); `keepalive`; idle timeout default 30 s (`session.idle_timeout`).
- **TTS**: `speak{text, context_id?, utterance_id?, flush?}` (per-context bounded FIFO; over-depth ⇒ `error{code:"backpressure"}`) → `chunk_meta{sample_rate, format, duration_ms, context_id, utterance_id?, generated: true, alignment?}` + binary chunk (`generated` = the §16.6 provenance floor; the model digest is session-level, carried in `ready`); `flush{id}`→`flushed{id}`; `clear{context_id?}`→`cleared{context_id?}` (drops queued output — cancel-with-ack, FR-A2).
- **Egress pacing & slow consumers**: the server keeps a bounded per-session egress buffer; delivery is consumer-paced. A consumer-full buffer **parks** generation (FR-S6) and is *not* a stall (FR-O5 measures engine progress, not consumer drain); a consumer slower than realtime for longer than the pacing buffer gets a `backpressure` event and, past a configured bound, a typed `close{reason:"slow_consumer"}`.
- **Session update**: `session.update{...}` mid-stream for mutable params (language, keyterms, speed).
- **Correlation**: `session.config` accepts `traceparent` (FR-O7); `utterance_id` is echoed on every chunk_meta/event it produced; `error` frames carry `context_id` when scoped to one.
- **Errors/close**: typed `error{code, message, retriable, retry_after_ms?, context_id?}` (taxonomy §13.5); `close{reason}`.
- Protocol types live in `waav-infer-protocol` (serde + bytes); AsyncAPI committed; URL-versioned `/v1/ws`. **Appendix A is generated from the protocol crate** — the r1 field-name drift (`flushed{id}` vs `flush_id`, `retry_after` vs `retry_after_ms`, `model_manifest_digest` vs `model_digest`) is exactly what generation prevents; the names above are normative.

### 13.3 OpenAI-compat adapters (ecosystem reach; thin, documented-divergence; REST ships in M1)
`POST /v1/audio/speech` (model/input/voice/`response_format` per the FR-E9 codec policy: wav|pcm|opus|flac native, mp3 feature-gated, aac → `unsupported_format`; `speed`; `stream_format` audio|sse) · `POST /v1/audio/transcriptions` (multipart with Symphonia ingress decode, `language`, `stream=true` SSE `transcript.text.delta/done`, timestamp_granularities) · `GET /v1/models` · WS `/v1/realtime` **GA dialect** at M3 (`session.type: transcription` first; `response.output_audio.delta` naming — the gateway's own realtime client still speaks the deprecated beta dialect and migrates as GW-7). `speed` semantics: P3 = native length-scale; AR models = `unsupported_param` unless the model is natively rate-controllable (manifest capability) — no post-hoc time-stretch in v1. `stream_format=audio` chunks wav/pcm; mp3/flac responses are buffered-encode (documented). Compat limitations (e.g. no word timestamps on realtime-transcription deltas — an OpenAI API gap, which is precisely why the native protocol exists; mp3-default divergence when the feature is absent) are machine-readably documented (Speaches precedent). **Explicitly rejected compat surfaces** (recorded): Riva-gRPC shim (Riva users have Riva; ADR-8's vendor-emulation rule), ElevenLabs/Deepgram wire clones. A browser/JS client for the native WS ships with M4 (LiveKit/web audience).

### 13.4 Control plane & API security
`GET /health/live` · `GET /health/ready` (aliases `/livez`/`/readyz`; served on both planes, FR-O1) · `GET /v1/models` (per-model `state`, `[license]` incl. **attribution text**, watermark provenance, rated capacity, provenance signed/unsigned) · `POST /v1/models/{id}/load|unload` · `POST /admin/drain` · `GET /metrics` · `GET /version` · `POST /v1/voices` (cloning registration, FR-E10, policy-gated) · `DELETE /v1/voices/{id}` (biometric deletion verb, §16) · **Batch jobs (M3)**: `POST /v1/jobs/transcriptions`, `GET /v1/jobs/{id}` (resolved OQ-6: lean async jobs, not OpenAI batch semantics).

**Security model (normative):**
- **Data plane vs control plane are separated**: model-admin verbs (load/unload/drain) are served on a **separate admin socket/port** or require a distinct **admin-scoped key** in all modes — a data-plane peer or leaked data key must not be able to unload a Ready model out from under live sessions.
- **UDS**: socket created in a 0700 runtime dir, mode 0600, owner = service user; optional `allowed_peer_uids` enforced via `SO_PEERCRED`.
- **TCP**: default bind `127.0.0.1`; **binding non-loopback without auth configured is a hard startup error** (the ollama CVE-2024-37032 / internet-exposed-instances lesson). TLS 1.2 floor (1.3 recommended, rustls defaults); ≥ 2 concurrent valid API keys with hot-reload (SIGHUP/admin verb) so rotation never requires a fleet drain; constant-time comparison; mTLS documented as the T3 fleet option.
- Admin/debug endpoints beyond the list above are disabled by default (vLLM dev-mode precedent).

### 13.5 Error-code taxonomy (normative, shared across native WS / REST / in-proc)
| code | retriable | meaning |
|---|---|---|
| `admission_rejected` | yes (+`retry_after_ms`) | over rated capacity / quota |
| `model_not_ready` | yes (+`retry_after_ms` ≈ load estimate) | exists, loading/warming |
| `model_not_found` | no | unknown id/tag |
| `draining` | yes (other instance) | instance is draining |
| `backpressure` | yes | per-context FIFO / queue bound hit |
| `payload_too_large` | no | limits table §13.6 |
| `unsupported_format` | no | e.g. aac egress; mp3 without feature |
| `unsupported_param` | no | e.g. biasing on a model without the capability |
| `bad_config` | no | invalid session.config (incl. unknown language/voice) |
| `unauthorized` / `forbidden` | no | key missing/wrong scope |
| `stall_timeout` | yes | mid-stream progress watchdog fired (FR-O5) |
| `internal` | maybe | bug — always logged + dump-policy eligible |

### 13.6 Ingress limits (normative defaults; all configurable)
Max JSON control frame 64 KiB · max binary audio frame 1 MiB · max `speak.text` 16 KiB · max transcription upload 100 MB · `session.config` must arrive ≤ 10 s after connect (handshake deadline) · max connections per listener (default 4× max sessions) · reject-path rate limiting per key/peer. Connections ≠ sessions; both are bounded.

## 14. Gateway integration contract

### 14.1 Named gateway deltas (the honest list — r1 claimed "zero new gateway concepts"; GW-1..GW-5 are M1 scope, GW-6 lands M4 with in-proc, GW-7 lands M3 with the GA realtime dialect)
| Δ | Change | Why |
|---|---|---|
| **GW-1** | Extract `waav-gateway-provider-api` crate (provider traits + `STTResult`/configs + `PluginConstructor`); gateway binary gains optional `provider-waav-infer` feature | Resolves the inventory-registration dependency cycle (ADR-16) |
| **GW-2** | Per-model EP override / co-residency profile pinning VAD+turn models to CPU EP | §4.2 — current `auto` probes CUDA first on Linux |
| **GW-3** | Circuit breaker classifies typed provider-busy as **non-failure** (failover-only) | FR-G2 — rejection is normal at capacity; breaker-flap otherwise |
| **GW-4** | `BaseSTT::finalize(id)` default-no-op method; `BaseTTS::clear_context(context_id)` defaulting to `clear()` | Finalize barrier + per-context cancel have no gateway seam today (Deepgram Finalize exists only as an ack handler) |
| **GW-5** | `STTConfig`/`TTSConfig` gain a typed `endpoint`/`mode` extension (or `extra` map) | Closed structs can't carry `mode=sidecar`, UDS path, class |
| **GW-6** | In-proc bootstrap: global `OnceLock<InferEngine>` + engine-config source for the `infer-inproc` feature | Providers are constructed per-session via sync factories; the engine is a process singleton |
| **GW-7** | Realtime client migrates beta→GA OpenAI dialect | §13.3; gateway's own client speaks the deprecated dialect |

- **Provider adapter** (`waav-infer-provider`): implements `BaseSTT::{connect,send_audio,on_result,...}` and `BaseTTS::{speak,speak_with_context,clear,flush,on_audio,...}` over the session verbs (§13.1/§13.2), **with explicit type conversions**: the protocol crate defines its own serde wire types; the adapter owns `From`/`Into` to gateway `STTResult`/`AudioData` (gateway types are not serde and are referenced at ~33 call sites — literal type-sharing would be a gateway-wide refactor; a CI parity test enforces non-drift instead). `get_provider_info()` reports topology, engine version, model digest. `set_resilience` hooks the gateway's reconnect classification (sidecar restart ⇒ retriable connection error).
- **Boot coherence**: at provider startup, the adapter verifies configured model+voice exist and are Ready-able on the engine — mismatch is a **fail-fast config error**, never a silent 100%-failover (a renamed model must page someone, not quietly send all "local" traffic to the cloud).
- **Config**: provider config carries `mode`, `endpoint` (UDS path / URL), `model`, `voice`, `class`, plus engine-level knobs only in sidecar-supervision config (device budgets, warm set). Secrets: remote API key via env (placeholder-filter rules from the gateway audit apply).
- **Sidecar supervision** (when gateway-managed): spawn with config file and supervise **per the §4.2b lifecycle contract** — adopt a live sidecar via pidfile/instance-id; restart on **death/`livez`/deep-probe-confirmed-dead only, never `/readyz`**; startup budget from `waav_infer_model_load_seconds`; flap damping + crash-loop quarantine + post-restart probation. `/health/ready` is used solely to mark the provider available/unavailable for routing (failover engages while unready).
- **DAG**: `InferNode` sugar over the adapter; DAG timeouts apply unchanged; `STTResultData`/`TTSAudioData` map losslessly.
- **VAD/turn coupling (FR-G3)**: gateway VAD speech-start may pre-warm a session slot (admission reservation with TTL); turn-end / `is_speech_final` triggers `finalize{id}`; barge-in triggers `clear` with the existing context-id bookkeeping (P1.1 work in the gateway).
- **Conformance**: the adapter passes the gateway's mock + live provider suites; a live-e2e profile pins gateway→sidecar→catalog-defaults and asserts NFR-P7 overhead.

## 15. Observability & operations

(Backed by `spec_ops.md`; gateway conventions reused throughout — P-9.)

### 15.1 Health model
`/livez` = process only (never model state — the Triton #7014 / vLLM #6073 k8s kill-loop trap is documented and avoided; k8s `startupProbe` sized from observed load time). `/readyz` = required models Ready ∧ not draining — **saturation excluded by default** (admission's 503 owns shedding; opt-in `readyz.include_saturation` per FR-O1; strictness configurable, Triton `--strict-readiness` analog). Per-model readiness via `/v1/models` state + `waav_infer_model_state` gauge. **Deep probe** per FR-O3 (the GB10 zombie-GPU detector).

### 15.2 Drain & upgrades (honest about the single-GPU case)
Per FR-O4 + §4.2b. Stream handoff is a non-goal (no prior art: vLLM/Triton/Envoy all drain).
- **Multi-instance/multi-GPU**: zero-downtime = blue-green/rolling with capacity overlap + gateway failover.
- **Single-GPU node (the primary v1 target)**: true zero-downtime is **not possible** without capacity headroom — two full-size engine processes cannot both hold their ledgers on one device, and the new instance's warmup burst invalidates the old instance's calibration mid-overlap. The honest contract: **upgrade = drain (bounded by `drain.deadline`) → stop → start**, with a serving gap = warm restart-to-Ready (**≤ 60 s GPU tier**, NFR-R2) covered by gateway failover. For compliance-pinned deployments where cloud failover is disabled, this is a **declared maintenance window**, and the spec says so rather than pretending otherwise. (A future cross-process ledger-tranche handoff is a named-trigger evolution, not v1.)
- Model **version swap** within one process per FR-M7 (no-loss in-place reload) — this covers the common "new model rev" case without any process restart.

### 15.3 Backpressure & admission (ops view)
Bounded queues everywhere; quota unit = **concurrent streams** (Deepgram precedent — the voice-native unit); rejects are 429 (quota) vs 503 (saturation/draining) + `Retry-After`; `vllm:num_requests_waiting`-style gauge with the standard "waiting > 0 sustained = saturated" alert.

### 15.4 Metrics (normative set; namespace `waav_infer_*`)
`ttfa_seconds` `ttft_seconds` (histograms) · `stream_rtf` (histogram) · `streaming_viability_ratio` · `stalls_total` `underruns_total` · `streams_running` `requests_waiting` (gauges) · `queue_time_seconds` · `batch_occupancy` · `kv_pool_usage_ratio` · `model_state{model,state}` · `model_load_seconds` (histogram — startupProbe sizing depends on it) · `stage_step_seconds{model,stage}` (histogram — the FR-S7 T_step series) · `pacing_buffer_seconds{model}` (min-level gauge) · `drain_duration_seconds` + `drain_overrun_total` · `probe_total{model,outcome=ok|fail|skipped_load}` · `admission_rejected_total{reason}` · `deadline_exceeded_total` · `tokens_total` `audio_seconds_total` · `backend_active{backend,ep}` · reuse `waav_degraded_total{component,reason}` (new components: `infer_backend`, `infer_probe`, `calibration`, `artifact_cache`) · `batch_starved_seconds` (FR-O5). Conventions: underscores (vLLM's own postmortem on `vllm:` colons), `_seconds`/`_total`, histograms-not-summaries, bounded labels, `model` label everywhere, **never per-stream labels**, monotonic-clock intervals (gateway turn_profile.rs already does ns-monotonic). New metrics use `_seconds` even though legacy gateway histograms are `_ms` — the wart is noted, not "fixed" by renaming. GPU hardware telemetry delegated to DCGM-exporter on NVIDIA.

### 15.5 Tracing & audio debug
OTel spans `infer.{admission,encoder,ar_decode,flow,codec_decode,vocoder}` parented under gateway turn-profiler stages, GenAI semconv attributes (`gen_ai.server.time_to_first_token` alignment); W3C traceparent across UDS. Audio debug per FR-O8 (30-s rings; `metadata_only` prod default excludes transcripts; `full` behind PII-ack flag; redact-before-store; 7-day TTL janitor; replay corpora are synthetic/licensed, never silently sourced from dumps). Logging per FR-O9.

### 15.6 Failure taxonomy (runbook skeleton; full table in ops note)
| Failure | Detection | Automatic response | Runbook |
|---|---|---|---|
| Model load fail | state=Failed, load error metric | fail_fast per policy; old version keeps serving on reload | inspect manifest/artifact digest |
| Backend degraded → CPU | `waav_degraded_total{infer_backend}`; RTF p99 → 1 alert | keep serving if SLO holds; else mark model Degraded | check EP/driver; silent-CPU-fallback inflates service time — alert is mandatory |
| Device OOM | impossible on request path (NFR-R3); load-time ledger error | admission/load rejected | resize warm set / budgets |
| Hung kernel / zombie GPU | deep probe timeout (FR-O3) | quarantine backend; sidecar: crash-only restart | GB10-known (vLLM #41725); driver/firmware |
| Stuck stream | progress watchdog (FR-O5) | cancel stream, free resources, typed error | inspect dump bundle |
| Codec underrun | `underruns_total`, viability ratio | codec-stage budget rebalance; shed Batch | the codec stage is frequently the true bottleneck |
| Queue saturation | `requests_waiting` sustained | 503 + Retry-After; gateway failover | scale out / shrink warm set |
| Drain overrun | drain deadline metric | hard-cancel + clean close | lengthen `drain.deadline` for long calls |

### 15.7 Disk lifecycle
Device-artifact caches: LRU GC by atime with a disk watermark (default: keep < 80% of the cache volume; stale SDK/driver keys evicted first); dump bundles capped (count + bytes) with the TTL janitor; `ENOSPC` on cache write ⇒ model still serves on portable artifacts + `waav_degraded_total{component="artifact_cache",reason="enospc"}` (never a serving failure); blob store GC = `waav rm` + orphaned-blob sweep with grace period.

## 16. Security, licensing, compliance

### 16.1 Process & API security
UDS 0700-dir/0600-socket + optional `SO_PEERCRED` allowlist; TCP defaults loopback, hard-errors on non-loopback-without-auth; admin verbs on a separate socket/admin-scoped key; key rotation without drain; TLS 1.2 floor (§13.4). No payload persistence by default (FR-D3); ingress limits (§13.6) bound the pre-admission parse surface. The engine never holds cloud-provider secrets (gateway's domain).

### 16.2 Supply chain & model ingestion
Signed manifests (OMS/sigstore) with **offline-capable verification** (embedded bundles, pinned trust root + staleness policy, issuer/identity pinning, org re-sign flow, fail-closed) per FR-M5; `model_policy = signed_only` recommended in production; unsigned `hf://` pulls gated, labeled, digest-pinned. **Model files are attack surface even without pickle**: the GGUF parser CVE lineage (heap overflows 2024–2026) and the ONNX external-data path-traversal lineage (CVE-2022-25882 → incomplete fixes) are why (a) external-data path resolution MUST canonicalize-and-confine within the blob dir, (b) ORT sessions MUST NOT register custom-op libraries from any model/manifest-controlled path, and (c) the §18.4 chaos suite includes a **malformed-model fuzz corpus** (GGUF KV/tensor headers, ONNX external-data refs, safetensors header JSON) alongside the audio fuzz.

### 16.3 Vulnerability management (the pinned-deps duty)
The dependency strategy (vendored moshi-core, exact-pinned llama-cpp-2/ort wrapping large C/C++ codebases) trades churn for drift — so the process is normative: `cargo-audit` + `cargo-deny` (advisories *and* licenses) on every PR; an upstream-CVE watch with a **security re-pin SLA (critical CVE → re-pin ≤ 7 days)**; SBOM (CycloneDX) per release artifact; `SECURITY.md` with coordinated disclosure. Risk-register row added (§20).

### 16.4 Engine licensing posture
Apache-2.0 codebase; required runtime deps permissive. **espeak-ng (GPL-3.0) runs ONLY as a separate helper process** (`waav-g2p-espeak`, itself GPL-licensed, distributed as a separate optional artifact with corresponding-source links, talking over pipes/UDS — the FSF "separate programs at arms length" line; **the r1 "process/dylib" disjunction is gone: a dylib boundary is not a defensible GPL boundary**). misaki/raw-text paths are the defaults precisely so most deployments never need it. LGPL at the edge: mp3 egress via dynamically-linked LAME behind a feature, with notices (FR-E9). abi_stable (gateway plugin ABI) is unmaintained — pinned, stabby named as successor (risk register).

### 16.5 Model licensing & attribution (full obligations, not SPDX strings)
Every manifest carries the `[license]` block (FR-M1): SPDX + upstream license + **required attribution text** + notices + modification statement + use restrictions. Concretely for catalog v1: `orpheus-3b` is a **Llama-3.2-Community-License derivative** (upstream Apache-2.0 relabel notwithstanding) — "Built with Llama" display, license copy, naming and AUP flow-down ship in the manifest and surface in `waav show`/`/v1/models`; `parakeet-tdt-0.6b-v3`/`mimi`/`moshi` are CC-BY-4.0 — attribution + modification indication surface the same way, and mirror repos carry license + NOTICE files (WaaV is a redistributor). NC models excluded or gated; per-voice licenses honored (piper precedent). The pull-time policy engine evaluates the obligation chain, not the tag.

### 16.6 Synthetic-media provenance & watermarking (EU AI Act Art. 50(2) applies 2026-08-02)
- **Truthful provenance is the floor**: every TTS response carries `generated: true` in `chunk_meta`/REST metadata; the model digest is carried **session-level in `ready`** (REST: a response header) — together they satisfy the machine-readable provenance pair (field placement per §13.2). `provenance.watermark = {upstream_declared, applied_by_engine}` — the engine **never claims a watermark it did not apply** (Chatterbox's Perth marker is reference-implementation post-processing, not part of the model graph; serving the weights does not watermark the audio).
- **An engine-level watermark component hook** (Perth-class, post-vocoder, available to *all* TTS models when enabled) ships at M3 so operators subject to Art. 50(2) machine-readable-marking obligations have an in-band mechanism for realtime PCM; porter-kit verify includes "declared watermark is detectable in engine output." Responsibility mapping documented: the deploying operator is the Art. 50 provider; the engine supplies truthful provenance fields + the marking capability. C2PA-style signed metadata for file-producing endpoints is a roadmap note.

### 16.7 Biometrics, PII & cloning governance
- **Speaker embeddings are voiceprints** (GDPR Art. 4(14)/9; BIPA): the SpeakerEmbeddingCache is **in-memory by default**, session/tenant-scoped, TTL-bound, with `DELETE /v1/voices/{id}` as the deletion verb; persistence is opt-in and documented for operator DPIAs.
- **Cloning policy**: `conditioning.cloning = enabled | builtin_voices_only` (FR-E10), default conservative for the standalone server; optional consent-attestation field on conditioning inputs, logged.
- Debug audio per FR-O8 (`metadata_only` excludes transcripts; `full` behind explicit PII-ack; TTL janitor); calibration/replay corpora synthetic or licensed only.

### 16.8 Multi-tenancy
Out of scope in the engine (single trust domain per instance); tenancy/quotas/billing live in the gateway (§2.2).

---

# Part III — Engineering

## 17. Rust implementation

(Backed by `spec_rust_impl.md` + `code_rust.md`; mistral.rs studied as the serving-shell blueprint.)

### 17.1 Workspace (normative)
```
waav-infer/
├── crates/
│   ├── waav-infer-protocol    # serde wire/result types shared with gateway provider (no deps beyond serde)
│   ├── waav-infer-core        # engine: sessions, stages, scheduler, ledger, lifecycle  [zero C/C++ deps]
│   ├── waav-infer-components  # DSP, codecs, vocoders, CFM, tokenizers, samplers, glue  [zero C/C++ deps]
│   ├── waav-infer-backend-ort     # Path A adapter (ort 2.0 pinned, load-dynamic)   [#[cfg] allowed here only]
│   ├── waav-infer-backend-candle  # Path B adapter (official candle, vendored moshi-core)
│   ├── waav-infer-backend-ggml    # Path B-edge adapter (llama-cpp-2, dynamic-backends)  [feature]
│   ├── waav-infer-models      # manifests, store, pull/resolve, porter-kit lib
│   ├── waav-infer-server      # waav-infer serve: native WS + OpenAI compat + control plane
│   └── waav-infer-provider    # gateway adapter — depends on waav-gateway-provider-api (GW-1) + protocol crate
└── bins: waav (CLI = models+server front), waav-infer (serve), waav-g2p-espeak (GPL helper, separate artifact)
```
Rules: `-core`/`-components` build with **zero C/C++** in the dependency graph (pure-Rust testability floor; `tokenizers` is used with `default-features = false` — its default `onig`/`esaxx` features pull C/C++); `#[cfg(feature)]` legal only inside `-backend-*`; everything else uses runtime capability queries (NFR-H2). Forbidden: fork-pinned candle (ADR-4); new Rust plugin ABIs for backends (ADR-5).

**Dependency-graph truth (replaces r1's false "separate workspace ⇒ feature-unification containment"):** Cargo unifies features across the *build graph*, not workspace boundaries. The invariant that actually holds: the gateway's **default** build graph depends only on `waav-gateway-provider-api` + `waav-infer-protocol` + a thin UDS/WS client (no engine, no backends). The **`infer-inproc`** profile links the engine into the gateway binary and is an explicitly **co-versioned build**: ort/tokenizers/cudarc pins must match across both trees (documented co-pinning rules), one process-global ORT env, one `ORT_DYLIB_PATH` owner (the engine's EP policy; the gateway's aux models ride it). That coupling is the stated price of T2 — another reason T1 sidecar is the default.

### 17.2 Threading model (normative)
- Each engine stage = dedicated named OS thread + bounded typed channels (mistral.rs engine-thread + vLLM-Omni stage pattern). The scheduler tick loop is synchronous; **no GPU work or device sync on tokio worker threads** (executor starvation).
- All device state is **thread-affine**: warmup, CUDA-graph capture, teardown happen on the owning stage thread (cuTile/CUDA-graph caches are thread-bound in practice — mistral.rs precedent).
- `spawn_blocking` only for bounded one-shot ops (ORT session create — the gateway already does this).
- Cancellation token in every stage message, checked per AR step (FR-E4).
- `audio_thread_priority` (realtime class) at the PCM emit edge only.
- Server layer (axum/tokio) communicates with stages exclusively via the bounded channels; the channel boundary is also the backpressure boundary (FR-S5).

### 17.3 Reuse-vs-build table (engineering economy)
| Piece | Decision |
|---|---|
| KV cache, GGUF/QMatMul, samplers, Device/DType | **Reuse candle** (`KvCache`, `RotatingKvCache`, `LogitsProcessor`, quantized) |
| Streaming transformer, StreamMask batch, Mimi codec, depth-transformer | **Vendor moshi-core** (MIT/Apache) — includes a **candle 0.9→0.10 port** (upstream pins 0.9.1), budgeted in M2 |
| ONNX runtime + EPs | **Reuse ort** (pinned =2.0.0-rc.x; rc API moves — re-pin deliberately) |
| GGUF breadth, Vulkan, Hexagon | **Bind llama.cpp** via llama-cpp-2 (pin exact + record vendored SHA; no semver upstream) |
| **Batched streaming AR backbone for Llama/Qwen-class** (per-slot positions, masked attention, per-slot scattered KV, weight mapping, parity goldens) | **Build** — *the largest single M2 item*: candle's `(quantized_)llama` decodes the whole batch at one scalar `index_pos` (unusable for B independent streams at different positions), and moshi-core's batched machinery is implemented for Kyutai's transformer — serving Llama/Qwen backbones in fixed slots means porting those architectures into the StreamMask/scattered-KV framework. On M2's critical path; named in §20 risks |
| Fixed-slot scheduler, ledger, lifecycle, manifests, protocol, porter kit | **Build** (the rest of the genuinely new code) |
| Whisper kernels | candle-transformers whisper (exists) on Path B, or CT2-converted ONNX on Path A — decided by M4 bench |
| **Codec decoders** (SNAC, DAC, EnCodec, Mimi) | **Extract from candle-transformers** (`snac.rs`/`dac.rs`/`encodec.rs`/`mimi/` — in-tree Rust on the chosen candle) into `waav-infer-components` + **add the sliding-window streaming wrapper** (only Mimi is already streaming). Was a build; now mostly verification. (INFER_REUSE §1) |
| **STT-A frame-sync** (CTC/TDT + cache-aware streaming + EOU endpointing) | **Vendor/extract `parakeet-rs`** (MIT/Apache, same EP set) feature-gated — implements the catalog STT-A path; **resolves OQ-4**. Port sherpa-onnx `endpoint.cc` (3-rule) + `context-graph.cc` (Aho-Corasick biasing, FR-A6) + the finality struct. Shrinks "the largest single M2 item." |
| **DSP frontend** (mel/STFT/iSTFT) | **Reuse `kaldi-native-fbank` (Rust)** or realfft+rustfft+`mel_spec` (mel_spec is bit-checked vs whisper.cpp → the golden oracle) |
| **Euler-CFM solver + DiT estimator** | **Build the ~40-LOC host-side `solve_euler`** (reference: Matcha-TTS / CosyVoice2 `CausalConditionalCFM`) using **candle `mmdit`/`flux` DiT blocks** (AdaLN/modulate exist); the estimator is the ONNX/candle graph (§10.2). CosyVoice2's chunk-aware flow cache (−34-frame overlap) is the streaming-equivalence enabler. |
| **CosyVoice2 Flow+HiFT (M2 P1+P2 model)** | **ONNX via `CosyVoiceForOnnx`** (Apache, ships the fp16-estimator-NaN fix) — removes the single hardest M2 porting blocker |
| **P3 one-shot (Kokoro)** | **Extract Kokoros/kokorox modules** (Rust, Apache, ONNX fwd + chunk + stream + OpenAI server) — pulls the M0 CPU-floor gate + M1 first-audio forward |
| **G2P / TN / segmenter** | **Reuse:** misaki-rs (GPL-free, `default-features=false`) for kokoro; **icu_segmenter + srx** for FR-E8 (UAX-29 + CJK/Thai + abbreviation rules); **wetext-rs + rustfst** for pure-Rust WFST TN/ITN; CharsiuG2P/DeepPhonemizer→ONNX for multilingual phonemes (shrinks the GPL surface to legacy Piper only); jpreprocess (ja), jieba-rs+pinyin (zh) |
| **Packaging / registry / supply-chain** | **Assembly of mature crates:** hf-hub, safetensors, candle `gguf_file`+`tensor-tools`, sigstore-rs (+prefix-dev for DSSE), oci-client (deferred), cargo-deny/audit/cyclonedx, clap/indicatif/comfy-table/minijinja. Net-new: the manifest schema + per-target resolver + signed-catalog-index + a resilient hf-hub download wrapper (reqwest-retry + Range-resume) |
| **Standalone server / API / ops** | **Reuse:** async-openai (types), axum + tokio-tungstenite + tower-governor + utoipa (OpenAPI/AsyncAPI gen), metrics-exporter-prometheus, opentelemetry-otlp + tracing-opentelemetry. **Copy the shape** of TEI (Backend trait + token-budget batching + disconnect-detect), moshi-server (scheduler + finalize-barrier heap), mistralrs-server-core (router) |
| **Telephony / multi-rate audio** (FR-D, §FR-E9) | **Reuse:** rubato (lift gateway `StreamResampler`), `audio-codec-algorithms` (G.711, 0BSD), `opus` crate (one codec spans 8/12/16/24/48 kHz), Symphonia/hound/cpal/rtrb, nnnoiseless / DeepFilterNet(ort) for NS |

> The full reuse decision set (~140 artifacts, license tier, maturity, milestone impact, and the
> battle-tested edge cases to import) is in **`WaaV/inferv2/INFER_REUSE.md`**. The two production Rust serving
> shells to mirror are **TEI** (candle+ORT dual-backend, Apache) and **moshi-server `batched_asr.rs`**
> (the literal fixed-slot scheduler, Apache); both are reference-pattern (TEI: liberal; moshi-server:
> mirror the loop, vendor moshi-core). lmdeploy "persistent batch" + Triton sequence-batcher are
> independent production validations of the fixed-slot design (KISS confirmation).

### 17.4 Distribution & binary size
One engine binary per OS/arch + runtime-loaded accelerator bundles (ORT dylib + `libggml-*` set): CPU/Vulkan bundles 10–40 MB; CUDA bundles 150–400 MB (+cudart). Keeps the artifact matrix linear in OS/arch. `glibc ≥ 2.39` floor for ort prebuilts documented; CUDA 12.x/13.x dual-toolkit policy (GB10 = CUDA 13/sm_121).

### 17.5 CI matrix (aligned with the NFR-H tiers)
Tier 1: linux-x64 (CPU gate on every PR; CUDA 12.x image), **GB10 self-hosted runner** (source-built ORT + llama.cpp; nightly perf trend gate on TTFA/RTF), macOS-arm64 (GH-hosted M-class = correctness; self-hosted M-Pro-class = perf trend, per NFR-H), and **cloud L4 + L40S runners for the NFR-P1 anchor gates** (the M2 gate's measurement substrate — the spec's measured-not-modeled ethos applies to its own acceptance numbers; the H100 anchor is a reference verified opportunistically, gated by no milestone). Tier 2 (build + smoke): windows-x64, ROCm/MIGraphX, aarch64-Vulkan, **Intel OpenVINO**. Tier 3 (compile-check only, matching NFR-H): android-arm64+Hexagon (NDK + Hexagon SDK; `libggml-htp-v{73,75,79,81}`). Release gates: conformance suite (§10.5), chaos suite (§18.4), catalog verify on Tier-1 devices, cargo-audit/deny (§16.3).

## 18. Quality methodology

1. **Porter-kit verify gate** (FR-M6) defines model support, with **stated initial bounds** (tightened with porter experience, versioned in the kit): STT WER delta ≤ 0.5 abs on the golden set; TTS speaker-similarity cosine ≥ 0.85 vs reference-implementation output, UTMOS-proxy delta ≥ −0.3, ASR-round-trip WER delta ≤ 1.0 abs; streaming-equivalence: chunked-vs-full log-mel distance within the per-family bound (also gates the NFR-P1 short-first-window ramp); per-quant re-verify; calibration capture. **A manifest lint rejects internally-inconsistent resource blocks**: rated streams/TTFA violating `T_step(B) ≤ S·T_frame`, or `first_window_tokens·T_step(b1) + codec/transport floor > ttfa_p90_ms` (the window defaults to the model's steady window, e.g. 28, only when no ramp is declared — the lint must honor the same ramp the TTFA claims use).
2. **Goldens**: token-exact per pinned (backend, quant, target) triple; **perceptual** audio goldens cross-backend (log-mel distance bounds — never bit-exact across backends/EPs).
3. **Property tests** (proptest): scheduler invariants — no starvation, KV-slot accounting closes, mask/slot ordering, ledger arithmetic never negative; loom reserved for hand-rolled lock-free structures only.
4. **Chaos suite** (release-gated, runs on GB10 + linux-x64): (1) paced concurrent-stream soak 30 min at rated concurrency — TTFA p99 ≤ budget, zero underruns — **including the mixed-model variant (STT + TTS at joint rated load on one GPU, exercising the §8.3c duty ledger)**; (2) saturation ramp — fast rejects with Retry-After while admitted streams hold SLO **and the gateway breaker stays Closed** (GW-3); (3) drain-under-load — zero streams lost pre-deadline; (4) `kill -9` mid-stream — gateway failover inside existing reconnect budget; (4b) **crash loop** — repeated kills at 30–60 s intervals: flap damping engages, session loss bounded, traffic settles on fallback, exactly one page (§4.2b); (5) injected hung kernel — watchdog + quarantine proven; (6) jitter/loss on the **TCP** transport via netem + a UDS slow-proxy shim for T1 (netem cannot shape AF_UNIX); (7) malformed-audio fuzz — typed errors, zero panics; (8) **malformed-model fuzz** (GGUF/ONNX/safetensors headers + external-data refs) — typed errors, zero panics, no path escape (§16.2).
5. **Benchmark harness**: a paced load generator with synthetic/licensed corpora (the replay harness shares its engine but dump-bundle replays stay inside the dump-policy domain — FR-O8); calibration tables are benchmark outputs; nightly GB10 trend gates.
6. **Gateway conformance**: provider suites + `OPENAI_BASE_URL` live-e2e pointed at Infer (FR-A5, FR-G4).

## 19. Roadmap & milestones (acceptance-gated)

| M | Scope | Acceptance gates |
|---|---|---|
| **M0 Foundations** | Workspace + GW-1 provider-api extraction; protocol crate; backend trait + ORT/candle adapters; DeviceCaps + bandwidth microbench; ledger; manifest v1 + store + pull; compute-type ladder; ort-sys arena shim | Unit+proptest green on linux-x64 CPU; manifest pull/verify round-trip; CPU floor proven: kokoro ONNX forward on CPU via Path A from precomputed phoneme inputs, golden-audio match (text→speech e2e is M1's clean-machine gate); manifest lint live |
| **M1 First audio** (externally adoptable: Speaches-parity; deliberately lean — r2's M1 was over-stuffed) | STT-A **parakeet** (batch + documented chunked streaming) + TTS-P3 **kokoro** (misaki G2P — no GPL helper needed) on Path A; **OpenAI-compat REST (`/v1/audio/speech`, `/v1/audio/transcriptions`, `/v1/models`) + egress encoders** (FR-E9); sidecar `serve` + native WS v1 + supervised-lifecycle contract (§4.2b); provider adapter (STT+TTS) + gateway deltas GW-1..GW-5; health/drain/metrics/logging core; fixed-slot scheduler (micro-batch stages); text segmenter; **`waav run` defined** (TTS: text→device/file with TTFA printed; STT: file/mic with streaming partials — mic via cpal, the demo UX, cut to file/stdin if M1 slips; egress encoders stay in M1 because the openai-SDK smoke's default response_format is mp3; auto-starts/attaches to the local server). *Moved out of M1: piper voices (M2 — they drag the GPL espeak helper), GW-6/in-proc bootstrap (M4 with the U4 edge persona), cache-aware streaming STT + NFR-P3 (M2, gated on OQ-4 license check with fallback = sherpa-class streaming zipformer)* | NFR-P2 (GPU + CPU-honest) on Tier-1; **clean-machine gate: `waav pull kokoro && waav run kokoro "hello"` → audio in < 60 s on a fresh Tier-1 box, measured in CI**; gateway live-e2e voice loop via local provider; crash-containment chaos (4) passes; openai-SDK smoke vs compat REST |
| **M2 AR substrate** | Batched streaming AR backbone (the §17.3 build item); P1 (orpheus, flat-SNAC, int4 + short-first-window ramp) + P1+P2 (cosyvoice2: Euler-CFM + HiFT) on candle; fixed-slot AR scheduler + **local calibration incl. the §8.3c device duty ledger** + admission; streaming codec stage; per-slot KV; warmup-gates-Ready; **cache-aware streaming STT model + NFR-P3**; piper voices (+GPL helper process); **whisper batch transcription** (compat endpoint); **cloning v1** (reference-audio conditioning, cosyvoice2) | NFR-P1 anchors on rated pairs; NFR-P3; **mixed-model soak (STT + TTS concurrently at joint rated load on one GPU — the actual voice-loop workload)**; capacity tables published from calibration; saturation chaos (2) passes incl. breaker-stays-Closed; barge-in cancel ≤ 1 tick |
| **M3 Serving maturity (= catalog "v1" GA)** | OpenAI-compat realtime-transcription WS (GA dialect); keep_alive lifecycle + admin API; deep probe + crash-loop damping; Batch class piggyback + jobs API; chatterbox + watermark component hook (§16.6); voice registration (`/v1/voices`); gateway realtime-client GA migration (GW-7); **official Pipecat service plugin + Python quickstart client** | openai-SDK CI green; full chaos suite (1–8, incl. crash-loop 4b validating the §4.2b(3) machinery shipped at M1); 72-h soak (NFR-R5); model-matched NIM comparison bench published; **Pipecat plugin gate: STT+TTS demo pipeline green against `waav-infer serve`** |
| **M4 Breadth** | STT-B whisper **streaming** (LocalAgreement; CT2-vs-candle decision); ggml backend (GGUF, Vulkan); EP widening (OpenVINO/MIGraphX/QNN-static encoder artifact); biasing for Whisper (initial-prompt); **in-proc bootstrap (`infer-inproc` + GW-6, U4 edge persona)**; porter kit GA; **LiveKit Agents plugin; Wyoming bridge; browser/JS client for the native WS**; Tier-2 targets | Tier-2 smoke green; whisper streaming-equivalence; one community model onboarded via porter kit only; LiveKit plugin demo; in-proc smoke on a CPU-tier model + T2 warning-label docs |
| **M5 S2S & edge AR** | moshi duplex (BaseRealtime provider; realtime GA full sessions); **ggml-Hexagon AR experimental scoped to ≤ 1B low-steps/s class (cosyvoice2-class / Mimi-class — Orpheus-class is bandwidth-excluded on phones)**; STT-C scoping | Duplex demo at **calibrated** concurrency on L40S-class (reference O(8–16), not the r1 misquote); Hexagon prototype TTFA measured on the scoped class |

Sequencing rationale: M1 is externally adoptable on day one (compat REST + CLI first-run + gateway provider) while M2 builds the differentiating substrate; everything after composes (P1 substrate ⇒ STT-B/C, P2/P4, S2S — per `learnings.md` §14). Distribution deliverables (Pipecat/LiveKit/Wyoming) are roadmap line-items with acceptance gates, not hopes.

**Reuse acceleration (per `INFER_REUSE.md`).** M0/M1 are now overwhelmingly *assembly of mature parts*: kokoro P3 (Kokoros modules), parakeet STT-A (parakeet-rs — resolves OQ-4), the DSP frontend (kaldi-native-fbank), the G2P/TN/segmenter stack (misaki-rs + icu_segmenter + srx + wetext-rs), the server skeleton (TEI/moshi-server shape + async-openai + axum + utoipa), the registry (hf-hub + sigstore-rs + cargo-deny + clap/indicatif). M2's codec stage extracts candle-transformers' in-tree SNAC/DAC/Mimi; its CFM stage references Matcha/CosyVoice2 + candle `mmdit` and uses CosyVoiceForOnnx (fp16-fix). The scheduler mirrors moshi-server `batched_asr.rs` (extract-module). The irreducible build stays the batched Llama/Qwen backbone + duty-ledger admission + calibration + manifest schema + protocol. **Telephony/8 kHz (FR-D1) lands in M1–M2 as a config lane** via rubato + audio-codec-algorithms + the `opus` crate — opening the contact-center self-host audience early.

## 20. Risks & mitigations (top; full register grows in review)
| Risk | Severity | Mitigation |
|---|---|---|
| GB10 source-build fragility (no prebuilts ecosystem-wide; CUDA 13/sm_121) | High | Self-hosted runner from M0; cached toolchains; documented build recipe; Tier-1 nightly gates |
| ort rc-status API churn | Med | Exact pin; deliberate re-pin PRs; backend isolation makes swaps local |
| llama-cpp-2 no-semver / upstream velocity | Med | Exact pin + vendored SHA; feature-gated; Path A covers catalog defaults without it |
| Fixed-slot scheduler idles capacity on bursty TTS | Med | Measured trigger → EDF evolution (§8.4) is designed, not speculative |
| Hexagon timeline (QNN constraints; ggml-HTP experimental) | Med | Tiered promise (Tier 3); QNN static-encoder path first; CPU floor always works |
| Catalog model licenses shift / upstream repos vanish | Med | Org mirrors + provenance pins; license policy at pull time; lock files |
| Euler-CFM numerical parity across backends | Med | Perceptual goldens + per-backend calibration sets; estimator stays Path-A ONNX where possible; **inherit the CosyVoiceForOnnx fp16-estimator-NaN fix + the "pre-alloc-not-concat / dtype-discipline" TRT memory-format rules baked into CosyVoice/Chatterbox source (INFER_REUSE §3)** |
| Model-file parser CVEs (GGUF/ONNX) in the loader | Med | §16.2 canonicalize-and-confine paths + size-validate-before-alloc + no model-controlled custom-op dylibs; §18.4(8) fuzz corpus seeded from the active CVE PoCs (CVE-2025-53630 + bypass; ONNX external-data CVE-2022-25882→2024-27318→2026-27489) |
| **Reuse-dep maintenance/license drift** (parakeet-rs single-maintainer; misaki-rs/wetext-rs pre-1.0; opus/Symphonia licenses) | Low–Med | Exact-pin + vendored-SHA the load-bearing ones; cargo-deny license+ban gate (§16.3); each has a named fallback (parakeet-rs→sherpa-pattern port; misaki-rs→CharsiuG2P-ONNX; Symphonia-opus→`opus` crate). Reuse is *acceleration*, never a single point of failure — the build path exists for each |
| abi_stable unmaintained (gateway plugin surface) | Low | Pinned; stabby migration noted; Infer doesn't depend on it for backends |
| Codec stage becomes the bottleneck under load | Med | Separately batched + budgeted stage; underrun telemetry; rebalance knob |
| **Batched streaming AR backbone** (the §17.3 build item) slips M2 | **High** | Earliest-start in M2; moshi-core ScatteredKvCache as the proven base; parity goldens vs reference impls; CosyVoice2 (0.5B, 25 Hz — easier budget) lands before Orpheus |
| CUDA-graph capture on candle proves hard | Med | Eager fallback is normative (FR-S2); capacity rated on the measured path; capture is headroom, not viability |
| Vendored/pinned dep CVE lag (llama.cpp/ort/moshi-core) | Med | §16.3: cargo-audit/deny per PR; CVE watch + ≤ 7-day re-pin SLA; SBOM per release |
| Calibration drift (thermal/co-tenant/driver) voiding admission | Med | FR-S3b online EWMA + shed-don't-smear; keyed invalidation + recalibrate-on-Warming |

## 21. Decision log (ADRs — alternatives were genuinely evaluated; reversal requires named triggers)
| ADR | Decision | Rejected alternatives → why |
|---|---|---|
| ADR-1 | Hybrid two-path backend (ORT + native-AR) behind one trait | Pure-ORT (AR/KV misfit); pure-compiler TVM/IREE/MLC (dynamic shapes, ops cost); single-framework burn (no AR serving stack, API churn — revisit if burn ships GGUF inference) |
| ADR-2 | **Sidecar default**; in-proc feature-gated for edge | In-proc default (CUDA sticky errors + SIGABRT uncatchable — crash physics); remote-only (edge needs in-proc; UDS latency is free) |
| ADR-3 | Fixed-slot masked-batch scheduler first; EDF later behind a measured trigger | vLLM-style continuous batching + paged everything (long-context machinery voice doesn't need); EDF-first (unproven complexity before evidence) |
| ADR-4 | Official candle pinned; **forks forbidden**; moshi-core vendored | mistral.rs as dependency (pins forked candle); hand-rolled kernels (moshi-core exists, MIT/Apache) |
| ADR-5 | Backends = compile-time features; runtime dynamism via ORT_DYLIB_PATH + GGML_BACKEND_DL only | Rust plugin ABI for backends (hot zero-copy tensor traffic is the wrong profile; abi_stable unmaintained) |
| ADR-6 | Native protocol = WS-style JSON control + raw binary audio over UDS/TCP | gRPC primary (no browser bidi; only Google/Riva chose it; every voice vendor chose WS); msgpack (debuggability; gateway dialect precedent) |
| ADR-7 | Static signed catalog + HF Hub + mirrors; **no registry server** | ollama-style hosted registry (infra cost; HF storage suffices; ollama's own hf.co passthrough proves it) |
| ADR-8 | OpenAI-compat as adapters, native protocol as source of truth | Compat-only (no word timestamps / finality fidelity); vendor emulation (Deepgram/11L wire clones — maintenance trap) |
| ADR-9 | Reject-don't-degrade admission from measured calibration tables | Best-effort admission (measured 259 ms→9.8 s cliff); analytic capacity models (Clockwork doctrine: measure) |
| ADR-10 | No live session migration; drain + gateway failover | KV/stream handoff (no prior art anywhere; complexity unbounded) |
| ADR-11 | Portable encodings distributed; device-locked artifacts generated+cached on-device | Per-SoC artifact distribution (Qualcomm AI Hub cautionary tale; matrix explosion) |
| ADR-12 | Voice-sized KV = **per-slot fixed-quota buffers** (ScatteredKvCache pattern); no paged blocks, no CoW/radix/swap; no spec-decode; no P/D disagg; no TP/PP ≤ 4B; **no `Paused` state** | vLLM-parity feature set incl. block-paging (wrong regime: short slot-bound KV, small models, rate-SLO; paging needs gather kernels candle lacks — only exists on a forked candle, forbidden by ADR-4). Triggers to revisit: variable very-long prompts at scale (paging); fleet evidence that unload/reload churn hurts and weights-resident/pools-released would pay (Paused/sleep-L1) |
| ADR-13 | espeak-ng isolated as a **separate helper process only** (pipes/UDS, own GPL artifact), default-off; raw-text & misaki defaults | Linked or dlopen'd GPL dependency (dylib is not a defensible GPL boundary — FSF FAQ); dropping phoneme models (catalog needs piper/kokoro) |
| ADR-14 | `steps_per_second` + calibration block as required manifest fields; **admission reads local tables only** | Capacity by experiment-per-deploy (operators can't plan); manifest-rated admission (violates measured-not-modeled on unrated devices) |
| ADR-15 | Codec policy: engine core PCM-only; transcoding at the server/adapter edge (Symphonia ingress; opus/flac native, mp3 feature-gated LAME, aac unsupported) | Codecs in core (pollutes the zero-C/C++ floor); no compressed egress (breaks OpenAI-compat default = mp3) |
| ADR-16 | Extract `waav-gateway-provider-api`; gateway gains `provider-waav-infer` feature | Inventory registration from an external crate (Cargo dependency cycle — r1's design did not build); in-tree adapter (couples release trains); abi_stable dynamic plugin as primary (unmaintained ABI crate for a hot path) |
| ADR-17 | Single-GPU upgrade = drain→restart with declared gap; zero-downtime claims reserved for multi-instance | Cross-process ledger tranche handoff (no prior art, complexity unbounded — named-trigger evolution); pretending blue-green works on one device (it cannot hold two ledgers) |

## 22. Open questions (tracked to critique rounds)
1. Whisper path: CT2-ONNX vs candle-native — decide by M4 benchmark (quality of int8 + streaming-equivalence both ways). *(Whisper batch lands M2 via whichever passes verify first; the M4 decision governs streaming.)*
2. CosyVoice2 flow estimator placement: ONNX (Path A) vs candle port — parity + chunk-cache ergonomics **+ tensor-interop/stream-sync cost under the §10.2 residency rule** decide in M2.
3. Session pre-warm reservations from gateway VAD speech-start: TTL + accounting semantics (avoid reservation leaks).
4. Streaming STT-A realtime default: confirm nemotron-streaming-class license/quality (cache-aware checkpoint required — parakeet-tdt-v3 is the batch model; NFR-P3 depends on this choice). **Fallback if license fails: sherpa-class streaming zipformer (Apache-2.0).** Decide by M2 (the NFR-P3 gate moved to M2 with this dependency).
5. ~~Apple tier~~ — **resolved into NFR-H Tier-1 macOS scoping** (r2).
6. ~~Batch-class API~~ — **resolved: minimal async-jobs endpoints** (`POST /v1/jobs/transcriptions`, `GET /v1/jobs/{id}`, M3), not OpenAI batch semantics; on drain, queued jobs are cancelled and running jobs cancel-and-requeue.
7. Short-first-window ramp quality floor per P1 family (7 vs 14 tokens; left-pad strategy) — M2 listening tests + FR-M6 gate calibration.
8. **New-feature roadmap (cheap adds the reuse sweep surfaced; sequence post-M3 by demand, `INFER_REUSE.md` §5):** speaker diarization DAG node (pyannote-rs/sherpa; CAMPPlus already in §11.2); real-time noise-suppression DAG node (nnnoiseless 16 kHz / DeepFilterNet-ort 48 kHz); semantic end-of-turn (smart-turn-v3 ONNX, license-clean); word/segment timestamps (faster-whisper DTW + moshi Marker); few-step distilled CFM (MeanFlow/CFG-Zero★/sway — manifest knobs, no engine change); multilingual VITS (MeloTTS 35-lang); `oci://` enterprise air-gap transport (oci-client); CycloneDX SBOM per model. Each is a manifest flag or a composable node, not core-engine work.

---

# Appendices

## Appendix A — Native WS v1 message sketch (GENERATED from `waav-infer-protocol`; field names below are the normative ones from §13.2 — one session per connection; `chunk_meta` immediately precedes its binary frame)
```jsonc
// → session.config
{"type":"session.config","model":"cosyvoice2-0.5b","task":"tts","class":"realtime",
 "language":"en","audio":{"encoding":"pcm16","sample_rate":24000,"channels":1},
 "conditioning":{"voice":"default"},"protocol_version":"1.0"}
// ← ready
{"type":"ready","session_id":"s_9f2","protocol_version":"1.0","model_digest":"sha256:..."}
// → speak / flush / clear      ← chunk_meta then its binary frame
{"type":"speak","text":"Hello there.","context_id":"c1","flush":true}
{"type":"chunk_meta","context_id":"c1","sample_rate":24000,"format":"pcm16","duration_ms":85,
 "generated":true,"alignment":[{"word":"Hello","start_ms":0}]}
{"type":"flush","id":"f1"}  →  {"type":"flushed","id":"f1"}
{"type":"clear","context_id":"c1"}  →  {"type":"cleared","context_id":"c1"}
// STT session: binary audio frames in → transcript events out
{"type":"transcript","text":"hello there","is_final":true,"is_speech_final":false,
 "is_finalized":false,"confidence":0.94,"language":"en","words":[/*...*/]}
{"type":"finalize","id":"t1"}  →  {"type":"finalized","id":"t1"}
// errors (taxonomy §13.5)
{"type":"error","code":"admission_rejected","message":"at rated capacity",
 "retriable":true,"retry_after_ms":1200}
```

## Appendix B — Manifest sketch (TOML; schema=1; numbers pass the §18.1 manifest lint — Orpheus budget: T_frame = 11.9 ms, bound = 9.52 ms at S=0.8; TTFA with the 14-token short-first-window ramp ≈ prefill + 14·t_step(b1) + codec + transport)
```toml
schema = 1
id = "orpheus-3b"            version = "0.1.0"
family = "orpheus"           task = "tts"
exec_path = "tts.p1"
languages = ["en"]
steps_per_second = 84        sample_rate_out = 24000

[license]                    # full obligation chain (§16.5), not a bare tag
spdx = "Apache-2.0"          # = upstream repo's DECLARED tag only; effective obligations come
                             #   from upstream_license + use_restrictions (FR-M5 evaluates the
                             #   chain, never this field alone — tooling beware)
upstream_license = "Llama-3.2-Community"          # derivative of Llama-3.2-3B
attribution = "Built with Llama"
notice_files = ["blobs/sha256-llamalicense"]
modifications = "int4/fp8 quantization; ONNX codec export"
use_restrictions = "Llama 3.2 AUP"

[capabilities]
biasing = false              alignment_events = false
cloning = "none"             language_detect = false

[serving]                    # codec-LM metadata (data, not code — FR-M1)
chat_template = "{{...jinja...}}"
audio_token_offset = 128266
frame_layout = "snac-flat-7"
stop_token_ids = [128258]
first_window_tokens = 14     # short-first-window ramp (NFR-P1), FR-M6-gated

[components]
codec = "snac-24k"           # engine-owned component ids
text_frontend = "raw-bpe"

[conditioning]
mode = "voice-prefix"        default_voice = "tara"
[[voices]]
id = "tara"  lang = "en"  license = "Apache-2.0"  preview = "blobs/sha256-..."

[resources]                  # reference-class PRIORS from `waav onboard verify`;
                             # admission uses LOCAL calibration only (FR-S3b)
kv_quota_tokens = 1024
# Reference t_step ladders (floor + measured overheads), per device class.
# Lint: every rated row must satisfy t_step(B) ≤ 0.8 × 11.9 = 9.52 ms AND
#       first_window_tokens × t_step(b1) + ~80 ms (prefill+codec+transport) ≤ ttfa_p90_ms.
t_step_ms.l40s = { b1 = 5.4, b4 = 6.8, b8 = 8.9 }   # floor ≈2.5 ms (2.15 GB ÷ 864 GB/s) → B_max 8
t_step_ms.gb10 = { b1 = 8.4, b4 = 8.9, b8 = 9.4 }   # floor 7.9 ms → B_max 8 (graph-assisted; eager → recalibrate)
t_step_ms.l4   = { b1 = 8.1, b2 = 8.8, b4 = 9.4 }   # floor 7.2 ms → B_max 4
rated = [
  { device = "l40s", precision = "int4", streams = 8, ttfa_p90_ms = 240 },  # 14×5.4+80 ≈ 156 ✓
  { device = "gb10", precision = "int4", streams = 6, ttfa_p90_ms = 330 },  # 14×8.4+80 ≈ 198 ✓
  { device = "l4",   precision = "int4", streams = 4, ttfa_p90_ms = 380 },  # 14×8.1+80 ≈ 193 ✓
]

[[artifacts]]                # per-target table; ≥1 cpu/any row (P-6)
component = "backbone"  format = "gguf"   precision = "q4_k_m"  target = "any"
class = "functional"    # CPU row = functional floor, NOT realtime-rated (P-6)
digest = "sha256:..."   size = 2147483648
[[artifacts]]
component = "backbone"  format = "safetensors"  precision = "int4"  target = "cuda"
class = "realtime-rated"
digest = "sha256:..."
[[artifacts]]
component = "codec"     format = "onnx"   precision = "fp32"  target = "cpu/any"
class = "realtime-rated"   # SNAC decode is realtime even on CPU
digest = "sha256:..."
```

## Appendix C — Glossary
TTFA (time to first audio) · TTFT (first token/transcript) · RTF (real-time factor; <1 = faster than realtime) · streaming viability (chunk i+1 before chunk i playout ends) · steps_per_second (AR steps per second of audio = codec frame rate × codebook scheme) · warm/Ready (post-warmup servable) · calibration table (measured T_step(B), capacity per device) · finality triple (`is_final`/`is_speech_final`/`is_finalized`).

## Appendix D — Research corpus index (all claims traceable)
`research/learnings.md` (architecture grounding; exec paths; components; staged engine; portability decision) · wave-1 notes: `serving_engines` `portability` `stt_models` `tts_models` `s2s_omni` `codecs_components` `code_omni_serving` `code_engines` `code_rust` `code_models` · wave-2 notes: `spec_competitive` `spec_deployment` `spec_api` `spec_packaging` `spec_memory` `spec_ops` `spec_scheduler` `spec_rust_impl` · **reuse notes (r4): `reuse_{rust_runtime, stt, tts, codecs_dsp, s2s_serving, flow_cfm, audio_io_telephony, packaging_registry, serving_api_ops, g2p_text}` → synthesized in `WaaV/inferv2/INFER_REUSE.md`** · cloned source: `research/{engines,rust,toolkits,models}/` (manifest: `notes/clone_manifest.txt`) · gateway source: `WaaV/gateway/`.

## Appendix E — Revision log (chronological)
| Rev | Change |
|---|---|
| r1 | Initial draft from learnings + wave-2 research (pre-critique) |
| r2 | Critique round 1 applied (76 findings: 5 CRITICAL, 53 MAJOR, 18 MINOR — 7-critic adversarial panel). Architecture-level: provider-api crate extraction (ADR-16, fixes Cargo cycle); supervisor restart keyed on death not readiness; per-slot KV replaces paged blocks (ADR-12); CUDA graphs demoted to optimization-with-eager-fallback; local-calibration lifecycle made normative (FR-S3b); single-GPU upgrade honesty (ADR-17); codec-edge policy (ADR-15, FR-E9). Numbers: NFR-P1 re-derived per (model×device×precision) with short-first-window ramp; §8.6 rebuilt (L4 ≠ L40S; Kokoro CPU RTF corrected 0.01→0.2–0.5; Kyutai duplex misquote fixed; ungrounded counts replaced by calibration-or-anchor); manifest example now passes its own lint. Product: cloning (FR-E10), biasing (FR-A6), language config, compat REST→M1, `waav run` + clean-machine gate, integrations as deliverables, catalog milestones+licenses re-audited (orpheus = Llama-3.2 derivative). Security: §16 rebuilt (GPL process-only boundary, truthful watermark provenance + EU AI Act posture, license obligation blocks, signed_only + model-parser fuzzing, offline sigstore, biometric governance, admin/data plane split, SBOM/CVE SLA). Ops: §4.2b lifecycle contract, flap damping, probe execution model, eviction interlocks, breaker busy-classification (GW-3), logging FR-O9, disk lifecycle §15.7, alert pack Appendix F. |
| r3 | Critique round 2 applied (48 findings: 1 CRITICAL, 25 MAJOR, 22 MINOR — 5 reviewers incl. r1-regression check: 52/58 FIXED, 0 MISSED/REGRESSED). **Design completion:** §8.3c device duty ledger — cross-model & cross-stage joint admission on one device (the r2 CRITICAL; AR + CFM/codec/vocoder stages all carry calibrated duty; mixed-model soak gates M2). **Simplifications (KISS lens):** probe slot removed (probe borrows a free slot; skip-under-load), `Paused` state removed (no trigger; ADR-12 records the revisit), M1 de-fatted (piper+GPL helper → M2, in-proc/GW-6 → M4, NFR-P3 → M2 with named fallback model). **Honesty fixes:** supervised-T1 drain bounded by supervisor lifetime (600 s only real under systemd/standalone; GW-8 candidate named); pipe-EOF parent-death (PDEATHSIG thread-semantics + macOS); k8s = kubelet-supervised adopt-only; NFR-P2 CPU re-derived (700 ms @ 60 chars); NFR-R2/P5 reconciled (10 s CPU / 60 s GPU). **Failure-semantics of r2 mechanisms:** auto-reload bounded (backoff + crash budget + terminal Failed), eviction Draining deadline (zombie sessions can't pin models), drift response debounced/rate-bounded/hysteresis + warming-suppressed + duty-paced warmup, signature verification pinned to pull-time (air-gap can't self-brick), breaker classification table over the full error taxonomy, egress slow-consumer policy, health on both planes. Consistency: FR-S1 de-blanketed; §14/§15.1 propagation; FR-M7a–d labeled; chunk_meta `generated`; lint honors first_window_tokens; Appendix B per-device ladders; L4/L40S runners added for anchor gates. |
| r3.1 | **Convergence patch** (round-3 panel: consistency CONVERGED, sign-off SHIP, regression 20/26): §10.2 cross-backend tensor-interop/device-residency rule (the one MISSED r2 finding — host-memory inter-stage handoffs, no per-step device↔host round-trips, fusion-or-io-binding for the CFM loop, OQ-2 criterion added); NFR-P2 CPU arithmetic re-derived with an explicit chars→seconds rate (duration-capped 1.4 s first segment ≈ 20 chars; 280–700 ms across the full RTF band — FR-E8 now mandates the cap); §9.4 NFR-R2 split propagated; ramp-gate-failure governance sentence (FR-S1); arity standardized to (model × device × precision); §11.1 phase cells aligned to FR-M8/§19; GW-6 + JS client in M4 scope + Pipecat gate in M3; M0 kokoro gate scoped to precomputed-phoneme forward; diagram probe label; L40S floor harmonized ≈2.5 ms; Appendix-B spdx-field semantics comment; rev-log made chronological. |
| **r4** | **Build-vs-borrow sweep folded in** (10-agent OSS reuse research, ~140 artifacts → `WaaV/inferv2/INFER_REUSE.md`). §17.3 reuse table expanded (candle-transformers codec decoders extract-not-build; parakeet-rs STT-A; kaldi-native-fbank DSP; Matcha/CosyVoice2 + candle-mmdit CFM; CosyVoiceForOnnx fp16-fix; Kokoros P3; misaki-rs/icu_segmenter/srx/wetext-rs G2P+TN; hf-hub/sigstore-rs/cargo-deny packaging; async-openai/axum/utoipa/otlp API+ops; TEI + moshi-server serving shells to mirror). **Audio gaps closed:** FR-D1 widened to {8,12,16,22.05,24,44.1,48} kHz with Opus as the rate-agnostic unifier + first-class 8 kHz telephony lane (G.711 via audio-codec-algorithms); FR-E9 opus-ingress fixed (Symphonia has no Opus decoder → `opus` crate) + hound+G.711 µ-law note; FR-D4 noise-suppression (nnnoiseless 16 kHz vs DeepFilterNet 48 kHz tax). **Components:** §11.2 ISTFT-ONNX porter recipe note + delay-pattern `where`-not-`clamp`; FR-E8 token-budget-aware caps + icu/srx segmenter + wetext-rs TN; FR-M1 flow/CFM solver metadata (meanflow/cfg_zero_star/t_scheduler — few-step levers); FR-E10 voice style mixing. §19 reuse-acceleration note; §20 risks (CosyVoice fp16/TRT rules, model-parser CVEs, reuse-dep drift with named fallbacks); OQ-8 new-feature roadmap (diarization, NS, semantic-EoT, word-ts, OCI, SBOM). No architecture/ADR change — reuse is acceleration with a build fallback for each item. |

## Appendix F — Deployment numbers & alert starter pack (normative defaults)

**Supervision mapping by substrate:** k8s = **two containers, kubelet-supervised, provider in adopt/connect-only mode** (gateway never spawns in k8s — PID namespaces make gateway-spawn the wrong tool); bare-metal production = systemd (`StartLimitBurst` per §4.2b(3)); dev/convenience = gateway-spawned (pipe-EOF parent-death detection; PDEATHSIG=SIGTERM Linux-only, set from a process-lifetime thread).

**k8s / supervisor arithmetic:** `terminationGracePeriodSeconds ≥ drain.deadline + 30 s` **for the systemd/standalone profiles where the 600 s drain is real**; in gateway-supervised mode the sidecar's effective drain is bounded by the gateway's grace (§4.2b(4)); preStop flips readiness before SIGTERM; `startupProbe` sized from observed `waav_infer_model_load_seconds` p99 + margin; liveness never gated on model state; supervisor restarts on death only (§4.2b).

**fd budget:** one UDS connection per session (§13.2) + listener overhead means `LimitNOFILE`/`ulimit -n` ≥ 4× max sessions + 1024 (e.g. 65536 for the NFR-X envelope) — stated here because default 1024 nofile WILL bite at scale.

**Alert starter pack** (ships as dashboard+rules; thresholds = defaults):
| Alert | Condition |
|---|---|
| Readiness flap | readyz transitions > 3 in 10 min |
| Model degraded | `model_state{state="degraded"} == 1` for > 5 min |
| TTFA breach | `ttfa_seconds` p99 > rated budget for 5 min |
| Stall rate | `stalls_total` rate > 2% of streams |
| Saturation | `requests_waiting > 0` sustained 5 min |
| Probe failure | `probe_total{outcome="fail"}` ≥ N consecutive |
| Drain overrun | `drain_overrun_total` increment |
| Silent CPU fallback | `waav_degraded_total{component="infer_backend"}` increment ∧ `stream_rtf` p99 → 1 |
| Crash loop | restarts > K in T (§4.2b quarantine fires) |
| Calibration drift | `waav_degraded_total{component="calibration"}` increment |

