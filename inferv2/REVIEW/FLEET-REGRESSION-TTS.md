# WaaV-Infer — LIVE Fleet Regression: TTS / S2S / codec / enhance (Path-B tch generative)

**Host:** GB10 (Grace-Blackwell sm_121, 121 GB unified mem). **Date:** 2026-06-24.
**Method:** each gate run process-isolated via `cargo test --include-ignored --test-threads=1 --nocapture`
(serialized to avoid the documented global-CUDA-RNG parallelism leak; `free -g` before each load; OOM guard wait if avail<25G).
A concurrent STT-fleet agent shared the GPU during this run.
**Repo:** `/home/bud/ditto/waav/waav-infer` (read-only; no commit/fmt). **Env:** `source gb10-env.sh`.

Every row is a REAL quoted live result (max|Δ| / N-of-N / RTF), not a claim. Self-skips and FAILs flagged honestly.

---

## HEADLINE SUMMARY

**Total wall-time:** ~56 min (07:08:43 → 08:04:36, 2026-06-24), 8 process-isolated batches, serialized
`--test-threads=1`, `free -g` before each load. No OOM (avail stayed 80-114 G; concurrent STT-fleet agent shared
GPU early on without contention issues).

**Live-verified PASS: 21 generative models + the 20-gate codec layer + 4 CUDA-graph perf-lever pairs.**
- **Byte-identical / byte-faithful LAW held** (quoted): dacvae (8.8e-6/7.5e-7), dia2 (CPU 544/544 + CUDA logit Δ=0),
  dia (CPU 23409/23409 + CUDA all-2601-frames ch0), csm (greedy 125×32 byte-identical), s2pro (frame-0 cb0..8 + codec
  0.9973), indextts2 (200/200), dots base/soar/mf (10240/9728/10240 latents byte-identical, corr 1.0), neutts
  (CPU+CUDA 96/96 + seams Δ=0), qwen3-tts (talker hidden Δ==0 + codec 0.9999), higgs-v3 (all seams Δ=0 + greedy 0/264),
  higgs-v2 (seams Δ=0 + 21-frame prefix), irodori (latent 1.96e-4), cosyvoice3 (123 AR tokens byte-identical),
  hibiki S2S (encode 128/128 + duplex 96/96), pocket-tts (corr 1.0/0.999999), RSB enhance (2.68e-7), lfm2 S2S
  (ASR transcript-exact + round-trip).
- **CUDA-graph perf lever** (capture==eager BYTE-IDENTICAL, accuracy-preserved): dia2 ×1.182 (+15.4%),
  csm ×1.266 (+21.0%), omnivoice ×1.065 (+6.1%), dots ×1.005 (marginal/edge-only).
- **Best RTFs (CUDA):** irodori 0.162, neutts 0.768, cosyvoice3 0.49, vibevoice 0.632, omnivoice 0.321, csm 1.062.

**REAL REGRESSIONS (2):**
- **zonos2** — greedy codes byte-identical WHERE they overlap (126/126) but tch EOS@frame14 vs golden 32 frames
  (frame-count/EOS divergence). f32 RTF also pathological (109.8).
- **misotts** — only 61/1024 f32-LAW codes match (first-div cb0 frame3); frame count OK. Golden-staleness suspected
  but unresolved — needs investigation.

**ENV-BLOCKED, not regressions:**
- **6 TRT perf-lever gates** (neutts fp16/int8/fp8/nvfp4, dia, higgs) — engines staged but the Torch-TensorRT
  RUNTIME (`torch_tensorrt`/`libtorchtrt_runtime.so`) is absent on this box → `trt_active=false`, engine won't
  deserialize. Base models load fine on CUDA. (See "TensorRT perf lever" section.)
- **2 SKIP (golden absent):** voxtral-tts (`/tmp/voxtral_golden` empty), viitorvoice-nar (goldens missing).
  omnivoice's vs-sidecar golden also absent BUT it still PASSED its graph capture==eager byte-identity + live synth.

The known **--test-threads=1 serialization** (global-CUDA-RNG parallelism leak) was honored throughout; the
**mem::forget teardown** (process-isolation requirement) was honored via one `cargo test` invocation per gate.

---

## Per-model evidence matrix

(populated incrementally — see "Live run log" below for the raw quoted numbers backing each row)

| Model | Task | Byte-faithful vs golden | RTF (CUDA) | Precision + perf-lever | PASS/FAIL |
|---|---|---|---|---|---|
| codec (Mimi/DAC/DACVAE/RVQ/flow_dac/conv) | codec | 20/20 deterministic gates (incl. live Mimi dia2+csm bit-faithful) | n/a | f32 (in-src) | **PASS** |
| dacvae (Semantic-DACVAE) | codec roundtrip | encode 8.821e-6 / decode 7.451e-7 max\|Δ\| | n/a | CPU f32 | **PASS** |
| dia2-2B | AR codec-TTS (Mimi) | CPU fp32 544/544 exact; CUDA logit Δ=0, codec 4.46e-4, env corr 1.000 | 3.63 | CPU fp32 LAW + CUDA bf16 | **PASS** |
| dia-1.6B (Nari, DAC) | AR codec-TTS | CPU fp32 23409/23409 exact; CUDA bf16 ch-0 all 2601 frames | 2.85 | CPU fp32 LAW + CUDA bf16; DAC corr 0.986 | **PASS** |
| CSM-1B (Sesame) | dual-AR + Mimi | CUDA bf16 greedy byte-identical 125×32 | 1.062 | CUDA bf16 | **PASS** |
| s2-pro | AR codec-TTS | frame-0 cb0..8 byte-identical; codec corr 0.9973 | 3.566 | CUDA bf16 (bf16-tie floor) | **PASS** |
| indextts2 | GPT mel-code AR | 200/200 greedy mel codes byte-identical | (cpu) | CPU f32 | **PASS** |
| zonos2 | AR codec-TTS | 126/126 match WHERE overlap but EOS@14 vs golden 32 (frame-count divergence) | 109.8 (f32) | f32 LAW | **FAIL** |
| misotts | dual-AR codec-TTS | 61/1024 codes match (first-div cb0 f3); golden-mismatch suspected | — | f32 LAW | **FAIL** |
| dots.tts-base | AR + flow-matching | 10240 latents byte-identical (seed 0), corr 1.0000 | 2.278 | CUDA bf16 | **PASS** |
| dots.tts-soar | AR + flow-matching | 9728 latents byte-identical, corr 1.0000 | 2.286 | CUDA bf16 | **PASS** |
| dots.tts-mf | AR + meanflow | 10240 latents byte-identical, corr 1.0000 | 1.994 | CUDA bf16 | **PASS** |
| neutts_air | AR codec-TTS | CPU f32 96/96 + hidden/logits Δ=0; CUDA bf16 96/96 | 0.768 | CPU f32 LAW + CUDA bf16 | **PASS** |
| qwen3-tts-12hz | dual-AR + 12Hz codec | talker hidden Δ==0 (prefill+decode); greedy 44 frames then SDPA-tie; codec corr 0.9999 | (14s gate) | CUDA bf16 | **PASS** |
| higgs-v3-tts-4b | AR codec-TTS (DAC) | CPU f32 all seams Δ=0 + greedy 0/264 byte-identical | 1.394 | CPU f32 LAW + CUDA f16 | **PASS** |
| higgs-v2-3b | AR codec-TTS (DualFFN/MoE) | CPU f32 seams Δ=0 + greedy byte-identical 21-frame prefix (tie fork) | (88s gate) | CPU f32 | **PASS** |
| irodori-TTS-500M | RF DiT over DACVAE | latent max\|Δ\| 1.96e-4 (CPU), 3.81e-4 (CUDA); wav 1.63e-4 | 0.162 | CPU f32 LAW + CUDA | **PASS** |
| cosyvoice3 | Qwen2 LM + CFM + HiFT | AR speech-token seq byte-identical (123 tokens, first-div None) | 0.49 | CUDA bf16 | **PASS** |
| omnivoice | masked-diffusion-LM | vs-sidecar golden absent → that gate SKIP; BUT graph capture==eager byte-identical (0/288) + live synth | 0.321 | CUDA f32 (graph) | **PASS (graph/synth)** |
| vibevoice-1.5b | AR + diffusion | byte-identity golden absent; e2e structural OK | 0.632 | CUDA bf16 (e2e only) | **PASS (e2e)** |
| voxtral-tts | AR codec-TTS | golden absent → SKIP (not run) | — | (n/a) | **SKIP** |
| dia2 CUDA-graph | perf lever | capture==eager byte-identical (1188 calls) | — | ×1.182 (+15.4%) accuracy-preserved | **PASS** |
| csm CUDA-graph | perf lever | capture==eager byte-identical (125×32) | — | ×1.266 (+21.0%) accuracy-preserved | **PASS** |
| dots CUDA-graph | perf lever | capture==eager byte-identical (0/10240, Δ=0) | — | ×1.005 (+0.5%, marginal) | **PASS** |
| omnivoice CUDA-graph | perf lever | capture==eager byte-identical (0/288) | 0.321 | ×1.065 (+6.1%) | **PASS** |
| hibiki-zero-3b | S2S duplex (Mimi) | Mimi encode 128/128 + duplex greedy 96/96 byte-identical | 22.8 (CPU EP) | CPU f32 | **PASS** |
| lfm2.5-audio-1.5b | S2S (ORT) | ASR transcript-exact; S2S round-trip OK | 0.280 (ASR, CPU) | ORT CPU | **PASS** |
| pocket-tts | on-device AR (Mimi) | CPU corr 1.000000 / CUDA corr 0.999999 byte-faithful | (<1s gate) | CPU f32 + CUDA | **PASS** |
| RSB (Resemble-Enhance) | enhance (SDE) | max\|Δ\| 2.68e-7 vs golden | (1.1s gate) | CUDA f32 | **PASS** |
| viitorvoice-nar | NAR codec-TTS | goldens absent → SKIP (not verified) | — | (n/a) | **SKIP** |
| neutts/dia/higgs TRT (6 gates) | TRT perf lever | engines staged but torch_tensorrt RUNTIME absent → `trt_active=false`, load fails | — | fp16/fp8/int8/nvfp4 | **FAIL (env: TRT runtime missing — NOT a regression)** |

---

## Live run log (raw quoted numbers)

### codec layer — Mimi / DAC / DACVAE / RVQ / flow_dac / conv (deterministic in-src)
`cargo test -p waav-infer-backend-torch --lib codec::` → **20/20 PASS** (0.19s). Includes the two live
bit-faithful Mimi-codec gates: `mimi_decode_bit_faithful_dia2_regime` and `mimi_decode_bit_faithful_csm_regime`.

### dacvae — Semantic-DACVAE codec roundtrip (CPU f32) — PASS
```
encode latent max|Δ| vs golden = 8.821e-6
decode recon  max|Δ| vs golden = 7.451e-7
round-trip stable: |y2|max=0.5963 (tanh-bounded)
✓ Semantic-DACVAE encode + decode BYTE-FAITHFUL
```

### zonos2 — AR codec-TTS
- `zonos2_greedy_codes_byte_identical` → **FAIL**: `tch frames 14, golden frames 32` — codes 126/126
  match WHERE THEY OVERLAP (first-div None over 14 frames) but tch emits EOS at frame 14 vs golden 32.
  Real EOS/frame-count divergence. (precision: f32 LAW, WAAV_ZONOS2_FP32=1 auto-set)
- `zonos2_rtf` → PASS (finite): **RTF 109.839** (122.42s wall / 1.11s audio, 49152 samples) — f32, extremely slow.

### misotts — dual-AR codec-TTS (f32 LAW)
- `misotts_greedy_codes_byte_identical` → **FAIL**: frames 32/32 (count OK) but only **61/1024 codes match**,
  first-div (cb0, frame3). Golden read codebook-major. Frame-0 nearly matches (diverges at last codebook).
  Likely a stale/mismatched golden artifact OR a real port regression — needs investigation. (precision: f32)

### s2-pro — AR codec-TTS (CUDA bf16) — PASS
```
[L1] prompt ids match (26 tokens)
[L2] step0 semantic argmax=152215 matches golden sem_tokens[0]
[L3] greedy frames: tch T=2048, golden T=200
[L3] frame-0 tch=[537,164,181,623,866,866,814,814,724,537]
[L3] frame-0 gld=[537,164,181,623,866,866,814,814,362,362]
[L3] LAW PASSED: frame-0 codebooks 0..8 BYTE-IDENTICAL; cb8/cb9 + later trajectory = documented bf16-tie AR-compounding floor
[L4] codec decode on golden codes: 409600 samples, waveform corr=0.9973
[RTF] s2-pro CUDA-bf16 greedy: 339.17s wall for 95.11s audio → RTF 3.566
```
(precision: CUDA bf16; byte-identity LAW = frame-0 cb0..8 exact, rest is documented bf16 AR-compounding floor)

### indextts2 — GPT mel-code AR (CPU f32) — PASS
```
OK: 200 greedy mel codes byte-identical to the IndexTTS-2 reference golden
test result: ok (95.23s)
```
(precision: CPU f32; 200/200 greedy mel codes byte-identical)

### dia2-2B — AR codec-TTS (Mimi codec) — PASS
- `cpu_fp32_codes_byte_identical` → **PASS**: `CODE byte-identity: 544/544 match; first-div=None` (CPU fp32, seed 0), 80s.
- `cuda_torch_dia2` → **PASS** (CUDA bf16, 16s):
```
CODEC parity: max|Δ|=4.46e-4 | err/sig RMS=3.75e-5/7.20e-2 (0.0521%)
max|Δ logit value| = 0.0000        (step-0 AR math byte-identical)
SYNTH "[S1] Hello world.": 36480 samples 1.52s | infer 5511 ms | RTF 3.63
50Hz energy-envelope corr tch-vs-sidecar = 1.000 (ASR: both "Hello world.")
```
(precision: CPU fp32 strict LAW = 544/544; CUDA bf16 codec/synth; RTF 3.63)

### dia-1.6B (Nari, DAC codec) — AR codec-TTS — PASS
- `cpu_fp32_raw_codes_byte_identical` → **PASS**: `CPU raw-code byte-identity: 23409/23409 over 2601 frames; first-div=None` (9 codebooks × 2601 frames, CPU fp32 greedy), 726s.
- `cuda_torch_dia` → **PASS** (CUDA bf16, 168s):
```
Gate 2: CUDA bf16 raw codes vs sidecar (greedy) tch=9x2601 ref=9x2601
✔ channel-0 (EOS spine) byte-identical over all 2601 frames; all-9-channel prefix 21 frames
   (per-channel divergences = documented 0.0-gap bf16 ties; strict all-frames LAW on CPU-fp32)
DAC/synth parity: 50Hz energy-envelope corr tch-vs-sidecar = 0.986 | RTF 2.85
```
(precision: CPU fp32 strict LAW = 23409/23409 byte-identical; CUDA bf16 ch-0 all-frames + bf16-tie floor; DAC corr 0.986; RTF 2.85)

### CSM-1B (Sesame, dual-AR + Mimi codec) — PASS
- `cuda_csm_codes_byte_identical_to_sidecar` → **PASS** (CUDA bf16, 33s):
```
[L3] LAW PASSED: GREEDY CUDA-bf16 codes BYTE-IDENTICAL to sidecar golden (125 frames × 32 codebooks)
[L4] seeded-sampled tracks sidecar for 69 frames (documented 1-ULP drift @frame 69 — known op-match gap, not a floor)
```
- `cuda_csm_rtf` → **PASS**: `[RTF] csm CUDA-bf16: 10.62s wall for 10.00s audio → RTF 1.062`
(precision: CUDA bf16; greedy LAW = byte-identical all 125×32; RTF 1.062 — near-realtime)

### dots.tts (base/soar/mf — one arch, 3 weights) — CUDA bf16
- `cuda_torch_dots` (base) → **PASS**: `✔ all 10240 latent values byte-identical to the CUDA-bf16 sidecar (seed 0)`;
  envelope corr 1.0000, sample max|Δ| 1.481e-3 (f32 BigVGAN vocoder BLAS floor), RTF 2.278, 19.5s.
- `cuda_torch_dots` (soar) → **PASS**: 9728 latents byte-identical, corr 1.0000, max|Δ| 1.274e-3, RTF 2.286.
- `cuda_torch_dots` (mf, meanflow) → **PASS**: 10240 latents byte-identical, corr 1.0000, max|Δ| 1.534e-3, RTF 1.994.

### neutts_air (Neuphonic, Qwen2-0.5B + NeuCodec ONNX) — PASS
- `cpu_f32_byte_identical_to_reference` → **PASS**: `llm_hidden maxΔ=0e0`, `first-step logits maxΔ=0e0`,
  `greedy(CPU f32) 96/96 codes, 0/96 differ` (42s).
- `cuda_bf16_greedy_codes_byte_identical` → **PASS**: `greedy(CUDA bf16) 96/96 codes, 0/96 differ` (8s).
- `cuda_bf16_synthesizes_and_reports_rtf` → **PASS**: `164 codes → 3.26s audio in 2.51s → RTF 0.768` (CUDA bf16, sub-realtime).

### qwen3-tts-12hz-0.6b (dual-AR, Qwen3 talker + 12Hz codec) — PASS
```
[L3a] LAW PASSED: PREFILL talker hidden BYTE-IDENTICAL (Δ==0 over 1024 dims)
[L3b] LAW PASSED: FIRST-DECODE talker hidden BYTE-IDENTICAL (Δ==0 over 1024 dims)
[L3c] greedy tracks fused-SDPA golden 44 frames (tch T=51 / golden T=50); tail divergence = bf16 SDPA-tie floor
[L5] codec decode on golden codes: max|Δ|=0.03271 corr=0.9999
```
(precision: CUDA bf16; dual-AR talker-hidden Δ==0 LAW; greedy tracks then documented SDPA-tie tail; codec corr 0.9999; 14s)

### higgs-audio-v3-tts-4b (Boson, Qwen3-4B + DAC codec) — PASS
- `cpu_f32_byte_identical_to_reference` → **PASS** (CPU f32, 58s):
```
audio_embed maxΔ=0e0 | codec maxΔ=0e0 | llm_hidden(18×2560) maxΔ=0e0 | first-frame head logits maxΔ=0e0
greedy codes: 0/264 positions differ (byte-identical)
```
- `cuda_f16_synthesizes_and_reports_rtf` → **PASS**: `76800 samples (3.20s) in 4.46s → RTF 1.394` (CUDA f16, non-silent).
(precision: CPU f32 strict LAW = all seams Δ=0 + greedy 0/264; CUDA f16 synth RTF 1.394)

### higgs-audio-v2-3b (DualFFN/MoE arch — distinct from v3) — PASS
- `cpu_f32_byte_identical_to_reference` → **PASS** (CPU f32, 88s):
```
audio_embed maxΔ=0e0 | llm_hidden(38×3072) maxΔ=0e0 | first-frame head logits maxΔ=0e0 | codec maxΔ=0e0
greedy: 56 vs golden 48 frames; byte-identical for first 21 frames (first fork @frame 21 — bf16/MoE-tie floor)
```
(precision: CPU f32; all deterministic seams Δ=0 + byte-identical greedy prefix; fork = documented tie floor)

### irodori-TTS-500M (Japanese rectified-flow DiT over DACVAE latents) — PASS
- `cuda_torch_irodori_latent_byte_faithful` → **PASS**: `latent max|Δ| vs golden = 1.955e-4`, wav max|Δ|=1.625e-4 (ref RMS 0.126), CPU f32, 11s.
- `cuda_torch_irodori_gpu_synth` → **PASS**: `RTF=0.162 | latent max|Δ| vs CPU-golden=3.811e-4` (CUDA, non-AR flow-matching — sub-realtime).
(precision: CPU f32 latent LAW max|Δ|~1.96e-4; CUDA tracks CPU golden 3.81e-4; RTF 0.162)

### cosyvoice3 (Qwen2 speech-LM + CFM flow + HiFT) — PASS
```
[3b] ✓ AR speech-token sequence BYTE-IDENTICAL to the sidecar (123 tokens, first-div None)
[3a] flow→mel→vocoder RTF 0.31 | e2e RTF 0.49
```
(precision: CUDA bf16; AR speech-token LAW byte-identical (golden via WAAV_CV3_GOLDEN=cosyvoice3/.cv3_golden); RTF 0.49)

### omnivoice (masked-diffusion-LM TTS) — SKIP (golden absent)
All 6 sub-gates SKIP — `/tmp/omni_golden` empty (codec/hidden/lp0/tokens/wav goldens missing). Byte-identity NOT verified this run; model not loaded/synthesized. Gate returns ok but is a no-op without the golden.

### vibevoice-1.5b (AR + diffusion TTS) — PASS (e2e only; byte-identity golden absent)
```
[L6] e2e: 30 tokens (28 diffusion), 89600 samples (3.73s), 2.36s → RTF=0.632, rms=0.2137
[L6] ✅ e2e correct + structurally faithful
```
Byte-identity seam layers (L1-L5) self-skipped (`/tmp/vv_golden` absent); e2e structural + RTF 0.632 verified (CUDA bf16).

### voxtral-tts (Voxtral-4B-TTS) — SKIP (golden absent)
`cpu_voxtral_tts_codes_byte_identical_to_reference` → SKIP: "missing weights or golden" (`/tmp/voxtral_golden` empty), 0.00s. Not run.

## CUDA-graph perf lever (iterative-perf-WITHOUT-accuracy-loss evidence)

The `*_graph_ab` gates prove the captured CUDA-graph is BYTE-IDENTICAL to eager (no accuracy loss); the
`*_graph_perf` gates measure the speedup.

### dia2 backbone CUDA-graph
- `dia2_graph_capture_vs_eager_trace` → **PASS**: `✔ backbone + depformer CUDA-graph capture == eager on all 1188 sample calls (byte-identical, BIT-FAITHFUL)`.
- `dia2_backbone_graph_perf_ab` → **PASS**: `AR-gen: 19718 ms → 16678 ms speedup ×1.182 (+15.4%) (same 116 frames)`.

### csm depth-decoder CUDA-graph
- `csm_graph_capture_vs_eager_codes` → **PASS**: `✔ capture == eager on all 125 frames × 32 codebooks (byte-identical, BIT-FAITHFUL)`.
- `csm_depth_graph_perf_ab` → **PASS**: `AR-gen: 9761 ms → 7713 ms speedup ×1.266 (+21.0%)`.

### dots DiT (flow-matching ODE) CUDA-graph
- `dots_dit_graph_capture_vs_eager` → **PASS**: `0/10240 latents differ; max|Δ|=0.000e0` (byte-identical).
- `dots_dit_graph_perf_ab` → **PASS**: `speedup ×1.005 (+0.5%)` — marginal (per-patch re-capture overhead; documented edge-only gain for DiT flow-matching).

### omnivoice masked-diffusion CUDA-graph (capture-vs-eager — needs NO external golden)
- `omnivoice_graph_capture_vs_eager` → **PASS**: `0/288 differ; ✔ capture == eager (single-forward argmax + full generation) byte-identical`.
- `omnivoice_graph_perf_ab` → **PASS**: `wall 4086 ms → 3836 ms speedup ×1.065 (+6.1%)`; RTF≈0.321 graph-ON (model DOES load + synthesize ~12s audio live).

## TensorRT perf lever — UNAVAILABLE in this environment (honest FAIL: runtime missing)
The TRT gates are `#![cfg(all(feature="cuda", accel_tensorrt))]`-gated. Built correctly with
`RUSTFLAGS="--cfg accel_tensorrt"` (the first run without it compiled to 0 tests — a no-op). With the cfg they
COMPILE + RUN but **all 6 FAIL** (neutts_trt fp16, neutts_trt_lowp int8/fp8/nvfp4, dia_trt, higgs_trt):
```
neutts TRT: Fp16 engine load failed (trt load (CModule::load): Internal torch error:
  Unknown type name '__torch__.torch.classes.tensorrt.Engine'
loaded neutts/dia/higgs (CUDA) ... ; trt_active = false
panicked: the TRT engine must be loaded (WAAV_*_TRT=1 + .ts staged + accel mapper picks torch-tensorrt)
```
**Root cause (environment, NOT a model regression):** the Torch-TensorRT runtime (`libtorchtrt_runtime.so` +
`torch_tensorrt`/`tensorrt_libs`, pointed at by `WAAV_TORCHTRT_LIB`/`WAAV_TENSORRT_LIB`) is **not installed
anywhere on this box** (`find` finds none; `import torch_tensorrt` fails), so the `.ts` engine's custom TensorRT
op class is unregistered and the engine cannot deserialize. The base models DO load on CUDA (dia 13.8s, higgs
30.8s, neutts 7.0s — all `trt_active=false` fall-back). The `.ts` engine files themselves ARE staged
(neutts fp16/fp8/int8/nvfp4, dia decoder_fp16, higgs backbone_fp16). **To run: stage torch_tensorrt + tensorrt_libs
and set WAAV_TORCHTRT_LIB/WAAV_TENSORRT_LIB on LD_LIBRARY_PATH (the B49/B52/B54/B55 recipe).** Not a regression.

## S2S + codec-enhance + remaining

### hibiki-zero-3b (S2S duplex, Mimi codec) — PASS (CPU f32)
```
Mimi encode: 128 codes, 0 mismatches vs golden (8 frames × 16 cb) — BYTE-IDENTICAL
duplex greedy: 96 comparable target codes, 0 mismatches; first=None — S2S BYTE-IDENTICAL
hibiki duplex RTF: 0.640s audio in 14.607s wall → RTF=22.823 (CPU EP — gate runs CPU-f32; not a CUDA RTF)
```
(precision: CPU f32 — golden is CPU-f32; S2S duplex greedy byte-identical; RTF is CPU not GPU)

### pocket-tts (on-device AR, Mimi codec) — PASS
- `pocket_tts_byte_faithful_cpu` → **PASS**: e2e wav corr 1.000000, byte-faithful (CPU f32, 0.8s).
- `pocket_tts_byte_faithful_cuda` → **PASS**: e2e wav corr 0.999999, byte-faithful (CUDA, 0.9s).

### RSB (Resemble-Enhance, BigGAN-UNet SDE enhance) — PASS
- `enhance_matches_golden` → **PASS**: `RSB enhance byte-faithful: max|Δ| = 2.68e-7 (cuda)` vs CPU/CUDA-f32 golden (1.1s).

### viitorvoice-nar — SKIP (golden absent)
All 4 sub-gates SKIP — prompt_row0/codec/codes/wav goldens missing in `viitorvoice-nar/`. Model loads but byte-identity NOT verified.

### LFM2.5-Audio-1.5b (S2S, ORT/registry) — PASS
- `lfm2_audio_asr_via_registry_matches_golden` → **PASS**: transcript == ONNX golden exactly; RTF 0.280 (CPU EP).
- `lfm2_audio_s2s_round_trip_speech_to_speech` → **PASS**: full speech-in → reply text + speech-out round-trip; turn 5.94s (CPU EP, ORT — greedy/deterministic == golden).
(precision: ORT CPU; ASR transcript-exact + S2S round-trip; LFM2 is an ONNX model, not tch)
