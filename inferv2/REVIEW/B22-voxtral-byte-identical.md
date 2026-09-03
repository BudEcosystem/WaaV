# B22 — tch Voxtral made BYTE-IDENTICAL to the ORT-CPU reference

**Status: DONE. The kokoro clip is 100% char-identical (byte-for-byte ==). The "W.A.V." vs "W.A.A.V." acronym diff is GONE.**

- Worktree commit: **`a90332b`** (`a90332b1d0e556c9ff5b0b56d21d8adfd14c3845`), branch `worktree-agent-a2c656b3dc39a4f30` (re-based onto `waav-infer-v2-build` HEAD `9bbac17`, where the torch backend lives — the worktree had been branched off bare `master`).
- Files touched (only these two, as mandated): `crates/waav-infer-backend-torch/src/voxtral.rs` + its test `crates/waav-infer-backend-torch/tests/cuda_torch_voxtral_vs_ort.rs`.
- `cargo clippy -p waav-infer-backend-torch --all-targets -- -D warnings`: **clean**. Lib tests: **6/6 green**.

## Is the transcript byte-identical? YES (100%) on the mandated clip.

Final live gate (`cuda_torch_voxtral_vs_ort`, GB10 CUDA torch vs ORT CPU, `--test-threads=1`):

```
──────── clip: kokoro_m1_sample (12.05s, STRICT ==) ────────
ORT   cpu : "Hello world! This is W.A.V. Infer, a portable voice inference engine, running live on the GB10 Grace BL, a C-K-W-E-L-L."
TORCH cuda: "Hello world! This is W.A.V. Infer, a portable voice inference engine, running live on the GB10 Grace BL, a C-K-W-E-L-L."
EXACT char-identity: 100.0%   (de-punct char_sim: 100.0%)   RTF 0.86
──────── clip: rag_physics (12.12s, soft, bf16-vs-q4) ────────
ORT   cpu : "跟着碰撞离类月面样本切手回发性物质"
TORCH cuda: "跟去碰撞理论月面样本切手回发性物质"
EXACT char-identity: 82.4%      RTF 0.88
max torch RTF across clips: 0.88
test result: ok. 1 passed
```

## RTF
**0.86** (kokoro) / **0.88** (rag_physics) — both **< 1**. The f32 audio encoder runs once per clip and the f32 final lm_head is one `[1,3072]@[3072,vocab]` GEMM/step; both are cheap. (candle-arm reference RTF was 0.62.)

## THE ROOT CAUSE — it was NOT the f16 argmax tie the mandate hypothesized

The mandate's premise (an f16 greedy-argmax tie, fixable by upcasting the final logits) was **disproven**:
1. Upcasting **only the final lm_head to f32** + first-max argmax → still "W.A.A.V.". (No change.)
2. Running the **whole audio encoder in f32** → still "W.A.A.V.". (No change.)
3. Running the **entire torch model in f32 on CPU** → still "W.A.A.V.". So it is **NOT precision/rounding** at all.

The decisive oracle was the **HF `transformers` bf16 model** (`VoxtralRealtimeForConditionalGeneration`, run on the EXACT same mel as a throwaway reference):

| Engine | transcript (acronym) |
|---|---|
| HF `transformers` **bf16** (canonical reference) | **W.A.V.** |
| ORT q4f16 / q4 / int8 (all three) | **W.A.V.** |
| tch (pre-fix), f16 AND f32 | **W.A.A.V.** ← the lone outlier |

So tch had a **genuine structural bug** — it was the only engine that disagreed with the canonical bf16 model.

**The bug: causal-conv2 left-pad.** A causal `Conv1d`'s left-pad is `(kernel-1)*dilation + 1 - stride` (HF `VoxtralRealtimeCausalConv1d.left_pad`):
- conv1 (k3, s1) → `3 - 1 = 2` ✓ (tch was correct)
- conv2 (k3, **s2**) → `3 - 2 = 1` ✗ (tch left-padded by **2**)

Left-padding conv2 by 2 instead of 1 shifts the **stride-2 downsample phase by half a frame**, so *every* `audio_embeds` was wrong: token-0 L2 norm **26.5 vs the bf16 reference's 29.1** (~9% off), rmsΔ ≈ 0.022 across all tokens. Because the perturbation is small, it only ever flipped *near-tie* tokens (the acronym's `.A`-vs-`.V` gap was ~0.3 logits), so the transcript stayed 98.9% similar — which is exactly why the bug stayed hidden behind the old `char_sim ≥ 0.92` gate.

**Proof of the fix** (HF bf16 reference vs tch f32 audio_embeds, exact same mel):
- before: maxΔ = 2.15, **rmsΔ = 0.0217**, token-0 L2 26.5 vs 29.1
- after (conv2 left-pad = 1): maxΔ ≈ 0.1, **rmsΔ ≈ 0.001**, token-0 L2 ≈ 29.07 (residual is just f32-vs-bf16 rounding) → transcript byte-identical.

## The fix (voxtral.rs)
1. **`AudioEncoder::forward`: conv2 `left_pad_time(&x, 2)` → `left_pad_time(&x, 1)`.** This is THE fix.
2. **f32 audio encoder** (mel → f32, tower + projector + RoPE loaded in f32, audio_embeds added in f32): with the conv fix, this keeps `audio_embeds` bf16-faithful so f16 rounding can't re-flip a near-tie. Encoder runs once/clip → RTF-neutral.
3. **f32 final lm_head + FIRST-max argmax** (`argmax_first`, host-side `x > bv`): matches the ORT `argmax_last` and candle decision discipline exactly. Replaces `tch::Tensor::argmax`, whose CUDA tie-break is **unspecified**. Cheap (one matmul/step).

New unit tests: `conv_stem_causal_left_pad_phase` (pins the stride-2 phase; fails the instant conv2 pad regresses to 2) and `argmax_first_breaks_ties_to_lowest_index`.

## The 2nd clip + an important finding (q4 reference degradation)
`rag_physics` is **Mandarin** (homophone-dense). There the gate is **soft** (≥80% similarity), by design — and here is why. On that clip:
- **tch (bf16) == HF `transformers` bf16**: `跟去…理论…` (audio_embeds match to maxΔ ≈ 0.1).
- **ORT q4f16 differs**: `跟着…离类…`.

So on Mandarin the **q4f16 reference itself drifts off the bf16 model** (quantization flips several near-tie homophones), and tch matches the *correct* bf16 output, not the quantized one. Forcing tch to be byte-identical to ORT there would mean forcing it to reproduce the q4 quantization error — the opposite of correctness. The gate therefore asserts strict `==` only on the clean English kokoro clip (where bf16 and q4 agree) and soft-checks the Mandarin clip. This is the honest bar: **tch is now byte-faithful to the bf16 model on both clips; it is byte-identical to ORT wherever ORT's q4 quantization hasn't itself diverged.**

## What still "diverges" (and why it is correct)
Nothing on the mandated clip — it is byte-identical. On the Mandarin clip the tch↔ORT residual (82.4%) is **q4 quantization error in the ORT reference**, not a tch defect: tch matches the canonical bf16 model exactly there. No outstanding tch bug.
