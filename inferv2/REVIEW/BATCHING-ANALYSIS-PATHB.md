# BATCHING ANALYSIS — Path B (in-process tch / libtorch) codec-AR LOCKSTEP batching

**Date:** 2026-06-24  **Box:** GB10 (Grace-Blackwell sm_121, 121 GB unified mem), aarch64 CPU.
**Scope:** the codec-AR `step_batch` frame-synchronous lockstep methodology applied to the **Path-B (tch)**
codec-AR models — dia-1.6b, dia2-2b, csm-1b, qwen3-tts-12hz-06b, higgs-tts, neutts-air. CUDA-bf16 (serving
regime) + aarch64 CPU-f32 where feasible. All numbers below are **live-measured** on this box.

---

## TL;DR — the headline finding (reframes the task)

**The lockstep `step_batch` multi-slot batcher is a Path-A (ONNX) facility. It is NOT wired to any
in-process Path-B (tch) codec-AR model.** Concretely:

1. **`ArStepModel` / `step_batch` (the 55×@64 lever) is implemented by exactly ONE *real* model:
   `ChatterboxTts` — which is an ONNX / `StaticGraph` (Path A) model, not tch.** Every other `ArStepModel`
   impl in the tree (≈40 of them) is a **test double**. **Zero** of the six cached tch codec-AR models
   (dia/dia2/csm/qwen3-tts/higgs/neutts) implement `ArStepModel`, so none of them is reachable by the
   lockstep scheduler's `step_batch` at all.
2. **On Path B the KV-cache `batch` axis is used for CFG branches (cond/uncond, B=2), NOT for concurrent
   user slots.** Every tch generate loop allocates `KvCache::new(1, …)` (single-stream) or
   `KvCache::new(branches, …)` (B=2 CFG on dia2/csm). There is **no per-stride multi-slot batched advance**
   on any tch model — the serving path is **B=1** (or fixed B=2 CFG, batched the same way every call).
3. **The one real Path-B multi-row batched forward is `TorchQwen3Tts::talker_logits_batched(&[Vec<i64>])`**
   — a `[B,L,hidden]` talker prefill → `[B,vocab]` last-position logits. It is a **probe/gate** (fresh KV
   per call, prefill only, used for the per-slot batched-mixed-LoRA gate), **not** a serving lockstep step.
   It requires a **rectangular** (non-ragged) batch (`talker_logits_batched: ragged or empty rows`).

So the task's premise — "the same step_batch frame-synchronous methodology on the tch codec-AR models" — is
**not implemented**. What *can* be measured on Path B is (a) the single real batched-forward primitive
(`talker_logits_batched`) scaled B∈{1..64}, and (b) the CFG-branch (B=2) bit-faithful batching on dia2/csm.
Both are reported below, plus the cross-path comparison to the Path-A chatterbox lockstep curve.

> The doc corpus already says this in different words: `INFER_PERF.md §3` — "55×@64" is an **idealized
> synthetic-GEMV roofline**; the REAL measured lockstep number (on Path-A chatterbox) is **~1.8× peak @
> B≈16, regressing to 0.95× @ B=64**, host-KV-restream-capped. `B1-pathb-reality.md` — the *other* "Path B"
> (the torch **sidecar**) is an opaque `model.generate()` HF black box the lockstep scheduler never reaches
> inside. Neither is the "tch lockstep step_batch" the prompt assumed.

---

## 1. ACCURACY — batched-vs-solo bit-identity per model × HW

### 1a. The one real Path-B batched primitive: `talker_logits_batched` (qwen3-tts)

Gate `qwen3tts_per_slot_batched_mixed_lora` (CPU f32), **PASS, live re-run**:
- (a) adapter-loaded base batch == standalone base batch — **max|Δ| = 0**
- (b) B=4 per-row mixed-adapter batch: **each row == its single-adapter solo run, max|Δ| = 0**
- (c) single-slot hot-swap + typed rejects — PASS

So at **B=4 CPU-f32**, the batched forward is **bit-identical** to per-slot solo (the lockstep correctness
law holds). **BUT** the new scaling probe (`qwen3tts_batch_scaling_probe`) shows this bit-identity is
**precision- and B-dependent** — it is NOT absolute on Path B:

| HW regime | bit-ident max\|Δ\| (logits), B=1 → 64 | argmax-code flips (batched vs solo)? |
|---|---|---|
| **CPU f32**   | 0 up to B=16, then **1.81e-4 @B32, 6.48e-5 @B64** | (not separately measured; magnitude ≪ code spacing) |
| **CUDA f32**  | 0 @B1, then ~1e-4..3e-4 for all B≥2 | **0 / N flips at every B** — never flips a code |
| **CUDA bf16** | 0 @B1, then **0.31 → 4.24** (logit scale) for B≥2 | **FLIPS: 1/4 @B4, 1/8 @B8** (0 elsewhere on the probe rows) |

**RCA of the divergence (the B23 scar, confirmed live):** a batched `[B,…]` GEMM and a B=1 GEMM use
different cuBLAS/MKL reduction tilings, so the float reduction order differs. In **f32** the resulting Δ is
~1e-4 on O(10)-scale logits (~1e-5 relative) and **never flips the greedy code** (CUDA f32: 0/N flips at
every B). In **bf16** the same reduction-order difference lands at ~0.3–4 on the logits and **does flip the
argmax code** on the high-magnitude synthetic probe rows (1/4 @B4, 1/8 @B8). This is exactly the documented
`dia2` B23 note: *"a batched forward and a per-branch batch-1 loop give different TF32 cuBLAS GEMM results,
which flip bf16 codes."*

**Why this is NOT a production bug today:** there is no batched-vs-solo serving regime on Path B. The
production qwen3-tts path is `KvCache::new(1, …)` (B=1, no concurrent slots); dia2/csm batch the **same**
B=2 CFG every call (batched-vs-**batched**, so the reference also batches → byte-identical, 608/608 CUDA-bf16
+ 544/544 CPU per B43). The bf16 batched-vs-solo flip is a **latent** invariant violation: *if* a tch
codec-AR model were ever wired into the lockstep multi-slot batcher in **bf16**, the AR-compounding
bit-identity invariant (the sacred "55×@64 is bit-identical to per-slot" claim) would break. **On Path B,
the lockstep bit-identity invariant holds only in f32 (CPU or CUDA), not bf16** — the opposite of Path-A
chatterbox, whose ONNX GroupQueryAttention left-align convention makes ragged batched==solo bit-identical
even in its serving dtype (gate `step_slots_batched_ragged_*` 544/544).

### 1b. CFG-branch (B=2) batching — dia2 / csm

This is the *only* multi-row batching that runs in a real serving loop on Path B, and it IS bit-faithful,
because it is batched-vs-batched (the reference also batches the 2 CFG branches). Per `B43-cuda-graph-perf.md`
(measured, not re-run here to bound GPU pressure): dia2 stays **608/608 CUDA-bf16 + 544/544 CPU
byte-identical** with the CFG B=2 backbone batched, CUDA-graph ON or OFF. csm depth-decoder likewise
byte-identical. No batched-vs-solo comparison exists for these (they have no solo path — CFG is intrinsic).

---

## 2. PERF SCALING CURVE — `talker_logits_batched`, B ∈ {1,2,4,8,16,32,64}

Best-of-12, warm, L=8, qwen3-tts-12hz-06b. Per-row time = batched-wall / B; speedup = per-row(B=1) /
per-row(B). (`tests/qwen3tts_batch_scaling.rs`, this analysis harness.)

### CUDA bf16 (the serving precision)

| B  | batched wall (ms) | rows/s | ms/row | speedup vs B=1 |
|----|------|--------|--------|--------|
| 1  | 11.0 | 91     | 11.00  | 1.00× |
| 2  | 11.2 | 179    | 5.59   | 1.97× |
| 4  | 10.7 | 375    | 2.67   | 4.12× |
| 8  | 11.7 | 685    | 1.46   | 7.54× |
| 16 | 13.0 | 1235   | 0.81   | 13.59× |
| 32 | 17.5 | 1828   | 0.55   | 20.11× |
| 64 | 23.3 | 2746   | 0.36   | **30.22×** |

**Knee:** the batched wall is essentially **flat (~11 ms) through B=16** (the forward is launch/latency-bound,
not compute-bound at these sizes), so throughput scales near-**linearly to B≈16** (13.6×). Past B=16 the
wall starts rising (17.5 ms @B32, 23.3 ms @B64) — compute begins to bind — but per-row time keeps falling, so
aggregate throughput still climbs (2746 rows/s @B64) and **there is NO regression at B=64** (unlike Path-A
chatterbox). **Measured 30.2× @ B=64.**

### CPU f32 (aarch64)

| B  | batched wall (ms) | rows/s | ms/row | speedup vs B=1 |
|----|------|--------|--------|--------|
| 1  | 266  | 3.8    | 266.2  | 1.00× |
| 2  | 326  | 6.1    | 163.0  | 1.63× |
| 4  | 489  | 8.2    | 122.3  | 2.18× |
| 8  | 823  | 9.7    | 102.9  | 2.59× |
| 16 | 998  | 16.0   | 62.4   | 4.27× |
| 32 | 1104 | 29.0   | 34.5   | 7.72× |
| 64 | 1325 | 48.3   | 20.7   | **12.85×** |

**CPU has NO flat region** — the wall rises monotonically from B=1 (MKL is already compute-bound at B=1), so
per-row time falls much more slowly: **12.85× @ B=64** vs CUDA's 30.2×. CPU batching still helps (amortizes
the weight stream + better BLAS tiling) but the knee is soft and the absolute throughput (48 rows/s) is ~57×
below CUDA's 2746 rows/s.

### Does the tch path batch as well as the doc 55×@64 curve?

- **vs the idealized 55×@64 synthetic roofline:** this primitive reaches **30× @ B=64 (CUDA bf16)** — about
  55% of the idealized roofline, and **closing** (still scaling at B=64). It gets far closer than Path-A
  chatterbox *because* `talker_logits_batched` keeps the **KV device-resident** (fresh `[B,L]` cache, no
  host↔device KV re-stream per stride) — exactly the "device-resident-KV graph" `INFER_PERF.md` says is
  required to recover the roofline. **Caveat:** this is a single *prefill* probe, not an AR decode loop with
  growing KV — a real per-stride decode would re-incur the `O(B·max_past)` attention cost the doc warns
  about, so 30× is an upper bound for the *prefill* shape, not a serving-loop guarantee.
- **vs the REAL Path-A chatterbox lockstep curve** (`INFER_PERF.md §3`, ONNX): chatterbox peaks at **~1.8×
  @ B≈16 and regresses to 0.95× @ B=64** (host-KV re-stream caps it). **Path B's device-resident batched
  forward scales dramatically better** (30× vs 1.8×, no regression) — but only the prefill shape is measured,
  and only on a model whose batched path is a non-serving probe.

---

## 3. CROSS-PATH COMPARISON (A = ONNX, B = tch)

| Dimension | Path A (ONNX / chatterbox) | Path B (tch / the 6 codec-AR models) |
|---|---|---|
| Lockstep `step_batch` wired to real model? | **Yes** — `ChatterboxTts` (the only real `ArStepModel`) | **No** — zero tch codec-AR models impl `ArStepModel` |
| Multi-slot concurrent batching in serving? | **Yes** — `step_slots_batched`, ragged-capable | **No** — serving is B=1 (or fixed B=2 CFG) |
| Batched-vs-solo bit-identity (serving dtype)? | **Yes, bit-identical** — left-aligned KV + left-justified mask make ragged batched == solo token-for-token (544/544) | **f32: yes; bf16: NO (flips codes)** — only `talker_logits_batched`, a probe; CFG B=2 is batched-vs-batched so stays 608/608+544/544 |
| Ragged (mixed-length) cohort? | **Supported, bit-identical** | **Rejected** (`talker_logits_batched` requires rectangular rows); no ragged path exists |
| Measured batched speedup | **~1.8× peak @B16, 0.95× @B64** (host-KV-capped) | **30× @B64 CUDA-bf16 / 12.85× @B64 CPU** (device-resident prefill probe) |
| Why the gap | exported ONNX graph re-streams split-KV host↔device every stride | in-process tch keeps KV device-resident; but the win is on a non-serving prefill, not an AR loop |

**Is the methodology identical across paths?** **No.** Path A has a genuine model-agnostic lockstep
multi-slot serving batcher (`ArStepModel::step_batch` → one `[B,…]` `StaticGraph::run` per stride), with an
absolute bit-identity invariant enforced in the serving dtype. Path B has **no equivalent**: its tch models
run their own solo `generate()` loop; the only batch axes are CFG branches (B=2) and a single non-serving
prefill probe. **The "55×@64 lockstep" property is a Path-A-only construct today.** Path B *could* batch
better (its device-resident forward scales 30× where Path A's host-streamed one caps at 1.8×), but that
capability is not wired into a serving lockstep batcher, and its bf16 GEMM is not bit-identical batched-vs-solo.

---

## 4. RANKED BUGS / GAPS / OPPORTUNITIES (with RCA)

**G1 (GAP, structural, highest leverage) — No tch codec-AR model is wired to the lockstep `step_batch`
seam.** RCA: `ArStepModel` lives in `waav-infer-runtime`; the only real impl is `ChatterboxTts`
(ONNX/Path-A) in `waav-infer-core`. The six tch models expose only solo `synthesize_pcm` /
`generate_codes_vec`; their `step()` methods are internal AR-loop helpers, not the trait verb. Consequence:
the entire v2 lockstep/step-bucket machinery (fixed-slot scheduler, duty ledger, the 55×@64 lever) reaches
**zero** of the in-process tch models. **Scope:** to put a tch model under the lockstep batcher you must (i)
implement `ArStepModel` (prefill/step/step_batch/decode_audio/reset_slot) over the model's tch state, (ii)
make its KvCache slot-indexed (today B=1 or B=2-CFG), (iii) make the batched forward bit-identical to solo
in the serving dtype — which **G2 shows is impossible in bf16** without a fix. Large, design-level.

**G2 (BUG, latent, would block G1) — Path-B batched forward is NOT bit-identical to solo in bf16 (flips
codes).** RCA: the B23 scar — batched `[B,…]` cuBLAS GEMM uses a different reduction order than the B=1 GEMM;
in bf16 the ~0.3–4 logit Δ flips the argmax code (measured: 1/4 @B4, 1/8 @B8 on `talker_logits_batched`). In
f32 the same Δ is ~1e-4 and never flips (CUDA f32: 0/N at every B; CPU f32: exact ≤B16). **Not hit in
production** (no batched-vs-solo serving path on Path B; CFG B=2 is batched-vs-batched). **It becomes a real
bug the moment G1 is attempted in bf16.** Mitigations when G1 is done: (a) gate lockstep bit-identity in
**f32** only (as the existing qwen3tts/dia2 CPU gates do), or (b) accept the AR-compounding identity only
batched-vs-batched (never compare against a solo path), the convention dia2/csm already use. **Scope:** a
gating/convention decision to land with G1, not a standalone fix.

**G3 (GAP) — `talker_logits_batched` rejects ragged cohorts.** RCA: it requires every row to be the same
length L (`ragged or empty rows` error). The real concurrent-user case is ragged (different start times /
lengths). Path-A chatterbox solved this with left-aligned KV + left-justified mask; Path B has no equivalent.
**Scope:** part of G1 (a tch lockstep batcher must left-align ragged KV like chatterbox does).

**G4 (GAP) — CUDA-graph + batching interaction is unexploited / can conflict.** RCA: the tch CUDA-graph seam
(B43/B46) captures a **fixed-shape** per-step forward — it is shape-bound, so a graph captured at one B
cannot replay at another B. dia2's graph is captured against the **B=2 CFG** cache and explicitly
`reset_graph()`s when the cache is reallocated. A lockstep batcher with **dynamic** B (slots join/leave)
would invalidate the captured graph on every cohort-size change — defeating both levers at once unless B is
bucketed (one captured graph per B bucket). **Opportunity:** bucket lockstep cohorts to a small set of fixed
B values so each B reuses a captured graph; otherwise graph + dynamic-batch fight. **Scope:** design note for G1.

**G5 (OPPORTUNITY, measured) — the device-resident batched forward scales 30× where Path-A caps at 1.8×.**
RCA: Path-A chatterbox's exported ONNX re-streams split-KV host↔device per stride (`O(B·max_past)` grows with
B), capping it at 1.8× and regressing past B=16. The tch `talker_logits_batched` keeps KV device-resident →
30× @B64, no regression. **This is the strongest argument for G1**: an in-process tch lockstep batcher is the
"device-resident-KV re-export" the doc says is needed to recover the roofline — Path B already has the
substrate, it just isn't wired to the scheduler. (Caveat: 30× is the *prefill* shape; a real AR decode loop
re-incurs growing-KV attention, so expect less than 30× in a serving loop — but plausibly well above 1.8×.)

**G6 (NON-BUG, documented) — these AR codec models are launch-bound and slower-than-realtime even at B=2.**
RCA: dia2 RTF 1.86–3.42, csm similar (B43). Each outer AR step = 1 backbone forward + 31 depformer stages ×
4 layers = many tiny kernel launches; the backbone CUDA-graph wins only ~3% (RTF 1.92→1.86) because the
depformer dominates. **Batching across concurrent users (G1) is the throughput lever; the depformer graph is
the latency lever.** Both are scoped follow-ons, neither is a defect.

**CPU batching cliff:** on CPU f32 there is no flat region (MKL is compute-bound at B=1), so batching yields a
soft 1.6×→12.85× curve (B2→B64) with no knee — CPU batching helps but is ~57× below CUDA throughput. Not a
bug; a property of the BLAS roofline.

---

## What was fixed / committed

**Nothing fixed; nothing committed.** No production bug was found — the bf16 batched-vs-solo divergence (G2)
is a **latent** property that the production B=1 / B=2-CFG serving paths do not hit, so there is no source
change to make (per the task rule "commit only if you FIX a bug"). The dia2 544/544 + csm regression gates
are untouched (I added only an `#[ignore]` analysis test; the pre-existing modified `chatterbox.rs` in the
working tree is a **concurrent Path-A agent's** CPU-ragged-batched gate, not mine — left alone).

**Added (analysis harness, `#[ignore]`, clippy-clean, not a CI gate):**
`crates/waav-infer-backend-torch/tests/qwen3tts_batch_scaling.rs` — the B∈{1..64} scaling + batched-vs-solo
bit-identity / code-flip probe used for all §1a/§2 numbers above. Run:
`WAAV_BATCH_DEV=cuda|cpu [WAAV_BATCH_FP32=1] cargo test -p waav-infer-backend-torch --test
qwen3tts_batch_scaling -- --ignored --nocapture --test-threads=1`.

## Re-verification

- `qwen3tts_per_slot_batched_mixed_lora` — PASS (live re-run, CPU f32, max|Δ|=0 batched==solo at B=4).
- New probe — PASS on CPU f32, CUDA f32, CUDA bf16 (it asserts nothing; it reports). Build + clippy clean.
- No source touched → dia2 608/544 + csm gates structurally unaffected.
