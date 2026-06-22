# B38 — Granite-Speech-4.1-2b on the in-process tch-rs backend (BYTE-IDENTICAL)

**Model:** `ibm-granite/granite-speech-4.1-2b` (`GraniteSpeechForConditionalGeneration`) — an LLM-decoder ASR.
**Result:** ✅ **BYTE-IDENTICAL to the Python torch sidecar** (CUDA bf16). 100% char-identity on the kokoro clip;
**every internal stage** (mel → conformer encoder → Q-Former projector → 40-layer Granite-LLM hidden) is
**maxΔ = 0.0** vs the sidecar. RTF **0.148** (12.05 s clip, ~1.78 s infer).

## Byte-identical transcript?
YES — exact `==` to the sidecar golden `transcript_bf16.txt`:
> `Hello world. This is W of V. Infer a portable voice inference engine running live on the GB10 Grace B L. A C K W E L L.`

Stage-by-stage byte-identity (Rust CUDA bf16 vs sidecar CUDA bf16, dumped goldens):

| stage | maxΔ | byte-identical |
|---|---|---|
| mel (f32, vs torchaudio CUDA feature-extractor) | 0.0 | ✅ |
| conformer encoder out `[1,603,1024]` | 0.0 | ✅ |
| Q-Former projector audio features `[1,123,2048]` | 0.0 | ✅ |
| Granite-LLM hidden (40-layer, pre-lm_head) `[1,142,2048]` | 0.0 | ✅ |
| step0 greedy argmax | match (9906) | ✅ |

(The raw step0 *logit values* differ ~bf16-vs-f32 because the final lm_head + argmax run in **f32** by design —
the voxtral/ark greedy-decision hardening — but the argmax and the full transcript are byte-identical.)

## Architecture (config.json)
- **Conformer-CTC encoder** (16 blocks, hidden 1024, 8 heads × 128, ctx 200, max_pos 512, conv k15): per block
  `½·FF1 + x → Shaw-rel-pos block-attn + x → conv-module + x → ½·FF2 + x → post-LN`, with a **mid-layer CTC
  injection** at layer 8 (`h += out_mid(softmax(out(h)))`). NOT a Whisper conv-stem → the shared
  `asr::AudioEncoder` does NOT fit; the conformer is model glue COMPOSING `nn` primitives.
- **BLIP-2 Q-Former projector** (2 BERT layers, 16 heads × 64, cross-attn every layer): windows the 603 encoder
  frames into 41 blocks of 15, runs 3 learned queries → `41×3 = 123` audio tokens → `linear(1024→2048)`.
- **Granite LLM decoder** (40 layers, hidden 2048, GQA 16/4 × 128, RoPE θ=10000, RMS eps 1e-5) with IBM-Granite's
  scalar **multipliers**: `embedding_multiplier=12.0`, `attention_multiplier=0.0078125` (the SDPA scale, NOT
  1/√128), `residual_multiplier=0.22`, `logits_scaling=8.0`.
- `has_lora_adapter=false` for THIS checkpoint → **no speech LoRA to merge** (the speech path is the base LLM;
  the prompt's mention of a LoRA does not apply to 4.1-2b — confirmed from config + the merge is a no-op).
- audio_token_id `100352`; EOS `100257`; tied lm_head (== embed_tokens in value); dtype **bf16**.

## Shared components COMPOSED vs new/extended
**Composed from `crate::nn` (reused, no rewrite):**
- `Backbone` (the 40-layer Granite stack + final norm + AR decode loop)
- `TransformerLayer` (pre-norm decoder layer)
- `Attention` (`Proj::Separate`, `Kernel::FusedCausalGqa`-style fused SDPA, `CacheRead::Contiguous`)
- `Mlp::swiglu_separate` (Granite's separate gate/up SiLU MLP)
- `RmsNorm` (`Square::Pow`, weight-first — Granite's RMSNorm)
- `Rope` (`InvFreq::f32_tensor_arange` θ=10000, **f32 tables**, rotate-half `apply_start`)
- `KvCache` (device-resident ring, contiguous read)
- `Linear` (`at_linear` = fused `F.linear`) — for **every** linear (encoder + projector + LLM)
- `LayerNorm` (the new **fused** variant) — conformer + Q-Former norms

**Shared-lib EXTENSIONS (config-driven, all other models re-verified byte-identical):**
1. `nn::TransformerLayer::with_residual_mul(f64)` (`layer.rs`) — scales each sub-block output by `m` before its
   residual add (`x = x + out·m`). Granite's `residual_multiplier=0.22`. Default `None` ⇒ ×1.0, byte-identical
   to the old `x + out` (unit-tested + the 7 other models unchanged). +1 unit test.
2. `nn::LayerNorm::fused()` + `LayerNormKind` (`layer_norm.rs`, re-exported in `mod.rs`) — the FUSED libtorch
   `Tensor::layer_norm` (== `nn.LayerNorm`), needed for bf16 byte-identity (the decomposed f32-reduce form
   differs 1 bf16 ULP). cohere/ark keep `Decomposed` (their byte-identity), unchanged. +1 unit test.

**New model glue in `granite.rs`** (the conformer + Q-Former + the torch-stft HTK-mel frontend — not in the
shared lib): `ConformerEncoder/Block/Ff/Attn/Conv`, `Projector/QFormerLayer/QFormerAttn`, `mel_filterbank` +
`granite_log_mel`. Each composes `nn::{Linear, LayerNorm, Mlp, sdpa}` primitives.

## The 8-bug playbook — per-class checks (the path to byte-identity)
The transcript matched on the first run, but per the standing 100%-correctness directive I drove **every stage to
maxΔ=0.0**, root-causing 7 real divergences (each a playbook scar, none "explained away"):

1. **Granite MULTIPLIERS** (the headline scar) — applied EXACTLY where `modeling_granite.py` does:
   `inputs_embeds ×= 12.0` AFTER the audio scatter; the attention softmax **scale IS 0.0078125** (not 1/√128,
   set as `Attention.scale`); `residual + sub_block·0.22` inside each layer (the new `with_residual_mul`);
   `logits /= 8.0` after the lm_head. Verified: `llm_embed` byte-identical, `llm_l0` byte-identical,
   `llm_hidden` byte-identical.
2. **FUSED ops, not hand-decompositions** (#1) — caught FOUR:
   - **Linear** → `at_linear` (fused `F.linear`/addmm), NOT manual `matmul+bias` (up_proj diverged maxΔ 0.125).
   - **LayerNorm** → the fused `Tensor::layer_norm`, NOT decomposed f32-reduce (1 bf16 ULP/elem).
   - **GLU** → `Tensor::glu(1)` (== `nn.GLU`), NOT `a*sigmoid(b)` (maxΔ 0.0625).
   - **Conformer attention** → the fused `nn::sdpa` with the Shaw bias as the attn_mask (→ MATH backend, the
     reference's `F.sdpa(attn_mask=pos_attn)`), NOT a hand softmax (which softmaxes f32 and diverged).
   - **Granite-LLM attention** → fused-causal SDPA `is_causal=is_prefill` (the reference's `_attn_implementation
     ="sdpa"` + `create_causal_mask=None` → the **flash** kernel; both a manual-GQA-f32-softmax AND a
     MATH-with-explicit-mask diverged — only flash-causal is bit-identical).
3. **bf16 vs f16** — the checkpoint + sidecar run **bf16** on CUDA; the port runs bf16 (not Half). tch==libtorch
   ⇒ bf16-CUDA is byte-identical to the sidecar's bf16-CUDA (the gate's reference). f32 on CPU.
4. **RoPE inv_freq** — `1/θ^(arange(0,d,2,i64).f32/d)` (`InvFreq::f32_tensor_arange`) **computed on the target
   device**; HF builds cos/sin in **f32** then casts at apply → `table_dtype=Float`.
5. **TF32 / cuDNN algo** — the conformer's 1×1/depthwise convs diverged because I had cuDNN **benchmark ON** (it
   autotunes a DIFFERENT algorithm whose bf16 accumulation rounds differently). The sidecar runs torch's default
   **benchmark=OFF**; matching it (removing `cudnn_set_benchmark(true)`) made the convs bit-identical. RTF-neutral
   (the conv tower runs once/clip).
6. **conv-pad** — depthwise k15 pad `(7,7)` (`pad=k//2`, `pad_offset=(k+1)%2`) replicated exactly; BatchNorm in
   eval (running stats, fused `Tensor::batch_norm`).
7. **greedy argmax first-max in f32** — the final lm_head projection + argmax run in **f32** with a first-max
   tie-break (`x > bv`), so an f16/bf16 near-tie can't re-flip the pick (EOS 100257).
8. **tokenizer** (#3) — the sidecar's slow `GPT2Tokenizer` splits `.\n` → `[. , \n]`, while the shipped
   `tokenizer.json`'s newer pre-tok regex MERGES it (a 1-token drift). Built the tokenizer from
   `vocab.json`+`merges.txt` with the **classic GPT-2 ByteLevel** pre-tokenizer (`use_regex=true`,
   `add_prefix_space=false`) + the `<|audio|>` added token — verified to reproduce the 142-token prompt exactly.
9. **mel frontend bit-identity** — `torch.stft` (manual reflect-pad `n_fft/2` + non-center stft = `center=True`,
   verified maxΔ=0.0) + an **HTK** filterbank (`melscale_fbanks` HTK/norm=None) **built on CPU then moved to the
   device** (CUDA `linspace`/`pow` round 1 ULP off CPU → 2.4e-5 mel diff) + the fused `Tensor::hann_window`
   **on CPU then moved** (a host-cos build was 1 ULP off). Result: the f32 mel is **maxΔ=0.0** vs the torchaudio
   CUDA feature-extractor (the exact mel the sidecar feeds).
10. **Q-Former batch broadcast** — the reference runs layer-0 self-attn at **query-batch 1**, then the cross-attn
    **broadcasts** the batch-1 query against the `nb` windows (`[1,…]@[nb,…]`), growing the batch to `nb`.
    Pre-expanding the query to `nb` first changed the bf16 batched-cuBLAS path and perturbed the tail; matching
    the reference's broadcast (and division-scale `/√d`, not `*reciprocal`) made the projector byte-identical.

## RTF
**0.148** on the 12.05 s kokoro clip (~1.78 s infer) — comfortably < 1. The once-per-clip conformer+Q-Former and
the f32 final matmul are cheap relative to the ~33-step 40-layer decode (the f32 mel-fb/window and benchmark-off
conv are RTF-negligible — they run once/clip).

## Exact files changed
- **NEW** `crates/waav-infer-backend-torch/src/granite.rs` (1143 lines) — `TorchGranite: SttModel`.
- **NEW** `crates/waav-infer-backend-torch/tests/cuda_torch_granite.rs` — the `#[ignore]` live byte-identity gate
  (+ an `#[ignore]` stage-dump diagnostic).
- `crates/waav-infer-backend-torch/src/lib.rs` — `pub mod granite;` + re-export `{GraniteTorchError, TorchGranite}`.
- `crates/waav-infer-backend-torch/src/nn/layer.rs` — `TransformerLayer::with_residual_mul` + field + test.
- `crates/waav-infer-backend-torch/src/nn/layer_norm.rs` — `LayerNorm::fused` + `LayerNormKind` + test.
- `crates/waav-infer-backend-torch/src/nn/mod.rs` — re-export `LayerNormKind`.
- `ci/heavy_live_tests.sh` — enrolled the granite gate (entry g2).

## Verification (all green)
- `cargo test -p waav-infer-backend-torch --lib` → **110 passed, 0 failed** (incl. 5 new granite + 2 new nn tests).
- `cargo clippy -p waav-infer-backend-torch --all-targets --features cuda -- -D warnings` → **clean**.
- Live GPU gate `cuda_torch_granite_byte_identical` → **100% char-identity, RTF 0.148**.
- **Regression** (the shared-lib changes): `cuda_torch_ark` 100%, `cuda_torch_cohere_vs_ort` byte-identical,
  `cuda_torch_voxtral_vs_ort` strict-clip 100% — the other models stay byte-identical.

## Golden
Persisted at `~/.cache/waav-models/granite-speech-golden/` (and `/tmp/granite_golden/`): `dump_golden.py`,
`pcm16.f32` (the exact 16 kHz EdgeResampler PCM, shared with the ark gate), `transcript_bf16.txt`,
`gen_ids_bf16.npy`. Regenerate the CUDA-bf16 golden with:
`source gb10-env.sh && WAAV_INFER_ROOT=$(pwd) HF_HUB_OFFLINE=1 python3 /tmp/granite_golden/dump_golden.py cuda bf16`.
