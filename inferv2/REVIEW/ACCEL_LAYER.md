# WaaV Infer — Pluggable Hardware-Acceleration Layer (Goal D)
User directive: "TensorRT only for NVIDIA CUDA devices — with a mapper that figures if it's compatible with a
given worker — but ABSTRACT this layer so for other hardware we can add such libraries to improve their perf."

## The abstraction
An accelerator is a hardware-specific library that compiles/optimizes a model for a device. It is NOT the runtime
(tch-rs/ORT execute); it's an optional *optimize* pass selected per (model, worker) by a compatibility mapper.
The `DeviceCaps` live-query (B15) is exactly the mapper's input.

```rust
// in waav-infer-backend-api (alongside DeviceCaps), backend-agnostic.
pub enum Vendor { Nvidia, Amd, Intel, Apple, Qualcomm, Cpu, Other }

pub enum Compat {
    Compatible { est_speedup: f32 },     // mapper may rank by this
    Incompatible { reason: String },     // logged — never silently dropped
}

/// A per-hardware perf accelerator. Instances plug in per vendor.
pub trait AccelBackend: Send + Sync {
    fn name(&self) -> &str;                       // "torch-tensorrt", "openvino", "eager"…
    /// The MAPPER's per-worker probe: is this accelerator usable for THIS model on THIS device?
    fn is_compatible(&self, model: &ModelSpec, dev: &DeviceCaps) -> Compat;
    /// Compile/optimize → an executable the runtime runs (or pass-through on Eager).
    fn accelerate(&self, m: Acceleratable, dev: &DeviceCaps) -> anyhow::Result<AcceleratedModule>;
    fn priority(&self) -> u8;                      // higher wins when several are compatible
}

/// The compatibility mapper: pick the best compatible accelerator for a (model, worker) pair.
pub struct AccelMapper { backends: Vec<Box<dyn AccelBackend>> }   // registered, priority-ordered
impl AccelMapper {
    pub fn select<'a>(&'a self, m: &ModelSpec, dev: &DeviceCaps) -> &'a dyn AccelBackend {
        self.backends.iter()
            .filter(|b| matches!(b.is_compatible(m, dev), Compat::Compatible{..}))
            .max_by_key(|b| b.priority())
            .map(|b| b.as_ref())
            .unwrap_or(&EAGER)               // Eager is ALWAYS compatible — the floor, never an error
    }
}
```

## Instances (pluggable)
| Accelerator | Vendor gate (`is_compatible`) | `accelerate` | prio |
|---|---|---|---|
| **TorchTensorRt** | `Nvidia` + sm_arch∈TRT-supported + ops TRT-legal | torch_tensorrt AOT → TorchScript `.ts` engine, run via tch/libtorch (no-Python). FP8/NVFP4 on Blackwell. | 80 |
| OpenVino | `Intel` (CPU/GPU/NPU) | OpenVINO IR compile | 70 |
| Migraphx / ROCm | `Amd` | MIGraphX / torch-rocm compile | 70 |
| CoreMl | `Apple` | CoreML / MPSGraph | 70 |
| Qnn | `Qualcomm` | QNN AOT (also the ONNX-QNN edge path) | 70 |
| **Eager** | always `Compatible{1.0}` | pass-through (the tch/ORT eager module) | 0 |

## Why this is right
- **TensorRT stays NVIDIA-only** and is *selected*, never assumed — `is_compatible` returns `Incompatible` on any
  non-NVIDIA worker, so a mixed fleet just doesn't pick it there (the "mapper that figures if it's compatible with a
  given worker" the user asked for).
- **Other hardware plugs in** — adding ROCm/Intel/Apple/Qualcomm accel = one `AccelBackend` impl + a `register()` line;
  zero runtime changes (the runtime always gets an `AcceleratedModule`, optimized or eager).
- **Unifies with ONNX** — ORT's EP selection (CUDA/ROCm/OpenVINO/QNN EP) is the same idea; the `AccelMapper` concept
  spans both paths, fed by the one `DeviceCaps` enumeration (B15) + its conformance harness.
- **Honest fallback** — Eager is the always-compatible floor, so a model on un-accelerated hardware still RUNS
  (correct, just not TRT-fast); an `Incompatible` is logged with its reason, never a silent skip.

## Integration point
Model load → `AccelMapper.select(model, DeviceCaps)` → `accelerate()` → `AcceleratedModule` → the tch-rs (or ORT)
runtime executes it. Lives in `waav-infer-backend-api` (trait + mapper) with per-vendor impls behind cargo features
(`accel-tensorrt`, `accel-openvino`, …) so a build only pulls the libs its target hardware needs.
