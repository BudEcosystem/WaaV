# Multi-LoRA FULLY LIVE-SERVED on a real model — the closed "per-model adoption" gap

> **The gap (`MULTI-LORA.md` §5 SCOPED):** the S-LoRA machinery (`nn/lora.rs`: `AdapterRegistry`/`LoraLinear`/
> `forward_mixed`) was byte-faithful to `peft` (max|Δ|=0) but **unattached** — "per-model adoption is
> integration, not machinery." A *specific* model's projections were never swapped for the LoRA seam, no
> per-slot `AdapterId` was threaded through the engine, and no engine-served live gate proved it.
>
> **This session closes it.** **NVIDIA canary-qwen-2.5b** (FastConformer → Qwen3-1.7B LLM-decoder ASR) is now
> **engine-served with hot-swappable LoRA adapters** on its Qwen3 q/k/v/o + gate/up/down projections,
> **byte-faithful to real `peft` 0.19.1 (max|Δ|=0)**, with the **base path bit-identical** (the existing canary
> CUDA byte gate still emits **0.0% WER**; dia2 + csm still byte-identical to their sidecar goldens).

---

## 1. Which model — and the verdict

**`canary_qwen` (NVIDIA canary-qwen-2.5b, `crate::canary_qwen::TorchCanaryQwen`)** — a Qwen3-1.7B-backbone
LLM-decoder ASR already cached (`~/.cache/waav-models/canary-qwen-2.5b`) with a durable golden, clean
`nn::Proj::Separate{q,k,v}` + `nn::Mlp::swiglu_separate(gate,up,down)`. (Its built-in *speech*-LoRA stays MERGED
into q/v at load — the resident multi-LoRA adapters are ADDITIONAL, hot-swappable, on top of that plain-Qwen3
base.)

| Question | Answer |
|---|---|
| A real model engine-served with a live hot-swappable LoRA adapter? | **YES** — `engine::load_model_at` reads `runtime.adapters`, attaches 3 resident peft adapters over ONE base; `SttModel::set_adapter(id)` hot-swaps per request. |
| Byte-faithful to the peft reference? | **YES** — base+adapterK == real `peft` 0.19.1 forward **max\|Δ\|=0** (CPU f32), 3 adapters × 3 projections, + scaling cross-check (2.0 / 4.0 / 8.0-rslora). |
| Base path bit-identical (no regression)? | **YES** — adapter-loaded base == standalone base **max\|Δ\|=0**; the canary CUDA WER gate is **0.0%** (unchanged); dia2 **544/544 + 608/608** and csm **125×32** codes still byte-identical to their goldens. |
| Hot-swap with no reload? | **YES** — distinct ids → distinct outputs; clear → base; swap-back → byte-identical (no hidden state). |
| Batched-mixed (S-LoRA)? | **LANDED at the projection seam** over the REAL canary q_proj (`LoraLinear::forward_mixed`: each row == its solo run, max\|Δ\|=0). Per-slot batched serving across engine slots is scoped (canary is sequential STT — see §6). |

---

## 2. The wiring (4 layers)

### 2.1 Shared seam — multi-LoRA *inside* `nn::Linear` (the only shared-file change)
The killer design constraint: a model's projections must become LoRA-capable **without changing the shared
`Attention`/`Mlp`/`Backbone`/`TransformerLayer` `forward` signatures** (those are used by all 7 tch models — any
signature change is a 7-model regression surface). Solution: the LoRA state lives **inside `Linear`** as an
optional, boxed field + a shared active-id selector.

- **`Linear.lora: Option<Box<LinearLora>>`** (`nn/linear.rs`). `LinearLora` = `{ adapters: BTreeMap<AdapterId,
  LoraAdapter>, active: ActiveAdapter }`. Each projection owns its OWN per-adapter `(A,B,scaling)`; only the
  active-id *selection* is shared.
- **`ActiveAdapter = Arc<Mutex<Option<AdapterId>>>`** (`nn/lora.rs`) — one per model, cloned into every
  LoRA-aware `Linear`. It holds only the id (a device-free `String`), so the `Arc` is `Send + Sync` even though
  the per-projection adapter tensors are not (they stay owned in each `Linear`, never in the shared `Arc`). This
  is what lets the model stay `Send` (boxed into `LoadedModel::Stt`) while sharing a per-request selector.
- **`Linear::forward`** fast-paths to the exact prior `base_forward(x)` when `lora == None` (every non-adopting
  model) OR the active selector is `None` (the base node) OR this projection didn't register the active id →
  **bit-identical**. With an active id registered here: `base(x) + adapter.delta(x)` (the SAME `LoraAdapter::
  delta` peft math `nn::lora` already proved byte-exact). **Boxed** so a non-LoRA `Linear` grows by a single null
  pointer — the regression guard extends to struct SIZE (it fixed a `clippy::large_enum_variant` that the
  un-boxed field had nudged the unrelated `zonos2::Ffn` enum into).
- `set_active(active, id)` is the hot-swap primitive (one pointer flip flips every projection at once).

### 2.2 Model adoption — canary's Qwen3 projections (`canary_qwen.rs`)
- `CanaryAdapterSpec { id, dir }` + `PeftAdapterBag` (reads a peft dir ONCE into f32 + the r/alpha/rslora
  scaling, then indexes each module's `(A,B)` by the real per-layer suffix `layers.{i}.<module>`).
- `build_qwen3_layer(..., lora)` now `attach`es the resident adapters to **q/k/v/o + gate/up/down** of each
  layer (no-op → bit-identical when `lora` is absent). The built-in speech-LoRA stays merged into q/v as before.
- `TorchCanaryQwen::load_with_lora(dir, dev, force_fp32, &specs)`; `set_adapter(id)` / `clear_adapter()` /
  `active_adapter()`; deterministic probes for the gate (`proj_forward`, `proj_base_weight`, `prefill_logits`).

### 2.3 Engine thread — `SttModel::set_adapter` + manifest `runtime.adapters` (`core/model.rs`, `engine.rs`)
- **`SttModel::set_adapter(&mut self, id: Option<&str>)`** added to the core trait (default: `None` Ok, `Some`
  rejected — never silently wrong). canary impls it via the shared selector. This is the per-request seam the
  scheduler's existing `LoraRegistry::adapter_for(session)` feeds: the engine resolves the per-slot id and calls
  `set_adapter` before `transcribe`.
- **`engine.rs`** `TorchInprocessCfg` gained `adapters: Vec<(String, PathBuf)>` from
  `runtime.adapters: [{"id","dir"}]`; the `canary_qwen` arm calls `load_with_lora` when adapters are present
  (f32 on CPU / bf16 on CUDA), else the unchanged base `load`.

### 2.4 Reference — real `peft` (throwaway, NOT a serving path, [[waav-infer-no-venv-wrap]])
`scratchpad/canary_lora_ref/export_ref.py` (persisted to `~/.cache/waav-models/canary-lora-golden/`): wraps the
REAL canary merged-base projections (q/v: `W + (256/128)·B·A` folded exactly like the Rust loader; gate: plain) in
real `peft.tuners.lora.Linear` layers, builds 3 small synthetic adapters (`voice_hi` r4 α8, `lang_med` r2 α8,
`rs_demo` r4 α16 **rslora**) targeting q_proj/v_proj/gate_proj of layer 0, and exports the peft on-disk adapters
+ the golden `base_out` / `adapter_out` / `merged_out` / `merged_w` / `scaling` for a fixed input — the byte
targets the Rust gate diffs.

---

## 3. The four LIVE-GATE results — `canary_lora_serve_byte_faithful_to_peft` (GREEN, CPU f32)

```
PASS (a) base == standalone == peft base (bit-identical), 3 projections
PASS (b) base+adapterK == peft (byte-faithful), 3 adapters × 3 projections + scaling cross-check
PASS (c) hot-swap: distinct adapters → distinct; clear → base; swap-back → byte-identical
PASS (d) batched-mixed S-LoRA over real canary q_proj: each row == solo (byte-identical)
test result: ok. 1 passed
```

| Gate | What it asserts | Result |
|---|---|---|
| **(a) base no-adapter == standalone** | canary loaded WITH adapters (active=None) proj forward == the standalone `load_fp32` proj forward, AND == peft's base golden | **max\|Δ\|=0** (no regression) |
| **(b) base+adapter == peft** | for each (adapter, projection): `proj_forward` with that id active == peft's `LoraLayer` forward; merged anchor agrees w/ peft `merge_and_unload`; Rust scaling == peft scaling | **max\|Δ\|=0** (byte-faithful) |
| **(c) hot-swap** | voice_hi→lang_med changes output; clear→base golden; swap-back→voice_hi byte-identical | distinct / base / **max\|Δ\|=0** |
| **(d) batched-mixed (S-LoRA)** | `LoraLinear::forward_mixed([voice_hi, lang_med, __base__, rs_demo])` over the REAL canary q_proj: each row == its solo `forward` | **max\|Δ\|=0** per row |

**Engine-served gate** — `canary_lora_engine_served_set_adapter_routes` (GREEN): `engine::load_model_at` on a
manifest with `runtime.adapters` attaches the 3 adapters; `SttModel::set_adapter(Some("voice_hi"/…))` hot-swaps
(Ok), `Some("ghost")` is a typed reject, `None` reverts to base — the production serve seam end-to-end.

> **One scar caught + fixed (key naming).** First (b) run diverged by ~the full delta magnitude: the reference
> adapter's safetensors keys lacked the per-layer prefix, so the Rust `layers.{i}.<module>` suffix match never
> bound the adapter (base-only fall-through). Fixed the export to the real `base_model.model.model.layers.{L}.
> <module>.lora_{A,B}.weight` naming → the loader binds to layer 0 only (and NOT layers 1..27) → max|Δ|=0.

---

## 4. Base-path bit-identical confirmation (no regression)

- **canary's own existing byte gate** (`cuda_torch_canary_qwen`, the adopting model, full FastConformer→Qwen3 on
  CUDA bf16): **0.0% WER vs the NeMo golden on both clips** — unchanged (the `build_qwen3_layer` refactor's
  `attach` is a no-op when no adapters; the base load path is bit-identical).
- **dia2** (`cuda_torch_dia2`): **544/544** CPU-fp32 + **608/608** CUDA-bf16 codes byte-identical to the sidecar.
- **csm** (`cuda_torch_csm`): greedy CUDA-bf16 codes **byte-identical** (125 frames × 32 codebooks).
- **`nn::` primitive byte-identity + the existing `nn::lora` peft gate**: the 198 lib tests (incl. linear /
  rms_norm / rope / attention / kv_cache + the 6 `nn::lora` unit tests) all green; `lora_peft_byte_faithful`
  still max|Δ|=0 to peft.

The shared change is a single early-return in `Linear::forward` (taken by all non-adopters) + one boxed optional
field, so dia2/csm/voxtral/… are structurally AND empirically unperturbed.

---

## 5. Test / clippy results (recorded)

- `cargo test -p waav-infer-backend-torch --lib` → **198 passed, 0 failed** (== the MULTI-LORA.md baseline).
- `cargo test -p waav-infer-backend-torch --test canary_lora_byte_faithful -- --ignored` → **1 passed** (the 4 live gates).
- `cargo test -p waav-infer-server --features torch --test canary_lora_served -- --ignored` → **1 passed** (engine-served).
- `cargo test -p waav-infer-server --features torch --lib` → **68 passed**; `cargo test -p waav-infer-core --lib` → **81 passed**.
- `cargo clippy --workspace --all-targets -D warnings` → **clean**; `-p waav-infer-server --features torch --all-targets` → **clean**.
- Regression CUDA gates: `cuda_torch_canary_qwen` 0.0% WER; `cuda_torch_dia2` 544/544+608/608; `cuda_torch_csm` 125×32 — all byte-identical/unchanged.

---

## 6. What LANDED vs SCOPED

**LANDED (byte-faithful + gated):** the in-`Linear` multi-LoRA seam (shared, bit-identical when off) · canary's
q/k/v/o + gate/up/down adopted to it · per-request `AdapterId` threaded `engine::load_model_at`
(`runtime.adapters`) → `SttModel::set_adapter` → the shared selector · single-slot hot-swap · the full
peft-byte-faithful live gate + the engine-served gate · batched-mixed (S-LoRA) at the projection seam over the
REAL canary q_proj.

**SCOPED (precisely):**
- **Per-slot batched-mixed across ENGINE slots.** canary is a **sequential** `SttModel::transcribe` (batch-1
  greedy decode); the engine serves one adapter per request via `set_adapter`. The batched-mixed
  `forward_mixed` (different rows → different adapters in ONE base GEMM) is landed + byte-gated **at the
  projection seam over the real canary weights**, but threading a per-row `adapter_names` slice through the
  canary `transcribe_batch` / lockstep step is net-new plumbing (the batcher would need a per-slot id vector) —
  scoped, not faked. The machinery (`LoraLinear::forward_mixed`) is proven on the real model.
- **bf16/CUDA multi-LoRA golden.** The gate is CPU f32 (the deterministic byte bar — same regime as the
  `nn::Linear` unit tests + the existing `nn::lora` gate). The apply reuses the shared `Matmul` spelling whose
  bf16/CUDA byte-identity is already gated for the canary base; a dedicated CUDA-bf16 multi-LoRA golden is the
  additive next gate.
- **Other Qwen3-backbone adopters (higgs-stt / qwen3-tts).** The seam is now a per-model `Linear` adoption +
  the same `set_adapter` wiring (canary is the proven template); not done blanket.

---

## 7. Exact files

| File | Status | Note |
|---|---|---|
| `crates/waav-infer-backend-torch/src/nn/linear.rs` | **EDITED (shared `nn::`)** | `Linear.lora: Option<Box<LinearLora>>` + `LinearLora` + `with_lora_adapter` + `new_active_adapter`; `forward` fast-paths to the bit-identical `base_forward` when off. **The load-bearing shared change** — base path bit-identical, struct grows by one null ptr. |
| `crates/waav-infer-backend-torch/src/nn/lora.rs` | **EDITED (shared `nn::`)** | `ActiveAdapter` type + `set_active`; `LoraAdapter::delta` made `pub` (the in-`Linear` seam reuses the SAME peft delta math). |
| `crates/waav-infer-backend-torch/src/nn/mod.rs` | **EDITED (shared `nn::`)** | re-export `new_active_adapter`/`LinearLora`/`set_active`/`ActiveAdapter`. |
| `crates/waav-infer-backend-torch/src/canary_qwen.rs` | **EDITED (model adoption)** | `CanaryAdapterSpec` + `PeftAdapterBag` + `build_qwen3_layer(..., lora)` attach + `load_with_lora` + `set_adapter`/`clear_adapter`/`active_adapter` + `proj_forward`/`proj_base_weight`/`prefill_logits` + `SttModel::set_adapter`. |
| `crates/waav-infer-core/src/model.rs` | **EDITED (trait seam)** | `SttModel::set_adapter(Option<&str>)` (default no-op/typed-reject) — the engine↔scheduler per-request seam. |
| `crates/waav-infer-server/src/engine.rs` | **EDITED (engine)** | `runtime.adapters` manifest read + `adapter_specs` resolve + the `canary_qwen` arm routes to `load_with_lora`. |
| `crates/waav-infer-backend-torch/tests/canary_lora_byte_faithful.rs` | **NEW** | The 4-gate live byte-faithful-to-peft gate on the real canary projections (CPU f32). |
| `crates/waav-infer-server/tests/canary_lora_served.rs` | **NEW** | The engine-served gate (`load_model_at` + `SttModel::set_adapter` routing). |
| `scratchpad/canary_lora_ref/export_ref.py` | reference-only (throwaway) | Generates the peft golden + 3 peft adapters into `~/.cache/waav-models/canary-lora-golden/`. NOT a serving path. |
| `~/.cache/waav-models/canary-qwen-lora-served/waav.json` | gate fixture | The `runtime.adapters` manifest the engine-served gate dispatches on (symlinks the canary weights). |

**Flagged shared `nn::` changes:** `nn/linear.rs` (+ optional boxed field, bit-identical-when-off forward),
`nn/lora.rs` (+`ActiveAdapter`/`set_active`, `delta` made pub), `nn/mod.rs` (re-exports). Every numeric primitive
(`rms_norm`/`rope`/`kv_cache`/`attention`/`self_attention`/`mlp`/`backbone`/`layer`) is **untouched** — dia2/csm
byte-identity is structurally AND empirically (544/544+608/608, 125×32) unchanged.
