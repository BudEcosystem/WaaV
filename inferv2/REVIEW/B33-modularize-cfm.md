# B33 — Modularize the shared diffusion/flow acoustic generator (`cfm/`)

**Phase 3c** of the WaaV Infer Torch-backend modularization (memory `waav-infer-modularize-reuse`; catalog
`COMPONENT_CATALOG.md`). Extract the diffusion/flow acoustic-generator seam into
`crates/waav-infer-backend-torch/src/cfm/` and rewire cosyvoice3 + vibevoice to it, **bit-faithful**.

This was less raw dedup than 3a/3b (only 2 of 7 models touch this family today) — the value is a **discoverable,
reusable diffusion seam** so the flow/diffusion fan-out (omnivoice, Wave-3 variants) plugs into ONE place instead
of re-deriving a CFG denoise loop.

---

## What shipped (the `cfm::` API)

`cfm/` is now a 3-module family + a lightweight discovery marker, all promoted **byte-for-byte** from the two
models:

### `cfm::ode` — the flow-matching integrator (from cosyvoice3)
```rust
pub trait FlowField {
    type Error;                                            // model keeps its own error at the seam
    fn eval(&mut self, x2: &Tensor, t: f32) -> Result<Tensor, Self::Error>;  // [2,C,T] → [2,C,T]
}
pub struct CfmOde { pub n_steps: usize, pub cfg_rate: f64 }
impl CfmOde {
    pub fn new(n_steps: usize, cfg_rate: f64) -> Self;
    pub fn schedule(&self) -> Vec<f64>;                    // cosine: 1 - cos(i/n · π/2)
    pub fn solve<F: FlowField + ?Sized>(&self, x0: &Tensor, field: &mut F) -> Result<Tensor, F::Error>;
}
```
Fixed-step CFG-guided **Euler** solve over a **cosine** schedule. CFG is INTERNAL: CFG-double → `field.eval` →
`(1+w)·cond − w·uncond` → `x += dt·dφ`. The one design change vs the pre-refactor code: `FlowField` gained an
**associated `Error` type** (was an inherent `Res<Tensor>` tied to `CosyVoice3Error`) so a torch-native
flow-field can plug in with its OWN error — zero behavioral change (cosyvoice3's `OnnxFlowField` sets
`type Error = CosyVoice3Error`). `solve`'s in-place `x += …` over a `shallow_clone` (caller deep-copies) is
preserved verbatim.

### `cfm::dpm` — the DPM-Solver++ diffusion stepper (from vibevoice)
```rust
pub struct DpmSolver { pub sigmas: Vec<f32>, pub timesteps: Vec<i64> }
impl DpmSolver {
    pub fn new(n_steps: usize) -> Self;                                          // cosine betas → set_timesteps
    pub fn convert_output(&self, model_output: &Tensor, sample: &Tensor, step_index: usize) -> Tensor;
    pub fn first_order (&self, m0: &Tensor, sample: &Tensor, step_index: usize) -> Tensor;
    pub fn second_order(&self, m0: &Tensor, m1: &Tensor, sample: &Tensor, step_index: usize) -> Tensor;
    pub fn solve<F: FnMut(&Tensor, i64) -> Tensor>(&self, x_init: &Tensor, dt: Kind, eval: F) -> Tensor;
}
```
v-prediction, cosine-beta, multistep (order-2, midpoint) DPM-Solver++. CFG is the CALLER's job (an
`eval(sample,t)→velocity` closure). The headline **byte-identity discipline is preserved**: sigmas held as
**f32**, every coefficient computed in **f32** (step-0 `sigma_s≈20291` amplifies sub-ULP diffs ~20000×), and the
per-step dtype flow (bf16 state → bf16 convert → f32 order-update → re-round to bf16). The localizer methods
(`convert_output`/`first_order`/`second_order`/`timesteps`) are now `pub` so vibevoice's `debug_diffuse_internal`
seam test still drives them.

### `cfm::vocoder` — the HiFT/NSF vocoder (from cosyvoice3)
`HiftVocoder` (`CausalHiFTGenerator`: mel[1,80,T] → 24 kHz wav, NSF source + iSTFTNet) + its tch-op helpers
(`CausalConv1d`/`ResBlock`/`UpsampleBlock`/`SourceDown`, `leaky_relu_slope`/`snake`, `weight_norm_reconstruct`,
`hann_periodic`) + the generator constants. **Moved for discoverability, not dedup** (only cosyvoice3 wires it;
vibevoice has its own streaming-conv acoustic VAE) — the catalog lists `cfm/vocoder.rs`, so the flow family's
mel→wav back-half now sits next to its integrator. The struct fields + constructors are `pub`; cosyvoice3 keeps
ONLY its model-specific **weight loader** (`build_vocoder`/`build_resblock`) which populates them. Every conv pad,
the f64 f0-predictor path, the SineGen2 NSF source, and the STFT/iSTFT are byte-for-byte identical.

### Shared trait — did one fit? **A discovery marker did; a unified `solve` did NOT.**
```rust
pub enum StepperKind { FlowEuler, DpmSolverPlusPlus }
pub trait DiffusionStepper { fn kind(&self) -> StepperKind; }   // impl'd for CfmOde + DpmSolver
```
The two steppers both "iteratively denoise a latent under CFG," but their `solve` signatures **do not unify
without distorting one**:
- `CfmOde::solve` is **fallible** (`Result<_, E>`), owns CFG, and takes a 2-row `FlowField` **trait object**.
- `DpmSolver::solve` is **infallible**, **dtype-aware** (`dt: Kind`), carries **multistep state**, and delegates
  CFG to an `FnMut` **velocity closure**.

Forcing a single `fn solve` would smear the per-family byte-identity contracts each GPU gate depends on (the ODE
combines CFG before the Euler step; the DPM solver re-rounds bf16 per step). So — exactly as the brief permitted
— they stay **two clean modules**, unified only by the zero-cost `DiffusionStepper` marker (a `kind()`
discriminant + family doc in `cfm/mod.rs`). That gives omnivoice/Wave-3 the discovery ("here are the CFG denoise
steppers, pick the recurrence that matches") with **no behavioral coupling**. This is documented in `cfm/mod.rs`.

---

## LOC reduction

| File | before | after | Δ |
|---|---:|---:|---:|
| `cosyvoice3.rs` | 1454 | 1035 | **−419** |
| `vibevoice.rs` | 2114 | 1957 | **−157** |
| **models total** | 3568 | 2992 | **−576** |

`git diff --stat`: **36 insertions, 612 deletions** across the two models (the insertions are pointer comments +
`use` lines). The deleted ~556 lines of duplicated/inlined generator code are now ONE discoverable copy:

| New `cfm/` module | LOC |
|---|---:|
| `cfm/ode.rs` | 80 |
| `cfm/dpm.rs` | 169 |
| `cfm/vocoder.rs` | 390 |
| `cfm/mod.rs` | 73 |
| `cfm/tests.rs` | 168 |
| **total** | **880** (712 prod + 168 test) |

---

## Proofs

### Unit tests (`cfm::tests`, 8/8 green on CPU — the fixed-input Δ==0 gates)
- `cfm_schedule_cosine` — cosine knots match `1−cos(i/n·π/2)`, endpoints {0,1}, `Σdt==1`.
- `cfm_integrator_exact_on_constant_field` — constant flow-field Euler integral == closed form `x0+(1+w)c`
  (Δ < 1e-5). The pre-refactor cosyvoice3 check, now against `cfm::ode`.
- `dpm_solver_timesteps_and_sigmas_match_reference` — `set_timesteps(10)` == `[999,899,…,100]`, `sigma[1]≈6.365`,
  `sigma[9]≈0.171`, `sigma[10]=0`, `sigma[0]∈(20000,20600)` (the amplifier).
- `dpm_convert_output_last_step_is_identity` — v-pred convert at `sigma=0` == sample (**max|Δ|==0**).
- `dpm_first_order_is_pure_and_deterministic` — fixed-input first-order step is finite + reproducible
  (**max|Δ|==0**).
- `dpm_solve_deterministic_on_fixed_field` — full 10-step solve on a fixed field, two runs (**max|Δ|==0**).
- `weight_norm_matches_hand`, `hann_window_periodic` — the promoted vocoder helpers vs hand computation.

Full lib suite: **87 passed, 0 failed**. The duplicate per-model tests were removed from cosyvoice3/vibevoice
(now covered once in `cfm::tests`); each model keeps its own model-local helper tests.

### GPU byte-identity spot-checks (GB10 CUDA, `free -g` checked, run one at a time)
**cosyvoice3** (`--test cuda_torch_cosyvoice3 -- --include-ignored`): **PASS**
- CFM seam (golden tokens → `run_cfm`): mel **max|Δ| = 0.0049** (== the ≤0.0049 bound), RMS(Δ)=0.00023 — the
  pure CUDA-vs-CPU ORT-EP delta, unchanged.
- AR speech-token sequence **BYTE-IDENTICAL** to the sidecar (123 tokens); deterministic flow→vocoder pipeline
  RTF 0.31; e2e RTF 0.51.

**vibevoice** (`--test cuda_torch_vibevoice -- --include-ignored`, 4 tests): **PASS** — every DETERMINISTIC seam
at **max|Δ| = 0.0000000**:
- diffusion head `head_out` = 0; DPM `eps_cfg` = 0; `m0(converted)` = 0; `prev_sample(exact)` = 0 (step 0).
- L3 28-layer backbone on golden embeds = 0; L4 neg-cond = 0; L4 DPM step-0 prev_sample = 0.
- The non-zero later-step speech-state / ref-feat / my-embed deltas are the **known token-sequence-chaos
  residual** (sigma-amplified cuBLAS/cuDNN non-determinism, non-zero pre-refactor too) — explicitly out of scope;
  **not regressed**.

### Lint / build
- `cargo clippy -p waav-infer-backend-torch --all-targets -- -D warnings` → **clean** (EXIT 0).
- `cargo build --workspace` → **clean** (the server/provider/core consumers compile against the rewired models).

---

## Files changed (ONLY these)
- **new** `crates/waav-infer-backend-torch/src/cfm/ode.rs` — `CfmOde` + `FlowField`.
- **new** `crates/waav-infer-backend-torch/src/cfm/dpm.rs` — `DpmSolver` (+ `interp1d`).
- **new** `crates/waav-infer-backend-torch/src/cfm/vocoder.rs` — `HiftVocoder` + helpers + constants.
- **new** `crates/waav-infer-backend-torch/src/cfm/tests.rs` — the 8 `cfm::` unit tests.
- `crates/waav-infer-backend-torch/src/cfm/mod.rs` — filled stub: module decls, re-exports, `DiffusionStepper`
  marker + `StepperKind`, family index doc.
- `crates/waav-infer-backend-torch/src/cosyvoice3.rs` — deleted local `CfmOde`/`FlowField` + the HiFT vocoder
  block + `weight_norm_reconstruct`/`hann_periodic`; rewired `OnnxFlowField` to `cfm::FlowField` (assoc-error),
  `run_cfm` to `cfm::CfmOde`, the loaders to `cfm::vocoder::*`; removed the 4 moved tests.
- `crates/waav-infer-backend-torch/src/vibevoice.rs` — deleted local `DpmSolver` + `interp1d`; imported
  `cfm::DpmSolver`; removed the 1 moved test.

**Not touched:** `lib.rs` (the `pub mod cfm;` was pre-declared), `ci/heavy_live_tests.sh`,
`COMPONENT_CATALOG.md`, other models, other crates.

No deterministic seam changed — **done**.
