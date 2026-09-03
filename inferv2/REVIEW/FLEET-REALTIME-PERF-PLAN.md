# Fleet-Wide Realtime + Perf + Batch Plan — "Every tch Model, Not Just dia2"

**Date:** 2026-06-27 · **Host:** GB10 (Grace-Blackwell **sm_121**, aarch64, 121 GiB unified CPU+GPU pool) · **libtorch:** tch / PyTorch 2.12+cu130
**Owner goal (broadened from dia2):** EVERY tch (Path-B) model must be **HIGH-PERF + NO ACCURACY LOSS (byte-identical, max|Δ|=0 on emitted codes/latents/transcript) + REAL-TIME (RTF<1) + HIGH-BATCH** — not just dia2.
**Non-negotiable:** the DEFAULT serve path (`PerfMode::Accuracy`) stays byte-identical. Every numerics-trading lever lives ONLY in opt-in `PerfMode::Throughput`. Selection is via `AccelMapper::select_perf` / `DeviceCaps` / `AccelBackend` — no hardcoded CUDA-only.

This plan synthesizes the four REVIEW docs (`PATHB-PERF-ACCEL-PLAN.md`, `PATHB-FLEET-PERF.md`, `PATHB-FLEET-RISK-MATRIX.md`, `PATHB-ARBITRARY-BATCH-PLAN.md`) into ONE fleet program. The thesis from dia2 generalizes: **the recoverable RTF time across the whole Path-B fleet is launch / inter-kernel-gap / host-restream overhead (dia2 measured 60.8% GPU-idle, 4.13M ~9.6µs launches, attention only ~1.5%), NOT arithmetic** — so the dominant realtime wins are byte-identical (replaying identical kernels in a tighter schedule changes no numerics). Fix the SHARED `nn/` primitive + `ByteIdenticalGraph` AccelBackend ONCE and the whole arch family lifts.

---

## 1. The SHARED byte-identical lever stack (L1–L4) — the generalization thesis

All four levers live in shared substrate (`crates/waav-infer-backend-torch/src/nn/` + the `ByteIdenticalGraph` AccelBackend, `backend-api/lib.rs:2883-2952`) so generalizing dia2's fix lifts the family, not one model.

### L1 — CUDA-graph capture/replay of the fixed-shape step (the #1 launch-bound lever)
- **What:** record the fixed-shape kernel sequence ONCE, replay as a single `cudaGraphLaunch`, eliminating per-kernel CPU dispatch + inter-kernel gaps. Byte-identical BY CONSTRUCTION — replay re-runs the IDENTICAL kernels on the IDENTICAL memory (`nn/cuda_graph.rs:5-11`).
- **Proven in-tree, default-ON via `ByteIdenticalGraph`:** dia2 (backbone `forward_graph` + 31 depformer `StageGraph`, `dia2.rs:393,503-619`; −19% AR wall, RTF(AR) 2.08→1.68, **608/608 + 544/544 byte-identical**), csm (RTF 1.007→**0.805**, 4000/4000), omnivoice (one captured masked-diffusion forward replays all 2·n_steps), dots (DiT `forward_graph`, deep-copy fixes cond/uncond aliasing, ×1.04 capped by per-patch re-capture).
- **Lifts:** the **graphable set** = {dia2, csm} (fleet) + {omnivoice, dots} + the **unrealized 4/5 hybrid heads** (voxtral_tts/vibevoice/indextts2/cosyvoice3 have ZERO `cuda_graph` refs today) + **hibiki** (the dia2/csm twin, f32 — byte-identity EASIER) + the **STT AR-decoder rings** (voxtral already has it; cohere/ark/granite/higgs_stt have zero). Web: PyTorch/SGLang report CUDA graphs recover 20–30% of small-batch decode; Voxtral-TTS captured its ODE solver → 47% latency cut / 2.5× RTF (133→70ms), byte-identical.

### L2 — On-device sampling / full-AR-loop single-graph capture (the depth de-serializer)
- **What:** keep the sampled/argmax id DEVICE-RESIDENT across the depth/AR chain (`nn::sample_token_on_device`, `nn/sampling.rs:57`; `broadcast_to`), removing the per-stage D2H→H2D sync. Byte-identical: multinomial advances the global Philox offset at LAUNCH (enqueue) not READ, so dropping the read changes no RNG.
- **Why it de-serializes:** dia2's 31 depformer stages cannot pipeline because each chains on a host→device prev-token upload (`dia2.rs:1497` `Tensor::from_slice(&vec![prev;branches]).to(dev)`). On-device sampling lets stages enqueue back-to-back → single-graph-capturable. This is the **in-progress R4/B3 dia2 RTF<1 lever (task#133)**.
- **Lifts:** the DEPTH/DEPFORMER class — dia2, csm, misotts, s2_pro (4L fast-AR), qwen3_tts (5L CodePredictor), hibiki (16-stage depformer + 17 D2H/frame). Also the greedy STT decoders (L2 = on-device **argmax**, voxtral did it via `argmax_first_device`; ark/granite/higgs_stt still D2H the full vocab every step). FORWARD: full-AR-loop capture via CUDA 12.4+ conditional WHILE/IF nodes (galv RNN-T 2.5×) — byte-identical, not yet in-tree (needs a tch conditional-node binding).
- **NOT applicable** to the PARALLEL-codebook class (dia/neutts/higgs/higgs_v2 — single forward/frame, no per-stage chain to de-serialize).

### L3 — Cast/copy/cat/reshape fusion (CAST-ONLY, the ~25%-GPU-busy f32↔bf16 sandwich tax)
- **What:** the f32↔bf16 "F32Sandwich" casts + ring copy/cat + reshape are 2.5M launches / ~25% of GPU-busy (`dia2.rs:297-302,436-470`). Byte-identical ONLY as cast-only/copy-elision — **NEVER GEMM-split-k or norm/RoPE/QKV reassociation** (the omnivoice ~6e-5 / misotts 0.016 scar flips a code over 28 layers).
- **Mostly FREE inside L1 replay** where graphable; standalone hand-fusion adds ~+5% ONLY behind a per-op Δ==0 codes gate. Lower value on the fp16 models (higgs/higgs_v2 — f32-head-over-fp16-body, lighter cast pattern, no full sandwich).
- **Lifts:** the bf16-sandwich set — dia2, csm, qwen3_tts, misotts, s2_pro, neutts, dia, cosyvoice3-LM, voxtral_tts, dots, vibevoice.

### L4 — Step-bucket cohort for the flow/diffusion HEAD (the diffusion batcher)
- **What:** group concurrent slots at the SAME inner ODE/denoise step into one `[K,...]` forward (`scheduler/cohort.rs StepIndex`). **NOT applicable to any codec-AR-TTS model** (those are AR frame-sync lockstep — that's the L1/L2/Fork-A1 axis).
- **Byte-identical CAVEAT:** the head is itself a reducing GEMM over the slot axis. **f32 + equal-shape padding** → maxΔ=0 (supertonic `synthesize_batch` precedent, 2.33×@B8). **bf16/fp16 continuous-latent heads** (dots patch, vibevoice/cosyvoice3 mel) reassociate sub-ULP and compound → must run Fork-A1 (per-slot B=1 reducing dispatch, only non-reducing glue + per-step noise draw fused). FSQ-quantized output (voxtral_tts) stays CODE-identical within rounding margin.
- **Lifts:** the flow/diffusion heads of the hybrids — cosyvoice3 (CFM tail), voxtral_tts (per-frame rectified-flow), dots (per-patch DiT), indextts2 (one-shot DiT-CFM back-half), vibevoice (DDPM) + the pure-diffusion set (omnivoice/viitorvoice/irodori/pocket_tts/rsb).

### Determinism guard (shared, applies to L2/L4)
Per-slot RNG isolation + preserved draw-ORDER (the **D2 fix**): replace process-global `tch::manual_seed` in the mux loop with per-slot `PerSlotRng` (content-keyed `rng_base`). Already pre-empted for higgs/higgs_v2/neutts; host-PCG models (voxcpm2/supertonic/vieneu/irodori) sidestep by keeping one noise instance per slot.

**Generalization claim:** L1 (graph) + L2 (on-device sampling) + L3 (cast fusion) are nn/+AccelBackend constructs. Build them once for dia2/csm, and every structurally-similar model (depth-class for L2, bf16-sandwich for L3, graphable for L1) inherits them with only a thin per-arch capture-binding + force-solo oracle — NOT a rewrite.

---

## 2. Per-MODEL targets — current RTF → RTF<1 → levers → byte-identical? → expected gain

RTF numbers are solo unless noted "serve". "✅ byte-id <1" = reaches realtime in the DEFAULT Accuracy path. "⚠ lossy-only" = byte-identical levers shrink the constant but only the opt-in Throughput tier crosses <1. "throughput" = single-stream stays >1; aggregate RTF<1 via the batch ring/step-bucket.

### 2.1 codec-AR TTS (the AR codec fleet — Fork-A1 ragged ring, codes-identical-to-solo)

| Model | dtype | current RTF | levers (byte-id) | reaches RTF<1 byte-id? | expected gain |
|---|---|---|---|---|---|
| **qwen3_tts** | bf16 | **0.63–0.71 serve, flat B1→16** (FLEET-PERF:28) | already L1-latent; done | **✅ ALREADY** | headline win, flat to B16, 0 shed |
| **csm** | bf16 | 1.06 eager → **0.805** L1-graph (B45) | L1 (done, default-on) + L2 depth de-serialize | **✅ CLOSED** | RTF 1.007→0.805, 4000/4000 byte-id |
| **neutts** | bf16 | **0.77–0.81** (B41/B57) | small 0.5B, q=1; nothing needed | **✅ ALREADY** | (TRT int8 0.374 is LOSSY, opt-in) |
| **dia2** | bf16 | 2.08 solo / **3.4 serve** | L1 (−19%, done) + L3 + **L2/B3 depformer de-serialize (task#133)** + GAP-A ring-graph | **partial** → ~1.3–1.6; **RTF<1@B1 NEEDS B3** | −19% measured; B3 is the <1 lever |
| **misotts** | bf16 | **UNMEASURED** (8B, download-gated) | latent L1 (graph 300M depth, csm-twin) + L2 | **MEASURE FIRST**; 8B GEMMs may compute-bind | likely needs lossy tier if compute-binds |
| **s2_pro** | bf16 | **3.566** (FLEET-REGRESSION) | latent L1 (graph 4L fast-AR) + L2 fast-AR + firefly-DAC budget | ✗ byte-id (heaviest depth-serialized) | shrinks constant; <1 needs Throughput |
| **dia** | bf16 | **2.77–2.94** CUDA-bf16 | L3 partial ONLY (no sub-decoder; MATH-pinned `finfo.min` mask not graphable) | **✗ STUCK byte-id** | ⚠ lossy: dynamic-shape TRT → forked codes |
| **higgs** | fp16 | **1.17–1.68** | L3 light (fp16, no full sandwich); no byte-faithful graph (growing-contiguous, RopeApply::Start) | **✗ STUCK ~1.1** | ⚠ lossy TRT-fp16 1.166→1.051 (barely, forked) |
| **higgs_v2** | fp16 | ~1.0–1.4 est (NO in-tree RTF) | same as higgs (parallel-codebook, no L2 chain) | **✗ STUCK** | ⚠ lossy TRT only |

### 2.2 hybrid AR + flow/diffusion (AR ring + per-slot flow/DiT/DDPM head)

| Model | dtype | current RTF | levers (byte-id) | reaches RTF<1 byte-id? | expected gain |
|---|---|---|---|---|---|
| **cosyvoice3** | bf16/f32 | **0.54 solo, 0.54→0.69 serve flat** | done; L4 step-bucket CFM tail for aggregate throughput | **✅ ALREADY** | byte-id-on-codes (CFM mel maxΔ 4.9e-3 BLAS floor) |
| **vibevoice** | bf16 | **0.583 solo** (Δ=0 backbone, DPM step-0 Δ=0) | L1 DDPM head ([2,64] clean) + L3 + sized dual-ring | **✅ ALREADY** (solo) | throughput cap = 2nd CFG ring + per-token VAE |
| **voxtral_tts** | bf16 | **1.01 solo → 0.93 B16** (FLEET-PERF:30) | **L1 graph the length-3 flow head (clean) + backbone step** | **✅ likely <1 byte-id, NO lossy** | the ONLY model that improves with load |
| **indextts2** | f32 | **UNMEASURED** | L1 graph AR step + L1 the one-shot 25-step DiT-CFM back-half | **MEASURE FIRST** (f32 GPT-2 AR small → likely <1) | watch P4: BigVGAN CUDA-only (no CPU twin) |
| **dots** | bf16 | **3.077 solo** | L1 (fixed-shape DiT, beyond ×1.04) + L2 device-EOS + L3 + L4 | **✗ @B1** (10-step × CFG-2 × 18L intact) | byte-id route to <1 = THROUGHPUT (many streams) |

### 2.3 STT (encoder once + AR/CTC/transducer decoder) — single-stream realtime ALREADY closed

| Model | dtype | current RTF | levers (byte-id) | reaches RTF<1 byte-id? | the gap |
|---|---|---|---|---|---|
| **voxtral** | fp16 | **0.64** (has CUDA-graph + dev-argmax) | reference impl of L1+L2 | **✅ ALREADY** | the pattern to generalize |
| **granite** | bf16 | **0.144** | L1 (zero graph today) + **L2 on-device argmax** (still D2H full vocab) | **✅ ALREADY** | P5 Contiguous → CUDA-graph candidate |
| **ark** | fp16 | **0.03** | L1 + L2 (still D2H, `ark.rs:595`); preserve bad-words mask | **✅ ALREADY** | generalize voxtral pattern |
| **cohere** | fp16 | **0.24** (q4) / <1 (tch) | already device-argmax; L1 + cross-attn ragged wrinkle | **✅ ALREADY** | per-row enc-K/V mask convention |
| **higgs_stt** | bf16 | <1 (ark/granite class) | L1 + L2 (still D2H, `higgs_stt.rs:256`) | **✅ ALREADY** | P5 ViewContiguous read-back |
| **whisper** (ORT) | — | **0.057** (17.7×RT), agg 0.83@16 | equal-shape encoder cohort (1.19×) | **✅ ALREADY** | ORT track, not tch L1 |
| **parakeet/sensevoice** (ORT) | — | **0.05 / 0.08** | encoder-compute-bound; equal-context cohort | **✅ ALREADY** | CTC/TDT byte-id by math |

**STT verdict:** no new realtime physics. The whole class is RTF 0.03–0.64 at B=1. The broadened goal reduces to byte-identical HIGH-BATCH concurrency: flip the already-built Fork-A1 rings (5 tch STTs) after the shared serve-loop shed, generalize L1 from voxtral, apply L2 on-device argmax to ark/granite/higgs_stt.

### 2.4 S2S + one-shot

| Model | dtype | current RTF | levers (byte-id) | reaches RTF<1 byte-id? | the gap |
|---|---|---|---|---|---|
| **hibiki** (tch) | f32 | **NO CUDA RTF** (only CPU-f32 22.8) | **MEASURE first**; L1 (28L backbone + 16-stage depformer, f32 = easy byte-id) + L2 (kill 17 D2H/frame) | per-FRAME 80ms budget, not whole-RTF | needs `as_duplex` + batched `DuplexStepModel` |
| **lfm2** (ORT) | — | ASR 0.280; turn 5.94s | ORT device-KV track (NOT tch L1); P3 hybrid conv+attn cache stays PER | ✅ ASR | stream depthformer frames; batching = PER |
| **kokoro** (ORT) | — | **0.15 flat to N=16** | one-shot VITS; equal-L `synthesize_batch` | **✅ ALREADY** | CUDA blocked by StyleTTS2-LSTM scar (CPU-pinned, perf-OK) |
| **melo** (ORT) | — | <1 (VITS one-shot) | `synthesize_batch` already wired (`melo.rs:212`) | **✅ ALREADY** | 2 unseeded RandomNormalLike → noise-pinned re-export for B>1 maxΔ=0 |

---

## 3. GENERALIZATION design — extend dia2's primitives without per-model reinvention

The two dia2 keystones are **(a) on-device-sampling (L2, B3)** and **(b) the `ByteIdenticalGraph` AccelBackend (L1)**. Both are SHARED substrate. Here is how each family inherits them, and where bespoke work is genuinely required.

### 3.1 `ByteIdenticalGraph` AccelBackend (L1) — already the auto-select default
- `is_compatible` gates on `dev.vendor()==Nvidia && model.graphable` (priority 40, between Eager=0 and TorchTensorRt=80), falls to Eager off-CUDA/non-graphable — **no hardcoded `is_cuda`** (`device.rs:119-184 graphable_cuda_graph_enabled`). The lossy-exclusion invariant (`graphable ⇒ never TRT`) is preserved.
- **Inheritance is just flipping `model.graphable=true` + supplying a fixed-shape capture binding.** csm/dia2/omnivoice/dots already do. **The generalization work** = wire the latent-graphable models into the same callers: qwen3_tts (5L CodePredictor), misotts (300M depth, csm-twin), s2_pro (4L fast-AR), and the hybrid heads voxtral_tts (fixed length-3 flow seq — captures cleanly), vibevoice (DDPM [2,64] — clean), indextts2 (25-step DiT-CFM, fixed-shape), and hibiki (28L backbone + 16-stage depformer, f32).
- **Bespoke per arch:** the read-back-kernel-preserving graph variant (`kv_cache.rs:213-224` — csm needs `append_contiguous_graph` to keep flash/mem-efficient SDPA; dia2 needs `append_full_masked`'s `finfo.min` to force MATH; B27 scar). Each arch picks its read-back to match the eager kernel-pick, then gates. NOT graphable: dia (MATH-pinned mask, no sub-decoder), higgs/higgs_v2 (growing-contiguous backbone, RopeApply::Start) — they stay eager / lossy-tier.

### 3.2 On-device sampling (L2) — one `nn/sampling.rs` primitive, N consumers
- `sample_token_on_device` + `broadcast_to` is BYTE-FOR-BYTE the host draw (Philox offset advances at multinomial enqueue, `sampling.rs:18-28,54`). dia2 runs all 31 stages this way with ONE bulk D2H/frame.
- **Inheritance:** the depth class chains the same `index_select(0, prev_audio)` on the device id — csm, misotts, s2_pro, qwen3_tts CodePredictor, hibiki depformer. The greedy STT class swaps `multinomial` for on-device **argmax** (voxtral's `argmax_first_device` is the reference) — generalize to ark/granite/higgs_stt, preserving first-max tie-break + ark bad-words + cohere REPEAT_GUARD on-device.
- **Bespoke:** the per-stage capture table + warmup-before-capture (`dia2.rs:619-688 step_graph`, WARMUP=2) per depth arch; the RNG Philox save/restore around capture (`capture_preserving_rng`, the B43 scar) — already shared by csm/omnivoice/dots.

### 3.3 The high-batch ring (Fork-A1) — already built across ~18 models
- `RaggedSlotRing` (`nn/ragged_ring.rs:20-39`) is the SHARED device-resident per-slot KV ring; each slot's reducing GEMM/SDPA is a B=1 dispatch against its OWN row → byte-identical-vs-solo BY CONSTRUCTION in any dtype. Win = host-restream elimination (2.34×@B24 class), NOT 30×.
- **Inheritance is structural:** all 9 codec-AR + 5 hybrid + 5 STT already have Fork-A1 paths built (`*_force_solo_codes.rs` oracles exist). Generalization = flip gated→default AFTER the shared serve-loop shed.
- **Bespoke:** CFG-grouped-ring sizing (dia2/dia 2 rows/slot, vibevoice 2-ring, vibevoice_realtime 3-ring) → `MAX_SLOTS` counts physical rows, `group=branches`; MoE mask-all-experts (zonos2); cohere cross-attn per-row enc-K/V mask; lfm2 hybrid conv+attn cache stays PER.

### 3.4 Where bespoke work is genuinely irreducible
1. **dia / higgs / higgs_v2** — no byte-faithful graph path (MATH-pinned mask OR growing-contiguous backbone). Reaching RTF<1 needs EITHER a new byte-faithful graph for growing-contiguous/MATH-pinned backbones (research gap) OR the lossy Throughput tier.
2. **hibiki / lfm2 S2S** — `LoadedModel` has NO `as_duplex` verb (only `as_stepped`). Needs a new `DuplexStepModel` impl + S2S analog of `codec_ar_batcher` (the only real impl is test-only `CodecArDuplexModel`).
3. **misotts / indextts2 / hibiki** — UNMEASURED on CUDA; must measure RTF before committing a lever.
4. **melo** — 2 unseeded in-graph RandomNormalLike → needs a noise-pinned re-export before a co-batched row is sample-identical.

---

## 4. The opt-in LOSSY Throughput tier (clearly separated — NOT the default)

Reachable ONLY under `PerfMode::Throughput`; produces a forked-but-valid output; each needs its own golden + honest `trt_active`/"forked-not-byte-identical" served-metadata label. These are for the models that CANNOT reach RTF<1 byte-identically.

- **T1 — Torch-TensorRT FP16** (the existing seam, `trt.rs`, proven on GB10 for FP16 only — TRT 10.16.1 / torch-tensorrt 2.12 floor; INT8/FP8/NVFP4 have documented sm_12x gaps, never default). Wired for neutts/dia/higgs. dia2 deliberately EXCLUDED (graphable ⇒ never-TRT). **Targets:** dia (3.4→ forked, still not <1), higgs (1.166→1.051, barely), higgs_v2, misotts/s2_pro if they compute-bind. backbone+depformer dia2 TRT → ~2.0 (forked, still not <1).
- **T2 — step-count reduction / flow distillation** (ZipVoice 4–8 step, RapFlow consistency-FM 2 NFE, OZSpeech one-step). The ONLY route to **dots RTF<1@B1** (10-step × CFG-2 × 18L is irreducible byte-id). Retrains a different velocity field → forfeits byte-identity.
- **T3 — SDPA flash/online-softmax swap** — different reduction order forks codes; FA2-class-only on sm_121 (**NEVER FA3/FA4/FlashInfer** — FA3 Hopper-only, FA4 datacenter-Blackwell sm_100/103 only; GB10 lacks TMEM/tcgen05). Only a Throughput PREFILL lever for the longer-context ASR encoders, behind its own golden. Process-wide hazard (`setSDPUseFlash` flips global libtorch context).
- **T4 — fused-[B] reducing GEMM (Fork B, the 30× lever)** — batched-vs-batched only; in bf16/fp16 flips codes vs solo (measured 1/4@B4). The ONLY byte-identical-vs-solo route is **batch-invariant kernels** (Thinking-Machines / SGLang deterministic) — not in-repo, ~25–45% kernel cost (graph-recoverable), the literature answer to make the 30× exact. High-value research gap.
- **EXCLUDED entirely:** quantization (INT8/FP8/FP4 — forks AR codes; grouped FP4/FP8 also BROKEN on sm_121), custom CUTLASS grouped GEMM, SoundStorm non-AR parallel decode.

**Honest separation:** every RTF gain from launch/gap ELIMINATION (L1–L4) is byte-identical and is the default; every gain from doing FEWER/CHEAPER steps (distillation, low precision) is lossy and opt-in.

---

## 5. HIGH-BATCH — every model's ring/step-bucket + admission/sizing

- **Rings BUILT, codes-identical-to-solo, gated→default pending the shared serve-loop shed:** 9 codec-AR (Fork-A1 `RaggedSlotRing`); 5 STT tch decoders (voxtral/cohere/ark/granite/higgs_stt — `*_force_solo_codes.rs` exist); 5 hybrid AR-axis rings (cosyvoice3/voxtral_tts/dots/indextts2/vibevoice, `as_stepped()->None` by default). Default-on serve flags already: qwen3_tts/dia2/dia/higgs_v2/neutts; opt-in: csm/misotts/s2_pro/higgs.
- **Step-bucket (L4) for the flow/diffusion HEADS** — the deferred Phase-5 workstream (own `flow_cfm_cohort_maxdelta_zero` gate). supertonic is the proven precedent (maxΔ=0.0, 2.33×@B8); melo `synthesize_batch` wired; kokoro needs `synthesize_batch` wiring.
- **The SHARED batch blocker (must land FIRST):** serve-loop graceful-shed hardening (PATHB §1.4 / §4.2) — the single `codec-ar-mux` thread hangs at n=2/4, empties WAVs at n=8, crashes at n≥16. Every tch device batcher inherits it. This is the **D1 prerequisite the whole fleet shares**.
- **Admission/sizing (Phase 0, accuracy-neutral, box-kill guards):**
  - **BUG-1 (layer-less footprint):** `KvFootprint::total_bytes()` (`gqa.rs:227`) has NO `n_layers` factor → under-counts 24–60× → over-admit → ring blows the 48 GiB arena. Fix: `per_slot_ring_bytes = n_layers × footprint(MAX_SEQ)`.
  - **BUG-2 (dtype-blind):** `KV_ELEM_BYTES=2` const → f32 cells silently halved → 2× over-admit. Fix: dtype-seeded `kv_elem_bytes` (bf16/fp16=2, f32=4).
  - **Budget reconcile:** `min(cuda_arena_limit [48 GiB], free_mem.unwrap_or(arena·0.5))` — NEVER `total_mem` on the unified pool (twice-observed box-kill).
  - **Vocoder transient (§4.4):** decode-concurrency semaphore + 2nd `VramAccountant` leg = `floor(free_arena / per_decode_transient)`, decode-batch width DECOUPLED from AR cohort. Protects dots/indextts2/voxcpm2 (48 kHz/BigVGAN, ~21.7 GiB S3Gen-class transients).
  - **CFG-grouped-ring sized cap** (D1): `MAX_SLOTS` counts physical rows, `group=branches`; dia 2-row, vibevoice 2-ring, vibevoice_realtime 3-ring.
  - **Per-slot RNG isolation** (D2): `PerSlotRng` keyed on `(slot,step)`, pre-empts higgs/higgs_v2/neutts/cosyvoice3/vibevoice_realtime divergence.

---

## 6. Dependency-ordered roadmap

```
SHARED SUBSTRATE (land once, whole fleet inherits)
 S0  Serve-loop graceful-shed (D1 prereq) + graph-mode teardown SIGSEGV fix   [unblocks EVERY batcher]
 S1  Phase-0 sizing: BUG-1(n_layers) + BUG-2(dtype) + budget reconcile        [box-kill guards, no behavior change]
      + vocoder-transient semaphore + CFG-grouped-ring sized cap + PerSlotRng
 S2  ByteIdenticalGraph AccelBackend auto-select (DONE) + force_solo_oracle::<M>  [the L1/L2 substrate + the long-pole oracle]

REALTIME (byte-id, PerfMode::Accuracy) — generalize dia2's L1/L2
 R1  dia2 L2/B3 on-device depformer pipelining (task#133)         [the dia2 RTF<1 lever, STRUCTURAL]  ◀ #1
 R2  Generalize L2 on-device argmax → ark/granite/higgs_stt       [kill per-step full-vocab D2H]
 R3  Generalize L1 graph → qwen3_tts/voxtral_tts/vibevoice/indextts2 heads + hibiki   [latent graphable set]
 R4  MEASURE misotts/indextts2/hibiki CUDA RTF                    [un-quantified gaps — decide lever after]
 R5  dia2 GAP-A ring-graph (bucketed by cohort width)            [serve-path RTF~3.4 win]

HIGH-BATCH (byte-id) — flip the built rings after S0
 B1  Flip 9 codec-AR + 5 STT + 5 hybrid rings gated→default (per force-solo oracle GREEN)
 B2  L4 step-bucket the flow/diffusion heads (supertonic precedent) + kokoro synthesize_batch + melo noise-pin

LOSSY THROUGHPUT (opt-in, parallel, lower priority) — only for the byte-id-stuck models
 T1  dia / higgs / higgs_v2 TRT-fp16 (forked codes, honest label)
 T2  dots flow-distillation (the only dots RTF<1@B1 route)
 T4  batch-invariant kernels (make the 30× Fork-B exact — research)

S2S (new workstream)
 D1  LoadedModel::as_duplex + DuplexStepModel for hibiki + S2S codec_ar_batcher analog
```

---

## 7. Honest fleet status — who reaches RTF<1 byte-identical

- **ALREADY RTF<1 byte-id (ship as-is):** qwen3_tts, csm, neutts, cosyvoice3, vibevoice (solo), kokoro, melo, + ALL STT (voxtral/granite/ark/cohere/higgs_stt/whisper/parakeet/sensevoice, 0.03–0.64).
- **Reachable RTF<1 byte-id with the generalized levers:** voxtral_tts (L1 flow-head graph, NO lossy needed), dia2 (NEEDS B3 depformer pipelining — the structural lever), indextts2 (f32 AR small, measure-then-graph).
- **MEASURE FIRST (un-quantified):** misotts (8B may compute-bind), hibiki (only CPU-f32 22.8 exists — no CUDA number).
- **STUCK byte-id → lossy Throughput tier needed for RTF<1:** dia (no graph lever), higgs / higgs_v2 (no byte-faithful graph for growing-contiguous backbone), s2_pro (heaviest depth-serialized), dots @B1 (irreducible 10-step×CFG-2×18L — byte-id route is THROUGHPUT via many concurrent streams, not single-stream <1).
- **HIGH-BATCH:** every model has a built ring (codec-AR/STT/hybrid) or step-bucket (diffusion/one-shot) path, codes-identical-to-solo; all gated behind the ONE shared serve-loop graceful-shed fix (S0).

**#1 highest-leverage shared primitive to build next: L2 on-device sampling generalized into the depth/AR de-serializer (dia2 R1/task#133 first).** It is the only byte-identical lever that pushes the depth-bound models toward RTF<1@B1 (dia2 depformer = ~50% of wall, the most-serialized chain), it is SHARED `nn/sampling.rs` substrate so csm/misotts/s2_pro/qwen3_tts/hibiki inherit it, and it is gated by S0 (serve-loop shed) + S2 (force_solo_oracle) which the whole high-batch program needs anyway.
