# B12 — Candle Cohere-Transcribe on CUDA (backlog item 1, arm 2 FIXED)

**Question answered:** Does candle cohere run on CUDA with the correct transcript (item-1 arm-2 fixed)?
**Answer: YES.** The candle CUDA cohere transcript is **100.0% char-identical** to the ORT-CPU
reference, at **RTF 0.09** (vs ORT-CPU RTF 0.13) on a 12.05 s clip. Reproducible across runs.

```
═════════════════ Cohere CUDA (candle decoder) vs CPU (ORT) ═════════════════
audio: 12.05s   candle decoder device: cuda
ORT  cpu : load 5393 ms | infer 1528 ms | RTF 0.13
           "Hello, world. This is WAV, infer a portable voice inference engine, running live on the GB10 Grace BL, a CKWELL."
CAND cuda: load 2804 ms | infer 1050 ms | RTF 0.09
           "Hello, world. This is WAV, infer a portable voice inference engine, running live on the GB10 Grace BL, a CKWELL."
de-punctuated char similarity: 100.0%
```

(The transcript itself — "Grace BL, a CKWELL" instead of "Grace Blackwell", "WAV" for "WaaV" — is the
*model's* own output and is identical on both runtimes; this gate proves the candle decoder reproduces
the trusted reference, not that the model is perfect. Voxtral hit 98.9%; cohere hits 100%.)

---

## What the failure actually was (empirically isolated, not assumed)

The prompt said cohere "fails on ORT-CUDA with cuDNN 'No execution plans support the graph'." I probed
the two graphs separately on ORT-CUDA before writing any code:

- **Encoder (FastConformer / `parakeet_encoder`, 48 layers, rel-pos)** — uses ORT's `MultiHeadAttention`
  op. **Runs CORRECTLY on ORT-CUDA** (produced `last_hidden_state[1,16,1024]`).
- **Decoder (8-layer transformer AED)** — its self-attention is exported as `GroupQueryAttention`
  carrying an `attention_bias`. **FAILS on ORT-CUDA at run time:**
  > `Non-zero status code ... GroupQueryAttention node ... Status Message: attention_bias is not
  > supported in GroupQueryAttention cuda kernel.`

This is the **identical** failure that put Voxtral on candle. So **only the decoder needs porting** —
reimplementing the 48-layer rel-pos Conformer (149 Conv ops, depthwise conv, batchnorm subsampling,
Transformer-XL relative attention) would be enormous, bug-prone, and pointless because it is not the
part that fails.

### Decision: hybrid arm (the minimal correct fix)

`CandleCohere` keeps the proven FastConformer **encoder + `nemo128` mel preprocessor on ORT-CUDA** and
reimplements **only the failing decoder in candle**. Because the frontend is the *same ORT graphs* in
both the reference and the new arm, the gate isolates exactly the candle decoder — which is why it lands
at a clean 100%. (ORT becomes a *runtime* dep of the candle crate, promoted from dev-dep; legal — this
is the one `-backend-*` crate where C/C++ in the graph is allowed, INFER_SPEC §17.1.)

---

## The decoder, fully reverse-engineered from the ONNX graph

Traced from `decoder_model_merged_fp16.onnx` (initializers + the `If` cross-attn-KV subgraph). It is a
**vanilla pre-LN transformer AED**, NOT Mistral-style:

- **Embedding:** `embed_tokens[id] + pos_emb[pos]` (both `[1024,1024]`, baked absolute positions, **no
  √d scaling** — a plain `Add`), then ONE shared `embedding_layernorm`.
- **8 layers**, hidden 1024, 8 heads × 128 (kv==q ⇒ plain MHA), scale `1/√128`, **no RoPE** (`do_rotary=0`):
  - pre-LN `input_ln` → causal self-attn → `o_proj`(+b) → residual
  - pre-LN `post_attn_ln` → cross-attn over encoder hidden states → `o_proj`(+b) → residual
  - pre-LN `final_ln` → MLP `fc1`→**ReLU**→`fc2` (inter 4096, +b) → residual
- **final** `final_norm` LayerNorm → `lm_head` (**untied**, `[16384,1024]`+bias). Greedy, EOS=3,
  START=13764, 24-repeat stuck-guard (mirrors the ORT arm).
- Every norm is **LayerNorm (weight + bias)**, not RMSNorm.
- ONNX `MatMul` weights are `[in,out]`; transposed to `[out,in]` at extraction for candle's `x@Wᵀ`.

The export's layer naming is offset (`layers.9.embedding_layernorm` = the pre-layer-0 embed LN;
`layers.8.final_norm` = the post-layer-7 final LN; the 8 real transformer layers are 0–7) — handled at
extraction.

---

## Voxtral perf patterns reused from the start (no re-introduced bugs)

- **Zero-copy `Linear`** — 2-D `[rows,in]@Wᵀ` gemm, `wᵀ` consumed as a cublas `OP_T` view. NEVER
  `broadcast_matmul` (whose per-call full-weight `.contiguous()` copy was 85% of Voxtral's decode time).
- **Device-resident ring-KV** — one candle-nn `KvCache` per layer, `slice_set` append per step, no
  per-step `Tensor::cat`, no O(n²) realloc.
- **Contiguous-view Kᵀ** in SDPA (no `ucopy` of Kᵀ); **softmax in f32**.
- **Cross-attn K/V projected once** per utterance from the encoder hidden states (the `present.encoder.*`
  the ONNX graph caches), reused every decode step.
- **Mask only on multi-row** — the single-row decode step's causal mask is all-zeros, so it's skipped
  (`None`).
- f16 weights on CUDA / f32 on CPU (Voxtral precision policy).

The encoder being a one-shot ORT call + a lean candle decode loop is why RTF (0.09) beats the all-ORT-CPU
path (0.13) despite the candle decoder going through host materialization at the ORT↔candle seam.

---

## Deliverables

1. **`crates/waav-infer-backend-candle/src/cohere.rs`** — `CandleCohere` impl
   `waav_infer_core::model::SttModel` (`load(dir, CandleDevice)` + `transcribe(&[f32]) -> String`),
   decoder on CUDA. Registered in `lib.rs` (`pub use cohere::{CandleCohere, CohereCandleError}`).
2. **`crates/waav-infer-backend-candle/tests/cuda_cohere_vs_ort.rs`** — `#[ignore]`'d live-GPU gate,
   mirrors `cuda_vs_ort.rs`: candle-CUDA vs ORT-CPU `CohereAsr` on `assets/kokoro_m1_sample.wav`,
   asserts ≥92% de-punct char similarity (**got 100.0%**). Registered in `ci/heavy_live_tests.sh`
   (verified it passes via the runner's exact `--exact --include-ignored` form, default `cuda` feature).
3. **RTF 0.09** (<1 target met; ORT-CPU reference 0.13).
4. **`cargo clippy -p waav-infer-backend-candle --all-targets --features cuda -- -D warnings` clean.**
   2 new CPU unit tests (`causal_mask_basic`, `layernorm_matches_manual`); all 9 lib tests green.
   CPU-only (`--no-default-features`) build still compiles.

**Files touched (only the candle crate + heavy_live_tests.sh, as instructed):**
- `crates/waav-infer-backend-candle/src/cohere.rs` (new)
- `crates/waav-infer-backend-candle/src/lib.rs` (module registration)
- `crates/waav-infer-backend-candle/Cargo.toml` (ort/api dev-deps → runtime deps for the hybrid encoder)
- `crates/waav-infer-backend-candle/tests/cuda_cohere_vs_ort.rs` (new)
- `ci/heavy_live_tests.sh` (gate registration)
- **Not committed** (per instructions).

**Model artifacts** (built, outside the repo): `~/.cache/waav-models/cohere-transcribe-candle/` —
`decoder.safetensors` (645 MB, extracted from the fp16 ONNX decoder initializers → f32, transposed),
`tokenizer.json`, `nemo128.onnx`, `onnx/encoder_model_fp16.onnx{,_data,_data_1}` (symlinked to the HF
ONNX-community snapshot). No HF download was needed — the decoder weights came straight out of the ONNX
export, guaranteeing the candle arm uses the *exact same weights* as the ORT reference.

---

## What's done vs gaps (honest)

**Done / proven:** correct CUDA transcript (100% match), RTF < 1, clippy clean, gate wired, no OOM
landmine (memory flat before/after the gate), CPU + CUDA builds both green.

**Scoped gaps (not regressions):**
- The arm is **hybrid** (ORT encoder + candle decoder), not all-candle. This is the deliberate,
  correct fix: the encoder runs fine on ORT-CUDA, and only the decoder's GQA failed. A full-candle
  FastConformer port is a separable, large future task with **zero accuracy or item-1 benefit**.
- The candle decoder is **batch-1 greedy**, matching the ORT cohere arm exactly (the reference is also
  batch-1 greedy). No beam search (config declares beam_size 1 anyway).
- The model dir's encoder is **symlinked** into the HF cache; both this arm and the ORT reference read
  the same snapshot, so they share that dependency. `decoder.safetensors` is a real self-contained copy.
- A standalone extraction script was used once to produce `decoder.safetensors` (kept out of the repo,
  temp files cleaned). Re-deriving it is a ~30-line onnx→safetensors transpose if ever needed again.
- The candle crate's sibling **Voxtral** CUDA gate (`cuda_voxtral.rs`) is *also* not in
  `heavy_live_tests.sh` (pre-existing); I added only the cohere gate as instructed.
