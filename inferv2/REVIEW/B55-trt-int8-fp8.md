# B55 — Low-precision (INT8 / FP8 / NVFP4) TensorRT on GB10 sm_121: closing the "int8-CUDA wall"

**Date:** 2026-06-22 · **Box:** NVIDIA GB10 (Grace-Blackwell), aarch64, CUDA 13.0, **sm_121** (compute cap 12.1),
121 GB unified, PyTorch 2.12.0+cu130, torch-tensorrt 2.12.0+cu130, **tensorrt-cu13 10.16.1.11**,
**nvidia-modelopt 0.44.0**.

## TL;DR — the "int8-CUDA is a hardware wall" finding is WRONG; it was an ORT/libtorch limit. TensorRT runs low-precision on sm_121.

B51 (G-2) found int8/uint8 **refused on every EP** through ORT's CUDA EP and the libtorch CPU tier
("CUDA has no int8-GEMM kernel"). **That is an ORT/libtorch limitation, NOT the hardware.** GB10 Blackwell
sm_121 HAS int8/FP8/NVFP4 tensor cores, TensorRT 10.16 exposes them, and this session **ran low-precision
inference on CUDA via TRT end-to-end** — quantized (modelopt PTQ) → compiled (torch-tensorrt) → loaded
NO-PYTHON via `tch::CModule::load` → ran the real neutts AR decode on GB10:

- **INT8 (W8A8): RUNS on CUDA.** Loaded via the no-Python tch runtime, `trt_active=true`, ran the full
  growing-KV AR decode, produced real audio (peak 0.38). **Isolated backbone speedup 2.51× vs eager fp16**
  (vs fp16-TRT's 1.91×); live AR-loop per-step **1.60×**. Real-activation backbone corr **0.973**.
- **NVFP4 (W4A4, Blackwell 4-bit float): RUNS on CUDA.** Same no-Python path, `trt_active=true`, real audio
  (peak 0.40). **Isolated backbone 2.67×**, live per-step **1.59×**, real-activation corr **0.957**. Smallest
  engine (274 MB, 0.29× the fp16 .ts).
- **FP8 (W8A8, E4M3): compiles + LOADS + RUNS on CUDA, but TRT lowers the FP8-ACTIVATION path to NaN.** The
  modelopt fake-quant eager model is CLEAN (corr 0.86) → the NaN is a **TRT 10.16 FP8-activation lowering bug
  on this Qwen2 SDPA graph**, NOT calibration. FP8-**weight-only** (W8A16) is clean but 0.82× (no tensor-core
  benefit without FP8 activations). Isolated W8A8 speedup would be 2.70× — the fastest — if the NaN were fixed.
- **THE WALL IS CLOSED:** TRT 10.16 on sm_121 exposes `BuilderFlag.{INT8, FP8, FP4, INT4}` and
  `platform_has_fast_int8 == True`; modelopt 0.44 supplies the calibrated Q/DQ; the engine runs the
  int8/nvfp4 tensor-core GEMMs on CUDA where ORT/libtorch refused. **Proven, live, measured.**

**Toward 5×? Honest answer: NO — the real number is ~2.5–2.7× isolated backbone (vs fp16-TRT's ~1.9×), and
~1.6× in the live AR loop.** B48 was right that fp16 is ~2×; low precision adds a further ~1.3–1.4× on the
backbone GEMMs — a genuine, measured tensor-core win, but the live AR loop is dominated by the non-backbone
per-frame cost (the f32 lm_head over the full vocab, the sampling chain, the H2D/D2H engine handoffs), which
dilutes the backbone speedup. The 5× would need the WHOLE per-frame path in low precision, not just the
backbone. **The deliverable — low-precision-CUDA WORKS via TRT, the wall is an ORT/libtorch artifact — stands
regardless of the modest live perf.**

---

## 1. THE HEADLINE — TRT 10.16 on sm_121 exposes the low-precision tensor cores (the wall is not the hardware)

Direct probe of the TRT 10.16.1.11 builder on GB10 (`tensorrt.Builder` + flags/dtypes):

```
TRT version: 10.16.1.11
platform_has_fast_fp16: True
platform_has_fast_int8: True            ← the int8 tensor cores ARE present + fast on sm_121
BuilderFlags:  … FP16, BF16, INT8, FP8, FP4, INT4 …   ← every low precision is a real builder flag
DataTypes:     … HALF, BF16, FP8, FP4, E8M0, INT8, INT4 …   ← FP8 + FP4(NVFP4) + E8M0(MX scale) datatypes
```

`torch_tensorrt 2.12` exposes the matching `dtype.{f8, i8, f4}`, and its dynamo path lowers modelopt's Q/DQ
nodes (`torch.ops.tensorrt.quantize_op`) to these. **The B51 G-2 conclusion ("int8 runs on NO EP") is true
*for ORT and the libtorch CPU tier* — both deliberately refuse int8 — but it is NOT a property of sm_121.**
TensorRT is the EP that exposes the int8/FP8/NVFP4 tensor cores, and it ran them live below.

---

## 2. WHAT WAS BUILT — the modelopt-PTQ → TRT → no-Python path (model: neutts Qwen2-0.5B, the B49 target)

The B49/B52/B54 two-stage no-Python path (offline AOT compile → `tch::CModule::load` runtime) is **reused
unchanged** — the runtime is precision-agnostic because the engine's I/O stays fp16 at the seam; the low
precision is INTERNAL to the engine (the int8/fp8/nvfp4 GEMMs). Only the COMPILE adds quantization, and a
precision KNOB selects which `.ts` the runtime loads.

### Stage 1 — modelopt PTQ + low-precision compile (`torch_runtime/trt_compile_neutts.py --precision`)
The B49 `StepDecoder` (the functional KV-explicit Qwen2 decode step) gains:
1. **`nn.Linear` modules** for the q/k/v/o/gate/up/down projections (was functional `F.linear(x, param)`).
   modelopt PTQ quantizes **modules**, not free params — the functional form inserts **0 quantizers**; the
   `nn.Linear` form inserts **504** (weight_quantizer + input_quantizer × 7 linears × 24 layers). Math-identical.
2. **modelopt PTQ** (`mtq.quantize(model, CFG, forward_loop=calibration)`): inserts calibrated fake-quant
   Q/DQ, runs a small calibration loop (24 decode-step samples across the KV-length profile) to collect the
   per-tensor amax (the dynamic range), returns the quantized model. `FP8_DEFAULT_CFG` (E4M3 W8A8) /
   `INT8_DEFAULT_CFG` / `INT8_SMOOTHQUANT_CFG` / `NVFP4_DEFAULT_CFG`.
3. **`export_torch_mode()` + `strict=False` export** — the modelopt quantizer forwards are NOT traceable by
   plain `torch.export` (it raises "found a fake tensor in the exported program constant's list" on the
   quantizer's lifted amax buffer). The fix is modelopt's `export_torch_mode()` context manager (the EXACT
   thing torch-tensorrt itself uses in `_MutableTorchTensorRTModule.py`), which swaps the quantizer forwards
   for the real `torch.ops.tensorrt.quantize_op` aten op that torch-tensorrt's converter lowers to a TRT
   int8/fp8/nvfp4 kernel. **No `enabled_precisions`** — the dtype is carried by the Q/DQ nodes (passing
   `enabled_precisions` asserts under the auto-enabled `use_explicit_typing`).
4. Serializes the engine embedded in a `.ts` (the `tch::CModule::load` artifact), with a dynamic KV-seq
   profile [min 1, opt 700, max 1024] (the B49 growing-context lever, unchanged).

### Stage 2 — the no-Python runtime is REUSED UNCHANGED (`crates/waav-infer-backend-torch/src/trt.rs`)
`TrtStepBackbone::load` + `.step()` (the B49 `CModule::load` + `forward_is`) run the int8/fp8/nvfp4 engine
**with zero changes** — the engine's `(embed, cos, sin, past_k, past_v) → (hidden, new_k, new_v)` fp16 I/O
contract is identical across precisions. This is the proof that **low precision is a drop-in engine swap**:
the AR loop, the build.rs force-link, the KV management — all precision-agnostic.

### The precision KNOB (the wiring — opt-in, default fp16/eager)
`trt::engine_path_with_precision(model_dir, "WAAV_NEUTTS_TRT_TS", "WAAV_NEUTTS_TRT_PRECISION")` resolves the
`.ts`: the explicit `*_TRT_TS` path wins outright; else `<model>/trt/backbone_<precision>.ts` where the
precision is the new `TrtPrecision` enum parsed from `WAAV_NEUTTS_TRT_PRECISION ∈ {fp16, int8, fp8, nvfp4}`
(default **fp16** — the accuracy-preserving B49 engine). neutts' `maybe_load_trt` uses it + logs the precision.
So the engine precision is **selectable at serve time, opt-in, default OFF/eager** — exactly the §5 ask.

---

## 3. THE LIVE MATRIX — per precision, RUN-on-CUDA + speedup + accuracy (all measured on GB10)

All speedups are **vs eager fp16** (the same pristine unquantized fp16 backbone, `eager_ref`). "isolated" =
the per-step backbone microbench in the compile script (100 iters, synthetic KV at opt-len); "live" = the
per-step speedup in the **real Rust AR loop** (`cuda_torch_neutts_trt_lowp.rs`, golden prompt). "real corr" =
the in-process backbone-hidden A/B on REAL prefill KV (`step_hidden_ab`) — the VALID accuracy metric (the
compile-time synthetic-random number is pessimistic, B49 §4).

| precision | RUNS on CUDA (no-Python tch) | NaN? | isolated backbone speedup | live AR per-step | real-activation backbone corr | .ts size | acc-preserving? |
|---|---|---|---:|---:|---:|---:|---|
| **fp16** (B49 baseline) | ✅ `trt_active` | no | **1.91×** | **1.52×** | **0.999964** | 958 MB | ✅ corr>0.999 |
| **int8** W8A8 (SmoothQuant) | ✅ `trt_active` | no | **2.51×** | **1.60×** | **0.973** | 509 MB | ⚠️ lossy lever |
| **nvfp4** W4A4 (Blackwell 4-bit) | ✅ `trt_active` | no | **2.67×** | **1.59×** | **0.957** | 274 MB | ⚠️ lossy lever |
| **fp8** W8A8 (E4M3) | ✅ loads+runs | **YES** | (2.70×) | — | NaN | 482 MB | ❌ TRT NaN (§5) |
| **fp8** weight-only (W8A16) | ✅ | no | 0.82× | — | (synth 0.91) | 487 MB | clean but no speedup |

**The headline cells: int8 and nvfp4 both LOAD + RUN the AR decode on CUDA via the no-Python tch runtime**
(`trt_active=true`, real non-silent audio), with a real tensor-core backbone speedup (2.5–2.7× isolated) over
eager fp16 — the int8-CUDA wall is closed. The live AR-loop win is smaller (~1.6×) because the backbone is
only part of the per-frame cost (§4).

### The live int8 run (verbatim, the wall-closer):
```
neutts TRT: loaded Int8 engine .../trt/backbone_int8.ts (no-Python CModule::load) — AR decode accelerated
B55 loaded neutts (CUDA) …; precision = int8; trt_active = true
B55 BACKBONE accuracy (int8, 1 step, REAL KV+token): hidden corr = 0.973482, max|Δ| = 1.7031, rel = 11.95%
B55 codes (int8): TRT 161 codes, eager 221 codes; agreeing greedy prefix = 1
B55 audio (int8): TRT 76800 samples, peak 0.3834
B55 SPEEDUP (int8): eager 2.604s/RTF 0.589 | TRT 1.187s/RTF 0.369 | per-step ~1.60x vs eager-fp16
```

---

## 4. Why the live win (~1.6×) << the isolated backbone win (~2.5×) — and why NOT 5×

The compile-script microbench runs the **backbone in isolation** (one query row + the cached KV, 100 iters) —
there, the int8/nvfp4 tensor-core GEMMs cut the backbone ~2.5–2.7× vs eager-fp16's cuBLAS. But the **live AR
loop** per frame also runs: the f32 `lm_head` projection over the full ~64k-id vocab, the
repetition-penalty/top-k/top-p/argmax sampling chain, and the H2D/D2H tensor handoffs across the engine
boundary — **none of which the low-precision backbone touches**. So the backbone is a smaller fraction of the
live frame, and its 2.5× shrinks to ~1.6× end-to-end (the SAME dilution B52/B54 documented: higgs isolated
1.48× → live 1.11×; dia isolated 0.98× → live 1.81×).

**The 5× is not reached and would require the whole per-frame path in low precision** (a low-precision lm_head,
fused sampling, in-engine KV) — out of scope here. **B48's "fp16 is ~2×, the 5× needs int8/FP8/NVFP4" is half
right: low precision DOES beat fp16 on the backbone GEMM (2.5× vs 1.9×), but 5× is a whole-pipeline target,
not a backbone-GEMM one.** The honest measured numbers: **isolated backbone ~2.5–2.7×; live AR loop ~1.6×.**

---

## 5. The FP8 blocker — PRECISE: TRT 10.16 lowers the FP8-ACTIVATION path to NaN on this Qwen2 SDPA graph

FP8 (E4M3, W8A8) is the task's first choice (Blackwell-native), and it **compiles, serializes (482 MB),
loads via the no-Python tch runtime, and RUNS on CUDA** — but the engine output is **NaN** (hidden AND
new_k/new_v). The diagnosis is decisive:

- **The modelopt FP8 fake-quant EAGER model is CLEAN** — no NaN, corr **0.8573** vs eager-fp16 (probed
  directly). So FP8 PTQ is numerically viable on this backbone; the calibration/scheme is fine.
- **Only the TRT-LOWERED engine NaNs.** → it is a **TRT 10.16 FP8-activation lowering bug** on this stack
  (GB10 sm_121, torch-tensorrt-dynamo), the direct analog of B52's fused-RMSNorm-zeroes-output and B54's
  no-op-mask-breaks-export TRT-lowering quirks — but here NOT fixed by a spelling change.
- **Bisected by quantization surface:**
  - FP8 **W8A8** (full) → NaN (2.70× would-be speed).
  - FP8 **weight-only (W8A16)** → CLEAN (no NaN, corr 0.909 synthetic) but **0.82×** — fp16 activations can't
    feed the FP8 tensor cores, so it's pure dequant overhead, no speedup.
  - FP8 W8A8 with the **attention (q/k/v/o) activation quant excluded** (MLP-activation-FP8 only) → STILL NaN.
    So the NaN is NOT specific to the attention projections — TRT's FP8-activation lowering NaNs wherever the
    FP8 activations are, on this graph.
- **The runtime correctly CATCHES it** — the B55 e2e test asserts no-NaN and hard-fails for fp8 with
  `"the fp8 TRT engine produced NaN hidden — low-precision lowering is broken for fp8"` (the engine loads,
  `trt_active=true`, then the first step is NaN). An honest hard failure, not a silent pass.

**Bottom line on FP8 today:** Blackwell + TRT 10.16 + modelopt *support* FP8 W8A8 (the flags/datatypes exist,
the engine builds), but TRT 10.16's FP8-activation kernel lowering produces NaN on this Qwen2 decode graph.
**int8 and nvfp4 are the working low-precision-CUDA paths** here; FP8 W8A8 is blocked on a TRT-lowering defect
(likely a newer TRT or a different FP8 recipe — block-scaled MXFP8, or per-channel FP8 — would be the next
thing to try; out of scope for B55's "honest at each step" bar).

---

## 6. The accuracy story — honest: a lossy throughput lever, the AR forks (same as every TRT path)

Low precision is **lossy by design** (the accuracy-preserving bar is fp16-TRT's corr>0.999; low precision is
explicitly NOT that). The real-activation backbone corr is **0.97 (int8) / 0.96 (nvfp4)** with rel max|Δ|
~12–21% — well below fp16-TRT's 0.999964 / 0.55%. Consequences:

- **The AR greedy sequence forks after ~1 code** (vs fp16-TRT's 15) — the larger per-step perturbation flips
  the first borderline argmax immediately, the KV diverges, and the two become *different valid utterances*
  (int8 161 codes, nvfp4 138 codes, eager 221 — all real, non-silent speech). This is the SAME compounding
  B49/B52/B54 documented for fp16-TRT, only stronger (more loss → it forks sooner).
- **SmoothQuant helped int8 modestly** (real corr 0.964 → 0.973). The accuracy ceiling here is the
  **synthetic-random calibration** — real-decode-activation calibration would migrate the true outliers and
  improve it (SmoothQuant on synthetic data has no real outlier structure to migrate; the synthetic corr even
  *dropped* with SmoothQuant — a clean tell that the synthetic metric is pessimistic/unreliable, exactly B49
  §4). The real ground-truth metric is the in-process A/B, which is what the B55 test reports.
- **THE LAW holds:** the byte-identity path stays **eager** (default). With TRT OFF (cfg on +
  `WAAV_NEUTTS_TRT` unset), the neutts byte-identity gate still passes **0/96 codes differ, first-div None**.
  The low-precision path is genuinely opt-in; the byte-identity path is untouched.

So this is a **perf lever for throughput/latency, NOT a byte-identity drop-in** — and a more aggressive one
than fp16-TRT (it forks sooner). Use it where a different-but-valid utterance is acceptable (the throughput
regime), keep eager for byte-identity.

---

## 7. Files changed (ALL within scope — `crates/waav-infer-backend-torch/` + the compile script + a test)

| file | change |
|---|---|
| `torch_runtime/trt_compile_neutts.py` | **EXTENDED** with the B55 low-precision path: `--precision {fp16,fp8,int8,nvfp4}` + `--weight-only` + `--exclude-attn-act` + `--smoothquant`; `nn.Linear` projections (so modelopt can quantize the modules); `quantize_modelopt` (modelopt PTQ via `mtq.quantize` + calibration loop); `calibration_inputs`; the `export_torch_mode()` + `strict=False` export for the quantized graph; a pristine `eager_ref` for honest fp16 accuracy/perf baselining; a quant-eager-vs-TRT diagnostic (splits calibration-blame from TRT-lowering-blame). The fp16 path is byte-for-byte the B49 behavior (regression-verified: corr 0.99989, 1.91×). |
| `crates/waav-infer-backend-torch/src/trt.rs` | **ADDITIVE** `TrtPrecision` enum (Fp16/Int8/Fp8/Nvfp4 + `from_env_value` + `ts_suffix`) + `engine_path_with_precision` (the precision knob: `*_TRT_TS` path override OR `backbone_<precision>.ts` via `WAAV_<MODEL>_TRT_PRECISION`, default fp16) + 2 unit tests. The `TrtStepBackbone` runtime is UNCHANGED — it is precision-agnostic (fp16 I/O contract). |
| `crates/waav-infer-backend-torch/src/neutts.rs` | `maybe_load_trt` now uses `engine_path_with_precision` + logs the resolved `TrtPrecision` — the wiring of the opt-in precision knob. The eager path + the byte-identity gate are untouched. |
| `crates/waav-infer-backend-torch/tests/cuda_torch_neutts_trt_lowp.rs` | **NEW** (`cfg(all(cuda, accel_tensorrt))`) — the B55 low-precision e2e gate: no-Python load of the int8/fp8/nvfp4 engine, the growing-KV AR decode RUN on CUDA, the REAL-activation backbone-accuracy A/B, the speedup vs eager-fp16, and a **no-NaN assertion** (so fp8's TRT NaN is an honest hard failure, not a silent pass). Default precision int8 (the green wall-closer); `WAAV_NEUTTS_TRT_PRECISION` selects the cell. |

**Reused UNCHANGED (precision-agnostic):** `src/trt.rs` `TrtStepBackbone` (the `CModule::load` no-Python
runtime + `step`), `build.rs` (the `--no-as-needed` force-link of `libtorchtrt_runtime.so` + nvinfer under
`cfg(accel_tensorrt)`), `src/nn/kv_cache.rs` `valid_kv()`. The low-precision generalization is ENTIRELY the
compile script + the precision knob + the test — the no-Python AR-loop runtime did not change, which is the
proof that low precision is a drop-in engine swap.

---

## 8. Gates (all green)

- `cargo test -p waav-infer-backend-torch --lib` (DEFAULT, cfg off): **145 passed** — unchanged.
- `cargo test -p waav-infer-backend-torch --lib` (`--cfg accel_tensorrt`): **147 passed** (145 + the 2 new
  `trt::tests` for the precision knob).
- `cargo clippy -p waav-infer-backend-torch --all-targets` (default) **clean**; `… --features cuda -- -D
  warnings` (`--cfg accel_tensorrt`) **clean** (the only "warning" is the build.rs force-link info line).
- **neutts byte-identity gate, TRT OFF** (`cuda_bf16_greedy_codes_byte_identical`, accel cfg, `WAAV_NEUTTS_TRT`
  unset): **0/96 codes differ, first-div None** — THE LAW holds; the low-precision opt-in is genuinely gated.
- **B49 fp16-TRT gate** (`neutts_trt_e2e_accuracy_and_rtf`, precision knob defaults to fp16): still PASSES —
  `loaded Fp16 engine`, backbone corr **0.999964**, per-step 1.52× — the precision wiring didn't break it.
- **B55 int8 e2e** (`neutts_trt_lowp_runs_on_cuda_and_speedup`, default int8): **PASSED** — `trt_active=true`,
  real-activation corr 0.973, per-step 1.60×, non-silent audio.
- **B55 nvfp4 e2e** (`WAAV_NEUTTS_TRT_PRECISION=nvfp4`): **PASSED** — `trt_active=true`, real-activation corr
  0.957, per-step 1.59×, non-silent audio.
- **B55 fp8 e2e** (`WAAV_NEUTTS_TRT_PRECISION=fp8`): **FAILS by design** — engine loads + runs on CUDA
  (`trt_active=true`) but the runtime catches the TRT FP8-activation NaN (the honest hard failure documenting
  the §5 blocker; the green default is int8).

---

## 9. How to reproduce

```bash
# (1) the torch_tensorrt + matching TRT 10.16.1 + modelopt throwaway venv — COMPILE-time only)
VENV=/tmp/trt_e2e_venv
$VENV/bin/pip install --no-deps torch-tensorrt==2.12.0 --extra-index-url https://download.pytorch.org/whl/cu130
$VENV/bin/pip install tensorrt-cu13==10.16.1.11 dllist
$VENV/bin/pip install --no-deps nvidia-modelopt==0.44.0        # + leaves: pulp cppimport pybind11 ninja nvidia-ml-py
TTLIB=$VENV/lib/python3.12/site-packages/torch_tensorrt/lib
TRTLIB=$VENV/lib/python3.12/site-packages/tensorrt_libs

# (2) AOT-compile a LOW-PRECISION engine (free -g first; ONE run at a time — ~480-510 MB each)
source gb10-env.sh; export LD_LIBRARY_PATH="$TTLIB:$TRTLIB:$LD_LIBRARY_PATH"
$VENV/bin/python3 torch_runtime/trt_compile_neutts.py --model-dir ~/.cache/waav-models/neutts-air \
  --out ~/.cache/waav-models/neutts-air/trt/backbone_int8.ts  --precision int8 --smoothquant --max-kv 1024 --opt-kv 700
$VENV/bin/python3 torch_runtime/trt_compile_neutts.py --model-dir ~/.cache/waav-models/neutts-air \
  --out ~/.cache/waav-models/neutts-air/trt/backbone_nvfp4.ts --precision nvfp4 --max-kv 1024 --opt-kv 700

# (3) build with the cfg + run the e2e gate (no Python at serve time); pick the precision via the env knob
export WAAV_TORCHTRT_LIB="$TTLIB" WAAV_TENSORRT_LIB="$TRTLIB" RUSTFLAGS="--cfg accel_tensorrt"
export WAAV_NEUTTS_TRT_PRECISION=int8   # or nvfp4  (fp8 demonstrates the §5 NaN)
cargo test -p waav-infer-backend-torch --features cuda --test cuda_torch_neutts_trt_lowp \
  -- --ignored --nocapture --test-threads=1
```

---

## 10. Honest bottom line

- **The "int8-CUDA is a hardware wall" finding is CLOSED — it was an ORT/libtorch limit, not the hardware.**
  GB10 sm_121 has the int8/FP8/NVFP4 tensor cores, TRT 10.16 exposes them (`platform_has_fast_int8==True`,
  `BuilderFlag.{INT8,FP8,FP4}`), modelopt 0.44 supplies the calibrated Q/DQ, and **int8 + nvfp4 ran the real
  neutts AR decode on CUDA via the no-Python `tch::CModule::load` runtime** — proven, live, measured.
- **Which low precision works on sm_121 via TRT, today:** **INT8 (W8A8)** and **NVFP4 (W4A4)** — both run
  clean on CUDA. **FP8 (W8A8)** compiles + loads + runs but TRT 10.16 lowers its FP8-ACTIVATION path to NaN
  on this Qwen2 SDPA graph (fake-quant is clean → a TRT-lowering bug, not calibration); FP8-weight-only is
  clean but gives no speedup.
- **Speedup (toward 5×?): NO.** Real measured: **isolated backbone ~2.5–2.7× vs eager fp16** (vs fp16-TRT's
  ~1.9×) — a genuine ~1.3–1.4× tensor-core win over fp16; **live AR loop ~1.6×** (the non-backbone per-frame
  cost dilutes it). The 5× is a whole-pipeline target (low-precision lm_head + sampling + KV), not a
  backbone-GEMM one.
- **Accuracy: lossy lever, the AR forks** (real-activation corr 0.97 int8 / 0.96 nvfp4; forks after ~1 code) —
  the same compounding as fp16-TRT, stronger. The byte-identity path stays **eager** (THE LAW: 0/96 with TRT
  OFF). A throughput lever, NOT a byte-identity drop-in.
- **Wired:** the engine precision is the opt-in `WAAV_NEUTTS_TRT_PRECISION` knob (fp16 | int8 | fp8 | nvfp4),
  default fp16/eager, the runtime precision-agnostic (the low precision is a drop-in engine swap — the
  no-Python AR-loop runtime did NOT change). The model-agnostic seam (B49/B52/B54) means this generalizes to
  higgs/dia the same way (write `--precision` into their compile scripts; the runtime + knob are shared).
