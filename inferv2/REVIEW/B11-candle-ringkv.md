# B11 — Candle Voxtral decoder: device-resident KV + the real bottleneck (weight-broadcast `ucopy`)

**Scope:** `crates/waav-infer-backend-candle/src/voxtral.rs` only. No git commit. Output (transcript) byte-identical.
**Date:** 2026-06-21. **Box:** GB10 (sm_121), candle-core/candle-nn 0.9.2, f16 on CUDA. Clip: `assets/kokoro_m1_sample.wav` (12.05 s, 161 decode steps).

## Headline: RTF 2.874 → 0.622 (4.62×). **RTF < 1 REACHED.** Candle-CUDA is now faster than ORT-CPU.

| stage | infer (ms) | RTF | transcript |
|---|---|---|---|
| **baseline** (committed `6190758`, per-step `Tensor::cat` KV) | 34627 | **2.874** | reference |
| + device-resident ring-KV (candle-nn `KvCache`, in-place `slice_set`) | 34610 | 2.872 | byte-identical |
| + fused `rms_norm` + precomputed `(1+ada_scale)` | 34444 | 2.858 | byte-identical |
| + GQA-native attention (no `repeat_kv` KV expansion) | 34329 | 2.849 | byte-identical |
| **+ `Linear`: 2-D gemm instead of `broadcast_matmul`** ← the fix | 7939 | **0.659** | byte-identical |
| + fused QKV + fused gate/up (one gemm each) | **7492** | **0.622** | byte-identical |

ORT-CPU reference on the same clip: RTF ~0.78–0.83. **Candle-CUDA 0.62 < ORT-CPU 0.78.**

## Gates (both pass, re-run with all optimizations)
- `cuda_voxtral`: RTF **0.622**, word-overlap vs ORT-CPU reference **100.0%**, transcript byte-identical to the pre-optimization candle output:
  `"Hello world! This is W.A.A.V. Infer, a portable voice inference engine, running live on the GB10 Grace BL, a C-K-W-E-L-L."`
- `cuda_vs_ort`: candle RTF 0.63 vs ORT 0.78, de-punctuated char similarity **98.9%** (unchanged from baseline; gate ≥92%).
- `cargo clippy -p waav-infer-backend-candle --all-targets -- -D warnings`: clean.
- 7/7 lib unit tests pass, incl. a **new CPU equivalence test** `sdpa_gqa_matches_expanded_mha` (GQA-native == expanded-MHA, |Δ|<1e-5).

## What actually was the bottleneck (the surprise)

The brief's hypothesis — per-step `Tensor::cat` KV growth (O(n²) device copy) — was **real but not the bottleneck**. Implementing the device-resident ring-KV first (the requested fix) moved RTF only 2.874 → 2.872. nsys then settled it:

```
nsys cuda_gpu_kern_sum (decode), BEFORE the Linear fix:
  ucopy_f16 ........ 85.3% | 28.6 s | 42,613 instances | avg 671 us   ← candle strided-copy kernel
  gemvx (cublas) ... ~13%
```

`ucopy_f16` is candle's `copy_strided_src` (what `.contiguous()` / non-contiguous `reshape` emit). **85% of decode GPU time was strided copies.** Source: **`Linear::forward` used `x.broadcast_matmul(weightᵀ)`**. With `x = [1, q, in]` (3-D) and `weightᵀ = [in, out]` (2-D), candle's `broadcast_matmul` broadcasts the **weight** up to 3-D and then `.contiguous()`-**copies the entire weight matrix on every call** (candle's own documented TODO: *"Avoid concretising the broadcasted matrixes via contiguous"*). The decoder runs ~183 such matmuls/step (q/k/v/o + gate/up/down × 26 layers + the **tied lm_head `[3072,131072]` = 805 MB**), so every decode step was copying hundreds of MB of weights through `ucopy`.

**The fix** (the lever that broke RTF<1): flatten the leading dims to a **2-D `[rows,in] @ weightᵀ` gemm**. A 2-D matmul has no broadcast, so cublas consumes `weightᵀ` as a plain **`OP_T` view** — zero weight copy. nsys after:

```
nsys cuda_gpu_kern_sum (decode), AFTER the Linear fix:
  gemvx (cublas) ... 92.6%   ← genuine M=1 GEMV compute (the real floor)
  ucopy_f16 ........  1.0% | 65 ms | 8,659 instances   (was 28.6 s)  ← 436× less copy time
```

Total decode GPU time dropped ~33.5 s → ~6.7 s. The loop is now **gemm-bound**, which is the correct floor.

## The KV change made (as requested) + the rest

1. **Device-resident pre-allocated KV** — replaced the per-step `Tensor::cat([past_k,new_k])` (+ `.contiguous()`) per layer with candle-nn's **`KvCache`** (one per layer, `KvCache::new(dim=2, max_seq=l+n_audio+1)`). It pre-allocates `[1, kv_heads, max_seq, head_dim]` on-device and writes the step's K/V in place via `slice_set` (`copy2d`), never reallocating. `append()` returns the `[0,current_seq_len)` narrow view that attention reads. This is the well-tested upstream component (preferred over a hand-rolled ring per the brief). Kept correct: the window (8192) never triggers for these clips, and the prefill/decode positions stay in lockstep with the cache write index.
2. **Mask: build once, skip on decode** — the prefill causal/sliding-window mask is built **once** on-device (was rebuilt on the host + H2D-copied every step). For decode steps (`q_len=1`, `kv_len ≤ 8192`) the single newest query row attends the whole in-window context → the mask is all-zeros → passed as `None` (no add, no host build). A correctness guard builds the real mask only past the window (>10-min audio).
3. **GQA-native attention** — removed `repeat_kv` (which `expand`+`reshape`-copied K/V from 8→32 heads every step). Instead fold the `n_rep=4` group factor into the query rows of a per-kv-group batched gemm (`Q:[1,8,4·q,d]`, K/V stay `[1,8,kv_len,d]`). Bit-identical (same per-head dot products); guarded by the new CPU unit test.
4. **`sdpa` Kᵀ via `OP_T` view** — dropped the unconditional `k.transpose(2,3).contiguous()` and `v.contiguous()`; cublas takes the transposed K view directly (caller guarantees contiguous k/v).
5. **Fused `rms_norm`** (candle-nn custom op, f32-accumulated like the hand-rolled one) replacing the ~7-op eager chain; **precomputed `(1+ada_scale)`** at load (was per-step); **fused QKV** and **fused gate/up** into one gemm each (weights concatenated at load, output split with `narrow`) — fewer M=1 GEMV launches.

All changes are arithmetic-preserving; the transcript is byte-identical at every step (verified by re-running both gates after each change).

## On `argmax` and host↔device syncs (item 3) — deliberately NOT changed

The brief asked to keep argmax on-device and sync only the token id. **I verified candle's CUDA `fast_argmax` does not guarantee first-max tie-breaking** (its strided per-thread scan + tree reduction can resolve a logit tie to a non-lowest index), whereas the host loop (and candle's *CPU* argmax) keep the first max. The LAW is byte-identical output, so I kept the host-side argmax. An A/B experiment (`argmax_dev` vs host, all 161 steps) showed **zero disagreements on this clip** *and* — decisively — that swapping it changes **nothing** for wall-clock: the per-step sync (whether D2H of 131k logits or a 1-scalar device-argmax) only lands where the GPU stream completes the step's enqueued work. The cost was never the argmax transfer; it was the `ucopy` weight copies. So on-device argmax is a latent correctness risk with ~0 perf upside here — left as host.

## Is RTF<1 reached, and what's left

**Yes — RTF 0.622 (target <1 met), 4.62× over baseline, and faster than the ORT-CPU reference.** The decode loop is now **gemm-bound** at batch=1/seq=1 (cublas GEMV, M=1, ~92% of GPU time). Remaining headroom, in order:

- **The lm_head GEMV** `[1,3072]@[3072,131072]` ≈ 4 ms/step (~0.6 s total) is the single largest op — inherent to a 131k vocab at M=1.
- **Batch=1 GEMV floor.** Each M=1 cublas GEMV has a fixed ~35–175 µs launch/exec cost; ~50–100 GEMVs/step set the floor. Reducing it further needs either (a) **CUDA-graph capture** of the per-step graph to amortize launch overhead — **not available in candle 0.9.2** (no `cuda_graph`/`begin_capture` API; would require patching candle, out of this crate's scope), or (b) **batching multiple streams** to raise M (architectural; the per-row GEMV efficiency improves ~6× from M=1→M=8 per a GB10 microbench) — that's an engine-scheduler change, not a single-stream win.
- The encoder (one batched pass, seq≈750) is ~1.2 s and was incidentally helped by the fused rms/gate-up; it is not the bottleneck.

Net: the requested device-resident-KV work is in place and correct, and the decoder is now well under real-time. The deepest further gains (CUDA graphs, cross-stream batching) are blocked by the candle version / live outside this crate.
