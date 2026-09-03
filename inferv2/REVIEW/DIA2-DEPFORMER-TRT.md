# dia2 DEPFORMER TensorRT engine — single-stream RTF < 1.0 (#3)

**Status: GOAL MET — dia2 single-stream RTF crosses < 1.0** (mean **0.908**) in the opt-in Throughput tier
with BOTH a backbone TRT engine AND a 31-stage **depformer** TRT engine. Live GB10, 2026-06-27, torch
2.12.0+cu130 + torch_tensorrt 2.12.1 + tensorrt 10.16.1.11, branch `waav-infer-v2-build`.
**Uncommitted on disk** (the operator commits).

## TL;DR

| mode | RTF (mean, 12 texts) | macro-WER | micro-WER |
|------|------|------|------|
| **Accuracy** (byte-identical eager+CUDA-graph, default) | 2.389 | 0.0337 | 0.0347 |
| **Backbone** TRT only (`WAAV_DIA2_TRT=1`) | 1.518 | 0.0379 | 0.0347 |
| **Both** TRT (backbone + depformer, `+WAAV_DIA2_DEPFORMER_TRT=1`) | **0.908** | 0.0560 | 0.0556 |

- **Both vs Accuracy = 2.63×; Both vs Backbone-only = 1.67×.** 10 of 12 texts are < 1.0 (0.82–1.03);
  texts 6 & 7 sit just over (1.015, 1.025).
- The depformer is **NOT** the launch-bound B3 dead-end: a TRT-fp16 depth-transformer engine is
  **~2.0–2.3× faster per stage** than the eager-fp16 body — **genuine compute** TRT collapses.
- WER: backbone-only is **WER-neutral** (0.0379 vs 0.0337 — deltas are normalizer artifacts, e.g.
  `finalize→finalise`). Both is +0.022 macro, **driven entirely by ONE outlier fp16 fork** (text 7,
  5 edits, `"a thousand miles"→"a thaw's and my as"`); excluding it, Both macro-WER ≈ 0.023 — i.e.
  **quality-equivalent** to Accuracy. No NaN/garbage in any of the 36 clips.

## What was built

The 31-stage depformer (4-layer depth transformer fired 31×/frame) is **~40% of the AR compute** (the
backbone-only-TRT residual). Profiler component split (CUDA-graph mode, GB10):
`backbone 51.8% / depformer 40.8% / CFG+sample 6.0% / host-rt 1.2% / codec 0.1%`.

### The 5 per-weight-group engines (not 31, not 1)
A single TRT engine bakes its weights as constants, but each depformer stage uses different
`in_proj[g]/out_proj[g]` (5 groups via `cfg::WEIGHTS_SCHEDULE`) **and** a different `logits[stage]` head
(31 heads). The clean factoring:

- The **only per-group weights** are `in/out_proj`; the norms + `wi/wo` MLP are group-independent, and the
  RoPE position + KV length are engine **inputs** (vary per stage). So **one engine per weight group serves
  all 31 of its stages** → **5 engines**, not 31.
- The engine outputs the **post-final-norm HIDDEN** `[B,1,1024]` (the per-stage `logits` head is applied in
  Rust, f32). This is exactly the existing `TrtStepBackbone::step` `(hidden,new_k,new_v)` contract — the
  Rust runtime (`trt.rs`) is **reused verbatim**.

Staged at `~/.cache/waav-models/dia2-2b/trt/depformer_g{0..4}_fp16.ts` (146 MB each, ~730 MB total).
Compile recipe: `torch_runtime/trt_compile_dia2_depformer.py --group G --emit hidden` (the `--emit logits`
mode is the per-stage measurement engine).

### Compile-time accuracy + per-stage speedup (Python, eager-fp16 vs TRT-fp16)
| group | hidden corr | per-stage speedup | 31-stage serial speedup |
|------|------|------|------|
| 0 | 0.99996 | 2.26× | 1.99× |
| 1 | 0.99997 | 2.22× | 2.02× |
| 2 | 0.99998 | 2.06× | 1.96× |
| 3 | 0.99997 | 2.03× | 1.96× |
| 4 | 0.99999 | 2.06× | 1.98× |

(The `logits`-emit measurement engine: per-stage 1.383 ms → 0.666 ms = **2.08×**; 31-stage serial 42.4 ms →
21.9 ms = **1.93×**; corr 0.99999, argmax-match.)

### Rust wiring (`crates/waav-infer-backend-torch/src/dia2.rs`)
- New field `trt_depformer: Option<Vec<TrtStepBackbone>>` (5 engines by group), `#[cfg(accel_tensorrt)]`.
- `maybe_load_depformer_trt()` — loads all 5 iff **backbone TRT is active** AND `WAAV_DIA2_DEPFORMER_TRT=1`
  (all-or-none; any miss → eager depformer). New `trt_depformer_active()` telemetry.
- `generate_codes_trt()` depformer loop: **stage 0 runs eager** (seeds the depformer-KV to S=1 — the engines'
  dynamic profile min is 1, mirroring the backbone's eager step-0 seed), **stages 1..30 run the per-group
  engine** (`hidden,new_k,new_v`), Rust applies the per-stage `logits` head. KV carried forward across
  stages; the CFG/sample/delay/undelay chain is byte-for-byte the eager chain.
- New helpers on `Depformer`: `apply_logits`, `doubled_cos_sin_fp16` (depformer rope), `stack_caches_fp16`.

## Does it cross < 1.0? YES — and the HONEST residual

Mean Both RTF = **0.908** crosses the bar; per-text it lands 0.82–1.03 (2 texts marginally > 1.0). The
residual that keeps the slowest texts near ~1.0 is **NOT** more depformer-body compute (the engine already
~halved that) — it is:

1. **The non-engine Rust depformer work, still paid per stage:** `embed_stage_input` (the `depformer_in`
   f32 GEMM + audio-embed `index_select`) and the per-stage `logits` head (f32, 1024→2050) both run in Rust,
   31× per frame. These were deliberately left out of the engine (they are what let 5 engines cover all 31
   stages); they are the next lever (e.g. fold `logits` into per-stage engines, or run the head in fp16).
2. **The serial AR data-dependency:** each depformer stage consumes the previous stage's sampled code, so the
   31 stages cannot overlap — pure per-stage latency (launch + the small Rust glue + the on-device sample)
   that no single engine call removes.
3. Codec is negligible (~0, CUDA-graphed); sampling is ~6%; the backbone is on TRT.

So the depformer TRT engine **is** a legitimate, decisive compute lever (1.67× on the whole-model wall over
backbone-only), and it crossed the goal. Pushing further below ~0.9 would target levers (1)/(2), not the
depth-transformer GEMMs.

## Invariants confirmed

- **Byte-identical Accuracy default UNTOUCHED:** `cuda_bf16_codes_byte_identical` = **608/608 match,
  first-div=None** with `--cfg accel_tensorrt` compiled in and Throughput NOT selected (`trt`/`trt_depformer`
  both `None` → `generate_codes_inner`, the eager byte-identical path, runs unchanged).
- **Builds green:** `cargo build -p waav-infer-backend-torch --features cuda` (default) **and** with
  `RUSTFLAGS=--cfg accel_tensorrt`. Clippy clean for the changed files (dia2.rs/trt.rs; the only clippy
  warnings are pre-existing in granite/vibevoice/rsb).
- **Lossy-by-design, honestly labeled:** TRT fp16 forks the greedy AR codes after a short agreeing prefix
  (same contract as the backbone engine); the gate is ASR-WER (above), not byte-identity.

## Two engineering findings (de-risked en route)

1. **Stale `backbone_fp16.ts` (compiled earlier) failed `CModule::load`** with a torch schema skew
   (`_is_full_backward_hook` — the serialized module schema differs across torch minor versions).
   **Fix:** recompile with the *current* venv (`trt_compile_dia2.py`); the fresh engine + the 5 fresh
   depformer engines all load cleanly. **Lesson:** restage TRT `.ts` whenever the torch/torch_tensorrt
   version moves.
2. **The server-crate test binary lacked `libtorchtrt_runtime.so` in DT_NEEDED.** `backend-torch`'s build.rs
   force-links the runtime via `cargo:rustc-link-arg`, which **does not propagate to dependent crates'
   binaries** (a cargo limitation). Without the runtime lib the embedded-engine custom op can't be resolved
   at `CModule::load` (manifests as the same schema parse error). **Workaround for the eval:**
   `LD_PRELOAD=$WAAV_TORCHTRT_LIB/libtorchtrt_runtime.so`. **Follow-up (separate from this task):** any
   server-crate TRT path needs the runtime lib force-linked into the *server* binary (or dlopen'd by
   `trt::ensure_runtime_loaded`).

## Repro

```
source /home/bud/torch212_trt_venv/bin/activate && source ./gb10-env-212.sh
VENV=/home/bud/torch212_trt_venv
export WAAV_TORCHTRT_LIB=$VENV/lib/python3.12/site-packages/torch_tensorrt/lib
export WAAV_TENSORRT_LIB=$VENV/lib/python3.12/site-packages/tensorrt_libs
export LD_LIBRARY_PATH="$WAAV_TORCHTRT_LIB:$WAAV_TENSORRT_LIB:$LD_LIBRARY_PATH"

# compile the 5 depformer group engines (hidden output) + (re)compile the backbone
for G in 0 1 2 3 4; do python torch_runtime/trt_compile_dia2_depformer.py \
  --model-dir ~/.cache/waav-models/dia2-2b \
  --out ~/.cache/waav-models/dia2-2b/trt/depformer_g${G}_fp16.ts --group $G --emit hidden; done
python torch_runtime/trt_compile_dia2.py --model-dir ~/.cache/waav-models/dia2-2b \
  --out ~/.cache/waav-models/dia2-2b/trt/backbone_fp16.ts --precision fp16

# 3-way RTF+WER eval (LD_PRELOAD works around the server-binary force-link gap)
export LD_PRELOAD="$WAAV_TORCHTRT_LIB/libtorchtrt_runtime.so"
RUSTFLAGS="--cfg accel_tensorrt" cargo test -p waav-infer-server --features torch \
  --test zz_trt_wer_eval -- --ignored --nocapture --test-threads=1
```

## Files touched (uncommitted)
- `torch_runtime/trt_compile_dia2_depformer.py` — finished: `--emit hidden|logits` + `--group`, fixed the
  31-stage serial bench (stage-0 eager seed), save-before-bench, hidden-corr metrics.
- `crates/waav-infer-backend-torch/src/dia2.rs` — `trt_depformer` field + `maybe_load_depformer_trt` +
  `trt_depformer_active` + 3 `Depformer` helpers + the `generate_codes_trt` both-engines depformer loop.
- `crates/waav-infer-server/tests/zz_trt_wer_eval.rs` — throwaway harness extended to the 3-way
  Accuracy/Backbone/Both RTF+WER comparison.
- Engines staged at `~/.cache/waav-models/dia2-2b/trt/{backbone_fp16,depformer_g0..4_fp16}.ts`.
