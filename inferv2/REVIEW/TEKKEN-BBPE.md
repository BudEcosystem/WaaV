# TEKKEN-BBPE — Voxtral-4B-TTS text-path gap CLOSED

**Status: IMPLEMENTED. Token ids byte-identical to `mistral_common` Tekkenizer. `synth(text) == synth(ids)` proven bit-identical (max|Δ|=0.0) live on GB10 CUDA.**

The previously-gated `TtsModel::synthesize(text, …)` on `TorchVoxtralTts` (it returned a `BadConfig`
"tekken-v7 BBPE not yet wired" error and forced callers to `synthesize_pcm_ids(prompt_ids,…)`) now
works end-to-end. The engine drives Voxtral-TTS through `TtsModel::synthesize`, so it automatically
gains text→audio synthesis with no engine change.

---

## 1. The format (tekken-v7, Mistral) — confirmed from `tekken.json` + `mistral_common` reference

`~/.cache/waav-models/voxtral-4b-tts/tekken.json` (14.9 MB) is **not** the HF `tokenizers` JSON schema
(no `model.type`/`merges`), which is exactly why `tokenizers::Tokenizer::from_file` genuinely cannot
load it. It is a tiktoken-family byte-level BPE:

- `config.pattern` — GPT-4-style regex pre-tokenizer split. Uses a negative-lookahead `\s+(?!\S)` →
  needs `fancy-regex` (the `regex` crate has no lookahead).
- `config.default_vocab_size = 131072`, `config.default_num_special_tokens = 1000`.
- `vocab[]` = 150000 entries `{rank, token_bytes(base64), token_str}` in **rank order** (rank == index).
  The merge priority IS the rank (lower rank merged first). Only the first
  `inner_vocab_size = 131072 − 1000 = 130072` entries are used as mergeable ranks (the rest are
  unreachable padding; `_reload_mergeable_ranks(max_vocab=…)` cuts them — must NOT be merge candidates).
- `special_tokens[0..1000]` = control ids (`<s>`=1, `[AUDIO]`=24, `[BEGIN_AUDIO]`=25,
  `[REPEAT_AUDIO_TEXT]`=35, `[NEXT_AUDIO_TEXT]`=36, …).
- `audio.voice_num_audio_tokens` = per-preset-voice `[AUDIO]` placeholder count (= voice-embedding
  frame count). `casual_male` → 147.

**ENCODE** (`mistral_common` `Tekkenizer.encode(s, bos=False, eos=False)`): `tiktoken.Encoding.encode`
over the pattern split, then `id = bpe_rank + num_special_tokens` (i.e. `+1000`).

**Speech-request scaffold** (`encode_speech_request(SpeechRequest(input, voice))`, `InstructTokenizerV7`):
```
[<s>] + [BEGIN_AUDIO] + [AUDIO]*N + [NEXT_AUDIO_TEXT] + encode(text) + [REPEAT_AUDIO_TEXT] + [BEGIN_AUDIO]
```
where `N = voice_num_audio_tokens[voice]`. For `("Paris is a beautiful city!", "casual_male")` this is
`[1, 25, 24×147, 36, 42572, 1395, 1261, 15568, 5970, 1033, 35, 25]` — 158 tokens, **exactly** the
`/tmp/voxtral_golden/prompt.json` the byte-faithful AR gate already replays.

---

## 2. Implementation

### `crates/waav-infer-components/src/tokenizer.rs` — `TekkenTokenizer` (new, ~230 lines)
The tekken seam lives with the other BBPE tokenizers (`ByteLevelTokenizer`, `SentencePieceTokenizer`).
- `from_tekken_json(path)` — parse: base64-decode the first `inner_vocab_size` vocab entries into a
  `Vec<u8> → rank` map; load the special-token `str → rank` map; read `voice_num_audio_tokens`; compile
  `config.pattern` with `fancy-regex`.
- `encode(text) -> Vec<u32>` — `pattern.find_iter` pre-tokenizer split → per-piece `byte_bpe`.
- `byte_bpe(piece, out)` — the tiktoken algorithm: whole-piece fast path, else start one part per byte
  and **repeatedly merge the adjacent pair whose merged byte-string has the lowest rank** until none
  remain; emit `rank + num_special_tokens`. (All 256 byte values are in the vocab, so it always bottoms
  out.)
- `encode_speech_request(text, voice) -> Option<Vec<i64>>` — the full scaffold above; `None` for an
  unknown voice.
- Exported from `lib.rs` as `waav_infer_components::TekkenTokenizer`.

`Cargo.toml`: added `base64 = "0.22"` (project convention) + `fancy-regex = "0.13"` — both already in
the workspace lock (transitive via `tokenizers`), zero new graph.

### `crates/waav-infer-backend-torch/src/voxtral_tts.rs` — wiring
- New field `tekken: Option<TekkenTokenizer>`; `load_inner` loads `dir/tekken.json` if present
  (absent → `None`, model still serves pre-tokenized ids; **present-but-malformed → hard load error**,
  so the gap can't silently re-open).
- New `prompt_ids(text, voice) -> Res<Vec<i64>>` (validates voice against the embedding bank too) and
  `synthesize_pcm(text, voice) -> Res<Vec<f32>>` (= `synthesize_pcm_ids(prompt_ids(text,voice), voice)`).
- `TtsModel::synthesize(text, voice, _speed)` now: `voice` empty/"default" → model default
  (matches sglang-omni's `"default" → "cheerful_female"`); tekken-tokenize → AR+codec synth →
  `SynthChunks` (same `ChunkMeta::pcm16` + `pcm_f32_to_i16` convention as the other torch TTS models).
- `synthesize_pcm_ids` is unchanged and still works.

---

## 3. Validation — BYTE-FAITHFUL, verified two ways

A throwaway venv (`mistral-common 1.11.3`, **removed** after use — validation only, never a serving path,
per `[[waav-infer-no-venv-wrap]]`) produced the reference dump `/tmp/tekken_ref_vectors.json`.

### (a) Encode byte-identity vs `mistral_common` Tekkenizer
`crates/waav-infer-components/src/tokenizer.rs` tests (run against the real `tekken.json`):
- `tekken_encode_matches_mistral_common_reference` — 9 inline goldens: ascii, accents
  (`café au lait — naïve façade`), Chinese, Russian, Devanagari (Hindi), punctuation, whitespace,
  empty string. **PASS.**
- `tekken_speech_request_scaffold_byte_identical` — the 158-token `casual_male` scaffold head/tail +
  147-placeholder count + unknown-voice→None. **PASS.**
- `tekken_encode_full_corpus_vs_reference` (opt-in `--ignored`) — **33 cases byte-identical**
  (24 encode incl. emoji/CJK/Korean/Arabic/Russian/Hindi/whitespace + 9 scaffolds across 3 voices ×
  3 scripts). **PASS.** Output: `tekken full-corpus: 33 cases byte-identical to mistral_common`.

### (b) End-to-end `synth(text) == synth(ids)` — LIVE GB10 CUDA
`cuda_torch_voxtral_tts.rs::cuda_voxtral_tts_text_path_matches_reference_ids`:
```
(1) prompt ids byte-identical to golden (158 tokens)
(2) synth(text) vs synth(ids): 59520 samples, max|Δ| = 0.000e0
```
- (1) `prompt_ids("Paris is a beautiful city!", "casual_male")` == the `mistral_common`-produced golden
  `prompt.json` ids (the reference scaffold), byte-for-byte.
- (2) `synthesize_pcm(text)` PCM == `synthesize_pcm_ids(golden_ids)` PCM, **max|Δ| = 0.0** over 59520
  samples (RNG pinned with `smoke::manual_seed_all(0)` on both arms; identical prompt ids + voice ⇒
  identical AR input ⇒ bit-identical waveform).

### No regression to the existing byte-faithful AR gate
`cuda_voxtral_tts_codes_byte_identical_to_reference` still passes: semantic(cb0) 0/24 mismatches,
18-frame bit-exact prefix (≥12 required). My changes add only the text→ids front door; the AR/codec
numerics are untouched.

---

## 4. Test + lint status
- `cargo test -p waav-infer-components --lib` → **51 passed, 0 failed** (1 ignored = opt-in full corpus).
- `cargo test -p waav-infer-backend-torch --lib` → **179 passed, 0 failed**.
- `cargo clippy -p waav-infer-components -p waav-infer-backend-torch --all-targets -- -D warnings` → **clean**.
- Live (GB10 CUDA, `--ignored`): text-path gate **PASS** (max|Δ|=0.0), existing byte-identity gate **PASS**.

---

## 5. Exact files touched
| File | Change |
|---|---|
| `crates/waav-infer-components/src/tokenizer.rs` | **+`TekkenTokenizer`** (parse/encode/byte_bpe/scaffold) + 3 tests |
| `crates/waav-infer-components/src/lib.rs` | export `TekkenTokenizer` |
| `crates/waav-infer-components/Cargo.toml` | `+base64 = "0.22"`, `+fancy-regex = "0.13"` (both already in lock) |
| `crates/waav-infer-backend-torch/src/voxtral_tts.rs` | `tekken` field + `from_tekken_json` load + `prompt_ids`/`synthesize_pcm` + wired `TtsModel::synthesize` (replaced the gated stub) |
| `crates/waav-infer-backend-torch/tests/cuda_torch_voxtral_tts.rs` | **+`cuda_voxtral_tts_text_path_matches_reference_ids`** (the synth(text)==synth(ids) gate) |

Reference dump `/tmp/tekken_ref_vectors.json` is kept (feeds the opt-in full-corpus test); the
`mistral-common` venv was removed. No `git commit` performed.

---

## 6. Answers to the brief
- **Implemented?** Yes. Tekken-v7 BBPE encode + the Voxtral-TTS speech-request scaffold in pure Rust
  (`waav_infer_components::TekkenTokenizer`), wired into `voxtral_tts.rs` `synthesize(text)`;
  `synthesize_pcm_ids` still works.
- **Token ids byte-identical to `mistral_common`?** Yes — 33/33 reference cases (multilingual + the exact
  158-token Voxtral-TTS prompt), zero divergence.
- **`synth(text) == synth(ids)` proof?** Yes — live GB10 CUDA: prompt ids byte-identical to the golden,
  and the decoded waveform max|Δ| = **0.000e0** over 59520 samples.
- **Blocker?** None.
