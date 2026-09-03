# Onboard — IndexTeam/IndexTTS-2 INPUT-SIDE ENCODERS CLOSED (arbitrary text + zero-shot voice clone, engine-served)

**Status: THE LAST SCOPED TAIL IS CLOSED — a REAL IndexTTS-2 arbitrary-text + arbitrary-reference-audio
voice-clone synth runs end-to-end on GB10 CUDA, byte-faithful to the reference golden, engine-served.** This
session ported ALL FIVE input-side encoders (the scoped remainder from `ONBOARD-indextts2-CHAIN.md` §3) +
wired `synthesize_cloned(text, ref_audio_pcm)` into the full chain. Every deterministic seam gates
byte-identical; the flow-matching wav is within the documented CUDA tolerance.

---

## 1. Arbitrary-text + voice-clone working + byte-faithful + engine-served? — **YES, all three.**

- **Working** — `TorchIndexTts2Full::synthesize_cloned(text, ref_pcm, sr)` runs the WHOLE input→wav chain
  (resample → SeamlessM4T features → w2v-bert → spk_cond_emb; mel → ref_mel; FVQ → S_ref → length_regulator →
  prompt_condition; kaldi-fbank → CAMPPlus → style; tokenizer → text_ids; front-end → AR → gpt_latent →
  back-half → 22.05 kHz wav) and returns a real 4.17 s waveform. The previous "canned demo text" (which the
  prior session staged to a **BUGGY** `<unk>`-laden tokenization, see §2) is GONE — `synthesize` now drives the
  ported SentencePiece tokenizer, so the engine-served demo produces proper speech (98560 samples, was a
  garbage 162304).
- **Byte-faithful** — every encoder gates byte-faithful to the reference golden (`golden_encoders/` +
  `golden_clone/`). On the **strict seam** (the reference-sinc-resampled audio, bypassing the portable linear
  resampler) the **AR codes are byte-IDENTICAL (224/224)** to the reference golden, and the **wav is rel_l2
  2.2e-2** vs the f32-CPU golden (the documented BigVGAN-on-CUDA conv-accumulation drift — the back-half alone
  is 1.73e-2; audibly identical).
- **Engine-served** — `engine_serves_inprocess_torch_indextts2_byte_identical_to_standalone` → **PASS: engine
  == standalone, 98560 samples BYTE-IDENTICAL** (CUDA). No numeric transform at the seam.

### Per-encoder byte-faithfulness (CPU f32, vs the reference golden)

| encoder | what | max\|Δ\| / match | rel_l2 | verdict |
|---|---|---|---|---|
| **text tokenizer** | `bpe.model` (Unigram nmt_nfkc) + `tokenize_by_CJK_char` → ids | **7/7 cases EXACT** | — | **byte-IDENTICAL to `sp.Encode`** |
| **mel_spectrogram** | librosa-slaney mel (n_fft 1024, hop 256) → `ref_mel [1,80,1037]` | **0.0** | **0.0** | **byte-IDENTICAL** |
| **FVQ quantize** | VocosBackbone + factorized-VQ → `S_ref [1,602]` codes | **602/602 EXACT** | — | **byte-IDENTICAL** |
| **CAMPPlus** | kaldi.fbank − mean → D-TDNN → `style [1,192]` (from raw audio) | **0.0** | **0.0** | **byte-IDENTICAL** |
| **SeamlessM4T feat** | audio_16k → `input_features [1,602,160]` | 1.3e-4 | 8.9e-7 | **byte-faithful** |
| **w2v-bert-2.0** | `Wav2Vec2BertModel.hidden_states[17]` → `spk_cond_emb [1,602,1024]` | 1.4e-4 | **2.3e-6** | **byte-faithful** |

(The w2v/seamless/mel/style residuals on CUDA are accumulation drift — `style`/`mel` are bit-identical on CPU.)

### RTF
The full clone synth on GB10 CUDA: ~14 s end-to-end incl. model load (~6 GB of weights: gpt 2 GB + frontend
1.4 GB + backhalf 0.86 GB + encoders 0.12 GB + w2v-bert 1.6 GB); the w2v-bert forward + 24L GPT-2 AR (batch-1,
launch-bound) + 25-step×13L CFG-doubled DiT dominate (the AR + CFM are the deferred perf targets — the priority
here was the byte-faithful gate). The 224-code (4.17 s audio) clone: ~13 s incl. load.

---

## 2. The byte-identity scars (and a corrected upstream assumption)

1. **THE `bpe.model` IS A UNIGRAM, NOT BPE** (the CHAIN doc's "BPE SentencePiece" was WRONG). `model_type=1`
   (UNIGRAM), with the **`nmt_nfkc`** precompiled charsmap (237 KB), `add_dummy_prefix` +
   `remove_extra_whitespaces` + `escape_whitespaces` all ON, no byte-fallback (`unk_id=2`). The vocab is CJK +
   pinyin + single ASCII chars + punctuation — **there are NO English word pieces**, so English tokenizes
   char/subword-wise. Reuse: the vendored `tokenizers::models::unigram::Unigram` (Viterbi) +
   `tokenizers::normalizers::Precompiled` (the `spm_precompiled` nmt_nfkc engine) — exactly the pieces
   `SpUnigram` uses, but `SpUnigram` *rejects* a non-empty charsmap, so a parallel reader keeps it.
2. **THE PRIOR DEMO TOKENIZATION WAS A BUG.** The staged `golden/meta.json` `text_ids`
   (`[10201,10539,2,10209,…]` — mostly `<unk>`=2) came from `sp.Encode(text)` WITHOUT the reference's
   `tokenize_by_CJK_char` (which UPPER-cases + space-splits CJK). The CORRECT tokenization is
   `[11122,10209,10220,…]` (`▁HELLO`,`,`,`▁THIS`,…). The reference path is
   `TextNormalizer.normalize → tokenize_by_CJK_char → sp.Encode`; our `encode` reproduces the post-normalize
   path (CJK-tokenize + SP encode), byte-IDENTICAL for any number/abbreviation-free text.
3. **The `tn`/WeTextProcessing FST is NOT portable** (OpenFST/pynini won't build on aarch64 without
   `libfst-dev`, which needs root). It expands numbers + abbreviations + applies a punctuation `char_rep_map`.
   This is the ONLY scoped-out bit: number/abbreviation-bearing text is best-effort (the digits pass through
   verbatim); `normalize_punctuation` exposes the portable `char_rep_map` slice. Every alphabetic/CJK demo is
   byte-faithful.
4. **w2v-bert masking was the lone porting scar (caught + fixed):** the reference audio yields 602 frames with
   **1 PADDING frame** (`SeamlessM4TFeatureExtractor` `pad_to_multiple_of=2` pads 1203 raw frames → 1204 → 602
   stride-2 pairs; the padded raw frame is literal `1.0`). HF's encoder ZEROES the padded position
   (`masked_fill`) AND excludes it from attention (`-inf` key mask). Porting w2v-bert WITHOUT the mask gave
   rel_l2 3.7e-2 (a few wildly-off frames); threading the attention/conv mask + reproducing the
   `pad_to_multiple_of=2` 602nd frame dropped it to 2.3e-6. The 602-vs-601 frame count ALSO drove the
   `prompt_condition` divergence (rel_l2 0.83 → 0.0) — the 1-frame mismatch broke the `length_regulator`
   602→1037 nearest-interp alignment.
5. **w2v-bert is the macaron conformer** (`ffn1 → attn(relative_key) → conv(causal depthwise k31) → ffn2 →
   final_ln`, each macaron-FFN `·0.5`). `hidden_states[17]` = output AFTER layer 16, so only the first 17 of 24
   layers are needed (extracted to keep the file at 1.6 GB). `relative_key` attention = scaled-dot QK +
   `distance_embedding` einsum (`bhld,lrd->bhlr`), `/√head_size`; `distance_embedding [73,64]` = left_max 64 +
   right_max 8 + 1.
6. **`S_ref` is the CONTINUOUS quantized output, not indices** — `_, S_ref = semantic_codec.quantize(...)`
   keeps `quantized_out.transpose` (= `vq2emb(indices)`), the indices are DISCARDED. The clone path computes
   `s_ref()` indices then re-embeds via the back-half's already-ported `vq2emb` → `length_regulator`.

---

## 3. What landed vs scoped

**LANDED (byte-faithful, gated, engine-served):**
- The **input-side encoders** (`indextts2_encoders.rs`, owned, NEW ~1450 LOC): the SentencePiece Unigram
  tokenizer (`TorchIndexTts2Tokenizer`), the `mel_spectrogram` + FVQ-quantize (VocosBackbone + factorized VQ) +
  CAMPPlus (D-TDNN) + kaldi.fbank + SeamlessM4T feature extractor (`TorchIndexTts2AudioEncoders`), and the
  **w2v-bert-2.0** SSL conformer (`TorchW2VBert`, the genuinely-large piece — first 17 of 24 macaron-conformer
  layers). Composes `nn::{Linear}` + raw tch conv/LN/BN/STFT ops + the vendored `tokenizers` crate; **NO shared
  module edited**.
- The **voice-clone wiring** (`indextts2.rs`, owned, ADDED): `synthesize_cloned(text, ref_pcm, sr)` +
  `synthesize_cloned_from_audio` (the strict no-resample seam) + `clone_codes_from_audio` (the byte-identical
  codes gate) + `synth_pcm_with_conditioning` (the shared acoustic body) + a portable `resample_linear`. The
  `TorchIndexTts2Full` struct gained optional `encoders`/`w2v`/`tokenizer` (loaded if their weights ship;
  otherwise the staged-voice path still works). `synthesize`/`text_ids` now drive the real tokenizer (the
  buggy canned map is gone).
- The **engine arm** — unchanged (`TorchIndexTts2Full` already wired); the engine gate now exercises the
  corrected tokenization.
- Weights (`~/.cache/waav-models/indextts2/`): `indextts2_encoders.safetensors` (0.12 GB — CAMPPlus + semantic
  VocosBackbone + librosa/kaldi/seamless mel filters + w2v stats), `indextts2_w2vbert.safetensors` (1.6 GB —
  feature_projection + 17 conformer layers). `waav.json` updated.
- Goldens: `golden_encoders/*.npy` (per-encoder boundaries) + `golden_clone/*.npy` (the full reference clone
  run for the corrected demo: codes, wav, audio_16k/22k, conditioning stages).

**SCOPED-OUT (precisely, the only remainder):**
1. **The `tn`/WeTextProcessing OpenFST normalizer** (number/abbreviation expansion). NOT portable (pynini won't
   build on aarch64 without root `libfst-dev`). Alphabetic/CJK text is byte-faithful; number-bearing text is
   best-effort. This is a pure DX nicety, not a model component.
2. **A sinc resampler.** The reference uses `torchaudio.transforms.Resample` (sinc); the portable
   `resample_linear` introduces a small spectral drift for the *raw-pcm* public API (the strict
   `synthesize_cloned_from_audio` seam, fed reference-sinc audio, is byte-identical-codes faithful). A
   pure-Rust sinc resampler (or reusing `waav_infer_components::EdgeResampler`) would close this; deferred as
   non-model glue.

---

## 4. Files (absolute; NO shared edits)

- **NEW (owned)** `/home/bud/ditto/waav/waav-infer/crates/waav-infer-backend-torch/src/indextts2_encoders.rs`
  — all five encoders + the tokenizer. Composes `nn::Linear` + `tokenizers::{models::unigram::Unigram,
  normalizers::Precompiled}` + raw tch ops. NO shared `nn::`/`codec::`/`cfm::` edit.
- **EDIT (owned)** `/home/bud/ditto/waav/waav-infer/crates/waav-infer-backend-torch/src/indextts2.rs` — added
  `synthesize_cloned` + `synthesize_cloned_from_audio` + `clone_codes_from_audio` +
  `synth_pcm_with_conditioning` + `resample_linear`; the optional encoder/w2v/tokenizer fields; the real
  tokenizer in `text_ids` (removed the buggy canned map).
- **EDIT (owned)** `.../src/lib.rs` — `pub mod indextts2_encoders;` + doc.
- **NEW (owned)** `.../tests/cuda_torch_indextts2_encoders.rs` — the 6 gates (tokenizer, audio encoders,
  style-from-raw-audio, seamless features, w2v-bert hidden17, the full clone e2e).
- Weights: `~/.cache/waav-models/indextts2/{indextts2_encoders,indextts2_w2vbert}.safetensors` + `waav.json`.
- Goldens: `~/.cache/waav-models/indextts2/{golden_encoders,golden_clone}/`.
- Throwaway reference scripts (session scratchpad, NOT a serving path; reuse the `refvenv`
  transformers==4.52.1 venv): `golden_encoders.py` (per-encoder dumper), `extract_encoders.py` (CAMPPlus +
  semantic weights), `golden_clone.py` (the full reference clone run). The w2v-bert weights were extracted
  inline via `safetensors`.

## 5. Verification

- `cargo test -p waav-infer-backend-torch --test cuda_torch_indextts2_encoders -- --ignored` → **6/6 pass on
  CPU** (clone skips CPU: BigVGAN SIGSEGVs on torch-2.12/aarch64) and **6/6 on CUDA** (`WAAV_INDEXTTS2_DEVICE=
  cuda`, incl. the full clone). Tokenizer 7/7 EXACT, mel/style/S_ref byte-identical on CPU, w2v hidden17 rel_l2
  2.3e-6, clone AR codes **224/224 byte-IDENTICAL**, clone wav rel_l2 2.2e-2.
- `cargo test -p waav-infer-server --features torch --test torch_inprocess_live -- --ignored
  engine_serves_inprocess_torch_indextts2_byte_identical_to_standalone` → **PASS: 98560 samples BYTE-IDENTICAL**.
- `cargo test -p waav-infer-backend-torch --lib` → **192 passed**. `-p waav-infer-server --features torch
  --lib` → **68 passed**.
- Shared re-verify (no shared code touched): indextts2 front-end **8/8**, dia2 `cpu_fp32_codes_byte_identical`
  **544/544**, csm `cuda_csm_codes_byte_identical_to_sidecar` **LAW PASSED** (`--test-threads=1`), irodori
  **3/3**.
- `cargo clippy -p waav-infer-backend-torch -p waav-infer-server --all-targets -- -D warnings` → **clean**.
- NO `git commit`, NO `cargo fmt` (per instructions).
