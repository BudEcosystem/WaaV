# WaaV Infer — Shared Component Library + Catalog (Torch backend)
The discoverable, reusable, kernel/backend-swappable component library. RULE (memory waav-infer-modularize-reuse):
before writing ANY primitive, find it here and REUSE it. A model = config + glue COMPOSING these, never a rewrite.

## Module tree (crates/waav-infer-backend-torch/src/)
```
nn/        the transformer primitives + the shared backbone (Phase 1 LIVE + Phase 2 LIVE, B31)
  # ── Phase 1: the 5/5-duplicated primitive ops ──
  rms_norm.rs       RmsNorm        — FUSED Tensor::rms_norm OR decomposed (Square::Mul/Pow, weight-order, opt weight);
                                      the byte-identical fix lives HERE once
  rope.rs           Rope + InvFreq — 5 inv_freq families (f64powf / rounded / min_max / host_exps / arange / llama3) ×
                                      table dtype × 3 apply geometries (start / positions / partial-interleaved)
  attention.rs      sdpa / sdpa_manual / sdpa_gqa_manual — the SDPA kernel primitives (flash-vs-math selected by args)
  kv_cache.rs       KvCache        — device-resident ring (pre-alloc + in-place index_copy_; view / contiguous / full-masked)
  linear.rs         Linear         — zero-copy [rows,in]@Wᵀ gemm (Matmul) / fused addmm (AtLinear == F.linear)
  layer_norm.rs     LayerNorm      — weight+bias, f32 population variance (cohere / ark-tower)
  # ── Phase 2 (B31): the shared transformer backbone — composes the Phase-1 primitives ──
  mlp.rs            Mlp            — gated SwiGLU (fused|separate gate/up) | ungated ReLU/GELU MLP; Act {Silu,Gelu,Relu}
  self_attention.rs Attention      — COMPOSED self-attn: proj (fused|separate) × prec (Native|F32Sandwich) × opt q/k-norm
                                      × rope_apply (Start|Positions|None) × cache_read (View|ViewContiguous|Contiguous|
                                      FullMasked) × kernel (ManualGqa|ManualMha|FusedCausalGqa|FusedCausalMaybeGqa|
                                      FusedMaskedGqa) × scale; + CrossAttention (cohere AED)
  layer.rs          TransformerLayer — pre-norm decoder layer = Norm(Rms|Layer) + Attention (+ opt CrossAttn insert) +
                                      Mlp|inline-Ffn + opt ada_scale; the 5/5-duplicated `Layer` struct, ONE impl
  backbone.rs       Backbone       — Vec<TransformerLayer> + final_norm + opt LmHead(tied|untied) + the AR decode loop
                                      (drives the per-layer KvCaches; the 5/5-duplicated `for layer in &self.layers{…}`)
  # ── planned (Phase 3+) ──
  embedding.rs  Embedding, scatter_into_placeholder (ark <|audio|> positions)
codec/
  rvq.rs        RvqDequant        — residual-VQ dequant (embed_sum / cluster_usage, per-codebook output_proj)
  mimi.rs       MimiDecoder       — Kyutai Mimi neural codec (upsample ConvT → transformer → SEANet → 24kHz); dia2, csm
cfm/
  ode.rs        CfmOde + FlowField trait — cosine-schedule CFG Euler solve; cosyvoice3 → vibevoice → omnivoice
  vocoder.rs    HiFT/NSF vocoder
asr/
  encoder.rs    AudioEncoder      — Whisper-style conv stem (PAD CONFIG: causal vs symmetric — the voxtral/ark distinction)
                                    + conformer/transformer; mel via waav_infer_components (already shared)
  adapter.rs    encoder→decoder adapter (cross-attn / scatter)
kernels/
  mod.rs        KernelBackend     — see below
```

## Components × current models (the dedupe map — what the extraction collapses)
| Component | voxtral | cohere | dia2 | cosyvoice3 | ark | (csm) | (vibevoice) |
|---|---|---|---|---|---|---|---|
| RmsNorm (fused) | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| Rope (variants) | ✓ | – | ✓ | ✓ | ✓×2 | ✓ | ✓ |
| KvCache (ring) | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| Attention (GQA sdpa) | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| Linear (zero-copy) | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| Mlp (B31) | SwiGLU-fused | ReLU-ungated | (inline f32) | SwiGLU-fused | SwiGLU-fused | SwiGLU-sep | SwiGLU-fused |
| Attention (B31, composed) | ManualGqa/View | ManualMha/View/+xattn | F32Sandwich+qk-norm/FullMasked | FusedCausalGqa/ViewContig | ManualGqa/View | FusedCausalMaybeGqa/Contig | FusedCausalGqa/Contig |
| TransformerLayer (B31) | +ada_scale | +CrossAttn,LayerNorm | inline-MLP | ✓ | ✓ | ✓ | ✓ |
| Backbone (B31, shared stack) | Mistral-26L | AED-8L | bbone-28L | Qwen2-24L | Qwen2-24L | Llama-16L+depth-4L | Qwen2.5-28L |
| MimiDecoder | – | – | ✓ | – | – | ✓ | – |
| CfmOde | – | – | – | ✓ | – | – | ✓ |
| AudioEncoder | ✓(causal) | ✓(conformer/ORT) | – | – | ✓(symmetric) | – | – |

**granite (B38, `granite.rs`)** — an 8th LLM-decoder ASR composing the shared lib: `Backbone` (Granite 40L) +
`TransformerLayer` (**+`with_residual_mul(0.22)`**, the new IBM-Granite residual scalar) + `Attention`
(Separate/FusedCausalGqa/Contiguous, scale = `attention_multiplier` 0.0078125) + `Mlp` (SwiGLU-sep) + `RmsNorm`
(Pow,wf) + `Rope` (arange/f32, θ=10000) + `KvCache` (ring/contig) + `Linear` (**at_linear** for every linear) +
`LayerNorm` (**the new `fused` variant** — conformer + Q-Former norms). Its Conformer-CTC encoder (Shaw-rel-pos
block-attn + GLU/depthwise conv module + mid-CTC) and BLIP-2 Q-Former projector and torch-stft HTK-mel frontend
are model glue (NOT the shared `AudioEncoder` — a conformer, not a Whisper stem). Byte-identical (maxΔ=0.0 every
stage) to the bf16-CUDA sidecar; mel built CPU-then-moved + cuDNN-benchmark-OFF + fused-causal SDPA were the scars.

### B38 shared-lib extensions (config-driven, all 7 prior models re-verified byte-identical)
- `nn::TransformerLayer::with_residual_mul(m)` — `x = x + sub_block·m` per sub-block (Granite 0.22; `None` ⇒ ×1.0,
  byte-identical to the old `x + out`).
- `nn::LayerNorm::fused()` + `LayerNormKind{Decomposed,Fused}` — the fused `Tensor::layer_norm` (granite bf16);
  cohere/ark keep `Decomposed` (their byte-identity), unchanged.

**higgs (B40, `higgs.rs`)** — Boson **higgs-audio-v3-tts-4b** (`higgs_multimodal_qwen3`), an AR codec-TTS
composing the shared lib: `Backbone` (Qwen3-4B **36L**) + `TransformerLayer` + `Attention`
(Separate/FusedCausalGqa/ViewContiguous, q/k-norm, scale `d^-0.5`) + `Mlp` (SwiGLU-sep) + `RmsNorm`
(**`Fused`** — the reference uses `F.rms_norm`/ORT `SimplifiedLayerNormalization`, the dia2 regime NOT the
qwen3_tts `Decomposed` one) + `Rope` (**`f32_tensor_arange`/f32**, θ=**1e6** — HF `compute_default_rope_parameters`;
the f64_powf path drifted the LLM-hidden 8e-5, this is the bug-#4 scar) + `KvCache` (ring/view-contig) + `Linear`
(at_linear) + **`codec::dac::DacDecoder`** for the codec. **v3 is a PLAIN Qwen3 — NO DualFFN/MoE** (the
v1/v2 dual text/audio FFN was unified away; verified from the weight map — one `mlp` per layer, no audio-FFN /
expert / router tensors). The fused multi-codebook audio embed/head (tied `[8208,2560]` = 8 cb × 1026, the
`audio_tokens_offsets` gather+sum / `hidden@Eᵀ`), the 8-codebook delay pattern, and the AR loop are model glue.
Byte-identical (maxΔ=0.0) to the PyTorch reference of the bf16 ground truth: audio_embed, codec, first-frame
audio-head logits, the 36-layer LLM hidden, AND the greedy raw codes (CPU f32).

### B40 shared-lib extensions (config-driven; dia re-verified byte-identical, all lib tests green)
- `codec::dac::DacConvT.output_padding` — `nn.ConvTranspose1d(output_padding=…)`, the extra output zeros DAC
  sets to `stride % 2` so ODD strides reach the exact `T·s` upsample (higgs strides `[8,5,4,2,3]`; `pad=⌈s/2⌉`
  alone drops one sample for s=5/3, the ONNX-attested geometry). Default `0` ⇒ byte-identical to the prior
  no-output-padding `DacConvT` (Dia's even strides `[8,8,4,2]`).
- `codec::dac::DacDecoder.pre_proj: Option<Linear>` — an OPTIONAL per-frame channel `Linear` between the RVQ
  sum and `conv1` (higgs `fc2` 1024→256). `None` ⇒ Dia's path (RVQ `out_proj` already lands in conv1), unchanged.

**neutts (B41, `neutts.rs`)** — Neuphonic **NeuTTS Air** (`neutts_air`), an on-device AR codec-TTS with instant
voice cloning composing the shared lib: a STOCK **Qwen2-0.5B** (`Qwen2ForCausalLM`, the SAME Qwen2-0.5B arch
`cosyvoice3` composes — 896/24L/14q-2kv-h64, SwiGLU-sep, RoPE θ=1e6, **sep q/k/v WITH bias**, `at_linear`,
**RmsNorm Decomp{Mul}**, FusedCausalGqa, ViewContiguous) via `nn::Backbone` + a tied/untied **`nn::LmHead`** over
a 217652 vocab whose tail is the `<|speech_N|>` NeuCodec FSQ codes (a contiguous 65536-id block). The codec is
the open **NeuCodec ONNX** decoder (Vocos + BS-Roformer + ISTFT + ResidualFSQ — genuinely new, NOT a `codec::`
member) run via **`waav_infer_backend_ort::OrtModel`** (the BLESSED `tch-backbone + ORT-codec` hybrid
`cosyvoice3`/`cohere` use — the sidecar decodes through ONNXRuntime, never torch → byte-identical codec by
construction; NO codec re-port, which would only ADD a cross-runtime delta). The prompt template, the speech
block + FSQ extraction, the faithful HF decode chain (**RepetitionPenalty 1.1 → TopK 50 → TopP 0.8 → softmax →
multinomial**; greedy = RepetitionPenalty → full-vocab argmax), and the misaki-G2P live path are model glue.
Byte-identical (maxΔ=0.0 every stage; greedy codes **96/96 exact** CPU-f32 AND CUDA-bf16) to the registered
sidecar. The 3 scars: FLASH SDPA must be enabled (tch defaults the CPU flash backend OFF → MATH → ~4e-4 hidden
drift); **seq-exact RoPE** (a precomputed-table slice rounds `cos` ~6e-8 per tensor-size and compounds ~4e-4);
the HF `repetition_penalty=1.1` (the `do_sample=False` golden applies the generation_config default). RTF ~0.77.

### B41 shared-lib extensions (config-driven; voxtral + csm GPU-re-verified byte-identical, all lib tests green)
- `nn::Rope::from_inv_freq_full(...)` + `Rope.full_tables` / `Rope.inv_freq` — the **HF-exact FULL doubled**
  cos/sin tables (`emb = cat([freqs,freqs]); cos = emb.cos()` — DOUBLE then `cos`, the modern-transformers
  `RotaryEmbedding.forward` spelling). The default half-table `from_inv_freq` (`freqs.cos()` then `cat([cos,cos])`
  at apply) is mathematically equal but rounds ~1.9e-6 apart in f32 (half-vs-full-tensor `cos` vectorization);
  bf16 rounds it away ⇒ the other models are byte-identical either way (the existing `from_inv_freq` is
  untouched; `rotate_half_apply` now delegates to a new `rotate_half_apply_doubled`, same ops).
- `nn::Rope::apply_start_exact` / `apply_positions_exact` (+ `nn::RopeApply::StartExact`) — the **seq-exact**
  per-forward RoPE recompute (build `cos` for the EXACT seq length from the retained `inv_freq`, like HF), the
  byte-identity spelling on the f32 path (slicing a `[max_pos,…]` table makes `cos` round ~6e-8 per tensor-size
  and compounds). Requires the `from_inv_freq_full` build. `apply_interleaved` debug-asserts the half-table build.

## Kernel / hardware-backend abstraction (kernels/mod.rs) — "easy to apply different kernels/HW, loaded appropriately"
```rust
pub enum AttnKernel { Math, MemEfficient, Flash, Cudnn, Fused }     // libtorch SDPA backends
pub trait KernelPolicy {
    /// pick the attention kernel for this (device, shape, dtype). e.g. GQA-on-CUDA-bf16 → Math (the byte-identical
    /// choice proven in B25); non-GQA large → Flash; CPU → the CPU path. Pinned via at::sdp backend flags.
    fn attn_kernel(&self, dev: &TorchDevice, q: &Shape, dtype: Kind, gqa: bool) -> AttnKernel;
    fn allow_tf32(&self, dev: &TorchDevice) -> bool;               // match float32_matmul_precision
}
```
- The policy is LOADED per device (a `DefaultPolicy` keyed off `DeviceCaps` from backend-api/B15) — not hardcoded
  in each model. Components (`Attention`) ask the policy, set the SDPA backend, run. Swapping a kernel = a policy
  change, zero model edits. Ties up to the accel layer (AccelMapper): kernels = the per-op tier under the per-model accel.
- Hardware backend = `TorchDevice` (cuda/cpu/metal); a component is device-agnostic (takes the device), the policy
  loads the right kernel per backend. Adding a HW backend = a policy branch, not a model rewrite.

## Extraction plan (after csm+vibevoice merge → all 7 present)
Phase 1 — `nn/` (the 5/5 dedupe, biggest win): extract RmsNorm/KvCache/Attention/Linear/Rope as the EXACT current
ops; rewire all 7 models to `nn::`. **DONE (B30).** Phase 2 — the shared TRANSFORMER BACKBONE: `nn::Mlp` +
`nn::Attention` (composed self-attn, config-selected kernel/cache/rope/prec/qk-norm) + `nn::CrossAttention` (cohere
AED) + `nn::TransformerLayer` (the 5/5-duplicated `Layer` struct, ONE impl; +ada_scale/+cross/+inline-MLP config) +
`nn::Backbone` (the AR decode loop + final norm + opt tied/untied lm_head). All 7 decoders rewired to it, the local
`Layer`/loop copies DELETED. **DONE (B31): 490 dup lines deleted from the models → 387 lines of declarative per-model
config; 4 GPU spot-checks byte-identical (voxtral 100% greedy, csm 4000/4000 greedy, dia2 608/608 sampled, cohere
100% AED/cross-attn) + ark/cosyvoice3/vibevoice byte-identical too; +13 nn unit tests; lib 77 green; clippy clean.**
Phase 3 — `codec`/`cfm`/`asr` (Mimi / CfmOde / AudioEncoder — the codec head, CFM, audio tower, depth-projector
glue still inline). Phase 4 — `kernels` policy + wire Attention through it.
LAW: every phase is BIT-FAITHFUL — the extracted op is byte-for-byte the same; re-run the byte-identical gates after
each phase; clippy clean; each shared component gets its OWN unit tests (extensively) + a doc-comment catalog entry.
Discovery: `nn/mod.rs` etc. re-export + list; this file is the top-level index.
