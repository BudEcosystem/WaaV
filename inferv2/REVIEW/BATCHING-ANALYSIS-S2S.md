# S2S DuplexStep / SlotBatch batched full-duplex seam — live perf + accuracy analysis (GB10 CUDA)

**Scope:** the WaaV-Infer **batched full-duplex (audio→audio) seam** — the read-while-emit, per-frame,
`[B]`-batched forward:
`DuplexStepModel::step(&SlotBatch)` (`crates/waav-infer-runtime/src/duplex.rs`) →
`CodecArDuplexModel::step` (`crates/waav-infer-core/src/s2s/duplex_codec_ar.rs`) → one
`ChatterboxArStep::step_slots_batched` = one `[B,…] language_model.onnx` `StaticGraph::run`
(`crates/waav-infer-core/src/tts/chatterbox.rs` `lm_forward_batched`).
Plus the **solo per-channel** seam: `DuplexStep::model_out` (K=1) → `HibikiDuplexModel`
(`crates/waav-infer-backend-torch/src/hibiki.rs`, kyutai/hibiki-zero-3b) and its engine wrapper
`HibikiS2sModel::s2s_turn`.
**Latency harness:** `FullDuplexBench` (`crates/waav-infer-provider/src/full_duplex_bench.rs`).
**Models:** chatterbox-onnx (codec-AR Llama backbone, fp32 `language_model.onnx`) as the
`CodecArDuplexModel` backbone; hibiki-zero-3b (native-moshi 3B, f32, tch/libtorch) as the solo S2S model.
**Hardware:** NVIDIA GB10 (sm_121, unified 121 GB pool), ORT-CUDA EP. **Date:** 2026-06-24.
**Env:** `source gb10-env.sh`, isolated single-test processes (`--test-threads=1`).

> Measurement note: the box was shared with concurrent foreign live-GPU runs during the timing windows.
> The **bit-identity assertions are contention-immune** (they compare codes, not wall-clock). The two
> timing numbers (scaling table, latency) were taken under partial contention — flagged below.

---

## 0. Headline findings (TL;DR)

1. **ACCURACY — bit-identity HOLDS on real CUDA weights.** A ragged, staggered 4-session cohort driven
   entirely through ONE `step(&SlotBatch)` is **token-for-token identical** to driving each session/slot
   solo through a `[B=1]` step — lengths `[55, 50, 45, 40]`, `EP=cuda`. **No divergence found.** RCA: the
   batched math reduces to each row's solo math because the backbone's `step_slots_batched` LEFT-aligns
   each slot's KV + LEFT-justifies its attention mask (GQA `seqlens_k = ReduceSum(mask) - 1`); the
   continuous-stream recycle is deterministic in `(user-stream, turn_idx)` so both paths recycle at the
   identical stride. Inherited from the proven codec-AR ragged identity (Path-A analysis §1).
2. **LATENCY — well under budget.** GPU-measured first-assistant-frame latency for a 4-tenant batched
   `SlotBatch` step = **81.9 ms** (budget 200 ms, `FULL_DUPLEX_LATENCY_BUDGET_MS`). The four modeled
   `VClock` benches (90–180 ms) are now backed by a real number.
3. **SCALING — batched beats solo only at B≥4, real win at B=8 (2.61×).** Per-stride wall (ms):
   N=2 → batched *slower* (0.88×), N=4 → 1.08×, N=8 → **2.61×**. Same host-KV-restream knee as TTS: the
   batched forward's fixed GEMM saving only outweighs the per-stride `[B]` KV re-stream once B is large.
4. **THE GAP (ranked #1) — the batched SlotBatch seam is WIRED-IN-TESTS-ONLY; engine-served S2S is SOLO.**
   `CodecArDuplexModel` is the **only** real `DuplexStepModel` and it is constructed **only** in
   tests/benches — never in the registry or any serve path. The production S2S serve path
   (`finalize_s2s` → `engine.s2s_turn` → `S2sModel::s2s_turn`) drives **one channel at a time** through
   the **solo** seam. There is a `codec_ar_batcher` lockstep multiplexer in the server, but it drives
   **TTS** (`ArStepModel::step_batch`), **not** `DuplexStepModel`. So multi-session batched duplex is
   **scoped/proven, not served.**
5. **THE GAP (ranked #2) — hibiki has NO batched form at all.** `HibikiDuplexModel` implements the K=1
   **solo** `DuplexStep`, not the batched `DuplexStepModel`. Every `KvCache`/embed/step is hardcoded
   `B=1`; `model_out` runs one per-channel session. `Lfm2AudioS2s::s2s_turn` ignores `channel` entirely.
   The batched seam's real-model coverage is **chatterbox-backbone only**.

No bug was found to fix — the seam is bit-faithful and the latency is under budget. The findings are
**architectural gaps** (wired-vs-scoped), precisely scoped in §3. Nothing committed (no bug).

---

## 1. ACCURACY — batched(SlotBatch) == per-slot solo (bit-identity)

Gate: `s2s::duplex_codec_ar::tests::s2s_duplex_ragged_concurrent_batched_bit_identical_and_scales`
(`#[ignore]`, run isolated; `343 s`). It builds two `CodecArDuplexModel`s over the real chatterbox CUDA
backbone, drives a 4-slot cohort admitted at **staggered ticks** (gap 5 ⇒ distinct context lengths AND
distinct user-stream-derived conditioning ⇒ a genuinely ragged batch), and compares:

- **REFERENCE:** each slot driven SOLO through `step(&SlotBatch)` with `[B=1]`, on its own schedule.
- **BATCHED:** every tick the whole active cohort advanced through ONE `step(&SlotBatch)` (ragged `[B]`).

Live result (GB10 CUDA, real weights):

```
=== perf-gap-1: REAL native-S2S DuplexStepModel batched seam on CUDA ===
  bit-identity: PASS — 4 ragged S2S slots at DISTINCT lengths [55, 50, 45, 40],
                batched(SlotBatch)==per-slot, user-stream modeled per tick, EP=cuda
```

Token-for-token identical, all 4 slots, distinct lengths (genuinely ragged). **No divergence — no RCA
needed.** Two supporting CUDA gates (each run isolated, real weights, PASS):

| Gate | What it proves | Result |
|---|---|---|
| `s2s_duplex_user_stream_is_load_bearing` | a DIFFERENT user-in stream ⇒ DIFFERENT model-out (the read is not cosmetic) | PASS (92 s) |
| `s2s_duplex_masked_slot_is_frozen_no_crosstalk` | a masked neighbour cannot perturb an active slot through the shared batched forward; masked slot never read | PASS (78 s) |

**Why bit-identity holds (RCA of the *mechanism*).** `CodecArDuplexModel::step` produces the model-out
for all continuing slots through ONE `ChatterboxArStep::step_slots_batched`, whose `lm_forward_batched`
LEFT-aligns each slot's real KV (indices `0..past`, pad on the right) and LEFT-justifies its attention
mask. The base LM's GroupQueryAttention computes `seqlens_k = ReduceSum(mask) - 1` and appends the new K
at that index, so each ragged row's batched math is **exactly** its solo math (`use_tf32=0`). The duplex
layer adds only deterministic, per-channel bookkeeping (the user-stream fold → prefill, the
`(user-stream, turn_idx)` continuous-stream recycle), and both the batched and per-slot paths execute that
bookkeeping identically — so the recycle lands on the identical stride in both, preserving bit-identity
**across sub-turns**, not just within one. This is the same EP-agnostic ONNX-graph identity the Path-A
codec-AR analysis established; the S2S seam **inherits** it.

---

## 2. PERF + LATENCY under a batched cohort

### 2a. End-to-end full-duplex latency (≤200 ms gate)

Gate: `full_duplex_bench::tests::full_duplex_bench_under_200ms_gpu_measured`
(`crates/waav-infer-provider`, NOT `#[ignore]`; run isolated, `40 s`). Builds a real `CodecArDuplexModel`
on CUDA, warms 3 ticks, then measures the wall time of ONE `step(&SlotBatch)` over a 4-tenant cohort —
"user stopped speaking" → "first assistant model-out frame":

```
=== GW-18 perf-gap-1: GPU-MEASURED full-duplex latency (real CodecArDuplexModel on cuda) ===
  first-assistant-frame latency for a 4-tenant batched SlotBatch step: 81.90 ms (budget 200 ms)
```

**81.9 ms ≤ 200 ms — PASS** (under partial GPU contention; a clean box would be lower). This replaces the
modeled `VClock` budgets (`SmoothTurnTaking` 160 ms, `Backchannel` 90, `Overlap` 180, `Interruption` 120)
in the four non-GPU `full_duplex_bench_*` tests with a real number for the headline turn.

> The other four `full_duplex_bench_*` tests (`under_200ms`, `no_glitch`, `multi_tenant_no_crosstalk`,
> `barge_in_stuck_stage_is_bounded`) are **virtual-clock** harnesses (the model's processing budget is
> *injected*, not run) — they assert the gateway plumbing (continuity, clean barge-in, per-tenant
> isolation), not real model time. Only `_gpu_measured` times the real seam.

### 2b. Scaling — batched(SlotBatch) vs per-slot loop (RTF per frame-rate)

From the same `..._and_scales` gate's throughput phase (per-stride wall ms, real CUDA, warmed to
mid-decode KV; partial contention):

| N (concurrent duplex sessions) | per-slot loop (ms/stride) | batched SlotBatch (ms/stride) | speedup |
|---:|---:|---:|---:|
| 2 | 37.04 | 42.19 | **0.88×** (batched slower) |
| 4 | 77.16 | 71.46 | **1.08×** |
| 8 | 288.04 | 110.34 | **2.61×** |

**Real-time framing.** Chatterbox codec-AR runs at ~50 Hz analog (20 ms/frame is the codec-AR analog used
by the Path-A benchmarks); hibiki/Mimi is 12.5 Hz (80 ms/frame). At N=8 the batched seam holds ~110 ms per
stride — i.e. one batched forward serves 8 sessions in the time the solo loop serves ~2.6. Whether each
session stays **RTF<1 per frame-rate** depends on the model's frame budget: at a 12.5 Hz (80 ms) S2S frame
rate, 110 ms/stride for 8 sessions is borderline (RTF≈1.4 of the 80 ms budget for the batched stride, but
that one stride advances all 8 — so effective per-session compute is ~14 ms, well inside budget). The
**knee is at N≈4** — below it the batched `[B]` KV re-stream cost (`O(B·H·max_past·D·n_layers·2)` per
stride) dominates the fixed GEMM saving, exactly the TTS host-KV-restream knee.

---

## 3. BUGS / GAPS / OPPORTUNITIES (ranked, with RCA + scope)

### GAP-1 (HIGH) — engine-served S2S does NOT use the batched SlotBatch seam (solo-only)

**Honest wired-vs-scoped state:** the batched `DuplexStepModel::step(&SlotBatch)` seam is **proven and
scoped, not served.**

RCA / evidence (`grep`-verified):
- The **only** real `DuplexStepModel` impl is `CodecArDuplexModel`
  (`crates/waav-infer-core/src/s2s/duplex_codec_ar.rs:224`). The other is `FakeDuplexModel` (a test
  double in `duplex.rs`).
- `CodecArDuplexModel::new` is called in **exactly two places**, both tests/benches:
  `full_duplex_bench.rs` (`_gpu_measured`) and `duplex_codec_ar.rs` (its own gates). It is **never**
  returned by the registry (`engine.rs` `load_model_at` has no `DuplexStepModel` arm — only
  `LoadedModel::{Stt,Tts,S2s}`), and **never** constructed in any serve path.
- The production S2S serve path is solo: `ws.rs:finalize_s2s` (one call per session/finalize) →
  `engine.s2s_turn(channel, …)` (`engine.rs:1446`, takes the model `Mutex`, one channel) →
  `S2sModel::s2s_turn(channel, pcm)`. No `SlotBatch`, no cohort, no `step(&SlotBatch)` anywhere on this
  path.
- The server's lockstep multiplexer `codec_ar_batcher.rs` *does* batch concurrent streams through ONE
  forward per tick — but it drives **`ArStepModel::step_batch` (TTS)**, not `DuplexStepModel`. There is
  **no** S2S analog of `CodecArBatcher`.

**Scope to close (greenfield, not a bug-fix):** add an S2S registry arm that builds a `CodecArDuplexModel`
(or a future native-S2S `DuplexStepModel`) and a duplex multiplexer (the `codec_ar_batcher` analog over
`DuplexStepModel::step(&SlotBatch)`) so concurrent `Task::S2s` sessions share ONE batched forward per
tick. The seam, the bit-identity, and the latency are all already proven — the missing piece is the
**serve wiring**. This is the single highest-value item; everything below is downstream of it.

### GAP-2 (HIGH) — hibiki (the real native-moshi 3B) has no batched form

RCA: `HibikiDuplexModel` implements the K=1 **solo** `DuplexStep` (`hibiki.rs:923`), not the batched
`DuplexStepModel`. `grep "batched\|SlotBatch\|step_batch"` over `hibiki.rs` → **NONE**. The Moshi
circular multistream cache, the 28 backbone `KvCache`s, and the 6 depformer `KvCache`s are all
constructed `KvCache::new(1, …)` — `B=1` hardcoded (`hibiki.rs:718,721,880,883`). `step_frame`/`embed`
are per-session, one column at a time. So even if GAP-1 were wired, hibiki could only ride the **solo**
seam; the batched seam's real-model coverage is **chatterbox-backbone only**.

**Scope:** batching hibiki is a substantial port — the backbone is straightforward to make `[B,…]` (it is
a standard GQA `StreamingTransformer`, the same shape `step_slots_batched` already batches), but the
weights-per-step depformer (9 weight groups via the `[0..8]×16` schedule, per-step LayerNorm, 16 logit
heads, `dep_caches.reset()` each frame) and the per-channel circular delay-cache scatter/gather would each
need a batched rewrite that preserves the f32 byte-faithful golden. Recommend: serve hibiki solo (correct,
real-time at 12.5 Hz for modest concurrency) and use `CodecArDuplexModel` as the batched-seam proof until
a batched native-S2S model is ported. Note `tch::Tensor` is `Send` not `Sync`, so today each channel is
single-threaded anyway.

### GAP-3 (MEDIUM) — `CodecArDuplexModel` is a *faithful-shape proxy*, not a trained S2S model

RCA: it drives the **real chatterbox codec-AR Llama backbone on CUDA** (a genuine batched forward, genuine
ragged KV, genuine bit-identity) — but the user-stream → conditioning map is a **deterministic synthetic
fold** (`user_conditioned_prefill`: hash the user frames into safe text tokens), not a trained
audio→audio cross-attention. It is single-codebook (D=1), whereas a real Moshi/hibiki emits D=16. So the
seam's *mechanics* (read-while-emit, masked≠absent, ragged batched identity, latency, scaling) are
real and load-bearing, but the **audio quality / translation correctness** is NOT what this model
measures — that is hibiki's golden gate (solo). This is correctly documented in the module header; flag
it so the 81.9 ms / 2.61× numbers are not mis-read as a trained-S2S quality result.

### GAP-4 (LOW) — padding/masking waste is real but bounded

The batched forward pads every row's KV to `max_past` of the cohort (RIGHT-pad). For a staggered cohort
(`[55,50,45,40]`) the shortest row wastes ~27% of its KV-stream lanes. This is the **same** waste the
TTS batcher carries; it is the mechanism that makes the host-KV-restream cost grow with B and produces the
N=2 regression (§2b). No correctness impact (the LEFT-justified mask zeroes the pad); it is a throughput
tax that the N≈4 knee already reflects. Opportunity (shared with TTS): a length-bucketed cohort would cut
the waste, but that is a cross-cutting batcher change, not S2S-specific.

### GAP-5 (LOW) — cohort frame-rate alignment is assumed, not enforced

`SlotBatch` advances ALL active slots ONE stride per `step`, implicitly assuming every slot in a cohort
runs at the **same frame rate**. `CodecArDuplexModel` (one backbone, one rate) satisfies this trivially.
But a future heterogeneous cohort (e.g. a 12.5 Hz hibiki slot beside a 50 Hz codec-AR slot) would need
cohort-by-frame-rate grouping (the INFER_ENGINE.md "cohort-by-frame-rate" rule) before sharing a forward.
Not a bug today (single-model cohorts only); scope it before any multi-model duplex cohort.

### CPU feasibility

The S2S batched gate is `#[ignore]`'d and gated on an **accelerated** EP (`active_ep` must contain
`cuda`/`gpu`); it does not exercise the aarch64 CPU EP. The bit-identity is an ONNX-graph property
(LEFT-align + mask), so it would hold on the CPU EP exactly as the Path-A codec-AR analysis confirmed
batched==solo on aarch64 CPU. Latency on CPU would not meet the 200 ms duplex budget (the codec-AR LM is
GPU-class), so CPU S2S is a correctness-only / low-rate fallback, not a real-time path. Not separately
measured here (out of the gate's accelerated-EP guard).

---

## 4. What was run (live, isolated, real numbers)

| Gate | Crate | Result | Wall |
|---|---|---|---|
| `s2s_duplex_ragged_concurrent_batched_bit_identical_and_scales` | core (`--lib`, `--ignored`) | PASS — bit-identical `[55,50,45,40]` + best 2.61× | 343 s |
| `s2s_duplex_user_stream_is_load_bearing` | core (`--lib`) | PASS | 92 s |
| `s2s_duplex_masked_slot_is_frozen_no_crosstalk` | core (`--lib`) | PASS | 78 s |
| `full_duplex_bench_under_200ms_gpu_measured` | provider (`--lib`) | PASS — 81.9 ms ≤ 200 ms | 40 s |

All run with `source gb10-env.sh`, process-isolated, `--test-threads=1`, real CUDA weights
(chatterbox-onnx). Peak memory stayed well within the 121 GB pool.

## 5. Verdict

- **Accuracy:** the batched SlotBatch seam is **bit-faithful** — ragged batched == per-slot solo,
  token-for-token, on real CUDA weights. No divergence; no fix needed.
- **Latency:** **81.9 ms** first-assistant-frame for a 4-tenant batched step (≤200 ms budget).
- **Scaling:** real win at B≥4 (2.61× at B=8); regresses below solo at B=2 (host-KV-restream knee at N≈4).
- **Wired vs scoped:** the batched duplex seam is **proven in tests/benches only**. Engine-served S2S is
  **solo per-channel** (`s2s_turn`), and the only batched real model is the chatterbox-backbone
  `CodecArDuplexModel` (a faithful-shape proxy, not a trained S2S model). Hibiki and lfm2 have **no**
  batched path. The #1 opportunity is to **wire** the batched seam into the S2S serve path (an S2S analog
  of `codec_ar_batcher`).
- **Fixed + committed:** nothing — no bug was found (the seam is correct as built); the open items are
  architectural wiring gaps, scoped above. No commit made.
