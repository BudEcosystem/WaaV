# Onboard: google/medasr (Conformer-CTC medical-dictation ASR)

**Status: ONBOARDED (config + minimal code) — live WER + RTF on GB10. Byte-identical to the
sherpa-onnx reference on the native-rate clip.**

| | |
|---|---|
| Model | Google MedASR — Conformer-CTC, ~105M params, English, medical dictation (radiology) |
| Triage tier | CLEAN / P1-onnx-direct (`WaaV/INFER_TRIAGE.md`) |
| Official repo | `google/medasr` — **GATED** (Health AI Developer Foundations terms) |
| Acquired weights | `csukuangfj/sherpa-onnx-medasr-ctc-en-2025-12-25` — **ungated ONNX mirror, VERIFIED EXISTS** |
| Onboarding | config (`waav.json`) + minimal CTC glue (one new module + a parameterized shared fbank) |
| Live WER | **0.000** on clean/native-16k clips (byte-identical to sherpa); **0.056 aggregate** over 198 ref words |
| Live RTF | **CPU 0.049**, **GB10 CUDA 0.037** (both ≪ 1, faster than real time) |

---

## 1. HfApi verification (method step 1)

The triage cited `community:csukuangfj/sherpa-onnx-medasr-*`. **Verified real** via `HfApi.list_models`:

- `csukuangfj/sherpa-onnx-medasr-ctc-en-2025-12-25` (fp32, `model.onnx` 421 MB) ← used
- `csukuangfj/sherpa-onnx-medasr-ctc-en-int8-2025-12-25` (`model.int8.onnx` 154 MB)

File layout (both): `model.onnx` (single graph), `tokens.txt` (512 entries), `test_wavs/{0..5}.wav` +
`test_wavs/transcript.txt`. The triage did **not** hallucinate this one.

## 2. Acquire (step 2)

`model.onnx`, `tokens.txt`, `test_wavs/`, `README.md`, `waav.json` → `~/.cache/waav-models/medasr/`.

## 3. ONNX contract (probed directly + sherpa-onnx C++ source)

```
inputs:  x[N, T, 128]  f32   (kaldi-fbank, 128 mel bins)
         mask[N, T]    i64   (1 = valid frame, 0 = padding; all-ones for one utterance)
outputs: logits[N, T', 512] f32  (T' = T/4, subsampling_factor=4)
         logits_len[N]      i64
metadata: model_type=medasr_ctc, vocab_size=512, subsampling_factor=4, model_author=google
```

**Feature frontend** (authoritative, from sherpa-onnx `offline-recognizer-ctc-impl.h` medasr branch +
`kaldi-native-fbank`): kaldi-fbank, **128 bins**, 16 kHz, 25 ms / 10 ms, **Hanning** window
(`0.5−0.5cos(2πi/(N−1))`, non-periodic), **remove_dc_offset=false**, **preemph=0**, **dither=0**,
mel range **[125, 7500] Hz**, snip_edges, and crucially **`normalize_samples=true`** → the raw
`[-1, 1]` PCM is fed directly (NOT int16-scaled ×32768, unlike SenseVoice). No CMVN
(`FeatureNormalizationMethod()` returns empty for medasr). Decode = CTC greedy best-path, **blank = id 0**
(`<blk>`), then SentencePiece detokenize (`▁`→space, `<s>`/`</s>`/`<unk>` control surfaces dropped).

## 4. Integration — REUSE assessment (step 3)

WaaV already has Conformer-CTC infra, but **none of the existing paths matched as-is**:
- `nemo_ctc.rs` (NemoCtc): expects a separate `nemo128` **preprocessor graph** + `audio_signal`/`length`
  inputs — medasr has no preprocessor graph and a different I/O (`x`+`mask`).
- `sensevoice.rs`: kaldi-fbank but with LFR+CMVN+language/text_norm control inputs and int16-scaled audio
  — different I/O, different frontend conditioning.

The medasr frontend differs from the existing `KaldiFbank` (which hardcoded Hamming + DC-removal +
0.97-preemph + int16 input) in 4 ways: **Hanning window, no DC removal, no preemph, raw [-1,1] input**,
plus mel range [125,7500] and 128 bins. So the integration is:

**(a) SHARED component touched — `KaldiFbank` parameterized (additive, note for coordinator):**
Added a private `FbankConfig`/`WindowType` + a `KaldiFbank::medasr(num_bins, sr)` constructor, and made
`compute()` honor `remove_dc_offset`/`preemph` flags. The existing `new()` (Hamming) and `wespeaker()`
(Povey) constructors are unchanged and re-route through the same config with the prior defaults —
**verified bit-for-bit**: the `fbank_matches_kaldi_native_golden` SenseVoice golden test still passes.

**(b) NEW module — `crates/waav-infer-core/src/stt/medasr.rs` (`MedAsrCtc`):**
~110 LoC of I/O glue. Composes `KaldiFbank::medasr` → ONNX (`x`+`mask`) → **shared** `ctc::greedy`
(blank=0) → **shared** `SentencePieceTokenizer::from_tokens_txt`+`decode` (zero-code: it already
maps `▁`→space, drops `<s>`/`</s>`/`<blk>`/`<unk>`, strips leading space).

**(c) Registry — one new arm `"medasr_ctc"`** in `model.rs` (selected via `waav.json`, no HF config.json).

## 5. Smoke + Accuracy + Perf (steps 4-6) — live on GB10

Ran through the **production registry seam** (`load_model_at` → `medasr_ctc` arm) on the model's own
real medical-dictation `test_wavs/`. Test: `crates/waav-infer-server/tests/medasr_live.rs`.

```
=== MedASR [CPU]  active_ep=cpu  load=652ms ===
  0.wav  43.8s @in16000  RTF 0.049  WER 0.000   (native 16k)
  1.wav  12.0s @in24000  RTF 0.049  WER 0.000   (resampled 24k→16k)
  2.wav  29.4s @in24000  RTF 0.047  WER 0.134   (resampled; reference punctuation drift)
  ── aggregate: WER 0.056 over 198 ref words | RTF 0.049 (85.2s audio in 4.14s)

=== MedASR [AUTO] active_ep=cuda load=942ms ===   (GB10 CUDA EP)
  0.wav  RTF 0.037  WER 0.000
  1.wav  RTF 0.037  WER 0.000
  2.wav  RTF 0.036  WER 0.134
  ── aggregate: WER 0.056 over 198 ref words | RTF 0.037 (85.2s audio in 3.12s)
```

**Bit-faithfulness vs sherpa-onnx reference:** ran the official sherpa-onnx 1.13.2
`OfflineRecognizer.from_medasr_ctc` (throwaway validation tool — no serving path, per
[[waav-infer-no-venv-wrap]]) on the same clips, and diffed the **full** transcript of the native-16k
`0.wav`: **BYTE-IDENTICAL** (573 chars, including the structured `[EXAM TYPE] … {period}` markup). The
greedy CTC id stream (280 emitted ids) also matched the raw-ONNX reference exactly. The `2.wav` 0.134
is **not** an engine bug — it is resampling the 24 kHz clip (`EdgeResampler` linear vs sherpa's
resampler) plus the loose reference punctuation; sherpa's own `2.wav` output likewise drops the final
paragraph and our output matches sherpa's, not the reference text.

Aggregate WER 0.056 is right at the model's published ~4.6% radiology WER (the small excess is the two
resampled clips). RTF 0.037 on GB10 CUDA = ~27× real-time.

## 6. Tests + clippy

- `cargo test -p waav-infer-components --lib` → 46 passed (incl. fbank golden — SenseVoice path unchanged).
- `cargo test -p waav-infer-core --lib model::` → 11 passed (registry dispatch; updated the load-bearing
  `REGISTERED_ARCHITECTURES.len()` invariant to 18 — see coordination note).
- `cargo test -p waav-infer-server --test medasr_live` → 1 passed (CPU + CUDA, WER+RTF gated).
- `cargo clippy -p waav-infer-components -p waav-infer-core` → **no warnings in my files** (medasr.rs,
  kaldi_fbank.rs clean).

## Files added / changed

**Added:**
- `crates/waav-infer-core/src/stt/medasr.rs` — `MedAsrCtc` STT module (new).
- `crates/waav-infer-server/tests/medasr_live.rs` — live WER+RTF gate (new).
- `~/.cache/waav-models/medasr/waav.json` — `{"architecture":"medasr_ctc","weights":{"model":"model.onnx"}}`.

**Changed:**
- `crates/waav-infer-components/src/kaldi_fbank.rs` — **SHARED**: parameterized fbank (added
  `KaldiFbank::medasr` + `FbankConfig`/`WindowType` + `hanning()`; `compute()` honors
  `remove_dc_offset`/`preemph`). Existing `new`/`wespeaker` bit-identical (golden test passes).
- `crates/waav-infer-core/src/stt/mod.rs` — export `medasr` module (`MedAsrCtc`, `MedAsrError`).
- `crates/waav-infer-core/src/model.rs` — import `MedAsrCtc`; new `"medasr_ctc"` dispatch arm;
  added `"medasr_ctc"` to `REGISTERED_ARCHITECTURES`; updated the registry-count invariant.

## ⚠️ Coordination notes for the coordinator

1. **Shared file `kaldi_fbank.rs` touched** (additive, bit-faithful to existing callers). Sequence
   alongside any other fbank work. The change is purely additive: a new constructor + two new flags
   plumbed through `compute()`.
2. **Concurrent registry collision with in-flight MOSS-TTS work.** This is a live git repo (not a
   worktree) with other agents' uncommitted changes present (`tts/moss.rs`, `tts/mod.rs`,
   `qwen3_tts.rs`, `noise.rs`, `backend-api/lib.rs`, `ci/`). The MOSS work already added
   `"moss_tts_nano"` to `REGISTERED_ARCHITECTURES` (16→17) and set the `registry_path_a_invariant`
   count to 17; my `medasr_ctc` makes it 18, so I set the invariant to **18**. **If the MOSS work is
   reverted/rebased, the count must drop to 17.** No git commit made (per instructions).
3. **License**: weights are under Health AI Developer Foundations terms (the ungated sherpa-onnx mirror,
   not the gated `google/medasr`). Note for any redistribution policy.
```
