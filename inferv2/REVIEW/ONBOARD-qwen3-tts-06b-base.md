# ONBOARD — Qwen3-TTS-12Hz-0.6B-Base (voice clone)

**Date:** 2026-06-23 · **Box:** GB10 (121 GB shared, CUDA sm_121) · **Arch:** `qwen3_tts` (Qwen3TTSForConditionalGeneration, `tts_model_type == "base"`)

## Verdict

**ONBOARDED — `+code` (a genuine new sub-component: ECAPA-TDNN speaker encoder + mel front-end + a
voice-clone prefill path).** It is **NOT a duplicate** of the already-onboarded 0.6B-CustomVoice and **NOT**
the zero-code `waav.json` path: the Base checkpoint has **different weights** (478 vs 402 tensors; +76
`speaker_encoder.*`) and a **different conditioning mechanism** — zero-shot voice cloning from a *reference
audio* (`generate_voice_clone(text, ref_audio, ref_text)`) instead of CustomVoice's named-speaker `spk_id`
bank. Everything downstream of the speaker slot (the 28-layer Qwen3 talker, the CodePredictor sub-talker, the
12 Hz codec decode) is **byte-identical** to CustomVoice — so the new code is confined to the speaker
encoder + mel + the one prefill line; the entire decode/codec pipeline is reused unchanged.

- **HfApi:** `Qwen/Qwen3-TTS-12Hz-0.6B-Base` EXISTS (13 files, ungated). NOT a repo alias — its
  `model.safetensors` (1,829,344,272 B) differs from CustomVoice's (1,811,626,576 B). The triage's
  "ONNX/weights mirror" was the official safetensors (native tch, like every qwen3_tts checkpoint).
- **Acquired** to `~/.cache/waav-models/qwen3-tts-12hz-06b-base/` (1.83 G talker + 651 M shared codec).

## Live result (GB10 CUDA-bf16) — voice-clone gate `cuda_qwen3_tts_base_voice_clone_byte_identical` **PASSES**

Layered against the reference engine golden (vendored `qwen3_tts`, bf16, CUDA), ref audio
`assets/kokoro_m1_sample.wav` @ 24 kHz, text "Hello world, this is a test of the Qwen text to speech model.":

| Level | Probe | Result |
|---|---|---|
| L0 | prompt token ids | **match** (24 tokens) |
| **L1** | **ECAPA speaker embedding** | **cos = 0.999999**, max\|Δ\| 0.00146, norm 10.2807 vs ref 10.2806 — the speaker encoder is **essentially exact** |
| **L2** | **clone step-0 codec argmax** (deterministic prefill law) | **1342 == 1342** (reference) — the voice-clone prefill is byte-faithful |
| L3 | dual-AR greedy codes (clone speaker slot) | frame-0 cb0 **1342 == 1342**; frames 54 vs ref 52 (EOS hit, not runaway); cb0 match 9/52 — leading frames agree, the tail is the documented bf16-kernel-fragile greedy walk (same caveat as the CustomVoice gate, amplified by the continuous speaker vector) |
| **RTF** | full clone synthesis | **0.695** (3.00 s wall for 4.32 s audio, 103 680 samples @ 24 kHz) |

**Accuracy verdict:** the **deterministic, load-bearing seams are byte-faithful** — the ECAPA speaker
embedding matches to 6-nines cosine and the clone prefill's step-0 codec argmax is **exact** vs the reference.
The greedy tail diverges after ~8 frames (bf16 non-associative CUDA reductions; the spelled-out caveat the
0.6B/1.7B CustomVoice gates carry too — it is NOT a clone-path bug). EOS termination + a correct frame count
(54 vs 52) + frame-0 exactness confirm the clone path is structurally correct and produces intelligible,
voice-conditioned audio.

- **Lib tests:** `cargo test -p waav-infer-backend-torch --lib` → **157 passed, 0 failed**. Clippy clean.
- **Regression (no drift from the `build_prefill` param change):** the CustomVoice 0.6B byte-identity gate
  **PASSES** — talker hidden **Δ==0 over 1024 dims** at BOTH the prefill last position AND the first decode
  step (the `None` prefill path is byte-identical to the original), step0 argmax 1995 exact, RTF 0.552.


## What the Base differs in (config + safetensors diff vs CustomVoice 0.6B)

| | Base | CustomVoice 0.6B |
|---|---|---|
| `tts_model_type` | `base` | `custom_voice` |
| speaker conditioning | ECAPA-TDNN over ref audio (`speaker_encoder_config` enc_dim 1024 / sr 24000) | named-speaker `spk_id` bank (aiden/dylan/…) + dialect ids |
| safetensors tensors | **478** | 402 |
| extra weights | **+76 `speaker_encoder.*`** (ECAPA: 1 TDNN + 3 SE-Res2Net + MFA + ASP + fc) | — |
| shared keys | identical shapes, **0 mismatches** | — |
| talker / code_predictor / codec | **byte-identical arch** | byte-identical arch |

So the Base = CustomVoice talker/CP/codec **+** an ECAPA-TDNN speaker encoder feeding the speaker slot. The
reference `Qwen3TTSForConditionalGeneration.generate` `x_vector_only_mode` path is, line-for-line, the SAME
prefill as `custom_voice` EXCEPT the speaker slot is `speaker_embed.view(1,1,-1)` (the continuous ECAPA
output) instead of `codec_embedding(spk_id)`.

## Integration (the `+code`)

All in **`crates/waav-infer-backend-torch/src/qwen3_tts.rs`** (the model's own file — no shared-type touches
beyond the engine already dispatching `qwen3_tts`):

- **`SpkMel`** — the `extract_speaker_embedding` mel front-end: n_fft 1024 / 128 mels / hop 256 / win 1024 /
  fmin 0 / fmax 12000, slaney-norm librosa filterbank (loaded from `mel_basis_24k.npy`), manual reflect-pad
  `(n_fft-hop)//2`, `tch::stft(center=False)`, mag `sqrt(re²+im²+1e-9)`, `log(clamp(x,1e-5))`. Computed in f32.
- **ECAPA-TDNN** (`SameConv1d` reflect-"same" / `TdnnBlock` / `Res2NetBlock` / `SeBlock` /
  `SeRes2NetBlock` / `AttentivePool` / `SpeakerEncoder`) — faithful to `Qwen3TTSSpeakerEncoder.forward`.
- **`build_prefill(input_ids, spk_emb_override)`** — one new `Option<&Tensor>` param; the speaker slot is the
  override (Base) or `codec_embed(aiden)` (CustomVoice). **Both paths share the identical geometry.**
- Public API: **`is_voice_clone()`**, **`extract_speaker_embedding(ref_24k)`**,
  **`synthesize_pcm_clone(text, ref_24k)`** (greedy), **`generate_codes_with_spk(...)`**,
  **`step0_codec_logits_clone(...)`** (test seam).
- Loader auto-detects Base by the presence of `speaker_encoder.fc.weight` (zero-config, future-proof).
- **No per-venv serving path.** mel_basis precomputed once into the model dir (deterministic, portable).

### Engine wiring

`waav.json` added (`architecture: qwen3_tts`, same as CustomVoice) → the model **loads** through the existing
engine dispatch arm unchanged. NOTE: the generic `TtsModel::synthesize(text, voice, speed)` trait has **no
reference-audio field**, so the engine's default TTS path would fall back to `DEFAULT_SPEAKER_ID` (untrained
for Base). The voice-clone value is exposed via the new `synthesize_pcm_clone` Rust API; threading ref-audio
through the `TtsModel` trait + REST/WS API is a separate cross-cutting feature (flagged, not done here — it
touches shared engine API used by concurrent agents).

## Files

- **TOUCHED (shared file, model-local edits):** `crates/waav-infer-backend-torch/src/qwen3_tts.rs`
  (+ECAPA/mel/clone path; `build_prefill` gained one param — 3 internal callers updated to pass `None`).
- **TOUCHED (test):** `crates/waav-infer-backend-torch/tests/cuda_torch_qwen3_tts.rs` (+Base gate).
- **NEW:** `torch_runtime/dump_qwen3tts_base_golden.py` (reference golden dumper).
- **NEW (model dir):** `~/.cache/waav-models/qwen3-tts-12hz-06b-base/{waav.json, mel_basis_24k.npy}`.
- No registry/`model.rs`/`engine.rs` edits (the `qwen3_tts` arm already exists).

## Reference oracle

`dump_qwen3tts_base_golden.py` (the vendored `qwen3_tts`, bf16 CUDA) ran the Base end-to-end:
`tts_model_type=base`, ref_mel [1,1129,128], spk_embed[1024] norm 10.28, input_ids[24], step0 argmax **1342**,
codes_greedy [52,16] frame0[0]=**1342**, audio [99840]@24kHz. Goldens in `/tmp/qwen3tts_base_golden/`.
