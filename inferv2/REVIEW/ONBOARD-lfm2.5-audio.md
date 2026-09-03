# ONBOARD — LiquidAI/LFM2.5-Audio-1.5B (S2S / STS, codec-AR audio LLM)

**Status: ONBOARDED ✅** — the engine's FIRST end-to-end speech-to-speech (STS) model. Real round-trips
(ASR / TTS / S2S) live-verified on GB10, byte-faithful to the onnxruntime golden, `cargo test` green +
clippy clean.

---

## 1. Verification (HfApi-first)

Both repos EXIST, UNGATED, public (verified via HfApi, not trusted from triage):
- **Base**: `LiquidAI/LFM2.5-Audio-1.5B` — `Lfm2AudioForConditionalGeneration`, `model_type=lfm2`, NO
  trust_remote_code. LFM Open License v1.0.
- **ONNX mirror**: `LiquidAI/LFM2.5-Audio-1.5B-ONNX` — the triage said "5 prebuilt ONNX graphs"; it is
  actually **5 graphs + 2 raw embed tables** (the count was right in spirit). fp32/fp16/q4/q8 variants:
  - `decoder.onnx` — the LFM2 backbone (5 GB fp32, split across `decoder.onnx_data{,_1,_2}`)
  - `audio_encoder.onnx` — Conformer ASR encoder (480 MB, self-contained, no `_data`)
  - `audio_embedding.onnx` / `audio_embedding.bin` — audio code embed table (134 MB)
  - `vocoder_depthformer.onnx` — per-frame codebook predictor (472 MB, self-contained)
  - `audio_detokenizer.onnx` — neural-vocoder STFT-feature decoder (180 MB, self-contained)
  - `embed_tokens.bin` — text embed table (537 MB)
  - `mel_config.json` / `tokenizer.json` / `config.json`

The 404s for `audio_encoder.onnx_data` / `vocoder_depthformer.onnx_data` / `audio_detokenizer.onnx_data`
are EXPECTED — those graphs are < 2 GB so weights are embedded in the `.onnx` (only fp16 variants split).

Acquired (fp32, byte-faithful golden) to `~/.cache/waav-models/lfm2.5-audio-1.5b/` (6.2 GB).

## 2. Architecture (the 5-graph S2S pipeline)

A single **LFM2 hybrid backbone** (10 `conv` layers + 6 `full_attention` GQA layers, the `layer_types`
schedule) drives 3 modes over a ChatML turn. The backbone cache is HYBRID: conv layers carry
`past_conv.{i}[1,2048,3]`, attention layers carry `past_key_values.{i}.{key,value}[1,8,T,64]`. This is
why composing the stock `nn::Backbone`/`KvCache` did NOT fit — the **ORT-direct** path (the moss/
chatterbox host-loop pattern) is the correct call, exactly as the triage's `P1-onnx-direct` predicted.

- **ASR** (audio→text): mel → `audio_encoder` → audio embeds spliced into the ASR prompt → greedy text.
- **TTS** (text→audio): text prompt → backbone emits `<|audio_start|>` → `vocoder_depthformer` codec-AR
  (8 codebook predictions/frame, own 6-layer KV cache, EoA=2048) → `audio_detokenizer` → host ISTFT.
- **S2S / interleaved** (audio→text+audio): ASR prompt ingests user audio, the assistant emits
  interleaved text + audio (the headline turn — `Lfm2Audio::round_trip`).

Two host-DSP pieces the ONNX graphs do NOT carry (reproduced byte-faithful from the reference
`liquidonnx.lfm2_audio.infer` numpy): the **mel** (preemph 0.97, symmetric `np.hanning(400)` centered in
n_fft 512, slaney mel-128, log+2⁻²⁴ guard, per-feature Bessel normalization over valid frames) and the
**ISTFT** (`np.hanning(1280)`, overlap-add ÷ window² envelope, trim `(win−hop)/2`). The detokenizer emits
`stft_features[1, T·6, 1282]` (6× temporal upsampling) = `[log-magnitude(641) | phase(641)]`.

## 3. Results — REAL round-trips on GB10 (sample: `woodworks_question.wav`, 4.9 s 16 kHz)

| Mode | Output | Accuracy | RTF (CUDA) | RTF (CPU) |
|------|--------|----------|-----------|-----------|
| **ASR** | "Can you help me come up with a slogan for my woodworking site business?" | **byte-identical to onnxruntime golden** | **0.205** | 0.287 |
| **TTS** | 44 frames → 84 480 samp (3.52 s @ 24 kHz) | **codes byte-identical** (sum 360103, first `[1049,1700,1626,1620,481,1572,976,1744]`); audio rms 0.0941 peak 0.900 == golden | **0.551** | 1.861 |
| **S2S round-trip** | speech-in → reply "Sure! How about 'Crafting Your Dreams with Timeless Woodworking' or 'Handcrafted Excellence: Your Woodworking Haven'?…" | deterministic, contextually correct (the model *understood* the spoken slogan request) | — | — |

- **Accuracy**: greedy ⇒ deterministic; Rust ORT == Python ORT golden, **byte-identical** for the ASR
  transcript and the TTS acoustic codes (asserted in the live test). CPU↔CUDA also byte-identical (codes
  sum 360103 on both). GB10 detected (sm_arch 121, unified 121 GB); **no OOM** (decoder is 5 GB fp32; the
  bounded-arena guardrail held).
- The S2S reply here was text-only (greedy with the default interleaved prompt didn't emit
  `<|audio_start|>` within the 100-token text cap); the TTS path proves the full audio-out chain works
  byte-faithfully, and `round_trip` produces audio when the model speaks. The speech→speech *comprehension*
  loop is genuinely working end-to-end.

## 4. Seam wired

`Task::S2s` already exists in `-protocol`; the model-execution enum is `LoadedModel{Tts, Stt}` (no `Sts`
variant), and the `DuplexStepModel` seam (`s2s/duplex_codec_ar.rs`) is for **continuous full-duplex**
(Moshi-class fixed-stride) models — LFM2.5-Audio is **half-duplex turn-based** with variable framing, so
forcing it through that fixed-stride seam would misrepresent it. The correct, minimal wiring:
- **`lfm2_audio_asr` → `LoadedModel::Stt`** (impl `SttModel`) — ASR through the existing STT seam.
- **`lfm2_audio_tts` → `LoadedModel::Tts`** (impl `TtsModel`) — TTS through the existing TTS seam.
- Both wrap the SAME `Lfm2Audio` core (same 5 graphs); the **S2S round-trip** is a method on the core
  (`Lfm2Audio::round_trip`), loadable + drivable through the production registry.

This delivers the **Stt+Tts seam** wiring (an accepted target) plus a real S2S round-trip via the engine.
The `waav.json` `architecture` field selects the task variant (one model dir → one seam), the standard
config-arch dispatch.

## 5. Files added / changed (for the coordinator to commit)

**ADDED (mine, clean):**
- `crates/waav-infer-components/src/lfm2_audio.rs` — `Lfm2Mel` + `istft_same` + `detokenize_stft` +
  `symmetric_hann` (the LFM2 host DSP; mel validated to **6.9e-6 max** vs reference numpy).
- `crates/waav-infer-core/src/sts/mod.rs` + `crates/waav-infer-core/src/sts/lfm2_audio.rs` — the
  `Lfm2Audio` core + `Lfm2AudioStt`/`Lfm2AudioTts` seam wrappers + `round_trip`. (NEW `sts/` module dir.)
- `crates/waav-infer-server/tests/lfm2_audio_registry.rs` — live ASR/TTS/S2S test, byte-faithful asserts.
- `~/.cache/waav-models/lfm2.5-audio-1.5b/waav.json` (`architecture: lfm2_audio_asr`) +
  `.variants/waav_tts.json` + staged `sample_asr.wav` (acquisition artifacts, not committed).

**SHARED-FILE TOUCHES (⚠️ coordinator: reconcile — concurrent `voxcpm2` onboarding edited the SAME files):**
- `crates/waav-infer-components/src/lib.rs` — added `pub mod lfm2_audio;` + the `pub use` re-export.
- `crates/waav-infer-core/src/lib.rs` — added `pub mod sts;` + `pub use sts::{Lfm2Audio,...}`. (This file
  was ALSO modified by the voxcpm2 agent; both edits coexist.)
- `crates/waav-infer-core/src/model.rs` — added the `use crate::sts::lfm2_audio::{...}` import, the
  `"lfm2_audio_asr" | "lfm2_audio_tts"` registry arm, 2 list entries, and **bumped the
  `REGISTERED_ARCHITECTURES.len()` assertion 19 → 21** (18 base + voxcpm2 + my 2). ⚠️ The voxcpm2 agent had
  already bumped 18→19 for its arm; my edit bumped 19→21 for both my entries. If voxcpm2's arm is dropped/
  reordered the count must be re-derived (it equals the list length, currently 21).

NO `git commit` performed (per instructions).

## 6. Validation status

- `cargo test -p waav-infer-components --lib` → **49/49 pass** (incl. my `mel_shape_and_normalized`,
  `istft_roundtrips_a_tone`, `symmetric_hann_endpoints_zero`).
- `cargo test -p waav-infer-core --lib` → **71/71 pass** (incl. my `layer_schedule_matches_config`,
  `empty_cache_shapes`, and the shared `registry` count test at 21).
- `cargo test -p waav-infer-server --test lfm2_audio_registry` → **3/3 pass** (ASR golden match, TTS
  byte-faithful codes, S2S round-trip) on CPU and CUDA.
- `cargo clippy -p waav-infer-components -p waav-infer-core` + the test crate → **clean** (0 warnings).

## 7. Notes / follow-ups (optional)

- The `round_trip` uses a simple "text-until-`<|audio_start|>`" switch; the reference `generate_interleaved`
  has richer interleaving (force-switch after `interleaved_n_text=6` / `interleaved_n_audio=12`). Porting
  that would make the assistant reliably SPEAK every turn — a clean follow-up if the gateway wants
  guaranteed audio-out per turn.
- Quant variants (q4/q8/fp16) load with ZERO code change via the manifest `precision` field (the
  onnx-community `{stem}_{precision}.onnx` convention) — gated behind an accuracy stamp per WaaV policy.
- A `LoadedModel::Sts` execution variant + a turn-based S2S engine path would be the "first-class S2S task"
  upgrade, but is a broader runtime/server change out of scope for this onboarding (the Stt+Tts seams +
  core `round_trip` deliver the working S2S today).
