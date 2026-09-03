# B7 — End-to-end fp16: graph-driven FLOAT *inputs* (backlog item #2)

**Goal:** close the input side of the fp16/q4f16 seam. F4 fixed the OUTPUT read path everywhere
(`to_f32_vec()` widens F16→f32). This change fixes the INPUT build path: the f32-pinned arms built every
float graph-input as a hardcoded `NamedTensor::f32`, so a future fp16/q4f16 export that *declares* an
input f16 would hand the CUDA node an f32 tensor it rejects (or that silently mis-types). Generalize the
already-correct cohere/encdec pattern (`cast_float` + `input_dtype` + `empty_kv_dtype`) to **all** arms.

**Method:** added one shared helper, threaded every f32-pinned float graph-input (B2's table) through it.
**Date:** 2026-06-21. Touched ONLY core arm files + the shared helper crate (runtime). Did NOT touch
`server/`, `runtime/serve.rs`, `codec_ar_batcher.rs` (another effort owns those). No `git commit`.

---

## 1. The shared helper — `feed_float`

Added next to `empty_kv_dtype_for` (the empty-state resolver it is the non-empty sibling of) in
`crates/waav-infer-runtime/src/precision.rs`, re-exported from `waav-infer-runtime`:

```rust
/// Build a NON-EMPTY float input tensor for `name` in the GRAPH'S DECLARED dtype for that input name.
pub fn feed_float(graph: &dyn StaticGraph, name: &str, shape: Vec<usize>, data: &[f32]) -> NamedTensor {
    let dtype = graph
        .input_names().iter().position(|n| n == name)
        .and_then(|i| graph.input_types().get(i).copied())
        .unwrap_or(ElemType::F32);            // graph declares nothing ⇒ f32 (fp32 back-compat)
    feed_float_as(name, shape, data, dtype)
}

/// `feed_float` with the dtype passed explicitly (for a site that already resolved it once).
pub fn feed_float_as(name: &str, shape: Vec<usize>, data: &[f32], dtype: ElemType) -> NamedTensor {
    let data = match dtype {
        ElemType::F16 => TensorData::F16(data.iter().map(|&x| f16::from_f32(x)).collect()),
        _ => TensorData::F32(data.to_vec()),  // F32 + every non-float/unmodeled dtype ⇒ keep f32
    };
    NamedTensor { name: name.into(), shape, data }
}
```

**Dtype resolution (the cohere `cast_float`/`input_dtype` pattern, generalized):**
1. graph declares `name` **F32** (every currently-shipping fp32/q4f16/fp16 export of these feature /
   embeds / conditioning inputs — B2 verified) → an **identical `NamedTensor::f32`** ⇒ ZERO numerical
   change for today's variants;
2. graph declares `name` **F16** (a future f16-declared export) → widen each `f32`→`f16` via
   `f16::from_f32` (the only behavioral *addition*);
3. anything else the seam doesn't model as a float (`I64`/`I32`/`Bool`/`Other`; there is **no
   `ElemType::BF16` member** — bf16 surfaces as `Other`) → **F32** (the safe back-compat default: never
   silently narrow real f32 data into a dtype the seam can't widen to bit-faithfully).

**bf16:** the task asked for a `bf16` arm "if an `ElemType::BF16` exists, else f32". It does **not** exist
(`backend-api` `ElemType` = `F32 | F16 | I64 | I32 | Bool | Other`), so bf16 surfaces as `Other` and
correctly falls into the f32 default branch (case 3). When a `BF16` member is added (B2 §4.1 G-4), add one
match arm here and every arm picks it up for free — the single-point-of-change property `feed_float` buys.

Why the runtime crate: `-core` already depends on `waav-infer-runtime`, `voxtral` already imports
`empty_kv_dtype_for` from it, and the runtime depends only on `protocol` + the pure-Rust `backend-api`
seam (P-8, zero C/C++) — `f16`/`ElemType`/`TensorData`/`StaticGraph` are all available there. This keeps
the helper next to its empty-state sibling, the single source of truth.

---

## 2. Per-arm before → after (the f32-pinned list from B2 §2.1)

For every site below: BEFORE = `NamedTensor::f32("name", shape, data)`, AFTER = `feed_float(graph, "name",
shape, &data)`. The empty-KV sites use `empty_kv_dtype_for` (voxtral's `zeros_typed` pattern). Where a
`run` call borrows the graph, the tensor is built into a `let` binding first (the graph borrow for `run`
and the `&dyn StaticGraph` for `feed_float` can't overlap), which is a pure refactor.

| Arm (file) | Inputs converted → graph-driven | Notes |
|---|---|---|
| **voxtral** (`stt/voxtral.rs`) | `inputs_embeds` (AR step), `input_features` (audio encoder) | The remaining f32-pin after the prior KV fix (`zeros_typed` already graph-drives the KV/`past_padding_cache`/`zero_past`). `input_features` now matches the cohere template's `cast_float(enc_in)`. |
| **qwen3_asr** (`stt/qwen3_asr.rs`) | `mel` (encoder), `audio_features` (decoder_init), `input_embeds` (decoder_step) | KV (`past_keys`/`past_values`) already carries graph dtype via `clone()` of the `present_*` outputs — unchanged. |
| **funasr_nano** (`stt/funasr_nano.rs`) | `x` (encoder), `inputs_embeds` (prefill + step), `cache_key_{i}` / `cache_value_{i}` (the 56 KV tensors) | KV is the sharp dtype break; routed through `feed_float` in `cache_tensors`. |
| **canary** (`stt/canary.rs`) | `audio_signal` (encoder), `encoder_embeddings` (decoder), `decoder_mems` (AED KV) | `decoder_mems` is empty on step-0 and grows from `decoder_hidden_states` (read via `to_f32_vec`→f32); now follows the decoder graph both ways. |
| **nemotron** (`stt/nemotron.rs`) | `audio_signal` + `cache_last_channel` + `cache_last_time` (encoder), `h_in` + `c_in` (decoder LSTM state), `encoder_output` + `decoder_output` (joint) | All RNNT streaming state; grown from the `*_next`/`*_out` outputs (read via `to_f32_vec`→f32). |
| **parakeet** (`stt/parakeet.rs`) | `audio_signal` (encoder), `encoder_outputs` + `input_states_1` + `input_states_2` (decoder_joint) | Prediction-net state grows from `output_states_*` (read via `to_f32_vec`→f32). |
| **chatterbox** (`tts/chatterbox.rs`) | `exaggeration` (embed); `inputs_embeds` single (`lm_forward`) + batched (`lm_forward_batched`); grown-KV re-feed `past_key_values.*` (batched assemble + per-slot un-pad); `empty_split_kv` (empty prefill KV, now `empty_kv_dtype_for`); `speaker_embeddings` + `speaker_features` (`conditional_decoder`); `audio_values` (`speech_encoder`); + the two `#[cfg(test)]` diag paths (`raw_batched_logits`, `raw_solo_logits`) for parity | Single-stream KV already carries graph dtype via `feedback_present_kv` (a dtype-preserving rename) — unchanged. `empty_split_kv` made graph-driven + given the `language_model` graph. |

### Arms genuinely fixed-f32 by graph (NO change needed — and why)

| Arm | Why no change |
|---|---|
| **whisper / moonshine / encdec** | Already CORRECT (B2 ✅): they route their float inputs through `encdec::cast_float`/`ehs_dtype`/`kv_dtype` (the reference shared seam). Untouched. |
| **cohere** | The reference template (B2 ✅) — `cast_float` + `input_dtype` + `empty_past(dtype)`. Untouched (it is what `feed_float` generalizes). |
| **sensevoice** (`stt/sensevoice.rs`) | CTC: a single feature input `x` (the mel) and no stepped state. It is a genuine f32 feature by NAME (the `empty_kv_dtype` feature rule) and the model ships int8/fp32 only — leaving it as `NamedTensor::f32` is correct today; convert when an fp16 variant is onboarded (B2 P2). Left as-is per task ("leave … if genuinely always-f32 by graph"). |
| **nemo_ctc** (`stt/nemo_ctc.rs`) | CTC: `audio_signal` to a NeMo encoder whose preprocessor is **CPU-pinned by design** (`load_graph_cpu`, to match the reference STFT/mel — B2 §3.3), so fp16-on-CUDA never applies to this leg. Genuinely f32. Left as-is (B2 P2). |
| **melo** (`tts/melo.rs`) | VITS scalars `noise_scale`/`length_scale`/`noise_scale_w` — ships fp32 today, lower fp16 demand (B2 P2). Genuinely f32 for the shipping graph. Left as-is. |
| **kokoro** (`tts/kokoro.rs`) | `style`/`speed` — but kokoro is **CPU-pinned by design** (StyleTTS2 duration-LSTM CUDA divergence, `model.rs:514`), so fp16-on-CUDA never applies (B2 marks it "F32-PINNED but N/A"). Left as-is. |
| **supertonic** (`tts/supertonic.rs`) | CFM constants + per-step + vocoder latents (B2 P1). Ships fp32 today; the IoBinding constants path is perf-coupled. NOT in the task's explicit B2 list to convert ("leave … if genuinely always-f32"; supertonic is P1-future, not P0). Left as-is — applies the same `feed_float` when an fp16 variant is onboarded. |
| **s2s/duplex_codec_ar** | Builds NO production graph inputs of its own — rides the `ChatterboxArStep` backbone (already converted). Its only `NamedTensor::f32("audio_values", …)` is inside `#[cfg(test)] mod tests` (a `FakeStage` helper), not a production path. Left as-is. |
| **`waveforms` raw-PCM input** (canary, parakeet — the one remaining `NamedTensor::f32` in each) | Feeds the **`nemo128` preprocessor**, which is `load_graph_cpu`-PINNED (CPU, to match the reference STFT/mel — same as nemo_ctc/cohere; `model.rs:430/497`). fp16-on-CUDA never applies to a CPU-pinned graph, and the preprocessor ships fp32. Genuinely f32 by graph — and the **cohere reference does exactly the same** (`cohere.rs:87` feeds `waveforms` as plain f32 to the same CPU preprocessor). Correctly left as `NamedTensor::f32`. |

---

## 3. Why this is BIT-FAITHFUL (the B7 law)

**Every currently-shipping fp32/q4f16/fp16 export declares these float *inputs* `F32`** (B2 verified
live on the graphs on disk — e.g. voxtral `inputs_embeds` is declared **f32 even in the q4f16 export**;
chatterbox base/turbo declare `inputs_embeds` + KV f32 even in the fp16/q4f16 LMs). So for every variant
on disk **today**, `feed_float` hits resolution case (1): graph dtype == F32 ⇒ it returns a tensor
**byte-identical** to the old `NamedTensor::f32(name, shape, data)` — same name, same shape, same bits.

The behavioral change is ADDITIVE and only fires on case (2): a graph that *declares* an input f16 (a
future fp16-declared export) gets the data widened to f16, where the old code would have crashed/mis-typed
the CUDA node. **Zero numerical change for everything that ships now; new correctness for a future
export.** This is exactly the cohere arm's existing behavior, now uniform across the fleet.

The empty-KV sites (`empty_split_kv`, voxtral `zeros_typed`) likewise resolve f32 today via
`empty_kv_dtype_for` (KV declared f32 in the shipping LMs) and only become f16 under a q4f16/fp16 export
that declares the KV inputs f16 — the same crash-fix the voxtral campaign already proved.

**Out-of-scope note (NOT regressed, NOT fixed here — F4 territory):** the chatterbox *batched* path still
reads `present.*` grown-KV via `as_f32()` (returns `None` on F16) before re-assembling, so under a real
f16-declared chatterbox LM that OUTPUT read would need F4-style `to_f32_vec()`. B7 only owns the INPUT
build; that read site is unchanged (and unreachable on today's f32-KV graphs). Flagged for the F4 owner.

---

## 4. Test + clippy results (the proof)

Run under `source gb10-env.sh` (live ORT-CUDA on GB10 sm_121).

### New unit tests (the helper)
`crates/waav-infer-runtime/src/precision.rs`:
- `feed_float_f32_graph_is_bit_identical` — an F32-declared graph (and an undeclared input) returns a
  tensor `==` the old `NamedTensor::f32(...)` constructor, dtype/shape/bits identical. **PASS.**
- `feed_float_f16_graph_widens` — an F16-declared graph widens each `f32`→`f16` via `f16::from_f32`
  (incl. the 65504/0.333 edge values); `feed_float_as` matches the graph-driven path; a non-feature
  KV-named float input also follows the graph to f16; an `Other`-declared input stays f32. **PASS.**
- (existing `empty_kv_dtype_*` tests still **PASS**.)

### Bit-identity gates — `cargo test -p waav-infer-core --lib`
```
test result: ok. 61 passed; 0 failed; 7 ignored; 0 measured; 0 filtered out; finished in 127.15s
```
The 7 ignored are the pre-existing `#[ignore]` live-GPU-leak gates (run via `ci/heavy_live_tests.sh`),
unchanged by this work. The deterministic bit-identity gates that exercise the converted code are GREEN:
- `tts::chatterbox::tests::batched_forward_codes_identical_to_per_slot` — ok (batched `inputs_embeds` @627 + KV @664 + un-pad @720)
- `tts::chatterbox::tests::ragged_batched_forward_codes_identical_to_per_slot` — ok
- `tts::chatterbox::tests::codec_ar_emitted_codes_identical_to_edge_path` — ok (single-stream `lm_forward` @531 + `empty_split_kv`)
- `tts::chatterbox::tests::codec_ar_run_ar_compounding_identical` — ok (the AR-compounding identity gate — the most dtype-sensitive)
- `tts::chatterbox::tests::chatterbox_decode_audio_stream_is_bit_identical_to_whole_body` — ok (`decode_body` speaker conditioning + `audio_values`)
- `tts::supertonic::tests::flow_solve_bit_identical_to_run_loop` — ok

### Live CUDA bit-identity (isolated, real chatterbox model on GB10)
`cargo test -p waav-infer-core --lib -- --exact --ignored
tts::chatterbox::tests::live_ragged_batched_forward_bit_identical_and_scales` (real chatterbox model on
GB10 sm_121, CUDA EP, process-isolated):
```
ragged bit-identity: PASS — 4 slots at DISTINCT lengths [18, 74, 67, 60], codes identical batched-vs-per-slot
ragged throughput: PASS — best ragged batched speedup 1.63x over the per-slot loop
test result: ok. 1 passed; 0 failed; 0 ignored; ... finished in 319.70s
```
**This is the on-hardware proof:** the converted chatterbox paths (batched `inputs_embeds` via
`feed_float`, batched KV re-feed + per-slot un-pad, single-stream `lm_forward`, graph-driven
`empty_split_kv`) emit BYTE-IDENTICAL codes to the per-slot path on the real CUDA model — bit-faithful
confirmed end-to-end, not just on the deterministic doubles.

### Clippy
- `cargo clippy -p waav-infer-core --lib -- -D warnings` → **clean** (no warnings).
- `cargo clippy -p waav-infer-runtime --lib -- -D warnings` → **clean** (no warnings).

---

## 5. Summary

- **Arms converted (7):** voxtral, qwen3_asr, funasr_nano, canary, nemotron, parakeet, chatterbox.
- **Shared helper:** `feed_float` / `feed_float_as` in `waav-infer-runtime` (`precision.rs`), unit-tested.
- **Genuinely fixed-f32 (no change):** sensevoice, nemo_ctc (CPU-pinned), melo, kokoro (CPU-pinned),
  supertonic (P1-future, not in the convert list), s2s/duplex (test-only input); whisper/moonshine/encdec/
  cohere already correct.
- **Bit-faithful:** every shipping variant declares these inputs f32 → `feed_float` returns the identical
  f32 tensor → ZERO numerical change; the change only adds correctness for a future f16-declared export.
- **Gates:** core lib 61/0 green (127s); helper unit tests green; clippy clean on both crates.
</content>
</invoke>
