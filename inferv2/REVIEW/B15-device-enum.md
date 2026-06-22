# B15 — Live device-enumeration feed (multi-hardware CORRECTness, ONNX-EP side)

**Scope:** Replace the hardcoded GB10 constants in WaaV Infer's device/EP layer with a **LIVE device query**, so the already-first-class device abstraction (`EpCaps`/`StagePlacer`/`Relay`) models the *actual* hardware instead of GB10 numbers. B3 (`REVIEW/B3-multihardware.md`) identified the missing piece precisely: *"the missing piece is the live device-enumeration feed, not the seam."* This closes that feed for the CUDA/ONNX-EP substrate.

**Touched ONLY** `-backend-ort` + `-backend-api` (not `-core/model.rs`, not `-candle`, not `-scheduler`/`-runtime`). **No git commit.**

**Law honored — bit-faithful on GB10:** the live query returns the SAME effective behavior the hardcoded GB10 path did (verified field-for-field + on real CUDA + CPU sessions). The legacy `EpCaps::cuda()` body is **unchanged** (pure additions); the live path is a no-op on a detected GB10.

**Gate (GREEN):**
```
source gb10-env.sh && cargo test -p waav-infer-backend-ort -p waav-infer-backend-api --lib
  → backend-api:  57 passed; 0 failed
  → backend-ort:  26 passed; 0 failed   (incl. live device query + conformance + CUDA bit-faithful)
cargo clippy -p waav-infer-backend-ort -p waav-infer-backend-api --all-targets  → clean (0 warnings)
```

---

## 1. The live `DeviceCaps` query — how it detects unified-vs-discrete + memory

### Seam choice (keeps `-backend-api` backend-agnostic)
- **Typed struct `DeviceCaps{name, sm_arch, total_mem, free_mem, unified}` lives in `-backend-api`** (`crates/waav-infer-backend-api/src/lib.rs`, new, just before `EpCaps`). Pure data, no backend type — so `-core`/`-components`/the placement layer can read it and `EpCaps` can be *fed* by it.
- **The query implementation lives in `-backend-ort`** (`crates/waav-infer-backend-ort/src/device.rs`, NEW), because that crate owns CUDA. Exposed as `pub fn query_cuda_device() -> Option<DeviceCaps>` (re-exported from the crate root).

### Why the CUDA Driver API via `libloading` (not `cudarc`, not ORT, not nvidia-smi)
Evaluated all four routes the task offered:
- **ORT exposes no device-property query** — its `memory` module has only `AllocationDevice` markers + `MemoryInfo`; it cannot report total memory or compute capability (confirmed by reading `ort-2.0.0-rc.12/src/memory.rs`). So the runtime that owns CUDA must ask CUDA directly.
- **`cudarc` IS in the workspace lockfile (0.17.8/0.19.8, candle's dep) but adding it to `-backend-ort` pulls a build-time CUDA link** (and the task forbids touching the candle crate). Heavier than needed for 6 read-only `cuDevice*` calls.
- **`nvidia-smi` reports `[N/A]` for `memory.total`/`memory.free` on GB10** (because it is unified — there's no discrete VRAM partition to report) — so it can't give the arena-sizing number; it would also be a fork+parse.
- **CUDA Driver API via `libloading`** — `libloading` is **already a dependency** of `-backend-ort` (the ORT-dylib pre-flight), and `libcuda.so.1` is already resident on the GB10 runtime (`gb10-env.sh` puts CUDA 13.1 `compat/` first on `LD_LIBRARY_PATH`). A plain `dlopen` of the driver + a handful of stable `cu*` C calls is the **lightest portable seam: no new heavy dependency, no hard CUDA link, no build-time `nvcc`**. The unsafe is confined to `device.rs`.

### What it reads (all stable CUDA Driver ABI)
| Field | CUDA call | Context needed? |
|---|---|---|
| `name` | `cuDeviceGetName` | no |
| `sm_arch` = major·10 + minor | `cuDeviceGetAttribute(COMPUTE_CAPABILITY_MAJOR/MINOR)` (75/76) | no |
| **`unified`** | **`cuDeviceGetAttribute(CU_DEVICE_ATTRIBUTE_INTEGRATED)` (18)** → `==1` | no |
| `total_mem` | `cuDeviceTotalMem_v2` | no |
| `free_mem` (best-effort) | `cuMemGetInfo_v2` via primary-context retain→query→**release-immediately** | yes (transient) |

**Unified-vs-discrete = the CUDA `INTEGRATED` attribute.** This is the canonical CUDA way to distinguish an integrated/coherent-memory GPU (GB10/Grace, Apple Silicon, iGPU — shares the host LPDDR pool) from a discrete PCIe card with its own VRAM. `DeviceCaps::buf_type()` maps `unified ⇒ Coherent`, `!unified ⇒ Discrete`. A query failure conservatively assumes **discrete** (the safe choice: a `Discrete` relay DMAs and is always correct; a wrong `Coherent` alias of non-coherent VRAM is not).

### Live-verified on GB10 (this box)
A standalone C prototype + the in-tree test both confirm:
```
name='NVIDIA GB10'  sm=121  totalMem=130602405888 (121.6 GiB)  INTEGRATED=1  → unified=true, buf=Coherent
```
The test `device::tests::live_cuda_device_query_on_gb10` asserts this shape (and skips cleanly with `None` on a non-CUDA host — never a fabricated claim). The query is **cached process-wide** (`OnceLock`) so a model loaded later sees the same substrate model the first did, and `cuInit` runs once.

### Side-effect safety (GB10 teardown is fragile)
The name/arch/total/`integrated` queries are **context-free**. Free memory needs a context, so it is best-effort via a primary-context **retain → query → release-immediately** — leaving no resident CUDA context behind (verified: CUDA sessions load + run bit-faithfully *after* the query in the same process; back-to-back CUDA tests don't wedge the GPU). This matters because GB10's CUDA-EP teardown already SIGABRTs on Drop (the `mem::forget`-on-exit scar) — the probe deliberately doesn't add a second context to the teardown.

---

## 2. What's now DEVICE-DRIVEN vs still GB10-pinned

### Now device-driven (was hardcoded)
| Was (GB10 constant) | Now (device-driven) | Where |
|---|---|---|
| `EpCaps::cuda()` stamped **`buf: Coherent`** on EVERY CUDA device (B3 **H3**, "the single most important wrong default") | `EpCaps::cuda_from_device(&DeviceCaps)` → `buf = dev.buf_type()` = **`Discrete` on a discrete dGPU**, `Coherent` only when the device reports unified. `relay_for` then picks `AsyncCopyDoubleBuffered` (DMA) on a discrete end, never an unsafe alias. | `backend-api/src/lib.rs` |
| `peak_flops 1e15` / `peak_bw 2.73e11` / `Wide` for EVERY CUDA device | **per-arch `cuda_roofline(sm_arch)` table** (GB10 / B200 / Hopper / Ada / Ampere rows; unknown → conservative GB10-class `Wide` default, never a tiny mis-clamp) feeding `peak_flops`/`peak_bw`/`batch_profile`. | `backend-api/src/lib.rs` |
| Fixed **48 GiB** `gpu_mem_limit` arena cap on EVERY CUDA device (B3 **H1**; a no-op above a 24 GB card's VRAM) | `cuda_arena_limit_bytes(dev)`: explicit `WAAV_ORT_GPU_MEM_LIMIT_BYTES` wins; else **detected-GB10 → 48 GiB** (proven); else **non-GB10 → 90% × real `total_mem`** (a 24 GB card → ~21.6 GiB, so the guardrail actually bounds the device); else (unqueryable) → legacy 48 GiB (no regression). | `backend-ort/src/ep.rs` |
| `kSameAsRequested` arena strategy on EVERY CUDA device (B3 **H2**) | applied **only on a detected GB10** (the unified-pool anti-fragmentation fix); a non-GB10 GPU keeps ORT's default `kNextPowerOfTwo` (usually faster on discrete). | `backend-ort/src/ep.rs` |
| `with_tf32(false)` default on EVERY CUDA device (B3 **H9**) | TF32-off **default scoped to a detected GB10** (the batch-invariance scar is GB10/Blackwell-aarch64); other arches get ORT's default. `WAAV_ORT_TF32` still overrides explicitly on **any** device. | `backend-ort/src/ep.rs` |

`sm_arch` now flows from the **real device** into the fp8/mxfp4 capability gates and the `flashinfer_allowed` sm_12x forbidden-list — so those judge the actual hardware (test: a discrete sm_80 is `flashinfer_allowed`, GB10/sm_121 stays forbidden).

### Still GB10-pinned (correctly — or out of scope)
- The **`SM12X_FORBIDDEN` band / `flashinfer_allowed`** is a *correct* hardware-conditional pin (B3 H6) — it's keyed on `sm_arch`, which is now device-fed, so it's already multi-hardware-correct (no consumer yet, Path B is design-only).
- **The detected-GB10 predicate `is_gb10()` = `sm_121 && unified`** scopes H1/H2/H9 to exactly the substrate that needs them. A non-GB10 CUDA device now gets device-appropriate values for all three. (B3 §6 P3.)
- **NOT touched (other efforts / hardware-gated):** the kokoro CPU-pin (B3 H11, lives in `core/model.rs`), the teardown `mem::forget` leak (H13), the int8-GEMM-per-EP model for ROCm/OpenVINO/etc. (H10 — the *CUDA* truth is correct and now reported by the conformance harness). `free_mem` is queried but the existing `slot_cap`/VRAM-accountant wiring that would consume it lives in `-scheduler` (out of scope here) — `total_mem` is what the arena cap needs and that is fully wired.

---

## 3. Which EPs are code-correct-but-UNPROVEN vs LIVE-VERIFIED

**Honest constraint:** this box is GB10 (CUDA-aarch64) + CPU only. ROCm/OpenVINO/CoreML/QNN/Metal/discrete-NVIDIA cannot be proven here — that needs the hardware. The CODE is made correct + portable; the claim is not faked.

| Substrate / path | Status |
|---|---|
| **CPU floor** | **LIVE-VERIFIED** — conformance harness proves the reference matmul+preprocessor output exact + int8-GEMM-capable. |
| **CUDA-aarch64 / GB10 (sm_121, unified)** | **LIVE-VERIFIED** — live `DeviceCaps` query returns the real device; `EpCaps::cuda_from_device(GB10)` is **bit-identical** to the legacy `EpCaps::cuda(121)`; the CUDA session output is bit-faithful with the new device-driven arena sizing; conformance reports CUDA cannot int8-GEMM. |
| **The `Discrete` path (`unified == false` → `Discrete` buf → double-buffered DMA relay)** | **CODE-CORRECT, hardware-UNPROVEN** — this box has no discrete GPU. Proven *as data*: `cuda_from_device(discrete_a100)` → `Discrete`, the per-arch roofline row, sm_80 flowing into the gates, and `relay_for(Discrete, _) == AsyncCopyDoubleBuffered`. The first real dGPU (x86 + dGPU) is the live proof point. |
| **Non-GB10 CUDA arena/strategy/TF32 scoping** | **CODE-CORRECT, hardware-UNPROVEN** — exercised via unit tests on hand-built `DeviceCaps`; needs a non-GB10 CUDA device to live-verify the 90%-of-total cap + `kNextPowerOfTwo` default. |
| **ROCm / MIGraphX / OpenVINO / CoreML / QNN / DirectML / XNNPACK / TensorRT** | **WIRED but UNPROVEN** (unchanged from B3) — no dylib, no CI. The conformance harness (§4) is the EP-agnostic scaffold they plug into the day a dylib exists; the device query is CUDA-specific (a future ROCm/Metal backend fills the same `DeviceCaps` from its own runtime). |

---

## 4. The conformance harness (the "prove any wired EP" scaffold)

`crates/waav-infer-backend-ort/src/lib.rs`, new `mod conformance`, test `active_ep_conformance` (INFER_SPEC §10.5 scaffold, B3 §6 P1.5).

For the **active** EP it:
1. **Asserts the reference compute is correct on real hardware** — runs `tiny_add.onnx` (`y = x·2 + 1`, a **MatMul (GEMM) followed by an Add (the pointwise preprocessor op)** in one graph) and asserts the exact output `[3,5,7,9]`. The floor IS the reference; an accelerator must be bit-faithful to it.
2. **Reports int8-GEMM capability** of that EP (`!EpKind::forbids_int8_gemm()`; CPU floor = true via MLAS).
3. **Reports the live `DeviceCaps`** (name/sm_arch/unified/total/buf).

It is **parameterized over the EP** (`probe(EpRequest)`), so the SAME assertions validate CPU and CUDA today and are the drop-in for ROCm/OpenVINO/CoreML/… — adding one is a single `probe(...)` line behind its dylib; an EP that can't reproduce the reference output fails the gate. An EP that doesn't load/activate is skipped cleanly (no false failure on a CI host lacking that accelerator).

**Live output on this box:**
```
CONFORMANCE cpu:    EpConformance { active: Cpu,      int8_gemm_capable: true,  matmul_ok: true }
CONFORMANCE device: name="NVIDIA GB10" sm_arch=Some(121) unified=true total_gib=121.6 buf=Coherent
CONFORMANCE cuda:   EpConformance { active: Ep(Cuda), int8_gemm_capable: false, matmul_ok: true }
```

---

## 5. Files

- `crates/waav-infer-backend-api/src/lib.rs` — `DeviceCaps` (+ `buf_type`/`is_gb10`), `GB10_SM_ARCH`, `cuda_roofline()` per-arch table, `EpCaps::cuda_from_device()` (legacy `cuda()` untouched), 4 unit tests.
- `crates/waav-infer-backend-ort/src/device.rs` — **NEW**: the live CUDA Driver query via `libloading`; `query_cuda_device() -> Option<DeviceCaps>`; live GB10 test.
- `crates/waav-infer-backend-ort/src/ep.rs` — `cuda_arena_limit_bytes()` device-sized arena cap; CUDA arm now calls `query_cuda_device()` and scopes the 48 GiB cap / `kSameAsRequested` / TF32-off to a detected GB10.
- `crates/waav-infer-backend-ort/src/lib.rs` — `pub mod device` + re-export; the `conformance` harness module.
