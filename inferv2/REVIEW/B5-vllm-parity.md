# B5 — vLLM-for-Voice Feature-Parity Gap Analysis

**Thesis under test:** "WaaV Infer is *vLLM for voice* — every flexibility/feature vLLM gives LLM
inference, WaaV gives Voice AI."
**Method:** READ-ONLY. vLLM's published feature set ⟷ the WaaV Infer Rust tree
(`/home/bud/ditto/waav/waav-infer/crates/waav-infer-*`), cross-checked against the 6-investigator
enterprise review (`REVIEW/00-SYNTHESIS.md`, `ENTERPRISE-READINESS-VERDICT.md`) and the project's own
reframed architecture (`INFER_ENGINE_V2.md`). Every HAVE/PARTIAL/MISSING below is grep/Read-verified for
**WIRED-to-the-live-serving-path vs SHELF-WARE** (built + unit-tested + `pub use`-exported but with **no
live caller** — the dominant theme of the review).
**Device of record:** GB10 (Grace-Blackwell, aarch64, sm121, 121 GB unified). Single-box by design.

> **The one distinction that governs this whole document.** WaaV Infer has *written the code* for a large
> fraction of vLLM's advanced features (paged KV, prefix cache, layered admission, cohort/tier scheduler,
> fleet router, LoRA registry, migration). The review proved — and this analysis re-confirms by grep — that
> **most of it is `pub`/tested but never called from a request handler.** So the honest parity verdict is
> rarely "MISSING" (the idea is absent) — it is overwhelmingly **PARTIAL (code exists, UNWIRED)**. For a
> *parity* claim, unwired == not-a-feature. That gap, not any missing algorithm, is what stands between
> WaaV and a truthful "vLLM for voice."

---

## SCORECARD

**14 major vLLM capability areas:**

| Verdict | Count | Areas |
|---|---|---|
| **HAVE** (wired + working) | **4** | #1 Continuous batching · #11 Streaming/cancellation · #12 Model variety/multi-modality · #13 OpenAI API |
| **PARTIAL** (exists but unwired / limited / broken-on-target) | **7** | #3 Prefix caching · #4 Quantization · #7 LoRA/multi-adapter · #9 Guided decoding · #10 Scheduling · #13b Metrics/observability · #14 Multi-hardware |
| **MISSING** (idea absent or only-as-scaffold) | **6** | #2 Paged KV (code exists, **0 callers → effectively missing**) · #5 Speculative decoding · #6 Chunked-prefill/disagg · #8 Tensor/pipeline parallelism · (#8b multi-worker fleet) · (S2S as a real model) |

> Counting note: #13 is split into REST-API (HAVE) and metrics/observability (PARTIAL); the table above
> reflects that. Paged KV is listed under MISSING because, although `paged_kv.rs` exists, it has **zero live
> callers** — for a parity claim it does not function. Several PARTIALs are "one wiring change from HAVE."

**Headline:** **The serving SPINE is genuinely vLLM-class** (continuous lockstep batching, bounded/VRAM/deadline
admission, streaming, barge-in, OpenAI API, 16  models, 9 hardware EPs). **The advanced KV & scheduling
machinery that *defines* vLLM's efficiency edge — PagedAttention, automatic prefix reuse, the layered
priority/fairness scheduler, S-LoRA, fleet routing — is written but SHELF-WARE.** WaaV has the *parts bin*
for vLLM-for-voice; it has not *assembled* the half that makes vLLM special.

---

## PARITY TABLE

Legend — **Status**: ✅ HAVE (wired+working) · 🟡 PARTIAL (exists-but-unwired / limited / broken-on-target) ·
❌ MISSING. **W** = wired to a live request handler? (Y / **shelf** = `pub`+tested, no live caller / partial).

| # | vLLM feature | Voice-domain equivalent | Status | W | File evidence | Gap |
|---|---|---|---|:--:|---|---|
| **1** | **Continuous / in-flight batching** (add/remove reqs mid-batch) | Lockstep **frame-sync** batcher: a new stream joins a free slot the next tick; finished/barged slots freed immediately | ✅ HAVE | Y | `server/codec_ar_batcher.rs` (MAX_SLOTS=24, shared mux thread); `runtime/serve.rs` `serve_codec_ar_multiplexed_inner` (pending `VecDeque<MuxAdmit>`, prefill-into-free-slot per tick, drop on EOS/barge); wired from `ws.rs:295` `speak` | JOIN is **staggered** (next tick, not mid-step) — bit-identical-by-design but not vLLM's instant insertion. **Batched-path TTFA = full-synth** (incremental emit reached single-stream only, `serve.rs` emits at slot completion — VERDICT #7/F6 deferred). Variable-stride loop (`dynamic_fr.rs`) for heterogeneous frame-rates is **shelf-ware**. |
| **2** | **PagedAttention / paged KV** (non-contiguous blocks, ~0 waste) | Block-table KV + pinned attention-sink (`PagedKvTable`) and per-slot ring (`RingKvCache`) | ❌ MISSING (code exists, **0 callers**) | shelf | `runtime/paged_kv.rs` (`PagedKvTable`, `KvResidency`) — referenced ONLY by `lib.rs:23/55` `pub use`. `scheduler/ring_kv.rs` (`RingKvCache`) — referenced ONLY by `scheduler/gqa.rs` (same-crate, tests). **No model arm uses either.** | Every live arm uses **contiguous per-slot KV tensors** passed to ORT (`core/stt/encdec.rs` merged-KV decoder; `voxtral.rs`/`qwen3_asr.rs` per-layer host KV). No paging, no block sharing, no near-zero-waste. The **long-form-KV escape (L12)** the design promises is INERT → multi-minute monologue / 10-min audio has no paged fallback. |
| **3** | **Prefix caching / automatic prefix reuse** (hash prompt prefix, share KV) | Radix prefix cache (`RadixPrefixCache`, blake2b/tenant-salt) for the **deterministic prefix** (ref-audio codes / speaker-embed / system-prompt / persona); "returning voice" prefix-affinity routing | 🟡 PARTIAL | shelf | `runtime/prefix_cache.rs` (`RadixPrefixCache`, `StreamingEncoderCache`, `TenantId`, `PrefixMatch`) — referenced ONLY by `lib.rs:25/57` `pub use`. `features/bias.rs` `BiasContext::fingerprint` (blake2b extra-key) is wired but only into the *unwired* `provider/fingerprint.rs`. `server/calib.rs` prefix-hash = **admission-calibration freshness, NOT KV reuse**. | **No model arm consults the prefix cache.** `INFER_ENGINE_V2.md` R1 cites Fish-Audio-S2 **86.4% avg / >90% peak** prefix-hit for same-voice serving — WaaV recomputes the ref-audio/system-prompt KV every request, forfeiting the single biggest win on the **top commercial workload** (cloned-voice multi-tenant agents). This is the #1 "vLLM-for-voice" gap by impact. |
| **4** | **Quantization** (AWQ/GPTQ/fp8/int8/bnb/marlin) | Config-driven precision per manifest: fp16 / int8 / q4 / q4f16 / fp32 / bf16; `{stem}_{precision}.onnx` resolution | 🟡 PARTIAL | Y (config) | `core/model.rs:33-102` `Manifest.precision` + `precision_token()` (notation-normalized half→fp16) → weight-file dispatch (zero-code). `runtime/precision.rs` graph-driven empty-KV dtype (the F4 q4f16 crash-fix, **landed**, `voxtral.rs:31` delegates). `backend-api/lib.rs:74/100` `as_f32()` vs `to_f32_vec()` (F4 OUTPUT-read fix applied to 35 sites). `backend-ort/lib.rs` int8-CUDA refusal + bounded CUDA arena. | **Per-precision reality (the honest matrix):** **fp32/fp16 = work on CUDA & CPU.** **int8 = physically impossible on CUDA** (ORT can't int8-GEMM → typed refusal; CPU-tier also forbids int8 → bf16/fp32-accum only). **q4f16 = works for voxtral (bit-identical int8 proven) BUT voxtral q4f16 + cohere fp16 *fail on the GB10 CUDA EP*** — ORT `GroupQueryAttention attention_bias not supported in cuda kernel` + cuDNN "no execution plans" → those 2 arms are **CPU-only here** (VERDICT #1). **Full e2e fp16 still needs INPUT-dtype casts for 7 arms** (canary/supertonic/chatterbox-AR/parakeet/nemo/qwen3/funasr build step inputs as f32 → fp16 not end-to-end, VERDICT #2). No AWQ/GPTQ/fp8/marlin (ORT-on-Blackwell + voice-model export constraints). |
| **5** | **Speculative decoding** (draft model / n-gram / Medusa / EAGLE) | (vLLM-analog would be MTP/depth-transformer on the acoustic path; reasoning two-tier is a *different* mechanism) | ❌ MISSING | — | grep: **no** `draft`/`spec`/`medusa`/`eagle`/`ngram`-speculation in any arm. `backend-api` `RepeatNgramGuard` = degeneracy-**termination**, not speculation (and shelf-ware, 0 callers). `runtime/reasoning.rs` + `scheduler/lease.rs` `TwoTier::parallel_fire` = **latency-FILLER masking** of a slow reasoning LLM (fast non-committal tier + crossfade), **not** draft-target speculation — no shared compute, no token-tree. | No latency-hiding speculation on the live path. The project's *own* design (`INFER_ENGINE_V2.md` R5) **deliberately bans EAGLE/Medusa draft-spec-decode for TTS** (exact draft = 0.98× net *slowdown* on acoustic tokens, PCG arXiv 2511.13732) and instead prescribes **MTP / depth-transformer** (2-5×, quality-neutral, preserves lockstep) — which is **also not implemented**. So the *correct* voice speculation (MTP) is the real MISSING item, not classic spec-decode. |
| **6** | **Chunked prefill** + **disaggregated prefill/decode** | Chunked streaming-encoder prefill (KV-length-aware firewall); intra-node spatial P/D (SM-partition/MIG) | ❌ MISSING | shelf | No chunked-prefill seam on any live arm (`encdec.rs` feeds the encoder **one-shot**). `features/streaming_encoder_cache.rs` `thread_chunk` = **shelf-ware** (dead, 5/7 features modules unwired). `backend-api` `StagePlacer`/`RelayPlan`/`run_or_degrade` (~900 LoC P/D-policy) = **0 callers**. `dag` thread-per-stage = **CLI-only** (`run-dag`), not request handlers. | Single shared lockstep loop, one `step_batch`/tick, prefill not split from decode. `INFER_ENGINE_V2.md` R4 prescribes a **KV-length-aware prefill firewall** (token-count chunking ignores attention cost, DuetServe >4× latency var) + **measured intra-node spatial P/D** (TaiChi +77% goodput for strict-TPOT/relaxed-TTFT = exactly the voice quadrant) — designed, not built. |
| **7** | **LoRA / multi-adapter serving** (S-LoRA: 1000s of adapters, ms-swap per-request) | Per-session adapter binding (`LoraRegistry`) + blue-green/canary lane routing (`RainbowRouter`) for **voice/language variant** adapters | 🟡 PARTIAL | partial | `scheduler/rollout.rs` `LoraRegistry` (register/bind/release, `ChannelId`-keyed, cross-session write *unrepresentable*) + `RainbowRouter` (canary-fraction lane routing). **WIRED into `server/control.rs`** (`ControlPlane{lora, rainbow}`, `register_adapter`/`bind_adapter`/`adapter_for`/`route_session`). | **The routing/bookkeeping is wired; the actual LoRA WEIGHT APPLICATION is missing.** No model arm reads `control.adapter_for(session)`, loads a delta-weight file, or applies LoRA tensors — `adapter_for` is consulted **only in `control.rs` tests** (`:1006-1046`). Every session loads the **same base graph** regardless of bound adapter. `INFER_ENGINE_V2.md` §3.9 calls S-LoRA "**the single biggest TTFA win**" (ms-swap, dodges load-OOM/thrash) — WaaV has the registry, not the swap. |
| **8** | **Tensor / pipeline parallelism + multi-GPU** | (single-GB10 by design; analog = none) | ❌ MISSING | — | grep: **zero** `tensor_parallel`/`pipeline`/`shard`/`tp_size`/`nccl`/`all_reduce`. `model.rs` loads one graph to one device (`cuda`/`cpu` via `backend-ort/ep.rs`). | No single-model sharding across GPUs. **Defensible by scope** — WaaV explicitly targets one Grace-Blackwell box (`INFER_ENGINE_V2.md` thesis #7: config-tiered edge↔DC, single device of record). A true "vLLM for voice" at DC scale eventually needs at least KV-aware multi-worker placement; today that's absent. |
| **8b** | **Distributed / multi-worker** (Ray, fleet, migration) | Prefix-affinity fleet router + cross-tier failover; fault-spill slot migration | ❌ MISSING (router) / 🟡 PARTIAL (migration) | shelf / partial | `router/lib.rs` `Router::route` (prefix-affinity / yield-to-duty) = **PURE shelf-ware, ZERO dependents** (no Cargo dep, no non-test `use`). `scheduler/migration.rs` `MigrationKind::FaultSpill` IS wired via `control.rs:704` `begin_fault_migration` (orchestrator-driven, append-only KV+latent) — but **not on the live per-request serve loop**. | Multi-worker routing is the **single biggest unwired surface** (a whole prefix-affinity/failover engine nothing calls). Migration is control-plane-only (drain/fault), not live load-balancing. No request actually crosses workers by routing decision. |
| **9** | **Structured output / guided decoding** (outlines/xgrammar JSON-schema/regex/grammar) | Forced-language / forced-position decoder prompt (Whisper task tokens), token-suppress mask, contextual hotword biasing, canonical-notation forcing | 🟡 PARTIAL | Y | `core/stt/whisper.rs:61-82` `forced_decoder_ids` (lang/task forced at positions) + `:102` `suppress_mask` (per-vocab bool) — **live on the STT AR path**. `features/bias.rs:59-125` `BiasContext` hotword phrase-weighting (wired into prefix extra-key seam). `components/standardize.rs` `resolve_alias`/`NotationMap` (canonical→native lang/precision/device, **live**). | The *voice-meaningful* guided cases (force language, force timestamp on/off, suppress tokens, bias hotwords, force canonical notation) **are covered and wired**. But there is **no general logit-bias / token-mask / grammar / regex / allowed-tokens-set per request**, and **no structured-JSON-output** path. All decoding is otherwise **greedy argmax** (no constrained sampler). Adequate for voice; not full vLLM guided-decoding. |
| **10** | **Scheduling / preemption / priority + fairness** | Bounded+VRAM+deadline admission (live); layered admission / cohort-by-frame-rate / tier-executor / KV-firewall (designed) | 🟡 PARTIAL | partial | **LIVE:** `server/codec_ar_admission.rs` `try_admit` (atomic bounded `max_inflight` + `VramAccountant` 256 MB/stream + deadline-projection `queue_depth×0.5s` + typed-429 shed). `engine.rs` `DutyLedger.admit_bandwidth` (saturated-bus refuse) + `VariableStrideLoop`. **SHELF-WARE (0 refs outside scheduler crate):** `admission.rs` `LayeredAdmission`, `cohort.rs` `CohortPlanner`, `tier.rs` `TierExecutor`, `admission.rs:1793` `KvFirewall`. | The live scheduler is **FCFS bounded-admission only** — **no priority, no preemption, no cross-tenant fairness** on the live path. The "sophisticated scheduler" the architecture advertises (layered/cohort/tier/KV-firewall, the **graded-overload ladder** of `INFER_ENGINE_V2.md` R3: shed-LO→brownout→EDF+negative-slack-drop) is **all unwired**. Live-hazard bugs in the *wired* `DutyLedger`/admission (admit→add TOCTOU over-admit; clone-full-map-per-call) per synthesis. A fixed-slot non-preemptible engine **structurally dodges** vLLM's preempt-thrash/priority-starvation scars (a real design *win*) — but it also means it has no priority knob at all. |
| **11** | **Streaming, cancellation, beam-search, best-of, logprobs** | Delta-streamed audio/text; **barge-in** cancellation (per-frame cooperative); sampling params | ✅ HAVE (stream+cancel) / ❌ MISSING (beam/best-of/logprobs) | Y | `ws.rs:295-352` codec-AR delta-stream (chunk_meta + binary audio as produced) + `barge_in` frame → `CancelToken.cancel()` (interrupt within a frame; `select!` also cancels on socket-close). `dag/barge_in.rs` `DagBargeIn::cancel` (fan-cancel-all-stages + await ACK, reliable not fire-and-forget). `runtime/numerics::sample_token` exists (shelf). | **Streaming + barge-in are HAVE and high-quality on the codec-AR path.** Caveats: **batched-path TTFA = full-synth** (incremental reached single-stream only — #11 ⨯ #1 overlap); **one-shot TTS (kokoro/melo) buffers whole utterance**; **STT emits partials only at the finalize barrier, not mid-audio**; `ws.rs` **silently drops `clear`/`flush`/`session.update` control frames** (synthesis). Sampling: **all arms hardcoded greedy argmax** — **no temperature/top-p/top-k/beam/best-of/logprobs per request** (intrinsic to most voice STT/TTS, but it's a literal parity gap vs vLLM's sampler suite). |
| **12** | **Multi-modality / model variety** (many archs via registry; "add model = config") | STT / TTS / S2S / diarize / enhance across **16 registered arch arms** (~40+ checkpoints) + telephony | ✅ HAVE (strong) | Y | `core/model.rs:378-587` config-arch dispatch (16-arm match, `REGISTERED_ARCHITECTURES` len-locked at 16). **STT (11):** whisper, moonshine (enc-dec AR); sensevoice, nemo-ctc/parakeet-ctc (CTC); parakeet-tdt, nemotron (transducer); qwen3_asr, funasr_nano, voxtral (LLM-decoder ASR); cohere, canary (AED enc-dec). **TTS (4):** kokoro (StyleTTS2, CPU-pinned), melo (VITS), chatterbox (codec-AR), supertonic (CFM flow-matching). **S2S (1):** duplex_codec_ar. **+** `diarize.rs` (pyannote+WeSpeaker), `enhance.rs` (GTCRN/DPDFNet). Telephony: `components/resample.rs` anti-aliased 8/16/24 kHz. | **Nuance:** new *checkpoint of a registered arch* = zero-code (weights+manifest); new *architecture* = 1 Rust arm (narrow seam). **S2S `duplex_codec_ar` is a SYNTHETIC SCAFFOLD, not a real model** (hash-folds user codes into chatterbox text tokens, no acoustic encoder / no trained EoT head, registers no loadable arm) — must be down-scoped or built (synthesis CRITICAL). Codec *decode* (Opus/G.711) is gateway-side, not in-engine. Otherwise this is WaaV's **strongest parity area** and is genuinely "vLLM-registry-for-voice." |
| **13** | **OpenAI-compatible API** | `/v1/audio/transcriptions`, `/v1/audio/speech`, `/v1/models`, native WS-v1 duplex, control plane | ✅ HAVE | Y | `server/lib.rs:300-330` routes: `/v1/audio/speech`, `/v1/audio/transcriptions`, `/v1/models` (per-model state), `/v1/control/{drain,load,unload,set-policy,lifecycle}`, `/health/{live,ready}`+`/livez`/`/readyz`, `/metrics`. `ws.rs` native frames (`SessionConfig`/`Speak`/`Finalize`/`BargeIn`/`Flush`/`Clear`/`SessionUpdate`). | REST + native WS + control endpoints are wired. Caveats from synthesis: **`POST /v1/control/drain` returns 200 but does NOT gate admission** (only SIGTERM drains — F3, *fixed this session* per VERDICT); **~63% of control endpoints operationally unreachable**; **S2S realtime route is a stub**; **cascade is CLI-only**. |
| **13b** | **Metrics / observability** (Prometheus, request tracing) | `/metrics` + W3C trace propagation | 🟡 PARTIAL | Y (thin) | `server/otel.rs` per-turn `tracing::Span` + W3C traceparent propagation (sampled honored). `/metrics` emits `waav_infer_model_state` (gauge), `waav_infer_frame_watchdog_shed_total`, `audio_seconds_total`, `waav_degraded_total`. | **The load-resilience layer (GATE #9) emits ZERO metrics** — no 429/shed/inflight/queue-depth/reserved-VRAM/TTFA/RTF/frame-miss (synthesis + VERDICT #5: "operationally blind under load"). No OTLP exporter, no coordinated-omission-corrected histograms, no SM_ACTIVE (vs GPU-util), no per-frame RTF. `INFER_ENGINE_V2.md` §3.8 ("observability that doesn't lie") is **almost entirely unbuilt**. The #2 wiring gap after prefix-cache. |
| **14** | **Multi-hardware** (CUDA/ROCm/TPU/CPU/Metal/XPU/Neuron) | ORT execution-provider abstraction + torch sidecar for non-ONNX | 🟡 PARTIAL | Y | `backend-api/lib.rs` `EpKind` declares 10 EPs; `backend-ort/ep.rs` maps **9 live via ORT**: CUDA, TensorRT, ROCm, MIGraphX, OpenVINO, QNN, CoreML, DirectML, XNNPACK (+ CPU floor, platform-ordered probe). `backend-ort/lib.rs` SM12X guards (FlashInfer-forbidden 120..130, int8→fp32 demote, bounded arena). `server/torch_sidecar.rs` + `torch_runtime/` Python sidecar (framed stdio) for stateful AR-codec/LLM-decoder/S2S models. | ORT-mediated multi-hardware is **broad on paper** but only **CUDA + CPU are exercised** on the GB10 of record (the other 7 EPs are compiled-in, untested here). **Torch sidecar is live code** but the per-request reaper is wired while the **idle-zombie death-scan has no production poller** (CRITICAL C1). **No ggml / no candle backend** despite design notes ("hybrid ORT+ggml", "candle codec decoders") — design-only. No TPU/Neuron. HPU = caps-only, no EP. |

---

## WHAT MOST DEFINES "vLLM FOR VOICE" — THE TOP GAPS

vLLM's reputation rests on **four** pillars above plain batching: **PagedAttention, automatic prefix
reuse, the priority/fairness/preemption scheduler, and S-LoRA**. WaaV Infer has **shelf-ware for all
four** and **live wiring for none**. Those are precisely the gaps that make the "vLLM for voice" claim
currently *aspirational*:

1. **Automatic prefix reuse is the #1 gap (biggest ROI).** `RadixPrefixCache` exists, unwired. The
   project's own evidence (`INFER_ENGINE_V2.md` R1) puts same-voice prefix-hit at **86%+** on the top
   commercial workload (cloned-voice multi-tenant agents). vLLM ships this on by default; WaaV recomputes
   ref-audio/system-prompt KV every request. **Wire `RadixPrefixCache` to the codec-AR/AED arms with the
   blake2b all-channel conditioning key + per-tenant salt.**

2. **Paged KV / long-form escape.** `PagedKvTable` + `RingKvCache` exist, 0 callers. Without paging there
   is no graceful 10-min-audio / many-turn path (AudioKV: generic LLM-KV fails past 30k tokens). vLLM's
   defining innovation has **no functional analog on the live path**.

3. **The "sophisticated scheduler" is FCFS in reality.** `LayeredAdmission`/`CohortPlanner`/`TierExecutor`/
   `KvFirewall` — **0 refs outside their crate**. Live = bounded+VRAM+deadline admission, **no priority,
   no preemption, no fairness, no graded-overload ladder** (R3). (Caveat: a fixed-slot non-preemptible
   engine *intentionally* dodges vLLM's worst scheduler scars — so the right move is **wire fairness +
   the LO-shed/brownout ladder**, not copy vLLM's preemptible scheduler wholesale.)

4. **S-LoRA = routing without weights.** `LoraRegistry`/`RainbowRouter` wired into `control.rs`, but **no
   arm applies adapter deltas** (`adapter_for` consulted only in tests). The "single biggest TTFA win" per
   the design is **half-built**: it routes, it doesn't swap.

5. **MTP, not classic spec-decode, is the missing voice-correct latency lever.** WaaV correctly *rejects*
   EAGLE/Medusa for TTS (net slowdown on acoustic tokens) — but it also hasn't built the **MTP /
   depth-transformer** (2-5×, quality-neutral) that is the right replacement. Net: **no latency-hiding
   speculation of any kind on the live path.**

6. **Observability that lies by omission.** The hardened admission/overload layer emits **zero** load
   metrics → blind under exactly the load it exists to survive. vLLM's Prometheus suite (queue, running,
   preemptions, KV-usage, TTFT/TPOT histograms) has **no functional counterpart**.

**Also load-bearing but lower-tier:** batched-path TTFA=full-synth (#1/#11); fp16 not end-to-end + 2 arms
CUDA-broken (#4); S2S is a scaffold not a model (#12); multi-worker router is dead (#8b); no per-request
sampling params (#11).

---

## PRIORITIZED ROADMAP — WIRE/BUILD TO REACH TRUE vLLM-FOR-VOICE PARITY

Ordered by **(parity-defining impact) × (1 / effort)**. Most of Phase 1 is **wiring existing shelf-ware**,
not new algorithms — the cheapest path to a *truthful* claim.

### Phase 1 — Wire the four pillars that define vLLM (mostly existing code)
- **P1 [highest ROI] Wire `RadixPrefixCache` to the live arms.** Hash the deterministic conditioning
  prefix (ref-audio codes + speaker-embed + system-prompt) with the blake2b all-channel key + per-tenant
  `cache_salt` (G1/H scars); consult/insert in `encdec.rs` + the codec-AR/AED decoders. Gate: same-voice
  re-request hits cache (target the cited 86%); cross-tenant isolation test. **Turns #3 PARTIAL→HAVE.**
- **P2 Wire `PagedKvTable` as the long-form escape** behind the per-slot ring (two-tier KV per R1/R5):
  ring for the suffix, paged for context beyond ring + the shared prefix. Pin attention-sink tokens. Gate:
  10-min-audio STT / many-turn TTS doesn't degrade. **Turns #2 MISSING→HAVE.**
- **P3 Wire S-LoRA weight application.** Make the arms read `control.adapter_for(session)`, singleflight-
  load the delta, apply LoRA tensors in the forward graph (or swap a fused graph). Gate: per-session
  adapter changes output; ms-swap measured. **Turns #7 PARTIAL→HAVE.**
- **P4 Wire the scheduler's fairness + graded-overload ladder** (R3): per-tenant token-bucket in
  audio-seconds (VTC) + LO-shed (enhancement/eager-EoT/second-tier-reasoning) → quality-brownout (fewer
  NFE / smaller tier) → deadline-EDF + negative-slack frame-drop → hard-reject last. Fix the wired
  `DutyLedger`/admission TOCTOU + clone-per-call hazards en route. **Turns #10 PARTIAL→(priority+fairness).**

### Phase 2 — Close the observability + correctness gaps that block "enterprise"
- **P5 Emit the load-resilience metrics** (429/shed/inflight/queue-depth/reserved-VRAM/TTFA-from-first-
  playable-frame/per-frame-RTF/frame-miss) as Prometheus + OTLP; CO-corrected histograms with a bucket
  edge at 0.08 s; SM_ACTIVE not GPU-util. **Turns #13b PARTIAL→HAVE.**
- **P6 Incremental TTFA on the batched path** (per-tick decode+emit in the mux loop, bit-identical concat)
  — closes the #1/#11 batched-TTFA=full-synth gap so concurrency doesn't kill first-audio latency.
- **P7 Finish fp16 end-to-end** (StaticGraph::input_types()-driven INPUT casts for the 7 arms) and resolve
  the 2 CUDA-broken arms (non-GQA-attention_bias export or ORT/op upgrade). **Hardens #4.**
- **P8 Honor `clear`/`flush`/`session.update`** on the WS path; make `drain` actually gate admission +
  `/readyz` (F3 follow-through). **Hardens #11/#13.**

### Phase 3 — Build the voice-correct versions of the remaining vLLM levers
- **P9 MTP / depth-transformer on the acoustic AR path** (the *correct* voice speculation per R5 — 2-5×,
  quality-neutral, preserves lockstep; explicitly NOT EAGLE/Medusa). **Turns #5 MISSING→HAVE-equivalent.**
- **P10 KV-length-aware chunked-prefill firewall** + A/B intra-node spatial P/D (SM-partition/MIG) on
  GB10 (R4). **Turns #6 MISSING→PARTIAL.**
- **P11 Variable-stride lockstep + AR-outer/generative-inner third execution class** (R2) for
  DiTAR/FlashTTS/FlexiCodec-class models; wire `dynamic_fr.rs`. **Deepens #1.**

### Phase 4 — Scope-honest decisions (down-scope OR build; either way fix the docs)
- **P12 Decide the dead crates:** wire `router` (multi-worker prefix-affinity placement) for the DC tier,
  OR delete it + the unwired `provider`/`gateway-provider-api`/5 `features` modules/~900-LoC backend-api
  policy block, and reconcile `INFER_ENGINE`/`INFER_SPEC`/memory so advertised==shipped. (**#8b**)
- **P13 S2S: build a real native-S2S arm OR label `duplex_codec_ar` a benchmark scaffold** in the docs +
  memory (it currently ships advertised as real native S2S; it is a hash-conditioned scaffold). (**#12**)
- **P14 Tensor/pipeline parallelism** remains correctly out-of-scope for single-GB10; revisit only when a
  multi-box DC tier is committed. (**#8**)

### Sequencing rationale
**Phase 1 = the parity-defining wins, and they are mostly wiring, not invention** — the code exists,
unit-tested, behind `pub use`. Doing P1-P4 alone moves the scorecard from **4 HAVE / 7 PARTIAL / 6 MISSING**
to roughly **8 HAVE / 4 PARTIAL / 4 MISSING** and makes "vLLM for voice" *defensible*. Phase 2 makes it
*operable* under load. Phase 3 builds the genuinely-new voice levers. Phase 4 stops the docs from
over-claiming again — the root cause the review named the #1 enterprise-readiness issue.
