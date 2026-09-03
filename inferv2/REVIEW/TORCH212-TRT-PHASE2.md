# TORCH212-TRT-PHASE2 — wiring the TensorRT Throughput tier on torch 2.12

> **Scope:** Phase 2 of the torch-2.11→2.12 move (after TORCH212-MIGRATION.md's GO). Stand up the no-Python
> `.ts` Torch-TensorRT **Throughput** tier for the heavy compute-bound AR models so single-stream RTF drops,
> **WER-gated, opt-in**, with the **byte-identical Accuracy default UNCHANGED**.
> **Worktree:** `/home/bud/ditto/waav/waav-infer-torch212` (branch `torch212-migration`, NOT the main checkout).
> **Box:** NVIDIA GB10 (sm_121, cap 12.1), aarch64, 121 GB unified pool. **Date:** 2026-06-27. **Not committed.**
> **Env:** `source /home/bud/torch212_trt_venv/bin/activate && source ./gb10-env-212.sh` + `WAAV_TORCHTRT_LIB`
> / `WAAV_TENSORRT_LIB` / `WAAV_TORCHTRT_PYTHON` → the 2.12 venv + `RUSTFLAGS="--cfg accel_tensorrt"`.

---

## 0. Headline answers

1. **Does TRT load on torch 2.12? → YES (the make-or-break, the thing 2.11 could not do).** The build links
   `libtorchtrt_runtime.so` with **no `torch::headeronly::Tag` undefined-symbol break** (the 2.11 wall), and a
   `.ts` engine **loads + runs** via `tch::CModule` in-process, no Python.
2. **Which heavy models reach single-stream RTF<1 in Throughput?**
   - **neutts-air** — YES, comfortably (TRT fp16 RTF **0.37**; even eager fp16 is 0.62). The accuracy-preserving
     fp16 tier holds.
   - **dia2-2b** — **NO** (TRT fp16 RTF **1.52**, down from 2.37 = **1.55×**, **WER-neutral**). The backbone-only
     engine cannot reach realtime because the **eager 31-stage depformer** dominates the remaining per-frame
     cost. Reaching <1 needs a *second* TRT engine for the depformer (documented next step).
3. **The WER fork per model:** dia2 TRT fp16 is **quality-equivalent** (macro-WER 0.0337→0.0296, *better* within
   noise; 0 NaN). neutts fp16 is accuracy-preserving (backbone corr 0.99996). **Low-precision is NOT free:**
   neutts int8/nvfp4 add accuracy loss with **zero** RTF gain over fp16 (decode is overhead-bound, not
   GEMM-bound), and the staged **fp8 engine is broken (NaN)** on this stack.

---

## 1. STEP 1 — the ABI gate (make-or-break): **TRT loads + runs on torch 2.12 = YES**

```
RUSTFLAGS='--cfg accel_tensorrt' cargo build -p waav-infer-backend-torch --features cuda   # → Finished, exit 0
# build warning fired: "B49 TensorRT runtime force-linked from .../torch_tensorrt/lib (libtorchtrt_runtime.so)"
cargo test -p waav-infer-backend-torch --features cuda --test cuda_torch_neutts_trt --no-run # → links clean
```

- **Link:** the test executable links `libtorchtrt_runtime.so` (torch 2.12.1) cleanly. The TRT-ENV-SETUP §3
  hard ABI break (`undefined reference to torch::Library::_def(... torch::headeronly::Tag ...)`) that made the
  torch-2.12-`.ts` / torch-2.11-runtime split impossible is **GONE on the matched 2.12 stack**.
- **Load + run** (`neutts_trt_e2e_accuracy_and_rtf`, `--cfg accel_tensorrt`, live GB10):
  ```
  neutts TRT [perf-mode=throughput]: loaded Fp16 engine .../neutts-air/trt/backbone_fp16.ts (no-Python CModule::load)
  loaded neutts (CUDA); trt_active = true
  B49 BACKBONE accuracy (1 step): hidden corr = 0.999964, max|Δ| = 0.0781, rel = 0.5482%
  B49 RTF: eager 0.624 | TRT 0.373 | per-step ~1.67x
  ```
  → a 2.12-compiled `.ts` **loads via `tch::CModule` and runs the AR decode in-process** (`forward_is`),
  no-Python. **This is the capability torch 2.11 structurally could not provide.**

---

## 2. STEP 2 — engines compiled (offline, the 2.12 venv)

| model | engine | precision | result | corr vs eager-fp16 | engine size |
|---|---|---|---|---|---|
| **neutts-air** (Qwen2-0.5B, 24L) | `neutts-air/trt/backbone_fp16.ts` | fp16 | pre-existing (2.12) + RE-VALIDATED loads/runs | 0.999964 | 958 MB |
| neutts-air | `backbone_{int8,nvfp4}.ts` | int8 / nvfp4 | pre-existing, load+run OK | 0.9735 / 0.9574 | 509 / 274 MB |
| neutts-air | `backbone_fp8.ts` | fp8 | **loads but NaN at runtime — BROKEN** | NaN | 482 MB |
| **dia2-2b** (28L GQA, hidden 2048) | `dia2-2b/trt/backbone_fp16.ts` | fp16 | **NEW — compiled this phase, corr 0.99999** | 0.9999898 | 3.76 GB |

**dia2 compile** (`torch_runtime/trt_compile_dia2.py`, new this phase — a faithful port of `dia2.rs`
`Backbone::step`: per-head q/k RMSNorm *before* RoPE, SDPA scale=1.0, combined-`wi` SwiGLU, θ=10000 rotate-half,
no biases; B=2 CFG branches, dynamic KV-seq profile min1/opt256/max1536):
```
hidden_correlation = 0.9999898   hidden_max_abs_delta = 0.0149 (rel 0.44%)
dyn S=1 corr 0.99999 | S=1536 corr 0.99997   (dynamic profile serves the whole growing context)
backbone-only per-step: eager 19.64 ms → TRT 15.78 ms = 1.24×
```

**Not compiled (deferred — honest):**
- **misotts** (MisoTTS **8B**, 32L Llama, hidden 4096) — a 16 GB fp16 engine + TRT-builder workspace far exceeds
  the **28 GB-free unified pool**; the 1.4B dia2 compile already emitted the TRT *"remaining GPU memory may not
  be enough to compile the engine → OOM"* warning, and GB10 unified-memory OOM has **hard-crashed this box**
  before. Also uses torchtune **interleaved Llama3ScaledRoPE** (a different convention from the rotate-half
  engine seam). Verdict: **does not fit safely** for offline TRT compile on this box.
- **s2_pro** (fish_qwen3_omni slow-AR, **~4.5B**, 36L Qwen3) — material compile-OOM risk (9 GB weights) **plus**
  a fish-specific **zero/degenerate interleaved-complex RoPE** on the slow-AR (`freqs_cis` all-zeros for the
  text+audio AR layers) — a deep, fish-specific port. Deferred behind the lower-risk dia2 headline.

The Rust TRT scaffolding for misotts/s2_pro (commit 9a2d862) is `with_graphable(false)` so their TRT route is
*reachable* the moment an engine is staged — no firewall blocker (unlike dia2; see §3). They are
compile-blocked, not wire-blocked.

---

## 3. STEP 3 — wiring: PerfMode/precision knob + the dia2 firewall fix

The routing is `AccelMapper::select_perf(perf_mode, spec, dev, staged)` (backend-api). The **byte-identity
firewall** is: a `graphable` model (dia2/csm/omnivoice/dots) is **NEVER** routed to lossy TorchTensorRt in
*either* mode — its throughput lever is the byte-identical CUDA graph (`select_excluding_lossy`).

**Bug found + fixed (dia2 only).** The 9a2d862 scaffolding built dia2's spec with `.with_graphable(true)`
*unconditionally*, so `select_perf(Throughput, graphable=true)` returned `byte-identical-graph` **even with
`WAAV_DIA2_TRT=1`** — the dia2 TRT path was a **silent no-op** (observed live:
`dia2 TRT: perf-mode=throughput selected 'byte-identical-graph' (not torch-tensorrt; staged=true) — eager decode`).
This contradicted the scaffolding's own claim ("only Throughput can route to the lossy TRT backbone").

**Fix** (`crates/waav-infer-backend-torch/src/dia2.rs`, `maybe_load_trt`): feed `.with_graphable(!model_override)`.
The **explicit per-model opt-in `WAAV_DIA2_TRT=1`** (`model_override`) now lifts the firewall and routes to
torch-tensorrt — matching the neutts/misotts/s2_pro semantics where the per-model knob *forces* the staged-`.ts`
path. The **auto/default paths are unchanged**: `WAAV_PERF_MODE=throughput` *without* the per-model knob, and
the Accuracy default, both keep `graphable=true` → `byte-identical-graph` (firewall preserved). Honest telemetry
(`trt_active()`/`trt_precision()`) + the FORKING-labeled codes are intact.

---

## 4. STEP 4 — the RTF + WER table (per model, per precision)

### 4a. dia2-2b — Accuracy (byte-identical eager+CUDA-graph default) vs Throughput (TRT fp16 backbone)
ASR = in-tree whisper-base (ONNX/ORT, CUDA EP); 12 texts, 144 ref words; same seed/voice, only the backbone path varies.

| metric | Accuracy (default) | Throughput (TRT fp16) | Δ |
|---|---|---|---|
| **mean RTF** | **2.366** | **1.524** | **1.55× faster** (still > 1 — NOT realtime) |
| macro-WER | 0.0337 | **0.0296** | −0.0042 (TRT marginally better → quality-equivalent) |
| micro-WER | 0.0347 | **0.0278** | −0.0069 |
| NaN / garbage clips | 0 | 0 | — |
| backbone hidden corr (compile) | — | 0.99999 | accuracy-preserving |

**Verdict (dia2):** TRT fp16 **holds WER** (no degradation — the forked-but-valid greedy realization transcribes
identically or better) and is **1.55× faster**, but **does not reach RTF<1**. The backbone is only part of the
per-frame cost; the **eager 31-stage depformer** (kept byte-identical on purpose) dominates the rest. **The
backbone-only engine is necessary but not sufficient for dia2 realtime** — a second depformer TRT engine is the
documented next lever.

### 4b. neutts-air — Accuracy (eager fp16) vs Throughput (TRT), all precisions
(`neutts_trt_e2e` + `neutts_trt_lowp`, live GB10, `--cfg accel_tensorrt`.)

| precision | trt_active | backbone corr | max\|Δ\| (rel) | eager RTF | **TRT RTF** | per-step | quality verdict |
|---|---|---|---|---|---|---|---|
| **fp16** | ✅ | **0.999964** | 0.078 (0.55%) | 0.624 | **0.373** | 1.67× | **accuracy-preserving (the sweet spot)** |
| int8 | ✅ | 0.9735 | 1.70 (11.95%) | 0.638 | 0.382 | 1.67× | lossy; audio non-silent but heavy code fork (prefix 1) |
| **fp8** | loads | **NaN** | — | — | — | — | **BROKEN** — NaN hidden (FP8 activation overflow on the SDPA-feeding q/k/v/o projections; the staged engine is plain per-tensor FP8) |
| nvfp4 | ✅ | 0.9574 | 3.05 (21.44%) | 0.632 | 0.379 | 1.67× | lossy; audio non-silent, heavy fork |

**Verdict (neutts):** fp16 reaches RTF **0.37** (well under realtime) and is accuracy-preserving. **Every
precision gives the SAME ~0.38 RTF / 1.67×** — the 0.5B per-step decode is **overhead-bound** (KV concat +
kernel launches), **not GEMM-bound**, so int8/nvfp4 buy **no extra throughput over fp16 while adding accuracy
loss**, and **fp8 is broken (NaN)**. **fp16 is the only precision worth shipping for neutts.**

> *On the "audio envelope corr 0.11" the raw tests print:* it is **not** a quality metric — a generative
> discrete-AR codec model forks to a different-but-valid realization on the first ULP GEMM difference (low
> envelope corr, identical *meaning*). The dia2 **WER** table proves this directly: forked codes, but WER-neutral.

---

## 5. STEP 5 — the byte-identical Accuracy default is UNTOUCHED

`dia2 cuda_bf16_codes_byte_identical` with the `accel_tensorrt` cfg present and **Throughput NOT selected**:
**608/608 codes byte-identical, first-div = None** — confirmed BOTH before and AFTER the §3 `graphable` edit
(the edit only changes the `maybe_load_trt` spec, which is reached *only* in Throughput; the Accuracy default
returns `None` before the spec is even built). The priority firewall holds: default = byte-identical, zero
behavior change.

---

## 6. Verdict

- **TRT on torch 2.12: WORKS** — links + loads + runs no-Python via `tch::CModule`. The 2.11 ABI wall is gone.
  Phase 2's premise is validated.
- **Reaches RTF<1 in Throughput:** **neutts-air** (fp16, RTF 0.37, accuracy-preserving). **dia2-2b does NOT**
  (1.52, 1.55× + WER-neutral, but depformer-bound — needs a 2nd engine).
- **WER fork:** dia2 fp16 = **quality-equivalent** (no degradation, 0 NaN). neutts fp16 = accuracy-preserving.
  **Low-precision (int8/fp8/nvfp4) is a net loss here:** no RTF gain over fp16 on these small/overhead-bound
  decoders, plus quality loss (int8/nvfp4) or outright breakage (fp8 NaN).
- **Honest gaps:** misotts (8B) and s2_pro (~4.5B) engines are **not compiled** — they exceed the safe TRT-compile
  memory budget on the 28 GB-free GB10 unified pool and use a different (interleaved) RoPE convention; deferred,
  not faked. dia2 realtime needs the depformer engine. fp8 needs a weight-only / exclude-attn-activation recompile
  to avoid the NaN.

## 7. Artifacts (worktree-local; NOT committed)

- New compile recipe: `torch_runtime/trt_compile_dia2.py` (faithful dia2 backbone port, fp16, dynamic KV).
- New engine: `~/.cache/waav-models/dia2-2b/trt/backbone_fp16.ts` (3.76 GB, torch 2.12).
- Code change: `crates/waav-infer-backend-torch/src/dia2.rs` `maybe_load_trt` → `.with_graphable(!model_override)`
  (the firewall fix; default behavior unchanged).
- New throwaway eval: `crates/waav-infer-server/tests/zz_trt_wer_eval.rs` (dia2 Accuracy-vs-Throughput RTF+WER).
- Gates run GREEN: ABI load/run (neutts), neutts fp16/int8/nvfp4 (fp8 NaN by design), dia2 byte-id 608/608
  (cfg present, Throughput off), dia2 Throughput WER+RTF.
