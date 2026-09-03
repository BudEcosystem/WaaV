# ONBOARD — ResembleAI/resemble-enhance (stage-1 denoiser)

**Status: ONBOARDED** ✅ — real denoise via the engine, model-core byte-near-faithful to the reference
(corr 0.999999 / 57.4 dB), RTF 0.166 on GB10 CUDA. No per-venv serving path; reuses the existing
`Enhancer` + `Stft` + `EdgeResampler` seam.

Date: 2026-06-23 · Box: GB10 (NVIDIA GB10, sm_121, 121 GB unified) · `free -g` start: 42 GB free / 63 GB swap.

---

## 1. What it is

`ResembleAI/resemble-enhance` is a 2-stage A2A speech-restoration model (denoise + super-resolution):
a **CFM denoiser** (stage 1) and a much larger **enhancer/vocoder** (stage 2). The HF repo
(`ResembleAI/resemble-enhance`, **ungated, public**, verified via `HfApi.model_info`) ships only a
PyTorch/DeepSpeed checkpoint (`enhancer_stage2/ds/G/.../mp_rank_00_model_states.pt`) — **no ONNX**.

**Onboarded scope = the stage-1 denoiser**, which is the practical, high-value stage. The community
ONNX export's own README states the enhancer (stage 2) "is much larger and the quality is not much
better (and sometimes it adds weird artifacts)". The denoiser alone is a strong full-band 44.1 kHz
denoiser and fits the WaaV enhancement task family cleanly.

## 2. Acquire / export — no in-house export needed

A clean, PyTorch-free ONNX export of the denoiser already exists and was **HfApi-verified**:
- `cqchangm/resemble-enhance-onnx-inference-src` → `resemble-denoise-onnx-inference-master.zip`
  contains a prebuilt `denoiser.onnx` (42.7 MB, opset 17) + `run.py`/`denoiser.py` showing the exact
  pre/post-processing. "All PyTorch dependencies are removed."

Acquired to `~/.cache/waav-models/resemble-enhance/denoiser.onnx` (+ `SOURCE.txt` provenance). The
`aoiandroid/resemble-enhance-{fp16,int8}-quantized` repos are still `.pt` (PyTorch), not usable here.

**Graph I/O** (auto-detected): inputs `mag`/`cos`/`sin` `[1, 841, T]`, outputs `out_mag`/`out_cos`/`out_sin`
`[1, 841, T]`. 841 = n_fft/2+1 (n_fft = win = 1680, hop = 420). The graph takes the **magnitude +
decomposed phase** (cos/sin of the angle) of a librosa STFT and returns the cleaned spectrogram — a
whole-utterance UNet (not streaming/recurrent like GTCRN/DPDFNet).

## 3. Integrate — reused the enhance seam (`core/enhance.rs`)

Added a 4th auto-detected mode to the existing `Enhancer` (detected from input names `mag`+`sin`):

- **`Mode::Resemble { stft }`** — reuses `Stft::with_window_center(1680, 420, hann_window(1680))` for
  both the forward STFT and the inverse (librosa full-Hann, center/reflect; `Stft::inverse`'s
  window²-energy overlap-add normalization **is** `librosa.istft`). Reuses `EdgeResampler` for the
  16↔44.1 kHz hops so the engine keeps its uniform **16 kHz in / 16 kHz out** enhancement contract
  (same as GTCRN/dasheng/DPDFNet).
- Replicated the reference exactly: 441-sample pre-pad → STFT → **drop the last frame** (`s[..,:-1]`)
  → `mag = |s|`, `cos/sin = cos/sin(angle(s))` → graph → reconstruct `re=mag·cos, im=mag·sin` →
  **edge-pad the dropped frame back** → iSTFT → trim. Chunking matches `run()`: `num_chunks =
  ⌈len₄₄ / (16000·30)⌉` equal parts, each **peak-normalized** before the forward and re-scaled after.

New shared helper: **`components/stft.rs::hann_window(n)`** (periodic full Hann; the existing
`sqrt_hann` is private and wrong for this model). Also exposed a public **`Enhancer::resemble_chunk_44k`**
(native-44.1 k, already-normalized chunk) — a faithful native-rate entrypoint and the accuracy hook.

### Files touched
- `crates/waav-infer-core/src/enhance.rs` — `Mode::Resemble`, detection, `enhance_resemble`,
  `resemble_chunk`, `resemble_chunk_44k`, consts, module doc, 3 unit tests. **(enhance-family file —
  shared with concurrent agents; appended additively, no existing arms changed.)**
- `crates/waav-infer-components/src/stft.rs` — new `pub fn hann_window`. **(SHARED component.)**
- `crates/waav-infer-components/src/lib.rs` — export `hann_window`. **(SHARED — re-export line.)**
- `crates/waav-infer-server/src/bin/waav_infer.rs` — enhance CLI doc comments (GTCRN→auto-detected).
- `gui/app.py` — registered `resemble-enhance` in the enhancement model list.
- `~/.cache/waav-models/resemble-enhance/{denoiser.onnx, SOURCE.txt}` — acquired model.

No `waav.json`/registry entry is needed: enhancers load by direct ONNX path (`load_enhancer`), exactly
like the other denoisers. **No per-venv serving path** (the pip pkg / python onnxruntime were used only
for the build-time golden capture).

## 4. Smoke — real denoise via the engine

```
waav-infer enhance re_noisy_16k.wav --model …/resemble-enhance/denoiser.onnx --ep auto
→ loaded enhancement model (EP=cuda); enhanced 12.05s audio in 2002ms (RTF 0.166)
```
Output sane (RMS 0.074, peak 0.77, non-trivial). CUDA vs CPU agree to 3e-5 (deterministic).

## 5. Accuracy — vs the reference engine (same ONNX in python onnxruntime)

**Model core (44.1 k, resamplers bypassed — the true faithfulness test):** fed the *identical*
peak-normalized 44.1 k chunk to both the WaaV Rust core and the reference, comparing the
mag/cos/sin → graph → iSTFT path:

| metric | value |
|---|---|
| Pearson corr (Rust vs ref) | **0.999999** |
| SNR vs reference | **57.40 dB** |
| max abs sample diff | 0.00094 |

→ The WaaV STFT/phase/iSTFT replication is **byte-near-identical** to librosa/scipy + the reference;
the sub-mdB residual is pure f32 realfft-vs-librosa rounding. Integration is correct.

**End-to-end (16 k in/out, includes the EdgeResampler↔librosa-kaiser difference):** log-mel L1
Rust-16k vs reference-16k = **2.16 dB** (RMS 0.074 vs 0.078). The remaining gap is **entirely the
resampler** (model core is 57 dB faithful); `EdgeResampler` (linear-up / sinc-down) ≠ librosa
`kaiser_best`. A native-44.1 k caller (via `resemble_chunk_44k`) gets the full 57 dB fidelity.

**Denoising actually works** (waveform corr is the wrong metric — resemble is phase-reconstructing, so
sample-level corr is low even when clean). Perceptual log-mel L1 vs the clean reference:
noisy 25.79 dB → **denoised 16.52 dB** (≈9.3 dB improvement), matching the reference's behavior.

## 6. Perf — GB10

| EP | RTF (12.05 s clip, full pipeline) |
|---|---|
| **CUDA (GB10)** | **0.166** (~6× real-time) |
| CPU | 0.221 |
| reference python onnxruntime (CPU) | 0.287 |

Model-core only (no resample): RTF 0.165 CUDA. Memory stable — no spike, 38 GB free after (42 MB
model; many shape ops fall to CPU EP, expected and harmless on unified memory).

## 7. Gates

- `cargo test -p waav-infer-core --lib` → **78 passed / 0 failed** (75 prior + 3 new resemble tests).
- `cargo clippy -p waav-infer-core -p waav-infer-components -p waav-infer-server --bin waav-infer` → clean.
- Temporary `re-core-eval` bin used for the byte-faithfulness capture was **removed**; the permanent
  `resemble_chunk_44k` public method remains as the native-44.1 k entrypoint.

## Honest caveats

- **Stage-2 enhancer not onboarded** — it's PyTorch-only (no ONNX export exists), much larger, and the
  export author flags it as low-marginal-quality / artifact-prone. The denoiser is the right scope.
- **16 k contract loses super-resolution headroom** — resemble denoises full-band 44.1 k, but the
  engine's enhancement contract is 16 k, so HF content >8 kHz is band-limited at the boundary. Native
  44.1 k fidelity is reachable via `resemble_chunk_44k`; a native-rate enhance contract is a future
  enhancement (out of scope here).
- End-to-end 16 k fidelity is 2.16 dB log-mel from the reference **only because of the resampler**, not
  the model. If exact reference parity at 16 k is ever required, swap `EdgeResampler` for a
  kaiser-windowed-sinc rate converter.
