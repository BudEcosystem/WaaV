# B25 — tch Dia2 CUDA bf16 sampled codes: byte-identical to the PyTorch sidecar

**Goal.** Close the LAST correctness gap from B23: make the tch
(`crates/waav-infer-backend-torch/src/dia2.rs`) Dia2-2B AR codec-TTS produce per-step sampled **codes
byte-identical on CUDA bf16** (not just CPU fp32) to the PyTorch sidecar (`torch_runtime/vendor/dia2/*`),
under the same `torch.manual_seed(0)` + "[S1] Hello world.". Standing rule: every divergence root-caused +
fixed, never explained away. B23 had reached CPU-fp32 byte-identity but left a CUDA bf16 residual it
attributed to a "cross-engine bf16 floor".

## TL;DR / answer

- **CUDA bf16: BYTE-IDENTICAL — ACHIEVED and gated.** The tch CUDA bf16 AR codes are **exactly** the CUDA
  sidecar golden's, all 32 codebooks × all frames — **608/608 (32×19), first-div=None**, under seed 0 +
  "[S1] Hello world.". Every backbone hidden state (36/36 steps), every depformer pre-logit hidden
  (1116/1116), and every layer-4 step-1 internal op is now bit-exact to the sidecar.
- **The diverging op was `RmsNorm`, NOT SDPA.** B23's prime suspect (SDPA backend selection) was
  **disproved** by direct probe — tch's SDPA bit-matches the sidecar for both the depformer (`enable_gqa=
  false` -> mem-efficient) and the backbone (`enable_gqa=true` -> GQA disqualifies mem-efficient, so both run
  MATH). The true first divergence was the **hand-decomposed RMSNorm rounding 1 bf16 ULP off the fused
  `torch.rms_norm` kernel** on a single borderline element. Fix: call the fused `Tensor::rms_norm`.
- **The "cross-engine bf16 floor" B23 concluded does NOT exist.** tch IS libtorch (`LIBTORCH_USE_PYTORCH=1`,
  literally the same `.so` PyTorch loads, torch 2.12.0+cu130, sm_121), so every op is reachable bit-for-bit.
- **CPU fp32 still byte-identical (544/544)** — the fused-RMSNorm change preserves it. **RTF ~3.7-4.3**
  (synth AR+codec, 1.52 s audio). clippy `-p waav-infer-backend-torch --features cuda --tests -D warnings`
  clean. Touched ONLY `dia2.rs` + `tests/cuda_torch_dia2.rs`.

## Method — op-by-op CUDA-bf16 diff to the FIRST divergent op

B23 localized the divergence to "step-1 depformer stage ~6" and stopped at a logit-level symptom. B25 dumped
the tch CUDA intermediates and diffed them op-by-op against fresh sidecar golden dumps, narrowing from coarse
to exact:

1. **SDPA backend isolation (the prime suspect — disproved).** Determined which backend the sidecar's SDPA
   dispatches to (no `sdpa_kernel` context -> the shared SDP priority order `[FLASH, EFFICIENT, MATH, CUDNN]`
   governs), then ran the *exact* depformer- and backbone-shaped q/k/v/mask through tch's
   `Tensor::scaled_dot_product_attention` and compared to per-backend Python goldens:
   - **Depformer** (`enable_gqa=false`, additive mask): flash ruled out (non-null mask), mem-eff usable ->
     default = mem-efficient. **tch bit-matches mem-eff/default (max|D|=0).** (MATH would differ 7.8e-3.)
   - **Backbone** (`enable_gqa=true`): GQA disqualifies BOTH flash and mem-efficient ("No available
     kernel"); cuDNN is usable but MATH precedes it in the priority order -> default = MATH. **tch
     bit-matches MATH/default (max|D|=0).** (cuDNN would differ 1.6e-2.)
   - => **SDPA is fully matched; it was never the divergence.**
2. **Whole-run hidden diff.** Captured branch-0 backbone `hidden_norm` per step + every depformer pre-logit
   `hidden` from BOTH engines. First divergence: **step-1 BACKBONE hidden** (max|D|=1.95e-3, 156/2048) — the
   depformer was a *victim*, not the source (B23 had mis-attributed it to the depformer).
3. **Per-layer bisection (step 1).** Embedded input + all 28 layers bit-exact through layer 3; **layer 4 is
   the FIRST divergent layer** (max|D|=3.9e-3, only **4/2048** elements).
4. **Per-op bisection (layer 4, step 1).** Captured every op inside `DecLayer::step`. Everything bit-exact
   through `post_attn_residual` (so pre_norm, q/k/v proj, q/k_norm, RoPE, KV-append+mask, **SDPA**, o_proj,
   residual all perfect); **`post_norm` is the FIRST divergent op** — exactly **1/2048** element, **1 bf16
   ULP** (`ref=-0.00343323` vs `tch=-0.00344849`, adjacent bf16 values), at idx 1238.
5. **RMSNorm pin-down.** On the exact failing input + layer-4 `post_norm` weight: the fused
   `torch.rms_norm(bf16_x, [2048], bf16_weight, eps)` (== the sidecar's `nn.RMSNorm` forward, cast to bf16 by
   `_cast_norms_to_compute`, `core/model.py:34`) is **bit-exact** to the reference; the tch decomposition
   `(x.f32() * rsqrt(mean(x^2)+eps) * weight).to(bf16)` differs by **1 ULP in 1 element**. The fused kernel
   fuses the `norm * weight` multiply + final-cast in one pass; the decomposed op materializes an
   intermediate f32 and rounds differently. (B23's earlier "Hypothesis B" — cast norm->bf16 before the
   weight — was much worse, 558/2048; B23 fixed *that*, but the surviving decomposition still wasn't the
   fused kernel.)

## The fix (the only code change in dia2.rs)

`RmsNorm::forward` now calls the **exact fused libtorch kernel** instead of a hand-decomposition:

```rust
fn forward(&self, x: &Tensor) -> Tensor {
    let d = *x.size().last().expect("rmsnorm input rank >= 1");
    x.rms_norm([d], Some(&self.weight), self.eps)   // == torch::rms_norm (the sidecar's kernel)
}
```

`tch`'s `Tensor::rms_norm` maps directly to `torch::rms_norm` (`torch-sys` `atg_rms_norm`), the identical
fused kernel the sidecar's bf16 `nn.RMSNorm` invokes. Because tch IS libtorch, this is bit-identical on CUDA
bf16 AND CPU f32. The single ULP it removes had compounded across the 28 backbone + 4x31 depformer norms and
flipped a borderline `multinomial` draw at step 1+, cascading the whole code stream.

### Post-fix verification (CUDA bf16, op-by-op)

| stage | before fix | after fix |
|---|---|---|
| layer-4 step-1 `post_norm` | 1/2048 differ (1 ULP) | **bit-exact** |
| layer-4 step-1 `layer_out` | 4/2048 differ | **bit-exact** |
| backbone hidden, all 36 steps | first-div = step 1 | **first-div = None (all exact)** |
| depformer hidden, all 1116 snaps | first-div = step1/stage0 | **all bit-exact** |
| **CUDA codes vs CUDA golden** | (B23: diverged at step 1+) | **608/608, first-div=None** |

## Why "544 vs 608": the correct CUDA golden

The task framed the target as "544/544". 544 = 32x17 is the **CPU-fp32** sidecar's frame count. The **CUDA
bf16** sidecar produces a *different but equally-valid* stream — 32x**19** = 608 frames (CPU fp32 and CUDA
bf16 round differently -> a different EOS frame, hence 17 vs 19). The honest cross-engine comparison is
**CUDA-tch vs CUDA-sidecar at the same precision/device**, which is **608/608 byte-identical**. (Comparing
CUDA-tch against the CPU golden would be wrong — different precision; it scores 3/544 as expected.) The task
intent — "the CUDA bf16 codes == the sidecar golden" — is met exactly.

## Gate (in `tests/cuda_torch_dia2.rs`)

- **Gate 4 `cuda_bf16_codes_byte_identical` (NEW — the B25 LAW):** loads the dia2 model on **CUDA bf16**,
  runs the full AR loop, and asserts the codes are **byte-identical to the CUDA sidecar golden**
  (`codes_cuda_bf16.npy`), ALL codebooks x frames — `assert_eq!(matched, total)` (608/608). On any
  divergence it localizes the FIRST divergent sample call (kind/step + argmax-same=>RNG-desync /
  argmax-differs=>logit-drift) from the captured trace. Reports **RTF** (synth AR+codec). **Result: 608/608,
  PASS.**
- **Gate 3 `cpu_fp32_codes_byte_identical` (retained):** 544/544 CPU fp32 byte-identical — re-verified PASS
  after the fused-RMSNorm change.
- Gate 1 (codec parity, max|D|=4.9e-4 / 0.05% RMS), Gate 1b (step-0 cb0 top-8 max|D|=0.0000), Gate 2
  (synthesis, RTF 3.73), Gate 2b (envelope correlation now **1.000** — the byte-identical codes track the
  sidecar's CUDA full-engine wave) all retained + PASS. The stale "cross-engine bf16 floor" narrative in the
  Gate 1b/Gate 3 doc-comments is corrected to point at the fused-RMSNorm root cause + Gate 4.
- Goldens persisted to `~/.cache/waav-models/dia2-golden/` (survive `/tmp` cleanup); the gate reads there
  first via a `golden()` resolver, falling back to the dumper's `/tmp/dia2_ref`.

## Honest-floor statement

There is **no remaining irreducible op**. After pinning every bf16 op, the only divergence was the
decomposed-vs-fused RMSNorm, which IS alignable (the fused kernel is exposed by tch as `Tensor::rms_norm`).
The CUDA bf16 codes are now byte-identical to the sidecar. The B23 "cross-engine bf16 floor" conclusion is
**retracted** — it was the un-pinned RMSNorm op, not a fundamental floor.

## RTF / build

- **CUDA synth RTF ~3.7-4.3** for "[S1] Hello world." (1.52 s audio; ~5.7 s AR-gen + ~0.9 s codec;
  dominated by the ~1188 small per-step GPU ops). Load ~5.3 s. (Matches B23's ~3.70; the spread is the extra
  warm AR-gen call the gate makes before the timed synth.)
- `cargo build -p waav-infer-backend-torch --features cuda` + `cargo clippy --features cuda --tests
  -D warnings` clean (also clean for the non-cuda CPU build). Touched ONLY `dia2.rs` +
  `tests/cuda_torch_dia2.rs`.

## Reproduce

```
source /home/bud/ditto/waav/waav-infer/gb10-env.sh
# goldens (once): seed-0, "[S1] Hello world." — both precisions
python3 /tmp/dia2_ref/dump_golden.py cpu  float32 --logits   # codes_cpu_fp32.npy + golden_calls
python3 /tmp/dia2_ref/dump_golden.py cuda  auto    --logits   # codes_cuda_bf16.npy  (the CUDA golden)
# (persisted copies live in ~/.cache/waav-models/dia2-golden/)
# the LAW gate (CUDA bf16 byte-identity):
cargo test -p waav-infer-backend-torch --test cuda_torch_dia2 --features cuda \
  cuda_bf16_codes_byte_identical -- --ignored --nocapture --test-threads=1
# -> "CUDA CODE byte-identity: 608/608 match; first-div=None"
```
