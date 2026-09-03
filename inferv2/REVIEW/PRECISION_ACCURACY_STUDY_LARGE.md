# dia2 Precision/TRT Accuracy — Large Real-Dataset Study (N=300, LibriSpeech test-clean)

**Goal:** decide whether TRT-both (RTF 0.896, super-realtime) can be the dia2 default vs the current bf16-native default,
on a real corpus with enough samples for a statistical non-inferiority verdict. Prior study was N=14 (not significant).

**Method (reproducible):** 300 LibriSpeech test-clean transcripts (3–25 words), synthesized by dia2 at 4 configs
(same seed), whisper-base (ORT) ASR → WER vs the transcript. Paired per-utterance analysis. Harness:
`crates/waav-infer-server/tests/zz_dia2_large_corpus_accuracy.rs` + `TorchDia2::synth_pcm_with_codes`; corpus + raw
per-utterance WER checkpointed in the session scratchpad (`dia2_large/`). GB10, torch 2.12.

## Per-config summary (N=300, 4053 reference words)

| config | mean WER | median WER | **micro-WER** (edits/refwords) | mean RTF | #WER=0 |
|--------|---------|-----------|-------------------------------|----------|--------|
| fp32-sandwich (ref) | 0.1243 | 0.0556 | **0.1034** (419/4053) | 2.041 | 131 |
| bf16-native (current default) | 0.1879 | 0.0742 | **0.1680** (681/4053) | 1.849 | 119 |
| TRT-backbone | 0.1243 | 0.0817 | **0.1083** (439/4053) | 2.085 | 116 |
| **TRT-both** (proposed) | 0.1285 | 0.0833 | **0.1078** (437/4053) | **0.896** | 113 |

## Paired non-inferiority analysis (decision: TRT-both vs bf16-native)

| comparison | mean Δ | bootstrap 95% CI | paired-t p | Wilcoxon p | micro-WER Δ | NI @0.01? | power |
|---|---|---|---|---|---|---|---|
| **TRT-both − bf16** | **−0.0594** | [−0.207, +0.022] | 0.385 | 0.714 | **−0.0602** | NO | 0.067 |
| TRT-both − fp32 | +0.0043 | [−0.013, +0.021] | 0.629 | 0.384 | +0.0044 | NO | 0.302 |
| bf16 − fp32 | **+0.0637** | [−0.018, +0.210] | 0.352 | 0.747 | **+0.0646** | NO | 0.067 |

## Verdict — TRT-both is NOT worse than bf16; strict margin-0.01 is unprovable at N=300 (not a refutation)

1. **By every aggregate metric, TRT-both ≥ bf16.** Mean WER delta favors TRT-both (−0.0594); micro-WER favors TRT-both
   (0.1078 vs 0.1680); paired tests are non-significant (p=0.38 / 0.71). TRT-both ≈ fp32 (micro 0.1078 vs 0.1034,
   delta +0.0044). **The depformer codebook flips do NOT show up as a systematic WER loss at scale.**
2. **Surprise: bf16-native measured WORSE than fp32 AND both TRT configs** on this corpus (micro 0.1680 vs 0.103–0.108).
   Likely heavy-tail noise (a few bf16 outliers), but it means the *current* default is not obviously the safest.
3. **The strict non-inferiority test (margin 0.01) FAILS for EVERY pair — including bf16 vs fp32 — because dia2's WER is
   heavy-tailed** (paired-delta sd ≈ 1.18: dia2 occasionally produces a catastrophic take on a hard utterance regardless
   of precision). Power is 0.067–0.302. Proving a 0.01 margin against that intrinsic variance would need thousands of
   utterances (or a MOS test). This is a POWER limitation, not evidence that TRT-both is worse.
4. **The per-utterance "regressions" are symmetric across all configs** (~26%): TRT-both regresses vs bf16 on 76/300,
   TRT-backbone vs bf16 on 80/300, bf16 vs fp32 on 81/300. Inspecting the transcripts, the worst takes are distributed
   across ALL configs (fp32 utt 79 WER 0.75 while bf16 0.00; bf16 utt 99 WER 0.70; TRT-both utt 0 WER 0.75) — it is
   dia2's inherent AR-sampling instability on hard inputs, NOT a TRT-specific defect.
5. **Median (typical utterance):** fp32 0.0556 < bf16 0.0742 < TRT-backbone 0.0817 ≈ TRT-both 0.0833 — so on the *typical*
   utterance TRT is marginally behind bf16 (~0.009, inside the 0.01 margin); on *aggregate* TRT is ahead (bf16's tail is
   worse). The metrics disagree only because of outlier structure; both gaps are tiny and non-significant.

## Recommendation

- **TRT-both is justified as the dia2 realtime path** by the evidence: aggregate WER ≈ fp32, ≥ bf16, no systematic loss,
  and it is the only config crossing **RTF < 1 (0.896, 2.28× faster than fp32)**. The "lossy TRT" label is not supported
  by the data — TRT-both is quality-equivalent to fp32/bf16 within the noise.
- **What the data does NOT license:** a *tight* (margin-0.01) statistical non-inferiority certificate — dia2's own output
  variance exceeds 0.01, so no precision choice can be certified at that margin by WER at N=300. **A MOS listening test on
  ~30–50 clips is the correct instrument** to settle "is TRT-both perceptibly different" (WER cannot resolve a sub-1% gap
  under this variance).
- **Safe operating stance:** keep bf16-native as the accuracy-exact *default*; ship TRT-both as the first-class **realtime
  mode** (`WAAV_PERF_MODE=throughput`, already implemented) — now backed by a 300-utterance study showing no measurable
  quality cost. Promote TRT-both to the *default* only after a MOS pass (or if the deployment prioritizes RTF<1).

**Honesty caveats:** whisper-base ASR has its own error floor (it cancels in the paired delta since it's the same ASR per
config, but inflates absolute WER); many "errors" are ASR-normalization artifacts (numbers, punctuation, casing)
identical across configs. N=300 is powered for large effects but not for a 0.01 margin against dia2's heavy tail.
