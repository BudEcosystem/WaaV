# Item 1 — voxtral-q4f16 + cohere-fp16 ON the ORT CUDA EP (the ONNX path, byte-faithful)

**Date:** 2026-06-24 · **Box:** GB10 (sm_121, 121 GB unified) · **ORT dylib:** libonnxruntime.so.1.27.0 (CUDA+CPU)

## RESULT — do both arms now run on the ORT CUDA EP?

**YES — both, via ONNX graph surgery (option a), byte-faithful, served through the registry.**

| Arm | Was | Now (ORT CUDA EP, `_nobias` graph) | Faithfulness vs ORT-CPU reference |
|---|---|---|---|
| **voxtral q4f16** | GQA `attention_bias` rejection at first forward | **RUNS on CUDA** | transcript **BYTE-IDENTICAL** (`==`) |
| **cohere fp16**  | GQA `attention_bias` rejection at first forward | **RUNS on CUDA** | transcript **CHAR-IDENTICAL** (`==`, 100%) |

Chosen arm: **(a) ONNX graph surgery** — the cleanest. Not (b) re-export (unnecessary: the fix is a 2-line, numerics-preserving removal), not (c) infeasible. The tch-bf16 arm remains valid and is still the only path for the bf16-faithful homophone cases where the **q4 reference itself** drifts off the bf16 model (documented in `cuda_torch_voxtral_vs_ort`).

## ROOT CAUSE (corrected from the B59 "infeasible" conclusion)

The B59 report concluded the ORT-CUDA EP could not run these (GQA CUDA kernel rejects `attention_bias`; TRT EP partitions the contrib op back to the same kernel). That diagnosis of the *failure* was right; the conclusion that it was *unfixable on the ONNX path* was **wrong**.

Three facts unlock the fix:

1. **The rejected `attention_bias` is a no-op in these models' only call pattern.** GQA input #10 is `(1 − attention_mask) · −65504` expanded to 4D — a *padding* mask. Both drivers (`VoxtralRealtime::transcribe`, `CohereAsr::transcribe`) **always** feed `attention_mask = all-ones** (single stream, left-to-right, growing KV cache; no padding). So the additive bias is **identically zero** at every step. GQA still does its causal + sliding-window masking internally via `seqlens_k` / `total_seq_len` / `local_window_size`.

2. **The CUDA GQA kernel rejects the *presence of the slot*, not a nonzero value.** Clearing input #10 to `""` is **not** enough — the kernel still rejects (arity > 10). The slot must be **absent**: truncate the trailing empty optionals so the node has only its real inputs. (Proven empirically: arity-9/10 no-bias GQA RUNS on CUDA at every optimization level and every shape, incl. the empty-KV prefill.)

3. **BOTH graphs carry the offending GQA, not just the decoder.** The voxtral **audio_encoder** has 32 GQA-with-`attention_bias` nodes (sliding window 750) and runs *first* — it is what the B59 control actually trips on (the error names `/model/layers.0/...` which exists in both graphs). The decoder has 26. Cohere: decoder 8 GQA (`num_heads==kv_num_heads==8`, `do_rotary=0`); its encoder already uses `MultiHeadAttention` with `add_qk` (CUDA-OK) so only the decoder needed surgery.

## THE SURGERY (`eval/onnx_drop_gqa_bias.py`)

Per `com.microsoft.GroupQueryAttention` node: clear the `attention_bias` input (#10), then **truncate trailing empty optionals** so the slot is absent (preserving a populated `position_ids` at #9 — cohere/voxtral-encoder have it). Then **dead-code-eliminate** the now-orphaned `(1−mask)·−65504` Expand/Sub/Mul/Cast subgraph. DCE traverses `If`/`Loop` subgraph-captured tensors (the cohere decoder's `If` cross-attn-KV bodies capture `encoder_hidden_states` projections — a naïve DCE would wrongly prune them and break topology). External data is **re-packed densely by hand** (the stock onnx writer honored stale offsets and emitted a ~5× padded blob: 12 GB for 2 GB of real q4 weights — now exactly the original byte count).

Numerics untouched: removing a provably-zero addend, and re-packing the *same bytes* contiguously. MatMulNBits (183) + UINT8 quant weights (366) preserved — still genuinely q4f16, not dequantized.

## BYTE-FAITHFULNESS PROOF

**(1) Surgery is bit-exact — proven on the ORT CPU EP** (where the stock graph runs), stock vs `_nobias`, identical inputs, all-ones mask, prefill + incremental decode with growing KV:

- voxtral decoder: logits **bit-identical** + present-KV **bit-identical**, maxΔ = 0.0
- cohere decoder: logits **bit-identical** + decoder-KV + encoder-KV **bit-identical**, maxΔ = 0.0

This isolates the surgery's correctness from any platform delta.

**(2) End-to-end on the ORT CUDA EP** (real transcribe, kokoro clip), `_nobias` graphs vs the stock ORT-CPU reference:

- voxtral: `"Hello world! This is W.A.V. Infer, a portable voice inference engine, running live on the GB10 Grace BL, a C-K-W-E-L-L."` — **byte-identical** (`cuda_txt == cpu_txt`)
- cohere: `"Hello, world. This is WAV, infer a portable voice inference engine, running live on the GB10 Grace BL, a CKWELL."` — **char-identical** (100%)

EP confirmed `cuda` (no CPU fallback) for encoder + decoder in every case.

**(3) Stock graph still fails** (`cuda_ort_gqa_attention_bias_control`) — proving the *surgery* is the fix, not an ORT/dylib change.

## VERIFICATION (all green on GB10)

- `cargo test -p waav-infer-backend-ort` → **27 + 1 passed**, clippy clean (no lib change — the ORT crate is byte-for-byte unchanged; the GQA CUDA kernel rejects the slot's presence, so no Rust fix was needed).
- `cuda_ort_voxtral_nobias_runs` (new) → voxtral q4f16 enc+dec on CUDA, **byte-identical** to CPU. ✓
- `cuda_ort_cohere_nobias_runs` (new) → cohere fp16 on CUDA, **char-identical** to CPU. ✓
- `item1_ort_cuda_registry` (new, server) → `load_model_at(dir, Cuda)` via `waav.json` → both serve on CUDA, transcripts identical to CPU. ✓ (the LAW)
- `cuda_ort_gqa_attention_bias_control` → stock graph still rejects (baseline intact). ✓

(All CUDA tests print `corrupted double-linked list` at process *exit* — the known ORT/CUDA teardown artifact AFTER `test result: ok`; not a test failure.)

## FILES

**Surgery + verify (one-time export, NOT a serving venv):**
- `/home/bud/ditto/waav/waav-infer/eval/onnx_drop_gqa_bias.py`

**Staged ONNX (dense-packed, same byte count as the originals) + manifests:**
- `/home/bud/.cache/waav-models/voxtral-realtime/onnx/audio_encoder_q4f16_nobias.onnx` (+ `_data`, 0.59 GB)
- `/home/bud/.cache/waav-models/voxtral-realtime/onnx/decoder_model_merged_q4f16_nobias.onnx` (+ `_data`, 2.02 GB)
- `/home/bud/.cache/waav-models/voxtral-realtime/waav.json` (updated → `_nobias` graphs)
- `/home/bud/.cache/waav-models/cohere-transcribe-candle/onnx/decoder_model_merged_fp16_nobias.onnx` (+ `_data`, 0.34 GB)
- `/home/bud/.cache/waav-models/cohere-transcribe-candle/waav.json` (new → `cohere_asr`, `_nobias` decoder)

**Gates:**
- `/home/bud/ditto/waav/waav-infer/crates/waav-infer-backend-torch/tests/cuda_ort_voxtral_nobias_runs.rs`
- `/home/bud/ditto/waav/waav-infer/crates/waav-infer-backend-torch/tests/cuda_ort_cohere_nobias_runs.rs`
- `/home/bud/ditto/waav/waav-infer/crates/waav-infer-server/tests/item1_ort_cuda_registry.rs`
- `/home/bud/ditto/waav/waav-infer/crates/waav-infer-backend-torch/tests/cuda_ort_gqa_attention_bias_control.rs` (doc updated to point at the fix; still asserts the stock-graph baseline failure)

## TRIPWIRE

If a future ORT dylib adds an `attention_bias`-accepting CUDA GQA kernel, `cuda_ort_gqa_attention_bias_control` flips to a panic — at which point the surgery is optional. Until then it is the runnable ONNX-CUDA path. The math equivalence (zero-bias removal) holds for ANY all-ones-mask call pattern, so the `_nobias` graphs are a permanent, model-faithful substitute for these drivers.
