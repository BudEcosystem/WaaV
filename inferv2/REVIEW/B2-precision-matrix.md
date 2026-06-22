# B2 — Precision / dtype / quantization support matrix (WaaV Infer)

**Goal (user hard requirement):** every model works in every precision on every hardware.
**Method:** READ-ONLY code read of the precision plumbing + per-arm input-dtype audit + live ONNX-graph
inspection of the failing voxtral/cohere graphs + the GB10 ORT-1.27 CUDA provider `.so` symbol/string dump.
**Date:** 2026-06-21. No `cargo build` was run (a separate effort owns builds).

---

## 0. TL;DR verdicts

1. **The #2 gap is real and BROAD.** Only **3** of the ~13 model arms build their stepped/decoder FLOAT
   *inputs* at the graph's declared dtype (`cohere`, the shared `encdec` used by `whisper`+`moonshine`,
   and `voxtral` — and `voxtral` only for its KV/state, NOT its `inputs_embeds` step input). **Every other
   arm hardcodes `NamedTensor::f32` for the tensors it feeds into graphs that can be exported fp16/q4f16**
   (KV caches, conditioning tensors, AR `inputs_embeds`, CFM constants, vocoder latents, RNNT/AED state).
   F4 fixed only the OUTPUT read path (`to_f32_vec()` is everywhere); the INPUT side is unfixed in 9 arms.

2. **voxtral q4f16 + cohere fp16 fail on CUDA for the SAME root cause, and it IS ONNX-fixable.** Both
   decoders use `com.microsoft::GroupQueryAttention` whose **11th input `attention_bias`** is populated
   (`/model/gqa_attention_bias/Expand/output_0`). The GB10 ORT-1.27 CUDA GQA kernel cannot serve a
   *biased* GQA on the Blackwell flash path (cuDNN "no plan" for head_dim=128 / 32:8) and the unfused/
   memory-efficient fallbacks don't carry the bias either. The fix is a **graph rewrite that folds the
   additive `attention_bias` into a path the CUDA kernel supports** (or routes self-attn through
   `MultiHeadAttention`, which cohere already uses for its cross-attn) — it does **not** require a non-ORT
   runtime. Counter-proof on disk: `chatterbox` q4f16 uses GQA **without** `attention_bias` (input[10]
   empty) and is the one large AR model that loads on CUDA.

3. **Device × precision gaps:** int8/uint8 **blocked on CUDA/TensorRT** (no int8-GEMM — already guarded);
   GQA-with-bias fp16/q4f16 **blocked on CUDA** (the voxtral/cohere bug); fp16 **works on CPU only as a
   load format** (ORT CPU silently up-converts fp16→fp32 compute); bf16 is **CPU-tier-internal only**
   (MLAS-SBGemm accumulate), never a weight format here; fp8/mxfp4 are resolver-modeled but **no weights
   exist**. CPU is the universal floor for fp32 and the only place int8 actually runs as int8.

4. **The `QuantStamp` / `admit_quant_variant` accuracy gate is DEAD CODE.** It is fully implemented +
   unit-tested in `core/src/model.rs` but **never called from `load_model`** and **never parsed from
   `waav.json`**. The only precision gate actually enforced at load is `guard_precision_ep` (int8-on-CUDA
   refusal) in `backend-ort`. So today every advertised quant (incl. voxtral's `"precision":"q4f16"`)
   loads **unstamped/ungated** — the opposite of the spec's "never default-on quant".

---

## 1. The precision plumbing (end-to-end trace)

### 1.1 How a precision token flows to a weight file

```
$WAAV_PRECISION (operator)  ─┐
waav.json "precision": "..." ─┤→ Manifest::precision_token()                  [core/src/model.rs:78]
                              │     → canonical_precision() alias-normalize    [components/standardize.rs:116]
                              │       (half→fp16, 8bit/q8→int8, 4bit→q4; unknown passes through verbatim)
                              ▼
                    Manifest::weight_path(dir, logical, stem)                  [core/src/model.rs:92]
                      explicit weights[logical] override  ── wins ──┐
                      else  onnx/{stem}{_precision}.onnx (fp32/absent ⇒ no suffix)
                              ▼
                    GraphLoader::load_graph(path)  →  OrtModel::load_ep        [server/engine.rs:60]
                              ▼
                    guard_precision_ep(active_ep, path)  ← THE ONLY enforced gate  [backend-ort/lib.rs:165,274]
                      refuses int8/uint8/q8/i8/u8/8bit file on Cuda|TensorRt (typed BackendError::Precision)
                              ▼
                    CpuTier::guard_cpu_tier_int8 (on CPU floor: also refuse int8 — CPU tier is bf16/fp32-accum)
                              ▼
                    session.commit_from_file → input_names/input_types/input_shapes captured  [lib.rs:288-290]
```

`Manifest::precision_token` precedence = **`$WAAV_PRECISION` → manifest `precision` → fp32**. The token is
*also* the onnx-community filename suffix, so adding a precision variant is config-only (the registry's
zero-code property). The `_{precision}.onnx` convention is emitted by `weight_path`; explicit `weights{}`
overrides win (used by voxtral, nemotron, chatterbox, etc. where filenames are hyphenated, not stem-suffixed).

### 1.2 The dtype side-channel (`ElemType` / `input_types()` / `TensorData`)

* `ElemType` (`backend-api/lib.rs:13`): `F32 | F16 | I64 | I32 | Bool | Other`. **No `BF16`, no int4/q4
  member** — bf16 weights and q4 graphs surface as `Other` (or the float they decompress to).
* `OrtModel` captures `input_types: Vec<ElemType>` from `session.inputs()[i].dtype()` at load
  (`lib.rs:289`, `elem_type_of` at `lib.rs:237`) and exposes it via `StaticGraph::input_types()`
  (`lib.rs:572`). `input_shape(name)` similarly (`lib.rs:575`). **This is the single source of truth an arm
  must consult to build inputs at the right dtype.**
* `TensorData` (`backend-api/lib.rs:53`): `F32 | F16 | I64 | I32 | Bool`. `to_f32_vec()` widens F16→f32
  (the **output-read** helper, used everywhere — F4). `as_f32()` returns `None` on F16 (the old trap).
* ORT I/O materialization (`to_ort_value` / `extract_named_output`, `lib.rs:350/404`) handles F16 on both
  sides incl. 0-length KV tensors via the session allocator. So **the backend seam is fully fp16-capable**;
  the gap is purely that **arms don't BUILD fp16 inputs**.

### 1.3 `empty_kv_dtype` — the graph-driven empty-state resolver

`runtime/src/precision.rs` resolves the dtype of empty KV / `past_padding_cache` / `zero_past` tensors from
`StaticGraph::input_types()` per input **name**:
* genuine-feature names (`input_features`/`inputs_embeds`/`audio_embeds`) → **f32 by NAME** (never narrowed),
* every other input → the graph's **declared** dtype (so a q4f16 graph's f16 KV → f16),
* unknown / undeclared → f32 (fp32 back-compat).

This is correct and is the pattern that should generalize. **Only `voxtral` consumes it** (`zeros_typed`,
`voxtral.rs:29`). It covers empty STATE only; it does **not** address a real, non-empty conditioning/embeds
input that must be cast (that's `cohere::cast_float`'s job).

### 1.4 The resolver + stamp gate that are NOT wired in

* `resolve_precision(requested, by_substrate, manifest, caps)` (`backend-api/lib.rs:589`) — full precedence
  + int8-on-CUDA demote + fp8(Hopper+)/mxfp4(Blackwell) one-tier-demote ladder. **Only called in its own
  unit tests** (`lib.rs:2178+`). Not in `load_model`.
* `admit_quant_variant(requested, caps, stamps)` + `QuantStamp` (`core/src/model.rs:238/134`) — the
  per-substrate accuracy/MOS gate ("never default-on quant"). **Type-level-gated, fully tested, and never
  called from production** (grep: the only non-test references are the `pub use` re-export at
  `core/src/lib.rs:14` and the test block). `waav.json` has **no stamp parser** (`Manifest` reads only
  `architecture` / `precision` / `weights`).

> **Consequence:** the spec's two precision-safety guarantees (accuracy-stamped quant; substrate-aware
> precision precedence/demote) exist as libraries but are **architecturally bypassed**. The live behavior is
> the dumb path: suffix → file → load, with only the int8-on-CUDA hard refusal protecting it.

---

## 2. Per-arm INPUT-dtype audit (the #2 work)

**Criterion:** does the arm build the FLOAT tensors it FEEDS into a graph that can be fp16/q4f16-exported
(encoder/decoder/AR-LM/KV/conditioning/CFM/vocoder) at the graph's **declared** dtype (via
`input_types()`/`input_dtype`/`cast_float`/`empty_kv_dtype`) — or hardcode `NamedTensor::f32`?
Reading OUTPUTS with `to_f32_vec()` is correct and NOT counted as a gap.

| Arm (file) | Reads `input_types()` for inputs? | Verdict | f32-pinned INPUT sites (line) |
|---|---|---|---|
| **whisper** (`stt/whisper.rs`) | YES (via `encdec`) | ✅ **CORRECT** | builds `input_features` f32 @191/221 but `encdec::decode` casts it (`encdec.rs:91-92`); KV+ehs cast @105-107 |
| **moonshine** (`stt/moonshine.rs`) | YES (via `encdec`) | ✅ **CORRECT** | `input_values` f32 @139, cast downstream by `encdec` |
| **encdec** (`stt/encdec.rs`, shared) | YES | ✅ **CORRECT** | `cast_float`(enc_in) @92/272; `ehs_dtype`/`kv_dtype` from decoder @105-107/286-288; empty KV typed @616/672 |
| **cohere** (`stt/cohere.rs`) | YES | ✅ **CORRECT** | `cast_float`(enc_in) @111-114; `ehs_dtype`/`kv_dtype` @121-122; `empty_past(dtype)` @127-128; `cast_float`(hidden) @123 |
| **voxtral** (`stt/voxtral.rs`) | PARTIAL (KV only) | ⚠️ **PARTIAL** | KV/`past_padding`/`zero_past` typed via `zeros_typed` ✅; but **`inputs_embeds` step input HARDCODED f32 @263-267**, and `input_features` f32 @164 (matches THIS graph but not graph-driven) |
| **qwen3_asr** (`stt/qwen3_asr.rs`) | NO | ❌ **F32-PINNED** | `mel`@217, `audio_features`@246, **`input_embeds` AR step @277**, KV `past_keys/values` re-fed f32 (@250-255 carries graph dtype only if graph emits f16 — but `decoder_init` audio_features fed f32 @246) |
| **funasr_nano** (`stt/funasr_nano.rs`) | NO | ❌ **F32-PINNED** | `cache_key_/value_` KV @163/168, encoder `x` @220, **`inputs_embeds` prefill @276 + step @298** |
| **canary** (`stt/canary.rs`) | NO | ❌ **F32-PINNED** | `audio_signal`@197, **`encoder_embeddings`@254**, **`decoder_mems` (KV) @256** (+ empty-init `Vec<f32>`) |
| **nemotron** (`stt/nemotron.rs`) | NO | ❌ **F32-PINNED** | `audio_signal`@147, `cache_last_channel`@149, `cache_last_time`@150, `h_in`@173, `c_in`@174, `encoder_output`@181, `decoder_output`@182 (all caches zero-init `Vec<f32>` @127-131) |
| **parakeet** (`stt/parakeet.rs`) | NO | ❌ **F32-PINNED** | `audio_signal`@140, `encoder_outputs`@184, prediction-net state `input_states_1/2`@187/192 (zero-init `Vec<f32>` @170-171) |
| **sensevoice** (`stt/sensevoice.rs`) | NO | ❌ **F32-PINNED** | CTC features `x`@163 |
| **nemo_ctc** (`stt/nemo_ctc.rs`) | NO | ❌ **F32-PINNED** | `audio_signal`@76 (+ raw `waveforms`@54, preprocessor is CPU-pinned) |
| **chatterbox** (`tts/chatterbox.rs`) | NO | ❌ **F32-PINNED** | `exaggeration`@468, **`inputs_embeds`@531/627/772/820** (single+batched AR), empty KV `empty_split_kv`@1372/1377, grown-KV re-feed @664/720, `conditional_decoder` `speaker_embeddings`/`speaker_features`@917/922, `audio_values`@1098 |
| **supertonic** (`tts/supertonic.rs`) | NO | ❌ **F32-PINNED** | CFM constants `text_emb`/`style_ttl`/`latent_mask`/`text_mask`/`total_step`@327-331 (+batch 585-589), per-step `noisy_latent`/`current_step`@858-859 (+batch 903-904), `style_dp`@253, vocoder `latent`@345/604 |
| **melo** (`tts/melo.rs`) | NO | ❌ **F32-PINNED** | `noise_scale`/`length_scale`/`noise_scale_w`@167-169 |
| **kokoro** (`tts/kokoro.rs`) | NO | ⚠️ **F32-PINNED but N/A** | `style`@235, `speed`@236 — but kokoro is **CPU-pinned by design** (`model.rs:514` LSTM CUDA divergence), so fp16-on-CUDA never applies |
| **s2s/duplex_codec_ar** (`s2s/duplex_codec_ar.rs`) | NO | ➖ **INHERITS chatterbox** | builds no production graph inputs of its own; rides `ChatterboxArStep` backbone (`audio_values`@427 is test-only) |

### 2.1 The prioritized f32-pinned fix list (the #2 work)

Ranked by (a) does a fp16/q4f16 variant exist on disk or is plausibly trending, and (b) is the model on the
realtime path:

| Rank | Arm | Why | Variant on disk? | Fix |
|---|---|---|---|---|
| **P0** | **voxtral** `inputs_embeds` @263 | The flagship fp16/q4f16 realtime STT; the `inputs_embeds` AR step is the ONE remaining f32-pin after the KV fix; q4f16 graph declares `inputs_embeds` **f32** today so it *happens* to work — but it is **not graph-driven**, so a future fp16 export (which declares `inputs_embeds` f16) silently breaks | q4f16 ✓ | route `inputs_embeds` through a `cast_float`-to-`input_types()` (the cohere pattern) |
| **P0** | **chatterbox** AR `inputs_embeds` + KV @531/627/664/720/1372 | fp16 **and** q4f16 LMs ship for base+turbo; AR-compounding makes a dtype mismatch fatal | fp16 ✓ q4f16 ✓ | `input_dtype`/`cast_float` for `inputs_embeds`; `empty_kv_dtype` for `empty_split_kv` + grown-KV re-feed |
| **P1** | **supertonic** CFM constants + per-step @327-331/858 | fp16 plausible; the CFM constants are the IoBinding loop-invariants (perf + precision both want this right) | (fp32 today) | cast the 5 CFM constants + 2 varying to `vector_estimator.input_types()` |
| **P1** | **qwen3_asr** `input_embeds`/`audio_features` @246/277 | LLM-decoder ASR family template; fp16 trending | (fp32 today) | cohere pattern on `audio_features` + `input_embeds`; KV already carries graph dtype via clone |
| **P1** | **funasr_nano** `inputs_embeds`+KV @163/276/298 | int8 sherpa export today (CPU-fine) but fp16 plausible; KV is the sharp break | (int8 today) | `empty_kv_dtype`-style for `cache_key_/value_`; cast `inputs_embeds` |
| **P2** | **cohere** (already correct) | reference implementation — replicate its `input_dtype`+`cast_float`+`empty_past(dtype)` shape | fp16 ✓ (HF cache) | — (it is the template) |
| **P2** | **canary** `encoder_embeddings`+`decoder_mems` @254/256 | AED; fp16 plausible | (fp32 today) | cast ehs + typed `decoder_mems` empty/feedback |
| **P2** | **nemotron / parakeet / sensevoice / nemo_ctc / melo** | streaming RNNT / CTC / VITS; mostly ship fp32 or int4-baked today; lower fp16 demand | mixed | apply the same two helpers when an fp16 variant is onboarded |

**The fix is mechanical and uniform** — promote the cohere/encdec helpers (`input_dtype`, `cast_float`,
`empty_kv_dtype`) into a **shared `components`/`runtime` utility** and thread every float graph-input
through `cast_float(t, graph.input_types()[name])`. The output side (`to_f32_vec`) is already done in all
arms. Recommend a single `fn feed_float(graph, name, shape, data: Vec<f32>) -> NamedTensor` that casts to the
declared dtype by name, mirroring `zeros_typed` but for non-empty data — then every arm's `NamedTensor::f32`
float-input call becomes `feed_float(...)`.

---

## 3. The voxtral / cohere CUDA failure — ROOT-CAUSE VERDICT

### 3.1 Evidence (live ONNX-graph + GB10 CUDA `.so` dump)

**voxtral `decoder_model_merged_q4f16.onnx`:**
- 26 × `com.microsoft::GroupQueryAttention`, 183 × `MatMulNBits` (the q4 weights), `do_rotary=1`,
  `num_heads=32`, `kv_num_heads=8`, `local_window_size=8192`.
- GQA has **11 inputs**; **input[10] `attention_bias` = `/model/gqa_attention_bias/Expand/output_0`**
  (a `Cast→Sub→Mul→Unsqueeze×3→Concat→Expand` subgraph producing a `[B, heads|1, S, T]` additive bias).
- Declared dtypes: `inputs_embeds` **f32**, all `past_key_values.*.key/value` **f16** (so the empty-KV must
  be f16 — the fix voxtral.rs already does; the f32 `inputs_embeds` is what voxtral.rs feeds @264, correct).

**cohere `decoder_model_merged_fp16.onnx`** (`~/.cache/huggingface/.../cohere-transcribe-...ONNX`):
- 8 × `GroupQueryAttention` (self-attn, **with the SAME `attention_bias` input[10]**) + 8 ×
  `MultiHeadAttention` (cross-attn). `encoder_hidden_states` f32, KV f16 — a genuine mixed-precision graph
  (why `cohere.rs` carefully casts per declared dtype).

**GB10 ORT-1.27 CUDA provider (`gb10-cuda-deps/ort-cuda/lib/libonnxruntime_providers_cuda.so`):**
- GQA kernel is instantiated ONLY as `GroupQueryAttention<MLFloat16,*>` and `<BFloat16,*>` —
  **no `GroupQueryAttention<float>` on CUDA** (the CPU lib has `GroupQueryAttentionIfEE` = float; CUDA does
  not). So even voxtral's q4 (fp32-compute, f32 KV) decoder has no CUDA GQA kernel at all.
- The CUDA GQA carries the strings `"Attention: using unified unfused path (is_gqa=..."` and
  `"falling back to Memory Efficient Attention or Unfused path."`, plus knobs `ORT_DISABLE_FLASH_ATTENTION`,
  `ORT_DISABLE_MEMORY_EFFICIENT_ATTENTION`, `ORT_DISABLE_FUSED_ATTENTION`. cuDNN backend API is linked.
- The op SCHEMA (CPU lib) validates `attention_bias` 4-dim shape AND enforces
  **`"Attention cannot have both past and attention_bias"`** on the legacy `Attention` op — confirming bias
  is a known-but-constrained feature across the attention family.

**Counter-example (proves it's the bias, not GQA itself):** `chatterbox` `language_model_q4f16.onnx` uses
30 × GQA **with input[10] EMPTY** (no attention_bias) — and chatterbox q4f16 is the large AR model that
loads on the GB10 CUDA EP. The ONLY structural difference at the failing node is the populated
`attention_bias`.

### 3.2 Verdict: **ONNX-FIXABLE — does NOT need a non-ORT runtime.**

The failure is `GroupQueryAttention` + a populated `attention_bias` on the GB10 CUDA kernel: the Blackwell
(sm_121) flash path has no cuDNN plan for head_dim=128 / 32:8 GQA *with* an additive bias, and the
unfused/memory-efficient fallbacks in this build don't carry the bias term either (so it either errors at
kernel selection or silently drops the bias = wrong logits → AR-compounding garbage). Three ONNX-path fixes,
in order of preference:

1. **Fold `attention_bias` into the attention mask (graph rewrite, best).** The `gqa_attention_bias`
   subgraph is a *static, additive* causal/window bias derived from `attention_mask` + positions. ONNX-side,
   rewrite the 26/8 GQA nodes to drop input[10] and instead encode the same masking through GQA's native
   `seqlens_k`/`total_sequence_length` + causal/`local_window_size` semantics (the bias here is the
   sliding-window causal mask `local_window_size=8192` already declared as an attribute — the explicit
   `attention_bias` is **redundant** with the attribute on a pure-causal decode). Dropping the redundant
   bias input makes the node identical to chatterbox's working GQA. This is a pure graph surgery
   (`onnx` GraphSurgeon / a Rust `StaticGraph`-rewrite pass at load), zero kernel work, bit-faithful for
   causal decode.

2. **Route the biased self-attn through `MultiHeadAttention` (cohere already does this for cross-attn).**
   The CUDA `MultiHeadAttention` kernel *does* accept an additive bias (`attention_bias`/`relative_position_
   bias`). Rewriting the 26 GQA→MHA loses the KV-cache-in-place GQA optimization but is fully CUDA-supported
   at fp16. Heavier rewrite (must materialize the GQA KV layout into MHA's), but mechanical.

3. **Force the unfused CUDA path that supports bias** (`ORT_DISABLE_FLASH_ATTENTION=1` +
   `ORT_DISABLE_MEMORY_EFFICIENT_ATTENTION=1`). If this build's *unfused* GQA path carries the bias (the
   `"unified unfused path (is_gqa=...)"` string suggests a unified kernel), this is a **zero-rewrite env
   toggle** — try first as a smoke test before committing to a graph rewrite. Slower (no flash), but
   unblocks correctness immediately and is reversible.

**Recommended sequence:** (3) as the smoke-test unblock → (1) as the production fix (redundant-bias drop,
bit-faithful, keeps GQA perf) → (2) only if (1)'s bias turns out non-redundant on some export.

### 3.3 ORT-CUDA contrib-op constraints catalogued (for the matrix)

- **int8 GEMM:** `QLinearMatMul`/`MatMulInteger` not run on CUDA/TensorRT (silent per-node CPU fallback) —
  already hard-refused by `guard_precision_ep` + `EpKind::forbids_int8_gemm` (`backend-api/lib.rs:437`).
  **q4 via `MatMulNBits` IS supported on CUDA** (voxtral q4f16 uses 183 of them — the weights aren't the
  blocker, the GQA bias is).
- **GroupQueryAttention on CUDA:** fp16/bf16 only (no fp32 kernel); **`attention_bias` unsupported on the
  Blackwell flash/efficient path** (the voxtral/cohere bug).
- **Kokoro StyleTTS2 duration LSTM:** numerically divergent on the GB10 CUDA EP → CPU-pinned by design
  (`model.rs:514`). A device-specific *correctness* pin, not a precision issue.
- **NeMo audio preprocessors:** CPU-pinned (`load_graph_cpu`) to match the reference STFT/mel.

---

## 4. Device × precision support grid

Legend: ✅ runs as that precision · ⚠️ loads but degrades/up-converts · ❌ blocked · — n/a (no weights).
"CUDA" = GB10 sm_121 / ORT-1.27. Gate column = where it's enforced (or "UNGATED" if it just loads).

| Precision | CPU (AMX/MLAS) | CUDA (GB10) | TensorRT | Gate / note |
|---|---|---|---|---|
| **fp32** | ✅ reference | ✅ (TF32 OFF by default for bit-faithful, `WAAV_ORT_TF32`) | ✅ | universal floor (P-6); no stamp needed |
| **fp16** (no GQA-bias) | ⚠️ loads, **compute up-converts to fp32** (ORT CPU has no fp16 GEMM) | ✅ | ✅ | UNGATED; arms must feed fp16 inputs (#2) |
| **fp16** (GQA **with** attention_bias) | ⚠️ (CPU GQA fp16→fp32) | ❌ **voxtral/cohere bug** | ❌ | §3 — ONNX-fixable graph rewrite |
| **q4f16** (`MatMulNBits`, no GQA-bias) | ⚠️ (MatMulNBits CPU = slow but runs) | ✅ (chatterbox proves it) | ✅ | UNGATED (should be stamp-gated) |
| **q4f16** (GQA **with** attention_bias) | ⚠️ | ❌ **voxtral bug** | ❌ | §3 |
| **q4** (`MatMulNBits`, f32 KV/compute) | ✅ | ❌ (no CUDA GQA-float kernel for the AR ones) | ❌ | UNGATED |
| **int8 / uint8 / q8** | ✅ **(the fast path here)** | ❌ **hard-refused** (`guard_precision_ep`) | ❌ refused | ENFORCED typed error |
| **int8 on CPU-TIER** | ❌ **refused** (`guard_cpu_tier_int8`) | — | — | CPU tier is bf16/fp32-accum, never int8 |
| **bf16** | ✅ *internal* (MLAS-SBGemm accumulate, exact-equiv to fp32) | (bf16 GQA kernel exists) | — | not a *weight* format here; `ElemType` has no BF16 member |
| **bnb4** | ⚠️ (only kyutai-mimi ships it) | ⚠️ untested | — | UNGATED; no arm/stamp |
| **quantized** (mixed int8) | ✅ if not int8-GEMM-on-CUDA | partial | — | passes through verbatim (unknown token) |
| **fp8 / mxfp4** | ❌ | resolver allows mxfp4≥sm100 / fp8≥sm90 | fp8 ✅ | **no weights exist** — resolver-modeled only |

### 4.1 Device × precision GAPS (what blocks "every precision on every hardware")

1. **GQA-with-bias on CUDA (G-1, the headline bug):** fp16 + q4f16 voxtral/cohere fail. **ONNX-fixable**
   (§3). Until fixed, voxtral/cohere realtime on GB10 must run CPU (slow) or torch-sidecar Path-B.
2. **fp16 compute on CPU (G-2):** ORT CPU loads fp16 weights but up-converts to fp32 for compute (no fp16
   MLAS GEMM) — so "fp16 on CPU" gives fp16 *memory* but fp32 *speed*. Not a correctness gap, a perf
   surprise; bf16/fp32-accumulate is the genuine CPU fast path. Document, don't "fix".
3. **int8 on CUDA (G-3):** correctly blocked — but it means the int8 sherpa exports (funasr_nano,
   nemotron-int4, sensevoice-int8) can only accelerate on CPU. The CUDA-accel quant path is **q4f16 only**.
   This is by design (the §5.2 master-constraint) but it caps which models get CUDA + small-footprint
   simultaneously to "q4f16 with no GQA-bias".
4. **No bf16 weight format (G-4):** `ElemType`/`TensorData` have no BF16 member, so a bf16-exported model
   would surface as `Other` and likely fail extraction. If bf16 weights are ever onboarded (Blackwell loves
   bf16), the seam needs a `BF16` arm in both enums + `to_ort_value`/`extract_named_output`.
5. **The accuracy gate is off (G-5):** because `admit_quant_variant` isn't wired, **no quant on any device
   is accuracy-verified at load** — the user's "every precision works" can't be *trusted* per-substrate
   until the stamp gate is connected and stamps are authored by the eval harnesses (which currently emit
   none — `eval/` has WER/MOS scripts but no stamp output).
6. **No per-substrate fallback ladder at load (G-6):** `resolve_precision`'s demote ladder (q4f16→fp16→fp32,
   fp8→fp16) is never invoked, so a failing precision **errors** instead of **degrading**. A model whose
   q4f16 hits the GQA bug today throws, rather than auto-falling-back to a working precision. Wiring
   `resolve_precision` + `admit_quant_variant` into `load_model` gives the graceful-degrade the
   "every precision everywhere" requirement implicitly needs (always land *something* runnable).

---

## 5. Quant variants present on disk (inventory)

| Model dir | precisions present (`onnx/*_{p}.onnx`) | waav.json precision | QuantStamp? | Notes |
|---|---|---|---|---|
| `voxtral-realtime` | q4f16, q4, quantized | **q4f16** | ❌ none (gate unwired) | q4f16 = the failing-on-CUDA GQA-bias case |
| `chatterbox-onnx` | fp16, q4f16, q4 | (no waav.json) | ❌ | q4f16 GQA has NO bias → loads on CUDA ✅ |
| `chatterbox-turbo-onnx` | fp16, q4f16, q4, quantized | note-only | ❌ | same GQA-no-bias shape |
| `kyutai-mimi-onnx` | **bnb4, fp16, int8, q4f16, q4, quantized, uint8** | (no waav.json) | ❌ | the widest variant set; int8/uint8 CUDA-blocked |
| cohere (HF cache `cohere-transcribe-...ONNX`) | fp16 (enc+dec) | — (loaded via ark-asr torch Path-B today) | ❌ | fp16 GQA WITH bias → CUDA-blocked (§3) |
| all other `waav-models/*` | fp32-only or int4/int8-baked-in (explicit `weights{}`) | mixed | ❌ | nemotron int4-baked, funasr int8, ark/dia/etc. are torch Path-B |

**Stamped & passing:** **none.** Every advertised quant is unstamped/untested at the gate level (the gate
doesn't run). The eval harnesses (`eval/stt_eval.py`, `sensevoice_eval.py`, `supertonic_eval.py`,
`tts_roundtrip.py`, `dataset_wer.py`) produce WER/MOS numbers but **emit no `QuantStamp` artifact** for
`waav.json`.

---

## 6. Prioritized precision fix list

| Pri | Fix | Files | Effort | Unblocks |
|---|---|---|---|---|
| **P0** | **GQA-attention_bias CUDA fix.** Smoke-test env toggle (`ORT_DISABLE_FLASH_ATTENTION` + `ORT_DISABLE_MEMORY_EFFICIENT_ATTENTION`) → then a load-time ONNX graph-rewrite pass dropping the redundant causal `attention_bias` from GQA (or GQA→MHA). | new graph-rewrite in `backend-ort` (or a `StaticGraph` decorator); voxtral/cohere graphs | M–L | voxtral q4f16 + cohere fp16 on GB10 CUDA (the #1 backlog item) |
| **P0** | **Generalize the input-dtype cast.** Promote `cast_float`/`input_dtype`/`empty_kv_dtype` to a shared `feed_float(graph, name, shape, Vec<f32>)` helper; thread EVERY float graph-input through it. | `components` (new helper) + all 9 f32-pinned arms (§2.1) | M | e2e fp16/q4f16 for chatterbox/qwen3/funasr/supertonic/canary/… (the #2 backlog item) |
| **P0** | **voxtral `inputs_embeds` graph-drive.** Replace the hardcoded f32 @263 with `feed_float`. (One line; the KV is already correct.) | `stt/voxtral.rs` | S | makes voxtral robust to a future fp16 export, not just today's f32-`inputs_embeds` q4f16 |
| **P1** | **Wire `admit_quant_variant` + `resolve_precision` into `load_model`.** Pass real `EpCaps` (active EP + sm_arch from `ep.rs`); parse `QuantStamp[]` from `waav.json`; degrade-don't-error on a failing precision (the ladder). | `core/src/model.rs::load_model`, `Manifest`, `server/engine.rs` (build `EpCaps`) | M | the spec's "never default-on quant" + graceful per-substrate fallback (G-5/G-6) |
| **P1** | **Author + emit `QuantStamp`s.** Have `eval/*` write a `waav.json` stamp block (`{precision, ep, passed}`) from the WER/MOS verdicts; stamp voxtral-q4f16-cuda, chatterbox-q4f16-cuda, the int8-CPU sherpa exports. | `eval/dataset_wer.py`, `stt_eval.py`, `supertonic_eval.py` + waav.json | M | trustable per-substrate accuracy for the matrix |
| **P2** | **Add `BF16` to `ElemType`/`TensorData` + ORT I/O** (forward-looking for Blackwell bf16 weights). | `backend-api`, `backend-ort` | S | G-4 (bf16 weight format) |
| **P2** | **Document fp16-on-CPU up-converts to fp32 compute** (G-2) in the precision docs so "fp16 on CPU" isn't mistaken for a speedup. | docs | S | expectation-setting |

---

## Appendix — key file:line references

- Precision plumbing: `core/src/model.rs:78` (`precision_token`), `:92` (`weight_path`),
  `:124` (`is_quant_variant`), `:238` (`admit_quant_variant`, **unwired**); `components/standardize.rs:116`
  (`canonical_precision`) + `:167` (`PRECISION_ALIASES`).
- Backend seam: `backend-api/lib.rs:13` (`ElemType`), `:53` (`TensorData` + `to_f32_vec` @100),
  `:376` (`StaticGraph::input_types`), `:437` (`forbids_int8_gemm`), `:589` (`resolve_precision`,
  **unwired**); `backend-ort/lib.rs:165/274` (`guard_precision_ep`, **the only enforced gate**),
  `:237` (`elem_type_of`), `:289` (input_types capture), `:350/404` (fp16-capable ORT I/O).
- Empty-state dtype: `runtime/src/precision.rs:45` (`empty_kv_dtype`), `:59` (`empty_kv_dtype_for`).
- Correct-pattern arms: `stt/cohere.rs:111-128/266-288` (template), `stt/encdec.rs:91-107/286-288/516-616`.
- Partial: `stt/voxtral.rs:29` (`zeros_typed`), **`:263` (the remaining f32-pin)**.
- EP policy / GB10 arena: `backend-ort/ep.rs:32-76` (CUDA EP, TF32-off, 48 GiB arena cap).
- Failing graphs on disk: `~/.cache/waav-models/voxtral-realtime/onnx/decoder_model_merged_q4f16.onnx`
  (26 GQA + attention_bias), `~/.cache/huggingface/hub/models--onnx-community--cohere-transcribe-03-2026-ONNX/.../onnx/decoder_model_merged_fp16.onnx`
  (8 GQA+bias / 8 MHA).
- GB10 CUDA provider: `~/ditto/budFoundry-Local/gb10-cuda-deps/ort-cuda/lib/libonnxruntime_providers_cuda.so`
  (GQA<MLFloat16|BFloat16> only, no float; "unified unfused path" + flash-disable knobs).
