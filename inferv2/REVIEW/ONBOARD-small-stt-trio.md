# Onboarding triage — "small STT/SR" trio: weya-ai/hush, YatharthS/NovaSR, YatharthS/LavaSR

**Date:** 2026-06-23 · **Host:** GB10 (`free -g`: 121 GB total, 42 GB free, 63 GB swap) ·
**Scope:** triage + onboard the three models from `WaaV/INFER_TRIAGE.md` rows 20/21/62, reusing
existing WaaV infra. **No git commit. No Rust touched** (none was a drop-in; see dispositions).

## Headline verdict

All three were listed in the triage as **STS**, and the prompt framed them as "STT/SR". **HfApi
live-verification confirms none of the three is ASR / speech-to-text.** All three are
`pipeline_tag: audio-to-audio` — the "SR" means **audio super-resolution / bandwidth-extension**,
not *speech recognition*. They are speech *enhancement* models (denoise / upsample), which is a
different task family from the STT infra (whisper / CTC / transducer / AED / sensevoice / qwen3-asr)
the prompt pointed at. The triage doc itself already flagged this ("NOT speech-to-speech", "NOT
speech-to-text", "MISCATEGORIZED as STS"); live verification corroborates it.

Consequently the requested reuse premise ("reuse WaaV's existing STT families") **does not apply** —
there is no STT transcribe/WER/RTF to produce, because none of these models transcribes. The honest
deliverable per the task LAW is therefore a precise **disposition** for each (exists? arch? why not a
drop-in?). One model (hush) was acquired and its weights smoke-validated to confirm authenticity.

| model | exists | gated | pipeline | real ASR? | arch | onnx? | disposition |
|---|---|---|---|---|---|---|---|
| weya-ai/hush | YES | no | audio-to-audio | **No** (enhancement) | DeepFilterNet3 (DfNetSE) | yes (3-graph split) | **SKIP-for-now** — real + valid weights, but needs a NEW DFNet3 multi-graph + ERB/deep-filter pipeline WaaV lacks; not a config+weights reuse |
| YatharthS/NovaSR | YES | no | audio-to-audio | **No** (super-res) | undisclosed 52 KB upsampler (GitHub-only) | no | **SKIP** — not ASR; no arch in repo, no ONNX; needs external-repo reconstruction (out of scope) |
| YatharthS/LavaSR | YES | no | audio-to-audio | **No** (bandwidth-ext) | **Vocos** (mel→ConvNeXt→ISTFTHead) + UL-UNAS denoiser | no | **SKIP** — not ASR; new vocoder-style arch, PyTorch-only, no enhancement path fits |

---

## weya-ai/hush — DeepFilterNet3 speech enhancement (NOT STT)

**HfApi (live):** exists · gated=False · private=False · `pipeline_tag=audio-to-audio` ·
`library_name=hush` · 84 downloads / 24 likes · tags include `speech-enhancement, denoising,
background-speaker-suppression, noise-cancellation`. Files: `config.json`, `model_best.ckpt` (9 MB),
**`onnx/advanced_dfnet16k_model_best_onnx.tar.gz`** (8.4 MB), README, audio samples, LICENSE (Apache-2.0).

**Arch (decoded from `config.json` + the ONNX):** `architectures: ["DfNetSE"]`,
`model_type: hush`, `fft_size=320, hop_size=160, nb_erb=32, nb_df=64, df_order=5,
emb_hidden_dim=256, df_hidden_dim=256, conv_ch=16, df_num_layers=3`. `config.ini` inside the tarball:
`[train] model = deepfilternet3`. → This **is DeepFilterNet3** (a competing-speaker-suppression
fine-tune of it). It is a denoiser, **not a transcriber.**

**Acquired:** `~/.cache/waav-models/hush/` (extracted `enc.onnx`, `erb_dec.onnx`, `df_dec.onnx`,
`config.ini`, `version.txt`).

**Weights validated (live smoke):** `enc.onnx` loaded in onnxruntime and ran on a random
`feat_erb[1,1,10,32]` + `feat_spec[1,2,10,64]` frame → all 7 encoder outputs present with the
expected DFNet3 shapes (`e0 [1,16,10,32]`, `e1 [1,16,10,16]`, `e2/e3 [1,16,10,8]`, `emb [1,10,128]`,
`c0 [1,16,10,64]`, `lsnr [1,10,1]`) and all finite. **The model is genuine, not a placeholder.**

**Why it is NOT a drop-in (the precise gap):** WaaV's `enhance.rs` (`waav-infer-core`) supports
*single-graph* spectral denoisers via input-name dispatch:
- `Gtcrn` (`mix` + 3 recurrent caches → `enh`),
- `Raw` (`audio_input` → `audio_output`),
- `Dpdfnet` (`spec` + `state_in` → `spec_e` + `state_out`, with normalization seeded from
  `metadata_props`).

`load_enhancer()` (`waav-infer-server/src/engine.rs:480`) takes **one** graph. hush ships the **raw
DeepFilterNet3 3-graph export** (`enc` + `erb_dec` + `df_dec`) with the upstream feature interface:
`feat_erb[1,1,S,32]` (32-band ERB features), `feat_spec[1,2,S,64]` (complex spec for the lowest 64
bins), an ERB **gain mask** `m` from `erb_dec`, and **deep-filter coefficients** `coefs[1,S,64,10]`
(= 5 complex taps, `df_order=5`) from `df_dec`. There is **no fused single graph, no
`metadata_props` normalization seed**, and WaaV has **no ERB analysis, no deep-filter (DF) operator,
and no multi-graph orchestration** (confirmed by grep: only slaney-mel exists in `mel.rs`; no
`erb`/`deep_filter`/`df_dec` code anywhere outside `target/`).

**To onboard properly** would require a genuinely new execution path (the triage's "enhance-onnx
~half day"): ERB feature extraction (32 bands, `min_nb_erb_freqs=2`), complex-spec features,
3-graph orchestration (`enc → {erb_dec, df_dec}`), apply ERB gain, then the **deep-filter** operation
over the lowest 64 bins, then ISTFT (`fft=320/hop=160`, causal, ~20 ms algorithmic latency). That is
new arch + new DSP, not the config+weights reuse this campaign targets, and it is an **enhancement**
model regardless — so it does not advance the STT goal. **Disposition: SKIP-for-now**, documented and
cached; revisit under the P6-enhance-onnx track if a DFNet3 multi-graph path is built.

## YatharthS/NovaSR — tiny audio super-resolution (NOT STT)

**HfApi (live):** exists · gated=False · `pipeline_tag=audio-to-audio` · `library_name=None` ·
tags `[pytorch, audio-to-audio]` · 112 downloads / 86 likes. Files: `config.json` (a **dummy** —
`{"description": "This is a dummy config file to allow HuggingFace to track downloads."}`),
`pytorch_model.bin` / `_v1` / `_v2` (each **52 KB**), `NovaSR.mp4`, README. **No ONNX, no arch code,
no real config.**

**Arch:** README — "tiny 50 kB audio upsampling model that upscales muffled 16 kHz audio into clear
48 kHz audio" (16 kHz → 48 kHz super-resolution). It is **audio-to-audio upsampling, not ASR.** The
actual architecture/usage lives only on an **external GitHub repo** (`ysharma3501/NovaSR`); the HF
repo is weights-only with a placeholder config.

**Disposition: SKIP.** (1) Not ASR — wrong task family entirely. (2) No ONNX and no architecture in
the HF repo → onboarding would require reconstructing the net from an external GitHub repo + a
`torch.onnx.export`, which is out of scope for the "reuse existing infra (config+weights)" mandate
and yields an *enhancement* model, not STT. Honest skip; not hallucinated, not gated.

## YatharthS/LavaSR — Vocos bandwidth-extension (NOT STT)

**HfApi (live):** exists · gated=False · `pipeline_tag=audio-to-audio` · `library_name=None` ·
731 downloads / 81 likes. Files: dummy `config.json`, `denoiser/denoiser.bin` (784 KB),
`enhancer/{config.yaml, pytorch_model.bin (53 MB)}`, `enhancer_v2/{config.yaml, pytorch_model.bin
(55 MB)}`, `LavaSR (1).mp4`, README. **No ONNX.**

**Arch (decoded from `enhancer*/config.yaml`):** **Vocos** —
`feature_extractor: vocos.feature_extractors.MelSpectrogramFeatures` →
`backbone: vocos.models.VocosBackbone` (dim 512, intermediate 1536, 8 ConvNeXt layers) →
`head: vocos.heads.ISTFTHead` (n_fft 1024/2048). README: "novel 50 MB BWE (bandwidth extension)
model along with the UL-UNAS denoiser… Input: any 8–48 kHz → Output: 48 kHz." It is **bandwidth
extension / super-resolution + denoising — audio-to-audio, not ASR.**

**Disposition: SKIP.** (1) Not ASR. (2) Architecturally it is a **Vocos vocoder-style** net (mel →
ConvNeXt backbone → ISTFT head) — a *new* arch family WaaV's enhancement path does not implement
(WaaV's `enhance.rs` does spectral-mask/recurrent denoisers, not mel→ConvNeXt→ISTFT synthesis),
PyTorch-only, no ONNX. Onboarding would mean a new Vocos arch + export, out of scope and still an
enhancement model. Honest skip; real, ungated, Apache-2.0.

---

## What I touched

- **Rust / registry / model.rs:** **none.** No model was a config+weights or minimal-reuse drop-in,
  so no code changes were made → no `cargo test` / clippy run was warranted (nothing to compile).
  (No shared-file edits; nothing to flag for concurrent agents.)
- **Cache (acquired):** `~/.cache/waav-models/hush/` — extracted DFNet3 ONNX (`enc.onnx`,
  `erb_dec.onnx`, `df_dec.onnx`, `config.ini`, `version.txt`) for the smoke-validation above.
- **Report:** this file, `WaaV/inferv2/REVIEW/ONBOARD-small-stt-trio.md`.

## Bottom line

3/3 models **exist, are ungated, Apache-2.0, and are genuine** (hush's weights smoke-validated).
**0/3 are STT/ASR** — all are `audio-to-audio` *enhancement* (denoise / super-resolution /
bandwidth-extension). The "SR" in the names is audio **super-resolution**, not speech recognition.
None is a clean reuse of existing infra: hush needs a new DFNet3 multi-graph + ERB/deep-filter
pipeline; NovaSR and LavaSR are PyTorch-only new arches (undisclosed / Vocos) with no ONNX. No
transcribe/WER/RTF is possible because none transcribes. All three honestly **SKIPPED for the STT
campaign**, with hush noted as a future P6-enhance-onnx candidate if a DeepFilterNet3 execution path
is built. **No fabrication: every claim above is from live HfApi + the actual config/ONNX files.**
