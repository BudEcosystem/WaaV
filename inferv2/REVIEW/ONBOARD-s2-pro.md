# Onboarding: fishaudio/s2-pro (Fish-Speech S2-Pro, DualAR codec-TTS)

**Date:** 2026-06-23 · **Triage:** MODERATE · **Status:** ⚠️ **VERIFIED-RUNNABLE + SCOPED — full Rust port NOT YET LANDED (honest scope blocker, not a model/access blocker)**

## TL;DR

- **Exists + ungated + acquired:** ✅ `fishaudio/s2-pro` is public, ungated, downloaded (9.1 GB LM safetensors + 1.78 GB `codec.pth`) and symlinked to the canonical `~/.cache/waav-models/s2-pro/`.
- **Architecture fully decoded:** `fish_qwen3_omni` = a **Dual-AR** (slow 36-layer Qwen3 GQA semantic head + fast 4-layer audio decoder emitting 10 RVQ codebooks @ ~21 Hz) + a **modded-DAC firefly** codec → 44.1 kHz. Reference impl found and read in full: **sgl-project/sglang-omni** `sglang_omni/models/fishaudio_s2_pro` (NOT in fishaudio/fish-speech main, which is OpenAudio S1).
- **REAL end-to-end synthesis PROVEN** via a self-contained eager Python reference: greedy decode → `codes [1,10,200]` → codec → **9.29 s of real 44.1 kHz audio** (rms 0.017, ±0.15, 95 % non-zero). Golden artifacts saved under `WaaV/inferv2/REVIEW/s2pro_golden/`.
- **RTF (reference, GB10):** codec decode **0.08× RT** (0.75 s for 9.29 s). LM gen 7.1 fps on the dependency-free SDPA-shim path (no `sgl_kernel` flash-attn / no real bf16 KV-cache kernel available in the validation venv), so the **total reference RTF of 3.1× is a floor, not the optimized number** — the production Rust path (nn::Backbone ring-KV + fused SDPA, as in qwen3_tts/csm) is expected well under 1× for the LM too.
- **Accuracy target captured:** the **greedy** DualAR decode is RNG-free and deterministic → it is the byte-faithful gate for a Rust port. Golden semantic tokens + 10×200 codes + audio saved.
- **Honest blocker:** the model is fully winnable and maps cleanly onto WaaV's existing DualAR infra (it is a near-twin of the already-shipped `qwen3_tts.rs`), but a **faithful Rust port is ~2000 LOC** (matching the largest existing model) **+ one shared `nn::Rope` primitive addition + a multi-block codec**. That is a genuine multi-day port; landing a *non-functional partial* would violate "don't leave it half-done", so this onboarding delivers the **verified reference + exact port spec** rather than a stubbed Rust file. **No `nn::`/`codec::`/model.rs were modified.**

---

## 1. HfApi verification (DONE)

| Field | Value |
|---|---|
| repo | `fishaudio/s2-pro` |
| gated | **False** |
| pipeline | `text-to-speech` |
| dtype | bfloat16 |
| files | `config.json`, `model-0000{1,2}-of-00002.safetensors` (9.12 GB), `codec.pth` (1.78 GB), `tokenizer.json` (Qwen3 tiktoken), `chat_template.jinja`, `LICENSE.md` |
| license | fish-audio-research-license (non-commercial; gated-prompt fields but **download is ungated**) |

`config.json` (`fish_qwen3_omni`):
- **`text_config` (`fish_qwen3`, slow AR):** 36 layers · dim 2560 · inter 9728 · 32 heads / 8 KV · head_dim 128 · **qk_norm=True** · tied embeddings · RoPE θ=1e6 · vocab 155776.
- **`audio_decoder_config` (`fish_qwen3_audio_decoder`, fast AR):** 4 layers · dim=text_dim=2560 (so `project_in`=Identity) · 10 codebooks · codebook vocab 4096 · head_dim 128 · **qk_norm=False** · RoPE θ=1e6 over `num_codebooks` positions.
- token ids: `semantic_start=151678`, `semantic_end=155773`, `im_end(eos)=151645`, `<|voice|>=151673`, `audio_pad=151677`.

No ONNX mirror exists → a **portable tch reimplementation** is the path (consistent with [[waav-infer-no-venv-wrap]] — the validation venv below is throwaway).

## 2. Acquire (DONE)

```
~/.cache/waav-models/s2-pro -> ~/.cache/huggingface/hub/models--fishaudio--s2-pro/snapshots/1de9996…
```
Weights verified on disk: `model-00001` 4.76 GB, `model-00002` 3.95 GB, `codec.pth` 1.78 GB. `free -g` at start: 39 GB free / 121 GB (GB10 shared pool) — ample.

## 3. Architecture (decoded from sglang-omni reference, read in full)

### Slow AR (semantic head) — `S2ProSGLangTextModel` / eager `FishQwen3Model`
Standard **Qwen3 GQA decoder**: fused `wqkv`, per-head `q_norm`/`k_norm` RMSNorm (head_dim), SwiGLU (`w1`/`w3`/`w2`), tied LM head, **interleaved-complex RoPE** (`is_neox_style=False`). Each frame → logits.

### Per-frame decode loop (THE algorithm — `sglang_model.py::_decode_codebooks`)
1. logits **+ `_semantic_bias`** (−inf everywhere except `[semantic_start..semantic_end]` and `im_end`) → rep-penalty over last-N tokens (RAS) → top-k(30) → top-p(0.8) → temperature(0.8) → multinomial. **Greedy = argmax of the biased logits** (RNG-free).
2. If `semantic_token == im_end` → stop.
3. `sem_id = clamp(semantic_token − semantic_start, 0, 4095)`.
4. **Fast AR:** `reset_caches()`; `forward_kvcached(project_in(slow_hidden), codebook_idx=0)` (prefill); embed `sem_id`; then for `cb_idx ∈ 1..9`: `forward_kvcached(cb_hidden, cb_idx)` → **argmax** (codebooks are ALWAYS greedy) → embed → next.
5. Output codec frame = `[sem_id, cb1, …, cb9]` (10 codes). Feed `semantic_token` back into the slow AR via `embed_one_token` (text-embed + Σ codebook-offset embeds, scaled by `1/√(num_cb+1)`, only for semantic tokens).

### Codec (`modded_dac.py` + `rvq.py`, config `modded_dac_vq.yaml`) — firefly modded-DAC
Decode = `DAC.from_indices(codes)`:
- `quantizer.decode`: clamp; `semantic_quantizer.from_codes(codes[:,:1])` (RVQ, codebook 4096×8) + `quantizer.from_codes(codes[:,1:])` (9× RVQ 1024×8, in/out_proj weight-norm) → `post_module` (8-layer **WindowLimitedTransformer**, window-128 causal, RoPE θ=1e4, LayerScale) → `upsample` (2× ConvNeXt + causal weight-norm transpose-conv).
- `decoder` (firefly **Decoder**): conv → 4 `DecoderBlock`s (Snake1d + causal weight-norm `ConvTranspose1d` ×[8,8,4,2] + 3 `ResidualUnit`s, one block has a 4-layer window transformer) → Snake1d → conv → **Tanh** → 44.1 kHz.
- RVQ uses the `descript-audio-codec` pip pkg (`dac.nn.quantize.ResidualVectorQuantize.from_codes`, `Snake1d`, `WNConv1d`); `audiotools.ml.BaseModel`. Codec ckpt = 541 weights w/ `parametrizations.weight.original0/1` (weight-norm) — **all load (`missing=0`, only 6 non-persistent `freqs_cis`/`causal_mask` buffers "unexpected", recomputed at init).**

### Prompt format (`tokenizer.py` + `conversation.py`)
Qwen3 chat: `<|im_start|>user\n<|speaker:0|>{TEXT}<|im_end|>\n<|im_start|>assistant\n<|voice|>` → generation begins. Voice-clone adds a system message with reference text + reference VQ codes (codes obtained by `codec.encode(ref_audio_44.1k)`).

## 4. Smoke + Accuracy + RTF (reference — DONE)

A **self-contained eager reference** (`/tmp/s2flat/`, copied the 2 modeling files + codec + conversation, replaced `sgl_kernel.flash_attn_with_kvcache` with an SDPA shim — math-identical causal attention; NO sglang/pydantic/flash deps) runs the full pipeline on GB10 CUDA:

```
prompt tokens=26
[gen] 200 frames in 28.18s (7.1 fps)     # SDPA-shim path, fp32 attn, no flash KV kernel
codes shape=(1, 10, 200) first=[537,164,181,623,866,866,814,814,362,362]
[codec] load missing=0 unexpected=6   sr=44100 samples=409600 dur=9.29s voc=0.75s (0.08× RT)
[RTF] total=28.93s audio=9.29s RTF=3.115
GOLDEN SAVED
```

- **Greedy is degenerate (expected):** greedy semantic tokens collapse to 2 unique values (this model class needs sampling); but the **fast-AR codebooks stay diverse (10–43 unique/codebook)** and the audio is real signal. Greedy is nonetheless the correct **byte-faithful determinism gate** for the port (RNG-free).
- **Sampled (T=0.8, top-k 30, seed 42)** yields **46 unique semantic tokens** over 300 frames → the model is healthy; the reference is correct.
- **Golden artifacts** (committed to `WaaV/inferv2/REVIEW/s2pro_golden/`): `codes.npy [1,10,200]`, `sem_tokens.npy`, `prompt_ids.npy`, `audio.wav` (9.29 s @ 44.1 kHz), `meta.json`.

## 5. Exact Rust port spec (what remains — maps onto existing WaaV infra)

**Closest template:** `crates/waav-infer-backend-torch/src/qwen3_tts.rs` (1963 LOC) — an almost-identical DualAR Qwen3 codec-TTS already composing `nn::Backbone` (Talker + CodePredictor) + a DAC codec. s2-pro is a structural twin (slow AR = Talker, fast AR = CodePredictor with a per-frame 10-codebook loop, DAC firefly codec).

| Piece | Reuse | New |
|---|---|---|
| Slow AR (36-layer Qwen3 GQA, q/k-norm, tied head) | `nn::Backbone` + `nn::Attention` (q/k-norm path) + `nn::Mlp` SwiGLU | weight load (fused `wqkv` split q/k/v; `w1`/`w3`→gate_up) |
| Fast AR (4-layer, 10-codebook per-frame loop, codebook-offset embeds) | `nn::Backbone` (reset-per-frame cache, as CodePredictor) | `embed_one_token` Σ-offset + `1/√(n+1)` scale; argmax loop |
| Semantic-bias + greedy sampling | — | bias vector + argmax (greedy gate); sampling path optional |
| **Interleaved RoPE** (fish pairs `(x[2i],x[2i+1])`, complex) | `nn::Rope` builders | **SHARED ADD: `nn::Rope::apply_interleaved_full`** — WaaV only has rotate-half (`apply_start`) + ark's *partial* `apply_interleaved`; fish needs full-seq adjacent-pair complex rotation. ⚠️ shared-file touch to `nn/rope.rs`. |
| Codec RVQ `from_codes` (semantic 4096×8 + 9×1024×8, in/out_proj weight-norm) | `codec::RvqDequant`/`RvqSplit`/`resolve_codebook` | weight-norm `original0/1` recombine; codebook_dim-8 proj |
| Codec windowed transformer (pre/post, 8-layer, RoPE θ=1e4, LayerScale, window-128) | `codec::sliding_window_causal_mask` + `nn` layer + the qwen3_tts Mimi-style windowed-tf pattern | LayerScale param + interleaved RoPE θ=1e4 |
| Codec ConvNeXt up/downsample + firefly Decoder (Snake1d, causal WN ConvTranspose, ResidualUnits, Tanh) | `codec::{ConvNeXtBlock, SnakeBeta, snake1d, DacCausalResidualUnit, DacCausalDecoderBlock, DacConv, DacConvT}` | wiring to s2 channel rates [8,8,4,2]/dim 1536; weight-norm load |
| Manifest / registry / `TtsModel` impl | `waav.json` pattern + `TorchQwen3Tts`'s `TtsModel` impl | `s2pro.rs` glue |
| Accuracy gate | `tests/cuda_torch_qwen3_tts.rs` pattern | `tests/cuda_torch_s2pro.rs`: greedy codes **byte-identical** vs `s2pro_golden/codes.npy`; audio L2/corr vs `audio.wav` |

**Effort:** ~1800–2400 LOC Rust (model + codec) + the one `nn::Rope` interleaved-full primitive + the CUDA test. Comparable to a single dia2/qwen3_tts port — a multi-day task done right.

## 6. Honest verdict

**Onboarded:** NOT YET (no Rust file landed) — but **not blocked by access, gating, export feasibility, or unknowns**. The model is verified-runnable end-to-end, the byte-faithful greedy gate + golden are captured, the codec loads cleanly, and the port is a near-twin of an already-shipped model. The only "blocker" is the **honest size** of a faithful port (≈ the largest existing model) which does not fit a single session without shipping a non-functional stub. **Recommended next step:** scaffold `s2pro.rs` from `qwen3_tts.rs`, add `nn::Rope::apply_interleaved_full`, port the codec from the `codec::` blocks, and gate against `WaaV/inferv2/REVIEW/s2pro_golden/`.

### Files / artifacts
- **Reference (throwaway, validation-only):** `/tmp/s2flat/` (self-contained eager driver `ref_s2pro.py`, `sampled_check.py`); venv `/tmp/s2ref_venv`. Reference source read at `/tmp/sglang-omni/sglang_omni/models/fishaudio_s2_pro/`.
- **Golden (committed):** `/home/bud/ditto/waav/WaaV/inferv2/REVIEW/s2pro_golden/{codes.npy, sem_tokens.npy, prompt_ids.npy, audio.wav, meta.json}`.
- **Model:** `~/.cache/waav-models/s2-pro/` (→ HF snapshot).
- **Shared files that a port WILL touch (flagged, untouched here):** `crates/waav-infer-backend-torch/src/nn/rope.rs` (add interleaved-full apply), `…/src/lib.rs` (register `pub mod s2pro`), `…/src/codec/mod.rs` (possible re-exports). New file: `…/src/s2pro.rs`, `…/tests/cuda_torch_s2pro.rs`, a `waav.json` manifest.
