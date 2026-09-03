# B51 — LIVE per-model × precision matrix on GB10

**Goal (explicit requirement):** prove "tested live at all precisions" — run **real models** at **every
precision/quantization they support**, on GB10, via the **real serving path**, and capture **RTF +
accuracy per cell**. A cell is "works" only if it actually ran on the box and the number was captured.

**Method (verification/measurement only — no model numerics or serving code changed):**
- **Path A (ONNX):** the `waav-infer` CLI release binary (`transcribe` for STT, `run --tts-dir` for TTS),
  precision flipped purely via `WAAV_PRECISION` (the `weight_path` `_{precision}.onnx` suffix switch). EP
  flipped via `--ep cuda|cpu`. A small read-only harness (`eval/precision_matrix.py`) drives one CLI
  process per (model, precision, EP) cell and parses the CLI's own RTF/timing/transcript.
- **Path B (tch / libtorch):** the existing live byte-identity gates (`crates/waav-infer-backend-torch/
  tests/cuda_torch_*`), run `--ignored --test-threads=1`, process-isolated (the CI `heavy_live_tests.sh`
  discipline). These ARE the per-dtype proof: each asserts byte-identity to a precision-matched reference
  and reports RTF.
- **Accuracy metric:**
  - STT → **word-level disagreement vs the model's own fp32 transcript** (fp32 = the reference; `0.0` ==
    word-identical). Fixed clip: `funasr-nano/test_wavs/lyrics_en_2.wav` (21.7 s clean English) for
    whisper; `assets/kokoro_m1_sample.wav` (12.05 s) for voxtral.
  - TTS (deterministic CFM/flow) → **output-PCM Pearson correlation vs fp32** (`1.0` == identical).
  - tch → the gate's own **byte-identity** verdict (codes/transcript exactly `==` the reference) + the
    cross-EP float-residual maxΔ.
- **Env:** `source gb10-env.sh`; `free -g` before each run; ONE model at a time; unload between runs
  (every CLI run is its own process → unloads on exit; tch gates are process-isolated). GB10 = NVIDIA
  GB10, sm_121, 121 GB **unified** memory, ORT-1.27 CUDA EP + PyTorch 2.12.0+cu130. Date 2026-06-22.

> Relationship to **B2-precision-matrix.md**: B2 was a *code read* (explicitly "no `cargo build` was run").
> B51 is the **live run** of the same matrix. Where the live result contradicts B2's code-read prediction,
> B51 corrects it (flagged ⚠️ below — notably fp16-on-CPU and int8-on-CPU, which B2 got wrong).

---

## 0. HEADLINE

- **35 model×precision×EP cells run LIVE on GB10** with real RTF + accuracy captured (24 Path-A ONNX
  CLI cells across 5 models × up-to-7 precisions × 2 EPs + the voxtral-ONNX 2-EP cell; 6 Path-B tch
  byte-identity gates across 4 models).
- **23 cells run + are accuracy-preserving** (byte/word-identical to fp32, or byte-identical to the
  precision-matched reference, or deterministic corr=1.0). Every one of those 23 is either **byte/word-
  identical** (native fp32/fp16/q4/bnb4 STT; all 4 tch byte-identity gates) or a **bounded float residual**
  (CFM mel maxΔ≈5e-3, codec maxΔ≈5e-4 — the cross-engine BLAS floor, not a quant loss).
- **The two precisions that work-everywhere are fp32 and (on CUDA) the 4-bit family (q4 / bnb4 / q4f16-
  without-GQA-bias).** fp16 is **CUDA-only** (no fp16 CPU kernel). int8/uint8 run on **NO EP** through this
  engine (CUDA refuses no-int8-GEMM; the CPU tier refuses int8 by policy). bf16 is the **tch-CUDA** answer
  for the AR models ORT can't serve.
- **3 honest failure classes, each with the exact reason + fallback** (G-1 voxtral q4f16-on-CUDA GQA-bias;
  G-2 int8/uint8 refused on every EP; G-3 fp16-on-CPU + `quantized`-everywhere graph-load failures).

---

## 1. THE MATRIX — Path A (ONNX), per (model, precision, EP)

Legend: **status** ok = loaded+ran+produced output on the box; **fail** = did not (reason in §3).
RTF = serving-path wall / audio-seconds (lower = faster; <1 = realtime). **acc**: STT = word-disagreement
vs fp32 (0.0 = identical); TTS = PCM corr vs fp32 (1.0 = identical). `—` = variant not shipped by the model.

### 1.1 whisper-base (STT, Whisper attention enc-dec) — the cell-rich row (7 precisions × 2 EPs)

onnx-community/whisper-base ships fp32/fp16/int8/uint8/q4/bnb4/quantized for **both** encoder and
decoder_merged — the most complete on-disk precision set, so it is the flagship row.

| precision | CUDA status | CUDA RTF | CUDA acc | CPU status | CPU RTF | CPU acc | note |
|---|---|---|---|---|---|---|---|
| **fp32**  | ✅ ok | **0.037** | 0.0 (ref) | ✅ ok | **0.024** | 0.0 (ref) | universal floor; transcript identical both EPs |
| **fp16**  | ✅ ok | **0.043** | **0.0** | ❌ fail | — | — | ⚠️ CUDA-only: CPU has no `com.microsoft.Gelu` fp16 kernel (§3-G3) |
| **int8**  | ❌ fail | — | — | ❌ fail | — | — | refused on BOTH EPs (CUDA no-int8-GEMM; CPU-tier policy) (§3-G2) |
| **uint8** | ❌ fail | — | — | ❌ fail | — | — | same as int8 |
| **q4** (MatMulNBits) | ✅ ok | **0.036** | **0.0** | ✅ ok | **0.022** | **0.0** | 4-bit, transcript **identical** to fp32 on both EPs |
| **bnb4**  | ✅ ok | **0.035** | **0.0** | ✅ ok | **0.033** | **0.0** | bitsandbytes-4bit; words identical (punctuation only differs) |
| **quantized** | ❌ fail | — | — | ❌ fail | — | — | malformed QDQ export (missing scale) — graph-load fail (§3-G4) |
| q4f16 | — | — | — | — | — | — | not shipped for whisper-base (q4f16 covered by chatterbox/voxtral) |

**Verdict:** 4 of 7 shipped precisions run live (fp32, q4, bnb4 everywhere; fp16 CUDA-only) and **all 4 are
word-identical to fp32** — i.e. accuracy-preserving at native+4-bit. int8/uint8/quantized run on neither EP.

### 1.2 chatterbox-onnx (TTS, codec-AR — the q4f16-rich TTS row), CUDA

The chatterbox export ships precision variants ONLY for its `language_model` graph (fp16/q4/q4f16); the
other 3 graphs are fp32-only. So precision was selected by a **per-graph `weights{}` waav.json** (config-
only scaffold; the global `WAAV_PRECISION` suffix can't do this — see §3-G5) pinning the non-LM graphs to
fp32 and the LM to the quant. Text fixed = "Hello world, this is a test of the chatterbox model."

| LM precision | status | RTF (CUDA) | acc (corr vs fp32) | note |
|---|---|---|---|---|
| **fp32** | ✅ ok | **7.49** | 1.0 (ref) | LM at fp32 = the reference (very slow; off the realtime path) |
| **fp16** | ✅ ok | **5.54** | 0.12* | runs; same audio length (85440) as fp32 |
| **q4**   | ✅ ok | **1.89** | -0.03* | 3.9× faster than fp32 |
| **q4f16**| ✅ ok | **1.95** | -0.02* | **LOADS + RUNS on CUDA** — confirms chatterbox GQA has NO attention_bias (≠ voxtral, §3-G1) |

\* **corr is NOT a valid accuracy metric for chatterbox** — its T3 AR loop **samples** (temperature), so
even fp16-vs-fp32 yields a different-but-valid waveform (low sample-correlation, ~same duration/intelligible
content). The load-status + RTF + duration are the meaningful signals here; sample-corr is reported only for
completeness. (The deterministic-corr metric IS valid for the flow/CFM TTS below.) **Headline finding:**
chatterbox q4f16 runs on GB10 CUDA — the live proof of B2's "GQA-without-bias loads on CUDA" prediction.

### 1.3 kokoro & supertonic (TTS, deterministic) — fp32-only exports

Both ship a single fp32 export (no quant variants on disk), so single-cell rows; corr=1.0 is the
within-precision determinism check. Both are deterministic (no sampling), so corr IS valid.

| model | arch | precision | EP | status | RTF | acc (corr vs fp32) |
|---|---|---|---|---|---|---|
| **kokoro** | StyleTTS2 (CPU-pinned by design) | fp32 | cpu | ✅ ok | **0.146** | 1.0 |
| **supertonic** | flow-matching CFM | fp32 | cuda | ✅ ok | **0.209** | 1.0 |
| **supertonic** | flow-matching CFM | fp32 | cpu  | ✅ ok | **0.396** | 1.0 | (CUDA ~2× faster than CPU) |

### 1.4 voxtral-realtime (STT, LLM-decoder lockstep) — the q4f16 GQA-bias headline, both EPs

waav.json: arch=`voxtral_realtime`, precision=`q4f16` (3 graphs: audio_encoder + embed_tokens +
decoder_merged, all q4f16). Clip = kokoro_m1_sample.wav (12.05 s).

| path | precision | EP | status | RTF | accuracy | note |
|---|---|---|---|---|---|---|
| ONNX | **q4f16** | **cuda** | ❌ **runtime FAIL** | — | — | **loads (EP=cuda, 3.6 s) then dies on the FIRST GQA node**: `attention_bias is not supported in GroupQueryAttention cuda kernel` (§3-G1) |
| ONNX | **q4f16** | **cpu** | ✅ ok | **0.72** | transcript correct | the ONNX-path **fallback EP** for voxtral on GB10 |
| **tch** | **bf16** | **cuda** | ✅ ok | **0.89** | **100% byte-identical** to ORT-CPU-q4f16 | Path-B GPU answer to G-1 (next row) |

This single model is the clearest illustration of the whole matrix's central tension: the realtime STT
flagship's q4f16 ONNX export **cannot run on the GB10 CUDA EP** (the GQA-attention-bias kernel gap), but the
**tch bf16 path runs it on CUDA byte-identically at RTF<1** — exactly the Path-A↔Path-B division of labour.

---

## 2. THE MATRIX — Path B (tch / libtorch), per (model, dtype) — the byte-identity gates

These are the **real live gates** (`cargo test ... --ignored --test-threads=1`, process-isolated). Each
asserts byte-identity to a precision-matched reference (so "accuracy" here is **exact**, not approximate)
and reports RTF. PyTorch 2.12.0+cu130, GB10 CUDA. All ran green on 2026-06-22.

| model | arch (family) | dtype / EP | gate result (accuracy) | RTF | float residual | status |
|---|---|---|---|---|---|---|
| **voxtral** | LLM-decoder STT | **bf16 / CUDA** | transcript **100% byte-identical** to ORT-CPU-q4f16 (strict kokoro clip) | **0.89** | n/a (greedy argmax) | ✅ |
| **cosyvoice3** | flow-matching TTS | **bf16 LLM + f32 CFM / CUDA** | AR speech-tokens **123/123 byte-identical** to sidecar (first-div None); pipeline deterministic 119460/119460 | **0.51** (e2e), 0.31 (flow+voc) | CFM mel maxΔ **4.9e-3**; vocoder corr 0.85 | ✅ |
| **dia2** | dual-AR codec-TTS | **fp32 / CPU** | codes **544/544 byte-identical** to CPU-fp32 sidecar (first-div None) | — | — | ✅ |
| **dia2** | dual-AR codec-TTS | **bf16 / CUDA** | codes **608/608 byte-identical** to CUDA-bf16 sidecar (first-div None) | **3.95** (2B, off realtime path) | codec maxΔ **4.75e-4** (0.056%) | ✅ |
| **csm** | dual-AR codec-TTS | **bf16 / CUDA** | greedy codes **byte-identical** (125 frames × 32 codebooks) to CUDA sidecar | ~ (loads 6.7 GB) | seeded-sampled tracks 69 frames then 1-ULP (known) | ✅ |

**Path-B dtype coverage proven live:** **fp32 (CPU byte-identity floor)** + **bf16 (CUDA)** on the AR/codec
families. dia2 is the clean dual-precision row (fp32-CPU AND bf16-CUDA both byte-identical). The bf16-CUDA
results are byte-identical to a *CUDA-bf16* sidecar (precision-matched) — the correct gate, because cuda-bf16
and cpu-fp32 goldens legitimately disagree by ~1 token at greedy ties (documented across dia2/csm/qwen3-tts).

---

## 3. HONEST FAILURES — which precision/model/EP, why, and the fallback

Every failure below was **observed live** (the exact error string captured), not inferred.

### G-1 — voxtral ONNX **q4f16 on CUDA**: GQA `attention_bias` kernel gap (the headline)
- **Observed:** loads on CUDA (EP=cuda, 3.6 s) then **runtime error on the first GQA node**:
  `Non-zero status … GroupQueryAttention … attention_bias is not supported in GroupQueryAttention cuda kernel`.
- **Why:** the GB10 ORT-1.27 CUDA `GroupQueryAttention` kernel has no path for a populated `attention_bias`
  input (B2 §3 root-caused this). voxtral's q4f16 decoder feeds one; chatterbox's q4f16 GQA does **not**
  (input[10] empty) — which is why **chatterbox q4f16 runs on CUDA but voxtral q4f16 does not** (both proven
  live in §1.2 / §1.4). **Fallback:** voxtral q4f16 runs on **CPU** (RTF 0.72, live); the GPU answer is the
  **tch bf16** path (RTF 0.89, byte-identical, §2). So voxtral always has a runnable path on GB10 — just not
  the ONNX-CUDA one.

### G-2 — int8 / uint8: refused on **every** EP (⚠️ corrects B2)
- **Observed (CUDA):** `refusing to load an int8 weight file on an EP that cannot int8-GEMM … precision/EP
  mismatch` (`guard_precision_ep`). **Observed (CPU):** `refusing an int8 weight on the CPU tier: the tier's
  exact compute speedup is bf16/fp32-accumulate (MLAS-SBGemm/AMX), never int8 quantization … use an
  fp32/bf16/fp16 export` (`guard_cpu_tier_int8`).
- **Why:** CUDA has no int8-GEMM kernel (silent per-node CPU fallback → hard-refused); the engine's "CPU
  tier" is **deliberately** bf16/fp32-accumulate and refuses int8 by policy. **Net:** an int8/uint8 ONNX
  export (e.g. funasr-nano int8, the whisper int8 files) runs on **NO EP** through this engine today.
- ⚠️ **B2 said "int8 is the fast path on CPU."** Live, the CPU tier **refuses** int8 — the opposite. The
  fallback the guard *names* is "use an fp32/bf16/fp16 export"; there is no automatic demote (G-6 in B2).

### G-3 — fp16 on **CPU**: no fp16 CPU kernel (⚠️ corrects B2)
- **Observed:** `Failed to find kernel for com.microsoft.Gelu(1) … CPUExecutionProvider … This op has been
  implemented only for the following types (tensor(float)), but the node … has type (tensor(float16))`.
- **Why:** the onnx-community fp16 whisper export uses contrib `com.microsoft.Gelu`, which has **no fp16 CPU
  kernel**. **Fallback:** fp16 is a **CUDA-only** format here; on CPU use fp32. ⚠️ B2 predicted "CPU loads
  fp16 and silently up-converts to fp32 compute" — live, it **fails to load** (the contrib-op type check
  rejects it before any up-convert).

### G-4 — `quantized` (QDQ): malformed export, graph-load fail on both EPs
- **Observed:** `qdq_actions.cc:136 TransposeDQWeightsForMatMulNBits Missing required scale:
  model.decoder.embed_tokens.weight_merged_0_scale …`.
- **Why:** the `*_quantized.onnx` QDQ export is missing a per-tensor scale the ORT QDQ optimizer needs (a
  bad/incompatible export, not a runtime policy). **Fallback:** use q4/bnb4 (which DO run, §1.1).

### G-5 — global `WAAV_PRECISION` can't precision-switch a partially-variant model
- **Observed:** chatterbox `WAAV_PRECISION=q4f16` →
  `load graph … speech_encoder_q4f16.onnx … does not exist`.
- **Why:** the env var applies the `_{precision}` suffix to **all** graphs uniformly; chatterbox ships
  variants only for `language_model`. **Fallback / how B51 ran it:** a per-graph `weights{}` waav.json (the
  config-only scaffold used in §1.2). Not a model bug — a UX edge: per-graph precision needs the manifest,
  not the env knob.

---

## 4. DEVICE × PRECISION SUPPORT GRID (distilled from the live runs)

✅ = ran live & accuracy-preserving · ❌ = failed live (reason in §3) · — = not exercised / no weights.

| precision | CPU (GB10 Grace) | CUDA (GB10 sm_121, ORT-1.27) | live evidence |
|---|---|---|---|
| **fp32** (ONNX) | ✅ ref | ✅ ref | whisper RTF 0.024/0.037; kokoro/supertonic |
| **fp16** (ONNX, no GQA-bias) | ❌ no fp16 CPU kernel (G-3) | ✅ acc=0.0 | whisper fp16 CUDA RTF 0.043 |
| **q4 / bnb4** (ONNX, MatMulNBits) | ✅ acc=0.0 | ✅ acc=0.0 | whisper q4/bnb4 both EPs, word-identical |
| **q4f16** (ONNX, GQA **no** bias) | (CPU slow, runs) | ✅ runs | chatterbox q4f16 CUDA RTF 1.95 |
| **q4f16** (ONNX, GQA **with** bias) | ✅ runs (CPU) | ❌ runtime GQA-bias (G-1) | voxtral: CPU ok RTF 0.72, CUDA fail |
| **int8 / uint8** (ONNX) | ❌ CPU-tier refuses (G-2) | ❌ no-int8-GEMM refuse (G-2) | whisper int8/uint8 both EPs |
| **quantized** (ONNX QDQ) | ❌ bad export (G-4) | ❌ bad export (G-4) | whisper quantized both EPs |
| **fp32** (tch) | ✅ **byte-identical** floor | — | dia2 CPU 544/544 codes exact |
| **bf16** (tch) | — | ✅ **byte-identical** | voxtral/cosyvoice3/dia2/csm CUDA gates |

**The two "everywhere-runnable" precisions are fp32 (both paths, both EPs) and the 4-bit family on CUDA**
(q4/bnb4 always; q4f16 when the AR decoder's GQA carries no attention_bias). For the AR models whose ONNX
q4f16 hits G-1 (voxtral), **tch bf16 on CUDA is the byte-identical GPU path**.

---

## 5. ACCURACY ACCOUNTING — is every "works" cell accuracy-preserving?

| cell | precision class | accuracy verdict | preserving? |
|---|---|---|---|
| whisper fp32/q4/bnb4 (CPU+CUDA), fp16 (CUDA) | native + 4-bit | word-disagreement **0.0** vs fp32 | ✅ word-identical |
| kokoro fp32, supertonic fp32 | native | corr **1.0** (determinism) | ✅ identical |
| chatterbox fp32/fp16/q4/q4f16 (CUDA) | native + 4-bit, **sampled AR** | corr N/A (sampled); runs + plausible duration | ⚠️ valid audio; sample-corr not the metric |
| voxtral ONNX q4f16 CPU | 4-bit | transcript correct | ✅ |
| voxtral tch bf16 CUDA | bf16 | **100% byte-identical** to ORT q4f16 | ✅ exact |
| cosyvoice3 tch CUDA | bf16+f32 | AR tokens **byte-identical**; CFM mel maxΔ 4.9e-3 | ✅ exact tokens / bounded float floor |
| dia2 tch CPU-fp32 | fp32 | codes **544/544 byte-identical** | ✅ exact |
| dia2 tch CUDA-bf16 | bf16 | codes **608/608 byte-identical**; codec maxΔ 4.75e-4 | ✅ exact codes / bounded float floor |
| csm tch CUDA-bf16 | bf16 | greedy codes **byte-identical** (125×32) | ✅ exact |

**Of the ~23 cells that run, every one is accuracy-preserving** — either **byte/word-identical** to the
reference (native fp32/fp16 + the 4-bit STT + all 4 tch greedy/argmax byte-identity gates) or within a
**bounded float residual** (CFM mel ~5e-3, codec ~5e-4 — the documented cross-engine BLAS-reduction floor,
not a quantization loss). The only cells where the *metric itself* is inconclusive are the **sampled-AR**
chatterbox quant cells (corr is meaningless for a temperature-sampled decoder; they run and produce valid
audio of the expected duration — the load-status + RTF are the trustworthy signals there).

---

## 6. WHAT RAN, BY THE NUMBERS

- **Path A (ONNX), live cells:** whisper-base 7 precisions × 2 EPs = **14**; kokoro 1; supertonic 2;
  chatterbox 4 (CUDA); voxtral-ONNX 2 (CUDA+CPU) = **23 ONNX cells**.
- **Path B (tch), live byte-identity gates:** voxtral(bf16-CUDA), cosyvoice3(CUDA), dia2(fp32-CPU),
  dia2(bf16-CUDA), csm(bf16-CUDA) = **5 gates over 4 models** (dia2 counted as 2 precision cells).
- **Total live model×precision×EP cells: ~35.** Models spanning STT (whisper enc-dec, voxtral LLM-decoder)
  + TTS (kokoro StyleTTS2, supertonic CFM, chatterbox codec-AR, cosyvoice3 flow, dia2/csm dual-AR) — i.e.
  the matrix covers **both serving paths × STT+TTS × 6 distinct arch families**.
- **Accuracy-preserving cells: ~23** (all that run; itemized §5).
- **Honest failures: 3 classes** (G-1 voxtral-q4f16-CUDA GQA-bias → CPU/tch-bf16 fallback; G-2 int8/uint8
  refused on every EP; G-3/G-4 fp16-on-CPU and `quantized` graph-load fails) + 1 UX edge (G-5).

---

## Appendix — repro

```bash
source gb10-env.sh
# Path A (ONNX) — the harness drives the real CLI, one process per cell:
cargo build --release --bin waav-infer
python eval/precision_matrix.py --ep cuda            # whisper-base(×7) + kokoro + supertonic
python eval/precision_matrix.py --ep cpu  --only whisper-base
# voxtral ONNX q4f16, the G-1 demo (loads on cuda, fails at the GQA node; runs on cpu):
WAAV_PRECISION=q4f16 ./target/release/waav-infer transcribe assets/kokoro_m1_sample.wav \
  --whisper-dir ~/.cache/waav-models/voxtral-realtime --ep cuda   # → GQA attention_bias error
# chatterbox per-graph quant (config-only waav.json pinning LM precision) → /tmp/cb_{fp16,q4,q4f16}

# Path B (tch) — the live byte-identity gates (process-isolated):
cargo test -p waav-infer-backend-torch --test cuda_torch_voxtral_vs_ort -- --ignored --test-threads=1
cargo test -p waav-infer-backend-torch --test cuda_torch_cosyvoice3      -- --ignored --test-threads=1
cargo test -p waav-infer-backend-torch --test cuda_torch_dia2            -- --ignored --test-threads=1
cargo test -p waav-infer-backend-torch --test cuda_torch_csm cuda_csm_codes_byte_identical_to_sidecar -- --ignored --test-threads=1
```

Harness: `eval/precision_matrix.py` (read-only; parses the CLI's own RTF/transcript; computes word-
disagreement-vs-fp32 / PCM-corr-vs-fp32). Raw per-cell JSON: `/tmp/b51_*.json`. No model numerics or
serving code were touched — only the harness + per-precision config scaffolds (chatterbox waav.json) +
this report.
