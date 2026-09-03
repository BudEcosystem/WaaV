# B42 — tch port of **omnivoice** (masked-diffusion-LM TTS) — BYTE-IDENTICAL

The LAST Wave-3 fan-out model. k2-fsa **OmniVoice** (`omnivoice`): a masked-diffusion-LM TTS — a BIDIRECTIONAL
**Qwen3-0.6B** backbone emits 8 codebooks of audio tokens via MaskGIT/LLaDA iterative confidence-ranked
**parallel unmasking** (32 steps, CFG + per-codebook penalty + Gumbel), decoded by the
`HiggsAudioV2TokenizerModel` DAC codec → 24 kHz. Ported in-process onto the tch-rs backend, COMPOSING the
shared library.

## Byte-identical? **YES** — the model's real outputs are byte-for-byte (maxΔ = 0.0)

CUDA-f32 acceptance gate (`cuda_torch_omnivoice::cuda_f32_byte_identical_to_reference`), 6 sub-gates on one load:

| gate | seam | maxΔ | bar |
|---|---|---|---|
| 1 | prompt ids (tokenizer + style/text template ×8 codebooks + MASKed target) | **0** (exact) | byte-identical |
| 2 | **codec** decode of fixed codes (RVQ → fc2 → DAC acoustic, NO tanh) | **0.0** | byte-identical |
| 3 | bidirectional Qwen3 `last_hidden_state` (all 28 layers, post-final-norm) | 6.10e-5 | ≤ proven floor |
| 4 | CFG-combined step-0 log-probs `[8,T,1025]` | 2.00e-4 (finite) | ≤ proven floor |
| 5 | **masked-diffusion emitted codes `[8,36]` (THE LAW)** | **0/288 differ** | byte-identical |
| 6 | **full synthesis wav (34560 samples)** | **0.0** | byte-identical |

The two byte-identity targets that matter — the emitted **codes** (gate 5) and the final **waveform** (gate 6)
— are byte-for-byte identical to the CUDA-f32 sidecar golden. RTF **~1.39** (CUDA f32, 36-frame utterance).

### gate 3/4 are a PROVEN numerical floor (the sidecar disagrees with itself), demonstrated not hand-waved
The omnivoice Qwen3 backbone is **ill-conditioned**: its deep layers reach activations ~10^4 with a
near-cancellation (layer-27 absmax ~9760 → layer-28 ~354, a ~27× collapse), which amplifies the unavoidable f32
cuBLAS GEMM-reduction-order difference (~1e-7 relative/layer). **The sidecar disagrees with ITSELF**: Python's
own forward CPU-vs-CUDA (both full-fp32, identical weights+code) diverges **6.87e-4** post-final-norm at this
exact seam — and our tch-CUDA vs the CUDA golden is **6.1e-5**, an order of magnitude TIGHTER than the model's
intrinsic floor. On **CPU the LM hidden is exactly 0.0** (tch CPU == torch CPU bit-exact), confirming the LM
math is byte-faithful; the 6.1e-5 is purely CUDA cuBLAS associativity. (The gate asserts `d ≤ 6.87e-4`, the
measured CPU↔CUDA floor; a larger Δ would be a real defect.)

## Shared components COMPOSED vs new/extended

### Composed (reused unchanged)
- `nn::Backbone` / `nn::TransformerLayer` / `nn::Attention` / `nn::RmsNorm` / `nn::Rope` / `nn::Mlp` /
  `nn::Linear` — the Qwen3-0.6B backbone (28L, hidden 1024, 16 q / 8 kv heads × d128, SwiGLU-sep, RMSNorm
  `Decomposed{Pow, weight_first}`, RoPE `f32_tensor_arange` + `from_inv_freq_full` + `StartExact` seq-exact,
  `at_linear` everywhere, q/k-norm).
- `kernels::DefaultPolicy` (via `Attention::default_policy`).
- `codec::dac::{DacDecoder, DacRvq, DacCodebook, DacConv, DacConvT, DacResidualUnit, DacDecoderBlock, snake1d}`
  — the codec is a `HiggsAudioV2Tokenizer.decode` = RVQ (8× Euclidean codebook `[1024,64]` → `project_out`
  Linear 64→1024) → `fc2` (the `pre_proj`, 1024→256) → DAC acoustic_decoder (strides `[8,5,4,2,3]`,
  `output_padding=stride%2`), which is **exactly** the shared DAC decoder.

### NEW — the masked-diffusion sampler (added to the `cfm` CFG-denoise family, per the directive)
- **`cfm::masked::MaskedDiffusion`** + **`cfm::masked::MaskedLogits`** (`src/cfm/masked.rs`, +6 unit tests) — the
  MaskGIT/LLaDA **discrete** iterative confidence-ranked parallel-unmask recurrence, genuinely distinct from the
  two continuous-latent siblings (`cfm::ode` flow-Euler / `cfm::dpm` DPM-Solver++). It owns the schedule
  (time-warped, sums to C·T), the per-step log-space CFG-combine + argmax + per-codebook penalty + Gumbel +
  `topk` reveal, and the per-step RNG draw; the model supplies the CFG logit pair via `MaskedLogits::eval`
  (mirroring the `FlowField` seam). Registered as `StepperKind::MaskedConfidenceUnmask` in the `cfm`
  `DiffusionStepper` family + the module/catalog docs (discoverable).

### NEW — cache-free **bidirectional** transformer pass (config-driven shared-lib extension; all prior models re-verified)
omnivoice is bidirectional (full, non-causal attention, NO KV cache — the whole sequence is re-evaluated every
diffusion step). Added, composing the existing primitives:
- `nn::Attention::forward_full(xn, rope, pos, positions, mask)` — projects (Separate) → q/k-norm → RoPE →
  **eager** full-sequence attention (`repeat_kv` to n_q heads + `sdpa_manual`, matching HF
  `eager_attention_forward` bit-for-bit — NOT the GQA-fold, which reassociates the cuBLAS GEMM ~6e-5 on CUDA) →
  o-proj. (+2 unit tests: matches the explicit eager GQA op; is non-causal.)
- `nn::TransformerLayer::forward_bidirectional` + `nn::Backbone::forward_bidirectional` — the same pre-norm
  skeleton over the cache-free attention. These are NET-NEW methods (no existing model calls them ⇒ zero risk to
  voxtral/cohere/dia2/cosyvoice3/ark/csm/vibevoice/dia/higgs/granite/qwen3_tts/dots/neutts).

### EXTENDED `codec::dac::DacDecoder` (config-driven; **higgs + dia re-verified byte-identical**, lib green)
- `tanh: bool` — omnivoice's `HiggsAudioV2Tokenizer._adjust_dac_decoder` replaces the final `nn.Tanh` with
  `Identity` (omnivoice = `false`; Dia/higgs = `true`, byte-identical to the prior always-tanh decoder).
- `pre_proj_contiguous: bool` — omnivoice's reference RVQ output is a `permute(0,2,1)` VIEW, so its
  `.transpose(1,2)` un-does the permute to a CONTIGUOUS `[B,T,hidden]` that `F.linear` reduces row-major;
  our `from_codes` materializes a contiguous `[B,hidden,T]` whose `.transpose` is strided (a strided `F.linear`
  reduces in a different order ⇒ CPU-f32 maxΔ ~2.7e-5). omnivoice = `true`; **higgs = `false`** (it was
  byte-identical with the strided path — adding `.contiguous()` SHIFTED it ~1.8e-6, a regression caught + fixed
  by re-verification). The DAC residual `conv2` (k1) now reads its **bias** (omnivoice ships it; the higgs
  codec did not — dropping it was the first ~0.16 codec divergence). +1 dac unit test for the contiguity flag.

**Re-verification of the touched shared component (`codec::dac`):**
- **higgs** `cpu_f32_byte_identical_to_reference`: gate1 codec **0.0**, audio_embed 0.0, llm_hidden 0.0,
  first-frame logits 0.0, greedy codes byte-identical — PASS.
- **dia** `cuda_torch_dia`: DAC codec parity + channel-0 byte-identical over all 2601 frames — PASS.

## Per-bug-class checks (the 8-bug playbook + the 3 new scars found here)
1. **fused-vs-decomposed RMSNorm** — Qwen3 `Qwen3RMSNorm` = `Decomposed{Pow, weight_first}` (`pow(2)`,
   weight-left), eps 1e-6. Verified: qproj/qnorm values byte-identical.
2. **bf16-vs-f16** — N/A: omnivoice is **all-f32** (config `dtype:"float32"`, manifest fp32).
3. **tokenizer** — the model `tokenizer.json` (Qwen2 BPE + `<|lang_start|>`…); gate 1 prompt ids = 0.0.
4. **RoPE inv_freq + θ** — `compute_default_rope_parameters` (`f32_tensor_arange`) + FULL doubled tables +
   **seq-exact** `StartExact`. **THE BIG ONE: θ = 10000, NOT the config.json's 1e6** — the sidecar builds
   `Qwen3Config(**{rope_theta: 1e6})`, but modern transformers reads RoPE from a `rope_parameters` DICT and
   IGNORES the top-level `rope_theta` kwarg → falls back to the default `{rope_theta:10000}`. The sidecar's
   ACTUAL forward uses θ=10000 (verified by hooking `apply_rotary_pos_emb`); the `omnivoice.py` runner has the
   identical quirk, so θ=10000 is the byte-identity target. (θ=1e6 gave a 112.2 hidden divergence.)
5. **TF32** — pinned **full-fp32 on CUDA** (cuBLAS matmul OFF + cuDNN conv OFF). The **codec runs on CPU**
   (conv1d is deterministic + tch==torch bit-exact there; a CUDA conv's cuDNN-algorithm choice drifts ~2.6e-6
   from PyTorch even at full fp32 — the codec is a one-shot at the end, RTF-negligible). The golden's codec is
   dumped on CPU too.
6. **RNG draw order (the masked-diffusion unmask + noise draws)** — `tch::manual_seed(0)` + **`Cuda::
   manual_seed_all(0)`** (the gumbel `rand_like` draws on the score's CUDA device — `tch::manual_seed` seeds
   only CPU). tch CUDA `rand` == torch CUDA `rand` (verified, maxΔ 0.0, both libtorch Philox). **THE SUBTLE
   ONE: the sidecar's `_gumbel_sample` is DEGENERATE** — `g = -log(-log(u.clamp_min(1e-20)).clamp_min(1e-20))`:
   Python binds `.clamp_min` to `log(u)` (≤0) BEFORE the unary minus, clamping the negative log UP to 1e-20, so
   the outer `-` makes `-1e-20` and `log(-1e-20) = NaN` → `g` is ALWAYS all-NaN, every step. The reveal
   `topk(score)` therefore sees an all-NaN score and selects positions by the platform's `topk(NaN)` ordering
   (the gumbel is dead). We **replicate this byte-for-byte** (the [[100-percent-correctness]] law: match the
   reference's exact ops, even a bug); tch's CUDA `topk(NaN)` matches the sidecar's CUDA kernel ⇒ codes
   byte-identical. (PROOF it's a real floor for the *sampled* output: the **sidecar disagrees with itself
   200/288 CPU-vs-CUDA** on the codes — `topk(NaN)` orders differently across devices in the reference too.)
7. **conv-pad** — the DAC acoustic decoder's symmetric conv pads + `output_padding=stride%2` (odd strides 5/3)
   + the residual `conv2` **bias** (omnivoice ships it). gate 2 codec = 0.0.
8. **batched-CFG** — cond + uncond are TWO batch-1 bidirectional forwards (NOT a batch-2 stack; the runner's
   `_logits_cfg2` discipline), written into both rows' target span each step.

## RTF
~1.39 (CUDA f32, 36-frame "Hello world." utterance: 1.44 s audio in ~2.00 s). The codec on CPU + the
full-fp32-no-TF32 LM trade some speed for exact byte-identity; the 32-step ×2-pass diffusion dominates.

## Exact files changed
**New:**
- `crates/waav-infer-backend-torch/src/omnivoice.rs` — `omnivoice::TorchOmnivoice` impl `TtsModel`.
- `crates/waav-infer-backend-torch/src/cfm/masked.rs` — `cfm::masked::{MaskedDiffusion, MaskedLogits}` (+6 tests).
- `crates/waav-infer-backend-torch/tests/cuda_torch_omnivoice.rs` — the `#[ignore]` live CUDA-f32 acceptance gate.
- `torch_runtime/dump_omnivoice_golden.py` — the seed-0 golden dumper (reproducibility tool).

**Modified:**
- `crates/waav-infer-backend-torch/src/lib.rs` — `pub mod omnivoice;` + `pub use omnivoice::{…}`.
- `crates/waav-infer-backend-torch/src/cfm/mod.rs` — `pub mod masked` + `StepperKind::MaskedConfidenceUnmask` +
  exports + family docs.
- `crates/waav-infer-backend-torch/src/codec/dac.rs` — `DacDecoder.{tanh, pre_proj_contiguous}` (+2 tests).
- `crates/waav-infer-backend-torch/src/nn/self_attention.rs` — `Attention::forward_full` (+2 tests).
- `crates/waav-infer-backend-torch/src/nn/layer.rs` — `TransformerLayer::forward_bidirectional`.
- `crates/waav-infer-backend-torch/src/nn/backbone.rs` — `Backbone::forward_bidirectional`.
- `crates/waav-infer-backend-torch/src/higgs.rs` — `DacDecoder` ctor: `tanh:true, pre_proj_contiguous:false`.
- `crates/waav-infer-backend-torch/src/dia.rs` — `DacDecoder` ctor: `tanh:true, pre_proj_contiguous:false`.
- `ci/heavy_live_tests.sh` — the omnivoice gate entry.

## Status
`cargo test -p waav-infer-backend-torch --lib` → **140 passed**. `cargo clippy --all-targets -D warnings`
(with + without `cuda`) → **clean**. The CUDA-f32 acceptance gate → **PASS** (codes 0/288, wav 0.0).
higgs + dia gates re-verified byte-identical. **Not done short of byte-identical** — the codes and the
waveform, the model's actual outputs, are byte-for-byte identical to the sidecar; gates 3/4 are a demonstrated
floor (the sidecar disagrees with itself by more).
