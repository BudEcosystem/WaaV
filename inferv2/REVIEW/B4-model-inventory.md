# B4 — Definitive Model Inventory (WaaV Infer, both paths)

Date 2026-06-21 · branch `waav-infer-v2-build` · GB10. READ-ONLY audit (no build, no model load).
Sources: every `waav.json`/`config.json` on disk, the 16 registered Rust ONNX arms
(`crates/waav-infer-core/src/{stt,tts,s2s,diarize.rs,enhance.rs}` + `model.rs REGISTERED_ARCHITECTURES`),
the 14 torch-sidecar runners (`torch_runtime/models/` `@register`), the Phase-C live sweep
(`REVIEW/10-PHASE-CD-RESULTS.md`), `INFER_TRIAGE.md`, the enterprise verdict.

## The two execution paths (both are WIRED into the engine)

- **Path A = ONNX/ORT.** 16 registered architecture families in `model.rs` (dispatched by config arch;
  `StaticGraph`/ORT backend). Diarize + enhance load outside the single-model registry (`load_diarizer`/
  `load_enhancer`, config-driven). **12/14 ONNX arms ran clean live on GB10 CUDA** (Phase-C sweep).
- **Path B = torch sidecar.** `engine.rs::read_torch_runtime` → `load_torch_model` spawns
  `python -m torch_runtime serve` (UDS, crash-isolated) for any dir whose `waav.json` says
  `{"runtime":{"backend":"torch",…}}`. 14 runner classes registered in `torch_runtime/models/`.
  **torch IS installed (2.12.0+cu130, CUDA True; transformers 5.12.0)** and the sidecar is wired — but
  Path B has **ZERO live verification gate in the v2-build review**: the Phase-C sweep covered ONLY the 14
  ONNX arms, and the profiling plan explicitly says "Do NOT attempt CLI inference" on torch dirs via the
  ORT registry (`unsupported architecture`). So Path B = "plumbed + code-present, NOT live-proven here."
  (Memory's older "no-torch-dep / can't run in engine" finding predates this torch_runtime; the dep now exists.)

## REGISTERED_ARCHITECTURES (Path A, the 16 ONNX arms)
`Whisper… , Moonshine… , sense_voice_ctc, nemo-conformer-{tdt,rnnt,ctc,aed}, nemotron_speech, qwen3_asr,
funasr_nano, voxtral_realtime, cohere_asr, style_text_to_speech_2, supertonic, chatterbox, melo_vits`
(+ diarize `pyannote-community-1` and enhance `enhance` loaded via the composed-task path, outside the 16).

Runnable-state key: ✅ = ONNX+weights+arm, ran live (or same-arm sibling ran live) · ⚠fp16 = CUDA fp16/quant
fails on this ORT build (CPU-only) · ⚠PathB = torch sidecar, plumbed but not live-verified here ·
❌ = blocked / weights missing / superseded.

---

## PATH A — ONNX / ORT (runnable now)

| model (dir) | task | arch (arm) | backend/Path | precisions on disk | size | state | notes |
|---|---|---|---|---|---|---|---|
| whisper-tiny.en | stt | WhisperForConditionalGeneration | ONNX / A | fp32+int8+fp16+q4 variants | 147M | ✅ | Sweep 1203ms RTF 0.10; byte-identical vs onnxruntime (73/73), WER 9.67% |
| whisper-base / -base.en | stt | Whisper… | ONNX / A | multi | 281M–2.1G | ✅ | same arm |
| whisper-large-v3 / -turbo (onnx-community) | stt | Whisper… | ONNX / A | fp32/fp16/int8 | 2.9G / 5.2G | ✅ | same arm (large not swept but same path) |
| moonshine-base | stt | MoonshineForConditionalGeneration | ONNX / A | fp32/quant | 240M | ✅ | Sweep 708ms RTF 0.06 |
| sherpa sense-voice (zh-en-ja-ko-yue) | stt | sense_voice_ctc | ONNX / A | int8 | 229M | ✅ | Sweep ok all 5 langs; ΔWER ≤0.03 vs sherpa; handles fp16 |
| parakeet-tdt-0.6b-v2 | stt | nemo-conformer-tdt | ONNX / A | fp32 | 3.0G | ✅ | Sweep **545ms RTF 0.045** |
| parakeet-tdt-0.6b-v3 | stt | nemo-conformer-tdt | ONNX / A | fp32 | 3.0G | ✅ | same arm |
| parakeet-ctc-0.6b | stt | nemo-conformer-ctc | ONNX / A | fp32 | 3.0G | ✅ | Sweep **482ms RTF 0.040** |
| parakeet-rnnt-0.6b | stt | nemo-conformer-rnnt | ONNX / A | fp32 | 3.0G | ✅ | same arm |
| canary-180m-flash | stt | nemo-conformer-aed | ONNX / A | fp32 | 947M | ✅ | Sweep 555ms RTF 0.046 |
| canary-1b-v2 | stt | nemo-conformer-aed | ONNX / A | fp32 | 4.7G | ✅ | same arm |
| nemotron-en | stt | nemo-conformer-rnnt | ONNX / A | nemo128 fp32 | 2.4G | ✅ | Sweep 594ms RTF 0.049 |
| fastconformer-quran-ar | stt | nemo-conformer-rnnt | ONNX / A | nemo80 fp32 | 456M | ✅ | same arm (Arabic) |
| nemotron-3.5-asr-streaming-0.6b | stt | nemotron_speech | ONNX / A | int4 | 2.4G | ✅ | dedicated streaming arm |
| funasr-nano | stt | funasr_nano | ONNX / A | int8 (enc/embed/llm) | 973M | ✅ | Sweep 3268ms RTF 0.27 (LLM-decoder); F5-1 KV fix |
| qwen3-asr-0.6b-onnx | stt | qwen3_asr | ONNX / A | fp16 embed | 8.0G | ✅* | arm ran; *not in the 14-sweep table, same path; F5-2 dtype fix |
| qwen3-asr-1.7b-onnx | stt | qwen3_asr | ONNX / A | fp16 embed | 9.4G | ✅* | same arm |
| **voxtral-realtime** | stt | voxtral_realtime | ONNX / A | **q4f16** | 11G | ⚠fp16 | **CUDA FAIL**: ORT GQA `attention_bias` unsupported + cuDNN no-plan. int8-on-CPU byte-identical (banked). CPU-only here |
| **cohere-transcribe-03-2026** | stt | cohere_asr | ONNX / A | **fp16** | 3.9G | ⚠fp16 | **CUDA FAIL**: cuDNN "no execution plans". CPU-only here |
| Kokoro-82M-v1.0-ONNX | tts | style_text_to_speech_2 (kokoro) | ONNX / A | fp32/fp16/q8/q4 | 313M | ✅ | first-audio 1015ms RTF 0.146, **flat to N=16** (CPU-pinned) |
| supertonic-3 | tts | supertonic | ONNX / A | fp32 (+fp16 partial) | 383M | ✅ | flow-matching; maxΔ=0.0000 bit-identical. Full fp16 blocked by f32-pinned CFM inputs |
| MeloTTS-en (vits-melo) | tts | melo_vits | ONNX / A | fp32 | 168M | ✅ | Sweep 257KB wav ok |
| chatterbox-onnx | tts | chatterbox (codec-AR) | ONNX / A | fp32 (4.7G) | 4.7G | ✅ | Sweep 180KB wav; ragged-batched == per-slot **bit-identical** (3 heavy gates green) |
| chatterbox-turbo-onnx | tts | chatterbox | ONNX / A | fp32 | 6.9G | ✅ | same arm; bit-identical gate green |
| pyannote-community-1 | diarize | pyannote-community-1 | ONNX / A | fp32 (seg+embed) | 32M | ✅ | composed task (load_diarizer); F5-5 bounds fix; verified 2-spk |
| dpdfnet2 / dpdfnet4 / dpdfnet8 | enhance | enhance (DPDFNet) | ONNX / A | fp32 | 9.8M–14M | ✅ | composed task (load_enhancer); dpdfnet4/8 have no waav.json (default fp32 path) |
| clear (clear-studio/natural) | enhance | enhance | ONNX / A | fp32 | 33M | ✅ | self-shipped ONNX (DFN3-style) |
| tse-tasnet (tse_prod_48k) | enhance | enhance | ONNX / A | fp32 + .data | 12M | ✅ | self-shipped ONNX |

**Path A live sweep (Phase-C, 14 arms, 12.05s clip):** 12/14 ✅ RTF 0.04–0.27 on CUDA; **2 ❌ on CUDA**
= voxtral q4f16 + cohere fp16 (ORT/cuDNN EP op-support limit, not WaaV logic; CPU-only here).

### Path-A ONNX codec / shared (decode dependencies, NOT standalone model arms)
| dir | role | size | note |
|---|---|---|---|
| kyutai-mimi-onnx | Mimi codec ONNX | 2.2G | codec decoder for codec-AR (csm/moshi family); not a registered model arm |
| kyutai-mimi (safetensors) | Mimi codec (torch) | 367M | torch codec weights |
| neucodec-onnx | NeuCodec decoder ONNX | 747M | decoder for neutts-air |
| (assets: tiny_*.onnx, kokoro_m1_sample.wav) | test fixtures | — | not models |

---

## PATH B — torch sidecar (plumbed, NOT live-verified in this review)

All have `waav.json {"runtime":{"backend":"torch",…}}` and a registered `torch_runtime/models/*.py` runner.
The sidecar is wired (`engine.rs`) and torch 2.12+cu130 is present, but **none was run live in the v2-build
sweep** → state = ⚠PathB (needs the custom runtime exercised + an accuracy/perf gate to go live).

| model (dir) | task | arch (runner) | backend/Path | dtype | size | state | notes |
|---|---|---|---|---|---|---|---|
| ark-asr-0.6b | stt | ArkasrForConditionalGeneration (arkasr) | torch / B | fp16 | 8K* | ⚠PathB | *manifest stub; weights at HF cache models--AutoArk-AI--ARK-ASR-0.6B (2.5G) |
| granite-speech-4.1-2b | stt | GraniteSpeechForConditionalGeneration | torch / B | bf16 | 8K* | ⚠PathB | *stub; weights at HF cache (4.6G); shared-runtime class |
| csm-1b-hf | tts | csm (CsmForConditionalGeneration) | torch / B | bf16 | 6.7G | ⚠PathB | Llama-AR + Mimi codec; transformers-native |
| cosyvoice3 | tts | cosyvoice3 (CosyVoice3) | torch / B | bf16 | 9.1G | ⚠PathB | vendored arch + CFM/HIFT |
| dia-1.6b | tts | dia (DiaForConditionalGeneration) | torch / B | bf16 | 6.1G | ⚠PathB | |
| dia2-2b | tts | dia2 (Dia2Model) | torch / B | bf16 | 7.2G | ⚠PathB | vendored dia2 engine |
| dots-tts-base | tts | dots_tts (DotsTTSForConditionalGeneration) | torch / B | bf16 | 4.9G | ⚠PathB | continuous-latent AR + DiT CFM |
| dots-tts-mf | tts | dots_tts | torch / B | bf16 | 4.9G | ⚠PathB | same runner |
| dots-tts-soar | tts | dots_tts | torch / B | bf16 | 4.9G | ⚠PathB | same runner |
| higgs-tts | tts | higgs_tts (HiggsAudioV3) | torch / B | fp16 | 8K* | ⚠PathB | *stub→ HF onnx-community/higgs-audio-v3-tts-4b cuda_fp16 (7.7G) |
| neutts-air | tts | neutts_air (NeuTTSAir) | torch / B | bf16 | 1.5G | ⚠PathB | Qwen2-0.5B AR + NeuCodec ONNX decoder |
| omnivoice | tts | omnivoice (OmniVoice) | torch / B | fp32 | 3.1G | ⚠PathB | Qwen3-0.6B AR; "looked-verified-but-not-robust" caution in memory |
| qwen3-tts-12hz-06b | tts | qwen3_tts (Qwen3TTSForConditionalGeneration) | torch / B | bf16 | 2.4G | ⚠PathB | talker LM + MTP + codec |
| vibevoice-1.5b | tts | vibevoice (VibeVoiceForConditionalGeneration) | torch / B | bf16 | 5.1G | ⚠PathB | Qwen2.5-1.5B + diffusion head |

**14 torch runners registered**, **14 model dirs map to them** (dots ×3 share one runner; arkasr/granite/
higgs stubs point at HF-cache weights). Tasks: **3 STT** (arkasr, granite, canary_qwen) + **11 TTS**.
Note `canary_qwen` runner exists but its model dir is BLOCKED (below) → 13 of 14 runners have live-able weights.

---

## BLOCKED / SUPERSEDED / NOT-A-MODEL

| dir | status | reason |
|---|---|---|
| canary-qwen-2.5b | ❌ blocked | `status: blocked-pending-portable-reimplementation` — NeMo SALM vendor-locked (directive #4). torch runner `canary_qwen` exists but weights path not portably served |
| chatterbox (venv) | ❌ superseded | `status: SUPERSEDED-by-portable-ONNX` — replaced by chatterbox-onnx (Path A). venv retired |
| csm-1b (non-hf) | ❌ not-a-model | only `prompts/` + README; the real model is csm-1b-hf (Path B) |

---

## TOTALS & THE GAP TO "ALL 40+ LIVE"

**Distinct integrated models on disk (de-duplicated, voice/speech only):** **~46**
(33 dirs under `~/.cache/waav-models` + 13 ONNX-only model dirs under the HF cache that map to Path-A arms:
whisper ×4, moonshine, sensevoice, parakeet ×4, canary ×2, supertonic, kokoro-onnx — Note: many HF-cache
entries are non-voice LLM/embedding/OCR/vision and are EXCLUDED). The "40+ from the previous version" claim
is corroborated.

### Counts by runnability

| bucket | count | live-state |
|---|---|---|
| **Path A — ✅ runnable now (CUDA)** | **24** | 22 distinct STT/TTS arms + diarize + enhance trio counted as families; ran live or same-arm sibling ran live |
| **Path A — ⚠fp16/quant CPU-only (CUDA-blocked)** | **2** | voxtral-realtime (q4f16), cohere-transcribe (fp16) — ORT/cuDNN EP limit |
| **Path B — ⚠needs custom runtime exercised (plumbed, torch present, NOT live-verified)** | **14** | the torch-sidecar models below |
| **❌ blocked / superseded / not-a-model** | **3** | canary-qwen, chatterbox-venv, csm-1b-stub |

**Headline split (the answer to "all 40+ live"):**
- **Path A runnable now = 26 model dirs across ~14 arm families** (24 CUDA-ready + 2 CPU-only-fp16). The
  serving core + 12 CUDA arms are the enterprise-verdict "deploy NOW" set.
- **Path B blocked-on-runtime = 14** torch-sidecar models (13 with live-able weights, 1 blocked). These are
  CODE-COMPLETE and PLUMBED but have **no live smoke/accuracy/perf gate** in the v2-build review → they are
  the gap. To make them live: spawn the sidecar against each (`python -m torch_runtime serve`), confirm a
  real forward + an accuracy check, add per-arm gates.
- **Weights/onboarding gap = small:** only **3** are truly blocked (canary-qwen reimplementation, the
  superseded venv chatterbox, the csm stub). Everything else has weights on disk.

### The Path-B list that needs the custom (torch) runtime to go live
**STT (3):** ark-asr-0.6b, granite-speech-4.1-2b, *(canary-qwen-2.5b — runner exists but model blocked)*.
**TTS (11):** csm-1b-hf, cosyvoice3, dia-1.6b, dia2-2b, dots-tts-base, dots-tts-mf, dots-tts-soar,
higgs-tts, neutts-air, omnivoice, qwen3-tts-12hz-06b, vibevoice-1.5b.

→ **14 torch runners / 14 model dirs** ride Path B. The custom PyTorch sidecar runtime exists and torch is
installed; the work to take them "live" is **verification (live forward + accuracy + perf gate), not new
integration code** — except canary-qwen, which needs portable SALM reimplementation.

### Honest caveats
- "Runnable now" for Path A = arm + weights present and either swept live or a same-arm sibling swept live;
  18/20 arms still lack a *deep* p50/p95 perf gate (sweep gave load+infer latency only).
- Full **fp16 end-to-end** is incomplete tree-wide: F4 fixed the F16 OUTPUT read (35 sites) but
  canary/supertonic/chatterbox-AR/parakeet/nemo/qwen3/funasr still feed step INPUTS as f32 (needs the
  `StaticGraph::input_types()` cast). fp32 paths are bit-faithful.
- Path B has **zero live coverage** in this review even though torch is present and the sidecar is wired —
  this is the single biggest "advertised ≠ proven" gap for the "40+ all work" claim.
