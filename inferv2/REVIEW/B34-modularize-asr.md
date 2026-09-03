# B34 — Modularize the shared audio encoder (`asr::AudioEncoder`); rewire voxtral + ark

Phase 3b of the WaaV Infer Torch-backend modularization (memory `waav-infer-modularize-reuse`; spec
`WaaV/inferv2/REVIEW/COMPONENT_CATALOG.md` → `asr/encoder.rs  AudioEncoder`). The Whisper-style **audio
encoder** that voxtral.rs and ark.rs had each ported as a self-contained copy (conv stem + transformer tower +
projector) is extracted ONCE into `crates/waav-infer-backend-torch/src/asr/`, config-parameterized over the
PAD CONFIG + tower details, and both models are rewired to it with their local copies DELETED. Both stay
**byte-for-byte identical** (proven on GPU).

## Outcome
- `asr::AudioEncoder` extracted to `src/asr/encoder.rs`, composing the Phase-1/2 `nn::` primitives
  (`sdpa_manual`, `Rope`, `LayerNorm`, `RmsNorm`, `Mlp`, `Linear`); mel stays in `waav_infer_components`.
- voxtral.rs + ark.rs DELETE their local `EncLayer` + `AudioEncoder` (+ voxtral's `left_pad_time` and dead
  `Mlp::forward`), each replaced by a ~20-line declarative `enc_config()` + the existing weight-load loop.
- **voxtral GPU gate: 100.0% char-identity** (torch CUDA == ORT CPU, strict kokoro clip). "W.A.V." preserved
  (the B22 causal conv2 left-pad survives the refactor).
- **ark GPU gate: 100.0% char-identity** (torch CUDA == sidecar golden).
- 84 lib tests green (3 new `asr::` byte-faithfulness tests, both regimes Δ==0 + the pad scar); clippy clean.
- cohere untouched (its FastConformer encoder is ORT-CUDA hybrid; its nemo128 mel genuinely differs from the
  voxtral/ark Whisper log-mel — nothing shared, correctly left alone).

## The `AudioEncoder` API + pad/tower config

```rust
// src/asr/encoder.rs
pub struct AudioEncoder {                 // built by the model loader (weight NAMES differ per model)
    pub conv1_w/conv1_b, conv2_w/conv2_b: Tensor,   // the 2-conv stem
    pub layers: Vec<EncLayer>,            // the transformer tower
    pub norm: nn::Norm,                   // final/adapter norm (after tower, before downsample)
    pub proj1, proj2: nn::Linear,         // the 2-layer projector (lin → gelu → lin)
    pub rope: nn::Rope,                   // built by the caller with the model's inv_freq / rot_dim
    pub cfg: EncoderConfig,
}
impl AudioEncoder { pub fn forward(&self, mel: &Tensor) -> Tensor; }  // input_features[1,n_mels,T] → audio_embeds[1,N,out]

pub struct EncLayer {                     // one pre-norm encoder layer (non-cached full self-attn)
    pub q, k, v, o: nn::Linear,
    pub attn_ln, final_ln: nn::Norm,      // RmsNorm (voxtral) | LayerNorm (ark)
    pub mlp: nn::Mlp,                     // SwiGLU-fused (voxtral) | ungated-GELU (ark)
}

pub struct EncoderConfig {
    pub n_layers, enc_dim, heads, head_dim: …,
    pub conv_pad: ConvPad,                // the load-bearing scar
    pub rope: EncRope,
    pub window: Option<usize>,            // Some(w) = causal sliding-window; None = bidirectional
    pub merge: i64,                       // downsample factor
    pub scale: f64,
}
pub enum ConvPad { Causal { conv1, conv2 }, Symmetric { conv1, conv2 } }  // left-pad-only vs nn.Conv1d(padding)
pub enum EncRope { RotateHalfStart, PartialInterleaved }                  // apply_start(0) vs apply_interleaved

// also pub: left_pad_time(x, pad), causal_mask(q,kv,window,dev,dt)
```

`forward`: conv stem (per `ConvPad`) → erf-gelu ×2 → permute `[1,enc_dim,S]→[1,S,enc_dim]` → tower (mask built
ONCE from `window`) → final `norm` → merge-K reshape → `proj1` gelu → `proj2`. `EncLayer::forward` =
`attn_ln → q/k/v/o → RoPE(per cfg) → sdpa_manual(opt mask) → +residual → final_ln → mlp → +residual`.

The encoder is **non-cached full self-attention** (runs once/clip), so it composes the `nn::sdpa_manual`
primitive directly rather than the cached `nn::Attention` (which is the decoder's per-step ring-KV path, and
cannot express ark's `apply_interleaved` RoPE). Every numeric op is a shared `nn::` call — only the conv stem,
the tower loop, and the projector glue live in `asr`.

### The per-model config (the dedup map)
| model | conv pad | tower norm | rope | mask | mlp | enc_dim×heads | merge |
|---|---|---|---|---|---|---|---|
| **voxtral** | `Causal{2,1}` | `RmsNorm(Mul)` | rotate-half `apply_start(0)` | causal window 750 | SwiGLU fused (down bias) | 1280×32 | 4 |
| **ark** | `Symmetric{1,1}` | `LayerNorm` | partial-interleaved `apply_interleaved` | none (bidirectional) | ungated GELU (fc1/fc2 bias) | 1280×20 | 4 |

The **PAD CONFIG is load-bearing**: voxtral's causal conv2 left-pad is `kernel−stride = 1` (the B22 fix; left-
pad 2 shifted the stride-2 downsample phase and flipped "W.A.V."→"W.A.A.V."); ark's is symmetric
`nn.Conv1d(padding=1)` (the B29 distinction). Both are pinned in `ConvPad` and unit-tested.

## LOC reduction
`git diff --numstat` (repo root `crates/waav-infer-backend-torch/src/`):

| file | +ins | −del | note |
|---|---|---|---|
| `voxtral.rs` | 40 | 129 | deleted local `EncLayer`+`AudioEncoder`+`left_pad_time`+dead `Mlp::forward`+pad test; added `enc_config()` + Norm/Mlp load wiring |
| `ark.rs` | 41 | 106 | deleted local `EncLayer`+`AudioEncoder`+pad test; added `enc_config()` + Norm/Mlp load wiring |
| `asr/mod.rs` | 343 | 5 | stub → full module (catalog doc + re-exports + 3 byte-faithfulness tests) |
| `asr/encoder.rs` | **+250** (new) | — | the shared encoder (all production code) |

- **~235 lines DELETED from the two model files** (the two duplicated ~75-line encoder copies + their pad
  tests). Replaced by ~81 lines of declarative config + load glue across both.
- The shared `asr::encoder` is **250 prod LOC** covering BOTH models (one impl), vs the ~150 LOC of duplicated
  encoder shape it collapses. `asr/mod.rs` adds the catalog doc-comment, re-exports, and ~300 LOC of CPU
  byte-faithfulness tests.

## Proofs

### Unit (CPU, gate every `cargo test --lib`)
`cargo test -p waav-infer-backend-torch --lib` → **84 passed; 0 failed**. New `asr::tests`:
- `voxtral_regime_matches_inline` — shared `AudioEncoder` (Causal pad, RmsNorm, rotate-half, window mask,
  SwiGLU) output of a fixed mel **Δ==0** vs an inline re-implementation of voxtral's exact pre-refactor op.
- `ark_regime_matches_inline` — shared `AudioEncoder` (Symmetric pad, LayerNorm, partial-interleaved, no
  mask, ungated-GELU) output of a fixed mel **Δ==0** vs an inline re-implementation of ark's exact op.
- `conv_pad_phase_and_length` — the causal-vs-symmetric conv2 (stride-2) downsample PHASE + the documented
  output LENGTH; a mis-set conv2 pad (2) shifts the phase and fails here (pins the load-bearing scar for both
  models in one place; the per-model `conv_stem_*` tests in voxtral.rs/ark.rs were removed as duplicates).

### GPU byte-identity (run one at a time; `free -g` 47–49G free; no OOM)
- **voxtral** `cargo test -p waav-infer-backend-torch --features cuda --test cuda_torch_voxtral_vs_ort --
  --include-ignored --test-threads=1`:
  `EXACT char-identity: 100.0%` on the strict kokoro clip (torch CUDA == ORT CPU); RTF 0.90. Transcript
  contains "W.A.V." (not "W.A.A.V.") — the causal conv2 left-pad is intact. (Soft Mandarin clip 82.4% = the
  pre-existing, documented bf16-vs-q4-reference drift, not a regression.)
- **ark** `cargo test … --test cuda_torch_ark -- --include-ignored --test-threads=1`:
  `EXACT char-identity: 100.0%` (torch CUDA == sidecar `transcript_cpu_fp32.txt` golden); RTF 0.240.

### Clippy
`cargo clippy -p waav-infer-backend-torch --all-targets -- -D warnings` → clean (all cuda-feature test targets
also compile via `--features cuda --no-run`).

## Files changed (exhaustive)
- `crates/waav-infer-backend-torch/src/asr/encoder.rs` — **NEW**: `AudioEncoder` + `EncLayer` +
  `EncoderConfig` / `ConvPad` / `EncRope` + `left_pad_time` / `causal_mask`.
- `crates/waav-infer-backend-torch/src/asr/mod.rs` — stub → catalog doc-comment + re-exports + the 3
  byte-faithfulness unit tests.
- `crates/waav-infer-backend-torch/src/voxtral.rs` — import `asr::{AudioEncoder,…}`; deleted local
  `EncLayer`/`AudioEncoder`/`left_pad_time`/dead `Mlp::forward`; added `enc_config()`; load builds the shared
  encoder (Norm::Rms + Mlp::swiglu_fused); removed the now-duplicate conv-pad test.
- `crates/waav-infer-backend-torch/src/ark.rs` — import `asr::{AudioEncoder,…}`; deleted local
  `EncLayer`/`AudioEncoder`; added `enc_config()`; load builds the shared encoder (Norm::Layer + Mlp::ungated
  GELU); removed the now-duplicate conv-pad test.

NOT touched: `lib.rs` (asr mod was pre-declared), `cohere.rs`, `ci/heavy_live_tests.sh`,
`COMPONENT_CATALOG.md`, dia2/csm/cosyvoice3/vibevoice, other crates (all verified unmodified via `git
status`). cohere keeps its FastConformer-on-ORT-CUDA encoder; its nemo128 mel preprocessor differs from the
voxtral/ark Whisper log-mel, so no mel/adapter was shared with it (correctly left untouched per the brief).

**No output changed** — both byte-identity gates remain at 100%.
