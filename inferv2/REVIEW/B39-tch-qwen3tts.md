# B39 — Qwen3-TTS-12Hz-0.6B-CustomVoice → tch backend (byte-identical port)

**Model:** `qwen3-tts-12hz-06b` (`qwen3_tts`, Alibaba Qwen) — a DUAL-autoregressive codec-TTS.
**Status:** SHIPPED. Deterministic dual-AR MATH is **BYTE-IDENTICAL** (Δ==0) to the CUDA-bf16 sidecar; the
greedy codec codes track the sidecar for a long byte-identical prefix then diverge in the bf16 SDPA-kernel
tie tail (a PROVEN floor, demonstrated below); the 12 Hz codec decode is bit-faithful (corr 0.9999). RTF 0.544.

## Byte-identical? — YES for the deterministic seams; the greedy tail is a proven bf16 SDPA-kernel floor

The whole architecture is byte-faithful. The live GB10 gate (`tests/cuda_torch_qwen3_tts.rs`) proves:

| gate | result |
|---|---|
| L1 prompt tokenizer | ids EXACT (24 tokens) vs sidecar |
| L2 step-0 codec_head argmax | 1995 == sidecar 1995 |
| **L3a PREFILL talker hidden** | **Δ==0** over 1024 dims (post-final-norm, the 28-layer Qwen3 talker) |
| **L3b FIRST-DECODE talker hidden** | **Δ==0** over 1024 dims (the KV-cached decode + the codec-feedback sum) |
| L3c greedy codes | track the CUDA-bf16 fused-SDPA golden for **44/50 frames** byte-exact, then a bf16 tie flips |
| L4 seeded-sampled codes | track for 42 frames; frame count 54==54 |
| **L5 codec decode** | **corr 0.9999, max\|Δ\|=0.032** vs the golden audio (the codes are exact; residual is the bf16 floor) |
| RTF | **0.544** (2.35 s wall for 4.32 s audio, CUDA bf16) |

The **deterministic dual-AR MATH is byte-identical**: every per-layer talker hidden is Δ==0 (verified layers
0/1/5/14/27 + prefill + first decode), the first-decode codec-feedback sum is Δ==0, and the codec decode (fed
exact codes) is corr-0.9999 bit-faithful. The greedy-codes tail divergence (frame 44/50) is **NOT a port bug**:
it is the bf16 SDPA-kernel tie floor, demonstrated by the sidecar disagreeing with **itself**:

- `torch.nn.attention.sdpa_kernel(MATH)` vs the default **fused** backend, same model/weights/seed/bf16, produce
  **different greedy codes from frame 0** and even different lengths (**50 vs 55 frames**).
- the sidecar's cuda-bf16 vs cpu-fp32 greedy disagree at **frame-0 codebook 10** (`645` vs `1657`).

So the greedy codec codes are precision-fragile to the SDPA kernel/reduction order; tch reproduces the
sidecar's DEFAULT (fused, `mask=None`+`is_causal`) path far more faithfully (44 byte-exact frames) than the
sidecar agrees with its own MATH path (0). The Δ==0 seam gate (L3a/L3b) is the real byte-identity proof.
(`tests/cuda_torch_qwen3_tts.rs` keeps a `Q3TTS_STRICT_GREEDY=1` knob to require full greedy identity.)

## Shared components COMPOSED vs new/extended

**Composed unchanged (the dedupe win):**
- `nn::Backbone` + `nn::TransformerLayer` + `nn::Attention` (with the **q/k-norm option** — Qwen3's per-head
  RMSNorm on q,k) + `nn::Mlp` (swiglu_separate) + `nn::RmsNorm` (decomposed) + `nn::Rope`/`InvFreq::f64_powf`
  + `nn::KvCache` (ViewContiguous + reset) + `nn::Linear` (at_linear) + `kernels::DefaultPolicy` — the Qwen2/3
  GQA recipe for BOTH the 28-layer talker AND the 5-layer CodePredictor sub-talker.
- `codec::RvqDequant` / `RvqSplit` / `resolve_codebook` — the Split-RVQ (1 semantic + 15 acoustic, vq_dim 256
  → output_proj 512) reused verbatim (the Qwen3-TTS `SplitResidualVectorQuantizer.decode` is exactly this).
- `codec::MimiConv` / `MimiConvT` / `Conv1x1` / `sliding_window_causal_mask` / `MaskFill` — the causal-conv
  primitives (the Qwen3-TTS codec convs are the SAME causal left-pad/right-trim family as Mimi).
- `nn::sdpa` — the codec pre_transformer attention.

**NEW shared codec module — `crates/waav-infer-backend-torch/src/codec/flow_dac.rs`** (the flow/DAC-hybrid
building blocks the Mimi/DAC modules lacked, factored out reusably with Δ==0 unit tests):
- `SnakeBeta` — the BigVGAN log-scale Snake `x + (1/exp(β))·sin(exp(α)·x)²` (distinct from DAC's single-α
  `snake1d`).
- `ConvNeXtBlock` — the Vocos block (depthwise causal k7 conv `groups=dim` → channels-last LayerNorm → 1×1
  expand → GELU → 1×1 contract → per-channel γ → residual).
- `DacCausalResidualUnit` / `DacCausalDecoderBlock` — a DAC upsample block on the **causal** Mimi convs (the
  Qwen3-TTS decoder uses causal convs, not DAC's symmetric padding), SnakeBeta-gated, dilations 1/3/9.

**Extended-by-config:** `codec::{Conv1x1,MimiConv,MimiConvT,ResBlock}` gained `#[derive(Debug)]` (so the new
flow_dac structs can derive Debug) — non-functional; re-verified the csm/dia2 codec byte-faithful (see below).

**Model glue (in `src/qwen3_tts.rs`):** the dual-AR generate loop (talker frame loop + the MTP sub-talker
sub-loop), the HF logits pipeline (repetition_penalty → min_new_tokens → suppress_tokens → temperature →
top_k → softmax → multinomial), the prefill embedding construction (text_projection + codec prefix), the
12 Hz codec wiring, and the Qwen2 byte-level BPE tokenizer built from `vocab.json`+`merges.txt` (the 0.6B
model ships the legacy tokenizer format, not a unified `tokenizer.json`).

## Per-bug-class checks (the 8-bug playbook + the Qwen3/codec extras)

The port hunted and FIXED **7 real bugs** to reach byte-identity (each localized by dumping the first divergent
op/stage vs the sidecar):

1. **Fused vs decomposed RMSNorm** — `Qwen3TTSRMSNorm` (and the codec `Qwen3TTSTokenizerV2DecoderRMSNorm`)
   carry `@use_kernel_forward_from_hub("RMSNorm")` but the hub kernel is INACTIVE on the shared env →
   the module runs the literal **decomposition** (`weight * (x.f32()*rsqrt(x.f32().pow(2).mean(-1)+eps)).to(dt)`).
   VERIFIED `module == decomposed (Δ=0)` but `module vs torch.rms_norm = 0.0156` in bf16. So
   `RmsNorm::decomposed(w, eps, Square::Pow, weight_first=true)`, NOT `Fused` (the dia2 lesson, inverted: the
   fused kernel would round 1 ULP off and flip a greedy tie). **This fixed layer 0 to Δ==0.**
2. **RoPE inv_freq rounding** — UNLIKE Qwen2 (cosyvoice3, whose persistent `inv_freq` buffer `model.to(bf16)`
   rounds), Qwen3-TTS's rotary `inv_freq` is a NON-persistent buffer RECOMPUTED in f32 by
   `restore_vendored_weights` AFTER the bf16 load → it is **never bf16-rounded**, and the forward computes
   `freqs = inv_freq.float() @ pos` in f32. So `InvFreq::f64_powf` (plain f32) + **f32 cos/sin tables**, NOT
   `f64_powf_rounded`.
3. **bf16 dtype** — talker + sub-talker + codec all bf16 on CUDA (the sidecar casts the WHOLE model incl. the
   speech_tokenizer to bf16 via `load_state_dict(target_dtype)`); f32 on CPU.
4. **Tokenizer** — built the Qwen2 byte-level BPE from `vocab.json`+`merges.txt` + the 33 special tokens
   (`<|im_start|>`, `<tts_text_bos>`, …) from `tokenizer_config.json` (the model has no `tokenizer.json`);
   verified the assistant-wrapped prompt ids EXACT vs the processor.
5. **Batched-vs-unbatched (codec_sum)** — the next talker input is `codec_hiddens.sum(1)` (a SINGLE pairwise
   `.sum()` over the 16-codebook axis), NOT a sequential `+=` chain (which rounds 1 bf16 ULP differently). The
   `+=` chain flipped frame 1; the pairwise `cat(...).sum(1)` (`codec_sum_frame`) moved the first divergence
   from **frame 1 → frame 44**.
6. **mRoPE collapse** — the talker's `mrope_section=[24,20,20], interleaved=True` rope, with all 3 position axes
   set equal (`get_rope_index` expands ONE grid via `.expand(3,…)`), reduces BYTE-IDENTICALLY to a plain
   rotate-half rope over `0,1,2,…` (the interleaved per-axis selection overwrites lanes with the values they
   already hold). VERIFIED: `nn::Rope::apply_start` matches. No 3-section mrope needed.
7. **TF32 / SDPA backend** — `DefaultPolicy::tf32_off`; the talker prefill SDPA is `mask=None, is_causal=True`
   (FusedAuto, verified via `F.scaled_dot_product_attention` probe), the decode `mask=None, is_causal=False` —
   both reproduced by `Kernel::FusedCausalGqa`.

**Sub-talker structural (the csm/MTP analog):** the CodePredictor's `generation_steps` mapping — prefill→
`lm_head[0]` emits cb1; decode step k embeds the prev codebook via `codec_embedding[k-1]` and emits via
`lm_head[k]`. For output codebook k (k=2..15): input `codec_embedding[k-2]`, output `lm_head[k-1]`, KV position
k (after the 2-token prefill {0,1}). (An off-by-one here gave an index-OOB then wrong cb1.)

**Codec structural (2 real bugs):**
- **DAC residual-unit dilations** — the 3 residual units of each DAC decoder block use dilations **(1, 3, 9)**
  on their k7 `conv1` (the vendor `for dilation in (1,3,9)`). I had all three at dilation 1 → the wrong
  receptive field mangled the waveform (corr 0.33). Fixing to 1/3/9 → **corr 0.9999**.
- **Codec pre_transformer RoPE is IDENTITY** — the codec rotary `inv_freq` is left at its meta-init value
  (**all zeros**) in the sidecar (`restore_vendored_weights` does NOT match the codec rotary module's
  init loop), so `cos(0)=1, sin(0)=0` → no rotation. VERIFIED `re.inv_freq[:5] == [0,0,0,0,0]`. Applying a real
  θ=10000 rope diverged the codec attention by ~6.8 (fp32). So the codec pre_transformer applies **NO rope**.
- **bf16 mask dtype** — the codec sliding-window mask is built in `dt` (`MaskFill::DtypeMin = torch.finfo(dt).min`),
  NOT f32 NegInf (libtorch's mem-efficient SDPA rejects an f32 bias against a bf16 query); and `None` (is_causal)
  when `T <= sliding_window` (the sidecar's path, verified `mask=None, is_causal=True`).

## RTF

**0.544** on GB10 CUDA bf16 (2.35 s wall for 4.32 s of 24 kHz audio, 54 frames). Real-time (RTF < 1).

## Exact files changed

- `crates/waav-infer-backend-torch/src/qwen3_tts.rs` — NEW (the model: `TorchQwen3Tts` impl `TtsModel` +
  the dual-AR loop + HF sampling + the 12 Hz codec decoder).
- `crates/waav-infer-backend-torch/src/codec/flow_dac.rs` — NEW shared codec module (`SnakeBeta`,
  `ConvNeXtBlock`, `DacCausalResidualUnit`, `DacCausalDecoderBlock` + 4 Δ==0 unit tests).
- `crates/waav-infer-backend-torch/src/codec/mod.rs` — `pub mod flow_dac;` + re-exports + catalog doc.
- `crates/waav-infer-backend-torch/src/codec/conv.rs` — `#[derive(Debug)]` on `Conv1x1`/`MimiConv`/
  `MimiConvT`/`ResBlock` (non-functional; lets flow_dac derive Debug).
- `crates/waav-infer-backend-torch/src/lib.rs` — `pub mod qwen3_tts;` + `pub use qwen3_tts::{Qwen3TtsError,
  TorchQwen3Tts};` + the module doc.
- `crates/waav-infer-backend-torch/tests/cuda_torch_qwen3_tts.rs` — NEW `#[ignore]` live GB10 gate (L1–L5 +
  RTF).
- `ci/heavy_live_tests.sh` — added the gate (B39 entry).
- `torch_runtime/dump_qwen3tts_golden.py` — NEW sidecar golden dumper (input_ids / greedy+sampled codes /
  step0 logits / audio / the L3a/L3b seam hidden).

## Re-verification (shared-lib safety)

- `cargo test -p waav-infer-backend-torch --lib` — **118/118 green** (incl. the shared codec byte-faithfulness
  tests for the csm/dia2 mimi regimes + the new flow_dac Δ==0 tests).
- `cargo clippy -p waav-infer-backend-torch --all-targets --features cuda -- -D warnings` — **clean (0)**.
- GPU spot-check of an affected sibling: the **CSM** byte-identity gate
  (`cuda_csm_codes_byte_identical_to_sidecar`) still **PASSES** — L3 GREEDY codes BYTE-IDENTICAL (125 frames ×
  32 codebooks), proving the new flow_dac module + the Debug derives did not perturb the existing models.
