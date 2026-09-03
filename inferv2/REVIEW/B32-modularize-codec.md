# B32 — Modularize the shared Mimi neural-codec decoder (Phase 3a)

**Status: COMPLETE.** The Kyutai **Mimi** neural-codec decoder — duplicated byte-for-byte (structure) inside
both `dia2.rs` and `csm.rs` — is extracted ONCE into
`crates/waav-infer-backend-torch/src/codec/` and both models are rewired to it. **dia2 and csm both stay
byte-identical** (608/608 + 544/544 dia2 codes, 4000/4000 csm greedy codes, dia2 codec-parity 0.052%).

## What was deduplicated

`dia2.rs` and `csm.rs` each carried their own full Mimi `decode` path: a 1×1 / causal-Conv1d / causal-ConvT
primitive set + a SEANet residual stack + a split-RVQ dequantizer + an 8-layer pre-norm RoPE
sliding-window transformer + the sliding-window mask builder. Structurally identical in both; the only real
differences are two **byte-identity scars** that flow from each model's compute dtype. These are now captured
in one config, never smoothed over.

## The extracted module — `src/codec/`

| file | contents |
|---|---|
| `codec/conv.rs` | `Conv1x1`, `MimiConv` (causal left-pad + stride-multiple right-pad), `MimiConvT` (right-trim `k−s`), `ResBlock`, `pad1d`, `sliding_window_causal_mask`, `MaskFill`, `dt_min` |
| `codec/rvq.rs` | `RvqDequant` + `RvqSplit` (the split-RVQ `quantizer.decode`) + `resolve_codebook` (`embed_sum/clamp(usage,ε)`) |
| `codec/mimi.rs` | `MimiDecoder` (the full codes→waveform pipeline), `MimiLayer`, `SeaNet`, `MimiConfig` |
| `codec/mod.rs` | module docs + re-exports + the byte-faithful unit tests |

It composes the Phase 1+2 `nn::` library: `nn::Linear` (matmul projections), `nn::Rope` (the RoPE table,
built by the caller), `nn::sdpa` (the fused masked SDPA kernel). No transformer primitive is re-implemented.

### Public API

```rust
// codec/mimi.rs
pub struct MimiConfig {
    pub hidden: i64, pub layers: usize, pub heads: i64, pub head_dim: i64,
    pub norm_eps: f64, pub sliding_window: i64, pub sample_rate: u32,
    pub dt: Kind,            // f32 (dia2) | BFloat16 (csm)  — the byte-identity dtype scar
    pub mask_fill: MaskFill, // NegInf (dia2) | DtypeMin (csm) — the byte-identity mask scar
}
impl MimiConfig { pub fn mimi_24khz(dt: Kind, mask_fill: MaskFill) -> Self; } // released Kyutai scalars

pub struct MimiDecoder {
    pub rvq: RvqDequant,
    pub upsample: MimiConvT,      // depthwise ×2
    pub transformer: Vec<MimiLayer>,
    pub rope: Rope,               // caller-built (inv_freq path differs per model)
    pub seanet: SeaNet,
    pub cfg: MimiConfig,
}
impl MimiDecoder {
    pub fn decode(&self, codes: &Tensor) -> Tensor; // [1,K,T] int64 → [samples] f32, clamped [-1,1]
    pub fn sample_rate(&self) -> u32;
}

// codec/rvq.rs
pub struct RvqDequant { pub semantic: RvqSplit, pub acoustic: RvqSplit, pub vq_dim: i64, pub n_semantic: i64 }
pub struct RvqSplit  { pub embeds: Vec<Tensor>, pub out_proj: Conv1x1 }
impl RvqDequant { pub fn decode(&self, codes: &Tensor, dt: Kind) -> Tensor; }   // [1,K,T] → [1,hidden,T]
pub fn resolve_codebook(embed_sum: &Tensor, cluster_usage: &Tensor, eps: f64) -> Tensor;

// codec/conv.rs
pub enum MaskFill { NegInf, DtypeMin }
pub fn sliding_window_causal_mask(seq, window, fill: MaskFill, dt: Kind, dev) -> Tensor;
```

### The three byte-identity axes the config parameterizes (NOT unified)

1. **compute dtype** (`MimiConfig::dt`) — dia2 runs the whole codec in **f32** (the checkpoint dtype); csm
   casts the whole codec to **bf16** (the model `torch_dtype`). Drives the RVQ accumulator, the mask tensor,
   and the final clamp; the caller's `load_codec` loads every weight in `dt` (`w.f32` vs `w.cast(_, dt)`).
2. **RoPE inv_freq** — handed in as a fully-built `nn::Rope`. dia2 = `InvFreq::f64_powf_min_max(64, 1.0,
   10000)` (NeMo timescale, f64 host); csm = `InvFreq::f32_tensor_arange(64, 10000)` (geometric f32-tensor).
   These are mathematically equal (`θ_min=1`) but round through different code (~sub-ULP apart) and each was
   pinned to its sidecar — so the `Rope` is built by the caller, never unified in the module.
3. **mask fill** (`MaskFill`) — dia2 fills masked attention slots with `f32::NEG_INFINITY` (f32 mask tensor);
   csm with `torch.finfo(dt).min` via a `where`-fill (`dt` mask tensor). Two spellings, both preserved.

Everything else — causal conv padding, depthwise upsample, per-channel layer-scale, exact-erf gelu, the
SEANet residual stack, the split-RVQ math — is identical and lives once.

## Rewiring

- `dia2.rs`: deleted its local `Conv1x1 / MimiConv / MimiConvT / pad1d / ResBlock / MimiLayer / MimiDecoder /
  rvq_decode / sliding_window_causal_mask` (~218 lines). `load_codec` now maps the dia2 weight names into
  `codec::{MimiConv, MimiConvT, Conv1x1, RvqSplit, RvqDequant, MimiLayer, SeaNet, MimiDecoder}` with
  `MimiConfig::mimi_24khz(Kind::Float, MaskFill::NegInf)` + the dia2 NeMo RoPE. Field `codec:
  codec::MimiDecoder`. The `pad1d_left_right` unit test now calls `codec::pad1d`. Three now-dead `mcfg`
  consts (HEADS / SLIDING_WINDOW / NORM_EPS) removed (the shared `MimiConfig` owns them).
- `csm.rs`: identical treatment with `MimiConfig::mimi_24khz(dt, MaskFill::DtypeMin)` + the geometric RoPE +
  the `codec_model.` weight-name prefix. Also deleted its `sdpa_masked` + `dt_min` helpers (now in the shared
  module) and the same three dead `mcfg` consts.
- The `self.codec.decode(...)` call sites and the `decode_codes` parity seam in both models are unchanged
  (same method name on the shared type).

## LOC reduction

| file | before | after | Δ |
|---|---|---|---|
| `src/dia2.rs` | 1673 | 1470 | **−203** |
| `src/csm.rs`  | 1199 | 988  | **−211** |
| **models combined** | 2872 | 2458 | **−414** |
| new `src/codec/conv.rs` | — | 167 | +167 |
| new `src/codec/mimi.rs` | — | 204 | +204 |
| new `src/codec/rvq.rs`  | — | 69  | +69 |
| `src/codec/mod.rs` (stub→docs+tests) | 3 | 334 | +331 |

The duplicated Mimi decoder (~430 LOC, once per model) collapses to a single ~280-LOC implementation
(`conv` + `rvq` + `mimi` impl bodies); the remaining new lines are module docs and the byte-faithful unit
tests. Net repo LOC is roughly flat, but the **decoder is now single-sourced** — the codec-decode of a Mimi
token stream is written once and config-instanced twice.

## Proofs

### Unit (CPU, gates every `cargo test --lib`) — `codec/mod.rs`
Builds a small but architecturally-complete decoder from fixed random weights and asserts the shared
`MimiDecoder::decode` output **exactly (Δ==0)** reproduces a standalone inline transcription of the
pre-refactor op-sequence — for BOTH regimes:
- `mimi_decode_bit_faithful_dia2_regime` (f32 / NegInf / min-max RoPE) → **Δ==0**
- `mimi_decode_bit_faithful_csm_regime` (bf16 / DtypeMin / arange RoPE) → **Δ==0**
- plus `resolve_codebook_matches_manual`, `mimi_conv_causal_length`, `mimi_convt_upsample_trim`,
  `mask_spellings_agree_on_pattern`.

`cargo test -p waav-infer-backend-torch --lib` → **83 passed; 0 failed** (was 77; +6 codec tests).

### GPU byte-identity #1 — dia2 (`--test cuda_torch_dia2 --features cuda -- --include-ignored`)
```
CODEC parity: tch=36480 samples, ref=36480 | max|Δ|=4.46e-4 | err/sig RMS=3.75e-5/7.20e-2 (0.0521%)   [<0.5% bar]
CUDA CODE byte-identity: 608/608 match; first-div=None
CPU  fp32 byte-identity: 544/544 match; first-div=None
speech-validity: 50Hz energy-envelope correlation tch-vs-sidecar = 1.000
test result: ok. 3 passed; 0 failed
```
The dia2 codec-parity gate runs the FIXED reference code tensor through the shared `codec::MimiDecoder` and
matches the sidecar wav (0.052% rel-RMS). 608/608 sampled codes byte-identical.

### GPU byte-identity #2 — csm (`--test cuda_torch_csm --features cuda -- --include-ignored`)
```
[L2] step0 cb0 argmax=420 (logit 10.1250 vs golden 10.1250)
[L3] LAW PASSED: GREEDY CUDA-bf16 codes BYTE-IDENTICAL to the sidecar golden (125 frames × 32 codebooks)  [= 4000/4000]
[RTF] csm CUDA-bf16: RTF 1.056
test result: ok. 2 passed; 0 failed
```
4000/4000 greedy codes byte-identical; the synthesis path (greedy AR → shared codec decode) produces audio.

## Clippy
`cargo clippy -p waav-infer-backend-torch --all-targets -- -D warnings` → **clean** (0 warnings).

## Files changed (for the coordinator to commit)
**Modified:**
- `crates/waav-infer-backend-torch/src/codec/mod.rs` (stub → module docs + re-exports + byte-faithful tests)
- `crates/waav-infer-backend-torch/src/dia2.rs` (deleted local Mimi; rewired `load_codec`; field type; test)
- `crates/waav-infer-backend-torch/src/csm.rs` (deleted local Mimi + `sdpa_masked`/`dt_min`; rewired `load_codec`)

**Added:**
- `crates/waav-infer-backend-torch/src/codec/conv.rs`
- `crates/waav-infer-backend-torch/src/codec/mimi.rs`
- `crates/waav-infer-backend-torch/src/codec/rvq.rs`

(`lib.rs` — codec mod pre-declared — `ci/heavy_live_tests.sh`, `COMPONENT_CATALOG.md`, other models, and
other crates were NOT touched. The pre-existing untracked `ci/phase_c_model_sweep.sh` + `docs/` are unrelated
and were left as-is.)
