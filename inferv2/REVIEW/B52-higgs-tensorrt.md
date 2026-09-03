# B52 — Torch-TensorRT END-TO-END for higgs (Qwen3-4B): the high-value case, measured + accuracy-preserving

**Date:** 2026-06-22 · **Box:** NVIDIA GB10 (Grace-Blackwell), aarch64, CUDA 13.0, **sm_121**, 121 GB unified,
PyTorch 2.12.0+cu130, torch-tensorrt 2.12.0+cu130, tensorrt-cu13 10.16.1.11.

## TL;DR

**The B49 Torch-TensorRT path (proven on neutts/Qwen2-0.5B) is generalized to higgs-audio-v3-tts-4b
(`higgs_multimodal_qwen3` — a plain Qwen3-4B AR codec-TTS), the headline case.** higgs is **NOT
CUDA-graphable** (B46: a growing-contiguous AR backbone with no fixed-position sub-decoder, so the fixed-shape
graph couldn't help it), so a **dynamic-shape TRT engine is its ONLY per-step perf lever** — and it works:

- **The 4B DID compile.** `torch.export` + `torch_tensorrt.dynamo.compile` produced a serialized **9.7 GB `.ts`
  engine** at FP16 with a **dynamic KV-seq profile [min 1, opt 256, max 768]**. No OOM (the 121 GB unified pool
  held; peak left ~62 GB free). The script loads ONLY the backbone weights directly at fp16 (no full-precision
  staging) — the audio embed/head/codec stay eager in Rust, so the compile footprint is just the 4B fp16 weights
  (~7.3 GB params) + the TRT builder.
- **The dynamic-shape engine served the growing context.** One engine ran the whole growing-KV AR decode (the
  per-step backbone over a context that grows by 1/frame). Compile-time dynamic verification: **S=1 corr
  0.99999, S=32 corr 0.99998, S=768 corr 0.99999** — the profile covers the real decode range.
- **Backbone accuracy (the right metric): hidden corr = 0.999999, rel max|Δ| = 0.14%** (LIVE A/B on real
  activations, in-process). PASSES the corr > 0.999 / rel < 0.5% bar — accuracy-preserving vs eager fp16.
- **Measured RTF (real AR loop, CUDA f16, sampling, 24 kHz @ 25 fps): eager 1.166 → TRT 1.051** (per-step
  ~1.11× in the live loop; isolated per-step microbench **1.48×**). **TRT pushes higgs from RTF 1.17 toward
  realtime (RTF 1.05, just over 1.0).**
- **Default path unchanged:** with TRT OFF (cfg off, OR cfg on + `WAAV_HIGGS_TRT` unset), the higgs CPU-f32
  byte-identity gate still passes **0/264 greedy codes differ, gates 1–5 all maxΔ=0**; the default binary has
  **no** torchtrt/nvinfer in `DT_NEEDED`.

## 1. The honest accuracy split (same as B49: throughput lever, NOT a byte-identical drop-in)

| metric | value | result |
|---|---:|---|
| **backbone hidden corr** (1 step, same KV+codes, LIVE) | **0.999999** | accuracy-preserving (>0.999) |
| backbone rel max\|Δ\| (LIVE) | **0.14 %** | < 0.5 % bar |
| compile-time backbone corr @ opt S=256 (synthetic KV) | 0.9999974 | passes |
| compile-time corr @ S=1 / S=32 / S=768 | 0.99999 / 0.99998 / 0.99999 | dynamic profile faithful |
| AR sampled agreeing flat prefix | 85 (~10.6 frames) | forks after (expected) |
| RTF eager | **1.166** | (ref) |
| **RTF TRT** | **1.051** | toward realtime |
| per-step speedup (live loop) | **~1.11×** | genuine win |
| per-step speedup (isolated microbench) | **1.48×** (eager 45.6 ms → TRT 30.8 ms) | the kernel win |

As B49 documented for neutts: **TRT FP16 is lossy by design** inside a greedy/sampled AR feedback loop — once a
borderline draw flips (~frame 11 here), the KV diverges and the two sequences become *different valid
utterances*. The TRT audio is real speech (80640 samples, peak 0.40). So this is a **perf lever for
throughput/latency, NOT a drop-in for the byte-identity gate**. The byte-identity path stays eager (default,
untouched). The agreeing prefix (85 flat / ~10.6 frames) is longer than neutts' (15 codes) because the 4B
backbone's per-step perturbation is even smaller (rel 0.14% vs neutts' 0.55%).

**Why the live per-step win (1.11×) < the isolated microbench (1.48×):** the live per-frame cost also includes
the audio head (an f32 matmul over the 8208 tied-vocab), per-codebook top-k sampling, the H2D/D2H tensor
handoffs across the engine boundary, and the codec — the backbone is a smaller fraction of the live frame than
in the isolated decode microbench. The RTF drop (1.166 → 1.051) is the real, end-to-end number.

## 2. The ONE new finding that mattered: fused `torch.rms_norm` does NOT lower through TRT-dynamo here

The B49 path is model-agnostic, but higgs' Qwen3 differs from neutts' Qwen2 in three ways that all live INSIDE
the compiled engine: **fused RMSNorm** (vs neutts' decomposed), **per-head q/k RMSNorm** (Qwen3), and
head_dim 128 / hidden 2560 / 36 layers. The first one bit:

- **First compile (faithful fused `F.rms_norm`, mirroring `nn::RmsNorm::Fused`):** the engine ran, the KV
  outputs (`new_k`/`new_v`) were CORRECT (absmax 42.8 vs eager 42.7), but the **`hidden` output came back ALL
  ZEROS** (absmax 0.0, nonzero 0 → corr NaN, rel_max exactly 1.0). Bisected live: the fused `torch.rms_norm`
  op lowers to a TRT op that **zeroes the final-norm graph output and corrupts the per-layer norms** on this
  stack (GB10 sm_121, TRT 10.16.1, torch-tensorrt dynamo).
- **Fix — DECOMPOSE the RMS norm INSIDE the engine only** (`x·rsqrt(mean(x²)+eps)·w`, the math the fused op
  computes): → TRT hidden corr **0.9999974**, rel max|Δ| **0.28%**. The decomposed form lowers to TRT
  elementwise/reduce ops that compute the identical normalization.

This is a **TRT-lowering quirk, NOT a faithfulness change**: TRT FP16 is already lossy by design (the bar is
corr>0.999 vs the FUSED eager reference, not byte-identical), and the fused-vs-decomposed rounding gap is far
below the FP16 noise floor — the engine's hidden still correlates **0.999999** with the FUSED eager backbone in
the live A/B. **The eager byte-identity path (`higgs.rs`) is UNCHANGED — it keeps `nn::RmsNorm::Fused`; the
decomposition lives ONLY in the offline-compiled throughput engine** (`trt_compile_higgs.py`, documented inline).

(Bisection bonus: the all-zeros-output / correct-KV signature is a clean tell that the bug is in the op feeding
the graph OUTPUT, not the attention math — `new_k` comes from `k` AFTER the q/k-norm + RoPE, so those lowered
fine; only the final-norm-to-output op was broken.)

## 3. The per-step AR-loop integration (the crux — it HOLDS for Qwen3 + 8-codebook delay)

`higgs.rs::generate_raw` dispatches to `generate_raw_trt` when an engine is loaded. The integration:
1. **Eager prefill (byte-faithful):** the existing `backbone.forward(prompt, is_prefill=true)` over the short
   TTS prompt → the first hidden AND a populated ring KV.
2. **Export KV:** the B49 read-only `KvCache::valid_kv()` (additive; model-agnostic; never mutates) → stacked
   `past_k`/`past_v` `[L,1,KV,S,HEAD_DIM]` (S=prompt_len), fp16 on the engine device.
3. **TRT decode loop:** per frame — the **audio head reads the current hidden (Rust), samples 8 codes (Rust),
   runs the delay-pattern state machine (Rust)**, then the next input embedding is the SAME fused multi-codebook
   `audio.embed_codes(codes)` the eager loop feeds; the doubled RoPE cos/sin at the position are built from the
   **half-table** `nn::Rope` (`cat([cos_half, cos_half])` — exactly what `apply_start`/`rotate_half_apply` do);
   the engine runs the backbone → next hidden; `new_k`/`new_v` carry forward. **Only the per-step backbone
   hidden is produced by TRT** — the audio head + 8-codebook sampling + delay/EOC/repeat logic are byte-for-byte
   the eager chain.

**The Qwen3-specific differences are entirely inside the engine** (the Python `StepDecoder`): fused→decomposed
RMSNorm, per-head q/k RMSNorm (decomposed, on the `[.,heads,d]` layout before RoPE), head_dim 128. The Rust
runtime (`trt.rs` `TrtStepBackbone`) and the build.rs force-link are **reused unchanged** — they are model-
agnostic (the engine's `(embed, cos, sin, past_k, past_v) → (hidden, new_k, new_v)` contract is identical).

## 4. Files changed (ALL within scope — `crates/waav-infer-backend-torch/` + the compile script + a test)

| file | change |
|---|---|
| `torch_runtime/trt_compile_higgs.py` | **NEW** — the offline AOT compile (functional KV-explicit **Qwen3-4B** step decoder: decomposed RMSNorm [the TRT-lowering fix], per-head q/k RMSNorm, half-table doubled RoPE, GQA SDPA over the full KV; dynamic KV profile; FP16; loads ONLY the backbone weights directly at fp16; `.ts` serialize + accuracy/perf measure). |
| `crates/waav-infer-backend-torch/src/higgs.rs` | the TRT wiring (the B49 neutts pattern): a `#[cfg(accel_tensorrt)] trt: Option<TrtStepBackbone>` field; `maybe_load_trt` (opt-in `WAAV_HIGGS_TRT=1` + the **AccelMapper** picking `torch-tensorrt` for the live NVIDIA+sm device via `query_cuda_device`); `generate_raw` dispatch + `generate_raw_trt` (the explicit-KV AR loop driving the engine); `stack_caches_fp16` / `doubled_cos_sin_fp16` (the half-table→doubled cos/sin seam); `step_hidden_ab` / `generate_raw_eager` / `generate_raw_active` / `trt_active` (the A/B surface). |
| `crates/waav-infer-backend-torch/tests/cuda_torch_higgs_trt.rs` | **NEW** (`cfg(all(cuda, accel_tensorrt))`) — the e2e accel gate: no-Python load, per-step AR integration, backbone-accuracy A/B (corr>0.999), RTF eager-vs-TRT, audio non-silence. |

**Reused unchanged (model-agnostic, from B49):** `src/trt.rs` (`TrtStepBackbone` no-Python `CModule::load`
runtime), `build.rs` (the `--no-as-needed` force-link of `libtorchtrt_runtime.so` + nvinfer under
`cfg(accel_tensorrt)`), `src/nn/kv_cache.rs` `valid_kv()`. No edit to `trt.rs` was needed — the 4B/Qwen3
generalization is entirely in the compile script + the higgs wiring.

## 5. Gates (all green)

- `cargo clippy -p waav-infer-backend-torch --all-targets -- -D warnings`: **clean** on BOTH the default build
  AND the `accel_tensorrt` cfg.
- `cargo test -p waav-infer-backend-torch --lib` (DEFAULT, cfg off): **145 passed**.
- `cargo test -p waav-infer-backend-torch --lib` (`--cfg accel_tensorrt`): **145 passed**.
- **higgs byte-identity gate, TRT OFF** (`cpu_f32_byte_identical_to_reference`, accel_tensorrt cfg,
  `WAAV_HIGGS_TRT` unset): **gates 1–5 all maxΔ=0, greedy 0/264 codes differ** — THE LAW holds; the opt-in is
  genuinely gated, the default eager path is unchanged.
- **B52 TRT e2e gate** (`higgs_trt_e2e_accuracy_and_rtf`): `trt_active=true`, engine loaded no-Python, backbone
  corr **0.999999**, RTF eager **1.166** → TRT **1.051**, audio non-silent (peak 0.40). **PASSED.**
- `readelf -d` the accel test binary: `DT_NEEDED` carries `libtorchtrt_runtime.so` + `libnvinfer.so.10` +
  `libnvinfer_plugin.so.10`. The **default** higgs test binary has **none** (the force-link only fires under the
  cfg + env).

## 6. How to reproduce

```bash
# (the torch_tensorrt + matching TRT 10.16.1 throwaway venv from B48/B49 — COMPILE-time only)
VENV=/tmp/trt_e2e_venv
TTLIB=$VENV/lib/python3.12/site-packages/torch_tensorrt/lib
TRTLIB=$VENV/lib/python3.12/site-packages/tensorrt_libs

# 1) AOT-compile the higgs Qwen3-4B decode backbone → the staged .ts (free -g first; ONE run at a time)
source gb10-env.sh; export LD_LIBRARY_PATH="$TTLIB:$TRTLIB:$LD_LIBRARY_PATH"
SNAP=/home/bud/.cache/huggingface/hub/models--bosonai--higgs-audio-v3-tts-4b/snapshots/58f6e418777ee36df5c28e1a152c71cbfe147ee9
"$VENV/bin/python3" torch_runtime/trt_compile_higgs.py \
  --ckpt "$SNAP" --out ~/.cache/waav-models/higgs-tts/trt/backbone_fp16.ts --max-kv 768 --opt-kv 256

# 2) build with the cfg + run the e2e gate (no Python at serve time)
export WAAV_TORCHTRT_LIB="$TTLIB" WAAV_TENSORRT_LIB="$TRTLIB" RUSTFLAGS="--cfg accel_tensorrt"
cargo test -p waav-infer-backend-torch --test cuda_torch_higgs_trt -- --ignored --nocapture --test-threads=1
```

## 7. Honest bottom line

- **The 4B compiled, the dynamic-shape engine served the growing context, and it is accuracy-preserving** —
  backbone hidden corr **0.999999** (LIVE), the dynamic KV profile faithful across S=1..768. higgs is the case
  CUDA graphs could not touch (B46), and the dynamic-KV TRT engine is the per-step lever that works.
- **It approached realtime:** RTF **1.166 → 1.051** end-to-end (per-step ~1.11× live, 1.48× isolated). higgs is
  now just over realtime on the TRT path (vs comfortably over 1.0 eager) — the lever is real, though the live
  win is gated by the non-backbone per-frame cost (audio head + sampling + codec + the engine handoff).
- **The honest AR caveat (identical to B49):** lossy FP16 + an AR feedback loop ⇒ the *code sequence* forks
  after a short agreeing prefix (a different valid utterance). This is a perf lever, NOT a byte-identity
  drop-in. **The byte-identity path stays eager (default, untouched, THE LAW still 0/264).**
- **The one new engineering finding:** the fused `torch.rms_norm` op does not lower through torch-tensorrt
  dynamo on this stack (it zeroes the graph output); the decomposed form lowers correctly and is mathematically
  identical. Fixed inside the engine only — the eager byte-identity path keeps the fused kernel.
- **Memory:** not blocked — the 4B compile ran twice (the broken-fused diagnose + the fixed run) with no OOM,
  ~62 GB free throughout. The single-run-at-a-time + fp16-direct-load discipline held.
```
