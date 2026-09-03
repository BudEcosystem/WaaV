# B41 — tch NeuTTS Air (`neutts_air`) port: BYTE-IDENTICAL, composing the shared library

**Status: DONE — byte-identical (CPU-f32 AND CUDA-bf16), shared-lib composed, other models re-verified.**

NeuTTS Air (Neuphonic, 0.5B, arch `neutts_air`) — an on-device AR codec-TTS with instant voice cloning —
ported from the Python torch sidecar (`torch_runtime/models/neutts_air.py`) onto the in-process tch-rs
backend. `neutts::TorchNeutts` impl `waav_infer_core::model::TtsModel` in
`crates/waav-infer-backend-torch/src/neutts.rs`.

## Byte-identical? YES.
Precision-matched to the registered sidecar's STOCK `Qwen2ForCausalLM` (tch IS libtorch ⇒ identical math ⇒
identical bytes). Goldens dumped by `torch_runtime/dump_neutts_golden.py` (the real sidecar runner: Qwen2 +
espeak G2P + NeuCodec ONNX + ref voice), persisted at `~/.cache/waav-models/neutts-golden`.

| gate | regime | result |
|---|---|---|
| 1 codec parity (NeuCodec ONNX decode of fixed ref codes) | CPU/ONNX | **maxΔ = 0.0** (30240 samples) |
| 2 LLM hidden (Qwen2 prefill, all 24 layers + final norm) | CPU f32 | **maxΔ = 0.0** ([598,896]) |
| 3 first-step speech logits (tied lm_head over the speech block) | CPU f32 | **maxΔ = 0.0** ([65536]) |
| 4 **greedy FSQ codes — THE LAW** (RepetitionPenalty 1.1 → full-vocab argmax) | CPU f32 | **0/96 differ (96/96 exact)** |
| 5 **greedy FSQ codes — THE LAW** (precision-matched) | **CUDA bf16** | **0/96 differ (96/96 exact)** |
| 6 production sampled synthesis + RTF | CUDA bf16 | 164 codes → 3.26s audio, **RTF 0.770**, non-silent (peak 0.485) |

No bf16-tie floor: the CUDA-bf16 greedy is exactly byte-identical (not "a long prefix then a tie") — the three
scars below were real bugs; once fixed there is no residual divergence in either regime.

## Shared components COMPOSED (vs new/extended)
A model = config + glue COMPOSING the shared lib — no reimplementation.

**Composed as-is (the IDENTICAL Qwen2-0.5B composition `cosyvoice3.rs` uses):**
- `nn::Backbone` (24-layer Qwen2 stack + final RmsNorm) + a tied/untied **`nn::LmHead`** (217652 vocab).
- `nn::TransformerLayer` / `nn::Attention` — `Proj::Separate { q,k,v }` **with bias** (Qwen2 attn-bias),
  `ProjPrec::Native`, `Kernel::FusedCausalGqa`, `CacheRead::ViewContiguous`, scale `d^-0.5`, 14q/2kv/h64.
- `nn::RmsNorm::decomposed(Square::Mul, weight_first=false)` (HF `Qwen2RMSNorm`), eps 1e-6.
- `nn::Linear::at_linear` (fused addmm) for every projection + the lm_head.
- `nn::Mlp::swiglu_separate` (gate/up/down, Silu), inter 4864.
- `nn::KvCache` (device-resident ring), `kernels::DefaultPolicy` (SDPA backend/TF32).
- **`waav_infer_backend_ort::OrtModel`** for the codec — the BLESSED `tch-backbone + ORT-codec` hybrid that
  `cosyvoice3` (CFM estimator) and the candle Cohere arm already use. The NeuCodec decoder (Vocos ConvNeXt/
  Resnet + 12 BS-Roformer blocks + ISTFT head + ResidualFSQ) is **genuinely new** (NOT a `codec::` member),
  and the sidecar decodes it **through ONNXRuntime, never torch** — so running the SAME `model.onnx` via
  `OrtModel` is byte-identical to the sidecar **by construction**. Re-porting it to tch would only ADD a
  cross-runtime ORT-vs-tch float delta (the opposite of byte-identity), so it is correctly NOT re-ported.

**Shared-lib EXTENSION (config-driven, additive — `nn::Rope`, the one new behavior):**
- `nn::Rope::from_inv_freq_full` + `Rope.full_tables`/`Rope.inv_freq` — the **HF-exact FULL doubled** cos/sin
  tables (`emb = cat([freqs,freqs]); cos = emb.cos()`).
- `nn::Rope::apply_start_exact` / `apply_positions_exact` + `nn::RopeApply::StartExact` — the **seq-exact**
  per-forward RoPE recompute (HF `RotaryEmbedding.forward`: build `cos` at the EXACT seq length).
- The existing `from_inv_freq` (half-table) path is UNTOUCHED; `rotate_half_apply` now delegates to a new
  `rotate_half_apply_doubled` (same ops). 3 new `nn::rope` unit tests added.

No new `codec::` component was added (the codec is the ONNX seam). NO model is forked.

## Per-bug-class checks (the 8-bug playbook, [[waav-infer-100-percent-correctness]])
1. **fused-vs-decomposed RMSNorm** — Qwen2 = `RmsNorm::decomposed{Mul}` (HF `Qwen2RMSNorm`), NOT the fused
   kernel (verified Δ=0). ✓
2. **bf16-vs-f16** — CUDA = **bf16** (config.json `torch_dtype`, the cosyvoice3 lesson), CPU = f32. ✓
3. **tokenizer** — the model `tokenizer.json` via `tokenizers`; the `<|speech_N|>` block start id (151671) +
   count (65536) derived from it and asserted contiguous. The byte-identity gates feed the sidecar's EXACT
   espeak prompt ids (dumped) so the AR-math proof is independent of the espeak-vs-misaki G2P. ✓
4. **RoPE inv_freq rounding (the recurring #1)** — `InvFreq::f32_tensor_arange` (HF
   `compute_default_rope_parameters`, the non-persistent f32 buffer), NOT `f64_powf_rounded`. **Plus** the new
   **seq-exact full-table RoPE** — THE dominant CPU-f32 scar: slicing a precomputed `[max_pos,…]` table makes
   the `cos` kernel round ~6e-8 differently per tensor-size and compounds the hidden ~4e-4; recomputing
   `cat([freqs,freqs]).cos()` at the EXACT seq length is byte-identical to HF (Δ=0). ✓
5. **TF32** — the global libtorch TF32 context enabled on CUDA (the f32 lm_head argmax projection), via the
   Itanium-ABI setters (the dia recipe). ✓
6. **RNG draw order** — the byte-identity gate is **greedy** (`do_sample=False` → argmax, RNG-free). The
   sidecar's `do_sample=False` ALSO applies the generation_config **`repetition_penalty=1.1`** (the 2nd scar):
   replicated faithfully (HF `RepetitionPenaltyLogitsProcessor` over all seen ids, `l<0?l*p:l/p`). The
   production sampled path replicates the full HF chain (RepetitionPenalty → TopK 50 → TopP 0.8 → softmax →
   multinomial, temperature 1.0). ✓
7. **conv-pad** — N/A for the backbone (no convs); the codec convs live INSIDE the shared NeuCodec ONNX graph
   (byte-identical to the sidecar's ONNX), so there is no conv-pad port to get wrong. ✓
8. **batched-vs-unbatched CFG** — neutts has NO CFG (single batch row); no batched-CFG TF32 hazard. ✓

### The 3rd scar (the FLASH SDPA backend)
tch's default libtorch context, on **CPU**, has the fused `cpu_flash_attention` backend OFF, so the mask-free
causal `scaled_dot_product_attention(is_causal=true)` falls to the **MATH** kernel — which rounds ~9.5e-7
differently than the reference's FLASH backend and compounds the hidden ~4e-4. Fix: enable flash + mem-efficient
on the global libtorch context at load (additive — math left enabled, so the explicit-mask models dia2/cohere
keep MATH; mirrors the sidecar's `install_process_sdpa_pin`). Verified: golden == FLASH == default Δ=0, MATH Δ≠0.

## RTF
CUDA bf16, the production sampled path: 164 FSQ codes → 78240 samples (3.26 s) in 2.51 s → **RTF 0.770**
(real-time capable on GB10), non-silent (peak 0.485, rms 0.067).

## Shared-component change re-verification (the modularize-reuse law)
The `nn::Rope` extension is additive (existing `from_inv_freq` half-table path byte-for-byte unchanged; the
`rotate_half_apply` refactor delegates to identical ops). Re-verified:
- **lib**: 127/127 `cargo test -p waav-infer-backend-torch --lib` green (incl. 3 new `nn::rope` tests +
  `apply_start_matches_manual_rotate_half` Δ=0 confirming the refactor).
- **GPU spot-check, `apply_start` path (rope-affected)**: **voxtral** `cuda_torch_voxtral_vs_ort` — 100%
  char-identity on the English clip (the 2nd clip's 82.4% is the documented bf16-vs-q4 soft bar, unchanged).
- **GPU spot-check, `apply_positions` path (rope-affected)**: **csm**
  `cuda_csm_codes_byte_identical_to_sidecar` — L3 LAW: greedy CUDA-bf16 codes BYTE-IDENTICAL (125 frames × 32
  codebooks). (csm covers the bf16 regime where the half-table 1.9e-6 rounds away → byte-identical either way.)
- clippy `--all-targets -D warnings`: clean.

## Exact files changed
- `crates/waav-infer-backend-torch/src/neutts.rs` — **NEW**: `TorchNeutts` (the model, the deliverable).
- `crates/waav-infer-backend-torch/src/lib.rs` — `pub mod neutts;` + `pub use neutts::{NeuttsTorchError, TorchNeutts};`.
- `crates/waav-infer-backend-torch/tests/cuda_torch_neutts.rs` — **NEW**: 3 `#[ignore]` live gates.
- `ci/heavy_live_tests.sh` — the 3 neutts gates enrolled (B41 block).
- `crates/waav-infer-backend-torch/src/nn/rope.rs` — shared-lib EXTENSION: `from_inv_freq_full` +
  `full_tables`/`inv_freq` fields + `apply_start_exact`/`apply_positions_exact` + `rotate_half_apply_doubled`
  + 3 unit tests.
- `crates/waav-infer-backend-torch/src/nn/self_attention.rs` — `RopeApply::StartExact` variant + its arm.
- `torch_runtime/dump_neutts_golden.py` — **NEW**: the golden dumper (the per-model test artifact, the sibling
  of `dump_higgs_golden.py`).
- `WaaV/inferv2/REVIEW/COMPONENT_CATALOG.md` — the B41 model + shared-lib-extension catalog entries (discovery).

## Notes
- The live `synthesize` path uses misaki-rs G2P (no espeak/GPL); espeak is NOT a numeric port concern and the
  byte-identity gates feed the sidecar's exact espeak prompt ids (`prompt_ids.npy`), so the AR-math proof is
  G2P-independent. The default-voice reference codes (`default_voice.pt`, a torch pickle tch can't read) are
  loaded from a portable `default_voice.npy` (dumped once) for the live path; load is OPTIONAL (the gates
  never need it).
