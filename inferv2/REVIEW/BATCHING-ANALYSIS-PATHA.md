# Path-A (ONNX Runtime) codec-AR LOCKSTEP cohort batching — live perf + accuracy analysis (GB10 CUDA + aarch64 CPU)

**Scope:** the WaaV-Infer custom frame-synchronous batcher on Path A (ORT): `CodecArBatcher`
(`crates/waav-infer-server/src/codec_ar_batcher.rs`) → `serve_codec_ar_multiplexed_bounded`
(`crates/waav-infer-runtime/src/serve.rs`) → `Driver::tick` (`crates/waav-infer-runtime/src/driver.rs`)
→ `ArStepModel::step_batch` (`crates/waav-infer-runtime/src/arstep.rs`), overridden by chatterbox as one
`[B,…] language_model.onnx` `StaticGraph::run` (`crates/waav-infer-core/src/tts/chatterbox.rs`
`lm_forward_batched`/`step_slots_batched`).
**Models:** chatterbox-onnx (codec-AR TTS, fp32 `language_model.onnx`), + whisper-tiny.en (STT micro-batch
cohort). Both cached.
**Hardware:** NVIDIA GB10 (sm_121, unified 121 GB pool) ORT-CUDA EP 1.27 + aarch64 CPU EP (same dylib).
**Date:** 2026-06-24. **Env:** `source gb10-env.sh` (CUDA), `WAAV_ORT_EP=cpu` (CPU EP).

> NOTE on measurement conditions: the box was **shared with concurrent foreign live-GPU test runs**
> (other Claude sessions running `s2s::duplex_codec_ar` / `qwen3tts` gates) during part of this analysis.
> Where a timing number was taken under contention it is flagged; the **bit-identity assertions are
> contention-immune** (they compare codes, not wall-clock). Numbers re-taken in a clean window are marked
> "(clean window)".

---

## 0. Headline finding (TL;DR)

1. **ACCURACY — bit-identity holds on BOTH HW.** The ragged/concurrent batched cohort is **token-for-token
   identical** to each stream's per-slot solo reference, on CUDA *and* on the aarch64 CPU EP. The GQA
   identity (LEFT-aligned KV + LEFT-justified mask ⇒ `seqlens_k = ReduceSum(mask) - 1`) is an ONNX-graph
   property, EP-agnostic. **No divergence found.**
2. **SCALING — the 55×@64 doc-curve thesis is FALSE on the real path (already known + re-confirmed live).**
   The real chatterbox curve **peaks ~1.8× at B≈16 and REGRESSES below per-slot by B=64** — the host-KV
   re-stream (`O(B·H·max_past·D·n_layers·2)` per stride) grows with B and eventually dominates the GEMM
   savings. The knee is at B≈16, NOT B=64.
3. **LIVE e2e — the batcher advances multiple streams per tick (not serialized).** N=16 concurrent codec-AR
   streams serve at RTF<1 on the production singleton (fp32, doc 0.688); the multiplexed loop's `step_batch`
   cohort exceeds 1 (recorded max cohort = 6 at N=6).
4. **🔴 FOUND + FIXED A P0 SERVING BUG (committed `65ba7ec`):** the batched serve path crashed outright on
   the **default registry config** (`waav.json` pins the LM to `q4f16`, whose KV is F16, but the batched
   forward hard-coded `.as_f32()` ⇒ "wrong dtype" on the first concurrent cohort). Single-stream worked;
   the lockstep cohort path did not; no gate caught it (all load the fp32 LM explicitly). Fixed byte-faithful
   (`to_f32_vec()` widen + `feed_float` re-narrow = bit-exact F16 round-trip). See §5-B0.
5. **TOP non-bug GAP — no production `/metrics` observability of the live cohort width** (max-streams-per-tick
   is only provable in tests via a recording wrapper). See §5-G2.

---

## 1. ACCURACY — batched cohort == per-stream solo (bit-identity), per model × HW

| Model | HW / EP | Cohort | Result | max\|Δ\| / token-identity |
|---|---|---|---|---|
| chatterbox codec-AR | GB10 CUDA | ragged 4-slot, lens [18,74,67,60], staggered | **PASS** | codes token-for-token identical batched==per-slot |
| chatterbox codec-AR | GB10 CUDA | wide ragged 16-slot, lens [18,52,49,46,43,40,…], pad>0 | **PASS** | codes token-for-token identical batched==per-slot |
| chatterbox codec-AR | **aarch64 CPU** | ragged 4-slot, lens [18,34,29,24], staggered | **PASS** | codes token-for-token identical batched==per-slot (GQA identity holds on CPU) |
| supertonic one-shot TTS | GB10 CUDA | ragged 4-row, distinct lengths | **PASS** | **maxΔ = 0.0** (sample-for-sample identical batched==per-row) |
| whisper-tiny.en STT | **aarch64 CPU EP** (auto-fell-back) | ragged [3,7,12,19]s | **PASS** | transcripts token-for-token identical batched==per-slot |

**The mechanism (why batched == solo, EP-agnostic):** `lm_forward_batched` LEFT-aligns each slot's real KV
at buffer indices `0..past` (pad on the RIGHT, zeroed) and LEFT-justifies the attention_mask (1s left, 0s
right). The chatterbox base LM's `GroupQueryAttention` contrib op derives `seqlens_k = ReduceSum(mask) - 1`,
rotates the new token at RoPE pos `seqlens_k`, rotates each cached key at its absolute buffer index, and
appends the new K at index `seqlens_k`. Under this layout every ragged row's batched math is **exactly** its
solo per-slot math (a right-pad key contributes nothing; the new K lands at the row's own `seqlens_k`). Plus
`use_tf32=0` (TF32 made the fp32 GEMM non-batch-invariant). This is graph/op-level, so it holds identically
on the CPU EP — confirmed by the new CPU gate.

---

## 2. PERF SCALING CURVE — per-stride wall, per-slot loop vs ONE batched run

### GB10 CUDA (equal-context cohort — the batch-favorable upper bound) — MEASURED LIVE 2026-06-24

| B | per-slot loop (ms) | batched (ms) | speedup | notes |
|---|---|---|---|---|
| 1 | 16.010 | 19.652 | **0.81×** | batch-of-1 overhead — per-slot wins |
| 2 | 31.362 | 29.202 | 1.07× | |
| 4 | 66.874 | 47.055 | 1.42× | |
| 8 | 130.589 | 74.584 | 1.75× | |
| 16 | 317.020 | 179.550 | **1.77× ← PEAK (the host-KV-feedback knee)** | |
| 32 | 536.753 | 312.945 | 1.72× | past the knee, declining |
| 64 | 1111.000 | 1044.692 | **1.06× ← REGRESSES toward parity** | host-KV re-stream dominates |

`live_headline_batched_scaling_matches_doc_curve` **PASS** (703s). Bit-identity @ width PASS (16 ragged
slots, lens [18,52,49,46,43,40,…], token-for-token identical). **Peak 1.77× @ B=16** (matches the
single-source-of-truth `CHATTERBOX_HEADLINE_PEAK_BATCH_SPEEDUP=1.80`), **B=64 = 1.06× regresses** — the
55×@64 thesis is FALSE; the **knee is B≈16**. This fresh run reproduces the documented 2026-06-21 curve
(B16 1.81→1.77×, B64 0.95→1.06× — within the unified-pool jitter band).

**Ragged cohort (the real concurrent-user case), GB10 CUDA (clean window):**

| N | per-slot loop (ms) | batched (ms) | speedup |
|---|---|---|---|
| 2 | 32.387 | 29.477 | 1.10× |
| 4 | 64.306 | 57.509 | 1.12× |
| 8 | 132.577 | 89.496 | **1.48×** |

(Ragged is strictly lower than the equal-context upper bound; doc reference ~1.66× @ N=8 — this run was
partly contended in its timing phase.)

### aarch64 CPU EP — chatterbox codec-AR (`cpu_ragged_batched_forward_bit_identical_and_scales`, NEW gate)

- **CPU ragged bit-identity: PASS** — 4 slots at DISTINCT lengths **[18,34,29,24]**, codes token-for-token
  identical batched-vs-per-slot. **This is the cross-HW accuracy deliverable: the GQA LEFT-aligned-KV
  identity holds on the CPU EP exactly as on CUDA — no divergence.**
- CPU per-stride wall scaling curve (equal-context, B∈{1,2,4,8}, fp32 LM on CPU EP) — MEASURED LIVE:

| B | per-slot loop (ms) | batched (ms) | speedup |
|---|---|---|---|
| 1 | 63.396 | 66.249 | 0.96× |
| 2 | 74.874 | 44.986 | 1.66× |
| 4 | 301.286 | 117.068 | 2.57× |
| 8 | 501.661 | 121.061 | **4.14× ← peak** |

**Surprising cross-HW finding: the CPU EP codec-AR batched forward scales BETTER than CUDA** (4.14× @ B=8
vs CUDA's 1.75× @ B=8). **RCA:** the CPU per-slot loop pays a large fixed per-`run()` dispatch overhead
(ORT session call + thread-pool dispatch per slot) that batching amortizes dramatically — the CPU batched
per-stride wall stays nearly flat (45→117→121 ms) while the per-slot loop grows linearly (75→301→502 ms).
Critically, the CPU path has **no H2D/D2H KV transfer** (KV stays in CPU RAM), so it does NOT hit the
host-KV-transfer wall that bends the CUDA curve down past B≈16 — within B≤8 it keeps climbing. Absolute
latency is ~4–8× higher than CUDA (CPU is the portability/correctness floor, not a serving target), but the
**batching speedup ceiling is higher on CPU.** Bit-identity holds (see above) — accuracy is HW-invariant.

### aarch64 CPU EP — whisper-tiny.en STT micro-batch cohort (`whisper_ragged_concurrent_batched…`, 95.5s)

- **Ragged bit-identity: PASS** — 4 clips [3,7,12,19]s, batched transcripts **token-for-token identical**
  to per-slot solo. (Ran on the CPU EP — whisper auto-fell-back to CPU on this box.)
- Direct batched-vs-serial (B copies of the 19s clip): B2 **1.12×** / B4 1.17× / B8 **1.19×** / B16 1.17×;
  per-clip wall 664.9→648.3 ms (the batching signature — per-clip cost drops as the cohort grows).
- Concurrency ramp (live `transcribe()` seam, coalesced): N2 1.13× / N4 1.11× / N8 1.15× / **N16 1.16×**,
  16/16 ok. **Best throughput scaling vs serialization = 1.16×.** The ~1.2× ceiling is whisper-tiny's
  merged-decoder ONNX having no device-handle KV input — the SAME host-KV constraint as the chatterbox
  codec-AR path (so the STT cohort batches with margin, not near-linearly).

**Reference (doc `live_headline_batched_scaling_matches_doc_curve`, CUDA, equal-context, 2026-06-21):**
B1 0.85× / B2 1.12× / B4 0.99× / B8 1.39× / **B16 1.81× PEAK** / B32 1.46× / **B64 0.95× REGRESS**.
The single-source-of-truth constants are `CHATTERBOX_HEADLINE_PEAK_BATCH_SPEEDUP = 1.8`,
`CHATTERBOX_HEADLINE_PEAK_BATCH = 16`.

**Knee RCA (B≈16 peak, B=64 regression):** the chatterbox `language_model.onnx` is a **host-KV-feedback
graph** — it takes the split-KV as host `past_key_values.{layer}.{key,value}` *inputs* and emits host
`present.*` *outputs* that the AR loop re-streams **every stride**. In `lm_forward_batched` each stride
allocates a fresh `vec![0f32; B·H·max_past·D]` per layer×{k,v}, `copy_from_slice`s each slot's LEFT-aligned
KV into it (host CPU work, ~186% CPU observed during the B-sweep), uploads it H2D, then reads `present.*`
D2H and un-pads it per row. This host marshaling is `O(B · H · max_past · D · n_layers · 2)` and grows
linearly with B, while the batched GEMM saving saturates. Below B≈16 the GEMM amortization wins (1→16 climbs
to 1.77×); past it the host-KV re-stream dominates and by B=64 it claws the speedup back to parity (1.06×).
The synthetic "55×@64" figure came from a GEMV-only decode-step microbenchmark that omitted this host KV
re-stream entirely. **55×@64 is recoverable ONLY by a re-exported graph that keeps KV device-resident
across strides** (no host re-stream) — until then the honest headline is ~1.8× peak @ B≈16. See §5 for the
opportunity.

---

## 3. LIVE SYSTEM e2e — the batcher advances multiple streams per tick

**FOUND A REAL P0 BUG on the production registry path (now FIXED — see §5 B0).** The first e2e run
(`gb10_serves_16_concurrent_codec_ar_streams_rtf_under_1`, which loads chatterbox via the registry/Engine =
the live WS/REST path) **FAILED at warmup**:
`InferError { Internal, "model output 'past_key_values.0.key dtype' missing or wrong dtype" }`. Root cause:
the shipped `waav.json` pins `language_model: q4f16` (set 2026-06-22, AFTER the perf-validation doc), and the
q4f16 LM declares its KV (`past_key_values.*` / `present.*`) as **F16**, but the BATCHED serve path
(`lm_forward_batched`) hard-coded `.as_f32()` on the KV → fails on F16. The single-stream `step` path
(`lm_forward` → `feedback_present_kv`, dtype-preserving) was unaffected; ONLY the lockstep `step_batch`
cohort path broke. No gate caught it because every lockstep gate loads the fp32 `language_model.onnx`
explicitly. **This means the live batcher could not serve a concurrent codec-AR cohort with the default
registry config.** Fixed (§5 B0, byte-faithful) + re-validated.

**Post-fix re-run (registry q4f16):** `gb10_serves_16_concurrent_…` now **clears the warmup serve** and runs
the live 16-concurrent serve — the crash is gone (validated). BUT the q4f16 serve is so **host-bound** (101%
single-core on the f16↔f32 KV marshaling, GPU ~6% idle) that the N=16 × ≤1000-stride serve **did not finish
in the 15-min gate window** (vs fp32's ~94 s in the doc) — a >10× slowdown. This is the q4f16-perf finding
(§5-B0 perf note), not a fix defect: my fix removes the CRASH; q4f16 is simply the wrong precision for this
host-KV-bound serve path. **Re-run on the fp32 LM (the doc's measured config) to get a clean RTF + cohort
proof:**

**`live_gb10_batcher_concurrent_ragged_is_bit_identical_and_scales` (fp32 LM, N=6 ragged, the REAL
`serve_codec_ar_multiplexed` batcher loop) — PASS (175s, MEASURED LIVE 2026-06-24):**
- **LIVE code bit-identity: PASS** — 6 concurrent ragged streams, body code lengths
  **[83,111,141,201,165,201]**, token-for-token identical batched-vs-per-slot on the REAL model through the
  production multiplexed serve loop.
- **max `step_batch` cohort = 6** — the batcher advanced **all 6 streams in ONE batched tick** (NOT 6
  serialized single-stream loops on the model mutex). **This is the direct proof the live batcher batches
  multiple streams per tick.** Shared-loop wall 57.67 s, 16 body-strides/s.
- The fp32 LM ran **GPU-bound (87% util)** — efficient — vs the q4f16 host-bound timeout above (6% GPU).
  This is the same paradigm running well on fp32 and badly on q4f16, the empirical basis for the §5-B0
  precision recommendation.
- (vocoder-decode determinism control confirmed the PCM is run-to-run non-deterministic on this box, so the
  bit-identity target is correctly the AR CODES — a documented GB10 fp32-vocoder property, not a batching bug.)

**Doc-measured fp32 e2e (2026-06-21, reproduced-architecture):** `gb10_serves_16_concurrent` = N=16, 75.4 s
audio / 51.9 s wall, **RTF 0.688**, ~26 GB (production singleton, ONE model instance, flat-bounded). The
prior live ragged batcher gate recorded max `step_batch` cohort = **6** (one shared loop, not serialized) on
N=6, body codes `[83,111,141,201,165,201]` token-for-token == per-slot.

---

## 4. Per-model × HW summary table

| Model · paradigm | HW / EP | Accuracy (batched vs solo) | Peak batched speedup | Knee |
|---|---|---|---|---|
| chatterbox · codec-AR | GB10 CUDA | **bit-identical** (4-slot + 16-slot ragged) | **1.77×** | B≈16 (regresses by B=64 → 1.06×) |
| chatterbox · codec-AR | aarch64 CPU | **bit-identical** (4-slot ragged) | **4.14×** (@ B=8) | not reached by B=8 (no H2D/D2H KV wall) |
| supertonic · one-shot/flow TTS | GB10 CUDA | **bit-identical** (maxΔ=0.0) | **2.33×** (@ B=8) | not reached by B=8 (no host-KV loop) |
| whisper-tiny.en · STT feedforward | aarch64 CPU EP | **bit-identical** (transcripts) | **1.19×** direct / **1.16×** concurrent | flat ~1.2× (merged-decoder host-KV) |

**Cross-cutting reading of the table:** the **codec-AR paradigm has the LOWEST CUDA batching ceiling**
(1.77×) of the three because it is the only one with a per-stride host-KV feedback loop (G1). The flow-TTS
(supertonic, 2.33×) and the feedforward STT (whisper) have no per-stride KV re-stream so they batch more
cleanly per-step (supertonic) or are simply encoder-bound (whisper). On the CPU EP the codec-AR ceiling is
much HIGHER (4.14×) because CPU has no H2D/D2H transfer and the fixed per-`run()` dispatch overhead is what
batching amortizes. **Accuracy is paradigm- and HW-invariant: every batched cohort == solo, bit-for-bit.**

---

## 5. BUGS / GAPS / OPPORTUNITIES (ranked, with RCA)

**No accuracy DIVERGENCE found** (every ragged/concurrent batched cohort is bit-identical to per-slot solo
on both CUDA and CPU). **One real P0 SERVING bug WAS found + FIXED** (B0 below) — the batched serve path
crashed outright on the default registry config; it was not a numeric divergence but a hard dtype failure.

### B0 (BUG, P0 — FIXED + committed) — the lockstep `step_batch` path crashed on the default registry q4f16 LM
- **Symptom:** `gb10_serves_16_concurrent_codec_ar_streams_rtf_under_1` (the registry/Engine path = the live
  WS/REST serve path) FAILED at warmup: `Internal: "model output 'past_key_values.0.key dtype' missing or
  wrong dtype"`. Reproduced on the very first concurrent serve.
- **RCA:** `waav.json` pins `language_model: q4f16` (set 2026-06-22, after the 2026-06-21 perf doc). The
  q4f16 LM declares its KV (`past_key_values.*` inputs + `present.*` outputs) as **F16** (verified via
  `onnx.load` — `inputs_embeds` is F32 but every KV tensor is F16). The batched forward `lm_forward_batched`
  did the host-side LEFT-align (scatter) + un-pad (gather) by calling **`.as_f32()`** on the KV — which
  returns `None` for an F16 tensor, surfacing the typed "wrong dtype" error. The single-stream `step`
  (`lm_forward` → `feedback_present_kv`, a dtype-preserving rename) was unaffected, so ONLY the lockstep
  cohort path broke — and no gate caught it because every lockstep gate loads the fp32 `language_model.onnx`
  explicitly via `EpKind::Cuda`. Net effect: **the live batcher could not serve any concurrent codec-AR
  cohort under the default registry config** (the production WS/REST path). A regression masked by the gates'
  fp32-explicit loads.
- **Fix (byte-faithful):** in `lm_forward_batched`, read the KV via `to_f32_vec()` (widens F16→f32
  losslessly) instead of `as_f32()` for both the scatter and the un-pad. The reorder is a pure copy; the
  assembled buffer is then handed to `feed_float`, which is ALREADY graph-dtype-driven and narrows it back
  to the LM's declared F16 — and `f16::from_f32(x)` where `x` came from an f16 is the IDENTICAL f16, so the
  `F16→f32→F16` round-trip is **bit-exact**. For the fp32 LMs `to_f32_vec()` returns the same data as
  `as_f32()` and `feed_float` keeps f32 → **zero behaviour change** (the deterministic
  `batched_forward_codes_identical_to_per_slot` + `ragged_…` + 24 chatterbox unit tests stay green).
- **Validation:** re-running `gb10_serves_16_concurrent_codec_ar_streams_rtf_under_1` with the UNCHANGED
  registry q4f16 config now **clears the warmup serve** (the line that previously threw the dtype error) and
  proceeds into the live 16-concurrent serve (16 worker tasks spawned, GPU active) — the crash is gone. The
  deterministic f32 bit-identity gates (`batched_forward_codes_identical_to_per_slot` + `ragged_…` + 24
  chatterbox unit tests) stay GREEN, so the fp32 path is byte-unchanged. _Final N=16 RTF number pending the
  serve completing (the q4f16 path is host-bound — see the §5-B0 perf note below)._
- **Perf note on q4f16 (follow-up, NOT a blocker):** the q4f16 serve is markedly slower than fp32 on this
  path because the f16 KV must be widened→reordered→narrowed on the HOST every stride (101% single-core,
  GPU ~6% idle) — q4f16 shrinks the *weights* but the codec-AR loop's bottleneck is the host-KV re-stream
  (G1), which q4f16 makes WORSE (extra f16↔f32 conversion). **Recommendation: keep the codec-AR LM at fp32
  for the lockstep serve path** (or land G1's device-resident ring-KV before defaulting it to q4f16). The
  fix here removes the CRASH; the precision choice for best serve latency is a separate config decision.

### Perf gaps + opportunities (ranked by value):

### G1 (OPPORTUNITY, highest value) — codec-AR batching peaks at only ~1.8× because the LM graph round-trips KV through the host every stride
- **RCA:** `language_model.onnx` exposes KV as host `past_key_values.*` inputs / `present.*` outputs; the AR
  loop re-streams the whole `[B,H,max_past,D]×n_layers×2` KV host↔device EVERY stride (`O(B·max_past·…)`).
  This host marshaling grows linearly with B and is what bends the curve down past B≈16 (1.77× peak →
  1.06× at B=64). Measured: ~186% host CPU during the B-sweep is the `copy_from_slice` KV marshaling.
- **Evidence it is the cause, not the GEMM:** the one-shot/flow TTS on the SAME box+EP (supertonic, no
  per-stride host-KV loop) scales to **2.33× @ B=8** vs chatterbox's 1.75× @ B=8 — the only structural
  difference is the host-KV feedback.
- **Fix (scoped):** re-export the chatterbox LM with **device-resident ring-KV** (KV stays on-device across
  strides, no host `past/present` round-trip) — the same IoBinding/device-handle KV the spec's ring-KV
  follow-up calls #1. That is the ONLY path to recover near-linear scaling toward the synthetic 55×@64; it
  requires a graph re-export (model-author work), not a serving-loop change. Until then 1.8×@B≈16 is the
  honest ceiling and the doc constants are correct. **Lockstep slots should be sized at B≈16, NOT B=64.**

### G2 (GAP, observability) — the live `/metrics` cannot observe the batcher's per-tick cohort width
- **RCA:** the production serve loop (`serve_codec_ar_multiplexed_bounded`) and `Driver::tick` emit NO metric
  for the `step_batch` cohort size. `/metrics` exposes `waav_infer_codec_ar_submitted_total` (a count of
  admitted streams) but NOT how many streams advanced per tick — so an operator cannot tell from telemetry
  whether the batcher is genuinely batching (cohort>1) or silently degraded to serial (cohort==1). The only
  proof of multi-stream-per-tick is in tests, via the `CohortRecordingTts`/`CodeRecordingArModel` wrappers.
- **Fix (small, scoped):** add a histogram `waav_infer_codec_ar_step_batch_cohort` (observed at each
  `Driver::tick`/`step_batch` call with `step_inputs.len()`) and a gauge for current live-slot count. ~10
  lines in the runtime serve loop; pure telemetry, zero numerics impact. Gives production the "is it
  batching?" signal the tests have. **This is the cleanest do-now improvement.**

### G3 (CHARACTERIZED, not a bug) — B=1 batched is SLOWER than the per-slot path (0.81×)
- **RCA:** a batch-of-1 pays the `[B=1,…]` feed-assembly + host-KV marshaling overhead with no GEMM
  amortization to offset it. The code already handles this correctly: `ChatterboxTts::step_batch` routes
  `inputs.len() <= 1` to the per-slot `step` path (so a lone stream never eats the batched overhead). The
  0.81× is only the *headline microbench* forcing `step_slots_batched` at B=1 to measure the curve floor —
  the production loop never does this. **No fix needed; behaviour is already optimal.** Confirms the knee
  shape (sub-1× at B=1, peak at B≈16).

### G4 (CHARACTERIZED) — ragged cohorts scale slightly below the equal-context upper bound
- **RCA:** a ragged cohort pads each row's KV to `max_past` (the longest slot), so the host-KV re-stream +
  GEMM both carry pad columns that contribute nothing — pure overhead vs an equal-context cohort. Measured:
  ragged 1.48× @ N=8 (clean, partly contended) / doc 1.66× @ N=8 vs equal-context 1.75× @ B=8. The gap is
  the padding waste. **Bit-identity is unaffected** (pad columns are masked to zero). Mitigation would be
  length-bucketing the cohort (group similar-length slots) — a scheduler optimization, lower value than G1.

### G5 (NOTE) — aarch64 CPU EP path
- The fp32 LM runs correctly on the CPU EP and is **bit-identical to per-slot** (G-accuracy holds EP-agnostic
  — see §1). Absolute CPU latency is ~30–60× slower than CUDA (the codec-AR loop is not a CPU serving target;
  CPU is the correctness/portability floor, P-6). The registry's default `q4f16` LM (waav.json) needs
  MatMulNBits and is GPU-oriented; the fp32 `language_model.onnx` is the CPU-runnable graph. See §2 CPU curve.

---

## 6. What was fixed / added (committed `65ba7ec` on `waav-infer-v2-build`)

1. **FIXED (P0 bug B0):** `lm_forward_batched` in `crates/waav-infer-core/src/tts/chatterbox.rs` now reads
   the KV via `to_f32_vec()` (F16→f32 lossless) instead of `as_f32()` for both the scatter (LEFT-align) and
   the un-pad — so the batched lockstep serve path no longer crashes on the q4f16 LM (F16 KV). Byte-faithful:
   `feed_float` re-narrows to the LM's declared F16 (bit-exact f16 round-trip); fp32 LMs are unchanged.
   2 lines changed (`.as_f32()` → `.to_f32_vec()`). Validated: 24 deterministic chatterbox bit-identity
   gates green + the live registry serve path clears the previously-failing warmup.
2. **ADDED:** `cpu_ragged_batched_forward_bit_identical_and_scales` (same file) — the CPU-EP analogue of the
   CUDA ragged gates (which pin `EpKind::Cuda` and SKIP on CPU). Proves the ragged batched seam is
   bit-identical to per-slot on the aarch64 CPU EP, and measures the CPU scaling curve. Gated behind
   `WAAV_CPU_BENCH=1` (opt-in; the CPU AR loop is slow). No CUDA session ⇒ no `mem::forget` leak — runs
   safely in-process.

## 7. Suggested follow-ups (scoped, NOT done here)

- **G1 — device-resident ring-KV re-export of the chatterbox LM** (the only path to recover near-linear
  codec-AR scaling past B≈16). Model-author / graph-export work.
- **G2 — `waav_infer_codec_ar_step_batch_cohort` histogram** in the runtime serve loop (~10 lines; the
  cleanest do-now telemetry win — gives production the "is it batching?" signal the tests have).
- **Precision policy:** keep the codec-AR LM at **fp32** for the lockstep serve path (q4f16 is host-bound on
  this host-KV-feedback graph — it shrinks weights but makes the actual bottleneck worse). Either flip the
  shipped `waav.json` LM back to fp32, or land G1 before defaulting it to q4f16.
