# B43 — CUDA-graph capture for the in-process tch AR decode loop (dia2 PoC)

**Goal.** Cut the per-step kernel-launch overhead that makes batch-1 AR codec-TTS models launch-bound by
adding **CUDA-graph capture/replay** to the in-process tch decode loop, BIT-FAITHFUL (a captured graph
replays the identical ops → byte-for-byte identical output; pure latency win). Proof-of-concept on **dia2**,
designed as a reusable `nn::Backbone` capability, default OFF.

**Verdict.** CUDA-graph capture **works in tch 0.20 here** (via a small C-ABI shim — tch exposes no graph
API). The reusable `Backbone::with_cuda_graph(true)` + `forward_graph` path is **BIT-FAITHFUL**: with the
graph ON, dia2 stays **608/608 CUDA-bf16 + 544/544 CPU byte-identical**, and the captured backbone is
byte-identical to the eager path on **all 1188 sample calls**. The measured win from graphing the **backbone
only** is **~3% AR-gen** (RTF 1.92 → 1.86 on a 116-frame utterance; 3.55 → 3.42 on the 19-frame gate). The
backbone is only ~18% of the per-step transformer-layer launches; the **depformer (the other ~82%) is the
real lever** and is the clear, designed next step (see §7). No perf claim breaks byte-identity.

---

## 1. Does CUDA-graph capture work in tch here? — YES, via a C-ABI shim

tch 0.20 and torch-sys 0.20 expose **no** CUDA-graph API (zero `CUDAGraph`/`capture`/`graph_pool` symbols in
either crate). But the full `at::cuda::CUDAGraph` C++ API **is exported** by the GB10 PyTorch 2.12 wheel's
`libtorch_cuda.so` (`CUDAGraph(bool)`, `capture_begin(MempoolId_t, cudaStreamCaptureMode)`, `capture_end()`,
`replay()`, `reset()`, `~CUDAGraph()`), and the headers ship (`ATen/cuda/CUDAGraph.h`).

`capture_begin` takes a `std::pair<u64,u64>` by value + a `cudaStreamCaptureMode` enum and the object holds
internal CUDA handles — fragile to bind via raw mangled symbols (the approach `dia2::libtorch_tf32` uses for
the simple `setAllowTF32*` setters). So I bound it with a **60-line C++ shim** compiled by `cc` against the
wheel's headers (C++20, the wheel's `_GLIBCXX_USE_CXX11_ABI=1`), exposing a clean C ABI:

```
waav_cuda_graph_new / _capture_begin(dev) / _capture_end / _replay / _free
waav_cuda_rng_get_offset(dev) / _set_offset(dev, off)   // RNG fix, see §4
```

The shim runs capture on a private side stream (`getStreamFromPool` + `CUDAStreamGuard`, mirroring
`torch.cuda.graph`) with `cudaStreamCaptureModeThreadLocal`, default mempool. `build.rs` compiles it on the
CUDA-present PyTorch path, links `-lcudart` (the shim references `cudaStreamWaitEvent` etc.), and sets
`cfg(waav_cuda_graph)`; CPU-only/non-PyTorch builds get a safe no-op fallback (`CudaGraph::new → None`).

**Feasibility proof (model-free):** `tests/cuda_graph_smoke.rs::cuda_graph_capture_replay_byte_identical`
captures a matmul→gelu→matmul→rmsnorm chain, then replays it across 3 distinct inputs (input written in
place, replay, read static output) → **`max|Δ|=0, bit_identical=true`** every input. CUDA-graph
capture/replay is viable AND byte-faithful in this build.

## 2. The reusable `nn::Backbone` capability

The capability is added to the **shared** `nn::Backbone` (single-sourced AR decode stack), default OFF:

- `Backbone::with_cuda_graph(bool)` — opt-in; `cuda_graph_active()` = enabled AND shim compiled.
- `Backbone::forward_graph(embeds, rope, caches, pos) -> [B,1,hidden]` — the graph-decode per-step forward.
  Warms up `WARMUP=3` eager steps (cuBLAS/cuDNN plan + allocator), then captures the fixed-shape per-step
  forward once and **replays** it thereafter.
- `Backbone::reset_graph()` — drops the captured graph; the caller **must** call this whenever the `caches`
  it passes change identity (a new generation allocates fresh `KvCache`s — a captured graph binds the exact
  cache tensor addresses).

Supporting opt-in additions (existing host paths untouched → all other models byte-identical):

- `nn::cuda_graph::CudaGraph` — the safe wrapper (`new/capture/capture_preserving_rng/replay/reset`); `Send`
  (graph touched on the owning thread only, same contract as `tch::Tensor`), not `Sync`.
- `KvCache::enable_graph_mode()` / `set_step_device(pos)` / `append_full_masked_graph()` — a device-resident
  decode mode: the write **slot** (`index_copy_` index), the mask **valid-length** (`positions.ge`), and the
  **RoPE position** live in device `[1]` tensors updated in place by `set_step_device` (via `fill_`, no H2D),
  so the scatter + mask are recomputed from device memory at replay time. The host-int `append_full_masked`
  bakes those as constants at capture and can't replay across positions.
- `Rope::apply_positions_device(x, pos_idx)` — `apply_positions` with a device index tensor (no host alloc
  inside the captured body).
- `Attention::forward` — a fast-path: when `cache.is_graph_mode()` AND the layer is the dia2 regime
  (`RopeApply::Positions` + `CacheRead::FullMasked`), it routes RoPE+cache-read through the device-position
  methods. Any other config in graph mode falls through to the host path (never silently wrong).

A model enables it by `nn::Backbone::new(...).with_cuda_graph(cuda)` and routing its decode step through
`forward_graph` when `cuda_graph_active()`.

## 3. dia2 wiring

`dia2::Backbone::step` calls `self.stack.forward_graph(...)` when `cuda_graph_active()`, else the existing
eager `forward`. The flag is set in `load_backbone` from `$WAAV_DIA2_CUDA_GRAPH` (truthy) AND CUDA — so the
**default is OFF** and the byte-identity gate can run BOTH paths. `generate_codes_inner` calls
`stack.reset_graph()` after allocating the per-generation `bb_caches` (so a new utterance re-captures against
its own buffers). The **depformer is NOT graphed** (still eager) — see §7.

## 4. Three hard problems solved (the bit-faithful path)

Getting capture byte-identical to eager required solving three non-obvious issues, each caught by a focused
de-risk test before touching the model:

1. **No host→device copy/alloc inside the captured body.** libtorch rejects `Cannot copy between CPU and CUDA
   tensors during CUDA graph capture` — so the `finfo.min` fill + mask-zeros scratch are pre-allocated
   on-device in `GraphState` (outside capture), and `set_step_device` runs *outside* the captured body.
   (Caught by `cuda_graph_ring_kv.rs`.)

2. **Device-position ring-KV replays + stays byte-identical.** `index_copy_(dim=2, index=<device [1]>, src)`
   reads the index from device memory at exec time, so updating its contents replays the scatter to a new
   slot; the mask recomputes from a device `cur` scalar. `tests/cuda_graph_ring_kv.rs` proves this is
   **byte-identical** to the host `append_full_masked` across 12 growing positions; unit tests
   `kv_cache::graph_full_masked_matches_host_full_masked` and `rope::apply_positions_device_matches_host`
   prove the device methods equal the host methods bit-for-bit (CPU).

3. **CUDA-graph capture desyncs the sampler RNG — TWO distinct effects, both fixed:**
   - `capture_begin/end` advance the **default CUDA generator's Philox offset** even for an RNG-free body;
     dia2's `multinomial` samples from that same generator *outside* the graph → a flipped draw.
     `CudaGraph::capture_preserving_rng` snapshots `philox_offset_per_thread` before capture and restores it
     after (shim `waav_cuda_rng_get/set_offset`). Probe:
     `cuda_graph_smoke::cuda_graph_capture_preserves_multinomial_rng` (post-capture draws byte-identical).
   - **The capture-time execution of the body** (on the capture side stream + the graph's private mempool)
     produces a `hidden_out` that differs from eager in the low bits — enough to perturb the softmax tail and
     flip a borderline draw, even though the **argmax matches** (proven by the A/B trace: first divergence at
     the capture step, `kind=text step=3`, argmax SAME, token differs). The *instantiated* graph, however,
     **replays byte-identically to eager** (every post-capture step matched). Fix: discard the capture-time
     output and **replay once immediately after capture** to produce the correct `hidden_out` for the capture
     step (re-scatters the same data to the same slot → harmless; consumes no RNG).

The bisect that localized #3b (`WAAV_DIA2_GRAPH_CAPSTEP_EAGER`, since removed): using the eager output at the
capture step gave 1188/1188; using the capture-time output diverged at the capture step → the divergence was
the capture-time output, not the replay.

## 5. Bit-faithfulness — proven UNCHANGED with the graph ON

`source gb10-env.sh` for all runs.

| gate | graph OFF (baseline) | **graph ON** |
|---|---|---|
| `cuda_torch_dia2 --include-ignored` CUDA bf16 codes | 608/608 | **608/608** ✓ |
| `cuda_torch_dia2 --include-ignored` CPU fp32 codes | 544/544 | **544/544** ✓ (CPU stays eager) |
| codec parity + full synth | pass | pass |
| `cuda_torch_dia2_graph_ab` capture vs eager-graph | — | **1188/1188 sample calls byte-identical** |

Regression (shared `Backbone`/`Attention`/`KvCache`/`Rope` changed): **voxtral** (`cuda_torch_voxtral_vs_ort`)
still **passes** byte-identical (its caches are not graph-mode → the new `Attention` fast-path is never taken).
`cargo test --lib` = **142/142**. `cargo clippy --all-targets -D warnings` = **clean**.

The graph-ON 608/608 was run via `WAAV_DIA2_CUDA_GRAPH=1 cargo test -p waav-infer-backend-torch --test
cuda_torch_dia2 -- --include-ignored ...`; the self-contained proof (no env) is
`cuda_torch_dia2_graph_ab::dia2_graph_capture_vs_eager_trace`.

## 6. Measured perf (the win)

dia2 backbone CUDA graph, **best-of-3, same frame count, byte-identical**:

| utterance | metric | graph OFF | graph ON | win |
|---|---|---|---|---|
| 19 frames ("Hello world.") | AR-gen / RTF | 5248 ms / 3.55 | 5053 ms / 3.42 | **−3.7% / ×1.04** |
| 116 frames (long) | AR-gen / RTF | 17793 ms / 1.92 | 17285 ms / 1.86 | **−2.9% / ×1.03** |

The win is **real but modest** because the graphed **backbone is ~18% of the per-step transformer-layer
launches**: each AR outer step runs 1 backbone forward (28 layers) **plus 31 depformer stages × 4 layers =
124 layer-forwards**. Graphing the backbone alone caps the achievable launch-overhead win at the backbone's
share. (The backbone layers are also wider — hidden 6144 — so partly compute-bound, not purely launch-bound.)

## 7. Does it generalize? — yes, and the depformer is the high-value next step

**The capability is model-agnostic** (it lives in `nn::Backbone`): any launch-bound model that drives a
fixed-shape per-step decode through `forward_graph` with graph-mode `FullMasked` caches gets it. The
device-position ring-KV + the RNG-preserving capture + the capture-step replay are the reusable hard parts,
now solved. Other batch-1 AR codec-TTS models on this backbone (csm, higgs, qwen3-tts, dots-tts, neutts —
all launch-bound) can adopt the same `with_cuda_graph(true)` + `forward_graph` wiring; the only per-model
work is confirming the regime (RopeApply/CacheRead) and adding the depformer/codec-head graphing.

**The depformer is where the win is** (124 of 152 layer-forwards/step). It is graphable and arguably *simpler*
than the backbone: each of the 31 stages has a **fixed** RoPE position (= stage_index), a **fixed** KV slot,
and a **fixed** weight-group — so per-stage capture needs no device-position state, only the static stage
input `x=[B,1,1024]` and output logits. Design: a `DepformerGraph` holding **31 captured graphs** (one per
stage; `dep_caches` reset each outer step but the graphs write fixed slots, so reset is a no-op for them),
the same RNG-preserve + capture-step-replay machinery. This is dia2-specific glue (the depformer is a
`Vec<DepLayer>` with per-group weights, not an `nn::Backbone`), so it's a bounded follow-up, not a shared-lib
change. Estimated win: graphing the depformer should capture most of the remaining ~82% launch overhead.

A second-order optimization for the backbone path: `set_step_device` issues 28 caches × 3 `fill_` = 84 tiny
kernels/step; making the 28 caches' position scalars **views into one shared `[28]` tensor** (1 fill each)
would cut that to 3 kernels/step and recover some of the win the position-update adds back.

## 8. Files changed (all under `crates/waav-infer-backend-torch/`)

New:
- `csrc/cuda_graph_shim.cpp` — C-ABI shim over `at::cuda::CUDAGraph` + the RNG offset get/set.
- `src/nn/cuda_graph.rs` — `CudaGraph` safe wrapper (FFI binding + no-op fallback).
- `tests/cuda_graph_smoke.rs` — capture/replay byte-identity + RNG-preserve probe (model-free).
- `tests/cuda_graph_ring_kv.rs` — dia2-shaped device-slot/mask ring-KV graph byte-identity (model-free).
- `tests/cuda_torch_dia2_graph_ab.rs` — self-contained gate: backbone graph capture == eager (1188/1188).
- `tests/cuda_torch_dia2_graph_perf.rs` — backbone graph perf A/B (long utterance).

Modified:
- `build.rs` — compile the shim with `cc` (C++20 + wheel CXX11 ABI), link `-lcudart`, set `cfg(waav_cuda_graph)`.
- `Cargo.toml` — `[build-dependencies] cc = "1.2"`.
- `src/nn/mod.rs` — `pub mod cuda_graph; pub use cuda_graph::CudaGraph;`.
- `src/nn/backbone.rs` — `with_cuda_graph` / `cuda_graph_active` / `forward_graph` / `reset_graph` + `GraphDecode`.
- `src/nn/kv_cache.rs` — graph mode (`enable_graph_mode`/`set_step_device`/`append_full_masked_graph` + `GraphState`) + parity unit test.
- `src/nn/rope.rs` — `apply_positions_device` + parity unit test.
- `src/nn/self_attention.rs` — graph-mode fast-path in `Attention::forward` (gated; host path unchanged).
- `src/dia2.rs` — route `Backbone::step` through `forward_graph`; `$WAAV_DIA2_CUDA_GRAPH` toggle; `reset_graph` per generation.

## 9. Honesty notes / caveats

- The win is **~3%** because only the backbone is graphed. This is a faithful PoC of the capability, not the
  full speedup; the depformer (§7) is the lever for a large win and is a bounded follow-up.
- CUDA-graph is **CUDA-only**; the CPU path stays eager (and 544/544). Default is OFF everywhere.
- Re-capture happens **per utterance** (caches are per-generation). For very short utterances the warmup +
  capture amortizes poorly; longer utterances amortize better (still only ~3% here because of §6).
- `set_step_device` adds 84 tiny kernels/step (§7) — a known, addressable overhead.
- The two debug env seams (`WAAV_DIA2_CUDA_GRAPH` toggle, `WAAV_DIA2_GRAPH_EAGER` force-eager escape hatch in
  `Backbone::forward_graph`) are intentional and gated; the `CAPSTEP_EAGER` bisect seam was removed.
