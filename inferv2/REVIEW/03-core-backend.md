# 03 — WaaV Infer CORE + ORT BACKEND: brutal correctness review

Scope: `crates/waav-infer-core/src/` (model registry + 12 STT + 4 TTS + S2S + diarize + enhance) and `crates/waav-infer-backend-ort/src/` (lib.rs, ep.rs, cpu_tier.rs). Static read only (no build/run). Bar: enterprise correctness — bit-faithful accuracy, every arm fully wired.

Method: foundational files (model.rs, encdec.rs, backend-api, ORT lib/ep/cpu_tier, whisper, moonshine, sensevoice, parakeet, nemo_ctc, kokoro, melo, s2s) read directly; LLM-decoder arms (voxtral/qwen3_asr/funasr_nano/cohere/canary), nemotron/diarize/enhance, mel/stft/kaldi components, and chatterbox/supertonic read by dedicated sub-reviewers. Findings cross-checked with targeted grep.

## Counts

- **CRITICAL: 4** · **HIGH: 9** · **MED: 12** · **LOW: 9** = **34 findings**
- Arms reviewed: **12 STT + 4 TTS + 1 S2S + 2 aux (diarize/enhance) = 19**
- Fully-wired arms: **18 / 19** load→infer→output (the lone exception, S2S, is shelf-ware: see C1)
- **Headline systemic defect:** the advertised fp16/q4f16 precision path is **broken in ~10 arms** — they read graph outputs with `TensorData::as_f32()`, which returns `None` for F16 (backend-api `lib.rs:74`) instead of widening via `to_f32_vec()` (`lib.rs:100`). The registry's quant machinery (`waav.json` precision, `$WAAV_PRECISION`) can *select* fp16, but most arms then hard-error on the first F16 tensor. **49 `as_f32()` call sites across core arms; only 2 files (sensevoice, chatterbox) use `to_f32_vec`.**
- **Second systemic defect:** the `s2s::CodecArDuplexModel` is a synthetic benchmark scaffold (hash-folded fake "user conditioning" driving the chatterbox TTS backbone), **not** a real native-S2S model, and is **not in the registry** — it cannot be loaded as a model.
- ORT backend (lib/ep/cpu_tier) is **solid**: TF32-off-by-default discipline, bounded CUDA arena (unified-memory OOM guard), int8-on-CUDA refusal, CPU-tier int8 refusal, dylib pre-flight deadlock defense, empty-tensor allocation, F16 output extraction all correct. Findings there are MED/LOW edges only.

---

## Per-arm integration table

| arm | wired? | accuracy-risk | unbounded loop? | notes |
|---|---|---|---|---|
| **whisper** (STT-B) | yes | low | capped (max_length, MAX_REPEAT=24) | shared encdec loop; dtype-agnostic argmax (F32/F16); batched `decode_batch` bit-faithful; the most robust arm |
| **moonshine** (STT-B) | yes | low | capped (448, MAX_REPEAT) | shares encdec; raw-audio frontend; clean |
| **sensevoice** (STT-A CTC) | yes | low | n/a (per-frame) | uses `to_f32_vec` for fp16 logits ✓; CMVN finite-guarded; LFR ceil-correct |
| **parakeet** (STT-T TDT) | yes | **med** | capped (MAX_TOKENS_PER_STEP=10 forces t+=1) | `.as_f32()`-only on enc/joint/states → no fp16 path; loop bounded |
| **nemo_ctc** (STT-A) | yes | med | n/a | `.as_f32()`-only; shared ctc::greedy |
| **nemotron** (STT-T streaming) | yes | med (low for shipped int4) | capped (max_sym, n_enc) | bit-faithful to python ref; `.as_f32()`-only → F16 re-export breaks |
| **qwen3_asr** (STT-LLM) | yes | **high** | capped (440) + repeat guard | **hardcoded fp16 stride** on embed_tokens.bin; `.as_f32()` on audio_features; hardcoded prompt ids |
| **funasr_nano** (STT-LLM) | yes | **high** | capped (256), no repeat guard | **silent KV-cache drop on overflow** (C2); `.as_f32()` everywhere; NaN-scrub hides bugs |
| **cohere** (STT-AED) | yes | med | capped (448) + repeat guard | best fp16 handling (cast_float on hidden/KV); only `features` read is `.as_f32()` |
| **canary** (STT-AED) | yes | **high** | capped (1024), no repeat guard | **zero fp16 support** (logits/mems/argmax all f32); resolves prompt ids by surface (good) |
| **voxtral** (STT-LLM "streaming") | yes | **high** | capped (8192!), no repeat guard | **prompt truncation when n_audio<39** (C3); `.as_f32()` on audio_embeds; doc claims streaming but `as_stepped`=None |
| **kokoro** (TTS-P3) | yes | low | n/a (token cap 510) | CPU-pinned (correct: CUDA LSTM divergence); voices real; clean |
| **melo** (TTS-VITS) | yes | low-med | n/a | `_voice`/`_speed` ignored, 1 hardcoded voice; OOV spell-out good; `.as_f32()` on output |
| **supertonic** (TTS-CFM) | yes | **high** | capped (total_step) | **no fp16 path at all** (f32-pinned feeds + `.as_f32()` outputs); CFM/epoch correct; multi-voice |
| **chatterbox** (TTS codec-AR) | yes | med | capped (1000), **no repeat/cap telemetry** | AR bit-identity sound; logits F16-safe BUT vocoder output `.as_f32()`; **1 hardcoded voice, speed ignored, cloning unwired** |
| **s2s CodecArDuplexModel** | **NO (scaffold)** | **n/a — fake** | capped (MIN_PREFIX) | **synthetic hash conditioning, not real S2S; NOT in registry** (C1) |
| **diarize** | yes | med | n/a (shrinks) | real output; `.as_f32()` → F16 break; always-overlapped speaker dropped (documented) |
| **enhance** | yes | med | n/a | 3 modes wired; `get()` positional fallback can bind wrong tensor (H8); `.as_f32()` only |
| **model registry** | yes | — | — | 16 arms, no drift, typed error on unknown arch, no panic; quant-stamp gate type-safe |

---

## CRITICAL

### [CRITICAL] S2S `CodecArDuplexModel` is a synthetic scaffold, not a real native-S2S model — and is unregistered/unloadable
`crates/waav-infer-core/src/s2s/duplex_codec_ar.rs:162-187` (`user_conditioned_prefill`) · `model.rs` (no S2S arch arm)
- **What:** The "native-S2S full-duplex" model does **not** condition on user audio. Its `user_conditioned_prefill` folds the user-in frames through a hash (`acc.wrapping_mul(31).wrapping_add(c)`, salted by `turn_idx`) into `SAFE_TOK_MOD=4096` **text tokens** that prime the chatterbox **TTS** Llama backbone (`:167-184`). There is no acoustic user stream, no S2S codec input lane, no semantic-VAD trained head — the EoT "confidence" is supplied by the *caller* (`batch.eot_confidence[b]`, `:324`), not produced by the model. The module doc calls the backbone "faithful, not a cheat," but mechanically it is TTS with a hashed-input-derived prompt. Confirmed unregistered: `load_model` has no duplex/S2S arch arm; the only constructor outside tests is `provider/full_duplex_bench.rs:861` (a benchmark).
- **Why it matters:** S2S is listed as a first-class arm of the engine. As shipped there is **no loadable, real speech-to-speech model** — a caller cannot obtain one through the registry, and the one that exists fakes the defining property (read-user-audio-while-emitting). The bit-identity test it passes only proves the *TTS backbone* batches deterministically, not that any S2S behavior is correct. This is shelf-ware presented as a working arm.
- **Fix:** Either (a) integrate a real codec-S2S model (Moshi/CSM-class) with genuine user-audio codec input and a trained EoT head, registered in `load_model`; or (b) relabel this explicitly as a *batched-duplex-seam harness / perf fixture*, remove the "real native-S2S model" framing, and stop counting S2S as a delivered arm until a real one lands.

### [CRITICAL] funasr_nano silently drops KV-cache writes on overflow / short delta → silent transcript corruption
`crates/waav-infer-core/src/stt/funasr_nano.rs:193`
- **What:** `write_deltas` copies a KV delta only `if off + span <= buf[i].len() && v.len() >= span`; otherwise it **skips the write with no error and no log**, then the loop advances `past += 1` and keeps decoding against a stale/zero KV slot. The prefill write (`write_deltas(..., 0, prompt_len)`) can already exceed `cache_cap` for a long clip and be silently truncated.
- **Why it matters:** A dropped KV delta means every later token attends to a zero/old key at that position → wrong logits → wrong token → **compounds for the rest of the utterance** (the AR-compounding identity violation), producing a plausible-but-wrong transcript with zero diagnostic signal. Enterprise-fatal: silent accuracy loss.
- **Fix:** Return a typed `FunAsrError` when `off+span > buf[i].len()` (cache exhausted — a real reportable condition) or `v.len() < span` (malformed model output); never skip-and-continue. Validate `prompt_len <= cache_cap` at prefill.

### [CRITICAL] qwen3_asr hardcodes a 2-byte (fp16) stride for the external embedding table → garbage embeddings on any non-fp16 export
`crates/waav-infer-core/src/stt/qwen3_asr.rs:110` (`off = t * hidden * 2`), `:115` (`f16::from_le_bytes`), `:81` (vocab inferred as `len()/2/vocab`)
- **What:** `embed_row` assumes `embed_tokens.bin` is **always** fp16 — byte offset `t*hidden*2`, reads 2 bytes as f16. Nothing reads the real itemsize from config/manifest.
- **Why it matters:** A same-architecture export whose embed table ships fp32 or bf16 feeds **completely wrong** `input_embeds` every decode step → nonsense transcript, no error. Directly contradicts the engine's "config + weights, zero code" charter: a loadable variant silently mis-decodes.
- **Fix:** Read dtype/itemsize from config or `waav.json` (the manifest already carries precision); compute the stride from itemsize and widen accordingly. At minimum assert `embed.len() == vocab * hidden * itemsize` and error on mismatch.

### [CRITICAL] voxtral prompt truncation when `n_audio < prompt length` → corrupted scaffold on short clips
`crates/waav-infer-core/src/stt/voxtral.rs:235-240`
- **What:** `let l = prompt.len().min(n_audio);` then the BOS + 38×STREAMING_PAD (=39-token) prompt is **silently truncated** to `l`, and the prefix audio embeds are sliced to `l*hidden`. For a short utterance where the encoder yields `n_audio < 39`, BOS and part of the streaming-pad scaffold are dropped, breaking the lockstep alignment.
- **Why it matters:** Wrong prompt scaffold → garbage first tokens → AR-compounds. Short utterances are exactly the realtime/streaming case this arm is named for; the failure is silent. The left/right pad math is supposed to guarantee `n_audio >= 39`, but it is never asserted.
- **Fix:** Assert `n_audio >= prompt.len()` and return a typed error (or right-pad the audio further) rather than truncating the prompt.

---

## HIGH

### [HIGH] fp16/q4f16 path is broken across ~10 arms: graph outputs read via `as_f32()` (returns `None` for F16) instead of `to_f32_vec()`
parakeet `:133,150,201,211,215` · nemo_ctc `:65,87` · nemotron `:159,166,183,192,295` · voxtral `:154,188` · qwen3_asr `:142` · funasr_nano `:151,192,221` · cohere `:100` · canary `:185,207,265,285` · supertonic `:288,354,477,498,611,867,912,930` · chatterbox `:940` (vocoder) · melo `:172` · kokoro `:242` · diarize `:205,254` · enhance `:227`
- **What:** `TensorData::as_f32()` returns `Some` only for `TensorData::F32` (`backend-api/lib.rs:74-79`); the ORT backend **preserves** a model's fp16 outputs as `TensorData::F16` (`backend-ort/lib.rs:422`) — it does not widen. The sibling `to_f32_vec()` (`lib.rs:100`) widens F16→F32, but these ~49 sites use the non-widening `as_f32()`.
- **Why it matters:** The registry's precision machinery (`waav.json` `precision`, `$WAAV_PRECISION`, `model.rs:78-101`) can select fp16/q4f16, and several arms' docs advertise it (voxtral `:23` "drive the f16 graphs with no code change"). But the moment any of these graphs emits an F16 tensor, the arm fails with `Output(...)` / `logits dtype` / `BadDuration` at infer time — i.e. it loads, then breaks on first inference. The advertised quant capability is partly fictional. (kokoro/melo silently return an *empty* waveform on `.as_f32()→None` rather than erroring — worse: silent no-audio.)
- **Fix:** Replace `.as_f32().ok_or(...)?.to_vec()` with `.to_f32_vec().ok_or(...)?` at every intermediate/output read (widens F16, passes F32 through, rejects only genuine non-float). For *inputs* that must match an fp16 graph's declared dtype, cast via `StaticGraph::input_types()` (the graph-driven-dtype seam, already used by encdec.rs `cast_float`). cohere shows the right pattern for `hidden`; extend it.

### [HIGH] canary has no fp16/quant support at all — f32-pinned end to end including the logits argmax
`crates/waav-infer-core/src/stt/canary.rs:265` (logits `.as_f32()`), `:285` (mems), `argmax :311` (`&[f32]`)
- **What:** Unlike whisper/cohere/qwen3 (which have an `F32|F16` argmax), canary's argmax takes `&[f32]`, logits come via `.as_f32()`, decoder_mems is f32-only, no `cast_float` anywhere.
- **Why it matters:** canary-1b is large; fp16 is its natural deploy precision. Any fp16 canary export fails at the first decode step. Inconsistent with the engine's manifest-precision design and with the other four LLM-decoder arms.
- **Fix:** Give canary the dtype-agnostic argmax (`F32`/`F16` arms) and `to_f32_vec()` for encoder_embeddings / decoder_mems / logits.

### [HIGH] supertonic is f32-only by construction — CFM feeds built `NamedTensor::f32` and outputs read `as_f32()`
`crates/waav-infer-core/src/tts/supertonic.rs:251-254,275-278,326-332,855-869,900-914`
- **What:** Every feed is constructed f32 and every graph output read via `as_f32()`; no `input_types()`-driven dtype.
- **Why it matters:** Even if an fp16 `vector_estimator` were dropped in via the manifest, the f32 feeds would be rejected by the backend (dtype mismatch) or the `as_f32()` output read would return `None`. The arm cannot run any non-fp32 graph despite the precision-selection promise. (The CFM math, epoch discipline, and reproducible seeding are otherwise correct.)
- **Fix:** Make feed dtype and output extraction graph-driven (cast f32→f16 when `input_types()` says F16; read with `to_f32_vec()`).

### [HIGH] No repetition guard in voxtral, funasr_nano, or canary — a degenerate model decodes to the cap silently
voxtral `:248-297` (cap 8192, only `pos>=n_audio`) · funasr `:272` (cap 256) · canary `:244` (cap 1024)
- **What:** whisper/moonshine/cohere/qwen3 (and shared encdec `MAX_REPEAT=24`) cut a same-token repetition loop early; these three do not. A model that locks onto one token emits up to the cap of identical tokens.
- **Why it matters:** Not infinite (caps exist) but a latency/cost cliff: canary emits up to 1024 junk tokens, **voxtral up to 8192** full Mistral-26L decoder steps (~minutes of GPU) on one bad utterance, and the output is a wall of repeated text instead of a fast clean stop. The team's own bar (24) is applied inconsistently.
- **Fix:** Add the `repeat >= 24` guard these three are missing (voxtral is least severe — `pos>=n_audio` bounds it — but funasr/canary are real exposures).

### [HIGH] chatterbox AR loop has the token cap but no repetition / no-progress / cap-hit telemetry — a never-STOP model silently truncates
`crates/waav-infer-core/src/tts/chatterbox.rs:959-984` (`run_to_stop`), `837-902` (`step_slots_batched`)
- **What:** `run_to_stop` loops `0..MAX_NEW_TOKENS` (1000) and breaks only on STOP; if STOP never comes it returns a 1000-token body **with no warn/error** — the caller can't tell "finished" from "hit the cap." The stepped/batched seams (`step_slots_batched`) have **no internal cap** at all — they rely entirely on the external driver stopping; a never-STOP model or driver bug loops until force-reset. There is a repetition *penalty* (1.2) but no repetition *detector*.
- **Why it matters:** 1000 frames of audio with no observability on a stuck decode; the stepped seam can loop unbounded.
- **Fix:** `warn!` (and ideally a typed soft-signal) when `run_to_stop` exits via the cap; give the stepped seam a per-slot stride cap that errors on an un-stopping slot.

### [HIGH] Kaldi-fbank bit-faithfulness (SenseVoice + diarize) is unverified — only a 3%-WER disagreement gate, no bit-exact golden; the wespeaker/diarize path has zero accuracy gate
`crates/waav-infer-components/src/kaldi_fbank.rs:180-200` (5-cell golden @1e-2) · eval is WER-disagreement only
- **What:** The kaldi fbank's config matches `kaldi-native-fbank` defaults (preemph 0.97 after DC-removal, round-to-pow2→512, HTK mel, Hamming N−1, log floor=f32 eps), but the only gate is a 5-cell hand-entered golden at **1e-2** absolute (on log-energies ~12–21) plus a WER-disagreement ≤3% test — neither catches a real per-frame divergence (a half-bin mel offset, a power-vs-magnitude slip). voxtral/nemotron mel have maxΔ-vs-reference golden; this does not. The Povey-window `wespeaker()` path feeding `diarize.rs:72` has **no** accuracy gate.
- **Why it matters:** A genuine fbank divergence sits under 1e-2/3%-WER undetected and silently degrades hard clips / other-language SenseVoice / all diarization (which keys speaker embeddings off these features).
- **Fix:** Add a per-frame maxΔ assertion against a stored `kaldi_native_fbank` dump (the eval already imports `knf`) at <1e-3 over the whole matrix, matching the voxtral/nemotron rigor; add a feature golden for the wespeaker/diarize path specifically.

### [HIGH] Whisper mel: dither precondition + 3000-frame contract are implicit, not asserted
`crates/waav-infer-components/src/mel.rs:85-86` (frame-count formula) + kaldi_fbank `:64-112` (no dither)
- **What:** (a) Whisper's required 3000-frame output holds only because the waveform is force-padded to `N_SAMPLES` before `total_frames = 1 + (reflected.len()-N_FFT)/HOP; n_frames = min(3000, total-1)`. The "drop last frame" does whisper's `[..., :-1]` work but is correct *only* at full-30s; the invariant "n_frames==3000" is load-bearing for the encoder's positional embeddings and never asserted. (b) The kaldi path has no dither term — correct **only because** the SenseVoice/NeMo references set `dither=0`; if any consumer's reference uses dither≠0 (some kaldi recipes do) it silently diverges.
- **Why it matters:** Both are correct *today* but rest on unstated invariants; a future caller passing variable-length audio (a) or a dither≠0 reference (b) breaks silently.
- **Fix:** `debug_assert_eq!(n_frames, N_FRAMES)` (or pad the mel to 3000 explicitly); document the "reference dither must be 0" precondition at the fbank struct level.

### [HIGH] chatterbox `decode_body` does not bound `prompt_token` length before allocating the codec-decoder feed
`crates/waav-infer-core/src/tts/chatterbox.rs:907-943`
- **What:** `decode_body` concatenates `prompt_token` (cached speech-encoder output) ++ body ++ silence_tail into a `[1, st_len]` tensor with no upper bound on `prompt_token`. The AR body is capped at 1000; the prompt-token contribution is unvalidated.
- **Why it matters:** A malformed-but-loadable speech encoder emitting a degenerate/huge `audio_tokens` shape flows straight into an unbounded allocation rather than a typed error.
- **Fix:** Validate `prompt_token` length against a sane ceiling in `LmDecoder::new` (typed `Layout` error), mirroring supertonic's `MAX_TEXT_POSITIONS`.

### [HIGH] enhance `get()` positional fallback can silently bind the WRONG tensor for the recurrent state
`crates/waav-infer-core/src/enhance.rs:223-229` (call site `:184` `state_out` fallback 1)
- **What:** `get(out, name, fallback)` does `find(name).or_else(|| out.get(fallback))`. For the threaded recurrent state read (`get(&out, "state_out", 1)`), a DPDFNet variant naming its outputs differently silently binds `state` to output index 1 — which may be the enhanced spectrum, not the state — corrupting every subsequent frame with no error.
- **Why it matters:** Positional fallback defeats the name contract in exactly the config-driven-onboarding scenario the module advertises; a wrong-but-present tensor produces silently-degraded audio.
- **Fix:** Drop the positional fallback for the state read (must be exact, error if absent); keep it only for genuine single-output models.

---

## MED

### [MED] chatterbox ignores `voice` and `speed`; ships one hardcoded voice; zero-shot cloning unwired
`tts/chatterbox.rs:1160` (`synthesize(_voice,_speed)`), `:1174` (`voices()->["default"]`), `:1089` (`default_voice.wav` fixed)
- The `TtsModel::synthesize` impl discards both args; voice bank is the literal `["default"]`; conditioning is computed once from `dir/default_voice.wav`. Chatterbox's headline feature (reference-wav cloning) has all the machinery (`from_parts` takes injected conditioning) but is not wired to `voice`. A caller requesting 1.5× speed silently gets 1.0×.
- Fix: cache per-voice conditioning keyed by `voice`; return `UnsupportedParam` for a non-default voice / non-1.0 speed until wired, so it's honest.

### [MED] melo ignores `voice`/`speed`, single hardcoded voice
`tts/melo.rs:177` (`synthesize(_voice,_speed)`), `:191` (`voices()->["default"]`), `:97` (`sid: 0`)
- VITS `sid` is fixed at 0; the graph is multi-speaker but only speaker 0 is ever used; `noise_scale`/`length_scale` are constants. `length_scale` is the natural speed control and is ignored.
- Fix: map `voice`→`sid`, `speed`→`length_scale`.

### [MED] chatterbox `default_voice.wav` SR mismatch is a warn, not a resample — wrong-SR reference degrades all output
`tts/chatterbox.rs:1092-1097` — non-24kHz reference logs `warn!("…using as-is")` and feeds raw samples to the 24kHz speech_encoder, producing pitch/format-shifted conditioning for every utterance. Fix: resample to `S3GEN_SR` in-arm or make it a typed error.

### [MED] qwen3_asr / funasr / voxtral bake one export's prompt-token ids as integer literals
qwen3_asr `:26-32` · funasr `:24-30` · voxtral `:42-44` — special-token ids and the chat scaffold are magic integers tied to one community export's tokenizer. A re-exported/differently-tokenized variant of the same arch scaffolds with wrong ids and silently mis-decodes. canary (`read_token_to_id` by surface) is the correct pattern. Fix: resolve ids by token surface at load.

### [MED] nemotron detokenize collapses internal whitespace; the python reference only strips ends
`stt/nemotron.rs:223-227` vs `eval/nemotron_ref.py:121-122` — Rust does `split_whitespace().join(" ")` (collapses all runs); ref does `replace("▁"," ").strip()` (ends only). Diverges on any mid-sequence bare `▁` / adjacent `▁`-pieces. Latent (verified samples put the lang token at the start where they converge), but breaks the "bit-faithful to reference" claim. Fix: make one match the other.

### [MED] funasr_nano silently scrubs non-finite activations to 0.0 (hides NaN-producing bugs); inconsistent with 4 sibling arms
`stt/funasr_nano.rs:224-228,241-245` — replaces every non-finite encoder/embed value with 0.0 silently; voxtral/qwen3/cohere/canary don't scrub. If the int8 encoder overflows to NaN, it's zeroed and decoding proceeds with a subtly-wrong transcript. Fix: count + `warn!` the scrub, or error on non-trivial non-finite fraction; pick one policy across arms.

### [MED] supertonic CFM `total_step` is a hidden process-global with silent fallback; no per-request quality dial
`tts/supertonic.rs:139-143,31` — the number of Euler steps (the core flow-matching accuracy knob) is `$WAAV_TTS_STEPS` read once at load, `>0`-filtered else 8, fixed for the model's life, not in `ChunkMeta`, and a non-numeric env silently degrades to 8. Fix: thread through `synthesize`; `warn!` on unparseable env.

### [MED] chatterbox/supertonic argmax tie-break and repetition-penalty-over-control-tokens may diverge from the onnx-community reference
chatterbox `:1479` (lowest-index tie), `:958/:1466` (penalty history includes `START_SPEECH`) — within-engine identity holds (stepped==edge both call the same fn), but vs the reference: F16-widened logits make exact ties common, and penalizing the START control token can shift the argmax at a stride → compounds. SUSPECTED divergence (lowest-index matches numpy; torch argmax tie is unspecified). Fix: confirm reference tie + penalty-history semantics; add a cross-engine byte-identity gate on real weights.

### [MED] Edge resampler is linear-interp / one-shot / endpoint-anchored — caps end-to-end STT fidelity for any non-native-rate clip
`crates/waav-infer-components/src/resample.rs:63-80` — linear interpolation (passband error ≫ fp rounding), `out_len=(len*ratio).round()` (±1 sample drift), endpoint-anchored `step=(len-1)/(out_len-1)` (sub-sample global stretch). 8k telephony upsamples through plain linear before the (bit-faithful) mel — garbage-in. The code itself flags rubato as the production target. Fix: lift the gateway rubato `StreamResampler`; at minimum windowed-sinc on upsample + exact-ratio mapping.

### [MED] STFT non-center (causal) path pads by a full `n_fft`, which is not librosa `center=False` framing
`crates/waav-infer-components/src/stft.rs:73-79` — right-pads by `n_fft` (librosa/torch pad 0), giving `1 + len/hop` frames; the paired inverse makes the OLA round-trip self-consistent (GTCRN enhancement is fine), but a non-paired spectral consumer expecting true causal framing sees a different frame count/offset. Fix: rename/comment as OLA-causal; add a true `causal_librosa` variant if any model needs it.

### [MED] nemotron `set_language` is a no-op for all 40 locales (documented but a silent behavioral surprise)
`stt/nemotron.rs:239-258` — any supported non-auto language still sets `lang_id=0` (auto-detect), since the per-language integer ordering is unpublished. `transcribe("de", …)` == `transcribe("auto", …)`. Correctly rejects *unsupported* langs, but a caller expecting *forced* language gets auto-detect. Fix: surface the limitation, or return `Unsupported` rather than silently auto.

### [MED] diarize `gather_runs` overlap-tolerant fallback is dead code; always-overlapped speakers dropped
`diarize.rs:111-126,261-300` — a speaker with zero solo frames yields no embedding and is dropped (documented gap vs pyannote's mask-weighted separated embeddings); the `solo_only=false` fallback path is never invoked (only `true` at `:120`). Accuracy gap, not a regression. Fix: wire the fallback or remove it.

---

## LOW

### [LOW] ORT auto-probe order puts CUDA before DirectML on Windows but the comment elsewhere implies DirectML-first; cosmetic
`backend-ort/ep.rs:82` — `windows => [DirectMl, Cuda]` is fine; just noting the auto order is platform-correct. No action.

### [LOW] ORT `gpu_mem_limit` default 48 GiB is GB10-tuned; a smaller GPU with the env unset could still over-reserve toward the cap before a clean OOM
`backend-ort/ep.rs:54-57` — the 48 GiB default assumes the 121 GiB unified pool; on a 16–24 GiB discrete GPU the limit is above VRAM, so the "clean OOM instead of box-kill" guard relies on the device OOM firing first. Tunable via env, but the default is not portable. Fix: clamp the default to a fraction of detected device memory when discoverable.

### [LOW] encdec / cohere / canary / supertonic `.last().unwrap()` / `.expect()` on the hot path — guarded but fragile
encdec `:147,251,355,386,389` · cohere `:136` · canary `:250` · supertonic `:534` — all provably guarded (e.g. `generated.last()` only when `!first_step`; `binding present` built on epoch change). Not reachable by malformed input, but fragile to future edits. Fix: prefer `if let`/`expect("invariant: …")` for clarity.

### [LOW] funasr / all-arm argmax does a full-vocab linear scan with an EoS branch per element each step
funasr `:321` + every `argmax_last` — correct but O(vocab) per step with a per-element `==EOS` check on Qwen's ~151k vocab. Micro-perf, not correctness. Fix (optional): check the single winning index for EoS, not every index.

### [LOW] cohere/qwen3/encdec repetition guard (`>=24`) can clip legitimate long repeats; cohere/qwen3 stop silently
cohere `:165` · qwen3 `:180` · encdec `:193` — "ha ha ha…", repeated digits, CJK fillers, "….." that emit one subword >24× are cut. Rare WER loss; encdec warns, cohere/qwen3 don't. Fix: raise threshold or gate on a known-degenerate token; log when it fires.

### [LOW] kokoro/melo return an empty waveform (not an error) when the output tensor is absent or F16
kokoro `:238-244` (`unwrap_or_default()`) · melo returns `Output` error — kokoro's silent empty-on-missing is worse than an error (no audio, no signal). Fix: error on a missing/dtype-mismatched waveform output.

### [LOW] supertonic duration→latent sizing uses `as usize` float truncation at a ceil boundary
`tts/supertonic.rs:294,536` — `((wav_len+chunk-1.0)/chunk) as usize` can land off-by-one vs a reference computing ceil on integer samples. Solo==batched within the arm (same expression), so bit-identity holds internally; only a cross-engine risk. Fix: integer ceil on sample counts.

### [LOW] nemotron decoder→joint "no transpose" is correct only because `target_len==1`
`stt/nemotron.rs:179-188` vs `nemotron_ref.py:108-110` — ref transposes `[1,640,1]→[1,1,640]`; Rust flattens to 640 and feeds `[1,1,640]` (bit-identical for one target). A future `target_len>1` export would silently feed a transposed/garbage tensor. Not a bug today. Fix: assert `target_len==1` or add the transpose.

### [LOW] supertonic `synth_epoch` correctness depends on the monotonic counter never wrapping to a previously-bound epoch on the same session
`tts/supertonic.rs:325,582` + ORT epoch key `backend-ort/lib.rs:493` — `wrapping_add(1)` only collides after 2^64 utterances; the run_bound stale-constant footgun is otherwise correctly avoided (constants' value is fixed within an epoch, `current_step` is in the varying set). Theoretical. Fix: none required; optionally `debug_assert` monotonicity.

---

## Verified CORRECT (anti-FUD)

- **Shared encdec AR loop** (`stt/encdec.rs`): dtype-agnostic argmax (`F32`/`F16` via per-element widen, no `as_f32` trap on logits), strict-`>` first-index tie (matches numpy), `MAX_REPEAT=24` guard, run_bound epoch discipline (prefill vs cached epochs, value-change-bumps-epoch), batched `decode_batch` active-set-shrink keeps an equal-context cohort (no position drift) → bit-identical to per-slot. The strongest code in the review.
- **ORT backend** (`backend-ort/lib.rs`, `ep.rs`, `cpu_tier.rs`): TF32 off by default on CUDA (the AR-compounding batch-invariance fix), bounded CUDA arena + `kSameAsRequested` (unified-memory OOM guard), int8-on-CUDA/TensorRT typed refusal, CPU-tier int8 refusal (bf16/fp32-accumulate only), dylib pre-flight (rc.12 re-entrant-Once deadlock defense), empty-0-dim tensor allocation via session allocator, F16 output extraction in `extract_named_output`, `run_bound` bit-identical to `run`. EP fallback to CPU floor never panics.
- **Model registry** (`model.rs`): 16-arm config-arch dispatch, no list/dispatch drift, typed `BadConfig` on unknown arch (no panic), `Manifest` parse surfaces errors, the quant-stamp admission gate is type-level-safe (private fields, single `quant()` constructor behind a passing per-substrate stamp) and defaults to fp32. P-8 isolation enforced (no backend symbol in core).
- **parakeet TDT decode**: bounded (MAX_TOKENS_PER_STEP=10 forces `t+=1`), prediction-net state advances only on non-blank, duration/time-skip handling matches `onnx-asr`. **nemotron streaming**: bit-faithful to `nemotron_ref.py` (window math, cache threading, RNN-T greedy). **sensevoice**: `to_f32_vec` for fp16 logits, CMVN finite-guarded, LFR ceil-correct. **cohere**: best fp16 handling (cast_float on hidden/KV). **kokoro**: CPU-pin is correct (CUDA LSTM divergence RCA), voices real, finite-guarded speed. **whisper/voxtral mel + nemo_mel**: golden-gated / byte-identical to reference.
- **chatterbox AR bit-identity**: per-slot vs ragged-batched share `argmax_row_with_penalty`; left-aligned KV + left-justified mask is correct for the GQA `seqlens_k` op; NaN/Inf guard runs before argmax; logits F16-safe (`to_f32_vec` branch); D2H decode hoisted out of the per-frame loop. **supertonic CFM**: exactly `total_step` Euler steps with the integration inside the ONNX graph (no host-side integration to get wrong), constants bound once per epoch, reproducible seeded noise.
