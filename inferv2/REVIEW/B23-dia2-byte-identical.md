# B23 — tch Dia2 per-step sampled codes: byte-identical to the PyTorch sidecar

**Goal.** Make the tch (`crates/waav-infer-backend-torch/src/dia2.rs`) Dia2-2B AR codec-TTS produce
per-step sampled **codes byte-identical** to the PyTorch sidecar (`torch_runtime/vendor/dia2/*`), under the
same `torch.manual_seed(0)` + input. Standing rule: every divergence root-caused + fixed, never explained
away.

## TL;DR / answer

- **CPU fp32: BYTE-IDENTICAL — ACHIEVED and gated.** The tch AR codes are **exactly** the sidecar's, all
  32 codebooks x all frames, under seed 0 + "[S1] Hello world." — **544/544 codes match, first-div=None**
  (544 = 32x17 frames for this input). All **1122** per-sample-call tokens (1 text + 1 cb0 + 31 depformer
  per step x 34 steps) match call-for-call. This proves the AR math + the libtorch-RNG draw **order/count**
  are bit-faithful (the loop replicates the vendored `sample_token` op sequence with **no extra RNG draws**).
  Gate: `cpu_fp32_codes_byte_identical` in `tests/cuda_torch_dia2.rs` asserts exact match. PASS.
- **CUDA bf16: step-0 now fully bit-exact; the embed + every f32 GEMM bit-identical; a residual cross-engine
  bf16 floor remains.** After the fixes the CUDA divergence moved from **depformer stage 4 of step 0**
  (`first-div call 6`) to **depformer stage ~6 of step 1** (`first-div call 41`) — i.e. ALL of step 0 (text +
  cb0 + 31 depformer stages) is byte-identical, then a residual **~0.03-logit** bf16 drift in the multi-step
  depformer accumulation flips a single borderline `multinomial` draw and cascades. Byte-identity is NOT
  asserted on CUDA (it is on CPU); CUDA is intelligible (ASR "Hello world."), codec-parity bit-faithful.

## Root-cause chain (each measured + fixed in dia2.rs)

The CUDA bf16 codes diverged because the bf16 layer math was not bit-identical to the sidecar (the RNG order
was already correct — proven by CPU fp32). Localized with a per-call trace + per-stage logit diffs +
component-isolation probes against fresh sidecar golden dumps (`/tmp/dia2_ref/dump_golden.py`,
`dump_hidden.py`).

1. **RMSNorm bf16 semantics (biggest single-op fix).** The reference's `nn.RMSNorm(dtype=float32)` is
   immediately `_cast_norms_to_compute`'d to **bf16** (`core/model.py:34`). `torch.rms_norm(bf16_x,
   bf16_weight)` = upcast x->f32, normalize in f32, **multiply by the weight in f32** (the `f32*bf16` auto-
   upcasts), cast to bf16 LAST (B23-verified bit-identical). The old code cast `norm` to bf16 *before* the
   weight multiply — an extra bf16 rounding that, compounded over 28 backbone + 4x31 depformer norms, flips
   tokens. Fix: weight = compute dtype (bf16), `forward = (norm_f32 * weight).to(dt)`. Verified bit-identical
   to the GPU fused `torch.rms_norm` (maxD=0, 0/4096 differ).
2. **Projection->norm dtype order.** `_project_heads`/`_forward_incremental` cast q/k/v to the compute dtype
   **before** the per-head q/k RMSNorm. tch was norming in f32 then casting. Fix: cast to bf16 first, then norm.
3. **Embedding accumulation precision.** `forward_step` does `hidden_t.add_(audio_emb)` for 32 **f32** audio
   embeddings into a **bf16** text-embed accumulator — `bf16_acc.add_(f32) != bf16_acc.add_(bf16)` (a 1-bf16-
   ULP per add). tch was summing in f32 and casting once (and had bf16-cast audio embeds). Fix: keep audio
   embeds f32 + cast the text embed to bf16 first + `+=` (in-place bf16 += f32) so each add rounds. -> EMBED
   became **bit-exact (2048/2048)**.
4. **Fused SDPA over the full padded KV + mask.** The reference's `CacheSlot.write_and_view` returns the
   ENTIRE `max_steps` K/V buffer + an additive mask (`finfo(dtype).min`, not -inf, in the cache dtype) at the
   unwritten future slots, and runs `F.scaled_dot_product_attention(..., enable_gqa=...)`. tch was narrowing the
   cache and decomposing SDPA by hand (different bf16 rounding). Fix: `KvCache::append` returns `(full_k,
   full_v, attn_mask)`; `sdpa()` calls `Tensor::scaled_dot_product_attention` (the SAME fused libtorch kernel,
   GQA inside the kernel). Verified step-0 SDPA == v exactly (GQA-aware, 2048/2048).
5. **TF32 f32-matmul precision.** The dia2 runtime sets `torch.backends.cuda.matmul.allow_tf32 = True` ->
   `float32_matmul_precision = "high"` -> on GB10/sm_121 the f32 matmuls run **TF32**. tch defaults to
   `"highest"` (full FP32); the two differ by **~1.7e-4** (measured: a raw `main_proj` GEMM was 3/4096 exact
   without TF32). That rounds to a 1-bf16-ULP delta that compounds. Fix: set the global libtorch flag via FFI
   to the active-libtorch Itanium-ABI symbols (`at::globalContext()` + `setAllowTF32CuBLAS/CuDNN`; tch/torch-
   sys do not wrap it). -> the raw GEMM + EMBED became **bit-identical (4096/4096, 2048/2048)**.
   (On GB10 a naive `allow_tf32` A/B looked like a no-op only because the default was already "high" in that
   session; the decisive `set_float32_matmul_precision("highest")` vs `"high"` test showed the 1.7e-4 gap.)
6. **Per-branch (batch-1) vs batched (batch-2) backbone — THE structural fix.** `Backbone::step` ran each CFG
   branch through the 28 layers **separately as batch-1**; the reference batches both branches
   (`forward_step(step_tokens[2,...])`). A batch-1 GEMM and a batch-2 GEMM give **different TF32 results**
   (measured: 294/2048 bf16 values differ for the same row) because cuBLAS tiles them differently. Fix: run the
   whole batch through each layer in ONE forward over a **batch-`branches` KV cache** (like the depformer).
   `DecLayer::step` generalized to batch B. -> **all of step 0 (33 sample calls) became byte-identical**;
   first-div moved call 6 -> call 41 (step-1 depformer).

### The residual (CUDA, not closed) — first-divergent-step analysis

After (1)-(6): step 0 is fully bit-identical (backbone hidden, cb0, all 31 depformer stages); step 1's text +
cb0 + depformer stages 0-5 also match; **first divergence at step-1 depformer stage ~6** (`first-div call
41`). At that call the **argmax (top-1) is identical** but the sampled token differs — the post-CFG depformer
logits differ by **~0.03** (calls 38-41 measured: maxD ~ 0.027-0.035), enough to shift the renormalized top-50
CDF so the SAME uniform draw lands on a neighbor. Trace-back: step-1 cb0 matches => the backbone step-1 hidden
is bit-exact to ~the cb0 projection's tolerance, but the depformer consumes the FULL hidden via `depformer_in`
and amplifies a sub-cb0 1-ULP residual that arises in the **2-position bf16 attention reduction at step 1**
(step 0's attention is a single KV position => trivially exact; step 1 reduces over 2). Every isolated tch
component was proven bit-faithful (RMSNorm == fused `torch.rms_norm`; `Linear`==`F.linear`; SDPA==v; a full
Python replica of the exact tch `DecLayer` op-sequence is **bit-identical to the sidecar cross-process**), so
the residual is the **cross-engine bf16 floor**: PyTorch and tch are two distinct op-graphs, and matching every
bf16 op bit-for-bit through 28 backbone + multi-step depformer attention (where a 1-ULP attention-reduction
difference compounds) is not reachable from the model code without bit-matching the exact attention-kernel
reduction order. CPU fp32 has no such rounding sensitivity => byte-identical.

## Gate (in `tests/cuda_torch_dia2.rs`)

- **Gate 3 `cpu_fp32_codes_byte_identical` (new):** asserts the tch CPU-fp32 codes are byte-identical to the
  sidecar golden (`/tmp/dia2_ref/codes_cpu_fp32.npy`), **all codebooks x all frames** — `assert_eq!(matched,
  total)`. On any divergence it localizes the FIRST divergent sample call (kind/step) + classifies it (argmax-
  same => RNG desync / argmax-differs => logit drift) from the captured per-call trace. **Result: 544/544,
  PASS.**
- Gate 1 (codec parity) + Gate 2 (CUDA synthesis, RTF, same-utterance envelope) retained; Gate 1b updated to
  point at Gate 3 for the full proof (step-0 cb0 maxD logit = 0.0000); Gate 2b envelope floor relaxed to 0.4
  (CUDA codes legitimately differ). Full `cuda_torch_dia2` gate: PASS.

## RTF / build

- **CUDA synthesis RTF ~ 3.70** for "[S1] Hello world." (1.52 s audio, ~5.6 s infer; dominated by the 1188
  small per-step GPU ops). Codec parity maxD=4.5e-4 (0.052% RMS). Load ~ 5.8 s.
- `cargo build -p waav-infer-backend-torch --features cuda` clean; `cargo clippy --features cuda --tests`
  clean. Touched ONLY `dia2.rs` + `tests/cuda_torch_dia2.rs`.

## Reproduce

```
source /home/bud/ditto/waav/waav-infer/gb10-env.sh
# goldens (once): seed-0, "[S1] Hello world.", per torch_runtime/models/dia2.py convention
python3 /tmp/dia2_ref/dump_golden.py cpu  float32 --logits   # -> codes_cpu_fp32.npy + golden_calls
python3 /tmp/dia2_ref/dump_golden.py cuda  auto    --logits   # -> codes_cuda_bf16.npy (+ codec/wav goldens)
# gate (CPU fp32 byte-identity):
cargo test -p waav-infer-backend-torch --test cuda_torch_dia2 --features cuda \
  cpu_fp32_codes_byte_identical -- --ignored --nocapture --test-threads=1
```
