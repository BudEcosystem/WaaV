# Onboard: microsoft/VibeVoice-Realtime-0.5B (vibevoice_streaming)

**Status: ONBOARDED — live-verified on GB10 CUDA. Real synth + accuracy (intelligible, on-text) + RTF 0.19 (realtime).**

Date: 2026-06-23. Engine: WaaV Infer (`waav-infer`), in-process Torch (tch) backend.

## 1. Exists? — YES (ungated)

HfApi-verified `microsoft/VibeVoice-Realtime-0.5B`: **EXISTS, ungated, public** (`gated=False`, `private=False`,
`pipeline_tag=text-to-speech`, tags include `vibevoice_streaming`, `Realtime TTS`, `Streaming text input`). The
cited ONNX mirror `FluffyBunnies/vibevoice-onnx-v2` also exists (int4/int8 5-graph ONNX) but was **not** used — the
official bf16 safetensors fits the torch-inprocess backend cleanly and is the higher-fidelity source.

Acquired to `~/.cache/waav-models/vibevoice-realtime-05b/` (single `model.safetensors`, 2.0 GB, 605 tensors).

## 2. Architecture (vs the already-done VibeVoice-1.5B)

`model_type: vibevoice_streaming`, arch `VibeVoiceStreamingForConditionalGenerationInference`. It is the small,
streaming, **single-speaker** sibling of the 1.5B. Key differences, all confirmed from the checkpoint weight keys
+ the official `modeling_vibevoice_streaming*.py`:

- **Two interleaved Qwen2.5-0.5B LMs** (the "windowed" design): a base text LM `language_model`
  (24−`tts_backbone_num_hidden_layers` = **4 layers**, final norm = **Identity**) feeds a TTS backbone LM
  `tts_language_model` (**20 layers**, with its own final RMSNorm). Dims: hidden 896, 14 q-heads / 2 kv-heads,
  head_dim 64, vocab 151936, `tie_word_embeddings=false`.
- **NO acoustic/semantic ENCODER, NO semantic tokenizer** — the streaming checkpoint shipped only the acoustic
  `decoder` (276 tensors, byte-identical key layout + shapes to the 1.5B's `acoustic_tokenizer.decoder`). Voice
  conditioning is supplied as a **prefilled KV cache + last-hidden** for all four streams (`lm`/`tts_lm`/`neg_lm`/
  `neg_tts_lm`), produced offline from a reference voice (the repo ships these as `voices/streaming_model/*.pt`).
- New components: `tts_input_types` (Embedding[2,896]; +1 for text positions, +0 for speech positions),
  `tts_eos_classifier` (Linear→ReLU→Linear→sigmoid, >0.5 ends).
- Diffusion head `prediction_head` is structurally identical to the 1.5B (HeadLayer adaLN-chunk3 + FinalLayer
  adaLN-chunk2 + sinusoidal TimestepEmbedder + RMSNorm) at hidden **896** (ffn inter 2688), proj named
  `noisy_images_proj`. Sampler = DPMSolverMultistepScheduler (cosine, v_prediction, 5 inference steps), CFG 1.5.
- 24 kHz, 3200 samples/VAE-token (7.5 Hz frame rate). The generate loop: text windows of 5 tokens → base LM →
  TTS LM (splice base hidden into tail + `tts_input_types(1)`) → 6 speech steps/window, each: diffuse one latent
  (CFG'd DDPM) → streaming acoustic-decode → feed `acoustic_connector(latent)` back into BOTH TTS LMs (type=0).

## 3. Integration — REUSE-heavy, self-contained new module

New module `crates/waav-infer-backend-torch/src/vibevoice_realtime.rs` (`TorchVibeVoiceRealtime`). It **reuses**
the 1.5B's machinery directly (the VAE decoder + DDPM solver + connector are byte-identical across the family) and
only adds the streaming-specific glue:

- REUSED from `crate::vibevoice` (widened to `pub(crate)`): `VaeDecoder` / `load_decoder` / `StreamCache` /
  `SpeechConnector` / `load_connector` / `Weights` / `load_tokenizer`.
- REUSED from `crate::cfm`: `DpmSolver` (the cosine + v_prediction + DPMSolver++ schedule the config specifies).
- REUSED from `crate::nn`: `TransformerLayer` / `KvCache` / `Rope` / `RmsNorm` / `Mlp` / `Linear` / fused-causal-GQA
  SDPA — the shared Qwen2 GQA backbone primitives. The base LM is run per-layer (skipping its Identity final norm);
  the TTS LM applies its own final norm.
- NEW here: the 896-dim diffusion head, the dual-LM `QwenLM` runner, `tts_input_types`/`tts_eos_classifier`, the
  prefilled-`VoiceCache` loader, and the windowed generate loop.

Voice conditioning: the reference `.pt` voice presets are torch-pickled `DynamicCache` objects (not Rust-loadable),
so the Carter preset was converted **offline** to a flat `voice_carter.safetensors` (`<stream>.last_hidden_state`,
`<stream>.k.<i>`, `<stream>.v.<i>`) placed beside the model — the Rust loader reads it via `read_safetensors` and
seeds each `KvCache` with one `append_contiguous` of the prefilled block. (Analogous to how the 1.5B ships
`default_voice.npy`.) **No per-venv/pip serving path.**

Engine-wired in `engine.rs` (arch arms `vibevoice_realtime` | `vibevoice_streaming` |
`VibeVoiceStreamingForConditionalGenerationInference` → `LoadedModel::Tts`). `waav.json` manifest written.
`TtsModel` trait impl added.

## 4. Smoke + RTF + Accuracy (all live on GB10 CUDA)

- **Engine smoke** (`engine_serves_inprocess_torch_vibevoice_realtime`, live): loads in-process on CUDA via the
  engine dispatch, synthesizes, emits 57600 samples (18 VAE tokens) of valid 24 kHz audio. PASS.
- **RTF** (`vibevoice_realtime_rtf_and_accuracy`, live): an 8.0s render in 1.55s → **RTF 0.194** (≈5× faster than
  realtime). Confirms the "realtime" claim. rms 0.063, peak 0.539 (healthy speech amplitude). PASS.
- **Accuracy = intelligible + on-text** (sampled-diffusion ⇒ NOT bit-exact, by design). I stood up the reference
  engine (official `vibevoice` pkg, transformers-5.12 — 4 version-drift patches in a throwaway harness) and ran it
  on the SAME passage. Findings:
  - Sample-domain correlation is ~0 for **every** pair *including reference-vs-reference* (s1 vs s3 corr −0.004):
    the model draws fresh `randn` per latent with no fixed seed, so even the reference is non-reproducible
    run-to-run. Byte/sample comparison is meaningless here (expected).
  - **Spectral/distributional**: the Rust output's spectral centroid (1458 Hz), 85% rolloff (2862 Hz), ZCR (0.156)
    and energy-modulation (0.807) all fall within the reference's own seed-to-seed range.
  - **EOS-length agreement**: the Rust impl stopped at exactly 192000 samples (60 tokens / 8.0s) — the SAME length
    the reference reached on seeds 1 and 3, i.e. the dual-LM hidden trajectory + EOS classifier are faithful.
  - **Whisper transcription of the Rust output**: *"Vibois is a novel framework designed for generating expressive,
    long-form, multi-speaker conversational audio, such as podcasts."* — a near-perfect transcription of the input
    text. (The released model is unstable: some reference seeds early-EOS at 1 token, and seeds 1/3 produced garbled
    8s content — the Rust port was actually MORE intelligible than those reference seeds.)

  Net: the port produces **accurate, intelligible, on-text English speech** at realtime; the only "divergence" is
  the model's own inherent sampled non-determinism, which the reference exhibits against itself.

## 5. Gates

- `cargo test -p waav-infer-backend-torch --lib` → **161 passed / 0 failed** (incl. 2 new `vibevoice_realtime`
  unit tests: timestep-embedding layout, qdims consistency).
- `cargo clippy -p waav-infer-backend-torch --lib` → **clean**.
- Live tests (`#[ignore]`, GB10): both PASS (see §4).

## 6. Files

NEW:
- `crates/waav-infer-backend-torch/src/vibevoice_realtime.rs` — the model (TorchVibeVoiceRealtime + TtsModel).
- `crates/waav-infer-server/tests/fixtures/torch_inprocess/vibevoice_realtime.waav.json` — in-process fixture.
- `~/.cache/waav-models/vibevoice-realtime-05b/{model.safetensors, config.json, voice_carter.safetensors, waav.json}`.

SHARED-FILE TOUCHES (additive, no behavior change to existing arms; re-read-on-stale honored — engine.rs had a
concurrent edit (`higgs_v2`) that I rebased onto):
- `crates/waav-infer-backend-torch/src/vibevoice.rs` — widened `VaeDecoder` (+ `n_slots`/`forward_streaming`),
  `StreamCache` (+ `new`), `load_decoder`, `load_tokenizer` to `pub(crate)` (the ASR sibling already established
  this reuse pattern). NO logic change.
- `crates/waav-infer-backend-torch/src/lib.rs` — `pub mod vibevoice_realtime` + `pub use TorchVibeVoiceRealtime`.
- `crates/waav-infer-server/src/engine.rs` — arch dispatch arm + import + error-message list.
- `crates/waav-infer-server/tests/torch_inprocess_live.rs` — `VIBE_RT_DIR` const + 2 live tests.

## 7. Notes / follow-ups

- The released model's EOS instability (frequent 1-token early-EOS on some random draws, occasional garbled 8s
  content) is a property of `microsoft/VibeVoice-Realtime-0.5B` itself, observed identically in the reference engine
  — not a port defect.
- Only the Carter (`en`) voice preset is shipped; adding the other 24 repo presets is a pure data step (run the
  same `.pt → voice_*.safetensors` conversion; the loader already picks any `voice_*.safetensors`).
- The reference's `randn` draws on a stochastic scheduler mean a future bit-exact gate would require a seeded
  reference run + matching the exact CPU RNG stream (as the 1.5B's parity gate does); deferred because the model is
  shipped/served as a sampled generator, and the structural/distributional/intelligibility evidence above is the
  appropriate bar for a sampled TTS.
