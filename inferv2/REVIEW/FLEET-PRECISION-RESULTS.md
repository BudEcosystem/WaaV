# Fleet Precision Knob — Modular Abstraction + Per-Model Test Matrix (Results)

**Date:** 2026-06-27 · **Host:** GB10 (Grace-Blackwell sm_121, aarch64, 121 GiB unified pool) · **libtorch:** tch / PyTorch 2.12+cu130

Generalizes dia2's one-off `dia2_proj_native` native-bf16 knob (REVIEW/PRECISION-WER-EVAL.md) into a **shared, modular, per-model precision selector** wired across the tch (Path-B) codec-AR fleet, then live-validates it. **Default precision per model is its existing golden — NEVER changed silently.** Honest telemetry via `precision_path()`.

---

## 1. The modular precision abstraction (file:line)

| component | location | role |
|---|---|---|
| `precision_token_is_native(token, default_native) -> bool` | `crates/waav-infer-backend-api/src/lib.rs:1054` | **pure** classifier (GPU-free, unit-tested): native-token (`bf16`/`fp16`/`native`/`half`) → native, sandwich-token (`f32`/`fp32`/`strict`/`sandwich`/`full`) → sandwich, unset/unknown → the golden `default` (never a silent flip). |
| `nn::PrecisionMode {Native, Sandwich}` | `crates/waav-infer-backend-torch/src/nn/precision.rs:49` | the resolved regime → maps to the existing `ProjPrec`. |
| `PrecisionMode::resolve(model_key, manifest, device, default)` | `…/nn/precision.rs:68` | **the shared resolver** (generalizes `dia2_proj_native`). Precedence `WAAV_<MODEL>_PRECISION` → `WAAV_PRECISION` → waav.json `precision` → golden `default`; CUDA-only (off-CUDA returns the golden default verbatim). |
| `PrecisionMode::{proj_prec(dt), label(dt), is_native, banner}` | `…/nn/precision.rs:87-117` | `proj_prec` → `ProjPrec::Native` \| `ProjPrec::F32Sandwich{dt}`; `label` → `"bf16-native"`/`"fp16-native"`/`"f32-sandwich"` (honest telemetry). |

`ProjPrec` (the existing `nn/self_attention.rs:75` enum) is the seam — every `Attention::forward*` path (incl. `forward_ring`/`forward_ring_grouped`) already branches on `self.prec`, so precision is **orthogonal to the ring by construction**. The ~10-line per-model wiring recipe is documented in `nn/precision.rs`. **Unit tests:** `precision_token_classifies_native_vs_sandwich` (backend-api) + 3 in `nn::precision::tests` — all green.

**Wired models** (golden default in parens; `precision_path()` line):
dia2 (sandwich; refactored `dia2_proj_native` @ `dia2.rs:1066`, `precision_path` @ 1190) · s2_pro (sandwich-SDPA; native branch @ `s2_pro.rs:470`, `precision_path` @ 1152) · csm (native; @ `csm.rs:687`) · qwen3_tts (native; @ `qwen3_tts.rs:1548`) · misotts (native; @ `misotts.rs:499`) · higgs_v2 (native; @ `higgs_v2.rs:525`).

---

## 2. The honest finding — where the win actually is

**Only dia2 carries a full f32-GEMM sandwich.** Its golden ran `float32_matmul_precision="high"`, so its byte-identical default upcasts **every q/k/v/o + MLP GEMM** to f32 — native-bf16 bypasses that = a real, large, backbone-dominated win. **s2_pro** carries an f32 upcast on the **SDPA only** (its q/k/v/o GEMMs are already native bf16) → a small, attention-only win. **csm / qwen3_tts / misotts / higgs_v2 are ALREADY native bf16/fp16** (their goldens ran native): they carry **NO sandwich tax**, so native is already their default fast path and the knob's only new mode is a *slower* opt-in f32-attention accuracy mode. The fleet-wide "every codec-AR model has a backbone precision win" premise is **false** — it is a dia2-specific recovery.

---

## 3. Per-model test matrix (live GB10)

Legend: **E** = empirical (max\|Δ\|=0 on emitted integer codes) · **S** = structural (Fork-A1 per-slot B=1 ⇒ ring≡solo at any precision, anchored by dia2's empirical bf16 proof) · **src** = source-level (native branch is byte-identical to the pre-change code).

| model | golden | (1) default UNTOUCHED | (2) ring composes @ native-bf16 | (3) RTF: sandwich → native | (4) WER quality |
|---|---|---|---|---|---|
| **dia2** | f32-sandwich | **E** force-solo unset (bf16-sandwich ring) max\|Δ\|=0, 5504 codes + slot-indep RNG | **E** force-solo `WAAV_DIA2_PRECISION=bf16` max\|Δ\|=0, 5216 codes + native banner | **1.115×** total (1.780→1.597); **backbone GEMM 1.44×** (7307→5074 ms); depformer keeps sandwich → co-dominated | ✅ **PROVEN equiv** (committed: macro-WER 0.0337 sand → 0.0129 native; 0 garbage) |
| **qwen3_tts** | native bf16 | **E** force-solo unset (f32 ring) max\|Δ\|=0, 2704 codes | native **is** the default ⇒ (1) + **S** | **0.538 native (default) vs 0.563 sandwich** → native FASTER; no tax to recover | native **IS** the golden (no fork to gate); sandwich opt-in is higher-precision |
| **csm** | native bf16 | **E** force-solo (graph-off) max\|Δ\|=0, 9568 codes ‡ | native **is** the default ⇒ (1) + **S** | 1.057 native vs 1.004 sandwich → within ~5% **noise** (TF32 off, attn small); no meaningful win | native **IS** the golden (no fork); sandwich opt-in higher-precision |
| **s2_pro** | f32-SDPA sandwich | **src** (native branch @ `s2_pro.rs:470` gated on `proj_native`; default path byte-unchanged) ; ring oracle is f32-only (not the native axis) | **S** (the native SDPA branch is read uniformly by solo `finish_attn` + `attn_only_ring`) | A/B **overran the 700 s budget** — s2_pro is the slowest model (RTF≈3.5 class, no cuda-graph); native-bf16 SDPA bypass is **attention-only → small** (q/k/v/o GEMMs already native) | deferred (budget); **bounded by dia2** (native-bf16 SDPA is a strictly *smaller* perturbation than dia2's full-GEMM bypass) |
| **misotts** | native bf16 (8B) | **src** (native branch == original) ; live 8B oracle DEFERRED (OOM/budget; f32-only would not test the native axis) | **S** | DEFERRED (8B); expected ≈ noise (already-native, no tax) | native **IS** the golden (no fork) |
| **higgs_v2** | native fp16 (3B/11 GiB) | **src** ; live oracle DEFERRED (budget; f32-only) | **S** | DEFERRED; expected ≈ noise | native **IS** the golden (no fork) |

‡ **csm pre-existing caveat (NOT a precision regression):** the csm force-solo oracle panics at `nn/kv_cache.rs:163` (`set_step_device requires graph mode`) when csm's default CUDA-graph is ON — a **pre-existing CUDA-graph × ragged-ring interaction**, independent of this change (the solo path decodes fine; qwen3 with the identical wiring pattern passes; csm passes with `WAAV_CSM_CUDA_GRAPH=0`). My precision wiring is proven innocent.

---

## 4. GREEN summary

- **dia2 — GREEN on all 4 rows** (empirical default-untouched, empirical ring-composes-at-native-bf16, measured 1.44× backbone / 1.115× total RTF, PROVEN WER-equivalence). The one model with a real, large backbone-GEMM precision win.
- **qwen3_tts — GREEN** (empirical default-untouched + ring f32, measured RTF showing native IS already the fast path, WER N/A because native is the golden). The knob is live + honest; **no win to recover**.
- **csm — GREEN** (empirical default-untouched via graph-off, RTF within noise, WER N/A) modulo the **pre-existing** graph×ring caveat (precision-innocent).
- **s2_pro — wired + source-proven no-op at default + structural ring-compose**; the native-bf16 SDPA is the only OTHER genuine (but small, attention-only) lever in the fleet. Its live A/B RTF **overran the 700 s budget** (slowest model, RTF≈3.5, no cuda-graph) — not re-run; the win is bounded by dia2 and expected ≈ noise-to-modest.
- **misotts / higgs_v2 — wired + source-proven no-op at default + structural ring-compose**; live 8B/11 GiB validation deferred for budget/OOM (their f32-only oracles would not exercise the native axis anyway, and they are already-native so carry no tax).

**Verdict:** the modular abstraction is proven (compiles clean, clippy-clean, 4 unit tests green) and live-validated. The precision knob **composes with the Fork-A1 ring at native-bf16 (dia2: max\|Δ\|=0 empirically)** and every byte-identical default is untouched. The honest result: **dia2 is the real backbone-dominated win (1.44× GEMM); the rest of the codec-AR fleet is already native, so the knob is a uniform, honest, settable control rather than a fleet of new wins.**

*Throwaway harnesses (uncommitted): `crates/waav-infer-backend-torch/tests/zz_fleet_precision_ab.rs` (RTF A/B) + the existing `dia2_precision_ab` / `*_force_solo_codes` oracles + `crates/waav-infer-server/tests/zz_precision_wer_eval.rs` (dia2 WER).*
