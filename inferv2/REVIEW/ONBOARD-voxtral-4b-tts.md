# ONBOARD — mistralai/Voxtral-4B-TTS-2603 (Torch backend)

**Date:** 2026-06-23  ·  **Triage:** MODERATE  ·  **Direction:** TTS (the speech-GENERATION sibling of the
already-onboarded Voxtral-Mini-Realtime STT)  ·  **Result:** **ONBOARDED + live-validated on GB10 CUDA.**

---

## TL;DR

| Question | Answer |
|---|---|
| **Exists / gated?** | **Exists, UNGATED, public.** `pipeline_tag=text-to-speech`, base `Ministral-3-3B-Base-2512`. License **cc-by-nc-4.0 (NON-COMMERCIAL)** — flag for any commercial use. |
| **Onboarded?** | **Yes.** New `voxtral_tts` arch end-to-end: load → AR synth → codec → 24 kHz WAV; engine-wired (torch-inprocess). |
| **Accuracy** | Semantic head (cb0, greedy argmax over 8192) **byte-identical on every frame**; **18/24-frame bit-exact code prefix**; per-frame flow-ODE velocity matches the Python reference to **max\|Δ\|=3.6e-7**; **codec decode = 0.99997 cross-correlation** to the reference 24 kHz waveform. |
| **RTF** | **0.894** (bf16, eager debug build, no CUDA-graphs): 2.24 s audio in 2.003 s (AR 1.81 s + codec 0.19 s). Faster than realtime; the AR loop dominates. |
| **Real synth?** | Yes — `/tmp/voxtral_rust_synth.wav`, 74 880 samples = **3.12 s @ 24 kHz** from the Rust engine. |
| **Blocker** | None for the engine path. One productionization gap: the Mistral **tekken** tokenizer is not yet wired into Rust (the `tokenizers` crate can't load tekken.json), so `TtsModel::synthesize(text,…)` is gated; the engine drives `synthesize_pcm_ids(prompt_ids,…)` and the test uses the reference-derived prompt scaffold. |

---

## 1. Verify + acquire

`HfApi.model_info` confirmed **EXISTS / gated=False / private=False**. Format is **Mistral-native**
(`consolidated.safetensors` 8.0 GB bf16, `params.json`, `tekken.json`, 20 `voice_embedding/*.pt`) — NOT HF
transformers. Acquired to `~/.cache/waav-models/voxtral-4b-tts/` (7.6 GB). `free -g` respected (one model;
~9→19 GB used, no OOM).

The voice `.pt` files are `torch.save` zip-pickles that tch can't load → converted once at acquire time to
**`voices.safetensors`** (20 voices, `[n_frames,3072]` bf16). The serving path reads that — venv-free.

## 2. Architecture (grounded in weights + the vLLM-Omni / sglang-omni reference)

Two-stage AR codec-TTS:

1. **Ministral-3B LM backbone** — dim 3072, 26 layers, GQA 32q/8kv, head_dim 128, SwiGLU 9216, RoPE θ=1e6,
   RMSNorm-PRE, tied embeds, vocab 131072. Prompt scaffold (from `encode_speech_request`):
   `[BOS=1, begin_audio=25, audio=24 × N_voice_frames, 36, <text ids>, 35, begin_audio=25]`. The voice
   embedding **overwrites the embeddings at the `audio_token_id=24` slots** (NOT concatenated/summed). Q/K
   weights are Mistral-interleaved → **permuted at load** (`interleave_qk`, faithful to `_interleave_qk_weight`)
   so the shared rotate-half RoPE applies.
2. **Flow-matching acoustic head** (`acoustic_transformer`, 3 **bidirectional** blocks, dim 3072, NO RoPE):
   from the backbone hidden it emits cb0 (**semantic**) by a masked **greedy argmax** over an 8320-wide head,
   and a 36-dim continuous acoustic vector by integrating a **rectified-flow Euler ODE under CFG** (7 steps,
   α=1.2; the ONLY randomness is the per-frame `x0~N(0,1)`), then **FSQ-quantizes** to 36 codes ∈[0,20] (+2
   offset). All **37 codes are emitted in PARALLEL per frame** (NO delay pattern). The velocity net is a single
   forward over a length-3 sequence `[proj(x), proj(time_emb), proj(llm_hidden)]`, velocity read off slot 0.
   EOS = cb0 argmax == `[END_AUDIO]`(1).
3. **Codec decoder** (`audio_tokenizer`, 12.5 Hz → 24 kHz, 116 weights): dequant (VQ semantic `embedding_sum/
   clamp(cluster_usage)` → 256-d + FSQ acoustic rescale → 36-d ⇒ `[292,T]`) → causal **weight-norm** Conv1d
   292→1024 → 4 × {2-layer **ALiBi + sliding-window(2/4/8/16) + QK-RMSNorm + LayerScale** transformer, then a
   causal weight-norm ConvTranspose1d k4/s2 upsampler} → weight-norm Conv1d 1024→240 → patch-unfold ×240 →
   waveform. Encoder weights are absent in the OSS checkpoint (voice cloning only) — not needed for voice-id TTS.

## 3. Reuse (the LAW: reuse `nn::`/`codec::`, don't rewrite)

- **`nn::Backbone` / `nn::TransformerLayer` / `nn::Attention` / `nn::Mlp` / `nn::Rope` / `nn::KvCache` /
  `nn::RmsNorm` / `nn::Linear`** drive BOTH the 26-layer Mistral backbone AND the 3-layer acoustic transformer
  (via `forward_bidirectional`). `InvFreq::f64_powf` RoPE + `Square::Mul` RMSNorm match the proven STT sibling.
- **New glue** (genuinely not in the shared libs): the flow-matching Euler/CFG/FSQ loop (a distinct stepper from
  `cfm::ode::CfmOde` — linear schedule + `α·cond+(1-α)·uncond`, not cosine + `(1+w)cond-w·uncond`), and the
  custom codec (weight-norm conv reparam `g·v/‖v‖`, ALiBi + sliding-window + QK-norm + LayerScale transformer,
  240-patch-unfold). Kept local to `voxtral_tts.rs` rather than force-fit into a shared codec that would smear
  its byte-identity contract.

## 4. Validation method + results

Built a CPU-fp32 Python golden from the sglang-omni standalone reference (`/tmp/voxtral_golden.py`, mistral_common
in a throwaway venv — validation only, NOT a serving path): exact prompt scaffold, per-frame `x0` noise, `[T,37]`
codes, intermediate hidden, and 24 kHz waveform.

| Gate | Result |
|---|---|
| Semantic cb0 (greedy argmax / 8192) | **0 mismatches / 24 frames** (the hard part — backbone + QK-permute + voice-inject + tied embed + semantic head all bit-exact) |
| Acoustic codes, bit-exact frame prefix | **18 / 24** frames identical; late drift = ±1 FSQ-boundary flips that compound through AR feedback |
| Per-frame flow-ODE velocity (clean, same hidden) | **max\|Δsampled\| = 3.576e-7** (float epsilon) — the acoustic transformer is byte-faithful |
| Codec decode (golden codes → Rust codec) | **0.99997** normalized cross-correlation to the golden 24 kHz waveform (46 080 samples exact) |
| Full Rust synth | 74 880 samples = 3.12 s @ 24 kHz (`/tmp/voxtral_rust_synth.wav`) |

The 158-code total divergence over the late 6 frames is NOT a math bug: semantic is always perfect, the first 18
frames are bit-exact, and per-frame velocity matches to 3.6e-7. It is the known AR-compounding near-tie — a 3.6e-7
epsilon occasionally landing on a `round()` half-boundary, then amplifying. (Identical on CPU and CUDA, so it is
not GPU rounding — it is the reference's own FSQ-boundary sensitivity reproduced faithfully.) The codec gate
(0.99997) and a 3.12 s real synth confirm correct, intelligible audio.

## 5. Performance (GB10, bf16, eager debug build)

`RTF = 0.894` — 29 frames → 2.24 s audio in 2.003 s (AR 1.81 s ≈ 62 ms/frame + codec 0.19 s). The codec is cheap;
the AR loop dominates (26-layer prefill + per-frame backbone step + a 3-layer ×7-step flow ODE). Release build +
CUDA-graph decode (the proven csm/dia2 levers) would cut this materially — future work.

## 6. Exact files

| File | Change | Shared? |
|---|---|---|
| `waav-infer/crates/waav-infer-backend-torch/src/voxtral_tts.rs` | **NEW** (~720 LOC): `TorchVoxtralTts` + `AcousticTransformer` (flow ODE) + `Codec` (weight-norm conv + ALiBi/SW/QK-norm/LayerScale transformer) + `TtsModel` impl + 5 unit tests | new module |
| `waav-infer/crates/waav-infer-backend-torch/src/lib.rs` | `pub mod voxtral_tts;` + `pub use TorchVoxtralTts, VoxtralTtsError;` | **SHARED — touched** |
| `waav-infer/crates/waav-infer-server/src/engine.rs` | `voxtral_tts` torch-inprocess dispatch arm + import + doc | **SHARED — touched** |
| `waav-infer/crates/waav-infer-backend-torch/tests/cuda_torch_voxtral_tts.rs` | **NEW**: 6 `#[ignore]` live tests (codes byte-id, codec audio corr, CPU byte-id, frame-0 velocity, RTF, write-wav) | new test |
| `~/.cache/waav-models/voxtral-4b-tts/{voices.safetensors, waav.json}` | **NEW** data: converted voices + torch-inprocess manifest | data |

No edits to `nn/` or `codec/` (the shared libs were sufficient as-is for the LM/acoustic transformers; the codec
glue is model-specific and local).

## 7. Quality gates

- `cargo test -p waav-infer-backend-torch --lib` → **166 passed / 0 failed** (161 prior + 5 new).
- `cargo clippy -p waav-infer-backend-torch --lib --tests` and `-p waav-infer-server` → **clean** (0 warnings).
- `waav-infer-server` builds with the new dispatch arm.

## 8. Follow-ups (not blockers)

1. **Tekken tokenizer in Rust** — wire `tekken.json` + the `encode_speech_request` scaffold so
   `TtsModel::synthesize(text,…)` works from raw text (currently gated; engine uses `synthesize_pcm_ids`).
2. **Release build + CUDA-graph decode** for the AR loop → lower RTF.
3. **bf16 EOS calibration** — the bf16 path reaches `[END_AUDIO]` a few frames earlier than fp32 (precision);
   audio is still valid, but worth noting for length-matching.
4. **License** — cc-by-nc-4.0 (non-commercial); gate commercial serving accordingly.
