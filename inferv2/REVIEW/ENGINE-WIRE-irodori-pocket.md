# Engine-wire + arbitrary-text synth: irodori + pocket-tts (TtsModel)

**Date:** 2026-06-24 · **Box:** GB10 · **Crates:** `waav-infer-backend-torch`, `waav-infer-server`

## TL;DR

Both freshly-ported flow-matching TTS are now **engine-served `TtsModel`s doing arbitrary-text synth,
byte-faithful to standalone** (pocket-tts on **CPU + CUDA**):

| Question | Answer |
|---|---|
| Both engine-served + arbitrary-text? | **YES.** `engine::load_model_at(dir, ep)` → `LoadedModel::Tts` → `.synthesize(text, …)` works for both. irodori does Japanese text→audio; pocket-tts does English text→audio. |
| pocket-tts ids byte-identical to ref tokenizer? | **YES — byte-identical** to `SentencePieceProcessor.encode(text, out_type=int)` across 10 cases (golden text, multi-word, single-char, digits/symbols, in-vocab + byte-fallback accents, fullwidth/ligature/emoji byte-fallback, multi-space collapse). And the full text→audio is **corr=1.000000, max|Δ|=4.1e-7** vs the reference `generate_audio`. |
| Live-gate results (CPU + CUDA)? | **ALL GREEN** (see below). |
| Flag shared? | `--features torch` (server) / `--features cuda` (torch backend). |
| Precise blocker? | **None.** |

## What was done

### TASK 1 — pocket-tts SentencePiece encode (the documented follow-up)

Wired the real text→token-ids path. The `tokenizer.model` at `~/.cache/waav-models/pocket-tts/` is a
**SentencePiece Unigram** model (model_type=1, byte_fallback=1, 4000 pieces: 1 unk + 3 control + 256 byte +
3740 normal; normalizer = `identity` rule, `add_dummy_prefix=true`, `remove_extra_whitespaces=false`,
`escape_whitespaces=true`, NO Unicode normalization — verified: NFKC/NFC folding does NOT occur).

The components `tokenizer.rs` only has a **decode-only** ByteLevel-BPE / SentencePiece (loads
`tokenizer.json`), so there was no reusable encode path for a `.model` protobuf. Added a clean **portable**
encoder — **NO per-system `sentencepiece` C++ dep** ([[waav-infer-no-venv-wrap]]):

**New file `crates/waav-infer-backend-torch/src/sentencepiece.rs`** (`SpUnigram`):
1. A tiny **hand-rolled protobuf wire-format reader** (varint / len-delimited / fixed32, bounds-checked, no
   `prost`/`protobuf` dep) parses `ModelProto` → `(piece, score, type)` vocab in id order + `byte_fallback`
   (detected from any BYTE-type piece) + unk_id + the normalizer flags. Rejects non-Unigram models and
   non-empty precompiled charsmaps (the only regimes it is proven byte-faithful for).
2. Drives the **Unigram Viterbi + byte-fallback** via the already-vendored
   `tokenizers::models::unigram::Unigram` (`Model::tokenize` applies the `<0xXX>` byte fallback exactly like
   SentencePiece).
3. Applies the SP **normalizer** locally (`add_dummy_prefix` + escape spaces → `▁`, identity rule).

**Verification (`sp_unigram_encode_matches_reference` unit test, GREEN):** byte-identical ids to the
reference `SentencePieceProcessor` on 10 cases. E.g. `"Hello world."` → `[2994, 578, 263]` (== golden
`text_tokens.npy`), fullwidth/ligature/emoji correctly byte-fallback-decomposed.

`pocket_tts.rs` now: auto-attaches the tokenizer in `load`/`load_with` (from the dir's `tokenizer.model`),
adds `prepare_text_prompt` (the reference frontend: strip → uppercase first → append `.` → `frames_after_eos
= guess + 2`), `tokenize`, and `synthesize_pcm(text)` (prepare → encode → `greedy` → `decode_latents`). The
existing byte-faithful golden-ids `greedy` path is untouched.

**Frame-accounting fidelity:** the reference `_autoregressive_generation` checks the
`eos_step + frames_after_eos` break **before** queueing the latent (so the break-step latent is dropped), and
the default `frames_after_eos = prepare_text_prompt_guess + 2`. `synthesize_pcm` reproduces both, so the
sample count matches the reference exactly (17280 for the test sentence, was 19200/15360 before the fix).

### TASK 2 — engine-wire BOTH as TtsModel

- **`impl TtsModel for TorchPocketTts`** (`synthesize` → 24 kHz PCM16 chunks; `voices`=["default"];
  `active_ep`).
- **`impl TtsModel for TorchIrodori`** (`synthesize` → 48 kHz PCM16; `supported_languages`=["ja"];
  `active_ep`). Added `IrodoriError → InferError`, a `SAMPLE_RATE`/`synth_defaults` const block, a
  `load_dir(dir)` engine constructor (latent core + DACVAE decoder + llm-jp tokenizer), and
  `synthesize_pcm(text)` at the byte-faithful default params (seed 0, 8-step RF, CFG text=3/spk=5).
  irodori already had the full raw-text path (`synthesize_text`: NFKC + llm-jp tokenizer) — re-used as-is.
- **engine.rs dispatch arms** in `load_torch_inprocess_model`: `"irodori"` → `TorchIrodori::load_dir(dir,
  device.raw())`; `"pocket_tts"`/`"pocket-tts"` → `TorchPocketTts::load(dir_str, device)`. Both return
  `LoadedModel::Tts`, mirroring the existing torch-TTS arms. Updated the doc + the unknown-arch error list.
- **Manifest fix:** the cache `~/.cache/waav-models/pocket-tts/waav.json` `backend` was `"torch"` (which the
  engine's `read_torch_inprocess_runtime` does NOT pick up) → changed to `"torch-inprocess"` so direct
  `load_model_at(POCKET_DIR)` dispatches in-process. irodori's cache manifest was already `torch-inprocess`.
- **Test fixtures:** `tests/fixtures/torch_inprocess/{irodori,pocket_tts}.waav.json`.

### TASK 3 — live engine-served gates

- **`tests/torch_inprocess_live.rs`**: `engine_serves_inprocess_torch_irodori_*` (CUDA) +
  `engine_serves_inprocess_torch_pocket_tts_{cpu,cuda}_*`. Each runs the standalone arm
  (`Torch{Irodori,PocketTts}::{load_dir,load}`) and the engine arm (`load_model_at(fixture, ep)`) on the same
  text and asserts the emitted i16 PCM is **byte-identical** (the engine path == the standalone path). The
  `engine_load_ep` helper selects CPU vs CUDA so pocket-tts runs on both (the runs-everywhere bar).
- **Registry/unit test:** `sp_unigram_encode_matches_reference` (torch lib) + the new reference-faithful
  `pocket_tts_text_to_audio_matches_reference` (text→wav corr≥0.999 vs the staged reference wav).

## Live-gate results

```
# engine-served (load_model_at → synthesize), byte-identical to standalone:
pocket-tts  CPU : 17280 samples — BYTE-IDENTICAL engine==standalone   ✅
pocket-tts  CUDA: 17280 samples — BYTE-IDENTICAL engine==standalone   ✅   (runs-everywhere)
irodori     CUDA: 266880 samples (JA text) — BYTE-IDENTICAL engine==standalone ✅

# standalone arbitrary-text fidelity vs the python reference:
pocket-tts text→audio (CPU f32): rust 17280 == ref 17280 samples, corr=1.000000, max|Δ|=4.098e-7 ✅
pocket-tts SentencePiece ids   : byte-identical to SentencePieceProcessor (10 cases)              ✅

# existing standalone byte-faithful gates STILL GREEN (no regression):
pocket-tts golden CPU : latents max|Δ|=4.6e-6, mimi corr=1.0, e2e corr=1.0   ✅
pocket-tts golden CUDA: latents max|Δ|=3.7e-6, mimi corr=1.0, e2e corr=0.999999 ✅
irodori latent (CPU)  : z max|Δ|=1.96e-4, wav max|Δ|=1.63e-4                  ✅
irodori GB10 CUDA     : RTF=0.159, latent max|Δ| vs golden=3.8e-4            ✅
irodori text→audio    : llm-jp tokenizer + normalize → golden latent         ✅
```

## Test/clippy status (the LAW)

- `cargo test -p waav-infer-server --features torch` → **all green** (67 lib + 18 integration binaries,
  0 failures; the new engine gates are `#[ignore]` live-GPU, run explicitly above).
- `cargo test -p waav-infer-backend-torch --lib --features cuda` → **191 passed, 0 failed** (incl. the new
  `sentencepiece` unit test).
- `cargo clippy --workspace --all-targets --features torch -- -D warnings` → **clean**.
- `cargo clippy --workspace --all-targets -- -D warnings` (default features) → **clean**.
- The registry invariant + every existing TTS engine gate stay green.
- **No `cargo fmt`** was run; `git diff --stat` shows additions only to the 6 touched files (+ 3 new files).

## Exact files changed

**New:**
- `crates/waav-infer-backend-torch/src/sentencepiece.rs` — portable SP Unigram encoder (the load-bearing piece).
- `crates/waav-infer-server/tests/fixtures/torch_inprocess/irodori.waav.json`
- `crates/waav-infer-server/tests/fixtures/torch_inprocess/pocket_tts.waav.json`
- `WaaV/inferv2/REVIEW/pocket_tts_golden/text_to_audio_ref_wav.npy` (+ `…_meta.json`) — reference text→wav for the gate.

**Modified:**
- `crates/waav-infer-backend-torch/src/lib.rs` — `pub mod sentencepiece;` + `SpUnigram`/`SpError` re-export.
- `crates/waav-infer-backend-torch/src/pocket_tts.rs` — tokenizer field + auto-load, `prepare_text_prompt`,
  `tokenize`, `synthesize_pcm` (with the reference break-before-queue trim), `impl TtsModel`.
- `crates/waav-infer-backend-torch/src/irodori.rs` — `InferError` conversion, `SAMPLE_RATE`/`synth_defaults`,
  `load_dir`, `synthesize_pcm`, `impl TtsModel`.
- `crates/waav-infer-server/src/engine.rs` — `irodori` + `pocket_tts` dispatch arms (+ doc/error-list).
- `crates/waav-infer-backend-torch/tests/cuda_torch_pocket_tts.rs` — `pocket_tts_text_to_audio_matches_reference`.
- `crates/waav-infer-server/tests/torch_inprocess_live.rs` — irodori + pocket-tts (CPU+CUDA) engine gates.
- `~/.cache/waav-models/pocket-tts/waav.json` — `backend: torch → torch-inprocess` (runtime config, not repo).

## Key design notes

- **Why not the `sentencepiece` crate:** it wraps the C++ lib (per-system dep) → violates runs-everywhere /
  no-per-venv. The hand-rolled proto reader + the already-vendored `tokenizers` Unigram is pure Rust and
  byte-faithful. The throwaway venv was used ONLY to capture reference ids/wav (validation, not serving).
- **The engine seam adds zero numerics:** both arms construct the SAME concrete `TorchIrodori`/`TorchPocketTts`
  the standalone gates drive, so the engine output is byte-identical to standalone (proven by the gates).
- **pocket-tts runs-everywhere:** the same f32 graph is byte-identical engine==standalone on CPU and CUDA;
  the per-frame greedy/Mimi path consults no RNG (temp=0), so there is no cross-device drift in the decision
  path (the CUDA wav corr vs the CPU golden is 0.999999 — irreducible f32 GEMM noise, no EOS flips).
