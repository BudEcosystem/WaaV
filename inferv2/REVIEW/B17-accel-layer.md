# B17 — Pluggable Hardware-Acceleration Layer (built)

Implements `REVIEW/ACCEL_LAYER.md` (Goal D). User directive: *"TensorRT only for NVIDIA CUDA devices,
with a mapper that figures if it's compatible with a given worker; ABSTRACT this layer so for other
hardware we can add such libraries to improve perf."*

**Location:** all in `crates/waav-infer-backend-api/src/lib.rs` (the backend-agnostic, `#![forbid(unsafe_code)]`,
C/C++-free seam crate), alongside the live `DeviceCaps` (B15). `Cargo.toml` gained `anyhow` + `tracing`
(both pure-Rust) and per-vendor `accel-*` features. **No other crate, and nothing in `backend-ort`/`torch`,
was touched.**

---

## 1. The trait + mapper, as built

```rust
// Vendor — derived from DeviceCaps.name (backend-agnostic; no cudarc/ort/SDK type leaks).
pub enum Vendor { Nvidia, Amd, Intel, Apple, Qualcomm, Cpu, Other }
impl DeviceCaps { pub fn vendor(&self) -> Vendor { /* name-substring heuristic, see §2 */ } }

// Compatibility verdict — Incompatible carries a reason that is LOGGED, never silently dropped.
pub enum Compat {
    Compatible { est_speedup: f32 },   // mapper may rank by this (1.0 = the Eager floor)
    Incompatible { reason: String },
}

// Minimal model signature — just enough for a compatibility DECISION (no weights, no backend session).
pub struct ModelSpec { pub arch: String, pub ops: Vec<String>, pub dtype: String }
impl ModelSpec { pub fn ops_trt_legal(&self) -> bool { true /* stub */ } /* + new/with_ops/with_dtype */ }

// Thin handles wrapping an OPAQUE module (Box<dyn Any + Send>) the runtime executes — no backend type
// crosses the seam; an accel downcasts to the concrete type it knows; Eager passes it straight through.
pub struct Acceleratable    { /* module: Box<dyn Any+Send>, arch */ }   // in  → accelerate()
pub struct AcceleratedModule{ /* module, accel: &'static str, est_speedup */ } // out ← accelerate()

// The per-hardware accelerator abstraction.
pub trait AccelBackend: Send + Sync {
    fn name(&self) -> &str;
    fn is_compatible(&self, model: &ModelSpec, dev: &DeviceCaps) -> Compat; // the per-worker probe
    fn accelerate(&self, m: Acceleratable, dev: &DeviceCaps) -> anyhow::Result<AcceleratedModule>;
    fn priority(&self) -> u8;  // higher wins among Compatible
}

// Typed "lib/feature absent" signal — so the caller downcasts it off the anyhow::Error and falls back
// to Eager cleanly (NOT a hard model-load failure).
pub struct AccelUnavailable { pub accel: &'static str, pub reason: String }  // : thiserror::Error

// The mapper: highest-priority Compatible (tie → larger est_speedup), else the always-compatible Eager.
pub struct AccelMapper { backends: Vec<Box<dyn AccelBackend>> }
impl AccelMapper {
    pub fn new(backends) -> Self;          // appends Eager if absent (select is always total)
    pub fn with_features() -> Self;        // Eager + each accel whose cargo feature is on (the default)
    pub fn register(self, b) -> Self;      // "adding accel = one register() line"
    pub fn select<'a>(&'a self, m, dev) -> &'a dyn AccelBackend {
        // for each backend: Compatible → keep best by (priority, est_speedup);
        //                   Incompatible{reason} → tracing::info!(... reason ...)  // NEVER a silent drop
        // returns the best, else the static EAGER_FLOOR (can't actually happen — Eager is always present)
    }
}
```

Accelerator instances (all `AccelBackend` impls in the crate):

| Accelerator    | priority | `is_compatible` today                                              | `accelerate` today |
|----------------|----------|-------------------------------------------------------------------|--------------------|
| **Eager**      | 0        | **always `Compatible{1.0}`** (the floor)                          | pass-through (tags `"eager"`) |
| **TorchTensorRt** | 80    | `Nvidia && trt_supported_sm(sm_arch) && ops_trt_legal` (see §3)  | typed `AccelUnavailable` unless `accel-tensorrt` wired |
| OpenVino       | 70       | `Incompatible` (placeholder — Intel)                              | `AccelUnavailable` |
| Migraphx       | 70       | `Incompatible` (placeholder — AMD/ROCm)                           | `AccelUnavailable` |
| CoreMl         | 70       | `Incompatible` (placeholder — Apple)                              | `AccelUnavailable` |
| Qnn            | 70       | `Incompatible` (placeholder — Qualcomm)                           | `AccelUnavailable` |

Integration point (unchanged from the design): `model load → AccelMapper.select(model, DeviceCaps) →
accelerate() → AcceleratedModule → the tch-rs (or ORT) runtime executes it`.

---

## 2. `DeviceCaps::vendor()` — name-string derivation

Backend-agnostic (reads only `name` + `sm_arch`, both already captured by B15 — no vendor SDK):

- contains `nvidia`/`geforce`/`tesla`/`quadro` → `Nvidia`
- `amd`/`radeon`/`instinct` → `Amd`
- `intel`/`arc` → `Intel`
- `apple`/`m1`..`m4`/`apple m*` → `Apple`
- `qualcomm`/`adreno`/`hexagon`/`snapdragon` → `Qualcomm`
- else, **split on `sm_arch`**: `None` (no compute capability ⇒ the CPU floor) → `Cpu`; `Some(_)`
  (an accelerator with an unrecognized name) → `Other`.

The `sm_arch` split is the load-bearing bit: it lets a CPU `DeviceCaps` derive to `Cpu` while a future
unknown *GPU* derives to `Other` — both served correctly (the accel that can't run there returns
`Incompatible`; Eager always can).

---

## 3. How TorchTensorRt's NVIDIA-only gate works off `DeviceCaps`

`is_compatible` is the exact AND the design specifies, read entirely off the live `DeviceCaps`:

```rust
if dev.vendor() != Vendor::Nvidia
    => Incompatible { "TensorRT requires an NVIDIA CUDA device" }   // (1) the directive's core
if !trt_supported_sm(dev.sm_arch)
    => Incompatible { "TensorRT does not support sm_arch {…} (supported: sm_70..sm_129)" }  // (2)
if !model.ops_trt_legal()
    => Incompatible { "model '{arch}' has ops TensorRT cannot lower" }  // (3) op veto, stubbed true
=> Compatible { est_speedup: 3.0 }
```

- **(1)** `dev.vendor()` derives off `DeviceCaps.name`; on ANY non-NVIDIA worker it short-circuits to
  the literal directive reason. A mixed fleet just doesn't pick it there.
- **(2)** `trt_supported_sm(sm_arch)` = `Some(70..=129)` (Volta→GB10); `None` (no compute capability)
  declines. So GB10 `sm_121` passes, A100 `sm_80` passes, an ancient `sm_50` is refused with the arch
  reason surfaced.
- **(3)** op-legality veto, currently `ModelSpec::ops_trt_legal() == true` (TODO: walk `ops` against the
  torch_tensorrt converter registry when the lib is wired).

`accelerate` documents the real **no-Python** path — `torch_tensorrt` AOT-compile → serialized
TorchScript `.ts` engine → load as a tch/libtorch `CModule` → execute via the same `StaticGraph` seam
(FP8/NVFP4 on Blackwell). Because `torch_tensorrt` is **not installed** (Blackwell/sm_121 needs the NGC
PyTorch container), the actual compile is gated behind the **`accel-tensorrt`** cargo feature:
- feature **off** (default) → returns typed `AccelUnavailable` so the mapper's caller falls back to
  Eager cleanly (the model still runs CUDA-eager);
- feature **on** → the real-compile arm lives here; until the NGC-container compile call is wired it
  still declines honestly with `AccelUnavailable` (never a panic).

---

## 4. How a new hardware accelerator plugs in (exact steps)

Adding ROCm/Intel/Apple/Qualcomm/any-vendor accel = **one impl + one register line + one feature**, ZERO
runtime/mapper changes (the runtime always receives an `AcceleratedModule`, optimized or eager):

1. **Add a cargo feature** in `crates/waav-infer-backend-api/Cargo.toml` under `[features]`, e.g.
   `accel-rocm = []` (so a build only pulls that vendor's lib).
2. **Write the `AccelBackend` impl** (the OpenVino/Migraphx/CoreMl/Qnn placeholders are copy-paste
   templates): `name()`; `is_compatible()` gating `dev.vendor() == Vendor::Amd` (+ any arch/op check),
   returning `Incompatible{reason}` everywhere else; `accelerate()` doing the vendor compile behind
   `#[cfg(feature = "accel-rocm")]`, returning typed `AccelUnavailable` when the feature/lib is absent;
   `priority()` (70 for a non-TRT vendor accel; higher to outrank TensorRt on shared hardware).
3. **Register it** in `AccelMapper::with_features` behind `#[cfg(feature = "accel-rocm")]`
   `backends.push(Box::new(Rocm))` — or at any call site via `.register(Box::new(Rocm))`.

That's it. `select()` automatically prefers it (by priority) on matching hardware and logs its
`Incompatible` reason elsewhere; un-accelerated hardware still falls to the Eager floor.

---

## 5. Test results

`cargo test -p waav-infer-backend-api` → **67 passed; 0 failed** (11 new accel tests + 56 pre-existing),
identical green under `--all-features` (which exercises the `accel-tensorrt`-on `accelerate` arm). All
GPU-free, driving hand-built `DeviceCaps`. New tests:

- `device_vendor_derivation` — NVIDIA/AMD/Radeon/Intel/Apple/Qualcomm names → right `Vendor`; CPU caps
  (no sm_arch) → `Cpu`; unknown accel (has sm_arch) → `Other`.
- `mapper_picks_tensorrt_on_nvidia_gb10` — GB10 `sm_121` (and discrete A100 `sm_80`) → `torch-tensorrt`.
- `mapper_picks_eager_on_non_nvidia` — AMD/Intel/Apple/Qualcomm/CPU → `eager` (TensorRt declines).
- `mapper_eager_floor_when_nothing_compatible` — a mapper of only-`Incompatible` placeholders still
  deterministically yields `eager` on an NVIDIA box (the auto-appended floor).
- `incompatible_reason_is_surfaced` — TensorRt on AMD returns the exact `"TensorRT requires an NVIDIA
  CUDA device"`; an out-of-band NVIDIA sm surfaces the arch reason.
- `priority_ordering_holds` — a synthetic prio-90 accel beats TensorRt(80) beats Eager(0).
- `eager_passthrough_and_always_compatible` — `Compatible{1.0}` on every device; module in == module
  out (downcast back to the original type), tagged `eager`.
- `tensorrt_accelerate_unavailable_without_feature` — declines with the **typed** `AccelUnavailable`
  (downcast off the `anyhow::Error`), so callers fall back to Eager.
- `trt_supported_sm_band`, `default_registry_is_eager_only_without_features` — band edges + the
  feature-gated registry (Eager-only with no features; `register()` adds one).

**LAW satisfied:**
- `cargo clippy -p waav-infer-backend-api --all-features --all-targets -- -D warnings` → **clean**.
- `cargo clippy -p waav-infer-backend-api --all-targets -- -D warnings` (default) → **clean**;
  default-feature `cargo build` is warning-free (the `unused_mut` in `with_features` is `cfg`-allowed
  only on the no-feature config, where no `push` consumes the `mut`).
- `cargo test -p waav-infer-backend-api` → **green** (and `--all-features`).
- Each per-vendor feature compiles individually (`--no-default-features --features accel-<vendor>` for
  all five → ok), proving "a build only pulls what its hardware needs".
- Scope honored: only `waav-infer-backend-api` (+ its `Cargo.toml`) changed. The pre-existing
  whole-workspace build break is solely `waav-infer-backend-torch` missing libtorch — unrelated to this
  change and out of scope; the libtorch-free consumers (`-ort`, `-core`, `-runtime`) build clean against
  the new crate. No git commit.
