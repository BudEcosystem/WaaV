# Voice Cloning API — widening the `TtsModel` synthesis seam end-to-end

**Date:** 2026-06-23 · **Scope:** `waav-infer` (in-process, branch `waav-infer-v2-build`) · **No commit.**

## Problem

The generic `TtsModel` synthesis seam took only `(text, voice, speed)`, so **zero-shot voice cloning
from a reference clip could not be reached over the REST/WS API** even though models implement it
behind a non-trait Rust API (qwen3-tts-Base ECAPA clone, vieneu MOSS-encode clone). Three onboards
flagged this as a recurring "fully-functional" gap.

## 1. Trait + API design

### Trait (additive, backward-compatible) — `crates/waav-infer-core/src/model.rs`

```rust
pub trait TtsModel: Send {
    fn synthesize(&mut self, text: &str, voice: &str, speed: f32) -> Result<SynthChunks, InferError>; // unchanged

    /// OPTIONAL cloning seam — DEFAULT returns the typed unsupported error.
    fn synthesize_cloned(
        &mut self, text: &str, reference_audio: &[f32], sample_rate: u32, speed: f32,
    ) -> Result<SynthChunks, InferError> {
        let _ = (text, reference_audio, sample_rate, speed);
        Err(InferError::unsupported("this model does not support voice cloning"))
    }
    fn supports_cloning(&self) -> bool { false }
    /* …existing methods unchanged… */
}
```

Key decisions:
- **`reference_audio: &[f32]` + `sample_rate`** (mono f32 PCM). The model resamples to its own native
  rate **internally** via `waav_infer_components::EdgeResampler`. When `sample_rate` already equals the
  model's native rate the resampler is a literal identity pass-through (`input.to_vec()`), and the
  cloning models bypass it entirely with a borrowed slice — so the output is **bit-identical** to the
  model's existing native-rate clone API (the gate-stamped path).
- **No `voice` arg** — the voice IS the reference clip. `speed` is kept for parity.
- **Typed error:** new `InferError::unsupported(msg)` → `ErrorCode::UnsupportedParam` (HTTP **400**,
  not retriable). Added in `crates/waav-infer-protocol/src/error.rs` (reuses the existing
  `UnsupportedParam` code — "a capability the request asked for that this model can't do").

### Wire format (server)

A **base64-encoded audio file** (wav/mp3/flac/ogg/m4a — anything the existing `ingress::decode_to_mono`
symphonia decoder reads), decoded to mono f32 + sample rate, then handed to the trait. This is the
cleanest fit: it reuses the server's existing ingress decoder (already used by
`/v1/audio/transcriptions`), is OpenAI-style (base64), and needs no new multipart handler.

**REST** `/v1/audio/speech` — two new optional JSON fields (additive; absent ⇒ OpenAI-compat path
byte-identical):
```jsonc
{ "input": "...", "voice": "...", "reference_audio": "<base64 wav/mp3/…>",
  "reference_audio_format": "wav" /* optional hint */ }
```
**Native WS** `speak` frame — two new optional fields (`#[serde(skip_serializing_if = "Option::is_none")]`
⇒ existing frames serialize byte-identically):
```jsonc
{ "type": "speak", "text": "...", "reference_audio": "<base64>", "reference_audio_format": "wav" }
```

### Routing

| Request | Route |
|---|---|
| no `reference_audio` | unchanged default-voice path (codec-AR batcher or `engine.synthesize`) |
| `reference_audio` + cloning model | `engine.synthesize_cloned` → `TtsModel::synthesize_cloned` (one-shot, off the codec-AR lockstep batcher) |
| `reference_audio` + non-cloning model | typed `unsupported_param` **400**, checked **before** admission (no wasted permit) |

The reference is base64-decoded + capability-checked **before** taking an admission permit, so malformed
clips / non-cloning models fail fast with a typed error.

## 2. Models wired

### qwen3-tts (`crates/waav-infer-backend-torch/src/qwen3_tts.rs`, +40 lines)
- `supports_cloning()` → `self.is_voice_clone()` (true iff the `-Base` ECAPA-TDNN speaker encoder is present).
- `synthesize_cloned()` → resample to native 24 kHz (identity/borrow when already 24 kHz) →
  existing **`synthesize_pcm_clone(text, ref_24k)`** (ECAPA speaker embed → talker speaker slot → dual-AR
  loop). **Did NOT touch** `synthesize_pcm_clone` / `extract_speaker_embedding` / the AR loop — the
  override is pure delegation.

### vieneu (`crates/waav-infer-core/src/tts/vieneu.rs`, +190 lines)
Cloning was previously deferred only for the seam. Wired the **MOSS encode graph**
(`moss_audio_tokenizer_encode.onnx`, present in the moss-tts-nano cache, symlinked into the vieneu dir +
named in `waav.json`):
- New `new_with_clone(…, codec_encode: Option<Box<dyn StaticGraph>>, …)`; `new(…)` delegates with `None`
  (so every existing caller — incl. `vieneu_live.rs` — is unchanged). The encode graph is **optional**:
  absent ⇒ cloning is the typed unsupported error, default-voice synthesis untouched.
- `encode_ref(ref, sr)` — byte-faithful to the reference `_encode_ref`: resample→48 kHz (identity at
  48 kHz), mono→stereo by duplication (`np.repeat(wav,2)`), `[1,2,n]` → MOSS encode → `audio_codes[0]` =
  `[T,16]` ref_codes.
- `synthesize_clone(text, ref, sr, speed)` — encode ref → build the prompt with the **`EMOTION_0`** "natural"
  leading token (the reference `_leading_token`, NOT a preset `reserved_id`) + ref_codes under
  `<|audio_ref_slot|>` rows → the **same** AR loop as the default path. Sampler seeded on `(ref_codes,text)`.
- Refactor: `build_rows(phonemes, preset)` now forwards to a shared `build_rows_with(phonemes, leading_id,
  ref_codes)`, and `generate_codes` funnels through a shared `generate_codes_from_rows(rows, …)` — so the
  default and clone paths differ ONLY in the prompt rows; the generation math is one code path.

Registry arm (`crates/waav-infer-core/src/model.rs`) loads the encode graph iff the manifest names
`codec_encode` OR `moss_audio_tokenizer_encode.onnx` exists on disk.

### voxtral_tts — NOT wired (correctly out of scope)
Voxtral conditions on a **fixed `voices.safetensors` embedding bank**, not a reference clip — it is not a
reference-audio clone model, so it inherits the default `supports_cloning()==false`. It can opt in later
via the seam if it gains a reference path.

## 3. Backward-compat proof (existing gates unchanged)

- **vieneu byte-faithful gate** (`vieneu_live.rs::vieneu_byte_faithful_and_synthesizes`, CPU EP) **PASSES
  unchanged** after the `build_rows`/`generate_codes` refactor: **0 differing codes over 300 frames**,
  codec wav **maxΔ = 0** (byte-identical), 10 voices. The default path is provably bit-identical.
- **qwen3 clone L1/L2 deterministic gates** (`cuda_torch_qwen3_tts.rs`, CUDA) **PASS unchanged**:
  ECAPA speaker-embed **cos = 0.999999**, clone step-0 codec argmax = 1342 (matches reference).
  - The L3 full-greedy-AR sub-gate (codebook-0 agreement) has a **PRE-EXISTING** divergence:
    verified **7/48 on HEAD before any of my changes**, 6/46 with my changes — i.e. my code is **not**
    the cause (the override is pure delegation to the untouched `synthesize_pcm_clone`). This is an
    existing qwen3-internal clone-AR-loop issue, orthogonal to the seam-widening task.
- **Identity argument:** at the model's native rate the new path borrows the reference slice and calls
  the model's *existing* native-rate clone API directly (no resample) ⇒ byte-identical **by construction**.
- **vieneu clone live gate** (new) proves the full pipeline end-to-end on CPU: MOSS encode → 59 RVQ
  ref_codes → EMOTION_0 clone prompt → AR loop → **3.28 s of non-silent 48 kHz audio (peak 0.78)**;
  the registry-loaded model reports `supports_cloning()==true`; a model loaded WITHOUT the encode graph
  reports `false`; a 24 kHz reference resamples + encodes correctly.

## 4. New tests

- `model.rs::synthesize_cloned_defaults_to_unsupported` (core, lib) — the trait default returns
  `UnsupportedParam`, is non-retriable, and the `synthesize` path is unaffected.
- `lib.rs::voice_cloning_routes_three_cases` (server, lib, `#[tokio::test]`) — the three acceptance cases
  via `Engine`: **(a)** no reference ⇒ unchanged default output (byte-for-byte marker); **(b)** a reference
  ⇒ the clone path is taken AND returns reference-derived audio at the model's native rate; **(c)** a clone
  request to a non-cloning model ⇒ the typed `UnsupportedParam` (HTTP 400, non-retriable), with that
  model's default path still working.
- `vieneu_live.rs::vieneu_voice_clone_end_to_end` (core, live/ignored) — the live end-to-end clone +
  encode_ref + rate-handling + registry-wiring assertions described above.
- `ws.rs` / `ws_map.rs` `speak` roundtrip tests updated for the additive frame fields (omitted-on-wire).

`cargo test -p waav-infer-core -p waav-infer-backend-torch -p waav-infer-server --features torch --lib`
→ **179 / 79(+8 ignored) / 67(+4 ignored) PASS, 0 failed.**
`cargo clippy --all-targets -D warnings` (the 5 touched crates) → **clean.**

## 5. Voxtral tekken gap (item 4 — documented, not forced)

`tekken.json` (voxtral-4b-tts) is Mistral **tekken-v7 byte-level BPE**: a `config.pattern` GPT-2-style
split regex + a rank-ordered `vocab` of **150 000 base64 `token_bytes`** — NOT the HF `tokenizers` JSON
schema (no `model.type` / `merges`), so `tokenizers::Tokenizer::from_file` **genuinely cannot load it**
(confirmed by inspecting the file). A faithful **encode** needs the full BBPE merge with tekken's rank
tie-break over that regex split — a real BBPE implementation, not a one-liner — so it is left as a
**precise documented gap** (refined comment in `voxtral_tts.rs`) rather than a half-correct encoder that
would silently mis-tokenize. It is also **orthogonal to cloning** (voxtral uses a fixed voice-embedding
bank). The engine drives `synthesize_pcm_ids` with pre-tokenized ids in the meantime.

## Exact files changed (14)

| File | Change |
|---|---|
| `crates/waav-infer-protocol/src/error.rs` | `InferError::unsupported()` (→ `UnsupportedParam`/400) |
| `crates/waav-infer-protocol/src/ws.rs` | `Speak{reference_audio, reference_audio_format}` (additive, skip-if-none) + test |
| `crates/waav-infer-core/src/model.rs` | trait `synthesize_cloned` (default Unsupported) + `supports_cloning`; vieneu registry loads optional encode graph; core trait-default test |
| `crates/waav-infer-core/src/tts/vieneu.rs` | `new_with_clone`, `encode_ref`, `synthesize_clone`, `supports_cloning`, `build_rows_with`/`generate_codes_from_rows` refactor, `EMOTION_0`, TtsModel override |
| `crates/waav-infer-core/tests/vieneu_live.rs` | live clone + encode_ref + registry-wiring gate |
| `crates/waav-infer-backend-torch/src/qwen3_tts.rs` | TtsModel `supports_cloning` + `synthesize_cloned` (delegates to `synthesize_pcm_clone`) |
| `crates/waav-infer-backend-torch/src/voxtral_tts.rs` | precise tekken-gap documentation |
| `crates/waav-infer-server/src/engine.rs` | `Engine::supports_cloning` + `Engine::synthesize_cloned` |
| `crates/waav-infer-server/src/lib.rs` | REST `/v1/audio/speech` reference-audio decode + clone routing; `speech_response` helper; 3-case server test |
| `crates/waav-infer-server/src/ws.rs` | WS `speak` clone dispatch + `speak_clone` helper |
| `crates/waav-infer-server/src/bin`/`tests/ws_live.rs`, `crates/waav-infer-provider/src/ws_map.rs`, `crates/waav-infer-server/Cargo.toml`, `Cargo.lock` | `Speak{}` constructors updated for new fields; `base64` dep |

**Cache (model artifact, not in repo):** symlinked `moss_audio_tokenizer_encode.onnx`/`.data` into
`~/.cache/waav-models/vieneu-tts-v3-turbo/` and added `"codec_encode"` to its `waav.json`.
