# B35 — The `kernels` policy: selectable libtorch SDPA backend + TF32 precision (Torch backend, Phase 4 / final)

**Goal (memory `waav-infer-modularize-reuse`; spec: `nn/mod.rs` "Kernel / hardware-backend swap"):** make the
"which libtorch SDPA backend runs, and what f32-matmul precision (TF32) is in effect" decisions **selectable
per `(device, shape, dtype, gqa, mask, causal)`** instead of hardcoded at each model's attention call — so
swapping a kernel (or adding a hardware backend) is a **policy swap, zero model edits**. The shared
`nn::Attention` (→ ALL 7 tch models) now consults a `kernels::KernelPolicy` for both decisions.

Status: **DONE.** 92 lib tests green (incl. 5 new `kernels::` tests), clippy `--all-targets -D warnings`
clean, and the **4 kernel-sensitive GPU gates stay byte-identical** (csm canary, dia2 608/608, voxtral
100.0%, cohere 100.0%). No model output changed. Only `crates/waav-infer-backend-torch/` was touched.

---

## 1. The `KernelPolicy` API (`crates/waav-infer-backend-torch/src/kernels/mod.rs`)

```rust
/// The libtorch SDPA backend an attention call resolves to.
pub enum AttnKernel { Math, MemEfficient, Flash, Cudnn, FusedAuto }

/// The shape/dtype/structure signature of one attention op (pure data — no tensors).
pub struct KernelSig { q_shape: [i64;4], dtype: Kind, gqa: bool, has_mask: bool, is_causal: bool }

pub trait KernelPolicy: Debug + Send + Sync {
    fn attn_kernel(&self, dev: &TorchDevice, caps: Option<&DeviceCaps>, sig: KernelSig) -> AttnKernel;
    fn allow_tf32(&self, dev: &TorchDevice) -> bool;
}

pub struct DefaultPolicy { tf32_on_cuda: bool }   // ::tf32_off()  | ::tf32_on()  (dia2)
pub struct PerfPolicy    { tf32_on_cuda: bool }   // ::new()       | ::with_tf32()  (the swap template)
```

- **`AttnKernel`** is the libtorch SDPA *backend identity*. Crucial reality (confirmed by reading `tch`
  0.20 / `torch-sys` 0.20): **libtorch exposes NO SDP-backend selector** over this FFI — no `enable_flash_sdp`,
  no `setSDPPriorityOrder`, only the raw `scaled_dot_product_attention` op + a read-only `_fused_sdp_choice`
  probe. So a backend is **steered by the SDPA call arguments, not set by a flag** (the B27/B23 law). Hence
  `AttnKernel` names the backend the chosen argument-combination *resolves to*, and the two realizable points
  are **`Math`** (form the call WITH an explicit additive mask — forces math) and **`FusedAuto`** (form it
  WITHOUT a mask + `is_causal` — libtorch's internal heuristic dispatches flash/mem-efficient). `Flash` /
  `MemEfficient` / `Cudnn` are *pin requests*; the current FFI can't pin one, so they realize as `FusedAuto`
  today (documented honestly — they exist so a policy can express the preference and the wiring lands the
  moment the SDP-priority FFI is bound, still no model edit).
- **`AttnKernel::wants_explicit_mask()`** (`true` only for `Math`) is the ONE bit the fused-attention wiring
  reads to form `(mask, is_causal)`.
- **`allow_tf32(dev)`** is the TF32 precision decision, carried per-policy (dia2 on, everyone else off).

### How a new kernel or hardware backend plugs in (zero model edits)
1. **New kernel preference** — write a `KernelPolicy` whose `attn_kernel` returns a different `AttnKernel`
   for the shapes you want, and pass it to the models. `PerfPolicy` is the shipped template: it prefers the
   fused backend wherever an explicit mask is *not* required, but **never drops a required mask** (correctness
   floor — swapping it onto dia2 is safe, still `Math`).
2. **New hardware backend (ROCm / Metal / …)** — `nn::Attention` already takes a `tch::Device` (the HW
   abstraction is "pass a different device"), so the only HW knob this layer owns is the per-vendor
   kernel/precision choice. The trait's `caps: Option<&DeviceCaps>` (B15) is the hook: a `RocmPolicy` /
   `MetalPolicy` (or a `match caps.vendor()` arm inside `DefaultPolicy`) returns the right `AttnKernel` +
   `allow_tf32` for that substrate (e.g. on Metal the fused backend differs; on ROCm TF32 has no analog). The
   models + `nn` primitives are untouched.

---

## 2. `DefaultPolicy` mapping — the BIT-FAITHFUL default (which kernel per signature)

`DefaultPolicy::attn_kernel` is the proven B27/B23 rule, **device/dtype-agnostic** (the choice is a
numerical-fidelity scar, not a HW pick — so it holds identically on CPU f32 and CUDA bf16, which is *why*
every model is byte-identical on both substrates):

> **`has_mask → Math` ; else → `FusedAuto`.**

| model | attention primitive / signature at the SDPA call | `AttnKernel` (Default) | TF32 on CUDA |
|---|---|---|---|
| voxtral / ark (decoder) | `ManualGqa` — hand `matmul→softmax(f32)→matmul` | `Math` (hand math; policy not consulted) | off |
| voxtral / ark (encoder) | `sdpa_manual` (in `asr/`, not via `Attention`) | — (unchanged) | off |
| cohere (self + cross) | `ManualMha` — hand math | `Math` (policy not consulted) | off |
| cosyvoice3 / vibevoice | `FusedCausalGqa`: `(has_mask=false, is_prefill)`, gqa=`true` | **`FusedAuto`** | off |
| csm (backbone + depth) | `FusedCausalMaybeGqa`: `(false, is_prefill)`, gqa=`n_q!=n_kv` | **`FusedAuto`** *(canary)* | off |
| dia2 | `FusedMaskedGqa`: `(has_mask=true, false)` + `finfo.min` mask, gqa=`n_q!=n_kv` | **`Math`** | **on** |

**TF32:** `DefaultPolicy::tf32_on()` (= `Attention::dia2_policy()`) is the only TF32-carrying policy;
`allow_tf32(dev) == tf32_on_cuda && dev.is_cuda()` → true **only for dia2, only on CUDA** — bit-identical to
the old `device.is_cuda()`-gated `libtorch_tf32::enable()` call dia2 made (B23). Every other model uses
`DefaultPolicy::tf32_off()` (tch's full-FP32 "highest" default).

**Two load-bearing scars the mapping protects:**
- **csm canary** — csm's `create_causal_mask` returns `None`, so it MUST stay on the no-mask + `is_causal` →
  `FusedAuto` path; an explicit mask would force `Math`, which rounds differently in bf16 and flips a sampled
  codebook (B27). Default returning `FusedAuto` for csm's `(false, is_causal)` signature is the guard.
- **dia2** — its full-padded `finfo.min` mask forces `Math` (B23) AND its f32 projections/heads need TF32
  ("high") to match the sidecar. Both are reproduced: `has_mask → Math`, and `dia2_policy()` carries TF32-on.

---

## 3. Wiring — `nn::Attention` / `sdpa` consult the policy (instead of the hardcoded per-call arg)

- **`Attention` gained one field:** `policy: Arc<dyn KernelPolicy>` (shared so the engine can inject one
  policy across all layers). Two helper constructors: `Attention::default_policy()` (the floor) and
  `Attention::dia2_policy()` (TF32-on).
- **`Kernel` enum (the primitive selector) was re-scoped:** it still pins the *primitive* (manual vs fused)
  and the **structural** `enable_gqa` flag + explicit-mask SOURCE (byte-identity scars, NOT a backend pick).
  The **math-vs-fused backend** is no longer hardcoded in its fused variants — it is the policy's.
- **`run_kernel`:** the manual family (`ManualGqa` / `ManualMha`) is unchanged (it IS a hand math op — the
  policy is not consulted). The **fused trio** (`FusedCausalGqa` / `FusedCausalMaybeGqa` / `FusedMaskedGqa`)
  now: computes its structural `gqa` flag + explicit-mask source → builds a `KernelSig` → asks
  `self.policy.attn_kernel(dev, None, sig)` → forms the `sdpa` args from `AttnKernel::wants_explicit_mask()`:
  - `Math` ⇒ `sdpa(q,k,v,scale, explicit_mask, is_causal=false, gqa)` — dia2's exact form.
  - else  ⇒ `sdpa(q,k,v,scale, None, is_causal=is_prefill, gqa)` — cosy/vibe/csm's exact form.
  - The device for the policy call is derived from `q.device()` (a CUDA tensor proves CUDA is available, so
    no device is stored in `Attention` and the engine seam is not crossed).

  Under `DefaultPolicy` each variant resolves to **exactly the backend + args it used before** → identical
  libtorch call → identical bytes. (Verified: dia2 `FusedMaskedGqa` → Math → `(attn_mask, false, n_q!=n_kv)`;
  cosy/vibe `FusedCausalGqa` → FusedAuto → `(None, is_prefill, true)`; csm `FusedCausalMaybeGqa` → FusedAuto →
  `(None, is_prefill, n_q!=n_kv)` — bit-for-bit the pre-B35 code.)
- **dia2 TF32 enable** now gated on `Attention::dia2_policy().allow_tf32(&device)` (true iff CUDA — identical
  to the old `device.is_cuda()` gate) — the precision regime is the policy's, not a hardcoded call.

---

## 4. Unit tests — `cargo test -p waav-infer-backend-torch --lib kernels::` → 5 passed

- **`default_policy_matches_each_model_backend`** — the regression guard for the SHARED Attention: one
  assertion per model signature. dia2 `(has_mask)` → `Math`; cosy/vibe `(no mask, is_causal, gqa)` →
  `FusedAuto`; csm backbone+depth `(no mask, is_causal, ±gqa)` → `FusedAuto` (canary); voxtral/ark/cohere
  explicit-mask prefill → `Math`. Asserted in **both** CUDA-bf16 and CPU-f32 signatures (the
  device/dtype-agnostic property).
- **`default_policy_tf32_intent`** — dia2 policy carries `tf32_on_cuda`, the floor doesn't; both are `false`
  on CPU (TF32 is a CUDA-only path; the CUDA-on case is confirmed end-to-end by the dia2 GPU gate).
- **`wants_explicit_mask_only_for_math`** — the one wiring bit is `true` only for `Math`.
- **`perf_policy_prefers_fused_but_keeps_required_mask`** — `PerfPolicy` returns `Flash` for a no-mask sig but
  keeps `Math` for dia2's required mask (correctness floor).
- **`kernel_labels_stable`** — telemetry labels.

Full lib: **92 passed; 0 failed** (was 64 at B30; +5 kernels tests + intervening growth). The shared
`nn::self_attention` / `nn::layer` / `nn::backbone` Attention-composition tests (manual-GQA / fused-causal-GQA
/ cross-attn / f32-sandwich-layer) all still pass through the new policy-driven `run_kernel`.

---

## 5. Byte-identity preserved — the 4 kernel-sensitive GPU gates (GB10 CUDA, `--include-ignored --test-threads=1`)

| model | gate | result |
|---|---|---|
| **csm** (canary; `FusedCausalMaybeGqa → FusedAuto`) | CUDA bf16 greedy codes vs sidecar golden + RTF | **2 passed** — `cuda_csm_codes_byte_identical_to_sidecar` ok (byte-identical), RTF ok |
| **dia2** (`FusedMaskedGqa → Math` + TF32-on policy) | CUDA bf16 codes vs CUDA sidecar + CPU fp32 | **608/608 CUDA bf16 match; first-div=None** + **544/544 CPU fp32**; `ok. 3 passed` |
| **voxtral** (`ManualGqa`) | STRICT char-identity transcript vs ORT-CPU | **EXACT char-identity 100.0%** (primary clip); `ok. 1 passed` (max RTF 0.89) |
| **cohere** (`ManualMha` self + cross-attn) | de-punctuated char similarity vs ORT-CPU | **100.0%**; `ok. 1 passed` (RTF 0.09) |

These four exercise the full backend space the policy now owns: dia2 = the math-via-explicit-mask path AND
the TF32-on precision gate (the single most kernel+precision-sensitive model — 608 sampled bf16 codes
byte-identical proves the policy reproduced both the `Math` backend and `allow_tf32` exactly); csm = the
`FusedAuto`-via-`is_causal` canary (an accidental flip to `Math` would have flipped a codebook — it didn't);
voxtral/cohere = the hand-written manual family (policy reports `Math`, primitive unchanged). **All stay
byte-identical → the policy layer is bit-faithful for every model's backend + TF32 choice.**

---

## 6. Clippy

`cargo clippy -p waav-infer-backend-torch --all-targets -- -D warnings` → **clean** (no findings; the only
transient warnings during development — an unused `tch::Device` import in the kernels test and an unused
`KernelPolicy` import in dia2 — were removed: trait-object methods on `Arc<dyn KernelPolicy>` resolve without
importing the trait).

---

## 7. Files changed (all under `crates/waav-infer-backend-torch/`)

- **New:** `src/kernels/mod.rs` — `AttnKernel`, `KernelSig`, `KernelPolicy`, `DefaultPolicy`, `PerfPolicy` +
  the 5 unit tests + the full design/HW-backend doc.
- **Wiring:** `src/nn/self_attention.rs` — `Attention.policy` field, `default_policy()`/`dia2_policy()`
  constructors, `policy_device()`, policy-driven fused-trio in `run_kernel`, `Kernel` enum re-scoped doc, the
  2 in-file test literals updated; `src/nn/mod.rs` — "Phase 4 — WIRED" catalog doc; `src/nn/layer.rs` +
  `src/nn/backbone.rs` — the 5 test-module `Attention` literals get `policy:`.
- **Models (only where they pass the policy — 1 line each):** `voxtral.rs`, `ark.rs`, `cohere.rs`, `csm.rs`,
  `cosyvoice3.rs`, `vibevoice.rs` → `policy: nn::Attention::default_policy()`; `dia2.rs` →
  `policy: nn::Attention::dia2_policy()` + its TF32 enable gated on `dia2_policy().allow_tf32(&device)`.
- **`src/lib.rs`** — `pub mod kernels;` (+ its one-line doc) only.

Not touched: `ci/heavy_live_tests.sh`, other crates. (The untracked `ci/phase_c_model_sweep.sh` and `docs/`
predate this work and were not modified.) No `git commit` — changes left for the coordinator.
