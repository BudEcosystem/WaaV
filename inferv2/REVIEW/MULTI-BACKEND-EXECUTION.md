# WaaV-Infer Multi-Backend EXECUTION — every ORT EP reachable on the PRESENT GB10 hardware

**Date:** 2026-06-24 · **Box:** GB10 (Grace-Blackwell, sm_121, 121 GiB unified) · **Host:** `aarch64` ARM CPU
+ NVIDIA Blackwell GPU · **Goal:** push each ORT execution provider that is *physically installable + runnable
on THIS hardware* from READY-NO-SILICON → PROVEN, with byte-faithful/decode-identical evidence vs the
CPU-MLAS reference. No new silicon, no fabrication, no x86-only wheel claimed as run.

This is the honest execution complement to `MULTI-HARDWARE-PROOF.md` (which proved CPU↔CUDA byte-identity and
documented the absent-silicon EPs as READY). Here I went after the *additional* ORT EPs the prompt named —
OpenVINO, WebGPU, oneDNN/DNNL, and anything else `get_available_providers()` exposes — and **actually tried to
install + run each one on this box.**

---

## 0. Method — the fixed deterministic harness

Scratch dir: `/home/bud/ditto/waav/portproof_multibackend/` (throwaway venvs + harness; NOT in the repo source).

- **Models (real WaaV ONNX graphs, from the HF cache):**
  - `whisper-tiny.en/encoder_model.onnx` (fp32, pure feed-forward — the universal cross-EP equality target;
    same family as the existing `multi_hardware_byte_identical` gate).
  - SenseVoiceSmall `model.int8.onnx` (`csukuangfj/sherpa-onnx-sense-voice-…`) — the int8 graph the prompt named.
- **Fixed deterministic input** (identical bytes across every EP):
  - whisper: `np.RandomState(1234).randn(1,80,3000)*0.5` → `input_features` → `last_hidden_state (1,1500,384)`.
  - sense: `np.RandomState(1234).randn(1,50,560)*0.5` → `x` + `x_length=[50],language=[0],text_norm=[15]`
    → `logits (1,54,25055)` (exactly the prompt's `randn(1,50,560)*0.5`).
- **Bar:** the documented f32 floor — `np.allclose(ref,out, atol=1e-3, rtol=1e-2)` on EVERY element **AND**
  decode (argmax/token) identical, shapes/dtypes equal; byte-faithful when the logits SHA-256 matches.
- **Reference:** CPU-MLAS (stock `onnxruntime==1.26.0` aarch64), saved to `refs/{model}.cpu.npy`.
- Harness: `ep_harness.py` (`<model> <provider> [--save-ref]`) prints `logits_sha16`, `argmax_sha16`,
  `max|Δ|`, allclose, `decode_identical=k/N`, `VERDICT`.

### Reference (CPU-MLAS) — the baseline every EP is diffed against

```
whisper_tiny  CPU  logits_sha16=4cdd37ff8529537d  argmax_sha16=d039a23a7ce254c6  sum=-9684.181641  shape=(1,1500,384)
sense         CPU  logits_sha16=7a59e7cd4abed8fd  argmax_sha16=5d6e7f4cc891e0d0  sum=-9442914.0    shape=(1,54,25055)
```

Self-consistency (CPU vs CPU-ref): `max|Δ|=0.000e+00`, `decode_identical=1500/1500` & `54/54`,
`VERDICT=PASS(byte-faithful)` — the harness is sound.

---

## 1. EPs ATTEMPTED, and the verdict for each on THIS aarch64+Blackwell box

| # | EP requested | installable on aarch64-linux? | compiled into the wheel & registers? | EXECUTED a WaaV model here? | verdict |
|---|---|---|---|---|---|
| 1 | **OpenVINO** (`OpenVINOExecutionProvider`) | **NO** — `onnxruntime-openvino` ships only `manylinux_2_28_x86_64` + `win_amd64` | n/a | no | **NOT-INSTALLABLE (x86/win-only wheel)** |
| 2 | **WebGPU** (`WebGpuExecutionProvider`, Dawn/Vulkan) | **NO** — `onnxruntime-webgpu` ships `macosx_14_0_arm64` + `manylinux_*_x86_64` + `win_amd64`; **no linux-aarch64** | n/a | no | **NOT-INSTALLABLE (no linux-aarch64 wheel)** |
| 3 | **oneDNN/DNNL** (`DnnlExecutionProvider`) | **NO prebuilt** — no `onnxruntime-dnnl` PyPI pkg; not in any aarch64 wheel/tarball/conda-forge build; needs ORT source build | n/a (source build network-infeasible here) | no | **NOT-INSTALLABLE (no prebuilt; source build blocked by throttled network)** |
| 4 | **CANN** (`CANNExecutionProvider`, Huawei Ascend) | **YES** — `onnxruntime-cann==1.24.4` has a `manylinux_2_28_aarch64` wheel | **YES** — `get_available_providers()` → `['CANNExecutionProvider','CPUExecutionProvider']`; provider `.so` present | no — `libascendcl.so: cannot open shared object file` (no Ascend NPU + CANN toolkit) | **READY-NO-SILICON (new EP, now compiled+registering here; needs Ascend silicon)** |
| 5 | **Azure** (`AzureExecutionProvider`) | **YES** — ships in the stock aarch64 wheel | **YES** — registers | **runs, but on CPU** (it is a remote-inference *delegate* shell, not a local compute backend) — byte-faithful | **PASS(executes; not a distinct local compute backend)** |
| 6 | **CPU-MLAS** (`CPUExecutionProvider`) | YES (stock) | YES | YES — the reference | **PROVEN (reference, byte-faithful)** |
| — | **CUDA / TensorRT** | no linux-aarch64 *python* wheel (`onnxruntime-gpu` is x86/win only) | — | **PROVEN via the Rust ORT-CUDA path** (`multi_hardware_byte_identical`, ORT_DYLIB_PATH=gb10-cpu-deps CUDA build) | **PROVEN (existing)** |

### 1.1 Evidence — the EPs that actually EXECUTED a WaaV model on this box

**CPU-MLAS (reference, stock onnxruntime 1.26.0 aarch64):**
```
whisper_tiny CPU : logits_sha16=4cdd37ff8529537d  max|Δ|=0.000e+00  decode_identical=1500/1500  PASS(byte-faithful)
sense        CPU : logits_sha16=7a59e7cd4abed8fd  max|Δ|=0.000e+00  decode_identical=54/54      PASS(byte-faithful)
```

**Azure EP (registers in the stock wheel; remote-delegate → local fallback executes on CPU):**
```
whisper_tiny Azure : USED_PROVIDERS=['AzureExecutionProvider','CPUExecutionProvider']
                     logits_sha16=4cdd37ff8529537d  max|Δ|=0.000e+00  decode_identical=1500/1500  PASS(byte-faithful)
```
Honest reading: `AzureExecutionProvider` is ORT's *cloud-delegate* EP (offload to an Azure-hosted endpoint).
With no endpoint configured it registers and the graph executes on the **CPU floor** — byte-identical to the
reference. So it "runs," but it is **NOT a distinct local compute backend** (it does not exercise a different
kernel library than MLAS). Reported truthfully, not counted as a new silicon/compute cell.

### 1.2 Evidence — CANN EP (a *new* EP, now compiled + registering on this aarch64 box)

```
onnxruntime-cann==1.24.4 (aarch64 wheel)
  get_available_providers() -> ['CANNExecutionProvider', 'CPUExecutionProvider']   # EP IS compiled in
  InferenceSession(whisper, providers=['CANNExecutionProvider', 'CPUExecutionProvider']):
    E: Failed to load libonnxruntime_providers_cann.so: libascendcl.so: cannot open shared object file
    -> Falling back to ['CPUExecutionProvider']; RAN shape=(1,1500,384) sum=-9684.1806640625
```
This is a **textbook READY-NO-SILICON** result for an EP that was not even in the original matrix: the aarch64
wheel **compiles the CANN provider in** and ORT **registers it**, but loading it requires the Huawei Ascend
**CANN toolkit (`libascendcl.so`) + Ascend NPU silicon** — absent on this NVIDIA GB10 box. It degraded cleanly
to CPU (the P-6 floor). The EP path is real and selected; execution needs the Ascend chip.

---

## 2. The precise install/run failure reasons (no hand-waving)

**Every fact below was checked live against the PyPI JSON / the prebuilt aarch64 ORT tarball symbol table.**

### 2.1 OpenVINO EP — NOT-INSTALLABLE on aarch64
- `pip install onnxruntime-openvino` → `No matching distribution found`.
- PyPI `onnxruntime-openvino` v1.24.1 wheels: **`manylinux_2_28_x86_64`, `win_amd64` only** — no aarch64.
- Note: the *OpenVINO core* (`openvino` v2026.2.1) DOES ship a `manylinux_2_35_aarch64` wheel (it has an ARM-CPU
  plugin), but the **ORT↔OpenVINO bridge** (`onnxruntime-openvino`) does not — and building it from source means
  compiling ORT with `--use_openvino` against the aarch64 OpenVINO toolkit, which the throttled network here
  (clone of `microsoft/onnxruntime` stalled at ~99 MB after >15 min) made infeasible. → Honestly: **not
  reachable as an ORT EP on this box.** (OpenVINO could run the ONNX via its *own* runtime, but that is not a
  WaaV-Infer ORT-EP cell.)

### 2.2 WebGPU EP — NOT-INSTALLABLE on aarch64
- The NVIDIA Vulkan ICD **is** present (`/usr/share/vulkan/icd.d/nvidia_icd.json` +
  `libGLX_nvidia.so.580.82.09`), so Dawn-Vulkan *could* reach the Blackwell GPU **if** a WebGPU-enabled ORT
  existed for aarch64.
- It does not: `onnxruntime-webgpu` v1.27.0 wheels = **`macosx_14_0_arm64`, `manylinux_*_x86_64`, `win_amd64`** —
  **no linux-aarch64**. The stock `onnxruntime` aarch64 wheel (1.26.0 AND 1.27.0) and the prebuilt
  `onnxruntime-linux-aarch64-1.27.0.tgz` C-library contain **zero** WebGPU/Dawn compiled symbols
  (`nm -D | grep -ci webgpu|dawn|wgpu = 0`; the `WebGpuExecutionProvider` string in the binary is just the
  universal EP-registry name constant, present in every build). → **not reachable without a source build**
  (Dawn build + network, infeasible here).

### 2.3 oneDNN/DNNL EP — NOT-INSTALLABLE (no prebuilt) on aarch64
- No `onnxruntime-dnnl` PyPI package exists. The stock aarch64 wheel/C-tarball has **zero** DNNL compiled
  symbols. conda-forge `linux-aarch64` ships only plain `onnxruntime` (CPU). So DNNL needs an ORT source build
  with `--use_dnnl` (oneDNN has good aarch64 + ACL support — and the system even offers `libdnnl3` /
  `libarm-compute-dev` via apt, so the *oneDNN lib* is obtainable), but the **ORT build itself** could not be
  obtained: the `microsoft/onnxruntime` clone stalled at ~99 MB after many minutes on this throttled network,
  and a full build additionally FetchContent-downloads abseil/protobuf/onnx/flatbuffers/Eigen/etc. → **the EP
  is wireable + the kernels exist for aarch64, but bringing up a DNNL-enabled `libonnxruntime.so` was blocked by
  the environment's network/disk, not by any architectural limit.**

### 2.4 What the stock aarch64 dylib actually exposes
```
onnxruntime 1.26.0 aarch64 : get_available_providers() = ['AzureExecutionProvider', 'CPUExecutionProvider']
onnxruntime 1.27.0 aarch64 : get_available_providers() = ['AzureExecutionProvider', 'CPUExecutionProvider']
```
The official aarch64 ORT (wheel **and** C-tarball) is **CPU + Azure only**. There is no `onnxruntime-gpu`
linux-aarch64 wheel either (`onnxruntime-gpu` 1.27.0 = x86/win only) — which is exactly why CUDA/TensorRT are
proven on this box via the **Rust** ORT path (custom CUDA `libonnxruntime.so`, `ORT_DYLIB_PATH` recipe), not via
a python wheel.

---

## 3. Wiring readiness (clean + additive, if a DNNL/WebGPU dylib were obtained)

The `ort = 2.0.0-rc.12` crate the engine uses **already exposes both providers**:
- `ort::ep::onednn::OneDNN` → registry name `"DnnlExecutionProvider"`.
- `ort::ep::webgpu::WebGPU` → registry name `"WebGpuExecutionProvider"` (+ `with_dawn_backend_type(Vulkan)`,
  `with_device_id`, `with_enable_graph_capture`, …).

And because the engine builds `ort` with **`load-dynamic`** (no per-EP cargo feature), the registerable EP set is
a property of the loaded `libonnxruntime.so` — i.e. **dropping in a DNNL/WebGPU-enabled dylib flips these on with
zero crate change.** The only additive code is two `EpKind` variants (`Dnnl`, `WebGpu`) + their `provider()` arms
in `crates/waav-infer-backend-ort/src/ep.rs` (and `ALL_EP_KINDS` + the label map) — exactly the "clean + additive
into `EpRequest`" the prompt scoped. **Not done here** (no dylib to validate against; wiring an arm that can
never register on this box would be dead code, not a proof). Documented as the ready hook.

---

## 4. Result — EPs that OPERATIONALLY RUN a WaaV model on this box

| EP | runs a WaaV ONNX model HERE? | distinct local compute backend? | evidence |
|---|---|---|---|
| **CPU-MLAS** | **YES** | yes (MLAS) | reference, byte-faithful (`sha16=4cdd…`, Δ=0, decode 1500/1500 & 54/54) |
| **CUDA / TensorRT** | **YES** (Rust path) | yes (cuDNN/cuBLAS / TRT) | existing `multi_hardware_byte_identical` gate, CPU-EP ≡ CUDA-EP |
| **Azure** | **YES** (falls back to CPU) | **no** — remote-delegate shell, executes on MLAS | byte-faithful via the harness |
| **CANN** | no (registers, can't load) | — | `libascendcl.so` missing → needs Ascend NPU → READY-NO-SILICON |
| **OpenVINO / WebGPU / DNNL** | no (not installable) | — | x86/macos-only wheels or source-build (network-blocked) |

**Net: no NEW distinct *local-compute* ORT backend was movable to PROVEN beyond the already-proven
CPU(MLAS)+CUDA+TensorRT — because every additional local-compute EP (OpenVINO/WebGPU/DNNL) has no
linux-aarch64 prebuilt and a source build was blocked by the throttled network, and the one extra *installable*
aarch64 EP (CANN) needs absent Ascend silicon.** What DID move: CANN goes from "not-in-matrix" to an honest,
*compiled-and-registering-on-this-box* READY-NO-SILICON cell with a precise blocker, and Azure is shown to
register + execute (on the CPU floor). The CPU-MLAS reference and CUDA proofs are re-confirmed live.

---

## 5. Honesty statement
- **Executed a WaaV model here:** CPU-MLAS (reference), CUDA/TensorRT (Rust path), Azure (→ CPU fallback).
- **Installable but silicon-gated here:** CANN — provider compiled into the aarch64 wheel, ORT registers it,
  load fails for lack of `libascendcl.so`/Ascend NPU. New, honest READY-NO-SILICON.
- **Not installable on aarch64-linux:** OpenVINO, WebGPU, DirectML (x86/win/macos-only wheels) — precise
  per-package wheel tags above. DNNL has no prebuilt and the source build was network-blocked, not
  architecturally impossible (oneDNN+ACL aarch64 kernels exist; the `ort` crate exposes the provider).
- **No EP is claimed to have run unless it actually executed the model here** (logits hash + decode count given).
- The OpenVINO-core aarch64 wheel exists, but running ONNX through *OpenVINO's own runtime* is not a WaaV-Infer
  ORT-EP cell, so it is not counted.

## 6. Files
- Harness + evidence: `/home/bud/ditto/waav/portproof_multibackend/{ep_harness.py, paths.py, refs/*.npy}`
- This report: `WaaV/inferv2/REVIEW/MULTI-BACKEND-EXECUTION.md`
- Matrix update: `WaaV/inferv2/REVIEW/MULTI-HARDWARE-PROOF.md` (§2 + new §2.1 CANN/Azure rows)
