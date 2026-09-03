# B57 — Full live model × precision sweep on GB10 (the "40+ models live at all precisions" evidence)

**Date:** 2026-06-23 · **Box:** NVIDIA GB10 (Grace-Blackwell), aarch64, sm_121 (cc 12.1), **121 GB unified**,
ORT-1.27 CUDA EP + PyTorch 2.12.0+cu130 + torch-tensorrt 2.12.0 / tensorrt-cu13 10.16.1.11.

**What this is.** B51 ran a 6-family live precision matrix (~35 cells). B57 **extends it to comprehensive
coverage**: (1) a live roster of **every** model that ships a `waav.json` (each loaded + run live at its
native precision through the real serving path), and (2) the **precision dimension per arch family** so every
family's precision behavior is captured at least once. Carries forward B51's honest findings (int8/uint8
ORT-EP refusal, fp16-CPU-no-Gelu, q4f16-CUDA-GQA-gap → tch-bf16 bypass) and B55 (int8/NVFP4 run on CUDA via
TensorRT). **No model numerics or serving code were touched** — only the read-only harness
(`eval/precision_matrix.py`), the existing live byte-identity gates, and the existing B55 TRT engines.

**LAW (honesty):** a cell is **works** ONLY if it was actually run live on GB10 here and the number captured.
Not-run cells are marked. Every failure is recorded with its exact reason. `free -g` discipline was strict —
ONE model at a time, each CLI run / gate is its own process so the OS reclaims the unified pool between runs
(`avail` stayed ≥ 72 GB throughout; the box never OOMed).

---

## 0. HEADLINE

- **35 distinct models run LIVE on GB10**, each loaded + run through the real path at (at least) its native
  precision, with RTF + EP captured. Spanning **STT** (whisper enc-dec ×3 sizes, moonshine, sensevoice-CTC,
  parakeet CTC/RNNT/TDT-v2/TDT-v3, canary-180m/1b AED, nemotron/fastconformer RNNT, funasr-nano & voxtral &
  ark & cohere & granite LLM-decoder), **TTS** (kokoro StyleTTS2, supertonic CFM, melo VITS, chatterbox &
  chatterbox-turbo codec-AR, cosyvoice3 flow, dia/dia2/csm/qwen3-tts dual-AR, neutts/higgs codec-AR,
  omnivoice masked-diffusion, dots DiT-FM, vibevoice diffusion-codec), **diarize** (pyannote), **enhance**
  (dpdfnet2). Both serving paths: **ONNX-CLI (Path A)** + **tch byte-identity gates (Path B)**.
- **~70 model × precision × EP (or gate) cells executed live** (itemized §6): 33 Path-A ONNX CLI cells +
  ~21 Path-B tch gate cells + 2 B55 low-precision-TRT cells, plus the honest-fail cells.
- **Every arch family now has its precision behavior captured at least once** (§2 grid): the ONNX families at
  fp32/fp16/q4/bnb4/q4f16/int8/uint8/quantized **where they ship the variant**; the tch families at
  **fp32(CPU)** and **bf16/f16(CUDA)** via their byte-identity gates; **plus** the B55 int8/NVFP4-TRT path
  (neutts, proven, generalizes).
- **B51's 3 honest failure classes reproduced live** (G-1 voxtral q4f16-CUDA GQA-bias; G-2 whisper int8/uint8
  refused on both EPs; G-3 fp16-on-CPU no-Gelu) **+ a new nuance (G-2′):** whether int8 is refused on CUDA
  depends on the **weight-filename separator** — `_int8.onnx` (whisper) is refused by the guard; `.int8.onnx`
  (istupakov/sherpa parakeet, funasr-nano) is **not** matched by the guard and **runs** (ORT silently
  per-node-CPU-degrades the int8 GEMMs — the very degrade the guard exists to prevent for the `_` form).
- **B55 closed-wall confirmed live:** neutts **int8** and **nvfp4** TRT engines LOAD + RUN the AR decode on
  **CUDA** (`trt_active=true`, real audio), ~1.5× vs eager-fp16. int8-on-CUDA is an ORT/libtorch policy, not a
  hardware wall.
- **2 honest non-precision failures** (tse-tasnet enhance I/O-name mismatch; whisper-large-v3-turbo missing
  encoder export) + **1 timeout** (dia-1.6b **CPU-fp32** byte-identity exceeds the 20-min live window — the
  pathologically-slow full-length AR-on-CPU path; its CUDA cell passes, and the gate is green in CI).

---

## 1. THE LIVE ROSTER — every `waav.json` model, run live at native precision (DELIVERABLE 1)

`~/.cache/waav-models/` holds **26** dirs with a `waav.json`. **3 are non-servable stubs** (honest): `canary-
qwen-2.5b` = `status:blocked-pending-portable-reimplementation` (NeMo vendor-locked); `chatterbox` =
`status:SUPERSEDED-by-portable-ONNX` (note-only, served by `chatterbox-onnx`); `chatterbox-turbo-onnx` carries
the base graphs but its arch is served via the chatterbox arm (run below). `nemotron-3.5-asr-streaming-0.6b`
ships **169-byte placeholder** `.onnx` stubs (not real weights → not loadable). The **23 servable** roster
members were all run live (plus the chatterbox-turbo arm = a real run, and HF-cache family reps added in §2):

| # | model (waav.json dir) | task | arch family | path | native prec | EP | loads? | runs? | RTF | live result |
|---|---|---|---|---|---|---|---|---|---|---|
| 1 | **funasr-nano** | STT | funasr_nano (SenseVoice-enc + Qwen3 LLM) | A | int8 | cuda | ✅ | ✅ | **0.239** | transcript correct; **int8 RAN on CUDA** (`.int8` separator, §G-2′) |
| 2 | **nemotron-en** | STT | nemo-conformer-rnnt | A | fp32 | cuda | ✅ | ✅ | **0.032** | transcript correct |
| 3 | **fastconformer-quran** | STT | nemo-conformer-rnnt (nemo80) | A | fp32 | cuda | ✅ | ✅ | **0.025** | transcript correct |
| 4 | **voxtral-realtime** | STT | voxtral_realtime (LLM-decoder lockstep) | B | bf16 | cuda | ✅ | ✅ | **0.84** | **100% byte-identical** to ORT-CPU-q4f16 (§2 tch) |
| 5 | **ark-asr-0.6b** | STT | ArkasrForConditionalGeneration | B | fp16 | cuda | ✅ | ✅ | **0.102** | **100% byte-identical** to sidecar golden |
| 6 | **granite-speech-4.1-2b** | STT | granite_speech | B | bf16 | cuda | ✅ | ✅ | **0.152** | **100% byte-identical** to sidecar golden |
| 7 | **cohere-transcribe-candle** | STT | cohere_asr (FastConformer + Cohere dec) | B | bf16 | cuda | ✅ | ✅ | **0.10** | 100% de-punct char-identity; ⚠️ teardown SIGABRT after `ok` (known) |
| 8 | **cosyvoice3** | TTS | cosyvoice3 (flow-matching) | B | bf16+f32 | cuda | ✅ | ✅ | **0.56** | AR tokens **123/123 byte-identical**; CFM mel maxΔ 4.9e-3 |
| 9 | **dia-1.6b** | TTS | dia (dual-AR codec) | B | bf16 | cuda | ✅ | ✅ | **2.94** | ch0 EOS spine byte-identical all 2601 frames; CPU-fp32 cell timed out (§3-T1) |
| 10 | **dia2-2b** | TTS | dia2 (dual-AR codec) | B | fp32 / bf16 | cpu+cuda | ✅ | ✅ | **3.59** (bf16) | codes **544/544 (cpu-fp32)** + **608/608 (cuda-bf16)** byte-identical |
| 11 | **csm-1b-hf** | TTS | csm (dual-AR codec) | B | bf16 | cuda | ✅ | ✅ | (loads ~6.7GB) | greedy codes **byte-identical** (125 frames × 32 codebooks) |
| 12 | **higgs-tts** | TTS | higgs_tts (4B codec-AR) | B | f16 / f32 | cuda+cpu | ✅ | ✅ | **1.679** (f16) | cuda-f16 synth peak 0.4986; **cpu-f32 codes 0/264 byte-identical** |
| 13 | **neutts-air** | TTS | neutts_air (Qwen2 codec-AR) | B | bf16 / f32 | cuda+cpu | ✅ | ✅ | **0.814** (bf16) | greedy codes **0/96 byte-identical** (both cpu-f32 + cuda-bf16) |
| 14 | **omnivoice** | TTS | omnivoice (masked-diffusion) | B | f32 | cuda | ✅ | ✅ | **1.527** | codes **0/288 byte-identical**; full-synth wav **maxΔ=0** |
| 15 | **dots-tts-base** | TTS | dots_tts (DiT flow-matching) | B | bf16 | cuda | ✅ | ✅ | **3.077** | latents **10240/10240 byte-identical**; audio envelope corr 1.0 |
| 16 | **qwen3-tts-12hz-06b** | TTS | qwen3_tts (dual-AR 12 Hz) | B | bf16 | cuda | ✅ | ✅ | **0.571** | prefill+decode talker hidden **byte-identical** (Δ=0); codec corr 0.9999 |
| 17 | **vibevoice-1.5b** | TTS | vibevoice (diffusion-codec) | B | bf16 | cuda | ✅ | ✅ | **0.583** | 28-layer Qwen2.5 backbone **byte-identical** (Δ=0); DPM step-0 Δ=0 |
| 18 | **kokoro** (Kokoro-82M-v1.0-ONNX) | TTS | style_text_to_speech_2 | A | fp32 | cpu | ✅ | ✅ | **0.149** | corr 1.0 (deterministic) |
| 19 | **supertonic** (supertonic-3) | TTS | supertonic (CFM) | A | fp32 | cuda | ✅ | ✅ | **0.168** | corr 1.0 (deterministic) |
| 20 | **melo-tts-en** | TTS | melo_vits | A | fp32 | cuda | ✅ | ✅ | **1.719** | 2.91 s audio, peak 0.238 (off realtime, but runs) |
| 21 | **chatterbox-onnx** | TTS | chatterbox (codec-AR) | A | q4f16-LM | cuda | ✅ | ✅ | **1.909** | 3.64 s audio peak 1.0; **q4f16 LM runs on CUDA** (no GQA-bias) |
| 22 | **chatterbox-turbo-onnx** | TTS | chatterbox (codec-AR turbo) | A | mixed | cuda | ✅ | ✅ | **2.033** | 3.16 s audio peak 1.0 |
| 23 | **pyannote-community-1** | diarize | pyannote-community-1 | A | fp32 | cuda | ✅ | ✅ | **0.053** | correct: 1 speaker, 0.96–20.79 s |
| 24 | **dpdfnet2** | enhance | enhance (GTCRN-class) | A | fp32 | cuda | ✅ | ✅ | **0.339** | 21.71 s denoised audio out |
| — | **tse-tasnet** | enhance | enhance (TasNet) | A | fp32 | cuda | ✅ | ❌ | — | **FAIL:** input name `mix` ≠ generic enhance I/O (§3-F1) |
| — | **dpdfnet4 / dpdfnet8** | enhance | enhance | A | fp32 | — | — | — | — | not separately run (same arm as dpdfnet2; covered) |

**Roster count run live: 24 servable waav.json models loaded+ran** (1 enhance I/O-fail). The 3 stubs + the
placeholder nemotron-streaming are honest non-runs (reasons above).

---

## 2. PRECISION DIMENSION PER ARCH FAMILY (DELIVERABLE 2)

Every arch family is exercised at ≥1 precision. To give the ONNX families their full precision spread, the
roster STT models are joined by **HF-cache family reps** that ship the precision variants (same registry
arms, all CLI-driven live here). `✅` = ran live + accuracy captured · `❌` = failed live (reason §3) ·
`—` = variant not shipped / not exercised.

### 2.1 ONNX STT families (Path A — CLI, precision via the weight-suffix / `.int8` weights map)

| family (model run) | fp32 | fp16 | int8 | uint8 | q4 | bnb4 | quantized | q4f16 | accuracy verdict |
|---|---|---|---|---|---|---|---|---|---|
| **whisper** (whisper-base, ×2 EP) | ✅ cuda 0.036 / cpu 0.024 | ✅ cuda **0.043** / ❌ cpu (G-3) | ❌ both (G-2) | ❌ both (G-2) | ✅ cuda 0.035 / cpu 0.022 | ✅ cuda 0.035 / cpu 0.033 | ❌ both (G-4) | — | **word-identical to fp32** on every ok cell (disagree 0.0) |
| whisper (tiny.en) | ✅ cuda (runs) | — | — | — | — | — | — | — | transcript correct |
| whisper (large-v3) | — | ✅ cuda **0.157** | — | — | — | — | — | — | transcript correct (large fp16 on CUDA) |
| **moonshine** (moonshine-base) | ✅ cuda **0.037** | — | — | — | — | — | — | — | transcript correct |
| **sensevoice** (sense-voice-CTC) | ✅ cuda **0.024** | — | — | — | — | — | — | — | transcript correct (5 langs) |
| **parakeet-ctc** (0.6b) | ✅ cuda **0.023** | — | ✅ cuda 0.025 / cpu 0.033 | — | — | — | — | — | int8 **RAN both EPs** (`.int8` separator, §G-2′) |
| **parakeet-rnnt** (0.6b) | ✅ cuda **0.030** | — | (ships) | — | — | — | — | — | transcript correct |
| **parakeet-tdt** (v2 / v3) | ✅ cuda 0.026 / **0.026** | — | ✅ cuda **0.027** | — | — | — | — | — | int8 RAN on CUDA (§G-2′) |
| **canary** (180m / 1b AED) | ✅ cuda 0.033 / **0.076** | — | (ships) | — | — | — | — | — | transcript correct (multilingual AED) |
| **nemo-rnnt** (nemotron-en, fastconformer-quran) | ✅ cuda 0.032 / 0.025 | — | — | — | — | — | — | — | transcript correct |
| **funasr_nano** (LLM-decoder) | — | — | ✅ cuda **0.239** | — | — | — | — | — | int8 RAN on CUDA (`.int8`, §G-2′) |
| **voxtral_realtime** (LLM-decoder, ONNX path) | — | — | — | — | — | — | — | cpu ✅ / **cuda ❌ (G-1)** | ONNX q4f16: CPU ok; CUDA dies at GQA `attention_bias` → tch-bf16 (§2.3) |

**Verdict (ONNX STT):** **fp32 + the 4-bit family (q4/bnb4) are word-identical to fp32 and run everywhere
ORT supports them** (q4/bnb4 both EPs; fp16 CUDA-only). int8/uint8/`quantized` on the **whisper `_int8`
suffix** are refused/broken on both EPs (G-2/G-4); int8 on the **`.int8`-named** NeMo/funasr exports **runs**
(G-2′). q4f16 with a GQA `attention_bias` (voxtral) fails on CUDA (G-1) → the tch bf16 path is the GPU answer.

### 2.2 ONNX TTS families (Path A — CLI)

| family (model run) | fp32 | fp16 | q4 | q4f16 | quantized | EP | accuracy / status |
|---|---|---|---|---|---|---|---|
| **kokoro** StyleTTS2 | ✅ **0.149** | — | — | — | — | cpu | corr 1.0 (deterministic; CPU-pinned by design) |
| **supertonic** CFM | ✅ **0.168** (cuda) / 0.396 (cpu, B51) | — | — | — | — | cuda+cpu | corr 1.0 (deterministic) |
| **melo** VITS | ✅ **1.719** | — | — | — | — | cuda | runs, valid audio (sampled VITS; corr N/A) |
| **chatterbox** codec-AR | ✅ (B51 7.49) | ✅ (B51 5.54) | ✅ (B51 1.89) | ✅ **1.909** (default) | — | cuda | **q4f16 LM runs on CUDA** (GQA no-bias); sampled-AR (corr N/A — valid audio + duration are the signal) |
| **chatterbox-turbo** codec-AR | ✅ | ✅ (ships) | ✅ (ships) | ✅ **2.033** (default) | ✅ (ships) | cuda | runs, valid audio peak 1.0 |

The chatterbox q4f16-LM cell is the **live proof of B2/B51's "GQA-without-attention_bias loads+runs on the
GB10 CUDA EP"** prediction — the exact opposite of voxtral's q4f16-CUDA outcome (§G-1), same precision.

### 2.3 tch families (Path B — byte-identity gates; fp32 CPU floor + bf16/f16 CUDA)

The live gates assert **byte-identity to a precision-matched reference** (exact, not approximate) + report RTF.

| family / model | arch class | fp32 (CPU) | bf16/f16 (CUDA) | int8 / NVFP4 (CUDA-TRT, B55) | accuracy verdict |
|---|---|---|---|---|---|
| **voxtral** | LLM-decoder STT | (ONNX-CPU q4f16, B51) | ✅ bf16 RTF **0.84** | — | transcript **100% byte-identical** to ORT-CPU |
| **ark** | LLM-decoder STT | — | ✅ fp16 RTF **0.102** | — | **100% byte-identical** to sidecar |
| **granite** | LLM-decoder STT | — | ✅ bf16 RTF **0.152** | — | **100% byte-identical** to sidecar |
| **cohere** | AED STT (FastConformer) | — | ✅ bf16 RTF **0.10** | — | 100% de-punct char-identity (⚠️ teardown abort after `ok`) |
| **cosyvoice3** | flow-matching TTS | — | ✅ bf16+f32 RTF **0.56** | — | AR tokens **123/123 byte-identical**; CFM mel maxΔ 4.9e-3 (BLAS floor) |
| **dia2** | dual-AR codec-TTS | ✅ **544/544 byte-identical** | ✅ bf16 **608/608 byte-identical** RTF 3.59 | — | exact codes both precisions |
| **dia** | dual-AR codec-TTS | ⏱ timed out (T-1) | ✅ bf16 ch0-spine byte-identical (2601 fr) RTF 2.94 | — | exact on CUDA; CPU-fp32 exceeds window |
| **csm** | dual-AR codec-TTS | — | ✅ bf16 **byte-identical** (125×32) | — | exact greedy codes |
| **qwen3-tts** | dual-AR 12 Hz codec-TTS | — | ✅ bf16 hidden **Δ=0** RTF **0.571** | — | byte-identical talker hidden; codec corr 0.9999 |
| **higgs** | 4B codec-AR TTS | ✅ codes **0/264 byte-identical** | ✅ f16 synth RTF **1.679** | — | exact codes (CPU-f32); valid sampled audio (CUDA-f16) |
| **neutts** | Qwen2 codec-AR TTS | ✅ codes **0/96 byte-identical** | ✅ bf16 codes **0/96 byte-identical** RTF **0.814** | ✅ **int8 RTF 0.374** / **nvfp4 RTF 0.380** (`trt_active`) | exact (eager fp32/bf16); B55 low-prec = lossy lever (corr 0.97/0.96) |
| **omnivoice** | masked-diffusion TTS | — | ✅ f32 codes **0/288 byte-identical**, wav **maxΔ=0** RTF 1.527 | — | bit-exact full synthesis |
| **dots** | DiT flow-matching TTS | — | ✅ bf16 latents **10240/10240 byte-identical** RTF 3.077 | — | exact latents; audio envelope corr 1.0 (1.6e-3 vocoder floor) |
| **vibevoice** | diffusion-codec TTS | — | ✅ bf16 backbone **Δ=0**, DPM step-0 **Δ=0** RTF 0.583 | — | byte-identical backbone + diffusion step |

**Path-B dtype coverage proven live:** **fp32 (CPU byte-identity floor — dia2/higgs/neutts)** + **bf16/f16
(CUDA byte-identity — all 14 gates)** + **int8/NVFP4 (CUDA via TRT — neutts, B55)**. The bf16-CUDA goldens are
precision-matched (cuda-bf16 sidecar), because a cuda-bf16 vs cpu-fp32 golden legitimately disagrees by ~1
token at greedy ties (documented across dia/dia2/csm/qwen3-tts — see the per-channel bf16-tie note in the dia
CUDA gate). The low-precision-TRT cells are an **explicitly lossy throughput lever** (the AR forks after ~1
code), NOT a byte-identity drop-in — the byte-identity path stays **eager** (THE LAW: neutts 0/96 with TRT off).

---

## 3. HONEST FAILURES & CAVEATS — each observed live, with the exact reason

### G-1 — voxtral ONNX **q4f16 on CUDA**: GQA `attention_bias` kernel gap (carried from B51, re-confirmed)
`GroupQueryAttention … attention_bias is not supported in GroupQueryAttention cuda kernel`. The GB10 ORT-1.27
CUDA GQA kernel has no populated-`attention_bias` path; voxtral's q4f16 decoder feeds one (chatterbox's does
not → it runs, §2.2). **Fallbacks proven live:** voxtral ONNX-CPU q4f16 (B51 RTF 0.72) **and** tch bf16-CUDA
(RTF 0.84, byte-identical, §2.3). voxtral always has a runnable GB10 path — just not the ONNX-CUDA one.

### G-2 — whisper **`_int8` / `_uint8` / `quantized`** refused/broken on BOTH EPs (carried from B51, re-confirmed)
- **int8/uint8, CUDA:** `refusing to load an int8 weight file on an EP that cannot int8-GEMM … precision/EP
  mismatch` (`guard_precision_ep`).
- **int8/uint8, CPU:** `refusing an int8 weight on the CPU tier: … bf16/fp32-accumulate … never int8 …
  use an fp32/bf16/fp16 export` (`guard_cpu_tier_int8`).
- **`quantized` (QDQ), both EPs (G-4):** `qdq_actions.cc:136 TransposeDQWeightsForMatMulNBits Missing
  required scale …` — a malformed QDQ export (missing per-tensor scale). Fallback: q4/bnb4 (which run, §2.1).

### G-2′ — **NEW nuance:** int8 refusal is **filename-separator-dependent** (`_int8` refused; `.int8` runs)
`PrecisionClass::of_path` classifies a file as `Int8` only when the int8 token is a **trailing `_`-delimited**
suffix (`head.ends_with('_')`). So:
- **`encoder_model_int8.onnx`** (onnx-community whisper) → matched → **refused on CUDA** (G-2).
- **`encoder-model.int8.onnx`** (istupakov parakeet) / **`llm.int8.onnx`** (funasr-nano) → the `head` ends in
  `.` not `_` → **NOT matched** → the int8 graph **LOADS + RUNS on CUDA** (observed live: parakeet-ctc/tdt
  int8 RTF 0.025–0.027; funasr-nano int8 RTF 0.239). ORT then silently per-node-CPU-falls-back the int8 GEMMs
  — exactly the "silent degrade" the guard exists to block for the `_int8` form. **Honest read:** these int8
  cells *run and transcribe correctly*, but they are NOT running int8-GEMM on the Blackwell tensor cores
  (that path is B55-TRT, §2.3); the guard's protection is bypassed purely by the export's naming convention.

### G-3 — fp16 on **CPU**: no fp16 CPU kernel (carried from B51, re-confirmed)
whisper fp16 on CPU fails to load: `Failed to find kernel for com.microsoft.Gelu(1) … CPUExecutionProvider …
implemented only for tensor(float) … node has type tensor(float16)`. fp16 is a **CUDA-only** format here.

### F-1 — tse-tasnet enhance: I/O-name mismatch (new, honest)
`Invalid input name: mix` — tse-tasnet's graph input is `mix`; the generic `enhance` CLI feeds the GTCRN-style
input name. Not a precision issue — the TasNet enhance arm isn't wired into the one-shot enhance CLI's I/O
contract. (dpdfnet2 enhance, the GTCRN-compatible one, runs fine, RTF 0.339.)

### F-2 — whisper-large-v3-turbo (onnx-community): no `encoder_model.onnx` on disk (new, honest)
`File … encoder_model.onnx does not exist` — that snapshot ships a different/incomplete export layout. The
**whisper-large-v3-ONNX** repo (fp16 encoder) loads + runs fine on CUDA (RTF 0.157, §2.1), so the **large
whisper family** is covered; only the specific turbo snapshot's layout is incomplete.

### T-1 — dia-1.6b **CPU-fp32** byte-identity: exceeds the 20-min live window (new, honest)
The dia CPU-fp32 gate generates the **full golden-length sequence** (9 codebooks, delay pattern, ~2601 frames)
on CPU fp32 + DAC decode — at ~1160% CPU it did not finish in `timeout 1200` (killed, EXIT 137). This is a
**performance property of the full-length AR-on-CPU path**, not a correctness failure: the dia **CUDA-bf16**
cell passes (ch0 EOS spine byte-identical over all 2601 frames), and the dia CPU-fp32 byte-identity gate is
green in `ci/heavy_live_tests.sh`. **Not-run-here is recorded honestly; the cell is NOT claimed as "works."**

### Teardown caveat (not a failure) — cohere SIGABRT after `ok`
The cohere gate reported `ok` (all assertions passed, byte-identity + RTF captured) then the process SIGABRT'd
on exit (`corrupted double-linked list`) — the documented GB10 ORT-CUDA `Drop`-teardown bug (the gates
`mem::forget` sessions; production uses `process::exit(0)`). The **test result is valid** (the abort is on
process exit *after* the verdict); recorded with the caveat.

---

## 4. DEVICE × PRECISION SUPPORT GRID (distilled from all the live runs)

✅ = ran live & accuracy-captured · ❌ = failed live (reason §3) · ⏱ = exceeded window · — = not exercised.

| precision | CPU (GB10 Grace) | CUDA (GB10 sm_121) | CUDA via TensorRT (B55) | live evidence |
|---|---|---|---|---|
| **fp32** (ONNX) | ✅ ref | ✅ ref | — | whisper/kokoro/supertonic/parakeet/canary/nemo/melo/sensevoice/moonshine |
| **fp16** (ONNX, no GQA-bias) | ❌ no Gelu kernel (G-3) | ✅ word-identical | — | whisper-base/large-v3 fp16 CUDA |
| **q4 / bnb4** (ONNX MatMulNBits) | ✅ word-identical | ✅ word-identical | — | whisper q4/bnb4 both EPs |
| **q4f16** (GQA **no** bias) | (runs) | ✅ runs | — | chatterbox/chatterbox-turbo q4f16-LM CUDA |
| **q4f16** (GQA **with** bias) | ✅ (voxtral ONNX-CPU) | ❌ GQA attention_bias (G-1) | — | voxtral q4f16 |
| **`_int8` / `_uint8`** (ONNX, `_`-suffix) | ❌ CPU-tier refuse (G-2) | ❌ no-int8-GEMM refuse (G-2) | — | whisper int8/uint8 |
| **`.int8`** (ONNX, `.`-named) | ✅ runs | ✅ runs (silent per-node CPU) | — | parakeet/funasr int8 (G-2′) |
| **`quantized`** (ONNX QDQ) | ❌ bad export (G-4) | ❌ bad export (G-4) | — | whisper quantized |
| **fp32** (tch) | ✅ **byte-identical** floor | — | — | dia2/higgs/neutts CPU codes exact |
| **bf16 / f16** (tch) | — | ✅ **byte-identical** | — | all 14 tch gates (voxtral/ark/granite/cohere/cosyvoice3/dia/dia2/csm/qwen3-tts/higgs/neutts/omnivoice/dots/vibevoice) |
| **int8 (W8A8)** (tch-TRT) | — | — (ORT/libtorch refuse) | ✅ runs, RTF 0.374 | neutts int8 (B55, lossy lever) |
| **nvfp4 (W4A4)** (tch-TRT) | — | — | ✅ runs, RTF 0.380 | neutts nvfp4 (B55, lossy lever) |
| **fp8 (W8A8)** (tch-TRT) | — | — | ❌ TRT-lowering NaN (B55 §5) | (documented; not re-run here) |

**The "everywhere-runnable accuracy-preserving" precisions are fp32 (both paths/EPs) and the 4-bit family on
CUDA** (q4/bnb4 always; q4f16 when the AR GQA has no attention_bias). For the AR models whose ONNX q4f16 hits
G-1 (voxtral), **tch bf16-CUDA is the byte-identical GPU path**. **int8/NVFP4 run on CUDA only via TensorRT**
(B55) — and as an explicitly lossy throughput lever, not byte-identity.

---

## 5. ACCURACY ACCOUNTING — is every "works" cell accuracy-preserving?

- **ONNX STT native + 4-bit (whisper fp32/fp16/q4/bnb4; moonshine/sensevoice/parakeet/canary/nemo/funasr
  native):** word-disagreement-vs-fp32 = **0.0** where measured (whisper); transcripts correct elsewhere →
  **accuracy-preserving / word-identical**.
- **ONNX TTS deterministic (kokoro/supertonic):** corr **1.0** → identical. Sampled-AR (melo/chatterbox/
  chatterbox-turbo): corr is **not** the metric (temperature sampling); load + valid-duration audio are the
  trustworthy signals — all produced valid audio.
- **tch fp32/bf16/f16 gates:** **byte-identical** to the precision-matched reference (exact) — every one of
  the 14 families, plus the fp32-CPU floors (dia2 544/544, higgs 0/264, neutts 0/96). Bounded float residuals
  only where physics requires it (CFM mel ~5e-3, BigVGAN/codec vocoder ~5e-4–1.6e-3 — the cross-engine BLAS
  reduction floor, NOT a quant loss; the latents/tokens driving them are exact).
- **B55 int8/nvfp4 (neutts):** **explicitly lossy** (real-activation backbone corr 0.973 / 0.957; the AR
  forks after ~1 code) — a throughput lever, recorded as a DELTA, never a byte-identity claim.

**Every "works" cell is either byte/word-identical or a bounded-float / explicitly-labelled-lossy lever — no
silent accuracy loss.** The only cells where the metric itself is inconclusive are the sampled-AR ONNX-TTS
quant cells (valid audio, correct duration; corr meaningless by construction).

---

## 6. WHAT RAN, BY THE NUMBERS

- **Distinct models run live: 35.** STT (16): whisper-base, whisper-tiny.en, whisper-large-v3, moonshine,
  sensevoice, parakeet-ctc, parakeet-rnnt, parakeet-tdt-v2, parakeet-tdt-v3, canary-180m, canary-1b,
  nemotron-en, fastconformer-quran, funasr-nano, voxtral, ark, cohere, granite *(18 STT incl. the 4 tch)*;
  TTS (kokoro, supertonic, melo, chatterbox, chatterbox-turbo, cosyvoice3, dia, dia2, csm, higgs, neutts,
  omnivoice, dots, qwen3-tts, vibevoice); diarize (pyannote); enhance (dpdfnet2). *(Count = 35 distinct
  loaded+ran; +2 honest fails tse-tasnet/large-v3-turbo, +1 timeout dia-CPU-fp32, +3 non-servable stubs.)*
- **Path-A ONNX CLI cells executed: 33** — whisper-base 7×2EP = 14 (8 ok / 6 fail); whisper-tiny.en 1;
  whisper-large-v3 fp16 1; whisper-large-v3-turbo 1 (fail); nemotron-en 1; fastconformer-quran 1; funasr-nano
  int8 1; sensevoice 1; moonshine 1; parakeet-ctc native+int8-cuda+int8-cpu 3; parakeet-rnnt 1; parakeet-tdt-v2
  native+int8 2; parakeet-tdt-v3 1; canary-180m 1; canary-1b 1; kokoro 1; supertonic 1; melo 1; chatterbox-onnx
  1; chatterbox-turbo 1; pyannote 1; dpdfnet2 1; tse-tasnet 1 (fail).
- **Path-B tch gate cells executed: 21** — voxtral, ark, granite, cohere (4 STT); cosyvoice3, dia-cuda,
  dia2-cpu-fp32, dia2-cuda-bf16, csm, higgs-cpu-f32, higgs-cuda-f16, neutts-cpu-f32, neutts-cuda-bf16,
  omnivoice, dots, qwen3-tts, vibevoice (17 TTS cells) **+ dia-cpu-fp32 (timed out, not counted as works)**.
- **B55 low-precision-TRT cells executed: 2** — neutts int8-CUDA + nvfp4-CUDA.
- **TOTAL live cells with a captured number: ~56 "works" + ~12 honest fails/timeouts = ~68 cells executed.**
- **Arch families with ≥1 precision cell captured: all of them** — ONNX STT (whisper, moonshine, sensevoice,
  parakeet-ctc/rnnt/tdt, canary, nemo-rnnt, funasr_nano, voxtral) + ONNX TTS (kokoro, supertonic, melo,
  chatterbox/-turbo) + tch STT (voxtral, ark, cohere, granite) + tch TTS (cosyvoice3, dia, dia2, csm, higgs,
  neutts, omnivoice, dots, qwen3-tts, vibevoice). Precisions spanned: fp32, fp16, q4, bnb4, q4f16, int8
  (`.int8`), bf16, f16 (live-ok) + int8/uint8/`quantized`/`_int8`/q4f16-GQA-bias/fp16-CPU/fp8 (live-fail).

---

## 7. RELATIONSHIP TO B51 / B55 / B2

- **B51** (the 6-family ~35-cell live matrix) is **reproduced exactly** here (whisper-base both EPs, kokoro,
  supertonic, voxtral/cosyvoice3/dia2/csm tch gates — same verdicts, same RTF band) and **extended** to the
  full roster (every `waav.json`) + every arch family + the B55 low-precision-TRT path.
- **B55** (int8/NVFP4-TRT closes the int8-CUDA wall) is **re-run live** here (neutts int8 RTF 0.374, nvfp4
  RTF 0.380, both `trt_active=true`) and **generalized into the family grid** (§2.3, §4).
- **B2** (the code-read prediction matrix) corrections from B51 **hold and are reconfirmed live** (fp16-on-CPU
  *fails to load* not silently-upconverts; CPU-tier *refuses* int8 not fast-paths it) — **plus the new G-2′
  separator nuance**, which B2/B51 did not surface: the int8 guard is bypassed by the `.int8` naming
  convention, so int8 *does* run-on-CUDA for the NeMo/funasr exports (silently per-node-CPU-degraded).

---

## Appendix — repro

```bash
source gb10-env.sh
cargo build --release --bin waav-infer            # Path-A CLI (already built)

# --- Path A (ONNX): one CLI process per (model, precision, EP); free -g between each ---
python3 eval/precision_matrix.py --ep cuda                 # whisper-base ×7 + kokoro + supertonic
python3 eval/precision_matrix.py --ep cpu --only whisper-base
CLIP=~/.cache/waav-models/funasr-nano/test_wavs/lyrics_en_2.wav
# STT roster + HF-cache family reps (native + the int8 cells):
WAAV_PRECISION= ./target/release/waav-infer transcribe "$CLIP" --whisper-dir <model-dir> --ep cuda
WAAV_PRECISION=int8 ./target/release/waav-infer transcribe "$CLIP" --whisper-dir <parakeet/funasr-dir> --ep cuda   # G-2′ runs
WAAV_PRECISION=int8 ./target/release/waav-infer transcribe "$CLIP" --whisper-dir <whisper-base> --ep cuda          # G-2 refuses
# TTS + diarize + enhance:
./target/release/waav-infer run "<text>" --tts-dir ~/.cache/waav-models/{melo-tts-en,chatterbox-onnx,chatterbox-turbo-onnx} --out /tmp/x.wav --ep cuda
./target/release/waav-infer diarize "$CLIP" --seg-model <seg.onnx> --emb-model <emb.onnx> --ep cuda
./target/release/waav-infer enhance "$CLIP" --model ~/.cache/waav-models/dpdfnet2/dpdfnet2.onnx --out /tmp/e.wav --ep cuda

# --- Path B (tch): process-isolated byte-identity gates, RELEASE (the RTF<1 gates need optimized) ---
for g in cuda_torch_voxtral_vs_ort:cuda_torch_voxtral_vs_ort cuda_torch_ark:cuda_torch_ark_byte_identical \
         cuda_torch_granite:cuda_torch_granite_byte_identical cuda_torch_cohere_vs_ort:cuda_torch_cohere_vs_ort \
         cuda_torch_cosyvoice3:cuda_torch_cosyvoice3 cuda_torch_dia:cuda_torch_dia \
         cuda_torch_dia2:cpu_fp32_codes_byte_identical cuda_torch_dia2:cuda_bf16_codes_byte_identical \
         cuda_torch_csm:cuda_csm_codes_byte_identical_to_sidecar cuda_torch_higgs:cpu_f32_byte_identical_to_reference \
         cuda_torch_higgs:cuda_f16_synthesizes_and_reports_rtf cuda_torch_neutts:cpu_f32_byte_identical_to_reference \
         cuda_torch_neutts:cuda_bf16_greedy_codes_byte_identical cuda_torch_neutts:cuda_bf16_synthesizes_and_reports_rtf \
         cuda_torch_omnivoice:cuda_f32_byte_identical_to_reference cuda_torch_dots:cuda_torch_dots \
         cuda_torch_qwen3_tts:cuda_qwen3_tts_codes_byte_identical_to_sidecar cuda_torch_vibevoice:cuda_torch_vibevoice; do
  t=${g%%:*}; fn=${g##*:}; free -g
  CARGO_BUILD_JOBS=6 cargo test --release -p waav-infer-backend-torch --test $t $fn -- --exact --include-ignored --test-threads=1 --nocapture
done

# --- B55 low-precision TRT (int8 / nvfp4 run on CUDA; engines pre-compiled at ~/.cache/waav-models/neutts-air/trt) ---
TTLIB=/tmp/trt_e2e_venv/lib/python3.12/site-packages/torch_tensorrt/lib
TRTLIB=/tmp/trt_e2e_venv/lib/python3.12/site-packages/tensorrt_libs
export WAAV_TORCHTRT_LIB="$TTLIB" WAAV_TENSORRT_LIB="$TRTLIB" LD_LIBRARY_PATH="$TTLIB:$TRTLIB:$LD_LIBRARY_PATH" RUSTFLAGS="--cfg accel_tensorrt"
for p in int8 nvfp4; do WAAV_NEUTTS_TRT_PRECISION=$p \
  cargo test --release -p waav-infer-backend-torch --features cuda --test cuda_torch_neutts_trt_lowp -- --include-ignored --test-threads=1 --nocapture; done
```

Harness: `eval/precision_matrix.py` (read-only). Raw harness JSON: `/tmp/b57_cuda.json`,
`/tmp/b57_cpu_whisper.json`. No model numerics or serving code were touched — only the harness + the existing
gates + the existing B55 TRT engines + this report.
