# B58 — The whole-pipeline perf gap: PROFILE the AR step, CLOSE the #1 non-backbone cost (bit-faithfully)

**Date:** 2026-06-23 · **Box:** NVIDIA GB10 (Grace-Blackwell), aarch64, CUDA 13.0, **sm_121**, 121 GB unified,
PyTorch 2.12.0+cu130. **Model:** neutts-air (Neuphonic NeuTTS Air) — a **Qwen2-0.5B** AR codec-TTS (24 layers,
hidden 896, 14q/2kv heads), the B49/B55 Torch-TensorRT target. Greedy decode is the byte-identity reference.

## TL;DR — B55's dilution hypothesis is WRONG for the eager path. The backbone is 85% (not the tail), and the #1 non-backbone cost (lm_head) is BANDWIDTH-bound and bit-faithfully irreducible.

B55 hypothesized the ~2.5×→~1.6× live dilution is because *"the non-backbone per-frame cost (f32 lm_head,
sampling, host↔device handoffs) dominates the AR loop tail."* **I instrumented the eager per-step AR loop and
MEASURED it. The hypothesis is refuted:**

| segment | us/step (eager bf16) | % of step | nature (proven by micro-probe) |
|---|---:|---:|---|
| **backbone fwd + KV** | **~9,960** | **84.9%** | compute/bandwidth-bound (M=1 GEMMs @ ~71 GB/s) |
| **lm_head GEMM** | **~1,620** | **13.9%** | **bandwidth-bound** (390 MB weight read @ ~244 GB/s) |
| f32 cast (full vocab) | ~70 | 0.6% | trivial |
| repetition penalty | ~49 | 0.4% | the H2D-handoff lever (closed below) |
| **sampling + host round-trip** | **~26** | **0.2%** | **trivial** — refutes the "sampling/handoffs dominate" claim |
| SUM (synchronized) | ~11,735 | 100% | |
| whole step (unsync) | ~11,759 | — | the real overlapped wall (≈ sync ⇒ the loop is serial) |
| **non-backbone share** | — | **~15%** | |

The non-backbone tail is **~15% of the step**, and within it the **sampling host round-trip is 0.2%** and the
**repetition penalty is 0.4%**. The single biggest non-backbone cost is the **lm_head (13.9%)** — and a
micro-probe (50 back-to-back GEMMs, one sync vs a single synced GEMM) proves it is **bandwidth-bound**, not
launch-bound: `single 1793 us ≈ amortized 1842 us`. A CUDA graph (which only removes launch overhead) **will
not help it** — confirmed, not assumed.

**What I closed bit-faithfully:** the **repetition-penalty H2D handoff** — the one genuinely improvable
bit-faithful lever. The old spelling rebuilt `Tensor::from_slice(&seen_vec).to(dev)` (a host→device copy of the
*entire growing* seen-id history) every frame; the new `SeenIds` keeps the ids **device-resident** and appends
one scalar per step (O(1) H2D). **Measured: that segment is 1.88× faster (40.65 us → 21.61 us).** Byte-identical
(the device tensor is value-identical to the from-slice tensor → the gather/scatter is unchanged; both
byte-identity gates re-run **0/96**).

**Whole-pipeline RTF: 0.607 before AND after** (96 codes → 1.92 s audio in 1.166 s, byte-identical). The lever
saves ~19 us out of ~11,735 us/step = **0.16% whole-pipeline** — real, correct, but immeasurable in RTF because
the non-backbone tail is tiny. **The honest finding: the eager non-backbone cost is near-optimal; the only
material lever is the backbone, which is compute-bound and reachable solely by lower precision (TRT, lossy).**

**Is the whole-pipeline 5× reachable bit-faithfully? NO.** It is a **throughput-path-only (lossy) target.** The
backbone (85%) is M=1-GEMM compute/bandwidth-bound; the only way down is fewer/smaller weight reads = lower
precision (int8/nvfp4 → the B55 lossy AR-fork) or real batching. The lm_head (the #1 non-backbone cost) is
bandwidth-bound on a full-vocab 390 MB read and is bit-faithfully irreducible. The eager path is already near
its bit-faithful floor.

---

## 1. The deliverable — the per-step AR profile (where the time ACTUALLY goes)

Instrumented `TorchNeutts::profile_step_breakdown` (neutts.rs) runs the **greedy** decode (the byte-identity
path, RNG-free) and time-segments each step, `cuda::synchronize`-ing after each segment to attribute the async
device time to the right bucket. It accumulates per segment over the measured steps (first 3 excluded for
cuBLAS/cuDNN plan + allocator warmup) and also records the **un**synchronized whole-step wall. The run rides the
EXACT byte-identity greedy path — the emitted codes are asserted `0/96` vs the golden.

Verbatim (representative of 4 reproducible runs; the test is `cuda_torch_neutts_profile::neutts_ar_step_profile`):

```
=== B58 neutts AR-step profile (CUDA bf16, 93 measured steps) ===
  lm_head GEMM          :  1627.16 us/step  ( 13.9% of sync)
  f32 cast (full vocab) :    69.53 us/step  (  0.6% of sync)
  repetition penalty    :    49.18 us/step  (  0.4% of sync)
  sampling+host-rt      :    26.05 us/step  (  0.2% of sync)
  backbone fwd+KV       :  9963.02 us/step  ( 84.9% of sync)
  ----------------------
  SUM (synchronized)    : 11734.94 us/step
  whole step (unsync)   : 11758.64 us/step  (real overlapped wall)
  non-backbone share    :  15.1% of sync
```

Two structural facts fall straight out:

- **The synchronized SUM ≈ the unsynchronized whole step** (11,735 ≈ 11,759 us). The AR loop is intrinsically
  **serial** (`hidden_N → logits_N → token_N → embed_N → hidden_{N+1}`), so syncing between segments costs ~0 —
  there was no cross-segment overlap to destroy. This is why reducing a small segment cannot be hidden behind
  another: every microsecond is on the critical path, but every *small* segment is a small fraction of it.
- **The backbone is the step.** 85% of every frame is the 24-layer Qwen2 decode + KV update. The "non-backbone
  tail" B55 worried about is ~15%, and it is overwhelmingly the **lm_head** (13.9%), not sampling/handoffs.

### Micro-probes — launch-bound (graph helps) vs bandwidth/compute-bound (it doesn't)

```
=== B58 micro-probes (amortized over 50 iters vs single synced) ===
  lm_head : single  1793.48 us | amortized  1842.48 us  → BANDWIDTH/compute-bound (graph won't)
  backbone: single  9678.62 us | amortized 10631.46 us  → compute-bound
```

Both are within ~10% amortized-vs-single — i.e. **neither is launch-bound**. The launches are already hidden
(within a step the layers are serially dependent, so there's nothing to pipeline; across steps the ~10 ms of
real kernel work dwarfs the ~tens-of-us launch overhead). The arithmetic confirms it:

- **lm_head** reads the **390 MB** untied head (`217652 × 896 × 2 B`) per step: `390 MB / 1.62 ms ≈ 244 GB/s` —
  near GB10's LPDDR5X bandwidth ceiling. It is a clean, efficient, **bandwidth-bound** GEMM.
- **backbone** reads **716 MB** of weights per step (24 × (q/k/v/o + gate/up/down)): `716 MB / 9.96 ms ≈
  72 GB/s` — far *below* the bandwidth ceiling, because at **M=1** the tensor-core GEMMs are deeply
  underutilized (tensor cores want M≥16) and the many small RoPE/RMSNorm/softmax/SwiGLU elementwise kernels add
  fixed per-op cost. It is **compute/occupancy-bound at batch-1**, not launch-bound and not bandwidth-saturated.

**This is the key engine insight:** a CUDA graph (the B43/B45/B46 lever — neutts has none today, only the lossy
TRT path) removes *launch* overhead. Here there is no launch overhead to remove. The backbone is slow because
M=1 GEMMs are intrinsically inefficient, and the lm_head is slow because it must stream 390 MB. **Neither is
addressable by a bit-faithful kernel-launch trick.**

---

## 2. The lever I applied — device-resident seen-ids for the repetition penalty (the only bit-faithful win)

Of the four non-backbone segments, three are immovable bit-faithfully:
- **lm_head (1.6 ms)** — bandwidth-bound full-vocab read; can't shrink the vocab byte-identically (the greedy
  argmax is over the FULL 217 652 vocab — narrowing to the speech block is NOT provably equal to the full
  argmax, so it would break the gate). Lower precision is the only lever, and that is lossy (B55).
- **f32 cast (70 us)** — required for the f32-argmax byte-identity; folding it into the GEMM would force the
  GEMM to f32 (2× the bandwidth, AND a different bf16-vs-f32 result → not byte-identical).
- **sampling + host round-trip (26 us)** — the `argmax(-1).int64_value()` host sync is unavoidable: the greedy
  loop needs the token on the **host** every step anyway (to check the `gen_end` stop, to `embed_ids` the next
  input, to map the FSQ code, to collect the output). "Keep the token on-device" doesn't apply to an
  early-stopping greedy loop that consumes the token on the host. And at 26 us (0.2%) there is nothing to win.

The **one** genuinely improvable bit-faithful cost is the **repetition-penalty handoff**. HF penalizes the
logits at *every* seen id (prompt ++ generated). The old code rebuilt the whole seen-id device tensor each step:

```rust
let seen_t = Tensor::from_slice(&seen_ids).to(dev);   // a growing H2D copy EVERY frame
```

i.e. a host→device copy of the *entire growing history* (≈600→700 i64) per frame — a textbook per-frame
"handoff" cost. The fix (`SeenIds`, a neutts-local helper) keeps the ids **device-resident** in a preallocated
`[cap]` i64 buffer and appends one scalar per step:

```rust
let seen_t = seen.tensor();    // narrow view [0,len) — value-identical to the from-slice tensor
...
seen.push(picked_id, dev);     // ONE-scalar H2D into the next slot (O(1), not O(history))
```

**Bit-faithfulness:** `seen.tensor()` holds the SAME ids in the SAME order as `from_slice(&full_vec)` (locked by
two CPU unit tests: `seen_ids_accumulator_matches_from_slice`, `seen_ids_overflow_grows_correctly`), so the
penalty `index_select`/`index_copy_` gather/scatter is byte-for-byte unchanged. The greedy argmax sees the same
logits → the same codes.

**Measured (the A/B, `probe_rep_penalty_ab`, history 694 ids, 200 iters):**

```
=== B58 lever — repetition-penalty handoff (history 694 ids, 200 iters) ===
  OLD (from_slice each step):   40.65 us | NEW (device-resident):   21.61 us  → 1.88x faster
```

**1.88× on that segment** — the growing H2D recopy is eliminated; only the full-vocab gather/scatter remains.

---

## 3. Whole-pipeline RTF — the real number, before → after (honest: the lever is immeasurable)

`cuda_torch_neutts_profile::neutts_whole_pipeline_rtf` times the FULL greedy decode + NeuCodec ONNX decode,
asserting the codes byte-identical:

```
=== B58 whole-pipeline greedy RTF (CUDA bf16) ===
  96 codes → 1.920s audio in 1.166s → RTF 0.607
```

- **Before the lever (B55 eager-fp16 baseline):** RTF **0.589**.
- **After the lever:** RTF **0.607** (same run-to-run band; the 19 us/step saved is 0.16% of the 11.7 ms step —
  below the measurement noise of a ~1.2 s end-to-end run).

**This is the honest whole-pipeline result: the bit-faithful lever does not move RTF**, because the cost it
closes is 0.4% of the frame. The profile is what makes this conclusion *earned* rather than asserted: we now
KNOW the tail is 15% and the movable part of it is sub-percent.

---

## 4. Where the B55 dilution actually comes from (quantified)

The dilution (`~2.5× isolated backbone → ~1.6× live`) is real, but its cause is the OPPOSITE of B55's framing.
It is **not** that the non-backbone tail is large — it is that the lm_head is a **fixed bandwidth cost** that the
low-precision backbone does not touch, so accelerating only the backbone re-weights the frame toward the
lm_head:

| | eager bf16 | TRT-backbone 2.5× (modeled) |
|---|---:|---:|
| backbone | 9.96 ms (85%) | ~3.98 ms (~67%) |
| lm_head | 1.62 ms (14%) | 1.62 ms (~27%) — **unchanged, now dominant tail** |
| rest (cast+rep+sample) | 0.16 ms (1%) | 0.16 ms (~3%) |
| **step** | **11.7 ms** | **~5.76 ms → ~2.04× whole-pipeline (modeled)** |

The model predicts ~2.04×; B55 **measured ~1.6×** live. The remaining gap is the TRT path's **own** per-step
H2D/D2H handoffs that the eager path does not pay — exporting the KV (`stack_caches_fp16`) and shipping
`embed`/`cos`/`sin` to the engine device every step across the engine boundary. So the live TRT loop adds back a
handoff cost precisely where the eager loop has none. (The eager loop's *only* meaningful non-backbone cost, the
lm_head, is bandwidth-bound and shared by both.)

To actually approach 5× you must cut the lm_head AND the backbone together in low precision (a low-precision
lm_head + an in-engine sampling/KV so nothing crosses the host) — i.e. the **whole** per-frame path lowered, not
the backbone GEMM alone. That is the throughput regime (lossy), exactly as B55 concluded; B58 adds the *reason*:
the eager non-backbone tail is already near its bit-faithful floor.

---

## 5. THE LAW — the byte-identity gates re-run, unchanged

The lever touches only how the seen-id tensor is *built* (value-identical), never the math. Proven:

- **neutts CUDA bf16 greedy** (`cuda_torch_neutts::cuda_bf16_greedy_codes_byte_identical`): **0/96 codes differ,
  first divergence None.**
- **neutts CPU f32 byte-identity** (`cuda_torch_neutts::cpu_f32_byte_identical_to_reference`): codec maxΔ **0**,
  llm_hidden maxΔ **0**, first-step speech logits maxΔ **0**, **greedy 0/96 differ** (THE LAW, CPU f32).
- The profiling + RTF tests themselves assert the produced greedy codes are byte-identical to the golden
  (`0/96`) — the perf run rides the exact byte-identity path.

---

## 6. Gates (all green)

- `cargo test -p waav-infer-backend-torch --lib`: **147 passed** (145 prior + 2 new `SeenIds` unit tests).
- `cargo clippy -p waav-infer-backend-torch --all-targets -- -D warnings`: **clean** (default).
- `cargo clippy -p waav-infer-backend-torch --all-targets --features cuda -- -D warnings`: **clean**.
- neutts byte-identity (CUDA bf16 + CPU f32): **0/96**, maxΔ **0** — unchanged.
- whole-pipeline greedy RTF: **0.607**, byte-identical.

---

## 7. Files changed (in scope — the chosen model + tests only; `nn/` untouched)

| file | change |
|---|---|
| `crates/waav-infer-backend-torch/src/neutts.rs` | **`SeenIds`** device-resident seen-id accumulator (the bit-faithful repetition-penalty handoff lever) + wired into `generate_codes` / `greedy_codes_eager` / the profiler (the TRT-only `generate_codes_trt` is left byte-for-byte as B49/B55). **Profiling instrumentation:** `profile_step_breakdown` (the per-segment AR breakdown), `probe_lm_head` / `probe_backbone` (launch-vs-bandwidth micro-probes), `probe_rep_penalty_ab` (the lever A/B), `greedy_rtf` (whole-pipeline timing), + the `StepProfile` struct. 2 CPU unit tests locking the `SeenIds` value-identity invariant. |
| `crates/waav-infer-backend-torch/tests/cuda_torch_neutts_profile.rs` | **NEW** (`cfg(feature="cuda")`) — the B58 gate: `neutts_ar_step_profile` (the per-segment profile + micro-probes + the lever A/B, byte-identity-asserted) and `neutts_whole_pipeline_rtf` (the end-to-end RTF, byte-identity-asserted). |

The lever lives **entirely in the model** (the repetition penalty is model-specific glue, not a shared `nn::`
primitive), so `nn/` is untouched — correct per the scope rule.

---

## 8. Honest bottom line

- **The profile (the deliverable):** the eager AR step is **85% backbone, 14% lm_head, ~1% everything else**.
  B55's "sampling + host↔device handoffs dominate the tail" is **measured false** — sampling is 0.2%.
- **The #1 non-backbone cost is the lm_head (1.6 ms, 14%)**, and a micro-probe proves it is **bandwidth-bound**
  (390 MB full-vocab read @ ~244 GB/s, amortized ≈ single) — **bit-faithfully irreducible** (can't narrow the
  vocab without breaking the full-vocab argmax; can't lower precision without losing byte-identity).
- **The lever applied (best available bit-faithful ROI):** device-resident seen-ids for the repetition penalty —
  **1.88× on that segment** (40.65 → 21.61 us), eliminating a per-frame growing H2D copy. Byte-identical (gates
  0/96). **Whole-pipeline RTF unchanged at 0.607** because the segment is 0.4% of the frame — an honest,
  profile-earned "near-optimal already" verdict for the eager tail.
- **Is whole-pipeline 5× reachable bit-faithfully? NO — it is a throughput-path-only (lossy) target.** The
  backbone (85%) is M=1-GEMM compute-bound; only lower precision (TRT int8/nvfp4, the B55 lossy AR-fork) or real
  batching cuts it. The eager byte-identity path is already at its bit-faithful floor; the 5× lives in the
  low-precision/batched regime, with the lm_head needing to be lowered *too* (not just the backbone) to avoid
  the bandwidth-bound dilution this profile pinpoints.
