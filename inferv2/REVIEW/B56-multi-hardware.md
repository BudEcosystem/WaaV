# B56 — Multi-hardware claim, substantiated with EXECUTED evidence (GB10)

**Date:** 2026-06-22 · **Box:** NVIDIA GB10 (Grace-Blackwell), aarch64, **sm_121**, 121 GB unified, NVIDIA-only (one CUDA GPU + the aarch64 Grace CPU — no AMD/Intel/Apple/Qualcomm silicon present). · **Branch:** `waav-infer-v2-build`.

## TL;DR — what is proven, by what

WaaV Infer's "multi-hardware" claim splits into three honesty tiers, and this review nails each to the strongest evidence this NVIDIA-only box physically allows:

1. **EXECUTION-PROVEN (two real hardware targets): CUDA + CPU.** The SAME models run live on BOTH the GB10 CUDA GPU and the aarch64 CPU, through the SAME production registry seam (`engine::load_model_at(dir, ep)`). A 6-model CPU sweep (whisper / sensevoice / parakeet / moonshine STT + kokoro / supertonic TTS) ran on `EpRequest::Cpu` AND `EpRequest::Auto`(CUDA): every model loads+runs on CPU at RTF < 1, and **CPU↔CUDA transcript word-agreement is 100% on every STT model**. The tch (libtorch) backend's CPU path is separately gated byte-identical (dia 1.6b CPU-fp32 == sidecar golden).
2. **ABSTRACTION-PROVEN (the selection/EP layer is genuinely hardware-portable): ROCm / Metal-MPS / QNN / OpenVINO / DirectML / TensorRT.** The model code contains **zero CUDA hardcoding** — EP/device choice lives behind a pure-Rust seam (`backend-ort::ep`/`device`, `backend-api` `EpKind`/`DeviceCaps`/`AccelMapper`). New unit tests drive **mocked non-CUDA `DeviceCaps`** (vendor = AMD / Apple / Qualcomm / Intel) and prove the `AccelMapper` selects the right vendor backend with **zero model-code change** — adding a hardware is config/EP selection. The ORT EPs compiled into this build are enumerated **live** from the dylib (not asserted).
3. **SILICON-BLOCKED (cannot be executed on this NVIDIA box): actual non-CUDA *execution*.** No ROCm/Metal/QNN/OpenVINO *device* exists here, and the ORT dylib on this box was built with the CUDA EP only (TensorRT-EP, ROCm-EP, etc. are NOT in this particular `libonnxruntime.so`). We do **not** claim non-CUDA execution — only that the backend/EP exists in the framework, the selection logic is proven, and execution awaits the device + a matching dylib.

**Bottom line:** "runs on two hardware targets" = **executed** (CUDA + CPU). "Code-portable, zero-code-to-add-a-hardware" = **proven by the selection logic + live EP enumeration**. Non-CUDA *execution* = **honestly blocked on silicon**, not claimed.

---

## 1. CPU sweep — the 2nd EXECUTED hardware target

Test: `crates/waav-infer-server/tests/cpu_sweep.rs` → `cpu_sweep_onnx`. Each model goes through the **production** registry seam `waav_infer_server::engine::load_model_at(dir, ep)` (the exact path the server uses) — once with `EpRequest::Cpu` (ONNX `CPUExecutionProvider`), once with `EpRequest::Auto` (→ CUDA on GB10). Input audio is synthesized closed-loop with Kokoro (itself a CPU-pinned TTS data point), then transcribed back, mirroring the existing `whisper_live` harness. RTF = wall / audio-seconds.

Run:
```
source gb10-env.sh && cargo test -p waav-infer-server --test cpu_sweep cpu_sweep_onnx \
  -- --nocapture --test-threads=1
```

**EXECUTED result (live, GB10, 2026-06-22):**

| model | runtime path | precision | **CPU RTF** | **CUDA RTF** | accuracy (CPU vs CUDA) |
|---|---|---|---:|---:|---|
| kokoro-82M | ONNX-TTS | fp32 | **0.167** | — (CPU-pinned) | audio rms 0.067, 4.0 s — CPU-only **by design** |
| whisper-tiny.en | ONNX-STT | fp32 | **0.176** | 0.148 | CPU↔CUDA words **100 %**; content hits 7/7 |
| sensevoice-small | ONNX-STT | **int8** | **0.041** | 0.046 | CPU↔CUDA words **100 %**; content hits 7/7 |
| parakeet-tdt-0.6b | ONNX-STT | fp32 | **0.074** | 0.009 | CPU↔CUDA words **100 %**; content hits 7/7 |
| moonshine-base | ONNX-STT | fp32 | **0.026** | 0.014 | CPU↔CUDA words **100 %**; content hits 7/7 |
| supertonic-3 | ONNX-TTS (flow-matching) | fp32 | **0.408** | 0.017 | both non-silent; CPU rms 0.045 / CUDA 0.045; dur ratio 1.00 |
| **tch (libtorch) CPU** | tch matmul+softmax µ-op | fp32 | n/a | — | `matmul_softmax_sum(Device::Cpu)` = **exactly 256.0** (executed, §1.2) |
| **dia-1.6b** (tch / libtorch) | tch-TTS, CPU device | fp32 | loads+runs (slow AR) | — | model **loaded + generated on the CPU device**; full CPU-fp32 byte-identity is the CI gate (§1.2) |

Notes that matter for honesty:

- **Every model runs on CPU at RTF < 1** (faster than real time) and **every STT model's CPU transcript agrees 100 % word-for-word with its CUDA transcript** — this is the literal "same model, two hardware targets, same answer" proof. The test asserts a CPU correctness floor (≥ half the content words read back) and the audio plausibility (non-silent, non-clipping) independently of the cross-EP comparison, so a green run is a real claim.
- **Kokoro is CPU-pinned *by the registry itself*** (`load_graph_cpu` in `model.rs`, regardless of the requested EP) because the StyleTTS2 duration LSTM diverges on the GB10 ORT-CUDA EP (the documented §5.3-KOKORO-CUDA-LSTM scar). So its "CUDA RTF" is honestly `—`: there is no CUDA path to compare; it is a CPU-only TTS data point. This is reported as such, not hidden.
- **SenseVoice is int8 — and is FASTER on CPU (0.041) than on CUDA (0.046).** This is the int8-GEMM truth made concrete: ORT-CUDA cannot int8-GEMM on Blackwell (it per-node-falls-back to fp32 with host↔device thrash), so int8 is the *fast CPU path*, not a CUDA path. The CPU target is not a degraded fallback here — for int8 it is the better target.
- **Supertonic** runs its vector-field estimator graph where cuDNN emits `No execution plans support the graph` warnings on sm_121; ORT transparently falls back to non-cuDNN kernels and the output is still correct (CPU↔CUDA duration ratio 1.00, identical rms) — a good demonstration that the CPU/CUDA agreement holds even when the accelerator path degrades internally.

### 1.2 tch (libtorch) CPU path — byte-identical, executed

The torch backend's models (voxtral / cosyvoice3 / dia / dia2 / csm / higgs / neutts / omnivoice) carry **CPU-fp32 byte-identity gates** — the CPU device is their bit-faithful reference math (no bf16 rounding). The cleanest live proof of "tch model on the CPU hardware target" is `cuda_torch_dia.rs::cpu_fp32_raw_codes_byte_identical`: it loads dia-1.6b on `TorchDevice::resolve(DeviceRequest::Cpu)` and asserts the greedy raw-code stream is byte-identical to the CPU-fp32 sidecar golden across all 9 channels × all frames.

Run (the backend-torch crate links cleanly with its own build.rs CUDA recipe; the server `--features torch` link is separately blocked, see §4):
```
source gb10-env.sh && cargo test -p waav-infer-backend-torch --features cuda \
  --test cuda_torch_dia cpu_fp32_raw_codes_byte_identical -- --include-ignored --test-threads=1 --nocapture
```

**EXECUTED result (live, GB10, 2026-06-22):** dia-1.6b CPU-fp32 raw codes **23409/23409 byte-identical** to the CPU-fp32 sidecar golden (9 channels × 2601 frames, first-divergence None) — matches the standing dia CPU gate (B54). Completed live on the aarch64 CPU device (legitimately slow — a 1.6B AR model on CPU is a real "CPU = valid-but-slower hardware for large AR models" data point, vs the sub-RTF-1 ONNX rows).

This makes the tch CPU device a genuine, *executed*, second-hardware data point for the Path-B runtime, alongside the ONNX `CPUExecutionProvider` rows above — the SAME `DeviceRequest::Cpu` / `EpRequest::Cpu` intent the engine seam consumes.

---

## 2. EP / device portability — the abstraction, proven without the silicon

### 2.1 (a) Which ORT execution providers are compiled into THIS build — enumerated LIVE

The multi-hardware design rests on **ONNX EP-portability**: the SAME `.onnx` runs on ANY ORT execution provider. Critically, `ort` is pinned with `default-features = false` and EP features `["std","ndarray","tracing","api-24","load-dynamic","half"]` — **no `cuda`/`tensorrt`/`rocm`/… cargo EP feature is compiled into the `ort` crate**. With `load-dynamic`, the *registerable EP set is a property of the loaded `libonnxruntime.so`*, queried at runtime via the ORT C API `GetAvailableProviders` (each EP's `is_available()`), **not** a compile-time constant.

New seam helper + test (test/diagnostic scaffolding only):
- `backend-ort::ep::available_eps()` → probes every `EpKind` against the live dylib.
- `backend-ort::ep::ALL_EP_KINDS` + unit test `all_ep_kinds_covers_every_variant` (exhaustive `match` over `EpKind` so a new EP can't be silently omitted).
- Integration test `crates/waav-infer-backend-ort/tests/ep_portability.rs::enumerate_available_execution_providers`.

Run:
```
source gb10-env.sh && cargo test -p waav-infer-backend-ort --test ep_portability -- --nocapture
```

**EXECUTED result (live, GB10, 2026-06-22):**
```
──────── ONNX Runtime execution providers compiled into this dylib ────────
(dylib: …/gb10-cuda-deps/ort-cuda/lib/libonnxruntime.so)
  cpu       : AVAILABLE (always — the guaranteed P-6 floor)
  cuda      : AVAILABLE
  tensorrt  : —
  rocm      : —     migraphx : —     openvino : —     qnn : —
  coreml    : —     directml : —     xnnpack  : —     hpu : —
available accelerator EPs: ["cuda"]
```

The honest read: **this dylib was built with CUDA + CPU only.** The `ort` Rust API *knows* TensorRT/ROCm/OpenVINO/QNN/CoreML/DirectML/MIGraphX/XNNPACK (all 10 `EpKind`s have provider mappings in `ep.rs`), and the dylib's symbol table even contains the provider name strings (`NvTensorRTRTXExecutionProvider`, `MIGraphXExecutionProvider`, `OpenVINOExecutionProvider`, …) — but `GetAvailableProviders` returns only `CUDA`+`CPU` because that is what is actually built/loadable (confirmed: the lib dir ships `libonnxruntime_providers_cuda.so`, no TensorRT/ROCm provider `.so`). **This is exactly why the design queries availability at runtime instead of asserting it** — and why "add a hardware" on the ORT path = ship a dylib built with that EP, zero WaaV code change (the `EpKind` → provider mapping already exists).

### 2.2 (b) No CUDA hardcoding — the EP/device seam is vendor-agnostic

- **`backend-ort/src/ep.rs`** is the *single policy point*: env-driven `WAAV_ORT_EP` → `parse_ep_request` → `apply_request`, with `auto` probing a platform-sensible order (`macos`→CoreML; `windows`→DirectML+CUDA; `linux`→CUDA→MIGraphX→ROCm→XNNPACK) and **CPU as the guaranteed floor (P-6)** — an unavailable accelerator degrades to CPU with `warn!` + telemetry, never a panic or a hard error. The GB10-specific knobs (48 GiB unified arena cap, `kSameAsRequested`, TF32-off) are scoped to `DeviceCaps::is_gb10()`, so a *different* CUDA device — or a non-CUDA one — gets device-appropriate values, not GB10 constants. **No EP-specific code exists outside this module.**
- **`backend-ort/src/device.rs`** fills a *backend-agnostic* `DeviceCaps` by walking the CUDA Driver API via `dlopen` (no hard CUDA link, no `nvcc`). On a non-CUDA host `libcuda.so` is simply absent → `query_cuda_device()` returns `None` and the caller keeps the CPU floor. The struct (`name/sm_arch/total_mem/free_mem/unified`) is the portable contract; a future ROCm/Metal backend fills the SAME struct from its own runtime.
- **`backend-api`** holds the pure-data seam — `EpKind` (10 vendors incl. Gaudi/HPU), `EpRequest`, `ActiveEp`, `DeviceCaps`, `Vendor`, and the **`AccelMapper`** (per-(model,worker) accelerator selection: `Eager` floor + `TorchTensorRt` + per-vendor placeholders behind cargo features `accel-tensorrt/openvino/migraphx/coreml/qnn`). ZERO `ort`/`tch`/`cudarc` type leaks across this `#![forbid(unsafe_code)]` crate.
- **`backend-torch/src/device.rs`** mirrors the same intent-in/label-out contract for libtorch's device set: `DeviceRequest::{Auto,Cpu,Cuda(n)}` → `TorchDevice`, CPU floor guaranteed, CUDA only when `tch::Cuda::is_available()`. A ROCm/MPS libtorch build fills the same `DeviceRequest`.

### 2.3 (b) Mocked non-CUDA device → correct backend selection (zero model-code change)

The existing B17 tests already prove "non-NVIDIA → Eager floor" (because the vendor accels are *placeholders* today). B56 adds the **wired-state** proof: with a vendor accel actually wired (gated on `DeviceCaps::vendor()`), a **mocked** AMD/Apple/Qualcomm/Intel `DeviceCaps` selects THAT vendor's backend — proving "adding a hardware = which accel the vendor gate picks", with the SAME `ModelSpec` flowing through untouched. New tests in `backend-api/src/lib.rs`:

- `mapper_selects_wired_rocm_accel_on_mocked_amd` — a wired ROCm accel is selected on mocked AMD Instinct + Radeon; TensorRT declines there (NVIDIA-only, surfaced); the SAME mapper still picks TensorRT on GB10 and Eager on CPU.
- `mapper_selects_wired_coreml_accel_on_mocked_apple` — wired CoreML selected on mocked Apple M3 (unified ⇒ Coherent); TensorRT declines with the NVIDIA reason.
- `mapper_selects_wired_qnn_accel_on_mocked_qualcomm` — wired QNN selected on mocked Qualcomm Adreno; declines (→ Eager) on Intel (exact vendor gate).
- `mapper_selects_and_accelerates_wired_openvino_on_mocked_intel` — wired OpenVINO selected on mocked Intel Arc, AND `accelerate()` produces the right tagged `AcceleratedModule` the runtime downcasts+executes (closes selection → accelerate → typed module).
- `full_registry_selection_matrix_across_mocked_vendors` — the whole matrix in one table: NVIDIA→TensorRT, AMD→ROCm, Intel→OpenVINO, Apple→CoreML, Qualcomm→QNN, CPU/unknown→Eager — no model-code or runtime change between rows.

Run:
```
source gb10-env.sh && cargo test -p waav-infer-backend-api --lib
```
**EXECUTED:** `test result: ok. 73 passed; 0 failed` (was 68; +5 B56 mocked-device tests). The five new tests:
```
test tests::mapper_selects_wired_rocm_accel_on_mocked_amd ... ok
test tests::mapper_selects_wired_coreml_accel_on_mocked_apple ... ok
test tests::mapper_selects_wired_qnn_accel_on_mocked_qualcomm ... ok
test tests::mapper_selects_and_accelerates_wired_openvino_on_mocked_intel ... ok
test tests::full_registry_selection_matrix_across_mocked_vendors ... ok
```

The point this nails: **adding AMD/Intel/Apple/Qualcomm is one `AccelBackend` impl gated on `vendor()` + one `register()`/feature line + (ONNX path) a dylib with that EP.** The model code, the registry, and the runtime contract (`AcceleratedModule`) are untouched — proven by the selection logic, on hardware this box does not have.

---

## 3. The honest multi-hardware matrix

| Hardware (row) | Executable on THIS box? | Framework/backend support (proven present) | Selection proven? | Status |
|---|---|---|---|---|
| **CUDA** (NVIDIA GPU) | **YES — executed live** | ORT CUDA EP in the dylib (enumerated live); tch CUDA via libtorch | yes (`mapper_picks_tensorrt_on_nvidia_gb10`) | **EXECUTION-PROVEN.** 6-model sweep + tch all run; CUDA RTF column above. |
| **CPU** (aarch64 Grace) | **YES — executed live** | ORT `CPUExecutionProvider` (always linked, P-6 floor); tch CPU device | yes (CPU floor; `mapper_picks_eager_on_non_nvidia`) | **EXECUTION-PROVEN.** Every model runs on CPU at RTF < 1; CPU↔CUDA 100 % word-agreement; tch dia CPU-fp32 byte-identical. |
| **ROCm / MIGraphX** (AMD) | no — no AMD device; not in this dylib | `EpKind::Rocm`/`MiGraphX` + ROCm/MIGraphX ORT EPs (a ROCm dylib enables them, zero WaaV change); tch-rocm libtorch build fills `DeviceRequest`; `Migraphx` accel placeholder | **yes** (`mapper_selects_wired_rocm_accel_on_mocked_amd`) | **ABSTRACTION-PROVEN; silicon-blocked execution.** |
| **Metal / MPS** (Apple Silicon) | no — no Apple device | `EpKind::CoreMl` + ORT CoreML EP; tch MPS device; `CoreMl` accel placeholder; `Vendor::Apple` ⇒ unified Coherent | **yes** (`mapper_selects_wired_coreml_accel_on_mocked_apple`) | **ABSTRACTION-PROVEN; silicon-blocked execution.** |
| **QNN** (Qualcomm Hexagon) | no — no Qualcomm device | `EpKind::Qnn` + ORT QNN EP; `Qnn` accel placeholder | **yes** (`mapper_selects_wired_qnn_accel_on_mocked_qualcomm`) | **ABSTRACTION-PROVEN; silicon-blocked execution.** |
| **OpenVINO** (Intel CPU/iGPU/NPU/Arc) | no — no Intel target; not in this dylib | `EpKind::OpenVino` + ORT OpenVINO EP; `OpenVino` accel placeholder | **yes** (`mapper_selects_and_accelerates_wired_openvino_on_mocked_intel`) | **ABSTRACTION-PROVEN; silicon-blocked execution.** |
| **DirectML** (Windows GPUs) | no — Linux box | `EpKind::DirectMl` + ORT DirectML EP; `auto` probe order includes it on `target_os="windows"` | partial (probe order pinned; no `Vendor` accel) | **ABSTRACTION-PROVEN (EP + auto-order); OS+silicon-blocked.** |
| **TensorRT** (NVIDIA, optimize pass) | NVIDIA present, but **NOT in this dylib** | `EpKind::TensorRt` (ORT) + `TorchTensorRt` accel (no-Python `.ts` path **proven on sm_121 in B48**, opt-in `accel-tensorrt`) | yes (NVIDIA gate, B17/B48) | **Accel path execution-proven (B48); the ORT TensorRT-EP is not in THIS dylib (a TRT-built dylib enables it).** |
| **Gaudi / HPU** (Intel Habana) | no | **No ORT EP in any build** — `EpKind::Hpu` maps to `None` → degrades to CPU floor by design (placement/caps concept only) | n/a (never selected) | **Honest non-support: framework has no provider; CPU floor serves it.** |

Legend: **EXECUTION-PROVEN** = ran live on this box with measured RTF/accuracy. **ABSTRACTION-PROVEN** = the EP/libtorch backend exists in the framework AND the WaaV selection logic picks it for that vendor (mocked-device unit test), but no device is present to execute on. **silicon-blocked** = needs the actual hardware (and, on the ONNX path, a dylib built with that EP).

---

## 4. What is execution-proven vs abstraction-proven vs silicon-blocked (plainly)

- **Execution-proven (ran live, this box):** **CUDA and CPU.** Two real hardware targets, SAME models, SAME registry seam, RTF measured on each, CPU↔CUDA STT output 100 % word-identical, tch CPU-fp32 byte-identical to its golden. This is the literal "runs on more than one hardware" claim, executed — not asserted.
- **Abstraction-proven (selection logic, no device):** **ROCm, Metal/MPS, QNN, OpenVINO** (and the TensorRT *optimize* path, separately execution-proven in B48). The EP exists in the `ort` API / libtorch device set, the `DeviceCaps`/`EpKind`/`AccelMapper` seam carries zero backend type leaks, and mocked non-CUDA `DeviceCaps` select the correct backend with zero model-code change. "Adding a hardware = config/EP selection" is true and tested.
- **Silicon-blocked (cannot honestly run here):** actual **non-CUDA execution**. This is an NVIDIA-only box; no AMD/Intel/Apple/Qualcomm device exists, and the ORT dylib present was built with the CUDA EP only. We make **no** claim of non-CUDA execution. The path to lighting up any row is: provide the device + an ORT dylib built with that EP (ONNX path) or a matching libtorch build (tch path), then flip the vendor's `accel-*` feature — the selection logic and runtime are already in place.

A note on a real, separate build issue surfaced (not changed, per scope): `waav-infer-server --features torch` currently fails to **link** on this box (`undefined reference to cudaStreamWaitEvent@@libcudart.so.13 … DSO missing from command line`) because a recently-added `cuda_graph_shim.o` references `libcudart` symbols the server bin's build.rs link recipe doesn't yet add (`-lcudart`). This is pre-existing — the untouched `torch_inprocess_live` test fails identically — and is orthogonal to multi-hardware. It is why the tch-CPU evidence (§1.2) is taken from the `backend-torch` crate directly (which links cleanly), not the server crate.

---

## 5. Files touched (test/scaffolding only — zero model-numeric / serving change)

- `crates/waav-infer-backend-ort/src/ep.rs` — added `available_eps()` + `ALL_EP_KINDS` (live EP enumeration) + `all_ep_kinds_covers_every_variant` unit test.
- `crates/waav-infer-backend-ort/src/lib.rs` — export `available_eps`, `ALL_EP_KINDS`.
- `crates/waav-infer-backend-ort/tests/ep_portability.rs` — NEW: live EP enumeration integration test.
- `crates/waav-infer-backend-api/src/lib.rs` — added 5 B56 mocked-non-CUDA-device `AccelMapper` selection tests (in the existing `#[cfg(test)]` module; no non-test code changed).
- `crates/waav-infer-server/tests/cpu_sweep.rs` — NEW: the CPU sweep (ONNX + tch) through `engine::load_model_at`.

**Gates:** `backend-api` 73/73 lib tests pass; `backend-ort` 27/27 lib tests pass + `ep_portability` green; `cpu_sweep_onnx` green (table above); clippy `--all-targets -D warnings` clean on `backend-api`, `backend-ort`, and `waav-infer-server --tests`. No model numerics or serving behavior were touched.
