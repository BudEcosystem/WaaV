# B3 — Multi-Hardware Support Review

**Scope:** Assess WaaV Infer against the hard requirement *"all models work on every hardware"* (vLLM-style multi-backend). READ-ONLY audit of the device/EP abstraction, hardware-pinned assumptions, and the gap to "every hardware."

**Verdict (one line):** The **device abstraction is already first-class and vLLM-shaped** — `EpKind`/`ActiveEp`/`EpRequest` is a pure-data EP enum with 10 ORT execution providers wired behind one trait, device is config-driven per-model with a guaranteed CPU floor, and a model runs CPU↔CUDA by config with **zero code change**. What is *missing* is **reach, not architecture**: only **CUDA + CPU** are live-validated; the other 8 EPs are code-complete but **unproven** (no dylib, no CI, no accuracy gate), there is **no Path-B (candle/ggml) backend at all** (design-only), and a handful of GB10/unified-memory assumptions are **hardcoded** where they should be device-queried.

---

## 1. What hardware works TODAY

| Substrate | Path | Status | Evidence |
|---|---|---|---|
| **NVIDIA CUDA, aarch64 (GB10/sm_121)** | Path A (ORT CUDA EP) | **LIVE, accuracy-gated** | `gb10-env.sh`; `OrtModel` CUDA arm `ep.rs:32-64`; bit-identity gates throughout |
| **CPU (the substrate the floor was sniffed on, i.e. ARM/Grace)** | Path A (ORT CPU EP + CpuTier) | **LIVE, accuracy-gated** | `cpu_tier.rs`; `cpu_tier_output_identical_to_fp32` |
| **CPU x86** | Path A | **Plausible-but-unproven** | Same MLAS CPU EP; AMX-BF16 path referenced but not CI-gated on x86 |
| NVIDIA CUDA x86 | Path A | **Plausible-but-unproven** | Same CUDA EP arm; no x86-CUDA dylib/runner in repo |
| PyTorch sidecar (Path B') | torch_runtime | **LIVE on CUDA + CPU only** | `engine.rs:128-161` maps `ep → "cuda"|"cpu"` only |
| AMD ROCm/MIGraphX, Intel OpenVINO, Apple CoreML, Qualcomm QNN, DirectML, XNNPACK, TensorRT | Path A | **CODE-COMPLETE, UNPROVEN** | All wired in `ep.rs:65-72`; none has a dylib, CI lane, or accuracy gate |
| candle / ggml native AR (Path B) | — | **DOES NOT EXIST** | No candle/ggml/llama-cpp dependency anywhere (design-only, INFER_SPEC §10.1) |

**Bottom line: 2 substrates are real (CUDA-aarch64, CPU). Everything else is code or design, not a validated path.**

---

## 2. The device / EP abstraction — **first-class, not hardcoded** (the good news)

The premise "is it CUDA-or-CPU only, or extensible?" — it is **structurally extensible and already wired for 10 EPs**. The abstraction is genuinely vLLM-platform-shaped:

### 2.1 The pure-data EP enum (the seam)
`crates/waav-infer-backend-api/src/lib.rs:394-412` — `EpKind { Cuda, TensorRt, Rocm, MiGraphX, OpenVino, Qnn, CoreMl, DirectMl, Xnnpack, Hpu }`, `#![forbid(unsafe_code)]`, **no `ort` type leaks** (P-8). `ActiveEp { Cpu, Ep(EpKind) }` (`:472`) with `Cpu` as the guaranteed fallback. `EpRequest { Auto, Cpu, Explicit(EpKind) }` (`:445`) parsed by `parse_ep_request` (`:453`).

### 2.2 The single EP policy point
`crates/waav-infer-backend-ort/src/ep.rs` — *all 10* ORT EPs are mapped in `provider()` (`:30-76`); `auto_probe_order()` (`:79-88`) is platform-sensible (macOS→CoreML, Windows→DirectML→CUDA, Linux→CUDA→MIGraphX→ROCm→XNNPACK). `apply_request` (`:166`) **never fails** — an unavailable accelerator degrades to the CPU floor with `warn!` + `waav_degraded_total` + `waav_ort_ep` telemetry. The gauge label set (`EP_GAUGE_LABELS`, `:22`) is exhaustive over `EpKind` (incl. `hpu`).

### 2.3 Device is config-driven, end-to-end, per-model
- `EngineConfig.ep: EpRequest` (`engine.rs:39`), default `Auto`.
- `OrtGraphLoader { ep }` → `OrtModel::load_ep(path, ep)` (`engine.rs:60-71`). The `GraphLoader` trait (`model.rs:347`) is the injected backend seam; `-core` never names `ort`.
- **Per-sub-graph CPU pin escape hatch:** `load_graph_cpu` (`model.rs:352`) forces the CPU EP for sub-graphs an accelerator mishandles (used for every NeMo `nemo128` mel preprocessor + kokoro — see §3).
- **Can a model run CPU and CUDA by config with no code change? YES.** Same registry arm, same weights, `--ep cpu` vs `--ep cuda`; the only model-side knowledge is the per-input *dtype*, which is **graph-driven** (`runtime/src/precision.rs` `empty_kv_dtype`), so even q4f16-on-CUDA vs fp32-on-CPU needs only a weights swap.

### 2.4 Canonical device notation (user-facing standardization)
`components/src/standardize.rs:179-190` `DEVICE_ALIASES`: `gpu`/`nvidia`/`nv`→`cuda`, `amd`→`rocm`, `metal`/`ane`/`apple`→`coreml`, `intel`→`openvino`, `npu`→`qnn`, `dml`→`directml`, `trt`→`tensorrt`. One canonical token per concept across all models (live-verified per MEMORY).

### 2.5 The capability descriptor + stage placer (the latent platform-abstraction layer)
`backend-api/src/lib.rs` already contains a **substrate-capability model** that is exactly what a vLLM `Platform` would carry:
- `EpCaps { ep, sm_arch, buf: SharedHostBufType, peak_flops, peak_bw, batch_profile }` (`:521`).
- `SharedHostBufType { Coherent, Discrete }` (`:667`) — UMA vs discrete VRAM.
- `BatchProfile { Static1, Tens, Wide }` (`:678`) — fixed-shape NPU vs tens-of-streams vs wide systolic.
- `StagePlacer::place` (`:834`) — a `ggml backend_sched`-style placement decision (manual-pin > capability > follow-weights > affinity), with `supports()` rejecting AR on a `Static1` NPU (HW-40) and `Relay::select` (`:~990`) choosing alias-zero-copy on coherent boundaries vs double-buffered DMA on any discrete end.

**This is the skeleton of the platform layer the task asks for — it already exists as pure data.** The gap is that almost none of it is *fed* by live device enumeration (see §4).

---

## 3. Hardware-pinned assumptions (file:line) — the portability risks

These are the places where "every hardware" is **not** yet honored — pins, hardcodes, and proven-only-on-GB10 paths.

### 3.1 GB10 unified-memory assumptions baked into defaults
| # | Assumption | Location | Risk on other HW |
|---|---|---|---|
| H1 | CUDA arena capped at **48 GiB** of "the 121 GiB unified pool" | `ep.rs:54-57` | A 24 GB discrete GPU: the 48 GiB `gpu_mem_limit` is *above* VRAM → the OOM guardrail is a no-op (clean-OOM-instead-of-crash protection lost). Tunable via `WAAV_ORT_GPU_MEM_LIMIT_BYTES` but the **default is GB10-shaped**, not device-queried. |
| H2 | `kSameAsRequested` arena strategy (`ArenaExtendStrategy::SameAsRequested`) | `ep.rs:62` | Chosen to stop unified-memory fragmentation; on a discrete GPU `kNextPowerOfTwo` (the ORT default) is usually faster. A GB10 fix applied universally. |
| H3 | `EpCaps::cuda()` **hardcodes `buf: Coherent`** + GB10 roofline numbers (`peak_flops 1e15`, `peak_bw 2.73e11`, `Wide`) | `backend-api/src/lib.rs:549-558` | **The single most important wrong default for discrete GPUs.** Every CUDA substrate is assumed unified-memory-coherent, so `Relay::select` will **alias** a producer buffer across a stage boundary that on a discrete GPU is *not* host-coherent → a correctness/perf trap. The `Discrete` machinery exists but is never selected for CUDA. |
| H4 | Unified-memory host-side stacking caps (`RUST_TEST_THREADS=4`, `CARGO_BUILD_JOBS=6`) | `gb10-env.sh:30-47` | Test/build-env only (not runtime), but encodes "GPU load and CPU build draw the same pool" — false on discrete-VRAM hosts. Harmless elsewhere. |
| H5 | `ORT_DYLIB_PATH` + `LD_LIBRARY_PATH` default to GB10 sbsa paths incl. `/usr/local/cuda-13.1/compat` | `gb10-env.sh:14-15` | Machine-specific; documented as "edit for your machine." Not a code pin, but the *only* turnkey recipe is GB10. |

### 3.2 Blackwell-aarch64 / sm_12x forbidden-list pins (these are CORRECT pins, but they are hardware-conditional paths)
| # | Path | Location | Note |
|---|---|---|---|
| H6 | **FlashInfer NEVER on sm_12x** (`SM12X_FORBIDDEN = 120..130`) | `backend-api/src/lib.rs:1601-1622`; gate `:2685` | Correct (prebuilt kernels don't cover GB10). `flashinfer_allowed`/`AttentionBackend::select` is the single source of truth; the native SDPA-pinned path is the universal fallback. Other GPUs (Hopper sm_90, Ada sm_89) would *enable* FlashInfer — but FlashInfer is **never actually invoked** because Path B doesn't exist, so this is policy without a consumer today. |
| H7 | Sparse-KV spec-decode forbidden on sm_12x (approx-attention veto) | `runtime/src/accel.rs:230-260` | Reuses `SM12X_FORBIDDEN`. Off-by-default carve-out; correct. |
| H8 | `mxfp4` Blackwell-only (`sm≥100`), `fp8` Hopper+/TensorRT (`sm≥90`) | `backend-api/src/lib.rs:628-634` | Correct capability gating; the demote ladder is sound. |

### 3.3 TF32-off discipline (correct, but a CUDA-family assumption)
| # | Path | Location | Note |
|---|---|---|---|
| H9 | `with_tf32(false)` default on the CUDA EP (`WAAV_ORT_TF32` opt-in) | `ep.rs:42, 60`; `cfg_batch.rs:13`; `kernel_discipline.rs:16` | Required for batch-invariant fp32 (the AR-compounding identity). Scoped to the CUDA EP. ROCm/other GPUs have **no equivalent knob wired** — if MIGraphX/ROCm has a TF32-analog (it does not, but other accelerators have reduced-precision matmul defaults), the batch-invariance guarantee is **unverified** off CUDA. |

### 3.4 int8-GEMM capability pin (correct, narrowly scoped)
| # | Path | Location | Note |
|---|---|---|---|
| H10 | `forbids_int8_gemm()` → only `Cuda | TensorRt` | `backend-api/src/lib.rs:437-441`; `guard_precision_ep` `lib.rs:165` | Correct & narrow ("ORT-CUDA/TRT can't int8-GEMM from an int8 ONNX graph"). But it is a **CUDA-specific** truth; whether ROCm/OpenVINO/CoreML/QNN can int8-GEMM an int8 ONNX graph is **unmodeled** (they pass the guard by default — possibly a silent per-node CPU fallback on those EPs, untested). |

### 3.5 Correctness CPU-pins (per-op, not whole-model) — a divergence escape hatch, not a portability bug per se
| # | Path | Location | Note |
|---|---|---|---|
| H11 | **kokoro/StyleTTS2 whole-model pinned to CPU** because the duration-predictor LSTM is *numerically divergent* on the GB10/aarch64 ORT-CUDA EP | `model.rs:502-516`; `kokoro_live.rs:49-53` | A model that does **not** work on its nominal accelerator — it works *correctly* only because it was force-pinned to CPU. This is the canonical example of "all models work on every hardware" being satisfied by **falling back to CPU**, not by the accelerator. The pin is unconditional (`load_graph_cpu`), so kokoro is CPU on *every* GPU, not just GB10 — possibly over-broad (the divergence is a GB10-aarch64-CUDA-LSTM finding). |
| H12 | Every NeMo `nemo128`/`nemo80` mel **preprocessor** pinned to CPU | `model.rs:430,439,489,497`; parakeet/canary/cohere/nemotron | Matches the reference engine (STFT/mel on CPU). Portable & intentional — CPU mel is correct everywhere. |

### 3.6 GB10/aarch64 CUDA-EP teardown SIGABRT (a real platform bug worked around by leaking)
| # | Path | Location | Note |
|---|---|---|---|
| H13 | `std::mem::forget(model)` / `process::exit(0)` to skip ORT-CUDA destructors that **SIGABRT on GB10/aarch64 Drop** | `lib.rs:719,826`; `bin/waav_infer.rs:278`; `cascade_live.rs:31`; `server_live.rs:214`; `codec_ar_batcher.rs:525` | A GB10-aarch64-specific driver-teardown race. The leak-on-exit is harmless (process dying) but **accumulates leaked unified memory if multiple such gates run in one binary** (`codec_ar_batcher.rs:525`). On a non-buggy CUDA stack this is unnecessary; it is applied unconditionally. |

### 3.7 Compilation portability — **clean** (no `#[cfg(target_arch)]` landmines)
- The only `cfg!` uses are **runtime string/order selection**, not compile gates: `ep.rs:80-82` (auto-probe order by OS) and `lib.rs:28` (default dylib name by OS). **No `#[cfg(target_arch = ...)]` blocks gate code** — the crate compiles identically on x86/arm/etc.
- `ort` is `default-features = false, features = ["load-dynamic", "api-24", ...]` (`Cargo.toml:43`). **`load-dynamic` is the key portability fact:** the EPs are **NOT compiled into the binary** — they come entirely from whatever `libonnxruntime.so` `ORT_DYLIB_PATH` points at. So "adding ROCm" needs no recompile, just a ROCm-enabled ORT dylib. This is the single biggest reason the abstraction is portable.

---

## 4. What's MISSING for "every hardware"

### 4.1 The EP code is wired but the substrates are unproven (reach gap)
8 of 10 EPs (ROCm, MIGraphX, OpenVINO, QNN, CoreML, DirectML, XNNPACK, TensorRT) have:
- ✅ `provider()` mapping (`ep.rs`), ✅ probe-order entry, ✅ telemetry label, ✅ device-alias.
- ❌ **No ORT dylib** built/shipped for them.
- ❌ **No CI lane** (the `.github/workflows` + `ci/*.sh` are GB10/CPU only).
- ❌ **No accuracy/conformance gate** (INFER_SPEC §10.5 backend-conformance suite is unimplemented).
- ❌ `is_available()` is the *only* runtime gate; a present-but-buggy EP would register and silently mis-compute.

### 4.2 No path for non-CUDA GPUs that *autoregressive* models can use
- AR/codec-LM TTS (chatterbox, orpheus, the lockstep scheduler) runs the AR loop in **host Rust over the `StaticGraph` seam** (`runtime/src/arstep.rs`, the batchers). That is ORT-EP-portable in principle — but the **on-device decode (`run_bound`/IoBinding)** path is implemented only for `CUDA_PINNED` and `HIP_PINNED` (`lib.rs:513-523`); **CoreML/OpenVINO/QNN/DirectML fall back to the host-materialized `run`** (correct but slow, the per-step copy is not eliminated). So AR models "work" on those EPs only in the degraded host-copy regime.
- **Path B (candle Metal / ggml HIP/Vulkan/Hexagon) does not exist** — INFER_SPEC §10.1/§10.3 specs it (candle 0.10 + vendored moshi-core + llama-cpp-2 behind a feature, `GGML_BACKEND_DL`), but **no candle/ggml/llama-cpp dependency is in any `Cargo.toml`**. The "two-path backend" is one path (ORT) plus a CUDA/CPU torch sidecar. The entire ggml-Vulkan-everywhere and Hexagon-HTP road is unbuilt.

### 4.3 Unified-memory assumed where discrete VRAM should be detected
- `EpCaps::cuda()` hardcodes `Coherent` (H3) — the `Discrete` relay (double-buffered DMA, `Relay::AsyncCopyDoubleBuffered`) is implemented and tested but **never selected for a real CUDA substrate**. A discrete NVIDIA GPU (x86 + dGPU) would be mis-modeled as UMA → cross-stage aliasing of non-coherent buffers.
- The arena cap (H1/H2) is GB10-pool-shaped; no live VRAM query feeds `gpu_mem_limit`.
- **There is no live device enumeration.** INFER_SPEC §10.2's `devices() -> Vec<DeviceCaps>` + bandwidth microbench is **unimplemented**; `EpCaps` is hand-constructed with constants, not sniffed. So `StagePlacer`/`Relay`/`batch_knee` all run on **GB10 constants regardless of the actual device**.

### 4.4 Compute-type ladder (§10.4) unimplemented
`resolve_precision` (`backend-api:589`) is the *substrate→precision* resolver and is solid, but the §10.4 `resolve_compute_type(requested, device, model)` that maps an **unsuffixed model tag → device-appropriate artifact row via live device capability** is not present. Today the user picks the weight precision via `waav.json`/`WAAV_PRECISION`; there is no "auto-pick int8 on this NPU, fp16 on this GPU" ladder.

---

## 5. vLLM comparison — what WaaV Infer's platform layer needs

| vLLM `Platform` capability | WaaV Infer today | Gap |
|---|---|---|
| `current_platform` auto-detect (CUDA/ROCm/TPU/CPU/XPU/Neuron) | `auto_probe_order` + ORT `is_available()` probe; no `DeviceCaps` enumeration | **Add `InferBackend::devices() -> Vec<DeviceCaps>` (§10.2) with a bandwidth microbench**; feed `EpCaps` from it instead of constants |
| Per-platform `Attention` backend selection | `AttentionBackend::select` + `flashinfer_allowed` (pure data, no consumer) | Wire it to a real kernel path (needs Path B); today FlashInfer is never invoked |
| Platform-specific memory model (`get_device_total_memory`, UMA vs discrete) | `SharedHostBufType` + `Relay::select` exist; `EpCaps::cuda()` hardcodes `Coherent` (H3) | **Query coherence + VRAM at load**; stop assuming UMA for CUDA |
| Multiple compiled backends (one wheel per platform) | Single binary; EPs via `load-dynamic` dylib swap + (specced) `GGML_BACKEND_DL` | **The `load-dynamic` model is actually *cleaner* than vLLM here** — keep it; add the ggml-DL seam |
| Platform feature gating (`#[cfg]` per platform) | **No `#[cfg(target_arch)]` gates — runtime capability queries instead** | This is the right design (INFER_SPEC §10.2: "capability query, not `#[cfg]`"); preserve it |
| CPU fallback floor | `ActiveEp::Cpu` guaranteed (P-6) + `CpuTier` | ✅ Already done; arguably ahead of vLLM's CPU story |

**The equivalent platform-abstraction layer is ~70% designed and ~40% built.** The pure-data spine (`EpKind`/`EpCaps`/`StagePlacer`/`Relay`/`AttentionBackend`) is the vLLM-`Platform` analogue and is excellent. The missing 30%/60% is the **live device-enumeration feed** + **a second backend (ggml/candle) for non-ORT accelerators** + **the per-EP conformance proof**.

---

## 6. Gap-closure plan for multi-hardware (prioritized)

**P0 — Make the abstraction *true*, not just present (correctness on already-wired HW):**
1. **Live `DeviceCaps` enumeration** (INFER_SPEC §10.2 `devices()` + bandwidth microbench). Feed `EpCaps` (coherence, VRAM, peak BW/FLOPs, batch_profile) from the *real* device. **This single change defuses H1, H2, H3, and unblocks correct `Relay`/`batch_knee`/arena sizing on any GPU.**
2. **Stop hardcoding `Coherent` for CUDA** (H3): query host-coherence; select `Discrete` double-buffered relay on discrete GPUs. Without this, composite P1+P2 DAGs are unsafe on discrete VRAM.
3. **Device-query the arena cap** (H1): default `gpu_mem_limit` from queried free VRAM, not the GB10 48 GiB constant.

**P1 — Prove the wired EPs (reach on existing code, no new backend):**
4. **CPU x86 + CUDA x86 CI lanes** — these need *zero* new code (same EP arms), only a runner + a stock/CUDA ORT dylib. Closes the "x86 is unproven" gap cheaply.
5. **Implement INFER_SPEC §10.5 backend-conformance suite** — op-parity micro-tests + perceptual audio goldens (log-mel distance, never bit-exact cross-backend) per (EP × device-class). An EP that can't pass doesn't ship in that tier (NFR-H).
6. **Bring up ROCm/MIGraphX + OpenVINO + CoreML** one at a time behind a ROCm/OpenVINO/CoreML-enabled ORT dylib; verify the `run_bound` HIP_PINNED path (ROCm) and add a CoreML/OpenVINO pinned-memory arm (or accept the host-copy fallback with a perf note).
7. **Model the int8-GEMM capability per EP** (H10): verify whether ROCm/OpenVINO/CoreML/QNN silently per-node-fall-back an int8 ONNX graph; extend `forbids_int8_gemm` or the precision resolver accordingly.

**P2 — Build the second path (non-ORT accelerators + AR breadth):**
8. **Path B: candle (Metal/CUDA) + ggml-DL (Vulkan/HIP/Hexagon)** per INFER_SPEC §10.1/§10.3 — the `InferBackend`/`LoadedModel(ArStep)` trait the spec defines, behind a cargo feature, using `GGML_BACKEND_DL` for the same dylib-swap portability ORT already enjoys. This is the only route to Apple-Metal AR-TTS, AMD-Vulkan, and the Hexagon NPU.
9. **Compute-type ladder §10.4** — `resolve_compute_type(requested, device, model)` so an unsuffixed model tag picks the device-appropriate artifact (int8 on NPU, fp16 on GPU) via live `DeviceCaps`.

**P3 — Generalize the GB10-specific workarounds:**
10. **Scope the kokoro CPU-pin to the substrate that needs it** (H11): the LSTM divergence is a GB10-aarch64-CUDA finding; on other CUDA/accelerators kokoro may run on-device. Gate the pin on `EpCaps` (active EP × sm_arch), not unconditionally.
11. **Scope the teardown leak** (H13) to the GB10/aarch64 CUDA stack via a runtime check, not an unconditional `mem::forget`.
12. **Verify TF32-analog/batch-invariance on non-CUDA accelerators** (H9) before declaring AR models accurate there.

---

## 7. Answers to the brief

**What hardware works today?** Exactly two validated substrates: **NVIDIA CUDA on aarch64 (GB10/sm_121)** and the **CPU floor** (the ARM/Grace CPU the tier was sniffed/gated on), both Path-A (ORT) and accuracy-gated. A **PyTorch sidecar** (Path B') serves the ~50 non-ONNX models but only with `device ∈ {cuda, cpu}` (`engine.rs:129-132`). x86-CPU and x86-CUDA are the *same code* but unproven. All 8 other EPs are wired but have no dylib, no CI, no accuracy gate. candle/ggml Path-B **does not exist**.

**Is device CUDA-or-CPU-only or extensible?** **Extensible, and already extended in code to 10 EPs.** Device is first-class config (`EpRequest` per-model, `WAAV_ORT_EP`/`--ep`, canonical aliases), the EP mapping is a single policy point (`ep.rs`), and `ort`'s `load-dynamic` means new accelerators need **no recompile** — only a matching ORT dylib. A model runs CPU↔CUDA by config with **zero code change** (dtype is graph-driven).

**Top portability blockers (file:line):**
1. `EpCaps::cuda()` hardcodes `buf: Coherent` + GB10 roofline constants — `backend-api/src/lib.rs:549-558` (mis-models every discrete GPU as unified-memory → unsafe stage aliasing).
2. No live device enumeration — `EpCaps` is hand-built from GB10 constants; INFER_SPEC §10.2 `devices()`/bandwidth-microbench unimplemented (so `StagePlacer`/`Relay`/`batch_knee`/arena all assume GB10).
3. GB10-pool-shaped arena defaults — `ep.rs:54-62` (`48 GiB gpu_mem_limit` + `kSameAsRequested`) applied to every CUDA device.
4. No second backend — zero candle/ggml/llama-cpp deps; Apple-Metal/AMD-Vulkan/Hexagon AR have no path (Path B is design-only).
5. 8 EPs unproven — `ep.rs:65-72` wired but no dylib/CI/conformance gate; `is_available()` is the only guard.
6. kokoro unconditionally CPU-pinned for a GB10-CUDA-LSTM divergence — `model.rs:502-516` ("works" only by CPU fallback, over-broadly).

**What it takes to reach "every hardware":** Not a re-architecture — the device abstraction is sound. It takes (in order): **(P0)** a live `DeviceCaps` feed so the existing pure-data platform layer stops running on GB10 constants and correctly models discrete vs unified memory; **(P1)** CI lanes + the §10.5 conformance suite to *prove* the already-wired EPs (x86-CPU/CUDA are nearly free; then ROCm/OpenVINO/CoreML behind their ORT dylibs); **(P2)** the specced Path-B (candle + ggml-DL) for the non-ORT accelerators (Metal/Vulkan/Hexagon) and AR breadth; **(P3)** scoping the GB10-specific workarounds (kokoro pin, teardown leak, arena, TF32) to the substrates that actually need them.
