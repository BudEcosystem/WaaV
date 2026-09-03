# Onboard: ZzWater/ViiTorVoice-NAR (12-codebook masked-diffusion NAR TTS + DualCodec)

**Status: VERIFIED + REAL SYNTH (reference) — NOT YET wired into the Rust engine. The model EXISTS,
is ungated, and was synthesized END-TO-END on the actual shipped weights (embeddings/heads from
`model.safetensors` + the shipped fp32 backbone ONNX + the shipped DualCodec decoder ONNX). The
output is ASR-verified intelligible speech and the masked-diffusion sampler is seed-deterministic
(codes bit-identical across runs → a clean byte-faithful target). It is a GENUINELY DISTINCT model
from the already-onboarded `omnivoice` (k2-fsa): different codec (DualCodec, not HiggsAudioV2 DAC),
12 codebooks (not 8), mixed vocab, split semantic/acoustic heads, forced aligner + emotion/NVV tags.
The Rust integration is a NEW `viitorvoice.rs` model file (NOT a config+weights drop-in), but it
REUSES two existing seams verbatim: `cfm::masked::MaskedDiffusion` + `nn::Backbone::forward_bidirectional`.**

| | |
|---|---|
| Model | ViiTorVoice-NAR — non-autoregressive masked-LM TTS; voice cloning, local speech editing, emotion/paralinguistic control. zh/en/ja/ko |
| Arch (config) | `architectures: ["ViiTorVoice"]`, `model_type: omnivoice` — a BIDIRECTIONAL **Qwen3-0.6B** (28L, h1024, 16q/8kv, head_dim128, SwiGLU 3072, RMSNorm 1e-6) emitting **12 DualCodec codebooks** via MaskGIT/LLaDA iterative confidence-ranked parallel unmasking (32 steps), decoded by **DualCodec @ 25 Hz** |
| Triage tier | MODERATE |
| Official repo | `ZzWater/ViiTorVoice-NAR` — **VERIFIED EXISTS, ungated** (`HfApi.model_info`: gated=False, private=False) |
| Upstream code | `github.com/viitor-ai/viitor-voice-nar` (cloned; `viitorvoice/llm/{model,generation,text_utils,runtime}.py`, `viitorvoice/codec/decoder.py`) |
| Onboarding | **NEW model file** (`viitorvoice.rs`) reusing `cfm::masked` + `nn::Backbone` + the ORT codec seam. NOT done in this pass (research + reference-synth validation only; no Rust touched). |
| Accuracy | **Reference synth ASR-verified** ("Nice to meet you. This is a non autoregressive voice." → ASR "Nice to meet you. This is a non-autoragressive voice."). **Seed-deterministic: codes BIT-IDENTICAL across runs (0/948 disagreement)** → byte-faithful bar is achievable (deterministic NAR, exactly the omnivoice gate's regime). |
| Live RTF | **CPU 2.19** (python onnxruntime is CPU-only on aarch64 — no CUDA EP wheel); **GB10 GPU ~0.33** (measured: torch-CUDA fp32 backbone = 29.1 ms/step × 32 = 931 ms for 3.16 s audio → RTF_backbone 0.295; + one-shot codec ≈0.1 s). Real-time-capable. |
| Footprint | `~/.cache/waav-models/viitorvoice-nar/` = **4.2 GB** (minimal zero-shot TTS path: LLM 2.2 GB + backbone ONNX 1.68 GB + DualCodec decoder ONNX 523 MB + silence asset). Skipped (cloning/editing-only): aligner 1.84 GB + w2v-bert 2.32 GB + encoder ONNX 197 MB. |

---

## 1. HfApi verification (method step 1)

`HfApi.model_info('ZzWater/ViiTorVoice-NAR', token=HF_TOKEN)` → **EXISTS, gated=False, private=False**,
`pipeline_tag=text-to-speech`, tags include `onnx, safetensors, TTS, speech-edit, zh/en/ja/ko,
base_model:Qwen/Qwen3-0.6B`. File tree (verified via `list_repo_files`):

```
config.json                                              # weights_md5 manifest only
llm/0p6_emotion/                config.json, model.safetensors(2.2G), tokenizer.json,
                               train_config.json, cache/onnx_backbone_fp32/llm_backbone_dynamic.onnx(1.68G)
dualcodec/dualcodec_ckpts/      dualcodec_25hz_16384_1024.safetensors(409M),
                               dualcodec_decoder.onnx(268M)+.onnx.data(268M), dualcodec_encode_core_30s.onnx(197M),
                               w2vbert2_mean_var_stats_emilia.pt
dualcodec/w2v-bert-2.0/         Wav2Vec2BertModel, model.safetensors(2.32G)   [encoder-only; not needed for zero-shot]
aligner/Qwen3-ForcedAligner-0.6B/  Qwen3ASRForConditionalGeneration(1.84G)    [editing-only]
assets/dualcodec_silence_2s.pt
```

**Is it a known stack?** It is a SIBLING of the already-onboarded `omnivoice` (k2-fsa) — same
masked-diffusion-LM *family*, same bidirectional Qwen3-0.6B *backbone family*, IDENTICAL sampler
constants. The `llm/0p6_emotion/config.json` even carries `model_type: omnivoice` and the upstream
README/configs say "inspired by OmniVoice and DualCodec". But it is **architecturally distinct enough
to require its own model file** (see §2).

## 2. Why the existing `omnivoice.rs` cannot serve it (the divergence)

| Aspect | onboarded `omnivoice` (k2-fsa) | **ViiTorVoice-NAR** |
|---|---|---|
| codebooks | 8 (uniform) | **12** (1 semantic + 11 acoustic) |
| audio vocab | 8×1025 (uniform) | **mixed**: semantic 16387, acoustic 11×1027 (`audio_codebook_sizes`) |
| embed | single `audio_embeddings [8200,1024]`, uniform offsets, summed | **split**: `semantic_embedding[16387]` + `acoustic_embedding[11*1027]` (offsets `c*1027`), summed, masked |
| head | single `audio_heads [8200,1024]` → `[1,s,8,1025]` | **two heads**: `semantic_head→16387` + `acoustic_head→11*1027` reshaped `[1,11,s,1027]` |
| backbone | tch from safetensors (eager, cache-free bidirectional) | shipped **fp32 ONNX** `llm_backbone_dynamic.onnx` (`inputs_embeds[b,s,1024]+attention_mask[b,1,s,s]→hidden[b,s,1024]`) |
| codec | **HiggsAudioV2 DAC** (RVQ Euclidean, 8 cb), torch-native, 24 kHz | **DualCodec** (semantic+acoustic split), **ONNX decoder**, 25 Hz/960-hop |
| extras | none | forced aligner (editing), emotion `<\|emotion-*\|>` + NVV `(laughs)` tags, no_ref_text mask-text, duration tokens, pause anchors |

The **sampler is identical** (verified against upstream `generation.py`): `num_step=32, t_shift=0.1,
layer_penalty_factor=5.0, position_temperature=5.0, guidance_scale=0.0` default; per-step
`log_softmax → CFG-combine → forbid MASK id → argmax/max → −layer_id·5.0 → +Gumbel(/5.0) → forbid
revealed → topk reveal`; schedule `t_i = 0.1·u/(1+(0.1−1)·u)` over `linspace(0,1,33)`, `k_step =
min(ceil(C·T·Δt), rem)`. This is **byte-for-byte** the recurrence in `cfm::masked::MaskedDiffusion`.

## 3. Acquire (step 2)

Downloaded the **minimal zero-shot TTS path** (4.2 GB) to `~/.cache/waav-models/viitorvoice-nar/` via
`hf_hub_download` (HF_TOKEN): LLM `model.safetensors` + `tokenizer.json` + `config.json`, the fp32
backbone ONNX, the DualCodec decoder ONNX (+ external `.onnx.data`), and `dualcodec_silence_2s.pt`.
Skipped the aligner (1.84 GB, editing-only) + w2v-bert (2.32 GB, cloning-encoder) + encode ONNX
(197 MB, cloning-encoder) — none are on the zero-shot text→speech path.

## 4. Reference synthesis (steps 4–5) — REAL, on the shipped weights

A faithful Python reference (`/tmp/vv_ref_synth.py`, **throwaway validation, NOT a serving path** —
per [[waav-infer-no-venv-wrap]]) runs the ACTUAL shipped weights end-to-end (zero re-impl of the
transformer): torch embeddings/heads from `model.safetensors` → backbone via the shipped fp32 ONNX
(onnxruntime) → masked-diffusion 32-step unmask (mirroring `_generate_iterative` +
`_predict_tokens_with_scoring`) → DualCodec decode via the shipped ONNX (`semantic_codes[1,1,T]` +
`acoustic_codes[1,11,T]` → `audio[1,1,960·T−4]`, with 25-frame silence padding, `tail_trim=4`).

Text "Nice to meet you. This is a non autoregressive voice." (duration estimator → **T=79 frames =
3.16 s**, prompt S=113, astart=34):

- **codes** `[12,79]`, valid ranges sem∈[76,16207] aco∈[1,1021] (within vocab, no MASK/sep leakage).
- **wav** 75 836 samples = 3.16 s @ 24 kHz, rms 0.083, no clipping (max |0.46|).
- **Speech-real**: frame-energy dynamic range 2.87 (speech >2; flat noise ≈1), spectral centroid
  2462 Hz (speech band), proper silences (min-energy frames = 0).
- **ASR round-trip** (whisper-base, fed the array directly — no ffmpeg): output **"Nice to meet you.
  This is a non-autoragressive voice."** — matches the input; the only delta ("autoregressive" →
  "autoragressive") is an ASR phonetic-spelling artifact, not a synthesis error. ✅ intelligible.

## 5. Accuracy / byte-faithfulness

NAR masked-diffusion is **deterministic under a fixed seed** (the one RNG is the per-step Gumbel
`torch.rand_like`, seeded `torch.manual_seed(0)`). Two seeded runs produced **bit-identical codes
(0/948 frame disagreements)** and identical wav stats. This is the SAME byte-faithful regime the
onboarded `omnivoice` gate relies on (tch == libtorch ⇒ identical MT19937 ⇒ identical Gumbel ⇒
identical reveal order ⇒ identical codes). So a future Rust port is gateable **byte-for-byte** against
this reference (codes-level), with the backbone/heads in tch-CUDA fp32 (the all-f32 + eager-attention
case — the cleanest of the 8-bug playbook) and the DualCodec decode via the ORT seam (ONNX is the
ground truth, identical EP ⇒ identical bytes). The codes golden is saved at
`/tmp/vv_ref_out/codes.npy` (+ `wav.npy`, `synth.wav`).

## 6. Perf / RTF on GB10 (step 6)

- **Python reference (CPU)**: RTF **2.19** — but `onnxruntime` on this aarch64 box is **CPU-only**
  (`get_available_providers()` = `[Azure, CPU]`; no CUDA EP wheel — the known aarch64-ORT gap). So
  this is a CPU number, not representative of GB10.
- **GB10 GPU (true)**: ran the dominant cost — the 32 backbone forwards — in **torch-CUDA fp32**
  (Qwen3Model loaded from the safetensors `llm.*`, 0 missing/unexpected keys, bidirectional all-zero
  mask): **29.1 ms/step × 32 = 931 ms** for 3.16 s audio → **RTF_backbone ≈ 0.295**. With the one-shot
  DualCodec decode (~0.1 s on GPU), **total GB10 RTF ≈ 0.33** — comfortably real-time. The existing
  omnivoice **CUDA-graph** seam (B46) would cut this further (captured fixed-shape replay).

## 7. Disposition + remaining work to fully onboard

**Honest disposition: VERIFIED real + reference-validated; Rust integration deferred (a new model
file, not a drop-in).** This pass did research + a real reference synth + accuracy/RTF; it did **not**
touch any Rust (so `cargo test`/clippy are unaffected). To complete the in-engine onboarding:

1. New `crates/waav-infer-backend-torch/src/viitorvoice.rs` (a `TtsModel`) — COMPOSING:
   - `nn::Backbone::forward_bidirectional` (the bidirectional Qwen3, cache-free) — reused, OR run the
     shipped fp32 backbone ONNX through the ORT seam (faster to land, ONNX is the byte-truth).
   - `cfm::masked::MaskedDiffusion` — reused verbatim (constants already match: NSTEP32/GS-as-config/
     TSHIFT0.1/LPF5.0/PT5.0). Needs a 12-codebook `MaskedLogits` provider with the **split
     semantic/acoustic** heads + the forbid-MASK-per-head logic (`audio_mask_ids[0]`/`[1]`).
   - a new DualCodec-decode glue over the **ORT backend** (`waav-infer-backend-ort`): bind
     `semantic_codes[1,1,T]`+`acoustic_codes[1,11,T]` → `audio`, with the 25-frame silence-pad +
     960-hop trim from `decoder.py`. CUDA EP is available in the Rust `ort` crate (unlike the python
     wheel) — this is where the GB10 codec RTF lives.
   - new glue: the split embed (`semantic_embedding` + `acoustic_embedding` w/ `c*1027` offsets,
     masked), the prompt build (`<\|lang_start\|>None<\|lang_end\|><\|instruct_start\|>None<\|instruct_end\|>`
     + `<\|text_start\|>…<\|text_end\|>`, each id replicated ×12, + fully-MASKed target), and the
     `RuleDurationEstimator` (char-weight; ref "Nice to meet you."@25, low-threshold-50 boost).
2. A `waav.json` manifest (arch `viitorvoice`, fp32) + an `engine::load_model_at` dispatch arm.
3. A golden-dump (`dump_viitorvoice_golden.py`, seed 0) + a `cuda_torch_viitorvoice` byte-faithful
   test (codes 948/948 == golden; the deterministic regime makes this clean) + an RTF test.
4. (Later) the aligner + w2v-bert + encode-ONNX for voice-cloning / local-editing modes.

## 8. Exact files

**Acquired** (no Rust touched, no commit): `~/.cache/waav-models/viitorvoice-nar/` (4.2 GB).
**Reference artifacts** (`/tmp`, throwaway): `vv_ref_synth.py`, `vv_ref_out/{synth.wav,codes.npy,wav.npy}`.
**Upstream clone**: `/tmp/viitor-voice-nar/` (`viitorvoice/llm/{model,generation,text_utils,runtime}.py`,
`viitorvoice/codec/decoder.py` — the byte-faithful reference for the future port).
**Reuse seams** (existing, unchanged): `crates/waav-infer-backend-torch/src/cfm/masked.rs`
(`MaskedDiffusion`/`MaskedLogits`), `…/src/nn/backbone.rs` (`forward_bidirectional`),
`crates/waav-infer-backend-ort/` (the ONNX codec seam). Sibling reference:
`…/src/omnivoice.rs` (the 8-cb k2-fsa variant — same family, different codec/cb/heads).
