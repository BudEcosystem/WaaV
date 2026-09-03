# Onboard: OpenMOSS-Team/MOSS-TTS-Nano-100M (global/local codec-AR TTS)

**Status: ONBOARDED (config + minimal code) — live RTF on GB10 + BYTE-FAITHFUL accuracy. The Rust ORT
pipeline reproduces a Python-onnxruntime golden byte-for-byte (896/896 acoustic codes, 56/56 frames),
and the SentencePiece encoder is byte-identical to the reference `sentencepiece` across 11 multilingual
texts. Synthesizes real 48 kHz audio end-to-end through the production registry seam.**

| | |
|---|---|
| Model | MOSS-TTS-Nano — ~0.1B multilingual codec-AR TTS, 20 languages, native **48 kHz stereo** |
| Arch | `moss_tts_nano` — GPT-2-style *global* transformer (h768, 12L, RoPE θ1e4, 32k ctx) + 1-layer *local* transformer emitting a 16-codebook frame/step; `MossTTSNanoForCausalLM` |
| Triage tier | CLEAN (`WaaV/INFER_TRIAGE.md`) |
| Official repo | `OpenMOSS-Team/MOSS-TTS-Nano-100M` — **ungated** (pytorch_model.bin + custom modeling) — VERIFIED EXISTS |
| Acquired weights | `OpenMOSS-Team/MOSS-TTS-Nano-100M-ONNX` + `OpenMOSS-Team/MOSS-Audio-Tokenizer-Nano-ONNX` — **ungated ONNX mirrors, VERIFIED EXIST** |
| Onboarding | **config + minimal code** — one new core module (`tts/moss.rs`) + a registry arm + 2 tiny shared helpers |
| Accuracy | **BYTE-FAITHFUL**: codes 896/896 == Python-ORT golden (0 diffs); tokenizer byte-identical to reference SP (11 texts incl. zh/fr/ru/ar/ja/ko/emoji) |
| Live RTF | **CPU 0.52**, **GB10 CUDA 0.21** (both ≪ 1, faster than real time) |

---

## 1. HfApi verification (method step 1)

The triage cited an ONNX mirror. **Verified real** via `HfApi.list_repo_files` (HF_TOKEN), and the
triage did NOT hallucinate it — both the TTS LM *and* its codec ship ungated ONNX mirrors:

- `OpenMOSS-Team/MOSS-TTS-Nano-100M` — original: `config.json`, `modeling_moss_tts_nano.py`,
  `prompting.py`, `tokenizer.model` (SentencePiece), `pytorch_model.bin`.
- `OpenMOSS-Team/MOSS-TTS-Nano-100M-ONNX` — a **5-graph browser pipeline**: `moss_tts_prefill.onnx`,
  `moss_tts_decode_step.onnx`, `moss_tts_local_fixed_sampled_frame.onnx`, `moss_tts_local_cached_step.onnx`,
  `moss_tts_local_decoder.onnx` (+ `*_shared.data` external weights, `tts_browser_onnx_meta.json`,
  `browser_poc_manifest.json` with prompt templates + builtin voices + generation defaults).
- `OpenMOSS-Team/MOSS-Audio-Tokenizer-Nano-ONNX` — the codec: `moss_audio_tokenizer_decode_full.onnx`
  (+ `decode_step` streaming, `encode`), `codec_browser_onnx_meta.json`.

## 2. Acquire (step 2)

`snapshot_download` of both ONNX repos → `~/.cache/waav-models/moss-tts-nano/{tts-onnx,codec-onnx}`
(642 MB + 87 MB). Composed into the registry layout (symlinks, no copy): `onnx/` (3 used TTS graphs +
shared data), `codec/` (decode_full + shared data), `tokenizer.json`, `waav.json`, `golden.json`.

## 3. The ONNX pipeline (probed directly + the reference `modeling_moss_tts_nano.py`)

The authors decomposed the global/local AR into a fused browser pipeline. Per the reference
`_iter_generation_events`, the model lays text/audio into a **17-wide row grid** (col 0 = text token,
cols 1..16 = the 16 audio codebooks). The graphs used:

```
prefill(input_ids[1,P,17] i32, attention_mask[1,P] i32)
   → global_hidden[1,P,768] f32, present_{key,value}_{0..11} f32         (12-layer KV)
decode_step(input_ids[1,1,17] i32, past_valid_lengths[1] i32, past_{k,v}_{0..11})
   → global_hidden[1,1,768], present_{k,v}_{0..11}                       (advances 1 frame)
local_fixed_sampled_frame(global_hidden[1,768], repetition_seen_mask[1,16,1024] i32,
                          assistant_random_u[1] f32, audio_random_u[1,16] f32)
   → should_continue[1,1] i32, frame_token_ids[1,16] i32                 (FUSED local AR + sampler)
codec: decode_full(audio_codes[1,T,16] i32, audio_code_lengths[1] i32)
   → audio[1,2,3840·T] f32  (48 kHz stereo)
```

**Key simplification:** `local_fixed_sampled_frame` is a *fused frame sampler* — it internally runs the
1-layer local transformer over all 17 channels (the text-slot decision + 16 audio codebooks), with the
generation-defaults constants (text/audio temp/top-p/top-k + **rep-penalty 1.2**) baked in, and emits the
16 codes + `should_continue` given a per-channel `repetition_seen_mask` and pre-drawn inverse-CDF `u`
values. So the host loop is tiny: prefill → {frame → write codes back via decode_step} until
`should_continue==0` → codec decode. (The `local_cached_step`/`local_decoder` graphs expose raw logits;
not needed for the fused path.)

Per-frame host bookkeeping (verified against the reference): mark each emitted code `c` seen via
`repetition_seen_mask[c, code]=1` (confirmed to shift the sampled codes — the rep-penalty state); build
the next `decode_step` row as `[<assistant_slot>=9, code0..15]`.

## 4. Integration — REUSE assessment (step 3)

**Decision: ORT-direct (the chatterbox/supertonic codec-AR pattern), NOT a tch reimpl.** The model ships
a complete, authors-blessed ONNX pipeline (the browser POC is fully ONNX), so a tch port of the
global/local transformer would only ADD a cross-runtime float delta vs the reference, the opposite of
byte-identity. WaaV already has the exact archetype: **`tts/chatterbox.rs` / `tts/supertonic.rs`** — a
multi-graph ORT codec-AR TTS where the AR loop runs in host Rust and every matrix op is inside the shared
ONNX graphs (`waav_infer_backend_api::StaticGraph` / `OrtModel`), NO pip/venv. `tts/moss.rs` composes
that seam.

**Reused:** `StaticGraph`/`OrtModel` (all 4 graphs), `NamedTensor`/`TensorData` (i32/f32 I/O),
`waav_infer_components::{audio::f32_to_i16, GaussianNoise}` (the PCG32 sampler for the `u` draws),
`tokenizers::Tokenizer` (the SentencePiece encoder), the `Manifest` weights-map resolver, and the
`load_model` config-arch dispatch. **Only the MOSS glue** (the chat-template prompt builder, the 17-row
layout, the fused-frame loop + rep-mask, the codec downmix) lives in the new module.

### The tokenizer (the one non-trivial piece)

MOSS ships a raw SentencePiece **BPE** `tokenizer.model` (nmt_nfkc + `remove_extra_whitespaces` +
**byte_fallback** + **11 USER_DEFINED/CONTROL pieces** like `<user_inst>`, `<|im_start|>`). WaaV's
existing `SentencePieceTokenizer` is decode-only, and `transformers`' `SpmConverter` errored on this
HF/transformers version. **Solution:** an offline converter (`build_tokenizer_json.py`, throwaway
validation tool — NOT a serving dep) builds a HF `tokenizer.json` directly from the SP proto:
BPE vocab + score-ranked merges, `Precompiled(nmt_nfkc charsmap)` + `Replace(\s+→' ')` + `Strip()`
normalizers, `Metaspace` pre-tokenizer/decoder + `ByteFallback`, and the 11 user-defined/control pieces as
`normalized=True, special=False` AddedTokens (matched after whole-string normalization → preserves SP's
metaspace-before-user-token semantics). **Verified byte-identical** to the reference `sentencepiece`
encode across 11 texts (en/zh/fr/ru/ar/ja/ko/emoji/whitespace edges) AND the full 92-token prompt. The
Rust loads this `tokenizer.json` via `tokenizers::Tokenizer::from_file` — the same pattern voxtral/dia2/ark
use. The chat template (`prompting.py`) is reproduced as `[raw id] + encode(chunk)` with chunk boundaries
at the raw-id insertions (avoids the SP user-token-at-start edge case; byte-identical to per-section encode).

## 5. Smoke + accuracy (steps 4-5)

A Python-onnxruntime prototype (`/tmp/moss_proto.py`, throwaway) first validated the orchestration:
43 frames → 3.44 s of 48 kHz stereo, RMS 0.11, terminating naturally via `should_continue`.

**The accuracy bar (byte-faithful, the codec-AR law):** since both runtimes call the SAME ONNX graphs via
`onnxruntime`, identical (`global_hidden`, mask, `u`) inputs ⇒ identical codes (ORT determinism). The
golden (`/tmp/moss_golden.py` → `golden.json`) drives the pipeline with a fixed LCG `u`-schedule and dumps
the `[T,16]` codes. The Rust test (`moss_live.rs::moss_byte_faithful_codes_vs_golden`) drives `generate_codes`
with the **same LCG** and asserts byte-identity:

```
prompt_ids: 92/92 match    LCG u-stream: matches (f64 division → f32, matches Python)
frames: 56/56              BYTE-FAITHFUL CODES: 896/896 match (0 diffs)
```

(`u→0` selects the top-CDF greedy draw; the fused frame graph is verified deterministic for identical
inputs. The model is inherently sampled — `do_sample=True` by default — so fixed-`u` cross-runtime
byte-identity is the correct faithful bar, exactly as for the other sampled codec-AR TTS.)

## 6. Live RTF on GB10 (step 6)

`moss_live.rs::moss_synthesizes_real_audio` (88-char sentence, warmup + steady-state):

| EP | synth wall | audio | RTF | audio quality |
|---|---|---|---|---|
| CPU (ort) | 3433 ms | 6.56 s @ 48 kHz | **0.523** | peak 1.07, rms 0.150 |
| GB10 CUDA | 1349 ms | 6.56 s @ 48 kHz | **0.206** | identical (deterministic w/ same seed) |

(Native is 48 kHz **stereo**; downmixed L+R to the engine's mono PCM16 canon. Vocoder peak >1.0 is normal
neural-vocoder overshoot; `f32_to_i16` clamps.)

The **production registry path** (`moss_registry.rs` → `engine::load_model_at(dir, Cpu)` →
`LoadedModel::Tts` → `synthesize`) also passes: 192000 samples @ 48 kHz, i16 rms 3409 — confirming the
`waav.json` arch dispatch + weights-map resolve exactly as the server does.

## 7. Files added / changed (for the coordinator to commit)

**New (model module + registry):**
- `crates/waav-infer-core/src/tts/moss.rs` — the `MossTts` model (the whole arch; ~390 lines).
- `crates/waav-infer-core/src/tts/mod.rs` — `pub mod moss; pub use moss::{MossError, MossTts};`
- `crates/waav-infer-core/src/lib.rs` — added `MossTts` to the crate-root `tts::` re-export.
- `crates/waav-infer-core/src/model.rs` — the registry arm (`"moss_tts_nano" | "MossTTSNanoForCausalLM"
  | "MossTTSNanoForConditionalGeneration"` → loads prefill/decode_step/frame/codec_decode →
  `LoadedModel::Tts(MossTts)`), the `use crate::tts::moss::MossTts;` import, and the
  `REGISTERED_ARCHITECTURES` entry.

**Shared-code touches (small, additive — NOTE for review):**
- `crates/waav-infer-components/src/noise.rs` — `GaussianNoise::next_uniform01()` (surfaces the existing
  PCG32 `next_open01` for inverse-CDF samplers; the codec-AR `u`-draw seam).
- `crates/waav-infer-backend-api/src/lib.rs` — `TensorData::as_i32()` (the i32 sibling of `as_i64`; the
  MOSS graphs emit i32 `should_continue`/`frame_token_ids`).

**Tests:**
- `crates/waav-infer-core/tests/moss_live.rs` — byte-faithful-vs-golden + real-audio/RTF (CPU; CUDA via
  `MOSS_CUDA=1`).
- `crates/waav-infer-server/tests/moss_registry.rs` — the production `load_model_at` → synthesize path.

**Model artifacts** (`~/.cache/waav-models/moss-tts-nano/`, NOT committed — coordinator decides
distribution): `waav.json`, `tokenizer.json` (1.3 MB, byte-identical to SP), `build_tokenizer_json.py`
(reproducible offline converter), `golden.json` (the accuracy fixture), `onnx/` + `codec/` (symlinks to the
acquired ONNX).

**Gates:** `cargo test -p waav-infer-core --lib` 69/0 green; `-p waav-infer-backend-api`/`-components`
73/0 + 46/0 green; clippy clean on core/components/backend-api/server; both live tests green.

## 8. Notes / follow-ups (non-blocking)

- **No torch backend needed** — pure ONNX, the default build links nothing extra.
- **Streaming**: the codec also ships `decode_step` (cached streaming decode) and the LM `should_continue`
  is per-frame, so this is a natural fit for the `as_stepped`/codec-AR streaming seam later (not wired
  here; one-shot `synthesize` is complete and faster-than-realtime).
- **Voice cloning**: the reference + `browser_poc_manifest.json` support reference-audio prompts
  (builtin voices carry `prompt_audio_codes`); the current module does continuation mode (zero-shot
  default voice). Adding a `voice_clone` path = feed the prompt-audio rows + the encode graph (additive,
  no new arch).
- **Stereo→mono**: the engine canon is mono PCM16; the native 48 kHz stereo is downmixed. A future
  multi-channel ChunkMeta could carry stereo natively.
