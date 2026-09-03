# B46 — CUDA-graph perf fan-out across the remaining launch-bound tch models (dots / dia / omnivoice / higgs)

**Goal.** Generalize the proven bit-faithful CUDA-graph seam (B43 `nn::cuda_graph` + `Backbone::with_cuda_graph`;
B44 the dia2 per-stage depformer graph; B45 the csm depth-decoder graph) across the remaining launch-bound tch
models — **dots-tts, dia, omnivoice, higgs** — applying the seam wherever it yields a **byte-faithful** win, OFF
by default, gated `WAAV_<MODEL>_CUDA_GRAPH=1`. Per the LAW: each model with its graph ON MUST stay byte-identical
to its existing gate, with a per-model AB gate (captured==eager) + a perf A/B.

**Verdict.** **2 of 4 shipped, both BIT-FAITHFUL; 2 honestly NOT graphable.**

| model | priority | graphable? | seam | RTF before→after | <1? | byte-identity proof |
|---|---|---|---|---|---|---|
| **omnivoice** | 3 | **YES** | **NEW bidirectional-graph nn/ regime** | **1.398 → 1.169** (×1.20) acc.; 0.339 → 0.314 (×1.08) long | long: yes; acc.: no (1.17) | codes **0/288** + full-synth wav **maxΔ=0** |
| **dots** | 1 | **YES** | DiT per-patch graph (**dots.rs only**) | AR+FM **×1.04** (+3.9%); 13705 → 13175 ms | no | latents **10240/10240 Δ=0** |
| **dia** | 2 | **NO** (csm-backbone, no sub-decoder) | — | baseline CUDA-bf16 **2.77** (2601 frames, launch-bound) | — | unchanged (not touched) |
| **higgs** | 4 | **NO** (csm-backbone + RopeApply::Start + CUDA≠byte-bar) | — | baseline CUDA-f16 **1.406** | — | unchanged (not touched) |

`cargo test -p waav-infer-backend-torch --lib` = **145/145** (+1 new rope unit). `cargo clippy --all-targets -D
warnings` = **clean**. Shared-`nn/` regression (omnivoice needed an `nn/` extension): **dia2 608/608** (graph ON),
**csm 4000/4000** (graph ON), **voxtral 100% char-identical** — all re-verified byte-identical. No perf claim
breaks byte-identity. `free -g`: 34–37 GB free / 103–106 GB available throughout — no OOM (one model at a time).

---

## 1. The per-model graphability assessment (the rubric, applied)

The captured graph needs a **FIXED shape**. The dominant launch-cost block per model + the matching pattern:

| model | arch | dominant launch block | RopeApply / CacheRead | graphable verdict |
|---|---|---|---|---|
| **omnivoice** | bidirectional masked-diffusion-LM (28-layer Qwen3, **NO cache**) | the per-diffusion-step backbone forward (2·32 = 64 full forwards/synth) — FIXED seq across ALL steps | StartExact / none | **(c) DiT/diffusion step → graphable**: ONE graph for the WHOLE generation |
| **dots** | flow-matching: LLM(28) + PatchEncoder(24) + **DiT(18)** | the DiT velocity predictor (10 Euler × 18 blocks × 2 CFG = **360 block-fwds/patch** vs ~52 of LLM+PE) | LLM=Start; DiT=internal | **(c) graphable per patch** (the FM prefix GROWS each patch → re-capture keyed by `total`) |
| **dia** | encoder→**18-layer AR cross-attn decoder**, batch-2 CFG | the single decoder backbone (no depth decoder) | Positions / **ContiguousMasked** | **(b) NOT graphable** — growing narrow + growing mask, MATH-pinned bf16; NO sub-decoder escape |
| **higgs** | single **36-layer Qwen3-4B AR** | the single backbone (no depth decoder) | **Start** / **ViewContiguous** | **(b) NOT graphable** — growing contiguous, Start unsupported, NO sub-decoder; CUDA path not the byte-bar |

The decisive split: omnivoice + dots are **(c) fixed-shape-per-step** (diffusion/flow) → graphable. dia + higgs are
**(b) growing-contiguous AR backbones with NO fixed-position sub-decoder** to graph instead — exactly the
csm-backbone verdict (B45 §3), except csm had a depth decoder (the 86% lever) and these do not.

---

## 2. omnivoice — SHIPPED, ×1.20, a GENUINE NEW bidirectional-graph nn/ regime

### 2.1 What is graphed (and why it is the lever)

omnivoice synthesizes via a 32-step masked-diffusion-LM: each step runs the **28-layer Qwen3 backbone
BIDIRECTIONALLY, cache-free, over the full id grid `[1, s, HIDDEN]`** (CFG → cond + uncond = **2 full forwards/
step**, 64/synth for T=36), then an audio head + a CFG/argmax reveal. The backbone forward is the entire
transformer launch cost AND — critically — **the grid length `s` is CONSTANT across all 32 steps** (tokens are
revealed *in place*, never appended). So **one captured graph replays for ALL 64 forwards** — the cleanest (c)
case (no per-step re-capture, unlike dots).

### 2.2 The genuine `nn/` extension (the bidirectional-graph regime)

omnivoice did **not** reuse the AR seam unchanged — the existing `Backbone::forward_graph` is the q==1
FullMasked AR decode path. omnivoice is a **bidirectional, cache-FREE, full-sequence** forward (`RopeApply::
StartExact`). Its only host→device op inside the forward is the StartExact RoPE recompute (`apply_positions_exact`
does `Tensor::from_slice(positions).to(dev)`, which CUDA-graph capture **forbids**). So the genuine extension
**hoists the RoPE table out of the captured body** (the positions `[0,s)` are constant → cos/sin are computed
ONCE, outside capture, and reused every replay):

- `nn::Rope::cos_sin_exact(positions, d, dt) -> (cos, sin)` — the seq-exact doubled `[1,1,s,d]` tables, byte-
  identical to what `apply_positions_exact` builds internally (the host→device GEMM runs ONCE).
- `nn::Rope::apply_doubled(x, cos, sin)` — the public, pure-tensor (capturable) `rotate_half_apply_doubled`.
- `Attention::forward_full_with_cossin` / `TransformerLayer::forward_bidirectional_with_cossin` /
  `Backbone::forward_bidirectional_with_cossin` — `forward_full`/`forward_bidirectional` but consuming the
  precomputed cos/sin (no in-line host→device recompute → the WHOLE 28-layer stack body is capturable).
- Rope unit test `cos_sin_exact_apply_doubled_matches_positions_exact` — Δ=0 vs `apply_positions_exact` (CPU,
  seqs 1/5/36/47).

The omnivoice glue (`logits_graph` in `omnivoice.rs`) captures `embed → forward_bidirectional_with_cossin →
audio_head → slice` (the host `embed` index-gather of the revealed grid stays OUTSIDE; its result is written into
the static `ie_in`), with the B43 RNG-preserve + discard-capture-output + replay-once traps reused.

### 2.3 THE SCAR — cond/uncond aliasing (the one-hour trap; the load-bearing fix)

The first graph-ON gate **diverged 269/288** while every *isolated* probe passed (single-forward capture==eager
0/288; capture-step output 0/288; replay-with-NEW-input 0/288). Root cause: `OmniLogits::eval` holds **BOTH** the
cond AND the uncond logits at once, and both came from `logits_graph` returning the SAME static `logits_out`
(`shallow_clone`) — so the uncond replay **overwrote** the cond result, aliasing `cl == ul`, and the CFG combine
`log_softmax(clp + GS·(clp − ulp))` collapsed to `ulp` → wrong reveal → cascade. (The masked-diffusion reveal is
argmax-driven with a dead-NaN Gumbel, so the divergence is purely the aliased logits, not RNG.) **Fix: return
`logits_out.copy()` (DEEP copy)** at every replay return point. dia2/csm can `shallow_clone` ONLY because they
consume each stage's output BEFORE the next graph overwrites; omnivoice's CFG pair is held simultaneously. This
scar is now documented in the code + the perf-fanout memory: **any CFG / multi-branch graph MUST deep-copy its
output.** (dots applied this preemptively.)

### 2.4 Bit-faithfulness + perf (graph ON)

| gate | graph OFF | **graph ON** |
|---|---|---|
| `cuda_torch_omnivoice` gate5 masked-diffusion codes (CUDA-f32) | 0/288 | **0/288** ✓ |
| `cuda_torch_omnivoice` gate6 full-synthesis wav maxΔ | 0e0 | **0e0** ✓ |
| `cuda_torch_omnivoice_graph_ab` single-forward argmax | — | **0/288** ✓ |
| `cuda_torch_omnivoice_graph_ab` replay-with-NEW-input argmax | — | **0/288** ✓ |
| `cuda_torch_omnivoice_graph_ab` FULL-generation codes (capture vs graph-eager) | — | **0/288** ✓ |
| acceptance utterance (T=36) RTF | 1.398 | **1.169** (×1.20, −16.4%) |
| long utterance (~12 s audio) RTF | 0.339 | **0.314** (×1.08, −7.2%) |

The win is **larger on the short utterance** (more launch-bound) than the long one (the bigger SDPA/GEMM over the
longer grid is more compute-bound — the honest pattern). The long realistic synthesis is well under RTF 1; the
T=36 acceptance utterance drops 1.398→1.169 (still >1 because of the un-graphable eager work: the masked-diffusion
`step` log_softmax/argmax/topk over `[1,8,36,1025]`, the host embed gather, the CPU DAC codec).

---

## 3. dots — SHIPPED, ×1.04, the DiT flow-matching graph (per-patch, dots.rs only)

### 3.1 What is graphed (the DiT velocity predictor)

dots emits **continuous latent patches** via a 10-step Euler ODE whose velocity is the **18-block DiT**, run as a
**batch-2 CFG** forward (cond + uncond) — **360 DiT-block-forwards per patch** (10 × 18 × 2), vs the ~52
LLM+PatchEncoder layer-forwards/patch. The DiT is the dominant launch cost (the dia2-depformer analogue). Within
one patch the DiT shape `[2, total, 1024]` is **FIXED** (only the latent-slot region of the packed sequence + the
timestep `t` change across the 10 Euler steps) → graphable; the only varying inputs `x`/`t` are written into static
`x_in`/`t_in`, the constant `pos_ids`/`attn_bias`/`g_cond` are graph-external constants.

### 3.2 Per-patch re-capture (the honest amortization cost)

Across patches the FM prefix `fm_len` **GROWS ~5/patch** (4 history + 1 hidden), so `total = fm_len + 4` grows and
a single graph cannot span patches. The graph is **re-captured whenever `total` changes** (the `captured_total`
guard). For the perf utterance ~224 patches each re-capture (2 warmup + 1 capture) → only ~7 of 10 Euler steps
replay per patch, and the warmup/capture overhead recurs each patch → the win is real but **modest (×1.04)**. This
is the inherent cost of a growing-shape DiT (vs omnivoice's once-per-generation fixed shape). The DiT also becomes
partly compute-bound as `total` grows (the SDPA over the full sequence).

### 3.3 Bit-faithfulness + perf (graph ON)

| gate | graph OFF | **graph ON** |
|---|---|---|
| `cuda_torch_dots` Gate-1 latent byte-identity (CUDA-bf16, the AR+FM seam) | 10240/10240 Δ=0 | **10240/10240 Δ=0** ✓ |
| `cuda_torch_dots` Gate-2 audio envelope corr | 1.0000 | **1.0000** ✓ |
| `cuda_torch_dots_graph_ab` capture vs graph-eager (full AR+FM) | — | **0/10240, max\|Δ\|=0** ✓ |
| AR+FM latent-gen (224-patch perf utterance) | 13705 ms | **13175 ms** (×1.04, +3.9%) |

No shared-`nn/` change — dots reuses `nn::CudaGraph` directly; ALL changes are in `dots.rs` (the DiT graph is
dots-specific glue, the B44/B45 "not a shared-lib change" call). RTF < 1 is **not** reached (the full synth is
dominated by the f32 BigVGAN vocoder, which the graph does not touch; the AR+FM seam is ~13 s for ~28 k latents).

---

## 4. dia — NOT GRAPHABLE (honest skip): the csm-backbone problem, no sub-decoder

dia's decoder is a **single 18-layer GQA cross-attention AR backbone** (batch-2 CFG, q==1 decode), `RopeApply::
Positions` + `CacheRead::ContiguousMasked`. Three facts make it un-graphable byte-identically with the existing
seam:

1. **Growing shape.** `append_contiguous_masked` returns `narrow(2, 0, cur).contiguous()` + an all-zero mask
   `[1,1,1,cur]` — **both grow every step** (cur = 1, 2, …, up to thousands of frames). A single captured graph
   bakes the shape → cannot replay a growing backbone (the csm-backbone verdict, B45 §3).
2. **No sub-decoder to graph instead.** Unlike dia2/csm, dia has NO depth decoder — it samples all 9 channels from
   ONE backbone forward + the `logits_dense` head. There is no fixed-shape per-stage block to graph (the lever
   B44/B45 used for the growing-backbone models).
3. **The masked FullMasked path is byte-unsafe here.** Converting to dia2's FullMasked (fixed `[max_seq]` buffer +
   `finfo.min` mask) is the only way to fix the shape — but dia's decoder code **explicitly documents** that the
   finfo.min-padding-slot mask drifts the bf16 SDPA and flips a borderline tail tie ~2500 frames in (B36); dia's
   reference uses an *exact-length all-zero* mask precisely to avoid that artifact. Graphing via the masked path
   would break dia's CUDA-bf16 byte-identity (the LAW forbids it).

Honest skip — not touched. dia is **heavily** launch-bound (baseline CUDA-bf16 **RTF 2.77**: 2601 frames →
83 s for 30 s audio), so a graph *would* help materially — making this an "unavailable win," not an "unneeded"
one. But forcing it requires the byte-unsafe FullMasked path (#3), which the LAW forbids.

## 5. higgs — NOT GRAPHABLE (honest skip): csm-backbone + RopeApply::Start + CUDA ≠ byte-bar

higgs is a **single 36-layer Qwen3-4B AR backbone**, `RopeApply::Start` + `CacheRead::ViewContiguous` (growing
contiguous), NO depth decoder. A triple blocker:

1. **`RopeApply::Start`** — the graph fast-path in `Attention::forward` requires `RopeApply::Positions` (the
   device-position RoPE). higgs's `Start` path is not wired for the device-position decode.
2. **Growing-contiguous, no sub-decoder** — same as dia (the csm-backbone problem) with no fixed-position sub-block.
3. **The CUDA path is not the byte-identity bar.** higgs's byte-identity gate is **CPU-f32 greedy**; the CUDA path
   runs **f16 SAMPLING** (temperature 0.8, top-k 50) because greedy degenerates to silence on this model (the
   sidecar documents it). A CUDA graph is CUDA-only, and there is no greedy CUDA byte-identity bar to hold it to
   (an AB would be self-referential against an un-gated sampled path).

Baseline CUDA-f16 RTF **1.406** (launch-bound) — but the byte-faithful seam does not apply. Honest skip — not
touched.

---

## 6. Files changed (all under `crates/waav-infer-backend-torch/`)

**Shared `nn/` (the genuine bidirectional-graph extension — re-verified dia2 + csm + voxtral byte-identical):**
- `src/nn/rope.rs` — NEW `Rope::cos_sin_exact` (hoist the seq-exact doubled cos/sin out of the captured body) +
  `Rope::apply_doubled` (public capturable apply) + NEW CPU unit `cos_sin_exact_apply_doubled_matches_positions_exact`.
- `src/nn/self_attention.rs` — NEW `Attention::forward_full_with_cossin` (bidirectional `forward_full` consuming
  precomputed cos/sin; the existing `forward_full`/host paths untouched).
- `src/nn/layer.rs` — NEW `TransformerLayer::forward_bidirectional_with_cossin` + a factored `bidirectional_tail`
  shared with the existing `forward_bidirectional` (byte-identical re-block).
- `src/nn/backbone.rs` — NEW `Backbone::forward_bidirectional_with_cossin`.

**omnivoice glue (`src/omnivoice.rs`):**
- `TorchOmnivoice` — `cuda_graph_enabled` (gated `$WAAV_OMNIVOICE_CUDA_GRAPH`, default OFF) + `RefCell<Option<
  OmniGraph>>`; `cuda_graph_active`/`reset_graph`; split `logits` into the host `embed` + `head_from_hidden` +
  `forward_body_eager` (capturable) + `logits_graph` (the warmup→capture→replay driver, **deep-copy out** for the
  CFG-pair aliasing fix); `reset_graph` per generation; NEW `OmniGraph` type. Debug methods
  `debug_graph_argmax_ab` / `debug_graph_replay_newinput_ab` / `debug_full_generation_ab`. Debug seams
  `WAAV_OMNIVOICE_GRAPH_EAGER` + `WAAV_OMNIVOICE_CAPSTEP_EAGER`.

**dots glue (`src/dots.rs`, no shared `nn/` change):**
- `Dit` — `cuda_graph_enabled` (gated `$WAAV_DOTS_CUDA_GRAPH`, CUDA + flow_matching only) + `RefCell<Option<
  DitGraph>>`; `cuda_graph_active`/`reset_graph`; NEW `forward_graph` (per-`total` re-capture driver, deep-copy
  out) + `DitGraph` type. `load_dit` sets the toggle; `fm_solve` routes through `forward_graph`;
  `generate_latents` calls `reset_graph`. Debug seam `WAAV_DOTS_GRAPH_EAGER`.

**Tests (new, `#[ignore]` live-GPU gates):**
- `tests/cuda_torch_omnivoice_graph_ab.rs` — capture==eager (single-forward argmax + replay-new-input + full
  generation, 0/288 each).
- `tests/cuda_torch_omnivoice_graph_perf.rs` — masked-diffusion forward perf A/B.
- `tests/cuda_torch_dots_graph_ab.rs` — DiT capture==graph-eager (latents 0/10240, max|Δ|=0).
- `tests/cuda_torch_dots_graph_perf.rs` — DiT flow-matching perf A/B.

## 7. Honesty notes / caveats

- **omnivoice** ×1.20 (acc.) / ×1.08 (long) — real, the launch-overhead win of the dominant bidirectional
  backbone forward. The graph helps MORE on short (launch-bound) utterances; the long utterance is more
  compute-bound. RTF < 1 is reached on the realistic long synthesis; the T=36 acceptance utterance is 1.169 (the
  residual is the un-graphable eager `step` math + the CPU DAC codec). The bidirectional-graph regime is a genuine
  `nn/` extension, unit-tested + regression-verified (dia2 608/csm 4000/voxtral unchanged).
- **dots** ×1.04 — real but modest; the DiT is the dominant block, but the FM prefix grows per patch → per-patch
  re-capture amortizes poorly (warmup+capture recurs each of ~224 patches; ~7/10 steps replay). The full-synth
  RTF is vocoder-bound (the graph does not touch the f32 BigVGAN). A bucketed multi-graph (one per `total`) would
  amortize better but is out of scope; the per-patch re-capture is the simplest byte-faithful form.
- **dia + higgs** are honestly NOT graphable with the byte-faithful seam (the csm-backbone growing-narrow problem
  with no fixed-position sub-decoder to graph instead; higgs additionally has `RopeApply::Start` and a CUDA path
  that is not its byte-identity bar). Forcing a graph via the masked path would break byte-identity (B36) — the
  LAW forbids trading byte-identity for a perf number. Both are left untouched.
- CUDA-graph is **CUDA-only**; CPU paths stay eager. Default OFF everywhere. The shared-`nn/` additions are purely
  additive (new methods; existing paths bit-for-bit unchanged), re-verified live.
