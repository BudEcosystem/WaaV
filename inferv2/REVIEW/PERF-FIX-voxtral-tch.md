# PERF-FIX — voxtral-realtime torch arm: RTF regression → fixed, BYTE-IDENTICAL preserved

**Date:** 2026-06-24 · **Box:** GB10 (Grace-Blackwell sm_121, 121 GB unified) · **Env:** `gb10-env.sh`
**Scope:** make the voxtral-realtime **tch (libtorch) STT arm** fast again without losing accuracy.
**Result:** **RTF 1.72 (sweep) / 0.81 (steady, pre-fix) → 0.64 steady / 0.69 gate**, transcript **byte-identical** to the
ORT-CPU reference (`cuda_torch_voxtral_vs_ort` PASSES: 100.0% char-identity, `==` assertion). LAW met (RTF < 1.0, ~0.6).
**No `git commit`, no `cargo fmt`.** Shared `nn::` change re-verified non-regressing on dia2/csm/ark.

---

## Root cause (profiled live, not guessed)

Added a phase-breakdown profiler (`transcribe_profiled`) and ran the kokoro clip warm. The **decode loop is the whole
RTF** (encode ~1.1 s once/clip; prefill ~50 ms; the rest is the 161-step AR loop). Mid-decode `nvidia-smi`:

| | SM clock | power | GPU util | per-step |
|---|---|---|---|---|
| **pre-fix** | 1820 MHz | **8 W** | **0 %** | **54 ms/step** |
| **post-fix** | 2430 MHz | 31 W | **96 %** | **41 ms/step** |

8 W / 0 % util = the decode was **idle-stalled**, not compute-bound. Two costs, both *outside* any useful overlap:

1. **THE #1 COST — a per-step full-vocab f16→f32 table re-cast.** The f32 greedy-decision lm_head was spelled
   `self.embed.transpose(-1,-2).to_kind(Kind::Float)` **inside the per-step `decode_step`**. The tied `embed` is
   `[vocab=131072, 3072]` f16 — so EVERY token re-materialized the **entire** table to f32 (~400 M elements,
   ~1.6 GB write) before the `[1,3072]@[3072,vocab]` matmul. A memory-bandwidth wall that dwarfed the 26-layer decode
   and (being host glue) sat outside the CUDA graph. (The "`embedᵀ` via OP_T view, no weight copy" comment was true for
   the `.transpose` view but missed that `.to_kind(Float)` forces a full table materialization per step.)

2. **Launch-bound 26-layer batch-1 AR step.** Each step issues hundreds of tiny kernels (norm / fused-qkv gemm / RoPE /
   manual-GQA matmul·softmax·matmul / o-proj / SwiGLU × 26 layers); at batch 1 the per-kernel *launch latency*
   dominated (the classic dia2/csm regime), leaving the GPU idle between launches.

A secondary per-step cost was the host-side argmax: `argmax_first` pulled the whole `[131072]` f32 logits row to host
(`Vec::<f32>::try_from`) every step, forcing a full device sync that serialized each step.

(The sweep's RTF **1.72** was a *contended* run; the genuine steady-state pre-fix RTF was **0.81** — still over the ~0.6
LAW target and one bad scheduling slot from breaching the 1.0 gate, so the fix stands regardless.)

---

## The fix (3 accuracy-preserving levers; each capture==eager / same-math-different-place)

1. **Precompute the f32 transposed lm_head ONCE at load** (`embed_f32_t = embed.to(f32).t().contiguous()`), used by a new
   shared `lm_head_f32`. Kills the per-step 1.6 GB re-cast. **#1 lever — ~12 ms/step** (54 → ~43). Byte-identical: same
   f32 values, materialized once instead of per token. *(voxtral.rs)*

2. **CUDA-graph the fixed-shape q==1 AR decode step** (the proven dia2/csm B43 C-shim lever). Voxtral's eager decode uses
   `RopeApply::Start` + `CacheRead::View` (growing-length narrow) + `Kernel::ManualGqa`, which the existing graph
   fast-path didn't cover. Added a **new graph regime** in `nn::Attention::forward`: `(View, ManualGqa)` →
   device-position RoPE (`apply_positions_device`, byte-identical to `apply_start` for q==1) + the FULL padded ring +
   `finfo.min` mask (`append_full_masked_graph`) fed through the ManualGqa `mask` arg. Wired in voxtral via
   `Backbone::with_cuda_graph` + `forward_graph` + per-generation `reset_graph` (default ON for CUDA; opt out
   `WAAV_VOXTRAL_CUDA_GRAPH=0`). **~2.4 ms/step** (43 → 41) AND it moved the GPU from 0 %→96 % util.
   Byte-identical: a replay re-runs identical kernels; the full-buffer ManualGqa equals the narrowed-view ManualGqa
   because the unwritten ring slots are zeros whose `+finfo.min` f32-softmax weight underflows to exactly 0
   (`0·V = 0`) — the same argument as dia2's `append_full_masked`. *(nn/self_attention.rs + voxtral.rs)*

3. **On-device first-max argmax** (`argmax_first_device`): reduce over the vocab on-device (`max` → exact-equality set →
   `where(arange, n).min()` = lowest tied index), pull only the single i64. Removes the per-step full-vocab D2H sync.
   Byte-identical to the host `argmax_first` (same first-max/lowest-index tie-break on the same f32 logits;
   gated by the `argmax_first_device_matches_host` unit test). *(voxtral.rs)*

The `Backbone::forward_graph` debug prints I added to localize the issue were removed — `backbone.rs` is net-unchanged.

---

## Before / after (live, GB10, kokoro_m1_sample 12.05 s)

| metric | PRE-FIX (steady) | POST-FIX (steady) | gate run (cold-ish 1st clip) |
|---|---|---|---|
| **decode per-step** | 53.5 ms | **41.1 ms** | — |
| **decode loop** | 8 700 ms | **6 610 ms** | — |
| encode (once) | 1 100 ms | 1 100 ms | — |
| **whole RTF** | **0.81** | **0.64** | **0.69** |
| GPU util / power | 0 % / 8 W | **96 % / 31 W** | — |

Graph A/B (post-lm_head-fix): OFF 43.5 ms/step (RTF 0.68) vs ON 41.1 ms/step (RTF 0.64) — graph is a net win + fills the
GPU. Lm_head precompute is the dominant lever (~23 %); graph + dev-argmax add the rest.

---

## Byte-identity proof (the LAW: post-fix == pre-fix == ORT-CPU)

- **`cuda_torch_voxtral_vs_ort` (the acceptance gate) PASSES** — `assert_eq!(torch_txt, ort_txt)` holds:
  `"Hello world! This is W.A.V. Infer, a portable voice inference engine, running live on the GB10 Grace BL, a
  C-K-W-E-L-L."` — **100.0 % EXACT char-identity** to ORT-CPU, RTF 0.69. (Mandarin soft clip unchanged: 82.4 % vs ORT —
  the documented q4-vs-bf16 reference drift, not a torch change.)
- **`voxtral_torch_graph_eager_byte_identical` (new gate) PASSES** — graph-ON transcript `==` graph-OFF (eager) transcript,
  proving every perf lever is capture==eager.
- **`argmax_first_device_matches_host` (new unit test) PASSES** — on-device argmax == host argmax across ties / negatives /
  f16, lowest-index tie-break preserved.

## No regression to shared `nn::` consumers (dia2 / csm / ark)

- `cuda_torch_dia2_graph_ab`: **byte-identical on all 1188 calls** (backbone+depformer capture==eager).
- `cuda_torch_csm_graph_ab`: **byte-identical on 125 frames × 32 codebooks**.
- `cuda_torch_ark` (shares `nn::Attention::forward`, no graph): **100.0 % byte-identical**, RTF 0.240.
- `cargo test -p waav-infer-backend-torch --lib`: **192 passed / 0 failed**. `cargo clippy --features cuda --tests`: clean.

My `self_attention.rs` change only *widened* the graph gate (`Positions` → `Positions|Start`) and switched
`match cache_read` → `match (cache_read, kernel)`; dia2 (`FullMasked,_`) and csm (`Contiguous,_`) hit the same arms as
before, and the new `(View, ManualGqa)` arm is reachable only when a cache is in graph mode (voxtral only — ark never
calls `with_cuda_graph`).

---

## Exact files

- `crates/waav-infer-backend-torch/src/voxtral.rs` — precomputed `embed_f32_t` + `lm_head_f32`; `argmax_first_device`;
  `decode_step_graph`; `with_cuda_graph` at load + graph decode loop + `reset_graph`; `transcribe_profiled`; new unit test.
- `crates/waav-infer-backend-torch/src/nn/self_attention.rs` — new `(View, ManualGqa)` graph regime + `Start`-rope gate.
- `crates/waav-infer-backend-torch/tests/cuda_torch_voxtral_perf.rs` (NEW) — phase-breakdown profiler +
  `voxtral_torch_graph_eager_byte_identical` standing gate.

## Note on the ONNX-CUDA arm

The ONNX q4f16 arm on ORT-CUDA (RTF 0.457, byte-identical) remains the *faster* production path for this model; the tch arm
is now comfortably realtime (RTF ~0.64) and byte-faithful, so both arms are viable. This regression was a genuine **code**
bug (per-step table re-cast) + the standard batch-1 launch-bound regime — **not** a libtorch-2.12/aarch64 limit.
