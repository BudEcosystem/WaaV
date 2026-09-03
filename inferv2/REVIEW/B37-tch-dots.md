# B37 — dots.tts ported to the in-process tch-rs backend (BYTE-IDENTICAL, 3 variants)

**Status: DONE — byte-identical to the CUDA-bf16 sidecar for ALL THREE weight variants.**

`dots::TorchDots` (`crates/waav-infer-backend-torch/src/dots.rs`) ports **rednote-hilab dots.tts**
(arch `dots_tts`) from the Python torch sidecar (`torch_runtime/.../dots_tts.py` + `vendor/dots_tts/*`)
onto the in-process libtorch (tch-rs) backend. ONE arch serves THREE weights, config-selected from
`config.json`:

| variant | mode | latents byte-identical | payload patches |
|---|---|---|---|
| **dots-tts-base** | flow_matching (CFG Euler ODE, w=1.2) | **10240/10240 exact (max\|Δ\|=0)** | 20 |
| **dots-tts-soar** | flow_matching | **9728/9728 exact (max\|Δ\|=0)** | 19 |
| **dots-tts-mf** | **meanflow** (duration-embedded, no CFG) | **10240/10240 exact (max\|Δ\|=0)** | 20 |

The model is NOT a discrete codec-TTS — it is a **fully-continuous** AR TTS:
Qwen2-1.5B AR backbone → causal semantic **PatchEncoder** (AR feedback) → **DiT** flow-matching head
(per-patch ODE) → 48 kHz **BigVGAN AudioVAE** vocoder. The voice is a fixed precomputed reference
(`default_voice.pt`: CAM++ x-vector + prompt VAE-latent distribution + transcript), injected so serving
runs neither the speaker encoder nor the VAE encoder (mirrors the sidecar runner exactly).

## Byte-identical? YES — which variants tested
All three (base, soar, mf) are gated against their own CUDA-bf16 sidecar golden under `manual_seed(0)`:
the generated **payload latent patches** (`[1, P*4, 128]`, denormalized — the deterministic AR+FM seam
*before* the vocoder) are **byte-for-byte identical**, every value (max\|Δ\| = 0.0). The full audio's
50 Hz energy envelope correlates **1.0000** with the golden; the sample-level residual is **~1.5e-3**,
which is the **f32 BigVGAN vocoder's cross-engine BLAS reduction-order floor** (the latents that drive
the vocoder are byte-identical; the vocoder runs in f32 under the sidecar's own `@autocast(enabled=False)`
seam, and tch-vs-torch f32 conv/BLAS reduction orders differ sub-ULP — the same documented vocoder floor
as cosyvoice3/dia2). This is the deterministic-seam parity, not hand-waved: the latent seam is exact, and
the residual is localized to the f32 vocoder convs.

## Shared components COMPOSED vs new/extended
**Composed the shared library (no shared component changed, no shared component extended):**
- The **Qwen2-1.5B AR backbone** is fully the shared `nn::Backbone` (28 × `nn::TransformerLayer`) using the
  byte-identity-proven cosyvoice3 Qwen2 recipe: `nn::RmsNorm::decomposed(Mul)`, `nn::Linear::at_linear`
  (fused addmm, biased q/k/v), `nn::Rope` w/ `nn::InvFreq::f64_powf_rounded` (θ=1e6), `RopeApply::Start`,
  `CacheRead::ViewContiguous`, `Kernel::FusedCausalGqa`, `nn::Mlp::swiglu_fused`, `nn::KvCache`.
- The **DiT** and **PatchEncoder** reuse `nn::Linear` (33×), `nn::RmsNorm` (fused, 12×), `nn::KvCache` (PE
  streaming ring), and `nn::sdpa` — only their *glue* is dots-specific.
- Usage census: `nn::Linear`×33, `nn::RmsNorm`×12, `nn::KvCache`×7, `nn::Backbone`×6, `nn::TransformerLayer`,
  `nn::Attention`, `nn::Rope`, `nn::Mlp`, `nn::sdpa`, …

**New dots-specific glue (genuinely-new families with no shared analog — kept local, NOT forced into the
shared lib because they don't match any existing component's contract):**
- **DiT velocity-field predictor** — an 18-layer adaLN-**modulation** transformer (non-affine LayerNorm +
  per-block 6-way adaLN shift/scale/gate, qk-norm, GELU-tanh FFN, RoPE θ=1e4) + TimestepEmbedder +
  FinalLayer. (The shared `cfm` is a CFG-ODE-for-mel flow field; dots's per-patch AR-FM DiT is a different
  contract — modulation-conditioned, packed-sequence, recomputed per ODE eval.)
- **VAESemanticEncoder (PatchEncoder)** — a causal-conv downsample (streamed via a `conv_tail`) + a 24-layer
  pre-RMSNorm transformer (plain MHA, **no RoPE**) re-encoding each generated latent patch → one LLM token.
- **BigVGAN AudioVAE vocoder** — 48 kHz, snakebeta + anti-aliased alias-free up/down kaiser-sinc filters +
  a 4-layer SLSTM MI layer + a 6-stage causal-ConvTranspose decoder (fp32). (Not Mimi/DAC — a distinct
  neural-vocoder family; the saved kaiser-sinc filter buffers are loaded as-is, verified bit-identical to a
  recompute.)
- The **FM ODE solvers** (flow_matching CFG Euler + meanflow) + the AR/FM generation loop (prefill, decode,
  prompt-tail drop, EOS).

Because no shared component was modified, the other tch models are unaffected — **103 backend-torch lib
tests pass** unchanged + clippy clean.

## Per-bug-class checks (the 8-bug byte-identical playbook)
Each was actively root-caused; 5 of the 8 produced a real, fixed bug here:
1. **Fused ops** — LLM RMSNorm = decomposed-Mul (HF Qwen2RMSNorm); DiT q/k-norm + PE attn/ffn-norm =
   **FUSED `F.rms_norm`** running in **f32** (autocast upcasts `rms_norm`) with `eps = f32::EPSILON`
   (= `finfo(f32).eps` EXACTLY; a rounded `1.192e-7` literal drifts q/k ~3e-6). All biased Linears use
   `at::linear`. **BUG FOUND+FIXED.**
2. **bf16 vs f16** — LLM/DiT/PE run bf16 on CUDA under the sidecar's `core.to(bf16)`+autocast; the **vocoder
   runs f32**. The autocast dtype rules were replicated per-op (norms→f32-out, matmuls/convs→bf16,
   rotary→f32-then-downcast). **BUG FOUND+FIXED** (PE downsample Conv must run **bf16** — Conv is on
   autocast's bf16 list, not f32).
3. **Tokenizer** — the model's own Qwen2 byte-level BPE (`tokenizer.json`), the exact
   `[文本]{prompt}\n{text}[文本对应语音]<audio_gen_start>(<audio_gen_span>×N)` schedule. Verified: LLM
   `inputs_embeds` byte-identical.
4. **RoPE inv_freq** — LLM uses `f64_powf_rounded` (θ=1e6). DiT uses the vendored `RotaryEmbedding`
   (θ=1e4, f32 inv_freq, `cat(freqs,freqs)`, einsum). **THE #1 BUG: the inv_freq buffer must be computed on
   CPU then moved to CUDA** — the vendored buffer is CPU-built (`__init__` has no device) then `.to(cuda)`,
   and a CPU vs CUDA `pow` rounds 2 of 32 entries by 1 ULP; computing inv_freq directly on CUDA drifted the
   rotary angle ~1e-6 and compounded across the 18 DiT layers (DiT replay went from max\|Δ\|=0.05 → **0.0**).
   **BUG FOUND+FIXED.** Also: the PE attention has **rotary_bias=False → NO RoPE** (I had wrongly applied it).
   **BUG FOUND+FIXED.**
5. **TF32** — the sidecar runs autocast-bf16 (no f32→TF32 surface in the hot path); tch's default f32
   precision is left as-is. No TF32 bug.
6. **RNG draw count/order** — verified bit-exact: `manual_seed(0)` → (1) `randn_like([1,128,prompt_frames])`
   for the prompt-latent sample, then (2) one `randn([1,4,128])` per patch (flow_matching and meanflow
   alike). The captured draw firsts match the sidecar exactly. No extra draws.
7. **Conv pad** — PE downsample = causal Conv1d (left-pad `dilation*(k-1)`, streamed `conv_tail`); vocoder =
   causal Conv1d (left-pad) + causal ConvTranspose1d (right-trim `stride`); conv_pre is non-causal sym-pad 2.
8. **Batched CFG** — flow_matching runs cond+uncond as a batch-2 DiT forward (`cat([z_c,z_cfg],0)`), the
   bf16 cuBLAS reduction is the batched one. Verified: first FM velocity byte-identical (60160/60160).
   **PLUS a 9th, model-specific bug: the FM Euler ODE time grid.** torchdiffeq builds `t_infer = t0 +
   arange(0,n+1)*step_size` **in bf16** (so `t[1]=0.10009766`, `t[4]=0.40039062` — NOT clean `i*0.1`) and
   steps by `dt_i = t_infer[i+1]-t_infer[i]`. Using clean f64 times drifted the timestep embedding and
   compounded the ODE (first patch went from max\|Δ\|=0.03 → **0.0**). **BUG FOUND+FIXED.**

## RTF
CUDA bf16, "Hello world, this is a test of the dots text to speech system." (3.2 s @ 48 kHz):
- **RTF ≈ 2.3** (release) / ≈ 2.6 (debug), full pipeline (AR + per-patch FM ODE + vocoder).
- The sidecar's own EAGER path measured RTF 1.71 on the same input; the gap is bf16-GEMM efficiency + no
  torch.compile/CUDA-graphs (the port is correctness-first, eager). The model is inherently heavy: per
  generated patch it runs a 10-step batch-2 CFG DiT ODE + a 24-layer PatchEncoder re-encode + a 28-layer
  Qwen2 step. Perf (compile/CUDA-graphs/bucketing — the sidecar's optimize=True ~1.6 s warm) is future work,
  out of scope for the byte-identical port.

## Exact files changed
- **`crates/waav-infer-backend-torch/src/dots.rs`** (NEW, 1754 lines) — `dots::TorchDots` impl
  `waav_infer_core::model::TtsModel`; config selects base/soar/mf.
- **`crates/waav-infer-backend-torch/src/lib.rs`** — `pub mod dots;` (+ doc).
- **`crates/waav-infer-backend-torch/tests/cuda_torch_dots.rs`** (NEW, 171 lines) — `#[ignore]` live gate
  (latent byte-identity + audio validity + RTF); `DOTS_VARIANT=base|soar|mf` selects the variant (covers
  ALL THREE).
- **`ci/heavy_live_tests.sh`** — added the `cuda_torch_dots` gate (base) + a soar/mf variant loop.
- NO shared-library file touched. Validation artifacts (in the model cache, not the repo):
  `~/.cache/waav-models/dots-tts-{base,mf,soar}/{default_voice.safetensors,latent_stats.safetensors,
  prompt_text.txt}` (one-time conversion of the `.pt` pickles — tch can't portably read torch.save pickles)
  + `~/.cache/waav-models/dots-tts-golden/{dump_golden.py, latents_*_cuda_bf16.npy, audio_*_cuda_bf16.npy}`.

## Verification commands
```
source /home/bud/ditto/waav/waav-infer/gb10-env.sh
cargo test -p waav-infer-backend-torch --lib                              # 103 pass
cargo clippy -p waav-infer-backend-torch --all-targets --features cuda -- -D warnings   # clean
# byte-identity gate (per variant):
DOTS_VARIANT=base cargo test -p waav-infer-backend-torch --test cuda_torch_dots cuda_torch_dots -- --include-ignored --nocapture --test-threads=1
DOTS_VARIANT=soar ...   ;  DOTS_VARIANT=mf ...
# regenerate goldens: python3 ~/.cache/waav-models/dots-tts-golden/dump_golden.py <base|soar|mf> cuda bfloat16
```
