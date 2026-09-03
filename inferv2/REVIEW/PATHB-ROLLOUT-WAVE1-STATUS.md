# Path-B Arbitrary-Batch — ROLLOUT WAVE-1 STATUS (brutal review + deep RCA + final regression)

**Date:** 2026-06-26. **Host:** GB10 (Grace-Blackwell sm_121, 121 GiB unified CPU+GPU pool), aarch64.
**Tree:** `waav-infer` @ committed HEAD `302e9c8` (the qwen3-tts Fork-A1 pilot + Path-B foundation) + the
on-disk **uncommitted Wave-1** (10 modified tracked files + 4 untracked; left on disk, NOT committed, no
`cargo fmt`, per coordinator discipline).
**Plan followed:** `PATHB-ARBITRARY-BATCH-PLAN.md` §3.1 (per-arch AR lockstep / CFG-axis / dual-AR), §6.1
(rollout matrix), §8 (extreme-TDD gates + no-regression). **Predecessor:** `PATHB-BATCH-PILOT-STATUS.md`
(qwen3-tts pilot GO).
**Contract (owner directive):** Fork A1 = per-slot **B=1 device-resident ring dispatch**,
**codes-identical-to-SOLO** (integer `assert_identical`, max|Δ|=0), precision-agnostic (the batch/slot index
never enters a reduction). Fork B (fused-[B] 30×) is NOT this work.

---

## 1. HEADLINE VERDICT — SPLIT GO.

**qwen3-tts: GO (production-ready).** **csm: CONDITIONAL GO (correctness GREEN; serve-loop hardening needed
before default-on).** **dia2 + misotts: NO-GO for production default-on as flipped** — the Fork-A1 *unit*
correctness is GREEN, but two live-serve defects (a sampled-model concurrency divergence on dia2 + a permanent
serve-loop admission-slot wedge on dia2 AND misotts) are blockers at the **default-on-for-CUDA** gating these
flips ship.

The **no-regression bar is GREEN across the board** — the additive Wave-1 work did NOT perturb a single
existing byte-identical gate. The blockers are NOT regressions in the existing solo paths; they are properties
of the **newly-flipped batched serve path** under live multiplexed concurrency.

| Model | Fork-A1 unit oracle (codes==solo) | Production flip (engine.rs) | Live-serve verdict |
|---|---|---|---|
| **qwen3_tts** | GREEN (2704 codes, max\|Δ\|=0, re-run this review) | default-on CUDA | **GO** — B1..8 byte-identical, B16 clean shed, recovers |
| **csm** | GREEN (9568 codes, max\|Δ\|=0 — prior run) | default-on CUDA | **CONDITIONAL** — B1..4 byte-identical, B≥8 clean shed/recover (no wedge) |
| **dia2** | GREEN (CPU-f32 6176 codes; CUDA-bf16 5472 codes; max\|Δ\|=0, CPU cell re-run this review) | default-on CUDA | **NO-GO** — B=2 stochastic divergence + B≥16 PERMANENT WEDGE |
| **misotts** | GREEN (4128 codes, max\|Δ\|=0 — prior 488s run) | default-on CUDA | **NO-GO** — B≥16 PERMANENT WEDGE (same defect as dia2) |

---

## 2. FINAL REGRESSION — re-verified THIS review (GREEN)

### 2.1 Build / lint — both feature configs, FORCED recompile of the touched crates

| Check | Result |
|---|---|
| `cargo clippy --workspace --all-targets -- -D warnings` (default) | **GREEN** |
| `cargo clippy --workspace --all-targets --features torch -- -D warnings` (after `touch` of dia2/csm/misotts/ragged_ring/engine — REAL recompile of backend-torch + server) | **GREEN** |

> The build-notes flag of "3 dia2.rs IndexOp errors from a concurrent agent's in-flight edit" is **RESOLVED on
> the current on-disk snapshot** — the forced recompile is clean.

### 2.2 Deterministic workspace `--lib` suites (`--test-threads=1`)

| Suite | Result |
|---|---|
| `cargo test --workspace --lib` (default) | **all GREEN, 0 failed** |
| `cargo test --workspace --lib --features torch` | **all GREEN, 0 failed** |

New/touched deterministic gates GREEN: `nn::ragged_ring::tests::{full_masked_group_matches_solo_cfg2_kvcache,
full_masked_row_matches_solo, per_row_heads_are_independent, recycle_zero_is_clean_and_bit_identical,
row_read_back_matches_solo_kvcache}` (5) + `dia2::ring::ring_tests::dia2_grouped_ring_readback_matches_solo_cfg2`
(the grouped CFG-2 layout twin). The grouped read-back (`append_full_masked_group`) is BYTE-IDENTICAL
(max|Δ|=0) to a solo `KvCache::new(2,…)` for the same write sequence.

### 2.3 Live byte-identity / oracle gates (CUDA, process-isolated, ONE model at a time)

| Gate | Result (this review) | Notes |
|---|---|---|
| **dia2** `cpu_fp32_codes_byte_identical` (**544/544**) | **GREEN** | CRITICAL — dia2 flipped + shared `nn/forward_ring*` touched |
| **dia2** `cuda_bf16_codes_byte_identical` (**608/608**) | **GREEN** | (same target, 60.7s set) |
| **dia2** envelope/ASR-sidecar parity | **GREEN** | (3/3 in the cuda_torch_dia2 set) |
| **qwen3** `qwen3_tch_force_solo_codes_identical_ragged` (CUDA-f32) | **GREEN** | 4 rows / 2704 codes / 12,51,35,71 frames / max\|Δ\|=0 |
| **dia2** `dia2_tch_force_solo_codes_identical_ragged` (CPU-f32) | **GREEN** | 4 rows / 6176 codes / 15,65,26,87 frames / max\|Δ\|=0 (416s) |
| **csm** `cuda_csm_codes_byte_identical_to_sidecar` | **RED — PRE-EXISTING (NOT a Wave-1 regression)** | see §2.4 |

### 2.4 The one RED gate is PROVEN PRE-EXISTING (RCA, not a Wave-1 regression)

`cuda_csm_codes_byte_identical_to_sidecar` FAILS (3150/4000 codes differ; first divergence frame0 / cb16 / the
**depth decoder**). **Proven pre-existing this review by `git stash`-ing all 10 Wave-1 tracked edits and
re-running at the clean committed HEAD `302e9c8`**, where it fails **identically** (same 3150/4000, same
first-div `(0,16,158→435)`). This is **golden-staleness at the csm depth-decoder cb16**, independent of the
Path-B flip:

- csm Wave-1 diffs are purely **additive** (`341/0`, zero deletions); the solo `generate_codes` path is
  byte-for-byte untouched.
- The Wave-1 **force-solo oracle** (`csm_tch_force_solo_codes_identical_ragged`, batched-vs-solo within the
  same engine) is the correct Fork-A1 accuracy bar and is **GREEN** (9568 codes, max|Δ|=0). The stale
  sidecar-golden gate compares against an EXTERNAL reference and is a separate, pre-existing defect.

### 2.5 Gates NOT re-run, with justification

- **host-KV / chatterbox ORT (`host_vs_device_kv_oracle`, `live_ragged_batched_forward_bit_identical_and_scales`):**
  the Wave-1 diff touches **ZERO core / backend-ort / runtime / scheduler files** (`git diff --name-only HEAD`
  over those crates returns NONE — all 10 modified files are backend-torch / server-engine / ci). These ORT
  gates exercise a code path Wave-1 provably does not modify; both were GREEN at HEAD `811e4ef`/`302e9c8`
  immediately before this work.
- **misotts force-solo oracle (488s) / csm force-solo oracle (CUDA):** GREEN in the prior `verifies` run; not
  re-run here to bound the GB10 serialized-GPU surface (each is single-model-at-a-time, mem::forget-leaking).
  The qwen3 + dia2 CPU-f32 oracle cells re-run this review independently confirm the shared `RaggedSlotRing` +
  `forward_ring` substrate is correct.

---

## 3. WHICH MODELS ARE NOW ARBITRARY-BATCH IN PRODUCTION

All four engine.rs arms were flipped from env-gated to **default-on for device-resident-capable (CUDA) HW**:
`use_ring = override_batched.unwrap_or_else(|| device.is_cuda())`. `WAAV_<MODEL>_BATCHED` became an OVERRIDE
(`0|off|false|no` forces solo even on CUDA; `1|on` forces ring even on CPU). CPU / unsupported falls back to
the unwrapped model (`as_stepped()->None`, the gate-stamped B=1 one-shot — **no-regression on every non-CUDA
target, verified**). Each batched wrapper has a `TtsModel` impl delegating the one-shot verbs +
`as_stepped()->Some`.

| Model | Arch class | Coverage | Production status |
|---|---|---|---|
| **qwen3_tts** | codec-AR (dual-cb), greedy LAW | FULL backbone ring | **SHIP** — Fork-A1 GREEN, live B1..8 byte-identical, B16 clean shed |
| **csm** | DUAL-AR (16L Llama backbone + 4L depth) | **PARTIAL** (backbone ring; depth + Mimi PER-SLOT) | **HOLD** for serve-loop gate — Fork-A1 GREEN, live ≤B4 byte-identical, B≥8 clean shed (no wedge) |
| **dia2** | CFG-axis codec-AR (2-branch grouped ring + 31-stage depformer), **SAMPLED** | **PARTIAL** (backbone grouped ring; depformer PER-SLOT) | **REVERT default-on** — two live blockers (§4) |
| **misotts** | DUAL-AR (32L Llama-8B backbone + 8L depth) | **PARTIAL** (backbone ring; depth + Mimi PER-SLOT) | **REVERT default-on** — permanent wedge (§4) |

**Architecture wrinkles delivered (REUSE notes for the remaining fleet):**
- **CFG-axis grouped ring (dia2):** `RaggedSlotRing::append_full_masked_group/write_group/reset_group/group_seqlen`
  reserve `group=branches` contiguous rows per logical slot; the SLOT axis never reduces, only the fixed CFG-2
  (dia2's established 608/608 golden) does. `Backbone::step_ring → forward_ring_grouped`. Layout twin GREEN.
- **Dual-AR backbone-ring + per-slot depth (csm/misotts):** the Llama backbone rides the ring; the depth
  decoder + Mimi codec stay per-slot (transient KvCache reset per frame == solo). PARTIAL per §3.1 — realized
  cohort speedup is depth-decoder-bound (~3% backbone lever per G6).
- **Positions/mask conventions (the §5 residual):** csm passes the absolute-positions slice
  (`CacheRead::Contiguous`); misotts passes both the positions slice AND rebuilds the external causal mask each
  forward (`InterleavedFull` RoPE + `FusedMaskedExpand`). Both verified by their force-solo oracles.

---

## 4. THE TWO LIVE-SERVE BLOCKERS (deep RCA) — why dia2/misotts must not default-on as flipped

### 4.1 dia2 SAMPLED-MODEL concurrency divergence (the per-(slot,step) re-seed)

dia2 is **sampled** (temperature>0), not greedy. `TorchDia2Batched::step_frame` re-seeds the global libtorch
RNG per stride to `SEED + slot*1_000_003 + step` (dia2.rs:1776) — by DESIGN, to make each ring row's draws
**cohort-independent** (a row's RNG keyed only on `(slot,step)`, never on which other slots are co-resident).

**The unit oracle is GREEN BECAUSE it tests exactly this property:** `solo_frames()` decodes each row ALONE
**on the same logical slot index** (`max_slots = slot+1`, drive slot `i`), so batched-row-`i` == solo-row-`i`
under the matched re-seed (cohort-independence at a FIXED slot). That is a real, valuable correctness result —
no cross-slot bleed, the ring layout is sound.

**But it is NOT "identical input → identical audio."** In the live serve loop two identical concurrent requests
land on DIFFERENT slots (slot 0, slot 1); the `slot*1_000_003` re-seed gives them DIFFERENT RNG streams →
DIFFERENT (each individually valid) audio. This is the `sys` live finding "dia2 B=2: only 1/2
byte-identical-to-solo" (one stream landed on slot 0, matching the single-`manual_seed(SEED)` solo; the other
on slot 1, a different stream). **Assessment:** defensible for a *stochastic* TTS model (the reference engine
also varies run-to-run), but it is a behavior change vs the deterministic single-seed solo path, and it is the
reason the dia2 force-solo oracle's contract is "cohort-independence at a fixed slot," NOT "concurrent-identical
== identical." This must be RATIFIED (or the re-seed law replaced with per-slot RNG-generator state once tch
exposes it) before dia2 ships default-on. qwen3/csm/misotts are GREEDY (argmax, RNG-free) and are NOT subject
to this — their concurrent-identical outputs ARE identical.

### 4.2 dia2 + misotts PERMANENT serve-loop WEDGE at B≥16 (the headline blocker)

Both are SLOW solo (dia2 14.6s, misotts 10.8s for ~12s audio — 31-stage depformer / Llama-8B). Because Fork-A1
`step_batch` is the per-slot SERIAL loop (correct by contract — the batch index must not reduce), a B≥4 cohort's
serial work blows the 30s synth deadline and sheds typed 503. That alone is acceptable (graceful shed). **The
defect:** at B=16 the codec-AR-mux **stops advancing permanently** — 21-24 admission slots **leak** (metric
`codec_ar_inflight_streams` stuck ~21-24; the J15 leak watchdog logs `"channels charged but never freed
leaked=N"` hundreds of times; `cohort_count` frozen). Every subsequent request — even a fresh solo, even after
the backlog "drains" — returns typed 503 at 30s. The process survives (`readyz=200`, no mux crash) but the TTS
path is **denied-of-service until restart**, and a **SIGSEGV (exit 139)** fires on shutdown teardown of the
wedged state.

**RCA:** this is a **serve-loop admission slot-accounting defect** (charge/free imbalance when a slow cohort is
sheds mid-flight), AMPLIFIED by slow models, NOT a `step_batch` correctness bug (`step_batch` is provably the
per-slot loop; the force-solo oracle is GREEN). It is backend-agnostic (the single-mux-thread + admission ledger),
exactly the §1.4 HARD-PREREQUISITE the plan flagged: *"the multiplexed batched-DEVICE serve branch must be
hardened (graceful typed shed, NOT a leak/crash) and proven green at n=2/8/16/32 BEFORE any tch
`as_stepped()->Some` flip."* The 811e4ef hardening held for the FAST qwen3 (B16 clean shed + recover) but does
NOT cover the slow-model + mid-flight-shed leak path. qwen3 and csm did NOT wedge (qwen3 recovers at B16; csm
recovers at B16) — the wedge is specific to dia2/misotts's slow per-cohort serial work crossing the deadline
while admission charges are outstanding.

**Therefore the §1.4 prerequisite is NOT yet satisfied for the slow models, and they were flipped default-on
ahead of it.** The fix is a serve-loop / admission item (idempotent slot-free on shed + a `serve_loop_graceful_shed`
standing gate at n=2/8/16/32 incl. slow models), NOT a per-model ring change.

---

## 5. BRUTAL SCRUTINY — the directive's questions, answered

1. **"Each flipped model's codes-identical-to-solo — truly per-row B=1, no fused reduction?"** → **YES.** Every
   `step_batch` is verbatim the per-slot `step` loop (the `arstep.rs:527` trait-default shape); the batch/slot
   index never enters a tensor op. dia2's grouped ring reduces ONLY the fixed CFG-2 axis (its established
   608/608 golden), never the slot axis. Confirmed by code inspection AND the qwen3 + dia2 force-solo oracles
   GREEN (max|Δ|=0).
2. **"Production-flip gating — B=1 fallback intact on unsupported HW?"** → **YES.** All four arms gate on
   `device.is_cuda()` with the override; CPU/unsupported returns the unwrapped model (`as_stepped()->None`).
   The unwrapped solo path is byte-for-byte the gate-stamped one (dia2 544/544 + 608/608 GREEN proves it). The
   one concern is the **default-on-for-CUDA** decision itself for dia2/misotts (§4) — the *mechanism* is sound,
   the *default policy* is premature for the two slow/sampled models.
3. **"Per-arch ring correctness (depformer / dual-AR)?"** → **CORRECT but PARTIAL.** Backbone rides the ring;
   depformer (csm/dia2/misotts) + Mimi codec stay per-slot (transient reset per frame == solo). Realized
   speedup is depth-decoder-bound (G6/§7.4) — do NOT report backbone-ring as full coverage. Layout twins +
   oracles GREEN.
4. **"No mux-crash?"** → **No mux-thread CRASH** (process stays `readyz=200` through every width), BUT dia2 +
   misotts exhibit a **permanent admission-slot WEDGE + teardown SIGSEGV** at B≥16 (§4.2) — a denial-of-service,
   functionally as bad as a crash for the TTS path until restart.

---

## 6. ROLLOUT DECISION

1. **qwen3-tts — SHIP default-on (CUDA).** Fork-A1 GREEN, live B1..8 byte-identical, B16 clean typed shed +
   recovers, no wedge. The proven template.
2. **csm — KEEP the ring code; HOLD default-on behind a serve-loop gate.** Fork-A1 GREEN; live ≤B4
   byte-identical, B≥8 clean shed and **recovers** (no wedge observed). Flip to default-on once
   `serve_loop_graceful_shed` is a standing gate. (Independent pre-existing issue: refresh the stale
   `cuda_csm_codes_byte_identical_to_sidecar` golden — §2.4 — not a Path-B blocker.)
3. **dia2 — REVERT to env-gated (`WAAV_DIA2_BATCHED` opt-in), do NOT default-on.** Two blockers: (a) the
   SAMPLED concurrency divergence (§4.1) — ratify the per-(slot,step) re-seed semantics OR move to per-slot RNG
   generator state; (b) the permanent serve-loop wedge (§4.2). Ring code + oracle stay (correct, valuable).
4. **misotts — REVERT to env-gated (`WAAV_MISOTTS_BATCHED` opt-in), do NOT default-on.** Same permanent wedge
   (§4.2). Ring code + oracle stay.
5. **Serve-loop hardening is the gating prerequisite (§1.4) and is the next must-do** before ANY slow tch model
   defaults on: idempotent admission slot-free on mid-flight shed + a `serve_loop_graceful_shed_n_2_8_16_32`
   standing gate that includes a SLOW model, + root-cause the teardown SIGSEGV on the wedged state.

**Honest perf note (carried from `sys`):** live concurrency does NOT yield wide-cohort throughput — the
codec-AR-mux serializes (cohort-width histogram dominated by width 1-2; qwen3 B=8 ≈ 8× solo, speedup-vs-serial
only ~1.47×; csm/misotts ~1.0×). The realized Wave-1 win is **correctness (byte-identical-to-solo for the
greedy models) + device-residency**, NOT live throughput scaling. Concurrent REST requests are not coalesced
into one wide batched step per tick (head-of-line single-mux) — a separate serve-loop lever, out of Wave-1
scope.

---

## 7. REMAINING FLEET (per PATHB §6, after Wave-1)

**Wave-1 done (codec-AR lockstep, this review):** qwen3_tts (SHIP), csm (HOLD), dia2 (REVERT), misotts (REVERT).

**Next lockstep-AR (Phase 4 fan-out, each needs its own force-solo oracle):**
- bf16 codec-AR: dia (CFG, `append_contiguous_masked` MATH-SDPA, not graphable), s2_pro (dual-AR 36L+4L),
  neutts (tch backbone + ONNX codec), zonos2 (**MoE** — dedicated mask-all-experts oracle, EDA router per-row).
- **fp16 fleet (own fp16 force-solo oracle — ZERO in-tree fp16 batched-vs-solo measurement exists):** higgs,
  higgs_v2 (DualFFN per-modality routing).

**Hybrid AR + flow/diffusion (Phase 5, two batchers — AR ring + step-bucket):** cosyvoice3, voxtral_tts, dots,
indextts2 (GPT-2, f32 Fork-C), vibevoice, vibevoice_realtime, voxcpm2 (FIG — fused inner-graph latent).

**Pure non-AR diffusion/flow (Phase 5/6, step-bucket, f32 EASY):** omnivoice/viitorvoice (RNG-order law),
irodori, pocket_tts, rsb; supertonic is the proven 1SHOT precedent.

**STT two-stage track (Phase 7 — encoder cohort + AR decoder ring, full re-plumb, NOT a thin config):** voxtral
(fp16), cohere (fp16 + cross-attn), ark (fp16), granite, canary_qwen (per-slot LoRA), higgs_stt, vibevoice_asr;
ORT whisper/canary/qwen3_asr/funasr_nano (AED encoder cohort + decoder ring); parakeet/nemo_ctc/sensevoice/
moonshine/medasr (CTC/TDT — equal-context cohort / PER); nemotron (streaming-chunk RNNT).

**S2S + one-shot (Phase 7):** hibiki (new `as_duplex` + `DuplexStepModel`, f32 Fork-C); lfm2_audio (hybrid
conv+attn cache — own primitive or PER); ORT kokoro/melo/vieneu (1SHOT `synthesize_batch`); moss (ORT codec-AR,
clean `as_stepped` candidate); chatterbox (ORT, already wired).

**Cross-cutting prerequisites that gate the fan-out:** the §1.4 serve-loop slow-model graceful-shed fix (Wave-1
blocker); the §4 codec/vocoder transient-budget (box-kill risk before any heavy whole-body decoder serves >1
slot); the §5 hardware-adaptive sizing wiring (the BUG-1/BUG-2/96-vs-48 fixes landed as typed capability with
zero live consumers — the live admission wiring is still pending).

---

## 8. FILES (on disk, NOT committed; no `cargo fmt`)

**Modified (10, uncommitted vs HEAD `302e9c8`):**
- `crates/waav-infer-backend-torch/src/nn/ragged_ring.rs` — `append_full_masked_group`/`write_group`/
  `reset_group`/`group_seqlen` (the CFG-axis grouped ring) + the grouped layout twin (`+129/-0`).
- `crates/waav-infer-backend-torch/src/nn/{backbone.rs (+28), layer.rs (+37), self_attention.rs (+61)}` —
  additive `forward_ring` / `forward_ring_grouped` (host path byte-for-byte unchanged; dia2 544/544+608/608
  proves it).
- `crates/waav-infer-backend-torch/src/dia2.rs` (`+506`) — `pub mod ring::TorchDia2Batched` (grouped ring,
  `step_ring`, per-(slot,step) re-seed, per-slot depformer) + `TtsModel` impl + `step_batch` (per-slot loop).
- `crates/waav-infer-backend-torch/src/csm.rs` (`+341`) — `pub mod ring::TorchCsmBatched` (backbone ring +
  per-slot depth/Mimi) + `TtsModel` impl.
- `crates/waav-infer-backend-torch/src/misotts.rs` (`+370`) — `pub mod ring::TorchMisoTtsBatched` (Llama-8B
  backbone ring + per-slot depth/Mimi) + `TtsModel` impl.
- `crates/waav-infer-server/src/engine.rs` (`+115/-10`) — flipped dia2/qwen3/csm/misotts arms to default-on-CUDA
  with `WAAV_<MODEL>_BATCHED` override + `WAAV_<MODEL>_MAX_SLOTS/_MAX_SEQ`.
- `ci/heavy_live_tests.sh` (`+41/-2`) — registers the dia2/csm/misotts force-solo oracles.

**New (untracked):**
- `crates/waav-infer-backend-torch/tests/{dia2,csm,misotts}_force_solo_codes.rs` — the three Fork-A1 force-solo
  oracles (mirror the qwen3 template).
- `ci/phase_c_model_sweep.sh`, `docs/`.

**Recommended on-disk fixes before any commit:** (1) revert dia2 + misotts engine.rs arms from default-on-CUDA
back to env-gated opt-in (keep the ring code + oracles); (2) land the serve-loop slow-model graceful-shed fix +
standing gate; (3) ratify the dia2 sampled re-seed semantics; (4) refresh the stale csm sidecar golden (pre-existing,
independent).
