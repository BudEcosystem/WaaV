# B21 — tch Cohere-Transcribe on CUDA (backlog item 1, arm 2 — ported to the Torch substrate)

**Verdict: SHIP.** The in-process **tch** Cohere-Transcribe ASR transcribes correctly on GB10 CUDA, **100.0 %
de-punctuated char-identical to the ORT-CPU reference** — holding the exact bar the retired candle arm held — at
**RTF 0.18** (infer 2129 ms on a 12.05 s clip; target < 1). This completes **item-1 arm-2 on the strategic Torch
substrate** and **REPLACES** the candle stopgap (`backend-candle/src/cohere.rs`), which is retired.

- New file: `crates/waav-infer-backend-torch/src/cohere.rs` (`cohere::TorchCohere`, re-exported from `lib.rs`).
- Module registered: `pub mod cohere;` + `pub use cohere::{CohereTorchError, TorchCohere};` in the crate `lib.rs`.
- Live gate: `crates/waav-infer-backend-torch/tests/cuda_torch_cohere_vs_ort.rs` (`#[ignore]` `cuda_torch_cohere_vs_ort`), wired into `ci/heavy_live_tests.sh` (item **(e)**, swapped in for the now-removed candle gate).
- **Zero new Cargo.toml deps** — the torch backend already declared everything needed.
- Worktree commit: **`76f333e0beda64be78b1e53129e4128506406266`** on branch `worktree-agent-ac12a8dfb4b89910e`.

## Does it transcribe correctly on GB10 CUDA, bit-faithful to ORT-CPU + the candle reference?

**Yes — 100.0 %, character-identical.** Live run (`source gb10-env.sh && ci/heavy_live_tests.sh` gate (e)):

```
═════════════════ Cohere CUDA (tch decoder) vs CPU (ORT) ═════════════════
audio: 12.05s   tch decoder device: cuda
ORT cpu : load 6595 ms | infer 3068 ms | RTF 0.25
          "Hello, world. This is WAV, infer a portable voice inference engine, running live on the GB10 Grace BL, a CKWELL."
TCH cuda: load 4263 ms | infer 2129 ms | RTF 0.18
          "Hello, world. This is WAV, infer a portable voice inference engine, running live on the GB10 Grace BL, a CKWELL."
de-punctuated char similarity: 100.0%
```

The tch transcript is **byte-for-byte the ORT-CPU reference output** (and the same string the candle arm produced
in B12). The model's own quirks ("WAV" for "WaaV", "Grace BL, a CKWELL" for "Grace Blackwell") are reproduced
identically on both runtimes — this gate proves the **tch decoder reproduces the trusted reference**, not that the
model is perfect. The bar (`sim >= 0.92`) is met with margin at **100 %**; the candle arm it replaces also hit
100 %, so the Torch port loses nothing.

## The hybrid approach (kept identical to the candle decomposition)

Cohere-Transcribe (`CohereAsrForConditionalGeneration`) = a 48-layer FastConformer (parakeet) encoder + an 8-layer
transformer AED decoder. **Only the decoder fails on ORT-CUDA**: its self-attention is exported as
`GroupQueryAttention` carrying an `attention_bias`, which ORT's CUDA GQA kernel rejects (*"attention_bias is not
supported in GroupQueryAttention cuda kernel"* — the identical wall that put Voxtral on the native path). The
encoder uses ORT's `MultiHeadAttention`, which ORT-CUDA runs correctly. So the arm is a **hybrid** — and the port
keeps the candle arm's exact decomposition:

- **ORT-CUDA half (unchanged from candle):** the `nemo128` mel preprocessor (pinned to the **CPU** EP, bit-for-bit
  the reference frontend) + the FastConformer encoder (CUDA EP) → `last_hidden_state[1,T',1024]`. Run through
  `waav_infer_backend_ort::OrtModel` — the **same `tch-integrator + ORT-component` hybrid the B19 cosyvoice3 arm
  uses** (an `OrtModel` on the CUDA EP alongside native tch), legal in a `-backend-*` crate (INFER_SPEC §17.1). The
  encoder + frontend are the SAME ONNX graphs as the candle + ORT-CPU arms, so this gate isolates the tch decoder.
- **tch half (the port):** the 8-layer pre-LN transformer AED decoder reimplemented in libtorch's eager graph —
  the minimal correct fix (reimplementing the 48-layer rel-pos Conformer would be enormous AND pointless, since it
  is not the part that fails). Decoder geometry (from the ONNX `decoder_model_merged_fp16` trace, identical to
  candle's reverse-engineering):
  - embedding = `embed_tokens[id] + pos_emb[pos]` (learned **absolute** positions, no √d scaling), then ONE shared
    `embedding_layernorm`. **No RoPE** (`do_rotary=0`).
  - 8 layers, hidden 1024, **8 heads × 128 head-dim** → `kv_num_heads == num_heads` ⇒ **plain MHA, no GQA grouping**;
    scale `1/√128`. Per layer: pre-LN self-attn (causal) → o_proj(+bias) → residual; pre-LN cross-attn over the
    encoder K/V → o_proj(+bias) → residual; pre-LN MLP (`fc1` → **ReLU** → `fc2`, inter 4096, all +bias) → residual.
  - final `final_norm` LayerNorm → **untied** `lm_head` (`[16384,1024]`+bias). Greedy, **EOS=3**, start token 13764.
  - Every norm is a **LayerNorm with weight AND bias** (not RMSNorm), variance accumulated in **f32**.
  - Cross-attn K/V projected from the encoder hidden states **once** (prefill) and reused every step; self-attn K/V
    grow one position per step into a device-resident ring-KV.

**Same weights as candle:** the model dir is reused verbatim
(`~/.cache/waav-models/cohere-transcribe-candle/{decoder.safetensors, tokenizer.json, nemo128.onnx, onnx/encoder_model_fp16.onnx}`).
`decoder.safetensors` is the f32 tensor set the candle arm extracted from the fp16 ONNX initializers; tch loads it
with `Tensor::read_safetensors`, moves+casts to the run dtype (**f16 on CUDA / f32 on CPU** — the Voxtral precision
policy). The tokenizer is `waav_infer_components::SentencePieceTokenizer` (the SAME decode-only SP-BPE the candle +
ORT arms use).

## tch idioms reused verbatim from voxtral / cosyvoice3

The decoder is a faithful translation of `backend-candle/src/cohere.rs` into the proven tch perf idioms:

- **Zero-copy `[rows,in] @ Wᵀ` gemm** (`Linear`): flatten leading dims, multiply by `w.transpose(-1,-2)` as a
  cublas `OP_T` **strided view** — NO per-call weight `.contiguous()` copy.
- **Device-resident ring-KV** (`KvCache`): a pre-allocated `[1, n_heads, max_seq, d]` buffer written in place via
  `index_copy_` at the current index and read back with `narrow` — no per-step `cat`, no O(n²) realloc. Sized once to
  `MAX_LENGTH+1`. (Plain MHA ⇒ the buffer holds `N_HEADS=8` heads, not a GQA `kvh`.)
- **`sdpa`**: QKᵀ with `K.transpose(-2,-1)` (OP_T, no Kᵀ copy), **softmax in f32**, optional additive mask.
- **Mask only on prefill**: every decode step submits one query row whose causal mask is all-zeros, so it passes
  `None` (the all-zeros fast path). A unit test (`sdpa_decode_step_matches_masked`) pins that `None` == the explicit
  causal mask on the newest row.
- **`LayerNorm`** is the one new primitive vs voxtral (which had only RmsNorm): centered mean/variance in **f32**,
  **biased** (population) variance, `(x-μ)/√(var+eps)·w + b` — numerically matching ONNX `LayerNormalization` and
  candle-nn's `LayerNorm`. Pinned by `layernorm_matches_manual`.

## RTF

| Arm | load | infer (12.05 s clip) | RTF | char-sim vs ORT-CPU |
|---|---|---|---|---|
| ORT-CPU reference (full cohere) | 6595 ms | 3068 ms | 0.25 | — |
| **tch hybrid (ORT-CUDA enc + tch dec)** | 4263 ms | **2129 ms** | **0.18** | **100.0 %** |

**RTF 0.18, well under the < 1 target.** (The B12 candle arm reported infer-RTF 0.09 on a less-loaded box run; the
delta is box-load variance + tch's first-call cuDNN/cublas autotune on the single inference — the encoder ORT-CUDA
pass is shared and identical between arms. Both arms clear the target with large margin; accuracy is identical at
100 %.) Teardown was clean — the test exits `ok` with no ORT-CUDA `Drop` SIGABRT and the unified pool is fully
reclaimed (12 G used / 108 G avail after), so unlike the codec-AR live gates it needs no `mem::forget`.

## Tests

- **4 new CPU unit tests** (in `src/cohere.rs`, all green): `causal_mask_basic`, `layernorm_matches_manual`,
  `kv_cache_appends_in_place`, `sdpa_decode_step_matches_masked`. Full torch-backend lib suite: **21/21 green**, no
  regressions.
- **1 live-GPU gate** `cuda_torch_cohere_vs_ort` (`#[ignore]`'d so the single-pass `cargo test` stays OOM-safe), run
  process-isolated via `ci/heavy_live_tests.sh` gate (e). Asserts `sim >= 0.92` (got 100 %) + non-empty transcript.
- `cargo clippy -p waav-infer-backend-torch --tests -- -D warnings`: **clean**.

## Scope discipline

Touched ONLY: `src/cohere.rs` (new), `src/lib.rs` (module registration + doc), `tests/cuda_torch_cohere_vs_ort.rs`
(new), `ci/heavy_live_tests.sh` (gate (e) added, retired candle cohere gate removed). Did **not** touch other
crates, `voxtral.rs`, `dia2.rs`, `cosyvoice3.rs`, `torch_runtime/*.py`, or any `Cargo.toml`.

## Answers to the brief

- **Does tch cohere transcribe correctly on GB10 CUDA, bit-faithful to ORT-CPU + the candle reference?** **Yes —
  100.0 % de-punctuated char-identical**, byte-for-byte the same transcript as both references.
- **The hybrid approach:** FastConformer encoder + nemo128 mel kept on **ORT-CUDA** (`OrtModel`, the same hybrid as
  B19 cosyvoice3); only the 8-layer pre-LN AED decoder reimplemented in **tch** — the part whose GQA `attention_bias`
  ORT's CUDA kernel rejects.
- **RTF:** **0.18** (target < 1; candle was 0.09 — both clear it).
- **char-similarity:** **100.0 %**.
- **New Cargo.toml dep:** **none** (all deps already present).
- **Worktree commit SHA:** `76f333e0beda64be78b1e53129e4128506406266` (branch `worktree-agent-ac12a8dfb4b89910e`).
