# Path-B (tch / libtorch) Arbitrary-Batch Serving — FINAL Implementation Plan

<!-- SKELETON: each section filled by a separate edit. -->

## 0. Ground truth, the acceptance bar, and the headline fork

**2026-06-25. GB10 (Grace-Blackwell sm_121, 121 GiB unified CPU+GPU pool) + aarch64 CPU floor. `tch`/libtorch
in-process (Path B); `ort = 2.0.0-rc.12` (Path A, for the proven precedent only).**

**Goal.** Make Path B (in-process tch) **and every currently-implemented model** serve at *arbitrary* batch
size B — B chosen at runtime by the **hardware** (roofline/free-mem) and the **live set** (cohort mix / seq
lengths) — with the most optimal perf and **ZERO accuracy loss** (decoded codes / argmax tokens / transcript
byte-identical, the AR-compounding invariant). This plan is grounded in the four REVIEW docs and re-verified
against the live tree at `/home/bud/ditto/waav/waav-infer`.

### 0.1 The five verified ground-truth facts the plan is built on

1. **G2 (structural blocker): zero tch models reach the lockstep batcher.** `ArStepModel` is the universal
   arbitrary-B seam (`crates/waav-infer-runtime/src/arstep.rs:496` `trait ArStepModel`, `:527` `step_batch`),
   but `grep -rn "impl ArStepModel" crates/waav-infer-backend-torch/src/` returns **EMPTY**. Every tch model
   inherits `as_stepped()->None` (`crates/waav-infer-core/src/model.rs:452`), so NONE is reachable by
   `Driver::tick → step_batch` or `serve_codec_ar_multiplexed` or `CodecArBatcher` (`codec_ar_batcher.rs:402`
   `g.as_stepped()`). Chatterbox-ONNX (Path A) is the ONLY real `ArStepModel`.

2. **The 30.2×@B64 is a PREFILL probe, not a serving guarantee.** `qwen3_tts.rs:1636 talker_logits_batched`
   builds a fresh `KvCache::new(b, …)` (`:1653`), runs ONE rectangular `[B,L]` prefill forward, returns
   logits, and **rejects ragged rows** (`:1645` `"ragged or empty rows"`). The real serving decode is q_len=1
   against a **growing** per-slot ring (`:1777` `KvCache::new(1, …)`). PATHB §2 caveat: "30× is an upper bound
   for the *prefill* shape, not a serving-loop guarantee."

3. **The bf16 batched GEMM FLIPS codes vs solo, and even fp32 is not logit-zero at scale.** PATHB §1a measured
   CUDA-bf16 argmax flips 1/4@B4, 1/8@B8 (logit Δ 0.31→4.24); CUDA-f32 never flips (Δ~1e-4). `cfg_batch.rs:13`
   measured at the real H=1024: fp32 batched-vs-solo is **9.5e-7, `torch.equal == False`**, even with
   `use_deterministic_algorithms(True)`. **Literal `max|Δ|=0` on *logits* vs a B=1 solo is mathematically
   unachievable for a fused `[B]` reducing GEMM at production hidden size.**

4. **The hardware-sizing machinery exists but is DEAD.** `cuda_roofline`/`batch_knee`/`BatchProfile`
   (`admission.rs:1286-1386`), `KvFootprint::total_bytes` (`gqa.rs:227`) are implemented with ZERO non-test
   consumers. The live loop uses `const MAX_SLOTS = 24` (`codec_ar_batcher.rs:47`) and a flat
   `DEFAULT_BYTES_PER_STREAM = 256 MiB` (`codec_ar_admission.rs:52`).

5. **The tch device-resident ring exists and has NO ORT alias limit.** `nn::KvCache::new(batch, …)`
   (`kv_cache.rs:104`) pre-allocates `[batch, kvh, max_seq, d]` once and writes in place via `index_copy_`
   (`kv_cache.rs:259`). Its `batch` axis is used only for CFG branches (B=2) today; its `write` advances ONE
   shared `self.cur` (`kv_cache.rs:261`). Unlike the ORT Path-A device-KV (capped at 2.34×@B24 by the rc.12
   `pub(crate)` `past==present` alias limit, FINAL-STATUS §4), the tch ring **is** the in-place
   `index_copy_` ring — no alias limit.

### 0.2 The acceptance bar — DEFINED, because the literal one is unsatisfiable

The task says "byte-identical (max|Δ|=0): decoded codes / transcript / max|Δ|=0". Fact #3 proves `max|Δ|=0`
**on logits vs a B=1 solo** is impossible for any fused `[B]` reducing GEMM at H=1024 in *every* dtype. So the
bar is defined as the standing in-tree gate already enforces:

> **BYTE-IDENTICAL ≡ decoded codes / argmax tokens / transcript identical, `max|Δ|=0` on the INTEGER emitted
> codes.** This is exactly what `assert_identical` (`harness.rs:301`) and the `ar_compounding` gate
> (`cfg_batch.rs:472`) compare — integer-only `Eq` types; "close" is uncompilable (`cfg_batch.rs:16-17`).

This is the **sacred AR-compounding invariant**: one flipped code at stride t corrupts the whole tail. It is
satisfiable and is the bar every gate in §8 enforces.

### 0.3 THE HEADLINE FORK — must be resolved before fan-out (the keystone decision)

Two distinct wins were conflated in earlier drafts. They are **mutually exclusive on the reducing path** and
the plan picks PER ARCH, never globally:

- **FORK A — byte-identical-vs-SOLO (the literal bar).** Each slot's *reducing* GEMM/SDPA is dispatched as an
  independent B=1 op against its own device-resident ring row; only the **non-reducing** row-independent work
  (embeds, RoPE apply, mask build, per-row argmax/sampler) is fused over the leading axis. This is the
  `cfg_batch.rs:19-34` two-pass discipline (`CfgReduceAxis::BatchStackedGemm` is a typed reject). The ONLY win
  over today's solo is **device-resident-KV re-stream elimination** — the same mechanism Path-A proved at
  2.34×@B24 (FINAL-STATUS §3.1), **NOT** 30×. Launch amortization applies only to the cheap non-reducing glue.

- **FORK B — fused-`[B]` reducing GEMM (the throughput lever, the 30× curve).** ONE `[B,…]` backbone/SDPA per
  stride. Real throughput, but its identity contract is **batched-vs-BATCHED only** (the dia2/csm CFG-B=2
  convention: 608/608, 544/544 — there is no solo reference to diverge from). Admissible ONLY where (i) the
  arch *always* runs batched (CFG, fixed replica) so no solo path exists, OR (ii) the dtype is f32 (CUDA-f32
  never flips, fact #3) optionally via f32-accumulate. **In bf16/fp16 with a live solo path, Fork B is
  FORBIDDEN until the bf16 floor is resolved (§2).**

**Why the shared-ring SDPA does not rescue Fork A's perf:** the GQA SDPA over a shared `[B,kvh,…]` ring is
itself a reducing batched GEMM (`attention.rs sdpa_gqa_manual`: `qg.matmul(k.T)` then `att.matmul(v)`), and
`self_attention.rs:405-409` documents that folding the batch/head leading axis "reassociates the reduction
~6e-5 on CUDA f32 (compounds over 28 layers — the omnivoice scar)". So Fork A's reducing attention stays
per-row; only the glue amortizes. **Fork A's realized speedup is therefore a HYPOTHESIS to re-measure on the
real growing-KV decode loop (§7), expected near the proven 2.34× device-residency win, not 30×.**

**The keystone:** the 30× headline belongs to Fork B, which is NOT byte-identical-vs-solo at dynamic B in
bf16. The byte-identical-vs-solo guarantee at arbitrary B belongs to Fork A, whose realized win is
device-residency (~2×-class), not 30×. **A reviewer/owner must ratify** that the production dynamic-B serving
contract is "codes deterministic per cohort + byte-identical-vs-solo where Fork A applies", because no single
tier gives both 30× AND byte-identical-vs-solo on a bf16 model. This fork leads the plan; everything downstream
is an instance of it.

## 1. The batched-step serve seam for tch (wiring the device-resident ring + step_batch)

### 1.1 The seam already exists and is backend-free — the wiring lives entirely BELOW it (P-8)

The runtime is backend-free (`driver.rs:8-15` "names no `ort`/`candle`/`ggml` AND no `SlotTable`") and is
UNCHANGED whether KV is host- or device-resident:

- `driver.rs:229 tick(&mut dyn ArStepModel, &ActiveSet)` builds `Vec<StepInput>`, calls `step_batch` ONCE per
  tick (`:256`), asserts `stepped.len() == step_inputs.len()` (`:257`).
- `serve.rs:650 live: Vec<Option<LiveStream>>` is a fixed Vec `0..max_slots`; `:778` compacts active rows via
  `filter_map`; `:547` records cohort width to `COHORT_WIDTH_METRIC` (`:552`, buckets `:563`).
- `codec_ar_batcher.rs:402 g.as_stepped()` is the single dispatch chokepoint; the moment a tch model returns
  `Some(self)` from `as_stepped()` it rides the live `codec-ar-mux` thread (`:399`) **with zero changes to
  runtime/server**. `as_stepped` is the gate (`model.rs:452` default `None`, `:585` `LoadedModel::as_stepped`).

**Invariant (P-8):** no `tch::Tensor`/`ort::Value` may leak above the seam. The batch-width decision crosses as
**pure data** (`usize`/`EpCaps`/`KvHeadLayout`), exactly like the backend-api `DeviceCaps`/`EpCaps` seam.

### 1.2 The two new tch primitives this seam needs (the core new code)

**(a) The ragged SlotId-keyed device ring.** Today `nn::KvCache` (`kv_cache.rs:104`) has ONE shared write head
`self.cur` (advanced in `write`, `:261`) — its `batch` axis is the CFG branch count (`dia2.rs:1311
KvCache::new(branches…)`). A ragged concurrent-slot cohort cannot be expressed: `serve.rs:778` compacts active
rows every tick, so a finishing slot **shifts** every later slot's compacted index (the B-index instability,
KV-ACCEL §1.3). Build a **new `RaggedSlotRing`** primitive in `backend-torch/src/nn/` that generalizes
`KvCache` from one shared `cur` to **per-slot state**:

- Pre-allocate ONCE at `[MAX_SLOTS, kvh, MAX_SEQ, d]` (the `kv_cache.rs:104` shape, but `batch = MAX_SLOTS`) —
  NEVER re-alloc on cohort churn (the GB10 OOM guardrail; per-cohort growing alloc twice-killed the box).
- A **per-row `seqlens_k: [MAX_SLOTS]`** write head (replace the scalar `self.cur`), each slot writing its new
  K/V at ITS OWN length via `index_copy_` into row `slot` — left-aligned, mirroring chatterbox-ONNX's
  left-align convention (`chatterbox.rs:838` LEFT-align + LEFT-justified mask makes each ragged row's batched
  math equal its solo math).
- A **per-row left-justified additive mask `[MAX_SLOTS,1,1,MAX_SEQ]`** (NOT the shared `[1,1,1,max_seq]` of
  `append_full_masked`, `kv_cache.rs:311`), so each ragged row attends exactly `[0..seqlens_k[slot])`.
- **Recycle-zero on slot finish** (privacy; doubly required on coherent GB10 where freed device bytes are
  host-readable) — `KvCache::reset` (`kv_cache.rs:251`) only rewinds `cur`; the ragged ring needs an explicit
  per-row device memset + `seqlens_k[slot]=0`.
- The ring is keyed by **stable SlotId (0..MAX_SLOTS)**, NOT compacted position. Membership churn = which rows
  bind this tick.

**Note — four read-back conventions, not one.** `kv_cache.rs` exposes `append_view` (`:267`, voxtral/cohere/
ark), `append_contiguous` (`:274`, csm/cosyvoice3), `append_contiguous_masked` (`:295`, dia — steers SDPA onto
the MATH backend), `append_full_masked` (`:311`, dia2 — full padded buffer + `finfo.min` mask). Each is a
documented per-arch bit-identity scar. The ragged ring must re-derive **each** under per-row `seqlens_k` + a
per-row mask, and re-gate per read-back (a per-row `narrow` is impossible for a ragged cohort → the
view/contiguous archs must return the FULL padded buffer + per-row mask at B>1, which changes the SDPA
kernel-pick — re-gate). This is `4×` the "core new primitive", not one.

**(b) The per-stride `[B,1,…]` batched-DECODE method (NOT the prefill probe).** `talker_logits_batched`
(`qwen3_tts.rs:1636`) is prefill-only (q_len=L, fresh KV, rejects ragged). The serving step is q_len=1 against
the growing ring. Build it on the **shared `Backbone::forward`** (`backbone.rs:109 forward_n_layers(embeds,
caches, …, mask, is_prefill)`) which already loops layers over a `[b,…]` hidden with per-layer caches and an
additive mask — so the substrate is arbitrary-B + mask-capable; the missing piece is per-slot ring bookkeeping,
not a new kernel.

### 1.3 The `ArStepModel` impl a tch model adds (the thin per-arch surface)

A tch model becomes batchable by implementing `ArStepModel` over its tch state and overriding `as_stepped`:

```
impl ArStepModel for TorchQwen3Tts {
    fn prefill(&mut self, slot, text) -> StepOutput  // scatter prefill KV into ring row `slot`
    fn step(&mut self, &StepInput) -> StepOutput     // per-slot fallback (B=1, correct everywhere)
    fn step_batch(&mut self, &[StepInput]) -> Vec<StepOutput> {
        // FORK A: per-slot B=1 reducing dispatch against ring rows + fused non-reducing glue
        // (or FORK B where ratified: one [B,1,…] backbone forward over the active ring rows)
    }
    fn decode_audio(&mut self, slot, body) -> Vec<f32>   // per-slot today; §4 adds decode_audio_batch
    fn reset_slot(&mut self, slot)                       // recycle-zero ring row `slot`
    fn output_sample_rate(&self) -> u32
}
fn as_stepped(&mut self) -> Option<&mut dyn ArStepModel> { Some(self) }  // the chokepoint flip
```

The default `step_batch` (`arstep.rs:527-532`) is a per-slot `step` loop — correct, B=1-equivalent, the Fork-A
fallback. The override MUST be bit-identical to it for the cohort it accepts (`arstep.rs:511-526` contract); a
cohort it cannot batch bit-faithfully returns the per-slot result, never an approximation.

### 1.4 Serve-loop concurrency hardening is a HARD PREREQUISITE to flipping `as_stepped`

FINAL-STATUS §5: the multiplexed batched-DEVICE serve branch **hangs at n=2/4 (→30s watchdog shed), empties
WAVs at n=8, crashes the single `codec-ar-mux` thread at n≥16** (→500 until restart). This is backend-agnostic
(the single-mux-thread `codec_ar_batcher.rs:399` failure), so **every** tch device batcher inherits it. Order:
the `serve_codec_ar_multiplexed_bounded` + single-mux-thread concurrency MUST be hardened (graceful typed shed,
NOT a thread crash) and proven green at n=2/8/16/32 **before** any tch `as_stepped()->Some` flip. Gate the
override behind a regime flag so the B=1 one-shot path stays default until the concurrency gate is green
(§8, §9). Fixing this once unblocks BOTH the deferred Path-A device-KV AND every tch batcher.

## 2. The BF16-FLOOR byte-identical resolution (the accuracy keystone)

### 2.1 The floor, exactly (verified)

A batched `[B,…]` cuBLAS GEMM reassociates the K-dim reduction tree vs a B=1 GEMM. In bf16/fp16 the ~0.3–4.2
logit Δ FLIPS the argmax code (PATHB §1a: 1/4@B4, 1/8@B8 on `talker_logits_batched`). In f32 the Δ is ~1e-4
and never flips a greedy code (CUDA-f32: 0/N at every B). `cfg_batch.rs:13`: even fp32 batched-vs-solo at
H=1024 is **9.5e-7 ≠ 0** (`torch.equal == False`). **Serving-dtype manifest** (the dtype decides whether the
floor fires):

| dtype on CUDA | models (verified) |
|---|---|
| **bf16** | qwen3_tts (`:1467`), dia2 (`:932`), csm (`:611`), dia, s2_pro, neutts, misotts, zonos2, dots, cosyvoice3-LM, granite, canary_qwen, higgs_stt, vibevoice, vibevoice_realtime, vibevoice_asr |
| **fp16 (Half)** | higgs (`:412`), higgs_v2, voxtral (`:266`), cohere (`:228`), ark (`:266`) |
| **f32** | omnivoice, viitorvoice, irodori, pocket_tts, rsb, hibiki; **all CPU paths** (`is_cuda()…else Kind::Float`) |

**fp16 is NON-EXEMPT.** The flip is a property of *any* non-f32 GEMM accumulation; fp16 reassociates exactly
as bf16. It has NARROWER exponent range; its flip rate is its OWN empirical quantity (there is ZERO fp16
batched-vs-solo measurement in-tree — the only probe `qwen3tts_batch_scaling.rs` is bf16-only). fp16 models
get their OWN tier row + their OWN force-solo oracle. The f32 lm_head + f32 argmax that
higgs/cohere/ark/higgs_v2 already do (e.g. `ark.rs:430`) removes only the head projection's rounding; the fp16
DECODER BODY (`hidden_last`) still reassociates — the f32 head is NOT evidence of batch-safety.

### 2.2 The resolution ladder (per arch, per dtype, per hardware)

**The byte-identity oracle is per-fork, never a fuzzy logit max|Δ|.**

- **FORK A / A1 — NON-REDUCING-FUSION (the literal byte-identical-vs-SOLO lever, default).** The reference is
  the per-slot `step` loop where each slot's reducing GEMM/SDPA is its own B=1 dispatch; the SUT fuses ONLY the
  row-independent non-reducing work (`CfgReduceAxis::PerRowHidden`/`PerRowSequence` are safe; `BatchStackedGemm`
  is the typed reject, `cfg_batch.rs:72,80`). Byte-identical-vs-solo BY CONSTRUCTION in any dtype, because the
  batch index never enters a reduction. **Perf: NO reducing-GEMM speedup** — the win is device-resident-KV
  re-stream elimination + launch amortization of the cheap glue (§7). This is the only tier that meets the
  literal bar at arbitrary dynamic B on a bf16/fp16 model.

- **FORK A2 — f32-accumulate SDPA (NEW, UNPROVEN; an optional margin-widener, NOT a free extension).** The
  in-tree `ProjPrec::F32Sandwich` (`self_attention.rs:230` `xn.to_kind(Kind::Float)`, cast back `:235`) covers
  **projections ONLY**; the QK^T/att·V reductions in `attention.rs sdpa_manual`/`sdpa_gqa_manual` run in
  `q.kind()` (bf16) with only softmax in f32. Extending f32-accumulate to the SDPA matmuls is **net-new code**
  on the op that binds past B16 (≈2× its FLOPs). It reaches ~5e-6 ("batch-invariant ENOUGH", ep.rs precedent),
  **not 0** — so it stops CODE flips but is "usually doesn't flip", which a strict all-rows stress oracle must
  prove before it is admissible. A narrow-argmax-gap row can re-flip. Reserved for the few cells where Fork A1's
  per-row dispatch is the launch bottleneck AND the perf gate shows f32-SDPA beats B parallel bf16 B=1 SDPAs.

- **FORK B — batched-vs-BATCHED-only (the throughput lever where no solo path exists).** The reference itself
  batches (dia2/csm CFG B=2, `dia2.rs:1311 KvCache::new(branches…)`, proven 608/608, 544/544). Admissible ONLY
  for fixed-replica batching (CFG) or where the model's B=1 serving path is ALSO routed through the same
  batched primitive (B=1 = batch-of-1), so solo and cohort share one trajectory and there is no solo reference
  to diverge from. **Never compared to an isolated B=1 run.**

- **FORK C — f32-serve (CPU is here; bounded).** CPU runs `Kind::Float` and is batch-invariant only to a
  **measured** limit: PATHB §1a — CPU-f32 `max|Δ|=0 up to B=16, then 1.81e-4 @B32, 6.48e-5 @B64`. So f32 is
  NOT free at arbitrary B — it is byte-identical-vs-solo only for `B ≤ B_inv(arch,hw)` (B_inv=16 the only
  measured point, AR-decode B_inv may be lower). The sizer (§5) MUST clamp the byte-identical cohort to B_inv;
  throughput above B_inv is an explicitly non-byte-identical opt-in mode (the WAAV_ORT_TF32-style honest label).

### 2.3 The TF32 global-state hazard (P0, must be enforced, not "per-model policy")

TF32 is a **process-wide libtorch global**, not a per-call scope. `kernels/mod.rs:155 allow_tf32` is INTENT
only; `DefaultPolicy::tf32_on()` (`:183`, dia2) vs `tf32_off()` (`:177`, everyone else) decides at LOAD and
flips `at::globalContext()` globally — whichever model loads last wins. dia2 REQUIRES TF32-ON for its own
byte-identity (B23); Fork-A2 batch-invariance REQUIRES TF32-OFF (`ep.rs:208-222` proves TF32-on flips a code
~stride 53; off is ~5e-6). One process cannot host both at byte-identity. Two enforced changes:

1. **PRIMARY (hard residency constraint):** lift the policy intent into a `Tf32Class {On, Off}` the registry
   reads at load; the residency scheduler **typed-refuses** to co-locate an On-class and an Off-class model in
   one process (and the batcher cohort key includes `Tf32Class` so a batched step never mixes classes). dia2
   stays on Fork B (its CFG is batched-vs-batched anyway); Fork A2 is FORBIDDEN until (2) lands.
2. **DEFENSE-IN-DEPTH:** a `Tf32Scope` RAII guard that reads `at::globalContext()->allowTF32CuBLAS()`, sets per
   model class on load, restores on unload — closing the GUI/hot-swap poisoning hole (the model-explorer
   `_unload` does NOT restore TF32). cuBLAS is the accuracy-critical axis; the cuDNN flag is re-asserted from
   the model class. Proven by an interleaved load/unload/step force-solo oracle.

### 2.4 The cfg_batch evidence is DEMOTED to "type discipline + sidecar docstring"

The `0/320 torch.equal==True at H=1024` number lives in a **module docstring** (`cfg_batch.rs:22`) describing a
Python sidecar `_logits_cfg2`; the compiled gate `cfg_batch_ar_compounding_identical` (`:472`) drives a
**test-double** model (`CfgCompoundingAr`, a HashMap emitting fake codes) and proves only that
`BatchAxisReducedInGemm` is type-refused. **No compiled tch gate yet proves a real bf16 GQA backbone
byte-identical under the two-pass discipline.** That gate (§8) is the actual long pole. Treat its first GREEN
on real qwen3 weights as the FIRST evidence Fork A1 holds on real weights — everything before it is
construction-reasoning, not proof.

### 2.5 Codecs are EXEMPT from the floor (different math)

Codec/vocoder decode has NO cross-batch reduction: the RVQ sum is over the codebook axis not the batch
(`codec/rvq.rs`, `codec/dac.rs from_codes`), convs/snake/transformer are per-row/pointwise (`dacvae.rs:366`
preserves `[B,1,samples]` end-to-end, proving the stack is batch-generic). So batched codec decode is
byte-identical in BOTH f32 AND bf16 — no Fork needed, only the mechanical `reshape([-1])`→`squeeze(1)+per-row
crop` fix + a Δ==0 ragged pad/crop gate (§4). Chatterbox S3Gen is the sole non-batchable codec (unseeded
`RandomNormalLike` → audio non-reproducible even solo) and stays per-slot.

## 3. Per-architecture batching (AR lockstep / step-bucket / S2S duplex) + per-arch gate

Three execution classes batch by DIFFERENT methodologies and must NOT be conflated. Each arch declares a
`StepClass` + `KvHeadLayout` (the thin config) and gets its own force-reference oracle.

### 3.1 AR / codec-AR — FRAME-SYNC LOCKSTEP (the ragged ring, §1.2)

`step_batch` advances all active slots ONE stride against per-slot rings. The class for dia/dia2/csm/qwen3/
higgs/higgs_v2/s2_pro/neutts/misotts/zonos2/cosyvoice3-LM/indextts2-GPT2 (+ ORT MOSS).

- **CFG-axis archs (dia2/dia/csm).** Today the `KvCache.batch` axis is the CFG branch (`dia2.rs:1311
  branches∈{1,2}`). Adding concurrent slots makes the CFG axis INNER and the slot axis OUTER → effective
  `B = 2·n`. dia2 stays Fork B (its golden IS a `[2,…]` run, no solo reference; it requires TF32-ON →
  Fork-A2 foreclosed). Its B=1 serving path MUST route through the `[2·n→2]` batched primitive so solo and
  cohort share a trajectory. **Gate:** a standalone `[2,…]` CFG run for slot s == slot s's rows extracted from
  a `[2·n,…]` cohort, on a staggered cohort, TF32-ON, with `append_full_masked`'s per-row mask.
- **Depformer/sub-decoder is NOT batched by the backbone lever (csm/dia2/misotts).** `csm.rs:32` — each outer
  step runs `num_codebooks-1 = 31` forced depth steps (`csm.rs:961 c.reset()` per outer step); the backbone
  graph wins only ~3% (PATHB G6). So these archs are **PARTIAL**: backbone-batched + depformer-per-slot, and
  realized speedup is capped by the serial depth decoder. Mark them PARTIAL with the caveat; depth-decoder
  cross-slot batching (its own B27 batched-projector scar, `[1,2,…]` vs `[1,1,…]` rounding) is a SEPARATE
  workstream, gated independently.
- **MoE (zonos2).** Router-states carried across MoE layers (top-1, layer-26 top-2), router softmax forced f32,
  the chain is sub-ULP-sensitive (`maxΔ~2.6` on step-0 hidden). Cross-slot batching must run the mask-all-
  experts variant (per-row reduction preserved, Fork A1) — FORBID permute-by-expert token-gathering until
  proven reduction-order-safe; EDA router-state strictly per-row. Dedicated zonos2 oracle before any MoE batch.

### 3.2 Masked-diffusion / flow-CFM — STEP-BUCKET (no KV ring)

`scheduler/src/cohort.rs:462 StepIndex` + the `StepBuckets` seam group slots at the SAME inner denoise/ODE step
into ONE `[B,…]` inner pass (`cohort.rs:482` "may co-batch one inner forward"). Consumers are scheduler-only
today; NO model is driven through it.

- **omnivoice/viitorvoice (masked-diffusion-LM, f32).** `omnivoice.rs:69-70` EXPLICITLY refuses to batch even
  its own 2 CFG rows ("a batch-2 GEMM reassociates the cuBLAS reduction and flips codes"). So a cross-SLOT
  step-bucket that fuses K slots into one `[B]` bidirectional forward hits the SAME trap. **Run each slot's
  bidirectional forward as a batch-1 dispatch (Fork A1 over the slot axis); fuse only non-reducing per-position
  work + the per-slot Gumbel draw IN ORDER.** "f32 = free byte-identity at arbitrary B" is FALSE at H=1024
  (cfg_batch.rs:13) through the masked-diffusion feedback; accept launch-amortization only. **Gate:** codes
  identical + the per-step RNG-draw-ORDER law (`omnivoice.rs:64` ONE `rand_like([1,8,T])` per step in order —
  a fused `[B,8,T]` draw consumes the RNG in a different order).
- **flow-CFM heads (cosyvoice3-flow, voxtral_tts acoustic, dots-DiT, indextts2-backhalf, irodori, pocket_tts).**
  The inner step is a pure function of `(latent, step)`, step-bucketable. supertonic `synthesize_batch`
  (equal-shape ODE-in-one-graph, maxΔ=0.0, 2.33×@B8) is the proven precedent. **Gate:** maxΔ=0 on the waveform
  (supertonic precedent), per-step RNG-order identical where stochastic. rsb (score-SDE enhance) = fixed-shape
  cohort.
- **voxcpm2 — a THIRD mode (fused-inner-graph AR).** Its CFM is a fused `decode_step` graph (10-step Euler in
  ONE call, emits a continuous latent patch, NO per-denoise StepIndex). It is neither lockstep-codes nor
  step-bucket-by-StepIndex: it is continuous-latent AR ring + whole-`[B]`-graph per step (supertonic-style).
  **Gate:** maxΔ=0 on the emitted latent patch (NOT integer codes — voxcpm2 emits patches).

### 3.3 Native-S2S duplex — READ-WHILE-EMIT (the SlotBatch seam)

`duplex.rs:89 trait DuplexStep` (single-slot, hibiki's seam `:84`) vs `duplex.rs:438 SlotBatch` /
`DuplexStepModel::step` (batched multi-slot, the K=2Q+1 folded-lane forward + `exec_mask` freeze, `:331`).
The ONLY `DuplexStepModel` impl is the **test-only** `CodecArDuplexModel` (ONNX, proven ragged-bit-identical at
≥4). **hibiki** uses `KvCache::new(1, …)` (`hibiki.rs:718,721`) and impls `DuplexStep` (NOT the batched
`DuplexStepModel`); it is f32 (no flip — accuracy tractable). There is NO registry dispatch verb for batched
S2S: `LoadedModel` exposes only `as_stepped` (TTS), not `as_duplex`. So S2S is a **distinct workstream**: (a)
add `LoadedModel::as_duplex(&mut self) -> Option<&mut dyn DuplexStepModel>`; (b) impl `DuplexStepModel` for
hibiki (multi-slot read-while-emit + per-slot depformer reset under `exec_mask` + ragged right-pad ~27% waste
G7); (c) build an S2S analog of `codec_ar_batcher`. **Gate:** a frozen slot (`exec_mask=false`) produces
byte-identical continuation to a never-frozen run, on a staggered freeze/unfreeze cohort, token-for-token vs
the `CodecArDuplexModel` reference. lfm2 is turn-based + a HYBRID conv(10L)+attention(6L) cache that the
attention-only ring cannot host — reclassify as its own hybrid-cache primitive or StaysPerSlot (across its
S2sModel/TtsModel/SttModel surfaces).

### 3.4 The per-arch oracle is a first-class deliverable

Each arch ships a `<arch>_batched_vs_force_solo_codes_oracle` (mirror `host_vs_device_kv_oracle`,
FINAL-STATUS:67, 83s) BEFORE it serves at B>1: a ragged staggered MID-FINISH cohort, full AR decode loop,
**all-rows** integer-CODE compare vs each row's true B=1 solo (Fork A) or vs the force-batched reference (Fork
B), on a stress corpus that includes narrow-argmax-gap rows. Factor a reusable `force_solo_oracle::<M:
ArStepModel>` so 23 archs supply only (loader, solo closure, corpus), not 23 bespoke gates.

## 4. Codec / vocoder batched decode + the vocoder-transient memory budget

### 4.1 The structural gap: decode is per-slot serialized OUTSIDE the tick

The batched AR tick advances only the LM. Audio decode runs per-slot, serialized, in the drain loop
(`serve.rs:871 drain_finished_stream` per cell). The trait seam is per-slot (`arstep.rs:540 decode_audio(slot,
body)`); there is **NO `decode_audio_batch`** in the tree. So a batched B-slot cohort fans out to N serialized
whole-body decodes — the decode tail amortizes the AR-loop speedup away.

### 4.2 The additive batched-decode seam (default per-slot delegate)

Add `fn decode_audio_batch(&mut self, bodies: &[(SlotId, &[Vec<i32>])]) -> Vec<Vec<f32>>` to `ArStepModel`
with a **default that delegates per-slot** (`arstep.rs:540` loop) — by-construction bit-identical, additive,
below the P-8 seam. A finished-slot **micro-cohort buffer** in the mux drain groups slots finishing in the same
tick. The mechanical fix per codec: drop the batch-collapse `reshape([-1])` (`mimi.rs:214`, `dac.rs:215`
`squeeze(1)`) → `squeeze(1)` to `[B,samples]` + per-row crop; `dacvae.rs:366` already keeps `[B,1,samples]`
proving the conv/RVQ stack is batch-generic. RVQ `from_codes` (`dac.rs:158`) is already `[B,n_cb,T]`-generic.

### 4.3 Codec batching is byte-identical by MATH (no Fork) — but the ragged pad/crop is GATED

§2.5: no cross-batch reduction → byte-identical f32 AND bf16. BUT a batched conv decode over length-ragged
finished slots needs right-pad-to-maxT + per-row crop, and symmetric DAC convs + the `DacResidualUnit`
center-crop can leak across a padded boundary. The per-row crop is an UNPROVEN assertion until gated. **Gate
(RED-first):** `decode_audio_batch_row_b_identical_to_decode_audio_b` on a deliberately ragged 2-row cohort
(row0 ≪ row1), f32 (dia2/DAC) AND bf16 (csm/Mimi), asserting each row's valid region == its solo decode.
Causal Mimi convs are safe; symmetric DAC convs are the open boundary question.

### 4.4 The vocoder-transient memory budget (a box-kill if naive)

A single B=1 S3Gen whole-body decode requests **~21.7 GiB in one transient** (KV-ACCEL-SERVELOOP). A naive
`[B]` batched decode multiplies that by B → instant unified-pool OOM. Worse: even two SERIAL per-slot decodes
overlapping in the arena (two finished slots in adjacent ticks) can exceed the 48 GiB cap. **Therefore:**

1. **Decode CONCURRENCY (even un-batched) is transient-budgeted, not just decode batching.** Add a decode
   semaphore / second `VramAccountant` leg = `floor(free_arena / per_decode_transient(body_len))` — typically
   1–2 on GB10 — that serializes/sheds concurrent heavy decodes BEFORE `cudaMalloc`.
2. **Decode-batch width is DECOUPLED from AR cohort width.** A B=64 AR cohort must NOT imply a B=64 vocoder
   decode. `decode_batch = free_pool / per_decode_transient`, a SEPARATE small budget.
3. **This must land BEFORE any device batcher serves >1 slot whose codec is a heavy whole-body decoder**
   (Phase 3/4 scope, not Phase 7) — the danger is two serial decodes overlapping, independent of batching.
4. **CUDA-only cells:** IndexTTS-2 BigVGAN SIGSEGVs on CPU → its batched-decode path is GB10-gated only (no
   portable CPU bit-twin); DAC/Mimi/HiFT keep a CPU-f32 bit-gate.

## 5. Hardware-adaptive batch-sizing policy (auto-pick B per HW + model + seq-len + free-mem)

B is chosen per tick by `pick_cohort_width(EpCaps, KvHeadLayout, free_budget, live_set) -> {max_slots, bucket}`,
wiring the dead machinery into the live path. The live cohort each tick is already `active_rows.len()`
(`serve.rs:778`) — only the CAP and the byte budget become hardware/footprint-derived.

### 5.1 The a-priori knee (a per-(arch,dtype) compute CEILING, NOT the whole answer)

`EpCaps::cuda_from_device(&DeviceCaps)` (`lib.rs:932`) → `EpCaps::batch_knee(caps, bytes_per_elem)`
(`lib.rs:1294`) = `⌈peak_flops/(peak_bw·bytes_per_elem)⌉` clamped per `BatchProfile`: GB10/sm_121 ⇒ **Wide
[64,512]** (`admission.rs:1310`); Ada/89 ⇒ Tens [8,64]; Static1/NPU ⇒ 1 (and AR is REJECTED on Static1 —
degrade to per-slot B=1, never strand the model). `BMax`/`BatchKnee::batch_knee` (`admission.rs:1470`)
min()-tightens with a measured sustained batch. **HONEST CAVEAT:** at GB10 the ridge is ~916–1831 for f32/bf16
→ both CLAMP to 512, ~32× the MEASURED tch knee (~B16 flat then compute-binds, PATHB §2). So the roofline knee
is a never-exceed ceiling, NOT a useful per-arch number on Wide HW — the BINDING constraint is **memory +
measured curve**. Schedule the BMax calibration job as a Phase-0 deliverable; without it the knee defaults to
the ceiling.

### 5.2 The byte budget (the real binding constraint) — and TWO sizing bugs to fix FIRST

Replace the flat 256 MiB/stream (`codec_ar_admission.rs:52`) with the REAL per-slot device footprint. **Two
verified bugs in the cited footprint must be fixed before wiring, or admission over-admits and box-kills:**

- **BUG-1 (layer-less):** `KvFootprint::total_bytes()` (`gqa.rs:227`) = `total_values × KV_ELEM_BYTES` where
  `total_values = kv_heads·head_dim·2·context` — **NO `n_layers` factor** (struct has only kv_heads/head_dim/
  context). The real per-slot ring is `n_layers ×` larger (qwen3 28L, misotts ~32L). Wiring `footprint().bytes()`
  directly under-counts by 24–60× → admits ~30× too many slots → the pre-alloc-once ring blows the 48 GiB arena
  → box-kill. **Fix:** `per_slot_ring_bytes = n_layers × footprint(MAX_SEQ).total_bytes()`.
- **BUG-2 (dtype-blind):** `KV_ELEM_BYTES = 2` is a const (`gqa.rs:43`) — fp16-only. The proven shippable device
  ring is the **fp32** cell (FINAL-STATUS §3.1: 240 MiB/slot F32, 4 bytes). f32-serve (Fork C) and the proven
  fp32 cell are silently HALVED → 2× over-admit → box-kill on the SHARED 121 GiB pool. **Fix:** replace the
  const with a `kv_elem_bytes` field seeded from the serving KV dtype (bf16/fp16=2, f32=4).

Then: `MAX_SLOTS = min(batch_knee, floor((budget − weights − decode_transient_headroom) / per_slot_ring_bytes))`.

### 5.3 The memory budget must be ONE reconciled number, seeded from free-mem (P0)

Two live budgets disagree and neither is free-mem: admission seeds `VramAccountant` from
`env_vram_cap() = 96 GiB` (`engine.rs`), while the ring/transients live in the **48 GiB** arena
(`ep.rs GB10_ARENA_LIMIT_BYTES`). Admission can green-light a cohort whose 96 GiB "fits" while the 48 GiB arena
OOMs. **Fix:** `budget = min(cuda_arena_limit_bytes(dev) [ep.rs:38], free_mem.unwrap_or(arena_cap·0.5))`;
`free_mem` is `Option` (`device.rs:188`, often `None`) → conservative fallback, **NEVER total_mem on a unified
device** (the documented box-kill). Seed BOTH the accountant and the ring sizing from it; the ring is
arena-resident so `MAX_SLOTS × per_slot_ring_bytes ≤ arena_cap`. Note the alloc-once ring is resident at
worst-case ALWAYS, so size `MAX_SLOTS` against `(arena − max_decode_transient − graph_pool − weights)` — the
proven 204-slot shrink (FINAL-STATUS §3.1) assumed NO co-resident heavy decoder.

### 5.4 Seq-len adaptivity + per-backend knee + bucketing

- `batch_knee` is seq-len-BLIND (only arch+dtype). Make the live set drive B: a cohort whose live lengths are
  all ≤256 admits far more slots than a flat MAX_SEQ=4096 cohort. Size the ring's effective MAX_SEQ per cohort
  (or bucket by seq-len) so short-context cohorts admit more.
- **CPU is NOT free / NOT Tens.** `EpCaps::cpu()` (`lib.rs:946`) hardcodes `BatchProfile::Tens` (floor 8) +
  fictional roofline — but PATHB measured CPU has NO flat region (soft 1.6×→12.85×@B64) and Fork-C byte-identity
  caps at B_inv=16. Give CPU its own `SoftCurve` profile (floor 1, measurement-driven via mandatory BMax), and
  clamp the byte-identical CPU cohort to B_inv.
- **CUDA-graph bucketing:** `static_shape_bucket` (`lib.rs:1314`) returns a SINGLE point (`min==opt==max==knee`)
  for elastic Wide/Tens — NOT the bucket SET CUDA-graph replay needs. Add a powers-of-two bucket-ladder
  generator (capture one graph per rung); `pad_batch` (`lib.rs:1181`) rounds a live cohort UP to the nearest
  rung. **Gate pad-row-invariance:** in bf16 a padded `[B+k]` GEMM reassociates vs unpadded — assert codes of a
  real cohort padded to a bucket == the same cohort unpadded, per graphable arch, BEFORE wiring; else run the
  padded GEMM under Fork A2 or f32. Budget `graph_pool_delta × bucket_count` at boot.
- The batch-width decision crosses the backend-free seam as PURE DATA (`SizingInputs{device_caps, kv_layout,
  n_layers, serving_kv_bytes_per_elem, max_seq, free_budget}`), computed at model-load and handed to
  `CodecArBatcher::new` — Phase 0 is a NEW sizing seam + per-model dtype/layer surfacing, NOT a const swap
  (`codec_ar_batcher.rs:47` is a module const with no device/model in scope).

## 6. Model-by-model rollout matrix (no model left at B=1)

Authoritative inventory (excluding infra modules: `lib/smoke/trt/device/sentencepiece/encdec`,
`indextts2_{frontend,backhalf,encoders}`). Every model has a disposition; a model absent from this matrix is a
coverage bug. **StepClass:** `AR`=lockstep ring; `SB`=step-bucket; `FIG`=fused-inner-graph AR; `DUP`=S2S
duplex; `1SHOT`=equal-shape graph cohort; `PER`=stays per-slot (with reason). **Tier:** A1/A2/B/C per §2.2.

### 6.1 tch codec-AR TTS (lockstep ring)

| Model | dtype | StepClass | Tier | Ring/notes | Gate |
|---|---|---|---|---|---|
| qwen3_tts | bf16 | AR | A1 (B if ratified) | **PILOT** — has `talker_logits_batched` seed; single/dual-cb | force-solo all-rows ragged |
| dia2 | bf16 | AR | **B** (TF32-ON→A2 foreclosed) | CFG B=2 inner, slots outer ⇒ 2·n; `append_full_masked`; depformer PARTIAL | batched-vs-batched [2·n→2] |
| dia | bf16 | AR | B | CFG; `append_contiguous_masked` (MATH SDPA); not graphable | batched-vs-batched |
| csm | bf16 | AR | A1 | backbone ring + depth PER-slot (31 steps, B27 projector); `append_contiguous` | force-solo; depth separate |
| misotts | bf16 | AR | A1 | csm-twin (Llama-8B + 300M depth PER-slot) | force-solo; depth separate |
| higgs | **fp16** | AR | A1 | 8-cb delay, DAC; f32 head over fp16 body | **fp16** force-solo (own row) |
| higgs_v2 | **fp16** | AR | A1 | DualFFN per-modality routing must batch | **fp16** force-solo |
| s2_pro | bf16 | AR | A1 | dual-AR (36L slow + 4L fast), both ring | force-solo |
| neutts | bf16 | AR (+ONNX codec) | A1 | tch backbone ring; ONNX codec decode separate (§4) | force-solo |
| zonos2 | bf16 | AR | A1 | **MoE** mask-all-experts only; EDA router per-row | dedicated MoE oracle |

### 6.2 tch hybrid AR + flow/diffusion (two batchers)

| Model | dtype | AR part | Head part | Gate |
|---|---|---|---|---|
| cosyvoice3 | bf16 LM / f32 flow | AR ring (A1) | SB CFM (maxΔ=0 waveform) | LM force-solo + flow maxΔ=0 |
| voxtral_tts | bf16 | AR cb0 ring (A1) | SB rectified-flow acoustic | force-solo + flow maxΔ=0 |
| dots | bf16 | AR Qwen2 ring (A1) | SB DiT flow (graphable) | force-solo + flow maxΔ=0 |
| indextts2 (GPT-2) | **f32** | AR ring (**C, free**) | SB DiT-CFM back-half (BigVGAN CUDA-only) | C codes + flow maxΔ=0 |
| vibevoice | bf16 | AR Qwen2 ring (A1) | SB DDPM acoustic+semantic | force-solo + diffusion RNG-order |
| vibevoice_realtime | bf16 | AR dual-LM ring (A1) | SB DDPM | force-solo + RNG-order |
| voxcpm2 (ORT) | — | continuous-latent AR | **FIG** fused decode_step | maxΔ=0 latent patch (not codes) |

### 6.3 tch / ORT pure non-AR diffusion / flow (step-bucket only — f32, EASY tier)

omnivoice (SB, Fork A1 over slot axis + RNG-order), viitorvoice (SB, ORT backbone `[B]`), irodori (SB RF-DiT),
pocket_tts (SB MAR), rsb (fixed-shape enhance cohort). All f32 → no flip; only serve-wiring. supertonic (ORT,
already wired `synthesize_batch` 2.33×@B8, maxΔ=0) is the 1SHOT precedent.

### 6.4 STT — a SEPARATE two-stage track (encoder cohort + AR decoder ring)

These impl ONE-SHOT `transcribe`, ZERO `as_stepped` — each needs a full re-plumb (encoder-prefill +
decoder-as-ArStepModel), NOT a thin config. The encoder batches via the proven equal-context whisper cohort
(`whisper.rs transcribe_batch`, 1.19×); the AR decoder rides the ring.

| Model | dtype | Disposition |
|---|---|---|
| voxtral | **fp16** | AR Mistral decoder ring (own fp16 oracle) |
| cohere | **fp16** | AED decoder + cross-attn ring |
| ark | **fp16** | Qwen2 decoder ring |
| granite | bf16 | Granite decoder ring (A1) |
| canary_qwen | bf16 | Qwen3 decoder + per-slot **LoRA** (per-row adapter, proven byte-identical) |
| higgs_stt | bf16 | Qwen3 decoder ring |
| vibevoice_asr | bf16 | Qwen2.5-7B decoder ring |
| whisper, canary, qwen3_asr, funasr_nano (ORT) | — | AED/LLM-decoder: encoder cohort + decoder ring |
| parakeet, nemo_ctc, sensevoice, moonshine, medasr (ORT) | — | **PER / equal-context cohort** (CTC/TDT, no AR decode) |
| nemotron (ORT) | — | **StreamingChunkCache** (cache-aware RNNT — a 4th shape; declare PER or chunk-cohort) |

### 6.5 S2S + one-shot TTS

| Model | StepClass | Disposition |
|---|---|---|
| hibiki | DUP | new `as_duplex` + `DuplexStepModel` impl (f32, Fork C); S2S analog of codec_ar_batcher (§3.3) |
| lfm2_audio | PER/DUP | hybrid conv+attn cache — own primitive or PER, across S2sModel/TtsModel/SttModel |
| duplex_codec_ar | — | TEST-ONLY blueprint (the proven DuplexStepModel) — not a registry model |
| kokoro, melo, vieneu (ORT) | 1SHOT | equal-shape `synthesize_batch` cohort (`model.rs:398` seam) or PER with reason |
| moss (ORT) | AR | codec-AR GPT-2, NOT yet wired — clean ORT `as_stepped` candidate (chatterbox pattern) |
| chatterbox (ORT) | AR | **already wired** (Path-A host-KV ~1.8×@B16; device-KV 2.34×@B24 decoder-test, serve-loop pending §1.4) |

## 7. Optimal perf (realize device-resident scaling + CUDA-graph where tch allows)

### 7.1 What the win actually is (honest, per fork)

- **Fork A1 (the byte-identical-vs-solo default):** the win is **device-resident-KV re-stream elimination** (no
  host bounce per stride — the same mechanism Path-A proved at 2.34×@B24, FINAL-STATUS §3.1) + launch
  amortization of the cheap non-reducing glue. The reducing attention stays per-row. **This is a HYPOTHESIS to
  re-measure on the real growing-KV decode loop** (the 30.2× is a prefill upper bound, PATHB §2; the decode
  re-incurs O(B·max_past) attention). Phase-3 go/no-go threshold: Fork A1 must beat Path-A's ~1.8× host-KV cap
  on the real decode loop, or it is a correctness move with no perf upside at that B.
- **Fork B / A2 (the 30× lever):** ONE fused `[B]` backbone GEMM — the throughput the probe measured — admissible
  only batched-vs-batched (B) or f32-accumulate (A2). This is where the headline number lives, and it is NOT
  byte-identical-vs-solo on a bf16 model.

### 7.2 The per-stride READ-BACK is a hidden cost device-residency alone does NOT remove

Device-residency removes the host KV re-stream, but the tch read-backs still materialize per stride:
`append_contiguous` (`kv_cache.rs:274 narrow.contiguous`) and `append_full_masked` (`:318 shallow_clone` of the
WHOLE `[batch,kvh,max_seq,d]`) materialize a fresh tensor each stride — the bf16 fleet (csm/cosyvoice3=contiguous,
dia2=full_masked) is exactly the re-streaming set. ONLY `append_view` (`:267`, voxtral/cohere/ark) is true
zero-copy. So the device-resident win is realized fully only on `append_view` archs; the contiguous/full_masked
majority must EITHER prefer a view where the kernel tolerates it, OR run under CUDA-graph mode (capture the
materialization once, replay) to avoid paying O(B·max_seq) per stride. Declare each arch's read-back class and
budget the per-stride read-back-bytes in the knee/cost model.

### 7.3 CUDA-graph must be CO-DESIGNED with the batcher (not Phase-9 polish)

The q_len=1 decode is launch-bound (`cuda_graph.rs:14` captures fixed addresses + fixed shape). The graph lever
and Fork A1 (N B=1 dispatches) FIGHT — Fork A1 emits B serial launches, defeating single-graph capture; dynamic
B defeats fixed-shape capture. **Resolution:** the graph path requires the **fused `[B]` step (Fork B/A2)**, so
graphability reinforces the tier choice. Per bucketed cohort width B, capture ONE graph of the fused `[B]` step
against the static ring (the `kv_cache.rs:82 GraphState` device write-index already exists for exactly this —
the `index_copy_`/`ge` recompute from device scalars at replay, `:72`). Bucket via §5.4 ladder. Graphable set:
dia2/csm/omnivoice/dots (per the device-KV-fanout memory). Non-graphable (dia/higgs growing-narrow) run eager.
**Gate `graph_replays_across_3_seqlens_k_codes_identical` BEFORE wiring** (the tch device-scalar precondition).

### 7.4 Depformer + decode tail are separate latency levers

For csm/dia2/misotts the depformer (31 serial depth launches/outer step) is the latency floor; the backbone
batch lever moves only ~3% (PATHB G6). Realized cohort speedup is depth-decoder-bound; do NOT report
backbone-ring wiring as full coverage. The codec/vocoder decode tail (per-slot serialized, §4) must be
PIPELINED/overlapped with the next AR tick (run decode on a separate stream/worker, bounded by the transient
budget §4.4) so the AR speedup is not amortized away — quantify END-TO-END speedup (including decode tail), not
AR-loop-only, in every perf gate.

### 7.5 Effective arithmetic intensity, not nominal dtype

The knee (`batch_knee`) is fed `bytes_per_elem` of the serving dtype (bf16=2), but Fork A2 runs the reducing
GEMMs in f32 (4) and the per-stride materialize doubles K/V traffic. At Tens-profile HW (Ada knee=64, binding)
this mis-places the ridge. Feed the EFFECTIVE intensity of the chosen (tier, read-back) into the knee, or add
an intensity-adjustment term. (At GB10/Wide this is masked by the 512 ceiling clamp, but the policy must be
portable.)

## 8. Extreme-TDD failing-first gates + no-regression strategy

Every gate is RED-first (authored to fail before the code exists). CUDA gates run process-isolated via
`ci/heavy_live_tests.sh` `--test-threads=1`, ONE model set at a time (every live tch/ORT gate `mem::forget`s the
model — GB10 CUDA Drop SIGABRTs — so multiple in-process accumulate leaked unified memory). Every CUDA gate
keeps a deterministic non-CUDA twin on `cargo test`.

### 8.1 The oracle is the LONG POLE (not the ring)

The standing `assert_concurrent_eq_serial` (`harness.rs:337`) drives `step_batch` through `Driver::tick` and
compares against `run_serial`/`run_serial_one` (`harness.rs:183,116`) — i.e. batched-cohort vs B=1-SOLO. **This
gate goes RED the instant a tch bf16 `step_batch` is wired** (the B23 flip). The only existing tch batched-vs-
solo probe (`qwen3tts_batch_scaling.rs`) is `#[ignore]`, asserts nothing (eprintln), and checks only 3 rows
(`:109 vec![0, b/2, b-1]`) on a single PREFILL — it CANNOT catch a flip on an un-probed row. So there is ZERO
real tch force-solo CODES oracle today. **First deliverable per arch:** a reusable
`force_solo_oracle::<M: ArStepModel>` — all-rows integer-CODE compare on a ragged staggered mid-finish cohort,
full AR decode loop, stress corpus with narrow-argmax-gap rows — RED-first. A model is NOT batchable until its
oracle is GREEN on real weights in the serving precision. Size it: ~23 archs × {CUDA-bf16/fp16 + CUDA-f32 +
CPU-f32} × ~80s = a multi-hour serialized GB10 surface → tiered (smoke subset per-PR, full matrix nightly).

### 8.2 Phase-by-phase gates (RED-first)

**Phase 0 (deterministic, no GPU):** `sizing_shrinks_max_slots_cleanly_on_over_budget`;
`per_slot_ring_bytes_includes_n_layers` (BUG-1: qwen3 28L == hand-derived `28·2·kvh·MAX_SEQ·hd·sizeof`);
`footprint_fp32_is_2x_bf16` (BUG-2: dtype-driven elem_bytes); `budget_is_min_arena_and_freemem_never_total`;
`cpu_knee_floor_is_1_not_8`; `static1_ar_degrades_to_b1_not_refused`; `tf32_class_coresidency_typed_reject`.

**Phase 1 (ragged ring — CPU-f32 deterministic FIRST, then CUDA):** `ragged_ring_codes_identical_to_per_slot_cpu_f32`
(staggered mid-finish, LAYOUT correctness — CPU-f32 is code-invariant ≤B16); per-read-back twins for view/
contiguous/contiguous_masked/full_masked under per-row seqlens_k; `recycle_zero_is_clean_bit_identical`. **Note:
the CPU-f32 ring gate proves LAYOUT, NOT the bf16 reduction-order floor** — left-align fixes layout, not the
GEMM reduction; the CUDA-bf16 identity is a SEPARATE oracle (§8.1).

**Phase 2 (bf16 floor):** `qwen3_tch_force_solo_codes_identical_ragged_f32` (Fork A1, must GREEN before any B>1
serve); `qwen3_tch_fused_batch_bf16_flips_codes_is_caught` (RED-witness — the fused bf16 path flips, the gate
catches it, fail-closed twin of `cfg_batch_gate_catches_batch_stacked_flip`); `qwen3_tch_fused_batch_f32accum_codes_identical`
(Fork A2 unblock, all-rows stress corpus); `tf32_scope_restores_global_under_interleaved_load_unload`.

**Phase 3 (pilot serve):** `qwen3_step_batch_serve_codes_identical_to_solo`; `serve_loop_graceful_shed_n_2_8_16_32`
(the §1.4 hardening — byte-identical + typed shed, NOT thread crash); re-measure B1..B64 on the real decode loop.

**Phase 4+ (per-arch fan-out):** each arch's `<arch>_batched_vs_force_solo_codes_oracle`; dia2
`cfg_2n_to_2_batched_vs_batched_identical`; zonos2 `moe_mask_all_experts_force_solo`; depformer
`depth_decoder_stays_per_slot_codes_identical`.

**Phase 6 (step-bucket/diffusion):** `omnivoice_step_bucket_per_slot_dispatch_rng_order_identical`;
`flow_cfm_cohort_maxdelta_zero` (supertonic precedent); `voxcpm2_fused_inner_graph_latent_maxdelta_zero`.

**Phase 7 (codec decode):** `decode_audio_batch_row_b_identical_to_decode_audio_b` (ragged pad/crop, f32 AND
bf16); `decode_concurrency_transient_budget_refuses_not_ooms` (the §4.4 box-kill guard).

**Phase 8 (STT/S2S):** `<stt>_decoder_ring_transcript_identical`; `hibiki_frozen_slot_continuation_identical`
(exec_mask no-op law); `hibiki_duplex_token_for_token_vs_codecar_duplex`.

**CUDA-graph:** `graph_replays_across_3_seqlens_k_codes_identical`; `pad_row_invariance_codes_identical_per_bucket`.

### 8.3 No-regression strategy

- **The B=1 path stays default until the concurrency gate is green.** Gate every tch `as_stepped()->Some` behind
  a regime flag; the model loads but routes to one-shot `synthesize` until `serve_loop_graceful_shed` passes.
  No-regression gate: existing chatterbox one-shot + codec-ar tests stay GREEN with the tch model loaded but
  `as_stepped` still `None` (additive + reversible).
- **All existing byte-identical gates stay GREEN:** dia2 544/544 + 608/608, csm, voxtral, granite, cohere,
  `host_vs_device_kv_oracle`, `live_ragged_batched_forward_bit_identical_and_scales` (production host-KV),
  supertonic flow maxΔ=0, whisper transcript. The new seam is additive (default per-slot delegate) below P-8;
  runtime/serve/driver are byte-for-byte unchanged.
- **Never ratchet a doc perf constant down**; promote constants ONLY post-measurement (the FINAL-STATUS §11.10
  discipline).

## 9. Dependency-ordered phasing (executable roadmap)

```
Phase -1 RATIFY the fork (§0.3) + observability        [decision gate — blocks everything]
Phase 0  Sizing seam + footprint fixes + TF32 class    [no behavior change]
Phase 1  Ragged SlotId device ring (CPU-f32 first)     [core new primitive]
Phase 2  bf16-floor kit + real tch force-solo oracle   [accuracy keystone]
Phase 3  PILOT qwen3-tts end-to-end + serve hardening  [risk-retire; go/no-go]
   └─ Phase 4  Fan out lockstep-AR fleet
   └─ Phase 5  Hybrid AR+flow + pure step-bucket/diffusion
   └─ Phase 6  Codec/vocoder batched decode + transient budget
   └─ Phase 7  STT two-stage + S2S duplex (new as_duplex)
   └─ Phase 8  CUDA-graph bucketing (graphable archs)
Phase 9  Promote constants post-measurement; full-fleet regression; per-HW knee tuning
```

**Phase -1 — RATIFY + observe (do-now, cheap de-risk).** Get owner sign-off on §0.3: dynamic-B production
contract = "codes deterministic per cohort + byte-identical-vs-solo where Fork A1 applies; 30× is Fork-B/A2 and
batched-vs-batched". Pull the live `COHORT_WIDTH_METRIC` histogram (`serve.rs:552`) to confirm the cohort
right-tail is fat enough to justify the program and to size the CUDA-graph buckets; prioritize fat-right-tail
archs.

**Phase 0 — sizing seam (accuracy-neutral).** Fix BUG-1 (n_layers) + BUG-2 (dtype elem_bytes) in
`gqa.rs`/`KvFootprint`; add `SizingInputs` pure-data carrier; replace `const MAX_SLOTS=24`
(`codec_ar_batcher.rs:47`) with `pick_cohort_width(SizingInputs)`; replace flat 256 MiB
(`codec_ar_admission.rs:52`) with `n_layers × footprint × elem_bytes`; reconcile budget to
`min(arena_cap, free_mem)` from probed `DeviceCaps` (`device.rs:124`); add `Tf32Class` co-residency reject +
`Tf32Scope` RAII. Land the Phase-0 deterministic gates. (Footprint swap + decode-transient budget land
TOGETHER — §4.4 — so admission cannot loosen ahead of the transient fix.)

**Phase 1 — ragged ring.** Build `RaggedSlotRing` (per-row seqlens_k + per-row left-justified mask, alloc-once
at MAX_SLOTS, recycle-zero); re-derive each of the 4 read-backs under per-row seqlens_k; the per-stride
`[B,1,…]` decode primitive on `Backbone::forward`. Gate CPU-f32 layout identity FIRST (deterministic), then
CUDA.

**Phase 2 — bf16-floor kit + oracle.** Fork A1 two-pass discipline generalized; Fork A2 f32-accumulate SDPA
(new code on QK^T/att·V); the reusable `force_solo_oracle`; promote `qwen3tts_batch_scaling` into an ASSERTING
all-rows code-flip gate; TF32 scope/co-residency proven. RED-first on CPU-f32 then CUDA.

**Phase 3 — PILOT qwen3-tts.** Impl `ArStepModel` over qwen3 tch state; override `as_stepped` (behind regime
flag); per-stride batched decode against the Phase-1 ring; apply Fork A1. HARDEN the multiplexed serve loop
(`serve_codec_ar_multiplexed_bounded` + single mux-thread) for graceful shed at n=2/8/16/32 BEFORE flipping the
flag. Re-measure B1..B64 on the real decode loop; **GO/NO-GO:** Fork A1 must beat ~1.8×, oracle GREEN, shed
graceful. This retires the ragged-decode-shape + concurrency risks before paying them 23×.

**Phase 4 — lockstep-AR fan-out.** dia/dia2/csm/higgs/higgs_v2/s2_pro/neutts/misotts/zonos2 + ORT MOSS, each
with Tier per (dtype, TF32) + force-solo oracle. dia2/csm/misotts marked PARTIAL (depformer per-slot, separate
lever). zonos2 MoE mask-all-experts. fp16 fleet (higgs/higgs_v2) own oracles.

**Phase 5 — hybrid + pure diffusion/flow.** Wire `StepBuckets` to omnivoice/viitorvoice/irodori/pocket_tts/rsb
(f32, easy) + the flow/diffusion heads of cosyvoice3/voxtral_tts/dots/indextts2/vibevoice/vibevoice_realtime;
voxcpm2 FIG mode. RNG-draw-order + maxΔ=0 gates.

**Phase 6 — codec decode.** `decode_audio_batch` (default per-slot delegate) + finished-slot micro-cohort +
mechanical `reshape→squeeze+crop`; decode-transient budget + concurrency semaphore. Δ==0 ragged pad/crop, f32 +
bf16. S3Gen stays per-slot.

**Phase 7 — STT + S2S.** STT decoder rings (own two-stage track; encoder equal-context cohort + decoder ring;
canary_qwen per-slot LoRA); CTC/TDT stay equal-context/PER; nemotron streaming-chunk. S2S: add
`LoadedModel::as_duplex`, impl `DuplexStepModel` for hibiki, S2S analog of codec_ar_batcher; lfm2 hybrid-cache.

**Phase 8 — CUDA-graph bucketing.** For graphable dia2/csm/omnivoice/dots: 3-seqlens_k replay gate +
pad-row-invariance gate FIRST, then capture per bucket; budget `graph_pool × bucket_count`.

**Phase 9 — promote + regress.** Per-HW knee tuning (CPU later/softer, B_inv clamp), full-fleet regression,
constants promoted only post-measurement.

## 10. Risks, open questions, and what must be ratified before proceeding

**MUST RATIFY (Phase -1, blocks everything):** the literal "max|Δ|=0 logits vs B=1 solo" bar is mathematically
unachievable for a fused `[B]` reducing GEMM at H=1024 (`cfg_batch.rs:13`, fp32 9.5e-7≠0). The bar IS
codes-identical (the standing integer `assert_identical`). And no single tier gives BOTH 30× AND
byte-identical-vs-solo on a bf16 model: byte-identical-vs-solo = Fork A1 ≈ device-residency win (~2×-class);
30× = Fork B/A2 = batched-vs-batched. The owner must accept this fork in writing.

**P0 risks:**
1. **Fork A1 serving perf is UNMEASURED.** The 30.2× is prefill-only; the growing-KV decode re-incurs
   O(B·max_past). Fork A1 (per-row reducing dispatch + glue fusion) may land below Path-A's 1.8×, possibly
   negative after ring bookkeeping. Phase-3 go/no-go is the only arbiter — never promise 30× for a
   byte-identical-vs-solo path.
2. **Serve-loop concurrency is BROKEN for any device batcher** (n≥2 hang/empty/crash, FINAL-STATUS §5). Hard
   prerequisite; if the root cause is deeper than the single mux thread, the whole serving win stalls.
3. **Sizing box-kill** if BUG-1 (n_layers) / BUG-2 (dtype) / the 96-vs-48 GiB budget / `free_mem=None` fallback
   are not all fixed before any tch ring flips — the GB10 unified pool is a hard box-kill, twice-observed.
4. **The cfg_batch "proof" is a docstring + test-double** (§2.4) — the real tch force-solo oracle on real bf16
   weights is the actual long pole and does not exist yet.
5. **The vocoder transient (~21.7 GiB × concurrency)** is unbudgeted in admission today; two serial decodes
   overlapping box-kill independent of batching (§4.4).

**P1 risks:** Fork A2 f32-accumulate is new, unproven, ~2× FLOPs on the binding op, and reaches ~5e-6 not 0
(narrow-gap re-flip). TF32 is a process-wide global (silent flip on co-residence/hot-swap). CUDA-graph × dynamic
B forces bucketing (pad-row-invariance must be gated; right-pad waste ~27%). Depformer-bound archs
(csm/dia2/misotts) see far less than headline. fp16 fleet has zero in-tree flip measurement. The ragged ring is
`4×` (four read-back conventions), not one primitive.

**P2 / open questions:** the per-stride read-back materialize erodes the device-residency win on
contiguous/full_masked archs unless CUDA-graphed (§7.2). lfm2 hybrid conv+attn cache has no ring host. voxcpm2
FIG is a third mode the two-class taxonomy did not anticipate. CPU `B_inv` for the AR decode loop is unmeasured
(assume ≤16). BigVGAN SIGSEGVs on CPU (no portable bit-twin). Heavy oracles (~80s × 23 archs × 3 cells) make
full-fleet RED-first a multi-hour serialized memory-pressured CI surface.

**Confidence:** HIGH that the seam/sizing/oracle architecture is correct and grounded; MEDIUM that the realized
Fork-A1 serving speedup beats Path-A's 1.8× (unmeasured — Phase-3 decides); the 30× belongs to Fork B and is
NOT the byte-identical-vs-solo path.
