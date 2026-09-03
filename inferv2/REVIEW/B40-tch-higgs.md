# B40 — tch port: higgs-tts (Boson higgs-audio-v3-tts-4b)

**Status: BYTE-IDENTICAL (deterministic seams + greedy AR codes, CPU f32 maxΔ=0.0). Live CUDA-f16 sampled
synthesis renders intelligible non-silent speech. RTF 1.45 (CUDA f16, 4B model).**

Port of `higgs_tts` (Boson AI Higgs-Audio, arch `higgs_multimodal_qwen3`, an AR codec-TTS) from the Python
sidecar onto the in-process tch-rs backend, COMPOSING the shared library.

---

## 1. What it is (and the DualFFN finding)

`HiggsMultimodalQwen3ForConditionalGeneration` (config.json) is a **PLAIN Qwen3-4B** GQA decoder that emits an
**8-codebook delay-patterned** audio stream, decoded by a **DAC-family** neural codec to 24 kHz (25 fps, 960
samples/frame).

- **LLM backbone** (`body.layers.{0..35}`): 36-layer Qwen3 — 32 q-heads / 8 kv-heads × head_dim 128, hidden
  2560, SwiGLU MLP (gate/up/down, inter 9728), **per-head q/k RMSNorm**, RMSNorm eps 1e-6, RoPE θ=1e6.
- **audio embed** (`tied.embedding.modality_embeddings.0.embedding.weight` `[8208,2560]` = 8 cb × 1026 vocab):
  fused multi-codebook embed `sum_c E[code_c + c·1026]` (the `audio_tokens_offsets` gather+sum).
- **audio head** (TIED to the embed): `hidden @ Eᵀ → [.,8208] → [.,8,1026]`.
- **codec** (`modality_embeddings.0.model`): RVQ (8× codebook `[1024,64]` → `project_out` Linear 64→1024,
  summed) → **`fc2`** Linear 1024→256 → DAC `acoustic_decoder` (conv1 k7 → 5 blocks `[snake,convT,3 res_units]`
  strides `[8,5,4,2,3]` → snake → conv2 k7 → tanh).

**DualFFN: NOT PRESENT in v3.** The task flagged a dual text/audio "DualFFN" MLP (the higgs-audio **v1/v2**
feature). The **v3** checkpoint unified it away — verified from the weight map: every layer has exactly ONE
`mlp.{gate,up,down}_proj` and there are **no** audio-FFN / expert / router / dual tensors. The audio path is the
SEPARATE tied embed/head + codec, NOT a per-layer FFN branch. So `nn::TransformerLayer` needed **no** audio-vs-
text FFN routing extension — the plain shared Qwen3 layer is exact. (transformers 5.12 ships `higgs_audio_v2`
modeling code, which DOES have the dual FFN — confirming v3's simplification.)

## 2. The reference / golden (honest framing)

- The registered sidecar (`torch_runtime/models/higgs_tts.py`) drives the **onnx-community ONNX export** via
  ONNX Runtime. On this box **python onnxruntime is CPU-only** (no aarch64 GPU wheel) and the export graphs are
  **fp16** — so a "CUDA-bf16 ORT sidecar golden" is not runnable here, and the fp16 ONNX export is a *derived*
  artifact (the bf16 model is ground truth, per [[waav-infer-100-percent-correctness]]).
- Therefore the byte-identity reference is a **from-scratch PyTorch reimplementation of the ORIGINAL Boson bf16
  checkpoint** (`torch_runtime/dump_higgs_llm_golden.py`) — the libtorch ground truth. **`tch` IS libtorch**, so
  identical math ⇒ identical bytes (the LAW). Goldens dumped at `~/.cache/waav-models/higgs-golden`.
- Regime: **CPU f32** is the byte-identity regime (deterministic seams + greedy). The codec/embed/head ONNX
  sub-graphs were ALSO cross-checked against the fp16 ONNX export (`dump_higgs_golden.py`): codec corr 0.99993
  (the f32-vs-fp16 precision delta), confirming the dataflow.

## 3. Byte-identity result (CPU f32, gate `cpu_f32_byte_identical_to_reference`)

| gate | seam | maxΔ / result |
|---|---|---|
| 2 | audio_embed (fused multi-cb gather+sum) | **0.0** |
| 1 | codec (RVQ → fc2 → DAC decoder, 38400 samples) | **0.0** |
| 4 | LLM hidden `[18,2560]` (all 36 Qwen3 layers) | **0.0** |
| 3 | first-frame audio-head logits `[8,1026]` (backbone + tied head) | **0.0** |
| 5 | **greedy raw delayed codes `[33,8]` — THE LAW** | **byte-identical** (0/264 differ, frames 33==33) |

Greedy is RNG-free ⇒ byte-identical hidden ⇒ byte-identical argmax ⇒ byte-identical codes AND stop frame. **No
sampled-tail floor was reached** — the greedy path is exactly byte-identical end-to-end (no bf16-tie to demonstrate).

## 4. The per-bug-class checks (the 8-bug playbook)

1. **fused vs decomposed RMSNorm** → **`nn::RmsNorm::Fused`** (`Tensor::rms_norm`). The reference uses
   `F.rms_norm` and the ONNX export `SimplifiedLayerNormalization` (both fused, f32-internal). This is the dia2
   regime, NOT the qwen3_tts `Decomposed` regime (whose vendored module ran a literal decomposition because its
   hub kernel was inactive — here the reference is the fused op). Used for the layer norms AND the q/k norm.
2. **bf16 vs f16** → CPU f32 (byte-identity) / CUDA f16 (manifest `cuda_fp16`); per-regime, weights cast at load.
3. **tokenizer** → the Qwen `tokenizer.json` via the `tokenizers` crate; special ids `<|tts|>=151667`/
   `<|text|>=151672`/`<|audio|>=151670` verified == the sidecar `get_added_vocab`; body ids verified equal.
4. **RoPE inv_freq rounding** → **`nn::InvFreq::f32_tensor_arange`** (HF `compute_default_rope_parameters`:
   `1/(θ^(arange(0,d,2,int64).float()/d))`, f32 tensor op), NOT `f64_powf`. **This was the one real bug found:**
   `f64_powf` drifted the LLM-hidden by **8e-5** (compounding over 36 layers); switching to `f32_tensor_arange`
   made it byte-identical (Δ=0). θ=1e6, f32 cos/sin tables, `apply_start`.
5. **TF32** → the f32 audio-head projection runs under the global libtorch TF32 context on CUDA (the dia ABI
   setters), matching PyTorch's Ampere+ default.
6. **RNG draw count/order** → the GATE is greedy (deterministic). Production sampling (temperature 0.8, top-k 50)
   draws per-codebook (rows 0..8) via the libtorch MT19937 (seeded), in the sidecar's `_sample` op order.
7. **conv pad** → DAC convs are SYMMETRIC-padded (`codec::dac`); the codec `conv_t` uses **`output_padding =
   stride % 2`** for the odd strides 5/3 (the ONNX-attested geometry — without it the codec dropped 25 samples,
   38375 vs the correct 38400).
8. **batched-vs-unbatched** → higgs has NO CFG (single batch row) ⇒ no batched-CFG TF32 hazard.

Extra: the **codec runs in f32** even when the LLM is f16 — the DAC Snake (`sin(αx)²`) + deep conv stack
OVERFLOWS in f16 (→ NaN). The codec is a separate sub-graph (the ONNX export ships it as its own session), so
f32 is both NaN-safe AND the exact reference regime; negligible perf cost vs the 4B LLM.

Extra: the **RVQ output must be `.contiguous()`** (matching the shared `DacRvq.from_codes`): a non-contiguous
RVQ output dispatches a different libtorch conv kernel downstream (~1.8e-6 drift). The golden was regenerated to
the canonical contiguous op; then codec Δ=0.

## 5. Shared components — COMPOSED vs extended

**Composed verbatim (a model = config + glue):**
- `nn::Backbone` (36 Qwen3 layers + final norm + the AR decode loop driving the per-layer ring KvCaches).
- `nn::TransformerLayer` / `nn::Attention` (Separate q/k/v/o, `Native` prec, q/k-norm, `RopeApply::Start`,
  `CacheRead::ViewContiguous`, `Kernel::FusedCausalGqa`, scale `d^-0.5`) — the SAME composition qwen3_tts/granite
  use. **No DualFFN routing needed** (see §1).
- `nn::RmsNorm` (Fused), `nn::Rope` (f32_tensor_arange/f32, θ=1e6, apply_start), `nn::KvCache` (ring),
  `nn::Linear` (at_linear), `nn::Mlp` (swiglu_separate, Silu).
- `kernels::DefaultPolicy` (via `Attention::default_policy`).
- `codec::dac::{DacRvq, DacCodebook, DacDecoder, DacDecoderBlock, DacConv, DacConvT, DacResidualUnit, snake1d}`.

**Extended by config (default-off ⇒ Dia byte-identical; with unit tests):**
- `codec::dac::DacConvT.output_padding` — `nn.ConvTranspose1d(output_padding=…)` = `stride % 2` for odd-stride
  upsample. Default `0` = byte-identical to the prior no-output-padding `DacConvT` (Dia's even strides).
- `codec::dac::DacDecoder.pre_proj: Option<Linear>` — optional per-frame channel Linear between RVQ and conv1
  (higgs `fc2` 1024→256). `None` = Dia's path, unchanged.
- 2 new unit tests in `codec/dac.rs` (`dac_convt_output_padding_reaches_exact_upsample`,
  `dac_decoder_pre_proj_matches_explicit_channel_linear`); both assert the default is byte-identical to the old op.

**Genuinely-new model glue (higgs-specific, lives in `higgs.rs`):** the tied fused multi-codebook audio
embed/head (`AudioCodec`), the 8-codebook delay pattern + revert, the AR greedy/sampled decode loop, the TTS
prompt tokenization. No new shared component was needed beyond the two dac config knobs.

**Re-verification of affected models:** only **dia** consumes the changed `codec::dac` (qwen3_tts uses
`codec::flow_dac`, untouched; no `nn::TransformerLayer` change ⇒ voxtral/dia2/granite unaffected). dia re-verified
byte-identical (CPU-fp32 raw codes gate, lib unit tests green). All 122 backend-torch lib tests pass; clippy
`--all-targets --features cuda -D warnings` clean.

## 6. RTF

CUDA f16, sampled (temperature 0.8, top-k 50), prompt = "Hello, this is a test…": **76800 samples (3.20 s) in
4.63 s → RTF 1.446**; peak 0.4986, rms 0.0553 (intelligible non-silent speech). RTF is above realtime — it is a
4B-param AR codec-TTS doing a full 36-layer Qwen3 forward per 40-ms frame; the codec + f32 head are minor. Greedy
degenerates to silence by design (the sidecar `inference.py` documents pure-argmax cb0-sticking → no EOC);
sampling is the production path that renders speech.

## 7. Files changed (exactly)

- `crates/waav-infer-backend-torch/src/higgs.rs` — **NEW** (`TorchHiggs` impl `TtsModel`).
- `crates/waav-infer-backend-torch/src/lib.rs` — `pub mod higgs;` + `pub use higgs::{HiggsTorchError, TorchHiggs};`.
- `crates/waav-infer-backend-torch/src/codec/dac.rs` — `DacConvT.output_padding` + `DacDecoder.pre_proj` (config
  extensions) + 2 unit tests.
- `crates/waav-infer-backend-torch/src/dia.rs` — pass `output_padding: 0` + `pre_proj: None` (byte-identical).
- `crates/waav-infer-backend-torch/tests/cuda_torch_higgs.rs` — **NEW** (#[ignore] live gates 1–6).
- `ci/heavy_live_tests.sh` — the two higgs gates (B40).
- `WaaV/inferv2/REVIEW/COMPONENT_CATALOG.md` — the higgs (B40) entry + the B40 dac extensions.
- `torch_runtime/dump_higgs_llm_golden.py`, `torch_runtime/dump_higgs_golden.py` — **NEW** golden dumpers
  (reference + ONNX cross-check). Goldens persisted at `~/.cache/waav-models/higgs-golden`.
