# B48 — Torch-TensorRT bring-up on GB10 (Blackwell sm_121)

**Date:** 2026-06-22 · **Box:** NVIDIA GB10 (Grace-Blackwell), aarch64, CUDA 13.0, **sm_121** (compute cap 12.1), 121 GB unified, PyTorch 2.12.0+cu130, Python 3.12.3.

## TL;DR

**Torch-TensorRT RUNS on GB10/sm_121 — REAL, measured, accuracy-preserving.** It installs from stock
wheels (no NGC container, no source build), compiles a Qwen2-style GQA transformer block to a TensorRT
engine on sm_121, is accuracy-preserving vs eager fp16 (correlation > 0.99999, rel max|Δ| < 0.31%), and
the serialized `.ts` engine runs **no-Python** via `libtorchtrt_runtime.so` + `jit::load` — the exact
tch/libtorch path the accel-layer design specifies.

- **Measured FP16 speedup: ~1.7–2.1×** on GQA decoder blocks (NOT the doc's aspirational "5×" — that
  number is unbacked for FP16 on this workload; >2× would need INT8/NVFP4, which is lossy + needs modelopt).
- **Accel layer:** `backend-api::TorchTensorRt::accelerate` (under `accel-tensorrt`) is now **real** for the
  C/C++-free half it owns — it validates the AOT-compiled TRT engine artifact and carries it across the
  seam for the runtime to `CModule::load`. The compile (Python+TRT) and the load (libtorch) correctly live
  in their own crates by §17.1; this is the wiring-ready-AND-real seam node between them.

## 1. Version / support matrix (resolved live on this box)

| Component | Version | Wheel (aarch64) | Note |
|---|---|---|---|
| PyTorch | 2.12.0+cu130 | (pre-installed) | sm_121-capable build already on box |
| **torch-tensorrt** | **2.12.0+cu130** | `torch_tensorrt-2.12.0+cu130-cp312-cp312-manylinux_2_28_aarch64.whl` | 3.7 MB; from `download.pytorch.org/whl/cu130` |
| **TensorRT** | **10.16.1.11** (cu13) | `tensorrt_cu13_libs-10.16.1.11-…manylinux_2_35_aarch64.whl` | 2.5 GB libs wheel; ships `libnvinfer.so.10` |
| modelopt | (not installed) | — | only needed for INT8/NVFP4 *quantized* compile, NOT fp16 |
| executorch / TRT-LLM | (not installed) | — | optional torch_tensorrt extras; NOT needed for the dynamo/TRT fp16 path |

**The version constraint that matters:** torch-tensorrt 2.12.0 requires `tensorrt >=10.16.1,<10.17.0`.
- The plain `pip install tensorrt` resolves to **TensorRT 11.1.0** (ships `libnvinfer.so.11`) → torch-tensorrt
  2.12 looks for `libnvinfer.so.10` and **fails to import**. Must pin **`tensorrt-cu13==10.16.1.11`**.
- **TensorRT support floor for sm_121:** TRT ≤ 10.15 has **no sm_121 path**; the working floor is **TRT
  10.16.1** (paired with torch-tensorrt 2.12 / CUDA 13). Earlier conflicting web summaries ("TRT 10.x maxes
  at sm_8.9", "needs TRT 11.x") are **wrong** — disproven by the live compile+run below. TRT 11.1 also
  exists for sm_121 but is ABI-incompatible with torch-tensorrt 2.12.

### Install recipe (reproduced clean)
```bash
python3 -m venv venv --system-site-packages          # reuse system torch 2.12.0+cu130
venv/bin/pip install --no-deps torch-tensorrt==2.12.0 --extra-index-url https://download.pytorch.org/whl/cu130
venv/bin/pip install tensorrt-cu13==10.16.1.11 dllist  # the MATCHING TRT 10.16 (not 11.x!)
# runtime: tensorrt_libs/ + torch_tensorrt/lib/ on LD_LIBRARY_PATH
```
Non-blocking import warnings (do not affect the fp16 compile): "CUDA 13 not supported for TRT-LLM plugins"
(only the TRT-LLM plugin subsystem), "Unable to import quantization op … install modelopt" (only quantized
models). `--no-deps` on torch-tensorrt avoids ~2 GB of unrelated extras (coremltools source build, aiohttp,
executorch) that otherwise stall the install.

## 2. Compile + accuracy + perf (the ground truth — `/tmp/trt_probe/trt_probe.py`, `trt_verify.py`)

Model: representative **Qwen2-style GQA decoder** — RMSNorm → GQA attention (16 heads / 4 KV, 4× GQA,
causal SDPA) → SwiGLU MLP, hidden 1024. The WaaV LLM-backbone shape (Voxtral / CosyVoice3 Qwen2 LLM).
Compile: `torch.export.export` → `torch_tensorrt.dynamo.compile(..., use_python_runtime=False)` at FP16
(explicit-typing from the fp16 graph — do **not** pass `enabled_precisions` when the graph is already fp16;
that asserts). Engine target confirmed by TRT: `Device(NVIDIA GB10, SM Capability: 12.1), Target Platform:
linux_aarch64`.

**Accuracy-preserving (TRT fp16 vs eager fp16 — lossy by design, bar is tolerance not bit-identity):**

| n_layers | seq | params | max\|Δ\| | rel max Δ | correlation | cosine |
|---:|---:|---:|---:|---:|---:|---:|
| 4 | 256 | 45 M | 0.0078 | 0.16 % | 0.99999971 | 0.99999976 |
| 8 | 512 | 90 M | 0.0098 | 0.19 % | 0.99999944 | — |
| 12 | 1024 | 135 M | 0.0107 | 0.23 % | 0.99999915 | — |
| 24 | 512 | 271 M | 0.0146 | 0.31 % | 0.99999797 | — |

**Tolerance:** rel max|Δ| < 0.5 %, correlation > 0.9999 → PASS everywhere. (Deltas are pure fp16 rounding
in fused TRT kernels; they grow gently with depth as expected, never diverging.)

**Measured FP16 speedup (eager fp16 vs TRT fp16, GB10):**

| n_layers | seq | eager ms/iter | TRT ms/iter | **speedup** |
|---:|---:|---:|---:|---:|
| 4 | 256 | 1.52 | 0.82 | **1.85×** |
| 8 | 512 | 3.94 | 1.85 | **2.13×** |
| 12 | 1024 | 9.21 | 5.54 | **1.66×** |
| 24 | 512 | 12.76 | 6.20 | **2.06×** |

> **The "≈5× NVIDIA-CUDA perf lever" is not substantiated for FP16 on this GQA workload — the honest
> measured number is ~2×.** A 5× would require lossy low-precision (INT8/NVFP4 via modelopt), which is a
> *different* accuracy bar than "accuracy-preserving vs eager fp16". The 2× is real, repeatable, and
> already a meaningful win (kernel fusion + TRT's tuned GEMMs over eager cuBLAS/cuDNN).

## 3. No-Python load path (the design's stated runtime path — PROVEN)

The dynamo-compiled module serializes the TRT engine **embedded in a TorchScript `.ts`** (`torch.jit.trace`
→ `.save`). The proof artifact `trt_decoder.ts` is **122 MB** (a 45 M-param fp16 model is ~90 MB of weights;
the surplus is the built TRT engine) with a `_run_on_acc_0` submodule — i.e. the engine IS embedded, not an
eager fallback.

**Canonical no-Python recipe (verified):** load **only** `libtorchtrt_runtime.so` (RTLD_GLOBAL), then
`jit::load(".ts")` → runs on GB10, output finite, mean_abs 0.7977 (matches the eager ref 0.798).
- ✅ runtime-only `.so` + `jit.load` → **runs**.
- ❌ loading `libtorchtrt.so` *and* `libtorchtrt_runtime.so` together → SIGABRT (`Engine custom class
  already registered`). Use the **runtime** lib alone — it is the purpose-built no-Python runtime.
- tch 0.20 (`tch::CModule::load`, `jit.rs`) is the Rust equivalent: tch already links libtorch; the only
  add is putting `libtorchtrt_runtime.so` in `DT_NEEDED` — the **same `--no-as-needed` force-link** the
  `-backend-torch` build.rs already does for `libtorch_cuda.so`.

## 4. Accel-layer wiring — what changed (`crates/waav-infer-backend-api/` ONLY)

The no-Python TensorRT path is **two stages in two crates** by the §17.1 C/C++ rule:
1. **AOT compile** (offline, Python `torch_tensorrt`+`tensorrt`) → the `.ts` engine. A build/packaging step,
   not a Rust call: `backend-api` is `#![forbid(unsafe_code)]` + C/C++-free, cannot link libtorch/TRT.
2. **Load+run** (runtime, `tch::CModule::load`) → executes the embedded engine. Lives in `-backend-torch`
   (C/C++ legal there), needs `libtorchtrt_runtime.so` force-linked.

`backend-api::TorchTensorRt::accelerate` is the **seam node between them**, and is now **real** for the
in-scope half: it takes the `Acceleratable` carrying the *already-AOT-compiled* engine artifact, **validates
it on disk** (exists / regular file / non-empty — the checks a no-libtorch crate can make), and hands it
across the seam as a typed `AcceleratedModule` for the runtime to `CModule::load`. With the feature off (or
no artifact staged), it returns the typed `AccelUnavailable` → Eager fallback (model still runs CUDA-eager).

**Files changed (only `crates/waav-infer-backend-api/src/lib.rs`):**
- **New public type `TrtEngineArtifact`** + `TrtEngineArtifact::validated(path)` — the validated handle to
  the `.ts` engine the runtime loads (path + byte-len; rejects missing/dir/empty with typed
  `AccelUnavailable`). Pure data — crosses the C/C++-free seam without a `tch` type.
- **`TorchTensorRt::accelerate` (`#[cfg(feature="accel-tensorrt")]`) made real** — recovers the artifact,
  re-validates at accelerate-time, carries it out as an `AcceleratedModule{"torch-tensorrt"}`; declines
  (typed) if the wrong payload was staged.
- **`#[cfg(not(feature))]` arm + `TorchTensorRt` doc** updated — removed the stale "torch_tensorrt not
  present / needs NGC container" claim (it IS installed + proven on sm_121); documented the two-stage path +
  the proven version floor (TRT 10.16.1 / torch-tensorrt 2.12).
- **`trt_supported_sm` comment** — noted the exact sm_121 floor is TRT 10.16.1 (band predicate unchanged).
- **2 new tests:** `trt_engine_artifact_validation` (on-disk checks) + `tensorrt_accelerate_carries_validated_artifact`
  (feature-gated; the real artifact carry + the wrong-payload decline).

**Gates (all green):**
- `cargo test -p waav-infer-backend-api` → **68 passed** (default).
- `cargo test -p waav-infer-backend-api --features accel-tensorrt` → **69 passed**.
- `cargo test -p waav-infer-backend-api --all-features` → **69 passed**.
- `cargo clippy -p waav-infer-backend-api --all-features -- -D warnings` → **clean** (also default +
  `accel-tensorrt`-only).
- `cargo check -p waav-infer-core -p waav-infer-components` → clean (changes are additive; no signature
  changes; dependents unaffected).

## 5. Remaining work to take it end-to-end in the runtime (next, NOT in this B48 scope)

The accel-layer seam is **real + ready**. To run a WaaV model TRT-accelerated in production needs (all in
`-backend-torch` / packaging, which B48 was scoped not to touch):
1. **AOT-compile step** for each TRT-eligible model → ship its `.ts` engine artifact beside the weights
   (a `waav.json`/manifest field pointing the `Acceleratable` at the `.ts`).
2. **`-backend-torch` runtime load**: `CModule::load(artifact.path())` when the mapper selects
   `torch-tensorrt`, with `libtorchtrt_runtime.so` force-linked (mirror the existing `libtorch_cuda`
   `--no-as-needed` in that crate's build.rs; add `LD_LIBRARY_PATH` entry for `torch_tensorrt/lib` in
   gb10-env.sh).
3. **`ops_trt_legal`** could be made real (walk `ModelSpec.ops` against the converter set) — today stubbed
   `true`; the compile itself reports unconvertible ops, and `min_block_size` controls partial fallback.

## 6. Honest bottom line

- **Does Torch-TensorRT run on GB10? YES** — torch-tensorrt 2.12.0+cu130 + tensorrt-cu13 10.16.1.11, stock
  aarch64 wheels, no container. Compiles + runs on sm_121.
- **Speedup:** ~**2×** FP16 (measured, accuracy-preserving). The "5×" is unbacked for FP16 — would need
  lossy INT8/NVFP4.
- **Accuracy:** preserving — correlation > 0.99999, rel max|Δ| < 0.31% vs eager fp16.
- **`accelerate()` status:** **real** for the C/C++-free seam half it owns (validated artifact hand-off),
  and **wiring-ready** for the runtime `CModule::load` half (proven no-Python load; plugs into `-backend-torch`
  with one force-link line). Not a stub.
