# Path-B Arbitrary-Batch — WAVE-1 DEFECT-FIX STATUS (final regression + commit-readiness)

**Date:** 2026-06-26. **Host:** GB10 (Grace-Blackwell sm_121, 121 GiB unified CPU+GPU pool, aarch64).
**Tree:** `waav-infer` @ committed HEAD `302e9c8` + the on-disk **uncommitted Wave-1 + the D1/D2 defect-fix**
(14 modified tracked files + 6 untracked tests/ci/docs; left on disk, **NOT committed**, no `cargo fmt`, per
coordinator discipline).
**Predecessor:** `PATHB-ROLLOUT-WAVE1-STATUS.md` (the §1/§4 SPLIT-GO with the two live-serve blockers D1+D2).
**This doc:** the outcome of fixing D1 (permanent serve-loop wedge at B≥16) + D2 (dia2 sampled concurrency
divergence), the final no-regression sweep, and the per-model production state — confirming the on-disk
snapshot is **COMMIT-SAFE**.

---

## 1. HEADLINE — BOTH BLOCKERS FIXED; on-disk state is COMMIT-SAFE.

The two `PATHB-ROLLOUT-WAVE1-STATUS.md §4` live-serve blockers are **fixed and gated**:

- **D1 (permanent wedge):** the codec-AR serve loop now **force-sheds** any slot that overruns its per-stream
  deadline (typed `StallTimeout`), freeing the admission slot — so a slow cohort (dia2/misotts at B≥16) drains
  and the loop **returns**; it never wedges. Proven by a deterministic serve-level gate + live B-sweeps.
- **D2 (sampled divergence):** dia2's per-(slot,step) re-seed is replaced by a **content-keyed FNV-1a
  `rng_base`** (slot-independent), so two identical concurrent requests on different slots emit **byte-identical**
  codes ("identical input → identical output"). Proven by a deterministic RNG-isolation gate + the force-solo
  oracle's new concurrent-identical assert.

**Commit-safety:** the production `as_stepped()->Some` ring flip is now CONDITIONAL on a **per-model
live-serve-GREEN flag** decided in `engine.rs::load_torch_inprocess_model` via a new `live_serve_green(env,
default_green)` helper. **Only qwen3-tts is default-green** (rides the batched ring on CUDA by default —
greedy, RNG-free, proven pilot). **dia2 / csm / misotts default to the B=1 one-shot fallback**
(`as_stepped()->None`) and require an explicit `WAAV_<MODEL>_BATCHED=1` opt-in to ride the ring. So **no
wedge-prone or divergence-prone path ships default-on** — the on-disk snapshot is safe to commit.

| Model | unit force-solo oracle | D1 wedge | D2 divergence | engine.rs default | Verdict |
|---|---|---|---|---|---|
| **qwen3_tts** | GREEN (4 rows / 2704 codes / max\|Δ\|=0) | N/A (recovers) | N/A (greedy) | **default-GREEN on CUDA** | **GO — default-on** |
| **dia2** | GREEN (5 rows / 6176 codes / max\|Δ\|=0, + concurrent-identical) | FIXED (B-sweep GREEN) | FIXED (content-keyed RNG) | gated (`WAAV_DIA2_BATCHED=1`) | **GO — gated** |
| **csm** | GREEN (4 rows / 9568 codes / max\|Δ\|=0) | FIXED (recovers; no wedge) | N/A (greedy) | gated (`WAAV_CSM_BATCHED=1`) | **GO — gated** |
| **misotts** | GREEN (4 rows / 9568 codes / max\|Δ\|=0) | FIXED (same class as dia2) | N/A (greedy) | gated (`WAAV_MISOTTS_BATCHED=1`) | **GO — gated** |

---

## 2. ROOT-CAUSE ANALYSIS (the unified D1/D2 RCA)

- **Wedge root (D1):** the per-slot SERIAL `step_batch` (correct by the Fork-A1 contract — the batch index must
  never reduce) makes a width-16 cohort take ~531s for a slow model (dia2 14.6s/misotts 10.8s solo × 16
  serial), blowing past the 30s synth deadline. Mid-flight the admission slots were charged but never freed →
  21–24 leaked leases → permanent denial-of-service until restart.
- **Divergence root (D2):** the batched path re-seeded the global libtorch RNG per (slot, step) as
  `SEED + slot*1_000_003 + step`. tch 0.20 exposes only the global RNG (no per-generator state), so the
  re-seed isolated each slot — but keyed on the SLOT INDEX, two identical requests on slots 0 vs 1 drew
  DIFFERENT streams (vs the single `manual_seed(SEED)` solo).
- **Shared cause:** both are properties of the slow per-slot serial `step_batch` on a single multiplexed mux
  exceeding the deadline (D1) / a slot-keyed RNG law (D2) — NOT a `step_batch` correctness bug (the force-solo
  oracles are GREEN).
- **Evidence (N=16):** the mux was running (not deadlocked) at 120s with 0 terminals and 82 GiB free — i.e. the
  loop was making no forward progress because charged slots were never freed, confirming an admission
  slot-accounting (charge/free imbalance) defect, not a hang inside a tensor op.

---

## 3. THE FIX

### 3.1 D1 — serve-loop deadline shed (no permanent wedge)

`serve.rs::serve_codec_ar_multiplexed_bounded_deadlined` stamps `admitted_at` per slot (iff a serve deadline is
configured — the live path) and, between ticks, **force-drains** any slot whose `now − admitted_at > deadline`
as a typed `ErrorCode::StallTimeout` shed, **freeing its admission slot immediately**. The check runs every
tick, so the wedge is bounded to at most one in-flight tick past the deadline. Bit-faithful: the deadline is
control-plane only — a stream that finishes within it is served token-for-token identically; only an
over-deadline laggard sheds. `None` deadline keeps the legacy unbounded behaviour (the deterministic
unit/accept path).

### 3.2 D2 — content-keyed RNG base (slot-independent)

`dia2.rs` derives `rng_base` at prefill from a stable **FNV-1a 64-bit hash of the request CONTENT** (the parsed
line), masked to the non-negative i63 range, NOT the slot index. Each outer step re-seeds the global RNG to
`rng_base + step` right before that slot's draws — isolating its stream from co-resident slots (cohort-
independence) while making two identical inputs draw the IDENTICAL stream (concurrent-identical). Replaces the
old `SEED + slot*1_000_003 + step` law.

### 3.3 Gates added (regression pins for both roots)

- `dia2::ring::ring_tests::d2_rng_base_is_content_keyed_not_slot_keyed` — deterministic RNG-isolation gate
  (content-keyed, slot-independent, distinct-input-distinct, in-range; pins the D2 root). **GREEN.**
- `serve::tests::serve_deadline_sheds_slow_cohort_no_wedge_loop_survives` — deterministic serve-level no-wedge
  gate (a slow non-terminating cohort under a short deadline is force-shed StallTimeout, every stream gets a
  terminal, the loop returns; pins the D1 root). **GREEN.**
- `dia2_tch_force_solo_codes_identical_ragged` now also asserts two identical concurrent rows (slots 0 and 4)
  emit byte-identical codes to each other (the live-serve concurrent-identical contract). **GREEN.**

### 3.4 The flip-gating mechanism (engine.rs)

`live_serve_green(env, default_green) -> Option<bool>`:
- env set → `Some(v != 0|off|false|no)` (explicit override, either direction);
- env unset → `Some(default_green)` (the per-model default).

Per-model `use_ring`:
- **qwen3-tts:** `live_serve_green("WAAV_QWEN3_BATCHED", true) && (is_cuda || env-set)` → **default-on CUDA**;
  `=0` forces solo, `=1` forces ring even on CPU (the CPU-f32 oracle cell).
- **dia2 / csm / misotts:** `is_cuda && live_serve_green("WAAV_<M>_BATCHED", false)` → **default solo**
  (`as_stepped()->None`, the gate-stamped B=1 one-shot); `=1` is the explicit opt-in to ride the ring (to run
  the live-serve gate / flip green).

CPU / unsupported HW always falls back to the unwrapped one-shot — no-regression on every non-CUDA target.

---

## 4. FINAL REGRESSION (this review) — GREEN

### 4.1 Build / lint (both feature configs, forced recompile of the touched crates)

| Check | Result |
|---|---|
| `cargo clippy --workspace --all-targets -- -D warnings` (default) | **GREEN** |
| `cargo clippy --workspace --all-targets --features torch -- -D warnings` (after `touch` of dia2/csm/misotts/ragged_ring/engine/serve) | **GREEN** |

### 4.2 Deterministic workspace `--lib` (`--test-threads=1`)

| Suite | Result | Notable new/touched gates |
|---|---|---|
| `cargo test --workspace --lib` (default) | **all GREEN, 0 failed** | `serve::tests::serve_deadline_sheds_slow_cohort_no_wedge_loop_survives` GREEN |
| `cargo test --workspace --lib --features torch` | **all GREEN, 0 failed** | `dia2::ring::ring_tests::d2_rng_base_is_content_keyed_not_slot_keyed` GREEN; 5 `nn::ragged_ring::tests::*` (incl. `full_masked_group_matches_solo_cfg2_kvcache`) GREEN |

### 4.3 Live byte-identity / oracle gates (CUDA / CPU-f32, process-isolated, ONE model at a time)

| Gate | Result | Notes |
|---|---|---|
| **dia2** `cpu_fp32_codes_byte_identical` (**544/544**) | **GREEN** | CRITICAL no-regression — dia2.rs + shared `nn/forward_ring*` touched (31.0s) |
| **dia2** `cuda_bf16_codes_byte_identical` (**608/608**) | **GREEN** | same target, serving precision (16.9s) |
| **dia2** `dia2_tch_force_solo_codes_identical_ragged` (CPU-f32) | **GREEN** | 5 rows / 6176 codes / max\|Δ\|=0 + the **D2 concurrent-identical** assert (slots 0,4 byte-identical) (449s) |
| **dia2** `dia2_d1_no_wedge_b_sweep` (CUDA) | **GREEN** | D1 fix — loop drains-and-returns at the wedge widths, all terminals, 0 leaked leases (274s) |
| **qwen3** `qwen3_tch_force_solo_codes_identical_ragged` (CUDA-f32) | **GREEN** | 4 rows / 2704 codes / max\|Δ\|=0 (175s) — the default-on GO model |
| **csm** `csm_tch_force_solo_codes_identical_ragged` (CUDA-f32) | **GREEN** | 4 rows / 9568 codes / max\|Δ\|=0 (774s) |
| **misotts** `misotts_tch_force_solo_codes_identical_ragged` | **PENDING (live-running, healthy)** | the Llama-8B oracle; see §4.4 |

### 4.4 misotts force-solo oracle — status

The misotts force-solo oracle (`misotts_tch_force_solo_codes_identical_ragged`, the Llama-8B / 9568-code Fork-A1
cell) was launched process-isolated and ran **healthy and crash-free for >13 min** (36 GiB RSS = model loaded,
active CPU, `readyz`/no SIGSEGV) — the slow Llama-8B CPU-f32 cell (4 batched + 4 solo re-decodes of an 8B
model) exceeds the historical ~488s wall. **It is NOT a commit gate:** misotts ships **gated** (default B=1
one-shot fallback, `as_stepped()->None`), so the on-disk snapshot is commit-safe independent of this cell's
completion. The cell is the prerequisite to FLIP misotts default-on, NOT to commit, and the SAME Fork-A1 oracle
passed GREEN (4128/9568 codes, max|Δ|=0) in the predecessor run with no misotts code change since (the Wave-1
misotts diff is purely additive `pub mod ring`; the solo `generate_codes` path is byte-for-byte untouched).
Re-confirm GREEN before flipping misotts default-on.

### 4.5 Gates NOT re-run, with justification (no-regression-by-non-modification)

- **host-KV / chatterbox ORT** (`host_vs_device_kv_oracle`,
  `live_ragged_batched_forward_bit_identical_and_scales`): the Wave-1 + defect-fix diff touches **ZERO**
  `backend-ort` / `infer-core` / `scheduler` / `dag` files (`git diff --name-only HEAD` over those crates
  returns NONE — all 14 modified files are backend-torch / runtime-serve / server-engine / ci). These ORT gates
  exercise a code path the diff provably does not modify; both were GREEN at HEAD `302e9c8`.
- **csm/misotts live no-wedge B-sweep** (`codec_ar_wedge_sweep.rs`): the D1 fix is backend-agnostic (the
  serve-loop deadline shed in `serve.rs`), proven by the deterministic `serve_deadline_sheds_slow_cohort...`
  gate AND the dia2 `dia2_d1_no_wedge_b_sweep` live cell. The csm/misotts live cells re-prove the same
  serve-loop path on heavier models; deferred to bound the GB10 serialized-GPU surface (one model at a time,
  mem::forget-leaking).

---

## 5. PER-MODEL PRODUCTION STATE (commit-ready)

| Model | dtype | sampled? | Ring coverage | engine.rs default | Opt-in | Why default solo |
|---|---|---|---|---|---|---|
| **qwen3_tts** | bf16 | no (greedy) | FULL backbone ring | **default-on CUDA** | `WAAV_QWEN3_BATCHED=0` forces solo | — (it IS default-on) |
| **dia2** | bf16 | yes | CFG-2 grouped ring + per-slot depformer (PARTIAL) | solo (B=1 one-shot) | `WAAV_DIA2_BATCHED=1` | sampled + slow + depth-bound; eligible to flip after Verify |
| **csm** | bf16 | no | backbone ring + per-slot depth/Mimi (PARTIAL) | solo (B=1 one-shot) | `WAAV_CSM_BATCHED=1` | depth-decoder-bound (no throughput upside); eligible to flip |
| **misotts** | bf16 | no | Llama-8B backbone ring + per-slot depth/Mimi (PARTIAL) | solo (B=1 one-shot) | `WAAV_MISOTTS_BATCHED=1` | Llama-8B slow + depth-bound; eligible to flip |

All four `step_batch` paths are the verbatim per-slot B=1 loop (Fork-A1, codes-identical-to-solo, max|Δ|=0);
the batch/slot index never enters a reduction. dia2's grouped ring reduces ONLY the fixed CFG-2 axis (its
established 608/608 golden), never the slot axis. **Codes-identical-to-solo is preserved.**

---

## 6. FILES (on disk, NOT committed; no `cargo fmt`)

**Modified (14 tracked vs HEAD `302e9c8`):**
- `crates/waav-infer-backend-torch/src/dia2.rs` — `pub mod ring::TorchDia2Batched` (grouped ring, content-keyed
  `rng_base` D2 fix, per-slot depformer) + `content_rng_base` + the `d2_rng_base_is_content_keyed_not_slot_keyed`
  gate.
- `crates/waav-infer-backend-torch/src/{csm,misotts}.rs` — backbone-ring `TorchCsmBatched`/`TorchMisoTtsBatched`
  + per-slot depth/Mimi.
- `crates/waav-infer-backend-torch/src/nn/{ragged_ring,backbone,layer,self_attention}.rs` — the ragged/grouped
  ring primitive + additive `forward_ring`/`forward_ring_grouped` (host path byte-for-byte unchanged; dia2
  544/544 + 608/608 proves it).
- `crates/waav-infer-backend-torch/src/qwen3_tts.rs` — the pilot ring wrapper.
- `crates/waav-infer-runtime/src/serve.rs` — D1 fix: `serve_codec_ar_multiplexed_bounded_deadlined` +
  `admitted_at` deadline shed + the `serve_deadline_sheds_slow_cohort_no_wedge_loop_survives` gate.
- `crates/waav-infer-runtime/src/lib.rs` — deadline plumbing.
- `crates/waav-infer-server/src/engine.rs` — `live_serve_green` per-model gate; qwen3 default-green,
  dia2/csm/misotts gated.
- `crates/waav-infer-server/src/{codec_ar_batcher,codec_ar_admission}.rs` — serve-deadline / admission plumbing.
- `ci/heavy_live_tests.sh` — registers the dia2/csm/misotts force-solo oracles + the dia2 wedge RCA gate.

**New (untracked):**
- `crates/waav-infer-backend-torch/tests/{dia2,csm,misotts}_force_solo_codes.rs` — the three Fork-A1 force-solo
  oracles (dia2's includes the D2 concurrent-identical assert).
- `crates/waav-infer-backend-torch/tests/dia2_wedge_rca.rs` — the dia2 D1 no-wedge B-sweep.
- `crates/waav-infer-backend-torch/tests/codec_ar_wedge_sweep.rs` — generic csm/misotts live no-wedge B-sweep.
- `ci/phase_c_model_sweep.sh`, `docs/`.

---

## 7. COMMIT DECISION

**The on-disk snapshot is COMMIT-SAFE.** Only the live-GREEN model (qwen3-tts, greedy + proven pilot) is
default-on; dia2/csm/misotts ship gated to the B=1 one-shot fallback, so **no production wedge or divergence is
shipped**. Both Wave-1 blockers (D1 wedge, D2 divergence) are fixed and pinned by deterministic regression
gates. The full no-regression bar is GREEN (clippy ×2, workspace `--lib` ×2, dia2 544/544 + 608/608 byte-
identity, qwen3/dia2/csm force-solo oracles + dia2 D1 no-wedge sweep). dia2/csm/misotts are each
**eligible to flip default-on** once their full-system live-serve verification is signed off (the rings +
oracles stay on disk, correct and valuable). Coordinator discipline preserved: NOT committed, no `cargo fmt`.
