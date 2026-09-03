# ONBOARD — kyutai/pocket-tts (WaaV Infer, byte-faithful, runs-everywhere)

**Status: SHIPPED — byte-faithful on BOTH CPU and CUDA (GB10), gate green, clippy clean.**

kyutai **Pocket TTS** (https://github.com/kyutai-labs/pocket-tts, arxiv 2509.06926) — a 100M-param
**CPU-first flow-matching continuous-latent TTS** — is ported into the Torch backend and verified
byte-faithful to a deterministic reference golden on `TorchDevice::Cpu` (the kyutai-primary path) AND CUDA.

---

## 1. The architecture (recon) — NOT a discrete RQ-Transformer

The prompt hypothesised a Moshi/RQ-Transformer over discrete Mimi codes. **It is not.** Pocket TTS is a
**flow-matching latent LM** over the **continuous** Mimi latent (the quantizer is a `DummyQuantizer` — a bare
1×1 conv, no codebooks). The exact stack (from `config/english_2026-04.yaml` + the GitHub inference code +
the bf16 checkpoint shapes — all HfApi-verified):

| Component | Spec |
|---|---|
| **Backbone** (`StreamingTransformer`) | Moshi-class, `d_model=1024`, **16 heads, 6 layers**, **plain MHA (NO GQA)**, fused-QKV `in_proj` `[3072,1024]`, **full-interleaved RoPE** θ=10000 (`(x0,x1)` adjacent-pair complex), pre-norm `nn.LayerNorm`, **GELU** FFN `hidden_scale=4` (→4096), **no layer-scale**, unbounded causal context. Emits a `[1024]` conditioning + a scalar EOS logit per frame. |
| **Flow net** (`SimpleMLPAdaLN`) | The MAR diffusion-loss MLP: `flow_dim=512`, **6 AdaLN res-blocks**, **2 `TimestepEmbedder`s** (for the `(s,t)` flow times), a `cond_embed` from the backbone conditioning. **Lagrangian-Self-Distillation** (`lsd_decode`, **`lsd_decode_steps=1`**) deterministically integrates noise → a **32-dim** latent. `temp=0` ⇒ zero noise ⇒ fully deterministic ("greedy"). |
| **Continuous Mimi codec** | `DummyQuantizer` 1×1 conv (32→512) → `ConvTrUpsample1d` depthwise reframe ×16 (stride 16, k 32, groups 512) → **2-layer `ProjectedTransformer`** (LayerScale, sliding-window context-250, interleaved RoPE) → **SEANet decoder** ratios `[6,5,4]`, n_filters 64. Each latent → **1920 samples @ 24 kHz** (frame_rate 12.5). |
| **Text** | raw **SentencePiece** `tokenizer.model` (`n_bins=4000`), a LUT `nn.Embedding[4001,1024]` (the conditioner output IS the embedding; dim == d_model). |
| **Precision** | weights stored **bf16**, but the config `dtype: float32` ⇒ the runtime widens every param to **f32** (a lossless bf16→f32 widen). **f32 is the byte-faithful regime on CPU and CUDA** (no TF32). |

**The trajectory contract (traced + matched exactly):** the reference makes 5 `_sample_next_latent` calls for
"Hello world." — **1 prompt pass** (text positions `[0,1,2]`, reads `transformer_out[:, -1]`) then **4 AR
steps** (BOS on the first, then the prev latent; positions 3,4,5,6). EOS fires at AR-step 1 (logit −1.47 > −4.0);
`frames_after_eos=2` ⇒ break after AR-step 3 ⇒ **5 latents** captured. Only the **AR-step latents** (3 of them,
after the EOS truncation) are Mimi-decoded into the canonical wav (5760 samples = 3×1920). The prompt latent
is captured in the trajectory but not decoded.

---

## 2. Gated-repo blocker — RESOLVED

`kyutai/pocket-tts` (the voice-cloning weights `tts_b6369a24.safetensors` + the `embeddings*/*.safetensors`
voices) is **gated** and the supplied token is not on the authorized list (HfApi can list metadata, file
download 403s). **Resolution:** the package's own fallback repo **`kyutai/pocket-tts-without-voice-cloning`
is NOT gated** — its `languages/english_2026-04/{model.safetensors (219 MB), tokenizer.model}` is the SAME
model architecture (the config references it as `weights_path_without_voice_cloning`). Downloaded cleanly;
this is exactly the path the upstream package takes when voice-cloning access is absent. The golden + the port
both use it. Voice conditioning (the gated speaker embeddings) is therefore **not exercised** — the golden uses
the text-only path (empty voice state), which fully exercises backbone + flow + Mimi-decode.

---

## 3. Acquisition

- Weights/tokenizer → `~/.cache/waav-models/pocket-tts/{model.safetensors, tokenizer.model}`
- `waav.json`: `{ "runtime": { "backend": "torch", "architecture": "pocket_tts", "model": "…", "dtype": "fp32" }, … }`

## 4. Golden (throwaway reference venv, CPU, f32, deterministic) — `WaaV/inferv2/REVIEW/pocket_tts_golden/`

A throwaway venv (`pip install pocket-tts`, torch 2.12, **reference-only — NOT a serving path**) captured the
deterministic (`temp=0`) greedy synth of `"Hello world."`. **Bit-identical across re-runs and thread counts.**
Captured: `text_tokens` `[1,3]`, `text_embeddings`, **`latents` `[5,32]`**, **`eos_flags` `[5]`**, **`wav` `[5760]`**,
a single-shot **`mimi_decoded_wav` `[1,1,15360]`** of a fixed 8-frame latent (independent codec gate),
`cond_normed`/`eos_logits` (RCA tensors), `meta.json`, `golden.wav`, `make_golden.py` (the recipe).

## 5. Port — `crates/waav-infer-backend-torch/src/pocket_tts.rs` (NEW, owned)

Composes the shared lib maximally; pocket-specific glue spelled locally (the hibiki/dia2 pattern).

**Reuse map (DISCOVER+REUSE):**
| Shared component | Used for |
|---|---|
| `nn::Linear::matmul` | every projection (backbone QKV/out/FFN, flow MLP, mimi) |
| `nn::LayerNorm::fused` (== `nn.LayerNorm`) | backbone norm1/norm2, `out_norm`, mimi norms |
| `nn::Rope` + **`nn::InvFreq::f64_powf`** + **`Rope::apply_interleaved_full`** | the EXACT `(x0,x1)`-interleaved RoPE (`apply_interleaved_full` is byte-identical to pocket's `apply_rope`: `reshape(..,half,2)`, `o0=x0·cos−x1·sin`, `o1=x1·cos+x0·sin`, f32 upcast) |
| `nn::sdpa` | attention (additive mask → math kernel) |
| **`codec::conv::{MimiConv, MimiConvT}`** | the SEANet decoder convs + the resample up-conv (their causal constant-pad + right-trim `k−s` **IS** pocket's `StreamingConv*1d` single-shot/offline math) |

**Pocket-specific glue (local):** the `SimpleMLPAdaLN` flow net (AdaLN `modulate`, the **Bessel-corrected
var-RMSNorm** of the `TimestepEmbedder` — `x·α·rsqrt(eps+var(unbiased=True))`, distinct from every existing
`RmsNorm`; the custom mlp-`LayerNorm` with biased var + no-affine final), the `lsd_decode` ODE, the
windowed/unbounded causal position mask, the continuous-Mimi `decode_from_latent`, and the prompt-pass +
AR-loop trajectory (BOS NaN→`bos_emb`, EOS + `frames_after_eos` break counting AR steps only).

Single-shot offline Mimi decode is used (verified **== the per-frame streaming decode within 1.7e-7**).

## 6. Byte-faithful gate — `tests/cuda_torch_pocket_tts.rs` (NEW, owned)

| Gate | CPU (`TorchDevice::Cpu`) | CUDA (GB10) |
|---|---|---|
| **Greedy latents** vs golden (max abs Δ over 5×32) | **4.6e-6** | **3.7e-6** |
| **EOS flags** (5) byte-identical | ✓ exact | ✓ exact |
| trajectory length | 5 == golden | 5 == golden |
| **Mimi decode** corr / maxΔ (15360 samples) | **1.000000** / 6.2e-7 | **1.000000** / 3.0e-4 |
| **E2E wav** corr (5760 samples) | **1.000000** | **0.999999** |

Both legs are byte-faithful **to the same CPU-f32 golden**. The residual latent Δ (~5e-6 CPU, ~3e-6 CUDA) is
the **irreducible f32 GEMM-accumulation floor** between tch/libtorch's BLAS and the reference torch (same f32
graph) — RCA-confirmed via an f32-CPU bisection: it appears already in the **backbone** conditioning at the
prompt step (~3.5e-6), is unchanged by swapping fused↔manual SDPA or fused↔decomposed LayerNorm, and the
reference itself is bit-stable across thread counts (so it is a cross-runtime BLAS-order delta, not a spelling
bug). It does **not** flip any EOS decision, change the trajectory length, or perturb the audio (corr 1.0).
**That 3.7e-6 on CUDA is itself the proof TF32 is OFF** — TF32 on a 1024-dim GEMM would yield ~1e-3 errors.

## 7. RUNS-EVERYWHERE (the user's ask)

- **CPU** (kyutai-primary): byte-faithful, gate green.
- **CUDA** (GB10): the SAME device-agnostic `tch` f32 graph, byte-faithful to the same golden.
- **Multi-hardware seam:** `TorchPocketTts::load(dir, dev: TorchDevice)` takes the resolved `TorchDevice`
  exactly like every other Torch model; `TorchDevice::resolve(DeviceRequest::{Cpu,Cuda,Auto})` is the
  fleet's device/EP-portability seam (CPU-floor auto policy). Pocket-tts has **zero device-specific code**
  (all ops carry the device/dtype), so it slots in with no special-casing — the portability is structural.
  Both gate legs run via the `pocket_tts_byte_faithful_cpu` / `…_cuda` test pair.

**Perf (note, not a gate):** single-shot batch e2e RTF ≈ **0.9× CPU / 1.1× CUDA** on GB10 (aarch64) for the
tiny 5-frame clip. This is FAR below kyutai's ~6× claim because (a) it is single-shot batch, not the streaming
2-thread setup their claim measures, (b) libtorch eager per-step tensor allocation dominates at this scale,
(c) GB10 aarch64 CPU ≠ MacBook-Air-M4. The model is correct; a streaming/threaded perf pass is the lever to
close the RTF gap (out of scope for the byte-faithful onboard).

## 8. Verification

- `cargo test -p waav-infer-backend-torch --lib` → **190 passed** (incl. the shared codec mimi/rope/layernorm
  byte-identity tests).
- `cargo clippy -p waav-infer-backend-torch --all-targets -- -D warnings` → **clean**.
- Shared `nn::`/`codec::` were **NOT edited** by this onboard (only `pocket_tts.rs` + the `lib.rs` module decl/
  re-export + the new test), so dia2/csm/hibiki byte-identity is untouched (their shared-lib tests pass).

## 9. Files

- **NEW (owned):** `crates/waav-infer-backend-torch/src/pocket_tts.rs`
- **NEW (owned):** `crates/waav-infer-backend-torch/tests/cuda_torch_pocket_tts.rs`
- **EDIT (shared, minimal):** `crates/waav-infer-backend-torch/src/lib.rs` — `pub mod pocket_tts;` + `pub use pocket_tts::{PocketTtsError, TorchPocketTts};`
- `~/.cache/waav-models/pocket-tts/{model.safetensors, tokenizer.model, waav.json}`
- `WaaV/inferv2/REVIEW/pocket_tts_golden/` — golden npys + `make_golden.py` + `golden.wav` + `meta.json`

## 10. Follow-ups (not blockers)

1. **SentencePiece encode:** the gate feeds golden token ids (the LAW path). A real synth needs to encode text
   from the raw SP `tokenizer.model` — the in-tree `components::SentencePieceTokenizer` is decode-only;
   wiring an encode path (unigram SP via `spm_precompiled`, already in the lockfile) is the remaining glue for
   end-user synth from arbitrary text.
2. **Voice conditioning:** the gated speaker embeddings / audio-prompt voice-clone path (`speaker_proj_weight`
   + the Mimi **encoder**, which `codec::mimi_encoder` already provides) is ported-ready but unexercised
   (gated voices). A non-gated `tts-voices` wav can drive it once encode is wired.
3. **Streaming + threaded perf** to chase the ~6× CPU RTF claim.
