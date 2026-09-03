# 09 — WaaV Infer model-arm CORRECTNESS fixes (Phase E · F4 + F5)

Scope: `crates/waav-infer-core/src/{stt,tts,s2s}/*.rs` + `diarize.rs` + `enhance.rs` ONLY (the model
arms). NOT touched: `server/`, `runtime/serve.rs`, `codec_ar_batcher.rs`, `backend-api` (read only). LAW =
**bit-faithful**: a VALID-input f32 forward pass is numerically unchanged; these fixes only correct WRONG
behavior (broken fp16 output reads, silent corruption, panics on hostile/edge input).

**Build status (all GREEN):**
- `cargo check -p waav-infer-core` → **exit 0**
- `cargo check -p waav-infer-core --tests` → **exit 0**
- `cargo check -p waav-infer-core -p waav-infer-backend-api -p waav-infer-runtime` → **exit 0**
- `cargo clippy -p waav-infer-core --lib --tests` → **exit 0, zero warnings**
- `cargo test -p waav-infer-core --lib` → **61 passed, 0 failed, 7 ignored** (7 ignored = pre-existing
  live-GPU tests, not mine; all pre-existing bit-identity tests still pass → bit-faithfulness confirmed)
- 6 new F5 unit tests (pure-CPU, no GPU, no model files) → **all pass**

Files changed (12): `stt/{voxtral,qwen3_asr,funasr_nano,encdec,nemo_ctc,nemotron,parakeet,canary,cohere}.rs`,
`tts/{supertonic,chatterbox,melo,kokoro}.rs`, `diarize.rs`, `enhance.rs`. (No `s2s/` change — see F4 leaves.)

---

## F5 — per-arm corruption fixes (each a targeted fix + a fail-before/pass-after unit test)

### F5-1 · funasr_nano KV-cache silent drop on overflow → typed error  ✅ unit-tested
`stt/funasr_nano.rs` `write_deltas` (was :193).
- **Before:** `if off+span <= buf[i].len() && v.len() >= span { copy } ` — else **silently skipped** the KV
  write; loop advanced `past+=1` and kept decoding against a stale/zero key → compounding corruption, no signal.
- **After:** overflow (`off+span > buf[i].len()`) → typed `FunAsrError::CacheOverflow{pos,span,cap}`; short
  delta (`v.len() < span`) → typed `Output`. Never skip-and-continue. Plus a prefill guard:
  `prompt_len > cache_cap` → `CacheOverflow` up front (the prefill `write_deltas(..,0,prompt_len)` could
  itself overflow on a long clip). New error variant `CacheOverflow` added.
- **Bit-faithful:** an in-bounds write is the identical `copy_from_slice`; only the previously-silent
  overflow/short cases now error instead of corrupting.
- Refactored the per-layer copy decision into free fn `write_delta_into(...)` so it is testable without a
  full model load. Also F4'd the `to_f32_vec` read of the delta (see F4).
- **Test** `write_delta_into_errors_on_overflow_not_silent_drop`: overflow → `CacheOverflow` + buffer
  untouched (no partial write); short delta → `Output`; valid write copies exactly.
- **GPU verification needed:** NO (pure-CPU unit test).

### F5-2 · qwen3_asr hardcoded fp16 stride on embed table → config-driven dtype  ✅ unit-tested
`stt/qwen3_asr.rs` `embed_row` + `new` (was :110/:115/:81).
- **Before:** `off = t*hidden*2`, `f16::from_le_bytes(...)` — **assumed fp16 always**. An fp32/bf16 re-export
  of the same arch fed completely-wrong `input_embeds` every step → nonsense transcript, no error.
- **After:** new `EmbedDtype{Fp16,Fp32,Bf16}` parsed from config `embed_tokens_dtype` (the field IS present
  in the community export, verified: `'float16'`); stride = `t*hidden*itemsize`; per-dtype LE decode. `new`
  reads `embed_tokens_dtype` + `decoder.hidden_size` + `decoder.vocab_size` from config and **validates**
  `embed.len()` is an exact `[rows,hidden]` grid AND `rows == decoder.vocab_size` (typed `Layout` error on
  mismatch). Defaults fp16 only when the field is ABSENT (back-compat with the existing fp16 export).
- **Bit-faithful:** for the existing fp16 export, itemsize=2 and the fp16 decode is identical to before.
- **Test** `embed_dtype_itemsize_and_decode_are_dtype_correct` (itemsize + exact fp16/fp32/bf16 LE decode +
  parse) and `decode_embed_row_fp32_strides_correctly_and_bounds_checks` (fp32 row1 reads the RIGHT bytes —
  the hardcoded-fp16 stride would read garbage).
- **GPU verification needed:** NO for the stride/decode (unit-tested). An fp32/bf16 embed_tokens.bin export
  does not exist locally; the existing fp16 export's bit-identity should be re-confirmed on GPU (it loads via
  the unchanged fp16 path).

### F5-3 · voxtral prompt truncation on short clip → PAD audio, keep full prompt  ✅ unit-tested
`stt/voxtral.rs` `transcribe` (was :235-240).
- **Before:** `let l = prompt.len().min(n_audio);` then the 39-token BOS+streaming-pad scaffold was
  **truncated** to `l` when `n_audio < 39` → BOS + part of the scaffold dropped → corrupt lockstep alignment
  → garbage first tokens that AR-compound (exactly the short/streaming clips this arm targets).
- **After:** `l = prompt.len()` (always full); the FULL prompt is embedded; `audio_embeds` is zero-padded up
  to `l*hidden` so the prefix `audio[..l] + embed(full_prompt)` is intact. Real `n_audio` still bounds
  emission (`pos >= n_audio` stops a short clip early — the padded rows are never read in the step loop).
- **Bit-faithful:** when `n_audio >= 39` (the normal case) the resize is a no-op and the prefix is
  element-for-element identical to before.
- Refactored the prefix add into free fn `build_scaffold_prefix(...)` for testability.
- **Test** `short_clip_pads_audio_keeps_full_prompt_scaffold`: `n_audio=2 < prompt_len=4` → prefix is full
  `prompt_len*hidden` (not truncated), BOS row present, scaffold preserved over zero-padded audio rows; the
  `n_audio >= prompt_len` case unchanged.
- **GPU verification needed:** the arithmetic is unit-tested; an end-to-end short-clip transcript on a live
  voxtral model would additionally confirm the in-loop path (the inline `resize` + step indexing). LOW risk.

### F5-1b · qwen3_asr out-of-range token id → typed error (was silent all-zero embedding)  ✅ unit-tested
`stt/qwen3_asr.rs` `embed_row` (was :109-122).
- **Before:** per-element `if b+1 < embed.len() { decode } else { 0.0 }` — an out-of-range token id silently
  produced an all-zero embedding → silently-WRONG transcript.
- **After:** `t >= embed_rows` → typed `Qwen3AsrError::TokenOutOfRange{id,rows}`. (funasr_nano's embedding is
  a GRAPH call, not a host-side raw table, so it has no analogous host-side silent-zero; its out-of-range id
  is the embedding graph's contract. The host-side silent-zero hazard named in the review is the qwen3 raw
  table, fixed here.) New error variant `TokenOutOfRange`.
- **Bit-faithful:** in-range ids decode identically (the validation in `new` guarantees the slice is
  in-bounds, so the old per-element `else 0.0` branch is now unreachable for valid input).
- **Test:** covered by `decode_embed_row_fp32_strides_correctly_and_bounds_checks` (id == rows → error).
- **GPU verification needed:** NO.

### F5-4 · encdec slice_rows no `ai<b` guard → typed error (was panic)  ✅ unit-tested
`stt/encdec.rs` `slice_rows` (was :563/:573).
- **Before:** only checked `v.len() < b*row_elems`; indexed `v[ai*row..(ai+1)*row]` with no guard that each
  `ai < b` → a cohort-bookkeeping bug feeding `ai >= b` panics the shared serve loop.
- **After:** `active.iter().find(|ai| ai >= b)` → typed `EncDecError::Output`. Both F32 and F16 arms covered.
- **Bit-faithful:** valid active sets (all `ai < b`) slice identically.
- **Test** `slice_rows_rejects_out_of_range_active_row`: out-of-range → typed Output (no panic); in-range row
  copies exactly; F16 bad-row likewise typed.
- **GPU verification needed:** NO.

### F5-5 · diarize segmentation slice trusts claimed shape → validate shape×len (was panic)  ✅ unit-tested
`core/diarize.rs` `segment_window` (was :209).
- **Before:** `frames = y.shape[1]` trusted; `data[f*NUM_CLASSES..(f+1)*NUM_CLASSES]` indexed for
  `f in 0..frames` → a malformed graph output whose `shape[1]` overstates the real element count panics.
- **After:** `if data.len() < frames*NUM_CLASSES { typed error }` before the per-frame loop.
- **Bit-faithful:** a well-formed output (data length matches the claimed shape) decodes identically.
- **Test** `segment_window_rejects_shape_vs_len_mismatch` (graph double via `Diarizer::new(seg, emb)` — no
  file I/O, pure CPU): a seg double claiming `[1,100,NUM_CLASSES]` but returning one short row → typed error,
  no panic.
- **GPU verification needed:** NO.

---

## F4 — fp16/q4f16 OUTPUT extraction (SYSTEMIC): `.as_f32()` → `.to_f32_vec()` at GRAPH-OUTPUT reads

`TensorData::as_f32()` returns `None` for F16 (backend-api `lib.rs:74`); `to_f32_vec()` widens F16→f32 and
passes F32 through (`lib.rs:100`, owned `Option<Vec<f32>>`). Every site below reads a tensor that is a GRAPH
OUTPUT (encoder/decoder/joint/vocoder/embed/preprocessor output, KV/state/logits/waveform) that becomes F16
under an fp16/q4f16 model — so `as_f32()` hard-errors (or silently empties) on the first F16 tensor.
Owned-`Vec` call sites adjusted; the redundant `.to_vec()` after the old `.as_f32()` was dropped (no extra
clone). **Bit-faithful:** for an F32 graph, `to_f32_vec` returns the identical values (`v.clone()`); only the
F16 case changes (widen instead of error).

| file:line(old) | tensor (why it can be F16) |
|---|---|
| voxtral.rs:154 | `inputs_embeds` — embed_tokens graph output (q4f16/fp16 → F16) |
| voxtral.rs:188 | `audio_embeds` — audio_encoder graph output |
| qwen3_asr.rs:142 | `audio_features` — encoder graph output |
| funasr_nano.rs:151 | `embeddings` — embedding graph output |
| funasr_nano.rs:192 | `key/value_delta_*` — llm graph KV-delta outputs |
| funasr_nano.rs:221 | `encoder_out` — encoder_adaptor graph output |
| canary.rs:185 | `features` — preprocessor graph output |
| canary.rs:207 | `encoder_embeddings` — encoder graph output |
| canary.rs:265 | `logits` — decoder graph output |
| canary.rs:285 | `decoder_hidden_states` (mems) — decoder graph output |
| cohere.rs:100 | `features` — preprocessor graph output (hidden/KV already use `cast_float`/`input_dtype`) |
| nemo_ctc.rs:64 | `features` — preprocessor graph output |
| nemo_ctc.rs:87 | `logprobs` — CTC model graph output (passed to `ctc::greedy(&data,…)`) |
| nemotron.rs:159 | encoder `outputs` — via `f32_out` |
| nemotron.rs:183 | `decoder_output` — via `f32_out` |
| nemotron.rs:192 | `joint_output` — via `f32_out` |
| nemotron.rs:295 | `f32_out` helper (covers `cache_last_channel_next`, `cache_last_time_next`, `h_out`, `c_out`) |
| parakeet.rs:132 | `features` — preprocessor graph output |
| parakeet.rs:149 | encoder `outputs` — encoder graph output (owned buffer also frees the graph for decode) |
| parakeet.rs:200 | joint `outputs` — decoder_joint graph output |
| parakeet.rs:212/217 | `output_states_1/2` — decoder_joint state outputs |
| supertonic.rs:288 | `text_emb` — text_encoder graph output (solo path) |
| supertonic.rs:354 | `wav_tts` — vocoder graph output (solo path) |
| supertonic.rs:477 | `duration` — duration_predictor graph output (batched) |
| supertonic.rs:498 | `text_emb` — text_encoder graph output (batched) |
| supertonic.rs:611 | `wav_tts` — vocoder graph output (batched) |
| supertonic.rs:867 | `denoised_latent` — vector_estimator graph output (`flow_solve`) |
| supertonic.rs:912 | `denoised_latent` — vector_estimator graph output (`flow_solve_batch`) |
| supertonic.rs:930 | `first_scalar` helper — `duration` scalar graph output |
| chatterbox.rs:940 | `waveform` — conditional_decoder/vocoder graph output |
| melo.rs:172 | VITS waveform graph output (`first_mut`→`first`, no longer needs `mut`) |
| kokoro.rs:242 | `waveform` graph output (was silent-empty via `unwrap_or_default` on F16 — worst case) |
| diarize.rs:205 | segmentation `seg` graph output |
| diarize.rs:254 | wespeaker embedding graph output |
| enhance.rs:227 | `get()` helper (covers `enh`/`spec_e`/`audio_output`/`*_cache_out`/`state_out` graph outputs); return type changed to owned `Vec<f32>`, the 4 `?.to_vec()` call sites dropped the redundant `.to_vec()` |

### F4 — deliberately LEFT unchanged (justified — NOT needless churn)
- **sensevoice.rs:177** — the `as_f32()` is the fast-path of a `match` that ALREADY has a `to_f32_vec()` F16
  branch (:182). Correct; review confirms sensevoice handles fp16.
- **encdec.rs:902/963/1293** — inside `#[cfg(test)] mod tests` (from :766): test-double fake graphs hashing
  known-f32 inputs. Not a production fp16 path. (Review marks encdec the strongest code.)
- **supertonic.rs:1012/1013/1124** — inside `#[cfg(test)]` (RecEstimator double + ref-equiv test); known-f32
  test tensors.
- **chatterbox.rs:473 (AR embed), :649/:710 (present.* KV re-fed as f32), :1405 (`take_f32` conditioning
  cache re-fed as `speaker_embeddings`/`speaker_features` f32), :787/:1827 (test)** — these are the
  chatterbox **AR / conditioning f32-pinned KV-feedback machinery**, NOT terminal output reads. The task
  scoped chatterbox to the vocoder output (:940) ONLY. Making these piecemeal-F16 without the matching
  INPUT-dtype work would be inconsistent and is the separate HIGH "chatterbox AR is f32-pinned" (out of F4
  scope). **FLAGGED** below.
- **s2s/duplex_codec_ar.rs:432** — inside the synthetic-scaffold bench constructor (`user_conditioned`/
  `from_dir`). S2S `CodecArDuplexModel` is the unregistered shelf-ware (CRITICAL C1) the lead will
  down-scope/relabel; it is not a real loadable arm. **FLAGGED**, not changed (scaffold disposition is the
  lead's).

### F4 — known REMAINING gap (flagged, beyond F4 scope = INPUT-dtype work, separate HIGHs)
Several arms now read OUTPUTS F16-safely but still build their step INPUTS as `NamedTensor::f32`, so a full
fp16 run would be rejected at the next input feed (the output read no longer hard-errors on the FIRST F16
tensor, which is the F4 fix, but full fp16 needs input casts via `StaticGraph::input_types()`):
- **canary** — `decoder_mems` / `encoder_embeddings` re-fed f32 (HIGH: "canary no fp16/quant at all").
- **supertonic** — all CFM feeds built `NamedTensor::f32` (HIGH: "supertonic f32-only by construction").
- **chatterbox** — AR KV + conditioning re-fed f32 (the :473/:649/:710/:1405 leaves above).
- **parakeet / nemo_ctc / nemotron / qwen3 / funasr** — encoder/joint/decoder feeds f32 (the bit-faithful
  fp16 path needs the `input_types()`-driven cast, the cohere pattern). cohere already does this for hidden/KV.

These are the existing HIGH findings (not F4). The F4 output-extraction fixes are complete and necessary for
any of them; the input-dtype work is the natural follow-up.

---

## runtime/precision.rs (voxtral.rs:31-34 narrowing) — INSPECTED, NO CHANGE NEEDED (flagged)
The synthesis line "precision.rs narrows every non-F16 KV dtype→F32" is a slight mischaracterization.
`empty_kv_dtype` (`crates/waav-infer-runtime/src/precision.rs:45-54`) ALREADY reads the graph's **declared**
dtype per input name (`types.get(i).copied()`) and returns it faithfully — a bf16 or i64 KV input the graph
declares is returned as bf16/i64, NOT collapsed to F32. The ONLY F32 fallback is `unwrap_or(ElemType::F32)`
when the graph declares NOTHING for a name (an fp32 model) — correct back-compat. So it is already
graph-driven (its own tests `empty_kv_dtype_follows_weight_precision_q4f16` /
`feature_inputs_stay_f32_by_name` prove f16 KV is preserved). No narrowing bug → no change. (File is consumed
by the voxtral arm and is the single source of truth `zeros_typed` delegates to; not serve-path.)

---

## Summary: what needs LIVE GPU fp16-model verification vs unit-tested

**Unit-tested (NO GPU needed) — proven fail-before/pass-after, all 6 pass:**
- F5-1 funasr write_deltas overflow (`write_delta_into_errors_on_overflow_not_silent_drop`)
- F5-2 qwen3 dtype-driven stride (`embed_dtype_itemsize_and_decode_are_dtype_correct`,
  `decode_embed_row_fp32_strides_correctly_and_bounds_checks`)
- F5-1b qwen3 out-of-range token (same `decode_embed_row` test)
- F5-3 voxtral prompt-pad arithmetic (`short_clip_pads_audio_keeps_full_prompt_scaffold`)
- F5-4 encdec slice_rows bounds (`slice_rows_rejects_out_of_range_active_row`)
- F5-5 diarize seg shape×len (`segment_window_rejects_shape_vs_len_mismatch`)

**Needs LIVE GPU fp16/q4f16-model verification (the F4 output-extraction changes — there is currently ZERO
fp16 live coverage, so a per-arm fp16 smoke/accuracy gate is the right follow-up):**
- **voxtral q4f16** — `inputs_embeds`/`audio_embeds` F16 outputs (+ confirm the F5-3 short-clip in-loop path).
- **supertonic fp16** — `text_emb`/`denoised_latent`/`wav_tts`/`duration` F16 outputs (NOTE: full fp16 still
  blocked by f32-pinned CFM inputs — the F4 read change alone won't make an fp16 supertonic end-to-end pass).
- **canary fp16** — `logits`/`mems`/`features`/`encoder_embeddings` F16 (full fp16 blocked by f32 mems feed).
- **chatterbox fp16** — `waveform` vocoder F16 output (full fp16-AR blocked by f32 KV/conditioning feeds).
- **parakeet / nemo_ctc / nemotron / cohere / qwen3 / funasr / melo / kokoro / diarize fp16/q4f16** — the
  encoder/joint/CTC/embedding/waveform/seg/embedding F16 outputs now widen; live fp16 exports would confirm
  the read path end-to-end (input-feed dtype caveats per the "remaining gap" flag above for the AR/CFM ones).

The lead owns the GPU and the live fp16 smoke/accuracy gates. No `git commit` performed.
