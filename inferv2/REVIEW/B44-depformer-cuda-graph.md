# B44 — CUDA-graph capture for the dia2 **Depformer** (the §7 follow-up to B43)

**Goal.** Extend the bit-faithful CUDA-graph perf work (B43, commit 7348254) from the dia2 **backbone** (28
layers, the ~3% PoC) to the **Depformer** — the dominant launch cost (4 layers × 31 codebook stages = **124
layer-forwards/step**, vs the 28 backbone layers). This is the designed §7 follow-up: graph the per-stage
Depformer forward (fixed-shape, q==1) into per-stage CUDA graphs, reusing the EXACT proven B43 patterns,
gated under the existing `WAAV_DIA2_CUDA_GRAPH=1` (default OFF), BIT-FAITHFUL.

**Verdict — SHIP.** The Depformer is now graphed alongside the backbone behind the **same single toggle**, and
it is **BIT-FAITHFUL**: with the graph ON, dia2 stays **608/608 CUDA-bf16 + 544/544 CPU byte-identical**, the
AB gate (`cuda_torch_dia2_graph_ab`) is **capture==eager on all 1188 sample calls** (which now include every
one of the 31 `dep` stages/step — the bulk of those calls), and a new CPU unit gate proves the Depformer's
graph-mode `DepLayer::step` is byte-identical to its host path across the 31-stage accumulation. The measured
win on the 116-frame utterance is **RTF 1.913 → 1.570 (−17.9%, ×1.219)** — i.e. graphing the Depformer took the
B43 backbone-only ~3.8% to **~18%**, capturing most of the remaining transformer-layer launch overhead. No perf
claim breaks byte-identity.

---

## 1. Measured perf — the Depformer win

`source gb10-env.sh`, best-of-3, same frame count, byte-identity gated separately.

### 116-frame utterance (`cuda_torch_dia2_graph_perf`, the steady-state amortized case)

| config | what is graphed | AR-gen | RTF | win vs OFF |
|---|---|---|---|---|
| **graph OFF** | nothing (fully eager) | 17756 ms | **1.913** | — |
| graph ON (B43 baseline) | backbone only (28 of 152 layers) | 17042 ms | 1.836 | −3.8% / ×1.04 |
| **graph ON (B44, this work)** | **backbone + depformer** (all 152 layers) | **14570 ms** | **1.570** | **−17.9% / ×1.219** |

The depformer graph delivered the bulk of the win: it is **124 of the 152 layer-forwards/step** (4 dep layers ×
31 stages = 124, vs 28 backbone). Graphing it on top of the backbone moved RTF 1.836 → **1.570** (a further
**−14.5%** beyond B43's backbone-only).

### 19-frame gate (`cuda_torch_dia2`, the short acceptance utterance)

| config | RTF (Gate-4 codes) | RTF (full synth Gate-2) |
|---|---|---|
| graph ON (B43 backbone-only) | 3.76 | — |
| **graph ON (B44 backbone+depformer)** | **3.37** | 2.88 |

Even on the short 19-frame gate the depformer graph helps (3.76 → 3.37); short utterances amortize the
per-stage warmup/capture less, so the long-utterance number is the representative steady-state win.

## 2. Is RTF < 1 reached? — **No** (and that is expected; the remaining cost is NOT launch overhead)

RTF is **1.570** on the long utterance — better but not realtime. This is honest and inherent: CUDA-graph only
removes **kernel-launch** overhead of the **fixed-shape transformer layers**. With all 152 layer-forwards now
graphed, the residual ~126 ms/frame is dominated by the **un-graphable** per-step work, which is genuine compute
+ host/RNG-bound ops that a CUDA graph cannot capture:

- **Sampling** — 33 × `sample_token`/step (1 text + 1 cb0 + 31 dep), each a `softmax → top-k → multinomial →
  gather` over a 2048-wide vector. `multinomial` is RNG and the top-k is data-dependent → **not graphable**.
- **Classifier-free guidance** — 33 × `classifier_guidance`/step (`topk` + `where` + lerp over the 2-branch
  logits) → data-dependent, eager.
- **Embeddings** — the backbone `embed` (32 `index_select` + projections + host token assembly) and the 31 ×
  depformer `embed_stage_input` (`index_select(prev_audio)` + `dep_in`) — these do **host→device** index
  gathers (`prev_audio` is a freshly-sampled host int), so they MUST stay outside any capture.
- Host buffer assembly, `mask_audio_logits`, the action/cb0 heads.

Reaching RTF < 1 would need orthogonal levers (batched/streamed sampling, streaming the Mimi codec, a faster
sampler, or fewer CFG branches) — out of scope for "graph the depformer." The depformer graph delivered the
full launch-overhead win it set out to (B43 §7: "graphing the depformer should capture most of the remaining
~82% launch overhead" — confirmed: ~3.8% → ~18%).

## 3. The per-stage-graph approach (how it works, and why it is byte-faithful)

The Depformer fires **31 stages** per outer AR step; each stage runs **4 layers** then a final norm + a
per-stage logits head + a `narrow` to `AUDIO_VOCAB_LIMIT`. Crucially, each stage's forward is **fixed-shape**
(`x=[B,1,1024]`, q==1) **and fixed-position**:

- RoPE position = **stage_index** (constant per stage),
- KV write slot = **stage_index** (the depformer cache accumulates one slot per stage, reset each outer step),
- KV valid length = **stage_index + 1**,
- weight-group = `WEIGHTS_SCHEDULE[stage]` (constant per stage).

So **one captured graph per stage** (a `Vec<Option<StageGraph>>` of 31) replays for the whole generation — B43
§7's exact design. The hard parts are the **same three** B43 solved, reused verbatim:

1. **No host→device copy inside the captured body.** The captured part is ONLY the pure-tensor compute (4
   layers → norm → logits-head → narrow). The host part (`embed_stage_input`: `index_select(prev_audio)` +
   `dep_in`) runs OUTSIDE the capture, writing its result into the static `x_in`.

2. **Device-position ring-KV (reused, not re-invented).** Rather than build a depformer-specific
   host-constant-baked graph, I **reuse the backbone's proven device-position machinery**: the 4 depformer
   caches are put in **graph mode** (`KvCache::enable_graph_mode`), and before each stage's replay the device
   scalars are set to `stage_index` via `KvCache::set_step_device(stage)` (slot=stage, len=stage+1,
   rope_pos=stage) — done OUTSIDE the captured body. The captured `DepLayer::step` takes a new **graph-mode
   fast-path** (mirroring `Attention::forward`'s): `Rope::apply_positions_device(q, cache.rope_pos_device())`
   + `cache.append_full_masked_graph(k, v)`. These are the EXACT shared `nn` methods B43 added + unit-proved
   byte-identical to the host `apply_positions(&[pos])` / `append_full_masked`. **No `nn/` file was changed.**

3. **The capture-time RNG/output traps (reused).** Each stage capture uses `CudaGraph::capture_preserving_rng`
   (the body has no RNG op, but `capture_begin/end` advance the default CUDA Philox offset that the sampler
   draws from OUTSIDE the graph), and **discards the capture-time output**, doing a single **post-capture
   replay** to produce the eager-identical logits for the capture step (the capture-time body runs on the side
   stream + private mempool and differs in the low bits — enough to flip a borderline `multinomial` draw — but
   the *instantiated* graph replays byte-identically; the replay consumes no RNG and re-scatters the same slot,
   harmless).

**Why per-stage graphs stay consistent across outer steps.** All 31 stages share the 4 caches (hence the
shared device scalars), so before each stage's replay `set_step_device(stage)` is re-issued. Within an outer
step the graphs replay in order 0→30; stage `s` re-scatters slot `s` then reads slots `[0..s+1)`, whose lower
slots were just (re-)written by stages `[0..s)` THIS step — so the buffer state at every stage is identical to
eager. The `dep_caches` are allocated once per generation and the graphs bind their addresses; `reset_graph()`
is called per generation (the caches are reallocated) so a stale graph never replays against new buffers. The
per-outer-step `c.reset()` is harmless in graph mode (it only rewinds the host `cur`, which `set_step_device`
overrides).

**Lazy capture.** Each stage warms up `WARMUP=2` eager forwards (cuBLAS/cuDNN plan; the allocator is already
hot from the backbone + earlier stages), then captures + replays-once. All 31 stages reach capture in lockstep
at outer-step 2; thereafter every stage replays.

## 4. Bit-faithfulness — proven UNCHANGED with the depformer graph ON

`source gb10-env.sh` for all live runs.

| gate | graph OFF | **backbone + depformer graph ON** |
|---|---|---|
| `cuda_torch_dia2::cuda_bf16_codes_byte_identical` (CUDA bf16) | 608/608 | **608/608** ✓ |
| `cuda_torch_dia2::cpu_fp32_codes_byte_identical` (CPU fp32) | 544/544 | **544/544** ✓ (CPU stays eager) |
| `cuda_torch_dia2::cuda_torch_dia2` (codec parity + full synth) | pass | **pass** (envelope corr 1.000, codec 0.052% RMS) |
| `cuda_torch_dia2_graph_ab` (capture vs eager, ALL sample calls) | — | **1188/1188 byte-identical** (incl. every `dep` stage) |
| `dia2::depformer_layer_graph_mode_matches_host` (CPU unit, NEW) | — | **Δ=0 across the 31-stage accumulation** |

**Regression — graph-OFF models untouched.** I changed **only `dia2.rs`** (the depformer is dia2-specific glue
per B43 §7, "not a shared-lib change"). **No `nn/` file was modified**, so every other model is byte-identical
by construction. Verified live anyway: **voxtral** (`cuda_torch_voxtral_vs_ort`) still passes — its caches are
never graph-mode, so the new `DepLayer` graph branch is never reached and the shared `Attention`/`KvCache`/
`Rope` are bit-for-bit unchanged (English clip 100% char-identical).

`cargo test -p waav-infer-backend-torch --lib` = **143/143** (+1 new). `cargo clippy --all-targets -D warnings`
= **clean**. `free -g`: 42 GB free / 106 GB available before and after — no OOM, no leak.

The graph-ON 608/608 was run via `WAAV_DIA2_CUDA_GRAPH=1 cargo test … cuda_bf16_codes_byte_identical`; the
self-contained no-env proof (capture==eager including the depformer) is `cuda_torch_dia2_graph_ab`.

## 5. The host path is provably unchanged (the `DepLayer`/`Depformer` refactor)

The change splits `Depformer::step` into `embed_stage_input` (host part) + `run_layers_eager` (the capturable
4-layers→norm→logits body), and adds a graph-mode branch to `DepLayer::step`. The **eager (host) path is a
pure rename + re-block** of the original ops — same op order, same spellings — so it is byte-identical (proven
by the unchanged 544/544 CPU gate AND the new CPU parity unit test, which exercises both branches and asserts
Δ=0). The graph-mode branch only swaps the RoPE/cache-read *sourcing* (host int → device scalar), which the
reused `nn` device methods guarantee is byte-identical.

## 6. Files changed (all under `crates/waav-infer-backend-torch/`)

Modified (no NEW files; **no `nn/` files touched**):
- `src/dia2.rs`:
  - `DepLayer::step` — add the CUDA-graph fast-path branch (device-position `apply_positions_device` +
    `append_full_masked_graph`) when the cache is in graph mode; the host path is the unchanged ops re-blocked.
  - `Depformer` — add `cuda_graph_enabled` (gated by `$WAAV_DIA2_CUDA_GRAPH`, default OFF) + a
    `RefCell<Option<DepformerGraph>>`; split `step` into `embed_stage_input` (host) + `run_layers_eager`
    (capturable) + `step_graph` (the per-stage warmup→capture→replay driver, reusing
    `capture_preserving_rng` + discard-capture-output + replay-once); add `cuda_graph_active` + `reset_graph`.
  - New types `StageGraph` (one captured graph + its static `x_in`/`logits_out`) and `DepformerGraph` (the 31
    per-stage slots).
  - `load_depformer` — set `cuda_graph_enabled` from the env toggle + init the graph `RefCell`.
  - `generate_codes_inner` — `self.depformer.reset_graph()` after allocating the per-generation `dep_caches`.
  - new CPU unit test `depformer_layer_graph_mode_matches_host` (graph-mode `DepLayer::step` == host, Δ=0).
- `tests/cuda_torch_dia2_graph_ab.rs` — doc + success-message updated to reflect that the single toggle now
  gates BOTH backbone (B43) and depformer (B44), and the 1188-call AB proof covers every `dep` stage.

## 7. Honesty notes / caveats

- The win is **~18%** (RTF 1.913 → 1.570) — real and the bulk of the achievable launch-overhead win (all 152
  layer-forwards are now graphed). RTF < 1 is **not** reached and is **not** achievable via layer-graphing: the
  residual is dominated by the un-graphable eager sampling (33×/step `multinomial`/top-k) + CFG + the
  host→device embedding gathers (§2). Those need orthogonal work (batched/streamed sampling, streamed codec).
- CUDA-graph is **CUDA-only**; the CPU path stays eager (and 544/544). Default is OFF everywhere; one toggle
  (`WAAV_DIA2_CUDA_GRAPH=1`) now enables both backbone + depformer.
- Re-capture is **per generation** (caches are per-generation); each stage warms up 2 eager forwards then
  captures at outer-step 2. Short utterances amortize this less (the 19-frame gate sees 3.76→3.37 vs the
  116-frame's 1.913→1.570).
- `set_step_device` is now also issued for the depformer (4 caches × 31 stages × 3 `fill_` ≈ 372 tiny
  kernels/step), a small known overhead (~hundreds of ms over a full long utterance, << the launch win it
  enables). The B43 §7 shared-position-tensor second-order optimization would trim it but is not the lever
  for RTF<1.
- The two debug seams are unchanged + shared: `WAAV_DIA2_CUDA_GRAPH` (the on/off toggle) and
  `WAAV_DIA2_GRAPH_EAGER` (force the eager graph-mode path forever — now also short-circuits the depformer
  capture, used by the AB gate's eager reference).
