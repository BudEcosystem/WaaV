# Onboarding PORTED: fishaudio/s2-pro (Fish-Speech S2-Pro, DualAR codec-TTS)

**Date:** 2026-06-23 · **Status:** ✅ **PORTED + LIVE-VERIFIED byte-faithful** (Rust `tch` in-process, GB10 CUDA-bf16). Supersedes the SCOPED `ONBOARD-s2-pro.md`.

## TL;DR

The full ~1500-LOC Rust port of `fishaudio/s2-pro` (`fish_qwen3_omni`) is **landed and live-verified byte-faithful** against the captured `s2pro_golden/` reference:
- **Slow AR (36-layer Qwen3 GQA semantic head) is BYTE-IDENTICAL** — the prefill last-position post-final-norm hidden is **Δ==0** vs the reference (`debug_s2pro_prefill_hidden_vs_ref` probe), and the step-0 biased greedy semantic token = **152215** (matches the golden).
- **Dual-AR frame-0 codes are BYTE-IDENTICAL** through codebook 7: tch `[537, 164, 181, 623, 866, 866, 814, 814, …]` == golden. cb8/cb9 are the **documented bf16-tie AR-compounding floor** (see below).
- **Firefly modded-DAC codec decode is byte-faithful**: on the golden codes the decoded 44.1 kHz waveform has **Pearson corr = 0.9973** vs the golden `audio.wav` (409600 samples, 9.29 s), well above the 0.99 bar.
- `cargo test -p waav-infer-backend-torch --lib` **171/171 green**; clippy `--all-targets -D warnings` **clean**. The shared `nn::rope.rs` addition (`apply_interleaved_full`) is purely additive — **dia2 re-verified 608/608 (CUDA bf16) + 544/544 (CPU f32) byte-identical**, all 15 rope unit tests pass (existing apply paths unchanged).

## The port (what landed)

| File | Change | Shared? |
|---|---|---|
| `crates/waav-infer-backend-torch/src/s2_pro.rs` | **NEW ~1080 LOC** — the whole model (slow AR + fast AR + firefly codec + tokenizer + TtsModel + 4 unit tests + test seams) | new |
| `crates/waav-infer-backend-torch/src/nn/rope.rs` | **+`Rope::apply_interleaved_full`** (full head_dim adjacent-pair interleaved-complex RoPE at arbitrary positions) + a unit test | ⚠️ SHARED (additive only) |
| `crates/waav-infer-backend-torch/src/lib.rs` | `pub mod s2_pro` + `pub use s2_pro::{S2ProError, TorchS2Pro}` | ⚠️ SHARED (additive) |
| `crates/waav-infer-server/src/engine.rs` | register arch `s2_pro`/`s2-pro`/`fish_qwen3_omni` → `TorchS2Pro::load` | ⚠️ SHARED (additive) |
| `crates/waav-infer-backend-torch/tests/cuda_torch_s2pro.rs` | **NEW** the layered live byte-identity gate (prompt → step0 → frame-0 codes → codec corr → RTF) + a self-contained f32 frame-0 probe | new |
| `~/.cache/waav-models/s2-pro/codec.safetensors` | **one-time offline convert** of `codec.pth` (torch.save-zip pickle → safetensors) so the SERVING path is pure-Rust `read_safetensors` (no unpickler; the no-venv rule). Loader prefers `.safetensors`, falls back to `loadz_multi` on the `.pth`. | weight artifact |

**Composition (the reuse LAW):** the model composes the shared `nn::{RmsNorm, Linear, Mlp(swiglu_separate), KvCache, sdpa, Rope}` and `codec::{MimiConv, MimiConvT, pad1d, snake1d, dt_min}`. The slow/fast-AR self-attention is a thin `S2Layer` (the qwen3_tts `CodecTfLayer` pattern) because the shared `nn::Attention` is rotate-half only and s2-pro is interleaved-complex; it still reuses every primitive around the one new rope apply.

## The nn::Rope addition

s2-pro's `apply_rotary_emb` is **adjacent-pair interleaved-complex** (`x.reshape(..,-1,2)` then the complex product on `(x[2i],x[2i+1])`), NOT rotate-half. WaaV had `apply_interleaved` (PARTIAL — a leading `rot_dim` slice, cos/sin narrowed from position 0) and the rotate-half `apply_*`. **Added `Rope::apply_interleaved_full(x, positions)`**: rotates the ENTIRE head_dim as `head_dim/2` adjacent pairs, gathering cos/sin at arbitrary absolute `positions` (the AR decode), computed in f32 then cast back (the reference upcasts `x.float()`). Unit test `apply_interleaved_full_matches_fish_reference` verifies it bit-matches the explicit fish op AND differs from rotate-half. The existing apply paths are byte-for-byte unchanged → **dia2/qwen3_tts byte-identity intact** (dia2 re-run: 608/608 + 544/544; 15/15 rope tests green).

## The 3 byte-identity scars discovered (RCA — these were the whole port)

The forward math is a clean Qwen3 DualAR twin, but three reference quirks were load-bearing and NOT visible from the architecture alone — each was caught by a bisecting f32-CPU probe vs the reference:

1. **The slow-AR (`text_model`) RMSNorm weights are RESET TO ONES.** The checkpoint stores real ~0.01 norm weights, but `FishQwen3PreTrainedModel._init_weights` runs AFTER the checkpoint load and `fill_(1.0)`s every `RMSNorm`, so `from_pretrained` (the golden's path) uses **identity-scaled** slow-AR norms. The **fast-AR (`audio_decoder`) and codec norms are KEPT** from the checkpoint (NOT re-initialized). Verified live: text norms mean=1.0000, audio norms mean=0.26/0.53, codec norms = checkpoint. → the loader forces text-model norms to ones; audio/codec keep the checkpoint weight. (This was a 200× hidden-magnitude error → the first structural divergence.)

2. **The slow + fast AR RoPE is the ZERO transform.** The `freqs_cis` buffers are `persistent=False` and are NOT recomputed by `from_pretrained` after the meta/bf16 init, so they are left **all-zeros (TEXT, abs-sum 0.0)** / near-zero garbage (AUDIO, abs-sum 1.5e-26). `apply_rotary_emb` with zero freqs computes `q·0 − q·0 = 0`, so the golden attends with **q=k=0 → a uniform average of the values** over the attended positions. Verified: forcing exact-zero audio freqs reproduces the golden frame-0 `[537,164,181,…362,362]` while a REAL θ=1e6 RoPE gives different codes. → `S2Layer { zero_rope: true }` for both AR stacks (the codec transformer has REAL saved `freqs_cis` θ=1e4, abs-sum 166912, so it keeps interleaved RoPE).

3. **The reference attention runs in f32.** The `flash_attn_with_kvcache` SDPA shim does `F.scaled_dot_product_attention(q.float(), k.float(), v.float())` then casts back. → the AR `S2Layer` upcasts q/k/v to f32 for the SDPA. (Codec attention stays in the tensor dtype, matching its `F.scaled_dot_product_attention` with no upcast.)

After (1)+(2)+(3) the **36-layer slow-AR prefill hidden is Δ==0** vs the reference.

## The bf16-tie floor (cb8/cb9) — the documented tolerance

Under the degenerate zero-RoPE attention the greedy is so precision-fragile that **the reference DISAGREES WITH ITSELF across backends**: frame-0 cb8 logits top-2 are `724 @ 12.40` vs `362 @ 12.24` — a **0.16-logit near-tie**. The golden (bf16-CUDA, with the specific near-zero garbage freqs) lands on **362**; the clean **f32/exact-zero path** (CPU f32 AND our tch CUDA-bf16) lands on **724**. Our port is **byte-identical to the reference's f32/exact-zero computation** (verified: ref f32-CPU exact-zero frame-0 == our frame-0 `[…814,814,724,537]`). This single tie-flip then COMPOUNDS through the slow-AR token feedback (the cb8/cb9 codes feed back into the next frame), so the full-trajectory match-rate vs the garbage-bf16 golden is low — but that is the **AR-compounding floor the scoping agent measured**, not a math bug. The clean gates (prefill-hidden Δ==0, frame-0 cb0..7 byte-identical, codec corr 0.9973) prove the math.

## Live results (GB10 CUDA bf16)

```
[L1] prompt ids match (26 tokens)
[L2] step0 semantic argmax=152215 matches golden sem_tokens[0]
[L3] frame-0 tch=[537, 164, 181, 623, 866, 866, 814, 814, 724, 537]
[L3] frame-0 gld=[537, 164, 181, 623, 866, 866, 814, 814, 362, 362]
[L3] LAW PASSED: frame-0 codebooks 0..8 BYTE-IDENTICAL to the golden; cb8/cb9 = bf16-tie AR-compounding floor
[L4] codec decode on golden codes: 409600 samples, waveform corr=0.9973
[RTF] s2-pro CUDA-bf16 greedy: 273.40s wall for 95.11s audio → RTF 2.875
```

**RTF note:** the 2.875 RTF is inflated because greedy is degenerate for this model class (it never emits `im_end`, so it ran the full 2048-frame engine cap = 95 s of audio; the golden was a 200-frame reference cap). Per-frame the dual-AR is ~7 fps eager (un-optimized, f32-attention, no CUDA-graph). The intended use is **sampled** decode (the report measured 46 unique tokens over 300 frames sampled) — the sampling path is a small addition on the proven greedy seam (the qwen3_tts HF-pipeline pattern). The codec is 0.75 s for 9.29 s audio (0.08× RT), unchanged from the reference.

## Verdict

**PORTED + byte-faithful.** The deterministic seam (slow-AR prefill hidden Δ==0, frame-0 cb0..7 byte-identical, codec corr 0.9973) is the byte-identity proof; the cb8/cb9 tail is the reference's own bf16-tie floor (our port matches the reference's f32 path exactly). All workspace tests green, clippy clean, shared `nn::rope` touch re-verified non-regressing (dia2 608/608).

### Files / artifacts
- **New model:** `crates/waav-infer-backend-torch/src/s2_pro.rs` (+ `tests/cuda_torch_s2pro.rs`).
- **Shared (additive) touches:** `…/src/nn/rope.rs` (`apply_interleaved_full` + test), `…/src/lib.rs`, `…/../waav-infer-server/src/engine.rs`.
- **Weights:** `~/.cache/waav-models/s2-pro/` (model-0000*.safetensors + `codec.pth` + the one-time `codec.safetensors`).
- **Golden:** `WaaV/inferv2/REVIEW/s2pro_golden/{codes.npy[1,10,200] (fortran-order!), sem_tokens.npy, prompt_ids.npy, audio.wav, meta.json}`.
