# B45 — CUDA-graph capture for the **csm** depth decoder (B43/B44 applied to Sesame CSM-1B)

**Goal.** Apply the proven bit-faithful CUDA-graph seam (B43 `nn::cuda_graph` + `nn::Backbone::with_cuda_graph`;
B44 the dia2 per-stage depformer-graph pattern) to **csm** (Sesame CSM-1B, a dual-AR model: a 16-layer Llama
backbone + a 4-layer Llama DEPTH decoder, structurally like dia2's backbone+depformer). Push csm RTF **< 1.0**,
BIT-FAITHFUL (a captured graph replays the identical ops → byte-identical codes). Gated under
`WAAV_CSM_CUDA_GRAPH=1`, default OFF.

**Verdict — SHIP.** csm's **depth decoder** is now graphed per-depth-position behind `WAAV_CSM_CUDA_GRAPH=1`
(default OFF), and it is **BIT-FAITHFUL**: with the graph ON, csm stays **4000/4000 GREEDY CUDA-bf16
byte-identical** to the sidecar golden (L3 — 125 frames × 32 codebooks), and a new self-contained AB gate
proves the depth-decoder capture == the graph-mode eager path on **all 125 frames × 32 codebooks**. The measured
win is **AR-gen 10072 ms → 8023 ms (−20.3% / ×1.255)**, taking csm **AR-RTF 1.007 → 0.802** and **full-synth RTF
1.007 → 0.805 — RTF < 1.0 REACHED**. csm needed a small, genuine generalization of the shared `nn/` seam (a
graph-mode **contiguous** read-back + a `seq==1` guard) because csm's regime is `CacheRead::Contiguous`
(`mask=None`/`is_causal` flash SDPA), NOT dia2's `FullMasked` (full-padded + `finfo.min` mask → MATH SDPA); the
generalization is unit-tested AND re-verified to leave dia2 (608/608 + 1188/1188 AB) and voxtral byte-identical.
No perf claim breaks byte-identity.

---

## 1. Measured perf — the win, and RTF < 1.0

`source gb10-env.sh`, best-of-3, same 125 frames, byte-identity gated separately. Text =
`"Hello world, this is a test of the CSM model."` (the acceptance utterance; greedy runs the full 125 frames).

### AR loop only (`cuda_torch_csm_graph_perf`, what the graph speeds up)

| config | what is graphed | AR-gen | AR-RTF | win vs OFF |
|---|---|---|---|---|
| **graph OFF** | nothing (fully eager) | 10072 ms | **1.007** | — |
| **graph ON (B45)** | **depth decoder** (the 30 single-token steps/frame × 4 layers) | **8023 ms** | **0.802** | **−20.3% / ×1.255** |

### Full synth (AR + Mimi codec, `cuda_torch_csm::cuda_csm_rtf`)

| config | wall | audio | RTF |
|---|---|---|---|
| graph OFF | 10.07 s | 10.00 s | **1.007** |
| **graph ON (B45)** | **8.05 s** | 10.00 s | **0.805** |

**RTF < 1.0 is reached** (0.80) on BOTH the AR loop and the full pipeline. The Mimi codec decode is a small
fixed cost here, so the AR and full-synth numbers nearly coincide; the AR loop is the whole story and the depth
graph is its lever. (The prompt's stated 1.12 baseline was a full-synth figure from an earlier run; this box now
measures the un-graphed baseline at ~1.01. The relative −20% win is robust and crosses RTF<1 either way.)

### Why the depth decoder is the lever (and the backbone is left eager — see §3)

Per frame, csm runs: the **backbone** = 1 step × 16 layers = **16 layer-forwards**; the **depth decoder** = a
seq-2 prefill (4 layers, 1 call) + **30 single-token steps × 4 layers = 120 layer-forwards**. So the 30
graphable depth single-steps are **~86%** of the per-frame transformer-layer launches — graphing them captures
the dominant launch overhead, exactly mirroring B44 (the dia2 depformer was the lever, not the backbone).

## 2. Bit-faithfulness — proven UNCHANGED with the depth graph ON

`source gb10-env.sh` for all live runs.

| gate | graph OFF (baseline) | **graph ON** |
|---|---|---|
| `cuda_torch_csm::cuda_csm_codes_byte_identical_to_sidecar` L3 (GREEDY CUDA-bf16) | 4000/4000 | **4000/4000** ✓ (125 frames × 32 cb) |
| `cuda_torch_csm` L2 (step0 cb0 argmax/logit) | 420 / 10.125 | **420 / 10.125** ✓ |
| `cuda_torch_csm` L4 (seeded-sampled prefix tracked) | 69 frames | **69 frames** ✓ (identical prefix) |
| `cuda_torch_csm_graph_ab` (depth capture vs graph-eager, ALL codes) | — | **125×32 byte-identical** (capture==eager) |
| `nn::kv_cache::graph_contiguous_matches_host_contiguous` (CPU unit, NEW) | — | **Δ=0** across the reset+reuse pattern |

**The byte-identity chain is fully closed:**
1. CPU unit `graph_contiguous_matches_host_contiguous` — `append_contiguous_graph` (device slot) == host
   `append_contiguous`, bit-exact, including the depth-decoder reset-each-frame reuse pattern.
2. CPU unit `apply_positions_device_matches_host` (pre-existing) — device RoPE == host RoPE, bit-exact.
3. Live **L3** (graph ON, the full capture path): greedy codes **4000/4000 == the CUDA-bf16 sidecar golden** —
   the strongest proof (capture == sidecar directly).
4. Live **AB** (`cuda_torch_csm_graph_ab`): depth-decoder capture == graph-mode eager, **125×32 byte-identical**,
   self-contained (sets the toggle itself, no golden needed).

The graph-ON L3 was run via `WAAV_CSM_CUDA_GRAPH=1 cargo test … cuda_csm_codes_byte_identical_to_sidecar
-- --include-ignored`; the self-contained no-golden proof is `cuda_torch_csm_graph_ab`.

**Regression — the shared `nn/` change re-verified (I touched `kv_cache.rs` + `self_attention.rs`):**
- **dia2** `cuda_torch_dia2::cuda_bf16_codes_byte_identical` (graph ON) = **608/608** ✓; its AB
  `cuda_torch_dia2_graph_ab` = **1188/1188** ✓ (the new `Contiguous` branch + the `seq==1` guard leave dia2's
  `FullMasked` decode path byte-for-byte unchanged; dia2's decode is q==1 so the guard is transparent).
- **voxtral** `cuda_torch_voxtral_vs_ort` still **passes** (100% char-identical clip 1) — its caches are never
  graph-mode, so the new branches are never reached.
- `cargo test -p waav-infer-backend-torch --lib` = **144/144** (+1 new). `cargo clippy --all-targets -D
  warnings` = **clean**. `free -g`: 39 GB free / 106 GB available before and after — no OOM, no leak.

## 3. The csm-specific generalization of the shared seam (vs dia2-only glue)

csm did **NOT** reuse the dia2 seam unchanged — it needed one genuine, minimal generalization in `nn/`, because
csm's attention regime differs from dia2's in the load-bearing way:

| | dia2 | **csm** |
|---|---|---|
| cache read-back | `CacheRead::FullMasked` | **`CacheRead::Contiguous`** |
| decode SDPA | full padded buffer + `finfo.min` mask → **MATH** kernel | narrowed `.contiguous()` K/V, **`mask=None`/`is_causal`** → **flash/mem-efficient** kernel |
| graph cache-read | `append_full_masked_graph` (fixed `[max_seq]` shape, device-length mask) | **`append_contiguous_graph`** (NEW: fixed-length contiguous narrow, device slot) |

**Why dia2's `append_full_masked_graph` cannot be reused for csm byte-identically.** csm's reference reads the
KV as a contiguous `torch.cat` and runs `mask=None`/`is_causal` SDPA — feeding the full padded buffer + a
`finfo.min` mask (dia2's form) would steer libtorch's SDPA onto the **MATH** kernel, which rounds differently in
bf16 and **flips a codebook** (the B27 csm scar). So csm needs a graph-mode read-back that feeds a **fixed-shape
contiguous** K/V to `mask=None` SDPA. The new `KvCache::append_contiguous_graph` does exactly that: it scatters
at the **device** write slot (`index_copy_` reads `cur_index` from device memory → replays to the right slot),
then returns `narrow(2, 0, cur).contiguous()` where the narrow length `cur` is a **fixed host constant per graph
stage** — csm's depth decoder resets its cache each frame, so depth position `p` ALWAYS has length `p+1`, so the
returned shape is stable across replays and the captured kernel sequence is fixed. Byte-identical to the host
`append_contiguous` (same `index_copy_` slot, same `narrow(2,0,cur)`, same `.contiguous()`), proven by the new
CPU unit.

**The `seq==1` guard (the second generalization).** csm's depth decoder, UNLIKE dia2's depformer, has a **seq-2
prefill** at each frame start (positions [0,1] → cb1) that **shares the same graph-mode caches** as the seq-1
single steps. Once `enable_graph_mode()` fires (on the first single step), that seq-2 prefill would also enter
the graph fast-path — but the device-slot scatter + single-position RoPE assume **one** query row (q==1). So the
graph fast-path in `Attention::forward` is now guarded on `seq == 1`: a `seq>1` prefill falls through to the host
path even in graph mode (where the host `append_contiguous`/`write` use the independent host `cur` bookkeeping,
writing slots 0,1 correctly — verified byte-identical by L3). This is a strict generalization: dia2's decode is
q==1 so the guard is transparent (608/608 + 1188/1188 unchanged).

**Backbone left eager (the honest scope).** csm's **backbone** position grows unboundedly per frame
(`prompt_len .. prompt_len+125`), so its `Contiguous` read-back narrow length GROWS each step — a single captured
graph (which bakes the narrow shape at capture) cannot replay a growing backbone byte-identically. dia2 solved
the analogous problem with `FullMasked` (fixed `[max_seq]` buffer + device-length mask), but csm **cannot** use a
mask (forces MATH → flips a codebook, B27). So the csm backbone is **not** graphed — and it doesn't need to be:
it is only ~11% (16/140) of the per-frame layer launches, and the depth decoder (the 86% lever) already takes
RTF < 1.0. This is the same call B44 made (graph the dominant stack, leave the minor one); attempting to graph
the backbone via the masked path would break byte-identity, which the LAW forbids. `nn::Backbone::with_cuda_graph`
is therefore NOT wired for csm (it would require the growing-narrow re-capture, which is not byte-faithful as a
single graph).

## 4. The per-depth-position graph approach (how it works, reusing B44's exact patterns)

csm's depth decoder fires 31 forced steps/frame: a **seq-2 prefill** (positions [0,1] → cb1, eager — one
variable-shape call/frame) then **30 single-token steps** (positions 2..31 → cb2..cb31). Each single step is
**fixed-shape** (q==1) AND **fixed-position**: RoPE position = depth position, KV write slot = position, KV valid
length = position+1 (the dep caches reset each frame), per-position logits head = `codebooks_head[position-1]`. So
**one captured graph per position** (a `Vec<Option<DepthStageGraph>>`) replays for the whole generation — B44's
exact design. The three hard parts B43/B44 solved are reused verbatim:

1. **No host→device copy inside the captured body.** The captured part is ONLY the pure-tensor compute
   (`project → 4 layers → final norm → per-position logits head`). The host part (`embed_token_raw`: the
   `index_select(prev)` of the freshly-sampled host int) runs OUTSIDE the capture, writing its result into the
   static `raw_in`.
2. **Device-position ring-KV (reused, not re-invented).** The 4 depth caches are put in graph mode; before each
   position's replay the device scalars are set to `position` via `KvCache::set_step_device(position)`
   (slot=position, len=position+1, rope_pos=position) — OUTSIDE the captured body. The captured layers take the
   new graph-mode **`Contiguous`** fast-path (`Rope::apply_positions_device` + `KvCache::append_contiguous_graph`,
   `mask=None`/`is_causal` SDPA). These are the EXACT shared `nn` device methods, unit-proven byte-identical to
   the host `apply_positions(&[pos])` / `append_contiguous`.
3. **The capture-time RNG/output traps (reused).** Each position capture uses `CudaGraph::capture_preserving_rng`
   (the body has no RNG op, but `capture_begin/end` advance the default CUDA Philox offset that the sampler draws
   from OUTSIDE the graph), and **discards the capture-time output**, doing one **post-capture replay** to produce
   the eager-identical logits for the capture step. csm is GREEDY at the byte-identity bar (`argmax_token` stays
   outside the graph), so the RNG trap is belt-and-suspenders — it keeps the *sampled* path (L4) from desyncing
   too (the L4 prefix tracked the identical 69 frames graph-ON vs OFF).

**Why per-position graphs stay consistent across frames.** All positions share the 4 caches (hence the shared
device scalars), so before each position's replay `set_step_device(position)` is re-issued. Within a frame the
prefill (eager, host path) writes slots 0,1; then graphed steps execute in order 2→31, each re-scattering its
slot then reading [0..position+1) — whose lower slots were written by the prefill + earlier steps THIS frame, so
the buffer state at every position is identical to eager. The `dep_caches` are allocated once per generation and
the graphs bind their addresses; `DepthDecoder::reset_graph()` is called per generation (the caches are
reallocated) so a stale graph never replays against new buffers. The per-frame `c.reset()` is harmless in graph
mode (it rewinds the host `cur`, which the seq-2 prefill re-advances and `set_step_device` overrides).

**Lazy capture.** Each position warms up `WARMUP=2` eager forwards (cuBLAS/cuDNN plan; the allocator is already
hot from the backbone + earlier positions), then captures + replays-once. All 30 positions reach capture by
frame ~2; thereafter every single step replays.

## 5. Files changed (all under `crates/waav-infer-backend-torch/`)

**Shared `nn/` (the genuine generalization — re-verified dia2 + voxtral byte-identical):**
- `src/nn/kv_cache.rs` — NEW `KvCache::append_contiguous_graph` (graph-mode contiguous single-step read-back:
  device-slot `index_copy_` + fixed-length `narrow(2,0,cur).contiguous()`, byte-identical to `append_contiguous`)
  + NEW CPU unit `graph_contiguous_matches_host_contiguous` (device==host across the reset+reuse pattern, Δ=0).
- `src/nn/self_attention.rs` — `Attention::forward` graph fast-path generalized: gated on `seq == 1` (so csm's
  seq-2 depth prefill falls through to the host path even in graph mode) and dispatched on `cache_read` —
  `CacheRead::FullMasked` (dia2, unchanged) OR `CacheRead::Contiguous` (csm: `append_contiguous_graph` +
  `mask=None`/`is_causal` SDPA, NO mask). Any other config in graph mode falls through (defensive).

**csm glue (`src/csm.rs`):**
- `DepthDecoder` — add `cuda_graph_enabled` (gated by `$WAAV_CSM_CUDA_GRAPH`, default OFF) + a
  `RefCell<Option<DepthGraph>>`; split the single-token step into `embed_token_raw` (host) + `single_step_eager`
  (the capturable `project→4-layers→norm→per-position-logits` body) + `single_step_graph` (the per-position
  warmup→capture→replay driver, reusing `capture_preserving_rng` + discard-capture-output + replay-once);
  `forward_caches` (the seq-1 forward split out for the capture closure); add `cuda_graph_active` + `reset_graph`.
- New types `DepthStageGraph` (one captured graph + its static `raw_in`/`logits_out`) and `DepthGraph` (the
  per-position slots).
- `load_depth` — set `cuda_graph_enabled` from the env toggle + init the graph `RefCell`.
- `generate_codes` — `self.depth.reset_graph()` after allocating the per-generation `dep_caches`; route the
  positions-2..31 single steps through `self.depth.single_step(...)` (was the inlined
  `embed_token_raw→project→forward→logits`).

**Tests (new, `#[ignore]`d live-GPU gates):**
- `tests/cuda_torch_csm_graph_ab.rs` — self-contained byte-identity gate: depth-decoder capture == graph-eager
  on all 125×32 greedy codes (sets `WAAV_CSM_CUDA_GRAPH` itself; `WAAV_CSM_GRAPH_EAGER=1` is the eager reference).
- `tests/cuda_torch_csm_graph_perf.rs` — depth-graph perf A/B (AR-gen + AR-RTF, OFF vs ON, best-of-3).

## 6. Honesty notes / caveats

- The win is **−20.3%** (RTF 1.007 → 0.80) — real, and **RTF < 1.0 is reached**. It comes from graphing the
  depth decoder (the ~86% launch lever); the **backbone is left eager** (§3) because its growing-position
  `Contiguous` read-back is not single-graph-replayable byte-identically AND csm cannot use the masked path
  (would flip a codebook, B27). Graphing the backbone would need orthogonal work (re-capture-on-growth, or a
  bucketed scheme) and is not the lever for RTF<1 here.
- The residual non-graphable per-frame cost is the un-graphable eager work: the backbone step, the seq-2 depth
  prefill, the host-int `embed`/`index_select` gathers (`prev` is a freshly-sampled host int), the per-step
  sampling (`softmax`/top-k/`multinomial`), and the Mimi codec decode — none of which a CUDA graph can capture.
- CUDA-graph is **CUDA-only**; the CPU path stays eager (and unaffected). Default is OFF everywhere; one toggle
  (`WAAV_CSM_CUDA_GRAPH=1`) enables the depth graph.
- Re-capture is **per generation** (the dep caches are per-generation); each position warms up 2 eager forwards
  then captures by frame ~2. csm utterances run the full 125 greedy frames, so the per-position warmup amortizes
  well.
- `set_step_device` is issued for the 4 depth caches × 30 positions × 3 `fill_` ≈ 360 tiny kernels/frame — a
  small known overhead (<< the launch win it enables; the B43 §7 shared-position-tensor optimization would trim
  it but is not needed for RTF<1).
- Two debug seams (gated, default off): `WAAV_CSM_CUDA_GRAPH` (the on/off toggle) and `WAAV_CSM_GRAPH_EAGER`
  (force the eager graph-mode device-position path forever — never capture; used by the AB gate's eager
  reference and to isolate device-position math from capture/replay).
