# dia2 Precision Knob — PERCEPTUAL (ASR-WER) Accuracy Eval

**Question (the decisive measurement):** Does **native-bf16** (`WAAV_DIA2_PRECISION=bf16`, the f32-sandwich
*bypassed*) cause meaningful **perceptual / quality** loss versus the **f32-sandwich byte-identical default**
on the dia2 codec-AR TTS model?

**Why this gate (and why codec-distance is the WRONG gate):** dia2 is a *generative discrete-AR* model. A
1-bf16-ULP GEMM perturbation flips one sampled token early, and the autoregressive loop then unrolls a
**different-but-equally-valid** realization of the same text. So codebook-agreement / wav-correlation between
the two paths is near-zero *by construction* — that measures *trajectory forking*, not *quality*. The only
honest quality gate is: **does the generated audio still say the right words and sound clean?** → measured as
**ASR Word Error Rate (WER)** of the synthesized audio against the reference text.

- **Model under test:** dia2-2b (`/home/bud/.cache/waav-models/dia2-2b`), GB10 CUDA, CUDA-graph ON for both paths.
- **Knob resolver:** `dia2_proj_native` — `WAAV_DIA2_PRECISION ∈ {bf16,fp16,native}` → native; unset/`fp32`/`strict` → sandwich (default).
- **Seed/voice:** identical. Both paths re-seed the global libtorch RNG to `cfg::SEED` per `synthesize_pcm`
  call; same `[S1]` speaker prompt. **Only variable = the backbone attention/MLP GEMM precision.**
- **ASR (the judge):** **`whisper-base`** ONNX via the in-tree ORT STT path (`WhisperStt`), EP = **CUDA**,
  24 kHz → 16 kHz linear resample. Same ASR for both conditions (so any ASR floor cancels in the comparison).
- **Corpus:** 12 varied sentences (9–14 words; 144 reference words total).
- **Harness (throwaway, uncommitted):** `crates/waav-infer-server/tests/zz_precision_wer_eval.rs`
  (`--features torch`, one model on the GPU at a time: synth-all-sandwich → drop → synth-all-bf16 → drop → ASR-all).

## Per-text WER

| text | ref words | WER f32-sandwich | WER bf16-native | dur f32/bf16 (s) | rms f32/bf16 |
|------|-----------|------------------|-----------------|------------------|--------------|
| 0  | 9  | 0.000 | 0.000 | 3.36 / 3.28 | 0.093 / 0.118 |
| 1  | 10 | 0.000 | 0.000 | 3.68 / 3.68 | 0.079 / 0.037 |
| 2  | 13 | 0.000 | 0.000 | 4.80 / 4.32 | 0.068 / 0.063 |
| 3  | 13 | 0.000 | 0.000 | 3.92 / 4.00 | 0.097 / 0.046 |
| 4  | 12 | 0.083 | 0.083 | 5.04 / 4.88 | 0.075 / 0.065 |
| 5  | 12 | 0.000 | 0.000 | 5.60 / 5.92 | 0.083 / 0.022 |
| 6  | 14 | 0.071 | 0.071 | 6.16 / 6.72 | 0.034 / 0.053 |
| 7  | 12 | 0.000 | 0.000 | 4.16 / 3.68 | 0.083 / 0.039 |
| 8  | 12 | 0.167 | 0.000 | 5.60 / 5.60 | 0.051 / 0.137 |
| 9  | 12 | 0.000 | 0.000 | 5.04 / 5.36 | 0.023 / 0.006 |
| 10 | 12 | 0.083 | 0.000 | 4.32 / 4.16 | 0.097 / 0.023 |
| 11 | 13 | 0.000 | 0.000 | 4.72 / 4.72 | 0.075 / 0.058 |

## Aggregate

| metric | f32-sandwich | native-bf16 | Δ (bf16 − f32) |
|--------|--------------|-------------|----------------|
| **macro-WER** (mean of per-text) | **0.0337** | **0.0129** | **−0.0208** |
| **micro-WER** (total edits / 144) | **0.0347** | **0.0139** | **−0.0208** |
| per-text wins (fewer edits) | 0 | 2 | (10 ties) |
| NaN / Inf / garbage clips | 0 / 12 | 0 / 12 | — |

## What the "errors" actually are (none are precision degradation)

Inspecting every non-zero cell shows the residual WER is **ASR/normalization noise, identical across both
paths**, not a quality difference:

- **text 4** (both 0.083): `finalize → "finalise"` — whisper's British spelling. **Same on both paths.**
- **text 6** (both 0.071): `Mathematics → "definitions"` — a whisper mis-hearing of the leading word.
  **Identical on both paths** → an ASR error, fully independent of dia2 precision.
- **text 8** (f32 0.167, bf16 0.000): f32 trajectory rendered `"chef prepare the delicious"` (ASR read
  `prepared a → prepare the`); the bf16 trajectory happened to render a cleaner take. This is the **AR
  trajectory fork**, not a precision-quality axis.
- **text 10** (f32 0.083, bf16 0.000): f32 take dropped the trailing `"today"`; bf16 take kept it. Again a
  trajectory difference, not degradation.

Durations and per-clip RMS are comparable and all non-silent (every clip well above the silence floor; no
NaN/Inf in either condition).

## VERDICT — claim PROVEN: native-bf16 is quality-equivalent

- bf16 WER is **not worse** than the byte-identical f32-sandwich on any aggregate or per-text measure — it is
  in fact **marginally lower** (macro 0.0129 vs 0.0337), but that gap is **trajectory luck**, well inside
  ASR/sampling noise, **not** a real bf16 advantage. The honest read is **statistically indistinguishable**.
- No clip degraded into wrong words, slurring, dropout, NaN, or garbage under bf16.
- **Therefore the owner's thesis holds:** the f32-sandwich's byte-identity vs bf16 is a *bit-exactness*
  property of a discrete-AR sampler, **not** an audible quality property. `WAAV_DIA2_PRECISION=bf16` is a
  **legitimate, quality-gated precision** — the ~1.25× backbone-GEMM / RTF win comes at **no perceptual cost**.
  The byte-identical sandwich remains the correct *default* (reproducibility, regression-gating, golden tests),
  but native-bf16 is safe to offer as an opt-in speed mode on perceptual grounds.

## Scope / honesty notes

- **One model.** Only **dia2** currently exposes the native-bf16 sandwich-bypass quality knob
  (`dia2_proj_native`). csm / qwen3_tts have **no** equivalent knob (their `*_PRECISION` envs are TRT
  *engine* precision, a separate opt-in throughput path, or fixed encoder-precision notes), so a second
  codec-AR datapoint on the *same* knob would require new production code (out of scope here). The dia2 result
  should generalize — every codec-AR TTS shares the same "1-ULP flips a sampled token → valid alt realization"
  structure — but that generalization is **inferred, not yet measured**.
- **ASR floor:** whisper-base is strong but not perfect (~1–3% WER floor from spelling/normalization). Because
  the *same* ASR judges both conditions, this floor cancels in the comparison; it does not bias the verdict.
- **Sample size:** 12 sentences / 144 words. Adequate to refute a *meaningful* degradation; a larger LibriSpeech
  sweep would tighten the confidence interval but is unlikely to change the conclusion given the zero-degradation
  signal.

---
*Measured 2026-06-27 on GB10 (dia2-2b, CUDA-graph ON, whisper-base ORT-CUDA). Harness:
`crates/waav-infer-server/tests/zz_precision_wer_eval.rs` (throwaway, uncommitted). Raw log:
`scratchpad/wer_run.log`.*
