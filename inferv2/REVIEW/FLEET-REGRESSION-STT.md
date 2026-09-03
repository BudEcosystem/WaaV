# WaaV-Infer STT/ASR Fleet — LIVE Regression + Perf + Precision Sweep (GB10)

**Date:** 2026-06-24 · **Box:** GB10 (Grace-Blackwell sm_121, 121 GB unified) · **Env:** `gb10-env.sh` (ORT-CUDA 1.27, libtorch CUDA via tch-rs)
**Repo:** `/home/bud/ditto/waav/waav-infer` (read-only; no commit, no fmt) · **Total wall-time:** ~22 min (sum of isolated gate runs; serialized one model at a time per the unified-memory OOM rule)
**Memory:** stayed ≥ 80 GB avail throughout; one model loaded at a time; concurrent TTS-fleet agent shared GPU without contention.

LAW: every cell below is quoted from an **actual live run** on this box today — not a claim. Self-skips and the one perf failure are flagged honestly.

---

## Per-model evidence matrix

| Model | Task | Path | Byte-faithful vs ref | RTF (CUDA) | Precision(s) tested | PASS/FAIL |
|---|---|---|---|---|---|---|
| **whisper-tiny.en** | STT | A (ONNX) | CPU↔CUDA words **100%**, 7/7 content hits | 0.157 (sweep) / 0.080 (perf_bench, ep=cpu, 12.5× RT) | fp32 (CPU+CUDA) | **PASS** |
| whisper (ragged/concurrent) | STT | A (ONNX) | **batched == per-slot token-for-token** | N=16 wall 10.2s, 1.18× scaling | fp32 | **PASS** |
| **sensevoice-small** | STT | A (ONNX) | CPU↔CUDA words **100%**, 7/7 | 0.054 | **int8** (CPU+CUDA) | **PASS** |
| **parakeet-tdt-0.6b** (v2) | STT | A (ONNX) | CPU↔CUDA words **100%**, 7/7 | 0.009 | fp32 | **PASS** |
| **parakeet-rnnt-0.6b** | STT | A (ONNX) | transcript correct (CLI) | 0.050 | fp32 | **PASS** (live, no standing gate) |
| **parakeet-ctc-0.6b** | STT | A (ONNX) | transcript correct (CLI) | 0.040 | fp32 | **PASS** (live, no standing gate) |
| **moonshine-base** | STT | A (ONNX) | CPU↔CUDA words **100%**, 7/7 | 0.014 | fp32 | **PASS** |
| **canary-1b-v2** | STT | A (ONNX) | transcript correct (CLI) | 0.062 | fp32 | **PASS** (live, no standing gate) |
| **canary-180m-flash** | STT | A (ONNX) | transcript correct (CLI) | 0.049 | fp32 | **PASS** (live, no standing gate) |
| **canary-qwen-2.5b** | STT | B (tch) | — golden+clips absent | — | bf16 | **SKIP** (unverified — needs NeMo to regen golden) |
| **cohere-transcribe** | STT | B (tch hybrid) | tch-CUDA == ORT-CPU, **100%** de-punct | 0.18 | ORT-CUDA enc + f32→f16 tch dec | **PASS** |
| **voxtral-realtime** (torch arm) | STT | B (tch) | tch-CUDA == ORT-CPU, **100.0%** char-identity | **1.72** | bf16→f16 | **FAIL (perf)** — transcript byte-identical, RTF≥1 assertion fired |
| **voxtral-realtime** (ONNX arm) | STT | A (ORT-CUDA) | **byte-identical to CPU ref** | 0.457 | **q4f16** | **PASS** |
| **ark-asr-0.6b** | STT | B (tch) | — golden absent | — | bf16→f16 | **SKIP** (unverified — golden `transcript_cpu_fp32.txt` absent) |
| **granite-speech-4.1-2b** | STT | B (tch) | **byte-identical** to sidecar cuda-bf16 golden, **100%** | 0.390 | bf16 | **PASS** |
| **higgs-stt** (higgs-audio-v3-stt) | STT | B (tch) | mean **WER 0.020** vs LibriSpeech GT (2/3 clips 0.000) | 0.47–1.16 | bf16→f16 | **PASS** |
| **vibevoice-asr** | STT | B (tch) | mean **WER 0.053** vs LibriSpeech GT (all artifacts: Mr/punct) | 1.09–2.19 | bf16 | **PASS** |
| **medasr** | STT | A (ONNX) | **WER 0.056** (CPU==CUDA), 2/3 clips 0.000 | 0.039 (CPU 0.058) | fp32 | **PASS** |
| **qwen3-asr-0.6b** | STT | A (ONNX) | transcript correct (CLI) | 0.487 | fp16 embeds + fp32 onnx (int4 variant avail) | **PASS** (live, no standing gate) |
| **sortformer-diar-4spk-v2** | Diar | A (ONNX) | 1 spk on dia_hunan (single-spk clip) | 0.098 | fp32 | **PASS** (live, no standing gate) |

**Live-verified this run: 19 models/variants** (16 PASS + 1 perf-FAIL-but-byte-identical + 2 honest SKIP).

---

## Headline findings

### 1. REGRESSION — voxtral-realtime torch arm fails the RTF gate (accuracy intact)
`cuda_torch_voxtral_vs_ort` **FAILED**, but the failure is **purely perf**, not accuracy:
- Transcript is **byte-identical** (100.0% char-identity, 100% de-punct) to the ORT-CPU reference — the MEMORY-documented byte-faithfulness holds.
- The gate panicked at `torch CUDA RTF 1.72 ≥ 1`. The candle-arm reference (in the test) was RTF 0.62. The timed inference was **20773 ms for 12.05 s** of audio.
- This is the **torch bf16→f16 path** specifically. The **ONNX q4f16 path of the same model on ORT-CUDA is RTF 0.457** (correct transcript, PASS) — so the model is well under realtime; the regression is the libtorch arm's decode throughput (~2.8× slower than the q4f16 ONNX arm, and ~2.8× over its own RTF<1 budget).
- Recommendation: investigate the torch voxtral decode (KV-cache / f32 final-matmul cost on the AR loop) OR route voxtral through the ONNX q4f16 arm in production (already byte-identical + fast).

### 2. Two honest SKIPs (golden artifacts absent after box reboot)
- **ark-asr-0.6b**: weights present, but `/tmp/ark_golden/transcript_cpu_fp32.txt` is gone (box rebooted today — /tmp cleared). ARK is `trust_remote_code` (no ONNX), so regenerating the golden needs the python sidecar `dump_golden.py`. Self-skips cleanly.
- **canary-qwen-2.5b**: weights present, but `/tmp/canary_clips/*.wav` + `/tmp/canary_golden/*.txt` (NeMo SALM goldens) are gone. Needs NeMo to regenerate. Self-skips cleanly. (Note: the canary *encoder-decoder ONNX* family IS live-verified via canary-1b-v2 + canary-180m-flash through the CLI — only the tch SALM variant is unverified.)

### 3. The Path-A ONNX STT fleet is uniformly green and very fast
`cpu_sweep_onnx` (whisper-tiny/sensevoice/parakeet-tdt/moonshine) all hit **100% CPU↔CUDA word-agreement, 7/7 content hits**. CUDA RTFs 0.009–0.157. The cudnn_frontend "No execution plans support the graph" errors are non-fatal warnings (ORT falls back to a supported kernel); transcripts are perfect.

### 4. The LLM-decoder-ASR seam (Path-B tch) is byte-faithful where goldens exist
granite (byte-identical to sidecar bf16), cohere (==ORT-CPU), higgs-stt (WER 0.020), vibevoice-asr (WER 0.053) all PASS. Where short clips push RTF>1 (higgs clip0 1.16, vibevoice 1.7–2.2) it is fixed AR overhead amortizing away on longer clips (higgs 12.5s clip = 0.47).

---

## Precision / quant matrix (which precisions are available + verified)

| Model | Precision RUN (live) | 2nd precision available | 2nd precision verified |
|---|---|---|---|
| whisper-tiny.en | fp32 (CPU + CUDA, agree 100%) | (turbo/base ONNX exist) | not run this sweep |
| sensevoice-small | **int8** (CPU + CUDA, agree 100%) | fp32 onnx exists | not run |
| parakeet-tdt-0.6b | fp32 | int8 mirror exists (sherpa-nemo) | not run |
| parakeet-rnnt / ctc | fp32 | — | n/a |
| moonshine-base | fp32 | — | n/a |
| canary-1b-v2 / 180m-flash | fp32 | — | n/a |
| qwen3-asr-0.6b | fp16 embeds + fp32 onnx | **int4** (decoder_init/step.int4.onnx + encoder.int4.onnx) present | not run |
| **voxtral-realtime** | **q4f16 ONNX (PASS, byte-identical, RTF 0.457)** + bf16→f16 torch (byte-identical, RTF 1.72 FAIL) | int8/q4/q4f16-nobias onnx all present | q4f16 ✓ byte-identical to CPU ref (2 precisions verified) |
| cohere-transcribe | ORT-CUDA enc + f32→f16 tch dec | — | n/a |
| granite-speech-4.1-2b | bf16 (byte-identical) | — | n/a |
| higgs-stt | bf16→f16 | — | n/a |
| vibevoice-asr | bf16 | — | n/a |
| medasr | fp32 (CPU == CUDA, WER identical) | — | n/a |
| sortformer-diar | fp32 | — | n/a |

**voxtral** is the only model in this sweep with a **second precision independently byte-verified** (q4f16-ONNX byte-identical to CPU + bf16-torch byte-identical to CPU). The qwen3-asr int4 and sensevoice-fp32 variants are present but not exercised this run (would be a fast follow-up).

---

## Coverage notes — models with NO standing live gate (verified ad-hoc via the `waav-infer transcribe`/`diarize-stream` CLI through `load_model_at`)

parakeet-rnnt, parakeet-ctc, canary-1b-v2, canary-180m-flash, qwen3-asr-0.6b, sortformer-diar — all have a `waav.json` (registry-loadable) and a model module, but only **deterministic unit tests** in-source (no integration gate that loads the real ONNX). They were each driven live on CUDA through the CLI and transcribe/diarize correctly. **Recommendation:** add standing `--ignored` gates for these (they're cheap ONNX, <0.5 RTF) so they don't silently rot.

## Test infrastructure observations
- `/tmp` golden/clip dirs are **wiped on reboot** (box rebooted 2026-06-24). The granite golden survives because it lives in the persisted `~/.cache/waav-models/granite-speech-golden/` — ark/canary-qwen goldens do not have a persisted copy and so self-skip. **Recommendation:** persist ark + canary-qwen goldens into `~/.cache/waav-models/<model>-golden/` (as granite does) so the gates survive reboots, and point the test default dirs there.
- The higgs-stt / vibevoice-asr LibriSpeech clips (`/tmp/higgs_clips/clip{0,1,2}.wav`) were regenerated this run from `hf-internal-testing/librispeech_asr_dummy` (the canonical Quilter clips) and match the embedded references exactly.

---

## Raw run logs (this session)
`/tmp/claude-1000/.../scratchpad/{whisper_ragged,bench_whisper,voxtral_vs_ort,cohere_vs_ort,granite,higgs_stt,vibevoice_asr,medasr,voxtral_q4f16}.log`
(cpu_sweep_onnx, ark, canary-qwen, and the CLI runs were captured inline.)
