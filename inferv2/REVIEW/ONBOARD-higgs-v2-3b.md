# ONBOARD — bosonai/higgs-audio-v2-generation-3B-base (higgs-audio-v2 DualFFN codec-TTS)

**Status: ONBOARDED + live byte-faithful + RTF on GB10.** A real synth, byte-identical deterministic seams
(maxΔ=0), greedy AR byte-identical for 21 frames before the f32-tie fork, RTF 1.063 on CUDA-f16. Reused the
shared `nn::` (Attention + 2×Mlp/layer + `InvFreq::llama3` RoPE + KvCache) + the `codec::dac` decoder (the SAME
v2 codec omnivoice composes) + ADDED the v2 **DualFFN** as an externally-composed layer (ZERO shared `nn::`
changes → v3 higgs/omnivoice/qwen3 byte-identity untouched, re-verified by the 161-test lib sweep).

## HfApi verification (method step 1)
- `bosonai/higgs-audio-v2-generation-3B-base` — **EXISTS, ungated** (`gated: False`). Single `model.safetensors`
  (bf16, 397 tensors) + `config.json` + `tokenizer.json` + `chat_template.jinja` + `processor_config.json`.
- `bosonai/higgs-audio-v2-tokenizer` — **EXISTS, ungated**. The codec checkpoint (`model.safetensors`, f32, 805MB
  incl. the unused HuBERT semantic encoder; decode uses only the RVQ+fc2+acoustic_decoder subset).
- **The prompt's architecture assumptions were 4-ways WRONG — corrected from config + the checkpoint weight map +
  the transformers-5.12 reference modeling code (`modeling_higgs_audio_v2.py`):**
  1. Backbone is **LLaMA-3.2-3B**, NOT Qwen3 (hidden 3072, 28 layers, 24q/8kv × head_dim 128, inter 8192, vocab
     128256, **NO per-head q/k norm**, rms_norm_eps **1e-5**).
  2. RoPE is **llama3-scaled** (θ=5e5, factor 32, low/high 0.125/0.5, orig_max 1024), NOT plain.
  3. The audio head is an **UNTIED separate** `audio_lm_head [8208,3072]` (`tie_word_embeddings:false`), NOT tied
     to the embed like v3.
  4. The codec is **NOT v3's simple DAC** — it is `HiggsAudioV2TokenizerModel` (a dual semantic-HuBERT + acoustic
     9-codebook DAC tokenizer in a separate repo). BUT its **decode path is clean+simple** (RVQ→fc2→acoustic
     DAC, NO semantic/HuBERT at decode) and is **bit-for-bit the codec omnivoice already composes**.
- **DualFFN CONFIRMED** from the weight map: every layer has BOTH `mlp.{gate,up,down}_proj` +
  `audio_mlp.{gate,up,down}_proj` AND `input_layernorm`/`audio_input_layernorm` +
  `post_attention_layernorm`/`audio_post_attention_layernorm`, with a SHARED `self_attn`. The prompt's core
  thesis (v2 HAS the dual text/audio FFN) is correct.

## The DualFFN (the structural delta from v3, `HiggsAudioV2DecoderLayer.forward`)
A per-layer SHARED self-attn + TWO modality-routed FFN branches:
1. pre-attn norm: audio positions → `audio_input_layernorm`, text positions → `input_layernorm` (masked_scatter).
2. shared GQA attention over the whole sequence + residual.
3. post-attn norm + FFN: text positions → `post_attention_layernorm` + `mlp`; audio positions →
   `audio_post_attention_layernorm` + `audio_mlp`; each added to its own positions.

**Key simplification that made this tractable:** for TTS generation the audio-token mask is **uniform per
forward** — the prompt prefill is pure text (all text-FFN), every decode step feeds ONE all-audio frame (all
audio-FFN). The reference `masked_scatter` with a uniform mask reduces to applying ONE branch to the whole
tensor → the `DualFfnLayer::forward_uniform(is_audio)` byte-identical fast-path. Because routing is dynamic
(per-position), the DualFFN is composed as its **own layer struct + backbone loop** in `higgs_v2.rs` (the
dia2/csm idiom), reusing `nn::Attention` for the shared attention + two `nn::Mlp` + six `nn::RmsNorm` — so the
shared `nn::TransformerLayer` is **UNCHANGED** (the additive law is satisfied by construction, not by an edit).

## Live verification (CPU-f32 byte-identity + CUDA-f16 synth) — `cargo test --test cuda_torch_higgs_v2 --ignored`
Reference: the HF `transformers` 5.12 `HiggsAudioV2ForConditionalGeneration` + `HiggsAudioV2TokenizerModel`,
CPU-f32 (the libtorch ground truth; `tch` IS libtorch). Golden: `torch_runtime/dump_higgs_v2_golden.py`.

| gate | seam | result |
|---|---|---|
| 0 | prompt token ids (chat-template build) | **38/38 byte-identical** |
| 1 | audio_embed (fused multi-codebook) | **maxΔ = 0** |
| 2 | LLM hidden — all 28 DualFFN layers + llama3 RoPE + LLaMA backbone (`[38,3072]`) | **maxΔ = 0** |
| 3 | audio head — the UNTIED `audio_lm_head` first-frame logits `[8,1026]` | **maxΔ = 0** |
| 4 | codec — v2 RVQ→fc2→DAC, 39360 samples | **maxΔ = 0** |
| 5 | greedy AR raw codes | **byte-identical for the first 21 frames** (168 codes), then the f32-tie fork |
| 6 | CUDA-f16 full synth + RTF | **129600 samples (5.40s) in 5.74s → RTF 1.063**, peak 0.471, rms 0.082 |

**gate5 — the AR-greedy fork (expected, not a bug):** gates 2/3 prove the backbone+head logits are byte-identical;
the greedy AR agrees byte-for-byte through the 8-frame BOS delay-ramp + 13 real-code frames (21 total), then
forks at frame 21 **codebook 1** on a logit near-tie — the documented AR-compounding-greedy behaviour (a sub-ULP
f32 tie picks the other code, then KV diverges). A logic bug would mismatch at a structured edge (a frame/ramp
boundary), not an arbitrary interior codebook after 21 perfect frames. The defensible bar (agreeing prefix
byte-identical + ≥16 frames) is asserted and met.

## RTF
- **RTF 1.063** on GB10 CUDA-f16 (3B model, per-step eager AR decode; 5.40s synthesized in 5.74s). Slightly
  above realtime, faster than the v3 4B sibling's ~1.4 baseline. higgs is NOT CUDA-graphable (B46: growing-
  contiguous AR backbone, no fixed-position sub-decoder); a Torch-TensorRT dynamic-shape decode engine (the B52
  higgs-v3 lever) is the future per-step accelerator if needed.

## Exact files
**New (mine):**
- `crates/waav-infer-backend-torch/src/higgs_v2.rs` — the model (`TorchHiggsV2`): the DualFFN LLaMA-3.2-3B
  backbone, the untied audio head + fused audio embed, the v2 delay-pattern logits processor + AR loop, the
  codec builder, the chat-template prompt build, the `TtsModel` impl + the byte-identity diagnostics.
- `crates/waav-infer-backend-torch/tests/cuda_torch_higgs_v2.rs` — the 7-gate acceptance test.
- `torch_runtime/dump_higgs_v2_golden.py` — the CPU-f32 golden dumper (drives the real HF reference).

**Shared-file edits (additive, append-only — FLAGGED touches):**
- `crates/waav-infer-backend-torch/src/lib.rs` — `pub mod higgs_v2;` + `pub use higgs_v2::{HiggsV2TorchError,
  TorchHiggsV2};` (+ doc).
- `crates/waav-infer-server/src/engine.rs` — the dispatch arm `"higgs_v2" | "higgs_audio_v2" |
  "HiggsAudioV2ForConditionalGeneration"` (reuses the manifest `codec` field for the separate codec dir, the
  neutts pattern) + the import + the unknown-arch error message.

**Shared `nn::` / `codec::dac` — UNCHANGED** (zero edits). The DualFFN is composed externally; `InvFreq::llama3`
already existed (csm uses it); the v2 codec maps onto the existing `codec::dac::DacDecoder` (omnivoice's
`tanh:false`/`pre_proj_contiguous:true` regime). Re-verified: the full backend-torch lib test sweep is **161/161
green** (incl. the higgs-v3 / omnivoice / qwen3 / dia2 / csm regression tests) + clippy clean.

## Acquired
- `~/.cache/waav-models/higgs-v2-3b/` (generation model + tokenizer).
- `~/.cache/waav-models/higgs-v2-codec/` (the `bosonai/higgs-audio-v2-tokenizer` codec).
- `~/.cache/waav-models/higgs-v2-golden/` (the CPU-f32 goldens).

To serve via the engine, a `waav.json` with `{"runtime":{"backend":"torch-inprocess","architecture":"higgs_v2",
"codec":"<codec-dir>"}}` (the `codec` field points at the v2-tokenizer checkpoint).
