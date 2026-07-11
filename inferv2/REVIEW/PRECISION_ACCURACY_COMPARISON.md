# dia2 Precision / Optimization Accuracy Comparison — fp32 vs bf16 vs TRT-backbone vs TRT-both

**Measured live on GB10 (Grace-Blackwell sm_121), 2026-07-02.** This is a MEASUREMENT report — every
number below is from a single live run, not an estimate.

- Engine: `waav-infer-backend-torch` dia2-2B (tch/libtorch 2.12 + Torch-TensorRT 10.16, `--cfg accel_tensorrt`).
- Harness: `crates/waav-infer-server/tests/zz_dia2_four_config_accuracy.rs` (throwaway, NOT committed).
- ASR: in-tree whisper-base (ONNX/ORT, **CUDA EP**).
- Texts: **14** diverse sentences (the bf16 agent's 12-sentence set + 2 number/punctuation-heavy: an
  order-number/time sentence and a percentage/year sentence). **175 reference words** total.
- Seed: `cfg::SEED = 0`, re-seeded per synth call. Voice `[S1]`. Byte-identical CUDA-graph decode ON for the
  eager (fp32/bf16) paths so the ONLY variable is GEMM precision; the TRT paths run the engine step.
- **One model on the GPU at a time** (GB10 unified-memory OOM rule); model dropped between configs.
- Reference for every delta = **config 1 (fp32-sandwich)**. Codes + audio for each text come from ONE
  generation (codebook-agreement, ASR-WER, and waveform-corr are scored on identical output).

## The four configs (all ran; no config was skipped)

| # | config | knob | what runs |
|---|--------|------|-----------|
| 1 | fp32-sandwich | `WAAV_DIA2_PRECISION=f32` | TF32 matmul + fp32 accumulate — the accuracy REFERENCE (byte-identical golden path) |
| 2 | bf16-native | `WAAV_DIA2_PRECISION=bf16` | native bf16 GEMM (f32-sandwich bypassed) — the shipped default |
| 3 | TRT-backbone | `WAAV_DIA2_TRT=1` | fp16 TensorRT on the 28-layer backbone; depformer stays eager bf16 |
| 4 | TRT-both | `WAAV_DIA2_TRT=1` + `WAAV_DIA2_DEPFORMER_TRT=1` | + the 5 fp16 TensorRT depformer engines |

TRT configs leave `WAAV_DIA2_PRECISION` at the shipped default (bf16-native) for their eager fallback parts
(step-0 KV seed + heads; and the depformer in config 3). All TRT engines were already staged in
`/home/bud/.cache/waav-models/dia2-2b/trt/` (backbone_fp16.ts + depformer_g0..g4_fp16.ts); the runtime
`dlopen`'d `libtorchtrt_runtime.so` with **no LD_PRELOAD** and both engines loaded (`trt_active=true`,
`trt_depformer_active=true` verified at load). NaN/Inf in output: **0** across all 4 configs.

---

## THE TABLE (per config, vs the fp32-sandwich reference; N=14, 175 ref words)

| config | RTF | macro-WER | WER-Δ-vs-fp32 | micro-WER (edits/175) | codebook-agree% | wav-corr | #outlier-texts |
|--------|-----|-----------|---------------|-----------------------|-----------------|----------|----------------|
| **fp32-sandwich** (ref) | 1.973 | 0.0463 | +0.0000 | 0.0514 (9) | 100.0 | 1.0000 | 6 |
| **bf16-native** | 1.647 | 0.0324 | **-0.0139** | 0.0400 (7) | 3.7 | 0.0117 | 4 |
| **TRT-backbone** | 1.274 | 0.0245 | **-0.0218** | 0.0286 (5) | 2.9 | 0.0049 | 4 |
| **TRT-both** | 0.791 | 0.0300 | **-0.0163** | 0.0343 (6) | 3.8 | 0.0078 | 5 |

RTF = synth wall / audio duration (lower = faster; **<1.0 = faster than real time**). macro-WER = mean of
per-text WER; micro-WER = total word-edits / total ref words. codebook-agree% and wav-corr are measured over
the aligned prefix vs the fp32 reference codes/waveform.

**Speed side (RTF vs fp32):** bf16 = 1.20×, TRT-backbone = 1.55×, TRT-both = **2.49×** faster. TRT-both is
the ONLY config that crosses single-stream **RTF < 1** (0.791); it is 1.61× faster than TRT-backbone and
2.08× faster than bf16.

### The `#outlier-texts` metric is degenerate here — read it with the caveat

Median per-text WER is **0.0000** for every config (most sentences transcribe perfectly), so the requested
">2× median" rule collapses to "any text with WER > 0". The counts above (fp32=6, bf16=4, bb=4, both=5) are
therefore just "#texts with any ASR error at all" — and note the fp32 REFERENCE itself has the MOST (6). A
more meaningful outlier metric is **#texts where a config regresses vs the fp32 reference on the same text**:

| config | #texts worse than fp32 | which |
|--------|------------------------|-------|
| bf16-native | **1** | text 12 (numeric: "order number is" → "order numbers", 4 ed vs fp32's 3) |
| TRT-backbone | **0** | — (≤ fp32 on every text) |
| TRT-both | **1** | text 3 ("might rain" → "might ran", the depformer fork — see verdict) |

---

## Codebook agreement & waveform correlation — what they actually show

**The fp32→bf16 step ALREADY fully forks the AR trajectory.** Codebook agreement collapses 100% → **3.7%**
and waveform correlation 1.0 → **0.012** the moment you leave the exact fp32 kernels. TRT does NOT fork the
trajectory any *harder*: TRT-backbone 2.9%, TRT-both 3.8% codebook agreement sit in the **same ~2–4% band**
as bf16, and all three have wav-corr ≈ 0.005–0.012 (effectively uncorrelated).

This is the expected behaviour of a **generative discrete-AR codec model**: a tiny numeric perturbation at
any step re-samples a different (but equally valid) code, and greedy/multinomial AR then compounds it — so
codebook agreement and waveform correlation are **fork DETECTORS that saturate near zero**, not quality
gradients. They confirm the fork magnitude is the SAME for bf16 and for both TRT configs; they say nothing
about whether the audio is worse. The correct quality gate is ASR-WER of the generated audio.

---

## Per-text WER (the accuracy detail; outliers visible)

| text | ref words | content | fp32 | bf16 | TRT-bb | TRT-both |
|------|-----------|---------|------|------|--------|----------|
| 0 | 9 | quick brown fox | 0.000 | 0.000 | 0.000 | 0.000 |
| 1 | 10 | seashells | 0.000 | 0.000 | 0.000 | 0.000 |
| 2 | 13 | artificial intelligence | 0.000 | 0.000 | 0.000 | 0.000 |
| 3 | 13 | umbrella / rain | 0.000 | 0.000 | 0.000 | **0.077** |
| 4 | 12 | committee budget | 0.083 | 0.083 | 0.083 | 0.083 |
| 5 | 12 | manuscript | 0.000 | 0.000 | 0.000 | 0.000 |
| 6 | 14 | Mathematics… | 0.071 | 0.071 | 0.071 | 0.071 |
| 7 | 12 | journey thousand miles | 0.000 | 0.000 | 0.000 | 0.000 |
| 8 | 12 | chef meal | **0.167** | 0.000 | 0.000 | 0.000 |
| 9 | 12 | butterfly rainforest | 0.000 | 0.000 | 0.000 | 0.000 |
| 10 | 12 | customer support | 0.083 | 0.000 | 0.000 | 0.000 |
| 11 | 13 | stock market | 0.000 | 0.000 | 0.000 | 0.000 |
| 12 | 18 | order#/time (numeric) | 0.167 | **0.222** | 0.111 | 0.111 |
| 13 | 13 | 78% / 2023 (numeric) | 0.077 | 0.077 | 0.077 | 0.077 |

**Most "WER" is ASR/normalization noise, identical across configs**, not a synthesis-quality gradient:
- text 4: "finalize"→"finalise" (British spelling) — all four configs identical.
- text 6: "Mathematics" mis-heard as a different first word — all four wrong (fp32 too).
- text 13: "78 percent"→"78%" — all four identical (whisper number formatting).
- text 12 (numeric): "3:45"→"345", "15th"→"15" — number-formatting artifacts present in ALL configs; here
  fp32 is actually WORSE (3 ed) than both TRT configs (2 ed).
- text 8 & 10: the fp32 REFERENCE is the one with errors ("chef prepare the", dropped "today"); bf16/TRT
  transcribe them perfectly.

The only config-specific synthesis regression in the whole set is **text 3 under TRT-both**: "might rain"
→ "might ran" (1 word), which fp32, bf16 AND TRT-backbone all get right. That single word is the depformer-
TRT trajectory fork made audible.

---

## VERDICT

**How does the TRT accuracy loss compare to the fp32→bf16 loss? They are the SAME order — negligible — and
by the perceptual (WER) gate no config is measurably worse than the fp32 reference.**

1. **The fp32→bf16 step is where the trajectory "loss" lives, and it costs zero perceptual accuracy.**
   bf16 forks the codes completely (agreement 100%→3.7%, wav-corr →0.01) yet its macro-WER (0.0324) is
   *lower* than fp32's (0.0463). The fork is a different-but-valid realization, exactly as the two-tier
   precision thesis predicts.

2. **TRT-backbone ≈ bf16 on accuracy — arguably the cleanest of all four.** Its codebook agreement (2.9%)
   is in the same band as bf16 (3.7%), its macro-WER is the **lowest of the four** (0.0245), and it
   regresses on **zero** texts vs the fp32 reference. So the fp16-TensorRT backbone adds **no** measurable
   accuracy loss beyond bf16, while cutting RTF from 1.65 to 1.27 (1.55× vs fp32).

3. **The depformer-TRT "outlier-fork" is real but tiny — TRT-both's WER does NOT exceed bf16's.** This is
   the coordinator's key question, answered directly: TRT-both macro-WER = **0.0300**, which is **below**
   bf16's 0.0324 and far below fp32's 0.0463. The depformer flips are therefore a **benign trajectory fork**,
   not a quality loss — WER ≈ bf16 despite ~4% codebook agreement. The only footprint of the depformer fork
   is a small directional signal vs TRT-*backbone*: macro-WER +0.0055 (0.0245→0.0300), micro +1 edit (5→6),
   and exactly **1 outlier text** — text 3, "rain"→"ran" — that TRT-backbone got right. One mangled word in
   175. **#outlier-texts for TRT-both = 1** (regression-vs-reference metric) / 5 (degenerate any-error metric).

4. **Quantified ranking (accuracy, best→worst macro-WER):** TRT-backbone 0.0245 < TRT-both 0.0300 <
   bf16 0.0324 < fp32 0.0463. All four fall inside a **±0.022 WER band**, and every non-fp32 config is
   *better* than the fp32 reference. The accuracy differences are not resolvable at this precision level.

**Recommendation:** For maximum speed, **TRT-both** is justified — it is the only config at RTF < 1 (2.49×
faster than fp32) and its accuracy is statistically indistinguishable from bf16 (in fact WER-neutral vs the
fp32 golden). If one wanted the most conservative accuracy posture at still-solid speed, **TRT-backbone**
(1.55×, zero regressions vs fp32, lowest WER) is the safe pick. bf16-native remains the correct byte-
identical-bypass default; fp32-sandwich stays as the verification/golden mode.

---

## Honesty caveats (do not over-read these numbers)

- **N=14 is small.** Macro-WER 0.025–0.046 corresponds to just **5–9 total word-edits across 175 words**,
  and most of those edits are ASR normalization artifacts (British spelling, "%", "3:45"→"345") that are
  **identical across configs**. The WER deltas (all within ±0.022, all NEGATIVE vs fp32) are **NOT
  statistically significant** — the correct reading is "all four configs are perceptually equivalent; none
  is measurably worse than fp32," not "TRT-backbone is genuinely the best."
- **The `#outlier-texts` (>2×median) metric degenerates** because median WER = 0 for every config. The raw
  count = "#texts with any error" (fp32 itself scores worst, 6). The meaningful metric is
  regression-vs-reference (bf16=1, TRT-backbone=0, TRT-both=1), reported above.
- **Codebook agreement and waveform correlation are fork detectors, not quality metrics** for a generative
  discrete-AR model. They saturate near zero for ANY departure from exact-fp32 kernels (bf16 and both TRT
  configs alike) and must not be read as "96% of the audio is wrong."
- Precision choices here match how the configs actually ship (TRT paths on default bf16 eager fallback).
  Whisper-base (not large) is the ASR judge; a stronger ASR or MOS study would tighten the estimate but is
  unlikely to change the verdict given the fork already costs zero WER at fp32→bf16.

---

### Reproduce

```
source /home/bud/torch212_trt_venv/bin/activate && source ./gb10-env-212.sh
TRT=/home/bud/torch212_trt_venv/lib/python3.12/site-packages
export WAAV_TORCHTRT_LIB="$TRT/torch_tensorrt/lib" WAAV_TENSORRT_LIB="$TRT/tensorrt_libs"
export LD_LIBRARY_PATH="$WAAV_TORCHTRT_LIB:$WAAV_TENSORRT_LIB:$LD_LIBRARY_PATH"
RUSTFLAGS="--cfg accel_tensorrt" cargo test -p waav-infer-server --features torch \
  --test zz_dia2_four_config_accuracy -- --ignored --nocapture --test-threads=1
```
Wall time: 481.7 s for all 4 configs + ASR (single run, one model on GPU at a time).
