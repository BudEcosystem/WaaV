# Per-slot batched-mixed LoRA (S-LoRA) LIVE in the lockstep batcher — the scoped tail closed

> **The gap (`MULTI-LORA-LIVE.md` §6 SCOPED):** the in-`nn::Linear` multi-LoRA seam served ONE adapter
> per request (a SINGLE shared `ActiveAdapter` id applied to the whole input). `LoraLinear::forward_mixed`
> was byte-faithful to peft's `_mixed_batch_forward` (max|Δ|=0) **standalone**, but it was never threaded
> through a model's **batched** forward — "threading a per-row `adapter_names` slice through the … lockstep
> step is net-new plumbing." No real batched serve routed different batch ROWS to different adapters.
>
> **This session closes it.** A model's batched `[B,…]` forward now routes **each batch ROW to its own
> adapter** through the SAME shared `nn::Linear` projection seam — **without any signature change** to the
> shared `Attention`/`Mlp`/`Backbone`/`TransformerLayer` `forward` (the 7-model regression surface). The
> **REAL qwen3-tts talker** (a codec-AR Qwen3 backbone) is adopted to it, and a **B=4 batched talker forward
> where rows use voice_hi / lang_med / base / rs_demo** is **byte-identical (max|Δ|=0)** to each row's
> single-adapter solo run, with the base (all-`None`) path **bit-identical** to the no-LoRA batch.

---

## 1. The verdict

| Question | Answer |
|---|---|
| Per-slot batched-mixed LoRA working + byte-faithful (each slot == solo)? | **YES** — a single B-row batched forward with DIFFERENT adapter ids per row == each row's single-adapter solo, **max\|Δ\|=0**, on the REAL qwen3-tts talker (CPU f32). |
| Base path (all-`None`) bit-identical to today's no-LoRA batch? | **YES** — adapter-loaded base batch == standalone base batch == `PerRow(all-None)` batch, **max\|Δ\|=0**; qwen3-tts CUDA bf16 golden (PREFILL + FIRST-DECODE talker hidden **Δ==0**) unchanged; canary gate still all max\|Δ\|=0; lib 206/206. |
| In-`Linear` `PerRow` routing == peft `_mixed_batch_forward`? | **YES** — `lora_peft_byte_faithful` now asserts the in-`Linear` `PerRow` path **byte-identical to the real peft 0.19.1 mixed_batch golden** (max\|Δ\|=0), not just `LoraLinear::forward_mixed`. |
| Single-slot hot-swap + typed rejects still hold? | **YES** — `set_adapter` (Single) distinct→distinct, clear→base, swap-back byte-identical; a ghost id (single OR per-row) is a typed reject. |

---

## 2. The batcher wiring — one selector flip routes EVERY projection per-row (no signature change)

The killer constraint (same as the single-adapter seam): make a model's batched forward route per-row
**without** changing the shared transformer `forward` signatures (any change = a 7-model regression surface).
Solution: the routing regime lives **inside the shared selector**, read by `Linear::forward`.

### 2.1 Shared `nn::` — the `AdapterSelection` regime (`nn/lora.rs`)
The shared per-model selector `ActiveAdapter = Arc<Mutex<AdapterSelection>>` now carries an enum:

```rust
pub enum AdapterSelection {
    Single(Option<AdapterId>),       // the original per-request regime — bit-identical
    PerRow(Vec<Option<AdapterId>>),  // per-slot batched-mixed: names[i] for batch ROW i (None ⇒ base)
}
```

- `set_active(active, id)` → `Single(id)` (unchanged behaviour; the default is `Single(None)` = the base node).
- **`set_active_per_row(active, names)`** → `PerRow(names)` — the **lockstep batcher's `adapter_names`** flip:
  ONE pointer flip routes every LoRA-aware projection of the model per-row. The model sets it once per batched
  step before its `[B,…]` forward.
- `active_single(active)` reads the single-adapter id (the hot-swap reader).

### 2.2 The per-row apply core (`nn/lora.rs::apply_per_row`)
The segmented gather/low-rank/scatter — **the SAME core both paths use** (so they are byte-identical by
construction): given the already-computed base GEMM `[B,…]` + `x[B,…]` + per-row `names[i]`, for each distinct
adapter it gathers that adapter's sub-batch rows, applies `LoraAdapter::delta` (the peft `scaling·(x·Aᵀ)·Bᵀ`
math already proven byte-exact), and `index_copy`s back — exactly peft's `_mixed_batch_forward`. A row whose id
this projection does not carry stays base (mirrors the single path: "a projection an adapter does not target
keeps the base output"). `LoraLinear::forward_mixed` was **refactored to delegate to this core** (so the
standalone and in-`Linear` paths can never drift).

### 2.3 The seam — `Linear::forward` routes per-row (`nn/linear.rs`)
`Linear::forward` (the primitive ALL `Attention`/`Mlp`/`Proj`/`GateUp` projections funnel through) now matches
the selection:

```rust
let Some(lora) = &self.lora else { return base };   // ← UNCHANGED fast path: 6 non-adopting models → base
match &*lora.active.lock()… {
    AdapterSelection::Single(id)   => …base (None) / base + adapter.delta(x) (Some, if registered here)…  // bit-identical to before
    AdapterSelection::PerRow(names) => apply_per_row(base, x, names, &lora.adapters, |_| true)             // NEW: per-row gather/low-rank/scatter
}
```

The `lora == None` early-return is byte-for-byte the prior code, so **every non-adopting model (dia2/csm/
voxtral/…) is structurally untouched** — the regression guard. The `Single` arms reproduce the original
`Option` behaviour exactly (proven: canary gate + lib 206/206 green).

### 2.4 Model adoption — qwen3-tts's talker (`qwen3_tts.rs`)
qwen3-tts's talker is a plain Qwen3 GQA decoder built from the SAME `nn::Proj::Separate{q,k,v}` +
`nn::Mlp::swiglu_separate(gate,up,down)` seam as canary — the proven template. Added (mirroring canary):
- `Qwen3TtsAdapterSpec` + `Qwen3TtsAdapterBag` (reads a peft dir once into f32 + r/alpha/rslora scaling,
  indexed by the `layers.{i}.<module>` suffix).
- `build_qwen3_layer(…, lora: Option<LoraAttach>)` `attach`es the resident adapters to q/k/v/o + gate/up/down
  of each talker layer (no-op → bit-identical when `lora` absent; the code-predictor passes `None`).
- `TorchQwen3Tts::load_with_lora(dir, dev, force_fp32, &specs)` + `set_adapter` / `clear_adapter` /
  **`set_per_row_adapters(&[Option<AdapterId>])`** / `active_adapter` + a deterministic batched probe
  `talker_logits_batched(&[Vec<i64>])` (one `[B,L,hidden]` talker backbone forward → per-row `codec_head`
  logits) — the gate seam.

The lockstep `step_batch` is `chatterbox`'s (ONNX `StaticGraph`, not `nn::Linear`); the tch codec-AR models
(qwen3-tts/dia2/csm) batch internally over a `[B,…]` axis through their `nn::Backbone`. The per-row LoRA seam
lands at `Linear::forward` precisely so it threads through that batched backbone forward with **zero**
batcher-signature change — `set_per_row_adapters` IS the `adapter_names` thread.

---

## 3. The LIVE-GATE results

### 3.1 `qwen3tts_per_slot_batched_mixed_lora` — REAL qwen3-tts talker, CPU f32 (GREEN)
```
PASS (a) base batch bit-identical: adapter-loaded==standalone, PerRow(all-None)==cleared
PASS (b) per-slot batched-mixed: each of 4 rows == its single-adapter solo (max|Δ|=0)
PASS (c) single-slot hot-swap + typed rejects
```
- **(a)** loaded-with-adapters base batch == standalone (never-adapted) base batch == explicit `PerRow(all
  None)` batch → all **max\|Δ\|=0** (no regression on the base path).
- **(b)** a B=4 `PerRow` batch routed `[voice_hi, lang_med, None(base), rs_demo]` differs from the base batch
  (adapters genuinely fire), and **each row of the ONE batched forward == that row's `Single`-adapter batch-1
  solo run, max\|Δ\|=0** — the S-LoRA per-slot isolation invariant, on the real 28-layer Qwen3 talker
  (q/k/v/o + gate/up/down all adopted).
- **(c)** single-slot `set_adapter` hot-swap (distinct→distinct, clear→base, swap-back byte-identical) +
  ghost id rejected (single AND per-row).

### 3.2 `lora_peft_byte_faithful` — in-`Linear` `PerRow` == peft 0.19.1 (GREEN)
Extended the existing gate: beyond `LoraLinear::forward_mixed`, it now builds the **in-`Linear` `PerRow`**
path (the lockstep-batcher path: set the shared selector, run one batched `Linear::forward`) and asserts it
**byte-identical to peft's `_mixed_batch_forward` golden (max\|Δ\|=0)** AND each per-row row == its `Single`
solo. So the new seam — not just the standalone helper — is peft-byte-faithful.

### 3.3 Unit gates (`nn/linear.rs`, GREEN — 5 new)
`single_none_is_bit_identical_to_plain_base` · `single_some_applies_one_adapter_to_whole_input` ·
**`per_row_routing_equals_single_solos`** · **`per_row_all_none_is_bit_identical_to_plain_base`** ·
`per_row_untargeted_adapter_falls_through_to_base` (an untargeted id routes that row to base on a projection,
single-path parity).

---

## 4. Base-path bit-identical confirmation (NO regression)

- **qwen3-tts CUDA bf16 golden** (`cuda_qwen3_tts_codes_byte_identical_to_sidecar`): PREFILL talker hidden
  **Δ==0** + FIRST-DECODE talker hidden **Δ==0**, greedy tracks 44 frames, codec decode corr 0.9998 — the
  adopting model's base path is byte-for-byte unchanged (`attach` is a no-op when `lora=None`; `load`/
  `load_fp32` pass `None`).
- **canary multi-LoRA gate** (`canary_lora_serve_byte_faithful_to_peft`): (a)/(b)/(c)/(d) all **max\|Δ\|=0**
  to peft — the shared `AdapterSelection` enum is bit-identical for the original single-slot path.
- **`waav-infer-backend-torch --lib`**: **206 passed, 0 failed** (incl. every non-LoRA model's byte gates,
  which take the unchanged `lora==None` fast path).
- **clippy** `-p waav-infer-backend-torch --all-targets --features cuda -D warnings`: **clean** (verified after
  the concurrent chunked-prefill agent's ark.rs/backbone.rs edits settled).

The shared change is: one early-return-preserving `match` in `Linear::forward` (the 6 non-adopters never
reach it), one enum on the shared selector, and a per-row apply core reused by `forward_mixed`. dia2/csm/
voxtral/… are structurally AND empirically unperturbed.

---

## 5. What LANDED vs SCOPED

**LANDED (byte-faithful + gated):**
- The shared `AdapterSelection::{Single, PerRow}` regime on `nn::ActiveAdapter` + `set_active_per_row` /
  `active_single` (`nn/lora.rs`, `nn/mod.rs`).
- The per-row gather/low-rank/scatter core `apply_per_row` (`nn/lora.rs`), reused by both
  `LoraLinear::forward_mixed` (refactored to delegate) AND the in-`Linear` path.
- `Linear::forward` routes per-row under `PerRow` with NO signature change; base path bit-identical (`nn/linear.rs`).
- **qwen3-tts talker adopted** to the seam (`load_with_lora` / `set_per_row_adapters` / `talker_logits_batched`).
- The LIVE per-slot batched-mixed gate on the real qwen3-tts talker + the in-`Linear` peft-byte-faithful gate
  + 5 unit gates.

**SCOPED (precisely):**
- **Engine-API surface (scheduler → `set_per_row_adapters`).** The model-side per-row seam is COMPLETE and
  gated; wiring the engine's `SttModel`/`TtsModel` serve loop to resolve the per-slot ids from
  `LoraRegistry::adapter_for` into a `&[Option<AdapterId>]` and call `set_per_row_adapters` before the batched
  step is the remaining engine-glue (the `TtsModel` trait has no per-row hook yet — additive, the model seam
  is the proven anchor). Deliberately not touched here: the **scheduler/prefill path is owned by a concurrent
  agent** (chunked-prefill); threading the per-row vector through `Driver::tick`/`step_batch` is their seam to
  extend, on top of this landed `set_per_row_adapters` primitive.
- **CUDA-bf16 per-row golden.** The gate is CPU f32 (the deterministic byte bar, same regime as the canary +
  `nn::lora` gates). The apply reuses the shared `Matmul`/`delta` spelling whose bf16/CUDA byte-identity is
  already gated for the qwen3-tts base; a dedicated CUDA-bf16 per-row golden is the additive next gate.
- **Other adopters (dia2/csm).** The seam is now a per-model `Linear` adoption + `set_per_row_adapters` (qwen3
  -tts is the proven template, exactly as canary was for the single path); not done blanket.

---

## 6. Exact files

| File | Status | Note |
|---|---|---|
| `crates/waav-infer-backend-torch/src/nn/lora.rs` | **EDITED (shared `nn::`)** | `AdapterSelection::{Single,PerRow}` enum + `set_active`/`set_active_per_row`/`active_single`; `apply_per_row` core; `forward_mixed` refactored to delegate; `AdapterRegistry::adapters()`. |
| `crates/waav-infer-backend-torch/src/nn/linear.rs` | **EDITED (shared `nn::`)** | `Linear::forward` routes per-row under `PerRow` (base path bit-identical; `lora==None` fast path unchanged); `new_active_adapter` → `Single(None)`; 5 new per-row unit gates. |
| `crates/waav-infer-backend-torch/src/nn/mod.rs` | **EDITED (shared `nn::`)** | re-export `active_single`/`set_active_per_row`/`AdapterSelection`. |
| `crates/waav-infer-backend-torch/src/qwen3_tts.rs` | **EDITED (model adoption)** | `Qwen3TtsAdapterSpec`/`Qwen3TtsAdapterBag` + `build_qwen3_layer(…, lora)` attach + `load_with_lora` + `set_adapter`/`clear_adapter`/`set_per_row_adapters`/`active_adapter` + `talker_logits_batched` probe. |
| `crates/waav-infer-backend-torch/src/canary_qwen.rs` | EDITED (1 line) | `active_adapter()` now reads via `nn::active_single` (the selector is an `AdapterSelection`). |
| `crates/waav-infer-backend-torch/tests/qwen3tts_lora_batched.rs` | **NEW** | The LIVE per-slot batched-mixed gate on the real qwen3-tts talker (CPU f32). |
| `crates/waav-infer-backend-torch/tests/lora_peft_byte_faithful.rs` | EDITED | + the in-`Linear` `PerRow` == peft `mixed_batch` byte-faithful assertion (gate part 5). |
| `scratchpad/qwen3tts_lora_ref/export_ref.py` | reference-only (throwaway) | Writes the 3 peft adapters (in=talker hidden 1024) into `~/.cache/waav-models/qwen3-tts-lora-golden/`. NOT a serving path. |

**Flagged shared `nn::` changes:** `nn/lora.rs` (+`AdapterSelection`/`set_active_per_row`/`active_single`/
`apply_per_row`; `forward_mixed` refactor), `nn/linear.rs` (+`PerRow` match arm, base path bit-identical),
`nn/mod.rs` (re-exports). Every numeric primitive (`rms_norm`/`rope`/`kv_cache`/`attention`/`mlp`/`backbone`/
`layer`) is **untouched** — non-adopting models take the unchanged `lora==None` early-return.

**Concurrency note:** `ark.rs` + `nn/backbone.rs` were being edited by the **concurrent chunked-prefill agent**
during this session (their `prefill_chunked`); those files are NOT part of this work. Final state with their
edits settled: lib 206/0, the package clippy `--all-targets --features cuda -D warnings` clean.
