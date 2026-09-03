# Path-B Arbitrary-Batch — FOUNDATION + PILOT STATUS (brutal review + RCA + final regression)

**Date:** 2026-06-25. **Host:** GB10 (Grace-Blackwell sm_121, 121 GiB unified CPU+GPU pool), aarch64.
**Tree:** `waav-infer` @ committed HEAD `811e4ef` + the on-disk uncommitted Path-B foundation+pilot
(9 modified files + 3 untracked; left on disk, NOT committed; no `cargo fmt`, per coordinator discipline).
**Plan followed:** `PATHB-ARBITRARY-BATCH-PLAN.md` (§0.3 Fork-A1 default, §1.2 ragged ring, §1.3 ArStepModel
surface, §1.4 serve-loop prereq, §5.2/§5.3 sizing bugs, §8 extreme-TDD gates). **Predecessor:**
`KV-ACCEL-FINAL-STATUS.md` §5 (the serve-loop concurrency fix already landed at `811e4ef`).
**Contract (owner directive):** "optimal perf WITHOUT accuracy loss" ⇒ DEFAULT = **Fork A1** (per-slot B=1
device-resident dispatch, **codes-identical-to-SOLO**, the standing integer `assert_identical` bar — NOT
logits max|Δ|=0, mathematically impossible for a batched GEMM per `cfg_batch.rs:13`). Fork B (fused-[B] 30×,
NOT solo-identical) DEFERRED to opt-in `PerfMode::Throughput`, out of scope here.
**Env:** `source gb10-env.sh`; `free -g` checked before each live gate (≥24 GiB free throughout, no OOM, no
box-kill); live gates `--test-threads=1`, ONE model at a time.

---

## 1. HEADLINE VERDICT — GO. Workspace GREEN, no regression, Fork-A1 codes-identical-vs-solo PROVEN.

The Path-B foundation (sizing-bug fixes + ragged ring) and the qwen3-tts pilot (Fork-A1 ArStepModel over
28 device-resident talker rings) are **correct, byte-identical-vs-solo on real weights, and regression-clean**.
Every named no-regression gate re-ran GREEN this review. The pilot's `step_batch` is — by direct code
inspection AND by the trait contract — the **per-slot `step` loop** (the trait default's exact shape): no
fused reduction snuck in, so the Fork-A1 codes-identity holds BY CONSTRUCTION and is CONFIRMED empirically by
the force-solo oracle (4 rows / 2704 integer codes / max|Δ|=0 on real qwen3 weights, CUDA-f32, re-reproduced
this run).

**The production B=1 one-shot path is UNTOUCHED** (the unwrapped `TorchQwen3Tts` keeps `as_stepped()->None`;
the batched wrapper is reached only behind the `WAAV_QWEN3_BATCHED` env gate). The work is additive below the
backend-free P-8 seam; runtime/serve/driver are byte-for-byte unchanged.

---

## 2. FINAL REGRESSION — what is GREEN (re-verified THIS review)

### 2.1 Build / lint — both feature configs (forced recompile of the touched crates)

| Check | Result |
|---|---|
| `cargo clippy --workspace --all-targets -- -D warnings` (default) | **GREEN** |
| `cargo clippy --workspace --all-targets --features torch -- -D warnings` (after `touch` of qwen3_tts/ragged_ring/engine — real recompile) | **GREEN** |

### 2.2 Deterministic workspace suites (`cargo test --workspace -- --test-threads=1`)

| Suite | passed | failed | ignored |
|---|---|---|---|
| default features | **691** | **0** | 11 |
| `--features torch` | **1192** | **0** | 183 |

(+8 vs FINAL-STATUS's 1184: the 3 new scheduler sizing gates + the 4 new `ragged_ring` CPU-f32 layout unit
tests + 1. The default-features count is lower than 1184 because the torch backend suite — ~210 tests —
compiles only under `--features torch`; that is the config that exercises the new code.)

**New deterministic gates (all GREEN):**
- `gqa::tests::kv_footprint_includes_n_layers` (BUG-1) — per-slot ring is EXACTLY n_layers× the per-layer
  watermark; box-kill witness shows the layer-less count's REAL ring overshoots the 48 GiB arena, the
  layer-aware count fits.
- `gqa::tests::kv_footprint_dtype_correct_bytes` (BUG-2) — F16=2 / F32=4; fp32 ring is EXACTLY 2× fp16;
  box-kill witness shows the fp16-assumed count's REAL f32 ring overshoots, the dtype-correct count fits.
- `gqa::tests::admission_never_exceeds_free_mem` (§5.2+§5.3) — budget = min(arena, free); arena-caps a 96 GiB
  free reading; arena/2 fallback on `None` (NEVER total_mem); admitted worst-case ring ≤ budget and one more
  slot would exceed it (tight); compute knee still min()-tightens; overhead ≥ budget admits 0 (shed, no
  underflow).
- `nn::ragged_ring::tests::{per_row_heads_are_independent, row_read_back_matches_solo_kvcache,
  full_masked_row_matches_solo, recycle_zero_is_clean_and_bit_identical}` — the ragged ring's per-row read-back
  is BYTE-IDENTICAL (max|Δ|=0) to a solo `KvCache` for the same write sequence, per-row heads are independent,
  recycle-zero is clean.
- The pre-existing `gqa_native_no_kv_head_replication` + `gqa_native_ar_compounding_identical` gates stayed
  GREEN (the default-field sizing fixes did not perturb the H3 watermark math — see §4.3).

### 2.3 Live byte-identity gates (CUDA, process-isolated, ONE model at a time)

| Gate | Crate / target | Result | Wall |
|---|---|---|---|
| **dia2** `cpu_fp32_codes_byte_identical` (**544/544**) | backend-torch `--test cuda_torch_dia2` | **GREEN** | 70.9 s set |
| **dia2** `cuda_bf16_codes_byte_identical` (**608/608**) | backend-torch | **GREEN** | (same set) |
| **dia2** envelope / ASR-sidecar parity | backend-torch | **GREEN** | (3/3) |
| **csm** `cuda_csm_codes_byte_identical_to_sidecar` (dual-AR greedy) | backend-torch `--test cuda_torch_csm` | **GREEN** | 47.3 s set |
| **csm** `cuda_csm_rtf` | backend-torch | **GREEN** | (2/2) |
| **qwen3** `qwen3_tch_force_solo_codes_identical_ragged` (**THE Fork-A1 long-pole oracle**, CUDA-f32) | backend-torch `--test qwen3tts_force_solo_codes` | **GREEN** | 32.4 s |
| **chatterbox** `host_vs_device_kv_oracle` (ORT device-KV serve path, 811e4ef) | core `--lib` | **GREEN** | 88.3 s |

The qwen3 oracle re-printed live this run: **4 rows (12/51/35/71 frames), 2704 integer codes, max|Δ|=0**,
frame counts exact-match (a flipped code would truncate the AR tail). This is THE first-and-only real-weights
evidence Fork A1 holds, independently reproduced.

dia2 (608/608 + 544/544) and csm exercise the shared `nn/` files I touched (the additive `forward_ring`
methods on `backbone.rs`/`layer.rs`/`self_attention.rs`) — their byte-identity proves the additions did NOT
perturb the existing host path.

### 2.4 No-regression gates NOT re-run, with explicit justification

`live_ragged_batched_forward_bit_identical_and_scales` (production host-KV, 311 s) and the broader Phase-0..7
ORT set were **re-verified GREEN at HEAD 811e4ef immediately before this work** (KV-ACCEL-FINAL-STATUS §2.3).
**The Path-B diff touches ZERO core/ORT files** (verified: the 9 changed files are all backend-torch /
scheduler / server-engine / ci — `git diff --name-only HEAD | grep -E 'core|backend-ort'` returns NONE). The
`host_vs_device_kv_oracle` spot-check above (GREEN, 88 s) confirms the ORT chatterbox serve path is unaffected;
re-running the 311 s heavy gate would only re-confirm an untouched code path. The qwen3 CPU-f32 oracle cell
(pre-work: PASS 438 s) was not re-run — the CUDA-f32 cell (GREEN this run) and the 4 deterministic CPU-f32
`ragged_ring` layout twins (GREEN this run) cover the same LAYOUT + dispatch correctness.

---

## 3. BRUTAL SCRUTINY — the directive's five questions, answered

### 3.1 "Is Fork-A1 TRULY solo-identical, or did a fused reduction sneak in?" → TRULY solo-identical.

**Confirmed three ways:**
1. **`step_batch` is literally the per-slot loop.** `TorchQwen3TtsBatched::step_batch` (`qwen3_tts.rs` `pub mod
   ring`) is `for input in inputs { out.push(self.step(input)?); }` — byte-for-byte the `arstep.rs:527` trait
   default. The batch index NEVER enters a tensor op; every row is an independent B=1 dispatch.
2. **`forward_ring` keeps the query B=1.** `Attention::forward_ring` `debug_assert_eq!(b, 1, …)` and reuses the
   EXACT host `forward` code: same `self.project`, same `apply_rope`, same `run_kernel(…, is_prefill, seq, b)`,
   same `ProjPrec` o-projection. The ONLY difference vs the host path is the KV SOURCE (a `RaggedSlotRing` row
   vs a per-stream `KvCache`); the per-row read-back returns the SAME `[1,kvh,len,d]` tensor the solo read-back
   returns (the `row_read_back_matches_solo_kvcache` unit gate proves max|Δ|=0). Because B=1, `run_kernel`'s
   `KernelPolicy::attn_kernel` picks the SAME backend as solo → byte-identical SDPA, no reassociated reduction.
3. **`step_frame` mirrors the solo `generate_codes_with_spk` per-stride math line-for-line** (verified): same
   `sample_talker_cb0(logits, history, step, suppress, greedy=true)` (greedy LAW), same cb0-history push, same
   5-layer CodePredictor sub-talker with a per-frame-reset transient `KvCache` (a fresh `KvCache::new` ==
   solo's `c.reset()`), same 16-codebook `codec_sum_frame` next input, same `trailing_text_hidden[step]` vs
   `tts_pad_embed` selection, same talker advance. The `st.step` counter increments after the talker advance,
   exactly as solo's loop variable does.

**The oracle closes it empirically:** max|Δ|=0 on 2704 integer codes vs the public `generate_codes(text,
greedy=true)`, on a ragged staggered MID-FINISH cohort (a SHORT row finishes via eos while LONGER rows keep
decoding). This is the literal Fork-A1 bar.

### 3.2 "Ragged ring per-row seqlens_k correctness" → CORRECT (4 CPU-f32 gates + the live oracle).

`RaggedSlotRing` (`nn/ragged_ring.rs`) replaces `KvCache`'s scalar `self.cur` with `seqlens_k: Vec<i64>` (one
write head per row), alloc-once at `[max_slots, kvh, max_seq, d]`. `write_row` does a nested
`narrow(0,slot,1).narrow(2,cur,q)` then `copy_` — a stride-correct MUTABLE VIEW of the parent ring storage, so
the in-place write aliases the ring (the docstring's claim vs `index_copy_`-on-a-narrow not aliasing in tch
0.20). **Empirically validated:** `per_row_heads_are_independent` proves writing slot 0 leaves slot 1's head at
0 AND a cumulative re-read of slot 0 still shows the earlier write (so `copy_` really wrote through);
`row_read_back_matches_solo_kvcache` interleaves a noise write into slot 4 and proves slot 2's ragged read-back
is max|Δ|=0 vs a solo `KvCache`; `full_masked_row_matches_solo` proves the per-row left-justified `finfo.min`
mask matches solo for the row's own length; `recycle_zero_is_clean_and_bit_identical` proves the device-memset
recycle makes a re-used slot decode identically to a fresh ring. The live oracle's MID-FINISH cohort (a slot
recycling/finishing while others continue) is the end-to-end proof on real weights.

### 3.3 "The sizing-bug fixes — no admission regression?" → CLEAN (additive, default-preserving).

- **BUG-1 (layer-less footprint):** `KvFootprint` now carries `n_layers` **defaulting to 1**; `total_values`/
  `total_bytes` scale by it; the new `KvHeadLayout::per_slot_ring_bytes(max_seq, n_layers, dtype)` is the
  layer-correct per-slot ring estimate.
- **BUG-2 (dtype-blind):** added `KvDtype{F16=2, F32=4}` + `kv_elem_bytes` field **defaulting to
  KV_ELEM_BYTES=2**; `total_bytes` multiplies by the real serving precision; F32 reports EXACTLY 2× F16.
- **96-vs-48 GiB reconciliation:** new `KvAdmissionBudget::reconcile(arena_cap, free_mem: Option, per_slot,
  overhead)` = `min(arena, free)` when free is known, conservative `arena/2` on `None` (NEVER total_mem),
  saturating overhead (admits 0, never underflow); `max_slots(knee) = ⌊ring_budget / per_slot⌋ min knee`.

**No-regression argument (verified):** the only NON-test consumers of the footprint math are `ring_kv.rs:128`
(`l.footprint(self.context)`) and `admission.rs` (`footprint(context).total_bytes()`) — both call
`KvHeadLayout::footprint` → `KvFootprint::of(...)`, which sets the new fields to their DEFAULTS (n_layers=1,
kv_elem_bytes=2). So every existing H3-watermark / KV-length-firewall / ring-residency value is byte-for-byte
unchanged, and the pre-existing `gqa_native_*` gates stayed GREEN. The new `with_layers`/`with_dtype`/
`per_slot_ring_bytes`/`KvAdmissionBudget` API has **zero non-test consumers yet** (the live admission wiring is
a future Phase-0 step, NOT this work) — so the fixes add typed capability without touching any live admission
decision. **The box-kill witnesses are real:** each gate constructs the buggy slot count and asserts its REAL
ring overshoots the 48 GiB arena while the corrected count fits.

### 3.4 "Serve-loop tch containment" → NOT modified this work; inherited from 811e4ef.

The serve-loop concurrency hardening (the §1.4 HARD PREREQUISITE: graceful typed shed, no `codec-ar-mux` thread
crash at n≥16) **already landed at HEAD 811e4ef** (KV-ACCEL-FINAL-STATUS §5). This work did NOT touch the
serve loop. The pilot is wired behind `WAAV_QWEN3_BATCHED` (default UNSET → the unwrapped model keeps
`as_stepped()->None`), so the production B=1 one-shot path is the default and the batched device serve path is
opt-in. Per the pre-work's live serve testing, the 811e4ef hardening held through every width (server stayed
ALIVE: no mux-thread crash, no 500, no OOM; B=64 cleanly sheds typed 429; over-deadline cohorts shed typed
`stall_timeout` 503).

### 3.5 "qwen3-tts B=1 path unchanged?" → UNCHANGED (env-gated wrapper, additive).

`engine.rs`: the `qwen3_tts` load arm wraps in `TorchQwen3TtsBatched` ONLY when `WAAV_QWEN3_BATCHED` is set;
otherwise it returns the unwrapped `TorchQwen3Tts` (the gate-stamped solo path). The wrapper's `TtsModel`
one-shot verbs (`synthesize`/`synthesize_cloned`/`voices`/…) DELEGATE to the inner model byte-for-byte; the
ONLY new behavior is `as_stepped()->Some(self)`. The solo `generate_codes`/`generate_codes_with_spk` functions
are untouched. (The engine arm also adds a `read_torch_inprocess_runtime` route for the TTS dir so an
in-process-only arch loads through the libtorch loader instead of falling to the ORT registry — additive, B26,
matches the existing `load_model_at` S2S path.)

---

## 4. MEASURED GO / NO-GO

**GO.** The decision rests on three measured facts:

1. **Correctness (the hard bar):** Fork-A1 is codes-identical-to-solo on real qwen3 weights — oracle GREEN
   (max|Δ|=0, 2704 integer codes), re-reproduced this review. This is the only bar that gates shipping under
   the "no accuracy loss" directive, and it is met.
2. **Perf (the pre-work measurement, NOT re-measured this review):** Fork A1 BEATS Path-A's ~1.8× host-KV cap
   on the real growing-KV bf16 decode loop — crossover at B=4 (2.02×), **peak 2.61× at B=12**
   (device-residency class, matching the predicted ~2.34× device-residency win, explicitly NOT the 30× that
   belongs to the deferred Fork B). This clears the PATHB §7.1 Phase-3 go/no-go threshold ("Fork A1 must beat
   Path-A's ~1.8× or it is a correctness move with no perf upside").
3. **Resilience:** codes byte-identical-to-solo live through B=24 (full MAX_SLOTS); B=64 cleanly sheds typed
   429; over-deadline cohorts shed typed `stall_timeout` 503; server stayed ALIVE through every width (811e4ef
   hardening held). No box-kill, no OOM, no mux crash.

**Honest caveat (carried from the plan + pre-work):** the realized win is **~2.6× device-residency**, NOT 30×.
A ~30 s per-cohort latency wall plus single-mux head-of-line at the highest widths / longest texts is the
remaining Phase-3+ hardening boundary (survivable typed shed, not a crash). The 30× headline belongs to the
DEFERRED Fork B (fused-[B], batched-vs-batched-only, opt-in `PerfMode::Throughput`) and is NOT the
byte-identical-vs-solo path.

---

## 5. ROLLOUT DECISION

**Ship the foundation; keep the pilot env-gated; fan out per-arch behind its own force-solo oracle.**

1. **Foundation (sizing fixes + ragged ring) — READY to land.** The three sizing-bug fixes are pure-data,
   default-preserving, and box-kill-witnessed; the ragged ring is layout-proven (4 CPU-f32 gates) + live-proven
   (the oracle). These are the fork-agnostic substrate every future tch batcher needs. No live admission
   consumer is wired yet (intentional — that is the next Phase-0 step), so landing them changes no runtime
   behavior.
2. **qwen3-tts pilot — STAYS behind `WAAV_QWEN3_BATCHED` (default OFF).** Per PATHB §1.4/§8.3, the
   `as_stepped()->Some` flip rides the multiplexed serve loop; the production default remains the B=1 one-shot
   path until the serve-loop graceful-shed gate is institutionalized as a standing CI gate (the 811e4ef
   hardening is in place but should be wired into `heavy_live_tests.sh` as a named `serve_loop_graceful_shed`
   gate before the env gate is flipped to default-on for any deployment).
3. **Fan-out (Phase 4+) — one arch at a time, each gated by its own `<arch>_batched_vs_force_solo_codes_oracle`
   BEFORE it serves B>1.** The reusable pattern is proven (the qwen3 oracle is the template). dia2/csm/misotts
   are PARTIAL (depformer per-slot, separate lever). fp16 fleet (higgs/higgs_v2/voxtral/cohere/ark) needs its
   OWN fp16 force-solo oracle (zero in-tree fp16 batched-vs-solo measurement exists). The bf16-floor cells stay
   Fork-A1 (per-row dispatch) until/unless Fork-A2 f32-accumulate SDPA is proven.
4. **Fork B / 30× — DEFERRED to `PerfMode::Throughput`,** batched-vs-batched-only, NOT this contract.

**Residual observations (non-blocking, for the fan-out):**
- The ring prefill passes an explicit `0..l` positions slice while the solo prefill passes `&[]`; for qwen3
  this is INERT because the talker's `rope_apply == RopeApply::Start` (uses `pos` only, ignores `positions`).
  An arch whose `rope_apply` is `Positions`/`InterleavedFull` would diverge — so each `forward_ring` caller in
  the fan-out MUST pass the positions slice matching its solo convention (and its oracle would catch a mismatch).
- `ContiguousMasked` ring read-back is `unimplemented!()` (typed, never silently wrong) — dia (its sole
  consumer) uses `KvCache`, not the ring, so this is correct for the pilot; the dia fan-out must wire it.
- Codec decode is still per-slot (`decode_audio`, no `decode_audio_batch`); the decode tail is not yet
  pipelined/transient-budgeted (PATHB §4) — a Phase-6 item, and a box-kill risk to close before any heavy
  whole-body codec serves >1 slot concurrently.

---

## 6. FILES (on disk, NOT committed; no `cargo fmt`)

**Modified (9, uncommitted vs HEAD `811e4ef`):**
- `crates/waav-infer-scheduler/src/gqa.rs` — BUG-1 (n_layers), BUG-2 (KvDtype + kv_elem_bytes),
  `KvAdmissionBudget::reconcile`/`max_slots`, + 3 RED-first gates.
- `crates/waav-infer-scheduler/src/lib.rs` — re-export `KvAdmissionBudget`, `KvDtype`.
- `crates/waav-infer-backend-torch/src/nn/ragged_ring.rs` — `RaggedSlotRing` (NEW file, untracked actually).
- `crates/waav-infer-backend-torch/src/nn/mod.rs` — `pub mod ragged_ring` + re-export.
- `crates/waav-infer-backend-torch/src/nn/self_attention.rs` — `Attention::forward_ring` (B=1 per-row dispatch).
- `crates/waav-infer-backend-torch/src/nn/layer.rs` — `TransformerLayer::forward_ring`.
- `crates/waav-infer-backend-torch/src/nn/backbone.rs` — `Backbone::forward_ring`.
- `crates/waav-infer-backend-torch/src/qwen3_tts.rs` — `pub mod ring` (`TorchQwen3TtsBatched` ArStepModel +
  TtsModel surface, `step_frame`, Fork-A1 `step_batch` = per-slot loop).
- `crates/waav-infer-server/src/engine.rs` — `WAAV_QWEN3_BATCHED` env-gated wrapper + in-process TTS-dir route.
- `ci/heavy_live_tests.sh` — registers the qwen3 force-solo oracle gate.

**New (untracked):**
- `crates/waav-infer-backend-torch/src/nn/ragged_ring.rs`,
  `crates/waav-infer-backend-torch/tests/qwen3tts_force_solo_codes.rs`, `ci/phase_c_model_sweep.sh`, `docs/`.

**Model cache:** `qwen3-tts-12hz-06b/waav.json` intact (runtime backend=torch, architecture=qwen3_tts).

**Regression logs (this review):** `scratchpad/{scratch_torch_test.log, scratch_dia2.log, scratch_csm.log,
scratch_oracle_cuda.log, scratch_hostdev.log}` + the task output files.
