# WaaV Infer v2.0 — Reframed Architecture (Brutal-Critique Revision)

**Status:** SUPERSEDES the serving-layer thesis of `INFER_ENGINE.md` v1.0 · **Date:** 2026-06-17 · **Device of record:** GB10 (Grace-Blackwell, aarch64, sm121, CUDA 13)

> This document is the output of an adversarial re-examination of `INFER_ENGINE.md` v1.0: a deep low-level mining of the proven systems (vLLM core, vLLM-Omni, SGLang-Omni, Moshi/moshi-server) for their **production failure scars**, a 2024-26 Arxiv literature challenge that **stress-tested the thesis against SOTA**, and a real-world-readiness sweep of **operational failure modes at scale**. The full evidence is in `/tmp/waav_failure_catalog.md` (238 entries, exactly cited) and is being instantiated as ≥1000 concrete scenarios in `WaaV/inferv2/scenarios/`. **v1.0's empirical foundation (§1, the 7 GB10 benchmarks) and HAL (§2) stand unchanged and are the substrate of everything below.** What changed is the serving-layer thesis — it was overstated, and the literature + scars force five material corrections.

---

## 0. The verdict — does the idea still stand?

**The kernel-level thesis STANDS. The serving-layer thesis was OVERSTATED and is now reframed.** Precisely:

- ✅ **STANDS (re-confirmed by primary sources):** frame-rate is fixed per active tick (DSM arXiv 2509.08753: "aligning X and Y to the same frame rate allows batched streaming inference"); AR codec-LM decode is memory-bandwidth-bound, so batching is the headline lever (the kernel-physics roofline is flat 1→64, 55×@64 — **though on real exported codec-LM graphs the host-KV re-stream caps the realized speedup at ~1.8× peak @ B≈16, regressing by B=64; size slots at the measured per-graph knee, INFER_PERF_VALIDATION.md §3a**); the two-batcher **catalog of compute kernels is complete** (no 2025-26 model needs a third *irreducible* kernel); lockstep scales to ~400 streams/H100 (Kyutai); MTP/depth-transformer (not draft-spec-decode) is the right codec multi-token mechanism (PCG arXiv 2511.13732: exact draft spec-decode = **0.98× net slowdown** on acoustic tokens); FlashInfer ~2× regression + ORT-CUDA-can't-int8-GEMM on Blackwell hold. **A fixed-slot non-preemptible engine structurally DODGES vLLM's four worst scars** (preempt-thrash, priority-starvation, 257-token padding, graph-disable cliffs) — the vLLM-core mining confirmed this directly.

- ❌ **BROKE (primary-source-contradicted) — the two precise conflations:**
  1. The thesis conflated **"fixed frame-rate per tick" (TRUE)** with **"no length-variance / fixed residency / static-known frame-rate" (FALSE)**. Stream *lifetime* (barge-in/EOS/turn-taking) and *frame-rate itself* (FlexiCodec dynamic 3-12.5 Hz) vary.
  2. The thesis conflated **"frame-sync alignment enables batching" (TRUE)** with **"fixed-slot rectangular ring-KV lockstep + reject-don't-glitch is OPTIMAL" (NOT SUPPORTED)** — contradicted by Fish Audio S2, VoxServe, ragged/continuous batching, ≤40% masked-slot waste, and deadline-aware degradation.

The reframing below keeps lockstep as the **efficient fast path** and corrects the five overreaches with evidence. None of this is a redesign; it is a **superset** — lockstep becomes one mode of a more general loop, and the production-hardening spine wraps it.

---

## 1. The five material corrections (critique → evidence → revision)

### R1 [CRITICAL] Hybrid KV: a radix prefix-cache for the deterministic prefix + ring for the suffix
**Critique.** v1.0 §4.3 claimed "prefix sharing is ~zero in voice" and chose a fixed-per-slot ring over paged KV. **Empirically FALSE.**
**Evidence.** Fish Audio S2 (arXiv 2603.08823, a frame-synchronous 21 Hz Dual-AR TTS, verified verbatim): reusing the same voice yields **86.4% average / >90% peak prefix-cache hit**; they extended RadixCache with multi-token keys for it. SGLang-Omni/Higgs partitions RadixAttention by reference-audio (`extra_key`). XTTS caches speaker embeddings fleet-wide. A fixed-per-slot ring **cannot share a prefix across slots** → it recomputes the ref-audio / system-prompt KV every request, forfeiting ~86% of cacheable work on the **top commercial workloads** (cloned-voice serving, multi-tenant voice agents).
**Revision.** KV is **two-tier**: (a) a **shared, paged, content-addressed prefix cache** (radix/blake2b key) for the *deterministic* prefix — reference-audio codes, speaker embedding, system prompt, persona — hit-shared across slots and tenants; (b) a **fixed per-slot ring** for the *per-utterance suffix* (the streaming generation), where v1.0's analysis (bounded context, no jitter, static shapes) holds. **The prefix-cache key MUST fingerprint the injected conditioning over ALL channels** (catalog G1: ref-audios sharing `-100` placeholder token-ids cross-contaminate KV → silent wrong-voice; key = `blake2b(full N-codebook ref sequence)`, `None` for zero-shot so genuine sharing survives). Hash = **sha256/blake2b, never xxhash** (catalog H: collision → cross-tenant KV leak); per-tenant `cache_salt` on block 0. This single change recovers the ~86% and makes prefix-affinity routing possible (R5).

### R2 [CRITICAL] Variable-stride lockstep + a third execution class (AR-outer + generative-inner head)
**Critique.** v1.0 §4.2 "advance one frame/tick" and the two-batcher model assume a static frame and a fused-or-separate inner loop. New architectures break both.
**Evidence.** DiTAR (arXiv 2502.03930, ICML'25): **patch-AR** advances by a *patch, not a frame*, with an inner DiT ODE solve (NFE 10→2) — two lockstep violations at once. FlashTTS (arXiv 2606.09141, verified): **MTP-3** (3 tokens/step) + a **2-NFE mean-flow head** breaks *both* WaaV batchers in one production model. CALM/SALAD/FELLE/VoxCPM: the inner generative solve's **NFE is a per-stream runtime dial** — two streams at different NFE cannot share one lockstep tick. FlexiCodec (arXiv 2510.00981, ICLR'26, verified): frame-rate is **data-dependent per-utterance and per-frame, unknown a priori** (3-12.5 Hz); the codec fleet spans 5/6.25/8/12.5/25/40/75 Hz.
**Revision.** Generalize the lockstep loop to **"advance a model-dependent, possibly-variable STRIDE."** The cohort key becomes `(model, stride-class)` and **must tolerate unknown-a-priori frame-rate** (a stream declares its stride per step, not once). Add a **third execution class — "AR-outer + generative-inner head"** — where the outer lockstep step composes *two* batchers: the AR advance (lockstep) **and** a per-stream variable-NFE inner micro-batch (step-bucket). The benchmark already proved this is profitable (§1.5: the nested per-frame patch batches 38×@64 because tiny-T is launch-bound). **Co-eviction invariant**: when an outer stream hits EOS, drop it from *all* nested inner loops in the same tick. The two-batcher *catalog* still holds (these compose the same two kernel families); what changes is the *composition* — they nest per step rather than picking one.

### R3 [CRITICAL] Deadline-aware admission + graceful degradation (not "reject-don't-glitch" as the primary mechanism)
**Critique.** v1.0 §6 made "NEVER admit-and-degrade" the rule; under overload it hard-rejects.
**Evidence.** Niyama (arXiv 2503.22562): graceful relegation meets **95%+ deadlines at 50% overload vs <20% for reject-baselines**. BrownoutServe (arXiv 2507.17133): quality-brownout cuts SLO violations **74%→7%** at ~5% accuracy loss. VoxServe (arXiv 2602.00269, UW Jan 2026 — a *direct* "vLLM-for-voice" competitor, 10-20× over vLLM/SGLang, which v1.0 never cited): a **binary streaming-viability objective** (once a frame is delivered in time, further latency is worthless → *don't over-serve*) + risk-of-violation scheduling + playback-buffer cadence protection.
**Revision.** Admission is **schedulability-gated** (`ΣU ≤ bound`, each stream reserves compute/period — EDF/RMS), and the overload response is a **graded ladder**: (1) shed **LO** work first (mixed-criticality: HI = the audio frame; LO = enhancement, denoise, eager-EoT speculation, second-tier reasoning, analytics); (2) **quality-brownout** (fewer NFE, smaller model tier); (3) **deadline-EDF reordering** with **negative-slack frame-drop + PLC** (drop a frame that cannot meet its playout deadline rather than produce it late — it has *negative* value); (4) only then **hard-reject new sessions** with a fast 503. Cadence is protected by the **client playback buffer**, not migration. Adopt VoxServe's binary-viability objective: stop spending compute on a stream already delivered-in-time. "Reject-don't-glitch" survives as the *final* rung, not the only one.

### R4 [HIGH] KV-length-aware prefill firewall + intra-node spatial P/D as a measured option
**Critique.** v1.0 §4.5 chunks the prefill firewall by **token count**; §4.5/§6 reject disaggregation as "DC-only."
**Evidence.** DuetServe (arXiv 2511.04791, Obs.2): decode batches with identical token-budget=8 show **>4× latency variation** as context grows — "token-budget scheduling ignores attention cost." SlidingServe (arXiv 2606.05933): a 7-feature latency predictor (incl. attention/context) hits MAE 2.5 ms, R²>0.99. TaiChi (arXiv 2508.01989, verified): "**disaggregation excels for strict-TPOT / relaxed-TTFT**" (+77% goodput) — *exactly* the isochronous-frame-clock + filler-masked-first-audio quadrant. Nexus (arXiv 2507.06608, intra-GPU P/D): 20× lower TTFT, 2.5× lower TBT vs vLLM; chunked-prefill mixed-batch measured **250 ms vs 15 ms decode-only (>8× TBT spike)**. vLLM-MORI-IO (single-node 8×MI300X): 2.4× goodput at p99-ITL<50 ms, "contradicting the misconception that disaggregation is only DC-scale."
**Revision.** The prefill firewall budgets on a **KV-length-aware predicted per-iteration latency** (not token count), and aligns the *fused* batch width (chunk + piggybacked decodes) to GB10 tile/SM counts (Bullet arXiv 2504.19516: chunk-too-small → 19.4% SM-idle wave-quantization). Reframe disaggregation: **cross-node physical P/D stays rejected** (correct for single-stream decode-heavy voice), but **intra-node SPATIAL P/D (SM-partition / MIG) is a first-class, measured option** — A/B-tested on GB10 against the chunked-prefill firewall (literature predicts an ~8× TBT tail spike from chunked prefill that spatial partitioning avoids). This is config-tiered, not mandatory.

### R5 [HIGH] Heterogeneous stream residency is first-class; MTP on the acoustic path; scoped spec-decode; long-form escape
**Critique.** v1.0 assumed homogeneous slot residency, a blanket spec-decode ban, and a universally-lossless ring.
**Evidence.** BayLing-Duplex (arXiv 2606.14528) *rejected* Moshi's per-frame synchrony for variable blocks and **beat Moshi** (overlap 2.07→1.10 s) using PAD/EPAD/SILENCE state tokens (= length-variance encodings). "Batch Spec-Decode Done Right" (arXiv 2510.22876): rectangular padding waste rises **13%@BS1 → 40%@BS32**; same-length grouping gives 3×. VocalNet (arXiv 2504.04060, verified) + Qwen3-TTS + FlashTTS: **MTP gives 2-5×, quality-neutral, and preserves the rectangular lockstep** (direct-emit, unlike draft-spec-decode). MagicDec (arXiv 2408.11049): sparse-KV spec-decode gives **2.51× at batch 32-256 for long context** (KV-memory-bound) — describes long-context token-AR STT, not TTS. StreamingLLM (arXiv 2309.17453): sliding-window forgetting + wraparound instability without **pinned attention-sink tokens**; AudioKV (arXiv 2604.06694): generic LLM-KV methods *fail* on 10-min audio (30k+ tokens).
**Revision.** (a) **Heterogeneous residency is first-class**: barge-in/EOS/VAD/async turn-taking produce real length variance even at fixed frame-rate → the scheduler either **compacts/repacks active slots** (avoid the ≤40% masked-idle waste) or explicitly **budgets the masked-slot bandwidth/energy cost**; drop the "no length variance" framing. (b) **Adopt MTP on the acoustic AR path** (treat the Depformer/code-predictor as the MTP mechanism, and/or add fixed MTP heads) — the cleanest competitive-parity win; **explicitly do NOT add EAGLE/Medusa draft-spec-decode** to TTS. (c) **Scope** the spec-decode ban: allow MagicDec-style sparse-KV spec-decode **only on the long-context token-AR-STT paging path**. (d) **Pin attention-sink tokens** in the ring and add a **paged/full-context escape hatch** for long-form TTS, long-audio STT, and many-turn agents — the ring is lossless only while context ≤ ring.

---

## 2. The reframed v2.0 executive thesis (supersedes v1.0 §0)

1. **Predictability before throughput (the Clockwork law).** A single inference is deterministic (p99.99 within 0.03% of median over 11M runs); GPU concurrency inflates the tail **100×** for ≤25% throughput. The frame deadline is met only by **serializing GPU execution per device** and making each per-frame step a predictable atom (CUDA-graphed exact-slot-count cohorts, pre-allocated, warmed, zero host-syncs). *(Clockwork OSDI'20; GB10 §1.3.)*

2. **Lockstep is the fast path, not the whole engine.** Frame-synchronous lockstep (fixed slots, exec-mask, per-slot ring suffix-KV) is the optimal steady-state for same-stride cohorts (Kyutai 400/H100). It generalizes to a **variable-stride loop** and composes a **third execution class** (AR-outer + variable-NFE generative-inner) for DiTAR/FlashTTS-class models. *(R2.)*

3. **KV is two-tier.** Shared paged radix prefix-cache (ref-audio/system prompt, ~86% hittable, fingerprinted over all channels) + per-slot ring suffix. Not "ring-only." *(R1.)*

4. **Paradigm sets the batch profile; precision and frame-rate are batching dimensions.** Two-batcher catalog (lockstep AR + step-bucket diffusion/flow) holds; they *nest* per step. fp8 is a DC-throughput lever not a batch-1 latency lever; KV-quant scales big-KV concurrency; cohort by (model, stride) tolerating dynamic frame-rate. *(v1.0 §1, R2.)*

5. **The worst case is the spec, and the worst case is operational.** One late frame in 100k is an audible click. Production survival = **cell/shard fault isolation + VRAM accountant + cooperative-cancellation-every-frame + frame-progress watchdog + GPU-health sidecar + media-on-UDP/QUIC + coordinated-omission-corrected observability**. These are not add-ons; they are the engine. *(§3, the real-world catalog.)*

6. **Overload is graded, not binary.** Schedulability admission → shed-LO → quality-brownout → deadline-EDF + negative-slack-drop+PLC → hard-reject. Cadence protected by the playback buffer. *(R3.)*

7. **One binary, config-tiered edge↔DC, KISS, progressive.** Inline single-stream edge (no scheduler/ledger) → pipelined-single → stage-batched DC (full machinery), mode resolved from config+load. The hardening spine is always-on but its heavy parts (duty ledger, spatial P/D, cell topology) are lazily constructed behind named triggers. *(v1.0 §8.)*

---

## 3. The production-hardening spine (day-one, from the real-world catalog §J)

This wraps the engine and is the difference between "works in a benchmark" and "real-world ready from day one." Nine pillars:

1. **Per-frame compute = a predictable atom.** Serialize GPU exec per device; pre-allocate all GPU buffers/KV-slots at startup (zero `cudaMalloc`/`cudaFree` in steady state — they're device-wide syncs); CUDA-graph the steady-state path capturing **exact slot-count cohorts** (0 padding, no 257→512 cliff); **READY = fully warmed** (warm every shape bucket + silence + barge-in path, gate readiness on it); a **canonical fixed set of frame shapes** as the audio-I/O↔model contract (defangs cuDNN-autotune/torch.compile/graph-capture/allocator-pool at once); no host syncs (`.item()`/`.cpu()`) in the decode loop. *(J6, J8, J12, J13.)*
2. **No unbounded stall on the frame thread.** WaaV is Rust (no GC — the Discord 40ms-spike class is gone); enforce **zero steady-state allocation** + lock-free SPSC rings for RT↔non-RT handoff; `mlockall` + prefault; RT-tune serving nodes (isolcpus + SCHED_FIFO 80 + nohz_full + IRQ-steering + perf-governor + NUMA-bind + PREEMPT_RT), gate deploys on a `cyclictest` budget. *(J9-J11.)*
3. **Cell/shard topology + VRAM accountant.** Partition streams across K worker processes (own CUDA context, ideally own MIG slice) so one fault/OOM loses a tolerable fraction, **not the whole box** (MPS shared-context fault propagation = the #1 fleet-killer); a single VRAM accountant gates every load/unload/KV-growth (free-before-load, refuse/reroute on projected-peak); `expandable_segments:True` + paged KV from day one. *(J1-J3.)*
4. **Liveness & cancellation.** Cooperative cancellation token checked **every frame boundary** + RAII single-owner slot-free on any exit + a **leak watchdog** reconciling active-slots vs live-connections (assume cancellation has bugs, *measure* the leak); a **frame-progress watchdog** (per-GPU "frames produced" heartbeat → fence+kill+migrate on stall — the *only* defense against silent GPU hangs); a **GPU-health sidecar** (DCGM Xid/ECC/remap → drain before the inevitable reset). 3-tier crash detection (sentinel-byte `ENGINE_CORE_DEAD` + out-of-band `waitpid`/`pidfd` + dead-flag fan-out) + `PR_SET_PDEATHSIG`. *(J5, J15, J16; catalog H6-H7.)*
5. **Numerics survival.** Always-on NaN/Inf logit detect → **reject-frame** (repeat-prev/codec-silence/greedy), *inverting* vLLM's emit-garbage default; fp32 sampler/CFM/ODE math regardless of model dtype; `_SAMPLING_EPS=1e-5` + `_MAX_TEMP=1e-2` + ≥1-survivor + NaN-safe `not(x<y)` pivot; multinomial sampled **outside** the CUDA-graph (or graph-safe gumbel-argmax inside); host-side input firewall (reject non-finite/over-length/bad-codec → 4xx, never a kernel fault) + poison-pill crash-counter → dead-letter quarantine. *(catalog H1-H5; J17.)*
6. **Graded overload + broken feedback loops.** Schedulability admission, shed-LO, brownout, deadline-EDF + negative-slack-drop, hard-reject last; Full-Jitter backoff + ~10% retry/reconnect budgets + CoDel time-in-queue + circuit breakers + a panic mode that sheds to ~1% (metastable-failure defense). *(R3; J19-J20.)*
7. **Transport: media on UDP/QUIC, control on TCP.** Neutralizes TCP-HOL (one lost packet = ~12 lost frames), Nagle×delayed-ACK (40-200ms metronomic hitch → `TCP_NODELAY`+`TCP_QUICKACK`), proxy buffering+compression bursting, and WebSocket-no-backpressure OOM (bounded drop-oldest send ring). Per-frame produce-timestamp + drop-oldest ring + PLC; FEC/RED over NACK; app-heartbeat (5-15s, 2-3 missed → free the slot) as primary liveness; session-id+seq resumable streams + per-turn idempotency. *(J21, J18.)*
8. **Observability that doesn't lie.** Deadline-relative (coordinated-omission-corrected) latency into mergeable histograms (HdrHistogram expected-interval back-fill) with a bucket edge **exactly at 0.08s**; **SM-efficiency (DCGM SM_ACTIVE) not GPU-util** (a 1-of-80-SM kernel reads 100% util); TTFA from first *playable* frame (not TTFB); per-frame RTF; multiwindow multi-burn-rate alerting on the frame-miss SLI + an output-validity SLI (empty/garbled transcript) + imminent-saturation causes (queue-wait p99, throttle bits, VRAM slope, fatal Xid); audio-quality drift proxies (concealment-rate, E-model MOS, NISQA-Discontinuity, ASR-round-trip on outgoing TTS, STT calibration-drift + golden-set replay + per-transcript model-version); the audio producer on an isolated core with **no observer code** (probe-effect defense). *(J22-J23.)*
9. **Lifecycle/rollout/multi-tenancy.** Drain-FSM + active-stream refcount (non-evictable while >0) + max-session-age + rainbow deploy + never-spot-for-live; **LoRA adapters for voice/language variants** (swap = ms, S-LoRA thousands/GPU — the single biggest TTFA win, dodges load-OOM/thrash) + sleep/wake eviction + singleflight load; VTC fair-share + per-tenant token-bucket in audio-seconds + MIG/MPS-caps/never-time-slice-live policy; canary on **new sessions** gated on **streaming SLIs** (a glitchy TTS isn't an HTTP 500) + session-ID affinity + prefix-affinity routing (R1). *(J4, J14, P2.)*

---

## 4. The revised component architecture (one diagram, in words)

```
                         ┌──────────────── CONTROL PLANE (TCP/gRPC: session, config, signaling) ────────────────┐
client ──WebRTC/QUIC──►  EDGE GATEWAY ──► ADMISSION (schedulability ΣU≤bound, prefix-affinity route, VTC fair-share)
 (media: UDP datagram,                          │
  drop-oldest ring,                             ▼
  PLC, FEC/RED,                          ┌── CELL / SHARD (own CUDA context, ideally MIG slice) ──────────────────┐
  app-heartbeat)                         │  VRAM ACCOUNTANT (free-before-load, projected-peak gate)               │
                                         │                                                                       │
                                         │  SCHEDULER (serialize GPU exec; deadline-EDF; HI/LO; negative-slack)   │
                                         │     │                                                                  │
                                         │     ▼   variable-stride lockstep loop  ───────────────────────────►   │
                                         │  ┌────────────── per-tick ──────────────┐                             │
                                         │  │ exec-mask (masked≠absent: substitute  │   STAGE-DAG (typed nodes):  │
                                         │  │  init token; gate EVERY mutation)     │   text→AR_talker{nested     │
                                         │  │ AR advance (lockstep, MTP) ───────────┼─► variable-NFE inner head}  │
                                         │  │ + nested step-bucket inner solve      │   →codec(micro-batch,fp32)  │
                                         │  │ KV: [radix PREFIX cache] + [ring SUFFIX]│  →vocoder(stream-window)   │
                                         │  └───────────────────────────────────────┘   (zero-copy on UMA;       │
                                         │     ▲ co-eviction: drop EOS from all loops     bandwidth-duty ledger)  │
                                         │  NUMERICS GUARD (NaN→reject-frame, fp32 sampler outside graph)         │
                                         │  CUDA-graph (exact slot-count cohorts, eager fallback on sm120)        │
                                         └───────────────────────────────────────────────────────────────────────┘
       WATCHDOGS (out-of-band): frame-progress heartbeat → fence+migrate · GPU-health (DCGM Xid/ECC) → drain ·
       leak-reconciler (slots vs connections) · PR_SET_PDEATHSIG · cooperative-cancel token (every frame)
       OBSERVABILITY (out-of-band, isolated core): CO-corrected histograms @0.08s · SM_ACTIVE · TTFA · quality-drift
```

**Path-A (ONNX/ORT)** and **Path-B (torch sidecar)** seams are unchanged from v1.0; the sidecar gains the multi-session stepped verb (R2) + the slot-keyed state discipline (catalog G5/F3) + `PR_SET_PDEATHSIG`. **Edge tier** collapses this to the inline single-stream path (no scheduler/ledger/cell machinery — KISS). **DC tier** runs the full diagram per cell, cells sized for tolerable blast radius.

---

## 5. What this means for the implementation plan + the scenario catalog

- The **extreme-TDD implementation plan** (`WaaV/inferv2/INFER_ENGINE_IMPL.md`) sequences this as test-first milestones extending the current 6-crate codebase, with **every failure-case in `/tmp/waav_failure_catalog.md` as an explicit failing-test-first gate** (e.g. `masked_slot_idle_then_resume_byte_identical`, `ring_kv_wraparound_vectors`, `nan_logit_rejects_frame`, `ref_audio_fingerprint_no_crosstalk`, `cancelled_stream_distinct_from_completed_FINAL`, `slot_freed_on_disconnect`, `cooperative_cancel_every_frame`, `coordinated_omission_corrected_metric`).
- The **scenario catalog** (`WaaV/inferv2/scenarios/`, ≥1000 entries across 10 families) is the coverage oracle: every architecture decision here must address its relevant scenarios, verified by the axes×families coverage matrix on consolidation.

**Bottom line:** the idea stands — frame-synchronous lockstep is the right spine — but the *optimal long-standing* architecture is **lockstep-as-fast-path inside a variable-stride, two-tier-KV, deadline-graded, cell-isolated, datagram-transported, coordinated-omission-honest engine**. Every correction is evidence-backed; every production scar is a test gate. That is the v2.0.

---

## 6. Post-audit gap-closure (v2.1) — the named-but-unbuilt layers, now specified

An adversarial 10-family coverage audit of all **1,113 scenarios** against §0-§5 + the TDD plan returned **651 SATISFIED / 386 PARTIAL / 76 GAP**. The verdict confirmed the core thesis (lockstep, two-batcher, hybrid-KV, numerics, hardening spine — the `MaskedCell` type-enforcement + post-graph NaN guard were cited as exemplary) and isolated every shortfall to **named-but-unbuilt seams/edges/DAG/control/scheduler-function** — all *additive*, zero core reframe. This section specifies them. Each item names its IMPL gate (added to `INFER_ENGINE_IMPL.md` §§M2b-M5 + the §6 coverage-completeness table).

### 6.0 Two hygiene fixes
- **Device string:** GB10 is **sm_121**; the vLLM CUDA-graph-hang scars (#40969/#44209) are filed against **sm_120-class Blackwell**. Both are the **sm_12x Blackwell family** → the eager-fallback (R-H4) applies to the whole family; docs use "GB10 (sm_121, sm_12x Blackwell family)".
- **v1.0→v2.1 §-crosswalk + restore.** v1.0's §-refs resolve (it's the governing substrate), but the v2.0 reframe *dropped* four v1.0 §6 mechanisms that scenarios depend on — **restored here**: (1) **drift-response** (EWMA live-measured step-time → sustained-p99-breach trips shed with 60 s hysteresis, shed-newest-least-progressed), (2) **calibration-stamp lifecycle** (`device+driver+warm-set` sha gates `/readyz`, refuses a stale stamp, cache-hit-skips-recalibration for fast rollback), (3) **thermal/throttle admission input** (DCGM `CLOCK_THROTTLE_REASONS` lowers the rated max before a frame misses), (4) **per-substrate accuracy/MOS re-stamp** (the `verified{substrate,precision,metric}` gate incl. the TTS MOS check for the WER-flat/MOS-crash signature).

### 6.1 LAYER 1 — Feature edges (the pipeline bookends + non-core features)
The engine modeled codec-tokens-inward; the *edges* are now first-class stage nodes (they exist in the codebase/`INFER_SPEC`; promoted here):
- **`IngressNormalizer`** (pre-encoder): any-SR → model-SR with **mandatory anti-alias on downsample to ≤16 k/8 k** (no chipmunk/foldover).
- **`TextFrontend`** (pre-AR, TTS): SSML (prosody/`<break>`/phoneme; degrade = strip-to-plain, **never speak tag literals**), locale number/date/currency **TN/ITN**, **code-switch** Unicode-script segmentation → per-run G2P → join, punctuation.
- **`AsrFeaturePost`** (post-decode, STT): CTC-collapse, **partial-stability/LocalAgreement**, `WordTiming`+confidence **population** (not just the drift proxy), **AED-DTW word-alignment** (cross-attention DTW, monotonicity-enforced, lower confidence — whisper has no duration head).
- **`TransportEgress`** (post-vocoder, OFF the AR clock, CPU/NPU-placeable): anti-alias resample → G.711/Opus encode **+ in-band FEC/RED** → **fixed 20 ms RTP repacketize** via jitter buffer.
- **`FeatureStage` taxonomy** for non-core features (denoise/dereverb/AGC/VAD/diarize/langID/speaker-verify/KWS/wake/**neural-SR ≠ rubato**/punct), **each with a `StageState::reset(slot)` contract** — not just `ArStepModel`. **Biasing** = a `BiasContext` threaded into the stepped seam + the F3 `reset_slot` fan-out + folded into the prefix-cache `extra_key` (else cross-tenant bias leak).

### 6.2 LAYER 2 — DAG machinery (was in v1.0 §3 + the SGLang G11 findings; promote to first-class)
- **Dynamic routing as `StageNode` fields:** `route_fn` (must return ∈ static topology, forbid empty), `wait_for_fn` (per-request expected-source set → conditional branches don't deadlock), `terminals[]`/`JoinByTime` multi-terminal (text+audio).
- **FINAL is DAG-propagated:** in-band `FINAL{stage_id, after_tail_drain}` on every edge + per-terminal FINAL (a stage must flush its delay tail before emitting FINAL, else truncation). `cancelled ≠ completed` propagates through every stage.
- **`SentenceAggregator`/`StableSpanGate`** archetype: commit only on sentence/stable-span boundary (no O(N²) MT churn/flicker; feeds the cascade LLM→TTS).
- **`DagSlotReset`** transaction + DAG-wide `channel_id`: reset fans to *every* stage's per-slot state (denoise gain / MT context / codec window / inner-solver latent), and a stale `channel_id` drops a late prev-occupant output at every stage (cross-user contamination guard).
- **`CloudStage{paradigm=remote}`**: vendor-mixed stages (local + cloud) — network SLO budget, credit-relay to the remote endpoint, fail-fast-on-disconnect, **reliable barge-in cancel to the remote session**, terminal-Error fan-out.
- **DAG-wide barge-in:** one cancel **fans to ALL stages with per-stage ACK** (G9 reliable, not fire-and-forget — a dropped cancel = a stream that keeps speaking).

### 6.3 LAYER 3 — Duplex / multi-stream seam (the `ArStepModel` is single-stream)
Generalize the stepped seam to a **`DuplexStepModel` / `MultiStreamSlot`**: K-stream interleave (the Moshi 17-stream), `user_in` **always modeled while speaking** + `model_out`, per-stream `(role, delay_sign, ring)`, a **`TurnState`/EoT head** (`eot_confidence` + PAD/EPAD/SILENCE state tokens + eager-EoT staging + a turn-strategy abstraction), a **`DoubleTalkPolicy`** (backchannel-vs-turn-grab). `StepOutput` gains **per-codebook depth** (for the RQ-Transformer depth/MTP). The **acoustic-delay ring (F8, depth `max_delay+2`, pad-force warm-up) moves to M2** (a correctness prereq for every multi-codebook TTS the plan headlines — Moshi/Mimi/Orpheus). `delay_sign` selects the task mode (STT / TTS / S2S / translate). **Cascade S2S nodes** (`LlmStreamNode` streaming token egress off the AR clock + the `SentenceAggregator` from §6.2 + an off-audio-path tool-call node with partial-fire) compose with R6.

### 6.4 LAYER 4 — Scheduler as a COMPUTABLE objective function (not principles) + router + tiers
The six competing objectives are unified into one specified function the scheduler optimizes each tick:
> **maximize Σ viable-sessions** s.t. `∀ substrate r: ΣU_r ≤ S` ∧ `Σ bandwidth_duty ≤ S·ceiling`, **ordered by RISK-of-violation slack** (VoxServe — *not* deadline-EDF alone; once a session is delivered-in-time, it yields slack: "don't over-serve"), **shed by criticality (HI/LO) then age**.
- **Binding-resource admission:** `bottleneck = argmax_r utilization(r)` over `{compute × N-substrates, shared-bandwidth}`, **re-picked per admit** (the mixed-clock 64-AR+8-CFM+codec+STT feasibility).
- **Corrected nested admission math** (arch audit): `T_step = T_ar + max_over_active(inner_steps_i × T_inner)` — the **max** over per-stream NFE, not a scalar; and the positive composition is a **`sub_bucket_inner_by_nfe`** structure (group B hidden-states into per-NFE inner passes within one outer tick, reassemble in slot order).
- **Bandwidth-duty MEASUREMENT** (the ledger's missing method): `bandwidth_duty = bytes_touched/ceiling × tick_rate`, measured via **DRAM_ACTIVE during co-load calibration** (calibration previously measured compute `T_step` only); each stage carries a `roofline_class ∈ {compute-bound, bandwidth-bound}` → **serialize two bandwidth-bound stages, overlap compute∥bandwidth** on a shared bus.
- **Masked-slot cost (decide the R5a disjunction):** default = a **`masked_bandwidth_duty` admission term** (KISS — charge the idle slots' bandwidth so admission can't over-admit); optional repack-trigger to a smaller pre-captured cohort.
- **Per-model realtime feasibility:** `reject_model_when_min_step(B=1) > T_f` (refuse a model that can't be realtime even single-stream — e.g. 150 Hz on a slow substrate).
- **Prefix-affinity ROUTER** (`waav-infer-router`): a fleet ref-KV residency map routes a returning voice to the worker holding its prefix KV (the R1 86 % hit), **yielding to duty when the holder is saturated**.
- **Per-SLA-tier reserved duty:** admit gold preferentially (a reservation), relegate a looser tier *within its own SLA*, protect the tightest contract.

### 6.5 LAYER 5 — Control plane / lifecycle FSM / cross-cell ledger / migration
- **Control-plane contract** (the engine↔orchestrator line, `waav-infer-control`): `drain / load / unload / lifecycle / reject-reason / used-total-slots` API + autoscale-signal / warm-pool / spill-routing / rollout-sequencing / canary-routing / region-failover / `freeze-rollout` / **`reconnect_admission_rate_capped_per_replica`** (storm governor). The engine *emits signals and accepts commands*; the orchestrator decides — drawn so the engine stays KISS.
- **Per-replica lifecycle FSM** (`lifecycle.rs`): `Loading → Warming → Ready ⇄ Degraded → Draining → Failed`, distinct from the per-stream `marker.rs` FSM. Transitions gated: **Degraded lowers the rated ceiling**; **Draining frees on refcount-zero**; **Ready requires warmup+calibration complete** (the first-request cliff + #44209 ready-then-crash-loop).
- **Box-scoped SINGLETON VRAM accountant** — *a real correctness gap*: cell/shard gives each cell its own CUDA context, so a per-process accountant lets **two cells each "see" free VRAM → double-load OOM**. The accountant is **box-scoped** and serializes all cross-cell loads.
- **Migration — self-contradiction FIXED:** **cadence-migration = rejected** (the client playback buffer protects steady cadence, VoxServe-style). **Fault/spill-migration = a measured, opt-in option** (append-only KV **+ inner-solver latent**, same-version, **leased ownership** with mid-migration-abort + zombie-slot + split-brain guards, playback-buffer-masked since one decode-step > one frame). The two are now explicitly distinct.

### 6.6 LAYER 6 — R6 reasoning cascade (fold in WaaV's existing `REALTIME_REASONING` work)
A slow/reasoning LLM is no longer just a "shed-LO" label. **R6 — slow-tier masking:** latency-filler fires when LLM TTFT exceeds budget; sentence-by-sentence LLM→TTS streaming via the `SentenceAggregator`; two-tier fast(non-committal)+reasoning with parallel-fire; **barge-in-cancels-LLM and reclaims leftover compute** (the standing invariant). Background-tier scheduling for the reasoning pass.

### 6.7 LAYER 7 — Guards, 3-tier KV, placement gates, precision resolution
- **KV is now THREE-tier:** radix **prefix-cache** (R1) + per-slot **ring suffix** + **`StreamingEncoderCache`** (cache-aware streaming-encoder delta-state, bounded from `genai_config`, deltas-only — the 560-streams/H100 STT headline).
- **`StagePlacer` IMPL milestone** (the §3.4 prose, now gated): ggml decision-order placement + `ZeroCopyBuffer` (alias-on-UMA / async-copy+double-buffer on discrete / per-edge relay with **credit back-pressure + notify-before-wait** + **cycle-safe sniffer** [WaaV's prior scar] + shm orphan-reaper) + `roofline_class` serialize/overlap + **per-substrate ridge-point batch-knee** (`peak-compute ÷ peak-bandwidth`, a-priori) + **degrade-to-`forward_native`** on op-fault (P-6 floor, telemetry).
- **Precision resolution** (GPU-free, in the `NopLoader` idiom): **`int8_file_never_lands_on_ort_cuda_ep`** (the §5.2 12 ms→232 ms master-constraint), `precision_resolves_per_active_ep` (`by_substrate[ep]`), `empty_kv_dtype_follows_weight_precision_q4f16` (the voxtral graph-driven-dtype seam via `StaticGraph::input_types()`).
- **`EpKind::Hpu` (Gaudi):** add the enum + a §2.2 row modeling the **systolic-MME → WIDE batch, HBM** (the generic NPU row wrongly models it as static B=1) + `hpu_degrades_to_forward_native`.
- **Shared guards:** one **`decode_repeat_ngram_guard`** covering BOTH STT AR-decode hallucination-loops and TTS codec-token degeneracy (rolling n-gram → terminate+FINAL+metric); **`max_inner_steps_per_tick`** per-slot cap (a runaway inner-NFE solve can't pace all); **mid-tick in-flight-recycle** guard (defer recycle to next tick; finish the in-flight kernel for the old occupant; drop by stale channel-id); teardown ordering (`abort-collectives-before-destroy` + kill-ladder + `restart_waits_on_vram_reclamation`); **`zero_d2h_sync_during_decode`** (the 9 ms/step budget rests on it); `sidecar_state_slot_keyed_no_crosstalk` (C3, the Python sidecar codec/sliding-window state); `slot_cap_by_vram_capacity` (RTX) + `multi_model_co_residency_admission` (MI300X 192 GB); `masked_slot_bandwidth_charged_in_admission`.

### 6.8 The completeness rule (META)
`INFER_ENGINE_IMPL.md` §6 now carries a **V2-mechanism → IMPL-gate cross-table**: every mechanism named in this document maps to ≥1 named test-gate; an un-gated mechanism is flagged **"requires-gate-before-production."** This closes the "named-not-gated" class (the bulk of the 386 PARTIALs) by construction.

**Post-closure state:** every one of the 76 GAPs now has a named mechanism above; every one of the 386 PARTIALs has a named gate or a concrete spec. The audit's verdict tables (`WaaV/inferv2/scenarios/coverage/*.md`) + `WaaV/inferv2/COVERAGE_ATTESTATION.md` record the scenario→mechanism→gate mapping. **This — §0-§6 — is the gap-closed final architecture (v2.1).**

---

## 7. Performance architecture (accuracy-preserving, empirically measured on GB10)

The full strategy + raw measurements are in `INFER_PERF.md` / `INFER_PERF_BENCH.md`. The *architectural* consequences (the decisions baked into the engine) are here. **Governing invariant:** the engine's default path is **exact / bit-faithful at the model's native precision** — no quantization, no speculative decoding, no approximate-attention, by default. Performance comes from **batching + memory-bandwidth physics + selecting the right *exact* kernel**, *not* from writing custom kernels.

### 7.0 The kernel-physics principle (proven, measured)
- **Zero custom kernels are required for the realtime target (batch ≤128, ctx ≤3000, memory-bound).** Lockstep inverts vLLM's contract: it demands *control-flow correctness* from a model (the §3 seam), not a custom paged-attention kernel. 100% of realtime perf = plain SDPA (right backend) + per-slot ring + CUDA-graph + batching. A custom kernel is only ever *justified* for quantized GEMM — a throughput-not-latency lever, vendored (TensorRT-EP/torchao) not hand-written, off the frame path — and excluded by the accuracy invariant anyway. **Fused RoPE+QKV is rejected on ACCURACY** (the #2274 fp32-compounding trap), not perf.
- **AR decode is bandwidth-bound** (`t_step ≈ WeightBytes/bandwidth`, flat in batch). Quantization (forbidden) would lower that floor; **batching fills the idle compute under it for free** — the headline lever the constraint leaves intact (measured **55×@64**). Size lockstep slots at the efficiency knee (B≈64 on GB10), not the KV wall.

### 7.1 The measured exact levers (each is a design decision)
| Lever | Measured | Architectural decision |
|---|---|---|
| Pin SDPA backend → cuDNN/flash | **40–135×** | the HAL's attention-kernel-selection table pins the exact backend per arch; **never auto-select** (math fallback = 40–135× slower), **never FlashInfer on sm_12x** |
| **IoBinding + on-device persistent KV** | **13%→2×** (grows with batch×ctx) | **the `StaticGraph` seam gains a `run_bound` path** (bind in/out `OrtValue` on-device, reuse a persistent KV buffer written at `cache_position`) — **the #1 engine perf change** |
| Lockstep batching | **idealized 55×@64; REAL ~1.8× peak @ B≈16** | the §4 lockstep batcher is *the* throughput mechanism — but the 55×@64 figure is the synthetic decode-step roofline; on real exported codec-LM graphs the host-KV re-stream caps it at **~1.8× peak @ B≈16, regressing by B=64** (live chatterbox gate, INFER_PERF_VALIDATION.md §3a). Size slots at the per-graph knee; 55×@64 needs device-resident-KV |
| GQA-native KV layout | **5.5–6.9× + 7× concurrency** | the ring-KV (§4.3) is laid out at the model's **native kv_heads**, never MHA-expanded |
| Prefix-KV reuse (R1) | **~7× TTFA** | the radix prefix-cache tier (R1) is also the top first-audio lever (bit-identical reuse) |
| CUDA-graph / compile | 1.18–1.24×@B1, *hurts* @B32 | **kernels tier by batch**: CUDA-graph/compile at edge/low-batch, eager at high-batch DC (already §2.4) |
| CPU bf16 (fp32-accumulate) | the only exact CPU compute speedup | the CPU tier uses bf16-fp32-accumulate (AMX / Grace-BFMMLA via MLAS-SBGemm), never int8 |

### 7.2 The per-hardware exact-attention matrix (folds into the §2 HAL)
GB10/sm_121: **cuDNN-SDPA / FA2** (FA3/FA4 need WGMMA/TMEM sm_12x lacks → "FA2-class"; **never FlashInfer** — 3 compounding sm_12x failures incl. a GQA=16 illegal-memory crash) · Hopper sm_90: FA3-split-KV · MI300X CDNA3: AITER/CK · CPU: fused `cpu_flash_attention`. **Rule baked into the HAL:** pin the backend per (arch, regime); never auto-select. (Path-A note: the ORT-CUDA EP fuses attention *inside* the graph; the SDPA-pin matters for the Path-B torch sidecar.)

### 7.3 The accuracy gate (a standing requirement on every perf change)
Every perf lever — including "math-preserving" ones — passes: **(1)** per-op tolerance vs the unfused-eager reference (fused-reduction kernels MUST keep the fp32 reduction + native-dtype weight-multiply — the #42325 trap); **(2) the AR-compounding identity test** — run the full N-step loop, the *emitted codes must be IDENTICAL*, not just close (catches per-op-invisible, audible drift); **(3)** the concurrency gate (bit-identical under `max_num_seqs≥4`). The exactness tripwire is a fused matmul epilogue silently demoting fp32 RMSNorm/RoPE → fenced by `epilogue_fusion=False` + fp32 custom norm/rotary (Path-B) + strongly-typed TRT-11 (Path-A). **This is §6.8's completeness rule extended to performance: no perf change ships without its accuracy gate green.**
