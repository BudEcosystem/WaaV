# B31 — WaaV Infer modularization Phase 2: the shared TRANSFORMER BACKBONE

**Goal (memory `waav-infer-modularize-reuse`; spec `COMPONENT_CATALOG.md`):** after Phase 1 (B30) landed the
`nn/` primitives, extract the shared transformer **backbone** — the decoder `Layer` struct (duplicated 5/5)
and the GQA-transformer decode loop (repeated across all 7 tch models) — into ONE config-parameterized
implementation, and rewire every model's decoder/backbone to it, **bit-faithfully**.

**Status: COMPLETE.** All 7 models rewired; local `Layer`/loop copies DELETED; 4 mandated GPU spot-checks
byte-identical (+3 bonus models also byte-identical); lib 77 green; clippy clean. Worktree commit below.

---

## What was built (in `crates/waav-infer-backend-torch/src/nn/`)

Four new modules composing the Phase-1 primitives (`RmsNorm`/`LayerNorm`/`Linear`/`Rope`/`KvCache`/`sdpa*`):

### `nn::Mlp` (`mlp.rs`, 267 LOC)
The position-wise feed-forward, parameterized:
```rust
enum Act { Silu, Gelu, Relu }
enum GateUp { Fused { gate_up: Linear, inter: i64 }, Separate { gate: Linear, up: Linear } }
enum MlpKind { Gated { gate_up: GateUp, act: Act }, Ungated { up: Linear, act: Act } }
struct Mlp { kind: MlpKind, down: Linear }
  Mlp::swiglu_fused(gate_up, down, inter, act)   // voxtral/ark/cosy/vibe
  Mlp::swiglu_separate(gate, up, down, act)      // csm
  Mlp::ungated(fc1, fc2, act)                    // cohere (ReLU)
```

### `nn::Attention` + `nn::CrossAttention` (`self_attention.rs`, 473 LOC)
The COMPOSED self-attention — the heart of the dedup. Runs the shared skeleton
`project → (q/k-norm) → head-transpose → RoPE → cache.append → SDPA → merge → o_proj`, with each step's exact
spelling chosen by config (every axis a per-model byte-identity scar):
```rust
enum Proj      { Fused { qkv }, Separate { q, k, v } }
enum ProjPrec  { Native, F32Sandwich { compute_dt } }      // dia2: layer(x.float())→cast dt
enum RopeApply { Start, Positions, None }                  // apply_start / apply_positions / cohere learned-pos
enum CacheRead { View, ViewContiguous, Contiguous, FullMasked }
enum Kernel    { ManualGqa, ManualMha, FusedCausalGqa, FusedCausalMaybeGqa, FusedMaskedGqa }
struct Attention {
    proj: Proj, o: Linear, prec: ProjPrec,
    q_norm: Option<RmsNorm>, k_norm: Option<RmsNorm>,      // dia2 per-head q/k RMSNorm
    rope_apply: RopeApply, cache_read: CacheRead, kernel: Kernel,
    scale: f64, n_q, n_kv, head_dim,
}
struct CrossAttention { q, o, scale, n_heads, head_dim }   // cohere AED (constant encoder K/V, no cache/rope)
```
Returns the post-`o_proj` output (the caller adds the residual) — matching every model's `x + o(ctx)`.

### `nn::TransformerLayer` (`layer.rs`, 479 LOC)
The pre-norm decoder layer — the 5/5-duplicated `Layer` struct, ONE impl:
```text
x = x + Attention(input_norm(x))                    // self-attention
[ x = x + CrossAttention(post_attn_norm(x)) ]        // cohere only (config)
x = x + Mlp( [ada_scale ⊙] mlp_norm(x) )             // FFN; ada_scale = voxtral only (config)
```
```rust
enum Norm { Rms(RmsNorm), Layer(LayerNorm) }          // cohere = Layer; all others = Rms
enum Ffn  { Shared(Mlp), Inline(Box<dyn Fn(&Tensor)->Tensor + Send>) }  // dia2 = inline f32-sandwich MLP
struct TransformerLayer { input_norm, attn, cross: Option<CrossAttn>, mlp_norm, ada_scale: Option<Tensor>, mlp: Ffn }
  ::new(input_norm, attn, mlp_norm, mlp)              // the 5/5 GQA case
  ::with_inline_mlp(input_norm, attn, mlp_norm, mlp)  // dia2
  .with_cross(CrossAttn)                              // cohere
  .with_ada_scale(Tensor)                             // voxtral
```

### `nn::Backbone` (`backbone.rs`, 232 LOC)
The stacked decoder + the AR decode loop — the 5/5-duplicated `for layer in &self.layers { h = layer.forward(…) }; final_norm(h)`:
```rust
struct LmHead  { embed: Tensor, untied: Option<Linear> }   // tied (xᵀ) or untied
struct Backbone { layers: Vec<TransformerLayer>, final_norm: Norm, head: Option<LmHead> }
  forward(embeds, rope, caches, pos, positions, mask, is_prefill, cross_kv) -> normed_hidden
  lm_logits / lm_logits_f32_tied / embed_ids            // simple-embed models
```
`pos` feeds `apply_start`, `positions` feeds `apply_positions` (each layer uses whichever its `rope_apply`
selects); `cross_kv` is the cohere per-layer encoder K/V.

---

## Per-model configs (how each decoder composes the shared layer)

| model | norm | proj | prec | q/k-norm | rope | cache | kernel | mlp | layer extra |
|---|---|---|---|---|---|---|---|---|---|
| **voxtral** (Mistral 26L) | RmsNorm(Mul) | Fused | Native | – | Start | View | ManualGqa | SwiGLU-fused | **ada_scale** |
| **ark** (Qwen2 24L) | RmsNorm(Mul) | Fused | Native | – | Start | View | ManualGqa | SwiGLU-fused | – |
| **cohere** (AED 8L) | **LayerNorm** | Separate | Native | – | **None** | View | ManualMha | **ReLU** | **CrossAttn** |
| **cosyvoice3** (Qwen2 24L) | RmsNorm(Mul) | Separate | Native | – | Start | **ViewContiguous** | FusedCausalGqa | SwiGLU-fused | – |
| **vibevoice** (Qwen2.5 28L) | RmsNorm(Pow) | Separate | Native | – | Start | Contiguous | FusedCausalGqa | SwiGLU-fused | – |
| **csm** (Llama 16L bb + 4L depth) | RmsNorm(Pow,wf) | Separate | Native | – | **Positions** | Contiguous | FusedCausalMaybeGqa | SwiGLU-**sep** | – |
| **dia2** (bbone 28L) | RmsNorm(Fused) | Separate | **F32Sandwich** | **✓** | Positions | **FullMasked** | FusedMaskedGqa | **inline f32** | – |

- **csm** is the clean proof that ONE `TransformerLayer` serves BOTH the 16-layer backbone AND the 4-layer
  depth-decoder with only different dims (`build_llama_layer(…, n_q, n_kv, head_dim)`).
- **cohere** and **dia2** compose the SAME `TransformerLayer` via config (cross-attn insert / F32-sandwich +
  q/k-norm + inline-MLP), not a forked layer type — exactly as the spec required.
- **Scope call (deliberate):** dia2's **Depformer** keeps its specialized `DepLayer` inline. Its per-stage
  schedule-group weight selection (`in_proj[weight_idx]`/`out_proj[weight_idx]`, 5 groups dispatched by
  `WEIGHTS_SCHEDULE[stage]`) is a model-specific dispatch, not a transformer-config axis; it already reuses the
  shared `nn::` primitives (`sdpa`/`Rope`/`KvCache`/`RmsNorm`/`Linear`). The dia2 **backbone** (the bigger,
  standard 28-layer part) IS on `nn::Backbone`/`TransformerLayer`. This kept the dia2 byte-identity (the
  hardest gate) safe while still deduping the standard part. The Mimi codec / CFM / audio towers / depth-
  projector stay inline (Phase 3, per the spec).

---

## LOC reduction (before → after)

Measured against the Phase-1 baseline (commit `a6dd10d`):

| model | git +ins / −del |
|---|---|
| voxtral | +45 / −67 |
| ark | +45 / −67 |
| cohere | +65 / −69 |
| cosyvoice3 | +47 / −76 |
| csm | +58 / −80 |
| dia2 | +61 / −66 |
| vibevoice | +66 / −65 |
| **7 models total** | **+387 / −490 (net −103)** |

**490 lines of duplicated decoder layer/loop LOGIC were DELETED from the 7 models**, replaced by 387 lines of
**declarative per-model config** (the `build_*_layer` field assignments + delegated `Backbone::forward` calls).
The shared forward math (the transformer layer + the AR decode loop) now lives **once** in `nn::` (~1450 LOC of
which roughly half is per-component unit tests + doc-comments, written once for all 7 models) instead of being
copy-pasted 7×. The byte-identity fixes (fused-vs-decomposed RMSNorm, RoPE inv_freq rounding, the flash-vs-math
SDPA selector, the cache read-back layout) are now STRUCTURAL — change them once, every model inherits it.

---

## Bit-faithful proofs

### Unit tests (`cargo test -p waav-infer-backend-torch --lib`) — **77 passed, 0 failed** (was 64 at baseline; +13)
New composed-block tests, each diffing the shared op against a hand reconstruction (maxΔ = 0.0 in f32):
- `nn::mlp` — swiglu_fused / swiglu_separate / fused==separate / ungated-ReLU (4)
- `nn::self_attention` — ManualGqa attention / FusedCausalGqa attention / CrossAttention all match the hand op (3)
- `nn::layer` — GQA layer skeleton / ada-scale insert / **cohere cross-attn AED skeleton** / **dia2
  F32-sandwich+q/k-norm+inline-MLP layer** all match explicit (4)
- `nn::backbone` — stacked forward == hand loop / tied+untied lm_head (2)
All 34 Phase-1 primitive tests still green.

### Clippy (`cargo clippy -p waav-infer-backend-torch --all-targets -- -D warnings`) — **clean.**

### GPU byte-identity spot-checks (run from the worktree, one at a time, `free -g` first; GB10 121GB unified)
The mandated 4, each re-run **on the refactored worktree binary** (the earlier accidental main-repo run was
discarded once caught):

| gate | result |
|---|---|
| **voxtral** `cuda_torch_voxtral_vs_ort` (greedy) | **100.0% EXACT char-identity** vs ORT-CPU reference |
| **csm** `cuda_csm_codes_byte_identical_to_sidecar` (greedy) | **GREEDY codes BYTE-IDENTICAL** to the CUDA-bf16 sidecar golden — **125 frames × 32 codebooks (= 4000)**; step0 cb0 argmax logit 10.1250 == golden |
| **dia2** `cuda_bf16_codes_byte_identical` (sampled, seed 0) | **608/608 CUDA bf16 codes byte-identical**, first-div=None — exercises the F32-sandwich + q/k-norm + full-masked path |
| **cohere** `cuda_torch_cohere_vs_ort` (AED/cross-attn) | **100.0% char similarity**; identical transcript ("Hello, world. This is WAV, infer …") — exercises the cross-attn/AED path |

Bonus (the other 3 rewired models, also re-run on the worktree binary, all byte-identical):
- **ark** `cuda_torch_ark_byte_identical` — 100.0% EXACT char-identity vs sidecar golden.
- **cosyvoice3** `cuda_torch_cosyvoice3` — AR speech-token sequence BYTE-IDENTICAL (123 tokens), first-div None.
- **vibevoice** `cuda_torch_vibevoice` — 28-layer Qwen2.5 backbone BYTE-IDENTICAL on golden embeds (L3) + L1/L4.

**No model's output changed.** Every model is byte-for-byte the same as before the refactor.

---

## Touched (ONLY the backend-torch crate)
- NEW: `nn/mlp.rs`, `nn/self_attention.rs`, `nn/layer.rs`, `nn/backbone.rs`.
- EDIT: `nn/mod.rs` (register + re-export + catalog doc-comment), and the 7 model files (`voxtral.rs`, `ark.rs`,
  `cohere.rs`, `cosyvoice3.rs`, `csm.rs`, `dia2.rs`, `vibevoice.rs`) — decoder `Layer`/loop copies deleted, a
  `build_*_layer` config fn + an `nn::Backbone`-backed decoder substituted; vibevoice's debug localizer helpers
  rewired to the new `nn::TransformerLayer` accessors.
- DOC: this report + `COMPONENT_CATALOG.md` (module tree, dedupe map, extraction plan marked Phase-2 done).
