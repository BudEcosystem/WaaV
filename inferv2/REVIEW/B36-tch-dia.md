# B36 — Nari Labs **Dia-1.6B** ported to the in-process tch-rs backend (composing the shared library)

**Goal:** port `dia-1.6b` (Nari Labs Dia, the encoder-decoder AR codec-TTS — the predecessor of dia2) from the
Python torch sidecar onto the in-process `tch` backend, BYTE-IDENTICAL to the sidecar, **composing the shared
library** (no primitive reimplementation).

## TL;DR
- **dia-1.6b is a COMPLETELY DIFFERENT architecture from dia2** (the prompt's "differs in size/config + maybe
  the delay pattern; uses a Kyutai-Mimi codec like dia2" assumption was wrong). dia2 = decoder-only AR + Mimi
  codec, 24 kHz. **dia-1.6b = an ENCODER-DECODER** (`is_encoder_decoder: true`, HF transformers-native
  `DiaForConditionalGeneration`): a 12-layer byte-level text encoder → an 18-layer **GQA cross-attention**
  decoder emitting a **9-codebook** delay-patterned `[0,8,9,…,15]` audio stream + CFG, decoded by the
  **Descript Audio Codec (DAC)** to **44.1 kHz** (NOT Mimi).
- **Byte-identical: YES on the bit-faithful CPU-fp32 path; CUDA-bf16 = the documented dia2/csm floor.**
  - Encoder hidden: **14336/14336 bit-exact (max|Δ|=0.0)** vs the sidecar `DiaEncoder.last_hidden_state`.
  - Step-0 channel-0 post-CFG argmax: **exact (568 == sidecar)**; top-8 order exact.
  - **CPU fp32 greedy raw codes: 23409/23409 BYTE-IDENTICAL (first-div=None)** — the strict LAW MET (all 9
    channels × all 2601 frames exact vs the sidecar CPU-fp32 greedy golden; the AR-codec-TTS bit-faithful bar).
  - **CUDA bf16 greedy: channel-0 (the EOS-trigger spine) byte-identical over ALL 2601 frames**; the per-channel
    flips are **0.0-gap bf16 ties** — the SAME inherent floor dia2/csm document. Proof it's inherent: the
    sidecar's OWN cuda-bf16 and cpu-fp32 greedy goldens disagree at the very same frames/channels (e.g. frame
    21 ch6: cuda=543, cpu=610), so no implementation can match cuda-bf16 there without the exact cuBLAS
    reduction order. The strict all-frames LAW is therefore proven on CPU-fp32, exactly as dia2/csm.

## Shared components COMPOSED (proving reuse, not reimplementation)
| Shared component | How dia composes it |
|---|---|
| `nn::Backbone` | the 12-layer ENCODER stack AND the 18-layer DECODER stack (two `Backbone`s) |
| `nn::TransformerLayer` | encoder layer (pre-norm + self-attn + MLP); decoder layer via `.with_cross(...)` (the cohere AED path: self-attn → cross-attn → MLP) |
| `nn::Attention` | encoder self-attn (MHA, scale 1, RoPE); decoder self-attn (GQA 16q/4kv, scale 1, RoPE) |
| `nn::CrossAttention` | the decoder cross-attn (MHA 16/16, the constant encoder K/V) — **extended** with a `fused` flag (see below) |
| `nn::RmsNorm` | `decomposed(Square::Pow, weight_first=true)` — HF `DiaRMSNorm` is the hand-decomposed `x.pow(2).mean→rsqrt→cast(dt)→weight·x`, NOT the fused kernel |
| `nn::Rope` | `InvFreq::f32_tensor_arange` + f32 tables + `apply_positions` (HF `compute_default_rope_parameters`; same family as csm) |
| `nn::Linear` | `matmul` (all Dia projections are `nn.Linear(bias=False)`) |
| `nn::KvCache` | the decoder self-attn ring KV (read via the new `ContiguousMasked`, below) |
| `nn::Mlp` | `swiglu_fused` (HF `DiaMLP`: `gate_up_proj` fused `[2·inter,h]`, `chunk`, `up·silu(gate)`, `down_proj`) |
| `kernels::DefaultPolicy` | `dia2_policy()` (TF32-on-CUDA intent) |

So a Dia layer = **config + glue COMPOSING** the same primitives every other tch model uses. No primitive was
re-written.

## Shared-library EXTENSIONS made (added to the lib, with unit tests, existing models re-verified)
1. **`codec::dac` — a NEW shared DAC (Descript Audio Codec) decoder family** (the sibling of `codec::mimi`).
   dia-1.6b uses DAC, not Mimi, so this is a genuinely-new shared component future DAC-based models reuse:
   `DacDecoder` (RVQ `from_codes` → `conv1` → 4 upsample blocks → snake → `conv2` → tanh) + `Snake1d`-style
   activation + the **symmetric-padded** `DacConv`/`DacConvT` (the padding regime is the family scar vs Mimi's
   causal convs) + `DacResidualUnit`/`DacDecoderBlock`/`DacRvq`/`DacCodebook`. Weight-norm is pre-folded in the
   `descript/dac_44khz` checkpoint (plain `conv.weight`/`bias`), so no `weight_g`/`weight_v` reconstruction. The
   1×1 `out_proj` conv is issued as the shared `nn::Linear` over the channel axis (byte-identical to the 1×1
   conv). **4 unit tests** (snake1d, symmetric conv length, ConvT upsample geometry, residual-unit center-crop).
2. **`nn::KvCache::append_contiguous_masked`** + **`CacheRead::ContiguousMasked`** — narrowed-contiguous K/V +
   an **all-zero** `[1,1,1,cur]` mask. The mask is a numerical no-op but STEERS libtorch's SDPA onto the
   **MATH** backend over EXACTLY the written keys, reproducing HF's `sdpa_attention_forward` (for `q_len==1` it
   forces `is_causal=False` and passes the prepared single-query causal mask = all-zeros). The narrow (NOT
   `append_full_masked`'s full padded buffer) avoids the `finfo.min`-padding-slot / large-N-kernel-pick
   artifacts. **1 unit test** (zero mask over the valid length; K/V == the plain contiguous read).
3. **`nn::CrossAttention.fused`** — a flag selecting the FUSED libtorch SDPA (dia: HF runs cross-attn through
   `sdpa_attention_forward`) vs the hand-written `sdpa_manual` (cohere's validated default). `CrossAttention::new`
   keeps the manual default; cohere + the nn cross-attn tests were converted to `::new` and **re-run green**
   (cohere's bytes unchanged).
- All 103 lib unit tests green after the extensions (the existing models' byte-identity unit tests included);
  `clippy --all-targets -D warnings` clean.

## The 8-bug byte-identity playbook — per-class checks
1. **Fused vs hand-decomposed op** — HF `DiaRMSNorm` is the **decomposed** form (`pow(2).mean → rsqrt → cast →
   weight·x`), so dia uses `RmsNorm::decomposed(Square::Pow, weight_first=true)` (NOT the fused kernel dia2 uses).
   `DiaMLP`/`DiaSelfAttention`/`logits_dense` are plain `nn.Linear` → `Linear::matmul`.
2. **bf16 vs f16** — the sidecar loads `dtype="bf16"` → the whole model (projections, norms, embeddings,
   `logits_dense`, the DAC codec) is bf16 on CUDA; dia loads `cast(dt)` everywhere (bf16 CUDA / f32 CPU).
   **Two bf16-precision fixes were required (B36 tail-tie hunt):** (a) the **multi-channel embedding sum** runs
   in bf16 (`embeds.sum(dim=2)` after the model is bf16 — NOT an f32 sum), and (b) **`logits_dense` runs in
   bf16** then the RESULT is cast to f32 (HF: bf16 matmul → `_sample`'s `.to(float32)`; NOT an f32-input matmul).
3. **Tokenizer** — `DiaTokenizer` is raw UTF-8 bytes (id = byte value) with `[S1]`=1/`[S2]`=2, `add_special_tokens
   =False`. The sidecar feeds `f"[S1] {text}"` → a **LEADING SPACE byte** follows the `[S1]` token. Replicated +
   unit-tested: `"[S1] Hello world." → [1,32,72,101,108,108,111,32,119,111,114,108,100,46]` (matches the live
   processor exactly).
4. **RoPE inv_freq** — HF `compute_default_rope_parameters` = `1/(θ^(arange(0,d,2,int64)/d))` as an **f32 tensor**
   op → `InvFreq::f32_tensor_arange` (NOT f64-host-powf, NOT min/max); f32 cos/sin tables (`cos.to(x.dtype)` at
   apply); applied by `position_ids` (decode position = `past_len`).
5. **TF32** — the Dia sidecar runs the default PyTorch context (TF32 on for f32 matmuls on Ampere+/GB10); dia
   gates `libtorch_tf32::enable()` on `dia2_policy().allow_tf32` (the same recipe dia2 uses), so the f32 ops
   (the upcast logits, the f32 RoPE GEMM) round identically.
6. **RNG draw count/order** — N/A for the byte-identity bar here: the reference is **greedy** (`do_sample=False`
   → `argmax`, no RNG draw). `tch::manual_seed(0)` is still set for parity. (The default config also ships
   `do_sample=True` temp=1.8/top_p=.9/top_k=50; the greedy bar is the deterministic one the LAW requires.)
7. **Causal-conv pad** — DAC uses **symmetric** `nn.Conv1d(padding=p)`/`ConvTranspose1d(padding=⌈s/2⌉)` (NOT the
   causal left-pad of Mimi). `codec::dac::DacConv`/`DacConvT` implement the symmetric regime (the family scar);
   the `DacResidualUnit` center-crops the residual `hidden[..., p:-p]` exactly as the reference.
8. **Batched-vs-unbatched CFG** — the 2 CFG branches (cond + the all-ZERO uncond text) are run BATCHED `[2,1,…]`
   through the decoder in one forward (matching HF's doubled-batch CFG), so the cuBLAS GEMM batching matches.
   The exact logits-processor chain is reproduced: `DiaCFG(scale=3.0, top_k=50) → Temperature(1.8) →
   DiaEOSChannelFilter → DiaEOSDelayPattern` (the warpers are NOT added for greedy; only the unconditional
   Temperature warper survives — and is monotonic, so it does not change the argmax). The per-channel EOS→PAD
   `_sample` masking (`next = next·unfinished + pad·(1-unfinished)`) + the delay-EOS cascade are replicated.

## The CUDA-bf16 floor — root-cause (NOT hand-waved)
The CUDA-bf16 greedy diverges from the cuda golden at scattered **0.0-gap ties** (top1≈top2 within a single
bf16 ULP) — first at frame 21 ch6 (tch=610, sidecar-cuda=543). This was driven to its root:
- the **encoder is bit-exact** (max|Δ|=0.0) → cross-attn K/V are byte-faithful;
- **channel 0 (the spine) is byte-identical over all 2601 frames** → the decoder math is byte-faithful;
- the per-channel flips are at ties where **the sidecar's own cuda-bf16 and cpu-fp32 goldens disagree**
  (cuda=543, cpu=610 at frame-21-ch6) — an **inherent bf16 ambiguity**, not a port defect;
- every SDPA-backend permutation was tried (no-mask `FusedAuto` `Contiguous`; explicit-zero-mask `MATH`
  `ContiguousMasked`; full-padded `finfo.min` `FullMasked`; fused vs manual cross-attn) — the frame-21 tie is
  **stable** across all of them, confirming it is the reduction-order tie, not a backend/op bug. (`FullMasked`'s
  oversized buffer introduced an EARLIER frame-111 artifact, so `ContiguousMasked` — the HF-faithful exact-length
  explicit-mask path — is the chosen config.)
This is exactly the floor [[waav-infer-100-percent-correctness]] records for the AR codec-TTS family ("on CUDA
bf16 a tiny matmul perturbation flips a sampled token and compounds — the sidecar documented this — so on CUDA
the bar is correct intelligible audio + the deterministic codec-decode parity; on CPU f32 the AR codes track the
reference far more tightly"). The **strict all-frames byte-identical LAW is met on CPU-fp32**, where the ties
resolve unambiguously.

## RTF
CUDA bf16, "[S1] Hello world." (the greedy default rambles to ~30s / 2585 frames): AR-gen ≈ 47–88 s for 2601
frames (the `ContiguousMasked` full-cache SDPA + per-step host round-trips), audio ≈ 30 s → **RTF ≈ 1.6–2.9**
(> 1). The AR loop is the cost (Wave-4 perf levers NOT yet applied: this is an eager per-step host-driven loop
with a 3087-slot cache; the IoBinding/CUDA-graph/lockstep-batch levers from INFER_PERF would bring it < 1, the
same as the other tch AR models before their perf passes). Reported, not optimized — the deliverable here is
byte-identity, not perf.

## Files changed (ONLY these + the noted shared-lib extensions)
- `crates/waav-infer-backend-torch/src/dia.rs` — **NEW** (`dia::TorchDia impl waav_infer_core::model::TtsModel`;
  the encoder/decoder glue, byte-level tokenizer, delay pattern, the 3-stage logits-processor chain, the
  encoder-decoder greedy loop with per-channel EOS→PAD + delay-EOS cascade, the DAC weight-name mapping).
- `crates/waav-infer-backend-torch/src/lib.rs` — `pub mod dia;`.
- `crates/waav-infer-backend-torch/src/codec/dac.rs` — **NEW shared component** (DAC decoder family) + tests.
- `crates/waav-infer-backend-torch/src/codec/mod.rs` — `pub mod dac;` + re-exports + catalog doc.
- `crates/waav-infer-backend-torch/src/nn/kv_cache.rs` — `append_contiguous_masked` + test.
- `crates/waav-infer-backend-torch/src/nn/self_attention.rs` — `CacheRead::ContiguousMasked` + the wiring;
  `CrossAttention.fused` + `CrossAttention::new`.
- `crates/waav-infer-backend-torch/src/nn/layer.rs`, `src/cohere.rs` — converted the `CrossAttention` struct
  literals to `::new` (cohere bytes unchanged; re-verified green).
- `crates/waav-infer-backend-torch/tests/cuda_torch_dia.rs` — **NEW** (#[ignore] live gates: DAC codec/synth
  parity, CUDA-bf16 ch0-spine byte-identity + documented bf16-tie floor, CPU-fp32 strict byte-identity, the
  step-0 AR-math probe, the encoder-bit-exact diagnostic).
- `ci/heavy_live_tests.sh` — added the `cuda_torch_dia` + `cpu_fp32_raw_codes_byte_identical` gates.

## Goldens (persisted at `/home/bud/.cache/waav-models/dia-golden/`, dumped via `/tmp/dump_dia_final.py`)
`raw_seq_{cuda_bf16,cpu_fp32}_greedy.npy` (`[9, seq]` the pre-delay-revert `_sample` stream),
`codes_*` (`[seq,9]` reverted), `wav_*` (the DAC waveform), `step0_logits_raw_*`, `enc_cond_cuda_bf16.npy`.
