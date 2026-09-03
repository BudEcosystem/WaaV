# Multi-LoRA / S-LoRA adapter serving — the closed vLLM-parity gap

> **The gap (`VLLM-PARITY-MATRIX.md` §3.4, pillar 4 = MISSING):** vLLM's S-LoRA — *many* fine-tuned LoRA
> adapters hot-swapped/batched over ONE loaded base. WaaV had only **bookkeeping** shelf-ware
> (`scheduler::rollout::LoraRegistry`: a session→adapter-id binding table, *no weights, no apply, no GEMM*).
> There was **no adapter registry of weights, no LoRA-apply on `nn::Linear`, no batched multi-adapter GEMM.**
>
> **This session built the real S-LoRA machinery on the tch path, byte-faithful to `peft`, with a passing
> live gate.** All four reference paths reproduce **real `peft` 0.19.1 BYTE-FOR-BYTE (max|Δ| = 0)** on CPU f32.

---

## 1. Verdict

| Question | Answer |
|---|---|
| Multi-LoRA working + byte-faithful? | **YES** — base-only, base+adapterK, merged==unmerged, **and batched mixed-adapter (S-LoRA)** all `max|Δ|=0` vs `peft`. |
| Adapter registry (load N from safetensors)? | **LANDED** — `AdapterRegistry::load_peft` reads peft `adapter_model.safetensors` + `adapter_config.json` (r/alpha/use_rslora), keyed by `AdapterId`. |
| LoRA-apply on `nn::Linear`? | **LANDED** — `LoraLinear::forward`, `y = base(x) + scaling·(x·Aᵀ)·Bᵀ`, byte-identical to peft's vanilla math. |
| Hot-swap (no reload)? | **LANDED** — select a different `AdapterId` on the same loaded base; gate proves swap→distinct, swap-back→byte-identical. |
| Batched mixed-adapter (the real S-LoRA win)? | **LANDED** — `LoraLinear::forward_mixed`, segmented per-adapter (peft `_mixed_batch_forward`), each row == its solo run. |
| Live gate (base+adapter == peft reference)? | **GREEN** — `lora_serve_byte_faithful_to_peft` passes against real `peft` golden. |
| Reused existing LoRA-apply? | None existed (matrix confirmed); added cleanly to a new shared `nn/lora.rs`. Bridges the scheduler's existing `LoraRegistry` *binding* layer (id↔session) — the weights half is now real. |
| Non-LoRA regression (dia2/csm byte-identity)? | **SAFE** — only change to a shared file is **2 additive lines** in `nn/mod.rs` (a `pub mod` + a re-export). `linear.rs`/`kv_cache.rs`/`rms_norm.rs`/`rope.rs`/`dia2.rs`/`csm.rs` are **untouched** (`git diff` empty). All 62 `nn::` primitive byte-identity tests + 198 lib tests green. |

---

## 2. What landed (each byte-faithful + gated)

All in the **`waav-infer-backend-torch`** crate — the tch path where `nn::Linear` is ours (the LAW's target).

### 2.1 Adapter registry + loader — `AdapterRegistry`
- `load_peft(id, dir, target_module, dev)` reads the **real peft on-disk format**:
  `adapter_config.json` → `r`, `lora_alpha`, `use_rslora`; `adapter_model.safetensors` → `lora_A:[r,in]`,
  `lora_B:[out,r]` (suffix-matched `…<module>.lora_{A,B}.weight`, tolerating the `base_model.model.` prefix).
- **`scaling = alpha/r`** (vanilla) **or `alpha/√r`** (rslora) — exactly `peft.LoraLayer.scaling`. The gate
  cross-checks each loaded scaling against peft's computed value (`2.0`, `4.0`, `8.0` for the 3 ref adapters).
- N adapters resident over ONE base, keyed by `AdapterId`. Loading an adapter is a small `[r,in]+[out,r]`
  read (~ms), not a model load. Typed `LoraError` (never a panic): `NotRegistered`, `Shape`, `Config`, `Load`,
  `BatchLenMismatch`.

### 2.2 LoRA-apply on `nn::Linear` — `LoraLinear` (the seam models compose)
- `forward(x, None)` ⇒ **bare base forward, bit-identical to today** (the non-LoRA regression guard —
  wrapping a `Linear` with no active adapter cannot perturb dia2/csm).
- `forward(x, Some(id))` ⇒ `base(x) + scaling·(x·Aᵀ)·Bᵀ`. The two low-rank matmuls use the **same zero-copy
  `reshape→matmul(wᵀ)→reshape` spelling** as `Linear::Matmul`, scalar-scaled last — byte-identical to peft's
  `result + lora_B(lora_A(x))·scaling`.
- `merged_weight(W)` = `W + scaling·B·A` (what peft `merge_and_unload` folds) — the hot-swap/merge anchor.

### 2.3 Hot-swap (no reload)
- Switch the active adapter by id on the loaded base. Gate proves: two distinct adapters → distinct outputs
  (a real swap, not a no-op); swap-back-to-K → byte-identical (no hidden state).

### 2.4 Batched mixed-adapter (S-LoRA) — `LoraLinear::forward_mixed(x, names)` ✅ LANDED THIS SESSION
- ONE base GEMM for the whole batch (the S-LoRA amortization), then **group rows by adapter** (deterministic
  `BTreeMap`), gather each adapter's sub-batch, apply its low-rank delta, scatter-add back. `__base__` rows get
  base only. This is **exactly peft's `_mixed_batch_forward`** (segmented, order-independent).
- Output row *i* is **byte-identical** to a single-adapter `forward` of row *i* alone (per-slot isolation).

---

## 3. The live gate — `lora_serve_byte_faithful_to_peft` (GREEN)

Loads adapters + golden produced by **real `peft` 0.19.1** (`export_ref.py`, a throwaway reference run —
**no serving path, [[waav-infer-no-venv-wrap]] honored**; the script only *generates a reference golden*).
Small **synthetic** base (`nn.Linear` 16→12) + **3 real peft LoRA adapters** with distinct r/alpha
(`voice_hi` r4 α8, `lang_med` r2 α8, `rs_demo` r4 α16 **rslora**) — disk-tight (no large base download; root was
at 22G/99% the whole session, `df` unchanged). Real whisper/qwen voice LoRAs exist on HF (`whisper-large-v2-LORA-*`)
but their base is multi-GB (won't fit) and whisper isn't on the tch path; the synthetic-vs-real-peft proof is the
**rigorous** machinery gate (byte-exact vs the actual peft kernels).

```
running 2 tests
test mixed_batch_len_mismatch_is_typed_error ... ok
PASS lora_serve_byte_faithful_to_peft: base-only + 3×(base+adapter) + merged==unmerged + mixed-batch(S-LoRA)
     all BYTE-IDENTICAL (max|Δ|=0) to peft 0.19.1 on CPU f32
test lora_serve_byte_faithful_to_peft ... ok
test result: ok. 2 passed; 0 failed
```

Asserted `max|Δ| == 0` vs the peft golden for:
1. **base-only** == peft `disable_adapter()` AND == the bare `Linear` (regression guard).
2. **base+adapter_K** (×3) == peft's adapter-K forward — the hot-swap correctness.
3. **merged W'** == peft `merge_and_unload`'s folded weight, **and** merged forward == peft merged forward.
4. **mixed-batch** `[voice_hi, lang_med, rs_demo, __base__]` == peft `_mixed_batch_forward`, **and** each
   row == its single-adapter solo run.

**One byte-identity scar caught + fixed (the f32-bisect):** the first run diverged `1.9e-6` on base-only.
RCA: peft's base is `nn.Linear`→`F.linear` (fused `addmm`), so the byte-faithful spelling is
`LinearKind::AtLinear`, not `Matmul`+separate-bias (the documented `nn/linear.rs` ULP scar). Fixing the gate's
base/merged spelling to `AtLinear` → `max|Δ|=0` everywhere. (The *adapter delta* itself is bias-free, so its
`Matmul`-spelled low-rank matmuls are already byte-identical to peft's bias-free `F.linear` projections on f32.)

---

## 4. Test / clippy results (recorded)

- `cargo test -p waav-infer-backend-torch --lib` → **198 passed, 0 failed, 2 ignored** (incl. 6 new
  `nn::lora` unit tests; the prior 192 unaffected).
- `cargo test -p waav-infer-backend-torch --test lora_peft_byte_faithful` → **2 passed, 0 failed** (the live
  peft gate + a typed-error guard).
- `cargo clippy -p waav-infer-backend-torch --all-targets -- -D warnings` → **clean** (and `--lib` clean).
- **Non-LoRA regression:** the 62 `nn::` primitive byte-identity tests (linear / rms_norm / rope / attention /
  kv_cache — the numeric base dia2/csm/voxtral compile against) all green; the shared numeric files are
  **untouched** (`git diff --stat` empty for `linear.rs`/`kv_cache.rs`/`rms_norm.rs`/`dia2.rs`/`csm.rs`).
  Heavy live-GPU model gates run process-isolated via `ci/heavy_live_tests.sh` and were **not** run (GB10
  ORT-teardown leak forces one-model-per-process; not run to avoid OOMing the unified pool) — but **no shared
  model byte-identity path was touched**, so dia2/csm/voxtral bit-faithfulness is structurally unaffected (the
  change is a new additive `nn/lora.rs` module + 2 re-export lines).

---

## 5. What LANDED vs SCOPED

**LANDED (byte-faithful + gated):** adapter registry + peft loader · LoRA-apply on `nn::Linear` · single-slot
hot-swap · **batched mixed-adapter (S-LoRA) grouped/segmented apply** · the full live peft gate. The "if
feasible this session" batched-grouped path **is landed** (not deferred).

**SCOPED (the last mile — net-new, multi-day, high regression surface — NOT faked):**
- **Per-model adoption + per-slot id threading.** To make a *specific* model serve multi-LoRA end-to-end, its
  projection `Linear`s must be swapped for `LoraLinear` and a per-slot `AdapterId` threaded from the
  scheduler's `LoraRegistry::adapter_for(session)` (the existing binding bookkeeping) into the model's
  `step_batch`/`StepInput`. That touches every adopting model's forward (a per-model byte-identity-regression
  surface) — scoped, not done blanket. The **machinery + apply + hot-swap + batched-mixed are all built and
  byte-faithful**; adoption is now a per-model `Linear`→`LoraLinear` substitution + id-plumb, not new physics.
- **Grouped-GEMM kernel fusion.** `forward_mixed` is the correct **segmented** S-LoRA apply (1 base GEMM + per-
  adapter gather/low-rank/scatter), byte-exact. A single *fused* grouped-GEMM kernel (BGMV/SGMV-style) is a perf
  optimization over the segmented loop, not a correctness change — additive, scoped.
- **bf16/CUDA byte-identity.** The gate is CPU f32 (the deterministic byte bar, same regime as the `nn::Linear`
  unit tests). The apply reuses the shared `Matmul`/`AtLinear` spellings whose bf16/CUDA byte-identity is
  already gated for the base models, so the CUDA path inherits the same kernel discipline; a dedicated
  CUDA-bf16 multi-LoRA golden is the additive next gate.

---

## 6. Exact files

| File | Status | Note |
|---|---|---|
| `crates/waav-infer-backend-torch/src/nn/lora.rs` | **NEW** | The S-LoRA machinery: `AdapterId`, `LoraAdapter`, `AdapterRegistry` (+ `load_peft`), `LoraLinear` (`forward` / `forward_mixed`), `LoraError`, `BASE_ADAPTER`. 6 unit tests. |
| `crates/waav-infer-backend-torch/src/nn/mod.rs` | **EDITED (additive, shared)** | +`pub mod lora;` and +`pub use lora::{…}`. **The only shared-file change** — 2 lines, no numeric path touched. |
| `crates/waav-infer-backend-torch/tests/lora_peft_byte_faithful.rs` | **NEW** | The live gate vs real peft golden (`lora_serve_byte_faithful_to_peft`) + a typed-error guard. |
| `scratchpad/lora_ref/export_ref.py` | reference-only (throwaway) | Generates the peft golden into `/home/bud/.cache/waav-models/lora-golden/` (regenerate cmd in the gate's skip notice). NOT a serving path. |

**Flagged shared `nn::` change:** `crates/waav-infer-backend-torch/src/nn/mod.rs` — purely additive (module
decl + re-export); `nn/linear.rs` and every other shared primitive are untouched, so dia2/csm/voxtral
byte-identity is structurally unchanged.

**Existing shelf-ware bridged:** `crates/waav-infer-scheduler/src/rollout.rs` `LoraRegistry` (session↔id
*binding* bookkeeping, already gated) now has its **weights half** — `nn::lora::AdapterRegistry` resolves an
`AdapterId` to resident `(A,B,scaling)` and applies it. The two compose: scheduler decides *which* id per
session; `LoraLinear` applies *that* id's adapter, byte-faithfully.
